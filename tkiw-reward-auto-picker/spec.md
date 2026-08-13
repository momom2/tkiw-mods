# Spec: config-driven auto reward picker for "The King is Watching"

Build target: an injected DLL that resolves queued in-run rewards on the player's behalf,
according to a hand-edited config file, and does nothing at all to any reward type the
config does not cover.

Status: behaviour is settled, and **the config format is now final** — every fact it depended
on has been established (§10). The id vocabularies are confirmed against the running game.
The runtime layer is built and proven, entirely read-only: nothing has ever been written to
the game. What remains is wiring the decision algorithm to the live choice, and one live
session to watch a reward being resolved before anything is allowed to act.

---

## 1. Purpose

During a run the player accumulates a queue of rewards, resolved one at a time in
acquisition order. Each offers ~3 options; for each the player may pick one, reroll the
options (costing denarii, escalating within a single reward), or scrap the reward for
5 denarii.

Most of these choices are mechanical. This mod resolves the mechanical ones from a
declared preference file so the interesting ones stay manual.

## 2. The contract

The mod is **pure quality of life**. It must never do anything the player could not have
done themselves at that same moment. Concretely:

1. **Strict FIFO.** Only the reward at the head of the queue is ever touched. The mod never
   reaches past an unresolvable reward to get at one behind it. The queue is re-read after
   every resolution, never snapshotted — resolving one entry can append others.
2. **Decide from what is on screen.** The choice is resolved from the option cards actually
   present, not from the queue entry's declared `reward_type`. The game can offer resource
   compensation in place of a type whose candidates are exhausted, so the two do not always
   agree, and acting on the declared type would apply the wrong preferences to a real choice.
3. **Real economy.** Rerolls obey real availability, the real escalating price, and real
   affordability. No reroll is conjured, no price skipped.
4. **RNG neutrality.** Tie-breaking between equally weighted options is the *mod's*
   decision, not a game event, and draws from the mod's own generator — never the game's.
   Otherwise every subsequent roll in the run would shift and a seeded run would diverge
   from its unmodded self. Mod-triggered *rerolls*, by contrast, legitimately consume game
   RNG, because a player pressing reroll would have consumed it too.
5. **No banishing.** Banishing an option is a run-long strategic commitment, not a chore.
   Out of scope.
6. **Fail closed, and stay closed.** Any uncertainty — an unknown option id, an unresolvable
   symbol, a malformed config — does not just skip the reward: the mod **switches itself off
   for the rest of the session** and logs why. Meeting an option the config does not classify
   means the config no longer describes the game, so continuing would be guessing, and a
   wrong auto-pick is worse than no auto-pick.

   Off is dormant, not dead: **Ctrl+Alt+P** switches it back on without restarting, so a
   player who has read the reason and fixed the config — or who simply disagrees — is one
   keypress away from resuming. The same chord switches it off at any time. It is a
   two-modifier chord precisely so it cannot collide with the game's own bindings, which use
   letters, digits, space and the arrows unmodified.
7. **In-run rewards only.** The `pending_rewards` queue. Town, meta-progression, challenge
   and customizable-level rewards are out of scope.

### Accepted costs

Two things the player gives up by switching the mod on, stated here so they are chosen
rather than discovered:

- **Deferral is lost** for any configured type. Holding a reward until a wave resolves, or
  banking one until it is needed, is a legitimate play and the mod removes it. The only way
  to keep deferral for a type is to leave that type out of the config.
- **Rerolls are spent, full stop.** If the mod burns its reroll budget and still ends up
  handing the reward back, those rerolls are gone. There is no speculative-reroll
  avoidance, no rollback, no refund accounting.

## 3. Decision algorithm

Each option id, for each reward type, carries two attributes:

- a **tier** — `wanted`, `fallback`, or `blacklist`
- a **weight** — a number ordering options *within* a tier

Weights are only meaningful within a tier. A `wanted` option always beats a `fallback` one
regardless of weight; comparing weights across tiers is meaningless. Equal weights within a
tier express genuine indifference and are broken at random.

