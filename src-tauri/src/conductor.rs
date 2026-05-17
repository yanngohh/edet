//! Post-plugin conductor helpers.
//!
//! The conductor lifecycle is managed by `runtime.rs` (boot/shutdown)
//! and `plugin.rs` (Tauri integration). What remains here is:
//!
//! - `APP_ID` — the installed_app_id edet is always installed under.
//!   Centralised so `commands.rs` and any other caller use the same string.
//! - `records_from_full_state_dump` — pure transformation from the
//!   admin API's `FullStateDump` into a `Vec<Record>` suitable for the
//!   backup bundle. The function uses zero I/O.
//! - `resolve_happ_bytes` — locate or embed the `.happ` bytes, either
//!   at compile time (Android `include_bytes!`) or from disk (desktop).

use std::path::PathBuf;

use anyhow::{bail, Result};
use holo_hash::AgentPubKey;
use holochain_zome_types::prelude::{CellId, Record, SignedActionHashed, SignedHashed};
use tauri::AppHandle;

/// Installed-app id. All admin-level bookkeeping (install, enable,
/// dump, graft) uses this fixed string, so `list_apps` output is
/// deterministic across restarts.
pub const APP_ID: &str = "edet";

/// Extract the authored records from a `FullStateDump` for the backup
/// bundle. The admin websocket's `dump_full_state` returns rich
/// metadata (DHT ops, validation, etc.); we only want the authored
/// records.
///
/// `SourceChainDumpRecord` exposes the three pieces we need
/// (`signature`, `action`, `entry`); reconstruct the `Record` by
/// pairing the signed action hash with the entry wrapper.
pub fn records_from_full_state_dump(
    dump: holochain_conductor_api::FullStateDump,
) -> Vec<Record> {
    dump.source_chain_dump
        .records
        .into_iter()
        .map(|r| {
            // Build HoloHashed<Action> from the precomputed action hash.
            let hashed = holo_hash::HoloHashed::with_pre_hashed(r.action, r.action_address);
            let signed: SignedActionHashed = SignedHashed {
                hashed,
                signature: r.signature,
            };
            // `Record::new` computes the correct `RecordEntry` variant based
            // on the action's entry visibility, so we pass through the
            // `Option<Entry>` directly.
            Record::new(signed, r.entry)
        })
        .collect()
}

/// Convenience constructor used by callers that already have the two halves.
#[allow(dead_code)]
pub fn cell_id(
    dna: holo_hash::DnaHash,
    agent: AgentPubKey,
) -> CellId {
    CellId::new(dna, agent)
}

/// On Android, `resource_dir()` returns the virtual WebView URL
/// `asset://localhost/` which Rust's filesystem layer cannot access.
/// We embed the hApp bytes at compile time instead, so no runtime I/O
/// or Android AssetManager JNI is needed.
#[cfg(mobile)]
static HAPP_BYTES: &[u8] = include_bytes!("../../workdir/edet.happ");

/// Return the raw bytes of the `.happ` bundle.
///
/// On **Android** the bytes are embedded at compile time via
/// `include_bytes!` because `resource_dir()` returns the virtual
/// `asset://localhost/` URL that Rust's `std::fs` layer cannot open.
///
/// On **desktop** the bytes are read from disk using `resolve_happ_path`.
#[allow(unused_variables)]
pub fn resolve_happ_bytes(app: &AppHandle) -> Result<Vec<u8>> {
    #[cfg(mobile)]
    return Ok(HAPP_BYTES.to_vec());

    #[cfg(not(mobile))]
    {
        let path = resolve_happ_path(app)?;
        std::fs::read(&path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))
    }
}

/// Locate the `.happ` bundle on disk (desktop only).
///
/// Resolution order:
///   1. `EDET_HAPP_PATH` env var (explicit override — useful for
///      development or for pointing an installer at a staged build).
///   2. Tauri bundled resource — resolves to the `bundle.resources`
///      entry in `tauri.conf.json`.
///   3. `$CWD/workdir/edet.happ` — fallback for `npm run tauri:dev`
///      where the resource dir may not have the bundle staged.
///
/// On Android use `resolve_happ_bytes` instead; `resource_dir()` there
/// returns a virtual `asset://` URL that `Path::exists()` cannot probe.
pub fn resolve_happ_path(app: &AppHandle) -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("EDET_HAPP_PATH") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Ok(path);
        }
        bail!("EDET_HAPP_PATH is set but {path:?} does not exist");
    }

    // Tauri's PathResolver returns the platform-specific resource dir.
    // In `tauri dev` the resource declared as `"../workdir/edet.happ"` in
    // tauri.conf.json is copied into the debug target tree as:
    //   <resource_dir>/_up_/workdir/edet.happ
    // (Tauri encodes `../` as `_up_/` to keep paths inside the target dir.)
    // In a release bundle it lands directly at:
    //   <resource_dir>/edet.happ  or  <resource_dir>/workdir/edet.happ
    if let Ok(resource_dir) = tauri::Manager::path(app).resource_dir() {
        for candidate in [
            resource_dir.join("_up_").join("workdir").join("edet.happ"), // tauri dev
            resource_dir.join("edet.happ"),                               // release flat
            resource_dir.join("workdir").join("edet.happ"),               // release subdir
        ] {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dev = cwd.join("workdir").join("edet.happ");
    if dev.exists() {
        return Ok(dev);
    }

    bail!(
        "could not find edet.happ — set EDET_HAPP_PATH or ship it as a Tauri resource"
    );
}
