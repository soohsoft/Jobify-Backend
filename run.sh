#!/usr/bin/env bash
# Start jobify-backend with the DeepSeek credentials loaded.
#
# backend/.env deliberately keeps DEEPSEEK_API_KEY empty; the real key lives in
# the central Hermes env. Two traps make this fiddly:
#
#   1. Order. backend/.env's empty `DEEPSEEK_API_KEY=` placeholder clobbers a
#      key sourced before it, so the project env goes FIRST and the central env
#      LAST.
#   2. dotenvy does not override pre-set variables. If DEEPSEEK_BASE_URL or
#      DEEPSEEK_MODEL are exported as empty strings in the parent shell, the
#      app inherits "" and never falls back to its own defaults. The `:-`
#      assignments below re-fill them, since `:-` treats empty as unset.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_ENV="${BACKEND_ENV:-$REPO_DIR/.env}"
CENTRAL_ENV="${CENTRAL_ENV:-$HOME/.hermes/.env}"

for f in "$BACKEND_ENV" "$CENTRAL_ENV"; do
  if [ ! -f "$f" ]; then
    echo "run.sh: env file not found: $f" >&2
    exit 1
  fi
done

set -a
# shellcheck disable=SC1090
. "$BACKEND_ENV"
# shellcheck disable=SC1090
. "$CENTRAL_ENV"
set +a

: "${DEEPSEEK_API_KEY:?is empty — expected in $CENTRAL_ENV}"
export DEEPSEEK_BASE_URL="${DEEPSEEK_BASE_URL:-https://api.deepseek.com}"
export DEEPSEEK_MODEL="${DEEPSEEK_MODEL:-deepseek-chat}"

if [ "${1:-}" = "--build" ]; then
  shift
  (cd "$REPO_DIR" && cargo build)
elif [ -x "$REPO_DIR/target/debug/jobify-backend" ] &&
  [ -n "$(find "$REPO_DIR/src" -name '*.rs' -newer "$REPO_DIR/target/debug/jobify-backend" 2>/dev/null | head -1)" ]; then
  # This script runs the existing binary unless --build is passed, so a plain restart
  # can silently serve stale code: the server reports healthy while running an older
  # build, and any test against it validates the wrong code. Warn instead.
  echo "run.sh: WARNING the binary is OLDER than src/*.rs — run './run.sh --build'." >&2
fi

cd "$REPO_DIR"
echo "run.sh: DEEPSEEK_MODEL=$DEEPSEEK_MODEL key_len=${#DEEPSEEK_API_KEY} port=${PORT:-3000}"
exec ./target/debug/jobify-backend "$@"
