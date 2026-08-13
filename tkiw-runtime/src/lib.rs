//! Shared runtime layer for mods to *The King is Watching*.
//!
//! Everything here is about getting safely into the game's process and reading
//! it: injection support, symbol discovery, validated memory access, the
//! per-frame hook onto the game's own thread, the log, and the crash reporter.
//! Nothing here knows what any particular mod is for.
//!
//! It exists because the alternative was copying eighteen files between mods,
//! and the things that go wrong in this layer -- an ASLR miscalculation, a region
//! cache that is slower than what it caches, a crash reporter that allocates --
//! each cost a session to find and should be fixed once.
//!
//! ## The two rules this crate exists to enforce
//!
//! **Resolve by name; bake addresses only when you must, and guard them.** Names
//! survive a game update. Addresses do not, and an address that has moved points
//! at whatever now lives there -- the by-name checks still pass, so the mod looks
//! healthy and calls into arbitrary code on someone else's machine. See
//! [`guard`].
//!
//! **Reads are safe; calls are not.** Nearly everything interesting can be done
//! with pure reads, and a wrong read fails to `None`. A wrong call corrupts
//! memory or takes the process down with it, on a player's machine, mid-run.
//!
//! ## Using it
//!
//! A host declares itself, then resolves:
//!
//! ```ignore
//! identity::set(Identity { name: "momomod-kit", .. });
//! fault::watch();
//! let rt = Runtime::resolve()?;      // symbols + the core signature guard
//! hook::install(&rt.image, rt.base, on_frame)?;
//! ```
//!
//! The full startup sequence a host should follow, including crash-loop
//! protection and save snapshots, is in [`breadcrumb`] and the kit's `lib.rs`.

pub mod breadcrumb;
pub mod builtin;
/// Generated: names for the runtime's builtins. See
/// `knowledge-base/tools/gen_builtin_table.py`.
pub mod builtins_table;
pub mod claim;
pub mod codecave;
pub mod dslist;
pub mod fault;
pub mod globals;
pub mod gml;
pub mod guard;
pub mod home;
pub mod hook;
pub mod identity;
pub mod instance;
pub mod log;
pub mod modules;
pub mod patch;
pub mod pe;
pub mod phase;
pub mod rvalue;
pub mod sample;
pub mod saves;
pub mod symbolize;
pub mod win;

pub use identity::Identity;

/// The game, resolved: everything a feature needs to read it.
///
/// Built once at startup and then shared immutably. Held by the host and handed
/// to features by reference, so no feature has to repeat the symbol pass or
/// re-derive the base.
pub struct Runtime {
    /// Where the module is loaded. **Live address = `base + rva`, always.**
    /// Never `rva + slide`: that drops the image base and lands in unmapped
    /// memory. It has crashed the game twice.
    pub base: usize,
    /// The executable as it is on disk. Symbol discovery reads the file rather
    /// than the process, because the discriminator for an unresolved variable
    /// slot -- `0xFFFFFFFF` -- is true on disk and false in a running process.
    pub image: pe::Image,
    pub syms: gml::Symbols,
    /// Bounds of the game's code, for rejecting a function pointer that does not
    /// land in it.
    pub text: (usize, usize),
    /// Resolved lazily, on first successful use. See [`Runtime::globals`].
    globals: std::sync::OnceLock<globals::Globals>,
}

impl Runtime {
    /// Resolve the game: locate the module, rebuild its symbol tables, and check
    /// the addresses the whole layer is built on.
    ///
    /// **Safe to call before the game's entry point**, which matters because a
    /// proxy DLL's startup thread runs there. Everything here comes from either
    /// the file on disk or code bytes that the loader has already mapped.
    ///
    /// Deliberately *not* included: the global-variable container, the object
    /// registry, and the `ds_*` tables. The GML runtime has not created any of
    /// them yet -- reading the container this early gives a null pointer -- so they
    /// resolve lazily, on the game's own thread. See [`Runtime::globals`].
    ///
    /// Every failure is a message naming what could not be resolved, because the
    /// caller's only sane response is to stand down and say why.
    pub fn resolve() -> Result<Runtime, String> {
        let exe = win::exe_path().ok_or("could not determine the executable path")?;
        let image = pe::Image::load(&exe).ok_or_else(|| format!("could not read {exe}"))?;
        let base = win::exe_base();
        if base == 0 {
            return Err("could not find the game's module base".into());
        }

        let text = image
            .section(".text")
            .map(|s| (base + s.va as usize, base + s.va as usize + s.vsize as usize))
            .ok_or("the executable has no .text section")?;

        // The signature guard comes before anything is called or dereferenced.
        // A mod that checks only names looks perfectly healthy on a new build and
        // then calls into arbitrary code; the signatures are the difference
        // between "stops working and says so" and "misbehaves silently".
        let bad = guard::verify(base, guard::CORE);
        if !bad.is_empty() {
            return Err(format!(
                "this is not the game build the runtime was written for \
                 (expected {}); {} core check(s) failed:\n  {}",
                guard::TARGET_BUILD,
                bad.len(),
                bad.join("\n  ")
            ));
        }

        let syms = gml::Symbols::discover(&image, base);
        if syms.functions.is_empty() || syms.var_slots.is_empty() {
            return Err(format!(
                "symbol discovery found {} functions and {} variable slots; \
                 one of the tables is missing",
                syms.functions.len(),
                syms.var_slots.len()
            ));
        }

        Ok(Runtime { base, image, syms, text, globals: std::sync::OnceLock::new() })
    }

    /// The global-variable container, resolved on first successful use.
    ///
    /// `None` means the GML runtime has not built it yet, which is the normal
    /// answer for the first seconds of a launch and not an error. Retries on every
    /// call until it succeeds, then caches -- the container is created once and
    /// does not move.
    ///
    /// # Safety of the caller
    /// Must be called from the game's thread: it validates a pointer chain into
    /// the single-threaded GML runtime, and anything done with the result reaches
    /// into it. There is no way to enforce that here, so it is the one invariant
    /// every caller owes.
    pub fn globals(&self) -> Option<&globals::Globals> {
        if let Some(g) = self.globals.get() {
            return Some(g);
        }
        match globals::Globals::resolve(self.base, self.text) {
            Ok(g) => {
                let _ = self.globals.set(g);
                self.globals.get()
            }
            Err(_) => None,
        }
    }

    /// As [`Runtime::globals`], but says why it could not resolve.
    ///
    /// For the one-shot line in a log that records *when* the runtime became
    /// usable; ordinary callers want `globals()` and to treat `None` as "not yet".
    pub fn globals_or_err(&self) -> Result<&globals::Globals, String> {
        if let Some(g) = self.globals.get() {
            return Ok(g);
        }
        let g = globals::Globals::resolve(self.base, self.text)?;
        let _ = self.globals.set(g);
        self.globals.get().ok_or_else(|| "globals vanished after resolving".to_string())
    }

    /// Resolved variable id, or `None` while the game has not resolved it yet.
    ///
    /// Slots read `0xFFFFFFFF` until the runtime fills them in during startup,
    /// which takes a few seconds. A lookup made too early must not be cached --
    /// see [`gml::Symbols::var_id`].
    pub fn var_id(&self, name: &str) -> Option<u32> {
        self.syms.var_id(name)
    }

    /// Live address of a compiled GML function, by name.
    pub fn func(&self, name: &str) -> Option<usize> {
        self.syms.func(name)
    }
}

