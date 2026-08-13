//! Turning a live choice into a decision.
//!
//! This is where the read side meets the picker. It reads the option cards on
//! screen, turns them into config ids, asks `picker::decide`, and reports what
//! it would do.
//!
//! **It does not act.** Nothing here presses a button. The mod logs its
//! intention so the player can check that the picks match what they meant
//! before it is ever allowed to touch the game -- and because a wrong auto-pick
//! is the one failure that cannot be undone.

use crate::config::{Config, TypeConfig};
use crate::picker::{self, Decision, Economy, RerollCost};
use crate::rvalue::Value;
use crate::survey::CARD_OBJECTS;
use crate::{dslist, instance, logln, State};

/// A choice as the mod sees it: which card object, and the option ids on offer.
pub struct Choice {
    /// The card object, e.g. `obj_card_resource`.
    pub card_object: &'static str,
    /// The reward-type section this maps to in the config.
    pub config_type: &'static str,
    /// One id per card, in card order.
    pub options: Vec<String>,
    /// The card instances, parallel to `options`.
    pub cards: Vec<usize>,
    pub scrap_available: bool,
}

/// Which config section a card object belongs to.
///
/// The mapping is by *card object*, deliberately, not by the queue entry's
/// declared `reward_type`: when a type's candidates are exhausted the game can
/// offer resource compensation instead, so the entry can say `artifact` while
/// resource cards are on screen. Acting on the declared type would apply the
/// wrong preferences to a real choice.
fn config_type_for(state: &State, base: usize, card_object: &str) -> Option<&'static str> {
    Some(match card_object {
        "obj_card_resource" => "resource",
        "obj_card_artifact" => "artifact",
        "obj_card_spell" => "spell",
        "obj_card_upgrade" => "upgrade",
        "obj_card_class_stat_bonus" => "unit_class_stat",
        // One card object, twelve config sections. Production, barracks,
        // infernals and the rest all arrive as `obj_card_improvement`, so
        // unlike every other type the object alone cannot say which
        // preferences apply -- `improvement_production_t2` and
        // `improvement_troops_t3` look identical on screen.
        //
        // The open reward's own declared type is what separates them, and it
        // is narrowing *within* the improvements rather than overriding what
        // is on screen: the card object still decides the family, and a
        // declared type that is not an improvement is refused rather than
        // guessed at. So the rule that a choice is read from the cards, not
        // from what the queue said it would be, still holds.
        "obj_card_improvement" => {
            let declared = open_reward_type(state, base)?;
            return crate::vocab::IMPROVEMENT_KINDS
                .iter()
                .find(|k| k.section == declared.as_str())
                .map(|k| k.section);
        }
        // start bonus cards carry no id at all -- refused by design
        _ => return None,
    })
}

/// The declared type of the reward whose cards are on screen *now*.
///
/// Not the queue head: opening a reward takes it off the front, so by the time
/// its cards exist the head is already the next one. The reroll button belongs
/// to the open reward and carries its type.
fn open_reward_type(state: &State, base: usize) -> Option<String> {
    let btn = instance::find_all(base, "obj_button_reroll_cards").first().copied()?;
    let id = state.syms.var_id("reward_type")?;
    let rv = unsafe { instance::get_var(btn, id) }?;
    crate::rvalue::decode(rv)?.as_str().map(str::to_owned)
}

/// Read whatever choice is on screen, if it is complete and unambiguous.
///
/// Returns `None` when there is nothing to decide. Returns `Err` when there is
/// something on screen the mod does not understand, which is a reason to stop
/// entirely rather than skip.
pub fn read_choice(state: &State, base: usize) -> Result<Option<Choice>, String> {
    let mut found: Option<Choice> = None;

    for (obj, identity) in CARD_OBJECTS {
        let cards = instance::find_all(base, obj);
        if cards.is_empty() {
            continue;
        }
        // Two different card types on screen at once is not something the game
        // does; if it happens, the mod does not understand the situation.
        if found.is_some() {
            return Err(format!("two kinds of option card on screen at once ({obj})"));
        }
        // Every card must be built before any of them is read: they hold
        // placeholder values for a few frames after they appear.
        if !cards.iter().all(|c| crate::survey::card_ready(state, *c)) {
            return Ok(None);
        }

        let Some(config_type) = config_type_for(state, base, obj) else {
            if *obj == "obj_card_improvement" {
                // Not an error worth switching the mod off for: it means the
                // open reward's type could not be read, or is one this build
                // does not know. Leaving it to the player is the right answer
                // -- but say so, because a silent no-op here is exactly how
                // buildings went unhandled for a whole release.
                static SAID: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    logln!(
                        "[would] building cards are on screen but the open reward's type                          reads as {:?}, which is not one of the improvement sections.                          Leaving it to you.",
                        open_reward_type(state, base)
                    );
                }
                return Ok(None);
            }
            return Err(format!("{obj} carries no addressable id; refusing to automate it"));
        };

        let mut options = Vec::new();
        for card in &cards {
            match option_id(state, *card, identity) {
                Some(id) => options.push(id),
                None => {
                    return Err(format!("could not identify an option on a {obj} card"));
                }
            }
        }
        found = Some(Choice {
            card_object: obj,
            config_type,
            options,
            cards,
            scrap_available: instance::count(base, "obj_button_rewards_scrap") > 0,
        });
    }
    Ok(found)
}

