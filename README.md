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
| **Claude** | [`notes-for-claude/`](notes-for-claude) | Whatever works. Narrative, lessons, wrong turns, session notes. Not for human consumption. |

The last row is the one that keeps the others clean: war stories, "this cost a session",
and reasoning-in-progress go there, so the human-facing documents can stay terse.

Anything that is none of those — logs, configs, save snapshots, build output — is
**local only** and never leaves the machine. It is listed in `.gitignore`, and
`package.py` refuses to ship it.

## Layout

```
Cargo.toml                one workspace; `cargo test --release` covers every mod
knowledge-base/           how the game works, and the tools to find out more
  tools/                  Python: disassembly, symbol tables, proxy picking, playtesting
for-the-developers/       shareable upstream write-ups
tkiw-runtime/             the shared Rust layer every mod depends on
tkiw-momomod-kit/         the multi-feature kit (start new work here)
tkiw-reward-auto-picker/  the reward picker
tkiw-morale-fix/          a static byte patch, and the pristine .exe every analysis reads
```

### Where new work goes

**A new change to the game should almost always be a feature of `tkiw-momomod-kit`, not
a new mod.** It gets the injection, the crash reporter, per-feature dependency checks,
panic isolation and a frame budget for free, and it costs the player one more line in
one config file rather than another DLL. See its
[`spec.md`](tkiw-momomod-kit/spec.md) for the feature contract.

A separate mod is justified only when it needs a different lifetime or a different
audience — the morale fix is a static patch to the executable, which is a genuinely
different thing.

## Conventions worth keeping

**Depend on `tkiw-runtime`; do not copy it.** The things that go wrong in that layer —
an ASLR miscalculation, a region cache slower than what it caches, a crash reporter that
allocates, a stale module list that attributes a third of a profile to nothing — each
cost a session to find. They should be fixed once.

**Measure before optimising, and write down what you measured.** Both speedups in this
repository were found by profiling and would not have been guessed: one looked like disk
I/O and was CPU, the other looked like unit AI and was text rendering. Two of the
loudest complaints turned out not to be the game's fault at all.

**Record the wrong turns too.** `notes-for-claude/pitfalls.md` and the `analysis/`
documents keep conclusions that were later contradicted, with what disproved them. A
wrong note that nobody knows is wrong is worse than no note.

**Every baked address carries a byte signature**, and a mismatch disables the thing that
depends on it rather than calling into whatever now lives there. A mod that checks only
names looks healthy on a new game build and then misbehaves silently on someone else's
machine.

## Building

```bash
cargo build --release       from here
cargo test --release        84 tests; game-dependent ones skip if the game is absent
```

Rust, MSVC toolchain, **standard library only** — no third-party crates, so it builds
offline with nothing but rustup, and a mod that ships to strangers has no supply chain.

Each mod ships with `python package.py`, which refuses to build a zip from a DLL that
carries a stamped install path.
