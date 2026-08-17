//! Every feature the kit can apply, and the mods they are grouped into.
//!
//! One module per change, and [`all`] is the only list. Adding a feature is
//! writing the module, adding one line here, and naming the mod it belongs to;
//! nothing else in the kit needs to know it exists.
//!
//! ## Mods
//!
//! A **mod** is a shippable unit with its own config file. The kit is not one of
//! them: it is a manager, and on its own it changes nothing about the game. The
//! grouping is not cosmetic -- a player who wants the gameplay changes but not the
//! measurement tools should be able to say so once, and a mod lifted out of the kit
//! and shipped alone must keep reading the same file it reads here.
//!
//! See [`crate::config`].
//!
//! Order matters only for the log and the generated config, where it should read
//! sensibly to a player: diagnostics last, since they are not really features.

use crate::config::ModState;
use crate::feature::Feature;

// One compiled-in feature is left: the stutter fix. Everything else a player can
// have is a separate plugin DLL they download (see [`PUBLISHED`]), and everything
// that is *not* for players is a plugin too, so it is simply absent from their
// install rather than present-but-suppressed.
pub mod popup_stutter_fix;

// Elsewhere, and on purpose:
// * the diagnostics (timeline, profiler, dump_libraries, draw_probe, hover_probe)
//   are `tkiw-diagnostics-plugin`, unpublished -- a modder builds it and drops the
//   DLL in `mods/`. They used to live here, kept from players by a `hidden` state
//   that never governed plugin loading and could therefore lie about what was
//   running; moving them out is what let that state be deleted.
// * `fast_boot` and `font_atlases` are parked in `quarantine/` at the repo root.

/// A shippable mod: a config file, and the features that file governs.
pub struct ModInfo {
    /// Config file stem. Stable -- it is a filename a player has edited.
    pub name: &'static str,
    /// What it is, for the header of its own config file.
    pub title: &'static str,
    /// One paragraph for the kit's file, where a player chooses what to load.
    pub blurb: &'static str,
    /// What the kit does with it when `config/momomod.ini` says nothing.
    pub default: ModState,
    /// The name this mod used to ship under, if it was ever renamed.
    ///
    /// A rename must not reset anyone's settings: the old config file is
    /// carried into the new one on first write (and left beside it as
    /// `.ini.migrated`), and an old `[mods]` line still governs the mod until
    /// the player renames it.
    pub formerly: Option<&'static str>,
    /// The mod writes its own config file; the kit must not generate it, must not
    /// overwrite it, and must not report the sections in it that it does not know.
    ///
    /// For a mod whose settings are derived from the live game -- the picker's tiers
    /// and weights come from the game's own option lists -- the kit has no way to
    /// generate a useful file and every way to destroy a good one.
    pub self_configuring: bool,
}

/// The manager's own compiled-in mods.
///
/// Not the published ones -- those (bugfixes, the auto-picker) are separate plugin
/// DLLs; see [`PUBLISHED`]. One is left here: the stutter fix, off until someone
/// turns it on. Whether a player can *have* a mod is now decided by whether it is
/// in the catalogue and the release, not by a state in this file.
pub const MODS: &[ModInfo] = &[ModInfo {
    name: "optimization",
    title: "Optimization",
    blurb: "Makes the game faster without changing anything it does. Currently one \
            fix, for the stutter the floating resource numbers cause. Not yet \
            published as a downloadable mod.",
    default: ModState::Off,
    formerly: Some("qol"),
    self_configuring: false,
}];

/// A mod the manager offers players in the mod manager.
///
/// These are **plugin DLLs downloaded on request**, not compiled into the
/// manager -- so this is metadata only, the one place the catalogue is authored.
/// The manager generates `catalog.json` from it (see `render_catalog`), which
/// the mod-manager window reads.
pub struct Published {
    pub name: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    pub self_configuring: bool,
}

