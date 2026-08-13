#!/usr/bin/env python3
"""
Build the zip that goes to players.

Three kinds of file live in this folder, and only the first kind ships:

  ship     what a player needs: the DLL, the scripts that install it, the readme
  source   what a developer needs: src/, tests/, analysis/, docs/, the spec
  private  what belongs to whoever ran it: logs, configs, save snapshots

The split is enforced here by an allow-list, not by an ignore-list. A new file
appearing in this folder is left out of the zip by default, which is the safe
direction: forgetting to ship something is a bug report, shipping someone's
save directory is not recoverable.

    python package.py            build dist/<name>-<version>.zip
    python package.py --list     show what would go in, and what would not
"""
import hashlib
import os
import re
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
NAME = "tkiw-reward-auto-picker"
BUILT_DLL = os.path.join("target", "release", "tkiw_reward_picker.dll")

# Everything a player needs, and nothing else.
SHIP = [
    ("README.md", "README.md"),
    ("install.py", "install.py"),
    ("uninstall.py", "uninstall.py"),
    ("restore-saves.py", "restore-saves.py"),
    (BUILT_DLL, "dist/tkiw_reward_picker.dll"),
]

# Named so `--list` can explain itself rather than just staying silent.
SOURCE = ["src", "tests", "analysis", "docs", "spec.md", "make-config.py",
          "Cargo.toml", "Cargo.lock", ".cargo", "package.py", ".gitignore"]
PRIVATE = ["config.ini", "config.reference.ini", "picker.log", "crash.log",
           "save-backups", "probe.incomplete", "target", "__pycache__"]


def version():
    with open(os.path.join(HERE, "Cargo.toml"), encoding="utf-8") as fh:
        m = re.search(r'^version\s*=\s*"([^"]+)"', fh.read(), re.M)
    return m.group(1) if m else "0.0.0"


def check_dll_is_unstamped(path):
    """The DLL must not carry a mod-folder path when it ships.

    `install.py` stamps the player's own folder into its copy as it installs.
    A DLL that already carried one would leak whoever built it -- an absolute
    path with a username in it -- to everybody who downloads the zip.
    """
    with open(path, "rb") as fh:
        data = fh.read()
    marker = b"TKIW_PICKER_MOD_DIR="
    at = data.find(marker)
    if at < 0:
        sys.exit("error: no stamp marker in the DLL; install.py will refuse it too.")
    after = data[at + len(marker):at + len(marker) + 512]
    baked = after.split(b"\x00", 1)[0].decode("utf-8", "replace")
    if baked:
        sys.exit(f"error: the built DLL already has a path stamped into it:\n"
                 f"    {baked}\n"
                 f"Rebuild with `cargo build --release` before packaging; do not\n"
                 f"ship a stamped DLL, it carries the builder's folder path.")


def main():
    listing = "--list" in sys.argv

    if listing:
        print("ships to players:")
        for src, dst in SHIP:
            mark = " " if os.path.exists(os.path.join(HERE, src)) else "  MISSING"
            print(f"    {dst:<36}{mark}")
        print("\nstays in the repository (developers get it from git, not the zip):")
        for p in SOURCE:
            print(f"    {p}")
        print("\nnever leaves this machine:")
        for p in PRIVATE:
            print(f"    {p}")
        print("\nAnything not named above is excluded by default.")
        return 0

    dll = os.path.join(HERE, BUILT_DLL)
    if not os.path.exists(dll):
        sys.exit("error: no built DLL. Run `cargo build --release` first.")
    check_dll_is_unstamped(dll)

    out_dir = os.path.join(HERE, "dist")
    os.makedirs(out_dir, exist_ok=True)
    zip_path = os.path.join(out_dir, f"{NAME}-v{version()}.zip")
    if os.path.exists(zip_path):
        os.remove(zip_path)

    missing = [s for s, _ in SHIP if not os.path.exists(os.path.join(HERE, s))]
    if missing:
        sys.exit("error: missing files to ship: " + ", ".join(missing))

    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as z:
        for src, dst in SHIP:
            z.write(os.path.join(HERE, src), f"{NAME}/{dst}")

    with open(zip_path, "rb") as fh:
        digest = hashlib.sha256(fh.read()).hexdigest()

    print(f"built {os.path.relpath(zip_path, HERE)}")
    print(f"      {os.path.getsize(zip_path) // 1024} KB")
    print(f"      sha256 {digest}")
    print()
    print("Contents:")
    for _, dst in SHIP:
        print(f"    {NAME}/{dst}")
    print()
    print("No config is shipped: the mod writes a complete, inert one on first")
    print("launch, read from the player's own copy of the game.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
