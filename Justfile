set shell := ["bash", "-euo", "pipefail", "-c"]
set positional-arguments

repo_root := justfile_directory()
cpython_bin := repo_root + "/vendor/cpython/python"
cpython_lib_dir := repo_root + "/vendor/cpython"
venv_dir := repo_root + "/.venv"
uv_cache_dir := repo_root + "/.uv-cache"
cargo_home := env_var_or_default("CARGO_HOME", repo_root + "/tmp/cargo-home")
pyo3_python := cpython_bin
web_dir := repo_root + "/web"
inspector_bin := repo_root + "/target/debug/soac-inspector"
port := env_var_or_default("PORT", "8000")
host := env_var_or_default("HOST", "127.0.0.1")
url := "http://" + host + ":" + port
limit_wrapper := repo_root + "/scripts/run_with_limits.sh"

export REPO_ROOT := repo_root
export CPYTHON_BIN := cpython_bin
export CPYTHON_LIB_DIR := cpython_lib_dir
export VENV_DIR := venv_dir
export UV_CACHE_DIR := uv_cache_dir
export UV_PYTHON := cpython_bin
export PYO3_PYTHON := pyo3_python
export PYO3_PYTHON_REAL := pyo3_python
export CARGO_HOME := cargo_home
export WEB_DIR := web_dir
export INSPECTOR_BIN := inspector_bin
export PORT := port
export HOST := host
export URL := url
export LIMIT_WRAPPER := limit_wrapper

[private]
ensure-cpython-checkout:
  #!/usr/bin/env bash
  if [[ ! -d "$REPO_ROOT/vendor/cpython" ]]; then
    echo "cpython checkout not found at $REPO_ROOT/vendor/cpython" >&2
    exit 1
  fi

[private]
ensure-shared-python: ensure-cpython
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  if [[ "$("$CPYTHON_BIN" -c 'import sysconfig; print(sysconfig.get_config_var("Py_ENABLE_SHARED") or 0)')" != "1" ]]; then
    echo "vendored CPython at $CPYTHON_BIN is not built with --enable-shared; run 'just build-python'" >&2
    exit 1
  fi

build-python: ensure-cpython-checkout
  #!/usr/bin/env bash
  set -euo pipefail
  cd "$REPO_ROOT/vendor/cpython"
  make clean && LDFLAGS="-Wl,-rpath,'\$\$ORIGIN'" ./configure --enable-shared --enable-optimizations
  make -j"$(nproc)"

[private]
ensure-cpython: ensure-cpython-checkout
  #!/usr/bin/env bash
  if [[ ! -x "$CPYTHON_BIN" ]]; then
    echo "python not found in $CPYTHON_BIN" >&2
    exit 1
  fi

[private]
ensure-venv:
  #!/usr/bin/env bash
  if [[ ! -x "$VENV_DIR/bin/python" ]]; then
    echo "venv not found at $VENV_DIR; run 'just update-venv' first" >&2
      exit 1
  fi

[private]
uninstall-extension: ensure-venv
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  SITE_PACKAGES="$("$VENV_DIR/bin/python" -c 'import sysconfig; print(sysconfig.get_path("platlib"))')"
  if [[ -d "$SITE_PACKAGES" ]]; then
    find "$SITE_PACKAGES" -maxdepth 1 -type f -name '_soac_ext*.so' -delete
    find "$SITE_PACKAGES" -maxdepth 1 -type l -name '_soac_ext*.so' -delete
  fi

[private]
install-extension build="debug": ensure-venv ensure-cpython
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  BUILD="{{build}}"

  if [[ "$BUILD" != "debug" && "$BUILD" != "release" ]]; then
    echo "build must be 'debug' or 'release'" >&2
    exit 2
  fi

  if [[ "$BUILD" == "release" ]]; then
    ARTIFACT_DIR="$REPO_ROOT/target/release"
  else
    ARTIFACT_DIR="$REPO_ROOT/target/debug"
  fi

  SOURCE_EXT="$ARTIFACT_DIR/lib_soac_ext.so"
  if [[ ! -f "$SOURCE_EXT" ]]; then
    echo "extension not found at $SOURCE_EXT" >&2
    exit 1
  fi

  SITE_PACKAGES="$("$VENV_DIR/bin/python" -c 'import sysconfig; print(sysconfig.get_path("platlib"))')"
  EXT_SUFFIX="$("$VENV_DIR/bin/python" -c 'import importlib.machinery; print(importlib.machinery.EXTENSION_SUFFIXES[0])')"
  TARGET_EXT="$SITE_PACKAGES/_soac_ext$EXT_SUFFIX"

  mkdir -p "$SITE_PACKAGES"
  just uninstall-extension
  ln -sf "$SOURCE_EXT" "$TARGET_EXT"

