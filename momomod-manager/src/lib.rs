//! TKIW's momomod Kit: many small changes to *The King is Watching*, in one DLL.
//!
//! Loaded as a proxy `mfreadwrite.dll` (see [`proxy`]). Everything the kit owns
//! lives in the kit's own folder; the game folder gets exactly one added file.
//!
//! ## The shape of a session
//!
//! ```text
//! DllMain            spawn a thread and get off the loader lock, nothing else
//!   startup()        home, crash reporter, save snapshot, crash-loop breadcrumb
//!     probe()        resolve the game, then arm the frame hook
//!       on_frame()   config reload, hotkey, registry.tick()  -- game's thread
//! ```
//!
//! ## The governing rule
//!
//! A feature may only cause effects that some sequence of legal player actions,
//! available at that same moment, would have caused -- with the exception of
//! optimisations, which must cause no observable effect at all beyond being
//! faster. Anything a feature cannot do confidently, it declines to do, and the
//! log says why.

use core::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use tkiw_runtime::{
    breadcrumb, fault, findln, guard, home, hook, identity, log, logln, saves, win, Identity,
    Runtime,
};

// The modding framework, re-exported so the manager's own code keeps using
// `crate::config`, `crate::feature` and `crate::registry` unchanged. The
// manager-specific config generation (the mirror, the mod catalogue) lives in
// `features`, on top of the framework's parser and `ConfigSet`.
pub use momomod_kit::{config, feature, registry};

pub mod features;
pub mod plugin_host;
pub mod proxy;

/// Short name, used in the log and in messages a player reads.
pub const NAME: &str = "momomod-kit";

/// The Draw event the kit's overlay tool draws into: `obj_display_manager`'s
/// Draw GUI End, which runs last in the GUI phase so an overlay sits on top of
/// the game's UI. Found by `draw_probe`; build-specific, and checked against
/// [`DRAW_HOST_PROLOGUE`] before anything patches it, so a game update disables
/// drawing rather than corrupting it.
const DRAW_HOST_SITE: usize = 0x1257860;

/// Its first two instructions -- `mov rax, rsp; mov [rax+0x10], rbx`, both
/// position-independent -- which the detour displaces into its trampoline.
const DRAW_HOST_PROLOGUE: &[u8] = &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10];

/// The chord that revives a session the crash-loop guard has held back, and
/// forces a config reload otherwise.
///
/// Two modifiers deliberately: the game's own bindings are letters, digits, space
/// and the arrows unmodified, so a two-modifier chord cannot collide with one. The
/// auto-picker owns Ctrl+Alt+P; this is Ctrl+Alt+M.
const HOTKEY_NAME: &str = "Ctrl+Alt+M";
fn hotkey() -> [i32; 3] {
    [win::VK_CONTROL, win::VK_MENU, win::vk_letter(b'M')]
}

// ---------------------------------------------------------------- the stamp

/// Reserved for the installer to write the kit folder's absolute path into.
///
/// Declared here rather than in `tkiw_runtime` on purpose: the buffer has to end
/// up in the built cdylib and be findable there by byte search, and a `static` in
/// an rlib survives the final link only because something happens to reference it.
/// Declaring it in the crate that produces the DLL makes that unconditional, and
/// it gives each mod its own marker so `uninstall.py` can tell one mod's proxy
/// from another's.
///
/// `#[used]` and `#[no_mangle]` keep the optimiser from folding or dropping it.
/// It must only ever be read volatilely -- see `home::stamped_path`.
const MARKER: &[u8] = b"TKIW_MOMOMOD_DIR=";

#[used]
#[no_mangle]
pub static TKIW_MOMOMOD_DIR_STAMP: [u8; home::STAMP_LEN] = {
    let mut a = [0u8; home::STAMP_LEN];
    let mut i = 0;
    while i < MARKER.len() {
        a[i] = MARKER[i];
        i += 1;
    }
    a
};

fn identify() {
    identity::set(Identity {
        name: NAME,
        marker: MARKER,
        stamp: core::ptr::addr_of!(TKIW_MOMOMOD_DIR_STAMP) as *const u8,
        stamp_len: home::STAMP_LEN,
        log_file: "momomod.log",
        orphan_note: "momomod_manager_error.log",
    });
}

// ------------------------------------------------------------------- state

