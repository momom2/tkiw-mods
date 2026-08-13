# Performance findings

Measured on the 2026-08-10 build with the kit's own `timeline` and `profiler`
features, on a machine with an NVMe SSD and **Intel integrated graphics only** (no
discrete GPU). The GPU matters for the second half of this document.

Every number here came from a log, not from reading code.

Upstream-facing version of these findings: [`../../for-the-developers/performance.md`](../../for-the-developers/performance.md).
The reasoning, the wrong turns and the session narrative are in
[`../../notes-for-claude/`](../../notes-for-claude).

---

## Startup: 39 seconds to the main menu, and where it goes

A launch, from the process starting to `obj_main_menu` existing:

| phase | duration | what it is |
|---|---|---|
| before the first message pump | 1–4 s | the GameMaker runner starting up |
| **one unbroken block, no pump at all** | **26 s** | `obj_init_Create_0` |
| `obj_init` visible as an instance | 3.5 s | the rest of its setup |
| splash screens | 8 s | logo animations, paced by their sounds |
| **total** | **~39 s** | |

### The 26-second block is `texture_prefetch`

The profiler put **59% of the first fifteen seconds** in two functions:

```
 self   total  function
37.2%   37.2%  sub_1c9fd30
21.5%   21.5%  sub_1ca3ab0
 9.2%   99.3%  ntdll.dll
 0.5%    8.3%  __scribble_font_add_from_project
              (and, holding it all in its stack:)
72.2%          obj_init_Create_0
```

with the hottest stack, innermost first:

```
sub_1c9fd30  <-  sub_1c9fb28  <-  sub_1c9f755  <-  ...
   <-  texture_prefetch  <-  call_builtin_by_index  <-  obj_init_Create_0
```

**Neither hot function calls a single Windows API.** Not `CreateFile`, not d3d11 —
nothing. So this is not disk I/O and not the GPU: it is about ten kilobytes of
straight-line CPU work per call, decoding texture pages, on the game's own thread,
before anything is on screen.

That also disposes of the obvious theory. `data.win` is 530 MB, which *sounds* like
the answer, but the machine's storage is NVMe and would read it in about a second.

### Two corrections this required

**The phase timeline was lying about when `obj_init` starts.** It reported
`obj_init` appearing at 25.8 s, because that is when the *object registry* first
became readable — but the profiler shows `obj_init_Create_0` running from ~3.5 s. The
"block before init" *is* init. A marker that depends on the game's own data
structures cannot see anything before those structures exist, which is exactly the
phase worth measuring.

**"No direct call sites" did not mean "never called".** An earlier analysis found
zero `call rel32` sites for every `Function_Add` builtin and concluded compiled GML
never calls them. The stack above disproves it: `texture_prefetch` *is* one of those
builtins. They are reached through a dispatcher at `0x1aa46c0` that scales a function
index by 24 — the stride of the game's own function table — and dispatches
indirectly. See [`runtime-internals.md`](../../knowledge-base/runtime-internals.md).

### What skipping it buys — measured, both ways

`fast_boot` stubs `texture_prefetch` to a no-op for the duration of startup. Same
machine, consecutive runs:

| | prefetch skipped | control | delta |
|---|---|---|---|
| before the first pump | 3.8 s | 1.3 s | +2.5 s (noise) |
| the init block | **7.9 s** | 26.3 s | **−18.4 s** |
| `obj_init` visible | 1.6 s | 3.5 s | −1.9 s |
| splash screens | 16.1 s | 8.0 s | **+8.1 s** |
| **to the main menu** | **29.4 s** | 39.1 s | **−9.7 s** |

**Read the splash row.** Roughly eight of the eighteen seconds saved come back during
the splash, as pages load on demand while the logos draw. That is the honest shape of
this optimisation: it is not eighteen seconds of pure profit, it is a real ~25%
reduction with part of the work relocated to where the player is already waiting.

It also suggests the splash is not purely sound-paced — its wall-clock length grows
when there is loading to do behind it.

### Still on the table

