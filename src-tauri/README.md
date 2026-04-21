# edet Tauri shell

The desktop wrapper for edet. Uses
[`tauri-plugin-holochain`](https://github.com/darksoil-studio/tauri-plugin-holochain)
to embed the Holochain conductor and lair-keystore **in-process** — there
are no subprocesses to manage. The same binary targets desktop (Linux,
macOS, Windows) and Android.

## IPC surface

The shell exposes a narrow command surface to the Svelte UI:

| Command | Purpose |
|---|---|
| `is_tauri` | Detect whether the UI is running inside the Tauri shell |
| `startup_state` | Check if an edet identity is installed (`fresh` / `installed` / `error`) |
| `dump_source_chain` | Snapshot the local cell for the backup bundle |
| `graft_source_chain` | Restore a previously-saved chain onto a fresh cell |
| `import_lair_seed` | Seed lair with a mnemonic-derived ed25519 seed |
| `install_app_with_agent_key` | Install the edet happ under a specific `AgentPubKey` |
| `export_backup_file` | Native save dialog for a `.edet-backup` file |
| `import_backup_file` | Native open dialog for a `.edet-backup` file |
| `get_current_backup` / `save_current_backup` | Atomic disk ops against `$APPDATA/edet/backup.edet-backup` |

Command names and argument shapes must stay in sync with the TypeScript
`TauriBridge` contract in `ui/src/common/tauriBridge.ts`.

## Runtime architecture

`tauri-plugin-holochain::async_init` starts the Holochain runtime
in-process in a background thread. On boot:

1. `ensure_lair_passphrase` reads or generates a 32-byte random passphrase
   at `$APPDATA/edet/.lair-passphrase` (mode `0600` on Unix).
2. The plugin initialises lair-keystore and the Holochain conductor using
   that passphrase — both run as in-process tasks, not subprocesses.
3. The plugin emits `holochain://setup-completed` once the conductor is
   accepting connections.
4. The plugin injects `window.__HC_LAUNCHER_ENV__ = { APP_INTERFACE_PORT,
   APP_INTERFACE_TOKEN }` into the WebView. The UI's `AppWebsocket.connect()`
   call reads these globals automatically — no URL or port is hardcoded in
   the UI.
5. On app quit the plugin shuts down the conductor gracefully (disabling
   apps so peers learn we've left) before the process exits.

Data is stored under:

| Mode | Path |
|---|---|
| Dev (`tauri dev`) | `$APPDATA/edet/holochain-dev/` |
| Release | `$APPDATA/edet/holochain/` |

The separation prevents a dev session from corrupting a release install's
keystore or source chain.

## Development

**Required shell:** `nix develop .#holochainTauriDev`

The default `nix develop` shell provides the DNA / hc-spin toolchain but
lacks the native Tauri libraries (`libclang`, `webkitgtk_4_1`, `gtk3`,
`libsoup`, `cmake` for `datachannel-sys`). Always use the dedicated shell
when compiling `src-tauri/`.

```bash
# 1. Enter the Tauri + Holochain native-deps shell
nix develop .#holochainTauriDev

# 2. Install Node dependencies
npm install

# 3. Start the dev build
#    - Builds the .happ bundle (WASM zomes + hc app pack)
#    - Starts the Vite dev server on :8888
#    - Compiles and launches the Tauri binary with hot-reload
npm run tauri:dev
```

The `tauri:dev` script runs `build:happ` automatically before `tauri dev`.
If you change zome code mid-session, stop the build, run
`npm run build:happ`, then restart `npm run tauri:dev`.

### Cargo check (without full compile)

```bash
# Inside nix develop .#holochainTauriDev:
cargo check --manifest-path src-tauri/Cargo.toml
```

Note: `src-tauri` is excluded from the workspace `Cargo.toml` at the
repo root. Running `cargo build` from the repo root builds only the WASM
zomes. Run `cargo check`/`build` from inside `src-tauri/` or via
`tauri dev` / `tauri build`.

### Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `LAIR_KEYSTORE_DISABLE_MLOCK` | unset | Set to `1` on systems without `RLIMIT_MEMLOCK` (many Linux VMs). The `holochainTauriDev` Nix shell sets this automatically. |
| `RUST_LOG` | `info` | Controls `tracing` log verbosity (e.g. `holochain=debug`). |
| `WASM_LOG` | unset | Controls in-WASM log level (e.g. `[wasm_trace]=debug`). |
| `EDET_HAPP_PATH` | auto-resolved | Override the `.happ` file location. Without this, the conductor resolves it via: Tauri resource dir → `$CWD/workdir/edet.happ`. |

## Version alignment

The Rust deps are pinned to the 0.6.x Holochain line that matches our DNA's
`hdi = 0.7.0` / `hdk = 0.6.0`. If you bump the DNA you must bump the
following in lockstep:

| Crate | Version | Notes |
|---|---|---|
| `holochain_client` | `=0.8.1-rc.8` | Provides `install_app`, `dump_full_state`, `graft_records` |
| `holochain_types` | `=0.6.1-rc.8` | |
| `holochain_conductor_api` | `=0.6.1-rc.8` | |
| `holochain_zome_types` | `=0.6.1-rc.5` | |
| `holo_hash` | `=0.6.1-rc.5` | |
| `lair_keystore_api` | `=0.6.3` | Caller-transparent X25519 `import_seed` |
| `crypto_box` | `0.9` | NaCl envelope for lair seed import |
| `sodoken` | `0.1` | `SharedLockedArray` for lair passphrase |

## Packaging

```bash
# Inside nix develop .#holochainTauriDev:
npm run tauri:build
```

Produces distribution artefacts under `src-tauri/target/release/bundle/`:

| Host | Produces |
|---|---|
| Linux | `.deb` (Debian/Ubuntu) and `.rpm` (Fedora/RHEL) |
| macOS | `.dmg` |
| Windows | `.msi` |

Linux targets are verified on Fedora 43 inside `nix develop .#holochainTauriDev`.
macOS and Windows bundles need a native build host.

### AppImage (opt-in)

AppImage bundling is disabled by default because Tauri's bundled
`linuxdeploy` walks the shared-library graph and needs every transitive
dependency resolvable from `LD_LIBRARY_PATH` — including glibc-adjacent
libraries that can cause ABI mismatches in the Nix environment.

```bash
npx tauri build --bundles appimage
```

## Mobile

Android packaging is experimental. See [`../plans/android.md`](../plans/android.md)
for the full plan and [`../README.md`](../README.md) for setup instructions.
The `holochainTauriDev` shell does **not** include the Android NDK; use
`nix develop .#androidDev` for mobile work.
