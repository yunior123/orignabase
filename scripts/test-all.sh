#!/usr/bin/env bash
# Run all orignabase tests: unit, integration (offline), proptest, snapshot
# For live integration tests (requires running server), use: --live
#
# Usage: ./scripts/test-all.sh [--live] [--coverage]

set -euo pipefail

cd "$(dirname "$0")/.."
. ./scripts/cargo-target-dir.sh

./scripts/check_rust_artifacts_size.sh

LIVE=false
COVERAGE=false

for arg in "$@"; do
    case "$arg" in
        --live) LIVE=true ;;
        --coverage) COVERAGE=true ;;
        *) echo "Unknown arg: $arg"; exit 1 ;;
    esac
done

echo "==> Running unit tests (all workspace crates)..."
if $COVERAGE; then
    export_orignabase_cargo_target_dir coverage
    cargo llvm-cov --workspace --no-report
else
    export_orignabase_cargo_target_dir test
    cargo test --workspace
fi

if $LIVE; then
    echo "==> Running live integration tests (requires running server)..."
    export_orignabase_cargo_target_dir test
    cargo test --test integration_test -- --ignored
    cargo test --test handlers_integration_test -- --ignored
fi

echo "==> Running clippy checks..."
export_orignabase_cargo_target_dir test
cargo clippy --workspace --all-targets -- -D warnings 2>/dev/null || true

if $COVERAGE; then
    echo "==> Generating coverage report..."
    export_orignabase_cargo_target_dir coverage
    cargo llvm-cov report --html --output-dir coverage/
    echo "==> Coverage report: coverage/html/index.html"
fi

./scripts/check_rust_artifacts_size.sh

echo "==> All tests complete."
