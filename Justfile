set shell := ["bash", "-euo", "pipefail", "-c"]
set positional-arguments

repo_root := justfile_directory()
cpython_bin := repo_root + "/vendor/cpython/python"
cpython_lib_dir := repo_root + "/vendor/cpython"
venv_dir := repo_root + "/.venv"
uv_cache_dir := env_var_or_default("UV_CACHE_DIR", repo_root + "/.uv-cache")
uv_tool_dir := env_var_or_default("UV_TOOL_DIR", repo_root + "/.uv/tools")
uv_tool_bin_dir := env_var_or_default("UV_TOOL_BIN_DIR", repo_root + "/.uv/bin")
xdg_cache_home := env_var_or_default("XDG_CACHE_HOME", repo_root + "/.xdg/cache")
xdg_data_home := env_var_or_default("XDG_DATA_HOME", repo_root + "/.xdg/data")
xdg_runtime_dir := env_var_or_default("XDG_RUNTIME_DIR", repo_root + "/tmp")
cargo_home := env_var_or_default("CARGO_HOME", repo_root + "/tmp/cargo-home")
soac_module_cache_dir := env_var_or_default("SOAC_MODULE_CACHE_DIR", repo_root + "/soac-module-cache")
pyo3_python := cpython_bin
web_dir := repo_root + "/web"
inspector_bin := repo_root + "/target/debug/soac-inspector"
port := env_var_or_default("PORT", "8000")
host := env_var_or_default("HOST", "127.0.0.1")
url := "http://" + host + ":" + port
last_benchmark_counters_dir := repo_root + "/logs/last_benchmark_counters"
last_benchmark_counters := last_benchmark_counters_dir + "/profile.bin"

export REPO_ROOT := repo_root
export CPYTHON_BIN := cpython_bin
export CPYTHON_LIB_DIR := cpython_lib_dir
export VENV_DIR := venv_dir
export UV_CACHE_DIR := uv_cache_dir
export UV_TOOL_DIR := uv_tool_dir
export UV_TOOL_BIN_DIR := uv_tool_bin_dir
export UV_PYTHON_DOWNLOADS := "never"
export UV_PYTHON := cpython_bin
export XDG_CACHE_HOME := xdg_cache_home
export XDG_DATA_HOME := xdg_data_home
export XDG_RUNTIME_DIR := xdg_runtime_dir
export PYO3_PYTHON := pyo3_python
export PYO3_PYTHON_REAL := pyo3_python
export CARGO_HOME := cargo_home
export SOAC_MODULE_CACHE_DIR := soac_module_cache_dir
export PATH := uv_tool_bin_dir + ":" + env_var_or_default("PATH", "")
export WEB_DIR := web_dir
export INSPECTOR_BIN := inspector_bin
export PORT := port
export HOST := host
export URL := url
export LAST_BENCHMARK_COUNTERS_DIR := last_benchmark_counters_dir
export LAST_BENCHMARK_COUNTERS := last_benchmark_counters

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
  if [[ -f Makefile ]]; then
    make clean
  fi
  LDFLAGS="-Wl,-rpath,'\$\$ORIGIN'" \
  ./configure \
    --enable-shared \
    --enable-optimizations \
    --with-lto \
    CFLAGS_NODIST="-O3 -g -fno-omit-frame-pointer -fasynchronous-unwind-tables"
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
uninstall-extension:
  #!/usr/bin/env bash
  if [[ ! -x "$VENV_DIR/bin/python" ]]; then
    exit 0
  fi
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
    PROFILE_DIR="release-ext"
  else
    PROFILE_DIR="debug"
  fi
  ARTIFACT_DIR="$REPO_ROOT/target/$PROFILE_DIR"

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

