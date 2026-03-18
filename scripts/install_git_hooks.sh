#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOKS_DIR="$ROOT_DIR/.githooks"

mkdir -p "$HOOKS_DIR"

cat >"$HOOKS_DIR/pre-push" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
"$ROOT_DIR/scripts/check_rust_artifacts_size.sh"
EOF

chmod +x "$HOOKS_DIR/pre-push"
git -C "$ROOT_DIR" config core.hooksPath "$HOOKS_DIR"

echo "Installed Git hooks in $HOOKS_DIR"
echo "pre-push now enforces ORIGNABASE_MAX_TARGET_GB=${ORIGNABASE_MAX_TARGET_GB:-30}"
