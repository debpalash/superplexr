# 06 — Desktop experience

## 1. Experience thesis — UX-THESIS-001

The application borrows a browser's orientation model and a control room's
information discipline. Each browser tab is one Mission; its sidebar is the
Mission's Session and attention index; its main area is a responsive waterfall
of live terminal instruments.

The recognizable frame lowers navigation cost. The deliberate visual risk is the
**signal gutter**: a narrow semantic timeline on every terminal Surface that
shows structured agent activity without overlaying or parsing terminal text.

```text
browser tab    = Mission view
sidebar row    = durable Session
waterfall tile = Surface attached to a Session
agent attempt  = Run, optionally using a Session
```

## 2. Application frame — UX-FRAME-001

```text
┌─ Mission A · 2 ─┬─ Mission B ──────┬─ ＋ ─────────────────────────┐
├─────────────────┴───────────────────┴──────────────────────────────┤
│ MISSION A                                      Delegate  Search ⌘K│
├────────────────────┬───────────────────────────────────────────────┤
│ NEEDS YOU 2        │ TERMINAL WATERFALL                            │
│ ◆ approve cleanup  │ ┌─ build · agent control ─────────────── 24 ┐│
│ ◐ choose package   │ │ $ cargo test                              ││
│                    │ └────────────────────────────────────────────┘│
│ SESSIONS           │ ┌─ shell · your control ─┐ ┌─ logs ────────┐│
│ ● build          2 │ │ $                      │ │ following…     ││
│ ● package          │ └────────────────────────┘ └────────────────┘│
│ ○ shell            │                                               │
│                    │                                               │
│ FINISHED           │                                               │
│ ✓ research         │                                               │
│ ＋ New session     │                                               │
├────────────────────┴───────────────────────────────────────────────┤
│ Type an action, session, run, file, or command…               ⌘K  │
└────────────────────────────────────────────────────────────────────┘
```

## 3. Mission tabs — UX-TABS-001

Each tab shows status glyph, intent, unresolved-attention count when nonzero, and
a quiet output-activity mark. Tabs are reorderable and pinnable presentation
state. Closing a tab detaches its view; it never completes or abandons a Mission.

`End mission` is named `Complete mission` when completion invariants pass and
`Abandon mission…` otherwise. Abandonment previews active Runs/Sessions and never
hides process termination inside the tab close button.

No Signal, bell, process exit, or background output activates a tab. High-risk
attention may trigger an OS notification under policy, but focus remains fixed.

The new-tab view lists Running Missions first, recently completed Missions next,
then `Create mission`. Empty state copy is: “Create a mission to keep its runs,
sessions, decisions, and results together.”

## 4. Sidebar — UX-SIDEBAR-001

The selected Mission owns one sidebar with groups:

1. **Needs you** — unresolved attention, ordered by high-risk approval, blocker,
   approval, input, then age.
2. **Sessions** — Running Sessions, grouped by active, idle, and lost.
3. **Finished** — Exited Sessions, collapsed by default.

A Session row shows name, lifecycle glyph, current Actor/Run when present,
attention count, control state, and low-noise activity tick. It MUST NOT show a
live output preview.

Click reveals an existing Surface or creates one. Clicking the already revealed
row focuses it. Drag places a Surface. The row menu contains `Rename`, `Reveal`,
`Open observer`, `Duplicate environment`, `View history`, and
`Terminate session…` as applicable.

Lineage may indent a Session beneath the Session hosting its parent Run by one
level, with at most two visible levels. The Run inspector shows deeper ancestry
and dependency edges. The sidebar does not pretend to be a graph editor.

Sidebar width is 208–320 px, defaults to 248 px, and collapses to a 52 px rail.
Attention counts remain visible in the rail.

## 5. Waterfall — UX-WATERFALL-001

The waterfall uses stable masonry placement:

| Main width | Columns | Gap |
|---|---:|---:|
| `< 760 px` | 1 | 8 px |
| `760–1279 px` | 2 | 8 px |
| `1280–1919 px` | 3 | 10 px |
| `>= 1920 px` | 4 | 10 px |

