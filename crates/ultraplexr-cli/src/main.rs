mod shell_init;
mod ssh_tunnel;
use ultraplexr_verification as verification;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::os::unix::fs::PermissionsExt;

use clap::{Parser, Subcommand, ValueEnum};
use thiserror::Error;
use tokio::{io::BufReader, net::UnixStream};
use ultraplexr_client::ControlClient;
use ultraplexr_core::{
    Actor, ActorId, ArtifactId, ChangeClaim, ChangeClaimKey, ChangeIntentSpec, ChangeOperation,
    ChangeScope, Command, DomainError, EvaluationCheck, EvaluationReceipt, EvaluationVerdict,
    FaultId, FinishOutcome, GrantEnforcement, GrantId, HandoffArtifact, MissionId, PauseReason,
    Risk, RunCandidate, RunId, RunPriority, SessionId, SignalId, SignalKind, VerificationPolicy,
    VerifiedDeliveryCommand,
};
use ultraplexr_protocol::{
    ChangeRequestEvidenceState, CheckEvidenceState, ClientRequest, FaultInput, FaultKind,
    MissionEvent, PROTOCOL_VERSION, ProtocolError, ProviderActivityState, ProviderFactInput,
    Request, ResponseBody, ResponseResult, RunActivityEvent, RunEvidenceInput, RunEvidenceKind,
    SchedulerPolicy, SchedulerSettings, ServerResponse, SessionGroupChange, SessionGroupEvent,
    SessionGroupId, SessionGroupSpec, ShareRole, TerminalIndexEvent, TerminalSessionSpec,
    TerminalWaitCondition, default_socket_path,
    wire_v3::{AsyncWireReader, AsyncWireWriter, FrameKind, Hello, client_handshake},
};
use ultraplexr_terminal::{
    GridSize, HistoryViewport, SelectionPoint, TerminalError, ViewportScroll,
};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(about = "Control agentic missions in the local ultraplexr runtime")]
struct Args {
    #[arg(long, default_value_os_t = default_socket_path())]
    socket: PathBuf,
    /// Reject a Mission mutation unless its current version matches.
    #[arg(long)]
    expected_version: Option<u64>,
    /// Reuse this UUID when retrying the same Mission mutation.
    #[arg(long)]
    idempotency_key: Option<Uuid>,
    /// Explicitly take terminal control from another surface for this command.
    #[arg(long)]
    force_control: bool,
    /// Read a Share token from an owner-only regular file.
    #[arg(long)]
    share_token_file: Option<PathBuf>,
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// List one bounded page of verifier Runs in a Mission; read-only.
    VerificationList {
        mission_id: MissionId,
        #[arg(long)]
        after: Option<RunId>,
        #[arg(long, default_value_t = 32, value_parser = clap::value_parser!(u16).range(1..=64))]
        limit: u16,
    },
    /// Inspect one verifier's execution, recorded receipts and subject disposition; read-only.
    VerificationStatus {
        mission_id: MissionId,
        verifier_run_id: RunId,
    },
    /// Describe a trusted verification plan and its digest; executes no checks or runtime mutations.
    VerificationPrepare {
        #[command(flatten)]
        setup: VerificationSetupArgs,
    },
    /// Launch the reviewed plan in a fresh verifier, or resume an existing pending verifier.
    VerificationLaunch {
        mission_id: MissionId,
        /// Source Run whose frozen Candidate will be independently verified.
        #[arg(
            long,
            required_unless_present = "verifier",
            conflicts_with = "verifier"
        )]
        subject: Option<RunId>,
        /// Existing pending verifier; never creates a replacement Run.
        #[arg(long)]
        verifier: Option<RunId>,
        /// Reserve this fresh Run ID before launch for interruption recovery; does not resume an existing ID.
        #[arg(long, requires = "subject", conflicts_with = "verifier")]
        new_verifier_id: Option<RunId>,
        #[command(flatten)]
        setup: VerificationSetupArgs,
        /// SHA-256 from verification-prepare, after reviewing its commands and limits.
        #[arg(long)]
        plan_sha256: String,
    },
    /// Generate a private six-check plan for a trusted Cargo binary project; executes nothing.
    VerificationPlanRust {
        /// Owner-only JSON recipe with Cargo, package, binary, scratch and smoke expectations.
        #[arg(long)]
        recipe: PathBuf,
        /// New plan file in an existing private directory; never overwrites.
        #[arg(long)]
        output: PathBuf,
    },
    /// Internal recipe child. Use a reviewed plan with verification-execute for enforced bounds.
    #[command(hide = true)]
    VerificationRustCheck {
        #[arg(long)]
        check: String,
        #[arg(long)]
        recipe_json: String,
    },
    /// Execute an explicit trusted check plan inside a runtime-launched verifier Run.
    /// Commands run with this OS user's permissions; this is not a sandbox.
    VerificationExecute {
        #[arg(long)]
        plan: PathBuf,
        /// Existing private directory outside the checkout; results use a new Run-ID subdirectory.
        #[arg(long)]
        evidence_root: PathBuf,
        /// Refuse plan bytes that differ from the owner's reviewed SHA-256.
        #[arg(long)]
        plan_sha256: Option<String>,
    },
    /// Record a finished verifier's digested results, without rerunning checks or accepting work.
    VerificationCollect {
        mission_id: MissionId,
        verifier_run_id: RunId,
        #[arg(long)]
        evidence_root: PathBuf,
    },
    /// Stream Mission and terminal-index updates as one JSON object per line.
    Events {
        #[arg(long, value_enum, default_value_t = EventScopeArg::All)]
        scope: EventScopeArg,
    },
    /// Mint a scoped Share token. The secret is printed once.
    ShareCreate {
        label: String,
        #[arg(long, value_enum, default_value_t = ShareRoleArg::Observer)]
        role: ShareRoleArg,
        #[arg(long = "mission")]
        mission_ids: Vec<MissionId>,
        #[arg(long = "session")]
        session_ids: Vec<SessionId>,
        #[arg(long, default_value_t = 86_400)]
        expires_in_seconds: u64,
    },
    /// List Share metadata without revealing secrets.
    ShareList,
    /// Revoke a Share immediately, including live subscriptions.
    ShareRevoke {
        share_id: Uuid,
    },
    /// Keep an encrypted owner attachment to a remote ultraplexr runtime.
    RemoteForward {
        #[arg(value_parser = validate_ssh_destination)]
        destination: String,
        /// New local Unix socket exposed to local ultraplexr clients.
        #[arg(long)]
        local_socket: PathBuf,
        /// Absolute control-socket path on the authoritative remote host.
        #[arg(long)]
        remote_socket: PathBuf,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        identity: Option<PathBuf>,
        /// Dedicated known-hosts file; also requires strict host-key matching.
        #[arg(long)]
        known_hosts: Option<PathBuf>,
        #[arg(long, value_parser = validate_ssh_destination)]
        jump: Option<String>,
        /// Exit when the SSH channel closes instead of reconnecting.
        #[arg(long)]
        once: bool,
    },
    /// Start a durable PTY process owned by the runtime.
    TerminalNew {
        #[arg(long)]
        program: Option<PathBuf>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, default_value_t = 120)]
        columns: u16,
        #[arg(long, default_value_t = 36)]
        rows: u16,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Start a durable remote shell through the system OpenSSH client.
    TerminalSsh {
        #[arg(value_parser = validate_ssh_destination)]
        destination: String,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        identity: Option<PathBuf>,
        #[arg(long, value_parser = validate_ssh_destination)]
        jump: Option<String>,
        #[arg(long, default_value_t = 120)]
        columns: u16,
        #[arg(long, default_value_t = 36)]
        rows: u16,
    },
    /// List visible runtime-owned terminals; include the archive on request.
    TerminalList {
        #[arg(long)]
        all: bool,
    },
    /// Create a durable named group of terminal Sessions for a sidebar.
    SessionGroupCreate {
        name: String,
        #[arg(long)]
        mission_id: Option<MissionId>,
        #[arg(long = "session", required = true)]
        session_ids: Vec<SessionId>,
        #[arg(long, default_value_t = 0)]
        position: u32,
        #[arg(long)]
        pinned: bool,
        #[arg(long)]
        detached: bool,
    },
    /// List authoritative Session groups, optionally for one Mission.
    SessionGroupList {
        #[arg(long)]
        mission_id: Option<MissionId>,
    },
    /// Rename a Session group with optimistic concurrency.
    SessionGroupRename {
        group_id: SessionGroupId,
        expected_group_version: u64,
        name: String,
    },
    /// Add a terminal Session to an existing group.
    SessionGroupAdd {
        group_id: SessionGroupId,
        expected_group_version: u64,
        session_id: SessionId,
    },
    /// Remove a terminal Session from a group without stopping it.
    SessionGroupRemove {
        group_id: SessionGroupId,
        expected_group_version: u64,
        session_id: SessionId,
    },
    /// Change a Session group's pinned state.
    SessionGroupPin {
        group_id: SessionGroupId,
        expected_group_version: u64,
        #[arg(long)]
        pinned: bool,
    },
    /// Hide a group without stopping or archiving member Sessions.
    SessionGroupDetach {
        group_id: SessionGroupId,
        expected_group_version: u64,
    },
    /// Make a detached group visible again.
    SessionGroupReattach {
        group_id: SessionGroupId,
        expected_group_version: u64,
    },
    /// Remove the group only; member processes continue running.
    SessionGroupDelete {
        group_id: SessionGroupId,
        expected_group_version: u64,
    },
    /// Fetch the latest retained visible frame.
    TerminalSnapshot {
        session_id: SessionId,
    },
    /// Capture terminal identity, Run binding, modes, cursor, and exact frame.
    TerminalCapture {
        session_id: SessionId,
    },
    /// Fetch a viewport from durable scrollback, addressed in rows before the bottom.
    TerminalHistory {
        session_id: SessionId,
        #[arg(long, default_value_t = 0)]
        rows_before_bottom: u32,
    },
    /// Send UTF-8 bytes exactly as supplied.
    TerminalWrite {
        session_id: SessionId,
        text: String,
    },
    /// Resize the kernel PTY and canonical terminal grid.
    TerminalResize {
        session_id: SessionId,
        columns: u16,
        rows: u16,
    },
    /// Scroll the retained terminal viewport by logical rows.
    TerminalScroll {
        session_id: SessionId,
        delta: i32,
    },
    /// Select a native Ghostty viewport range.
    TerminalSelect {
        session_id: SessionId,
        anchor_column: u16,
        anchor_row: u16,
        head_column: u16,
        head_row: u16,
        #[arg(long)]
        rectangle: bool,
    },
    /// Format the active native selection as clipboard text.
    TerminalCopy {
        session_id: SessionId,
    },
    /// Search all Ghostty-owned terminal history.
    TerminalSearchPages {
        session_id: SessionId,
        query: String,
        #[arg(long)]
        case_sensitive: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Search all Ghostty-owned terminal history, collecting one JSON response.
    TerminalSearch {
        session_id: SessionId,
        query: String,
        #[arg(long)]
        case_sensitive: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Wait until text appears in a canonical visible frame.
    TerminalWaitText {
        session_id: SessionId,
        query: String,
        #[arg(long)]
        case_sensitive: bool,
        #[arg(long, default_value_t = 30_000)]
        timeout_millis: u64,
    },
    /// Wait until no canonical frame change occurs for the quiet interval.
    TerminalWaitQuiet {
        session_id: SessionId,
        #[arg(long, default_value_t = 500)]
        quiet_millis: u64,
        #[arg(long, default_value_t = 30_000)]
        timeout_millis: u64,
    },
    /// Wait until the Session exits or fails.
    TerminalWaitExit {
        session_id: SessionId,
        #[arg(long, default_value_t = 300_000)]
        timeout_millis: u64,
    },
    /// Terminate a runtime-owned process.
    TerminalKill {
        session_id: SessionId,
    },
    /// Send SIGINT to the PTY foreground process group.
    TerminalInterrupt {
        session_id: SessionId,
    },
    /// Start graded SIGHUP then SIGTERM termination.
    TerminalTerminate {
        session_id: SessionId,
    },
    /// Hide a stopped Session while preserving all history.
    TerminalArchive {
        session_id: SessionId,
    },
    /// Return an archived Session to its owning workspace.
    TerminalRestore {
        session_id: SessionId,
    },
    Create {
        intent: String,
        #[arg(long, default_value = "human")]
        actor: String,
    },
    Start {
        mission_id: MissionId,
        objective: String,
        #[arg(long, default_value = "agent-1")]
        actor: String,
        #[arg(long, default_value = "codex")]
        engine: String,
        #[arg(long)]
        parent: Option<RunId>,
    },
    /// Record a pending Run with immutable lineage, dependencies, and priority.
    Plan {
        mission_id: MissionId,
        objective: String,
        #[arg(long, default_value = "agent-1")]
        actor: String,
        #[arg(long, default_value = "codex")]
        engine: String,
        #[arg(long)]
        parent: Option<RunId>,
        #[arg(long = "depends-on")]
        dependencies: Vec<RunId>,
        #[arg(long)]
        retry_of: Option<RunId>,
        #[arg(long, value_enum, default_value_t = PriorityArg::Normal)]
        priority: PriorityArg,
    },
    /// Start a pending Run only if all dependencies succeeded.
    StartReady {
        mission_id: MissionId,
        run_id: RunId,
    },
    /// Start a ready agent Run in a new durable terminal Session.
    RunLaunch {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long, default_value = "agent")]
        session_name: String,
        #[arg(long)]
        program: PathBuf,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, default_value_t = 120)]
        columns: u16,
        #[arg(long, default_value_t = 36)]
        rows: u16,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Launch a ready Run through its configured named engine driver.
    RunEngine {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long, default_value = "agent")]
        session_name: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Launch from the ready managed checkout for this Run.
        #[arg(long)]
        checkout: bool,
        #[arg(long, default_value_t = 120)]
        columns: u16,
        #[arg(long, default_value_t = 36)]
        rows: u16,
    },
    /// Preview the exact configured engine launch without starting a Run.
    RunEnginePreview {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Preview against the ready managed checkout for this Run.
        #[arg(long)]
        checkout: bool,
        #[arg(long, default_value_t = 120)]
        columns: u16,
        #[arg(long, default_value_t = 36)]
        rows: u16,
    },
    /// Prepare a durable isolated Git worktree for one pending Run.
    RunCheckoutNew {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long)]
        repository: Option<PathBuf>,
        #[arg(long, default_value = "HEAD")]
        base_ref: String,
    },
    /// List managed Run checkouts, optionally limited to one Mission.
    RunCheckoutList {
        #[arg(long)]
        mission_id: Option<MissionId>,
    },
    /// Retire a clean checkout after proving its HEAD is merged into a ref.
    RunCheckoutRetire {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long)]
        merged_into_ref: String,
    },
    /// Declare or amend the exact repository resources a pending Run proposes to change.
    ChangeIntentDeclare {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long)]
        repository_identity: String,
        #[arg(long)]
        base_revision: String,
        #[arg(long)]
        expected_intent_version: Option<u64>,
        #[arg(long = "create")]
        create_paths: Vec<String>,
        #[arg(long = "modify")]
        modify_paths: Vec<String>,
        #[arg(long = "delete")]
        delete_paths: Vec<String>,
        #[arg(long = "contingent-create")]
        contingent_create_paths: Vec<String>,
        #[arg(long = "contingent-modify")]
        contingent_modify_paths: Vec<String>,
        #[arg(long = "contingent-delete")]
        contingent_delete_paths: Vec<String>,
    },
    /// Admit one proposed Change intent and issue a runtime-authored Execution lease.
    ChangeIntentAdmit {
        mission_id: MissionId,
        run_id: RunId,
        expected_intent_version: u64,
    },
    /// Promote declared contingent paths and rotate the Run's Execution lease.
    ChangeIntentPromote {
        mission_id: MissionId,
        run_id: RunId,
        expected_intent_version: u64,
        #[arg(long = "create")]
        create_paths: Vec<String>,
        #[arg(long = "modify")]
        modify_paths: Vec<String>,
        #[arg(long = "delete")]
        delete_paths: Vec<String>,
    },
    /// Require a separate verifier Run and passing receipt before owner acceptance.
    VerificationRequire {
        mission_id: MissionId,
        run_id: RunId,
    },
    /// Create an independent verifier with the exact Candidate and harness frozen as input.
    VerifierCreate {
        mission_id: MissionId,
        subject_run_id: RunId,
        #[arg(long, default_value = "verifier")]
        actor: String,
        #[arg(long, default_value = "codex")]
        engine: String,
        #[arg(long, value_enum, default_value_t = PriorityArg::Urgent)]
        priority: PriorityArg,
    },
    /// Create a linked retry from an owner-returned Candidate and Return note.
    RetryReturned {
        mission_id: MissionId,
        source_run_id: RunId,
        #[arg(long, default_value = "retry")]
        actor: String,
        #[arg(long, default_value = "codex")]
        engine: String,
        #[arg(long, value_enum, default_value_t = PriorityArg::Urgent)]
        priority: PriorityArg,
    },
    /// Submit the immutable Candidate that a Run wants verified and settled.
    CandidateSubmit {
        mission_id: MissionId,
        run_id: RunId,
        revision: String,
        #[arg(long)]
        content_sha256: String,
        /// Active Execution lease epoch injected into an admitted agent Run.
        #[arg(long)]
        lease_epoch: Option<u64>,
        #[arg(long = "artifact")]
        artifact_ids: Vec<ArtifactId>,
        #[arg(long, default_value = "agent-1")]
        by: String,
    },
    /// Record bounded evidence-linked state for a successor Run or reviewer.
    HandoffRecord {
        mission_id: MissionId,
        run_id: RunId,
        summary: String,
        #[arg(long = "completed")]
        completed: Vec<String>,
        #[arg(long = "remaining")]
        remaining: Vec<String>,
        #[arg(long = "evidence")]
        evidence: Vec<ArtifactId>,
        #[arg(long = "external-effect")]
        external_effects: Vec<String>,
    },
    /// Record independent verification for one exact Candidate.
    EvaluationRecord {
        mission_id: MissionId,
        subject_run_id: RunId,
        verifier_run_id: RunId,
        #[arg(long)]
        candidate_sha256: String,
        #[arg(long, value_enum)]
        verdict: EvaluationVerdictArg,
        #[arg(long = "passed-check")]
        passed_checks: Vec<String>,
        #[arg(long = "failed-check")]
        failed_checks: Vec<String>,
        #[arg(long)]
        delivery_validated: bool,
        #[arg(long)]
        repeatable: bool,
        #[arg(long)]
        summary: String,
    },
    /// Report one expiring provider-adapter fact for a Run.
    ProviderReport {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        adapter_version: String,
        #[arg(long, value_enum)]
        state: ProviderStateArg,
        #[arg(long)]
        summary: String,
        #[arg(long, default_value_t = 30)]
        valid_for_seconds: u16,
    },
    /// Explain the current derived activity for one Run.
    ProviderStatus {
        mission_id: MissionId,
        run_id: RunId,
    },
    /// List current derived activity for every Run in one Mission.
    ProviderList {
        mission_id: MissionId,
    },
    /// Print or install shell integration so failing commands become Faults.
    ///
    /// Without it ultraplexr only sees whole sessions fail; with it, each
    /// command's exit status is reported. Add `eval "$(ultraplexr shell-init)"`
    /// to your shell startup file, or pass --install to do it for you.
    ShellInit {
        /// Shell to emit integration for. Detected from $SHELL when absent.
        #[arg(long)]
        shell: Option<String>,
        /// Write the snippet into the shell's startup file instead of stdout.
        #[arg(long)]
        install: bool,
    },
    /// Record one observed failure with the evidence needed to replay it.
    ///
    /// Intended for shell hooks, CI, and agents: pipe the failing output in on
    /// stdin, or pass it with --output.
    FaultReport {
        /// The exact command that failed, replayed verbatim by `fault repro`.
        #[arg(long)]
        command: String,
        /// Directory the command ran in. Defaults to the current directory.
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = FaultKindArg::CommandFailed)]
        kind: FaultKindArg,
        #[arg(long)]
        exit_code: Option<i32>,
        #[arg(long)]
        revision: Option<String>,
        /// One line naming the failure. Defaults to the command.
        #[arg(long)]
        summary: Option<String>,
        /// Failing output. When absent, stdin is read.
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        session_id: Option<SessionId>,
        #[arg(long)]
        mission_id: Option<MissionId>,
        #[arg(long)]
        run_id: Option<RunId>,
    },
    /// List Faults, newest first.
    FaultList {
        #[arg(long)]
        mission_id: Option<MissionId>,
        #[arg(long)]
        session_id: Option<SessionId>,
        /// Include Faults that are already resolved or dismissed.
        #[arg(long)]
        all: bool,
    },
    /// Show one Fault with its evidence and latest replay.
    FaultShow {
        fault_id: FaultId,
    },
    /// Replay a Fault's command in its recorded directory and record the result.
    FaultRepro {
        fault_id: FaultId,
        #[arg(long)]
        timeout_seconds: Option<u16>,
    },
    /// Re-run the replay of resolved Faults and reopen any that fail again.
    ///
    /// Runs their recorded commands. This is what keeps a resolution true: a
    /// Fault that closed on a passing replay is worth little if it can come
    /// back unnoticed.
    FaultGuard {
        /// Most Faults to check in one pass, oldest proof first.
        #[arg(long)]
        limit: Option<u16>,
        #[arg(long)]
        timeout_seconds: Option<u16>,
    },
    /// Close a Fault as fixed. Refused unless its latest replay passed.
    FaultResolve {
        fault_id: FaultId,
        #[arg(long)]
        note: String,
    },
    /// Close a Fault without replay evidence, recording why.
    FaultDismiss {
        fault_id: FaultId,
        #[arg(long)]
        note: String,
    },
    /// Hand a Fault to an agent: plan a Run with the Fault as its objective,
    /// link the two, and launch it with the configured engine driver.
    ///
    /// The Run is asked to make the Fault's command succeed. Whether it did is
    /// decided by `fault repro`, never by the agent's own account.
    FaultFix {
        fault_id: FaultId,
        #[arg(long)]
        mission_id: MissionId,
        /// Configured engine driver to launch, for example `codex`.
        #[arg(long)]
        engine: String,
        /// Actor name recorded for the Run.
        #[arg(long, default_value = "fixer")]
        actor: String,
        /// Plan and link the Run without launching it.
        #[arg(long)]
        plan_only: bool,
    },
    /// Print one Fault as a plain-text brief ready to hand to an agent.
    ///
    /// Emits the command, directory, revision, replay state, and failing
    /// output, so the next actor does not re-read scrollback.
    FaultHandoff {
        fault_id: FaultId,
    },
    /// Record or replace one provider-neutral CI check observation.
    EvidenceCheck {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        adapter_version: String,
        #[arg(long)]
        key: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        name: String,
        #[arg(long, value_enum)]
        state: CheckEvidenceStateArg,
        #[arg(long)]
        summary: String,
        #[arg(long)]
        url: Option<String>,
    },
    /// Record or replace one provider-neutral change-request observation.
    EvidenceReview {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        adapter_version: String,
        #[arg(long)]
        key: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        title: String,
        #[arg(long, value_enum)]
        state: ChangeRequestEvidenceStateArg,
        #[arg(long)]
        summary: String,
        #[arg(long)]
        url: Option<String>,
    },
    /// List durable CI and change-review evidence attached to one Run.
    EvidenceList {
        mission_id: MissionId,
        run_id: RunId,
    },
    Pause {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long, value_enum, default_value_t = PauseReasonArg::Human)]
        reason: PauseReasonArg,
    },
    Resume {
        mission_id: MissionId,
        run_id: RunId,
    },
    Cancel {
        mission_id: MissionId,
        run_id: RunId,
        summary: String,
    },
    Accept {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long, default_value = "human")]
        by: String,
        #[arg(long, default_value = "Accepted")]
        note: String,
    },
    Reject {
        mission_id: MissionId,
        run_id: RunId,
        #[arg(long, default_value = "human")]
        by: String,
        #[arg(long, default_value = "Rejected")]
        note: String,
    },
    Ask {
        mission_id: MissionId,
        run_id: RunId,
        question: String,
    },
    Approval {
        mission_id: MissionId,
        run_id: RunId,
        operation: String,
        #[arg(long, value_enum, default_value_t = RiskArg::Medium)]
        risk: RiskArg,
    },
    /// Allow an approval request with an explicit cooperative Grant.
    Approve {
        mission_id: MissionId,
        signal_id: SignalId,
        #[arg(long, default_value = "human")]
        by: String,
        #[arg(long, default_value = "")]
        scope: String,
        #[arg(long, default_value_t = 1)]
        uses: u32,
    },
    /// Deny an approval request without issuing a Grant.
    Deny {
        mission_id: MissionId,
        signal_id: SignalId,
        #[arg(long, default_value = "human")]
        by: String,
    },
    GrantRevoke {
        mission_id: MissionId,
        grant_id: GrantId,
        reason: String,
        #[arg(long, default_value = "human")]
        by: String,
    },
    Blocked {
        mission_id: MissionId,
        run_id: RunId,
        reason: String,
    },
    /// Escalate defective or contradictory work with an explicit requested resolution.
    Escalate {
        mission_id: MissionId,
        run_id: RunId,
        conflict: String,
        requested_resolution: String,
        #[arg(long = "evidence")]
        evidence: Vec<ArtifactId>,
    },
    Artifact {
        mission_id: MissionId,
        run_id: RunId,
        name: String,
        locator: String,
        #[arg(long, default_value = "application/octet-stream")]
        media_type: String,
        #[arg(long)]
        digest: Option<String>,
    },
    Resolve {
        mission_id: MissionId,
        signal_id: SignalId,
        response: String,
        #[arg(long, default_value = "human")]
        by: String,
    },
    SessionStart {
        mission_id: MissionId,
        name: String,
        #[arg(long, default_value = "human")]
        actor: String,
        #[arg(long)]
        engine: Option<String>,
    },
    SessionAssign {
        mission_id: MissionId,
        session_id: SessionId,
        run_id: RunId,
    },
    SessionFinish {
        mission_id: MissionId,
        session_id: SessionId,
        #[arg(long)]
        exit_code: Option<i32>,
    },
    Take {
        mission_id: MissionId,
        session_id: SessionId,
        #[arg(long, default_value = "human")]
        actor: String,
    },
    Return {
        mission_id: MissionId,
        session_id: SessionId,
        #[arg(long, default_value = "human")]
        actor: String,
        #[arg(long, default_value = "agent-1")]
        to_agent: String,
        #[arg(long, default_value = "codex")]
        engine: String,
    },
    Finish {
        mission_id: MissionId,
        run_id: RunId,
        summary: String,
        #[arg(long, value_enum, default_value_t = OutcomeArg::Succeeded)]
        outcome: OutcomeArg,
    },
    Complete {
        mission_id: MissionId,
    },
    Abandon {
        mission_id: MissionId,
    },
    Show {
        mission_id: MissionId,
    },
    /// Read the latest durable Mission events with correlation metadata.
    History {
        mission_id: MissionId,
        #[arg(long)]
        before_sequence: Option<u64>,
        #[arg(long, default_value_t = 100)]
        limit: u16,
    },
    /// Preview deterministic priority/dependency scheduling under a slot cap.
    Schedule {
        mission_id: MissionId,
        #[arg(long, default_value_t = 4)]
        max_concurrency: u16,
    },
    /// Launch every currently startable agent Run under an explicit slot cap.
    ScheduleLaunch {
        mission_id: MissionId,
        #[arg(long, default_value_t = 4)]
        max_concurrency: u16,
        #[arg(long, default_value = "agent")]
        session_name_prefix: String,
        #[arg(long)]
        program: PathBuf,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, default_value_t = 120)]
        columns: u16,
        #[arg(long, default_value_t = 36)]
        rows: u16,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Launch every currently startable agent Run through its configured engine.
    ScheduleEngineLaunch {
        mission_id: MissionId,
        #[arg(long, default_value_t = 4)]
        max_concurrency: u16,
        #[arg(long, default_value = "agent")]
        session_name_prefix: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, default_value_t = 120)]
        columns: u16,
        #[arg(long, default_value_t = 36)]
        rows: u16,
    },
    /// Enable or disable durable continuous configured-engine scheduling.
    ScheduleAuto {
        mission_id: MissionId,
        #[arg(long)]
        disable: bool,
        #[arg(long, default_value_t = 4)]
        max_concurrency: u16,
        #[arg(long, default_value = "agent")]
        session_name_prefix: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, default_value_t = 120)]
        columns: u16,
        #[arg(long, default_value_t = 36)]
        rows: u16,
    },
    /// List every durable per-Mission continuous scheduling policy.
    ScheduleAutoList,
    /// Set the durable runtime-wide agent concurrency limit.
    ScheduleSettings {
        #[arg(long)]
        global_max_concurrency: u16,
    },
    /// Show the durable runtime-wide scheduler settings.
    ScheduleSettingsShow,
    List,
    /// Print redacted runtime health, version, platform, and resource counts.
    Status,
    /// List executable plugins and their isolated runtime health.
    PluginList,
    /// Install the bundled agent-status plugin for the next daemon launch.
    PluginInstallAgentStatus {
        /// Built `ultraplexr-agent-status-plugin` executable to copy.
        executable: PathBuf,
        #[arg(long, default_value_os_t = ultraplexr_protocol::default_state_dir().join("plugins"))]
        plugin_dir: PathBuf,
    },
    Ping,
}

