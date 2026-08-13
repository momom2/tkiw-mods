//! The slice of Win32 the mod needs, declared by hand.
//!
//! Small enough that a binding crate would cost more than it saves, and this
//! keeps the build dependency-free and offline.

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Hmodule = Handle;

pub const DLL_PROCESS_ATTACH: u32 = 1;
pub const DLL_PROCESS_DETACH: u32 = 0;

extern "system" {
    pub fn LoadLibraryW(name: *const u16) -> Hmodule;
    pub fn GetProcAddress(module: Hmodule, name: *const u8) -> *mut c_void;
    pub fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    pub fn GetModuleHandleW(name: *const u16) -> Hmodule;
    pub fn DisableThreadLibraryCalls(module: Hmodule) -> i32;
    pub fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    pub fn CloseHandle(h: Handle) -> i32;
    pub fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetModuleFileNameW(module: Hmodule, buf: *mut u16, size: u32) -> u32;
    pub fn VirtualQuery(addr: *const c_void, buf: *mut MemoryBasicInformation, len: usize) -> usize;
    pub fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    pub fn GetCurrentThreadId() -> u32;
}

// Keyboard state lives in user32, which is not linked by default. The game
// already imports user32, so this adds no new dependency to the process.
#[link(name = "user32")]
extern "system" {
    pub fn GetAsyncKeyState(vk: i32) -> i16;
}

pub const PAGE_READWRITE: u32 = 0x04;

// Virtual-key codes for the toggle chord.
pub const VK_CONTROL: i32 = 0x11;
pub const VK_MENU: i32 = 0x12; // Alt
pub const VK_P: i32 = 0x50;

/// Whether a key is down right now.
pub fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