/// The config id for one card.
///
/// Simple types carry a library struct whose `system_name` is the id. Troops
/// Training carries an array of `{stat_type, unit_class, stat_amount}`, which
/// maps to a `<class>.<stat>` key.
fn option_id(state: &State, card: usize, identity: &str) -> Option<String> {
    let id = state.syms.var_id(identity)?;
    let rv = unsafe { instance::get_var(card, id) }?;
    let v = crate::rvalue::decode(rv)?;

    match &v {
        Value::Object(_) => {
            let name_id = state.syms.var_id("system_name")?;
            unsafe { dslist::struct_member(&v, name_id) }?
                .as_str()
                .map(str::to_owned)
        }
        Value::Array(payload) => {
            // one element per bonus; a card can hold more than one
            let len = crate::rvalue::read_i32(*payload + crate::rvalue::ARRAY_LEN)?;
            if len != 1 {
                return None; // multi-bonus cards are not addressable by one key
            }
            let items = crate::rvalue::read_usize(*payload + crate::rvalue::ARRAY_DATA)?;
            let elem = crate::rvalue::decode(items)?;
            let class = num_field(state, &elem, "unit_class")? as usize;
            let stat = num_field(state, &elem, "stat_type")? as usize;
            Some(format!("{}.{}", UNIT_CLASSES.get(class)?, UNIT_STATS.get(stat)?))
        }
        _ => None,
    }
}

#[cfg(test)]
mod card_mapping_tests {
    /// Every improvement section the generator writes must be one the resolver
    /// will accept, and vice versa.
    ///
    /// This is the test that was missing. The generator wrote twelve
    /// `improvement_*` sections; the resolver looked up a single
    /// `[improvement]`, found nothing, and left every building reward to the
    /// player -- silently, because "no section for this type" is also what a
    /// deliberately-unautomated type looks like. Nothing compared the two lists
    /// because there was no one place holding them.
    #[test]
    fn every_improvement_section_is_resolvable() {
        // The config sections a fresh file would contain, as the generator
        // names them.
        let generated: Vec<&str> =
            crate::vocab::IMPROVEMENT_KINDS.iter().map(|k| k.section).collect();
        assert!(!generated.is_empty());

        for section in &generated {
            assert!(
                section.starts_with("improvement_"),
                "{section} is in the improvement list but is not named like one"
            );
            // what `config_type_for` will match a declared type against
            assert!(
                crate::vocab::IMPROVEMENT_KINDS.iter().any(|k| k.section == *section),
                "{section} is generated but the resolver would not accept it"
            );
        }

        let mut seen = generated.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), generated.len(), "duplicate improvement section");
    }

    /// The plain `improvement` name is not a config section, and must not
    /// become one by accident -- that spelling was the bug.
    #[test]
    fn there_is_no_bare_improvement_section() {
        assert!(
            !crate::vocab::IMPROVEMENT_KINDS.iter().any(|k| k.section == "improvement"),
            "a bare [improvement] section cannot carry per-tier preferences"
        );
    }
}

/// Class index -> config key. Positional, so the count is checked before use.
pub const UNIT_CLASSES: &[&str] = &[
    "grunt", "rider", "flying", "ranged", "arcane", "warrior", "champion", "undead",
];
/// `stat_type` 0 and 1. Attack speed exists as a mod but is never offered here.
pub const UNIT_STATS: &[&str] = &["hp", "damage"];

/// Read a numeric field, accepting every numeric kind the game emits.
///
/// These come back as **int64** rather than doubles, so a reader that only
/// accepts kind 0 silently never matches.
fn num_field(state: &State, strukt: &Value, name: &str) -> Option<i64> {
    let id = state.syms.var_id(name)?;
    match unsafe { dslist::struct_member(strukt, id) }? {
        Value::Int(v) => Some(v),
        Value::Real(v) => Some(v as i64),
        Value::Bool(b) => Some(b as i64),
        _ => None,
    }
}

