#!/usr/bin/env bash
set -euo pipefail

HOST="${HOST:-192.168.122.35}"
REMOTE_USER="${REMOTE_USER:-ghost_vps}"
REMOTE_DIR="${REMOTE_DIR:-/home/ghost_vps/matchs_api}"
TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/opencode/matchs_api_target}"

if [ -n "${SSH_PASS:-}" ]; then
  SSH=(sshpass -p "$SSH_PASS" ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null)
  SCP=(sshpass -p "$SSH_PASS" scp -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null)
else
  SSH=(ssh -o StrictHostKeyChecking=no)
  SCP=(scp -o StrictHostKeyChecking=no)
fi

CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release --bins

"${SSH[@]}" "$REMOTE_USER@$HOST" "systemctl --user stop matchs-api.service 2>/dev/null || true"

"${SSH[@]}" "$REMOTE_USER@$HOST" "mkdir -p '$REMOTE_DIR/bin' '$REMOTE_DIR/logs'"
"${SCP[@]}" "$TARGET_DIR/release/server" "$TARGET_DIR/release/ingest" "$REMOTE_USER@$HOST:$REMOTE_DIR/bin/"

"${SSH[@]}" "$REMOTE_USER@$HOST" "systemctl --user daemon-reload && systemctl --user restart matchs-api.service && sleep 2 && curl -fsS http://127.0.0.1:18080/healthz"
printf 'deployed to %s@%s:%s\n' "$REMOTE_USER" "$HOST" "$REMOTE_DIR"
