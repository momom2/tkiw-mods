//! What a feature is, and what the loader promises it.
//!
//! The kit hosts many small independent changes. The point of this module is that
//! **the unit of health is the feature, not the mod**: each one declares what it
//! depends on, and the loader checks each separately, so a game update that moves
//! one function costs the player that one feature rather than all of them.
//!
//! A feature never decides whether it is enabled, never checks its own
//! preconditions, never times itself, and never catches its own panics. All four
//! belong to the loader, because all four are the kind of thing that is done
//! inconsistently when it is done twelve times.

use std::time::Duration;

use tkiw_runtime::{guard::Signature, Runtime};

/// What must still be true of the game for a feature to be safe to run.
///
/// Declared as data rather than as code so the loader can check it before the
/// feature has run a single instruction, and report exactly which item failed.
/// "skip_splash: variable `goto_menu` no longer exists" is a bug report;
/// "skip_splash disabled" is not.
#[derive(Clone, Copy, Default)]
pub struct Requirements {
    /// Compiled GML functions that must resolve by name. Names survive a game
    /// update moving code around, so prefer these to signatures wherever the
    /// feature only needs to *find* something.
    pub functions: &'static [&'static str],
    /// GML variables whose id slots must exist. Note that a slot existing is not
    /// the same as it being resolved: ids are `0xFFFFFFFF` until the game fills
    /// them in a few seconds into startup, so this checks the slot, and reading
    /// the id stays fallible.
    pub variables: &'static [&'static str],
    /// Baked addresses with the bytes that must be there. Only for things with no
    /// name; every one of these is a liability across game updates, which is
    /// exactly why it has to be declared here rather than used quietly.
    pub signatures: &'static [Signature],
    /// Object names the feature works with. **Not checked at startup**: outside
    /// gameplay these genuinely do not exist, so absence proves nothing. Recorded
    /// so the generated config and the log can say what a feature touches.
    pub objects: &'static [&'static str],
}

/// How often the loader should call a feature.
#[derive(Clone, Copy, PartialEq)]
pub enum Cadence {
    /// Never called after `activate`. For features that are entirely a code patch
    /// or a hook installed once.
    Never,
    /// Called once, on the first frame after activation.
    Once,
    /// Called every message pump. Note that the pump rate is wildly uneven --
    /// tens of thousands per second while loading, about sixty per second in play
    /// -- so anything paced by wall-clock time wants `Interval` instead.
    EveryFrame,
    /// Called no more often than this.
    Interval(Duration),
}

/// One self-contained change to the game.
pub trait Feature: Send {
    /// Config section key, as `[feature.<name>]`. Stable: it is what a player's
    /// ini says, so renaming one silently disables their setting.
    fn name(&self) -> &'static str;

    /// Which shippable mod this feature belongs to, and so which config file it
    /// is read from: `config/<module>.ini`.
    ///
    /// No default. Every mod must be able to ship on its own, so a feature that
    /// has not said which mod it is part of is a question, not something to guess
    /// at -- and guessing would put a player's setting in a file the standalone
    /// mod does not read.
    fn module(&self) -> &'static str;

    /// One line for the generated config, in the player's terms. What changes,
    /// not how it is implemented.
    fn summary(&self) -> &'static str;

