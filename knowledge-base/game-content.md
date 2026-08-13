# The game's content, as the game holds it

Counts are from the 2026-08-10 build. Read them from the live game rather than
copying them — the shapes are stable, the contents are not.

## Libraries

Global `ds_map`s, keyed by system name string, values are structs. They exist
once the game has started; you do not need to be in a run.

| global | entries | notable fields |
|---|---|---|
| `ARTIFACTS` | 127 | `tier` (1 ordinary, 2 legendary), `title_default_value`, `unlocked` |
| `SPELLS` | 49 | `tier`, `title_default_value` |
| `IMPROVEMENTS` | 176 | `excluded_from_drop_pool`, `title_default_value` |
| `UPGRADES` | 269 | `upgrade_of` (the improvement it belongs to) |
| `RESOURCES` | 22 | includes a `random` marker that is not a real resource |
| `UNIT_CLASSES` | 8 | keyed by **integer index**, not by name |

Two grouping maps carve up the improvements. Both have **numeric** keys and
array-of-string values:

* `IMPROVEMENTS_BY_CATEGORY` — `0` production, `1` troops, `2` attacking,
  `3` misc, `4` special, `5` terrain. Categories 4 and 5 are entirely
  `excluded_from_drop_pool`, which is what confirms the mapping.
* `IMPROVEMENTS_BY_TIER` — `1`, `2`, `3`.

45 of the 176 improvements are `excluded_from_drop_pool` and can never be
offered as a reward. Do not filter these by hand; ask the game.

The nine infernal barracks are the `imp_*`-prefixed members of the troops
category.

## Rewards

35 reward types exist; 25 present a card choice. The ones a mod can drive:

```
unit_class_stat   resource          artifact     artifact_legend
spell             spell_legend      upgrade
improvement_production_t1..t3       improvement_troops_t1..t3
improvement_infernals               improvement_infernals_t1..t3
improvement_misc                    improvement_attacking
```

Not card choices, and not drivable the same way: `shop`, `prophecy`,
`rewards_wheel`, `onaraks_favour`. And `run_start_bonus`, whose cards carry no
identifier at all.

### Cards

Option cards are separate objects per type — `obj_card_resource`,
`obj_card_artifact`, `obj_card_spell`, `obj_card_improvement`,
`obj_card_upgrade`, `obj_card_class_stat_bonus`, `obj_card_start_bonus`.

**Identify a choice by the card object on screen, not by the queue entry's
declared `reward_type`.** When a type's candidates are exhausted the game offers
resource compensation instead, so the entry can say `artifact` while resource
cards are on screen.

Each card holds a `*_contained` member pointing at the library struct; the id is
that struct's `system_name`. Troops Training is the exception: it holds an
**array** of `{stat_type, unit_class, stat_amount}`, where `unit_class` indexes
`UNIT_CLASSES` and `stat_type` is 0 (hp) or 1 (damage).

Cards hold placeholder values for a few frames after they appear. Wait until
every card in the choice is built before reading any of them — a card's
`select_button` reading as a plain number rather than an instance reference is a
reliable "not ready yet".

### Unit classes

Positional, by index:

```
0 grunt   1 rider    2 flying     3 ranged
4 arcane  5 warrior  6 champion   7 undead
```

`UNIT_CLASSES_LENGTH` is 8. A card names its class by *index*, so any config or
id scheme has to use the same positional list — the names cannot be read out of
the `UNIT_CLASSES` map, whose keys are those same indices.

A Troops Training choice **mixes classes and stats freely**: three cards can be
flying-hp, rider-damage, ranged-damage. Only hp and damage are ever offered;
attack speed exists as a unit-class mod but is never a reward.

> **The goblin sprites on Troops Training cards do not match the choices.** This
> is a long-standing game bug the developers never fixed. The stated choices and
> the "units affected" preview are both correct; only the sprite lies. Do not
> build a cross-check against the sprite — it will contradict a correct decode
> and send you chasing a bug that is not yours.

## Rerolls

Three sources, spent in this order:

1. **Voodoo Beads** — a per-reward freebie, `free_rerolls_per_reward_left` on
   the reroll button. Distinct from the run pool.
2. **The reign-wide free pool** — `FREE_REROLLS_PER_RUN_LEFT`.
3. **Denarii** — paid.

`non_free_rerolls_made` counts the paid ones for the current reward.

### Costs

A cost is a struct `{type: "coin", amount: 10}`. The reroll button holds:

* `cost_initial` — one such struct
* `cost_increase_per_reroll` — one such struct
* `resource_cost` — an **array** of them, since a price can name more than one
  resource. This is the current price, maintained by the game.

