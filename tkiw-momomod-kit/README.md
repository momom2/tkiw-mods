# TKIW's momomod Kit

Small fixes, conveniences and optimisations for *The King is Watching*, in one
mod. Every change is independent and separately switchable, so you can take the
ones you want and leave the rest.

Built for the game as of **2026-08-10**. Ask momom2 on Hypnohead's Discord server
for support if needed.

## Quickstart

Needs Python 3.8+ to install. The mod itself has no dependencies.

```bash
python install.py
```

Launch the game once. The kit writes a `config/` folder here, **one file per mod**:

```text
config/momomod.ini      the kit itself: which mods to load, and a mirror of their settings
config/qol.ini          quality of life
config/bugfixes.ini     bug fixes
config/reward-picker.ini  the reward auto-picker's own rules
config/diagnostics.ini  measurement tools
```

The kit is a manager — on its own it changes nothing about the game. Each mod's file
is the same document that mod would read installed on its own, so settings travel
with you. `config/momomod.ini` also **mirrors** every mod's settings, refreshed from
their files each time the kit reads them, so it is a place to see and set everything
from. Mirror lines are commented out; uncomment one to override what that mod's file
says.

Then either

```bash
python configure.py
```

for a window — one tab per mod, a checkbox per switch, Apply — or edit the files in
any text editor. Both take effect while the game runs, no restart. Press
**Ctrl+Alt+M** in game to force a re-read and have the log report what is on.

**Anything that changes the rules starts switched off.** Upgrading from a version
that kept everything in one `momomod.ini`? That file is migrated into the new ones
on first launch, keeping the settings you had, and left beside them as
`momomod.ini.migrated`.

## Features

**`qol.ini`** — quality of life:

| feature | what it does | default |
|---|---|---|
| `fast_boot` | Skips the texture prefetch the game does before the menu. **Its saving is unproven**: four launches each way gave medians 48.7s with and 52.4s without, against 18.8s of run-to-run spread. Left on because the median is the right way round. | **on** |
| `popup_stutter_fix` | **Removes the in-run stutter** caused by the floating resource-gain numbers rebuilding their text every frame. | **on** |

**`bugfixes.ini`** — restores behaviour the game describes but does not do:

| feature | what it does | default |
|---|---|---|
| `morale_fix` | **Resuming a reign keeps your morale.** Without it morale drops to 0 on load and creeps back over minutes, with King effects reading the crept value. | **on** |
| `fortifications_cap` | Makes **Fortifications stop at the 100 max castle HP it says it grants** — as shipped it never stops, so a Brick Factory raises your castle's maximum forever. Caps growth from when you switch it on; HP already gained is left alone. | off |

`morale_fix` stands down automatically if the standalone `tkiw-morale-fix` has patched
the executable on disk — the bytes it expects are no longer there, and it says so in the
log. Run that mod's `unpatch.py` to use this one instead.

**`reward-picker.ini`** — the reward auto-picker. This file is written by the picker
itself from the live game's option lists, so the kit never generates, overwrites or
parses it; switch the mod on or off under `[mods]`. If an older standalone picker
`version.dll` is still installed, this one stands down and says so — run that mod's
`uninstall.py` to hand over.

**`diagnostics.ini`** — measurement tools, which change nothing about the game:

| feature | what it does | default |
|---|---|---|
| `timeline` | Logs how long each phase of a launch takes and any frame hitches. | off |
| `profiler` | Samples the game's thread and reports which functions the time goes to. Invasive; for measurement sessions only. | off |
| `dump_libraries` | Writes the game's own content libraries to JSON, once, then switches itself off. | off |

