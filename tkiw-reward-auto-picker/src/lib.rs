//! Config-driven auto reward picker for "The King is Watching".
//!
//! Loaded into the game as a proxy `version.dll` (see `proxy`). Everything the
//! mod owns lives in its own folder (see `home`); the game folder gets exactly
//! one added file.
//!
//! The governing rule, from the spec: the mod may only cause effects that some
//! sequence of legal player actions, available at that same moment, would have
//! caused. Anything it cannot do confidently, it declines to do.

#[cfg(feature = "standalone")]
use core::ffi::c_void;

// ---- the shared layer, from `tkiw-runtime` --------------------------------------
//
// These were fourteen files in this crate, 2,273 lines, byte-identical to their
// counterparts in the shared runtime. They are re-exported here rather than
// re-declared so that every `crate::win::` and `crate::instance::` path in the rest
// of this mod keeps working untouched -- the migration is a change to this block and
// nothing else.
//
// The old copies are still on disk in `src/` and are no longer compiled. They should
// be deleted; they are kept only so this change is trivially reversible.
pub use tkiw_runtime::{
    builtin, dslist, fault, globals, gml, guard, home, hook, instance, log, patch, pe,
    phase, rvalue, saves, win,
};
// `#[macro_export]` puts these at the defining crate's root, so they need re-exporting
// too for `crate::logln!` to keep resolving.
pub use tkiw_runtime::{faultln, findln, logln};

// ---- this mod's own code -------------------------------------------------------
pub mod config;
pub mod picker;
pub mod press;
#[cfg(feature = "standalone")]
pub mod proxy;
pub mod resolve;
pub mod survey;
pub mod vocab;

use std::sync::OnceLock;

// ---- identity ------------------------------------------------------------------
//
// The shared runtime needs to know which mod it is inside: whose stamp buffer to read,
// which log to write. The buffer itself must live in *this* crate, because it has to
// end up in this cdylib and be findable there by `install.py`'s byte search -- see
// `tkiw_runtime::identity` for why an rlib is the wrong place for it.
//
// The marker is unchanged from before the migration, so existing installations and the
// existing `install.py` keep working.
const MARKER: &[u8] = b"TKIW_PICKER_MOD_DIR=";

#[used]
#[cfg(feature = "standalone")]
#[no_mangle]
pub static TKIW_PICKER_MOD_DIR_STAMP: [u8; home::STAMP_LEN] = {
    let mut a = [0u8; home::STAMP_LEN];
    let mut i = 0;
    while i < MARKER.len() {
        a[i] = MARKER[i];
        i += 1;
    }
    a
};

#[cfg(feature = "standalone")]
fn identify() {
    tkiw_runtime::identity::set(tkiw_runtime::Identity {
        name: "tkiw-reward-picker",
        marker: MARKER,
        stamp: core::ptr::addr_of!(TKIW_PICKER_MOD_DIR_STAMP) as *const u8,
        stamp_len: home::STAMP_LEN,
        log_file: "picker.log",
        orphan_note: "tkiw_reward_picker_error.log",
    });
}

/// What the frame hook needs, published before the hook is armed.
pub struct State {
    pub syms: gml::Symbols,
    pub text: (usize, usize),
}

static STATE: OnceLock<State> = OnceLock::new();

/// Globals whose value is already known from static analysis, so reading them
/// back from the live game either proves the read path or exposes it.
const KNOWN_GLOBALS: &[(&str, &str)] = &[
    ("REWARD_UNIT_CLASS_STAT", "unit_class_stat"),
    ("REWARD_RESOURCE", "resource"),
    ("REWARD_ARTIFACT", "artifact"),
    ("REWARD_SPELL", "spell"),
];

#[cfg(feature = "standalone")]
use win::{Handle, Hmodule};

/// Symbols the mod cannot work without. Probed at startup purely so a game
/// update that renames one shows up as a named failure in the log rather than
/// as mysterious inaction later.
const REQUIRED_FUNCS: &[&str] = &[
    "gml_Script_spawn_rewards_choice@gml_Object_obj_run_controller_Create_0",
    "gml_Script_spawn_choice_unified@gml_Object_obj_run_controller_Create_0",
    "gml_Script_spawn_stat_upgrade_choice@gml_Object_obj_run_controller_Create_0",
    "gml_Object_obj_reward_option_Create_0",
    "gml_Object_obj_button_reroll_cards_Create_0",
    "gml_Script_setup_reroll_button@gml_Object_obj_button_reroll_cards_Create_0",
];

const REQUIRED_VARS: &[&str] = &[
    "pending_rewards",
    "reward",
    "reward_type",
    "run_rerolls_left",
    "FREE_REROLLS_PER_RUN_LEFT",
    "FREE_REROLLS_PER_REWARD_LIMIT",
    "free_rerolls_per_reward_left",
    "non_free_rerolls_made",
    "resolve_reroll_cost",
];

/// Startup work, off the loader lock.
///
/// `DllMain` runs with the loader lock held, where almost anything interesting
/// -- loading a library, touching another module, blocking -- can deadlock. So
/// `DllMain` does nothing but hand off to this thread.
#[cfg(feature = "standalone")]
extern "system" fn init(_: *mut c_void) -> u32 {
    // Before anything reads the stamp, the log, or the crash-report path.
    identify();

    // Claim first, and before any waiting: a host that has absorbed this crate polls
    // for this claim and stands down when it sees it, so the earlier it exists the
    // smaller the window in which both could think they are the one acting.
    if let Ok(mut g) = STANDALONE.lock() {
        *g = tkiw_runtime::claim::Claim::take(STANDALONE_CLAIM);
    }

    if home::dir().is_none() {
        // Nowhere to log to. Leave a note where it can still be found, and
        // stay disabled: no home means uninstalled or moved, and guessing
        // would be worse than doing nothing.
        home::orphan_note();
        return 0;
    }

    // Before anything else that touches the game: if this session ends in a
    // fault, the log should say where.
    fault::watch();

    logln!("---- tkiw-reward-picker starting ----");
    // Which game build the baked addresses were taken from. If someone reports
    // the guard refusing to load, this is the line that says why.
    logln!("built for  : the game as of {}", guard::TARGET_BUILD);
    logln!("mod folder : {}", home::dir().unwrap().display());
    logln!("exe base   : {:#x}", win::exe_base());
    logln!("pid        : {}", unsafe { win::GetCurrentProcessId() });

    // Snapshot the saves before anything else, and unconditionally -- including
    // on runs where the probe is skipped. The save is the only thing here that
    // cannot be rebuilt from source.
    match std::panic::catch_unwind(saves::snapshot) {
        Ok(Ok((dst, n))) => logln!("saves      : snapshot of {n} files -> {}", dst.display()),
        Ok(Err(e)) => logln!("WARNING: no save snapshot taken ({e})"),
        Err(_) => logln!("WARNING: no save snapshot taken (panic)"),
    }

    // A panic must disable the mod, never propagate into the game. But a
    // panic is the polite failure: an access violation takes the process down
    // with no chance to catch anything, so the probe is also armed with a
    // breadcrumb that survives the process dying.
    match arm() {
        // The probe was skipped because the last session died. That protection
        // is worth keeping, but it must not be a dead end: the player can ask
        // for it anyway, with the same key that controls everything else.
        Armed::Held(marker) => wait_for_recovery(marker),
        Armed::Skip => {}
        Armed::Go(_marker) => {
            let result = std::panic::catch_unwind(probe);
            if result.is_err() {
                logln!("PANIC during startup probe - the mod is disabled for this session.");
            }
            // The breadcrumb is deliberately *not* cleared here. The frame hook
            // outlives this function and runs for the whole session, so the
            // "did this run survive?" question is only answered at process
            // exit -- see DllMain's DLL_PROCESS_DETACH.
        }
    }
    0
}

