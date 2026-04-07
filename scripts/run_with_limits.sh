#!/usr/bin/env bash
set -euo pipefail

MEMORY_LIMIT_MB="${DIET_PYTHON_MEMORY_LIMIT_MB:-8192}"
TIMEOUT_SECS="${DIET_PYTHON_TIMEOUT_SECS:-180}"
CPUSET="${DIET_PYTHON_CPUSET:-}"
RUNTIME_DIR="${DIET_PYTHON_SYSTEMD_RUNTIME_DIR:-/run/user/$(id -u)}"
BUS_ADDRESS="unix:path=${RUNTIME_DIR}/bus"

if ! [[ "$MEMORY_LIMIT_MB" =~ ^[0-9]+$ ]]; then
  echo "invalid memory limit '$MEMORY_LIMIT_MB' (expected integer MiB)" >&2
  exit 2
fi
if ! [[ "$TIMEOUT_SECS" =~ ^[0-9]+$ ]]; then
  echo "invalid timeout '$TIMEOUT_SECS' (expected integer seconds)" >&2
  exit 2
fi

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <command> [args...]" >&2
  exit 2
fi

CMD=("$@")

if ! command -v systemd-run >/dev/null 2>&1; then
  echo "systemd-run not found but test limits require it" >&2
  exit 2
fi

if [ ! -S "${RUNTIME_DIR}/bus" ]; then
  echo "user systemd bus not available at ${RUNTIME_DIR}/bus; cannot apply test limits without prompting" >&2
  exit 2
fi

CMD=(
  env
  "XDG_RUNTIME_DIR=${RUNTIME_DIR}"
  "DBUS_SESSION_BUS_ADDRESS=${BUS_ADDRESS}"
  systemd-run
  --user
  --scope
  --quiet
  --no-ask-password
)

if [ -n "$CPUSET" ]; then
  echo "Restricting build/test execution to CPU set: $CPUSET" >&2
  CMD+=(-p "AllowedCPUs=$CPUSET")
fi

if [ "$MEMORY_LIMIT_MB" -gt 0 ]; then
  echo "Applying cgroup memory cap: ${MEMORY_LIMIT_MB} MiB" >&2
  CMD+=(-p "MemoryMax=${MEMORY_LIMIT_MB}M")
fi

if [ "$TIMEOUT_SECS" -gt 0 ]; then
  echo "Applying cgroup wall-clock timeout: ${TIMEOUT_SECS}s" >&2
  CMD+=(-p "RuntimeMaxSec=${TIMEOUT_SECS}s")
fi

CMD+=(-- "$@")

exec "${CMD[@]}"
