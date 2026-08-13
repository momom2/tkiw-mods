//! Where the mod keeps its things, and how the DLL is told.
//!
//! The DLL lives in the game folder but owns nothing there: config, log and any
//! backup live in the mod's own folder. The DLL cannot derive that path at
//! runtime, and dropping a pointer file next to it in the game folder would
//! defeat the point, so `install.py` stamps the absolute path into a reserved
//! buffer in the DLL as it copies it.
//!
//! The buffer and its marker belong to the host mod, not to this crate -- see
//! [`crate::identity`] for why. This module is the logic that reads them.
//!
//! The marker is also how `uninstall.py` proves a proxy DLL in the game folder
//! is ours before deleting it.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::identity;
use crate::win;

/// How much room a host should reserve for its stamp, marker included. 544
/// bytes comfortably holds a marker plus a `MAX_PATH`-and-then-some path in
/// UTF-8, and the installer refuses rather than truncating if it does not fit.
pub const STAMP_LEN: usize = 544;

static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Read the host's stamp buffer **volatilely**, never by indexing it.
///
/// The buffer is an immutable `static`, so its contents are known to the
/// compiler, which will happily constant-fold any ordinary read of it into the
/// zeros it was built with -- and then the path the installer patched in is
/// never looked at. That failure is silent and looks exactly like "not
/// installed", so it is worth the ugliness to make the read opaque.
fn read_stamp() -> Vec<u8> {
    let id = identity::get();
    if id.stamp.is_null() || id.stamp_len == 0 {
        return Vec::new();
    }
    let base = core::hint::black_box(id.stamp);
    let mut out = vec![0u8; id.stamp_len];
    for (i, b) in out.iter_mut().enumerate() {
        *b = unsafe { core::ptr::read_volatile(base.add(i)) };
    }
    out
}

/// The stamped path as written by the installer, empty if never stamped.
///
/// The marker is checked rather than assumed. A DLL whose buffer does not start
/// with our marker is not one of ours, and reading a path out of it would be
/// reading someone else's bytes.
pub fn stamped_path() -> String {
    let raw = read_stamp();
    let marker = identity::get().marker;
    if marker.is_empty() || raw.len() <= marker.len() || !raw.starts_with(marker) {
        return String::new();
    }
    let body = &raw[marker.len()..];
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
    let id = identity::get();
    let stamped = stamped_path();
    let tmp = win::temp_directory()?;
    let path = PathBuf::from(tmp).join(id.orphan_note);
    let msg = if stamped.is_empty() {
        format!(
            "{}: this proxy DLL was never stamped with a mod folder.\n\
             Re-run install.py.\n",
            id.name
        )
    } else {
        format!(
            "{}: the mod folder is missing.\n\
             stamped path: {stamped}\n\
             If the mod folder was moved, re-run install.py from its new location.\n",
            id.name
        )
    };
    let _ = std::fs::write(&path, msg);
    Some(path)
}
