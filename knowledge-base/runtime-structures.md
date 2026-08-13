# Runtime structures

Everything here is reachable with pure reads. No code patching is required to
read any of it. Offsets are from the 2026-08-10 build; the shapes are GameMaker
runtime internals and change rarely, but verify before trusting.

## RValue

The universal value type: **16 bytes**.

```
+0x00  payload   (8 bytes: double, pointer, or integer)
+0x08  flags     (u32)
+0x0c  kind      (u32)
```

| kind | meaning | payload |
|---|---|---|
| 0 | real | `f64::from_bits` |
| 1 | string | pointer to a RefString descriptor |
| 2 | array | pointer to an array header |
| 5 | undefined | — |
| 6 | struct / object | pointer to the struct |
| 7 | int32 | sign-extend the low 32 bits |
| 10 | int64 | the payload as `i64` |
| 13 | bool | non-zero is true |
| 15 | reference | see below |
| 0xFFFFFF | unset | a member that was never written |

Accept **every** numeric kind when reading a number. Struct fields that look
like plain numbers frequently come back as int64 (kind 10), so a reader that
only accepts kind 0 silently never matches — which looks exactly like the field
being absent.

### Strings (kind 1)

The payload is **not** the characters. It points at a descriptor:

```
+0x00  char* data
+0x08  u32 refcount
+0x0c  u32 size      (mask with 0x7fffffff)
```

### Arrays (kind 2)

```
+0x08  pointer to contiguous 16-byte RValue elements
+0x24  i32 length
```

### References (kind 15)

The payload splits into `{high dword = ref type, low dword = id}`:

| ref type | what |
|---|---|
| `0x01000005` | script |
| `0x02000001` | `ds_list` |
| `0x02000002` | `ds_map` |
| `0x04000001` | instance |

An instance reference carries an **id**, not a pointer. Ids are `>= 100000`. To
get a pointer you must go through the instance-id hash (below).

## Variable access — globals, instances and structs alike

One interface covers all three, which is the single most useful thing in this
document.

```
container = *(base + 0x2af7a08)     ; the global container
vtable    = *container
get       = *(vtable + 0x08)        ; read
get_w     = *(vtable + 0x10)        ; read-for-write

get(container_or_instance_or_struct, var_id) -> RValue*
```

The same `vtable+8` call works when you pass an instance pointer or a struct
pointer instead of the global container. So one function reads a global, an
instance variable, and a struct member.

`var_id` comes from the variable name table (see [orientation.md](orientation.md)).

### Not every member name has a variable id

This costs a session if you do not know it. The variable table only holds names
that appear **literally in the game's code**. Data-driven keys do not. The
player's resources are a struct keyed by resource id, and `resources.coin` is
never written that way in source — so `var_id("coin")` returns nothing, and any
reader built on it silently returns "absent" forever.

For those, go through the runtime by name:

```
variable_struct_get_names(struct) -> array of GML strings   ; RVA 0x1b00560, argc 1
variable_struct_get(struct, name)  -> value                 ; RVA 0x1b00380, argc 2
```

Trick worth reusing: you need the name as a *GML string* to call
`variable_struct_get`, and allocating one means asking the runtime for memory.
You do not have to. `variable_struct_get_names` hands back an array of exactly
the strings you might want — find the matching element and pass that RValue
straight back in. Nothing is allocated on your side.

Note that `variable_struct_get_names` allocates a GML array that is never freed.
It is fine for one-shot diagnostics and startup work; do not call it per frame.

## Objects and instances

```
object registry = *(base + 0x2b011d8)
    +0x00  void** buckets      ; slots are 16 bytes {head, tail}
    +0x08  i32 mask
    +0x0c  i32 count

registry node
    +0x08  next
    +0x10  i32 key             ; object index
    +0x18  void* value         ; CObjectGML*

CObjectGML
    +0x00  const char* name    ; "obj_card_resource"
    +0x68  instance list head
    +0x94  i32 object index

instance list node
    +0x00  next
    +0x10  CInstance*          ; 0 marks the sentinel tail

CInstance
    +0x00  vtable
    +0x90  CObjectGML*         ; must match the object you looked up
    +0xB8  u32 flags           ; alive 0x4, dead mask 0x00100003
    +0xBC  i32 id              ; >= 100000
```

Finding instances by object name is a registry walk. **Cache the object
records** — they are created at load and do not move — but re-verify the name on
each cache hit so a stale entry falls back to a fresh walk instead of being
trusted. Walking all ~1,750 registry entries per lookup per frame made the game
unplayable; see [pitfalls.md](../notes-for-claude/pitfalls.md).

### Instance id to pointer

```
hash = *(base + 0x2974450)
mask =  (base + 0x2974458)
node   +0x08 next   +0x10 i32 id   +0x18 CInstance*
```

## ds_list

```
table = *(base + 0x2affde8)
count = *(i32*)(base + 0x2affdf0)
CDsList
    +0x08  size
    +0x18  items          ; element i is at items + i*16, an RValue
```

## ds_map

```
table = *(base + 0x2affdd8)
count = *(base + 0x2affdcc)

map +0x00 -> HashTable
    +0x00  buckets
    +0x08  mask
bucket
    +0x08  next
    +0x18  pair
pair
    +0x10  value          ; key is at the pair start
```

Library globals (`ARTIFACTS`, `IMPROVEMENTS`, …) are `ds_map`s keyed by the
system name string, whose values are structs. Grouping globals
(`IMPROVEMENTS_BY_CATEGORY`) are `ds_map`s with **numeric** keys whose values
are arrays of strings — so handle both string and numeric keys when walking.

## Reading safely

Validate every hop and fail to `None`. A `VirtualQuery`-backed `readable()` check
before each read is the right idea, but cache the results per frame or it will
dominate your runtime. Two caching details that matter:

* Remember the whole region returned, not the bytes you asked for. The next read
  is almost always a few bytes further into the same allocation.
* Keep a one-entry front cache of the last region hit, checked before any scan.
  Reads are extremely local, so nearly every check is answered by one
  comparison. Without it, a linear scan of the cache runs thousands of times per
  frame and becomes the largest single cost in the mod.

Flush the cache once per frame. Doing it on the game's own thread is what makes
that sound: nothing else mutates the process between your reads.
