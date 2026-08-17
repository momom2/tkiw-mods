//! Dump the game's own libraries to JSON, once, from a live process.
//!
//! The game keeps its content in global `ds_map`s -- `UNITS`, `IMPROVEMENTS`,
//! `UPGRADES`, `ARTIFACTS`, `SPELLS` -- keyed by system name, with a struct per entry.
//! Those structs hold the numbers everything else is derived from: a unit's base HP and
//! damage, an improvement's production, an upgrade's magnitude.
//!
//! Nothing else in this repository has them. An earlier dump recovered only the *keys*,
//! which answers "what exists" and not "what is it". This answers the second.
//!
//! ## Why a live dump rather than static analysis
//!
//! The values are assigned by `unit_library` and its siblings -- tens of thousands of
//! compiled instructions setting struct fields from immediates. Recovering them by
//! disassembly is possible and miserable. The running game has already done it, and the
//! result is one `ds_map` walk away.
//!
//! **The libraries exist at the main menu.** No run is needed, so this can be collected
//! unattended with `knowledge-base/tools/playtest.py`.
//!
//! ## What it costs, and why it is a one-shot
//!
//! Enumerating a struct's members calls `variable_struct_get_names`, which allocates a
//! GML array the runtime never frees. That is fine once and unacceptable per frame, so
//! the feature writes its file and disables itself. It is a data-collection tool, not
//! something to leave switched on.
//!
//! ## Reading fields safely
//!
//! Members are read **by enumerated name**, never by guessing a variable id. Asking a
//! struct for a field it does not have is the unresolved hazard recorded in
//! `analysis/gameplay-features.md` -- it may raise a fatal dialog rather than returning
//! undefined. Enumerating first removes the question entirely: every name asked for is
//! one the struct just said it has.

use std::collections::BTreeMap;

use tkiw_runtime::{builtin, dslist, findln, logln, rvalue, rvalue::Value, Runtime};

use momomod_kit::config::Section;
use momomod_kit::feature::{Cadence, Feature, Requirements};

/// The globals to walk. Each is a `ds_map` keyed by system name.
const LIBRARIES: &[&str] = &[
    "UNITS",
    "UNIT_CLASSES",
    "IMPROVEMENTS",
    "UPGRADES",
    "ARTIFACTS",
    "SPELLS",
    "RESOURCES",
    "ADVISORS",
    "KINGS",
    "ASCENSIONS",
    "CHALLENGES",
    "ENCOUNTERS",
    "LEVELS",
];

/// Sanity bound on a library. The largest real one is UPGRADES at 269.
const MAX_ENTRIES: usize = 2000;

pub struct DumpLibraries {
    done: bool,
    file: String,
    attempts: u32,
}

impl Default for DumpLibraries {
    fn default() -> DumpLibraries {
        DumpLibraries { done: false, file: "libraries.json".into(), attempts: 0 }
    }
}

