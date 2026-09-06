use std::os::unix::fs::PermissionsExt;
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use ultraplexr_core::{Actor, Command, DomainError, Event, Mission, MissionId, RunId};
use ultraplexr_protocol::{MissionHistoryEntry, MissionSummary};
use thiserror::Error;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};
use uuid::Uuid;

const EVENT_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EventEnvelope {
    envelope_version: u16,
    event_id: Uuid,
    mission_id: MissionId,
    mission_sequence: u64,
    schema_version: u16,
    occurred_at_unix_micros: u64,
    correlation_id: Uuid,
    causation_id: Option<Uuid>,
    idempotency_key: Uuid,
    payload_type: String,
    payload: Event,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StoredEvent {
    Envelope(EventEnvelope),
    Legacy(Event),
}

#[derive(Clone, Copy)]
struct CommitRecord {
    mission_id: MissionId,
    start: usize,
    end: usize,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("event store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("event store contains invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("mission {0} already exists")]
    MissionAlreadyExists(MissionId),
    #[error("mission {0} does not exist")]
    MissionNotFound(MissionId),
    #[error("mission {mission_id} is at version {actual}, expected {expected}")]
    VersionConflict {
        mission_id: MissionId,
        expected: u64,
        actual: u64,
    },
    #[error("idempotency key {0} was already used for another mission")]
    IdempotencyConflict(Uuid),
}

pub struct MissionStore {
    root: PathBuf,
    missions: HashMap<MissionId, Mission>,
    histories: HashMap<MissionId, Vec<Event>>,
    records: HashMap<MissionId, Vec<MissionHistoryEntry>>,
    commits: HashMap<Uuid, CommitRecord>,
}

impl MissionStore {
    pub async fn open(root: PathBuf) -> Result<Self, StoreError> {
        fs::create_dir_all(&root).await?;
        fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).await?;
        let mut missions = HashMap::new();
        let mut histories = HashMap::new();
        let mut records = HashMap::new();
        let mut commits: HashMap<Uuid, CommitRecord> = HashMap::new();
        let mut entries = fs::read_dir(&root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).await?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(StoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("mission journal {} is not a real file", path.display()),
                )));
            }
            fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).await?;
            let stored = read_events_recovering_tail(&path).await?;
            let mut events = Vec::with_capacity(stored.len());
            let mut mission_records = Vec::with_capacity(stored.len());
            for record in stored {
                match record {
                    StoredEvent::Legacy(event) => {
                        let mission_sequence = events.len() as u64 + 1;
                        mission_records.push(legacy_history_entry(mission_sequence, &event)?);
                        events.push(event);
                    }
                    StoredEvent::Envelope(envelope) => {
                        let expected_sequence = events.len() as u64 + 1;
                        if envelope.envelope_version != 1
                            || envelope.schema_version != EVENT_SCHEMA_VERSION
                            || envelope.mission_id != envelope.payload.mission_id()
                            || envelope.mission_sequence != expected_sequence
                        {
                            return Err(StoreError::Domain(DomainError::InvalidEventStream(
                                format!(
                                    "invalid event envelope at sequence {expected_sequence} in {}",
                                    path.display()
                                ),
                            )));
                        }
                        let index = events.len();
                        if let Some(commit) = commits.get_mut(&envelope.idempotency_key) {
                            if commit.mission_id != envelope.mission_id || commit.end != index {
                                return Err(StoreError::IdempotencyConflict(
                                    envelope.idempotency_key,
                                ));
                            }
                            commit.end += 1;
                        } else {
                            commits.insert(
                                envelope.idempotency_key,
                                CommitRecord {
                                    mission_id: envelope.mission_id,
                                    start: index,
                                    end: index + 1,
                                },
                            );
                        }
                        mission_records.push(history_entry(&envelope));
                        events.push(envelope.payload);
                    }
                }
            }
            if events.is_empty() {
                continue;
            }
            let mission = Mission::rehydrate(&events)?;
            records.insert(mission.id, mission_records);
            histories.insert(mission.id, events);
            missions.insert(mission.id, mission);
        }
        Ok(Self {
            root,
            missions,
            histories,
            records,
            commits,
        })
    }

    pub async fn create(
        &mut self,
        id: MissionId,
        intent: String,
        created_by: Actor,
    ) -> Result<Mission, StoreError> {
        if self.missions.contains_key(&id) {
            return Err(StoreError::MissionAlreadyExists(id));
        }
        let (mission, event) = Mission::create(id, intent, created_by)?;
        let idempotency_key = Uuid::new_v4();
        let envelopes = envelope_events(
            id,
            0,
            std::slice::from_ref(&event),
            idempotency_key,
            idempotency_key,
        )?;
        self.append_envelopes(id, &envelopes).await?;
        self.records
            .insert(id, envelopes.iter().map(history_entry).collect::<Vec<_>>());
        self.histories.insert(id, vec![event]);
        self.missions.insert(id, mission.clone());
        Ok(mission)
    }

    pub async fn dispatch(
        &mut self,
        id: MissionId,
        command: Command,
        expected_version: Option<u64>,
        idempotency_key: Uuid,
        correlation_id: Uuid,
    ) -> Result<(Vec<Event>, Mission), StoreError> {
        self.dispatch_batch(
            id,
            vec![command],
            expected_version,
            idempotency_key,
            correlation_id,
        )
        .await
    }

    /// Decide and commit a choreography as one causally-linked durable batch.
    /// If any command fails, no event from the batch reaches the journal.
    pub async fn dispatch_batch(
        &mut self,
        id: MissionId,
        commands: Vec<Command>,
        expected_version: Option<u64>,
        idempotency_key: Uuid,
        correlation_id: Uuid,
    ) -> Result<(Vec<Event>, Mission), StoreError> {
        if let Some(committed) = self.committed(id, idempotency_key)? {
            return Ok(committed);
        }
        let mission = self
            .missions
            .get(&id)
            .ok_or(StoreError::MissionNotFound(id))?;
        if let Some(expected) = expected_version
            && mission.version != expected
        {
            return Err(StoreError::VersionConflict {
                mission_id: id,
                expected,
                actual: mission.version,
            });
        }
        let start = usize::try_from(mission.version).map_err(|_| {
            StoreError::Domain(DomainError::InvalidEventStream(
                "mission version does not fit in memory".to_owned(),
            ))
        })?;
        if commands.is_empty() {
            return Err(StoreError::Domain(DomainError::InvalidEventStream(
                "an empty command batch cannot be committed".to_owned(),
            )));
        }
        let mut decided = mission.clone();
        let mut events = Vec::new();
        let occurred_at_unix_micros = unix_timestamp_micros()?;
        for mut command in commands {
            stamp_authoritative_command(&mut command, &decided, occurred_at_unix_micros);
            let mut command_events = decided.decide(command)?;
            for event in &mut command_events {
                stamp_lifecycle_event(event, occurred_at_unix_micros);
            }
            for event in &command_events {
                decided.apply(event)?;
            }
            events.extend(command_events);
        }
        let envelopes = envelope_events(
            id,
            mission.version,
            &events,
            idempotency_key,
            correlation_id,
        )?;

        self.append_envelopes(id, &envelopes).await?;
        let mission = self
            .missions
            .get_mut(&id)
            .ok_or(StoreError::MissionNotFound(id))?;
        for event in &events {
            mission.apply(event)?;
        }
        self.histories
            .get_mut(&id)
            .ok_or(StoreError::MissionNotFound(id))?
            .extend(events.iter().cloned());
        self.records
            .get_mut(&id)
            .ok_or(StoreError::MissionNotFound(id))?
            .extend(envelopes.iter().map(history_entry));
        self.commits.insert(
            idempotency_key,
            CommitRecord {
                mission_id: id,
                start,
                end: start + events.len(),
            },
        );
        Ok((events, mission.clone()))
    }

    /// Return an earlier atomic commit before a caller repeats expensive
    /// preparation associated with the same idempotent request.
    pub fn committed(
        &self,
        id: MissionId,
        idempotency_key: Uuid,
    ) -> Result<Option<(Vec<Event>, Mission)>, StoreError> {
        let Some(commit) = self.commits.get(&idempotency_key).copied() else {
            return Ok(None);
        };
        if commit.mission_id != id {
            return Err(StoreError::IdempotencyConflict(idempotency_key));
        }
        let history = self
            .histories
            .get(&id)
            .ok_or(StoreError::MissionNotFound(id))?;
        let events = history[commit.start..commit.end].to_vec();
        let mission = Mission::rehydrate(&history[..commit.end])?;
        Ok(Some((events, mission)))
    }

    pub fn get(&self, id: MissionId) -> Result<Mission, StoreError> {
        self.missions
            .get(&id)
            .cloned()
            .ok_or(StoreError::MissionNotFound(id))
    }

    pub fn verification_status(&self, id: MissionId, verifier: RunId) -> Result<ultraplexr_core::VerificationStatus, StoreError> {
        self.missions.get(&id).ok_or(StoreError::MissionNotFound(id))?
            .verification_status(verifier).map_err(StoreError::from)
    }

    pub fn verification_catalog(&self, id: MissionId, after: Option<RunId>, limit: u16) -> Result<ultraplexr_core::VerificationCatalog, StoreError> {
        Ok(self.missions.get(&id).ok_or(StoreError::MissionNotFound(id))?
            .verification_catalog(after, limit))
    }

    pub fn list(&self) -> Vec<MissionSummary> {
        let mut missions: Vec<_> = self.missions.values().map(MissionSummary::from).collect();
        missions.sort_by_key(|mission| mission.id);
        missions
    }

    pub fn all(&self) -> Vec<Mission> {
        let mut missions: Vec<_> = self.missions.values().cloned().collect();
        missions.sort_by_key(|mission| mission.id);
        missions
    }

    pub fn history(
        &self,
        id: MissionId,
        before_sequence: Option<u64>,
        limit: u16,
    ) -> Result<(Vec<MissionHistoryEntry>, bool), StoreError> {
        let records = self
            .records
            .get(&id)
            .ok_or(StoreError::MissionNotFound(id))?;
        let before = before_sequence.unwrap_or(u64::MAX);
        let eligible = records
            .iter()
            .take_while(|entry| entry.mission_sequence < before)
            .count();
        let limit = usize::from(limit.clamp(1, 500));
        let start = eligible.saturating_sub(limit);
        Ok((records[start..eligible].to_vec(), start > 0))
    }

    async fn append_envelopes(
        &self,
        id: MissionId,
        events: &[EventEnvelope],
    ) -> Result<(), StoreError> {
        let path = self.root.join(format!("{id}.jsonl"));
        let existed = path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .await?;
        let mut batch = Vec::new();
        for event in events {
            serde_json::to_writer(&mut batch, event)?;
            batch.push(b'\n');
        }
        file.write_all(&batch).await?;
        file.flush().await?;
        file.sync_data().await?;
        if !existed {
            std::fs::File::open(&self.root)?.sync_all()?;
        }
        Ok(())
    }
}

