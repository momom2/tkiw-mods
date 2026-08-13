//! Resuming a reign no longer drops morale to 0 and crawls back up.
//!
//! The game keeps `morale_current` (what King effects read) and `morale_target`. Loading
//! a save restores `morale_target` but leaves `morale_current` at 0, so it climbs via
//! `approach()` at 0.15/frame scaled by `TIME_SCALE` -- which is 0 whenever gameplay is
//! frozen. This assigns `morale_current = morale_target` once, on resume. Fresh runs are
//! untouched.
//!
//! Ported from the standalone `tkiw-morale-fix`, which patches the executable on disk.
//! Same two hooks, same stubs; the difference is that this applies them in memory, so
//! there is nothing to uninstall and the guard checks the bytes on every launch.
//!
//! **The two do not collide.** If the file patch is installed, these sites hold a `jmp`
//! rather than the original bytes, the signature check fails, and this feature reports
//! itself unsupported and stays off.
//!
//! ## The hooks
//!
//! **A**, in the run controller's `setup`, right after the saved `gc_stat_mods` are
//! applied: arm a latch, but only when `PLAYER_CONTINUED_RUN` is true -- `setup` also
//! runs for fresh runs, and that flag is what makes this resume-only.
//!
//! **B**, in the gameplay controller's Step, where `approach()`'s result is stored into
//! `morale_current`: when armed, store `morale_target` (in `rbx`) instead. Going through
//! the store rather than writing the variable means the snap does not depend on
//! `approach()` or `SCALED_DELTA` at all.

use tkiw_runtime::{
    codecave::{self, Cave},
    guard::Signature,
    hook, logln,
    patch::Patch,
    Runtime,
};

use crate::config::Section;
use crate::feature::{Feature, Requirements};

/// Run controller `setup`, after the saved stat mods are applied.
const SITE_A: usize = 0x1610842;
/// `mov [rsp+0x30], r12` then `mov [rsp+0x38], r12d`.
const EXPECT_A: &[u8] = &[0x4c, 0x89, 0x64, 0x24, 0x30, 0x44, 0x89, 0x64, 0x24, 0x38];
/// `[rbp+0x258]` holds `RValue* PLAYER_CONTINUED_RUN` at hook A.
const PCR_DISP: i32 = 0x258;

/// Gameplay controller Step, storing the approach result into `morale_current`.
const SITE_B: usize = 0x1317262;
/// `mov rdx, rax` / `mov rcx, rsi` / `call COPY_RValue`.
const EXPECT_B: &[u8] = &[0x48, 0x8b, 0xd0, 0x48, 0x8b, 0xce, 0xe8, 0x23, 0x86, 0xd7, 0xfe];

/// `COPY_RValue(dst, src)`.
const COPY_RVALUE: usize = 0x8f890;

pub struct MoraleFix {
    patches: Vec<Patch>,
    cave: Option<Cave>,
}

impl Default for MoraleFix {
    fn default() -> MoraleFix {
        MoraleFix { patches: Vec::new(), cave: None }
    }
}

/// `jmp rel32`, which is `call rel32` with one byte changed.
fn jmp_rel32(at: usize, target: usize) -> Option<[u8; 5]> {
    let mut b = codecave::call_rel32(at, target)?;
    b[0] = 0xe9;
    Some(b)
}

fn rel32(from: usize, insn_len: usize, to: usize) -> Option<[u8; 4]> {
    let rel: i32 = (to as i64 - (from + insn_len) as i64).try_into().ok()?;
    Some(rel.to_le_bytes())
}

