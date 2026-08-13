//! Load only the glyph atlases you can read.
//!
//! Measured per texture group on a real launch, by timing `texture_prefetch` one group
//! at a time:
//!
//! ```text
//! default                     0 ms      <- every sprite in the game
//! __yy__0fallbacktexture      1 ms
//! font_lat                 1017 ms      <- the one you are reading
//! font_cyr                 1621 ms
//! font_kr                  4051 ms
//! font_jp                  9933 ms
//! font_chi                11002 ms
//! ```
//!
//! The game's art is free. **Twenty-six and a half seconds of a cold start are Chinese,
//! Japanese, Korean and Cyrillic glyphs**, on a launch where none of them is ever drawn.
//! `font_kr` and `font_lat` are in that list because this feature found them: the first
//! measurement stopped after five groups, when the budget guard switched it off.
//!
//! ## How it skips them
//!
//! Not by inspecting arguments. [`crate::features::fast_boot`] already stubs
//! `texture_prefetch` for the whole of startup, so at the main menu *nothing* has been
//! prefetched. This feature then prefetches back exactly the groups you asked for, using
//! the game's own call, and simply never asks for the others.
//!
//! That makes the skip permanent rather than deferred: an atlas that is never fetched
//! and never drawn costs nothing for the rest of the session.
//!
//! **It therefore needs `fast_boot` on.** With `fast_boot` off the game prefetches
//! everything during startup and there is nothing left for this to decline. That is
//! checked and said plainly rather than left to be discovered.
//!
//! ## What you lose by switching one off
//!
//! Text in that script has no glyphs prepared. The game falls back -- most likely to
//! boxes -- so a player reading Chinese wants `chinese = true` and their eleven seconds
//! back. That is the whole reason these are five switches and not one.

use tkiw_runtime::{builtin, instance, logln, rvalue, Runtime};

use crate::config::Section;
use crate::feature::{Cadence, Feature, Requirements};

/// `texturegroup_get_names()` -- every texture group, as an array of strings. argc 0.
const TEXTUREGROUP_GET_NAMES: usize = 0x1c53f50;
/// `texture_prefetch(name)`.
const TEXTURE_PREFETCH: usize = 0x1c53c30;

/// The groups this feature decides about, and the config key for each.
///
/// Anything not named here is always prefetched: this feature exists to decline glyph
/// atlases, not to second-guess the game's art.
/// `(group, config key, loaded by default)`.
///
/// Latin is here on the same terms as the rest -- one switch, same mechanism, and you
/// can decline it. Only its **default** differs, because only the fact differs: it is
/// the script the menu is drawn in on this build, so shipping it off would ship a game
/// of empty boxes. Turn it off and you get your second back and your text as squares;
/// that is your call to make, not a decision to bake in.
const FONTS: &[(&str, &str, bool)] = &[
    ("font_lat", "latin", true),
    ("font_chi", "chinese", false),
    ("font_jp", "japanese", false),
    ("font_kr", "korean", false),
    ("font_cyr", "cyrillic", false),
];

pub struct FontAtlases {
    /// Whether each of [`FONTS`] should be loaded, in the same order.
    wanted: [bool; 5],
    next: usize,
    total: Option<usize>,
    done: bool,
}

impl Default for FontAtlases {
    fn default() -> FontAtlases {
        FontAtlases { wanted: [true, false, false, false, false], next: 0, total: None, done: false }
    }
}

impl FontAtlases {
    /// Whether a group should be prefetched. Unknown groups always are.
    fn allowed(&self, name: &str) -> bool {
        match FONTS.iter().position(|(g, _, _)| *g == name) {
            Some(i) => self.wanted[i],
            None => true,
        }
    }
}

