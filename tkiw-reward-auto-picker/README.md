# tkiw-reward-auto-picker

> **This mod is now part of TKIW's momomod Kit.**
>
> The picking logic lives on here and is linked into the kit, which supplies the DLL,
> the frame hook and the log. The standalone `version.dll` is no longer built: install
> the kit instead, switch on the `reward-picker` mod, and put your rules in
> `config/reward-picker.ini` — the same file this one calls `config.ini`, so copying it
> across keeps every setting.
>
> An existing standalone install keeps working and takes priority; the kit's copy
> detects it and stands down. `uninstall.py` here still removes it.


Empties your reward queue in *The King is Watching* the way you would have, from
a configurable list of preferences.

Built for the game as of **2026-08-10**; it refuses to run on any other build.
Ask momom2 on Hypnohead's Discord server for support if needed.

## Quickstart

Needs Python 3.8+ to install. The mod itself has no dependencies.

```
python install.py
```

Launch the game once. The mod writes `config.ini` in this folder, listing every
reward type and every option the game can offer you. **It does nothing until you
edit that file** — everything starts blacklisted.

Open `config.ini`, move the ids you want into a `wanted` section, then press
**Ctrl+Alt+P** in game to activate. That's it.

Press Ctrl+Alt+P in game again to deactivate.

---

## Configuring

`config.ini` has one section per reward type, each with three tiers and a set of
reroll budgets. Edit it while the game runs; it is re-read as you save.

```ini
[unit_class_stat]
voodoo_depth = 1        # rerolls using at most one per-reward freebie (Voodoo Beads)
free_depth   = -1       # -1: as many as the game gives
paid_depth   = 0        # refuse rerolls paid for in denarii
denarii_floor = 500     # never spend below this balance

[unit_class_stat.wanted]
ranged.damage = 10      # highest value -> take on sight
arcane.damage = 10      # equal weight: no preference between the two (will pick at random)

[unit_class_stat.fallback]
ranged.hp = 5           # settle for this once the reroll budget is spent
_scrap    = 1           # or scrap the reward for 5 denarii

[unit_class_stat.blacklist]
grunt.hp                # never take
```

### The three tiers

| tier | meaning |
|---|---|
| `wanted` | take on sight |
| `fallback` | take only once the reroll budget is spent |
| `blacklist` | never take, at any depth |

A wanted option always beats a fallback one whatever the numbers say. Weights
order options **within** a tier only, highest first; equal weights mean you are
indifferent and the mod picks between them at random.

Every id must appear in exactly one tier. The generated file lists them all, so
you are moving lines rather than typing ids from memory. An option written with
no number is taken at weight 0, and the mod writes the `= 0` back so the file
says what it will do.

`_scrap` is a rankable option like any other, wherever the reward can be
scrapped.

If no desirable option appears (e.g. all blacklisted) and the reroll budget is
spent, the mod does nothing more (lets the user pick manually).

### Reroll budgets

Four settings per type, counted per reward:

| setting | what it spends |
|---|---|
| `voodoo_depth` | the per-reward freebie (Voodoo Beads) |
| `free_depth` | the reign-wide free pool |
| `paid_depth` | denarii |
| `denarii_floor` | a balance the mod will not spend below |

They never fall through to one another: if `free_depth` is spent and free
rerolls remain banked, the mod stops rather than starting to pay.

`-1` on any depth means **as many as the game allows** — which is not unlimited.
Free rerolls run out, paid ones stop at what you can afford and at
`denarii_floor`, and a reroll the game will not offer is not one the mod can
take.

Rerolls it spends are really spent. If it burns three and then hands the choice
back to you, those three are gone.

## Ways to use it

**Switch it on and off mid-game.** Ctrl+Alt+P toggles pressing without leaving
the game. `[global] act = true` starts it switched on.

**Automate some types and not others.** Delete a whole `[section]` and that
reward type is left entirely to you. Any type not mentioned is manual.

**Stop it at anything surprising.** If it meets an option your config does not
classify, it switches itself off and logs why rather than guessing. Ctrl+Alt+P
resumes once you have fixed the config.

**Monitor what it does.** Check the logs, everything that it does (if activated),
or would do (if not) is shown there. You can use the logs to test a config without
activating it.

## What it will not do

- Touch anything but the front of the queue. If the front is a type with no
  config section, it stops there.
