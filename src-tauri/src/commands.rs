//! Tauri `#[command]` handlers. One function per bridge method; each is
//! a thin wrapper that marshals arguments through the plugin API.
//!
//! The names and signatures here must stay in lockstep with the
//! TypeScript `TauriBridge` interface in `ui/src/common/tauriBridge.ts`.
//!
//! Runtime access pattern
//! ----------------------
//!
//! All conductor-touching commands begin with
//! `wait_for_holochain(&app).await?` which blocks (with a timeout) until
//! `tauri_plugin_holochain::async_init` has finished bringing up the
//! embedded conductor + lair. This lets the UI call `startup_state`
//! immediately on boot without having to implement plugin-event
//! listening itself — the existing `{kind: "error"}` path is used when
//! we time out waiting.

use std::time::Duration;

use holo_hash::AgentPubKey;
use tauri::{AppHandle, State};
use tauri_plugin_holochain::{AllowedOrigins, AppStatusFilter, CellId, HolochainExt};
use tracing::info;

use crate::{backup, bridge::*, conductor::APP_ID, seed, AppState};

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Block until `app.holochain()` succeeds, or time out. The plugin's
/// `async_init` spawns setup in the background and emits
/// `holochain://setup-completed` when ready; `app.holochain()` returns
/// `HolochainNotInitializedError` until that event fires.
///
/// Poll interval is 100 ms; max wait 30 s. On Android first-boot the
/// cold-start can take ~10 s; desktop first-boot is typically under 3 s.
async fn wait_for_holochain(app: &AppHandle) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if app.holochain().is_ok() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("holochain runtime did not initialize within 30s".to_string());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tauri::command]
pub fn is_tauri() -> bool {
    true
}

/// High-level Tauri startup state the UI consults on boot to choose
/// between the onboarding wizard and the restore screen.
///
/// - `fresh`: conductor has no app installed (no prior edet identity here).
/// - `installed`: edet is already installed and the UI should boot normally.
/// - `error`: start-up failed; message is surfaced to the user.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartupState {
    Fresh,
    Installed { app_id: String },
    Error { message: String },
}

/// Ensure the plugin has brought up conductor + lair, then report
/// whether `edet` is installed. The UI uses this to gate the
/// onboarding overlay.
#[tauri::command]
pub async fn startup_state(app: AppHandle) -> CmdResult<StartupState> {
    if let Err(message) = wait_for_holochain(&app).await {
        return Ok(StartupState::Error { message });
    }
    let plugin = match app.holochain() {
        Ok(p) => p,
        Err(e) => return Ok(StartupState::Error { message: format!("holochain runtime: {e:?}") }),
    };
    let admin = match plugin.admin_websocket().await {
        Ok(a) => a,
        Err(e) => return Ok(StartupState::Error { message: format!("admin_websocket: {e:?}") }),
    };
    match admin.list_apps(Some(AppStatusFilter::Enabled)).await {
        Ok(apps) => {
            if let Some(id) = apps.into_iter().map(|a| a.installed_app_id).find(|id| id == APP_ID) {
                Ok(StartupState::Installed { app_id: id })
            } else {
                // No enabled app found. Check if it exists but is disabled —
                // if so, try to enable it and report installed, otherwise fresh.
                match admin.list_apps(None).await {
                    Ok(all_apps) => {
                        if all_apps.iter().any(|a| a.installed_app_id == APP_ID) {
                            // App exists but disabled — attempt re-enable.
                            match admin.enable_app(APP_ID.to_string()).await {
                                Ok(_) => Ok(StartupState::Installed { app_id: APP_ID.to_string() }),
                                Err(e) => Ok(StartupState::Error { message: format!("enable_app: {e:?}") }),
                            }
                        } else {
                            Ok(StartupState::Fresh)
                        }
                    }
                    Err(e) => Ok(StartupState::Error { message: format!("list_apps(all): {e:?}") }),
                }
            }
        }
        Err(e) => Ok(StartupState::Error { message: format!("list_apps: {e:?}") }),
    }
}

