# Spec: TKIW's momomod Kit

A single injected DLL hosting many small, independent changes to *The King is
Watching* — quality-of-life fixes, bug fixes, and optimisations — each one
separately switchable and separately able to fail.

Status: **built and shipping.** The loader, the feature contract, the per-feature guard
and the scheduler all work; the diagnostics (`timeline`, `profiler`, ...) and
`popup_stutter_fix` exist, and the published mods (bugfixes, auto-picker) plus the
`gameplay` unit-stats overlay are plugin DLLs. `fast_boot` and `font_atlases` are
**quarantined** (see `quarantine/README.md`). The auto-picker shares the runtime layer.

What is *not* built, and why, is in §11.

---

## 1. Why a kit and not a mod

The first two mods for this game were each one change in one binary. That does
not scale to a dozen small changes, for one reason above all others: **a game
update breaks addresses, and an all-or-nothing guard turns one broken change
into a dead mod.** The reward auto-picker checks nine byte signatures at startup
and disables *itself* if any one disagrees. For a mod that does one thing, that
is right. For a mod that does twelve, it means a game update that moves one
function costs the player the other eleven.

So the unit of health is the **feature**, not the mod. Each feature declares
what it depends on; the loader checks each feature's dependencies separately and
disables only the ones that no longer hold, naming them. Eleven features keep
working.

Everything else the kit centralises — one symbol pass, one frame hook, one log,
one crash reporter, one install — follows from having decided to host more than
one thing.

## 2. Features are compile-time, not plugin DLLs

A feature is a Rust module in this crate, not a DLL the loader discovers at
runtime. Dynamic loading was considered and rejected:

* Rust has no stable ABI, so plugins would need a hand-written `repr(C)` service
  vtable — read memory, resolve a symbol, find an instance, invoke a method,
  log, read config — versioned forever, for a repository with one author.
* The isolation is illusory. An access violation in a plugin DLL takes the
  process down exactly as one in the loader would. Only panics are catchable,
  and those are catchable in a single binary.
* One binary means one `cargo test`, compiler-checked refactors, and one zip to
  install.

Players lose nothing perceptible: config already gives them mix-and-match.

## 3. Layout

```
tkiw-runtime/            shared crate, path dependency
    pe win gml globals builtin dslist rvalue instance
    hook log fault home saves guard identity
tkiw-momomod-kit/        this crate: cdylib
    src/lib.rs           startup, the registry, the scheduler
    src/proxy.rs         dwmapi.dll export forwarding
    src/config.rs        the ini
    src/features/        one module per change
```

`tkiw-runtime` is shared source, not a shared process: each mod that uses it
links its own copy. It exists so that a fix to the region cache or the crash
reporter lands once. `tkiw-reward-auto-picker` may migrate onto it later; it is
not required to.

## 4. Delivery

**A proxy `dwmapi.dll` in the game folder.** The game statically imports it and
it is not a KnownDLL, so a copy in the application directory wins the loader
search and is mapped before the game's entry point. 44 exports are forwarded to
the real one in `System32`; the game itself uses three of them
(`DwmSetWindowAttribute`, `DwmGetWindowAttribute`,
`DwmGetCompositionTimingInfo`).

`version.dll` — the slot the auto-picker uses and the better-documented choice —
is deliberately left alone so both mods can be installed at once. **When the
auto-picker is absorbed as a feature, the kit moves onto `version.dll` and
`dwmapi.dll` is retired.** That change is the export trampoline and one string in
the installer.

Both mods hooking the same `PeekMessageW` IAT slot is fine: each reads what is
in the slot and calls it, so whichever installs second chains onto the first,
order-independently. The one sharp edge is uninstall — restoring the original
pointer while another hook sits on top of it orphans that hook — so the kit
**never restores the slot except at process detach**, and says so in `hook.rs`.

Everything the kit owns lives in the kit's own folder: `momomod.ini`,
`momomod.log`, `crash.log`, `save-backups/`. The game folder gets exactly one
added file. `install.py` stamps the kit folder's absolute path into the DLL as it
copies it, behind the marker `TKIW_MOMOMOD_DIR=`, which is also how
`uninstall.py` proves a `dwmapi.dll` is ours before deleting it.

## 5. The feature contract