/// `if (PLAYER_CONTINUED_RUN) armed = 1;` then the displaced instructions, then back.
fn stub_a(at: usize, armed: usize, site: usize) -> Option<Vec<u8>> {
    let mut c: Vec<u8> = Vec::new();
    let mut p = at;
    c.push(0x50); // push rax
    p += 1;
    c.extend_from_slice(&[0x48, 0x8b, 0x85]); // mov rax, [rbp+PCR_DISP]
    c.extend_from_slice(&PCR_DISP.to_le_bytes());
    p += 7;
    c.extend_from_slice(&[0x48, 0x8b, 0x00]); // mov rax, [rax]
    p += 3;
    c.extend_from_slice(&[0x48, 0x85, 0xc0]); // test rax, rax
    p += 3;
    c.extend_from_slice(&[0x74, 0x07]); // je skip
    p += 2;
    c.extend_from_slice(&[0xc6, 0x05]); // mov byte [armed], 1
    c.extend_from_slice(&rel32(p, 7, armed)?);
    c.push(0x01);
    p += 7;
    c.push(0x58); // skip: pop rax
    p += 1;
    c.extend_from_slice(EXPECT_A);
    p += EXPECT_A.len();
    c.push(0xe9);
    c.extend_from_slice(&rel32(p, 5, site + EXPECT_A.len())?);
    Some(c)
}

/// `rdx = armed ? morale_target(rbx) : approach result(rax)`, then the original store.
fn stub_b(at: usize, armed: usize, site: usize, copy_rvalue: usize) -> Option<Vec<u8>> {
    let mut c: Vec<u8> = Vec::new();
    let mut p = at;
    c.extend_from_slice(&[0x80, 0x3d]); // cmp byte [armed], 0
    c.extend_from_slice(&rel32(p, 7, armed)?);
    c.push(0x00);
    p += 7;
    c.extend_from_slice(&[0x74, 0x0c]); // je normal
    p += 2;
    c.extend_from_slice(&[0xc6, 0x05]); // mov byte [armed], 0
    c.extend_from_slice(&rel32(p, 7, armed)?);
    c.push(0x00);
    p += 7;
    c.extend_from_slice(&[0x48, 0x8b, 0xd3]); // mov rdx, rbx
    p += 3;
    c.extend_from_slice(&[0xeb, 0x03]); // jmp have
    p += 2;
    c.extend_from_slice(&[0x48, 0x8b, 0xd0]); // normal: mov rdx, rax
    p += 3;
    c.extend_from_slice(&[0x48, 0x8b, 0xce]); // have: mov rcx, rsi
    p += 3;
    c.push(0xe8); // call COPY_RValue
    c.extend_from_slice(&rel32(p, 5, copy_rvalue)?);
    p += 5;
    c.push(0xe9);
    c.extend_from_slice(&rel32(p, 5, site + EXPECT_B.len())?);
    Some(c)
}

