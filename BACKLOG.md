# Backlog

Grouped by **what is blocking each item**, not by feature area — because the useful
question when picking up work is "what can I actually do right now".

Status: `[ ]` not started · `[~]` partly done · `[x]` done

---

## Done

- [~] **Faster startup** — **claim withdrawn.** The original "~30% (39s -> 23-27s)" came
      from single launches. Measured properly with `timeit.py`, four launches each way:
      `fast_boot` on gives median **48.7s** (spread 18.8s), off gives median **52.4s**
      (spread 5.9s). A 3.7s median difference against 19s of noise is not a result. The
      feature stays on because the median is the right way round and it costs nothing,
      but nobody should claim a number for it until a larger batch says one.
      Cause: `texture_prefetch` called from `obj_init_Create_0`, 26 s of CPU-bound
      texture decoding before anything is on screen.
- [x] **Faster in-reign speed with lots of production.** It was **not** the units: the
      floating resource-gain popups rebuild their Scribble text every frame, because the
      fade value is baked into the string and the string is the cache key. Feature
      `popup_stutter_fix`. The stall share stops climbing (was 11%→43% over a run, now
      flat at 9–13%) and the allocator leaves the profile entirely.
- [x] **Reward auto-picker** — the mod itself. Shares `tkiw-runtime`; still ships
      separately (see below).

Measurements: [`tkiw-momomod-kit/analysis/FINDINGS.md`](tkiw-momomod-kit/analysis/FINDINGS.md).
Upstream write-up: [`for-the-developers/performance.md`](for-the-developers/performance.md).

**Not fixable here, established by measurement:** the main-menu lag spikes and the
stalls remaining after the two fixes above are `Present` blocking in the graphics stack
on integrated graphics, with the Steam overlay in the same path. No game-side frames at
all. Advice for that is outside a mod — frame cap, or disabling the overlay for the game.

---

## Next: bugs — nothing blocking these

All of these are readable from the executable. No drawing, no playtime needed to
investigate; a fix may need a code patch, which the kit can now do safely.

- [x] **Brick Factory + Fortifications increases castle HP without bound. Confirmed —
      a real bug, and a plain one.** `wall_repairs_fortification` is configured with
      `max_hp_gained_cap = 100` and gates each grant on `hp_gained_current <
      max_hp_gained_cap`. `hp_gained_current` is set to 0 by `upgrades_library` and
      **incremented nowhere**: an exhaustive byte scan of `.text` for its variable slot
      (`0x2ad1808`) finds four references, of which one is the initialiser and three are
      reads. The gate is therefore always open, and each factory grants +1 max castle HP
      per 5 cycles forever. `castle_hp` is persisted, so it compounds for the whole run —
      the examined save reads `hp_max = 1032` against a base of 100 and a designed
      ceiling of 200 from this source. A second, latent defect: `run_data.dat` stores
      `upgrades` as bare names with no per-upgrade state, so the counter would reset each
      session even once incremented. Reported in
      [`for-the-developers/brick-factory-fortifications.md`](for-the-developers/brick-factory-fortifications.md).
      **Fixing it in the kit is a decision, not a task — see below.**
- [x] **Gryphons do not get all troop upgrades.** **Answered — not a bug.** Largely explained, and **not a bug on
      the save examined**: Griffins are rider+flying, and that save's training is ~130 in
      ranged/arcane against ~0.15 everywhere else. The upgrade that changes this is
      `lion_circus_all_class` ("Griffins receive bonuses from all troop classes"), which
      is not equipped. Confirmed from the live `UNITS` library: `griffin classes=[1,2]`
      (rider, flying), `mage_lightning classes=[4,3]` (arcane, ranged). King Leo **is**
      the `blueprint` king (`title_default_value` = "King Leo the Wise"); his second
      ability *Extra Draft* picks a unit type from `BLUEPRINT_HIERARCHY` indexed by
      ability level — "unit type is upgraded" means a better tier, not "carries all unit
      upgrades". It routes through `spawn_player_unit`, so summons get the normal
      triggers.
- [x] **Troop stats — something is not adding up. Found it, and it is a real bug.**
      Stat modifiers are **additive** (`base × (1 + Σv) + flat`), confirmed against two
      units. King Leo's morale bonus is described as "0.5% damage per morale" but
      `KINGS["blueprint"].morale_damage_bonus` is **0.25** — half. Independently
      confirmed: the observed Griffin damage implies 26,798 morale at 0.25%, matching the
      player's >26k, where 0.5% would need 13,399. Productivity is 0.2 against a
      described 0.25%. Reported in
      [`for-the-developers/king-leo-morale.md`](for-the-developers/king-leo-morale.md);
      working in [`tkiw-momomod-kit/analysis/bug-troop-upgrades.md`](tkiw-momomod-kit/analysis/bug-troop-upgrades.md).
