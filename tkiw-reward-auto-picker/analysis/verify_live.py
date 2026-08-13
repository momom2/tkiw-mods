#!/usr/bin/env python3
"""
Check the generated docs against the live game.

The mod dumps the game's own libraries (`REWARDS`, `RESOURCES`, `ARTIFACTS`, …)
into `picker.log` once per run, by walking their ds_map buckets. Those keys are
ground truth for the id vocabularies the config file is written against, so
this diffs them against `docs/vocabulary.json`, which was produced by static
analysis of the executable.

Agreement means the config can be trusted. Disagreement means the static
extraction missed something, and the live list wins.

  python verify_live.py [path to picker.log]
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))
DEFAULT_LOG = os.path.join(ROOT, "picker.log")
VOCAB = os.path.join(ROOT, "docs", "vocabulary.json")
OUT = os.path.join(ROOT, "docs", "live-libraries.json")

HEADER = re.compile(r"^\[[^\]]*\]\s+(\w+): (\d+) entries\s*$")
CONT = re.compile(r"^\[[^\]]*\]\s{6,}(\S.*)$")


def parse(path):
    """{library: [keys]} from the most recent dump in the log."""
    libs, current = {}, None
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            m = HEADER.match(line.rstrip("\n"))
            if m:
                current = m.group(1)
                libs[current] = []          # a later dump replaces an earlier one
                continue
            if current:
                c = CONT.match(line.rstrip("\n"))
                if c:
                    libs[current].extend(k.strip() for k in c.group(1).split(",") if k.strip())
                elif line.strip().startswith("["):
                    current = None
    return libs


def compare(name, live, static):
    live_s, static_s = set(live), set(static)
    only_live = sorted(live_s - static_s)
    only_static = sorted(static_s - live_s)
    ok = not only_live and not only_static
    status = "OK" if ok else "MISMATCH"
    print(f"{name:<14} live {len(live_s):>4}   docs {len(static_s):>4}   {status}")
    for k in only_live:
        print(f"    only in live game : {k}")
    for k in only_static:
        print(f"    only in docs      : {k}")
    return ok


def main():
    log = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_LOG
    if not os.path.isfile(log):
        sys.exit(f"error: no log at {log}")
    libs = parse(log)
    if not libs:
        sys.exit("error: no library dump found in the log "
                 "(the mod writes one once per run)")

    for k, v in libs.items():
        if len(v) != len(set(v)):
            print(f"warning: {k} has duplicate keys in the dump")

    with open(OUT, "w", encoding="utf-8") as fh:
        json.dump({k: sorted(v) for k, v in libs.items()}, fh, indent=2)
    print(f"wrote {OUT}\n")

    if not os.path.isfile(VOCAB):
        print("no docs/vocabulary.json to compare against; run extract.py first")
        return 0
    with open(VOCAB, encoding="utf-8") as fh:
        vocab = json.load(fh)

    ok = True
    if "REWARDS" in libs:
        ok &= compare("reward types", libs["REWARDS"],
                      [r["id"] for r in vocab["reward_types"]])
    if "RESOURCES" in libs:
        ok &= compare("resources", libs["RESOURCES"],
                      [r["id"] for r in vocab["resources"] if r.get("scope") != "meta"])
    if "ARTIFACTS" in libs:
        ok &= compare("artifacts", libs["ARTIFACTS"],
                      [a["id"] for a in vocab["artifacts"]])
    if "UNIT_CLASSES" in libs:
        n_live, n_docs = len(libs["UNIT_CLASSES"]), len(vocab["unit_classes"])
        same = n_live == n_docs
        ok &= same
        print(f"{'unit classes':<14} live {n_live:>4}   docs {n_docs:>4}   "
              f"{'OK' if same else 'MISMATCH'}")

    print()
    for name in ("SPELLS", "IMPROVEMENTS", "UPGRADES", "ADVISORS", "UNITS"):
        if name in libs:
            print(f"{name.lower():<14} live {len(libs[name]):>4}   "
                  "(no static list -- these are option vocabularies for their "
                  "reward types)")

    print("\n" + ("all vocabularies agree with the live game."
                  if ok else "DISAGREEMENT: the live game wins; regenerate or fix the docs."))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
