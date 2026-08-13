//! `version.dll` proxy.
//!
//! The game statically imports `version.dll`, which is not a KnownDLL, so a
//! copy in the application directory wins the loader search and is mapped
//! before the game's entry point runs. That is the whole injection mechanism:
//! no executable is modified, nothing is renamed, and uninstalling is deleting
//! one file.
//!
//! In exchange we owe the process a working `version.dll`, so every export is
//! forwarded to the real one in System32.
//!
//! **On the uniform signature.** Each forwarder is declared with eight `usize`
//! parameters regardless of the real arity, and passes all eight along. The
//! Windows x64 ABI puts the first four in registers and the rest on the
//! caller's stack, so for a two-argument call the last six are garbage read
//! from the caller's frame -- mapped memory, harmless to read -- which the real
//! function then ignores because its own arity is lower. This costs one
//! trampoline instead of seventeen hand-transcribed signatures, and there is no
//! signature to get subtly wrong. `VerInstallFile*` has the widest arity at
//! eight, which is what sets the number.

use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::win::{self, Hmodule};

const N: usize = 17;
static REAL: [AtomicPtr<c_void>; N] = [const { AtomicPtr::new(null_mut()) }; N];
static REAL_LIB: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

/// The genuine `version.dll`, by absolute path so we cannot find ourselves.
///
/// Deliberately lazy: `LoadLibrary` from `DllMain` risks the loader lock, and
/// nothing calls a version.dll export before startup finishes.
fn real_lib() -> Hmodule {
    let cached = REAL_LIB.load(Ordering::Acquire);
    if !cached.is_null() {
        return cached;
    }
    let dir = match win::system_directory() {
        Some(d) => d,
        None => return null_mut(),
    };
    let path = win::wide(&format!("{dir}\\version.dll"));
    let lib = unsafe { win::LoadLibraryW(path.as_ptr()) };
    if !lib.is_null() {
        // benign race: another thread may have loaded it too, and LoadLibrary
        // refcounts, so whichever handle wins is equally valid
        REAL_LIB.store(lib, Ordering::Release);
    }
    lib
}

type Fwd = unsafe extern "system" fn(
    usize, usize, usize, usize, usize, usize, usize, usize,
) -> usize;

#[inline]
unsafe fn forward(idx: usize, name: *const u8, a: [usize; 8]) -> usize {
    let mut p = REAL[idx].load(Ordering::Acquire);
    if p.is_null() {
        let lib = real_lib();
        if lib.is_null() {
            return 0;
        }
        p = win::GetProcAddress(lib, name);
        if p.is_null() {
            return 0;
        }
        REAL[idx].store(p, Ordering::Release);
    }
    let f: Fwd = core::mem::transmute(p);
    f(a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7])
}

macro_rules! forwarders {
    ($($idx:literal => $name:ident),* $(,)?) => {
        $(
            #[no_mangle]
            pub unsafe extern "system" fn $name(
                a1: usize, a2: usize, a3: usize, a4: usize,
                a5: usize, a6: usize, a7: usize, a8: usize,
            ) -> usize {
                forward(
                    $idx,
                    concat!(stringify!($name), "\0").as_ptr(),
                    [a1, a2, a3, a4, a5, a6, a7, a8],
                )
            }
        )*

        /// Every export this proxy must provide, for the install-time check.
        pub const EXPORTS: [&str; N] = [$(stringify!($name)),*];
    };
}

forwarders! {
     0 => GetFileVersionInfoA,
     1 => GetFileVersionInfoByHandle,
     2 => GetFileVersionInfoExA,
     3 => GetFileVersionInfoExW,
     4 => GetFileVersionInfoSizeA,
     5 => GetFileVersionInfoSizeExA,
     6 => GetFileVersionInfoSizeExW,
     7 => GetFileVersionInfoSizeW,
     8 => GetFileVersionInfoW,
     9 => VerFindFileA,
    10 => VerFindFileW,
    11 => VerInstallFileA,
    12 => VerInstallFileW,
    13 => VerLanguageNameA,
    14 => VerLanguageNameW,
    15 => VerQueryValueA,
    16 => VerQueryValueW,
}
