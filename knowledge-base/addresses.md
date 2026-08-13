# Baked addresses

**These are RVAs for the 2026-08-10 build. They will be wrong after a game
update.** Everything else a mod needs is resolved by name at runtime and
survives updates; this file is the exception list.

## Code

Each of these should carry a byte signature in your mod, checked at startup
against the loaded image. On mismatch, disable the mod and say which check
failed — do not call into whatever now lives there.

| RVA | what | argc |
|---|---|---|
| `0x1aa0df0` | `Object_Find` (the registry walk this reproduces) | — |
| `0x1aa47f0` | method invoker | see [calling-into-the-game.md](calling-into-the-game.md) |
| `0x1ad71f0` | `array_length` | 1 |
| `0x1adab90` | `is_method` | 1 |
| `0x1affb80` | `variable_get_hash` | 1 |
| `0x1b00380` | `variable_struct_get` | 2 |
| `0x1b00560` | `variable_struct_get_names` | 1 |
| `0x1b07b90` | `ds_list_size` | 1 |
| `0x1b08f70` | `ds_map_find_value` | 2 |
| `0x1b6bcf0` | `Function_Add` (walk its call sites for the builtin table) | — |
| `0x1c4ebc0` | `script_execute` | variadic |

## Data

These cannot be signature-checked. Validate every use and fail to `None`.

| RVA | what |
|---|---|
| `0x2974450` | instance-id hash table |
| `0x2974458` | instance-id hash mask |
| `0x2af7a08` | global variable container |
| `0x2affdcc` | `ds_map` count |
| `0x2affdd8` | `ds_map` table |
| `0x2affde8` | `ds_list` table |
| `0x2affdf0` | `ds_list` count |
| `0x2b011d8` | object registry descriptor |
| `0x21ca8a8` | IAT slot for `PeekMessageW` (find by walking imports instead) |

## How to re-derive them after an update

**Builtins by name.** `analysis/builtins.py` walks the `Function_Add` call sites
and gives you name → `(rva, argc)` for all 2,767. This covers most of the code
table above outright. Start here; it is nearly free.

**`Function_Add` itself.** Find the function called ~2,769 times with a string
pointer as its first argument.

**The data tables.** Disassemble a builtin that uses one. `ds_list_size` opens
by loading the `ds_list` table; `ds_map_find_value` loads the map table. The
global container appears in any compiled GML that reads a global. `Object_Find`
gives you the object registry. `analysis/gmldis.py` annotates rip-relative data
references, so the address is usually visible in the first few instructions.

**The IAT slot.** Walk the import directory for `USER32.dll` /`PeekMessageW`.
Never bake it.

## A note on the guard

The by-name symbol check passing tells you nothing about whether these addresses
are still right — the two are independent. A mod that checks only names will
look perfectly healthy on a new build and then call into arbitrary code on a
player's machine, with no explanation and no way for them to know why. The byte
signatures are what make the difference between "stops working and says so" and
"misbehaves silently".

Keep a test that verifies the signatures against the executable on disk, so a
game update is caught at build time rather than in a player's game.
