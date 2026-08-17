#!/usr/bin/env python3
"""
Install TKIW's momomod Kit for "The King is Watching".

Adds exactly one file to the game folder: `mfreadwrite.dll`, a proxy that forwards
every export to the real one in System32. Everything the kit owns -- config, log,
save snapshots -- lives here, in the kit's own folder, so the game folder stays
clean and uninstalling is deleting that one file. No game file is modified, so
nothing here can be undone by Steam's integrity check.

The DLL cannot work out where this folder is on its own, so its absolute path is
stamped into the DLL as it is copied. That means: **if you move this folder,
re-run this script.**

The reward auto-picker uses the `version.dll` slot, so the two can be installed at
the same time without interacting.

Usage:  python install.py [path to game folder or .exe]
"""
import os
import sys

MOD_DIR = os.path.dirname(os.path.abspath(__file__))
EXE_NAME = "The King is Watching.exe"
TARGET_DLL = "mfreadwrite.dll"
MARKER = b"TKIW_MOMOMOD_DIR="
STAMP_LEN = 544

BUILT_DLL_CANDIDATES = [
    os.path.join(MOD_DIR, "..", "target", "release", "momomod_manager.dll"),
    os.path.join(MOD_DIR, "target", "release", "momomod_manager.dll"),
    os.path.join(MOD_DIR, "dist", "momomod_manager.dll"),
]

BUILD_HINT = ("       build it first, from the repository root:\n"
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
    """(offset, capacity) of the reserved path buffer, or exit if it is not sane.

    The marker string appears **twice** in the built DLL: once at the head of the
    reserved buffer, and once as the constant the DLL compares against to prove a
    stamp is its own. Only the first is writable space, so the two are told apart
    by what follows -- the buffer is padded to its full length with NULs, and the
    comparison constant is followed by whatever the linker put next.

    Requiring the whole reservation to be zero is also a check that we are looking
    at a freshly built DLL rather than an already-stamped one: this only ever runs
    against `target/release`, and stamping a stamped file would append a path to a
    path.
    """
    cap = STAMP_LEN - len(MARKER)
    hits, start = [], 0
    while True:
        i = data.find(MARKER, start)
        if i < 0:
            break
        hits.append(i)
        start = i + 1
    if not hits:
        sys.exit("error: the built DLL has no mod-folder stamp marker.\n"
                 "       it was not built from this source tree; rebuild it.")

    buffers = [i for i in hits if data[i + len(MARKER):i + len(MARKER) + cap] == b"\0" * cap]
    if not buffers:
        sys.exit(f"error: found the stamp marker {len(hits)} time(s) in the DLL, but none of\n"
                 f"       them is followed by {cap} free bytes.\n"
                 "       either the DLL is already stamped, or the reservation in lib.rs\n"
                 "       has changed and this script needs updating to match.")
    if len(buffers) > 1:
        sys.exit(f"error: {len(buffers)} stamp buffers in the DLL; refusing to guess which\n"
                 "       one the mod reads.")
    return buffers[0] + len(MARKER), cap


def stamp(data, mod_dir):
    off, cap = find_stamp(data)
    encoded = mod_dir.encode("utf-8")
    if len(encoded) + 1 > cap:
        sys.exit(f"error: the kit folder path is {len(encoded)} bytes, which does not fit "
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


# ------------------------------------------------------------------- sanity checks
def check_real_dll_exists():
    """We forward to the real DLL, so it had better be there.

    On a Windows N/KN edition without the Media Feature Pack it is not -- but the
    game statically imports it, so such a machine cannot run the game with or
    without us. Worth saying plainly rather than letting the player discover it as
    a mysteriously silent kit.
    """
    real = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", TARGET_DLL)
    if not os.path.isfile(real):
        sys.exit(f"error: {real} does not exist on this machine.\n"
                 "       that is Media Foundation missing (a Windows N/KN edition without\n"
                 "       the Media Feature Pack). The game imports it too, so it will not\n"
                 "       launch here either way. Nothing was changed.")


def warn_about_neighbours(game):
    """Say what else is installed, since a player debugging their game needs to
    know which mods are in play."""
    others = {
        "version.dll": "the reward auto-picker (or another mod using that slot)",
    }
    for name, what in others.items():
        if os.path.isfile(os.path.join(game, name)):
            print(f"note       : '{name}' is also present - {what}.")
            print("             both load independently; neither disturbs the other.")


# ---------------------------------------------------------------------------- main
def main():
    game = find_game(sys.argv[1] if len(sys.argv) > 1 else None)
    print(f"game folder: {game}")
    print(f"kit folder : {MOD_DIR}")

    check_real_dll_exists()

    built = next((p for p in BUILT_DLL_CANDIDATES if os.path.isfile(p)), None)
    if built is None:
        sys.exit("error: the built DLL was not found. Looked in:\n"
                 + "".join(f"           {os.path.normpath(p)}\n" for p in BUILT_DLL_CANDIDATES)
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
    warn_about_neighbours(game)
    print()
    print("momomod is installed, but on its own it does nothing: it is a manager,")
    print("and the mods are downloaded separately. Two windows do the rest:")
    print()
    print("    python manage-mods.py   choose which mods to install (download them)")
    print("    python configure.py     enable and tune the mods you have installed")
    print()
    print("Installed mods live as DLLs in the 'mods' folder here, and load the next")
    print("time the game starts. Anything that changes the rules starts switched")
    print("off, so a freshly installed mod changes nothing until you say so. What")
    print("momomod did is in momomod.log beside this script.")
    print()
    print("If you move this folder, re-run this script.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
