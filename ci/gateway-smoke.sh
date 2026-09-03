#!/bin/zsh
# End-to-end: a runtime with the TLS gateway on, pairing, requests over TLS,
# refusal of the unpaired, revocation, and attach over the gateway under a pty.
set -u
REPO="${1:-$(cd "$(dirname "$0")/.." && pwd)}"; BIN="$REPO/target/debug/superplexr"; DAEMON="$REPO/target/debug/superplexr-desktop"
ROOT=/tmp/spxgw; rm -rf "$ROOT" "$ROOT.sock" "$HOME/.superplexr/devices/127.0.0.1_17373.json"; mkdir -p "$ROOT/work"
"$DAEMON" --internal-daemon --socket "$ROOT.sock" --state-dir "$ROOT" --gateway 127.0.0.1:17373 > "$ROOT.log" 2>&1 &
echo $! > "$ROOT.pid"
for i in $(seq 1 200); do [ -S "$ROOT.sock" ] && break; sleep 0.05; done
sleep 0.5; echo "daemon: $(grep -h 'gateway listening' "$ROOT.log" | cut -c1-110)"

echo "▸ 1. unpaired device is refused at the handshake"
"$BIN" --gateway 127.0.0.1:17373 --fingerprint "$(grep -o 'sha256:[0-9a-f]*' "$ROOT.log" | head -1)" ping 2>&1 | tail -1 | cut -c1-140

echo "▸ 2. pair: code on the host, code typed on the device"
PAIR=$("$BIN" --socket "$ROOT.sock" device-pair --label "laptop-test" 2>&1)
CODE=$(echo "$PAIR" | python3 -c "import sys,json; print(json.load(sys.stdin)['code'])")
FP=$(echo "$PAIR" | python3 -c "import sys,json; print(json.load(sys.stdin)['fingerprint'])")
echo "   code=$CODE fingerprint=${FP:0:20}…"
"$BIN" pair --gateway 127.0.0.1:17373 --fingerprint "$FP" "$CODE" 2>&1 | tail -1 | cut -c1-120
ls -la "$HOME/.superplexr/devices/" | awk 'NR>1{print "   creds:", $1, $NF}'

echo "▸ 3. a request over TLS with the stored token"
"$BIN" --gateway 127.0.0.1:17373 terminal-list 2>&1 | python3 -c "import sys,json; d=json.load(sys.stdin); print('   terminal-list over tls: ok,', len(d['terminals']), 'terminals')" 2>&1 | tail -1

echo "▸ 4. a wrong fingerprint is refused before any bytes of ours are trusted"
"$BIN" --gateway 127.0.0.1:17373 --fingerprint "sha256:$(printf '0%.0s' {1..64})" ping 2>&1 | tail -1 | cut -c1-140

echo "▸ 5. attach over the gateway under a pty"
SID=$("$BIN" --gateway 127.0.0.1:17373 terminal-new --cwd /tmp --program /bin/sh -- -c 'printf "GATEWAY-MARKER\n"; read line; printf "you typed: %s\n" "$line"; sleep 5' 2>&1 | python3 -c "import sys,json; print(json.load(sys.stdin)['terminal']['session_id'])")
OUT=/tmp/gw_attach.txt; : > "$OUT"
( sleep 1.5; printf 'over tls\r'; sleep 1.5; printf '\x1d'; sleep 0.3 ) | script -q "$OUT" "$BIN" --gateway 127.0.0.1:17373 attach --take "$SID" >/dev/null 2>/tmp/gw_attach_err.txt
echo "   painted: $(grep -c 'GATEWAY-MARKER' "$OUT") · echoed: $(grep -c 'you typed: over tls' "$OUT") · $(tail -1 /tmp/gw_attach_err.txt)"

echo "▸ 6. revoke, then the device's next request is refused"
DEV=$("$BIN" --socket "$ROOT.sock" device-list 2>&1 | python3 -c "import sys,json; print(json.load(sys.stdin)['devices'][0]['device_id'])")
"$BIN" --socket "$ROOT.sock" device-revoke "$DEV" >/dev/null 2>&1 && echo "   revoked $DEV"
"$BIN" --gateway 127.0.0.1:17373 ping 2>&1 | tail -1 | cut -c1-140

kill $(cat "$ROOT.pid") 2>/dev/null; rm -rf "$ROOT" "$ROOT.sock" "$ROOT.pid" "$ROOT.log"
