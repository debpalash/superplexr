# Browser tabs and terminal waterfall — v1

## Design resolution

The desktop uses a familiar browser frame to organize unfamiliar agentic work:

```text
browser tab    = one Mission
sidebar row    = one durable Session
waterfall tile = one Surface attached to a Session
agent attempt  = one Run, optionally using a Session
```

This separation is structural. Closing a tab does not end its Mission, closing a
tile does not end its Session, and a Session can remain useful before or after an
agent Run. Ending work always uses an explicit verb such as `End mission`,
`Terminate session`, or `Cancel run`.

## Primary workspace

```text
┌─ ◉ Fix Linux renderer · 2 ─┬─ Package v1 ─────┬─ Research ────────┬─ ＋ ───┐
├────────────────────────────┴───────────────────┴───────────────────┴────────┤
│ FIX LINUX RENDERER                                    Delegate     Search ⌘K│
├────────────────────┬─────────────────────────────────────────────────────────┤
│ NEEDS YOU  2       │ TERMINAL WATERFALL                                      │
│ ◆ approve cleanup  │                                                         │
│ ◐ package choice   │ ┌─ researcher · agent controls ──────────────── 24 rows─┐│
│                    │ │ $ rg "GHOSTTY_PLATFORM"                              ││
│ SESSIONS           │ │ ...                                                   ││
│ ● researcher     2 │ └───────────────────────────────────────────────────────┘│
│ ● linux-build      │ ┌─ linux-build · you control ─┐ ┌─ shell ──────────────┐│
│ ◐ package-check  1 │ │ $ cargo test                │ │ $ git status          ││
│ ○ shell            │ │ ...                         │ │ ...                   ││
│                    │ └──────────────────────────────┘ └───────────────────────┘│
│ FINISHED           │                    ┌─ logs · observer ──────────────────┐│
│ ✓ protocol         │                    │ following build output…            ││
│                    │                    └─────────────────────────────────────┘│
│ ＋ New session     │                                                         │
├────────────────────┴─────────────────────────────────────────────────────────┤
│ Type an action, session, run, file, or command…                         ⌘K   │
└──────────────────────────────────────────────────────────────────────────────┘
```

The browser frame provides orientation; the staggered terminal field is the
product's expressive element. There is no separate Mission Spine or permanent
Attention Dock. Mission-level attention is folded into the tab badge and the top
of that tab's sidebar, leaving the main area for embedded terminals.

## Mission tabs

Each browser-style tab is a live view of one Mission. It contains:

- a mission status glyph;
- the human-readable mission intent, truncated from the middle when necessary;
- an unresolved-attention count, shown only when nonzero;
- a quiet activity mark when detached Sessions are producing output;
- a close control that detaches the view without ending the Mission.

Tabs can be reordered and pinned as local presentation state. Reordering does not
create domain events. A closed active Mission remains discoverable from the new
tab page under `Running missions`. `End mission` lives in the tab context menu and
requires confirmation when active Sessions exist.

No new signal activates another tab automatically. A high-priority item changes
the badge and may produce an operating-system notification, but focus remains
where the human left it.

## Per-tab Session sidebar

The sidebar belongs to the selected Mission and uses three groups:

1. **Needs you** — unresolved attention items, linked to their Run and Session;
2. **Sessions** — live and idle terminal Sessions;
3. **Finished** — recently exited Sessions, collapsed by default.

A Session row shows its name, process/actor glyph, lifecycle, attention count,
and a low-noise activity tick. It never shows a continuously changing output
preview. That motion would turn twelve agents into twelve distractions.

Interactions:

- click a row to reveal its existing Surface or add one to the waterfall;
- click the revealed row again to focus its terminal;
- drag a row to place a Surface at a particular waterfall position;
- use the row menu to rename, duplicate environment, open an observer, restart,
  or terminate;
- drag the sidebar edge between 208 and 320 px;
- collapse it to a 52 px status rail without losing attention counts.

Rows preserve causal information without becoming a graph. A Session associated
with a child Run is indented one level beneath the Session that hosted its parent
Run, with a maximum of two visible nesting levels. Deeper ancestry appears in the
row inspector.

## Terminal waterfall

The main content area is a stable responsive masonry layout of embedded terminal
Surfaces. “Waterfall” means vertically stacked columns with staggered tile ends;
it does not mean terminals resize continuously in response to their output.

