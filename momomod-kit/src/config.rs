//! The config: which mods are loaded, which of their features are on, and what
//! each of those thinks.
//!
//! Line-oriented INI, parsed by hand -- it is a hundred lines of code and it keeps
//! the build dependency-free.
//!
//! ## One file per mod
//!
//! ```text
//! config/momomod.ini        the kit: tracing, and which mods to load
//! config/optimization.ini   the optimization mod's features
//! config/bugfixes.ini       the bugfix mod's features
//! config/diagnostics.ini    the measurement tools
//! ```
//!
//! The split is not tidiness. **Every mod must be shippable on its own**, and a
//! mod that ships alone reads exactly the file it reads here -- same name, same
//! sections, same keys. So `config/optimization.ini` under the kit and
//! `optimization.ini` beside a standalone optimization mod are the same document,
//! and a player moving between the two keeps their settings. Nothing kit-specific
//! may leak into a mod's file; that is what `config/momomod.ini` is for.
//!
//! Three rules, each of which is a lesson from the auto-picker rather than a
//! preference:
//!
//! * **An unknown section or key is logged and ignored, never fatal.** A config
//!   written for a newer build of the kit must not brick an older one, or the
//!   reverse. Being strict here means every feature added is a config that
//!   suddenly refuses to load.
//! * **A failed reload keeps the last known-good config.** A bad edit mid-run must
//!   never silently change behaviour.
//! * **An existing config is never overwritten.** Regenerating one over a
//!   carefully-tuned file left it inert and the mod looking broken, once. When the
//!   kit has features the file does not mention it writes
//!   `momomod.reference.ini` beside it instead, and says so.

use std::collections::BTreeMap;
use std::path::Path;

use tkiw_runtime::{home, logln};

/// The folder holding every config file, inside the mod folder.
pub const DIR: &str = "config";
/// The kit's own file, inside [`DIR`].
pub const KIT_FILE: &str = "momomod.ini";
/// Where the pre-`config/` kit kept everything. Read once, to migrate.
pub const LEGACY_FILE: &str = "momomod.ini";

/// A mod's config file name -- the same name it would use shipped on its own.
pub fn mod_file(module: &str) -> String {
    format!("{module}.ini")
}

/// The reference file written beside a mod's config when the kit has features
/// that file has never heard of.
pub fn reference_file(module: &str) -> String {
    format!("{module}.reference.ini")
}

/// One section's keys, lowercased, in file order.
#[derive(Clone, Default)]
pub struct Section {
    keys: BTreeMap<String, String>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.get(key).map(|s| s.as_str())
    }

    /// A boolean, accepting the spellings a player might reasonably write.
    pub fn bool(&self, key: &str, default: bool) -> Result<bool, String> {
        match self.get(key) {
            None => Ok(default),
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Ok(true),
                "false" | "no" | "off" | "0" => Ok(false),
                other => Err(format!("{key}: expected true or false, found {other:?}")),
            },
        }
    }

    pub fn u64(&self, key: &str, default: u64) -> Result<u64, String> {
        match self.get(key) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("{key}: expected a whole number, found {v:?}")),
        }
    }

    /// This section's keys laid over `base`: whatever is here wins, and anything
    /// only `base` mentions is kept.
    ///
    /// How the kit's mirror overrides a mod's own file. A key absent from the
    /// mirror is absent, not empty -- which is why the mirror is generated
    /// commented out, and why uncommenting one line overrides that one setting
    /// rather than the whole section.
    pub fn over(&self, base: &Section) -> Section {
        let mut keys = base.keys.clone();
        for (k, v) in &self.keys {
            keys.insert(k.clone(), v.clone());
        }
        Section { keys }
    }

    /// The keys this section actually names, for reporting what an override took
    /// control of.
    pub fn names(&self) -> Vec<&str> {
        self.keys.keys().map(|k| k.as_str()).collect()
    }

    /// Keys this feature did not ask about, so the loader can point out a typo
    /// rather than letting a misspelled setting sit there doing nothing.
    pub fn unknown(&self, known: &[&str]) -> Vec<&str> {
        self.keys
            .keys()
            .map(|k| k.as_str())
            .filter(|k| !known.contains(k))
            .collect()
    }
}

pub struct Config {
    pub(crate) sections: BTreeMap<String, Section>,
    /// Anything we could not make sense of, reported once per load.
    pub complaints: Vec<String>,
}

