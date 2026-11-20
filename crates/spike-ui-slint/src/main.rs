//! M0 UI toolkit spike, Slint variant (ADR-0001).
//!
//! Same feature set as `spike-ui-win32`: system panel refreshed every second,
//! a 300-row sortable table, a modal confirmation, a tray icon, hide to tray
//! and restore. Benchmark hooks (`--ready-event`, `--hide-event`) let
//! `reclaim-bench ui-bench` measure startup and idle footprint.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() {
    app::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("spike-ui-slint only runs on Windows");
}