update-venv: ensure-cpython
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  rm -rf "$VENV_DIR"
  uv venv --python "$CPYTHON_BIN" "$VENV_DIR"

  (
    cd "$REPO_ROOT"
    VIRTUAL_ENV="$VENV_DIR" PATH="$VENV_DIR/bin:$PATH" \
      uv sync --project "$REPO_ROOT/soac_py" --group dev --frozen --active
  )

build-extension build="debug": ensure-cpython
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  BUILD="{{build}}"

  if [[ "$BUILD" != "debug" && "$BUILD" != "release" ]]; then
    echo "build must be 'debug' or 'release'" >&2
    exit 2
  fi

  if [[ "$BUILD" == "release" ]]; then
    BUILD_ARGS=(--release)
  else
    BUILD_ARGS=()
  fi

  (
    cd "$REPO_ROOT"
    cargo build --quiet "${BUILD_ARGS[@]}" -p soac-pyo3
  )
  just install-extension "$BUILD"

build-all: (update-venv) ensure-cpython ensure-shared-python
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"
  cargo build --quiet --workspace --tests
  just build-extension debug
  just build-web-inspector-server



run-cpython-tests jobs="0" *args='': build-all ensure-cpython ensure-venv
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  TEST_JOBS="{{jobs}}"
  if ! [[ "$TEST_JOBS" =~ ^[0-9]+$ ]]; then
    echo "invalid jobs '$TEST_JOBS' (expected non-negative integer)" >&2
    exit 2
  fi

  set -- {{args}}

  export SOURCE_DATE_EPOCH="$(date +%s)"
  VENV_SITE_PACKAGES="$("$VENV_DIR/bin/python" -c 'import sysconfig; print(sysconfig.get_path("platlib"))')"

  # Regrtest must run the vendored CPython interpreter from the source tree so
  # stdlib modules resolve from vendor/cpython/Lib. The extension itself is
  # explicitly installed into the repo venv and added to PYTHONPATH below.
  PYTHON_BIN="$CPYTHON_BIN"
  PYTHONPATH_PREFIX="$REPO_ROOT/vendor/cpython/Lib:$VENV_SITE_PACKAGES:$REPO_ROOT"
  SKIP_ARGS=()
  while IFS= read -r skip_id; do
    [ -n "$skip_id" ] && SKIP_ARGS+=(-x "$skip_id")
  done < <(
    SKIP_FILE="$REPO_ROOT/cpython_skipped_tests.txt" \
    EXPECTED_FAILURES_FILE="$REPO_ROOT/EXPECTED_FAILURE.md" \
    SKIP_EXPECTED_FAILURES="${SKIP_EXPECTED_FAILURES:-1}" \
    PYTHON_BIN="$PYTHON_BIN" \
    "$REPO_ROOT/scripts/collect_cpython_skip_ids.sh"
  )

  find "$REPO_ROOT/vendor/cpython" -name '*.pyc' -delete

  (
    cd "$REPO_ROOT/vendor/cpython"

    TEST_CMD=(
      "$LIMIT_WRAPPER"
      "$PYTHON_BIN"
      -m test "-j$TEST_JOBS" -v
      "${SKIP_ARGS[@]}"
      "$@"
    )

    DIET_PYTHON_INSTALL_HOOK=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONPATH="$PYTHONPATH_PREFIX${PYTHONPATH:+:$PYTHONPATH}" \
    "${TEST_CMD[@]}"
  )

build-web-inspector-server: ensure-cpython ensure-shared-python
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  TIMEFORMAT='[diet-python timing] build_web_inspector_server_s=%3R'
  time {
    cd "$REPO_ROOT"
    cargo build -p soac-inspector --bin soac-inspector
  }

build-web-inspector: build-web-inspector-server