impl Config {
    pub fn parse(text: &str) -> Config {
        let mut sections: BTreeMap<String, Section> = BTreeMap::new();
        let mut complaints = Vec::new();
        let mut current = String::new();

        for (n, raw) in text.lines().enumerate() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                match rest.strip_suffix(']') {
                    Some(name) => {
                        current = name.trim().to_ascii_lowercase();
                        sections.entry(current.clone()).or_default();
                    }
                    None => complaints.push(format!("line {}: unclosed section header", n + 1)),
                }
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                complaints.push(format!("line {}: no '=' in {line:?}", n + 1));
                continue;
            };
            if current.is_empty() {
                complaints.push(format!("line {}: {:?} before any section", n + 1, k.trim()));
                continue;
            }
            sections
                .entry(current.clone())
                .or_default()
                .keys
                .insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
        Config { sections, complaints }
    }

    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Config::parse(&text))
    }

    /// An empty config: every feature at its default. What the kit runs on before
    /// the file exists.
    pub fn defaults() -> Config {
        Config { sections: BTreeMap::new(), complaints: Vec::new() }
    }

    pub fn section(&self, feature: &str) -> Section {
        self.sections
            .get(&format!("feature.{}", feature.to_ascii_lowercase()))
            .cloned()
            .unwrap_or_default()
    }

    /// Any section by its literal name.
    pub fn section_named(&self, name: &str) -> Section {
        self.sections.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default()
    }

    /// The `[kit]` section.
    pub fn kit(&self) -> Section {
        self.sections.get("kit").cloned().unwrap_or_default()
    }

    /// Whether a feature is on. A section that exists but says nothing about
    /// `enabled` counts as wanting the feature at its default -- writing
    /// `[feature.x]` with a setting under it should not silently do nothing.
    pub fn enabled(&self, feature: &str, default: bool) -> bool {
        let s = self.section(feature);
        match s.bool("enabled", default) {
            Ok(v) => v,
            Err(why) => {
                logln!("config: feature.{feature}: {why} - using the default ({default})");
                default
            }
        }
    }

    /// Sections that do not correspond to anything the kit knows about.
    ///
    /// Reported, not rejected: it is far more likely to be a feature removed or a
    /// typo than a reason to refuse the whole file.
    pub fn unknown_sections(&self, features: &[&'static str]) -> Vec<&str> {
        self.sections
            .keys()
            .map(|k| k.as_str())
            .filter(|k| {
                if *k == "kit" {
                    return false;
                }
                match k.strip_prefix("feature.") {
                    Some(name) => !features.iter().any(|f| f.eq_ignore_ascii_case(name)),
                    None => true,
                }
            })
            .collect()
    }

    /// Feature names the file says nothing about, so the kit can offer a fresh
    /// reference file rather than silently using defaults for a feature the player
    /// has never heard of.
    pub fn missing_sections(&self, features: &[&'static str]) -> Vec<&'static str> {
        features
            .iter()
            .copied()
            .filter(|f| !self.sections.contains_key(&format!("feature.{}", f.to_ascii_lowercase())))
            .collect()
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find(['#', ';']) {
        Some(i) => &line[..i],
        None => line,
    }
}

/// The folder every config file lives in, if the kit knows where home is.
pub fn dir() -> Option<std::path::PathBuf> {
    home::dir().map(|d| d.join(DIR))
}

/// The path of one config file inside [`DIR`].
pub fn path(name: &str) -> Option<std::path::PathBuf> {
    dir().map(|d| d.join(name))
}

/// Where the kit kept its single config before the split.
pub fn legacy_path() -> Option<std::path::PathBuf> {
    home::file(LEGACY_FILE)
}

/// What `[mods]` in the kit's file says about one mod: on, or off.
///
/// There was a third state, `Hidden` -- off, and left out of the settings window
/// as if the kit did not have it -- for mods still in construction. It is gone,
/// because it answered a question the architecture now answers better and could
/// answer wrongly. Whether someone can *have* a mod is decided by whether it is
/// in the published catalogue and the release: an unfinished mod is simply not
/// there, which no stale config line can contradict. `Hidden` only ever governed
/// the manager's own compiled-in features, never the plugins it loads, so a
/// plugin could run while the window insisted it did not exist.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModState {
    On,
    Off,
}

impl std::fmt::Display for ModState {
    /// The spelling the config file uses, so a default can be written into the
    /// generated `[mods]` and read back as itself.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ModState::On => "true",
            ModState::Off => "false",
        })
    }
}

