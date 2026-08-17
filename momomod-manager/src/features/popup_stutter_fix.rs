//! The resource-gain popups are the in-run stutter. This rounds one number.
//!
//! ## What the measurement said
//!
//! Ten minutes of a real run. The share of samples taken while the game was overdue to
//! draw climbs as the run progresses -- 11.6% at 90s, 23% at 210s, 43% at 300s, **52.9%
//! at 540s** -- and unlike the main menu, **80% of those stalls are inside the game's
//! own code**. The top three stall stacks are one chain, about half of all stalled
//! samples:
//!
//! ```text
//! obj_resource_gained_Draw_0
//!   -> scribble
//!     -> @@NewGMLObject@@              (the GML `new` operator)
//!       -> __scribble_class_element
//!         -> ... -> MemoryManager -> the allocator   (48% of stall time)
//! ```
//!
//! ## The cause is one line of GML
//!
//! `obj_resource_gained_Draw_0` builds its text as
//!
//! ```text
//! "[fnt_pixel][fa_center][fa_middle][alpha, " + string(alpha) + "]" + text
//! ```
//!
//! and `alpha` is the popup's fade, recomputed every frame from an animation curve. In
//! Scribble **the string is the cache key**, so every frame, for every popup, the key is
//! new, the cache misses, and a complete text model is parsed, typeset, allocated and
//! thrown away.
//!
//! The game's own fix would be `.blend(c_white, alpha)`, which does not touch the cache
//! key. We cannot change its source, so we round the number instead: at 10 steps the
//! string takes ten distinct values rather than sixty a second, and the cache hits.
//!
//! ## Why a state write cannot do this, and a patch can
//!
//! The obvious cheap fix -- have the frame hook round `alpha` on each instance -- does
//! not work, and it is worth recording why. `obj_resource_gained_Step_0` **recomputes**
//! `alpha` from `curve_progress` every frame, and Step runs before Draw. Anything
//! written from the frame hook is overwritten before Draw ever reads it. In the variant
//! where it did stick, it would freeze the fade and the popups would never expire.
//!
//! So the rounding has to happen between the read and the string, which means changing
//! the code.
//!
//! ## The patch
//!
//! At the point the alpha RValue is copied to a temporary:
//!
//! ```text
//! 015bc8ea  je     0x15bc91a
//! 015bc8ec  movups xmm0, [rdi]           <- 3 bytes, the whole RValue
//! 015bc8ef  movups [rsp+0x30], xmm0      <- 5 bytes
//! 015bc8f4  mov    ecx, [rbp+0xc]        <- next instruction
//! ```
//!
//! Exactly **eight bytes**, replaced by `call rel32` to a stub plus three `nop`. Three
//! properties make this safe rather than hopeful, and each was checked rather than
//! assumed:
//!
//! * **`rax` is dead here.** Last read by the `cmp rdi, rax` two instructions earlier,
//!   next written at `0x15bc91a`. Nothing between touches it, so the stub may use it as
//!   scratch without saving anything.
//! * **`flags` and `kind` survive.** An RValue is `{f64 payload, u32 flags, u32 kind}`.
//!   `mulsd` and `cvtsi2sd` write only the low 64 bits and leave bits 64..127 alone, so
//!   rounding in place preserves the tag. Rebuilding the value would have to know it.
//! * **The fade is untouched.** `[alpha, N]` sets only the *text's* transparency; the
//!   sprite and the upward motion still animate off the real curve.
//!
//! ## When it is safe to apply
//!
//! Patching code a thread might be executing is how a mod kills a game. There are two
//! windows here and the feature uses both:
//!
//! * **At startup**, from the kit's own thread, the game has not reached its entry
//!   point -- no game code has run at all.
//! * **Mid-session**, from the frame hook, we are *on the game's thread*, inside
//!   `PeekMessageW`. The game is single-threaded, so it is provably not inside a Draw
//!   event at that moment.
//!
//! Both are checked in [`PopupStutterFix::activate`] rather than assumed.

use tkiw_runtime::{
    codecave::{self, Cave},
    guard::Signature,
    hook, logln,
    patch::Patch,
    Runtime,
};

use crate::config::Section;
use crate::feature::{Cadence, Feature, Requirements};

/// Where the alpha RValue is copied to a temporary, in `obj_resource_gained_Draw_0`.
const SITE: usize = 0x15bc8ec;

/// `movups xmm0,[rdi]` then `movups [rsp+0x30],xmm0` -- the eight bytes we replace.
const EXPECT: &[u8] = &[0x0f, 0x10, 0x07, 0x0f, 0x11, 0x44, 0x24, 0x30];

/// Stack displacement of the destination, adjusted for the return address our `call`
/// pushes. The original stores to `[rsp+0x30]`; inside the stub that same slot is eight
/// bytes further up. Getting this wrong corrupts a neighbouring local, which is exactly
/// the kind of bug that looks like anything but its cause.
const DEST_DISP: u8 = 0x30 + 8;

pub struct PopupStutterFix {
    steps: f64,
    patch: Option<Patch>,
    cave: Option<Cave>,
}

