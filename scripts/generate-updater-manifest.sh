#!/usr/bin/env bash
# Usage: ./generate-updater-manifest.sh <tag>
# Requires Python 3, gh and curl. Writes latest.json only after validation.
set -euo pipefail
exec python3 "$(dirname "${BASH_SOURCE[0]}")/generate-updater-manifest.py" "$@"
