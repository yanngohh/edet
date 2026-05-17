#!/usr/bin/env bash
# scripts/patch-android.sh - Apply all custom and required patches to the generated Android project.
#
# Patches applied:
#   1. BuildTask.kt  — skip i686 builds (wasmer-vm 6.1.0 UNWIND_DATA_REG issue)
#   2. AndroidManifest.xml — ensure CAMERA + ACCESS_NETWORK_STATE permissions and camera hardware features
#   3. MainActivity.kt — inject our robust dynamic WebView console logger
#   4. Mipmap resources — restore our custom padded circular launcher icons

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Change to project root
cd "$PROJECT_ROOT"

BUILDTASK_FILE="src-tauri/gen/android/buildSrc/src/main/java/org/edet/kotlin/BuildTask.kt"
MANIFEST_FILE="src-tauri/gen/android/app/src/main/AndroidManifest.xml"
PATCH_MARKER="// Skip i686 (x86 32-bit): wasmer-vm 6.1.0 does not define"

# ── 0. Initialize Android project if missing ───────────────────────────────────

if [ ! -d "src-tauri/gen/android" ]; then
  echo "📱 Initializing Android Studio project via Tauri CLI..."
  # Run in non-interactive mode using npx tauri android init
  npx -y tauri android init --no-interactive || true
fi

# ── 1. Patch BuildTask.kt (i686 skip) ─────────────────────────────────────────

is_buildtask_patched() {
  [[ -f "$BUILDTASK_FILE" ]] && grep -q "$PATCH_MARKER" "$BUILDTASK_FILE"
}

apply_buildtask_patch() {
  echo "Applying i686 skip patch to BuildTask.kt..."
  local temp_patch
  temp_patch=$(mktemp)
  cat > "$temp_patch" << 'EOF'

        // Skip i686 (x86 32-bit): wasmer-vm 6.1.0 does not define
        // UNWIND_DATA_REG for this target. No real Android device uses it.
        if (target == "i686") {
            project.logger.lifecycle("Skipping Rust build for i686 (wasmer-vm incompatible)")
            return
        }
EOF
  awk '
    /val target = target \?: throw GradleException\("target cannot be null"\)/ {
      print $0
      while ((getline line < "'"$temp_patch"'") > 0) { print line }
      close("'"$temp_patch"'")
      next
    }
    { print }
  ' "$BUILDTASK_FILE" > "$BUILDTASK_FILE.tmp" && mv "$BUILDTASK_FILE.tmp" "$BUILDTASK_FILE"
  rm "$temp_patch"
  echo "✅ BuildTask.kt patch applied."
}

if [[ -f "$BUILDTASK_FILE" ]]; then
  if is_buildtask_patched; then
    echo "✅ BuildTask.kt i686 patch already present"
  else
    apply_buildtask_patch
  fi
else
  echo "⚠️ BuildTask.kt not found"
fi

# ── 2. Patch AndroidManifest.xml permissions ────────────────────────────────

if [[ -f "$MANIFEST_FILE" ]]; then
  add_permission_if_missing() {
    local permission="$1"
    local marker="android.permission.${permission}"
    if grep -q "$marker" "$MANIFEST_FILE"; then
      echo "✅ ${permission} permission already in manifest"
    else
      echo "Adding ${permission} permission to AndroidManifest.xml..."
      sed -i "s|<uses-permission android:name=\"android.permission.INTERNET\" />|<uses-permission android:name=\"android.permission.INTERNET\" />\n    <uses-permission android:name=\"${marker}\" />|" "$MANIFEST_FILE"
      echo "✅ ${permission} permission added."
    fi
  }

  add_permission_if_missing "ACCESS_NETWORK_STATE"
  add_permission_if_missing "CAMERA"

  # Ensure the camera hardware feature is declared as optional
  if ! grep -q "android.hardware.camera" "$MANIFEST_FILE"; then
    echo "Adding camera hardware feature declarations..."
    sed -i "s|<uses-permission android:name=\"android.permission.CAMERA\" />|<uses-permission android:name=\"android.permission.CAMERA\" />\n    <uses-feature android:name=\"android.hardware.camera\" android:required=\"false\" />\n    <uses-feature android:name=\"android.hardware.camera.autofocus\" android:required=\"false\" />|" "$MANIFEST_FILE"
    echo "✅ Camera feature declarations added."
  fi
else
  echo "⚠️ AndroidManifest.xml not found"
fi

# ── 3. Copy our custom MainActivity.kt ─────────────────────────────────────────

echo "🩹 Copying patched MainActivity.kt..."
mkdir -p src-tauri/gen/android/app/src/main/java/org/edet
cp src-tauri/android-patches/MainActivity.kt src-tauri/gen/android/app/src/main/java/org/edet/MainActivity.kt
echo "✅ MainActivity.kt patched."

echo "✅ Android project patched successfully!"
