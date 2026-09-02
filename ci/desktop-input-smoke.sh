#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "desktop native-input smoke is currently implemented for macOS"
  exit 0
fi

workspace=${1:-.}
binary_dir=${TERMI9NE_INPUT_BINARY_DIR:-$workspace/target/debug}
smoke_root=$(mktemp -d "${TMPDIR:-/tmp}/termi9ne-input-smoke.XXXXXX")
socket_path=$smoke_root/control.sock
state_path=$smoke_root/state
server_log=$smoke_root/server.log
desktop_log=$smoke_root/desktop.log
server_pid=
desktop_pid=

cleanup() {
  if [[ -n "$desktop_pid" ]] && kill -0 "$desktop_pid" 2>/dev/null; then
    kill "$desktop_pid" 2>/dev/null || true
    wait "$desktop_pid" 2>/dev/null || true
  fi
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [[ "$smoke_root" == "${TMPDIR:-/tmp}"/termi9ne-input-smoke.* ]]; then
    find "$smoke_root" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

cargo build -p termi9ne-server -p termi9ne-cli -p termi9ne-desktop
server=$binary_dir/termi9ne-server
cli=$binary_dir/termi9ne
desktop=$binary_dir/termi9ne-desktop

"$server" --socket "$socket_path" --state-dir "$state_path" >"$server_log" 2>&1 &
server_pid=$!
for _ in {1..100}; do
  [[ -S "$socket_path" ]] && break
  sleep 0.05
done
[[ -S "$socket_path" ]] || {
  echo "native-input smoke runtime did not become ready" >&2
  sed -n '1,120p' "$server_log" >&2
  exit 1
}

terminal=$($cli --socket "$socket_path" terminal-new --program /bin/sh)
session_id=$(sed -n 's/.*"session_id": "\([^"]*\)".*/\1/p' <<<"$terminal" | head -n 1)
[[ -n "$session_id" ]] || {
  echo "native-input smoke terminal did not start" >&2
  exit 1
}

"$desktop" \
  --connect-only \
  --socket "$socket_path" \
  --state-dir "$state_path" >"$desktop_log" 2>&1 &
desktop_pid=$!

mission_subscribers=
terminal_subscribers=
diagnostics=
for _ in {1..100}; do
  diagnostics=$($cli --socket "$socket_path" status 2>/dev/null || true)
  mission_subscribers=$(sed -n 's/.*"mission_subscribers": \([0-9][0-9]*\).*/\1/p' <<<"$diagnostics")
  terminal_subscribers=$(sed -n 's/.*"terminal_index_subscribers": \([0-9][0-9]*\).*/\1/p' <<<"$diagnostics")
  if [[ "$mission_subscribers" == "1" && "$terminal_subscribers" == "1" ]]; then
    break
  fi
  sleep 0.05
done
[[ "$mission_subscribers" == "1" && "$terminal_subscribers" == "1" ]] || {
  echo "desktop did not retain its multiplexed subscriptions" >&2
  sed -n '1,120p' "$desktop_log" >&2
  exit 1
}

# Post t9input as genuine macOS keyboard events directly to the GPUI process.
# Key codes are physical ANSI positions: t, 9, i, n, p, u, t.
TERMI9NE_TARGET_PID=$desktop_pid swift -e 'import Foundation; import CoreGraphics
let pid = pid_t(Int(ProcessInfo.processInfo.environment["TERMI9NE_TARGET_PID"]!)!)
let source = CGEventSource(stateID: .hidSystemState)!
for keyCode: CGKeyCode in [17, 25, 34, 45, 35, 32, 17] {
    CGEvent(keyboardEventSource: source, virtualKey: keyCode, keyDown: true)!.postToPid(pid)
    CGEvent(keyboardEventSource: source, virtualKey: keyCode, keyDown: false)!.postToPid(pid)
    Thread.sleep(forTimeInterval: 0.02)
}'

if ! $cli --socket "$socket_path" terminal-wait-text \
  --case-sensitive \
  --timeout-millis 5000 \
  "$session_id" \
  t9input >/dev/null; then
  echo "native keyboard events did not reach the terminal" >&2
  $cli --socket "$socket_path" terminal-capture "$session_id" >&2 || true
  sed -n '1,120p' "$desktop_log" >&2
  exit 1
fi

protocol=$(sed -n 's/.*"protocol_version": \([0-9][0-9]*\).*/\1/p' <<<"$diagnostics")
profile=$(sed -n 's/.*"wire_profile": "\([^"]*\)".*/\1/p' <<<"$diagnostics")
[[ "$profile" == "v3-json-control-protobuf-terminal-zstd-multiplexed" ]]
echo "desktop native-input smoke PASS: protocol=$protocol, multiplexed subscriptions retained"
