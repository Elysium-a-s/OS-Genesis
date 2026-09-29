use std::path::Path;

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    User,
    Service,
    Automation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Actor {
    pub actor_type: ActorType,
    pub actor_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Accepted,
    Sent,
    ProviderConfirmed,
    DeviceConfirmed,
    Unknown,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ProviderAck,
    DeviceObservation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub reference: String,
    pub at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CommandRequest {
    pub household_id: String,
    pub command_id: String,
    pub device_id: String,
    pub capability_id: String,
    pub value: Value,
    pub actor: Actor,
    pub idempotency_key: String,
    pub correlation_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Snapshot {
    pub request: CommandRequest,
    pub status: Status,
    pub confirmation_level: String,
    pub status_changed_at: String,
    pub reason: Option<String>,
    pub evidence: Option<Evidence>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Event {
    pub command_id: String,
    pub status: Status,
    pub at: String,
    pub actor: Actor,
    pub correlation_id: String,
    pub idempotency_key: String,
    pub reason: Option<String>,
    pub evidence: Option<Evidence>,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid command: {0}")]
    Invalid(&'static str),
    #[error("idempotency key already belongs to a different command intent")]
    IdempotencyConflict,
    #[error("command not found")]
    NotFound,
    #[error("invalid command status transition or evidence")]
    InvalidTransition,
}

pub struct Ledger {
    connection: Connection,
}

impl Ledger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    #[cfg(test)]
    fn in_memory() -> Result<Self, LedgerError> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(connection: Connection) -> Result<Self, LedgerError> {
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS commands (
               command_id TEXT PRIMARY KEY,
               household_id TEXT NOT NULL,
               idempotency_key TEXT NOT NULL,
               intent_json TEXT NOT NULL,
               snapshot_json TEXT NOT NULL,
               UNIQUE (household_id, idempotency_key)
             );
             CREATE TABLE IF NOT EXISTS command_events (
               sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               command_id TEXT NOT NULL REFERENCES commands(command_id),
               event_json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS command_events_by_command
               ON command_events(command_id, sequence);",
        )?;
        connection.execute_batch(crate::grant::SCHEMA)?;
        crate::grant::migrate(&connection)?;
        Ok(Self { connection })
    }

    /// Časovo obmedzené granty žijú v tej istej databáze ako povely, na ktoré sa
    /// odvolávajú; `grant` si cez tento prístup drží vlastnú schému u seba.
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn accept(&mut self, request: CommandRequest) -> Result<Snapshot, LedgerError> {
        validate_request(&request)?;
        let intent = serde_json::to_string(&serde_json::json!({
            "household_id": request.household_id,
            "device_id": request.device_id,
            "capability_id": request.capability_id,
            "value": request.value,
            "actor": request.actor,
        }))?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT intent_json, snapshot_json FROM commands
                 WHERE household_id = ?1 AND idempotency_key = ?2",
                params![request.household_id, request.idempotency_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((old_intent, old_snapshot)) = existing {
            if old_intent != intent {
                return Err(LedgerError::IdempotencyConflict);
            }
            return Ok(serde_json::from_str(&old_snapshot)?);
        }

        let at = now_utc();
        let snapshot = Snapshot {
            request: request.clone(),
            status: Status::Accepted,
            confirmation_level: "none".to_owned(),
            status_changed_at: at.clone(),
            reason: None,
            evidence: None,
        };
        let event = Event {
            command_id: request.command_id.clone(),
            status: Status::Accepted,
            at,
            actor: request.actor.clone(),
            correlation_id: request.correlation_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            reason: None,
            evidence: None,
        };
        tx.execute(
            "INSERT INTO commands
             (command_id, household_id, idempotency_key, intent_json, snapshot_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                request.command_id,
                request.household_id,
                request.idempotency_key,
                intent,
                serde_json::to_string(&snapshot)?
            ],
        )?;
        tx.execute(
            "INSERT INTO command_events (command_id, event_json) VALUES (?1, ?2)",
            params![event.command_id, serde_json::to_string(&event)?],
        )?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub fn transition(
        &mut self,
        command_id: &str,
        status: Status,
        actor: Actor,
        evidence: Option<Evidence>,
        reason: Option<String>,
    ) -> Result<Snapshot, LedgerError> {
        if !valid_id(&actor.actor_id) {
            return Err(LedgerError::Invalid("actor_id"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let raw: String = tx
            .query_row(
                "SELECT snapshot_json FROM commands WHERE command_id = ?1",
                [command_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(LedgerError::NotFound)?;
        let mut snapshot: Snapshot = serde_json::from_str(&raw)?;
        if !allowed(&snapshot.status, &status) || !valid_details(&status, &evidence, &reason) {
            return Err(LedgerError::InvalidTransition);
        }
        snapshot.confirmation_level = match status {
            Status::ProviderConfirmed => "provider".to_owned(),
            Status::DeviceConfirmed => "device".to_owned(),
            Status::Unknown if snapshot.confirmation_level == "provider" => "provider".to_owned(),
            _ => "none".to_owned(),
        };
        snapshot.status = status.clone();
        snapshot.status_changed_at = now_utc();
        snapshot.reason = reason.clone();
        snapshot.evidence = evidence.clone();
        let event = Event {
            command_id: command_id.to_owned(),
            status,
            at: snapshot.status_changed_at.clone(),
            actor,
            correlation_id: snapshot.request.correlation_id.clone(),
            idempotency_key: snapshot.request.idempotency_key.clone(),
            reason,
            evidence,
        };
        tx.execute(
            "UPDATE commands SET snapshot_json = ?1 WHERE command_id = ?2",
            params![serde_json::to_string(&snapshot)?, command_id],
        )?;
        tx.execute(
            "INSERT INTO command_events (command_id, event_json) VALUES (?1, ?2)",
            params![command_id, serde_json::to_string(&event)?],
        )?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub fn get(&self, command_id: &str) -> Result<Option<Snapshot>, LedgerError> {
        let raw: Option<String> = self
            .connection
            .query_row(
                "SELECT snapshot_json FROM commands WHERE command_id = ?1",
                [command_id],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    pub fn events(&self, command_id: &str) -> Result<Vec<Event>, LedgerError> {
        let mut statement = self.connection.prepare(
            "SELECT event_json FROM command_events WHERE command_id = ?1 ORDER BY sequence",
        )?;
        let rows = statement.query_map([command_id], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
}

fn allowed(current: &Status, next: &Status) -> bool {
    matches!(
        (current, next),
        (
            Status::Accepted,
            Status::Sent | Status::Unknown | Status::Failed
        ) | (
            Status::Sent,
            Status::ProviderConfirmed | Status::DeviceConfirmed | Status::Unknown | Status::Failed
        ) | (
            Status::ProviderConfirmed,
            Status::DeviceConfirmed | Status::Unknown | Status::Failed
        ) | (
            Status::Unknown,
            Status::ProviderConfirmed | Status::DeviceConfirmed | Status::Failed
        )
    )
}

fn valid_details(status: &Status, evidence: &Option<Evidence>, reason: &Option<String>) -> bool {
    let evidence_ok = match status {
        Status::ProviderConfirmed => {
            matches!(evidence, Some(Evidence { kind: EvidenceKind::ProviderAck, reference, at }) if valid_id(reference) && at.ends_with('Z'))
        }
        Status::DeviceConfirmed => {
            matches!(evidence, Some(Evidence { kind: EvidenceKind::DeviceObservation, reference, at }) if valid_id(reference) && at.ends_with('Z'))
        }
        _ => evidence.is_none(),
    };
    let reason_ok = match status {
        Status::Unknown | Status::Failed => reason
            .as_ref()
            .is_some_and(|r| !r.is_empty() && r.len() <= 500),
        _ => reason.is_none(),
    };
    evidence_ok && reason_ok
}

fn validate_request(request: &CommandRequest) -> Result<(), LedgerError> {
    for (name, value) in [
        ("household_id", &request.household_id),
        ("command_id", &request.command_id),
        ("device_id", &request.device_id),
        ("capability_id", &request.capability_id),
        ("actor_id", &request.actor.actor_id),
        ("idempotency_key", &request.idempotency_key),
        ("correlation_id", &request.correlation_id),
    ] {
        if !valid_id(value) {
            return Err(LedgerError::Invalid(name));
        }
    }
    if !matches!(
        request.value,
        Value::Bool(_) | Value::Number(_) | Value::String(_)
    ) {
        return Err(LedgerError::Invalid("value"));
    }
    Ok(())
}

fn valid_id(value: &str) -> bool {
    value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Actor {
        Actor {
            actor_type: ActorType::User,
            actor_id: "user-1".to_owned(),
        }
    }

    fn request() -> CommandRequest {
        CommandRequest {
            household_id: "home-1".to_owned(),
            command_id: "cmd-1".to_owned(),
            device_id: "light-1".to_owned(),
            capability_id: "on".to_owned(),
            value: Value::Bool(true),
            actor: actor(),
            idempotency_key: "idem-1".to_owned(),
            correlation_id: "corr-1".to_owned(),
        }
    }

    #[test]
    fn duplicate_intent_returns_original_command_without_new_event() {
        let mut ledger = Ledger::in_memory().unwrap();
        let original = ledger.accept(request()).unwrap();
        let mut retry = request();
        retry.command_id = "cmd-2".to_owned();
        retry.correlation_id = "corr-2".to_owned();
        assert_eq!(ledger.accept(retry).unwrap(), original);
        assert_eq!(ledger.events("cmd-1").unwrap().len(), 1);
        assert!(ledger.get("cmd-2").unwrap().is_none());
    }

    #[test]
    fn reused_key_with_different_intent_is_rejected() {
        let mut ledger = Ledger::in_memory().unwrap();
        ledger.accept(request()).unwrap();
        let mut conflict = request();
        conflict.value = Value::Bool(false);
        assert!(matches!(
            ledger.accept(conflict),
            Err(LedgerError::IdempotencyConflict)
        ));
    }

    #[test]
    fn sent_is_not_success_and_confirmation_requires_evidence() {
        let mut ledger = Ledger::in_memory().unwrap();
        ledger.accept(request()).unwrap();
        let sent = ledger
            .transition("cmd-1", Status::Sent, actor(), None, None)
            .unwrap();
        assert_eq!(sent.confirmation_level, "none");
        assert!(matches!(
            ledger.transition("cmd-1", Status::DeviceConfirmed, actor(), None, None),
            Err(LedgerError::InvalidTransition)
        ));
        let evidence = Evidence {
            kind: EvidenceKind::ProviderAck,
            reference: "ack-1".to_owned(),
            at: now_utc(),
        };
        let confirmed = ledger
            .transition(
                "cmd-1",
                Status::ProviderConfirmed,
                actor(),
                Some(evidence),
                None,
            )
            .unwrap();
        assert_eq!(confirmed.confirmation_level, "provider");
        assert_eq!(ledger.events("cmd-1").unwrap().len(), 3);
        assert!(ledger
            .events("cmd-1")
            .unwrap()
            .iter()
            .all(|event| event.at.ends_with('Z')
                && event.correlation_id == "corr-1"
                && event.idempotency_key == "idem-1"));
    }

    #[test]
    fn unknown_cannot_be_sent_again_but_can_be_reconciled() {
        let mut ledger = Ledger::in_memory().unwrap();
        ledger.accept(request()).unwrap();
        ledger
            .transition("cmd-1", Status::Sent, actor(), None, None)
            .unwrap();
        ledger
            .transition(
                "cmd-1",
                Status::Unknown,
                actor(),
                None,
                Some("timeout".to_owned()),
            )
            .unwrap();
        assert!(matches!(
            ledger.transition("cmd-1", Status::Sent, actor(), None, None),
            Err(LedgerError::InvalidTransition)
        ));
        let evidence = Evidence {
            kind: EvidenceKind::DeviceObservation,
            reference: "obs-1".to_owned(),
            at: now_utc(),
        };
        let confirmed = ledger
            .transition(
                "cmd-1",
                Status::DeviceConfirmed,
                actor(),
                Some(evidence),
                None,
            )
            .unwrap();
        assert_eq!(confirmed.confirmation_level, "device");
    }

    #[test]
    fn ledger_survives_reopen() {
        let path =
            std::env::temp_dir().join(format!("genesis-ledger-{}.sqlite", std::process::id()));
        {
            let mut ledger = Ledger::open(&path).unwrap();
            ledger.accept(request()).unwrap();
        }
        let ledger = Ledger::open(&path).unwrap();
        assert_eq!(
            ledger.get("cmd-1").unwrap().unwrap().status,
            Status::Accepted
        );
        assert_eq!(ledger.events("cmd-1").unwrap().len(), 1);
        std::fs::remove_file(path).unwrap();
    }
}
