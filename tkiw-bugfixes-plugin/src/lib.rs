//! Bug fixes for The King is Watching, as a momomod-loadable mod DLL.
//!
//! Restores behaviour the game describes but does not do: the morale damage
//! bonus, and the Fortifications castle-HP cap. Each fix is a
//! [`Feature`](momomod_kit::Feature); the [`momomod_kit::plugin`] lifecycle does
//! everything else -- resolving the game, reading `bugfixes.ini`, probing,
//! timing and guarding -- so this crate is the feature list plus the four ABI
//! exports.

use std::path::PathBuf;

use momomod_kit::feature::Feature;
use tkiw_plugin::{InitContext, ABI_VERSION, OK};

mod features {
    pub mod fortifications_cap;
    pub mod morale_fix;
}

/// The mod's name -- the config stem and the log file. Stable: it is a file a
/// player edits.
const NAME: &str = "bugfixes";

/// This mod's default config, exactly as it writes on first launch.
///
/// Public so `dump-default-config` can stage it with a release: the mod manager
/// ships that file rather than hand-writing a second copy of this document.
pub fn default_config() -> String {
    momomod_kit::plugin::render_default_config(NAME, &all())
}

/// Every fix this mod applies.
fn all() -> Vec<Box<dyn Feature>> {
    vec![
        Box::new(features::morale_fix::MoraleFix::default()),
        Box::new(features::fortifications_cap::FortificationsCap::default()),
    ]
}

#[no_mangle]
pub extern "C" fn momomod_abi_version() -> u32 {
    ABI_VERSION
}

#[no_mangle]
pub extern "C" fn momomod_init(ctx: *const InitContext) -> i32 {
    std::panic::catch_unwind(|| unsafe { init(ctx) }).unwrap_or(-100)
}

/// # Safety
/// `ctx` is the loader's context, valid for this call.
unsafe fn init(ctx: *const InitContext) -> i32 {
    let Some(ctx) = ctx.as_ref() else { return -1 };
    if ctx.abi_version != ABI_VERSION {
        return -2;
    }
    let Some(config_dir) = ctx.config_dir() else { return -3 };

    // Our own identity, so the log lands in bugfixes.log. Home is set from the
    // config directory by plugin::start (a plugin has no DLL stamp).
    tkiw_runtime::identity::set(tkiw_runtime::Identity {
        name: "bugfixes",
        marker: b"TKIW_BUGFIXES_DIR=",
        stamp: core::ptr::null(),
        stamp_len: 0,
        log_file: "bugfixes.log",
        orphan_note: "tkiw_bugfixes_error.log",
    });

    momomod_kit::plugin::start(NAME, PathBuf::from(config_dir), all());
    OK
}

#[no_mangle]
pub extern "C" fn momomod_frame(pump: u64) {
    let _ = std::panic::catch_unwind(|| momomod_kit::plugin::frame(pump));
}

#[no_mangle]
pub extern "C" fn momomod_shutdown() {
    let _ = std::panic::catch_unwind(momomod_kit::plugin::shut_down);
}