#[derive(Debug, clap::Args)]
struct VerificationSetupArgs {
    /// Existing owner-private JSON plan, outside the source checkout.
    #[arg(long)]
    plan: PathBuf,
    /// Existing private evidence directory, outside the source checkout.
    #[arg(long)]
    evidence_root: PathBuf,
    /// Ultraplexr CLI executable on this host; defaults to this executable.
    #[arg(long)]
    runner: Option<PathBuf>,
}

impl VerificationSetupArgs {
    fn resolve(self) -> Result<verification::Setup, CliError> {
        Ok(verification::Setup {
            plan_path: self.plan,
            evidence_root: self.evidence_root,
            runner_path: match self.runner {
                Some(path) => path,
                None => std::env::current_exe().map_err(verification::VerificationError::from)?,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RiskArg {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum EvaluationVerdictArg {
    Passed,
    Failed,
    Inconclusive,
}

impl From<EvaluationVerdictArg> for EvaluationVerdict {
    fn from(value: EvaluationVerdictArg) -> Self {
        match value {
            EvaluationVerdictArg::Passed => Self::Passed,
            EvaluationVerdictArg::Failed => Self::Failed,
            EvaluationVerdictArg::Inconclusive => Self::Inconclusive,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ShareRoleArg {
    Observer,
    Controller,
}

impl From<ShareRoleArg> for ShareRole {
    fn from(value: ShareRoleArg) -> Self {
        match value {
            ShareRoleArg::Observer => Self::Observer,
            ShareRoleArg::Controller => Self::Controller,
        }
    }
}

impl From<RiskArg> for Risk {
    fn from(value: RiskArg) -> Self {
        match value {
            RiskArg::Low => Self::Low,
            RiskArg::Medium => Self::Medium,
            RiskArg::High => Self::High,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutcomeArg {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PriorityArg {
    Urgent,
    Normal,
    Background,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PauseReasonArg {
    Attention,
    Human,
    Resource,
    System,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProviderStateArg {
    Working,
    Idle,
    WaitingInput,
    WaitingApproval,
    Blocked,
}

/// How a reported failure is classified. The reporter states this; the daemon
/// records it without reinterpretation.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum FaultKindArg {
    CommandFailed,
    TestFailed,
    BuildFailed,
    Panic,
    Crashed,
}

impl From<FaultKindArg> for FaultKind {
    fn from(value: FaultKindArg) -> Self {
        match value {
            FaultKindArg::CommandFailed => Self::CommandFailed,
            FaultKindArg::TestFailed => Self::TestFailed,
            FaultKindArg::BuildFailed => Self::BuildFailed,
            FaultKindArg::Panic => Self::Panic,
            FaultKindArg::Crashed => Self::Crashed,
        }
    }
}

/// Read failing output piped into `fault report`. An empty pipe is accepted:
/// some failures are an exit code and nothing else.
fn read_stdin_output() -> Result<String, CliError> {
    use std::io::Read;
    let mut output = String::new();
    std::io::stdin()
        .read_to_string(&mut output)
        .map_err(|error| CliError::Usage(format!("could not read failing output: {error}")))?;
    Ok(output)
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CheckEvidenceStateArg {
    Queued,
    Running,
    Passed,
    Failed,
    Cancelled,
    Skipped,
}

impl From<CheckEvidenceStateArg> for CheckEvidenceState {
    fn from(value: CheckEvidenceStateArg) -> Self {
        match value {
            CheckEvidenceStateArg::Queued => Self::Queued,
            CheckEvidenceStateArg::Running => Self::Running,
            CheckEvidenceStateArg::Passed => Self::Passed,
            CheckEvidenceStateArg::Failed => Self::Failed,
            CheckEvidenceStateArg::Cancelled => Self::Cancelled,
            CheckEvidenceStateArg::Skipped => Self::Skipped,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ChangeRequestEvidenceStateArg {
    Draft,
    Open,
    Approved,
    ChangesRequested,
    Merged,
    Closed,
}

impl From<ChangeRequestEvidenceStateArg> for ChangeRequestEvidenceState {
    fn from(value: ChangeRequestEvidenceStateArg) -> Self {
        match value {
            ChangeRequestEvidenceStateArg::Draft => Self::Draft,
            ChangeRequestEvidenceStateArg::Open => Self::Open,
            ChangeRequestEvidenceStateArg::Approved => Self::Approved,
            ChangeRequestEvidenceStateArg::ChangesRequested => Self::ChangesRequested,
            ChangeRequestEvidenceStateArg::Merged => Self::Merged,
            ChangeRequestEvidenceStateArg::Closed => Self::Closed,
        }
    }
}

impl From<ProviderStateArg> for ProviderActivityState {
    fn from(value: ProviderStateArg) -> Self {
        match value {
            ProviderStateArg::Working => Self::Working,
            ProviderStateArg::Idle => Self::Idle,
            ProviderStateArg::WaitingInput => Self::WaitingInput,
            ProviderStateArg::WaitingApproval => Self::WaitingApproval,
            ProviderStateArg::Blocked => Self::Blocked,
        }
    }
}

impl From<PauseReasonArg> for PauseReason {
    fn from(value: PauseReasonArg) -> Self {
        match value {
            PauseReasonArg::Attention => Self::Attention,
            PauseReasonArg::Human => Self::Human,
            PauseReasonArg::Resource => Self::Resource,
            PauseReasonArg::System => Self::System,
        }
    }
}

impl From<PriorityArg> for RunPriority {
    fn from(value: PriorityArg) -> Self {
        match value {
            PriorityArg::Urgent => Self::Urgent,
            PriorityArg::Normal => Self::Normal,
            PriorityArg::Background => Self::Background,
        }
    }
}

impl From<OutcomeArg> for FinishOutcome {
    fn from(value: OutcomeArg) -> Self {
        match value {
            OutcomeArg::Succeeded => Self::Succeeded,
            OutcomeArg::Failed => Self::Failed,
            OutcomeArg::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Verification(#[from] verification::VerificationError),
    #[error("could not resolve the current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
    #[error("{0}")]
    Usage(String),
    #[error("SSH port must be between 1 and 65535")]
    InvalidSshPort,
    #[error(
        "Share token file must be private and owned by this user, <=16 KiB with one <=512-byte token: {0}"
    )]
    InvalidShareTokenFile(PathBuf),
    #[error("could not connect to the runtime at {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Client(#[from] ultraplexr_client::ClientError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error(transparent)]
    Tunnel(#[from] ssh_tunnel::TunnelError),
    #[error("runtime rejected the request ({code}): {message}")]
    Remote { code: String, message: String },
    #[error("runtime returned protocol version {actual}; client requires {PROTOCOL_VERSION}")]
    Version { actual: u16 },
    #[error("runtime response id did not match its request")]
    RequestMismatch,
    #[error("could not format the response: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Plugin(#[from] ultraplexr_plugin::PluginError),
    #[error(
        "--idempotency-key cannot represent multiple scheduled launches; retry the batch without it"
    )]
    BatchIdempotencyKey,
}

#[tokio::main]
async fn main() -> Result<(), CliError> {
    let Args {
        socket,
        expected_version,
        idempotency_key,
        force_control,
        share_token_file,
        command,
    } = Args::parse();
    let share_token = share_token_file
        .as_deref()
        .map(read_share_token)
        .transpose()?;
    // A hand-off renders the same record as a brief instead of JSON, so the
    // next actor can read it (or paste it) without parsing.
    let render_handoff = matches!(command, CliCommand::FaultHandoff { .. });
    let command = match command {
        CliCommand::TerminalSearchPages {
            session_id,
            query,
            case_sensitive,
            limit,
        } => {
            if expected_version.is_some() || idempotency_key.is_some() || force_control {
                return Err(CliError::Usage(
                    "paged search accepts no mutation overrides".into(),
                ));
            }
            tokio::task::spawn_blocking(move || -> Result<(), CliError> {
                use std::io::Write;
                let client = match share_token {
                    Some(token) => {
                        ultraplexr_client::ControlClient::connect_with_share(&socket, token)?
                    }
                    None => ultraplexr_client::ControlClient::connect(&socket)?,
                };
                let mut pages =
                    client
                        .terminal(session_id)
                        .search_pages(query, case_sensitive, limit)?;
                let mut output = std::io::stdout().lock();
                while let Some(page) = pages.next_page()? {
                    serde_json::to_writer(&mut output, &page)?;
                    output
                        .write_all(b"\n")
                        .map_err(|error| CliError::Usage(error.to_string()))?;
                    output
                        .flush()
                        .map_err(|error| CliError::Usage(error.to_string()))?;
                }
                Ok(())
            })
            .await
            .map_err(|error| CliError::Usage(error.to_string()))??;
            return Ok(());
        }
        CliCommand::VerificationList {
            mission_id,
            after,
            limit,
        } => {
            if expected_version.is_some() || idempotency_key.is_some() || force_control {
                return Err(CliError::Usage(
                    "verification discovery is read-only and accepts no mutation overrides".into(),
                ));
            }
            let catalog = tokio::task::spawn_blocking(move || -> Result<_, CliError> {
                let client = match share_token {
                    Some(token) => ControlClient::connect_with_share(socket, token)?,
                    None => ControlClient::connect(socket)?,
                };
                Ok(client.verification_catalog(mission_id, after, limit)?)
            })
            .await
            .map_err(|error| CliError::Usage(error.to_string()))??;
            println!("{}", serde_json::to_string_pretty(&catalog)?);
            return Ok(());
        }
        CliCommand::VerificationStatus {
            mission_id,
            verifier_run_id,
        } => {
            if expected_version.is_some() || idempotency_key.is_some() || force_control {
                return Err(CliError::Usage(
                    "verification status is read-only and accepts no mutation overrides".into(),
                ));
            }
            let status = tokio::task::spawn_blocking(move || -> Result<_, CliError> {
                let client = match share_token {
                    Some(token) => ControlClient::connect_with_share(socket, token)?,
                    None => ControlClient::connect(socket)?,
                };
                Ok(verification::inspect(&client, mission_id, verifier_run_id)?)
            })
            .await
            .map_err(|error| CliError::Usage(error.to_string()))??;
            println!("{}", serde_json::to_string_pretty(&status)?);
            return Ok(());
        }
        CliCommand::VerificationPrepare { setup } => {
            if share_token.is_some()
                || expected_version.is_some()
                || idempotency_key.is_some()
                || force_control
            {
                return Err(CliError::Usage("verification preparation is local-only and accepts no Share or mutation overrides".into()));
            }
            let setup = setup.resolve()?;
            let reviewed = tokio::task::spawn_blocking(move || verification::prepare(&setup))
                .await
                .map_err(|error| CliError::Usage(error.to_string()))??;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "setup":reviewed.setup, "plan_sha256":reviewed.sha256,
                    "description":reviewed.description, "execution_started":false,
                    "review_required":true,
                    "trust":"Local OS-user execution; executable contents and inherited environment are not pinned"
                }))?
            );
            return Ok(());
        }
        CliCommand::VerificationLaunch {
            mission_id,
            subject,
            verifier,
            new_verifier_id,
            setup,
            plan_sha256,
        } => {
            if share_token.is_some()
                || expected_version.is_some()
                || idempotency_key.is_some()
                || force_control
            {
                return Err(CliError::Usage("verification launch requires an owner connection and shared-workflow versioning; no mutation overrides are accepted".into()));
            }
            if plan_sha256.len() != 64 || !plan_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(CliError::Usage(
                    "plan-sha256 must be the 64-digit SHA-256 from the reviewed plan".into(),
                ));
            }
            let (target, attempted_verifier) = match (subject, verifier) {
                (Some(subject), None) => {
                    let verifier = new_verifier_id.unwrap_or_else(RunId::new);
                    (
                        verification::LaunchTarget::NewVerifier { subject, verifier },
                        verifier,
                    )
                }
                (None, Some(verifier)) if new_verifier_id.is_none() => {
                    (verification::LaunchTarget::Verifier(verifier), verifier)
                }
                _ => {
                    return Err(CliError::Usage(
                        "select exactly one of --subject or --verifier".into(),
                    ));
                }
            };
            let setup = setup.resolve()?;
            let expected = plan_sha256.to_ascii_lowercase();
            let outcome = tokio::task::spawn_blocking(move || -> Result<_, CliError> {
                let reviewed = verification::prepare(&setup)?;
                if reviewed.sha256 != expected {
                    return Err(CliError::Usage("plan changed since review; run verification-prepare and review it again".into()));
                }
                let client = ControlClient::connect(socket)?;
                verification::launch(&client, mission_id, target, &reviewed).map_err(|error| {
                    CliError::Usage(format!("Verification launch did not complete: {error}. Inspect Mission {mission_id} before retrying; a Run may already have been created or launched. No launch was replayed."))
                })
            }).await.map_err(|error| CliError::Usage(format!("Verification launch worker ended: {error}; inspect Mission {mission_id} before retrying")))
                .and_then(|result| result);
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "mission_id":mission_id, "verifier_run_id":attempted_verifier,
                            "run_existence":"unconfirmed", "launch_confirmed":false,
                            "launch_error":error.to_string(), "acceptance_requested":false,
                            "next_action":"Inspect this exact Run ID in the Mission. If it exists and is pending, use --verifier; never blindly create a replacement"
                        }))?
                    );
                    return Err(error);
                }
            };
            let confirmed = outcome.launch_error.is_none();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "mission_id":mission_id, "verifier_run_id":outcome.run_id,
                    "run_existence":"confirmed",
                    "plan_sha256":plan_sha256.to_ascii_lowercase(),
                    "launch_confirmed":confirmed, "launch_error":outcome.launch_error,
                    "verification_passed":null, "acceptance_requested":false,
                    "next_action":if confirmed { "Wait for this verifier, then collect evidence separately" }
                        else { "Inspect this verifier; resume with --verifier only if it is still pending. Do not create a replacement blindly" }
                }))?
            );
            if !confirmed {
                return Err(CliError::Usage("launch was not confirmed; the verifier identity is preserved in the JSON result".into()));
            }
            return Ok(());
        }
        CliCommand::VerificationPlanRust { recipe, output } => {
            if share_token.is_some()
                || expected_version.is_some()
                || idempotency_key.is_some()
                || force_control
            {
                return Err(CliError::Usage(
                    "plan generation is local-only and accepts no connection or mutation overrides"
                        .into(),
                ));
            }
            let runner = std::env::current_exe().map_err(verification::VerificationError::from)?;
            let plan = output.clone();
            tokio::task::spawn_blocking(move || {
                verification::write_rust_project_plan(&recipe, &plan, &runner)
            })
            .await
            .map_err(|error| CliError::Usage(error.to_string()))??;
            println!("{}", output.display());
            return Ok(());
        }
        CliCommand::VerificationRustCheck { check, recipe_json } => {
            if share_token.is_some()
                || expected_version.is_some()
                || idempotency_key.is_some()
                || force_control
            {
                return Err(CliError::Usage(
                    "recipe checks are local-only and accept no connection or mutation overrides"
                        .into(),
                ));
            }
            tokio::task::spawn_blocking(move || {
                verification::execute_rust_project_check(&recipe_json, &check)
            })
            .await
            .map_err(|error| CliError::Usage(error.to_string()))??;
            return Ok(());
        }
        CliCommand::VerificationExecute {
            plan,
            evidence_root,
            plan_sha256,
        } => {
            if share_token.is_some()
                || expected_version.is_some()
                || idempotency_key.is_some()
                || force_control
            {
                return Err(CliError::Usage(
                    "verification execution does not accept connection or mutation overrides"
                        .into(),
                ));
            }
            let report = verification::execute_reviewed(plan, evidence_root, plan_sha256).await?;
            println!("{}", report.display());
            return Ok(());
        }
        CliCommand::VerificationCollect {
            mission_id,
            verifier_run_id,
            evidence_root,
        } => {
            if share_token.is_some()
                || expected_version.is_some()
                || idempotency_key.is_some()
                || force_control
            {
                return Err(CliError::Usage("verification collection requires an owner connection and uses report-derived idempotency keys".into()));
            }
            let receipt = tokio::task::spawn_blocking(move || {
                let client = ControlClient::connect(socket)?;
                verification::collect(&client, mission_id, verifier_run_id, &evidence_root)
            })
            .await
            .map_err(|error| CliError::Usage(error.to_string()))??;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
            return Ok(());
        }
        CliCommand::Events { scope } => {
            return stream_events(&socket, share_token, scope).await;
        }
        CliCommand::RemoteForward {
            destination,
            local_socket,
            remote_socket,
            port,
            identity,
            known_hosts,
            jump,
            once,
        } => {
            validate_optional_ssh_port(port)?;
            return ssh_tunnel::run(ssh_tunnel::TunnelSpec {
                destination,
                local_socket,
                remote_socket,
                port,
                identity,
                known_hosts,
                jump,
                once,
            })
            .await
            .map_err(CliError::from);
        }
        CliCommand::ScheduleLaunch {
            mission_id,
            max_concurrency,
            session_name_prefix,
            program,
            cwd,
            columns,
            rows,
            args,
        } => {
            if idempotency_key.is_some() {
                return Err(CliError::BatchIdempotencyKey);
            }
            return launch_scheduled_runs(
                &socket,
                expected_version,
                mission_id,
                max_concurrency,
                session_name_prefix,
                program,
                cwd,
                GridSize::new(columns, rows)?,
                args,
                share_token,
            )
            .await;
        }
        CliCommand::ShellInit { shell, install } => {
            return run_shell_init(shell.as_deref(), install);
        }
        CliCommand::FaultFix {
            fault_id,
            mission_id,
            engine,
            actor,
            plan_only,
        } => {
            return run_fault_fix(
                &socket,
                share_token.clone(),
                fault_id,
                mission_id,
                &engine,
                &actor,
                plan_only,
            );
        }
        CliCommand::PluginInstallAgentStatus {
            executable,
            plugin_dir,
        } => {
            let installed = ultraplexr_plugin::install_agent_status(plugin_dir, executable)?;
            println!("{}", installed.display());
            return Ok(());
        }
        command => command,
    };
    let action = into_request(command)?;
    let client_id = Uuid::new_v4();
    let surface_id = Uuid::new_v4();
    let terminal_mutation = terminal_mutation_session(&action);
    let starts_terminal = matches!(action, Request::StartTerminal { .. });
    let mut request = ClientRequest::for_client(client_id, action);
    request.share_token.clone_from(&share_token);
    if let Some(idempotency_key) = idempotency_key {
        request.request_id = idempotency_key;
    }
    request.expected_mission_version = expected_version;
    if starts_terminal {
        request.surface_id = Some(surface_id);
    }
    let (mut read, mut write) = open_cli_wire(&socket, client_id).await?;
    if let Some(session_id) = terminal_mutation {
        let mut claim = ClientRequest::for_client(
            client_id,
            Request::ClaimTerminalControl {
                session_id,
                force: force_control,
            },
        );
        claim.share_token.clone_from(&share_token);
        claim.surface_id = Some(surface_id);
        let epoch = match transact(&mut read, &mut write, &claim).await? {
            ResponseBody::TerminalControlChanged { control_epoch, .. } => control_epoch,
            body => {
                return Err(CliError::Remote {
                    code: "unexpected_response".to_owned(),
                    message: format!("control claim returned {body:?}"),
                });
            }
        };
        request.surface_id = Some(surface_id);
        request.control_epoch = Some(epoch);
    }
    let body = transact(&mut read, &mut write, &request).await?;
    if render_handoff && let ResponseBody::FaultRecorded { fault } = &body {
        println!("{}", fault_brief(fault));
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(())
}

/// Render a Fault as a compact brief for the next actor, human or agent.
fn fault_brief(fault: &ultraplexr_protocol::FaultSummary) -> String {
    use std::fmt::Write as _;
    let mut brief = String::new();
    let _ = writeln!(brief, "FAULT {}", fault.fault_id);
    let _ = writeln!(brief, "summary: {}", fault.summary);
    let _ = writeln!(brief, "command: {}", fault.command);
    let _ = writeln!(brief, "cwd:     {}", fault.cwd.display());
    if let Some(exit_code) = fault.exit_code {
        let _ = writeln!(brief, "exit:    {exit_code}");
    }
    if let Some(revision) = &fault.revision {
        let _ = writeln!(brief, "rev:     {revision}");
    }
    let state = match &fault.state {
        ultraplexr_protocol::FaultState::Open => "open".to_owned(),
        ultraplexr_protocol::FaultState::Resolved { note, .. } => format!("resolved · {note}"),
        ultraplexr_protocol::FaultState::Dismissed { note, .. } => format!("dismissed · {note}"),
    };
    let _ = writeln!(brief, "state:   {state}");
    let replay = match &fault.repro {
        None => "never replayed".to_owned(),
        Some(receipt) => match (&receipt.error, receipt.reproduced) {
            (Some(error), _) => format!("could not run · {error}"),
            (None, true) => format!(
                "still fails · exit {} after {} ms",
                receipt
                    .exit_code
                    .map_or_else(|| "?".to_owned(), |code| code.to_string()),
                receipt.duration_ms
            ),
            (None, false) => format!("passes now · after {} ms", receipt.duration_ms),
        },
    };
    let _ = writeln!(
        brief,
        "replay:  {replay} ({} attempts)",
        fault.repro_attempts
    );
    let _ = writeln!(brief, "\n--- failing output ---");
    brief.push_str(fault.output.trim_end());
    brief.push('\n');
    let _ = write!(
        brief,
        "\nReproduce with: ultraplexr fault repro {}",
        fault.fault_id
    );
    brief
}

#[allow(clippy::too_many_arguments)]
async fn launch_scheduled_runs(
    socket: &Path,
    mut expected_version: Option<u64>,
    mission_id: MissionId,
    max_concurrency: u16,
    session_name_prefix: String,
    program: PathBuf,
    cwd: Option<PathBuf>,
    grid: GridSize,
    args: Vec<String>,
    share_token: Option<String>,
) -> Result<(), CliError> {
    let cwd = match cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir().map_err(CliError::CurrentDirectory)?,
    };
    let client_id = Uuid::new_v4();
    let surface_id = Uuid::new_v4();
    let (mut read, mut write) = open_cli_wire(socket, client_id).await?;
    let mut plan_request = ClientRequest::for_client(
        client_id,
        Request::SchedulerPlan {
            mission_id,
            max_concurrency,
        },
    );
    plan_request.share_token.clone_from(&share_token);
    let plan = match transact(&mut read, &mut write, &plan_request).await? {
        ResponseBody::SchedulerPlan { plan, .. } => plan,
        body => {
            return Err(CliError::Remote {
                code: "unexpected_response".to_owned(),
                message: format!("scheduler returned {body:?}"),
            });
        }
    };
    let mut launched = Vec::new();
    let mut failures = Vec::new();
    for scheduled in &plan.startable {
        let session_id = SessionId::new();
        let short_run_id = scheduled.run_id.to_string();
        let session_name = format!("{session_name_prefix}-{}", &short_run_id[..8]);
        let mut request = ClientRequest::for_client(
            client_id,
            Request::LaunchAgentRun {
                mission_id,
                run_id: scheduled.run_id,
                session_name,
                spec: TerminalSessionSpec {
                    session_id,
                    mission_id: Some(mission_id),
                    run_id: Some(scheduled.run_id),
                    program: program.clone(),
                    args: args.clone(),
                    cwd: cwd.clone(),
                    environment_delta: BTreeMap::new(),
                    grid,
                },
            },
        );
        request.share_token.clone_from(&share_token);
        request.surface_id = Some(surface_id);
        request.expected_mission_version = expected_version;
        match transact(&mut read, &mut write, &request).await {
            Ok(ResponseBody::AgentRunLaunched {
                events,
                mission,
                terminal,
            }) => {
                expected_version = Some(mission.version);
                launched.push(serde_json::json!({
                    "run_id": scheduled.run_id,
                    "session_id": terminal.session_id,
                    "process_id": terminal.process_id,
                    "events_committed": events.len(),
                    "mission_version": mission.version,
                }));
            }
            Ok(body) => {
                failures.push(serde_json::json!({
                    "run_id": scheduled.run_id,
                    "code": "unexpected_response",
                    "message": format!("launch returned {body:?}"),
                }));
            }
            Err(CliError::Remote { code, message }) => {
                failures.push(serde_json::json!({
                    "run_id": scheduled.run_id,
                    "code": code,
                    "message": message,
                }));
            }
            Err(error) => return Err(error),
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "type": "scheduled_runs_launched",
            "mission_id": mission_id,
            "max_concurrency": plan.max_concurrency,
            "occupied_slots": plan.occupied_slots,
            "available_slots": plan.available_slots,
            "selected": plan.startable.len(),
            "still_queued": plan.ready_queued,
            "launched": launched,
            "failures": failures,
        }))?
    );
    Ok(())
}

async fn transact<R, W>(
    read: &mut AsyncWireReader<R>,
    write: &mut AsyncWireWriter<W>,
    request: &ClientRequest,
) -> Result<ResponseBody, CliError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    write
        .send_json(FrameKind::Request, 0, request)
        .await
        .map_err(ProtocolError::from)?;
    // Bound the wait. Without this a daemon that never answers — busy, wedged,
    // or replaying a large journal — hangs the command forever, which is worse
    // in a script or an agent loop than a reported failure.
    let timeout = ultraplexr_client::request_timeout(&request.action);
    let response: ServerResponse =
        tokio::time::timeout(timeout, read.receive_json(FrameKind::Response, 0))
            .await
            .map_err(|_| {
                CliError::Protocol(ProtocolError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("daemon did not answer within {timeout:?}"),
                )))
            })?
            .map_err(ProtocolError::from)?;
    if response.version != PROTOCOL_VERSION {
        return Err(CliError::Version {
            actual: response.version,
        });
    }
    if response.request_id != request.request_id {
        return Err(CliError::RequestMismatch);
    }
    match response.result {
        ResponseResult::Success { body } => Ok(*body),
        ResponseResult::Error { code, message } => Err(CliError::Remote { code, message }),
    }
}

