#!/usr/bin/env bash
# Generates latest.json for the Tauri updater by reading release assets.
# Requires: gh CLI, curl, jq
#
# Usage: ./generate-updater-manifest.sh <tag>
#
# Outputs latest.json in the current directory with:
#   - version
#   - notes
#   - pub_date
#   - platforms: { <target>: { signature, url } }

set -euo pipefail

TAG="${1:-}"
if [ -z "$TAG" ]; then
    echo "Usage: $0 <tag>" >&2
    exit 1
fi

REPO="${GITHUB_REPOSITORY:-Miike0303/Locust}"
VERSION="${TAG#v}"
PUB_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Fetch release assets
ASSETS_JSON=$(gh release view "$TAG" --repo "$REPO" --json assets,body)
NOTES=$(echo "$ASSETS_JSON" | jq -r '.body // ""')

# Map file patterns to updater targets
declare -A PLATFORMS

# Windows x86_64
WIN_MSI=$(echo "$ASSETS_JSON" | jq -r '.assets[] | select(.name | endswith("_x64_en-US.msi")) | .name' | head -1)
WIN_MSI_SIG=$(echo "$ASSETS_JSON" | jq -r '.assets[] | select(.name | endswith(".msi.sig")) | .name' | head -1)
if [ -n "$WIN_MSI" ] && [ -n "$WIN_MSI_SIG" ]; then
    SIG=$(curl -sL "https://github.com/$REPO/releases/download/$TAG/$WIN_MSI_SIG")
    URL="https://github.com/$REPO/releases/download/$TAG/$WIN_MSI"
    PLATFORMS["windows-x86_64"]="$(jq -n --arg sig "$SIG" --arg url "$URL" '{signature: $sig, url: $url}')"
fi

# macOS aarch64
MAC_ARM=$(echo "$ASSETS_JSON" | jq -r '.assets[] | select(.name | test("aarch64.*\\.app\\.tar\\.gz$")) | .name' | head -1)
MAC_ARM_SIG=$(echo "$ASSETS_JSON" | jq -r '.assets[] | select(.name | test("aarch64.*\\.app\\.tar\\.gz\\.sig$")) | .name' | head -1)
if [ -n "$MAC_ARM" ] && [ -n "$MAC_ARM_SIG" ]; then
    SIG=$(curl -sL "https://github.com/$REPO/releases/download/$TAG/$MAC_ARM_SIG")
    URL="https://github.com/$REPO/releases/download/$TAG/$MAC_ARM"
    PLATFORMS["darwin-aarch64"]="$(jq -n --arg sig "$SIG" --arg url "$URL" '{signature: $sig, url: $url}')"
fi

# macOS x86_64
MAC_X64=$(echo "$ASSETS_JSON" | jq -r '.assets[] | select(.name | test("x64.*\\.app\\.tar\\.gz$")) | .name' | head -1)
MAC_X64_SIG=$(echo "$ASSETS_JSON" | jq -r '.assets[] | select(.name | test("x64.*\\.app\\.tar\\.gz\\.sig$")) | .name' | head -1)
if [ -n "$MAC_X64" ] && [ -n "$MAC_X64_SIG" ]; then
    SIG=$(curl -sL "https://github.com/$REPO/releases/download/$TAG/$MAC_X64_SIG")
    URL="https://github.com/$REPO/releases/download/$TAG/$MAC_X64"
    PLATFORMS["darwin-x86_64"]="$(jq -n --arg sig "$SIG" --arg url "$URL" '{signature: $sig, url: $url}')"
fi

# Linux x86_64
LINUX=$(echo "$ASSETS_JSON" | jq -r '.assets[] | select(.name | endswith("_amd64.AppImage")) | .name' | head -1)
LINUX_SIG=$(echo "$ASSETS_JSON" | jq -r '.assets[] | select(.name | endswith("_amd64.AppImage.sig")) | .name' | head -1)
if [ -n "$LINUX" ] && [ -n "$LINUX_SIG" ]; then
    SIG=$(curl -sL "https://github.com/$REPO/releases/download/$TAG/$LINUX_SIG")
    URL="https://github.com/$REPO/releases/download/$TAG/$LINUX"
    PLATFORMS["linux-x86_64"]="$(jq -n --arg sig "$SIG" --arg url "$URL" '{signature: $sig, url: $url}')"
fi

# Assemble platforms object
PLATFORMS_JSON="{"
FIRST=1
for key in "${!PLATFORMS[@]}"; do
    if [ $FIRST -eq 0 ]; then PLATFORMS_JSON+=","; fi
    PLATFORMS_JSON+="\"$key\":${PLATFORMS[$key]}"
    FIRST=0
done
PLATFORMS_JSON+="}"

jq -n \
    --arg version "$VERSION" \
    --arg notes "$NOTES" \
    --arg pub_date "$PUB_DATE" \
    --argjson platforms "$PLATFORMS_JSON" \
    '{version: $version, notes: $notes, pub_date: $pub_date, platforms: $platforms}' \
    > latest.json

cat latest.json
