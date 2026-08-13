#!/usr/bin/env python3
"""
Uninstall the auto reward picker for "The King is Watching".

Removes the one file the installer added to the game folder, after checking it
is ours. Never deletes a `version.dll` this mod did not place.

Usage:  python uninstall.py [--purge] [path to game folder or .exe]

  --purge   also delete config.ini and picker.log from this folder, so that
            afterwards the folder can simply be deleted
"""
import os
import sys

from install import (MOD_DIR, TARGET_DLL, find_game, is_ours, read_stamped_dir)

OWNED_FILES = ["config.ini", "picker.log", "picker.log.prev"]


def main():
    args = [a for a in sys.argv[1:] if a != "--purge"]
    purge = "--purge" in sys.argv[1:]

    game = find_game(args[0] if args else None)
    print(f"game folder: {game}")

    target = os.path.join(game, TARGET_DLL)
    if not os.path.exists(target):
        print(f"no '{TARGET_DLL}' in the game folder - nothing to remove there.")
    elif not is_ours(target):
        sys.exit(f"error: the '{TARGET_DLL}' in the game folder is not ours.\n"
                 "       it belongs to something else; refusing to delete it.")
    else:
        stamped = read_stamped_dir(target)
        if stamped and os.path.normcase(os.path.normpath(stamped)) != \
                os.path.normcase(os.path.normpath(MOD_DIR)):
            print(f"note: it was installed from {stamped}, not this folder.")
            print("      removing it anyway - it is still ours.")
        try:
            os.remove(target)
        except PermissionError:
            sys.exit(f"error: cannot delete {target}\n"
                     "       close the game and Steam and try again.")
        print(f"removed    : {target}")
        print("the game folder is now exactly as it was.")

    if purge:
        for name in OWNED_FILES:
            p = os.path.join(MOD_DIR, name)
            if os.path.exists(p):
                os.remove(p)
                print(f"removed    : {p}")
        print("this folder can now be deleted.")
    else:
        cfg = os.path.join(MOD_DIR, "config.ini")
        if os.path.exists(cfg):
            print(f"kept       : {cfg}")
            print("             (use --purge to remove it too)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
