//! Where a launch actually spends its time.
//!
//! Not a change to the game: a diagnostic, and the first thing built, because the
//! three things worth optimising -- boot, the menu-to-run load, and the in-run lag
//! with many units -- are three different problems and guessing which code is
//! responsible is how a week disappears.
//!
//! Two measurements, both from pure reads:
//!
//! **A phase timeline.** Marker objects are watched for appearing and
//! disappearing, which is the cheapest honest answer to "what is the game doing
//! now". `obj_splash_screen` exists during the splashes, `obj_main_menu` on the
//! menu, `obj_gameplay_controller` in a run -- so the log gets a line per
//! transition with the wall-clock time since the process started, and the shape of
//! a launch falls out of it:
//!
//! ```text
//! [      4.812] timeline: obj_splash_screen appeared   (+4.812 since launch)
//! [     11.004] timeline: obj_splash_screen gone       (6.192s in splash)
//! [     11.310] timeline: obj_main_menu appeared       (+0.306)
//! ```
//!
//! **A hitch record.** The frame hook is called from the game's message pump, so
//! the gap between consecutive calls is the frame time as the player experiences
//! it. The worst gap per second is logged when it exceeds a threshold, which is
//! what "it gets laggy with a lot of units" looks like in numbers.
//!
//! ## What this deliberately does not do
//!
//! It does not attribute cost to code. A phase timeline says *when*, never *why*.
//! Reading a room name would need a call into the runtime and tells us less than
//! the objects do, and attributing time to functions needs a sampling profiler,
//! which is a separate feature with a much heavier risk profile.
//!
//! ## On the pump rate
//!
//! The hook is called tens of thousands of times per second while the game loads
//! and about sixty times per second in play. So a "frame" here is a pump, not a
//! rendered frame, and everything is paced by wall-clock time rather than by
//! counting calls. During a load the gaps go to near zero and the hitch detector
//! correctly reports nothing; the phase timeline is what covers loads.

use std::time::{Duration, Instant};

use tkiw_runtime::{instance, logln, Runtime};

use crate::config::Section;
use crate::feature::{Cadence, Feature, Requirements};

/// The objects whose presence marks a phase, in the order they appear in a launch.
///
/// Chosen for being singletons that exist for exactly one phase. Deliberately not
/// checked as requirements: outside the phase they mark they genuinely do not
/// exist, so absence at startup proves nothing.
/// `obj_run_controller` is deliberately absent: it appears with `obj_init` and never
/// goes away, so as a phase marker it says nothing. Measured, not assumed.
const MARKERS: &[(&str, &str)] = &[
    ("obj_init", "the init room: libraries, localisation, fonts"),
    ("obj_splash_screen", "the splash screens"),
    ("obj_main_menu", "the main menu"),
    ("obj_gameplay_controller", "in a run"),
];

/// A gap between pumps longer than this is a hitch a player can feel. 50ms is
/// three missed frames at 60Hz.
const HITCH: Duration = Duration::from_millis(50);

/// Above this, a gap is not a hitch: it is the game blocking on a load, or the
/// window being backgrounded while the player is elsewhere.
///
/// Worth separating because otherwise one alt-tab dominates every statistic in the
/// report. A 210-second "worst hitch" in the first measurement session was the
/// player being away from the keyboard, and it made the real figures -- a median
/// stall of 63ms -- impossible to see.
const PAUSE: Duration = Duration::from_secs(2);

/// How often to summarise stalls. Per-interval reporting produced 1,061 log lines
/// in one session, which is a lot of scrolling for a number that only means
/// anything in aggregate.
const SUMMARY_EVERY: Duration = Duration::from_secs(5);

pub struct Timeline {
    interval: Duration,
    /// Whether each marker was present last time we looked.
    present: [bool; MARKERS.len()],
    /// When each marker last appeared, so we can report how long the phase lasted.
    since: [Option<Instant>; MARKERS.len()],
    /// Wall clock at the first pump, which is as close as we can get to "the game
    /// started running" from inside it.
    first_pump: Option<Instant>,
    last_pump: Option<Instant>,
    /// When the markers were last checked.
    window_started: Option<Instant>,
    /// Stall accounting since the last summary.
    stalls: Stalls,
    summary_started: Option<Instant>,
    started: Option<Instant>,
}

/// Gaps between pumps, split by magnitude so one alt-tab cannot swamp the numbers.
#[derive(Default)]
struct Stalls {
    /// Gaps in `HITCH..PAUSE`: stutter the player feels as the game running badly.
    count: u32,
    worst: Duration,
    /// How much time went into them, which is the figure that actually matters --
    /// forty 60ms stalls a second is unplayable, and one is not.
    total: Duration,
    /// Gaps over `PAUSE`: a blocking load, or the window in the background.
    pauses: u32,
    longest_pause: Duration,
}

impl Stalls {
    fn note(&mut self, gap: Duration) {
        if gap >= PAUSE {
            self.pauses += 1;
            self.longest_pause = self.longest_pause.max(gap);
        } else if gap >= HITCH {
            self.count += 1;
            self.total += gap;
            self.worst = self.worst.max(gap);
        }
    }

    fn anything(&self) -> bool {
        self.count > 0 || self.pauses > 0
    }
}

impl Default for Timeline {
    fn default() -> Timeline {
        Timeline {
            interval: Duration::from_millis(500),
            present: [false; MARKERS.len()],
            since: [None; MARKERS.len()],
            first_pump: None,
            last_pump: None,
            window_started: None,
            stalls: Stalls::default(),
            summary_started: None,
            started: None,
        }
    }
}

