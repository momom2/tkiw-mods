# Fortifications never reaches its cap

`upgrade_wall_repairs_fortification` is documented and configured to grant at most
100 max castle HP per run. It grants without bound. The counter that enforces the cap
is initialised, read three times, and incremented nowhere.

Build 2026-08-10. Found by static analysis of the shipped executable, confirmed against
a live library dump and a real `run_data.dat`.

## The upgrade as configured

From the `UPGRADES` library at runtime:

```
system_name          wall_repairs_fortification
title                Fortifications
upgrade_of           wall_repairs            (improv_wall_repairs, "Brick factory")
changes_required     5
max_hp_gained_cap    100
hp_gained_current    0
description          "When the HP of the castle is full the building will accumulate
                      charges. Once 5 charges is gathered, the charges are spent and
                      the max HP of the castle is increased by 1. Has a cap of 100 HP
                      earned this way."
```

## What the code does

`obj_improvement_wall_repairs`'s production-complete handler
(`anon@289@gml_Object_obj_improvement_wall_repairs_Create_0`, rva `0x13c7f60`),
reconstructed:

```gml
on_production_complete = function() {
    on_production_complete_base();
    if (is_damaged()) {
        castle_heal(amount_healed);
    } else if (upgrade_check("wall_repairs_fortification")) {
        var u = UPGRADES_EQUIPPED[? "wall_repairs_fortification"];
        if (u.hp_gained_current < u.max_hp_gained_cap) {     // 0 < 100, always
            hp_increase_charges_gained++;
            if (hp_increase_charges_gained >= hp_increase_charges_required) {
                hp_increase_charges_gained -= hp_increase_charges_required;
                increase_max_hp(1);
                // hp_gained_current is never incremented
            }
        }
    }
}
```

`increase_max_hp` (`obj_castle_wall`, rva `0x11eb640`) is `hp_max += n; hp += n`, with
no clamp of its own.

## The evidence

Every reference to the `hp_gained_current` variable slot (rva `0x2ad1808`) in the whole
`.text` section, found by scanning for `mov r32, [rip+disp32]` against the slot address
rather than by trusting a function list:

| rva | function | what |
|---|---|---|
| `0x10071a6` | `upgrades_library` | `hp_gained_current = 0` — the initialiser |
| `0x13c8432` | `anon@289@…wall_repairs_Create_0` | read, for the cap test |
| `0x13c8b19` | `anon@874@…wall_repairs_Create_0` | read |
| `0x13c97a7` | `obj_improvement_wall_repairs_Draw_0` | read, for the charge display |

Four references, one write, and that write is the initialiser. For contrast,
`hp_increase_charges_gained` has five references including a read-modify-write at
`0x13c85e3`, which is what a field that is actually maintained looks like.

The same always-true comparison gates the Draw handler, so the charge counter above the
building never stops advancing — the UI is consistent with the bug, not with the
description.

## Consequences

Each Brick Factory grants +1 max castle HP per 5 completed cycles, indefinitely, for as
long as the castle is undamaged. `cycle_time` is 300 frames, so one factory yields
roughly +1 HP per 25 seconds of undamaged production, and factories accumulate charges
independently — two factories double the rate.

`castle_hp` **is** persisted (`{is_stone, hp, hp_max}`), so the gain compounds across
sessions for the whole length of a run. Base castle `hp_max` is 100 (`obj_castle_wall`,
rva `0x11ec924`).

Observed on a 6047-day endless save with two Brick Factories: `hp_max = 1032` against a
designed ceiling of 200 from this source. Other things also call `increase_max_hp`
(artifacts, advisors, encounters), so not all 932 is attributable, but the magnitude and
the growth rate during play both match.

## A second, latent defect

`run_data.dat` stores `upgrades` as a flat array of system names with no per-upgrade
state. Adding the missing increment alone would therefore cap the gain per *session*
rather than per run: the counter returns to 0 on every load, since `upgrades_library`
re-initialises it at boot. A complete fix needs the counter in the upgrade's save data
as well as the increment.

## Suggested fix

Two changes:

1. `hp_gained_current++` alongside the `increase_max_hp(1)` call.
2. Persist `hp_gained_current` through the upgrade's `get_save_data` / `apply_save_data`.

Consider also whether the description's "Has a cap of 100 HP earned this way" is still
the intended balance — the upgrade has presumably only ever been played uncapped.

## A variant worth considering: cap per Brick Factory

Raised by a player, and recorded here because it is a design option rather than a bug
report.

The cap as designed is a flat 100 per run, regardless of how many Brick Factories are
standing. An alternative is to make the ceiling scale with them:

```
reachable = non_brick_max_hp + 100 x (Brick Factories currently in the castle)
```

so each Brick Factory raises the ceiling Fortifications may fill, rather than merely
filling a shared one faster. Two factories could contribute 200 in total; one could
contribute 100.

The asymmetry is the interesting part. **Destroying a Brick Factory lowers the ceiling
but does not lower current max HP.** So HP already earned is kept, and the player simply
cannot earn more until the ceiling is above the current maximum again. That avoids the
two bad outcomes: no retroactive punishment for removing a building, and no way to farm
the cap by building, filling, demolishing and rebuilding.

Compared with the flat cap this makes a second factory worth building for its
Fortifications value rather than only for its repair rate, which is closer to how the
rest of the building upgrades read. It costs one more thing to track: the ceiling is a
function of live building count, so it has to be recomputed when buildings appear and
disappear rather than fixed at the start of a run.

Noted as an option, not a recommendation. Whichever is chosen, the counter still has to
be incremented and still has to be saved.
