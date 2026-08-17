# The King is Watching — notes for modding it

Everything here was recovered by working on the game, not from documentation or
source. It is written for whoever picks up the next mod, so that they start
where the last one finished rather than where it began.

**Style: clinical.** Facts, tables, addresses, APIs; minimal prose. Lessons, war stories
and reasoning-in-progress belong in [`../notes-for-claude/`](../notes-for-claude)
instead — keeping them out is what lets this stay a reference you can scan.

Read [orientation.md](orientation.md) first. It is short and it will save you
from the two or three days that went into discovering that the obvious approach
does not work.

| file | what is in it |
|---|---|
| [orientation.md](orientation.md) | what kind of binary this is, and what that rules out |
| [runtime-structures.md](runtime-structures.md) | RValue, strings, arrays, structs, `ds_list`, `ds_map`, instances, globals |
| [runtime-internals.md](runtime-internals.md) | the unnamed runtime routines compiled GML actually calls, and why the builtin table does not name them |
| [calling-into-the-game.md](calling-into-the-game.md) | the three calling conventions, and which to use |
| [drawing.md](drawing.md) | drawing shapes on screen from a mod, with `tkiw_runtime::overlay` |
| [injection.md](injection.md) | getting loaded, getting onto the game thread, surviving Steam |
| [addresses.md](addresses.md) | every baked address, what it is, and how to re-derive it |
| [game-content.md](game-content.md) | the libraries: rewards, resources, artifacts, improvements, unit classes |
| [pitfalls.md](../notes-for-claude/pitfalls.md) | the mistakes already made, so they are not made twice |
| [tooling.md](tooling.md) | the analysis scripts in [`tools/`](tools) and what each is for |

## The one-paragraph version

The game is GameMaker with the YYC compiler, so all its GML is native x86-64 and
`data.win` holds no code. UndertaleModTool and every other `data.win` tool is
useless here. But the executable **symbolises itself**: it carries tables
pairing `gml_*` name strings with function pointers and variable-id slots, so a
mod can resolve almost everything it needs *by name* at runtime and survive game
updates. Getting loaded is easy — the game statically imports several DLLs that are
not KnownDLLs, so a proxy copy of one in the game folder wins the loader search;
pick the slot by **export coverage**, not export count, and see
[injection.md](injection.md) for the table and the trap. Getting onto the game's
thread is done by hooking its `PeekMessageW` IAT slot, which needs no code patching
at all. Reading game state needs no patching either: instances, globals and data
structures can all be walked with pure reads.

Two things that are *not* obvious from the above, and cost a session each: your
startup thread runs **before the game's entry point**, so the GML runtime does not
exist yet and half of what you want to resolve must be resolved lazily; and a
question about *cost* is answered by a profiler, not a disassembler — the
disassembly cannot even show you which builtin is being called.

## Two rules worth keeping

**Resolve by name; bake addresses only when you must, and guard them.** Names
survive a game update. Addresses do not, and an address that has moved points at
whatever now lives there — the by-name checks still pass, so the mod looks
healthy and calls into arbitrary code on someone else's machine. Every baked
address should carry a byte signature of the function it is supposed to be, and
a mismatch should disable the mod loudly. See [addresses.md](addresses.md).

**Reads are safe; calls are not.** Nearly everything interesting can be done
with pure reads, and a wrong read fails to `None`. A wrong call corrupts memory
or takes the process down with it, on a player's machine, mid-run. Prefer
reading. When you must call, use the convention the game itself uses at a site
you have disassembled — not the one that looks right.

## The code, as well as the notes

One Cargo workspace at the repository root; `cargo test --release` from there covers
every mod at once (75 tests).

| crate / folder | what it is |
|---|---|
| `tkiw-runtime/` | the shared layer **every mod depends on**: injection, symbol discovery, validated reads, the frame hook, module enumeration, stack sampling, symbolisation, verified byte patching, the log, the crash reporter, save snapshots, the signature guard |
| `tkiw-momomod-kit/` | a loader hosting many small features, each declaring its own dependencies. **A new change to the game probably wants to be a feature here rather than a new mod** — see its `spec.md` for the contract |
| `tkiw-reward-auto-picker/` | the reward picker; shares `tkiw-runtime` since 2026-08-12 |
| `tkiw-morale-fix/` | a static byte patch, and the pristine `.exe.orig` every analysis reads |

**Depend on `tkiw-runtime` rather than copying it.** The things that go wrong in that
layer — an ASLR miscalculation, a region cache slower than what it caches, a crash
reporter that allocates, a module list that goes stale and attributes a third of a
profile to nothing — each cost a session to find, and should be fixed once.

### Where the measurements live

`tkiw-momomod-kit/analysis/` holds what profiling the game actually established:

* `FINDINGS.md` — where startup time goes (and the one intervention that helps),
  why the main-menu lag spikes are **not** the game's fault, and two measurement bugs
  that produced confident wrong answers before they were found.
* `gameplay-features.md` — the objects and variables behind production values, unit
  stats and building replacement, the reason none of them is implemented yet (the kit
  cannot draw), and the three ways out of that.

## Provenance

Written while building:

* `tkiw-morale-fix` — a static byte patch to the executable.
* `tkiw-reward-auto-picker` — a proxy DLL that reads live game state and drives
  the reward UI by invoking the game's own methods. The source of nearly
  everything here.
* `tkiw-momomod-kit` — a multi-feature loader, and the reason the runtime layer
  and these tools are shared rather than copied.
