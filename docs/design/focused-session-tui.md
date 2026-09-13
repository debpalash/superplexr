# Terminal workspace TUI

Status: optional development client, verified on macOS arm64 through 2026-09-06.
This includes scoped navigation, two-pane splits and streamed history search, not full desktop parity
or a Windows port.

## Run

Use an already running daemon and an existing Session. The TUI never starts a
daemon, launches a terminal, restores archived processes, or writes a session DB.

```sh
cargo build -p superplexr-tui
target/debug/superplexr-tui --socket /absolute/path/control.sock --list
target/debug/superplexr-tui --socket /absolute/path/control.sock
target/debug/superplexr-tui --socket /absolute/path/control.sock SESSION_ID
```

Pass the socket of the runtime you intend to use, especially with the desktop's
isolated `.superplexr-dev/v25` development state. Omitting `--socket` uses the native
protocol's default socket, which need not be the debug desktop's socket.

Observation is the default, even on an owner connection. `--control` explicitly
requests a non-forced lease on initial attach; it never displaces another Surface.
Alternatively request Control from inside the TUI. The current holder must return
Control first. A scoped client can use `--share-token-file /private/observer.token`
or a Controller Share file (regular, owner-only, at most 16 KiB). An invalid Share
never falls back to owner access. The same permissions are checked by the runtime.

Omit the Session ID to open the navigator. Tab cycles Sessions, Missions, and
Attention; arrows or `j`/`k` move, Enter opens the selected item, `/` edits a
case-insensitive filter, `r` refreshes, and Esc returns to the panes. Backspace
outside filter editing clears the Mission filter. `q` detaches the whole TUI.
At least seven terminal rows are required to choose a navigator item.

Entering a Mission filters its Sessions. Session labels use authorized group or
domain names, with foreground-process identity as a fallback. Pinned groups sort
first. Attention uses unresolved runtime signals, ordered by the domain's urgency;
Enter observes the Run's explicit primary Session. Items without an accessible
primary Session remain visible but cannot open an arbitrary terminal. Opening an
Attention item never resolves it, approves an operation, or claims Control.

The navigator refreshes every two seconds only while open, preserving selection
by stable identity and avoiding redraw when content is unchanged. These metadata
reads are separate from event-driven terminal frames. Errors clear stale entries;
revocation ends access. Mission/Attention visibility follows Share scope, and
owner-only group names are not requested by shared clients. Desktop-local hidden
tabs are not runtime deletion: this navigator lists authorized runtime work, but
never automatically opens old workspaces or persists its own layout.

## Keys

An optional `--workflow-read` module exposes read-only verifier discovery and
inspection. In the navigator select or enter a Mission and press `w` to list
up to 64 verifiers. Enter reads a verifier's status, `n` reads the next page,
and `0` returns to the first page. `W` accepts a full verifier Run UUID manually.
Each page reports its observed Mission version and requires explicit refresh;
filtering covers only the current page. The recorded snapshot shows
execution, candidate identity, receipt summaries and owner disposition; it
does not execute checks or accept work. `r` refreshes explicitly, Backspace
returns to the current catalog page or Mission Sessions, and Esc returns/detaches as usual. See
[implementation and limits](../engineering/tui-verification-inspection-implementation.md).
The current release-profile TUI suite passes, including the focused discovery
regressions; the earlier verified baseline above predates this addition.

Press `Ctrl-]`, release it, then the command key:

| Key | Action |
| --- | --- |
| `d` or `q` | Detach, return this view's Control if held; leave the Session running |
| `n` | Open Mission/Session navigation to replace the focused pane |
| `a` | Open Attention navigation |
| `v` | Choose a second Session for a side-by-side split |
| `s` | Choose a second Session for a stacked split |
| `o` or Tab | Focus the other pane, returning the old pane's Control |
| `x` | Close the focused pane, not its Session; the last pane returns to navigation |
| `c` | Request non-forced Control and fit the shared PTY to this terminal |
| `r` | Return Control and continue observing |
| `p` | Pause/resume the local display; input is blocked while paused |
| `h` or Page Up | Read an earlier page of retained host history |
| `/` | Open incremental, case-insensitive literal history search |
| `l` or End | Return immediately to the latest live frame |
| `y` | Explicitly export displayed text, only with `--osc52` enabled |
| `?` | Show input commands; the prefix alone shows navigation commands |
| `Ctrl-]` again | Send a literal prefix to the controlled terminal |

