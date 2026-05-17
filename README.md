# edet

edet is a decentralized mutual credit protocol built on Holochain. Instead of transferring tokens, buying creates a debt obligation: when you buy something, you owe the equivalent value to your seller, which you repay by selling goods or services to anyone in your network. Debt propagates forward through the economy rather than money flowing backward.

## Why edet is different

**vs. traditional mutual credit (Circles, Trustlines, Sardex)**
Existing systems require bilateral credit limits negotiated in advance between
every pair of trading partners. edet replaces bilateral limits with a
network-wide EigenTrust reputation score. Your credit capacity is derived from
what the network collectively thinks of you, not from individual handshake
agreements — you can trade with strangers without prior negotiation.

**vs. token-based DeFi (DAI, USDC, algorithmic stablecoins)**
Tokens have a speculative premium: holders accumulate them hoping they
appreciate, which creates hoarding and deflationary pressure. edet has no
token to hoard. Debt obligations expire (30-day maturity), so the only
rational strategy is to participate — buy, sell, repay. The medium of exchange
has no speculative value because it exists only as an obligation that must be
discharged.

**vs. reputation-gated lending (TrustCredo, Karma)**
Most reputation lending systems have a central authority computing
creditworthiness. edet is fully subjective: every agent runs their own
EigenTrust computation over their local acquaintance subgraph. There is no
global credit score, no central oracle, no issuer. Two observers can disagree
on your capacity — that is by design.

**vs. Holochain mutual credit (HoloFuel, RedGrid)**
HoloFuel is pure accounting with no reputation-gated capacity. edet adds a
formal reputation layer — EigenTrust power iteration, satisfaction/failure
counters, trust attenuation, and contagion propagation — that bounds how much
debt a newcomer or bad actor can accumulate before the network excludes them.

## Key properties

- **Spend what you earn, not what you buy** — edet doesn't use tokens or coins. Your ability to buy things is a direct reflection of your reputation as an honest trader.
- **Buying creates a debt, selling pays it back** — When you buy something, you don't "lose" money; you gain a debt. You clear this debt by selling your own services or products to others in the network.
- **Trust flows through your network** — There is no central bank or credit bureau. Your spending limit is determined by the trust your local neighbors have in you, which spreads through the network like a web.
- **Start small, grow fast** — New accounts start with a small, safe "trial" limit. As you complete trades and follow through on your obligations, the network automatically increases your capacity to match your activity.
- **Proven history, not fake accounts** — Creating thousands of fake accounts doesn't help an attacker. The protocol requires newcomers to prove themselves through small, successful transactions before they can access significant credit.
- **No fees, no middlemen** — Transactions are approved by you and your peers directly. There are no gas fees, no miners, and no central authority taking a cut.

## Repository layout

```
dnas/edet/
  zomes/
    integrity/transaction/   # Entry types, link types, validation rules
    coordinator/transaction/  # EigenTrust, risk score, cascade, capacity
doc/
  edet.pdf                    # Whitepaper with formal proofs
  edet.tex                    # LaTeX source
sim/                          # Python simulation + theorem verification
  main.py                     # Run a full N-agent simulation
  verify_theory.py            # Verify all 13 security theorems
  config.py                   # Protocol parameters (aligned with constants.rs)
src-tauri/                    # Tauri v2 shell (self-hosted in-process conductor
                              # + lair-keystore for desktop + Android)
tests/sweettest/              # Rust integration tests (134 tests)
ui/                           # Svelte frontend
  src/edet/onboarding/        # First-run wizard + mnemonic + backup export
  src/common/                 # mnemonic.ts, backup.ts, backup storage, Tauri bridge
workdir/                      # Packaged .happ and .webhapp outputs
```

## Getting started