type CliWireReader = AsyncWireReader<BufReader<tokio::net::unix::OwnedReadHalf>>;
type CliWireWriter = AsyncWireWriter<tokio::net::unix::OwnedWriteHalf>;

async fn open_cli_wire(
    socket: &Path,
    client_id: Uuid,
) -> Result<(CliWireReader, CliWireWriter), CliError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|source| CliError::Connect {
            path: socket.to_owned(),
            source,
        })?;
    let (read, write) = stream.into_split();
    let mut read = AsyncWireReader::new(BufReader::new(read));
    let mut write = AsyncWireWriter::new(write);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client_handshake(
            &mut read,
            &mut write,
            &Hello::new(client_id, "cli", client_id),
        ),
    )
    .await
    .map_err(|_| {
        ProtocolError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "local protocol handshake exceeded five seconds",
        ))
    })?
    .map_err(ProtocolError::from)?;
    Ok((read, write))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum EventScopeArg {
    All,
    Missions,
    Activities,
    Terminals,
    Groups,
}

async fn stream_events(
    socket: &Path,
    share_token: Option<String>,
    scope: EventScopeArg,
) -> Result<(), CliError> {
    let client = match share_token {
        Some(token) => ControlClient::connect_with_share(socket, token)?,
        None => ControlClient::connect(socket)?,
    };
    let (send, mut receive) = tokio::sync::mpsc::channel(4);
    let mut cancellations = Vec::new();

    if matches!(scope, EventScopeArg::All | EventScopeArg::Missions) {
        cancellations.push(forward_event_stream(
            "missions",
            client.subscribe_missions()?,
            send.clone(),
            |mission| CliStreamEvent::Missions(Box::new(MissionEvent::MissionChanged { mission })),
        )?);
    }
    if matches!(scope, EventScopeArg::All | EventScopeArg::Terminals) {
        cancellations.push(forward_event_stream(
            "terminals",
            client.subscribe_terminals()?,
            send.clone(),
            |terminal| CliStreamEvent::Terminals(TerminalIndexEvent::TerminalChanged { terminal }),
        )?);
    }
    if matches!(scope, EventScopeArg::All | EventScopeArg::Activities) {
        cancellations.push(forward_event_stream(
            "activities",
            client.subscribe_run_activities(None)?,
            send.clone(),
            |activity| CliStreamEvent::Activities(RunActivityEvent::ActivityChanged { activity }),
        )?);
    }
    if matches!(scope, EventScopeArg::All | EventScopeArg::Groups) {
        cancellations.push(forward_event_stream(
            "session-groups",
            client.subscribe_session_groups(None)?,
            send.clone(),
            CliStreamEvent::SessionGroups,
        )?);
    }
    drop(send);

    loop {
        tokio::select! {
            event = receive.recv() => {
                let Some(event) = event else { return Ok(()); };
                if let Some(event) = event?.into_current() { event.print()?; }
            }
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(ProtocolError::Io)?;
                return Ok(());
            }
        }
    }
}

