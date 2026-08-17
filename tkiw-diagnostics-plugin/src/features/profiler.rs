//! Where the game's time goes, in terms you can act on.
//!
//! ## What the first profiler got wrong
//!
//! It answered "where is the CPU executing", which produced `sub_1c9fd30 28.5%` and
//! `sub_1ca3ab0 17.2%` at the top of a startup profile -- 45.7% of a launch, named by
//! nothing. Turning that into an answer took a day of disassembly: recognising a CRC-32
//! inner loop, matching the polynomial table, and scanning for the two functions' shared
//! caller -- and the conclusion, "one Ogg/Vorbis decoder", was wrong. They are texture
//! page decompression. A profiler that needs that much follow-up has not profiled
//! anything, and the follow-up it needed got the wrong answer.
//!
//! It also ranked by self time and printed 25 rows, so the game's C++ runtime, `ntdll`,
//! `win32u`, `d3d11` and the decompressor filled every slot and not one GML function appeared.
//! And it reported on a wall-clock timer, so `obj_init` and the splash screen were
//! averaged together and neither could be recovered.
//!
//! ## The three views
//!
//! Every sample is counted three ways:
//!
//! | view | question |
//! |---|---|
//! | **self** | where the CPU is literally executing |
//! | **inclusive** | what is expensive, whoever is running it |
//! | **responsible** | *which GML function asked for this work* |
//!
//! The third is the one that turns an address into an answer. It is the innermost frame
//! on the stack that resolves to a named GML function, so a sample deep inside the
//! allocator or a codec is charged to the GML that caused it.
//!
//! Samples with **no** named GML frame anywhere are charged to their module instead and
//! reported separately, so engine and OS work can never be mistaken for game code. That
//! separation is what makes the responsible column worth trusting.
//!
//! ## Why the responsible column can be trusted, and where it cannot
//!
//! Checked rather than assumed: **12,769 of 13,132 named GML functions (97.2%) carry
//! their own `.pdata` entry**, so `RtlVirtualUnwind` walks them exactly; the 363 without
//! are trivial config scripts with no prologue, where the leaf rule applies. And the old
//! profiler's own numbers showed `ntdll.dll` at 99.0% inclusive against 11.6% self --
//! the thread's base frame, in almost every sample, which means stacks were reaching the
//! bottom.
//!
//! What it cannot do is attribute **deferred** work. If GML queues something the engine
//! performs later, no GML frame is on the stack when it runs, and the sample lands in
//! the engine table. That is what a sampling profiler measures; naming a cause would
//! need a different and much larger tool. It is stated here because a GML function that
//! causes expensive work without being on the stack for it will look cheap.
//!
//! Two numbers on every report say whether to believe the column at all: the share of
//! samples that found no named GML frame, and the number of stacks that hit
//! [`sample::MAX_FRAMES`]. If either is large, the report says so on its own face rather
//! than presenting a confident wrong table.
//!
//! ## Reports are keyed to the phase, not to the clock
//!
//! The game thread writes the current phase into an atomic; the sampler reads it per
//! sample. **The sampler never touches the object registry** -- a lookup costs about 2ms
//! and would dwarf the sample it is labelling. A report is emitted when a phase ends, so
//! `obj_init` and the splash are never averaged together again.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tkiw_runtime::{
    findln, home, hook, instance, logln,
    modules::Modules,
    sample::{self, Target, Unwinder},
    symbolize::Symbolizer,
    Runtime,
};

use momomod_kit::config::Section;
use momomod_kit::feature::{Cadence, Feature, Requirements};

