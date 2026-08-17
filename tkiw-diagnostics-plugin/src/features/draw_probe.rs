//! Everything the drawing detour needs to know, collected into one report.
//!
//! The kit cannot draw yet. The way through is a detour into a Draw event
//! (`analysis/gameplay-features.md`), and what stands between here and there is
//! information, not machinery: `popup_stutter_fix` already proved the cave, the
//! patch and a call from inside a live Draw event. This diagnostic gathers the
//! facts the detour has to be designed against, and writes them to a file that
//! can be read after an unattended `playtest.py` session:
//!
//! * **Which draw builtins resolve.** Our calls go through the runtime builtins
//!   (`draw_text`, `draw_set_font`, ...) with the convention proven in
//!   `calling-into-the-game.md`. Each one the detour would use is checked
//!   against the table and the live image.
//! * **The GUI metrics.** `display_get_gui_*` and friends, read live, because
//!   overlay layout needs them and they are not knowable from the executable.
//! * **The font table.** Font asset ids and names, enumerated with the game's
//!   own guard (`font_exists` before `font_get_name`), so `draw_set_font` can
//!   be given a real id rather than a guess.
//! * **The detour hosts.** Every object with a GUI-layer or begin/end Draw
//!   event, from the symbol table, with **live instance tracking per phase** --
//!   the host has to be an object that is reliably alive whenever we want to
//!   draw. Prologue bytes and function sizes are recorded so the patch site can
//!   be chosen offline, with a disassembler, rather than guessed here.
//!
//! ## The absent-variable experiment
//!
//! `analysis/gameplay-features.md` records one unresolved hazard: what the
//! instance-variable getter does when the variable is **not set** on that
//! instance. Everything downstream (hover panels, production sweeps) wants that
//! answer, and the honest way to get it is one deliberate call: one instance,
//! one variable proven absent by enumeration first, logged loudly *before* the
//! call so a dead session is itself the finding.
//!
//! That call is behind `absent_read = true`, **off by default**, so switching
//! the probe on is guaranteed safe and running the experiment is a decision.
//!
//! ## What this deliberately does not do
//!
//! It draws nothing and patches nothing. Every call it makes is a read-only
//! lookup, on the game's thread, from inside `PeekMessageW` -- the game is
//! provably not mid-Draw at that moment.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use tkiw_runtime::{
    builtin::{self, raw_of},
    findln, instance, logln, phase,
    rvalue::Value,
    Runtime,
};

use momomod_kit::config::Section;
use momomod_kit::feature::{Cadence, Feature, Requirements};

/// The builtins the drawing detour plans to call, per the conventions document.
/// Presence is reported, not required: a missing one shrinks the design space
/// and the report should say so rather than the probe refusing to run.
const NEEDED_BUILTINS: &[&str] = &[
    // state save/restore around our drawing
    "draw_get_font",
    "draw_get_colour",
    "draw_get_alpha",
    "draw_get_halign",
    "draw_get_valign",
    "draw_set_font",
    "draw_set_colour",
    "draw_set_alpha",
    "draw_set_halign",
    "draw_set_valign",
    // text and shapes
    "draw_text",
    "draw_text_transformed",
    "string_width",
    "string_height",
    "draw_rectangle",
    "draw_sprite_ext",
    // layout inputs
    "display_get_gui_width",
    "display_get_gui_height",
    "window_get_width",
    "window_get_height",
    // font census
    "font_exists",
    "font_get_name",
    "font_get_fontname",
];

/// Phase markers, in launch order; the current phase is the **last** one alive.
/// Same objects the timeline watches, for the same reason: cheapest honest
/// answer to "what is the game doing now".
const PHASES: &[(&str, &str)] = &[
    ("obj_init", "init"),
    ("obj_splash_screen", "splash"),
    ("obj_main_menu", "menu"),
    ("obj_gameplay_controller", "run"),
];

/// GameMaker's draw-event subtype numbers, as they appear in YYC function names
/// (`gml_Object_<obj>_Draw_<n>`).
fn event_label(subtype: u32) -> &'static str {
    match subtype {
        0 => "Draw",
        64 => "Draw GUI",
        65 => "Resize",
        72 => "Draw Begin",
        73 => "Draw End",
        74 => "Draw GUI Begin",
        75 => "Draw GUI End",
        76 => "Pre-Draw",
        77 => "Post-Draw",
        _ => "Draw_?",
    }
}

