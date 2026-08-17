#!/usr/bin/env python3
"""
Flatten the UNITS library into one CSV row per unit, split three ways:

    units-player.csv    faction 0
    units-enemies.csv   faction 1, no boss markers
    units-bosses.csv    faction 1 with boss markers (boss_mods et al. -- every
                        marker agrees, and every has_super_action unit is one)

Rows are sorted by display name. Several bosses share one display name across
forms ("Forest Lord" three times); the `name` column is the unambiguous id.

    python units_table.py                # writes into ../../tkiw-momomod-kit/analysis/
    python units_table.py --json X --outdir Y

Input is `libraries.json`, written by the kit's `dump_libraries` diagnostic from
the live game (menu is enough; no run needed). Regenerate that first when the
game updates.

## Derived columns, and what they rest on

Formulas from `stat-formulas.md`, where they are verified against the in-game
display:

    atk_per_s   = 60 / attack_time              (attack_time is frames at 60fps)
    dps         = attack_damage / (attack_time / 60)
    hit_delay_s = attack_action_frame[0] / attack_img_speed / 60

`dps` was verified on single-hit units (griffin, mage_lightning). Units with
several entries in `attack_action_frame` (the `hits` column) may deal
attack_damage per action frame; whether the in-game figure multiplies by hits
is NOT verified, so `dps` here deliberately does not. `hit_delay_s` follows
`gameplay-features.md` ("the frame the hit lands on") and has not been checked
against a stopwatch.

Values are **base**: training, upgrades and morale modify them at runtime as
`base x (1 + sum(percent)) + flat` — see `stat-formulas.md`.

## The specials column

Everything a unit carries that most units do not, minus presentation plumbing
(sprites, sounds, animation speeds, localisation keys). That long tail of
one-off fields — `meteorite_damage`, `skeletons_spawned`, `burrow_hp_threshold`
— is exactly what makes a boss a boss, so it is kept verbatim as `key=value`
pairs rather than forced into columns.
"""
import argparse
import collections
import csv
import json
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_JSON = os.path.join(HERE, "..", "..", "tkiw-momomod-kit", "libraries.json")
DEFAULT_OUTDIR = os.path.join(HERE, "..", "..", "tkiw-momomod-kit", "analysis")
PARAMS_DB = os.path.join(HERE, "describe_params.json")

# The trait fields the shared bonus-trait templates feed into {&1}..{&3}, in
# token order -- recovered by describe_params.py from Unit_trait_class_bonus.
TEMPLATE_FIELDS = ("stat_bonus_per_unit", "unit_class_buffed", "max_units_giving_buff")

# Trait fields that are bookkeeping, not description parameters.
TRAIT_NOISE = {
    "is_updating", "mod_type", "replace_title_parameters_array", "icon_sprite",
    "unit_stats_buffed",
}

# A unit is a boss if it carries any of these. Checked against the data: the
# markers agree with each other, none appears on faction 0, and every
# has_super_action unit is covered.
BOSS_MARKERS = ("boss_intro_name", "icon_boss_sprite", "BI_face_sprite",
                "hp_percent_thresholds", "boss_mods")

# Fields that are presentation or plumbing, not gameplay. Kept out of specials.
NOISE_PREFIXES = ("get_", "push_", "populate_", "replace_", "parent_")
NOISE_EXACT = {
    "system_name", "system_name_full", "parameters_description_handle",
    "specific_description", "exclude_from_tiers_map", "dead_body_type",
    "hp_bar_type", "boss_intro_name", "has_intro", "attack_sounds",
    "shadow_width", "shadow_offset", "shadow_height", "shadow_alpha",
    "add_localized_parameter",
}


def is_noise(key, value):
    if key.startswith(NOISE_PREFIXES) or key in NOISE_EXACT:
        return True
    if "sprite" in key or "sound" in key:
        return True
    if key.endswith(("_img_speed", "_img_speeds", "_key", "_default_value", "_order")):
        return True
    if "description" in key or key.endswith(("_title", "_tips")) or key.startswith(("title_", "tips_")):
        return True
    # animation frame lists of the extra actions; attack_action_frame is used directly
    if key.endswith(("_frames", "_action_frame", "_action_frames")) and key != "attack_action_frame":
        return True
    if value == "<struct>" or value == "<ref>" or value == "<undefined>":
        return True
    return False