/// The phases worth telling apart, most specific first: during a run the menu's
/// controller may still exist, and the innermost thing the player is in is the answer.
///
/// Each is `(object, description, frame prefix)`. The third is what makes the labelling
/// work at all: **an object is not in the instance registry while its own Create event
/// is still running**, so polling for `obj_init` first saw it at 42.1s, long after the
/// 36 seconds of work its Create had already done. Every one of those samples was
/// filed under "before obj_init".
///
/// So the phase is taken from the stack when the stack says: a sample with
/// `obj_init_Create_0` on it is in the init phase by definition, registry or not. The
/// registry poll stays as the fallback for when the game is idle and none of these
/// frames is on the stack -- sitting on the menu, mostly.
const PHASES: &[(&str, &str, &str)] = &[
    ("obj_gameplay_controller", "in a run", "obj_gameplay_controller"),
    ("obj_main_menu", "the main menu", "obj_main_menu"),
    ("obj_splash_screen", "the splash screens", "obj_splash_screen"),
    ("obj_init", "the init room", "obj_init"),
];
/// Index into [`PHASES`], or [`UNKNOWN`] before anything has been seen.
const UNKNOWN: u8 = u8::MAX;

/// What the game thread has most recently seen. Read by the sampler, per sample.
static PHASE: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Address ranges nobody has a name for, that somebody has since identified.
///
/// Data rather than code, so that identifying an unnamed hot spot costs one line here
/// and never has to be done twice. Seeded with what the first startup profile cost a day
/// to learn.
/// Ranges are each function's own `.pdata` extent, not a span covering several. The
/// first attempt used `0x1c9fb28..0x1ca269c` for the pair, which is the *decoder's* own
/// extent -- it contains one of the two and misses the other by 5 KB.
///
/// **These two were called "ogg/vorbis decode" for a day, and it was wrong.** The
/// polynomial fit, and the image contains `OggS` and `vorbis`, so the identification
/// looked solid. It was circumstantial: those strings live in the audio subsystem,
/// which is nowhere near this code. A literal captured stack settled it --
///
/// ```text
/// #6  sub_1c0dd90              the per-page loader
/// #7  sub_1c53cc6              inside texture_prefetch (0x1c53c30)
/// #8  call_builtin_by_index
/// #9  obj_init_Create_0
/// ```
///
/// -- so it decompresses **texture pages**. CRC-32 with polynomial 0x04C11DB7 is
/// bzip2's block checksum as much as Ogg's page checksum, and the image carries `bzip`
/// strings and a `1.0.8` marker, which is bzip2's version. Naming it bzip2 outright
/// would repeat the mistake, so it is named for what the stack proves: texture pages.
const KNOWN: &[(usize, usize, &str)] = &[
    // The parent that calls both halves.
    (0x1c9fb28, 0x1c9fd30, "texture page decompress"),
    // The decompressor: 10,604 bytes around a very large state struct.
    (0x1c9fd30, 0x1ca269c, "texture page decompress"),
    // CRC-32, MSB-first, polynomial 0x04C11DB7 against the table at 0x29856d0. Not
    // PNG's or zlib's, which both use the reflected 0xEDB88320.
    (0x1ca3ab0, 0x1ca427a, "texture page checksum"),
    // `rep stosb`, sixteen bytes long.
    (0x1ea5cc0, 0x1ea5cd0, "memset"),
    // Walks a table of pointers backwards, comparing each entry byte by byte against
    // the argument. A linear scan per lookup, which is worth knowing about: it is the
    // shape that turns a growing table into a growing startup.
    (0x1b54600, 0x1b54684, "string table lookup (linear)"),
    // Checks a magic of 0x716f6966 -- "fioq" in memory, present 8 times in the image --
    // then clears a 0x100-byte index and walks pixels with 0xff000000 alpha. QOI, in
    // the variant GameMaker uses for texture pages.
    (0x1c09667, 0x1c098e9, "qoi image decode"),
];

/// A sample is "stalled" if the game has not returned to its message loop for this long.
/// An average profile of a hitching game is dominated by the frames that were fine.
const STALL_MS: u64 = 20;

fn phase_name(p: u8) -> &'static str {
    PHASES.get(p as usize).map(|(o, _, _)| *o).unwrap_or("(no phase on the stack)")
}

/// The phase a stack is in, from the frames themselves. `None` if none of them says.
fn phase_of_stack(sym: &Symbolizer, base: usize, frames: &[usize]) -> Option<u8> {
    for &pc in frames {
        let site = sym.resolve(base, pc);
        if !site.inside || !sym.is_gml(site.func) {
            continue;
        }
        let name = sym.name_of(site.func);
        for (i, (_, _, prefix)) in PHASES.iter().enumerate() {
            if name.starts_with(prefix) {
                return Some(i as u8);
            }
        }
    }
    None
}