`_scrap` is a reserved pseudo-option, always on offer **[TBD-3]**, representing scrapping the
reward for 5 denarii. It is tiered and weighted exactly like a real option. Placing it in
`wanted` means "take the denarii for this type over unranked options, never reroll"; placing 
it in `fallback` means "settle for the denarii rather than bother me".

When a reward reaches the head of the queue:

```
if the mod is disabled                          -> stop
if the reward's type has no config section      -> leave manual, halt the drain
if a reward UI is already open by player action -> do nothing this frame
otherwise                                       -> open reward picking menu

wait delay_ms

loop:
    options := the offered option ids, plus _scrap
    if any id is outside this type's known vocabulary
        -> log an error, leave manual, halt the drain

    W := { o in options : tier(o) = wanted }
    if W is non-empty
        -> pick argmax weight, ties broken by the mod's RNG; apply; done

    if another reroll is permitted (see §4)
        -> reroll; continue loop

    F := { o in options : tier(o) = fallback }
    if F is non-empty
        -> pick argmax weight, ties broken by the mod's RNG; apply; done

    -> leave manual, halt the drain
```

Blacklisted options are simply never candidates. They do not block a reward: if something
`wanted` is also on offer, it is taken.

**Halting the drain** means the mod stops processing the queue entirely, because the head is
now unresolvable and rule 2.1 forbids reaching past it. The drain resumes when the player
resolves that reward themselves and a new head appears.

## 4. Reroll economy

The mod does not choose what a reroll costs — the game does, spending free rerolls
automatically before it ever charges denarii. So the config is not "spend N free then M
paid". It is four independent caps on *what the mod is willing to trigger*, checked against
what the next reroll would actually cost:

| key | meaning |
|---|---|
| `voodoo_depth` | rerolls taken via the Voodoo Beads per-reward freebie **[TBD-4]** |
| `free_depth` | rerolls taken from the reign-wide free pool (`run_rerolls_left`) |
| `paid_depth` | rerolls paid for in denarii |
| `denarii_floor` | don't do a paid reroll if it would leave the balance below this (does not gate free rerolls) |

All four are per reward type; the three depths reset at the start of each reward.

Before each reroll the mod asks the game what the next one would cost **[TBD-5]**, finds the
matching budget, and permits the reroll only if that budget has room. Where the cost is
denarii, it must additionally be affordable and leave the balance at or above
`denarii_floor`.

**The mod does not fall through to a later budget.** If the next reroll would be free and
`free_depth` is exhausted, rerolling stops — it does not "upgrade" to spending denarii,
because the player could not have chosen to pay while a free reroll was banked. A direct
consequence: `free_depth: 1, paid_depth: 1` with two or more free rerolls banked will reroll
once and stop, and `paid_depth` stays unreachable until the free pool runs dry. That is a
real and intended consequence of the configuration, not a defect.

Voodoo rerolls are consumed first where the distinction exists, since they do not touch the
reign-wide pool.

## 5. Config file

### Location

`config.ini`, **in the mod's own folder** — the one holding `install.py` — alongside the log
and anything else the mod owns. See §7.1 for why nothing lives in the game folder and how
the DLL is told where to look.

### Format

Line-oriented INI. Comments with `#`. One section per reward type, plus three sub-sections
per type for the tiers, plus a `[global]` section.

```ini
[global]
enabled  = true
delay_ms = 100

[unit_class_stat]
voodoo_depth  = 1
free_depth    = 2
paid_depth    = 0
denarii_floor = 0

[unit_class_stat.wanted]
ranged.attack_speed = 10
ranged.damage       = 10      # indifferent between these two
warrior.hp           =  4

[unit_class_stat.fallback]
warrior.damage = 7
_scrap        = 1             # settle for the 5 denarii before bothering me

[unit_class_stat.blacklist]
grunt.hp                    # bare keys; weights are meaningless here
```

Option key syntax is **[TBD-1]** (section names) and **[TBD-2]** (`unit_class_stat` keys —
the dotted `class.stat` form above is a placeholder).

A reward type with no section is fully manual. `[global] enabled = false` disables the mod
entirely without uninstalling it.

### Each type lists only what it could actually offer

