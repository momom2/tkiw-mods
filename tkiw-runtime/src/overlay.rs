//! Draw on the game's screen from any mod, without touching a detour.
//!
//! The kit proved it can draw: a [`trampoline`](crate::trampoline) into a Draw
//! GUI event, and the game's own draw builtins called from inside it with the
//! ambient state saved and restored. This packages that so a feature can draw
//! by writing a closure, and never sees a patch, a register, or a raw builtin.
//!
//! ## For a mod author
//!
//! Once, at startup, the host mod tells the overlay which Draw event to hang
//! off -- a build-specific address, found with `draw_probe` and passed as a
//! signature so a game update disables drawing rather than corrupting it:
//!
//! ```no_run
//! # use tkiw_runtime::overlay;
//! // obj_display_manager's Draw GUI End: runs last in the GUI phase, so an
//! // overlay drawn here sits on top of the game's UI.
//! overlay::set_host(0x1257860, &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10]);
//! ```
//!
//! Then any feature draws by registering a painter and holding the handle for
//! as long as it wants to draw:
//!
//! ```no_run
//! # use tkiw_runtime::{overlay, overlay::Colour, Runtime};
//! # fn demo(rt: &Runtime) -> Result<(), String> {
//! let handle = overlay::paint(rt, |c| {
//!     let (w, h) = c.gui_size();
//!     c.rectangle(w / 2.0 - 12.0, h / 2.0 - 12.0, w / 2.0 + 12.0, h / 2.0 + 12.0, Colour::BLACK);
//! })?;
//! // drawing stops the moment `handle` is dropped; the detour is removed when
//! // the last painter goes.
//! # let _ = handle; Ok(())
//! # }
//! ```
//!
//! ## Guarantees
//!
//! * **Nothing is patched until something draws.** The detour is installed on
//!   the first painter and reverted when the last is dropped, so a kit with no
//!   overlay feature active changes nothing about the game.
//! * **One misbehaving painter cannot take the others down.** Each runs inside
//!   a panic boundary; one that panics is dropped, with a line in the log, and
//!   the rest carry on.
//! * **The game's own drawing is unaffected.** The colour and alpha are saved
//!   before the painters run and restored after, so the game continues with the
//!   state it had.
//!
//! ## What it does not do yet
//!
//! Shapes and colours only. Text needs a GML string handed to `draw_text`,
//! which means constructing one the runtime will accept -- a separate, riskier
//! piece, deferred until it can be proved on screen. The API is additive, so it
//! lands here without disturbing what shapes callers already wrote.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::builtin::{self, raw_of};
use crate::rvalue::Value;
use crate::trampoline::Trampoline;
use crate::{hook, logln, win, Runtime};

/// A colour, as GameMaker holds one: a 24-bit **BGR** integer. Use the helpers
/// rather than remembering the byte order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Colour(pub u32);

impl Colour {
    pub const BLACK: Colour = Colour(0x00_00_00);
    pub const WHITE: Colour = Colour(0xFF_FF_FF);
    pub const RED: Colour = Colour(0x00_00_FF);
    pub const GREEN: Colour = Colour(0x00_FF_00);
    pub const BLUE: Colour = Colour(0xFF_00_00);

    /// From the usual red/green/blue, in that order. Stored BGR, as the game
    /// wants, so callers never handle the swap.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Colour {
        Colour((b as u32) << 16 | (g as u32) << 8 | r as u32)
    }

    fn as_f64(self) -> f64 {
        self.0 as f64
    }
}

/// A drawing surface, handed to a painter each frame. Coordinates are GUI
/// pixels; the origin is the top-left, matching the game's own GUI layer.
///
/// Every method calls the game's own draw builtin, so what a painter draws is
/// indistinguishable from what the game draws.
pub struct Canvas {
    base: usize,
    text: (usize, usize),
}

impl Canvas {
    fn call(&self, name: &str, args: &[builtin::RValueRaw]) -> Option<Value> {
        // SAFETY: a Canvas exists only inside the draw detour, on the game
        // thread, mid-frame -- the one place these are safe to call.
        unsafe { builtin::call_by_name(self.base, self.text, name, args) }
    }

    fn set_colour(&self, c: Colour) {
        self.call("draw_set_colour", &[raw_of(&Value::Real(c.as_f64()))]);
    }

