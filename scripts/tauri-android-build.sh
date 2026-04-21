#!/bin/bash

# tauri-android-build.sh - Run Tauri Android build with automatic patching
# This wrapper ensures the i686 skip patch is applied after Tauri regenerates files

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "🚀 Starting Tauri Android build..."

# Change to project root
cd "$PROJECT_ROOT"

# Build the hApp first — include_bytes! in conductor.rs embeds it at
# compile time, so it must exist before cargo runs.
echo "📦 Building hApp bundle..."
npm run build:happ

# Run the Tauri command with all passed arguments
echo "📱 Running tauri android build..."
npm run -- tauri android build "$@"

# Apply the patch after Tauri has potentially regenerated files
echo "🔧 Checking and applying i686 skip patch..."
"$SCRIPT_DIR/patch-buildtask.sh"

echo "✅ Android build completed!"