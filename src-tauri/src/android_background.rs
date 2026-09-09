//! The Android half of background mode: a foreground service, and the WebView
//! that keeps running behind it.
//!
//! Only ever compiled for `target_os = "android"` (see the `mod` declaration
//! in `lib.rs`, gated the way `android_keystore` is). It registers the Kotlin
//! `BackgroundPlugin` (`src-tauri/gen/android/edet-background`) and gives
//! `background.rs` a typed way to call the three commands it has.
//!
//! **The service holds no logic and cannot sign.** It is a process priority
//! and a permanent notification: what decides is the WebView, which
//! `MainActivity` re-resumes after wry's `onPause` pauses it, and which is
//! gone the moment the process is. That is why the service is
//! `START_NOT_STICKY` — a restarted service with no WebView would be a
//! notification claiming a rule that is not running.
//!
//! UNVERIFIED here: this sandbox has no Android SDK/NDK, so neither this nor
//! the Kotlin side is compiled by any gate that runs on it. Written against
//! the same `tauri` 2.11 plugin API `android_keystore` uses.

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime};

/// Android package/class the Kotlin plugin lives under
/// (`src-tauri/gen/android/edet-background/src/main/java/org/edet/client/background/BackgroundPlugin.kt`).
const ANDROID_PACKAGE: &str = "org.edet.client.background";
const ANDROID_CLASS: &str = "BackgroundPlugin";

/// **A newtype, and it has to be.**
///
/// `PluginHandle<R>` is what `register_android_plugin` returns, and app state
/// is keyed by TYPE: a second plugin managing the bare handle would find
/// `manage` a no-op and every later call would reach the first plugin's
/// Kotlin class. Custody's handle is wrapped for the same reason
/// (`android_keystore::KeystoreHandle`), so the next plugin added here
/// inherits the rule rather than the defect.
struct BackgroundHandle<R: Runtime>(PluginHandle<R>);

#[derive(Deserialize)]
struct BatteryOptimisedResponse {
    optimised: bool,
}

/// System-bar and cutout insets, in CSS pixels.
#[derive(Deserialize, Serialize, Default)]
pub struct Insets {
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

#[derive(Serialize)]
struct ArmArgs {
    title: String,
    body: String,
}

/// Registers the Kotlin plugin and stashes its handle in app state.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("edet-background")
        .setup(|app, api| {
            let handle = api.register_android_plugin(ANDROID_PACKAGE, ANDROID_CLASS)?;
            app.manage(BackgroundHandle(handle));
            Ok(())
        })
        .build()
}

/// Start the foreground service, with the notification text the client
/// localized. Idempotent on the Kotlin side.
pub fn arm<R: Runtime>(app: &AppHandle<R>, title: String, body: String) -> Result<(), String> {
    let handle = app.state::<BackgroundHandle<R>>();
    handle
        .0
        .run_mobile_plugin::<()>("arm", ArmArgs { title, body })
        .map_err(|e| e.to_string())
}

/// Stop it. The WebView pauses again with the next `onPause`.
pub fn disarm<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let handle = app.state::<BackgroundHandle<R>>();
    handle.0.run_mobile_plugin::<()>("disarm", ()).map_err(|e| e.to_string())
}

/// Is this app still subject to Doze, or has the member exempted it?
///
/// A read with no permission behind it, which is what lets the client ask
/// before it briefs anybody: a wallet that asks for something already granted
/// is one whose prompts get dismissed unread.
pub fn battery_optimised<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    let handle = app.state::<BackgroundHandle<R>>();
    let resp: BatteryOptimisedResponse =
        handle.0.run_mobile_plugin("batteryOptimised", ()).map_err(|e| e.to_string())?;
    Ok(resp.optimised)
}

/// What the system bars are covering, so the page can keep its own controls
/// out from under them.
///
/// Android reports the safe area from the display CUTOUT and never from the
/// navigation bar, so `env(safe-area-inset-bottom)` is 0 on a phone whose
/// buttons are sitting on top of the page — measured, on a handset.
/// `WindowInsetsCompat` is the only thing that knows.
pub fn system_insets<R: Runtime>(app: &AppHandle<R>) -> Result<Insets, String> {
    let handle = app.state::<BackgroundHandle<R>>();
    handle.0.run_mobile_plugin("systemInsets", ()).map_err(|e| e.to_string())
}

/// Open the OS list where an app can be exempted from battery optimisation.
///
/// The exemption is the member's to grant, in Android's own screen; the app
/// asks for no wake lock and holds no permission that would let it grant
/// itself one.
pub fn open_battery_settings<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let handle = app.state::<BackgroundHandle<R>>();
    handle
        .0
        .run_mobile_plugin::<()>("openBatterySettings", ())
        .map_err(|e| e.to_string())
}
