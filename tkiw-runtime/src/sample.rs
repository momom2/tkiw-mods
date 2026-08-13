//! Taking a stack sample of another thread, without deadlocking the game.
//!
//! The game's thread cannot profile itself: the frame hook only gets control at the
//! message pump, which is precisely when the game is *not* doing the work worth
//! measuring. So a separate thread suspends it, reads its context, walks its stack,
//! and resumes it.
//!
//! That is a genuinely dangerous thing to do, and everything in this module is
//! shaped by two hazards.
//!
//! ## Hazard 1: never allocate or lock while the target is suspended
//!
//! A suspended thread may be holding the process heap lock, or the log mutex, or a
//! CRT lock. Anything this module does between `SuspendThread` and `ResumeThread`
//! that waits on one of those is an instant, total hang of the game -- and a hang is
//! strictly worse for a player than a crash, because there is nothing to report.
//!
//! So inside the suspend window: **no allocation, no logging, no mutex, and no
//! `win::readable`.** Frames go into a caller-supplied fixed array; everything else
//! happens after the resume. `win::readable` is specifically banned because it is
//! backed by a shared cache -- exactly the shape of thing that deadlocks here.
//!
//! ## Hazard 2: never call `RtlLookupFunctionEntry`
//!
//! The obvious way to walk an x64 stack is `RtlLookupFunctionEntry` +
//! `RtlVirtualUnwind`. The second is safe -- it interprets unwind codes and reads
//! the target's stack, taking no locks. The first is not: it consults the loader's
//! module and dynamic-function-table state, which the suspended thread may be
//! holding.
//!
//! It is also unnecessary. We already parse the game's `.pdata` from the file on
//! disk, so we can find the `RUNTIME_FUNCTION` ourselves and hand it straight to
//! `RtlVirtualUnwind`. The lookup becomes a binary search over our own memory,
//! and the loader is never involved.
//!
//! ## What a sample can and cannot see
//!
//! Unwind data is taken from **every** module in the process, not just the game's
//! (see [`crate::modules`]), so a stack is walked through `ntdll`, `d3d11` and the
//! graphics driver back into the game code that called them. That matters: a first
//! version stopped at the first foreign frame, and two thirds of boot samples
//! reduced to "outside the game", which names nothing and blames nobody.
//!
//! What remains out of reach is a frame in no module at all -- JIT, or a manually
//! mapped image. There is no unwind data for it and no honest basis for guessing, so
//! the walk stops there.

use core::ffi::c_void;

use crate::win::{self, Handle};

/// Deepest stack a sample will record. Deeper is more time suspended, and GML call
/// chains are not deep; the compiled closures nest, but not far.
pub const MAX_FRAMES: usize = 32;

/// `CONTEXT` for x86-64, as an opaque aligned blob.
///
/// Declared this way on purpose. The struct is 1,232 bytes of registers,
/// segment selectors, debug registers, a 512-byte legacy FPU save area and 26
/// vector registers, of which this module reads three fields -- and
/// `RtlVirtualUnwind` needs all of it, correctly aligned, whether we name the
/// fields or not. Transcribing it would be forty lines of opportunity for an
/// off-by-eight that produces a plausible wrong answer.
#[repr(C, align(16))]
pub struct Context {
    bytes: [u8; 1232],
}

// Offsets into CONTEXT_AMD64. Stable since Windows XP x64; part of the platform
// ABI, not an implementation detail.
const CTX_FLAGS: usize = 0x30;
const CTX_RSP: usize = 0x98;
const CTX_RIP: usize = 0xF8;

/// `CONTEXT_FULL | CONTEXT_FLOATING_POINT`. The floating-point area is included
/// because some unwind codes restore non-volatile XMM registers, and
/// `RtlVirtualUnwind` will write them into the save area.
const CONTEXT_WANTED: u32 = 0x0010_000F;

impl Context {
    pub fn new() -> Context {
        let mut c = Context { bytes: [0; 1232] };
        c.set_u32(CTX_FLAGS, CONTEXT_WANTED);
        c
    }

