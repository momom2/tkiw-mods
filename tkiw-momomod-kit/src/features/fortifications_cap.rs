//! Fortifications stops granting max castle HP once the castle is already too big.
//!
//! ## The bug
//!
//! `upgrade_wall_repairs_fortification` says "Has a cap of 100 HP earned this way", and
//! its data agrees: `max_hp_gained_cap = 100`. Each grant is gated on
//!
//! ```text
//! if (u.hp_gained_current < u.max_hp_gained_cap) { ... increase_max_hp(1) }
//! ```
//!
//! `hp_gained_current` is set to 0 by `upgrades_library` and **incremented nowhere** --
//! four references to its variable slot in the whole of `.text`, of which one is the
//! initialiser and three are reads. The gate is therefore always open, and each Brick
//! Factory grants +1 max castle HP every 5 cycles for as long as the castle is
//! undamaged. `castle_hp` is saved, so it compounds for the whole run.
//!
//! Full write-up: `for-the-developers/brick-factory-fortifications.md`.
//!
//! ## Scope
//!
//! **In scope:** stopping Brick Factories accumulating more once the castle is past what
//! the game intends. **Out of scope by decision:** taking back HP already gained. This
//! feature never lowers `hp_max`.
//!
//! ## Where the ceiling comes from
//!
//! Every source of max castle HP in the game, found by scanning `.text` for calls to
//! `increase_max_hp` and reading the constant each one passes. All six are fixed:
//!
//! | source | amount | how this reads it |
//! |---|---|---|
//! | `obj_castle_wall` | 100 | always applies |
//! | stone castle | 20 | `is_stone` on the castle |
//! | advisor `branimir` | 100 | `ADVISORS_EQUIPPED` |
//! | artifact `emerald_shield` | 30 | `ARTIFACTS_EQUIPPED` |
//! | encounter `happy_accident` | 40 | `ENCOUNTERS_HAPPENED` |
//! | encounter `they_came_unprepared` | 30 | `ENCOUNTERS_HAPPENED` |
//!
//! ```text
//! ceiling = 100 + stone + branimir + emerald + encounters + cap x Brick Factories
//! ```
//!
//! **The allowance scales with the Brick Factories standing in the castle.** One factory
//! may fill 100; two may fill 200. That makes a second factory worth building for its
//! Fortifications value rather than only for its repair rate, which reads the way the
//! rest of the building upgrades do.
//!
//! Demolishing one lowers the ceiling but **not** the HP already earned -- this feature
//! never lowers `hp_max`. So the player keeps what they have and simply cannot earn more
//! until the ceiling is above the castle again. That is what stops
//! build-fill-demolish-rebuild from farming the cap, and it means removing a building is
//! never punished retroactively.
//!
//! The two encounters took a two-level library dump to find. They carry
//! `castle_max_hp_given` on an entry of their `options` array, so a walk that stops at
//! the top level reports -- wrongly -- that no encounter grants castle HP at all.
//!
//! **This needs no history**, which is the whole point: it reads what the run has right
//! now and compares it against `hp_max`. A save that has been running the bug for six
//! thousand days is handled exactly like a fresh one, with nothing carried over and
//! nothing assumed about what happened before the mod was installed.
//!
//! One deliberate looseness, and it is an upper bound rather than a guess: the game
//! records *which encounters happened*, not which of the three options was taken, and
//! the HP is one option. So an encounter that happened counts in full. A player who
//! declined it keeps a ceiling up to 40 higher than strictly earned -- erring towards
//! letting them keep HP, never towards stopping them early.
//!
//! ## The patch
//!
//! One site, applied only while the ceiling is exceeded and reverted when it is not:
//!
//! ```text
//! 013c8542  cmp eax, -2          83 f8 fe
//! 013c8545  je  0x13c8725        0f 84 da 01 00 00     <- to the end of the branch
//! ```
//!
//! Nine verified bytes replaced by a `jmp` to that same end. The destination is read out
//! of the game's own `je` rather than baked in, so the patch cannot disagree with the
//! code it is patching.
//!
//! An earlier version of this feature counted its own grants and stopped at 100. That is
//! right only for a run started after it loaded; on an existing save it cheerfully
//! allowed another hundred on top of the nine hundred already there. There is no counter
//! here and no code cave -- the castle already knows the answer.

use tkiw_runtime::{
    codecave, dslist, guard::Signature, hook, instance, logln, patch::Patch, rvalue,
    rvalue::Value, Runtime,
};

use crate::config::Section;
use crate::feature::{Cadence, Feature, Requirements};

