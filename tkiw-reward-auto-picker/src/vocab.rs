//! Building the config's option lists from the running game.
//!
//! A reward type offers a *filtered subset* of its library, not the whole
//! thing: `artifact_legend` offers only legendary artifacts, and
//! `improvement_production_t1` only tier-1 production buildings. The filters are
//! fields on the library entries themselves, and the improvement split is the
//! game's own `IMPROVEMENTS_BY_CATEGORY` / `_BY_TIER` grouping.
//!
//! Doing this here rather than in a Python script means a player needs nothing
//! but the mod, and the lists describe *their* game rather than the one it was
//! developed against.
//!
//! The rules, stated once so they can be argued with:
//!
//! * If an id could be offered under **some** run -- any king, any level, any
//!   stage -- it is included. Leaving one out makes the mod refuse a real
//!   choice, which is worse than a spare line in a file.
//! * If it could **never** be offered for that type, it is left out. Every
//!   extra line makes the file harder to read.
//! * `unlocked = false` entries are kept: that is meta-progression state, so it
//!   means "not earned yet", not "impossible".
//! * Biome-gated entries are kept: they cannot appear in every run, but they can
//!   appear in some.

use std::collections::HashMap;

use crate::config::TypeVocab;
use crate::rvalue::Value;
use crate::{dslist, globals::Globals, State};

/// Tier 1 is ordinary, tier 2 is legendary -- for both artifacts and spells.
const TIER_ORDINARY: i64 = 1;
const TIER_LEGENDARY: i64 = 2;

/// Improvement categories, from the game's own grouping. Categories 4 and 5
/// (special and terrain) are wholly `excluded_from_drop_pool`, which is what
/// confirms the mapping.
const CAT_PRODUCTION: &str = "0";
const CAT_TROOPS: &str = "1";
const CAT_ATTACKING: &str = "2";
const CAT_MISC: &str = "3";

/// How the eight unit classes are titled in the config's comments. The *ids*
/// come from [`crate::resolve::UNIT_CLASSES`], because a card names its class by
/// index into that list -- so the config must use the same spellings or nothing
/// the mod reads off a card will match anything the player wrote.
const CLASS_TITLES: [&str; 8] = [
    "Grunt", "Rider", "Flying", "Ranged", "Arcane", "Warrior", "Champion", "Undead",
];

/// Said of every improvement list, since they all come from the same place.
const GROUPED: &str = "from the game's own category/tier grouping, minus what it                        marks as never dropping";
const INFERNAL_NOTE: &str = "the `imp_` members of the troops category";

/// Every improvement reward type, with the slice of the game's own grouping it
/// draws from.
///
/// **This is the one place these names are written.** `resolve` matches an open
/// improvement reward against it to pick a config section, and the generator
/// builds those sections from it. They were once two lists that had to agree,
/// nothing checked that they did, and they silently did not: the generator
/// wrote ten `improvement_*` sections while the resolver looked for a single
/// `[improvement]`, so no building reward was ever handled. There is a test
/// that this list and the generated sections still match.
///
/// `tier` is `None` where the type is not split by tier. `infernal_only` picks
/// the `imp_*` members out of the troops category.
pub struct ImprovementKind {
    pub section: &'static str,
    pub display: &'static str,
    pub category: &'static str,
    pub tier: Option<&'static str>,
    pub infernal_only: bool,
}

pub const IMPROVEMENT_KINDS: &[ImprovementKind] = &[
    k("improvement_production_t1", "Basic Production", CAT_PRODUCTION, Some("1"), false),
    k("improvement_production_t2", "Established Production", CAT_PRODUCTION, Some("2"), false),
    k("improvement_production_t3", "Advanced Production", CAT_PRODUCTION, Some("3"), false),
    k("improvement_troops_t1", "Levy Barracks", CAT_TROOPS, Some("1"), false),
    k("improvement_troops_t2", "Veteran Barracks", CAT_TROOPS, Some("2"), false),
    k("improvement_troops_t3", "Elite Barracks", CAT_TROOPS, Some("3"), false),
    k("improvement_infernals", "Infernal Barracks", CAT_TROOPS, None, true),
    k("improvement_infernals_t1", "Infernal Barracks T1", CAT_TROOPS, Some("1"), true),
    k("improvement_infernals_t2", "Infernal Barracks T2", CAT_TROOPS, Some("2"), true),
    k("improvement_infernals_t3", "Infernal Barracks T3", CAT_TROOPS, Some("3"), true),
    k("improvement_misc", "Kingdom Infrastructure", CAT_MISC, None, false),
    k("improvement_attacking", "Offensive Structures", CAT_ATTACKING, None, false),
];