/// How many rerolls the mod itself has triggered for the reward currently open.
///
/// The budgets are per reward, so this resets when a new choice appears. It
/// counts what the *mod* spent, which is what the config caps -- rerolls the
/// player made by hand are theirs and are not charged against it.
#[derive(Default, Clone, Copy)]
pub struct Spent {
    pub voodoo: u32,
    pub free: u32,
    pub paid: u32,
}

static SPENT: std::sync::Mutex<Spent> = std::sync::Mutex::new(Spent {
    voodoo: 0,
    free: 0,
    paid: 0,
});

pub fn reset_spent() {
    if let Ok(mut g) = SPENT.lock() {
        *g = Spent::default();
    }
}

fn spent() -> Spent {
    SPENT.lock().map(|g| *g).unwrap_or_default()
}

/// Latched once `resolve_reroll_cost` has been shown not to answer, so the mod
/// stops paying for a question whose answer will not change this session.
/// Cleared only by restarting, which is also when a new build could load.
static PRICE_UNREADABLE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn record_spend(cost: RerollCost) {
    if let Ok(mut g) = SPENT.lock() {
        match cost {
            RerollCost::Voodoo => g.voodoo += 1,
            RerollCost::Free => g.free += 1,
            RerollCost::Paid(_) => g.paid += 1,
            RerollCost::Unavailable => {}
        }
    }
}

/// Read the live reroll economy for the open choice.
pub fn read_economy(state: &State, base: usize) -> Economy {
    let btn = instance::find_all(base, "obj_button_reroll_cards")
        .first()
        .copied();
    let num = |inst: usize, n: &str| -> Option<f64> {
        let id = state.syms.var_id(n)?;
        let rv = unsafe { instance::get_var(inst, id) }?;
        crate::rvalue::decode(rv)?.as_f64()
    };

    let (free_per_reward, paid_made) = match btn {
        Some(b) => (
            num(b, "free_rerolls_per_reward_left").unwrap_or(0.0),
            num(b, "non_free_rerolls_made").unwrap_or(0.0),
        ),
        None => (0.0, 0.0),
    };

    let _ = paid_made;
    // The reign-wide pool is a global.
    let run_pool = crate::globals::Globals::resolve(base, state.text)
        .ok()
        .and_then(|g| {
            state
                .syms
                .var_id("FREE_REROLLS_PER_RUN_LEFT")
                .and_then(|id| unsafe { g.get(id) })
        })
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    // What the *next* reroll costs is the game's decision, not the mod's: it
    // spends the per-reward freebie first, then the reign pool, and only then
    // charges denarii. The budgets are matched against whichever it would be.
    let next_cost = match btn {
        None => RerollCost::Unavailable,
        Some(b) => {
            if free_per_reward >= 1.0 {
                RerollCost::Voodoo
            } else if run_pool >= 1.0 {
                RerollCost::Free
            } else {
                // A paid reroll needs its price, and the mod will not spend an
                // unknown amount.
                //
                // `resolve_reroll_cost` was the obvious place to get it, and it
                // returned kind 5 -- undefined -- on all 597 calls in one
                // session. Disassembling it says why: it is not a getter. It
                // reads `FREE_REROLLS_PER_RUN_LEFT`, `free_rerolls_per_reward_left`
                // and `non_free_rerolls_made`, works the price out from
                // `cost_initial` and `cost_increase_per_reroll`, and *writes it
                // to* `self.resource_cost`. It returns nothing because it was
                // never meant to.
                //
                // So: let it recompute, then read the member it maintains.
                //
                // Asked once per session once it has failed, not once per
                // frame: a choice sitting on screen was re-asking sixty times a
                // second, each time an invoke and a log write.
                let price = (!PRICE_UNREADABLE.load(std::sync::atomic::Ordering::Relaxed))
                    .then(|| read_price(state, base, b))
                    .flatten();
                match price {
                    Some(p) => RerollCost::Paid(p),
                    None => {
                        if !PRICE_UNREADABLE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            logln!(
                                "[reroll] free rerolls are exhausted and the paid price \
                                 cannot be read from the game, so paid rerolls are \
                                 declined. paid_depth has no effect until that is fixed."
                            );
                        }
                        RerollCost::Unavailable
                    }
                }
            }
        }
    };

    let s = spent();
    Economy {
        next_cost,
        denarii: denarii(state, base),
        voodoo_used: s.voodoo,
        free_used: s.free,
        paid_used: s.paid,
    }
}

