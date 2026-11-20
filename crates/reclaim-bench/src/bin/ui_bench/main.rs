//! M0 UI toolkit benchmark (ADR-0001 gates, BENCHMARKS B1w, B2, B3).
//!
//! Usage: `ui-bench --exe <spike.exe> [--label name] [--runs 10] [--idle-secs 60] [--out file.md]`
//!
//! Measures warm start to "usable UI" (the app signals a named event after its
//! first painted frame), then idle CPU and private bytes with the window visible
//! and after closing it to the tray.

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ui-bench only runs on Windows");
    std::process::exit(2);
}