fn known_name(rva: usize) -> Option<&'static str> {
    KNOWN.iter().find(|(lo, hi, _)| rva >= *lo && rva < *hi).map(|(_, _, n)| *n)
}

pub struct Profiler {
    interval: Duration,
    top: usize,
    stalls: bool,
    /// Sampling stops itself after this. Zero means never, which is a choice a player
    /// has to make deliberately rather than one they can be left in.
    stop_after: Duration,
    /// Print whole stacks for samples whose innermost frame's name contains this.
    ///
    /// An aggregate says a function is hot; it cannot say who called it, and inferring
    /// the caller from inclusive percentages got the answer wrong once already. A
    /// handful of literal stacks settles it.
    trace: String,
    stop: Option<Arc<AtomicBool>>,
}

impl Default for Profiler {
    fn default() -> Profiler {
        Profiler {
            interval: Duration::from_millis(1),
            top: 25,
            stalls: true,
            // Long enough for a startup and a look at the menu, short enough that a
            // profiler left switched on is not a profiler running all evening. This
            // exists because one was left on, and the game raised "font already exists"
            // out of a load callback that fired twice.
            stop_after: Duration::from_secs(120),
            trace: String::new(),
            stop: None,
        }
    }
}

/// One phase's worth of counts.
#[derive(Default)]
struct Bucket {
    samples: u64,
    stalled: u64,
    /// Innermost named GML function -> samples charged to it.
    responsible: HashMap<u32, u64>,
    /// Leaf function -> samples whose innermost frame was there.
    self_: HashMap<u32, u64>,
    /// Function -> samples with it anywhere on the stack.
    incl: HashMap<u32, u64>,
    /// Leaves outside the game module, by module name.
    outside: HashMap<String, u64>,
    /// Samples where nothing on the stack was a named GML function.
    no_gml: u64,
    /// Stacks that hit the frame ceiling, and so may have lost their caller.
    truncated: u64,
    began: Option<Instant>,
    ended: Option<Instant>,
}

impl Bucket {
    fn note(&mut self, now: Instant) {
        if self.began.is_none() {
            self.began = Some(now);
        }
        self.ended = Some(now);
    }

    fn seconds(&self) -> f64 {
        match (self.began, self.ended) {
            (Some(a), Some(b)) => b.duration_since(a).as_secs_f64(),
            _ => 0.0,
        }
    }
}

