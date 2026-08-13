//! The decision algorithm.
//!
//! Pure: given the offered options, the reroll economy and the player's config,
//! decide what to do. No game state is touched here, which is what makes the
//! rules testable exhaustively rather than by playing.
//!
//! From the spec:
//!
//! ```text
//! loop:
//!     if any WANTED option is offered -> take the highest weight (ties at random)
//!     if another reroll is permitted  -> reroll, and look again
//!     if any FALLBACK option is offered -> take the highest weight (ties at random)
//!     otherwise -> leave it for the player
//! ```
//!
//! Blacklisted options are never candidates, at any depth. They do not block:
//! if something wanted is also on offer, it is taken.

use crate::config::{Tier, TypeConfig, SCRAP};

/// What a reroll would cost, as the game decides it -- not as the mod chooses.
/// Free rerolls are spent automatically before denarii are ever charged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerollCost {
    /// The per-reward freebie (Voodoo Beads).
    Voodoo,
    /// The reign-wide free pool.
    Free,
    /// Denarii.
    Paid(i64),
    /// No reroll is available at all.
    Unavailable,
}

/// The reroll economy at this moment, read from the game.
#[derive(Debug, Clone, Copy)]
pub struct Economy {
    pub next_cost: RerollCost,
    pub denarii: i64,
    /// How many of each class the mod has already spent on *this* reward.
    pub voodoo_used: u32,
    pub free_used: u32,
    pub paid_used: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Take this option id (`_scrap` means press scrap).
    Pick(String),
    /// Reroll, then decide again.
    Reroll,
    /// Leave it for the player, and stop draining the queue.
    Manual(&'static str),
}

/// Why a reroll was or was not permitted, for the log.
#[derive(Debug, Clone, PartialEq)]
pub enum RerollVerdict {
    Allowed(RerollCost),
    Denied(&'static str),
}

/// Whether the mod may trigger the next reroll.
///
/// The mod does **not** choose the currency: the game spends free rerolls
/// automatically. So this matches the cost the game reports against that
/// class's budget, and **never falls through to another budget**. A player with
/// free rerolls banked cannot choose to pay, so neither may the mod -- which is
/// why `free_depth: Depth::Limit(1), paid_depth: Depth::Limit(1)` with two free rerolls banked rerolls once
/// and stops, leaving `paid_depth` unreachable. That is intended.
pub fn may_reroll(conf: &TypeConfig, eco: &Economy) -> RerollVerdict {
    let b = &conf.budgets;
    match eco.next_cost {
        RerollCost::Unavailable => RerollVerdict::Denied("no reroll available"),
        RerollCost::Voodoo => {
            if b.voodoo_depth.allows(eco.voodoo_used) {
                RerollVerdict::Allowed(RerollCost::Voodoo)
            } else {
                RerollVerdict::Denied("voodoo_depth spent")
            }
        }
        RerollCost::Free => {
            if b.free_depth.allows(eco.free_used) {
                RerollVerdict::Allowed(RerollCost::Free)
            } else {
                RerollVerdict::Denied("free_depth spent")
            }
        }
        RerollCost::Paid(price) => {
            if !b.paid_depth.allows(eco.paid_used) {
                RerollVerdict::Denied("paid_depth spent")
            } else if price > eco.denarii {
                RerollVerdict::Denied("cannot afford it")
            } else if eco.denarii - price < b.denarii_floor {
                RerollVerdict::Denied("would breach denarii_floor")
            } else {
                RerollVerdict::Allowed(RerollCost::Paid(price))
            }
        }
    }
}

/// Decide what to do about the options currently on offer.
///
/// `offered` are the option ids on the cards. `scrap_available` says whether
/// this reward type actually has a scrap button -- `unit_class_stat` does not,
/// so `_scrap` must not be reachable for it even if the config ranks it.
///
/// `tiebreak` picks among equally-weighted options. It must **not** draw on the
/// game's RNG: choosing between two options the player called indifferent is
/// the mod's decision, not a game event, and consuming game randomness would
/// shift every later roll in the run.
pub fn decide(
    conf: &TypeConfig,
    offered: &[String],
    eco: &Economy,
    scrap_available: bool,
    tiebreak: &mut dyn FnMut(usize) -> usize,
) -> Decision {
    // Any id the config does not classify means the mod is looking at something
    // it was never told about. Fail closed rather than pick around it.
    for id in offered {
        if conf.tier_of(id).is_none() {
            return Decision::Manual("an offered option is not in the config");
        }
    }

    let mut candidates: Vec<&str> = offered.iter().map(String::as_str).collect();
    if scrap_available && conf.tier_of(SCRAP).is_some() {
        candidates.push(SCRAP);
    }

    if let Some(pick) = best(conf, &candidates, Tier::Wanted, tiebreak) {
        return Decision::Pick(pick.to_string());
    }
    if let RerollVerdict::Allowed(_) = may_reroll(conf, eco) {
        return Decision::Reroll;
    }
    if let Some(pick) = best(conf, &candidates, Tier::Fallback, tiebreak) {
        return Decision::Pick(pick.to_string());
    }
    Decision::Manual("nothing acceptable, and no rerolls left in budget")
}

/// Highest-weighted candidate in `tier`, ties broken by `tiebreak`.
fn best<'a>(
    conf: &TypeConfig,
    candidates: &[&'a str],
    tier: Tier,
    tiebreak: &mut dyn FnMut(usize) -> usize,
) -> Option<&'a str> {
    let mut best_weight = f64::NEG_INFINITY;
    let mut tied: Vec<&'a str> = Vec::new();
    for id in candidates {
        let Some((t, w)) = conf.options.get(*id) else { continue };
        if *t != tier {
            continue;
        }
        if *w > best_weight {
            best_weight = *w;
            tied.clear();
            tied.push(id);
        } else if *w == best_weight {
            tied.push(id);
        }
    }
    match tied.len() {
        0 => None,
        1 => Some(tied[0]),
        n => Some(tied[tiebreak(n) % n]),
    }
}

/// A small deterministic generator for tie-breaking.
///
/// Deliberately the mod's own, and deliberately not the game's: see `decide`.
pub struct TieBreaker(u64);

impl TieBreaker {
    /// For a `static` initialiser; seeded well enough for coin-flips between
    /// options the player said they do not care about.
    pub const fn new_const() -> TieBreaker {
        TieBreaker(0x9E37_79B9_7F4A_7C15)
    }

