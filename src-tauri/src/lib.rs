//! edet Tauri shell — library entry point.
//!
//! Boots the embedded Holochain conductor + lair-keystore via the custom
//! `plugin` module (see `src-tauri/src/plugin.rs`).
//!
//! # Lifecycle
//!
//! - `setup` phase: `plugin::init(config)` spawns conductor boot in the
//!   background and emits `holochain://setup-completed` once ready.
//! - The UI can render the "New identity / Restore" screen immediately;
//!   it calls `startup_state` which blocks (with a 30s timeout) until the
//!   runtime state is managed.
//! - Commands in `commands.rs` reach the runtime via `app.edet_runtime()`.
//! - Shutdown: `plugin.rs` handles `RunEvent::ExitRequested`, gracefully
//!   shutting the conductor down (5s timeout) before Tauri exits.
//!
//! # Passphrase
//!
//! A 32-byte random passphrase is stored at
//! `$APPDATA/edet/.lair-passphrase` (hex-encoded). Lair uses it to
//! encrypt the on-disk keystore; losing the file means losing the
//! keystore (mnemonic recovery is the fallback in that case).
//!
//! # App-data path
//!
//! `app_dirs2::app_root(UserData, ...)` gives a cross-platform path that
//! works on Linux, macOS, Windows, and Android. The name / author match
//! the Tauri identifier in `tauri.conf.json` (`org.edet`).

pub mod backup;
pub mod bridge;
pub mod commands;
pub mod conductor;
pub mod platform;
pub mod plugin;
pub mod runtime;
pub mod seed;
pub mod signer;

// Android Storage Access Framework helpers (content:// URI I/O via JNI).
// Only compiled when targeting Android; not needed on desktop.
#[cfg(target_os = "android")]
pub mod android_fs;

use std::path::PathBuf;

use tauri::{Listener, Manager};
use tracing::{error, info};

use crate::runtime::{RuntimeConfig, vec_to_locked};

// The zome-call-signer.js script sets window.__HC_ZOME_CALL_SIGNER__ so that
// AppWebsocket uses lair (via the plugin's sign_zome_call command) to sign
// zome calls.  We inject it at window creation time via initialization_script().
// Plugin name is "holochain" so the script's `plugin:holochain|sign_zome_call`
// IPC path requires no changes.
const ZOME_CALL_SIGNER_SCRIPT: &str = include_str!("../zome-call-signer.js");

/// Minimal runtime state beyond what the conductor owns.
pub struct AppState {
    pub backup_dir: PathBuf,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    info!("edet-tauri starting");

    // Load or create the lair passphrase from the user-data directory.
    const APP_INFO: app_dirs2::AppInfo =
        app_dirs2::AppInfo { name: "edet", author: "edet contributors" };
    let app_data_root = app_dirs2::app_root(app_dirs2::AppDataType::UserData, &APP_INFO)
        .expect("could not resolve app data root");
    let passphrase_bytes = runtime::ensure_lair_passphrase(&app_data_root)
        .expect("lair passphrase setup failed");
    let passphrase = vec_to_locked(passphrase_bytes);

    let config = RuntimeConfig {
        data_dir:       runtime::holochain_dir(),
        passphrase,
        network_config: runtime::network_config(),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        // Our custom conductor plugin (see plugin.rs / runtime.rs).
        // Registered as "holochain" so zome-call-signer.js needs no changes.
        .plugin(plugin::init(config))
        // Foreground service plugin — no-op on desktop, starts an Android
        // Foreground Service on mobile to keep the process alive.
        .plugin(tauri_plugin_conductor_service::init())
        .setup(|app| {
            let app_data = app
                .path()
                .app_data_dir()
                .expect("app data dir is available on all supported platforms");
            std::fs::create_dir_all(&app_data).ok();

            app.manage(AppState { backup_dir: app_data });
            // QR scan state: managed once, updated on each scan.
            app.manage(commands::QrScanState::new());

            // Once the conductor + lair are ready, open the main window.
            // We listen for the event rather than awaiting `plugin::init`
            // so the UI can render a loading/onboarding screen immediately.
            let handle = app.handle().clone();
            app.handle().listen("holochain://setup-completed", move |_ev| {
                info!("conductor ready — opening main window");
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let builder = tauri::WebviewWindowBuilder::new(
                        &handle,
                        "main",
                        tauri::WebviewUrl::App("index.html".into()),
                    )
                    .initialization_script(ZOME_CALL_SIGNER_SCRIPT)
                    .on_page_load(|window, _| platform::configure_webview_on_load(&window));

                    // Window geometry APIs are desktop-only; Android lets the
                    // system control window size.
                    #[cfg(desktop)]
                    let builder = builder
                        .title("edet")
                        .inner_size(1280.0, 800.0)
                        .min_inner_size(960.0, 600.0)
                        .resizable(true);

                    let _ = builder.build().expect("failed to open main window");
                });
            });

            app.handle().listen("holochain://setup-failed", |_ev| {
                error!("conductor setup failed — UI will see an error state");
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::is_tauri,
            commands::startup_state,
            commands::dump_source_chain,
            commands::graft_source_chain,
            commands::import_lair_seed,
            commands::install_app_with_agent_key,
            commands::export_backup_file,
            commands::import_backup_file,
            commands::get_current_backup,
            commands::save_current_backup,
            commands::get_app_websocket_auth,
            commands::start_qr_scan,
            commands::stop_qr_scan,
            signer::sign_zome_call,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the tauri application");
}
