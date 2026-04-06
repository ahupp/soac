#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <absolute-url>" >&2
  exit 2
fi

OPEN_URL="$1"

open_browser() {
  local open_url="$1"
  echo "[3/3] Opening browser..."
  if command -v open >/dev/null 2>&1; then
    open "$open_url" >/dev/null 2>&1 || true
  elif command -v xdg-open >/dev/null 2>&1; then
    xdg-open "$open_url" >/dev/null 2>&1 || true
  else
    echo "No browser opener found. Open this URL manually: $open_url"
  fi
}

inspector_healthcheck() {
  curl -fsS "$URL/api/inspect_pipeline" \
    -H 'content-type: application/json' \
    -d '{"source":"def classify(n):\n    return n\n"}' >/dev/null 2>&1
}

if ss -ltnH "( sport = :$PORT )" | grep -q .; then
  if inspector_healthcheck; then
    echo "[2/3] Reusing existing web inspector at $URL ..."
    open_browser "$OPEN_URL"
    echo "Serving $URL (opened $OPEN_URL)."
    exit 0
  fi

  echo "Port $PORT is already in use, but the existing listener is not a healthy soac-inspector server." >&2
  ss -ltnp "( sport = :$PORT )" >&2 || true
  exit 1
fi

echo "[2/3] Starting web server in $WEB_DIR on $URL ..."

cd "$REPO_ROOT"
HOST="$HOST" PORT="$PORT" "$INSPECTOR_BIN" &
SERVER_PID=$!

cleanup() {
  if kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

sleep 0.5

if ! kill -0 "$SERVER_PID" >/dev/null 2>&1; then
  echo "Web inspector server exited before startup." >&2
  wait "$SERVER_PID"
fi

open_browser "$OPEN_URL"

echo "Serving $URL (opened $OPEN_URL, pid=$SERVER_PID). Press Ctrl+C to stop."
wait "$SERVER_PID"