/// Set once, before the frame hook is armed; read from the game's thread after.
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The resolved game, for a feature that needs it beyond the borrow it is handed.
///
/// The profiler's sampling thread needs `'static` access, which the `&Runtime` in
/// `activate` cannot give it. Sound because the runtime is published once and never
/// replaced or dropped while the process lives.
pub fn runtime() -> Option<&'static Runtime> {
    RUNTIME.get()
}
static REGISTRY: Mutex<Option<registry::Registry>> = Mutex::new(None);
static STARTED: OnceLock<Instant> = OnceLock::new();

// -------------------------------------------------------------- entry point

#[no_mangle]
pub extern "system" fn DllMain(module: win::Hmodule, reason: u32, _reserved: *mut c_void) -> i32 {
    match reason {
        win::DLL_PROCESS_ATTACH => {
            // The loader lock is held here. Loading a library, touching another
            // module, or blocking can all deadlock, so this does exactly two
            // things and neither of them is interesting.
            unsafe { win::DisableThreadLibraryCalls(module) };
            let h = unsafe {
                win::CreateThread(
                    core::ptr::null_mut(),
                    0,
                    startup,
                    core::ptr::null_mut(),
                    0,
                    core::ptr::null_mut(),
                )
            };
            if !h.is_null() {
                unsafe { win::CloseHandle(h) };
            }
        }
        win::DLL_PROCESS_DETACH => {
            // Deliberately minimal. The IAT slot is *not* restored: another mod
            // may have chained its own hook on top of ours, and putting the
            // original pointer back would orphan it. The process is going away
            // either way.
            if let Some(rt) = RUNTIME.get() {
                if let Ok(mut g) = REGISTRY.lock() {
                    if let Some(reg) = g.as_mut() {
                        reg.shut_down(rt);
                    }
                }
            }
            plugin_host::shut_down();
        }
        _ => {}
    }
    1
}

/// Startup work, off the loader lock.
extern "system" fn startup(_: *mut c_void) -> u32 {
    identify();

    if home::dir().is_none() {
        // Nowhere to log to. Leave a note where it can still be found and stay
        // disabled: no home means uninstalled or moved, and guessing would be
        // worse than doing nothing.
        home::orphan_note();
        return 0;
    }

    // Before anything that touches the game: if this session ends in a fault, the
    // log should say where.
    fault::watch();

    logln!("---- {NAME} starting ----");
    logln!("built for  : the game as of {}", guard::TARGET_BUILD);
    logln!("mod folder : {}", home::dir().unwrap().display());
    logln!("exe base   : {:#x}", win::exe_base());
    logln!("pid        : {}", unsafe { win::GetCurrentProcessId() });

    // Load the real mfreadwrite.dll from here, where blocking is safe, rather
    // than leaving it to whichever thread first calls a forwarded export.
    if !proxy::preload() {
        logln!("WARNING: could not load the real {} - video playback may fail.", proxy::REAL_NAME);
    }

    // Unconditionally, including on runs where the probe is skipped: the save is
    // the only thing here that cannot be rebuilt from source.
    match std::panic::catch_unwind(saves::snapshot) {
        Ok(Ok((dst, n))) => logln!("saves      : snapshot of {n} files -> {}", dst.display()),
        Ok(Err(e)) => logln!("WARNING: no save snapshot taken ({e})"),
        Err(_) => logln!("WARNING: no save snapshot taken (panic)"),
    }

    match breadcrumb::arm(HOTKEY_NAME) {
        breadcrumb::Armed::Skip => {}
        breadcrumb::Armed::Go(_) => {
            if std::panic::catch_unwind(probe).is_err() {
                logln!("PANIC during startup - the kit is disabled for this session.");
            }
        }
        // Held back because the last session died. That protection is worth
        // keeping, but it must not be a dead end: polling the keyboard touches no
        // game memory, so waiting here is exactly as safe as not running at all.
        breadcrumb::Armed::Held(marker) => {
            if breadcrumb::wait_for_recovery(&marker, &hotkey())
                && std::panic::catch_unwind(probe).is_err()
            {
                logln!("PANIC during startup - the kit is disabled for this session.");
            }
        }
    }
    0
}

