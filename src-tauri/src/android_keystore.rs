//! Android Keystore-backed custody for the identity-vault device key.
//!
//! Only ever compiled for `target_os = "android"` (see the `mod` declaration
//! in `lib.rs`, gated the same way `keyring`'s desktop dependency is gated in
//! `Cargo.toml`) — this file does not exist as far as the default desktop
//! build is concerned.
//!
//! This registers the Kotlin `KeystorePlugin`
//! (`src-tauri/gen/android/edet-keystore`) as a Tauri mobile plugin and gives
//! the two `keystore_get_device_key` / `keystore_set_device_key` IPC commands
//! in `lib.rs` a thin, typed way to call into it. See
//! the paper's §Implementation for the design.
//!
//! UNVERIFIED: this sandbox has no Android SDK/NDK/device, so none of this
//! (nor the Kotlin side) has been compiled or run. Written and cross-checked
//! against the `tauri` 2.11 source (`src/plugin/mobile.rs`) rather than
//! executed.

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime};

/// Android package/class the Kotlin plugin lives under
/// (`src-tauri/gen/android/edet-keystore/src/main/java/org/edet/client/keystore/KeystorePlugin.kt`).
const ANDROID_PACKAGE: &str = "org.edet.client.keystore";
const ANDROID_CLASS: &str = "KeystorePlugin";

/// **A newtype, and it has to be.**
///
/// App state is keyed by TYPE, and `register_android_plugin` hands every
/// plugin the same `PluginHandle<R>`. Two plugins managing the bare handle
/// would find the second `manage` a no-op and every later call would reach
/// the FIRST plugin's Kotlin class — a defect no signature and no gate here
/// would show. Background mode's handle is wrapped the same way
/// (`android_background::BackgroundHandle`).
struct KeystoreHandle<R: Runtime>(PluginHandle<R>);

#[derive(Serialize)]
struct SetDeviceKeyArgs {
    hex: String,
}

#[derive(Deserialize)]
struct GetDeviceKeyResponse {
    hex: Option<String>,
}

/// Registers the Kotlin plugin and stashes its `PluginHandle` in app state.
/// Call `.plugin(android_keystore::init())` on the `tauri::Builder` before
/// `.run()` (see `lib.rs::run`, android-only branch).
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("edet-keystore")
        .setup(|app, api| {
            let handle = api.register_android_plugin(ANDROID_PACKAGE, ANDROID_CLASS)?;
            app.manage(KeystoreHandle(handle));
            Ok(())
        })
        .build()
}

/// Reads the wrapped device key back from the Keystore-backed
/// `EncryptedSharedPreferences`, or `None` if none has been provisioned yet.
pub fn get_device_key<R: Runtime>(app: &AppHandle<R>) -> Result<Option<String>, String> {
    let handle = app.state::<KeystoreHandle<R>>();
    let resp: GetDeviceKeyResponse = handle.0.run_mobile_plugin("getDeviceKey", ()).map_err(|e| e.to_string())?;
    Ok(resp.hex)
}

/// Wraps `hex` (the UI's existing or freshly generated 32-byte device key,
/// already hex-encoded) and persists only the wrapped blob. Caller
/// (`lib.rs::keystore_set_device_key`) has already applied `is_device_key_hex`,
/// the one predicate `keychain_set_device_key` asks too; the Kotlin side
/// re-checks it (defense in depth, see `KeystorePlugin.kt`), and both refuse
/// the same 64 lowercase hex characters.
pub fn set_device_key<R: Runtime>(app: &AppHandle<R>, hex: String) -> Result<(), String> {
    let handle = app.state::<KeystoreHandle<R>>();
    // The plugin resolves with no payload (`invoke.resolve()`, serialized as
    // the JSON literal `null`) on success, so `()` is the expected response
    // type here — mirrors `PluginResult`'s no-arg `resolve()` on the Kotlin
    // side, see `KeystorePlugin.kt::setDeviceKey`.
    handle
        .0
        .run_mobile_plugin("setDeviceKey", SetDeviceKeyArgs { hex })
        .map_err(|e| e.to_string())
}
