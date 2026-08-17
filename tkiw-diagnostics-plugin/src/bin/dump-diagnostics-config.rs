//! Print this mod's default config, for inspection or for staging with a release.
//!
//! ```bash
//! cargo run --release -p tkiw_diagnostics_plugin --bin dump-default-config
//! ```

fn main() {
    let text = tkiw_diagnostics_plugin::default_config();
    match std::env::args().nth(1) {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, text) {
                eprintln!("could not write {path}: {e}");
                std::process::exit(1);
            }
        }
        None => print!("{text}"),
    }
}
