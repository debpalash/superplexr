# Zoom layout implementation

Date: 2026-09-06 IST

Status: implemented in the working tree. The owner has resumed validation;
focused GPUI regressions have been added and await the coordinated test run.
Earlier passing suites and release binaries do not certify these changes.

## Report and source finding

The supplied screenshot shows enlarged sidebar labels clipped inside their rows
and cramped titlebar controls after Cmd +/−. `AppZoom` changed the window's rem
size, and text followed it, but many controls and containers retained fixed pixel
dimensions. The old zoom test checked the rem value, not layout containment.

## Implementation

- Convert design dimensions in the titlebar, sidebar, row controls, menus,
  provider panels, Fault panels and terminal overlays to root-relative lengths.
- Scale named row/icon/footer dimensions as well as numeric style values.
- Calculate tab capacity and overflow using the viewport divided by zoom.
- Calculate responsive terminal columns and status-widget visibility in that
  same design coordinate space.
- Store sidebar width in design units and convert pointer coordinates before
  applying drag limits. Render the calculated sidebar width through rem units.
- Keep native macOS traffic-light reservation and its minimum titlebar height
  in physical layout pixels. Keep measured scrollback/scrollbar geometry and
  terminal input coordinates in their existing pixel coordinate space.
- Bound Mission graph, attention, termination, provider settings, plugin,
  Fault and command panels against the current window viewport at render time.
  Their design dimensions still scale with zoom, but modal width is capped at
  95% of the viewport and height at 90%; no cached window bounds are introduced.
- Scroll long attention/termination dialogs and command actions instead of
  clipping their lower controls. Limit graph/Fault list-column width so the
  details area retains space in narrow windows. Graph depth indentation now
  scales in the same design units as its rows.
- Keep workspace overflow within the width of the window and the vertical space
  remaining below its titlebar anchor. The command deck reserves a proportional
  top margin rather than an unbounded zoom-scaled offset.
- Use GPUI's measured, window-aware anchoring for workspace action menus, workspace
  rename editors, session action menus and row termination confirmations. Popup
  offsets scale from their owning row/tab; content width and scrollable height
  are bounded by the current viewport. Session menus no longer infer an upward
  opening direction from the number or index of sessions, which ignored actual
  scroll position and zoom. Menu action rows retain their natural block heights
  when the menu scrolls rather than being squeezed by a flex column.

## Regression coverage

`cargo test -p ultraplexr-desktop --release --all-features --locked --offline zoom`
exercises the root-relative metrics at every zoom stop, actual sidebar row/name/
detail containment, nested terminal row heights, collapsed dimensions, reset,
small-window menu bounds and unsqueezed action rows, rename/termination bounds,
overlay sizing after resize, responsive terminal columns, and sidebar drag
conversion to persisted design units. These tests use GPUI's isolated test
windows and fixture surfaces; they do not launch or connect to the owner's app.

## Remaining acceptance

Exercise every zoom stop and reset with
sidebar expansion, nested rows, rename/context menus, provider/Fault panels,
tab overflow and manual sidebar/splitter dragging. Check small windows and
native platform controls as well as the supplied full-window scenario. Verify
terminal input/cursor/selection and resize coordinates after zoom. Validate
the implemented overlay and popup containment, header/action wrapping and keyboard
reachability; no all-platform or accessibility claim is made.
