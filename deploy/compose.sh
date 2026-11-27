#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v podman >/dev/null 2>&1 && podman compose version >/dev/null 2>&1; then
  COMPOSE=(podman compose)
elif command -v podman-compose >/dev/null 2>&1; then
  COMPOSE=(podman-compose)
else
  printf 'install docker compose, podman compose or podman-compose\n' >&2
  exit 2
fi

exec "${COMPOSE[@]}" --env-file .env -f compose.yaml "$@"
