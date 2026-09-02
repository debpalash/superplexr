# M2 runtime and desktop evidence

Status date: 2026-09-01. This is an implementation-evidence ledger, not a release
announcement. A row is `proven` only when its command was executed on the named
host; CI-required and long-duration gates remain explicit.

## Implemented product slice

- The per-user daemon owns real PTYs independently of desktop windows.
- Canonical terminal state is backed by pinned `libghostty-vt`; the UI consumes
  backend-neutral full frames and row deltas.
- Sessions support input, rich keys, IME, focus, mouse, scroll, selection, copy,
  search, resize, interrupt, graded termination, retained raw journals, and
  final-frame reconstruction after daemon restart.
- Closed Sessions remain searchable by replaying at most the latest 64 MiB of
  raw output into an isolated Ghostty model on a blocking worker, so large
  history queries do not stall control/subscription tasks.
- Bounded, sanitized OSC title and working-directory metadata survives full
  frames/deltas and can replace generated shell labels in the compact sidebar.
- A controller is a specific client plus Surface plus monotonically increasing
  epoch. Sibling and stale Surfaces cannot mutate a Session. Forced takeover is
  explicit and disconnect releases ownership without replaying input.
- The desktop dynamically discovers Sessions, reattaches after reconnect, and
  persists browser-like workspaces with a compact Session sidebar and gapless,
  responsive terminal waterfall.
- The Mission model implements Runs, immutable lineage, dependency gating,
  priority, review, typed attention Signals, cooperative Grants, Artifacts,
  Session assignment, and causal event history.
- The graph inspector exposes Run relationships and recent operational history;
  the command deck and platform-specific keyboard routes make the workspace
  operable without pointer-only navigation.
- Packages carry compiled `xterm-ghostty` terminfo and the runtime discovers both
  macOS Resources and Linux shared-data layouts.
- `termi9ne status` returns redacted operational diagnostics suitable for health
  checks and support bundles.
- `termi9ne schedule` exposes a deterministic, capacity-aware scheduler preview
  that separates startable, queued-ready, waiting, blocked, and manual work.
- `termi9ne schedule-launch` applies that plan to an explicit command driver,
  launches only available slots, and reports each selected Run independently.
- `termi9ne schedule-engine-launch` freezes the capacity plan in the daemon,
  resolves each selected Run's named engine, records a redacted immutable driver
  snapshot, and reports launch or resolution failure independently.
- `termi9ne schedule-auto` persists an owner-only per-Mission policy; the daemon
  continuously reconciles configured agent Runs under its cap and restores the
  policy after restart. Disable prevents new launches without killing active work.
- `termi9ne schedule-settings` atomically persists the runtime-wide agent cap;
  direct, batch, and continuous launches share the same serialized admission gate.
- `termi9ne run-engine` resolves an agent Run through bounded, owner-only,
  structured driver configuration without shell-string interpolation.
- Owner-only Run checkouts bind an exact Git repository/base revision and
  deterministic isolated branch/worktree to one pending Run. Configured preview
  and launch select the server-owned path; lifecycle retry is idempotent, and
  retirement refuses live, dirty, or unmerged work without a force path.
- Structured terminal capture returns identity and the exact canonical frame;
  bounded text, quiet, and exit waits subscribe to canonical Session events.
  `termi9ne events` emits race-free Mission and terminal-index snapshots plus
  push updates as stable NDJSON rather than polling terminal pixels.
- Expiring provider facts record bounded adapter state with server-authored
  owner-hook versus authenticated-agent provenance. Run Activity is derived
  from lifecycle, unresolved Signals, fresh provider evidence, then `Unknown`;
  reports cannot complete work or resolve attention.
- Agent-launched PTYs receive a separate owner-only socket authenticated by
  kernel peer PID and the Run's isolated process group; requests use a narrow
  Run-scoped allowlist and are revalidated against lifecycle state.
- `termi9ne terminal-ssh` creates a daemon-owned OpenSSH PTY with validated
  destination/port/jump/identity arguments and no shell-command interpolation.
- `termi9ne remote-forward` supervises an OpenSSH StreamLocal tunnel from a new
  owner-only client socket to one authoritative runtime. The desktop's
  connect-only mode attaches the complete native workspace without a local
  fallback, and its titlebar identifies LOCAL versus REMOTE ownership.
- Observer Shares mint a one-time 256-bit capability for explicit Mission/Session
  scope, persist only its digest, filter direct reads and live streams, deny all
  mutations/control, and revoke already-open subscriptions immediately.
