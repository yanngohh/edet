//! Tauri plugin for the edet Android Foreground Service.
//!
//! On Android: auto-starts `EdetConductorService` when the plugin loads
//! (during Tauri app init), keeping the Holochain conductor process alive
//! even when the app UI is backgrounded or swiped away.
//!
//! On desktop: a complete no-op. All public surface compiles on all targets
//! but the Android-specific code is gated behind `#[cfg(target_os = "android")]`
//! inside the Kotlin plugin.
//!
//! Also exposes commands for:
//! - Battery optimization exemption requests (Android only)
//! - Detecting whether the shared `android-service-runtime` is installed

use tauri::{plugin::Builder, Runtime};

/// Initialize the plugin. Call from `tauri::Builder::plugin`.
pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    Builder::<R>::new("conductor-service")
        // No Rust-side setup needed — the Android service is started by
        // the Kotlin plugin's `load()` hook automatically.
        .build()
}
