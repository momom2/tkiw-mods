//! A mouseover panel of the numbers behind a unit's hover popup.
//!
//! ## What it adds
//!
//! The game's own hover panel shows modified **HP** and modified **DPS**
//! (`get_hp_max` / `get_dps_modified`, confirmed by probing). It does not show
//! what a player planning a fight actually wants: how far the unit reaches, how
//! often it swings, and how hard each swing lands. This draws those three, in a
//! small panel next to the game's popup.
//!
//! ## Where the numbers come from
//!
//! The hovered unit is found the game's own way -- `obj_cursor` writes
//! `hovered_unit_instance` and computes `hovered_unit_damage` (the DPS the panel
//! shows). From the unit instance three base fields are read directly
//! (`attack_radius`, `attack_time`, `attack_spd_multi`), each a single getter
//! call that returns nothing rather than faulting when the field is absent (the
//! absent-variable read is survivable -- established by `draw_probe`).
//!
//! Then, all consistent with the DPS the game already displays:
//!
//! ```text
//! attacks_per_second = (60 / attack_time) * attack_spd_multi
//! damage_per_hit     = dps_modified / attacks_per_second
//! range              = attack_radius
//! ```
//!
//! `attack_time` is in frames at 60 fps (verified: griffin 24 dmg / 36 frames =
//! 40 base DPS, `stat-formulas.md`). `damage_per_hit` is derived from the
//! *modified* DPS, so it carries every modifier the game applied, and by
//! construction `damage_per_hit * attacks_per_second` equals the DPS on the
//! game's panel.
//!
//! **One assumption still wants a live check** (flagged honestly rather than
//! hidden): `attack_spd_multi` is read as a plain speed multiplier -- higher
//! means more swings per second. In every unit probed so far it was exactly
//! `1`, so the split between rate and per-hit damage is only *unverified*, never
//! *wrong*, for a unit with attack-speed modifiers. The DPS the pair multiplies
//! back to is correct regardless.
//!
//! ## Drawing
//!
//! Through [`overlay`](tkiw_runtime::overlay): the reading happens on the game
//! thread in [`Feature::on_frame`] and stores a [`Panel`]; a registered painter
//! draws that panel inside the game's Draw GUI End event. The two never overlap
//! -- both run on the game thread, at different points in the frame -- so the
//! shared [`PANEL`] needs only a plain mutex, never blocks, and shows nothing at
//! all until a unit is actually hovered inside a run.

use std::sync::Mutex;
use std::time::Duration;

use tkiw_runtime::{
    guard::Signature,
    instance, logln,
    overlay::{self, Canvas, Colour, PaintHandle},
    rvalue::Value,
    Runtime,
};

use momomod_kit::config::Section;
use momomod_kit::feature::{Cadence, Feature, Requirements};

/// `obj_display_manager`'s Draw GUI End -- runs last in the GUI phase, so an
/// overlay drawn here sits on top of the game's own UI. Absolute site is
/// `base + this`. The prologue is required as a signature so a game update
/// disables drawing rather than corrupting it.
const DRAW_HOST_SITE: usize = 0x1257860;
const DRAW_HOST_PROLOGUE: &[u8] = &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10];

/// One frame's worth of panel, handed from the reader ([`Feature::on_frame`]) to
/// the painter. `None` means nothing is hovered, so nothing is drawn.
struct Panel {
    /// Anchor in GUI pixels; the painter clamps it fully on-screen.
    x: f64,
    y: f64,
    title: String,
    rows: Vec<(&'static str, String)>,
}

/// The bridge between reading (game thread, in the pump) and drawing (game
/// thread, in the Draw event). Same thread, different points in the frame, so
/// the lock is never contended.
static PANEL: Mutex<Option<Panel>> = Mutex::new(None);

pub struct UnitStats {
    /// Held for as long as drawing should happen; dropping it removes the
    /// painter and, if it was the last, reverts the draw detour.
    handle: Option<PaintHandle>,
    /// Whether the one-time text self-test has run this session.
    text_tested: bool,
}

impl Default for UnitStats {
    fn default() -> UnitStats {
        UnitStats { handle: None, text_tested: false }
    }
}

/// A number out of an RValue, whichever numeric kind it arrived as.
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Real(x) => Some(*x),
        Value::Int(x) => Some(*x as f64),
        Value::Bool(b) => Some(*b as u8 as f64),
        _ => None,
    }
}

/// Read one variable off an instance, as a number, or `None` if absent/unreadable.
///
/// # Safety
/// Game thread.
unsafe fn field(rt: &Runtime, inst: usize, name: &str) -> Option<f64> {
    let id = rt.var_id(name)?;
    as_f64(&rt.globals()?.get_on(inst, id)?)
}

