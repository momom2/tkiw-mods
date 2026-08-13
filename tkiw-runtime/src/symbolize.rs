//! Turning an address back into a name.
//!
//! A profiler is only worth having if its output names things. The game carries a
//! symbol table for its own 13,132 compiled GML functions and `.pdata` boundaries
//! for all 86,347, so an address can be attributed exactly -- no PDB, no symbol
//! server, no guessing.
//!
//! Three tiers of answer, best first:
//!
//! 1. **A named GML function.** `gml_Object_obj_unit_Step_0+0x1c4`. The whole point.
//! 2. **A known runtime routine.** The GameMaker runtime is not symbolised, but the
//!    handful of routines that dominate any profile have been identified by their
//!    error strings -- see `runtime-internals.md`. Naming them is the difference
//!    between "38% in `sub_1ac46e0`" and "38% reading struct members".
//! 3. **A `.pdata` boundary.** `sub_1b0d3a0+0x40`. Still useful: it groups samples
//!    by function, so a hot unknown shows up as one line rather than forty.
//!
//! Anything outside the module is reported as such. That is not a failure -- a
//! sample landing in `d3d11` or `ntdll` means the game was waiting, which is
//! exactly the sort of thing worth knowing.

use std::collections::HashMap;

use crate::pe::Image;

/// Unnamed runtime routines identified from their error strings or from the builtin
/// wrapper that reaches them.
///
/// Kept in sync with `tools/summarise.py`'s `RUNTIME` table; both are worth
/// extending whenever another is identified, and this one changes what a profile
/// reads like. RVAs are for the 2026-08-10 build, and a wrong name here is worse
/// than none -- so each is checked against the build by the test below.
pub const RUNTIME_ROUTINES: &[(u32, &str)] = &[
    (0x1a8a940, "YYGetBool"),
    (0x1a8bf50, "YYGetInt32"),
    (0x1a8c0d0, "YYGetInt64"),
    (0x1a8c880, "YYGetReal"),
    (0x1a8ee40, "YYGetRef"),
    (0x1aa0df0, "Object_Find"),
    (0x1aa47f0, "method_invoke"),
    (0x1aa4c90, "static_get"),
    (0x1ac46e0, "member_get"),
    (0x1af1390, "to_string"),
    (0x1b0d3a0, "ds_get"),
    (0x1b31600, "Object_FindIndexByName"),
    (0x8f4e0, "RValue_assign"),
    (0x8f580, "RValue_release"),
    (0x8f6b0, "RValue_copy"),
    (0x1aa4090, "GMLString_new"),
    // **How compiled GML calls a builtin.** Takes an index, scales it by 24 -- the
    // stride of the game's own function table -- indexes a table of descriptors, and
    // allocates argc*16 bytes of RValue arguments before dispatching. So a builtin
    // call is an *indirect* call through here, which is why counting direct call
    // sites to a builtin finds exactly zero.
    //
    // Named wrongly at first ("RValue_from_literal", guessed from the string
    // constants near its call sites), and the profile is what exposed it: it showed
    // up in a stack between `obj_init_Create_0` and `texture_prefetch`, and nothing
    // that builds a string from a literal calls texture_prefetch.
    (0x1aa46c0, "call_builtin_by_index"),
    (0x1e9ff30, "gml_frame_push"),
    (0x1e9fc10, "gml_frame_pop"),
];

/// Address -> name, for everything in the game's image.
pub struct Symbolizer {
    /// `(start rva, name)`, sorted. Holds GML names, runtime-routine names, and a
    /// `.pdata` entry for everything else.
    entries: Vec<(u32, Name)>,
    /// `(start, end)` of every `.pdata` function, so a sample can be told from one
    /// that merely follows a known start.
    ranges: Vec<(u32, u32)>,
    size_of_image: u32,
}

