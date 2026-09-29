//! Časovo obmedzený prístup a zosúladenie fyzického stavu.
//!
//! Rozhodnutie Behavior enginu sa vykoná ako unlock, pri expirácii sa musí
//! vrátiť späť relockom a po reštarte, strate Home Assistanta, zmazaní cieľa
//! alebo override sa musí fyzický stav zosúladiť s tým, čo Genesis naposledy
//! potvrdil.
//!
//! Modul sám nekomunikuje s Home Assistantom. Pripraví povel, ktorý zapíše do
//! execution ledgeru, a čaká, kým mu volajúci prinesie výsledný snapshot alebo
//! pozorovanie z inventára. Vďaka tomu sa celý cyklus initial lock → dôkaz →
//! unlock → expirácia → relock → zosúladenie dá odohrať v teste bez zariadenia.
//!
//! Čas je vždy parameter `now`, nikdy systémové hodiny; zosúladenie sa inak
//! nedá odohrať deterministicky.
//!
//! Bezpečnostné výnimky sú vymenované v `core/README.md`. V kóde ich držia
//! `closing_value` (zosúladenie posiela iba opak otvorenej hodnoty),
//! `may_attempt` (pokusy sú ohraničené) a `confirmed_enough` (provider ack
//! nenahradí potvrdenie zariadením).

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    behavior_decision::{BehaviorDecision, Operation, RequiredConfirmation},
    ledger::{
        Actor, ActorType, CommandRequest, Evidence, EvidenceKind, Ledger, LedgerError, Snapshot,
        Status,
    },
};

/// Odvodený relock kľúč pridáva k pôvodnému sedem znakov a opakovaný pokus
/// ešte jednu číslicu; ledger neprijme identifikátor dlhší než 128.
const MAX_DECISION_KEY: usize = 128 - RELOCK_SUFFIX.len() - 1;
const RELOCK_SUFFIX: &str = ":relock";

/// Zosúladenie nesmie skúšať do nekonečna. Po vyčerpaní pokusov zostáva
/// incident otvorený pre človeka.
pub const MAX_CLOSE_ATTEMPTS: i64 = 5;
const _: () = assert!(
    MAX_CLOSE_ATTEMPTS < 10,
    "číslo pokusu musí zostať jednociferné, inak sa idempotency kľúč nezmestí"
);

/// Prvý opakovaný pokus čaká minútu, každý ďalší dvojnásobok predchádzajúceho.
const RETRY_BACKOFF_SECONDS: i64 = 60;

/// Relock pri expirácii aj zosúladenie píše samotný Genesis, nie Behavior
/// engine; audit má ukázať, kto povel skutočne vydal.
const RECONCILER: &str = "genesis-core";

/// Prehľad je pilotný a jednodomácnostný, ale odpoveď API nesmie rásť bez
/// hranice.
const OVERVIEW_LIMIT: i64 = 200;

const UNLOCK: &str = "unlock";
const RELOCK: &str = "relock";
const WITHDRAW: &str = "withdraw";

const GRANT_COLUMNS: &str = "decision_id, household_id, device_id, capability_id, granted_value,
     expires_at, state, unlock_confirmed, unlock_command_id, relock_command_id, updated_at";

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
     CREATE INDEX IF NOT EXISTS access_grants_by_device
       ON access_grants(household_id, device_id, capability_id, state);
     CREATE TABLE IF NOT EXISTS grant_commands (
       command_id TEXT NOT NULL,
       decision_id TEXT NOT NULL REFERENCES access_grants(decision_id),
       kind TEXT NOT NULL,
       attempt INTEGER NOT NULL,
       value INTEGER NOT NULL,
       PRIMARY KEY (command_id, decision_id)
     );
     CREATE INDEX IF NOT EXISTS grant_commands_by_grant
       ON grant_commands(decision_id, attempt);
     CREATE TABLE IF NOT EXISTS incidents (
       incident_id TEXT PRIMARY KEY,
       decision_id TEXT NOT NULL REFERENCES access_grants(decision_id),
       kind TEXT NOT NULL,
       detail TEXT NOT NULL,
       at TEXT NOT NULL,
       resolved_at TEXT
     );";

/// Databáza z ELYSIUM-343 nepozná uzavretie incidentu ani tabuľku povelov
/// grantu. Ledger sa otvára pri každom štarte, takže doplnenie musí zniesť
/// opakovanie aj prázdnu databázu.
///
/// Index nad `resolved_at` patrí sem, nie do `SCHEMA`: nad staršou tabuľkou by
/// vznikal skôr, než stĺpec existuje, a otvorenie ledgeru by zlyhalo.
pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    let resolution_exists = connection
        .prepare("SELECT 1 FROM pragma_table_info('incidents') WHERE name = 'resolved_at'")?
        .exists([])?;
    if !resolution_exists {
        connection.execute_batch("ALTER TABLE incidents ADD COLUMN resolved_at TEXT")?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS incidents_open ON incidents(decision_id, resolved_at)",
    )?;
    connection.execute_batch(
        "INSERT OR IGNORE INTO grant_commands (command_id, decision_id, kind, attempt, value)
           SELECT unlock_command_id, decision_id, 'unlock', 1, granted_value FROM access_grants;
         INSERT OR IGNORE INTO grant_commands (command_id, decision_id, kind, attempt, value)
           SELECT relock_command_id, decision_id, 'relock', 1, NOT granted_value
           FROM access_grants WHERE relock_command_id IS NOT NULL",
    )
}

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
    /// Zariadenie na tej istej hodnote drží novšie rozhodnutie, takže tento
    /// grant ho nezamyká.
    Superseded,
}

