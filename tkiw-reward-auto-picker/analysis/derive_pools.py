#!/usr/bin/env python3
"""
Derive each reward type's option pool from the game's own data.

A reward type offers a *filtered subset* of its library, not the whole thing:
`artifact_legend` offers only legendary artifacts, `improvement_production_t1`
only tier-1 production buildings. The filters are fields on the library entries
themselves, which the mod dumps to its log (`FIELD <library> <key> | ...`).

This parses that dump and writes `docs/pools.json`, from which `make-config.py`
builds the config. Nothing here infers a filter -- it reads the values the game
actually holds and applies rules stated in one place, below.

  python derive_pools.py [path to picker.log]
"""
import collections
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))
LOG = os.path.join(ROOT, "picker.log")
OUT = os.path.join(ROOT, "docs", "pools.json")

FIELD = re.compile(r"FIELD (\w+) (\S+) \| (.*)$")
KV = re.compile(r'(\w+)=(?:Str\("([^"]*)"\)|Int\((-?\d+)\)|Real\((-?[\d.]+)\)|Bool\((true|false)\))')


def parse(path):
    """{library: {key: {field: value}}} from the mod's log."""
    libs = collections.defaultdict(dict)
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            m = FIELD.search(line.strip())
            if not m:
                continue
            lib, key, rest = m.groups()
            fields = {}
            for kv in KV.finditer(rest):
                name = kv.group(1)
                s, i, r, b = kv.group(2), kv.group(3), kv.group(4), kv.group(5)
                if s is not None:
                    fields[name] = s
                elif i is not None:
                    fields[name] = int(i)
                elif r is not None:
                    fields[name] = float(r)
                else:
                    fields[name] = b == "true"
            libs[lib][key] = fields
    return libs


# ---------------------------------------------------------------- the rules
#
# Stated in one place so they can be argued with. The governing principle: if an
# id could be offered under *some* run -- any king, any level, any stage --
# include it, because leaving it out makes the mod refuse a choice it meets. If
# it could never be offered for that type, leave it out, because every extra
# line makes the file harder to navigate.
#
# `unlocked=false` entries ARE included: unlocked is meta-progression state, so
# a locked artifact is one the player has not earned *yet*, not one that can
# never appear.
#
# `level` gates an entry to one biome ("graveyard", "lava"). Those are included
# too: they cannot appear in every run, but they can appear in some.

def artifacts(entries):
    """tier 1 = ordinary, tier 2 = legendary. 109 / 18 in the observed dump."""
    ordinary = sorted(k for k, f in entries.items() if f.get("tier") == 1)
    legend = sorted(k for k, f in entries.items() if f.get("tier") == 2)
    return ordinary, legend


def spells(entries):
    """Same split as artifacts: 39 ordinary, 10 legendary."""
    ordinary = sorted(k for k, f in entries.items() if f.get("tier") == 1)
    legend = sorted(k for k, f in entries.items() if f.get("tier") == 2)
    return ordinary, legend


def improvements(entries):
    """Everything that is not excluded from the drop pool.

    Splitting these by reward type needs the game's own `IMPROVEMENTS_BY_CATEGORY`
    and `IMPROVEMENTS_BY_TIER` groupings, which are not in this dump yet -- so
    this returns the whole eligible set and the per-type split is left undone
    rather than guessed.
    """
    return sorted(k for k, f in entries.items()
                  if not f.get("excluded_from_drop_pool", False))


# ---------------------------------------------------- improvement reward types
#
# Derived from the game's own `IMPROVEMENTS_BY_CATEGORY` and `_BY_TIER` maps
# rather than from any reading of what a category number might mean:
#
#   category 0  producers        -> improvement_production_t{1,2,3}
#   category 1  unit producers   -> improvement_troops_t{1,2,3}
#              (the 9 `imp_*` ones, 3 per tier, are the infernal barracks)
#   category 2  offensive        -> improvement_attacking   (no tier split)
#   category 3  infrastructure   -> improvement_misc        (no tier split)
#   category 4  special/event    -> never a reward: 13 of 13 excluded
#   category 5  terrain          -> never a reward: 19 of 19 excluded
#
# Categories 4 and 5 being wholly `excluded_from_drop_pool` is what confirms the
# mapping: the two groups that are never offered are exactly the two that the
# game flags.
#
# The infernal buildings are left in the troops pools as well as their own. They
# are category 1, so a troops reward could offer one; including them is the
# cautious direction, and a missing id makes the mod refuse a real choice.