def class_names(lib):
    """index -> name, recovered from each class's text_color_tag rather than
    kept as a second list here."""
    out = {}
    for key, entry in lib.get("UNIT_CLASSES", {}).items():
        tag = entry.get("text_color_tag", "")
        m = re.search(r"unit_class_(\w+)\]", tag)
        try:
            out[int(key)] = m.group(1) if m else key
        except ValueError:
            pass
    return out


def fmt(v):
    if isinstance(v, bool):
        return "yes" if v else "no"
    if isinstance(v, float) and v == int(v):
        return str(int(v))
    if isinstance(v, list):
        return "+".join(fmt(x) for x in v)
    if isinstance(v, dict):
        return "<struct>"
    return str(v)


def param_entries(name, u, classes, params_db):
    """Best-effort `{&N}` sources for a unit's dynamic texts, in token order.

    Three tiers, in order of confidence:
    * the unit's own parameter methods, recovered statically by
      `describe_params.py` -- shown as the fields each method reads, with the
      unit's actual values substituted where the field is in the dump;
    * a bonus-trait built by the shared `Unit_trait_class_bonus` family --
      instantiated from the trait's own fields in template order;
    * any other trait: its gameplay-numeric fields in declaration order,
      which observation says usually tracks token order.
    """
    t = u.get("trait") if isinstance(u.get("trait"), dict) else {}

    def with_value(field):
        v = u.get(field, t.get(field))
        if v is None or isinstance(v, (dict, str)):
            return field
        if field == "unit_class_buffed" and isinstance(v, (int, float)):
            return f"{field}={classes.get(int(v), v)}"
        return f"{field}={fmt(v)}"

    def is_field(w):
        return "=" not in w and w == w.lower() and (w in u or w in t)

    # Presentation plumbing a formula may mention but a reader never wants.
    drop = {"STRING_EMPTY", "id"}

    tokens = len(set(re.findall(r"\{&(\d+)\}", str(t.get("description_default_value", "")))))

    direct = params_db.get(name)
    if direct and direct.get("params"):
        formulas = [
            [with_value(w) if is_field(w) else w
             for w in f.split(" ")
             if w not in drop and "sprite" not in w]
            for f in direct["params"]
        ]
        # One method serving several tokens computes them in token order, so
        # split its word list at each field mention.
        if len(formulas) == 1 and tokens > 1:
            buckets, cur = [], []
            for w in formulas[0]:
                if "=" in w or is_field(w):
                    if cur and any("=" in x or is_field(x) for x in cur):
                        buckets.append(cur)
                        cur = []
                cur.append(w)
            if cur:
                buckets.append(cur)
            formulas = buckets
        return [" ".join(f) for f in formulas if f]

    if not t or tokens == 0:
        return []
    if all(f in t for f in TEMPLATE_FIELDS[:1]):  # a class/tag/same-unit bonus trait
        return [with_value(f) for f in TEMPLATE_FIELDS if f in t]
    return [
        f"{k}={fmt(v)}"
        for k, v in t.items()
        if isinstance(v, (int, float)) and not isinstance(v, bool)
        and k not in TRAIT_NOISE and "sprite" not in k
    ][:3]


