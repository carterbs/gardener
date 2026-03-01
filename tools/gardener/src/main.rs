#![deny(
    clippy::manual_strip,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_update,
    clippy::redundant_clone
)]

fn main() {
    match gardener::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