```rust
pub trait Feature: Send + Sync {
    /// Config section key. Stable: it is what a player's ini says.
    fn name(&self) -> &'static str;

    /// One line, written into the generated config as a comment.
    fn summary(&self) -> &'static str;

    /// What must still be true of the game for this feature to be safe.
    /// Checked by the loader before `activate`, and never by the feature.
    fn requires(&self) -> Requirements;

    /// Read this feature's own config keys. An error leaves the feature off.
    fn configure(&mut self, section: &Section) -> Result<(), String>;

    /// Take effect. Install code patches and IAT hooks here, not in `on_frame`.
    fn activate(&mut self, rt: &Runtime) -> Result<(), String> { Ok(()) }

    /// Undo exactly what `activate` did. Must be safe to call when inactive.
    fn deactivate(&mut self, rt: &Runtime) {}

    fn cadence(&self) -> Cadence { Cadence::Never }
    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> { Ok(()) }
}
```

`enabled` is **not** the feature's business: the loader owns it, reads it from
`[feature.<name>] enabled`, and defaults it per feature.

### Requirements

The heart of the design. A feature declares its dependencies as data:

```rust
Requirements {
    // resolved by name from the game's own symbol tables; survives code moving
    functions: &["gml_Object_obj_splash_screen_Step_0"],
    variables: &["warmup_frames", "goto_menu"],
    // baked addresses, each with the bytes that must be there
    signatures: &[("splash step", 0x17c6870, &[0x48, 0x8b, ...])],
    // object names that must exist; checked in-game, absent is not a failure
    objects:   &["obj_splash_screen"],
}
```

The loader probes these at startup, per feature, and:

* all hold → the feature may activate;
* any fails → **that feature only** is disabled, and the log names the
  requirement, not just the feature. "skip_splash: variable `goto_menu` no
  longer exists" is a bug report; "skip_splash disabled" is not.

A small set of addresses is *shared* — the global container, the object
registry, the instance hash, the `ds_list`/`ds_map` tables — because the whole
runtime is built on them and nothing can work if they have moved. Those are
checked once, globally, and a failure stands the whole kit down. That is the
only all-or-nothing check left.

### Cadence and the frame budget

```rust
enum Cadence { Never, Once, EveryFrame, Interval(Duration) }
```

The loader owns the frame hook and calls features itself. It **times each
feature separately** and keeps a per-feature moving average. A feature that
overruns its budget has its interval widened; one that keeps overrunning is
disabled for the session and named.

This is not speculative. `notes-for-claude/pitfalls.md` records both halves of getting it wrong
in the auto-picker: a kill switch written for the diagnostic sweep sat at the top
of the frame hook and silently killed the real feature too, and timing two things
together let the cheap one be blamed for the expensive one's cost. With a dozen
features sharing one hook, per-feature accounting is the only version of this
that stays honest.

### Panics

Every call into a feature — `configure`, `activate`, `on_frame` — is wrapped in
`catch_unwind`. A panic disables that feature for the session and names it. The
loader and every other feature carry on. An access violation is not catchable and
takes the process down; that is what the crash reporter is for.

## 6. Config

`momomod.ini`, in the kit's folder, re-read when its modification time changes.

```ini
[kit]
trace = false            # log each feature call as it begins; verbose
survey = false           # expensive periodic diagnostics, for bug reports

[feature.skip_splash]
enabled = true

[feature.profiler]
enabled = false
interval_ms = 1
```

Rules, following the auto-picker's precedent where it earned it:

* **A feature with no section uses its defaults**, which are conservative. An
  unknown section or key is logged and ignored, never fatal — a config written
  for a newer build must not brick an older one, or the reverse.
* **A failed reload keeps the last known-good config** and says so loudly. A bad
  edit mid-run never silently changes behaviour.
* **An existing config is never overwritten.** If features have been added since
  it was written, the kit writes `momomod.reference.ini` beside it — the same
  file freshly generated — so the two can be diffed. It is written again only if
  absent.
* **Installing without configuring changes nothing** unless a feature's default
  is on. Features default off; the exceptions are stated in the generated config
  and in the readme, per feature, with a reason.

## 7. Failure behaviour

Everything fails toward "the game behaves exactly as it does unmodded".

