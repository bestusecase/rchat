//! rchat mobile shell entry point.
//!
//! Android starts the app through the generated `MainActivity`, which calls the
//! `run()` below via `tauri::mobile_entry_point`. The desktop binary keeps its own
//! `main` in `apps/desktop`; this one exists so `cargo run` still works on a
//! desktop for quick UI checks.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    rchat_mobile_lib::run()
}
