//! Embeddable entry point for the durable termi9ne runtime.
//!
//! The standalone server and the desktop's self-hosted daemon use the same
//! implementation so their wire protocol can never drift.

include!("runtime.rs");
