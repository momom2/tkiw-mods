//! The momomod modding framework -- the tools a mod is built from.
//!
//! A momomod mod is a small DLL the manager loads (see [`tkiw_plugin`]). This
//! crate is what a mod author writes against: the [`Feature`](feature::Feature)
//! trait for one self-contained change, the [`config`] system that reads a mod's
//! `.ini`, and the [`registry`] that probes, configures, times and guards each
//! feature so a fault takes down one feature rather than the game.
//!
//! It bundles the two lower layers so a mod depends on one crate:
//!
//! * [`tkiw_runtime`] -- reading and patching the game, and the `overlay`
//!   drawing tool;
//! * [`tkiw_plugin`] -- the ABI the manager drives a mod through.
//!
//! [`plugin`] ties them together: given a mod's name and its features, it is the
//! whole lifecycle behind the four ABI exports, so a mod's own crate is only
//! that list plus a one-line hand-off.

pub mod config;
pub mod feature;
pub mod plugin;
pub mod registry;

pub use feature::Feature;

// Re-exported so a mod depends on `momomod_kit` alone.
pub use tkiw_plugin;
pub use tkiw_runtime;
