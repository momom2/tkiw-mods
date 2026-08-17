//! Gameplay quality-of-life for The King is Watching, as a momomod-loadable mod DLL.
//!
//! Draws information the game keeps to itself. The first feature is
//! [`features::unit_stats`], a mouseover panel of the numbers behind a unit's
//! hover popup -- range, attack rate, damage per hit. Each is a
//! [`Feature`](momomod_kit::feature::Feature); the [`momomod_kit::plugin`]
//! lifecycle does everything else -- resolving the game, reading `gameplay.ini`,
//! probing, timing and guarding -- so this crate is the feature list plus the
//! four ABI exports.

use std::path::PathBuf;

use momomod_kit::feature::Feature;
use tkiw_plugin::{InitContext, ABI_VERSION, OK};

mod features {
    pub mod unit_stats;
}

/// The mod's name -- the config stem and the log file. Stable: it is a file a
/// player edits.
const NAME: &str = "gameplay";

/// Every feature this mod offers.
fn all() -> Vec<Box<dyn Feature>> {
    vec![Box::new(features::unit_stats::UnitStats::default())]
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

    // Our own identity, so the log lands in gameplay.log. Home is set from the
    // config directory by plugin::start (a plugin has no DLL stamp).
    tkiw_runtime::identity::set(tkiw_runtime::Identity {
        name: "gameplay",
        marker: b"TKIW_GAMEPLAY_DIR=",
        stamp: core::ptr::null(),
        stamp_len: 0,
        log_file: "gameplay.log",
        orphan_note: "tkiw_gameplay_error.log",
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
