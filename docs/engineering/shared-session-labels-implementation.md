# Project/task labels in the shared session catalog

Status: source implemented, unbuilt and untested. Tests, builds, validation,
terminal/browser runs, and resource measurements remain deferred by owner
direction. Existing fixture constructors were adapted for the added fields;
those fixtures were not executed.

The native `TerminalSessionSummary` now carries optional `display_title` and
`display_directory` fields. They are sanitized, bounded display strings derived
from canonical terminal frames, not user-authored names or trusted paths.
Control characters and directional-formatting controls are removed. The title
is limited to 512 UTF-8 bytes and directory to 2,048 bytes, with an ellipsis for
truncation; a few bytes are reserved for that marker. Original frame metadata
is unchanged. Never use the display directory for execution or filesystem I/O.

Live launch and retained-session recovery initialize this metadata from their
frames. The terminal projection refreshes it on title/directory changes and
publishes a collection update only when the bounded display projection changes.
Ordinary output does not publish additional catalog updates. Comparing the
previous frame's metadata avoids allocating fresh labels for unchanged frames.
No per-session polling timer or additional terminal subscription is introduced.

## Client presentation

- The browser session feed carries the two labels within its existing complete
  snapshot and incremental-update stream. Native Share filtering remains the
  boundary: this does not list unrelated Missions, groups, commands, or paths.
- Browser primary labels prefer terminal title, then directory basename, then
  foreground executable, then short Session ID. Executable and status remain
  visible as secondary text; the tooltip includes the Session ID to distinguish
  duplicate names. Labels are rendered as text, never HTML.
- Browser label updates patch keyed buttons instead of rebuilding navigation.
  Unchanged membership/order retains button identity and keyboard focus. The
  existing 4,096-entry limit and complete-snapshot removal rules remain.
- TUI navigation preserves explicit group and Mission Session names first.
  Otherwise it uses the same title/project fallback before executable name.
  This updates when the existing navigation refresh runs; it does not add a
  live TUI metadata subscription or overwrite saved names.
- Desktop surfaces retain catalog labels and their frame sequence separately
  from canonical frames. Automatic naming uses catalog metadata only when it
  is newer; explicit names and bound Run objectives retain precedence. Removal
  clears the catalog labels. Display directories never enter the accessor used
  for shell launch. This ordering is not an input-authority or restart check.
- MCP `terminal_list` includes display labels without capturing each terminal.
  A 256 KiB allowance counts pretty-encoded entries, escaping, and indentation
  headroom. Pages may contain fewer entries than requested; `byte_limited` and
  `next_after` describe continuation. The first omitted entry remains eligible
  after that cursor. The full native list is still decoded before paging, pages
  remain non-transactional, and outer tool-response limits still apply.

These are terminal-authored labels, not inferred agent task summaries. A
terminal that provides neither title nor directory metadata still falls back to
its executable/ID. A title can be stale or misleading process output; it conveys
no ownership, approval, completion, or identity guarantee. This change does not
add browser rename/pin/group-management permission or alter explicit names.

## Compatibility and deferred acceptance

The fields are additive JSON metadata with deserialization defaults. Older
servers yield absent labels; the browser accepts absent optional fields too.
The protocol version and command surface are unchanged. Rust struct constructors
must include the new fields. No compatibility or performance acceptance has
been run for this source extension.

Future coverage must include OSC title/directory changes without output polling,
metadata clearing, long/non-ASCII/control-containing labels, retained recovery,
Share scope, snapshot reconciliation, explicit-name precedence, duplicate names,
keyboard focus retention, and unchanged-output catalog silence. Measure the
additional per-entry metadata cost and sustained title-change workloads before
claiming the aggregate tiny-footprint or platform targets are met.
The desktop/MCP extension also requires delayed frame/catalog ordering, explicit
rename preservation, launch-directory separation, byte-limited cursor progress,
and escaping-heavy response coverage. It is likewise unbuilt and untested.