/// Resolve the game and arm the frame hook.
///
/// Runs from the startup thread, which is **before the game's entry point**. So
/// only things derivable from the file on disk or from already-mapped code can be
/// touched here; the GML runtime does not exist yet. Anything needing it -- the
/// global container, the object registry -- resolves later, on the game's thread.
fn probe() {
    let t0 = Instant::now();
    let rt = match Runtime::resolve() {
        Ok(rt) => rt,
        Err(why) => {
            // This is the message a player will paste into a bug report, so it
            // says what failed rather than that something did.
            findln!("DISABLED: {why}");
            stood_down();
            return;
        }
    };
    findln!(
        "resolved   : {} GML functions, {} variable slots in {}ms",
        rt.syms.functions.len(),
        rt.syms.var_slots.len(),
        t0.elapsed().as_millis()
    );

    let reg = registry::Registry::new(features::all());
    let names = reg.names();
    logln!("features   : {}", names.join(", "));
    if let Ok(mut g) = REGISTRY.lock() {
        *g = Some(reg);
    }

    let Ok(()) = RUNTIME.set(rt).map_err(|_| ()) else {
        logln!("DISABLED: the runtime was already published; this is a bug.");
        stood_down();
        return;
    };
    let rt = RUNTIME.get().unwrap();

    // Name the Draw event the kit's drawing tool hangs off, so any feature can
    // draw with `tkiw_runtime::overlay`. Nothing is patched until a feature
    // actually registers a painter; this only records where.
    tkiw_runtime::overlay::set_host(rt.base + DRAW_HOST_SITE, DRAW_HOST_PROLOGUE);

    reload_config(rt, true);

    // Load the mod DLLs the player has installed. On the startup thread, a safe
    // window for a mod to patch: no game code has run yet. Each mod resolves the
    // game for itself and is driven from the frame hook below.
    if let Some(dir) = config::dir() {
        plugin_host::load_all(&dir);
        if plugin_host::count() > 0 {
            findln!("mods       : {} loaded from mods/", plugin_host::count());
        }
    }

    match hook::install(&rt.image, rt.base, on_frame) {
        Ok(slot) => {
            STARTED.get_or_init(Instant::now);
            findln!("frame hook : armed at IAT slot {slot:#x}");
        }
        Err(why) => {
            findln!("DISABLED: could not get onto the game's thread: {why}");
            stood_down();
        }
    }
}

/// The kit has decided not to run, cleanly, with nothing dangerous left in the
/// process. Drop the crash-loop breadcrumb so the *next* launch is not held back
/// and told its last session crashed -- it did not.
fn stood_down() {
    if breadcrumb::clear() {
        logln!("nothing risky is running, so the next launch is not held back.");
    }
}

// ------------------------------------------------------------- the frame hook

/// Called once per message pump, on the game's thread.
///
/// Deliberately thin. Everything expensive is a feature's business and is timed
/// as such; what happens here is only the housekeeping that has to happen
/// somewhere, and it is paced by wall-clock time because the pump rate is wildly
/// uneven -- tens of thousands per second while loading, sixty per second in play.
fn on_frame(n: u64) {
    let Some(rt) = RUNTIME.get() else { return };
    let _guard = match Reentry::enter() {
        Some(g) => g,
        // We are called from inside PeekMessageW; if game code we invoke pumps
        // messages we are called again underneath ourselves. Skipping the frame
        // is always correct and costs nothing.
        None => return,
    };
    let now = Instant::now();

    // The single most valuable thing the frame hook does that is not a feature:
    // stand the crash-loop guard down once this session has proven itself, so an
    // ordinary crash three minutes in does not cost the player the next launch.
    if let Some(started) = STARTED.get() {
        breadcrumb::clear_when_healthy(started);
    }

    // Reads are validated against a per-frame cache of known-good regions. Sound
    // only because we are on the game's own thread: nothing else can unmap or
    // reprotect anything while this function runs.
    win::flush_region_cache();

    note_when_runtime_is_up(rt);
    housekeeping(rt, now);

    if let Ok(mut g) = REGISTRY.lock() {
        if let Some(reg) = g.as_mut() {
            reg.tick(rt, now);
        }
    }

    // Drive the installed mod DLLs. Each was handed to the game on its own terms
    // at load; here it gets the game's thread once per pump, inside our guard.
    plugin_host::frame(n);
}

/// Report, once, the moment the GML runtime becomes readable.
///
/// This is a real boot measurement and not just plumbing: the gap between the
/// process starting and this line is time spent inside the GameMaker runner before
/// a single line of the game's own GML has run -- reading a 530 MB `data.win`,
/// building asset tables, creating the graphics device. Nothing a mod does can
/// shorten that, so knowing how big it is decides whether boot is worth attacking
/// at all.
fn note_when_runtime_is_up(rt: &Runtime) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAID: AtomicBool = AtomicBool::new(false);
    if SAID.load(Ordering::Relaxed) {
        return;
    }
    match rt.globals_or_err() {
        Ok(g) => {
            SAID.store(true, Ordering::Relaxed);
            findln!(
                "gml runtime: usable - container {:#x}, get-variable {:#x}",
                g.container(),
                g.getter()
            );
        }
        // Normal for the first seconds of a launch. Not logged, or it would be
        // logged sixty times a second.
        Err(_) => {}
    }
}

