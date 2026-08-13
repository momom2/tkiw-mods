# King Leo: morale bonuses do not match their description

Build 2026-08-10. Read from the live `KINGS` library and confirmed against a player's
run.

## The discrepancy

`KINGS["blueprint"]` (King Leo the Wise):

| field | value |
|---|---|
| `morale_description_default_value` | "Increases all units damage by 0.5% and buildings productivity by 0.25% per morale" |
| `morale_damage_bonus` | **0.25** |
| `morale_productivity_bonus` | **0.2** |

Damage is applied at **half** the described rate. Productivity at 80% of it.

## Confirmation from play

A player with >26,000 morale, Griffins (`hp_max` 430, `attack_damage` 24,
`attack_time` 36 → 40 dps base), observed **2,736** damage.

Modifiers stack additively: `total = base x (1 + sum of multipliers) + flat`.

```
observed multiplier            = 2736/40 - 1      = 67.400
minus rider+flying training    = 67.400 - 0.405   = 66.995   from morale
```

| rate | implied morale |
|---|---|
| 0.5% (as described) | 13,399 |
| **0.25% (as implemented)** | **26,798** |

The player's stated morale is >26k, matching the implemented value.

## Effect

At high morale this is the dominant damage multiplier — for the Griffins above it is
67 of the 67.8 total. Halving it halves late-game damage against what the description
promises.

Most likely a balance change where the value was updated and the description string was
not. If the values are intended, the description needs updating; if the description is
intended, `morale_damage_bonus` should be 0.5 and `morale_productivity_bonus` 0.25.

## Related

The same run shows a second, separate effect worth being aware of: **percentage upgrades
are diluted to nothing at high class-training totals**, because everything is additive.
A "+50% HP and damage" unit upgrade contributed **+0.108%** to a Lightning Mage whose
class-training pool was ~462. Not a bug in itself, but the sort of thing that makes an
upgrade feel broken to a player. See `performance.md`'s sibling analysis in the mod
repository for the working.
