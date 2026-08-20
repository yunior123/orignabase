#!/usr/bin/env bash
# run-all-tests.sh — Full cross-stack test runner for origna_gta
#
# Starts local services, runs all Rust + Flutter + SDK tests, prints summary.
# Results saved to /tmp/origna_test_*.log
#
# Usage:
#   ./scripts/run-all-tests.sh              # Full suite (starts/stops services)
#   ./scripts/run-all-tests.sh --no-infra   # Skip service start/stop (assume running)
#   ./scripts/run-all-tests.sh --rust-only  # Rust tests only
#   ./scripts/run-all-tests.sh --flutter-only # Flutter tests only
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MONOREPO_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"
FLUTTER_DIR="$MONOREPO_ROOT/origna_gta"
SDK_DIR="$PROJECT_ROOT/sdks/flutter/orignabase"
FLUTTER_BIN="/Users/yuniorrodriguezosorio/flutter/bin/flutter"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# --- Timestamp for log files ---
TS=$(date +%Y%m%d_%H%M%S)
LOG_DIR="/tmp/origna_tests_$TS"
mkdir -p "$LOG_DIR"

CLIPPY_LOG="$LOG_DIR/clippy.log"
CARGO_TEST_LOG="$LOG_DIR/cargo_test.log"
CARGO_IGNORED_LOG="$LOG_DIR/cargo_ignored.log"
FLUTTER_ANALYZE_LOG="$LOG_DIR/flutter_analyze.log"
FLUTTER_TEST_LOG="$LOG_DIR/flutter_test.log"
SDK_TEST_LOG="$LOG_DIR/sdk_test.log"
SUMMARY_LOG="$LOG_DIR/summary.log"

# --- Parse args ---
MANAGE_INFRA=true
RUST_ONLY=false
FLUTTER_ONLY=false

for arg in "$@"; do
  case "$arg" in
    --no-infra)      MANAGE_INFRA=false ;;
    --rust-only)     RUST_ONLY=true ;;
    --flutter-only)  FLUTTER_ONLY=true ;;
    --help|-h)
      echo "Usage: $0 [--no-infra] [--rust-only] [--flutter-only]"
      echo ""
      echo "  --no-infra      Skip starting/stopping local services"
      echo "  --rust-only     Run only Rust tests (clippy + cargo test + ignored)"
      echo "  --flutter-only  Run only Flutter tests (analyze + test + SDK)"
      echo ""
      echo "Results saved to /tmp/origna_tests_<timestamp>/"
      exit 0
      ;;
  esac
done

# --- Track results ---
declare -a RESULTS=()
TOTAL_PASS=0
TOTAL_FAIL=0

record_result() {
  local name="$1" status="$2" details="$3"
  if [[ "$status" == "PASS" ]]; then
    RESULTS+=("${GREEN}[PASS]${NC} $name — $details")
    TOTAL_PASS=$((TOTAL_PASS + 1))
  else
    RESULTS+=("${RED}[FAIL]${NC} $name — $details")
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
  fi
}

# --- Cleanup on exit ---
cleanup() {
  if $MANAGE_INFRA && ! $FLUTTER_ONLY; then
    echo ""
    echo -e "${YELLOW}[cleanup] Stopping local services...${NC}"
    "$SCRIPT_DIR/local-dev.sh" --stop 2>/dev/null || true
  fi

  # --- Print summary ---
  echo ""
  echo -e "${CYAN}================================================================${NC}"
  echo -e "${CYAN}  Test Results Summary${NC}"
  echo -e "${CYAN}================================================================${NC}"
  for r in "${RESULTS[@]}"; do
    echo -e "  $r"
  done
  echo -e "${CYAN}----------------------------------------------------------------${NC}"
  if [[ $TOTAL_FAIL -eq 0 ]]; then
    echo -e "  ${GREEN}ALL $TOTAL_PASS SUITES PASSED${NC}"
  else
    echo -e "  ${GREEN}$TOTAL_PASS passed${NC}, ${RED}$TOTAL_FAIL failed${NC}"
  fi
  echo -e "  Logs: $LOG_DIR/"
  echo -e "${CYAN}================================================================${NC}"

  # Save summary
  {
    echo "Test Results — $(date)"
    echo "=============================="
    for r in "${RESULTS[@]}"; do
      echo "$r" | sed 's/\x1b\[[0-9;]*m//g'
    done
    echo "------------------------------"
    echo "Total: $TOTAL_PASS passed, $TOTAL_FAIL failed"
    echo "Logs: $LOG_DIR/"
  } > "$SUMMARY_LOG"
}
trap cleanup EXIT

# --- Start infrastructure ---
if $MANAGE_INFRA && ! $FLUTTER_ONLY; then
  echo -e "${CYAN}[1/6] Starting local services...${NC}"
  "$SCRIPT_DIR/local-dev.sh" --auto 2>&1 | tail -20
  echo ""
fi

