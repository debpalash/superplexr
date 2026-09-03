//! Authoritative inspection of the filesystem state a Run wants to publish.
//!
//! This module is the single Git-to-domain Adapter. Callers provide a managed
//! checkout and its admitted intent; they receive an immutable, bounded
//! manifest or a typed rejection. Git parsing, path hardening, and content
//! hashing stay hidden here so Candidate admission has one small interface.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
};

use sha2::{Digest, Sha256};
use superplexr_core::{
    ChangeIntent, ChangeOperation, ChangeScope, RealizedChange, RealizedChangeManifest,
};
use superplexr_protocol::RunCheckoutSummary;
use thiserror::Error;
use uuid::Uuid;

const MANIFEST_SCHEMA_VERSION: u16 = 1;
const SCANNER_VERSION: u16 = 2;
const MAX_CHANGES: usize = 1_024;
const MAX_GIT_ERROR_BYTES: usize = 4_096;

#[derive(Debug, Error)]
pub(crate) enum RealizedChangeError {
    #[error("realized-change inspection failed: {0}")]
    Io(#[from] io::Error),
    #[error("Git failed while inspecting realized changes: {0}")]
    Git(String),
    #[error("Git emitted an invalid or unsupported changed path")]
    InvalidPath,
    #[error("Git reported unsupported change status {0}")]
    UnsupportedStatus(String),
    #[error("realized-change manifest exceeds {MAX_CHANGES} entries")]
    TooManyChanges,
    #[error("managed checkout identity changed during Candidate inspection")]
    CheckoutIdentityChanged,
    #[error("managed checkout base does not match the admitted ChangeIntent")]
    BaseRevisionMismatch,
    #[error("managed checkout changed while its Candidate was being inspected")]
    CheckoutChangedDuringInspection,
    #[error("Candidate predates realized-change manifests and must be resubmitted before settlement")]
    CandidateManifestMissing,
    #[error("managed checkout no longer matches the immutable Candidate")]
    CandidateChanged,
    #[error("realized {operation:?} of undeclared path {path}")]
    UndeclaredChange {
        path: String,
        operation: ChangeOperation,
    },
    #[error("realized {operation:?} of contingent path {path} requires intent promotion")]
    ContingentChange {
        path: String,
        operation: ChangeOperation,
    },
    #[error("changed path is not a regular file or symbolic link: {0}")]
    UnsupportedFileType(String),
    #[error("Git emitted an invalid object identity while freezing the Candidate")]
    InvalidObjectIdentity,
    #[error("immutable Candidate snapshot is missing or corrupt")]
    SnapshotCorrupt,
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateSnapshot {
    pub manifest: RealizedChangeManifest,
    pub patch_locator: String,
}

/// Inspect a stable checkout and freeze exactly its admitted bytes into Git.
pub(crate) fn inspect(
    checkout: &RunCheckoutSummary,
    intent: &ChangeIntent,
) -> Result<CandidateSnapshot, RealizedChangeError> {
    if checkout.base_revision != intent.base_revision {
        return Err(RealizedChangeError::BaseRevisionMismatch);
    }
    let worktree = canonical_worktree(checkout)?;
    let head_revision = git_text(&worktree, ["rev-parse", "HEAD"])?;
    let first = changed_operations(&worktree, &checkout.base_revision)?;
    enforce_intent(intent, &first)?;

    let temporary_index = TemporaryIndex::new(&worktree)?;
    git_success_with_index(
        &worktree,
        [OsStr::new("read-tree"), OsStr::new(&checkout.base_revision)],
        temporary_index.path(),
        &[],
    )?;

    let mut pending = Vec::with_capacity(first.len());
    for (path, operation) in &first {
        if *operation != ChangeOperation::Delete {
            pending.push(inspect_checkout_file(&worktree, path)?);
        }
    }
    let object_ids = write_blob_objects(&worktree, &pending)?;
    let mut changes = Vec::with_capacity(first.len());
    for (path, operation) in &first {
        let (content_sha256, git_mode, git_object_id) = match operation {
            ChangeOperation::Delete => (None, None, None),
            ChangeOperation::Create | ChangeOperation::Modify => {
                let file = pending
                    .iter()
                    .find(|file| file.path == *path)
                    .ok_or(RealizedChangeError::SnapshotCorrupt)?;
                let object = object_ids
                    .get(path)
                    .cloned()
                    .ok_or(RealizedChangeError::SnapshotCorrupt)?;
                (
                    Some(file.content_sha256.clone()),
                    Some(file.mode.clone()),
                    Some(object),
                )
            }
        };
        changes.push(RealizedChange {
            path: path.clone(),
            operation: *operation,
            content_sha256,
            git_mode,
            git_object_id,
        });
    }

    update_temporary_index(
        &worktree,
        temporary_index.path(),
        &changes,
        checkout.base_revision.len(),
    )?;
    let snapshot_tree = git_text_with_index(
        &worktree,
        [OsStr::new("write-tree")],
        temporary_index.path(),
    )?;
    require_object_id(&snapshot_tree)?;
    let snapshot_revision = write_snapshot_commit(checkout, &worktree, &snapshot_tree)?;
    let patch_sha256 = patch_digest(
        &worktree,
        &checkout.base_revision,
        &snapshot_revision,
    )?;

    // A second observation prevents publishing a manifest assembled across two
    // visible checkout states without serializing normal terminal interaction.
    let second_head = git_text(&worktree, ["rev-parse", "HEAD"])?;
    let second = changed_operations(&worktree, &checkout.base_revision)?;
    if first != second
        || head_revision != second_head
        || pending
            .iter()
            .any(|file| !file.fingerprint.matches_path(&worktree.join(&file.path)))
    {
        return Err(RealizedChangeError::CheckoutChangedDuringInspection);
    }

    let snapshot_ref = format!(
        "refs/superplexr/candidates/{}/{}/{}",
        checkout.mission_id, checkout.run_id, snapshot_revision
    );
    git_success(
        &worktree,
        [
            OsStr::new("update-ref"),
            OsStr::new(&snapshot_ref),
            OsStr::new(&snapshot_revision),
        ],
    )?;
    let manifest = RealizedChangeManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        scanner_version: SCANNER_VERSION,
        repository_identity: intent.repository_identity.clone(),
        base_revision: checkout.base_revision.clone(),
        head_revision,
        snapshot_revision: Some(snapshot_revision.clone()),
        snapshot_tree: Some(snapshot_tree),
        snapshot_ref: Some(snapshot_ref),
        patch_sha256,
        changes,
    };
    Ok(CandidateSnapshot {
        patch_locator: format!(
            "git-diff:{}..{}",
            checkout.base_revision, snapshot_revision
        ),
        manifest,
    })
}

/// Verify that the retained Git objects still identify the frozen Candidate.
/// Later worktree writes are intentionally irrelevant to this check.
pub(crate) fn verify_snapshot(
    checkout: &RunCheckoutSummary,
    expected: &RealizedChangeManifest,
) -> Result<(), RealizedChangeError> {
    let worktree = canonical_worktree(checkout)?;
    if expected.scanner_version < 2 {
        return Err(RealizedChangeError::CandidateManifestMissing);
    }
    let revision = expected
        .snapshot_revision
        .as_deref()
        .ok_or(RealizedChangeError::SnapshotCorrupt)?;
    let tree = expected
        .snapshot_tree
        .as_deref()
        .ok_or(RealizedChangeError::SnapshotCorrupt)?;
    let reference = expected
        .snapshot_ref
        .as_deref()
        .ok_or(RealizedChangeError::SnapshotCorrupt)?;
    let retained = git_text(
        &worktree,
        ["rev-parse", "--verify", "--end-of-options", reference],
    )?;
    let retained_tree = git_text(
        &worktree,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{tree}}"),
        ],
    )?;
    let patch = patch_digest(&worktree, &expected.base_revision, revision)?;
    if retained != revision || retained_tree != tree || patch != expected.patch_sha256 {
        return Err(RealizedChangeError::CandidateChanged);
    }
    Ok(())
}

