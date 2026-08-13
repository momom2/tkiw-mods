# Calling into the game

This is the dangerous part. A wrong read returns `None`; a wrong call corrupts
memory or kills the process on a player's machine mid-run. Read all of this
before making a single call.

## There are three conventions, and they are different

### 1. Runtime builtins

The 2,767 functions recovered from `Function_Add`.

```
f(RValue* result, void* self, void* other, int argc, RValue* args)
```

`args` is a flat array of 16-byte RValues. This is the convention proven first
and the easiest to get right.

### 2. Compiled GML

Anything named `gml_Script_*` or `gml_Object_*`.

```
f(CInstance* self, CInstance* other, RValue* result, int argc, RValue* args)
```

Note that `self`/`other` and `result` are in the **opposite order** to the
builtin convention. Getting this wrong writes a result through a pointer that is
actually an instance.

### 3. The method invoker

For a bound method held in an instance variable — which is how the game holds
`button_pressed_action`, `resolve_reroll_cost`, `hide_units_icons` and most of
its interesting behaviour.

```
RVA 0x1aa47f0
f(self, other, RValue* result, int argc, RValue* method, RValue* args)
```

The method is passed **by pointer**, not by value. This was taken from the
game's own call site:

```asm
call [rax+8]          ; fetch the method -> RValue* in rax
mov  [rsp+0x28], r15  ; args = null
mov  [rsp+0x20], rax  ; the method, BY POINTER
xor  r9d, r9d         ; argc = 0
lea  r8, [rbp-0x50]   ; result
mov  rdx, r12         ; other
mov  rcx, rdi         ; self
call 0x1aa47f0
```

## Pressing a button

The game has no "pressed" flag to set. `obj_cursor` detects the hover and
invokes that instance's `button_pressed_action` method. So to press a button you
invoke that method.

The safest route is not to use the compiled-GML convention at all. Use a
**builtin** to do it for you:

* `script_execute(callable, ...args)` — RVA `0x1c4ebc0`, variadic. Preferred
  over `method_call`, which wants its arguments as a GML array and would mean
  allocating one just to express "no arguments".
* Check it really is a method first with the game's own `is_method` — RVA
  `0x1adab90`, argc 1. Do not assume.

### `self` must be the owning instance

Passing null crashed the game. `button_pressed_action` opens with:

```asm
mov rcx, [rbp+0x70]   ; load self
mov rax, [rcx]        ; immediately dereference it
```

It reads `self.card_parent` as its first act. An earlier successful builtin call
(`variable_struct_get_names`) simply never touches `self`, which hid the
omission until something did.

## Rules that were learned the hard way

**Rate-limit every press.** A press that does not visibly change anything is a
bug to investigate, not something to repeat sixty times a second. Track the last
instance pressed and refuse a repeat inside a floor of a few tens of ms.

**Wrap calls in `catch_unwind` and run on the game's thread.** A panic must
disable your mod, never propagate into the game.

**Not every method is a getter.** `resolve_reroll_cost` looks exactly like one.
It is a *setter*: it works the price out from `cost_initial` and
`cost_increase_per_reroll` and **writes it to** `self.resource_cost`, returning
undefined. Nearly two sessions went into calling it different ways before
disassembling it. If a method returns kind 5 consistently, stop varying the call
and go read the function.

**Objects the game is still building are not safe to read, and neither are ones
it is tearing down.** Both ends. Check a readiness condition before reading a
freshly-created instance's members, and leave a window after you destroy
something before reading anything near it. Reading a half-built card's array is
the most likely cause of a crash that took several sessions to corner.

**Invoke the game's own teardown rather than inventing one.** If you cause a
state the game cannot normally reach — say, destroying a card while the mouse
still believes it is hovering it — find the method the game itself calls and
call that. For hover, the card's Step event does:

```
is_hovered != was_hovered -> call show_units_icons or hide_units_icons
                             then was_hovered = is_hovered
```

so calling `hide_units_icons` on a card whose `was_hovered` is true produces
exactly the screen the player would have seen had they moved the mouse away.

## Getting the disassembly

`analysis/gmldis.py` gives an annotated disassembly of any named function, with
variable names, string constants and call targets resolved inline. It is the
tool that answers "what does this method actually do", and reaching for it
earlier would have saved most of the time lost to guessing.

```bash
python analysis/gmldis.py gml_Object_obj_card_class_stat_bonus_Step_0
python analysis/gmldis.py --grep reroll
```

Anonymous methods do not appear under the name they are assigned to. Disassemble
the *Create* event, find where the member is written, and read off the
`anon@NNNN@...` address being bound to it.
