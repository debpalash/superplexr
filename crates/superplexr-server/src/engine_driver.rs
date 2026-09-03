use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use superplexr_core::{MissionId, RunId, SessionId};
use superplexr_protocol::TerminalSessionSpec;
use superplexr_terminal::GridSize;
use thiserror::Error;

use crate::sandbox::{self, SandboxAttestation, SandboxProfile};

pub(crate) const CONFIG_VERSION: u16 = 1;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_DRIVERS: usize = 64;
const MAX_ARGS: usize = 128;
const MAX_ENVIRONMENT_CHANGES: usize = 64;
const MAX_VALUE_BYTES: usize = 16 * 1024;

pub(crate) struct PrepareRequest<'a> {
    pub engine: &'a str,
    pub objective: &'a str,
    pub mission_id: MissionId,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub cwd: PathBuf,
    pub grid: GridSize,
}

#[derive(Debug)]
pub(crate) struct PreparedDriver {
    pub spec: TerminalSessionSpec,
    pub sandbox: Option<SandboxAttestation>,
}

#[derive(Debug, Error)]
pub(crate) enum DriverError {
    #[error("named agent driver configuration does not exist at {0}")]
    Missing(PathBuf),
    #[error("agent driver configuration is not a regular owner-controlled file: {0}")]
    Insecure(PathBuf),
    #[error("agent driver configuration exceeds 1 MiB")]
    TooLarge,
    #[error("agent driver configuration could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent driver configuration is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported agent driver configuration version {0}")]
    Version(u16),
    #[error("agent driver configuration exceeds {MAX_DRIVERS} drivers")]
    TooManyDrivers,
    #[error("no configured agent driver matches engine {0:?}")]
    UnknownEngine(String),
    #[error("agent driver {0:?} has an empty program")]
    EmptyProgram(String),
    #[error("agent driver {0:?} exceeds its argument or environment limits")]
    Limits(String),
    #[error("agent driver {driver:?} attempts to set reserved environment key {key:?}")]
    ReservedEnvironment { driver: String, key: String },
    #[error("agent driver {driver:?} contains NUL in {field}")]
    Nul { driver: String, field: &'static str },
    #[error(transparent)]
    Sandbox(#[from] sandbox::SandboxError),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DriverFile {
    version: u16,
    drivers: BTreeMap<String, DriverConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DriverConfig {
    program: PathBuf,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    append_objective: bool,
    #[serde(default)]
    environment_delta: BTreeMap<String, Option<String>>,
    #[serde(default)]
    sandbox: Option<SandboxProfile>,
}

pub(crate) fn prepare(
    config_path: &Path,
    request: PrepareRequest<'_>,
) -> Result<PreparedDriver, DriverError> {
    let metadata = match fs::symlink_metadata(config_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(DriverError::Missing(config_path.to_owned()));
        }
        Err(error) => return Err(error.into()),
    };
    // SAFETY: geteuid has no preconditions and does not dereference memory.
    let current_uid = unsafe { libc::geteuid() };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.uid() != current_uid {
        return Err(DriverError::Insecure(config_path.to_owned()));
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(DriverError::TooLarge);
    }
    fs::set_permissions(config_path, fs::Permissions::from_mode(0o600))?;
    let contents = fs::read(config_path)?;
    let file: DriverFile = serde_json::from_slice(&contents)?;
    if file.version != CONFIG_VERSION {
        return Err(DriverError::Version(file.version));
    }
    if file.drivers.len() > MAX_DRIVERS {
        return Err(DriverError::TooManyDrivers);
    }
    let driver = file
        .drivers
        .get(request.engine)
        .ok_or_else(|| DriverError::UnknownEngine(request.engine.to_owned()))?;
    validate_driver(request.engine, driver)?;

    let mut args = driver.args.clone();
    if driver.append_objective {
        args.push(request.objective.to_owned());
    }
    let mut spec = TerminalSessionSpec {
        session_id: request.session_id,
        mission_id: Some(request.mission_id),
        run_id: Some(request.run_id),
        program: driver.program.clone(),
        args,
        cwd: request.cwd,
        environment_delta: driver.environment_delta.clone(),
        grid: request.grid,
    };
    let sandbox = driver
        .sandbox
        .map(|profile| {
            sandbox::apply(
                profile,
                &mut spec,
                config_path
                    .parent()
                    .expect("driver configuration path has a parent"),
            )
        })
        .transpose()?;
    Ok(PreparedDriver { spec, sandbox })
}

fn validate_driver(name: &str, driver: &DriverConfig) -> Result<(), DriverError> {
    if driver.program.as_os_str().is_empty() {
        return Err(DriverError::EmptyProgram(name.to_owned()));
    }
    if driver.args.len() > MAX_ARGS
        || driver.environment_delta.len() > MAX_ENVIRONMENT_CHANGES
        || driver
            .args
            .iter()
            .any(|argument| argument.len() > MAX_VALUE_BYTES)
        || driver.environment_delta.iter().any(|(key, value)| {
            key.len() > MAX_VALUE_BYTES
                || value
                    .as_ref()
                    .is_some_and(|value| value.len() > MAX_VALUE_BYTES)
        })
    {
        return Err(DriverError::Limits(name.to_owned()));
    }
    if driver.program.as_os_str().to_string_lossy().contains('\0') {
        return Err(DriverError::Nul {
            driver: name.to_owned(),
            field: "program",
        });
    }
    if driver.args.iter().any(|argument| argument.contains('\0')) {
        return Err(DriverError::Nul {
            driver: name.to_owned(),
            field: "arguments",
        });
    }
    for (key, value) in &driver.environment_delta {
        if key.starts_with("SUPERPLEXR_") {
            return Err(DriverError::ReservedEnvironment {
                driver: name.to_owned(),
                key: key.clone(),
            });
        }
        if key.is_empty()
            || key.contains(['=', '\0'])
            || value.as_ref().is_some_and(|value| value.contains('\0'))
        {
            return Err(DriverError::Nul {
                driver: name.to_owned(),
                field: "environment",
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    fn write_config(root: &Path, contents: &str) -> PathBuf {
        fs::create_dir_all(root).expect("test directory should be created");
        let path = root.join("engines.json");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .expect("config should open");
        file.write_all(contents.as_bytes())
            .expect("config should write");
        path
    }

    #[test]
    fn configured_driver_prepares_structured_argv_and_preserves_identity() {
        let root = std::env::temp_dir().join(format!("superplexr-driver-{}", SessionId::new()));
        let path = write_config(
            &root,
            r#"{
                "version": 1,
                "drivers": {
                    "codex": {
                        "program": "codex",
                        "args": ["exec", "--full-auto"],
                        "append_objective": true,
                        "environment_delta": {"AGENT_MODE": "durable"}
                    }
                }
            }"#,
        );
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let session_id = SessionId::new();
        let prepared = prepare(
            &path,
            PrepareRequest {
                engine: "codex",
                objective: "Implement the graph",
                mission_id,
                run_id,
                session_id,
                cwd: PathBuf::from("/tmp"),
                grid: GridSize::new(120, 36).expect("grid should be valid"),
            },
        )
        .expect("driver should prepare");
        let spec = prepared.spec;
        assert_eq!(spec.program, PathBuf::from("codex"));
        assert_eq!(spec.args, ["exec", "--full-auto", "Implement the graph"]);
        assert_eq!(spec.mission_id, Some(mission_id));
        assert_eq!(spec.run_id, Some(run_id));
        assert!(prepared.sandbox.is_none());
        assert_eq!(
            spec.environment_delta.get("AGENT_MODE"),
            Some(&Some("durable".to_owned()))
        );
        fs::remove_dir_all(root).expect("test directory should be removable");
    }

    #[test]
    fn configured_driver_cannot_spoof_runtime_identity() {
        let root = std::env::temp_dir().join(format!("superplexr-driver-{}", SessionId::new()));
        let path = write_config(
            &root,
            r#"{
                "version": 1,
                "drivers": {
                    "bad": {
                        "program": "/bin/true",
                        "environment_delta": {"SUPERPLEXR_RUN_ID": "spoofed"}
                    }
                }
            }"#,
        );
        let error = prepare(
            &path,
            PrepareRequest {
                engine: "bad",
                objective: "No",
                mission_id: MissionId::new(),
                run_id: RunId::new(),
                session_id: SessionId::new(),
                cwd: PathBuf::from("/tmp"),
                grid: GridSize::new(80, 24).expect("grid should be valid"),
            },
        )
        .expect_err("reserved identity must be rejected");
        assert!(matches!(error, DriverError::ReservedEnvironment { .. }));
        fs::remove_dir_all(root).expect("test directory should be removable");
    }

    #[test]
    fn configured_driver_can_require_an_attested_workspace_sandbox() {
        let root = std::env::temp_dir().join(format!("superplexr-driver-{}", SessionId::new()));
        let state = root.join(".superplexr");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace should exist");
        let path = write_config(
            &state,
            r#"{
                "version": 1,
                "drivers": {
                    "contained": {
                        "program": "/bin/echo",
                        "args": ["safe"],
                        "sandbox": "workspace_write"
                    }
                }
            }"#,
        );
        let prepared = prepare(
            &path,
            PrepareRequest {
                engine: "contained",
                objective: "Confined execution",
                mission_id: MissionId::new(),
                run_id: RunId::new(),
                session_id: SessionId::new(),
                cwd: workspace.canonicalize().expect("workspace should resolve"),
                grid: GridSize::new(80, 24).expect("grid should be valid"),
            },
        )
        .expect("sandboxed driver should prepare");
        let sandbox = prepared.sandbox.expect("sandbox should be attested");
        assert_eq!(sandbox.profile, "workspace_write");
        assert!(!sandbox.network_isolated);
        assert!(prepared.spec.args.iter().any(|arg| arg == "/bin/echo"));
        assert!(prepared.spec.args.iter().any(|arg| arg == "safe"));
        fs::remove_dir_all(root).expect("test directory should be removable");
    }
}
