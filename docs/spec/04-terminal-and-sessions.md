# 04 — Terminal and Session contract

## 1. Ownership — T-OWN-001

Each running Session owns exactly:

- one leader child process and process group;
- one POSIX PTY master/slave pair until spawn completes, then the master;
- one canonical `TerminalModel` backed by public `libghostty-vt`;
- one accepted logical grid size;
- one Control lease and epoch;
- one raw output offset and one terminal frame sequence;
- zero or more client subscriptions.

The daemon parses every PTY byte exactly once. Desktop clients never replay raw
output to construct authoritative state.

## 2. Session creation — T-CREATE-001

`StartSession` accepts:

```text
mission_id, session_id, actor, name
program, argv, cwd, environment_delta
grid { columns, rows, cell_width_px?, cell_height_px? }
shell_mode { ordinary | login }
retention_profile
```

Validation MUST occur before creating the PTY. The working directory must exist
and be a directory. Environment keys cannot contain `=` or NUL; values cannot
contain NUL. Program resolution and all effective environment changes are shown
in the creation preview for agent-started Sessions.

Defaults:

- program is the user's configured shell, then `$SHELL`, then `/bin/sh`;
- arguments start an ordinary interactive shell, not a login shell;
- grid is 100 columns by 30 rows;
- `TERM=xterm-ghostty` with bundled terminfo;
- `COLORTERM=truecolor`, `TERM_PROGRAM=superplexr`, and the product version;
- UTF-8 locale is inherited; superplexr does not invent a locale.

The child becomes a session leader, gains the slave as controlling terminal,
duplicates it to stdin/stdout/stderr, and starts in its own process group. The
daemon closes the slave and all unintended descriptors after spawn.

## 3. Process and PTY lifecycle — T-PROC-001

- PTY reads continue when no client is connected.
- Output is journaled before or concurrently with terminal parsing without
  allowing disk latency to block the PTY indefinitely.
- Device replies emitted by the TerminalModel are written to the PTY through the
  same ordered Session actor as user input.
- Leader exit is recorded, then PTY output is drained until EOF or a bounded
  500-millisecond quiet timeout.
- Normal exit records its status or signal. Unknown loss records `Lost`, never a
  fabricated exit code.

User actions are distinct:

- `Interrupt` sends SIGINT to the foreground process group.
- `Terminate` sends SIGHUP, waits two seconds, sends SIGTERM, waits three more
  seconds, then offers—not automatically performs—SIGKILL.
- `Kill` sends SIGKILL after explicit destructive confirmation.

Session teardown MUST be idempotent. It finalizes raw output, terminal checkpoint,
process metadata, and the domain Session outcome even if a client disconnects.

### Recoverable archive — T-ARCHIVE-001

Archive is a reversible lifecycle state, not deletion. Only a terminal whose
process has stopped may be archived. Archiving MUST:

- remove the terminal from default indexes and active workspace projections;
- retain its launch specification, raw journal, final frame, searchability,
  domain Session identity, and Mission/Run association;
- persist through daemon and desktop restart using owner-only state; and
- publish an index update so all connected clients converge without polling.

Archived terminals remain discoverable through an explicit inclusive index and
the desktop Archive drawer. Restore removes the archive marker, republishes the
terminal, and reattaches it to its owning Mission workspace. Archive and restore
are idempotent. Permanent history deletion is a separate destructive operation
and is not implied by either action.

## 4. Ghostty seam — T-GHOST-001

The implementation MUST consume the public `libghostty-vt` interface, preferably
through an audited and pinned `libghostty-rs`. It MUST NOT embed Ghostty's private
application surface or let Ghostty own the PTY or product window.

`superplexr-terminal` owns all unsafe FFI, callback, allocator, render-state, and
version adaptation. Its public interface contains no raw pointer, borrowed
Ghostty object, GPUI object, or upstream enum.

### Terminal actions

