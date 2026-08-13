# Spike findings

Answers to the spec's §10 items, recovered from the shipped executable.

Verified against the build installed 2026-08-10, analysed from the pristine copy
(`tkiw-morale-fix/The King is Watching.exe.orig`) so the results describe the shipped game
rather than a patched one. **Every address and index here moves when the game updates.**

Reproduce with the scripts in this folder:

```bash
python index.py        # build the cross-reference cache (~15s)
python strconsts.py    # recover the string-constant pool (~6s)
python methods.py gml_Object_obj_run_controller_Create_0
python gmldis.py <symbol>
python xrefs.py --str "some string"
```

## Toolkit

| table | count | how |
|---|---|---|
| named GML functions | 13,132 | 24-byte-stride records in `.data` |
| GML variable slots | 13,476 | name string + 8 → id slot (`0xFFFFFFFF` on disk) |
| function boundaries | 86,347 | `.pdata` |
| GML string constants | 11,452 | callers of the string ctor at `0x1aa4090` |
| functions indexed | 12,769 | vars / strings / calls, forward and inverse |

Two techniques were needed beyond the notes handed over:

- **Whole-`.text` xrefs.** The GML symbol table covers only 12.8k of 86k functions, so
  references from unnamed runtime code are invisible to a symbol-table-only index. `xrefs.py`
  sweeps the raw bytes arithmetically — a `disp32` at file offset `o` targets `T` iff
  `disp32(o) + o == T - V - 4 - tail + R` — then verifies the implicated functions with
  capstone. Whole-binary xref in about 5 seconds.
- **Method-name recovery.** Most methods are anonymous (`spawn_resources_choice = function()`),
  so YYC names them `anon@NNNN@parent` and drops the member name. The binding is recoverable
  from the parent: the member's variable-id slot is loaded, then the method's address is
  `lea`'d within a few instructions. This bound **81 of the run controller's methods**,
  including every `spawn_*_choice`, and is what made the rest tractable.

---

## [TBD-1] Reward-type vocabulary — ANSWERED

35 types, from the 35 `library_add_reward` calls in `reward_library`. Internal id first,
display name second.

| id | display | id | display |
|---|---|---|---|
| `artifact` | Artifact | `improvement_infernals` | Infernal Barracks |
| `artifact_legend` | Legendary Artifact | `improvement_misc` | Kingdom Infrastructure |
| `basic_construction` | Basic construction | `improvement_attacking` | Offensive Structures |
| `nether_runes_mine` | Nether Runes mine | `improvement_any_generic` | — |
| `castle_heal` | Castle heal | `improvement_any` | — |
| `coins` | Denarii | `prophecy` | Prophecy |
| `resource_relics` | Relics | `resource` | Resource |
| `resource_nether_rune` | Nether Runes | `shop` | Trader |
| `resource_wood` | Wood | `shop_graveyard` | Cemetery Trader |
| `resource_wine` | Wine | `spell` | Spell |
| `infernal_boss` | Infernal Overlord | `spell_legend` | Legendary Spell |
| `encounter` | Encounter | `upgrade` | Building Upgrade |
| `improvement_production_t1/t2/t3` | Basic/Established/Advanced Production | `unit_class_stat` | Troops Training |
| `improvement_troops_t1/t2/t3` | Levy/Veteran/Elite Barracks | `run_start_bonus` | Start Bonus |
| `{0}_t1/t2/t3` | Infernal Barracks T1/T2/T3 | `onaraks_favour` | Developers' Appreciation |
| | | `rewards_wheel` | Rewards' Wheel |

`{0}_t1/_t2/_t3` are format strings built from `improvement_infernals`.

### Only 25 of the 35 present a choice

`spawn_rewards_choice` dispatches on the queue entry's type. Types it does not handle are
direct grants with nothing to pick, and are therefore outside this mod's scope entirely:

| choice path | reward types |
|---|---|
| `spawn_improvements_choice` | the 12 `improvement_*` types |
| `spawn_artifacts_choice` | `artifact`, `artifact_legend` |
| `spawn_spells_choice` | `spell`, `spell_legend` |
| `spawn_upgrades_choice` | `upgrade` |
| `spawn_shop` | `shop`, `shop_graveyard` |
| `spawn_prophecy_choice` | `prophecy` |
| `spawn_resources_choice` | `resource` |
| `spawn_starting_bonus_choice` | `run_start_bonus` |
| `spawn_unit_class_stat_bonus_choice` | `unit_class_stat` |
| `spawn_wheel` | `rewards_wheel` |
| **direct grant, no choice** | `castle_heal`, `coins`, `basic_construction`, `nether_runes_mine`, `infernal_boss`, `encounter`, `onaraks_favour`, `resource_wood`, `resource_wine`, `resource_relics`, `resource_nether_rune` |