> **Prerequisite:** set up the
> [Holochain development environment](https://developer.holochain.org/docs/install/).

Enter the nix shell from the repository root:

```bash
nix develop
npm install
```

All commands below must be run inside this nix shell.

## Running locally

Start a 2-agent network with UI and Holochain Playground:

```bash
npm run start
```

To run more agents:

```bash
AGENTS=5 npm run network
```

## Running tests

```bash
npm run test
```

Builds the test DNA (integrity zome compiled with `test-epoch` feature for
accelerated epoch timing) then runs all 134 integration tests via
`cargo nextest`. Expected output: `134 tests run: 134 passed`.

## Running simulations

```bash
cd sim
python3 main.py          # simulate N=1000 agents, 100 epochs
python3 verify_theory.py # verify all 13 whitepaper security theorems
```

Simulation parameters are in `sim/config.py` and are kept in sync with the
protocol constants in
`dnas/edet/zomes/integrity/transaction/src/types/constants.rs`.

## Building and packaging

Build the production hApp:

```bash
npm run build:happ
```

Package as a distributable `.webhapp` for the legacy Holochain Launcher:

```bash
npm run package
```

Output is written to `workdir/edet.webhapp`.

### Desktop application (Tauri v2)

The desktop shell boots the Holochain conductor and lair-keystore
**in-process** — no subprocesses, no external runtime manager. The same
binary targets desktop (Linux, macOS, Windows) and Android.

Building `src-tauri/` needs additional native libraries (libclang,
WebKit/GTK, cmake for datachannel, etc.) that are **not** in the default
`nix develop` shell. You must use the dedicated `holochainTauriDev` shell:

```bash
# 1. Enter the Tauri + Holochain native-deps shell
nix develop .#holochainTauriDev

# 2. Install Node dependencies (first time or after package.json changes)
npm install

# 3. Start the dev build
#    This automatically builds the .happ bundle first, starts the Vite
#    dev server on :8888, then compiles and launches the Tauri binary.
#    The embedded conductor boots in-process; no separate holochain
#    process is needed.
npm run tauri:dev

# 4. Release bundle (deb/rpm on Linux, dmg on macOS, msi on Windows)
npm run tauri:build
```

> **Important:** `npm run tauri:dev` now runs `build:happ` automatically
> before launching. If you change zome code during a session, stop the
> dev build, run `npm run build:happ`, then restart `npm run tauri:dev`
> to pick up the updated `.happ`.

> **Do not use `nix develop` (the default shell) for `tauri:dev`.** The
> default shell provides the DNA/hc-spin toolchain but lacks the native
> Tauri libraries. Only switch to `holochainTauriDev` when compiling the
> Tauri crate.

#### `zome-call-signer.js`

`src-tauri/zome-call-signer.js` is bundled in-tree. It sets
`window.__HC_ZOME_CALL_SIGNER__` in the WebView so that `AppWebsocket`
routes zome-call signing through lair via the `sign_zome_call` Tauri
command (registered under the `holochain` plugin namespace in
`src-tauri/src/signer.rs`). No external sync is needed.

#### Wipe stale conductor data

The embedded Holochain runtime stores its databases and lair keystore under
`~/.local/share/edet/holochain-dev/` (dev builds) or
`~/.local/share/edet/holochain/` (release builds). If you see errors like
*"app not found"*, *"connection token was issued for an app that was not found"*,
or lair import failures after wiping the app and reinstalling, the stale
conductor data is likely the cause. Delete it and restart:

```bash
# Dev data (tauri:dev)
rm -rf ~/.local/share/edet/holochain-dev/

# Release data (tauri:build / installed app)
rm -rf ~/.local/share/edet/holochain/
```

> **Note:** this deletes your local agent key and source chain. Any
> transactions or reputation not captured in a backup file will be lost.
> Export a backup first if you have data worth keeping.

The two development flows use **different shells** for a reason:

| Task | Shell | Command |
|------|-------|---------|
| Tauri desktop app | `nix develop .#holochainTauriDev` | `npm run tauri:dev` |
| Multi-agent browser dev | `nix develop` | `npm run start` |
| Integration tests | `nix develop` | `npm run test` |
| DNA / zome work | `nix develop` | `npm run build:happ` |

See `src-tauri/README.md` for the IPC surface and runtime architecture.

## Android

The Tauri + Holochain integration targets Android.

### Prerequisites

- A physical Android device (API 26 / Android 8.0 or newer) or an
  emulator image.
- The `androidDev` nix shell (see below) — it bundles the Android NDK,
  JDK 17, and all four Rust Android targets. First entry takes ~10 min
  to fetch; subsequent entries are instant.

### One-time: scaffold the Android project

Run once per fresh checkout. Creates `src-tauri/gen/android/` with the
Kotlin `MainActivity`, Gradle Rust plugin, and `AndroidManifest.xml`:

```bash
nix develop .#androidDev
npm install
npm run tauri android init -- --skip-targets-install
```

### Build

```bash
nix develop .#androidDev

# Live-reload on a USB-connected device or running emulator.
# Device and dev machine must be on the same WiFi network.
# On NixOS: run `sudo adb devices` once first to grant USB access.
npm run android:dev

# Self-contained APK + AAB: UI bundled, no LAN dependency.
# First run is slow (full Holochain stack × 3 ABIs); incremental builds are fast.
# Omit the ANDROID_* vars to produce an unsigned APK (fine for quick local testing).
export ANDROID_STORE_FILE=/path/to/edet-release.jks
export ANDROID_STORE_PASSWORD=yourpassword
export ANDROID_KEY_ALIAS=edet
export ANDROID_KEY_PASSWORD=yourpassword
bash scripts/patch-signing.sh 
npm run android:build
```

### Outputs

| Path | Use |
|------|-----|
| `src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk` | APK — sideload or GitHub Releases |
| `src-tauri/gen/android/app/build/outputs/bundle/universalRelease/app-universal-release.aab` | AAB — Google Play Store |

### Sideloading the APK

```bash
# USB — fastest
adb install src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk

# Re-install over an existing version
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk
```

### CI — Signed release builds

`.github/workflows/android-release.yml` triggers automatically on every `v*.*.*`
tag push. It runs inside the `androidDev` Nix shell, produces a signed universal APK
and AAB, uploads both as workflow artifacts, and creates a GitHub Release with the
files attached.

#### One-time keystore setup

Android release signing requires an RSA JKS keystore. A helper script generates a fresh one for you:

```bash
bash scripts/create-keystore.sh edet-release.jks
```

The script prompts for a key alias and password, then prints the four values you
need to add as repository secrets (`Settings → Secrets and variables → Actions`):

| Secret | Description |
|--------|-------------|
| `ANDROID_KEYSTORE_BASE64` | base64-encoded `.jks` file (printed by the script) |
| `ANDROID_KEY_ALIAS` | key alias chosen during creation (default: `edet`) |
| `ANDROID_KEYSTORE_PASSWORD` | keystore password |
| `ANDROID_KEY_PASSWORD` | key password (same value as keystore password) |

> **Keep `edet-release.jks` and its passwords safe.** Losing the keystore makes
> it impossible to publish updates under the same signing identity. Store it in a
> password manager — never commit it to this repository.


#### Trigger a release

```bash
git tag v0.2.1 && git push origin v0.2.1
```

The workflow completes in ~30–90 min on first run while Nix fetches the Android NDK
and cross-compile toolchain; subsequent runs are faster via Cargo and Cachix caches.

You can also trigger the workflow manually from
`Actions → Android Release → Run workflow` without pushing a tag (useful for
testing signing on an untagged build; no GitHub Release is created in that case).

### First-run expectations

- Cold start takes ~10 s on-device while the embedded Holochain runtime
  boots. The UI waits on the `holochain://setup-completed` event;
  `startup_state` blocks up to 30 s before returning an error.
- The `.happ` is shipped as a Tauri bundle resource
  (`bundle.resources` in `src-tauri/tauri.conf.json`) and extracted to
  the app's data directory on first install.

### Known limitations

- **iOS is not supported.** Wasmer cannot JIT-compile on iOS due to
  Apple's restrictions; an interpreter backend is being developed
  upstream.
- **Background operation on Android requires battery optimization exemption.**
  The app runs a Foreground Service that keeps the conductor alive when
  backgrounded, but aggressive OEM battery managers (Xiaomi, Huawei,
  Samsung, OnePlus) may still kill the process unless the user whitelists
  edet. On first launch the app prompts for battery optimization exemption.

## Documentation

The formal whitepaper (`doc/edet.pdf`) covers:

- Protocol definition and debt lifecycle (Sections 1–3)
- EigenTrust adaptation, convergence proof, and subgraph approximation (Section 4)
- Credit capacity formula and two-layer enforcement (Section 5)
- Security analysis: 13 theorems covering Sybil isolation, flash loans,
  whitewashing, eclipse attacks, gateway attacks, and more (Section 6)
- Implementation notes and parameter derivations (Appendix B)

## Tooling

| Tool | Purpose |
|------|---------|
| [hc](https://github.com/holochain/holochain/tree/develop/crates/hc) | Holochain CLI — pack DNAs, manage sandboxes |
| [cargo nextest](https://nexte.st) | Fast Rust test runner |
| [tauri](https://tauri.app/) | Cross-platform application bundler |
| [holochain](https://github.com/holochain/holochain) | Embedded in-process conductor + lair-keystore |
| [@holochain/client](https://www.npmjs.com/package/@holochain/client) | UI ↔ conductor WebSocket client |
| [@holochain-playground/cli](https://www.npmjs.com/package/@holochain-playground/cli) | DHT introspection during development |

## Licensing

edet is licensed under [AGPL-3.0](./LICENSE). Contributions are
accepted on the same terms.