fn enforce_intent(
    intent: &ChangeIntent,
    realized: &BTreeMap<String, ChangeOperation>,
) -> Result<(), RealizedChangeError> {
    for (path, operation) in realized {
        match intent
            .claims
            .iter()
            .find(|claim| claim.path == *path && claim.operation == *operation)
        {
            Some(claim) if claim.scope == ChangeScope::Committed => {}
            Some(_) => {
                return Err(RealizedChangeError::ContingentChange {
                    path: path.clone(),
                    operation: *operation,
                });
            }
            None => {
                return Err(RealizedChangeError::UndeclaredChange {
                    path: path.clone(),
                    operation: *operation,
                });
            }
        }
    }
    Ok(())
}

fn changed_operations(
    worktree: &Path,
    base_revision: &str,
) -> Result<BTreeMap<String, ChangeOperation>, RealizedChangeError> {
    let output = git_output(
        worktree,
        [
            OsStr::new("diff"),
            OsStr::new("--name-status"),
            OsStr::new("-z"),
            OsStr::new("--find-renames=50%"),
            OsStr::new("--no-ext-diff"),
            OsStr::new(base_revision),
            OsStr::new("--"),
        ],
    )?;
    require_git_success(output).and_then(|output| parse_name_status(&output.stdout))
        .and_then(|mut changes| {
            let untracked = require_git_success(git_output(
                worktree,
                [
                    OsStr::new("ls-files"),
                    OsStr::new("--others"),
                    OsStr::new("--exclude-standard"),
                    OsStr::new("-z"),
                    OsStr::new("--"),
                ],
            )?)?;
            for raw_path in nul_fields(&untracked.stdout) {
                let path = validate_git_path(raw_path)?;
                insert_change(&mut changes, path, ChangeOperation::Create)?;
            }
            Ok(changes)
        })
}

