# Agentic Execution

This context describes durable collaboration between humans and autonomous
agents. Terminal processes are observable machinery, not the organizing model.

## Language

**Mission**:
A durable desired outcome that coordinates one or more runs and their artifacts.
_Avoid_: Workspace, project, tab

**Run**:
A bounded attempt by an actor to advance a mission, optionally descended from
another run and using a session for interactive execution.
_Avoid_: Session, pane, tab, terminal, job

**Lineage**:
The parent-child relationship that records which run created or delegated
another run. Lineage explains causality but does not determine when work starts.
_Avoid_: Dependency, scheduling edge

**Dependency**:
A directed prerequisite relationship between runs. A dependent run becomes
ready only after all of its dependencies succeed.
_Avoid_: Parent, child, lineage

**Session**:
A durable process and terminal environment within a mission. A session may host
ordinary shell work or one or more sequential runs.
_Avoid_: Run, surface, pane

**Session group**:
A durable, owner-arranged collection of one or more Sessions presented as one
named row in a Mission sidebar. Group state never changes process identity,
Run lineage, or Mission history.
_Avoid_: Session, workspace, terminal group, folder

**Actor**:
The human or agent responsible for a run or an intervention.
_Avoid_: User, worker, bot

**Signal**:
A structured report from a run, such as progress, a question, an approval
request, a blocker, or a produced artifact.
_Avoid_: Message, notification, terminal output

**Attention item**:
An unresolved signal that requires human judgment or information.
_Avoid_: Alert, unread message

**Intervention**:
A human decision, response, or direct-control period that changes a run's path.
_Avoid_: Prompt, chat reply

**Grant**:
An explicit, scoped, and auditable authorization for a run to perform a protected
action. A grant may be single-use or time-bounded and is meaningful only when
the runtime can enforce it.
_Avoid_: Permission, approval request, write lease

**Control lease**:
The exclusive, transferable authority to send input to and resize a session.
Only one actor or surface holds a session's control lease at a time.
_Avoid_: Grant, focus, ownership

**Change intent**:
A versioned declaration of the repository revision and resource changes a Run
proposes before receiving write authority.
_Avoid_: Run plan, diff, worktree, file lock

**Execution lease**:
The exclusive, fenced authority for one Run attempt to mutate the resources
admitted from its Change intent. It does not authorize terminal input.
_Avoid_: Control lease, Grant, checkout ownership

**Agent channel**:
A least-privilege runtime connection bound to one live run and its process
identity. It is not the general control socket and is not an OS sandbox.
_Avoid_: Control socket, bearer token, agent terminal

**Driver snapshot**:
An immutable, redacted Run record of the resolved execution profile: driver ID,
profile version, process-spec digest, argument count, and environment key names.
It proves what launch configuration was selected without exposing arguments or
environment values through Mission history.
_Avoid_: Terminal launch spec, command log, environment dump

**Harness snapshot**:
An immutable Run launch fact identifying the task, Driver, tools, skills,
sandbox, delivered context, and evaluator contract that governed an Attempt.
_Avoid_: Driver snapshot, transcript, provider configuration

**Provider fact**:
A bounded, time-limited observation emitted by a named agent-provider adapter
about one run, with server-authored provenance and observation time. It may
describe working, idle, or provider-side waiting, but it cannot mutate Run
lifecycle, resolve attention, or claim completion.
_Avoid_: Signal, Run status, terminal heuristic, heartbeat

**Activity projection**:
The explainable current activity shown for one run. Authoritative Run lifecycle
and unresolved Signals take precedence over a fresh provider fact; absent or
expired evidence produces Unknown rather than guessed Idle.
_Avoid_: Run status, Signal, provider fact, terminal output

**Run checkout**:
A durable, isolated Git working tree bound to one run and an exact repository
base revision. It may outlive every session for that run and is retired only
when doing so cannot discard uncommitted or unmerged work.
_Avoid_: Workspace, session working directory, sandbox

**Run evidence**:
A bounded, durable observation from a named external system about one run, such
as a CI check result or change-request review state. Evidence records provenance
and a revision but does not itself change Run outcome, Disposition, or Signals.
_Avoid_: Provider fact, Artifact, Signal, Run status

**Evidence key**:
A provider-scoped stable identity used to replace the latest observation for one
check or change request without growing an unbounded event feed.
_Avoid_: Artifact ID, Run ID, URL

