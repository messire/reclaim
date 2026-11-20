//! M0 API spike: verifies the `[verify]` items of
//! `docs/architecture/OS-CAPABILITIES.md` on a real Windows machine and prints a
//! Markdown report.
//!
//! Usage: `api-spike [--out report.md] [--sizes 256,1024]`
//!
//! The spike never trims or modifies any process other than its own probe
//! children. Probes exit when their stdin is closed.

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("api-spike only runs on Windows");
    std::process::exit(2);
}
