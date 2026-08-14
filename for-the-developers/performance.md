# Performance findings

Build 2026-08-10. Sampling profiler on the game thread, 1 kHz. Windows 11, NVMe SSD,
Intel integrated graphics.

---

## 1. Boot: 26 s of 39 s in `texture_prefetch`

**Measured.** Process start to `obj_main_menu` existing: ~39 s. Of that, 26 s is a
single block with no message pump.

Profile of the first 15 s of that block:

| self | total | frame |
|---|---|---|
| 37.2% | 37.2% | texture page decode (runtime, unnamed) |
| 21.5% | 21.5% | texture page decode (runtime, unnamed) |
| — | 72.2% | `obj_init_Create_0` |
| — | 8.3% | `__scribble_font_add_from_project` |

Stack: `obj_init_Create_0` → `texture_prefetch` → page decode.

Neither hot function issues a Windows API call. Not disk-bound: storage is NVMe;
`data.win` (530 MB) reads in ~1 s.

**Measured effect of skipping it.** `texture_prefetch` stubbed to a no-op for the
duration of startup, consecutive runs, same machine:

| | skipped | unchanged |
|---|---|---|
| init block | 7.9 s | 26.3 s |
| splash screens | 16.1 s | 8.0 s |
| to main menu | 29.4 s | 39.1 s |

Over more runs: 23–27 s vs 39–43 s. **~30% reduction.** Note the splash row: ~8 s of the
18 s saved returns during the splash, as pages load on demand there. Net saving ~10 s.

**Inference.** `texture_prefetch` is a hint; unprefetched pages load on first draw. The
question is scope and timing, not whether. Options:

- prefetch only what the main menu needs, defer the rest;
- move the remainder behind `obj_splash_screen`, which already performs a sprite warm-up
  in its Draw event (`sprites_loaded`, `loaded_sprites_drawn`, paced by `warmup_frames`)
  and is therefore already a loading screen;
- prefetch during the first in-run loading screen.

In-run profiles after skipping showed no compensating texture-load cost.

### Revisited, 2026-08-13: the mechanism holds, the headline does not

Everything above about *where* the time goes was confirmed by a second, larger
measurement: 22 launches, sampling profiler, medians with confidence intervals. In
`obj_init`, texture page decompression is 45.2% (95% CI 43.6-46.8) and its CRC 28.0%
(26.5-29.5), under one `texture_prefetch` call from `obj_init_Create_0` -- verified this
time by a captured stack rather than inferred.

**The "~30% reduction" did not reproduce.** Skipping the prefetch, four launches each
way: median 48.7s to the menu with it skipped against 52.4s without, when run-to-run
spread with nothing changed is 18.8s. The figures in the table above come from single
runs, which on this machine cannot distinguish a 10s effect from noise.

The two are not in conflict about the cause. The note above -- that ~8s of the saving
returns during the splash, because unprefetched pages decompress on first draw -- is
most likely the whole story: skipping moves the work rather than removing it.

**What would remove it** is declining an atlas that is never drawn at all. Per-group
timings: `default` (every sprite in the game) 0 ms, `font_lat` ~1.0 s, `font_cyr` ~1.6 s,
`font_kr` ~4.1 s, `font_jp` ~9.9 s, `font_chi` ~11.0 s. A player reading one script needs
one of those five. That saving has not yet been measured and is the open question.

---

## 2. Resource-gain popups rebuild their text every frame

**Source.** `obj_resource_gained` Draw event:

```gml
scribble("[fnt_pixel][fa_center][fa_middle][alpha, " + string(alpha) + "]" + text)
```

`alpha` is recomputed each frame from an animation curve in the Step event.

**Cause.** In Scribble the string is the cache key. A per-frame `alpha` produces a new
key every frame, per popup. Each miss constructs a full text model — `new
__scribble_class_element`, parse, typeset, glyph positioning, vertex buffer — discarded
one frame later.

**Measured.** Profile of stalled samples (game >20 ms overdue to pump), late in a run:

```
obj_resource_gained_Draw_0
  -> scribble
    -> @@NewGMLObject@@
      -> __scribble_class_element
        -> MemoryManager -> allocator        48% of stall time
```

Cost scales with concurrent popups, therefore with production:

| time into run | 90 s | 210 s | 300 s | 540 s |
|---|---|---|---|---|
| samples stalled | 11.6% | 23.2% | 43.3% | 52.9% |

At 540 s, 80% of stalls were in game code; half of all stalled samples were in the chain
above.

**Fix.**

```gml
scribble("[fnt_pixel][fa_center][fa_middle]" + text).blend(c_white, alpha)
```

`.blend()` applies at draw time and is not part of the cache key, so one model is built
per distinct `text` and reused across the fade.

**Measured effect of an approximation of that fix.** Rounding `alpha` to 10 steps before
it reaches the string (same effect on the cache; stepped fade):

- the two allocator functions, previously 33.1% and 14.7% of stall time, left the
  profile entirely;
- stalls in game code fell from 80% to 44%; the remainder is `Present`;
- the progression stopped: 9–13% flat for a whole run, vs 11.6% → 43.3% unmodified.

A `.blend()` fix should do at least as well: it removes the rebuild rather than reducing
its frequency.

**Worth checking elsewhere.** Any per-frame value concatenated into a Scribble string has
the same cost. Search for `[alpha,` and for `string(` inside Scribble format strings.

---

## 3. Not the game's code

Stall profile at the main menu, and after fix 2 in-run:

```
100% of stalled samples' innermost frame outside game code
41.7% ntdll  25.0% win32u  16.7% igd10um64xe  8.3% gameoverlayrenderer64  8.3% d3d11
hottest stack: win32u <- dxgi <- dxgi <- dxgi
```

`Present` blocking on integrated graphics, Steam overlay in the same path. No game-side
frames. Not addressable in GML.

---

## Method note

Profile stalls separately from the average. An average profile of this game at the menu
is ~45% `dxgi` / ~39% `win32u` — a healthy idle game — while spikes are visible to the
player. Spikes are a small fraction of samples and all of the complaint; averaging hides
them.

The profiler used here keeps two profiles from one sample stream: all samples, and only
those taken while the game had not returned to its message loop for >20 ms. The second
found both problems above. In-engine equivalent: sample only when the previous frame
delta exceeded a threshold.
## Most of a cold start is glyph atlases for unselected languages

Measured on the shipped build by calling `texture_prefetch` on one texture group at a
time and timing each, from the main menu, with the boot-time prefetch suppressed:

| texture group | time |
|---|---|
| `__yy__0fallbacktexture.png_yyg_auto_gen_tex_group_name_` | 1 ms |
| `default` | **0 ms** |
| `font_chi` | **11,002 ms** |
| `font_cyr` | 1,621 ms |
| `font_jp` | **9,933 ms** |

`default` holds the game's sprites and costs nothing to bring in. The 22.5 seconds are
Chinese, Japanese and Cyrillic glyph atlases — on a launch where the game is in English
and none of them is ever drawn.

This is the same cost visible in the init profile as `__scribble_font_add_from_project`,
seen from the other side.

The saving available is large and it does not require deferring anything: a player in
one language does not need the atlases for the others. Prefetching only the groups the
selected language needs would take a 39-second cold start to roughly 17 seconds, with
no behaviour change for anyone. Loading them lazily on first use would be equivalent for
a player who never switches, and correct for one who does.

Measured on build 2026-08-10, Windows 11, NVMe SSD.
