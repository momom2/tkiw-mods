# Groundwork for the three gameplay features

Analysis for the features that are **not** implemented. Everything here was recovered
from the executable; nothing has been tested against a running game. It exists so the
next session starts from the facts rather than the search.

Read [the blocker](#the-blocker-the-kit-cannot-draw) first — it decides the order the
rest should be attempted in.

---

## The blocker: the kit cannot draw

All three features add something to the screen. The kit currently **cannot**, and this
is structural rather than an oversight.

The only foothold on the game's thread is a hook on `PeekMessageW`, which runs inside
the message pump. GameMaker draws from Draw events, and by the time the pump is
reached the frame is finished. Calling `draw_text` there would either do nothing or
draw into a surface that is never presented.

Three ways out, cheapest first:

**1. Write state the game already draws.** If a number on screen comes from an
instance variable, writing that variable makes the game draw our value with its own
fonts, layout and styling, in the right place, at no risk of a torn frame. This is by
far the best option where it applies — and for *modified production values* it very
probably does.

**2. Extend a string the game already builds.** If a panel's text is assembled into a
variable before being drawn, appending to it adds lines using the game's own layout.
Likely available for *unit stats on hover*, and needs the panel's text variable found.

**3. A code detour into a Draw event.** The general solution and the expensive one:
allocate executable memory, write a trampoline that preserves registers, patch a call
site, and call back into Rust. `tkiw-morale-fix` did this with an appended PE section,
so there is precedent in this repository. It is the only route for drawing something
that does not already exist — the *greyed-out blueprint* almost certainly needs it.

`tkiw_runtime::patch` already does verified, revertible byte patching, which is the
first half of (3). What is missing is the trampoline and a safe place to put it.

**Do not attempt (3) without being able to see the screen.** A wrong detour is a crash
or a corrupted frame in someone's game, and neither shows up in a log.

---

## 1. Modified production values

**Most likely to be reachable, and by option (1) above.**

`obj_improvement_parent` carries the production machinery:

| variable | what it appears to be |
|---|---|
| `main_product` | the resource the building produces |
| `has_production_cycle` | whether it produces at all — set per building type in each `obj_improvement_*_Create_0` |
| `production_multi` | the building's own multiplier |
| `cell_production_multi` | the multiplier from the tile it stands on, read in `obj_improvement_parent_Step_0` and set in `obj_town_cell_Create_0` |
| `on_production_complete`, `_base`, `_specific` | the completion callbacks |
| `trigger_production_completed_on_action_completed` | ties production to an action finishing |

Modifiers seen elsewhere: `production_bonus`, `production_boost`, `production_increase`,
`production_debuff`, `production_limit_multi`, `bonus_production` (advisors),
`chance_for_free_production`, and `Stats_mods_cell@stats_mods` which sets
`cell_production_multi`.

**What to establish next:** find where the displayed number is produced. If the
building's Draw event reads a variable holding the *base* amount and applies nothing,
then the fix is to compute the modified value and write it — option (1), and the game
draws it. Start with `obj_improvement_parent_Step_0` (it already reads
`cell_production_multi`) and the Draw events of a specific producer such as
`obj_improvement_animal_farm`.

**Caution:** writing a value the game later recomputes will flicker. Prefer writing in
the same step the game reads it, or find the one place it is derived.

---

## 2. Production building replacement

**The hardest of the three, and the one that needs option (3).**

What exists in the game already:

| variable | where | reading |
|---|---|---|
| `can_not_be_replaced` | `obj_improvement_parent_Create_0`, and `get_tiles_priorities@town_setup_place_improvement` | buildings already have a replaceable/not flag, consulted when the game places improvements itself |
| `dnd_manage_cell_activation` | `obj_improvement_parent` | the drag-and-drop hook for cell activation — the placement path |
| `cell_placed_on`, `get_cell_id`, `cell_type` | `obj_improvement_parent` | which tile a building occupies |
| `on_improvement_destroy`, `on_improvement_destroy_specific` | `obj_improvement_parent` | **the expiry hook to hang auto-build off** |
| `building_charges`, `infinite_charges` | `obj_improvement_option`, `improvement_library` | how many uses a building has before it goes |

A **false lead worth recording** so nobody follows it twice: `replace_time`,
`replace_timer`, `replaced_improv_name`, `replaced_button_obj` and `replaced_info_obj`
look exactly like a building-replacement system and are nothing of the kind — they
belong to `boss_mods_library`, `ascensions_library` and `kings_library`. Only
`can_not_be_replaced` is about placement.

The blueprint side is `obj_improvement_option` inside
`obj_improvement_options_container`, which is what the player drags and where a queued
building would have to be greyed out.

**Shape of the feature, once drawing is solved:**

1. Watch for a drag that drops a blueprint onto an occupied production cell whose
   charges are nearly out. The game presumably refuses this today; the mod records it
   instead as a queued replacement `(cell id, improvement name)`.
2. On `on_improvement_destroy` for that cell, place the queued building through the
   game's own placement method — never by inventing one. This follows the rule from
   `calling-into-the-game.md`: invoke the game's own routine rather than reproducing
   its effects.
3. Grey out the queued blueprint in the container, and cancel on re-selection.
4. Persist nothing: a queue that survives a reload would have to agree with a save
   the mod does not own.

**Establish first:** what the game does today when a blueprint is dropped on an
occupied cell, and whether `dnd_manage_cell_activation` is a method the mod can read
the drop from. That is one live session with the `survey`-style diagnostic, not more
disassembly.

---

## 3. Unit stats on hover

**Reachable via option (2) if the hover panel builds its text into a variable.**

The stats the request asks for all exist on `obj_unit_parent`:

| wanted | variable | notes |
|---|---|---|
| range | `attack_radius` | also `attack_preset_aoe_circle`, `attack_preset_aoe_front_ellipse` for shape |
| attack speed | `attack_time`, `attack_timer` | with `attack_spd_multi` as the modifier |
| attack animation startup | `attack_action_frame` | the frame the hit lands on; combine with `attack_img_speed` to get a time |
| (bonus) damage | `damage_multi`, `damage_received_multi`, `charge_damage_min/max` | |

Also present and useful: `attack_in_progress`, `attack_frames_performed`,
`attack_sprite`, `aimed_at_by_shooters_amount`.

`obj_improvement_parent` has `uses_delayed_hover`, and there is a component framework
(`gml_Script_Component_hover`, plus `component_SV_*` variables) which is where a hover
panel is likely assembled. **No object with `info`, `panel` or `tooltip` in its name
exists**, so the panel is drawn by the hovered thing itself or by that component
system.

**Establish first:** hover a unit in a live session and find which instance holds the
panel text. If it is a string variable, option (2) applies and this becomes the
easiest of the three features. If the text is drawn directly from fields without an
intermediate string, it needs option (3).

---

## An unresolved hazard in reading instance fields

All three features start by reading fields off a live instance. `tkiw-runtime` now has
the primitive for it — `Globals::get_on(instance_ptr, var_id)` and `num_on`, which work
because the same `vtable+8` getter serves globals, instances and structs.

**But that is a *call* into the GML runtime, not a read**, and one question about it is
open: what happens when the variable is **not set on that instance**.

Compiled GML only ever reads variables it knows are present, so the getter has never
needed to be defensive. `ds_map_find_value` is known to call `YYError` — a fatal dialog,
not an error return — on a bad argument, so a getter that does the same for an undefined
variable would put a dialog in a player's game. `get_on` validates the *returned*
pointer and fails to `None`, which covers a wrong instance pointer but not this.

This matters because the natural first move is to sweep every `obj_improvement_*`
instance asking for `main_product`, and buildings that do not produce anything will not
have it.

**Establish it deliberately, with one instance and one variable known to be absent,
before sweeping anything.** Two safer routes if it turns out to throw:

* read `has_production_cycle` first and only ask further questions of instances where it
  is true — the game's own guard, reused;
* or use the runtime's `variable_struct_get_names` (already wrapped in
  `tkiw_runtime::builtin`) to enumerate what an instance actually has, and ask only for
  those. Slower, allocates a GML array that is never freed, and therefore fine for a
  one-shot diagnostic and not for a per-frame feature.

A diagnostic feature to dump this state was written up to here and deliberately **not
shipped**, because it would have called the getter on arbitrary instances with arbitrary
variable ids, and that is precisely the untested case.

## Suggested order

1. **Modified production values** — most likely to need no drawing at all.
2. **Unit stats on hover** — needs one live session to find the text variable.
3. **A trampoline in `tkiw_runtime`**, with someone watching the screen.
4. **Production replacement** — last, because it needs all of the above plus drag
   interception.
