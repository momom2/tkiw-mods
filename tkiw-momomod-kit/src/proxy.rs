//! `mfreadwrite.dll` proxy: how the kit gets loaded.
//!
//! The game statically imports `mfreadwrite.dll`, which is **not** a KnownDLL, so
//! a copy in the application directory wins the loader search and is mapped
//! before the game's entry point runs. That is the whole injection mechanism: no
//! executable is modified, nothing is renamed, and uninstalling is deleting one
//! file. In exchange we owe the process a working `mfreadwrite.dll`, so every
//! export is forwarded to the real one in System32.
//!
//! ## Why this slot
//!
//! `version.dll` is the better-documented slot and the one the reward auto-picker
//! uses, which is exactly why the kit does not use it: both mods have to be
//! installable at once. When the auto-picker is absorbed as a feature the kit
//! could move onto `version.dll` -- though on the numbers below there is no
//! particular reason to bother.
//!
//! Candidates were ranked on **export coverage**, not on how many exports there
//! are to forward. A proxy can only provide exports it can *name*, and an import
//! it cannot satisfy does not degrade gracefully -- the importing DLL fails to
//! load outright, with nothing pointing at us as the cause. Several of the game's
//! imports export most of their table by ordinal only:
//!
//! | candidate | game imports | named exports | ordinal-only (unprovidable) |
//! |---|---|---|---|
//! | **`mfreadwrite.dll`** | **1** | **7** | **0** |
//! | `version.dll` | 3 | 17 | 0 |
//! | `d3d11.dll` | 1 | 51 | 0 |
//! | `winmm.dll` | 6 | 180 | 1 |
//! | `dbghelp.dll` | 1 | 252 | 16 |
//! | `dwmapi.dll` | 3 | 44 | 75 |
//!
//! `dwmapi` was the first choice, on the grounds that 44 exports was the smallest
//! number to forward. That was the wrong metric: three quarters of its export
//! table is ordinal-only and cannot be proxied at all. `mfreadwrite` wins on both
//! metrics at once -- and `d3d11`, which also has full coverage, is ruled out
//! because the Steam overlay hooks it, and a proxy in that slot invites a
//! confusing failure inside someone else's code.
//!
//! The game imports one function, `MFCreateSourceReaderFromMediaSource`, for
//! video playback. `DllGetClassObject` and `DllCanUnloadNow` are COM entry points
//! and are forwarded like the rest, so anything that activates a class from this
//! DLL still reaches the real one.
//!
//! Media Foundation is absent on Windows N/KN editions without the Media Feature
//! Pack -- but the game statically imports it, so on such a machine the game does
//! not launch with or without us. This adds no failure mode.
//!
//! ## On the uniform signature
//!
//! Each forwarder is declared with ten `usize` parameters regardless of the real
//! arity, and passes all ten on. The x64 ABI puts the first four in registers and
//! the rest on the caller's stack, so for a three-argument call the last seven are
//! garbage read from the caller's frame -- mapped stack, harmless to read -- which
//! the real function then ignores because its own arity is lower. One trampoline
//! instead of a signature per export, and no signature to get subtly wrong.
//!
//! **Integers and pointers only.** A float or double argument travels in XMM
//! registers, which a trampoline declared in terms of `usize` is free to clobber.
//! No export here takes one; check before reusing this pattern elsewhere.
//!
//! Regenerate the list below with `tools/gen_proxy.py mfreadwrite.dll` rather than
//! editing it by hand.

#![allow(non_snake_case)]

use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, Ordering};

use tkiw_runtime::win::{self, Hmodule};

/// The DLL we impersonate, and therefore the one we must forward to.
pub const REAL_NAME: &str = "mfreadwrite.dll";

const N: usize = 7;
static REAL: [AtomicPtr<c_void>; N] = [const { AtomicPtr::new(null_mut()) }; N];
static REAL_LIB: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

/// The genuine DLL, by absolute path so we cannot find ourselves.
///
/// Deliberately lazy: `LoadLibrary` from `DllMain` risks the loader lock, and
/// nothing calls a Media Foundation export before startup finishes. [`preload`]
/// closes that window from a context where blocking is safe.
fn real_lib() -> Hmodule {
    let cached = REAL_LIB.load(Ordering::Acquire);
    if !cached.is_null() {
        return cached;
    }
    let dir = match win::system_directory() {
        Some(d) => d,
        None => return null_mut(),
    };
    let path = win::wide(&format!("{dir}\\{REAL_NAME}"));
    let lib = unsafe { win::LoadLibraryW(path.as_ptr()) };
    if !lib.is_null() {
        // benign race: another thread may have loaded it too, and LoadLibrary
        // refcounts, so whichever handle wins is equally valid
        REAL_LIB.store(lib, Ordering::Release);
    }
    lib
}

