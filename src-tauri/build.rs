//! Build script for the Tauri v2 shell.
//!
//! `tauri_build::build()` generates the `tauri.conf.json`-backed context
//! struct and registers the capability manifests under
//! `src-tauri/capabilities/`. Keeping it as a tiny wrapper lets us add
//! pre-build steps later (e.g. embedding the `.happ` file) without breaking
//! the generated context handling.

fn main() {
    tauri_build::build();
}
