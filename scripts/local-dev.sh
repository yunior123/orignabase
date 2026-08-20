#!/usr/bin/env bash
# local-dev.sh — Start all local dev services and verify health
# Usage: ./scripts/local-dev.sh [--stripe] [--seed] [--auto] [--stop] [--status]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MONOREPO_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"
DOCKER_DIR="$PROJECT_ROOT/docker"
STRIPE_CLI="/opt/homebrew/bin/stripe"
SEED_SCRIPT_TS="$MONOREPO_ROOT/e2e/lib/seed-dev.ts"
SEED_SCRIPT_PY="$PROJECT_ROOT/scripts/seed_orignabase.py"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

log()  { echo -e "${GREEN}[local-dev]${NC} $*"; }
warn() { echo -e "${YELLOW}[local-dev]${NC} $*"; }
err()  { echo -e "${RED}[local-dev]${NC} $*" >&2; }
info() { echo -e "${CYAN}[local-dev]${NC} $*"; }

# --- Parse args ---
ENABLE_STRIPE=false
ENABLE_SEED=false
AUTO_MODE=false
STOP_MODE=false
STATUS_MODE=false

for arg in "$@"; do
  case "$arg" in
    --stripe) ENABLE_STRIPE=true ;;
    --seed)   ENABLE_SEED=true ;;
    --auto)   AUTO_MODE=true ;;
    --stop)   STOP_MODE=true ;;
    --status) STATUS_MODE=true ;;
    --help|-h)
      echo "Usage: $0 [--stripe] [--seed] [--auto] [--stop] [--status]"
      echo ""
      echo "  --stripe  Start Stripe CLI webhook forwarding"
      echo "  --seed    Run seed script after services are healthy"
      echo "  --auto    Auto-detect: start Stripe if CLI exists, seed if DB empty"
      echo "  --stop    Stop all local services and exit"
      echo "  --status  Print service status and exit"
      echo ""
      echo "Services started:"
      echo "  PostgreSQL   -> localhost:5432"
      echo "  Meilisearch  -> localhost:7700"
      echo "  OrignaBase   -> localhost:8080"
      echo "  Caddy        -> localhost:80/443"
      echo "  ChromaDB     -> localhost:8100 (if image exists)"
      echo "  Stripe CLI   -> forwards webhooks to localhost:8080 (with --stripe/--auto)"
      exit 0
      ;;
  esac
done

# --- Status mode ---
print_service_status() {
  local name="$1" probe="$2"
  if [[ "$probe" == pg:* ]]; then
    local host="${probe#pg:}"
    if pg_isready -h "$host" -p 5432 >/dev/null 2>&1; then
      echo -e "  ${GREEN}[UP]${NC}   $name  postgres://$host:5432"
    else
      echo -e "  ${RED}[DOWN]${NC} $name  postgres://$host:5432"
    fi
    return
  fi

  if curl -sf "$probe" >/dev/null 2>&1; then
    echo -e "  ${GREEN}[UP]${NC}   $name  $probe"
  else
    echo -e "  ${RED}[DOWN]${NC} $name  $probe"
  fi
}

if $STATUS_MODE; then
  echo ""
  echo -e "${CYAN}Service Status:${NC}"
  print_service_status "PostgreSQL " "pg:localhost"
  print_service_status "Meilisearch" "http://localhost:7700/health"
  print_service_status "OrignaBase " "http://localhost:8080/health"
  if docker ps --format '{{.Names}}' 2>/dev/null | grep -q chromadb; then
    print_service_status "ChromaDB   " "http://localhost:8100/api/v2/heartbeat"
  fi
  if pgrep -f "stripe listen" >/dev/null 2>&1; then
    echo -e "  ${GREEN}[UP]${NC}   Stripe CLI  (forwarding webhooks)"
  else
    echo -e "  ${RED}[DOWN]${NC} Stripe CLI  (not running)"
  fi
  echo ""
  # Show Docker container status
  echo -e "${CYAN}Docker Containers:${NC}"
  docker compose -f "$DOCKER_DIR/docker-compose.yml" ps --format "table {{.Name}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null || echo "  (docker compose not available)"
  exit 0
fi

