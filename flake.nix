{
  description = "Flake for Holochain app development";

  inputs = {
    holonix.url = "github:holochain/holonix?ref=main-0.6";

    nixpkgs.follows = "holonix/nixpkgs";
    flake-parts.follows = "holonix/flake-parts";

    # p2p Shipyard (tauri-plugin-holochain). Provides the `androidDev`
    # devShell that pre-bakes the Android NDK, a patched Go toolchain
    # (works around holochain/tx5#87 on Android 11+), and Rust cross-
    # compile targets for all four Android ABIs. The plugin itself is
    # Source-Available pending darksoil.studio's crowdfunding goal;
    tauri-plugin-holochain.url = "github:darksoil-studio/tauri-plugin-holochain";
    tauri-plugin-holochain.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = inputs@{ flake-parts, ... }: flake-parts.lib.mkFlake { inherit inputs; } {
    systems = builtins.attrNames inputs.holonix.devShells;
    perSystem = { inputs', system, ... }:
      let
        pkgs = import inputs.nixpkgs {
          inherit system;
          config.allowUnfree = true;
        };
      in
      {
        formatter = pkgs.nixpkgs-fmt;

        devShells = {
          # Default shell: Holochain SDK + Essential Dev Tools
          # Python/GPU simulation is now managed via Conda/Mamba/Pixi separately.
          default = pkgs.mkShell {
            inputsFrom = [ inputs'.holonix.devShells.default ];

            packages = with pkgs; [
              nodejs_22
              husky
              binaryen
              cargo-audit
              cargo-nextest
              pkg-config
              cmake
              openssl
              zlib
              # System libraries required by the Tauri v2 desktop shell
              # (`src-tauri/`). Keep in sync with the platform requirements
              # listed at https://tauri.app/start/prerequisites/#linux.
              webkitgtk_4_1
              libsoup_3
              gtk3
              glib
              cairo
              pango
              gdk-pixbuf
              atk
              librsvg
              libayatana-appindicator
              # Transitive deps linuxdeploy resolves when producing an
              # AppImage (ldd walk of our Tauri binary). Missing these
              # causes the appimage bundle step to fail with "Could not
              # find dependency: libfribidi.so.0" and similar.
              fribidi
              harfbuzz
              freetype
              fontconfig
            ];

            # LD_LIBRARY_PATH is set only for libraries that must be
            # resolvable when the Tauri binary is *run* from the dev
            # shell (e.g. `cargo run`, `npm run tauri:dev`). Mixing this
            # with system library paths (e.g. /usr/lib64) breaks glibc
            # ABI assumptions and has caused segfaults in the Tauri
            # bundler; we therefore keep it strictly nix-only.
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
              webkitgtk_4_1
              libsoup_3
              gtk3
              glib
              cairo
              pango
              gdk-pixbuf
              atk
              librsvg
              libayatana-appindicator
              fribidi
              harfbuzz
              freetype
              fontconfig
              openssl
              zlib
            ]);

            shellHook = ''
              export PS1='\[\033[1;34m\][holonix:\w]\$\[\033[0m\] '
              export LAIR_KEYSTORE_DISABLE_MLOCK=1
              ulimit -l unlimited 2>/dev/null || true
            '';
          };

          # Android development shell. Based on p2p Shipyard's androidDev
          # which pre-bakes: Android NDK (26.x), JDK 17, the patched Go
          # toolchain that bypasses the tx5 Android 11+ netlinkrib bug
          # (holochain/tx5#87), and Rust cross-compile targets for
          # `aarch64-linux-android`, `armv7-linux-androideabi`,
          # `x86_64-linux-android`, `i686-linux-android`.
          #
          # We extend it with dev tooling so that pre-commit hooks
          # (cargo audit, cargo clippy, npm test) work in this shell.
          # NOTE: we do NOT include holonix here because its Rust
          # toolchain would shadow the Android-target-enabled one from
          # p2p Shipyard.
          #
          # Use via:   nix develop .#androidDev
          # Then:      npm run tauri android init -- --skip-targets-install
          #            npm run tauri android dev
          androidDev = pkgs.mkShell {
            inputsFrom = [
              inputs'.tauri-plugin-holochain.devShells.androidDev
            ];

            # The upstream `androidDev` shell provides only the Android SDK/NDK
            # and environment variables — it does NOT include a Rust toolchain.
            # Locally this is fine (system rustup has the targets), but CI needs
            # a self-contained Nix-provided Rust.
            #
            # `androidTauriRust` from the plugin provides Rust with all four
            # Android ABI targets PLUS wasm32-unknown-unknown (needed for
            # `build:happ` which compiles zomes to WASM). This replaces the
            # old comment about "not including holonix to avoid shadowing" —
            # we now use the plugin's purpose-built Rust package that has
            # everything in one toolchain.
            packages = with pkgs; [
              nodejs_22
              husky
              cargo-audit
              cargo-nextest
              inputs'.holonix.packages.hc
              inputs'.holonix.packages.lair-keystore
              inputs'.tauri-plugin-holochain.packages.androidTauriRust
            ];

            nativeBuildInputs = [
              inputs'.tauri-plugin-holochain.packages.fixNixCflagsAndroidHook
            ];

            shellHook = ''
              export PS1='\[\033[1;34m\][androidDev:\w]\$\[\033[0m\] '
              export LAIR_KEYSTORE_DISABLE_MLOCK=1
              export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS='--cfg getrandom_backend="custom"'
              ulimit -l unlimited 2>/dev/null || true
            '';
          };

          # Desktop Tauri + Holochain build shell. Needed for *building*
          # the src-tauri crate now that it statically links the
          # in-process holochain runtime. Adds libclang (required by
          # bindgen for datachannel-sys), openssl, cmake and the
          # WebKit/GTK libs Tauri itself needs.
          #
          # `holochainTauriDev` extends the upstream tauri-plugin-holochain
          # shell with holonix (adds hc, hc-spin, lair-keystore, etc.) so
          # that `npm run tauri:dev` (which calls hc app pack) works in a
          # single shell. Use this for all day-to-day Tauri + Holochain work.
          #
          # Use via:   nix develop .#holochainTauriDev
          # Then:      cargo check --manifest-path src-tauri/Cargo.toml
          #            npm run tauri:dev
          holochainTauriDev = pkgs.mkShell {
            inputsFrom = [
              inputs'.tauri-plugin-holochain.devShells.holochainTauriDev
              inputs'.holonix.devShells.default
            ];

            packages = with pkgs; [
              nodejs_22
              husky
              cargo-audit
              cargo-nextest
            ];

            shellHook = ''
              export PS1='\[\033[1;34m\][tauri-plugin-holochain:\w]\$\[\033[0m\] '
              export LAIR_KEYSTORE_DISABLE_MLOCK=1
              ulimit -l unlimited 2>/dev/null || true
            '';
          };
        };
      };
  };
}