    /// The size of the GUI surface, `(width, height)` in pixels.
    pub fn gui_size(&self) -> (f64, f64) {
        let n = |name: &str| match self.call(name, &[]) {
            Some(Value::Real(v)) => v,
            Some(Value::Int(i)) => i as f64,
            _ => 0.0,
        };
        (n("display_get_gui_width"), n("display_get_gui_height"))
    }

    fn num(&self, name: &str, args: &[builtin::RValueRaw]) -> f64 {
        match self.call(name, args) {
            Some(Value::Real(v)) => v,
            Some(Value::Int(i)) => i as f64,
            _ => 0.0,
        }
    }

    /// Draw a string at `(x, y)` in the current font, top-left aligned.
    ///
    /// The string reaches the game as a GML string built by [`gml_string`]; the
    /// current font and alignment are whatever the game last set, which for a
    /// GUI overlay is a sensible default. Set a font with [`Canvas::set_font`].
    pub fn text(&self, x: f64, y: f64, s: &str, colour: Colour) {
        self.set_colour(colour);
        let string = builtin::RValueRaw { payload: gml_string(s), flags: 0, kind: 1 };
        self.call(
            "draw_text",
            &[raw_of(&Value::Real(x)), raw_of(&Value::Real(y)), string],
        );
    }

    /// The pixel size `(width, height)` a string would occupy in the current
    /// font -- for laying a panel out around it before drawing.
    pub fn measure(&self, s: &str) -> (f64, f64) {
        let string = builtin::RValueRaw { payload: gml_string(s), flags: 0, kind: 1 };
        (self.num("string_width", &[string]), self.num("string_height", &[string]))
    }

    /// Set the font for subsequent [`text`](Canvas::text) by its asset id (see
    /// the font census in `draw_probe`). The game restores its own font after
    /// the overlay's painters run, so this does not disturb its drawing.
    pub fn set_font(&self, font_asset: i64) {
        self.call("draw_set_font", &[raw_of(&Value::Real(font_asset as f64))]);
    }

    /// A filled rectangle.
    pub fn rectangle(&self, x1: f64, y1: f64, x2: f64, y2: f64, colour: Colour) {
        self.rect(x1, y1, x2, y2, colour, false);
    }

    /// A rectangle outline, one pixel wide.
    pub fn frame(&self, x1: f64, y1: f64, x2: f64, y2: f64, colour: Colour) {
        self.rect(x1, y1, x2, y2, colour, true);
    }

    fn rect(&self, x1: f64, y1: f64, x2: f64, y2: f64, colour: Colour, outline: bool) {
        self.set_colour(colour);
        self.call(
            "draw_rectangle",
            &[
                raw_of(&Value::Real(x1)),
                raw_of(&Value::Real(y1)),
                raw_of(&Value::Real(x2)),
                raw_of(&Value::Real(y2)),
                raw_of(&Value::Bool(outline)),
            ],
        );
    }

    /// A line one pixel wide.
    pub fn line(&self, x1: f64, y1: f64, x2: f64, y2: f64, colour: Colour) {
        self.set_colour(colour);
        self.call(
            "draw_line",
            &[
                raw_of(&Value::Real(x1)),
                raw_of(&Value::Real(y1)),
                raw_of(&Value::Real(x2)),
                raw_of(&Value::Real(y2)),
            ],
        );
    }

    /// A line of a given width.
    pub fn line_width(&self, x1: f64, y1: f64, x2: f64, y2: f64, width: f64, colour: Colour) {
        self.set_colour(colour);
        self.call(
            "draw_line_width",
            &[
                raw_of(&Value::Real(x1)),
                raw_of(&Value::Real(y1)),
                raw_of(&Value::Real(x2)),
                raw_of(&Value::Real(y2)),
                raw_of(&Value::Real(width)),
            ],
        );
    }

    /// A circle, filled or outline.
    pub fn circle(&self, x: f64, y: f64, radius: f64, colour: Colour, filled: bool) {
        self.set_colour(colour);
        self.call(
            "draw_circle",
            &[
                raw_of(&Value::Real(x)),
                raw_of(&Value::Real(y)),
                raw_of(&Value::Real(radius)),
                raw_of(&Value::Bool(!filled)),
            ],
        );
    }
}

