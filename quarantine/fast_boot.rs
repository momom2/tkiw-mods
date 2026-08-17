//! Cutting the boot-time texture prefetch, which is most of the wait.
//!
//! ## What the measurement said
//!
//! Profiling a launch put **59% of the first fifteen seconds** in two functions:
//!
//! ```text
//!  self   total  function
//! 37.2%   37.2%  sub_1c9fd30
//! 21.5%   21.5%  sub_1ca3ab0
//!   ...
//! 72.2%          obj_init_Create_0        <- has all of it in its stack
//! ```
//!
//! with the stack reading `obj_init_Create_0` → `call_builtin_by_index` →
//! `texture_prefetch` → … → `sub_1c9fd30`. Neither hot function calls a single
//! Windows API, so this is not disk I/O and not the GPU: it is ten kilobytes of
//! straight-line CPU work decoding texture pages, on the game's own thread, before
//! anything is on screen. `data.win` is 530 MB and the machine's storage is NVMe, so
//! reading it was never the problem.
//!
//! ## What this does
//!
//! Turns `texture_prefetch` into a no-op for the duration of startup, then puts it
//! back once the main menu exists.
//!
//! In GameMaker `texture_prefetch` is a **hint**: it moves work earlier so it is not
//! paid later. Pages the game never asked to prefetch still load automatically the
//! first time something draws from them. So skipping it does not lose any texture --
//! it trades a long, guaranteed wait at startup for short, occasional loads later.
//!
//! ## The trade-off, stated plainly
//!
//! You may see a brief hitch the first time a new kind of thing appears on screen in
//! a session. In exchange the game reaches its menu much sooner. If that trade is not
//! the one you want, set `enabled = false` -- and if you see a hitch that never settles
//! down, that is worth reporting, because it would mean something here is loading a
//! page repeatedly rather than once.
//!
//! ## Why patching is safe *here* specifically
//!
//! A byte patch is not atomic, and a thread entering a function mid-write dies. This
//! feature has a window where that cannot happen: the kit's startup thread runs
//! **before the game's entry point**, so no game code has executed at all. The patch
//! is applied there and nowhere else, and [`FastBoot::activate`] refuses outright if
//! the game has already pumped a frame -- which would mean the window has closed.

use tkiw_runtime::{
    builtin, codecave::Cave, guard::Signature, hook, instance, logln, patch::Patch, rvalue,
    Runtime,
};

use crate::config::Section;
use crate::feature::{Cadence, Feature, Requirements};

/// `texture_prefetch`, the builtin wrapper. RVA for the 2026-08-10 build.
const TEXTURE_PREFETCH: usize = 0x1c53c30;

/// The bytes that must be there: the prologue, up to and including the point where
/// the function writes its own `-1.0` result.
const EXPECT: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, // mov [rsp+0x10], rbx
    0x57, // push rdi
    0x48, 0x83, 0xec, 0x30, // sub rsp, 0x30
    0x48, 0x8b, 0x7c, 0x24, 0x60, // mov rdi, [rsp+0x60]
    0x33, 0xdb, // xor ebx, ebx
    0x48, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0xbf, // movabs rax, -1.0
    0x89, 0x59, 0x0c, // mov [rcx+0xc], ebx      ; kind = real
    0x48, 0x89, 0x01, // mov [rcx], rax          ; payload = -1.0
];

/// A no-op with the same observable result the real function produces on its own
/// "nothing to do" path: `result = -1.0`, kind real.
///
/// `rcx` is the result RValue -- the builtin convention is
/// `f(RValue* result, self, other, argc, args)` -- which is exactly what the two
/// instructions at the end of `EXPECT` are doing, so this is the game's own answer,
/// reached sooner.
const STUB: &[u8] = &[
    0xc7, 0x41, 0x0c, 0x00, 0x00, 0x00, 0x00, // mov dword [rcx+0xc], 0
    0x48, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0xbf, // movabs rax, -1.0
    0x48, 0x89, 0x01, // mov [rcx], rax
    0xc3, // ret
];

