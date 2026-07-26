#!/usr/bin/env bash

set -euo pipefail

if [[ $# -eq 0 ]]; then
  echo "usage: $0 <command> [args...]" >&2
  exit 2
fi

env_flag_enabled() {
  local raw="${!1-}"
  case "$raw" in
    "" | 0 | false | False | FALSE | no | No | NO | off | Off | OFF)
      return 1
      ;;
    *)
      return 0
      ;;
  esac
}

needs_elevated_write() {
  local path="$1"
  [[ ! -w "$path" ]]
}

ensure_sudo_ready() {
  if [[ -n "${BENCHMARK_CLOCKS_SUDO_READY-}" ]]; then
    return 0
  fi
  if ! command -v sudo >/dev/null 2>&1; then
    echo "benchmark constant-clock mode needs sudo for cpufreq writes, but sudo is not installed" >&2
    exit 1
  fi
  if [[ -t 0 && -t 2 ]]; then
    sudo -v
  elif ! sudo -n -v 2>/dev/null; then
    echo "benchmark constant-clock mode needs sudo credentials, but this benchmark is running without a tty; rerun from an interactive terminal or set BENCHMARK_CONSTANT_CLOCKS=0" >&2
    exit 1
  fi
  BENCHMARK_CLOCKS_SUDO_READY=1
}

write_sysfs_value() {
  local path="$1"
  local value="$2"
  if needs_elevated_write "$path"; then
    ensure_sudo_ready
    printf '%s\n' "$value" | sudo tee "$path" >/dev/null
  else
    printf '%s\n' "$value" > "$path"
  fi
}

restore_cpufreq_state() {
  local cpu_dir
  local value
  local cpu

  for cpu in "${RESTORE_CPUS[@]}"; do
    cpu_dir="/sys/devices/system/cpu/cpu${cpu}/cpufreq"

    value="${RESTORE_SCALING_MAX_FREQ[$cpu]-}"
    if [[ -n "$value" && -e "$cpu_dir/scaling_max_freq" ]]; then
      write_sysfs_value "$cpu_dir/scaling_max_freq" "$value"
    fi

    value="${RESTORE_SCALING_MIN_FREQ[$cpu]-}"
    if [[ -n "$value" && -e "$cpu_dir/scaling_min_freq" ]]; then
      write_sysfs_value "$cpu_dir/scaling_min_freq" "$value"
    fi

    value="${RESTORE_SCALING_GOVERNOR[$cpu]-}"
    if [[ -n "$value" && -e "$cpu_dir/scaling_governor" ]]; then
      write_sysfs_value "$cpu_dir/scaling_governor" "$value"
    fi

    value="${RESTORE_EPP[$cpu]-}"
    if [[ -n "$value" && -e "$cpu_dir/energy_performance_preference" ]]; then
      write_sysfs_value "$cpu_dir/energy_performance_preference" "$value"
    fi
  done

  if [[ -n "${RESTORE_BOOST_FILE-}" && -n "${RESTORE_BOOST_VALUE-}" && -e "$RESTORE_BOOST_FILE" ]]; then
    write_sysfs_value "$RESTORE_BOOST_FILE" "$RESTORE_BOOST_VALUE"
  fi
}

CPU="${BENCHMARK_CPU:-}"
if [[ -n "$CPU" ]]; then
  CPU_DIR="/sys/devices/system/cpu/cpu${CPU}/cpufreq"
  if ! command -v taskset >/dev/null 2>&1; then
    echo "taskset is required for benchmark cpu pinning" >&2
    exit 1
  fi
fi

if env_flag_enabled BENCHMARK_CONSTANT_CLOCKS; then
  if [[ -z "$CPU" ]]; then
    echo "benchmark constant-clock mode requires BENCHMARK_CPU=<cpu>; rerun with BENCHMARK_CONSTANT_CLOCKS=0 or set BENCHMARK_CPU" >&2
    exit 1
  fi

  if [[ ! -d "$CPU_DIR" ]]; then
    echo "benchmark constant-clock mode needs cpufreq controls at $CPU_DIR; rerun with BENCHMARK_CONSTANT_CLOCKS=0 if this cpu does not support them" >&2
    exit 1
  fi

  if [[ ! -r "$CPU_DIR/related_cpus" ]]; then
    echo "benchmark constant-clock mode needs readable related_cpus at $CPU_DIR/related_cpus" >&2
    exit 1
  fi

  read -r -a RESTORE_CPUS <<<"$(<"$CPU_DIR/related_cpus")"
  declare -Ag RESTORE_SCALING_GOVERNOR=()
  declare -Ag RESTORE_SCALING_MIN_FREQ=()
  declare -Ag RESTORE_SCALING_MAX_FREQ=()
  declare -Ag RESTORE_EPP=()
  RESTORE_BOOST_FILE=""
  RESTORE_BOOST_VALUE=""

  if [[ -e "$CPU_DIR/boost" ]]; then
    RESTORE_BOOST_FILE="$CPU_DIR/boost"
    RESTORE_BOOST_VALUE="$(<"$CPU_DIR/boost")"
    write_sysfs_value "$CPU_DIR/boost" "0"
  fi

  trap restore_cpufreq_state EXIT

  for cpu in "${RESTORE_CPUS[@]}"; do
    cpu_dir="/sys/devices/system/cpu/cpu${cpu}/cpufreq"
    if [[ ! -d "$cpu_dir" ]]; then
      continue
    fi

    if [[ ! -e "$cpu_dir/scaling_governor" || ! -e "$cpu_dir/scaling_min_freq" || ! -e "$cpu_dir/scaling_max_freq" ]]; then
      echo "benchmark constant-clock mode needs cpufreq knobs under $cpu_dir; rerun with BENCHMARK_CONSTANT_CLOCKS=0 if this cpu does not support them" >&2
      exit 1
    fi

    RESTORE_SCALING_GOVERNOR[$cpu]="$(<"$cpu_dir/scaling_governor")"
    RESTORE_SCALING_MIN_FREQ[$cpu]="$(<"$cpu_dir/scaling_min_freq")"
    RESTORE_SCALING_MAX_FREQ[$cpu]="$(<"$cpu_dir/scaling_max_freq")"

    if [[ -r "$cpu_dir/energy_performance_preference" ]]; then
      RESTORE_EPP[$cpu]="$(<"$cpu_dir/energy_performance_preference")"
    fi

    max_freq="$(<"$cpu_dir/cpuinfo_max_freq")"
    write_sysfs_value "$cpu_dir/scaling_governor" "performance"
    write_sysfs_value "$cpu_dir/scaling_min_freq" "$max_freq"
    write_sysfs_value "$cpu_dir/scaling_max_freq" "$max_freq"

    if [[ -e "$cpu_dir/energy_performance_preference" ]]; then
      write_sysfs_value "$cpu_dir/energy_performance_preference" "performance"
    fi
  done
fi

if [[ -n "$CPU" ]]; then
  taskset -c "$CPU" "$@"
else
  "$@"
fi