/// Build (once, cached) a GML string the game can read, returning the descriptor
/// pointer a kind-1 RValue carries as its payload.
///
/// A GML string RValue does not point at the characters -- it points at a
/// `{char* data, u32 refcount, u32 size}` descriptor. We leak the bytes and a
/// descriptor, with a refcount so large the runtime's RefString bookkeeping can
/// never reach the free path and try to release memory it did not allocate. The
/// result is cached by content, so drawing the same text every frame allocates
/// nothing after the first time and the leak is bounded by the number of
/// distinct strings ever drawn.
fn gml_string(s: &str) -> u64 {
    static CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&p) = c.get(s) {
        return p;
    }
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0); // GML strings are null-terminated as well as length-counted
    let data = Box::leak(bytes.into_boxed_slice()).as_ptr() as u64;
    let desc: &mut [u8; 16] = Box::leak(Box::new([0u8; 16]));
    desc[0..8].copy_from_slice(&data.to_le_bytes());
    desc[8..12].copy_from_slice(&0x4000_0000u32.to_le_bytes()); // refcount, never near zero
    desc[12..16].copy_from_slice(&(s.len() as u32).to_le_bytes()); // size (high bit, a flag, left clear)
    let ptr = desc.as_ptr() as u64;
    c.insert(s.to_string(), ptr);
    ptr
}

/// Verify GML-string construction against the running game: build a string and
/// measure it with the game's own `string_width`/`string_height`. A sensible
/// non-zero size back means the runtime accepted the constructed string -- the
/// one risky thing about drawing text -- so this is the self-test a startup
/// check runs before trusting [`Canvas::text`].
///
/// # Safety
/// Game thread only.
pub unsafe fn measure_test(base: usize, text: (usize, usize), s: &str) -> Option<(f64, f64)> {
    let string = builtin::RValueRaw { payload: gml_string(s), flags: 0, kind: 1 };
    let num = |v: Option<Value>| match v {
        Some(Value::Real(x)) => Some(x),
        Some(Value::Int(i)) => Some(i as f64),
        _ => None,
    };
    let w = num(builtin::call_by_name(base, text, "string_width", &[string]))?;
    let h = num(builtin::call_by_name(base, text, "string_height", &[string]))?;
    Some((w, h))
}

/// A registered painter. Drawing continues while this is held; dropping it stops
/// the painter, and dropping the last one reverts the detour.
#[must_use = "drawing stops when the PaintHandle is dropped"]
pub struct PaintHandle(u64);

impl Drop for PaintHandle {
    fn drop(&mut self) {
        let mut painters = painters().lock().unwrap_or_else(|e| e.into_inner());
        painters.retain(|p| p.id != self.0);
        if painters.is_empty() {
            drop(painters);
            // No painters left: take the patch out. Safe here only from the
            // game thread; a feature's deactivate (the usual dropper) is on it.
            revert_detour();
        }
    }
}

struct Painter {
    id: u64,
    draw: Box<dyn FnMut(&Canvas) + Send>,
}

fn painters() -> &'static Mutex<Vec<Painter>> {
    static P: OnceLock<Mutex<Vec<Painter>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Vec::new()))
}

/// The host Draw event, set once by the mod that owns the drawing. Absolute
/// address (base + rva) and the exact bytes there, checked before patching.
static HOST: Mutex<Option<(usize, &'static [u8])>> = Mutex::new(None);
static DETOUR: Mutex<Option<Trampoline>> = Mutex::new(None);
static BASE: AtomicUsize = AtomicUsize::new(0);
static TEXT_LO: AtomicUsize = AtomicUsize::new(0);
static TEXT_HI: AtomicUsize = AtomicUsize::new(0);
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// Name the Draw event the overlay hangs off: `site` is `base + rva`, `expected`
/// the bytes currently there (a whole, position-independent instruction
/// sequence of at least five bytes -- see [`Trampoline`]).
///
/// Call once at startup. Nothing is patched here; the detour goes in only when
/// the first painter is registered.
pub fn set_host(site: usize, expected: &'static [u8]) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some((site, expected));
}

/// Register a painter. It is called once per frame, inside the host Draw event,
/// with a [`Canvas`] to draw on. Returns a handle; drawing stops when it drops.
///
/// The first painter installs the detour, so this must be called where patching
/// is safe: on the game's thread (a feature's `activate`), or before the game's
/// entry point. Fails if no host has been set, or if the detour cannot install.
pub fn paint(
    rt: &Runtime,
    draw: impl FnMut(&Canvas) + Send + 'static,
) -> Result<PaintHandle, String> {
    BASE.store(rt.base, Ordering::Relaxed);
    TEXT_LO.store(rt.text.0, Ordering::Relaxed);
    TEXT_HI.store(rt.text.1, Ordering::Relaxed);

    ensure_detour()?;

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) as u64;
    painters()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Painter { id, draw: Box::new(draw) });
    Ok(PaintHandle(id))
}

