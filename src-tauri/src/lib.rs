//! edet Tauri shell — library entry point.
//!
//! This crate wraps the [`tauri-plugin-holochain`] in-process runtime
//! (embedded `holochain` + `lair-keystore`, no subprocesses) and
//! exposes the narrow command surface defined in `PLAN.md §10` to the
//! Svelte UI. The commands in `commands.rs` are mapped 1:1 with the
//! TypeScript `tauriBridge` contract in `ui/src/common/tauriBridge.ts`.
//!
//! Lifecycle
//! ---------
//!
//! - `setup` phase: we register `tauri_plugin_holochain::async_init`
//!   which spawns the runtime in the background and emits
//!   `holochain://setup-completed` once the embedded conductor + lair
//!   are ready. The UI can render the "New identity / Restore" screen
//!   immediately without waiting for Holochain to boot.
//! - Commands in `commands.rs` acquire the runtime via
//!   `app.holochain()?` (the plugin's `HolochainExt` extension trait).
//!   The runtime is `Clone` and cheap to reach from any handle.
//! - Shutdown is handled by the plugin's own `RunEvent::ExitRequested`
//!   listener, which disables all apps (so peers learn we've left)
//!   before terminating the conductor.
//!
//! Passphrase
//! ----------
//!
//! We persist a 32-byte random passphrase at
//! `$APPDATA/edet/.lair-passphrase` and feed it to the plugin via
//! `vec_to_locked`. Lair encrypts the on-disk keystore with this
//! passphrase; losing the file means losing the keystore (mnemonic
//! recovery is the intended recourse in that case).
//!
//! App-data path
//! -------------
//!
//! We use `app_dirs2::app_root(UserData, ...)` for a cross-platform
//! resolution that works on Linux, macOS, Windows, and Android. The
//! name / author fields match the Tauri identifier in
//! `tauri.conf.json` (`org.edet.app`).

pub mod backup;
pub mod bridge;
pub mod commands;
pub mod conductor;
pub mod platform;
pub mod seed;

// Android Storage Access Framework helpers (content:// URI I/O via JNI).
// Only compiled when targeting Android; not needed on desktop.
#[cfg(target_os = "android")]
pub mod android_fs;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use tauri::{ipc::CapabilityBuilder, Listener, Manager};
use tauri_plugin_holochain::{vec_to_locked, HolochainPluginConfig, NetworkConfig};
use tracing::{error, info};

// The zome-call-signer.js script sets window.__HC_ZOME_CALL_SIGNER__ so that
// AppWebsocket uses lair (via the plugin's sign_zome_call command) to sign
// zome calls. main_window_builder injects this automatically; since we open a
// plain window we inject it ourselves via initialization_script().
// Kept in src-tauri/zome-call-signer.js — copy from the plugin source when
// upgrading tauri-plugin-holochain (or run `npm run sync-signer`).
const ZOME_CALL_SIGNER_SCRIPT: &str = include_str!("../zome-call-signer.js");

/// Minimal runtime state. The plugin owns the conductor + keystore;
/// we only need to remember where to drop the backup file.
pub struct AppState {
    pub backup_dir: PathBuf,
}

/// App identifier used by `app_dirs2` for cross-platform path
/// resolution. Matches `tauri.conf.json`'s `identifier` field.
const APP_INFO: app_dirs2::AppInfo = app_dirs2::AppInfo {
    name: "edet",
    author: "edet contributors",
};

/// Returns the directory where the embedded Holochain runtime will
/// store its databases, keystore, and bundled apps. Deterministic
/// across launches; the plugin itself handles sub-directory layout.
///
/// In `tauri::is_dev()` mode we use a separate "edet-dev" subdir so a
/// dev session does not collide with a previously-installed release
/// build's data.
fn holochain_dir() -> PathBuf {
    let subdir = if tauri::is_dev() { "holochain-dev" } else { "holochain" };
    app_dirs2::app_root(app_dirs2::AppDataType::UserData, &APP_INFO)
        .expect("could not resolve app data root")
        .join(subdir)
}