/// Subtypes worth tracking live: everything that runs once per frame regardless
/// of instance visibility economics -- the GUI layer and the begin/end/pre/post
/// brackets. Plain `Draw_0` objects are censused but not tracked: there are
/// hundreds, and a per-instance world-space event is the wrong detour host
/// anyway.
const TRACKED_SUBTYPES: &[u32] = &[64, 65, 72, 73, 74, 75, 76, 77];

/// Cap on live-tracked objects, so a surprising census cannot turn the tick
/// into a stall. Overflow is reported in the file, never silent.
const MAX_TRACKED: usize = 400;

/// `gml_Object_<obj>_Draw_<n>` -> `(<obj>, n)`. Nested functions (`@`) are not
/// events and are refused.
fn parse_draw_event(name: &str) -> Option<(&str, u32)> {
    let rest = name.strip_prefix("gml_Object_")?;
    if rest.contains('@') {
        return None;
    }
    let at = rest.rfind("_Draw_")?;
    let (obj, tail) = (&rest[..at], &rest[at + "_Draw_".len()..]);
    if obj.is_empty() || tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((obj, tail.parse().ok()?))
}

struct Tracked {
    /// subtype -> function rva, every draw event this object has.
    events: BTreeMap<u32, u32>,
    /// subtype -> first bytes of the function, hex, for offline detour analysis.
    prologue: BTreeMap<u32, String>,
    /// Phases in which a live instance was observed.
    seen_in: BTreeSet<&'static str>,
    alive_now: bool,
}

struct Census {
    tracked: BTreeMap<String, Tracked>,
    /// Objects with only a plain Draw_0, counted rather than tracked.
    draw0_only: usize,
    /// rva -> function length, from `.pdata`, for the tracked functions.
    sizes: HashMap<u32, u32>,
    scribble_fns: usize,
    /// Tracked-set overflow beyond [`MAX_TRACKED`], if any.
    dropped: usize,
}

#[derive(Default)]
struct Metrics {
    gui: Option<(i64, i64)>,
    window: Option<(i64, i64)>,
    display: Option<(i64, i64)>,
    /// Kept raw: in this runtime the current font may come back as an asset
    /// ref rather than a number, and the report should show what it actually is.
    font: Option<String>,
}

enum Absent {
    Off,
    /// Waiting for its preconditions, with the current blocker for the report.
    Pending(&'static str),
    Done(String),
}

pub struct DrawProbe {
    interval: Duration,
    fonts_max: u64,
    file: String,
    absent_object: String,
    absent_var: String,

    census: Option<Census>,
    fonts: Option<Vec<(u32, String)>>,
    fonts_diag: String,
    text_tested: bool,
    metrics: Metrics,
    absent: Absent,
    /// phase -> samples taken in it, so the report says how much each phase was seen.
    phase_samples: BTreeMap<&'static str, u32>,
    started: Option<Instant>,
    last_write: Option<Instant>,
    last_written: String,
}

impl Default for DrawProbe {
    fn default() -> DrawProbe {
        DrawProbe {
            interval: Duration::from_millis(1000),
            fonts_max: 64,
            file: "draw-probe.md".into(),
            absent_object: "obj_main_menu".into(),
            absent_var: "main_product".into(),
            census: None,
            fonts: None,
            fonts_diag: String::new(),
            text_tested: false,
            metrics: Metrics::default(),
            absent: Absent::Off,
            phase_samples: BTreeMap::new(),
            started: None,
            last_write: None,
            last_written: String::new(),
        }
    }
}

/// How often the report file may be rewritten. It changes on the scale of
/// phases, and a write from the frame path should be rare, not per tick.
const WRITE_GAP: Duration = Duration::from_secs(5);

impl Feature for DrawProbe {
    fn name(&self) -> &'static str {
        "draw_probe"
    }

