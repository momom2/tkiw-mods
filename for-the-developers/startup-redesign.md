# Startup: what it is made of, and how it could be shortened

Everything below is marked **measured**, **inferred**, or **unknown**. Two earlier
versions of this document stated a confident wrong cause, so the marking is not
decoration.

Build 2026-08-10. Windows 11, NVMe SSD, Intel integrated graphics.
Sampling profiler, 1ms, 22 launches, medians with 95% confidence intervals.

---

## 1. The sequence — measured, read from the binary

`obj_init_Create_0` is 50,170 bytes. Its calls, in address order:

```text
texturegroup_set_mode(...)
texture_prefetch(...)                     // ONE call
scribble_color_set(...) x31
string_consts(); randomize()
~101 builtin calls building inline tables (ds_map / array / struct)
dcos(); dsin(); languages_sprites_offsets()
instance_create(...) x5; instance_create_depth(...) x2
23 content libraries: shape, resource, improvement, reward,
    starting_rewards, unit, spell, artifact, game_events, encounters,
    upgrades, boss_mods, advisor, challenges, ascensions, king_skins,
    kings, meta_ups, boss_intro, tutorial_stages, level, level_mods,
    wave_modifiers
challenges_provide_rewards(); initialize_customizable_level_variables()
game_text_init(); init_time()
load_game(); load_run_data(); process_locations_unlock_cumulative()
load_game_text(); load_wave_info() x2
instance_create(...) x4
texturegroup_load("splash_screen")
```

Then, driven from `obj_font_tex_control`'s Step event: poll for the font texture group,
and on completion invoke `on_loaded_callback` → `obj_init`'s alarm → `generate_fonts`.
Then `obj_splash_screen`, then `obj_main_menu`.

Phase timings, from the mod's own timeline: ~4.6s before the message loop runs,
`obj_init` around 25s, splash 6.2s unmodified. Run-to-run spread on the whole launch is
**18.8 seconds**, so no single-run figure means anything.

## 2. Where the time goes — measured

`obj_init`, self time, 22 runs:

| mean | 95% CI | what |
|---|---|---|
| 45.2% | 43.6–46.8 | texture page decompress |
| 28.0% | 26.5–29.5 | texture page checksum (CRC-32, poly `0x04C11DB7`) |
| 5.2% | 4.9–5.5 | `memset` |
| 4.7% | 4.5–5.0 | QOI image decode |

**About three quarters of the init room is decompressing texture pages.** Every content
library together is under 2%, despite being 6.2 MB of compiled code. That line of
enquiry is closed.

### It is all under one call — measured

A captured stack, innermost first:

```text
#0  texture page decompress
#2  sub_1c9f755
#3  sub_1c095dc
#4  sub_1c0e1c0
#5  sub_1c0f08e
#6  sub_1c0dd90              per-page loader, in texture_prefetch's loop
#7  sub_1c53cc6              inside texture_prefetch (0x1c53c30)
#8  call_builtin_by_index
#9  obj_init_Create_0
```

So: **`obj_init` makes one `texture_prefetch` call, and it is three quarters of the
phase.** `texturegroup_load("splash_screen")` is the last statement in init and is *not*
where the time goes -- an earlier version of this document said it was, on the strength
of it being last and the cost being large.

### What the decompressor is — inferred

CRC-32 MSB-first with polynomial `0x04C11DB7` is bzip2's block checksum. The image
carries `bzip` strings and a `1.0.8` marker, which is bzip2's version. That fits, and it
is not proven. It was called "Ogg/Vorbis" for a day because the same polynomial is
Ogg's page checksum and the image contains `OggS` and `vorbis` -- strings that belong to
the audio subsystem, which never appears on this stack.

## 3. The per-group cost — measured

Timing `texture_prefetch` on one group at a time:

| group | time |
|---|---|
| `default` — every sprite in the game | 0 ms |
| `__yy__0fallbacktexture` | 1 ms |
| `font_lat` | ~1,000 ms |
| `font_cyr` | ~1,600 ms |
| `font_kr` | ~4,100 ms |
| `font_jp` | ~9,900 ms |
| `font_chi` | ~11,000 ms |

The game's art is free to decompress. **The cost is glyph atlases**, and four of the
five are for scripts a given player never reads.

## 4. What follows

### 4.1 Prefetch only the language in use

~25 seconds of the ~26.5 above is Chinese, Japanese, Korean and Cyrillic. A player in
one language needs one atlas. This is the largest single saving available and it changes
nothing anyone can see.

**Caveat, and it is the important one:** a mod that skipped the whole prefetch produced
no measurable saving over 4 launches each way (medians 48.7s against 52.4s, against
18.8s of spread). Skipping does not remove the decompression -- pages are decompressed
on first draw instead, which is why the splash phase grew when the prefetch was skipped.
**Declining an atlas that is never drawn should be a real saving; declining one that is
drawn merely moves the cost.** That distinction has not been measured and is the first
thing to test.

### 4.2 Do the work behind the splash, not before it

A splash screen is a loading screen. Init currently blocks for ~25s and the splash plays
afterwards. Starting the splash first and decompressing behind it would hide most of what
remains, at the granularity of a page -- `texture_prefetch` loops over a group's pages at
`0x1c53cd0`, fetching each with `0x1bebf90` and uploading with `0x1c0dd90`, so per-page
pacing is available. Per-*group* pacing is not: a single group took 11 seconds and froze
the game when tried.

### 4.3 Ship the atlases prebuilt

`obj_init_Create_0` sets `GENERATE_FONTS` and `USE_DYNAMIC_TEXTURES_FOR_FONTS`, so
atlases are built at runtime. `scribble_font_bake_shader` is 7.8% of init on average but
ranges from 0% to 11% -- present on some launches and absent on others, which suggests a
cache that is sometimes warm. Why it varies is **unknown** and worth knowing.

### 4.4 A re-entrancy hole worth fixing regardless

`obj_font_tex_control_Step_0` polls for a texture group, then sets `state` and invokes
`on_loaded_callback` → `generate_fonts`. Slow the main thread down and that callback
fires twice; `generate_fonts` is 62 KB with no guard against a second entry, so Scribble
raises:

```text
Font "fnt_hr_semibold_12_outlined_2px" already exists
gml_Script_scribble_font_duplicate
gml_Script_generate_fonts
gml_Script_anon@169@gml_Object_obj_init_Alarm_0
gml_Object_obj_font_tex_control_Step_0
```

Reproduced twice by attaching a sampling profiler. A guard on `generate_fonts`, or
setting `state` before invoking the callback, closes it.

### 4.5 The first 4.6 seconds

Before the message loop runs at all: no window, nothing drawable. Not investigated.

## 5. What is closed

- **The content libraries are not the problem.** Under 2%, measured over 22 runs.
- **`texturegroup_load` is not the problem.** It does not appear on any hot stack.
- **The audio subsystem is not involved in startup at all**, on the evidence available.
