use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use superplexr_core::{MissionId, RunId};
use superplexr_protocol::{RunCheckoutState, RunCheckoutSummary};
use thiserror::Error;
use uuid::Uuid;

const FILE_VERSION: u16 = 1;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CHECKOUTS: usize = 512;
const MAX_REF_BYTES: usize = 1024;
const MAX_ERROR_BYTES: usize = 4096;

#[derive(Debug, Error)]
pub(crate) enum CheckoutError {
    #[error("Run checkout store is not a regular owner-controlled file: {0}")]
    InsecureStore(PathBuf),
    #[error("Run checkout store exceeds 4 MiB")]
    StoreTooLarge,
    #[error("Run checkout store contains invalid or path-escaping record for Run {0}")]
    InvalidRecord(RunId),
    #[error("Run checkout store could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("Run checkout store is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported Run checkout store version {0}")]
    Version(u16),
    #[error("Run checkout limit of {MAX_CHECKOUTS} reached")]
    TooMany,
    #[error("Git ref must contain 1 to {MAX_REF_BYTES} bytes and no NUL")]
    InvalidRef,
    #[error("repository is not an owner-controlled Git working tree: {0}")]
    InvalidRepository(PathBuf),
    #[error("Git command failed: {0}")]
    Git(String),
    #[error("Run {run_id} already has a checkout with different provenance")]
    ProvenanceConflict { run_id: RunId },
    #[error("Run {0} does not have a managed checkout")]
    NotFound(RunId),
    #[error("Run checkout is not ready")]
    NotReady,
    #[error("Run must be finished and have no live terminal before checkout retirement")]
    RunActive,
    #[error("Run checkout has uncommitted or untracked files and cannot be retired")]
    Dirty,
    #[error("Run checkout HEAD is not reachable from {0:?} and cannot be retired")]
    Unmerged(String),
    #[error("managed Run checkout path escaped its owner-controlled root")]
    PathEscape,
    #[error("system time is before the Unix epoch")]
    Clock,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckoutReservation {
    pub summary: RunCheckoutSummary,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckoutFile {
    version: u16,
    checkouts: BTreeMap<RunId, RunCheckoutSummary>,
}

pub(crate) struct RunCheckoutStore {
    path: PathBuf,
    managed_root: PathBuf,
    checkouts: BTreeMap<RunId, RunCheckoutSummary>,
}

impl RunCheckoutStore {
    pub(crate) fn open(path: PathBuf, managed_root: PathBuf) -> Result<Self, CheckoutError> {
        fs::create_dir_all(&managed_root)?;
        fs::set_permissions(&managed_root, fs::Permissions::from_mode(0o700))?;
        let managed_root = fs::canonicalize(managed_root)?;
        let checkouts = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                // SAFETY: geteuid reads immutable process credentials.
                let uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.uid() != uid
                {
                    return Err(CheckoutError::InsecureStore(path));
                }
                if metadata.len() > MAX_FILE_BYTES {
                    return Err(CheckoutError::StoreTooLarge);
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                let file: CheckoutFile = serde_json::from_slice(&fs::read(&path)?)?;
                if file.version != FILE_VERSION {
                    return Err(CheckoutError::Version(file.version));
                }
                if file.checkouts.len() > MAX_CHECKOUTS {
                    return Err(CheckoutError::TooMany);
                }
                for (run_id, checkout) in &file.checkouts {
                    validate_summary(&managed_root, *run_id, checkout)?;
                }
                file.checkouts
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            managed_root,
            checkouts,
        })
    }

    pub(crate) fn list(&self, mission_id: Option<MissionId>) -> Vec<RunCheckoutSummary> {
        self.checkouts
            .values()
            .filter(|checkout| mission_id.is_none_or(|id| checkout.mission_id == id))
            .cloned()
            .collect()
    }

    pub(crate) fn get_ready(
        &self,
        mission_id: MissionId,
        run_id: RunId,
    ) -> Result<RunCheckoutSummary, CheckoutError> {
        let checkout = self
            .checkouts
            .get(&run_id)
            .filter(|checkout| checkout.mission_id == mission_id)
            .ok_or(CheckoutError::NotFound(run_id))?;
        if checkout.state != RunCheckoutState::Ready {
            return Err(CheckoutError::NotReady);
        }
        Ok(checkout.clone())
    }

    pub(crate) fn reserve(
        &mut self,
        reservation: CheckoutReservation,
    ) -> Result<RunCheckoutSummary, CheckoutError> {
        let proposed = reservation.summary;
        if let Some(existing) = self.checkouts.get(&proposed.run_id) {
            if existing.mission_id != proposed.mission_id
                || existing.repository_root != proposed.repository_root
                || existing.base_revision != proposed.base_revision
                || existing.branch != proposed.branch
            {
                return Err(CheckoutError::ProvenanceConflict {
                    run_id: proposed.run_id,
                });
            }
            return Ok(existing.clone());
        }
        if self.checkouts.len() == MAX_CHECKOUTS {
            return Err(CheckoutError::TooMany);
        }
        self.checkouts.insert(proposed.run_id, proposed.clone());
        if let Err(error) = self.persist() {
            self.checkouts.remove(&proposed.run_id);
            return Err(error);
        }
        Ok(proposed)
    }

    pub(crate) fn transition(
        &mut self,
        run_id: RunId,
        state: RunCheckoutState,
        error: Option<String>,
    ) -> Result<RunCheckoutSummary, CheckoutError> {
        let previous = self
            .checkouts
            .get(&run_id)
            .cloned()
            .ok_or(CheckoutError::NotFound(run_id))?;
        let checkout = self
            .checkouts
            .get_mut(&run_id)
            .expect("checkout existence checked above");
        checkout.state = state;
        checkout.last_error = error.map(|error| truncate(error, MAX_ERROR_BYTES));
        if state == RunCheckoutState::Retired {
            checkout.retired_at_unix_micros = Some(now_micros()?);
        }
        let updated = checkout.clone();
        if let Err(error) = self.persist() {
            self.checkouts.insert(run_id, previous);
            return Err(error);
        }
        Ok(updated)
    }

    pub(crate) fn managed_root(&self) -> &Path {
        &self.managed_root
    }

    fn persist(&self) -> Result<(), CheckoutError> {
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "checkout path has no parent",
            )
        })?;
        fs::create_dir_all(parent)?;
        let temporary = temporary_path(&self.path);
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            serde_json::to_writer_pretty(
                &mut file,
                &CheckoutFile {
                    version: FILE_VERSION,
                    checkouts: self.checkouts.clone(),
                },
            )?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
            File::open(parent)?.sync_all()?;
            Ok::<(), CheckoutError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

