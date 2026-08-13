#!/usr/bin/env python3
"""
List and restore save snapshots taken by the auto reward picker.

The mod snapshots the save directory into `save-backups/` on every launch,
before it does anything. This puts one back.

Usage:
  python restore-saves.py                 list the snapshots
  python restore-saves.py <name>          restore that snapshot
  python restore-saves.py --latest        restore the most recent one

Restoring first sets the current saves aside as a fresh snapshot, so a restore
is itself undoable.
"""
import os
import shutil
import sys
import time

MOD_DIR = os.path.dirname(os.path.abspath(__file__))
BACKUPS = os.path.join(MOD_DIR, "save-backups")


def save_dir():
    local = os.environ.get("LOCALAPPDATA")
    if not local:
        sys.exit("error: LOCALAPPDATA is not set")
    p = os.path.join(local, "The_king_is_watching_steam", "Release")
    if not os.path.isdir(p):
        sys.exit(f"error: no save directory at {p}")
    return p


def snapshots():
    if not os.path.isdir(BACKUPS):
        return []
    out = []
    for name in sorted(os.listdir(BACKUPS)):
        p = os.path.join(BACKUPS, name)
        if os.path.isdir(p):
            files = [f for f in os.listdir(p) if os.path.isfile(os.path.join(p, f))]
            out.append((name, p, files))
    return out


def describe(name, path, files):
    newest = max((os.path.getmtime(os.path.join(path, f)) for f in files), default=0)
    when = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(newest)) if newest else "?"
    size = sum(os.path.getsize(os.path.join(path, f)) for f in files)
    run = "run_data.dat" in files
    return (f"  {name:<24} {len(files):>2} files  {size / 1024:>7.0f} KB  "
            f"saved {when}{'' if run else '   (no run_data.dat)'}")


def restore(entry):
    name, path, files = entry
    dst = save_dir()

    # set the current saves aside first, so this is reversible too
    stamp = time.strftime("%Y%m%d-%H%M%S")
    aside = os.path.join(BACKUPS, f"{stamp}-before-restore")
    os.makedirs(aside, exist_ok=True)
    kept = 0
    for f in os.listdir(dst):
        src = os.path.join(dst, f)
        if os.path.isfile(src):
            shutil.copy2(src, os.path.join(aside, f))
            kept += 1
    print(f"current saves set aside: {aside} ({kept} files)")

    for f in files:
        shutil.copy2(os.path.join(path, f), os.path.join(dst, f))
    print(f"restored {len(files)} files from {name} -> {dst}")
    print("if the game is running, quit without saving or it will overwrite this.")


def main():
    args = sys.argv[1:]
    snaps = snapshots()
    if not snaps:
        print(f"no snapshots in {BACKUPS}")
        print("the mod takes one on every launch; run the game once with it installed.")
        return 0

    if not args:
        print(f"snapshots in {BACKUPS}:\n")
        for s in snaps:
            print(describe(*s))
        print("\nrestore with:  python restore-saves.py <name>")
        print("           or:  python restore-saves.py --latest")
        return 0

    if args[0] == "--latest":
        chosen = snaps[-1]
    else:
        matches = [s for s in snaps if s[0] == args[0]]
        if not matches:
            print(f"no snapshot named {args[0]!r}. available:\n")
            for s in snaps:
                print(describe(*s))
            return 1
        chosen = matches[0]

    print(f"about to restore: {chosen[0]}")
    print(describe(*chosen))
    reply = input("proceed? [y/N] ").strip().lower()
    if reply != "y":
        print("nothing was changed.")
        return 1
    restore(chosen)
    return 0


if __name__ == "__main__":
    sys.exit(main())
