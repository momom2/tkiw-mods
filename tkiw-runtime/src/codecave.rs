//! Executable memory close enough to the game to `call` into.
//!
//! A patch that needs to *do* something, rather than merely stop doing something, has
//! to put its new instructions somewhere. There is rarely room at the patch site: the
//! bytes being replaced are a handful, and the replacement is tens.
//!
//! So the replacement lives here, and the patch site gets a five-byte
//! `call rel32` to it. That encoding is the whole constraint on this module:
//! **`rel32` is a signed 32-bit displacement**, so the stub must land within ±2 GB of
//! the call site. `VirtualAlloc` with no hint will happily satisfy the request from
//! anywhere in a 64-bit address space, and a call that cannot reach is not a subtle
//! bug -- the displacement silently wraps and the game jumps into nothing.
//!
//! Hence [`Cave::near`], which scans outward from the target for an address that both
//! allocates and is in range, and refuses rather than returning something unreachable.
//!
//! ## What this deliberately is not
//!
//! Not a detour engine. There is no register preservation, no instruction-length
//! decoding, no relocation of moved instructions. The caller writes the exact bytes it
//! wants and is responsible for knowing which registers are dead at the patch site --
//! which, for a specific site you have disassembled, is a far smaller and more
//! checkable question than solving it in general.

use core::ffi::c_void;

use crate::win;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;

/// `call rel32` reaches this far in either direction. Leave a margin so a stub near
/// the edge cannot be defeated by rounding.
const REACH: usize = 0x7000_0000;

#[link(name = "kernel32")]
extern "system" {
    fn VirtualAlloc(addr: *mut c_void, size: usize, typ: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, typ: u32) -> i32;
}

/// A page of executable memory, freed when dropped.
pub struct Cave {
    addr: usize,
    size: usize,
    used: usize,
}

impl Cave {
    /// Allocate `size` bytes of executable memory within `rel32` reach of `target`.
    ///
    /// Scans outward in 64 KB steps -- the allocation granularity -- taking the first
    /// address the kernel accepts. Returns `None` if nothing in range is free, which is
    /// vanishingly unlikely in a 2 GB window but is a real answer rather than an
    /// unreachable pointer.
    pub fn near(target: usize, size: usize) -> Option<Cave> {
        const GRANULARITY: usize = 0x1_0000;
        let size = size.max(1);

        // Below the target first, then above: the game's image sits low in its own
        // reservation, so the space just under it is usually free.
        let lo = target.saturating_sub(REACH);
        let mut offset = GRANULARITY;
        while offset < REACH {
            for candidate in [target.checked_sub(offset), target.checked_add(offset)]
                .into_iter()
                .flatten()
            {
                if candidate < lo || candidate < GRANULARITY {
                    continue;
                }
                let base = candidate & !(GRANULARITY - 1);
                let p = unsafe {
                    VirtualAlloc(
                        base as *mut c_void,
                        size,
                        MEM_COMMIT | MEM_RESERVE,
                        PAGE_EXECUTE_READWRITE,
                    )
                };
                if !p.is_null() {
                    let addr = p as usize;
                    if in_reach(target, addr) && in_reach(target, addr + size) {
                        return Some(Cave { addr, size, used: 0 });
                    }
                    // Allocated but out of reach: give it straight back rather than
                    // leaking a page we cannot use.
                    unsafe { VirtualFree(p, 0, MEM_RELEASE) };
                }
            }
            offset += GRANULARITY;
        }
        None
    }

    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Append bytes, returning the address they were written at.
    pub fn write(&mut self, bytes: &[u8]) -> Option<usize> {
        if self.used + bytes.len() > self.size {
            return None;
        }
        let at = self.addr + self.used;
        for (i, b) in bytes.iter().enumerate() {
            unsafe { core::ptr::write_volatile((at + i) as *mut u8, *b) };
        }
        self.used += bytes.len();
        win::flush_instruction_cache(at, bytes.len());
        Some(at)
    }

    /// Reserve `n` bytes, 16-byte aligned, for data the stub reads rip-relatively.
    pub fn reserve_aligned(&mut self, n: usize, align: usize) -> Option<usize> {
        let pad = (align - ((self.addr + self.used) % align)) % align;
        if self.used + pad + n > self.size {
            return None;
        }
        self.used += pad;
        let at = self.addr + self.used;
        self.used += n;
        Some(at)
    }
}

impl Drop for Cave {
    fn drop(&mut self) {
        unsafe { VirtualFree(self.addr as *mut c_void, 0, MEM_RELEASE) };
    }
}

fn in_reach(from: usize, to: usize) -> bool {
    let delta = (to as i64) - (from as i64);
    (-(REACH as i64)..=(REACH as i64)).contains(&delta)
}

/// The five bytes of a `call rel32` from `at` to `target`.
///
/// Returns `None` if the target is out of range, which is the one way this encoding
/// fails, and fails silently if you let it.
pub fn call_rel32(at: usize, target: usize) -> Option<[u8; 5]> {
    let next = at.checked_add(5)?;
    let delta = (target as i64) - (next as i64);
    let rel: i32 = delta.try_into().ok()?;
    let b = rel.to_le_bytes();
    Some([0xE8, b[0], b[1], b[2], b[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_within_reach_of_a_real_address() {
        let target = win::exe_base();
        let cave = Cave::near(target, 256).expect("no cave in range");
        assert!(in_reach(target, cave.addr()), "cave is out of rel32 range");
        assert!(call_rel32(target, cave.addr()).is_some());
    }

    #[test]
    fn a_call_encodes_to_the_right_displacement() {
        // call at 0x1000 to 0x1010: next insn is 0x1005, so rel32 = 0x0B.
        assert_eq!(call_rel32(0x1000, 0x1010).unwrap(), [0xE8, 0x0B, 0, 0, 0]);
        // backwards
        let b = call_rel32(0x2000, 0x1000).unwrap();
        assert_eq!(b[0], 0xE8);
        assert_eq!(i32::from_le_bytes([b[1], b[2], b[3], b[4]]), -0x1005);
    }

    #[test]
    fn an_unreachable_target_is_refused_rather_than_wrapped() {
        assert!(call_rel32(0x1000, 0x1000 + 0x9000_0000).is_none());
    }

    #[test]
    fn writes_are_bounded_by_the_allocation() {
        let mut cave = Cave::near(win::exe_base(), 64).expect("cave");
        assert!(cave.write(&[0x90; 32]).is_some());
        assert!(cave.write(&[0x90; 4096]).is_none(), "overran the allocation");
    }
}
