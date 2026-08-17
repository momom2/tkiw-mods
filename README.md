# TKIW mods

Mods for *The King is Watching*, and the shared knowledge and tooling behind them.

- **[BACKLOG.md](BACKLOG.md)** — what is done, what is next, and what is blocking each.
- **[knowledge-base/](knowledge-base)** — how to mod this game at all. Read
  [`orientation.md`](knowledge-base/orientation.md) first; it is short and it rules out
  the obvious approaches that do not work.

## The four audiences

Everything here is written for exactly one of four readers, and it should always be
obvious which. **This is the organising principle; keep it.**

| audience | where | style |
|---|---|---|
| **Players** | each mod's `README.md`, `install.py`, `uninstall.py`, `dist/*.zip` | Only what is needed to use the mod. No build steps, no internals. |
| **The game's developers** | [`for-the-developers/`](for-the-developers) | Clinical. Phrased in terms of *their* source, self-contained, short. |
| **Other modders** | [`knowledge-base/`](knowledge-base), each mod's `spec.md` and `analysis/` | Clinical. Facts, tables, addresses, APIs, measurements. Minimal prose. |
| **Claude** | `notes-for-claude/` — **local only, not in this repository** | Whatever works. Narrative, lessons, wrong turns, session notes. Not for human consumption, and so not published. |

The last row is the one that keeps the others clean: war stories, "this cost a session",
and reasoning-in-progress go there, so the human-facing documents can stay terse.

Anything that is none of those — logs, configs, save snapshots, build output — is
**local only** and never leaves the machine. It is listed in `.gitignore`, and
`package.py` refuses to ship it.

## What this is

**momomod** is a mod manager. A player installs it once (it adds one proxy DLL to the
game folder and changes nothing else), then downloads the mods they want; momomod loads
them when the game starts, and each can be switched on and off and configured. The
manager and its published mods are separate DLLs, so a player stores only what they use.

The launch line-up is the **bugfixes** mod and the **reward auto-picker**. Everything
else in the tree is the machinery behind them, or work not yet published.

## Layout

```
Cargo.toml                one workspace; `cargo test --release` covers every crate

  the shared layers
tkiw-runtime/             the Rust layer every mod depends on: symbol resolution,
                          RValue/instance access, calling the game's builtins, code
                          caves, byte patching, and the overlay drawing tool
tkiw-plugin/              the mod ABI -- the four C exports the manager loads a mod by
momomod-kit/              the modding framework on top of runtime+plugin: the Feature
                          contract, config, and the runner that probes/times/guards
                          features. A mod crate is its features plus a hand-off to this.

  the manager
momomod-manager/          the mod manager: the mfreadwrite.dll proxy loader, plugin_host,
                          the install/enable/configure Python (install.py, manage-mods.py,
                          configure.py), and internal developer-only features (diagnostics,
                          popup_stutter_fix), all hidden

  the mods (each a plugin DLL)
tkiw-bugfixes-plugin/     morale_fix + fortifications_cap                     [published]
tkiw-reward-picker-plugin/ the auto-picker as a plugin                         [published]
tkiw-reward-auto-picker/  the picker's core logic (rlib the plugin links)
tkiw-gameplay-plugin/     unit-stats-on-hover overlay          [built, not yet published]
tkiw-morale-fix/          the standalone static byte-patch morale fix, and the pristine
                          .exe every analysis reads (not a workspace member)

  knowledge and parked work
knowledge-base/           how the game works, and the tools to find out more
  tools/                  Python: disassembly, symbol tables, proxy picking, playtesting
for-the-developers/       shareable upstream write-ups
quarantine/               features parked out of the build (fast_boot, font_atlases)
```

### Where new work goes

**A change players see should be a feature in a mod's plugin crate**, built on the
[`momomod-kit`](momomod-kit) `Feature` contract: it gets the game resolved for it, the
crash reporter, per-feature dependency checks, panic isolation and a frame budget for
free. A feature declares what it depends on as data, so a game update disables *that one
feature* rather than the mod. Group related features into one plugin (as `bugfixes`
does); reach for a new plugin crate when the mod is a distinct shippable thing.

**A developer-only tool** (a probe, a profiler, a diagnostic) is a `hidden` feature of
[`momomod-manager`](momomod-manager) instead, so a player never meets it.

The standalone morale fix is the exception that proves the rule: a static patch to the
executable on disk is a genuinely different lifetime from an in-memory feature, so it is
its own thing outside the manager.

## Conventions worth keeping

**Depend on `tkiw-runtime`; do not copy it.** The things that go wrong in that layer —
an ASLR miscalculation, a region cache slower than what it caches, a crash reporter that
allocates, a stale module list that attributes a third of a profile to nothing — each
cost a session to find. They should be fixed once.

**Measure before optimising, and write down what you measured.** Both speedups in this
repository were found by profiling and would not have been guessed: one looked like disk
I/O and was CPU, the other looked like unit AI and was text rendering. Two of the
loudest complaints turned out not to be the game's fault at all.

**Record the wrong turns too.** The `analysis/` documents (and the unpublished
`notes-for-claude/`) keep conclusions that were later contradicted, with what
disproved them. A wrong note that nobody knows is wrong is worse than no note.

**Every baked address carries a byte signature**, and a mismatch disables the thing that
depends on it rather than calling into whatever now lives there. A mod that checks only
names looks healthy on a new game build and then misbehaves silently on someone else's
machine.

## Building

```bash
cargo build --release       from here
cargo test --release        142 tests; game-dependent ones skip if the game is absent
```

Rust, MSVC toolchain, **standard library only** — no third-party crates, so it builds
offline with nothing but rustup, and a mod that ships to strangers has no supply chain.

`momomod-manager/package.py` builds the manager zip and refuses to ship a DLL that
carries a stamped install path (which would leak the builder's folder).

## Releasing

A release is four assets on one GitHub release: the manager zip
(`momomod-<version>.zip`), `catalog.json` (the mods the manager offers), and one
`<mod>.dll` per published mod. The manager fetches all of them from the release's
`latest/download/` URL, so the repository must be **public** for players to reach them.

```bash
python stage-release.py            build and stage all four into dist/release/
python stage-release.py --serve    also serve them locally, to run the whole
                                   download-and-install flow before publishing
```

Point the manager at the local server with `MOMOMOD_MODS_BASE=http://localhost:<port>`
to test exactly what a player will do, without touching GitHub.
