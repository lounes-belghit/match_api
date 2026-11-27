#!/usr/bin/env bash
set -euo pipefail

HOST="${HOST:-192.168.122.35}"
REMOTE_USER="${REMOTE_USER:-ghost_vps}"
REMOTE_DIR="${REMOTE_DIR:-matchs_api}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ -n "${SSH_PASS:-}" ]; then
  SSH=(sshpass -p "$SSH_PASS" ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null)
else
  SSH=(ssh -o StrictHostKeyChecking=no)
fi

if [ ! -f "$ROOT/deploy/.env" ]; then
  printf 'missing deploy/.env; run deploy/init-secrets.sh first\n' >&2
  exit 2
fi

tar --exclude='target' --exclude='.git' --exclude='frontend' --exclude='deploy/.env' -czf - -C "$ROOT" . \
  | "${SSH[@]}" "$REMOTE_USER@$HOST" "mkdir -p ~/$REMOTE_DIR && tar -xzf - -C ~/$REMOTE_DIR"

"${SSH[@]}" "$REMOTE_USER@$HOST" "set -e
cd ~/$REMOTE_DIR/deploy
./compose.sh pull db redis caddy
./compose.sh up -d --build db redis api caddy
./compose.sh run --rm ingest backfill --seasons 3
set -a
. ./.env
set +a
./compose.sh exec -T api /app/bin/server init-admin --email \"\$ADMIN_EMAIL\" --name 'Admin'
"

KEY="$("${SSH[@]}" "$REMOTE_USER@$HOST" "cd ~/$REMOTE_DIR/deploy && set -a && . ./.env && set +a && ./compose.sh exec -T api /app/bin/server create-api-key --email \"\$ADMIN_EMAIL\" --name bootstrap")"
printf '%s\n' "$KEY" | "${SSH[@]}" "$REMOTE_USER@$HOST" "umask 077; cat > ~/$REMOTE_DIR/api_key.txt"

"${SSH[@]}" "$REMOTE_USER@$HOST" "set -e
mkdir -p ~/.config/systemd/user
for unit in ~/$REMOTE_DIR/deploy/systemd/*; do
  sed "s|%h/matchs_api|%h/$REMOTE_DIR|g" "\$unit" > ~/.config/systemd/user/"\$(basename "\$unit")"
done
systemctl --user daemon-reload
systemctl --user enable --now matchs-live.timer matchs-backfill.timer matchs-events.timer matchs-backup.timer
cd ~/$REMOTE_DIR/deploy
./smoke.sh ~/$REMOTE_DIR/api_key.txt
"

printf 'deployment complete: https://%s\n' "$("${SSH[@]}" "$REMOTE_USER@$HOST" "cd ~/$REMOTE_DIR/deploy && set -a && . ./.env && set +a && printf '%s' \"\$DOMAIN\"")"