setup-dev-env:
  #!/usr/bin/env bash
  set -euo pipefail

  link_shared_dir() {
    local local_path="$1"
    local shared_path="$2"
    local label="$3"
    local migrate_existing="${4:-0}"

    if [[ ! -d "$shared_path" ]]; then
      echo "$label shared directory not found at $shared_path" >&2
      exit 1
    fi

    local shared_real
    shared_real="$(realpath -m "$shared_path")"

    if [[ -L "$local_path" ]]; then
      local local_real
      local_real="$(realpath -m "$local_path")"
      if [[ "$local_real" == "$shared_real" ]]; then
        return
      fi
      if [[ "$migrate_existing" == "1" && -d "$local_path" ]]; then
        cp -a --no-clobber "$local_path/." "$shared_path/"
        rm -- "$local_path"
      else
        echo "$label is a symlink to $(readlink "$local_path"), expected $shared_path" >&2
        exit 1
      fi
    fi

    if [[ -e "$local_path" ]]; then
      if [[ -d "$local_path" && -z "$(find "$local_path" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
        rmdir "$local_path"
      elif [[ "$migrate_existing" == "1" && -d "$local_path" ]]; then
        case "$local_path" in
          "$REPO_ROOT"/*) ;;
          *)
            echo "refusing to migrate $label outside REPO_ROOT: $local_path" >&2
            exit 1
            ;;
        esac
        cp -a --no-clobber "$local_path/." "$shared_path/"
        chmod -R u+w "$local_path"
        rm -rf -- "$local_path"
      else
        echo "$label exists at $local_path and is not the expected shared-state symlink" >&2
        echo "move it aside or set SOAC_PARENT_REPO before running setup-dev-env in this worktree" >&2
        exit 1
      fi
    fi

    mkdir -p "$(dirname "$local_path")"
    ln -s "$shared_path" "$local_path"
  }

  if [[ -f "$REPO_ROOT/.jj/repo" ]]; then
    parent_repo="${SOAC_PARENT_REPO:-}"
    parent_repo_source="SOAC_PARENT_REPO"
    if [[ -z "$parent_repo" ]]; then
      parent_repo_source="$REPO_ROOT/.jj/repo"
      jj_repo_path="$(head -n 1 "$REPO_ROOT/.jj/repo")"
      if [[ -z "$jj_repo_path" ]]; then
        echo "setup-dev-env: cannot infer parent checkout from empty $REPO_ROOT/.jj/repo" >&2
        echo "set SOAC_PARENT_REPO to the parent checkout that owns vendor/cpython, bench, and shared offline caches" >&2
        exit 1
      fi

      case "$jj_repo_path" in
        /*) jj_repo_real="$jj_repo_path" ;;
        *) jj_repo_real="$REPO_ROOT/.jj/$jj_repo_path" ;;
      esac
      jj_repo_real="$(realpath -m "$jj_repo_real")"

      case "$jj_repo_real" in
        */.jj/repo) parent_repo="${jj_repo_real%/.jj/repo}" ;;
        *)
          echo "setup-dev-env: cannot infer parent checkout from $REPO_ROOT/.jj/repo" >&2
          echo ".jj/repo points to $jj_repo_real, not a checkout-local .jj/repo directory" >&2
          echo "set SOAC_PARENT_REPO to the parent checkout that owns vendor/cpython, bench, and shared offline caches" >&2
          exit 1
          ;;
      esac
    fi

    parent_repo="${parent_repo%/}"
    if [[ ! -d "$parent_repo" ]]; then
      echo "$parent_repo_source does not identify a directory: $parent_repo" >&2
      exit 1
    fi

    parent_repo="$(cd "$parent_repo" && pwd -P)"
    if [[ "$parent_repo" == "$(pwd -P)" ]]; then
      echo "$parent_repo_source identifies this worktree; set SOAC_PARENT_REPO to the parent checkout" >&2
      exit 1
    fi

    if [[ ! -d "$parent_repo/vendor/cpython" ]]; then
      echo "parent checkout is missing vendor/cpython: $parent_repo/vendor/cpython" >&2
      exit 1
    fi

    mkdir -p \
      "$parent_repo/bench" \
      "$parent_repo/.uv-cache" \
      "$parent_repo/.uv/tools" \
      "$parent_repo/.uv/bin" \
      "$parent_repo/.xdg/cache" \
      "$parent_repo/.xdg/data" \
      "$parent_repo/soac-module-cache" \
      "$parent_repo/tmp/cargo-home"

    link_shared_dir "$REPO_ROOT/vendor/cpython" "$parent_repo/vendor/cpython" "vendor/cpython"
    link_shared_dir "$REPO_ROOT/bench" "$parent_repo/bench" "bench" 1
    link_shared_dir "$REPO_ROOT/.uv-cache" "$parent_repo/.uv-cache" ".uv-cache" 1
    link_shared_dir "$REPO_ROOT/.uv" "$parent_repo/.uv" ".uv" 1
    link_shared_dir "$REPO_ROOT/.xdg" "$parent_repo/.xdg" ".xdg" 1
    link_shared_dir "$REPO_ROOT/soac-module-cache" "$parent_repo/soac-module-cache" "soac-module-cache" 1
    link_shared_dir "$REPO_ROOT/tmp/cargo-home" "$parent_repo/tmp/cargo-home" "tmp/cargo-home" 1
  else
    if [[ -L "$REPO_ROOT/bench" ]]; then
      echo "bench is a symlink in the parent checkout; replace it with a regular directory before running setup-dev-env" >&2
      exit 1
    fi
    mkdir -p "$REPO_ROOT/bench"
  fi

  mkdir -p \
    "$UV_CACHE_DIR" \
    "$UV_TOOL_DIR" \
    "$UV_TOOL_BIN_DIR" \
    "$XDG_CACHE_HOME" \
    "$XDG_DATA_HOME" \
    "$SOAC_MODULE_CACHE_DIR" \
    "$XDG_RUNTIME_DIR"

  if [[ ! -x "$CPYTHON_BIN" ]]; then
    echo "python not found in $CPYTHON_BIN; run setup-dev-env in the parent checkout or build Python there first" >&2
    exit 1
  fi

  rustup toolchain install nightly
  rustup component add rustc-codegen-cranelift-preview --toolchain nightly
  cargo install --locked inferno
  env -u UV_OFFLINE uv tool install ruff
  echo 'Run "apt update && apt install -y gdb"'
  env -u UV_OFFLINE just update-venv

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
  touch "$VENV_DIR/.soac-ready"

[private]
update-venv-offline:
  #!/usr/bin/env bash
  UV_OFFLINE=1 just update-venv

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
    BUILD_ARGS=(--profile release-ext)
  else
    BUILD_ARGS=()
  fi

  (
    cd "$REPO_ROOT"
    cargo build "${BUILD_ARGS[@]}" -p soac-pyo3
  )
  just install-extension "$BUILD"

build-test-runtime: (update-venv-offline) ensure-cpython ensure-shared-python
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"
  just build-extension debug

build-test-runtime-fast: ensure-cpython ensure-shared-python
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  VENV_STAMP="$VENV_DIR/.soac-ready"
  needs_venv_refresh=0
  if [[ ! -x "$VENV_DIR/bin/python" || ! -f "$VENV_STAMP" \
      || "$CPYTHON_BIN" -nt "$VENV_STAMP" \
      || "$REPO_ROOT/soac_py/pyproject.toml" -nt "$VENV_STAMP" ]]; then
    needs_venv_refresh=1
  elif [[ -f "$REPO_ROOT/uv.lock" && "$REPO_ROOT/uv.lock" -nt "$VENV_STAMP" ]]; then
    needs_venv_refresh=1
  fi

  if [[ "$needs_venv_refresh" -eq 1 ]]; then
    just update-venv-offline
  fi

  SOURCE_EXT="$REPO_ROOT/target/debug/lib_soac_ext.so"
  SITE_PACKAGES="$("$VENV_DIR/bin/python" -c 'import sysconfig; print(sysconfig.get_path("platlib"))')"
  EXT_SUFFIX="$("$VENV_DIR/bin/python" -c 'import importlib.machinery; print(importlib.machinery.EXTENSION_SUFFIXES[0])')"
  TARGET_EXT="$SITE_PACKAGES/_soac_ext$EXT_SUFFIX"

  needs_build=0
  if [[ ! -f "$SOURCE_EXT" ]]; then
    needs_build=1
  elif find "$REPO_ROOT" \
      -path "$REPO_ROOT/.jj" -prune -o \
      -path "$REPO_ROOT/.venv" -prune -o \
      -path "$REPO_ROOT/target" -prune -o \
      -path "$REPO_ROOT/vendor" -prune -o \
      -path "$REPO_ROOT/bench" -prune -o \
      -path "$REPO_ROOT/logs" -prune -o \
      \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'Cargo.lock' -o -name 'build.rs' \) \
      -newer "$SOURCE_EXT" -print -quit | grep -q .; then
    needs_build=1
  fi

  if [[ "$needs_build" -eq 1 ]]; then
    just build-extension debug
    exit 0
  fi

  if [[ ! -L "$TARGET_EXT" || "$(realpath -m "$TARGET_EXT")" != "$(realpath -m "$SOURCE_EXT")" ]]; then
    just install-extension debug
  fi

