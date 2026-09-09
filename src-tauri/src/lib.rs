//! edet desktop/mobile client.
//!
//! A signing client, and nothing more. It holds the member's seeds, signs
//! with them, and reads and submits over the HTTP surface of an `edet-node`
//! chosen by the member (`ui/src/lib/networks.ts`). It runs no consensus and
//! stores no ledger.
//!
//! **It embeds no node, and that is the decision worth knowing about.** A
//! handset is a poor validator — the process is reclaimed when the task is
//! swiped, the address roams behind CGNAT, and the store grows without bound
//! while the ledger's size limit is still an open problem — and a client that
//! embedded one would found, on every first run, a private single-validator
//! `edet-dev` chain on published dev keys. Founding is irreversible and there
//! is no merge, so such a chain is a dead end: a device that traded on it
//! holds standing it can never carry anywhere. Validators are charter
//! institutions running `edet-node malachite` headless; this is the wallet.
//!
//! What is left here is what only the platform can do: custody of the
//! identity-vault device key, in the OS keychain (desktop/iOS) or the Android
//! Keystore. Everything else the client needs is HTTP, and the node applies
//! the same viewer rules to it that it applies to a browser
//! (`serve::auth::Viewer`), so this transport has no rule of its own.
//!
//! Trusting one remote node is not the deal: `views::head` publishes the
//! state commitment so a client can put the same question to several nodes
//! and compare, which is why a network is a SET of node URLs rather than one.

#[cfg(target_os = "android")]
mod android_background;
#[cfg(target_os = "android")]
mod android_keystore;
mod background;

// --- OS keychain: custody of the identity-vault device key -----------------
// The UI's vault (ui/src/common/vault.ts) encrypts the Ed25519 seed ring with
// a 32-byte device key. On desktop and iOS that key lives here — the platform
// keychain — which is what makes the vault genuine at-rest protection.
// Keyring calls block, so they run on the blocking pool.
//
// Android: the keyring crate has NO Android Keystore backend, and its
// fallback is a mock store that does not persist — which would strand the
// vault on the second launch. Both commands therefore refuse on Android;
// real Android custody is the `keystore_*` pair below instead (backed by the
// `edet-keystore` Tauri mobile plugin, `src-tauri/gen/android/edet-keystore`),
// with the UI (`ui/src/common/vault.ts`) falling back to the app-sandboxed
// WebView storage only if that plugin errors.

#[cfg(not(target_os = "android"))]
const KEYCHAIN_SERVICE: &str = "org.edet.client";
#[cfg(not(target_os = "android"))]
const KEYCHAIN_ENTRY: &str = "vault-device-key";

/// A device key is 32 bytes, and the wallet sends it as 64 LOWERCASE hex
/// characters — `bytesToHex` (`ui/src/lib/crypto.ts`) emits nothing else, and
/// both custody backends answer in the same spelling. `is_ascii_hexdigit`
/// would admit an uppercase key here that `DeviceKeyStore.set` refuses on
/// arrival, so the contract is stated ONCE and both commands ask it: two
/// layers disagreeing about what a device key looks like is a refusal the
/// member meets one layer late, with the other layer's message.
fn is_device_key_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

