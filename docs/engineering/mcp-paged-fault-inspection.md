# Paged MCP Fault inspection

Status: source implemented, unbuilt and untested. No tests, builds, Fault reads,
replays, runtime commands, or MCP client checks were executed, per request.

The existing `fault_list` and `fault_show` tools now accept compact/paging options.
No new tool name or execution authority is added. Existing calls without those
options retain their full-result shape, subject to the response-byte limits.
Share-connected terminal-only bridges still reject these tools entirely.

## Compact discovery

Use `fault_list` with `compact:true` and optionally `limit` (1–200, default 50
for compact mode). Each entry contains identity, lifecycle/receipt indicators,
output lengths, and summary/command/directory display snippets capped at 256
UTF-8 bytes each. Snippets expose their truncation and source display-text byte
length. Full lifecycle notes and receipt bodies are explicitly omitted; snippets
are not exact replay commands or verified filesystem paths.

The response returns `has_more` and `next_after`. Supply the latter as `after`
for another page. `limit` can also page full records, though large full records
can still exceed the tool-result cap. `open` counts open entries in the current
filtered collection, not just the page. Pages preserve backend order and are
explicitly not a transactionally consistent collection snapshot. A disappeared
cursor fails with instructions to restart, rather than silently skipping it.

`fault_show` with `compact:true` returns the same bounded metadata for one ID.
The default non-compact behavior remains available for smaller records.

## Recorded output pages

For a large log, call `fault_show` with a Fault ID and `output_field`:

- `observed`: the original recorded failing output;
- `repro`: the most recent recorded replay output;
- `proof`: the preserved replay output that demonstrated the resolved state.

Missing replay/proof data returns an error; reading never executes a replay.
`offset` defaults to zero, and `max_bytes` defaults to 16 KiB with a range of
4–65,536 bytes. Page ends preserve UTF-8 boundaries. Offsets must be valid byte
boundaries within the chosen text; callers should use the returned `next_offset`.
The minimum page size permits forward progress over a four-byte UTF-8 character.

Every page returns the field, Fault ID, total bytes, offset, next offset,
completion flag, evidence timestamp, and SHA-256 digest. The digest covers a
domain-separated combination of Fault identity, selected field, evidence
timestamp, length, and full output. A nonzero offset requires that digest in
`expected_sha256`. If the recorded field has changed, the bridge rejects the
page and asks the caller to discard partial output and restart at zero. A digest
may also constrain an offset-zero read.

This is consistency pinning for recorded text, not a cryptographic signature,
independent verification receipt, immutable whole-Fault snapshot, or historical
version store. Metadata outside the selected output can change independently.
The bridge hashes the selected full output for each page; CPU costs are not
measured or claimed constant-time relative to log length.

Unknown arguments, malformed types/IDs/digests, incompatible compact/paging
options, and invalid numeric bounds fail before backend reads. The same global
512 KiB tool-text and 1 MiB JSON-RPC response limits still apply. Native Fault
listing/get currently decodes full records before projecting them, so this is
not an end-to-end native memory or server-paging guarantee.

## Deferred acceptance

Future checks must cover unchanged default responses, compact limits/truncation,
open counts and cursor disappearance, empty/multibyte/exact-boundary logs, no
replay on missing fields, preserved proof versus latest replay, required digest
continuation, evidence changes between pages, malformed options, result escaping
limits, Share denial, and MCP interoperability. Earlier full-record inspection
evidence does not certify this implementation.
