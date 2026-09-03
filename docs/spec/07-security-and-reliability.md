# 07 — Security and reliability

## 1. V1 trust statement — S-TRUST-001

superplexr v1 is a single-user local application. The desktop and CLI are trusted
clients belonging to the logged-in OS user. Agent messages, terminal applications,
terminal output, repository contents, pasted text, links, and artifact locators
are untrusted input.

Unless a Session is launched by an OS sandbox adapter, its child processes run
with the user's ordinary filesystem and network authority. Such a process is
inside the same-UID OS trust boundary: it may be able to discover runtime files
or imitate a trusted local client even though its supplied agent channel is
restricted. Channel capabilities prevent accidental and protocol-level misuse;
they are not process containment. Recorded approval does not change OS authority.
The interface MUST say `cooperative` and MUST NOT claim enforcement.

## 2. Threat model — S-THREAT-001

V1 MUST mitigate:

- another local user connecting to the runtime socket or reading state;
- an agent using its supplied restricted channel beyond its Run;
- forged or replayed control/input messages;
- terminal escape sequences attempting clipboard writes, link execution, title
  spoofing, memory exhaustion, parser corruption, or focus theft;
- malicious repositories influencing shell startup, working directory, command
  previews, artifact paths, or package scripts;
- a slow or malformed client blocking PTY consumption;
- crashes or power loss corrupting Mission history;
- logs, diagnostics, or telemetry leaking terminal content or secrets;
- dependency or update tampering.

V1 does not claim protection from:

- the same OS user reading process memory or state files;
- root, kernel, hypervisor, or physical compromise;
- unrestricted child processes accessing everything the OS user can access;
- a malicious unrestricted same-UID child discovering or imitating a trusted
  desktop/CLI connection without an OS sandbox;
- malicious shell configuration deliberately executed by the user;
- denial of service by a process the user allowed to consume machine resources.

## 3. Local authentication — S-AUTH-000

### S-AUTH-001

The control socket directory MUST be owned by the current user and mode `0700`;
the socket MUST be mode `0600`. The runtime refuses symlinks, unexpected owners,
group/world access, and pre-existing non-socket paths.

### S-AUTH-002

For every desktop/CLI connection the runtime MUST verify peer UID using
`getpeereid` on macOS and `SO_PEERCRED` on Linux. A same-user check is required
even when filesystem permissions appear correct.

### S-AUTH-003

Agents MUST NOT receive the general control socket. Each receives a Run-scoped
socket or inherited connected descriptor and a 256-bit random capability token
through a close-on-exec-controlled file descriptor. Tokens MUST NOT appear in
argv, environment values, state JSON, logs, crash reports, or child descendants
unless delegation explicitly transfers a narrower token.

Capability comparison is constant time. Tokens expire at Run finish/revocation,
are single-host, and rotate when control policy changes.

S-AUTH-003 scopes the channel superplexr supplies to an agent. It MUST NOT be cited
as containment of the agent process. An OS sandbox adapter may upgrade this claim
only after its filesystem, socket, process, and network bypass tests pass on the
specific platform.

### S-AUTH-004 — Owner Remote attachment

The runtime does not expose TCP. An owner Remote attachment delegates host and
user authentication, confidentiality, integrity, rekeying, and optional jump
hosts to OpenSSH, and forwards an owner-only local Unix socket to the existing
control socket. The remote sshd connects under the runtime owner's UID, so the
runtime still performs its normal kernel peer-UID check.

The client-side forwarding directory MUST be a real directory owned by the
current user with no group/world permissions. The target socket MUST be absent;
automatic unlink of pre-existing paths is forbidden. The supervisor may remove
only the exact owner-owned Unix socket it created. It MUST use bounded reconnect
backoff, keepalives, and connect-only client mode so channel loss cannot create a
split-brain runtime. Supplying a dedicated known-hosts database MUST also enable
strict host-key matching.

SSH access as the runtime owner grants the full trusted-client authority already
available to that owner. It is not a multi-user Share, a narrower role, or a
revocable invitation. Those require separate device/person identity and
method-level authorization.

### S-AUTH-005 — Observer Shares

