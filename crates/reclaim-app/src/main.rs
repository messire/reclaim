//! Reclaim desktop application.
//!
//! M0 skeleton. The UI toolkit is chosen by the M0 UI spike (ADR-0001); until
//! then this binary only exists so CI can track the baseline release size.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let _version = reclaim_core::VERSION;
}