history-metrics-report history_jsonl="logs/warloc_history.jsonl" daily_jsonl="logs/warloc_history_daily.jsonl" html_output="web/history_metrics.html" revset="..@": ensure-cpython
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"
  mkdir -p "$(dirname "{{history_jsonl}}")" "$(dirname "{{daily_jsonl}}")" "$(dirname "{{html_output}}")"
  "$CPYTHON_BIN" scripts/collect_warloc_history.py "{{history_jsonl}}" --revset "{{revset}}"
  "$CPYTHON_BIN" scripts/build_history_metrics_rollup.py "{{history_jsonl}}" "{{daily_jsonl}}" --html-output "{{html_output}}"

run-web-inspector: build-web-inspector
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  OPEN_URL="${URL}/?v=$(date +%s)"
  "$REPO_ROOT/scripts/open_web_url.sh" "$OPEN_URL"

view-speedscope profile="": build-web-inspector
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  PROFILE="{{profile}}"

  if [[ -z "$PROFILE" ]]; then
    PROFILE="$(
      find "$REPO_ROOT/logs" -maxdepth 1 -type f -name '*speedscope.json' -printf '%T@ %P\n' \
        | sort -nr \
        | head -n 1 \
        | cut -d' ' -f2-
    )"
    if [[ -z "$PROFILE" ]]; then
      echo "no speedscope profile found under $REPO_ROOT/logs" >&2
      exit 1
    fi
    PROFILE="logs/$PROFILE"
  fi

  PROFILE_URL="$("$CPYTHON_BIN" -c 'import pathlib, sys, urllib.parse; base_url = sys.argv[1]; profile_arg = sys.argv[2]; repo_root = pathlib.Path.cwd().resolve(); profile_path = pathlib.Path(profile_arg); rel_path = profile_path.resolve().relative_to(repo_root) if profile_path.is_absolute() else pathlib.Path(profile_arg); print(f"{base_url}/api/speedscope_profile?path={urllib.parse.quote(str(rel_path))}")' "$URL" "$PROFILE")"

  OPEN_URL="$("$CPYTHON_BIN" -c 'import pathlib, sys, urllib.parse; base_url = sys.argv[1]; profile_url = sys.argv[2]; profile_arg = sys.argv[3]; title = pathlib.Path(profile_arg).name; print(base_url + "/speedscope/#profileURL=" + urllib.parse.quote(profile_url, safe="") + "&title=" + urllib.parse.quote(title, safe=""))' "$URL" "$PROFILE_URL" "$PROFILE")"

  "$REPO_ROOT/scripts/open_web_url.sh" "$OPEN_URL"

