//! Dump the full live state of whatever the player hovers.
//!
//! The comparison data the units database cannot provide: what the game
//! actually *displays* comes from live instances with every modifier applied,
//! and the open questions -- does the in-game dps multiply multi-hit attacks,
//! what do the `{&N}` description parameters resolve to -- are all answered by
//! reading a hovered thing at the moment a person is looking at its panel.
//!
//! ## How the hovered thing is found
//!
//! Not by sweeping. The game's own hover detection lives on **`obj_cursor`**:
//! its Step writes `hovered_unit_instance`, computes `hovered_unit_hp` and
//! `hovered_unit_damage`, and its Draw draws the panel from them (established
//! by byte-scan: those variables are referenced *only* from `obj_cursor`
//! events). So the probe reads the cursor's pointer once a tick and follows
//! it -- one singleton lookup and a few member reads, O(1) whatever the
//! battlefield holds. The cursor's own display values ride along in each dump,
//! which is exactly the internal-vs-displayed comparison wanted.
//!
//! The first design swept `obj_unit_*` instances for an `is_hovered` flag.
//! It found nothing in a real session, because no unit function references
//! `is_hovered` -- that flag belongs to cards. The sweep is kept out of the
//! history books as a reminder that the game's own reader is always the
//! better source than a guessed flag.
//!
//! ## Nothing happens outside a run
//!
//! Until `obj_gameplay_controller` exists this feature is one singleton lookup
//! per tick. An earlier version acted during boot and enumerated members on
//! instances the game was still constructing -- the recorded unsafe case --
//! and that boot never reached the menu. The gate is structural, not tuning.
//!
//! ## Safety against the absent-variable hazard
//!
//! * the cursor's members are enumerated once (it is a singleton built at
//!   startup, long-lived by the time a run exists), and the pointer member is
//!   read only if that enumeration lists it;
//! * a dump reads members **by enumerated name**, the same discipline as
//!   `dump_libraries`. Enumeration allocates a GML array the runtime never
//!   frees -- once per cursor, once per dump; a measurement-session budget.

use std::time::{Duration, Instant};

use tkiw_runtime::{
    builtin, findln, home, instance, logln,
    rvalue::Value,
    Runtime,
};

use crate::config::Section;
use crate::feature::{Cadence, Feature, Requirements};

/// Members listed per dump. Units carry 625 (measured); the bound exists so a
/// runaway enumeration truncates loudly rather than writing a megabyte. The
/// first cap chosen (256) silently cost a session two-thirds of every dump --
/// including all the combat stats, which sort late in the alphabet.
const MAX_MEMBERS: usize = 1024;

/// A new dump is written only when the hovered target changes, and no more
/// often than this.
const DUMP_GAP: Duration = Duration::from_millis(500);

pub struct HoverProbe {
    interval: Duration,
    holder: String,
    pointer: String,
    extras: Vec<String>,
    file: String,

    /// The cursor's enumerated members, established once per session:
    /// `Some(true)` = pointer present, `Some(false)` = absent (probe idles).
    holder_ok: Option<bool>,
    /// Whether the dump pipeline has been proven this session -- see
    /// [`HoverProbe::self_test`].
    self_tested: bool,
    last_dump: Option<i64>,
    last_dump_at: Option<Instant>,
    started: Option<Instant>,
}

impl Default for HoverProbe {
    fn default() -> HoverProbe {
        HoverProbe {
            interval: Duration::from_millis(250),
            holder: "obj_cursor".into(),
            pointer: "hovered_unit_instance".into(),
            extras: vec!["hovered_unit_hp".into(), "hovered_unit_damage".into()],
            file: "hover-probe.md".into(),
            holder_ok: None,
            self_tested: false,
            last_dump: None,
            last_dump_at: None,
            started: None,
        }
    }
}