`resolve_reroll_cost` recomputes `resource_cost` and returns nothing. Call it for
the effect, then read the member.

The player's balances live on `obj_gameplay_controller.resources`, keyed by
resource id. `coin` is denarii. Read it by name through `variable_struct_get` —
it has no compile-time variable id.

## The reward queue

`obj_gameplay_controller.pending_rewards_list` is a `ds_list`. Strict FIFO; the
head is the next reward. `obj_button_reward_queue` opens it.

No queued reward is two-step: opening one always leads directly to a card choice
or to nothing.

## Scrapping

Scrapping a reward gives 5 denarii. `obj_button_rewards_scrap` is present when
it is available — observe it rather than inferring it from the type. Troops
Training has no scrap button.

## The full library dump

`tkiw-momomod-kit` has a `dump_libraries` feature that writes every content library to
JSON from a live process, one shot, at the main menu. It is the authoritative source for
per-entry values and supersedes reading them out of the disassembly.

```
UNITS 305   IMPROVEMENTS 176   UPGRADES 269   ARTIFACTS 127   SPELLS 49
RESOURCES 22   ADVISORS 40   KINGS 12   ASCENSIONS 74   CHALLENGES 66
ENCOUNTERS 84   LEVELS 6   UNIT_CLASSES 8
```

Collect it unattended:

```bash
python knowledge-base/tools/playtest.py --log tkiw-momomod-kit/momomod.log \
    --until "dump_libraries] wrote" --timeout 240
```

Unit fields of note: `hp_max`, `attack_damage`, `attack_time` (frames),
`attack_radius`, `attack_action_frame` (array), `attack_img_speed`, `classes` (array of
class indices), `tier`, `faction`, `engage_speed`. Each unit also carries the game's own
accessors — `get_hp_max`, `get_damage`, `get_dps_base`, `get_dps_modified`,
`get_attack_time` — which are the right things to call rather than re-deriving.

**DPS = `attack_damage / (attack_time / 60)`.** Verified: griffin 24/(36/60) = 40,
mage_lightning 41/(90/60) = 27.33, both matching the in-game display.

Two things the walker must handle, both learned by getting them wrong: `UNIT_CLASSES`
is keyed by **integer**, not by name, so a string-only walk reports it empty; and
several of the most useful fields are **arrays** (`classes`, `attack_action_frame`).

## The save files

`%LOCALAPPDATA%\The_king_is_watching_steam\Release\`. Each has a `.prev` beside it,
which is the previous write and a free backup.

| file | holds |
|---|---|
| `run_data.dat` | the run in progress |
| `savedata.dat` | meta progress |
| `player_meta_progress.dat` | meta progress |
| `player_challenges.dat` | challenge state |
| `prefs.dat` | settings |

All are JSON with **one trailing NUL byte**. `json.load` fails with "Extra data"; use
`json.JSONDecoder().raw_decode(text)` and ignore the remainder.

`run_data.dat` top level:

```
advisors  artifacts  ascension  banishment_data  barracks_state  bodies
boss_mods_full  castle_hp  cells_castle  challenges  custom_data  custom_mods
debug_mods  days_of_reign  earned_so_far  encounters_happened
endless_waves_generated  game_stage  gaze_data  improvements_hand  is_endless
king  king_abilities  king_quests  level  level_state  limits_upgrade
pending_rewards  prophecies_opened  refund_data  resources  run_rerolls_left
spell_slots  timeline_events  training_atk_speed  training_damage  training_hp
units  upgrades  vision_upgrade  waves_defeated
```

### What the schema does not carry

Worth knowing before diagnosing any "it resets when I reload" report, since the answer
has twice been "the field is not in the save":

- **`units[]` carries seven fields only** — `hp_portion`, `in_barracks`,
  `is_time_limited`, `life_time_left`, `mods`, `name`, `state`. Every other instance
  variable is rebuilt by `obj_unit_parent_Create_0` at its default. Status flags
  attached to a unit (`affected_by_timed_charge`, for one) do not survive a reload.
- **`upgrades` is a flat array of system names**, with no per-upgrade state. An upgrade
  that accumulates something in its library struct loses it on every load.
- **`king_abilities[]` carries `{level, cooldown, wild_data}`** — one free scalar per
  ability, and nothing else.
- **`improvements_hand[]` carries `{improv_name, building_charges}`**.

`castle_hp` *is* carried, as `{is_stone, hp, hp_max}`, so anything that raises max
castle HP compounds across sessions for the whole run.