/// A field value, flattened to something JSON can hold.
enum Flat {
    Num(f64),
    Str(String),
    Bool(bool),
    Other(&'static str),
}

impl Flat {
    fn json(&self) -> String {
        match self {
            Flat::Num(v) if v.is_finite() => {
                if (v.fract()).abs() < 1e-9 && v.abs() < 1e15 {
                    format!("{}", *v as i64)
                } else {
                    format!("{v}")
                }
            }
            Flat::Num(_) => "null".into(),
            Flat::Bool(b) => format!("{b}"),
            Flat::Str(s) => escape(s),
            Flat::Other(k) => escape(&format!("<{k}>")),
        }
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Expand an array of scalars. Arrays are where several of the most useful fields
/// live -- `classes` (which class bonuses a unit receives) and `attack_action_frame`
/// (the animation delay before a hit lands) are both arrays, and a dump that renders
/// them as `<array>` is missing the point.
///
/// Nested arrays and structs are not followed: one level is what the data needs, and
/// unbounded recursion through live game structures is how a diagnostic becomes a hang.
unsafe fn flatten_array(rt: &Runtime, v: &Value) -> Option<String> {
    let Value::Array(ptr) = v else { return None };
    let ptr = *ptr;
    if ptr == 0 || !tkiw_runtime::win::readable(ptr + rvalue::ARRAY_LEN + 4, 4) {
        return None;
    }
    let len = rvalue::read_i32(ptr + rvalue::ARRAY_LEN)?;
    if !(0..256).contains(&len) {
        return None;
    }
    let data = rvalue::read_usize(ptr + rvalue::ARRAY_DATA)?;
    let mut out = String::from("[");
    for i in 0..len as usize {
        let at = data.checked_add(i * 16)?;
        if !tkiw_runtime::win::readable(at, 16) {
            return None;
        }
        let Some(el) = rvalue::decode(at) else { continue };
        if i > 0 {
            out.push(',');
        }
        out.push(' ');
        // An element that is itself a struct gets one level of expansion. This is
        // where the answer usually is: an encounter's `options` array holds the structs
        // carrying `castle_max_hp_given`, and a dump that stops at `<nested>` reports,
        // wrongly, that no encounter grants castle HP at all.
        out.push_str(&match &el {
            Value::Object(_) => flatten_struct(rt, &el).unwrap_or_else(|| escape("<struct>")),
            Value::Array(_) => escape("<nested>"),
            other => flatten(other).json(),
        });
    }
    out.push_str(" ]");
    Some(out)
}

/// Expand a struct one level down.
///
/// The first version of this dump rendered every nested struct as `<struct>`, which was
/// enough until a value that mattered turned out to live one level in: two encounters
/// carry `castle_max_hp_given`, and neither appears anywhere in an 84-entry dump that
/// stops at the top. One level deeper is the difference between "no encounter grants
/// castle HP" and knowing exactly which two do.
///
/// Bounded at one level on purpose. Unbounded recursion through live game structures --
/// which contain methods, references back to their parents, and the whole object graph
/// -- is how a diagnostic becomes a hang.
///
/// # Safety
/// Game thread.
unsafe fn flatten_struct(rt: &Runtime, v: &Value) -> Option<String> {
    let Value::Object(ptr) = v else { return None };
    if *ptr == 0 {
        return None;
    }
    let fields = builtin::struct_member_names(rt.base, rt.text, v)?;
    if fields.is_empty() || fields.len() > 64 {
        return None;
    }
    let mut row: BTreeMap<String, String> = BTreeMap::new();
    for f in fields {
        let Some(inner) = builtin::struct_get_by_name(rt.base, rt.text, v, &f) else { continue };
        // No third level: at this depth a struct is rendered as a tag, as before.
        let rendered = flatten_array(rt, &inner).unwrap_or_else(|| flatten(&inner).json());
        row.insert(f, rendered);
    }
    let mut out = String::from("{");
    let mut first = true;
    for (k, val) in row {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(" {}: {val}", escape(&k)));
    }
    out.push_str(" }");
    Some(out)
}

fn flatten(v: &Value) -> Flat {
    match v {
        Value::Real(x) => Flat::Num(*x),
        Value::Int(x) => Flat::Num(*x as f64),
        Value::Bool(b) => Flat::Bool(*b),
        Value::Str(s) => Flat::Str(s.clone()),
        Value::Array(_) => Flat::Other("array"),
        Value::Object(_) => Flat::Other("struct"),
        Value::Undefined => Flat::Other("undefined"),
        other => Flat::Other(match other {
            Value::Ref { .. } => "ref",
            _ => "unknown",
        }),
    }
}

impl Feature for DumpLibraries {
    fn name(&self) -> &'static str {
        "dump_libraries"
    }

    fn module(&self) -> &'static str {
        "diagnostics"
    }

