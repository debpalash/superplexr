#!/bin/zsh
# End-to-end for replay: chapters, pace, a chapter start, the TUI player,
# its scripts over HTTPS, a browser-style WebSocket client (node) that is
# refused unpaired, pairs, starts a shell, subscribes, types and reads the
# echo out of the protobuf frames, and an off-host origin that is refused.
set -u
REPO="${1:-$(cd "$(dirname "$0")/.." && pwd)}"; BIN="$REPO/target/debug/ultraplexr"; DAEMON="$REPO/target/debug/ultraplexr-desktop"
PORT=17377; ROOT=/tmp/spxreplay; rm -rf "$ROOT" "$ROOT.sock"; mkdir -p "$ROOT"
"$DAEMON" --internal-daemon --socket "$ROOT.sock" --state-dir "$ROOT" --gateway 127.0.0.1:$PORT > "$ROOT.log" 2>&1 &
echo $! > "$ROOT.pid"
for i in $(seq 1 200); do [ -S "$ROOT.sock" ] && break; sleep 0.05; done
sleep 0.5
cd "$REPO" && node "${REPLAY_E2E:-$REPO/ci/replay-smoke.mjs}" "https://127.0.0.1:$PORT" "$ROOT.sock" "$BIN"; STATUS=$?
[ "$STATUS" -ne 0 ] && { echo "--- runtime log ---"; tail -20 "$ROOT.log"; }
kill $(cat "$ROOT.pid") 2>/dev/null; rm -rf "$ROOT" "$ROOT.sock" "$ROOT.pid" "$ROOT.log"
exit $STATUS
