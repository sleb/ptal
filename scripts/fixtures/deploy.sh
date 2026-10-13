#!/usr/bin/env bash
set -euo pipefail

SERVER=deploy@example.internal

echo "copying build/ to $SERVER"
rsync -a --delete build/ "$SERVER:/srv/app/"

echo "restarting service"
ssh "$SERVER" systemctl restart app

echo "running smoke test"
curl -fsS "https://example.internal/health" | grep -q ok
echo "smoke test: passed"
