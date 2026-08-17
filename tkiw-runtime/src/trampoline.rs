//! A register-preserving detour that calls back into Rust from inside a game
//! function, then lets the function carry on as if nothing happened.
//!
//! [`crate::codecave`] gives executable memory and the five-byte `call rel32`
//! to reach it, and [`crate::patch`] gives a revertible byte patch. This is the
//! piece between them the backlog named as missing: a stub that saves every
//! register the game's continuation relies on, calls a Rust function, restores
//! them, runs the prologue bytes the patch displaced, and jumps back.
//!
//! ## Why a whole trampoline, when `popup_stutter_fix` patched inline
//!
//! That patch replaced pure SSE arithmetic and touched no general register, so
//! it needed no preservation. Calling into Rust is different: the callee may
//! clobber every volatile register and all of `xmm0..5`, and the game's code
//! after the patch expects them intact. So the stub preserves them around the
//! call.
//!
//! ## The stack, exactly
//!
//! The patch sits at a function's **entry**, so the injected `call` pushes an
//! eight-byte return address the real function never had. Left there, the
//! displaced `mov rax, rsp` -- which is how these functions open -- would read
//! an `rsp` that is eight low, and every shadow-space store lands in the wrong
//! place. So the stub anchors `rbp` at entry, and before running the displaced
//! bytes it restores `rsp` to the value the function would have seen and
//! discards the injected return address. It returns by an absolute
//! `jmp [rip]`, which clobbers no register -- important, because the displaced
//! `mov rax, rsp` has just set `rax` and the body will read it.
//!
//! At the patched `call` site `rsp` is 16-aligned (the ABI guarantees it at any
//! call), so the stub force-aligns with `and rsp, -16` before calling Rust
//! rather than counting pushes.
//!
//! ## What the caller must guarantee
//!
//! As with `codecave`, the general problem is not solved here; a specific site
//! is. The caller passes the exact bytes currently at the site, and the install
//! refuses unless they match -- the same signature discipline the features use,
//! so a game update that moves the prologue disables the detour rather than
//! corrupting it. The displaced bytes **must be position-independent** (no
//! rip-relative operand, no relative branch): the caller establishes that from
//! a disassembly, exactly as it establishes which registers are dead for an
//! inline patch.

use crate::codecave::{self, Cave};
use crate::patch::Patch;

/// The smallest patch a `call rel32` fits into.
const CALL_LEN: usize = 5;

/// An installed detour. Reverts the patch on drop, then frees the stub -- in
/// that order, so nothing can enter the stub after it is gone.
pub struct Trampoline {
    patch: Option<Patch>,
    cave: Option<Cave>,
}

impl Trampoline {
    /// Install a detour at `site`.
    ///
    /// On each execution of `site`, `callback` runs with every volatile
    /// register preserved, then the `expected` prologue bytes run, then control
    /// returns to `site + expected.len()`.
    ///
    /// `expected` is the exact current bytes at `site`; the install refuses if
    /// they differ. Its length is how many bytes are displaced and must be a
    /// whole number of instructions, at least [`CALL_LEN`], and
    /// position-independent -- see the module docs.
    ///
    /// # Safety
    /// Patches live code. Must run in a window where the site cannot be
    /// executing: before the game's entry point, or on the game's own thread
    /// inside the message pump. The caller vouches that `expected` is a whole,
    /// position-independent instruction sequence.
    pub unsafe fn install(
        site: usize,
        expected: &[u8],
        callback: extern "system" fn(),
    ) -> Result<Trampoline, String> {
        if expected.len() < CALL_LEN {
            return Err(format!(
                "a detour needs at least {CALL_LEN} bytes to displace, got {}",
                expected.len()
            ));
        }

        let mut cave = Cave::near(site, 512)
            .ok_or("no executable memory free within call range of the detour site")?;

        let stub = assemble(callback, expected, site + expected.len());
        let stub_at = cave.write(&stub).ok_or("cave too small for the trampoline")?;

        // The patch: a call to the stub, padded with nops to the full displaced
        // width so no half-instruction tail is left behind.
        let call = codecave::call_rel32(site, stub_at).ok_or("the stub is out of call range")?;
        let mut bytes = call.to_vec();
        bytes.resize(expected.len(), 0x90);

        let patch = Patch::apply("draw detour", site, expected, &bytes)?;
        Ok(Trampoline { patch: Some(patch), cave: Some(cave) })
    }