impl GrantState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Active => "active",
            Self::UnlockFailed => "unlock_failed",
            Self::Relocked => "relocked",
            Self::RelockPending => "relock_pending",
            Self::Superseded => "superseded",
        }
    }

    fn parse(raw: &str) -> Result<Self, GrantError> {
        match raw {
            "granted" => Ok(Self::Granted),
            "active" => Ok(Self::Active),
            "unlock_failed" => Ok(Self::UnlockFailed),
            "relocked" => Ok(Self::Relocked),
            "relock_pending" => Ok(Self::RelockPending),
            "superseded" => Ok(Self::Superseded),
            _ => Err(GrantError::Corrupt("unknown grant state")),
        }
    }

    /// Stavy, v ktorých môže byť zariadenie stále otvorené.
    fn open() -> [Self; 3] {
        [Self::Granted, Self::Active, Self::RelockPending]
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

impl Grant {
    /// Hodnota, ktorou sa prístup zatvára. Zosúladenie nikdy neposiela nič iné.
    pub fn closing_value(&self) -> bool {
        !self.granted_value
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Incident {
    pub incident_id: String,
    pub decision_id: String,
    pub kind: String,
    pub detail: String,
    pub at: String,
    /// Otvorený incident nemá čas uzavretia.
    pub resolved_at: Option<String>,
}

/// Posledný stav zariadenia, ktorý Genesis skutočne potvrdil dôkazom.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConfirmedState {
    pub command_id: String,
    pub value: Value,
    pub confirmation: &'static str,
    /// Kedy ledger potvrdenie zapísal.
    pub at: String,
    /// Čas dôkazu od providera alebo z pozorovania, ak ho dôkaz nesie.
    pub observed_at: Option<String>,
}

/// To, čo Genesis vie o jednom grante ukázať prevádzkovateľovi.
#[derive(Clone, Debug, Serialize)]
pub struct AccessState {
    pub grant: Grant,
    pub required_confirmation: &'static str,
    pub close_attempts: i64,
    pub last_confirmed: Option<ConfirmedState>,
    pub open_incidents: Vec<Incident>,
}

/// Ďalší krok uzavretia grantu.
#[derive(Clone, Debug)]
pub enum RelockStep {
    /// Povel je prijatý v ledgeri a čaká na vykonanie.
    Send(Snapshot),
    /// Grant sa uzavrel bez fyzického povelu.
    Settled(Grant),
    /// Ďalší pokus by bol priskoro alebo už vyčerpal limit.
    Wait(Grant),
}

/// Jeden povel, ktorý uzatvára prístup na základe rozhodnutia `revert`.
#[derive(Clone, Debug)]
pub struct Withdrawal {
    pub command_id: String,
    pub device_id: String,
    /// Hodnota, na ktorú sa zariadenie vracia.
    pub value: bool,
    /// Granty, ktoré tento jediný povel uzatvára.
    pub closing: Vec<String>,
    pub accepted: Snapshot,
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
    // Dva grantom držané protikladné hodnoty na jednom zariadení nemajú
    // definovaný fyzický výsledok, takže sa taká dvojica vôbec nezaloží.
    if let Some(other) = opposing_grant(ledger, decision, now)? {
        return Err(GrantError::Invalid(format!(
            "grant {other} holds this device at the opposite value"
        )));
    }

    let command_id = format!("unlock-{decision_id}");
    let snapshot = ledger.accept(CommandRequest {
        household_id: decision.household_id.clone(),
        command_id: command_id.clone(),
        device_id: decision.device_id.clone(),
        capability_id: decision.capability_id.clone(),
        value: decision.requested_value.into(),
        actor: issuer(decision),
        idempotency_key: decision.idempotency_key.clone(),
        correlation_id: format!("decision-{decision_id}"),
    })?;
    if snapshot.request.command_id != command_id {
        return Err(GrantError::Invalid(
            "idempotency key already belongs to another decision".into(),
        ));
    }

    let grant = Grant {
        decision_id: decision_id.clone(),
        household_id: decision.household_id.clone(),
        device_id: decision.device_id.clone(),
        capability_id: decision.capability_id.clone(),
        granted_value: decision.requested_value,
        expires_at: canonical_utc(&decision.expires_at)?,
        state: GrantState::Granted,
        unlock_confirmed: false,
        unlock_command_id: command_id.clone(),
        relock_command_id: None,
        updated_at: stamp(now),
    };
    insert(ledger, &grant, &decision.required_confirmation)?;
    record_command(
        ledger,
        &decision_id,
        &command_id,
        UNLOCK,
        1,
        grant.granted_value,
    )?;
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
    now: DateTime<Utc>,
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
    update_state(ledger, decision_id, state, Some(confirmed), None, now)?;
    reload(ledger, decision_id)
}

/// Prevezme grant, ktorý zostal bez výsledku.
///
/// Volá sa pri štarte procesu aj na začiatku každého prechodu zosúladenia.
/// Oboje drží zámok ledgeru a povel sa odosiela iba pod tým istým zámkom, takže
/// v oboch prípadoch platí to isté: čo zostalo v `accepted` alebo `sent`, už
/// výsledok nedostane. Genesis to prizná ako `unknown` a grant radšej považuje
/// za otvorený, než by ho nechal bez zámku — inak by pád alebo chyba ledgeru
/// medzi prijatím unlocku a jeho výsledkom nechali zariadenie odomknuté.
pub fn resume(ledger: &mut Ledger, now: DateTime<Utc>) -> Result<Vec<Grant>, GrantError> {
    let mut resumed = Vec::new();
    for decision_id in open_decision_ids(ledger)? {
        let mut interrupted = Vec::new();
        for (command_id, _, _) in commands_of(ledger, &decision_id)? {
            let Some(snapshot) = ledger.get(&command_id)? else {
                continue;
            };
            if matches!(snapshot.status, Status::Accepted | Status::Sent) {
                ledger.transition(
                    &command_id,
                    Status::Unknown,
                    reconciler(),
                    None,
                    Some("interrupted_before_result".to_owned()),
                )?;
                interrupted.push(command_id);
            }
        }
        if !interrupted.is_empty() {
            raise_incident(
                ledger,
                &decision_id,
                "result_lost",
                &format!(
                    "{} had no result when Genesis took the grant over, so the physical outcome is unknown",
                    interrupted.join(", ")
                ),
                now,
            )?;
        }
        let grant = reload(ledger, &decision_id)?;
        if grant.state == GrantState::Granted {
            let unlock = ledger
                .get(&grant.unlock_command_id)?
                .ok_or(GrantError::Corrupt("grant without its unlock command"))?;
            resumed.push(settle_unlock(ledger, &decision_id, &unlock, now)?);
        } else if !interrupted.is_empty() {
            resumed.push(grant);
        }
    }
    Ok(resumed)
}

/// Granty, ktorým vypršalo okno a ešte neboli vrátené späť.
pub fn due(ledger: &Ledger, now: DateTime<Utc>) -> Result<Vec<Grant>, GrantError> {
    let query = format!(
        "SELECT {GRANT_COLUMNS} FROM access_grants
         WHERE state = ?1 AND expires_at <= ?2 ORDER BY expires_at, decision_id"
    );
    let mut statement = ledger.connection().prepare(&query)?;
    let rows = statement.query_map(params![GrantState::Active.as_str(), stamp(now)], read_grant)?;
    rows.map(|row| row?).collect()
}

/// Granty, ktorých relock skončil neisto a čakajú na zosúladenie.
pub fn pending(ledger: &Ledger) -> Result<Vec<Grant>, GrantError> {
    by_state(ledger, GrantState::RelockPending)
}

/// Prijme povel, ktorý grant uzatvára.
///
/// Opakované volanie nezaloží druhý povel, kým ten predchádzajúci nemá výsledok.
pub fn begin_relock(
    ledger: &mut Ledger,
    decision_id: &str,
    now: DateTime<Utc>,
) -> Result<RelockStep, GrantError> {
    let grant = reload(ledger, decision_id)?;
    if !matches!(grant.state, GrantState::Active | GrantState::RelockPending) {
        return Err(GrantError::Invalid(
            "only an open grant can be relocked".into(),
        ));
    }
    // Novšie rozhodnutie drží to isté zariadenie otvorené a má vlastnú
    // expiráciu; starší grant ho preto nesmie zamknúť.
    if superseding_grant(ledger, &grant, now)?.is_some() {
        update_state(ledger, decision_id, GrantState::Superseded, None, None, now)?;
        resolve_incidents(ledger, decision_id, now)?;
        return Ok(RelockStep::Settled(reload(ledger, decision_id)?));
    }
    if let Some(command_id) = in_flight_close(ledger, decision_id)? {
        let snapshot = ledger
            .get(&command_id)?
            .ok_or(GrantError::Corrupt("close command without a snapshot"))?;
        return Ok(RelockStep::Send(snapshot));
    }
    if !may_attempt(ledger, &grant, now)? {
        return Ok(RelockStep::Wait(grant));
    }
    let snapshot = issue_close(ledger, &grant, now)?;
    Ok(RelockStep::Send(snapshot))
}

/// Zapíše výsledok relocku. Neistý výsledok založí incident.
pub fn settle_relock(
    ledger: &mut Ledger,
    decision_id: &str,
    snapshot: &Snapshot,
    now: DateTime<Utc>,
) -> Result<Grant, GrantError> {
    let (grant, required) = load(ledger, decision_id)?;
    if !matches!(grant.state, GrantState::Active | GrantState::RelockPending) {
        return Ok(grant);
    }
    close(ledger, decision_id, &required, snapshot, now)
}

/// Uzavrie grant dôkazom z inventára namiesto ďalšieho povelu.
///
/// Volajúci musí zariadenie naozaj vidieť v hodnote, ktorou sa prístup zatvára;
/// modul si to sám overiť nedokáže. Dôkaz nie je korelovaný s konkrétnym
/// povelom — dokazuje fyzický stav, nie to, ktorý povel ho spôsobil — a preto
/// nesie referenciu `inventory:`, aby to bolo v audite vidieť.
pub fn close_from_observation(
    ledger: &mut Ledger,
    decision_id: &str,
    observed_at: &str,
    now: DateTime<Utc>,
) -> Result<Option<Grant>, GrantError> {
    let (grant, required) = load(ledger, decision_id)?;
    if !GrantState::open().contains(&grant.state) {
        return Ok(None);
    }
    let Ok(at) = canonical_utc(observed_at) else {
        return Ok(None);
    };
    let command_id = match reusable_close(ledger, decision_id)? {
        Some(command_id) => command_id,
        // Bez povelu nie je na čo dôkaz zapísať. Nový sa zakladá pod tou istou
        // hranicou pokusov ako odoslanie, hoci sa nič neposiela.
        None if may_attempt(ledger, &grant, now)? => {
            issue_close(ledger, &grant, now)?.request.command_id
        }
        None => return Ok(None),
    };
    let snapshot = ledger
        .get(&command_id)?
        .ok_or(GrantError::Corrupt("close command without a snapshot"))?;
    let evidence = Evidence {
        kind: EvidenceKind::DeviceObservation,
        reference: format!("inventory:{at}"),
        at,
    };
    let confirmed = match snapshot.status {
        // Nič sa neodoslalo, takže `sent` by bola lož; výsledok je najprv
        // neznámy a až pozorovanie ho zosúladí.
        Status::Accepted => {
            ledger.transition(
                &command_id,
                Status::Unknown,
                reconciler(),
                None,
                Some("not_sent_device_already_in_the_closing_value".to_owned()),
            )?;
            ledger.transition(
                &command_id,
                Status::DeviceConfirmed,
                reconciler(),
                Some(evidence),
                None,
            )?
        }
        Status::Sent | Status::ProviderConfirmed | Status::Unknown => ledger.transition(
            &command_id,
            Status::DeviceConfirmed,
            reconciler(),
            Some(evidence),
            None,
        )?,
        Status::DeviceConfirmed | Status::Failed => return Ok(None),
    };
    Ok(Some(close(
        ledger,
        decision_id,
        &required,
        &confirmed,
        now,
    )?))
}

/// Prijme povel, ktorým rozhodnutie `revert` uzatvára prístup pred expiráciou.
///
/// Toto je cesta pre zmazaný cieľ aj pre override: jeden povel uzavrie všetky
/// granty, ktoré na danom zariadení a schopnosti ešte môžu byť otvorené.
/// `Ok(None)` znamená, že Genesis na tom zariadení nemá čo uzavrieť.
pub fn begin_withdrawal(
    ledger: &mut Ledger,
    decision: &BehaviorDecision,
    now: DateTime<Utc>,
) -> Result<Option<Withdrawal>, GrantError> {
    decision.validate().map_err(GrantError::Invalid)?;
    if !matches!(decision.operation, Operation::Revert) {
        return Err(GrantError::Invalid(
            "only a revert decision withdraws a timed grant".into(),
        ));
    }
    if !decision.active_at(now) {
        return Err(GrantError::Invalid(
            "decision window is not open at this time".into(),
        ));
    }
    let closing = open_grants_on(ledger, decision)?;
    if closing.is_empty() {
        return Ok(None);
    }
    // Revert smie prístup iba zatvoriť. Rovnaká hodnota, akú grant otvoril, by
    // ho držala otvorený ďalej, takže také rozhodnutie sa nevykoná.
    if let Some(grant) = closing
        .iter()
        .find(|grant| grant.granted_value == decision.requested_value)
    {
        return Err(GrantError::Invalid(format!(
            "a revert must not restore the value grant {} opened",
            grant.decision_id
        )));
    }

    let decision_id = decision.decision_id.to_string();
    let command_id = format!("withdraw-{decision_id}");
    let snapshot = ledger.accept(CommandRequest {
        household_id: decision.household_id.clone(),
        command_id: command_id.clone(),
        device_id: decision.device_id.clone(),
        capability_id: decision.capability_id.clone(),
        value: decision.requested_value.into(),
        actor: issuer(decision),
        idempotency_key: decision.idempotency_key.clone(),
        correlation_id: format!("decision-{decision_id}"),
    })?;
    if snapshot.request.command_id != command_id {
        return Err(GrantError::Invalid(
            "idempotency key already belongs to another decision".into(),
        ));
    }
    for grant in &closing {
        let attempt = close_attempts(ledger, &grant.decision_id)? + 1;
        record_command(
            ledger,
            &grant.decision_id,
            &command_id,
            WITHDRAW,
            attempt,
            decision.requested_value,
        )?;
        update_state(
            ledger,
            &grant.decision_id,
            grant.state,
            None,
            Some(&command_id),
            now,
        )?;
    }
    Ok(Some(Withdrawal {
        command_id,
        device_id: decision.device_id.clone(),
        value: decision.requested_value,
        closing: closing.into_iter().map(|grant| grant.decision_id).collect(),
        accepted: snapshot,
    }))
}

/// Zapíše výsledok jedného withdraw povelu do každého grantu, ktorý uzatváral.
pub fn settle_withdrawal(
    ledger: &mut Ledger,
    withdrawal: &Withdrawal,
    snapshot: &Snapshot,
    now: DateTime<Utc>,
) -> Result<Vec<Grant>, GrantError> {
    let mut settled = Vec::new();
    for decision_id in &withdrawal.closing {
        let (grant, required) = load(ledger, decision_id)?;
        if GrantState::open().contains(&grant.state) {
            settled.push(close(ledger, decision_id, &required, snapshot, now)?);
        } else {
            settled.push(grant);
        }
    }
    Ok(settled)
}

pub fn get(ledger: &Ledger, decision_id: &str) -> Result<Option<Grant>, GrantError> {
    let query = format!("SELECT {GRANT_COLUMNS} FROM access_grants WHERE decision_id = ?1");
    ledger
        .connection()
        .query_row(&query, [decision_id], read_grant)
        .optional()?
        .transpose()
}

/// Posledný potvrdený stav a otvorené incidenty jedného grantu.
pub fn access_state(ledger: &Ledger, decision_id: &str) -> Result<Option<AccessState>, GrantError> {
    let Some(grant) = get(ledger, decision_id)? else {
        return Ok(None);
    };
    Ok(Some(AccessState {
        required_confirmation: confirmation_str(&required_confirmation(ledger, decision_id)?),
        close_attempts: close_attempts(ledger, decision_id)?,
        last_confirmed: last_confirmed(ledger, decision_id)?,
        open_incidents: open_incidents(ledger, decision_id)?,
        grant,
    }))
}

/// Prehľad grantov domácnosti, od naposledy zmeneného.
pub fn overview(ledger: &Ledger, household_id: &str) -> Result<Vec<AccessState>, GrantError> {
    let query = format!(
        "SELECT {GRANT_COLUMNS} FROM access_grants WHERE household_id = ?1
         ORDER BY updated_at DESC, decision_id LIMIT ?2"
    );
    let mut statement = ledger.connection().prepare(&query)?;
    let rows = statement.query_map(params![household_id, OVERVIEW_LIMIT], read_grant)?;
    let grants = rows
        .map(|row| row?)
        .collect::<Result<Vec<Grant>, GrantError>>()?;
    grants
        .into_iter()
        .map(|grant| {
            Ok(AccessState {
                required_confirmation: confirmation_str(&required_confirmation(
                    ledger,
                    &grant.decision_id,
                )?),
                close_attempts: close_attempts(ledger, &grant.decision_id)?,
                last_confirmed: last_confirmed(ledger, &grant.decision_id)?,
                open_incidents: open_incidents(ledger, &grant.decision_id)?,
                grant,
            })
        })
        .collect()
}

pub fn incidents(ledger: &Ledger, decision_id: &str) -> Result<Vec<Incident>, GrantError> {
    read_incidents(
        ledger,
        "SELECT incident_id, decision_id, kind, detail, at, resolved_at
         FROM incidents WHERE decision_id = ?1 ORDER BY at, incident_id",
        decision_id,
    )
}

pub fn open_incidents(ledger: &Ledger, decision_id: &str) -> Result<Vec<Incident>, GrantError> {
    read_incidents(
        ledger,
        "SELECT incident_id, decision_id, kind, detail, at, resolved_at
         FROM incidents WHERE decision_id = ?1 AND resolved_at IS NULL
         ORDER BY at, incident_id",
        decision_id,
    )
}

/// Uzavretie grantu podľa toho, či výsledok dosiahol vyžadované potvrdenie.
fn close(
    ledger: &mut Ledger,
    decision_id: &str,
    required: &RequiredConfirmation,
    snapshot: &Snapshot,
    now: DateTime<Utc>,
) -> Result<Grant, GrantError> {
    if confirmed_enough(&snapshot.status, required) {
        update_state(ledger, decision_id, GrantState::Relocked, None, None, now)?;
        resolve_incidents(ledger, decision_id, now)?;
        return reload(ledger, decision_id);
    }
    update_state(
        ledger,
        decision_id,
        GrantState::RelockPending,
        None,
        None,
        now,
    )?;
    raise_incident(
        ledger,
        decision_id,
        "relock_uncertain",
        &format!(
            "the relock ended as {} without the confirmation the decision required",
            status_name(&snapshot.status)
        ),
        now,
    )?;
    if close_attempts(ledger, decision_id)? >= MAX_CLOSE_ATTEMPTS {
        raise_incident(
            ledger,
            decision_id,
            "relock_exhausted",
            &format!(
                "{MAX_CLOSE_ATTEMPTS} attempts did not confirm the device; a person has to check it"
            ),
            now,
        )?;
    }
    reload(ledger, decision_id)
}

/// Prijme ďalší povel, ktorý grant uzatvára. Prvý pokus si drží kľúč aj
/// identifikátor z ELYSIUM-343, ďalšie pridávajú číslo pokusu.
fn issue_close(
    ledger: &mut Ledger,
    grant: &Grant,
    now: DateTime<Utc>,
) -> Result<Snapshot, GrantError> {
    let attempt = close_attempts(ledger, &grant.decision_id)? + 1;
    let decision_id = &grant.decision_id;
    let base = unlock_key(ledger, decision_id)?;
    let (command_id, key) = if attempt == 1 {
        (
            format!("relock-{decision_id}"),
            format!("{base}{RELOCK_SUFFIX}"),
        )
    } else {
        (
            format!("relock{attempt}-{decision_id}"),
            format!("{base}{RELOCK_SUFFIX}{attempt}"),
        )
    };
    let snapshot = ledger.accept(CommandRequest {
        household_id: grant.household_id.clone(),
        command_id: command_id.clone(),
        device_id: grant.device_id.clone(),
        capability_id: grant.capability_id.clone(),
        value: grant.closing_value().into(),
        actor: reconciler(),
        idempotency_key: key,
        correlation_id: format!("decision-{decision_id}"),
    })?;
    record_command(
        ledger,
        decision_id,
        &command_id,
        RELOCK,
        attempt,
        grant.closing_value(),
    )?;
    update_state(
        ledger,
        decision_id,
        grant.state,
        None,
        Some(&command_id),
        now,
    )?;
    Ok(snapshot)
}

/// Smie sa poslať ďalší pokus? Prvý nečaká, ďalšie majú rastúci odstup a po
/// vyčerpaní limitu sa už neposiela nič.
fn may_attempt(ledger: &Ledger, grant: &Grant, now: DateTime<Utc>) -> Result<bool, GrantError> {
    let attempts = close_attempts(ledger, &grant.decision_id)?;
    if attempts == 0 {
        return Ok(true);
    }
    if attempts >= MAX_CLOSE_ATTEMPTS {
        return Ok(false);
    }
    let waited = (now - parse_stamp(&grant.updated_at)?).num_seconds();
    Ok(waited >= RETRY_BACKOFF_SECONDS << (attempts - 1))
}

fn confirmed_enough(status: &Status, required: &RequiredConfirmation) -> bool {
    match required {
        RequiredConfirmation::Provider => {
            matches!(status, Status::ProviderConfirmed | Status::DeviceConfirmed)
        }
        RequiredConfirmation::Device => matches!(status, Status::DeviceConfirmed),
    }
}

fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Accepted => "accepted",
        Status::Sent => "sent",
        Status::ProviderConfirmed => "provider_confirmed",
        Status::DeviceConfirmed => "device_confirmed",
        Status::Unknown => "unknown",
        Status::Failed => "failed",
    }
}