pub(crate) fn preflight(
    managed_root: &Path,
    mission_id: MissionId,
    run_id: RunId,
    repository: &Path,
    base_ref: &str,
) -> Result<CheckoutReservation, CheckoutError> {
    validate_ref(base_ref)?;
    let repository_root = repository_root(repository)?;
    let base_revision = git_text(
        &repository_root,
        [
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from("--end-of-options"),
            OsString::from(format!("{base_ref}^{{commit}}")),
        ],
    )?;
    if !is_object_id(&base_revision) {
        return Err(CheckoutError::Git(
            "base ref did not resolve to a full commit object ID".to_owned(),
        ));
    }
    let mission = mission_id.to_string();
    let run = run_id.to_string();
    let branch = format!("superplexr/m{}/r{}", &mission[..8], &run[..8]);
    git_success(
        &repository_root,
        [
            OsString::from("check-ref-format"),
            OsString::from("--branch"),
            OsString::from(&branch),
        ],
    )?;

    let root = absolute_normalized(managed_root)?;
    let worktree_path = root.join(&mission).join(&run);
    if !worktree_path.starts_with(&root) {
        return Err(CheckoutError::PathEscape);
    }
    Ok(CheckoutReservation {
        summary: RunCheckoutSummary {
            checkout_id: Uuid::new_v4(),
            mission_id,
            run_id,
            repository_root,
            worktree_path,
            base_revision,
            branch,
            state: RunCheckoutState::Provisioning,
            created_at_unix_micros: now_micros()?,
            retired_at_unix_micros: None,
            last_error: None,
        },
    })
}