- Controller Shares add non-forced Control and input for explicit terminal scope
  while denying forced takeover, process lifecycle, graph/scheduler mutation,
  diagnostics, and Share administration; revocation releases held leases.
- Exited terminals can be recoverably archived out of active indexes and later
  restored without losing history or Mission/Run identity.
- Configured drivers can fail closed into an attested `workspace_write` sandbox:
  macOS enforcement is bypass-tested for workspace/state/outside writes, while
  Linux Bubblewrap construction and package dependency are implemented pending
  execution in the Linux matrix. Network remains explicitly inherited. The Run
  inspector renders one compact execution-proof rail: enforced filesystem scope
  versus cooperative process, backend/profile, network posture, and spec hash.

## Superlogical public-plan comparison

The comparison target is [Superlogical's public product plan](https://www.superlogical.com/),
not an assumed private implementation.

| Public-plan capability | termi9ne repository status |
|---|---|
| long-lived local terminal Sessions | implemented and real-PTY-soaked |
| preserved operational history | implemented for Mission causality and terminal journals/search |
| coding-agent/background work | domain model, dependency/priority/aging scheduler, bounded batch launch, durable opt-in continuous configured-engine scheduling, configurable global admission/fairness, atomic launch, exact-base Run checkouts, structured capture/waits/push events, truthful expiring provider facts, graph-identity injection, restricted agent channel, automatic exit completion, and opt-in filesystem-write sandboxing implemented; SCM/CI fact ingestion, network-denying profiles, and Linux execution proof are pending |
| production-safe intervention | explicit Surface epochs, forced takeover, SIGINT, and graded termination implemented |
| structured composition beyond terminal bytes | typed Runs, Signals, Grants, Artifacts, dependencies, and review implemented |
| remote shells and owner workspace attachment | durable OpenSSH PTY Sessions plus reconnectable native desktop/CLI attachment to one authoritative remote runtime implemented; fleet provisioning remains pending |
| web/mobile access | pending |
| multiplayer shared terminals | revocable native Observer and scoped non-forced Controller roles implemented and live-proven; concurrent editing, editor/graph roles, and public Internet gateway pending |

The differentiated product is already concrete: it multiplexes work and agency
rather than only streams, and an owner can project that same workspace from
another host without duplicating domain state. Scoped read-only viewing and
terminal control are implemented; fleet provisioning, concurrent editing,
editor/graph roles, web, and mobile are not claimed as complete.

## Executed macOS evidence

Apple Silicon host, Rust 1.97.1, Zig 0.16.0:

```text
cargo fmt --all -- --check                                      PASS
cargo test --workspace --all-targets --locked                   PASS, 177 tests
cargo clippy --workspace --all-targets --locked
  -- -D warnings                                                PASS
./ci/audit-normal-licenses.sh .                                 PASS
cargo build --workspace --release --locked --offline            PASS
./ci/package-smoke.sh .                                         PASS
./ci/desktop-render-benchmark.sh                                PASS, 60.00 FPS,
                                                               58.58 raw FPS,
                                                               5.042 ms p95,
                                                               6 visible PTYs
./ci/desktop-idle-benchmark.sh                                  PASS, 0.54% desktop,
                                                               0.0% runtime CPU,
                                                               124.6 MiB RSS,
                                                               12 visible PTYs
./ci/cli-event-multiplex-smoke.sh .                             PASS, 4 logical
                                                               feeds, 1 socket
TERMI9NE_SOAK_SECONDS=3 TERMI9NE_SOAK_SESSIONS=12
  ./ci/runtime-soak.sh .                                        PASS, 12 iterations
TERMI9NE_SOAK_SECONDS=5 TERMI9NE_SOAK_SESSIONS=12
  ./ci/runtime-soak.sh .                                        PASS, 22 iterations
TERMI9NE_SOAK_SECONDS=60 TERMI9NE_SOAK_SESSIONS=12
  ./ci/runtime-soak.sh .                                        PASS, 317 iterations
SPDX 2.3 JSON + collected license/NOTICE texts + SHA256SUMS      PASS
```

The soak created twelve `/bin/cat` PTYs, repeatedly transferred control, sent
line-delimited input, resized, snapshotted, searched canonical scrollback,
checked index cardinality, graded-terminated every process, and asserted zero
running Sessions and zero retained control leases afterward.

Retained terminal journals are also replayable through a bounded blocking worker.
Regression coverage pages a deterministic historical viewport immediately after
process exit and again after daemon recovery; search follows the same recovered
path. Offset zero addresses the bottom viewport, and offsets are capped at the
100,000-row retention contract.

Generic agent execution now crosses a typed launch boundary rather than relying
on terminal prose. A four-command choreography (`ResolveRunDriver`,
`StartReadyRun`, `StartSession`, `AssignSession`) is decided completely before
its causally-linked event batch is written. A real-PTY regression proves that an immediately exiting child is
observed, its output is retained, and its Run plus Session finish from the child
exit fact. Resource-identity retries return the existing binding; launch errors
trigger a durable failed-completion compensation.

Live macOS evidence used Mission `df380a9d-6172-4ab2-928f-ca6f1337c41b`, Run
`7d13320d-2d02-4260-ad36-ce112a9bf862`, and Session
`9633cfbc-79d6-4bb6-9790-309094570f58`. The launch returned a real
PTY, `/bin/sh` exited zero, the Mission reached version 7 with a `Succeeded` Run
awaiting review and an exited Session, retained search found
`agent-launch-live-pass`, and runtime diagnostics reported zero running or
controlled terminals afterward.

The desktop projection uses the persisted Mission/Run binding as ownership, even
when terminal-index delivery wins the race with Mission delivery. A live restart
migrated the evidence Session out of the previously active workspace and into
its Mission workspace with the correct `live-agent` actor. Closing that desktop
then reduced diagnostics from one Mission and one terminal-index subscriber to
zero immediately; idle subscription handlers now watch peer EOF instead of
remaining retained until the next domain event.

The same evidence Session was then archived through the live daemon. It
disappeared from the default terminal index, remained in the inclusive index
with its Mission and Run IDs, retained searchable history, and was counted by
redacted diagnostics. After both desktop and daemon restart the archive marker
was recovered with owner-only permissions. Restore removed only that marker,
returned the archived count to zero, and reattached the terminal to the original
Mission workspace as `live agent evidence` with actor `live-agent`.

Live scheduler evidence used Mission `83f3bdcf-c6e1-4188-bd4f-b65aad08db80`.
Two independent agent Runs were ready under a one-slot cap. The first
`schedule-launch` selected one and reported one queued; after process exit, the
second selected the remaining Run. Sessions
`990c3c11-9ca7-4857-be95-049c8e05cb40` and
`6542cacf-e879-4bfe-bb33-14ef904f66da` each retained output containing their
daemon-injected Mission, Run, and Session identities. Both Runs finished
Succeeded and diagnostics reported zero running or controlled terminals.

Configured-driver evidence ran in an isolated daemon using Mission
`11bdae13-1a0f-4c0c-8733-32ed3a93e3a1`, Run
`09de0c5e-2be3-4ddf-9325-7404bfb2ae7e`, and Session
`cec7e9ee-804e-44c7-81ce-5b6e9b068fcb`. The owner-only v1 config mapped engine
`fixture-echo` to structured `/bin/echo` arguments and appended the objective as
one argument. The process exited zero, completed both domain objects, and its
output remained searchable. Symlink/owner/type/size/schema/argv/environment and
reserved-identity rejection are covered at the driver seam. A redacted preview
regression resolves the same launch, exposes only environment key names, and
proves the Mission version is unchanged.

Daemon-side configured scheduling evidence used Mission
`0e66b7bb-98a7-44ff-89e6-daec1aa1d6c1`. Under a one-slot cap, urgent Run
`dc34f7e4-09f1-457b-ba50-766d44317c49` launched before normal Run
`a3f19e4a-f39c-454f-87ea-b871e8cb6958`; each response committed four events and
reported one queued or zero queued Runs as appropriate. Sessions
`e1479da0-858e-53be-a0d4-fb827734f2ed` and
`21a73cdc-dc8a-591f-90c2-bef0d2f3bd13` exited zero and retained searchable
`configured-driver` output. Both Runs persisted profile v1 snapshots with
64-character SHA-256 digests, argument count two, and only key name
`T9_DRIVER_FIXTURE`. Run `0e6dd81b-fbff-4c68-a638-4f1216a1e7b0` then selected
engine `unavailable-engine`; the batch reported driver resolution failure,
launched no PTY, and left the Run pending without a snapshot. Regression tests
also hold a first Run open, repeat reconciliation under an occupied slot, and
prove no duplicate Session is created.

Continuous-policy live evidence used Mission
`9816a13b-bfaa-4657-ad16-a45e32a32a0e`. Enabling a one-slot policy launched
urgent Run `81b06dbd-ea57-4e24-b4bd-0e1dcc6a4af2` followed by normal Run
`1f6d46fe-a716-4c8b-b7ac-715e836ec482` without another control request; both
resolved `fixture-echo`, exited zero, and retained their Sessions. Disabling the
policy left Run `5d147a9b-94f4-4f25-b989-11808cb5ab99` pending with no snapshot
or Session across more than one reconciliation interval. After daemon restart,
protocol-v5 policy listing still reported the policy disabled and diagnostics
reported one durable policy, zero enabled. Re-enabling launched that Run into
Session `9f256571-6301-5e91-b7f0-366dc173fc60`, which exited zero. The policy
file was owner-only and the isolated proof state was moved to Trash.

That restart also exposed and locked down a recovery defect: finished agent
bindings were redundantly submitted for finalization and produced `already
finished` startup errors. A red-capable restart regression first reproduced two
attempts. Recovery now derives only missing finish commands from the replayed
Mission, skips fully finished pairs, and still fails genuinely interrupted
Run/Session bindings. The original three-Session restart then emitted no
redundant finalization errors.

Global admission is exercised at a test cap of one with real sleeping PTYs.
Four Runs across two enabled Mission policies never exceed one running agent,
and interval rotation gives each Mission one launch before either receives its
second. A separate simultaneous two-request race admits exactly one launch,
returns an explicit capacity error for the other without graph mutation, then
admits the pending Run after the first exits. Production diagnostics expose the
owner-configured limit and current redacted occupancy. Store and request-path
regressions set the cap to one, prove that it survives reopen, reject zero
without replacing the durable value, and exercise continuous scheduling through
the production setting reader.

Scheduler aging uses event-store-authored planning and completion timestamps.
Boundary tests prove promotion at exactly five minutes, two-level Background
promotion at ten minutes, deterministic ready-time/Run-ID ties, and no early
promotion one microsecond before the boundary. A dependent Background Run planned
long ago but unblocked only five minutes ago receives one promotion—not two.
Store/reopen coverage proves both timestamps survive journal replay, while legacy
events without trusted time remain valid and unaged.

Live protocol-v6 timestamp evidence (retained before the settings schema bump)
used Mission
`5e03eac9-a899-485a-92c1-fb4dc35b43a2` and Background Run
`47bed7cf-8d18-46a3-a426-75d9f76b24ee`. The Run and scheduler preview both
reported store-authored `planned_at`/`ready_since` value `1788222658259320`, with
base and effective Background priority. After daemon restart the same plan
returned the identical timestamp and ordering. The isolated state was moved to
Trash.

Live protocol-v7 scheduler-setting evidence used an isolated release daemon.
The owner set `global_max_concurrency` to three; both the settings response and
redacted diagnostics reported three. After a clean daemon stop and restart with
a different PID, the same value reopened from the owner-only policy file and
remained the admission limit. The isolated proof state was moved to Trash. The
workspace desktop was then relaunched on protocol v7 with four Missions, twelve
exited terminals, the default limit of twelve, zero occupancy, and both local
sockets owner-only.

Live protocol-v8 sandbox evidence used Mission
`17a94ed1-4d1c-449a-8628-8c69390f5012`, Run
`7fc5ef44-4f79-4f4f-bf8f-fd4ada937623`, and Session
`1c5c703f-551d-40d4-857c-ae63a582187d`. Preview exposed
`macos_sandbox_exec` / `workspace_write` with network isolation false. The real
PTY child created the allowed workspace file, while macOS denied both the
runtime-state write and a sibling-path write. Exit code one durably failed the
Run and Session, and the immutable snapshot retained the same attestation plus
its wrapped-spec SHA-256. The isolated proof state was moved to Trash.

Restricted-channel live evidence used Mission
`c1368a75-b560-4de5-bedf-f9c63c943d8e`, Run
`6ae9fc18-715c-496d-bf57-241c85e6364e`, and Session
`5a1e19ad-8140-4427-a5bd-7c205352e5de`. While the Run was active, an unrelated
same-user process group was denied at connection authentication. The bound agent
was denied global Mission listing, then successfully read its own Mission and
recorded Artifact `channel-proof` through the same socket. It exited zero, both
domain objects completed, the output remained searchable, and the owner-only
agent socket was removed when the isolated daemon stopped.

Current macOS smoke archive SHA-256:
`d35437e53326f4a4c22143191c1a68f7762011f050c76801eaa30029418071a0`.

The macOS archive is ad-hoc signed for structural verification. It is not
Developer ID signed or notarized.

### Owner Remote attachment proof

An isolated macOS runtime and isolated OpenSSH server were started with
disposable Ed25519 host/client keys and a dedicated known-hosts database. The
tunnel created its client socket as `srw-------`; status through that socket
reported the authoritative daemon identity and protocol v8. A real PTY launched
through the forwarded connection printed `remote-attachment-pass`, exited, and
remained searchable.

The SSH listener and established forwarding processes were then stopped. The
supervisor observed channel loss, removed its exact local socket, and retried at
1, 2, 4, then 8 seconds. After the isolated SSH server returned, the owner-only
socket was recreated and status reported the same daemon process; the retained
terminal output was still searchable. A native GPUI desktop then attached with
`--connect-only`, established Mission and terminal-index subscriptions, and did
not spawn a second runtime. This proves reconnectable same-owner projection. It
does not prove a Share or multi-user authorization.

### Observer Share proof

An isolated protocol-v10 daemon held two Missions. The owner minted a one-hour
Observer Share scoped to exactly one Mission. A guest CLI using an owner-only
token file listed only that Mission; direct access to the hidden Mission and a
Mission-creation attempt both returned `share_request_denied`. The plaintext
secret was absent from `shares.json`, while owner listing returned only metadata.
After revocation, the same capability returned the deliberately indistinguishable
invalid/expired/revoked authorization error.

A second Share attached the native GPUI desktop in connect-only mode with no
terminal Sessions present, proving the read-only empty state does not spawn a
shell or a second daemon. Runtime diagnostics observed one Mission subscriber and
one terminal-index subscriber. Revocation dropped both counts to zero within one
second while the authoritative daemon and owner connections remained live.

### Controller Share proof

An isolated protocol-v10 daemon owned a real 60×8 `/bin/cat` PTY. The owner
minted a one-hour Controller Share scoped to exactly that Session and stored the
one-time capability in a mode-0600 file. The guest acquired non-forced Control,
sent `controller-parity-proof` through the terminal paste path, and Ghostty
history search returned both the line-discipline echo and process output.

The same guest's forced-Control request and process-kill request both returned
`share_request_denied`. Owner revocation returned Controller metadata without a
secret; the next guest search returned the indistinguishable invalid, expired,
or revoked authorization error. The server test additionally holds a Controller
lease across revocation and verifies that client, Surface, and Share controller
IDs are cleared and the Control epoch increments from four to five.
Digest-store coverage also advances an elapsed Controller capability into a
durable revocation record; the runtime's one-second expiry reaper sends that
identity through the same stream-closure and lease-release path without waiting
for a guest request.

Native GPUI coverage distinguishes shared authority from Observer input state:
both Share roles hide and reject owner-only workspace, terminal-process,
archive/restore, and termination actions, while Controller still reaches the
normal non-forced terminal Control path. The shared-projection regression proves
those lifecycle calls cannot change tab, Session, Surface, or confirmation state.

The workspace is running the final protocol-v10 release daemon with one native
desktop projection, six retained Missions, fourteen exited terminals, zero
failed/running/controlled/archived terminals, a global scheduler limit of twelve
with zero occupancy, and owner-only control and agent sockets. Two consecutive
launches against an isolated exact state copy preserved Mission and terminal
cardinality, confirming restart does not bootstrap duplicate workspaces.

## Linux evidence boundary

`ci/linux.Dockerfile` is pinned for amd64/arm64 and defines the following gate:

- format, license, workspace tests, and strict Clippy;
- network-disabled offline release build;
- pinned linuxdeploy dependency bundling followed by a pinned-runtime AppImage,
  plus AppDir archive and `.deb` with terminfo/linkage verification;
- twelve-Session real-PTY soak;
- X11/Xvfb and Wayland/Weston GPUI launch smokes for both the release binary and
  the self-extracting AppImage.

The workstation's existing Colima daemon was resumed on 2026-09-01. Its first
unqualified build selected ARM64 while the reusable cache was x86_64; committing
the new dependency layer exhausted the host volume's physical headroom and
BuildKit returned an I/O error before repository tests began. The VM was stopped
and only regenerable Cargo cache was cleared to restore host headroom. No Linux
gate is claimed from that attempt. The matrix remains `CI required` until an
x86_64 Docker/GitHub Actions host with adequate physical storage records the
complete command successfully.

## Remaining production gates

- production macOS signing, notarization, hardened runtime, and update channel
  are intentionally deferred; preview artifacts remain unsigned/ad-hoc only;
- AppImage portability execution on clean supported Linux distributions;
- physical macOS and Linux performance/presentation baselines;
- VoiceOver and Orca end-to-end journeys;
- protocol and terminal fuzzing beyond deterministic chunk-boundary corpora;
- nightly 12-hour and weekly 72-hour crash/churn soaks;
- signed build provenance and public release attestation;
- network-denying sandbox profiles and Linux sandbox bypass execution;
- editor/graph-mutation Share roles, a TLS browser/mobile gateway, fleet
  provisioning, and additional sandbox adapters if promoted into release scope.
