//! "Only one copy of this may act."
//!
//! Two mods can end up holding the same logic: one absorbed into a host, one still
//! installed as its own DLL. They load into the same process and neither can see the
//! other's statics, so a shared flag cannot work. A named kernel object can.
//!
//! The rule this exists to enforce is not "one instance" for its own sake. Two copies
//! of a mod that *presses buttons in the game* would press twice on one screen, which
//! is a corrupted run rather than a cosmetic bug.

use crate::win::{self, Handle};

/// `ERROR_ALREADY_EXISTS` -- the object was there before this call.
const ALREADY_EXISTS: u32 = 183;
/// `SYNCHRONIZE`, the least access that still proves the object exists.
const SYNCHRONIZE: u32 = 0x0010_0000;

/// Session-local, so this never reaches across users on a shared machine.
fn full_name(name: &str) -> Vec<u16> {
    format!("Local\\{name}\0").encode_utf16().collect()
}

/// A claim held for as long as this value lives.
pub struct Claim {
    handle: Handle,
    name: String,
}

// SAFETY: the handle is a process-wide kernel handle. It is only ever closed, once,
// by `Drop`, and the kernel object it names is not thread-affine -- unlike, say, a
// window handle. Holding one in a static is the whole point: the claim must outlive
// the thread that took it.
unsafe impl Send for Claim {}
unsafe impl Sync for Claim {}

impl Claim {
    /// Take the claim, or `None` if something else in this process already has it.
    ///
    /// The handle is kept: dropping it releases the claim, which is why this returns a
    /// value to hold rather than a bare bool.
    pub fn take(name: &str) -> Option<Claim> {
        let wide = full_name(name);
        // SAFETY: a null security descriptor and a NUL-terminated name.
        let handle = unsafe { win::CreateMutexW(core::ptr::null_mut(), 0, wide.as_ptr()) };
        if handle.is_null() {
            return None;
        }
        if unsafe { win::GetLastError() } == ALREADY_EXISTS {
            unsafe { win::CloseHandle(handle) };
            return None;
        }
        Some(Claim { handle, name: name.to_string() })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        // SAFETY: ours, opened above, closed once.
        unsafe { win::CloseHandle(self.handle) };
    }
}

/// Whether someone holds this claim, without taking it.
///
/// Deliberately separate from [`Claim::take`]: a component that must *yield* to another
/// should ask this, not race for the claim itself.
pub fn held(name: &str) -> bool {
    let wide = full_name(name);
    // SAFETY: NUL-terminated name; the handle is closed immediately.
    let handle = unsafe { win::OpenMutexW(SYNCHRONIZE, 0, wide.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    unsafe { win::CloseHandle(handle) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_is_visible_while_held_and_gone_after() {
        let name = "tkiw_runtime_claim_selftest";
        assert!(!held(name));
        let c = Claim::take(name).expect("first take succeeds");
        assert!(held(name));
        assert!(Claim::take(name).is_none(), "a second take must fail");
        drop(c);
        assert!(!held(name), "the claim outlived its holder");
    }

    /// The name is session-local, so it must not be taken verbatim.
    #[test]
    fn names_are_session_scoped() {
        let w = full_name("x");
        let s = String::from_utf16_lossy(&w[..w.len() - 1]);
        assert_eq!(s, "Local\\x");
    }
}
