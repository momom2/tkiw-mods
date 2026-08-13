# Orientation

## What the binary is

*The King is Watching* is a GameMaker game built with **YYC**, the compiler that
turns GML into native code. Consequences, in order of how much time they save:

* **`data.win` contains no code.** There are no `CODE`, `VARI` or `FUNC` chunks.
  It holds sprites, rooms, and other assets.
* **UndertaleModTool cannot help you.** Nor can any other `data.win` script
  editor, decompiler, or GML injector. They are all built for the VM target.
  Do not spend a day confirming this; it is confirmed.
* **All game logic is x86-64 in the executable.** Modding means binary work:
  static patching, or injecting a DLL and working with the live process.

The executable is roughly 50 MB with about 86,000 functions in `.pdata`.

## Why this is still tractable

The YYC output is not stripped in the way you might fear. The runtime needs
names for its own reflection, so the executable carries them, and they can be
recovered wholesale.

### Three symbol tables

**1. The function table.** A 24-byte-stride table in `.data`, each record
pairing a `gml_*` name string pointer with a function pointer. About **13,132**
entries. Names look like:

```
gml_Object_obj_button_reroll_cards_Create_0
gml_Script_setup_reroll_button@gml_Object_obj_button_reroll_cards_Create_0
gml_Script_anon@3062@gml_Object_obj_button_reroll_cards_Create_0
```

The `@` form is a nested function: `inner@outer`. Anonymous functions get
`anon@<number>@<outer>` and struct literals get `___struct___<number>@<outer>`.

**2. The variable table.** Pairs of `{const char* name, u32 slot}` in `.data`,
where the slot holds `0xFFFFFFFF` on disk and the runtime writes the resolved
variable id into it during startup. About **13,476** slots. This is how you turn
a name like `pending_rewards` into the integer id the variable getters want.

Read these slots **after** the game has been running a few seconds. Before that
they still read `0xFFFFFFFF`, and a lookup made too early must not be cached.

**3. Runtime builtins.** A function called at RVA `0x1b6bcf0` — call it
`Function_Add` — is invoked 2,769 times at startup with `(name, pointer, argc,
...)`. Walking its call sites recovers **2,767** named runtime builtins:
`ds_map_find_value`, `variable_struct_get_names`, `script_execute`, and so on.
`tools/builtins.py` does this and caches the result.

These are the functions **your mod calls**. They are *not* the functions the
game's own compiled GML calls: YYC knows the argument types at compile time, so
it skips the RValue-unpacking wrapper and calls the inner implementation
directly. A builtin's call-site count across the whole of `.text` is exactly
zero. Do not build a disassembly annotator on this table and then conclude that
a function which obviously reads a file calls nothing at all — see
[runtime-internals.md](runtime-internals.md).

`.pdata` gives function boundaries for everything, which is what makes
disassembling a named function straightforward.

### What this buys you

Almost every symbol a mod needs can be resolved **by name at runtime**, from the
executable on disk, in about 100 ms. That means the mod keeps working when the
game updates and code moves. Only a dozen or so unnamed things — the global
container, the object registry, the `ds_list`/`ds_map` tables — have to be baked
as addresses, and those can be signature-checked. See [addresses.md](addresses.md).

## ASLR

The executable has an image base of `0x140000000` and is relocated. The rule is
`live = GetModuleHandle(NULL) + rva`. Do not compute `rva + slide` where slide
is the difference — that drops the image base and lands you in unmapped memory.
This mistake crashed the game twice; see [pitfalls.md](../notes-for-claude/pitfalls.md).

## What "modding" can mean here

Three approaches, in increasing order of power and risk:

1. **Static byte patch** of the executable. Simple and durable, but Steam's
   integrity check will notice and re-download, and uninstalling means restoring
   a backup. Used by `tkiw-morale-fix`.
2. **Proxy DLL, reading only.** Inject, resolve symbols, read live state. Cannot
   break anything: a wrong read fails to `None`. Surprisingly capable — the
   whole reward queue, every card on screen, and the player's resources are all
   readable.
3. **Proxy DLL, invoking the game's own methods.** This is how you press a
   button without synthesising input. Powerful and genuinely dangerous; get the
   calling convention wrong and you take down someone's run. See
   [calling-into-the-game.md](calling-into-the-game.md).

Prefer 2 over 3, and 3 over 1, unless you need a behaviour change the game has
no method for.
