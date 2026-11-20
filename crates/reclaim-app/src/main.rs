//! Reclaim desktop application (Windows). Run with `--tray` to start in the tray.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() {
    std::process::exit(ui::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Reclaim's desktop app runs on Windows only.");
    std::process::exit(2);
}
