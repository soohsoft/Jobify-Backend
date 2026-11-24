#!/usr/bin/env bash
# Start the bot from the built output.
#
# .env is sourced here so the same file drives a manual run, `npm start` and a systemd
# EnvironmentFile; values already exported by the service manager win, because the parser in
# src/env.ts never overwrites them.
set -euo pipefail
cd "$(dirname "$0")"

if [[ ! -f dist/index.js ]]; then
  echo "dist/index.js is missing — run 'npm run build' first" >&2
  exit 1
fi

if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  . ./.env
  set +a
fi

exec node dist/index.js