impl Feature for Profiler {
    fn name(&self) -> &'static str {
        "profiler"
    }

    fn module(&self) -> &'static str {
        "diagnostics"
    }

    fn summary(&self) -> &'static str {
        "Logs which processes in the game consume how much time. Unsuitable for \
         regular gameplay."
    }

    fn config_template(&self) -> &'static str {
        "# How often to take a sample.\n\
         interval_ms = 1\n\
         # How many separate functions are listed in the report; heaviest first.\n\
         top = 25\n\
         # Report how long hiccups last.\n\
         stalls = true\n\
         # Stop sampling after this many seconds. 0 never stops.\n\
         stop_after_s = 120\n"
    }

    fn requires(&self) -> Requirements {
        Requirements { objects: &["obj_init", "obj_main_menu"], ..Requirements::default() }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        let ms = section.u64("interval_ms", 1)?;
        if !(1..=1000).contains(&ms) {
            return Err(format!("interval_ms: {ms} is outside 1..1000"));
        }
        self.interval = Duration::from_millis(ms);

        let top = section.u64("top", 25)?;
        if !(1..=500).contains(&top) {
            return Err(format!("top: {top} is outside 1..500"));
        }
        self.top = top as usize;

        self.stalls = section.bool("stalls", true)?;
        self.trace = section.get("trace").unwrap_or("").to_string();
        self.stop_after = Duration::from_secs(section.u64("stop_after_s", 120)?);
        for k in
            section.unknown(&["enabled", "interval_ms", "top", "stalls", "stop_after_s", "trace"])
        {
            logln!("[profiler] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn cadence(&self) -> Cadence {
        // Phase boundaries are seconds apart, and each check is four registry lookups
        // at roughly 2ms. This is the whole cost of the feature on the game's thread.
        Cadence::Interval(Duration::from_millis(250))
    }

    fn activate(&mut self, rt: &Runtime) -> Result<(), String> {
        // The game's thread id is *not* known here. Features activate before the game
        // has run a frame, which is exactly when a startup profiler needs to start, so
        // the sampler waits for the id rather than this refusing to start without it.
        //
        // Built here, on our own time, and moved into the sampler: it sorts ~86k
        // entries, which is not something to do between a suspend and a resume.
        let symbolizer = Symbolizer::build(&rt.image, &rt.syms.functions);
        let base = rt.base;
        let interval = self.interval;
        let top = self.top;
        let stalls = self.stalls;
        let stop_after = self.stop_after;
        let trace = self.trace.clone();

        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        std::thread::Builder::new()
            .name("momomod-profiler".into())
            .spawn(move || run(base, symbolizer, interval, top, stalls, stop_after, trace, stop))
            .map_err(|e| format!("could not start the sampling thread: {e}"))?;

        logln!(
            "[profiler] sampling every {}ms once the game's thread appears; reporting per phase",
            interval.as_millis()
        );
        Ok(())
    }

    fn deactivate(&mut self, _rt: &Runtime) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// The game thread's only job here: say which phase it is in.
    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        let mut found = UNKNOWN;
        for (i, (obj, _, _)) in PHASES.iter().enumerate() {
            if instance::find_singleton(rt.base, obj).is_some() {
                found = i as u8;
                break;
            }
        }
        PHASE.store(found, Ordering::Relaxed);
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    base: usize,
    sym: Symbolizer,
    interval: Duration,
    top: usize,
    stalls: bool,
    stop_after: Duration,
    trace: String,
    stop: Arc<AtomicBool>,
) {
    // Wait for the frame hook to name the game's thread. It arms within a second of the
    // game reaching its message loop; a minute is generous and bounded so a launch that
    // never gets there leaves a thread that exits rather than one that spins forever.
    let mut waited = Duration::ZERO;
    while hook::game_thread() == 0 {
        if stop.load(Ordering::Relaxed) || waited >= Duration::from_secs(60) {
            logln!("[profiler] the game's thread never appeared; not sampling");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
        waited += Duration::from_millis(20);
    }
    let thread = hook::game_thread() as u32;
    let Some(target) = Target::open(thread) else {
        logln!("[profiler] could not open the game's thread; not sampling");
        return;
    };
    logln!("[profiler] sampling thread {thread}");
    let Some(unwind) = sample::virtual_unwind() else {
        logln!("[profiler] RtlVirtualUnwind is unavailable; not sampling");
        return;
    };
    // The unwinder *is* the module list: walking a frame needs to know which module the
    // return address belongs to, so the same snapshot serves both.
    let mut mods: Unwinder = Modules::snapshot();
    let mut mods_at = Instant::now();
    let mut buckets: HashMap<u8, Bucket> = HashMap::new();
    let mut failures = 0u32;
    let mut frames = [0usize; sample::MAX_FRAMES];
    let mut seen: HashSet<u32> = HashSet::new();
    // Emitted on a timer as well as at the end, because the end may never come: a
    // measurement run kills the game, and a report written only after the sampling loop
    // exits is a report nobody ever sees. Each one covers everything so far, so the
    // last one in the log is the complete answer and the earlier ones are supersed
    let mut reported_at = Instant::now();
    let mut traced = 0u32;
    let csv = run_file();
    if let Some(p) = &csv {
        logln!("[profiler] this run's samples go to {}", p.display());
    }

    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        // Stop of our own accord. Suspending the game's thread a thousand times a
        // second is not a state to leave a game in, and "remember to switch it off" is
        // not a safety mechanism -- it failed once and the game raised a font error.
        if !stop_after.is_zero() && started.elapsed() >= stop_after {
            logln!(
                "[profiler] stopping after {}s, as configured; the game is left alone \
                 from here",
                stop_after.as_secs()
            );
            break;
        }
        std::thread::sleep(interval);

        // A stale module list put 37% of one profile into `<unmapped>`.
        if mods_at.elapsed() >= Duration::from_secs(3) {
            mods = Modules::snapshot();
            mods_at = Instant::now();
        }

        let n = match sample::capture(&target, &mods, unwind, &mut frames) {
            Ok(n) if n > 0 => n,
            Ok(_) => continue,
            Err(()) => {
                failures += 1;
                if failures > 200 {
                    logln!("[profiler] the game's thread stopped answering; done");
                    break;
                }
                continue;
            }
        };
        failures = 0;

        // The stack decides; the polled registry is only the fallback.
        //
        // Reporting is *not* done on a transition. The phase flips constantly -- a
        // sample taken between two GML events has no phase on its stack and falls back
        // to the registry -- so a report per transition emitted the same bucket four
        // times over, each cumulative on the last. The buckets accumulate correctly;
        // they are printed once, at the end.
        let phase = phase_of_stack(&sym, base, &frames[..n])
            .unwrap_or_else(|| PHASE.load(Ordering::Relaxed));

        let now = Instant::now();
        let b = buckets.entry(phase).or_default();
        b.note(now);
        b.samples += 1;
        if n == sample::MAX_FRAMES {
            b.truncated += 1;
        }
        // Only once the game is actually drawing. Before the first pump everything is
        // "overdue" by definition, which reported 99.9% of a startup phase as stalled
        // and said nothing at all.
        if stalls
            && hook::frames() > 0
            && hook::since_last_pump().is_some_and(|d| d.as_millis() as u64 >= STALL_MS)
        {
            b.stalled += 1;
        }

        // Leaf: where the CPU actually is.
        let leaf = sym.resolve(base, frames[0]);

        // A few literal stacks for whatever is being traced. Aggregates cannot name a
        // caller; this can.
        if !trace.is_empty() && traced < 3 {
            let leaf_name = known_name(leaf.func as usize)
                .map(String::from)
                .unwrap_or_else(|| sym.name_of(leaf.func));
            if leaf_name.contains(&trace) {
                traced += 1;
                logln!("[profiler] stack {traced} with leaf {leaf_name:?}, innermost first:");
                for (depth, &pc) in frames.iter().take(n).enumerate() {
                    let s = sym.resolve(base, pc);
                    let name = if s.inside {
                        known_name(s.func as usize)
                            .map(String::from)
                            .unwrap_or_else(|| sym.name_of(s.func))
                    } else {
                        mods.name_of(pc)
                    };
                    logln!("[profiler]   #{depth:<2} {name}");
                }
            }
        }
        if leaf.inside {
            *b.self_.entry(leaf.func).or_insert(0) += 1;
        } else {
            *b.outside.entry(mods.name_of(frames[0])).or_insert(0) += 1;
        }

        // Inclusive, and the innermost named GML frame.
        seen.clear();
        let mut responsible = None;
        for &pc in frames.iter().take(n) {
            let site = sym.resolve(base, pc);
            if !site.inside {
                continue;
            }
            if seen.insert(site.func) {
                *b.incl.entry(site.func).or_insert(0) += 1;
            }
            if responsible.is_none() && is_gml(&sym, site.func) {
                responsible = Some(site.func);
            }
        }
        match responsible {
            Some(f) => *b.responsible.entry(f).or_insert(0) += 1,
            None => b.no_gml += 1,
        }

        if reported_at.elapsed() >= Duration::from_secs(15) {
            emit(&sym, &mods, base, &buckets, top, stalls, csv.as_deref());
            reported_at = Instant::now();
        }
    }

    emit(&sym, &mods, base, &buckets, top, stalls, csv.as_deref());
}

/// Every phase seen so far, longest first, and the CSV beside it.
///
/// Longest first because that is the order the question is asked in: the phase that
/// took the most wall-clock is the one worth reading.
fn emit(
    sym: &Symbolizer,
    mods: &Modules,
    base: usize,
    buckets: &HashMap<u8, Bucket>,
    top: usize,
    stalls: bool,
    csv: Option<&std::path::Path>,
) {
    let mut order: Vec<(&u8, &Bucket)> = buckets.iter().collect();
    order.sort_unstable_by(|a, b| b.1.seconds().total_cmp(&a.1.seconds()));
    for (phase, b) in order {
        report(sym, mods, base, *phase, b, top, stalls);
    }
    if let Some(path) = csv {
        write_csv(sym, base, buckets, path);
    }
}

/// Whether a function is compiled GML, as opposed to a runtime routine or an unnamed
/// `.pdata` entry.
///
/// The first version of this asked only whether the name was not `sub_...`, which
/// accepted the runtime's own named helpers. `call_builtin_by_index` then took 80.6% of
/// the responsible column: every builtin call in the game goes through the dispatcher,
/// so it is always the innermost named frame and never the answer.
fn is_gml(sym: &Symbolizer, func: u32) -> bool {
    sym.is_gml(func)
}

fn report(
    sym: &Symbolizer,
    mods: &Modules,
    _base: usize,
    phase: u8,
    b: &Bucket,
    top: usize,
    stalls: bool,
) {
    if b.samples == 0 {
        return;
    }
    let n = b.samples as f64;
    logln!(
        "[profiler] ---- {} ({:.1}s, {} samples) ----",
        phase_name(phase),
        b.seconds(),
        b.samples
    );

    // Say whether the responsible column can be believed, before showing it.
    logln!(
        "[profiler] {:.1}% of samples had no GML function anywhere on the stack{}",
        b.no_gml as f64 * 100.0 / n,
        if b.truncated > 0 {
            format!("; {} stack(s) hit the frame ceiling", b.truncated)
        } else {
            String::new()
        }
    );
    if stalls && b.stalled > 0 {
        logln!(
            "[profiler] {:.1}% of samples were taken while the game was over {STALL_MS}ms \
             overdue to draw",
            b.stalled as f64 * 100.0 / n
        );
    }

    let mut rows: Vec<(u32, u64)> = b.responsible.iter().map(|(&f, &c)| (f, c)).collect();
    rows.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    if !rows.is_empty() {
        logln!("[profiler]  share  responsible GML function");
        for (func, count) in rows.iter().take(top) {
            logln!(
                "[profiler] {:5.1}%  {}",
                *count as f64 * 100.0 / n,
                sym.name_of(*func)
            );
        }
    }

    // Everything the game's own code did not account for, kept apart on purpose.
    let mut out: Vec<(String, u64)> = b.outside.iter().map(|(m, &c)| (m.clone(), c)).collect();
    let mut engine: Vec<(String, u64)> = b
        .self_
        .iter()
        .filter(|(&f, _)| !is_gml(sym, f))
        .map(|(&f, &c)| {
            (known_name(f as usize).map(String::from).unwrap_or_else(|| sym.name_of(f)), c)
        })
        .collect();
    engine.append(&mut out);
    engine.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    if !engine.is_empty() {
        logln!("[profiler]  share  engine, runtime and OS (self time)");
        for (what, count) in engine.iter().take(top) {
            logln!("[profiler] {:5.1}%  {what}", *count as f64 * 100.0 / n);
        }
    }
    let _ = mods;
}

/// A file of its own for this run, in `profiles/`.
///
/// One run says very little: the same configuration profiled twice put a codec at a
/// third of a phase and then did not show it at all. Runs are therefore kept rather than
/// overwritten, and `knowledge-base/tools/profiles.py` reports the median and the spread
/// across all of them. A number from a single run is a hypothesis.
fn run_file() -> Option<std::path::PathBuf> {
    let dir = home::dir()?.join("profiles");
    std::fs::create_dir_all(&dir).ok()?;
    let next = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix("run-"))
                .and_then(|n| n.strip_suffix(".csv"))
                .and_then(|n| n.parse::<u32>().ok())
        })
        .max()
        .unwrap_or(0)
        + 1;
    Some(dir.join(format!("run-{next:04}.csv")))
}

