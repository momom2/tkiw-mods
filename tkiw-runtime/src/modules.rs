//! Every module in the process, and the unwind data each one carries.
//!
//! A profiler that can only unwind the game's own image stops at the first frame
//! belonging to Windows -- and since the game spends much of a launch inside file I/O
//! and the graphics driver, that means most samples reduce to "outside the game",
//! which names nothing and blames nobody.
//!
//! This fixes that. Every loaded module's exception directory is found once, and
//! unwind entries are looked up from it, so a stack can be walked *through* `ntdll`
//! and `d3d11` back into the game code that was waiting.
//!
//! ## Read from memory, not from disk
//!
//! The obvious approach is to load each module's file and parse it. Reading the
//! **mapped image** instead is both simpler and more correct:
//!
//! * `RtlVirtualUnwind` wants a pointer to a live `RUNTIME_FUNCTION`, which is what
//!   the mapped `.pdata` already is -- nothing to copy or relocate.
//! * No file I/O, so a snapshot costs microseconds and can be taken on any thread.
//! * No chance of parsing a file that does not match what is loaded.
//!
//! Everything here reads only our own process's mapped, read-only pages, so it is
//! safe to call **with another thread suspended** -- which the sampler relies on. It
//! takes no locks and allocates nothing on the lookup path, for the same reason.

use crate::win::{self, Handle, Hmodule};

/// One loaded module.
pub struct Mod {
    pub base: usize,
    pub size: usize,
    /// File name only, lowercased: `"ntdll.dll"`.
    pub name: String,
    /// Live address of the `RUNTIME_FUNCTION` array, and how many entries it has.
    /// Zero-length for a module with no exception directory.
    pdata: usize,
    entries: usize,
}

impl Mod {
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.base + self.size
    }
}

/// Every module, sorted by base address.
pub struct Modules {
    mods: Vec<Mod>,
}

const SIZE_OF_RUNTIME_FUNCTION: usize = 12;

#[repr(C)]
struct ModuleInfo {
    base: usize,
    size: u32,
    _pad: u32,
    entry: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn K32EnumProcessModules(
        process: Handle,
        modules: *mut Hmodule,
        cb: u32,
        needed: *mut u32,
    ) -> i32;
    fn K32GetModuleInformation(
        process: Handle,
        module: Hmodule,
        info: *mut ModuleInfo,
        cb: u32,
    ) -> i32;
    fn K32GetModuleBaseNameW(process: Handle, module: Hmodule, buf: *mut u16, size: u32) -> u32;
}

/// Read a `T` from our own mapped memory, bounds-checked against a module.
///
/// No `win::readable` here: that is backed by a shared cache, and the sampler calls
/// into this with the game's thread suspended, where waiting on a lock would hang the
/// process. Soundness comes instead from every address being inside a module's mapped
/// image, which the snapshot has already established.
#[inline]
unsafe fn peek<T: Copy>(addr: usize) -> T {
    core::ptr::read_unaligned(addr as *const T)
}

impl Modules {
    /// Enumerate the process's modules. Cheap: no file I/O, a few hundred
    /// microseconds.
    pub fn snapshot() -> Modules {
        let process = unsafe { GetCurrentProcess() };
        let mut handles = [core::ptr::null_mut::<core::ffi::c_void>(); 512];
        let mut needed: u32 = 0;
        let ok = unsafe {
            K32EnumProcessModules(
                process,
                handles.as_mut_ptr(),
                (handles.len() * 8) as u32,
                &mut needed,
            )
        };
        let mut mods = Vec::new();
        if ok == 0 {
            return Modules { mods };
        }
        let count = (needed as usize / 8).min(handles.len());
        for &h in handles.iter().take(count) {
            if h.is_null() {
                continue;
            }
            let mut info = ModuleInfo { base: 0, size: 0, _pad: 0, entry: 0 };
            if unsafe {
                K32GetModuleInformation(process, h, &mut info, core::mem::size_of::<ModuleInfo>() as u32)
            } == 0
            {
                continue;
            }
            if info.base == 0 || info.size == 0 {
                continue;
            }
            let mut buf = [0u16; 260];
            let n = unsafe { K32GetModuleBaseNameW(process, h, buf.as_mut_ptr(), 260) } as usize;
            let name = win::from_wide(&buf[..n.min(260)]).to_ascii_lowercase();

            let (pdata, entries) = exception_directory(info.base, info.size as usize);
            mods.push(Mod { base: info.base, size: info.size as usize, name, pdata, entries });
        }
        mods.sort_unstable_by_key(|m| m.base);
        Modules { mods }
    }

    pub fn len(&self) -> usize {
        self.mods.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }

    pub fn all(&self) -> &[Mod] {
        &self.mods
    }