- `Output { bytes, output_offset }`
- `Resize { grid }`
- `EncodeKey { key, modifiers, action, text? }`
- `EncodeMouse { kind, button, modifiers, cell, pixels? }`
- `Paste { bytes, confirmation }`
- `Focus { focused }`
- `SetViewport { anchor, rows }`

### Terminal effects

- PTY bytes to write back;
- changed rows and full-frame requirement;
- cursor, title, icon, palette, working-directory, mode, and bell changes;
- clipboard read/write request;
- unsafe-paste confirmation request;
- history compression or checkpoint work;
- protocol violations suitable for diagnostics.

Effects are data. The terminal module never opens URLs, writes the OS clipboard,
displays notifications, or performs process actions.

## 5. Grid and frame model — T-FRAME-001

Grid limits are 2–1,000 columns and 1–500 rows. Every accepted resize increments
the terminal frame sequence only after both PTY and terminal model accept it.

### FullFrame

A full frame contains:

```text
session_id, frame_sequence, output_offset
grid { columns, rows }
viewport { absolute_top, visible_rows, history_epoch }
rows[] { absolute_line, wrapped, cells[] }
cursor { row, column, shape, visible, blinking }
modes { alternate_screen, mouse, focus_reporting, bracketed_paste, kitty_keyboard }
title, icon, palette, default_colors
control_epoch
```

A cell contains a grapheme string, display width (`0`, `1`, or `2`), style-table
index, optional hyperlink-table index, and flags. Styles and hyperlinks are
interned once per message. Continuation cells have width zero and no independent
grapheme. All strings are valid UTF-8; invalid PTY bytes are handled by Ghostty's
terminal semantics before frame translation.

### FrameDelta

A delta contains `base_sequence`, new `frame_sequence`, latest `output_offset`,
complete replacements for changed visible rows, and optional replacements for
cursor, modes, title, palette, viewport mapping, and control epoch. Deltas never
contain partial cells. Applying a delta to any base other than `base_sequence`
is an error and triggers resynchronization.

Dirty changes are coalesced at most once per display interval, default 8 ms and
never more than 16 ms while output is active. Coalescing may replace many deltas
with one later delta, but may not create a sequence gap for a subscribed client;
when a queue drops state it sends `ResyncRequired` instead.

## 6. Attachment and projection — T-ATTACH-001

Attaching is a four-step protocol:

1. Client requests a Session and viewport with its last known sequence, if any.
2. Runtime responds with metadata and either an exact incremental continuation
   or `FullFrame`.
3. Client installs the full frame atomically, acknowledges its sequence, and
   becomes live.
4. Runtime sends ordered deltas and lifecycle events.

The runtime MUST buffer live updates between snapshot capture and acknowledgement.
The client MUST NOT display a delta over an unknown frame. A sequence gap, history
epoch mismatch, invalid row, or decode failure transitions the projection to
`resynchronizing`; the last valid frame remains visible with an indicator and
input is paused until an exact full frame arrives.

Multiple Surfaces in one desktop share one SessionProjection and glyph cache.
Each Surface owns only viewport, selection, focus, geometry, and observer state.

A Session group is a separate presentation object with stable identity, name,
order, pin state, and an ordered list of Session IDs. The runtime persists and
versions groups independently; changing group membership never reparents a
process, rewrites Run lineage, or merges PTY/model/history ownership.

## 7. Input — T-INPUT-001

Input messages include `session_id`, `control_epoch`, `client_sequence`, and one
logical input operation. The daemon validates control before encoding or writing.
Accepted client sequences are strictly increasing per control epoch. Duplicate
messages are acknowledged without repeating input; gaps reject the batch and
request client repair.

Key events preserve physical key, logical key, produced text, modifiers, press/
repeat/release action, and composing state. GPUI input MUST be rich enough to
support Kitty keyboard semantics; a lossy character-only adapter is a spike
failure.

