//! Holochain conductor + lair-keystore in-process runtime.
//!
//! Boots the conductor directly using `ConductorBuilder` with
//! `KeystoreConfig::LairServerInProc`, then exposes the `ConductorHandle`
//! and `LairClient` directly so all admin operations can be dispatched
//! without an admin websocket.
//!
//! # Admin dispatch
//!
//! All admin operations use `AdminInterfaceApi::new(conductor).handle_request()`
//! — pure in-process function calls, no serialization or network overhead.
//!
//! # App websocket
//!
//! One app websocket interface is attached on a random port during boot.
//! `get_app_ws_auth` issues a non-expiring token for `APP_ID` against that
//! port; the UI uses the port + token to open an `AppWebsocket` directly.
//!
//! # Lair device seed
//!
//! We create a seed under `EDET_DEVICE_SEED_TAG` after boot. This seed's
//! X25519 public key is used as the `recipient` in the crypto_box envelope
//! that `lair_client.import_seed` expects (see `seed.rs`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use holochain::conductor::api::{
    AdminInterfaceApi, AdminRequest, AdminResponse,
};
use holochain::conductor::config::{ConductorConfig, KeystoreConfig};
use holochain::conductor::{ConductorBuilder, ConductorHandle};
use holochain_conductor_api::{
    IssueAppAuthenticationTokenPayload,
};
use holochain_conductor_api::conductor::NetworkConfig;
use holochain_types::websocket::AllowedOrigins;
use lair_keystore_api::prelude::LairClient;
use lair_keystore_api::types::SharedLockedArray;
use sodoken::LockedArray;
use tracing::{error, info};

/// Tag for edet's device seed in lair. Used as the crypto_box recipient
/// in `seed::import_ed25519_seed` — must match exactly what `seed.rs` looks up.
pub const EDET_DEVICE_SEED_TAG: &str = "EDET_DEVICE_SEED";

/// Installed-app id shared between runtime and `conductor::APP_ID`.
const APP_ID: &str = crate::conductor::APP_ID;

/// The running Holochain runtime. Cheap to clone (all fields are Arc/Clone).
#[derive(Clone)]
pub struct EdetRuntime {
    pub conductor: ConductorHandle,
    pub lair_client: LairClient,
    /// Port of the app websocket attached during boot.
    pub app_ws_port: u16,
}

/// Configuration passed to `boot`.
pub struct RuntimeConfig {
    /// Directory where conductor databases and lair keystore are stored.
    pub data_dir: PathBuf,
    /// Locked passphrase for lair.
    pub passphrase: SharedLockedArray,
    /// Kitsune2 network configuration.
    pub network_config: NetworkConfig,
}

/// Convert a `Vec<u8>` passphrase into a `SharedLockedArray` (mlock'd memory).
pub fn vec_to_locked(bytes: Vec<u8>) -> SharedLockedArray {
    let mut locked = LockedArray::new(bytes.len()).expect("sodoken LockedArray alloc failed");
    // lock() returns a mutable guard that derefs to &mut [u8]
    locked.lock().copy_from_slice(&bytes);
    Arc::new(Mutex::new(locked))
}

/// Read the lair passphrase from `{dir}/.lair-passphrase`.
/// Creates and persists a fresh random 32-byte hex passphrase if none exists.
pub fn ensure_lair_passphrase(dir: &Path) -> std::io::Result<Vec<u8>> {
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
    crate::platform::restrict_file_permissions(&path);

    Ok(hex.into_bytes())
}

/// Cross-platform path where conductor data (databases + lair) lives.
/// A separate `holochain-dev` subdir is used in `tauri::is_dev()` mode to
/// prevent dev sessions from colliding with an installed release build.
pub fn holochain_dir() -> PathBuf {
    const APP_INFO: app_dirs2::AppInfo = app_dirs2::AppInfo {
        name: "edet",
        author: "edet contributors",
    };
    let subdir = if tauri::is_dev() { "holochain-dev" } else { "holochain" };
    app_dirs2::app_root(app_dirs2::AppDataType::UserData, &APP_INFO)
        .expect("could not resolve app data root")
        .join(subdir)
}

/// Build the `NetworkConfig` for the conductor.
pub fn network_config() -> NetworkConfig {
    let mut cfg = NetworkConfig::default();
    cfg.bootstrap_url = url2::url2!("https://relay.volla.tech");
    cfg.signal_url   = url2::url2!("wss://relay.volla.tech");
    cfg.webrtc_config = Some(serde_json::json!({
        "iceServers": [
            { "urls": ["stun:stun.nextcloud.com:443"] },
            { "urls": ["stun:stun.l.google.com:19302"] }
        ]
    }));
    // On mobile we participate in gossip but don't serve as a DHT arc host.
    // This reduces bandwidth/storage on devices while still allowing the node
    // to receive entries addressed specifically to it.
    if cfg!(mobile) {
        cfg.target_arc_factor = 0;
    }
    cfg
}