Most choice paths funnel into **`spawn_choice_unified`**, called with a key list from a
`get_*_keys` method and a `*_per_choice` count. `unit_class_stat` instead goes through
`spawn_stat_upgrade_choice`. Both end at `assign_rewards_to_cards`.

## [TBD-2] `unit_class_stat` option identity — ANSWERED, with a caveat

An option is a **(unit class, stat) pair**: `assign_rewards_to_cards` works in terms of
`class`, `stat_index`, `stats`, `possible_unit_classes` and `generate_class_stat_reward`.

**3 stats**, from `UNIT_CLASS_MODS_ATTACK_SPD` / `_DAMAGE` / `_HP`, confirmed by the save's
own `training_atk_speed`, `training_damage`, `training_hp`.

**8 classes**, from `localization.csv`:

| index | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
|---|---|---|---|---|---|---|---|---|
| class | Grunt | Rider | Flying | Ranged | Arcane | Warrior | Champion | Undead |

So 24 possible options, and the `class.stat` config key shape in the spec holds —
`ranged.damage`, `grunt.hp`, and so on.

**The caveat: class identity is positional, not nominal.** The save stores training as
`{"0": …, "7": …}` keyed by class index, and there is no class-id string anywhere in the
data — `unit_class_title_N` in `localization.csv` is the only name, and it is display text.
A game update that inserts or reorders a class silently changes what `ranged.damage` means.
The config must therefore be keyed by name and resolved through a baked index table, with a
count check at load: **if the game no longer has exactly 8 classes, refuse to automate this
type** rather than risk applying preferences to the wrong class.

## [TBD-3] `_scrap` is NOT universally available — ANSWERED

`scrap_button_setup` is referenced by exactly one caller: `spawn_choice_unified`.

`spawn_stat_upgrade_choice` — the `unit_class_stat` path — never calls it, and neither do
`spawn_bonus_choice` or `spawn_custom_resources_choice`. **Troops Training rewards cannot be
scrapped.**

This bites: `unit_class_stat` was 115 of the 119 rewards in the sample queue, and ranking
`_scrap` was the intended way to make that type resolve itself cheaply. Ranking `_scrap` in a
type that cannot scrap must be a config error, caught at load.

## [TBD-4] Voodoo Beads IS separately expressed — ANSWERED

The distinction the four-budget model needs exists in the game's own state, as two
independent counters:

| concept | game state |
|---|---|
| per-**reward** free rerolls | `FREE_REROLLS_PER_REWARD_LIMIT` → `free_rerolls_per_reward_left` |
| per-**run** free reroll pool | `FREE_REROLLS_PER_RUN_LIMIT` → `FREE_REROLLS_PER_RUN_LEFT`, saved as `run_rerolls_left` |
| paid rerolls so far | `non_free_rerolls_made`, escalating by `cost_increase_per_reroll` |

Two 124-byte artifact callbacks (`anon@31774` / `anon@31875` in `artifact_library`) touch
`FREE_REROLLS_PER_REWARD_LIMIT` and nothing else — an acquire/remove pair incrementing and
decrementing the per-reward allowance. They are the only artifact-library code that touches
it, which together with Voodoo Beads being the game's one "free reroll per reward" artifact
identifies the mechanism. The residual — that these two callbacks belong to the `voodoo_beads`
entry specifically rather than some other artifact — is cheap to confirm at runtime and does
not change the design either way.

**`voodoo_depth` is expressible.** It is more accurately "per-reward free rerolls" than
"Voodoo rerolls", since the counter is a general mechanism Voodoo Beads happens to feed;
naming it for the mechanism would survive a second artifact being added.

## [TBD-5] Reroll cost is queryable — ANSWERED

`resolve_reroll_cost` is a method on the reroll button, called by `setup_reroll_button`
before the button is drawn. The cost of the next reroll is computed up front, so the mod can
ask what a reroll would cost and match it to a budget without committing. The four-budget
model in §4 of the spec stands as written.