fn history_entry(envelope: &EventEnvelope) -> MissionHistoryEntry {
    MissionHistoryEntry {
        mission_sequence: envelope.mission_sequence,
        event_id: Some(envelope.event_id),
        occurred_at_unix_micros: Some(envelope.occurred_at_unix_micros),
        correlation_id: Some(envelope.correlation_id),
        causation_id: envelope.causation_id,
        payload_type: envelope.payload_type.clone(),
        payload: envelope.payload.clone(),
    }
}

fn legacy_history_entry(
    mission_sequence: u64,
    event: &Event,
) -> Result<MissionHistoryEntry, StoreError> {
    let value = serde_json::to_value(event)?;
    let payload_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("legacy_event")
        .to_owned();
    Ok(MissionHistoryEntry {
        mission_sequence,
        event_id: None,
        occurred_at_unix_micros: None,
        correlation_id: None,
        causation_id: None,
        payload_type,
        payload: event.clone(),
    })
}

fn envelope_events(
    mission_id: MissionId,
    current_version: u64,
    events: &[Event],
    idempotency_key: Uuid,
    correlation_id: Uuid,
) -> Result<Vec<EventEnvelope>, StoreError> {
    let occurred_at_unix_micros = unix_timestamp_micros()?;
    let mut envelopes = Vec::with_capacity(events.len());
    let mut causation_id = None;
    for (offset, event) in events.iter().enumerate() {
        let value = serde_json::to_value(event)?;
        let payload_type = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                StoreError::Domain(DomainError::InvalidEventStream(
                    "event payload has no stable type tag".to_owned(),
                ))
            })?
            .to_owned();
        let event_id = Uuid::new_v4();
        envelopes.push(EventEnvelope {
            envelope_version: 1,
            event_id,
            mission_id,
            mission_sequence: current_version + offset as u64 + 1,
            schema_version: EVENT_SCHEMA_VERSION,
            occurred_at_unix_micros,
            correlation_id,
            causation_id,
            idempotency_key,
            payload_type,
            payload: event.clone(),
        });
        causation_id = Some(event_id);
    }
    Ok(envelopes)
}