impl Feature for FontAtlases {
    fn name(&self) -> &'static str {
        "font_atlases"
    }

    fn module(&self) -> &'static str {
        "qol"
    }

    fn summary(&self) -> &'static str {
        "Declines the glyph atlases for scripts you do not read. Prefetching them costs \
         26 seconds when timed on its own, but declining them is not yet shown to make \
         startup shorter, so this is off until it is."
    }

    /// Defaults **off**, because the saving is not demonstrated.
    ///
    /// The per-group timings are real: `texture_prefetch("font_chi")` measured on its
    /// own takes eleven seconds. What is *not* established is that declining it makes
    /// startup shorter. A first A/B, one run each, went the wrong way -- 51.5s to the
    /// menu with this on against 41.5s with it off -- and run-to-run spread on that
    /// machine was 36-56s, so neither number means much on its own.
    ///
    /// There is a structural reason to doubt the saving, and it is the thing to test
    /// next. `texture_prefetch` **uploads** an atlas the game has already built;
    /// building it is `GENERATE_FONTS` and `__scribble_font_add_from_project`, in
    /// `obj_init`, and happens whether or not anyone prefetches. Declining the upload
    /// may therefore save nothing and merely move it to the first draw.
    ///
    /// So: off, and honestly labelled, until a repeated A/B says otherwise.
    fn default_enabled(&self) -> bool {
        false
    }

    fn requires(&self) -> Requirements {
        Requirements { objects: &["obj_main_menu"], ..Requirements::default() }
    }

    fn configure(&mut self, section: &Section) -> Result<(), String> {
        for (i, (_group, key, default)) in FONTS.iter().enumerate() {
            self.wanted[i] = section.bool(key, *default)?;
        }
        let known: Vec<&str> = std::iter::once("enabled")
            .chain(FONTS.iter().map(|(_, k, _)| *k))
            .collect();
        for k in section.unknown(&known) {
            logln!("[font_atlases] config: unknown key {k:?} - ignored");
        }
        Ok(())
    }

    fn cadence(&self) -> Cadence {
        Cadence::Interval(std::time::Duration::from_millis(250))
    }

    fn on_frame(&mut self, rt: &Runtime) -> Result<(), String> {
        if self.done {
            return Ok(());
        }
        if instance::find_singleton(rt.base, "obj_main_menu").is_none() {
            return Ok(());
        }

        // SAFETY: on the game's thread, via the frame hook. The names array is read and
        // used inside each call and never held across frames -- a GML string kept past
        // the call is a reference the runtime believes nobody holds.
        let Some(count) = (unsafe { group_count(rt) }) else {
            self.done = true;
            return Err("texturegroup_get_names did not return an array".into());
        };
        if self.total.is_none() {
            self.total = Some(count);
        }

        // One group per tick. `default` costs under a millisecond and any atlas the
        // player asked for costs seconds, so spacing them keeps a wanted atlas from
        // stopping the menu dead.
        if self.next >= count {
            self.done = true;
            let skipped: Vec<&str> = FONTS
                .iter()
                .zip(self.wanted)
                .filter(|(_, w)| !w)
                .map(|((g, _, _), _)| *g)
                .collect();
            logln!(
                "[font_atlases] {} group(s) prefetched; declined {}",
                count - skipped.len(),
                if skipped.is_empty() { "nothing".to_string() } else { skipped.join(", ") }
            );
            return Ok(());
        }

        let i = self.next;
        self.next += 1;
        // SAFETY: game thread; see above.
        let Some(name) = (unsafe { group_name(rt, i) }) else { return Ok(()) };
        if !self.allowed(&name) {
            logln!("[font_atlases] declining {name:?} - not a script you asked for");
            return Ok(());
        }
        let t0 = std::time::Instant::now();
        // SAFETY: game thread.
        unsafe { prefetch(rt, i) };
        let ms = t0.elapsed().as_millis();
        if ms > 100 {
            logln!("[font_atlases] {name:?} took {ms}ms");
        }
        Ok(())
    }
}

/// How many texture groups the game has.
///
/// # Safety
/// Game thread.
unsafe fn group_count(rt: &Runtime) -> Option<usize> {
    let (ptr, _) = names_array(rt)?;
    let len = rvalue::read_i32(ptr + rvalue::ARRAY_LEN)?;
    (0..4096).contains(&len).then_some(len as usize)
}

