#!/usr/bin/env bash
# Start the API on HOST:PORT (default 0.0.0.0:3000).
#
# Pass --build after changing Rust: a plain restart executes the existing binary and will
# happily serve stale code while /health reports healthy.
set -euo pipefail
cd "$(dirname "$0")"

if [[ "${1:-}" == "--build" ]] || [[ ! -x target/debug/jobify-backend ]]; then
    cargo build
elif [[ -n "$(find src Cargo.toml -newer target/debug/jobify-backend -print -quit 2>/dev/null)" ]]; then
    echo "warning: src is newer than the binary — run ./run.sh --build" >&2
fi

exec ./target/debug/jobify-backend