An Observer Share uses a 256-bit random capability scoped to bounded explicit
Mission and/or Session IDs and a bounded lifetime. The plaintext is emitted once
and MUST be transferred only over an encrypted channel; persistent state keeps
only its SHA-256 digest. Clients load it from a real owner-only file rather than
argv. Comparison is constant-time and error responses do not distinguish a bad,
expired, unknown, or revoked secret.

Observers may read scoped Mission projections/history, deterministic scheduler
plans, scoped terminal indexes/frames/history/search, and their live streams.
They may not mutate Missions, launch/control/archive processes, acquire Control,
change scheduling, read runtime-wide diagnostics, or administer Shares. List and
event responses are filtered as well as direct reads. Revocation is durable and
closes active subscriptions immediately; channel lag fails closed. Controller is
the separate allowlist in S-AUTH-006; future roles likewise cannot be implemented
by silently expanding Observer.

### S-AUTH-006 — Controller Shares

A Controller Share is a separate role and allowlist, not an expanded Observer.
It inherits scoped reads and may claim scoped terminal Control only with
`force=false`, release it, and send focus, key, paste, mouse, scroll, selection,
selection-clear/read, and resize requests carrying the current epoch. It cannot
force takeover, create/interrupt/terminate/kill/archive/restore processes,
mutate the Mission graph, launch or schedule Runs, read runtime-wide diagnostics,
or administer Shares.

The terminal projection records the controlling Share ID in addition to client
and Surface IDs. Disconnect releases that client's leases. Expiry or owner
revocation invalidates the capability, terminates its streams, clears every
lease held by that Share, and increments each affected epoch before publishing
the replacement projection. Terminal input authority is equivalent to shell
authority inside the scoped process and MUST be communicated as such when the
owner creates or transfers the capability.

## 4. Authorization — S-AUTHZ-001

Every method has an authorization class: trusted-client, Run capability, Session
controller, Grant issuer, or runtime-internal. Authorization is checked before
payload-dependent side effects and again inside the owning Session/Mission module
to prevent confused-deputy paths.

Client-kind authorization is trustworthy for the restricted agent channel and
for sandboxed processes. In the cooperative unsandboxed profile, it does not
defend against a malicious process acting with the user's full local authority;
the runtime and interface expose that profile explicitly.

A Grant decision contains exact operation class and normalized resource scope.
String prefixes are not valid path scope checks. Filesystem scope uses resolved
directory handles or canonical paths with explicit symlink policy. Network scope
uses normalized host, port, and protocol.

`Allow once` is consumed atomically with the authorized operation. Expired,
revoked, wrong-Actor, wrong-Run, or scope-mismatched Grants fail closed. A
cooperative Grant records intent but does not claim that an unrestricted process
was technically prevented from bypassing it.

## 5. Terminal-output safety — S-TERMINAL-001

Terminal bytes are untrusted parser input. The implementation MUST:

- use only the audited safe `superplexr-terminal` seam for Ghostty FFI;
- bound escape sequence, title, hyperlink, clipboard, image, and frame sizes;
- deny OSC 52 writes by default and never allow reads without a user-mediated
  policy;
- require a user gesture to open links and show the normalized destination;
- ignore escape sequences that attempt application focus or arbitrary process
  execution outside documented terminal semantics;
- isolate parser failure to one Session and retain the raw offset for diagnosis;
- fuzz terminal translation, frame decode, and history restore continuously.

Kitty graphics, if enabled, uses decoded pixel, dimension, image-count, and
memory caps. Images cannot resolve external URLs or arbitrary file paths.

## 6. Process launch safety — S-PROCESS-001

No shell is inserted when the caller provides an executable and argv. Command
Deck shell commands explicitly say they use the Session shell. Previews quote
argv for display but never reconstruct execution from the quoted string.

The runtime strips its own capability descriptors/tokens from unrelated children,
sets close-on-exec on runtime descriptors, and never inherits the control socket.
Sensitive environment values are redacted in UI and diagnostics by key policy;
the unredacted effective environment remains accessible only through an explicit
local inspection action.

Working-directory and worktree creation reject paths escaping the selected root
when isolation was requested. Cleanup previews exact paths and uses recoverable
trash where practical. superplexr never recursively deletes a workspace root.

