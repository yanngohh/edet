//! Zome call signing via lair keystore.
//!
//! The WebView's `zome-call-signer.js` invokes `plugin:holochain|sign_zome_call`
//! which routes here. This command serializes and hashes the unsigned zome call
//! params, then asks lair to sign the hash with the agent's private key.

use holochain::prelude::Signature;
use holochain_conductor_api::{ZomeCallParamsSigned};
use holochain_zome_types::prelude::{ExternIO, ZomeCallParams};
use lair_keystore_api::prelude::LairClient;
use tauri::AppHandle;

use crate::plugin::EdetRuntimeExt;

/// Tauri command registered as `plugin:holochain|sign_zome_call`.
///
/// Called by `zome-call-signer.js` in the WebView every time the Holochain
/// JS client needs to submit a zome call.  Returns the signed call bundle
/// that the app websocket then forwards to the conductor.
#[tauri::command]
pub async fn sign_zome_call(
    app: AppHandle,
    zome_call_unsigned: ZomeCallParams,
) -> Result<ZomeCallParamsSigned, String> {
    let rt = app.edet_runtime().map_err(|e| format!("{e}"))?;
    do_sign(zome_call_unsigned, &rt.lair_client)
        .await
        .map_err(|e| format!("{e}"))
}

/// Core signing logic (extracted for testability).
async fn do_sign(
    params: ZomeCallParams,
    client: &LairClient,
) -> anyhow::Result<ZomeCallParamsSigned> {
    // Extract the 32-byte raw pubkey from the 39-byte `AgentPubKey`.
    let pub_key = params.provenance.clone();
    let mut pub_key_32 = [0u8; 32];
    pub_key_32.copy_from_slice(pub_key.get_raw_32());

    // Serialize the call params and produce a Blake2b hash of the bytes.
    let (bytes, bytes_hash) = params.serialize_and_hash()?;

    // Ask lair to sign the hash using the private key corresponding to the
    // agent pubkey. Lair never exposes the private key itself.
    let signature = client
        .sign_by_pub_key(pub_key_32.into(), None, bytes_hash.into())
        .await
        .map_err(|e| anyhow::anyhow!("lair sign_by_pub_key: {e}"))?;

    Ok(ZomeCallParamsSigned {
        bytes:     ExternIO(bytes),
        signature: Signature(*signature.0),
    })
}
