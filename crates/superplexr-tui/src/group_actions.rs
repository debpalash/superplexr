//! Explicit owner-only group writes. One admitted action, no automatic retries.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use superplexr_client::ControlClient;
use superplexr_protocol::{SessionGroupChange, SessionGroupSummary};

pub enum Action {
    Rename(SessionGroupSummary, String),
    Pin(SessionGroupSummary),
    Position(SessionGroupSummary, u32),
}

impl Action {
    fn execute(self, client: &ControlClient) -> Result<String, String> {
        let (id, operation, result) = match self {
            Self::Rename(group, name) => (
                group.group_id,
                "Rename",
                client.update_session_group(
                    group.group_id,
                    group.version,
                    SessionGroupChange::Rename { name },
                ),
            ),
            Self::Pin(group) => (
                group.group_id,
                "Pin change",
                client.update_session_group(
                    group.group_id,
                    group.version,
                    SessionGroupChange::SetPinned {
                        pinned: !group.pinned,
                    },
                ),
            ),
            Self::Position(group, position) => (
                group.group_id,
                "Order change",
                client.update_session_group(
                    group.group_id,
                    group.version,
                    SessionGroupChange::SetPosition { position },
                ),
            ),
        };
        match result {
            Ok(group) => Ok(format!(
                "{operation} saved: {} ({})",
                group.name, group.group_id
            )),
            Err(error) => Err(format!(
                "{operation} unconfirmed for {id}: {}. Refresh before retrying; no write was replayed.",
                error.to_string().chars().take(768).collect::<String>()
            )),
        }
    }
}

pub struct Worker {
    send: mpsc::SyncSender<Action>,
    receive: mpsc::Receiver<Result<String, String>>,
    stopped: Arc<AtomicBool>,
    busy: bool,
}

impl Worker {
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn new(client: ControlClient) -> std::io::Result<Self> {
        if client.is_shared() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Group edits require an owner connection",
            ));
        }
        let (send, commands) = mpsc::sync_channel::<Action>(1);
        let (results, receive) = mpsc::sync_channel(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stopped);
        std::thread::Builder::new()
            .name("superplexr-group-actions".into())
            .spawn(move || {
                while let Ok(action) = commands.recv() {
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let result = action.execute(&client);
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            send,
            receive,
            stopped,
            busy: false,
        })
    }

    pub fn submit(&mut self, action: Action) -> Result<(), String> {
        if self.busy {
            return Err("A group action is already pending; wait for its result".into());
        }
        self.send
            .try_send(action)
            .map_err(|_| "Group action worker unavailable; no new action was queued".to_owned())?;
        self.busy = true;
        Ok(())
    }

    pub fn take(&mut self) -> Option<Result<String, String>> {
        if !self.busy {
            return None;
        }
        match self.receive.try_recv() {
            Ok(result) => {
                self.busy = false;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                Some(Err(
                    "Group worker stopped; outcome unknown. Refresh before retrying.".into(),
                ))
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Do not join a stalled write or try to undo an admitted user action.
        self.stopped.store(true, Ordering::Release);
    }
}