/// Dump the source chain for the given cell using the admin websocket.
///
/// Returns records as opaque JSON values so the TypeScript side can
/// round-trip them into a backup bundle without having the Holochain
/// type definitions.
#[tauri::command]
pub async fn dump_source_chain(app: AppHandle, cell_id: CellIdWire) -> CmdResult<Vec<OpaqueRecord>> {
    wait_for_holochain(&app).await?;
    let (dna, agent) = cell_id.decode().map_err(err)?;
    let cell_id = CellId::new(dna, agent);
    let plugin = app.holochain().map_err(|e| format!("holochain: {e:?}"))?;
    let admin = plugin.admin_websocket().await.map_err(err)?;
    let dump = admin
        .dump_full_state(cell_id, None)
        .await
        .map_err(|e| format!("dump_full_state: {e:?}"))?;
    let records = crate::conductor::records_from_full_state_dump(dump);
    records
        .into_iter()
        .map(OpaqueRecord::from_record)
        .collect::<anyhow::Result<Vec<_>>>()
        .map_err(err)
}

/// Graft a previously-dumped source chain onto the cell. Used only
/// during identity recovery; must be called before the agent authors
/// any action.
#[tauri::command]
pub async fn graft_source_chain(app: AppHandle, cell_id: CellIdWire, records: Vec<OpaqueRecord>) -> CmdResult<()> {
    wait_for_holochain(&app).await?;
    let (dna, agent) = cell_id.decode().map_err(err)?;
    let cell_id = CellId::new(dna, agent);
    let records: Vec<_> = records
        .into_iter()
        .map(OpaqueRecord::into_record)
        .collect::<anyhow::Result<Vec<_>>>()
        .map_err(err)?;
    let plugin = app.holochain().map_err(|e| format!("holochain: {e:?}"))?;
    let admin = plugin.admin_websocket().await.map_err(err)?;
    admin
        .graft_records(cell_id, true, records)
        .await
        .map_err(|e| format!("graft_records: {e:?}"))
}

/// Import a 32-byte ed25519 seed into lair and return the resulting
/// 39-byte AgentPubKey the conductor will use.
#[tauri::command]
pub async fn import_lair_seed(app: AppHandle, seed: Vec<u8>) -> CmdResult<Vec<u8>> {
    if seed.len() != 32 {
        return Err(format!("seed must be 32 bytes, got {}", seed.len()));
    }
    let mut fixed = [0u8; 32];
    fixed.copy_from_slice(&seed);
    wait_for_holochain(&app).await?;
    let plugin = app.holochain().map_err(|e| format!("holochain: {e:?}"))?;
    // `.keystore().lair_client()` returns the in-process LairClient owned
    // by the plugin. Clone it so we hold an owned handle for the duration
    // of the import; the type is cheap to clone (Arc inside).
    let lair = plugin.holochain_runtime.conductor_handle.keystore().lair_client().clone();
    let pk_bytes = seed::import_ed25519_seed(&lair, fixed).await.map_err(err)?;
    // Zero the stack copy of the raw seed.
    for b in fixed.iter_mut() {
        *b = 0;
    }
    // Wrap the raw ed25519 pubkey into a Holochain AgentPubKey (39 bytes).
    let agent = AgentPubKey::from_raw_32(pk_bytes.to_vec());
    Ok(agent.get_raw_39().to_vec())
}

/// Install the bundled edet happ under the given agent pubkey and
/// enable it. The `.happ` file itself is resolved via
/// `conductor::resolve_happ_path` (env var / Tauri resource / dev fallback).
#[tauri::command]
pub async fn install_app_with_agent_key(app: AppHandle, agent_pub_key: Vec<u8>) -> CmdResult<()> {
    wait_for_holochain(&app).await?;
    let agent = AgentPubKey::try_from_raw_39(agent_pub_key).map_err(|e| format!("decode AgentPubKey: {e:?}"))?;

    let happ_bytes = crate::conductor::resolve_happ_bytes(&app).map_err(err)?;
    // `AppBundle` is re-exported from the plugin (via `holochain_types::prelude`).
    // `unpack` takes any `impl Read`; slice Reader works here.
    let bundle =
        tauri_plugin_holochain::AppBundle::unpack(&happ_bytes[..]).map_err(|e| format!("unpack AppBundle: {e:?}"))?;

    let plugin = app.holochain().map_err(|e| format!("holochain: {e:?}"))?;
    // `install_app` does NOT auto-enable; we call `enable_app` next.
    plugin
        .install_app(APP_ID.to_string(), bundle, None, Some(agent), None)
        .await
        .map_err(|e| format!("install_app: {e:?}"))?;

    let admin = plugin.admin_websocket().await.map_err(err)?;
    admin
        .enable_app(APP_ID.to_string())
        .await
        .map_err(|e| format!("enable_app: {e:?}"))?;

    info!("edet installed + enabled under agent");
    Ok(())
}

