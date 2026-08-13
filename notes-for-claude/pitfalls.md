# Pitfalls

Every one of these actually happened. Several crashed the game on a real save.

## Address arithmetic

**`base + rva`, never `rva + slide`.** Computing the ASLR slide and adding it to
an RVA drops the `0x140000000` image base, and every derived address lands in
unmapped memory. This crashed the game twice.

The regression test that would have caught it must assert **absolute bounds** —
that the computed address lies inside the loaded module. A test that only checks
addresses *relative* to each other passes happily with the bug present.

## Performance, on the game's own thread

You run inside the player's frame. A slow poll is a stutter they feel.

**What made the game unplayable once:** a `VirtualQuery` per read, plus a full
1,750-entry object-registry walk per lookup, times thirteen objects, twice a
second — about 68,000 syscalls per poll. Fixed with a per-frame validated-region
cache and a cached object registry.

**What was still costing an unreasonable amount afterwards:**

* The region cache was a linear scan of 96 slots on *every read*. Add a
  one-entry front cache of the last region hit; reads are extremely local, so
  nearly every check becomes one comparison.
* Counting instances by building a `Vec` and taking its length — a heap
  allocation per question, a dozen or more per frame, for an integer.
* Two separate passes counting the same card objects in the same frame.
* Opening and closing the log file for every line.

**A walk whose misses cannot be cached must be rate-limited.** The object registry
is keyed by object *index*, so finding one by *name* means walking every bucket —
and the walk is O(mask), where the mask is not knowable from the executable and is
only sanity-capped. A hit can be cached forever; a **miss cannot**, because an
object that does not exist yet may exist a second later. So every miss re-walks. A
diagnostic polling four object names during startup — while the registry was open but
still sparse, so all four missed — was measured at **three seconds per poll**, which
is enough to stop the game, and did. The fix is one shared snapshot refreshed at most
every couple of seconds, not a cleverer cache.

**A self-limiter gated on "was the game running normally" is blind to exactly the
feature it needs to catch.** Judging a feature's cost only on frames where the game
pumped at a normal rate is right for a *share-of-frame* verdict — a feature is not to
blame for a frame the game spent thirty-nine seconds inside. But a feature slow
enough to stop the game **causes** the abnormal pump rate, and so hides behind the
very condition meant to be fair to it. One averaging 67 ms per call, worst 3.1
seconds, ran for twenty-five seconds before the loader noticed, and the game never
reached its main menu.

Keep both judgements: share-of-frame gated on a normal pump rate, **and** an absolute
ceiling judged unconditionally. Deciding that no feature may hold the game's thread
for a quarter of a second needs no knowledge of what the game was doing.

**Self-limit.** Measure your own poll and back off if it blows a budget. But be
careful what you disable when it does: a kill switch written for a *diagnostic*
sweep, placed at the top of the frame hook, silently killed the actual feature
too — and the mod's hotkey went on logging "ACTING" while no code remained to
act. Gate the diagnostics; let the feature degrade rather than die. And time
each part separately, or the cheap one gets blamed for the expensive one's cost.

## Startup ordering

**Your startup thread runs before the game's entry point, so the GML runtime does
not exist yet.** A proxy DLL is mapped by the loader, `DllMain` fires, and the
thread it spawns is running within a millisecond — while the GameMaker runner has
not yet created the global-variable container, the object registry, or the `ds_*`
tables. Reading the container that early gives a **null pointer**, and a startup
sequence that treats that as "this is not the build I was written for" disables the
mod on a perfectly good game.

Split startup accordingly:

| resolvable before the entry point | only on the game's thread, later |
|---|---|
| the module base, sections, `.text` bounds | the global-variable container |
| the `gml_*` function table (read from the file on disk) | the object registry and instance lookup |
| variable-id *slots* (addresses) | variable-id *values* (`0xFFFFFFFF` until resolved) |
| baked code signatures — `.text` is mapped already | `ds_list` / `ds_map` tables |

Resolve the second column **lazily, retrying until it succeeds**, and treat `None`
as "not yet" rather than as a failure. The auto-picker got this right by accident of
structure — it resolved globals inside its frame hook — and the momomod kit got it
wrong by folding that into one tidy `resolve()`, which disabled the whole kit
0.37 s into the first launch it ever ran.

**A crash-loop breadcrumb must be cleared when the mod stands down cleanly.** The
breadcrumb exists to catch a probe that *kills the process*; a probe that returns an
error leaves nothing dangerous running, so keeping the marker holds the next launch
back and tells the player their last session crashed when it did not. One clean
failure becomes two, and the second one is a lie. Clear it on any orderly
stand-down; keep it only while risky code is still live.

## Reading live objects

**Both ends of an object's life are unsafe.** Reading a card while the game is
still building it, and reading one while the game is tearing it down, are the
same mistake. It is easy to guard one and forget the other — the picker waited
for readiness on the way in, while the diagnostic sweep read half-built cards on
the way out and printed them under the word "building" without anyone noticing
what that meant.

## Calling

See [calling-into-the-game.md](calling-into-the-game.md) for the conventions.
The specific crashes:

* **Null `self`.** `script_execute` invoked `button_pressed_action` with null
  self/other; the method's first instructions load `self` and dereference it.
* **Hammering a button** while the thing it acts on is still materialising.
  Settle windows on both the press and the open.

## Instrumentation that lies

**A dedupe guard that suppressed the action, not just the log line.** The
"unchanged decision" check returned early *before* the press, so switching the
mod off and on again left it silently inert. Suppress logging; never suppress
the work.