- [~] **Save-and-quit with King Brezhnius clears explosive charges.** First look done,
      and the cause is almost certainly the save schema. Brezhnius is `KINGS["tanky"]`;
      the ability is `obj_king_ability_4_3_timed_charge`, and a charge is recorded as
      `affected_by_timed_charge` **on the target unit** (set in
      `anon@361@…timed_charge_Create_0`, consumed in `obj_unit_parent`'s Step/Destroy).
      A saved unit in `run_data.dat` carries exactly seven fields — `hp_portion`,
      `in_barracks`, `is_time_limited`, `life_time_left`, `mods`, `name`, `state` — so
      the flag has nowhere to go, and units are rebuilt on load with
      `obj_unit_parent_Create_0`'s default. `king_abilities` saves only
      `{level, cooldown, wild_data}`, so the ability cannot carry it either.
      **Caveat:** inferred from the serialiser's field list, on a save from a Leo run
      with no charges on it. Confirm on a Brezhnius save before reporting upstream.

- [ ] **Percentage upgrades are diluted to nothing at high training totals.** Additive
      stacking means a "+50% HP and damage" unit upgrade was worth **+0.108%** on a
      Lightning Mage with a class-training pool of ~462. Not a bug as such, but it makes
      upgrades feel broken. Worth raising with the devs as a design note.


### Fortifications: three behaviours, one built

- [x] **Stop granting at the cap.** Shipped as `[feature.fortifications_cap]` in the QoL
      mod, **default off** because it takes power away from a save that already has it.
      A `call rel32` at `0x13c860d` into a counting stub, and a 9-byte `jmp` over the
      gate at `0x13c8542` once the count reaches `cap` (default 100), reverted on
      returning to the menu. No arithmetic is needed to find "non-brick max + 100":
      `hp_max == non_brick_max + granted`, so the condition is just `granted >= 100`,
      and counting stays correct if the game ever adds another source of castle HP.
      **Verified as far as it can be without a long run:** both signatures match, the
      stub installs, the game reaches the menu with it applied, and the encoding is unit
      tested. Not yet verified in anger — that needs ~42 minutes of undamaged production
      to reach 100 grants.
- [x] **Gate on the castle, not on a private counter. Done, and exact.** The old version
      counted its own grants, which is right only for a run started after it loaded; on an
      existing save it allowed another hundred on top of the nine hundred already there.
      It now reads the castle and compares against what the run could legitimately have:

      | source | amount | read from |
      |---|---|---|
      | `obj_castle_wall` | 100 | always |
      | stone castle | 20 | `is_stone` |
      | advisor `branimir` | 100 | `ADVISORS_EQUIPPED` |
      | artifact `emerald_shield` | 30 | `ARTIFACTS_EQUIPPED` |
      | encounter `happy_accident` | 40 | `ENCOUNTERS_HAPPENED` |
      | encounter `they_came_unprepared` | 30 | `ENCOUNTERS_HAPPENED` |

      The two encounters were the open question and are now closed. They carry
      `castle_max_hp_given` on an entry of their `options` array, which is why an 84-entry
      dump found nothing: `dump_libraries` walked one level. It now descends into structs
      **and into struct elements inside arrays**, which is what surfaced them.

      No counter, no code cave, no history: one instance read and three list lookups per
      tick. Checked against the real save — ceiling 280 (100 base + 20 stone + 30 Emerald
      Shield + 30 they_came_unprepared + 100 cap) against 1032 actual, so it stops at once.

      **Not yet seen firing in a live run.** `playtest.py` reaches the main menu and the
      castle only exists inside a run, so activation and the arithmetic are verified but
      the patch going in during play is not.

- [ ] **Cap per Brick Factory rather than per run.** `non_brick_max + 100 x (factories
      currently in the castle)`, so each factory raises the ceiling Fortifications may
      fill instead of filling a shared one faster. Destroying a factory lowers the
      ceiling but **not** current max HP — which is what stops build-fill-demolish-rebuild
      from farming the cap, and stops removing a building from punishing the player
      retroactively. Documented for the devs in
      [`for-the-developers/brick-factory-fortifications.md`](for-the-developers/brick-factory-fortifications.md).
      In the kit this is a change to what the counter is compared against, plus tracking
      the live factory count, so it builds on what is already there.


### Fast boot and fonts: a real cost, an unproven saving

Timing `texture_prefetch` one group at a time, from the menu, with the boot prefetch
suppressed:

```text
default                     0 ms      <- every sprite in the game
__yy__0fallbacktexture      1 ms
font_lat                  846-1017 ms
font_cyr                 1621 ms
font_kr                  4051 ms
font_jp                  9933 ms
font_chi                11002 ms
```

**The game's art is free; ~26.5s is glyph atlases.** That part is solid — each figure is
a direct measurement of one call.

Shipped as `[feature.font_atlases]` in the QoL mod: one switch per script, Latin
included on the same terms (only its default differs, since it is what the menu is drawn
in). It provably declines the ones you switch off — the log names each.

**It is off by default, because the saving is not demonstrated.** First A/B, one run
each: 51.5s to the menu with it on, 41.5s with it off. Run-to-run spread on the same
machine was 36-56s, so a single pair proves nothing either way — but it certainly does
not prove a win.

- [~] **Settle it with repeated runs.** `knowledge-base/tools/timeit.py` now does this:
      launches N times, reads the main-menu timestamp from the timeline, reports every
      run plus median and **spread**. Flipping the setting between batches is left to the
      caller so that what is being compared is explicit. Until a batch has been run each
      way, every startup number in this repository — including `fast_boot`'s "25%
      faster" — rests on single runs and should be treated as a hypothesis.
- [ ] **Test the structural doubt.** `texture_prefetch` *uploads* an atlas the game has
      already built. Building it is `GENERATE_FONTS` / `USE_DYNAMIC_TEXTURES_FOR_FONTS`
      in `obj_init` and `__scribble_font_add_from_project` (8.3% of the init window),
      which run whether or not anything prefetches. If so, declining the upload saves
      nothing and merely moves it to first draw — which would explain the A/B, and would
      mean the atlas *generation* is the thing to attack.
- [ ] **Name the two hot functions.** `sub_1c9fd30` (28.5% of startup samples) and
      `sub_1ca3ab0` (17.2%) — 45.7% between them, and ~73% of startup is inside the
      game's own code rather than waiting on disk or GPU, so this is computation. Both
      are past the end of the GML symbol table, and attributing them by nearest symbol
      gives nonsense. Identifying them is what turns "fonts are expensive" into a report
      the developers can act on.


---

## Blocked: the kit cannot draw yet

Four items share one dependency. Nothing else stands in their way.

- [ ] **Unit stats on mouseover** — range, attack rate, damage per attack, attack
      animation delay. The fields exist on `obj_unit_parent`: `attack_radius`,
      `attack_time` (with `attack_spd_multi`), `attack_action_frame`, `attack_img_speed`.
- [ ] **Modified production speed on building mouseover**, with every modifier applied.
      Consider showing `resource/s` rather than `s/resource` for late Leo saves, where
      the latter goes uselessly small.
- [ ] **Modified spell damage on spell mouseover.**
- [ ] **Production building replacement** — queue a building over an expiring one,
      auto-build on expiry, cancellable. Only the greyed-out-blueprint part needs
      drawing; the queue and the auto-build do not.

### The dependency, and why it is now cheap

The kit's only foothold is the `PeekMessageW` hook, which runs *after* a frame is
drawn, so it cannot draw. The way through is a detour into a Draw event.

**`popup_stutter_fix` already built most of it**: a code-cave allocator that lands
within `call rel32` reach, verified byte patching with revert, and a proven `call` from
inside a live Draw event into a stub. What remains is making the stub call back into
Rust with registers preserved, and calling the game's own text drawing from there.

Groundwork, objects and variables per feature:
[`tkiw-momomod-kit/analysis/gameplay-features.md`](tkiw-momomod-kit/analysis/gameplay-features.md).

**One hazard is unresolved** and should be settled before sweeping any instances:
reading a field off an instance that does not have it may raise a fatal dialog rather
than returning undefined. See the same document.


## Wanted: more time-speed options

- [ ] **10x and 25x time speed, and a separate combat time speed.** Needs design
      discussion before any code: the question is what "faster" is allowed to change.
      A speed multiplier that scales a per-frame step is not behaviour-preserving —
      anything that counts frames rather than time (attack animations, `attack_time`,
      production `cycle_time`, spawn timers) drifts against anything that does not, and
      collision and projectile travel resolve differently at coarser steps. The two
      honest implementations are running the game's whole step function N times per
      frame, which is exact but costs N times the CPU and is bounded by what the machine
      can do, and scaling a global `delta`-like factor, which is cheap and changes
      outcomes. Which one is acceptable, and whether combat may differ from the base
      game, is the design call to make first. Related: the profiler already says where
      the per-step time goes, so the cost of the exact version is measurable before it
      is built.

---

## Kit infrastructure

Valuable tidying; nothing depends on any of it.

- [x] **One config for everything.** Done: `config/` holds one file per mod, each the
      same document that mod would read shipped alone, plus `config/momomod.ini` for the
      kit — which mods to load, and a **mirror** of every mod's settings, refreshed from
      their files on each read and commented out so that uncommenting a line is an
      override the player made on purpose. A pre-`config/` `momomod.ini` is migrated
      rather than reset.
- [x] **Mods split by kind.** `qol` (fast_boot, popup_stutter_fix) and `bugfixes`
      (morale_fix, fortifications_cap) are separate mods with separate files, so a player
      can take the pleasantness without the rule changes.
- [x] **A window to set it up.** `configure.py`: one tab per mod, in the order the kit
      lists them, checkboxes for booleans, spin and text boxes otherwise, scrollable, an
      Apply button. No schema of its own — the widget comes from the value in the file
      and the help text from the comment above it, so a new feature appears with its own
      documentation and no change here. Writes are line-surgical, so comments survive.
- [x] **Absorb the auto-picker.** Done, by **linking rather than copying**: the crate
      is an `rlib` the kit depends on, and the kit supplies the lifecycle it used to
      carry itself (frame hook, re-entry guard, panic boundary, budget, log). It is the
      `reward-picker` mod, reading `config/reward-picker.ini`.

      The standalone `version.dll` is no longer built — its `DllMain` and proxy exports
      are behind a `standalone` feature nothing enables, because two `DllMain`s in one
      DLL do not link. `uninstall.py` still works, for removing an existing install.

      **Only one copy may ever act**, since two would press twice on one reward screen.
      Two mechanisms, because one cannot see the past: a named kernel claim (exact, but
      only taken by builds made after it existed) and a scan of the game folder's DLLs
      for the picker's stamp marker (works against every installation already out there;
      cannot match the hosted copy, whose stamp is compiled out). The hosted copy yields;
      the standalone wins, being the one whose config the player tuned. Verified live
      both ways — refused to start with `version.dll` present, ran once it was removed.

      Its config file is **not in the kit's dialect**: tiers and weights are generated
      from the live game's option lists. `ModInfo.self_configuring` marks that, and the
      kit neither generates, overwrites, nor parses it. It parsed it once and reported a
      thousand syntax errors, which is how that flag came to exist.

      **Two things the absorption changed, both deliberate.** The picker's own crash
      breadcrumb (`probe.incomplete`, and the "wait for recovery" pause after a fault)
      no longer runs — the kit has its own crash reporter and breadcrumb, and two of
      them fighting over one process is worse than either. And `Ctrl+Alt+P` is still the
      picker's own key handler rather than going through the kit; harmless today since
      it does not collide with `Ctrl+Alt+M`, and it folds into the rebindable-hotkey
      item below.

      Verified end to end on a real launch: 13,132 symbols resolved, every required
      function and variable found, read path confirmed 4/4 against known globals, object
      registry read. The first hosted attempt ran 200 seconds doing **nothing**, because
      gating out `DllMain` also gated out the probe that sets `STATE` — hence
      `hosted_start()`, and hence checking rather than trusting the "on" line.