perf-pystone-jit-warm loops="500000" output_prefix="logs/pystone_jit_perf_warm": ensure-cpython
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  mkdir -p logs
  mkdir -p "$REPO_ROOT/tmp"

  LOOPS="{{loops}}"
  OUTPUT_PREFIX="{{output_prefix}}"
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  PERF_FREQUENCY="${PERF_FREQUENCY:-999}"
  PERF_CALL_GRAPH="${PERF_CALL_GRAPH:-dwarf,16384}"
  PERF_PERCENT_LIMIT="${PERF_PERCENT_LIMIT:-0.5}"
  PERF_BUILDID_DIR="${PERF_BUILDID_DIR:-$REPO_ROOT/tmp/perf-buildid}"

  mkdir -p "${PERF_BUILDID_DIR}"

  PERF_DATA_BASENAME="$(basename "${OUTPUT_PREFIX}").data"
  PERF_DATA="$REPO_ROOT/tmp/${PERF_DATA_BASENAME}"
  RUN_LOG="${OUTPUT_PREFIX}.log"
  PERF_RECORD_LOG="${OUTPUT_PREFIX}_record.txt"
  REPORT_SYMBOLS="${OUTPUT_PREFIX}_report.txt"
  REPORT_DSO="${OUTPUT_PREFIX}_by_dso.txt"
  REPORT_DSO_SYMBOLS="${OUTPUT_PREFIX}_by_dso_symbol.txt"
  REPORT_CALLGRAPH="${OUTPUT_PREFIX}_callgraph.txt"
  REPORT_SPEEDSCOPE="${OUTPUT_PREFIX}_speedscope.json"
  PYO3_RELEASE_LIB="$REPO_ROOT/target/release/lib_soac_ext.so"
  PYO3_STAGING_DIR="$(mktemp -d)"
  READY_FILE="$(mktemp "$REPO_ROOT/tmp/pystone_jit_perf_ready.XXXXXX")"
  PYTHONPATH_PREFIX="${REPO_ROOT}:${REPO_ROOT}/soac_py/src:${REPO_ROOT}/scripts:${PYO3_STAGING_DIR}"
  PY_PID=""
  PERF_PID=""

  cleanup() {
    if [[ -n "${PERF_PID}" ]] && kill -0 "${PERF_PID}" >/dev/null 2>&1; then
      kill -INT "${PERF_PID}" >/dev/null 2>&1 || true
      wait "${PERF_PID}" >/dev/null 2>&1 || true
    fi
    if [[ -n "${PY_PID}" ]] && kill -0 "${PY_PID}" >/dev/null 2>&1; then
      kill "${PY_PID}" >/dev/null 2>&1 || true
      wait "${PY_PID}" >/dev/null 2>&1 || true
    fi
    rm -rf "$PYO3_STAGING_DIR"
    rm -f "$READY_FILE"
  }
  trap cleanup EXIT

  if ! command -v perf >/dev/null 2>&1; then
    echo "perf is required but was not found on PATH" >&2
    exit 1
  fi
  echo "date: $(date +%F)"
  echo "warmup loops: ${WARMUP_LOOPS}"
  echo "profile loops: ${LOOPS}"
  echo "perf data: ${PERF_DATA}"
  echo "run log: ${RUN_LOG}"
  echo "perf record log: ${PERF_RECORD_LOG}"
  echo "report: ${REPORT_SYMBOLS}"
  echo "report by dso: ${REPORT_DSO}"
  echo "report by dso/symbol: ${REPORT_DSO_SYMBOLS}"
  echo "report callgraph: ${REPORT_CALLGRAPH}"
  echo "report speedscope: ${REPORT_SPEEDSCOPE}"
  echo "perf buildid dir: ${PERF_BUILDID_DIR}"

  cd "$REPO_ROOT"
  cargo build --release

  if [ ! -f "$PYO3_RELEASE_LIB" ]; then
    echo "release extension not found at ${PYO3_RELEASE_LIB}" >&2
    exit 1
  fi

  ln -sf "$PYO3_RELEASE_LIB" "$PYO3_STAGING_DIR/_soac_ext.so"

  rm -f "${READY_FILE}"
  env \
    LOOPS="${LOOPS}" \
    WARMUP_LOOPS="${WARMUP_LOOPS}" \
    PERF_BUILDID_DIR="${PERF_BUILDID_DIR}" \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONPATH="${PYTHONPATH_PREFIX}${PYTHONPATH:+:${PYTHONPATH}}" \
    READY_FILE="${READY_FILE}" \
    "$CPYTHON_BIN" -c 'import os, signal; from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); open(os.environ["READY_FILE"], "w").write("ready\n"); os.kill(os.getpid(), signal.SIGSTOP); pystone.main(loops)' \
    >"${RUN_LOG}" 2>&1 &
  PY_PID=$!

  for _ in $(seq 1 400); do
    if [[ -f "${READY_FILE}" ]]; then
      break
    fi
    if ! kill -0 "${PY_PID}" >/dev/null 2>&1; then
      wait "${PY_PID}"
      echo "python exited before signaling readiness" >&2
      exit 1
    fi
    sleep 0.05
  done

  if [[ ! -f "${READY_FILE}" ]]; then
    echo "timed out waiting for python warmup readiness" >&2
    exit 1
  fi

  for _ in $(seq 1 200); do
    if [[ "$(ps -o state= -p "${PY_PID}" 2>/dev/null | tr -d ' ')" == T* ]]; then
      break
    fi
    if ! kill -0 "${PY_PID}" >/dev/null 2>&1; then
      wait "${PY_PID}"
      echo "python exited before stopping for perf attach" >&2
      exit 1
    fi
    sleep 0.01
  done

  if [[ "$(ps -o state= -p "${PY_PID}" 2>/dev/null | tr -d ' ')" != T* ]]; then
    echo "python never entered SIGSTOP state for perf attach" >&2
    exit 1
  fi

  perf record \
    --call-graph "${PERF_CALL_GRAPH}" \
    -F "${PERF_FREQUENCY}" \
    -o "${PERF_DATA}" \
    -p "${PY_PID}" \
    >"${PERF_RECORD_LOG}" 2>&1 &
  PERF_PID=$!

  for _ in $(seq 1 100); do
    if ! kill -0 "${PERF_PID}" >/dev/null 2>&1; then
      wait "${PERF_PID}"
      cat "${PERF_RECORD_LOG}" >&2
      echo "perf exited before the benchmark resumed" >&2
      exit 1
    fi
    sleep 0.01
  done

  kill -CONT "${PY_PID}"
  wait "${PY_PID}"
  PY_STATUS=$?
  PY_PID=""

  if kill -0 "${PERF_PID}" >/dev/null 2>&1; then
    kill -INT "${PERF_PID}" >/dev/null 2>&1 || true
  fi
  wait "${PERF_PID}"
  PERF_STATUS=$?
  PERF_PID=""

  if [[ ${PY_STATUS} -ne 0 ]]; then
    cat "${RUN_LOG}" >&2
    exit "${PY_STATUS}"
  fi
  if [[ ${PERF_STATUS} -ne 0 && ${PERF_STATUS} -ne 130 ]]; then
    cat "${PERF_RECORD_LOG}" >&2
    exit "${PERF_STATUS}"
  fi

  perf report \
    --stdio \
    --percent-limit "${PERF_PERCENT_LIMIT}" \
    --sort overhead,symbol \
    -i "${PERF_DATA}" \
    >"${REPORT_SYMBOLS}"

  perf report \
    --stdio \
    --percent-limit "${PERF_PERCENT_LIMIT}" \
    --sort overhead,dso \
    -i "${PERF_DATA}" \
    >"${REPORT_DSO}"

  perf report \
    --stdio \
    --percent-limit "${PERF_PERCENT_LIMIT}" \
    --sort overhead,dso,symbol \
    -i "${PERF_DATA}" \
    >"${REPORT_DSO_SYMBOLS}"

  perf report \
    --stdio \
    --percent-limit "${PERF_PERCENT_LIMIT}" \
    --sort overhead,symbol \
    --children \
    --call-graph graph,0.5,caller \
    -i "${PERF_DATA}" \
    >"${REPORT_CALLGRAPH}"

  perf report \
    --stdio \
    --stdio-color never \
    --no-children \
    --show-nr-samples \
    --percent-limit 0 \
    --sort overhead,dso,symbol \
    --call-graph flat,0,caller,count \
    -i "${PERF_DATA}" \
    | python3 "$REPO_ROOT/scripts/perf_report_to_speedscope.py" "$(basename "${OUTPUT_PREFIX}")" \
    >"${REPORT_SPEEDSCOPE}"

  VIEW_SPEEDSCOPE_PROFILE="$("$CPYTHON_BIN" -c 'import pathlib, sys; repo_root = pathlib.Path(sys.argv[1]).resolve(); report_path = pathlib.Path(sys.argv[2]).resolve(); print(report_path.relative_to(repo_root))' "$REPO_ROOT" "$REPORT_SPEEDSCOPE")"

  echo "finished"
  echo "view speedscope: just view-speedscope ${VIEW_SPEEDSCOPE_PROFILE@Q}"

