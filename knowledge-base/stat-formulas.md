# Stat formulas

How the game turns base values into the numbers it displays. Verified against the live
`UNITS` library, a real `run_data.dat`, and in-game readings that agree to three
significant figures.

Build 2026-08-10.

## Sources of truth

| what | where |
|---|---|
| base stats, per unit | `UNITS` library — dump with the kit's `dump_libraries` feature |
| accumulated modifiers, per live unit | `run_data.dat` → `units[].mods{}` |
| class training totals | `run_data.dat` → `training_hp` / `training_damage` / `training_atk_speed`, keyed by class index |
| king constants | `KINGS` library |

`run_data.dat` is plain JSON with **one trailing NUL byte** — `json.load` fails with
"Extra data"; use `json.JSONDecoder().raw_decode()`.

## DPS

```
dps = attack_damage / (attack_time / 60)
```

`attack_time` is in frames at 60 fps. Verified:

| unit | attack_damage | attack_time | dps | in game |
|---|---|---|---|---|
| `griffin` | 24 | 36 | 40.00 | 40 |
| `mage_lightning` | 41 | 90 | 27.33 | 27 |

The in-game "damage" figure on a unit is **DPS**, not damage per attack.

## Modifier stacking — additive

```
total = base × (1 + Σ multiplier_mods) + Σ flat_mods
```

**Not multiplicative.** For a Lightning Mage with ~462 in multipliers, multiplicative
would give 2,221,922 HP against an observed 46,257.

Each entry in `units[].mods{}`:

| field | meaning |
|---|---|
| `stat_type` | `0` hp, `1` damage, `6` attack speed |
| `mod_type` | `0` flat, `1` multiplier |
| `mod_value` | the addend, e.g. `0.5` = +50% |
| `timer` | `"infinite"` or a duration |

### Consequence worth knowing

Because stacking is additive, a percentage upgrade is diluted by everything else in the
pool. A "+50% HP and damage" unit upgrade on a mage with a class-training pool of ~462
is worth **+0.108%**, not +50%. This is not a bug, but it is why late-game upgrades feel
inert.

## Class training

A unit receives one modifier per class in its `classes` array:

```
mod id     class_mod_<class><stat>_<stat_type>
mod_value  training_<stat>[class]            (from run_data.dat)
```

Class indices, positional:

```
0 grunt   1 rider    2 flying     3 ranged
4 arcane  5 warrior  6 champion   7 undead
```

Stat index inside the id: `0` hp, `1` damage, `6` attack speed.

Verified: `griffin classes=[1,2]` → `class_mod_10_0`, `class_mod_11_1`, `class_mod_16_6`,
`class_mod_20_0`, … `mage_lightning classes=[4,3]` → `class_mod_3*` and `class_mod_4*`.

**Dual-class units draw from both classes**, and the contributions add. A Lightning Mage
collects ranged *and* arcane training on the same stat.

## Morale

```
damage multiplier      = morale × KINGS[king].morale_damage_bonus
productivity multiplier = morale × KINGS[king].morale_productivity_bonus
```

Applied as an ordinary modifier into the additive pool (`stat_type 1`, `mod_type 1`),
under the id `morale_1`.

For `blueprint` (**King Leo the Wise**): `morale_damage_bonus = 0.25`,
`morale_productivity_bonus = 0.2`.

> **The description disagrees with the implementation.** It reads "Increases all units
> damage by 0.5% and buildings productivity by 0.25% per morale". Damage is applied at
> **half** the described rate. See
> [`../for-the-developers/king-leo-morale.md`](../for-the-developers/king-leo-morale.md).
> Trust the `KINGS` values, not the description string.

At high morale this is the dominant damage term — 67 of a Griffin's 67.8 total.

## Worked example

Griffin, King Leo run, >26k morale, training ~0.4 in rider+flying, iron helmet equipped:

```
hp   = 430 × (1 + 0.405) + 30            = 634        (observed 634)
dps  =  40 × (1 + 0.405 + 26798×0.0025)  = 2736       (observed ~2736)
```

Lightning Mage, same run, base 100 hp / 27.33 dps, +50% unit upgrade:

```
hp   = 100 × (1 + 460.77 + 0.5) + 30     = 46,257     (observed 46,257)
dps  = 27.33 × (1 + 412.87 + 67.0 + 0.5) = 12,997     (observed 12,997)
```

## Calling the game rather than re-deriving

Every unit struct carries the game's own accessors: `get_hp_max`, `get_damage`,
`get_attack_time`, `get_dps_base`, `get_dps_modified`, `get_dps_charge_base`,
`get_dps_charge_modified`. For anything player-facing, call these — they cannot drift
from the game the way a reimplementation can.

The formulas above are for understanding and for offline analysis, where calling is not
an option.
