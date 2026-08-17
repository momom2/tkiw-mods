# momomod — a mod manager for The King is Watching

momomod installs, loads and manages mods for *The King is Watching*. On its own
it changes nothing about the game: it is a manager. You install it once, then
download the mods you want; it loads them when the game starts, and each mod can
be switched on and off and configured to taste.

Ask momom2 on Hypnohead's Discord server for support if needed.

## Quickstart

Needs Python 3.8+ (for the installer and the two windows). The mods themselves
have no dependencies.

**1. Install the manager.** One file is added to the game folder; the game's own
files are never touched.

```bash
python install.py
```

**2. Get the mods you want.** This window lists the mods momomod supports and
downloads the ones you choose into a `mods/` folder beside this script.

```bash
python manage-mods.py
```

**3. Enable and tune them.** Installed mods start switched off — nothing changes
the game until you say so. This window turns them on and adjusts their settings.

```bash
python configure.py
```

Installed mods load the next time the game starts. That is the whole loop:
**install momomod → download mods → configure them.**

## The mods

**Reward auto-picker** — picks reward choices for you, by rules you set. It
writes its own config from the live game's option lists, so you edit which
resources, spells, units and upgrades it wants, how many rerolls it may spend,
and it presses the buttons for you. Press **Ctrl+Alt+P** in game to toggle it on
and off; the toggle is saved, so what you chose is what the next launch does.

**Bug fixes** — restores behaviour the game describes but does not do. Each fix
is switchable on its own:

| fix | what it does | default |
|---|---|---|
| morale fix | **Resuming a reign keeps your morale.** Without it morale drops to 0 on load and creeps back over minutes, with King effects reading the crept value. | on |
| fortifications cap | Makes **Fortifications stop at the max castle HP the game says they grant** — as shipped it never stops, so a Brick Factory raises your castle's maximum forever. Caps growth from when you switch it on; HP already gained is left alone. | off |

More mods may be offered over time; the manager fetches the current list each
time you open it, so new ones appear without updating the manager.

## How it works

A mod is a small DLL in the `mods/` folder. Installing a mod is downloading its
DLL; uninstalling is deleting it — both from the mod-manager window. The manager
loads whatever it finds in `mods/` when the game starts, so **changes take effect
on the next launch.**

**A freshly installed mod is switched off** — installing changes nothing about
the game until you enable it. The mod-manager offers two ways in: *Install*
(dormant) and *Install & enable* (on, with sensible defaults). You can also enable
and tune a mod any time in the config window.

**Each mod has exactly one config file**, its own, beside this script
(`bugfixes.ini`, `reward-picker.ini`) — that file is the single place a mod's
settings live, and `configure.py` edits it. The manager's own `momomod.ini` does
**not** hold any mod's settings; it is just the manager, and a player has no
reason to touch it. Each mod also writes its own log (`bugfixes.log`,
`picker.log`).

The manager patches nothing on disk and never modifies the game's executable, so
nothing it does can be undone by Steam's integrity check. A mod that patches the
game does so only in memory, and only in a window where the game is provably not
running that code.

## Game updates

Each mod checks what it depends on — functions and variables it looks up by name,
and any fixed addresses with the bytes that must be there — when it starts. So
when the game updates, **only the parts that actually broke switch off**, and the
log names the specific thing that moved rather than failing silently. Everything
else keeps working. If something a whole mod is built on has moved, that mod
stands down completely and says which check failed.

## When something goes wrong

Each mod writes a log beside this script (`<mod>.log`), and the manager writes
`momomod.log`.

```bash
grep "mods:" momomod.log         which mods loaded, and any that did not
grep DISABLED *.log              why a mod or a fix stood down
```

A mod that panics or starts costing you frames is switched off for the session
and named in its log; the rest carry on. If a session ends badly, the next
launch stays passive so a bad build cannot break your game twice; a minute of
trouble-free play stands that guard down again.

Your save directory is copied into `save-backups/` at every launch, keeping the
last ten — even on launches where the manager disables itself.

## Uninstalling

```bash
python manage-mods.py          remove individual mods (delete their DLLs)
python uninstall.py            remove the manager; the game folder is left as it was
python uninstall.py --purge    also delete the config, logs and save snapshots
```

Installing adds exactly one file to the game folder. A Steam integrity check may
delete that file; re-run `install.py`.

## Build

The manager and each mod are separate crates in one workspace:

```bash
cargo build --release          from the repository root, builds them all
cargo test --workspace         the tests; game-dependent ones skip if the game is absent
python package.py              the manager's release zip
python manage-mods.py          for developers, set MOMOMOD_MODS_BASE to a local
                               server or release to test the download flow
```

Rust with the MSVC toolchain, standard library only — no third-party crates, so
it builds offline. A mod author builds against the `momomod-kit` crate (the
`Feature` trait, the config system, the drawing tool) and ships a small DLL the
manager loads; the shared game-reading layer is `tkiw-runtime`.

The release zip carries the manager DLL and the scripts, and no stamped path:
`package.py` refuses to build if the DLL already has one, because a stamped DLL
would leak the builder's folder to everyone who downloaded it.

## Clanker disclaimer

This project was realized by Claude, long may it code.
Don't use it if you refuse to interact with AI-generated code.