/// The cap test the bug leaves permanently open.
const SITE_GATE: usize = 0x13c8542;
/// `cmp eax, -2` then `je <end of branch>`.
const EXPECT_GATE: &[u8] = &[0x83, 0xf8, 0xfe, 0x0f, 0x84, 0xda, 0x01, 0x00, 0x00];
/// Offset of the `je`'s rel32 within [`EXPECT_GATE`], so the destination is read from
/// the game rather than assumed.
const GATE_REL_AT: usize = 5;

/// The game's own figure, from the upgrade description and from `max_hp_gained_cap`.
///
/// Not configurable. It is the number the game states, and a player who wants a
/// different one wants a different game rather than a bug fixed.
const CAP: f64 = 100.0;

/// `obj_castle_wall` sets this in its Create event (rva `0x11ec924`).
const BASE_HP: f64 = 100.0;
/// `btf_castle`, on `upgraded_to_stone` (rva `0x10c6fca`).
const STONE_HP: f64 = 20.0;
/// Sources outside the castle: `(list global, entry name, amount)`.
const EQUIPPED: &[(&str, &str, f64)] = &[
    ("ADVISORS_EQUIPPED", "branimir", 100.0),
    ("ARTIFACTS_EQUIPPED", "emerald_shield", 30.0),
];
/// Encounters carrying `castle_max_hp_given` on an option of their `options` array.
const ENCOUNTERS: &[(&str, f64)] = &[("happy_accident", 40.0), ("they_came_unprepared", 30.0)];

/// The Brick Factory. Each one standing raises the allowance by [`CAP`] in
/// [`Allowance::PerFactory`].
const FACTORY: &str = "obj_improvement_wall_repairs";

/// The config key, and the two values it accepts.
const MODE_KEY: &str = "cap";
const PER_FACTORY: &str = "per_factory";
const TOTAL: &str = "total";

/// How much Fortifications may add before it stops.
#[derive(Clone, Copy, PartialEq)]
enum Allowance {
    /// [`CAP`] for each Brick Factory standing. A second factory is then worth building
    /// for its Fortifications value, not only its repair rate.
    PerFactory,
    /// [`CAP`] for the run, however many factories there are. The strict reading of what
    /// the upgrade says.
    Total,
}

pub struct FortificationsCap {
    allowance: Allowance,
    gate_patch: Option<Patch>,
    /// Last state reported, so the log says it once rather than four times a second.
    announced: Option<(i64, i64)>,
}

impl Default for FortificationsCap {
    fn default() -> FortificationsCap {
        FortificationsCap {
            allowance: Allowance::PerFactory,
            gate_patch: None,
            announced: None,
        }
    }
}

/// The bytes that force the gate shut: `jmp` to wherever the game's own `je` goes.
fn gate_bytes(site: usize) -> Option<Vec<u8>> {
    let je_at = site + GATE_REL_AT - 2; // the 0f 84 opcode
    let rel = i32::from_le_bytes(EXPECT_GATE[GATE_REL_AT..GATE_REL_AT + 4].try_into().ok()?);
    let dest = (je_at as i64 + 6 + rel as i64) as usize;
    let mut out = codecave::call_rel32(site, dest)?.to_vec();
    out[0] = 0xe9; // the same rel32 form, as a jmp rather than a call
    out.resize(EXPECT_GATE.len(), 0x90);
    Some(out)
}

/// A number out of an RValue, whichever numeric kind it arrived as.
fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Real(x) => Some(*x),
        Value::Int(x) => Some(*x as f64),
        Value::Bool(b) => Some(*b as u8 as f64),
        _ => None,
    }
}

impl FortificationsCap {
    /// Read one variable off the castle instance.
    ///
    /// # Safety
    /// Game thread.
    unsafe fn castle_var(&self, rt: &Runtime, castle: usize, name: &str) -> Option<Value> {
        let id = rt.var_id(name)?;
        let at = instance::get_var(castle, id)?;
        rvalue::decode(at)
    }

    /// Whether a named entry is present in one of the game's lists.
    ///
    /// These globals are a `ds_map` in some cases and a `ds_list` in others, so both are
    /// tried. **Absent reads as not-present**: outside a run they hold nothing useful,
    /// and "cannot tell" must not become "assume it is there", or the ceiling drifts up
    /// on its own and the fix quietly stops fixing anything.
    ///
    /// # Safety
    /// Game thread.
    unsafe fn holds(&self, rt: &Runtime, global: &str, name: &str) -> bool {
        let Some(globals) = rt.globals() else { return false };
        let Some(id) = rt.var_id(global) else { return false };
        let Some(v) = globals.get(id) else { return false };
        if let Some(entries) = dslist::ds_map_entries(rt.base, &v, 512) {
            if entries.iter().any(|(k, _)| k.as_str() == Some(name)) {
                return true;
            }
        }
        match dslist::DsList::from_value(rt.base, &v) {
            Some(list) => (0..list.len())
                .filter_map(|i| list.get(i))
                .any(|e| e.as_str() == Some(name)),
            None => false,
        }
    }

