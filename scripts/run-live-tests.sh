#!/usr/bin/env bash
# run-live-tests.sh — Run Rust integration tests against a live OrignaBase server
# These are the #[ignore] tests in crates/orignabase/tests/
#
# Usage:
#   ./scripts/run-live-tests.sh                          # localhost:8080 (default)
#   ./scripts/run-live-tests.sh https://api.dev.orignagta.ca  # remote dev
#   ./scripts/run-live-tests.sh --smoke                  # smoke tests only
#   ./scripts/run-live-tests.sh --file security_fixes_test  # single test file
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# --- Parse args ---
OB_TEST_URL="${1:-http://localhost:8080}"
SMOKE_ONLY=false
TEST_FILE=""

for arg in "$@"; do
  case "$arg" in
    --smoke) SMOKE_ONLY=true ;;
    --file)  shift; TEST_FILE="${2:-}" ;;
    --help|-h)
      echo "Usage: $0 [URL] [--smoke] [--file TEST_NAME]"
      echo ""
      echo "  URL         Server URL (default: http://localhost:8080)"
      echo "  --smoke     Run smoke tests only (health, register, login)"
      echo "  --file X    Run a specific test file (e.g., security_fixes_test)"
      echo ""
      echo "Test files available:"
      ls "$PROJECT_ROOT/crates/orignabase/tests/"*.rs 2>/dev/null | xargs -I{} basename {} .rs | grep -v mod | sort
      exit 0
      ;;
    http*) OB_TEST_URL="$arg" ;;
  esac
done

# --- Preflight: Check server is reachable ---
echo -e "${YELLOW}Testing connection to $OB_TEST_URL ...${NC}"
if ! curl -sf "$OB_TEST_URL/health" >/dev/null 2>&1; then
  echo -e "${RED}Server at $OB_TEST_URL is not reachable.${NC}"
  echo ""
  echo "Start local server:"
  echo "  ./scripts/local-dev.sh          # Docker Compose"
  echo "  cargo run -- serve              # Embedded RocksDB"
  echo ""
  echo "Or use remote dev:"
  echo "  $0 https://api.dev.orignagta.ca"
  exit 1
fi
echo -e "${GREEN}Server healthy.${NC}"

# --- Source cargo target dir helper ---
if [[ -f "$PROJECT_ROOT/scripts/cargo-target-dir.sh" ]]; then
  source "$PROJECT_ROOT/scripts/cargo-target-dir.sh"
  export_orignabase_cargo_target_dir test 2>/dev/null || true
fi

cd "$PROJECT_ROOT"

export OB_TEST_URL

# --- Run tests ---
if $SMOKE_ONLY; then
  echo -e "${YELLOW}Running smoke tests...${NC}"
  cargo test --test smoke_test -- --ignored --nocapture 2>&1
elif [[ -n "$TEST_FILE" ]]; then
  echo -e "${YELLOW}Running $TEST_FILE...${NC}"
  cargo test --test "$TEST_FILE" -- --ignored --nocapture 2>&1
else
  echo -e "${YELLOW}Running ALL integration tests (this may take a while)...${NC}"
  echo "Test files:"
  ls crates/orignabase/tests/*.rs | xargs -I{} basename {} .rs | grep -v mod | sort | sed 's/^/  /'
  echo ""
  cargo test --test '*' -- --ignored --nocapture 2>&1
fi

EXIT_CODE=$?
echo ""
if [[ $EXIT_CODE -eq 0 ]]; then
  echo -e "${GREEN}All tests passed.${NC}"
else
  echo -e "${RED}Some tests failed (exit code: $EXIT_CODE).${NC}"
fi
exit $EXIT_CODE