    fn set_u32(&mut self, at: usize, v: u32) {
        self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn u64_at(&self, at: usize) -> u64 {
        u64::from_le_bytes(self.bytes[at..at + 8].try_into().unwrap_or([0; 8]))
    }

    pub fn rip(&self) -> usize {
        self.u64_at(CTX_RIP) as usize
    }

    pub fn rsp(&self) -> usize {
        self.u64_at(CTX_RSP) as usize
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.bytes.as_mut_ptr() as *mut c_void
    }
}

impl Default for Context {
    fn default() -> Context {
        Context::new()
    }
}

pub const THREAD_SUSPEND_RESUME: u32 = 0x0002;
pub const THREAD_GET_CONTEXT: u32 = 0x0008;
pub const THREAD_QUERY_INFORMATION: u32 = 0x0040;

#[link(name = "kernel32")]
extern "system" {
    pub fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> Handle;
    fn SuspendThread(thread: Handle) -> u32;
    fn ResumeThread(thread: Handle) -> u32;
    fn GetThreadContext(thread: Handle, ctx: *mut c_void) -> i32;
}

type RtlVirtualUnwindFn = unsafe extern "system" fn(
    handler_type: u32,
    image_base: u64,
    control_pc: u64,
    function_entry: *const c_void,
    context: *mut c_void,
    handler_data: *mut *mut c_void,
    establisher_frame: *mut u64,
    context_pointers: *mut c_void,
) -> *mut c_void;

/// `RtlVirtualUnwind`, resolved from `ntdll` at runtime.
///
/// Resolved rather than linked so the build needs no `ntdll.lib`, which keeps the
/// no-crates, nothing-but-rustup property. Resolved **once, before any sampling**,
/// because `GetProcAddress` is not something to call with a thread suspended.
pub fn virtual_unwind() -> Option<RtlVirtualUnwindFn> {
    use core::sync::atomic::{AtomicUsize, Ordering};
    static CACHED: AtomicUsize = AtomicUsize::new(0);
    let cached = CACHED.load(Ordering::Acquire);
    if cached != 0 {
        return Some(unsafe { core::mem::transmute::<usize, RtlVirtualUnwindFn>(cached) });
    }
    let name = win::wide("ntdll.dll");
    // ntdll is always already loaded; GetModuleHandle avoids a refcount and any
    // chance of a load.
    let ntdll = unsafe { win::GetModuleHandleW(name.as_ptr()) };
    if ntdll.is_null() {
        return None;
    }
    let p = unsafe { win::GetProcAddress(ntdll, b"RtlVirtualUnwind\0".as_ptr()) };
    if p.is_null() {
        return None;
    }
    CACHED.store(p as usize, Ordering::Release);
    Some(unsafe { core::mem::transmute::<*mut c_void, RtlVirtualUnwindFn>(p) })
}

/// A handle to the thread being profiled.
pub struct Target {
    handle: Handle,
}

impl Target {
    pub fn open(thread_id: u32) -> Option<Target> {
        let h = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                thread_id,
            )
        };
        (!h.is_null()).then_some(Target { handle: h })
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { win::CloseHandle(self.handle) };
        }
    }
}

/// Where to look up unwind data.
///
/// This is [`crate::modules::Modules`] -- every module in the process, with the
/// exception directory of each. Supplying it ourselves is what keeps
/// `RtlLookupFunctionEntry` out of the suspend window, and covering *every* module
/// rather than only the game's is what lets a stack be walked through `ntdll` and
/// `d3d11` back into the game code that was waiting on them.
pub type Unwinder = crate::modules::Modules;

/// One sample: suspend, capture, walk, resume.
///
/// Writes return addresses into `frames` innermost-first and returns how many were
/// written. `Err` means the sample could not be taken at all, which is normal and
/// not worth logging per occurrence -- a thread can be suspended mid-transition.
///
/// # Safety
/// Suspending a thread of your own process is inherently hazardous. This is safe
/// only because of the discipline described in this module's documentation: between
/// the suspend and the resume, nothing allocates, locks, logs, or calls into the
/// loader. **Do not add anything to the marked region.**
pub fn capture(
    target: &Target,
    unwinder: &Unwinder,
    unwind: RtlVirtualUnwindFn,
    frames: &mut [usize; MAX_FRAMES],
) -> Result<usize, ()> {
    let mut ctx = Context::new();
    let mut n = 0usize;

    // ---- the suspend window begins. Nothing here may allocate, lock or log. ----
    if unsafe { SuspendThread(target.handle) } == u32::MAX {
        return Err(());
    }

    let got = unsafe { GetThreadContext(target.handle, ctx.as_ptr()) } != 0;
    if got {
        let mut handler_data: *mut c_void = core::ptr::null_mut();
        let mut establisher: u64 = 0;

        while n < frames.len() {
            let pc = ctx.rip();
            if pc == 0 {
                break;
            }
            frames[n] = pc;
            n += 1;

            // An address in no module at all cannot be unwound: no unwind data, and
            // no basis for guessing. Rare -- JIT or a manually mapped image.
            if unwinder.find(pc).is_none() {
                break;
            }
            let rsp_before = ctx.rsp();

            match unwinder.entry_for(pc) {
                Some((image_base, entry)) => unsafe {
                    unwind(
                        0, // UNW_FLAG_NHANDLER: we are not running handlers
                        image_base as u64,
                        pc as u64,
                        entry as *const c_void,
                        ctx.as_ptr(),
                        &mut handler_data,
                        &mut establisher,
                        core::ptr::null_mut(),
                    );
                },
                None => {
                    // A leaf with no unwind data: the return address is at *rsp.
                    // Reading the stack of a thread we have suspended is sound --
                    // its own rsp points into its committed stack by construction.
                    if rsp_before == 0 || rsp_before % 8 != 0 {
                        break;
                    }
                    let ret = unsafe { core::ptr::read_volatile(rsp_before as *const u64) };
                    ctx.set_u64(CTX_RIP, ret);
                    ctx.set_u64(CTX_RSP, (rsp_before + 8) as u64);
                }
            }

            // The stack pointer must move towards the base of the stack on every
            // frame. If it does not, the unwind is not making progress and would
            // loop until it filled the array with nonsense.
            if ctx.rsp() <= rsp_before {
                break;
            }
        }
    }

    unsafe { ResumeThread(target.handle) };
    // ---- the suspend window ends. Allocation is fine again from here. ----

    if got {
        Ok(n)
    } else {
        Err(())
    }
}