pub(crate) fn provision(checkout: &RunCheckoutSummary) -> Result<(), CheckoutError> {
    if checkout.worktree_path.exists() {
        verify_existing(checkout)?;
        return Ok(());
    }
    let parent = checkout
        .worktree_path
        .parent()
        .ok_or(CheckoutError::PathEscape)?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;

    let branch_ref = format!("refs/heads/{}", checkout.branch);
    let branch_exists = git_status(
        &checkout.repository_root,
        [
            OsString::from("show-ref"),
            OsString::from("--verify"),
            OsString::from("--quiet"),
            OsString::from(&branch_ref),
        ],
    )?
    .success();
    let mut args = vec![OsString::from("worktree"), OsString::from("add")];
    if branch_exists {
        let branch_revision = git_text(
            &checkout.repository_root,
            [
                OsString::from("rev-parse"),
                OsString::from("--verify"),
                OsString::from("--end-of-options"),
                OsString::from(format!("{}^{{commit}}", checkout.branch)),
            ],
        )?;
        if branch_revision != checkout.base_revision {
            return Err(CheckoutError::ProvenanceConflict {
                run_id: checkout.run_id,
            });
        }
        args.push(checkout.worktree_path.as_os_str().to_owned());
        args.push(OsString::from(&checkout.branch));
    } else {
        args.extend([
            OsString::from("--no-track"),
            OsString::from("-b"),
            OsString::from(&checkout.branch),
            checkout.worktree_path.as_os_str().to_owned(),
            OsString::from(&checkout.base_revision),
        ]);
    }
    git_success(&checkout.repository_root, args)?;
    verify_existing(checkout)
}

pub(crate) fn retire(
    checkout: &RunCheckoutSummary,
    merged_into_ref: &str,
) -> Result<(), CheckoutError> {
    validate_ref(merged_into_ref)?;
    let target_revision = git_text(
        &checkout.repository_root,
        [
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from("--end-of-options"),
            OsString::from(format!("{merged_into_ref}^{{commit}}")),
        ],
    )?;
    if checkout.worktree_path.exists() {
        verify_checkout_identity(checkout)?;
        let status = git_raw(
            &checkout.worktree_path,
            [
                OsString::from("status"),
                OsString::from("--porcelain=v1"),
                OsString::from("--untracked-files=all"),
            ],
        )?;
        if !status.stdout.is_empty() {
            return Err(CheckoutError::Dirty);
        }
        let ancestor = git_status(
            &checkout.repository_root,
            [
                OsString::from("merge-base"),
                OsString::from("--is-ancestor"),
                OsString::from(&checkout.branch),
                OsString::from(&target_revision),
            ],
        )?;
        if !ancestor.success() {
            return Err(CheckoutError::Unmerged(merged_into_ref.to_owned()));
        }
        git_success(
            &checkout.repository_root,
            [
                OsString::from("worktree"),
                OsString::from("remove"),
                checkout.worktree_path.as_os_str().to_owned(),
            ],
        )?;
    }

    let branch_ref = format!("refs/heads/{}", checkout.branch);
    if git_status(
        &checkout.repository_root,
        [
            OsString::from("show-ref"),
            OsString::from("--verify"),
            OsString::from("--quiet"),
            OsString::from(&branch_ref),
        ],
    )?
    .success()
    {
        let ancestor = git_status(
            &checkout.repository_root,
            [
                OsString::from("merge-base"),
                OsString::from("--is-ancestor"),
                OsString::from(&checkout.branch),
                OsString::from(&target_revision),
            ],
        )?;
        if !ancestor.success() {
            return Err(CheckoutError::Unmerged(merged_into_ref.to_owned()));
        }
        git_success(
            &checkout.repository_root,
            [
                OsString::from("branch"),
                OsString::from("-d"),
                OsString::from(&checkout.branch),
            ],
        )?;
    }
    Ok(())
}