/// Every config file at once: the kit's, and one per mod.
///
/// Held together rather than merged, because a mod's file has to stay a document
/// that mod could read on its own. Merging would let a key from one mod satisfy
/// a lookup for another, and the first time that happened it would look like a
/// setting that mysteriously does nothing.
pub struct ConfigSet {
    kit: Config,
    mods: BTreeMap<String, Config>,
}

impl ConfigSet {
    pub fn new(kit: Config) -> ConfigSet {
        ConfigSet { kit, mods: BTreeMap::new() }
    }

    /// Everything at its default: what the kit runs on before any file exists.
    pub fn defaults() -> ConfigSet {
        ConfigSet::new(Config::defaults())
    }

    pub fn insert(&mut self, module: &str, cfg: Config) {
        self.mods.insert(module.to_ascii_lowercase(), cfg);
    }

    /// The `[kit]` section of the kit's own file.
    pub fn kit(&self) -> Section {
        self.kit.kit()
    }

    /// The kit's file, for reporting complaints against it by name.
    pub fn kit_config(&self) -> &Config {
        &self.kit
    }

    pub fn module(&self, name: &str) -> Option<&Config> {
        self.mods.get(&name.to_ascii_lowercase())
    }

    /// What `[mods]` in the kit's file says about a mod.
    ///
    /// A mod switched off here is not configured, not checked and not started --
    /// its own file is left entirely alone, so switching it back on restores the
    /// settings rather than resetting them.
    ///
    /// `hidden` is still accepted, and read as off. It was a third state once, and
    /// a config someone wrote then must not start erroring at them now.
    pub fn mod_state(&self, module: &str, default: ModState) -> ModState {
        let key = module.to_ascii_lowercase();
        let section = self.kit.sections.get("mods").cloned().unwrap_or_default();
        match section.get(&key) {
            None => default,
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => ModState::On,
                "false" | "no" | "off" | "0" | "hidden" => ModState::Off,
                other => {
                    logln!(
                        "config: [mods] {key}: expected true or false, found \
                         {other:?} - using the default ({default})"
                    );
                    default
                }
            },
        }
    }

    /// Whether a mod is loaded at all.
    pub fn mod_enabled(&self, module: &str, default: bool) -> bool {
        let default = if default { ModState::On } else { ModState::Off };
        self.mod_state(module, default) == ModState::On
    }

    /// The kit's mirrored override for one feature: `[<mod>.feature.<name>]` in
    /// `config/momomod.ini`. Empty unless the player uncommented something.
    pub fn override_for(&self, module: &str, feature: &str) -> Section {
        let key = format!(
            "{}.feature.{}",
            module.to_ascii_lowercase(),
            feature.to_ascii_lowercase()
        );
        self.kit.sections.get(&key).cloned().unwrap_or_default()
    }

    /// A feature's section: its mod's file, with the kit's mirror laid over it.
    pub fn section(&self, module: &str, feature: &str) -> Section {
        let base = match self.module(module) {
            Some(cfg) => cfg.section(feature),
            None => Section::default(),
        };
        self.override_for(module, feature).over(&base)
    }

    /// Every mirrored override actually in force, as `(mod, feature, keys)`.
    ///
    /// The loader logs these. An override is invisible from the mod's own file --
    /// that file goes on showing a value that is not in effect -- so the one place
    /// it can be made obvious is the log.
    pub fn overrides(&self, known: &[(&'static str, &'static str)]) -> Vec<String> {
        let mut out = Vec::new();
        for (module, feature) in known {
            let section = self.override_for(module, feature);
            let keys = section.names();
            if !keys.is_empty() {
                out.push(format!("{module}/{feature}: {}", keys.join(", ")));
            }
        }
        out
    }

    /// Mirror sections naming a mod or feature this build does not have.
    pub fn stray_overrides(&self, known: &[(&'static str, &'static str)]) -> Vec<&str> {
        self.kit
            .sections
            .keys()
            .map(|k| k.as_str())
            .filter(|k| k.contains(".feature."))
            .filter(|k| {
                !known.iter().any(|(m, f)| {
                    *k == format!("{}.feature.{}", m.to_ascii_lowercase(), f.to_ascii_lowercase())
                })
            })
            .collect()
    }

    /// Whether a feature is on. Off if its mod is not loaded, whatever the
    /// feature's own file says -- a mod that is switched off is switched off.
    pub fn enabled(&self, module: &str, feature: &str, default: bool) -> bool {
        if !self.mod_enabled(module, true) {
            return false;
        }
        let section = self.section(module, feature);
        match section.bool("enabled", default) {
            Ok(v) => v,
            Err(why) => {
                logln!("config: {module}/{feature}: {why} - using the default ({default})");
                default
            }
        }
    }
}