fn unix_timestamp_micros() -> Result<u64, StoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .as_micros()
        .try_into()
        .map_err(|_| std::io::Error::other("system timestamp exceeds u64").into())
}

fn stamp_lifecycle_event(event: &mut Event, occurred_at_unix_micros: u64) {
    match event {
        Event::RunPlanned {
            planned_at_unix_micros,
            ..
        } => *planned_at_unix_micros = Some(occurred_at_unix_micros),
        Event::RunFinished {
            finished_at_unix_micros,
            ..
        } => *finished_at_unix_micros = Some(occurred_at_unix_micros),
        Event::RunCancelled {
            cancelled_at_unix_micros,
            ..
        } => *cancelled_at_unix_micros = Some(occurred_at_unix_micros),
        _ => {}
    }
}

fn stamp_authoritative_command(
    command: &mut Command,
    mission: &Mission,
    occurred_at_unix_micros: u64,
) {
    if let Command::VerifiedDelivery { command } = command {
        match command {
            ultraplexr_core::VerifiedDeliveryCommand::AdmitChangeIntent {
                lease_epoch,
                ..
            } => *lease_epoch = occurred_at_unix_micros.max(1),
            ultraplexr_core::VerifiedDeliveryCommand::PromoteContingentClaims {
                run_id,
                lease_epoch,
                ..
            } => {
                let next = mission
                    .verified_delivery
                    .change_intents
                    .get(run_id)
                    .map_or(1, |intent| intent.lease_epoch.saturating_add(1));
                *lease_epoch = occurred_at_unix_micros.max(next);
            }
            _ => {}
        }
    }
}

