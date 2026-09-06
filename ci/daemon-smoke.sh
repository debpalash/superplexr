#!/bin/zsh
# A hosted runtime, on this machine: the headless daemon with the gateway on,
# then the same three shells the gate names — TUI (attach under a pty), web
# (the browser smoke), and a viewer link — against it. No desktop anywhere.
set -u
REPO="${1:-$(cd "$(dirname "$0")/.." && pwd)}"; BIN="$REPO/target/debug/ultraplexr"; DAEMON="$REPO/target/debug/ultraplexr-daemon"
PORT=17379; ROOT=/tmp/spxhost; rm -rf "$ROOT" "$ROOT.sock" "$HOME/.ultraplexr/devices/127.0.0.1_$PORT.json"; mkdir -p "$ROOT"
"$DAEMON" --socket "$ROOT.sock" --state-dir "$ROOT/state" --gateway 127.0.0.1:$PORT > "$ROOT.log" 2>&1 &
echo $! > "$ROOT.pid"
for i in $(seq 1 200); do [ -S "$ROOT.sock" ] && break; sleep 0.05; done
sleep 0.5
echo "▸ 1. the headless daemon is up with its gateway"
grep -h "gateway listening" "$ROOT.log" | cut -c1-90 || { echo "   FAIL: no gateway"; tail -5 "$ROOT.log"; kill $(cat "$ROOT.pid"); exit 1; }
echo "▸ 2. a laptop pairs and starts a session on the host"
PAIR=$("$BIN" --socket "$ROOT.sock" device-pair --label laptop 2>&1)
CODE=$(echo "$PAIR" | python3 -c "import sys,json; print(json.load(sys.stdin)['code'])")
FP=$(echo "$PAIR" | python3 -c "import sys,json; print(json.load(sys.stdin)['fingerprint'])")
"$BIN" pair --gateway 127.0.0.1:$PORT --fingerprint "$FP" "$CODE" >/dev/null 2>&1 && echo "   paired"
SID=$("$BIN" --gateway 127.0.0.1:$PORT terminal-new --cwd /tmp --program /bin/sh -- -c 'printf "HOSTED-MARKER\n"; read line; printf "host echoed: %s\n" "$line"; sleep 20' 2>&1 | python3 -c "import sys,json; print(json.load(sys.stdin)['terminal']['session_id'])")
echo "   session $SID on the host"
echo "▸ 3. the TUI shell attaches over the gateway"
OUT=/tmp/host_attach.txt; : > "$OUT"
( sleep 1.5; printf 'from the tui\r'; sleep 1.5; printf '\x1d'; sleep 0.3 ) | script -q "$OUT" "$BIN" --gateway 127.0.0.1:$PORT attach --take "$SID" >/dev/null 2>&1
echo "   painted: $(grep -c 'HOSTED-MARKER' "$OUT") · echoed: $(grep -c 'host echoed: from the tui' "$OUT")"
echo "▸ 4. the web shell's whole path against the host"
cd "$REPO" && node ci/web-smoke.mjs "https://127.0.0.1:$PORT" "$ROOT.sock" "$BIN" 2>&1 | grep -E "all web|FAIL" | sed 's/^/   /'
echo "▸ 5. a viewer link for the host's session"
LINK=$("$BIN" --socket "$ROOT.sock" stream "$SID" --label onlooker 2>&1 | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['url'])")
echo "   $(echo "$LINK" | sed 's/#share=.*/#share=…/')"
STATUS=0; grep -q 'host echoed: from the tui' "$OUT" || STATUS=1
kill $(cat "$ROOT.pid") 2>/dev/null; rm -rf "$ROOT" "$ROOT.sock" "$ROOT.pid" "$ROOT.log" "$OUT" "$HOME/.ultraplexr/devices/127.0.0.1_$PORT.json"
[ $STATUS -eq 0 ] && echo "all hosted steps pass"
exit $STATUS