A reward type does **not** draw from its whole library. `artifact_legend` offers only
legendary artifacts; `improvement_production_t1` offers only tier-1 production buildings, not
terrain tiles or starter structures. The eligible pool is filtered — and for improvements it
is filtered *dynamically*, by the equipped king (`select_improvements_rewards_filter` reads
`KING_EQUIPPED`), by how far the run has progressed (`filter_improvements_by_weight` reads
`game_stage` and `tier`), and by an exclusion set (`FILTERED_IMPROVEMENTS_REWARDS`).

The rule for what a section lists, therefore:

- **If an id could be offered under some run — any king, any stage — include it.** Being
  cautious costs nothing but a line.
- **If it could never be offered for that type, leave it out.** Fewer lines make the file
  navigable, and listing an impossible option is noise that makes the real choices harder to
  find.

An early generated config ignored this and listed every library entry under every type. That
put ordinary artifacts under `artifact_legend` and terrain tiles under
`improvement_production_t1` — wrong, and unusable at 3,168 lines.

### Completeness is required

For every type that has a section, **every option id in that type's vocabulary must appear
in exactly one tier**. There is no default for an unlisted id — an unlisted id is a config
error, and an id listed in two tiers is a config error.

This is affordable because the mod generates the complete file (§5.4) and cheap to enforce,
and it means the player has consciously classified everything the mod might ever pick.

### Validation and reload

The config is re-read when its modification time changes; no restart, no recompile.

- **Load-time validation.** Each section is checked against the known vocabulary for its
  type. A section with a missing id, a duplicate id, an unknown id, or a malformed value is
  rejected; that type falls back to fully manual. Other sections are unaffected.
- **On a failed reload**, the last known-good config stays in force and the failure is logged
  loudly. A bad edit mid-run never silently changes behaviour.
- **Runtime backstop.** If an option id appears that is outside the vocabulary anyway — a
  game update added one and the mod's baked vocabulary is stale — the reward is left manual
  and the discrepancy is logged. §3 already covers this; it is restated because it is the
  one path that survives load-time validation.

### First-run generation

If no config exists, the mod writes a complete one containing **every** reward type, with
**every** option id in that type's `blacklist` tier, all three depths at `0` and
`denarii_floor` at `0`. Ids are annotated with their display names, resolved from
`local/localization.csv`, as trailing comments.

With nothing wanted, nothing in fallback and no reroll budget, this config resolves nothing:
the mod is inert until the player edits it. **Installing the mod without configuring it
changes nothing about the game.**

The generated file doubles as the reference for what is configurable, so the player edits
tiers and weights rather than typing ids.

## 6. Logging

`picker.log`, in the mod's folder next to the config.

Because the mod acts invisibly, the log is the only record of what it did, so it is not
optional. One line per resolved reward:

- timestamp
- reward type
- the ids offered
- each reroll taken, with its cost class and price
- the action taken (`pick <id>` / `scrap` / `manual`) and the tier and weight that justified it

Config load results, validation failures, version-guard failures and vocabulary mismatches
are logged to the same file.

Rotated at a size cap so a long session cannot fill a disk.

## 7. Delivery

**A proxy `version.dll` placed in the game folder.**

The game statically imports `version.dll`, which is not a KnownDLL, so a copy in the
application directory wins the loader search and is loaded before the game's entry point.
This is a pure addition: no game file is renamed, no byte of the executable is modified, and
there is no patched-exe to maintain. It coexists with the existing resume-morale patch
without interaction.

The DLL forwards every `version.dll` export to the real one in `System32` **[TBD-7]** and
otherwise gets out of the way.

Hooks are installed in memory at runtime, not written to disk.

### 7.1 Footprint: everything the mod owns lives in the mod's folder

**The game folder gets exactly one added file: `version.dll`.** Nothing else — no config, no
log, no backup, no marker. At a glance the game folder is untouched, uninstalling is deleting
one file, and no other mod's files can be shadowed or confused with this one's.

Everything else — `config.ini`, `picker.log`, and any backup the mod ever needs to take —
lives in the mod's own folder, the one holding `install.py`. That folder is user-owned and so
always writable, it survives a game reinstall, and it keeps the mod's state next to the
`uninstall.py` that knows how to clean it up.