fn parse_name_status(
    bytes: &[u8],
) -> Result<BTreeMap<String, ChangeOperation>, RealizedChangeError> {
    let fields = nul_fields(bytes);
    let mut index = 0;
    let mut changes = BTreeMap::new();
    while index < fields.len() {
        let status = std::str::from_utf8(fields[index])
            .map_err(|_| RealizedChangeError::UnsupportedStatus("non-UTF-8".to_owned()))?;
        index += 1;
        let kind = status
            .as_bytes()
            .first()
            .copied()
            .ok_or_else(|| RealizedChangeError::UnsupportedStatus("empty".to_owned()))?;
        match kind {
            b'A' | b'M' | b'T' | b'D' => {
                let raw_path = fields
                    .get(index)
                    .ok_or_else(|| RealizedChangeError::UnsupportedStatus(status.to_owned()))?;
                index += 1;
                let operation = match kind {
                    b'A' => ChangeOperation::Create,
                    b'D' => ChangeOperation::Delete,
                    b'M' | b'T' => ChangeOperation::Modify,
                    _ => unreachable!(),
                };
                insert_change(&mut changes, validate_git_path(raw_path)?, operation)?;
            }
            b'R' => {
                let old = fields
                    .get(index)
                    .ok_or_else(|| RealizedChangeError::UnsupportedStatus(status.to_owned()))?;
                let new = fields
                    .get(index + 1)
                    .ok_or_else(|| RealizedChangeError::UnsupportedStatus(status.to_owned()))?;
                index += 2;
                insert_change(
                    &mut changes,
                    validate_git_path(old)?,
                    ChangeOperation::Delete,
                )?;
                insert_change(
                    &mut changes,
                    validate_git_path(new)?,
                    ChangeOperation::Create,
                )?;
            }
            b'C' => {
                let _source = fields
                    .get(index)
                    .ok_or_else(|| RealizedChangeError::UnsupportedStatus(status.to_owned()))?;
                let destination = fields
                    .get(index + 1)
                    .ok_or_else(|| RealizedChangeError::UnsupportedStatus(status.to_owned()))?;
                index += 2;
                insert_change(
                    &mut changes,
                    validate_git_path(destination)?,
                    ChangeOperation::Create,
                )?;
            }
            _ => return Err(RealizedChangeError::UnsupportedStatus(status.to_owned())),
        }
    }
    Ok(changes)
}

fn insert_change(
    changes: &mut BTreeMap<String, ChangeOperation>,
    path: String,
    operation: ChangeOperation,
) -> Result<(), RealizedChangeError> {
    if changes.len() >= MAX_CHANGES && !changes.contains_key(&path) {
        return Err(RealizedChangeError::TooManyChanges);
    }
    match changes.insert(path.clone(), operation) {
        Some(existing) if existing != operation => Err(RealizedChangeError::UnsupportedStatus(
            format!("ambiguous operations for {path}"),
        )),
        _ => Ok(()),
    }
}

