# Bounded MCP response encoding

Status: implemented in source, unbuilt and untested. No tests, builds, MCP client
runs, Fault execution, or performance measurements were performed, per request.

All tool results now pass through a byte-limited JSON writer, capped at 512 KiB
for pretty-printed tool text. This replaces unbounded pretty serialization and
the terminal-only size check that previously allocated the entire encoded text
before comparing its length. The writer stops before extending its buffer past
the limit; no partial result is sent to stdout on this encoding failure.

The enclosing JSON-RPC response has its own 1 MiB cap, including the eventual
newline. Escaping the tool text can enlarge that response, so the outer cap is
independent. If exceeded, the bridge emits a small valid JSON-RPC error with the
bounded request ID, warning that a tool may already have completed. Output
buffering cannot prevent a physical stdout/pipe failure partway through an OS
write; in that case the bridge stops as before.

String request IDs are limited to 256 UTF-8 bytes before cloning or dispatch.
Oversized IDs are rejected with a null-ID invalid-request response and do not
invoke a tool. Integer IDs retain existing behavior. Tool/RPC error descriptions
are capped at approximately 4 KiB on UTF-8 boundaries with an explicit marker.

## Effects are not rolled back

A tool can succeed and then exceed the result-size limit. For read tools, the
error asks for a smaller terminal request or narrower Fault record. For write
tools, it states that the tool completed and instructs the integration not to
repeat the write/replay automatically. Backend errors from Fault write/replay
tools also carry an uncertainty warning before their bounded diagnostic text.
Neither a response-size failure nor a timeout proves the mutation did not occur.
No write is retried or compensated by this implementation.

## Scope and deferred acceptance

These are encoded-response bounds. The native client may already have decoded
a full Fault collection or terminal frame, and construction of the intermediate
JSON Value still has costs. This does not establish bounded native list paging,
whole-process memory, asynchronous MCP cancellation, or transport interoperability.
The subsequent [paged Fault inspection](mcp-paged-fault-inspection.md) source
change adds compact metadata and digest-pinned output pages for oversized records;
it remains unbuilt and untested and does not change native full-record decoding.

Future acceptance must cover small existing responses, exact byte limits and
escaping expansion, UTF-8 boundaries, malformed/oversized IDs before effects,
large Fault/terminal results, successful mutations with undeliverable results,
backend uncertainty messages, no partial encoding output, stdout failure, and
client recovery without automatic mutation replay. Previous MCP input-boundary
evidence does not validate these new output paths.