On backups specifically: **this mod displaces nothing, so it has nothing to back up.** It
modifies no game file and appends nothing to the executable. If a future version ever does
displace a game file, the original goes to the mod's folder, never next to the game. The one
adjacent case — a `version.dll` already present because another mod claimed the same proxy
slot — is handled by refusing to install (§9), not by overwriting and backing up, because
silently displacing another mod's hook would break it in ways this mod can't predict.

### 7.2 Telling the DLL where its folder is

The DLL sits in the game folder but must read its config from the mod's folder, and it has no
way to derive that path at runtime. Rather than drop a pointer file in the game folder — which
would defeat §7.1 — **`install.py` stamps the absolute path into the DLL as it copies it.**

The DLL reserves a fixed-size buffer behind a unique marker:

```rust
#[used]
#[no_mangle]
static MOD_DIR: [u8; 520] = *b"TKIW_PICKER_MOD_DIR\0<padding to 520 bytes>";
```

`install.py` locates the marker in the built DLL by byte search, writes the mod folder's
absolute path as NUL-terminated UTF-8 into the bytes following it, and writes the result to
the game folder. It refuses if the path does not fit, or if the marker is missing or appears
more than once — a stamping failure must be loud, never silent.

Re-running `install.py` re-stamps, so **moving the mod folder requires re-running it**. If the
stamped path no longer resolves, the DLL disables itself (§8) — correct behaviour if the
folder was deleted, merely surprising if it was moved, so the failure is additionally reported
to `%TEMP%\tkiw_reward_picker_error.log`. That is the one thing the mod ever writes outside
its own folder, it exists only for the case where the mod's folder is unreachable, and it says
nothing but which path was missing and to re-run `install.py`.

### Reading the game

The executable is a GameMaker YYC build — all GML is compiled to native x86-64 and `data.win`
has no `CODE`/`VARI`/`FUNC` chunks, so there is no bytecode. Two recoverable tables make it
tractable:

- A 24-byte-stride table in `.data` pairing `gml_*` name strings with function pointers,
  yielding ~12,769 named GML functions.
- Each GML variable name string is followed 8 bytes later by its variable-id slot
  (`0xFFFFFFFF` until resolved at startup), so a rip-relative operand whose target minus 8
  points at a C string identifies the variable being touched.

The DLL resolves everything it needs through these tables **at runtime, by name**, rather
than through baked addresses. That makes it far more robust across game updates than the
morale patch's byte-signature approach — but it is not immunity, and §8 covers what happens
when resolution fails.

`.pdata` gives ~86k function boundaries for free, which makes the offline analysis pass
straightforward. Python + `capstone` is sufficient for that pass; no Ghidra or IDA needed.

### Language and build

Rust, `crate-type = ["cdylib"]`, `x86_64-pc-windows-msvc`, **standard library only — no
crates**. The FFI surface is small enough that dependency-free is no hardship, and it means
the mod builds offline with nothing but the toolchain already installed.

```
cargo build --release
```

## 8. Failure behaviour

The mod is loaded into someone's game. Everything below fails toward "the game behaves
exactly as it does unmodded".

- **Symbol or variable resolution fails at startup** — the mod logs what it could not find,
  disables itself, and forwards `version.dll` calls as normal. The game runs unmodded.
- **The stamped mod folder does not resolve** — the folder was moved or deleted. The mod
  disables itself and reports to `%TEMP%\tkiw_reward_picker_error.log` (§7.2), since it has
  nowhere else to write.
- **A required game function's signature is not what the mod expects** — same: disable, log,
  do not call it.
- **The config is missing** — generate the inert default (§5.4).
- **The config is invalid** — §5.3.
- **An option cannot be confidently identified** — leave the reward manual and log.
- **Any panic inside the mod** — caught at the hook boundary, logged, and the mod disables
  itself for the remainder of the session rather than propagating into the game.
- **The mod costing the player frames** — every poll is timed against a budget. A slow poll
  widens the polling interval; persistently awful ones disable the monitor for the session.
  The mod runs inside the player's frame, so a slow poll is a stutter they can feel, and an
  early build made the game unplayable this way. Backing off automatically makes that a
  structural property rather than something that depends on the implementation staying
  careful.