#[cfg(feature = "standalone")]
enum Armed {
    Go(std::path::PathBuf),
    /// A breadcrumb from a session that died. Recoverable on request.
    Held(std::path::PathBuf),
    /// Nothing to run and nothing to offer -- no home, or the breadcrumb could
    /// not be written, so there would be no protection next time either.
    Skip,
}

#[cfg(feature = "standalone")]
/// Crash-loop protection.
///
/// A breadcrumb file is written before the probe and removed after it. Finding
/// one at startup means the last run died partway through, so this run does not
/// try again -- the game launches normally and the log says where it stopped.
/// The mod never breaks a player's game twice for the same reason.
fn arm() -> Armed {
    let Some(marker) = home::file("probe.incomplete") else {
        return Armed::Skip;
    };
    if marker.exists() {
        logln!(
            "the last session did not end cleanly - not probing this run, so the game \
             launches normally."
        );
        logln!("the log above ends at whatever killed it.");
        logln!("press Ctrl+Alt+P in game to probe anyway and switch the mod on.");
        return Armed::Held(marker);
    }
    if std::fs::write(&marker, b"probe in progress\n").is_err() {
        logln!("could not write the crash-loop breadcrumb; skipping the probe to be safe.");
        return Armed::Skip;
    }
    Armed::Go(marker)
}

#[cfg(feature = "standalone")]
/// Sit out the session unless the player asks for the mod back.
///
/// Nothing here touches the game: it polls the keyboard and nothing else, so a
/// session skipped for safety stays exactly as safe as it was until the player
/// decides otherwise. Pressing the chord probes and switches pressing on -- one
/// key for "yes, I mean it", the same one that controls everything else.
fn wait_for_recovery(marker: std::path::PathBuf) {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(120));
        if !(win::key_down(win::VK_CONTROL)
            && win::key_down(win::VK_MENU)
            && win::key_down(win::vk_letter(b'P')))
        {
            continue;
        }
        logln!("toggle: probing after all, at your request.");
        // Rewrite the breadcrumb rather than clear it: if the probe is what
        // kills the game, the next launch must still be protected.
        if std::fs::write(&marker, b"probe in progress (recovery)
").is_err() {
            logln!("could not write the breadcrumb; not probing, to stay safe.");
            return;
        }
        if std::panic::catch_unwind(probe).is_err() {
            logln!("PANIC during the probe - the mod is disabled for this session.");
            return;
        }
        ACTING.store(true, std::sync::atomic::Ordering::Relaxed);
        logln!("toggle: ACTING - the mod will now press buttons");
        return;
    }
}

/// Startup self-check: rebuild the game's symbol tables and report what
/// resolved. This is the spike's instrumentation; it does not touch the game.
fn probe() {
    let t0 = std::time::Instant::now();

    let Some(exe) = win::exe_path() else {
        logln!("ERROR: could not determine the executable path; disabled.");
        return;
    };
    logln!("exe path   : {exe}");

    let Some(image) = pe::Image::load(&exe) else {
        logln!("ERROR: could not parse the executable; disabled.");
        return;
    };
    let read_ms = t0.elapsed().as_millis();

    let syms = gml::Symbols::discover(&image, win::exe_base());
    logln!(
        "symbols    : {} functions, {} variable slots  (read {read_ms}ms, total {}ms)",
        syms.functions.len(),
        syms.var_slots.len(),
        t0.elapsed().as_millis()
    );
    logln!("image base : {:#x}", image.image_base);
    logln!("aslr slide : {:#x}", syms.slide);

    let mut missing = Vec::new();
    for name in REQUIRED_FUNCS {
        match syms.func(name) {
            Some(a) => logln!("  fn  {name} -> {a:#x}"),
            None => {
                missing.push(*name);
                logln!("  fn  {name} -> MISSING");
            }
        }
    }
    for name in REQUIRED_VARS {
        match syms.slot(name) {
            Some(a) => logln!("  var {name} -> slot {a:#x}"),
            None => {
                missing.push(*name);
                logln!("  var {name} -> MISSING");
            }
        }
    }

    if !missing.is_empty() {
        logln!(
            "ERROR: {} required symbol(s) missing; this is not the build the mod \
             was written for. Disabled.",
            missing.len()
        );
        return;
    }
    logln!("all required symbols resolved.");

    // Names survive a game update; the baked addresses do not. Refuse the build
    // rather than call into whatever now lives at them -- this mod is shared,
    // and a silent mis-fire on someone else's machine is the worst outcome
    // available.
    // `guard::CORE` is the shared runtime's list of the addresses the whole layer is
    // built on -- the same nine this mod used to carry itself, now in one place so a
    // game update is re-derived once rather than per mod.
    let bad = guard::verify(win::exe_base(), guard::CORE);
    if !bad.is_empty() {
        logln!("ERROR: this is not the game build the mod was built for.");
        for b in &bad {
            logln!("  {b}");
        }
        logln!("The mod is disabled. It needs rebuilding against this game version;");
        logln!("see analysis/ in the mod folder. Your game is unaffected.");
        return;
    }
    logln!("build guard: all {} baked addresses verified.", guard::CORE.len());

    // Variable ids are patched in during the game's own startup, so they are
    // still 0xFFFFFFFF this early. Look again once the game is up, to confirm
    // the live side of the mechanism works at all.
    //
    // Every read is announced before it happens: if one of these ever does take
    // the process down, the log names the exact address that did it instead of
    // just stopping.
    // Variable ids are patched in during the game's own startup, so they are
    // still 0xFFFFFFFF this early. Wait for the game to come up before reading.
    let mut resolved = 0;
    for round in 1..=6 {
        std::thread::sleep(std::time::Duration::from_secs(5));
        resolved = REQUIRED_VARS.iter().filter(|n| syms.var_id(n).is_some()).count();
        if resolved == REQUIRED_VARS.len() {
            logln!("variables resolved after ~{}s:", round * 5);
            for name in REQUIRED_VARS {
                logln!("  {name} = id {}", syms.var_id(name).unwrap());
            }
            break;
        }
    }
    if resolved != REQUIRED_VARS.len() {
        logln!(
            "only {resolved}/{} variables resolved; not installing the frame hook.",
            REQUIRED_VARS.len()
        );
        return;
    }

    // Publish what the hook will need before arming it.
    let text = match image.section(".text") {
        Some(s) => (
            win::exe_base() + s.va as usize,
            win::exe_base() + (s.va + s.vsize) as usize,
        ),
        None => {
            logln!("ERROR: no .text section; not installing the frame hook.");
            return;
        }
    };
    let _ = STATE.set(State { syms, text });

    // Everything from here has to run on the game's own thread.
    if hosted() {
        logln!("frame hook : not installed - a host is driving this mod.");
        return;
    }
    match hook::install(&image, win::exe_base(), on_frame) {
        Ok(slot) => logln!("frame hook : installed at IAT slot {slot:#x} (PeekMessageW)"),
        Err(e) => {
            logln!("frame hook : NOT installed ({e}) - the mod stays passive.");
            return;
        }
    }

    // Watch it for a while from this thread, then leave it running.
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_secs(5));
        logln!(
            "frame hook : {} frames, game thread {}, armed {}",
            hook::frames(),
            hook::game_thread(),
            hook::armed()
        );
    }
}

