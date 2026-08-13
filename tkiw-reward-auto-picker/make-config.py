#!/usr/bin/env python3
"""
Generate a complete, inert config.ini.

Every option starts blacklisted and every reroll budget at zero, so the mod
resolves nothing until you edit it. The file doubles as the reference for what
is configurable: you move ids between tiers rather than typing them from memory,
which is what makes the "every id in exactly one tier" rule affordable.

Ids come from docs/, which is generated from the game and checked against the
running game by analysis/verify_live.py.

Usage:
  python make-config.py            write config.ini (refuses to overwrite)
  python make-config.py --force    regenerate, KEEPING your tier/weight edits
  python make-config.py --reset    regenerate from scratch, discarding edits
  python make-config.py --only unit_class_stat,resource

Regeneration preserves what you have set. An id that still exists keeps the
tier and weight you gave it; new ids arrive blacklisted; ids the game no longer
has are dropped. Budgets are carried over too. Losing a config to a regenerate
is a bad trade for a game update.
"""
import json
import os
import sys

MOD_DIR = os.path.dirname(os.path.abspath(__file__))
DOCS = os.path.join(MOD_DIR, "docs")
OUT = os.path.join(MOD_DIR, "config.ini")
NL = chr(10)

# Reward types the mod can drive, most useful first. Everything else is either
# a direct grant, an open-ended screen, or has no addressable id -- see
# docs/reward-types.md.
HANDLED = [
    ("unit_class_stat", "Troops Training", "unit_class_stat", False),
    ("resource", "Resource", "RESOURCES", True),
    ("artifact", "Artifact", "pool", True),
    ("artifact_legend", "Legendary Artifact", "pool", True),
    ("spell", "Spell", "pool", True),
    ("spell_legend", "Legendary Spell", "pool", True),
    ("upgrade", "Building Upgrade", "pool", True),
    ("improvement_production_t1", "Basic Production", "pool", True),
    ("improvement_production_t2", "Established Production", "pool", True),
    ("improvement_production_t3", "Advanced Production", "pool", True),
    ("improvement_troops_t1", "Levy Barracks", "pool", True),
    ("improvement_troops_t2", "Veteran Barracks", "pool", True),
    ("improvement_troops_t3", "Elite Barracks", "pool", True),
    ("improvement_infernals", "Infernal Barracks", "pool", True),
    ("improvement_infernals_t1", "Infernal Barracks T1", "pool", True),
    ("improvement_infernals_t2", "Infernal Barracks T2", "pool", True),
    ("improvement_infernals_t3", "Infernal Barracks T3", "pool", True),
    ("improvement_misc", "Kingdom Infrastructure", "pool", True),
    ("improvement_attacking", "Offensive Structures", "pool", True),
]

HEADER = """\
# tkiw auto reward picker -- configuration
# ============================================================================
#
# As generated, this file does NOTHING. Every option is blacklisted and every
# reroll budget is zero, so the mod stays out of the way until you edit it.
#
# ---------------------------------------------------------------- the tiers
#
#   [<type>.wanted]     take it as soon as it is offered
#   [<type>.fallback]   only once the reroll budget is spent
#   [<type>.blacklist]  never take it
#
# Move ids between those three sections. The number after an id is a WEIGHT,
# which orders options WITHIN a tier -- a wanted option always beats a fallback
# one whatever the numbers say. Equal weights mean you genuinely do not mind
# which you get, and the mod picks between them at random.
#
#   metal = 10      \\  both wanted, equally: take whichever shows up, and if
#   ore   = 10      /   both show up, either will do
#   wine  = 5           wanted, but only if neither of the above is offered
#
# ------------------------------------------------------------- the rerolls
#
#   voodoo_depth    rerolls using the per-reward freebie (Voodoo Beads)
#   free_depth      rerolls from your reign-wide free pool
#   paid_depth      rerolls paid for in denarii
#   denarii_floor   never pay for a reroll that drops you below this
#
# These are caps on what the mod will TRIGGER, not on what it will spend. The
# game decides what a reroll costs -- it always spends free rerolls first -- so
# the mod matches the cost the game reports against that budget and stops if it
# is spent. It never "upgrades" from a spent free budget to the paid one. With
# free_depth=1 and paid_depth=1 and two free rerolls banked, it rerolls once and
# stops: paid_depth is unreachable until the free pool runs dry. That is
# intended, not a bug.
#
# Rerolls the mod spends are spent even if it ends up handing the reward back.
#
# ---------------------------------------------------------------- the rules
#
# * Delete a whole [<type>] section to leave that reward type entirely manual.
#   Any type not mentioned here is manual too. That is the safe default.
# * Every id must appear in exactly one tier. The mod refuses to auto-resolve a
#   choice containing an id it was not told about, so it fails safe rather than
#   guessing.
# * Only the reward at the FRONT of the queue is ever touched, and the mod stops
#   as soon as it reaches one it has no section for.
# * Automating a type means giving up the ability to save it for later.
#
# ============================================================================

[global]
enabled  = true

# How fast the mod is allowed to act, in milliseconds between button presses.
# It is not a fixed wait: the mod watches for the queue or the cards changing
# and reacts immediately, so this only caps the rate. 0 means as fast as the
# game will accept (floored at 40ms so a reroll cannot be spammed).
delay_ms = 100

# act = false  -> decide and log "[would PICK] ...", but press nothing
# act = true   -> actually press the buttons
#
# This is the STARTING value. Ctrl+Alt+P toggles it while you play, so you can
# switch pressing on and off without leaving the game. Editing it here also
# takes effect immediately -- the file is re-read as you save it.
#
# Either way the mod keeps reading choices and logging what it would do, so
# with pressing off you get a running commentary rather than silence.
act      = false

# trace = true logs each phase as it begins, so if the game crashes the last
# line names what was underway rather than what last finished. Verbose -- turn
# it on only when chasing a crash.
trace    = false
"""