/// `texturegroup_get_names()` -- every texture group in the game, as an array of
/// string RValues. argc 0.
const TEXTUREGROUP_GET_NAMES: usize = 0x1c53f50;

/// `inc qword ptr [rip + disp32]`, prefixed to [`STUB`] so the stub counts how often
/// the game actually calls this function.
///
/// **This exists because the central assumption was never checked.** The feature was
/// built on "the game prefetches its texture groups during startup, and skipping that
/// is the saving". Nobody verified the game calls `texture_prefetch` at all: the
/// builtins go through a dispatcher by index, so no cross-reference over the compiled
/// GML can see such a call, and the one test that looked conclusive turned out to
/// report zero for `draw_sprite` too.
///
/// A counter in the stub settles it in a single launch. If it reads 0 at the main menu,
/// this feature has been skipping nothing, and the 3.7s it appeared to save was noise.
const COUNT: &[u8] = &[0x48, 0xff, 0x05];

/// The whole patch: count, then the no-op. 7 + 21 bytes, inside the 30 verified.
fn stub_with_counter(site: usize, counter: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(COUNT.len() + 4 + STUB.len());
    out.extend_from_slice(COUNT);
    // rip-relative displacements count from the end of the instruction
    let end = site + COUNT.len() + 4;
    let rel: i32 = (counter as i64 - end as i64).try_into().ok()?;
    out.extend_from_slice(&rel.to_le_bytes());
    out.extend_from_slice(STUB);
    (out.len() <= EXPECT.len()).then_some(out)
}

#[derive(Default)]
pub struct FastBoot {
    patch: Option<Patch>,
    /// Where the stub's call counter lives, inside [`FastBoot::cave`].
    counter: usize,
    cave: Option<Cave>,
    /// Set once the menu has been seen and the patch reverted, so we stop looking.
    done: bool,
    restore_on: RestorePoint,
    /// Groups to prefetch per tick once the menu is up. **0 by default, and for a
    /// better reason than cost.** Timed per group on a real launch:
    ///
    /// ```text
    /// __yy__0fallbacktexture      1 ms
    /// default                     0 ms     <- every sprite in the game
    /// font_chi                11002 ms
    /// font_cyr                 1621 ms
    /// font_jp                  9933 ms
    /// ```
    ///
    /// So there is nothing worth catching up. The game's art is free, and the 22.5
    /// seconds are glyph atlases for scripts the player is not reading. Loading them
    /// later is not an improvement on not loading them at all -- see the backlog for
    /// the fix that follows from this, which is to leave the unused ones alone.
    catch_up: u64,
    /// How many groups have been prefetched, and how many there are.
    next_group: usize,
    total_groups: Option<usize>,
    caught_up: bool,
}

/// When to put `texture_prefetch` back.
#[derive(Default, Clone, Copy, PartialEq)]
enum RestorePoint {
    /// As soon as the main menu exists. The default: startup is over by then, and
    /// anything the game prefetches later (entering a run, say) runs normally.
    #[default]
    MainMenu,
    /// Never during this session. Faster still, at the cost of every prefetch the
    /// game would do all session being skipped.
    Never,
}