/// Runs once per message pump, on the game's thread.
///
/// Still read-only. The callback rate is wildly uneven -- tens of thousands per
/// second while loading, about sixty per second in play -- so anything paced
/// here must use elapsed time, never frame counts.
fn on_frame(n: u64) {
    // The hook fires from inside `PeekMessageW`, and the mod invokes the game's
    // own code from here. If anything it invokes pumps messages -- opening a
    // reward screen is exactly the sort of thing that might -- the hook fires
    // again underneath us. Without this the mod would press a second time from
    // inside the first press, and the frame lock it holds would be taken twice
    // on one thread.
    let Some(_busy) = Reentry::enter() else { return };

    let started = FIRST_FRAME.get_or_init(std::time::Instant::now);
    poll_toggle();
    if n % 3600 == 0 {
        logln!("  [frame hook] alive at frame {n}, thread {}", unsafe {
            win::GetCurrentThreadId()
        });
    }

    // Give the game a few seconds to finish coming up before reaching into it.
    if started.elapsed().as_secs() < 5 {
        return;
    }
    clear_breadcrumb_once_safe(started);
    if !READ_TEST_DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        read_test();
    }

    // The gameplay controller only exists inside a run, so watching for it has
    // to be a poll, not a one-shot. Paced by elapsed time: the callback rate
    // swings between ~60/s in play and tens of thousands/s while loading.
    let mut m = match MONITOR.lock() {
        Ok(m) => m,
        Err(e) => e.into_inner(),
    };
    let t0 = std::time::Instant::now();
    win::flush_region_cache();

    // The picker runs every frame. It has to: the interesting moments are cards
    // appearing, cards becoming ready, and cards vanishing after a pick, and
    // any wait between those is a wait the player sees. Its cost is a handful
    // of reads unless something is actually on screen.
    //
    // The *survey* -- the wide diagnostic sweep -- keeps the interval and the
    // backoff. Putting the picker behind that gate was what made it take
    // seconds to react: an earlier slow sweep had widened the interval to four
    // seconds and the picker inherited it.
    // Deliberately NOT gated on `m.disabled`. That flag exists to stop the
    // survey from costing frames; it once stopped the picker too, so the mod
    // went quietly dead mid-session and Ctrl+Alt+P appeared to do nothing --
    // it logged "ACTING" while no code was left running to act. The feature
    // outlives the diagnostics.
    act_now(&mut m);

    if survey_on() && !m.disabled && m.last_poll.map_or(true, |t| t.elapsed() >= m.interval) {
        m.last_poll = Some(t0);
        // Timed on its own. Charging the survey for the picker's time -- and
        // for the region-cache flush -- is how a busy reward queue could trip
        // the survey's kill switch.
        let s0 = std::time::Instant::now();
        poll_run(&mut m);
        m.observe(s0.elapsed());
    }
}

/// Drop the crash-loop breadcrumb once the session has clearly survived startup.
///
/// The breadcrumb exists to catch a *probe* that kills the game on launch. It
/// was only ever cleared at process exit, so any crash -- including one three
/// minutes into play, with the probe long finished and proven fine -- disabled
/// the mod for the whole of the next session. That turned one crash into two
/// wasted launches, and the player had no way to tell the difference from the
/// mod simply being broken.
///
/// A minute of frames is proof enough that the probe was not the problem.
fn clear_breadcrumb_once_safe(started: &std::time::Instant) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if started.elapsed() < std::time::Duration::from_secs(60)
        || DONE.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }
    if let Some(marker) = home::file("probe.incomplete") {
        if marker.exists() && std::fs::remove_file(&marker).is_ok() {
            logln!(
                "startup survived a minute of play, so the crash-loop guard is stood down                  -- a later crash will not cost you the next session."
            );
        }
    }
}

/// Whether the diagnostic sweep is switched on. Off by default: it is the
/// most expensive thing the mod does, and nothing the player cares about needs
/// it.
static SURVEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn survey_on() -> bool {
    SURVEY.load(std::sync::atomic::Ordering::Relaxed)
}

/// One-at-a-time marker for the frame hook.
struct Reentry;

static IN_FRAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static REENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Reentry {
    fn enter() -> Option<Reentry> {
        if !IN_FRAME.swap(true, std::sync::atomic::Ordering::Acquire) {
            return Some(Reentry);
        }
        // Worth saying out loud. If the hook really does fire from inside a
        // press, that is the explanation for a crash with no fault report --
        // and if this line never appears, re-entrancy was never the problem and
        // the crash is still to be found.
        let n = REENTRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n == 1 || n % 100 == 0 {
            findln!("frame hook re-entered from inside itself ({n} so far) - skipping that frame.");
        }
        None
    }
}

impl Drop for Reentry {
    fn drop(&mut self) {
        // Released on the way out however that happens, panic included.
        IN_FRAME.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Default)]
struct Monitor {
    last_poll: Option<std::time::Instant>,
    in_run: bool,
    last_queue: Option<String>,
    last_ui: Option<String>,
    last_watch: Option<resolve::Watch>,
    slow_steps: u32,
    libraries_dumped: bool,
    choices_seen: u32,
    in_choice: bool,
    interval: std::time::Duration,
    over_budget: u32,
    disabled: bool,
    /// How long the picker waits between steps. Zero -- every frame -- unless
    /// it has been measured to be expensive, in which case it backs off rather
    /// than switching itself off.
    step_gap: std::time::Duration,
    last_step: Option<std::time::Instant>,
}

static MONITOR: std::sync::Mutex<Monitor> = std::sync::Mutex::new(Monitor {
    last_poll: None,
    in_run: false,
    last_queue: None,
    last_ui: None,
    last_watch: None,
    slow_steps: 0,
    libraries_dumped: false,
    choices_seen: 0,
    in_choice: false,
    interval: BASE_INTERVAL,
    over_budget: 0,
    disabled: false,
    step_gap: std::time::Duration::ZERO,
    last_step: None,
});