/// Run `f` with `len` bytes at `addr` temporarily writable, restoring the
/// original protection afterwards even if `f` decides not to write anything.
pub fn with_writable<R>(addr: usize, len: usize, f: impl FnOnce() -> R) -> Option<R> {
    let mut old = 0u32;
    let ok = unsafe {
        VirtualProtect(addr as *mut c_void, len, PAGE_READWRITE, &mut old)
    };
    if ok == 0 {
        return None;
    }
    let result = f();
    let mut back = 0u32;
    unsafe {
        VirtualProtect(addr as *mut c_void, len, old, &mut back);
    }
    Some(result)
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct MemoryBasicInformation {
    pub base_address: *mut c_void,
    pub allocation_base: *mut c_void,
    pub allocation_protect: u32,
    pub partition_id: u16,
    _pad: u16,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub mem_type: u32,
    _pad2: u32,
}

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_FREE: u32 = 0x10000;
pub const PAGE_GUARD: u32 = 0x100;
const READABLE: u32 = 0x02  /* READONLY */
    | 0x04  /* READWRITE */
    | 0x08  /* WRITECOPY */
    | 0x20  /* EXECUTE_READ */
    | 0x40  /* EXECUTE_READWRITE */
    | 0x80  /* EXECUTE_WRITECOPY */;

pub fn query(addr: usize) -> Option<MemoryBasicInformation> {
    let mut mbi = MemoryBasicInformation::default();
    let n = unsafe {
        VirtualQuery(
            addr as *const c_void,
            &mut mbi,
            core::mem::size_of::<MemoryBasicInformation>(),
        )
    };
    if n == 0 {
        None
    } else {
        Some(mbi)
    }
}

/// Validated-region cache, flushed at every poll.
///
/// `VirtualQuery` is a kernel transition, and checking it per read made the
/// game unplayable: a single survey pass did tens of thousands of them. Reads
/// cluster into a handful of regions, so remembering them collapses that to a
/// few calls.
///
/// This is only sound because the mod runs on the **game's own thread**: the
/// game cannot unmap or reprotect anything while our poll is executing. Across
/// polls it very much can, so the cache is cleared at the start of each one
/// rather than being allowed to persist.
mod region_cache {
    use core::cell::RefCell;

    const SLOTS: usize = 96;

    thread_local! {
        static CACHE: RefCell<[(usize, usize); SLOTS]> = const {
            RefCell::new([(0, 0); SLOTS])
        };
        static NEXT: RefCell<usize> = const { RefCell::new(0) };
    }

    // The region the last read landed in, kept apart from the scan.
    //
    // Reads are extremely local -- walking an instance list touches the same
    // allocation over and over -- so nearly every call is answered by this one
    // comparison. Without it every read paid a linear scan of all 96 slots,
    // thousands of times a frame.
    thread_local! {
        static LAST: RefCell<(usize, usize)> = const { RefCell::new((0, 0)) };
    }

    pub fn contains(addr: usize, len: usize) -> bool {
        let end = addr.saturating_add(len);
        if LAST.with(|l| {
            let (lo, hi) = *l.borrow();
            lo != hi && addr >= lo && end <= hi
        }) {
            return true;
        }
        let hit = CACHE.with(|c| {
            c.borrow()
                .iter()
                .find(|&&(lo, hi)| lo != hi && addr >= lo && end <= hi)
                .copied()
        });
        match hit {
            Some(r) => {
                // promote, so a run of reads through this region is answered by
                // the fast path from here on
                LAST.with(|l| *l.borrow_mut() = r);
                true
            }
            None => false,
        }
    }

    pub fn insert(lo: usize, hi: usize) {
        if hi <= lo {
            return;
        }
        LAST.with(|l| *l.borrow_mut() = (lo, hi));
        CACHE.with(|c| {
            NEXT.with(|n| {
                let mut n = n.borrow_mut();
                c.borrow_mut()[*n % SLOTS] = (lo, hi);
                *n = n.wrapping_add(1);
            })
        });
    }

    pub fn clear() {
        CACHE.with(|c| *c.borrow_mut() = [(0, 0); SLOTS]);
        LAST.with(|l| *l.borrow_mut() = (0, 0));
    }
}

/// Drop the validated-region cache. Call once per poll, before any reads.
pub fn flush_region_cache() {
    region_cache::clear();
}

/// Whether `len` bytes at `addr` are committed and readable.
///
/// Every address the mod derives from the on-disk image is checked through
/// this before it is dereferenced. The arithmetic being right is not the same
/// as the page being there, and a wrong guess is an access violation, which
/// takes the player's game down with it.
pub fn readable(addr: usize, len: usize) -> bool {
    if addr == 0 {
        return false;
    }
    if region_cache::contains(addr, len) {
        return true;
    }
    let mut covered = 0usize;
    let mut probe = addr;
    while covered < len {
        let Some(mbi) = query(probe) else {
            return false;
        };
        if mbi.state != MEM_COMMIT
            || mbi.protect & PAGE_GUARD != 0
            || mbi.protect & READABLE == 0
        {
            return false;
        }
        let base = mbi.base_address as usize;
        let end = base + mbi.region_size;
        if end <= probe {
            return false;
        }
        // remember the whole region, not just the bytes asked for: the next
        // read is almost always a few bytes further into the same allocation
        region_cache::insert(base, end);
        covered += end - probe;
        probe = end;
    }
    true
}

/// Full path of the running executable.
pub fn exe_path() -> Option<String> {
    let mut buf = [0u16; 32768];
    let n = unsafe { GetModuleFileNameW(core::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    Some(from_wide(&buf[..n as usize]))
}

/// A NUL-terminated UTF-16 copy of `s`, for the `*W` entry points.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// `C:\Windows\System32` (or wherever this install puts it).
pub fn system_directory() -> Option<String> {
    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    Some(from_wide(&buf[..n as usize]))
}

pub fn temp_directory() -> Option<String> {
    let mut buf = [0u16; 260];
    let n = unsafe { GetTempPathW(buf.len() as u32, buf.as_mut_ptr()) };
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    Some(from_wide(&buf[..n as usize]))
}

/// The base address of the running executable, for turning the RVAs the
/// offline analysis produces into live addresses.
pub fn exe_base() -> usize {
    unsafe { GetModuleHandleW(core::ptr::null()) as usize }
}