    pub fn new(seed: u64) -> TieBreaker {
        TieBreaker(seed | 1)
    }

    pub fn next(&mut self, n: usize) -> usize {
        // xorshift64*: tiny, no dependency, and good enough to be fair between
        // two options the player said they did not care about
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % n.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Depth;
    use crate::config::{parse, Budgets};

    fn cfg(text: &str) -> TypeConfig {
        let c = parse(text);
        assert!(c.errors.is_empty(), "config errors: {:?}", c.errors);
        c.types.get("resource").cloned().unwrap_or_default()
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn eco(next: RerollCost) -> Economy {
        Economy { next_cost: next, denarii: 1000, voodoo_used: 0, free_used: 0, paid_used: 0 }
    }

    /// Always take the first, so tests are deterministic.
    fn first(_: usize) -> usize {
        0
    }

    const BASIC: &str = "
[resource]
voodoo_depth = 0
free_depth = 0
paid_depth = 0
[resource.wanted]
metal = 10
ore = 5
[resource.fallback]
clay = 7
wheat = 3
[resource.blacklist]
water
";

    #[test]
    fn takes_the_highest_weighted_wanted_option() {
        let c = cfg(BASIC);
        let d = decide(&c, &ids(&["ore", "metal", "clay"]), &eco(RerollCost::Unavailable),
                       false, &mut first);
        assert_eq!(d, Decision::Pick("metal".into()));
    }

    #[test]
    fn wanted_beats_fallback_regardless_of_weight() {
        // clay is weighted 7, ore only 5 -- but ore is wanted, so ore wins
        let c = cfg(BASIC);
        let d = decide(&c, &ids(&["ore", "clay"]), &eco(RerollCost::Unavailable),
                       false, &mut first);
        assert_eq!(d, Decision::Pick("ore".into()), "tier must dominate weight");
    }

    #[test]
    fn blacklisted_options_are_never_taken_but_do_not_block() {
        let c = cfg(BASIC);
        // water is blacklisted; metal is still taken
        let d = decide(&c, &ids(&["water", "metal"]), &eco(RerollCost::Unavailable),
                       false, &mut first);
        assert_eq!(d, Decision::Pick("metal".into()));

        // nothing but blacklisted, no budget -> manual, never the blacklisted one
        let d = decide(&c, &ids(&["water"]), &eco(RerollCost::Unavailable), false, &mut first);
        assert!(matches!(d, Decision::Manual(_)));
    }

    /// `-1` means "spend what the game gives me", not "reroll forever".
    #[test]
    fn unlimited_depth_still_obeys_the_real_economy() {
        let mut c = cfg(BASIC);
        c.budgets = Budgets {
            voodoo_depth: Depth::Unlimited,
            free_depth: Depth::Unlimited,
            paid_depth: Depth::Unlimited,
            denarii_floor: 100,
        };

        // however many have been spent, another free one is still allowed
        let mut e = eco(RerollCost::Free);
        e.free_used = 9_999;
        assert_eq!(may_reroll(&c, &e), RerollVerdict::Allowed(RerollCost::Free));

        // but a reroll the game will not offer is still not available
        assert_eq!(
            may_reroll(&c, &eco(RerollCost::Unavailable)),
            RerollVerdict::Denied("no reroll available")
        );

        // and paid rerolls still stop at the floor and at affordability
        let mut e = eco(RerollCost::Paid(50));
        e.denarii = 120;
        assert_eq!(may_reroll(&c, &e), RerollVerdict::Denied("would breach denarii_floor"));

        let mut e = eco(RerollCost::Paid(500));
        e.denarii = 120;
        assert_eq!(may_reroll(&c, &e), RerollVerdict::Denied("cannot afford it"));
    }

    /// Unlimited on one kind must not spill into another, exactly as a number
    /// on one does not.
    #[test]
    fn unlimited_does_not_fall_through_to_another_budget() {
        let mut c = cfg(BASIC);
        c.budgets = Budgets { free_depth: Depth::Unlimited, ..Default::default() };
        assert_eq!(
            may_reroll(&c, &eco(RerollCost::Voodoo)),
            RerollVerdict::Denied("voodoo_depth spent")
        );
        assert_eq!(
            may_reroll(&c, &eco(RerollCost::Paid(1))),
            RerollVerdict::Denied("paid_depth spent")
        );
    }

    #[test]
    fn fallback_is_only_reached_once_rerolls_are_spent() {
        let mut c = cfg(BASIC);
        c.budgets = Budgets { free_depth: Depth::Limit(1), ..Default::default() };

        // a reroll is available and budgeted: reroll rather than settle
        let d = decide(&c, &ids(&["clay"]), &eco(RerollCost::Free), false, &mut first);
        assert_eq!(d, Decision::Reroll, "must not settle while budget remains");

        // budget spent: now settle on the best fallback
        let spent = Economy { free_used: 1, ..eco(RerollCost::Free) };
        let d = decide(&c, &ids(&["wheat", "clay"]), &spent, false, &mut first);
        assert_eq!(d, Decision::Pick("clay".into()), "highest-weighted fallback");
    }

    #[test]
    fn unknown_offered_id_falls_closed() {
        let c = cfg(BASIC);
        let d = decide(&c, &ids(&["metal", "unobtainium"]), &eco(RerollCost::Unavailable),
                       false, &mut first);
        assert!(matches!(d, Decision::Manual(_)),
                "an unclassified option must stop the mod, even alongside a wanted one");
    }

    #[test]
    fn scrap_is_unreachable_when_the_type_cannot_scrap() {
        let c = cfg("
[resource]
[resource.wanted]
metal = 1
[resource.fallback]
_scrap = 5
[resource.blacklist]
water
");
        // scrap_available = false: unit_class_stat has no scrap button
        let d = decide(&c, &ids(&["water"]), &eco(RerollCost::Unavailable), false, &mut first);
        assert!(matches!(d, Decision::Manual(_)), "must not press a button that is not there");

        let d = decide(&c, &ids(&["water"]), &eco(RerollCost::Unavailable), true, &mut first);
        assert_eq!(d, Decision::Pick(SCRAP.into()), "and must use it when it is there");
    }

    // ---- the reroll economy

    #[test]
    fn budgets_do_not_fall_through_to_another_class() {
        // the spec's worked example: free_depth 1, paid_depth 1, and the game
        // says the next reroll is free. Once free_depth is spent, rerolling
        // stops -- paid_depth is unreachable while free rerolls remain banked.
        let mut c = cfg(BASIC);
        c.budgets = Budgets { free_depth: Depth::Limit(1), paid_depth: Depth::Limit(1), ..Default::default() };

        let fresh = eco(RerollCost::Free);
        assert_eq!(may_reroll(&c, &fresh), RerollVerdict::Allowed(RerollCost::Free));

        let spent = Economy { free_used: 1, ..fresh };
        assert_eq!(may_reroll(&c, &spent), RerollVerdict::Denied("free_depth spent"),
                   "must not upgrade to the paid budget");
    }

    #[test]
    fn voodoo_is_budgeted_separately_from_the_run_pool() {
        let mut c = cfg(BASIC);
        c.budgets = Budgets { voodoo_depth: Depth::Limit(1), free_depth: Depth::None, ..Default::default() };

        assert_eq!(may_reroll(&c, &eco(RerollCost::Voodoo)),
                   RerollVerdict::Allowed(RerollCost::Voodoo));
        // the run pool is separately zero, so a free reroll is refused
        assert_eq!(may_reroll(&c, &eco(RerollCost::Free)),
                   RerollVerdict::Denied("free_depth spent"));
    }

    #[test]
    fn paid_rerolls_respect_affordability_and_the_floor() {
        let mut c = cfg(BASIC);
        c.budgets = Budgets { paid_depth: Depth::Limit(3), denarii_floor: 500, ..Default::default() };

        let e = Economy { denarii: 1000, ..eco(RerollCost::Paid(100)) };
        assert_eq!(may_reroll(&c, &e), RerollVerdict::Allowed(RerollCost::Paid(100)));

        // would drop below the floor
        let e = Economy { denarii: 550, ..eco(RerollCost::Paid(100)) };
        assert_eq!(may_reroll(&c, &e), RerollVerdict::Denied("would breach denarii_floor"));

        // cannot afford at all
        let e = Economy { denarii: 50, ..eco(RerollCost::Paid(100)) };
        assert_eq!(may_reroll(&c, &e), RerollVerdict::Denied("cannot afford it"));
    }

    #[test]
    fn an_unavailable_reroll_is_never_permitted() {
        let mut c = cfg(BASIC);
        c.budgets = Budgets { voodoo_depth: Depth::Limit(9), free_depth: Depth::Limit(9), paid_depth: Depth::Limit(9),
                              denarii_floor: 0 };
        assert_eq!(may_reroll(&c, &eco(RerollCost::Unavailable)),
                   RerollVerdict::Denied("no reroll available"));
    }

    // ---- tie-breaking

    #[test]
    fn equal_weights_are_broken_between_all_tied_options() {
        let c = cfg("
[resource]
[resource.wanted]
metal = 10
ore = 10
gold = 10
[resource.blacklist]
water
");
        let offered = ids(&["metal", "ore", "gold"]);
        let mut seen = std::collections::HashSet::new();
        let mut rng = TieBreaker::new(12345);
        for _ in 0..500 {
            let d = decide(&c, &offered, &eco(RerollCost::Unavailable), false,
                           &mut |n| rng.next(n));
            if let Decision::Pick(id) = d {
                seen.insert(id);
            }
        }
        assert_eq!(seen.len(), 3, "every tied option must be reachable, got {seen:?}");
    }

    #[test]
    fn tiebreaker_is_deterministic_for_a_given_seed() {
        let mut a = TieBreaker::new(7);
        let mut b = TieBreaker::new(7);
        let xs: Vec<usize> = (0..20).map(|_| a.next(5)).collect();
        let ys: Vec<usize> = (0..20).map(|_| b.next(5)).collect();
        assert_eq!(xs, ys);
        assert!(xs.iter().all(|&v| v < 5));
    }
}
