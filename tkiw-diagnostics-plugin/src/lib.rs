//! Measurement and probing tools for The King is Watching, as a momomod-loadable
//! mod DLL.
//!
//! These change nothing about the game: they watch it and write reports. The
//! timeline says how long each phase of a launch takes, the profiler says where
//! the time goes, and the three probes dump the game's own content libraries,
//! what a drawing feature would need, and whatever the cursor is hovering.
//!
//! ## Why this is a plugin rather than part of the manager
//!
//! It used to be compiled into the manager and kept away from players by a
//! `hidden` state in the manager's config. That meant every player downloaded the
//! profiler's code and was protected from it by a flag -- and a flag can go stale,
//! which it did: a mod once ran while the settings window insisted it did not
//! exist, because `hidden` never governed plugin loading at all. As a plugin this
//! is simply absent from a player's install, which is a stronger guarantee than
//! any setting, and it is what let the `hidden` state be deleted.
//!
//! **Deliberately not in the published catalogue.** Build the workspace and drop
//! `diagnostics.dll` into the manager's `mods/` folder to use it.

use std::path::PathBuf;

use momomod_kit::feature::Feature;
use tkiw_plugin::{InitContext, ABI_VERSION, OK};

mod features {
    pub mod draw_probe;
    pub mod dump_libraries;
    pub mod hover_probe;
    pub mod profiler;
    pub mod timeline;
}

/// The mod's name -- the config stem and the log file. Stable: it is a file a
/// person edits.
const NAME: &str = "diagnostics";

/// Every tool this mod offers, in the order someone should meet them.
fn all() -> Vec<Box<dyn Feature>> {
    vec![
        Box::new(features::timeline::Timeline::default()),
        Box::new(features::profiler::Profiler::default()),
        Box::new(features::dump_libraries::DumpLibraries::default()),
        Box::new(features::draw_probe::DrawProbe::default()),
        Box::new(features::hover_probe::HoverProbe::default()),
    ]
}

/// This mod's default config, exactly as it writes on first launch.
pub fn default_config() -> String {
    momomod_kit::plugin::render_default_config(NAME, &all())
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

    // Our own identity, so the log lands in diagnostics.log. Home is set from the
    // config directory by plugin::start (a plugin has no DLL stamp).
    tkiw_runtime::identity::set(tkiw_runtime::Identity {
        name: "diagnostics",
        marker: b"TKIW_DIAGNOSTICS_DIR=",
        stamp: core::ptr::null(),
        stamp_len: 0,
        log_file: "diagnostics.log",
        orphan_note: "tkiw_diagnostics_error.log",
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
