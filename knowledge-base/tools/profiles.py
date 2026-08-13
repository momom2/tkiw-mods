#!/usr/bin/env python3
"""
Aggregate the profiler's runs, because one run does not mean anything.

Two launches of the same configuration put an audio codec at a third of a phase and
then did not show it at all. Time to a main menu varied by 18.8 seconds with nothing
changed. Any single profile is a hypothesis; this reports the **median across runs**
and the spread beside it, so a number that moves is visibly a number that moves.

  python profiles.py                     every phase, by responsible GML function
  python profiles.py --kind self         where the CPU actually was
  python profiles.py --phase obj_init
  python profiles.py --runs 5            only the five most recent

Reads `tkiw-momomod-kit/profiles/run-*.csv`, which the profiler writes one per launch.
"""
import argparse
import csv
import os
import statistics
import sys
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
PROFILES = os.path.join(ROOT, "tkiw-momomod-kit", "profiles")


def load(path):
    """{(phase, kind, name): share of that phase's samples} for one run."""
    out = {}
    with open(path, encoding="utf-8", newline="") as fh:
        for row in csv.DictReader(fh):
            try:
                total = int(row["phase_samples"])
                n = int(row["samples"])
            except (ValueError, KeyError):
                continue
            if total <= 0:
                continue
            out[(row["phase"], row["kind"], row["name"])] = 100.0 * n / total
    return out


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--kind", default="responsible",
                    help="responsible | self | inclusive | module")
    ap.add_argument("--phase", default=None)
    ap.add_argument("--top", type=int, default=15)
    ap.add_argument("--runs", type=int, default=0, help="most recent N, 0 for all")
    args = ap.parse_args(argv)

    if not os.path.isdir(PROFILES):
        print("no runs yet: %s does not exist" % PROFILES)
        return 1
    files = sorted(f for f in os.listdir(PROFILES) if f.endswith(".csv"))
    if args.runs:
        files = files[-args.runs:]
    if not files:
        print("no runs in %s" % PROFILES)
        return 1

    runs = [load(os.path.join(PROFILES, f)) for f in files]
    print("%d run(s): %s" % (len(runs), ", ".join(files)))

    # A row missing from a run counts as zero for that run -- it was sampled and did
    # not appear, which is a real observation and the reason the spread matters.
    phases = {k[0] for r in runs for k in r}
    if args.phase:
        phases = {p for p in phases if p == args.phase}

    for phase in sorted(phases):
        shares = defaultdict(list)
        seen_phase = 0
        for r in runs:
            if not any(k[0] == phase for k in r):
                continue
            seen_phase += 1
            names = {k[2] for k in r if k[0] == phase and k[1] == args.kind}
            for name in names:
                shares[name].append(r.get((phase, args.kind, name), 0.0))
        if not shares:
            continue
        # pad with zeros for runs where the phase appeared but the name did not
        for name, vals in shares.items():
            vals.extend([0.0] * (seen_phase - len(vals)))

        rows = sorted(shares.items(), key=lambda kv: statistics.median(kv[1]), reverse=True)
        print("\n=== %s   (%s, in %d/%d run(s))" % (phase, args.kind, seen_phase, len(runs)))
        print("  median      range   name")
        for name, vals in rows[:args.top]:
            med = statistics.median(vals)
            if med < 0.05 and max(vals) < 0.5:
                continue
            print("  %5.1f%%  %5.1f-%5.1f%%  %s" % (med, min(vals), max(vals), name))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
