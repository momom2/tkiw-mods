# Startup: what it is made of, and how it could be shortened

A ground-up look at cold start, replacing an earlier account that was built on one
plausible assumption and never checked. Everything below is marked **measured**,
**inferred**, or **unknown**, because the previous round's mistake was not a wrong
number — it was an unmarked guess.

Build 2026-08-10. Windows 11, NVMe SSD, Intel integrated graphics.

---

## 1. The shape of a cold start — measured

From the mod's own phase timeline, which watches for each object appearing:

| from | to | phase | duration |
|---|---|---|---|
| 0.0s | 4.6s | before the game's message loop runs | **4.6s** |
| 4.6s | 13.6s | `obj_init` — libraries, localisation, fonts | **~4–5s** |
| 13.6s | 19.8s | `obj_splash_screen` | **6.2s** unmodified |
| 19.8s | | `obj_main_menu` | |

Total to a usable menu is tens of seconds; the exact figure is not worth quoting,
because **run-to-run spread on one machine with nothing changed is 18.8 seconds**
(four launches: 43.2, 46.0, 51.5, 62.0). Any proposal below has to beat that noise
before it can be called an improvement, which means batches, not single runs.

## 2. Where the time goes — measured

Sampling profile, 17,663 samples over 30s, taken from ~5s in, so it covers `obj_init`
and the splash:

| share | function | what it is |
|---|---|---|
| 28.5% | `sub_1c9fd30` | 10,604 bytes, very large context struct |
| 17.2% | `sub_1ca3ab0` | 1,994 bytes, byte-at-a-time table loop |
| 11.6% | `ntdll.dll` | |
| 7.9% | `win32u.dll` | |
| 5.9% | `sub_1ea5cc0` | |

**26.9% of samples had their innermost frame outside the game's own code** (OS, GPU,
system libraries). So roughly three quarters of startup is the game's own CPU work,
not waiting on disk.

### The two hot functions are one subsystem, and it is audio

`sub_1ca3ab0`'s inner loop is

```text
crc = (crc << 8) ^ table[(crc >> 24) ^ byte]
```

against a table at `0x29856d0` beginning `00000000 04c11db7 09823b6e 0d4326d9` —
**CRC-32, MSB-first, polynomial `0x04C11DB7`**. That is not PNG's and not zlib's
(both use the reflected `0xEDB88320`). It is the polynomial Ogg uses for its page
checksums. The binary contains `OggS` and five occurrences of `vorbis`.

Both functions are called from the same parent, `sub_1c9fb28`, which has no direct
callers — reached through a pointer, as a registered codec entry point is.

**So ~46% of startup is Ogg/Vorbis decoding.** Confidence: high for the CRC (the
polynomial and table are conclusive); high for the pairing (one shared parent);
*inferred* for `sub_1c9fd30` being the Vorbis decoder specifically rather than another
stage of the same pipeline.

### What is *not* where the time goes

Texture groups, timed one at a time by calling `texture_prefetch` directly:

| group | time |
|---|---|
| `default` — every sprite in the game | **0 ms** |
| `__yy__0fallbacktexture` | 1 ms |
| `font_lat` | 1,017 ms |
| `font_cyr` | 1,621 ms |
| `font_kr` | 4,051 ms |
| `font_jp` | 9,933 ms |
| `font_chi` | 11,002 ms |

The game's art is free to bring in. The 26.5 seconds are glyph atlases, four of them
for scripts a given player never reads.

**Unknown, and it matters:** whether the game calls `texture_prefetch` during startup
at all. The builtins are dispatched by index, so no cross-reference over the compiled
GML can answer it — a search that returned zero for `texture_prefetch` returned zero
for `draw_sprite` too. A mod that stubbed the function out for the whole of startup
produced a 3.7s median difference against 18.8s of noise, which is consistent with it
never having been called. A call counter now sits in that stub; one launch settles it.

## 3. What `obj_init` does — measured

`obj_init_Create_0` is 50 KB of compiled code and calls 37 distinct scripts. Almost
all of it is declaration; the work is in the library builders it invokes:

| compiled bytes | builder |
|---|---|
| 2,114,793 | `unit_library` |
| 1,189,394 | `improvement_library` |
| 601,008 | `encounters_library` |
| 529,359 | `upgrades_library` |
| 511,394 | `kings_library` |
| 407,571 | `meta_ups_library` |
| 166,816 | `artifact_library` |
| … | 9 more |
| **6,197,377** | **top 16 together** |

That is six megabytes of straight-line code whose entire job is to populate 305 units,
176 improvements, 269 upgrades and so on, one struct member at a time. Every one of
those assignments is a runtime variable-slot write.

This is not an algorithmic problem — it is O(entries × fields), which is the minimum
for building the data. It is a **constant-factor** problem, and the constant is a
function call and a hash write per field.

---

## 4. Where the opportunities are

Ordered by expected value, with the honest caveat that #1 rests on a measurement that
should be repeated before anyone acts on it.

