//! Never breaking a player's game twice for the same reason.
//!
//! An access violation takes the process down with no unwinding, no
//! `catch_unwind`, and no chance to write anything. So a startup probe that
//! kills the game at launch would kill it again on the next launch, and the next,
//! and the player's only recourse is to work out which of their files to delete.
//!
//! A file is written before the risky part and removed once the session is
//! clearly healthy. Finding one at startup means the last session died, so this
//! session stays passive and the game launches normally.
//!
//! Three refinements, all of which came from getting them wrong first:
//!
//! * **Stand down once the session is healthy**, not at process exit. The
//!   breadcrumb exists to catch a *probe* that kills the game at launch; clearing
//!   it only on exit means an unrelated crash three minutes into play costs the
//!   player the whole next session, and looks identical to the mod simply being
//!   broken.
//! * **Give the player a way back in.** A held session must still respond to the
//!   mod's hotkey. Polling the keyboard touches no game memory, so a session held
//!   back for safety stays exactly as safe until the player decides otherwise.
//! * **Rewrite the breadcrumb when recovering, do not clear it.** If the probe is
//!   what kills the game, the launch after the recovery attempt must still be
//!   protected.

use std::path::PathBuf;

use crate::{home, logln, win};

const FILE: &str = "probe.incomplete";

/// How long a session must run before the breadcrumb is cleared.
pub const HEALTHY_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

pub enum Armed {
    /// Clear to probe. Carries the breadcrumb, to be cleared by
    /// [`clear_when_healthy`] once the session has proven itself.
    Go(PathBuf),
    /// A breadcrumb from a session that died. Recoverable on request via
    /// [`wait_for_recovery`].
    Held(PathBuf),
    /// Nothing to run and nothing to offer: no mod folder, or the breadcrumb
    /// could not be written, so there would be no protection next time either.
    Skip,
}

/// Look for a breadcrumb, and leave one if there is none.
///
/// `hotkey` is described in the log line that tells a held player how to get
/// back in, so it must match what the host actually polls.
pub fn arm(hotkey: &str) -> Armed {
    let Some(marker) = home::file(FILE) else {
        return Armed::Skip;
    };
    if marker.exists() {
        logln!(
            "the last session did not end cleanly - not probing this run, so the game \
             launches normally."
        );
        logln!("the log above ends at whatever killed it.");
        logln!("press {hotkey} in game to probe anyway and switch the mod on.");
        return Armed::Held(marker);
    }
    if std::fs::write(&marker, b"probe in progress\n").is_err() {
        logln!("could not write the crash-loop breadcrumb; skipping the probe to be safe.");
        return Armed::Skip;
    }
    Armed::Go(marker)
}

/// Sit out the session unless the player asks for the mod back.
///
/// Nothing here touches the game: it polls the keyboard and nothing else.
/// Returns `true` if the player asked and the breadcrumb was successfully
/// rewritten, meaning the caller should now probe.
///
/// `keys` is the chord to wait for, as virtual-key codes.
pub fn wait_for_recovery(marker: &PathBuf, keys: &[i32]) -> bool {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(120));
        if !keys.iter().all(|&vk| win::key_down(vk)) {
            continue;
        }
        logln!("hotkey: probing after all, at your request.");
        if std::fs::write(marker, b"probe in progress (recovery)\n").is_err() {
            logln!("could not write the breadcrumb; not probing, to stay safe.");
            return false;
        }
        return true;
    }
}

/// Clear the breadcrumb once the session has run long enough to be trusted.
///
/// Call from the frame hook; it does nothing until [`HEALTHY_AFTER`] has passed
/// and nothing at all after the first time it succeeds.
pub fn clear_when_healthy(started: &std::time::Instant) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) || started.elapsed() < HEALTHY_AFTER {
        return;
    }
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if clear() {
        logln!(
            "this session has run {}s without trouble - crash-loop guard stood down.",
            HEALTHY_AFTER.as_secs()
        );
    }
}

/// Clear the breadcrumb now, because there is no longer any risky code running.
///
/// Call this when the mod **stands itself down cleanly** -- an unresolvable symbol,
/// a guard mismatch, a hook that would not install. In that case nothing dangerous
/// is left in the process, so holding the next launch back protects nothing and
/// costs the player a session with a message that says their game crashed when it
/// did not.
///
/// This distinction is the whole point of the breadcrumb: it exists to catch a
/// probe that **kills the process**, not one that returns an error. Leaving it for
/// the latter turns every clean failure into two.
pub fn clear() -> bool {
    match home::file(FILE) {
        Some(marker) => marker.exists() && std::fs::remove_file(&marker).is_ok(),
        None => false,
    }
}