impl Feature for HoverProbe {
    fn name(&self) -> &'static str {
        "hover_probe"
    }

    fn module(&self) -> &'static str {
        "diagnostics"
    }

    fn summary(&self) -> &'static str {
        "Records what the cursor hovers over in-game."
    }

    fn requires(&self) -> Requirements {
        Requirements {
            variables: &["hovered_unit_instance", "hovered_unit_hp", "hovered_unit_damage"],
            objects: &["obj_cursor", "obj_gameplay_controller"],
            ..Requirements::default()
        }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        let ms = section.u64("interval_ms", 250)?;
        if !(50..=5_000).contains(&ms) {
            return Err(format!("interval_ms: {ms} is outside 50..5000"));
        }
        self.interval = Duration::from_millis(ms);

        let varname = |what: &str, v: &str| -> Result<String, String> {
            if v.is_empty() || !v.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return Err(format!("{what}: {v:?} is not a variable name"));
            }
            Ok(v.to_string())
        };
        if let Some(h) = section.get("holder") {
            if !h.starts_with("obj_") {
                return Err(format!("holder: {h:?} does not look like an object name"));
            }
            self.holder = h.to_string();
        }
        if let Some(p) = section.get("pointer") {
            self.pointer = varname("pointer", p)?;
        }
        if let Some(e) = section.get("extras") {
            self.extras = e
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| varname("extras", s))
                .collect::<Result<_, _>>()?;
        }
        if let Some(f) = section.get("file") {
            if f.is_empty() || f.contains(['/', '\\', ':']) {
                return Err(format!("file: {f:?} must be a bare file name"));
            }
            self.file = f.to_string();
        }
        for k in section.unknown(&["enabled", "interval_ms", "holder", "pointer", "extras", "file"]) {
            logln!("[hover_probe] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn activate(&mut self, _rt: &Runtime) -> Result<(), String> {
        self.started = Some(Instant::now());
        self.holder_ok = None;
        logln!(
            "[hover_probe] following {}.{}; dumps go to {}",
            self.holder,
            self.pointer,
            self.file
        );
        Ok(())
    }

    fn cadence(&self) -> Cadence {
        Cadence::Interval(self.interval)
    }

    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        let Some(globals) = rt.globals() else { return Ok(()) };

        // From the menu on, the whole dump pipeline is provable on the cursor
        // itself -- no run, no mouse, no player. So prove it, once, where an
        // unattended launch can see the result: a layer that would otherwise
        // fail on the first hover of a real session fails here instead.
        if !self.self_tested
            && instance::find_singleton(rt.base, "obj_main_menu").is_some()
        {
            self.self_tested = true;
            self.self_test(rt);
        }

        // Nothing else before a run exists; see the module docs for the scar.
        if instance::find_singleton(rt.base, "obj_gameplay_controller").is_none() {
            return Ok(());
        }
        let Some(cursor) = instance::find_singleton(rt.base, &self.holder) else {
            return Ok(());
        };

        // Once per session: the cursor's own member list decides whether the
        // pointer may be read, and doubles as discovery output.
        if self.holder_ok.is_none() {
            let names = unsafe {
                builtin::struct_member_names(rt.base, rt.text, &Value::Object(cursor))
            };
            let Some(names) = names else { return Ok(()) };
            let hoverish: Vec<&str> = names
                .iter()
                .map(String::as_str)
                .filter(|n| n.contains("hover"))
                .collect();
            findln!(
                "hover_probe: {} has {} members; hover-related: {}",
                self.holder,
                names.len(),
                hoverish.join(", ")
            );
            self.holder_ok = Some(names.iter().any(|n| n == &self.pointer));
            if self.holder_ok == Some(false) {
                logln!(
                    "[hover_probe] {} has no {:?} - idle. The hover-related members \
                     above are the candidates; set `pointer` to one of them.",
                    self.holder,
                    self.pointer
                );
            }
        }
        if self.holder_ok != Some(true) {
            return Ok(());
        }

        let Some(id) = rt.var_id(&self.pointer) else { return Ok(()) };
        // The pointer holds an instance reference: an id in this build's
        // observed encodings, or nothing (`noone` is -4) when unhovered.
        let target_id = match unsafe { globals.get_on(cursor, id) } {
            Some(Value::Real(v)) if v >= 100_000.0 => v as i64,
            Some(Value::Int(i)) if i >= 100_000 => i,
            Some(Value::Ref { id, .. }) if id >= 100_000 => id as i64,
            _ => return Ok(()),
        };

        let now = Instant::now();
        if self.last_dump == Some(target_id) {
            return Ok(());
        }
        if self.last_dump_at.is_some_and(|t| now.duration_since(t) < DUMP_GAP) {
            return Ok(());
        }
        let Some(inst) = instance::by_id(rt.base, target_id as i32) else {
            return Ok(());
        };
        self.dump(rt, cursor, target_id, inst, now);
        self.last_dump = Some(target_id);
        self.last_dump_at = Some(now);
        Ok(())
    }
}

impl HoverProbe {
    /// Prove the dump pipeline on the cursor itself, at the menu.
    ///
    /// Exercises every layer a real hover dump uses -- singleton lookup, id
    /// extraction, **id-form enumeration**, member read by name, file append
    /// -- on an instance that always exists. One loud PASS/FAIL line either
    /// way, so an unattended launch settles what previously took a play
    /// session per layer.
    fn self_test(&mut self, rt: &Runtime) {
        let step = (|| -> Result<String, &'static str> {
            let cursor = instance::find_singleton(rt.base, &self.holder)
                .ok_or("no live holder instance")?;
            let id = instance::id_of(cursor).ok_or("could not read the holder's id")?;
            let owner = Value::Int(id as i64);
            let names = unsafe { builtin::struct_member_names(rt.base, rt.text, &owner) }
                .ok_or("id-form enumeration returned nothing")?;
            if names.is_empty() {
                return Err("id-form enumeration returned an empty list");
            }
            let probe_member = names[0].clone();
            unsafe { builtin::struct_get_by_name(rt.base, rt.text, &owner, &probe_member) }
                .ok_or("member read by name returned nothing")?;
            Ok(format!(
                "{} members on {} (instance {id}), first ({probe_member}) readable",
                names.len(),
                self.holder
            ))
        })();

