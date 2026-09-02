# arXiv agent-systems review — 2026-09-01

**Status:** research note

**Reviewed:** 2026-09-01

**Sources:** [arXiv Computer Science new listings](https://arxiv.org/list/cs/new)
and broader arXiv searches for agent harnesses, runtimes, workspaces, protocols,
parallel coding, cloud sandboxes, agentic IDEs, and computer-use infrastructure.

**Purpose:** identify recent research that should influence termi9ne's design as
a durable, human-supervised execution environment for coding agents.

The consolidated implementation status and priority plan derived from this
research now lives in
[termi9ne-unified-roadmap.md](termi9ne-unified-roadmap.md).

## 1. Scope and method

The reviewed listing contained 2,441 entries:

- 1,327 new submissions;
- 104 cross-lists;
- 1,010 replacements of previously submitted papers.

Both listing pages were screened for work on agent harnesses, coding agents,
long-running execution, orchestration, tool use, context and memory, recovery,
provenance, evaluation, security, and human supervision. Fourteen papers were
then inspected at abstract and full-text level, including results, conclusions,
and limitations where the HTML version exposed them.

Selection was based on transferable systems ideas, not title similarity. Papers
about generic model training, domain-specific agents, robotics, and benchmarks
without a clear runtime implication were excluded. Relevant replacements were
included because the `cs/new` feed surfaces materially updated papers alongside
new submissions.

The papers are preprints. Their reported results are useful design evidence, not
settled fact. Numbers below are attributed to the papers and should be
revalidated before being used in product or security claims.

A second pass searched 616 unique arXiv records across nine broad queries, then
ran exact searches for agentic IDEs, parallel coding agents, workspace runtimes,
interoperability protocols, and agent sandboxes. The phrase “agent multiplexer”
returned no papers; the relevant research vocabulary is *harness*, *runtime*,
*workspace delegation*, *parallel coding*, *agent-computer interface*, and
*sandbox infrastructure*.

Full-text sections in the second pass were scraped with
[Wigolo](https://github.com/KnockOutEZ/wigolo) using its HTTP-first headless
`fetch` path with automatic browser escalation. The returned evidence included
the source URL, retrieval method, content hash, section heading, citation ID,
and source span. The paper links below remain the durable public citations.

## 2. Executive conclusions

| Finding | Research signal | Consequence for termi9ne |
|---|---|---|
| The harness is part of the system under evaluation. | DS-Lighting shows that task representation, workflow, execution, and evaluation choices affect reliability and comparability. | Snapshot and identify the complete harness contract for every Run, not only the model and command. |
| Agents need a legitimate way to reject a defective task. | A structured escalation tool plus policy reduced reward hacking from 23.6% to 5.3% in the reported experiment. | Add an explicit escalation Signal with evidence, resolution, and continuation semantics. |
| Repository state is not an adequate handoff. | Structured or trace-bearing handoffs reduced takeover effort and prompt use in Handoff Debt. | Make a compact, provenance-linked Handoff Artifact part of pause, replacement, and review workflows. |
| Memory cannot be managed as one token pool. | Coding-agent instructions, artifacts, tool outputs, and generated state have different retention and delivery behavior. | Track stored state, delivered context, management work, and task outcome separately. |
| Correct side effects do not guarantee correct completion. | In APIFlow-Bench, 77% of failing clean-slice Runs had reached the correct final state but failed final delivery. | Validate both execution effects and the final receipt presented to the owner. |
| A restored checkpoint may be unsafe to resume. | Safe to Resume identifies five ways rollback can violate execution continuity. | Resume requires a Recovery Manifest and reconciliation of external assumptions and effects. |
| Failure diagnosis should identify the point of no recovery. | AgentRx localizes critical failure steps using auditable constraint violations. | Derive evidence-backed Failure Diagnosis Artifacts without rewriting source events. |
| Cross-deployer delegation needs stronger evidence than identity. | Output signatures and co-signed delegation edges answer who released bytes and who authorized an edge. | Use content digests now; reserve co-signed ancestry for a future multi-owner fleet. |
| Self-reported early confidence is a weak attention signal. | In the studied long-horizon tasks, no tested uncertainty metric exceeded mean AUROC 0.60 at 50% progress. | Continue deriving attention from observable state, deadlines, policy, and explicit Signals. |
| Long-horizon attacks compose across prompts and tools. | Repository poisoning and ECLIPSE show that task wording, tool descriptions, and delayed actions jointly shape risk. | Label trust per context source and enforce policy before effects, not only at prompt ingress. |

The papers strengthen the existing architecture rather than argue for a new
foundation. Mission, Run, Session, Signal, Artifact, capability, worktree, and
event-history primitives are suitable places to record the additional facts.

## 3. Priority paper briefs

### 3.1 DS-Lighting: make the harness explicit

[DS-Lighting: Making Agent Harnesses Explicit for Data-Science Automation](https://arxiv.org/abs/2608.28590)
by Fan Liu and Hao Liu, arXiv:2608.28590v1.

The paper decomposes an agent harness into four layers:

1. data and task representation;
2. workflow and operator program;
3. execution environment;
4. evaluation protocol.

It places heterogeneous agents behind a common task interface, sandboxed
runtime, and metric protocol. The authors report that making these layers
explicit improves reproducibility and reduces avoidable execution and
evaluation failures. Their limitations are important: the evaluation is in
data-science benchmarks and does not cover private systems, streaming work,
human review, or domain governance; dependency and sandbox versions can still
change outcomes.

**Implication for termi9ne:** extend the existing resolved Driver snapshot into
a complete, digest-backed Harness Snapshot:

```text
HarnessSnapshot
├── task contract and context sources
├── Mission workflow/template version
├── Driver, model, tool, and skill versions
├── execution and sandbox policy
└── evaluator and acceptance-policy version
```

The snapshot should describe how a Run was asked to work and how its result was
judged. Evaluation remains separate from execution: an agent's successful exit
or self-report is not owner acceptance.

### 3.2 Escalation channels: give agents a safe alternative

[Can escalation channels redirect reward hacking toward defect disclosure?](https://arxiv.org/abs/2608.29460)
by Francesca Gomez, arXiv:2608.29460v1.

The experiment studies coding agents facing defective tests. A structured
escalation tool, especially when paired with explicit policy, redirected agents
away from hard-coding answers or editing tests. Across eight models from five
families, the combined intervention reportedly reduced reward hacking from
23.6% to 5.3%, eliminated it for six models, and added 10.1 percentage points of
defect-detection coverage over monitoring alone. In 96.8% of escalation events,
the agent did not subsequently hack.

The result does not show that a tool button alone is sufficient. Models could
already describe a defect in their answer and generally did not; framing and
placement in the decision environment affected uptake. The study covers only
nine ambiguous competitive-programming problems, and its prompts combine tool
availability with normative guidance.

**Implication for termi9ne:** add a first-class `escalation` Signal rather than
expecting prose in a terminal:

```text
EscalationSignal
├── defect or conflict kind
├── blocked objective/action
├── evidence Artifact references
├── rejected unsafe shortcut, if any
├── proposed alternatives
└── requested owner decision
```

An escalation raises Attention, suspends only the affected work, and can resolve
to amend, retry, waive, replace, or stop. The Driver contract should teach agents
when and how to use it. Escalation complements sandboxing, permissions, and
monitoring; it does not replace them.

### 3.3 Handoff Debt: optimize resumability, not only completion

[Handoff Debt: The Rediscovery Cost When Coding Agents Take Over Interrupted Tasks](https://arxiv.org/abs/2606.02875)
by Dipesh KC and Anjila Budathoki, arXiv:2606.02875v2.

The study interrupts coding agents at deterministic points and gives successors
one of four views: repository state, raw trace, summary notes, or structured
notes. Across three successor models, context-bearing handoffs reduced median
agent events by 20–59% and cumulative prompt tokens by 42–63% compared with
repository state alone. Effects on solved rate were smaller and model-dependent.

Raw traces carry information but grow without bound. Compact handoff artifacts
were the practical middle ground. The study uses one OpenHands-style runtime and
75 SWE-bench Verified source tasks, so absolute costs should not be generalized.

**Implication for termi9ne:** every paused, replaced, timed-out, or voluntarily
yielded Run should be able to produce a Handoff Artifact:

```text
HandoffArtifact
├── objective and current interpretation
├── completed changes and checkpoints
├── tests and other evidence
├── failed approaches and why
├── unresolved questions and risks
├── external effects already performed
├── recommended next action
└── links to source events, files, commits, and Signals
```

The Artifact is a bounded index into durable evidence, not a replacement for the
event history. A takeover benchmark should measure rediscovery events, delivered
tokens, wall time, and correctness.

### 3.4 Safe to Resume: recovery is a security boundary

[Safe to Resume? Breaking Execution Continuity of Agent Execution via Rollback](https://arxiv.org/abs/2608.29381)
by Guanlong Wu et al., arXiv:2608.29381v1.

The paper distinguishes a correctly restored checkpoint from a secure
continuation. It identifies five recurring failure modes:

1. incomplete internal-state coverage;
2. inconsistent checkpoint state;
3. external-state mismatch;
4. unbound nondeterministic replay;
5. unrecorded external effects.

The authors demonstrate malware-verification bypass, unauthorized mail
forwarding, and duplicate payment across Hermes, Cline, and LangGraph examples.
The general lesson is that internal state, workspace state, tool state, external
services, and persistent effects may have different recovery boundaries.

**Implication for termi9ne:** restored PTY bytes and event history are necessary
but do not prove that an agent can safely continue. Any future semantic
checkpoint/rollback feature needs a Recovery Manifest:

```text
RecoveryManifest
├── Run, Session, process, and worktree state digests
├── Driver and capability epoch
├── external reads and version assumptions
├── recorded external effects and idempotency keys
├── outstanding leases and approvals
├── nondeterministic inputs or replay boundary
└── reconciliation result
```

If continuity cannot be established, termi9ne should create a new Attempt with
the prior state as evidence or require owner approval. It must not label a local
restore as a transparent resume.

### 3.5 Measure Before You Manage: memory has semantic object types

[Measure Before You Manage: Evaluating Agent Working Memory in Coding Agents](https://arxiv.org/abs/2608.31057)
by Le Chen et al., arXiv:2608.31057v1.

Across 55 archived coding-agent trajectories, the paper finds different size,
residency, retention, and representation behavior for instructions, artifacts,
tool outputs, and agent-generated state. Equal nominal token caps did not imply
equal delivered context or equal management cost. The proposed evaluation model
has four levels:

1. stored state;
2. delivered context;
3. management work;
4. task or process outcome.

The evidence base is small and repository-clustered, and some lifecycle signals
in the studied archive were defective. The value lies primarily in the
measurement model, not in a proven universal compression policy.

**Implication for termi9ne:** context assembly should operate on typed objects
and record what was actually delivered. A Run's context receipt should include
object IDs, source versions, full-versus-summary form, delivered size, omissions,
retrieval/compression cost, and the policy that selected them. Raw Mission
history and terminal journals remain durable even when provider context is
compressed.

### 3.6 APIFlow-Bench: certify both labor and delivery

[APIFlow-Bench: Measuring Whether Agents Survive Long, Dependent API Workflows](https://arxiv.org/abs/2608.29128)
by Zelin Wan et al., arXiv:2608.29128v1.

APIFlow-Bench evaluates long, dependent REST workflows using deterministic mock
worlds and provenance-sensitive grading. It separates seven capabilities:

- carrying state across calls;
- repairing rejected schemas;
- confirming side effects;
- pagination;
- retry and error recovery;
- refreshing authentication;
- discovering the right resource among look-alikes.

Across the paper's 19-model panel, success fell from 93% on individual subtasks
to 74% on clean 20-subtask chains. More importantly, repeated reliability
separated models far more than best-of-five capability: the reported range was
44 points for five-of-five reliability versus seven points for best-of-five.
On the clean slice, 77% of failed Runs had produced the correct final world state
but failed the final delivery.

The benchmark uses synthetic REST mocks, only eleven headline sampling units,
and a generation/oracle pipeline with possible model-family entanglement.

**Implication for termi9ne:** an Evaluation Receipt should independently record:

- whether required actions were authorized and executed;
- whether resulting state was observed and verified;
- whether the final Artifact came from the required source;
- whether its schema and fields are correct;
- whether the result was delivered to the correct owner/channel;
- repeated-run reliability, not only one successful attempt.

### 3.7 AgentRx: diagnose where recovery became impossible

[AgentRx: Diagnosing AI Agent Failures from Execution Trajectories](https://arxiv.org/abs/2602.02475)
by Shraddha Barke et al., arXiv:2602.02475v2.

AgentRx provides a benchmark of 170 failed trajectories across eleven task
settings. It synthesizes global and dynamic constraints, evaluates them against
each step, writes an auditable violation log, and uses that evidence to identify
the critical failure step and category. The authors report a 75% average
improvement in step localization over prior work.

Surfaced violations can still be false positives, downstream symptoms, or weak
signals that misdirect diagnosis. The taxonomy will not cover every domain.

**Implication for termi9ne:** build failure diagnosis as a derived Artifact over
immutable Run events:

```text
FailureDiagnosis
├── critical event or bounded event range
├── violated invariant/constraint
├── evidence references
├── failure category and confidence
├── downstream symptoms distinguished from cause
└── suggested recovery boundary
```

This should never silently rewrite the Run outcome. Humans and later agents can
accept, dispute, or supersede a diagnosis.

### 3.8 Delegation ancestry and output attestation

[Attesting Outputs and Delegation Ancestry in Multi-Agent AI Systems](https://arxiv.org/abs/2608.30387)
by Lifei Liu and Haoran Yu, arXiv:2608.30387v1.

The paper separates two post-incident questions:

1. which deployer released the reported bytes;
2. whether every cross-deployer delegation edge was authorized.

Its first layer signs an output digest. Its second compares signed linked lists,
Merkle-chain variants, and a co-signed delegation DAG. A child-signed parent
claim is insufficient after child-key compromise; the co-signed DAG requires
the parent to authorize the complete edge manifest.

This is attribution evidence, not prompt-injection prevention. Production key
rotation, registry compromise, endpoint collusion, streaming semantics, and
application-policy enforcement remain out of scope.

**Implication for termi9ne:** use content-addressed Artifacts, Driver snapshots,
and explicit parent/child Run events now. If termi9ne later coordinates workers
owned by different people or organizations, add co-signed delegation edges
rather than trusting child-provided ancestry. Do not add cross-deployer
cryptography to the local-first V1 merely for architectural symmetry.

### 3.9 AgentLogs: preserve useful traces without collecting hidden reasoning

[AgentLogs: A Dataset for Opening the Black Box of GitHub's Cloud Agent](https://arxiv.org/abs/2608.29204)
by Jonan Richards et al., arXiv:2608.29204v1.

AgentLogs contains 307,416 tasks, 549,239 sessions, and 64,255,174 log entries
from 35,810 public repositories. Its schema links task requests, sessions,
models, outcomes, usage, branches, pull requests, edits, Git actions, reviews,
CI, and other tool calls. The dataset makes workflow patterns, task formulation,
review response, costs, failures, and adoption measurable.

**Implication for termi9ne:** maintain a documented, versioned event-export
schema so owners can study their own Runs. Store observable messages, tool
actions, state transitions, costs, Artifacts, and review facts. Do not require or
claim access to a provider's hidden chain of thought. Exports must scrub secrets,
capabilities, raw environment values, and private repository content unless the
owner deliberately includes it.

### 3.10 Repository poisoning depends on how the agent is invoked

[Beyond the Payload: How User Invocation Shapes Coding Agent Vulnerability to Repository Poisoning](https://arxiv.org/abs/2608.30686)
by Fukang Zhu et al., arXiv:2608.30686v1.

CIPR varies task type, prompt style, and skill/rule configuration across 1,920
instances in 20 poisoned repositories. The paper reports up to a 4.5-fold
difference in attack success by task type. Test-execution requests were a silent
attack surface: high attack success with low alerting. Security instructions
could improve alerting without reliably preventing execution, meaning an alert
may arrive after the effect.

The study uses unconstrained automation, focuses on exfiltration, and does not
systematically vary memory, MCP servers, IDEs, or permission modes.

**Implication for termi9ne:** repository instructions, test scripts, tool
descriptions, skills, and user text all need source trust labels. High-risk
actions require a policy gate before execution; emitting an alert afterward is
not enforcement. Test and setup commands deserve stronger pre-execution review
than ordinary reads. Prompt normalization may reduce stylistic variance but is
not a security boundary.

### 3.11 ECLIPSE: attacks can emerge only across a trajectory

[ECLIPSE: Self-Evolving Stealthy Prompt Injection Attack against Long-Horizon Agentic Systems](https://arxiv.org/abs/2608.30441)
by Shiqian Zhao et al., arXiv:2608.30441v1.

ECLIPSE distributes an attack across the initial request, tool descriptions, and
later corrective signals. Its LASE-Bench contains 120 malicious tasks and 198
tools; 96.7% of tasks require at least five calls. The reported attack succeeds
up to 96.7% without defense and 69.2% under a common safety filter. The authors'
core conclusion is that no individual prompt, tool description, or early action
must reveal the full objective.

**Implication for termi9ne:** policy evaluation must be sequence-aware and
effect-aware. Keep provenance for every context fragment and tool result, limit
the capabilities available to each Run phase, and check the cumulative action
plan before privileged effects. A one-time input filter is insufficient.

### 3.12 TRACER: compression has downstream consequences

[TRACER: Per-Tool Context Retention for LLM Agents via Consequence-Attributed Reinforcement Learning](https://arxiv.org/abs/2608.29363)
by Ziqi Lin et al., arXiv:2608.29363v1.

TRACER treats retention as a per-tool decision and penalizes later tool
re-invocations caused by over-compression. On the authors' held-out production
queries, it reportedly reduced total tokens by 29–46% relative to retaining all
context while maintaining comparable or higher success. It transferred with
18–25% savings on five LOCA-bench environments.

**Implication for termi9ne:** do not implement an RL compression policy in the
core now. First record per-object retention, later re-fetch/re-execution,
delivered tokens, latency, and outcome. These facts make future policy evaluation
possible. Never compress away the durable source evidence merely because the
provider received a smaller view.

### 3.13 Agentic skills are a supply-chain object

[Towards a Systems Foundation for Agentic Skills: Architecture, Lifecycle, and Security](https://arxiv.org/abs/2608.29596)
by Sanket Badhe et al., arXiv:2608.29596v1.

This systems survey frames skills as externalized procedural knowledge and
describes a nine-stage lifecycle: discovery, authoring, storage, retrieval,
composition, execution and repair, adaptation, evaluation, and security
governance. It argues that reusable executable skills bridge model planning and
deterministic execution.

**Implication for termi9ne:** a skill is not trusted prompt text. A future Skill
Manifest should include identity, version, content digest, origin, declared
tools and capabilities, parameters, compatible Drivers, evaluation evidence,
and revocation state. Imported executable skills require approval and sandbox
policy. The paper is a broad architecture survey, so individual claims need
primary-source validation before implementation.

### 3.14 Last Step Matters: do not supervise by confidence alone

[Last Step Matters: Early Uncertainty Cannot Predict Failure in Long-Horizon Agents](https://arxiv.org/abs/2608.29685)
by Zongyue Li et al., arXiv:2608.29685v1.

On the studied deep-research tasks, verbal confidence separated failures at
completion with mean AUROC 0.85, but no evaluated uncertainty metric exceeded
mean AUROC 0.60 at 50% trajectory progress. Agents often switched paths, breaking
the relationship between intermediate confidence and final outcome. Confidence
also remained poorly calibrated.

**Implication for termi9ne:** do not infer Attention or terminate a Run from an
agent's intermediate confidence. Prefer expired leases, missing heartbeats,
failed checks, blocked dependencies, permission requests, explicit Signals, and
observable inactivity. Final confidence may help decide whether to launch an
independent verifier or retry; it must not settle the work. The paper's main
evidence is deep-research work and may not transfer unchanged to stateful coding.

## 4. Cutting-edge addendum: runtimes, IDEs, and agent multiplexing

The strongest results from the broader search cluster around a new systems
layer between the model and the ordinary operating system. That layer controls
who may write, how Runs branch, which facts become durable task state, how
untrusted context flows, and when a human must be interrupted.

### 4.1 Most relevant new papers

| Paper | Cutting-edge primitive | Evidence and caution |
|---|---|---|
| [Claim Plane](https://arxiv.org/abs/2607.21909) and its [confirmatory study](https://arxiv.org/abs/2608.00947) | Versioned pre-write `ChangeIntent`; typed resources and regions; committed versus contingent scope; atomic admission; scope promotion; leases, fencing tokens, worktree locks, and patch provenance. | Static admission improved integration reliability but serialized 96.7% of the confirmatory Runs. Dynamic admission was selective but failed closed frequently because declarations under-covered regions. Strong primitive; learned declarations are not mature. |
| [Shepherd](https://arxiv.org/abs/2605.10913) | Agent execution as a first-class value: structured events, atomic environment/agent forks, replay, revert, resume, and meta-agent transformations. | Three proof-of-existence studies report supervision, counterfactual-optimization, and training gains. Strong meta-runtime idea, but supervisor cost can exceed worker cost and replay assumes limited coupling to external effects. |
| [When Parallelism Pays Off](https://arxiv.org/abs/2606.00953) | Static-analysis dependency graph, hub isolation, community partitioning, and dependency-aware release of coding work. | Across 28 tasks the paper reports up to 2.10× speedup, up to 14-point pass-rate gain, and up to 35% cost reduction. Evaluation size is modest and benefits depend on repository structure. |
| [Effective Strategies for Asynchronous Software Engineering Agents](https://arxiv.org/abs/2603.21489) | Centralized, asynchronous, isolated delegation: dependency-aware manager, per-agent worktree/branch, commit-and-merge integration, executable test gates. | Reports absolute gains of 25.6% on PaperBench and 14.7% on Commit0. Direct validation of termi9ne's planned worktree model; benchmark transfer still needs local testing. |
| [LongHorizon-Harness](https://arxiv.org/abs/2608.01964) | Manage–Execute–Audit loop; task state outside model context; fresh-context executor; read-only auditor; only verified facts advance state. | Reports consistent gains on WeaveBench, OSWorld 2.0, and Terminal-Bench 2.1. Clean conceptual fit, though the extra manager/auditor calls add cost. |
| [openJiuwen](https://arxiv.org/abs/2608.27969) | Inner/outer execution loops; ordered capability “Rails”; shared semantics for single, delegated, and swarm agents; Goal Mode; LSP-driven passive feedback; adaptive context management. | Strong benchmark results, but the paper asks for broader models, benchmarks, and mechanism ablations. Adopt its separations, not its branded abstraction wholesale. |
| [Agentic Harness Engineering](https://arxiv.org/abs/2604.25850) | Harness components represented as editable files; layered trajectory evidence; every harness edit paired with a falsifiable outcome prediction. | Ten iterations reportedly lifted Terminal-Bench 2 pass@1 from 69.7% to 77.0% and transferred across models. Evolution should run as reviewed experiments, never live self-modification of trusted policy. |
| [CHILL-Harness](https://arxiv.org/abs/2607.25825) | Learned counterfactual intervention policy decides whether a harness should deliberate, revise, or complete; advantage margin authorizes changes. | Interesting route toward adaptive orchestration. It is complex and learning-dependent; retain as a research direction after deterministic policies are measurable. |
| [Scrouting](https://arxiv.org/abs/2608.04804) | Cheap read-only repository scout creates a structured handoff; sandbox verifies reproduction claims and strips false ones before a fixer sees them; routing occurs after scouting. | Matched the best single fixer on 159/266 versus 158/266 at about one-fifth reported cost per solve. Crucially, its cheapest-fixer ablation tied the router: the verified handoff, not routing intelligence, carried the result. |
| [APPA](https://arxiv.org/abs/2607.24625) | Recoverable information-flow control: prospective tool check, realized-return admission, gradual security typing, policy remedies, and disposable taint-confined child trajectories with schema-bounded output. | Reports zero observed attacks in 1,320 guarded episodes within 6,600 controlled Runs while retaining 64.2–91% utility. Promising but requires a substantial trusted computing base and careful local reproduction. |
| [When Context Gets Root](https://arxiv.org/abs/2608.27299) | Defines instruction privilege escalation caused by harness context reconstruction that promotes tool or agent text into a higher model-facing role. | Thirteen objectives succeeded across six harnesses under full access, including automatic permission review modes. Direct evidence that provenance without role preservation is insufficient. |
| [Overeager Coding Agents](https://arxiv.org/abs/2605.18583) | Treats benign out-of-scope work as an authorization failure; paired prompts, behavioral-gradient validation, and dual-channel action auditing. | Roughly 7,500 Runs across four products; permissive frameworks showed materially more overeager behavior. Current verdicts cover declaratively enumerable, shell-mediated actions. |
| [Crab](https://arxiv.org/abs/2604.28138) | eBPF observes OS-visible effects at agent-turn boundaries; checkpoints only recovery-relevant turns, overlaps checkpoint work with model latency, and schedules traffic across colocated sandboxes. | More than 75% of studied turns had no recovery-relevant state. The paper reports 100% recovery correctness, up to 87% less checkpoint traffic, and runtime within 1.9% of fault-free execution. |
| [DeltaBox](https://arxiv.org/abs/2605.22781) | Diff-based OS checkpoint/restore: dynamically frozen OverlayFS layers plus incremental process dumps and warm-template forks. | Reports 14 ms checkpoints and 5 ms rollbacks. Highly relevant to future speculative Run trees, but it does not solve semantic continuity or external-effect reconciliation. |
| [OSGym](https://arxiv.org/abs/2511.11672) | Decentralized replica state, hardware-aware placement, KVM plus copy-on-write disks, fault-tolerant pools, and a central task/data interface. | Reports more than 1,000 OS replicas, 1,420 trajectories/minute, 88% disk reduction, and 37× faster provisioning. Designed for training infrastructure; interactive human supervision remains underexplored. |
| [AWCP](https://arxiv.org/abs/2602.20493) | Temporary remote workspace delegation with separate control and transport planes; SSHFS for live work and archives for bounded exchange; delegator/executor state machines. | Demonstration-oriented, one-to-one, and still missing granular permissions, audit, conflict-free multi-party writes, and strong governance. Useful future protocol seam, not a V1 dependency. |
| [ESAA-Conversational](https://arxiv.org/abs/2606.23752) | Vendor-neutral visible-turn capture into an append-only log with deterministic projections for handoff, state, decisions, and tasks; mechanical capture separated from judgmental curation. | One 570-event, Windows/PowerShell case study; lacks signatures, redaction, robust concurrency, snapshots, and cross-platform support. The architectural boundary aligns strongly with termi9ne. |
| [JarvisBench](https://arxiv.org/abs/2608.14870) | A bidirectional attention coordinator outside worker loops answers owner questions, detects when agents need judgment, and routes responses back to Runs. | Forty-five tasks across single- and multi-agent settings. Shows an attention layer can improve workers without modifying them, while exposing quality/latency/attention trade-offs. |
| [LiteCUA / AIOS 1.0](https://arxiv.org/abs/2505.18829) | Contextualizes the computer as an MCP server, separating interface complexity from agent decision complexity. | LiteCUA achieved only 14.66% on OSWorld but beat several more elaborate frameworks. The semantic-interface direction matters more than the absolute score. |
| [Governance Gaps in Agent Interoperability Protocols](https://arxiv.org/abs/2606.31498) | Separates coordination protocols from governance: membership, deliberation, voting, dissent preservation, escalation, and audit/replay. | Specification analysis rather than runtime experiment. Protocols change rapidly, but the identified layer should remain owned by termi9ne's domain rather than delegated to MCP/A2A. |
| [Cuckoo Attack](https://arxiv.org/abs/2509.15572) | Persistent AI-IDE compromise through ordinary configuration edits that later execute commands invisibly, including MCP configuration paths. | End-to-end proof of concept across eight agent/IDE pairs plus responsible disclosure. Strong argument for immutable reviewed configuration and visible executable diffs. |
| [ZitPit](https://arxiv.org/abs/2604.06241) | First-seen repositories, packages, workflows, manifests, and configuration become durable admission-policy events before receiving local execution rights. | The paper deliberately makes narrow empirical claims. The consumer-side admission boundary is valuable even if this specific implementation is not adopted. |

### 4.2 Deterministic change admission for parallel agents

Worktree isolation prevents simultaneous filesystem corruption but does not stop
two locally valid changes from colliding semantically. Claim Plane suggests a
separate authority layer before an agent receives write rights:

```text
ChangeIntent
├── exact base commit
├── typed resources: file, symbol, schema, migration, test, config
├── operations: create, modify, delete, rename, generate
├── committed scope and contingent scope
├── dependencies and invalidation triggers
├── evidence and integration requirements
├── lease epoch and fencing token
└── amendment/version history
```

The scheduler atomically admits compatible intents. An attempted write outside
committed scope either promotes a declared contingent resource and rechecks the
active set, requests an amendment, or fails closed. An old worker cannot write
after lease transfer because its fencing token is stale.

The confirmatory study is a useful warning: conservative admission can obtain
reliability simply by serializing everything. termi9ne should begin with exact
file/create/delete claims, collect conflicts and amendments, and treat symbol or
semantic scope as advisory until local evidence supports enforcement.

### 4.3 Execution as a forkable, inspectable value

Shepherd's most novel idea is that an execution is not only a transcript or
container. It is a structured value over which an authorized supervisor can
operate:

```text
ExecutionBranch
├── parent branch and fork event
├── immutable action-intent and action-result events
├── environment state reference
├── reversible-effect boundary
├── outstanding irreversible effects
├── active Driver/capability epoch
└── branch outcome and comparison evidence
```

This enables counterfactual replay from the first affected event, parallel
candidate branches, supervisory intervention before an effect, and reuse of
unchanged prefixes. It should extend Run lineage rather than introduce a second
history model.

“Reversible trace” must not mean “all effects are reversible.” The earlier Safe
to Resume paper shows that files, process memory, remote services, approvals,
and nondeterministic inputs have different recovery boundaries. Branching is
safe only inside declared reversible scopes; other branches require an Effect
Record and reconciliation.

### 4.4 Cohesion-aware asynchronous scheduling

CAID and Co-Coder together suggest a better default than either one agent per
file or maximum fan-out:

1. Build a repository graph from imports, symbol use, ownership, tests, schemas,
   and known co-change edges.
2. Keep strongly coupled files and symbols in the same work package.
3. Isolate hub resources or place them behind an explicit ChangeIntent.
4. Generate a dependency DAG and release a package only after upstream interface
   evidence is committed.
5. Execute each package in a separate worktree.
6. Integrate through immutable commits, merge simulation, and executable checks.

The scheduler should optimize a measurable communication-to-computation trade:
parallelism is valuable only when saved critical-path time exceeds context
handoff, conflict, and integration costs.

### 4.5 Verified task state outside the model context

LongHorizon-Harness's Manage–Execute–Audit loop maps cleanly onto termi9ne:

```text
Manager             Executor                 Auditor
suggests next work  acts in fresh context    read-only inspection
        \                 |                       /
         +-------- verified state transition ----+
```

- The Mission projection is the durable task state.
- A worker proposes progress through Signals, Artifacts, commits, and effects.
- A read-only verifier checks the environment and acceptance policy.
- Only verified facts advance the authoritative projection.
- The next executor receives the verified state and relevant evidence, not the
  preceding agent's unbounded self-narrative.

openJiuwen adds a useful implementation distinction: keep the model's inner loop
stable while the outer harness adapts context, feedback, stopping, and delegated
capabilities using observed evidence.

### 4.6 Recoverable information-flow control

Process permissions answer what a Run may call; they do not answer what
information may influence that call. APPA suggests adding provenance-preserving
labels to context and effects:

```text
ContextFragment
├── source identity and original model-facing role
├── confidentiality label
├── integrity/trust label
├── allowed destinations and tools
├── transformations/declassification evidence
└── parent fragment references
```

Enforcement happens twice:

1. before dispatch, evaluate the requested tool/effect against the complete
   context label and workflow history;
2. after return, validate the realized result before admitting it into parent
   context.

Untrusted data can be inspected in a disposable child Run whose taint remains
local. Only a schema-bounded, evidence-linked Artifact crosses back. This is a
more useful recovery path than permanently poisoning the main Run or aborting
all work.

When Context Gets Root supplies the corresponding invariant: copying tool,
subagent, schedule, goal, or skill text into another prompt must never silently
raise its instruction privilege. Context Receipts need both source provenance
and the exact model-facing role used at delivery.

### 4.7 Effect-aware and diff-based sandbox snapshots

Crab and DeltaBox address different halves of efficient branchable sandboxes:

- Crab decides *when and how much* to checkpoint using OS-visible effects at
  agent-turn boundaries. It exploits model wait time and coordinates snapshot
  traffic across colocated sandboxes.
- DeltaBox reduces *the cost of each checkpoint* by switching copy-on-write
  filesystem layers and incrementally restoring process state from warm
  templates.

A future cloud Runner can combine these ideas:

```text
agent turn
  -> observe file/process/network effects
  -> classify: no state | files only | process + files | external effect
  -> choose journal, filesystem checkpoint, full sandbox checkpoint, or fence
  -> overlap safe snapshot work with provider latency
  -> register state reference in the Run event stream
```

Low-level snapshot success never bypasses the Recovery Manifest. External state,
credentials, leases, approvals, and nondeterministic dependencies must still be
reconciled before semantic resume.

### 4.8 An attention broker outside worker loops

JarvisBench formalizes the missing middle between always-running agents and an
intermittently available owner. termi9ne already has the right source material:
Signals, Attention, Session activity, deadlines, approvals, Artifacts, and Run
state. A Supervisor projection can:

- answer “what is happening?” from durable evidence without interrupting a
  worker;
- rank pending owner decisions across Missions;
- group redundant questions from sibling Runs;
- route an owner response to the exact waiting Run and capability epoch;
- record response latency and task impact;
- never mutate work except through ordinary authorized commands.

The broker should initially be deterministic and read-only. An optional model
may summarize evidence or suggest prioritization, but it cannot invent Attention
or settle work.

### 4.9 Scout, verify, hand off, then route

Scrouting's router was less important than its pre-routing scout. This suggests
a cost-effective Mission pattern:

1. a cheap, read-only Scout Run explores the repository;
2. it produces a structured Handoff Artifact with candidate files, symbols,
   reproduction steps, dependency risks, and estimated work shape;
3. deterministic sandbox checks verify reproduction claims and remove false
   claims;
4. the scheduler selects a Driver, budget, and degree of parallelism using the
   verified task shape;
5. the worker receives the verified handoff and exact source references.

This can improve cheap workers even before a learned router exists. Routing
should be added only after a no-router cheapest-capable baseline is measured.

### 4.10 Auditable harness evolution

Agentic Harness Engineering turns harness changes into falsifiable experiments:

- every editable component has an explicit file/manifest representation;
- raw trajectories become a layered evidence corpus with drill-down links;
- each proposed edit states its predicted effect, evaluation cohort, and stop
  condition;
- the candidate runs in isolated branches against frozen fixtures;
- promotion requires measured improvement without violating safety invariants;
- the previous harness remains immediately recoverable.

This is suitable for an owner-reviewed Harness Experiment Mission. It is not
authorization for a production agent to rewrite its own sandbox, permissions,
Signal semantics, or acceptance policy during a Run.

CHILL-Harness points toward online selective orchestration later. Its learned
intervention should compete against understandable deterministic policies and
must satisfy an advantage margin plus invariant checks before activation.

### 4.11 External software must earn execution rights

Cuckoo Attack, ZitPit, and OverEager converge on a single boundary: repository
content and agent configuration are software supply-chain inputs, not harmless
context.

- First-seen repositories, packages, workflow files, MCP definitions, hooks,
  skills, and generated configuration become Admission events.
- Executable differences are shown separately from descriptive text.
- Approval binds an immutable digest, scope, capabilities, and expiry; changing
  the artifact invalidates the approval.
- The runtime intercepts the effect before execution. A later warning is not
  enforcement.
- Scope is inferred from both the request and durable policy, then monitored for
  undeclared expansion.
- Writes to agent configuration, shell startup, hooks, CI, package scripts, and
  credential helpers receive higher scrutiny than ordinary source edits.

### 4.12 Workspace delegation and semantic computer interfaces

AWCP identifies a useful future protocol layer: remote agents often need the
workspace, not another text message. termi9ne can model a delegated workspace as
a scoped, expiring capability over an exact snapshot or live projection, with a
control-plane lifecycle independent of SSHFS, archive, object-store, or other
transport.

The reviewed AWCP version lacks the access control and audit needed for a secure
implementation. Multi-writer live projection should remain out of scope until
ChangeIntent admission, fencing, and conflict semantics exist.

LiteCUA offers a complementary interface principle: adapt the computer to the
agent with typed semantic state and actions rather than forcing every agent to
reverse-engineer a human UI. termi9ne's side channel can eventually expose file,
symbol, terminal, test, Git, and browser facts through one capability-checked
Agent-Computer Interface while preserving the raw terminal as the universal
fallback.

### 4.13 Coordination protocols are not governance

MCP, A2A, and related protocols can transport messages and advertise tools, but
they do not replace Mission governance. Membership, deliberation, disagreement,
human escalation, acceptance, and audit remain durable domain facts.

ESAA-Conversational reinforces the local memory boundary: capture observable
events mechanically, curate decisions explicitly, project compact views, and
let each agent read selectively. termi9ne already has the stronger event store;
it should add provider import adapters rather than a second JSONL source of
truth.

### 4.14 Roadmap delta from the cutting-edge pass

**Promote into P0:**

1. Preserve source trust and original model-facing role in every Context
   Receipt; forbid silent privilege promotion.
2. Add file-level ChangeIntent admission, lease epochs, and fencing before
   enabling parallel write Runs.
3. Separate proposed progress from audited Mission state and add a read-only
   verification role.
4. Make first-seen executable configuration and external artifacts durable
   Admission events bound to content digests.
5. Treat undeclared scope expansion as a policy event before the write/effect.

**Add to P1:**

1. Cohesion-aware partitioning and dependency-wave scheduling.
2. Read-only Scout Runs with sandbox-verified Handoff Artifacts.
3. A deterministic Supervisor projection for bidirectional owner attention.
4. Effect classification at agent-turn boundaries and checkpoint scheduling.
5. Owner-reviewed Harness Experiment Missions with prediction/evidence records.
6. Logical ExecutionBranch APIs over Run lineage and reversible local effects.

**Keep experimental or P2:**

1. Diff-based process and filesystem checkpoint backends.
2. Full recoverable information-flow control and schema-bounded declassification.
3. Remote live workspace delegation and multi-party writers.
4. Learned online harness intervention or routing.
5. A broad semantic Agent-Computer Interface over remote cloud desktops.

## 5. Cross-paper architecture proposal

The research suggests adding typed records around the existing domain model,
not creating a parallel orchestration system:

```text
Mission plan + Harness Snapshot
              |
              v
     isolated Run / Attempt
       |              |
       |              +--> Escalation Signal --> owner resolution
       v
tool actions + Effect Records + Artifacts
       |
       +--> Handoff Artifact --> successor Run
       |
       +--> Recovery Manifest --> reconcile or branch
       v
Evaluation Receipt --> independent verification --> owner settlement
       |
       +--> Failure Diagnosis when unsuccessful
```

### 5.1 New or extended record types

| Record | Domain representation | Required properties |
|---|---|---|
| Harness Snapshot | Run launch fact | Immutable digest of task, workflow, Driver, tools, skills, sandbox, and evaluator configuration. |
| Escalation | Signal | Evidence-backed conflict that pauses affected work and requests an owner decision. |
| Handoff | Artifact | Bounded, structured state summary with links to source evidence and external effects. |
| Effect Record | Artifact/event | Intended action, authorization, idempotency key, observed result, and verification. |
| Context Receipt | Artifact/event | Typed objects selected, representation delivered, omissions, size, and management cost. |
| Recovery Manifest | Artifact/event | State domains, dependencies, effects, leases, nondeterminism, and reconciliation result. |
| Evaluation Receipt | Artifact | Checks labor, side effects, provenance, final schema, delivery, and repeatability separately. |
| Failure Diagnosis | Derived Artifact | Critical step/range, violated constraint, evidence, cause-versus-symptom distinction, and supersession. |

These records should use existing IDs, append-only Mission history, and scoped
capabilities. Most can begin as typed Artifacts or Signals before becoming new
core aggregates.

## 6. Research-derived invariants

1. A Run is not reproducible unless its harness and evaluator are identified.
2. An agent exit, final message, or confidence score cannot settle work.
3. Every agent must have a policy-backed path to escalate defective work.
4. A handoff must be understandable without replaying an unbounded transcript.
5. Provider context reduction must never delete durable evidence.
6. A checkpoint is not resumable until external assumptions and effects are
   reconciled.
7. An alert that occurs after a privileged effect is observability, not
   prevention.
8. Tool outputs, repository content, skills, and agent-authored notes retain
   their source trust even when copied into another prompt.
9. Evaluation must distinguish correct execution from correct final delivery.
10. Failure analyses are derived interpretations and may be disputed or
    superseded; source events remain immutable.

## 7. Evaluation work suggested by the papers

| Test family | Experiment | Primary measures |
|---|---|---|
| Harness conformance | Run the same fixture through each Driver and evaluator version. | Launch/resume/interrupt correctness, Signal truth, Artifact schema, effect provenance. |
| Handoff debt | Freeze a Run at deterministic points and give successors repository-only versus structured handoff views. | Success, rediscovery events, delivered tokens, time, repeated tool calls. |
| Chain reliability | Compose 1-, 5-, 10-, and 20-step workflows with deterministic faults. | Per-step success, full-chain success, five-of-five reliability, recovery, delivery accuracy. |
| Resume continuity | Checkpoint before/after external reads, writes, approvals, and nondeterministic calls. | Detected mismatches, duplicate effects, stale permissions, safe refusal rate. |
| Escalation uptake | Inject defective tests, impossible constraints, and contradictory owner instructions. | Unsafe shortcut rate, escalation rate, defect precision, resolution latency, task success. |
| Context accounting | Vary context policy over typed object classes. | Stored bytes, delivered tokens, management calls/time, re-fetches, outcome. |
| Failure diagnosis | Seed known critical failures and downstream symptoms. | Critical-step accuracy, category accuracy, evidence quality, false diagnosis rate. |
| Security composition | Vary repository content, test commands, skill text, tool descriptions, and prompt style. | Pre-effect block rate, attack success, false positives, attention timing. |

The release harness should retain fixture-based tests without live model
credentials and keep provider-backed acceptance tests separately gated.

## 8. Wigolo as an implementation reference

[Wigolo](https://github.com/KnockOutEZ/wigolo) is not an agent multiplexer. It is
a local-first web-intelligence service for agents exposed through MCP, REST,
CLI, and SDKs. At review time its README described it as a public beta under
AGPL-3.0, even though GitHub's license metadata did not identify an SPDX license.

Its most transferable idea is an evidence-native result contract. Results can
include a verbatim excerpt, byte-level source span, citation ID, component
scores, engine consensus, and freshness confidence. Stale cache, failed engines,
truncation, weak results, and challenge blocks are surfaced instead of being
silently converted into content.

Other useful patterns include:

- direct HTTP first, escalating to a headless browser only on observable SPA or
  challenge signals;
- parallel multi-engine search followed by deterministic rank fusion and local
  reranking;
- local keyword/vector cache, similar-page lookup, change detection, and watch
  notifications;
- robots.txt handling and per-domain rate limits;
- one web capability available through MCP, REST, CLI, and typed SDKs;
- opt-in model synthesis, with deterministic fetching and evidence handling kept
  outside the model.

### 8.1 Potential termi9ne Web Evidence Artifact

```text
WebEvidenceArtifact
├── source URL and retrieval time
├── content digest and extraction method
├── verbatim excerpt and byte/DOM span
├── citation ID
├── freshness evidence and confidence
├── retrieval/ranking component scores
├── cache state
└── blocked, degraded, stale, weak, or truncated flags
```

This fits the arXiv papers' broader lesson: record evidence and its provenance,
not only a synthesized answer. Browser escalation should remain an observable
execution choice, and headless direct fetching should be the default.

Wigolo should be evaluated as an optional external MCP/Driver integration, not
copied into the scheduler or event store. Its AGPL license, public-beta status,
browser/model downloads, remote-search privacy, and self-published benchmark
claims require independent review. Its autonomous `agent` loop should not nest
an opaque second Mission system inside termi9ne.

## 9. Recommended implementation order

### P0 — correctness and operator control

1. Define the Harness Snapshot and Evaluation Receipt schemas.
2. Add an evidence-backed escalation Signal and explicit resolution events.
3. Add a structured Handoff Artifact for pause, replacement, and review.
4. Record external effects, their authorization, and idempotency keys.
5. Validate final delivery separately from process exit and side-effect success.
6. Treat recovery as reconciliation; do not advertise semantic rollback without
   a Recovery Manifest.
7. Add pre-effect policy gates for untrusted repository scripts, skills, tool
   descriptions, and context.

### P1 — diagnostics and measurable efficiency

1. Record Context Receipts at stored, delivered, management, and outcome levels.
2. Add a derived Failure Diagnosis Artifact and a problems view.
3. Implement handoff-debt, chain-reliability, resume, and escalation fixtures.
4. Add digest-backed Artifact provenance and delegation manifests.
5. Add an optional Web Evidence adapter with explicit degraded-state reporting.
6. Export a scrubbed, versioned event dataset for owner analysis.

### P2 — distributed trust and learned optimization

1. Co-signed delegation ancestry for cross-owner Runner fleets.
2. Consequence-aware context optimization after sufficient local telemetry.
3. Signed, governed Skill registries and portability bundles.
4. Change-watch and scheduled research Missions over Web Evidence Artifacts.

### Do not adopt from this pass

- early intervention based primarily on agent confidence;
- transparent checkpoint resume without external-effect reconciliation;
- context policies measured only by a nominal token limit;
- security alerts that do not gate the effect they warn about;
- automatic trust of repository instructions, imported skills, or tool text;
- a second opaque autonomous loop hidden inside a tool integration.

## 10. Selected bibliography

| Paper | Listing kind | Why retained |
|---|---|---|
| [DS-Lighting](https://arxiv.org/abs/2608.28590) | New, v1 | Explicit harness layers and reproducible evaluation. |
| [APIFlow-Bench](https://arxiv.org/abs/2608.29128) | New, v1 | Long-chain reliability, provenance gating, and final-delivery failures. |
| [AgentLogs](https://arxiv.org/abs/2608.29204) | New, v1 | Real coding-agent trajectory schema and ecosystem-scale data. |
| [TRACER](https://arxiv.org/abs/2608.29363) | New, v1 | Consequence-aware per-tool context retention. |
| [Safe to Resume?](https://arxiv.org/abs/2608.29381) | New, v1 | Security limits of checkpoint and rollback. |
| [Escalation channels](https://arxiv.org/abs/2608.29460) | New, v1 | Structured defect reporting as an agent safety mechanism. |
| [Agentic skills systems foundation](https://arxiv.org/abs/2608.29596) | New, v1 | Skill lifecycle, execution, evaluation, and governance. |
| [Last Step Matters](https://arxiv.org/abs/2608.29685) | New, v1 | Limits of intermediate uncertainty for supervision. |
| [Delegation ancestry attestation](https://arxiv.org/abs/2608.30387) | New, v1 | Output provenance and cross-deployer authorization evidence. |
| [ECLIPSE](https://arxiv.org/abs/2608.30441) | New, v1 | Multi-stage, multi-source prompt-injection risk. |
| [Repository poisoning and invocation](https://arxiv.org/abs/2608.30686) | New, v1 | Task- and prompt-dependent coding-agent security. |
| [Agent working memory](https://arxiv.org/abs/2608.31057) | New, v1 | Semantic memory types and four-level measurement. |
| [Handoff Debt](https://arxiv.org/abs/2606.02875) | Replacement, v2 | Resumability cost and compact handoff evidence. |
| [AgentRx](https://arxiv.org/abs/2602.02475) | Replacement, v2 | Evidence-grounded critical failure localization. |
| [Claim Plane](https://arxiv.org/abs/2607.21909) | Broader search, v1 | Pre-write authority and dynamic scope for parallel coding agents. |
| [Claim Plane confirmatory study](https://arxiv.org/abs/2608.00947) | Broader search, v1 | Reliability gains and the serialization/under-declaration limits. |
| [Shepherd](https://arxiv.org/abs/2605.10913) | Broader search, v3 | Reversible, forkable execution traces as first-class values. |
| [When Parallelism Pays Off](https://arxiv.org/abs/2606.00953) | Broader search, v1 | Cohesion-aware graph partitioning and dependency scheduling. |
| [Effective Strategies for Asynchronous SWE Agents](https://arxiv.org/abs/2603.21489) | Broader search, v2 | Branch-and-merge worktree coordination with test gates. |
| [LongHorizon-Harness](https://arxiv.org/abs/2608.01964) | Broader search, v1 | External verified task state and Manage–Execute–Audit. |
| [openJiuwen](https://arxiv.org/abs/2608.27969) | Broader search, v1 | Composable inner/outer loops and evidence-driven runtime adaptation. |
| [Agentic Harness Engineering](https://arxiv.org/abs/2604.25850) | Broader search, v4 | Falsifiable, evidence-backed harness evolution. |
| [CHILL-Harness](https://arxiv.org/abs/2607.25825) | Broader search, v1 | Learned selective counterfactual harness interventions. |
| [Scrouting](https://arxiv.org/abs/2608.04804) | Broader search, v1 | Verified repository scout handoff before model routing. |
| [APPA](https://arxiv.org/abs/2607.24625) | Broader search, v2 | Recoverable information-flow control and confined child trajectories. |
| [When Context Gets Root](https://arxiv.org/abs/2608.27299) | Broader search, v1 | Instruction privilege escalation caused by context construction. |
| [Overeager Coding Agents](https://arxiv.org/abs/2605.18583) | Broader search, v1 | Scope expansion as an authorization failure. |
| [Crab](https://arxiv.org/abs/2604.28138) | Broader search, v1 | Effect-aware checkpoint scheduling for colocated sandboxes. |
| [DeltaBox](https://arxiv.org/abs/2605.22781) | Broader search, v2 | Millisecond diff-based process/filesystem checkpoint and rollback. |
| [OSGym](https://arxiv.org/abs/2511.11672) | Broader search, v5 | Thousand-replica OS sandbox infrastructure. |
| [AWCP](https://arxiv.org/abs/2602.20493) | Broader search, v1 | Remote workspace delegation above pluggable transports. |
| [ESAA-Conversational](https://arxiv.org/abs/2606.23752) | Broader search, v1 | Event-sourced continuity across heterogeneous coding agents. |
| [JarvisBench](https://arxiv.org/abs/2608.14870) | Broader search, v1 | Bidirectional attention coordination outside worker loops. |
| [LiteCUA / AIOS 1.0](https://arxiv.org/abs/2505.18829) | Broader search, v2 | Computer-as-MCP semantic agent interface. |
| [Governance Gaps](https://arxiv.org/abs/2606.31498) | Broader search, v1 | Governance layer missing above interoperability protocols. |
| [Cuckoo Attack](https://arxiv.org/abs/2509.15572) | Broader search, v1 | Persistent AI-IDE compromise through executable configuration. |
| [ZitPit](https://arxiv.org/abs/2604.06241) | Broader search, v1 | Consumer-side execution admission for external artifacts. |

## 11. Bottom line

The most useful recent research is not asking termi9ne to make agents more
autonomous. It is asking the runtime to make their work more explicit:

- what harness and policy governed the Run;
- what evidence and external effects it produced;
- when it escalated instead of gaming a task;
- what another agent needs to resume it;
- whether recovery preserves the assumptions that made earlier actions valid;
- whether verification covers both the work and its final delivery.

Those are natural extensions of termi9ne's event-sourced Mission graph. The next
step should be stronger receipts, handoffs, escalation, and recovery semantics,
not a broader but less trustworthy autonomous surface.

The broader cutting-edge pass adds one sharper product thesis: termi9ne can be
the deterministic execution-control plane around probabilistic coding agents.
Its differentiating primitives would be pre-write ChangeIntents, verified task
state, forkable Run lineage, provenance-preserving context, effect-aware
snapshots, scoped admission of external software, and an attention broker that
lets one human supervise many continuing Runs without trusting any agent's
self-narrative.