Managed Run checkouts resolve the repository root and base commit before any Git
mutation, use deterministic paths beneath an owner-only runtime directory, and
invoke Git through structured arguments with lifecycle hooks disabled. A
checkout belongs to one Run rather than a Session. Retirement MUST reject a live
Run or terminal, tracked or untracked changes, and a checkout HEAD not reachable
from the explicit target ref. The runtime uses Git's non-forced worktree removal
and merged-branch deletion; it exposes no force-cleanup method. Restart security
hardening protects the container directory without rewriting Git-recorded file
modes or rejecting repository symlinks.

Configured drivers may require the `workspace_write` OS sandbox profile. The
adapter canonicalizes the workspace and runtime-state paths before launch,
mounts or authorizes host reads, allows workspace writes, and explicitly denies
writes into the runtime state directory even when it is nested in the workspace.
macOS uses the system Sandbox profile runner; Linux uses Bubblewrap with a
read-only root, writable workspace bind, read-only state rebind, dropped
capabilities, and separate user/process/IPC/UTS namespaces. Missing backends fail
the launch before graph mutation. This profile intentionally inherits networking
and MUST be presented as filesystem-write containment, not network isolation.

## 7. Storage integrity — R-STORE-000

### R-STORE-001 — Mission commits

A Mission commit is:

1. validate expected version and idempotency;
2. serialize one event batch with length and CRC32C per record;
3. append to the active segment;
4. call `fdatasync`/`F_FULLFSYNC` according to durability profile;
5. update in-memory projection;
6. acknowledge and publish.

Default durability acknowledges only after data reaches the OS durability
primitive. A documented `balanced` profile MAY group commits for up to 10 ms;
the UI must expose the active profile. Partial trailing records are truncated to
the last valid checksum on recovery. Mid-stream corruption quarantines the
Mission and never skips unknown bytes.

### R-STORE-002 — Optimistic concurrency

One Mission sequence is gapless. A stale expected version returns conflict with
current version. The idempotency index is persisted in the same atomic batch as
the resulting events.

### R-STORE-003 — Snapshots

Mission snapshots accelerate load but are never authoritative. Each records last
event sequence, schema version, and digest. Invalid snapshots are discarded and
events replayed. A snapshot is written to a temporary sibling, synced, renamed,
then its directory synced.

## 8. Session recording and recovery — R-SESSION-001

Raw PTY output is recorded in 4 MiB append-only segments with starting byte
offset, timestamps, length-delimited chunks, and checksums. Input is not recorded
as a separate keystroke log; echoed input may naturally appear in output.

A backend-neutral terminal checkpoint is written every five seconds or 4 MiB of
output, whichever occurs first, and at clean exit. A checkpoint records output
offset, frame sequence, Ghostty pin identifier, schema version, and digest.

On GUI reconnect, canonical in-memory state supplies the frame. On daemon startup,
v1 marks formerly running Sessions Lost; it MAY restore their final view from the
latest checkpoint plus raw output replay, but it MUST NOT imply that the process
survived. Exited Session recovery replays from the latest valid checkpoint to the
end and compares final digest in tests.

Disk writing uses a bounded queue. When storage cannot keep up:

1. terminal parsing and device replies continue within an emergency memory cap;
2. the Session raises a high-priority storage Signal;
3. new Sessions are refused before existing Session output is dropped;
4. if the cap is reached, the affected process is paused with SIGSTOP when safe
   and the user is told exactly why; silent raw-output loss is forbidden.

## 9. Failure behavior — R-FAILURE-001

| Failure | Required result |
|---|---|
| desktop exit/crash | runtime, PTYs, and Sessions continue |
| desktop network/socket loss | last frame remains read-only; reconnect/resync |
| slow desktop | only its deltas drop; canonical state continues |
| one Session parser failure | Session becomes Lost/failed; others continue |
| child exit | output drains; Session Exited; Run remains separately decided |
| runtime graceful restart | active Sessions require explicit handling |
| runtime crash/power loss | active Sessions become Lost; event log recovers |
| corrupt Mission tail | truncate only incomplete tail; report recovery |
| corrupt middle record | quarantine Mission; preserve files and diagnostics |
| disk full | reject new writes; pause before output loss; offer cleanup |
| incompatible state | preserve and fail with upgrade/downgrade guidance |