impl Default for PopupStutterFix {
    fn default() -> PopupStutterFix {
        PopupStutterFix { steps: 10.0, patch: None, cave: None }
    }
}

/// Assemble the stub, given the addresses its two constants will live at.
///
/// ```text
///   movups    xmm0, [rdi]           ; the whole RValue
///   mulsd     xmm0, [rip+steps]     ; * steps
///   cvttsd2si rax,  xmm0            ; truncate towards zero
///   cvtsi2sd  xmm0, rax             ; back to a double, upper half preserved
///   mulsd     xmm0, [rip+inv]       ; * (1/steps)
///   movups    [rsp+DEST_DISP], xmm0
///   ret
/// ```
fn assemble(stub_at: usize, steps_at: usize, inv_at: usize) -> Option<Vec<u8>> {
    let mut code: Vec<u8> = Vec::with_capacity(40);

    // A rip-relative displacement is measured from the end of the instruction, so it
    // can only be computed once the instruction's own length is known.
    fn rip(code: &mut Vec<u8>, stub_at: usize, opcode: &[u8], target: usize) -> Option<()> {
        let end = stub_at + code.len() + opcode.len() + 4;
        let rel: i32 = (target as i64 - end as i64).try_into().ok()?;
        code.extend_from_slice(opcode);
        code.extend_from_slice(&rel.to_le_bytes());
        Some(())
    }

    code.extend_from_slice(&[0x0f, 0x10, 0x07]); // movups xmm0, [rdi]
    rip(&mut code, stub_at, &[0xf2, 0x0f, 0x59, 0x05], steps_at)?; // mulsd xmm0,[rip+steps]
    code.extend_from_slice(&[0xf2, 0x48, 0x0f, 0x2c, 0xc0]); // cvttsd2si rax, xmm0
    code.extend_from_slice(&[0xf2, 0x48, 0x0f, 0x2a, 0xc0]); // cvtsi2sd  xmm0, rax
    rip(&mut code, stub_at, &[0xf2, 0x0f, 0x59, 0x05], inv_at)?; // mulsd xmm0,[rip+inv]
    code.extend_from_slice(&[0x0f, 0x11, 0x44, 0x24, DEST_DISP]); // movups [rsp+d], xmm0
    code.push(0xc3); // ret
    Some(code)
}