### 4.1 Audio: decode less, or decode later — the largest single share

~46% of startup, and the splash screen plays a jingle. Three questions decide the fix,
and none is answered yet:

1. **Is this bulk decompression at load, or streaming playback?** GameMaker decompresses
   whole audio groups on load for non-streamed sounds. If the splash's jingle and the
   menu music are compressed-on-disk and decompressed-to-memory up front, that cost is
   paid before anything can be shown.
2. **How much audio is decoded that is never played?** The same question the font
   atlases turned out to answer badly for the game: everything, regardless of need.
3. **Can it be deferred?** Menu music is not needed until the menu; run music is not
   needed until a run.

Likely shapes of a fix, for the game's own source:
- Mark sounds that are not needed during startup as **streamed** rather than
  decompressed, so their cost is spread over playback instead of paid at load.
- Split audio groups so the boot group holds only the splash jingle, and load the rest
  after the menu appears.
- If a jingle gates the splash's duration, that is a design choice worth revisiting
  independently: the splash is currently as long as the sound.

### 4.2 Fonts: generate only what the language needs

26.5 seconds of glyph atlases, of which a player in one language needs one. This is the
same cost visible in the init profile as `__scribble_font_add_from_project`, which was
8.3% of the init window in an earlier profile.

`obj_init_Create_0` sets `GENERATE_FONTS` and `USE_DYNAMIC_TEXTURES_FOR_FONTS`, so the
atlases are built at runtime rather than shipped prebuilt. Two independent savings:

- **Build only the selected language's atlas.** A player who switches language pays
  once, then, instead of everybody paying for four languages every launch.
- **Ship prebuilt atlases** for the common case, and generate only on a miss.

**Unknown:** whether generation and prefetch are separable, i.e. whether an atlas that
is never prefetched is also never built. If building happens regardless, then declining
the prefetch saves nothing and only the generation side is worth attacking.

### 4.3 Library building: a constant-factor problem with two exits

Six megabytes of code, executed once, to build data that is identical on every launch.

- **Data-driven rather than code-driven.** The libraries are literals compiled into
  instructions. The same content as a data blob — parsed once, or better, laid out so
  it needs no parsing — replaces millions of runtime variable writes with a copy. This
  is the single biggest structural change available, and also the largest to make.
- **Build lazily, per library.** Nothing in the main menu needs `encounters_library` or
  `boss_mods_library`. Building each on first use moves them off the startup path
  entirely, at the cost of a hitch when first touched — which, unlike startup, can be
  hidden behind a transition that is already happening.

### 4.4 Hide what is left behind the splash

Only worth doing after the above, and only for work that genuinely cannot be removed.

The splash is 6.2 seconds of animation during which the game is otherwise idle. Work
moved there is invisible **if it is sliced finely enough to keep the animation running**.
The unit matters: an attempt to do texture prefetching there, one group per frame,
froze the game for 11 seconds on a single group, because a group is far too coarse. The
finer unit exists — `texture_prefetch` loops over a group's pages at `0x1c53cd0`,
fetching each with `0x1bebf90` and uploading with `0x1c0dd90` — and per-page pacing is
small enough to hide. The same principle applies to anything else moved here: it must
be interruptible at a granularity of a frame.

### 4.5 The first 4.6 seconds

Before the game's message loop runs at all. Not investigated, and worth a look purely
because it is 4.6 seconds during which nothing can be shown to the player — no splash,
no window, nothing. Candidates: runtime initialisation, `data.win` (530 MB) being opened
and indexed, audio device setup.

---

## 5. What to do, in order

Each step's output decides whether the next one is worth taking.

1. **Build a startup measurement harness that beats the noise.** Batches of launches,
   median and spread reported, one variable changed between batches. Without this,
   nothing below can be evaluated. *(The mod now has `timeit.py`, which does this.)*
2. **Settle the texture-prefetch question.** One launch with the call counter. If the
   game never calls it, the entire texture line of enquiry closes and the font cost
   belongs wholly to generation.
3. **Instrument the phases from inside.** Timestamps around each library builder, around
   font generation, and around audio group loading, so the 4.6s / init / splash split
   becomes a per-subsystem breakdown rather than a profile that has to be reverse
   engineered.
4. **Confirm the audio hypothesis** by sampling with the audio subsystem's entry point
   named, and by checking whether the cost scales with the number of sounds loaded.
5. **Then, and only then, choose between** deferring audio, scoping font generation, and
   making the libraries data-driven — in that order, because that is the order of
   measured share.

---

## 6. What this replaces

An earlier version of this document claimed boot was "26s of 39s in `texture_prefetch`"
and that a mod skipping it made startup ~25% faster. Both came from single launches, and
the profile they rested on was never checked against a repeated measurement. The skip
does not reproduce a saving. That claim is withdrawn; this document is what the evidence
actually supports, including where it supports nothing yet.
