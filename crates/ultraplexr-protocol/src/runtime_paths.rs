//! New installations use the Ultraplexr namespace. Existing state stays put:
//! moving a live daemon's socket or forking its state would orphan Sessions.

use std::path::{Path, PathBuf};

/// State for the CLI/server/release desktop, with a legacy-install fallback.
#[must_use]
pub fn default_state_dir() -> PathBuf {
    select_state_dir(Path::new(""), None)
}

/// Debug desktop state is isolated by wire version, as before the rebrand.
#[must_use]
pub fn desktop_state_dir(debug: bool) -> PathBuf {
    select_state_dir(Path::new(""), debug.then_some(crate::PROTOCOL_VERSION))
}

fn select_state_dir(root: &Path, version: Option<u16>) -> PathBuf {
    let (current, legacy) = match version {
        Some(version) => (
            root.join(format!(".ultraplexr-dev/v{version}")),
            root.join(format!(".termi9ne-dev/v{version}")),
        ),
        None => (root.join(".ultraplexr"), root.join(".termi9ne")),
    };
    // Never silently switch away from an established branded installation.
    if !current.exists() && legacy.exists() {
        legacy
    } else {
        current
    }
}

#[cfg(test)]
#[path = "runtime_paths_tests.rs"]
mod tests;