perf-pystone-jit-specialized loops="500000" output_prefix="logs/pystone_jit_perf_warm_specialized": ensure-cpython (update-venv) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  SPECIALIZATION_PROFILE_LOOPS="${SPECIALIZATION_PROFILE_LOOPS:-8000000}"

  cd "$REPO_ROOT"
  counter_dump_path="$(mktemp "${TMPDIR:-/tmp}/soac_perf_call_targets_XXXXXX.bin")"
  trap 'rm -f "$counter_dump_path"' EXIT

  LOOPS="${SPECIALIZATION_PROFILE_LOOPS}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
  DIET_PYTHON_CALL_TARGET_COUNTERS=1 \
  DIET_PYTHON_COUNTERS_OUTPUT_FILE="$counter_dump_path" \
    "$REPO_ROOT/.venv/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)' >/tmp/soac_perf_specialization_profile.out 2>&1

  DIET_PYTHON_COUNTERS_FILE="$counter_dump_path" \
    just perf-pystone-jit-warm "{{loops}}" "{{output_prefix}}"

[private]
_pytest-run *args='': ensure-venv
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  # This is the authoritative transformed-runtime pytest entrypoint.
  # Prefer `just pytest ...` over invoking `python -m pytest` directly:
  # the recipe selects the interpreter/environment that can import the
  # built `_soac_ext` extension and applies the expected test settings.

  if [ "$#" -eq 0 ]; then
    "$VENV_DIR/bin/python" -m pytest --help
    exit 0
  fi

  export RUST_LOG="${RUST_LOG:-soac_jit=info}"
  # Repo tests are written around transforming integration modules and the
  # modules they explicitly opt into. Rewriting pytest/stdlib imports here
  # adds noise and teardown-only failures without improving coverage.
  export DIET_PYTHON_INTEGRATION_ONLY="${DIET_PYTHON_INTEGRATION_ONLY:-1}"
  PYTEST_TB=native

  TMP_PYTEST_OUTPUT="$(mktemp -t diet-python-pytest.XXXXXX.log)"
  TEST_CMD=(
    "$VENV_DIR/bin/python"
    "$REPO_ROOT/scripts/run_pytest_parallel.py"
    "$@"
  )

  set +e
  TIMEFORMAT='[diet-python timing] pytest_s=%3R'
  DIET_PYTHON_TIMEOUT_SECS="${DIET_PYTHON_TIMEOUT_SECS:-45}" \
  time "${TEST_CMD[@]}" 2>&1 | tee "$TMP_PYTEST_OUTPUT"
  TEST_STATUS=${PIPESTATUS[0]}
  set -e

  rm -f "$TMP_PYTEST_OUTPUT"
  exit "$TEST_STATUS"

