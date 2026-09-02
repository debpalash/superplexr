# 03 — System architecture

## 1. System shape — A-SYS-000

termi9ne has one durable local runtime and any number of transient clients.

```text
┌──────────────────── desktop process ─────────────────────┐
│ GPUI shell                                                │
│ Mission tabs · Session sidebar · waterfall                │
│ SessionProjection ──> custom TerminalElement              │
└──────────────────────────┬────────────────────────────────┘
                           │ local versioned protocol
┌──────────── CLI ──────────┤
└───────────────────────────┤
┌──────── agent process ────┤ restricted capability channel
└───────────────────────────┤
                            ▼
┌──────────────────── termi9ned runtime ────────────────────┐
│ MissionEngine · Scheduler · AttentionProjection           │
│ RunExecutor · AgentDriver adapters                         │
│ SessionSupervisor                                         │
│   SessionActor[1..N]                                      │
│   PTY + process + canonical TerminalModel + journal       │
│ EventStore · SessionStore · ProtocolHub                   │
└───────────────────────────────────────────────────────────┘
```

### A-SYS-001

The runtime MUST be the sole owner of child processes, PTY masters, canonical
terminal state, Control leases, Mission events, and Session journals.

### A-SYS-002

Clients MUST be replaceable projections. No client may be required for terminal
device replies, output parsing, persistence, scheduling, or process continuity.

### A-SYS-003

V1 runs as one OS user on one authoritative host. Runtime endpoints remain local
Unix-domain sockets and no termi9ne TCP listener is permitted by default. An
opt-in owner Remote attachment MAY ask OpenSSH to forward an owner-only local
Unix socket to the authoritative control socket; SSH owns network
authentication/encryption and the runtime retains its peer-UID check.

### A-SYS-004

A non-owner Share MUST cross a capability-aware authorization seam before it can
reach Mission or Session modules. The runtime owns Share issuance, digest-only
persistence, expiry, scope filtering, and revocation. The implemented Observer
role is read-only. Controller adds scoped terminal interaction through the same
Control lease seam but no lifecycle or graph authority; transport adapters do
not widen either role. Live stream handlers subscribe to revocation and fail
closed on notification lag. Revocation also releases Controller-held leases and
increments their epochs.

## 2. Supported platform contract — A-PLATFORM-001

| Target | Required architecture | Window backend | Renderer | Package |
|---|---|---|---|---|
| macOS | `aarch64`, `x86_64` | AppKit through GPUI | Metal | signed `.app` in notarized `.dmg` |
| Linux | `x86_64` | Wayland and X11 through GPUI | GPUI platform renderer | AppImage and `.deb` |

The Linux executable MUST choose Wayland when available and fall back to X11.
A command-line override MUST select either backend for diagnosis. UI semantics,
keyboard actions, fixtures, and protocol types MUST be shared across platforms.

Platform-specific code is limited to:

- runtime directory and service installation;
- peer-credential lookup;
- notifications, menus, secure token storage, and open-with behavior;
- packaging, code signing, and crash-report integration.

Terminal emulation, Mission behavior, layout rules, and transport semantics MUST
NOT be forked by operating system.

## 3. Target workspace — A-MODULE-001

The implementation converges on these Rust crates. Existing crates may be split
incrementally, but the seams are normative.

| Crate/module | Responsibility | Types it may expose |
|---|---|---|
| `termi9ne-core` | domain decisions and projections | domain types only |
| `termi9ne-store` | atomic Mission event storage and snapshots | store records/errors |
| `termi9ne-terminal` | safe Ghostty adaptation and backend-neutral frames | terminal-domain types only |
| `termi9ne-session` | PTY/process supervision and Session actor | session commands/events |
| `termi9ne-agent` | Run execution, child setup, agent-driver adapters | Run launch types |
| `termi9ne-server::share_store` | bounded Share capability lifecycle and durable digest metadata | Share summaries/authentication result |
| `termi9ne-server::provider_status` | bounded expiring provider facts and explainable Run Activity projection | provider facts and derived activity only |
| `termi9ne-protocol` | v3 framing, control types, terminal messages | transport types |
| `termi9ne-server` | composition root, scheduler, protocol hub | standalone executable plus path-scoped embedded runtime entry point |
| `termi9ne-plugin` | executable-plugin discovery, bounded protocol, supervision, SDK seam | semantic plugin events, manifests, health snapshots |
| `termi9ne-desktop` | GPUI shell, projections, terminal painting | executable; GPUI-local types |
| `termi9ne-cli` | human/script control client | executable |