# ============================================================
# RUST TESTS
# ============================================================
if ! $FLUTTER_ONLY; then

  # --- Source cargo target dir helper ---
  if [[ -f "$SCRIPT_DIR/cargo-target-dir.sh" ]]; then
    source "$SCRIPT_DIR/cargo-target-dir.sh"
  fi

  cd "$PROJECT_ROOT"

  # --- Clippy ---
  echo -e "${CYAN}[2/6] Running cargo clippy...${NC}"
  if [[ -f "$SCRIPT_DIR/cargo-target-dir.sh" ]]; then
    export_orignabase_cargo_target_dir test
  fi
  if cargo clippy --workspace --all-targets -- -D warnings > "$CLIPPY_LOG" 2>&1; then
    record_result "Rust clippy" "PASS" "zero warnings"
  else
    record_result "Rust clippy" "FAIL" "see $CLIPPY_LOG"
  fi

  # --- Cargo test (unit) ---
  echo -e "${CYAN}[3/6] Running cargo test --workspace...${NC}"
  if cargo test --workspace > "$CARGO_TEST_LOG" 2>&1; then
    # Extract test count from output
    TEST_LINE=$(grep -E "^test result:" "$CARGO_TEST_LOG" | tail -1 || echo "")
    record_result "Rust unit tests" "PASS" "$TEST_LINE"
  else
    TEST_LINE=$(grep -E "^test result:" "$CARGO_TEST_LOG" | tail -1 || echo "see log")
    record_result "Rust unit tests" "FAIL" "$TEST_LINE"
  fi

  # --- Cargo test --ignored (integration) ---
  echo -e "${CYAN}[4/6] Running cargo test -- --ignored (integration)...${NC}"
  export OB_TEST_URL="${OB_TEST_URL:-http://localhost:8080}"
  if cargo test -- --ignored > "$CARGO_IGNORED_LOG" 2>&1; then
    TEST_LINE=$(grep -E "^test result:" "$CARGO_IGNORED_LOG" | tail -1 || echo "")
    record_result "Rust integration tests" "PASS" "$TEST_LINE"
  else
    TEST_LINE=$(grep -E "^test result:" "$CARGO_IGNORED_LOG" | tail -1 || echo "see log")
    record_result "Rust integration tests" "FAIL" "$TEST_LINE"
  fi

fi

# ============================================================
# FLUTTER TESTS
# ============================================================
if ! $RUST_ONLY; then

  # --- Flutter analyze ---
  echo -e "${CYAN}[5/6] Running flutter analyze...${NC}"
  cd "$FLUTTER_DIR"
  if "$FLUTTER_BIN" analyze --no-fatal-infos > "$FLUTTER_ANALYZE_LOG" 2>&1; then
    ISSUE_COUNT=$(grep -cE "info|warning|error" "$FLUTTER_ANALYZE_LOG" 2>/dev/null || echo "0")
    record_result "Flutter analyze" "PASS" "$ISSUE_COUNT issues (no fatal)"
  else
    ISSUE_COUNT=$(grep -cE "error" "$FLUTTER_ANALYZE_LOG" 2>/dev/null || echo "?")
    record_result "Flutter analyze" "FAIL" "$ISSUE_COUNT errors — see $FLUTTER_ANALYZE_LOG"
  fi

  # --- Flutter test ---
  echo -e "${CYAN}[6/6] Running flutter test...${NC}"
  if "$FLUTTER_BIN" test --exclude-tags golden > "$FLUTTER_TEST_LOG" 2>&1; then
    TEST_LINE=$(grep -E "All tests passed|tests passed" "$FLUTTER_TEST_LOG" | tail -1 || echo "passed")
    record_result "Flutter tests" "PASS" "$TEST_LINE"
  else
    TEST_LINE=$(grep -E "tests? (passed|failed)" "$FLUTTER_TEST_LOG" | tail -1 || echo "see log")
    record_result "Flutter tests" "FAIL" "$TEST_LINE"
  fi

  # --- SDK tests ---
  if [[ -d "$SDK_DIR" ]]; then
    echo -e "${CYAN}[bonus] Running OrignaBase SDK tests...${NC}"
    cd "$SDK_DIR"
    if "$FLUTTER_BIN" test > "$SDK_TEST_LOG" 2>&1; then
      TEST_LINE=$(grep -E "All tests passed|tests passed" "$SDK_TEST_LOG" | tail -1 || echo "passed")
      record_result "SDK tests" "PASS" "$TEST_LINE"
    else
      TEST_LINE=$(grep -E "tests? (passed|failed)" "$SDK_TEST_LOG" | tail -1 || echo "see log")
      record_result "SDK tests" "FAIL" "$TEST_LINE"
    fi
  else
    record_result "SDK tests" "FAIL" "SDK dir not found: $SDK_DIR"
  fi

fi

# Exit with failure if any test suite failed
if [[ $TOTAL_FAIL -gt 0 ]]; then
  exit 1
fi