const BASE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
const MAX_INTERVAL: std::time::Duration = std::time::Duration::from_millis(4000);
/// A poll costing more than this is stealing frames from the player.
const POLL_BUDGET: std::time::Duration = std::time::Duration::from_millis(8);
/// Repeatedly blowing this means something is badly wrong; stop entirely.
const POLL_HARD_LIMIT: std::time::Duration = std::time::Duration::from_millis(40);
const STRIKES_BEFORE_DISABLE: u32 = 5;
/// The picker's backoff range. The floor is about one frame, so backing off at
/// all is already a visible slowdown; the ceiling still resolves a queue, just
/// four times a second instead of sixty.
const STEP_GAP_MIN: std::time::Duration = std::time::Duration::from_millis(16);
const STEP_GAP_MAX: std::time::Duration = std::time::Duration::from_millis(250);

impl Monitor {
    /// Self-limiting: the mod runs inside the player's frame, so a slow poll is
    /// a stutter they can feel.
    ///
    /// An early build made the game unplayable by doing tens of thousands of
    /// `VirtualQuery` calls per poll. That was fixed, but relying on nobody
    /// reintroducing it is not a safety property -- this is. Slow polls back
    /// off, and persistently awful ones disable the monitor outright.
    fn observe(&mut self, took: std::time::Duration) {
        if took <= POLL_BUDGET {
            // recover gradually once things are healthy again
            if self.interval > BASE_INTERVAL {
                self.interval = (self.interval / 2).max(BASE_INTERVAL);
            }
            self.over_budget = 0;
            return;
        }

        self.over_budget += 1;
        let was = self.interval;
        self.interval = (self.interval * 2).min(MAX_INTERVAL);
        if was != self.interval {
            logln!(
                "poll took {:?} (budget {:?}) - backing off to {:?}",
                took, POLL_BUDGET, self.interval
            );
        }
        if took > POLL_HARD_LIMIT && self.over_budget >= STRIKES_BEFORE_DISABLE {
            self.disabled = true;
            logln!(
                "poll took {took:?} on {} consecutive occasions - switching the survey \
                 off rather than keep costing the player frames.",
                self.over_budget
            );
            logln!(
                "  the survey is only the diagnostic sweep -- picking, rerolling and \
                 opening the queue carry on. Ctrl+Alt+P switches it back on."
            );
        }
    }
}

/// The picker, run every frame.
///
/// Cheap unless something changed: two numbers when idle. When a choice is on
/// screen it re-runs each frame until the cards are built, so the pick lands as
/// soon as the game is ready for it rather than on the next tick of a timer.
fn act_now(m: &mut Monitor) {
    if m.last_step.is_some_and(|t| t.elapsed() < m.step_gap) {
        return;
    }
    m.last_step = Some(std::time::Instant::now());
    let Some(state) = STATE.get() else { return };
    let base = win::exe_base();
    let Some(cfg) = config_now() else { return };
    note_config_act(cfg.global.act);
    press::set_pace(cfg.global.delay_ms);
    press::set_tracing(cfg.global.trace);
    SURVEY.store(cfg.global.survey, std::sync::atomic::Ordering::Relaxed);
    if !cfg.global.enabled {
        return;
    }

    let now = resolve::watch(state, base);
    // A different number of cards means a different reward -- or the same one
    // rerolled -- so the per-reward reroll budgets start again.
    if now.map(|w| w.cards) != m.last_watch.map(|w| w.cards) {
        resolve::reset_spent();
    }
    m.last_watch = now;

    let took = std::time::Instant::now();
    resolve::step(state, base, &cfg, now.map(|w| w.cards), &mut |n| {
        TIEBREAK.lock().map(|mut g| g.next(n)).unwrap_or(0)
    });
    // Its own guard, separate from the survey's: if the picker itself ever gets
    // expensive, say so rather than quietly costing frames.
    let ms = took.elapsed().as_millis();
    if ms > 6 {
        m.slow_steps += 1;
        if m.slow_steps % 120 == 1 {
            logln!("picker step took {ms}ms - watch this if the game feels uneven");
        }
        // Slow down, but never stop. The survey is a diagnostic and can be
        // switched off outright; this is the mod itself, and a player whose
        // picker has silently died cannot tell that from a config problem.
        if ms > 25 {
            m.step_gap = (m.step_gap * 2).clamp(STEP_GAP_MIN, STEP_GAP_MAX);
        }
    } else if m.step_gap > std::time::Duration::ZERO {
        m.step_gap /= 2;
        if m.step_gap < STEP_GAP_MIN {
            m.step_gap = std::time::Duration::ZERO;
        }
    }
}

/// Watch for a run starting, and report the reward queue while one is going.
fn poll_run(m: &mut Monitor) {
    let Some(state) = STATE.get() else { return };
    let base = win::exe_base();

    // Never survey through a teardown. The survey is diagnostics; the cards it
    // reads are being destroyed, and no diagnostic is worth reading freed
    // memory in a player's game. It will report on the next tick instead.
    if resolve::settling() {
        return;
    }
    press::trace("surveying the run");

    let Some(inst) = instance::find_singleton(base, "obj_gameplay_controller") else {
        if m.in_run {
            logln!("[run] gameplay controller gone - back in menus");
            m.in_run = false;
            m.last_queue = None;
        }
        return;
    };
    if !m.in_run {
        logln!("[run] gameplay controller live at {inst:#x}");
        m.in_run = true;
    }

    // Dump the game's own libraries once per run: their keys are the option id
    // vocabularies the config will be written against, so having the live list
    // lets the generated config be cross-checked rather than trusted.
    if !m.libraries_dumped {
        m.libraries_dumped = true;
        if let Ok(g) = globals::Globals::resolve(base, state.text) {
            survey::dump_libraries(state, base, &g);
        }
    }

    // `pending_rewards` is the *save-file key*, not the live variable -- it is
    // touched only by save/load code. The runtime queue is
    // `pending_rewards_list`, on this object. Likewise `run_rerolls_left` is a
    // save key; the live counter is the global FREE_REROLLS_PER_RUN_LEFT, and
    // the per-reward counters live on obj_button_reroll_cards, which exists
    // only while a reward choice is open.
    let mut parts = Vec::new();
    for var in ["pending_rewards_list", "rewards_state"] {
        let text = match state.syms.var_id(var) {
            None => "<no id>".to_string(),
            Some(id) => match unsafe { instance::get_var(inst, id) } {
                None => "<unreadable>".to_string(),
                Some(rv) => match rvalue::decode(rv) {
                    None => "<undecodable>".to_string(),
                    Some(v) => {
                        if var == "pending_rewards_list" {
                            describe_queue(state, base, &v)
                        } else {
                            summarise(&v)
                        }
                    }
                },
            },
        };
        parts.push(format!("{var}={text}"));
    }

    // The reroll button exists only while a reward choice is open, which is
    // exactly the window the per-reward budgets apply to.
    if let Some(btn) = instance::find_singleton(base, "obj_button_reroll_cards") {
        for var in ["free_rerolls_per_reward_left", "non_free_rerolls_made", "reward_type"] {
            let text = match state.syms.var_id(var) {
                None => "<no id>".to_string(),
                Some(id) => match unsafe { instance::get_var(btn, id) } {
                    None => "<unreadable>".to_string(),
                    Some(rv) => rvalue::decode(rv).map_or("<undecodable>".into(), |v| summarise(&v)),
                },
            };
            parts.push(format!("reroll.{var}={text}"));
        }
    }

    // Only on change: this runs twice a second for a whole session.
    let line = parts.join("  ");
    if m.last_queue.as_deref() != Some(line.as_str()) {
        logln!("[run] {line}");
        m.last_queue = Some(line);
    }

    // The wide survey: whenever what is on screen changes, describe it fully.
    // This is where the two questions the spec has been holding open get
    // answered, so it errs heavily towards recording too much.
    let counts = survey::ui_counts(base);
    // Cheap gate. Describing the choice reads dozens of members per card and
    // calls into the runtime for each; doing it every poll when nothing is on
    // screen was most of the poll cost.
    let cards: usize = survey::CARD_OBJECTS
        .iter()
        .enumerate()
        .map(|(i, _)| counts.get(i).copied().unwrap_or(0))
        .sum();
    let lines = if cards > 0 {
        survey::describe_choice(state, base, &counts)
    } else {
        vec![format!("  ui: {}", survey::fingerprint(&counts))]
    };

    // Fingerprint the *content*, not the object counts. Cards exist for several
    // frames before they are populated, so a count-triggered report fires once,
    // too early, and never again -- which is exactly how a placeholder reading
    // got reported as the offer three times running. Keying on the decoded text
    // means the report is repeated when the cards actually fill in.
    let ui = lines.join("\n");
    if m.last_ui.as_deref() != Some(ui.as_str()) {
        let cards: usize = survey::CARD_OBJECTS
            .iter()
            .map(|(o, _)| instance::count(base, o))
            .sum();
        if cards > 0 && !m.in_choice {
            m.choices_seen += 1;
            m.in_choice = true;
            logln!("[choice #{}] ---------------------------------", m.choices_seen);
        } else if cards == 0 {
            m.in_choice = false;
        }
        for line in &lines {
            logln!("{line}");
        }
        m.last_ui = Some(ui);
    }
}

