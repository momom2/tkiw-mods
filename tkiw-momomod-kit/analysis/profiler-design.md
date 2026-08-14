# Profiler, second attempt: design

The first one produced numbers nobody could act on. This is what it should have been,
written from the two concrete ways it failed rather than from first principles.

## How the old one failed

**It reported addresses, not answers.** The two entries at the top of the startup
profile were `sub_1c9fd30` (28.5%) and `sub_1ca3ab0` (17.2%) — 45.7% of startup, named
by nothing. Working out that they were one decompression subsystem took a separate manual
investigation: disassembling both, recognising a CRC-32 inner loop, matching the
polynomial table, and scanning for their shared caller. **A profiler whose output
requires that much follow-up has not profiled anything.**

**It ranked by self time and showed 25 rows.** The game's C++ runtime, `ntdll`,
`win32u`, `d3d11` and the codec filled every slot. Not one GML function appeared, so
the question actually being asked — *which of the 16 library builders costs what* — was
invisible in a profile that had the samples to answer it.

**Its reports were keyed to wall-clock windows.** "30 seconds from when the feature
started" straddles `obj_init` and the splash, so a shift in cost between phases reads as
a change in the mix, and the phase where a cost lives cannot be recovered.

## The design

### 1. Sampling stays as it is

It worked: 17,663 samples over 30s with zero failures. `SuspendThread` +
`GetThreadContext` + `RtlVirtualUnwind`, supplying the `RUNTIME_FUNCTION` ourselves so
`RtlLookupFunctionEntry` is never called under the loader lock. Keep the module-list
refresh too — enumerating once put 37% of a menu profile into `<unmapped>`.

### 2. Three views of the same samples, and the third is the new one

| view | question it answers |
|---|---|
| **self** (leaf) | where is the CPU literally executing |
| **inclusive** (frame anywhere on the stack) | what is the expensive thing, whoever runs it |
| **responsible** | *which GML function asked for this work* |

**Responsible frame** = the innermost frame on the stack that resolves to a named GML
function. A sample deep inside the allocator, the codec or `ntdll` is charged to the GML
that caused it. That is what turns "28.5% in `sub_1c9fd30`" into "28.5% under
`obj_splash_screen_Draw_0`" — an answer with somewhere to go.

When no frame on the stack is named GML, the sample is charged to its **module** instead
and reported in a separate table, so engine and OS work can never be mistaken for game
code. That separation is the whole reason the number is trustworthy.

### 3. A small table for subsystems worth naming

Some leaves have no GML ancestor at all — an audio callback is driven by the engine, not
by the game. For those, a table of `(range, name)`:

```rust
const KNOWN: &[(usize, usize, &str)] = &[
    (0x1c9fb28, 0x1ca269c, "texture page decompress"),
    // one line per thing anybody ever identifies by hand
];
```

Data, not code, so the next person who identifies an unnamed hot spot spends one line
making sure nobody has to identify it again. Seeded with what this session cost a day to
learn.

### 4. Reports keyed to phase, not to the clock

The kit already knows the phase — `obj_init`, `obj_splash_screen`, `obj_main_menu`,
`obj_gameplay_controller`. The main thread writes the current phase into an atomic when
it changes; the sampler reads that atomic per sample. **The sampler never touches the
object registry**, which costs ~2ms a lookup and would otherwise dwarf the sample it is
labelling.

A report is emitted when a phase ends, covering exactly that phase, plus one at the end
of the session. Startup then reads as:

```text
obj_init          4.1s   1642 samples
  responsible                       self   incl
  gml_Script_unit_library           ...    ...
  gml_Script_improvement_library    ...    ...
  ...
  engine and OS                     ...    ...
    texture page decompress         ...    ...
    d3d11.dll                       ...    ...
```

### 5. Output that is not the log

Every analysis this session has meant grepping a log with PowerShell. The profiler
should write a **CSV beside the log** — one row per `(phase, responsible, leaf, count)` —
so a question like "which library builder dominates init" is a one-line query rather
than a scrape. The log keeps the human-readable summary; the CSV is what gets analysed.

### 6. Fewer knobs

| key | why it stays |
|---|---|
| `interval_ms` | the one real trade: resolution against cost |
| `top` | an analysis pass needs the rows a reading pass does not |
| `stalls` | on/off for the second lens below |

`report_every_s` goes: phases replace it. The old `stall_threshold_ms` becomes a fixed
20ms inside the stall lens.

### 7. Keep the stall lens, but as a lens

Profiling only the samples taken while the game is overdue to pump was what found the
popup stutter — an average profile of a hitching game is dominated by the frames that
were fine. It stays, reported only when there were stalls, so it never pads a clean
report.

## What this does not do

- **No call-tree output.** Inclusive-per-frame plus responsible-frame answers the
  questions we have had, and a tree is a large amount of machinery and reading for a
  question nobody has asked yet.
- **No cross-session aggregation.** One launch, one CSV. Comparing runs is `timeit.py`'s
  job and it already does the statistics.
- **No automatic naming of unknown code.** The table is hand-fed on purpose; a profiler
  that guessed at names would be the previous profiler's mistake in a new shape.

## Risk

The responsible-frame rollup is the part most likely to be wrong. If the GML-to-native
boundary does not unwind cleanly — YYC compiles GML to native, so a "GML function" is a
real frame and should — then responsible attribution silently charges work to the wrong
place, which is worse than not attributing it.

**Mitigation:** report, per phase, the share of samples that found *no* named GML frame.
If that number is large the rollup is not working, and it says so on its own face
instead of producing a confident wrong table.

---

## What shipping it taught

Three things the design did not anticipate, all found by using it:

* **A name in the table can be wrong.** These two were called "ogg/vorbis decode" for a
  day on circumstantial evidence -- the right CRC polynomial, and `OggS`/`vorbis` present
  somewhere in the image. A captured stack showed they run under `texture_prefetch` and
  have nothing to do with audio. The table makes an identification permanent, which is
  its value and its hazard.

* **Aggregates cannot name a caller.** Inferring one from inclusive percentages produced
  a confident wrong answer twice. `trace = <substring>` now prints whole stacks for
  samples whose innermost frame matches, and one stack settled what a week of
  percentages could not.

* **The profiler can crash the game.** Suspending the game thread a thousand times a
  second makes a load callback fire twice; see `notes-for-claude/pitfalls.md`. It now
  stops itself after `stop_after_s`.
