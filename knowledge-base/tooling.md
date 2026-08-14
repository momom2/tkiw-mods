# Tooling

**The shared scripts live in [`tools/`](tools), next to these notes.** They are
plain Python over the executable on disk, with `capstone` for disassembly. Use
them from here rather than copying them into a mod — a fix made here is a fix
for every future mod. Copy one out only if a mod needs to modify it.

They read the game from a path you pass, or from the pristine copy at
`tkiw-morale-fix/The King is Watching.exe.orig`. Keeping an unmodified copy of
the executable around is worth doing — it is what every analysis and every
build-guard test reads.

| script | what it does |
|---|---|
| `tkiw.py` | the library the rest use: PE parsing, the `gml_*` function table, the variable-slot table, `.pdata` function boundaries, a disassembler |
| `summarise.py` | **the right first look at a function.** Its calls in order, with variables and strings inline, housekeeping hidden |
| `gmldis.py` | **the right second look.** Full annotated disassembly of a named function |
| `builtins.py` | recovers the 2,767 named runtime builtins by walking `Function_Add`'s call sites. Cached in `builtins.pickle` as name → `(rva, argc)`. Read [runtime-internals.md](runtime-internals.md) before using it to annotate anything |
| `strconsts.py` | recovers the GML string-constant pool |
| `xrefs.py` | every instruction in `.text` referencing a given address |
| `index.py` | a cached cross-reference index over every compiled GML function |
| `slotrefs.py` | every reference to a GML variable, by raw byte scan -- complete where `index.py` is not |
| `methods.py` | resolves anonymous methods (`anon@NNNN@...`) to the variable names they are bound to |
| `gen_proxy.py` | picks a proxy-DLL slot for a new mod, and generates its forwarder list. `--survey` ranks the candidates; see [injection.md](injection.md) |
| `playtest.py` | **launches the game unattended**, waits for patterns in a mod's log, and kills it. Turns "does it still boot, and how long does it take" into a command |
| `timeit.py` | launches the game N times and reports median and **spread** of time to the main menu. The spread is why: single-run startup figures on this machine varied by 20s |
| `measure-startup.py` | switches everything the kit does off, turns the profiler on, launches once, and **restores the config afterwards**. `--keep` opts out of the restore; see the pitfall about leaving a measuring config installed |
| `profiles.py` | aggregates the profiler's per-launch CSVs into means with 95% confidence intervals. One run is a hypothesis; this is what turns a batch into a finding |
| `builtin_calls.py` | names every runtime builtin a given GML function calls. The trick is that a builtin's dispatcher index slot carries its name string pointer 8 bytes below it, exactly as a variable slot does |
| `gen_builtin_table.py` | bakes those 2,769 names into `tkiw-runtime/src/builtins_table.rs`, so the profiler can symbolise them at runtime |
| `keep-awake.ps1` | holds the machine awake for an unattended session, via `SetThreadExecutionState`. For overnight runs, where a machine that sleeps halfway through wastes all of it |

The `.pickle` files beside them are caches and are regenerated on demand;
deleting one costs seconds (`builtins.py`) to half a minute (`index.py`).

Two more scripts stayed with the mod that needed them, in
`tkiw-reward-auto-picker/analysis/`: `extract.py`, which regenerates that mod's
reward-option reference docs, and `verify_live.py`, which diffs them against a
running game. Both are worth reading as examples if you need the same shape.

## summarise.py

```bash
python summarise.py gml_Object_obj_init_Alarm_0
python summarise.py load_game_text --callers
```

YYC emits roughly fifteen instructions of RValue housekeeping per line of GML,
so a full disassembly buries the shape of the function. `summarise.py` prints
only the calls, each annotated with the variables read since the previous one:

```
; gml_Object_obj_init_Alarm_0   rva 0x13de4f0..0x13de8b8  (968 bytes)
013de587  *qword ptr [rax + 8]  [var:LANGUAGE]
013de5dd  member_get  [var:setup]
013de62d  method_invoke
013de6b1  member_get  [var:load_font_textures]
013de747  method_invoke
013de7ee  sub_1ac4be0  [var:on_loaded_callback]
```

which is the boot sequence, legibly. Note what it cannot show: **builtin calls
never appear**, because compiled GML does not make them — see
[runtime-internals.md](runtime-internals.md), and expect to read a function's
*variables* rather than its calls to work out what it does.

## gmldis.py

```bash
python gmldis.py gml_Object_obj_card_class_stat_bonus_Step_0
python gmldis.py --grep reroll
python gmldis.py <name> --exe "path/to/The King is Watching.exe"
```

It resolves inline:

* `var:NAME` for a rip-relative read of a variable-id slot
* `"str"` for a string constant
* `-> symbol` for a call to a named function or known builtin

That annotation is what makes YYC output readable. A function's variable
references alone usually tell you what it does — this is how
`resolve_reroll_cost` was identified as a setter rather than a getter, and how
the hover/`hide_units_icons` mechanism was found.

**Reach for it earlier than feels necessary.** Most of the time lost across two
mods went into inferring behaviour from observation that ten minutes of
disassembly answered outright.

## Finding an anonymous method

A method assigned to an instance variable does not appear under that name.

1. Disassemble the object's `Create_0` event.
2. Find `var:<the member name>` — it will be a write through `[vtable+0x10]`.
3. A few instructions later, a `lea rdx, [rip - ...]` gives the address of the
   `anon@NNNN@...` function being bound.
4. Disassemble that.

## playtest.py

```bash
python playtest.py --log ../../tkiw-momomod-kit/momomod.log \
                   --until "+ obj_main_menu" --timeout 200
```

Launches the game, watches the log for the patterns given, kills it, prints the tail.
Exit code says whether every pattern appeared. This is what makes "did that change
break booting?" a five-line command rather than a request to a human.

Two things it knows that cost an hour each to find out:

* **Running the executable directly does not work.** `steam_api64` sees no app
  context, calls `SteamAPI_RestartAppIfNecessary`, and exits with code 0 while Steam
  starts a *fresh* process. A launcher watching the process it spawned sees a clean
  exit after nine seconds and misses the game entirely. So it launches
  `steam://rungameid/2753900` and finds the game **by process name**.
* **A force-kill leaves the mod's crash-loop breadcrumb behind**, and with Steam
  relaunching, the next launch can be seconds later and part of the same test — where
  it would go passive and record nothing. The breadcrumb is cleared both before
  launching and once the game is up.

On timeout it reports whether the window was *responding*, which distinguishes a game
that is busy from one that is wedged.

**Be careful with it.** Each launch reads 530 MB and does real GPU work; three
back-to-back runs with force-kills between them noticeably slowed the whole machine.
Run them one at a time, and never while someone is playing — it refuses if the game is
already running.

## Regenerating after a game update

0. Delete the `.pickle` caches in `tools/`. They are keyed to nothing and will
   happily serve you the old build's answers.
1. `builtins.py` — recovers most code addresses by name, nearly free.
2. `gmldis.py` on a builtin that touches each data table, to re-derive the
   handful of data addresses.
3. Update the byte signatures in the mod's build guard, and run its test against
   the new executable.
4. Option pools do not need regenerating if your mod reads them from the live
   game, which is the right way to do it.