A kill switch is available without editing files: `[global] enabled = false` takes effect on
the next reload, and a hotkey **[TBD-8: keybind]** disables all automation for the session
immediately.

### A note on backlogs

Reward resolution freezes gameplay, so each auto-resolved reward injects a pause of roughly
`delay_ms`. For normal play that is an imperceptible hitch. A large backlog accumulated
*before* the mod was installed drains back to back — a 119-deep queue at the default 100 ms
would freeze the game for about twelve seconds on first load. This is documented rather than
special-cased; a player expecting it can set `delay_ms = 0` for the first launch.

## 9. Install and uninstall

Two Python scripts, following the resume-morale-fix precedent: Python 3.8+, runnable from
anywhere, locating the game themselves. "The mod's folder" throughout means the directory
holding these scripts, resolved from `__file__` rather than the working directory.

### `install.py [game folder or exe]`

1. Locate the game through the Steam library folders, or accept an explicit path.
2. Refuse if the game is running.
3. Refuse if a `version.dll` is already present and is not ours — another mod may be using
   the same proxy slot, and silently replacing it would break it.
4. Refuse, with build instructions, if the built DLL is not found.
5. Stamp the mod folder's absolute path into a copy of the DLL (§7.2), refusing loudly if the
   marker is missing, duplicated, or the path does not fit.
6. Write the stamped copy into the game folder as `version.dll`.
7. Print both paths — the one file added to the game folder, and the mod folder where the
   config will appear — and note that the config itself is generated, inert, on first launch.

### `uninstall.py [--purge] [game folder or exe]`

1. Locate the game the same way.
2. Verify the `version.dll` present is ours, by its stamp marker, before removing it — never
   delete a file we did not place.
3. Remove it. The game folder is now exactly as it was.
4. `--purge` additionally removes `config.ini` and `picker.log` from the mod's folder, so that
   afterwards the folder can simply be deleted. Without `--purge` the config survives, and
   reinstalling restores the player's preferences.

Running either twice is harmless. Neither ever touches the executable, so neither can be
undone by a Steam integrity check — though a check may remove `version.dll` itself, in which
case re-running `install.py` restores it.

Because the mod's whole footprint in the game folder is one file, and everything else sits in
the mod's own folder, this mod and the resume-morale-fix can be installed together without
interacting, and either can be removed without disturbing the other.

## 10. To be established before the format hardens

These are facts about the game, not open design questions. Each is answered by reading the
binary, and each will be answered rather than guessed at.

> **Spike complete — see [`analysis/FINDINGS.md`](analysis/FINDINGS.md) for the evidence.**
> TBD-1 through 7 are answered, TBD-6 included: option identity is cleanly readable, from a
> `<thing>_contained` member on the `obj_card_*` instance. The two-step worry is **resolved —
> no queued reward is two-step** (§10.1), so the config format stands. That work turned up
> three rules the spec had not accounted for (§10.2), all now folded in.
>
> **The id vocabularies are confirmed against the running game.** The mod walks the game's own
> libraries at runtime and `analysis/verify_live.py` diffs them against the generated docs:
> 35 reward types, 22 resources, 127 artifacts and 8 unit classes all agree exactly. Five
> further option vocabularies came from the live game — 49 spells, 176 improvements,
> 269 upgrades, 40 advisors, 305 units — and are in
> [`docs/option-vocabularies.md`](docs/option-vocabularies.md). Where live and static ever
> disagree, the live game wins.
>
> **The runtime layer is built and proven read-only**: injection, symbol discovery, safe
> memory access, a per-frame hook on the game's own thread, instance lookup by pure reads, and
> `ds_list`/`ds_map` traversal. Nothing has ever been written to the game.

- **[TBD-1] ANSWERED — 35 reward types**, from the `library_add_reward` calls in
  `reward_library`. Only **25 present a choice**; the other 10 are direct grants with nothing
  to pick and are out of scope. Section names come from the 25. Full table in FINDINGS.