### Placement

The layout calculates columns from available width:

| Main-area width | Columns | Default tile span |
|---|---:|---:|
| below 760 px | 1 | 1 |
| 760–1279 px | 2 | 1 |
| 1280–1919 px | 3 | 1 |
| 1920 px and above | 4 | 1 |

Each Surface has a stable order key, a width of one or two columns, and one of
three grid-height presets: Compact (12 rows), Standard (24 rows), or Tall (40
rows). Focus mode temporarily spans all columns and consumes the available
height. The waterfall fills the shortest compatible column without changing
keyboard or accessibility order.

Dragging changes only local view state. When the window crosses a breakpoint,
tiles retain their order and preferred span, then fall into the nearest valid
arrangement. A two-column tile becomes one column only when no two-column slot is
possible.

### Surface anatomy

```text
┌─ session name · actor/run ───────── controller ───── size ── actions ─┐
│▏ terminal grid                                                     │
│▏                                                                   │
│▏                                                                   │
└────────────────────────────────────────────────────────────────────┘
```

The 3 px signal gutter at the left edge is the signature visual element. New
structured signals add short semantic ticks: Relay for progress, amber for human
attention, coral for failure, and Chalk for an artifact. The gutter summarizes
recent agent events without overlaying or parsing terminal text. Selecting a tick
opens its structured context.

The header contains the Session name, associated Actor/Run, connection state,
controller, grid dimensions, and actions for focus, span, height, observe, close
Surface, and terminate Session. Destructive actions never hide under the same
close glyph used for a Surface.

### Stable-grid resizing

Responsive masonry can damage terminal usability if every animation produces a
PTY resize. The layout therefore uses a stable-grid protocol:

1. During drag, window resize, or breakpoint animation, the Surface keeps its
   last terminal grid and clips or pads it inside the changing tile.
2. After geometry settles for 120 ms, the controlling Surface calculates the new
   whole-cell grid.
3. It sends one authoritative resize to the daemon only when rows or columns
   actually changed.
4. The daemon resizes the Session PTY and canonical `libghostty-vt` model, then
   broadcasts the accepted grid.
5. Observer Surfaces render that accepted grid with padding; they never resize
   the Session.

Focus mode bypasses the debounce after its transition ends. Hidden tabs and
occluded tiles never issue resize events. This prevents responsive layout from
thrashing shells, Neovim, `htop`, and other alternate-screen programs.

### Multiple Surfaces for one Session

A Session may appear once as the controlling Surface and any number of times as
an observer. Only the Control lease holder accepts keyboard, paste, or authoritative
resize input. Observer headers say `Observer` and use a distinct cursor-free
rendering mode. Selecting an observer offers `Request control`; it never steals
the lease implicitly.

## Attention inside the layout

Attention has three coordinated representations:

- the Mission tab shows the total unresolved count;
- the sidebar's `Needs you` group shows actionable items;
- the related Session row and visible Surface signal gutter show locality.

Selecting an item explicitly activates its Mission tab, reveals or creates the
related Surface, scrolls it into view, and opens a context sheet anchored to the
tile. Automated signals do none of these focus-changing actions.

The sheet shows the exact question or operation, the Actor's reason, relevant
terminal excerpt or artifact, and concrete verbs such as `Deny`, `Allow once`, or
`Send response`. Cooperative approval remains labeled `Agent-reported` until an
execution sandbox can enforce it.

## Run and Session behavior

A new Session starts as an ordinary shell unless created for a specific Run. A
Run can be:

- **interactive**, with a primary Session shown in the sidebar;
- **headless**, with signals and artifacts but no Session;
- **sequential**, reusing an existing shell Session after an earlier Run ends.

Delegating an interactive child Run creates a new Session by default, inherited
from the parent's working directory and environment. The human may instead choose
an existing idle Session or a new isolated Git worktree. The sidebar names the
Session after its current Run but preserves a stable Session identity and history.

Process exit ends the Session lifecycle; it does not automatically accept or
reject the associated Run's disposition. A finished Session remains reopenable
from its final snapshot and output history until retention removes it.

### Prototype alignment

The core domain now has stable `SessionId` values, Session lifecycle events,
sequential Run assignment, and a Session-owned Control lease. A Run references
an optional primary Session; Session exit does not finish or judge the Run; and
Mission completion checks unfinished Runs and live Sessions independently.