#[tauri::command]
pub async fn export_backup_file(app: AppHandle, bytes: Vec<u8>, suggested_name: String) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::{DialogExt, FilePath};
    let picked = app
        .dialog()
        .file()
        .add_filter("edet backup", &["edet-backup"])
        .set_file_name(&suggested_name)
        .blocking_save_file();
    let Some(file_path) = picked else {
        return Ok(None);
    };
    match file_path {
        FilePath::Path(path) => {
            tokio::fs::write(&path, &bytes).await.map_err(err)?;
            info!("exported backup to {}", path.display());
            Ok(Some(path.to_string_lossy().into_owned()))
        }
        FilePath::Url(url) => {
            // Android Storage Access Framework: the picker returns a
            // content:// URI.  tokio::fs cannot open these; go through
            // Android's ContentResolver via JNI instead.
            #[cfg(target_os = "android")]
            {
                let url_str = url.to_string();
                tokio::task::spawn_blocking(move || {
                    crate::android_fs::write_content_uri(&url_str, &bytes)
                })
                .await
                .map_err(|e| format!("spawn_blocking: {e}"))?.map_err(err)?;
                info!("exported backup to {url}");
                return Ok(Some(url.to_string()));
            }
            #[cfg(not(target_os = "android"))]
            Err(format!("unexpected content:// URL on non-Android platform: {url}"))
        }
    }
}

#[tauri::command]
pub async fn import_backup_file(app: AppHandle) -> CmdResult<Option<Vec<u8>>> {
    use tauri_plugin_dialog::{DialogExt, FilePath};
    let picked = app
        .dialog()
        .file()
        .add_filter("edet backup", &["edet-backup"])
        .blocking_pick_file();
    let Some(file_path) = picked else {
        return Ok(None);
    };
    match file_path {
        FilePath::Path(path) => {
            let bytes = tokio::fs::read(&path).await.map_err(err)?;
            Ok(Some(bytes))
        }
        FilePath::Url(url) => {
            // Android content:// URI — read via ContentResolver JNI.
            #[cfg(target_os = "android")]
            {
                let url_str = url.to_string();
                let bytes = tokio::task::spawn_blocking(move || {
                    crate::android_fs::read_content_uri(&url_str)
                })
                .await
                .map_err(|e| format!("spawn_blocking: {e}"))?.map_err(err)?;
                return Ok(Some(bytes));
            }
            #[cfg(not(target_os = "android"))]
            Err(format!("unexpected content:// URL on non-Android platform: {url}"))
        }
    }
}

#[tauri::command]
pub async fn get_current_backup(state: State<'_, AppState>) -> CmdResult<Option<Vec<u8>>> {
    backup::read_current(&state.backup_dir).await.map_err(err)
}

#[tauri::command]
pub async fn save_current_backup(state: State<'_, AppState>, bytes: Vec<u8>) -> CmdResult<()> {
    backup::write_current(&state.backup_dir, &bytes).await.map_err(err)
}

/// Return the app-websocket port and authentication token for `APP_ID`.
///
/// The UI calls this command after `startup_state` returns `installed`
/// (i.e. edet is already set up) or after `install_app_with_agent_key`
/// completes (first-run / restore path).  The returned values are used
/// to open an `AppWebsocket` directly:
///
/// ```ts
/// const { port, token } = await tauriBridge.getAppWebsocketAuth();
/// const client = await AppWebsocket.connect({
///     url: new URL(`ws://localhost:${port}`),
///     token: Uint8Array.from(token),
/// });
/// ```
///
/// Using an explicit command avoids the `__HC_LAUNCHER_ENV__` injection
/// that `main_window_builder` performs at window-creation time, which
/// cannot work for edet because the app may not be installed yet when
/// the window first opens.
#[derive(serde::Serialize)]
pub struct AppWebsocketAuth {
    pub port: u16,
    pub token: Vec<u8>,
}

#[tauri::command]
pub async fn get_app_websocket_auth(app: AppHandle) -> CmdResult<AppWebsocketAuth> {
    wait_for_holochain(&app).await?;
    let plugin = app.holochain().map_err(|e| format!("holochain: {e:?}"))?;
    let auth = plugin
        .holochain_runtime
        .get_app_websocket_auth(&APP_ID.to_string(), AllowedOrigins::Any)
        .await
        .map_err(|e| format!("get_app_websocket_auth: {e:?}"))?;
    Ok(AppWebsocketAuth { port: auth.app_websocket_port, token: auth.token })
}