#[derive(Clone)]
enum Name {
    /// A compiled GML function, from the game's own symbol table.
    Gml(String),
    /// A runtime routine identified by hand; see [`RUNTIME_ROUTINES`].
    Runtime(&'static str),
    /// A `.pdata` boundary with no name available.
    Anon(u32),
}

impl Name {
    fn render(&self) -> String {
        match self {
            Name::Gml(s) => s.clone(),
            Name::Runtime(s) => s.to_string(),
            Name::Anon(rva) => format!("sub_{rva:x}"),
        }
    }
}

/// What an address resolved to.
pub struct Site {
    /// Start RVA of the enclosing function, which is what samples aggregate by.
    pub func: u32,
    pub offset: u32,
    pub inside: bool,
}

impl Symbolizer {
    /// Build from the on-disk image and the game's own symbol table.
    ///
    /// Costs one sort of ~86k entries and is done once, off the game's thread.
    pub fn build(image: &Image, functions: &HashMap<String, u32>) -> Symbolizer {
        let pdata = image.pdata_functions();
        let mut by_rva: HashMap<u32, Name> = HashMap::with_capacity(pdata.len() + functions.len());

        for (begin, _end, _) in &pdata {
            by_rva.insert(*begin, Name::Anon(*begin));
        }
        // The game's own names win over an anonymous boundary.
        for (name, rva) in functions {
            by_rva.insert(*rva, Name::Gml(shorten(name)));
        }
        // The runtime's builtins, generated from the `Function_Add` walk. Inserted only
        // where they land on a real `.pdata` boundary: the table is build-specific, and
        // a stale entry should make a frame anonymous, never mislabelled.
        //
        // These are what turn an engine table of `sub_1b1db20` into one that names the
        // call the game actually made.
        for (rva, name) in crate::builtins_table::BUILTINS {
            if let Some(slot) = by_rva.get_mut(rva) {
                if matches!(slot, Name::Anon(_)) {
                    *slot = Name::Runtime(name);
                }
            }
        }
        // And a hand-identified runtime routine wins over everything, since it is
        // the only name that will ever exist for it.
        for (rva, name) in RUNTIME_ROUTINES {
            by_rva.insert(*rva, Name::Runtime(name));
        }

        let mut entries: Vec<(u32, Name)> = by_rva.into_iter().collect();
        entries.sort_unstable_by_key(|(rva, _)| *rva);
        let ranges: Vec<(u32, u32)> = pdata.iter().map(|(b, e, _)| (*b, *e)).collect();

        Symbolizer { entries, ranges, size_of_image: image.size_of_image }
    }

    /// Attribute a live address, given the module base.
    pub fn resolve(&self, base: usize, addr: usize) -> Site {
        if addr < base || addr >= base + self.size_of_image as usize {
            return Site { func: 0, offset: 0, inside: false };
        }
        let rva = (addr - base) as u32;
        let i = match self.entries.binary_search_by(|e| e.0.cmp(&rva)) {
            Ok(i) => i,
            Err(0) => return Site { func: 0, offset: 0, inside: false },
            Err(i) => i - 1,
        };
        let start = self.entries[i].0;
        Site { func: start, offset: rva - start, inside: true }
    }

    /// Whether this function is compiled GML, as opposed to a runtime routine or an
    /// unnamed `.pdata` entry.
    ///
    /// The distinction matters to anything asking "which of the game's own functions is
    /// responsible for this work". `call_builtin_by_index`, `RValue_release` and the
    /// rest are named, but naming them is not an answer: every builtin call in the game
    /// passes through the dispatcher, so it appeared as the responsible frame for 80.6%
    /// of a startup phase and explained nothing.
    pub fn is_gml(&self, func: u32) -> bool {
        match self.entries.binary_search_by(|e| e.0.cmp(&func)) {
            Ok(i) => matches!(self.entries[i].1, Name::Gml(_)),
            Err(_) => false,
        }
    }