build-all: build-test-runtime
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"
  cargo build --workspace --tests
  just build-web-inspector-server



run-cpython-tests jobs="0" *args='': build-test-runtime ensure-cpython ensure-venv
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
  PYTHONPATH_PREFIX="$REPO_ROOT/vendor/cpython/Lib:$REPO_ROOT/soac_py/src:$VENV_SITE_PACKAGES:$REPO_ROOT"
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
      "$PYTHON_BIN"
      -m soac.import_hook test.__main__ "-j$TEST_JOBS" -v
      "${SKIP_ARGS[@]}"
      "$@"
    )

    PYTHONDONTWRITEBYTECODE=1 \
    SOAC_CRANELIFT_OPT_LEVEL="${SOAC_CRANELIFT_OPT_LEVEL:-none}" \
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

run-and-view-speedscope loops="10000000" counters_dir="" output_prefix="logs/pystone_jit_perf_warm_specialized_from_benchmark": ensure-cpython
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  COUNTERS_DIR="{{counters_dir}}"

  if [[ -z "$COUNTERS_DIR" ]]; then
    COUNTERS_DIR="$LAST_BENCHMARK_COUNTERS_DIR"
  fi
  COUNTERS_FILE="$COUNTERS_DIR/profile.bin"

  if [[ ! -f "$COUNTERS_FILE" ]]; then
    echo "counter profile not found at $COUNTERS_FILE; run 'just benchmark' first or pass counters_dir=<dir>" >&2
    exit 1
  fi

  cd "$REPO_ROOT"
  SOAC_WORK_DIR="$COUNTERS_DIR" \
  SOAC_OPT_MODE=apply \
    just perf-pystone-jit-warm "{{loops}}" "{{output_prefix}}"

  just view-speedscope "{{output_prefix}}_speedscope.json"

perf-pystone-jit-warm loops="10000000" output_prefix="logs/pystone_jit_perf_warm": ensure-cpython
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  mkdir -p logs
  mkdir -p "$REPO_ROOT/tmp"

  LOOPS="{{loops}}"
  OUTPUT_PREFIX="{{output_prefix}}"
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  PERF_FREQUENCY="${PERF_FREQUENCY:-999}"
  PERF_CALL_GRAPH="${PERF_CALL_GRAPH:-dwarf,65528}"
  PERF_PERCENT_LIMIT="${PERF_PERCENT_LIMIT:-0.5}"
  PERF_HELPER_FRAMES="${SOAC_JIT_PERF_HELPER_FRAMES:-1}"
  PERF_BUILDID_DIR="${PERF_BUILDID_DIR:-$REPO_ROOT/tmp/perf-buildid}"

  PERF_DATA_BASENAME="$(basename "${OUTPUT_PREFIX}").data"
  PERF_DATA="$REPO_ROOT/tmp/${PERF_DATA_BASENAME}"
  RUN_LOG="${OUTPUT_PREFIX}.log"
  PERF_RECORD_LOG="${OUTPUT_PREFIX}_record.txt"
  REPORT_SYMBOLS="${OUTPUT_PREFIX}_report.txt"
  REPORT_DSO="${OUTPUT_PREFIX}_by_dso.txt"
  REPORT_DSO_SYMBOLS="${OUTPUT_PREFIX}_by_dso_symbol.txt"
  REPORT_CALLGRAPH="${OUTPUT_PREFIX}_callgraph.txt"
  REPORT_SPEEDSCOPE="${OUTPUT_PREFIX}_speedscope.json"
  INJECTED_PERF_DATA="$REPO_ROOT/tmp/$(basename "${OUTPUT_PREFIX}").injected.data"
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
  if ! command -v inferno-collapse-perf >/dev/null 2>&1; then
    echo "inferno-collapse-perf is required but was not found on PATH; install it with: cargo install inferno" >&2
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
  echo "perf helper frames: ${PERF_HELPER_FRAMES}"

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
    SOAC_JIT_PERF_HELPER_FRAMES="${PERF_HELPER_FRAMES}" \
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
    -k 1 \
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

  perf inject --jit \
    -i "${PERF_DATA}" \
    -o "${INJECTED_PERF_DATA}"

  perf script \
    -i "${INJECTED_PERF_DATA}" \
    | inferno-collapse-perf \
    | python3 "$REPO_ROOT/scripts/folded_to_speedscope.py" "$(basename "${OUTPUT_PREFIX}")" \
    >"${REPORT_SPEEDSCOPE}"

  VIEW_SPEEDSCOPE_PROFILE="$("$CPYTHON_BIN" -c 'import pathlib, sys; repo_root = pathlib.Path(sys.argv[1]).resolve(); report_path = pathlib.Path(sys.argv[2]).resolve(); print(report_path.relative_to(repo_root))' "$REPO_ROOT" "$REPORT_SPEEDSCOPE")"

  echo "finished"
  echo "view speedscope: just view-speedscope ${VIEW_SPEEDSCOPE_PROFILE@Q}"