- **The splash screens themselves**, 8–16 s — but **do not simply skip them.**

  This was the obvious next target and it is a trap. `obj_splash_screen` does advance on
  `sound_played` against `sound_length_1..3` with `goto_menu` as the exit, so part of the
  wait really is just logo jingles. But look at where its other variables are used:

  | variable | Create | **Draw** | Step |
  |---|---|---|---|
  | `sprites_loaded` | ✓ | ✓ | |
  | `loaded_sprites_drawn` | ✓ | ✓ (and `Draw_64`) | |
  | `warmup_frames` | ✓ | ✓ | ✓ |

  `sprites_loaded` and `loaded_sprites_drawn` appear **only in the Draw events**. The
  splash is drawing sprites to force their texture pages to upload — the standard
  GameMaker warm-up trick — and `warmup_frames` paces it. It is not a wait with a logo
  over it; it is a loading screen that happens to have a logo over it.

  This also explains the `fast_boot` splash row: with the prefetch skipped, the pages
  the splash draws have to load right there, and it stretches from 8 s to 16 s. The
  splash is *the right place* for that work — the player is already waiting and nothing
  is interactive.

  So the refinement worth making is narrower than "skip the splash": keep the warm-up,
  drop the part of the wait that is only the jingle finishing. That needs establishing
  which of the two is binding in `obj_splash_screen_Step_0` — the sound comparison or
  `warmup_frames` — and it is a small piece of disassembly, not a guess.
- **Font generation.** `obj_init_Create_0` sets `GENERATE_FONTS`,
  `USE_DYNAMIC_TEXTURES_FOR_FONTS` and `FONT_SCALING_ON`, and
  `__scribble_font_add_from_project` is 8.3% of the init window. If glyph atlases are
  generated at runtime for a character set wider than the language in use — the game
  ships a 4 MB `localization.csv` and a `LANGUAGE` global — restricting that is a
  large and safe win. Not yet investigated.

---

## The main-menu lag spikes are not the game's fault

This was the other headline complaint, and the answer is not what a mod can fix.

An *average* profile of the menu is 45% `dxgi` and 39% `win32u` — the game waiting
for vsync, which is what a healthy idle game looks like. The spikes are a small
fraction of samples by count and all of the complaint, so averaging them away hides
them. The profiler therefore keeps a **second profile of only those samples taken
while the game had not returned to its message loop for 20 ms**:

```
==== only the samples taken while the game was >20ms overdue to pump ====
100.0% of samples had their innermost frame outside the game's own code
 self   total  function
41.7%  100.0%  ntdll.dll
25.0%   25.0%  win32u.dll
16.7%   50.0%  igd10um64xe.dll        <- Intel integrated GPU driver
 8.3%   58.3%  gameoverlayrenderer64.dll   <- Steam overlay
 8.3%   58.3%  d3d11.dll
              hottest stack: win32u.dll <- dxgi.dll <- dxgi.dll <- dxgi.dll
```

**Not one game-side frame.** The stalls are `Present` blocking in the graphics stack
on an integrated GPU, with the Steam overlay hooked into the same path. There is no
GML to make faster, and no patch that helps.

What would actually help this machine is outside the mod: an unthrottled frame limit
or disabling the Steam overlay for the game. Worth telling a player; not worth
pretending a feature can do it.

### A measurement bug worth knowing about

The first version of that stall profile attributed **37% of samples to
`<unmapped>`** — an address in no known module. The module list was enumerated *once*,
seconds into the launch, before the graphics stack, the audio codecs and the Steam
overlay had finished loading. A module we do not know about cannot be named, and
worse cannot be unwound through, so whole stacks were lost as well. The list is now
re-enumerated every three seconds, and `<unmapped>` disappeared entirely.

Bounded staleness matters more than it looks: a profiler that silently drops a third
of its samples into a bucket labelled "nothing" will happily support whatever
conclusion you already had.

---

## In-run stutter, with many units

From a real play session, 459 seconds after a run loaded: **1,567 stalls over 50 ms,
about 3.4 per second sustained**, median worst-per-window 63 ms, p90 105 ms. At 60 Hz
a 63 ms stall is four dropped frames, which matches "it gets laggy with a lot of
units".

### Profiled, and it is not the units

