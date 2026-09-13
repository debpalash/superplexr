# Scoped browser Control evidence

Verified 2026-09-05 on macOS arm64, Apple M2, using development builds.
This records an optional localhost browser milestone, not universal-platform
completion or production remote readiness.

## Implemented boundary

The existing runtime owns every PTY. The separate browser gateway remains
read-only by default; `--allow-control` requires a scoped Controller Share.
Each live browser attachment receives a distinct runtime Surface. Explicit,
non-forced Control enables ordered keys, confirmed paste, and bounded resize.
Detach, expired activity, or a broken native link retires that input identity.
Observation may reconnect, but input and Control are never replayed or restored
automatically. No browser process-creation, termination, or owner fallback exists.

## Automated checks

```sh
cargo test --workspace --locked --quiet -- --test-threads=1
node --test crates/superplexr-observer/web/control_tests.mjs crates/superplexr-observer/web/stream_tests.mjs
cargo clippy -p superplexr-observer -p superplexr-desktop --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The workspace suite passes 339 Rust tests (four environment/explicit-run tests
remain ignored). The browser modules pass 15 JavaScript tests. Clippy and format
checks pass; Cargo still reports the external `block` future-compatibility notice.

Four new real-daemon tests exercise independent browser views, contention with
the owner, stale nonces, duplicate sequences, no force takeover, origin/auth and
scope checks, paste confirmation, payload/grid limits, resize, HTTP detach,
native-link loss, revocation, and idle lease expiry. An idle controlled feed
does not emit unchanged terminal frames during its heartbeat checks.

The JavaScript tests cover ordered requests, uncertain acknowledgements, retired
attachments, bounded queues, key mapping, IME commits, clipboard routing, and
duplicate key/text events. Visual QA exposed two browser-only integration bugs:
native `fetch` was invoked with the wrong receiver, and a keyboard driver could
insert text after a forwarded key despite cancellation. Both now have regression
tests. Backgrounding the page alone no longer discards a healthy attachment.

## Rendered-browser checks

ego-lite drove an isolated gateway and disposable `cat` Session, not the user's
daemon or desktop workspace. The rendered interface was inspected at desktop
size and an emulated 390×844 viewport; the narrow page had no horizontal overflow.

- Taking Control enabled input; a typed marker and Enter appeared exactly once.
- Multiline text opened a preview before transmission. Cancelled text never
  reached output; explicitly confirmed lines did, with Control remaining active.
- An explicit 100×30 resize was confirmed by the runtime's canonical capture.
- Earlier output disabled input and returned Control. Back to live remained
  read-only until another explicit claim.
- Return control disabled input and made Take control available again.
- Explicit Ctrl+] browser key events moved focus to Return control without
  sending the shortcut to the PTY.

The narrow viewport check does not establish physical-phone keyboard, Safari,
Firefox, or mobile authorization support. Clipboard/IME event adapters have
regressions, but complete OS/browser input parity is not claimed.

## Still outside this milestone

Styled terminal cells/cursor fidelity, mouse forwarding, browser workspace
parity, public remote identity/TLS, embedded SDKs, Windows/Linux acceptance,
multi-user production isolation, full automation acceptance journeys, and
sustained resource budgets remain separate work. No performance improvement is
claimed from these correctness checks. See the earlier
[streaming baseline](observer-streaming-baseline.md) for its measured scope and
the [browser design](../design/browser-observer-prototype.md) for usage and limits.