const fn k(
    section: &'static str,
    display: &'static str,
    category: &'static str,
    tier: Option<&'static str>,
    infernal_only: bool,
) -> ImprovementKind {
    ImprovementKind { section, display, category, tier, infernal_only }
}

struct Lib {
    entries: Vec<(String, Value)>,
}

impl Lib {
    fn keys_where(&self, state: &State, f: impl Fn(&Value) -> bool) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .entries
            .iter()
            .filter(|(_, v)| f(v))
            .map(|(k, v)| (k.clone(), title(state, v)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn all(&self, state: &State) -> Vec<(String, String)> {
        self.keys_where(state, |_| true)
    }
}

fn title(state: &State, v: &Value) -> String {
    state
        .syms
        .var_id("title_default_value")
        .and_then(|id| unsafe { dslist::struct_member(v, id) })
        .and_then(|t| t.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn num(state: &State, v: &Value, field: &str) -> Option<i64> {
    let id = state.syms.var_id(field)?;
    match unsafe { dslist::struct_member(v, id) }? {
        Value::Int(i) => Some(i),
        Value::Real(r) => Some(r as i64),
        Value::Bool(b) => Some(b as i64),
        _ => None,
    }
}

fn flag(state: &State, v: &Value, field: &str) -> bool {
    let Some(id) = state.syms.var_id(field) else { return false };
    matches!(
        unsafe { dslist::struct_member(v, id) },
        Some(Value::Bool(true))
    )
}

/// Read a library `ds_map` into `(key, entry-struct)` pairs.
fn library(state: &State, base: usize, g: &Globals, name: &str) -> Option<Lib> {
    let id = state.syms.var_id(name)?;
    let v = unsafe { g.get(id) }?;
    let entries = dslist::ds_map_entries(base, &v, 8192)?;
    Some(Lib {
        entries: entries
            .into_iter()
            .filter_map(|(k, val)| {
                let key = k.as_str()?.to_owned();
                matches!(val, Value::Object(_)).then_some((key, val))
            })
            .collect(),
    })
}

/// Read a grouping `ds_map` -- keys to arrays of member names.
fn grouping(state: &State, base: usize, g: &Globals, name: &str) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    let Some(id) = state.syms.var_id(name) else { return out };
    let Some(v) = (unsafe { g.get(id) }) else { return out };
    let Some(entries) = dslist::ds_map_entries(base, &v, 4096) else { return out };
    for (k, val) in entries {
        let key = match k {
            Value::Str(s) => s,
            Value::Int(i) => i.to_string(),
            Value::Real(r) => format!("{}", r as i64),
            _ => continue,
        };
        out.insert(key, crate::survey::array_strings(&val));
    }
    out
}

/// Every reward type the mod can drive, with the options it can offer, plus the
/// improvements the game excludes from every drop pool -- listed in the config
/// for information, so the filtering can be checked rather than trusted.
///
/// Returns `None` before the libraries exist -- they are globals, so this needs
/// the game to have started, not a run to be in progress.
pub fn build(
    state: &State,
    base: usize,
) -> Option<(Vec<TypeVocab<'static>>, Vec<(String, String)>)> {
    let g = Globals::resolve(base, state.text).ok()?;

    let artifacts = library(state, base, &g, "ARTIFACTS")?;
    let spells = library(state, base, &g, "SPELLS")?;
    let upgrades = library(state, base, &g, "UPGRADES")?;
    let resources = library(state, base, &g, "RESOURCES")?;
    let improvements = library(state, base, &g, "IMPROVEMENTS")?;

    let by_category = grouping(state, base, &g, "IMPROVEMENTS_BY_CATEGORY");
    let by_tier = grouping(state, base, &g, "IMPROVEMENTS_BY_TIER");

    // Improvements the game itself says can never drop.
    let excluded: Vec<String> = improvements
        .entries
        .iter()
        .filter(|(_, v)| flag(state, v, "excluded_from_drop_pool"))
        .map(|(k, _)| k.clone())
        .collect();
    let titles: HashMap<String, String> = improvements
        .entries
        .iter()
        .map(|(k, v)| (k.clone(), title(state, v)))
        .collect();

    let improv = |cat: &str, tier: Option<&str>| -> Vec<(String, String)> {
        let Some(in_cat) = by_category.get(cat) else { return Vec::new() };
        let mut out: Vec<(String, String)> = in_cat
            .iter()
            .filter(|id| !excluded.contains(id))
            .filter(|id| match tier {
                None => true,
                Some(t) => by_tier.get(t).is_some_and(|v| v.contains(id)),
            })
            .map(|id| (id.clone(), titles.get(id).cloned().unwrap_or_default()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    };
    // The infernal barracks are the `imp_*` members of the troops category.
    let infernal = |tier: Option<&str>| -> Vec<(String, String)> {
        improv(CAT_TROOPS, tier)
            .into_iter()
            .filter(|(id, _)| id.starts_with("imp_"))
            .collect()
    };

    let mut out = vec![
        TypeVocab {
            reward_type: "unit_class_stat",
            display: "Troops Training",
            options: troops(state, base, &g)?,
            can_scrap: false,
            note: "a choice mixes classes and stats freely".into(),
        },
        TypeVocab {
            reward_type: "resource",
            display: "Resource",
            // `random` is a marker meaning "roll a resource", not an option
            options: resources.keys_where(state, |_| true)
                .into_iter()
                .filter(|(k, _)| k != "random")
                .collect(),
            can_scrap: true,
            note: "biome-specific resources are included -- they appear only on                    their own levels, but they can appear".into(),
        },
        TypeVocab {
            reward_type: "artifact",
            display: "Artifact",
            options: artifacts.keys_where(state, |v| num(state, v, "tier") == Some(TIER_ORDINARY)),
            can_scrap: true,
            note: format!("tier 1 of {} artifacts", artifacts.entries.len()),
        },
        TypeVocab {
            reward_type: "artifact_legend",
            display: "Legendary Artifact",
            options: artifacts.keys_where(state, |v| num(state, v, "tier") == Some(TIER_LEGENDARY)),
            can_scrap: true,
            note: format!("tier 2 of {} artifacts", artifacts.entries.len()),
        },
        TypeVocab {
            reward_type: "spell",
            display: "Spell",
            options: spells.keys_where(state, |v| num(state, v, "tier") == Some(TIER_ORDINARY)),
            can_scrap: true,
            note: format!("tier 1 of {} spells", spells.entries.len()),
        },
        TypeVocab {
            reward_type: "spell_legend",
            display: "Legendary Spell",
            options: spells.keys_where(state, |v| num(state, v, "tier") == Some(TIER_LEGENDARY)),
            can_scrap: true,
            note: format!("tier 2 of {} spells", spells.entries.len()),
        },
        TypeVocab {
            reward_type: "upgrade",
            display: "Building Upgrade",
            options: upgrades.all(state),
            can_scrap: true,
            note: "all of them; offered per built improvement".into(),
        },
    ];

    // Order follows the shape of the reward screen, and comes from the one
    // list these names live in.
    for kind in IMPROVEMENT_KINDS {
        let options = if kind.infernal_only {
            infernal(kind.tier)
        } else {
            improv(kind.category, kind.tier)
        };
        out.push(TypeVocab {
            reward_type: kind.section,
            display: kind.display,
            options,
            can_scrap: true,
            note: if kind.infernal_only { INFERNAL_NOTE.into() } else { GROUPED.into() },
        });
    }

    let mut never: Vec<(String, String)> = excluded
        .iter()
        .map(|id| (id.clone(), titles.get(id).cloned().unwrap_or_default()))
        .collect();
    never.sort_by(|a, b| a.0.cmp(&b.0));

    Some((out, never))
}

/// `class.stat` pairs, in the exact spelling the card reader produces.
///
/// A Troops Training card names its class as an *index*, so the ids here are
/// positional and cannot be read out of the game's `UNIT_CLASSES` map (whose
/// keys are those same indices). Returns `None` if the game no longer has eight
/// classes -- then the index-to-name table is stale and every id would be wrong.
fn troops(state: &State, base: usize, g: &Globals) -> Option<Vec<(String, String)>> {
    let live = state
        .syms
        .var_id("UNIT_CLASSES_LENGTH")
        .and_then(|id| unsafe { g.get(id) })
        .and_then(|v| match v {
            Value::Int(i) => Some(i),
            Value::Real(r) => Some(r as i64),
            _ => None,
        });
    let _ = base;
    if live != Some(CLASS_TITLES.len() as i64) {
        // Said once: this is retried on the config cadence until a file exists.
        static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::logln!(
                "config: the game reports {live:?} unit classes, not {}; the class names                  here are positional, so writing them now would produce ids no card                  will ever match.",
                CLASS_TITLES.len()
            );
        }
        return None;
    }

    let mut out = Vec::new();
    for (id, title) in crate::resolve::UNIT_CLASSES.iter().zip(CLASS_TITLES) {
        for stat in crate::resolve::UNIT_STATS {
            out.push((format!("{id}.{stat}"), format!("{title} +{stat}")));
        }
    }
    Some(out)
}

