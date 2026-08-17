//! The config file.
//!
//! Line-oriented INI, hand-edited by the player. One section per reward type,
//! three tier sub-sections per type, and a `[global]` section.
//!
//! ```ini
//! [global]
//! enabled  = true
//! delay_ms = 100
//!
//! [resource]
//! voodoo_depth  = 1
//! free_depth    = 2
//! paid_depth    = 0
//! denarii_floor = 0
//!
//! [resource.wanted]
//! metal = 10
//! ore   = 10          # indifferent between these two
//!
//! [resource.fallback]
//! clay   = 7
//! _scrap = 1
//!
//! [resource.blacklist]
//! water
//! ```
//!
//! **Completeness is required.** For any type with a section, every id in that
//! type's vocabulary must appear in exactly one tier. There is no default for
//! an unlisted id -- the mod generates a complete file, so the player is
//! classifying from a list rather than from memory, and an id that appears
//! later (a game update) is a loud error rather than a silent guess.

use std::collections::HashMap;
use std::path::Path;

/// The pseudo-option for scrapping the reward. Reserved so it cannot collide
/// with a game id.
pub const SCRAP: &str = "_scrap";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Take immediately; highest weight wins.
    Wanted,
    /// Never while reroll budget remains; once it is spent, highest weight wins.
    Fallback,
    /// Never, at any depth.
    Blacklist,
}

#[derive(Debug, Clone, Default)]
pub struct Budgets {
    /// Rerolls taken via the per-reward freebie (Voodoo Beads).
    pub voodoo_depth: Depth,
    /// Rerolls taken from the reign-wide free pool.
    pub free_depth: Depth,
    /// Rerolls paid for in denarii.
    pub paid_depth: Depth,
    /// Never reroll if it would leave the balance below this.
    pub denarii_floor: i64,
}

/// How many rerolls of one kind the mod may spend on a single reward.
///
/// `-1` in the file means **as many as the game will give**, which is not the
/// same as unbounded. The real economy still applies: free rerolls run out,
/// paid ones stop at what the player can afford and at `denarii_floor`, and a
/// reroll the game will not offer is not one the mod can take. So `-1` says
/// "spend whatever budget I have" rather than "keep going forever".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Depth {
    #[default]
    None,
    Limit(u32),
    Unlimited,
}

impl Depth {
    /// Whether one more reroll is within this budget, having already made `used`.
    pub fn allows(self, used: u32) -> bool {
        match self {
            Depth::None => false,
            Depth::Limit(n) => used < n,
            Depth::Unlimited => true,
        }
    }

    /// Any negative value means unlimited, so `-1` reads naturally and a stray
    /// `-2` does not become a silent surprise.
    pub fn parse(n: i64) -> Depth {
        match n {
            n if n < 0 => Depth::Unlimited,
            0 => Depth::None,
            n => Depth::Limit(n.min(u32::MAX as i64) as u32),
        }
    }
}

