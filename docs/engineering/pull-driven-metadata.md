# Pull-driven metadata and bounded frontend handoff

Date: 2026-09-06 IST

## Implementation

Mission, terminal-index, Run Activity and Session Group subscriptions previously
decoded into unbounded standard channels on four native relay threads. Desktop
workers then forwarded those objects into four more unbounded Tokio channels;
the CLI merged them into an unbounded Tokio channel too.

`MetadataReceiver<T>` now reads and decodes directly from the bounded native
wire mailbox. The four native relay threads and their decoded FIFOs are gone.
There is no new dependency or Tokio runtime requirement in the native client.
The desktop uses one worker and one queued delivery per feed. The CLI uses four
shared queued slots plus one worker per selected feed. Backpressure reaches the
wire mailbox instead of accumulating another decoded history of changes.

`MetadataDelivery<T>` carries the exact logical subscription and physical
connection. Frontends keep that wrapper through their bounded handoff and check
`into_current` immediately before applying or printing the item. Cancellation,
observed logical Close, physical disconnect and subscription replacement
invalidate held deliveries. This is local observed continuity, not a durable
Share revalidation RPC and not a guarantee to undo data already displayed.

Reconnect is pull-driven: it happens when the consumer asks for another item,
not repeatedly while its handoff remains stalled. Only read subscriptions are
reopened; no input or Control claim is replayed. Transient reconnect failures
back off up to two seconds. Explicit daemon authorization/version rejection
ends the receiver instead of retrying forever.

A cancellation guard wakes blocked receive and retry backoff without socket
I/O on the dropping thread. The receiving worker owns the subsequent
Unsubscribe write. A handshake or write already in progress still has the
existing blocking transport constraints; cancellation does not pretend to
interrupt arbitrary connector work.

Development interface change: the four `subscribe_*` methods return
`MetadataReceiver<T>`, not `std::sync::mpsc::Receiver<T>`. Immediate consumers
can call `recv`; async handoffs must use `recv_delivery`. `recv_timeout` makes
one bounded wait on the current mailbox and does not initiate a potentially
five-second reconnect handshake. Timeout leaves the subscription intact.

## Verification status

The release workspace passed **470 Rust tests**, with zero failures and six
explicitly ignored fixture/acceptance entries. All **28 browser/harness tests**
passed. Release Clippy across all targets/features passed with `-D warnings`,
as did formatting and whitespace checks. The existing `block v0.1.6`
future-incompatibility notice remains. All five release executables rebuilt;
isolated native-keyboard and CLI multiplex smoke checks passed.

Four new real-daemon regressions exercise the same metadata receiver and desktop
handoff used by the app:

- Hold a delivery and stall the desktop handoff through 300 Session Group
  changes. The upstream mailbox overflows, old Control retires, the physical
  connection closes without reconnect churn while the handoff is stalled,
  and the held delivery is rejected. Resuming consumption recovers the latest
  group version on the same PTY, without an automatic Control claim.
- Cancel an idle metadata receive. The blocked worker exits and the subscription
  count returns to zero while the shared control connection remains usable.
- Time out a mailbox wait without detaching it, then drop an idle desktop feed
  and verify that its subscription is removed.
- Revoke an Observer Share. The held delivery becomes invalid before its
  consumer reads again; the next receive ends with explicit Share rejection
  instead of an endless reconnect loop.

These four regressions also passed an additional standalone run after the full
suite. Fixtures cleaned up their own processes; the user's runtime was not
restarted or rewritten.

## Fresh release resource sample

The [raw release artifact](shared-client-pull-metadata-release.json) records
the current binary hashes and a 300.069-second idle sample on the macOS M2 host,
with a ten-second warmup, twelve 80x24 Sessions retaining 100,000 short ASCII
history rows each, one visible Observer desktop Surface, one TUI pane and one
HTTP event feed plus metadata polling. No build/test jobs overlapped the window.

| Observation | Measured | Reference target |
|---|---:|---:|
| Runtime idle CPU, one core | 0.320% | <= 0.5% |
| Desktop idle CPU, one core | 0.367% | <= 1% |
| Runtime + desktop sampled peak RSS | 120.33 MiB | <= 600 MiB |
| Unexpected idle SSE frames | 0 | 0 |
| Idle TUI output bytes | 0 | 0 |

All six gate observations passed, including the five-minute duration. Gateway
and TUI sampled peak RSS were 4.91 MiB and 2.67 MiB, respectively. Grid geometry,
oldest history marker, Session IDs and PTY PIDs survived. CLI-to-SSE p95 was
68.23 ms, including CLI startup and 50 ms observation granularity, not pixel
latency. Cleanup removed only the benchmark's isolated processes and temporary
fixture tree; the user's existing desktop/runtime were rechecked alive.

RSS is sampled resident memory, not total allocation/commit or an allocation
high-water mark. Browser/compositor/outer-terminal/driver costs are excluded.
The lower resident sample relative to earlier runs is not proof that removing
metadata relay threads caused that difference. This is one idle workload on
one host, not sustained output/search, startup or cross-platform certification.

```sh
node ci/shared-client-resource-benchmark.mjs --output docs/engineering/shared-client-pull-metadata-release.json
```

The output must not already exist; use a fresh path when reproducing it.

## Remaining work and limits

- Complete snapshot start/end markers and replacement semantics are absent
  from these preview metadata streams. Reconnecting individual changed items
  cannot reliably remove collection entries deleted while disconnected.
  Collection reconciliation and retained GUI-state invalidation remain open.
- The existing generic wire budget counts queued encoded payload capacity.
  Decoded items held by a worker, bounded frontend slots and current UI
  projections are outside that byte charge. The desktop handoff holds at most
  one queued item, one worker-held item and one consumer-held item per feed;
  this is not a certified total decoded-memory/RSS bound.
- Oversized individual snapshots/records, full collection paging and explicit
  retry/admission policy need acceptance independently of queue count limits.
- The legacy terminal `subscribe()` convenience FIFO, performance-sample FIFO,
  response buffers and protocol sequence bookkeeping remain separate work.
- Ordinary metadata reads, initial subscription setup, worker Unsubscribe and
  CLI stdout can still block their callers. No broad nonblocking-I/O claim.
- IME/accessibility, non-macOS and sustained-workload evidence remain open.
