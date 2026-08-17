#!/usr/bin/env python3
"""
Uninstall TKIW's momomod Kit.

Removes the one file it added to the game folder. The executable was never
touched, so afterwards the game folder is exactly as it was.

    python uninstall.py            remove it; the config and log survive
    python uninstall.py --purge    also delete the config, log and snapshots,
                                   so this folder can then simply be deleted

Running it twice is harmless. It takes the same optional game-folder argument as
install.py.
"""
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from install import MOD_DIR, TARGET_DLL, find_game, is_ours  # noqa: E402

OWNED = ["momomod.ini", "momomod.reference.ini", "momomod.log", "momomod.log.prev",
         "crash.log", "probe.incomplete"]
OWNED_DIRS = ["save-backups"]


def main():
    args = [a for a in sys.argv[1:] if a != "--purge"]
    purge = "--purge" in sys.argv[1:]

    game = find_game(args[0] if args else None)
    target = os.path.join(game, TARGET_DLL)

    if not os.path.exists(target):
        print(f"nothing to remove: no '{TARGET_DLL}' in {game}")
    elif not is_ours(target):
        # Never delete a file we did not place. A `mfreadwrite.dll` without our
        # stamp belongs to something else, and removing it would break it.
        sys.exit(f"error: the '{TARGET_DLL}' in the game folder is not ours "
                 "(no stamp marker).\n       leaving it alone.")
    else:
        try:
            os.remove(target)
        except PermissionError:
            sys.exit(f"error: cannot remove {target}\n"
                     "       close the game and Steam and try again.")
        print(f"removed    : {target}")

    if not purge:
        print(f"kept       : the config and log in {MOD_DIR}")
        print("             (re-installing restores your settings; --purge deletes them)")
        return 0

    for name in OWNED:
        path = os.path.join(MOD_DIR, name)
        if os.path.isfile(path):
            os.remove(path)
            print(f"deleted    : {name}")
    for name in OWNED_DIRS:
        path = os.path.join(MOD_DIR, name)
        if os.path.isdir(path):
            shutil.rmtree(path, ignore_errors=True)
            print(f"deleted    : {name}{os.sep}")
    print()
    print("This folder now holds only the kit's own files; it can be deleted.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
