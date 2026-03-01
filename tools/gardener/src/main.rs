#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone,
    clippy::needless_borrowed_reference
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
