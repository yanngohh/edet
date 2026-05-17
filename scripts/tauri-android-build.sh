#!/bin/bash

# tauri-android-build.sh - Run Tauri Android build with automatic patching
# This wrapper ensures the i686 skip patch is applied after Tauri regenerates files

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Intercept openssl-src perl Configure calls to inject 'no-asm' to bypass NDK assembler mnemonic errors
export OPENSSL_SRC_PERL="$SCRIPT_DIR/perl-wrapper.sh"

echo "🚀 Starting Tauri Android build..."

# Change to project root
cd "$PROJECT_ROOT"

# Build the hApp first — include_bytes! in conductor.rs embeds it at
# compile time, so it must exist before cargo runs.
echo "📦 Building hApp bundle..."
npm run build:happ

# Apply all patches before running Tauri
echo "🔧 Applying Android patches..."
"$SCRIPT_DIR/patch-android.sh"

# Run the Tauri command with all passed arguments
echo "📱 Running tauri android build..."
npm run -- tauri android build "$@"

echo "✅ Android build completed!"