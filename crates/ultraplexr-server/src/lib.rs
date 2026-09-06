//! Embeddable entry point for the durable ultraplexr runtime.
//!
//! The standalone server and the desktop's self-hosted daemon use the same
//! implementation so their wire protocol can never drift.

include!("runtime.rs");