impl Feature for Timeline {
    fn name(&self) -> &'static str {
        "timeline"
    }

    fn module(&self) -> &'static str {
        "diagnostics"
    }

    fn summary(&self) -> &'static str {
        "Periodically check which phase the game is in. Logs how long each one lasts."
    }

    fn requires(&self) -> Requirements {
        Requirements {
            // Nothing by name: this reads the object registry, which the shared
            // runtime has already checked. The markers are recorded rather than
            // required, since a marker that has been renamed should cost this
            // feature one line of output, not the whole feature.
            objects: &[
                "obj_init",
                "obj_splash_screen",
                "obj_main_menu",
                "obj_gameplay_controller",
            ],
            ..Requirements::default()
        }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        // 500ms, not 100ms. A marker check is five object-registry lookups, each
        // validating instance pointers, and it was measured at 2.1ms per call --
        // enough to look like a feature costing frames. A phase boundary does not
        // need 100ms resolution: the phases last seconds.
        let ms = section.u64("interval_ms", 500)?;
        if !(50..=5_000).contains(&ms) {
            return Err(format!("interval_ms: {ms} is outside 50..5000"));
        }
        self.interval = Duration::from_millis(ms);
        for k in section.unknown(&["enabled", "interval_ms"]) {
            logln!("[timeline] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn activate(&mut self, _rt: &Runtime) -> Result<(), String> {
        self.started = Some(Instant::now());
        logln!(
            "[timeline] watching {} markers every {}ms; stalls over {}ms are reported, \
             gaps over {}s are counted separately as pauses",
            MARKERS.len(),
            self.interval.as_millis(),
            HITCH.as_millis(),
            PAUSE.as_secs()
        );
        Ok(())
    }

    /// Every pump, because the whole point is to measure the gaps between them.
    /// The work done here has to stay trivial, or the measurement becomes the
    /// thing being measured.
    fn cadence(&self) -> Cadence {
        Cadence::EveryFrame
    }

    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        let now = Instant::now();
        self.pump(now);
        // Reading the object registry is far too expensive to do every pump; the
        // markers change on the scale of seconds.
        if self.window_started.is_none_or(|t| now.duration_since(t) >= self.interval) {
            self.window_started = Some(now);
            self.summarise(now);
            self.markers(rt, now);
        }
        Ok(())
    }
}

impl Timeline {
    /// Frame-gap accounting. Called every pump, so it does nothing but arithmetic.
    fn pump(&mut self, now: Instant) {
        let first = *self.first_pump.get_or_insert(now);
        if let Some(last) = self.last_pump {
            self.stalls.note(now.duration_since(last));
        } else {
            logln!(
                "[timeline] first pump at {:.3}s - the game's message loop is running",
                first.duration_since(*self.started.as_ref().unwrap_or(&first)).as_secs_f64()
            );
        }
        self.last_pump = Some(now);
    }

    /// Summarise stalls, at most every [`SUMMARY_EVERY`].
    ///
    /// Reports the share of wall-clock time lost to stutter, because that is the
    /// number that corresponds to what a player feels. A count on its own does not:
    /// forty 60ms stalls a second is unplayable and one is imperceptible.
    fn summarise(&mut self, now: Instant) {
        let started = *self.summary_started.get_or_insert(now);
        let over = now.duration_since(started);
        if over < SUMMARY_EVERY {
            return;
        }
        self.summary_started = Some(now);
        let s = std::mem::take(&mut self.stalls);
        if !s.anything() {
            return;
        }
        let elapsed = self.started.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        if s.count > 0 {
            logln!(
                "[timeline] {elapsed:8.3}s  {} stalls over {}ms in {:.0}s: {:.1}% of the time \
                 lost, worst {:.0}ms",
                s.count,
                HITCH.as_millis(),
                over.as_secs_f64(),
                s.total.as_secs_f64() * 100.0 / over.as_secs_f64().max(0.001),
                s.worst.as_secs_f64() * 1000.0
            );
        }
        if s.pauses > 0 {
            logln!(
                "[timeline] {elapsed:8.3}s  {} pause(s) over {}s, longest {:.1}s \
                 (a blocking load, or the window in the background - not stutter)",
                s.pauses,
                PAUSE.as_secs(),
                s.longest_pause.as_secs_f64()
            );
        }
    }

    /// Look for marker transitions.
    fn markers(&mut self, rt: &Runtime, now: Instant) {
        for (i, (obj, what)) in MARKERS.iter().enumerate() {
            // A pure registry read: no call into the game, and a miss is `None`
            // rather than a fault. The registry itself is cached by the runtime,
            // so this is a hash lookup and a short list walk per marker.
            let here = instance::find_singleton(rt.base, obj).is_some();
            if here == self.present[i] {
                continue;
            }
            self.present[i] = here;
            let elapsed = self
                .started
                .map(|s| now.duration_since(s).as_secs_f64())
                .unwrap_or_default();
            if here {
                self.since[i] = Some(now);
                logln!("[timeline] {elapsed:8.3}s  + {obj}  ({what})");
            } else {
                let lasted = self.since[i].map(|s| now.duration_since(s).as_secs_f64());
                match lasted {
                    Some(d) => logln!("[timeline] {elapsed:8.3}s  - {obj}  (lasted {d:.3}s)"),
                    None => logln!("[timeline] {elapsed:8.3}s  - {obj}"),
                }
                self.since[i] = None;
            }
        }
    }
}