fn validate_git_path(bytes: &[u8]) -> Result<String, RealizedChangeError> {
    let path = std::str::from_utf8(bytes).map_err(|_| RealizedChangeError::InvalidPath)?;
    let candidate = Path::new(path);
    if path.is_empty()
        || path.contains('\0')
        || path.contains(['\n', '\r'])
        || candidate.is_absolute()
        || candidate.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(RealizedChangeError::InvalidPath);
    }
    Ok(path.to_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileFingerprint {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FileFingerprint {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            mode: metadata.mode(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    fn matches_path(&self, path: &Path) -> bool {
        fs::symlink_metadata(path)
            .map(|metadata| Self::from_metadata(&metadata) == *self)
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug)]
enum FileSource {
    Regular,
    Symlink(Vec<u8>),
}

#[derive(Clone, Debug)]
struct PendingFile {
    path: String,
    content_sha256: String,
    mode: String,
    fingerprint: FileFingerprint,
    source: FileSource,
}

fn inspect_checkout_file(
    worktree: &Path,
    relative: &str,
) -> Result<PendingFile, RealizedChangeError> {
    let path = worktree.join(relative);
    let parent = path.parent().ok_or(RealizedChangeError::InvalidPath)?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !canonical_parent.starts_with(worktree) {
        return Err(RealizedChangeError::InvalidPath);
    }
    let metadata = fs::symlink_metadata(&path)?;
    let before = FileFingerprint::from_metadata(&metadata);
    let mut hasher = Sha256::new();
    let (mode, source) = if metadata.file_type().is_symlink() {
        let target = fs::read_link(&path)?.as_os_str().as_bytes().to_vec();
        hasher.update(&target);
        ("120000".to_owned(), FileSource::Symlink(target))
    } else if metadata.is_file() {
        let mut file = open_regular_nofollow(&path)?;
        let opened = FileFingerprint::from_metadata(&file.metadata()?);
        if before != opened {
            return Err(RealizedChangeError::CheckoutChangedDuringInspection);
        }
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        if opened != FileFingerprint::from_metadata(&file.metadata()?) {
            return Err(RealizedChangeError::CheckoutChangedDuringInspection);
        }
        let mode = if metadata.mode() & 0o111 == 0 {
            "100644"
        } else {
            "100755"
        };
        (mode.to_owned(), FileSource::Regular)
    } else {
        return Err(RealizedChangeError::UnsupportedFileType(
            relative.to_owned(),
        ));
    };
    if !before.matches_path(&path) {
        return Err(RealizedChangeError::CheckoutChangedDuringInspection);
    }
    Ok(PendingFile {
        path: relative.to_owned(),
        content_sha256: format!("{:x}", hasher.finalize()),
        mode,
        fingerprint: before,
        source,
    })
}

fn open_regular_nofollow(path: &Path) -> Result<File, io::Error> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn write_blob_objects(
    worktree: &Path,
    files: &[PendingFile],
) -> Result<BTreeMap<String, String>, RealizedChangeError> {
    let regular = files
        .iter()
        .filter(|file| matches!(file.source, FileSource::Regular))
        .collect::<Vec<_>>();
    let mut objects = BTreeMap::new();
    if !regular.is_empty() {
        let mut paths = Vec::new();
        for file in &regular {
            paths.extend_from_slice(file.path.as_bytes());
            paths.push(b'\n');
        }
        let output = require_git_success(git_output_with_input(
            worktree,
            ["hash-object", "-w", "--stdin-paths", "--no-filters"],
            &paths,
        )?)?;
        let identities = String::from_utf8(output.stdout)
            .map_err(|_| RealizedChangeError::InvalidObjectIdentity)?;
        let identities = identities.lines().collect::<Vec<_>>();
        if identities.len() != regular.len() {
            return Err(RealizedChangeError::InvalidObjectIdentity);
        }
        for (file, identity) in regular.into_iter().zip(identities) {
            require_object_id(identity)?;
            objects.insert(file.path.clone(), identity.to_owned());
        }
    }
    for file in files {
        let FileSource::Symlink(target) = &file.source else {
            continue;
        };
        let output = require_git_success(git_output_with_input(
            worktree,
            ["hash-object", "-w", "--stdin", "--no-filters"],
            target,
        )?)?;
        let identity = String::from_utf8(output.stdout)
            .map_err(|_| RealizedChangeError::InvalidObjectIdentity)?;
        let identity = identity.trim();
        require_object_id(identity)?;
        objects.insert(file.path.clone(), identity.to_owned());
    }
    Ok(objects)
}

fn update_temporary_index(
    worktree: &Path,
    index: &Path,
    changes: &[RealizedChange],
    object_id_bytes: usize,
) -> Result<(), RealizedChangeError> {
    let zero = "0".repeat(object_id_bytes);
    let mut input = Vec::new();
    for change in changes {
        match change.operation {
            ChangeOperation::Delete => {
                write!(&mut input, "0 {zero}\t").map_err(io::Error::other)?;
            }
            ChangeOperation::Create | ChangeOperation::Modify => {
                let mode = change
                    .git_mode
                    .as_deref()
                    .ok_or(RealizedChangeError::SnapshotCorrupt)?;
                let object = change
                    .git_object_id
                    .as_deref()
                    .ok_or(RealizedChangeError::SnapshotCorrupt)?;
                write!(&mut input, "{mode} {object}\t").map_err(io::Error::other)?;
            }
        }
        input.extend_from_slice(change.path.as_bytes());
        input.push(0);
    }
    git_success_with_index(
        worktree,
        ["update-index", "-z", "--add", "--index-info"],
        index,
        &input,
    )
}

fn write_snapshot_commit(
    checkout: &RunCheckoutSummary,
    worktree: &Path,
    tree: &str,
) -> Result<String, RealizedChangeError> {
    let message = format!(
        "superplexr Candidate snapshot\n\nMission: {}\nRun: {}\n",
        checkout.mission_id, checkout.run_id
    );
    let mut command = git_command(worktree);
    command
        .args(["commit-tree", tree, "-p", &checkout.base_revision])
        .env("GIT_AUTHOR_NAME", "superplexr")
        .env("GIT_AUTHOR_EMAIL", "snapshot@superplexr.local")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "superplexr")
        .env("GIT_COMMITTER_EMAIL", "snapshot@superplexr.local")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z");
    let output = require_git_success(finish_git(command, message.as_bytes())?)?;
    let identity = String::from_utf8(output.stdout)
        .map_err(|_| RealizedChangeError::InvalidObjectIdentity)?;
    let identity = identity.trim().to_owned();
    require_object_id(&identity)?;
    Ok(identity)
}

fn patch_digest(
    worktree: &Path,
    base_revision: &str,
    snapshot_revision: &str,
) -> Result<String, RealizedChangeError> {
    let mut command = git_command(worktree);
    command
        .args([
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--no-color",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            base_revision,
            snapshot_revision,
            "--",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Git diff stdout was not piped"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("Git diff stderr was not piped"))?;
    let stderr_reader =
        thread::spawn(move || read_bounded_while_draining(&mut stderr, MAX_GIT_ERROR_BYTES));
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = stdout.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let status = child.wait()?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("Git diff stderr reader panicked"))??;
    if !status.success() {
        let mut message = String::from_utf8_lossy(&stderr).trim().to_owned();
        message.truncate(MAX_GIT_ERROR_BYTES);
        return Err(RealizedChangeError::Git(message));
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn read_bounded_while_draining(
    reader: &mut impl Read,
    limit: usize,
) -> Result<Vec<u8>, io::Error> {
    let mut retained = Vec::with_capacity(limit);
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(retained);
        }
        let remaining = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

fn require_object_id(value: &str) -> Result<(), RealizedChangeError> {
    if matches!(value.len(), 40 | 64)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err(RealizedChangeError::InvalidObjectIdentity)
    }
}

struct TemporaryIndex {
    directory: PathBuf,
    path: PathBuf,
}

impl TemporaryIndex {
    fn new(worktree: &Path) -> Result<Self, RealizedChangeError> {
        let parent = worktree
            .parent()
            .ok_or(RealizedChangeError::CheckoutIdentityChanged)?;
        let directory = parent.join(format!(".superplexr-candidate-{}", Uuid::new_v4()));
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let path = directory.join("index");
        Ok(Self { directory, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.directory);
    }
}

fn canonical_worktree(checkout: &RunCheckoutSummary) -> Result<PathBuf, RealizedChangeError> {
    let worktree = fs::canonicalize(&checkout.worktree_path)?;
    let toplevel = PathBuf::from(git_text(&worktree, ["rev-parse", "--show-toplevel"])?);
    let toplevel = fs::canonicalize(toplevel)?;
    if worktree != toplevel {
        return Err(RealizedChangeError::CheckoutIdentityChanged);
    }
    Ok(worktree)
}

fn git_text<I, S>(cwd: &Path, args: I) -> Result<String, RealizedChangeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = require_git_success(git_output(cwd, args)?)?;
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|_| RealizedChangeError::Git("Git emitted non-UTF-8 identity data".to_owned()))
}

fn git_text_with_index<I, S>(
    cwd: &Path,
    args: I,
    index: &Path,
) -> Result<String, RealizedChangeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = require_git_success(git_output_with_index(cwd, args, index, &[])?)?;
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|_| RealizedChangeError::Git("Git emitted non-UTF-8 identity data".to_owned()))
}

fn git_success<I, S>(cwd: &Path, args: I) -> Result<(), RealizedChangeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    require_git_success(git_output(cwd, args)?).map(|_| ())
}