/// Build the NetworkConfig for the embedded conductor.
fn network_config() -> NetworkConfig {
    #[allow(unused_mut)]
    let mut cfg = NetworkConfig::default();
    if cfg!(mobile) {
        cfg.target_arc_factor = 0;
    }
    cfg
}

/// Read the lair passphrase from `$APPDATA/edet/.lair-passphrase`;
/// create a random one if the file doesn't exist yet.
fn ensure_lair_passphrase(dir: &Path) -> std::io::Result<Vec<u8>> {
    let path = dir.join(".lair-passphrase");
    if let Ok(bytes) = std::fs::read(&path) {
        if !bytes.is_empty() {
            return Ok(bytes);
        }
    }
    std::fs::create_dir_all(dir).ok();

    let mut raw = [0u8; 32];
    getrandom::getrandom(&mut raw).map_err(std::io::Error::other)?;
    let hex = hex::encode(raw);
    std::fs::write(&path, &hex)?;
    platform::restrict_file_permissions(&path);

    Ok(hex.into_bytes())
}

/// Load or mint the passphrase, wrap it in the plugin's expected
/// `SharedLockedArray`.
fn load_passphrase() -> Result<lair_keystore_api::prelude::SharedLockedArray> {
    let app_data = app_dirs2::app_root(app_dirs2::AppDataType::UserData, &APP_INFO)
        .map_err(|e| anyhow!("app_dirs2::app_root: {e}"))?;
    let bytes = ensure_lair_passphrase(&app_data)
        .map_err(|e| anyhow!("ensure_lair_passphrase: {e}"))?;
    Ok(vec_to_locked(bytes))
}

/// Run the desktop application.
///
/// Called from both the binary entry point (`main.rs`) and the mobile
/// shim (via `#[cfg_attr(mobile, tauri::mobile_entry_point)]`). The
/// function returns only when the Tauri event loop ends, which happens
/// after the last window is closed.
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

    let passphrase = load_passphrase().expect("lair passphrase setup failed");
    let hc_config = HolochainPluginConfig::new(holochain_dir(), network_config());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_holochain::async_init(passphrase, hc_config))
        .setup(|app| {
            let app_data = app
                .path()
                .app_data_dir()
                .expect("app data dir is available on all supported platforms");
            std::fs::create_dir_all(&app_data).ok();

            app.manage(AppState { backup_dir: app_data });
            // QR scan state: managed once, updated on each scan.
            app.manage(commands::QrScanState::new());
            // Once the embedded conductor + lair are ready, open the main
            // window. We use a plain WebviewUrl::App window rather than
            // `main_window_builder` because edet may not be installed yet
            // on a fresh device — `main_window_builder` would try to issue
            // an app-websocket token for "edet" and fail.  Instead the UI
            // calls the `get_app_websocket_auth` command explicitly after
            // installation completes (or on every boot once installed).
            let handle = app.handle().clone();
            app.handle().listen("holochain://setup-completed", move |_ev| {
                info!("holochain runtime setup completed — opening main window");
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    // Grant the sign_zome_call IPC permission to the main
                    // window. main_window_builder does this automatically;
                    // since we open a plain window we add it explicitly.
                    handle
                        .add_capability(
                            CapabilityBuilder::new("sign-zome-call")
                                .permission("holochain:allow-sign-zome-call")
                                .window("main"),
                        )
                        .expect("failed to add sign-zome-call capability");

                    let builder = tauri::WebviewWindowBuilder::new(
                        &handle,
                        "main",
                        tauri::WebviewUrl::App("index.html".into()),
                    )
                    .initialization_script(ZOME_CALL_SIGNER_SCRIPT)
                    .on_page_load(|window, _| platform::configure_webview_on_load(&window));

                    // title/inner_size/resizable are desktop-only APIs;
                    // on Android the system controls window geometry.
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
                error!("holochain runtime setup failed — UI will see an error state");
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running the tauri application");
}
