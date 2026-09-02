# termi9ne

An agent-native execution environment built around missions, runs, and human
attention—not windows, tabs, and panes.

Traditional terminal multiplexers preserve processes and arrange terminal
rectangles. termi9ne's durable state is a graph of work: what is being attempted,
who is doing it, which runs depend on one another, what needs a human decision,
and which artifacts were produced. A terminal Surface is one transient projection
of a durable Session, which may host agent Runs or ordinary shell work.

## Product shape

- **Browser workspace**: each Mission is a tab with its own Session sidebar and
  responsive waterfall of embedded terminal Surfaces.
- **Attention queue**: blocked runs, questions, and risky approvals rise above
  routine output.
- **Structured signals**: agents report progress, questions, approvals, and
  artifacts over a local control protocol. Terminal prose remains terminal
  prose.
- **Takeover and return**: a human can take control of a Session used by a Run
  and explicitly return it to the agent.
- **Durable history**: commands become domain events, allowing reconstruction,
  audit, replay, and future time-travel views.
- **Disposable views**: closing a Surface, Mission tab, or the app does not define
  the lifetime of a Session, Run, or Mission.

## Architecture

```text
termi9ne desktop (one Rust app for macOS and Linux)
  Mission tabs + Session sidebar + responsive terminal waterfall
  GPUI + custom TerminalElement + shared SessionProjection
                              |
          binary terminal data + JSON control protocol
                              |
termi9ne runtime ----------------------------- agent side-channel
  mission/run events + durable Sessions + PTYs + canonical libghostty-vt state
```

The daemon uses public `libghostty-vt` terminal state through a safe Rust module
and publishes backend-neutral full frames followed by sequenced row deltas. GPUI provides the
shared macOS/Linux desktop, while a custom `TerminalElement` paints terminal
frames without exposing Ghostty or GPUI types across their seams. The private
full-Ghostty surface interface remains excluded because it is not a stable Linux
interface. The complete v1 design is in
[`docs/architecture/v1-cross-platform-multiplexer.md`](docs/architecture/v1-cross-platform-multiplexer.md).
The product interaction, visual, protocol, and delivery plan is in
[`docs/design/browser-waterfall-v1.md`](docs/design/browser-waterfall-v1.md).
The evidence required before termi9ne calls itself superior is recorded in
[`docs/product/north-star.md`](docs/product/north-star.md).
The normative v1 product and engineering contract starts at
[`docs/spec/README.md`](docs/spec/README.md), covering the execution graph,
module interfaces, terminal protocol, desktop behavior, security, reliability,
quality gates, and delivery plan.

The repository now owns real PTYs in a durable per-user daemon. The desktop can
exit and reattach without ending Sessions; retained raw journals reconstruct the
final Ghostty frame after daemon loss. The implemented v3 local wire profile
uses a fixed 32-byte header, Hello/Welcome negotiation, gapless per-stream
sequences, bounded zstd, JSON control messages, and protobuf terminal frames. It
supports terminal-index discovery, terminal subscriptions and delta
resynchronization, push-based Mission updates, bounded causal Mission history,
one controller with multiple observers, per-Surface control epochs, revocable
digest-backed Observer and scoped Controller Shares, optimistic Mission versions,
durable idempotency keys, durable exact-base Run checkouts, structured terminal
capture and bounded event-driven waits, and redacted runtime diagnostics. The
current request schema is version 25. The desktop, native client, and CLI event
stream carry independently cancellable subscription classes beside control
requests on one physical connection.
The release wire contract is defined in
[`docs/spec/05-local-protocol.md`](docs/spec/05-local-protocol.md).

## Run the desktop

The native desktop boots on macOS and Linux, starts an exact-version self-hosted
daemon when needed, restores Mission tabs, and attaches real terminal Sessions. Focus a grid
to type, use the Mission sidebar to switch Sessions, and open the command deck
with Cmd/Ctrl-K. Unsafe multiline paste is held for explicit confirmation:

```sh
cargo run -p termi9ne-desktop
```

Graphite and Paper are built in under **View → Theme**. A custom data-only theme
can be loaded without placing code in the desktop process:

```sh
cargo run -p termi9ne-desktop -- --theme docs/examples/theme.json
```

Custom themes use the bounded, strict JSON schema in
[`docs/examples/theme.json`](docs/examples/theme.json). Theme selection persists
per device; **View → Theme → Reload Theme File** reparses and atomically applies
changes. Invalid colors, fonts, fields, versions, file types, sizes, or contrast
leave the previous theme active. Theme parsing and file I/O never occur in
terminal paint paths.