impl Feature for FastBoot {
    fn name(&self) -> &'static str {
        "fast_boot"
    }

    fn module(&self) -> &'static str {
        "optimization"
    }

    fn summary(&self) -> &'static str {
        "Skips the texture prefetch the game does before the menu. Four launches each \
         way put it 3.7s ahead of not using it, against 19s of run-to-run noise -- so \
         treat any saving as unproven."
    }

    /// The one feature here that defaults **on**.
    ///
    /// The kit's rule is that features default off, with exceptions stated and
    /// justified. This is the exception: it is the reason a player installs a mod
    /// advertising a faster startup, it is reverted before gameplay begins, and it
    /// cannot lose data or change the state of a run.
    fn default_enabled(&self) -> bool {
        true
    }

    fn requires(&self) -> Requirements {
        Requirements {
            signatures: &[Signature {
                what: "texture_prefetch",
                rva: TEXTURE_PREFETCH,
                bytes: EXPECT,
            }],
            objects: &["obj_main_menu"],
            ..Requirements::default()
        }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        self.restore_on = match section.get("restore_on").unwrap_or("main_menu") {
            "main_menu" => RestorePoint::MainMenu,
            "never" => RestorePoint::Never,
            other => {
                return Err(format!(
                    "restore_on: expected main_menu or never, found {other:?}"
                ))
            }
        };
        self.catch_up = section.u64("catch_up", 0)?;
        if self.catch_up > 64 {
            return Err(format!("catch_up: {} is too many groups per tick", self.catch_up));
        }
        for k in section.unknown(&["enabled", "restore_on", "catch_up"]) {
            logln!("[fast_boot] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn activate(&mut self, rt: &Runtime) -> Result<(), String> {
        // The interlock. Patching is only safe before the game has executed any code,
        // and a pumped frame proves it has. Refusing is correct: a feature that cannot
        // do its job safely should not do it unsafely.
        if hook::frames() > 0 {
            return Err(
                "the game has already started running, so patching its code is no \
                 longer safe. This feature only applies at launch."
                    .into(),
            );
        }
        let addr = rt.base + TEXTURE_PREFETCH;

        // A counter inside the stub, so the log can say how many prefetches were
        // actually skipped rather than how many were assumed.
        let mut cave = Cave::near(addr, 64).ok_or("no memory within reach for the counter")?;
        let counter = cave.reserve_aligned(8, 16).ok_or("cave too small")?;
        unsafe { core::ptr::write_volatile(counter as *mut u64, 0) };
        let bytes = stub_with_counter(addr, counter).ok_or("counter out of range of the stub")?;

        // SAFETY: no game code has executed yet -- checked immediately above -- so
        // nothing can be inside `texture_prefetch` while the bytes change.
        let patch = unsafe { Patch::apply("texture_prefetch", addr, EXPECT, &bytes) }?;
        self.patch = Some(patch);
        self.counter = counter;
        self.cave = Some(cave);
        self.done = false;
        logln!(
            "[fast_boot] texture_prefetch stubbed at {addr:#x}; restoring {}",
            match self.restore_on {
                RestorePoint::MainMenu => "when the main menu appears",
                RestorePoint::Never => "never (restore_on = never)",
            }
        );
        Ok(())
    }

    fn deactivate(&mut self, _rt: &Runtime) {
        if let Some(p) = self.patch.as_mut() {
            // SAFETY: same hazard as applying. By the time anything deactivates this,
            // the stub is a four-instruction straight line with no calls, so a thread
            // inside it is between two of those instructions and returns normally.
            match unsafe { p.revert() } {
                Ok(()) => logln!("[fast_boot] texture_prefetch restored"),
                Err(e) => logln!("[fast_boot] WARNING: could not restore: {e}"),
            }
        }
        self.patch = None;
    }

    fn cadence(&self) -> Cadence {
        Cadence::Interval(std::time::Duration::from_millis(250))
    }

    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        // `done` alone is not the stopping condition any more: reverting the patch is
        // the first half, and the catch-up runs for many ticks after it. Checking it
        // here is what made the catch-up run exactly once.
        if self.restore_on == RestorePoint::Never || (self.done && self.caught_up) {
            return Ok(());
        }
        if instance::find_singleton(rt.base, "obj_main_menu").is_none() {
            return Ok(());
        }
        if !self.done {
            self.done = true;
            self.revert_at_menu();
        }
        self.catch_up_one(rt);
        Ok(())
    }
}

