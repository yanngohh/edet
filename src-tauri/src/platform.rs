//! Platform-specific configuration.
//!
//! All `#[cfg(target_os = ...)]` and `#[cfg(unix)]` gates live here so
//! `lib.rs` stays readable and cross-platform logic is not scattered.
//!
//! Each public function has a no-op fallback so callers need no cfg guards.

/// Configure the WebView after a page load.
///
/// Currently a no-op on all platforms — no WebView-level media or
/// permission configuration is needed because camera access goes through
/// the native `scan_qr_code` Tauri command (nokhwa) rather than
/// WebKitGTK's getUserMedia pipeline.
///
/// Kept as an extension point for future platform-specific WebView needs.
#[allow(unused_variables)]
pub fn configure_webview_on_load(_webview_window: &tauri::WebviewWindow) {}

/// Set the file-mode permission on a path to owner-read/write only.
///
/// No-op on non-Unix platforms (Windows uses ACL-based security instead).
#[allow(unused_variables)]
pub fn restrict_file_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}