The control protocol is version 2. Its remaining PTY slice must route attach,
input, resize, full-frame, delta, and resynchronization messages by `SessionId`.
Signals and disposition remain on Runs and resolve their terminal locality
through the Session association. Legacy prototype Run-control events remain
readable but have no Session effect after migration.

The `RunStatus` decomposition identified in the earlier design review remains a
separate projection change; it must land before agent orchestration is called
complete.

## Keyboard and focus model

- `Command-K` on macOS and `Super-K` on Linux opens the global Command Deck.
- `Command/Super-1…9` selects a Mission tab.
- `Command/Super-Shift-[` and `]` move among visible Surfaces.
- `Command/Super-Shift-Enter` toggles focused/full-waterfall mode.
- Arrow keys navigate the sidebar; Enter reveals a Surface without transferring
  terminal control.
- Terminal keystrokes go to the PTY only when the Surface is focused and holds
  the Control lease.

The terminal never consumes application shortcuts using Command/Super. Every
mouse operation has a keyboard equivalent. Accessibility order follows tab strip,
sidebar, then stable Surface order—not current masonry coordinates.

## Responsive behavior

- At widths below 980 px, the sidebar starts collapsed and opens as an overlay.
- At one column, the waterfall becomes a vertical stack; tile order is unchanged.
- Attention context uses a bottom sheet below 1180 px and a tile-anchored side
  sheet above it.
- Browser tabs remain horizontally scrollable, with pinned Missions fixed left.
- There is no mobile layout in v1; the minimum supported window is 720×560 px.

## Visual system

The browser chrome is deliberately familiar and quiet. Identity comes from the
staggered field of terminal instruments and their signal gutters, not ornamental
window furniture.

| Role | Token | Value |
|---|---|---|
| App and terminal surround | Deck | `#10151B` |
| Sidebar and tile headers | Slate | `#19212A` |
| Primary text | Chalk | `#E8EDF2` |
| Secondary labels and inactive tabs | Trace | `#8795A5` |
| Focus, progress, and selected tab | Relay | `#79A7D3` |
| Attention and failure | Signal | `#D8A85B` / `#D7776B` |

Familjen Grotesk carries Mission tabs, Session names, and interface copy. Iosevka
Term carries terminal grids, identifiers, dimensions, timestamps, and compact
states. Both are bundled for macOS/Linux consistency. Tile corners use 3 px radii;
there are no floating card shadows or gradients. Focus uses a two-pixel Relay
outline outside the terminal grid so it never covers a cell.

Motion is limited to explicit layout changes and one 160 ms signal-tick arrival.
New terminal output itself provides enough motion. Reduced-motion mode removes
the signal transition and uses instant masonry placement.

## View-state persistence

The client stores the following per Mission and per device:

- tab order and pinned tabs;
- sidebar width and collapsed groups;
- revealed Session IDs;
- Surface order, span, and height preset;
- last focused Surface and waterfall scroll position.

This state is presentation-only and may be discarded without affecting Missions,
Runs, Sessions, PTYs, signals, or artifacts. It does not enter the domain event
log or synchronize between clients in v1.

## Acceptance scenarios

1. Open three Mission tabs; each retains a different Session sidebar and
   waterfall arrangement.
2. Run twelve Sessions in one Mission while showing six Surfaces across three
   columns; hidden Sessions continue without output loss.
3. Resize from three columns to one while Neovim is open; the Session receives one
   settled PTY resize rather than an animation's worth of intermediate sizes.
4. Open the same Session as controller and observer; only the controller can type
   or resize it.
5. Receive attention in a background Mission; its tab and sidebar update without
   switching tabs or moving terminal focus.
6. Close a Surface, then its Mission tab; the Session continues and restores from
   the sidebar after reopening the Mission.
7. Terminate a Session deliberately; associated Run history and artifacts remain
   reviewable.
8. Complete the same journey using only the keyboard and a screen reader on
   macOS, Wayland, and X11.

## Explicit constraints

- A Surface is never called a Session in code or copy.
- A browser tab is never the lifetime owner of a Mission.
- Masonry placement never changes PTY size until geometry settles.
- Terminal output never determines tile height or order.
- Attention indicators never activate a tab or Surface automatically.
- Closing view chrome never terminates durable work.
