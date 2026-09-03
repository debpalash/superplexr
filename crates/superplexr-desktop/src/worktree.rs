//! Naming a terminal by where it runs.
//!
//! A shell's working directory is the most stable identity a Session has: the
//! title changes with every foreground program, but a shell opened in a repo
//! stays in that repo. This turns a directory into `repo@branch` by reading
//! git's own files, with no subprocess, so it is cheap enough to call from a
//! render path behind a short cache.

use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Where a terminal is running, as a person would name it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Worktree {
    /// The repository's directory name, or the directory itself outside one.
    pub name: String,
    /// The checked-out branch, or a short commit id when detached.
    pub branch: Option<String>,
}

impl Worktree {
    /// `repo@branch`, or just the name outside a repository.
    pub fn label(&self) -> String {
        match &self.branch {
            Some(branch) => format!("{}@{branch}", self.name),
            None => self.name.clone(),
        }
    }
}

/// How long a described directory is trusted before git's files are re-read.
///
/// Long enough that a 60 Hz render never touches the disk, short enough that a
/// checkout shows within a moment.
const CACHE_TTL: Duration = Duration::from_secs(2);

/// Render-side cache of described directories.
#[derive(Default)]
pub(crate) struct WorktreeCache {
    entries: RefCell<HashMap<PathBuf, (Instant, Worktree)>>,
}

impl WorktreeCache {
    pub(crate) fn describe(&self, path: &Path) -> Worktree {
        let now = Instant::now();
        if let Some((seen, worktree)) = self.entries.borrow().get(path)
            && now.duration_since(*seen) < CACHE_TTL
        {
            return worktree.clone();
        }
        let worktree = describe(path);
        self.entries
            .borrow_mut()
            .insert(path.to_path_buf(), (now, worktree.clone()));
        worktree
    }
}

/// Describe a directory without caching.
pub(crate) fn describe(path: &Path) -> Worktree {
    match find_git_dir(path) {
        Some((root, git_dir)) => Worktree {
            name: leaf_name(&root).unwrap_or_else(|| root.display().to_string()),
            branch: branch_from_head(&git_dir),
        },
        None => Worktree {
            name: leaf_name(path).unwrap_or_else(|| path.display().to_string()),
            branch: None,
        },
    }
}

/// The path inside an OSC 7 `file://host/path` report.
///
/// Shell integrations write the path raw or percent-encoded depending on the
/// shell, so both are accepted.
pub(crate) fn path_from_osc7(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // The authority runs up to the first slash; a bare host has no path.
    let path = &rest[rest.find('/')?..];
    let decoded = percent_decode(path);
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn leaf_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

/// Walk up to the enclosing repository, returning its root and its git
/// directory. A linked worktree keeps a `.git` file pointing at its real git
/// directory, which is where its HEAD lives.
fn find_git_dir(start: &Path) -> Option<(PathBuf, PathBuf)> {
    for ancestor in start.ancestors() {
        let dot_git = ancestor.join(".git");
        let Ok(metadata) = fs::metadata(&dot_git) else {
            continue;
        };
        if metadata.is_dir() {
            return Some((ancestor.to_path_buf(), dot_git));
        }
        if metadata.is_file()
            && let Ok(contents) = fs::read_to_string(&dot_git)
            && let Some(target) = contents.trim().strip_prefix("gitdir:")
        {
            let target = target.trim();
            let git_dir = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                ancestor.join(target)
            };
            return Some((ancestor.to_path_buf(), git_dir));
        }
    }
    None
}

/// The branch named by HEAD, or a short commit id when detached.
fn branch_from_head(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    parse_head(&head)
}

fn parse_head(head: &str) -> Option<String> {
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        return Some(
            reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_owned(),
        );
    }
    // Detached: the file holds a commit id. Eight characters is what git
    // itself shows for a short id in a repo of ordinary size.
    (head.len() >= 8 && head.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| head[..8].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("superplexr-worktree-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("test root should be creatable");
        root
    }

    #[test]
    fn osc7_paths_are_accepted_raw_and_percent_encoded() {
        assert_eq!(
            path_from_osc7("file://mac.local/Users/me/src/superplexr"),
            Some(PathBuf::from("/Users/me/src/superplexr"))
        );
        assert_eq!(
            path_from_osc7("file:///Users/me/my%20project"),
            Some(PathBuf::from("/Users/me/my project"))
        );
        assert_eq!(path_from_osc7("file://host"), None);
        assert_eq!(path_from_osc7("/not/a/url"), None);
    }

    #[test]
    fn head_names_a_branch_or_a_short_detached_commit() {
        assert_eq!(
            parse_head("ref: refs/heads/main\n"),
            Some("main".to_owned())
        );
        assert_eq!(
            parse_head("ref: refs/heads/feature/x"),
            Some("feature/x".to_owned())
        );
        assert_eq!(
            parse_head("d5b7076a1f2e3c4b5a697887766554433221100f\n"),
            Some("d5b7076a".to_owned())
        );
        assert_eq!(parse_head(""), None);
        assert_eq!(parse_head("garbage"), None);
    }

    #[test]
    fn a_repository_is_named_by_its_root_and_branch_from_anywhere_inside() {
        let root = temp_root("repo");
        let repo = root.join("superplexr");
        fs::create_dir_all(repo.join(".git")).expect("git dir");
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");
        let deep = repo.join("crates/superplexr-desktop/src");
        fs::create_dir_all(&deep).expect("nested dir");

        let worktree = describe(&deep);
        assert_eq!(worktree.name, "superplexr");
        assert_eq!(worktree.branch.as_deref(), Some("main"));
        assert_eq!(worktree.label(), "superplexr@main");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_linked_worktree_follows_its_gitdir_file_to_the_real_head() {
        let root = temp_root("linked");
        let real = root.join("main-checkout/.git/worktrees/fix");
        fs::create_dir_all(&real).expect("real git dir");
        fs::write(real.join("HEAD"), "ref: refs/heads/fix/freeze\n").expect("HEAD");
        let linked = root.join("fix");
        fs::create_dir_all(&linked).expect("linked worktree");
        fs::write(linked.join(".git"), format!("gitdir: {}\n", real.display())).expect(".git file");

        let worktree = describe(&linked);
        assert_eq!(worktree.name, "fix");
        assert_eq!(worktree.branch.as_deref(), Some("fix/freeze"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn outside_a_repository_the_directory_itself_is_the_name() {
        let root = temp_root("plain");
        let plain = root.join("notes");
        fs::create_dir_all(&plain).expect("plain dir");
        let worktree = describe(&plain);
        assert_eq!(worktree.name, "notes");
        assert_eq!(worktree.branch, None);
        assert_eq!(worktree.label(), "notes");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_cache_serves_repeats_without_rereading() {
        let root = temp_root("cache");
        let repo = root.join("r");
        fs::create_dir_all(repo.join(".git")).expect("git dir");
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/a\n").expect("HEAD");
        let cache = WorktreeCache::default();
        assert_eq!(cache.describe(&repo).label(), "r@a");
        // A change inside the TTL is deliberately not seen yet.
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/b\n").expect("HEAD");
        assert_eq!(cache.describe(&repo).label(), "r@a");
        let _ = fs::remove_dir_all(&root);
    }
}