`termi9ne-core`, `termi9ne-store`, `termi9ne-terminal`, `termi9ne-session`, and
`termi9ne-agent` MUST NOT depend on GPUI. Only `termi9ne-terminal` may depend on
the safe libghostty wrapper; raw FFI is private to that wrapper or its `-sys`
crate. Only `termi9ne-desktop` may expose GPUI types.

## 4. Deep module interfaces — A-INTERFACE-001

Pseudocode specifies behavior, not exact Rust syntax.

### 4.1 MissionEngine — A-MISSION-001

```rust,ignore
trait MissionEngine {
    fn execute(&mut self, command: CommandEnvelope) -> Result<Commit, DomainError>;
    fn load(&self, id: MissionId) -> Result<MissionProjection, LoadError>;
}
```

`execute` validates expected version and idempotency, decides an atomic event
batch, durably commits it, updates projections, and only then returns. Callers do
not append or apply events themselves. This is the interface used by desktop,
CLI, scheduler, agent channel, and tests.

### 4.2 TerminalModel — A-TERMINAL-001

```rust,ignore
struct TerminalModel;

impl TerminalModel {
    fn new(config: TerminalConfig, grid: GridSize) -> Result<Self, TerminalError>;
    fn advance(&mut self, action: TerminalAction)
        -> Result<TerminalEffects, TerminalError>;
    fn snapshot(&mut self, viewport: ViewportRequest)
        -> Result<FullFrame, TerminalError>;
}
```

The module owns raw Ghostty lifetimes, callbacks, device replies, dirty tracking,
frame sequencing, scrollback addressing, and snapshot translation. Callers never
hold Ghostty render-state references.

### 4.3 SessionActor — A-SESSION-001

```rust,ignore
enum SessionCommand {
    Input(InputBatch), Resize(ResizeRequest), TransferControl(ControlTransfer),
    Attach(Subscriber), Detach(ClientId), Interrupt, Terminate, Kill,
    Snapshot(SnapshotRequest), Search(SearchRequest),
}

async fn run_session(
    spec: SessionSpec,
    inbox: Receiver<SessionCommand>,
    records: SessionRecorder,
) -> SessionExit;
```

One task owns each running Session's PTY handle, child handle, TerminalModel, and
ordering counters. Nothing else writes the PTY or mutates terminal state. Tests
drive the same command interface using a fake byte-stream process adapter.

### 4.4 SessionProjection — A-PROJECTION-001

```rust,ignore
struct SessionProjection;

impl SessionProjection {
    fn apply(&mut self, update: TerminalUpdate) -> ProjectionEffect;
    fn frame(&self, viewport: Viewport) -> RenderFrame<'_>;
}
```

One desktop projection exists per attached Session, regardless of Surface count.
It validates sequence, requests resynchronization on a gap, retains the latest
complete rows, and exposes immutable render data. It does not understand PTYs,
Ghostty, or Mission rules.

### 4.5 RunExecutor and AgentDriver — A-AGENT-001

> Preview implementation (2026-09-01): the generic command-driver launch is
> operational for ready planned Runs. Run start, Session creation, and assignment
> commit as one event batch; the daemon-owned PTY installs its lifecycle observer
> before the child can publish; exit completes both domain objects. A bounded
> batch command selects the deterministic scheduler plan inside the daemon,
> resolves each selected Run's named engine, and launches it with per-Run
> results. The daemon overwrites graph identity
> environment values so a caller cannot spoof bindings. Owner-controlled named
> engine drivers resolve structured program/argv/environment config at launch.
> A separate agent channel binds kernel peer credentials to the Run's live PTY
> process group and applies a Run-scoped allowlist on every request. Configured
> drivers can additionally require a fail-closed canonical-workspace write
> sandbox backed by macOS Sandbox or Linux Bubblewrap; its backend/profile and
> inherited-network status are attested in preview and Mission history. Network
> isolation and remote-daemon adapters remain release work.

