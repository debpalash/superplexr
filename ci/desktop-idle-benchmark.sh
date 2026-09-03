#!/usr/bin/env bash
set -euo pipefail

maximum_desktop_cpu="${SUPERPLEXR_DESKTOP_MAX_IDLE_CPU:-1.0}"
maximum_runtime_cpu="${SUPERPLEXR_RUNTIME_MAX_IDLE_CPU:-0.5}"
maximum_combined_rss_mib="${SUPERPLEXR_MAX_COMBINED_RSS_MIB:-600}"
benchmark_root="$(mktemp -d "${TMPDIR:-/tmp}/superplexr-idle-bench.XXXXXX")"
socket_path="$benchmark_root/control.sock"
state_path="$benchmark_root/state"
server_pid=""

cleanup() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [[ "$benchmark_root" == "${TMPDIR:-/tmp}"/superplexr-idle-bench.* ]]; then
    find "$benchmark_root" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

cargo build -p superplexr-server -p superplexr-cli -p superplexr-desktop
target/debug/superplexr-server --socket "$socket_path" --state-dir "$state_path" &
server_pid="$!"

for _ in {1..100}; do
  [[ -S "$socket_path" ]] && break
  sleep 0.05
done
[[ -S "$socket_path" ]] || { echo "idle benchmark daemon did not start" >&2; exit 1; }

group_arguments=()
for _ in {1..12}; do
  response="$(target/debug/superplexr --socket "$socket_path" terminal-new --program /bin/cat)"
  session_id="$(sed -n 's/.*"session_id": "\([^"]*\)".*/\1/p' <<<"$response" | head -n 1)"
  [[ -n "$session_id" ]] || { echo "idle benchmark terminal did not start" >&2; exit 1; }
  group_arguments+=(--session "$session_id")
done
target/debug/superplexr --socket "$socket_path" session-group-create quiet-grid "${group_arguments[@]}" >/dev/null

result="$(target/debug/superplexr-desktop \
  --connect-only \
  --socket "$socket_path" \
  --state-dir "$state_path" \
  --idle-benchmark)"
printf '%s\n' "$result"

desktop_cpu="$(sed -n 's/.*"desktop_cpu_percent":\([0-9.]*\).*/\1/p' <<<"$result")"
desktop_rss_mib="$(sed -n 's/.*"desktop_rss_mib":\([0-9.]*\).*/\1/p' <<<"$result")"
runtime_cpu="$(ps -p "$server_pid" -o %cpu= | tr -d ' ')"
runtime_rss_kib="$(ps -p "$server_pid" -o rss= | tr -d ' ')"
[[ -n "$desktop_cpu" && -n "$desktop_rss_mib" ]] || {
  echo "idle benchmark emitted incomplete process telemetry" >&2
  exit 1
}
[[ -n "$runtime_cpu" && -n "$runtime_rss_kib" ]] || {
  echo "idle benchmark could not sample the runtime" >&2
  exit 1
}

combined_rss_mib="$(awk -v desktop="$desktop_rss_mib" -v runtime_kib="$runtime_rss_kib" 'BEGIN { printf "%.1f", desktop + runtime_kib / 1024 }')"
printf '{"benchmark":"runtime_idle","runtime_cpu_percent":%s,"combined_rss_mib":%s}\n' "$runtime_cpu" "$combined_rss_mib"

awk -v actual="$desktop_cpu" -v maximum="$maximum_desktop_cpu" 'BEGIN { exit !(actual <= maximum) }' || {
  echo "desktop idle CPU ${desktop_cpu}% exceeds the ${maximum_desktop_cpu}% gate" >&2
  exit 1
}
awk -v actual="$runtime_cpu" -v maximum="$maximum_runtime_cpu" 'BEGIN { exit !(actual <= maximum) }' || {
  echo "runtime idle CPU ${runtime_cpu}% exceeds the ${maximum_runtime_cpu}% gate" >&2
  exit 1
}
awk -v actual="$combined_rss_mib" -v maximum="$maximum_combined_rss_mib" 'BEGIN { exit !(actual <= maximum) }' || {
  echo "combined RSS ${combined_rss_mib} MiB exceeds the ${maximum_combined_rss_mib} MiB gate" >&2
  exit 1
}
