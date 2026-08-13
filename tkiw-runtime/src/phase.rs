//! What was under way when the process died.
//!
//! The log records what *succeeded*, so after an access violation its last line
//! is the last thing that finished — which has twice now been a successful
//! action with the real fault somewhere after it. This narrows the question to
//! one line.
//!
//! Two properties earned the hard way, both recorded in `notes-for-claude/pitfalls.md`:
//!
//! * **Recorded unconditionally, not behind a verbose mode.** The one fact the
//!   crash reporter cannot do without was originally written only when tracing
//!   was on — and the session that finally faulted had tracing off, on the
//!   author's own advice, so the phase was lost. A flood of trace lines is
//!   rightly opt-in; this costs a memcpy and is not.
//! * **Read without allocating and without ever blocking.** The reader runs
//!   inside the exception handler, where the heap may be the reason we are there
//!   and where waiting on a lock turns a crash into a hang.

/// The phase currently under way, in a fixed buffer.
static PHASE: std::sync::Mutex<([u8; 128], usize)> = std::sync::Mutex::new(([0; 128], 0));

/// Note what is starting now. Cheap enough to call on every step of a frame.
///
/// `try_lock`: losing a phase update is better than blocking a frame, and the
/// only contender is the crash handler, which by then has more pressing news.
pub fn note(what: &str) {
    if let Ok(mut g) = PHASE.try_lock() {
        let src = what.as_bytes();
        let n = src.len().min(g.0.len());
        g.0[..n].copy_from_slice(&src[..n]);
        g.1 = n;
    }
}

/// Copy the last phase noted into `dst`, returning how many bytes were written.
///
/// Deliberately not `-> String`: this is called from the crash handler, where
/// allocating is the thing to avoid.
pub fn copy(dst: &mut [u8]) -> usize {
    match PHASE.try_lock() {
        Ok(g) => {
            let n = g.1.min(dst.len());
            dst[..n].copy_from_slice(&g.0[..n]);
            n
        }
        Err(_) => 0,
    }
}