perf-pystone-jit-specialized loops="10000000" output_prefix="logs/pystone_jit_perf_warm_specialized": ensure-cpython (update-venv-offline) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  SPECIALIZATION_PROFILE_LOOPS="${SPECIALIZATION_PROFILE_LOOPS:-8000000}"

  cd "$REPO_ROOT"
  counters_dir="$(mktemp -d "${TMPDIR:-/tmp}/soac_perf_counters_XXXXXX")"
  trap 'rm -rf "$counters_dir"' EXIT

  LOOPS="${SPECIALIZATION_PROFILE_LOOPS}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
  SOAC_WORK_DIR="$counters_dir" \
  SOAC_OPT_MODE=profile \
    "$REPO_ROOT/.venv/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)' >/tmp/soac_perf_specialization_profile.out 2>&1

  SOAC_WORK_DIR="$counters_dir" \
  SOAC_OPT_MODE=apply \
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

  export SOAC_CRANELIFT_OPT_LEVEL="${SOAC_CRANELIFT_OPT_LEVEL:-none}"
  SOAC_PYTEST_EVENTS_LOG="${SOAC_PYTEST_EVENTS_LOG:-$REPO_ROOT/logs/pytest_events.jsonl}"
  if [[ -z "${SOAC_LOG:-}" && "${SOAC_PYTEST_TRACE:-0}" =~ ^(1|true|yes|on)$ ]]; then
    rm -f "$SOAC_PYTEST_EVENTS_LOG"
    export SOAC_LOG="soac_jit=info,soac_module_load=info,soac_jit_codegen=info;json=$SOAC_PYTEST_EVENTS_LOG"
  fi
  PYTEST_TB=native

  TMP_PYTEST_OUTPUT="$(mktemp -t diet-python-pytest.XXXXXX.log)"
  TEST_CMD=(
    "$VENV_DIR/bin/python"
    -u
    "$REPO_ROOT/scripts/run_pytest_parallel.py"
    "$@"
  )

  set +e
  TIMEFORMAT='[diet-python timing] pytest_s=%3R'
  time "${TEST_CMD[@]}" 2>&1 | tee "$TMP_PYTEST_OUTPUT"
  TEST_STATUS=${PIPESTATUS[0]}
  set -e

  rm -f "$TMP_PYTEST_OUTPUT"
  exit "$TEST_STATUS"

pytest *args='': build-test-runtime
  #!/usr/bin/env bash
  just _pytest-run "$@"

pytest-fast *args='': build-test-runtime-fast
  #!/usr/bin/env bash
  just _pytest-run "$@"

py *args='': build-test-runtime
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  # Authoritative ad-hoc transformed-runtime Python entrypoint.
  # Prefer this over invoking `.venv/bin/python` or `vendor/cpython/python`
  # directly when you need the built extension/import-hook path.
  set -- {{args}}
  "$VENV_DIR/bin/python" "$@"

ipython *args='': build-test-runtime
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  # Interactive optimization-inspection entrypoint. This uses the repo venv
  # and built debug extension so `%load_ext soac.ipython` works out of the box.
  set -- {{args}}
  "$VENV_DIR/bin/ipython" "$@"

py-fast *args='': build-test-runtime-fast
  #!/usr/bin/env bash
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  # Fast transformed-runtime Python entrypoint for tight edit/repro loops.
  # This reuses an existing venv and debug extension when the relevant
  # dependency and Rust inputs are unchanged, and falls back to the full
  # build path when they are stale or missing.
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

