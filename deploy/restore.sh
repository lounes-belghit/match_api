#!/usr/bin/env bash
set -euo pipefail

FILE="${1:-}"
if [ -z "$FILE" ] || [ ! -f "$FILE" ]; then
  printf 'usage: %s /path/to/matchs-*.sql.gz\n' "$0" >&2
  exit 2
fi
if [ "${CONFIRM_RESTORE:-}" != "yes" ]; then
  printf 'set CONFIRM_RESTORE=yes to overwrite the current database\n' >&2
  exit 2
fi

cd "$(dirname "$0")"
set -a
. ./.env
set +a

./compose.sh stop api
gunzip -c "$FILE" | ./compose.sh exec -T db psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -v ON_ERROR_STOP=1
./compose.sh start api
printf 'database restored from %s\n' "$FILE"
