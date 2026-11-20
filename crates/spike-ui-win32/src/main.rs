//! M0 UI toolkit spike, native Win32 variant (ADR-0001).
//!
//! Same feature set as `spike-ui-slint`. The main window is destroyed when it is
//! closed to the tray and recreated on Open (ARCHITECTURE §8.2); the tray icon
//! lives on a separate hidden window.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() {
    app::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("spike-ui-win32 only runs on Windows");
}