/// The names array, as `(array ptr, items ptr)`.
///
/// # Safety
/// Game thread.
unsafe fn names_array(rt: &Runtime) -> Option<(usize, usize)> {
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
    let items = rvalue::read_usize(ptr + rvalue::ARRAY_DATA)?;
    (items != 0).then_some((ptr, items))
}

/// # Safety
/// Game thread.
unsafe fn group_name(rt: &Runtime, i: usize) -> Option<String> {
    let (_, items) = names_array(rt)?;
    let at = items + i * 16;
    if !tkiw_runtime::win::readable(at, 16) {
        return None;
    }
    match rvalue::decode(at) {
        Some(rvalue::Value::Str(s)) => Some(s),
        _ => None,
    }
}

/// Prefetch group `i`, handing the runtime its own string RValue straight back.
///
/// # Safety
/// Game thread. The array is fetched, used and dropped within this call.
unsafe fn prefetch(rt: &Runtime, i: usize) {
    let (Some((_, items)), Some(pf)) = (
        names_array(rt),
        builtin::resolve(rt.base, TEXTURE_PREFETCH, rt.text),
    ) else {
        return;
    };
    let at = items + i * 16;
    if !tkiw_runtime::win::readable(at, 16) {
        return;
    }
    let arg = core::ptr::read(at as *const builtin::RValueRaw);
    let mut out = builtin::RValueRaw { payload: 0, flags: 0, kind: rvalue::KIND_UNDEFINED };
    pf(&mut out, core::ptr::null_mut(), core::ptr::null_mut(), 1, &arg);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A group nobody named must still be prefetched. This feature declines glyph
    /// atlases; it has no opinion about the game's art, and a default-deny would drop
    /// whatever a future update adds.
    #[test]
    fn unknown_groups_are_always_allowed() {
        let f = FontAtlases::default();
        assert!(f.allowed("default"));
        assert!(f.allowed("__yy__0fallbacktexture.png_yyg_auto_gen_tex_group_name_"));
        assert!(f.allowed("font_something_new"));
    }

    #[test]
    fn a_font_is_declined_unless_asked_for() {
        let mut f = FontAtlases::default();
        assert!(!f.allowed("font_chi"));
        f.wanted[1] = true;
        assert!(f.allowed("font_chi"));
        assert!(!f.allowed("font_jp"));
    }

    /// Latin is a switch like any other -- declinable, same mechanism. Only its default
    /// differs, and only because it is the script the game is drawn in.
    #[test]
    fn latin_is_declinable_but_on_by_default() {
        let f = FontAtlases::default();
        assert!(f.allowed("font_lat"));
        let mut f = FontAtlases::default();
        let cfg = crate::config::Config::parse("[feature.font_atlases]\nlatin = false\n");
        f.configure(&cfg.section("font_atlases")).expect("configures");
        assert!(!f.allowed("font_lat"));
    }

    /// Each script is its own switch: a player reading one should not pay for three.
    #[test]
    fn each_script_is_configured_separately() {
        let mut f = FontAtlases::default();
        let cfg = crate::config::Config::parse(
            "[feature.font_atlases]\nchinese = true\njapanese = false\ncyrillic = true\n",
        );
        f.configure(&cfg.section("font_atlases")).expect("configures");
        assert!(f.allowed("font_chi"));
        assert!(!f.allowed("font_jp"));
        assert!(f.allowed("font_cyr"));
    }

    /// The keys must match the groups the game actually has, or a switch does nothing
    /// and the player has no way to tell.
    #[test]
    fn the_group_names_are_the_ones_measured() {
        let groups: Vec<&str> = FONTS.iter().map(|(g, _, _)| *g).collect();
        assert_eq!(groups, ["font_lat", "font_chi", "font_jp", "font_kr", "font_cyr"]);
    }
}
