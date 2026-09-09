//! Background mode: keeping the WebView — and therefore the acceptance rule —
//! running while the app is not in front of the member.
//!
//! **The rule signs in the WebView.** `ui/src/lib/autosign.ts` polls the
//! pending pool over `fetch` and signs with the seed held in that JavaScript
//! context; no signature crosses IPC. So the whole of this file is about
//! keeping that page alive, and it holds no logic about what to sign — that
//! stays where the seed is.
//!
//! Two platforms, two different things in the way:
//!
//!   - **Desktop**: the window's close button ends the process. With this
//!     armed, a close is turned into a hide and a tray icon appears, which is
//!     also the only way back to the window and the only way to quit.
//!   - **Android**: `WryActivity.onPause()` pauses the WebView, and a
//!     backgrounded app with no foreground service is a cached process the
//!     system kills at will. `MainActivity` re-resumes the WebView while this
//!     is armed, and the `edet-background` plugin runs the service that keeps
//!     the process off that list.
//!
//! Neither reaches past Doze, and nothing here tries to: an unplugged,
//! stationary device with the screen off has its network suspended for every
//! app outside the battery-optimisation allowlist, foreground service or not.
//! `open_battery_settings` takes the member to the OS screen where that
//! exemption is theirs to grant; the wallet acquires no wake lock (the
//! `WAKE_LOCK` permission in the merged manifest is the notification plugin's,
//! for the scheduled notifications this client does not use).
//!
//! Concrete `Wry` rather than a `Runtime` parameter throughout, because the
//! tray icon is stored in app state and `run()` builds exactly one runtime.

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_os = "android"))]
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[cfg(not(target_os = "android"))]
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder},
};

/// The strings the platform shows, localized by the client that asks for them.
///
/// The member chose a language in the app; a service notification and a tray
/// menu written in English beside it would be the one part of the client that
/// did not listen. So the copy travels with the request rather than living in
/// Android string resources or a Rust constant — six locales, one place
/// (`ui/src/locales/*.json`).
#[derive(Deserialize)]
pub struct Labels {
    /// Tray tooltip; the Android service notification's title.
    pub title: String,
    /// The Android service notification's body.
    pub body: String,
    /// Tray menu: show the window again.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub open: String,
    /// Tray menu: end the app, which also ends the rule.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub quit: String,
}

/// What this device is currently doing about background mode.
#[derive(Default)]
pub struct Mode {
    /// Read by the window-close handler. `false` restores the ordinary close.
    on: AtomicBool,
    /// Held so it can be dropped: dropping the icon is what removes it.
    #[cfg(not(target_os = "android"))]
    tray: Mutex<Option<TrayIcon>>,
}

impl Mode {
    /// Read by the desktop's close handler, and by nothing on Android — there
    /// the flag that matters is the Kotlin side's `BackgroundMode.armed`,
    /// which `MainActivity` asks before re-resuming the WebView.
    #[cfg(not(target_os = "android"))]
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::SeqCst)
    }
}

/// Start keeping this device awake to the pool. Idempotent.
pub fn arm(app: &AppHandle, labels: Labels) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        crate::android_background::arm(app, labels.title, labels.body)?;
        app.state::<Mode>().on.store(true, Ordering::SeqCst);
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    {
        desktop_arm(app, labels)
    }
}

/// Stop. Idempotent, and the ordinary close comes back with it.
pub fn disarm(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<Mode>();
    state.on.store(false, Ordering::SeqCst);
    #[cfg(target_os = "android")]
    {
        crate::android_background::disarm(app)
    }
    #[cfg(not(target_os = "android"))]
    {
        // Drop the icon, and put the window back if it is hidden — a member
        // who turns this off from a hidden window would otherwise be left with
        // an app that has neither a window nor a tray to reach it by.
        let mut tray = state.tray.lock().map_err(|_| "tray lock poisoned".to_string())?;
        *tray = None;
        if let Some(window) = app.get_webview_window("main") {
            if !window.is_visible().unwrap_or(true) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        Ok(())
    }
}

/// What the platform's own chrome is covering, in CSS pixels.
///
/// Zero everywhere but Android: every other platform this client runs on
/// either reports it through `env(safe-area-inset-*)` or has no such chrome,
/// and the page takes the `max()` of the two.
#[derive(Serialize, Default)]
pub struct Insets {
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

pub fn system_insets(app: &AppHandle) -> Result<Insets, String> {
    #[cfg(target_os = "android")]
    {
        let i = crate::android_background::system_insets(app)?;
        Ok(Insets { top: i.top, bottom: i.bottom, left: i.left, right: i.right })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(Insets::default())
    }
}

/// Is this device still going to suspend edet's network under Doze?
///
/// `false` everywhere but Android, where there is no such thing to be subject
/// to — the client asks this to decide whether the member has anything left to
/// do, so "nothing to grant" and "already granted" are the same answer.
pub fn battery_optimised(app: &AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        crate::android_background::battery_optimised(app)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(false)
    }
}

/// Android's battery-optimisation list, where edet can be exempted from Doze.
pub fn open_battery_settings(app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        crate::android_background::open_battery_settings(app)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("battery optimisation is an Android setting".into())
    }
}

#[cfg(not(target_os = "android"))]
fn desktop_arm(app: &AppHandle, labels: Labels) -> Result<(), String> {
    let state = app.state::<Mode>();
    let mut tray = state.tray.lock().map_err(|_| "tray lock poisoned".to_string())?;
    if tray.is_none() {
        *tray = Some(build_tray(app, &labels)?);
    }
    state.on.store(true, Ordering::SeqCst);
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn build_tray(app: &AppHandle, labels: &Labels) -> Result<TrayIcon, String> {
    let open = MenuItem::with_id(app, "edet-open", &labels.open, true, None::<&str>).map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(app, "edet-quit", &labels.quit, true, None::<&str>).map_err(|e| e.to_string())?;
    let menu = Menu::with_items(app, &[&open, &quit]).map_err(|e| e.to_string())?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "this build has no window icon to put in the tray".to_string())?;
    // Two lines: what the icon means, and what it does not. A tray icon with
    // a tooltip that says only "edet" is one a member has to guess at.
    let tooltip = format!("{}\n{}", labels.title, labels.body);
    let builder = TrayIconBuilder::with_id("edet-tray")
        .icon(icon)
        .tooltip(&tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "edet-open" => show_main(app),
            // The tray's quit is the honest one: it ends the process, and with
            // it the rule. Hiding the window did not stop anything; this does.
            "edet-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    // **A missing tray is a panic, not an error.** On Linux the indicator
    // library is loaded with `dlopen` inside a `Lazy` that panics when no
    // `libayatana-appindicator3` / `libappindicator3` is installed — a desktop
    // where this is simply not available. Unwinding it here turns a dead
    // client into a refused arming: `backgroundArmed` stays false in the UI,
    // which is the true statement about what this device will do when it is
    // put down.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| builder.build(app)))
        .map_err(|_| "this desktop has no system tray (no appindicator library)".to_string())?
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "android"))]
fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The window's close button, under background mode.
///
/// A close is a hide while this is armed, and the ordinary close otherwise —
/// so a member who has never turned this on meets exactly the behaviour they
/// always had, and one who has can reach the window and the exit from the
/// tray. Nothing here is a background PROCESS: the page goes on running with
/// its window hidden, which is what keeps the rule deciding.
#[cfg(not(target_os = "android"))]
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        let app = window.app_handle();
        if app.state::<Mode>().is_on() {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}