fn verify_existing(checkout: &RunCheckoutSummary) -> Result<(), CheckoutError> {
    verify_checkout_identity(checkout)?;
    let head = git_text(
        &checkout.worktree_path,
        [OsString::from("rev-parse"), OsString::from("HEAD")],
    )?;
    if head != checkout.base_revision {
        return Err(CheckoutError::ProvenanceConflict {
            run_id: checkout.run_id,
        });
    }
    let branch = git_text(
        &checkout.worktree_path,
        [
            OsString::from("symbolic-ref"),
            OsString::from("--short"),
            OsString::from("HEAD"),
        ],
    )?;
    if branch != checkout.branch {
        return Err(CheckoutError::ProvenanceConflict {
            run_id: checkout.run_id,
        });
    }
    Ok(())
}

fn verify_checkout_identity(checkout: &RunCheckoutSummary) -> Result<(), CheckoutError> {
    let actual = repository_root(&checkout.worktree_path)?;
    let expected = fs::canonicalize(&checkout.worktree_path)?;
    if actual != expected {
        return Err(CheckoutError::PathEscape);
    }
    Ok(())
}

fn repository_root(repository: &Path) -> Result<PathBuf, CheckoutError> {
    let candidate = fs::canonicalize(repository)
        .map_err(|_| CheckoutError::InvalidRepository(repository.to_owned()))?;
    let metadata = fs::metadata(&candidate)?;
    // SAFETY: geteuid reads immutable process credentials.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != uid {
        return Err(CheckoutError::InvalidRepository(candidate));
    }
    let output = git_text(
        &candidate,
        [
            OsString::from("rev-parse"),
            OsString::from("--show-toplevel"),
        ],
    )?;
    let root = fs::canonicalize(output.trim())?;
    let root_metadata = fs::metadata(&root)?;
    if !root_metadata.is_dir() || root_metadata.uid() != uid {
        return Err(CheckoutError::InvalidRepository(root));
    }
    Ok(root)
}

fn validate_ref(reference: &str) -> Result<(), CheckoutError> {
    if reference.is_empty() || reference.len() > MAX_REF_BYTES || reference.contains('\0') {
        return Err(CheckoutError::InvalidRef);
    }
    Ok(())
}

fn validate_summary(
    managed_root: &Path,
    key: RunId,
    checkout: &RunCheckoutSummary,
) -> Result<(), CheckoutError> {
    let mission = checkout.mission_id.to_string();
    let run = checkout.run_id.to_string();
    let expected_path = managed_root.join(&mission).join(&run);
    let expected_branch = format!("superplexr/m{}/r{}", &mission[..8], &run[..8]);
    let valid_error = checkout
        .last_error
        .as_ref()
        .is_none_or(|error| error.len() <= MAX_ERROR_BYTES && !error.contains('\0'));
    if key != checkout.run_id
        || checkout.repository_root.as_os_str().is_empty()
        || !checkout.repository_root.is_absolute()
        || checkout.worktree_path != expected_path
        || checkout.branch != expected_branch
        || !is_object_id(&checkout.base_revision)
        || !valid_error
    {
        return Err(CheckoutError::InvalidRecord(key));
    }
    Ok(())
}

fn git_success<I>(cwd: &Path, args: I) -> Result<(), CheckoutError>
where
    I: IntoIterator<Item = OsString>,
{
    let output = git_raw(cwd, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CheckoutError::Git(output_error(&output)))
    }
}