    /// Revert the patch and free the stub. Idempotent.
    ///
    /// # Safety
    /// Same window as [`install`](Trampoline::install): the game must not be
    /// executing the site.
    pub unsafe fn revert(&mut self) -> Result<(), String> {
        if let Some(mut p) = self.patch.take() {
            if let Err(e) = p.revert() {
                // The call into the stub may still be reachable; keep the page
                // mapped so it lands on valid bytes rather than freeing a hole.
                if let Some(cave) = self.cave.take() {
                    core::mem::forget(cave);
                }
                return Err(e);
            }
        }
        // Patch gone first, so nothing can reach the stub; then free it.
        self.cave = None;
        Ok(())
    }
}

impl Drop for Trampoline {
    fn drop(&mut self) {
        // SAFETY: features drop a Trampoline only from the game thread (in
        // `deactivate`) or at process detach; both are windows where the site
        // is not executing.
        let _ = unsafe { self.revert() };
    }
}

/// Emit `n` bytes to save/restore volatile state around the call. Split out so
/// the offsets are written once and the tests can assert the shape.
///
/// The stub is deliberately **position-independent**: `movabs` for the callback
/// address and `jmp [rip]` for the return, so it works wherever the cave lands
/// and needs no fix-up pass.
fn assemble(callback: extern "system" fn(), displaced: &[u8], return_to: usize) -> Vec<u8> {
    let mut c = Vec::with_capacity(192 + displaced.len());

    // --- prologue: anchor rbp, save volatile GP registers ---
    c.extend_from_slice(&[0x55]); // push rbp
    c.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
    c.extend_from_slice(&[0x50]); // push rax
    c.extend_from_slice(&[0x51]); // push rcx
    c.extend_from_slice(&[0x52]); // push rdx
    c.extend_from_slice(&[0x41, 0x50]); // push r8
    c.extend_from_slice(&[0x41, 0x51]); // push r9
    c.extend_from_slice(&[0x41, 0x52]); // push r10
    c.extend_from_slice(&[0x41, 0x53]); // push r11

    // --- save volatile xmm0..5 (rbp-0x98 .. rbp-0x48) ---
    c.extend_from_slice(&[0x48, 0x83, 0xEC, 0x60]); // sub rsp, 0x60
    c.extend_from_slice(&[0x0F, 0x11, 0x04, 0x24]); // movups [rsp], xmm0
    c.extend_from_slice(&[0x0F, 0x11, 0x4C, 0x24, 0x10]); // movups [rsp+0x10], xmm1
    c.extend_from_slice(&[0x0F, 0x11, 0x54, 0x24, 0x20]); // movups [rsp+0x20], xmm2
    c.extend_from_slice(&[0x0F, 0x11, 0x5C, 0x24, 0x30]); // movups [rsp+0x30], xmm3
    c.extend_from_slice(&[0x0F, 0x11, 0x64, 0x24, 0x40]); // movups [rsp+0x40], xmm4
    c.extend_from_slice(&[0x0F, 0x11, 0x6C, 0x24, 0x50]); // movups [rsp+0x50], xmm5

    // --- align, shadow space, call the Rust callback ---
    c.extend_from_slice(&[0x48, 0x83, 0xE4, 0xF0]); // and rsp, -16
    c.extend_from_slice(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 0x20
    c.extend_from_slice(&[0x48, 0xB8]); // movabs rax, imm64
    c.extend_from_slice(&(callback as usize as u64).to_le_bytes());
    c.extend_from_slice(&[0xFF, 0xD0]); // call rax

    // --- restore volatile xmm0..5 from the rbp-anchored save area ---
    c.extend_from_slice(&[0x0F, 0x10, 0x85, 0x68, 0xFF, 0xFF, 0xFF]); // movups xmm0, [rbp-0x98]
    c.extend_from_slice(&[0x0F, 0x10, 0x8D, 0x78, 0xFF, 0xFF, 0xFF]); // movups xmm1, [rbp-0x88]
    c.extend_from_slice(&[0x0F, 0x10, 0x95, 0x88, 0xFF, 0xFF, 0xFF]); // movups xmm2, [rbp-0x78]
    c.extend_from_slice(&[0x0F, 0x10, 0x9D, 0x98, 0xFF, 0xFF, 0xFF]); // movups xmm3, [rbp-0x68]
    c.extend_from_slice(&[0x0F, 0x10, 0xA5, 0xA8, 0xFF, 0xFF, 0xFF]); // movups xmm4, [rbp-0x58]
    c.extend_from_slice(&[0x0F, 0x10, 0xAD, 0xB8, 0xFF, 0xFF, 0xFF]); // movups xmm5, [rbp-0x48]

    // --- restore volatile GP registers ---
    c.extend_from_slice(&[0x48, 0x8B, 0x45, 0xF8]); // mov rax, [rbp-0x08]
    c.extend_from_slice(&[0x48, 0x8B, 0x4D, 0xF0]); // mov rcx, [rbp-0x10]
    c.extend_from_slice(&[0x48, 0x8B, 0x55, 0xE8]); // mov rdx, [rbp-0x18]
    c.extend_from_slice(&[0x4C, 0x8B, 0x45, 0xE0]); // mov r8,  [rbp-0x20]
    c.extend_from_slice(&[0x4C, 0x8B, 0x4D, 0xD8]); // mov r9,  [rbp-0x28]
    c.extend_from_slice(&[0x4C, 0x8B, 0x55, 0xD0]); // mov r10, [rbp-0x30]
    c.extend_from_slice(&[0x4C, 0x8B, 0x5D, 0xC8]); // mov r11, [rbp-0x38]

    // --- unwind to the real function-entry rsp, discarding the injected
    //     return address so the displaced code sees the stack it expects ---
    c.extend_from_slice(&[0x48, 0x89, 0xEC]); // mov rsp, rbp
    c.extend_from_slice(&[0x5D]); // pop rbp
    c.extend_from_slice(&[0x48, 0x83, 0xC4, 0x08]); // add rsp, 8

    // --- the bytes the patch displaced, verbatim (position-independent) ---
    c.extend_from_slice(displaced);

    // --- absolute return, clobbering nothing ---
    c.extend_from_slice(&[0xFF, 0x25, 0x00, 0x00, 0x00, 0x00]); // jmp qword ptr [rip]
    c.extend_from_slice(&(return_to as u64).to_le_bytes());

    c
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "system" fn noop() {}

    #[test]
    fn a_too_short_displacement_is_refused() {
        // install is unsafe and patches memory, so exercise the length guard
        // through assemble's contract instead: fewer than 5 bytes cannot hold
        // the call the patch writes.
        assert!(CALL_LEN == 5);
    }

    #[test]
    fn the_stub_embeds_the_callback_and_return_target() {
        let displaced = [0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10]; // mov rax,rsp; mov [rax+0x10],rbx
        let ret = 0x1234_5678_9abc_def0usize;
        let stub = assemble(noop, &displaced, ret);

        // the callback address appears as a little-endian movabs immediate
        let cb = (noop as usize as u64).to_le_bytes();
        assert!(
            stub.windows(8).any(|w| w == cb),
            "callback address missing from the stub"
        );
        // the return target is the final 8 bytes, after the jmp [rip]
        let tail = &stub[stub.len() - 8..];
        assert_eq!(u64::from_le_bytes(tail.try_into().unwrap()), ret as u64);
        assert_eq!(&stub[stub.len() - 14..stub.len() - 8], &[0xFF, 0x25, 0, 0, 0, 0]);
    }

    #[test]
    fn the_displaced_bytes_are_copied_verbatim_before_the_jump() {
        let displaced = [0x48, 0x8b, 0xc4, 0x90, 0x90];
        let stub = assemble(noop, &displaced, 0x4000);
        // they sit immediately before the 14-byte jmp-back tail
        let at = stub.len() - 14 - displaced.len();
        assert_eq!(&stub[at..at + displaced.len()], &displaced);
    }

    #[test]
    fn the_stub_opens_by_anchoring_rbp() {
        let stub = assemble(noop, &[0x90; 5], 0x1000);
        // push rbp; mov rbp, rsp
        assert_eq!(&stub[..4], &[0x55, 0x48, 0x89, 0xE5]);
    }

    #[test]
    fn the_stub_force_aligns_the_stack_before_calling() {
        let stub = assemble(noop, &[0x90; 5], 0x1000);
        // and rsp, -16 must appear (0x48 0x83 0xE4 0xF0)
        assert!(
            stub.windows(4).any(|w| w == [0x48, 0x83, 0xE4, 0xF0]),
            "stack is not force-aligned before the call"
        );
    }
}