def load(name):
    with open(os.path.join(DOCS, name), encoding="utf-8") as fh:
        return json.load(fh)


def troops_options(vocab):
    """(id, comment) for every (class, stat) pair."""
    classes = vocab["unit_classes"]
    stats = [s["name"] for s in vocab["unit_stats"]]
    offered = set(vocab.get("offered_class_indices", [c["index"] for c in classes]))
    out = []
    for c in classes:
        for s in stats:
            note = "" if c["index"] in offered else "  (never offered by the game)"
            out.append((f"{c['display'].lower()}.{s}", f"{c['display']} +{s}{note}"))
    return out


def section(rid, display, options, can_scrap, notes="", prev_tiers=None, prev_budgets=None):
    prev_tiers = prev_tiers or {}
    prev_budgets = prev_budgets or {}
    width = max((len(i) for i, _ in options), default=0)
    placed = {"wanted": [], "fallback": [], "blacklist": []}
    for i, comment in options:
        tier, weight = prev_tiers.get((rid, i), ("blacklist", None))
        if tier not in placed:
            tier = "blacklist"
        placed[tier].append((i, comment, weight))
    # a ranked id that has no weight would be a warning at load; default it
    for t in ("wanted", "fallback"):
        placed[t] = [(i, c, w if w is not None else "1") for i, c, w in placed[t]]
    L = []
    L.append("")
    L.append("")
    L.append("# " + "-" * 74)
    L.append(f"# {display}   [{rid}]   {len(options)} options")
    if notes:
        L.append(f"# {notes}")
    L.append("# " + "-" * 74)
    L.append(f"[{rid}]")
    b = prev_budgets.get(rid, {})
    for k in ("voodoo_depth", "free_depth", "paid_depth", "denarii_floor"):
        L.append(f"{k.ljust(13)} = {b.get(k, '0')}")

    for tier in ("wanted", "fallback", "blacklist"):
        L.append("")
        L.append(f"[{rid}.{tier}]")
        if tier == "fallback":
            scrap = prev_tiers.get((rid, "_scrap"))
            if not can_scrap:
                L.append("# this reward type has no scrap button, so _scrap is unavailable")
            elif scrap and scrap[0] == "fallback":
                L.append(f"_scrap = {scrap[1] or '1'}")
            else:
                L.append("# _scrap = 1     # settle for scrapping the reward (+5 denarii)")
        elif tier == "wanted" and can_scrap:
            scrap = prev_tiers.get((rid, "_scrap"))
            if scrap and scrap[0] == "wanted":
                L.append(f"_scrap = {scrap[1] or '1'}")
        for i, comment, weight in placed[tier]:
            lhs = i if tier == "blacklist" else f"{i} = {weight}"
            L.append(f"{lhs.ljust(width + 6)}   # {comment}".rstrip() if comment else lhs)
    return NL.join(L)


def read_existing(path):
    """{(section, id): line-suffix} and {section: budget lines} from a config.

    Deliberately textual: it keeps whatever the player wrote, including their
    comments and spacing, rather than reformatting it.
    """
    tiers, budgets = {}, {}
    if not os.path.exists(path):
        return tiers, budgets
    section = None
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            raw = line.rstrip()
            stripped = raw.split("#")[0].strip()
            if not stripped:
                continue
            if stripped.startswith("[") and stripped.endswith("]"):
                section = stripped[1:-1]
                continue
            if section is None or section == "global":
                continue
            if "." in section:
                ty, tier = section.split(".", 1)
                key = stripped.split("=")[0].strip()
                weight = stripped.split("=", 1)[1].strip() if "=" in stripped else None
                # If an id somehow appears twice, keep the deliberate placement.
                # Blacklist is the default everything starts in, so a wanted or
                # fallback entry is the one that carries intent. (The mod itself
                # reports the duplicate at load; this just refuses to silently
                # demote a preference while regenerating.)
                prev = tiers.get((ty, key))
                if prev is None or (prev[0] == "blacklist" and tier != "blacklist"):
                    tiers[(ty, key)] = (tier, weight)
            else:
                k = stripped.split("=")[0].strip()
                v = stripped.split("=", 1)[1].strip() if "=" in stripped else None
                if v is not None:
                    budgets.setdefault(section, {})[k] = v
    return tiers, budgets


