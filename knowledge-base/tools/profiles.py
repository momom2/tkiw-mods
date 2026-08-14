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


# Runs written before the profiler learned to apply its own name table recorded raw
# addresses in the `self` column while the log said "ogg/vorbis decode". Grouping by
# name would then split one function across two labels and halve both.
#
# The authoritative table is KNOWN in tkiw-momomod-kit/src/features/profiler.rs; this
# only has to cover names that appear in CSVs already on disk.
ALIASES = {
    "sub_1c9fd30": "texture page decompress",
    "sub_1c9fb28": "texture page decompress",
    "sub_1ca3ab0": "texture page checksum",
    "sub_1ea5cc0": "memset",
    "sub_1b54600": "string table lookup (linear)",
    "sub_1c09667": "qoi image decode",
    # Runs recorded before a captured stack showed these are under texture_prefetch
    # and not the audio subsystem. Same addresses, corrected name.
    "ogg/vorbis decode": "texture page decompress",
    "ogg page checksum": "texture page checksum",
}


# Two-sided 95% critical values of Student's t, by degrees of freedom. The runs are
# few, so the normal approximation is wrong in the direction that matters -- it would
# report intervals narrower than the data supports.
T95 = {1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571, 6: 2.447, 7: 2.365,
       8: 2.306, 9: 2.262, 10: 2.228, 11: 2.201, 12: 2.179, 13: 2.160, 14: 2.145,
       15: 2.131, 16: 2.120, 17: 2.110, 18: 2.101, 19: 2.093, 20: 2.086,
       24: 2.064, 29: 2.045, 39: 2.023, 59: 2.001}


def ci95(vals):
    """95% confidence interval for the mean, as (low, high).

    On the mean, not on the runs: it says how well these launches pin down the true
    share, not how much any one launch may differ. A wide interval here means take
    more runs; a wide *range* means the game itself varies.
    """
    n = len(vals)
    if n < 2:
        return (vals[0], vals[0]) if vals else (0.0, 0.0)
    mean = statistics.mean(vals)
    se = statistics.stdev(vals) / (n ** 0.5)
    df = n - 1
    t = T95.get(df) or T95[min(T95, key=lambda k: abs(k - df))]
    return (mean - t * se, mean + t * se)


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
            name = ALIASES.get(row["name"], row["name"])
            key = (row["phase"], row["kind"], name)
            share = 100.0 * n / total
            # An alias merges several functions into one name, and how to combine them
            # depends on what is being counted.
            #
            # `self` and `responsible` charge each sample to exactly one function, so
            # merged rows are disjoint and add. `inclusive` counts a sample once per
            # function on its stack -- and a subsystem's parent and its worker are both
            # on the stack together, so adding them counted the same sample twice and
            # reported 120% of a phase. Max is the honest merge: a lower bound on
            # "samples with any of these on the stack".
            if row["kind"] == "inclusive":
                out[key] = max(out.get(key, 0.0), share)
            else:
                out[key] = out.get(key, 0.0) + share
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

        rows = sorted(shares.items(), key=lambda kv: statistics.mean(kv[1]), reverse=True)
        print("\n=== %s   (%s, in %d/%d run(s))" % (phase, args.kind, seen_phase, len(runs)))
        # Mean and median together, because a gap between them is the finding: they
        # agree on a stable cost and diverge on one that is present in some runs and
        # absent in others. The standard deviation says how far apart the runs were.
        print("    mean         95% CI   median        range   name")
        for name, vals in rows[:args.top]:
            avg = statistics.mean(vals)
            med = statistics.median(vals)
            lo, hi = ci95(vals)
            if avg < 0.05 and max(vals) < 0.5:
                continue
            print("  %5.1f%%  %5.1f-%5.1f%%  %5.1f%%  %5.1f-%5.1f%%  %s"
                  % (avg, lo, hi, med, min(vals), max(vals), name))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
