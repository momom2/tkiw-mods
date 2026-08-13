//! The reward auto-picker, absorbed as a mod of the kit.
//!
//! Linked, not copied. `tkiw_reward_picker` already builds as an `rlib`, so this drives
//! its logic and the kit supplies the lifecycle it used to carry itself: the frame hook,
//! the re-entry guard, the panic boundary, the time budget, the log. What is left is one
//! call per frame.
//!
//! ## Exactly one copy may act
//!
//! Both this and a standalone `version.dll` install press buttons on the reward screen.
//! Two of them pressing on one screen is a corrupted run, not a cosmetic bug, and they
//! cannot see each other's statics -- separate modules, separate copies of every global.
//!
//! So this yields to any standalone install. The standalone wins deliberately: it is the
//! copy whose `config.ini` the player has tuned, so installing the kit beside an existing
//! picker changes nothing about how rewards get picked.
//!
//! Detecting it takes two mechanisms, because one of them cannot see the past. A named
//! kernel claim is exact, but only a standalone built after the claim existed takes it --
//! and every installation already out there predates that. So activation also looks for
//! the picker's stamp marker in the game folder's DLLs, which any installation has. The
//! marker cannot match the hosted copy: the stamp carrying it is compiled only into the
//! standalone build.
//!
//! The claim is then re-checked through [`SETTLE`], since load order between two DLLs of
//! one executable is not something to rely on. That window is also the one the picker
//! waits out before touching the game, so nothing is lost to it.
//!
//! ## Its config is its own
//!
//! `config/reward-picker.ini` is written by the picker, not by the kit: the tiers and
//! weights are generated from the live game's option lists, which the kit does not have
//! and should not learn. The mod is marked `self_configuring` so the kit neither
//! generates that file nor complains about the sections in it that it does not know.
//! The only key the kit owns is `enabled`.

use std::time::{Duration, Instant};

use tkiw_runtime::{logln, Runtime};

use crate::config::Section;
use crate::feature::{Cadence, Feature, Requirements};

/// How long to keep watching for a standalone install before concluding there is none.
///
/// The picker does no work in its first five seconds either way -- it waits for the game
/// to finish coming up -- so this costs nothing.
const SETTLE: Duration = Duration::from_secs(6);

pub struct RewardPicker {
    started: Option<Instant>,
    /// Set once the standalone DLL has been ruled out, so the check stops.
    settled: bool,
    frames: u64,
}

impl Default for RewardPicker {
    fn default() -> RewardPicker {
        RewardPicker { started: None, settled: false, frames: 0 }
    }
}

impl Feature for RewardPicker {
    fn name(&self) -> &'static str {
        "reward_picker"
    }

    fn module(&self) -> &'static str {
        "reward-picker"
    }

    fn summary(&self) -> &'static str {
        "Picks reward choices for you, by the rules in this file. Stands down by itself \
         if the standalone auto-picker DLL is also installed."
    }

    /// On when the mod is loaded: a player who has switched this mod on in the kit's
    /// file has already said what they want, and a second switch saying the same thing
    /// is a way to have it not work for no visible reason.
    fn default_enabled(&self) -> bool {
        true
    }

    fn requires(&self) -> Requirements {
        // The picker carries its own guard, checked against the game build it was made
        // for, and reports through the same log. Duplicating those checks here would
        // mean two places to update and two ways to disagree.
        Requirements::default()
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        // Everything else in this file belongs to the picker, so nothing here is
        // "unknown" -- reporting the picker's own keys as typos would be noise.
        let _ = section;
        Ok(())
    }

    fn cadence(&self) -> Cadence {
        // The picker paces itself internally; it wants to see every frame.
        Cadence::EveryFrame
    }

    fn activate(&mut self, _rt: &Runtime) -> Result<(), String> {
        // The full check, which also sees an installation predating the claim -- and
        // that is every one currently out there, so this is the one that matters today.
        if tkiw_reward_picker::standalone_installed() {
            return Err("a standalone auto-picker DLL is installed in the game folder; \
                        leaving the picking to it. Run its uninstall.py to use this one."
                .into());
        }
        let path = crate::config::path(&crate::config::mod_file("reward-picker"))
            .ok_or("no mod folder to keep the picker's config in")?;
        tkiw_reward_picker::host_config_at(path.clone());

        // Resolve the game. The standalone did this from its own startup thread; hosted,
        // nothing else will, and without it the picker runs every frame and does nothing
        // -- which is precisely what it did for two hundred seconds when this was missing.
        tkiw_reward_picker::hosted_start();

        self.started = Some(Instant::now());
        self.settled = false;
        self.frames = 0;
        logln!("[reward_picker] hosted; config {}", path.display());
        Ok(())
    }

    fn on_frame(&mut self, _rt: &Runtime) -> Result<(), String> {
        let started = *self.started.get_or_insert_with(Instant::now);

        // Load order between two DLLs of the same executable is not something to rely
        // on, so keep asking until the window has passed.
        if !self.settled {
            if tkiw_reward_picker::standalone_running() {
                return Err("the standalone auto-picker started after this one; standing \
                            down so only one of us presses"
                    .into());
            }
            if started.elapsed() >= SETTLE {
                self.settled = true;
            }
        }

        self.frames += 1;
        tkiw_reward_picker::hosted_frame(self.frames);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The claim is process-wide, so these two facts cannot be checked in separate
    /// tests: cargo runs them on threads of one process, and one taking the claim makes
    /// the other's "nobody holds it" fail. One test, in order.
    #[test]
    fn the_claim_is_shared_and_taking_it_is_visible() {
        // both sides must name it identically, or each concludes it is alone
        assert_eq!(tkiw_reward_picker::STANDALONE_CLAIM, "tkiw_reward_picker_standalone");
        let held = tkiw_runtime::claim::Claim::take(tkiw_reward_picker::STANDALONE_CLAIM)
            .expect("take");
        assert!(tkiw_reward_picker::standalone_running(), "a held claim is invisible");
        drop(held);
        assert!(!tkiw_reward_picker::standalone_running(), "the claim outlived its holder");
    }

    #[test]
    fn it_belongs_to_its_own_mod() {
        let f = RewardPicker::default();
        assert_eq!(f.module(), "reward-picker");
        assert!(f.default_enabled());
    }
}