impl std::fmt::Display for Depth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Depth::None => write!(f, "0"),
            Depth::Limit(n) => write!(f, "{n}"),
            Depth::Unlimited => write!(f, "unlimited"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TypeConfig {
    pub budgets: Budgets,
    /// id -> (tier, weight). Weight orders *within* a tier only.
    pub options: HashMap<String, (Tier, f64)>,
}

impl TypeConfig {
    pub fn tier_of(&self, id: &str) -> Option<Tier> {
        self.options.get(id).map(|(t, _)| *t)
    }

    pub fn weight_of(&self, id: &str) -> Option<f64> {
        self.options.get(id).map(|(_, w)| *w)
    }
}

#[derive(Debug, Clone)]
pub struct Global {
    pub enabled: bool,
    pub delay_ms: u64,
    /// Whether the mod may actually press buttons.
    ///
    /// Defaults to **false**: with `act = false` it decides and logs
    /// `[would PICK] ...` but touches nothing, which is the safe way to check
    /// that its choices match yours before letting it act on them.
    pub act: bool,
    /// Log each phase as it starts, so a crash names the operation in flight
    /// rather than the last one that finished. Verbose; for diagnosis only.
    pub trace: bool,
    /// The periodic diagnostic sweep over the whole reward UI.
    ///
    /// Off by default. It is how the mod was developed and it is what a bug
    /// report should carry, but it is pure cost to a player: it walks every
    /// card, button and library on a timer, and it is the most expensive thing
    /// the mod does by a wide margin. Picking, rerolling and opening the queue
    /// do not use it.
    pub survey: bool,
}

impl Default for Global {
    fn default() -> Self {
        Global { enabled: true, delay_ms: 100, act: false, trace: false, survey: false }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub global: Global,
    /// Only types with a section. A type absent here is fully manual.
    pub types: HashMap<String, TypeConfig>,
    /// Problems found while parsing, for the log. A type with any error is
    /// dropped rather than half-applied.
    pub errors: Vec<String>,
}

impl Config {
    pub fn for_type(&self, reward_type: &str) -> Option<&TypeConfig> {
        self.types.get(reward_type)
    }

    pub fn load(path: &Path) -> std::io::Result<Config> {
        Ok(parse(&std::fs::read_to_string(path)?))
    }
}

/// Parse the config text. Never fails outright: problems are collected in
/// `errors` and the offending sections dropped, so one bad type cannot disable
/// the rest.
pub fn parse(text: &str) -> Config {
    let mut cfg = Config::default();
    let mut section: Option<(String, Option<Tier>)> = None;
    // collected per type so a section with errors can be dropped whole
    let mut staged: HashMap<String, (TypeConfig, Vec<String>)> = HashMap::new();

    for (n, raw) in text.lines().enumerate() {
        let line_no = n + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let name = name.trim();
            match parse_section(name) {
                Some(s) => section = Some(s),
                None => {
                    cfg.errors.push(format!("line {line_no}: unknown section [{name}]"));
                    section = None;
                }
            }
            if let Some((ty, _)) = &section {
                if ty != "global" {
                    staged.entry(ty.clone()).or_default();
                }
            }
            continue;
        }

        let Some((ty, tier)) = section.clone() else {
            cfg.errors.push(format!("line {line_no}: setting outside any section"));
            continue;
        };

        if ty == "global" {
            if let Err(e) = apply_global(&mut cfg.global, line) {
                cfg.errors.push(format!("line {line_no}: {e}"));
            }
            continue;
        }

        let entry = staged.entry(ty.clone()).or_default();
        match tier {
            None => {
                if let Err(e) = apply_budget(&mut entry.0.budgets, line) {
                    entry.1.push(format!("line {line_no}: {e}"));
                }
            }
            Some(t) => {
                if t != Tier::Blacklist && !line.contains('=') {
                    cfg.errors.push(format!(
                        "line {line_no}: {line} in [{ty}.{}] had no weight; written back                          as `= 0`, the lowest. Change the number to rank it.",
                        if t == Tier::Wanted { "wanted" } else { "fallback" }
                    ));
                }
                let (id, weight) = split_option(line, t);
                if id.is_empty() {
                    entry.1.push(format!("line {line_no}: empty option id"));
                    continue;
                }
                match weight {
                    Err(e) => entry.1.push(format!("line {line_no}: {e}")),
                    Ok(w) => {
                        if let Some((prev, _)) = entry.0.options.get(&id) {
                            entry.1.push(format!(
                                "line {line_no}: {id} is listed twice ({prev:?} and {t:?}); \
                                 every id must be in exactly one tier"
                            ));
                        } else {
                            entry.0.options.insert(id, (t, w));
                        }
                    }
                }
            }
        }
    }

    for (ty, (conf, errs)) in staged {
        if errs.is_empty() {
            cfg.types.insert(ty, conf);
        } else {
            cfg.errors
                .push(format!("[{ty}] dropped, {} problem(s):", errs.len()));
            cfg.errors.extend(errs.into_iter().map(|e| format!("    {e}")));
        }
    }
    cfg
}

/// Check a parsed type against the vocabulary it must cover.
///
/// Returns the problems; empty means the section is complete and usable.
pub fn check_complete(ty: &str, conf: &TypeConfig, vocabulary: &[&str], can_scrap: bool)
    -> Vec<String>
{
    let mut problems = Vec::new();
    for id in vocabulary {
        if !conf.options.contains_key(*id) {
            problems.push(format!("[{ty}] missing id: {id}"));
        }
    }
    for id in conf.options.keys() {
        if id == SCRAP {
            if !can_scrap {
                problems.push(format!(
                    "[{ty}] ranks {SCRAP}, but this reward type has no scrap button"
                ));
            }
            continue;
        }
        if !vocabulary.contains(&id.as_str()) {
            problems.push(format!("[{ty}] unknown id: {id}"));
        }
    }
    problems
}


/// One reward type's generation input.
pub struct TypeVocab<'a> {
    pub reward_type: &'a str,
    pub display: &'a str,
    /// Every option id, with its display name where the game has one.
    pub options: Vec<(String, String)>,
    pub can_scrap: bool,
    /// One line saying where this list came from, so the player can judge it
    /// rather than take it on trust.
    pub note: String,
}

