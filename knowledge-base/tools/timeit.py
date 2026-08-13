#!/usr/bin/env python3
"""
How long does the game take to start? Asked properly, N times.

Every startup figure in this repository so far came from a single launch, and a
single launch is worth very little: measured back to back on one machine, time to
the main menu ranged from 36 to 56 seconds with nothing changed. A claim like
"25% faster" cannot be built on that, and one A/B that went the wrong way cannot
disprove it either.

This launches the game N times, reads the main-menu timestamp from the mod's own
timeline, and reports every run plus the median and the spread. Two of these, with
one setting flipped between them, is what an honest before-and-after looks like.

  python timeit.py --runs 5
  python timeit.py --runs 5 --label "font_atlases off"

Nothing is changed between runs -- flipping the setting is the caller's job, so
that what is being compared is explicit rather than implied.
"""
import argparse
import os
import re
import statistics
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
DEFAULT_LOG = os.path.join(ROOT, "tkiw-momomod-kit", "momomod.log")

# `[     41.534] [timeline]   41.534s  + obj_main_menu  (the main menu)`
MENU = re.compile(r"^\[\s*([0-9.]+)\]\s+\[timeline\].*\+ obj_main_menu")


def one_run(log, timeout):
    """Launch once; return seconds to the main menu, or None if it never got there."""
    cmd = [
        sys.executable, os.path.join(HERE, "playtest.py"),
        "--log", log,
        "--until", "obj_main_menu",
        "--timeout", str(timeout),
    ]
    subprocess.run(cmd, capture_output=True, text=True)
    try:
        with open(log, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                m = MENU.match(line)
                if m:
                    return float(m.group(1))
    except OSError:
        return None
    return None


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--log", default=DEFAULT_LOG)
    ap.add_argument("--timeout", type=float, default=180.0)
    ap.add_argument("--label", default="")
    args = ap.parse_args(argv)

    if args.label:
        print("== %s" % args.label)
    times = []
    for i in range(args.runs):
        t = one_run(args.log, args.timeout)
        if t is None:
            print("  run %d/%d: never reached the menu" % (i + 1, args.runs))
            continue
        times.append(t)
        print("  run %d/%d: %6.1fs" % (i + 1, args.runs, t))

    if not times:
        print("no successful runs")
        return 1
    times.sort()
    print("  ---")
    print("  median %.1fs   min %.1fs   max %.1fs   spread %.1fs   n=%d"
          % (statistics.median(times), times[0], times[-1],
             times[-1] - times[0], len(times)))
    # The spread is the number that decides whether a difference means anything.
    # Printed last, and on its own, because it is the one that gets forgotten.
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