enum CliStreamEvent {
    Missions(Box<MissionEvent>),
    Terminals(TerminalIndexEvent),
    Activities(RunActivityEvent),
    SessionGroups(SessionGroupEvent),
    Snapshot(
        &'static str,
        ultraplexr_protocol::collection_stream::SnapshotMarker,
    ),
}

impl CliStreamEvent {
    fn print(self) -> Result<(), CliError> {
        match self {
            Self::Missions(event) => print_ndjson("missions", event),
            Self::Terminals(event) => print_ndjson("terminals", event),
            Self::Activities(event) => print_ndjson("activities", event),
            Self::SessionGroups(event) => print_ndjson("session_groups", event),
            Self::Snapshot(stream, marker) => print_ndjson(stream, marker),
        }
    }
}

fn forward_event_stream<T: Send + 'static>(
    thread_name: &'static str,
    receive: ultraplexr_client::MetadataReceiver<T>,
    send: tokio::sync::mpsc::Sender<
        Result<ultraplexr_client::MetadataDelivery<CliStreamEvent>, ultraplexr_client::ClientError>,
    >,
    map: impl Fn(T) -> CliStreamEvent + Send + 'static,
) -> Result<ultraplexr_client::MetadataCancellation, CliError> {
    let cancel = receive.cancellation();
    std::thread::Builder::new()
        .name(format!("ultraplexr-cli-{thread_name}"))
        .spawn(move || {
            loop {
                match receive.recv_update_delivery() {
                    Ok(event) => {
                        let event = event.map(|update| match update {
                            ultraplexr_client::MetadataUpdate::Item(item) => map(item),
                            ultraplexr_client::MetadataUpdate::Snapshot(marker) => {
                                CliStreamEvent::Snapshot(
                                    if thread_name == "session-groups" {
                                        "session_groups"
                                    } else {
                                        thread_name
                                    },
                                    marker,
                                )
                            }
                        });
                        if send.blocking_send(Ok(event)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = send.blocking_send(Err(error));
                        break;
                    }
                }
            }
        })
        .map_err(ProtocolError::Io)?;
    Ok(cancel)
}

fn print_ndjson(event_stream: &str, event: impl serde::Serialize) -> Result<(), CliError> {
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "stream": event_stream,
            "event": event,
        }))?
    );
    Ok(())
}

