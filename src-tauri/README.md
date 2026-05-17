# edet Tauri shell

The desktop and Android wrapper for edet. Boots the Holochain conductor and
lair-keystore **in-process** — no subprocesses, no external runtime manager.
The same binary targets desktop (Linux, macOS, Windows) and Android.

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
| `get_app_websocket_auth` | Return the app websocket port + auth token for `AppWebsocket.connect()` |

Command names and argument shapes must stay in sync with the TypeScript
`TauriBridge` contract in `ui/src/common/tauriBridge.ts`.

## Runtime architecture

`plugin::init(config)` spawns `runtime::boot(config)` in the background.
On boot:

1. `ensure_lair_passphrase` reads or generates a 32-byte random passphrase
   at `$APPDATA/edet/.lair-passphrase` (mode `0600` on Unix).
2. `ConductorBuilder` boots the Holochain conductor and lair-keystore
   in-process. Both run as async tasks within the Tauri process — no
   subprocesses are spawned.
3. The plugin emits `holochain://setup-completed` once the conductor is
   accepting connections.
4. The UI calls `get_app_websocket_auth` to retrieve the websocket port
   and auth token, then opens an `AppWebsocket` directly. No URL or
   port is hardcoded in the UI.
5. On app quit the conductor shuts down gracefully (5-second timeout)
   before the process exits.

Admin operations (list apps, install, enable, dump state, graft records)
are dispatched via `AdminInterfaceApi::new(conductor).handle_request()` —
pure in-process function calls with no admin websocket overhead.

### Android Foreground Service

On Android, `tauri-plugin-conductor-service` (in
`plugins/tauri-plugin-conductor-service/`) starts an Android Foreground
Service when the plugin loads. The persistent notification keeps the
process priority high so Android does not kill the conductor when the app
is backgrounded or swiped from recents. The service uses the
`specialUse` foreground service type (no time limit).

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

## Mobile

Android packaging is supported. See [`../README.md`](../README.md) for
setup instructions. The `holochainTauriDev` shell does **not** include
the Android NDK; use `nix develop .#androidDev` for mobile work.

### Android Debugging

To stream both Rust and frontend JavaScript console logs in real time, run this command in a separate terminal:
```bash
nix develop .#androidDev -c bash -c '$ANDROID_HOME/platform-tools/adb logcat | grep -E "TauriWebConsole|RustStdoutStderr"'
```