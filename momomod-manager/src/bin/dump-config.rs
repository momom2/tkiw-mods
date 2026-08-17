//! Print the config files the kit would generate on first launch.
//!
//! A development tool, not shipped. It exists so that the defaults can be
//! inspected and diffed without launching the game -- and so that nobody is ever
//! tempted to hand-write a config that drifts from what the kit generates.
//!
//! ```bash
//! cargo run --release --bin dump-config              # every file, to stdout
//! cargo run --release --bin dump-config -- optimization  # just that mod's file
//! cargo run --release --bin dump-config -- --into config/   # write them
//! ```
//!
//! Writing never overwrites an existing file, for the same reason the kit does
//! not: regenerating a config over a tuned one left it inert and the mod looking
//! broken, once.

use momomod_manager::config;
use momomod_manager::features;

fn render(name: &str) -> Option<String> {
    if name == "momomod" || name == config::KIT_FILE {
        return Some(features::render_kit_config());
    }
    features::MODS
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .map(|m| features::render_mod_config(m.name))
}

/// Every file the kit generates, as `(filename, contents)`.
fn everything() -> Vec<(String, String)> {
    std::iter::once((config::KIT_FILE.to_string(), features::render_kit_config()))
        .chain(
            features::MODS
                .iter()
                .map(|m| (config::mod_file(m.name), features::render_mod_config(m.name))),
        )
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(i) = args.iter().position(|a| a == "--into") {
        let Some(dir) = args.get(i + 1) else {
            eprintln!("--into needs a directory");
            std::process::exit(2);
        };
        let dir = std::path::Path::new(dir);
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("could not create {}: {e}", dir.display());
            std::process::exit(1);
        }
        let mut wrote = 0;
        for (name, text) in everything() {
            let path = dir.join(&name);
            if path.exists() {
                eprintln!("{} already exists; left alone.", path.display());
                continue;
            }
            match std::fs::write(&path, &text) {
                Ok(()) => {
                    eprintln!("wrote {}", path.display());
                    wrote += 1;
                }
                Err(e) => {
                    eprintln!("could not write {}: {e}", path.display());
                    std::process::exit(1);
                }
            }
        }
        eprintln!("{wrote} file(s) written");
        return;
    }

    match args.first() {
        None => {
            for (name, text) in everything() {
                println!("# ===== {name} =====");
                print!("{text}");
                println!();
            }
        }
        Some(name) => match render(name) {
            Some(text) => print!("{text}"),
            None => {
                eprintln!(
                    "no such mod: {name:?}. Known: momomod, {}",
                    features::MODS.iter().map(|m| m.name).collect::<Vec<_>>().join(", ")
                );
                std::process::exit(2);
            }
        },
    }
}
