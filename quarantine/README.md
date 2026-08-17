# Quarantine

Features parked out of the build on purpose. They are **not compiled, not loaded,
not shipped, and not offered by the mod manager**. The source is kept here so the
work is recoverable, not lost — "come back to it someday."

To bring one back, reverse the steps in "How they were removed" below.

---

## `fast_boot.rs` + `font_atlases.rs` — startup optimization (parked 2026-08-15)

**Decision:** the author asked to quarantine `fast_boot` and pretend it does not
exist for now. `font_atlases` went with it because it **cannot work without
`fast_boot`**: it relies on fast_boot stubbing the whole boot prefetch and then
re-fetches only the glyph groups you want, so with fast_boot gone it has nothing to
decline. `popup_stutter_fix` stays in the optimization mod; it is unrelated and
proven useful.

**What they did.**
- `fast_boot`: byte-patched `texture_prefetch` into a no-op for the duration of
  startup, restoring it when `obj_main_menu` appears. `texture_prefetch` is a
  GameMaker *hint* (front-loads texture-page decompression), so skipping it loses no
  texture — it defers each page to first draw. Config: `restore_on` (main_menu /
  never), `catch_up` (groups warmed per menu tick).
- `font_atlases`: with fast_boot's stub in place, re-prefetched only the glyph atlas
  groups you asked for (Latin on by default; Chinese/Japanese/Korean/Cyrillic off),
  making the skip of the others permanent rather than deferred.

**What the measurements said (overnight 2026-08-15, `timeit.py`, warm adjacent
baselines).** Kept here because it is the reason to park them, and the map for a
future return:
- `fast_boot` **does** save ~6s (~13%): off ~46s median over 16 runs (none under
  42.3s), on 39.7s (n=6), reaching 34.7s as the OS texture cache warms. Not warming
  — the on-batch ran before the warm off-batch, which stayed at 46s.
- `font_atlases` saves **nothing**: declining chi+jp+kr+cyr moved the median 1.0s,
  lost inside a 5-7s spread. Those atlases are never drawn in an English run, so
  deferring them changes nothing at boot.
- Reconciliation: fast_boot's ~6s is in deferring the *drawn* pages (default sprites,
  Latin) to first draw. The ~26s foreign-glyph figure is atlas *generation*
  (`GENERATE_FONTS` / `__scribble_font_add_from_project`, `sub_1c9fd30` /
  `sub_1ca3ab0`), which runs regardless of prefetch and is the real target for
  anyone wanting boot shorter still.

So the standing knowledge for a return: **fast_boot works and is worth ~6s;
font_atlases as built is a dead end** (or needs decoupling from fast_boot and a
different mechanism that attacks generation, not the prefetch upload).

### How they were removed (reverse to restore)

In `momomod-manager/src/features/mod.rs`:
- deleted `pub mod fast_boot;` and `pub mod font_atlases;`
- deleted the two `Box::new(...)` entries from `all()`
- deleted the `"fast_boot"` and `"font_atlases"` arms of `extra_keys()`
- the tests that used them as their example feature were switched to
  `popup_stutter_fix`

In `momomod-manager/config/optimization.ini` (gitignored, local install only): the
`[feature.fast_boot]` and `[feature.font_atlases]` blocks were removed. The
`momomod.ini` mirror regenerates itself on next launch.

Restoring is: move these two files back into `momomod-manager/src/features/`, re-add
the four edits above, and rebuild. The features' own signatures are for the
2026-08-10 build — re-check them against the current build with `draw_probe` /
`guard::verify` before trusting the patches.