    fn module(&self) -> &'static str {
        "diagnostics"
    }

    fn summary(&self) -> &'static str {
        "Collects various data - draw builtins, fonts, GUI metrics, and which objects' \
         GUI draw events are alive in each phase - into a report file."
    }

    fn config_template(&self) -> &'static str {
        "# How often to sample which draw-event objects are alive, in milliseconds.\n\
         interval_ms = 1000\n\
         # How many font ids to check for, listing those that exist. 0 skips.\n\
         fonts = 64\n\
         # Name of the output file, within the mod folder.\n\
         file = draw-probe.md\n\
         \n\
         # advanced: for working on the diagnostic itself. Set these here; the\n\
         # settings window does not show them.\n\
         # The deliberate experiment: ONE read of a variable proven absent on one\n\
         # instance, to learn whether the getter survives it. May end the session.\n\
         absent_read = false\n\
         # The instance and variable the experiment uses. The variable must exist in\n\
         # the game's code but not on this object.\n\
         absent_object = obj_main_menu\n\
         absent_var = main_product\n"
    }

    fn requires(&self) -> Requirements {
        Requirements {
            // Recorded, not required: the probe's whole job is to report what is
            // and is not there.
            objects: &["obj_init", "obj_splash_screen", "obj_main_menu", "obj_gameplay_controller"],
            ..Requirements::default()
        }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        let ms = section.u64("interval_ms", 1000)?;
        if !(100..=10_000).contains(&ms) {
            return Err(format!("interval_ms: {ms} is outside 100..10000"));
        }
        self.interval = Duration::from_millis(ms);

        let fonts = section.u64("fonts", 64)?;
        if fonts > 512 {
            return Err(format!("fonts: {fonts} is outside 0..512"));
        }
        self.fonts_max = fonts;

        if let Some(f) = section.get("file") {
            if f.is_empty() || f.contains(['/', '\\', ':']) {
                return Err(format!("file: {f:?} must be a bare file name"));
            }
            self.file = f.to_string();
        }

        let absent_read = section.bool("absent_read", false)?;
        self.absent = if absent_read { Absent::Pending("not attempted yet") } else { Absent::Off };
        if let Some(o) = section.get("absent_object") {
            self.absent_object = o.to_string();
        }
        if let Some(v) = section.get("absent_var") {
            self.absent_var = v.to_string();
        }

        for k in section.unknown(&[
            "enabled",
            "interval_ms",
            "fonts",
            "file",
            "absent_read",
            "absent_object",
            "absent_var",
        ]) {
            logln!("[draw_probe] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn activate(&mut self, _rt: &Runtime) -> Result<(), String> {
        self.started = Some(Instant::now());
        logln!(
            "[draw_probe] sampling every {}ms into {}; the absent-variable experiment is {}",
            self.interval.as_millis(),
            self.file,
            match self.absent {
                Absent::Off => "off (absent_read = false)",
                _ => "ON: one deliberate read of an absent variable will be made",
            }
        );
        Ok(())
    }

    fn cadence(&self) -> Cadence {
        Cadence::Interval(self.interval)
    }

    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        if self.census.is_none() {
            let census = build_census(rt);
            findln!(
                "draw_probe: {} tracked draw-event object(s), {} Draw-only object(s), \
                 {} scribble function(s)",
                census.tracked.len(),
                census.draw0_only,
                census.scribble_fns
            );
            self.census = Some(census);
        }

        // Which phase we are in, from the same markers the timeline uses.
        let mut phase_now = "boot";
        for (obj, label) in PHASES {
            if instance::find_singleton(rt.base, obj).is_some() {
                phase_now = label;
            }
        }
        *self.phase_samples.entry(phase_now).or_default() += 1;

        if let Some(census) = self.census.as_mut() {
            for (name, t) in census.tracked.iter_mut() {
                t.alive_now = instance::find_singleton(rt.base, name).is_some();
                if t.alive_now {
                    t.seen_in.insert(phase_now);
                }
            }
        }

        // Everything below calls into the runtime, so it waits for the GML
        // runtime to be usable at all.
        if rt.globals().is_some() {
            self.metrics = read_metrics(rt);
            if self.fonts.is_none() && self.fonts_max > 0 {
                let (fonts, diag) = scan_fonts(rt, self.fonts_max);
                findln!(
                    "draw_probe: {} font(s) among ids 0..{} ({diag})",
                    fonts.len(),
                    self.fonts_max
                );
                self.fonts = Some(fonts);
                self.fonts_diag = diag;
            }
            if matches!(self.absent, Absent::Pending(_)) {
                self.absent_experiment(rt);
            }
            // Verify GML-string construction: build a string and measure it with
            // the game's own string_width. A sensible size back means the runtime
            // accepted a string we built -- the one risky part of drawing text.
            if !self.text_tested {
                self.text_tested = true;
                let probe = "momomod text probe 123";
                match unsafe { tkiw_runtime::overlay::measure_test(rt.base, rt.text, probe) } {
                    Some((w, h)) if w > 0.0 && h > 0.0 => findln!(
                        "draw_probe: text self-test PASS: string_width({probe:?}) = {w} x {h} \
                         -- constructed GML strings work, so text drawing is safe"
                    ),
                    other => findln!(
                        "draw_probe: text self-test result {other:?} (0 or none may mean no font \
                         is set yet, not a failure)"
                    ),
                }
            }
        }

        self.write_report();
        Ok(())
    }

    fn deactivate(&mut self, _rt: &Runtime) {
        // Best effort: the report is the product, so leave the freshest one.
        self.last_write = None;
        self.write_report();
    }
}