- **[TBD-2] ANSWERED — `unit_class_stat` options are (class, stat) pairs**: 8 classes
  (Grunt, Rider, Flying, Ranged, Arcane, Warrior, Champion, Undead) × 3 stats (`atk_speed`,
  `damage`, `hp`). The `class.stat` key shape holds. **Caveat: class identity is positional**
  — the save keys training by class index and no class-id string exists — so the config is
  keyed by name against a baked index table, and the mod refuses to automate this type if the
  game no longer has exactly 8 classes rather than risk applying preferences to the wrong one.
- **[TBD-3] ANSWERED — `_scrap` is NOT universally available.** `scrap_button_setup` has
  exactly one caller, `spawn_choice_unified`. The `unit_class_stat` path never calls it, so
  **Troops Training rewards cannot be scrapped**. Ranking `_scrap` in a type that cannot
  scrap is a config error, caught at load. This removes the intended cheap resolution for the
  highest-volume reward type.
- **[TBD-4] ANSWERED — Voodoo Beads IS separately expressed**, so `voodoo_depth` survives and
  no discussion is needed. The game keeps two independent counters: per-**reward** free
  rerolls (`FREE_REROLLS_PER_REWARD_LIMIT` → `free_rerolls_per_reward_left`, fed by an
  acquire/remove artifact callback pair) and the per-**run** pool
  (`FREE_REROLLS_PER_RUN_LEFT`, saved as `run_rerolls_left`). The key is better named for the
  mechanism — per-reward free rerolls — than for the one artifact that currently feeds it.
- **[TBD-5] ANSWERED — the cost is queryable.** `resolve_reroll_cost` computes the next
  reroll's price up front, before the button is drawn. §4's four-budget model stands as
  written.
- **[TBD-6] Reduced to runtime confirmation.** Option cards are `obj_reward_option` instances
  carrying a **`reward` member**, and for every `spawn_choice_unified` type the options are
  plain string keys from a `get_*_keys` method. Identity is a member on the instance, not
  something recoverable only from a closure — the failure mode this item existed to catch.
  What remains is reading that member off a live instance from the DLL. **Any reward type
  where identity turns out not to be cleanly readable is still a type this mod will refuse to
  automate**, stated rather than worked around.
- **[TBD-7] ANSWERED — 17 exports** to forward; list in FINDINGS.
- **[TBD-8] The session kill-switch keybind**, chosen so it cannot collide with a game
  binding. Still open; trivial, and best chosen once the mod runs.

### 10.1 RESOLVED: no queued reward is two-step

The concern was real but misplaced, and the config format does not change.

The per-type dispatcher that produces "opens a further choice" actions belongs to
`obj_rewards_bundle` — the post-wave claim panel — **not to the reward queue**. Its installer
`setup_reward_option` has exactly two referencing functions, and neither is on the queue path.
In a bundle the cards are not alternatives at all: they are a claim list of everything the
wave dropped, so "pick a category then pick within it" is really "claim this reward, then make
its one choice".

A queue entry instead produces `obj_card_*` instances, and **every one of the seven card
types' press actions is terminal** — `add_resource`, `equip_spell`, `equip_artifact`,
`equip_improvement`, `equip_upgrade`, `unit_class_mod_change`, `spawn_player_unit`, each
followed by `destroy_card_choice`. None calls a `spawn_*_choice`. So one queue entry is one
decision, and §3's algorithm stands as written.

This also corrects [TBD-6]: the mod was looking at the wrong object. Identity lives on the
`obj_card_*` instance in a consistently named member — `resource_contained`,
`artifact_contained`, `spell_contained`, `improvement_contained`, `upgrade_contained`,
`start_bonus_packs_contained`, `class_stat_bonuses_contained`. Cleanly readable, which is the
answer that decides the mod is viable at all.

### 10.2 Three rules this uncovered

**Four types are not a card pick and must be refused.** `shop` and `shop_graveyard` open a
shop to browse, `prophecy` a drag-and-drop board, `rewards_wheel` a spin-then-apply sequence.
None is a set of alternatives, and none may be automated — they are marked in
[`docs/reward-types.md`](docs/reward-types.md) and are config errors if given a section.

**The queue can grow while being drained.** Wheel-apply and shop purchases call
`add_pending_reward`, so resolving one entry can append others. The drain loop must **re-read
the queue after every resolution** rather than snapshot it — which the strict-FIFO rule
already implies, but it is worth stating because snapshotting would look correct and silently
skip entries.

