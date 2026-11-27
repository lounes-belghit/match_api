#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'usage: %s --domain API_DOMAIN [--cors ORIGINS] [--email ADMIN_EMAIL] [--output FILE]\n' "$0"
}

DOMAIN=""
CORS=""
ADMIN_EMAIL="admin@example.com"
OUTPUT="$(cd "$(dirname "$0")" && pwd)/.env"

while [ $# -gt 0 ]; do
  case "$1" in
    --domain) DOMAIN="${2:-}"; shift 2 ;;
    --cors) CORS="${2:-}"; shift 2 ;;
    --email) ADMIN_EMAIL="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ -z "$DOMAIN" ]; then
  read -r -p "API domain (api.example.com): " DOMAIN
fi
if [ -z "$DOMAIN" ]; then
  printf 'domain is required\n' >&2
  exit 2
fi
if [ -z "$CORS" ]; then
  CORS="https://$DOMAIN"
fi

random_hex() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex "$1"
  else
    od -An -v -N "$1" /dev/urandom | tr -d ' \n'
  fi
}

POSTGRES_PASSWORD="$(random_hex 24)"
APP_SECRET="$(random_hex 32)"
ADMIN_PASSWORD="$(random_hex 18)"

umask 077
cat > "$OUTPUT" <<EOF
APP_ENV=production
DOMAIN=$DOMAIN
ACME_EMAIL=$ADMIN_EMAIL
CORS_ALLOWED_ORIGINS=$CORS

POSTGRES_DB=matchs
POSTGRES_USER=matchs
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
DATABASE_URL=postgres://matchs:$POSTGRES_PASSWORD@db:5432/matchs
REDIS_URL=redis://redis:6379

APP_SECRET=$APP_SECRET
ADMIN_EMAIL=$ADMIN_EMAIL
ADMIN_PASSWORD=$ADMIN_PASSWORD

BIND_ADDR=0.0.0.0:8080
DEFAULT_QUOTA=100000
RATE_PER_MIN=300
CACHE_TTL_SECS=60
STALE_AFTER_SECS=900
INGEST_ALERT_AFTER_SECS=3600
SESSION_TTL_HOURS=168
COOKIE_SECURE=true
REGISTRATIONS_ENABLED=true
DB_MAX_CONNECTIONS=20
DB_ACQUIRE_TIMEOUT_SECS=10
SOURCE_TIMEOUT_SECS=20
INGEST_CONCURRENCY=6
INGEST_MAX_RETRIES=3
INGEST_RETRY_BASE_MS=500
RUST_LOG=info
EOF
chmod 600 "$OUTPUT"
printf 'wrote %s (mode 600)\n' "$OUTPUT"
