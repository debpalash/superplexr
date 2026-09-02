---
status: superseded by ADR-0006
---

# Use one libghostty-vt terminal renderer on macOS and Linux

To make macOS and Linux first-class in v1, both clients will render public
`libghostty-vt` render state through the same Rust wgpu module, hosted by
eframe/winit. We rejected Ghostty's private embedded surface because it only
supports Apple platforms, and rejected separate SwiftUI and GTK implementations
because duplicated terminal behavior would make parity a continuing product
risk. This costs us a custom glyph atlas and terminal renderer, but confines
Ghostty API churn to one adapter and gives both platforms identical behavior.