## Runtime layer — confirmed in the live game

Verified in the running process, not inferred:

- The proxy `version.dll` loads before the game's entry point and forwards correctly.
- Symbol discovery inside the live process recovers **13,132 functions and 13,476 variable
  slots** — identical to the offline analysis, from an independent implementation.
- Variable ids resolve at runtime once the game has started: all nine of the mod's required
  variables read back real ids from their slots.

Two bugs found the hard way, both now regression-tested:

- **Address arithmetic.** A live address is `base + rva`, not `rva + slide` where
  `slide = base - image_base`. The latter is short by `image_base` and lands in unmapped
  space. It cost two game launches, and the original test suite missed it because it only
  compared lookups against each other, so a constant offset cancelled out. Absolute bounds
  checks now cover it.
- **Constant-folded stamp.** Reading the installer's mod-folder stamp through an ordinary
  reference lets the compiler fold it to the zeros it was built with, so the patched path is
  never seen. It must be read volatilely.

Every address derived from the on-disk image is now checked with `VirtualQuery` before it is
dereferenced, and the startup probe writes a breadcrumb so a run that dies partway disarms
itself — the mod cannot break a player's game twice for the same reason.

## Live data layout, observed in the running game

Recovered by dumping memory rather than guessing, and cross-checked two ways.

**Global variable access.** Compiled GML reaches a global through a container object with a
small virtual interface, confirmed across the reward code before being relied on: 255 read
sites, 196 write sites, one dominant container pointer used by 66 of them.

```text
container = *(base + 0x2af7a08)     ; the mod's ONLY baked address
vtable    = *container
get       = *(vtable + 8)           ; (container, var_id) -> RValue*   [+0x10 is get-for-write]
```

**RValue** is 16 bytes: 8-byte payload, `u32` flags, `u32` kind. Kind 0 is real, 1 string,
2 array, 5 undefined, 6 object, 7/10 int, 13 bool.

**Strings are two levels deep.** A string RValue's payload is a refcounted descriptor, not
the characters:

```text
offset 0   char*  data       ; for a compiled-in literal, points into .rdata
offset 8   u32    refcount
offset 12  u32    size       ; high bit is a flag; mask with 0x7fffffff
```

Verified on `REWARD_UNIT_CLASS_STAT`: size read back as 15, matching `len("unit_class_stat")`,
and `data` landed on the same `.rdata` literal the offline analysis had already located --
subtracting its known RVA from the live pointer gave a module base that also put the
`get-variable` pointer inside `.text`. Two independent numbers agreeing.

The first attempt at this assumed the payload *was* the characters and probed a few offsets.
It failed cleanly rather than returning plausible garbage, because the read test compares
against values already known from static analysis; a decoder validated only against itself
would have produced nonsense that looked like data.

## The vocabularies are confirmed against the running game

The mod walks the game's own libraries at runtime and logs their keys;
`analysis/verify_live.py` diffs them against the statically derived docs. Everything agrees:
**35 reward types, 22 resources, 127 artifacts, 8 unit classes**, all exact. That also
confirms two inferences made statically -- that `{0}_t1` resolves to
`improvement_infernals_t1`, and that the resource ids are the constant suffix lowercased.

It additionally yielded five vocabularies static analysis had not produced: 49 spells,
176 improvements, 269 upgrades, 40 advisors, 305 units. These are the *option* id lists for
their reward types, i.e. exactly what a config section is written against. The only ids
without display names are three developer debug spells, which are never offered.

## A performance failure worth recording

The first wide survey build made the game unplayable -- heavy lag, and a freeze on continuing
a reign. The cause was entirely the mod's:

- `win::readable()` called `VirtualQuery` -- a kernel transition -- before **every single
  read**, including each byte of a string comparison.
- `instance::count()` walked the **whole ~1,750-entry object registry** to find one object by
  name, and the survey called it 13 times.
- That ran twice a second, so a single poll cost on the order of 70,000 `VirtualQuery` calls.

Fixed by caching both layers:

- a small **validated-region cache**, flushed at the start of every poll. This is sound only
  because the mod runs on the game's own thread: the game cannot unmap or reprotect anything
  while a poll is executing, and the cache never outlives one.
