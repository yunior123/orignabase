#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-test}"
CONFIG_FILE="${STRIPE_CONFIG_FILE:-$HOME/.config/stripe/config.toml}"

if [[ ! -f "$CONFIG_FILE" ]]; then
  echo "Stripe CLI config not found: $CONFIG_FILE" >&2
  exit 1
fi

case "$MODE" in
  test)
    api_key_field="test_mode_api_key"
    publishable_key_field="test_mode_pub_key"
    ;;
  live)
    api_key_field="live_mode_api_key"
    publishable_key_field="live_mode_pub_key"
    ;;
  *)
    echo "Usage: $0 [test|live]" >&2
    exit 1
    ;;
esac

read_toml_value() {
  local key="$1"
  awk -F" = " -v wanted="$key" '
    $1 == wanted {
      value = $2
      gsub(/^'\''|'\''$/, "", value)
      print value
      exit
    }
  ' "$CONFIG_FILE"
}

stripe_secret_key="$(read_toml_value "$api_key_field")"
stripe_publishable_key="$(read_toml_value "$publishable_key_field")"

if [[ -z "$stripe_secret_key" ]]; then
  echo "Missing $api_key_field in $CONFIG_FILE" >&2
  exit 1
fi

cat <<EOF
export OB_SECRETS__STRIPE_SECRET_KEY='$stripe_secret_key'
export STRIPE_SECRET_KEY='$stripe_secret_key'
EOF

if [[ -n "$stripe_publishable_key" ]]; then
  cat <<EOF
export STRIPE_PUBLISHABLE_KEY='$stripe_publishable_key'
EOF
fi