```rust,ignore
trait AgentDriver {
    fn prepare(&self, run: &RunSpec, capability: AgentCapability)
        -> Result<ProcessSpec, DriverError>;
}

trait RunExecutor {
    async fn launch(&self, request: RunLaunch) -> Result<RunHandle, LaunchError>;
    async fn stop(&self, handle: RunHandle, mode: StopMode) -> Result<(), StopError>;
}
```

The driver translates a configured agent engine into an executable, argv,
environment delta, and declared capabilities. It does not spawn, parse terminal
output, decide domain state, or issue Grants. The executor launches either a
headless child or a Session-backed child, supplies the restricted side channel,
tracks exit, and reports facts to MissionEngine.

V1 includes a generic command driver and configured named drivers. Every preset
uses the same structured signal protocol; provider-specific terminal prose is
never authoritative. The resolved program, argv, working directory, and
environment delta are persisted with the terminal launch specification. A
redacted pre-launch preview exposes environment key names without values and does
not mutate Mission state. Before a process starts, the same atomic Mission batch
records an immutable redacted driver snapshot: driver ID, profile version,
SHA-256 of the complete pre-runtime-injection process spec, argument count, and
sorted environment key names. When configured, the OS adapter also contributes
its bounded sandbox backend/profile and explicit network-isolation fact. Full
argv and environment values remain only in the owner-only terminal launch
specification. A daemon reconciler applies
durable owner-only, opt-in per-Mission policies as capacity opens and recovers
them after restart; disabling a policy never terminates active work. A serialized
launch gate enforces one owner-configurable global agent cap across explicit,
batch, and continuous launches, and reconciliation rotates Mission order for fairness.
Trusted event-store timestamps drive dependency-aware `ready_since` and
five-minute priority aging after restart. The default cap is twelve; values from
one through 256 persist atomically beside host scheduler policies and are
reported by diagnostics.

Existing-Session launch requires the verified idle-shell contract in section 02.
Shell integration reports prompt readiness and command completion through the
restricted side channel, not escape-sequence regexes. An unaware shell remains a
valid ordinary Session but cannot receive automatic sequential Run launches.

### 4.6 TerminalElement — A-ELEMENT-001

The custom GPUI element accepts a `RenderFrame`, font metrics, selection, and
viewport. It computes cell geometry, batches backgrounds/glyphs/decorations,
paints a cursor, and reports hit-test coordinates. It MUST NOT create a general
purpose view node per cell.

## 5. Runtime concurrency — A-CONCURRENCY-001

The daemon uses Tokio with these ownership rules:

- one bounded mailbox and task per running Session;
- one serialized Mission commit writer, sharded by Mission only after profiling;
- one protocol read task and one bounded write queue per connection;
- blocking FFI or disk work runs outside the async executor when it can exceed
  500 microseconds;
- no mutex guard is held across `.await`;
- a slow client can lose deltas and resynchronize, but cannot block PTY parsing,
  process waits, device replies, event commits, or another client.

Within a Session, the total order is PTY output, accepted input, resize, and
control commands as dequeued by the Session actor. Domain commits that authorize
control transfer occur before the Session actor accepts the new epoch.

The desktop runs GPUI state and painting on its main thread. Socket decode,
history decompression, and expensive glyph preparation run on background
executors and publish immutable batches to GPUI entities.

## 6. Storage layout — A-STORAGE-001

Runtime state uses platform application-data directories, never the current
repository in a packaged build:

```text
state/
  manifest.json
  missions/<mission-id>/events-000001.t9log
  missions/<mission-id>/snapshot.t9snap
  sessions/<session-id>/metadata.json
  sessions/<session-id>/output-000001.t9raw
  sessions/<session-id>/frames-000001.t9frame
  sessions/<session-id>/checkpoint.t9frame
  view/<device-id>.json
run/
  control.sock
  agent/<run-id>.sock
logs/
  runtime.jsonl
```