/// The manager's config drives the registry: many mods, keyed by module.
impl crate::registry::FeatureCfg for ConfigSet {
    fn enabled(&self, module: &str, feature: &str, default: bool) -> bool {
        ConfigSet::enabled(self, module, feature, default)
    }
    fn section(&self, module: &str, feature: &str) -> Section {
        ConfigSet::section(self, module, feature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_keys_and_comments() {
        let c = Config::parse(
            "\
            # a comment\n\
            [kit]\n\
            trace = true    ; trailing comment\n\
            \n\
            [feature.Skip_Splash]\n\
            enabled = yes\n\
            interval_ms = 40\n",
        );
        assert!(c.complaints.is_empty(), "{:?}", c.complaints);
        assert_eq!(c.kit().bool("trace", false), Ok(true));
        // section names are case-insensitive, since a player will not match ours
        assert!(c.enabled("skip_splash", false));
        assert_eq!(c.section("SKIP_SPLASH").u64("interval_ms", 0), Ok(40));
    }

    #[test]
    fn an_unknown_section_is_reported_not_fatal() {
        let c = Config::parse("[feature.gone]\nenabled = true\n[kit]\n");
        assert!(c.complaints.is_empty());
        assert_eq!(c.unknown_sections(&["stays"]), vec!["feature.gone"]);
        assert_eq!(c.missing_sections(&["stays"]), vec!["stays"]);
    }

    #[test]
    fn a_bad_boolean_falls_back_to_the_default_rather_than_guessing() {
        let c = Config::parse("[feature.x]\nenabled = maybe\n");
        assert!(!c.enabled("x", false));
        assert!(c.enabled("x", true));
    }

    #[test]
    fn a_section_with_no_enabled_key_uses_the_default() {
        let c = Config::parse("[feature.x]\ninterval_ms = 5\n");
        assert!(!c.enabled("x", false));
        assert!(c.enabled("x", true));
    }

    /// `hidden` was a third state and is now a spelling of off. Someone's config
    /// still says it, and it must keep switching the mod off rather than becoming
    /// an unknown value that falls through to a default of on.
    #[test]
    fn the_retired_hidden_state_still_reads_as_off() {
        let set = ConfigSet::new(Config::parse("[mods]\noptimization = hidden\n"));
        assert_eq!(set.mod_state("optimization", ModState::On), ModState::Off);
        assert!(!set.mod_enabled("optimization", true));
        assert!(!set.enabled("optimization", "popup_stutter_fix", true));
    }

    /// Every state must survive being written with `Display` and read back, or a
    /// generated `[mods]` line would not mean what the default meant.
    #[test]
    fn mod_states_round_trip_through_the_file() {
        for state in [ModState::On, ModState::Off] {
            let other = if state == ModState::On { ModState::Off } else { ModState::On };
            let set = ConfigSet::new(Config::parse(&format!("[mods]\nx = {state}\n")));
            assert_eq!(set.mod_state("x", other), state, "{state} did not round-trip");
        }
    }

    /// A value that is not a state falls back to the default, same as a bad
    /// boolean anywhere else.
    #[test]
    fn a_bad_mod_state_falls_back_to_the_default() {
        let set = ConfigSet::new(Config::parse("[mods]\noptimization = maybe\n"));
        assert_eq!(set.mod_state("optimization", ModState::Off), ModState::Off);
        assert_eq!(set.mod_state("optimization", ModState::On), ModState::On);
    }

    #[test]
    fn malformed_lines_are_collected_with_line_numbers() {
        let c = Config::parse("[kit\nstray = 1\n[kit]\nnonsense\n");
        assert_eq!(c.complaints.len(), 3, "{:?}", c.complaints);
        assert!(c.complaints[0].contains("line 1"));
    }
}
