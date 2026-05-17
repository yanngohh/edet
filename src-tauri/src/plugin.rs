//! Tauri plugin that manages the embedded Holochain conductor lifecycle.
//!
//! Registered under the plugin name `"holochain"` so that:
//! - `zome-call-signer.js` continues to call `plugin:holochain|sign_zome_call`
//!   without modification.
//! - The capability name `"holochain:allow-sign-zome-call"` in `lib.rs` stays
//!   the same.
//!
//! # Lifecycle
//!
//! 1. `plugin::init(config)` is registered in `tauri::Builder` (see `lib.rs`).
//! 2. During Tauri setup the plugin spawns `runtime::boot(config)` in the
//!    background.
//! 3. When the conductor is ready the runtime is stored in managed Tauri state
//!    and `holochain://setup-completed` is emitted.
//! 4. `lib.rs` listens for this event to open the main window.
//! 5. On `RunEvent::ExitRequested` the plugin shuts the conductor down
//!    gracefully (5-second timeout) before letting Tauri exit.
//!
//! # Extension trait
//!
//! `EdetRuntimeExt` is implemented on `AppHandle` to give any command handler
//! a one-liner access point identical in spirit to the old `HolochainExt`:
//!
//! ```rust,ignore
//! let rt = app.edet_runtime().map_err(|e| format!("{e}"))?;
//! ```

use tauri::{plugin::Builder, Emitter, Manager, Runtime, Wry};

use crate::runtime::{self, EdetRuntime, RuntimeConfig};

/// Returned from `edet_runtime()` when the runtime isn't ready yet.
#[derive(Debug)]
pub struct RuntimeNotReady;

impl std::fmt::Display for RuntimeNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "holochain runtime not yet initialized")
    }
}

/// Extension trait added to `AppHandle` for conductor access.
pub trait EdetRuntimeExt {
    /// Returns a reference to the `EdetRuntime` stored in managed state, or
    /// `RuntimeNotReady` if the conductor has not finished booting yet.
    fn edet_runtime(&self) -> Result<tauri::State<'_, EdetRuntime>, RuntimeNotReady>;
}

impl<R: Runtime> EdetRuntimeExt for tauri::AppHandle<R> {
    fn edet_runtime(&self) -> Result<tauri::State<'_, EdetRuntime>, RuntimeNotReady> {
        self.try_state::<EdetRuntime>().ok_or(RuntimeNotReady)
    }
}

/// Build the plugin.  Must be called from `tauri::Builder::plugin`.
///
/// The plugin is registered under name `"holochain"` — the same string that
/// `zome-call-signer.js` uses when it invokes
/// `plugin:holochain|sign_zome_call`.
#[allow(unused_variables)]
pub fn init(config: RuntimeConfig) -> tauri::plugin::TauriPlugin<Wry> {
    Builder::<Wry>::new("holochain")
        .setup(move |app, _api| {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                match runtime::boot(config).await {
                    Ok(rt) => {
                        handle.manage(rt);
                        let _ = handle.emit("holochain://setup-completed", ());
                        tracing::info!("conductor ready — emitted holochain://setup-completed");
                    }
                    Err(e) => {
                        tracing::error!("conductor boot failed: {e:#}");
                        let _ = handle.emit("holochain://setup-failed", e.to_string());
                    }
                }
            });
            Ok(())
        })
        .on_event(|app, event| {
            #[cfg(not(target_os = "android"))]
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(rt) = app.try_state::<EdetRuntime>() {
                    let rt = rt.inner().clone();
                    tauri::async_runtime::block_on(runtime::shutdown(&rt));
                }
            }
            #[cfg(target_os = "android")]
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        })
        .build()
}
