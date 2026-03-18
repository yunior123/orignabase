#!/usr/bin/env bash
# OrignaBase test coverage report using cargo-llvm-cov
# Usage: ./scripts/coverage.sh [--html] [--open] [--json]
#
# Prerequisites:
#   rustup component add llvm-tools-preview
#   cargo install cargo-llvm-cov

set -euo pipefail

cd "$(dirname "$0")/.."
. ./scripts/cargo-target-dir.sh

./scripts/check_rust_artifacts_size.sh

FORMAT="text"
OPEN=false

for arg in "$@"; do
    case "$arg" in
        --html) FORMAT="html" ;;
        --open) OPEN=true ;;
        --json) FORMAT="json" ;;
        --lcov) FORMAT="lcov" ;;
        *) echo "Unknown arg: $arg"; exit 1 ;;
    esac
done

echo "==> Running tests with coverage instrumentation..."
export_orignabase_cargo_target_dir coverage

case "$FORMAT" in
    html)
        cargo llvm-cov --workspace --html --output-dir coverage/
        echo "==> HTML report: coverage/html/index.html"
        if $OPEN; then open coverage/html/index.html; fi
        ;;
    json)
        cargo llvm-cov --workspace --json --output-path coverage/coverage.json
        echo "==> JSON report: coverage/coverage.json"
        ;;
    lcov)
        cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info
        echo "==> LCOV report: coverage/lcov.info"
        ;;
    text)
        cargo llvm-cov --workspace
        ;;
esac

./scripts/check_rust_artifacts_size.sh

echo "==> Done."