- [ ] **Rebindable hotkeys.** Currently fixed: Ctrl+Alt+P (auto-picker), Ctrl+Alt+M
      (kit). Config work, no game interaction.
- [ ] **Shared minimal install/uninstall UI** for every mod at once.
- [ ] **Keep track of which of my mods are installed** — an install script that reports
      the whole picture rather than one mod at a time.

---

## Known issues

- [ ] **Auto-picker: rare crash after long runs.** Now caught *with* a phase for the
      first time: a use-after-free of the **source** of an RValue copy, during a select
      press. Open question that decides the fix: is the stale RValue one the mod handed
      the game, or one the game already held? Evidence in
      [`tkiw-reward-auto-picker/analysis/FINDINGS.md`](tkiw-reward-auto-picker/analysis/FINDINGS.md).

---

## Ideas not yet committed to

- Shorten the splash screens. **Do not simply skip them** — they are a loading screen
  with a logo over it, doing a real sprite warm-up in their Draw event. The narrower
  version is to keep the warm-up and drop only the part of the wait that is the jingle
  finishing. See FINDINGS.
- Font generation at boot: `obj_init_Create_0` sets `GENERATE_FONTS` and
  `USE_DYNAMIC_TEXTURES_FOR_FONTS`, and `__scribble_font_add_from_project` is 8.3% of
  the init window. **Confirmed and quantified** — see "what it actually skips is other
  languages' fonts" above: `font_chi` 11.0s, `font_jp` 9.9s, `font_cyr` 1.6s, against
  0ms for every sprite in the game. Promoted out of ideas and into a real item there.
