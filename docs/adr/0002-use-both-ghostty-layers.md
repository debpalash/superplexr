---
status: superseded by ADR-0004
---

# Use libghostty-vt in the runtime and embedded Ghostty in the macOS shell

The durable runtime will own PTYs and use `libghostty-vt` for headless terminal
state and snapshots, while the macOS shell will embed Ghostty's native Metal
surface behind a private adapter. Driving the Ghostty app directly was rejected
because its tab and split model constrains the product; using its internal embedder
interface throughout was rejected because it is explicitly unstable and owns its
child process. A surface will therefore run a small attach client connected to a
runtime-owned run, isolating Ghostty churn while keeping terminal fidelity.