fn git_text<I>(cwd: &Path, args: I) -> Result<String, CheckoutError>
where
    I: IntoIterator<Item = OsString>,
{
    let output = git_raw(cwd, args)?;
    if !output.status.success() {
        return Err(CheckoutError::Git(output_error(&output)));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| CheckoutError::Git("Git emitted non-UTF-8 identity data".to_owned()))
}

fn git_status<I>(cwd: &Path, args: I) -> Result<std::process::ExitStatus, CheckoutError>
where
    I: IntoIterator<Item = OsString>,
{
    Ok(git_raw(cwd, args)?.status)
}

fn git_raw<I>(cwd: &Path, args: I) -> Result<std::process::Output, CheckoutError>
where
    I: IntoIterator<Item = OsString>,
{
    Command::new("git")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(CheckoutError::Io)
}

fn output_error(output: &std::process::Output) -> String {
    let text = if output.stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    truncate(text.trim().to_owned(), MAX_ERROR_BYTES)
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn absolute_normalized(path: &Path) -> Result<PathBuf, CheckoutError> {
    fs::canonicalize(path).map_err(CheckoutError::Io)
}

fn now_micros() -> Result<u64, CheckoutError> {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CheckoutError::Clock)?
        .as_micros();
    u64::try_from(micros).map_err(|_| CheckoutError::Clock)
}

fn truncate(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("run-checkouts.json"))
        .to_string_lossy()
        .into_owned();
    name.push_str(&format!(".{}.tmp", Uuid::new_v4()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .status()
            .expect("Git fixture command should start");
        assert!(status.success(), "Git fixture command failed: {args:?}");
    }

    fn repository(root: &Path) -> PathBuf {
        let repository = root.join("repository");
        fs::create_dir_all(&repository).expect("repository should exist");
        git(&repository, &["init", "-q"]);
        git(&repository, &["config", "user.name", "superplexr test"]);
        git(
            &repository,
            &["config", "user.email", "test@superplexr.invalid"],
        );
        fs::write(repository.join("README.md"), "fixture\n").expect("fixture should write");
        git(&repository, &["add", "README.md"]);
        git(&repository, &["commit", "-q", "-m", "fixture"]);
        repository
    }

    #[test]
    fn checkout_is_durable_and_clean_merged_retirement_is_conservative() {
        let root = std::env::temp_dir().join(format!("superplexr-checkout-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("fixture root should exist");
        let repository = repository(&root);
        // Default deployments keep private runtime state beneath the primary
        // checkout, so prove Git accepts and safely removes this nested path.
        let managed = repository.join(".superplexr").join("run-checkouts");
        let path = root.join("run-checkouts.json");
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let mut store =
            RunCheckoutStore::open(path.clone(), managed.clone()).expect("store should open");
        let reservation = preflight(&managed, mission_id, run_id, &repository, "HEAD")
            .expect("preflight should pass");
        let checkout = store
            .reserve(reservation)
            .expect("reservation should persist");
        provision(&checkout).expect("worktree should provision");
        let ready = store
            .transition(run_id, RunCheckoutState::Ready, None)
            .expect("ready state should persist");
        assert!(ready.worktree_path.is_dir());
        assert_eq!(
            RunCheckoutStore::open(path, managed)
                .expect("store should reopen")
                .get_ready(mission_id, run_id)
                .expect("ready checkout should reload"),
            ready
        );

        fs::write(ready.worktree_path.join("untracked.txt"), "keep me\n")
            .expect("dirty fixture should write");
        assert!(matches!(retire(&ready, "HEAD"), Err(CheckoutError::Dirty)));
        fs::remove_file(ready.worktree_path.join("untracked.txt"))
            .expect("dirty fixture should remove");
        retire(&ready, "HEAD").expect("clean merged checkout should retire");
        store
            .transition(run_id, RunCheckoutState::Retired, None)
            .expect("retired state should persist");
        assert!(!ready.worktree_path.exists());
        fs::remove_dir_all(root).expect("isolated fixture should be removable");
    }
}