pytest *args='': build-all
  #!/usr/bin/env bash
  just _pytest-run "$@"

py *args='': build-all
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  # Authoritative ad-hoc transformed-runtime Python entrypoint.
  # Prefer this over invoking `.venv/bin/python` or `vendor/cpython/python`
  # directly when you need the built extension/import-hook path.
  set -- {{args}}
  "$VENV_DIR/bin/python" "$@"

cpython *args='':
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  # Raw CPython entrypoint for debugging vendored-CPython behavior without
  # relying on the transformed-runtime environment from `just py`.
  set -- {{args}}
  "$REPO_ROOT/vendor/cpython/python" "$@"

fmt-markdown:
  #!/usr/bin/env bash
  cd "$REPO_ROOT"

  mapfile -d '' markdown_files < <(
    find . \
      -path './.jj' -prune -o \
      -path './.venv' -prune -o \
      -path './target' -prune -o \
      -path './vendor' -prune -o \
      -name '*.md' -print0
  )

  if [[ "${#markdown_files[@]}" -eq 0 ]]; then
    exit 0
  fi

  npx prettier --write "${markdown_files[@]}"


regen-snapshots:
  #!/usr/bin/env bash
  cd "$REPO_ROOT"
  cargo run --quiet --bin regen_snapshots

[private]
_test-all-test-phase:
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  run_parallel_step() {
    local pid_var="$1"
    local log_path="$2"
    shift 2
    (
      local timing_name="$1"
      shift
      local start_s end_s elapsed_s status
      start_s="$(date +%s.%N)"
      set +e
      "$@"
      status=$?
      set -e
      end_s="$(date +%s.%N)"
      elapsed_s="$(awk -v start="$start_s" -v end="$end_s" 'BEGIN { printf "%.3f", end - start }')"
      printf '[diet-python timing] %s=%s\n' "$timing_name" "$elapsed_s"
      exit "$status"
    ) >"$log_path" 2>&1 &
    printf -v "$pid_var" '%s' "$!"
  }

  cat_parallel_log() {
    local label="$1"
    local log_path="$2"
    if [ -s "$log_path" ]; then
      echo "[diet-python test-all] output: $label"
      cat "$log_path"
    fi
  }

  overall_status=0
  parallel_log_dir="$(mktemp -d)"
  cargo_test_log="$parallel_log_dir/cargo-test.log"
  pytest_log="$parallel_log_dir/pytest.log"

  run_parallel_step cargo_test_pid "$cargo_test_log" cargo_test_s cargo test
  run_parallel_step pytest_pid "$pytest_log" pytest_s just _pytest-run tests/

  cargo_test_status=0
  pytest_status=0

  if wait "$cargo_test_pid"; then
    cargo_test_status=0
  else
    cargo_test_status=$?
  fi
  if wait "$pytest_pid"; then
    pytest_status=0
  else
    pytest_status=$?
  fi

  cat_parallel_log "cargo-test" "$cargo_test_log"
  cat_parallel_log "pytest" "$pytest_log"

  if [ "$cargo_test_status" -ne 0 ]; then
    echo "[diet-python test-all] step failed: cargo-test (exit $cargo_test_status)" >&2
    overall_status="$cargo_test_status"
  fi

  if [ "$pytest_status" -ne 0 ]; then
    echo "[diet-python test-all] step failed: pytest (exit $pytest_status)" >&2
    if [ "$overall_status" -eq 0 ]; then
      overall_status="$pytest_status"
    fi
  fi

  exit "$overall_status"

