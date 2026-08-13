#!/usr/bin/env python3
"""
Set the kit up for a startup measurement, and launch.

A profiling run is only worth as much as the thing it profiles resembles the real
game. Everything the kit can do to the game is switched off for the duration --
including the picker, whose startup probe reads the 46 MB executable and sleeps in
five-second rounds, which is not a thing the unmodified game does.

  python measure-startup.py                 baseline: nothing patched, profiler on
  python measure-startup.py --with fast_boot,font_atlases
  python measure-startup.py --restore       put the config back as it was

The config files are rewritten in place and a copy of each is kept as
`<name>.ini.premeasure`, so `--restore` puts back exactly what was there.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
KIT = os.path.join(ROOT, "tkiw-momomod-kit")
CONFIG = os.path.join(KIT, "config")
LOG = os.path.join(KIT, "momomod.log")

# Everything that changes the game, and so must be off for a baseline.
PATCHING = ["fast_boot", "font_atlases", "popup_stutter_fix", "morale_fix",
            "fortifications_cap"]
# Diagnostics: on, because they are the measurement.
WATCHING = ["profiler", "timeline"]
# Off even so: its one-shot dump costs seconds at the menu.
QUIET = ["dump_libraries"]


def files():
    return [os.path.join(CONFIG, f) for f in os.listdir(CONFIG) if f.endswith(".ini")]


def save():
    for p in files():
        if not os.path.exists(p + ".premeasure"):
            shutil.copy2(p, p + ".premeasure")


def restore():
    n = 0
    for p in files():
        if os.path.exists(p + ".premeasure"):
            shutil.copy2(p + ".premeasure", p)
            os.remove(p + ".premeasure")
            n += 1
    print("restored %d config file(s)" % n)


def set_enabled(path, feature, on):
    """Flip one feature's `enabled`, leaving every other line alone."""
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    want = "[feature.%s]" % feature
    inside = False
    changed = False
    for i, line in enumerate(lines):
        s = line.strip()
        if s.startswith("["):
            inside = s == want
            continue
        if inside and re.match(r"enabled\s*=", s):
            lines[i] = "enabled = %s" % ("true" if on else "false")
            changed = True
            inside = False
    if changed:
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write("\n".join(lines) + "\n")
    return changed


def apply(keep_on):
    for feature in PATCHING:
        on = feature in keep_on
        for p in files():
            if set_enabled(p, feature, on):
                break
    for feature in WATCHING:
        for p in files():
            if set_enabled(p, feature, True):
                break
    for feature in QUIET:
        for p in files():
            if set_enabled(p, feature, False):
                break
    # The picker is a whole mod, switched off in the kit's own file.
    kit = os.path.join(CONFIG, "momomod.ini")
    if os.path.exists(kit):
        with open(kit, encoding="utf-8") as fh:
            text = fh.read()
        text = re.sub(r"(?m)^reward-picker\s*=.*$", "reward-picker = false", text)
        with open(kit, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--with", dest="keep", default="",
                    help="comma-separated features to leave switched on")
    ap.add_argument("--restore", action="store_true")
    ap.add_argument("--timeout", type=float, default=200.0)
    args = ap.parse_args(argv)

    if args.restore:
        restore()
        return 0

    keep = [f.strip() for f in args.keep.split(",") if f.strip()]
    unknown = [f for f in keep if f not in PATCHING]
    if unknown:
        print("not a patching feature: %s" % ", ".join(unknown))
        print("known: %s" % ", ".join(PATCHING))
        return 2

    save()
    apply(keep)
    print("measuring with: %s" % (", ".join(keep) if keep else "nothing patched"))
    subprocess.run([sys.executable, os.path.join(KIT, "install.py")],
                   capture_output=True, text=True)
    subprocess.run([sys.executable, os.path.join(HERE, "playtest.py"),
                    "--log", LOG, "--until", "obj_main_menu",
                    "--timeout", str(args.timeout)])
    print("\nlog: %s" % LOG)
    print("csv: %s" % os.path.join(KIT, "profile.csv"))
    print("run --restore when done.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
