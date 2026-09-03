#!/bin/sh
set -eu

binary=${1:-target/release/superplexr-desktop}
test -x "$binary"
runtime_dir=$(mktemp -d "${TMPDIR:-/tmp}/superplexr-wayland.XXXXXX")
weston_log="$runtime_dir/weston.log"
app_log="$runtime_dir/app.log"
weston_pid=
xvfb_pid=

cleanup() {
    if test -n "$weston_pid"; then
        kill "$weston_pid" 2>/dev/null || true
        wait "$weston_pid" 2>/dev/null || true
    fi
    if test -n "$xvfb_pid"; then
        kill "$xvfb_pid" 2>/dev/null || true
        wait "$xvfb_pid" 2>/dev/null || true
    fi
    rm -rf "$runtime_dir"
}
trap cleanup EXIT INT TERM
chmod 700 "$runtime_dir"

Xvfb :99 -screen 0 1280x800x24 >"$runtime_dir/xvfb.log" 2>&1 &
xvfb_pid=$!
attempt=0
while ! test -S /tmp/.X11-unix/X99; do
    attempt=$((attempt + 1))
    if test "$attempt" -gt 50; then
        echo "Xvfb did not create its socket" >&2
        sed -n '1,160p' "$runtime_dir/xvfb.log" >&2
        exit 1
    fi
    sleep 0.1
done

set +e
dbus-run-session -- env DISPLAY=:99 XDG_SESSION_TYPE=x11 \
    timeout 6 "$binary" >"$app_log" 2>&1
x11_status=$?
set -e
if test "$x11_status" -ne 124; then
    echo "X11 window smoke failed with status $x11_status" >&2
    sed -n '1,160p' "$app_log" >&2
    exit 1
fi
echo "X11 virtual-window smoke PASS"

DISPLAY=:99 XDG_RUNTIME_DIR="$runtime_dir" weston \
    --backend=x11-backend.so \
    --socket=wayland-superplexr \
    --idle-time=0 \
    --log="$weston_log" &
weston_pid=$!

attempt=0
while ! test -S "$runtime_dir/wayland-superplexr"; do
    attempt=$((attempt + 1))
    if test "$attempt" -gt 50; then
        echo "Wayland compositor did not create its socket" >&2
        sed -n '1,160p' "$weston_log" >&2
        exit 1
    fi
    sleep 0.1
done

set +e
XDG_RUNTIME_DIR="$runtime_dir" \
WAYLAND_DISPLAY=wayland-superplexr \
XDG_SESSION_TYPE=wayland \
timeout 6 "$binary" >"$app_log" 2>&1
wayland_status=$?
set -e
if test "$wayland_status" -ne 124; then
    echo "Wayland window smoke failed with status $wayland_status" >&2
    sed -n '1,160p' "$app_log" >&2
    sed -n '1,160p' "$weston_log" >&2
    exit 1
fi
echo "Wayland virtual-window smoke PASS"