A Surface has a stable order key, preferred span of one or two columns, and
Compact (12), Standard (24), or Tall (40) terminal rows. Focus mode spans all
columns and uses available height. Placement fills the shortest compatible
column without changing keyboard or accessibility order.

Responsive changes preserve order and preferred span. Terminal output never
changes tile height, position, or order. Geometry follows the stable-grid resize
contract in section 04.

## 6. Surface anatomy — UX-SURFACE-001

Each Surface contains:

- 28 px header with Session name, Actor/Run, connection state, controller, grid
  size, and actions;
- 3 px signal gutter;
- terminal viewport;
- transient search or IME overlay that never covers the active cursor without
  repositioning.

Header actions are `Focus`, `Span`, `Height`, `Open observer`, `Close surface`,
and the separately styled `Terminate session…`. Closing and terminating cannot
share a glyph or confirmation flow.

The signal gutter uses short ticks: Relay for progress, Signal amber for human
attention, coral for failure, and Chalk for Artifact. A tick opens structured
context with Actor, reason, evidence, related terminal range, and concrete
actions. Decorative continuous animation is forbidden.

An observer has an `Observer` label, no terminal cursor, no application mouse
input, and a `Request control` action. Clicking or focusing it never steals
control.

A Controller Share has a `Shared control` identity and may acquire Control only
when the scoped Session is unoccupied. It renders the normal input cursor only
while its lease is current. The desktop MUST NOT offer force takeover,
termination, archive, Mission mutation, scheduling, diagnostics, or Share
administration to this role. Revocation returns the Surface to a disconnected
read-only state without ending the Session.

## 7. Attention workflow — UX-ATTENTION-001

Attention appears in three synchronized places:

- total count in Mission tab;
- actionable item in `Needs you`;
- locality mark on Session row and visible Surface gutter.

Selecting an attention item is an explicit navigation action: select its Mission,
reveal or create the related Surface, scroll it into view, and open a context
sheet. The sheet shows exact request, requesting Actor, risk, evidence, Grant
enforcement mode, relevant terminal excerpt, and actions.

Actions use exact semantics:

- input: `Send response`, `Cancel`;
- approval: `Deny`, `Allow once`, optional scoped duration when enforceable;
- blocker: `Send guidance`, `Take control`, `Cancel run`;
- successful result: `Accept result`, `Reject result`.

Cooperative authorization is labeled `Agent-reported request`; enforced policy is
labeled `Runtime-enforced`. The same colors never imply both.

## 8. Run graph inspector — UX-GRAPH-001

The graph inspector is opened from a Mission header or Run detail. V1 presents a
read-only causal map plus actions; it is not a freeform canvas.

- solid edges show lineage;
- arrowed dashed edges show dependencies;
- nodes show phase, Actor, Session, outcome, disposition, and attention;
- selecting a Run reveals its Session, Signals, Artifacts, and history;
- ready/waiting/blocked reasons are stated in text, not color alone;
- `Cancel descendants…` previews the exact set and remains explicit.

Graph layout is deterministic for the same projection. Keyboard and list views
provide all information represented visually.

## 9. Command Deck — UX-DECK-001

The bottom deck and global shortcut open one searchable action surface. Results
are grouped by actions, Missions, Sessions, Runs, Artifacts, files, then shell
commands. Executing a shell command always names or creates the target Session;
the deck never spawns an untracked process.

Potentially destructive results end in an ellipsis and open a preview. Search
may find retained terminal history but MUST label the Session and timestamp.

## 10. Keyboard and focus — UX-FOCUS-001

| Action | macOS | Linux |
|---|---|---|
| Command Deck | `Cmd-K` | `Super-K` |
| Mission 1–9 | `Cmd-1…9` | `Super-1…9` |
| Previous/next Surface | `Cmd-Shift-[ / ]` | `Super-Shift-[ / ]` |
| Toggle focus mode | `Cmd-Shift-Enter` | `Super-Shift-Enter` |
| Find in Session | `Cmd-F` | `Ctrl-Shift-F` |
| New Session | `Cmd-T` within Mission | `Ctrl-Shift-T` |