    /// The feature's own config keys, beyond `enabled`, for the default file a
    /// mod writes on first run. INI lines, with `#` comments, e.g.
    /// `"# how many\nsteps = 10\n"`. Empty means the feature has only `enabled`.
    ///
    /// Whatever this offers, [`configure`](Feature::configure) must accept, or a
    /// player uncommenting a generated line gets an error from a file the mod
    /// wrote itself.
    fn config_template(&self) -> &'static str {
        ""
    }

    /// Whether this feature is on when the config does not say.
    ///
    /// Default `false`. A feature should only default on if it fixes something
    /// nobody could want, and the reason belongs in the readme.
    fn default_enabled(&self) -> bool {
        false
    }

    fn requires(&self) -> Requirements {
        Requirements::default()
    }

    /// Read this feature's own config keys. Returning `Err` leaves the feature
    /// off with the message logged; it must not partially apply settings first.
    fn configure(&mut self, _section: &crate::config::Section) -> Result<(), String> {
        Ok(())
    }

    /// Take effect. Install code patches and hooks here, not in `on_frame`, so
    /// that "is this feature doing anything" has one answer and one place.
    ///
    /// Called on the game's thread, after every requirement has been checked.
    fn activate(&mut self, _rt: &Runtime) -> Result<(), String> {
        Ok(())
    }

    /// Undo exactly what `activate` did. Must be safe to call when inactive, and
    /// safe to call after a panic in `on_frame` -- which is precisely when it is
    /// most likely to be called.
    fn deactivate(&mut self, _rt: &Runtime) {}

    fn cadence(&self) -> Cadence {
        Cadence::Never
    }

    /// Whether this feature governs its own per-call cost, and so must not be
    /// switched off by the loader for exceeding the frame budget.
    ///
    /// Default `false`: the loader's cost-kill is the right protection for a
    /// feature that has no other. The exception is a feature that carries its
    /// own rate-limiting and has legitimate expensive bursts -- the reward
    /// picker resolves every card the frame a reward screen appears, which is
    /// tens of milliseconds a few times a run, between long cheap stretches.
    /// Killing it for that left it having picked one reward and then gone
    /// dormant, which is the worst of both worlds. Such a feature is still held
    /// to the [`crate::registry`] pathological limit -- nothing may stop the
    /// game outright -- and its cost is still logged; it is only spared the
    /// ordinary strike-based shutdown.
    fn self_paced(&self) -> bool {
        false
    }

    /// Do the per-frame work. Called on the game's thread.
    ///
    /// Returning `Err` disables the feature and logs the message: use it for "I
    /// have found a state I do not understand", which is a reason to stop rather
    /// than to guess.
    fn on_frame(&mut self, _rt: &Runtime) -> Result<(), String> {
        Ok(())
    }
}

/// Why a feature is not running.
#[derive(Clone)]
pub enum Off {
    /// The config says so, or its default is off and the config is silent.
    Disabled,
    /// A requirement no longer holds. Carries the specific failure.
    Unsupported(String),
    /// `configure` rejected the section.
    Misconfigured(String),
    /// `activate` failed.
    FailedToStart(String),
    /// It panicked, or returned an error, at runtime.
    Faulted(String),
    /// It kept overrunning its share of the frame.
    TooSlow(String),
}

impl Off {
    pub fn describe(&self) -> String {
        match self {
            Off::Disabled => "disabled in the config".to_string(),
            Off::Unsupported(w) => format!("not supported by this game build: {w}"),
            Off::Misconfigured(w) => format!("config rejected: {w}"),
            Off::FailedToStart(w) => format!("could not start: {w}"),
            Off::Faulted(w) => format!("switched off after a fault: {w}"),
            Off::TooSlow(w) => format!("switched off for costing frames: {w}"),
        }
    }
}

/// Check a feature's declared requirements against the resolved game.
///
/// Returns the first failure, described in terms a bug report can use. Order is
/// deliberate: by-name lookups first, since they are the cheap ones and the ones
/// most likely to be informative, and baked signatures last.
pub fn check(rt: &Runtime, req: &Requirements) -> Result<(), String> {
    for name in req.functions {
        if rt.func(name).is_none() {
            return Err(format!("no such GML function: {name}"));
        }
    }
    for name in req.variables {
        if rt.syms.slot(name).is_none() {
            return Err(format!("no such GML variable: {name}"));
        }
    }
    let bad = tkiw_runtime::guard::verify(rt.base, req.signatures);
    if !bad.is_empty() {
        return Err(bad.join("; "));
    }
    Ok(())
}
