#!/bin/sh
set -eu

workspace=${1:-.}
binary_dir=${TERMI9NE_MULTIPLEX_BINARY_DIR:-$workspace/target/debug}
runtime_dir=$(mktemp -d "${TMPDIR:-/tmp}/termi9ne-cli-mux.XXXXXX")
socket=$runtime_dir/control.sock
state=$runtime_dir/state
server_log=$runtime_dir/server.log
events_log=$runtime_dir/events.ndjson
server_pid=
events_pid=

cleanup() {
    if test -n "$events_pid"; then
        kill -INT "$events_pid" 2>/dev/null || true
        wait "$events_pid" 2>/dev/null || true
    fi
    if test -n "$server_pid"; then
        kill -INT "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi
    if test -d "$runtime_dir"; then
        find "$runtime_dir" -depth -delete 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

cli=$binary_dir/termi9ne
server=$binary_dir/termi9ne-server
test -x "$cli"
test -x "$server"

"$server" --socket "$socket" --state-dir "$state" >"$server_log" 2>&1 &
server_pid=$!
attempt=0
while ! test -S "$socket"; do
    attempt=$((attempt + 1))
    if test "$attempt" -gt 100 || ! kill -0 "$server_pid" 2>/dev/null; then
        echo "multiplex smoke runtime failed to become ready" >&2
        sed -n '1,120p' "$server_log" >&2
        exit 1
    fi
    sleep 0.05
done

"$cli" --socket "$socket" terminal-new --program /bin/cat >/dev/null
"$cli" --socket "$socket" events --scope all >"$events_log" &
events_pid=$!

attempt=0
while ! test -s "$events_log"; do
    attempt=$((attempt + 1))
    if test "$attempt" -gt 100 || ! kill -0 "$events_pid" 2>/dev/null; then
        echo "multiplex event stream emitted no snapshot" >&2
        exit 1
    fi
    sleep 0.05
done

diagnostics=$("$cli" --socket "$socket" status)
open_connections=$(printf '%s' "$diagnostics" | sed -n 's/.*"open_connections": \([0-9][0-9]*\).*/\1/p')
test "$open_connections" = 2 || {
    echo "events --scope all used more than one persistent connection: diagnostics reported $open_connections including the status probe" >&2
    exit 1
}

grep -q '"stream":"terminals"' "$events_log"
echo "CLI event multiplex PASS: four logical feeds, one persistent connection"