const fn terminal_mutation_session(request: &Request) -> Option<SessionId> {
    match request {
        Request::TerminalKey { session_id, .. }
        | Request::TerminalPaste { session_id, .. }
        | Request::TerminalFocus { session_id, .. }
        | Request::TerminalMouse { session_id, .. }
        | Request::TerminalScroll { session_id, .. }
        | Request::TerminalSelect { session_id, .. }
        | Request::TerminalClearSelection { session_id }
        | Request::TerminalResize { session_id, .. }
        | Request::KillTerminal { session_id }
        | Request::InterruptTerminal { session_id }
        | Request::TerminateTerminal { session_id } => Some(*session_id),
        _ => None,
    }
}

fn change_claims(
    paths: Vec<String>,
    operation: ChangeOperation,
    scope: ChangeScope,
) -> impl Iterator<Item = ChangeClaim> {
    paths.into_iter().map(move |path| ChangeClaim {
        path,
        operation,
        scope,
    })
}

fn change_claim_keys(
    paths: Vec<String>,
    operation: ChangeOperation,
) -> impl Iterator<Item = ChangeClaimKey> {
    paths
        .into_iter()
        .map(move |path| ChangeClaimKey { path, operation })
}

/// Hand a Fault to an agent Run.
///
/// Three steps, in order, so a failure never points at work that does not
/// exist: read the Fault, plan a Run carrying its objective, link them, then
/// launch the configured driver.
fn run_fault_fix(
    socket: &Path,
    share_token: Option<String>,
    fault_id: FaultId,
    mission_id: MissionId,
    engine: &str,
    actor: &str,
    plan_only: bool,
) -> Result<(), CliError> {
    let client = match share_token {
        Some(token) => ControlClient::connect_with_share(socket, token)?,
        None => ControlClient::connect(socket)?,
    };
    let fault = client
        .fault(fault_id)
        .map_err(|error| CliError::Usage(error.to_string()))?;
    if !fault.is_open() {
        return Err(CliError::Usage(format!(
            "Fault {fault_id} is already closed"
        )));
    }

    let run_id = RunId::new();
    client
        .dispatch(
            mission_id,
            Command::PlanRun {
                run_id,
                parent: None,
                dependencies: Vec::new(),
                retry_of: None,
                actor: Actor::agent(actor, engine)?,
                objective: fault.fix_objective(),
                priority: RunPriority::Normal,
            },
        )
        .map_err(|error| CliError::Usage(format!("could not plan the fix Run: {error}")))?;

    let linked = client
        .assign_fault_fix(fault_id, mission_id, run_id)
        .map_err(|error| CliError::Usage(format!("could not link the Run: {error}")))?;
    println!("planned Run {run_id} for Fault {fault_id}");

    if plan_only {
        println!("{}", serde_json::to_string_pretty(&linked)?);
        return Ok(());
    }

    // The configured launch starts the Run itself; starting it here first
    // would leave nothing pending for the driver to claim.
    let launched = client
        .launch_configured_agent_run(
            mission_id,
            run_id,
            SessionId::new(),
            format!("fix {}", short_command(&fault.command)),
            fault.cwd.clone(),
            GridSize::new(120, 36)?,
        )
        .map_err(|error| CliError::Usage(format!("could not launch the driver: {error}")))?;
    let (_events, _mission, session) = launched;
    println!("{}", serde_json::to_string_pretty(&session)?);
    println!("When it reports done, verify with: ultraplexr fault repro {fault_id}");
    Ok(())
}

/// A short, readable stand-in for a command in a Session name.
fn short_command(command: &str) -> String {
    let first = command
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    if first.len() <= 40 {
        return first;
    }
    let mut end = 40;
    while end > 0 && !first.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &first[..end])
}

/// Print the shell integration snippet, or install it into a startup file.
fn run_shell_init(shell: Option<&str>, install: bool) -> Result<(), CliError> {
    let shell = match shell {
        Some(name) => name.parse::<shell_init::Shell>().map_err(CliError::Usage)?,
        None => shell_init::Shell::detect(std::env::var("SHELL").ok().as_deref()),
    };
    if !install {
        print!("{}", shell_init::snippet(shell));
        return Ok(());
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CliError::Usage("HOME is not set".to_owned()))?;
    let path = home.join(shell.startup_file());
    let existing = match std::fs::read_to_string(&path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(CliError::Usage(format!(
                "could not read {}: {error}",
                path.display()
            )));
        }
    };
    let (updated, outcome) = shell_init::install_into(&existing, shell);
    match outcome {
        shell_init::InstallOutcome::Unchanged => {
            println!("{} already up to date", path.display());
            return Ok(());
        }
        shell_init::InstallOutcome::Added | shell_init::InstallOutcome::Updated => {}
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CliError::Usage(format!("could not create {}: {error}", parent.display()))
        })?;
    }
    // Write through a temporary file so an interrupted install cannot leave a
    // startup file truncated.
    let temporary = path.with_extension("ultraplexr-tmp");
    std::fs::write(&temporary, updated.as_bytes()).map_err(|error| {
        CliError::Usage(format!("could not write {}: {error}", temporary.display()))
    })?;
    std::fs::rename(&temporary, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        CliError::Usage(format!("could not update {}: {error}", path.display()))
    })?;
    let verb = if outcome == shell_init::InstallOutcome::Added {
        "installed into"
    } else {
        "updated in"
    };
    println!("{shell} integration {verb} {}", path.display());
    println!("Open a new terminal, or source that file, to start reporting failures.");
    Ok(())
}