/// Generate a complete, **inert** config.
///
/// Every id goes in the blacklist and every budget is zero, so a freshly
/// installed mod resolves nothing at all: installing without configuring
/// changes nothing about the game. The file doubles as the reference for what
/// is configurable, so the player edits lines that already exist rather than
/// typing ids from memory -- which is what makes the completeness requirement
/// affordable.
///
/// Built from the vocabularies read out of the *running game*, so it is correct
/// for the installed build rather than for whatever version was analysed.
pub fn generate(types: &[TypeVocab], excluded: &[(String, String)]) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    for t in types {
        let rt = t.reward_type;
        out.push_str(&format!(
            "\n\n# --------------------------------------------------------------------------\n\
             # {}   [{rt}]   {} options\n",
            t.display,
            t.options.len()
        ));
        if !t.note.is_empty() {
            out.push_str(&format!("# {}\n", t.note));
        }
        out.push_str("# --------------------------------------------------------------------------\n\n");
        out.push_str(&format!("[{rt}]\n"));
        out.push_str(
            "voodoo_depth  = 0    # rerolls using the per-reward freebie (Voodoo Beads)\n\
             free_depth    = 0    # rerolls from the reign-wide free pool\n\
             paid_depth    = 0    # rerolls paid for in denarii\n\
             denarii_floor = 0    # never reroll below this balance\n\
             #                    any depth may be -1: as many as the game allows\n",
        );
        out.push_str(&format!("\n[{rt}.wanted]\n"));
        out.push_str(&format!("\n[{rt}.fallback]\n"));
        if t.can_scrap {
            out.push_str("# _scrap = 1     # settle for scrapping the reward (+5 denarii)\n");
        } else {
            out.push_str("# this reward type has no scrap button, so _scrap is unavailable\n");
        }
        out.push_str(&format!("\n[{rt}.blacklist]\n"));
        let width = t.options.iter().map(|(i, _)| i.len()).max().unwrap_or(0);
        for (id, display) in &t.options {
            if display.is_empty() {
                out.push_str(&format!("{id}\n"));
            } else {
                out.push_str(&format!("{id:<width$}   # {display}\n"));
            }
        }
    }

    if !excluded.is_empty() {
        out.push_str(&format!(
            "\n\n# ==========================================================================\n\
             # FOR INFORMATION ONLY -- nothing below here is configurable\n\
             # ==========================================================================\n\
             #\n\
             # The game marks these {} improvements `excluded_from_drop_pool`,\n\
             # meaning they can never be offered as a reward, so they are left out of\n\
             # the improvement sections. They are listed here so that exclusion can be\n\
             # checked: if you recognise one you HAVE been offered, the filter is wrong\n\
             # and worth saying so.\n\
             #\n",
            excluded.len()
        ));
        let width = excluded.iter().map(|(i, _)| i.len()).max().unwrap_or(0);
        for (id, display) in excluded {
            out.push_str(&format!("#   {id:<width$}   {display}\n"));
        }
    }
    out
}

/// The preamble: what the file does, and everything in `[global]`.
const HEADER: &str = r#"# tkiw auto reward picker -- configuration
#
# As generated, this file does NOTHING: every option is blacklisted and every
# reroll budget is zero, so the mod stays out of the way until you edit it.
#
# To automate a reward type, move ids out of [<type>.blacklist] into
#   [<type>.wanted]    take it immediately; highest weight wins, ties at random
#   [<type>.fallback]  only once the reroll budget is spent; highest weight wins
# and leave in [<type>.blacklist] anything you never want taken.
#
# Weights order options WITHIN a tier only: a wanted option always beats a
# fallback one, whatever the numbers say. Equal weights mean you are indifferent.
#
# Every id must appear in exactly one tier. Delete a whole [<type>] section to
# leave that reward type entirely manual -- that is also what happens to any
# type not mentioned here.
#
# The reward at the head of the queue is the only one ever touched, and the mod
# stops as soon as it reaches one it is not configured for.
#
# This file was generated from the libraries of the game it is installed in, so
# the lists describe your build. Delete it and it will be written again.
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
# Ctrl+Alt+P toggles this while you play, and the toggle is WRITTEN BACK to
# this line, so what you chose in game is what the next launch does. Editing
# it here also takes effect immediately -- the file is re-read as you save it.
#
# Either way the mod keeps reading choices and logging what it would do, so
# with pressing off you get a running commentary rather than silence.
act      = false

