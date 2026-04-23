#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

if [[ $# -gt 1 ]]; then
  echo "usage: $0 [work-dir]" >&2
  exit 2
fi

if [[ $# -eq 1 ]]; then
  WORK_DIR="$1"
  if [[ "$WORK_DIR" != /* ]]; then
    WORK_DIR="$REPO_ROOT/$WORK_DIR"
  fi
  mkdir -p "$WORK_DIR"
else
  mkdir -p "$REPO_ROOT/work/example"
  WORK_DIR="$(mktemp -d "$REPO_ROOT/work/example/profile-optimize-verify.XXXXXX")"
fi

if [[ -e "$WORK_DIR/profile.bin" || -e "$WORK_DIR/verify.bin" || -e "$WORK_DIR/modules" ]]; then
  echo "work dir already contains SOAC profile artifacts: $WORK_DIR" >&2
  exit 2
fi

export SOAC_WORK_DIR="$WORK_DIR"
export SOAC_MODULE_ENABLED="path:$SCRIPT_DIR"

echo "work dir: $SOAC_WORK_DIR"

echo "build release workspace"
cargo build --release --workspace

echo "profile"
SOAC_OPT_MODE=profile just py "$SCRIPT_DIR/specialization_demo.py"

echo "optimize"
"$REPO_ROOT/target/release/decide_optimizations" \
  --counters "$SOAC_WORK_DIR/profile.bin" \
  --out "$SOAC_WORK_DIR/modules"

echo "verify"
SOAC_OPT_MODE=verify just py "$SCRIPT_DIR/specialization_demo.py"

echo "dump verify counters"
"$REPO_ROOT/target/release/inspect_counters" \
  --pretty "$SOAC_WORK_DIR/verify.bin" | tee "$SOAC_WORK_DIR/verify_counters.txt"

echo "wrote profile: $SOAC_WORK_DIR/profile.bin"
echo "wrote verify:  $SOAC_WORK_DIR/verify.bin"
echo "wrote dump:    $SOAC_WORK_DIR/verify_counters.txt"
echo "module cache:   $SOAC_WORK_DIR/modules"
