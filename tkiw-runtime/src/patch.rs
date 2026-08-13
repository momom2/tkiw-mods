//! Writing to the game's code, and putting it back.
//!
//! The last resort. Everything else in this crate reads; a wrong read fails to
//! `None`, and a wrong patch corrupts someone's game. Reach for this only when the
//! game has no method, no variable and no other lever for what you need -- and when
//! you have disassembled the exact bytes you are replacing.
//!
//! ## The hazard that is easy to miss
//!
//! **A patch is not atomic.** Writing twenty-one bytes over a function prologue takes
//! several stores, and a thread that *enters* the function midway through sees a torn
//! instruction stream and dies. Overwriting a prologue while a thread is already
//! executing the body is usually survivable -- its epilogue restores a stack the old
//! prologue set up correctly -- but entry during the write is not.
//!
//! There is no cheap general fix. The practical rule, and the one this module's
//! callers follow: **patch when the code provably cannot be running.** For a mod
//! loaded as a proxy DLL there is a perfect window -- the startup thread runs before
//! the game's entry point, so no game code has executed at all. [`Patch::apply`]
//! cannot check that for you, so its callers must, and say so.

use crate::win;

/// A byte patch that remembers what was there.
pub struct Patch {
    addr: usize,
    original: Vec<u8>,
    what: String,
    applied: bool,
}

impl Patch {
    /// Overwrite `bytes` at `addr`, remembering the originals.
    ///
    /// `expect` is checked first: the bytes that must currently be there. This is the
    /// difference between patching the function you disassembled and patching whatever
    /// a game update moved into its place.
    ///
    /// # Safety
    /// The caller must ensure no thread can be executing this code. See the module
    /// documentation -- this is not checkable here, and getting it wrong is a crash in
    /// someone else's game.
    pub unsafe fn apply(
        what: &str,
        addr: usize,
        expect: &[u8],
        bytes: &[u8],
    ) -> Result<Patch, String> {
        if bytes.len() > expect.len() {
            return Err(format!(
                "{what}: refusing to write {} bytes where only {} were verified",
                bytes.len(),
                expect.len()
            ));
        }
        if !win::readable(addr, expect.len()) {
            return Err(format!("{what}: {addr:#x} is not readable"));
        }
        let found: Vec<u8> = (0..expect.len())
            .map(|i| core::ptr::read_volatile((addr + i) as *const u8))
            .collect();
        if found != expect {
            return Err(format!(
                "{what}: expected {} at {addr:#x} but found {} - not patching",
                crate::guard::hex(expect),
                crate::guard::hex(&found)
            ));
        }

        let original = found;
        let wrote = win::with_writable(addr, bytes.len(), || {
            for (i, b) in bytes.iter().enumerate() {
                core::ptr::write_volatile((addr + i) as *mut u8, *b);
            }
        });
        if wrote.is_none() {
            return Err(format!("{what}: could not make {addr:#x} writable"));
        }
        win::flush_instruction_cache(addr, bytes.len());
        Ok(Patch { addr, original, what: what.to_string(), applied: true })
    }

    /// Put the original bytes back. Idempotent.
    ///
    /// # Safety
    /// Same condition as [`Patch::apply`]: nothing may be executing the code. In
    /// practice a revert is safer, because the stub being replaced is short and
    /// straight-line, but it is the same hazard.
    pub unsafe fn revert(&mut self) -> Result<(), String> {
        if !self.applied {
            return Ok(());
        }
        let n = self.original.len();
        let wrote = win::with_writable(self.addr, n, || {
            for (i, b) in self.original.iter().enumerate() {
                core::ptr::write_volatile((self.addr + i) as *mut u8, *b);
            }
        });
        if wrote.is_none() {
            return Err(format!("{}: could not restore {:#x}", self.what, self.addr));
        }
        win::flush_instruction_cache(self.addr, n);
        self.applied = false;
        Ok(())
    }

    pub fn is_applied(&self) -> bool {
        self.applied
    }

    pub fn what(&self) -> &str {
        &self.what
    }
}