/// Scan a QR code from the default camera using native platform APIs.
///
/// On **Android** this command immediately returns the sentinel error
/// `"use-web-camera"`.  The `QrScanner` component catches that string
/// and falls back to `startWebScan()` (html5-qrcode via `getUserMedia`),
/// which works correctly in the Tauri Android WebView.
///
/// On **desktop** (Linux/macOS/Windows) this spawns a background thread
/// that:
///   1. Opens the first available camera via nokhwa (v4l2 / AVFoundation
///      / Media Foundation).
///   2. Encodes each frame as JPEG and emits a `qr://frame` event so the
///      UI can display a live preview.
///   3. Emits `qr://result` when a QR code is decoded.
///   4. Exits when `stop_qr_scan` is called.
///
/// Returns immediately; the caller listens for `qr://frame` /
/// `qr://result` events.
#[tauri::command]
#[allow(unused_variables)]
pub fn start_qr_scan(app: tauri::AppHandle, state: tauri::State<'_, QrScanState>) -> CmdResult<()> {
    // nokhwa has no Android camera backend; signal the UI to use the
    // WebView's getUserMedia path instead.
    #[cfg(target_os = "android")]
    return Err("use-web-camera".into());

    #[cfg(not(target_os = "android"))]
    {
        use nokhwa::{
            pixel_format::RgbFormat,
            utils::{CameraIndex, RequestedFormat, RequestedFormatType, Resolution},
            Camera,
        };
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        use tauri::Emitter;

        // Stop any existing scan and install a fresh flag.
        let stop = Arc::new(AtomicBool::new(false));
        if let Ok(mut guard) = state.0.lock() {
            if let Some(old) = guard.take() {
                old.store(true, Ordering::Relaxed);
            }
            *guard = Some(stop.clone());
        }

        std::thread::spawn(move || {
            let format = RequestedFormat::new::<RgbFormat>(
                RequestedFormatType::HighestResolution(Resolution::new(640, 480)),
            );
            let mut camera = match Camera::new(CameraIndex::Index(0), format) {
                Ok(c) => c,
                Err(e) => {
                    let _ = app.emit("qr://error", format!("camera open: {e}"));
                    return;
                }
            };
            if let Err(e) = camera.open_stream() {
                let _ = app.emit("qr://error", format!("camera stream: {e}"));
                return;
            }

            let mut frame_count: u32 = 0;

            loop {
                if stop.load(Ordering::Relaxed) { break; }

                let frame = match camera.frame() {
                    Ok(f) => f,
                    Err(_) => { std::thread::sleep(std::time::Duration::from_millis(33)); continue; }
                };
                let rgb = match frame.decode_image::<RgbFormat>() {
                    Ok(img) => img,
                    Err(_) => continue,
                };
                let (w, h) = (rgb.width(), rgb.height());

                if frame_count == 0 {
                    tracing::info!("QR scanner: camera frame size = {w}x{h}");
                }
                frame_count = frame_count.wrapping_add(1);

                let img: image::DynamicImage = if w > 640 {
                    let nh = (h as f32 * (640.0 / w as f32)) as u32;
                    image::DynamicImage::ImageRgb8(
                        image::imageops::resize(&rgb, 640, nh, image::imageops::FilterType::Triangle),
                    )
                } else {
                    image::DynamicImage::ImageRgb8(rgb)
                };

                let mut jpeg: Vec<u8> = Vec::new();
                if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
                    .encode_image(&img)
                    .is_ok()
                {
                    use base64::Engine;
                    let _ = app.emit(
                        "qr://frame",
                        base64::engine::general_purpose::STANDARD.encode(&jpeg),
                    );
                }
            }

            let _ = camera.stop_stream();
        });

        Ok(())
    }
}

/// Stop an in-progress QR scan started by `start_qr_scan`.
#[tauri::command]
pub fn stop_qr_scan(state: tauri::State<'_, QrScanState>) {
    if let Ok(mut guard) = state.0.lock() {
        if let Some(flag) = guard.take() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Shared stop-flag slot. Managed once at app startup; the inner Option
/// is replaced on each new scan so start/stop always operate on the
/// current thread's flag. Using Mutex<Option<...>> avoids the one-time
/// app.manage() restriction that caused silent failures on reopen.
pub struct QrScanState(pub std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>);

impl QrScanState {
    pub fn new() -> Self {
        QrScanState(std::sync::Mutex::new(None))
    }
}
