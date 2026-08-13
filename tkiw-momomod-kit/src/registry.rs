//! The feature registry and the frame scheduler.
//!
//! One frame hook, many features. Everything here exists so that no feature can
//! hurt another: each is probed, configured, timed, and caught separately, and
//! anything that goes wrong takes down exactly one of them.
//!
//! ## Why the loader owns the timing
//!
//! `notes-for-claude/pitfalls.md` records both halves of getting this wrong in the auto-picker,
//! with only two things sharing a hook:
//!
//! * a kill switch written for the *diagnostic* sweep sat at the top of the frame
//!   hook, and silently killed the actual feature too -- the mod went on logging
//!   that it was acting while no code remained to act;
//! * two things were timed together, so the cheap one was blamed for the
//!   expensive one's cost.
//!
//! With a dozen features the only version of this that stays honest is per-feature
//! accounting, which means the loader does the timing and the backing off, and a
//! feature is never handed a budget it has to respect by hand.

use std::time::{Duration, Instant};

use tkiw_runtime::{logln, phase, Runtime};

use crate::feature::{self, Cadence, Feature, Off};

/// A feature and everything the loader knows about it.
pub struct Slot {
    pub feature: Box<dyn Feature>,
    pub state: State,
    /// When it last ran, for `Cadence::Interval`.
    last: Option<Instant>,
    /// Moving average of how long `on_frame` takes, over calls made while the game
    /// was running at a normal pump rate.
    cost: Duration,
    /// Worst single call this session, for the log.
    worst: Duration,
    calls: u64,
    /// Calls that counted towards `cost`, which is what `MIN_CALLS` gates on.
    judged: u64,
    /// Consecutive budget overruns.
    strikes: u32,
    /// Calls over [`PATHOLOGICAL`], counted whatever the game was doing.
    pathological: u32,
}

pub enum State {
    /// Requirements hold, config accepted, `activate` succeeded.
    Running,
    /// Not running, with the reason.
    Off(Off),
}

/// A feature's share of a frame. Deliberately small: this is not a budget for
/// "how long may I take", it is the point at which the loader starts to suspect a
/// feature of being the reason the game stutters.
const BUDGET: Duration = Duration::from_millis(2);
/// A single call this long is not a slow feature, it is a broken one.
const HARD_LIMIT: Duration = Duration::from_millis(50);
/// Overruns in a row before a feature is switched off.
const STRIKES: u32 = 8;

/// Calls a feature must have made before its cost can condemn it.
///
/// The first measurement session switched the only diagnostic off after **eleven
/// calls**, during startup, and the log then went silent for exactly the phase we
/// were trying to measure. Eleven calls is not evidence: a feature's first call
/// populates every cache it has, and during a load the game pumps a dozen times a
/// second rather than sixty, so nearly every call pays the expensive path.
///
/// At sixty pumps a second this is about four seconds of ordinary play.
const MIN_CALLS: u64 = 240;

/// A call is only folded into the cost average if the game reached the previous
/// pump within this. Beyond it the game is loading or blocking on its own account,
/// and a feature's share of a frame that took thirty-nine seconds is not a
/// meaningful number.
const NORMAL_PUMP: Duration = Duration::from_millis(100);

/// A single call this long is broken behaviour, and is judged **whatever the game is
/// doing**.
///
/// This exists because `NORMAL_PUMP` opened a hole that a measurement session drove
/// straight through. Gating all judgement on "was the game pumping normally" is right
/// for a *share-of-frame* verdict -- but a feature slow enough to stop the game
/// **causes** the abnormal pump rate, and so hides behind the very condition meant to
/// be fair to it. In that session a diagnostic averaging 67ms per call, worst 3.1
/// seconds, ran for twenty-five seconds before the loader noticed, and the game never
/// reached its main menu.
///
/// No feature has any business holding the game's thread for a quarter of a second,
/// and deciding that needs no knowledge of what the game was doing. Three of these
/// and it is gone.
const PATHOLOGICAL: Duration = Duration::from_millis(250);
const PATHOLOGICAL_STRIKES: u32 = 3;

pub struct Registry {
    slots: Vec<Slot>,
    /// When `tick` last ran, so a feature can be told apart from a load screen.
    last_tick: Option<Instant>,
}