**The offered cards may not be the type the queue entry named.** When a choice is spawned with
an empty key list — every candidate already owned or banished — the game either offers
*resource* compensation instead (so an `artifact` entry can present resource cards) or
auto-scraps the reward with no decision at all. So the mod must **decide from the card objects
actually on screen, not from the queue entry's `reward_type`**, and treat a mismatch between
them as a reason to leave the reward alone. Deciding from the entry's type would apply the
wrong config section to a real choice — the single worst failure available to this project.

## Appendix: orientation notes

Verified against the build installed as of 2026-08-10. Re-verify before relying on anything
address-shaped; a game update moves everything.

**Saves are plain JSON** with one trailing NUL byte — parse with
`json.JSONDecoder().raw_decode`. Under `%LOCALAPPDATA%\The_king_is_watching_steam\Release\`;
`run_data.dat` is the in-progress run. Reading these is the cheapest way to understand the
data model before touching disassembly.

**Options are rolled at open time, not stored.** `pending_rewards` holds only
`{reward_type, params}` — e.g. `{"reward_type": "unit_class_stat", "options_amount": 3}`.
There is no option list to inspect offline, which rules out any save-editing approach and
forces the in-process design.

**Terminology.** Two distinct mechanisms that are easy to conflate:

- `obj_button_banish_reward` acts on a single *option* — "Banished options will not be
  offered again during this run". Out of scope (§2.4).
- `obj_button_rewards_scrap` / `obj_button_rewards_skip` act on the whole *reward*, and
  scrapping pays 5 denarii. This is `_scrap`.

Searching for "recycle" finds almost nothing.

**Starting points.** Reward and choice machinery, all
`@gml_Object_obj_run_controller_Create_0`: `spawn_rewards_choice`, `spawn_choice_unified`,
`spawn_bonus_choice`, `spawn_custom_resources_choice`, `spawn_stat_upgrade_choice`. Option
cards are `obj_reward_option`, spawned by `obj_spawner_reward_option`; the bulk of the
per-type logic appears to live in `anon@928@gml_Object_obj_reward_option_Create_0` and its
many nested closures. Queue UI: `obj_button_reward_queue`,
`obj_button_reward_queue_parent`. Rerolls: `obj_button_reroll_cards`, `setup_reroll_button`.
Generation: `reward_library`, `generate_reward_by_difficulty`, `wave_reward_pools_construct`.
Variables: `pending_rewards`, `run_rerolls_left`, `FREE_REROLLS_PER_RUN_LEFT`;
`get_pending_rewards_save_data` / `apply_pending_rewards_save_data` on the gameplay
controller.

Voodoo Beads is artifact id `voodoo_beads`; artifact behaviour lives in `artifact_library`.

**Resources.** The `RESOURCE_*` constants are `WATER, WOOD, CLAY, ORE, WHEAT, GRAPES, WINE,
FLOUR, METAL, CRYSTAL, GOLD, COIN, MEAT, FURNITURE, FUEL, BANANAS, BLOOD, ORGANS,
DARK_ENERGY, NETHER_RUNE, RELIC_GRAVEYARD` (plus `RESOURCE_RANDOM` and
`RESOURCE_PACK_AMOUNT_BIG`/`_SMALL`, which are not resources). Several are biome- or
level-specific. The ids do not match player-facing names one-to-one — "grain" is `WHEAT`,
"oil" is probably `FUEL` — so the id↔display mapping must be confirmed against
`local/localization.csv`, which is also where the generated config's annotation comments
come from.

**Toolchain, verified present.** Rust 1.96 (`x86_64-pc-windows-msvc`) with VS BuildTools; a
`cdylib` test build completed offline in under three seconds. Python 3.13 with `capstone`
5.0.7 for the offline analysis pass.

**Precedent.** `tkiw-morale-fix/` in the parent folder — a shipped resume-morale fix using
appended-PE-section detours. Worth reading for the game-location logic, the refuse-politely
posture, and the install/uninstall shape, all of which this project reuses. Its *binary*
approach is deliberately not reused.