impl FastBoot {
    fn revert_at_menu(&mut self) {
        // The number this feature exists on. Reported whether it is flattering or not:
        // 0 means the game never called `texture_prefetch` during startup and this
        // feature skipped nothing at all.
        let calls = if self.counter == 0 {
            0
        } else {
            // SAFETY: our own cave, still mapped, written only by the stub.
            unsafe { core::ptr::read_volatile(self.counter as *const u64) }
        };
        if let Some(p) = self.patch.as_mut() {
            // SAFETY: we are on the game's thread, so the game is inside
            // PeekMessageW and demonstrably not inside the stub.
            match unsafe { p.revert() } {
                Ok(()) => logln!(
                    "[fast_boot] main menu reached - texture_prefetch restored after \
                     skipping {calls} call(s){}",
                    if calls == 0 {
                        " - the game never called it, so this feature did nothing"
                    } else {
                        ""
                    }
                ),
                Err(e) => logln!("[fast_boot] WARNING: could not restore: {e}"),
            }
        }
        self.patch = None;
    }

    /// Prefetch the next group or two, on the menu, where nobody is waiting.
    ///
    /// This is the half that makes skipping honest. Skipping alone does not remove the
    /// work, it moves it to whenever the texture is first drawn -- which is during a
    /// run, as a hitch. The menu is time the player is already spending, so the same
    /// uploads cost nothing they notice.
    ///
    /// **The names are re-read every tick rather than cached.** They come back as GML
    /// string RValues, and holding one across frames means holding a reference the
    /// runtime believes nobody holds -- the exact use-after-free that has bitten this
    /// repository once already. Read, use, drop, inside one call.
    fn catch_up_one(&mut self, rt: &Runtime) {
        if self.caught_up || self.catch_up == 0 || self.restore_on == RestorePoint::Never {
            return;
        }
        // SAFETY: on the game's thread, via the frame hook.
        let names = unsafe { self.group_names(rt) };
        let Some(count) = names else {
            self.caught_up = true;
            return;
        };
        if self.total_groups.is_none() {
            self.total_groups = Some(count);
            logln!("[fast_boot] catching up {count} texture group(s) on the menu");
        }
        for _ in 0..self.catch_up {
            if self.next_group >= count {
                self.caught_up = true;
                logln!("[fast_boot] texture catch-up done ({count} group(s))");
                return;
            }
            // Time each group on its own. The budget guard reports the whole on_frame
            // call, which is not the same thing and was not good enough to reason from.
            let t0 = std::time::Instant::now();
            // SAFETY: game thread; the RValue is used and dropped within this call.
            let named = unsafe { self.prefetch(rt, self.next_group) };
            logln!(
                "[fast_boot] group {}/{count} {:?} took {}ms",
                self.next_group + 1,
                named.as_deref().unwrap_or("?"),
                t0.elapsed().as_millis()
            );
            self.next_group += 1;
        }
    }

    /// How many texture groups there are. `None` if the call did not come back as an
    /// array, which is reason to stop rather than to guess.
    ///
    /// # Safety
    /// Game thread.
    unsafe fn group_names(&self, rt: &Runtime) -> Option<usize> {
        let f = builtin::resolve(rt.base, TEXTUREGROUP_GET_NAMES, rt.text)?;
        let mut out = builtin::RValueRaw { payload: 0, flags: 0, kind: rvalue::KIND_UNDEFINED };
        f(&mut out, core::ptr::null_mut(), core::ptr::null_mut(), 0, core::ptr::null());
        if out.kind != rvalue::KIND_ARRAY {
            return None;
        }
        let ptr = out.payload as usize;
        if !tkiw_runtime::win::readable(ptr + rvalue::ARRAY_LEN + 4, 4) {
            return None;
        }
        let len = rvalue::read_i32(ptr + rvalue::ARRAY_LEN)?;
        if !(0..4096).contains(&len) {
            return None;
        }
        Some(len as usize)
    }