static FIRST_FRAME: OnceLock<std::time::Instant> = OnceLock::new();
static READ_TEST_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// One-shot: prove the mod can read the game's own state.
///
/// Runs on the game's thread, which is the only place calling into the GML
/// runtime is safe.
fn read_test() {
    let Some(state) = STATE.get() else { return };

    logln!("---- reading game state ----");
    let g = match globals::Globals::resolve(win::exe_base(), state.text) {
        Ok(g) => {
            logln!(
                "globals    : container {:#x}, get-variable {:#x}",
                g.container(),
                g.getter()
            );
            g
        }
        Err(e) => {
            logln!("globals    : NOT resolved ({e}) - the mod stays passive.");
            return;
        }
    };

    // Self-validating: these values are known from the static analysis, so a
    // mismatch means the read path is wrong, not that the game changed.
    let mut correct = 0;
    for (name, expected) in KNOWN_GLOBALS {
        let Some(id) = state.syms.var_id(name) else {
            logln!("  {name}: no variable id");
            continue;
        };
        match unsafe { g.get(id) } {
            Some(v) => {
                let got = v.as_str().unwrap_or("<not a string>");
                if got == *expected {
                    correct += 1;
                    logln!("  {name} = {got:?}  OK");
                } else {
                    logln!("  {name} = {v:?}  MISMATCH - expected {expected:?}");
                    dump_rvalue(&g, state, name);
                }
            }
            None => logln!("  {name}: read failed"),
        }
    }

    if correct == KNOWN_GLOBALS.len() {
        logln!("read path CONFIRMED: {correct}/{} known globals match.", KNOWN_GLOBALS.len());
    } else {
        logln!(
            "read path SUSPECT: only {correct}/{} known globals match; staying passive.",
            KNOWN_GLOBALS.len()
        );
        return;
    }

    // With reads proven, see what else is reachable as a global. This is
    // reconnaissance for the reward queue: `pending_rewards` is expected to be
    // an *instance* variable on the gameplay controller rather than a global,
    // and this is the cheapest way to find out.
    logln!("---- what is reachable as a global ----");
    for name in [
        "REWARDS", "UNIT_CLASSES_LENGTH",
        "FREE_REROLLS_PER_RUN_LEFT", "FREE_REROLLS_PER_REWARD_LIMIT",
    ] {
        match state.syms.var_id(name) {
            None => logln!("  {name:<32} no variable id"),
            Some(id) => match unsafe { g.get(id) } {
                Some(v) => logln!("  {name:<32} {}", summarise(&v)),
                None => logln!("  {name:<32} not readable as a global"),
            },
        }
    }

    read_controller(state);
}

/// Find the gameplay controller and read the reward queue off it.
///
/// Pure reads to locate the instance -- no game code is patched and no game
/// function is called to find it. Only the variable read itself goes through
/// the runtime, using the interface already proven on globals.
fn read_controller(state: &State) {
    logln!("---- the gameplay controller ----");
    let base = win::exe_base();

    let Some(reg) = instance::Registry::open(base) else {
        logln!("object registry not readable; staying passive.");
        return;
    };
    let objects = reg.objects();
    logln!("object registry: {} objects", objects.len());

    for name in ["obj_gameplay_controller", "obj_run_controller"] {
        let Some((index, obj)) = reg.find_object(name) else {
            logln!("  {name}: not in the registry");
            continue;
        };
        let inst = instance::find_singleton(base, name);
        logln!("  {name}: index {index}, CObjectGML {obj:#x}, instance {inst:?}");

        let Some(inst) = inst else {
            logln!("    no live instance (expected outside a run)");
            continue;
        };
        for var in [
            "pending_rewards",
            "run_rerolls_left",
            "free_rerolls_per_reward_left",
            "non_free_rerolls_made",
        ] {
            let Some(id) = state.syms.var_id(var) else {
                logln!("    {var:<30} no variable id");
                continue;
            };
            match unsafe { instance::get_var(inst, id) } {
                None => logln!("    {var:<30} read failed"),
                Some(rv) => match rvalue::decode(rv) {
                    Some(v) => {
                        logln!("    {var:<30} {}", summarise(&v));
                        // the queue is expected to be an array; its layout is
                        // the next unknown, so capture it while we are here
                        if let rvalue::Value::Array(p) = v {
                            logln!("      array payload {p:#x} ({})", rvalue::region(p));
                            logln!("      bytes:{}", rvalue::dump(p, 0, 64));
                        }
                    }
                    None => logln!("    {var:<30} undecodable RValue at {rv:#x}"),
                },
            }
        }
    }
}