Runtime task supervision MUST distinguish cancellation, expected child failure,
resource exhaustion, storage failure, and programmer panic. A panic in one
connection or Session task is captured with correlation ID and does not abort the
runtime unless global invariants can no longer be trusted.

## 10. Retention — R-RETENTION-001

Defaults:

- active Mission events: retained indefinitely;
- completed Mission metadata/events: retained indefinitely until user archive or
  delete;
- raw Session output and terminal checkpoints: 30 days after Mission completion;
- maximum retained raw output: 2 GiB per Session and 20 GiB globally;
- structured logs: seven days, 200 MiB globally;
- idempotency results: 24 hours after final retry window.

Limits are configurable. Eviction is oldest-completed-first, never active
Sessions, and records a retention event plus visible truncation marker. Before
automatic deletion, Artifacts referenced by accepted Runs are preserved unless
their own policy expires. Users can export or pin a Mission.

## 11. Resource limits — R-LIMIT-001

Defaults are conservative and configurable:

- 12 running Runs globally, eight per Mission;
- 64 running Sessions globally;
- 128 subscriptions per client;
- 100,000 scrollback lines per Session;
- 8 MiB paste, 1 MiB OSC clipboard, 1,024-byte title;
- 16 MiB connection send queue and 8 MiB Session-stream queue;
- 16 MiB compressed / 64 MiB uncompressed protocol payload;
- 256 MiB Kitty image memory per Session when enabled;
- 1,000 × 500 maximum terminal grid.

Limit errors identify the limit and remediation. They never panic or wrap integer
arithmetic.

## 12. Privacy and telemetry — S-PRIVACY-001

V1 sends no terminal content, command text, Mission intent, repository path,
Artifact, environment, or agent prompt off-device. Product analytics are off by
default and, if later enabled, contain only documented aggregate counters with no
stable project identifier.

Crash reports are opt-in and locally previewable. Automatic redaction removes
home-directory prefixes, tokens, environment values, terminal rows, clipboard
data, argv, and Mission text. Update checks MAY contact the release endpoint but
contain only app version, platform, architecture, and anonymous request metadata.

### S-PLUGIN-001 — Executable plugin trust and containment

Installing an executable plugin is an explicit owner action and grants that code
the authority of the local OS user; plugin capabilities constrain the superplexr
data channel but are not an OS sandbox claim. Plugin roots, manifests, and
executables MUST be real owner-controlled paths without symlinks, traversal, or
group/world write permission. The host clears inherited environment variables,
uses an exact validated executable path, bounds every manifest/message/queue,
and never sends secrets or terminal content through the plugin protocol.

Plugins cannot use a Share or agent identity through the host. Any plugin that
independently connects to the owner control socket remains ordinary owner code
and is subject to the same OS-user trust model; future package signing or WASM
containment does not weaken normal protocol authorization.

## 13. Observability — R-OBSERVE-001

Structured runtime logs contain timestamp, level, module, event name, correlation
ID, and hashed object ID. Content fields are deny-by-default. Terminal bytes and
protocol payload bodies are never logged.

Required metrics are local and bounded:

- PTY bytes read/written and parse latency;
- frames/deltas/full resyncs and queue high-water marks;
- input/resize acknowledgement latency;
- active Missions/Runs/Sessions/subscriptions;
- event commit and recovery latency;
- dropped client deltas, never dropped PTY bytes;
- memory, CPU, journal size, and checkpoint duration.

`Export diagnostics` creates a previewable archive containing versions, pins,
platform, redacted logs, configuration schema (not secrets), performance counters,
and integrity results. It excludes content unless the user selects exact Sessions
and sees the added files.

## 14. Updates and supply chain — S-SUPPLY-001

Release artifacts are signed. The updater verifies signature, platform,
architecture, version monotonicity, and digest before replacement. Automatic
installation never occurs while active Sessions depend on runtime replacement;
the UI offers `Install after sessions finish`.

CI uses locked Cargo dependencies, pinned Git revisions, checksums for vendored
Ghostty/Zig inputs, an SBOM, license policy, vulnerability audit, and provenance
attestation. Build scripts MUST support network-free release builds.