fn into_request(command: CliCommand) -> Result<Request, CliError> {
    let request = match command {
        CliCommand::TerminalNew {
            program,
            cwd,
            columns,
            rows,
            args,
        } => Request::StartTerminal {
            spec: TerminalSessionSpec {
                session_id: SessionId::new(),
                mission_id: None,
                run_id: None,
                program: program
                    .or_else(|| std::env::var_os("SHELL").map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from("/bin/sh")),
                args,
                cwd: match cwd {
                    Some(cwd) => cwd,
                    None => std::env::current_dir().map_err(CliError::CurrentDirectory)?,
                },
                environment_delta: BTreeMap::new(),
                grid: GridSize::new(columns, rows)?,
            },
        },
        CliCommand::TerminalSsh {
            destination,
            port,
            identity,
            jump,
            columns,
            rows,
        } => {
            let mut args = Vec::new();
            if let Some(port) = port {
                validate_optional_ssh_port(Some(port))?;
                args.extend(["-p".to_owned(), port.to_string()]);
            }
            if let Some(identity) = identity {
                args.extend(["-i".to_owned(), identity.to_string_lossy().into_owned()]);
            }
            if let Some(jump) = jump {
                args.extend(["-J".to_owned(), jump]);
            }
            args.extend(["--".to_owned(), destination]);
            Request::StartTerminal {
                spec: TerminalSessionSpec {
                    session_id: SessionId::new(),
                    mission_id: None,
                    run_id: None,
                    program: PathBuf::from("ssh"),
                    args,
                    cwd: std::env::current_dir().map_err(CliError::CurrentDirectory)?,
                    environment_delta: BTreeMap::new(),
                    grid: GridSize::new(columns, rows)?,
                },
            }
        }
        CliCommand::TerminalList { all } => Request::ListTerminals {
            include_archived: all,
        },
        CliCommand::SessionGroupCreate {
            name,
            mission_id,
            session_ids,
            position,
            pinned,
            detached,
        } => Request::CreateSessionGroup {
            group: SessionGroupSpec {
                group_id: SessionGroupId::new(),
                mission_id,
                name,
                session_ids,
                position,
                pinned,
                detached,
            },
        },
        CliCommand::SessionGroupList { mission_id } => Request::ListSessionGroups { mission_id },
        CliCommand::SessionGroupRename {
            group_id,
            expected_group_version,
            name,
        } => Request::UpdateSessionGroup {
            group_id,
            expected_version: expected_group_version,
            change: SessionGroupChange::Rename { name },
        },
        CliCommand::SessionGroupAdd {
            group_id,
            expected_group_version,
            session_id,
        } => Request::UpdateSessionGroup {
            group_id,
            expected_version: expected_group_version,
            change: SessionGroupChange::AddSession { session_id },
        },
        CliCommand::SessionGroupRemove {
            group_id,
            expected_group_version,
            session_id,
        } => Request::UpdateSessionGroup {
            group_id,
            expected_version: expected_group_version,
            change: SessionGroupChange::RemoveSession { session_id },
        },
        CliCommand::SessionGroupPin {
            group_id,
            expected_group_version,
            pinned,
        } => Request::UpdateSessionGroup {
            group_id,
            expected_version: expected_group_version,
            change: SessionGroupChange::SetPinned { pinned },
        },
        CliCommand::SessionGroupDetach {
            group_id,
            expected_group_version,
        } => Request::UpdateSessionGroup {
            group_id,
            expected_version: expected_group_version,
            change: SessionGroupChange::SetDetached { detached: true },
        },
        CliCommand::SessionGroupReattach {
            group_id,
            expected_group_version,
        } => Request::UpdateSessionGroup {
            group_id,
            expected_version: expected_group_version,
            change: SessionGroupChange::SetDetached { detached: false },
        },
        CliCommand::SessionGroupDelete {
            group_id,
            expected_group_version,
        } => Request::DeleteSessionGroup {
            group_id,
            expected_version: expected_group_version,
        },
        CliCommand::TerminalSnapshot { session_id } => Request::TerminalSnapshot { session_id },
        CliCommand::TerminalCapture { session_id } => Request::TerminalCapture { session_id },
        CliCommand::TerminalWrite { session_id, text } => Request::TerminalPaste {
            session_id,
            bytes: text.into_bytes(),
            confirmed: true,
        },
        CliCommand::TerminalResize {
            session_id,
            columns,
            rows,
        } => Request::TerminalResize {
            session_id,
            grid: GridSize::new(columns, rows)?,
            cell_width_px: 0,
            cell_height_px: 0,
        },
        CliCommand::TerminalScroll { session_id, delta } => Request::TerminalScroll {
            session_id,
            scroll: ViewportScroll::Delta(delta),
        },
        CliCommand::TerminalSelect {
            session_id,
            anchor_column,
            anchor_row,
            head_column,
            head_row,
            rectangle,
        } => Request::TerminalSelect {
            session_id,
            anchor: SelectionPoint {
                column: anchor_column,
                row: anchor_row,
            },
            head: SelectionPoint {
                column: head_column,
                row: head_row,
            },
            rectangle,
        },
        CliCommand::TerminalCopy { session_id } => Request::TerminalSelectionText { session_id },
        CliCommand::TerminalHistory {
            session_id,
            rows_before_bottom,
        } => Request::TerminalHistoryFrame {
            session_id,
            viewport: HistoryViewport::RowsBeforeBottom(rows_before_bottom),
        },
        CliCommand::TerminalSearch {
            session_id,
            query,
            case_sensitive,
            limit,
        } => Request::TerminalSearch {
            session_id,
            query,
            case_sensitive,
            limit,
        },
        CliCommand::TerminalSearchPages { .. } => {
            return Err(CliError::Usage(
                "paged search uses the shared streaming client".into(),
            ));
        }
        CliCommand::TerminalWaitText {
            session_id,
            query,
            case_sensitive,
            timeout_millis,
        } => Request::TerminalWait {
            session_id,
            condition: TerminalWaitCondition::Text {
                query,
                case_sensitive,
            },
            timeout_millis,
        },
        CliCommand::TerminalWaitQuiet {
            session_id,
            quiet_millis,
            timeout_millis,
        } => Request::TerminalWait {
            session_id,
            condition: TerminalWaitCondition::Quiet { quiet_millis },
            timeout_millis,
        },
        CliCommand::TerminalWaitExit {
            session_id,
            timeout_millis,
        } => Request::TerminalWait {
            session_id,
            condition: TerminalWaitCondition::Exit,
            timeout_millis,
        },
        CliCommand::TerminalKill { session_id } => Request::KillTerminal { session_id },
        CliCommand::TerminalInterrupt { session_id } => Request::InterruptTerminal { session_id },
        CliCommand::TerminalTerminate { session_id } => Request::TerminateTerminal { session_id },
        CliCommand::TerminalArchive { session_id } => Request::ArchiveTerminal { session_id },
        CliCommand::TerminalRestore { session_id } => Request::RestoreTerminal { session_id },
        CliCommand::Create { intent, actor } => Request::CreateMission {
            mission_id: MissionId::new(),
            intent,
            created_by: Actor::human(actor)?,
        },
        CliCommand::Start {
            mission_id,
            objective,
            actor,
            engine,
            parent,
        } => Request::Dispatch {
            mission_id,
            command: Command::StartRun {
                run_id: RunId::new(),
                parent,
                actor: Actor::agent(actor, engine)?,
                objective,
            },
        },
        CliCommand::Plan {
            mission_id,
            objective,
            actor,
            engine,
            parent,
            dependencies,
            retry_of,
            priority,
        } => Request::Dispatch {
            mission_id,
            command: Command::PlanRun {
                run_id: RunId::new(),
                parent,
                dependencies,
                retry_of,
                actor: Actor::agent(actor, engine)?,
                objective,
                priority: priority.into(),
            },
        },
        CliCommand::StartReady { mission_id, run_id } => Request::Dispatch {
            mission_id,
            command: Command::StartReadyRun { run_id },
        },
        CliCommand::RunLaunch {
            mission_id,
            run_id,
            session_name,
            program,
            cwd,
            columns,
            rows,
            args,
        } => Request::LaunchAgentRun {
            mission_id,
            run_id,
            session_name,
            spec: TerminalSessionSpec {
                session_id: SessionId::new(),
                mission_id: Some(mission_id),
                run_id: Some(run_id),
                program,
                args,
                cwd: match cwd {
                    Some(cwd) => cwd,
                    None => std::env::current_dir().map_err(CliError::CurrentDirectory)?,
                },
                environment_delta: BTreeMap::new(),
                grid: GridSize::new(columns, rows)?,
            },
        },
        CliCommand::RunEngine {
            mission_id,
            run_id,
            session_name,
            cwd,
            checkout,
            columns,
            rows,
        } => Request::LaunchConfiguredAgentRun {
            mission_id,
            run_id,
            session_id: SessionId::new(),
            session_name,
            cwd: match cwd {
                Some(cwd) => cwd,
                None => std::env::current_dir().map_err(CliError::CurrentDirectory)?,
            },
            use_run_checkout: checkout,
            grid: GridSize::new(columns, rows)?,
        },
        CliCommand::RunEnginePreview {
            mission_id,
            run_id,
            cwd,
            checkout,
            columns,
            rows,
        } => Request::PreviewConfiguredAgentRun {
            mission_id,
            run_id,
            cwd: match cwd {
                Some(cwd) => cwd,
                None => std::env::current_dir().map_err(CliError::CurrentDirectory)?,
            },
            use_run_checkout: checkout,
            grid: GridSize::new(columns, rows)?,
        },
        CliCommand::RunCheckoutNew {
            mission_id,
            run_id,
            repository,
            base_ref,
        } => Request::PrepareRunCheckout {
            mission_id,
            run_id,
            repository: match repository {
                Some(repository) => repository,
                None => std::env::current_dir().map_err(CliError::CurrentDirectory)?,
            },
            base_ref,
        },
        CliCommand::RunCheckoutList { mission_id } => Request::ListRunCheckouts { mission_id },
        CliCommand::RunCheckoutRetire {
            mission_id,
            run_id,
            merged_into_ref,
        } => Request::RetireRunCheckout {
            mission_id,
            run_id,
            merged_into_ref,
        },
        CliCommand::ChangeIntentDeclare {
            mission_id,
            run_id,
            repository_identity,
            base_revision,
            expected_intent_version,
            create_paths,
            modify_paths,
            delete_paths,
            contingent_create_paths,
            contingent_modify_paths,
            contingent_delete_paths,
        } => {
            let mut claims = Vec::new();
            claims.extend(change_claims(
                create_paths,
                ChangeOperation::Create,
                ChangeScope::Committed,
            ));
            claims.extend(change_claims(
                modify_paths,
                ChangeOperation::Modify,
                ChangeScope::Committed,
            ));
            claims.extend(change_claims(
                delete_paths,
                ChangeOperation::Delete,
                ChangeScope::Committed,
            ));
            claims.extend(change_claims(
                contingent_create_paths,
                ChangeOperation::Create,
                ChangeScope::Contingent,
            ));
            claims.extend(change_claims(
                contingent_modify_paths,
                ChangeOperation::Modify,
                ChangeScope::Contingent,
            ));
            claims.extend(change_claims(
                contingent_delete_paths,
                ChangeOperation::Delete,
                ChangeScope::Contingent,
            ));
            Request::Dispatch {
                mission_id,
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::DeclareChangeIntent {
                        run_id,
                        expected_version: expected_intent_version,
                        spec: ChangeIntentSpec {
                            repository_identity,
                            base_revision,
                            claims,
                        },
                    },
                },
            }
        }
        CliCommand::ChangeIntentAdmit {
            mission_id,
            run_id,
            expected_intent_version,
        } => Request::Dispatch {
            mission_id,
            command: Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::AdmitChangeIntent {
                    run_id,
                    expected_version: expected_intent_version,
                    lease_epoch: 0,
                },
            },
        },
        CliCommand::ChangeIntentPromote {
            mission_id,
            run_id,
            expected_intent_version,
            create_paths,
            modify_paths,
            delete_paths,
        } => {
            let mut claims = Vec::new();
            claims.extend(change_claim_keys(create_paths, ChangeOperation::Create));
            claims.extend(change_claim_keys(modify_paths, ChangeOperation::Modify));
            claims.extend(change_claim_keys(delete_paths, ChangeOperation::Delete));
            Request::Dispatch {
                mission_id,
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::PromoteContingentClaims {
                        run_id,
                        expected_version: expected_intent_version,
                        lease_epoch: 0,
                        claims,
                    },
                },
            }
        }
        CliCommand::VerificationRequire { mission_id, run_id } => Request::Dispatch {
            mission_id,
            command: Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::SetVerificationPolicy {
                    run_id,
                    policy: VerificationPolicy::Independent,
                },
            },
        },
        CliCommand::VerifierCreate {
            mission_id,
            subject_run_id,
            actor,
            engine,
            priority,
        } => Request::Dispatch {
            mission_id,
            command: Command::CreateVerifierRun {
                subject_run_id,
                verifier_run_id: RunId::new(),
                actor: Actor::agent(actor, engine)?,
                priority: priority.into(),
            },
        },
        CliCommand::RetryReturned {
            mission_id,
            source_run_id,
            actor,
            engine,
            priority,
        } => Request::Dispatch {
            mission_id,
            command: Command::RetryReturnedRun {
                source_run_id,
                retry_run_id: RunId::new(),
                actor: Actor::agent(actor, engine)?,
                priority: priority.into(),
            },
        },
        CliCommand::CandidateSubmit {
            mission_id,
            run_id,
            revision,
            content_sha256,
            lease_epoch,
            artifact_ids,
            by,
        } => Request::Dispatch {
            mission_id,
            command: Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::SubmitCandidate {
                    run_id,
                    candidate: RunCandidate {
                        revision,
                        content_sha256,
                        execution_lease_epoch: lease_epoch,
                        realized_changes: None,
                        artifact_ids,
                        submitted_by: ActorId::new(by)?,
                    },
                },
            },
        },
        CliCommand::HandoffRecord {
            mission_id,
            run_id,
            summary,
            completed,
            remaining,
            evidence,
            external_effects,
        } => Request::Dispatch {
            mission_id,
            command: Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::RecordHandoff {
                    handoff: HandoffArtifact {
                        artifact_id: ArtifactId::new(),
                        run_id,
                        summary,
                        completed,
                        remaining,
                        evidence,
                        external_effects,
                    },
                },
            },
        },
        CliCommand::EvaluationRecord {
            mission_id,
            subject_run_id,
            verifier_run_id,
            candidate_sha256,
            verdict,
            passed_checks,
            failed_checks,
            delivery_validated,
            repeatable,
            summary,
        } => {
            let mut checks = passed_checks
                .into_iter()
                .map(|name| EvaluationCheck {
                    name,
                    passed: true,
                    evidence: Vec::new(),
                })
                .collect::<Vec<_>>();
            checks.extend(failed_checks.into_iter().map(|name| EvaluationCheck {
                name,
                passed: false,
                evidence: Vec::new(),
            }));
            Request::Dispatch {
                mission_id,
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::RecordEvaluationReceipt {
                        receipt: EvaluationReceipt {
                            artifact_id: ArtifactId::new(),
                            subject_run_id,
                            verifier_run_id,
                            candidate_sha256,
                            verdict: verdict.into(),
                            checks,
                            delivery_validated,
                            repeatable,
                            summary,
                        },
                    },
                },
            }
        }
        CliCommand::ProviderReport {
            mission_id,
            run_id,
            provider,
            adapter_version,
            state,
            summary,
            valid_for_seconds,
        } => Request::ReportProviderFact {
            mission_id,
            run_id,
            fact: ProviderFactInput {
                provider_id: provider,
                adapter_version,
                state: state.into(),
                summary,
                valid_for_seconds,
            },
        },
        CliCommand::ProviderStatus { mission_id, run_id } => {
            Request::GetRunActivity { mission_id, run_id }
        }
        CliCommand::ProviderList { mission_id } => Request::ListRunActivities { mission_id },
        CliCommand::FaultReport {
            command,
            cwd,
            kind,
            exit_code,
            revision,
            summary,
            output,
            session_id,
            mission_id,
            run_id,
        } => {
            let cwd = match cwd {
                Some(cwd) => cwd,
                None => std::env::current_dir()
                    .map_err(|error| CliError::Usage(format!("working directory: {error}")))?,
            };
            let output = match output {
                Some(output) => output,
                None => read_stdin_output()?,
            };
            Request::ReportFault {
                fault: FaultInput {
                    kind: kind.into(),
                    summary: summary.unwrap_or_else(|| command.clone()),
                    command,
                    cwd,
                    exit_code,
                    revision,
                    output,
                    session_id,
                    mission_id,
                    run_id,
                },
            }
        }
        CliCommand::FaultList {
            mission_id,
            session_id,
            all,
        } => Request::ListFaults {
            mission_id,
            session_id,
            include_closed: all,
        },
        CliCommand::FaultShow { fault_id } | CliCommand::FaultHandoff { fault_id } => {
            Request::GetFault { fault_id }
        }
        CliCommand::FaultRepro {
            fault_id,
            timeout_seconds,
        } => Request::ReproduceFault {
            fault_id,
            timeout_seconds,
        },
        CliCommand::FaultGuard {
            limit,
            timeout_seconds,
        } => Request::GuardFaults {
            limit,
            timeout_seconds,
        },
        CliCommand::FaultResolve { fault_id, note } => Request::ResolveFault { fault_id, note },
        CliCommand::FaultDismiss { fault_id, note } => Request::DismissFault { fault_id, note },
        CliCommand::EvidenceCheck {
            mission_id,
            run_id,
            provider,
            adapter_version,
            key,
            revision,
            name,
            state,
            summary,
            url,
        } => Request::ReportRunEvidence {
            mission_id,
            run_id,
            evidence: RunEvidenceInput {
                provider_id: provider,
                adapter_version,
                evidence_key: key,
                revision,
                summary,
                url,
                kind: RunEvidenceKind::Check {
                    name,
                    state: state.into(),
                },
            },
        },
        CliCommand::EvidenceReview {
            mission_id,
            run_id,
            provider,
            adapter_version,
            key,
            revision,
            title,
            state,
            summary,
            url,
        } => Request::ReportRunEvidence {
            mission_id,
            run_id,
            evidence: RunEvidenceInput {
                provider_id: provider,
                adapter_version,
                evidence_key: key,
                revision,
                summary,
                url,
                kind: RunEvidenceKind::ChangeRequest {
                    title,
                    state: state.into(),
                },
            },
        },
        CliCommand::EvidenceList { mission_id, run_id } => {
            Request::ListRunEvidence { mission_id, run_id }
        }
        CliCommand::Pause {
            mission_id,
            run_id,
            reason,
        } => Request::Dispatch {
            mission_id,
            command: Command::PauseRun {
                run_id,
                reason: reason.into(),
            },
        },
        CliCommand::Resume { mission_id, run_id } => Request::Dispatch {
            mission_id,
            command: Command::ResumeRun { run_id },
        },
        CliCommand::Cancel {
            mission_id,
            run_id,
            summary,
        } => Request::Dispatch {
            mission_id,
            command: Command::CancelRun { run_id, summary },
        },
        CliCommand::Accept {
            mission_id,
            run_id,
            by,
            note,
        } => Request::Dispatch {
            mission_id,
            command: Command::AcceptRunResult {
                run_id,
                by: ActorId::new(by)?,
                note,
            },
        },
        CliCommand::Reject {
            mission_id,
            run_id,
            by,
            note,
        } => Request::Dispatch {
            mission_id,
            command: Command::RejectRunResult {
                run_id,
                by: ActorId::new(by)?,
                note,
            },
        },
        CliCommand::Ask {
            mission_id,
            run_id,
            question,
        } => signal_request(mission_id, run_id, SignalKind::InputNeeded { question }),
        CliCommand::Approval {
            mission_id,
            run_id,
            operation,
            risk,
        } => signal_request(
            mission_id,
            run_id,
            SignalKind::ApprovalNeeded {
                operation,
                risk: risk.into(),
            },
        ),
        CliCommand::Approve {
            mission_id,
            signal_id,
            by,
            scope,
            uses,
        } => Request::Dispatch {
            mission_id,
            command: Command::DecideApproval {
                signal_id,
                grant_id: Some(GrantId::new()),
                by: ActorId::new(by)?,
                allow: true,
                resource_scope: scope,
                use_count: uses,
                enforcement: GrantEnforcement::Cooperative,
            },
        },
        CliCommand::Deny {
            mission_id,
            signal_id,
            by,
        } => Request::Dispatch {
            mission_id,
            command: Command::DecideApproval {
                signal_id,
                grant_id: None,
                by: ActorId::new(by)?,
                allow: false,
                resource_scope: String::new(),
                use_count: 1,
                enforcement: GrantEnforcement::Cooperative,
            },
        },
        CliCommand::GrantRevoke {
            mission_id,
            grant_id,
            reason,
            by,
        } => Request::Dispatch {
            mission_id,
            command: Command::RevokeGrant {
                grant_id,
                by: ActorId::new(by)?,
                reason,
            },
        },
        CliCommand::Blocked {
            mission_id,
            run_id,
            reason,
        } => signal_request(mission_id, run_id, SignalKind::Blocked { reason }),
        CliCommand::Escalate {
            mission_id,
            run_id,
            conflict,
            requested_resolution,
            evidence,
        } => signal_request(
            mission_id,
            run_id,
            SignalKind::Escalation {
                conflict,
                evidence,
                requested_resolution,
            },
        ),
        CliCommand::Artifact {
            mission_id,
            run_id,
            name,
            locator,
            media_type,
            digest,
        } => Request::Dispatch {
            mission_id,
            command: Command::RecordArtifact {
                artifact_id: ArtifactId::new(),
                run_id,
                name,
                media_type,
                locator,
                digest,
            },
        },
        CliCommand::Resolve {
            mission_id,
            signal_id,
            response,
            by,
        } => Request::Dispatch {
            mission_id,
            command: Command::ResolveSignal {
                signal_id,
                by: ActorId::new(by)?,
                response,
            },
        },
        CliCommand::SessionStart {
            mission_id,
            name,
            actor,
            engine,
        } => Request::Dispatch {
            mission_id,
            command: Command::StartSession {
                session_id: SessionId::new(),
                name,
                started_by: match engine {
                    Some(engine) => Actor::agent(actor, engine)?,
                    None => Actor::human(actor)?,
                },
            },
        },
        CliCommand::SessionAssign {
            mission_id,
            session_id,
            run_id,
        } => Request::Dispatch {
            mission_id,
            command: Command::AssignSession { session_id, run_id },
        },
        CliCommand::SessionFinish {
            mission_id,
            session_id,
            exit_code,
        } => Request::Dispatch {
            mission_id,
            command: Command::FinishSession {
                session_id,
                exit_code,
            },
        },
        CliCommand::Take {
            mission_id,
            session_id,
            actor,
        } => Request::Dispatch {
            mission_id,
            command: Command::TakeControl {
                session_id,
                human: Actor::human(actor)?,
            },
        },
        CliCommand::Return {
            mission_id,
            session_id,
            actor,
            to_agent,
            engine,
        } => Request::Dispatch {
            mission_id,
            command: Command::ReturnControl {
                session_id,
                human_id: ActorId::new(actor)?,
                controller: Actor::agent(to_agent, engine)?,
            },
        },
        CliCommand::Finish {
            mission_id,
            run_id,
            summary,
            outcome,
        } => Request::Dispatch {
            mission_id,
            command: Command::FinishRun {
                run_id,
                outcome: outcome.into(),
                summary,
            },
        },
        CliCommand::Complete { mission_id } => Request::Dispatch {
            mission_id,
            command: Command::CompleteMission,
        },
        CliCommand::Abandon { mission_id } => Request::Dispatch {
            mission_id,
            command: Command::AbandonMission,
        },
        CliCommand::Show { mission_id } => Request::GetMission { mission_id },
        CliCommand::History {
            mission_id,
            before_sequence,
            limit,
        } => Request::MissionHistory {
            mission_id,
            before_sequence,
            limit,
        },
        CliCommand::Schedule {
            mission_id,
            max_concurrency,
        } => Request::SchedulerPlan {
            mission_id,
            max_concurrency,
        },
        CliCommand::ScheduleEngineLaunch {
            mission_id,
            max_concurrency,
            session_name_prefix,
            cwd,
            columns,
            rows,
        } => Request::LaunchConfiguredSchedulerBatch {
            mission_id,
            max_concurrency,
            session_name_prefix,
            cwd: match cwd {
                Some(cwd) => cwd,
                None => std::env::current_dir().map_err(CliError::CurrentDirectory)?,
            },
            grid: GridSize::new(columns, rows)?,
        },
        CliCommand::ScheduleAuto {
            mission_id,
            disable,
            max_concurrency,
            session_name_prefix,
            cwd,
            columns,
            rows,
        } => Request::SetSchedulerPolicy {
            policy: SchedulerPolicy {
                mission_id,
                enabled: !disable,
                max_concurrency,
                session_name_prefix,
                cwd: match cwd {
                    Some(cwd) => cwd,
                    None => std::env::current_dir().map_err(CliError::CurrentDirectory)?,
                },
                grid: GridSize::new(columns, rows)?,
            },
        },
        CliCommand::ScheduleAutoList => Request::ListSchedulerPolicies,
        CliCommand::ScheduleSettings {
            global_max_concurrency,
        } => Request::SetSchedulerSettings {
            settings: SchedulerSettings {
                global_max_concurrency,
            },
        },
        CliCommand::ScheduleSettingsShow => Request::GetSchedulerSettings,
        CliCommand::ShareCreate {
            label,
            role,
            mission_ids,
            session_ids,
            expires_in_seconds,
        } => Request::CreateShare {
            label,
            role: role.into(),
            mission_ids,
            session_ids,
            expires_in_seconds,
        },
        CliCommand::ShareList => Request::ListShares,
        CliCommand::ShareRevoke { share_id } => Request::RevokeShare { share_id },
        CliCommand::ScheduleLaunch { .. } => {
            unreachable!("scheduled launches are executed as a multi-request CLI operation")
        }
        CliCommand::RemoteForward { .. } => {
            unreachable!("remote forwarding is executed before protocol connection")
        }
        CliCommand::Events { .. } => {
            unreachable!("event streaming is executed before one-shot protocol conversion")
        }
        CliCommand::VerificationExecute { .. }
        | CliCommand::VerificationStatus { .. }
        | CliCommand::VerificationList { .. }
        | CliCommand::VerificationPrepare { .. }
        | CliCommand::VerificationLaunch { .. }
        | CliCommand::VerificationCollect { .. }
        | CliCommand::VerificationPlanRust { .. }
        | CliCommand::VerificationRustCheck { .. } => {
            unreachable!("verification commands run before one-shot protocol conversion")
        }
        CliCommand::List => Request::ListMissions,
        CliCommand::Status => Request::RuntimeDiagnostics,
        CliCommand::PluginList => Request::ListPlugins,
        CliCommand::ShellInit { .. } => {
            unreachable!("shell integration is handled before connecting to the runtime")
        }
        CliCommand::FaultFix { .. } => {
            unreachable!("fault hand-off runs its own request sequence")
        }
        CliCommand::PluginInstallAgentStatus { .. } => {
            unreachable!("plugin installation is handled before connecting to the runtime")
        }
        CliCommand::Ping => Request::Ping,
    };
    Ok(request)
}

