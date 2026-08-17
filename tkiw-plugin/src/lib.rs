//! The contract between the momomod **loader** and a **mod DLL**.
//!
//! momomod is a loader: it owns the game's lifecycle -- the crash reporter, the
//! save snapshots, the one frame hook onto the game's thread -- and drives each
//! installed mod through the four C functions named here. A mod is a `cdylib`
//! that resolves the game itself (linking `tkiw_runtime`) and does its work when
//! the loader calls it.
//!
//! Keeping the two sides apart is the whole point: a player stores only the mod
//! DLLs they want, and a mod can be built, released and updated on its own.
//!
//! ## What a mod DLL must export
//!
//! Four functions, C ABI, exactly these names:
//!
//! ```ignore
//! #[no_mangle] pub extern "C" fn momomod_abi_version() -> u32
//! #[no_mangle] pub extern "C" fn momomod_init(ctx: *const tkiw_plugin::InitContext) -> i32
//! #[no_mangle] pub extern "C" fn momomod_frame(pump: u64)
//! #[no_mangle] pub extern "C" fn momomod_shutdown()
//! ```
//!
//! * **`momomod_abi_version`** returns [`ABI_VERSION`]. The loader refuses a mod
//!   whose number it does not recognise rather than call into it blindly.
//! * **`momomod_init`** is called once, on the game's thread, before the first
//!   frame. It receives an [`InitContext`]. Return [`OK`] to run, or a negative
//!   code to decline (the loader logs it and moves on). A mod resolves the game
//!   and installs its patches here.
//! * **`momomod_frame`** is called once per message pump, on the game's thread,
//!   from inside the loader's hook. `pump` counts up from 1. A mod that has no
//!   per-frame work leaves this empty.
//! * **`momomod_shutdown`** is called at process detach. Best-effort: the process
//!   is going away regardless, so a mod need only undo what would be unsafe to
//!   leave.
//!
//! ## Why a mod resolves the game itself
//!
//! The loader could resolve once and hand the symbol tables across, but a
//! `Runtime` is a Rust type with no stable ABI, and marshalling ~13,000 symbols
//! across a C boundary buys nothing: `Runtime::resolve` is about 100 ms, paid
//! once per mod at startup, against a launch measured in tens of seconds. So a
//! mod links `tkiw_runtime` and resolves for itself -- the same code the mod
//! would run shipped standalone, which keeps the two shapes identical.

/// The ABI the loader and this build of the contract speak. Bump it only for a
/// change the two sides cannot both tolerate; a mod built against an older
/// version is then refused rather than mis-called.
pub const ABI_VERSION: u32 = 1;

/// The success return from `momomod_init`. Anything negative declines the mod.
pub const OK: i32 = 0;

/// What the loader passes to `momomod_init`.
///
/// Every pointer is borrowed for the duration of the call only; a mod that keeps
/// a string copies it out. Strings are UTF-8 and **not** null-terminated -- the
/// paired length is authoritative.
#[repr(C)]
pub struct InitContext {
    /// The ABI version the loader is driving. Equals [`ABI_VERSION`] when the
    /// loader and the mod were built against the same contract; a mod may check
    /// it and decline on a mismatch it cannot handle.
    pub abi_version: u32,
    /// The mod's own name -- its config stem, e.g. `reward-picker`. This is how a
    /// mod knows which config file under [`config_dir`](Self::config_dir) is its
    /// own, and what to name its log.
    pub name: *const u8,
    /// Length of [`name`](Self::name) in bytes.
    pub name_len: usize,
    /// The folder holding every mod's `<name>.ini`, and where a mod may write its
    /// log. The loader's own config directory.
    pub config_dir: *const u8,
    /// Length of [`config_dir`](Self::config_dir) in bytes.
    pub config_dir_len: usize,
}

impl InitContext {
    /// The mod name as a `&str`, or `None` if the loader passed something that is
    /// not valid UTF-8 (which it never should).
    ///
    /// # Safety
    /// Only valid during the `momomod_init` call the context was passed to.
    pub unsafe fn name(&self) -> Option<&str> {
        slice_str(self.name, self.name_len)
    }

    /// The config directory as a `&str`.
    ///
    /// # Safety
    /// Only valid during the `momomod_init` call the context was passed to.
    pub unsafe fn config_dir(&self) -> Option<&str> {
        slice_str(self.config_dir, self.config_dir_len)
    }
}

unsafe fn slice_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).ok()
}

/// The exact export names the loader looks up. Named here so the loader and any
/// tooling agree with the mods, and a typo is a compile-time mismatch rather
/// than a silent "mod does nothing".
pub mod exports {
    pub const ABI_VERSION: &str = "momomod_abi_version";
    pub const INIT: &str = "momomod_init";
    pub const FRAME: &str = "momomod_frame";
    pub const SHUTDOWN: &str = "momomod_shutdown";
}
