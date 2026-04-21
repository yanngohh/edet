//! edet desktop entry point.
//!
//! This binary is an extremely thin wrapper around the library so that the
//! same `run()` function can be reused for mobile targets once Tauri mobile
//! support is enabled. Keep heavy logic in `lib.rs` and its submodules.

fn main() {
    edet_tauri_lib::run();
}
