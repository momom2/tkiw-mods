# Reference docs

Id vocabularies recovered from the game, for writing and checking config files. All of it is
**generated, not transcribed** — regenerate after any game update and diff:

```bash
python analysis/extract.py
```

| file | what |
|---|---|
| [reward-types.md](reward-types.md) | all 35 reward types: id, display name, whether it presents a choice, which choice path, whether it can be scrapped |
| [resources.md](resources.md) | 22 run resources + 2 meta currencies |
| [artifacts.md](artifacts.md) | 127 artifact ids with display names |
| [unit-classes.md](unit-classes.md) | the 8 troop classes and 3 training stats behind `unit_class_stat` |
| [option-vocabularies.md](option-vocabularies.md) | 49 spells, 176 improvements, 269 upgrades, 40 advisors, 305 units — the option ids for their reward types |
| [vocabulary.json](vocabulary.json) | the statically-derived lists, machine-readable |
| [live-libraries.json](live-libraries.json) | the same lists read out of the **running game**, machine-readable |

Analysis method and the evidence behind the design decisions is in
[../analysis/FINDINGS.md](../analysis/FINDINGS.md).

## These have been checked against the running game

The mod walks the game's own libraries (`REWARDS`, `RESOURCES`, `ARTIFACTS`, …) at runtime
and writes the keys to its log; `analysis/verify_live.py` diffs them against the statically
derived lists:

```
reward types   live   35   docs   35   OK
resources      live   22   docs   22   OK
artifacts      live  127   docs  127   OK
unit classes   live    8   docs    8   OK
```

So these ids are ground truth, not inference. Re-run `verify_live.py` after a game update:
where the two disagree, **the live game wins**.

## Things worth knowing before writing a config

- **Only 24 of the 35 reward types present a choice.** The rest are direct grants with
  nothing to pick, and are out of scope.
- **`unit_class_stat` cannot be scrapped**, nor can `shop`, `shop_graveyard` or
  `rewards_wheel` — only types routed through `spawn_choice_unified` get a scrap button.
- **Troop class identity is positional**, so `unit-classes.md` is the only thing tying a
  config key to a class. See the warning in that file.
- **Two resource ids display as something else**: `relic_graveyard` is "Relic" and `organs`
  is "Canopies".
- The prior hand-off note claimed "grain is `WHEAT`" and "oil is probably `FUEL`". Neither
  holds against `localization.csv`, which gives `wheat` → "Wheat" and `fuel` → "Fuel".
