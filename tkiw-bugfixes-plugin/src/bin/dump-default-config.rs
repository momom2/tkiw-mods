//! Print this mod's default config, for staging with a release.
//!
//! The mod manager needs a mod's config document to enable it *before* the mod
//! has ever run (a mod writes its own config on first launch, which is too late
//! for "install and enable"). Rather than keep a hand-written copy in the
//! manager -- which drifts the moment a description is reworded -- the release
//! ships this rendering, and the manager copies it.
//!
//! ```bash
//! cargo run --release --bin dump-default-config              # to stdout
//! cargo run --release --bin dump-default-config -- out.ini   # to a file
//! ```

fn main() {
    let text = tkiw_bugfixes_plugin::default_config();
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
