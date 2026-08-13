//! Who the host mod is.
//!
//! This crate is shared by more than one mod, and three things it does need to
//! know which one it is running inside: the installer's path stamp (each mod
//! stamps its own DLL behind its own marker), the log file's name, and what to
//! call itself in a message a player will read.
//!
//! Rather than thread that through every call, the host sets it once at startup
//! and everything here reads it. Set before anything else; the accessors serve
//! sane fallbacks if it was not, because a missing identity must not be a crash
//! in the code whose job is to report crashes.
//!
//! ## Why the stamp lives in the host, not here
//!
//! The reserved buffer the installer patches must end up in the **cdylib**, and
//! be findable in the built file by byte search. A `static` in an rlib survives
//! into the final link only because something references it, which is a property
//! of the optimiser rather than a promise. Declaring it in the host crate makes
//! it unconditional, and it means each mod owns its own marker rather than
//! sharing one -- so `uninstall.py` can tell one mod's proxy DLL from another's.
//!
//! See `home::stamped_path` for the other half of this: the buffer must be read
//! **volatilely**, or LLVM folds it to the zeros it was compiled with and the
//! installed path is never seen. That bug is completely silent and cost a
//! session once already.

use std::sync::OnceLock;

/// Everything the shared runtime needs to know about its host.
#[derive(Clone, Copy)]
pub struct Identity {
    /// Short lowercase name, used in messages: `"momomod-kit"`.
    pub name: &'static str,
    /// The bytes that precede the stamped path in the built DLL, including the
    /// `=`. Must be unique to this mod: `b"TKIW_MOMOMOD_DIR="`.
    pub marker: &'static [u8],
    /// Address and length of the host's reserved stamp buffer.
    pub stamp: *const u8,
    pub stamp_len: usize,
    /// Log file name inside the mod folder: `"momomod.log"`.
    pub log_file: &'static str,
    /// Name of the note written to `%TEMP%` when the mod folder is unreachable.
    /// The only thing ever written outside the mod's own folder.
    pub orphan_note: &'static str,
}

// The stamp pointer is to a `static` in the host binary, which outlives every
// thread and is never written after the installer has finished with the file on
// disk. Reads of it are volatile and byte-wise.
unsafe impl Send for Identity {}
unsafe impl Sync for Identity {}

static ID: OnceLock<Identity> = OnceLock::new();

/// Declare who we are. The first call wins; later ones are ignored rather than
/// panicking, since this is startup code and a duplicate call is a harmless
/// mistake.
pub fn set(id: Identity) {
    let _ = ID.set(id);
}

/// The host's identity, or a placeholder if `set` was never called.
///
/// The placeholder has an empty marker and a null stamp, which makes
/// `home::dir()` answer `None` -- the same answer as "installed but never
/// stamped", and handled the same way.
pub fn get() -> Identity {
    *ID.get().unwrap_or(&Identity {
        name: "tkiw-mod",
        marker: b"",
        stamp: core::ptr::null(),
        stamp_len: 0,
        log_file: "tkiw-mod.log",
        orphan_note: "tkiw_mod_error.log",
    })
}
