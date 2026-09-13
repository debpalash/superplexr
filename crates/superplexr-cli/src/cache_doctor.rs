use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct TreeSize {
    pub bytes: u64,
    pub entries: usize,
    pub limited: bool,
}

pub(crate) fn measure_tree(path: &Path, entry_limit: usize) -> std::io::Result<TreeSize> {
    let mut size = TreeSize {
        bytes: 0,
        entries: 0,
        limited: false,
    };
    if !path.exists() {
        return Ok(size);
    }
    let mut pending = vec![path.to_path_buf()];
    while let Some(next) = pending.pop() {
        if size.entries >= entry_limit {
            size.limited = true;
            break;
        }
        let metadata = fs::symlink_metadata(&next)?;
        size.entries += 1;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_file() {
            size.bytes = size.bytes.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            for entry in fs::read_dir(next)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(size)
}

pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(crate) fn run(
    workspace: PathBuf,
    state_dir: PathBuf,
    entry_limit: usize,
    clean_build_cache: bool,
) -> Result<(), String> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("could not resolve {}: {error}", workspace.display()))?;
    if clean_build_cache && !workspace.join("Cargo.toml").is_file() {
        return Err(format!(
            "{} does not contain Cargo.toml; build-cache cleanup uses cargo clean",
            workspace.display()
        ));
    }
    println!("SUPERPLEXR DOCTOR");
    println!("workspace {}", workspace.display());

    let locations = [
        ("build", workspace.join("target"), true),
        ("sessions", state_dir.join("sessions"), false),
        ("checkouts", state_dir.join("run-checkouts"), false),
        ("fault-worktrees", state_dir.join("fault-worktrees"), false),
    ];
    let mut reclaimable = 0_u64;
    for (label, path, can_clean) in locations {
        let size = measure_tree(&path, entry_limit)
            .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
        let suffix = if size.limited { "+ (scan limit)" } else { "" };
        println!(
            "{label:<15} {:>10} {suffix}  {}",
            human_bytes(size.bytes),
            path.display()
        );
        if can_clean {
            reclaimable = reclaimable.saturating_add(size.bytes);
        }
    }

    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
    if let Some(cargo_home) = cargo_home {
        for (label, child) in [("cargo-registry", "registry"), ("cargo-git", "git")] {
            let path = cargo_home.join(child);
            let size = measure_tree(&path, entry_limit)
                .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
            let suffix = if size.limited { "+ (scan limit)" } else { "" };
            println!(
                "{label:<15} {:>10} {suffix}  {}",
                human_bytes(size.bytes),
                path.display()
            );
        }
    }

    if clean_build_cache {
        let status = Command::new("cargo")
            .arg("clean")
            .arg("--manifest-path")
            .arg(workspace.join("Cargo.toml"))
            .status()
            .map_err(|error| format!("could not start cargo clean: {error}"))?;
        if !status.success() {
            return Err(format!("cargo clean exited with {status}"));
        }
        println!("cleaned build cache (up to {})", human_bytes(reclaimable));
    } else if reclaimable > 0 && workspace.join("Cargo.toml").is_file() {
        println!(
            "action          superplexr doctor --workspace {} --clean-build-cache",
            workspace.display()
        );
    } else {
        println!("status          build cache is already clear");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_binary_sizes() {
        assert_eq!(human_bytes(17), "17 B");
        assert_eq!(human_bytes(1_572_864), "1.5 MiB");
    }

    #[test]
    fn scan_is_bounded() {
        let root = std::env::temp_dir().join(format!("superplexr-doctor-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("one"), [0_u8; 8]).unwrap();
        fs::write(root.join("two"), [0_u8; 8]).unwrap();
        let size = measure_tree(&root, 2).unwrap();
        assert_eq!(size.entries, 2);
        assert!(size.limited);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scan_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("superplexr-doctor-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("file"), [0_u8; 4]).unwrap();
        symlink(&root, root.join("loop")).unwrap();
        let size = measure_tree(&root, 10).unwrap();
        assert_eq!(size.bytes, 4);
        assert!(!size.limited);
        fs::remove_dir_all(root).unwrap();
    }
}
