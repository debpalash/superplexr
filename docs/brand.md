# SuperPlexr brand guide

## Brand idea

SuperPlexr makes parallel agent work legible and controllable. The mark shows four
durable work streams converging through one control rail into a single decision
point. It comes from the product's terminal waterfall and signal-gutter system;
it is not a generic terminal prompt or AI symbol.

## Naming

- **SuperPlexr** is the display name in headings, prose, app metadata, and launch
  material.
- **superplexr** is the technical name for repositories, crates, binaries,
  commands, environment paths, and configuration keys.
- Do not write `Superplexr`, `Super Plexr`, `SuperPlexer`, or abbreviate the public
  name to `SP`.

The primary line is **Mission control for coding agents.** Use it as a concise
descriptor, not as part of the product name.

## Logo

The primary lockup is [superplexr-lockup-dark.svg](assets/brand/superplexr-lockup-dark.svg).
Use the standalone [color mark](assets/brand/superplexr-mark.svg) when the name is
already present. The [monochrome mark](assets/brand/superplexr-mark-mono.svg) uses
`currentColor` for one-color applications.

Keep clear space around the mark equal to the diameter of its decision node. Do
not rotate the mark, recolor individual paths, add effects, or place it on a busy
background. At sizes below 24 px, use the monochrome mark.

The repository social card is
[social-preview.png](assets/launch/social-preview.png). Its editable overlay is
[social-preview-overlay.svg](assets/launch/social-preview-overlay.svg). Once the
repository is public, upload the PNG unchanged through GitHub's repository social
preview control.

The launch gallery uses captures from GPUI's real compositor with deterministic
demo data. Use [superplexr-desktop-demo.gif](assets/launch/superplexr-desktop-demo.gif)
as the repository walkthrough. Use the framed [terminal waterfall](assets/launch/desktop-waterfall-framed.png),
[command deck](assets/launch/desktop-command-deck-framed.png), and
[focus mode](assets/launch/desktop-focus-framed.png) stills for release listings
and announcements. The editable wallpaper and window frame are in
[desktop-wallpaper.svg](assets/launch/desktop-wallpaper.svg).

## Color

| Token | Hex | Use |
|---|---|---|
| Deck | `#10151B` | Primary dark background |
| Slate | `#19212A` | Panels and secondary surfaces |
| Chalk | `#E8EDF2` | Primary type and mark geometry |
| Trace | `#8795A5` | Secondary type and inactive state |
| Relay | `#79A7D3` | Focus, progress, and control rail |
| Signal | `#D8A85B` | Human attention |
| Fault | `#D7776B` | Failure and destructive state |

Relay is the brand accent. Signal and Fault are semantic colors and should remain
small. Avoid decorative gradients, neon terminal green, and multicolor effects.

## Typography

- **Brand and launch display:** Avenir Next Demi Bold, with Avenir or a neutral
  grotesk fallback.
- **Product interface:** Familjen Grotesk.
- **Terminal, identifiers, and technical data:** Iosevka Term.

Use sentence case for interface and documentation headings. The mixed-case
`SuperPlexr` wordmark is the deliberate exception.

## Voice

Write for developers operating real work. Be direct, technically specific, and
honest about evidence. Prefer verbs such as run, observe, interrupt, delegate,
and review. Avoid calling planned capabilities shipped or using vague AI claims
such as intelligent, magical, autonomous workforce, or revolutionary.
