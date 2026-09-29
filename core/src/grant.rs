//! Časovo obmedzený prístup: rozhodnutie Behavior enginu sa vykoná ako unlock
//! a pri expirácii sa musí vrátiť späť relockom.
//!
//! Modul sám nekomunikuje s Home Assistantom. Pripraví povel, ktorý zapíše do
//! execution ledgeru, a čaká, kým mu volajúci prinesie výsledný snapshot. Vďaka
//! tomu sa celý cyklus initial lock → dôkaz → unlock → expirácia → relock dá
//! odohrať v teste bez zariadenia.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    behavior_decision::{BehaviorDecision, Operation, RequiredConfirmation},
    ledger::{Actor, ActorType, CommandRequest, Ledger, LedgerError, Snapshot, Status},
};

/// Odvodený relock kľúč pridáva k pôvodnému sedem znakov a ledger neprijme
/// identifikátor dlhší než 128.
const MAX_DECISION_KEY: usize = 128 - RELOCK_SUFFIX.len();
const RELOCK_SUFFIX: &str = ":relock";

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS access_grants (
       decision_id TEXT PRIMARY KEY,
       household_id TEXT NOT NULL,
       device_id TEXT NOT NULL,
       capability_id TEXT NOT NULL,
       granted_value INTEGER NOT NULL,
       required_confirmation TEXT NOT NULL,
       expires_at TEXT NOT NULL,
       state TEXT NOT NULL,
       unlock_confirmed INTEGER NOT NULL,
       unlock_command_id TEXT NOT NULL,
       relock_command_id TEXT,
       updated_at TEXT NOT NULL
     );
     CREATE INDEX IF NOT EXISTS access_grants_due ON access_grants(state, expires_at);
     CREATE TABLE IF NOT EXISTS incidents (
       incident_id TEXT PRIMARY KEY,
       decision_id TEXT NOT NULL REFERENCES access_grants(decision_id),
       kind TEXT NOT NULL,
       detail TEXT NOT NULL,
       at TEXT NOT NULL
     );";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantState {
    /// Unlock je prijatý v ledgeri, výsledok ešte nie je známy.
    Granted,
    /// Prístup môže byť otvorený, takže pri expirácii treba relock.
    Active,
    /// Unlock sa nevykonal, nie je čo vracať.
    UnlockFailed,
    /// Relock potvrdený v rozsahu, ktorý rozhodnutie vyžadovalo.
    Relocked,
    /// Relock skončil neisto. Vznikol incident a stav čaká na zosúladenie.
    RelockPending,
}

