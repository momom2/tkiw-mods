# Reading compiled GML: the runtime routines it actually calls

[runtime-structures.md](runtime-structures.md) covers the *data* the runtime
holds. This is about the *code*: the unnamed runtime functions that compiled GML
calls, why the builtin table does not name them, and how to name them yourself.

Read this before spending an afternoon wondering why a function that obviously
opens a file appears to call nothing.

## The builtin table does not name builtin calls

[orientation.md](orientation.md) describes three symbol tables, the third being
the 2,767 runtime builtins recovered from `Function_Add`. Those entries are
real, and the mod calls them successfully — but **compiled GML never calls
them.** Measured across the whole `.text` on the 2026-08-10 build, counting both
`call rel32` and `jmp rel32`:

| builtin | call sites | jmp sites |
|---|---|---|
| `ds_map_find_value` | 0 | 0 |
| `room_goto` | 0 | 0 |
| `string_split` | 0 | 0 |
| `ds_grid_get` | 0 | 0 |
| `file_text_read_string` | 0 | 0 |
| `buffer_load` | 0 | 0 |
| `draw_sprite` | 0 | 0 |
| `instance_create_depth` | 0 | 0 |
| `string_length` | 0 | 0 |
| `audio_play_sound` | 0 | 0 |

Not "few". **Zero.** Every one of them.

### They are called, but indirectly — corrected

An earlier version of this file concluded from the table above that compiled GML
"never calls the builtins". That was wrong, and a **profile** is what exposed it: a
live stack from the game's boot read

```
obj_init_Create_0  ->  sub_1aa46c0  ->  texture_prefetch  ->  ...
```

`texture_prefetch` is a `Function_Add` builtin, so plainly it does get called.

`0x1aa46c0` is how. It takes a function **index**, scales it by 24 — the stride of
the game's own function table — indexes a table of descriptors, allocates `argc * 16`
bytes for the RValue arguments, and dispatches. So every builtin call from compiled
GML is an *indirect* call through this one dispatcher, which is exactly why counting
`call rel32` sites to a builtin's address finds nothing.

The corrected statement:

* **The count above is real** — a builtin is never the target of a direct call, so an
  annotator that looks for one finds nothing and reports a function that obviously
  reads a file as calling nothing at all. That trap is genuine.
* **But the builtin table is not useless for reading the game.** It is what puts
  `texture_prefetch` on a stack. It is useful the moment you have a *return address*
  rather than a call instruction — which is what a profiler gives you, and what a
  disassembler cannot.
* **For calling into the game the table is exactly right**, unchanged: the wrapper is
  the documented convention and validates its arguments. See
  [calling-into-the-game.md](calling-into-the-game.md).

The lesson worth carrying: "no direct call sites" is a statement about one
instruction encoding, not about what the program does. Reaching for the second
conclusion cost a wrong entry in a shared table, and the wrong name then sat in a
profile pointing at the wrong function.

So a compiled GML function's *statically visible* call list contains only: other
named GML functions, and unnamed runtime routines. The routines are the vocabulary
you have to learn — and `0x1aa46c0` is the most important one, because seeing it
means "a builtin is being called here" even though the disassembly cannot say which.

## The routines you will see everywhere

Identified from their error-message strings, or from which builtin wrapper
reaches them. RVAs are for the 2026-08-10 build.

| rva | what it is | how it was identified | call sites |
|---|---|---|---|
| `0x1aa46c0` | **`call_builtin_by_index`** — how GML calls a builtin | index scaled by 24 into the function table; seen calling `texture_prefetch` in a live stack | many |
| `0x1ac46e0` | `member_get(struct_or_instance, var_id)` | argument shape at 18k sites | 18,729 |
| `0x1aa47f0` | method invoke | disassembled a call site, see [calling-into-the-game.md](calling-into-the-game.md) | 9,908 |
| `0x1af1390` | `to_string` | — | 7,387 |
| `0x1a8c880` | `YYGetReal` (RValue → double) | `"REAL argument incorrect type %s"`; reached from the `real` wrapper | 6,714 |
| `0x1a8a940` | `YYGetBool` | reached from the `bool` wrapper | 5,727 |
| `0x1a8bf50` | `YYGetInt32` | `"I32 argument incorrect type %d"` | 3,116 |
| `0x1aa4c90` | `static_get` | reached from the `static_get` wrapper | 2,953 |
| `0x1a8c0d0` | `YYGetInt64` | reached from the `int64` wrapper | 1,174 |
| `0x1b0d3a0` | a `ds_*` accessor | `"Data structure with index does not exist."` | 362 |
| `0x8f580` / `0x8f6b0` / `0x8f4e0` | RValue release / copy / assign | ubiquitous, no strings | — |
| `0x1e9ff30` / `0x1e9fc10` | GML stack frame push / pop | first and last call in every function | — |
| `0x1aa46c0` | RValue from a C string literal | always preceded by a `.rdata` string | — |
| `0x1aa4280` | runtime error | takes a format string | — |

These are kept in machine-readable form in `tools/summarise.py`'s `RUNTIME`
dict. **Add to it whenever you identify another one** — it is the cheapest
possible improvement to every future disassembly.

## How to identify one

In rough order of how often it works:

1. **Its error strings.** Runtime routines validate their arguments and the
   message usually names the type or the operation. `"REAL argument incorrect
   type %s"` is `YYGetReal` and nothing else. This identified most of the table
   above.
2. **Which builtin wrapper reaches it.** Disassemble all 2,767 wrappers,
   collect their callees, and keep the ones attributable to a single wrapper.
   This works for thin wrappers (`real`, `bool`, `int64`) and fails for the rest
   — the wrappers share a lot of argument-conversion code, so most callees are
   reached from dozens of builtins and tell you nothing. Expect a low hit rate;
   it produced about 860 candidates of which only a handful were real.
3. **Its argument shape at a call site you already understand.** Find a GML
   function whose behaviour you know, and read off what it passes. This is how
   `member_get` was pinned: every one of its 18,729 sites is preceded by
   `mov edx, <a variable-id slot>`.
4. **`xrefs.py` on a distinctive constant** it must use.

## Practical consequence for analysis

**Do not try to answer "does this function read a file / spawn an instance /
call room_goto" statically.** You can get there, but it costs an identification
pass per routine. Two cheaper routes:

* **Variables, not calls.** The variable-id annotations survive perfectly, and a
  function's variable list usually settles what it does. `obj_splash_screen`'s
  Create event reads `warmup_frames`, `fade_start`, `sound_length_1..3`,
  `sprites_loaded`, `goto_menu` — that is the whole design of the object, with
  no call resolved at all. This is why `summarise.py` prints variables inline.
* **Measure it in the live game.** A question about *cost* — which is most
  performance questions — is answered by a profiler, not a disassembler.

## Tools

`tools/summarise.py` prints a function as its call sequence with variables and
strings inline, hiding the housekeeping. It is the right first look at any
function; `gmldis.py` is the right second look once you know which instruction
matters.