    /// The most max castle HP this run could legitimately have, Fortifications included.
    ///
    /// # Safety
    /// Game thread.
    unsafe fn ceiling(&self, rt: &Runtime, castle: usize) -> (f64, usize) {
        let mut total = BASE_HP;
        let stone = self
            .castle_var(rt, castle, "is_stone")
            .as_ref()
            .and_then(as_number)
            .unwrap_or(0.0);
        if stone != 0.0 {
            total += STONE_HP;
        }
        for (global, name, amount) in EQUIPPED {
            if self.holds(rt, global, name) {
                total += amount;
            }
        }
        for (name, amount) in ENCOUNTERS {
            if self.holds(rt, "ENCOUNTERS_HAPPENED", name) {
                total += amount;
            }
        }
        // Counted live rather than remembered: a factory demolished this second must
        // lower the ceiling this second, or the count becomes a thing to farm.
        let factories = instance::count(rt.base, FACTORY);
        let allowed = match self.allowance {
            Allowance::PerFactory => CAP * factories as f64,
            Allowance::Total => CAP,
        };
        (total + allowed, factories)
    }

    fn close_gate(&mut self, rt: &Runtime) -> Result<(), String> {
        if self.gate_patch.as_ref().is_some_and(|p| p.is_applied()) {
            return Ok(());
        }
        let site = rt.base + SITE_GATE;
        let bytes = gate_bytes(site).ok_or("the gate's destination is out of jmp range")?;
        // SAFETY: on the game's thread, via the frame hook, so the game is inside
        // PeekMessageW and provably not executing this branch.
        let patch = unsafe {
            Patch::apply("wall_repairs_fortification cap gate", site, EXPECT_GATE, &bytes)
        }?;
        self.gate_patch = Some(patch);
        Ok(())
    }

    fn open_gate(&mut self) {
        if let Some(p) = self.gate_patch.as_mut() {
            // SAFETY: same window as closing it.
            let _ = unsafe { p.revert() };
        }
        self.gate_patch = None;
    }
}

