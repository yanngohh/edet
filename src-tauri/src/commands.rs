//! Tauri `#[command]` handlers. One function per bridge method; each is
//! a thin wrapper that marshals arguments through the conductor admin API.
//!
//! The names and signatures here must stay in lockstep with the
//! TypeScript `TauriBridge` interface in `ui/src/common/tauriBridge.ts`.
//!
//! # Runtime access pattern
//!
//! All conductor-touching commands call `wait_for_runtime(&app).await?`
//! which blocks (with a 30-second timeout) until `plugin::init` has finished
//! booting the conductor.  This lets the UI call `startup_state` immediately
//! on boot without needing to implement event listening itself.
//!
//! Once ready, commands reach the conductor via `app.edet_runtime()` and
//! dispatch admin requests through `runtime::admin_request` — in-process
//! function calls with no websocket serialization overhead.

use std::time::Duration;

use holo_hash::AgentPubKey;
use holochain::conductor::api::{AdminRequest, AdminResponse};
use holochain_conductor_api::AppStatusFilter;
use holochain_types::app::{AppBundleSource, InstallAppPayload};
use holochain_zome_types::prelude::CellId;
use tauri::{AppHandle, State};
use tracing::info;

use crate::{backup, bridge::*, conductor::APP_ID, plugin::EdetRuntimeExt, runtime, seed, AppState};

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Block until the `EdetRuntime` appears in Tauri managed state (conductor
/// boot finished) or the 30-second deadline elapses.
async fn wait_for_runtime(app: &AppHandle) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if app.edet_runtime().is_ok() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("conductor did not initialize within 30s".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ─── Simple commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn is_tauri() -> bool {
    true
}

// ─── Startup state ───────────────────────────────────────────────────────────

/// High-level startup state the UI consults on boot to choose between the
/// onboarding wizard and the main app screen.
///
/// - `fresh`:     conductor running but edet happ not installed.
/// - `installed`: edet is installed and enabled; UI should boot normally.
/// - `error`:     conductor failed to start or admin request failed.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartupState {
    Fresh,
    Installed { app_id: String },
    Error { message: String },
}

#[tauri::command]
pub async fn startup_state(app: AppHandle) -> CmdResult<StartupState> {
    if let Err(message) = wait_for_runtime(&app).await {
        return Ok(StartupState::Error { message });
    }
    let rt = match app.edet_runtime() {
        Ok(r) => r,
        Err(e) => return Ok(StartupState::Error { message: format!("{e}") }),
    };

    // Check for an enabled instance of the edet app.
    let enabled_resp = runtime::admin_request(
        &rt.conductor,
        AdminRequest::ListApps { status_filter: Some(AppStatusFilter::Enabled) },
    )
    .await
    .map_err(err)?;

    if let AdminResponse::AppsListed(apps) = enabled_resp {
        if let Some(id) = apps.into_iter().map(|a| a.installed_app_id).find(|id| id == APP_ID) {
            return Ok(StartupState::Installed { app_id: id });
        }
    }

    // Not enabled — check if it exists in a disabled state and re-enable it.
    let all_resp = runtime::admin_request(
        &rt.conductor,
        AdminRequest::ListApps { status_filter: None },
    )
    .await
    .map_err(err)?;

    if let AdminResponse::AppsListed(all) = all_resp {
        if all.iter().any(|a| a.installed_app_id == APP_ID) {
            let enable_resp = runtime::admin_request(
                &rt.conductor,
                AdminRequest::EnableApp { installed_app_id: APP_ID.to_string() },
            )
            .await
            .map_err(err)?;
            match enable_resp {
                AdminResponse::AppEnabled(info) => {
                    return Ok(StartupState::Installed { app_id: info.installed_app_id })
                }
                other => {
                    return Ok(StartupState::Error {
                        message: format!("enable_app unexpected response: {other:?}"),
                    })
                }
            }
        }
    }

    Ok(StartupState::Fresh)
}

// ─── Source chain dump / graft ────────────────────────────────────────────────

/// Dump the full source chain of `cell_id` for backup.
/// Returns records as opaque JSON values for round-tripping without Holochain
/// type definitions in TypeScript.
#[tauri::command]
pub async fn dump_source_chain(app: AppHandle, cell_id: CellIdWire) -> CmdResult<Vec<OpaqueRecord>> {
    wait_for_runtime(&app).await?;
    let (dna, agent) = cell_id.decode().map_err(err)?;
    let cell = CellId::new(dna, agent);
    let rt = app.edet_runtime().map_err(|e| format!("{e}"))?;

    let response = runtime::admin_request(
        &rt.conductor,
        AdminRequest::DumpFullState { cell_id: Box::new(cell), dht_ops_cursor: None },
    )
    .await
    .map_err(err)?;

    match response {
        AdminResponse::FullStateDumped(dump) => {
            let records = crate::conductor::records_from_full_state_dump(dump);
            records
                .into_iter()
                .map(OpaqueRecord::from_record)
                .collect::<anyhow::Result<Vec<_>>>()
                .map_err(err)
        }
        other => Err(format!("dump_source_chain unexpected response: {other:?}")),
    }
}

/// Graft a previously-dumped source chain onto `cell_id`. Used only during
/// identity recovery; call before the agent authors any action.
#[tauri::command]
pub async fn graft_source_chain(
    app: AppHandle,
    cell_id: CellIdWire,
    records: Vec<OpaqueRecord>,
) -> CmdResult<()> {
    wait_for_runtime(&app).await?;
    let (dna, agent) = cell_id.decode().map_err(err)?;
    let cell = CellId::new(dna, agent);
    let records: Vec<_> = records
        .into_iter()
        .map(OpaqueRecord::into_record)
        .collect::<anyhow::Result<Vec<_>>>()
        .map_err(err)?;
    let rt = app.edet_runtime().map_err(|e| format!("{e}"))?;

    let response = runtime::admin_request(
        &rt.conductor,
        // cell_id is not boxed in the 0.6.0 GraftRecords variant
        AdminRequest::GraftRecords { cell_id: cell, validate: true, records },
    )
    .await
    .map_err(err)?;

    match response {
        AdminResponse::RecordsGrafted => Ok(()),
        other => Err(format!("graft_source_chain unexpected response: {other:?}")),
    }
}

// ─── Identity / seed management ───────────────────────────────────────────────

/// Import a 32-byte ed25519 seed into lair and return the resulting 39-byte
/// `AgentPubKey`. Used by the onboarding wizard when the user provides a
/// mnemonic phrase.
#[tauri::command]
pub async fn import_lair_seed(app: AppHandle, seed: Vec<u8>) -> CmdResult<Vec<u8>> {
    if seed.len() != 32 {
        return Err(format!("seed must be 32 bytes, got {}", seed.len()));
    }
    let mut fixed = [0u8; 32];
    fixed.copy_from_slice(&seed);
    wait_for_runtime(&app).await?;
    let rt = app.edet_runtime().map_err(|e| format!("{e}"))?;
    let pk_bytes = seed::import_ed25519_seed(&rt.lair_client, fixed).await.map_err(err)?;
    // Zero the stack copy of the raw seed.
    for b in fixed.iter_mut() {
        *b = 0;
    }
    // Wrap the raw 32-byte ed25519 pubkey into a Holochain AgentPubKey (39 bytes).
    let agent = AgentPubKey::from_raw_32(pk_bytes.to_vec());
    Ok(agent.get_raw_39().to_vec())
}

/// Install the bundled edet happ under the given agent pubkey and enable it.
/// The `.happ` bytes are resolved via `conductor::resolve_happ_bytes`.
#[tauri::command]
pub async fn install_app_with_agent_key(app: AppHandle, agent_pub_key: Vec<u8>) -> CmdResult<()> {
    wait_for_runtime(&app).await?;
    let agent = AgentPubKey::try_from_raw_39(agent_pub_key)
        .map_err(|e| format!("decode AgentPubKey: {e:?}"))?;

    let happ_bytes = crate::conductor::resolve_happ_bytes(&app).map_err(err)?;

    let rt = app.edet_runtime().map_err(|e| format!("{e}"))?;

    let payload = InstallAppPayload {
        source:                 AppBundleSource::Bytes(happ_bytes.into()),
        agent_key:              Some(agent),
        installed_app_id:       Some(APP_ID.to_string()),
        network_seed:           None,
        roles_settings:         None,
        ignore_genesis_failure: false,
    };

    let install_resp = runtime::admin_request(
        &rt.conductor,
        AdminRequest::InstallApp(Box::new(payload)),
    )
    .await
    .map_err(err)?;

    match install_resp {
        AdminResponse::AppInstalled(_) => {}
        other => return Err(format!("install_app unexpected response: {other:?}")),
    }

    let enable_resp = runtime::admin_request(
        &rt.conductor,
        AdminRequest::EnableApp { installed_app_id: APP_ID.to_string() },
    )
    .await
    .map_err(err)?;

    match enable_resp {
        AdminResponse::AppEnabled(_) => {}
        other => return Err(format!("enable_app unexpected response: {other:?}")),
    }

    info!("edet installed + enabled under agent");
    Ok(())
}

// ─── App websocket auth ───────────────────────────────────────────────────────

/// Return the app-websocket port and authentication token for `APP_ID`.
///
/// The UI calls this after `startup_state` returns `installed` (existing run)
/// or after `install_app_with_agent_key` (first run / restore path). The
/// returned values open an `AppWebsocket` directly:
///
/// ```ts
/// const { port, token } = await tauriBridge.getAppWebsocketAuth();
/// const client = await AppWebsocket.connect({
///     url: new URL(`ws://localhost:${port}`),
///     token: Uint8Array.from(token),
/// });
/// ```
#[derive(serde::Serialize)]
pub struct AppWebsocketAuth {
    pub port:  u16,
    pub token: Vec<u8>,
}

#[tauri::command]
pub async fn get_app_websocket_auth(app: AppHandle) -> CmdResult<AppWebsocketAuth> {
    wait_for_runtime(&app).await?;
    let rt = app.edet_runtime().map_err(|e| format!("{e}"))?;

    let (port, token) = runtime::get_app_ws_auth(&rt.conductor, rt.app_ws_port)
        .await
        .map_err(err)?;

    Ok(AppWebsocketAuth { port, token })
}

// ─── Backup file I/O ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn export_backup_file(
    app: AppHandle,
    bytes: Vec<u8>,
    suggested_name: String,
) -> CmdResult<Option<String>> {
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
            // Android Storage Access Framework: the file picker returns a
            // content:// URI. tokio::fs cannot open these; use ContentResolver JNI.
            #[cfg(target_os = "android")]
            {
                let url_str = url.to_string();
                tokio::task::spawn_blocking(move || {
                    crate::android_fs::write_content_uri(&url_str, &bytes)
                })
                .await
                .map_err(|e| format!("spawn_blocking: {e}"))?
                .map_err(err)?;
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
                .map_err(|e| format!("spawn_blocking: {e}"))?
                .map_err(err)?;
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

// ─── QR scan ─────────────────────────────────────────────────────────────────

/// Scan a QR code from the default camera.
///
/// On **Android** returns sentinel error `"use-web-camera"` and the Svelte
/// `QrScanner` component falls back to `html5-qrcode` via `getUserMedia`.
///
/// On **desktop** spawns a background thread (nokhwa), streams JPEG frames
/// as `qr://frame` events, and emits `qr://result` on decode.
#[tauri::command]
#[allow(unused_variables)]
pub fn start_qr_scan(app: tauri::AppHandle, state: tauri::State<'_, QrScanState>) -> CmdResult<()> {
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
                if stop.load(Ordering::Relaxed) {
                    break;
                }

                let frame = match camera.frame() {
                    Ok(f) => f,
                    Err(_) => {
                        std::thread::sleep(std::time::Duration::from_millis(33));
                        continue;
                    }
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
                    image::DynamicImage::ImageRgb8(image::imageops::resize(
                        &rgb,
                        640,
                        nh,
                        image::imageops::FilterType::Triangle,
                    ))
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

/// Shared stop-flag slot for the QR scanner background thread.
pub struct QrScanState(
    pub std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
);

impl QrScanState {
    pub fn new() -> Self {
        QrScanState(std::sync::Mutex::new(None))
    }
}
