//! The whole plugin lifecycle behind the four ABI exports.
//!
//! A mod's own crate is its features plus a one-line hand-off to [`start`],
//! [`frame`] and [`shut_down`] here. This resolves the game (on its own thread,
//! so `momomod_init` returns at once), writes a default `<mod>.ini` if none
//! exists, and then probes, configures, times and guards the features through
//! the shared [`Registry`] -- the same machinery the manager runs its own
//! features on.
//!
//! Everything is a process global because a mod DLL is one module driven by one
//! loader; there is only ever one of each.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use tkiw_runtime::{home, logln, Runtime};

use crate::config::{Config, Section};
use crate::feature::Feature;
use crate::registry::{FeatureCfg, Registry};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);
static MODULE: OnceLock<String> = OnceLock::new();
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
static SEEN: Mutex<Option<Option<SystemTime>>> = Mutex::new(None);
static STARTED: OnceLock<Instant> = OnceLock::new();

/// One mod's config, adapting a single `.ini` to the registry's [`FeatureCfg`].
/// The module argument is ignored: all a plugin's features are its own module.
struct ModCfg {
    cfg: Config,
    /// The mod-level master switch, `[mod] enabled`. When off, no feature runs,
    /// whatever its own line says -- so the whole mod is one toggle.
    mod_enabled: bool,
}

impl FeatureCfg for ModCfg {
    fn enabled(&self, _module: &str, feature: &str, default: bool) -> bool {
        self.mod_enabled && self.cfg.enabled(feature, default)
    }
    fn section(&self, _module: &str, feature: &str) -> Section {
        self.cfg.section(feature)
    }
}

/// Start the mod. `config_dir` is the loader's config folder; the mod reads and
/// writes `<module>.ini` there. Returns at once: the game is resolved on a
/// background thread, and the features come up once it is ready.
///
/// The caller sets its own identity (log file, name) before this, since those
/// are the mod's static strings; this sets `home` from the config directory.
pub fn start(module: &str, config_dir: PathBuf, features: Vec<Box<dyn Feature>>) {
    if let Some(parent) = config_dir.parent() {
        // A plugin has no DLL stamp; the loader's config dir is where home is.
        home::set_dir(parent.to_path_buf());
    }
    let _ = MODULE.set(module.to_string());
    let _ = CONFIG_PATH.set(config_dir.join(format!("{module}.ini")));
    write_default_config_if_absent(&features);
    if let Ok(mut g) = REGISTRY.lock() {
        *g = Some(Registry::new(features));
    }

    // Resolving reads the whole executable's symbol tables; off the loader's
    // thread so init returns and the game keeps coming up. This thread only
    // resolves and publishes the runtime -- it does **not** apply the features.
    //
    // Applying is where a feature patches game code, and that must happen on the
    // game's own thread, inside the message pump, where the game is provably not
    // executing the patch site. This background thread is neither. So the first
    // apply is deferred to [`frame`], which the loader calls from its hook, on
    // the game's thread -- the same window the manager's own features patch in.
    std::thread::Builder::new()
        .name("momomod-mod-probe".into())
        .spawn(|| match Runtime::resolve() {
            Ok(rt) => {
                let _ = RUNTIME.set(rt);
                STARTED.get_or_init(Instant::now);
            }
            Err(why) => logln!("DISABLED: could not resolve the game: {why}"),
        })
        .ok();
}

/// Drive one frame. Cheap until the game is resolved; then a rate-limited
/// config reload and one pass over the features.
pub fn frame(_pump: u64) {
    let Some(rt) = RUNTIME.get() else { return };
    reload_and_apply(false);
    if let Ok(mut g) = REGISTRY.lock() {
        if let Some(reg) = g.as_mut() {
            reg.tick(rt, Instant::now());
        }
    }
}

/// Deactivate everything, at process detach.
pub fn shut_down() {
    let Some(rt) = RUNTIME.get() else { return };
    if let Ok(mut g) = REGISTRY.lock() {
        if let Some(reg) = g.as_mut() {
            reg.shut_down(rt);
        }
    }
}

/// Re-read `<module>.ini` if it changed, and apply it. On `force`, always.
fn reload_and_apply(force: bool) {
    let Some(rt) = RUNTIME.get() else { return };
    let Some(path) = CONFIG_PATH.get() else { return };

    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    {
        let Ok(seen) = SEEN.lock() else { return };
        if !force && *seen == Some(mtime) {
            return;
        }
    }

    let cfg = match Config::load(path) {
        Ok(c) => c,
        // Keep the settings already in force on a bad read, like the manager.
        Err(_) if !force => return,
        Err(_) => Config::defaults(),
    };
    for c in &cfg.complaints {
        logln!("config: {c}");
    }

    // The mod-level master switch. Off by default, so an installed-but-untouched
    // mod does nothing.
    let mod_enabled = cfg.section_named("mod").bool("enabled", false).unwrap_or(false);
    if let Ok(mut g) = REGISTRY.lock() {
        if let Some(reg) = g.as_mut() {
            reg.apply(rt, &ModCfg { cfg, mod_enabled });
            for line in reg.report() {
                tkiw_runtime::log::write(&line);
            }
        }
    }
    if let Ok(mut seen) = SEEN.lock() {
        *seen = Some(mtime);
    }
}

/// Write a default `<module>.ini` from the features, if the file is absent.
///
/// A mod owns its config file, and this is the only place it is written -- so a
/// mod has exactly one config, its own `<mod>.ini`, never a second copy in the
/// manager's file.
///
/// **A freshly installed mod is switched off** by its `[mod] enabled = false`
/// master switch, so installing a mod changes nothing until it is turned on (in
/// `configure.py`, the mod manager, or by installing it with "enable"). The
/// individual features keep their own conservative defaults, which is what runs
/// once the mod is switched on. An existing file is never touched.
fn write_default_config_if_absent(features: &[Box<dyn Feature>]) {
    let Some(path) = CONFIG_PATH.get() else { return };
    if path.exists() {
        return;
    }
    let module = MODULE.get().map(String::as_str).unwrap_or("mod");
    let s = render_default_config(module, features);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, s);
}

/// Render a mod's default config: the document a fresh install starts from.
///
/// **The one place this text exists.** The mod writes it on first launch, and
/// the release ships the same rendering as `<mod>.default.ini` so the mod
/// manager can enable a mod before it has ever run without hand-writing a
/// second copy that drifts from this one.
pub fn render_default_config(module: &str, features: &[Box<dyn Feature>]) -> String {
    let mut s = format!(
        "# {module}\n#\n# A momomod mod, and the one place its settings live. Each \
         feature is\n# independent; one that stops working on a new game build switches \
         itself off,\n# names the reason in the log, and the rest carry on.\n\n\
         # The master switch for the whole mod. Off until you turn it on; then each\n\
         # feature below runs per its own line.\n\
         [mod]\n\
         enabled = false\n\n"
    );
    for f in features {
        for line in wrap_comment(f.summary()) {
            s.push_str(&line);
        }
        s.push_str(&format!("[feature.{}]\nenabled = {}\n", f.name(), f.default_enabled()));
        s.push_str(f.config_template());
        s.push('\n');
    }
    s
}

/// Wrap a summary into `# ` comment lines so a long one is readable in an editor.
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