fn issuer(decision: &BehaviorDecision) -> Actor {
    Actor {
        actor_type: ActorType::Automation,
        actor_id: decision.issuer.clone(),
    }
}

fn reconciler() -> Actor {
    Actor {
        actor_type: ActorType::Automation,
        actor_id: RECONCILER.to_owned(),
    }
}

fn unlock_key(ledger: &Ledger, decision_id: &str) -> Result<String, GrantError> {
    Ok(ledger.connection().query_row(
        "SELECT c.idempotency_key FROM commands c
         JOIN access_grants g ON g.unlock_command_id = c.command_id
         WHERE g.decision_id = ?1",
        [decision_id],
        |row| row.get(0),
    )?)
}

/// Povely grantu od najstaršieho: unlock, potom každé uzavretie.
fn commands_of(
    ledger: &Ledger,
    decision_id: &str,
) -> Result<Vec<(String, String, i64)>, GrantError> {
    let mut statement = ledger.connection().prepare(
        "SELECT command_id, kind, attempt FROM grant_commands
         WHERE decision_id = ?1 ORDER BY kind = 'unlock' DESC, attempt, command_id",
    )?;
    let rows = statement.query_map([decision_id], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn close_attempts(ledger: &Ledger, decision_id: &str) -> Result<i64, GrantError> {
    Ok(ledger.connection().query_row(
        "SELECT COUNT(*) FROM grant_commands WHERE decision_id = ?1 AND kind <> ?2",
        params![decision_id, UNLOCK],
        |row| row.get(0),
    )?)
}

fn latest_close(ledger: &Ledger, decision_id: &str) -> Result<Option<String>, GrantError> {
    Ok(ledger
        .connection()
        .query_row(
            "SELECT command_id FROM grant_commands
             WHERE decision_id = ?1 AND kind <> ?2 ORDER BY attempt DESC LIMIT 1",
            params![decision_id, UNLOCK],
            |row| row.get(0),
        )
        .optional()?)
}

/// Posledný uzatvárací povel, ktorý ešte nemá výsledok.
fn in_flight_close(ledger: &mut Ledger, decision_id: &str) -> Result<Option<String>, GrantError> {
    let Some(command_id) = latest_close(ledger, decision_id)? else {
        return Ok(None);
    };
    let Some(snapshot) = ledger.get(&command_id)? else {
        return Ok(None);
    };
    Ok(matches!(snapshot.status, Status::Accepted | Status::Sent).then_some(command_id))
}

/// Posledný uzatvárací povel, na ktorý sa dá zapísať pozorovanie.
fn reusable_close(ledger: &mut Ledger, decision_id: &str) -> Result<Option<String>, GrantError> {
    let Some(command_id) = latest_close(ledger, decision_id)? else {
        return Ok(None);
    };
    let Some(snapshot) = ledger.get(&command_id)? else {
        return Ok(None);
    };
    Ok(matches!(
        snapshot.status,
        Status::Accepted | Status::Sent | Status::ProviderConfirmed | Status::Unknown
    )
    .then_some(command_id))
}

fn last_confirmed(
    ledger: &Ledger,
    decision_id: &str,
) -> Result<Option<ConfirmedState>, GrantError> {
    let mut latest: Option<ConfirmedState> = None;
    for (command_id, _, _) in commands_of(ledger, decision_id)? {
        let Some(snapshot) = ledger.get(&command_id)? else {
            continue;
        };
        let confirmation = match snapshot.status {
            Status::ProviderConfirmed => "provider",
            Status::DeviceConfirmed => "device",
            _ => continue,
        };
        let candidate = ConfirmedState {
            command_id,
            value: snapshot.request.value.clone(),
            confirmation,
            at: snapshot.status_changed_at.clone(),
            observed_at: snapshot
                .evidence
                .as_ref()
                .map(|evidence| evidence.at.clone()),
        };
        if latest.as_ref().is_none_or(|best| best.at <= candidate.at) {
            latest = Some(candidate);
        }
    }
    Ok(latest)
}

fn opposing_grant(
    ledger: &Ledger,
    decision: &BehaviorDecision,
    now: DateTime<Utc>,
) -> Result<Option<String>, GrantError> {
    Ok(ledger
        .connection()
        .query_row(
            "SELECT decision_id FROM access_grants
             WHERE decision_id <> ?1 AND household_id = ?2 AND device_id = ?3
               AND capability_id = ?4 AND granted_value <> ?5
               AND state IN (?6, ?7) AND expires_at > ?8
             ORDER BY expires_at DESC LIMIT 1",
            params![
                decision.decision_id.to_string(),
                decision.household_id,
                decision.device_id,
                decision.capability_id,
                decision.requested_value,
                GrantState::Granted.as_str(),
                GrantState::Active.as_str(),
                stamp(now)
            ],
            |row| row.get(0),
        )
        .optional()?)
}

fn superseding_grant(
    ledger: &Ledger,
    grant: &Grant,
    now: DateTime<Utc>,
) -> Result<Option<String>, GrantError> {
    Ok(ledger
        .connection()
        .query_row(
            "SELECT decision_id FROM access_grants
             WHERE decision_id <> ?1 AND household_id = ?2 AND device_id = ?3
               AND capability_id = ?4 AND granted_value = ?5
               AND state IN (?6, ?7) AND expires_at > ?8
             ORDER BY expires_at DESC LIMIT 1",
            params![
                grant.decision_id,
                grant.household_id,
                grant.device_id,
                grant.capability_id,
                grant.granted_value,
                GrantState::Granted.as_str(),
                GrantState::Active.as_str(),
                stamp(now)
            ],
            |row| row.get(0),
        )
        .optional()?)
}

fn open_grants_on(ledger: &Ledger, decision: &BehaviorDecision) -> Result<Vec<Grant>, GrantError> {
    let query = format!(
        "SELECT {GRANT_COLUMNS} FROM access_grants
         WHERE household_id = ?1 AND device_id = ?2 AND capability_id = ?3
           AND state IN (?4, ?5, ?6) ORDER BY expires_at, decision_id"
    );
    let mut statement = ledger.connection().prepare(&query)?;
    let rows = statement.query_map(
        params![
            decision.household_id,
            decision.device_id,
            decision.capability_id,
            GrantState::Granted.as_str(),
            GrantState::Active.as_str(),
            GrantState::RelockPending.as_str()
        ],
        read_grant,
    )?;
    rows.map(|row| row?).collect()
}

fn open_decision_ids(ledger: &Ledger) -> Result<Vec<String>, GrantError> {
    let mut statement = ledger.connection().prepare(
        "SELECT decision_id FROM access_grants WHERE state IN (?1, ?2, ?3)
         ORDER BY expires_at, decision_id",
    )?;
    let rows = statement.query_map(
        params![
            GrantState::Granted.as_str(),
            GrantState::Active.as_str(),
            GrantState::RelockPending.as_str()
        ],
        |row| row.get(0),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn by_state(ledger: &Ledger, state: GrantState) -> Result<Vec<Grant>, GrantError> {
    let query = format!(
        "SELECT {GRANT_COLUMNS} FROM access_grants WHERE state = ?1
         ORDER BY expires_at, decision_id"
    );
    let mut statement = ledger.connection().prepare(&query)?;
    let rows = statement.query_map([state.as_str()], read_grant)?;
    rows.map(|row| row?).collect()
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

fn record_command(
    ledger: &mut Ledger,
    decision_id: &str,
    command_id: &str,
    kind: &str,
    attempt: i64,
    value: bool,
) -> Result<(), GrantError> {
    ledger.connection().execute(
        "INSERT OR IGNORE INTO grant_commands
         (command_id, decision_id, kind, attempt, value) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![command_id, decision_id, kind, attempt, value],
    )?;
    Ok(())
}

fn update_state(
    ledger: &mut Ledger,
    decision_id: &str,
    state: GrantState,
    unlock_confirmed: Option<bool>,
    relock_command_id: Option<&str>,
    now: DateTime<Utc>,
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
            stamp(now)
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
    now: DateTime<Utc>,
) -> Result<(), GrantError> {
    ledger.connection().execute(
        "INSERT OR IGNORE INTO incidents (incident_id, decision_id, kind, detail, at, resolved_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
        params![
            format!("{kind}-{decision_id}"),
            decision_id,
            kind,
            detail,
            stamp(now)
        ],
    )?;
    Ok(())
}

fn resolve_incidents(
    ledger: &mut Ledger,
    decision_id: &str,
    now: DateTime<Utc>,
) -> Result<(), GrantError> {
    ledger.connection().execute(
        "UPDATE incidents SET resolved_at = ?2
         WHERE decision_id = ?1 AND resolved_at IS NULL",
        params![decision_id, stamp(now)],
    )?;
    Ok(())
}

fn read_incidents(
    ledger: &Ledger,
    query: &str,
    decision_id: &str,
) -> Result<Vec<Incident>, GrantError> {
    let mut statement = ledger.connection().prepare(query)?;
    let rows = statement.query_map([decision_id], |row| {
        Ok(Incident {
            incident_id: row.get(0)?,
            decision_id: row.get(1)?,
            kind: row.get(2)?,
            detail: row.get(3)?,
            at: row.get(4)?,
            resolved_at: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn load(ledger: &Ledger, decision_id: &str) -> Result<(Grant, RequiredConfirmation), GrantError> {
    Ok((
        reload(ledger, decision_id)?,
        required_confirmation(ledger, decision_id)?,
    ))
}

fn reload(ledger: &Ledger, decision_id: &str) -> Result<Grant, GrantError> {
    get(ledger, decision_id)?.ok_or(GrantError::NotFound)
}

fn required_confirmation(
    ledger: &Ledger,
    decision_id: &str,
) -> Result<RequiredConfirmation, GrantError> {
    let raw: String = ledger.connection().query_row(
        "SELECT required_confirmation FROM access_grants WHERE decision_id = ?1",
        [decision_id],
        |row| row.get(0),
    )?;
    match raw.as_str() {
        "provider" => Ok(RequiredConfirmation::Provider),
        "device" => Ok(RequiredConfirmation::Device),
        _ => Err(GrantError::Corrupt("unknown required confirmation")),
    }
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

fn stamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_stamp(raw: &str) -> Result<DateTime<Utc>, GrantError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| GrantError::Corrupt("stored timestamp is not RFC 3339"))
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
    use serde_json::json;

    const VALID_FROM: &str = "2026-09-29T18:00:00Z";
    const EXPIRES_AT: &str = "2026-09-29T19:00:00Z";
    const AFTER_EXPIRY: &str = "2026-09-29T19:30:00Z";
    const DECISION: &str = "ff77bdb0-70af-4f2a-a913-76609b66761b";
    const OTHER_DECISION: &str = "b0c3f0d1-2f4a-4a53-9f7c-6ac1d1b2e3f4";

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn build(
        decision_id: &str,
        key: &str,
        operation: &str,
        requested_value: bool,
        expires_at: &str,
        required: &str,
    ) -> BehaviorDecision {
        BehaviorDecision::parse(
            &json!({
                "schema_version": "1.0",
                "decision_id": decision_id,
                "issuer": "behavior-engine",
                "household_id": "pilot-home",
                "central_unit_id": "ad19a578-21e2-453f-a57c-1913350be34e",
                "subject_id": "64582f6b-38a5-48dd-9ed4-ae02949c7740",
                "device_id": "ha:light.living",
                "capability_id": "power",
                "requested_value": requested_value,
                "operation": operation,
                "valid_from": VALID_FROM,
                "expires_at": expires_at,
                "reason_code": "goal_verified",
                "idempotency_key": key,
                "required_confirmation": required
            })
            .to_string(),
        )
        .unwrap()
    }

    fn decision(required: &str) -> BehaviorDecision {
        build(
            DECISION,
            "behavior:ff77bdb0",
            "apply",
            true,
            EXPIRES_AT,
            required,
        )
    }

    fn automation() -> Actor {
        Actor {
            actor_type: ActorType::Automation,
            actor_id: "behavior-engine".to_owned(),
        }
    }

    /// Dovedie povel do cieľového stavu tak, ako by to spravil HA adaptér.
    fn drive(
        ledger: &mut Ledger,
        command_id: &str,
        target: Status,
        now: DateTime<Utc>,
    ) -> Snapshot {
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
                    at: stamp(now),
                }),
                None,
            ),
            Status::ProviderConfirmed => (
                Some(Evidence {
                    kind: EvidenceKind::ProviderAck,
                    reference: "ack-1".to_owned(),
                    at: stamp(now),
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

    fn sent(step: RelockStep) -> Snapshot {
        match step {
            RelockStep::Send(snapshot) => snapshot,
            other => panic!("expected a command to send, got {other:?}"),
        }
    }

    /// Odomkne grant a potvrdí unlock, čo je vstup väčšiny scenárov.
    fn unlocked(ledger: &mut Ledger, decision: &BehaviorDecision, now: DateTime<Utc>) -> Grant {
        let grant = open(ledger, decision, now).unwrap();
        let confirmed = drive(
            ledger,
            &grant.unlock_command_id,
            Status::DeviceConfirmed,
            now,
        );
        settle_unlock(ledger, &grant.decision_id, &confirmed, now).unwrap()
    }

    #[test]
    fn full_cycle_unlocks_then_relocks_after_expiry() {
        let mut ledger = ledger();
        let decision = decision("device");
        let now = at("2026-09-29T18:30:00Z");
        let grant = open(&mut ledger, &decision, now).unwrap();
        assert_eq!(grant.state, GrantState::Granted);
        assert!(grant.granted_value);

        let unlocked = drive(
            &mut ledger,
            &grant.unlock_command_id,
            Status::DeviceConfirmed,
            now,
        );
        let grant = settle_unlock(&mut ledger, &grant.decision_id, &unlocked, now).unwrap();
        assert_eq!(grant.state, GrantState::Active);
        assert!(grant.unlock_confirmed);

        // Počas okna sa nič nevracia.
        assert!(due(&ledger, at("2026-09-29T18:45:00Z")).unwrap().is_empty());

        let expiry = at(AFTER_EXPIRY);
        let outstanding = due(&ledger, expiry).unwrap();
        assert_eq!(outstanding.len(), 1);

        let relock = sent(begin_relock(&mut ledger, &grant.decision_id, expiry).unwrap());
        assert_eq!(relock.request.value, json!(false));
        assert_eq!(relock.request.idempotency_key, "behavior:ff77bdb0:relock");
        assert_eq!(relock.request.actor.actor_id, RECONCILER);

        let relocked = drive(
            &mut ledger,
            &relock.request.command_id,
            Status::DeviceConfirmed,
            expiry,
        );
        let grant = settle_relock(&mut ledger, &grant.decision_id, &relocked, expiry).unwrap();
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
            now,
        );
        let settled = settle_unlock(&mut ledger, &first.decision_id, &unlocked, now).unwrap();
        assert_eq!(
            settle_unlock(&mut ledger, &first.decision_id, &unlocked, now).unwrap(),
            settled
        );

        let expiry = at(AFTER_EXPIRY);
        let relock = sent(begin_relock(&mut ledger, &first.decision_id, expiry).unwrap());
        let again = sent(begin_relock(&mut ledger, &first.decision_id, expiry).unwrap());
        assert_eq!(relock.request.command_id, again.request.command_id);
        assert_eq!(ledger.events(&relock.request.command_id).unwrap().len(), 1);
        assert_eq!(close_attempts(&ledger, &first.decision_id).unwrap(), 1);

        let relocked = drive(
            &mut ledger,
            &relock.request.command_id,
            Status::DeviceConfirmed,
            expiry,
        );
        let done = settle_relock(&mut ledger, &first.decision_id, &relocked, expiry).unwrap();
        assert_eq!(done.state, GrantState::Relocked);
        assert_eq!(
            settle_relock(&mut ledger, &first.decision_id, &relocked, expiry).unwrap(),
            done
        );
    }

    #[test]
    fn uncertain_relock_becomes_relock_pending_with_one_incident() {
        let mut ledger = ledger();
        let now = at("2026-09-29T18:30:00Z");
        let grant = unlocked(&mut ledger, &decision("device"), now);

        let expiry = at(AFTER_EXPIRY);
        let relock = sent(begin_relock(&mut ledger, &grant.decision_id, expiry).unwrap());
        let unsure = drive(
            &mut ledger,
            &relock.request.command_id,
            Status::Unknown,
            expiry,
        );
        let grant = settle_relock(&mut ledger, &grant.decision_id, &unsure, expiry).unwrap();
        assert_eq!(grant.state, GrantState::RelockPending);

        let raised = incidents(&ledger, &grant.decision_id).unwrap();
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].kind, "relock_uncertain");
        assert!(raised[0].resolved_at.is_none());

        // Opakované doručenie toho istého výsledku nezaloží druhý incident.
        settle_relock(&mut ledger, &grant.decision_id, &unsure, expiry).unwrap();
        assert_eq!(incidents(&ledger, &grant.decision_id).unwrap().len(), 1);
    }

    #[test]
    fn provider_ack_alone_does_not_settle_a_relock_that_needs_the_device() {
        let mut ledger = ledger();
        let now = at("2026-09-29T18:30:00Z");
        let grant = unlocked(&mut ledger, &decision("device"), now);
        let expiry = at(AFTER_EXPIRY);
        let relock = sent(begin_relock(&mut ledger, &grant.decision_id, expiry).unwrap());
        let ack = drive(
            &mut ledger,
            &relock.request.command_id,
            Status::ProviderConfirmed,
            expiry,
        );
        let grant = settle_relock(&mut ledger, &grant.decision_id, &ack, expiry).unwrap();
        assert_eq!(grant.state, GrantState::RelockPending);
        assert_eq!(incidents(&ledger, &grant.decision_id).unwrap().len(), 1);
    }

    #[test]
    fn an_uncertain_unlock_is_still_relocked_but_a_failed_one_is_not() {
        let now = at("2026-09-29T18:30:00Z");
        let mut unsure = ledger();
        let grant = open(&mut unsure, &decision("device"), now).unwrap();
        let outcome = drive(&mut unsure, &grant.unlock_command_id, Status::Unknown, now);
        let grant = settle_unlock(&mut unsure, &grant.decision_id, &outcome, now).unwrap();
        assert_eq!(grant.state, GrantState::Active);
        assert!(!grant.unlock_confirmed);
        assert_eq!(due(&unsure, at(AFTER_EXPIRY)).unwrap().len(), 1);

        let mut failed = ledger();
        let grant = open(&mut failed, &decision("device"), now).unwrap();
        let outcome = drive(&mut failed, &grant.unlock_command_id, Status::Failed, now);
        let grant = settle_unlock(&mut failed, &grant.decision_id, &outcome, now).unwrap();
        assert_eq!(grant.state, GrantState::UnlockFailed);
        assert!(due(&failed, at(AFTER_EXPIRY)).unwrap().is_empty());
    }

    #[test]
    fn an_expiry_given_in_whole_seconds_is_due_at_that_instant() {
        let mut ledger = ledger();
        let grant = unlocked(&mut ledger, &decision("device"), at("2026-09-29T18:30:00Z"));
        assert!(due(&ledger, at("2026-09-29T18:59:59Z")).unwrap().is_empty());
        assert_eq!(due(&ledger, at(EXPIRES_AT)).unwrap().len(), 1);
        assert_eq!(grant.expires_at, "2026-09-29T19:00:00.000Z");
    }

    #[test]
    fn a_revert_decision_or_a_closed_window_opens_no_grant() {
        let mut ledger = ledger();
        let mut revert = decision("device");
        revert.operation = Operation::Revert;
        assert!(open(&mut ledger, &revert, at("2026-09-29T18:30:00Z")).is_err());
        assert!(open(&mut ledger, &decision("device"), at(AFTER_EXPIRY)).is_err());
        assert!(get(&ledger, DECISION).unwrap().is_none());
    }

    #[test]
    fn a_restart_in_flight_admits_the_unknown_result_and_keeps_the_grant_lockable() {
        let mut ledger = ledger();
        let grant = open(&mut ledger, &decision("device"), at("2026-09-29T18:30:00Z")).unwrap();
        // Proces skončil po odoslaní unlocku a pred jeho výsledkom.
        ledger
            .transition(
                &grant.unlock_command_id,
                Status::Sent,
                automation(),
                None,
                None,
            )
            .unwrap();

        let resumed = resume(&mut ledger, at("2026-09-29T18:31:00Z")).unwrap();
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].state, GrantState::Active);
        assert!(!resumed[0].unlock_confirmed);
        assert_eq!(
            ledger
                .get(&grant.unlock_command_id)
                .unwrap()
                .unwrap()
                .status,
            Status::Unknown
        );
        let raised = open_incidents(&ledger, &grant.decision_id).unwrap();
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].kind, "result_lost");
        // Neznámy výsledok znamená možný otvorený prístup, takže zámok musí prísť.
        assert_eq!(due(&ledger, at(AFTER_EXPIRY)).unwrap().len(), 1);
        // Ďalší štart už nemá čo dorovnávať.
        assert!(resume(&mut ledger, at("2026-09-29T18:32:00Z"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_longer_grant_on_the_same_device_supersedes_the_older_one() {
        let mut ledger = ledger();
        let now = at("2026-09-29T18:30:00Z");
        let first = unlocked(&mut ledger, &decision("device"), now);
        let extended = build(
            OTHER_DECISION,
            "behavior:b0c3f0d1",
            "apply",
            true,
            "2026-09-29T20:00:00Z",
            "device",
        );
        let second = unlocked(&mut ledger, &extended, now);

        // Staršiemu grantu vypršalo okno, ale zariadenie drží novšie rozhodnutie.
        let expiry = at(AFTER_EXPIRY);
        assert_eq!(due(&ledger, expiry).unwrap().len(), 1);
        let step = begin_relock(&mut ledger, &first.decision_id, expiry).unwrap();
        let RelockStep::Settled(closed) = step else {
            panic!("an older grant must not lock a device a newer grant holds");
        };
        assert_eq!(closed.state, GrantState::Superseded);
        assert!(closed.relock_command_id.is_none());
        assert_eq!(close_attempts(&ledger, &first.decision_id).unwrap(), 0);

        // Novší grant sa zamyká sám, keď doňho dorastie čas.
        let outstanding = due(&ledger, at("2026-09-29T20:30:00Z")).unwrap();
        assert_eq!(outstanding.len(), 1);
        assert_eq!(outstanding[0].decision_id, second.decision_id);
    }

    #[test]
    fn an_opposing_grant_on_the_same_device_is_refused() {
        let mut ledger = ledger();
        let now = at("2026-09-29T18:30:00Z");
        unlocked(&mut ledger, &decision("device"), now);
        let opposite = build(
            OTHER_DECISION,
            "behavior:b0c3f0d1",
            "apply",
            false,
            "2026-09-29T20:00:00Z",
            "device",
        );
        assert!(open(&mut ledger, &opposite, now).is_err());
        assert!(get(&ledger, OTHER_DECISION).unwrap().is_none());
    }

    #[test]
    fn a_revert_closes_the_grant_before_expiry() {
        let mut ledger = ledger();
        let grant = unlocked(&mut ledger, &decision("device"), at("2026-09-29T18:30:00Z"));

        // Cieľ zmizol, takže engine pošle revert ešte v okne grantu.
        let cancel = build(
            OTHER_DECISION,
            "behavior:b0c3f0d1",
            "revert",
            false,
            EXPIRES_AT,
            "device",
        );
        let moment = at("2026-09-29T18:40:00Z");
        let withdrawal = begin_withdrawal(&mut ledger, &cancel, moment)
            .unwrap()
            .unwrap();
        assert_eq!(withdrawal.closing, vec![grant.decision_id.clone()]);
        assert_eq!(withdrawal.accepted.request.value, json!(false));
        assert_eq!(
            withdrawal.accepted.request.idempotency_key,
            "behavior:b0c3f0d1"
        );

        let outcome = drive(
            &mut ledger,
            &withdrawal.command_id,
            Status::DeviceConfirmed,
            moment,
        );
        let settled = settle_withdrawal(&mut ledger, &withdrawal, &outcome, moment).unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].state, GrantState::Relocked);
        // Uzavretý grant už nie je splatný a druhé doručenie revertu nič nespraví.
        assert!(due(&ledger, at(AFTER_EXPIRY)).unwrap().is_empty());
        assert!(
            begin_withdrawal(&mut ledger, &cancel, at("2026-09-29T18:41:00Z"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_revert_that_would_restore_the_granted_value_is_refused() {
        let mut ledger = ledger();
        unlocked(&mut ledger, &decision("device"), at("2026-09-29T18:30:00Z"));
        let wrong = build(
            OTHER_DECISION,
            "behavior:b0c3f0d1",
            "revert",
            true,
            EXPIRES_AT,
            "device",
        );
        assert!(begin_withdrawal(&mut ledger, &wrong, at("2026-09-29T18:40:00Z")).is_err());
    }

    #[test]
    fn a_pending_relock_is_retried_with_its_own_key_until_the_limit() {
        let mut ledger = ledger();
        let grant = unlocked(&mut ledger, &decision("device"), at("2026-09-29T18:30:00Z"));
        let mut moment = at(AFTER_EXPIRY);
        let mut keys = Vec::new();

        for attempt in 1..=MAX_CLOSE_ATTEMPTS {
            let accepted = sent(begin_relock(&mut ledger, &grant.decision_id, moment).unwrap());
            keys.push(accepted.request.idempotency_key.clone());
            let unsure = drive(
                &mut ledger,
                &accepted.request.command_id,
                Status::Unknown,
                moment,
            );
            let settled = settle_relock(&mut ledger, &grant.decision_id, &unsure, moment).unwrap();
            assert_eq!(settled.state, GrantState::RelockPending);
            assert_eq!(
                close_attempts(&ledger, &grant.decision_id).unwrap(),
                attempt
            );
            // Hneď po neistom výsledku sa ďalší pokus neposiela.
            assert!(matches!(
                begin_relock(&mut ledger, &grant.decision_id, moment).unwrap(),
                RelockStep::Wait(_)
            ));
            moment += chrono::TimeDelta::seconds(RETRY_BACKOFF_SECONDS << (attempt - 1));
        }

        assert_eq!(
            keys,
            [
                "behavior:ff77bdb0:relock",
                "behavior:ff77bdb0:relock2",
                "behavior:ff77bdb0:relock3",
                "behavior:ff77bdb0:relock4",
                "behavior:ff77bdb0:relock5"
            ]
        );
        // Po vyčerpaní limitu už zosúladenie neposiela nič, aj keď odstup uplynul.
        assert!(matches!(
            begin_relock(&mut ledger, &grant.decision_id, at("2026-09-30T06:00:00Z")).unwrap(),
            RelockStep::Wait(_)
        ));
        let raised = open_incidents(&ledger, &grant.decision_id).unwrap();
        assert_eq!(raised.len(), 2);
        assert!(raised.iter().any(|item| item.kind == "relock_exhausted"));
        assert_eq!(pending(&ledger).unwrap().len(), 1);
    }

    #[test]
    fn an_observation_of_the_closing_value_settles_the_grant_without_sending() {
        let mut ledger = ledger();
        let grant = unlocked(&mut ledger, &decision("device"), at("2026-09-29T18:30:00Z"));
        let expiry = at(AFTER_EXPIRY);
        let accepted = sent(begin_relock(&mut ledger, &grant.decision_id, expiry).unwrap());
        let unsure = drive(
            &mut ledger,
            &accepted.request.command_id,
            Status::Unknown,
            expiry,
        );
        settle_relock(&mut ledger, &grant.decision_id, &unsure, expiry).unwrap();
        assert_eq!(
            open_incidents(&ledger, &grant.decision_id).unwrap().len(),
            1
        );

        // Inventár medzitým vidí zariadenie vypnuté. Dôkaz uzavrie grant a
        // žiadny ďalší povel nevznikne; HA čas prichádza s posunom, nie so `Z`.
        let closed = close_from_observation(
            &mut ledger,
            &grant.decision_id,
            "2026-09-29T19:35:00.500+00:00",
            at("2026-09-29T19:40:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(closed.state, GrantState::Relocked);
        assert_eq!(close_attempts(&ledger, &grant.decision_id).unwrap(), 1);
        assert!(open_incidents(&ledger, &grant.decision_id)
            .unwrap()
            .is_empty());
        assert!(pending(&ledger).unwrap().is_empty());

        let evidence = ledger
            .get(&accepted.request.command_id)
            .unwrap()
            .unwrap()
            .evidence
            .unwrap();
        assert_eq!(evidence.kind, EvidenceKind::DeviceObservation);
        assert_eq!(evidence.reference, "inventory:2026-09-29T19:35:00.500Z");
    }

    #[test]
    fn the_overview_shows_the_last_confirmed_state_and_the_open_incident() {
        let mut ledger = ledger();
        let grant = unlocked(&mut ledger, &decision("device"), at("2026-09-29T18:30:00Z"));
        let expiry = at(AFTER_EXPIRY);
        let accepted = sent(begin_relock(&mut ledger, &grant.decision_id, expiry).unwrap());
        let unsure = drive(
            &mut ledger,
            &accepted.request.command_id,
            Status::Unknown,
            expiry,
        );
        settle_relock(&mut ledger, &grant.decision_id, &unsure, expiry).unwrap();

        let states = overview(&ledger, "pilot-home").unwrap();
        assert_eq!(states.len(), 1);
        let state = &states[0];
        assert_eq!(state.grant.state, GrantState::RelockPending);
        assert_eq!(state.required_confirmation, "device");
        assert_eq!(state.close_attempts, 1);
        assert_eq!(state.open_incidents.len(), 1);
        assert_eq!(state.open_incidents[0].kind, "relock_uncertain");

        // Posledné, čo Genesis potvrdil, je odomknutie; relock potvrdený nebol.
        let last = state.last_confirmed.as_ref().unwrap();
        assert_eq!(last.command_id, grant.unlock_command_id);
        assert_eq!(last.value, json!(true));
        assert_eq!(last.confirmation, "device");
        assert!(last.observed_at.is_some());

        assert!(access_state(&ledger, DECISION).unwrap().is_some());
        assert!(overview(&ledger, "other-home").unwrap().is_empty());
    }

    /// Presne to, čo v databáze zostalo po ELYSIUM-343: incidenty bez uzavretia
    /// a žiadna tabuľka povelov grantu.
    const PREVIOUS_SCHEMA: &str = "CREATE TABLE access_grants (
           decision_id TEXT PRIMARY KEY, household_id TEXT NOT NULL,
           device_id TEXT NOT NULL, capability_id TEXT NOT NULL,
           granted_value INTEGER NOT NULL, required_confirmation TEXT NOT NULL,
           expires_at TEXT NOT NULL, state TEXT NOT NULL,
           unlock_confirmed INTEGER NOT NULL, unlock_command_id TEXT NOT NULL,
           relock_command_id TEXT, updated_at TEXT NOT NULL
         );
         CREATE TABLE incidents (
           incident_id TEXT PRIMARY KEY, decision_id TEXT NOT NULL,
           kind TEXT NOT NULL, detail TEXT NOT NULL, at TEXT NOT NULL
         );
         INSERT INTO access_grants VALUES
           ('old', 'pilot-home', 'ha:light.living', 'power', 1, 'device',
            '2026-09-29T19:00:00.000Z', 'active', 1, 'unlock-old', 'relock-old',
            '2026-09-29T19:00:00.000Z');
         INSERT INTO incidents VALUES
           ('relock_uncertain-old', 'old', 'relock_uncertain', 'detail',
            '2026-09-29T19:30:00.000Z');";

    #[test]
    fn an_older_database_gains_incident_resolution_and_the_grant_commands() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(PREVIOUS_SCHEMA).unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        migrate(&connection).unwrap();
        // Ledger sa otvára pri každom štarte, takže migrácia musí zniesť opakovanie.
        migrate(&connection).unwrap();

        let open: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM incidents WHERE resolved_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(open, 1);
        let commands: Vec<(String, String)> = connection
            .prepare("SELECT kind, command_id FROM grant_commands WHERE decision_id = 'old'")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            commands,
            [
                ("unlock".to_owned(), "unlock-old".to_owned()),
                ("relock".to_owned(), "relock-old".to_owned())
            ]
        );
    }

    #[test]
    fn a_ledger_file_from_the_previous_schema_still_opens() {
        let path =
            std::env::temp_dir().join(format!("genesis-grant-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(PREVIOUS_SCHEMA).unwrap();
        }

        // Otvorenie ledgeru nesmie nad staršou databázou zlyhať, inak sa služba
        // po aktualizácii vôbec nespustí.
        let ledger = Ledger::open(&path).unwrap();
        let grant = get(&ledger, "old").unwrap().unwrap();
        assert_eq!(grant.state, GrantState::Active);
        assert_eq!(open_incidents(&ledger, "old").unwrap().len(), 1);
        assert_eq!(close_attempts(&ledger, "old").unwrap(), 1);
        drop(ledger);
        std::fs::remove_file(path).unwrap();
    }
}