Legacy terminals encode Ctrl-] and Ctrl-5 with the same byte; both spellings are
accepted. Key decoding uses [Crossterm events](https://docs.rs/crossterm/0.29.0/crossterm/event/index.html).
Outside local navigation/search, normal keys, including Ctrl-C, go to the host only while controlling and live.
Thus Ctrl-C may interrupt the host program; it is not the TUI detach command.
Multiline or control-character bracketed paste requires an explicit `y`; any
other key cancels. Paste confirmation does not echo the pasted payload.
This guard requires the outer terminal to send bracketed paste; unmarked pasted
bytes cannot be distinguished from ordinary typing.

Search consumes keys and paste locally, including while the pane holds Control.
Type a query and Enter to search; arrows or `j`/`k` select results and Enter opens
history in a paused local view. `/` edits the query, Ctrl-C cancels while retaining
partial results, and Esc cancels/returns to the pane. Prefix commands still allow
detach, navigation and focus changes. Search supports 1024-byte queries and retains
at most 1000 results/2 MiB of previews, with explicit limit messages. Five rows are
required to open a result. See [search behavior and evidence](../engineering/tui-streaming-search.md)
for threading, continuity and history-coordinate limits.

The outer terminal owns fonts and text selection. Pause before selecting moving
output; mouse capture is deliberately disabled. Clipboard OSC 52 is off by
default, requires an explicit local command, exports at most 64 KiB, and depends
on the outer terminal's support/permission. Remote OSC/hyperlink/title bytes are
never forwarded as outer-terminal commands. There is no automatic clipboard write.

## Split and switching safety

Owner navigation now includes a Session groups view (`g`, or cycle with Tab).
It reads the runtime's names, pin/order state, detached state and available member
counts. Enter opens the group's current available Sessions; Backspace returns to
groups. Enter on a member uses the existing observe/split flow and never creates
or duplicates a process. Removed/empty groups remain empty, not an all-Sessions
fallback. Shared connections skip this owner-only view without changing scope.
On a group row, F2 renames, F3 edits order and `p` pins/unpins. Uppercase `D` chooses a member for
another view; on a Session row it opens that Session in another pane directly.
The rename draft accepts Unicode and paste up to 128 UTF-8 bytes; Enter saves,
Esc cancels and Ctrl-U clears. Edits run on one lazy background worker with one
action admitted at a time. Rename/pin use the displayed version and report
conflicts, with no automatic rebase or ambiguous-write retry. View duplication
creates another Surface of the same Session, with independent pause/history state,
not another group or process. This preserves the runtime's exclusive group
membership rule. Unconfirmed group edits identify
the attempted group so it can be checked before retrying. Detaching does not
undo an already admitted action. Order accepts decimal values from 0 through
4294967295, with lower values first within each pin tier and ID tie-breaking for
equal positions. This changes one group rather than atomically rearranging several
neighbors. Oversized input must be corrected before saving; it is not silently
submitted as a truncated value. These
source additions are unbuilt and untested at the owner's request.

Two panes maximum; ordinary opening of a Session already displayed focuses its
existing pane. Explicit `D` opens another Surface instead, within the same limit.
Splits are view attachments, never new processes. The focused pane is marked `>`
and identified by Session ID. Each pane owns independent pause/history state and
one bounded latest-frame mailbox. Its renderer clears only its own rectangle.

Entering navigation, replacing a Session, moving focus, or closing a pane returns
the old focused pane's Control first. The new focus always observes until an
explicit `Ctrl-] c`. A failed release prevents a target switch and disables input
locally; stale epochs cannot release another Surface's lease. Pending paste is
cancelled before a target change. Closing panes and detaching leave PTYs alive.

Only the focused controller resizes its host PTY. Side-by-side requires at least
41 columns and three rows; stacked requires at least seven rows. Below those
sizes, only the focused pane is shown full-size. Background Sessions keep their
existing grid, and the split reappears when the outer terminal grows. No mouse
dragging, arbitrary split trees, split-ratio editing, or layout persistence yet.

## Behavior and implementation

- One reverse-video status row per pane leaves the remaining space for canonical terminal
  cells. RGB/basic attributes, wide-cell continuation, combining graphemes and
  cursor position are preserved. Unchanged rows are not repainted. Blink, exact
  underline/cursor shapes and hyperlink activation are not reproduced.
- `FocusedSession` reuses `superplexr-client` for subscriptions, delta repair,
  history, Share checks, and per-Surface Control epochs. This client has a
  coalescing latest-frame mailbox rather than an unbounded display queue.
- Frame updates are event-driven. A 40 ms local input poll lets one UI thread
  check keys, signals, and the mailbox; it does not capture/poll the host screen
  or redraw at idle in the terminal view. Navigator metadata refreshes only while
  visible. Navigation refresh now uses one lazily created background worker with
  one replaceable request and one result slot. Tab/Mission changes discard stale
  results; closing navigation cancels queued work and stops further reads between
  native requests. An already running request retains the client's timeout, but
  the input thread does not wait for it. The worker sleeps while navigation is
  closed and is not created for a terminal-only visit. Search and result-history
  reads also use bounded background workers; other synchronous commands still have the existing client's
  bounded request timeouts; severe network stalls can delay input/detach.
- Observation clips the host's grid to the available display. Only a controlling
  view resizes the PTY, reserving one row for its status line. History is an
  independent bounded host read, never a shared viewport mutation.
  Page Up / `Ctrl-] h` now uses the existing history/search worker instead of
  blocking the input thread. The pane pauses immediately while loading; repeated
  paging replaces pending work. Returning live, switching panes or opening another
  local view discards the outstanding result. A received page must still match
  the active Session and its observed connection continuity.
- Reconnect uses the same Session and Share. It repairs the latest frame but
  disables input until Control is explicitly requested again. It never replays
  uncertain input or starts a replacement process. Intermediate screens may be
  coalesced; this is not complete event-by-event replay.
- Revocation clears retained local frame references and exits the TUI. It cannot
  retract text already seen or copied. Exit/error unwinding and SIGINT/SIGTERM/
  SIGHUP restore raw-mode settings, cursor, wrapping, paste mode, and the outer
  screen; SIGKILL cannot be cleaned up. Suspend/resume job control is not a tested
  contract. Run `stty sane` from another terminal if forcibly killed in raw mode.
- Normal dependencies include no GPUI, HTTP gateway, or runtime process. Shared
  terminal types still live in the Ghostty-linked terminal crate; this change
  does not extract a WASM-safe/types-only package or certify resource budgets.

## Verification

The navigation-worker and background history-paging changes above are implementation-only as of 2026-09-06;
no tests or builds were run at the owner's request. Prior evidence below does
not certify these changes, stalled-transport responsiveness or cancellation.

```sh
cargo test -p superplexr-tui
cargo clippy -p superplexr-tui --all-targets --all-features -- -D warnings
```

Ten unit tests cover search retention/routing/rendering, prefix/key translation, paste risk, safe rendering, row
reuse, split geometry/isolation, and outer-VT interpretation of wide/combining cells and cursor.
Thirteen integration tests use isolated real daemons; seven launch the real TUI in
an outer PTY. They exercise:

- scope/lease enforcement, stale input rejection after handoff, and detach that
  cannot release a different Surface's lease;
- keyboard input, observer-versus-controller resize, rejected/confirmed paste,
  zero idle repaint, detach/reattach with the original Session ID and PTY PID;
- outer terminal restoration on normal detach, SIGTERM, and Share revocation;
- a real interactive shell and `vi` saving wide/combining Unicode text;
- a forcibly dropped native link, output during outage, frame recovery, and the
  requirement to explicitly reacquire Control before any new input;
- scoped group names and Mission/Attention navigation, urgency order, unresolved
  approval preservation, and missing-primary-Session handling;
- independent input to two Sessions, release-before-focus, no process duplication,
  side-by-side/stacked layouts and compact-terminal resizing without background resize;
- revoked access in the navigator with no attached pane.
- streamed search, replacement/cancellation while backpressured or awaiting
  admission, Unicode query editing and paste isolation, history navigation,
  stale-preview rejection, and revoked access while search is visible.

The ignored `isolated_daemon` test is only a child-process fixture entry point.
Tests never use the person's existing daemon or workspace state. Linux, remote
SSH/tunnel conditions, clipboard acceptance, agent-specific hotkeys, long-running
stress, advanced keyboard protocols, mouse forwarding and job control still
need dedicated acceptance runs.

## Next slice

Navigator-driven Session attachment now runs on a bounded reusable worker in
source, with cancelled-intent fencing and off-thread retirement of replaced
views. This change is unbuilt and untested; see the
[attachment worker implementation](../engineering/tui-attachment-worker-implementation.md).
Explicit Control/input/resize now use a bounded ordered worker in source;
focus/close refuse to change views until asynchronous return is acknowledged.
This is also unbuilt and untested; see the
[ordered input implementation and remaining limits](../engineering/tui-ordered-input-implementation.md).
Pending focus/close/navigation now complete automatically after confirmed Control
return and cancel on changed Surface/continuity; see the unverified
[deferred view-change implementation](../engineering/tui-deferred-view-changes-implementation.md).

Next hardening: exercise this workflow over an authenticated remote link and on
Linux, and move metadata/command work off the input thread with bounded queues.
Desktop/browser search parity, stable history coordinates, explicit structured Attention responses, richer split management,
mouse forwarding and full workspace parity remain separate work. No new workflow
database, approval policy or scheduler belongs in this client.