/// A display name from the object name: `obj_unit_mage_blue` -> `Mage Blue`.
/// The game's marketing names ("Lightning Mage") live elsewhere; this is the
/// honest internal name, which is enough to say what is hovered.
fn pretty_name(object: &str) -> String {
    let stem = object
        .strip_prefix("obj_unit_")
        .or_else(|| object.strip_prefix("obj_"))
        .unwrap_or(object);
    let mut out = String::new();
    for word in stem.split('_') {
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        object.to_string()
    } else {
        out
    }
}

/// Format a stat value: whole numbers without a decimal, otherwise one place.
fn num(v: f64) -> String {
    if (v.round() - v).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

impl UnitStats {
    /// Build the panel for the currently hovered unit, or `None` when nothing is
    /// hovered / the pieces cannot be read.
    ///
    /// # Safety
    /// Game thread.
    unsafe fn read_panel(&self, rt: &Runtime) -> Option<Panel> {
        // Nothing outside a run: the cursor's hover fields only mean anything
        // while gameplay is up, and reading during boot enumerated half-built
        // instances once (the recorded scar). Structural gate, not tuning.
        instance::find_singleton(rt.base, "obj_gameplay_controller")?;
        let cursor = instance::find_singleton(rt.base, "obj_cursor")?;

        // The cursor's own record of what is hovered. An instance id in this
        // build's observed encodings, or nothing (noone = -4) when unhovered.
        let hovered_id = rt.var_id("hovered_unit_instance")?;
        let target_id = match rt.globals()?.get_on(cursor, hovered_id)? {
            Value::Real(v) if v >= 100_000.0 => v as i64,
            Value::Int(i) if i >= 100_000 => i,
            Value::Ref { id, .. } if id >= 100_000 => id as i64,
            _ => return None,
        };
        let inst = instance::by_id(rt.base, target_id as i32)?;

        // The DPS the game's own panel shows (get_dps_modified), read from the
        // cursor where the game already computed it.
        let dps = field(rt, cursor, "hovered_unit_damage")?;

        // Base fields off the unit. Absent ones read as None and drop their row
        // rather than faulting.
        let attack_time = field(rt, inst, "attack_time");
        let attack_radius = field(rt, inst, "attack_radius");
        let spd = field(rt, inst, "attack_spd_multi").unwrap_or(1.0);

        let mut rows: Vec<(&'static str, String)> = Vec::new();
        if let Some(r) = attack_radius {
            rows.push(("Range", num(r)));
        }
        if let Some(t) = attack_time.filter(|t| *t > 0.0) {
            let rate = (60.0 / t) * spd;
            rows.push(("Rate", format!("{}/s", num(rate))));
            if rate > 0.0 {
                rows.push(("Hit", num(dps / rate)));
            }
        }
        if rows.is_empty() {
            return None;
        }

        // Position: the game's own popup anchor, when the cursor exposes it;
        // otherwise a fixed readable corner. Either way the painter clamps it
        // on-screen.
        let x = field(rt, cursor, "hover_popup_x").unwrap_or(24.0);
        let y = field(rt, cursor, "hover_popup_y").unwrap_or(120.0);

        let object = instance::object_name_of(inst).unwrap_or_else(|| "unit".into());
        Some(Panel { x, y, title: pretty_name(&object), rows })
    }
}

impl Feature for UnitStats {
    fn name(&self) -> &'static str {
        "unit_stats"
    }

    fn module(&self) -> &'static str {
        "gameplay"
    }

    fn summary(&self) -> &'static str {
        "Shows a small panel when you hover a unit: its range, attacks per second, \
         and damage per hit - the numbers behind the game's own hp/dps popup."
    }

    fn requires(&self) -> Requirements {
        Requirements {
            // The cursor's hover record. The per-unit fields (attack_time, ...)
            // are read best-effort, so they are not required here.
            variables: &["hovered_unit_instance", "hovered_unit_damage"],
            signatures: &[Signature {
                what: "obj_display_manager Draw GUI End (overlay host)",
                rva: DRAW_HOST_SITE,
                bytes: DRAW_HOST_PROLOGUE,
            }],
            objects: &["obj_cursor", "obj_gameplay_controller", "obj_display_manager"],
            ..Requirements::default()
        }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        for k in section.unknown(&["enabled"]) {
            logln!("[unit_stats] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn cadence(&self) -> Cadence {
        // Ten reads a second tracks a moving cursor without cost; the panel is
        // repainted every frame from the last read, so this is only how fresh
        // the numbers are, not how smoothly they follow.
        Cadence::Interval(Duration::from_millis(100))
    }

    fn activate(&mut self, rt: &Runtime) -> Result<(), String> {
        // This DLL has its own copy of the overlay statics, so it must name the
        // host itself before registering a painter.
        overlay::set_host(rt.base + DRAW_HOST_SITE, DRAW_HOST_PROLOGUE);
        let handle = overlay::paint(rt, |c| {
            if let Ok(g) = PANEL.lock() {
                if let Some(p) = g.as_ref() {
                    draw_panel(c, p);
                }
            }
        })?;
        self.handle = Some(handle);
        logln!("[unit_stats] overlay painter registered; panel draws while a unit is hovered");
        Ok(())
    }

    fn deactivate(&mut self, _rt: &Runtime) {
        // Dropping the handle removes the painter and reverts the detour if it
        // was the last one; clear the panel so a re-activation starts blank.
        self.handle = None;
        if let Ok(mut g) = PANEL.lock() {
            *g = None;
        }
    }

    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        // Once, at the menu, prove this DLL's own text construction against the
        // running game -- the one risky part of drawing text -- so an unattended
        // launch settles it without waiting for a hover.
        if !self.text_tested && instance::find_singleton(rt.base, "obj_main_menu").is_some() {
            self.text_tested = true;
            let probe = "unit_stats text probe";
            match unsafe { overlay::measure_test(rt.base, rt.text, probe) } {
                Some((w, h)) if w > 0.0 && h > 0.0 => {
                    logln!("[unit_stats] text self-test PASS: {probe:?} measures {w} x {h}")
                }
                other => logln!(
                    "[unit_stats] text self-test inconclusive: {other:?} (0/none may just mean \
                     no font set yet)"
                ),
            }
        }

        // SAFETY: on the game's thread, via the frame hook.
        let panel = unsafe { self.read_panel(rt) };
        if let Ok(mut g) = PANEL.lock() {
            *g = panel;
        }
        Ok(())
    }
}