- an **object-registry cache** of `name -> CObjectGML*`, including negative results, since
  object records are created at load and never move. The name is re-checked on every hit, so
  a stale entry falls back to a fresh walk rather than being trusted.

The general lesson: validating every pointer is right, but validating *per read* is not. The
unit of validation should be the region, and the unit of caching should be the poll.

## What is reachable as a global, measured in the running game

| global | reads back as | meaning |
|---|---|---|
| `REWARD_UNIT_CLASS_STAT` etc. | `"unit_class_stat"` … | plain strings, all four verified |
| `UNIT_CLASSES_LENGTH` | `Real(8.0)` | **8 troop classes, confirmed at runtime** |
| `FREE_REROLLS_PER_RUN_LEFT` | `Real(0.0)` | numbers decode |
| `REWARDS`, `RESOURCES`, `ARTIFACTS`, `SPELLS`, `IMPROVEMENTS`, `UNITS`, `UNIT_CLASSES` | `kind 15`, raw `0x02000002_000002xx` | struct references, not pointers |
| `pending_rewards`, `run_rerolls_left`, `free_rerolls_per_reward_left` | `kind 0xFFFFFF`, raw 0 | **unset — not globals at all** |

Three consequences:

- **`UNIT_CLASSES_LENGTH` reading back as exactly 8** independently confirms the class count
  behind [TBD-2], and is precisely the guard the spec requires before automating
  `unit_class_stat`: if a game update changes it, the mod can detect that at runtime and
  refuse the type rather than apply preferences to the wrong class.
- **The libraries are `ds_map` references** — kind 15 is `VALUE_REF`, and the payload is
  `{high dword = ref type, low dword = id}`. `0x02000002` is the runtime's own constant for
  `ds_map` (ref-type name table at `.data` `0x2974010`, 32 entries of
  `{const char* name, i32 type, i32 pad}`). So `REWARDS` is a
  `ds_map<string reward_id, Reward struct>` with id 700, and the others likewise
  (`RESOURCES` 694, `IMPROVEMENTS` 696, `UNITS` 701, `UNIT_CLASSES` 703, `SPELLS` 710,
  `ARTIFACTS` 713).

  Corroborated from compiled GML rather than from the tag alone: `reward_sprite_tag` calls
  the builtin `ds_map_find_value` on `REWARDS`, and `library_add_reward` calls `ds_map_set`.
  **Ids are run-scoped — resolve them from the global every time, never cache 700.**
- **`pending_rewards` is an instance variable, not a global** — as expected, it lives on the
  gameplay controller. Reading the reward queue therefore needs an *instance pointer*, which
  is the next thing to obtain.

### Getting an instance pointer — a read-only route exists

**No code patching is needed.** The runtime keeps a name-keyed object registry that can be
walked with plain reads, and both lookup functions were verified instruction by instruction.

```text
registry = *(base + 0x2b011d8)      ; { void** buckets @0, i32 mask @8, i32 count @0xc }
slot     = buckets[(objindex & mask) * 2]        ; 16-byte {head,tail} slots
node     = { next @0x08, i32 key @0x10, void* value @0x18 }   ; key = objindex
```

`Object_Find` (`0x1aa0df0`, a leaf function, which is why it has no `.pdata` entry) is
exactly this walk; `Object_FindIndexByName` (`0x1b31600`) walks the same buckets comparing
`*(char**)CObjectGML` — so the mod can do it itself with **no calls at all**.

| `CObjectGML` | | `CInstance` (this is `self`) | |
|---|---|---|---|
| `+0x00` | `const char* name` | `+0x00` | vtable; `[vt+8]` get, `[vt+0x10]` get-for-write |
| `+0x08` | parent | `+0x90` | owning `CObjectGML*` (back-pointer, validate with it) |
| `+0x68` | instance list head; node `{next @0, CInstance* @0x10}` | `+0xB8` | flags: `0x4` alive, `0x100003` dead/deactivated |
| `+0x94` | `i32` object index | `+0xBC` | `i32` instance id (>= 100000) |

