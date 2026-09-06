# MCP input boundary

Date: 2026-09-06 IST

Status: source implementation only. Tests and builds are deferred at the owner's
request; this is not new security or interoperability acceptance evidence.

The stdio reader admits at most 1 MiB per newline-delimited message, including a
line terminator when present. It reads at most one byte beyond that limit before
rejecting input. Oversized or invalid UTF-8 input closes the bridge after a
generic parse-error response, rather than growing an unbounded string or trying
to drain an indefinitely long line. An incomplete final line at EOF remains
processable. This is an input-memory limit, not an idle/read deadline or total
process/output memory budget.

Before dispatch, the bridge requires a JSON-RPC 2.0 object, a string method, a
string/integer request id and object-valued MCP parameters/tool arguments.
Id-less messages are ignored before tool dispatch; in particular, malformed
notification-style tool calls cannot replay, report, resolve or dismiss Faults.
This does not add a full initialization/cancellation state machine.

Explicit invalid booleans, limits, replay timeouts, Fault kinds, exit codes and
output values are rejected instead of silently selecting defaults. Omission
still selects the documented defaults. Replay timeout is 1–900 seconds and guard
limit is 1–200. Parameters and tool arguments borrow the parsed message rather
than cloning those subtrees.

The bridge remains Fault-only and owner-connected. Fault replay and regression
guard execute recorded commands; Fault report accepts commands. Limiting exposed
tools is not a sandbox or a safe capability grant to an untrusted agent. No Share
fallback, new network listener, broader workflow tools or new execution authority
is introduced here. Output pagination/budgets and protocol/security acceptance
remain separate work.
