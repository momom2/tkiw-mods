#!/usr/bin/env python3
"""
Install the auto reward picker for "The King is Watching".

Adds exactly one file to the game folder: `version.dll`. Everything the mod
owns -- config, log -- lives here, in the mod's own folder, so the game folder
stays clean and uninstalling is deleting that one file.

The DLL cannot work out where this folder is on its own, so its absolute path
is stamped into the DLL as it is copied. That means: **if you move this folder,
re-run this script.**

Usage:  python install.py [path to game folder or .exe]
"""
import os
import shutil
import struct
import sys

MOD_DIR = os.path.dirname(os.path.abspath(__file__))
EXE_NAME = "The King is Watching.exe"
TARGET_DLL = "version.dll"
MARKER = b"TKIW_PICKER_MOD_DIR="
STAMP_LEN = 544

BUILT_DLL_CANDIDATES = [
    os.path.join(MOD_DIR, "target", "release", "tkiw_reward_picker.dll"),
    os.path.join(MOD_DIR, "dist", "tkiw_reward_picker.dll"),
]

BUILD_HINT = ("       build it first:\n"
              "           cargo build --release\n"
              "       (needs the Rust MSVC toolchain; no crates are downloaded)")


# ---------------------------------------------------------------- locating the game
def steam_libraries():
    roots = []
    if os.name == "nt":
        roots += [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"]
    home = os.path.expanduser("~")
    roots += [
        os.path.join(home, ".steam", "steam"),
        os.path.join(home, ".local", "share", "Steam"),
        os.path.join(home, "Library", "Application Support", "Steam"),
    ]
    libs = []
    for root in roots:
        if not os.path.isdir(root):
            continue
        libs.append(root)
        try:
            with open(os.path.join(root, "steamapps", "libraryfolders.vdf"),
                      encoding="utf-8", errors="replace") as fh:
                for line in fh:
                    parts = line.split('"')
                    if len(parts) >= 5 and parts[1] == "path":
                        libs.append(parts[3].replace("\\\\", "\\"))
        except OSError:
            pass
    return libs


def find_game(arg=None):
    """The game folder."""
    if arg:
        if os.path.isfile(arg) and arg.lower().endswith(".exe"):
            return os.path.dirname(os.path.abspath(arg))
        if os.path.isdir(arg):
            if os.path.isfile(os.path.join(arg, EXE_NAME)):
                return os.path.abspath(arg)
            sys.exit(f"error: no '{EXE_NAME}' inside {arg}")
        sys.exit(f"error: no such file or folder: {arg}")
    for lib in steam_libraries():
        cand = os.path.join(lib, "steamapps", "common", "The King is Watching")
        if os.path.isfile(os.path.join(cand, EXE_NAME)):
            return cand
    sys.exit("error: could not find the game automatically.\n"
             "       pass the game folder, e.g.:\n"
             '       python install.py "C:\\Program Files (x86)\\Steam\\steamapps\\'
             'common\\The King is Watching"')


# ------------------------------------------------------------------------ stamping
def find_stamp(data):
    """(offset, capacity) of the path buffer, or exit if it is not sane."""
    hits = []
    start = 0
    while True:
        i = data.find(MARKER, start)
        if i < 0:
            break
        hits.append(i)
        start = i + 1
    if not hits:
        sys.exit("error: the built DLL has no mod-folder stamp marker.\n"
                 "       it was not built from this source tree; rebuild it.")
    if len(hits) > 1:
        sys.exit(f"error: the stamp marker appears {len(hits)} times in the DLL; "
                 "refusing to guess which one to write.")
    off = hits[0] + len(MARKER)
    return off, STAMP_LEN - len(MARKER)


def stamp(data, mod_dir):
    off, cap = find_stamp(data)
    encoded = mod_dir.encode("utf-8")
    if len(encoded) + 1 > cap:
        sys.exit(f"error: the mod folder path is {len(encoded)} bytes, which does not fit "
                 f"in the {cap}-byte stamp.\n"
                 "       move this folder somewhere with a shorter path and try again.")
    out = bytearray(data)
    out[off:off + cap] = encoded + b"\0" * (cap - len(encoded))
    return bytes(out)


def is_ours(path):
    try:
        with open(path, "rb") as fh:
            return MARKER in fh.read()
    except OSError:
        return False


def read_stamped_dir(path):
    try:
        with open(path, "rb") as fh:
            data = fh.read()
    except OSError:
        return None
    i = data.find(MARKER)
    if i < 0:
        return None
    i += len(MARKER)
    end = data.find(b"\0", i)
    return data[i:end].decode("utf-8", "replace") if end > i else None


# ---------------------------------------------------------------------------- main
def main():
    game = find_game(sys.argv[1] if len(sys.argv) > 1 else None)
    print(f"game folder: {game}")
    print(f"mod folder : {MOD_DIR}")

    built = next((p for p in BUILT_DLL_CANDIDATES if os.path.isfile(p)), None)
    if built is None:
        sys.exit("error: the built DLL was not found. Looked in:\n"
                 + "".join(f"           {p}\n" for p in BUILT_DLL_CANDIDATES)
                 + BUILD_HINT)

    target = os.path.join(game, TARGET_DLL)
    if os.path.exists(target) and not is_ours(target):
        sys.exit(f"error: a '{TARGET_DLL}' is already in the game folder and it is not ours.\n"
                 "       another mod is probably using the same proxy slot. Replacing it\n"
                 "       would break that mod, so nothing was changed.")

    with open(built, "rb") as fh:
        data = fh.read()
    data = stamp(data, MOD_DIR)

    try:
        with open(target, "wb") as fh:
            fh.write(data)
    except PermissionError:
        sys.exit(f"error: cannot write {target}\n"
                 "       close the game and Steam and try again.\n"
                 "       (if the game is running, the DLL is locked.)")

    print(f"installed  : {target}")
    print()
    print(f"On first launch the mod writes an inert config to:")
    print(f"    {os.path.join(MOD_DIR, 'config.ini')}")
    print("Until you edit it, the mod changes nothing about the game.")
    print()
    print("If you move this folder, re-run this script.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
