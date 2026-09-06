---
status: accepted
supersedes: ADR-0004
---

# Use GPUI for the cross-platform desktop

The macOS and Linux desktop will use GPUI for windows, layout, input, text, focus,
accessibility, asynchronous tasks, and GPU painting. Terminal grids will be a
custom GPUI `Element` backed by a client-side projection of backend-neutral
terminal frames. GPUI and libghostty revisions are pinned exactly and isolated
inside their respective modules because both interfaces are pre-1.0.

We rejected the earlier eframe/winit, egui, wgpu, and cosmic-text stack. It is
portable, but it makes us own the integration among windowing, input, shaping,
accessibility, and custom GPU work. We rejected separate SwiftUI and GTK clients
because duplicated behavior would make macOS/Linux parity a permanent risk. We
continue to reject Ghostty's private full-surface embedder because it is not a
stable cross-platform interface and would own product concerns above terminal
emulation.

GPUI gives the ordinary browser chrome and responsive waterfall a declarative
layout system while allowing the terminal renderer to take direct control of
layout and painting. Its pre-1.0 churn is a real cost, so no core, protocol, PTY,
or terminal-domain type may depend on GPUI. The `ultraplexr-desktop` crate is the
only external seam. A macOS/Wayland/X11 spike must prove terminal input, IME,
clipboard, accessibility, custom painting, and packaging before the rest of the
desktop is built.