**`?` in a diagnostic path.** A price reader built out of `?` operators returned
a bare `None` from six different places, so the message said "cannot read" and
threw away which of the six it was. A whole session produced no usable
information. Return a reason from every exit in anything whose job is to explain
a failure.

**No log on the success path.** The same reader, once fixed, reported only
failures — so a price that read perfectly but was then declined on budget looked
identical in the log to one that could not be read at all.

**A crash reporter that allocates.** Produced nothing on two separate crashes.
See [injection.md](injection.md).

**Diagnostics that depend on a verbose mode.** The one fact the crash reporter
cannot do without — what was under way — was recorded only when tracing was on,
and tracing was off for the session that finally faulted. Record the phase
unconditionally into a fixed buffer; keep the *flood* of trace lines opt-in.

**Log rotation eating the evidence.** One-shot findings all happen in the first
minute; the crash happens minutes later. Carry findings across a rotation.

## Analysis

**The builtin table does not name builtin calls.** An annotator built on the
2,767 `Function_Add` entries reports that `load_raw_data` calls nothing but
RValue housekeeping — because compiled GML calls the inner implementations, not
the registered wrappers, and the wrappers have zero call sites in the entire
`.text`. Half an hour went into "why does this function that obviously reads a
file appear to call nothing", and the honest answer is that the question cannot
be asked that way. See [runtime-internals.md](runtime-internals.md); read a
function's *variables* instead, or measure it in the live game.

## Content

**Do not infer content rules from small samples.** A conclusion drawn from two
Troops Training choices ("a card fixes the stat") was wrong, and the decode had
been right all along. The user's screenshot settled it in one message.

**Do not trust the goblin sprites.** They do not match the choices; it is a
known game bug. A cross-check built against them would contradict a correct
decode. This warning stopped a real mistake before it was made.

**Derive option pools from the game's own fields**, not by hand. Legendary lists
built by eye contained ordinary items; improvement lists contained every
building in the game. `tier`, `excluded_from_drop_pool`,
`IMPROVEMENTS_BY_CATEGORY` and `IMPROVEMENTS_BY_TIER` are all there for the
asking, and they are right by construction.

**Not every name has a variable id.** Data-driven keys like `coin` do not appear
literally in the game's code, so `var_id` returns nothing and any reader built on
it silently reports "absent" forever. This made the player's denarii read as zero
for the entire life of a mod without anyone noticing, because nothing had yet
depended on the number.

## Process

**Verify a patch actually applied.** Scripted `str.replace` edits that silently
match nothing produced "fixed" code that was not, several times. Assert the
match, then check the result.

**Regenerating a config must not discard the user's edits.** Doing so once left
a carefully-tuned config inert and the mod looking broken.

**Ask what a message means before acting on it.** "It crashed" and "it froze"
are different diagnoses. So are "it stopped working" and "it disabled itself" —
the latter looked exactly like the former until the log's first line was read.


## Investigation

**The save file is plain JSON and is often faster than disassembly.**
`%LOCALAPPDATA%\The_king_is_watching_steam\Release\run_data.dat` parses with
`json.JSONDecoder().raw_decode()` — there is one trailing NUL byte, so plain `json.load`
fails with "Extra data". It contains every unit with every stat modifier resolved,
`training_hp/damage/atk_speed` per class, equipped upgrades, artifacts and advisors.

An hour of chasing `Stats_mods.calculate_stat` through YYC output produced nothing
readable; the same question was answered in five minutes by reading the save, because
the save stores the *inputs* to the formula with their names intact. Reach for it before
the disassembler whenever the question is "what values does the game actually have".

**Check the player's own configuration before believing a bug report.** "Gryphons do not
benefit from troop upgrades" was real, reproducible, and not a bug: Griffins are
rider+flying, and the reporter's training was ~130 in ranged/arcane and ~0.15 in
everything else — because the auto-picker had been configured to pick exactly those two.
The observation was correct and the diagnosis was in the config, not the game.

## The profiler can crash the game, and did

Sampling suspends and resumes the game's thread at the configured interval. At the
default 1ms that is about a thousand stop/starts a second, and it is enough to break
the game's font loading:

```
Font "fnt_hr_semibold_12_outlined_2px" already exists
gml_Script_scribble_font_duplicate
gml_Script_generate_fonts
gml_Script_anon@169@gml_Object_obj_init_Alarm_0
gml_Object_obj_font_tex_control_Step_0
```

`obj_font_tex_control_Step_0` is a state machine polling for a texture group to finish
loading; when it sees the group loaded it sets `state` and invokes `on_loaded_callback`,
which is what runs `generate_fonts`. Slow the main thread down enough and that callback
runs twice. `generate_fonts` is 62 KB of straight-line code with no guard against a
second entry, so Scribble raises on the duplicate.

The suspend/resume discipline in `sample.rs` is not the problem -- `ResumeThread` always
runs and the only early return is when the suspend never took. It is the perturbation
itself.

Two consequences, both now in the code:

* The profiler stops itself after `stop_after_s` (120 by default). "Remember to switch
  it off" is not a safety mechanism; it failed, on a player's own machine.
* `measure-startup.py` restores the config and reinstalls at the end of every run. It
  used to leave the measuring config in place, which is how a player came to launch a
  game with everything switched off and a profiler suspending their thread.

**Never leave a measurement configuration installed.** The player's next launch is not
your measurement.
