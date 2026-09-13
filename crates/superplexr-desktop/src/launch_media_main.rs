//! Explicit macOS launch-media renderer.
//!
//! This target is opt-in and uses GPUI's real compositor on the main thread.

include!("main.rs");

#[cfg(target_os = "macos")]
fn main() {
    tests::capture_desktop_launch_media();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("desktop launch-media capture currently requires macOS");
}