/// Render the reward queue: length, plus each entry's type and parameters.
///
/// Entries are structs matching the save file's shape --
/// `{reward_type, options_amount}` or `{reward_type, quantity}`.
fn describe_queue(state: &State, base: usize, v: &rvalue::Value) -> String {
    let Some(list) = dslist::DsList::from_value(base, v) else {
        return format!("{} (not a readable ds_list)", summarise(v));
    };
    let n = list.len();
    if n == 0 {
        return "empty".to_string();
    }

    let field = |entry: &rvalue::Value, name: &str| -> Option<rvalue::Value> {
        let id = state.syms.var_id(name)?;
        unsafe { dslist::struct_member(entry, id) }
    };

    // Summarise as counts by type, and spell out the head -- that is the only
    // one the picker may ever touch, under the strict-FIFO rule.
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut head = String::new();
    for i in 0..n.min(256) {
        let Some(entry) = list.get(i) else { continue };
        let ty = field(&entry, "reward_type")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "<unknown>".into());
        if i == 0 {
            let opts = field(&entry, "options_amount").and_then(|v| v.as_f64());
            let qty = field(&entry, "quantity").and_then(|v| v.as_f64());
            head = format!(
                "head={ty}{}{}",
                opts.map(|o| format!(" options={o}")).unwrap_or_default(),
                qty.map(|q| format!(" quantity={q}")).unwrap_or_default()
            );
        }
        match counts.iter_mut().find(|(t, _)| *t == ty) {
            Some((_, c)) => *c += 1,
            None => counts.push((ty, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let by_type = counts
        .iter()
        .map(|(t, c)| format!("{t}x{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{n} queued [{by_type}] {head}")
}

pub fn summarise(v: &rvalue::Value) -> String {
    use rvalue::Value::*;
    match v {
        Str(s) if s.len() > 60 => format!("Str({:?}...)", &s[..60]),
        Array(p) => format!("Array @ {p:#x}"),
        Object(p) => format!("Object @ {p:#x}"),
        other => format!("{other:?}"),
    }
}

/// Only on a mismatch: show the bytes so the layout can be fixed from evidence.
fn dump_rvalue(g: &globals::Globals, state: &State, name: &str) {
    let Some(id) = state.syms.var_id(name) else { return };
    let Some(rv) = (unsafe { g.get_raw(id) }) else { return };
    logln!("  rvalue at {rv:#x} ({}):{}", rvalue::region(rv), rvalue::dump(rv, 0, 16));
    let ptr = unsafe { core::ptr::read_volatile(rv as *const usize) };
    logln!("  payload {ptr:#x} ({}):{}", rvalue::region(ptr), rvalue::dump(ptr, 16, 64));
}

#[cfg(feature = "standalone")]
#[no_mangle]
pub extern "system" fn DllMain(module: Hmodule, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == win::DLL_PROCESS_DETACH {
        // A clean exit is the only proof the session survived. Take the frame
        // hook out first, then drop the breadcrumb -- so a game that died with
        // the hook running still has one, and the next launch stays passive.
        hook::uninstall();
        if let Some(marker) = home::file("probe.incomplete") {
            let _ = std::fs::remove_file(marker);
        }
        return 1;
    }
    if reason == win::DLL_PROCESS_ATTACH {
        unsafe {
            // we have no per-thread state; not being called for every thread
            // the game spawns is free performance and one less way to deadlock
            win::DisableThreadLibraryCalls(module);
            let h: Handle = win::CreateThread(
                core::ptr::null_mut(),
                0,
                init,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
            );
            if !h.is_null() {
                win::CloseHandle(h);
            }
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn monitor() -> Monitor {
        Monitor { interval: BASE_INTERVAL, ..Default::default() }
    }

    #[test]
    fn healthy_polls_stay_at_the_base_interval() {
        let mut m = monitor();
        for _ in 0..10 {
            m.observe(Duration::from_micros(200));
        }
        assert_eq!(m.interval, BASE_INTERVAL);
        assert!(!m.disabled);
    }

    #[test]
    fn slow_polls_back_off_and_then_recover() {
        let mut m = monitor();
        m.observe(POLL_BUDGET + Duration::from_millis(1));
        assert!(m.interval > BASE_INTERVAL, "a slow poll must widen the interval");
        let backed_off = m.interval;

        m.observe(Duration::from_micros(100));
        assert!(m.interval < backed_off, "a healthy poll must narrow it again");

        for _ in 0..10 {
            m.observe(Duration::from_micros(100));
        }
        assert_eq!(m.interval, BASE_INTERVAL, "recovery must stop at the base interval");
        assert!(!m.disabled, "backing off must never disable on its own");
    }

    #[test]
    fn backoff_is_bounded() {
        let mut m = monitor();
        for _ in 0..50 {
            m.observe(POLL_BUDGET + Duration::from_millis(1));
        }
        assert_eq!(m.interval, MAX_INTERVAL);
    }

    #[test]
    fn persistently_awful_polls_disable_the_monitor() {
        let mut m = monitor();
        let awful = POLL_HARD_LIMIT + Duration::from_millis(1);
        for _ in 0..STRIKES_BEFORE_DISABLE - 1 {
            m.observe(awful);
            assert!(!m.disabled, "must not disable before the strike count");
        }
        m.observe(awful);
        assert!(m.disabled, "must disable once the strikes run out");
    }

    #[test]
    fn merely_slow_polls_never_disable() {
        let mut m = monitor();
        // over budget, but nowhere near the hard limit: back off, never disable
        for _ in 0..100 {
            m.observe(POLL_BUDGET + Duration::from_millis(1));
        }
        assert!(!m.disabled);
    }
}

// ---------------------------------------------------------------- acting

/// Whether the mod may press buttons right now.
///
/// One switch, not two. `[global] act` in the config is the *starting* value;
/// Ctrl+Alt+P toggles it during play, so enabling the mod does not mean
/// alt-tabbing to a text file. Editing `act` in the config also takes effect,
/// because changing it there is a deliberate statement of intent.
///
/// Observation is unaffected: the mod always reads the choice and logs what it
/// would do. Only pressing is gated. That way switching it off leaves you with
/// a running commentary rather than silence.
static ACTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The last `act` value seen in the config, so an edit can be distinguished
/// from the runtime toggle.
static CONFIG_ACT: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
/// Why the mod switched itself off, if it did.
static OFF_REASON: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Adopt `act` from the config when it changes; otherwise leave the toggle be.
fn note_config_act(act: bool) {
    let mut g = match CONFIG_ACT.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if *g != Some(act) {
        if g.is_some() {
            logln!("config: act = {act} (adopted from the config file)");
        }
        *g = Some(act);
        ACTING.store(act, std::sync::atomic::Ordering::Relaxed);
        if act {
            if let Ok(mut r) = OFF_REASON.lock() {
                *r = None;
            }
        }
    }
}

/// Stop pressing, and say why.
///
/// Meeting an option the config does not classify means the config no longer
/// describes the game, so carrying on would be guessing. It goes dormant, not
/// dead: Ctrl+Alt+P brings it back without restarting.
pub fn shut_down(reason: impl Into<String>) {
    let reason = reason.into();
    if !ACTING.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return; // already not acting; do not spam the log
    }
    logln!("=== AUTO-PICKER OFF: {reason}");
    logln!("=== it will keep reporting what it WOULD do, but press nothing.");
    logln!("=== fix the config, or press Ctrl+Alt+P to switch it back on.");
    if let Ok(mut g) = OFF_REASON.lock() {
        *g = Some(reason);
    }
}

pub fn off_reason() -> Option<String> {
    OFF_REASON.lock().map(|g| g.clone()).unwrap_or(None)
}

/// Whether the mod may press a button right now.
pub fn acting() -> bool {
    ACTING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Ctrl+Alt+P toggles pressing on and off.
///
/// A two-modifier chord, chosen so it cannot collide with the game's own
/// bindings -- those use letters, digits, space and the arrows unmodified.
fn poll_toggle() {
    use std::sync::atomic::Ordering;
    static WAS_DOWN: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    let down = win::key_down(win::VK_CONTROL)
        && win::key_down(win::VK_MENU)
        && win::key_down(win::vk_letter(b'P'));

    // edge-triggered: holding the chord toggles once, not sixty times a second
    if down && !WAS_DOWN.swap(true, Ordering::Relaxed) {
        let now_on = !ACTING.fetch_xor(true, Ordering::Relaxed);
        if now_on {
            // Pressing the key is the player asking for the mod's attention, so
            // it is also the moment to give the survey another chance. Without
            // this, a survey switched off for cost stays off until the game is
            // restarted, and the player has no way to ask for it back.
            if let Ok(mut m) = MONITOR.lock() {
                if m.disabled {
                    m.disabled = false;
                    m.over_budget = 0;
                    m.interval = BASE_INTERVAL;
                    logln!("toggle: the survey was switched off for cost; trying it again.");
                }
                m.step_gap = std::time::Duration::ZERO;
            }
            match off_reason() {
                Some(r) => {
                    logln!("toggle: ACTING (overriding an earlier shutdown: {r})");
                    if let Ok(mut g) = OFF_REASON.lock() {
                        *g = None;
                    }
                }
                None => logln!("toggle: ACTING - the mod will now press buttons"),
            }
        } else {
            logln!("toggle: OBSERVING only - it will report but press nothing");
        }
    } else if !down {
        WAS_DOWN.store(false, Ordering::Relaxed);
    }
}

// ------------------------------------------------------------------- config

static TIEBREAK: std::sync::Mutex<picker::TieBreaker> =
    std::sync::Mutex::new(picker::TieBreaker::new_const());

struct Loaded {
    cfg: std::sync::Arc<config::Config>,
    mtime: Option<std::time::SystemTime>,
    checked: std::time::Instant,
}

static LOADED: std::sync::Mutex<Option<Loaded>> = std::sync::Mutex::new(None);

/// The config as it is on disk, reloaded when the file changes.
///
/// A rejected reload keeps the last good config in force rather than falling
/// back to nothing: a typo saved mid-run must not silently change how rewards
/// resolve.
/// The name the standalone DLL claims, so a hosted copy can tell it is there.
///
/// Both copies press buttons on the reward screen. Two of them pressing on one screen
/// is a corrupted run, so exactly one must act, and the rule is that **the standalone
/// install wins**: it is the deliberate one, and it is the one whose `config.ini` the
/// player has tuned. The hosted copy asks and yields.
pub const STANDALONE_CLAIM: &str = "tkiw_reward_picker_standalone";

/// Held for the life of the process once the standalone DLL starts.
#[cfg(feature = "standalone")]
static STANDALONE: std::sync::Mutex<Option<tkiw_runtime::claim::Claim>> =
    std::sync::Mutex::new(None);

/// Whether the standalone DLL is running in this process. Cheap: one handle open.
///
/// Only true for a standalone built after the claim existed. For an installation
/// predating it -- which is every one already out there -- use
/// [`standalone_installed`], and use this for the repeated check afterwards.
pub fn standalone_running() -> bool {
    tkiw_runtime::claim::held(STANDALONE_CLAIM)
}

/// Whether a standalone picker DLL sits in the game folder.
///
/// The claim above cannot see an installation built before the claim existed, and that
/// is every one currently installed. This can: a standalone install is a proxy DLL in
/// the game folder carrying the stamp marker, and the executable imports it, so present
/// on disk means loaded in the process.
///
/// Reads a handful of DLLs, so it is a once-at-startup check rather than a per-frame
/// one. The marker cannot match this crate when it is hosted: the stamp that contains
/// it is compiled only into the standalone build.
pub fn standalone_installed() -> bool {
    if standalone_running() {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(dir) = exe.parent() else { return false };
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e.eq_ignore_ascii_case("dll")) != Some(true) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes.windows(MARKER.len()).any(|w| w == MARKER) {
            return true;
        }
    }
    false
}

/// Where the config lives when this crate is hosted inside another mod.
///
/// Standalone, the picker owns its folder and reads `config.ini` from it. Hosted by
/// the kit it does not: the kit's folder is laid out `config/<mod>.ini`, and the
/// picker's file has to be the one the kit lists, mirrors and shows in its window.
/// Set once by the host before the first frame; unset means standalone.
static HOSTED_CONFIG: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// Host this picker inside another mod: read config from `path`, not from
/// `<mod folder>/config.ini`.
pub fn host_config_at(path: std::path::PathBuf) {
    if let Ok(mut g) = HOSTED_CONFIG.lock() {
        *g = Some(path);
    }
}

/// A file beside the config, wherever the config is.
///
/// Standalone that is `<mod folder>/config.<suffix>`; hosted it is the kit's
/// `config/reward-picker.<suffix>`, so the pair never ends up split across folders.
fn beside_config(suffix: &str) -> Option<std::path::PathBuf> {
    match HOSTED_CONFIG.lock().ok().and_then(|g| g.clone()) {
        Some(p) => Some(p.with_extension(suffix)),
        None => home::file(&format!("config.{suffix}")),
    }
}

/// Resolve the game and make this crate ready to be driven by a host.
///
/// The standalone DLL did this inside its `DllMain` thread, alongside arming the
/// crash breadcrumb and waiting out an interrupted probe -- lifecycle the host already
/// provides. This is only the part that matters to the picking: find the symbols and
/// build the state everything else reads.
///
/// Without it `STATE` is never set and the picker sits there doing nothing. That is
/// exactly what it did for two hundred seconds the first time this was hosted.
/// Set while a host is driving this crate, so the parts of startup the host already
/// provides are skipped.
static HOSTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn hosted() -> bool {
    HOSTED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn hosted_start() {
    HOSTED.store(true, std::sync::atomic::Ordering::Relaxed);
    // On its own thread: `probe` sleeps in five-second rounds waiting for the game to
    // fill in variable ids, and the host's startup must not wait for that. Hosted, it
    // blocked the kit for forty seconds and every feature after this one started late.
    let _ = std::thread::Builder::new()
        .name("tkiw-picker-probe".into())
        .spawn(probe);
}

/// Drive one frame from a host that owns the frame hook.
///
/// The kit already has a hook, a re-entry guard, a panic boundary and a budget, so a
/// hosted picker must not install a second set of any of them. This is the same work
/// the standalone DLL does per frame, minus the lifecycle the host provides.
pub fn hosted_frame(n: u64) {
    on_frame(n);
}

fn config_now() -> Option<std::sync::Arc<config::Config>> {
    let path = match HOSTED_CONFIG.lock().ok().and_then(|g| g.clone()) {
        Some(p) => p,
        None => home::file("config.ini")?,
    };
    let mut g = match LOADED.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    let stale = match g.as_ref() {
        None => true,
        Some(l) => l.checked.elapsed() >= std::time::Duration::from_secs(2),
    };
    if !stale {
        return g.as_ref().map(|l| l.cfg.clone());
    }

    let mut mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

    // No file: write a complete, inert one from the live game. Retried on the
    // ordinary two-second cadence, because the libraries this reads are not up
    // on the first frame after injection -- one attempt at load time would
    // almost always be too early, and would leave the player with no file and
    // no way to get one short of reinstalling.
    if mtime.is_none() && may_build() {
        if let Some(cfg) = write_default_config(&path) {
            let arc = std::sync::Arc::new(cfg);
            mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            *g = Some(Loaded { cfg: arc.clone(), mtime, checked: std::time::Instant::now() });
            return Some(arc);
        }
    } else if mtime.is_some() {
        // A config already exists, so it is not touched -- the player's tiers
        // and weights are theirs. But a game update can add options that an
        // existing file has no line for, and the mod stops dead at an option it
        // cannot classify. So the current lists are written alongside, once, as
        // something to diff against.
        write_reference();
    }

    let changed = g.as_ref().map(|l| l.mtime != mtime).unwrap_or(true);
    if !changed {
        if let Some(l) = g.as_mut() {
            l.checked = std::time::Instant::now();
        }
        return g.as_ref().map(|l| l.cfg.clone());
    }

    // Give any unvalued wanted/fallback entry an explicit `= 0` before reading
    // it, so the file says what the mod will do rather than relying on the
    // player remembering an implied default.
    normalise_config(&path);

    match config::Config::load(&path) {
        Ok(cfg) => {
            if cfg.errors.is_empty() {
                logln!("config: loaded, {} reward type(s) configured", cfg.types.len());
            } else {
                logln!("config: {} problem(s):", cfg.errors.len());
                for e in &cfg.errors {
                    logln!("  {e}");
                }
            }
            let arc = std::sync::Arc::new(cfg);
            *g = Some(Loaded { cfg: arc.clone(), mtime, checked: std::time::Instant::now() });
            Some(arc)
        }
        Err(e) => {
            if g.is_none() {
                logln!("config: none at {} ({e})", path.display());
                *g = Some(Loaded {
                    cfg: std::sync::Arc::new(config::Config::default()),
                    mtime,
                    checked: std::time::Instant::now(),
                });
            }
            g.as_ref().map(|l| l.cfg.clone())
        }
    }
}

/// Reading every library is not free, and this runs on the game's own thread.
/// The libraries are up within a few seconds of launch, so a minute of retries
/// is generous; past that, something is wrong that waiting will not fix, and
/// walking them every two seconds for the rest of the session would be the kind
/// of background cost this mod exists to avoid.
const MAX_BUILD_TRIES: u32 = 30;
static BUILD_TRIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// True while it is still worth asking the game for its libraries.
fn may_build() -> bool {
    if GAVE_UP.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let n = BUILD_TRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n == MAX_BUILD_TRIES {
        logln!("config: gave up reading the game's libraries after {MAX_BUILD_TRIES} tries.");
    }
    n < MAX_BUILD_TRIES
}

/// Set once generation has failed for a reason that retrying cannot fix, so the
/// log says why once rather than every two seconds forever.
static GAVE_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Set once the "waiting for the libraries" note has been made.
static WAITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Write the current option lists next to the config, for diffing.
///
/// Never overwrites: an existing reference is the player's to delete when they
/// have finished with it.
fn write_reference() {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let Some(path) = beside_config("reference.ini") else { return };
    if path.exists() {
        return;
    }
    if !may_build() {
        return;
    }
    let Some(state) = STATE.get() else { return };
    let base = win::exe_base();
    let Ok(Some((vocab, excluded))) = std::panic::catch_unwind(|| vocab::build(state, base)) else {
        return;
    };
    if vocab.iter().any(|t| t.options.is_empty()) {
        return;
    }
    if std::fs::write(&path, config::generate(&vocab, &excluded)).is_ok()
        && !DONE.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        logln!("config: wrote {} -- the option lists as this game build has", path.display());
        logln!("        them. Diff it against config.ini to find anything new; delete it");
        logln!("        when you are done. Your config.ini was not touched.");
    }
}

fn give_up() {
    GAVE_UP.store(true, std::sync::atomic::Ordering::Relaxed);
    logln!("config: not trying again. Ship or write a config.ini by hand.");
}

/// Rewrite the config in place so implied weights become written ones.
///
/// Only ever adds `= 0` to an option that had no weight; never reorders,
/// reformats, or touches a comment. Converges after one pass -- there is a test
/// for that, because a rewrite that keeps finding work would rewrite the
/// player's file on every reload forever.
fn normalise_config(path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Some(fixed) = config::normalise(&text) else { return };
    match std::fs::write(path, &fixed) {
        Ok(()) => logln!(
            "config: gave every unvalued wanted/fallback option an explicit `= 0` in {}",
            path.display()
        ),
        Err(e) => logln!("config: could not write back the weights ({e}); reading it as it is."),
    }
}

/// Write a complete, inert config from the running game, on first run.
///
/// Everything starts blacklisted with zero budgets, so installing the mod and
/// never opening the file changes nothing about the game. The file doubles as
/// the reference for what is configurable: the player moves ids between tiers
/// rather than typing them from memory, which is what makes "every id in
/// exactly one tier" a reasonable rule to enforce.
///
/// Built from *this* installation's libraries, so it is right for whatever
/// build the player has rather than the one the mod was developed against.
fn write_default_config(path: &std::path::Path) -> Option<config::Config> {
    let state = STATE.get()?;
    let base = win::exe_base();

    let (vocab, excluded) = match std::panic::catch_unwind(|| vocab::build(state, base)) {
        Ok(Some(v)) => v,
        // Not an error: the libraries are built during startup, so this simply
        // means "too early". Try again on the next check.
        Ok(None) => {
            if !WAITED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                logln!("config: none on disk; waiting for the game's libraries to write one.");
            }
            return None;
        }
        Err(_) => {
            logln!("config: PANIC while reading the libraries; not writing one.");
            give_up();
            return None;
        }
    };

    // A type with no options means something is wrong with the filtering, and a
    // section that lists nothing would silently make that type unresolvable.
    let empty: Vec<&str> = vocab
        .iter()
        .filter(|t| t.options.is_empty())
        .map(|t| t.reward_type)
        .collect();
    if !empty.is_empty() {
        logln!("config: NOT writing one - these types came out empty: {empty:?}");
        logln!("        that means the option pools were read wrongly, and a config");
        logln!("        built from them would be worse than none at all.");
        give_up();
        return None;
    }

    let text = config::generate(&vocab, &excluded);
    if let Err(e) = std::fs::write(path, &text) {
        logln!("config: could not write {}: {e}", path.display());
        give_up();
        return None;
    }
    let total: usize = vocab.iter().map(|t| t.options.len()).sum();
    logln!(
        "config: wrote {} with {} reward types and {total} options, all blacklisted.",
        path.display(),
        vocab.len()
    );
    logln!("        edit it to choose what the mod should take; it does nothing until you do.");
    Some(config::parse(&text))
}