# trace = true logs each phase as it begins, so if the game crashes the last
# line names what was underway rather than what last finished. Verbose -- turn
# it on only when chasing a crash. It is NOT needed for a crash report: the
# faulting address and the operation in flight are always recorded.
trace    = false

# survey = true adds a periodic sweep over the whole reward UI -- every card,
# button and library -- to the log. It is what a bug report should carry, and
# it is the most expensive thing the mod does. Picking, rerolling and opening
# the queue do not use it, so leave it off unless you are diagnosing something.
survey   = false
"#;

/// Give every unvalued option in `wanted` or `fallback` an explicit `= 0`.
///
/// The mod already *treats* a bare id there as weight 0, but leaving the file
/// saying one thing and the mod meaning another is a trap: the player moves an
/// id up from the blacklist, sees no number, and has no reason to think one is
/// implied. Writing it back makes the file say what the mod will do, and gives
/// them something to edit rather than something to remember.
///
/// Bare ids in `blacklist` are left exactly as they are -- ordering means
/// nothing there, and a weight would be noise.
///
/// Returns `None` when there was nothing to change, so the caller can leave the
/// file alone rather than rewriting it identically.
/// Written this way rather than as a character escape: this file is edited by
/// scripts often enough that a bare `\n` has already been mangled into a real
/// newline once, and a char literal is where that fails loudest.
const NL: char = '\u{000A}';

pub fn normalise(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len() + 64);
    let mut tier: Option<Tier> = None;
    let mut changed = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            tier = parse_section(name).and_then(|(_, t)| t);
            out.push_str(line);
            out.push(NL);
            continue;
        }

        let body = strip_comment(line).trim_end();
        let needs = matches!(tier, Some(Tier::Wanted) | Some(Tier::Fallback))
            && !body.trim().is_empty()
            && !trimmed.starts_with('#')
            && !body.contains('=');

        if !needs {
            out.push_str(line);
            out.push(NL);
            continue;
        }

        changed = true;
        // Keep the id's own indentation and anything written after it.
        out.push_str(body);
        out.push_str(" = 0");
        if let Some(i) = line.find('#') {
            out.push_str("   ");
            out.push_str(&line[i..]);
        }
        out.push(NL);
    }

    // `lines()` drops a trailing newline; only restore one if it was there.
    if !text.ends_with(NL) {
        out.pop();
    }
    changed.then_some(out)
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn parse_section(name: &str) -> Option<(String, Option<Tier>)> {
    if name.eq_ignore_ascii_case("global") {
        return Some(("global".to_string(), None));
    }
    match name.split_once('.') {
        None => Some((name.to_string(), None)),
        Some((ty, tier)) => {
            let t = match tier.trim().to_ascii_lowercase().as_str() {
                "wanted" => Tier::Wanted,
                "fallback" => Tier::Fallback,
                "blacklist" => Tier::Blacklist,
                _ => return None,
            };
            Some((ty.trim().to_string(), Some(t)))
        }
    }
}

fn kv(line: &str) -> Option<(&str, &str)> {
    line.split_once('=').map(|(k, v)| (k.trim(), v.trim()))
}

fn apply_global(g: &mut Global, line: &str) -> Result<(), String> {
    let Some((k, v)) = kv(line) else {
        return Err(format!("expected `key = value`, got {line:?}"));
    };
    match k {
        "enabled" => {
            g.enabled = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
            Ok(())
        }
        "trace" => {
            g.trace = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
            Ok(())
        }
        "survey" => {
            g.survey = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
            Ok(())
        }
        "act" => {
            g.act = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
            Ok(())
        }
        "delay_ms" => {
            g.delay_ms = v.parse().map_err(|_| format!("delay_ms: {v:?} is not a number"))?;
            Ok(())
        }
        other => Err(format!("unknown setting in [global]: {other}")),
    }
}