Debug desktops use `.termi9ne-dev/v25` so they can run beside an older packaged
runtime without taking over its socket or state. Release builds use `.termi9ne`.
An explicit incompatible `--socket` fails before GPUI starts, reports both wire
versions, and never replaces a daemon that may own live PTYs.

## Install the first executable plugin

Executable plugins are supervised by the durable runtime and never run in GPUI,
terminal parsing, or paint. Build and install the bundled Agent Status proof
plugin before starting the runtime. If it is already running, restart the
durable runtime; closing only the desktop does not stop it:

```sh
cargo build -p termi9ne-plugin --bin termi9ne-agent-status-plugin
cargo run -p termi9ne-cli -- plugin-install-agent-status \
  target/debug/termi9ne-agent-status-plugin \
  --plugin-dir .termi9ne-dev/v25/plugins
cargo run -p termi9ne-cli -- \
  --socket .termi9ne-dev/v25/control.sock plugin-list
```

The shown path targets `cargo run -p termi9ne-desktop`; packaged builds use
`.termi9ne/plugins`. The installer refuses to replace an existing plugin.
Manifests and executable paths must be owner-controlled, non-symlink files under
the selected plugin directory. Runtime health is visible under
**Tools → Plugins**.
Plugins receive capability-filtered terminal lifecycle and Run Activity events,
not terminal contents. Event publication uses bounded `try_send`; slow plugins
drop replaceable observations and expose counters instead of blocking the daemon
or desktop. The first-party plugin proves handshake, observation, status,
shutdown, and restart isolation. Installation/enable/disable UI, live discovery,
signed packages, command contributions, and a WASM adapter remain later slices
behind this same seam.

Workspace tabs use browser navigation: Cmd/Ctrl-T creates, Cmd/Ctrl-W closes,
Cmd/Ctrl-Shift-T restores, Cmd/Ctrl-Tab cycles, and Cmd/Ctrl-1 through 9 jumps
directly. New Session, new terminal, sidebar, focus mode, and graph inspector
also have native macOS and Linux bindings discoverable from the command deck.

The verified-delivery inspector can create an independent verifier or a retry
from returned work. The runtime freezes the exact Candidate, harness, and Return
note onto the new Run, prepares a separate Candidate checkout off the UI thread,
and refuses to launch that workflow from an unrelated working directory.

Rust 1.97.1 is selected by `rust-toolchain.toml`; Zig 0.16.0 must be available
on `PATH`. A reproducible Linux verification, including both Wayland and X11
features, is available when Docker is installed:

```sh
docker build --file ci/linux.Dockerfile --tag termi9ne-ci .
```

On a graphical macOS, Wayland, or X11 host, run the six-PTY output/render gate:

```sh
ci/desktop-render-benchmark.sh
```

The companion quiet-state gate renders twelve PTYs and enforces desktop CPU,
runtime CPU, combined RSS, and startup telemetry:

```sh
ci/desktop-idle-benchmark.sh
```

On macOS, the native-input smoke launches an isolated desktop, posts real
CoreGraphics keyboard events into its focused terminal, and verifies both PTY
echo and retained multiplexed subscriptions:

```sh
ci/desktop-input-smoke.sh
```

## Try the control plane

Run the daemon in one terminal:

```sh
cargo run -p termi9ne-server
```

Create and inspect a mission from another:

```sh
cargo run -p termi9ne-cli -- create "Ship the first agent-native terminal"
cargo run -p termi9ne-cli -- list
cargo run -p termi9ne-cli -- status
cargo run -p termi9ne-cli -- schedule MISSION_ID --max-concurrency 4
cargo run -p termi9ne-cli -- run-launch MISSION_ID RUN_ID --program /usr/bin/env -- bash -lc 'your-agent-command'
cargo run -p termi9ne-cli -- schedule-launch MISSION_ID --max-concurrency 4 --program /usr/bin/env -- your-agent-command
cargo run -p termi9ne-cli -- schedule-engine-launch MISSION_ID --max-concurrency 4
cargo run -p termi9ne-cli -- schedule-auto MISSION_ID --max-concurrency 4
cargo run -p termi9ne-cli -- schedule-auto-list
cargo run -p termi9ne-cli -- schedule-settings --global-max-concurrency 12
cargo run -p termi9ne-cli -- schedule-settings-show
cargo run -p termi9ne-cli -- run-engine MISSION_ID RUN_ID
cargo run -p termi9ne-cli -- run-engine-preview MISSION_ID RUN_ID
cargo run -p termi9ne-cli -- run-checkout-new MISSION_ID RUN_ID --repository /path/to/repo --base-ref main
cargo run -p termi9ne-cli -- run-engine MISSION_ID RUN_ID --checkout
cargo run -p termi9ne-cli -- run-checkout-list --mission-id MISSION_ID
cargo run -p termi9ne-cli -- evidence-check MISSION_ID RUN_ID --provider github --adapter-version 1 --key check/test --revision COMMIT --name test --state passed --summary "tests passed"
cargo run -p termi9ne-cli -- evidence-review MISSION_ID RUN_ID --provider github --adapter-version 1 --key pr/42 --revision COMMIT --title "Ship feature" --state approved --summary "review approved"
cargo run -p termi9ne-cli -- evidence-list MISSION_ID RUN_ID
cargo run -p termi9ne-cli -- run-checkout-retire MISSION_ID RUN_ID --merged-into-ref main
cargo run -p termi9ne-cli -- provider-report MISSION_ID RUN_ID --provider codex --adapter-version 1 --state working --summary 'executing a tool'
cargo run -p termi9ne-cli -- provider-status MISSION_ID RUN_ID
cargo run -p termi9ne-cli -- provider-list MISSION_ID
cargo run -p termi9ne-cli -- terminal-capture SESSION_ID
cargo run -p termi9ne-cli -- terminal-wait-text SESSION_ID 'ready>' --timeout-millis 30000
cargo run -p termi9ne-cli -- terminal-wait-quiet SESSION_ID --quiet-millis 500 --timeout-millis 30000
cargo run -p termi9ne-cli -- terminal-wait-exit SESSION_ID --timeout-millis 300000
cargo run -p termi9ne-cli -- events --scope all
cargo run -p termi9ne-cli -- terminal-ssh user@host --port 22
cargo run -p termi9ne-cli -- terminal-history SESSION_ID --rows-before-bottom 500
cargo run -p termi9ne-cli -- terminal-list --all
cargo run -p termi9ne-cli -- terminal-archive SESSION_ID
cargo run -p termi9ne-cli -- terminal-restore SESSION_ID
cargo run -p termi9ne-cli -- session-group-create "review agents" --session SESSION_ID
cargo run -p termi9ne-cli -- session-group-list
cargo run -p termi9ne-cli -- session-group-rename GROUP_ID GROUP_VERSION "landing queue"
cargo run -p termi9ne-cli -- session-group-detach GROUP_ID GROUP_VERSION
cargo run -p termi9ne-cli -- session-group-reattach GROUP_ID GROUP_VERSION
cargo run -p termi9ne-cli -- events --scope groups
```

Every command accepts `--socket`; the default is `.termi9ne/control.sock` in the
current workspace. Mission mutations also accept `--expected-version` and
`--idempotency-key`. The daemon persists owner-only enveloped event journals
under `.termi9ne/missions` and raw terminal output under `.termi9ne/sessions`.
`status` reports daemon identity, platform, uptime, connection/subscriber counts,
Mission and scheduler-policy counts, global agent capacity, and terminal
lifecycle counts without exposing state paths, environment values, commands, or
terminal contents.
`schedule` is a read-only deterministic orchestration preview: it separates
startable agent Runs from capacity-queued, dependency-waiting,
dependency-blocked, and manual-human work. Process launch remains behind an
explicit command driver rather than guessing an executable from an engine name.
Ready work is ordered by effective priority, durable `ready_since`, then Run ID.
Every five ready minutes promotes one priority level, preventing Background work
from starving while preserving deterministic preview/replay behavior.
`schedule-launch` applies that same plan under an explicit concurrency cap and
launches every selected Run into its own durable PTY. It reports successes and
races per Run, and a retry naturally considers only work still pending.
`schedule-engine-launch` performs plan selection and named-driver resolution in
the daemon, then launches the frozen startable set with per-Run results. A second
reconciliation while all slots are occupied starts nothing, and a missing or
invalid driver leaves that Run pending with an explicit failure result.
`schedule-auto` persists an owner-only per-Mission policy and enables the daemon
reconciler. It continuously launches newly ready configured-engine Runs as slots
open, survives daemon restart, respects the owner-configurable runtime-wide
agent cap, and never starts manual-human work. `schedule-settings` atomically
persists that cap (1..=256); the default is twelve and `status` reports the same
authoritative setting plus current occupancy. Re-run `schedule-auto` with
`--disable` to stop new launches without terminating active Runs;
`schedule-auto-list` reports the durable policy set. Invalid drivers remain
pending and do not create partial graph or terminal state.
`run-launch` is the generic command-driver boundary: it atomically starts a ready
planned Run, creates and assigns its domain Session, and binds both IDs to a new
daemon-owned PTY. The runtime observes the child before it can emit or exit;
process completion then durably finishes the Session and Run. Launch failure is
recorded through a compensating failed completion, and retrying the same bound
Session is idempotent.
Agent processes receive daemon-owned `TERMI9NE_MISSION_ID`, `TERMI9NE_RUN_ID`,
and `TERMI9NE_SESSION_ID` environment values; caller-supplied values cannot spoof
those bindings.
They also receive an owner-only `TERMI9NE_AGENT_SOCKET`. The daemon authenticates
its kernel-reported peer PID against the live PTY process group and permits only
Run-scoped reads, Signals, Artifacts, and configured-driver preview. Global
listing, approval resolution, Grant issuance, unrelated Runs, and terminal
control are denied; authorization is rechecked and revoked on Run or Session
completion.
`run-engine` resolves the Run actor's engine through `.termi9ne/engines.json`.
Configuration is structured argv—never an interpolated shell string—and is
reloaded for each launch. For example:

```json
{
  "version": 1,
  "drivers": {
    "my-agent": {
      "program": "/absolute/path/to/my-agent",
      "args": ["run"],
      "append_objective": true,
      "environment_delta": { "AGENT_MODE": "durable" },
      "sandbox": "workspace_write"
    }
  }
}
```

Plan the Run with `--engine my-agent`; the daemon selects that driver, appends
the objective as one argument when requested, and rejects insecure config,
unknown fields, unsafe sizes, NULs, and reserved identity environment keys.
`run-engine-preview` resolves the same launch without mutating the Mission or
starting a process; it returns environment key names but redacts their values.
`run-checkout-new` resolves an exact Git commit and prepares one owner-only,
deterministically named worktree for a pending Run. `run-engine --checkout`
launches the configured driver there. Checkout records survive daemon restart;
Git hooks are disabled for lifecycle commands, paths cannot escape the managed
root, and retirement is explicit. `run-checkout-retire` refuses while the Run or
one of its terminals is live, while tracked/untracked changes exist, or while
the checkout branch is not reachable from the exact `--merged-into-ref` target.
There is deliberately no force-retire operation.
`provider-report` records one bounded, expiring adapter observation with a
server-authored timestamp and provenance. Reports through a Run's authenticated
agent socket are distinguished from owner hooks. `provider-status` explains the
derived Activity projection: finished/pending lifecycle and unresolved typed
Signals outrank fresh provider evidence; expired or missing evidence becomes
`unknown`, never guessed `idle`. Provider facts cannot complete Runs or resolve
attention. Activity expiry and replacements are pushed into the desktop sidebar
and `events`; rendered output is used only as the conservative fallback beneath
explicit evidence. `events` emits race-free Mission, Activity, and terminal-index
snapshots followed by NDJSON updates, so scripts can react without polling or
screen scraping.
The optional fail-closed `workspace_write` sandbox canonicalizes the working
directory, exposes the host read-only, permits writes only inside that workspace,
and re-protects `.termi9ne`. It uses `/usr/bin/sandbox-exec` on macOS and
Bubblewrap on Linux. Network remains available so agents can reach providers and
their restricted local channel; preview and Mission history say so explicitly
rather than implying network isolation. If the platform backend is missing, the
Run stays pending and no unsandboxed fallback occurs.
Every successful generic or configured launch first commits an immutable driver
snapshot to the Run. The snapshot carries a SHA-256 digest of the complete
pre-injection process spec, driver/profile identity, argument count, and sorted
environment key names plus any enforced sandbox backend/profile; arguments and
environment values never enter Mission history.
`terminal-ssh` starts OpenSSH as another daemon-owned PTY, so it survives desktop
closure and receives the same history, control-epoch, sidebar, and waterfall
behavior as a local shell. It uses the existing user SSH configuration/agent and
does not accept an interpolated remote command.

An owner can also attach the complete native workspace to one authoritative
runtime on another macOS or Linux host. Start that host's daemon, create a new
owner-only directory on the client, then forward its control socket through
OpenSSH:

```sh
mkdir -m 700 /absolute/local/termi9ne-remote
cargo run -p termi9ne-cli -- remote-forward user@host \
  --local-socket /absolute/local/termi9ne-remote/control.sock \
  --remote-socket /absolute/remote/project/.termi9ne/control.sock \
  --identity /absolute/path/to/id_ed25519 \
  --known-hosts /absolute/path/to/known_hosts

cargo run -p termi9ne-desktop -- \
  --socket /absolute/local/termi9ne-remote/control.sock \
  --connect-only --runtime-label staging
```