impl GrantState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Active => "active",
            Self::UnlockFailed => "unlock_failed",
            Self::Relocked => "relocked",
            Self::RelockPending => "relock_pending",
        }
    }

    fn parse(raw: &str) -> Result<Self, GrantError> {
        match raw {
            "granted" => Ok(Self::Granted),
            "active" => Ok(Self::Active),
            "unlock_failed" => Ok(Self::UnlockFailed),
            "relocked" => Ok(Self::Relocked),
            "relock_pending" => Ok(Self::RelockPending),
            _ => Err(GrantError::Corrupt("unknown grant state")),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Grant {
    pub decision_id: String,
    pub household_id: String,
    pub device_id: String,
    pub capability_id: String,
    pub granted_value: bool,
    pub expires_at: String,
    pub state: GrantState,
    /// Nepravda znamená, že unlock nedosiahol vyžadovanú úroveň potvrdenia.
    pub unlock_confirmed: bool,
    pub unlock_command_id: String,
    pub relock_command_id: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Incident {
    pub incident_id: String,
    pub decision_id: String,
    pub kind: String,
    pub detail: String,
    pub at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    #[error("ledger error: {0}")]
    Ledger(#[from] LedgerError),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid decision: {0}")]
    Invalid(String),
    #[error("grant not found")]
    NotFound,
    #[error("stored grant is unreadable: {0}")]
    Corrupt(&'static str),
}

/// Zaeviduje rozhodnutie a prijme unlock povel do ledgeru.
///
/// Opakovanie toho istého rozhodnutia vráti pôvodný grant. Ledger drží unikátny
/// `(household_id, idempotency_key)`, takže druhý povel nevznikne ani vtedy, keď
/// sa rozhodnutie doručí viackrát.
pub fn open(
    ledger: &mut Ledger,
    decision: &BehaviorDecision,
    now: DateTime<Utc>,
) -> Result<Grant, GrantError> {
    decision.validate().map_err(GrantError::Invalid)?;
    if !matches!(decision.operation, Operation::Apply) {
        return Err(GrantError::Invalid(
            "only an apply decision opens a timed grant".into(),
        ));
    }
    if decision.idempotency_key.len() > MAX_DECISION_KEY {
        return Err(GrantError::Invalid(format!(
            "idempotency key must be at most {MAX_DECISION_KEY} characters so the relock key fits"
        )));
    }
    if !decision.active_at(now) {
        return Err(GrantError::Invalid(
            "decision window is not open at this time".into(),
        ));
    }

    let decision_id = decision.decision_id.to_string();
    if let Some(existing) = get(ledger, &decision_id)? {
        return Ok(existing);
    }

    let snapshot = ledger.accept(CommandRequest {
        household_id: decision.household_id.clone(),
        command_id: format!("unlock-{decision_id}"),
        device_id: decision.device_id.clone(),
        capability_id: decision.capability_id.clone(),
        value: decision.requested_value.into(),
        actor: issuer(decision),
        idempotency_key: decision.idempotency_key.clone(),
        correlation_id: format!("decision-{decision_id}"),
    })?;

    let grant = Grant {
        decision_id,
        household_id: decision.household_id.clone(),
        device_id: decision.device_id.clone(),
        capability_id: decision.capability_id.clone(),
        granted_value: decision.requested_value,
        expires_at: canonical_utc(&decision.expires_at)?,
        state: GrantState::Granted,
        unlock_confirmed: false,
        unlock_command_id: snapshot.request.command_id,
        relock_command_id: None,
        updated_at: now_utc(),
    };
    insert(ledger, &grant, &decision.required_confirmation)?;
    Ok(grant)
}

/// Zapíše výsledok unlocku.
///
/// Neistý unlock (`unknown`, alebo len provider ack tam, kde rozhodnutie žiada
/// potvrdenie zariadením) sa zámerne považuje za otvorený prístup. Bezpečnejšie
/// je relock navyše než nechať zariadenie odomknuté.
pub fn settle_unlock(
    ledger: &mut Ledger,
    decision_id: &str,
    snapshot: &Snapshot,
) -> Result<Grant, GrantError> {
    let (grant, required) = load(ledger, decision_id)?;
    if grant.state != GrantState::Granted {
        return Ok(grant);
    }
    let confirmed = confirmed_enough(&snapshot.status, &required);
    let state = if snapshot.status == Status::Failed {
        GrantState::UnlockFailed
    } else {
        GrantState::Active
    };
    update_state(ledger, decision_id, state, Some(confirmed), None)?;
    get(ledger, decision_id)?.ok_or(GrantError::NotFound)
}

/// Granty, ktorým vypršalo okno a ešte neboli vrátené späť.
pub fn due(ledger: &Ledger, now: DateTime<Utc>) -> Result<Vec<Grant>, GrantError> {
    let cutoff = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut statement = ledger.connection().prepare(
        "SELECT decision_id, household_id, device_id, capability_id, granted_value,
                expires_at, state, unlock_confirmed, unlock_command_id, relock_command_id,
                updated_at
         FROM access_grants WHERE state = ?1 AND expires_at <= ?2 ORDER BY expires_at",
    )?;
    let rows = statement.query_map(params![GrantState::Active.as_str(), cutoff], read_grant)?;
    rows.map(|row| row?).collect()
}

/// Prijme relock povel do ledgeru. Opakované volanie vráti ten istý povel.
pub fn begin_relock(ledger: &mut Ledger, decision_id: &str) -> Result<Snapshot, GrantError> {
    let (grant, _) = load(ledger, decision_id)?;
    if grant.state != GrantState::Active {
        return Err(GrantError::Invalid(
            "only an active grant can be relocked".into(),
        ));
    }
    let key = relock_key(ledger, decision_id)?;
    let snapshot = ledger.accept(CommandRequest {
        household_id: grant.household_id.clone(),
        command_id: format!("relock-{decision_id}"),
        device_id: grant.device_id.clone(),
        capability_id: grant.capability_id.clone(),
        value: (!grant.granted_value).into(),
        actor: Actor {
            actor_type: ActorType::Automation,
            actor_id: "behavior-engine".to_owned(),
        },
        idempotency_key: key,
        correlation_id: format!("decision-{decision_id}"),
    })?;
    update_state(
        ledger,
        decision_id,
        GrantState::Active,
        None,
        Some(snapshot.request.command_id.as_str()),
    )?;
    Ok(snapshot)
}

/// Zapíše výsledok relocku. Neistý výsledok založí incident.
///
/// `relock_pending` je koncový stav tejto úlohy: zosúladenie fyzického stavu po
/// výpadku rieši ELYSIUM-344, nie automatický opakovaný pokus.
pub fn settle_relock(
    ledger: &mut Ledger,
    decision_id: &str,
    snapshot: &Snapshot,
) -> Result<Grant, GrantError> {
    let (grant, required) = load(ledger, decision_id)?;
    if grant.state != GrantState::Active {
        return Ok(grant);
    }
    if confirmed_enough(&snapshot.status, &required) {
        update_state(ledger, decision_id, GrantState::Relocked, None, None)?;
    } else {
        update_state(ledger, decision_id, GrantState::RelockPending, None, None)?;
        raise_incident(
            ledger,
            decision_id,
            "relock_uncertain",
            &format!(
                "relock ended as {} without the confirmation the decision required",
                serde_json::to_string(&snapshot.status)
                    .unwrap_or_default()
                    .trim_matches('"')
            ),
        )?;
    }
    get(ledger, decision_id)?.ok_or(GrantError::NotFound)
}

pub fn get(ledger: &Ledger, decision_id: &str) -> Result<Option<Grant>, GrantError> {
    ledger
        .connection()
        .query_row(
            "SELECT decision_id, household_id, device_id, capability_id, granted_value,
                    expires_at, state, unlock_confirmed, unlock_command_id, relock_command_id,
                    updated_at
             FROM access_grants WHERE decision_id = ?1",
            [decision_id],
            read_grant,
        )
        .optional()?
        .transpose()
}

pub fn incidents(ledger: &Ledger, decision_id: &str) -> Result<Vec<Incident>, GrantError> {
    let mut statement = ledger.connection().prepare(
        "SELECT incident_id, decision_id, kind, detail, at
         FROM incidents WHERE decision_id = ?1 ORDER BY at, incident_id",
    )?;
    let rows = statement.query_map([decision_id], |row| {
        Ok(Incident {
            incident_id: row.get(0)?,
            decision_id: row.get(1)?,
            kind: row.get(2)?,
            detail: row.get(3)?,
            at: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn confirmed_enough(status: &Status, required: &RequiredConfirmation) -> bool {
    match required {
        RequiredConfirmation::Provider => {
            matches!(status, Status::ProviderConfirmed | Status::DeviceConfirmed)
        }
        RequiredConfirmation::Device => matches!(status, Status::DeviceConfirmed),
    }
}

fn issuer(decision: &BehaviorDecision) -> Actor {
    Actor {
        actor_type: ActorType::Automation,
        actor_id: decision.issuer.clone(),
    }
}

fn relock_key(ledger: &Ledger, decision_id: &str) -> Result<String, GrantError> {
    let key: String = ledger.connection().query_row(
        "SELECT c.idempotency_key FROM commands c
         JOIN access_grants g ON g.unlock_command_id = c.command_id
         WHERE g.decision_id = ?1",
        [decision_id],
        |row| row.get(0),
    )?;
    Ok(format!("{key}{RELOCK_SUFFIX}"))
}

fn insert(
    ledger: &mut Ledger,
    grant: &Grant,
    required: &RequiredConfirmation,
) -> Result<(), GrantError> {
    ledger.connection().execute(
        "INSERT INTO access_grants
         (decision_id, household_id, device_id, capability_id, granted_value,
          required_confirmation, expires_at, state, unlock_confirmed, unlock_command_id,
          relock_command_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11)",
        params![
            grant.decision_id,
            grant.household_id,
            grant.device_id,
            grant.capability_id,
            grant.granted_value,
            confirmation_str(required),
            grant.expires_at,
            grant.state.as_str(),
            grant.unlock_confirmed,
            grant.unlock_command_id,
            grant.updated_at
        ],
    )?;
    Ok(())
}

fn update_state(
    ledger: &mut Ledger,
    decision_id: &str,
    state: GrantState,
    unlock_confirmed: Option<bool>,
    relock_command_id: Option<&str>,
) -> Result<(), GrantError> {
    let changed = ledger.connection().execute(
        "UPDATE access_grants
         SET state = ?2,
             unlock_confirmed = COALESCE(?3, unlock_confirmed),
             relock_command_id = COALESCE(?4, relock_command_id),
             updated_at = ?5
         WHERE decision_id = ?1",
        params![
            decision_id,
            state.as_str(),
            unlock_confirmed,
            relock_command_id,
            now_utc()
        ],
    )?;
    if changed == 0 {
        return Err(GrantError::NotFound);
    }
    Ok(())
}

/// Incident id je odvodené od rozhodnutia, takže opakovaný zápis nevytvorí
/// druhý záznam o tej istej udalosti.
fn raise_incident(
    ledger: &mut Ledger,
    decision_id: &str,
    kind: &str,
    detail: &str,
) -> Result<(), GrantError> {
    ledger.connection().execute(
        "INSERT OR IGNORE INTO incidents (incident_id, decision_id, kind, detail, at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            format!("{kind}-{decision_id}"),
            decision_id,
            kind,
            detail,
            now_utc()
        ],
    )?;
    Ok(())
}

fn load(ledger: &Ledger, decision_id: &str) -> Result<(Grant, RequiredConfirmation), GrantError> {
    let grant = get(ledger, decision_id)?.ok_or(GrantError::NotFound)?;
    let raw: String = ledger.connection().query_row(
        "SELECT required_confirmation FROM access_grants WHERE decision_id = ?1",
        [decision_id],
        |row| row.get(0),
    )?;
    let required = match raw.as_str() {
        "provider" => RequiredConfirmation::Provider,
        "device" => RequiredConfirmation::Device,
        _ => return Err(GrantError::Corrupt("unknown required confirmation")),
    };
    Ok((grant, required))
}

fn confirmation_str(required: &RequiredConfirmation) -> &'static str {
    match required {
        RequiredConfirmation::Provider => "provider",
        RequiredConfirmation::Device => "device",
    }
}

fn read_grant(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Grant, GrantError>> {
    let decision_id: String = row.get(0)?;
    let household_id: String = row.get(1)?;
    let device_id: String = row.get(2)?;
    let capability_id: String = row.get(3)?;
    let granted_value: bool = row.get(4)?;
    let expires_at: String = row.get(5)?;
    let raw_state: String = row.get(6)?;
    let unlock_confirmed: bool = row.get(7)?;
    let unlock_command_id: String = row.get(8)?;
    let relock_command_id: Option<String> = row.get(9)?;
    let updated_at: String = row.get(10)?;
    Ok(GrantState::parse(&raw_state).map(|state| Grant {
        decision_id,
        household_id,
        device_id,
        capability_id,
        granted_value,
        expires_at,
        state,
        unlock_confirmed,
        unlock_command_id,
        relock_command_id,
        updated_at,
    }))
}

fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// `due` porovnáva časy v SQLite ako reťazce, takže musia mať rovnaký tvar.
/// Rozhodnutie smie prísť so sekundovou presnosťou (`…19:00:00Z`) a `'Z'` je
/// lexikograficky väčšie než `'.'` — bez normalizácie by sa taký grant v
/// okamihu expirácie ešte nepovažoval za splatný.
fn canonical_utc(raw: &str) -> Result<String, GrantError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| {
            value
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        })
        .map_err(|_| GrantError::Invalid("invalid decision timestamp".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Evidence, EvidenceKind};
    use serde_json::json;

    const VALID_FROM: &str = "2026-09-29T18:00:00Z";
    const EXPIRES_AT: &str = "2026-09-29T19:00:00Z";

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn decision(required: &str) -> BehaviorDecision {
        BehaviorDecision::parse(
            &json!({
                "schema_version": "1.0",
                "decision_id": "ff77bdb0-70af-4f2a-a913-76609b66761b",
                "issuer": "behavior-engine",
                "household_id": "pilot-home",
                "central_unit_id": "ad19a578-21e2-453f-a57c-1913350be34e",
                "subject_id": "64582f6b-38a5-48dd-9ed4-ae02949c7740",
                "device_id": "ha:light.living",
                "capability_id": "power",
                "requested_value": true,
                "operation": "apply",
                "valid_from": VALID_FROM,
                "expires_at": EXPIRES_AT,
                "reason_code": "goal_verified",
                "idempotency_key": "behavior:ff77bdb0",
                "required_confirmation": required
            })
            .to_string(),
        )
        .unwrap()
    }

    fn automation() -> Actor {
        Actor {
            actor_type: ActorType::Automation,
            actor_id: "behavior-engine".to_owned(),
        }
    }

    /// Dovedie povel do cieľového stavu tak, ako by to spravil HA adaptér.
    fn drive(ledger: &mut Ledger, command_id: &str, target: Status) -> Snapshot {
        if target == Status::Failed {
            return ledger
                .transition(
                    command_id,
                    Status::Failed,
                    automation(),
                    None,
                    Some("ha_unavailable".to_owned()),
                )
                .unwrap();
        }
        ledger
            .transition(command_id, Status::Sent, automation(), None, None)
            .unwrap();
        let (evidence, reason) = match target {
            Status::DeviceConfirmed => (
                Some(Evidence {
                    kind: EvidenceKind::DeviceObservation,
                    reference: "obs-1".to_owned(),
                    at: now_utc(),
                }),
                None,
            ),
            Status::ProviderConfirmed => (
                Some(Evidence {
                    kind: EvidenceKind::ProviderAck,
                    reference: "ack-1".to_owned(),
                    at: now_utc(),
                }),
                None,
            ),
            Status::Unknown => (None, Some("result_unverified".to_owned())),
            ref other => panic!("unsupported target {other:?}"),
        };
        ledger
            .transition(command_id, target, automation(), evidence, reason)
            .unwrap()
    }

    fn ledger() -> Ledger {
        Ledger::open(":memory:").unwrap()
    }

    #[test]
    fn full_cycle_unlocks_then_relocks_after_expiry() {
        let mut ledger = ledger();
        let decision = decision("device");
        let grant = open(&mut ledger, &decision, at("2026-09-29T18:30:00Z")).unwrap();
        assert_eq!(grant.state, GrantState::Granted);
        assert!(grant.granted_value);

        let unlocked = drive(
            &mut ledger,
            &grant.unlock_command_id,
            Status::DeviceConfirmed,
        );
        let grant = settle_unlock(&mut ledger, &grant.decision_id, &unlocked).unwrap();
        assert_eq!(grant.state, GrantState::Active);
        assert!(grant.unlock_confirmed);

        // Počas okna sa nič nevracia.
        assert!(due(&ledger, at("2026-09-29T18:45:00Z")).unwrap().is_empty());

        let outstanding = due(&ledger, at("2026-09-29T19:30:00Z")).unwrap();
        assert_eq!(outstanding.len(), 1);

        let relock = begin_relock(&mut ledger, &grant.decision_id).unwrap();
        assert_eq!(relock.request.value, json!(false));
        assert_eq!(relock.request.idempotency_key, "behavior:ff77bdb0:relock");

        let relocked = drive(
            &mut ledger,
            &relock.request.command_id,
            Status::DeviceConfirmed,
        );
        let grant = settle_relock(&mut ledger, &grant.decision_id, &relocked).unwrap();
        assert_eq!(grant.state, GrantState::Relocked);
        assert!(incidents(&ledger, &grant.decision_id).unwrap().is_empty());
        assert!(due(&ledger, at("2026-09-29T20:00:00Z")).unwrap().is_empty());
    }

    #[test]
    fn repeating_the_decision_and_the_outcomes_changes_nothing() {
        let mut ledger = ledger();
        let decision = decision("device");
        let now = at("2026-09-29T18:30:00Z");
        let first = open(&mut ledger, &decision, now).unwrap();
        let second = open(&mut ledger, &decision, now).unwrap();
        assert_eq!(first, second);
        assert_eq!(ledger.events(&first.unlock_command_id).unwrap().len(), 1);

        let unlocked = drive(
            &mut ledger,
            &first.unlock_command_id,
            Status::DeviceConfirmed,
        );
        let settled = settle_unlock(&mut ledger, &first.decision_id, &unlocked).unwrap();
        assert_eq!(
            settle_unlock(&mut ledger, &first.decision_id, &unlocked).unwrap(),
            settled
        );

        let relock = begin_relock(&mut ledger, &first.decision_id).unwrap();
        let again = begin_relock(&mut ledger, &first.decision_id).unwrap();
        assert_eq!(relock.request.command_id, again.request.command_id);
        assert_eq!(ledger.events(&relock.request.command_id).unwrap().len(), 1);

        let relocked = drive(
            &mut ledger,
            &relock.request.command_id,
            Status::DeviceConfirmed,
        );
        let done = settle_relock(&mut ledger, &first.decision_id, &relocked).unwrap();
        assert_eq!(done.state, GrantState::Relocked);
        assert_eq!(
            settle_relock(&mut ledger, &first.decision_id, &relocked).unwrap(),
            done
        );
    }

    #[test]
    fn uncertain_relock_becomes_relock_pending_with_one_incident() {
        let mut ledger = ledger();
        let decision = decision("device");
        let grant = open(&mut ledger, &decision, at("2026-09-29T18:30:00Z")).unwrap();
        let unlocked = drive(
            &mut ledger,
            &grant.unlock_command_id,
            Status::DeviceConfirmed,
        );
        settle_unlock(&mut ledger, &grant.decision_id, &unlocked).unwrap();

        let relock = begin_relock(&mut ledger, &grant.decision_id).unwrap();
        let unsure = drive(&mut ledger, &relock.request.command_id, Status::Unknown);
        let grant = settle_relock(&mut ledger, &grant.decision_id, &unsure).unwrap();
        assert_eq!(grant.state, GrantState::RelockPending);

        let raised = incidents(&ledger, &grant.decision_id).unwrap();
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].kind, "relock_uncertain");

        // Opakované doručenie toho istého výsledku nezaloží druhý incident.
        settle_relock(&mut ledger, &grant.decision_id, &unsure).unwrap();
        assert_eq!(incidents(&ledger, &grant.decision_id).unwrap().len(), 1);
    }

    #[test]
    fn provider_ack_alone_does_not_settle_a_relock_that_needs_the_device() {
        let mut ledger = ledger();
        let decision = decision("device");
        let grant = open(&mut ledger, &decision, at("2026-09-29T18:30:00Z")).unwrap();
        let unlocked = drive(
            &mut ledger,
            &grant.unlock_command_id,
            Status::DeviceConfirmed,
        );
        settle_unlock(&mut ledger, &grant.decision_id, &unlocked).unwrap();
        let relock = begin_relock(&mut ledger, &grant.decision_id).unwrap();
        let ack = drive(
            &mut ledger,
            &relock.request.command_id,
            Status::ProviderConfirmed,
        );
        let grant = settle_relock(&mut ledger, &grant.decision_id, &ack).unwrap();
        assert_eq!(grant.state, GrantState::RelockPending);
        assert_eq!(incidents(&ledger, &grant.decision_id).unwrap().len(), 1);
    }

    #[test]
    fn an_uncertain_unlock_is_still_relocked_but_a_failed_one_is_not() {
        let mut unsure = ledger();
        let grant = open(&mut unsure, &decision("device"), at("2026-09-29T18:30:00Z")).unwrap();
        let outcome = drive(&mut unsure, &grant.unlock_command_id, Status::Unknown);
        let grant = settle_unlock(&mut unsure, &grant.decision_id, &outcome).unwrap();
        assert_eq!(grant.state, GrantState::Active);
        assert!(!grant.unlock_confirmed);
        assert_eq!(due(&unsure, at("2026-09-29T19:30:00Z")).unwrap().len(), 1);

        let mut failed = ledger();
        let grant = open(&mut failed, &decision("device"), at("2026-09-29T18:30:00Z")).unwrap();
        let outcome = drive(&mut failed, &grant.unlock_command_id, Status::Failed);
        let grant = settle_unlock(&mut failed, &grant.decision_id, &outcome).unwrap();
        assert_eq!(grant.state, GrantState::UnlockFailed);
        assert!(due(&failed, at("2026-09-29T19:30:00Z")).unwrap().is_empty());
    }

    #[test]
    fn an_expiry_given_in_whole_seconds_is_due_at_that_instant() {
        let mut ledger = ledger();
        let grant = open(&mut ledger, &decision("device"), at("2026-09-29T18:30:00Z")).unwrap();
        let unlocked = drive(
            &mut ledger,
            &grant.unlock_command_id,
            Status::DeviceConfirmed,
        );
        settle_unlock(&mut ledger, &grant.decision_id, &unlocked).unwrap();
        assert!(due(&ledger, at("2026-09-29T18:59:59Z")).unwrap().is_empty());
        assert_eq!(due(&ledger, at(EXPIRES_AT)).unwrap().len(), 1);
    }

    #[test]
    fn a_revert_decision_or_a_closed_window_opens_no_grant() {
        let mut ledger = ledger();
        let mut revert = decision("device");
        revert.operation = Operation::Revert;
        assert!(open(&mut ledger, &revert, at("2026-09-29T18:30:00Z")).is_err());
        assert!(open(&mut ledger, &decision("device"), at("2026-09-29T19:30:00Z")).is_err());
        assert!(get(&ledger, &decision("device").decision_id.to_string())
            .unwrap()
            .is_none());
    }
}
