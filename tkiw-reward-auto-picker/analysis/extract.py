#!/usr/bin/env python3
"""
Generate the reference docs in ../docs/ from the game itself.

These are the id vocabularies the config file is written against, so they are
generated rather than transcribed, and regenerating them after a game update is
how you find out what changed.

  python extract.py            write ../docs/*.md and ../docs/vocabulary.json
"""
import csv
import io
import json
import os
import sys
from collections import OrderedDict

import tkiw
from gmldis import Annotator

HERE = os.path.dirname(os.path.abspath(__file__))
DOCS = os.path.normpath(os.path.join(HERE, "..", "docs"))
GAME_LOCAL = (r"C:\Program Files (x86)\Steam\steamapps\common"
              r"\The King is Watching\local\localization.csv")

# reward types whose queue entry presents a choice, and the path each takes
CHOICE_PATHS = OrderedDict([
    ("spawn_improvements_choice", [
        "improvement_production_t1", "improvement_production_t2",
        "improvement_production_t3", "improvement_troops_t1",
        "improvement_troops_t2", "improvement_troops_t3",
        "improvement_infernals", "improvement_infernals_t1",
        "improvement_infernals_t2", "improvement_infernals_t3",
        "improvement_misc", "improvement_attacking", "improvement_any_generic",
        "improvement_any"]),
    ("spawn_artifacts_choice", ["artifact", "artifact_legend"]),
    ("spawn_spells_choice", ["spell", "spell_legend"]),
    ("spawn_upgrades_choice", ["upgrade"]),
    ("spawn_shop", ["shop", "shop_graveyard"]),
    ("spawn_prophecy_choice", ["prophecy"]),
    ("spawn_resources_choice", ["resource"]),
    ("spawn_starting_bonus_choice", ["run_start_bonus"]),
    ("spawn_unit_class_stat_bonus_choice", ["unit_class_stat"]),
    ("spawn_wheel", ["rewards_wheel"]),
])
# Only paths that reach spawn_choice_unified install a scrap button.
# spawn_starting_bonus_choice goes via spawn_bonus_choice, which installs
# neither scrap nor reroll; spawn_prophecy_choice never reaches it at all.
NO_SCRAP_PATHS = {
    "spawn_unit_class_stat_bonus_choice",
    "spawn_shop",
    "spawn_wheel",
    "spawn_prophecy_choice",
    "spawn_starting_bonus_choice",
}
# Types whose "choice" is not a set of alternative cards at all -- an open-ended
# screen the player interacts with freely. These must never be automated.
NOT_A_CARD_PICK = {"shop", "shop_graveyard", "prophecy", "rewards_wheel"}
# Choices that are real choices, but presented on their own bespoke screen
# rather than as option cards. Out of scope: each would need its own UI driver.
OWN_SCREEN = {"onaraks_favour"}
# Notes worth carrying into the table.
TYPE_NOTES = {
    "rewards_wheel": "gated on the Wheel Keeper advisor, which has no display "
                     "name -- parked content, unreachable in normal play",
    "onaraks_favour": "its own screen (`obj_onaraks_reward_screen`) with resource "
                      "buttons; a choice, but not a card pick",
    "run_start_bonus": "cards carry no id, only bundle contents -- not addressable "
                       "by a config",
}

# A Troops Training card offers only HP and Damage. Attack speed exists as a
# unit-class mod (`UNIT_CLASS_MODS_ATTACK_SPD`, and other sources grant it) but
# `assign_rewards_to_cards` builds its option list from `stats = [0, 1]`, and
# `card_setup` has exactly two icon branches. Corroborated in a live save: after
# a run with 115 Troops Training rewards queued, `training_atk_speed` was zero
# for all eight classes while damage and hp carried large values.
UNIT_STATS = [(0, "hp"), (1, "damage")]
# `possible_unit_classes` is the literal [4, 6, 2, 0, 3, 1, 5] -- seven entries,
# shuffled before each offer. Undead (7) exists as a class but is never offered
# by a Troops Training card.
OFFERED_CLASS_INDICES = [4, 6, 2, 0, 3, 1, 5]


def localization():
    rows = list(csv.reader(io.open(GAME_LOCAL, encoding="utf-8-sig")))
    out = {}
    for r in rows:
        if len(r) > 1 and r[0]:
            out[r[0]] = (r[2] or r[1]).strip() if len(r) > 2 else r[1].strip()
    return out