[private]
_test-all-test-phase:
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  line_buffer() {
    if command -v stdbuf >/dev/null 2>&1; then
      stdbuf -oL -eL "$@"
    else
      "$@"
    fi
  }

  run_parallel_step() {
    local pid_var="$1"
    local label="$2"
    local log_path="$3"
    local status_path="$4"
    shift 4
    (
      local timing_name="$1"
      shift
      local start_s end_s elapsed_s status
      start_s="$(date +%s.%N)"
      set +e
      line_buffer "$@" 2>&1 \
        | awk -v label="$label" '{ printf "[diet-python test-all][%s] %s\n", label, $0; fflush(); }' \
        | tee "$log_path"
      status=${PIPESTATUS[0]}
      set -e
      end_s="$(date +%s.%N)"
      elapsed_s="$(awk -v start="$start_s" -v end="$end_s" 'BEGIN { printf "%.3f", end - start }')"
      printf '%s\n' "$status" >"$status_path"
      printf '[diet-python timing] %s=%s\n' "$timing_name" "$elapsed_s"
      exit "$status"
    ) &
    printf -v "$pid_var" '%s' "$!"
  }

  overall_status=0
  parallel_log_dir="$(mktemp -d)"
  cargo_test_log="$parallel_log_dir/cargo-test.log"
  pytest_log="$parallel_log_dir/pytest.log"
  cargo_test_status_file="$parallel_log_dir/cargo-test.status"
  pytest_status_file="$parallel_log_dir/pytest.status"

  # Some soac-jit tests share CPython process state and JIT finalization state.
  # Keep the Rust harness serial here so the full gate is deterministic.
  run_parallel_step cargo_test_pid cargo-test "$cargo_test_log" "$cargo_test_status_file" cargo_test_s cargo test -- --test-threads=1
  run_parallel_step pytest_pid pytest "$pytest_log" "$pytest_status_file" pytest_s just _pytest-run tests/

  cargo_test_status=0
  pytest_status=0
  cargo_test_reported=0
  pytest_reported=0
  phase_start_s="$(date +%s)"
  next_progress_s="$phase_start_s"

  echo "[diet-python test-all] started: cargo-test (pid $cargo_test_pid)"
  echo "[diet-python test-all] started: pytest (pid $pytest_pid)"

  while [ ! -f "$cargo_test_status_file" ] || [ ! -f "$pytest_status_file" ]; do
    now_s="$(date +%s)"

    if [ "$cargo_test_reported" -eq 0 ] && [ -f "$cargo_test_status_file" ]; then
      cargo_test_reported=1
      echo "[diet-python test-all] completed: cargo-test"
    fi

    if [ "$pytest_reported" -eq 0 ] && [ -f "$pytest_status_file" ]; then
      pytest_reported=1
      echo "[diet-python test-all] completed: pytest"
    fi

    if [ ! -f "$cargo_test_status_file" ] || [ ! -f "$pytest_status_file" ]; then
      if [ "$now_s" -ge "$next_progress_s" ]; then
        running_steps=()
        if [ ! -f "$cargo_test_status_file" ]; then
          running_steps+=("cargo-test")
        fi
        if [ ! -f "$pytest_status_file" ]; then
          running_steps+=("pytest")
        fi
        elapsed_s=$(( now_s - phase_start_s ))
        echo "[diet-python test-all] still running after ${elapsed_s}s: ${running_steps[*]}"
        next_progress_s=$(( now_s + 10 ))
      fi
      sleep 1
    fi
  done

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

  cargo_test_status="$(cat "$cargo_test_status_file")"
  pytest_status="$(cat "$pytest_status_file")"

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
  TIMEFORMAT='[diet-python timing] build_test_runtime_s=%3R'
  if time just build-test-runtime; then
    :
  else
    status=$?
    echo "[diet-python test-all] step failed: build-test-runtime (exit $status)" >&2
    just uninstall-extension
    exit "$status"
  fi
  TIMEFORMAT='[diet-python timing] test_phase_s=%3R'
  if time just _test-all-test-phase; then
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
  cargo run -p soac-inspector --bin inspect_counters -- --specializations "{{dump_path}}"

benchmark-verify loops="100000" counters_dir="": (update-venv-offline) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  BENCHMARK_CPU="${BENCHMARK_CPU:-}"
  BENCHMARK_CONSTANT_CLOCKS="${BENCHMARK_CONSTANT_CLOCKS:-0}"
  COUNTERS_DIR="{{counters_dir}}"
  if [[ -z "$COUNTERS_DIR" ]]; then
    COUNTERS_DIR="$LAST_BENCHMARK_COUNTERS_DIR"
  fi
  if [[ ! -f "$COUNTERS_DIR/profile.bin" ]]; then
    echo "counter profile not found at $COUNTERS_DIR/profile.bin; run 'just benchmark' first or pass counters_dir=<dir>" >&2
    exit 1
  fi
  rm -f "$COUNTERS_DIR/events.jsonl"
  rm -f "$COUNTERS_DIR/verify.bin"

  echo "jit transformed verify pass"
  echo "loops: {{loops}}"
  echo "counters dir: $COUNTERS_DIR"
  LOOPS="{{loops}}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
  BENCHMARK_CPU="${BENCHMARK_CPU}" \
  BENCHMARK_CONSTANT_CLOCKS="${BENCHMARK_CONSTANT_CLOCKS}" \
  SOAC_WORK_DIR="$COUNTERS_DIR" \
  SOAC_OPT_MODE=verify \
    "$REPO_ROOT/scripts/run_benchmark_with_cpu_mode.sh" "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'
  echo "verification counters: $COUNTERS_DIR/verify.bin"
  echo "SOAC events log: $COUNTERS_DIR/events.jsonl"

benchmark-warm loops="8000000": (update-venv-offline) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  BENCHMARK_CPU="${BENCHMARK_CPU:-}"
  BENCHMARK_CONSTANT_CLOCKS="${BENCHMARK_CONSTANT_CLOCKS:-0}"
  echo "date: $(date +%F)"
  echo "loops: {{loops}}"
  echo "warmup loops: ${WARMUP_LOOPS}"
  echo "benchmark cpu: ${BENCHMARK_CPU}"
  echo "benchmark constant clocks: ${BENCHMARK_CONSTANT_CLOCKS}"

  cd "$REPO_ROOT"

  echo "jit transformed warm"
  SOAC_WARM_EVENTS_LOG="$REPO_ROOT/logs/benchmark_warm_events.jsonl"
  if [[ -z "${SOAC_LOG:-}" ]]; then
    rm -f "$SOAC_WARM_EVENTS_LOG"
    export SOAC_LOG="soac_jit=info,soac_module_load=info,soac_jit_codegen=info;json=$SOAC_WARM_EVENTS_LOG"
  fi
  LOOPS="{{loops}}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
  BENCHMARK_CPU="${BENCHMARK_CPU}" \
  BENCHMARK_CONSTANT_CLOCKS="${BENCHMARK_CONSTANT_CLOCKS}" \
    "$REPO_ROOT/scripts/run_benchmark_with_cpu_mode.sh" "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'

  echo "stock cpython"
  LOOPS="{{loops}}" \
  WARMUP_LOOPS="${WARMUP_LOOPS}" \
  BENCHMARK_CPU="${BENCHMARK_CPU}" \
  BENCHMARK_CONSTANT_CLOCKS="${BENCHMARK_CONSTANT_CLOCKS}" \
    "$REPO_ROOT/scripts/run_benchmark_with_cpu_mode.sh" "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'