async fn read_events_recovering_tail(
    path: &std::path::Path,
) -> Result<Vec<StoredEvent>, StoreError> {
    let contents = fs::read(path).await?;
    let mut events = Vec::new();
    let mut cursor = 0;
    while cursor < contents.len() {
        let tail = &contents[cursor..];
        if let Some(relative_end) = tail.iter().position(|byte| *byte == b'\n') {
            let line = &tail[..relative_end];
            cursor += relative_end + 1;
            if !line.iter().all(u8::is_ascii_whitespace) {
                events.push(serde_json::from_slice(line)?);
            }
            continue;
        }

        if tail.iter().all(u8::is_ascii_whitespace) {
            break;
        }
        match serde_json::from_slice(tail) {
            Ok(event) => {
                events.push(event);
                let mut file = OpenOptions::new().append(true).open(path).await?;
                file.write_all(b"\n").await?;
                file.sync_data().await?;
            }
            Err(_) => {
                let file = OpenOptions::new().write(true).open(path).await?;
                file.set_len(cursor as u64).await?;
                file.sync_data().await?;
            }
        }
        break;
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultraplexr_core::{
        ChangeClaim, ChangeIntentSpec, ChangeIntentState, ChangeOperation, ChangeScope,
        FinishOutcome, RunId, RunPriority, VerifiedDeliveryCommand,
    };

    fn temporary_store_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ultraplexr-store-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ))
    }

    #[test]
    fn events_in_one_commit_form_an_explicit_causation_chain() {
        let id = MissionId::new();
        let (_, event) = Mission::create(
            id,
            "Trace one commit".to_owned(),
            Actor::human("alice").expect("test actor should be valid"),
        )
        .expect("mission event should be valid");
        let correlation = Uuid::new_v4();
        let envelopes = envelope_events(id, 0, &[event.clone(), event], correlation, correlation)
            .expect("envelopes should build");
        assert_eq!(envelopes[0].causation_id, None);
        assert_eq!(envelopes[1].causation_id, Some(envelopes[0].event_id));
        assert!(
            envelopes
                .iter()
                .all(|envelope| envelope.correlation_id == correlation)
        );
    }

    #[tokio::test]
    async fn command_batches_are_all_or_nothing() {
        let path = temporary_store_path();
        let id = MissionId::new();
        let run_id = RunId::new();
        let mut store = MissionStore::open(path.clone())
            .await
            .expect("store should open");
        store
            .create(
                id,
                "Launch atomically".to_owned(),
                Actor::human("alice").expect("test actor should be valid"),
            )
            .await
            .expect("mission should persist");

        let result = store
            .dispatch_batch(
                id,
                vec![
                    Command::StartRun {
                        run_id,
                        parent: None,
                        actor: Actor::agent("worker", "codex").expect("test actor should be valid"),
                        objective: "Do work".to_owned(),
                    },
                    Command::AssignSession {
                        session_id: ultraplexr_core::SessionId::new(),
                        run_id,
                    },
                ],
                Some(1),
                Uuid::new_v4(),
                Uuid::new_v4(),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(store.get(id).expect("mission should remain").version, 1);
        assert_eq!(
            store
                .history(id, None, 100)
                .expect("history should remain readable")
                .0
                .len(),
            1
        );

        fs::remove_dir_all(path)
            .await
            .expect("temporary store should be removable");
    }

    #[tokio::test]
    async fn store_stamps_and_replays_trusted_run_lifecycle_times() {
        let path = temporary_store_path();
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let mut store = MissionStore::open(path.clone())
            .await
            .expect("store should open");
        store
            .create(
                mission_id,
                "Persist scheduler time".to_owned(),
                Actor::human("alice").expect("test actor should be valid"),
            )
            .await
            .expect("mission should persist");
        for command in [
            Command::PlanRun {
                run_id,
                parent: None,
                dependencies: Vec::new(),
                retry_of: None,
                actor: Actor::agent("worker", "codex").expect("test actor should be valid"),
                objective: "Age durably".to_owned(),
                priority: RunPriority::Background,
            },
            Command::StartReadyRun { run_id },
            Command::FinishRun {
                run_id,
                outcome: FinishOutcome::Succeeded,
                summary: "done".to_owned(),
            },
        ] {
            let key = Uuid::new_v4();
            store
                .dispatch(mission_id, command, None, key, key)
                .await
                .expect("lifecycle command should persist");
        }
        let committed = store.get(mission_id).expect("mission should remain");
        let run = &committed.runs[&run_id];
        let planned_at = run
            .planned_at_unix_micros
            .expect("store should stamp planning time");
        let finished_at = run
            .finished_at_unix_micros
            .expect("store should stamp completion time");
        assert!(finished_at >= planned_at);
        drop(store);

        let reopened = MissionStore::open(path.clone())
            .await
            .expect("store should reopen");
        let replayed = reopened.get(mission_id).expect("mission should replay");
        assert_eq!(
            replayed.runs[&run_id].planned_at_unix_micros,
            Some(planned_at)
        );
        assert_eq!(
            replayed.runs[&run_id].finished_at_unix_micros,
            Some(finished_at)
        );

        fs::remove_dir_all(path)
            .await
            .expect("temporary store should be removable");
    }

    #[tokio::test]
    async fn store_replaces_caller_supplied_execution_lease_epoch() {
        let path = temporary_store_path();
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let mut store = MissionStore::open(path.clone())
            .await
            .expect("store should open");
        store
            .create(
                mission_id,
                "Fence one exact change".to_owned(),
                Actor::human("alice").expect("test actor should be valid"),
            )
            .await
            .expect("mission should persist");
        for command in [
            Command::PlanRun {
                run_id,
                parent: None,
                dependencies: Vec::new(),
                retry_of: None,
                actor: Actor::agent("worker", "codex").expect("test actor should be valid"),
                objective: "Edit the claimed file".to_owned(),
                priority: RunPriority::Normal,
            },
            Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::DeclareChangeIntent {
                    run_id,
                    expected_version: None,
                    spec: ChangeIntentSpec {
                        repository_identity: "fixture-repository".to_owned(),
                        base_revision: "abc123".to_owned(),
                        claims: vec![ChangeClaim {
                            path: "src/main.rs".to_owned(),
                            operation: ChangeOperation::Modify,
                            scope: ChangeScope::Committed,
                        }],
                    },
                },
            },
        ] {
            let key = Uuid::new_v4();
            store
                .dispatch(mission_id, command, None, key, key)
                .await
                .expect("setup command should persist");
        }

        let caller_epoch = 999;
        let key = Uuid::new_v4();
        store
            .dispatch(
                mission_id,
                Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::AdmitChangeIntent {
                        run_id,
                        expected_version: 1,
                        lease_epoch: caller_epoch,
                    },
                },
                None,
                key,
                key,
            )
            .await
            .expect("server-authored admission should persist");

        let mission = store.get(mission_id).expect("mission should remain");
        let intent = &mission.verified_delivery.change_intents[&run_id];
        assert_eq!(intent.state, ChangeIntentState::Admitted);
        assert_ne!(intent.lease_epoch, caller_epoch);
        assert!(intent.lease_epoch > 1_000_000);

        fs::remove_dir_all(path)
            .await
            .expect("temporary store should be removable");
    }

    #[tokio::test]
    async fn missions_survive_reopening_the_store() {
        let path = temporary_store_path();
        let id = MissionId::new();
        {
            let mut store = MissionStore::open(path.clone())
                .await
                .expect("store should open");
            store
                .create(
                    id,
                    "Persist agent work".to_owned(),
                    Actor::human("alice").expect("test actor should be valid"),
                )
                .await
                .expect("mission should persist");
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(path.join(format!("{id}.jsonl")))
                    .expect("mission log metadata should exist")
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let reopened = MissionStore::open(path.clone())
            .await
            .expect("store should reopen");
        assert_eq!(
            reopened.get(id).expect("mission should exist").intent,
            "Persist agent work"
        );

        fs::remove_dir_all(path)
            .await
            .expect("temporary store should be removable");
    }

    #[tokio::test]
    async fn incomplete_crash_tail_is_truncated_without_losing_committed_events() {
        let path = temporary_store_path();
        let id = MissionId::new();
        let journal = path.join(format!("{id}.jsonl"));
        {
            let mut store = MissionStore::open(path.clone())
                .await
                .expect("store should open");
            store
                .create(
                    id,
                    "Recover after power loss".to_owned(),
                    Actor::human("alice").expect("test actor should be valid"),
                )
                .await
                .expect("mission should persist");
        }
        let committed_len = fs::metadata(&journal)
            .await
            .expect("journal should exist")
            .len();
        let mut file = OpenOptions::new()
            .append(true)
            .open(&journal)
            .await
            .expect("journal should open");
        file.write_all(b"{\"type\":\"run_started\"")
            .await
            .expect("partial crash tail should write");
        file.sync_data().await.expect("tail should reach disk");
        drop(file);

        let reopened = MissionStore::open(path.clone())
            .await
            .expect("partial final record should be recoverable");
        assert_eq!(
            reopened.get(id).expect("mission should survive").intent,
            "Recover after power loss"
        );
        assert_eq!(
            fs::metadata(&journal)
                .await
                .expect("journal should exist")
                .len(),
            committed_len
        );
        fs::remove_dir_all(path)
            .await
            .expect("temporary store should be removable");
    }

    #[tokio::test]
    async fn corruption_before_a_record_boundary_is_not_silently_discarded() {
        let path = temporary_store_path();
        let id = MissionId::new();
        let journal = path.join(format!("{id}.jsonl"));
        {
            let mut store = MissionStore::open(path.clone())
                .await
                .expect("store should open");
            store
                .create(
                    id,
                    "Reject committed corruption".to_owned(),
                    Actor::human("alice").expect("test actor should be valid"),
                )
                .await
                .expect("mission should persist");
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&journal)
            .await
            .expect("journal should open");
        file.write_all(b"not-json\n")
            .await
            .expect("corrupt record should write");
        file.sync_data().await.expect("record should reach disk");
        drop(file);

        assert!(matches!(
            MissionStore::open(path.clone()).await,
            Err(StoreError::Json(_))
        ));
        fs::remove_dir_all(path)
            .await
            .expect("temporary store should be removable");
    }

    #[tokio::test]
    async fn expected_versions_and_idempotency_survive_reopening() {
        let path = temporary_store_path();
        let id = MissionId::new();
        let key = Uuid::new_v4();
        let command = Command::StartRun {
            run_id: RunId::new(),
            parent: None,
            actor: Actor::agent("worker", "codex").expect("test actor should be valid"),
            objective: "Perform one durable mutation".to_owned(),
        };
        {
            let mut store = MissionStore::open(path.clone())
                .await
                .expect("store should open");
            store
                .create(
                    id,
                    "Test concurrency controls".to_owned(),
                    Actor::human("alice").expect("test actor should be valid"),
                )
                .await
                .expect("mission should persist");
            let (events, committed) = store
                .dispatch(id, command.clone(), Some(1), key, key)
                .await
                .expect("matching version should commit");
            assert_eq!(events.len(), 1);
            assert_eq!(committed.version, 2);
            let (retried_events, retried) = store
                .dispatch(id, command.clone(), Some(1), key, key)
                .await
                .expect("same key should return its original commit");
            assert_eq!(retried_events, events);
            assert_eq!(retried.version, 2);
            assert_eq!(store.get(id).expect("mission should exist").version, 2);
            let (history, has_more) = store
                .history(id, None, 100)
                .expect("history should be queryable");
            assert!(!has_more);
            assert_eq!(history.len(), 2);
            assert_eq!(history[0].mission_sequence, 1);
            assert_eq!(history[1].mission_sequence, 2);
            assert_eq!(history[1].correlation_id, Some(key));
            assert_eq!(history[1].payload_type, "run_started");
            let (first_page, has_more) = store
                .history(id, Some(2), 1)
                .expect("history should page backwards");
            assert!(!has_more);
            assert_eq!(first_page[0].mission_sequence, 1);
            assert!(matches!(
                store
                    .dispatch(id, command.clone(), Some(1), Uuid::new_v4(), Uuid::new_v4(),)
                    .await,
                Err(StoreError::VersionConflict {
                    expected: 1,
                    actual: 2,
                    ..
                })
            ));
        }

        let mut reopened = MissionStore::open(path.clone())
            .await
            .expect("enveloped store should reopen");
        let (_, retried) = reopened
            .dispatch(id, command, Some(1), key, key)
            .await
            .expect("persisted key should remain idempotent");
        assert_eq!(retried.version, 2);
        assert_eq!(reopened.get(id).expect("mission should exist").version, 2);
        let (history, _) = reopened
            .history(id, None, 100)
            .expect("envelope metadata should survive reopening");
        assert_eq!(history[1].correlation_id, Some(key));

        let journal = fs::read_to_string(path.join(format!("{id}.jsonl")))
            .await
            .expect("journal should read");
        assert!(journal.lines().all(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|value| value.get("envelope_version").cloned())
                == Some(serde_json::json!(1))
        }));
        fs::remove_dir_all(path)
            .await
            .expect("temporary store should be removable");
    }
}