/// Config reloads and the hotkey, both rate-limited off the frame path.
fn housekeeping(rt: &Runtime, now: Instant) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let Ok(mut last) = LAST.lock() else { return };
        if last.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(500)) {
            return;
        }
        *last = Some(now);
    }

    if hotkey().iter().all(|&vk| win::key_down(vk)) {
        static SAID: Mutex<Option<Instant>> = Mutex::new(None);
        let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
        if said.is_none_or(|t| now.duration_since(t) > std::time::Duration::from_millis(800)) {
            *said = Some(now);
            logln!("hotkey: re-reading the config and reporting.");
            reload_config(rt, true);
        }
        return;
    }
    reload_config(rt, false);
}

/// Re-read the config if it has changed, and apply it.
///
/// On a failed read the last known-good config stays in force: a bad edit mid-run
/// must never silently change behaviour.
fn reload_config(rt: &Runtime, force: bool) {
    static SEEN: Mutex<Option<Vec<Option<std::time::SystemTime>>>> = Mutex::new(None);

    let Some(dir) = config::dir() else { return };
    // A mod with no features yet has no file: the kit never writes one, so
    // watching for it would make "a file is missing" permanently true.
    let files: Vec<std::path::PathBuf> = std::iter::once(dir.join(config::KIT_FILE))
        .chain(
            features::MODS
                .iter()
                .filter(|m| !features::names_in(m.name).is_empty())
                .map(|m| dir.join(config::mod_file(m.name))),
        )
        .collect();

    if files.iter().any(|p| !p.exists()) {
        if force {
            match features::write_defaults() {
                Ok(()) => logln!(
                    "config: wrote defaults into {} - one file per mod; edit them to \
                     switch features on.",
                    dir.display()
                ),
                Err(e) => logln!("config: could not write the config: {e}"),
            }
        } else {
            // Run on defaults regardless, so a kit that cannot write a config still
            // behaves predictably rather than not at all.
            apply(rt, &config::ConfigSet::defaults());
            return;
        }
    }

    // Any file changing is a reload of all of them: they are one setting between them,
    // and reloading only the file that moved would leave the rest at whatever they were
    // when they were last read, which is a different thing from what is on disk.
    let stamps = |files: &[std::path::PathBuf]| -> Vec<Option<std::time::SystemTime>> {
        files
            .iter()
            .map(|p| std::fs::metadata(p).ok().and_then(|m| m.modified().ok()))
            .collect()
    };
    {
        let Ok(seen) = SEEN.lock() else { return };
        if !force && seen.as_ref() == Some(&stamps(&files)) {
            return;
        }
    }

    let kit = match config::Config::load(&dir.join(config::KIT_FILE)) {
        Ok(c) => c,
        Err(why) => {
            logln!("config: {why} - keeping the settings already in force.");
            return;
        }
    };
    for c in &kit.complaints {
        logln!("config: {}: {c}", config::KIT_FILE);
    }
    let mut set = config::ConfigSet::new(kit);

    // A mirrored override is invisible from the mod's own file, which goes on
    // showing a value that is not in force. The log is the one place that can say so.
    let pairs: Vec<(&'static str, &'static str)> = features::all()
        .iter()
        .map(|f| (f.module(), f.name()))
        .collect();
    for line in set.overrides(&pairs) {
        logln!("config: {} overrides {line}", config::KIT_FILE);
    }
    for stray in set.stray_overrides(&pairs) {
        logln!("config: {}: [{stray}] names no mod and feature this build has - ignored.", config::KIT_FILE);
    }

    for m in features::MODS {
        let mine = features::names_in(m.name);
        // A mod added after the player's momomod.ini was written is not in its
        // [mods] list, so it loads at its default and nothing says so. One line, or
        // it is a mod they cannot see to switch off.
        // The name [mods] governs this mod under: its own, or -- for a file
        // predating a rename -- its old one, so a mod a player switched off
        // does not come back on under a new name.
        let mods_section = set.kit_config().section_named("mods");
        let listed_as = if mods_section.get(m.name).is_some() {
            m.name
        } else if let Some(old) = m.formerly.filter(|o| mods_section.get(o).is_some()) {
            logln!(
                "config: {} lists mod {:?} under its old name {old:?}; that still \
                 works - rename the line when convenient.",
                config::KIT_FILE,
                m.name
            );
            old
        } else {
            logln!(
                "config: {} does not list mod {:?}; its default ({}) is in force. Add it \
                 under [mods] to control it.",
                config::KIT_FILE,
                m.name,
                m.default
            );
            m.name
        };
        match set.mod_state(listed_as, m.default) {
            config::ModState::On => {}
            state => {
                let how = if state == config::ModState::Hidden {
                    "hidden"
                } else {
                    "switched off"
                };
                logln!("config: mod {:?} is {how} - {} feature(s) not loaded.", m.name, mine.len());
                continue;
            }
        }
        let file = config::mod_file(m.name);
        // A self-configuring mod's file is not in the kit's dialect and is not the
        // kit's to read: the picker's is a thousand lines of option names, which the
        // kit's parser reported as a thousand syntax errors. The mod reads its own
        // file; the kit only needs to know the mod is switched on, which lives in
        // [mods] and in the kit's mirror.
        if m.self_configuring {
            set.insert(m.name, config::Config::defaults());
            continue;
        }
        let cfg = match config::Config::load(&dir.join(&file)) {
            Ok(c) => c,
            Err(why) => {
                logln!("config: {why} - {:?} left at its defaults.", m.name);
                continue;
            }
        };
        for c in &cfg.complaints {
            logln!("config: {file}: {c}");
        }
        // A self-configuring mod's file is full of sections the kit has never heard
        // of, by design. Reporting them as typos would bury the real complaints.
        for sec in cfg.unknown_sections(&mine) {
            if m.self_configuring {
                break;
            }
            logln!("config: {file}: [{sec}] is not a feature of this mod - ignored.");
        }
        let missing = if m.self_configuring { Vec::new() } else { cfg.missing_sections(&mine) };
        if !missing.is_empty() {
            logln!(
                "config: {file} says nothing about: {}. Their defaults are in use; see {} \
                 for the current list.",
                missing.join(", "),
                config::reference_file(m.name)
            );
            features::write_reference(m.name);
        }
        set.insert(m.name, cfg);
    }

    // The mirror in the kit's file is refreshed from what the mods' files actually
    // say, so that reading it tells the truth. This writes only when something
    // changed -- and the modification times are recorded *after* it, so the kit's
    // own write is never mistaken for a player's edit.
    match features::refresh_mirror(&set) {
        Ok(true) => logln!("config: refreshed the mirror in {}.", config::KIT_FILE),
        Ok(false) => {}
        Err(why) => logln!("config: could not refresh the mirror: {why}"),
    }
    if let Ok(mut seen) = SEEN.lock() {
        *seen = Some(stamps(&files));
    }

    apply(rt, &set);
}