fn git_success_with_index<I, S>(
    cwd: &Path,
    args: I,
    index: &Path,
    input: &[u8],
) -> Result<(), RealizedChangeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    require_git_success(git_output_with_index(cwd, args, index, input)?).map(|_| ())
}

fn git_output<I, S>(cwd: &Path, args: I) -> Result<Output, RealizedChangeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_command(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(RealizedChangeError::Io)
}

fn git_output_with_input<I, S>(
    cwd: &Path,
    args: I,
    input: &[u8],
) -> Result<Output, RealizedChangeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = git_command(cwd);
    command.args(args);
    finish_git(command, input)
}

fn git_output_with_index<I, S>(
    cwd: &Path,
    args: I,
    index: &Path,
    input: &[u8],
) -> Result<Output, RealizedChangeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = git_command(cwd);
    command.args(args).env("GIT_INDEX_FILE", index);
    finish_git(command, input)
}

fn git_command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-c")
        .arg("commit.gpgSign=false")
        .arg("-c")
        .arg("i18n.commitEncoding=UTF-8")
        .arg("-C")
        .arg(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    command
}

fn finish_git(mut command: Command, input: &[u8]) -> Result<Output, RealizedChangeError> {
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("Git stdin was not piped"))?;
    stdin.write_all(input)?;
    drop(stdin);
    child.wait_with_output().map_err(RealizedChangeError::Io)
}