On Linux, `run/` uses `$XDG_RUNTIME_DIR/termi9ne` and state uses
`$XDG_STATE_HOME/termi9ne` with standards-compliant fallbacks. On macOS, state
uses `~/Library/Application Support/termi9ne` and the socket uses a private
per-user runtime directory whose path length fits Unix-socket limits.

Every directory MUST be mode `0700`; state files MUST be `0600`. Session output
and Mission events MUST never be stored in a project directory unless the user
explicitly selects a portable development profile.

## 7. Configuration — A-CONFIG-001

Configuration precedence is command-line, environment, per-project file, user
file, then defaults. Unknown keys produce warnings; invalid security or storage
keys prevent startup. Configuration reload MAY change visual, notification,
retention, and concurrency settings. It MUST NOT silently change a running
Session's shell, environment, TERM, encoding, or control policy.

Secrets MUST NOT be accepted directly in configuration files. Secret references
use OS secure storage or environment-variable names.

## 8. Startup and shutdown — A-LIFETIME-001

Runtime startup MUST:

1. acquire a single-instance lock;
2. validate ownership and permissions of state/runtime directories;
3. open and recover event/session stores;
4. mark previously running Sessions `Lost` because v1 cannot recover PTY handles
   after daemon death;
5. bind the socket and verify peer-credential support;
6. publish readiness atomically.

Desktop startup discovers or launches the runtime, completes a version
handshake, restores presentation state, subscribes to active Missions, and then
opens its first window. A protocol mismatch shows a repair action rather than an
empty workspace. It MUST NOT be collapsed into a launch timeout and MUST NOT
replace a live incompatible daemon. A self-hosted desktop daemon uses server code
from the same build; debug runtimes use versioned state/socket paths so local
development can coexist with an older packaged runtime and its live PTYs.

Closing the last desktop window disconnects the client only. Runtime shutdown is
an explicit command. Graceful runtime shutdown previews active Sessions and
requires a choice to keep the runtime running, terminate selected Sessions, or
cancel shutdown.

## 9. Error taxonomy — A-ERROR-001

Errors cross a seam as stable categories with actionable context:

- `invalid_request`, `conflict`, `not_found`, `not_controller`, `not_ready`;
- `unsupported_version`, `capability_denied`, `resource_exhausted`;
- `pty_spawn_failed`, `process_failed`, `terminal_failed`;
- `storage_unavailable`, `storage_corrupt`, `recovery_required`;
- `internal` with a correlation ID and no sensitive details.

Panics indicate programmer defects. Runtime request and Session tasks MUST catch
task failure, record context, isolate the affected Mission/Session where possible,
and keep unrelated Sessions alive.

## 10. Dependency direction — A-DEPENDENCY-001

```text
desktop ───────> protocol ───────> core
cli ───────────> protocol ───────> core
server ────────> protocol/core/store/session/agent
session ───────> terminal/core
agent ─────────> protocol/core/session
server ────────> plugin ─────────> core
terminal ──────> libghostty-vt safe wrapper
store ─────────> core
```

No arrow may reverse. In particular, core code cannot call transport, storage,
terminal, or UI code. This keeps domain tests deterministic and prevents
framework types from becoming project vocabulary.

## 11. Plugin supervision — A-PLUGIN-001

The durable runtime owns executable Plugin processes. Its `PluginSupervisor`
interface accepts semantic events with a non-blocking bounded enqueue and
returns immutable health snapshots; callers never manage child handles, pipes,
handshakes, restart timing, or plugin queues. With no installed plugins the
module starts no process or worker thread.

Plugin manifests and the JSONL plugin protocol are independently versioned.
Capabilities filter events before serialization. V1 executable plugins MAY
observe bounded terminal lifecycle and Run Activity replacements and MAY publish
bounded status contributions. They MUST NOT receive terminal cells, PTY bytes,
environment values, command arguments, Mission intent, or repository paths, and
they MUST NOT execute on terminal, daemon request, or GPUI threads.

Every plugin has a bounded writer queue. A full plugin queue drops that plugin's
replaceable semantic observation and increments diagnostics; it never applies
backpressure to the runtime publisher. Handshake failure, malformed output,
unauthorized contribution, blocked I/O, and child exit affect only that plugin
and enter bounded exponential restart backoff.
