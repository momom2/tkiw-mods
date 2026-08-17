//! Print the mod catalogue: the published mods, as JSON, for the mod manager.
//!
//! A development tool, not shipped. The mod manager fetches this from the
//! release, but it is generated from [`momomod_manager::features::MODS`] so the two
//! never drift -- what the kit publishes is what the installer offers.
//!
//! ```bash
//! cargo run --release --bin dump-catalog                 # to stdout
//! cargo run --release --bin dump-catalog -- --into .     # write catalog.json here
//! ```

use momomod_manager::features;

fn main() {
    let json = features::render_catalog();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(i) = args.iter().position(|a| a == "--into") {
        let Some(dir) = args.get(i + 1) else {
            eprintln!("--into needs a directory");
            std::process::exit(2);
        };
        let path = std::path::Path::new(dir).join("catalog.json");
        match std::fs::write(&path, &json) {
            Ok(()) => println!("wrote {}", path.display()),
            Err(e) => {
                eprintln!("could not write {}: {e}", path.display());
                std::process::exit(1);
            }
        }
    } else {
        print!("{json}");
    }
}