/// Install the detour if it is not already in. Idempotent.
fn ensure_detour() -> Result<(), String> {
    let mut detour = DETOUR.lock().unwrap_or_else(|e| e.into_inner());
    if detour.is_some() {
        return Ok(());
    }
    let (site, expected) = HOST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .ok_or("overlay: no host Draw event set; call overlay::set_host first")?;

    // Same window rule as any code patch: before the game runs, or on its own
    // thread inside the pump, where the Draw event provably is not executing.
    let at_startup = hook::frames() == 0;
    let this = unsafe { win::GetCurrentThreadId() } as u64;
    let on_game_thread = hook::game_thread() != 0 && hook::game_thread() == this;
    if !at_startup && !on_game_thread {
        return Err("overlay: refusing to install the draw detour off the game's thread".into());
    }

    // SAFETY: the window was established above; `expected` is the caller's
    // vouched-for position-independent prologue.
    let t = unsafe { Trampoline::install(site, expected, on_draw) }?;
    *detour = Some(t);
    logln!("overlay: draw detour installed at {site:#x}");
    Ok(())
}

fn revert_detour() {
    let mut detour = DETOUR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut t) = detour.take() {
        // SAFETY: reached from PaintHandle::drop, which features call from the
        // game thread (deactivate) or process detach.
        match unsafe { t.revert() } {
            Ok(()) => logln!("overlay: draw detour removed (no painters left)"),
            Err(e) => logln!("overlay: WARNING: could not remove the draw detour: {e}"),
        }
    }
}

/// The single detour callback. Builds a [`Canvas`], runs every painter inside a
/// panic boundary, and leaves the ambient draw state as it found it.
extern "system" fn on_draw() {
    let _ = std::panic::catch_unwind(|| {
        let base = BASE.load(Ordering::Relaxed);
        if base == 0 {
            return;
        }
        let canvas = Canvas { base, text: (TEXT_LO.load(Ordering::Relaxed), TEXT_HI.load(Ordering::Relaxed)) };

        // Save the ambient colour and alpha once, restore once, so the game's
        // own drawing after this event is untouched. Set alpha to opaque for
        // the painters.
        let old_col = canvas.call("draw_get_colour", &[]);
        let old_alpha = canvas.call("draw_get_alpha", &[]);
        canvas.call("draw_set_alpha", &[raw_of(&Value::Real(1.0))]);

        {
            let mut painters = painters().lock().unwrap_or_else(|e| e.into_inner());
            for p in painters.iter_mut() {
                let f = &mut p.draw;
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&canvas))).is_err() {
                    p.id = 0; // mark dead; swept below
                    logln!("overlay: a painter panicked and was removed");
                }
            }
            painters.retain(|p| p.id != 0);
        }

        if let Some(c) = old_col {
            canvas.call("draw_set_colour", &[raw_of(&c)]);
        }
        if let Some(a) = old_alpha {
            canvas.call("draw_set_alpha", &[raw_of(&a)]);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_packs_to_bgr() {
        // red only -> low byte
        assert_eq!(Colour::rgb(0xFF, 0, 0).0, 0x0000FF);
        // green -> middle
        assert_eq!(Colour::rgb(0, 0xFF, 0).0, 0x00FF00);
        // blue -> high
        assert_eq!(Colour::rgb(0, 0, 0xFF).0, 0xFF0000);
        assert_eq!(Colour::rgb(0, 0, 0xFF), Colour::BLUE);
        assert_eq!(Colour::WHITE.0, 0xFFFFFF);
    }

    /// A handle dropped with no host set must not panic, and must leave the
    /// registry empty. (No game here, so paint itself cannot run, but the
    /// registry and drop path are exercisable.)
    #[test]
    fn dropping_a_handle_removes_its_painter() {
        painters().lock().unwrap().clear();
        painters().lock().unwrap().push(Painter { id: 42, draw: Box::new(|_| {}) });
        {
            let h = PaintHandle(42);
            assert_eq!(painters().lock().unwrap().len(), 1);
            drop(h);
        }
        assert!(painters().lock().unwrap().is_empty());
    }

    #[test]
    fn paint_without_a_host_is_refused() {
        *HOST.lock().unwrap() = None;
        assert!(ensure_detour().is_err());
    }
}