benchmark benchmark_loops="1000000" verify_loops="100000" results_root="bench" result_mode="one-off": (update-venv-offline) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

  BENCHMARK_LOOPS="{{benchmark_loops}}"
  VERIFY_LOOPS="{{verify_loops}}"
  PROFILE_LOOPS="100000"
  SPECIALIZED_RUNS="${BENCHMARK_SPECIALIZED_RUNS:-3}"
  RESULTS_ROOT="{{results_root}}"
  RESULT_MODE="{{result_mode}}"
  CRANELIFT_OPT_LEVEL="${SOAC_CRANELIFT_OPT_LEVEL:-speed}"
  if [[ "$RESULTS_ROOT" != /* ]]; then
    RESULTS_ROOT="$REPO_ROOT/$RESULTS_ROOT"
  fi
  WARMUP_LOOPS="${WARMUP_LOOPS:-1000}"
  BENCHMARK_CPU="${BENCHMARK_CPU:-}"
  BENCHMARK_CONSTANT_CLOCKS="${BENCHMARK_CONSTANT_CLOCKS:-0}"

  # Snapshot the invoking jj workspace so the benchmark labels the exact tree it executes.
  jj status --no-pager >/dev/null

  change_id="$(jj --ignore-working-copy log -r @ --no-graph -T 'change_id.short()')"
  commit_id="$(jj --ignore-working-copy log -r @ --no-graph -T 'commit_id.short()')"
  case "$RESULT_MODE" in
    one-off)
      result_name="${change_id}_${commit_id}"
      ;;
    finalized)
      result_name="$change_id"
      ;;
    *)
      echo "unknown benchmark result mode: $RESULT_MODE" >&2
      echo "expected one-off or finalized" >&2
      exit 1
      ;;
  esac
  result_dir="$RESULTS_ROOT/$result_name"
  counters_dir="$result_dir/counters"
  report="$result_dir/benchmark.txt"

  rm -rf "$result_dir"
  mkdir -p "$counters_dir"
  SOAC_BENCHMARK_EVENTS_LOG="$counters_dir/events.jsonl"
  rm -f "$SOAC_BENCHMARK_EVENTS_LOG"

  {
    echo "result dir: $result_dir"
    echo "executed revision:"
    jj --ignore-working-copy log -r @ --no-graph \
      -T '"change_id " ++ change_id ++ "\ncommit_id " ++ commit_id ++ "\ndescription " ++ description.first_line() ++ "\n"'
    echo
    echo "date: $(date +%F)"
    echo "profile loops: $PROFILE_LOOPS"
    echo "benchmark loops: $BENCHMARK_LOOPS"
    echo "verify loops: $VERIFY_LOOPS"
    echo "result mode: $RESULT_MODE"
    echo "specialized runs: $SPECIALIZED_RUNS"
    echo "warmup loops: $WARMUP_LOOPS"
    echo "benchmark cpu: $BENCHMARK_CPU"
    echo "benchmark constant clocks: $BENCHMARK_CONSTANT_CLOCKS"
    echo "cranelift opt level: $CRANELIFT_OPT_LEVEL"
    echo "apply refcount modes: disabled, enabled"
    echo

    echo "jit transformed profile pass"
    LOOPS="$PROFILE_LOOPS" \
    WARMUP_LOOPS="$WARMUP_LOOPS" \
    BENCHMARK_CPU="$BENCHMARK_CPU" \
    BENCHMARK_CONSTANT_CLOCKS="$BENCHMARK_CONSTANT_CLOCKS" \
    SOAC_WORK_DIR="$counters_dir" \
    SOAC_CRANELIFT_OPT_LEVEL="$CRANELIFT_OPT_LEVEL" \
    SOAC_OPT_MODE=profile \
      "$REPO_ROOT/scripts/run_benchmark_with_cpu_mode.sh" "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'

    echo
    echo "jit transformed verify pass"
    LOOPS="$VERIFY_LOOPS" \
    WARMUP_LOOPS="$WARMUP_LOOPS" \
    BENCHMARK_CPU="$BENCHMARK_CPU" \
    BENCHMARK_CONSTANT_CLOCKS="$BENCHMARK_CONSTANT_CLOCKS" \
    SOAC_WORK_DIR="$counters_dir" \
    SOAC_CRANELIFT_OPT_LEVEL="$CRANELIFT_OPT_LEVEL" \
    SOAC_OPT_MODE=verify \
      "$REPO_ROOT/scripts/run_benchmark_with_cpu_mode.sh" "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'

    site_count="$(just _call-target-specializations-from-dump "$counters_dir/profile.bin" | awk -F';' 'NF { print NF }')"
    run_apply_pass() {
      local refcount_label="$1"
      local refcount_env="$2"
      echo
      echo "jit transformed specialized apply pass (${site_count:-0} callsites, refcounts $refcount_label)"
      for run in $(seq 1 "$SPECIALIZED_RUNS"); do
        echo "specialized run $run/$SPECIALIZED_RUNS"
        LOOPS="$BENCHMARK_LOOPS" \
        WARMUP_LOOPS="$WARMUP_LOOPS" \
        BENCHMARK_CPU="$BENCHMARK_CPU" \
        BENCHMARK_CONSTANT_CLOCKS="$BENCHMARK_CONSTANT_CLOCKS" \
        SOAC_WORK_DIR="$counters_dir" \
        SOAC_CRANELIFT_OPT_LEVEL="$CRANELIFT_OPT_LEVEL" \
        SOAC_OPT_MODE=apply \
        SOAC_JIT_EMIT_REFCOUNTS="$refcount_env" \
          "$REPO_ROOT/scripts/run_benchmark_with_cpu_mode.sh" "$VENV_DIR/bin/python" -c 'import os, sys; sys.path.insert(0, "scripts"); from soac.import_hook import install; install(); import pystone; warmup_loops = int(os.environ["WARMUP_LOOPS"]); loops = int(os.environ["LOOPS"]); warmup_loops > 0 and pystone.pystones(warmup_loops); pystone.main(loops)'
      done
    }
    run_apply_pass disabled 0
    run_apply_pass enabled 1
  } 2>&1 | tee "$report"

  python3 "$REPO_ROOT/scripts/summarize_benchmark_result.py" \
    "$result_dir" \
    --json-out "$result_dir/summary.json" \
    | tee "$result_dir/summary.txt" \
    | tee -a "$report" >/dev/null

  echo "benchmark result: $result_dir"

precompile-shared-library counters="" out="logs/libsoac_precompiled.so" object_dir="":
  #!/usr/bin/env bash
  set -euo pipefail
  cd "$REPO_ROOT"

  COUNTERS="{{counters}}"
  if [[ -z "$COUNTERS" ]]; then
    COUNTERS="$LAST_BENCHMARK_COUNTERS"
  elif [[ "$COUNTERS" != /* ]]; then
    COUNTERS="$REPO_ROOT/$COUNTERS"
  fi
  if [[ ! -f "$COUNTERS" ]]; then
    echo "counter profile not found at $COUNTERS; run 'just benchmark' first or pass counters=<profile.bin>" >&2
    exit 1
  fi

  OUT="{{out}}"
  if [[ "$OUT" != /* ]]; then
    OUT="$REPO_ROOT/$OUT"
  fi

  args=(--counters "$COUNTERS" --out "$OUT")
  OBJECT_DIR="{{object_dir}}"
  if [[ -n "$OBJECT_DIR" ]]; then
    if [[ "$OBJECT_DIR" != /* ]]; then
      OBJECT_DIR="$REPO_ROOT/$OBJECT_DIR"
    fi
    args+=(--object-dir "$OBJECT_DIR")
  fi

  cargo run -p soac-inspector --bin precompile_blockpy -- "${args[@]}"

[private]
_benchmark-export-specialized-artifacts result_dir:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "$REPO_ROOT"

  RESULT_DIR="{{result_dir}}"
  if [[ "$RESULT_DIR" != /* ]]; then
    RESULT_DIR="$REPO_ROOT/$RESULT_DIR"
  fi
  COUNTERS_DIR="$RESULT_DIR/counters"
  CLIF_DIR="$RESULT_DIR/clif"

  if [[ ! -f "$COUNTERS_DIR/profile.bin" ]]; then
    echo "counter profile not found at $COUNTERS_DIR/profile.bin; run 'just benchmark' first or pass result_dir=<dir>" >&2
    exit 1
  fi
  if [[ ! -f "$COUNTERS_DIR/verify.bin" ]]; then
    echo "verification counters not found at $COUNTERS_DIR/verify.bin; run 'just benchmark-verify' or 'just benchmark-deep-profile-from-profile' first" >&2
    exit 1
  fi

  rm -rf "$CLIF_DIR"
  mkdir -p "$CLIF_DIR"

  cargo run -p soac-inspector --bin inspect_counters -- \
    "$COUNTERS_DIR/profile.bin" > "$RESULT_DIR/profile_counters.txt"
  cargo run -p soac-inspector --bin inspect_counters -- \
    "$COUNTERS_DIR/verify.bin" > "$RESULT_DIR/verify_counters.txt"
  cargo run -p soac-inspector --bin inspect_counters -- \
    --specializations "$COUNTERS_DIR/profile.bin" > "$RESULT_DIR/profile_specializations.txt"
  cargo run -p soac-inspector --bin inspect_counters -- \
    --specializations "$COUNTERS_DIR/verify.bin" > "$RESULT_DIR/verify_specializations.txt"

  cargo run -q -p soac-inspector --bin list_jit_functions -- scripts/pystone.py \
    > "$CLIF_DIR/functions.tsv"
  while IFS=$'\t' read -r function_id qualname; do
    safe_qualname="$(printf '%s' "$qualname" | tr -cs '[:alnum:]_.' '_')"
    output_base="$CLIF_DIR/fn_${function_id}_${safe_qualname}"
    SOAC_WORK_DIR="$COUNTERS_DIR" \
    SOAC_OPT_MODE=apply \
      cargo run -q -p soac-inspector --bin render_jit_clif -- \
        --specialized --module-name pystone \
        --cfg-dot-out "$output_base.cfg.dot" \
        --vcode-out "$output_base.vcode" \
        scripts/pystone.py "$function_id" \
        > "$output_base.clif"
  done < "$CLIF_DIR/functions.tsv"

[private]
_benchmark-run-specialized-perf result_dir perf_loops="10000000": ensure-cpython
  #!/usr/bin/env bash
  set -euo pipefail
  export LD_LIBRARY_PATH="$CPYTHON_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  cd "$REPO_ROOT"

  RESULT_DIR="{{result_dir}}"
  if [[ "$RESULT_DIR" != /* ]]; then
    RESULT_DIR="$REPO_ROOT/$RESULT_DIR"
  fi
  COUNTERS_DIR="$RESULT_DIR/counters"
  if [[ ! -f "$COUNTERS_DIR/profile.bin" ]]; then
    echo "counter profile not found at $COUNTERS_DIR/profile.bin; run 'just benchmark' first or pass result_dir=<dir>" >&2
    exit 1
  fi

  OUTPUT_PREFIX="$RESULT_DIR/$(basename "$RESULT_DIR")_perf"
  SOAC_WORK_DIR="$COUNTERS_DIR" \
  SOAC_OPT_MODE=apply \
    just perf-pystone-jit-warm "{{perf_loops}}" "$OUTPUT_PREFIX"

  PERF_DATA_SOURCE="$REPO_ROOT/tmp/$(basename "$OUTPUT_PREFIX").data"
  PERF_INJECTED_SOURCE="$REPO_ROOT/tmp/$(basename "$OUTPUT_PREFIX").injected.data"
  if [[ ! -f "$PERF_DATA_SOURCE" ]]; then
    echo "perf data not found at $PERF_DATA_SOURCE" >&2
    exit 1
  fi
  if [[ ! -f "$PERF_INJECTED_SOURCE" ]]; then
    echo "injected perf data not found at $PERF_INJECTED_SOURCE" >&2
    exit 1
  fi

  cp -f "$PERF_DATA_SOURCE" "$RESULT_DIR/perf.data"
  cp -f "$PERF_INJECTED_SOURCE" "$RESULT_DIR/perf.injected.data"

[private]
_benchmark-add-deep-profile-artifacts result_dir perf_loops="10000000": ensure-cpython
  #!/usr/bin/env bash
  set -euo pipefail
  cd "$REPO_ROOT"

  RESULT_DIR="{{result_dir}}"
  if [[ "$RESULT_DIR" != /* ]]; then
    RESULT_DIR="$REPO_ROOT/$RESULT_DIR"
  fi
  COUNTERS_DIR="$RESULT_DIR/counters"
  if [[ ! -f "$COUNTERS_DIR/profile.bin" ]]; then
    echo "counter profile not found at $COUNTERS_DIR/profile.bin; run 'just benchmark' first or pass result_dir=<dir>" >&2
    exit 1
  fi
  if [[ ! -f "$COUNTERS_DIR/verify.bin" ]]; then
    echo "verification counters not found at $COUNTERS_DIR/verify.bin; run 'just benchmark-verify' or 'just benchmark-deep-profile-from-profile' first" >&2
    exit 1
  fi

  just _benchmark-export-specialized-artifacts "$RESULT_DIR"
  just _benchmark-run-specialized-perf "$RESULT_DIR" "{{perf_loops}}"
  cargo run -q -p soac-inspector --bin annotate_cranelift_perf -- "$RESULT_DIR"

benchmark-deep-profile-from-profile result_dir verify_loops="100000" perf_loops="10000000": (update-venv-offline) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  cd "$REPO_ROOT"

  RESULT_DIR="{{result_dir}}"
  if [[ "$RESULT_DIR" != /* ]]; then
    RESULT_DIR="$REPO_ROOT/$RESULT_DIR"
  fi
  COUNTERS_DIR="$RESULT_DIR/counters"
  REPORT="$RESULT_DIR/deep_profile.txt"
  if [[ ! -f "$COUNTERS_DIR/profile.bin" ]]; then
    echo "counter profile not found at $COUNTERS_DIR/profile.bin; run a profile pass first or pass result_dir=<dir>" >&2
    exit 1
  fi

  {
    echo "result dir: $RESULT_DIR"
    echo "date: $(date +%F)"
    echo "verify loops: {{verify_loops}}"
    echo "perf loops: {{perf_loops}}"
    echo "starting point: $COUNTERS_DIR/profile.bin"
    echo
    just benchmark-verify "{{verify_loops}}" "$COUNTERS_DIR"
    just _benchmark-add-deep-profile-artifacts "$RESULT_DIR" "{{perf_loops}}"
    echo
    echo "deep profile result: $RESULT_DIR"
  } 2>&1 | tee "$REPORT"

benchmark-deep-profile benchmark_loops="1000000" verify_loops="100000" perf_loops="10000000" results_root="bench" result_mode="one-off": (update-venv-offline) (build-extension "release")
  #!/usr/bin/env bash
  set -euo pipefail
  cd "$REPO_ROOT"

  benchmark_log="$(mktemp "${TMPDIR:-/tmp}/soac_benchmark_deep_profile.XXXXXX")"
  trap 'rm -f "$benchmark_log"' EXIT

  just benchmark "{{benchmark_loops}}" "{{verify_loops}}" "{{results_root}}" "{{result_mode}}" \
    2>&1 | tee "$benchmark_log"

  RESULT_DIR="$(sed -n 's/^benchmark result: //p' "$benchmark_log" | tail -n 1)"
  if [[ -z "$RESULT_DIR" ]]; then
    echo "failed to determine benchmark result directory from just benchmark output" >&2
    exit 1
  fi

  REPORT="$RESULT_DIR/deep_profile.txt"
  {
    echo "result dir: $RESULT_DIR"
    echo "date: $(date +%F)"
    echo "perf loops: {{perf_loops}}"
    echo
    just _benchmark-add-deep-profile-artifacts "$RESULT_DIR" "{{perf_loops}}"
    echo
    echo "deep profile result: $RESULT_DIR"
  } 2>&1 | tee "$REPORT"
