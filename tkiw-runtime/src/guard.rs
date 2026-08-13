//! Refusing to run on code this mod was not written for.
//!
//! Most of what a mod needs it resolves **by name**, which survives a game
//! update moving code around. But a handful of things have no name and are baked
//! as addresses: the global-variable container, the object registry, the
//! instance hash, the `ds_list` and `ds_map` tables, and any runtime helper
//! being called.
//!
//! After a game update those addresses point at whatever now happens to live
//! there. The by-name symbol check would still pass, so the mod would look
//! healthy and then call into arbitrary code -- on someone else's machine, with
//! no explanation and no way for them to know why.
//!
//! So every baked *code* address carries the first bytes of the function it is
//! supposed to be. If any one disagrees, whatever depends on it is disabled and
//! says so plainly. Baked *data* addresses cannot be signature-checked, but
//! every use of them is validated and fails to `None`.
//!
//! ## Why this is a list and not a constant
//!
//! The auto-picker checked one global list and disabled *itself* on any
//! mismatch. For a mod that does one thing that is right. For a kit hosting a
//! dozen features it means one moved function costs the player the other eleven
//! -- so signatures here are grouped by who needs them, and the caller decides
//! how much to switch off. See `tkiw-momomod-kit/spec.md` §5.

use crate::win;

/// A baked code address and the bytes that must be there.
///
/// Twelve bytes is enough to be conclusive without being fragile: it reaches
/// past the prologue's register saves into something the function actually does,
/// and it is short enough not to trip over a compiler that reordered two
/// unrelated instructions.
#[derive(Clone, Copy)]
pub struct Signature {
    /// What the function is, for the message a player reads.
    pub what: &'static str,
    pub rva: usize,
    pub bytes: &'static [u8],
}

/// Check a set of signatures against the loaded image.
///
/// Returns one message per mismatch, naming what was expected and what is
/// actually there. Empty means every one of them holds.
pub fn verify(base: usize, sigs: &[Signature]) -> Vec<String> {
    let mut bad = Vec::new();
    for sig in sigs {
        let addr = base + sig.rva;
        if !win::readable(addr, sig.bytes.len()) {
            bad.push(format!("{} at {:#x}: not readable", sig.what, sig.rva));
            continue;
        }
        let found: Vec<u8> = (0..sig.bytes.len())
            .map(|i| unsafe { core::ptr::read_volatile((addr + i) as *const u8) })
            .collect();
        if found != sig.bytes {
            bad.push(format!(
                "{} at {:#x}: expected {} but found {}",
                sig.what,
                sig.rva,
                hex(sig.bytes),
                hex(&found)
            ));
        }
    }
    bad
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

/// The addresses the shared runtime itself is built on.
///
/// These are not a feature's business: instance lookup, global reads and
/// `ds_list` traversal all depend on them, so if they have moved there is
/// nothing a mod can safely do at all. A host checks these once and stands
/// itself down on any failure -- the one all-or-nothing check that remains.
///
/// The *data* addresses cannot be signature-checked and are not listed here;
/// they live in the modules that use them and every use is validated.
/// Signatures are from the 2026-08-10 build.
pub const CORE: &[Signature] = &[
    Signature {
        what: "Object_Find",
        rva: 0x1aa0df0,
        bytes: &[0x48, 0x8b, 0x15, 0xe1, 0x03, 0x06, 0x01, 0x48, 0x63, 0xc1, 0x4c, 0x63],
    },
    Signature {
        what: "method invoke",
        rva: 0x1aa47f0,
        bytes: &[0x40, 0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x81],
    },
    Signature {
        what: "array_length",
        rva: 0x1ad71f0,
        bytes: &[0x33, 0xc0, 0x89, 0x41, 0x0c, 0x48, 0x89, 0x01, 0x48, 0x8b, 0x44, 0x24],
    },
    Signature {
        what: "is_method",
        rva: 0x1adab90,
        bytes: &[0x40, 0x53, 0x48, 0x83, 0xec, 0x20, 0xc7, 0x41, 0x0c, 0x0d, 0x00, 0x00],
    },
    Signature {
        what: "variable_struct_get",
        rva: 0x1b00380,
        bytes: &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89],
    },
    Signature {
        what: "variable_struct_get_names",
        rva: 0x1b00560,
        bytes: &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89],
    },
    Signature {
        what: "ds_list_size",
        rva: 0x1b07b90,
        bytes: &[0x40, 0x53, 0x48, 0x83, 0xec, 0x40, 0x48, 0x8b, 0x05, 0x4b, 0x82, 0xff],
    },
    Signature {
        what: "ds_map_find_value",
        rva: 0x1b08f70,
        bytes: &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89],
    },
    Signature {
        what: "script_execute",
        rva: 0x1c4ebc0,
        bytes: &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89],
    },
];

/// The game build these signatures were taken from. Reported at startup so a
/// mismatch report says which build the mod was expecting.
pub const TARGET_BUILD: &str = "2026-08-10";

#[cfg(test)]
mod tests {
    use super::*;

    /// The signatures must match the executable they were taken from.
    ///
    /// This is the check that catches a game update at build time rather than in
    /// a player's game. It reads the pristine copy the morale fix keeps, so it
    /// describes the shipped game rather than someone's patched one.
    #[test]
    fn core_signatures_match_the_analysed_build() {
        let candidates = [
            r"..\tkiw-morale-fix\The King is Watching.exe.orig",
            r"C:\Program Files (x86)\Steam\steamapps\common\The King is Watching\The King is Watching.exe",
        ];
        let Some(img) = candidates.iter().find_map(|p| crate::pe::Image::load(p)) else {
            eprintln!("no game executable found; skipping");
            return;
        };
        for sig in CORE {
            let off = img
                .rva2off(sig.rva as u32)
                .unwrap_or_else(|| panic!("{}: rva {:#x} is not file-backed", sig.what, sig.rva));
            let found = &img.data[off..off + sig.bytes.len()];
            assert_eq!(found, sig.bytes, "{} at {:#x} has moved", sig.what, sig.rva);
        }
    }
}