/// The published mods, in the order a player should meet them.
pub const PUBLISHED: &[Published] = &[
    Published {
        name: "bugfixes",
        title: "Bugfix mod",
        blurb: "Fix miscellaneous inconsistencies and minor issues.",
        self_configuring: false,
    },
    Published {
        name: "reward-picker",
        title: "Auto-picker mod",
        blurb: "Pick rewards automatically according to your preferences. No effect \
                until the config is set.",
        self_configuring: true,
    },
];

/// Every feature the manager itself carries.
///
/// One, now: the diagnostics moved to their own plugin and the two startup
/// experiments are quarantined.
pub fn all() -> Vec<Box<dyn Feature>> {
    vec![Box::new(popup_stutter_fix::PopupStutterFix::default())]
}

/// Just the names, for config validation and tests. Must agree with [`all`].
pub fn names() -> Vec<&'static str> {
    all().iter().map(|f| f.name()).collect()
}

/// The published mods as JSON, the catalogue the mod manager is built from.
///
/// Shipped as a static file (players cannot run the build), but generated from
/// [`PUBLISHED`] so the two never drift: what the manager offers is authored in
/// exactly one place.
pub fn render_catalog() -> String {
    let mut s = String::from("{\n  \"mods\": [\n");
    for (i, m) in PUBLISHED.iter().enumerate() {
        s.push_str(&format!(
            "    {{\"name\": {}, \"title\": {}, \"blurb\": {}, \"self_configuring\": {}}}",
            json_str(m.name),
            json_str(m.title),
            json_str(m.blurb),
            m.self_configuring,
        ));
        s.push_str(if i + 1 < PUBLISHED.len() { ",\n" } else { "\n" });
    }
    s.push_str("  ]\n}\n");
    s
}

/// Minimal JSON string escaping -- the kit stays dependency-free.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => {}
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The names of one mod's features, which is what its own file may mention.
pub fn names_in(module: &str) -> Vec<&'static str> {
    all()
        .iter()
        .filter(|f| f.module().eq_ignore_ascii_case(module))
        .map(|f| f.name())
        .collect()
}

/// A feature's own config keys, with defaults and an explanation, for the
/// generated file.
///
/// Kept here rather than on the trait so the generated config can be read as one
/// document; the authoritative description of each key is the doc comment on the
/// field it sets, and `configure` is what actually validates it.
fn extra_keys(name: &str) -> &'static str {
    match name {
        "popup_stutter_fix" => {
            "# Distinct fade levels for the text. Fewer is cheaper, steppier.\n\
             steps = 10\n"
        }
        _ => "",
    }
}

/// The kit's own file: tracing, which mods to load, and a mirror of their settings.
///
/// The mirror is what makes this a place to configure everything from. Each mod's
/// sections appear here **commented out**, namespaced by mod; uncomment a line and
/// it overrides that mod's own file, leave it and the mod's file decides.
///
/// Commented rather than live, deliberately. A live copy would be a second truth
/// for every setting, and the first time the two disagreed one of them would be a
/// lie with no way to tell which. Commented, the override is always something the
/// player did on purpose, and a mod installed on its own behaves identically.
pub fn render_kit_config() -> String {
    compose(&render_kit_head(), &crate::config::ConfigSet::defaults())
}

/// Everything above the mirror: the header and `[mods]`.
fn render_kit_head() -> String {
    let mut s = String::from(
        "# momomod manager\n\
         #\n\
         # This is the MANAGER's own file, not a mod's. The mods you install each\n\
         # keep their own file beside this one (bugfixes.ini, reward-picker.ini,\n\
         # ...) and that file is the only place a mod's settings live -- install,\n\
         # enable and configure them with manage-mods.py and configure.py. Nothing\n\
         # about those mods is set here.\n\
         #\n\
         # What is here is the manager itself. A player has no reason to edit this\n\
         # file.\n\
         #\n\
         # It is re-read while the game runs; press Ctrl+Alt+M in game to force a\n\
         # re-read and log what is on.\n\
         \n\
         # The features built into the manager itself, rather than downloaded as\n\
         # mods. Off unless set to true.\n\
         [mods]\n",
    );
    for m in MODS {
        for line in wrap_comment(m.blurb) {
            s.push_str(&line);
        }
        s.push_str(&format!("{} = {}\n\n", m.name, m.default));
    }

    s
}

