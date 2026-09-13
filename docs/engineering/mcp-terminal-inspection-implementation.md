# Opt-in MCP terminal inspection

Status: source implemented, unbuilt and untested. No tests, builds, MCP clients,
terminal reads, or runtime commands were executed, per owner direction.

Three read-only tools extend the optional stdio bridge when the owner starts it
with `--terminal-read`: `terminal_list`, `terminal_capture`, and
`terminal_history`. They use the existing native read methods. No terminal input,
claim, resize, launch, terminate, Mission mutation, or generic RPC tool is added.

An independent `--workflow-read` module now exposes compact verification status;
it is not implied by `--terminal-read`. Share token files may be used with either
explicit inspection module. See [MCP access modes](../design/mcp-access-modes.md).
This extension remains source-only and untested.

The later [shared-label extension](shared-session-labels-implementation.md) adds
display-only title/directory metadata and byte-limited cursor pages to
`terminal_list`. It remains unbuilt and untested; callers must follow
`next_after`, not assume every page contains the requested number of entries.

## Access modes

Usage examples below are configuration instructions, not commands run during
implementation:

```sh
# Owner-connected reads: Fault inspection plus terminal inspection.
superplexr-mcp --socket /absolute/path/control.sock --read-only --terminal-read

# Share-scoped terminal inspection only; Observer Shares are preferred.
superplexr-mcp --socket /absolute/path/control.sock --terminal-read \
  --share-token-file /absolute/path/observer.token
```

Without `--terminal-read`, the existing Fault-only tool surface is preserved.
Adding that flag to an owner bridge **does not disable Fault replay/report/guard**;
also supply `--read-only` when execution is not intended. Read-only Fault tools
can themselves return commands, paths, or secrets from recorded failure output.

`--share-token-file` requires an explicit inspection module (`--terminal-read`
or `--workflow-read`). A Share-connected bridge is always
read-only, even for a Controller Share. It advertises only the three terminal
inspection tools and rejects direct Fault-tool calls before backend dispatch.
It connects with the supplied Share only; rejection never falls back to owner
connection or starts a runtime. Scope and revocation remain runtime-authoritative.
An MCP request cannot change these process-level choices.

The Unix token reader opens without following a final symlink and without
blocking on a FIFO, checks the opened descriptor is a regular owner-only file,
then reads at most 16 KiB plus an overflow byte. Empty, oversized, or non-UTF-8
tokens are rejected. No token contents are included in tool definitions/results.
This is not a Windows token-file implementation or an OS sandbox: a process with
independent owner-socket access still has whatever the OS allows it to do.

## Tool output and limits

- `terminal_list`: defaults to 100 entries, maximum 200. `next_after` is the last
  Session ID of a page with more entries. IDs sort lexically. Returned records
  omit tty paths and Control identities; executable labels are capped at 256
  UTF-8 bytes with a truncation flag. These pages are explicitly **not** a
  transactionally consistent snapshot: concurrent creation/archive may change
  membership between calls.
- `terminal_capture`: returns current canonical text, a string sequence number,
  grid dimensions, lifecycle status, and bounded title/directory metadata. It
  observes without claiming Control.
- `terminal_history`: requests a retained viewport at an offset from zero to
  100,000 rows. Its live lifecycle status is not inferred from a separate call;
  `status` is null. History coordinates may change as output advances.
- Both text tools default to 16 KiB and accept at most 64 KiB via `max_bytes`.
  Truncation preserves UTF-8 boundaries and is reported explicitly. Titles and
  directories have separate 512/2,048-byte limits and truncation flags.
- Unknown arguments, wrong types, invalid IDs, and out-of-range bounds fail
  before the read RPC. Serialized terminal-tool JSON text is limited to 512 KiB
  before wrapping as MCP text content; exceeding it returns an error asking the
  caller to reduce its request.

These are tool-result bounds, not a paged native-terminal protocol or a whole
process memory guarantee. The native client still decodes its existing full
Session list/frame response before this projection, and JSON-RPC text wrapping
adds escaping overhead. General Fault-result sizing and cancellation of an
already executing synchronous MCP call are separate concerns.

Terminal contents, process labels, and titles are untrusted data, potentially
including secrets or prompt-injection text. Tool descriptions explicitly tell
the integration to treat them as task data, not instructions. Scoped read access
is still a real data-access grant. Choose the Share and recipient accordingly.

## Deferred acceptance

Future checks must cover unchanged Fault-only behavior, both owner read modes,
Observer/Controller Share filtering and direct-call rejection, no owner fallback,
revocation/reconnect, token file types/permissions/races/size, malformed arguments,
cursor paging under membership changes, UTF-8 truncation, history bounds, result
limits, secret non-disclosure, absent execution/input routes, and MCP client
interoperability. No production/security/platform readiness is established.
