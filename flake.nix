{
  description = "Flake for Holochain app development";

  inputs = {
    holonix.url = "github:holochain/holonix?ref=main-0.6";

    nixpkgs.follows = "holonix/nixpkgs";
    flake-parts.follows = "holonix/flake-parts";
  };

  outputs = inputs@{ flake-parts, ... }: flake-parts.lib.mkFlake { inherit inputs; } {
    systems = builtins.attrNames inputs.holonix.devShells;
    perSystem = { inputs', system, ... }:
      let
        pkgs = import inputs.nixpkgs {
          inherit system;
          config.allowUnfree = true;
          # Allow androidenv to install the SDK
          config.android_sdk.accept_license = true;
        };

        # Android SDK composition via nixpkgs androidenv.
        # NDK 26.1.10909125 matches the ABI level edet targets and the
        # clang version required for Rust cross-compilation to Android.
        androidSdk = pkgs.androidenv.composeAndroidPackages {
          cmdLineToolsVersion  = "8.0";
          platformVersions     = [ "34" ];
          buildToolsVersions   = [ "34.0.0" ];
          includeNDK           = true;
          ndkVersions          = [ "26.1.10909125" ];
          includeEmulator      = false;
          includeSources       = false;
          includeSystemImages  = false;
        };

        # Convenience: the path where the NDK lands inside the SDK directory.
        ndkHome = "${androidSdk.androidsdk}/libexec/android-sdk/ndk/26.1.10909125";

      in
      {
        formatter = pkgs.nixpkgs-fmt;

        devShells = {
          # ── Default shell ── Holochain SDK + desktop build tools ───────────
          # Use for day-to-day zome development, unit tests, and desktop builds.
          default = pkgs.mkShell {
            inputsFrom = [ inputs'.holonix.devShells.default ];

            packages = with pkgs; [
              nodejs_22
              binaryen
              cargo-audit
              cargo-nextest
              pkg-config
              cmake
              openssl
              zlib
              # Tauri v2 desktop WebView dependencies (Linux)
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
              # Transitive link deps for AppImage bundling
              fribidi
              harfbuzz
              freetype
              fontconfig
            ];

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

          # ── androidDev shell ── Android NDK + Rust cross targets ────────────
          # Use for Android builds and CI.
          #
          # Self-contained using nixpkgs androidenv — no external flake inputs
          # required beyond holonix.
          #
          # The tx5 Android 11+ networking fix (holochain/tx5#87) is handled
          # automatically through the wlynxg/anet dependency in pion/webrtc/v4.
          # No patched Go toolchain is required.
          #
          # NOTE: holonix is NOT included here. Its Rust toolchain does not
          # have Android ABI targets baked in and would shadow the targets
          # we install below via rustup in shellHook. CI must run
          # `rustup target add` once before building.
          #
          # Use via:  nix develop .#androidDev
          # Then:     rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
          #           npm ci
          #           npm run tauri android init -- --skip-targets-install
          #           npm run tauri android build --apk
          androidDev = pkgs.mkShell {
            packages = with pkgs; [
              # Android toolchain
              androidSdk.androidsdk
              jdk17
              # Node / build tools
              nodejs_22
              cargo-audit
              cargo-nextest
              # Holochain CLI for happ bundle building
              inputs'.holonix.packages.hc
              inputs'.holonix.packages.lair-keystore
              # Misc
              pkg-config
              openssl
              zlib
              # libclang for bindgen (datachannel-sys)
              llvmPackages.libclang
            ];

            ANDROID_NDK_HOME = ndkHome;
            NDK_HOME = ndkHome;
            JAVA_HOME = "${pkgs.jdk17}";

            shellHook = ''
              export PS1='\[\033[1;34m\][androidDev:\w]\$\[\033[0m\] '
              export LAIR_KEYSTORE_DISABLE_MLOCK=1
              export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"

              # Create a local mutable Android SDK directory to satisfy Tauri's 
              # expectation of cmdline-tools/latest and read-write permissions.
              export ANDROID_HOME="$PWD/.android-sdk"
              export ANDROID_SDK_ROOT="$ANDROID_HOME"
              export ANDROID_NDK_HOME="$NDK_HOME"
              export ANDROID_NDK_ROOT="$NDK_HOME"
              export ANDROID_NDK="$NDK_HOME"
              
              mkdir -p "$ANDROID_HOME/cmdline-tools"
              for f in "${androidSdk.androidsdk}/libexec/android-sdk/"*; do
                  basename_f=$(basename "$f")
                  if [ "$basename_f" != "cmdline-tools" ] && [ "$basename_f" != "ndk" ] && [ "$basename_f" != "ndk-bundle" ]; then
                      if [ -d "$f" ]; then
                          mkdir -p "$ANDROID_HOME/$basename_f"
                          for subf in "$f"/*; do
                              if [ -e "$subf" ]; then
                                  ln -sfn "$subf" "$ANDROID_HOME/$basename_f/$(basename "$subf")"
                              fi
                          done
                      else
                          ln -sfn "$f" "$ANDROID_HOME/$basename_f"
                      fi
                  fi
              done
              
              # Symlink cmdline-tools to 'latest'
              if [ -d "${androidSdk.androidsdk}/libexec/android-sdk/cmdline-tools" ]; then
                  # Get the first versioned directory (e.g., 8.0)
                  version_dir=$(ls -1 "${androidSdk.androidsdk}/libexec/android-sdk/cmdline-tools" | head -n 1)
                  if [ -n "$version_dir" ]; then
                      ln -sfn "${androidSdk.androidsdk}/libexec/android-sdk/cmdline-tools/$version_dir" "$ANDROID_HOME/cmdline-tools/latest"
                  fi
              fi
              
              # Ensure NDK is visible in the SDK path Tauri expects
              if [ -d "$NDK_HOME" ]; then
                  mkdir -p "$ANDROID_HOME/ndk"
                  ln -sfn "$NDK_HOME" "$ANDROID_HOME/ndk/26.1.10909125"
                  ln -sfn "$NDK_HOME" "$ANDROID_HOME/ndk-bundle"
              fi

              # wasm32 getrandom must use the custom backend in zome builds
              export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS='--cfg getrandom_backend="custom"'

              # Rust linkers for Android ABI cross-compilation.
              # These point to the clang wrappers in the NDK toolchain.
              LLVM_BIN="$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
              export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$LLVM_BIN/aarch64-linux-android24-clang"
              export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$LLVM_BIN/armv7a-linux-androideabi24-clang"
              export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$LLVM_BIN/x86_64-linux-android24-clang"
              export CARGO_TARGET_I686_LINUX_ANDROID_LINKER="$LLVM_BIN/i686-linux-android24-clang"

              # Bindgen needs to know the cross-compilation target and sysroot
              # otherwise it falls back to host headers (and fails with stubs-32.h)
              SYSROOT="$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot"
              export BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android="--sysroot=$SYSROOT --target=aarch64-linux-android24"
              export BINDGEN_EXTRA_CLANG_ARGS_armv7_linux_androideabi="--sysroot=$SYSROOT --target=armv7a-linux-androideabi24"
              export BINDGEN_EXTRA_CLANG_ARGS_x86_64_linux_android="--sysroot=$SYSROOT --target=x86_64-linux-android24"
              export BINDGEN_EXTRA_CLANG_ARGS_i686_linux_android="--sysroot=$SYSROOT --target=i686-linux-android24"

              # Prevent Nix from injecting host C flags into Android cross builds.
              # Nix sets NIX_CFLAGS_COMPILE and NIX_LDFLAGS for native host builds,
              # which break Android cross-compilation when passed to the NDK clang.
              unset NIX_CFLAGS_COMPILE
              unset NIX_LDFLAGS

              ulimit -l unlimited 2>/dev/null || true
            '';
          };

          # ── holochainTauriDev shell ── Desktop Tauri + Holochain ─────────────
          # Use for desktop `npm run tauri:dev` and `cargo check`.
          # Extends the default holonix shell with libclang (needed by
          # datachannel-sys bindgen) and GTK/WebKit deps.
          holochainTauriDev = pkgs.mkShell {
            inputsFrom = [ inputs'.holonix.devShells.default ];

            packages = with pkgs; [
              nodejs_22
              cargo-audit
              cargo-nextest
              # libclang for bindgen (datachannel-sys and similar C-binding crates)
              llvmPackages.libclang
              cmake
              pkg-config
              openssl
              zlib
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
            ];

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
              export PS1='\[\033[1;34m\][tauriDev:\w]\$\[\033[0m\] '
              export LAIR_KEYSTORE_DISABLE_MLOCK=1
              export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
              ulimit -l unlimited 2>/dev/null || true
            '';
          };
        };
      };
  };
}