fn read_share_token(path: &Path) -> Result<String, CliError> {
    ultraplexr_client::read_share_token_file(path)
        .map_err(|_| CliError::InvalidShareTokenFile(path.to_owned()))
}

fn validate_ssh_destination(value: &str) -> Result<String, String> {
    let valid = !value.is_empty()
        && !value.starts_with('-')
        && value.len() <= 512
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'-' | b'_' | b'@' | b':' | b'%' | b'[' | b']')
        });
    valid
        .then(|| value.to_owned())
        .ok_or_else(|| "SSH destination contains unsafe or unsupported characters".to_owned())
}

const fn validate_optional_ssh_port(port: Option<u16>) -> Result<(), CliError> {
    if matches!(port, Some(0)) {
        Err(CliError::InvalidSshPort)
    } else {
        Ok(())
    }
}

fn signal_request(mission_id: MissionId, run_id: RunId, kind: SignalKind) -> Request {
    Request::Dispatch {
        mission_id,
        command: Command::RaiseSignal {
            signal_id: SignalId::new(),
            run_id,
            kind,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_destinations_accept_hosts_and_reject_option_or_command_injection() {
        for valid in ["host", "user@example.com", "[2001:db8::1]", "jump%route"] {
            assert_eq!(validate_ssh_destination(valid).as_deref(), Ok(valid));
        }
        for invalid in [
            "",
            "-oProxyCommand=bad",
            "host command",
            "host;bad",
            "host/path",
        ] {
            assert!(validate_ssh_destination(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn ssh_session_uses_argument_boundaries_without_a_remote_command() {
        let request = into_request(CliCommand::TerminalSsh {
            destination: "agent@example.com".to_owned(),
            port: Some(2222),
            identity: Some(PathBuf::from("/tmp/test key")),
            jump: Some("jump@example.com".to_owned()),
            columns: 100,
            rows: 30,
        })
        .expect("SSH request should be valid");
        let Request::StartTerminal { spec } = request else {
            panic!("SSH command must start a terminal");
        };
        assert_eq!(spec.program, PathBuf::from("ssh"));
        assert_eq!(
            spec.args,
            [
                "-p",
                "2222",
                "-i",
                "/tmp/test key",
                "-J",
                "jump@example.com",
                "--",
                "agent@example.com"
            ]
        );
    }

    #[test]
    fn session_names_stay_short_without_splitting_characters() {
        assert_eq!(short_command("cargo test -p thing --all"), "cargo test -p");
        assert_eq!(short_command("ls"), "ls");
        let wide = short_command(&"é".repeat(60));
        assert!(wide.len() <= 44, "{}", wide.len());
        assert!(wide.ends_with('…'));
    }

    #[test]
    fn fault_briefs_carry_replay_state_and_the_failing_output() {
        let fault_id = FaultId::new();
        let base = ultraplexr_protocol::FaultSummary {
            fault_id,
            kind: FaultKind::TestFailed,
            command: "cargo test -p thing".to_owned(),
            cwd: PathBuf::from("/work/thing"),
            exit_code: Some(101),
            revision: Some("deadbeef".to_owned()),
            summary: "2 tests failed".to_owned(),
            output: "assertion failed: left == right\n".to_owned(),
            session_id: None,
            mission_id: None,
            run_id: None,
            source: ultraplexr_protocol::FaultSource::TerminalExit,
            observed_at_unix_micros: 1,
            state: ultraplexr_protocol::FaultState::Open,
            repro: None,
            repro_attempts: 0,
            fix_run_id: None,
            proof: None,
            regressions: 0,
        };

        let never = fault_brief(&base);
        assert!(never.contains("cargo test -p thing"));
        assert!(never.contains("/work/thing"));
        assert!(never.contains("exit:    101"));
        assert!(never.contains("rev:     deadbeef"));
        assert!(never.contains("state:   open"));
        assert!(never.contains("never replayed"));
        assert!(never.contains("assertion failed: left == right"));
        assert!(never.contains(&format!("ultraplexr fault repro {fault_id}")));

        let failing = ultraplexr_protocol::FaultSummary {
            repro: Some(ultraplexr_protocol::ReproReceipt {
                attempted_at_unix_micros: 2,
                reproduced: true,
                exit_code: Some(101),
                output: "same failure".to_owned(),
                duration_ms: 4_200,
                revision: None,
                error: None,
            }),
            repro_attempts: 1,
            ..base.clone()
        };
        assert!(fault_brief(&failing).contains("still fails · exit 101 after 4200 ms"));

        let passing = ultraplexr_protocol::FaultSummary {
            repro: Some(ultraplexr_protocol::ReproReceipt {
                reproduced: false,
                error: None,
                duration_ms: 900,
                ..failing.repro.clone().expect("receipt")
            }),
            repro_attempts: 2,
            state: ultraplexr_protocol::FaultState::Resolved {
                note: "fixed in run 7".to_owned(),
                at_unix_micros: 3,
            },
            ..base.clone()
        };
        let resolved = fault_brief(&passing);
        assert!(resolved.contains("passes now · after 900 ms"));
        assert!(resolved.contains("resolved · fixed in run 7"));

        let broken = ultraplexr_protocol::FaultSummary {
            repro: Some(ultraplexr_protocol::ReproReceipt {
                reproduced: false,
                error: Some("directory is unavailable".to_owned()),
                ..failing.repro.clone().expect("receipt")
            }),
            repro_attempts: 1,
            ..base
        };
        assert!(fault_brief(&broken).contains("could not run · directory is unavailable"));
    }

    #[test]
    fn provider_reports_preserve_structured_provenance_fields() {
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let request = into_request(CliCommand::ProviderReport {
            mission_id,
            run_id,
            provider: "openai-codex".to_owned(),
            adapter_version: "2026.09".to_owned(),
            state: ProviderStateArg::WaitingApproval,
            summary: "waiting for tool approval".to_owned(),
            valid_for_seconds: 45,
        })
        .expect("provider report should convert");
        let Request::ReportProviderFact {
            mission_id: actual_mission,
            run_id: actual_run,
            fact,
        } = request
        else {
            panic!("provider report should stay structured");
        };
        assert_eq!(actual_mission, mission_id);
        assert_eq!(actual_run, run_id);
        assert_eq!(fact.provider_id, "openai-codex");
        assert_eq!(fact.adapter_version, "2026.09");
        assert_eq!(fact.state, ProviderActivityState::WaitingApproval);
        assert_eq!(fact.valid_for_seconds, 45);
    }

    #[test]
    fn evidence_reports_preserve_revision_and_provider_neutral_state() {
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let request = into_request(CliCommand::EvidenceCheck {
            mission_id,
            run_id,
            provider: "github".to_owned(),
            adapter_version: "1".to_owned(),
            key: "check/test".to_owned(),
            revision: "abc123".to_owned(),
            name: "test".to_owned(),
            state: CheckEvidenceStateArg::Failed,
            summary: "one test failed".to_owned(),
            url: Some("https://example.invalid/check/1".to_owned()),
        })
        .expect("evidence report should convert");
        let Request::ReportRunEvidence {
            mission_id: actual_mission,
            run_id: actual_run,
            evidence,
        } = request
        else {
            panic!("evidence report should stay structured");
        };
        assert_eq!(actual_mission, mission_id);
        assert_eq!(actual_run, run_id);
        assert_eq!(evidence.provider_id, "github");
        assert_eq!(evidence.evidence_key, "check/test");
        assert_eq!(evidence.revision, "abc123");
        assert!(matches!(
            evidence.kind,
            RunEvidenceKind::Check {
                state: CheckEvidenceState::Failed,
                ..
            }
        ));
    }

    #[test]
    fn change_intent_cli_preserves_exact_scope_and_uses_server_epoch_placeholder() {
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let request = into_request(CliCommand::ChangeIntentDeclare {
            mission_id,
            run_id,
            repository_identity: "repo-identity".to_owned(),
            base_revision: "abc123".to_owned(),
            expected_intent_version: Some(4),
            create_paths: vec!["src/new.rs".to_owned()],
            modify_paths: vec!["src/main.rs".to_owned()],
            delete_paths: Vec::new(),
            contingent_create_paths: Vec::new(),
            contingent_modify_paths: vec!["docs/notes.md".to_owned()],
            contingent_delete_paths: Vec::new(),
        })
        .expect("intent request should convert");
        let Request::Dispatch {
            mission_id: actual_mission,
            command:
                Command::VerifiedDelivery {
                    command:
                        VerifiedDeliveryCommand::DeclareChangeIntent {
                            run_id: actual_run,
                            expected_version,
                            spec,
                        },
                },
        } = request
        else {
            panic!("intent declaration should remain structured");
        };
        assert_eq!(actual_mission, mission_id);
        assert_eq!(actual_run, run_id);
        assert_eq!(expected_version, Some(4));
        assert_eq!(spec.claims.len(), 3);
        assert!(spec.claims.iter().any(|claim| {
            claim.path == "src/main.rs"
                && claim.operation == ChangeOperation::Modify
                && claim.scope == ChangeScope::Committed
        }));
        assert!(spec.claims.iter().any(|claim| {
            claim.path == "docs/notes.md" && claim.scope == ChangeScope::Contingent
        }));

        let admission = into_request(CliCommand::ChangeIntentAdmit {
            mission_id,
            run_id,
            expected_intent_version: 5,
        })
        .expect("admission request should convert");
        assert!(matches!(
            admission,
            Request::Dispatch {
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::AdmitChangeIntent {
                        run_id: actual_run,
                        expected_version: 5,
                        lease_epoch: 0,
                    },
                },
                ..
            } if actual_run == run_id
        ));

        let promotion = into_request(CliCommand::ChangeIntentPromote {
            mission_id,
            run_id,
            expected_intent_version: 5,
            create_paths: vec!["docs/new.md".to_owned()],
            modify_paths: Vec::new(),
            delete_paths: Vec::new(),
        })
        .expect("promotion request should convert");
        assert!(matches!(
            promotion,
            Request::Dispatch {
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::PromoteContingentClaims {
                        run_id: actual_run,
                        expected_version: 5,
                        lease_epoch: 0,
                        claims,
                    },
                },
                ..
            } if actual_run == run_id
                && claims == vec![ChangeClaimKey {
                    path: "docs/new.md".to_owned(),
                    operation: ChangeOperation::Create,
                }]
        ));
    }

    #[test]
    fn evaluation_cli_keeps_pass_and_failure_checks_structured() {
        let mission_id = MissionId::new();
        let subject_run_id = RunId::new();
        let verifier_run_id = RunId::new();
        let digest = "a".repeat(64);
        let request = into_request(CliCommand::EvaluationRecord {
            mission_id,
            subject_run_id,
            verifier_run_id,
            candidate_sha256: digest.clone(),
            verdict: EvaluationVerdictArg::Failed,
            passed_checks: vec!["format".to_owned()],
            failed_checks: vec!["test".to_owned()],
            delivery_validated: true,
            repeatable: false,
            summary: "test failed".to_owned(),
        })
        .expect("evaluation request should convert");
        let Request::Dispatch {
            command:
                Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::RecordEvaluationReceipt { receipt },
                },
            ..
        } = request
        else {
            panic!("evaluation should remain structured");
        };
        assert_eq!(receipt.subject_run_id, subject_run_id);
        assert_eq!(receipt.verifier_run_id, verifier_run_id);
        assert_eq!(receipt.candidate_sha256, digest);
        assert_eq!(receipt.verdict, EvaluationVerdict::Failed);
        assert!(
            receipt
                .checks
                .iter()
                .any(|check| check.name == "format" && check.passed)
        );
        assert!(
            receipt
                .checks
                .iter()
                .any(|check| check.name == "test" && !check.passed)
        );
    }

    #[test]
    fn delivery_follow_up_cli_uses_structured_verifier_and_retry_commands() {
        let mission_id = MissionId::new();
        let source_run_id = RunId::new();
        let verifier = into_request(CliCommand::VerifierCreate {
            mission_id,
            subject_run_id: source_run_id,
            actor: "reviewer".to_owned(),
            engine: "codex".to_owned(),
            priority: PriorityArg::Urgent,
        })
        .expect("verifier request should convert");
        assert!(matches!(
            verifier,
            Request::Dispatch {
                command: Command::CreateVerifierRun {
                    subject_run_id,
                    priority: RunPriority::Urgent,
                    ..
                },
                ..
            } if subject_run_id == source_run_id
        ));

        let retry = into_request(CliCommand::RetryReturned {
            mission_id,
            source_run_id,
            actor: "retry".to_owned(),
            engine: "codex".to_owned(),
            priority: PriorityArg::Urgent,
        })
        .expect("retry request should convert");
        assert!(matches!(
            retry,
            Request::Dispatch {
                command: Command::RetryReturnedRun {
                    source_run_id: actual_source,
                    priority: RunPriority::Urgent,
                    ..
                },
                ..
            } if actual_source == source_run_id
        ));
    }

    #[test]
    fn share_token_files_must_be_owner_only_and_are_trimmed() {
        let root = std::env::temp_dir().join(format!("ultraplexr-token-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).expect("token fixture should exist");
        let path = root.join("observer.token");
        std::fs::write(&path, "t9s_fixture_secret\n").expect("token fixture should write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("secure permissions should apply");
        assert_eq!(
            read_share_token(&path).expect("secure token should read"),
            "t9s_fixture_secret"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("insecure permissions should apply");
        assert!(matches!(
            read_share_token(&path),
            Err(CliError::InvalidShareTokenFile(_))
        ));
        std::fs::remove_dir_all(root).expect("isolated token fixture should be removable");
    }
}