fn apply_budget(b: &mut Budgets, line: &str) -> Result<(), String> {
    let Some((k, v)) = kv(line) else {
        return Err(format!("expected `key = value`, got {line:?}"));
    };
    let num = |v: &str| v.parse::<i64>().map_err(|_| format!("{k}: {v:?} is not a number"));
    match k {
        "voodoo_depth" => b.voodoo_depth = Depth::parse(num(v)?),
        "free_depth" => b.free_depth = Depth::parse(num(v)?),
        "paid_depth" => b.paid_depth = Depth::parse(num(v)?),
        "denarii_floor" => b.denarii_floor = num(v)?,
        other => return Err(format!("unknown setting: {other}")),
    }
    Ok(())
}

/// `id = weight`, or a bare `id` where the weight is left off.
///
/// The tier no longer changes the parse -- a bare id is taken at weight 0
/// everywhere, and the caller warns when that happens outside the blacklist.
fn split_option(line: &str, _tier: Tier) -> (String, Result<f64, String>) {
    match kv(line) {
        Some((id, w)) => {
            let parsed = w
                .parse::<f64>()
                .map_err(|_| format!("{id}: weight {w:?} is not a number"));
            (id.to_string(), parsed)
        }
        None => {
            // A bare id is legal in the blacklist, where ordering means nothing.
            // Elsewhere it is almost always someone moving an id up out of the
            // blacklist and forgetting the weight -- so take it at weight 0
            // (lowest in its tier, which is the cautious reading) and warn,
            // rather than dropping the whole section over a missing number.
            (line.to_string(), Ok(0.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tiers_weights_and_budgets() {
        let c = parse("
[global]
enabled = true
delay_ms = 250

[resource]
voodoo_depth = 1
free_depth = 2
paid_depth = 0
denarii_floor = 300

[resource.wanted]
metal = 10
ore = 10

[resource.fallback]
clay = 7
_scrap = 1

[resource.blacklist]
water
");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(c.global.delay_ms, 250);
        let r = c.for_type("resource").unwrap();
        assert_eq!(r.budgets.voodoo_depth, Depth::Limit(1));
        assert_eq!(r.budgets.free_depth, Depth::Limit(2));
        assert_eq!(r.budgets.denarii_floor, 300);
        assert_eq!(r.tier_of("metal"), Some(Tier::Wanted));
        assert_eq!(r.weight_of("metal"), Some(10.0));
        assert_eq!(r.tier_of("clay"), Some(Tier::Fallback));
        assert_eq!(r.tier_of("water"), Some(Tier::Blacklist));
        assert_eq!(r.tier_of(SCRAP), Some(Tier::Fallback));
    }

    #[test]
    fn a_type_absent_from_the_config_is_fully_manual() {
        let c = parse("[resource]\n[resource.wanted]\nmetal = 1\n");
        assert!(c.for_type("unit_class_stat").is_none());
    }

    #[test]
    fn an_id_in_two_tiers_is_an_error_and_drops_only_that_type() {
        let c = parse("
[resource]
[resource.wanted]
metal = 10
[resource.blacklist]
metal

[spell]
[spell.wanted]
freeze = 1
");
        assert!(c.for_type("resource").is_none(), "the broken type must be dropped");
        assert!(c.for_type("spell").is_some(), "a good type must survive its neighbour");
        assert!(c.errors.iter().any(|e| e.contains("listed twice")), "{:?}", c.errors);
    }

    /// Moving an id up out of the blacklist and forgetting the weight is the
    /// easy mistake -- bare ids are legal there. It warns and takes the id at
    /// weight 0 rather than dropping the whole section over a missing number.
    #[test]
    fn a_missing_weight_warns_but_keeps_the_section() {
        let c = parse("[resource]\n[resource.wanted]\nmetal\nore = 5\n");
        let conf = c.for_type("resource").expect("the section must survive");
        assert_eq!(conf.tier_of("metal"), Some(Tier::Wanted));
        assert_eq!(conf.weight_of("metal"), Some(0.0), "lowest in its tier");
        assert_eq!(conf.weight_of("ore"), Some(5.0));
        assert!(c.errors.iter().any(|e| e.contains("no weight")), "{:?}", c.errors);
    }

    /// ...and weight 0 really is lowest, so the warned id loses to a ranked one.
    #[test]
    fn an_unweighted_id_ranks_below_a_weighted_one() {
        let c = parse("[resource]\n[resource.wanted]\nmetal\nore = 5\n");
        let conf = c.for_type("resource").unwrap();
        assert!(conf.weight_of("ore") > conf.weight_of("metal"));
    }

    #[test]
    fn bare_ids_stay_legal_in_the_blacklist() {
        let c = parse("[resource]\n[resource.blacklist]\nwater\nwine\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let conf = c.for_type("resource").unwrap();
        assert_eq!(conf.tier_of("water"), Some(Tier::Blacklist));
        assert_eq!(conf.tier_of("wine"), Some(Tier::Blacklist));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let c = parse("
# leading comment
[resource]      # trailing
free_depth = 2  # how many

[resource.wanted]
metal = 10      # the good one
");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(c.for_type("resource").unwrap().budgets.free_depth, Depth::Limit(2));
        assert_eq!(c.for_type("resource").unwrap().weight_of("metal"), Some(10.0));
    }

    #[test]
    fn completeness_is_enforced_against_the_vocabulary() {
        let c = parse("
[resource]
[resource.wanted]
metal = 10
[resource.blacklist]
water
");
        let conf = c.for_type("resource").unwrap();
        let vocab = ["metal", "water", "ore"];
        let problems = check_complete("resource", conf, &vocab, true);
        assert!(problems.iter().any(|p| p.contains("missing id: ore")), "{problems:?}");
    }

    #[test]
    fn an_id_outside_the_vocabulary_is_reported() {
        let c = parse("
[resource]
[resource.wanted]
metal = 10
unobtainium = 5
");
        let conf = c.for_type("resource").unwrap();
        let problems = check_complete("resource", conf, &["metal"], true);
        assert!(problems.iter().any(|p| p.contains("unknown id: unobtainium")), "{problems:?}");
    }

    #[test]
    fn ranking_scrap_where_it_does_not_exist_is_reported() {
        let c = parse("[unit_class_stat]\n[unit_class_stat.fallback]\n_scrap = 1\n");
        let conf = c.for_type("unit_class_stat").unwrap();
        let problems = check_complete("unit_class_stat", conf, &[], false);
        assert!(problems.iter().any(|p| p.contains("no scrap button")), "{problems:?}");
        // and is perfectly fine where it does exist
        assert!(check_complete("resource", conf, &[], true).is_empty());
    }
}

#[cfg(test)]
mod generate_tests {
    use super::*;

    fn vocab() -> Vec<TypeVocab<'static>> {
        vec![
            TypeVocab {
                reward_type: "resource",
                display: "Resource",
                options: vec![
                    ("metal".into(), "Metal".into()),
                    ("water".into(), "Water".into()),
                ],
                can_scrap: true,
                note: String::new(),
            },
            TypeVocab {
                reward_type: "unit_class_stat",
                display: "Troops Training",
                options: vec![("ranged.damage".into(), "Ranged damage".into())],
                can_scrap: false,
                note: String::new(),
            },
        ]
    }

    /// The whole point of the generated file: installing without configuring
    /// must change nothing about the game.
    #[test]
    fn unvalued_options_are_written_back_with_a_weight() {
        let before = "[resource]
free_depth = 1

[resource.wanted]
metal = 10
ore
gold        # shiny

[resource.fallback]
clay

[resource.blacklist]
sand
stone       # never
";
        let after = normalise(before).expect("there was something to fix");
        assert!(after.contains("ore = 0
"), "{after}");
        assert!(after.contains("gold = 0   # shiny
"), "{after}");
        assert!(after.contains("clay = 0
"), "{after}");
        // untouched: weights already given, blacklist entries, and everything else
        assert!(after.contains("metal = 10
"), "{after}");
        assert!(after.contains("
sand
"), "{after}");
        assert!(after.contains("stone       # never
"), "{after}");
        assert!(after.contains("free_depth = 1
"), "{after}");

        // and the rewritten file parses to exactly what the old one meant
        let a = parse(before);
        let b = parse(&after);
        for id in ["ore", "gold", "clay", "metal", "sand"] {
            let ta = a.for_type("resource").unwrap();
            let tb = b.for_type("resource").unwrap();
            assert_eq!(ta.tier_of(id), tb.tier_of(id), "{id} changed tier");
            assert_eq!(ta.weight_of(id), tb.weight_of(id), "{id} changed weight");
        }
    }

    /// Once fixed, it must stay fixed -- or the mod rewrites the file forever.
    #[test]
    fn normalising_twice_changes_nothing_the_second_time() {
        let once = normalise("[a.wanted]
x
").expect("first pass changes it");
        assert_eq!(normalise(&once), None, "second pass must find nothing to do");
    }

    #[test]
    fn a_file_that_needs_nothing_is_left_alone() {
        assert_eq!(normalise("[a.wanted]
x = 3

[a.blacklist]
y
"), None);
    }

    #[test]
    fn minus_one_means_as_many_as_the_game_allows() {
        let c = parse(
            "[resource]
voodoo_depth = -1
free_depth = 0
paid_depth = 2
             [resource.blacklist]
clay
",
        );
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let b = &c.for_type("resource").unwrap().budgets;
        assert_eq!(b.voodoo_depth, Depth::Unlimited);
        assert_eq!(b.free_depth, Depth::None);
        assert_eq!(b.paid_depth, Depth::Limit(2));
        assert!(b.voodoo_depth.allows(1_000_000));
        assert!(!b.free_depth.allows(0));
        assert!(b.paid_depth.allows(1) && !b.paid_depth.allows(2));
    }

    #[test]
    fn the_generated_config_is_inert() {
        let text = generate(&vocab(), &[]);
        let c = parse(&text);
        assert!(c.errors.is_empty(), "generated config must parse cleanly: {:?}", c.errors);

        for ty in ["resource", "unit_class_stat"] {
            let conf = c.for_type(ty).expect(ty);
            assert_eq!(conf.budgets.voodoo_depth, Depth::None);
            assert_eq!(conf.budgets.free_depth, Depth::None);
            assert_eq!(conf.budgets.paid_depth, Depth::None);
            assert!(
                conf.options.values().all(|(t, _)| *t == Tier::Blacklist),
                "every id must start blacklisted"
            );
        }
    }

    /// It is also the reference for what is configurable, so it must be complete.
    #[test]
    fn the_generated_config_is_complete() {
        let v = vocab();
        let c = parse(&generate(&v, &[]));
        for t in &v {
            let conf = c.for_type(t.reward_type).unwrap();
            let ids: Vec<&str> = t.options.iter().map(|(i, _)| i.as_str()).collect();
            let problems = check_complete(t.reward_type, conf, &ids, t.can_scrap);
            assert!(problems.is_empty(), "{}: {problems:?}", t.reward_type);
        }
    }

    #[test]
    fn scrap_is_only_suggested_where_it_exists() {
        let text = generate(&vocab(), &[]);
        let (resource, troops) = text.split_once("[unit_class_stat]").unwrap();
        assert!(resource.contains("# _scrap = 1"), "resource can scrap");
        assert!(troops.contains("no scrap button"), "unit_class_stat cannot");
        assert!(!troops.contains("# _scrap = 1"));
    }

    /// The user asked for the never-dropping improvements to be visible so the
    /// filtering can be checked. Visible, but inert: they are not choices.
    #[test]
    fn the_information_block_is_only_a_comment() {
        let excluded = vec![("dragon_nest".into(), "Dragon's Nest".into())];
        let text = generate(&vocab(), &excluded);
        assert!(text.contains("dragon_nest"), "it must actually be listed");
        let c = parse(&text);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        for ty in ["resource", "unit_class_stat"] {
            assert!(
                c.for_type(ty).unwrap().tier_of("dragon_nest").is_none(),
                "an excluded id must not become a configurable option"
            );
        }
    }

    #[test]
    fn a_note_says_where_a_list_came_from() {
        let mut v = vocab();
        v[0].note = "tier 1 of 127 artifacts".into();
        let text = generate(&v, &[]);
        assert!(text.contains("# tier 1 of 127 artifacts"));
        assert!(parse(&text).errors.is_empty());
    }

    #[test]
    fn display_names_are_carried_through_as_comments() {
        let text = generate(&vocab(), &[]);
        assert!(text.contains("# Metal"), "ids should be annotated with display names");
        // and the annotation must not change how the file parses
        let c = parse(&text);
        assert_eq!(c.for_type("resource").unwrap().tier_of("metal"), Some(Tier::Blacklist));
    }
}
