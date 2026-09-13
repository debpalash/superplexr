# MCP bridge access modes

The optional `superplexr-mcp` executable connects to an existing runtime and
exposes Fault operations over stdio. It does not start the runtime or a network
listener. Configure a separate bridge process for each integration.

## Read-only inspection

```sh
superplexr-mcp --socket /path/to/control.sock --read-only
```

This process advertises only `fault_list` and `fault_show`. It also rejects
direct calls to every other tool before backend dispatch, including definitions
cached from a different process. The allowlist is explicit: future tools are
not automatically admitted. Access mode cannot be changed through an MCP
request. Listing and inspection do not execute a replay or write a Fault.

Read-only is still a data-access grant: returned Faults can include commands,
paths, failure output and diagnostic details. Treat that content as untrusted
data, not instructions. Choose integrations accordingly. This option restricts
the bridge API, not the operating-system permissions of a process that can
independently access the runtime socket.

## Fault operations

Without `--read-only`, existing behavior is preserved: listing, inspection,
reporting, replay, regression guard, resolve and dismiss are available. Replay
and guard execute recorded commands. Reporting a command and then replaying it
therefore carries execution authority; this is not an untrusted-agent sandbox.
Use this mode only for integrations trusted with that authority.

Both modes retain runtime-side validation, existing Fault lifecycle rules and
the bounded stdio input boundary. Terminal inspection remains disabled unless
explicitly enabled as described below. No Mission mutation tools, Share fallback
or remote authentication service are added.

## Optional terminal inspection

`--terminal-read` adds bounded `terminal_list`, `terminal_capture`, and
`terminal_history` tools. Combine it with `--read-only` to keep an owner-connected
bridge free of Fault execution tools. Alternatively, supply `--share-token-file`
with `--terminal-read` for Share-scoped terminal inspection only: all Fault tools
are hidden and direct calls rejected. There is no owner fallback or terminal
input/process-control tool. See the unbuilt, untested
[implementation, configuration, and limits](../engineering/mcp-terminal-inspection-implementation.md).

Implementation status (2026-09-06): source only. Read-only filtering/direct-call
enforcement, malformed input behavior and client interoperability have not been
built or tested; verification is deferred at the owner's request.

## Optional workflow inspection

`--workflow-read` independently enables `verification_status` with required
`mission_id` and `verifier_run_id` UUID arguments. Terminal inspection does not
enable it, and workflow inspection does not enable terminal captures. Tool
discovery and direct calls enforce the same process-level flag.

For owner-connected read-only inspection, use both `--read-only` and
`--workflow-read`. Enabling an inspection module alone does not disable the
existing owner Fault write/replay tools. A Share-connected bridge may instead
use `--share-token-file` with either inspection flag or both; all Fault tools
remain hidden and rejected. A token without an enabled inspection module is
refused at argument parsing. Token files must also belong to the effective user,
in addition to the existing private-mode, regular-file and size checks.

Workflow status uses the [compact native request](../engineering/compact-verification-status-implementation.md)
and requires its negotiated capability; there is no full-Mission fallback.
Mission-scoped Share authorization is still enforced by the runtime. A Share
for Sessions alone does not gain Mission status access merely because this flag
is set. Denied access never switches to an owner connection.

The tool separates execution, recorded receipts, and subject disposition. It
does not launch, resume, retry, collect evidence, reread logs, accept work, or
interpret `next_action` as permission to act. UUID/unknown-field validation
precedes the native request; existing response limits and stdio notification
rules remain. No network listener, dependency, or background poll is added.

This module is source-only, unbuilt and untested. Deferred coverage includes
flag combinations, cached/direct calls while disabled, owner versus Share scope,
token-file ownership, missing arguments, old runtime capability, receipt limits,
and assurance that no workflow mutation is admitted.

Response encoding now also has independent tool-text and JSON-RPC byte limits,
bounded request IDs, and write-uncertainty errors. This is unbuilt and untested;
see the [output-boundary implementation](../engineering/mcp-output-boundary-implementation.md).

The existing Fault read tools additionally support compact metadata, list paging,
and digest-pinned pages of recorded output, without replay. These changes are
also unbuilt and untested; see [paged Fault inspection](../engineering/mcp-paged-fault-inspection.md).