`obj_gameplay_controller` is object index 550 and `obj_run_controller` 1079 in this build
(from `data.win`'s `OBJT` chunk), but **neither should be baked** — resolve by name. Neither
has a parent, and the gameplay controller is placed once per gameplay room, so its instance
list holds exactly one live instance during a run and none in menus. A `None` result outside
gameplay is correct, not a failure.

### Two corrections that came out of this

- **Kind 15 is a `ds_map` ref, not a struct ref** (above). The earlier note here was wrong.
- **Compiled GML does not use the builtin calling convention.** Runtime builtins are
  `f(RValue* result, void* self, void* other, int argc, RValue* args)` with `args` a flat
  16-byte-stride array. Compiled GML functions are
  `f(CInstance* self, CInstance* other, RValue* result, int argc, RValue* args)` — different
  register order entirely. Getting this backwards would corrupt memory on the first call, so
  it matters the moment the mod calls a GML function rather than a builtin.

### A third symbol table

The runtime registers its own API at startup through `Function_Add(name, fn, argc, flags)`
at `0x1b6bcf0`, called 2,769 times. Walking those call sites recovers **2,767 builtins** by
name — `ds_map_find_value` `0x1b08f70`, `variable_struct_get` `0x1b00380`, `array_length`
`0x1ad71f0`, `instance_find` `0x1b147b0`, and so on. `analysis/builtins.py` builds it.

Caution: `ds_map_find_value` calls `YYError` — a fatal dialog, not an error return — on a bad
ref or out-of-range id, so the ref type and id bounds must be validated before calling it.

## Next: reading values requires running on the game's thread

Reading a variable *id* is just a memory read and is safe from any thread. Reading a
variable's **value** is not: the compiled code does it through the GML runtime, roughly

```
obj  = *(void**)(base + 0x2af7a08)   // global variable manager
vt   = *(void**)obj                  // its vtable
fn   = *(void**)(vt + 8)             // get-variable
rval = fn(obj, var_id)               // -> RValue*
```

GameMaker is single-threaded, so calling that from the mod's background thread would race
the game. All actual work has to happen **on the game's own thread**, which means hooking a
function that runs every frame and doing the work from inside it. That is the first time the
mod writes to the game's code memory, and it is the next milestone.

## [TBD-6] CLOSED — option identity is fully readable

Confirmed in the running game, for the type that matters most.

**Cards are `obj_card_*`, not `obj_reward_option`.** The latter belongs to the post-wave
rewards bundle and never appears on the queue path. Each card type carries its identity in a
`<thing>_contained` member, and that member holds the **library element struct**, not the id
— the id is one hop further at `.system_name`, which is the `ds_map` key the game's own
`equip_*` functions look back up.

**Arrays**: length at `payload + 0x24` (from the `array_length` builtin), elements a
contiguous run of 16-byte RValues at `payload + 0x08`.

A live Troops Training choice read back as:

```
card[0] len=1 [unit_class=Int(1) stat_type=Int(1) stat_amount=Real(0.3)]   Rider   +30% Damage
card[1] len=1 [unit_class=Int(5) stat_type=Int(1) stat_amount=Real(0.3)]   Warrior +30% Damage
card[2] len=1 [unit_class=Int(2) stat_type=Int(1) stat_amount=Real(0.3)]   Flying  +30% Damage
```

Three things confirmed at once: the fields really are **int64** (`Int`, not `Real` — a reader
comparing against kind 0 would silently never match), every class seen is from the offered
set with **no Undead**, and `stat_type` is only ever 0 or 1.

**A choice mixes stats freely.** Verified against a screenshot: `[Flying] +30% HP`,
`[Rider] +30% Damage`, `[Ranged] +30% Damage` read back as
`(stat_type 0, class 2)`, `(1, 1)`, `(1, 3)` -- exact, in order.

An earlier note here claimed the opposite, that a choice fixed one stat and varied only the
class. That was generalised from two observed choices which both happened to be homogeneous
-- which occurs naturally about a quarter of the time, so it was a coincidence, not evidence.
The decode itself was correct throughout; only the conclusion drawn from it was wrong.

**Cards are placeholders for a few frames after they appear.** `select_button` reads `-4`
(`noone`) and `total_units_affected` reads `0` until the game finishes building them. The
identity fields are populated from the start, but anything keyed on card *existence* fires
too early to see the rest, so readiness is gated on `select_button` becoming a real instance
reference.

**Do not use the goblin sprite as a cross-check.** It does not match the card's stated class
-- a long-standing display bug the developers never fixed. The label and the units-affected
preview are correct; the sprite is not. A cross-check built on it would have contradicted a
correct decode.

## [TBD-6] original note — strongly favourable, not yet closed

Option cards are `obj_reward_option` instances carrying a **`reward` member**, set by
`assign_rewards_to_cards` via `assign_reward` / `card_setup`. For every type routed through
`spawn_choice_unified` the options are plain **string keys** from a `get_*_keys` method
(`get_resources_keys`, `get_artifact_keys`, …). Identity is a member on the instance, not
something recoverable only from a callback closure — which was the failure mode the spec was
worried about.

What remains is runtime work that static analysis cannot settle: reading `reward` off a live
instance from inside the DLL. The static picture says this should be straightforward for all
`spawn_choice_unified` types and for `unit_class_stat` (as a class/stat pair). **This is now a
runtime confirmation task, not an open design risk.**

## [TBD-7] `version.dll` exports — ANSWERED

17 exports to forward:

```
GetFileVersionInfoA        GetFileVersionInfoByHandle   GetFileVersionInfoExA
GetFileVersionInfoExW      GetFileVersionInfoSizeA      GetFileVersionInfoSizeExA
GetFileVersionInfoSizeExW  GetFileVersionInfoSizeW      GetFileVersionInfoW
VerFindFileA               VerFindFileW                 VerInstallFileA
VerInstallFileW            VerLanguageNameA             VerLanguageNameW
VerQueryValueA             VerQueryValueW
```

---

## New finding: some rewards are a two-step choice

Not anticipated by the spec, and it affects the resolution model.

`anon@928@obj_reward_option_Create_0` builds each card's pressed-action by reward type, and
those actions fall into two kinds:

- **terminal** — `add_resource`, `equip_spell`, `equip_improvement`, unit spawn. Picking
  resolves the reward.
- **opens a further choice** — `spawn_improvements_choice`, `spawn_artifacts_choice`,
  `spawn_spells_choice`, `spawn_resources_choice`, `spawn_unit_class_stat_bonus_choice`,
  `spawn_shop`, `spawn_prophecy_choice`. Picking presents *another* set of options.

So an `obj_reward_option` is not always a leaf: for some reward types the player first picks a
category and then picks within it. A picker that assumes one decision per queue entry would
select a category and then leave a second choice sitting open — visibly wrong, and exactly the
class of half-finished state the spec's fail-closed rule exists to prevent.

The mod must either resolve both levels (needing a config for each) or refuse two-step types.
**Which reward types are two-step in practice still needs establishing** — the dispatch above
shows which *actions* exist, not which are reachable from a given queue entry. This is the
first thing to pin down at runtime, alongside [TBD-6].


---

## The crash after long runs, caught with a phase at last (2026-08-12)

The readme's one known gap -- "rare and always follows a pick closely" -- has been
caught with the diagnostic that previously kept being lost.

```
---- FAULT ----
code 0xc0000005 at 0x7ff6b1fcd029
game base 0x7ff6b0540000
rva 0x1a8d029
while reading 0x36a755403c
last phase: pressing select arcane.hp @0x17f9acb8460
```

A 21-minute session draining a long `unit_class_stat` queue; the fault landed on the
press itself.

**The faulting instruction identifies the shape of the bug.** `0x1a8d029` is inside a
short helper at `0x1a8d000..0x1a8d0b9` that copies one RValue over another:

```asm
01a8d01d  mov  rcx, rbx
01a8d020  call 0x8f580                    ; release the destination first
01a8d025  mov  r10d, dword ptr [rbx+0xc]
01a8d029  mov  r8d,  dword ptr [rsi+0xc]  ; <== FAULT: read the SOURCE's kind
01a8d032  mov  dword ptr [rbx+0xc], r8d   ; then copy kind, flags, payload
```

So `rbx` is the destination and **`rsi` is a source RValue that has been freed** --
`0x36a7554030`, unreadable. This is not a null; it is a plausible-looking pointer to
memory that is gone. A use-after-free of the *source* of a copy.

That is consistent with the standing hypothesis and sharpens it: the press tears down
the choice, and something the game then copies still refers to a card's storage. What
it does **not** yet say is whether the stale RValue is one the mod handed the game or
one the game already held. Worth establishing before attempting a fix, because those
have opposite remedies -- hold a reference longer, versus press at a different moment.

Both mods' crash reporters recorded the same fault, one with the phase and one without
(the kit was standing down that session, so it had no phase to report). Two independent
vectored handlers agreeing on the address is a useful confirmation that the reporter
itself is sound.
