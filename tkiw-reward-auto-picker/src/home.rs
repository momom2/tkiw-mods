//! Where the mod keeps its things, and how the DLL is told.
//!
//! The DLL lives in the game folder but owns nothing there: config, log and any
//! backup live in the mod's own folder. The DLL cannot derive that path at
//! runtime, and dropping a pointer file next to it in the game folder would
//! defeat the point, so `install.py` stamps the absolute path into the reserved
//! buffer below as it copies the DLL.
//!
//! The marker is also how `uninstall.py` proves a `version.dll` is ours before
//! deleting it.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::win;

pub const MARKER: &[u8] = b"TKIW_PICKER_MOD_DIR=";
pub const STAMP_LEN: usize = 544;

/// Reserved for the stamp. `#[used]` and `#[no_mangle]` keep the optimiser from
/// folding or dropping it, so the byte pattern is findable in the built file.
#[used]
#[no_mangle]
pub static TKIW_PICKER_MOD_DIR_STAMP: [u8; STAMP_LEN] = {
    let mut a = [0u8; STAMP_LEN];
    let mut i = 0;
    while i < MARKER.len() {
        a[i] = MARKER[i];
        i += 1;
    }
    a
};

static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Read the stamp **volatilely**, and never by indexing the static directly.
///
/// The buffer is an immutable `static`, so its contents are known to the
/// compiler, which will happily constant-fold any ordinary read of it into the
/// zeros it was built with -- and then the path the installer patched in is
/// never looked at. That failure is silent and looks exactly like "not
/// installed", so it is worth the ugliness to make the read opaque.
fn read_stamp() -> [u8; STAMP_LEN] {
    let base = core::hint::black_box(core::ptr::addr_of!(TKIW_PICKER_MOD_DIR_STAMP) as *const u8);
    let mut out = [0u8; STAMP_LEN];
    for (i, b) in out.iter_mut().enumerate() {
        *b = unsafe { core::ptr::read_volatile(base.add(i)) };
    }
    out
}

/// The stamped path as written by the installer, empty if never stamped.
fn stamped_path() -> String {
    let raw = read_stamp();
    let body = &raw[MARKER.len()..];
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    String::from_utf8_lossy(&body[..end]).to_string()
}

/// The stamped mod folder, or `None` if unstamped or the path is gone.
///
/// A missing folder is not an error to route around: it means the mod was
/// uninstalled or moved, and the right response is to disable ourselves.
pub fn dir() -> Option<PathBuf> {
    HOME.get_or_init(|| {
        let s = stamped_path();
        if s.is_empty() {
            return None;
        }
        let path = PathBuf::from(s);
        if path.is_dir() {
            Some(path)
        } else {
            None
        }
    })
    .clone()
}

/// A file inside the mod folder.
pub fn file(name: &str) -> Option<PathBuf> {
    dir().map(|d| d.join(name))
}

/// The one thing ever written outside the mod's folder: a note saying where the
/// mod folder was supposed to be, for the case where it has gone missing and
/// there is nowhere else to report it.
pub fn orphan_note() -> Option<PathBuf> {
    let stamped = stamped_path();
    let tmp = win::temp_directory()?;
    let path = PathBuf::from(tmp).join("tkiw_reward_picker_error.log");
    let msg = if stamped.is_empty() {
        "tkiw-reward-picker: this version.dll was never stamped with a mod folder.\n\
         Re-run install.py.\n"
            .to_string()
    } else {
        format!(
            "tkiw-reward-picker: the mod folder is missing.\n\
             stamped path: {stamped}\n\
             If the mod folder was moved, re-run install.py from its new location.\n"
        )
    };
    let _ = std::fs::write(&path, msg);
    Some(path)
}
