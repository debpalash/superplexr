use std::path::{Path, PathBuf};

use serde::Deserialize;
use ultraplexr_protocol::TerminalSessionSpec;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SandboxProfile {
    WorkspaceWrite,
}

impl SandboxProfile {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "workspace_write",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SandboxAttestation {
    pub backend: &'static str,
    pub profile: &'static str,
    pub network_isolated: bool,
}

#[derive(Debug, Error)]
pub(crate) enum SandboxError {
    #[error("sandbox working directory could not be resolved: {0}")]
    WorkingDirectory(#[source] std::io::Error),
    #[error("runtime state directory could not be resolved: {0}")]
    StateDirectory(#[source] std::io::Error),
    #[error("required {backend} sandbox executable is unavailable at {path}")]
    BackendUnavailable {
        backend: &'static str,
        path: PathBuf,
    },
    #[error("sandbox profile cannot encode a non-Unicode path: {0}")]
    NonUnicodePath(PathBuf),
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[error("sandbox profile is unsupported on {0}")]
    UnsupportedPlatform(&'static str),
}

pub(crate) fn apply(
    profile: SandboxProfile,
    spec: &mut TerminalSessionSpec,
    state_dir: &Path,
) -> Result<SandboxAttestation, SandboxError> {
    let workspace = spec
        .cwd
        .canonicalize()
        .map_err(SandboxError::WorkingDirectory)?;
    let state_dir = state_dir
        .canonicalize()
        .map_err(SandboxError::StateDirectory)?;
    spec.cwd.clone_from(&workspace);

    #[cfg(target_os = "macos")]
    {
        apply_macos(profile, spec, &workspace, &state_dir)
    }
    #[cfg(target_os = "linux")]
    {
        apply_linux(profile, spec, &workspace, &state_dir)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (profile, spec, workspace, state_dir);
        Err(SandboxError::UnsupportedPlatform(std::env::consts::OS))
    }
}

#[cfg(target_os = "macos")]
fn apply_macos(
    profile: SandboxProfile,
    spec: &mut TerminalSessionSpec,
    workspace: &Path,
    state_dir: &Path,
) -> Result<SandboxAttestation, SandboxError> {
    const BACKEND: &str = "macos_sandbox_exec";
    let executable = PathBuf::from("/usr/bin/sandbox-exec");
    require_backend(BACKEND, &executable)?;
    let workspace = quoted_path(workspace)?;
    let state_dir = quoted_path(state_dir)?;
    let policy = format!(
        "(version 1)\n\
         (deny default)\n\
         (allow process*)\n\
         (allow sysctl-read)\n\
         (allow file-read*)\n\
         (allow file-write* (subpath {workspace}) (literal \"/dev/null\") (literal \"/dev/tty\"))\n\
         (deny file-write* (subpath {state_dir}))\n\
         (allow network*)\n\
         (allow mach-lookup)\n\
         (allow signal (target self))"
    );
    wrap(spec, executable, vec!["-p".to_owned(), policy]);
    Ok(SandboxAttestation {
        backend: BACKEND,
        profile: profile.as_str(),
        network_isolated: false,
    })
}

#[cfg(target_os = "linux")]
fn apply_linux(
    profile: SandboxProfile,
    spec: &mut TerminalSessionSpec,
    workspace: &Path,
    state_dir: &Path,
) -> Result<SandboxAttestation, SandboxError> {
    const BACKEND: &str = "linux_bubblewrap";
    let executable = [PathBuf::from("/usr/bin/bwrap"), PathBuf::from("/bin/bwrap")]
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| SandboxError::BackendUnavailable {
            backend: BACKEND,
            path: PathBuf::from("/usr/bin/bwrap"),
        })?;
    let workspace = unicode_path(workspace)?.to_owned();
    let state_dir = unicode_path(state_dir)?.to_owned();
    wrap(
        spec,
        executable,
        vec![
            "--die-with-parent".to_owned(),
            "--new-session".to_owned(),
            "--unshare-user-try".to_owned(),
            "--unshare-pid".to_owned(),
            "--unshare-ipc".to_owned(),
            "--unshare-uts".to_owned(),
            "--cap-drop".to_owned(),
            "ALL".to_owned(),
            "--ro-bind".to_owned(),
            "/".to_owned(),
            "/".to_owned(),
            "--bind".to_owned(),
            workspace.clone(),
            workspace.clone(),
            "--ro-bind".to_owned(),
            state_dir.clone(),
            state_dir,
            "--chdir".to_owned(),
            workspace,
            "--".to_owned(),
        ],
    );
    Ok(SandboxAttestation {
        backend: BACKEND,
        profile: profile.as_str(),
        network_isolated: false,
    })
}

fn wrap(spec: &mut TerminalSessionSpec, executable: PathBuf, mut wrapper_args: Vec<String>) {
    wrapper_args.push(spec.program.to_string_lossy().into_owned());
    wrapper_args.append(&mut spec.args);
    spec.program = executable;
    spec.args = wrapper_args;
}

#[cfg(target_os = "macos")]
fn require_backend(backend: &'static str, executable: &Path) -> Result<(), SandboxError> {
    if executable.is_file() {
        Ok(())
    } else {
        Err(SandboxError::BackendUnavailable {
            backend,
            path: executable.to_owned(),
        })
    }
}

#[cfg(target_os = "macos")]
fn quoted_path(path: &Path) -> Result<String, SandboxError> {
    serde_json::to_string(unicode_path(path)?)
        .map_err(|_| SandboxError::NonUnicodePath(path.into()))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn unicode_path(path: &Path) -> Result<&str, SandboxError> {
    path.to_str()
        .ok_or_else(|| SandboxError::NonUnicodePath(path.to_owned()))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::*;
    #[cfg(target_os = "macos")]
    use std::{collections::BTreeMap, process::Command};
    #[cfg(target_os = "macos")]
    use ultraplexr_core::SessionId;
    #[cfg(target_os = "macos")]
    use ultraplexr_terminal::GridSize;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_workspace_profile_writes_workspace_but_protects_runtime_and_outside() {
        let root = std::env::temp_dir().join(format!("ultraplexr-sandbox-{}", SessionId::new()));
        let state = root.join(".ultraplexr");
        std::fs::create_dir_all(&state).expect("sandbox fixture should exist");
        let outside = root.with_extension("outside");
        let mut spec = TerminalSessionSpec {
            session_id: SessionId::new(),
            mission_id: None,
            run_id: None,
            program: PathBuf::from("/bin/sh"),
            args: vec![
                "-c".to_owned(),
                format!(
                    "printf allowed > allowed; printf denied > .ultraplexr/denied; printf denied > {}",
                    outside.display()
                ),
            ],
            cwd: root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).expect("valid grid"),
        };
        let attestation = apply(SandboxProfile::WorkspaceWrite, &mut spec, &state)
            .expect("macOS sandbox should prepare");
        let status = Command::new(&spec.program)
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .status()
            .expect("sandbox wrapper should execute");
        assert!(!status.success(), "forbidden writes should fail the shell");
        assert!(root.join("allowed").is_file());
        assert!(!state.join("denied").exists());
        assert!(!outside.exists());
        assert_eq!(attestation.backend, "macos_sandbox_exec");
        std::fs::remove_dir_all(root).expect("sandbox fixture should be removable");
    }
}