fn require_git_success(output: Output) -> Result<Output, RealizedChangeError> {
    if output.status.success() {
        return Ok(output);
    }
    let bytes = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    let mut message = String::from_utf8_lossy(bytes).trim().to_owned();
    message.truncate(MAX_GIT_ERROR_BYTES);
    Err(RealizedChangeError::Git(message))
}

fn nul_fields(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use superplexr_core::{ChangeClaim, ChangeIntentState, RunId};
    use superplexr_protocol::RunCheckoutState;
    use uuid::Uuid;

    fn intent(claims: Vec<ChangeClaim>) -> ChangeIntent {
        ChangeIntent {
            run_id: RunId::new(),
            version: 1,
            state: ChangeIntentState::Admitted,
            lease_epoch: 7,
            repository_identity: "fixture".to_owned(),
            base_revision: "a".repeat(40),
            claims,
        }
    }

    #[test]
    fn committed_claims_authorize_only_the_exact_operation() {
        let intent = intent(vec![ChangeClaim {
            path: "src/main.rs".to_owned(),
            operation: ChangeOperation::Modify,
            scope: ChangeScope::Committed,
        }]);
        let realized = BTreeMap::from([(
            "src/main.rs".to_owned(),
            ChangeOperation::Modify,
        )]);
        assert!(enforce_intent(&intent, &realized).is_ok());

        let wrong = BTreeMap::from([(
            "src/main.rs".to_owned(),
            ChangeOperation::Delete,
        )]);
        assert!(matches!(
            enforce_intent(&intent, &wrong),
            Err(RealizedChangeError::UndeclaredChange { .. })
        ));
    }

    #[test]
    fn contingent_claim_requires_promotion_before_publication() {
        let intent = intent(vec![ChangeClaim {
            path: "docs/notes.md".to_owned(),
            operation: ChangeOperation::Create,
            scope: ChangeScope::Contingent,
        }]);
        let realized = BTreeMap::from([(
            "docs/notes.md".to_owned(),
            ChangeOperation::Create,
        )]);
        assert!(matches!(
            enforce_intent(&intent, &realized),
            Err(RealizedChangeError::ContingentChange { .. })
        ));
    }

    #[test]
    fn rename_is_normalized_to_delete_plus_create() {
        let parsed = parse_name_status(b"R100\0old.rs\0new.rs\0").expect("parse rename");
        assert_eq!(parsed.get("old.rs"), Some(&ChangeOperation::Delete));
        assert_eq!(parsed.get("new.rs"), Some(&ChangeOperation::Create));
    }

    #[test]
    fn path_aliases_are_rejected() {
        assert!(validate_git_path(b"../outside").is_err());
        assert!(validate_git_path(b"/absolute").is_err());
        assert!(validate_git_path(b"src/lib.rs").is_ok());
    }

    #[test]
    fn duplicate_conflicting_operations_fail_closed() {
        let error = parse_name_status(b"A\0same\0D\0same\0").expect_err("must reject");
        assert!(matches!(
            error,
            RealizedChangeError::UnsupportedStatus(_)
        ));
    }

    #[test]
    fn nul_field_parser_ignores_only_the_trailing_separator() {
        assert_eq!(
            nul_fields(b"M\0src/lib.rs\0"),
            vec![b"M".as_slice(), b"src/lib.rs".as_slice()]
        );
    }

    #[test]
    fn inspects_real_git_state_and_hashes_the_manifest() {
        let repository = FixtureRepository::new();
        repository.write("modified.txt", "before\n");
        repository.write("deleted.txt", "remove me\n");
        repository.write("old-name.txt", "renamed\n");
        repository.git(["add", "."]);
        repository.git(["commit", "-m", "base"]);
        let base = repository.git_text(["rev-parse", "HEAD"]);

        repository.write("modified.txt", "after\n");
        fs::remove_file(repository.path.join("deleted.txt")).expect("delete fixture file");
        fs::rename(
            repository.path.join("old-name.txt"),
            repository.path.join("new-name.txt"),
        )
        .expect("rename fixture file");
        repository.write("created.txt", "new\n");

        let run_id = RunId::new();
        let mission_id = superplexr_core::MissionId::new();
        let checkout = RunCheckoutSummary {
            checkout_id: Uuid::new_v4(),
            mission_id,
            run_id,
            repository_root: repository.path.clone(),
            worktree_path: repository.path.clone(),
            base_revision: base.clone(),
            branch: "fixture".to_owned(),
            state: RunCheckoutState::Ready,
            created_at_unix_micros: 1,
            retired_at_unix_micros: None,
            last_error: None,
        };
        let intent = ChangeIntent {
            run_id,
            version: 1,
            state: ChangeIntentState::Admitted,
            lease_epoch: 7,
            repository_identity: "fixture".to_owned(),
            base_revision: base,
            claims: vec![
                claim("modified.txt", ChangeOperation::Modify),
                claim("deleted.txt", ChangeOperation::Delete),
                claim("old-name.txt", ChangeOperation::Delete),
                claim("new-name.txt", ChangeOperation::Create),
                claim("created.txt", ChangeOperation::Create),
            ],
        };

        let snapshot = inspect(&checkout, &intent).expect("inspect fixture checkout");
        let manifest = &snapshot.manifest;
        assert_eq!(manifest.changes.len(), 5);
        assert_eq!(manifest.base_revision, manifest.head_revision);
        assert_eq!(manifest.patch_sha256.len(), 64);
        assert_eq!(manifest.scanner_version, 2);
        assert!(manifest.snapshot_revision.is_some());
        assert!(manifest.snapshot_tree.is_some());
        assert!(manifest.snapshot_ref.is_some());
        assert!(snapshot.patch_locator.starts_with("git-diff:"));
        assert!(manifest.changes.iter().any(|change| {
            change.path == "deleted.txt"
                && change.operation == ChangeOperation::Delete
                && change.content_sha256.is_none()
        }));
        assert!(manifest.changes.iter().any(|change| {
            change.path == "created.txt"
                && change.operation == ChangeOperation::Create
                && change.content_sha256.as_ref().is_some_and(|hash| hash.len() == 64)
                && change.git_object_id.as_ref().is_some_and(|id| id.len() == 40)
                && change.git_mode.as_deref() == Some("100644")
        }));
        assert!(
            repository
                .git_text(["diff", "--cached", "--name-only"])
                .is_empty(),
            "Candidate snapshot must not mutate the agent's index"
        );
        let revision = manifest
            .snapshot_revision
            .as_deref()
            .expect("snapshot revision");
        assert_eq!(
            repository.git_text(["show", &format!("{revision}:created.txt")]),
            "new"
        );
        repository.write("created.txt", "later checkout mutation\n");
        verify_snapshot(&checkout, manifest).expect("frozen snapshot should remain valid");
        assert_eq!(
            repository.git_text(["show", &format!("{revision}:created.txt")]),
            "new"
        );
        repository.git([
            "update-ref",
            "-d",
            manifest.snapshot_ref.as_deref().expect("snapshot ref"),
        ]);
        assert!(
            verify_snapshot(&checkout, manifest).is_err(),
            "settlement must fail closed after snapshot retention is removed"
        );
    }

    #[test]
    fn real_git_inspection_rejects_undeclared_expansion() {
        let repository = FixtureRepository::new();
        repository.write("tracked.txt", "before\n");
        repository.git(["add", "."]);
        repository.git(["commit", "-m", "base"]);
        let base = repository.git_text(["rev-parse", "HEAD"]);
        repository.write("surprise.txt", "not admitted\n");
        let run_id = RunId::new();
        let checkout = RunCheckoutSummary {
            checkout_id: Uuid::new_v4(),
            mission_id: superplexr_core::MissionId::new(),
            run_id,
            repository_root: repository.path.clone(),
            worktree_path: repository.path.clone(),
            base_revision: base.clone(),
            branch: "fixture".to_owned(),
            state: RunCheckoutState::Ready,
            created_at_unix_micros: 1,
            retired_at_unix_micros: None,
            last_error: None,
        };
        let intent = ChangeIntent {
            run_id,
            version: 1,
            state: ChangeIntentState::Admitted,
            lease_epoch: 7,
            repository_identity: "fixture".to_owned(),
            base_revision: base,
            claims: vec![claim("tracked.txt", ChangeOperation::Modify)],
        };
        assert!(matches!(
            inspect(&checkout, &intent),
            Err(RealizedChangeError::UndeclaredChange { path, .. }) if path == "surprise.txt"
        ));
    }

    #[test]
    fn symlink_content_hashes_the_link_target_without_reading_outside() {
        let repository = FixtureRepository::new();
        repository.write("tracked.txt", "base\n");
        repository.git(["add", "."]);
        repository.git(["commit", "-m", "base"]);
        let base = repository.git_text(["rev-parse", "HEAD"]);
        let outside = repository.path.with_extension("outside");
        fs::write(&outside, "first secret\n").expect("write outside fixture");
        symlink(&outside, repository.path.join("outside-link"))
            .expect("create fixture symlink");
        let run_id = RunId::new();
        let checkout = checkout(&repository, run_id, &base);
        let intent = admitted_intent(
            run_id,
            base,
            vec![claim("outside-link", ChangeOperation::Create)],
        );

        let first = inspect(&checkout, &intent).expect("inspect symlink");
        fs::write(&outside, "different secret\n").expect("change outside fixture");
        let second = inspect(&checkout, &intent).expect("reinspect symlink");
        assert_eq!(
            first.manifest.patch_sha256,
            second.manifest.patch_sha256
        );
        fs::remove_file(outside).expect("remove outside fixture");
    }

    #[test]
    fn nested_repository_fails_closed_instead_of_hashing_a_directory() {
        let repository = FixtureRepository::new();
        repository.write("tracked.txt", "base\n");
        repository.git(["add", "."]);
        repository.git(["commit", "-m", "base"]);
        let base = repository.git_text(["rev-parse", "HEAD"]);
        let nested = repository.path.join("nested");
        fs::create_dir(&nested).expect("create nested repository");
        let nested_output = Command::new("git")
            .args(["init", "-q"])
            .arg(&nested)
            .output()
            .expect("initialize nested repository");
        assert!(nested_output.status.success());
        fs::write(nested.join("data.txt"), "nested\n").expect("write nested data");
        let run_id = RunId::new();
        let checkout = checkout(&repository, run_id, &base);
        let intent = admitted_intent(
            run_id,
            base,
            vec![claim("nested/", ChangeOperation::Create)],
        );

        assert!(matches!(
            inspect(&checkout, &intent),
            Err(RealizedChangeError::UnsupportedFileType(path)) if path == "nested/"
        ));
    }

    #[test]
    fn delete_then_recreate_is_conservatively_a_modify() {
        let repository = FixtureRepository::new();
        repository.write("same.txt", "before\n");
        repository.git(["add", "."]);
        repository.git(["commit", "-m", "base"]);
        let base = repository.git_text(["rev-parse", "HEAD"]);
        fs::remove_file(repository.path.join("same.txt")).expect("delete tracked file");
        repository.write("same.txt", "replacement\n");
        let run_id = RunId::new();
        let checkout = checkout(&repository, run_id, &base);
        let intent = admitted_intent(
            run_id,
            base,
            vec![
                claim("same.txt", ChangeOperation::Delete),
                claim("same.txt", ChangeOperation::Create),
            ],
        );

        assert!(matches!(
            inspect(&checkout, &intent),
            Err(RealizedChangeError::UndeclaredChange {
                path,
                operation: ChangeOperation::Modify,
            }) if path == "same.txt"
        ));
    }

    fn claim(path: &str, operation: ChangeOperation) -> ChangeClaim {
        ChangeClaim {
            path: path.to_owned(),
            operation,
            scope: ChangeScope::Committed,
        }
    }

    fn admitted_intent(
        run_id: RunId,
        base_revision: String,
        claims: Vec<ChangeClaim>,
    ) -> ChangeIntent {
        ChangeIntent {
            run_id,
            version: 1,
            state: ChangeIntentState::Admitted,
            lease_epoch: 7,
            repository_identity: "fixture".to_owned(),
            base_revision,
            claims,
        }
    }

    fn checkout(
        repository: &FixtureRepository,
        run_id: RunId,
        base_revision: &str,
    ) -> RunCheckoutSummary {
        RunCheckoutSummary {
            checkout_id: Uuid::new_v4(),
            mission_id: superplexr_core::MissionId::new(),
            run_id,
            repository_root: repository.path.clone(),
            worktree_path: repository.path.clone(),
            base_revision: base_revision.to_owned(),
            branch: "fixture".to_owned(),
            state: RunCheckoutState::Ready,
            created_at_unix_micros: 1,
            retired_at_unix_micros: None,
            last_error: None,
        }
    }

    struct FixtureRepository {
        path: PathBuf,
    }

    impl FixtureRepository {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("superplexr-realized-{}", Uuid::new_v4()));
            fs::create_dir(&path).expect("create fixture repository");
            let fixture = Self { path };
            fixture.git(["init", "-q"]);
            fixture.git(["config", "user.email", "fixture@superplexr.local"]);
            fixture.git(["config", "user.name", "superplexr fixture"]);
            fixture
        }

        fn write(&self, relative: &str, contents: &str) {
            fs::write(self.path.join(relative), contents).expect("write fixture file");
        }

        fn git<const N: usize>(&self, args: [&str; N]) {
            let output = git_output(&self.path, args).expect("run fixture Git command");
            assert!(
                output.status.success(),
                "fixture Git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn git_text<const N: usize>(&self, args: [&str; N]) -> String {
            super::git_text(&self.path, args).expect("read fixture Git output")
        }
    }

    impl Drop for FixtureRepository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

}