    /// The module containing `addr`.
    pub fn find(&self, addr: usize) -> Option<&Mod> {
        let i = match self.mods.binary_search_by(|m| m.base.cmp(&addr)) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        self.mods.get(i).filter(|m| m.contains(addr))
    }

    /// `(image base, pointer to the RUNTIME_FUNCTION)` for the function containing
    /// `addr`, or `None` if it is a leaf or lies outside every module.
    ///
    /// Safe to call with another thread suspended: pure reads of mapped pages, no
    /// allocation, no locks, and never `RtlLookupFunctionEntry` -- which consults
    /// loader state the suspended thread may be holding.
    pub fn entry_for(&self, addr: usize) -> Option<(usize, usize)> {
        let m = self.find(addr)?;
        if m.entries == 0 {
            return None;
        }
        let rva = (addr - m.base) as u32;
        // Binary search the RUNTIME_FUNCTION array in place.
        let (mut lo, mut hi) = (0usize, m.entries);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let at = m.pdata + mid * SIZE_OF_RUNTIME_FUNCTION;
            let begin: u32 = unsafe { peek(at) };
            let end: u32 = unsafe { peek(at + 4) };
            if rva < begin {
                hi = mid;
            } else if rva >= end {
                lo = mid + 1;
            } else {
                return Some((m.base, at));
            }
        }
        None
    }

    /// `"ntdll.dll+0x1a2b3"`, for a frame we have no better name for.
    pub fn describe(&self, addr: usize) -> String {
        match self.find(addr) {
            Some(m) => format!("{}+{:#x}", m.name, addr - m.base),
            None => format!("<unmapped {addr:#x}>"),
        }
    }

    /// Just the module name, for grouping samples.
    pub fn name_of(&self, addr: usize) -> String {
        match self.find(addr) {
            Some(m) => m.name.clone(),
            None => "<unmapped>".to_string(),
        }
    }
}

/// Locate a mapped module's exception directory: `(live address, entry count)`.
///
/// Parses the PE headers out of the mapped image. Every offset is checked against the
/// module's own size, so a module with unusual headers yields `(0, 0)` and is simply
/// not unwound through, rather than producing a wild pointer.
fn exception_directory(base: usize, size: usize) -> (usize, usize) {
    if size < 0x1000 {
        return (0, 0);
    }
    unsafe {
        if peek::<u16>(base) != 0x5A4D {
            return (0, 0); // not MZ
        }
        let peo = peek::<u32>(base + 0x3C) as usize;
        if peo + 0x108 >= size {
            return (0, 0);
        }
        if peek::<u32>(base + peo) != 0x0000_4550 {
            return (0, 0); // not PE\0\0
        }
        let opt = base + peo + 24;
        if peek::<u16>(opt) != 0x20B {
            return (0, 0); // not PE32+, so not x64
        }
        // Data directory 3 is the exception table.
        let dir = opt + 112 + 3 * 8;
        let rva = peek::<u32>(dir) as usize;
        let bytes = peek::<u32>(dir + 4) as usize;
        if rva == 0 || bytes < SIZE_OF_RUNTIME_FUNCTION || rva + bytes > size {
            return (0, 0);
        }
        (base + rva, bytes / SIZE_OF_RUNTIME_FUNCTION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_usual_modules_with_unwind_data() {
        let m = Modules::snapshot();
        assert!(m.len() > 3, "only {} modules found", m.len());
        let names: Vec<&str> = m.all().iter().map(|x| x.name.as_str()).collect();
        assert!(names.iter().any(|n| *n == "ntdll.dll"), "no ntdll in {names:?}");
        let ntdll = m.all().iter().find(|x| x.name == "ntdll.dll").unwrap();
        assert!(ntdll.entries > 100, "ntdll has {} unwind entries", ntdll.entries);
    }

    /// The lookup has to find an entry for a real function in a real module. This is
    /// the test that the header parsing and the binary search agree.
    #[test]
    fn finds_an_unwind_entry_for_a_known_function() {
        let m = Modules::snapshot();
        let f = super::super::sample::virtual_unwind().expect("RtlVirtualUnwind") as usize;
        let found = m.entry_for(f);
        assert!(found.is_some(), "no unwind entry for RtlVirtualUnwind itself");
        let (image_base, entry) = found.unwrap();
        let ntdll = m.all().iter().find(|x| x.name == "ntdll.dll").unwrap();
        assert_eq!(image_base, ntdll.base);
        let begin: u32 = unsafe { peek(entry) };
        let end: u32 = unsafe { peek(entry + 4) };
        let rva = (f - image_base) as u32;
        assert!(begin <= rva && rva < end, "{rva:#x} not in {begin:#x}..{end:#x}");
    }

    #[test]
    fn an_address_in_no_module_is_reported_as_unmapped() {
        let m = Modules::snapshot();
        assert!(m.find(0x1000).is_none());
        assert!(m.describe(0x1000).contains("unmapped"));
    }
}