impl Feature for MoraleFix {
    fn name(&self) -> &'static str {
        "morale_fix"
    }

    fn module(&self) -> &'static str {
        "bugfixes"
    }

    fn summary(&self) -> &'static str {
        "Restores morale instead of snapping it to zero when resuming a reign."
    }

    /// Defaults **on**. It restores what the game already intends, costs nothing, and
    /// cannot alter a save -- morale_current is derived state, recomputed either way.
    fn default_enabled(&self) -> bool {
        true
    }

    fn requires(&self) -> Requirements {
        Requirements {
            signatures: &[
                Signature { what: "morale resume: run controller setup", rva: SITE_A, bytes: EXPECT_A },
                Signature { what: "morale resume: morale_current store", rva: SITE_B, bytes: EXPECT_B },
            ],
            ..Requirements::default()
        }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        for k in section.unknown(&["enabled"]) {
            logln!("[morale_fix] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn activate(&mut self, rt: &Runtime) -> Result<(), String> {
        let this_thread = unsafe { tkiw_runtime::win::GetCurrentThreadId() } as u64;
        let safe = hook::frames() == 0
            || (hook::game_thread() != 0 && hook::game_thread() == this_thread);
        if !safe {
            return Err("refusing to patch: the game is running and this is not its thread".into());
        }

        let (site_a, site_b) = (rt.base + SITE_A, rt.base + SITE_B);
        let mut cave = Cave::near(site_a, 512).ok_or("no executable memory within reach")?;

        let armed = cave.reserve_aligned(8, 16).ok_or("cave too small")?;
        unsafe { core::ptr::write_volatile(armed as *mut u64, 0) };

        let at_a = cave.reserve_aligned(0, 16).ok_or("cave too small")?;
        let code_a = stub_a(at_a, armed, site_a).ok_or("stub A out of range")?;
        if cave.write(&code_a) != Some(at_a) {
            return Err("cave layout disagreed with itself".into());
        }

        let at_b = cave.reserve_aligned(0, 16).ok_or("cave too small")?;
        let code_b =
            stub_b(at_b, armed, site_b, rt.base + COPY_RVALUE).ok_or("stub B out of range")?;
        if cave.write(&code_b) != Some(at_b) {
            return Err("cave layout disagreed with itself".into());
        }

        for (what, site, expect, stub) in [
            ("morale resume hook A", site_a, EXPECT_A, at_a),
            ("morale resume hook B", site_b, EXPECT_B, at_b),
        ] {
            let jmp = jmp_rel32(site, stub).ok_or("stub out of jmp range")?;
            let mut bytes = jmp.to_vec();
            bytes.resize(expect.len(), 0x90);
            // SAFETY: the window was established above.
            match unsafe { Patch::apply(what, site, expect, &bytes) } {
                Ok(p) => self.patches.push(p),
                Err(e) => {
                    // Half-applied is worse than not applied: undo whatever landed.
                    self.deactivate(rt);
                    return Err(e);
                }
            }
        }

        self.cave = Some(cave);
        logln!("[morale_fix] on; hooks at {site_a:#x} and {site_b:#x}");
        Ok(())
    }

    fn deactivate(&mut self, _rt: &Runtime) {
        for p in self.patches.iter_mut() {
            // SAFETY: same windows as activation.
            let _ = unsafe { p.revert() };
        }
        self.patches.clear();
        self.cave = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both stubs must end by jumping back to just past the bytes they displaced,
    /// and must replay those bytes -- a stub that swallowed them would skip real work.
    #[test]
    fn stub_a_replays_and_returns() {
        let code = stub_a(0x10000, 0x10f00, 0x20000).expect("assembles");
        assert!(code.windows(EXPECT_A.len()).any(|w| w == EXPECT_A), "displaced bytes missing");
        assert_eq!(code[code.len() - 5], 0xe9, "does not end in a jmp");
        let rel = i32::from_le_bytes(code[code.len() - 4..].try_into().unwrap());
        let from = 0x10000 + code.len();
        assert_eq!((from as i64 + rel as i64) as usize, 0x20000 + EXPECT_A.len());
    }

    #[test]
    fn stub_b_calls_copy_rvalue_and_returns() {
        let code = stub_b(0x10000, 0x10f00, 0x20000, 0x30000).expect("assembles");
        let call = code.iter().rposition(|b| *b == 0xe8).expect("no call");
        let rel = i32::from_le_bytes(code[call + 1..call + 5].try_into().unwrap());
        assert_eq!((0x10000 + call + 5) as i64 + rel as i64, 0x30000);
        let rel = i32::from_le_bytes(code[code.len() - 4..].try_into().unwrap());
        let from = 0x10000 + code.len();
        assert_eq!((from as i64 + rel as i64) as usize, 0x20000 + EXPECT_B.len());
    }

    /// The patch must fill the whole verified window, or a stray byte of the old
    /// instruction is left to execute after the jump.
    #[test]
    fn the_jump_is_padded_to_the_displaced_length() {
        let jmp = jmp_rel32(0x10000, 0x10100).expect("in range");
        let mut bytes = jmp.to_vec();
        bytes.resize(EXPECT_B.len(), 0x90);
        assert_eq!(bytes.len(), EXPECT_B.len());
        assert!(bytes[5..].iter().all(|b| *b == 0x90));
    }

    #[test]
    fn an_unreachable_stub_is_refused() {
        assert!(stub_a(0x10000, 0x10000 + 0x9000_0000, 0x20000).is_none());
    }
}
