use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::{process::Command, time::sleep};

const MAX_SOCKET_PATH_BYTES: usize = 100;
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(15);
const HEALTHY_CONNECTION: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct ReconnectBackoff {
    next: Duration,
}

impl ReconnectBackoff {
    const fn new() -> Self {
        Self {
            next: INITIAL_RECONNECT_DELAY,
        }
    }

    fn after_connection(&mut self, connected_for: Duration) -> Duration {
        if connected_for >= HEALTHY_CONNECTION {
            self.next = INITIAL_RECONNECT_DELAY;
        }
        let delay = self.next;
        self.next = (self.next * 2).min(MAX_RECONNECT_DELAY);
        delay
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TunnelSpec {
    pub destination: String,
    pub local_socket: PathBuf,
    pub remote_socket: PathBuf,
    pub port: Option<u16>,
    pub identity: Option<PathBuf>,
    pub known_hosts: Option<PathBuf>,
    pub jump: Option<String>,
    pub once: bool,
}

#[derive(Debug, Error)]
pub(crate) enum TunnelError {
    #[error("SSH destination is empty or option-like")]
    InvalidDestination,
    #[error("{name} socket path must be absolute, bounded, and contain no colon or NUL")]
    InvalidSocketPath { name: &'static str },
    #[error("local forwarding directory is not a real owner-controlled directory: {0}")]
    InsecureLocalDirectory(PathBuf),
    #[error("local forwarding socket already exists: {0}")]
    LocalSocketExists(PathBuf),
    #[error("SSH identity is not a regular file: {0}")]
    InvalidIdentity(PathBuf),
    #[error("SSH known-hosts database is not a regular file: {0}")]
    InvalidKnownHosts(PathBuf),
    #[error("OpenSSH client is unavailable at /usr/bin/ssh or /bin/ssh")]
    SshUnavailable,
    #[error("could not start or supervise OpenSSH: {0}")]
    Io(#[from] std::io::Error),
    #[error("OpenSSH tunnel exited with status {0}")]
    Exited(std::process::ExitStatus),
}

pub(crate) async fn run(spec: TunnelSpec) -> Result<(), TunnelError> {
    let executable = ssh_executable().ok_or(TunnelError::SshUnavailable)?;
    let mut reconnect_backoff = ReconnectBackoff::new();
    loop {
        validate(&spec)?;
        let started_at = Instant::now();
        eprintln!(
            "opening encrypted superplexr attachment {} -> {} through {}",
            spec.local_socket.display(),
            spec.remote_socket.display(),
            spec.destination
        );
        let mut child = command(executable, &spec)
            .kill_on_drop(true)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        let outcome = tokio::select! {
            status = child.wait() => Some(status?),
            signal = tokio::signal::ctrl_c() => {
                signal?;
                child.start_kill()?;
                let _ = child.wait().await;
                None
            }
        };
        remove_owned_socket(&spec.local_socket)?;
        let Some(status) = outcome else {
            return Ok(());
        };
        if spec.once {
            return if status.success() {
                Ok(())
            } else {
                Err(TunnelError::Exited(status))
            };
        }
        let reconnect_delay = reconnect_backoff.after_connection(started_at.elapsed());
        eprintln!(
            "remote attachment closed ({status}); reconnecting in {}s",
            reconnect_delay.as_secs()
        );
        tokio::select! {
            () = sleep(reconnect_delay) => {}
            signal = tokio::signal::ctrl_c() => {
                signal?;
                return Ok(());
            }
        }
    }
}

fn validate(spec: &TunnelSpec) -> Result<(), TunnelError> {
    if spec.destination.is_empty() || spec.destination.starts_with('-') {
        return Err(TunnelError::InvalidDestination);
    }
    validate_socket_path("local", &spec.local_socket)?;
    validate_socket_path("remote", &spec.remote_socket)?;
    let parent = spec
        .local_socket
        .parent()
        .ok_or(TunnelError::InvalidSocketPath { name: "local" })?;
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|_| TunnelError::InsecureLocalDirectory(parent.to_owned()))?;
    // SAFETY: geteuid reads immutable process credentials and has no preconditions.
    let current_uid = unsafe { libc::geteuid() };
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != current_uid
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(TunnelError::InsecureLocalDirectory(parent.to_owned()));
    }
    if std::fs::symlink_metadata(&spec.local_socket).is_ok() {
        return Err(TunnelError::LocalSocketExists(spec.local_socket.clone()));
    }
    if let Some(identity) = &spec.identity
        && !identity.is_file()
    {
        return Err(TunnelError::InvalidIdentity(identity.clone()));
    }
    if let Some(known_hosts) = &spec.known_hosts
        && !known_hosts.is_file()
    {
        return Err(TunnelError::InvalidKnownHosts(known_hosts.clone()));
    }
    Ok(())
}

fn validate_socket_path(name: &'static str, path: &Path) -> Result<(), TunnelError> {
    let encoded = path.as_os_str().as_encoded_bytes();
    if !path.is_absolute()
        || encoded.is_empty()
        || encoded.len() > MAX_SOCKET_PATH_BYTES
        || encoded.contains(&b':')
        || encoded.contains(&0)
    {
        return Err(TunnelError::InvalidSocketPath { name });
    }
    Ok(())
}

fn ssh_executable() -> Option<&'static Path> {
    [Path::new("/usr/bin/ssh"), Path::new("/bin/ssh")]
        .into_iter()
        .find(|path| path.is_file())
}

fn command(executable: &Path, spec: &TunnelSpec) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("-N")
        .arg("-T")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("-o")
        .arg("StreamLocalBindMask=0177")
        .arg("-o")
        .arg("StreamLocalBindUnlink=no")
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg("ConnectTimeout=10");
    if let Some(port) = spec.port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = &spec.identity {
        command.arg("-i").arg(identity);
    }
    if let Some(known_hosts) = &spec.known_hosts {
        command
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", known_hosts.display()))
            .arg("-o")
            .arg("StrictHostKeyChecking=yes");
    }
    if let Some(jump) = &spec.jump {
        command.arg("-J").arg(jump);
    }
    command
        .arg("-L")
        .arg(format!(
            "{}:{}",
            spec.local_socket.display(),
            spec.remote_socket.display()
        ))
        .arg(&spec.destination);
    command
}