impl Feature for PopupStutterFix {
    fn name(&self) -> &'static str {
        "popup_stutter_fix"
    }

    fn module(&self) -> &'static str {
        "optimization"
    }

    fn summary(&self) -> &'static str {
        "Removes the in-run stutter caused by the floating resource-gain numbers, which \
         rebuild their text every frame because the fade is baked into it."
    }

    /// Defaults **on**, an exception to the kit's off-by-default rule.
    ///
    /// The kit's rule is that features default off unless the exception is stated and
    /// justified. This one qualifies: it is verified by measurement rather than by
    /// argument (the allocator frames left the stall profile entirely, and the cost
    /// stopped scaling with production), it cannot alter the state of a run, and the
    /// only thing a player can perceive is that the popup text fades in ten steps
    /// instead of continuously -- which has to be looked for to be seen.
    ///
    /// Anyone who would rather have the smooth fade sets `enabled = false`.
    fn default_enabled(&self) -> bool {
        true
    }

    fn requires(&self) -> Requirements {
        Requirements {
            functions: &["gml_Object_obj_resource_gained_Draw_0"],
            signatures: &[Signature {
                what: "obj_resource_gained_Draw_0 alpha copy",
                rva: SITE,
                bytes: EXPECT,
            }],
            objects: &["obj_resource_gained"],
            ..Requirements::default()
        }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        let steps = section.u64("steps", 10)?;
        if !(1..=255).contains(&steps) {
            return Err(format!("steps: {steps} is outside 1..255"));
        }
        self.steps = steps as f64;
        for k in section.unknown(&["enabled", "steps"]) {
            logln!("[popup_stutter_fix] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn activate(&mut self, rt: &Runtime) -> Result<(), String> {
        // See the module docs: patching is safe before the game has run any code, and
        // safe from the frame hook because we are then on the game's own single thread.
        // Anything else -- a reload arriving from some other context -- is not.
        let at_startup = hook::frames() == 0;
        let this_thread = unsafe { tkiw_runtime::win::GetCurrentThreadId() } as u64;
        let on_game_thread = hook::game_thread() != 0 && hook::game_thread() == this_thread;
        if !at_startup && !on_game_thread {
            return Err("refusing to patch: the game is running and this is not its \
                        thread, so code could be executing at the patch site"
                .into());
        }

        let site = rt.base + SITE;
        let mut cave = Cave::near(site, 256)
            .ok_or("no executable memory free within call range of the patch site")?;

        // Constants first, so the stub's rip-relative displacements are known.
        let steps_at = cave.reserve_aligned(8, 16).ok_or("cave too small for constants")?;
        let inv_at = cave.reserve_aligned(8, 16).ok_or("cave too small for constants")?;
        for (at, v) in [(steps_at, self.steps), (inv_at, 1.0 / self.steps)] {
            for (i, b) in v.to_bits().to_le_bytes().iter().enumerate() {
                unsafe { core::ptr::write_volatile((at + i) as *mut u8, *b) };
            }
        }

        let stub_at = cave.reserve_aligned(0, 16).ok_or("cave too small for the stub")?;
        let code = assemble(stub_at, steps_at, inv_at).ok_or("stub constants out of range")?;
        let written = cave.write(&code).ok_or("cave too small for the stub")?;
        if written != stub_at {
            return Err("cave layout disagreed with itself".into());
        }

        let call = codecave::call_rel32(site, stub_at).ok_or("the stub is out of call range")?;
        let mut bytes = call.to_vec();
        bytes.extend_from_slice(&[0x90; 3]); // pad to the full eight

        // SAFETY: the window was established above -- either no game code has executed
        // yet, or we are on the game's own thread and it is inside PeekMessageW.
        let patch =
            unsafe { Patch::apply("obj_resource_gained alpha rounding", site, EXPECT, &bytes) }?;

        self.patch = Some(patch);
        self.cave = Some(cave);
        logln!(
            "[popup_stutter_fix] rounding popup alpha to {} step(s); stub at {stub_at:#x}, \
             patched {site:#x} ({})",
            self.steps as u64,
            if at_startup { "before the game started" } else { "from the game's thread" }
        );
        Ok(())
    }

    fn deactivate(&mut self, _rt: &Runtime) {
        if let Some(p) = self.patch.as_mut() {
            // SAFETY: deactivation reaches us either from the frame hook (the game's
            // thread, inside PeekMessageW) or at process detach.
            match unsafe { p.revert() } {
                Ok(()) => logln!("[popup_stutter_fix] patch reverted"),
                Err(e) => {
                    // The call into the stub is still live. Freeing the cave now would
                    // point it at unmapped memory, so leak the page deliberately: a
                    // wasted 64 KB beats a jump into nothing.
                    logln!(
                        "[popup_stutter_fix] WARNING: could not revert ({e}); keeping the \
                         stub mapped so the call still lands somewhere valid"
                    );
                    if let Some(cave) = self.cave.take() {
                        core::mem::forget(cave);
                    }
                    self.patch = None;
                    return;
                }
            }
        }
        // Order matters: the patch is gone, so nothing can call the stub any more.
        self.patch = None;
        self.cave = None;
    }

    /// Nothing per-frame: the patch does the work.
    fn cadence(&self) -> Cadence {
        Cadence::Never
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_patch_site_matches_the_analysed_build() {
        let candidates = [
            r"..\tkiw-morale-fix\The King is Watching.exe.orig",
            r"C:\Program Files (x86)\Steam\steamapps\common\The King is Watching\The King is Watching.exe",
        ];
        let Some(img) = candidates.iter().find_map(|p| tkiw_runtime::pe::Image::load(p)) else {
            eprintln!("no game executable found; skipping");
            return;
        };
        let off = img.rva2off(SITE as u32).expect("site is file-backed");
        assert_eq!(&img.data[off..off + EXPECT.len()], EXPECT, "the patch site has moved");
    }

    /// The replacement must be exactly the size of what it replaces: shorter leaves a
    /// stray tail of the old instruction, longer overwrites the next one.
    #[test]
    fn the_replacement_is_exactly_eight_bytes() {
        let call = codecave::call_rel32(0x1000, 0x1100).unwrap();
        assert_eq!(call.len() + 3, EXPECT.len());
    }

    /// The stub must load the whole RValue, return, and store back one slot higher to
    /// account for the return address the call pushes.
    #[test]
    fn the_stub_assembles_and_addresses_the_right_slot() {
        let code = assemble(0x1000, 0x2000, 0x2010).expect("assembles");
        assert_eq!(&code[..3], &[0x0f, 0x10, 0x07], "must load the whole RValue");
        assert_eq!(*code.last().unwrap(), 0xc3, "must return");
        let store = [0x0f, 0x11, 0x44, 0x24, DEST_DISP];
        assert!(
            code.windows(store.len()).any(|w| w == store),
            "the store must target [rsp+0x38], not the original [rsp+0x30]"
        );
        assert_eq!(DEST_DISP, 0x38, "the call pushes 8 bytes, so the slot moves by 8");
    }

    /// The rip-relative displacements must be computed from the end of each
    /// instruction. A stub assembled at a different address must differ only in those.
    #[test]
    fn rip_displacements_track_the_stub_address() {
        let a = assemble(0x1000, 0x2000, 0x2010).unwrap();
        let b = assemble(0x1100, 0x2000, 0x2010).unwrap();
        assert_eq!(a.len(), b.len(), "same instructions either way");
        assert_ne!(a, b, "displacements must depend on where the stub lives");
    }

    /// A constant too far away must be refused, not silently encoded with a wrapped
    /// displacement.
    #[test]
    fn an_out_of_range_constant_is_refused() {
        assert!(assemble(0x1000, 0x1000 + 0x9000_0000, 0x2000).is_none());
    }
}
