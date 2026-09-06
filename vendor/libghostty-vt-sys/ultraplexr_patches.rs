//! Reproducible native overlay, confined to Cargo-owned build sources.
//! This is a build-time boundary, not a sandbox for executing untrusted Zig code.
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

const PREIMAGE_SHA256: &str = "633c582cb9a2ec1b279e84c0c21e848e637098ee2bedbb7dfcefeabd8288bc9f";
const POSTIMAGE: &[u8] = include_bytes!("../../patches/ghostty/mem.zig");

/// Apply the reviewed replacement only to the pinned preimage (or no-op on
/// the exact postimage). Unknown revisions and escaping links fail before write.
pub fn apply_native_patch(root: &Path) -> io::Result<()> {
    let root = root.canonicalize()?;
    let target = root.join("src/terminal/mem.zig");
    if !fs::symlink_metadata(&target)?.file_type().is_file()
        || !target.canonicalize()?.starts_with(&root)
    {
        return Err(io::Error::other(
            "native patch target must be an owned regular file",
        ));
    }
    let bytes = fs::read(&target)?;
    if bytes == POSTIMAGE {
        return Ok(());
    }
    if format!("{:x}", Sha256::digest(&bytes)) != PREIMAGE_SHA256 {
        return Err(io::Error::other(
            "unrecognized Ghostty mem.zig; review the native pin before updating",
        ));
    }
    // Replace rather than truncate: copied package-manager files may be
    // read-only, and hard-linked source bytes must never be changed in place.
    // create_new also prevents following a pre-existing staging-file link.
    let staged = target.with_file_name(format!(".mem.zig.ultraplexr-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    let result = file.write_all(POSTIMAGE).and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| fs::rename(&staged, &target));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

/// Copy an explicit source override into an isolated build directory. Never
/// patch the input checkout or an immutable package-manager store in place.
pub fn prepare_override(source: &Path, out_dir: &Path) -> io::Result<PathBuf> {
    let source = source.canonicalize()?;
    let out_dir = out_dir.canonicalize()?;
    if source.starts_with(&out_dir) || out_dir.starts_with(&source) {
        return Err(io::Error::other(
            "Ghostty source and Cargo output must not overlap",
        ));
    }
    if !source.is_dir() {
        return Err(io::Error::other("Ghostty source must be a directory"));
    }
    let destination = out_dir.join("ghostty-patched-src");
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            // Exact generated child only; never follow a destination symlink.
            fs::remove_dir_all(&destination)?;
        }
        Ok(_) => {
            return Err(io::Error::other(
                "generated Ghostty directory is not a directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    copy_tree(&source, &source, &destination)?;
    Ok(destination)
}

fn copy_tree(root: &Path, source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if [".git", ".zig-cache", "zig-out", ".DS_Store"]
            .iter()
            .any(|skip| name == *skip)
        {
            continue;
        }
        let input = entry.path();
        let output = destination.join(name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(root, &input, &output)?;
        } else if kind.is_file() {
            fs::copy(input, output)?;
        } else if kind.is_symlink() {
            let resolved = input.canonicalize()?;
            // Ghostty includes CLAUDE.md -> AGENTS.md. Materialize internal
            // file links; reject external links and directory cycles.
            if !resolved.starts_with(root) || !resolved.is_file() {
                return Err(io::Error::other(
                    "Ghostty source link escapes the tree or targets a directory",
                ));
            }
            fs::copy(resolved, output)?;
        } else {
            return Err(io::Error::other("Ghostty source contains a special file"));
        }
    }
    Ok(())
}
