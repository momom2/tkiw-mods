//! The wide diagnostic pass.
//!
//! Built to answer everything still open from one play session, because the
//! next opportunity to observe the running game may be hours away. Strictly
//! read-only, and every read is validated -- nobody will be around to recover a
//! crashed game, so this must not be able to cause one.
//!
//! What it is trying to settle:
//!
//! * **[TBD-6]** whether an option card's identity is cleanly readable, which
//!   decides which reward types can be automated at all
//! * **[TBD-10.1]** whether picking an option opens a *second* choice, which is
//!   the thing that could still change the config format
//! * **[TBD-3]** which reward types actually offer a scrap button, observed
//!   rather than inferred from which spawn path installs one
//! * the live contents of the game's own libraries, to cross-check the option
//!   id vocabularies the config will be written against

use crate::instance;
use crate::rvalue::Value;
use crate::{dslist, logln, State};
#[allow(unused_imports)]
use crate::builtin;

/// The option cards a *queued* reward presents, and the member on each that
/// carries the option's identity.
///
/// An earlier version of this watched `obj_reward_option`, which was wrong:
/// that object belongs to `obj_rewards_bundle`, the post-wave claim panel.
/// `setup_reward_option` has exactly two referencing functions, and neither is
/// on the queue path. A queue entry produces `obj_card_*` instances instead.
///
/// The identity member is named by a consistent convention -- `<thing>_contained`
/// -- across every card type.
pub const CARD_OBJECTS: &[(&str, &str)] = &[
    ("obj_card_resource", "resource_contained"),
    ("obj_card_artifact", "artifact_contained"),
    ("obj_card_spell", "spell_contained"),
    ("obj_card_improvement", "improvement_contained"),
    ("obj_card_upgrade", "upgrade_contained"),
    ("obj_card_start_bonus", "start_bonus_packs_contained"),
    ("obj_card_class_stat_bonus", "class_stat_bonuses_contained"),
];

/// Reward-related objects worth knowing the presence of. Which of these exist
/// while a choice is open says what UI the game put up, and therefore what the
/// player could have done.
pub const UI_OBJECTS: &[&str] = &[
    "obj_card_resource",
    "obj_card_artifact",
    "obj_card_spell",
    "obj_card_improvement",
    "obj_card_upgrade",
    "obj_card_start_bonus",
    "obj_card_class_stat_bonus",
    "obj_card_info",
    "obj_button_reroll_cards",
    "obj_button_rewards_scrap",
    "obj_button_banish_reward",
    "obj_button_rewards_skip",
    "obj_button_rewards_proceed",
    "obj_button_reward_queue",
    // screens that are NOT a single-card pick, and must never be automated
    "obj_shop",
    "obj_shop_graveyard",
    "obj_prophecy",
    "obj_rewards_wheel",
    // the bundle path, for contrast: a claim list, not the reward queue
    "obj_rewards_bundle",
    "obj_reward_option",
];

/// Candidate members of an option card. Drawn from the static analysis of
/// `obj_reward_option` and the per-type builders inside `anon@928`; the point
/// is to find which actually carry identity.
const OPTION_VARS: &[&str] = &[
    // The player confirms the units-affected preview matches in-game state,
    // whereas the goblin sprite is a long-standing display bug and must NOT be
    // used as a cross-check.
    "total_units_affected",
    "resource_amount",
    "rarity",
    "class",
    "stat_index",
    "amount",
    "name",
    "reward",
    "reward_type",
    "banish_button",
    "select_button",
];

/// Candidate members of the reroll button, to pin down the live cost model.
const REROLL_VARS: &[&str] = &[
    "reward_type",
    "reward_tier",
    "reward_quantity",
    "options_amount",
    "free_rerolls_per_reward_left",
    "non_free_rerolls_made",
    "cost",
    "reroll_cost",
    "current_cost",
    "price",
    "max_rerolls",
    "extra_rerolls",
    "cost_increase_per_reroll",
    "can_afford",
    "is_free",
];

fn member(state: &State, inst: usize, name: &str) -> Option<Value> {
    let id = state.syms.var_id(name)?;
    let rv = unsafe { instance::get_var(inst, id) }?;
    crate::rvalue::decode(rv)
}

/// Render only members that are actually present, so absence is visible.
fn members(state: &State, inst: usize, names: &[&str]) -> String {
    let mut parts = Vec::new();
    for n in names {
        match member(state, inst, n) {
            None => {}
            Some(Value::Other { kind: 0xFF_FFFF, .. }) => {} // unset: not on this instance
            Some(v) => parts.push(format!("{n}={}", crate::summarise(&v))),
        }
    }
    if parts.is_empty() {
        "<no known members>".to_string()
    } else {
        parts.join(" ")
    }
}

