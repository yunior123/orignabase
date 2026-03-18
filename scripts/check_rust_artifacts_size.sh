#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${1:-$ROOT_DIR/target}"
MAX_GB="${ORIGNABASE_MAX_TARGET_GB:-30}"
MAX_BYTES=$((MAX_GB * 1024 * 1024 * 1024))

if [[ ! -e "$TARGET_DIR" ]]; then
    echo "Rust build artifacts check: $TARGET_DIR does not exist."
    exit 0
fi

SIZE_BYTES="$(du -sk "$TARGET_DIR" | awk '{print $1 * 1024}')"
SIZE_GB="$(awk -v bytes="$SIZE_BYTES" 'BEGIN { printf "%.2f", bytes / 1024 / 1024 / 1024 }')"

echo "Rust build artifacts: $TARGET_DIR = ${SIZE_GB} GiB (limit: ${MAX_GB} GiB)"

if (( SIZE_BYTES > MAX_BYTES )); then
    echo "Rust build artifacts exceed the configured limit." >&2
    echo "Run ./scripts/clean_rust_artifacts.sh --all or remove legacy target/debug output." >&2
    exit 1
fi