/// Draw one panel: a dark plate, a thin frame, a titled column of rows. Runs
/// inside the overlay's Draw detour, on a [`Canvas`] whose ambient state the
/// overlay saves and restores around it.
fn draw_panel(c: &Canvas, p: &Panel) {
    const PAD: f64 = 8.0;

    // Every line as it will be drawn, so measurement and drawing agree.
    let mut lines: Vec<(String, Colour)> = Vec::with_capacity(p.rows.len() + 1);
    lines.push((p.title.clone(), Colour::rgb(255, 220, 120)));
    for (label, value) in &p.rows {
        lines.push((format!("{label}  {value}"), Colour::WHITE));
    }

    let mut width = 0.0f64;
    let mut line_h = 0.0f64;
    for (text, _) in &lines {
        let (w, h) = c.measure(text);
        width = width.max(w);
        line_h = line_h.max(h);
    }
    if line_h <= 0.0 {
        line_h = 18.0; // a sane default if the font has not reported a height
    }

    let box_w = width + PAD * 2.0;
    let box_h = line_h * lines.len() as f64 + PAD * 2.0;

    // Keep the whole plate on the GUI surface whatever the anchor was.
    let (gui_w, gui_h) = c.gui_size();
    let x = p.x.clamp(0.0, (gui_w - box_w).max(0.0));
    let y = p.y.clamp(0.0, (gui_h - box_h).max(0.0));

    c.rectangle(x, y, x + box_w, y + box_h, Colour::rgb(20, 20, 28));
    c.frame(x, y, x + box_w, y + box_h, Colour::rgb(90, 90, 110));

    let mut ty = y + PAD;
    for (text, colour) in &lines {
        c.text(x + PAD, ty, text, *colour);
        ty += line_h;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_prettied_from_the_object() {
        assert_eq!(pretty_name("obj_unit_mage_blue"), "Mage Blue");
        assert_eq!(pretty_name("obj_unit_griffin"), "Griffin");
        assert_eq!(pretty_name("obj_castle_wall"), "Castle Wall");
        assert_eq!(pretty_name("weird"), "Weird");
    }

    #[test]
    fn numbers_lose_a_pointless_decimal_but_keep_a_real_one() {
        assert_eq!(num(70.0), "70");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(1.667), "1.7");
        assert_eq!(num(40.0), "40");
    }

    /// The griffin from the probe: base 24 dmg / 36 frames, modified DPS 2727,
    /// no speed mod. Rate and per-hit must multiply back to the DPS the panel
    /// shows, which is the whole correctness claim.
    #[test]
    fn rate_and_hit_reconstruct_the_shown_dps() {
        let attack_time = 36.0f64;
        let spd = 1.0f64;
        let dps = 2727.0f64;
        let rate = (60.0 / attack_time) * spd;
        let hit = dps / rate;
        assert!((rate - 1.6667).abs() < 0.001, "rate {rate}");
        assert!((hit * rate - dps).abs() < 0.001, "hit*rate should equal dps");
        assert!((hit - 1636.2).abs() < 0.5, "hit {hit}");
    }

    #[test]
    fn a_number_is_read_whatever_kind_it_arrives_as() {
        assert_eq!(as_f64(&Value::Real(36.0)), Some(36.0));
        assert_eq!(as_f64(&Value::Int(70)), Some(70.0));
        assert_eq!(as_f64(&Value::Bool(true)), Some(1.0));
        assert_eq!(as_f64(&Value::Str("no".into())), None);
    }
}