impl Registry {
    pub fn new(features: Vec<Box<dyn Feature>>) -> Registry {
        Registry {
            last_tick: None,
            slots: features
                .into_iter()
                .map(|f| Slot {
                    feature: f,
                    state: State::Off(Off::Disabled),
                    last: None,
                    cost: Duration::ZERO,
                    worst: Duration::ZERO,
                    calls: 0,
                    judged: 0,
                    strikes: 0,
                    pathological: 0,
                })
                .collect(),
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.slots.iter().map(|s| s.feature.name()).collect()
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// Bring the registry into line with a config: probe, configure, activate.
    ///
    /// Safe to call again on a config reload. A feature that was running and is
    /// now disabled is deactivated; one that was off and is now wanted is probed
    /// afresh, so fixing a config mistake does not need a restart.
    pub fn apply(&mut self, rt: &Runtime, cfg: &crate::config::ConfigSet) {
        for slot in &mut self.slots {
            let name = slot.feature.name();
            let module = slot.feature.module();
            let wanted = cfg.enabled(module, name, slot.feature.default_enabled());

            if !wanted {
                if matches!(slot.state, State::Running) {
                    guarded(name, "deactivate", || slot.feature.deactivate(rt));
                    logln!("[{name}] switched off");
                }
                slot.state = State::Off(Off::Disabled);
                continue;
            }
            if matches!(slot.state, State::Running) {
                continue; // already up; nothing to do
            }
            // Do not keep retrying something that has already failed for a reason
            // a config reload cannot change. Otherwise every reload re-runs a
            // probe that is going to fail, and re-logs it.
            if let State::Off(off) = &slot.state {
                if matches!(off, Off::Unsupported(_) | Off::Faulted(_) | Off::TooSlow(_)) {
                    continue;
                }
            }

            let req = slot.feature.requires();
            if let Err(why) = feature::check(rt, &req) {
                logln!("[{name}] {}", Off::Unsupported(why.clone()).describe());
                slot.state = State::Off(Off::Unsupported(why));
                continue;
            }
            let section = cfg.section(slot.feature.module(), name);
            match guarded(name, "configure", || slot.feature.configure(&section)) {
                Some(Ok(())) => {}
                Some(Err(why)) => {
                    logln!("[{name}] {}", Off::Misconfigured(why.clone()).describe());
                    slot.state = State::Off(Off::Misconfigured(why));
                    continue;
                }
                None => {
                    let why = "panicked while reading its config".to_string();
                    slot.state = State::Off(Off::Faulted(why));
                    continue;
                }
            }
            match guarded(name, "activate", || slot.feature.activate(rt)) {
                Some(Ok(())) => {
                    slot.state = State::Running;
                    slot.last = None;
                    slot.strikes = 0;
                    logln!("[{name}] on");
                }
                Some(Err(why)) => {
                    logln!("[{name}] {}", Off::FailedToStart(why.clone()).describe());
                    slot.state = State::Off(Off::FailedToStart(why));
                }
                None => {
                    slot.state = State::Off(Off::Faulted("panicked in activate".into()));
                }
            }
        }
    }

    /// One pass over the running features, on the game's thread.
    ///
    /// `now` is compared against the previous tick to tell ordinary play from a
    /// load: a feature is not charged for a frame the game itself spent thirty
    /// seconds inside.
    pub fn tick(&mut self, rt: &Runtime, now: Instant) {
        let pumping_normally = self
            .last_tick
            .replace(now)
            .is_some_and(|prev| now.duration_since(prev) < NORMAL_PUMP);

        for slot in &mut self.slots {
            if !matches!(slot.state, State::Running) {
                continue;
            }
            let due = match slot.feature.cadence() {
                Cadence::Never => false,
                Cadence::Once => slot.calls == 0,
                Cadence::EveryFrame => true,
                Cadence::Interval(gap) => {
                    // A feature that has overrun gets a wider interval rather than
                    // being switched off outright: degrade, do not die.
                    let gap = gap * (1 << slot.strikes.min(4));
                    slot.last.map_or(true, |t| now.duration_since(t) >= gap)
                }
            };
            if !due {
                continue;
            }
            slot.last = Some(now);
            slot.calls += 1;

            // Recorded unconditionally, not behind a trace flag: this is the one
            // fact the crash reporter cannot do without, and the session that
            // finally faulted in the auto-picker had tracing off.
            phase::note(slot.feature.name());

            let started = Instant::now();
            let outcome = guarded(slot.feature.name(), "on_frame", || slot.feature.on_frame(rt));
            let took = started.elapsed();

            let name = slot.feature.name();
            match outcome {
                None => {
                    guarded(name, "deactivate", || slot.feature.deactivate(rt));
                    slot.state = State::Off(Off::Faulted("panicked on a frame".into()));
                    continue;
                }
                Some(Err(why)) => {
                    logln!("[{name}] {}", Off::Faulted(why.clone()).describe());
                    guarded(name, "deactivate", || slot.feature.deactivate(rt));
                    slot.state = State::Off(Off::Faulted(why));
                    continue;
                }
                Some(Ok(())) => {}
            }

            slot.worst = slot.worst.max(took);

            // Absolute misbehaviour, judged unconditionally. This check must come
            // before the `pumping_normally` gate, because a feature this slow is why
            // the game is not pumping normally.
            if took >= PATHOLOGICAL {
                slot.pathological += 1;
                logln!(
                    "[{name}] held the game's thread for {:.0}ms - that is never \
                     acceptable ({}/{} before it is switched off)",
                    took.as_secs_f64() * 1000.0,
                    slot.pathological,
                    PATHOLOGICAL_STRIKES
                );
                if slot.pathological >= PATHOLOGICAL_STRIKES {
                    let why = format!(
                        "{} call(s) over {}ms, worst {:.0}ms. This feature can stop the \
                         game; it stays off until you change its settings.",
                        slot.pathological,
                        PATHOLOGICAL.as_millis(),
                        slot.worst.as_secs_f64() * 1000.0
                    );
                    logln!("[{name}] {}", Off::TooSlow(why.clone()).describe());
                    guarded(name, "deactivate", || slot.feature.deactivate(rt));
                    slot.state = State::Off(Off::TooSlow(why));
                    continue;
                }
            }

            // Only judge a feature on frames the game itself was running normally.
            // Its first calls populate its caches, and during a load the game pumps
            // a dozen times a second rather than sixty -- so nearly every call pays
            // the expensive path and the average means nothing.
            if !pumping_normally {
                continue;
            }
            slot.judged += 1;
            // Exponential moving average: one slow frame is not evidence, a
            // sustained cost is.
            slot.cost = if slot.cost.is_zero() {
                took
            } else {
                (slot.cost * 7 + took) / 8
            };
            if slot.judged < MIN_CALLS {
                continue;
            }

            if took > HARD_LIMIT || slot.cost > BUDGET {
                slot.strikes += 1;
                if slot.strikes >= STRIKES {
                    let why = format!(
                        "averaging {:.2}ms per call over {} judged calls (worst {:.1}ms). \
                         Raise its interval_ms, or leave it off.",
                        slot.cost.as_secs_f64() * 1000.0,
                        slot.judged,
                        slot.worst.as_secs_f64() * 1000.0
                    );
                    logln!("[{name}] {}", Off::TooSlow(why.clone()).describe());
                    guarded(name, "deactivate", || slot.feature.deactivate(rt));
                    slot.state = State::Off(Off::TooSlow(why));
                } else if slot.strikes == 1 {
                    logln!(
                        "[{name}] slow: {:.1}ms this call, averaging {:.2}ms - backing off",
                        took.as_secs_f64() * 1000.0,
                        slot.cost.as_secs_f64() * 1000.0
                    );
                }
            } else {
                slot.strikes = 0;
            }
        }
    }

    /// What every feature is doing, for the log at startup and on request.
    pub fn report(&self) -> Vec<String> {
        self.slots
            .iter()
            .map(|s| match &s.state {
                State::Running if s.calls > 0 => format!(
                    "  {:24} on   ({} calls, {} judged, avg {:.2}ms, worst {:.2}ms)",
                    s.feature.name(),
                    s.calls,
                    s.judged,
                    s.cost.as_secs_f64() * 1000.0,
                    s.worst.as_secs_f64() * 1000.0
                ),
                State::Running => format!("  {:24} on", s.feature.name()),
                State::Off(off) => format!("  {:24} off  {}", s.feature.name(), off.describe()),
            })
            .collect()
    }

    /// Deactivate everything. For process detach.
    pub fn shut_down(&mut self, rt: &Runtime) {
        for slot in &mut self.slots {
            if matches!(slot.state, State::Running) {
                guarded(slot.feature.name(), "deactivate", || slot.feature.deactivate(rt));
                slot.state = State::Off(Off::Disabled);
            }
        }
    }
}

/// Call into a feature with its panics caught. `None` means it panicked.
///
/// `AssertUnwindSafe` is the honest description of the situation: a feature that
/// panics halfway through may well have left its own state inconsistent. That is
/// why a panic disables it rather than being retried -- the loader does not
/// pretend the feature is still fit to run.
fn guarded<R>(name: &str, what: &str, f: impl FnOnce() -> R) -> Option<R> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Some(r),
        Err(_) => {
            logln!("[{name}] PANIC in {what} - this feature is off for the session.");
            None
        }
    }
}
