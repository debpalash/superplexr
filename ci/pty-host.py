#!/usr/bin/env python3
"""Give benchmark clients a real controlling PTY without third-party packages.

The first stdout line identifies the child PID for process measurements; all
following bytes are PTY output. Signals are forwarded and the child is reaped.
"""
import contextlib
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios
import time


def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: pty-host.py EXECUTABLE [ARGS ...]")
    child, master = pty.fork()
    if child == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 120, 0, 0))
        os.execv(sys.argv[1], sys.argv[1:])
    print(f"ULTRAPLEXR_PTY_PID {child}", flush=True)
    stopping = False
    sent_stop = None
    status = None
    stdin_open = True

    def request_stop(_signal, _frame):
        nonlocal stopping
        stopping = True

    for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(signum, request_stop)
    try:
        while status is None:
            if stopping and sent_stop is None:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(child, signal.SIGTERM)
                sent_stop = time.monotonic()
            if sent_stop is not None and time.monotonic() - sent_stop > 1:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(child, signal.SIGKILL)
            readers = [master] + ([sys.stdin.fileno()] if stdin_open and not stopping else [])
            ready, _, _ = select.select(readers, [], [], 0.05)
            if master in ready:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    data = b""
                if data:
                    sys.stdout.buffer.write(data)
                    sys.stdout.buffer.flush()
            if stdin_open and sys.stdin.fileno() in ready:
                data = os.read(sys.stdin.fileno(), 4096)
                if data:
                    os.write(master, data)
                else:
                    stdin_open = False
            exited, value = os.waitpid(child, os.WNOHANG)
            if exited:
                status = value
    finally:
        os.close(master)
        if status is None:
            with contextlib.suppress(ProcessLookupError):
                os.kill(child, signal.SIGKILL)
            _, status = os.waitpid(child, 0)
    code = os.waitstatus_to_exitcode(status)
    return code if code >= 0 else 128 - code


if __name__ == "__main__":
    raise SystemExit(main())
