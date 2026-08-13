//! The log.
//!
//! A mod that acts invisibly leaves the log as the only record of what it did on
//! the player's behalf. That makes it load-bearing rather than a debugging aid,
//! and it is not optional.
//!
//! The file's name comes from [`crate::identity`], so each host mod logs to its
//! own file even when two are loaded into the same process.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

use crate::home;
use crate::identity;

const ROTATE_AT: u64 = 4 * 1024 * 1024;

static LOCK: Mutex<()> = Mutex::new(());

pub fn write(line: &str) {
    let Some(path) = home::file(identity::get().log_file) else {
        return; // no home: nothing to write to, and orphan_note has said so
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut rotated = false;
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > ROTATE_AT {
            let _ = std::fs::rename(&path, path.with_extension("log.prev"));
            rotated = true;
        }
    }
    if rotated {
        HANDLE.lock().map(|mut h| *h = None).ok();
        carry_findings(&path);
    }

    let mut h = match HANDLE.lock() {
        Ok(h) => h,
        Err(e) => e.into_inner(),
    };
    // Reopen if the file has been moved or deleted underneath us -- taking the
    // log aside mid-session is a thing worth supporting -- but only check for
    // that occasionally, since the whole point here is to stop paying for a
    // file open on every line.
    if h.as_ref().is_some_and(|(_, at): &(std::fs::File, std::time::Instant)| {
        at.elapsed() >= std::time::Duration::from_secs(2) && !path.exists()
    }) {
        *h = None;
    }
    if h.is_none() {
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(fh) => *h = Some((fh, std::time::Instant::now())),
            Err(_) => return,
        }
    }
    if let Some((fh, at)) = h.as_mut() {
        if at.elapsed() >= std::time::Duration::from_secs(2) {
            *at = std::time::Instant::now();
        }
        if writeln!(fh, "{}{}", stamp(), line).is_err() {
            *h = None;
        }
    }
}

/// The log file, held open.
///
/// Opening and closing it for every line is a system call per line, and the mod
/// writes a great many of them while a queue is draining -- more still with
/// `trace` on, where several lines land in a single frame. The handle is
/// dropped on rotation, and reopened if the file goes missing.
static HANDLE: Mutex<Option<(std::fs::File, std::time::Instant)>> = Mutex::new(None);

/// The one-shot conclusions this session has reached.
///
/// Every diagnostic worth having is reported *once*, and every one of them
/// happens in the first minute -- while the thing they explain happens minutes
/// later. A rotation in between would leave a log holding the crash and none of
/// the findings that make sense of it, which is how a whole session gets
/// wasted. So they are kept and rewritten at the top of each new file.
static FINDINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Log a line and remember it across log rotations.
pub fn finding(line: &str) {
    if let Ok(mut g) = FINDINGS.lock() {
        // Bounded: this is for conclusions, not for a running commentary.
        if g.len() < 64 {
            g.push(line.to_string());
        }
    }
    write(line);
}

/// Called with the log lock held, immediately after a rotation.
fn carry_findings(path: &std::path::Path) {
    let Ok(g) = FINDINGS.lock() else { return };
    if g.is_empty() {
        return;
    }
    if let Ok(mut fh) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(fh, "{}---- carried over from earlier in this session ----", stamp());
        for line in g.iter() {
            let _ = writeln!(fh, "{}  {line}", stamp());
        }
        let _ = writeln!(fh, "{}---- end of carry-over ----", stamp());
    }
}

/// Write without taking the lock.
///
/// For the crash reporter only. A fault can be raised *inside* `write`, while
/// this thread already holds `LOCK` -- and a handler that then waits for it
/// would hang the game instead of letting it die, which is strictly worse for
/// the player. Interleaved bytes are an acceptable price; a deadlock is not.
pub fn write_unlocked(line: &str) {
    let Some(path) = home::file(identity::get().log_file) else { return };
    if let Ok(mut fh) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(fh, "{}{}", stamp(), line);
    }
}

/// Seconds since the process started, which is all the ordering a session log
/// needs and avoids dragging in a date formatter for a no-dependency build.
fn stamp() -> String {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let t = START.get_or_init(Instant::now).elapsed();
    format!("[{:7}.{:03}] ", t.as_secs(), t.subsec_millis())
}

#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => { $crate::log::write(&format!($($arg)*)) };
}

/// A conclusion worth keeping: logged now, and repeated after any log rotation
/// so it always sits in the same file as whatever it explains.
#[macro_export]
macro_rules! findln {
    ($($arg:tt)*) => { $crate::log::finding(&format!($($arg)*)) };
}

/// `logln!` for the crash reporter: never blocks, never waits on the log lock.
#[macro_export]
macro_rules! faultln {
    ($($arg:tt)*) => { $crate::log::write_unlocked(&format!($($arg)*)) };
}