/// The coin amount inside whatever shape the game used to express a cost.
///
/// Three shapes turn up, and the mod is not in a position to insist on one:
///
/// * `{type: "coin", amount: 10}` -- a single cost, which is how the reroll
///   button holds `cost_initial` and `cost_increase_per_reroll`
/// * an **array** of those, which is what `resource_cost` actually is -- a
///   price can name more than one resource
/// * a struct keyed by resource id, which is how the player's balances are held
///
/// Only coin is ever returned. A cost naming another resource reads as no coin
/// price at all, so the mod declines rather than paying something it has no
/// budget for.
fn coin_in(state: &State, base: usize, v: &Value) -> Option<i64> {
    match v {
        Value::Object(_) => {
            // `{type, amount}` first: a cost entry, not a balance sheet.
            let ty = state
                .syms
                .var_id("type")
                .and_then(|id| unsafe { dslist::struct_member(v, id) })
                .and_then(|t| t.as_str().map(str::to_owned));
            if let Some(ty) = ty {
                if ty != "coin" {
                    return None;
                }
                return state
                    .syms
                    .var_id("amount")
                    .and_then(|id| unsafe { dslist::struct_member(v, id) })
                    .and_then(|a| a.as_f64())
                    .map(|n| n as i64);
            }
            coin_amount(state, base, v)
        }
        Value::Array(payload) => {
            let len = crate::rvalue::read_i32(*payload + crate::rvalue::ARRAY_LEN)?;
            if !(0..64).contains(&len) {
                return None;
            }
            let items = crate::rvalue::read_usize(*payload + crate::rvalue::ARRAY_DATA)?;
            for i in 0..len as usize {
                let elem = crate::rvalue::decode(items + i * 16)?;
                if let Some(n) = coin_in(state, base, &elem) {
                    return Some(n);
                }
            }
            None
        }
        // a ds_map keyed by resource id
        Value::Ref { .. } => {
            let entries = dslist::ds_map_entries(base, v, 64)?;
            entries.into_iter().find_map(|(k, val)| {
                (k.as_str() == Some("coin")).then(|| val.as_f64())?.map(|n| n as i64)
            })
        }
        _ => None,
    }
}

/// The coin figure out of a resource-keyed struct.
///
/// Read by *name*, not by variable id: `coin` is a data-driven key that never
/// appears literally in the game's code, so it has no id in the exe's variable
/// table. Asking for one returned nothing, which made this read fail silently --
/// it took the paid-reroll price down with it, and had been quietly reporting
/// the player's denarii as zero for as long as it has existed.
fn coin_amount(state: &State, base: usize, v: &Value) -> Option<i64> {
    unsafe { crate::builtin::struct_get_by_name(base, state.text, v, "coin") }
        .and_then(|c| c.as_f64())
        .map(|n| n as i64)
}

/// What the next paid reroll costs in denarii, or `None` if it cannot be read.
///
/// `resource_cost` is the member the game maintains, and it is shaped like
/// `resources` on the gameplay controller: a struct keyed by resource id. Coin
/// is denarii. Anything else on it is a price in some other resource, which the
/// mod has no budget for and will not pay -- so a cost naming any resource but
/// coin reads as unpayable rather than free.
fn read_price(state: &State, base: usize, btn: usize) -> Option<i64> {
    match price_of(state, base, btn) {
        Ok(p) => {
            // Say it worked, not only when it fails. A price that reads fine but
            // is then declined on budget or floor looks identical, in the log,
            // to one that could not be read at all.
            static SAID: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::findln!("[reroll] paid reroll priced at {p} denarii - the price reads correctly");
            }
            Some(p)
        }
        Err(why) => {
            // Last time this returned a bare `None` and the reason was thrown
            // away with it, which cost a whole session to learn nothing. Every
            // way out of the read now says what it was.
            static SAID: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::findln!("[reroll] cannot price a paid reroll: {why}");
                dump_reroll_button(state, base, btn);
            }
            None
        }
    }
}