/// The option's identity, which is **two hops**, not one.
///
/// `X_contained` holds the *library element struct*, not the id — every
/// `assign_reward` is a one-liner `X_contained = argument0` where the argument
/// came from `ds_map_find_value(<LIBRARY>, key)`. The id is that map key, and
/// it lives on the struct as `system_name`; the game's own `equip_*` functions
/// take `X_contained.system_name` and look it straight back up in the library.
///
/// Two card types are arrays instead, holding several bonuses rather than one
/// named thing, and are described element-wise.
/// Whether a card has finished being set up.
///
/// Cards exist for several frames before they are populated: sampling on the
/// frame they appear yields placeholder values -- `select_button` still
/// `noone` (-4), `total_units_affected` still 0 -- and, for Troops Training,
/// a uniform `stat_type` across all three that does not match the screen.
/// Reading those and reporting them as the offer was the cause of a wrong
/// answer given three times.
///
/// `select_button` becoming a real instance reference is the signal that the
/// card is built.
pub fn card_ready(state: &State, card: usize) -> bool {
    matches!(
        member(state, card, "select_button"),
        Some(Value::Ref { ref_type: 0x0400_0001, .. })
    )
}

fn describe_identity(state: &State, card: usize, identity: &str) -> String {
    let Some(v) = member(state, card, identity) else {
        return format!("{identity}=<absent>");
    };
    match &v {
        Value::Object(_) => {
            let name = state
                .syms
                .var_id("system_name")
                .and_then(|id| unsafe { dslist::struct_member(&v, id) });
            match name.as_ref().and_then(|n| n.as_str().map(str::to_owned)) {
                Some(s) => format!("{identity}.system_name={s:?}"),
                None => format!("{identity}=struct, but system_name unreadable"),
            }
        }
        Value::Array(p) => format!("{identity}=array @{p:#x} {}", array_fields(state, &v)),
        other => format!("{identity}={}", crate::summarise(other)),
    }
}

/// For the two array-shaped cards, describe the elements.
///
/// `class_stat_bonuses_contained` is `[{stat_type, unit_class, stat_amount}]`,
/// and a card can hold more than one. `array_length` reads the count from
/// `payload + 0x24`; the element pointer's offset is not yet established, so
/// candidates are probed and the header dumped once if none works -- the same
/// approach that settled the string layout.
fn array_fields(state: &State, v: &Value) -> String {
    let Value::Array(payload) = v else { return String::new() };
    let payload = *payload;
    let Some(len) = crate::rvalue::read_i32(payload + crate::rvalue::ARRAY_LEN) else {
        return "(length unreadable)".to_string();
    };
    if !(0..4096).contains(&len) {
        return format!("(implausible length {len})");
    }

    // The element block is a contiguous run of 16-byte RValues. Try the
    // plausible offsets and keep the first whose first element decodes to
    // something a bonus struct could be.
    for off in crate::rvalue::ARRAY_DATA_CANDIDATES {
        let Some(items) = crate::rvalue::read_usize(payload + off) else { continue };
        if items == 0 || !crate::win::readable(items, (len.max(1) as usize) * 16) {
            continue;
        }
        // A readable pointer is not evidence: `decode` succeeds on almost any
        // mapped bytes, so accepting the first candidate that reads would
        // happily report garbage. These arrays hold structs, so require the
        // elements to actually decode as structs before believing the offset.
        let mut elems = Vec::new();
        let mut all_structs = true;
        for i in 0..len.min(8) {
            match crate::rvalue::decode(items + i as usize * 16) {
                Some(e @ Value::Object(_)) => elems.push(describe_bonus(state, &e)),
                _ => {
                    all_structs = false;
                    break;
                }
            }
        }
        if all_structs && !elems.is_empty() {
            return format!("len={len} @+{off:#x} [{}]", elems.join("; "));
        }
    }
    format!(
        "len={len} (at +{:#x}), no candidate offset held structs. header:{}",
        crate::rvalue::ARRAY_LEN,
        crate::rvalue::dump(payload, 0, 96)
    )
}

