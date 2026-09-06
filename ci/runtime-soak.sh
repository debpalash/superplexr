#!/bin/sh
set -eu

workspace=${1:-.}
duration=${ULTRAPLEXR_SOAK_SECONDS:-60}
session_count=${ULTRAPLEXR_SOAK_SESSIONS:-12}
binary_dir=${ULTRAPLEXR_SOAK_BINARY_DIR:-$workspace/target/release}
runtime_dir=$(mktemp -d "${TMPDIR:-/tmp}/ultraplexr-soak.XXXXXX")
socket="$runtime_dir/control.sock"
state="$runtime_dir/state"
server_log="$runtime_dir/server.log"
sessions="$runtime_dir/sessions"
server_pid=

cleanup() {
    if test -n "$server_pid"; then
        kill -INT "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi
    rm -rf "$runtime_dir"
}
trap cleanup EXIT INT TERM

cli="$binary_dir/ultraplexr"
server="$binary_dir/ultraplexr-server"
test -x "$cli"
test -x "$server"
case "$duration" in
    *[!0-9]*|'') echo "ULTRAPLEXR_SOAK_SECONDS must be a positive integer" >&2; exit 1 ;;
esac
case "$session_count" in
    *[!0-9]*|'') echo "ULTRAPLEXR_SOAK_SESSIONS must be a positive integer" >&2; exit 1 ;;
esac
test "$duration" -gt 0
test "$session_count" -gt 0
mkdir -p "$state"
chmod 700 "$runtime_dir" "$state"

"$server" --socket "$socket" --state-dir "$state" >"$server_log" 2>&1 &
server_pid=$!
attempt=0
while ! test -S "$socket"; do
    attempt=$((attempt + 1))
    if test "$attempt" -gt 100 || ! kill -0 "$server_pid" 2>/dev/null; then
        echo "soak runtime failed to become ready" >&2
        sed -n '1,160p' "$server_log" >&2
        exit 1
    fi
    sleep 0.05
done

index=1
while test "$index" -le "$session_count"; do
    response=$("$cli" --socket "$socket" terminal-new --program /bin/cat)
    session_id=$(printf '%s' "$response" | sed -n 's/.*"session_id": "\([^"]*\)".*/\1/p')
    test -n "$session_id"
    printf '%s\n' "$session_id" >>"$sessions"
    index=$((index + 1))
done

deadline=$(($(date +%s) + duration))
iteration=0
while test "$(date +%s)" -lt "$deadline"; do
    iteration=$((iteration + 1))
    index=0
    while IFS= read -r session_id; do
        marker="soak-${iteration}-${index}"
        payload=$(printf '%s\r' "$marker")
        columns=$((80 + (iteration + index) % 41))
        rows=$((20 + (iteration * 3 + index) % 17))
        "$cli" --socket "$socket" --force-control \
            terminal-write "$session_id" "$payload" >/dev/null
        "$cli" --socket "$socket" --force-control \
            terminal-resize "$session_id" "$columns" "$rows" >/dev/null
        "$cli" --socket "$socket" terminal-snapshot "$session_id" >/dev/null
        attempt=0
        while :; do
            search=$("$cli" --socket "$socket" \
                terminal-search "$session_id" "$marker" --limit 1)
            if printf '%s' "$search" | grep -q "$marker"; then
                break
            fi
            attempt=$((attempt + 1))
            if test "$attempt" -gt 50; then
                echo "marker $marker did not reach Session $session_id" >&2
                exit 1
            fi
            sleep 0.02
        done
        index=$((index + 1))
    done <"$sessions"
    listed=$("$cli" --socket "$socket" terminal-list | grep -c '"status": "running"')
    test "$listed" -eq "$session_count"
done

while IFS= read -r session_id; do
    "$cli" --socket "$socket" --force-control terminal-terminate "$session_id" >/dev/null
done <"$sessions"

sleep 1
remaining=$("$cli" --socket "$socket" terminal-list | grep -c '"status": "running"' || true)
test "$remaining" -eq 0
diagnostics=$("$cli" --socket "$socket" status)
printf '%s' "$diagnostics" | grep -q '"terminals_running": 0'
printf '%s' "$diagnostics" | grep -q '"terminals_controlled": 0'
test -s "$server_log"
echo "runtime soak PASS: $session_count Sessions, $iteration iteration(s), ${duration}s"