Paste is one logical operation, capped at 8 MiB. It passes through the public
Ghostty paste safety and bracketed-paste behavior. Unsafe multi-line paste
requires an explicit confirmation unless trusted policy suppresses it. Pasted
content MUST NOT be logged separately from terminal input.

Mouse messages include cell and pixel coordinates. Observer Surfaces MAY select
and scroll locally but MUST NOT send application mouse reporting.

## 8. Resize — T-RESIZE-001

Only the current controlling Surface or controlling agent may request an
authoritative resize. The request includes control epoch, desired cells, and
optional pixel dimensions.

During window, tile, or waterfall animation:

1. retain the last accepted grid and clip or pad it;
2. wait 120 ms after geometry settles;
3. calculate whole cells using the active font metrics;
4. send only if rows or columns changed;
5. wait for `ResizeAccepted` before treating the new grid as authoritative.

The Session actor serializes PTY `TIOCSWINSZ`, terminal-model resize/reflow, and
frame publication. Hidden Mission tabs and observer Surfaces never resize PTYs.

## 9. Scrollback, selection, and search — T-HISTORY-001

Default scrollback is 100,000 logical lines per Session, configurable from
10,000 to 10,000,000 subject to retention limits. Alternate-screen content is
not added to normal scrollback unless terminal semantics require it.

Scrollback uses absolute line coordinates paired with a `history_epoch`.
Compression or eviction may change the oldest available line but not coordinates
inside the retained epoch. Requests outside retention return the nearest range
and an explicit truncation marker.

Selection endpoints use absolute line, cell column, and affinity. Copy produces
plain text by default and MAY produce HTML on explicit action. Rectangular and
semantic word/line selection are required. Selection is Surface-local.

Search executes in the daemon against canonical retained history, supports
literal Unicode text and case sensitivity in v1, and streams bounded result
pages. Regex search MAY follow. Search never blocks PTY consumption.

## 10. Clipboard, links, titles, and bells — T-SAFETY-001

- Copy requires a user gesture.
- Ordinary paste requires a user gesture or an explicit agent Grant.
- OSC 52 clipboard writes are denied by default, surfaced as a structured
  request, and bounded to 1 MiB decoded content.
- OSC 8 URLs are parsed and displayed but open only after user activation.
- URL schemes default to `https`, `http`, and `mailto`; others require
  confirmation.
- Titles are valid UTF-8, stripped of control characters, and capped at 1,024
  bytes. They may label a Surface but never rename durable identity silently.
- Bells create a low-noise activity mark. They do not become attention items,
  steal focus, or create OS notifications unless configured.

## 11. Rendering contract — T-RENDER-001

The TerminalElement MUST support:

- exact cell backgrounds and decorations;
- foreground glyph shaping with fallback and color emoji;
- wide graphemes and combining marks without column drift;
- optional ligatures that preserve cell advances and cursor hit testing;
- block, beam, and underline cursors with focus-aware blink;
- selection, search highlights, hyperlink hover, and IME preedit;
- display scale changes without corrupting grid coordinates.

Rendering MAY batch and cache aggressively, but MUST derive every visible cell
from one immutable RenderFrame. A clean frame performs no row translation and no
glyph upload. Damage is limited to changed rows plus cursor/overlay regions.

## 12. Terminal acceptance corpus — T-COMPAT-001

The same corpus MUST pass on macOS, Wayland, and X11:

- shell job control, foreground/background processes, signals, and exit;
- UTF-8, combining marks, ZWJ emoji, double-width and ambiguous-width cases;
- SGR colors/styles, hyperlinks, titles, palette changes, and bells;
- resize with wrapped-line reflow and alternate screen;
- Neovim, `less`, `htop` or equivalent, and shell line editing;
- mouse modes, focus reports, bracketed paste, and Kitty keyboard events;
- detach during high output, reconnect, sequence-gap resync, and history search;
- one controller plus at least three observers under contention.

Golden terminal semantics are compared before pixel rendering. A smaller set of
cross-platform pixel snapshots validates fonts, cursor, selection, and scaling.
