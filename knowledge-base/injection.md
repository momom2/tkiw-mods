# Getting loaded, and getting onto the game's thread

## Proxy DLL

The game statically imports a DLL that is **not** a KnownDLL, so a copy placed in
the game folder wins the loader search and gets loaded into the process before
anything else runs. No launcher, no injector, no patching of the executable.

Forward all exports to the real one in `System32`. A uniform trampoline that
lazily loads the real DLL and jumps through it covers them all without
hand-writing a thunk per export — see "the uniform signature" below.

### Choosing a slot

**Rank candidates by how many exports are ordinal-only, not by how many exports
there are.** This is the one thing to know, and it is counter-intuitive.

A proxy can only provide exports it can *name*. An import it cannot satisfy does
not degrade: the importing DLL fails to load outright, with nothing pointing at
you as the cause. Several of this game's imports export most of their table by
ordinal only, so the DLL with the fewest exports can be the one you cannot proxy
at all.

`tools/gen_proxy.py --survey` produces this, and the numbers for the 2026-08-10
build are:

| candidate | game imports | named exports | ordinal-only |
|---|---|---|---|
| `mfreadwrite.dll` | 1 | 7 | 0 |
| `version.dll` | 3 | 17 | 0 |
| `d3d11.dll` | 1 | 51 | 0 |
| `mfplat.dll` | 8 | 234 | 0 |
| `iphlpapi.dll` | 2 | 313 | 0 |
| `winmm.dll` | 6 | 180 | 1 |
| `dbghelp.dll` | 1 | 252 | 16 |
| `dwmapi.dll` | 3 | 44 | 75 |

`dwmapi` looks like the obvious pick on export count and is unusable: three
quarters of its table has no names. Avoid `d3d11` despite its clean numbers — the
Steam overlay hooks it, and a proxy in that slot invites a confusing failure
inside someone else's code.

**Slots in use:** `version.dll` by `tkiw-reward-auto-picker`, `mfreadwrite.dll` by
`tkiw-momomod-kit`. A third mod should take another zero-ordinal candidate — or,
better, be a feature of the kit rather than a third DLL.

### Two mods can share the frame hook

Both proxies want the `PeekMessageW` IAT slot, and that works: each reads what is
currently in the slot and calls it, so whichever installs second chains onto the
first, order-independently. Load order follows the import directory and is not
worth reasoning about.

The sharp edge is **uninstall**. Restoring the original pointer while another
mod's hook sits on top of yours orphans it. So do not restore the slot except at
process detach, where it does not matter.

### A proxy's failure return belongs to the DLL being proxied

When forwarding fails — the real DLL will not load, or has no such export — the
forwarder has to return *something*, and **what "failure" looks like is a property of
the DLL you are impersonating.**

`version.dll`'s exports return `BOOL`/`DWORD`, where zero means failure, so `return 0`
is right. Copying that into a proxy for a COM DLL inverts it: every `mfreadwrite.dll`
export returns an `HRESULT`, and zero is `S_OK`. The forwarder was therefore telling
its caller *"your call succeeded"* while writing nothing to the output pointer — the
caller then uses a null interface pointer, and the failure surfaces as a crash or a
wait that never ends, nowhere near the proxy.

Check the return convention of the specific DLL and pick the sentinel deliberately.
`E_FAIL` (`0x80004005`) for COM, zero for the Win32 `BOOL` style.

### On the uniform signature

Declare every forwarder with the same generous parameter count — ten `usize` — and
pass them all on. The x64 ABI puts the first four in registers and the rest on the
caller's stack, so extra parameters read mapped stack that the callee ignores
because its own arity is lower. One trampoline, no per-export signature to get
subtly wrong.

**Integers and pointers only.** A float or double travels in XMM registers, which a
trampoline declared in terms of `usize` is free to clobber. Check the candidate has
no floating-point exports before relying on this; none of the ones above do.

### The failure return is a property of the DLL, not of the trampoline

When a forwarder cannot reach the real function, what should it return? This is
**not** a detail to carry over from another proxy, and getting it wrong is silent.

* `version.dll`'s exports return `BOOL`/`DWORD`, where **zero means failure**. So
  `return 0` is correct.
* `mfreadwrite.dll`'s exports all return `HRESULT`, where **zero means _success_**.
  The same `return 0` tells the caller its call worked, and the caller then uses an
  output pointer that was never written — a null interface, followed by a crash or a
  wait that never ends. The right answer is `E_FAIL` (`0x80004005`).

The momomod kit's proxy inherited `return 0` from the auto-picker's and inverted the
sentinel by doing so. Decide this per DLL, from the actual return type of its
exports, and write down which convention you chose and why.

Why this is the right choice here:

* **Steam's integrity check does not care.** It verifies the game's own files.
  It may delete an added `version.dll`, in which case the player re-runs the
  installer — but it never triggers a re-download, and it never leaves the game
  unlaunchable.
* **Uninstalling is deleting one file.** Nothing about the executable is
  touched, so there is nothing to restore and nothing to get wrong.
* **The game folder stays visibly clean.** Keep config, logs, and backups in the
  mod's own folder, not next to the game. The installer can bake the mod folder
  path into the DLL so it can find its way home.

### Baking a path into the DLL

If the installer patches a path into a `static` byte array, read it with
`read_volatile` and `black_box`. LLVM will otherwise constant-fold the
immutable initial value and the patched bytes are never seen. This is a real bug
that was hit and is completely silent.

## DllMain

`DllMain` runs with the loader lock held, where almost anything interesting —
loading a library, touching another module, blocking — can deadlock. Do nothing
there but `CreateThread` to your real startup, and `DisableThreadLibraryCalls`.

### Your startup thread is earlier than you think

It runs **before the game's entry point**. Within a millisecond of the process
starting, the GameMaker runner has done nothing: no global-variable container, no
object registry, no data-structure tables. Reading the container there returns a
null pointer.

So startup can only use what comes from the file on disk or from already-mapped
code — the symbol tables, the section bounds, the baked code signatures. Everything
that needs the live runtime has to resolve **lazily from the frame hook, retrying
until it works**, with "not yet" treated as a normal answer rather than a failure.
See [pitfalls.md](../notes-for-claude/pitfalls.md) for the table and for the launch this cost.

Measured on the 2026-08-10 build: the startup thread is running at about 4 ms, and
the global container becomes readable much later — the gap is the runner reading a
530 MB `data.win` and building its asset tables, and no mod can shorten it.

## Getting onto the game's thread

You must run on the game's own thread to touch its state safely. Hooking the
**IAT slot for `PeekMessageW`** does this with no code patching at all: write
your function pointer into the import table entry, and you are called every time
the game pumps messages.

On the 2026-08-10 build that slot is at RVA `0x21ca8a8`. Find it by walking the
import directory rather than baking it.

Two things about the callback rate:

* It is **wildly uneven** — tens of thousands per second while loading, about
  sixty per second in play. Pace anything you do by elapsed time, never by frame
  counts.
* Guard against **re-entrancy**. You are called from inside `PeekMessageW`, and
  if game code you invoke pumps messages, you are called again underneath
  yourself. An atomic flag with a `Drop` guard costs nothing and turns a
  potential deadlock or recursion into a skipped frame. (Measured on this game:
  it does not actually happen — but the guard is still correct.)

## Surviving your own bugs

An access violation takes the process down with no unwinding, no `catch_unwind`,
and no chance to write anything. Two protections earned their keep:

**A crash-loop breadcrumb.** Write a marker file before your startup probe and
remove it on clean exit. Finding one at launch means the last session died, so
this session stays passive and the game launches normally. Never break a
player's game twice for the same reason.

Two refinements that matter, both learned by getting them wrong:

* **Stand the guard down once the session is clearly healthy** — say a minute of
  frames. The breadcrumb exists to catch a *probe* that kills the game at
  launch. Clearing it only at process exit means an unrelated crash three
  minutes into play costs the player the whole next session, and looks
  identical to the mod simply being broken.
* **Give the player a way back in.** A held session should still respond to the
  mod's hotkey by probing anyway. Poll the keyboard from your startup thread —
  that touches no game memory, so a session held back for safety stays exactly
  as safe until the player decides otherwise.

**A crash reporter.** `AddVectoredExceptionHandler` sees the fault before the
game's handlers; add `SetUnhandledExceptionFilter` as a second net. Log the
exception code, the faulting address, its RVA into the game, and the address
being touched, then return `EXCEPTION_CONTINUE_SEARCH` so nothing about the
game's behaviour changes.

Write the handler as if the process is already broken, because it is:

* **Open the file handle at startup**, not in the handler.
* **No allocation, no formatting** — assemble into a fixed stack buffer. A heap
  that is the reason you are here will not serve a `format!`.
* **`try_lock`, never `lock`.** A fault can be raised inside your own logger
  while it holds its mutex, and a handler that waits turns a crash into a hang,
  which is strictly worse for the player.

A reporter written the comfortable way produced nothing on two separate crashes.
Treat silence from your own instrumentation as a defect in the instrumentation.

## Logging

Keep the file handle open. Opening and closing per line is a system call per
line, and during a burst of activity that dominates. Drop the handle on
rotation, and reopen if the file goes missing so that moving the log aside
mid-session still works.

If you rotate on size, **carry your one-shot findings across the rotation**.
Every useful diagnostic reports once, in the first minute; the thing it explains
happens minutes later. One rotation in between leaves a log holding the crash
and none of the findings that make sense of it.