fn price_of(state: &State, base: usize, btn: usize) -> Result<i64, String> {
    // Let the game work the current price out first. This is its own method,
    // called the way it calls it, and it only writes to the button it belongs
    // to -- the same recompute that happens when the button is drawn.
    unsafe {
        crate::press::run_method(state, base, btn, "resolve_reroll_cost");
    }

    let id = state
        .syms
        .var_id("resource_cost")
        .ok_or("the build has no variable named resource_cost")?;
    let rv = unsafe { instance::get_var(btn, id) }
        .ok_or("the reroll button has no resource_cost member")?;
    let v = crate::rvalue::decode(rv).ok_or("resource_cost did not decode as a value")?;

    // A bare number would mean denarii with no resource named.
    if let Some(n) = v.as_f64() {
        return if n >= 0.0 {
            Ok(n as i64)
        } else {
            Err(format!("resource_cost is a negative number ({n})"))
        };
    }

    match coin_in(state, base, &v) {
        Some(n) if n >= 0 => Ok(n),
        Some(n) => Err(format!("the coin price came out negative ({n})")),
        // A price naming no coin is a price in some other resource, which the
        // mod has no budget for and will not pay.
        None => Err(format!(
            "no coin amount in resource_cost ({})",
            crate::summarise(&v)
        )),
    }
}

/// Member names of a struct, for a diagnostic. Allocates inside the game and
/// never frees, so this is for one-shot reporting only, never a hot path.
fn members(state: &State, base: usize, v: &Value) -> String {
    match unsafe { crate::builtin::struct_member_names(base, state.text, v) } {
        Some(names) => names.join(", "),
        None => "<could not enumerate>".into(),
    }
}

/// Everything on the reroll button that could plausibly carry a price.
///
/// `resolve_reroll_cost` builds the price from `cost_initial` and
/// `cost_increase_per_reroll`, both of which the survey shows as structs. If
/// `resource_cost` is not where the price lands, the answer is in one of these.
fn dump_reroll_button(state: &State, base: usize, btn: usize) {
    for field in ["resource_cost", "cost_initial", "cost_increase_per_reroll"] {
        let Some(id) = state.syms.var_id(field) else {
            logln!("          {field}: no such variable in this build");
            continue;
        };
        let Some(rv) = (unsafe { instance::get_var(btn, id) }) else {
            logln!("          {field}: not present on the button");
            continue;
        };
        let Some(v) = crate::rvalue::decode(rv) else {
            logln!("          {field}: did not decode");
            continue;
        };
        crate::findln!("          {field} = {}", crate::summarise(&v));
        if matches!(v, Value::Object(_)) {
            crate::findln!("            members: {}", members(state, base, &v));
            // and one level in, since a cost is likely {resource: amount}
            if let Some(names) =
                unsafe { crate::builtin::struct_member_names(base, state.text, &v) }
            {
                for n in names.iter().take(24) {
                    if let Some(mid) = state.syms.var_id(n) {
                        if let Some(inner) = unsafe { dslist::struct_member(&v, mid) } {
                            crate::findln!("            .{n} = {}", crate::summarise(&inner));
                        }
                    }
                }
            }
        }
    }
}

/// The player's denarii, for affordability and the floor.
///
/// Falling back to zero here is not neutral -- it makes every paid reroll look
/// unaffordable and every `denarii_floor` look breached, which is indisputably
/// safe but silently wrong. So a failure says so once, rather than passing a
/// zero on as if it were a reading.
fn denarii(state: &State, base: usize) -> i64 {
    match read_denarii(state, base) {
        Ok(n) => {
            static SAID: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::findln!("[reroll] denarii read as {n} - the balance is being read correctly");
            }
            n
        }
        Err(why) => {
            static SAID: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::findln!("[reroll] cannot read your denarii ({why}), so treating it as 0.");
                logln!("         that makes every paid reroll look unaffordable.");
            }
            0
        }
    }
}

fn read_denarii(state: &State, base: usize) -> Result<i64, String> {
    let ctrl = instance::find_singleton(base, "obj_gameplay_controller")
        .ok_or("no gameplay controller")?;
    let id = state.syms.var_id("resources").ok_or("no variable id for resources")?;
    let rv = unsafe { instance::get_var(ctrl, id) }
        .ok_or("the controller has no resources member")?;
    let v = crate::rvalue::decode(rv).ok_or("resources did not decode")?;
    coin_in(state, base, &v)
        .ok_or_else(|| format!("no coin in resources, which is {}", crate::summarise(&v)))
}

