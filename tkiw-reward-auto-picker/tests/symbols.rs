//! Validates the runtime symbol discovery against a real game executable.
//!
//! The numbers here are the ones the Python analysis in `analysis/` produces
//! for the build installed 2026-08-10. They are a regression check on the
//! *algorithm*, not on the game: if a game update changes them, that is
//! information, not a failure -- rerun `python analysis/index.py` and compare.
//!
//! Skips itself when no game executable can be found, so `cargo test` still
//! passes on a machine without the game.

use tkiw_reward_picker::{gml, pe};

const CANDIDATES: &[&str] = &[
    // the pristine copy the morale-fix keeps, preferred: unpatched
    r"..\tkiw-morale-fix\The King is Watching.exe.orig",
    r"C:\Program Files (x86)\Steam\steamapps\common\The King is Watching\The King is Watching.exe",
];

const EXPECT_FUNCS: usize = 13_132;
const EXPECT_SLOTS: usize = 13_476;

fn image() -> Option<pe::Image> {
    CANDIDATES.iter().find_map(|p| pe::Image::load(p))
}

#[test]
fn discovers_both_symbol_tables() {
    let Some(img) = image() else {
        eprintln!("no game executable found; skipping");
        return;
    };
    let syms = gml::Symbols::discover(&img, img.image_base as usize);

    assert_eq!(syms.slide, 0, "zero slide when live base == image base");
    assert_eq!(syms.functions.len(), EXPECT_FUNCS, "gml function count");
    assert_eq!(syms.var_slots.len(), EXPECT_SLOTS, "variable slot count");
}

#[test]
fn resolves_the_symbols_the_mod_depends_on() {
    let Some(img) = image() else {
        eprintln!("no game executable found; skipping");
        return;
    };
    let syms = gml::Symbols::discover(&img, img.image_base as usize);

    for name in [
        "gml_Script_spawn_rewards_choice@gml_Object_obj_run_controller_Create_0",
        "gml_Script_spawn_choice_unified@gml_Object_obj_run_controller_Create_0",
        "gml_Script_spawn_stat_upgrade_choice@gml_Object_obj_run_controller_Create_0",
        "gml_Script_reward_library",
        "gml_Script_library_add_reward",
        "gml_Object_obj_reward_option_Create_0",
    ] {
        assert!(syms.func(name).is_some(), "missing function {name}");
    }

    for name in [
        // the self-validating read test depends on these existing
        "REWARD_ARTIFACT",
        "REWARD_SPELL",
        "pending_rewards",
        "reward",
        "reward_type",
        "run_rerolls_left",
        "FREE_REROLLS_PER_RUN_LEFT",
        "FREE_REROLLS_PER_REWARD_LIMIT",
        "free_rerolls_per_reward_left",
        "non_free_rerolls_made",
        "resolve_reroll_cost",
        "REWARD_UNIT_CLASS_STAT",
        "REWARD_RESOURCE",
    ] {
        assert!(syms.slot(name).is_some(), "missing variable {name}");
    }
}

#[test]
fn function_addresses_land_in_text() {
    let Some(img) = image() else {
        eprintln!("no game executable found; skipping");
        return;
    };
    let syms = gml::Symbols::discover(&img, img.image_base as usize);
    let text = img.section(".text").expect(".text");
    let (lo, hi) = (text.va, text.va + text.vsize);

    for (name, &rva) in syms.functions.iter().take(2000) {
        assert!(rva >= lo && rva < hi, "{name} at {rva:#x} outside .text");
    }
}

#[test]
fn lookups_move_with_the_load_address() {
    let Some(img) = image() else {
        eprintln!("no game executable found; skipping");
        return;
    };
    let base = img.image_base as usize;
    let plain = gml::Symbols::discover(&img, base);
    let moved = gml::Symbols::discover(&img, base + 0x10_0000);

    let name = "gml_Script_reward_library";
    assert_eq!(
        moved.func(name).unwrap() - plain.func(name).unwrap(),
        0x10_0000,
        "function lookups must move with the load address"
    );
    let var = "pending_rewards";
    assert_eq!(
        moved.slot(var).unwrap() - plain.slot(var).unwrap(),
        0x10_0000,
        "variable slots must move with the load address"
    );
}

/// The regression test for the bug that took the game down.
///
/// A relative check like the one above passes happily while every address is
/// wrong by a constant -- which is exactly what `rva + slide` was, short by
/// `image_base`. Only an absolute bound catches it: every derived address must
/// land inside the module as actually loaded.
#[test]
fn every_address_lands_inside_the_loaded_module() {
    let Some(img) = image() else {
        eprintln!("no game executable found; skipping");
        return;
    };
    // a realistic high-entropy ASLR base, not the preferred one, so a
    // base/slide confusion cannot accidentally cancel out
    let base = 0x7ff7_76cb_0000usize;
    let syms = gml::Symbols::discover(&img, base);
    let end = base + img.size_of_image as usize;

    for (name, _) in syms.functions.iter().take(3000) {
        let a = syms.func(name).unwrap();
        assert!(a >= base && a < end,
                "function {name} at {a:#x} outside module {base:#x}..{end:#x}");
    }
    for (name, _) in syms.var_slots.iter().take(3000) {
        let a = syms.slot(name).unwrap();
        assert!(a >= base && a < end,
                "variable {name} slot at {a:#x} outside module {base:#x}..{end:#x}");
    }
}

/// The import-table slot the frame hook redirects.
///
/// The expected RVA is what `analysis/` reads out of the same binary, so this
/// is an independent implementation agreeing with the Python one.
#[test]
fn finds_the_message_pump_import() {
    let Some(img) = image() else {
        eprintln!("no game executable found; skipping");
        return;
    };
    assert_eq!(
        img.iat_slot("USER32.dll", "PeekMessageW"),
        Some(0x21ca8a8),
        "PeekMessageW IAT slot"
    );
    // case-insensitive dll matching, since the import name's case is arbitrary
    assert_eq!(
        img.iat_slot("user32.dll", "PeekMessageW"),
        Some(0x21ca8a8),
        "dll name matching must be case-insensitive"
    );
    assert_eq!(
        img.iat_slot("USER32.dll", "DispatchMessageW"),
        Some(0x21ca8b0)
    );
    assert_eq!(img.iat_slot("USER32.dll", "NoSuchFunction"), None);
    assert_eq!(img.iat_slot("nosuch.dll", "PeekMessageW"), None);
}

/// Live address must be exactly base + rva, checked against a known symbol.
#[test]
fn live_address_is_base_plus_rva() {
    let Some(img) = image() else {
        eprintln!("no game executable found; skipping");
        return;
    };
    let base = 0x7ff7_76cb_0000usize;
    let syms = gml::Symbols::discover(&img, base);

    let name = "gml_Script_spawn_rewards_choice@gml_Object_obj_run_controller_Create_0";
    let rva = *syms.functions.get(name).expect(name) as usize;
    assert_eq!(syms.func(name).unwrap(), base + rva);
    assert_ne!(
        syms.func(name).unwrap(),
        rva + syms.slide,
        "base + rva must not coincide with rva + slide, or the test proves nothing"
    );
}
