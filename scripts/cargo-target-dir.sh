#!/usr/bin/env bash

set -euo pipefail

ORIGNABASE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

orignabase_cargo_target_dir() {
    local bucket="${1:-dev}"
    case "$bucket" in
        dev|test|coverage|release)
            printf '%s\n' "$ORIGNABASE_ROOT/target/$bucket"
            ;;
        *)
            echo "Unknown Cargo target bucket: $bucket" >&2
            return 1
            ;;
    esac
}

export_orignabase_cargo_target_dir() {
    local bucket="${1:-dev}"
    export CARGO_TARGET_DIR
    CARGO_TARGET_DIR="$(orignabase_cargo_target_dir "$bucket")"
    echo "Using CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
}
