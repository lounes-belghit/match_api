#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
set -a
. ./.env
set +a

BASE="${BASE:-https://$DOMAIN}"
KEY_FILE="${1:-${API_KEY_FILE:-}}"
if [ -z "$KEY_FILE" ]; then
  printf 'usage: %s /path/to/api_key.txt\n' "$0" >&2
  exit 2
fi
KEY="$(tr -d '\r\n' < "$KEY_FILE")"

printf 'config: '
./compose.sh exec -T api /app/bin/server check-config
printf 'healthz: '
curl -fsS -o /dev/null -w '%{http_code}\n' "$BASE/healthz"
printf 'readyz: '
curl -fsS -o /dev/null -w '%{http_code}\n' "$BASE/readyz"
printf 'matches: '
curl -fsS -o /dev/null -w '%{http_code}\n' -H "X-API-Key: $KEY" "$BASE/api/v1/matches?per_page=1"
printf 'status: '
curl -fsS -o /dev/null -w '%{http_code}\n' -H "X-API-Key: $KEY" "$BASE/api/v1/status"
