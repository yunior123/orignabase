#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MANAGED_BUCKETS=(dev test coverage release)
REMOVE_MANAGED=true
REMOVE_LEGACY=false

for arg in "$@"; do
    case "$arg" in
        --managed)
            REMOVE_MANAGED=true
            ;;
        --legacy)
            REMOVE_LEGACY=true
            ;;
        --all)
            REMOVE_MANAGED=true
            REMOVE_LEGACY=true
            ;;
        *)
            echo "Unknown arg: $arg" >&2
            echo "Usage: ./scripts/clean_rust_artifacts.sh [--managed] [--legacy] [--all]" >&2
            exit 1
            ;;
    esac
done

TARGET_DIR="$ROOT_DIR/target"
TO_REMOVE=()

if [[ "$REMOVE_MANAGED" == true ]]; then
    for bucket in "${MANAGED_BUCKETS[@]}"; do
        TO_REMOVE+=("$TARGET_DIR/$bucket")
    done
fi

if [[ "$REMOVE_LEGACY" == true ]]; then
    TO_REMOVE+=(
        "$TARGET_DIR/debug"
        "$TARGET_DIR/release"
        "$TARGET_DIR/incremental"
        "$TARGET_DIR/.rustc_info.json"
        "$TARGET_DIR/CACHEDIR.TAG"
    )
fi

echo "Removing Rust build artifacts:"
for path in "${TO_REMOVE[@]}"; do
    echo "  $path"
done

rm -rf "${TO_REMOVE[@]}"
