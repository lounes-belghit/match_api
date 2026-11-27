#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
set -a
. ./.env
set +a

BACKUP_DIR="${BACKUP_DIR:-$HOME/backups/matchs}"
RETENTION="${BACKUP_RETENTION_DAYS:-14}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$BACKUP_DIR"
TARGET="$BACKUP_DIR/matchs-$STAMP.sql.gz"

./compose.sh exec -T db pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" | gzip > "$TARGET"
find "$BACKUP_DIR" -maxdepth 1 -type f -name 'matchs-*.sql.gz' -mtime "+$RETENTION" -delete
printf 'backup written to %s\n' "$TARGET"
