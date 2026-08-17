//! Loading mod DLLs and driving them through the [`tkiw_plugin`] ABI.
//!
//! This is the manager's core: momomod owns the game's lifecycle -- the crash
//! reporter, the save snapshots, the one frame hook onto the game's thread --
//! and each installed mod is a separate DLL it loads and calls. A player stores
//! only the mod DLLs they downloaded, in the `mods/` folder beside the config.
//!
//! ```text
//! <kit folder>/
//!   config/            one <mod>.ini per installed mod  (the manager's config dir)
//!   mods/              one <mod>.dll per installed mod   (what the manager loads)
//! ```
//!
//! A mod resolves the game for itself (see [`tkiw_plugin`]); the manager only
//! finds the DLLs, checks the ABI version, hands each its name and config
//! directory, and calls `momomod_frame` from its hook. A mod that declines at
//! init, or whose ABI the manager does not recognise, is logged and skipped --
//! never fatal, so one bad mod cannot stop the rest or the game.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tkiw_runtime::{logln, win};

use tkiw_plugin::{exports, InitContext, ABI_VERSION};

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type InitFn = unsafe extern "C" fn(*const InitContext) -> i32;
type FrameFn = unsafe extern "C" fn(u64);
type ShutdownFn = unsafe extern "C" fn();

struct Loaded {
    name: String,
    frame: FrameFn,
    shutdown: ShutdownFn,
    // The module handle, kept as an integer so the DLL stays mapped for the
    // session and `Loaded` stays `Send`. We never FreeLibrary: a mod may have
    // patched game code or spawned a thread, and the process is ending anyway.
    _module: usize,
}

static LOADED: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());

/// The folder mod DLLs are installed into, beside the config.
pub fn mods_dir() -> Option<PathBuf> {
    tkiw_runtime::home::dir().map(|d| d.join("mods"))
}

/// Load and initialise every mod DLL in the mods folder.
///
/// Called once at startup, on the startup thread (before the hook is armed) --
/// which is a safe window for a mod to install a patch, the same one the
/// manager's own features use. `config_dir` is handed to each mod so it finds
/// its `<name>.ini` there.
pub fn load_all(config_dir: &Path) {
    let Some(dir) = mods_dir() else { return };
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        // No mods folder is the ordinary state of a fresh install with nothing
        // installed yet; say so once and carry on.
        Err(_) => {
            logln!("mods: no {} folder yet - no mods installed", dir.display());
            return;
        }
    };

    let mut dlls: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
        .collect();
    dlls.sort();

    if dlls.is_empty() {
        logln!("mods: {} is empty - no mods installed", dir.display());
        return;
    }

    for path in dlls {
        match load_one(&path, config_dir) {
            Ok(name) => logln!("mods: loaded {name:?} from {}", path.display()),
            Err(why) => logln!("mods: {} not loaded - {why}", path.display()),
        }
    }
}

fn load_one(path: &Path, config_dir: &Path) -> Result<String, String> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("its filename is not usable as a mod name")?
        .to_string();

    let module = unsafe { win::LoadLibraryW(wide(path).as_ptr()) };
    if module.is_null() {
        return Err("LoadLibrary failed (is it a valid DLL for this architecture?)".into());
    }

    let abi = proc::<AbiVersionFn>(module, exports::ABI_VERSION)
        .ok_or("no momomod_abi_version export; not a momomod mod")?;
    let init = proc::<InitFn>(module, exports::INIT).ok_or("no momomod_init export")?;
    let frame = proc::<FrameFn>(module, exports::FRAME).ok_or("no momomod_frame export")?;
    let shutdown =
        proc::<ShutdownFn>(module, exports::SHUTDOWN).ok_or("no momomod_shutdown export")?;

    let their_abi = unsafe { abi() };
    if their_abi != ABI_VERSION {
        return Err(format!(
            "built for ABI version {their_abi}, this manager speaks {ABI_VERSION}; update one \
             of them"
        ));
    }

    let config_str = config_dir.to_string_lossy();
    let ctx = InitContext {
        abi_version: ABI_VERSION,
        name: name.as_ptr(),
        name_len: name.len(),
        config_dir: config_str.as_ptr(),
        config_dir_len: config_str.len(),
    };
    let code = unsafe { init(&ctx) };
    if code != tkiw_plugin::OK {
        // The mod logs its own reason; the code distinguishes "declined" from
        // "loaded". Declining is normal (e.g. a standalone copy is present).
        return Err(format!("declined at init (code {code})"));
    }

    LOADED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Loaded { name: name.clone(), frame, shutdown, _module: module as usize });
    Ok(name)
}

/// Drive one frame into every loaded mod. Called from the manager's frame hook.
pub fn frame(pump: u64) {
    let loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    for m in loaded.iter() {
        // A mod that faults across the ABI edge must not take the manager down.
        let f = m.frame;
        if std::panic::catch_unwind(|| unsafe { f(pump) }).is_err() {
            logln!("mods: {:?} panicked on a frame (its own guard should have caught this)", m.name);
        }
    }
}

/// Shut every loaded mod down, at process detach.
pub fn shut_down() {
    let loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    for m in loaded.iter() {
        let f = m.shutdown;
        let _ = std::panic::catch_unwind(|| unsafe { f() });
    }
}

/// How many mods are loaded, for the startup report.
pub fn count() -> usize {
    LOADED.lock().map(|g| g.len()).unwrap_or(0)
}

fn proc<T: Copy>(module: win::Hmodule, name: &str) -> Option<T> {
    // GetProcAddress wants a null-terminated ASCII name.
    let mut cname: Vec<u8> = name.bytes().collect();
    cname.push(0);
    let p = unsafe { win::GetProcAddress(module, cname.as_ptr()) };
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller pairs the export name with its true signature type.
    Some(unsafe { core::mem::transmute_copy::<*mut core::ffi::c_void, T>(&p) })
}

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}