/// One pass over the symbol table and `.pdata`. Done once; nothing here changes
/// while the process lives.
fn build_census(rt: &Runtime) -> Census {
    let mut tracked: BTreeMap<String, Tracked> = BTreeMap::new();
    let mut draw0: BTreeMap<&str, u32> = BTreeMap::new();
    let mut scribble_fns = 0usize;
    let mut dropped = 0usize;

    for (name, rva) in &rt.syms.functions {
        if name.contains("scribble") {
            scribble_fns += 1;
        }
        let Some((obj, subtype)) = parse_draw_event(name) else { continue };
        if !TRACKED_SUBTYPES.contains(&subtype) {
            draw0.insert(obj, *rva);
            continue;
        }
        if !tracked.contains_key(obj) && tracked.len() >= MAX_TRACKED {
            dropped += 1;
            continue;
        }
        tracked
            .entry(obj.to_string())
            .or_insert_with(|| Tracked {
                events: BTreeMap::new(),
                prologue: BTreeMap::new(),
                seen_in: BTreeSet::new(),
                alive_now: false,
            })
            .events
            .insert(subtype, *rva);
    }

    // A tracked object's plain Draw_0, if it has one, belongs in its row; and an
    // object only counts as "Draw-only" if nothing tracked it.
    let mut draw0_only = 0usize;
    for (obj, rva) in draw0 {
        match tracked.get_mut(obj) {
            Some(t) => {
                t.events.insert(0, rva);
            }
            None => draw0_only += 1,
        }
    }

    // Function sizes for the tracked events, from .pdata, one pass.
    let wanted: BTreeSet<u32> =
        tracked.values().flat_map(|t| t.events.values().copied()).collect();
    let mut sizes = HashMap::new();
    for (begin, end, _unwind) in rt.image.pdata_functions() {
        if wanted.contains(&begin) && end > begin {
            sizes.insert(begin, end - begin);
        }
    }

    // Prologue bytes, read from the live image. Recorded for the subtypes a
    // detour would hook; another feature's patch would show here, which is a
    // feature of the report rather than a bug.
    for t in tracked.values_mut() {
        for (subtype, rva) in t.events.clone() {
            if subtype == 0 {
                continue;
            }
            let at = rt.base + rva as usize;
            if tkiw_runtime::win::readable(at, 16) {
                let bytes: Vec<String> = (0..16)
                    .map(|i| format!("{:02x}", unsafe { core::ptr::read_volatile((at + i) as *const u8) }))
                    .collect();
                t.prologue.insert(subtype, bytes.join(" "));
            }
        }
    }

    Census { tracked, draw0_only, sizes, scribble_fns, dropped }
}

fn as_num(v: Option<Value>) -> Option<i64> {
    match v? {
        Value::Real(f) => Some(f as i64),
        Value::Int(i) => Some(i),
        Value::Bool(b) => Some(b as i64),
        _ => None,
    }
}

