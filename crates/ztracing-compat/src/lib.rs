//! License-compatible tracing seam for the pinned GPUI dependency graph.
//!
//! GPUI and `sum_tree` consume only the `ztracing::instrument` attribute. This
//! module deliberately implements exactly that interface by re-exporting the
//! independently licensed `tracing` macro. It contains no Zed tracing code.

pub use tracing::instrument;
