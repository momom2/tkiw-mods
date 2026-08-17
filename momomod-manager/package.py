#!/usr/bin/env python3
"""
Build the zip that ships.

    python package.py           build dist/momomod-<version>.zip
    python package.py --list    what ships, what stays, and why

What ships is deliberately small: the DLL, the two install scripts, and the readme.
No config, no log, no save snapshots, and above all **no stamped path** -- the DLL in
the zip must not carry anyone's folder in it. `install.py` stamps the player's own
folder as it installs.

The refusal at the end is the important part. A stamped DLL in a public zip leaks the
builder's username and directory layout to everyone who downloads it, and would also
send a player's mod looking for a folder that does not exist on their machine.
"""
import os
import re
import sys
import zipfile

KIT_DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(KIT_DIR)
DIST = os.path.join(KIT_DIR, "dist")
MARKER = b"TKIW_MOMOMOD_DIR="

DLL_CANDIDATES = [
    os.path.join(ROOT, "target", "release", "momomod_manager.dll"),
    os.path.join(KIT_DIR, "target", "release", "momomod_manager.dll"),
]

# (path relative to the kit folder, name inside the zip)
SHIPS = [
    ("install.py", "install.py"),
    ("uninstall.py", "uninstall.py"),
    # The mod manager: downloads and removes mods. And the config window:
    # enables and tunes the mods you have installed. The catalogue lists the
    # mods the manager offers; generated from the build so it cannot drift.
    ("manage-mods.py", "manage-mods.py"),
    ("configure.py", "configure.py"),
    ("catalog.json", "catalog.json"),
    ("README.md", "README.md"),
]

# Stated so the list is a decision record rather than whatever happened to be lying
# around when someone ran this.
STAYS = {
    "momomod.ini": "the player's own settings; generated on first launch",
    "momomod.reference.ini": "generated when needed; would only confuse",
    "momomod.log": "carries paths, usernames and the state of a real save",
    "crash.log": "same",
    "save-backups/": "someone's saves",
    "probe.incomplete": "session state, meaningless elsewhere",
    "spec.md": "for whoever works on the kit, not for players",
    "analysis/": "measurements and workings; interesting, not shippable",
    "src/": "source lives in the repository",
    "mods/": "the mods a player downloads; not bundled, that is the whole point",
}


def version():
    """From the workspace Cargo.toml, so the zip cannot disagree with the build."""
    try:
        with open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8") as fh:
            m = re.search(r'^version\s*=\s*"([^"]+)"', fh.read(), re.M)
            return m.group(1) if m else "0.0.0"
    except OSError:
        return "0.0.0"


def find_dll():
    for p in DLL_CANDIDATES:
        if os.path.isfile(p):
            return p
    sys.exit("error: no built DLL. Run `cargo build --release` from the repo root.\n"
             + "".join(f"       looked in {os.path.normpath(p)}\n" for p in DLL_CANDIDATES))


def classify(body, cap):
    """What the bytes after a marker occurrence are.

    The marker appears **twice** in the built DLL: once at the head of the reserved
    544-byte buffer, and once as the constant the DLL compares against to prove a
    stamp is its own. Telling them apart needs care -- a first version asked only
    "are the following bytes non-zero?", and the comparison constant, which sits in
    `.rdata` followed by unrelated string literals, therefore looked exactly like a
    stamped path. It rejected every clean build with a garbled quote from the middle
    of the string pool.

    A real buffer has a shape: `cap` bytes that are either all zero (unstamped), or a
    path followed by zeros all the way to the end (stamped). Anything else is not a
    buffer at all.
    """
    if len(body) < cap:
        return "not-a-buffer"
    if body == b"\0" * cap:
        return "unstamped"
    first_nul = body.find(b"\0")
    if first_nul <= 0:
        return "not-a-buffer"
    head, tail = body[:first_nul], body[first_nul:]
    if tail != b"\0" * len(tail):
        return "not-a-buffer"      # padding is not clean: string-pool neighbours
    try:
        text = head.decode("utf-8")
    except UnicodeDecodeError:
        return "not-a-buffer"
    if not all(0x20 <= ord(c) < 0x7F or ord(c) > 0x7F for c in text):
        return "not-a-buffer"
    return "stamped"


def check_unstamped(path):
    with open(path, "rb") as fh:
        data = fh.read()
    if MARKER not in data:
        sys.exit("error: the DLL has no stamp marker at all; it was not built from "
                 "this source tree.")

    cap = 544 - len(MARKER)
    i, unstamped, stamped = 0, 0, []
    while True:
        i = data.find(MARKER, i)
        if i < 0:
            break
        body = data[i + len(MARKER):i + len(MARKER) + cap]
        kind = classify(body, cap)
        if kind == "unstamped":
            unstamped += 1
        elif kind == "stamped":
            stamped.append(body.split(b"\0")[0].decode("utf-8", "replace"))
        i += 1

    if stamped:
        sys.exit("error: this DLL is STAMPED with a real path and must not ship:\n"
                 + "".join(f"           {s}\n" for s in stamped)
                 + "       rebuild with `cargo build --release` and package that,\n"
                 "       not the copy install.py wrote into the game folder.")
    if unstamped != 1:
        sys.exit(f"error: found {unstamped} unstamped stamp buffer(s); expected exactly "
                 "one. The reservation in lib.rs may have changed.")


def main():
    if "--list" in sys.argv:
        print("ships:")
        for src, name in [("momomod_manager.dll (built)", "momomod_manager.dll")] + SHIPS:
            print(f"  {name:28} <- {src}")
        print("\nstays behind:")
        for name, why in STAYS.items():
            print(f"  {name:28} {why}")
        print("\nnever ships: a DLL carrying a stamped path (checked at build time)")
        return 0

    dll = find_dll()
    check_unstamped(dll)

    missing = [s for s, _ in SHIPS if not os.path.isfile(os.path.join(KIT_DIR, s))]
    if missing:
        sys.exit(f"error: missing files that must ship: {missing}")

    os.makedirs(DIST, exist_ok=True)
    out = os.path.join(DIST, f"momomod-{version()}.zip")
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        z.write(dll, "momomod_manager.dll")
        for src, name in SHIPS:
            z.write(os.path.join(KIT_DIR, src), name)

    size = os.path.getsize(out)
    print(f"built {out}  ({size / 1024:.0f} KB)")
    with zipfile.ZipFile(out) as z:
        for info in z.infolist():
            print(f"    {info.filename:28} {info.file_size / 1024:8.1f} KB")
    print("\nthe DLL in this zip carries no path; install.py stamps one as it installs.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
