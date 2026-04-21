#!/bin/bash

# create-keystore.sh — Generate a fresh Android-compatible JKS keystore
# for app signing (RSA 2048-bit).
#
# Usage:
#   bash scripts/create-keystore.sh [output.jks]
#
# Defaults:
#   output : edet-release.jks  (written to the current directory)
#
# Requirements: keytool (comes with any JDK 8+)
#
# After running, add four secrets to your GitHub repository
# (Settings → Secrets and variables → Actions → New repository secret):
#
#   ANDROID_KEYSTORE_BASE64    — base64 of the generated .jks file
#   ANDROID_KEY_ALIAS          — value you entered when prompted (default: edet)
#   ANDROID_KEYSTORE_PASSWORD  — password you chose below
#   ANDROID_KEY_PASSWORD       — same value (PKCS12 requires them to match)

set -euo pipefail

OUTPUT_JKS="${1:-edet-release.jks}"

# ── Validate inputs ──────────────────────────────────────────────────────────

if ! command -v keytool &>/dev/null; then
    echo "Error: 'keytool' is not installed or not in PATH"
    exit 1
fi

echo ""
echo "=== Android Keystore Creator ==="
echo "Output  : $OUTPUT_JKS"
echo ""

# ── Prompt for configuration ─────────────────────────────────────────────────

read -rp "Key alias (default: edet): " KEY_ALIAS
KEY_ALIAS="${KEY_ALIAS:-edet}"

read -rp "Distinguished name CN (default: edet-release): " CERT_CN
CERT_CN="${CERT_CN:-edet-release}"

while true; do
    read -rsp "Keystore / key password (min 6 chars): " KS_PASS
    echo
    if [[ ${#KS_PASS} -ge 6 ]]; then
        break
    fi
    echo "Password must be at least 6 characters."
done

read -rsp "Confirm password: " KS_PASS2
echo
if [[ "$KS_PASS" != "$KS_PASS2" ]]; then
    echo "Error: passwords do not match."
    exit 1
fi

# ── Generate Keystore ─────────────────────────────────────────────────────────

echo ""
echo "Generating new RSA-2048 keypair..."

# We generate a JKS keystore natively.
# Even though AGP 8 supports PKCS12, JKS is historically standard for Android.
rm -f "$OUTPUT_JKS"
keytool -genkeypair -v \
    -keystore "$OUTPUT_JKS" \
    -storetype JKS \
    -alias "$KEY_ALIAS" \
    -keyalg RSA \
    -keysize 2048 \
    -validity 10000 \
    -dname "CN=$CERT_CN, O=edet, OU=mobile, C=IT" \
    -storepass "$KS_PASS" \
    -keypass "$KS_PASS" 2>/dev/null

echo ""
echo "=== Keystore created: $OUTPUT_JKS ==="
echo ""
echo "Verify with:"
echo "  keytool -list -v -keystore $OUTPUT_JKS -storepass '$KS_PASS'"
echo ""
echo "=== GitHub Secrets ==="
echo "Add the following secrets to your GitHub repository"
echo "(Settings → Secrets and variables → Actions → New repository secret):"
echo ""
echo "  ANDROID_KEYSTORE_BASE64  :"
base64 -w0 "$OUTPUT_JKS"
echo ""
echo ""
echo "  ANDROID_KEY_ALIAS        : $KEY_ALIAS"
echo "  ANDROID_KEYSTORE_PASSWORD: $KS_PASS"
echo "  ANDROID_KEY_PASSWORD     : $KS_PASS"
echo ""
echo "IMPORTANT: Keep '$OUTPUT_JKS' and these passwords safe."
echo "Losing the keystore or passwords makes it impossible to update"
echo "your app on Google Play or re-sign future releases with the same key."