fn remove_owned_socket(path: &Path) -> Result<(), TunnelError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    // SAFETY: geteuid reads immutable process credentials and has no preconditions.
    let current_uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_socket() || metadata.uid() != current_uid {
        return Err(TunnelError::LocalSocketExists(path.to_owned()));
    }
    std::fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> TunnelSpec {
        TunnelSpec {
            destination: "dev@example.test".to_owned(),
            local_socket: root.join("control.sock"),
            remote_socket: PathBuf::from("/srv/superplexr/control.sock"),
            port: Some(2222),
            identity: None,
            known_hosts: None,
            jump: Some("bastion@example.test".to_owned()),
            once: true,
        }
    }

    #[test]
    fn tunnel_arguments_are_structured_and_owner_socket_is_not_auto_unlinked() {
        let root = PathBuf::from("/tmp").join(format!("t9-tunnel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("fixture directory should exist");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .expect("fixture permissions should apply");
        let spec = fixture(&root);
        std::fs::write(root.join("known_hosts"), b"fixture")
            .expect("known-hosts fixture should exist");
        let spec = TunnelSpec {
            known_hosts: Some(root.join("known_hosts")),
            ..spec
        };
        validate(&spec).expect("fixture should validate");
        let command = command(Path::new("/usr/bin/ssh"), &spec);
        let arguments = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-o", "StreamLocalBindUnlink=no"])
        );
        assert!(arguments.windows(2).any(|pair| pair == ["-p", "2222"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-J", "bastion@example.test"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| { pair[0] == "-o" && pair[1].starts_with("UserKnownHostsFile=") })
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-o", "StrictHostKeyChecking=yes"])
        );
        assert!(arguments.contains(&format!(
            "{}:/srv/superplexr/control.sock",
            spec.local_socket.display()
        )));
        assert_eq!(arguments.last(), Some(&"dev@example.test".to_owned()));
        std::fs::remove_file(root.join("known_hosts"))
            .expect("known-hosts fixture should be removable");
        std::fs::remove_dir(root).expect("fixture directory should be removable");
    }

    #[test]
    fn tunnel_rejects_insecure_parent_existing_target_and_relative_remote() {
        let root = PathBuf::from("/tmp").join(format!("t9-tunnel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("fixture directory should exist");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
            .expect("fixture permissions should apply");
        let mut spec = fixture(&root);
        assert!(matches!(
            validate(&spec),
            Err(TunnelError::InsecureLocalDirectory(_))
        ));
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .expect("fixture permissions should apply");
        std::fs::write(&spec.local_socket, b"occupied").expect("target fixture should exist");
        assert!(matches!(
            validate(&spec),
            Err(TunnelError::LocalSocketExists(_))
        ));
        std::fs::remove_file(&spec.local_socket).expect("target fixture should be removable");
        spec.remote_socket = PathBuf::from("relative.sock");
        assert!(matches!(
            validate(&spec),
            Err(TunnelError::InvalidSocketPath { name: "remote" })
        ));
        std::fs::remove_dir(root).expect("fixture directory should be removable");
    }

    #[test]
    fn reconnect_backoff_is_bounded_and_resets_after_a_healthy_channel() {
        let mut backoff = ReconnectBackoff::new();
        let delays = (0..7)
            .map(|_| backoff.after_connection(Duration::ZERO).as_secs())
            .collect::<Vec<_>>();
        assert_eq!(delays, [1, 2, 4, 8, 15, 15, 15]);
        assert_eq!(
            backoff.after_connection(HEALTHY_CONNECTION),
            INITIAL_RECONNECT_DELAY
        );
        assert_eq!(backoff.after_connection(Duration::ZERO).as_secs(), 2);
    }

    #[test]
    fn cleanup_removes_only_an_owned_unix_socket() {
        let root = PathBuf::from("/tmp").join(format!("t9-tunnel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("fixture directory should exist");
        let socket = root.join("control.sock");
        let listener =
            std::os::unix::net::UnixListener::bind(&socket).expect("socket fixture should bind");
        drop(listener);
        remove_owned_socket(&socket).expect("owned socket should be removed");
        assert!(!socket.exists());
        std::fs::write(&socket, b"not a socket").expect("regular fixture should write");
        assert!(matches!(
            remove_owned_socket(&socket),
            Err(TunnelError::LocalSocketExists(_))
        ));
        assert!(
            socket.is_file(),
            "cleanup must preserve a non-socket target"
        );
        std::fs::remove_file(socket).expect("regular fixture should be removable");
        std::fs::remove_dir(root).expect("fixture directory should be removable");
    }
}