/// Join a kit-file head to a freshly rendered mirror.
///
/// **Both generating the file and refreshing it go through here.** They used to
/// each do their own joining, and differed by one newline -- which meant the kit
/// wrote the file, saw its own write as a change, and reloaded, forever. One
/// function, so the two cannot disagree again.
fn compose(head: &str, set: &crate::config::ConfigSet) -> String {
    format!("{}\n{}", head.trim_end_matches('\n'), render_mirror(set))
}

/// The line that begins the mirror. Stable, and matched exactly: it is how the
/// kit finds the region to rewrite without disturbing anything a player wrote
/// above it.
pub const MIRROR_MARKER: &str = "# ===== mirror of the mods' own files =====";

/// The mirror region: every mod's settings, showing what each mod's file
/// currently says, with any override the player has made left live.
///
/// Regenerated from the mods' files rather than from the defaults, because a
/// mirror showing something other than what it mirrors is worse than no mirror.
/// Lines the player has uncommented are **kept live and kept verbatim** -- those
/// are overrides in force, and rewriting one back to a comment would silently
/// undo a setting.
pub fn render_mirror(set: &crate::config::ConfigSet) -> String {
    let mut s = String::from("\n");
    s.push_str(MIRROR_MARKER);
    s.push_str(
        "\n\
         # Everything below mirrors the mods' own files, so everything can be set\n\
         # from one place. Uncomment a line to override what that mod's file says;\n\
         # leave it commented and the mod's file decides.\n\
         #\n\
         # The kit refreshes the commented values from the mods' files as it reads\n\
         # them, so what you see is what those files hold. Lines you have\n\
         # uncommented are left exactly as you wrote them. For what each setting\n\
         # means, open the mod's own file.\n",
    );
    for m in MODS {
        s.push_str(&format!(
            "\n# ----- {} (mirrors {}) -----\n",
            m.name,
            crate::config::mod_file(m.name)
        ));
        for f in all().iter().filter(|f| f.module() == m.name) {
            let live = set.override_for(m.name, f.name());
            let current = set.section(m.name, f.name());
            let header = format!("[{}.feature.{}]", m.name, f.name());
            s.push('\n');
            // A section with any override in force is shown live, so that the file
            // reads the way it behaves.
            s.push_str(&if live.names().is_empty() {
                format!("# {header}\n")
            } else {
                format!("{header}\n")
            });
            for key in default_keys(f.name(), f.default_enabled()) {
                match (live.get(&key.0), current.get(&key.0)) {
                    (Some(v), _) => s.push_str(&format!("{} = {v}\n", key.0)),
                    (None, Some(v)) => s.push_str(&format!("# {} = {v}\n", key.0)),
                    (None, None) => s.push_str(&format!("# {} = {}\n", key.0, key.1)),
                }
            }
        }
    }
    s
}

/// Every key a feature's generated config offers, with its default, in file order.
///
/// Read back out of the generated text rather than kept as a second list, so the
/// two cannot drift: whatever the generator writes is exactly what the mirror
/// mirrors.
fn default_keys(feature: &str, default_enabled: bool) -> Vec<(String, String)> {
    let mut out = vec![("enabled".to_string(), default_enabled.to_string())];
    for line in extra_keys(feature).lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    out
}