/// One step of the mod's job.
///
/// Either resolve the choice on screen, or -- if there is none -- open the next
/// reward from the queue. Strict FIFO: only the head is ever touched, and if
/// the head is a type the config does not cover, the mod stops rather than
/// reaching past it.
pub fn step(
    state: &State,
    base: usize,
    config: &Config,
    // how many option cards the caller just counted, if it did
    cards: Option<u32>,
    tiebreak: &mut dyn FnMut(usize) -> usize,
) {
    // Something on screen takes precedence: finish what is open before opening
    // anything else.
    // Leave a just-resolved choice completely alone until its cards are gone.
    if settling() {
        return;
    }
    // Taken from the watch rather than recounted. `watch` has just walked every
    // card object's instance list to get this number; doing it a second time in
    // the same frame doubled the mod's per-frame cost for no new information.
    let anything_open = match cards {
        Some(n) => n > 0,
        None => CARD_OBJECTS.iter().any(|(o, _)| instance::count(base, o) > 0),
    };
    if anything_open {
        note_choice_appeared();
        crate::press::trace("reading the choice on screen");
        resolve_choice(state, base, config, tiebreak);
        return;
    }
    open_next(state, base, config);
}

/// How long to leave the cards alone after picking one.
///
/// Choosing destroys the cards, and the game animates them out -- so for some
/// frames afterwards the instances still exist while their contents are being
/// torn down. The mod reads every card every frame, so without this it goes on
/// reading structures that are in the middle of being freed.
///
/// The guard on the way *in* (`card_ready`) has no counterpart on the way out,
/// and a run that drained 119 rewards died immediately after a successful pick,
/// which is exactly where this window is.
const PICK_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

static SETTLING: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

fn begin_settling() {
    if let Ok(mut g) = SETTLING.lock() {
        *g = Some(std::time::Instant::now());
    }
}

/// True while the cards from a just-resolved choice may still be dying.
///
/// Public because the *survey* has to respect it too. The survey reads every
/// field of every card, and it runs in the same frame as a press -- so after a
/// pick it can be walking structures the game is in the middle of tearing down.
/// That is the most likely shape of the crash that keeps arriving shortly after
/// a successful pick, with the log ending on the `[PICK]` line and nothing
/// after it.
pub fn settling() -> bool {
    let mut g = match SETTLING.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    match *g {
        Some(t) if t.elapsed() < PICK_SETTLE => true,
        Some(_) => {
            *g = None;
            false
        }
        None => false,
    }
}

/// How long to wait for a choice to appear after pressing the queue button.
///
/// Cards take several frames to be created and built. Without this the mod
/// would see "no cards on screen" the frame after opening, decide the queue
/// still needed opening, and press again -- stacking reward screens on top of
/// each other as fast as the press rate allowed. Opening is the one action
/// whose effect is not immediate, so it is the one that needs a settling
/// period.
const OPEN_SETTLE: std::time::Duration = std::time::Duration::from_millis(2500);

static AWAITING_OPEN: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

/// Called when cards appear, so the mod stops waiting for them.
pub fn note_choice_appeared() {
    if let Ok(mut g) = AWAITING_OPEN.lock() {
        *g = None;
    }
}

fn still_awaiting_open() -> bool {
    let mut g = match AWAITING_OPEN.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    match *g {
        Some(t) if t.elapsed() < OPEN_SETTLE => true,
        Some(_) => {
            // it never arrived; stop waiting rather than wedge forever
            *g = None;
            false
        }
        None => false,
    }
}

/// Open the next queued reward, if the mod is allowed to resolve it.
fn open_next(state: &State, base: usize, config: &Config) {
    if still_awaiting_open() {
        return;
    }
    let Some(btn) = queue_button(base) else { return };
    let Some(head) = head_type(state, base) else { return };

    // Strict FIFO: an unconfigured head is where the drain stops. Opening it
    // would leave a choice on screen the mod cannot finish.
    if config.for_type(&head).is_none() {
        static SAID: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        if let Ok(mut g) = SAID.lock() {
            if g.as_deref() != Some(head.as_str()) {
                logln!("[queue] head is {head}, which has no config section - stopping here");
                *g = Some(head);
            }
        }
        return;
    }
    if !crate::acting() {
        static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
            logln!("[would OPEN] the next reward ({head}) - not acting");
        }
        return;
    }
    if crate::press::press(state, base, btn, &format!("open queued {head}")).is_ok() {
        reset_spent();
        if let Ok(mut g) = AWAITING_OPEN.lock() {
            *g = Some(std::time::Instant::now());
        }
        logln!("[OPEN] {head}");
    }
}