CATEGORY_PRODUCTION, CATEGORY_TROOPS = "0", "1"
CATEGORY_ATTACKING, CATEGORY_MISC = "2", "3"


def improvement_pools(groups, excluded):
    cat, tier = groups["category"], groups["tier"]
    keep = lambda xs: sorted(x for x in xs if x not in excluded)
    out, notes = {}, {}

    for n in ("1", "2", "3"):
        both = set(cat.get(CATEGORY_PRODUCTION, [])) & set(tier.get(n, []))
        out[f"improvement_production_t{n}"] = keep(both)
        both = set(cat.get(CATEGORY_TROOPS, [])) & set(tier.get(n, []))
        out[f"improvement_troops_t{n}"] = keep(both)
        infernal = {x for x in both if x.startswith("imp_")}
        out[f"improvement_infernals_t{n}"] = keep(infernal)

    infernal_all = {x for x in cat.get(CATEGORY_TROOPS, []) if x.startswith("imp_")}
    out["improvement_infernals"] = keep(infernal_all)
    out["improvement_attacking"] = keep(cat.get(CATEGORY_ATTACKING, []))
    out["improvement_misc"] = keep(cat.get(CATEGORY_MISC, []))

    for k, v in out.items():
        notes[k] = f"{len(v)} from the game's own category/tier grouping"
    return out, notes


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else LOG
    if not os.path.isfile(path):
        sys.exit(f"error: no log at {path}")
    libs = parse(path)
    if not libs:
        sys.exit("error: no FIELD lines in the log; run the game with the mod installed")

    pools, notes = {}, {}

    if "ARTIFACTS" in libs:
        ordinary, legend = artifacts(libs["ARTIFACTS"])
        pools["artifact"] = ordinary
        pools["artifact_legend"] = legend
        notes["artifact"] = f"tier 1 ({len(ordinary)} of {len(libs['ARTIFACTS'])})"
        notes["artifact_legend"] = f"tier 2 ({len(legend)})"

    if "SPELLS" in libs:
        ordinary, legend = spells(libs["SPELLS"])
        pools["spell"] = ordinary
        pools["spell_legend"] = legend
        notes["spell"] = f"tier 1 ({len(ordinary)} of {len(libs['SPELLS'])})"
        notes["spell_legend"] = f"tier 2 ({len(legend)})"

    if "UPGRADES" in libs:
        # Upgrades are per-improvement (`upgrade_of`); which are offered depends
        # on what is built, so any of them could come up.
        pools["upgrade"] = sorted(libs["UPGRADES"])
        notes["upgrade"] = f"all {len(pools['upgrade'])}; offered per built improvement"

    if "IMPROVEMENTS" in libs:
        eligible = improvements(libs["IMPROVEMENTS"])
        pools["_improvements_eligible"] = eligible
        # Kept so the config can show what was left out. An exclusion nobody can
        # see is an exclusion nobody can check, and every filter here has been
        # wrong at least once.
        pools["_improvements_excluded"] = sorted(
            k for k, f in libs["IMPROVEMENTS"].items()
            if f.get("excluded_from_drop_pool", False))
        notes["_improvements_excluded"] = (
            f"{len(pools['_improvements_excluded'])} entries flagged "
            "excluded_from_drop_pool by the game")
        notes["_improvements_eligible"] = (
            f"{len(eligible)} of {len(libs['IMPROVEMENTS'])} are not "
            "excluded_from_drop_pool; the per-type split still needs "
            "IMPROVEMENTS_BY_CATEGORY / _BY_TIER from the game"
        )

    groups_path = os.path.join(ROOT, "docs", "improvement-groups.json")
    if os.path.isfile(groups_path) and "_improvements_excluded" in pools:
        with open(groups_path, encoding="utf-8") as fh:
            groups = json.load(fh)
        p2, n2 = improvement_pools(groups, set(pools["_improvements_excluded"]))
        pools.update(p2)
        notes.update(n2)

    with open(OUT, "w", encoding="utf-8") as fh:
        json.dump({"pools": pools, "notes": notes}, fh, indent=2)

    print(f"wrote {OUT}\n")
    for k in sorted(pools):
        print(f"  {k:28} {len(pools[k]):>4}   {notes.get(k, '')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