fn apply(rt: &Runtime, cfg: &config::ConfigSet) {
    let Ok(mut g) = REGISTRY.lock() else { return };
    let Some(reg) = g.as_mut() else { return };
    reg.apply(rt, cfg);
    for line in reg.report() {
        log::write(&line);
    }
}

/// Re-entrancy guard for the frame hook.
struct Reentry;

static IN_FRAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Reentry {
    fn enter() -> Option<Reentry> {
        use std::sync::atomic::Ordering;
        IN_FRAME
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Reentry)
    }
}

impl Drop for Reentry {
    fn drop(&mut self) {
        IN_FRAME.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stamp must be findable by byte search in the built DLL, which means the
    /// marker has to be present exactly once and the buffer big enough for a real
    /// path. `install.py` refuses rather than truncating, so this is the check that
    /// the reservation is not absurdly small.
    #[test]
    fn the_stamp_has_room_for_a_path() {
        assert!(TKIW_MOMOMOD_DIR_STAMP.starts_with(MARKER));
        assert!(
            home::STAMP_LEN - MARKER.len() > 300,
            "only {} bytes for a path",
            home::STAMP_LEN - MARKER.len()
        );
    }

    /// Feature names are config keys and appear in a player's ini, so they must be
    /// unique and lowercase-stable.
    #[test]
    fn feature_names_are_unique_and_well_formed() {
        let names = features::names();
        let mut seen = std::collections::HashSet::new();
        for n in &names {
            assert!(seen.insert(n.to_ascii_lowercase()), "duplicate feature name: {n}");
            assert!(
                n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{n}: feature names should be lowercase_with_underscores"
            );
        }
    }
}