def main():
    args = sys.argv[1:]
    force = "--force" in args or "--reset" in args
    only = None
    if "--only" in args:
        only = {s.strip() for s in args[args.index("--only") + 1].split(",")}

    if os.path.exists(OUT) and not force:
        sys.exit(f"error: {OUT} already exists.\n"
                 "       edit it, or pass --force to regenerate and lose your edits.")

    vocab = load("vocabulary.json")
    live = load("live-libraries.json")
    try:
        pools = load("pools.json")
    except FileNotFoundError:
        pools = {"pools": {}, "notes": {}}
    import csv, io
    loc = {}
    try:
        for r in csv.reader(io.open(
                r"C:\Program Files (x86)\Steam\steamapps\common"
                r"\The King is Watching\local\localization.csv", encoding="utf-8-sig")):
            if len(r) > 1 and r[0]:
                loc[r[0]] = (r[2] or r[1]).strip() if len(r) > 2 else r[1].strip()
    except OSError:
        pass
    display = {r["id"]: r.get("display") or r["id"] for r in vocab["reward_types"]}
    res_display = {r["id"]: r.get("display") or r["id"] for r in vocab["resources"]}

    prev_tiers, prev_budgets = ({}, {}) if "--reset" in args else read_existing(OUT)
    if prev_tiers:
        print(f"keeping {len(prev_tiers)} existing tier assignment(s); "
              "pass --reset to discard them")

    body = [HEADER]
    written = 0
    for rid, disp, source, can_scrap in HANDLED:
        if only and rid not in only:
            continue
        if source == "unit_class_stat":
            options = troops_options(vocab)
            notes = ("a choice mixes classes and stats freely -- verified against a "
                     "screenshot")
        elif source == "RESOURCES":
            # `random` is a marker meaning "roll a resource", not something that
            # can be offered as an option, so it is excluded. The biome-specific
            # ones are kept: they cannot appear in every run, but they can
            # appear in some, and the rule is to include anything that could be
            # offered under some run.
            options = [(i, res_display.get(i, "")) for i in live["RESOURCES"]
                       if i != "random"]
            notes = ("biome-specific resources are included -- they appear only on their "
                     "own levels, but they can appear")
        elif source == "pool":
            ids = pools.get("pools", {}).get(rid)
            if ids is None:
                continue  # not derived yet; better absent than wrong
            titles = ("improv_title_" if rid.startswith("improvement_") else {
                "artifact": "artifact_title_", "artifact_legend": "artifact_title_",
                "spell": "spell_title_", "spell_legend": "spell_title_",
                "upgrade": "upgrade_title_",
            }[rid])
            options = [(i, loc.get(titles + i, "")) for i in ids]
            notes = pools.get("notes", {}).get(rid, "")
        else:
            continue
        body.append(section(rid, display.get(rid, disp), options, can_scrap, notes,
                            prev_tiers, prev_budgets))
        written += 1

    # Purely informative: what the game itself marks as never offerable, so the
    # filtering can be checked rather than taken on trust. Entirely commented
    # out -- these are not configurable and the parser must never see them.
    excluded = pools.get("pools", {}).get("_improvements_excluded") or []
    if excluded and not only:
        L = ["", "",
             "# " + "=" * 74,
             "# FOR INFORMATION ONLY -- nothing below here is configurable",
             "# " + "=" * 74,
             "#",
             f"# The game marks these {len(excluded)} improvements "
             "`excluded_from_drop_pool`,",
             "# meaning they can never be offered as a reward, so they are left out of",
             "# the improvement sections. They are listed here so that exclusion can be",
             "# checked: if you recognise one you HAVE been offered, the filter is wrong",
             "# and worth saying so.",
             "#"]
        width = max(len(i) for i in excluded)
        for i in excluded:
            title = loc.get("improv_title_" + i, "")
            L.append(f"#   {i.ljust(width)}   {title}".rstrip())
        body.append(NL.join(L))

    with open(OUT, "w", encoding="utf-8") as fh:
        fh.write(NL.join(body) + NL)

    print(f"wrote {OUT}")
    print(f"  {written} reward type section(s)")
    print(f"  {sum(1 for _ in open(OUT, encoding='utf-8'))} lines")
    print()
    print("Everything starts blacklisted, so the mod does nothing until you edit it.")
    print("Delete any [section] you would rather keep manual.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