impl Context {
    fn set_u64(&mut self, at: usize, v: u64) {
        self.bytes[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_the_size_and_alignment_windows_expects() {
        assert_eq!(core::mem::size_of::<Context>(), 1232);
        assert_eq!(core::mem::align_of::<Context>(), 16);
    }

    #[test]
    fn context_flags_are_set_where_getthreadcontext_looks() {
        let c = Context::new();
        assert_eq!(
            u32::from_le_bytes(c.bytes[CTX_FLAGS..CTX_FLAGS + 4].try_into().unwrap()),
            CONTEXT_WANTED
        );
    }

    /// The one function this module resolves at runtime. If it is not there, the
    /// profiler cannot work at all, so it is worth knowing at build time.
    #[test]
    fn rtl_virtual_unwind_resolves() {
        assert!(virtual_unwind().is_some(), "ntdll has no RtlVirtualUnwind");
    }

    /// Sampling a real thread, end to end, with a stack we know the shape of. This
    /// is the test that the CONTEXT offsets and the unwind plumbing are right --
    /// getting either wrong yields plausible garbage rather than an error.
    #[test]
    fn captures_this_process_own_stack() {
        let unwind = virtual_unwind().expect("RtlVirtualUnwind");
        let unwinder = Unwinder::snapshot();
        assert!(!unwinder.is_empty(), "no modules found");

        // A thread that parks in a known place, so there is a real stack to walk.
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d = done.clone();
        let ids = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let i = ids.clone();
        let t = std::thread::spawn(move || {
            *i.lock().unwrap() = unsafe { win::GetCurrentThreadId() };
            while !d.load(std::sync::atomic::Ordering::Relaxed) {
                std::hint::spin_loop();
            }
        });
        // Let it get going and publish its id.
        let tid = loop {
            let id = *ids.lock().unwrap();
            if id != 0 {
                break id;
            }
            std::thread::yield_now();
        };

        let target = Target::open(tid).expect("open the thread");
        let mut frames = [0usize; MAX_FRAMES];
        let mut best = 0;
        // A suspend can legitimately fail or catch the thread mid-transition, so
        // take the best of a few rather than asserting on one.
        for _ in 0..20 {
            if let Ok(n) = capture(&target, &unwinder, unwind, &mut frames) {
                best = best.max(n);
            }
            std::thread::yield_now();
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        t.join().ok();

        assert!(best >= 2, "walked only {best} frame(s); the unwind is not working");
        assert!(frames[0] != 0);
    }

    /// Walking **out of a foreign module** and back into our own code.
    ///
    /// This is the capability the first version lacked, and it is the reason two
    /// thirds of a boot profile said only "outside the game". A sleeping thread is
    /// parked deep inside `ntdll`, so a walk that cannot cross a module boundary
    /// returns one or two frames, all of them Windows'. A walk that can gets back to
    /// the closure this test spawned.
    #[test]
    fn walks_out_of_ntdll_into_our_own_code() {
        let unwind = virtual_unwind().expect("RtlVirtualUnwind");
        let unwinder = Unwinder::snapshot();
        let own_base = win::exe_base();
        let own_size = unwinder.find(own_base).map(|m| m.size).unwrap_or(0);
        assert!(own_size > 0, "our own module was not found");

        let ids = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let i = ids.clone();
        let t = std::thread::spawn(move || {
            *i.lock().unwrap() = unsafe { win::GetCurrentThreadId() };
            // Parked in a kernel wait, i.e. innermost frames inside ntdll.
            std::thread::sleep(std::time::Duration::from_millis(1500));
        });
        let tid = loop {
            let id = *ids.lock().unwrap();
            if id != 0 {
                break id;
            }
            std::thread::yield_now();
        };
        std::thread::sleep(std::time::Duration::from_millis(50));

        let target = Target::open(tid).expect("open the thread");
        let mut frames = [0usize; MAX_FRAMES];
        let mut deepest = Vec::new();
        for _ in 0..10 {
            if let Ok(n) = capture(&target, &unwinder, unwind, &mut frames) {
                if n > deepest.len() {
                    deepest = frames[..n].to_vec();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        t.join().ok();

        assert!(deepest.len() >= 4, "walked only {} frames from a sleeping thread", deepest.len());
        let names: Vec<String> = deepest.iter().map(|&a| unwinder.name_of(a)).collect();
        assert!(
            names.iter().any(|n| n == "ntdll.dll"),
            "expected a sleeping thread to be inside ntdll, got {names:?}"
        );
        let reached_us = deepest
            .iter()
            .any(|&a| a >= own_base && a < own_base + own_size);
        assert!(reached_us, "never got back into our own module: {names:?}");
    }
}