def row_for(name, u, classes, universal, params_db):
    dmg = u.get("attack_damage")
    at = u.get("attack_time")
    # a single action frame is stored bare on some units, as a list on others
    aaf = u.get("attack_action_frame")
    if isinstance(aaf, (int, float)):
        aaf = [aaf]
    aaf = aaf or []
    imgspd = u.get("attack_img_speed")

    atk_per_s = round(60.0 / at, 3) if at else None
    dps = round(dmg / (at / 60.0), 2) if dmg is not None and at else None
    hit_delay = (
        round(aaf[0] / imgspd / 60.0, 3)
        if aaf and imgspd and isinstance(aaf[0], (int, float))
        else None
    )

    trait = u.get("trait")
    trait_text = ""
    if isinstance(trait, dict):
        trait_text = trait.get("description_default_value", "") or "<trait>"
    params = param_entries(name, u, classes, params_db)

    used = {
        "attack_damage", "attack_time", "attack_action_frame", "attack_img_speed",
        "hp_max", "attack_radius", "constant_attack_radius", "engage_speed",
        "weight", "tier", "faction", "tags", "classes", "unreleased", "trait",
        "is_attacker", "is_caster", "cast_cooldown_time",
        "charge_damage_min", "charge_damage_max",
        "has_special_attack", "has_super_action", "is_push_immune",
    }
    specials = []
    for k in sorted(u):
        if k in used or k in universal or is_noise(k, u[k]):
            continue
        specials.append(f"{k}={fmt(u[k])}")

    charge = ""
    if "charge_damage_min" in u or "charge_damage_max" in u:
        charge = f"{fmt(u.get('charge_damage_min', '?'))}-{fmt(u.get('charge_damage_max', '?'))}"

    return {
        "name": name,
        "title": u.get("title_default_value", ""),
        "faction": {0: "player", 1: "enemy"}.get(u.get("faction"), u.get("faction")),
        "tier": u.get("tier", ""),
        "unreleased": "yes" if u.get("unreleased") else "",
        "tags": "+".join(u.get("tags") or []),
        "classes": "+".join(classes.get(c, str(c)) for c in (u.get("classes") or [])),
        "hp": fmt(u["hp_max"]) if "hp_max" in u else "",
        "dmg_per_hit": fmt(dmg) if dmg is not None else "",
        "hits": len(aaf) if aaf else "",
        "atk_time_frames": fmt(at) if at is not None else "",
        "atk_per_s": atk_per_s if atk_per_s is not None else "",
        "dps": dps if dps is not None else "",
        "hit_delay_s": hit_delay if hit_delay is not None else "",
        "range": fmt(u["attack_radius"]) if "attack_radius" in u else "",
        "range_constant": fmt(u["constant_attack_radius"]) if "constant_attack_radius" in u else "",
        "engage_speed": fmt(u["engage_speed"]) if "engage_speed" in u else "",
        "weight": fmt(u["weight"]) if "weight" in u else "",
        "caster": fmt(u["is_caster"]) if "is_caster" in u else "",
        "cast_cooldown_frames": fmt(u["cast_cooldown_time"]) if "cast_cooldown_time" in u else "",
        "charge_dmg": charge,
        "special_attack": fmt(u["has_special_attack"]) if "has_special_attack" in u else "",
        "super_action": fmt(u["has_super_action"]) if "has_super_action" in u else "",
        "push_immune": fmt(u["is_push_immune"]) if "is_push_immune" in u else "",
        "trait": trait_text,
        "param_1": params[0] if len(params) > 0 else "",
        "param_2": params[1] if len(params) > 1 else "",
        "param_3": " | ".join(params[2:]) if len(params) > 2 else "",
        "specials": "; ".join(specials),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", default=DEFAULT_JSON)
    ap.add_argument("--outdir", default=DEFAULT_OUTDIR)
    args = ap.parse_args()

    with open(args.json, encoding="utf-8") as fh:
        d = json.load(fh)
    units = d["UNITS"]
    classes = class_names(d)

    freq = collections.Counter()
    for u in units.values():
        for k in u:
            freq[k] += 1
    universal = {k for k, c in freq.items() if c == len(units)}

    params_db = {}
    if os.path.exists(PARAMS_DB):
        with open(PARAMS_DB, encoding="utf-8") as fh:
            params_db = json.load(fh)
    else:
        print("note: no describe_params.json - param columns will use trait fields only")

    split = {"units-player.csv": [], "units-enemies.csv": [], "units-bosses.csv": []}
    for name, u in units.items():
        row = row_for(name, u, classes, universal, params_db)
        if u.get("faction") == 0:
            split["units-player.csv"].append(row)
        elif any(m in u for m in BOSS_MARKERS):
            split["units-bosses.csv"].append(row)
        else:
            split["units-enemies.csv"].append(row)

    os.makedirs(args.outdir, exist_ok=True)
    for filename, rows in split.items():
        # Display name first, system name as the tiebreaker for shared titles.
        rows.sort(key=lambda r: ((r["title"] or r["name"]).lower(), r["name"]))
        path = os.path.join(args.outdir, filename)
        with open(path, "w", newline="", encoding="utf-8") as fh:
            w = csv.DictWriter(fh, fieldnames=list(rows[0].keys()))
            w.writeheader()
            w.writerows(rows)
        print(f"{len(rows):4} rows -> {path}")


if __name__ == "__main__":
    main()