More are coming; the diagnostics came first on purpose, and `fast_boot` exists because
of what they measured — see [What is being worked on](#what-is-being-worked-on).

### `fast_boot`

Measured on the 2026-08-10 build, two consecutive launches on the same machine:

| | with `fast_boot` | without | |
|---|---|---|---|
| the long block before init | **7.9 s** | 26.3 s | −18.4 s |
| splash screens | 16.1 s | 8.0 s | +8.1 s |
| **to the main menu** | **29.4 s** | 39.1 s | **−9.7 s** |

Profiling put 59% of that block inside `texture_prefetch`, called from the game's init
event — CPU work decoding texture pages, not disk I/O. In GameMaker `texture_prefetch`
is only a *hint*: pages it skips still load automatically the first time something
draws from them. So this trades a long guaranteed wait for short occasional loads.

**Read the splash row honestly**: about eight of the eighteen seconds saved come back
during the splash screens, as pages load while the logos play. The net is a real ~25%
cut, not eighteen free seconds.

The trade-off is a possible brief hitch the first time something new appears on screen
in a session. If you would rather not, set `enabled = false`. A hitch that *never*
settles down is worth reporting — that would mean something is reloading a page rather
than loading it once.

`restore_on = main_menu` (the default) puts `texture_prefetch` back once the menu
exists, so anything the game prefetches later behaves normally. `restore_on = never`
leaves it skipped all session: slightly faster still, and less predictable.

It is applied by patching the game's code in memory — never on disk — and only in the
one window where that is provably safe: the kit's startup thread runs *before* the
game's entry point, so no game code has executed. If the game has already started, the
feature refuses to apply itself and says so.

### `popup_stutter_fix`

The stutter that gets worse as a run goes on is not the units — it is the floating
"+N resource" numbers. Their Draw event bakes the fade value into the text string, and
in Scribble the string is the cache key, so every popup rebuilds its entire text model
every frame: parse, typeset, allocate, discard.

This rounds the fade to ten steps so the cache works. Measured over a real run, before
and after:

| time into the run | before | after |
|---|---|---|
| ~30 s | 17.8% | 9.6% |
| ~90 s | 22.1% | 13.3% |
| ~150 s | 34.3% | 8.8% |
| ~210 s | 43.3% | 11.6% |

(share of time lost to stalls over 50 ms)

More telling than the numbers: **the climb stops**. Unmodified, the cost rises with
production for as long as the run lasts. With the fix it stays flat, and the memory
allocator — previously 48% of all stall time — disappears from the profile entirely.
What is left is the graphics stack waiting for the GPU, which no mod can help.

The cost is that the text's fade becomes slightly stepped. `steps = 10` is a fade you
have to look for; `steps = 1` means the text does not fade at all and is cheapest.

Like `fast_boot`, it works by patching the game's code in memory, only in a window
where nothing can be executing it. **This one has a proper fix in the game's own
source** — one line — which is written up for the developers in
[`analysis/for-the-developers.md`](analysis/for-the-developers.md).

### `timeline`

Writes a phase-by-phase account of a launch to `momomod.log`:

```
[timeline]    4.812s  + obj_splash_screen  (the splash screens)
[timeline]   11.004s  - obj_splash_screen  (lasted 6.192s)
[timeline]   11.310s  + obj_main_menu      (the main menu)
[timeline] 3 hitch(es) in the last 1.0s, worst 91.4ms
```

Turn it on if you are reporting that something is slow — the log is far more
useful than a description. Turn it off again afterwards; it is not doing you any
good the rest of the time.

It reads the game and nothing else. It cannot change how long anything takes.

### `profiler`

Answers the question `timeline` cannot: not *when* the game is slow, but *what is
running*. Its own thread samples the game's thread a thousand times a second and
reports a flat profile of named functions:

```
[profiler]  self   total  function
[profiler]  31.4%   38.2%  obj_unit_Step_0
[profiler]  18.9%   61.0%  member_get
[profiler]   9.2%    9.2%  <outside the game>
```

- **self** — the innermost frame: where instructions are actually executing.
- **total** — anywhere in the stack: where the *cost* belongs.

The difference is the useful part. A runtime helper called from thousands of places
will have a high *self* and tell you nothing; the caller with the high *total* is
the one worth changing. The report also lists callers that hold a lot of time
without spending any themselves, which is usually where a fix goes.

**`<outside the game>` means waiting** — on the GPU, or in a system call. Those
samples carry no caller information, because a stack cannot be walked out of a
frame that has no unwind data. If they dominate your profile, the bottleneck is not
the game's own logic.

**Turn this on for a measurement session and off again afterwards.** It suspends
the game's thread thousands of times a minute. Each suspend lasts microseconds and
the mechanics are careful about it, but it is the most invasive thing in the kit and
it does nothing for you the rest of the time.

## Configuring

`python configure.py`, or `config/` in this folder by hand. The kit's own file says
which mods to load; each mod's file has one section per feature.

```ini
# config/momomod.ini
[kit]
trace = false           # log each feature call as it begins; verbose

[mods]
qol = true              # a mod switched off here is not configured, not checked
bugfixes = true         # and not started, and its own file is left untouched
reward-picker = true
diagnostics = true

# ===== mirror of the mods' own files =====
# [diagnostics.feature.timeline]
# enabled = true        # uncomment a line to override the mod's own file
# interval_ms = 500
```

```ini
# config/diagnostics.ini
[feature.timeline]
enabled = true
interval_ms = 100
```

The file is forgiving on purpose:

- A feature the file does not mention uses its default. Nothing is silently
  applied that the file argues against.
- A section or key this build does not know is **reported in the log and
  ignored**, never fatal — so a config from a newer or older kit still loads.
- A bad edit while the game is running keeps the settings already in force and
  says so loudly, rather than half-applying a broken file.
- Your file is never overwritten. If the kit has features your config predates, it
  writes `momomod.reference.ini` beside it — the same file freshly generated — so
  you can diff the two. Delete it when you are done.

## Game updates

Every feature declares what it depends on: functions and variables it looks up by
name, and any fixed addresses with the bytes that must be there. The kit checks
each feature's dependencies separately at startup.

So when the game updates, **only the features that actually broke switch off**,
and the log names the specific thing that moved:

```
[skip_splash] not supported by this game build: no such GML variable: goto_menu
```

That is a usable bug report. Everything else keeps working.

One check is still all-or-nothing: a handful of addresses that the whole mod is
built on — the object registry, the global-variable container, the data-structure
tables. If those have moved, nothing can be done safely and the kit stands down
completely, saying which check failed.

## When something goes wrong

`momomod.log`, in this folder, is the only record of what the kit did.

```bash
grep DISABLED momomod.log       why the kit or a feature stood down
grep '^\[' momomod.log          per-feature lines
grep config: momomod.log        config problems
```

If the game faults, `crash.log` gets the exception, the address, the offset into
the game, and which feature was running at the time.

Features are isolated from each other. One that panics, errors, or starts costing
you frames is switched off for the session and named in the log; the others carry
on. A feature that keeps overrunning its share of a frame first has its interval
widened and only then gets switched off — it degrades before it dies.

If a session ends badly, the next launch does not probe the game at all: the kit
stays completely passive so a bad build cannot break your game twice. The log says
so on the first line, and Ctrl+Alt+M overrides it. Once a session has run a minute
without trouble the guard stands down by itself, so an ordinary mid-session crash
costs you nothing.

## Saves

The save directory is copied into `save-backups/` at every launch, keeping the
last ten. This happens even on launches where the kit disables itself.

## Installing alongside the auto-picker

Both can be installed at once and neither disturbs the other. They use different
proxy slots — the auto-picker takes `version.dll`, the kit takes
`mfreadwrite.dll` — and each has its own config, log and crash reporter.

`install.py` says so when it notices the other one.

## Uninstalling

```bash
python uninstall.py            remove it; the game folder is left as it was
python uninstall.py --purge    also delete the config, log and snapshots
```

Installing adds exactly one file to the game folder and never touches the
executable, so nothing here can be undone by a Steam integrity check. A check may
delete the added file itself; re-run `install.py`.

## What is being worked on

Done:

- **Measurement** — `timeline` and `profiler`.
- **Startup** — `fast_boot`, above. Measured, not guessed.

Findings that change the plan, both in
[`analysis/FINDINGS.md`](analysis/FINDINGS.md):

- **The main-menu lag spikes are not the game's fault.** A profile of only those
  samples taken while the game was overdue to draw contains **no game code at all** —
  it is `Present` blocking in the graphics stack (`win32u` ← `dxgi`), on integrated
  graphics, with the Steam overlay hooked into the same path. No feature here can help.
  What does help is outside the mod: disabling the Steam overlay for the game, or a
  frame cap.
- **In-run stutter with many units is real but not yet diagnosed**: about 3.4 stalls
  per second sustained, median 63 ms. Reaching a unit-heavy run needs a human at the
  controls; the tooling is ready, and the recipe is in FINDINGS.

Not started, with the groundwork done and written up in
[`analysis/gameplay-features.md`](analysis/gameplay-features.md):

- **Modified production values**, **unit stats on hover**, and **production building
  replacement**. All three add something to the screen, and the kit currently cannot
  draw: its only foothold is the message pump, which is reached after the frame is
  finished. That document records the objects and variables each feature needs, the
  three ways out of the drawing problem, and which one each feature probably wants —
  including a false lead worth not repeating.

## For the game's developers

Every speedup this mod achieves is also written up **in terms of the game's own
source**, so it can be fixed upstream rather than only worked around here:
[`analysis/for-the-developers.md`](analysis/for-the-developers.md).

It is self-contained and shareable — it assumes nothing about this mod, and there is
nothing in it you need the mod installed to act on. The in-run stutter in particular
turns out to be a one-line change that would help every player.

Each new speedup gets added there as it lands.

## Build

```bash
cargo build --release      from the repository root
cargo test --release       75 tests across the workspace; game-dependent ones
                           skip if the game is absent
python package.py          the zip that ships
python package.py --list   what ships, what stays, and why
```

Rust, MSVC toolchain, standard library only — no crates, so it builds offline.
The shared injection and game-reading layer lives in [`../tkiw-runtime`](../tkiw-runtime)
and is used by every mod in this repository, including the reward auto-picker.

The zip carries the DLL and the install scripts, and no stamped path: `package.py`
refuses to build if the DLL it is given already has one, because a stamped DLL would
leak the builder's folder to everyone who downloaded it and send a player's mod
looking for a directory that does not exist.

## Clanker disclaimer

This mod was realized by Claude Opus 5, long may it code.
Don't use this mod if you refuse to interact with AI-generated code.