Application shortcuts using Cmd/Super are resolved before terminal input.
Platform terminal conventions such as Ctrl-C remain PTY input. Focus transfer
and Control transfer are separate: Enter on a sidebar row focuses/reveals but
does not take control.

Focus MUST restore to the initiating control after closing a sheet. Reconnection
MUST NOT synthesize key-up/down events or replay buffered keys.

## 11. Responsive behavior — UX-RESPONSIVE-001

- Minimum window is 720 × 560 px.
- Below 980 px, sidebar starts collapsed and opens as an overlay.
- One column becomes a stable vertical stack.
- Attention uses a bottom sheet below 1,180 px and anchored side sheet above.
- Tabs scroll horizontally; pinned tabs remain left.
- There is no mobile-specific design in v1.

## 12. Visual system — UX-VISUAL-001

The appearance is quiet industrial instrumentation, not a generic neon terminal.

| Role | Token | Value |
|---|---|---|
| app/terminal surround | Deck | `#10151B` |
| sidebar and headers | Slate | `#19212A` |
| primary text | Chalk | `#E8EDF2` |
| secondary/inactive | Trace | `#8795A5` |
| focus/progress | Relay | `#79A7D3` |
| attention | Signal | `#D8A85B` |
| failure/destructive | Fault | `#D7776B` |

Familjen Grotesk carries product text; Iosevka Term carries terminal grids and
compact technical data. Both are bundled and licensed for redistribution before
release. Corners use 3 px radius. There are no gradients, floating card shadows,
glass effects, or decorative terminal scanlines.

Focus is a 2 px external Relay outline. Text/background pairs MUST pass WCAG AA;
status never relies on color alone. A user theme MAY remap application semantic
tokens and terminal typography only when the complete data-only theme validates
before activation. Chalk on Deck and Panel MUST meet WCAG AA; Relay, Signal, and
Fault on Deck MUST meet 3:1 non-text contrast. Invalid or partially specified
themes leave the last valid theme active. Terminal ANSI palette customization is
a separate terminal-model capability and MUST NOT be inferred from application
token remapping.

## 13. Motion — UX-MOTION-001

Motion is limited to explicit layout transitions and one 160 ms signal-tick
arrival. Terminal output supplies its own motion. Reduced-motion mode makes
placement and signals immediate and disables cursor blink when the OS preference
requires it.

No background Surface pulses, auto-scrolls its containing waterfall, or animates
an activity graph.

## 14. Accessibility — UX-A11Y-001

- Full operation is possible without a pointer.
- Accessibility order is tabs, Mission header, sidebar, then stable Surface order.
- Each Surface exposes Session name, controller, grid dimensions, selection, and
  visible terminal text without creating one accessibility node per cell.
- Live terminal output is not announced continuously. The user can enable an
  explicit “follow terminal” mode for the focused Surface.
- Attention changes use polite announcements; high-risk requests use assertive
  announcements only when user policy permits.
- Focus, controller, observer, waiting, blocked, and lost states have text labels.
- 200% interface scaling works at the minimum window size.

Screen-reader acceptance is required on macOS VoiceOver and Linux Orca under both
Wayland and X11.

## 15. Empty, loading, error, and disconnected states — UX-STATE-001

- Loading preserves existing content and shows progress in its locality; it does
  not replace the entire window with a spinner.
- Disconnected Surfaces retain the last valid frame, label it `Disconnected`, and
  reject input. Reconnecting changes to `Resynchronizing` until a FullFrame lands.
- Lost Sessions show the final frame and `Process continuity was lost when the
  runtime stopped`; actions are `View history` and `Start replacement session`.
- Corrupt state shows the affected Mission/Session, preserved recovery path, and
  `Export diagnostics`; it never silently starts empty.
- A Mission with no Sessions offers `New session` and `Delegate run` with one
  sentence explaining their difference.

## 16. Presentation persistence — UX-PERSIST-001

Per device, the desktop stores tab order/pins, sidebar width/groups, revealed
Session IDs, Surface order/span/height, focused Surface, waterfall scroll, theme,
and non-sensitive notification settings. Presentation data MAY be discarded
without affecting domain or process state and MUST NOT enter the Mission event
log.