test-all:
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"
  just uninstall-extension
  TIMEFORMAT='[diet-python timing] build_all_s=%3R'
  if time just build-all; then
    :
  else
    status=$?
    echo "[diet-python test-all] step failed: build-all (exit $status)" >&2
    just uninstall-extension
    exit "$status"
  fi
  TIMEFORMAT='[diet-python timing] regen_snapshots_s=%3R'
  if time just regen-snapshots; then
    :
  else
    status=$?
    echo "[diet-python test-all] step failed: regen-snapshots (exit $status)" >&2
    just uninstall-extension
    exit "$status"
  fi
  TIMEFORMAT='[diet-python timing] test_phase_s=%3R'
  if time DIET_PYTHON_LIMITS_ALREADY_APPLIED=1 \
    DIET_PYTHON_TIMEOUT_SECS="${DIET_PYTHON_TEST_ALL_TIMEOUT_SECS:-0}" \
    "$LIMIT_WRAPPER" \
    just _test-all-test-phase; then
    :
  else
    status=$?
    echo "[diet-python test-all] step failed: test-phase (exit $status)" >&2
    just uninstall-extension
    exit "$status"
  fi

  just uninstall-extension
  exit 0

_call-target-specializations-from-dump dump_path:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "$REPO_ROOT"
  cargo run -q -p soac-inspector --bin inspect_counters -- --specializations "{{dump_path}}"

benchmark-warm loops="8000000": (update-venv) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  echo "date: $(date +%F)"
  echo "loops: {{loops}}"
  echo "warmup loops: ${WARMUP_LOOPS}"

  cd "$REPO_ROOT"

  echo "jit transformed warm"
  LOOPS="{{loops}}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
    "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'

  echo "stock cpython"
  LOOPS="{{loops}}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
    "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'

benchmark loops="8000000": (update-venv) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  echo "date: $(date +%F)"
  echo "loops: {{loops}}"
  echo "warmup loops: ${WARMUP_LOOPS}"

  cd "$REPO_ROOT"
  counter_dump_path="$(mktemp "${TMPDIR:-/tmp}/soac_benchmark_call_targets_XXXXXX.bin")"
  trap 'rm -f "$counter_dump_path"' EXIT

  echo "jit transformed profile pass"
  LOOPS="{{loops}}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
  DIET_PYTHON_CALL_TARGET_COUNTERS=1 \
  DIET_PYTHON_COUNTERS_OUTPUT_FILE="$counter_dump_path" \
    "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'

  site_count="$(just _call-target-specializations-from-dump "$counter_dump_path" | awk -F';' 'NF { print NF }')"
  if [[ -n "$site_count" && "$site_count" != "0" ]]; then
    echo "jit transformed specialized pass (${site_count} callsites)"
    LOOPS="{{loops}}" \
    WARMUP_LOOPS="${WARMUP_LOOPS}" \
    DIET_PYTHON_COUNTERS_FILE="$counter_dump_path" \
      "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'
  else
    echo "jit transformed specialized pass (no hot callsites recorded)"
    LOOPS="{{loops}}" \
    WARMUP_LOOPS="${WARMUP_LOOPS}" \
      "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'
  fi

  echo "stock cpython"
  LOOPS="{{loops}}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
    "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'