def reward_types(pe, ann):
    """[(id, display)] in library order, from the library_add_reward calls."""
    start = ann.syms["gml_Script_reward_library"]
    end = ann.idx.enclosing(start)[1]
    tgt = ann.syms["gml_Script_library_add_reward"]
    out, buf = [], []
    for insn in tkiw.disasm_function(pe, ann.md, start, end):
        for t in tkiw.rip_targets(pe, insn):
            if t in ann.strs:
                buf.append(ann.strs[t])
        if insn.mnemonic == "call" and insn.op_str.startswith("0x"):
            if pe.va2rva(int(insn.op_str, 16)) == tgt:
                if len(buf) >= 2:
                    out.append((buf[-2], buf[-1]))
                elif buf:
                    out.append((buf[-1], ""))
                buf = []
    # the infernal tiers are registered as format strings built from the
    # preceding entry: `{0}_t1` with {0} = improvement_infernals
    base = None
    for i, (rid, disp) in enumerate(out):
        if rid.startswith("{0}_"):
            if base:
                out[i] = (base + rid[3:], disp)
        elif not rid.startswith("{"):
            base = rid
    return out


def resources(pe, ann):
    """[(id, CONST)] in library order; the constants are hoisted as a block."""
    start = ann.syms["gml_Script_resource_library"]
    end = ann.idx.enclosing(start)[1]
    consts = []
    for insn in tkiw.disasm_function(pe, ann.md, start, end):
        for t in tkiw.rip_targets(pe, insn):
            v = ann.slots.get(t)
            if v and (v.startswith("RESOURCE_") or v.startswith("META_RESOURCE_")):
                if v not in consts:
                    consts.append(v)
    out = []
    for c in consts:
        meta = c.startswith("META_")
        rid = c.split("RESOURCE_", 1)[1].lower()
        out.append((rid, c, "meta" if meta else "run"))
    return out


def artifacts(loc, strings):
    """[(id, display, in_binary)] from localization, checked against the binary."""
    vals = set(strings.values())
    out = []
    for k, v in loc.items():
        if k.startswith("artifact_title_"):
            aid = k[len("artifact_title_"):]
            out.append((aid, v, aid in vals))
    out.sort()
    return out


def md_table(headers, rows):
    w = [len(h) for h in headers]
    for r in rows:
        for i, c in enumerate(r):
            w[i] = max(w[i], len(str(c)))
    line = lambda cs: "| " + " | ".join(str(c).ljust(w[i]) for i, c in enumerate(cs)) + " |"
    sep = "|" + "|".join("-" * (x + 2) for x in w) + "|"
    return "\n".join([line(headers), sep] + [line(r) for r in rows])


HEADER = """<!-- generated by analysis/extract.py -- do not edit by hand -->
# {title}

Recovered from the game build installed **2026-08-10**. Regenerate after any game update:

```bash
python analysis/extract.py
```

"""