A 10-minute session with a real run. The stall fraction climbs steadily as the run
progresses -- 11.6% of samples at 90s, 23% at 210s, 43% at 300s, **52.9% at 540s** --
which is what "it gets laggy with a lot of units" feels like from the inside.

Unlike the menu, **80% of these stalls are inside the game's own code**. The top three
stall stacks are all one chain, ~50% of stalled samples:

```
obj_resource_gained_Draw_0
  -> scribble
    -> @@NewGMLObject@@            (0x1af3cf0 -- the GML `new` operator)
      -> __scribble_class_element
        -> ... -> MemoryManager (0x1ac5fa0) -> the allocator
                                   sub_1ea5cc0  33.1% self
                                   sub_1ea52e0  14.7% self
```

**The cause is one line of GML.** `obj_resource_gained_Draw_0` builds its text as

```
"[fnt_pixel][fa_center][fa_middle][alpha, " + string(alpha) + "]" + text
```

and `alpha` is the popup's fade, which changes every frame. In Scribble **the string is
the cache key**, so every frame, for every popup, the key is new, the cache misses, and
a complete text model is constructed and thrown away. Scribble's own guidance is to set
alpha with `.blend()` -- which does not touch the cache key -- rather than with an
inline `[alpha, N]`.

This is a genuine performance bug in the game, worth reporting upstream: the fix is
`.blend(c_white, alpha)` instead of the inline tag, and it costs nothing.

**What a mod could do**, in increasing order of intrusiveness:

1. **Quantise `alpha` on each `obj_resource_gained` instance** to, say, one decimal
   place, every frame. The string then takes ~10 distinct values instead of 60 per
   second, and the cache hits. This is a *state write* -- no drawing, no detour -- and
   so is the cheapest real fix available. It needs the write path (`vtable+0x10`), which
   the runtime does not expose yet, and it makes the fade very slightly steppy.
2. **Cap the number of simultaneous popups.** Changes what the player sees.
3. **Detour the Draw event** to build the string properly. Correct, and the most work.

Route 1 is the one to try, and it wants measuring before and after with this same
stall profile.

### The measurement caveat

**The earlier note here said this was unprofiled**, because reaching a unit-heavy run needs a
human at the controls. That is the next measurement to take, and the tooling for it is
already in place: enable `profiler`, load a heavy run, and read the stalled-samples
block.

One caveat recorded from the same session: the worst "stall" was 210 seconds, which
was the player away from the keyboard. Gaps are now split into **stalls** (50 ms–2 s)
and **pauses** (>2 s), because one alt-tab otherwise dominates every statistic.


### Verified: the fix works, and the evidence that matters is not the percentage

A 290-second run with `popup_stutter_fix` active, against the baseline above at matched
points into the run:

| time into the run | baseline | with the fix |
|---|---|---|
| ~30 s | 17.8% | 9.6% |
| ~60 s | 20.6% | 13.5% |
| ~90 s | 22.1% | 13.3% |
| ~120 s | 23.2% | 6.5% |
| ~150 s | 34.3% | 8.8% |
| ~180 s | 38.1% | 9.0% |
| ~210 s | 43.3% | 11.6% |

**The percentages are the weaker evidence**, and it is worth being clear why. The two
sessions were not perfectly matched: the baseline was real play with the auto-picker
resolving rewards, the second was a quieter run with it disabled. A lighter workload
lowers the number on its own.

The strong evidence is what left the profile. In the stall profile of a mid-run window:

| | baseline | with the fix |
|---|---|---|
| `sub_1ea5cc0` (allocator) | 33.1% self | **not in the top 20** |
| `sub_1ea52e0` (allocator) | 14.7% self | **not in the top 20** |
| stalls inside the game's own code | 80% | 44% |
| top of the profile | `obj_resource_gained -> scribble -> @@NewGMLObject@@` | `win32u`, `ntdll`, `igd10um64xe`, `dxgi` |

A quieter run would have made those frames *smaller*. It would not have removed them.
They are gone because they stopped being called.

And the shape changed: the baseline's stalled share climbed monotonically with
production, 11.6% -> 43.5%. With the fix it is flat at 9-13% for the whole run, which is
what "the cost no longer scales with popup count" looks like. What remains is the same
`Present`/vsync signature as the main menu -- the machine's GPU ceiling.