/// One element of a card's contained array.
///
/// **Does not assume the member names.** An earlier version read `unit_class`,
/// `stat_type` and `stat_amount` by name, got plausible small integers back,
/// and reported them confidently -- but they did not match what the player saw
/// on screen. Reading a guessed name off a struct returns something whether or
/// not it is the field that means what you think, so a wrong guess is
/// indistinguishable from a right one.
///
/// So: enumerate what the struct really has, and print every member.
/// The fields of a bonus element, by name.
///
/// These names were *recovered* from the game with `variable_struct_get_names`
/// and then confirmed against a screenshot, so using them directly is no longer
/// a guess. Enumerating them per call is not: that builtin allocates a GML
/// array of strings and the result is never freed, which is a leak in the
/// player's game and milliseconds of cost when it runs per card per poll.
const BONUS_FIELDS: &[&str] = &["stat_type", "unit_class", "stat_amount",
                                "bonus_type", "bonus_item", "amount"];

fn describe_bonus(state: &State, e: &Value) -> String {
    if !matches!(e, Value::Object(_)) {
        return crate::summarise(e);
    }
    let mut parts = Vec::new();
    for f in BONUS_FIELDS {
        if let Some(id) = state.syms.var_id(f) {
            if let Some(val) = unsafe { dslist::struct_member(e, id) } {
                if !matches!(val, Value::Other { kind: 0xFF_FFFF, .. }) {
                    parts.push(format!("{f}={}", crate::summarise(&val)));
                }
            }
        }
    }
    if parts.is_empty() { crate::summarise(e) } else { parts.join(" ") }
}

/// A stable fingerprint of what is on screen, so changes can be detected
/// without logging every poll.
pub fn ui_counts(base: usize) -> Vec<usize> {
    UI_OBJECTS.iter().map(|o| instance::count(base, o)).collect()
}

pub fn fingerprint(counts: &[usize]) -> String {
    counts.iter().map(usize::to_string).collect::<Vec<_>>().join(",")
}

/// Everything about the choice currently on screen.
pub fn describe_choice(state: &State, base: usize, counts: &[usize]) -> Vec<String> {
    let mut out = Vec::new();

    // counts come from the caller's single enumeration pass: walking the
    // object lists twice per poll was half the cost of the whole poll
    let present: Vec<String> = UI_OBJECTS
        .iter()
        .zip(counts)
        .filter(|(_, &n)| n > 0)
        .map(|(o, n)| format!("{}x{n}", o.trim_start_matches("obj_")))
        .collect();
    out.push(format!("  ui: {}", if present.is_empty() {
        "<nothing>".to_string()
    } else {
        present.join(" ")
    }));

    // [TBD-6] the option cards, and whether their identity is readable
    for (obj, identity) in CARD_OBJECTS {
        for (i, card) in instance::find_all(base, obj).iter().enumerate() {
            let ready = card_ready(state, *card);
            let name = obj.trim_start_matches("obj_card_");
            // A card that is not ready is one the game is still writing. The
            // picker has always waited for `card_ready`; the survey did not,
            // and went reading the arrays it hangs off -- printing them under
            // the word "building" without pausing on what that meant. It is
            // the same mistake as reading a card during teardown, at the other
            // end of its life, and the log shows it happening moments before
            // the game died.
            if !ready {
                out.push(format!("  {name}[{i}] building @{card:#x} - not read"));
                continue;
            }
            out.push(format!(
                "  {name}[{i}] READY @{card:#x} {}  |  {}",
                describe_identity(state, *card, identity),
                members(state, *card, OPTION_VARS)
            ));
        }
    }

    // the reroll economy, live
    for btn in instance::find_all(base, "obj_button_reroll_cards") {
        out.push(format!("  reroll @{btn:#x} {}", members(state, btn, REROLL_VARS)));
    }

    // [TBD-3] scrap availability, observed rather than inferred
    let idx = |name: &str| UI_OBJECTS.iter().position(|o| *o == name)
        .and_then(|i| counts.get(i).copied()).unwrap_or(0);
    let scrap = idx("obj_button_rewards_scrap");
    let banish = idx("obj_button_banish_reward");
    out.push(format!("  scrap_buttons={scrap} banish_buttons={banish}"));

    out
}