/// Rewrite the mirror region of the kit's file to match the mods' files.
///
/// Returns `true` if the file was changed. **Only writes when the text actually
/// differs**, which is not an optimisation: the loader decides whether to reload
/// by watching modification times, so a rewrite on every pass would reload on
/// every pass, forever.
///
/// Everything above [`MIRROR_MARKER`] is preserved byte for byte -- that is the
/// player's `[kit]` and `[mods]`, and the kit has no business touching it.
pub fn refresh_mirror(set: &crate::config::ConfigSet) -> Result<bool, String> {
    let Some(path) = crate::config::path(crate::config::KIT_FILE) else {
        return Ok(false);
    };
    let existing = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;

    let head = match existing.find(MIRROR_MARKER) {
        Some(i) => &existing[..i],
        None => &existing[..],
    };
    let wanted = compose(head, set);
    if wanted == existing {
        return Ok(false);
    }
    std::fs::write(&path, &wanted).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

/// One mod's file: its features, and nothing about the kit.
pub fn render_mod_config(module: &str) -> String {
    let info = MODS.iter().find(|m| m.name.eq_ignore_ascii_case(module));
    let title = info.map(|m| m.title).unwrap_or(module);
    // Second paragraph is the mod's own description, and the settings window shows
    // exactly that -- so a mod is described once, here, rather than again in Python.
    let mut s = format!("# {title}\n#\n");
    for line in wrap_comment(info.map(|m| m.blurb).unwrap_or("")) {
        s.push_str(&line);
    }
    s.push_str(
        "#\n\
         # Part of TKIW's momomod Kit, and readable on its own: this file has\n\
         # nothing kit-specific in it.\n\
         #\n\
         # Each feature is independent. One that stops working on a new game build\n\
         # switches itself off, names the reason in momomod.log, and the rest carry\n\
         # on.\n\
         \n",
    );
    for f in all() {
        if !f.module().eq_ignore_ascii_case(module) {
            continue;
        }
        for line in wrap_comment(f.summary()) {
            s.push_str(&line);
        }
        s.push_str(&format!(
            "[feature.{}]\nenabled = {}\n",
            f.name(),
            f.default_enabled()
        ));
        s.push_str(extra_keys(f.name()));
        s.push('\n');
    }
    s
}

/// Wrap a summary into `# ` comment lines, so a long one does not produce a line
/// nobody can read in an editor without horizontal scrolling.
fn wrap_comment(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::from("#");
    for word in text.split_whitespace() {
        if line.len() + 1 + word.len() > 78 {
            out.push(format!("{line}\n"));
            line = String::from("#");
        }
        line.push(' ');
        line.push_str(word);
    }
    out.push(format!("{line}\n"));
    out
}

/// Write any config file that does not exist yet, and never touch one that does.
///
/// A pre-`config/` `momomod.ini` in the mod folder is **migrated rather than
/// ignored**: its `[feature.*]` sections are copied into whichever new file now
/// owns them, so splitting the config does not quietly reset a player's careful
/// settings back to the defaults. The old file is left where it is, renamed, so
/// the migration is inspectable and reversible.
pub fn write_defaults() -> Result<(), String> {
    let Some(dir) = crate::config::dir() else {
        return Err("no mod folder to write a config into".into());
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let legacy = crate::config::legacy_path().filter(|p| p.exists());
    let carried = legacy
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| crate::config::Config::parse(&t));

    let kit = dir.join(crate::config::KIT_FILE);
    if !kit.exists() {
        std::fs::write(&kit, render_kit_config()).map_err(|e| format!("{}: {e}", kit.display()))?;
    }
    for m in MODS {
        // A mod with no features yet has no file to write; one that writes its
        // own file is not the kit's to generate.
        if m.self_configuring || names_in(m.name).is_empty() {
            continue;
        }
        let path = dir.join(crate::config::mod_file(m.name));
        if path.exists() {
            continue;
        }
        let mut text = render_mod_config(m.name);
        if let Some(old) = &carried {
            text = carry_over(&text, old, m.name);
        }
        // A renamed mod's old file is the same document under its old name.
        // Carried second, so the newer file wins over the pre-split one.
        if let Some(old_name) = m.formerly {
            let old_path = dir.join(crate::config::mod_file(old_name));
            if let Ok(t) = std::fs::read_to_string(&old_path) {
                text = carry_over(&text, &crate::config::Config::parse(&t), m.name);
                let _ = std::fs::rename(&old_path, old_path.with_extension("ini.migrated"));
            }
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    if let Some(old) = legacy {
        let to = old.with_extension("ini.migrated");
        let _ = std::fs::rename(&old, &to);
    }
    Ok(())
}

/// Replace generated values with the ones a player already chose.
///
/// Deliberately line-oriented and dumb: it rewrites `key = value` inside a
/// `[feature.x]` block when the old config had an opinion about that key, and
/// leaves every comment and every key it does not recognise exactly as generated.
/// A migration that tried to be clever is a migration that loses settings.
fn carry_over(generated: &str, old: &crate::config::Config, module: &str) -> String {
    let mut out = String::with_capacity(generated.len());
    let mut section: Option<String> = None;
    for line in generated.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("[feature.") {
            section = rest.strip_suffix(']').map(|n| n.to_ascii_lowercase());
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut written = false;
        if let (Some(name), Some((key, _))) = (&section, trimmed.split_once('=')) {
            if names_in(module).iter().any(|f| f.eq_ignore_ascii_case(name)) {
                let key = key.trim().to_ascii_lowercase();
                if let Some(v) = old.section(name).get(&key) {
                    out.push_str(&format!("{key} = {v}\n"));
                    written = true;
                }
            }
        }
        if !written {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Write `<mod>.reference.ini`: the same list, freshly generated, so a player
/// whose config predates a feature can diff the two.
///
/// Written only if absent, so it does not churn on every launch. Delete it when
/// done with it; it is written again only when something is missing again.
pub fn write_reference(module: &str) {
    let Some(path) = crate::config::path(&crate::config::reference_file(module)) else {
        return;
    };
    if path.exists() {
        return;
    }
    let _ = std::fs::write(path, render_mod_config(module));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every feature must belong to a mod that exists, or its config lands in a
    /// file nothing ever reads.
    #[test]
    fn every_feature_belongs_to_a_declared_mod() {
        for f in all() {
            assert!(
                MODS.iter().any(|m| m.name == f.module()),
                "{}: module {:?} is not in MODS",
                f.name(),
                f.module()
            );
        }
    }

    /// A mod with no features would generate an empty file and puzzle a player.
    /// There is no longer a state that excuses one: a mod not ready to be met is
    /// left out of the catalogue and the release, not listed here and suppressed.
    #[test]
    fn every_mod_has_at_least_one_feature() {
        for m in MODS {
            assert!(!names_in(m.name).is_empty(), "{}: no features", m.name);
        }
    }

    /// The generated config must parse, and every feature in it must be set to the
    /// feature's own default -- otherwise installing the kit and not editing the
    /// file does something different from installing it and deleting the file.
    #[test]
    fn the_generated_configs_round_trip() {
        for m in MODS {
            let text = render_mod_config(m.name);
            let cfg = crate::config::Config::parse(&text);
            assert!(cfg.complaints.is_empty(), "{}: {:?}", m.name, cfg.complaints);
            let mine = names_in(m.name);
            assert!(cfg.unknown_sections(&mine).is_empty(), "{}", m.name);
            assert!(cfg.missing_sections(&mine).is_empty(), "{}", m.name);
            for f in all().iter().filter(|f| f.module() == m.name) {
                assert_eq!(
                    cfg.enabled(f.name(), !f.default_enabled()),
                    f.default_enabled(),
                    "{}: the generated config disagrees with the feature's default",
                    f.name()
                );
            }
        }
    }

    /// The kit's file must parse, and must mention every mod -- a mod missing from
    /// it is one a player cannot switch off.
    #[test]
    fn the_kit_config_lists_every_mod() {
        let cfg = crate::config::Config::parse(&render_kit_config());
        assert!(cfg.complaints.is_empty(), "{:?}", cfg.complaints);
        let set = crate::config::ConfigSet::new(cfg);
        for m in MODS {
            let flipped = if m.default == ModState::On { ModState::Off } else { ModState::On };
            assert_eq!(set.mod_state(m.name, flipped), m.default, "{}", m.name);
        }
    }

    /// Every key the generated config offers must be one the feature accepts, or a
    /// player uncommenting a line gets a config error from a file we wrote.
    #[test]
    fn generated_keys_are_accepted_by_their_feature() {
        for m in MODS {
            let cfg = crate::config::Config::parse(&render_mod_config(m.name));
            for mut f in all().into_iter().filter(|f| f.module() == m.name) {
                let section = cfg.section(f.name());
                assert!(
                    f.configure(&section).is_ok(),
                    "{}: rejects the config we generate for it",
                    f.name()
                );
            }
        }
    }

    /// Every feature needs a summary a player can act on, not a placeholder.
    #[test]
    fn every_feature_describes_itself() {
        for f in all() {
            assert!(
                f.summary().len() > 30,
                "{}: summary is too short to be useful",
                f.name()
            );
        }
    }

    /// The mirror must be inert as generated. If a single line of it were live,
    /// every mod file would be overridden the moment the kit was installed.
    #[test]
    fn the_generated_mirror_overrides_nothing() {
        let kit = crate::config::Config::parse(&render_kit_config());
        assert!(kit.complaints.is_empty(), "{:?}", kit.complaints);
        let set = crate::config::ConfigSet::new(kit);
        for f in all() {
            assert!(
                set.override_for(f.module(), f.name()).names().is_empty(),
                "{}: the generated mirror is live, not commented",
                f.name()
            );
        }
    }

    /// Uncommenting a mirrored line must override that one setting, and only that
    /// one -- the whole point of generating it commented.
    #[test]
    fn uncommenting_a_mirrored_line_overrides_just_that_key() {
        let kit = crate::config::Config::parse(
            "[kit]\ntrace = false\n[optimization.feature.popup_stutter_fix]\nsteps = 3\n",
        );
        let mut set = crate::config::ConfigSet::new(kit);
        set.insert(
            "optimization",
            crate::config::Config::parse(
                "[feature.popup_stutter_fix]\nenabled = true\nsteps = 10\n",
            ),
        );
        let section = set.section("optimization", "popup_stutter_fix");
        assert_eq!(section.get("steps"), Some("3"), "the override did not win");
        assert_eq!(section.get("enabled"), Some("true"), "an untouched key was lost");
        assert!(set.enabled("optimization", "popup_stutter_fix", false));
    }

    /// An override of `enabled` has to work too, or a player cannot switch a feature
    /// off from the file the kit tells them is the place to set everything.
    #[test]
    fn a_mirrored_enabled_overrides_the_mods_file() {
        let kit = crate::config::Config::parse("[optimization.feature.popup_stutter_fix]\nenabled = false\n");
        let mut set = crate::config::ConfigSet::new(kit);
        set.insert("optimization", crate::config::Config::parse("[feature.popup_stutter_fix]\nenabled = true\n"));
        assert!(!set.enabled("optimization", "popup_stutter_fix", true));
    }

    /// Every mirrored section must name a mod and feature that exist, or the mirror
    /// quietly stops matching what it mirrors.
    #[test]
    fn the_mirror_names_only_real_features() {
        let kit = crate::config::Config::parse(&render_kit_config());
        let set = crate::config::ConfigSet::new(kit);
        let pairs: Vec<(&'static str, &'static str)> =
            all().iter().map(|f| (f.module(), f.name())).collect();
        assert!(set.stray_overrides(&pairs).is_empty());
    }

    /// The mirror must cover every feature, or a player told they can set everything
    /// from one file finds one they cannot.
    #[test]
    fn the_mirror_covers_every_feature_and_key() {
        let text = render_kit_config();
        for f in all() {
            let header = format!("# [{}.feature.{}]", f.module(), f.name());
            assert!(text.contains(&header), "{} is missing from the mirror", f.name());
        }
        // and the keys, not just the headers. Only the stutter fix is compiled in
        // now -- the diagnostics are their own plugin, the published mods always
        // were, and fast_boot/font_atlases are quarantined -- so `steps` is the
        // whole set the mirror should carry.
        for key in ["steps"] {
            assert!(text.contains(&format!("# {key} =")), "{key} is missing from the mirror");
        }
    }

    /// **The no-loop property.** The kit decides whether to reload by watching
    /// modification times, and it rewrites the mirror itself. If regenerating a
    /// mirror from its own output produced different text, the kit would rewrite
    /// the file, see the file change, reload, rewrite, forever.
    #[test]
    fn regenerating_the_mirror_from_itself_changes_nothing() {
        let first = render_kit_config();
        let set = crate::config::ConfigSet::new(crate::config::Config::parse(&first));
        let head = first[..first.find(MIRROR_MARKER).expect("marker")].to_string();
        let second = compose(&head, &set);
        assert_eq!(first, second, "the mirror is not stable across a refresh");

        // and again, in case the second pass is the one that settles
        let set = crate::config::ConfigSet::new(crate::config::Config::parse(&second));
        let third = compose(&head, &set);
        assert_eq!(second, third);
    }

    /// The mirror must show what the mods' files hold, not what the defaults are --
    /// otherwise it is a mirror of nothing.
    #[test]
    fn the_mirror_shows_the_mods_current_values() {
        let mut set = crate::config::ConfigSet::new(crate::config::Config::parse("[kit]\n"));
        set.insert(
            "optimization",
            crate::config::Config::parse("[feature.popup_stutter_fix]\nsteps = 4\n"),
        );
        let text = render_mirror(&set);
        assert!(text.contains("# steps = 4"), "the mod's own value is not mirrored");
        assert!(!text.contains("# steps = 10"), "the default is mirrored instead");
    }

    /// An override the player uncommented is a setting in force. A refresh that
    /// commented it back out would silently undo it.
    #[test]
    fn a_refresh_keeps_a_live_override_live() {
        let kit = crate::config::Config::parse(
            "[kit]\ntrace = false\n[optimization.feature.popup_stutter_fix]\nsteps = 3\n",
        );
        let mut set = crate::config::ConfigSet::new(kit);
        set.insert("optimization", crate::config::Config::parse("[feature.popup_stutter_fix]\nenabled = true\n"));
        let text = render_mirror(&set);

        // the section and the overridden key come back live, not commented
        assert!(text.contains("\n[optimization.feature.popup_stutter_fix]\n"), "section was commented out");
        assert!(text.contains("\nsteps = 3\n"), "the override was lost");
        // and it survives another pass, which is what the kit actually does
        let again = crate::config::ConfigSet::new(crate::config::Config::parse(&format!(
            "[kit]\n{text}"
        )));
        assert_eq!(again.override_for("optimization", "popup_stutter_fix").get("steps"), Some("3"));
    }

    /// A player's existing settings must survive the split, or upgrading the kit
    /// silently resets a carefully-tuned file back to the defaults.
    #[test]
    fn migration_carries_old_settings_into_the_new_file() {
        let old = crate::config::Config::parse("[feature.popup_stutter_fix]\nenabled = false\n");
        let text = carry_over(&render_mod_config("optimization"), &old, "optimization");
        let cfg = crate::config::Config::parse(&text);
        // the old file's setting is carried (default is on; the old file turned it off)
        assert!(!cfg.enabled("popup_stutter_fix", true));
        // and a key the old file said nothing about keeps its generated default
        assert_eq!(cfg.section("popup_stutter_fix").get("steps"), Some("10"));
    }

    /// Migration must not damage the comments, which are the documentation.
    #[test]
    fn migration_keeps_the_generated_comments() {
        let old = crate::config::Config::parse("[feature.popup_stutter_fix]\nenabled = false\n");
        let text = carry_over(&render_mod_config("optimization"), &old, "optimization");
        assert!(text.contains("# Distinct fade levels"), "the option's comment was lost");
        assert!(
            text.contains("# Removes the in-run stutter"),
            "the feature's description was lost"
        );
    }
}
