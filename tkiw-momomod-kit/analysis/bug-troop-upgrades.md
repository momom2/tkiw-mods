# Troop upgrades and Griffins

Investigated from the executable and from a real `run_data.dat` (endless Blueprint run,
day 5345, 67 waves, ascension 2). No live session required — the save is plain JSON with
one trailing NUL byte, and it stores every stat modifier per unit.

## How stat modifiers are stored

`run_data.dat` → `units[]` → `mods{}`. Each entry:

```json
"class_mod_41_1": { "stat_type": 1, "mod_type": 1, "mod_value": 157.95, "timer": "infinite" }
```

| field | meaning |
|---|---|
| `stat_type` | `0` hp, `1` damage, `6` attack speed |
| `mod_type` | `0` flat, `1` multiplier |
| id `class_mod_<C><S>_<stat>` | `C` = class index, `S` = stat index |

Class indices: `0` grunt, `1` rider, `2` flying, `3` ranged, `4` arcane, `5` warrior,
`6` champion, `7` undead. Values match `training_hp` / `training_damage` /
`training_atk_speed` in the same file exactly.

**A unit carries one `class_mod` per class it belongs to.** Observed:

| unit | classes | hp mods | damage mods |
|---|---|---|---|
| `mage_lightning` | 3 ranged, 4 arcane | 109.75, 132.75 | 122.35, 157.95 |
| `mage_healer` | 4 arcane | 132.75 | 157.95 |
| `griffin` | 1 rider, 2 flying | 0.15, 0.15 | 0.15, 0.15 |
| `assassin` | 0 grunt | 0.95 | 0.15 |

## Finding 1: Griffins are not bugged on this save

`training_*` on this save is concentrated almost entirely in two classes:

```
training_hp     ranged 109.75   arcane 132.75   every other class 0.15–0.95
training_damage ranged 122.35   arcane 157.95   every other class 0.15–0.25
```

Griffins are **rider + flying**, so they receive rider and flying bonuses — correctly,
and the save shows the mods present with the right values. Those classes have ~0.15
invested against ~130 for ranged/arcane, a factor of roughly 800.

So "Griffins don't benefit from troop upgrades" is, on this save, "Griffins benefit from
the classes they belong to, and nothing was invested in those classes". The auto-picker
config drove all training into `arcane.*` and `ranged.*`.

There is an upgrade for exactly this: **`lion_circus_all_class`** —

> *Griffins receive bonuses from all troop classes and are counted as all-class. Their
> cost to produce is increased by {&1}%*

It is **not** among the 23 upgrades equipped on this save. Griffins come from
`improv_lion_circus` ("Spawns Griffins"); `obj_improvement_griffin_stable` also exists.

**Unresolved:** the reported claim was that a *king ability* summons Griffins "with all
unit upgrades". No king is named Leo — the ten are `blueprint, cannon, cleopatra,
diversity, glass, magic, masked, necro, starter, tanky`. The save's king is `blueprint`,
whose second ability reads *"Summon a unit to fight for you (unit type is upgraded)"* —
which plausibly means "a higher-tier unit type", not "all upgrades applied". Needs the
player to identify which ability they meant before this can be checked.

## Finding 2: dual-class units draw from both classes

`mage_lightning` is both ranged **and** arcane, so it receives `class_mod_30/31`
*and* `class_mod_40/41`. Investing in both classes stacks on the same unit.

Whether that is intended is a design question, but it is worth noting that the two
largest investments on this save are exactly the two classes a Lightning Mage belongs
to.

## Finding 3: the open question — additive or multiplicative?

Full modifier set for a `mage_lightning` on this save:

```
mod_id                                   stat  type            value
class_mod_40_0                             hp     1         132.7500
class_mod_30_0                             hp     1         109.7500
artifact_iron_helmet_0                     hp     0          30.0000
upgrade_mage_lightning_hp_and_damage_0     hp     1           0.5000
class_mod_41_1                            dmg     1         157.9500
class_mod_31_1                            dmg     1         122.3500
morale_1                                  dmg     1          49.7225
upgrade_mage_lightning_hp_and_damage_1    dmg     1           0.5000
```

Two candidate combinations, orders of magnitude apart:

| | additive `1 + Σv` | multiplicative `Π(1+v)` |
|---|---|---|
| HP | **×244.00** (+30 flat) | ×22,219 |
| damage | **×331.52** | ×1,491,735 |

**This is the check to run.** Read a Lightning Mage's displayed HP and damage in game
and divide by its base. If it matches neither figure, the discrepancy is the bug and its
size says where to look.

Static analysis did not settle it: `Stats_mods.calculate_stat` is where the combination
happens, but its arithmetic is generic RValue work (`++` over an iteration) rather than a
readable formula, so the aggregation is in `update_stat` / `total_stats` and would need
more disassembly than the in-game check costs.