/// One row per `(phase, kind, name, samples)`, so an analysis is a query rather than a
/// scrape. Every investigation this session has meant grepping a log.
fn write_csv(sym: &Symbolizer, _base: usize, buckets: &HashMap<u8, Bucket>, path: &std::path::Path) {
    let mut out = String::from("phase,kind,name,samples,phase_samples\n");
    let esc = |s: String| s.replace('"', "'");
    for (phase, b) in buckets {
        let p = phase_name(*phase);
        for (func, count) in &b.responsible {
            out.push_str(&format!(
                "{p},responsible,\"{}\",{count},{}\n",
                esc(sym.name_of(*func)),
                b.samples
            ));
        }
        for (func, count) in &b.self_ {
            // The same naming the log uses. Without this the CSV reported raw addresses
            // while the log said "ogg/vorbis decode", and reading the two side by side
            // made a stable finding look like it had vanished.
            let name = known_name(*func as usize)
                .map(String::from)
                .unwrap_or_else(|| sym.name_of(*func));
            out.push_str(&format!("{p},self,\"{}\",{count},{}\n", esc(name), b.samples));
        }
        for (func, count) in &b.incl {
            out.push_str(&format!(
                "{p},inclusive,\"{}\",{count},{}\n",
                esc(sym.name_of(*func)),
                b.samples
            ));
        }
        for (name, count) in &b.outside {
            out.push_str(&format!("{p},module,\"{}\",{count},{}\n", esc(name.clone()), b.samples));
        }
        out.push_str(&format!("{p},no_gml,\"\",{},{}\n", b.no_gml, b.samples));
        out.push_str(&format!("{p},truncated,\"\",{},{}\n", b.truncated, b.samples));
    }
    match std::fs::write(&path, out) {
        Ok(()) => findln!("[profiler] wrote {}", path.display()),
        Err(e) => logln!("[profiler] could not write {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The phase list is ordered most-specific-first, because during a run more than one
    /// of these controllers exists and the innermost is the answer.
    #[test]
    fn phases_are_ordered_most_specific_first() {
        assert_eq!(PHASES[0].0, "obj_gameplay_controller");
        assert_eq!(PHASES[PHASES.len() - 1].0, "obj_init");
        assert_eq!(phase_name(UNKNOWN), "(no phase on the stack)");
        // every phase must have a frame prefix, or it can only ever be found by the
        // registry poll -- which is what mislabelled 36 seconds of obj_init
        assert!(PHASES.iter().all(|(_, _, prefix)| !prefix.is_empty()));
    }

    /// The table is the whole point: an address identified once must never need
    /// identifying again.
    #[test]
    fn known_ranges_name_what_was_identified_by_hand() {
        // the two functions that were 45.7% of a startup profile, and their parent
        assert_eq!(known_name(0x1c9fb28), Some("texture page decompress"));
        assert_eq!(known_name(0x1c9fd30), Some("texture page decompress"));
        assert_eq!(known_name(0x1ca3ab0), Some("texture page checksum"));
        // an address inside each range, not just its first byte
        assert_eq!(known_name(0x1ca0000), Some("texture page decompress"));
        assert_eq!(known_name(0x1ca4000), Some("texture page checksum"));
        // and the gap between them belongs to neither
        assert_eq!(known_name(0x1ca3000), None);
        assert_eq!(known_name(0x1000), None);
    }

    #[test]
    fn config_is_validated() {
        let mut p = Profiler::default();
        let cfg = momomod_kit::config::Config::parse("[feature.profiler]\ninterval_ms = 0\n");
        assert!(p.configure(&cfg.section("profiler")).is_err());
        let cfg = momomod_kit::config::Config::parse("[feature.profiler]\ntop = 60\nstalls = false\n");
        assert!(p.configure(&cfg.section("profiler")).is_ok());
        assert_eq!(p.top, 60);
        assert!(!p.stalls);
    }
}