The supervisor reconnects with bounded backoff, never asks the runtime to listen
on TCP, refuses unsafe or occupied socket paths, and enables strict host-key
checking when a dedicated known-hosts file is supplied. `--connect-only` is a
hard safety boundary: loss of the tunnel cannot silently start a second local
runtime. This grants the remote SSH account the same trusted-owner authority as
a local desktop; it is not a revocable multi-user Share.

For a collaborator, mint a bounded Share instead of sharing owner SSH access.
Scope it to explicit Missions and/or Sessions and capture the one-time token in
an owner-only file. Observer is the default read-only role:

```sh
(umask 077; target/debug/termi9ne share-create reviewer \
  --mission MISSION_ID --expires-in-seconds 86400 | jq -r .token > reviewer.token)

target/debug/termi9ne --share-token-file reviewer.token list
target/debug/termi9ne-desktop --socket /absolute/path/control.sock \
  --connect-only --share-token-file reviewer.token

target/debug/termi9ne share-list
target/debug/termi9ne share-revoke SHARE_ID
```

Observers receive only scoped Mission/terminal reads and live updates. Terminal
input/control, process lifecycle, graph mutation, scheduler administration,
runtime-wide diagnostics, and Share administration are denied. To grant
interactive terminal input without owner authority, use
`share-create collaborator --role controller --session SESSION_ID`. Controller
may acquire the scoped terminal's Control lease without force and send terminal
input, focus, mouse, selection, scroll, paste, and resize operations. It still
cannot force takeover, launch/kill/archive processes, mutate Missions, schedule
work, inspect runtime diagnostics, or administer Shares. Expiry or revocation
releases its leases and closes live streams promptly. Tokens never enter argv and
only their SHA-256 digests persist. The current channel is the local Unix protocol
(and may be carried by an encrypted owner-managed transport); a public
browser/mobile TLS gateway is separate release work.

`terminal-history` reads deterministic viewports from the daemon's retained raw
journal without moving a live surface: offset zero is the bottom and increasing
row offsets move backward through the bounded 100,000-line Ghostty scrollback.
Exited Sessions remain pageable and searchable after daemon recovery.
`terminal-archive` removes an exited terminal from active workspace projections
without deleting its raw journal, launch specification, Mission/Run binding, or
searchable history. Archived terminals survive daemon restart, appear in the
desktop Archive drawer and `terminal-list --all`, and can be restored to their
owning workspace. A running terminal must be stopped before it can be archived.

## Verification and packages

The macOS package smoke creates an ad-hoc-signed `.app` archive. Linux CI uses
pinned `linuxdeploy`, `appimagetool`, and type-2 runtime inputs to create a
self-contained AppImage alongside an AppDir archive and `.deb`; it tests both
Wayland and X11 windows and runs the same real-PTY soak. All bundles compile and
install the included
`xterm-ghostty` terminfo rather than assuming it is present globally. Packaging
also emits and embeds an SPDX 2.3 dependency graph, collected third-party
license/NOTICE texts, and a `SHA256SUMS` manifest.

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
TERMI9NE_SOAK_SECONDS=60 TERMI9NE_SOAK_SESSIONS=12 ./ci/runtime-soak.sh .
```

Executed evidence and remaining release gaps are kept separate in
[`docs/engineering/m2-runtime-release-evidence.md`](docs/engineering/m2-runtime-release-evidence.md).
A package smoke is not production signing/notarization, and the Linux Docker
matrix must pass on an available Docker host before a cross-platform release
claim is made.

## Repository map

- `termi9ne-core`: event-sourced Mission, Run, Session, and attention model.
- `termi9ne-protocol`: negotiated v3 framing, sequencing, compression, JSON control, and protobuf terminal codecs.
- `termi9ne-server`: durable local runtime and Unix-socket server.
- `termi9ne-cli`: human- and agent-usable control client.
- `termi9ne-terminal`: product-owned libghostty adapter and semantic frames.
- `termi9ne-desktop`: GPUI application shell and custom terminal painter.
- `gpui-*-compat` / `ztracing-compat`: small permissively licensed seams that
  keep unapproved Zed auxiliary crates out of the product graph.
- `CONTEXT.md`: canonical product language.
- `docs/adr`: decisions whose trade-offs should remain visible.
- `docs/architecture`: executable design for upcoming implementation slices.
- `docs/design`: product behavior, interaction, and visual plans.
- `docs/product`: product thesis and measurable release gates.
- `docs/engineering`: implementation audits and milestone evidence.
