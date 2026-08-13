//! Refusing to run on a game build this mod was not written for.
//!
//! Most of what the mod needs it resolves **by name**, which survives a game
//! update moving code around. But a handful of things have no name and are
//! baked as addresses: the global-variable container, the object registry, the
//! instance hash, the `ds_list` and `ds_map` tables, and the runtime helpers it
//! calls.
//!
//! After a game update those addresses point at whatever now happens to live
//! there. The by-name symbol check would still pass, so the mod would look
//! healthy and then call into arbitrary code -- on someone else's machine, with
//! no explanation and no way for them to know why.
//!
//! So every baked *code* address carries the first bytes of the function it is
//! supposed to be. If any one of them disagrees, the mod disables itself and
//! says so plainly. Baked *data* addresses cannot be signature-checked, but
//! every use of them is already validated and fails to `None`.
//!
//! This is the same posture as the resume-morale-fix's byte-signature guard,
//! and for the same reason: a patch that silently mis-fires on a new build is
//! worse than one that refuses to load.

use crate::win;

/// The game build the signatures below were taken from. Reported at startup so
/// a mismatch report says which build the mod was expecting.
pub const TARGET_BUILD: &str = "2026-08-10";

/// `(what it is, RVA, the first bytes of that function)`.
///
/// Generated from the analysed build by `analysis/`; regenerate after a game
/// update, once the addresses have been re-established.
pub const CODE_SIGNATURES: &[(&str, usize, &[u8])] = &[
    ("Object_Find", 0x1aa0df0, &[0x48, 0x8b, 0x15, 0xe1, 0x03, 0x06, 0x01, 0x48, 0x63, 0xc1, 0x4c, 0x63]),
    ("method invoke", 0x1aa47f0, &[0x40, 0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x81]),
    ("array_length", 0x1ad71f0, &[0x33, 0xc0, 0x89, 0x41, 0x0c, 0x48, 0x89, 0x01, 0x48, 0x8b, 0x44, 0x24]),
    ("is_method", 0x1adab90, &[0x40, 0x53, 0x48, 0x83, 0xec, 0x20, 0xc7, 0x41, 0x0c, 0x0d, 0x00, 0x00]),
    ("variable_struct_get", 0x1b00380, &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89]),
    ("variable_struct_get_names", 0x1b00560, &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89]),
    ("ds_list_size", 0x1b07b90, &[0x40, 0x53, 0x48, 0x83, 0xec, 0x40, 0x48, 0x8b, 0x05, 0x4b, 0x82, 0xff]),
    ("ds_map_find_value", 0x1b08f70, &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89]),
    ("script_execute", 0x1c4ebc0, &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89]),
];

/// Check every baked address against the code that should be there.
///
/// Returns the mismatches; empty means this is the build the mod knows.
pub fn verify(base: usize) -> Vec<String> {
    let mut bad = Vec::new();
    for (what, rva, expect) in CODE_SIGNATURES {
        let addr = base + rva;
        if !win::readable(addr, expect.len()) {
            bad.push(format!("{what} at {rva:#x}: not readable"));
            continue;
        }
        let found: Vec<u8> = (0..expect.len())
            .map(|i| unsafe { core::ptr::read_volatile((addr + i) as *const u8) })
            .collect();
        if found != *expect {
            bad.push(format!(
                "{what} at {rva:#x}: expected {} but found {}",
                hex(expect),
                hex(&found)
            ));
        }
    }
    bad
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signatures must match the executable they were taken from.
    ///
    /// This is the check that catches a game update at build time rather than
    /// in a player's game.
    #[test]
    fn signatures_match_the_analysed_build() {
        let candidates = [
            r"..\tkiw-morale-fix\The King is Watching.exe.orig",
            r"C:\Program Files (x86)\Steam\steamapps\common\The King is Watching\The King is Watching.exe",
        ];
        let Some(img) = candidates
            .iter()
            .find_map(|p| crate::pe::Image::load(p))
        else {
            eprintln!("no game executable found; skipping");
            return;
        };
        for (what, rva, expect) in CODE_SIGNATURES {
            let off = img.rva2off(*rva as u32).unwrap_or_else(|| {
                panic!("{what}: rva {rva:#x} is not file-backed")
            });
            let found = &img.data[off..off + expect.len()];
            assert_eq!(found, *expect, "{what} at {rva:#x} has moved");
        }
    }
}