        let line = match &step {
            Ok(what) => format!("self-test PASS: {what}"),
            Err(why) => format!("self-test FAIL: {why} - hover dumps would fail the same way"),
        };
        findln!("hover_probe: {line}");
        if let Some(path) = home::file(&self.file) {
            use std::io::Write;
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| writeln!(f, "\n# hover_probe {line}"));
        }
    }

    /// Append one hovered instance's members to the report, with the cursor's
    /// own display values as the header.
    fn dump(&self, rt: &Runtime, cursor: usize, target_id: i64, inst: usize, now: Instant) {
        let Some(globals) = rt.globals() else { return };
        // Enumerated by id: instances handed to the getters as raw pointers
        // come back undefined; only the id form works (a session's lesson).
        // Batched: name-by-name reads re-enumerate per member, and a
        // 625-member unit held the game's thread for 373ms that way.
        let owner = Value::Int(target_id);
        let members = unsafe { builtin::struct_members(rt.base, rt.text, &owner, MAX_MEMBERS) };
        let object = instance::object_name_of(inst).unwrap_or_else(|| "?".into());
        let elapsed = self.started.map(|s| now.duration_since(s).as_secs_f64()).unwrap_or(0.0);

        let mut s = format!(
            "\n## {elapsed:.1}s  {object}  (instance {target_id}, {})\n\n",
            members
                .as_ref()
                .map_or("members unenumerable".to_string(), |(_, total)| format!("{total} members"))
        );
        // The cursor's own display values first: even if the member walk
        // fails, the record still carries the internal-vs-displayed pair.
        for extra in &self.extras {
            let shown = rt
                .var_id(extra)
                .and_then(|id| unsafe { globals.get_on(cursor, id) })
                .map_or("<unread>".into(), |v| render(&v));
            s.push_str(&format!("- cursor.{extra} = {shown}\n"));
        }
        s.push('\n');
        let (members, total) = members.unwrap_or_default();
        for (name, v) in &members {
            s.push_str(&format!(
                "- {name} = {}\n",
                v.as_ref().map_or("<unreadable>".into(), render)
            ));
        }
        if total > members.len() {
            s.push_str(&format!("- ... and {} more (capped)\n", total - members.len()));
        }
        let Some(path) = home::file(&self.file) else { return };
        use std::io::Write;
        let r = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(s.as_bytes()));
        match r {
            Ok(()) => findln!(
                "hover_probe: dumped {object} ({total} members, {} written) at {elapsed:.1}s",
                members.len()
            ),
            Err(e) => logln!("[hover_probe] could not write {}: {e}", self.file),
        }
    }
}

fn render(v: &Value) -> String {
    match v {
        Value::Real(x) => format!("{x}"),
        Value::Int(i) => format!("{i}"),
        Value::Bool(b) => format!("{b}"),
        Value::Str(t) => {
            let mut t = t.replace('\n', "\\n");
            t.truncate(200);
            format!("{t:?}")
        }
        Value::Undefined => "undefined".into(),
        Value::Array(_) => "<array>".into(),
        Value::Object(_) => "<struct/method>".into(),
        Value::Ref { ref_type, id } => format!("<ref {ref_type}:{id}>"),
        Value::Other { kind, .. } => format!("<kind {kind}>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_and_rejects_junk() {
        let mut p = HoverProbe::default();
        let good = crate::config::Config::parse(
            "[feature.hover_probe]\nholder = obj_cursor\npointer = hovered_unit_instance\n\
             extras = hovered_unit_hp, hovered_unit_damage\n",
        );
        assert!(p.configure(&good.section("hover_probe")).is_ok());
        assert_eq!(p.extras.len(), 2);

        let bad = crate::config::Config::parse("[feature.hover_probe]\nholder = cursor\n");
        assert!(p.configure(&bad.section("hover_probe")).is_err());
        let bad = crate::config::Config::parse("[feature.hover_probe]\npointer = Not A Var\n");
        assert!(p.configure(&bad.section("hover_probe")).is_err());
        let bad = crate::config::Config::parse("[feature.hover_probe]\nfile = a\\b.md\n");
        assert!(p.configure(&bad.section("hover_probe")).is_err());
    }
}
