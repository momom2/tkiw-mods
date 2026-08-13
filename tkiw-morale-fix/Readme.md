# The King is Watching — resume morale fix

Fixes the bug where resuming a reign drops your effective morale to 0 and makes it crawl back up over many minutes; with this patch it is restored instantly.

## Requirements

- Python 3.8+ on your `PATH`
- `patch.py` and `unpatch.py` kept in the same folder (`unpatch.py` imports from `patch.py`)
- The game closed, and Steam not mid-update

You can run them from anywhere — either as `python patch.py` from inside this folder, or by full path from any directory. They locate the game themselves; the current working directory doesn't matter.

## Install

```sh
python patch.py
```

The game is found automatically in your Steam libraries. If it isn't, pass the game folder as an argument:

```sh
python patch.py "C:\Program Files (x86)\Steam\steamapps\common\The King is Watching"
```

A backup of the original executable is written **into this folder** as `The King is Watching.exe.orig`, not into the game folder. That keeps the game folder free of any trace of this mod, and stops two mods that both back up the executable from shadowing each other's backup. The backup travels with the `unpatch.py` that knows how to use it, so keep them together — if you move this folder, move all of it.

## Uninstall

```sh
python unpatch.py --purge
```

This restores the original executable byte-for-byte and deletes the backup; afterwards this folder can be deleted. Drop `--purge` to keep the backup. It takes the same optional game-folder argument.

Uninstalling does not actually need the backup — it reverses the patch by editing the executable back — so a lost backup is not a disaster. If a backup left in the game folder by an older version of this patch is still there, `unpatch.py` finds and removes that too.

To move to a newer version of the fix, uninstall first, then install again.

## Notes

- **It only patches the exact game build it was written for.** Every address moves when the game updates, so the patcher checks the bytes at both hook sites first and refuses to touch anything if they don't match. Running it against the wrong version is safe — it just declines and changes nothing.
- Running `patch.py` twice is harmless; it detects an existing patch and tells you to uninstall first.
- A game update, or Steam's "Verify integrity of game files", will revert the patch. Re-run `patch.py` if the build still matches.
- Only morale *on resume* is affected. Morale gained during normal play still animates exactly as it does in the unpatched game.
- On Windows run it from PowerShell or `cmd`, with `python` on your `PATH` (the Microsoft Store build of Python works fine).

## What it actually does

The game holds two values: `morale_current` — the one every King effect reads — and `morale_target`, the true total. Loading a save restores `morale_target` correctly but leaves `morale_current` at 0, and it then creeps toward the target at 0.15/frame scaled by the game's time scale, which is 0 whenever gameplay is frozen (shops, blueprint picks, reward screens). That's why it can take so long to recover.

The patch appends a small section to the executable and hooks two places:

1. the run controller's setup routine, just after the saved stat modifiers are applied — it sets a one-shot flag, but only when the game's own `PLAYER_CONTINUED_RUN` flag is true, so starting a fresh run never arms it;
2. the point in the gameplay controller's step where the smoothed morale value is written back — if the flag is set, it clears it and writes `morale_target` straight into `morale_current` instead of the interpolated value.

The assignment deliberately bypasses the game's `approach()` helper rather than just speeding it up, because `approach()` scales every step by the frame delta and time scale — both of which are zero while gameplay is frozen, which is exactly the situation during a load. Every other frame, and every fresh run, behaves identically to the unpatched game.
