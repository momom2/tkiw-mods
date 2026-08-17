//! The reward auto-picker, as a momomod-loadable mod DLL.
//!
//! Thin by design: the picking logic is [`tkiw_reward_picker`], which already
//! has a hosted lifecycle from its time absorbed into the kit. This crate is only
//! the four C exports [`tkiw_plugin`] defines, translating the loader's calls
//! into that lifecycle. Building the picker this way -- a `cdylib` a player can
//! download on its own -- is what the plugin split is for.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use tkiw_plugin::{InitContext, ABI_VERSION, OK};

/// Set once init succeeds, so `momomod_frame` does nothing if the picker
/// declined (a standalone install is present, or init faulted).
static RUNNING: AtomicBool = AtomicBool::new(false);

#[no_mangle]
pub extern "C" fn momomod_abi_version() -> u32 {
    ABI_VERSION
}

#[no_mangle]
pub extern "C" fn momomod_init(ctx: *const InitContext) -> i32 {
    // A panic must not cross back into the loader; catch it and decline.
    std::panic::catch_unwind(|| unsafe { init(ctx) }).unwrap_or(-100)
}

/// # Safety
/// `ctx` is the loader's `InitContext`, valid for this call.
unsafe fn init(ctx: *const InitContext) -> i32 {
    let Some(ctx) = ctx.as_ref() else { return -1 };
    if ctx.abi_version != ABI_VERSION {
        return -2;
    }
    let Some(config_dir) = ctx.config_dir() else { return -3 };

    match tkiw_reward_picker::plugin_start(PathBuf::from(config_dir)) {
        Ok(()) => {
            RUNNING.store(true, Ordering::Relaxed);
            OK
        }
        // The picker logs the reason itself (e.g. a standalone install); a
        // negative code tells the loader it declined.
        Err(_) => -10,
    }
}

#[no_mangle]
pub extern "C" fn momomod_frame(pump: u64) {
    if !RUNNING.load(Ordering::Relaxed) {
        return;
    }
    // The loader's hook already guards re-entry and catches panics, but a plugin
    // must not rely on the host to contain its own faults across the ABI edge.
    let _ = std::panic::catch_unwind(|| tkiw_reward_picker::hosted_frame(pump));
}

#[no_mangle]
pub extern "C" fn momomod_shutdown() {
    RUNNING.store(false, Ordering::Relaxed);
}