    /// Prefetch group `i`, passing the runtime's own string RValue straight back to it.
    ///
    /// # Safety
    /// Game thread. The array is fetched and used without ever being stored.
    unsafe fn prefetch(&self, rt: &Runtime, i: usize) -> Option<String> {
        let (Some(names), Some(pf)) = (
            builtin::resolve(rt.base, TEXTUREGROUP_GET_NAMES, rt.text),
            builtin::resolve(rt.base, TEXTURE_PREFETCH, rt.text),
        ) else {
            return None;
        };
        let mut arr = builtin::RValueRaw { payload: 0, flags: 0, kind: rvalue::KIND_UNDEFINED };
        names(&mut arr, core::ptr::null_mut(), core::ptr::null_mut(), 0, core::ptr::null());
        if arr.kind != rvalue::KIND_ARRAY {
            return None;
        }
        let ptr = arr.payload as usize;
        let items = rvalue::read_usize(ptr + rvalue::ARRAY_DATA)?;
        let at = items + i * 16;
        if items == 0 || !tkiw_runtime::win::readable(at, 16) {
            return None;
        }
        // What the game calls this group, so the log names it rather than numbering it.
        let name = match rvalue::decode(at) {
            Some(rvalue::Value::Str(s)) => Some(s),
            _ => None,
        };
        let arg = core::ptr::read(at as *const builtin::RValueRaw);
        let mut out = builtin::RValueRaw { payload: 0, flags: 0, kind: rvalue::KIND_UNDEFINED };
        pf(&mut out, core::ptr::null_mut(), core::ptr::null_mut(), 1, &arg);
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stub must not be longer than the bytes we verified, or a patch would write
    /// over code it never looked at.
    #[test]
    fn the_stub_fits_inside_the_verified_prologue() {
        assert!(
            STUB.len() <= EXPECT.len(),
            "stub is {} bytes, only {} were verified",
            STUB.len(),
            EXPECT.len()
        );
    }

    /// `EXPECT` must be what is actually at that address in the shipped game.
    ///
    /// This is the check that catches a game update at build time rather than in a
    /// player's launch -- where it would be caught anyway, by the requirement check,
    /// but only after someone had already installed a kit that cannot do its job.
    #[test]
    fn the_prologue_matches_the_analysed_build() {
        let candidates = [
            r"..\tkiw-morale-fix\The King is Watching.exe.orig",
            r"C:\Program Files (x86)\Steam\steamapps\common\The King is Watching\The King is Watching.exe",
        ];
        let Some(img) = candidates
            .iter()
            .find_map(|p| tkiw_runtime::pe::Image::load(p))
        else {
            eprintln!("no game executable found; skipping");
            return;
        };
        let off = img
            .rva2off(TEXTURE_PREFETCH as u32)
            .expect("texture_prefetch is file-backed");
        assert_eq!(
            &img.data[off..off + EXPECT.len()],
            EXPECT,
            "texture_prefetch has moved or changed"
        );
    }

    /// The stub must produce the same observable result the real function does.
    ///
    /// Both write `-1.0` into the result RValue and set its kind to real. The two
    /// instruction encodings are compared byte for byte, at the offsets each occupies
    /// in its own sequence, so a change to either constant fails here rather than
    /// silently handing the game a different answer than it expects.
    #[test]
    fn the_stub_writes_the_same_result_the_real_function_does() {
        // In EXPECT: `movabs rax, -1.0` then, after the kind store, `mov [rcx], rax`.
        const MOVABS: &[u8] = &[0x48, 0xb8, 0, 0, 0, 0, 0, 0, 0xf0, 0xbf];
        const STORE: &[u8] = &[0x48, 0x89, 0x01];

        assert!(EXPECT.windows(MOVABS.len()).any(|w| w == MOVABS), "EXPECT lost the -1.0");
        assert!(EXPECT.ends_with(STORE), "EXPECT should end with the result store");

        assert!(STUB.windows(MOVABS.len()).any(|w| w == MOVABS), "STUB lost the -1.0");
        assert!(
            STUB[..STUB.len() - 1].ends_with(STORE),
            "STUB should store the result just before returning"
        );
        // kind = real (0), written as an immediate rather than via ebx.
        assert_eq!(&STUB[..7], &[0xc7, 0x41, 0x0c, 0, 0, 0, 0], "kind store changed");
        assert_eq!(*STUB.last().unwrap(), 0xc3, "the stub must end in a ret");
    }
}
