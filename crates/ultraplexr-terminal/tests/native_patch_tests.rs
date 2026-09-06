//! Tests the actual build overlay, including fidelity to the reviewable diff.
#[path = "../../../vendor/libghostty-vt-sys/ultraplexr_patches.rs"]
mod native_patch;

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const POSTIMAGE: &[u8] = include_bytes!("../../../patches/ghostty/mem.zig");
const PATCH: &[u8] =
    include_bytes!("../../../patches/ghostty/0001-darwin-owned-backing-replacement.patch");

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "up-native-patch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("unique fixture");
        Self(path)
    }
    fn source(&self) -> PathBuf {
        let root = self.0.join("source");
        fs::create_dir_all(root.join("src/terminal")).expect("source tree");
        fs::write(root.join("src/terminal/mem.zig"), POSTIMAGE).expect("postimage");
        root
    }
    fn out(&self) -> PathBuf {
        let out = self.0.join("out");
        fs::create_dir(&out).expect("Cargo output");
        out
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove owned fixture");
    }
}

#[test]
fn reviewed_diff_reconstructs_pinned_preimage_and_overlay_is_idempotent() {
    let fixture = Fixture::new();
    let root = fixture.source();
    let mut child = Command::new("git")
        .args(["apply", "--reverse", "--whitespace=error", "-"])
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git apply");
    child
        .stdin
        .take()
        .expect("patch input")
        .write_all(PATCH)
        .expect("write patch");
    let result = child.wait_with_output().expect("reverse diff");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let target = root.join("src/terminal/mem.zig");
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(&target).expect("preimage"))),
        "633c582cb9a2ec1b279e84c0c21e848e637098ee2bedbb7dfcefeabd8288bc9f"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(POSTIMAGE)),
        "12ae340635185b0aa0987d8c6376f8bc6101f7183fbcc092cb6dcc17ccfa905b"
    );
    let original = fs::read(&target).expect("original bytes");
    let sibling = fixture.0.join("hard-linked-input");
    fs::hard_link(&target, &sibling).expect("linked source input");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).expect("immutable input");
    }
    native_patch::apply_native_patch(&root).expect("apply pinned overlay");
    assert_eq!(fs::read(&target).expect("replacement"), POSTIMAGE);
    assert_eq!(fs::read(sibling).expect("linked input preserved"), original);
    native_patch::apply_native_patch(&root).expect("repeat safely");
    assert_eq!(fs::read(target).expect("unchanged"), POSTIMAGE);
}

#[test]
fn unknown_revision_fails_without_modification() {
    let fixture = Fixture::new();
    let root = fixture.source();
    let target = root.join("src/terminal/mem.zig");
    fs::write(&target, b"different native version").expect("unknown source");
    assert!(native_patch::apply_native_patch(&root).is_err());
    assert_eq!(
        fs::read(target).expect("preserved source"),
        b"different native version"
    );
}

#[test]
fn override_is_independent_refreshable_and_excludes_build_and_git_state() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let out = fixture.out();
    for name in [".git", ".zig-cache", "zig-out"] {
        fs::create_dir(source.join(name)).expect("excluded directory");
        fs::write(source.join(name).join("sentinel"), b"keep").expect("sentinel");
    }
    let copy = native_patch::prepare_override(&source, &out).expect("owned copy");
    assert_eq!(
        fs::read(copy.join("src/terminal/mem.zig")).expect("copy"),
        POSTIMAGE
    );
    for name in [".git", ".zig-cache", "zig-out"] {
        assert!(!copy.join(name).exists());
        assert!(source.join(name).join("sentinel").exists());
    }
    fs::write(copy.join("src/terminal/mem.zig"), b"altered copy").expect("modify copy");
    assert_eq!(
        fs::read(source.join("src/terminal/mem.zig")).expect("input untouched"),
        POSTIMAGE
    );
    fs::write(source.join("new-file"), b"source changed").expect("source update");
    native_patch::prepare_override(&source, &out).expect("refresh copy");
    assert_eq!(
        fs::read(copy.join("new-file")).expect("fresh"),
        b"source changed"
    );
    native_patch::apply_native_patch(&copy).expect("refreshed postimage");
}

#[test]
fn overlapping_source_and_output_are_rejected_before_deletion() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let nested_out = source.join("output");
    fs::create_dir(&nested_out).expect("nested output");
    assert!(native_patch::prepare_override(&source, &source).is_err());
    assert!(native_patch::prepare_override(&source, &nested_out).is_err());
    assert!(native_patch::prepare_override(&source, &fixture.0).is_err());
    assert!(source.join("src/terminal/mem.zig").exists());
}

#[cfg(unix)]
#[test]
fn escaping_patch_targets_and_destination_links_are_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let source = fixture.source();
    let out = fixture.out();
    let external = fixture.0.join("external");
    fs::create_dir(&external).expect("external directory");
    fs::write(external.join("mem.zig"), POSTIMAGE).expect("external file");
    let target = source.join("src/terminal/mem.zig");
    fs::remove_file(&target).expect("remove fixture file");
    symlink(external.join("mem.zig"), &target).expect("leaf link");
    assert!(native_patch::apply_native_patch(&source).is_err());
    fs::remove_file(&target).expect("remove fixture link");
    fs::remove_dir(source.join("src/terminal")).expect("empty directory");
    symlink(&external, source.join("src/terminal")).expect("parent link");
    assert!(native_patch::apply_native_patch(&source).is_err());
    symlink(&external, out.join("ghostty-patched-src")).expect("destination link");
    assert!(native_patch::prepare_override(&source, &out).is_err());
    assert_eq!(
        fs::read(external.join("mem.zig")).expect("external untouched"),
        POSTIMAGE
    );
}

#[cfg(unix)]
#[test]
fn internal_file_links_are_materialized_but_external_and_directory_links_fail() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let source = fixture.source();
    let out = fixture.out();
    fs::write(source.join("AGENTS.md"), b"guide").expect("guide");
    symlink("AGENTS.md", source.join("CLAUDE.md")).expect("internal link");
    let copy = native_patch::prepare_override(&source, &out).expect("materialize link");
    assert!(
        fs::symlink_metadata(copy.join("CLAUDE.md"))
            .expect("copied guide")
            .file_type()
            .is_file()
    );
    assert_eq!(
        fs::read(copy.join("CLAUDE.md")).expect("contents"),
        b"guide"
    );
    fs::write(fixture.0.join("external"), b"outside").expect("outside file");
    symlink("../external", source.join("escape")).expect("escaping link");
    assert!(native_patch::prepare_override(&source, &out).is_err());
    fs::remove_file(source.join("escape")).expect("remove fixture link");
    symlink(".", source.join("cycle")).expect("directory link");
    assert!(native_patch::prepare_override(&source, &out).is_err());
}