/// Booleans come back from builtins as whichever numeric kind the
/// implementation liked -- the first font scan matched only `Bool(true)` and
/// found zero fonts for it.
fn truthy(v: &Option<Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Real(x)) => *x != 0.0,
        Some(Value::Int(i)) => *i != 0,
        _ => false,
    }
}

/// The live numbers overlay layout would need. Read-only lookups, argc 0.
fn read_metrics(rt: &Runtime) -> Metrics {
    let call = |name: &str| as_num(unsafe { builtin::call_by_name(rt.base, rt.text, name, &[]) });
    let pair = |w: &str, h: &str| Some((call(w)?, call(h)?));
    Metrics {
        gui: pair("display_get_gui_width", "display_get_gui_height"),
        window: pair("window_get_width", "window_get_height"),
        display: pair("display_get_width", "display_get_height"),
        font: unsafe { builtin::call_by_name(rt.base, rt.text, "draw_get_font", &[]) }
            .map(|v| format!("{v:?}")),
    }
}

/// Font ids and names, gated the way the game itself would: `font_exists`
/// first, `font_get_name` only on ids that exist.
///
/// The second value is a diagnostic: the raw results for id 0, recorded so
/// that a scan finding nothing says what the builtins actually returned
/// instead of leaving the next session to guess.
fn scan_fonts(rt: &Runtime, max: u64) -> (Vec<(u32, String)>, String) {
    let mut out = Vec::new();
    let mut diag = String::new();
    for id in 0..max {
        let arg = [raw_of(&Value::Real(id as f64))];
        let exists = unsafe { builtin::call_by_name(rt.base, rt.text, "font_exists", &arg) };
        if id == 0 {
            diag = format!("font_exists(0) -> {exists:?}");
        }
        if !truthy(&exists) {
            continue;
        }
        // Only ever called on an id font_exists just vouched for -- the same
        // guard discipline as everywhere else.
        let name = unsafe { builtin::call_by_name(rt.base, rt.text, "font_get_name", &arg) };
        if id == 0 {
            diag = format!("font_exists(0) truthy; font_get_name(0) -> {name:?}");
        }
        if let Some(Value::Str(s)) = name {
            out.push((id as u32, s));
        }
    }
    (out, diag)
}

impl DrawProbe {
    /// The deliberate experiment, exactly as `gameplay-features.md` prescribes:
    /// one instance, one variable **proven absent by enumeration first**, and a
    /// loud log line before the call so a dead session is itself the answer.
    fn absent_experiment(&mut self, rt: &Runtime) {
        let Some(inst) = instance::find_singleton(rt.base, &self.absent_object) else {
            self.absent = Absent::Pending("waiting for a live instance of the object");
            return;
        };
        let Some(id) = rt.var_id(&self.absent_var) else {
            self.absent = Absent::Pending("waiting for the variable id to resolve");
            return;
        };
        // Enumeration last: it allocates a GML array the runtime never frees, so
        // it runs only once the attempt can actually complete.
        let names = unsafe {
            builtin::struct_member_names(rt.base, rt.text, &Value::Object(inst))
        };
        let Some(names) = names else {
            self.absent = Absent::Pending("could not enumerate the instance's members");
            return;
        };
        if names.iter().any(|n| n == &self.absent_var) {
            let msg = format!(
                "not run: {:?} IS present on {} ({} members) - pick a variable this \
                 object does not have (absent_var)",
                self.absent_var,
                self.absent_object,
                names.len()
            );
            findln!("draw_probe: absent-variable experiment {msg}");
            self.absent = Absent::Done(msg);
            return;
        }

        findln!(
            "draw_probe: about to read absent variable {:?} (id {id}) off {} at {inst:#x}. \
             If the session ends here, the getter is fatal on absent variables - that is \
             the finding.",
            self.absent_var,
            self.absent_object
        );
        phase::note("draw_probe absent-variable read");
        let got = unsafe { rt.globals().map(|g| g.get_on(inst, id)) }.flatten();
        let msg = format!(
            "SURVIVED on {} ({} members): getter returned {:?} for absent {:?}. A wrong \
             guess is distinguishable, and enumeration-gated sweeps can proceed.",
            self.absent_object,
            names.len(),
            got,
            self.absent_var
        );
        findln!("draw_probe: absent-variable experiment {msg}");
        self.absent = Absent::Done(msg);
    }

