#!/bin/zsh
# End-to-end for Web Push: node plays the push service and the browser; the
# runtime encrypts (RFC 8291), signs (RFC 8292) and delivers with curl.
set -u
REPO="${1:-$(cd "$(dirname "$0")/.." && pwd)}"; BIN="$REPO/target/debug/ultraplexr"; DAEMON="$REPO/target/debug/ultraplexr-desktop"
PORT=17378; PUSHPORT=17390; ROOT=/tmp/spxpush; rm -rf "$ROOT" "$ROOT.sock"; mkdir -p "$ROOT/push"
# A certificate for the stand-in push service; the runtime trusts it via push/ca.pem.
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout "$ROOT/push-key.pem" -out "$ROOT/push/ca.pem" -days 2 -subj "/CN=127.0.0.1" -addext "subjectAltName=IP:127.0.0.1" >/dev/null 2>&1 || { echo "openssl could not make a certificate"; exit 1; }
"$DAEMON" --internal-daemon --socket "$ROOT.sock" --state-dir "$ROOT" --gateway 127.0.0.1:$PORT > "$ROOT.log" 2>&1 &
echo $! > "$ROOT.pid"
for i in $(seq 1 200); do [ -S "$ROOT.sock" ] && break; sleep 0.05; done
sleep 0.5
cd "$REPO" && node "${PUSH_E2E:-$REPO/ci/push-smoke.mjs}" "https://127.0.0.1:$PORT" "$ROOT.sock" "$BIN" "$ROOT/push/ca.pem" "$ROOT/push-key.pem" "$PUSHPORT"; STATUS=$?
[ "$STATUS" -ne 0 ] && { echo "--- runtime log ---"; tail -20 "$ROOT.log"; }
kill $(cat "$ROOT.pid") 2>/dev/null; rm -rf "$ROOT" "$ROOT.sock" "$ROOT.pid" "$ROOT.log"
exit $STATUS