# --- Stop mode ---
if $STOP_MODE; then
  log "Stopping all local services..."
  cd "$DOCKER_DIR"
  docker compose down --remove-orphans 2>/dev/null || true
  docker stop chromadb 2>/dev/null || true
  docker rm chromadb 2>/dev/null || true
  if pgrep -f "stripe listen" >/dev/null 2>&1; then
    pkill -f "stripe listen" 2>/dev/null || true
    log "Stripe CLI stopped."
  fi
  log "All services stopped."
  exit 0
fi

# --- Preflight checks ---
log "Preflight checks..."

# Check colima/Docker
if ! docker info >/dev/null 2>&1; then
  warn "Docker not running. Starting colima..."
  colima start 2>&1 | tail -3
  if ! docker info >/dev/null 2>&1; then
    err "Docker still not running after colima start. Aborting."
    exit 1
  fi
fi

# Check .env exists
if [[ ! -f "$DOCKER_DIR/.env" ]]; then
  if [[ -f "$DOCKER_DIR/.env.local.example" ]]; then
    log "Copying .env.local.example -> .env"
    cp "$DOCKER_DIR/.env.local.example" "$DOCKER_DIR/.env"
  elif [[ -f "$DOCKER_DIR/.env.dev" ]]; then
    log "Copying .env.dev -> .env"
    cp "$DOCKER_DIR/.env.dev" "$DOCKER_DIR/.env"
  else
    err "No .env file found in $DOCKER_DIR"
    err "Create one from .env.local.example: cp docker/.env.local.example docker/.env"
    exit 1
  fi
fi

# --- Start Docker services ---
log "Starting Docker services..."
cd "$DOCKER_DIR"
docker compose up -d --wait 2>&1 | tail -5 || {
  warn "docker compose --wait failed, falling back to manual health checks..."
}

# --- Wait for health ---
wait_for_health() {
  local name="$1" probe="$2" max_wait="${3:-60}"
  local elapsed=0
  while true; do
    if [[ "$probe" == pg:* ]]; then
      local host="${probe#pg:}"
      if pg_isready -h "$host" -p 5432 >/dev/null 2>&1; then
        break
      fi
    elif curl -sf "$probe" >/dev/null 2>&1; then
      break
    fi
    sleep 2
    elapsed=$((elapsed + 2))
    if [[ $elapsed -ge $max_wait ]]; then
      err "$name failed to become healthy after ${max_wait}s"
      return 1
    fi
  done
  log "$name healthy (${elapsed}s)"
}

log "Waiting for services to become healthy..."
wait_for_health "PostgreSQL"  "pg:localhost" 60
wait_for_health "Meilisearch" "http://localhost:7700/health" 60
wait_for_health "OrignaBase"  "http://localhost:8080/health" 90

# --- ChromaDB (optional — only if image exists) ---
if docker image inspect chromadb/chroma:latest >/dev/null 2>&1; then
  if ! docker ps --format '{{.Names}}' | grep -q chromadb; then
    log "Starting ChromaDB..."
    docker run -d --name chromadb -p 8100:8000 chromadb/chroma:latest 2>/dev/null || true
    wait_for_health "ChromaDB" "http://localhost:8100/api/v2/heartbeat" 30 || warn "ChromaDB failed to start (non-fatal)"
  else
    log "ChromaDB already running"
  fi
fi

# --- Auto-detect: Stripe CLI ---
if $AUTO_MODE && [[ -x "$STRIPE_CLI" ]]; then
  ENABLE_STRIPE=true
  info "Auto-detected Stripe CLI at $STRIPE_CLI"
fi