#[tauri::command]
async fn keychain_get_device_key() -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        Err("no OS keychain backend on Android".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        tauri::async_runtime::spawn_blocking(|| {
            let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ENTRY).map_err(|e| e.to_string())?;
            match entry.get_password() {
                Ok(hex) => Ok(Some(hex)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

#[tauri::command]
async fn keychain_set_device_key(value: String) -> Result<(), String> {
    if !is_device_key_hex(&value) {
        return Err("device key must be 64 lowercase hex characters".into());
    }
    #[cfg(target_os = "android")]
    {
        Err("no OS keychain backend on Android".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ENTRY).map_err(|e| e.to_string())?;
            entry.set_password(&value).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

// --- Android Keystore: hardware-backed custody of the device key ----------
// Same shape as `keychain_get_device_key` / `keychain_set_device_key` above
// (a hex string in, the same hex string out) so `ui/src/common/vault.ts`
// needs only a new branch, not a new call convention — see the doc comment
// on `android_keystore` and the paper's §Implementation.
// Defined unconditionally (like the keychain commands) so
// `generate_handler!` below doesn't need a cfg-gated variant; only the
// Android body does real work — the non-Android branch symmetrically
// returns `Err` here, mirroring how the keychain commands' Android branch
// always returns `Err` (there is no Keystore to wrap into off Android).

#[tauri::command]
async fn keystore_get_device_key(app: tauri::AppHandle) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        // `run_mobile_plugin` blocks on a channel recv() (see
        // `android_keystore`'s doc comment), so — like the keychain calls
        // above — it runs on the blocking pool rather than the async
        // executor thread.
        tauri::async_runtime::spawn_blocking(move || android_keystore::get_device_key(&app))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Android Keystore is only available on Android".into())
    }
}

#[tauri::command]
async fn keystore_set_device_key(app: tauri::AppHandle, value: String) -> Result<(), String> {
    if !is_device_key_hex(&value) {
        return Err("device key must be 64 lowercase hex characters".into());
    }
    #[cfg(target_os = "android")]
    {
        tauri::async_runtime::spawn_blocking(move || android_keystore::set_device_key(&app, value))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, value);
        Err("Android Keystore is only available on Android".into())
    }
}

// --- background mode: the rule going on deciding while the app is behind ----
// The acceptance rule signs in the WebView (`ui/src/lib/autosign.ts`), so
// every one of these commands is about keeping that page alive — a tray on
// the desktop, a foreground service and a re-resumed WebView on Android. None
// of them can sign, and none of them survives the process: see
// `background.rs`.

#[tauri::command]
async fn background_arm(app: tauri::AppHandle, labels: background::Labels) -> Result<(), String> {
    background::arm(&app, labels)
}

#[tauri::command]
async fn background_disarm(app: tauri::AppHandle) -> Result<(), String> {
    background::disarm(&app)
}

#[tauri::command]
async fn background_system_insets(app: tauri::AppHandle) -> Result<background::Insets, String> {
    background::system_insets(&app)
}

#[tauri::command]
async fn background_battery_optimised(app: tauri::AppHandle) -> Result<bool, String> {
    background::battery_optimised(&app)
}

#[tauri::command]
async fn background_open_battery_settings(app: tauri::AppHandle) -> Result<(), String> {
    background::open_battery_settings(&app)
}

// Required by Tauri on mobile: Android (and iOS) launch straight into the
// native library via JNI rather than through `main.rs`'s `fn main`, so
// `run()` needs to be marked as the mobile entry point. This is the one bit
// of boilerplate `tauri android init` would otherwise add on its own; adding
// it here doesn't require the Android toolchain (the attribute is a no-op
// off mobile) and is a prerequisite for the app to launch on Android at all,
// keystore plugin aside.
#[cfg_attr(any(target_os = "android", target_os = "ios"), tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android_keystore::init()).plugin(android_background::init());
    // The close button is a HIDE while background mode is armed, and the
    // ordinary close otherwise. Registered unconditionally rather than behind
    // the arming, because a handler cannot be added to a window after the fact
    // and the state it reads is the one the JS sets.
    #[cfg(not(target_os = "android"))]
    let builder = builder.on_window_event(background::on_window_event);
    builder
        .setup(move |_app| {
            // Background mode's own state: whether the member asked for it,
            // and the tray icon that is the only way back to a hidden window.
            tauri::Manager::manage(_app, background::Mode::default());
            // Nothing to start, and nothing may block here: `setup` runs on
            // the main thread while the WebView is already loading, so a
            // `block_on` in this hook is the window in which an `invoke` is
            // issued and never answered — a client spinning forever. There is
            // no engine, and so nothing to wait for.
            //
            // Desktop Linux: wry wires no WebKit permission handling at all,
            // so getUserMedia — the QR scanner — is refused before the user
            // is ever asked (NotAllowedError). Enable media streams and
            // allow the one request this app's own UI makes: camera, no
            // audio. Anything else keeps WebKit's default deny. Android
            // needs none of this — wry's WebChromeClient there already asks
            // the OS, against the CAMERA permission the manifest declares.
            #[cfg(target_os = "linux")]
            if let Some(window) = tauri::Manager::get_webview_window(_app, "main") {
                let _ = window.with_webview(|webview| {
                    use webkit2gtk::glib::object::Cast;
                    use webkit2gtk::{
                        PermissionRequestExt, SettingsExt, UserMediaPermissionRequest, UserMediaPermissionRequestExt,
                        WebViewExt,
                    };
                    let view = webview.inner();
                    if let Some(settings) = WebViewExt::settings(&view) {
                        settings.set_enable_media_stream(true);
                    }
                    view.connect_permission_request(|_, request| {
                        if let Some(media) = request.dynamic_cast_ref::<UserMediaPermissionRequest>() {
                            if media.is_for_video_device() && !media.is_for_audio_device() {
                                media.allow();
                                return true;
                            }
                        }
                        false
                    });
                });
            }
            Ok(())
        })
        // Custody only. Every read and every submit is HTTP now, against the
        // node the member picked — the same routes, and the same viewer
        // rules, the browser client uses.
        .invoke_handler(tauri::generate_handler![
            keychain_get_device_key,
            keychain_set_device_key,
            keystore_get_device_key,
            keystore_set_device_key,
            background_arm,
            background_disarm,
            background_battery_optimised,
            background_system_insets,
            background_open_battery_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running the edet client");
}