- Resolve `shop`, `prophecy`, `rewards_wheel` or `onaraks_favour`: those are not
  card choices. Or `run_start_bonus`, whose cards carry no identifier.
- Reroll past the budget you set, spend below `denarii_floor`, or spend denarii
  whose cost it cannot read.
- Draw on the game's random number generator. Ties are broken with its own, so a
  run does not diverge from what it would otherwise have been.

## Log

`picker.log`, in this folder. It is the only record of what the mod did.

```
grep '\[PICK\]' picker.log      what it chose
grep 'config:' picker.log       config problems
```

If the game faults, `crash.log` gets the address, the offset into the game, and
the phase that was under way.

Two settings help with a bug report, both off by default:

- `[global] survey = true` — a periodic sweep over the whole reward UI. This is
  what a report should carry. It is also the most expensive thing the mod does;
  picking, rerolling and opening the queue do not use it.
- `[global] trace = true` — logs each phase as it begins. Verbose, and not
  needed for crash reports.

If a session ends badly, the next launch does not probe the game at all — the
mod stays completely passive so a bad build cannot break your game twice. The
log says so on the first line, and Ctrl+Alt+P overrides it. Once a session has
run a minute without trouble the guard stands down by itself, so an ordinary
mid-session crash costs you nothing.

## Saves

The save directory is copied into `save-backups/` at every launch, keeping the
last ten.

```
python restore-saves.py             list them
python restore-saves.py --latest    put the most recent one back
```

A restore sets the current saves aside first, so it is itself reversible.

## Uninstalling

```
python uninstall.py            remove it; the game folder is left as it was
python uninstall.py --purge    also delete the config and log
```

Installing adds exactly one file to the game folder, `version.dll`, and never
touches the executable — so nothing here can be undone by a Steam integrity
check. A check may delete `version.dll` itself; re-run `install.py`.

## Game updates

The mod resolves most of what it needs by name, which survives code moving. A
dozen addresses have no name and are fixed at build time. Each carries a byte
signature of the function it should be; on any mismatch the mod **disables
itself and logs which check failed**, rather than calling into whatever now
lives there. So after a game update it stops working rather than misbehaving.

Option lists do not need rebuilding: they are read from your installed game each
time a config is written, so a build that adds an artifact will list it. An
existing `config.ini` is never overwritten — instead the mod writes
`config.reference.ini` beside it, the same lists freshly generated, so you can
diff the two to find options your file has no line for. Delete it when you are
done; it is written again only if absent.

Rebuilding for a new game version means re-running the analysis in `analysis/`
and updating the signatures.

## Build

```
cargo build --release        from the repository root
cargo test --release         48 tests here, 75 across the workspace
python package.py            build the zip that ships
python package.py --list     what ships, what stays, what never leaves
```

Rust, MSVC toolchain, no third-party crates, builds offline.

**This mod now shares its runtime layer with the rest of the repository.** Injection,
symbol discovery, safe memory access, the frame hook, the log, the crash reporter, save
snapshots and the build guard live in [`../tkiw-runtime`](../tkiw-runtime); fourteen
files and 2,273 lines that used to be duplicated here are no longer compiled.

Nothing about the mod's behaviour changed — same proxy slot, same stamp marker, same
`config.ini`, same `picker.log` — so an existing installation keeps working and
`install.py` is untouched. Verified in a live launch after the migration: symbols
resolved, all nine baked addresses verified, `pending_rewards` read back.

The benefit is that a fix to that layer lands once. The nine addresses this mod used to
carry are now `tkiw_runtime::guard::CORE`, so a game update is re-derived in one place
for every mod at once.

Its old copies of those fourteen files are still in `src/` and are dead code; they are
kept only so the migration is trivially reversible, and should be deleted.

The shipped zip is the DLL and the three scripts. It carries no config, no log,
and no path: `install.py` stamps the player's own folder into its copy of the
DLL as it installs, and `package.py` refuses to build if the DLL it is given
already has one.

## Known gaps

- **The crash after long runs is not fixed.** It is rare and always follows a
  pick closely. Each attempt has closed off a real defect without ending it. If
  it happens to you, please send `crash.log` and the tail of `picker.log` to
  momom2 for investigation.

## Clanker disclaimer

This mod was realized by Claude Opus 5, long may it code.
Don't use this mod if you refuse to interact with AI-generated code.