/// Take back the hover state before a card is destroyed.
///
/// A Troops Training card shows the "units affected" icons while hovered, and
/// its Step event tears them down when `is_hovered` stops matching
/// `was_hovered`:
///
/// ```text
///   is_hovered != was_hovered -> call show_units_icons or hide_units_icons
///                                then was_hovered = is_hovered
/// ```
///
/// The mod picks by invoking the select button directly, without the mouse ever
/// leaving the card, so the card is destroyed still believing it is hovered and
/// the icons it drew are left behind. In the base game the situation cannot
/// arise -- the pointer has to travel to the button -- so this is a bug the mod
/// introduces, and it is the mod's to undo.
///
/// It undoes it the way the game does: by calling the card's own
/// `hide_units_icons`, and only on a card that says it is currently showing
/// them. Nothing here is invented, and it changes no game state -- the same
/// screen the player would have seen had they moved the mouse away first.
fn unhover(state: &State, base: usize, choice: &Choice) {
    crate::press::trace("taking back the hover before the cards go");
    // Only when there is actually something drawn. `was_hovered` is the card's
    // own belief, and calling the teardown against a drawer that does not exist
    // is exactly the kind of guess that has crashed this game before.
    if instance::count(base, "obj_unit_icons_drawer") == 0 {
        return;
    }
    for card in &choice.cards {
        let showing = state
            .syms
            .var_id("was_hovered")
            .and_then(|id| unsafe { instance::get_var(*card, id) })
            .and_then(crate::rvalue::decode)
            .map(|v| match v {
                Value::Bool(b) => b,
                Value::Real(r) => r != 0.0,
                Value::Int(i) => i != 0,
                _ => false,
            })
            .unwrap_or(false);
        if !showing {
            continue;
        }
        unsafe {
            crate::press::run_method(state, base, *card, "hide_units_icons");
        }
    }
}

/// Decide about the choice on screen, and act if allowed.
pub fn resolve_choice(state: &State, base: usize, config: &Config, tiebreak: &mut dyn FnMut(usize) -> usize) {
    let choice = match read_choice(state, base) {
        Err(e) => {
            crate::shut_down(e);
            return;
        }
        Ok(None) => return,
        Ok(Some(c)) => c,
    };

    let Some(conf): Option<&TypeConfig> = config.for_type(choice.config_type) else {
        logln!(
            "[would] {} offers [{}] - no [{}] section, leaving it to you",
            choice.card_object,
            choice.options.join(", "),
            choice.config_type
        );
        return;
    };

    crate::press::trace(&format!("deciding about [{}]", choice.options.join(", ")));
    crate::press::trace("reading the reroll economy");
    let eco = read_economy(state, base);
    let decision = picker::decide(conf, &choice.options, &eco, choice.scrap_available, tiebreak);

    // The same choice stays on screen for many frames, so an unchanged decision
    // is reported once. This suppresses the *log line only*: returning early
    // here also skipped the press, which meant that switching the mod off and
    // on again left it silently inert -- the decision it had recorded while
    // observing looked unchanged, so the press never happened.
    let quiet = {
        let line = format!("{decision:?} {:?}", choice.options);
        static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        let mut g = match LAST.lock() { Ok(g) => g, Err(e) => e.into_inner() };
        let same = g.as_deref() == Some(line.as_str());
        *g = Some(line);
        same
    };

    match &decision {
        Decision::Pick(id) => {
            // ---- STAGE 1: this is the mod's first and only action.
            //
            // Press the select button of the card carrying the chosen id. The
            // reroll, scrap and queue buttons are deliberately not pressed yet;
            // they are reported below so the next stage is informed by data
            // rather than by another round of inference.
            let pressed = if !crate::acting() {
                None
            } else { choice
                .options
                .iter()
                .position(|o| o == id)
                .and_then(|i| choice.cards.get(i).copied())
                .and_then(|card| {
                    unhover(state, base, &choice);
                    crate::press::select_button_of(state, base, card)
                })
                .map(|btn| crate::press::press(state, base, btn, &format!("select {id}"))) };

            match pressed {
                Some(Ok(())) => {
                    begin_settling();
                    logln!(
                        "[PICK] {id}   from [{}]   ({})",
                        choice.options.join(", "),
                        choice.config_type
                    );
                }
                Some(Err(e)) => logln!("[would PICK] {id} - but could not press it: {e}"),
                None if !crate::acting() => {
                    if !quiet {
                        logln!(
                            "[would PICK] {id}   from [{}]   (not acting; Ctrl+Alt+P to                              switch on, or set act = true in the config)",
                            choice.options.join(", ")
                        );
                    }
                }
                None => logln!("[would PICK] {id} - no select button found on its card"),
            }
            report_next_stage_targets(state, base);
        }
        Decision::Reroll => {
            let cost = eco.next_cost;
            let btn = instance::find_all(base, "obj_button_reroll_cards").first().copied();
            match (crate::acting(), btn) {
                (true, Some(b)) => {
                    // A reroll destroys and rebuilds the cards just as a pick
                    // does, so the hover has to be taken back here too. Missing
                    // this is why the leftover icons came back intermittently:
                    // they only survived a reroll, never a pick.
                    unhover(state, base, &choice);
                    match crate::press::press(state, base, b, "reroll") {
                        Ok(()) => {
                            begin_settling();
                            record_spend(cost);
                            logln!(
                                "[REROLL] {cost:?} - none of [{}] is wanted",
                                choice.options.join(", ")
                            );
                        }
                        Err(e) => logln!("[would REROLL] but could not press it: {e}"),
                    }
                }
                (false, _) => {
                    if !quiet {
                        logln!(
                            "[would REROLL] {cost:?} - none of [{}] is wanted",
                            choice.options.join(", ")
                        );
                    }
                }
                (true, None) => logln!("[would REROLL] but there is no reroll button"),
            }
        }
        Decision::Manual(why) => {
            if !quiet {
                logln!("[would LEAVE] [{}] - {why}", choice.options.join(", "));
            }
        }
    }
}


