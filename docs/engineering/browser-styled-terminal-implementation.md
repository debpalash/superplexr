# Browser canonical styled terminal projection

Status: implemented in source, unbuilt and untested. No browser checks, tests,
builds, or resource measurements were run for this change, per owner direction.

The browser previously reduced every canonical frame to plain text. Snapshots
now optionally include a bounded `display` projection derived from the same
runtime-owned `FullFrame`. Live output, retained history, and revealed history
search results use it through one renderer. The original `text`, `rows`, status,
and sequence fields remain intact for existing consumers and plain-text copy.

## Rendering contract

- Resolved RGB foreground/background and default terminal colors come from the
  canonical frame, not an independent browser ANSI parser or palette.
- Bold, italic, faint, inverse, concealed glyphs, strike, overline, and single,
  double, curly, dotted, and dashed underline use fixed CSS properties. Unknown
  underline styles fall back to a single solid line.
- Adjacent narrow cells of one style share a fixed-column span. Each wide
  grapheme has its own two-column span; width-zero continuation cells do not
  render a second glyph. Glyph weight/style does not alter the outer column box.
  Ligatures and kerning are disabled, and runs use explicit left-to-right visual
  ordering rather than browser paragraph bidirectional reordering.
- Cursor position and block/bar/underline/hollow-block shape are projected as a
  non-selectable CSS overlay. Unknown shapes use a block. Local paused/history
  views and ended output hide it. Cursor and text blinking are intentionally
  stationary: there are no animation timers or flashing text.
- Only changed row signatures or style-table changes rebuild row contents;
  cursor updates do not replace terminal text. This is an implementation choice,
  not a measured latency or selection-stability guarantee.
- All terminal strings enter through `textContent`. Hyperlinks are not rendered
  as anchors, terminal strings cannot supply CSS/HTML, and no remote fonts,
  images, scripts, or frontend dependencies are added.
- Copy output uses the original canonical text, preserving its row separators
  and trailing-space behavior independently of the styled DOM. An explicit
  browser selection still takes precedence. Concealed text has the same copy
  semantics as the prior plain-text projection; concealment is not redaction.

## Bounds and fallbacks

The styling projection accepts at most 400 columns, 200 rows, 80,000 canonical
cells, and 4,096 styles. Grapheme data and a conservative estimated serialized
styling budget are each capped at 2 MiB. Adjacent runs coalesce; this is not an
unbounded scrollback DOM. The original text payload and existing native/HTTP
limits remain separate; these new styling bounds do not establish an overall
end-to-end memory budget.

Unsupported grids/styles/size bounds omit styling without resizing or mutating
the host terminal. Missing or malformed browser styling falls back to canonical
plain text and labels the live view as text-only. Authority changes continue to
clear both renderer state and retained copy text.

The existing slate UI chrome, system UI typeface, monospace output, and responsive
layout remain unchanged. The visible change is the terminal's own canonical
appearance, not a new app theme or decorative animation.

## Not yet established

This is not full browser terminal parity. Font fallback metrics, combining
clusters and emoji, precise selection across styled runs, wrapped-line selection,
underline geometry, cursor inversion/wide-cursor coverage, browser zoom, screen
reader output, CSP CSSOM behavior, and every supported browser need deferred
acceptance. Mouse forwarding, OSC hyperlink activation, graphics, full IME/key
parity, native delta transport to the browser, and rich workspace layouts remain
separate work. No input authority, automatic resize, or terminal parser was added.

Future acceptance must also cover malformed styling and bounds, unchanged-row
reuse, empty and alternate-screen output, non-ASCII cell widths, history/search
rendering, pause/resume and exit cursor state, canonical copy, access revocation,
and sustained-output CPU/memory/network costs. Prior text-only evidence cannot
certify this implementation.
