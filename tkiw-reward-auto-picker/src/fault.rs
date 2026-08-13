//! Recording where the game died.
//!
//! The log records what *succeeded*. When an access violation takes the process
//! down there is no unwinding, no `catch_unwind`, and no chance to write
//! anything -- so the last line is whatever last finished, which has twice now
//! been a successful pick with the real fault somewhere after it.
//!
//! A vectored exception handler runs *before* the game's own handlers, so it
//! gets to see the fault first. This one writes down the exception code, the
//! faulting instruction, and the address that was touched, then hands the
//! exception straight on. It changes nothing about what happens next: if the
//! game would have crashed, it still crashes, and if the game handles the
//! exception itself, it still handles it.
//!
//! That is the whole point. The last time a fault address was available it took
//! one reading to find the cause -- a null `self` passed to a method whose first
//! instruction dereferenced it. Guessing at the same question without an address
//! has now cost two sessions.

use core::ffi::c_void;

use crate::win;

const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xC000_001D;
const EXCEPTION_STACK_OVERFLOW: u32 = 0xC000_00FD;
/// Do not handle it, do not swallow it -- just carry on down the chain.
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    record: *mut ExceptionRecord,
    address: *mut c_void,
    n_params: u32,
    _pad: u32,
    info: [usize; 15],
}

#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut c_void,
}

extern "system" {
    fn AddVectoredExceptionHandler(
        first: u32,
        handler: extern "system" fn(*mut ExceptionPointers) -> i32,
    ) -> *mut c_void;
    fn SetUnhandledExceptionFilter(
        filter: extern "system" fn(*mut ExceptionPointers) -> i32,
    ) -> *mut c_void;
}

/// A dedicated file handle, opened before anything can go wrong.
///
/// Twice now the game has died and neither handler has written a word. The
/// handler was doing exactly the things a dying process is worst at: formatting
/// strings, joining paths, opening files -- every one of them an allocation, on
/// a heap that may be the reason we are here, from a stack that may have run
/// out. So the handle is opened at startup and the report is assembled in a
/// fixed stack buffer with no allocation at all.
static REPORT: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

/// Start watching. Safe to call more than once; only the first takes effect.
pub fn watch() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(path) = crate::home::file("crash.log") {
            if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                if let Ok(mut g) = REPORT.lock() {
                    *g = Some(f);
                }
            }
        }
        // 0 = last in the chain. The game's own handlers get first refusal, so
        // an exception it deals with routinely is not reported as a fault.
        let h = unsafe { AddVectoredExceptionHandler(0, on_exception) };
        // A second net. The vectored handler can be defeated -- a stack
        // overflow leaves too little stack to run one -- and last time the game
        // died there was no fault block at all, which is itself a clue. The
        // unhandled filter runs later, from a different place, so between them
        // something should get written.
        unsafe { SetUnhandledExceptionFilter(on_unhandled) };
        crate::findln!(
            "crash reporter: {}",
            if h.is_null() { "FAILED to install" } else { "watching" }
        );
    });
}

extern "system" fn on_exception(p: *mut ExceptionPointers) -> i32 {
    // Whatever happens here, the exception must continue on its way unchanged.
    let _ = std::panic::catch_unwind(|| report(p));
    EXCEPTION_CONTINUE_SEARCH
}

/// Last chance, once nothing else has claimed the exception.
extern "system" fn on_unhandled(p: *mut ExceptionPointers) -> i32 {
    let _ = std::panic::catch_unwind(|| report(p));
    EXCEPTION_CONTINUE_SEARCH
}

fn report(p: *mut ExceptionPointers) {
    if p.is_null() {
        return;
    }
    let rec = unsafe { (*p).record };
    if rec.is_null() {
        return;
    }
    let (code, at) = unsafe { ((*rec).code, (*rec).address as usize) };

    // Only the fatal kinds. A running game raises others routinely -- C++
    // exceptions, debugger notifications -- and reporting those would bury the
    // one line that matters.
    if !matches!(
        code,
        EXCEPTION_ACCESS_VIOLATION | EXCEPTION_ILLEGAL_INSTRUCTION | EXCEPTION_STACK_OVERFLOW
    ) {
        return;
    }

    // A fault can repeat; say it once and let the process get on with dying.
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let (n, info) = unsafe { ((*rec).n_params as usize, (*rec).info) };
    let base = win::exe_base();

    let mut buf = Buf::new();
    buf.put(b"---- FAULT ----
code ");
    buf.hex(code as usize);
    buf.put(b" at ");
    buf.hex(at);
    buf.put(b"
game base ");
    buf.hex(base);
    buf.put(b"
rva ");
    buf.hex(at.wrapping_sub(base));
    if code == EXCEPTION_ACCESS_VIOLATION && n >= 2 {
        buf.put(match info[0] {
            0 => b"
while reading ".as_slice(),
            1 => b"
while writing ".as_slice(),
            8 => b"
while executing ".as_slice(),
            _ => b"
while touching ".as_slice(),
        });
        buf.hex(info[1]);
    }
    buf.put(b"
last phase: ");
    let mut phase = [0u8; 128];
    let n = crate::press::copy_phase(&mut phase);
    buf.put(&phase[..n]);
    buf.put(b"
---------------
");
    buf.flush();
}

/// A fixed buffer, so the report costs no allocation and no formatting.
struct Buf {
    bytes: [u8; 512],
    len: usize,
}

impl Buf {
    fn new() -> Buf {
        Buf { bytes: [0; 512], len: 0 }
    }

    fn put(&mut self, s: &[u8]) {
        for b in s {
            if self.len < self.bytes.len() {
                self.bytes[self.len] = *b;
                self.len += 1;
            }
        }
    }

    fn hex(&mut self, mut v: usize) {
        self.put(b"0x");
        let mut digits = [0u8; 16];
        let mut n = 0;
        loop {
            digits[n] = b"0123456789abcdef"[v & 0xf];
            v >>= 4;
            n += 1;
            if v == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            let d = digits[n];
            self.put(&[d]);
        }
    }

    fn flush(&self) {
        use std::io::Write;
        // try_lock, never lock: a handler that waits is a handler that hangs
        // the game instead of letting it die.
        if let Ok(mut g) = REPORT.try_lock() {
            if let Some(f) = g.as_mut() {
                let _ = f.write_all(&self.bytes[..self.len]);
                let _ = f.flush();
            }
        }
    }
}