/// One-shot: dump the game's own libraries, to cross-check the id vocabularies
/// the config file will be written against.
pub fn dump_libraries(state: &State, base: usize, g: &crate::globals::Globals) {
    logln!("---- game libraries (live) ----");
    for name in [
        "REWARDS",
        "RESOURCES",
        "ARTIFACTS",
        "SPELLS",
        "IMPROVEMENTS",
        "UNITS",
        "UNIT_CLASSES",
        "UPGRADES",
        "ADVISORS",
        // The game's own groupings. These are what split the 131 eligible
        // improvements across the 12 improvement reward types -- deriving that
        // split any other way would be inference, and inference has been wrong
        // every time it was tried here.
        "IMPROVEMENTS_BY_CATEGORY",
        "IMPROVEMENTS_BY_TIER",
        "FILTERED_IMPROVEMENTS_REWARDS",
        "IMPROVEMENTS_AVAILABLE_POOL",
    ] {
        let Some(id) = state.syms.var_id(name) else {
            logln!("  {name}: no variable id");
            continue;
        };
        let Some(v) = (unsafe { g.get(id) }) else {
            logln!("  {name}: unreadable");
            continue;
        };
        if let Value::Array(_) = v {
            logln!("  {name}: array of {}", array_strings(&v).len());
            for chunk in array_strings(&v).chunks(8) {
                logln!("      {}", chunk.join(", "));
            }
            continue;
        }
        match dslist::ds_map_entries(base, &v, 4096) {
            None => logln!("  {name}: {} (not a walkable ds_map)", crate::summarise(&v)),
            Some(entries) => {
                let mut keys: Vec<String> = entries
                    .iter()
                    .map(|(k, _)| k.as_str().map(str::to_owned).unwrap_or_else(|| format!("{k:?}")))
                    .collect();
                keys.sort();
                logln!("  {name}: {} entries", keys.len());
                for chunk in keys.chunks(8) {
                    logln!("      {}", chunk.join(", "));
                }

                // Which of these a given reward type can actually offer is a
                // *filtered subset* -- legendary artifacts are not ordinary
                // ones, and the improvement pool varies with the equipped king
                // and the game stage. Building a config section from the whole
                // library is therefore wrong. Dump each element's real fields
                // so the filter can be derived from the game rather than
                // guessed at again.
                dump_element_fields(state, base, name, &entries);
            }
        }
    }
}

/// The fields that decide whether a library entry can be offered as a reward,
/// and by which reward type. Recovered by enumerating a real entry's members.
const FILTER_FIELDS: &[&str] = &[
    "system_name",
    "tier",
    "unlocked",
    "category",
    "rarity",
    "level",
    "excluded_from_drop_pool",
    "upgrade_of",
    "main_product",
];

/// Dump the filtering fields of **every** library entry.
///
/// A reward type offers a filtered subset of its library -- legendary artifacts
/// are not ordinary ones, and the improvement pool depends on the equipped king
/// and the game stage. Deriving those subsets needs the entries' own fields, so
/// they are dumped wholesale and the per-type lists are worked out offline from
/// real data rather than from inference.
///
/// Emitted one entry per line, machine-readable, for `analysis/derive_pools.py`.
fn dump_element_fields(state: &State, _base: usize, library: &str, entries: &[(Value, Value)]) {
    let mut n = 0;
    for (k, v) in entries {
        // A map whose values are arrays is a *grouping* -- which improvements
        // belong to which category or tier. That is exactly the split the
        // improvement reward types need, so dump the members.
        if let Value::Array(_) = v {
            let members = array_strings(v);
            logln!("    GROUP {library} {} | {}", key_of(k), members.join(","));
            n += 1;
            continue;
        }
        if !matches!(v, Value::Object(_)) {
            continue;
        }
        let key = k.as_str().unwrap_or("?");
        let mut parts = Vec::new();
        for f in FILTER_FIELDS {
            if let Some(id) = state.syms.var_id(f) {
                if let Some(val) = unsafe { dslist::struct_member(v, id) } {
                    if !matches!(val, Value::Other { kind: 0xFF_FFFF, .. }) {
                        parts.push(format!("{f}={}", crate::summarise(&val)));
                    }
                }
            }
        }
        logln!("    FIELD {library} {key} | {}", parts.join(" "));
        n += 1;
    }
    logln!("    ({n} entries dumped for {library})");
}


/// Key of a map entry, however it is typed.
fn key_of(k: &Value) -> String {
    match k {
        Value::Str(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Real(r) => format!("{r}"),
        other => format!("{other:?}"),
    }
}

/// The string elements of an array RValue.
pub fn array_strings(v: &Value) -> Vec<String> {
    let Value::Array(payload) = v else { return Vec::new() };
    let Some(len) = crate::rvalue::read_i32(*payload + crate::rvalue::ARRAY_LEN) else {
        return Vec::new();
    };
    if !(0..4096).contains(&len) {
        return Vec::new();
    }
    let Some(items) = crate::rvalue::read_usize(*payload + crate::rvalue::ARRAY_DATA) else {
        return Vec::new();
    };
    if items == 0 || !crate::win::readable(items, len as usize * 16) {
        return Vec::new();
    }
    (0..len as usize)
        .filter_map(|i| crate::rvalue::decode(items + i * 16))
        .map(|v| match v {
            Value::Str(s) => s,
            other => format!("{other:?}"),
        })
        .collect()
}