/// Load the real DLL now, from the kit's startup thread where blocking is safe,
/// so the lazy path above is only ever a fallback.
pub fn preload() -> bool {
    !real_lib().is_null()
}

type Fwd = unsafe extern "system" fn(
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
) -> usize;

/// What to return when we cannot forward at all.
///
/// **Every export of this DLL returns an `HRESULT`, where zero means _success_.**
/// So the obvious `return 0` is the worst possible answer: it tells the caller its
/// call succeeded, and the caller then uses an output pointer that was never
/// written -- a null `IMFSourceReader`, followed by a crash or a wait that never
/// ends.
///
/// This is inherited from the auto-picker's `version.dll` proxy, where returning 0
/// was *correct* because those exports return `BOOL`/`DWORD` and zero means
/// failure. Copying the pattern into a COM DLL inverted the sentinel. The lesson
/// generalises: a proxy's failure return is a property of the DLL being proxied, and
/// has to be chosen per DLL rather than carried over.
const E_FAIL: usize = 0x8000_4005;

#[inline]
unsafe fn forward(idx: usize, name: *const u8, a: [usize; 10]) -> usize {
    let mut p = REAL[idx].load(Ordering::Acquire);
    if p.is_null() {
        let lib = real_lib();
        if lib.is_null() {
            return E_FAIL;
        }
        p = win::GetProcAddress(lib, name);
        if p.is_null() {
            return E_FAIL;
        }
        REAL[idx].store(p, Ordering::Release);
    }
    let f: Fwd = core::mem::transmute(p);
    f(a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8], a[9])
}

macro_rules! forwarders {
    ($($idx:literal => $name:ident),* $(,)?) => {
        $(
            #[no_mangle]
            pub unsafe extern "system" fn $name(
                a1: usize, a2: usize, a3: usize, a4: usize, a5: usize,
                a6: usize, a7: usize, a8: usize, a9: usize, a10: usize,
            ) -> usize {
                forward(
                    $idx,
                    concat!(stringify!($name), "\0").as_ptr(),
                    [a1, a2, a3, a4, a5, a6, a7, a8, a9, a10],
                )
            }
        )*

        /// Every export this proxy provides, for the install-time check and the
        /// completeness tests below.
        pub const EXPORTS: [&str; N] = [$(stringify!($name)),*];
    };
}

forwarders! {
    0 => DllCanUnloadNow,
    1 => DllGetClassObject,
    2 => MFCreateSinkWriterFromMediaSink,
    3 => MFCreateSinkWriterFromURL,
    4 => MFCreateSourceReaderFromByteStream,
    5 => MFCreateSourceReaderFromMediaSource,
    6 => MFCreateSourceReaderFromURL,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_image() -> Option<tkiw_runtime::pe::Image> {
        let dir = win::system_directory()?;
        tkiw_runtime::pe::Image::load(&format!("{dir}\\{REAL_NAME}"))
    }

    /// Every export the real DLL names, we must name too.
    ///
    /// A gap means some other module in the process fails to load: a hard failure
    /// with no diagnostic pointing at us. This is the test that catches a Windows
    /// update adding an export.
    #[test]
    fn forwards_every_named_export_of_the_real_dll() {
        let Some(img) = real_image() else {
            eprintln!("could not read the real {REAL_NAME}; skipping");
            return;
        };
        let real = img.export_names().expect("the real DLL has an export table");
        let ours: std::collections::HashSet<&str> = EXPORTS.iter().copied().collect();
        let missing: Vec<&String> = real.iter().filter(|n| !ours.contains(n.as_str())).collect();
        assert!(
            missing.is_empty(),
            "{} export(s) of the real {REAL_NAME} are not forwarded: {missing:?}",
            missing.len()
        );
    }

    /// And nothing we claim should be absent from the real one, or a caller gets a
    /// silent zero from us where it would have had a real function.
    #[test]
    fn claims_nothing_the_real_dll_does_not_have() {
        let Some(img) = real_image() else { return };
        let real: std::collections::HashSet<String> =
            img.export_names().unwrap_or_default().into_iter().collect();
        let extra: Vec<&&str> = EXPORTS.iter().filter(|n| !real.contains(**n)).collect();
        assert!(extra.is_empty(), "we export what the real DLL does not: {extra:?}");
    }
}