impl Feature for FortificationsCap {
    fn name(&self) -> &'static str {
        "fortifications_cap"
    }

    fn module(&self) -> &'static str {
        "bugfixes"
    }

    fn summary(&self) -> &'static str {
        "Brick Factories no longer raise castle max hp indefinitely. Does not affect \
         current max hp in ongoing saves."
    }

    /// Defaults **off**. It changes the rules of a save that is already running, and
    /// that is the player's decision even though it takes nothing away.
    fn default_enabled(&self) -> bool {
        false
    }

    fn requires(&self) -> Requirements {
        Requirements {
            variables: &[
                "hp_max",
                "is_stone",
                "ADVISORS_EQUIPPED",
                "ARTIFACTS_EQUIPPED",
                "ENCOUNTERS_HAPPENED",
            ],
            signatures: &[Signature {
                what: "wall_repairs fortification cap test",
                rva: SITE_GATE,
                bytes: EXPECT_GATE,
            }],
            objects: &["obj_castle_wall", FACTORY],
            ..Requirements::default()
        }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        self.allowance = match section.get(MODE_KEY).unwrap_or(PER_FACTORY) {
            PER_FACTORY => Allowance::PerFactory,
            TOTAL => Allowance::Total,
            other => {
                return Err(format!(
                    "{MODE_KEY}: expected {PER_FACTORY} or {TOTAL}, found {other:?}"
                ))
            }
        };
        for k in section.unknown(&["enabled", MODE_KEY]) {
            logln!("[fortifications_cap] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn cadence(&self) -> Cadence {
        // A grant is 25 seconds of production apart per factory, so a quarter-second
        // check is far finer than it needs to be, and costs one instance lookup.
        Cadence::Interval(std::time::Duration::from_millis(250))
    }

    fn activate(&mut self, _rt: &Runtime) -> Result<(), String> {
        let this_thread = unsafe { tkiw_runtime::win::GetCurrentThreadId() } as u64;
        if hook::frames() != 0 && !(hook::game_thread() != 0 && hook::game_thread() == this_thread)
        {
            return Err("refusing to patch: the game is running and this is not its thread".into());
        }
        self.announced = None;
        logln!(
            "[fortifications_cap] watching the castle; Fortifications may add {} HP {}",
            CAP as u64,
            match self.allowance {
                Allowance::PerFactory => "per Brick Factory standing",
                Allowance::Total => "in total",
            }
        );
        Ok(())
    }

    fn deactivate(&mut self, _rt: &Runtime) {
        self.open_gate();
    }

    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        // Outside a run there is no castle, so there is nothing to decide and the gate
        // goes back to the game's own (broken, but original) rule.
        let Some(castle) = instance::find_singleton(rt.base, "obj_castle_wall") else {
            self.open_gate();
            self.announced = None;
            return Ok(());
        };
        // SAFETY: on the game's thread, via the frame hook.
        let Some(hp_max) = (unsafe { self.castle_var(rt, castle, "hp_max") })
            .as_ref()
            .and_then(as_number)
        else {
            return Ok(());
        };
        let (ceiling, factories) = unsafe { self.ceiling(rt, castle) };

        if hp_max >= ceiling {
            self.close_gate(rt)?;
            let state = (hp_max as i64, ceiling as i64);
            if self.announced != Some(state) {
                self.announced = Some(state);
                logln!(
                    "[fortifications_cap] castle at {} max HP against a ceiling of {} \
                     ({} Brick Factory/ies) - Fortifications stopped",
                    state.0,
                    state.1,
                    factories
                );
            }
        } else {
            self.open_gate();
            self.announced = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The forced jump must land exactly where the game's own `je` lands, and must fill
    /// the whole verified window so no stray byte is left executable.
    #[test]
    fn the_gate_jump_matches_the_games_own_destination() {
        let site = 0x4000_0000usize;
        let bytes = gate_bytes(site).expect("encodes");
        assert_eq!(bytes.len(), EXPECT_GATE.len());
        assert_eq!(bytes[0], 0xe9, "not a jmp");
        let rel = i32::from_le_bytes(bytes[1..5].try_into().unwrap());
        let ours = (site as i64 + 5 + rel as i64) as usize;
        let je_rel = i32::from_le_bytes(EXPECT_GATE[5..9].try_into().unwrap());
        let theirs = (site as i64 + 3 + 6 + je_rel as i64) as usize;
        assert_eq!(ours, theirs);
        assert!(bytes[5..].iter().all(|b| *b == 0x90), "tail is not padded with nop");
    }

    /// Every amount is a constant read out of the game. If one of these changes while
    /// the game has not, somebody has adjusted a number by feel.
    #[test]
    fn the_ceiling_terms_are_the_ones_measured() {
        assert_eq!(BASE_HP, 100.0);
        assert_eq!(STONE_HP, 20.0);
        assert_eq!(EQUIPPED.iter().map(|(_, _, v)| v).sum::<f64>(), 130.0);
        assert_eq!(ENCOUNTERS.iter().map(|(_, v)| v).sum::<f64>(), 70.0);
        // one Brick Factory standing, which is also the whole of `total` mode
        let most = BASE_HP
            + STONE_HP
            + EQUIPPED.iter().map(|(_, _, v)| v).sum::<f64>()
            + ENCOUNTERS.iter().map(|(_, v)| v).sum::<f64>()
            + CAP;
        assert_eq!(most, 420.0, "the largest castle the game can legitimately produce");
    }

    /// `is_stone` may arrive as a bool or as a real; both must count.
    #[test]
    fn numbers_are_read_whatever_kind_they_arrive_as() {
        assert_eq!(as_number(&Value::Real(1032.0)), Some(1032.0));
        assert_eq!(as_number(&Value::Int(420)), Some(420.0));
        assert_eq!(as_number(&Value::Bool(true)), Some(1.0));
        assert_eq!(as_number(&Value::Str("no".into())), None);
    }

    /// Both values must be accepted, and the default must be the per-factory one.
    #[test]
    fn the_allowance_is_one_of_two_named_values() {
        let mut f = FortificationsCap::default();
        assert!(f.allowance == Allowance::PerFactory, "default is not per_factory");

        let cfg = crate::config::Config::parse("[feature.fortifications_cap]\ncap = total\n");
        f.configure(&cfg.section("fortifications_cap")).expect("total is accepted");
        assert!(f.allowance == Allowance::Total);

        let cfg =
            crate::config::Config::parse("[feature.fortifications_cap]\ncap = per_factory\n");
        f.configure(&cfg.section("fortifications_cap")).expect("per_factory is accepted");
        assert!(f.allowance == Allowance::PerFactory);
    }

    /// A misspelling must be refused rather than silently picking one. The two modes
    /// differ by a factor of however many factories are standing.
    #[test]
    fn an_unknown_allowance_is_refused() {
        let mut f = FortificationsCap::default();
        let cfg = crate::config::Config::parse("[feature.fortifications_cap]\ncap = 100\n");
        assert!(f.configure(&cfg.section("fortifications_cap")).is_err());
    }
}