def main():
    os.makedirs(DOCS, exist_ok=True)
    pe = tkiw.load()
    ann = Annotator(pe)
    loc = localization()

    # ---------------------------------------------------------- reward types
    rts = reward_types(pe, ann)
    path_of = {}
    for path, ids in CHOICE_PATHS.items():
        for i in ids:
            path_of[i] = path
    rows = []
    for rid, disp in rts:
        p = path_of.get(rid, "")
        if rid in OWN_SCREEN:
            kind = "**own screen**"
            scrap = "n/a"
        elif p and rid in NOT_A_CARD_PICK:
            kind = "**not a card pick**"
            scrap = "n/a"
        elif p:
            kind = "choice"
            scrap = "no" if p in NO_SCRAP_PATHS else "yes"
        else:
            kind = "direct grant"
            scrap = "n/a"
        rows.append((f"`{rid}`", disp or "—", kind, p or "—", scrap,
                     TYPE_NOTES.get(rid, "")))
    body = HEADER.format(title="Reward types")
    body += (f"{len(rts)} types, in `reward_library` order.\n\n"
             "**In scope for the auto-picker: the `choice` rows only.** `direct grant` rows have "
             "nothing to pick. `not a card pick` rows put up an open-ended screen instead of a "
             "set of alternative cards — a shop to browse, a prophecy board to arrange, a wheel "
             "to spin — and the mod refuses them rather than pretending they are a choice.\n\n"
             "`scrap` is whether the type offers a scrap button — only paths reaching "
             "`spawn_choice_unified` do. Note `resource` scraps only on its default branch: a "
             "queue entry carrying a custom resource list goes to "
             "`spawn_custom_resources_choice`, which has neither scrap nor reroll.\n\n"
             "Option cards are `obj_card_*` instances, each carrying its identity in a "
             "`<thing>_contained` member. (`obj_reward_option` is a *different* thing — the "
             "post-wave rewards bundle — and is not on the queue path at all.)\n\n")
    body += md_table(["id", "display", "kind", "choice path", "scrap", "notes"], rows) + "\n"
    open(os.path.join(DOCS, "reward-types.md"), "w", encoding="utf-8").write(body)

    # ------------------------------------------------------------- resources
    res = resources(pe, ann)
    rows = []
    for rid, const, scope in res:
        rows.append((f"`{rid}`", loc.get(f"resource_title_{rid}", "—"), scope, f"`{const}`"))
    run_ids = [r for r, _, s in res if s == "run"]
    mismatched = [(r, loc.get(f"resource_title_{r}", ""))
                  for r, _, s in res
                  if s == "run" and loc.get(f"resource_title_{r}", "").lower()
                  not in (r, r.replace("_", " "), r + "s")]
    body = HEADER.format(title="Resources")
    body += (f"{len(run_ids)} run resources plus {len(res) - len(run_ids)} meta-progression "
             "ones, in `resource_library` order. Ids are the constant suffix lowercased, "
             "confirmed against the `resources` map in a live save.\n\n"
             "The `meta` entries are the town/meta-progression currencies and share ids with "
             "run resources; they are a separate namespace and are **not** valid config keys "
             "for in-run resource rewards.\n\n"
             "Several run resources are biome- or level-specific and never appear in an "
             "ordinary run. `random` is a pseudo-resource the library carries as an entry.\n\n")
    if mismatched:
        body += ("Ids whose display name is not simply the id — the ones worth knowing before "
                 "writing a config:\n\n")
        for r, d in mismatched:
            body += f"- `{r}` displays as **{d}**\n"
        body += "\n"
    body += md_table(["id", "display", "scope", "constant"], rows) + "\n"
    open(os.path.join(DOCS, "resources.md"), "w", encoding="utf-8").write(body)

    # ------------------------------------------------------------- artifacts
    import strconsts
    arts = artifacts(loc, strconsts.load(pe))
    rows = [(f"`{a}`", d, "yes" if b else "**no**") for a, d, b in arts]
    missing = [a for a, _, b in arts if not b]
    body = HEADER.format(title="Artifacts")
    body += (f"{len(arts)} artifact ids, from `artifact_title_*` in `localization.csv`, "
             "cross-checked against the string-constant pool recovered from the executable.\n\n"
             "`in binary` = the id appears as a string constant. A **no** means the "
             "localization entry has no matching constant — most likely a cut or renamed "
             f"artifact. Currently {len(missing)} such.\n\n")
    body += md_table(["id", "display", "in binary"], rows) + "\n"
    open(os.path.join(DOCS, "artifacts.md"), "w", encoding="utf-8").write(body)

    # ---------------------------------------------------------- unit classes
    classes = []
    i = 0
    while f"unit_class_title_{i}" in loc:
        classes.append((i, loc[f"unit_class_title_{i}"]))
        i += 1
    body = HEADER.format(title="Unit classes and training stats")
    offered = list(OFFERED_CLASS_INDICES)
    n_opts = len(offered) * len(UNIT_STATS)
    intro = [
        "`unit_class_stat` (Troops Training) options are a **(unit class, stat)** pair.",
        "",
        "**{} classes offered x {} stats = {} possible options** -- not 8 x 3:".format(
            len(offered), len(UNIT_STATS), n_opts),
        "",
        "- **Attack speed is never offered.** It exists as a unit-class mod and other sources",
        "  grant it, but a Troops Training card is built from `stats = [0, 1]` and `card_setup`",
        "  has exactly two icon branches. Corroborated in a live save: after a run with 115 of",
        "  these queued, `training_atk_speed` was zero for all eight classes while damage and hp",
        "  carried large values. A config naming `*.attack_speed` should be rejected at load",
        "  rather than silently never matching.",
        "- **Undead is never offered.** `possible_unit_classes` is a literal seven-entry array,",
        "  shuffled before each offer. The class exists and other sources modify it; it just",
        "  never appears on a card.",
        "",
        "On the card, `class_stat_bonuses_contained` is an **array** of",
        "`{stat_type, unit_class, stat_amount}` -- one card can carry more than one bonus, so",
        "treat it as length N, not 1. Both fields are numeric and the game emits them as",
        "**int64** (RValue kind 10), not doubles, so a reader must accept kinds 0, 7 and 10 and",
        "normalise or it will silently fail to match.",
        "",
        "**Class identity is positional.** The save keys training by class index and no class-id",
        "string exists -- the titles below are display text. A game update that reorders or",
        "inserts a class silently changes what a config key means, which is why the mod refuses",
        "this type unless the class count still matches {}.".format(len(classes)),
        "",
    ]
    body += "\n".join(intro) + "\n"
    rows = [(i, d, "`{}`".format(d.lower()), "yes" if i in offered else "**never offered**")
            for i, d in classes]
    body += md_table(["index", "display", "config key prefix", "offered?"], rows) + "\n\n"
    body += "Stats: " + ", ".join("`{}` (stat_type {})".format(n, v) for v, n in UNIT_STATS)
    body += (". `stat_type 0` maps to `UNIT_CLASS_MODS_HP` and `1` to `UNIT_CLASS_MODS_DAMAGE`"
             " in `unit_class_mod_change`.\n\n"
             "So a config key `ranged.damage` resolves to `(unit_class 3, stat_type 1)`.\n")
    open(os.path.join(DOCS, "unit-classes.md"), "w", encoding="utf-8").write(body)

    # ------------------------------------- option vocabularies, from live data
    live_path = os.path.join(DOCS, "live-libraries.json")
    live = {}
    if os.path.isfile(live_path):
        with open(live_path, encoding="utf-8") as fh:
            live = json.load(fh)
    if live:
        body = HEADER.format(title="Option vocabularies")
        body += (
            "The ids a reward type's options are drawn from -- the keys of the game's own "
            "libraries, read out of the running game by walking their `ds_map` buckets "
            "(`analysis/verify_live.py`). These are what a config section for the "
            "corresponding reward type is written against.\n\n"
            "Display names come from `localization.csv`. An id with **no title** is usually "
            "debug or cut content that the player will never be offered; it is listed anyway, "
            "because the config is required to be complete and the mod will refuse to "
            "auto-resolve a choice containing an id it does not know.\n\n")
        for lib, prefix, types in [
            ("SPELLS", "spell_title_", "`spell`, `spell_legend`"),
            ("IMPROVEMENTS", "improv_title_", "the 13 `improvement_*` types"),
            ("UPGRADES", "upgrade_title_", "`upgrade`"),
            ("ADVISORS", "advisor_title_", "advisor rewards"),
            ("UNITS", "unit_title_", "unit rewards"),
        ]:
            keys = live.get(lib) or []
            if not keys:
                continue
            untitled = [k for k in keys if f"{prefix}{k}" not in loc]
            body += f"\n## {lib.title()} — {len(keys)} ids\n\n"
            body += f"Used by: {types}."
            if untitled:
                body += f" {len(untitled)} without a display name."
            body += "\n\n"
            rows = [(f"`{k}`", loc.get(f"{prefix}{k}", "—")) for k in keys]
            body += md_table(["id", "display"], rows) + "\n"
        open(os.path.join(DOCS, "option-vocabularies.md"), "w", encoding="utf-8").write(body)

    # ------------------------------------------------------------------ json
    blob = {
        "build_analysed": "2026-08-10",
        "reward_types": [
            {"id": r, "display": d, "choice_path": path_of.get(r),
             "can_scrap": (path_of.get(r) is not None
                           and path_of[r] not in NO_SCRAP_PATHS)}
            for r, d in rts],
        "resources": [{"id": r, "display": loc.get(f"resource_title_{r}"),
                       "constant": c, "scope": s} for r, c, s in res],
        "artifacts": [{"id": a, "display": d, "in_binary": b} for a, d, b in arts],
        "unit_classes": [{"index": i, "display": d} for i, d in classes],
        "unit_stats": [{"stat_type": v, "name": n} for v, n in UNIT_STATS],
        "offered_class_indices": OFFERED_CLASS_INDICES,
    }
    with open(os.path.join(DOCS, "vocabulary.json"), "w", encoding="utf-8") as fh:
        json.dump(blob, fh, indent=2)

    print(f"wrote {DOCS}")
    print(f"  reward-types.md  {len(rts)} types")
    print(f"  resources.md     {len(res)} entries")
    print(f"  artifacts.md     {len(arts)} artifacts ({len(missing)} not in binary)")
    print(f"  unit-classes.md  {len(classes)} classes x {len(UNIT_STATS)} stats")
    print(f"  vocabulary.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
