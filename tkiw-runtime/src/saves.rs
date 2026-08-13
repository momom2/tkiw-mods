//! Save snapshots.
//!
//! A mod that acts on the player's behalf inside a run puts nothing at risk in
//! the game folder -- the thing genuinely at risk is the save. So every launch
//! takes a snapshot **before** the mod does anything, into the mod's own folder,
//! and a session that goes wrong can be rolled back to the state it started
//! from.
//!
//! Worth taking even for a mod that only reads: it costs nothing, and the launch
//! where it turns out to have been needed is not the launch where you get to
//! decide to start taking them.
//!
//! Cheap enough not to think about: the whole save directory is well under a
//! megabyte.

use std::path::{Path, PathBuf};

use crate::home;

const DIR: &str = "save-backups";
const PREFIX: &str = "launch-";
const KEEP: usize = 10;

/// `%LOCALAPPDATA%\The_king_is_watching_steam\Release`
pub fn save_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let p = Path::new(&local)
        .join("The_king_is_watching_steam")
        .join("Release");
    p.is_dir().then_some(p)
}

/// Copy the save directory into a new numbered snapshot.
///
/// Returns `(destination, files copied)`. Failures are reported, never fatal:
/// a missing backup is a reason to log loudly, not a reason to stop the game
/// from starting.
pub fn snapshot() -> Result<(PathBuf, usize), String> {
    let src = save_dir().ok_or("no save directory found")?;
    let root = home::file(DIR).ok_or("no mod folder")?;
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {DIR}: {e}"))?;

    let next = next_index(&root);
    let dst = root.join(format!("{PREFIX}{next:04}"));
    std::fs::create_dir_all(&dst).map_err(|e| format!("cannot create snapshot dir: {e}"))?;

    let mut copied = 0;
    let entries = std::fs::read_dir(&src).map_err(|e| format!("cannot read saves: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name() else { continue };
        std::fs::copy(&path, dst.join(name))
            .map_err(|e| format!("copying {}: {e}", path.display()))?;
        copied += 1;
    }
    if copied == 0 {
        let _ = std::fs::remove_dir(&dst);
        return Err("the save directory was empty".into());
    }
    prune(&root);
    Ok((dst, copied))
}

fn indices(root: &Path) -> Vec<(u32, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
            if let Some(rest) = name.strip_prefix(PREFIX) {
                if let Ok(n) = rest.parse::<u32>() {
                    out.push((n, p));
                }
            }
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

fn next_index(root: &Path) -> u32 {
    indices(root).last().map(|(n, _)| n + 1).unwrap_or(1)
}

/// Keep the most recent `KEEP` snapshots. Only ever removes directories this
/// module created and named itself.
fn prune(root: &Path) {
    let all = indices(root);
    if all.len() <= KEEP {
        return;
    }
    for (_, path) in &all[..all.len() - KEEP] {
        let _ = std::fs::remove_dir_all(path);
    }
}