    fn summary(&self) -> &'static str {
        "Write the game's own content libraries (artifacts, buildings, spells, units, \
         upgrades) to a JSON file, once, then switches itself off."
    }

    fn config_template(&self) -> &'static str {
        "# Name of the output file, within the mod folder.\n\
         file = libraries.json\n"
    }

    fn requires(&self) -> Requirements {
        // Only by-name variables: the library globals themselves. Everything else this
        // feature uses is already covered by the shared runtime's core guard.
        Requirements { variables: LIBRARIES, ..Requirements::default() }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        if let Some(f) = section.get("file") {
            if f.contains(['/', '\\', ':']) {
                return Err(format!("file: {f:?} must be a bare filename"));
            }
            self.file = f.to_string();
        }
        for k in section.unknown(&["enabled", "file"]) {
            logln!("[dump_libraries] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    /// Every couple of seconds until the runtime is up and the walk succeeds.
    fn cadence(&self) -> Cadence {
        Cadence::Interval(std::time::Duration::from_secs(2))
    }

    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        if self.done {
            return Ok(());
        }
        // The libraries do not exist until the game has built them. `None` here is
        // "not yet", not a failure -- but do not retry forever either.
        let Some(globals) = rt.globals() else { return Ok(()) };
        self.attempts += 1;
        if self.attempts > 60 {
            self.done = true;
            return Err("the libraries never became readable (2 minutes)".into());
        }

        let mut out = String::from("{\n");
        let mut wrote_any = false;

        for (i, lib) in LIBRARIES.iter().enumerate() {
            let Some(id) = rt.var_id(lib) else { continue };
            // SAFETY: on the game's thread, via the frame hook.
            let Some(value) = (unsafe { globals.get(id) }) else { continue };
            let Some(entries) = dslist::ds_map_entries(rt.base, &value, MAX_ENTRIES) else {
                continue;
            };
            if entries.is_empty() {
                continue;
            }
            wrote_any = true;

            if i > 0 && out.len() > 2 {
                out.push_str(",\n");
            }
            out.push_str(&format!("  {}: {{\n", escape(lib)));

            let mut first = true;
            for (key, entry) in &entries {
                // Most libraries are keyed by system name, but UNIT_CLASSES is keyed by
                // class *index* -- a string-only walk reports it as empty, which is how
                // the first dump lost it entirely.
                let owned;
                let name = match key.as_str() {
                    Some(s) => s,
                    None => match key.as_f64() {
                        Some(n) => {
                            owned = if n.fract() == 0.0 {
                                format!("{}", n as i64)
                            } else {
                                format!("{n}")
                            };
                            &owned
                        }
                        None => continue,
                    },
                };
                // SAFETY: game thread. Names are enumerated before anything is read, so
                // no field is ever asked for that the struct did not just report.
                let fields = unsafe { builtin::struct_member_names(rt.base, rt.text, entry) };
                let Some(fields) = fields else { continue };

                let mut row: BTreeMap<String, String> = BTreeMap::new();
                for f in fields {
                    let got = unsafe { builtin::struct_get_by_name(rt.base, rt.text, entry, &f) };
                    if let Some(v) = got {
                        let rendered = unsafe { flatten_array(rt, &v) }
                            .or_else(|| unsafe { flatten_struct(rt, &v) })
                            .unwrap_or_else(|| flatten(&v).json());
                        row.insert(f, rendered);
                    }
                }
                if !first {
                    out.push_str(",\n");
                }
                first = false;
                out.push_str(&format!("    {}: {{", escape(name)));
                let mut inner = true;
                for (k, v) in row {
                    if !inner {
                        out.push(',');
                    }
                    inner = false;
                    out.push_str(&format!(" {}: {v}", escape(&k)));
                }
                out.push_str(" }");
            }
            out.push_str("\n  }");
            logln!("[dump_libraries] {lib}: {} entries", entries.len());
        }
        out.push_str("\n}\n");

        if !wrote_any {
            return Ok(()); // libraries not built yet; try again next tick
        }

        self.done = true;
        match tkiw_runtime::home::file(&self.file) {
            Some(path) => match std::fs::write(&path, out) {
                Ok(()) => findln!(
                    "[dump_libraries] wrote {} - switching off; this is a one-shot",
                    path.display()
                ),
                Err(e) => return Err(format!("could not write {}: {e}", path.display())),
            },
            None => return Err("no mod folder to write into".into()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escaping_survives_awkward_names() {
        assert_eq!(escape(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(escape("tab\there"), r#""tab\there""#);
    }

    #[test]
    fn whole_numbers_render_without_a_decimal_point() {
        assert_eq!(Flat::Num(100.0).json(), "100");
        assert_eq!(Flat::Num(0.5).json(), "0.5");
        assert_eq!(Flat::Num(f64::NAN).json(), "null");
    }

    /// A path in the filename would let a config write outside the mod folder.
    #[test]
    fn the_output_filename_must_be_bare() {
        let mut f = DumpLibraries::default();
        let cfg = momomod_kit::config::Config::parse("[feature.dump_libraries]\nfile = ../evil.json\n");
        assert!(f.configure(&cfg.section("dump_libraries")).is_err());
    }
}