    fn write_report(&mut self) {
        let text = self.render();
        if text == self.last_written {
            return;
        }
        let now = Instant::now();
        if self.last_write.is_some_and(|t| now.duration_since(t) < WRITE_GAP) {
            return;
        }
        let Some(path) = tkiw_runtime::home::file(&self.file) else { return };
        match std::fs::write(&path, &text) {
            Ok(()) => {
                self.last_write = Some(now);
                self.last_written = text;
            }
            Err(e) => logln!("[draw_probe] could not write {}: {e}", self.file),
        }
    }

    fn render(&self) -> String {
        let mut s = String::with_capacity(16 * 1024);
        s.push_str(
            "# Draw probe\n\n\
             Written by the kit's `draw_probe` diagnostic; regenerated every few seconds \
             while it runs. Facts a drawing detour needs, per `analysis/gameplay-features.md`. \
             RVAs are for this game build; prologue bytes are read from the **live** image, \
             so another feature's patch shows here.\n",
        );

        s.push_str("\n## Sampling\n\n");
        for (phase, n) in &self.phase_samples {
            s.push_str(&format!("- {phase}: {n} sample(s)\n"));
        }

        s.push_str("\n## Drawing builtins\n\n| builtin | rva |\n|---|---|\n");
        for name in NEEDED_BUILTINS {
            match builtin::by_name(name) {
                Some(rva) => s.push_str(&format!("| {name} | {rva:#x} |\n")),
                None => s.push_str(&format!("| {name} | **missing from the table** |\n")),
            }
        }

        s.push_str("\n## Live metrics\n\n");
        let dim = |v: Option<(i64, i64)>| match v {
            Some((w, h)) => format!("{w} x {h}"),
            None => "unread".into(),
        };
        s.push_str(&format!(
            "- gui: {}\n- window: {}\n- display: {}\n- font current at pump time: {}\n",
            dim(self.metrics.gui),
            dim(self.metrics.window),
            dim(self.metrics.display),
            self.metrics.font.as_deref().unwrap_or("unread"),
        ));

        s.push_str("\n## Fonts\n\n");
        match &self.fonts {
            None => s.push_str("not scanned yet (the GML runtime was not up, or fonts = 0)\n"),
            Some(fonts) => {
                s.push_str("| id | name |\n|---|---|\n");
                for (id, name) in fonts {
                    s.push_str(&format!("| {id} | {name} |\n"));
                }
                if !self.fonts_diag.is_empty() {
                    s.push_str(&format!("\ndiagnostic: {}\n", self.fonts_diag));
                }
                if fonts.len() as u64 == self.fonts_max {
                    s.push_str(&format!(
                        "\n**every id up to the cap ({}) exists** - the table goes on; \
                         raise `fonts` to see the rest.\n",
                        self.fonts_max
                    ));
                }
            }
        }

        s.push_str("\n## Detour hosts\n\n");
        match &self.census {
            None => s.push_str("census not built yet\n"),
            Some(c) => {
                s.push_str(
                    "Objects with GUI-layer or begin/end draw events. A good host is alive \
                     in every phase you want to draw in.\n\n\
                     | object | event | rva | size | alive in |\n|---|---|---|---|---|\n",
                );
                for (obj, t) in &c.tracked {
                    for (subtype, rva) in &t.events {
                        if *subtype == 0 {
                            continue;
                        }
                        let seen = if t.seen_in.is_empty() {
                            "never seen alive".to_string()
                        } else {
                            t.seen_in.iter().copied().collect::<Vec<_>>().join(", ")
                        };
                        s.push_str(&format!(
                            "| {obj} | {} ({subtype}) | {rva:#x} | {} | {seen} |\n",
                            event_label(*subtype),
                            c.sizes.get(rva).map_or("?".into(), |n| n.to_string()),
                        ));
                    }
                }

                s.push_str("\n### Prologues (first 16 bytes)\n\n");
                let mut any = false;
                for (obj, t) in &c.tracked {
                    if t.seen_in.is_empty() {
                        continue;
                    }
                    for (subtype, hex) in &t.prologue {
                        s.push_str(&format!(
                            "- `{obj}` {} ({subtype}): `{hex}`\n",
                            event_label(*subtype)
                        ));
                        any = true;
                    }
                }
                if !any {
                    s.push_str("none yet: no tracked object has been seen alive\n");
                }

                s.push_str(&format!(
                    "\nAlso in the symbol table: {} object(s) with only a plain Draw event \
                     (world-space, per-instance - not detour hosts), and {} scribble \
                     function(s) (the game's own text stack; calling it means the \
                     compiled-GML convention).\n",
                    c.draw0_only, c.scribble_fns
                ));
                if c.dropped > 0 {
                    s.push_str(&format!(
                        "\n**{} object(s) beyond the {MAX_TRACKED}-object tracking cap were \
                         dropped** - raise the cap if this build really has that many.\n",
                        c.dropped
                    ));
                }
            }
        }

        s.push_str("\n## Absent-variable experiment\n\n");
        match &self.absent {
            Absent::Off => s.push_str(
                "off (`absent_read = false`). Switching it on makes ONE deliberate read of \
                 a variable proven absent, to establish whether the getter survives it.\n",
            ),
            Absent::Pending(why) => s.push_str(&format!("pending: {why}\n")),
            Absent::Done(msg) => s.push_str(&format!("{msg}\n")),
        }

        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_event_names_parse_and_nested_functions_do_not() {
        assert_eq!(
            parse_draw_event("gml_Object_obj_main_menu_Draw_64"),
            Some(("obj_main_menu", 64))
        );
        assert_eq!(parse_draw_event("gml_Object_obj_x_Draw_0"), Some(("obj_x", 0)));
        // an object whose own name contains _Draw_ still parses via the last match
        assert_eq!(parse_draw_event("gml_Object_obj_Draw_thing_Draw_75"), Some(("obj_Draw_thing", 75)));
        assert_eq!(parse_draw_event("gml_Script_scribble"), None);
        assert_eq!(parse_draw_event("gml_Script_anon@3062@gml_Object_obj_x_Draw_0"), None);
        assert_eq!(parse_draw_event("gml_Object_obj_x_Step_0"), None);
        assert_eq!(parse_draw_event("gml_Object_obj_x_Draw_"), None);
        assert_eq!(parse_draw_event("gml_Object_obj_x_Draw_64b"), None);
    }

    /// Every builtin the detour plans on must be in the generated table --
    /// otherwise the plan is wrong *now*, not at some future probe run.
    #[test]
    fn the_needed_builtins_are_all_in_the_table() {
        for name in NEEDED_BUILTINS {
            assert!(
                builtin::by_name(name).is_some(),
                "{name} is not in the builtins table"
            );
        }
    }

    #[test]
    fn the_tracked_subtypes_are_the_frame_bracket_events() {
        for sub in TRACKED_SUBTYPES {
            assert_ne!(event_label(*sub), "Draw_?", "subtype {sub} has no label");
            assert_ne!(*sub, 0, "plain Draw is censused, not tracked");
        }
    }

    #[test]
    fn the_report_renders_before_anything_is_known() {
        let probe = DrawProbe::default();
        let text = probe.render();
        assert!(text.contains("census not built yet"));
        assert!(text.contains("absent_read = false"));
    }

    #[test]
    fn config_rejects_paths_and_wild_intervals() {
        let mut probe = DrawProbe::default();
        let bad = momomod_kit::config::Config::parse("[feature.draw_probe]\nfile = ..\\evil.md\n");
        assert!(probe.configure(&bad.section("draw_probe")).is_err());
        let bad = momomod_kit::config::Config::parse("[feature.draw_probe]\ninterval_ms = 5\n");
        assert!(probe.configure(&bad.section("draw_probe")).is_err());
        let good = momomod_kit::config::Config::parse(
            "[feature.draw_probe]\ninterval_ms = 500\nfonts = 32\nabsent_read = true\n",
        );
        assert!(probe.configure(&good.section("draw_probe")).is_ok());
        assert!(matches!(probe.absent, Absent::Pending(_)));
    }
}