# --- Stripe CLI (optional) ---
if $ENABLE_STRIPE; then
  if [[ ! -x "$STRIPE_CLI" ]]; then
    err "Stripe CLI not found at $STRIPE_CLI"
    err "Install: brew install stripe/stripe-cli/stripe"
  elif ! [[ -f "$HOME/.config/stripe/config.toml" ]]; then
    err "Stripe CLI not configured. Run: stripe login"
  else
    # Check if already listening
    if pgrep -f "stripe listen" >/dev/null 2>&1; then
      warn "Stripe CLI already listening"
    else
      log "Starting Stripe CLI webhook forwarding..."
      "$STRIPE_CLI" listen \
        --forward-to localhost:8080/api/webhooks/stripe \
        --log-level warn \
        2>&1 | while IFS= read -r line; do
          # Capture and display the webhook signing secret
          if echo "$line" | grep -q "whsec_"; then
            WHSEC=$(echo "$line" | grep -oE 'whsec_[a-zA-Z0-9]+')
            echo ""
            echo -e "${YELLOW}=== Stripe webhook signing secret ===${NC}"
            echo -e "${GREEN}  $WHSEC${NC}"
            echo -e "${YELLOW}  Add to .env: OB_SECRETS__STRIPE_WEBHOOK_SECRET=$WHSEC${NC}"
            echo -e "${YELLOW}=====================================${NC}"
            echo ""
          fi
          echo "$line"
        done &
      sleep 3
      log "Stripe CLI forwarding webhooks -> localhost:8080"
    fi
  fi
fi

# --- Auto-detect: seed if DB is empty ---
if $AUTO_MODE && ! $ENABLE_SEED; then
  info "Checking if database needs seeding..."
  # Query OrignaBase health/users count via a simple auth attempt
  # If we get a valid response from listing users, DB has data
  USER_COUNT=$(curl -sf "http://localhost:8080/health" 2>/dev/null | grep -c "ok" || echo "0")
  if [[ "$USER_COUNT" != "0" ]]; then
    # Server is up — try to detect if users collection is empty via a test login
    # A 401 means the endpoint works (DB has schema). A 500/connection error means empty DB.
    LOGIN_STATUS=$(curl -sf -o /dev/null -w "%{http_code}" \
      -X POST "http://localhost:8080/auth/login" \
      -H "Content-Type: application/json" \
      -d '{"email":"probe@test.local","password":"probe"}' 2>/dev/null || echo "000")
    if [[ "$LOGIN_STATUS" == "000" || "$LOGIN_STATUS" == "500" ]]; then
      info "Database appears empty (login probe returned $LOGIN_STATUS). Auto-seeding..."
      ENABLE_SEED=true
    else
      info "Database has data (login probe returned $LOGIN_STATUS). Skipping seed."
    fi
  fi
fi

# --- Seed (optional) ---
if $ENABLE_SEED; then
  if [[ -f "$SEED_SCRIPT_TS" ]] && command -v bun >/dev/null 2>&1; then
    log "Seeding dev database via bun (seed-dev.ts)..."
    ORIGNABASE_URL=http://127.0.0.1:8080 bun run "$SEED_SCRIPT_TS" 2>&1 | tail -10
    log "Seed complete"
  elif [[ -f "$SEED_SCRIPT_PY" ]]; then
    log "Seeding dev database via python (seed_orignabase.py)..."
    python3 "$SEED_SCRIPT_PY" --url http://localhost:8080 2>&1 | tail -5
    log "Seed complete"
  else
    warn "No seed script found. Checked:"
    warn "  $SEED_SCRIPT_TS (needs bun)"
    warn "  $SEED_SCRIPT_PY"
  fi
fi

# --- Summary ---
echo ""
echo -e "${GREEN}========================================================${NC}"
echo -e "${GREEN}  Local dev environment ready!${NC}"
echo ""
print_service_status "PostgreSQL " "pg:localhost"
print_service_status "Meilisearch" "http://localhost:7700/health"
print_service_status "OrignaBase " "http://localhost:8080/health"
if docker ps --format '{{.Names}}' 2>/dev/null | grep -q chromadb; then
  print_service_status "ChromaDB   " "http://localhost:8100/api/v2/heartbeat"
fi
if pgrep -f "stripe listen" >/dev/null 2>&1; then
  echo -e "  ${GREEN}[UP]${NC}   Stripe CLI  (forwarding webhooks)"
fi
echo ""
echo "  Flutter:  cd origna_gta && flutter run --dart-define=ENVIRONMENT=emulator"
echo "  Tests:    flutter test --dart-define=RUN_ORIGNABASE_LIVE_TESTS=true --dart-define=ENVIRONMENT=emulator"
echo "  Rust:     OB_TEST_URL=http://localhost:8080 cargo test -- --ignored"
echo "  Status:   $0 --status"
echo "  Stop:     $0 --stop"
echo -e "${GREEN}========================================================${NC}"