**Scheduler policy**:
An owner-controlled, durable, opt-in rule that continuously reconciles ready
agent runs against a Mission's concurrency cap and configured engine drivers.
Disabling it stops new launches without terminating work already running.
_Avoid_: Scheduler plan, cron job, Run

**Scheduler settings**:
Owner-controlled runtime-wide admission limits shared by explicit, batch, and
continuous agent launches. They are durable host policy, not Mission history.
_Avoid_: Scheduler policy, Mission concurrency, UI preference

**Sandbox attestation**:
The immutable backend/profile fact produced only after a configured process spec
has been wrapped by an available OS enforcement adapter. It states network
isolation separately and never upgrades a cooperative Run by declaration alone.
_Avoid_: Grant, permission label, restricted agent channel

**Ready since**:
The trusted durable instant at which all of a pending Run's dependencies had
succeeded. For a dependency-free Run it is its planning time. Scheduling age
and starvation prevention use this instant, never UI observation time.
_Avoid_: Created at, elapsed process time, waiting status

**Artifact**:
A durable output of a run that can be inspected, accepted, or used by another
run.
_Avoid_: Attachment, output file

**Candidate**:
An immutable, retained Git snapshot revision plus its patch digest and Realized
change manifest, admitted from a Run checkout for verification and owner
judgment. It is not an alias for the checkout's later mutable state.
_Avoid_: Outcome, branch, latest checkout

**Realized change manifest**:
A server-authored, bounded record of the exact create, modify, and delete
effects frozen from a Run checkout when its Candidate was published. It binds
the base, snapshot commit/tree/ref, patch digest, and each retained Git entry. A
rename is represented as delete-old plus create-new.
_Avoid_: Change intent, Git status output, claimed diff

**Candidate patch Artifact**:
A server-authored Artifact committed atomically with a Candidate. Its locator
names the exact base-to-snapshot Git diff and its digest binds that binary-safe
patch stream.
_Avoid_: Current working-tree diff, terminal transcript, agent-authored digest

**Delivery Run input**:
The immutable Candidate and HarnessSnapshot frozen onto a verifier or retry Run.
A retry also carries the exact owner-authored Return note. Launch must use the
managed checkout prepared at that Candidate revision.
_Avoid_: Prompt prose, latest checkout, copied objective

**Handoff Artifact**:
A bounded Artifact that tells a successor what was completed, what remains,
which evidence supports it, and which external effects must be reconciled.
_Avoid_: Transcript, summary message, repository state

**Evaluation Receipt**:
An immutable Artifact recording how one exact Candidate was independently
checked, including delivery correctness, without accepting it for the Mission.
_Avoid_: CI status, Outcome, Disposition, approval

**Settlement**:
The authoritative owner judgment that accepts or rejects one exact Candidate
after its required verification. Settlement changes Disposition, not Outcome.
_Avoid_: Process exit, merge, test result

**Disposition**:
The mission-level judgment of whether a finished run's result is awaiting review,
accepted, or rejected.
_Avoid_: Exit status, process result

**Outcome**:
The execution result of a finished run: succeeded, failed, or cancelled. Outcome
does not express whether a human accepts the result.
_Avoid_: Disposition, exit code, status

**Surface**:
A disposable interactive view of a session's terminal state. Closing a surface
does not end its session or any run using it.
_Avoid_: Pane, session

**Remote attachment**:
An owner client reaching the one authoritative runtime through an authenticated,
encrypted transport. It preserves normal client identity and Control leases; it
does not create or mirror a Mission, Run, Session, or terminal process.
_Avoid_: Remote Session, federation, Share, synchronization

**Share**:
A revocable, time-bounded invitation for another person or device identity with
narrower, explicit authority than the runtime owner. Observer can only read
explicitly scoped Missions/Sessions. Controller adds non-forced Control over
explicitly scoped terminal Sessions, but cannot mutate the graph, manage process
lifecycle, schedule work, inspect the runtime, or administer Shares. Revocation
ends live streams and releases Controller-held leases.
_Avoid_: Remote attachment, Unix user, SSH login, Control lease

**Plugin**:
An owner-installed, runtime-supervised executable that observes explicitly
granted semantic events and may publish bounded contributions. A plugin is not
part of terminal rendering, authoritative Mission state, or the desktop process.
_Avoid_: Theme, agent, shell hook, in-process extension

**Plugin capability**:
A host-enforced declaration of which semantic event classes or contribution
types one plugin may use. It conveys no authority to mutate a Mission, control a
Session, or bypass normal client authorization.
_Avoid_: Grant, Share role, agent channel
