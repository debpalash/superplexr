#!/usr/bin/env bash
set -euo pipefail

minimum_fps="${ULTRAPLEXR_DESKTOP_MIN_FPS:-60}"
maximum_p95_ms="${ULTRAPLEXR_DESKTOP_MAX_P95_MS:-16.7}"
benchmark_root="$(mktemp -d "${TMPDIR:-/tmp}/ultraplexr-render-bench.XXXXXX")"
socket_path="$benchmark_root/control.sock"
state_path="$benchmark_root/state"
server_pid=""

cleanup() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [[ "$benchmark_root" == "${TMPDIR:-/tmp}"/ultraplexr-render-bench.* ]]; then
    find "$benchmark_root" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

cargo build -p ultraplexr-server -p ultraplexr-cli -p ultraplexr-desktop
target/debug/ultraplexr-server --socket "$socket_path" --state-dir "$state_path" &
server_pid="$!"

for _ in {1..100}; do
  [[ -S "$socket_path" ]] && break
  sleep 0.05
done
[[ -S "$socket_path" ]] || { echo "renderer benchmark daemon did not start" >&2; exit 1; }

group_arguments=()
for _ in {1..6}; do
  response="$(target/debug/ultraplexr --socket "$socket_path" terminal-new --program /bin/sh)"
  session_id="$(sed -n 's/.*"session_id": "\([^"]*\)".*/\1/p' <<<"$response" | head -n 1)"
  [[ -n "$session_id" ]] || { echo "renderer benchmark terminal did not start" >&2; exit 1; }
  group_arguments+=(--session "$session_id")
done
target/debug/ultraplexr --socket "$socket_path" session-group-create perf-grid "${group_arguments[@]}" >/dev/null

result="$(target/debug/ultraplexr-desktop \
  --connect-only \
  --socket "$socket_path" \
  --state-dir "$state_path" \
  --render-benchmark)"
printf '%s\n' "$result"

fps="$(sed -n 's/.*"fps":\([0-9.]*\).*/\1/p' <<<"$result")"
p95_ms="$(sed -n 's/.*"frame_p95_ms":\([0-9.]*\).*/\1/p' <<<"$result")"
[[ -n "$fps" ]] || { echo "renderer benchmark emitted no FPS result" >&2; exit 1; }
[[ -n "$p95_ms" ]] || { echo "renderer benchmark emitted no p95 result" >&2; exit 1; }
awk -v actual="$fps" -v minimum="$minimum_fps" 'BEGIN { exit !(actual >= minimum) }' || {
  echo "desktop renderer ${fps} FPS is below the ${minimum_fps} FPS gate" >&2
  exit 1
}
awk -v actual="$p95_ms" -v maximum="$maximum_p95_ms" 'BEGIN { exit !(actual <= maximum) }' || {
  echo "desktop renderer ${p95_ms} ms p95 exceeds the ${maximum_p95_ms} ms frame budget" >&2
  exit 1
}
