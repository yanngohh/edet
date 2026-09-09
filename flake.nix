{
  description = "edet — a non-hoardable clearing medium on a community-federated ledger";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Pinned Rust toolchain. >=1.87 is required (int is_multiple_of).
        rust = pkgs.rust-bin.stable."1.90.0".default.override {
          extensions = [ "rust-src" "rustfmt" "clippy" "rust-analyzer" ];
        };

        # Native deps the workspace + the M3 engine (protobuf) will want.
        buildInputs = with pkgs; [ openssl ];
        nativeBuildInputs = with pkgs; [ pkg-config protobuf ];

        python = pkgs.python312.withPackages (ps: with ps; [ numpy scipy ]);

        # Android SDK/NDK for the mobile client (T3, on-device custody).
        # A second, scoped nixpkgs import so the SDK's unfree license
        # acceptance never leaks into the main `pkgs`.
        androidPkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
            android_sdk.accept_license = true;
          };
        };
        # Matched to the tauri 2.11.4 android template (AGP 8.11, gradle
        # 8.14.3, JDK 21): the app compiles against SDK 36, the committed
        # edet-keystore plugin module against 34.
        # The emulator and one x86_64 system image are included because
        # `just android-keystore-test` is the only thing that RUNS Android
        # custody: a Keystore key and a file under `noBackupFilesDir` exist on
        # a device or an emulator and nowhere else, so without an image that
        # path is reasoning rather than a gate. `google_apis` rather than the
        # bare AOSP image because the Play-services-flavoured one is what a
        # handset runs.
        androidComposition = androidPkgs.androidenv.composeAndroidPackages {
          platformVersions = [ "34" "36" ];
          buildToolsVersions = [ "34.0.0" "35.0.0" ];
          includeNDK = true;
          includeEmulator = true;
          includeSystemImages = true;
          systemImageTypes = [ "google_apis" ];
          abiVersions = [ "x86_64" ];
        };
        androidSdkRoot = "${androidComposition.androidsdk}/libexec/android-sdk";

        # The same pinned toolchain plus the four Android rust-std targets
        # (`cargo tauri android build` defaults to building all four ABIs).
        rustAndroid = pkgs.rust-bin.stable."1.90.0".default.override {
          extensions = [ "rust-src" "rustfmt" "clippy" ];
          targets = [
            "aarch64-linux-android"
            "armv7-linux-androideabi"
            "i686-linux-android"
            "x86_64-linux-android"
          ];
        };
      in
      {
        # `nix develop` — Rust build + test shell.
        #
        # `cargo-audit` is here because `just audit` FAILS without it (exit 2,
        # "the checker could not run") rather than skipping itself green. A
        # shell that cannot run a gate is a shell that reports on fewer gates
        # than the reader thinks. `cargo-nextest` is here for the same reason
        # and more sharply: `just test`, `engine-test`, `swarm`, `cost` and
        # `size-seed` all invoke `cargo nextest run`, and without it in the
        # shell every one of them fails at "no such subcommand" — the gate list
        # does not run at all rather than running and reporting.
        devShells.default = pkgs.mkShell {
          inherit buildInputs nativeBuildInputs;
          packages = [ rust python pkgs.cargo-audit pkgs.cargo-nextest ];
          env.PROTOC = "${pkgs.protobuf}/bin/protoc";
          shellHook = ''
            echo "edet dev shell — rust $(rustc --version | cut -d' ' -f2), $(python3 --version)"
            echo "  cargo nextest run --workspace   # rust suites"
            echo "  python3 sim/run.py --fixtures   # sim suites + kernel cross-pin"
          '';
        };

        # `nix develop .#sim` — lighter Python-only shell for the simulation.
        devShells.sim = pkgs.mkShell {
          packages = [ python ];
          shellHook = ''echo "edet sim shell — $(python3 --version) with numpy, scipy"'';
        };

        # `nix develop .#tauri` — Rust + the webkit/gtk stack the Tauri client
        # bundle needs, plus the Tauri CLI. Heavier; only needed to build the
        # desktop/Android app in `src-tauri`.
        devShells.tauri = pkgs.mkShell {
          buildInputs = with pkgs; [
            openssl
            gtk3
            webkitgtk_4_1
            libsoup_3
            librsvg
          ];
          nativeBuildInputs = with pkgs; [
            pkg-config
            protobuf
            wrapGAppsHook3
            cargo-tauri
            nodejs_22
          ];
          packages = [ rust ];
          env.PROTOC = "${pkgs.protobuf}/bin/protoc";
          shellHook = ''echo "edet tauri shell — rust + webkitgtk4.1 + cargo-tauri + node"'';
        };

        # `nix develop .#android` — the tauri shell plus the Android SDK/NDK
        # and a Gradle-compatible JDK, for `cargo tauri android init/build`
        # against a real device (see README, "What is left").
        devShells.android = pkgs.mkShell {
          buildInputs = with pkgs; [
            openssl
            gtk3
            webkitgtk_4_1
            libsoup_3
            librsvg
          ];
          nativeBuildInputs = with pkgs; [
            pkg-config
            protobuf
            wrapGAppsHook3
            cargo-tauri
            nodejs_22
          ];
          packages = [ rustAndroid androidComposition.androidsdk androidPkgs.jdk21 ];
          env = {
            PROTOC = "${pkgs.protobuf}/bin/protoc";
            JAVA_HOME = "${androidPkgs.jdk21.home}";
            ANDROID_HOME = androidSdkRoot;
            ANDROID_SDK_ROOT = androidSdkRoot;
            NDK_HOME = "${androidSdkRoot}/ndk-bundle";
            ANDROID_NDK_ROOT = "${androidSdkRoot}/ndk-bundle";
          };
          shellHook = ''echo "edet android shell — tauri + android sdk/ndk + jdk21"'';
        };

        # `nix flake check` runs the workspace tests + the sim suites.
        checks.tests = pkgs.stdenv.mkDerivation {
          name = "edet-tests";
          src = self;
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ [ rust python pkgs.cargo-nextest ];
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          buildPhase = ''
            export CARGO_HOME=$TMPDIR/cargo
            cargo nextest run --workspace --offline || cargo nextest run --workspace
            python3 sim/run.py --fixtures
          '';
          installPhase = "touch $out";
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