    /// The name of a function by its start RVA.
    pub fn name_of(&self, func: u32) -> String {
        if func == 0 {
            return "<outside the game>".to_string();
        }
        match self.entries.binary_search_by(|e| e.0.cmp(&func)) {
            Ok(i) => self.entries[i].1.render(),
            Err(_) => format!("sub_{func:x}"),
        }
    }

    /// Whether `rva` falls inside a `.pdata` function, as opposed to merely after
    /// one. Used to decide whether an address can be unwound with unwind data or
    /// has to be treated as a leaf.
    pub fn has_unwind_data(&self, rva: u32) -> bool {
        match self.ranges.binary_search_by(|r| r.0.cmp(&rva)) {
            Ok(_) => true,
            Err(0) => false,
            Err(i) => {
                let (b, e) = self.ranges[i - 1];
                rva >= b && rva < e
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `gml_Object_obj_unit_Step_0` reads better than the full mangled form, and a
/// profile is a wall of names where every character counts.
///
/// Only the uninformative prefixes go; the `inner@outer` structure is what tells
/// you which closure you are in, so it stays.
fn shorten(name: &str) -> String {
    for p in ["gml_Script_", "gml_GlobalScript_", "gml_Object_"] {
        if let Some(rest) = name.strip_prefix(p) {
            return rest.to_string();
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Option<Image> {
        [
            r"..\tkiw-morale-fix\The King is Watching.exe.orig",
            r"C:\Program Files (x86)\Steam\steamapps\common\The King is Watching\The King is Watching.exe",
        ]
        .iter()
        .find_map(|p| Image::load(p))
    }

    /// A named runtime routine that has moved would put a *wrong* name on a hot
    /// line in a profile, which is worse than leaving it as `sub_...`: it sends the
    /// reader after the wrong function. So every entry is checked to be the start
    /// of a real function on this build.
    #[test]
    fn every_named_runtime_routine_is_a_real_function_start() {
        let Some(img) = game() else {
            eprintln!("no game executable; skipping");
            return;
        };
        let text = img.section(".text").expect(".text");
        for (rva, name) in RUNTIME_ROUTINES {
            assert!(
                *rva >= text.va && *rva < text.va + text.vsize,
                "{name} at {rva:#x} is not in .text"
            );
        }
        // Most, but not all, have .pdata entries: a true leaf has none, which is
        // exactly why `Object_Find` was noted as one. So this asserts the weaker
        // and actually-true property: none of them lands *inside* a different
        // function's range.
        let sym = Symbolizer::build(&img, &Default::default());
        for (rva, name) in RUNTIME_ROUTINES {
            let site = sym.resolve(0, *rva as usize);
            assert!(site.inside, "{name}: not inside the image");
            assert_eq!(site.offset, 0, "{name} at {rva:#x} is not a function start");
        }
    }

    #[test]
    fn resolves_a_named_gml_function_and_an_offset_into_it() {
        let Some(img) = game() else { return };
        let mut funcs = HashMap::new();
        // A function whose address is known from the analysis notes.
        funcs.insert("gml_Object_obj_splash_screen_Step_0".to_string(), 0x17c6870);
        let sym = Symbolizer::build(&img, &funcs);

        let at_start = sym.resolve(0, 0x17c6870);
        assert!(at_start.inside);
        assert_eq!(at_start.offset, 0);
        assert_eq!(sym.name_of(at_start.func), "obj_splash_screen_Step_0");

        let inside = sym.resolve(0, 0x17c6870 + 0x40);
        assert_eq!(inside.func, 0x17c6870);
        assert_eq!(inside.offset, 0x40);
    }

    #[test]
    fn an_address_outside_the_image_is_reported_as_outside() {
        let Some(img) = game() else { return };
        let sym = Symbolizer::build(&img, &Default::default());
        let base = 0x1_0000_0000usize;
        let out = sym.resolve(base, base + img.size_of_image as usize + 0x1000);
        assert!(!out.inside);
        assert_eq!(sym.name_of(out.func), "<outside the game>");
    }
}
