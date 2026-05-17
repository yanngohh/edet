#!/bin/bash

# tauri-android-dev.sh - Run Tauri Android development with automatic patching
# This wrapper ensures the i686 skip patch is applied after Tauri regenerates files

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Intercept openssl-src perl Configure calls to inject 'no-asm' to bypass NDK assembler mnemonic errors
export OPENSSL_SRC_PERL="$SCRIPT_DIR/perl-wrapper.sh"

echo "🚀 Starting Tauri Android development..."

# Change to project root
cd "$PROJECT_ROOT"

# Build the hApp first — include_bytes! in conductor.rs embeds it at
# compile time, so it must exist before cargo runs.
echo "📦 Building hApp bundle..."
npm run build:happ

# Ensure UI dist directory exists to satisfy Tauri's generate_context macro
mkdir -p ui/dist

# Setup USB port forwarding to bypass Wi-Fi/Firewall issues
echo "🔄 Setting up adb port forwarding (tcp:8888)..."
$ANDROID_HOME/platform-tools/adb reverse tcp:8888 tcp:8888 || true
export TAURI_DEV_HOST=127.0.0.1

# Apply all patches before running Tauri
echo "🔧 Applying Android patches..."
"$SCRIPT_DIR/patch-android.sh"

# Run the Tauri command with all passed arguments
echo "📱 Running tauri android dev..."
npm run -- tauri android dev --host 127.0.0.1 "$@"

echo "✅ Ready for Android development!"