| what | what happens |
|---|---|
| shared baked addresses have moved | the whole kit stands down, log says which check failed |
| a feature's requirement fails | that feature only is disabled, the requirement is named |
| a feature panics | that feature only is disabled for the session |
| a feature overruns its frame budget | its interval widens, then it is disabled |
| the stamped kit folder is missing | kit stands down, note to `%TEMP%` (the only thing ever written outside the kit folder) |
| config missing | generate the default |
| config invalid | keep the last good one, log loudly |
| the previous session died at startup | stay completely passive this launch; the log's first line says so; the hotkey overrides |

The crash-loop breadcrumb stands down once a session has run cleanly for a
minute, so an ordinary mid-session crash does not cost the player the next
launch. **Ctrl+Alt+M** re-probes a held session, and is polled from the startup
thread — which touches no game memory, so a session held back for safety stays
exactly as safe until the player asks otherwise.

Individual features are also switchable at runtime by editing the config; the
hotkey is for the case where the kit has stood itself down and there is nothing
to edit.

## 8. What goes in, and what does not

A feature belongs in the kit if it is **small, separable, and defensible as
something the game arguably should have done**. Optimisations, bug fixes, and
conveniences that remove a chore.

Out: anything that changes the balance of a run, anything that gives the player
information the game deliberately withholds, and anything whose failure mode is
a silently wrong game state rather than a visibly absent feature.

Each feature's own section in the readme states what it changes and what it
cannot do, because a player debugging their game needs to know which of a dozen
features could be responsible.

## 9. First payload — as built

1. **`timeline` and `profiler`** — diagnostics, off by default. Built first because the
   optimisation targets could not be chosen honestly without them, and that judgement
   paid off immediately: the two headline complaints turned out to have completely
   different causes, and one of them is not the game's fault at all.
2. **`fast_boot`** — the startup win the profiler identified. **Now quarantined** (see
   `quarantine/README.md`): the honest overnight measurement put it at ~6s (~13%), not
   the "~30%" first claimed, and the author parked it for now. See
   [`analysis/FINDINGS.md`](analysis/FINDINGS.md).

## 10. What the measurements changed

Two conclusions that a plan written in advance would have got wrong:

**The main-menu lag spikes cannot be fixed here.** A profile restricted to samples
taken while the game was overdue to draw contains no game code at all — it is `Present`
blocking in the graphics stack on integrated graphics, with the Steam overlay in the
same path. The right advice to a player is outside the mod.

**The startup cost was not I/O.** `data.win` is 530 MB, which invites the obvious
theory; the machine's storage is NVMe and the two hot functions call no Windows API at
all. It is CPU-bound texture decoding.

## 11. Not built, and what stands in the way

Three requested features remain: **modified production values**, **unit stats on
hover**, and **production building replacement**. The groundwork — objects, variables,
mechanisms, and one false lead — is in
[`analysis/gameplay-features.md`](analysis/gameplay-features.md).

They share a blocker. **All three put something new on the screen, and the kit cannot
draw.** Its only foothold on the game's thread is the `PeekMessageW` hook, which runs
after the frame is already finished; GameMaker draws from Draw events. Three ways out,
cheapest first:

1. **Write state the game already draws** — no drawing needed, no risk of a torn
   frame. Probably enough for modified production values.
2. **Extend a string the game already builds** — likely enough for unit stats on hover,
   once the panel's text variable is found.
3. **A code detour into a Draw event** — needed for anything genuinely new, such as a
   greyed-out blueprint. `tkiw_runtime::patch` is the verified, revertible half of
   this; what is missing is a trampoline. `tkiw-morale-fix` is precedent.

**(3) should not be attempted by someone who cannot see the screen.** A wrong detour is
a crash or a corrupted frame, and neither appears in a log.

## 12. Build

```
cargo build --release          from the workspace root
cargo test --release           runtime and loader; game-dependent tests skip if absent
python package.py              the zip that ships
```

Rust, MSVC toolchain, `crate-type = ["cdylib", "rlib"]`, **standard library
only, no crates** — the same posture as the auto-picker, for the same reasons: it
builds offline with nothing but rustup, and the FFI surface is small enough that
a binding crate would cost more than it saves.

The shipped zip carries the DLL and the install scripts, no config, no log, and
no stamped path. `install.py` stamps the player's own folder as it installs;
`package.py` refuses to build if the DLL it is given already has one.
