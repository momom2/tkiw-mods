//! Getting onto the game's thread, by import-table hook.
//!
//! GameMaker is single-threaded: everything the mod eventually wants to do --
//! reading the reward queue, inspecting option cards, pressing a button -- has
//! to happen on the game's own thread, or it races the game and corrupts a run.
//!
//! Rather than detour a game function, which means patching code and getting
//! instruction lengths right, this redirects one pointer in the game's import
//! address table. The game calls `PeekMessageW` every frame from its message
//! pump, so replacing that entry gives a per-frame callback on exactly the
//! right thread. Nothing is disassembled, no code is written, and uninstalling
//! is putting the original pointer back.
//!
//! The hook is deliberately dumb: it calls the real function first, then does
//! its own work inside `catch_unwind`, and disarms itself permanently on any
//! failure. A hook that runs every frame is the last place to be clever.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

use crate::win;

type PeekMessageW = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32, u32) -> i32;

static REAL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static SLOT: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static GAME_THREAD: AtomicU64 = AtomicU64::new(0);

/// What the per-frame callback does, as a raw fn pointer so there is no
/// `static mut` to reason about. Set before `ARMED` is ever true, and the
/// release/acquire pair on `ARMED` is what publishes it.
static ON_FRAME: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn hooked(
    msg: *mut c_void,
    hwnd: *mut c_void,
    min: u32,
    max: u32,
    remove: u32,
) -> i32 {
    let real: PeekMessageW = core::mem::transmute(REAL.load(Ordering::Acquire));
    // the game's call happens first and unconditionally: whatever we do next,
    // the message pump must behave exactly as it did unmodded
    let result = real(msg, hwnd, min, max, remove);

    if ARMED.load(Ordering::Acquire) {
        let n = FRAMES.fetch_add(1, Ordering::Relaxed);
        if n == 0 {
            GAME_THREAD.store(win::GetCurrentThreadId() as u64, Ordering::Relaxed);
        }
        let raw = ON_FRAME.load(Ordering::Relaxed);
        if raw != 0 {
            let f: fn(u64) = core::mem::transmute(raw as *const ());
            if std::panic::catch_unwind(move || f(n)).is_err() {
                // never let a fault repeat sixty times a second
                ARMED.store(false, Ordering::Release);
                crate::logln!("PANIC in the frame hook - disarmed for this session.");
            }
        }
    }
    result
}

/// Install the hook. `on_frame` is called once per pump, on the game's thread.
pub fn install(image: &crate::pe::Image, base: usize, on_frame: fn(u64)) -> Result<usize, String> {
    let rva = image
        .iat_slot("USER32.dll", "PeekMessageW")
        .ok_or("no PeekMessageW import found")?;
    let slot = base + rva as usize;

    if !win::readable(slot, 8) {
        return Err(format!("IAT slot at {slot:#x} is not readable"));
    }
    let original = unsafe { core::ptr::read_volatile(slot as *const usize) };
    if original == 0 {
        return Err("IAT slot is null; the import is not bound yet".into());
    }
    // sanity: the original must point at real code, not back at us
    if !win::readable(original, 1) {
        return Err(format!("IAT target {original:#x} is not readable"));
    }

    ON_FRAME.store(on_frame as *const () as u64, Ordering::Relaxed);
    REAL.store(original as *mut c_void, Ordering::Release);
    SLOT.store(slot as u64, Ordering::Release);

    let wrote = win::with_writable(slot, 8, || unsafe {
        core::ptr::write_volatile(slot as *mut usize, hooked as *const () as usize);
    });
    if wrote.is_none() {
        return Err("could not make the IAT slot writable".into());
    }

    ARMED.store(true, Ordering::Release);
    Ok(slot)
}

/// Put the original pointer back. Safe to call when not installed.
pub fn uninstall() {
    ARMED.store(false, Ordering::Release);
    let slot = SLOT.swap(0, Ordering::AcqRel) as usize;
    let real = REAL.swap(core::ptr::null_mut(), Ordering::AcqRel);
    if slot == 0 || real.is_null() {
        return;
    }
    win::with_writable(slot, 8, || unsafe {
        core::ptr::write_volatile(slot as *mut usize, real as usize);
    });
}

pub fn frames() -> u64 {
    FRAMES.load(Ordering::Relaxed)
}

pub fn game_thread() -> u64 {
    GAME_THREAD.load(Ordering::Relaxed)
}

pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}