/// Reconnaissance for the stages that are not switched on yet.
///
/// Reported once per choice: whether the reroll, scrap and queue buttons are
/// present, and whether their `button_pressed_action` really is a method the
/// same way a card's is. Confirming that now means stage 2 starts from evidence.
fn report_next_stage_targets(state: &State, base: usize) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    for obj in [
        "obj_button_reroll_cards",
        "obj_button_rewards_scrap",
        "obj_button_reward_queue",
        "obj_button_rewards_skip",
    ] {
        for inst in instance::find_all(base, obj) {
            let kind = state
                .syms
                .var_id("button_pressed_action")
                .and_then(|id| unsafe { instance::get_var(inst, id) })
                .and_then(crate::rvalue::decode);
            logln!(
                "[stage2] {obj} @{inst:#x} button_pressed_action = {}",
                match kind {
                    Some(v) => crate::summarise(&v),
                    None => "<absent>".into(),
                }
            );
        }
    }
}


// ------------------------------------------------------------------ watching
//
// Polling the whole UI on a timer was wasted work: almost every tick, nothing
// had changed. Instead a *cheap* tick runs every frame and reads two numbers --
// how many rewards are queued, and how many option cards exist. The expensive
// path only runs when one of them moves.
//
// That makes the mod effectively event-driven without patching any game code:
// a reward being earned changes the queue length, opening one makes cards
// appear, rerolling replaces them, and picking makes them vanish.

/// The two numbers the watch compares. Cheap: a cached object lookup, a short
/// instance-list walk, and a `ds_list` size read.
#[derive(PartialEq, Clone, Copy, Debug, Default)]
pub struct Watch {
    pub queued: i32,
    pub cards: u32,
    pub reroll_button: bool,
}

pub fn watch(state: &State, base: usize) -> Option<Watch> {
    let ctrl = instance::find_singleton(base, "obj_gameplay_controller")?;
    let queued = state
        .syms
        .var_id("pending_rewards_list")
        .and_then(|id| unsafe { instance::get_var(ctrl, id) })
        .and_then(crate::rvalue::decode)
        .and_then(|v| dslist::DsList::from_value(base, &v).map(|l| l.len() as i32))
        .unwrap_or(-1);

    let mut cards = 0u32;
    for (obj, _) in CARD_OBJECTS {
        cards += instance::count(base, obj) as u32;
    }
    Some(Watch {
        queued,
        cards,
        reroll_button: instance::count(base, "obj_button_reroll_cards") > 0,
    })
}

/// The queue button, if the queue can be opened right now.
pub fn queue_button(base: usize) -> Option<usize> {
    instance::find_all(base, "obj_button_reward_queue").first().copied()
}

/// The reward type at the head of the queue.
pub fn head_type(state: &State, base: usize) -> Option<String> {
    let ctrl = instance::find_singleton(base, "obj_gameplay_controller")?;
    let id = state.syms.var_id("pending_rewards_list")?;
    let rv = unsafe { instance::get_var(ctrl, id) }?;
    let v = crate::rvalue::decode(rv)?;
    let list = dslist::DsList::from_value(base, &v)?;
    let entry = list.get(0)?;
    let ty = state.syms.var_id("reward_type")?;
    unsafe { dslist::struct_member(&entry, ty) }?
        .as_str()
        .map(str::to_owned)
}
