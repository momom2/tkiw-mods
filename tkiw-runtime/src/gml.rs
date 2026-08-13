//! Recovering the game's own symbols at runtime.
//!
//! The game is a GameMaker YYC build: all GML is compiled to native code and
//! `data.win` carries no bytecode. But the executable is effectively
//! self-symbolising, through two tables this module rebuilds:
//!
//! * a 24-byte-stride table in `.data` pairing `gml_*` name strings with
//!   function pointers -- a full symbol table for the compiled GML
//! * each GML variable name string is followed 8 bytes later by its
//!   variable-id slot, which holds `0xFFFFFFFF` on disk and the resolved id
//!   once the game has started
//!
//! Everything is resolved **by name**, so a game update that moves code around
//! does not by itself break the mod -- unlike baking addresses in. What an
//! update can still do is rename or remove something, and that is why every
//! lookup is fallible and a failed lookup disables the mod rather than
//! guessing.

use std::collections::HashMap;

use crate::pe::Image;

/// Records in the function table are 24 bytes apart, and a run of at least
/// this many consecutive valid records is what confirms we are looking at the
/// table rather than at coincidental data.
const STRIDE: u32 = 24;
const MIN_RUN: usize = 3;

pub struct Symbols {
    /// Where the module is actually loaded. **Live address = base + rva.**
    ///
    /// Deliberately the base and not the ASLR slide. The slide is the right
    /// correction for a *preferred virtual address* (`image_base + rva`), and
    /// mixing the two conventions silently produces addresses that are short by
    /// `image_base` -- plausible-looking, entirely unmapped, and fatal the first
    /// time one is dereferenced. Everything here is in RVAs, so everything here
    /// adds the base.
    pub base: usize,
    /// GML function name -> RVA.
    pub functions: HashMap<String, u32>,
    /// GML variable name -> RVA of its id slot.
    pub var_slots: HashMap<String, u32>,
    /// Kept only for reporting; never for address arithmetic.
    pub slide: usize,
}

impl Symbols {
    /// Discover both tables from the on-disk executable.
    pub fn discover(image: &Image, live_base: usize) -> Symbols {
        Symbols {
            base: live_base,
            slide: live_base.wrapping_sub(image.image_base as usize),
            functions: function_table(image),
            var_slots: variable_slots(image),
        }
    }

    /// Live address of a compiled GML function.
    pub fn func(&self, name: &str) -> Option<usize> {
        self.functions.get(name).map(|&r| self.base + r as usize)
    }

    /// Live address of a variable's id slot.
    pub fn slot(&self, name: &str) -> Option<usize> {
        self.var_slots.get(name).map(|&r| self.base + r as usize)
    }

    /// The resolved variable id, read out of the live slot.
    ///
    /// Unresolved slots still read `0xFFFFFFFF`; treat that as "not available
    /// yet" rather than as an id, because using it would address nothing.
    ///
    /// The address is derived from the on-disk image, so it is **checked before
    /// it is dereferenced**. Correct arithmetic is not the same as a mapped
    /// page, and getting that wrong is an access violation that takes the
    /// player's game down.
    pub fn var_id(&self, name: &str) -> Option<u32> {
        let addr = self.slot(name)?;
        if !crate::win::readable(addr, 4) {
            return None;
        }
        let id = unsafe { core::ptr::read_volatile(addr as *const u32) };
        if id == u32::MAX {
            None
        } else {
            Some(id)
        }
    }

    /// Why a slot could not be read, for logging.
    pub fn slot_state(&self, name: &str) -> &'static str {
        let Some(addr) = self.slot(name) else {
            return "no such variable";
        };
        match crate::win::query(addr) {
            None => "VirtualQuery failed",
            Some(mbi) if mbi.state == crate::win::MEM_FREE => "NOT MAPPED (free address space)",
            Some(mbi) if mbi.state != crate::win::MEM_COMMIT => "reserved but not committed",
            Some(mbi) if mbi.protect & crate::win::PAGE_GUARD != 0 => "guard page",
            Some(_) if !crate::win::readable(addr, 4) => "committed but not readable",
            Some(_) => "readable",
        }
    }

    /// How many of `names` resolve, for a cheap health check.
    pub fn resolved_count<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> (usize, usize) {
        let mut have = 0;
        let mut total = 0;
        for n in names {
            total += 1;
            if self.functions.contains_key(n) || self.var_slots.contains_key(n) {
                have += 1;
            }
        }
        (have, total)
    }
}

/// Walk `.data` for `{const char* gml_name, void* func, ...}` records.
fn function_table(image: &Image) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    let Some((lo, hi)) = image.initialised_range(".data") else {
        return out;
    };

    let record_at = |rva: u32| -> Option<(String, u32)> {
        let name_va = image.u64_rva(rva)?;
        let func_va = image.u64_rva(rva + 8)?;
        if name_va == 0 || func_va == 0 {
            return None;
        }
        let func_rva = image.va2rva(func_va)?;
        if !image.rva_in(func_rva, ".text") {
            return None;
        }
        let name_rva = image.va2rva(name_va)?;
        if !image.rva_in(name_rva, ".rdata") {
            return None;
        }
        let name = image.cstr(name_rva, 512)?;
        if !name.starts_with("gml_") {
            return None;
        }
        Some((name.to_string(), func_rva))
    };

    let mut rva = lo + (8 - lo % 8) % 8;
    while rva < hi {
        if record_at(rva).is_none() {
            rva += 8;
            continue;
        }
        // confirm by requiring a run at the stride, so a lone lookalike record
        // does not drag in whatever follows it
        let mut run = Vec::new();
        let mut probe = rva;
        while probe < hi {
            match record_at(probe) {
                Some(r) => run.push(r),
                None => break,
            }
            probe += STRIDE;
        }
        if run.len() >= MIN_RUN {
            for (name, func_rva) in run {
                out.entry(name).or_insert(func_rva);
            }
            rva = probe;
        } else {
            rva += 8;
        }
    }
    out
}

/// Walk `.data` for `{const char* var_name, u32 slot = 0xFFFFFFFF}` pairs.
fn variable_slots(image: &Image) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    let Some((lo, hi)) = image.initialised_range(".data") else {
        return out;
    };
    let mut rva = lo + (8 - lo % 8) % 8;
    while rva + 16 <= hi {
        (|| {
            let name_va = image.u64_rva(rva)?;
            if name_va == 0 {
                return None;
            }
            let name_rva = image.va2rva(name_va)?;
            if !image.rva_in(name_rva, ".rdata") {
                return None;
            }
            // the on-disk sentinel is what tells a variable slot apart from any
            // other qword that happens to point at a string
            if image.u32_rva(rva + 8)? != u32::MAX {
                return None;
            }
            let name = image.cstr(name_rva, 128)?;
            if name.is_empty() || !is_identifier(name) {
                return None;
            }
            out.entry(name.to_string()).or_insert(rva + 8);
            Some(())
        })();
        rva += 8;
    }
    out
}

fn is_identifier(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