/// Boot the conductor and lair in-process. Returns when the conductor is ready.
///
/// On first run: creates conductor databases + lair keystore.
/// On subsequent runs: opens existing data in `config.data_dir`.
pub async fn boot(config: RuntimeConfig) -> Result<EdetRuntime> {
    // Must be called before any TLS connections (bootstrap / signal servers).
    // The `aws_lc_rs` provider is required on Android where there is no
    // system-wide rustls crypto backend.  `.ok()` suppresses the
    // `AlreadyInstalled` error that fires on hot restarts.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let conductor_config = ConductorConfig {
        data_root_path: Some(config.data_dir.clone().into()),
        // Lair runs in-process. With `lair_root: None` the conductor
        // derives the lair path as `{data_root_path}/lair/`.
        keystore: KeystoreConfig::LairServerInProc { lair_root: None },
        network: config.network_config,
        ..ConductorConfig::default()
    };

    info!("booting conductor at {}", config.data_dir.display());

    // `ConductorBuilder::build()` starts lair in-process, opens or creates
    // all conductor databases, and starts the kitsune2 networking stack.
    let conductor: ConductorHandle = ConductorBuilder::new()
        .passphrase(Some(config.passphrase))
        .config(conductor_config)
        .build()
        .await
        .map_err(|e| anyhow!("conductor build failed: {e}"))?;

    // Create the device seed that `seed::import_ed25519_seed` uses as the
    // crypto_box recipient when importing mnemonic-derived keys.
    ensure_device_seed(&conductor).await?;

    // Grab a direct `LairClient` reference — used by `signer.rs` and `seed.rs`.
    let lair_client = conductor.keystore().lair_client();

    // Attach one app websocket interface on a random port.
    let app_ws_port = attach_app_interface(&conductor).await?;

    info!("conductor ready — app websocket on port {app_ws_port}");
    Ok(EdetRuntime { conductor, lair_client, app_ws_port })
}

/// Gracefully shut down the conductor (with a 5-second timeout).
///
/// Note: kitsune2 networking tasks may continue briefly after this returns —
/// that is an upstream limitation of `holochain 0.6.x`.
pub async fn shutdown(runtime: &EdetRuntime) {
    use tokio::time::Duration;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        runtime
            .conductor
            .shutdown()
            .await
            .map_err(|e| anyhow!("shutdown join: {e}"))?
            .map_err(|e| anyhow!("shutdown conductor: {e}"))
    })
    .await;

    match result {
        Ok(Ok(())) => info!("conductor shut down cleanly"),
        Ok(Err(e)) => error!("conductor shutdown error: {e}"),
        Err(_) => error!("conductor shutdown timed out (5s)"),
    }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Dispatch an admin request to the conductor in-process.
/// No admin websocket — pure Rust function call, no serialization.
pub async fn admin_request(
    conductor: &ConductorHandle,
    req: AdminRequest,
) -> Result<AdminResponse> {
    let api = AdminInterfaceApi::new(conductor.clone());
    api.handle_request(Ok(req))
        .await
        .map_err(|e| anyhow!("admin request failed: {e:?}"))
}

/// Ensure the device seed used as a crypto_box recipient exists in lair.
/// Called once during `boot`; idempotent on subsequent runs.
async fn ensure_device_seed(conductor: &ConductorHandle) -> Result<()> {
    let client = conductor.keystore().lair_client();
    if client.get_entry(EDET_DEVICE_SEED_TAG.into()).await.is_err() {
        client
            .new_seed(EDET_DEVICE_SEED_TAG.into(), None, true)
            .await
            .map_err(|e| anyhow!("create device seed: {e}"))?;
        info!("created device seed in lair under tag '{EDET_DEVICE_SEED_TAG}'");
    }
    Ok(())
}

/// Attach an app websocket interface on a random free port.
/// The port is cached in `EdetRuntime.app_ws_port`.
async fn attach_app_interface(conductor: &ConductorHandle) -> Result<u16> {
    // Check whether an interface is already attached (e.g. after a hot restart
    // where conductor state was persisted). If one exists, reuse its port.
    let list_resp = admin_request(conductor, AdminRequest::ListAppInterfaces).await?;
    if let AdminResponse::AppInterfacesListed(interfaces) = list_resp {
        if let Some(iface) = interfaces.first() {
            info!("reusing existing app interface on port {}", iface.port);
            return Ok(iface.port);
        }
    }

    let attach_resp = admin_request(conductor, AdminRequest::AttachAppInterface {
        port:             None,   // auto-assign a free port
        danger_bind_addr: None,   // bind to localhost (the safe default)
        allowed_origins:  AllowedOrigins::Any,
        installed_app_id: None,   // accept connections for any installed app
    })
    .await?;

    match attach_resp {
        AdminResponse::AppInterfaceAttached { port } => {
            info!("attached app interface on port {port}");
            Ok(port)
        }
        other => Err(anyhow!("unexpected AttachAppInterface response: {other:?}")),
    }
}

/// Issue a non-expiring, multi-use authentication token for `APP_ID`.
/// Used by `commands::get_app_websocket_auth`.
pub async fn get_app_ws_auth(
    conductor: &ConductorHandle,
    app_ws_port: u16,
) -> Result<(u16, Vec<u8>)> {
    let response = admin_request(
        conductor,
        AdminRequest::IssueAppAuthenticationToken(IssueAppAuthenticationTokenPayload {
            installed_app_id: APP_ID.to_string(),
            expiry_seconds:   0,     // 0 = never expires
            single_use:       false, // reusable token
        }),
    )
    .await?;

    match response {
        AdminResponse::AppAuthenticationTokenIssued(info) => Ok((app_ws_port, info.token)),
        other => Err(anyhow!("unexpected IssueAppAuthenticationToken response: {other:?}")),
    }
}
