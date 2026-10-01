//! Hlasový povel: prepis reči → typovaný intent → ten istý execution ledger,
//! ktorým ide panel.
//!
//! Genesis neprijíma zvuk. Rozpoznávanie reči patrí hlasovému rozhraniu — v
//! pilote Home Assistant Assist — a sem prichádza iba text. Prepis sa nikam
//! neukladá ani nezapisuje do logov; uloží sa len so súhlasom používateľa, a aj
//! potom iba k povelu, ktorý sa naozaj vykonal.
//!
//! Prevod textu na intent je deterministický: uzavretý zoznam slov, žiadny
//! model. Čomu Genesis nerozumie jednoznačne, to nevykoná — radšej sa nič
//! nestane, než aby sa pohnulo iné zariadenie, než používateľ myslel.

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    ha::{Device, HaConfig, Inventory},
    ha_command::{self, ExecutionError},
    ledger::{Actor, CommandRequest, Ledger, LedgerError, Snapshot, Status},
};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS voice_commands (
       command_id TEXT PRIMARY KEY REFERENCES commands(command_id),
       household_id TEXT NOT NULL,
       intent TEXT NOT NULL,
       transcript TEXT,
       at TEXT NOT NULL
     );
     CREATE TABLE IF NOT EXISTS voice_confirmations (
       confirmation_id TEXT PRIMARY KEY,
       household_id TEXT NOT NULL,
       actor_id TEXT NOT NULL,
       device_id TEXT NOT NULL,
       value INTEGER NOT NULL,
       idempotency_key TEXT NOT NULL,
       correlation_id TEXT NOT NULL,
       transcript TEXT,
       requested_at TEXT NOT NULL,
       expires_at TEXT NOT NULL,
       used_at TEXT
     );
     CREATE TABLE IF NOT EXISTS voice_audit (
       event_id TEXT PRIMARY KEY,
       household_id TEXT NOT NULL,
       actor_id TEXT NOT NULL,
       decision TEXT NOT NULL,
       reason TEXT NOT NULL,
       device_id TEXT,
       command_id TEXT,
       at TEXT NOT NULL
     );
     CREATE INDEX IF NOT EXISTS voice_audit_recent ON voice_audit(household_id, at);";

/// Povel sa nevykonal, pretože mu Genesis nerozumel jednoznačne.
pub const UNRECOGNISED_ACTION: &str = "unrecognised_action";
/// Povel obsahoval zapnutie aj vypnutie.
pub const CONFLICTING_ACTION: &str = "conflicting_action";
/// Žiadne zariadenie nezodpovedá tomu, čo bolo povedané.
pub const NO_MATCHING_DEVICE: &str = "no_matching_device";
/// Zodpovedá viac zariadení a Genesis si nevybral.
pub const SEVERAL_MATCHING_DEVICES: &str = "several_matching_devices";
/// Citlivá akcia čaká na výslovné potvrdenie.
pub const CONFIRMATION_REQUIRED: &str = "confirmation_required";
/// Potvrdenie Genesis nepozná — alebo patrí inému aktérovi.
pub const UNKNOWN_CONFIRMATION: &str = "unknown_confirmation";
/// Potvrdenie už nie je platné.
pub const EXPIRED_CONFIRMATION: &str = "expired_confirmation";
/// Potvrdenie bolo použité; druhýkrát sa nedá.
pub const CONFIRMATION_ALREADY_USED: &str = "confirmation_already_used";
/// Potvrdenie patrí inému aktérovi. Do odpovede sa nedostane, iba do auditu.
pub const FOREIGN_CONFIRMATION: &str = "foreign_confirmation";
/// Zariadenie nie je v inventári.
pub const DEVICE_UNKNOWN: &str = "device_unknown";
/// Zariadenie povel práve nemôže prijať.
pub const DEVICE_NOT_EXECUTABLE: &str = "device_not_executable";
/// Ledger povel neprijal.
pub const LEDGER_REFUSED: &str = "ledger_refused";
/// Rola volajúceho neovláda zariadenia.
pub const ROLE_NOT_PERMITTED: &str = "role_not_permitted";
/// Požiadavka mierila na inú domácnosť.
pub const OTHER_HOUSEHOLD: &str = "other_household";

/// Predpony Home Assistant entít, ktoré sú citlivé samy o sebe. Pilot mapuje
/// iba `light.` a `switch.`, takže dnes sem nič z inventára nespadne; zoznam je
/// tu preto, aby zámok platil už v momente, keď sa inventár rozšíri.
const SENSITIVE_DOMAINS: [&str; 6] = [
    "lock.",
    "cover.",
    "valve.",
    "water_heater.",
    "climate.",
    "alarm_control_panel.",
];

/// Ako dlho platí potvrdenie. Citlivá akcia nemá čakať na neskôr; kto ju chcel,
/// potvrdí ju hneď.
const CONFIRMATION_TTL_SECONDS: i64 = 120;

/// Koľko záznamov auditu vráti prehľad. Odpoveď nesmie rásť bez hranice.
const AUDIT_LIMIT: i64 = 200;

const DECISION_EXECUTED: &str = "executed";
const DECISION_REFUSED: &str = "refused";
const DECISION_AWAITING: &str = "awaiting_confirmation";

/// Formy, ktoré zapínajú. Zoznam je zámerne uzavretý; rozpoznávanie reči dodáva
/// text, nie význam.
const ON_WORDS: [&str; 8] = [
    "zapni",
    "zapnite",
    "zapnut",
    "rozsviet",
    "rozsvietit",
    "zasviet",
    "zapal",
    "zapalte",
];
const OFF_WORDS: [&str; 7] = [
    "vypni", "vypnite", "vypnut", "zhasni", "zhasnite", "zhasnut", "zhasni",
];
/// Anglické sloveso je dvojslovné, takže sa hľadá ako pár.
const ENGLISH_VERBS: [&str; 2] = ["turn", "switch"];

/// Druhy zariadení, ktoré pilot pozná. Kľúčom je stem slova v povele, hodnotou
/// predpona Home Assistant entity — jediný poskytovateľ pilotu je HA.
const CLASS_WORDS: [(&str, &str); 8] = [
    ("svetl", "light."),
    ("lamp", "light."),
    ("light", "light."),
    ("zasuvk", "switch."),
    ("zastrck", "switch."),
    ("switch", "switch."),
    ("plug", "switch."),
    ("socket", "switch."),
];

/// Slová bez cieľa. Bez nich by zdvorilo vyslovený povel skončil ako
/// nejednoznačný.
const FILLERS: [&str; 9] = [
    "prosim", "please", "the", "mne", "teraz", "hned", "tak", "mi", "uz",
];

/// Typovaný intent. Pilot pozná jedinú akciu, a aj tá nesie konkrétne
/// zariadenie — nie to, čo bolo povedané.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "intent")]
pub enum Intent {
    SetPower { device_id: String, value: bool },
}

/// Prečo sa povel nevykonal.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unclear {
    pub reason: &'static str,
    /// Zariadenia, medzi ktorými sa nedalo rozhodnúť; inak prázdne.
    pub candidates: Vec<String>,
    /// Veta pre používateľa. Hovorí iba to, čo Genesis naozaj vie.
    pub message: &'static str,
}

/// Vysvetlenie výsledku. `code` je pre klienta, `message` pre človeka; ani jedno
/// netvrdí viac, než ledger vie dokázať.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Explanation {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Resolution {
    Resolved(Intent),
    Unclear(Unclear),
}

/// Čo prišlo z hlasového rozhrania.
///
/// Zámerne bez `Debug`: prepis sa nesmie dostať do logu ani omylom.
pub struct Spoken {
    pub transcript: String,
    /// Súhlas používateľa s uložením prepisu. Bez neho sa prepis neuloží.
    pub store_transcript: bool,
    pub idempotency_key: String,
    pub correlation_id: String,
}

/// Výsledok hlasového povelu.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Outcome {
    Executed {
        intent: Intent,
        command: Snapshot,
        /// Rozlišuje odoslanie, potvrdenie poskytovateľom, potvrdenie
        /// zariadením a neistý výsledok.
        explanation: Explanation,
    },
    /// Citlivá akcia. Nič sa nevykonalo a čaká sa na výslovné potvrdenie.
    ConfirmationRequired {
        intent: Intent,
        confirmation_id: String,
        expires_at: String,
        explanation: Explanation,
    },
    Unclear(Unclear),
    /// Povel bol zamietnutý z dôvodu, ktorý nesúvisí s porozumením.
    Refused {
        explanation: Explanation,
    },
}

/// Záznam auditu hlasovej akcie.
///
/// Zámerne neobsahuje prepis ani identifikátor potvrdenia: audit má povedať, čo
/// sa rozhodlo a prečo, nie vyzradiť, čo bolo povedané alebo čím sa potvrdzuje.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AuditEvent {
    pub event_id: String,
    pub household_id: String,
    pub actor_id: String,
    pub decision: String,
    pub reason: String,
    pub device_id: Option<String>,
    pub command_id: Option<String>,
    pub at: String,
}

/// Čo si Genesis nechal o hlasovom povele.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VoiceRecord {
    pub command_id: String,
    pub household_id: String,
    pub intent: Intent,
    /// Prepis je vyplnený len vtedy, keď používateľ dal súhlas.
    pub transcript: Option<String>,
    pub at: String,
}

/// Prevedie prepis na typovaný intent.
///
/// Rozhoduje sa v tomto poradí: podľa názvu zariadenia, potom podľa druhu
/// zariadenia, a keď ani to nie je jednoznačné, nevykoná sa nič.
pub fn resolve(transcript: &str, devices: &[Device]) -> Resolution {
    let words = normalize(transcript);
    let value = match action(&words) {
        Ok(value) => value,
        Err(unclear) => return Resolution::Unclear(unclear),
    };
    let target = target_stems(&words);

    // 1. Podľa názvu: všetky významné slová názvu musia byť v povele. Jedna
    //    zhoda sa vykoná, viac zhôd nie.
    let named: Vec<&Device> = devices
        .iter()
        .filter(|device| name_fully_matches(device, &target))
        .collect();
    match named.as_slice() {
        [device] => return resolved(device, value),
        [] => {}
        several => return Resolution::Unclear(unclear(SEVERAL_MATCHING_DEVICES, several)),
    }

    // 2. Podľa druhu zariadenia, a to len vtedy, keď povel neobsahuje nič iné
    //    než druh. „Zhasni svetlo v spálni" nesmie zhasnúť svetlo v obývačke
    //    len preto, že je to jediné svetlo: spálňu Genesis nepozná, takže
    //    správna odpoveď je nevykonať nič.
    if let Some(prefix) = sole_class(&target) {
        let class: Vec<&Device> = devices
            .iter()
            .filter(|device| device.provider_device_ref.starts_with(prefix))
            .collect();
        match class.as_slice() {
            [device] => return resolved(device, value),
            [] => {}
            several => return Resolution::Unclear(unclear(SEVERAL_MATCHING_DEVICES, several)),
        }
    }

    // 3. Ak povel pripomína viac zariadení, aspoň sa dá povedať, medzi čím sa
    //    Genesis nerozhodol.
    let similar: Vec<&Device> = devices
        .iter()
        .filter(|device| name_partly_matches(device, &target))
        .collect();
    if similar.len() > 1 {
        return Resolution::Unclear(unclear(SEVERAL_MATCHING_DEVICES, &similar));
    }
    Resolution::Unclear(plain_unclear(NO_MATCHING_DEVICE))
}

/// Vykoná hlasový povel tou istou cestou ako panel.
///
/// Nejednoznačný povel sa nedostane k ledgeru vôbec — nevznikne povel, nezapíše
/// sa nič a neuloží sa ani prepis. Citlivá akcia sa nevykoná na prvé slovo;
/// vráti sa identifikátor potvrdenia a čaká sa na `confirm`.
pub async fn execute(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    household_id: &str,
    actor: Actor,
    spoken: &Spoken,
    declared_sensitive: &[String],
    now: DateTime<Utc>,
) -> Result<Outcome, ExecutionError> {
    let devices = inventory.devices().await;
    let intent = match resolve(&spoken.transcript, &devices) {
        Resolution::Resolved(intent) => intent,
        Resolution::Unclear(unclear) => {
            write_audit(
                ledger,
                household_id,
                &actor.actor_id,
                DECISION_REFUSED,
                unclear.reason,
                None,
                None,
                now,
            );
            return Ok(Outcome::Unclear(unclear));
        }
    };
    let Intent::SetPower { device_id, .. } = &intent;

    // Citlivá akcia sa nevykoná na prvé slovo. Genesis ju odloží a čaká, kým ju
    // ten istý oprávnený člen výslovne potvrdí.
    let sensitive = devices
        .iter()
        .find(|device| &device.device_id == device_id)
        .is_some_and(|device| is_sensitive(device, declared_sensitive));
    if sensitive {
        let pending = hold_for_confirmation(ledger, household_id, &actor, &intent, spoken, now)?;
        write_audit(
            ledger,
            household_id,
            &actor.actor_id,
            DECISION_AWAITING,
            CONFIRMATION_REQUIRED,
            Some(device_id),
            None,
            now,
        );
        return Ok(Outcome::ConfirmationRequired {
            intent,
            confirmation_id: pending.confirmation_id,
            expires_at: pending.expires_at,
            explanation: explain_reason(CONFIRMATION_REQUIRED),
        });
    }

    run(
        config,
        inventory,
        ledger,
        household_id,
        actor,
        intent,
        spoken.idempotency_key.clone(),
        spoken.correlation_id.clone(),
        spoken.store_transcript.then(|| spoken.transcript.clone()),
        now,
    )
    .await
}

/// Vykoná potvrdenú citlivú akciu.
///
/// Potvrdenie platí raz, krátko a iba pre toho, kto o akciu požiadal. Čokoľvek
/// iné je zamietnutie, ktoré sa zapíše do auditu presnejšie, než sa povie
/// volajúcemu.
pub async fn confirm(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    household_id: &str,
    actor: Actor,
    confirmation_id: &str,
    now: DateTime<Utc>,
) -> Result<Outcome, ExecutionError> {
    let pending = match claim_confirmation(ledger, household_id, &actor, confirmation_id, now)? {
        Ok(pending) => pending,
        // Audit pozná presný dôvod aj vtedy, keď odpoveď hovorí menej: že
        // potvrdenie existuje, ale patrí niekomu inému, sa volajúci nedozvie.
        Err(refusal) => {
            write_audit(
                ledger,
                household_id,
                &actor.actor_id,
                DECISION_REFUSED,
                refusal.audited,
                None,
                None,
                now,
            );
            return Ok(Outcome::Refused {
                explanation: explain_reason(refusal.told),
            });
        }
    };
    let intent = Intent::SetPower {
        device_id: pending.device_id,
        value: pending.value,
    };
    run(
        config,
        inventory,
        ledger,
        household_id,
        actor,
        intent,
        pending.idempotency_key,
        pending.correlation_id,
        pending.transcript,
        now,
    )
    .await
}

/// Spoločná cesta k vykonaniu: povel, hlasová značka a audit.
#[allow(clippy::too_many_arguments)]
async fn run(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    household_id: &str,
    actor: Actor,
    intent: Intent,
    idempotency_key: String,
    correlation_id: String,
    transcript: Option<String>,
    now: DateTime<Utc>,
) -> Result<Outcome, ExecutionError> {
    let Intent::SetPower { device_id, value } = &intent;
    let actor_id = actor.actor_id.clone();
    let command_id = format!("cmd:{}", Uuid::new_v4());
    let command = match ha_command::run_power_command(
        config,
        inventory,
        ledger,
        CommandRequest {
            household_id: household_id.to_owned(),
            command_id: command_id.clone(),
            device_id: device_id.clone(),
            capability_id: "power".to_owned(),
            value: Value::Bool(*value),
            actor,
            idempotency_key,
            correlation_id,
        },
    )
    .await
    {
        Ok(command) => command,
        Err(error) => {
            let reason = match &error {
                ExecutionError::UnknownDevice => DEVICE_UNKNOWN,
                ExecutionError::NotExecutable => DEVICE_NOT_EXECUTABLE,
                ExecutionError::Ledger(_) => LEDGER_REFUSED,
            };
            write_audit(
                ledger,
                household_id,
                &actor_id,
                DECISION_REFUSED,
                reason,
                Some(device_id),
                None,
                now,
            );
            return Err(error);
        }
    };
    // Idempotency kľúč mohol patriť už existujúcemu povelu — aj takému, ktorý
    // prišiel z panela. Hlasom sa preto označí iba povel, ktorý vznikol týmto
    // povelom; inak by audit tvrdil o panelovom povele, že ho niekto vyslovil.
    if command.request.command_id == command_id {
        record(ledger, &command_id, household_id, &intent, transcript, now)?;
    }
    let explanation = explain_status(&command.status);
    write_audit(
        ledger,
        household_id,
        &actor_id,
        DECISION_EXECUTED,
        explanation.code,
        Some(device_id),
        Some(&command.request.command_id),
        now,
    );
    Ok(Outcome::Executed {
        intent,
        command,
        explanation,
    })
}

/// Je zariadenie citlivé? Rozhoduje o tom prevádzkovateľ, ktorý vie, čo je za
/// zásuvkou, a zoznam vlastne citlivých domén Home Assistanta.
pub fn is_sensitive(device: &Device, declared: &[String]) -> bool {
    declared.iter().any(|entry| entry == &device.device_id)
        || SENSITIVE_DOMAINS
            .iter()
            .any(|domain| device.provider_device_ref.starts_with(domain))
}

/// Odložená citlivá akcia.
struct Pending {
    confirmation_id: String,
    expires_at: String,
    device_id: String,
    value: bool,
    idempotency_key: String,
    correlation_id: String,
    transcript: Option<String>,
}

/// Prečo sa potvrdenie neprijalo: `audited` je presný dôvod, `told` ten, ktorý
/// sa povie volajúcemu.
struct Refusal {
    audited: &'static str,
    told: &'static str,
}

/// Uloží citlivú akciu a vráti identifikátor, ktorým sa dá potvrdiť.
///
/// Identifikátor je náhodný a funguje ako oprávnenie, takže sa nikdy neloguje.
fn hold_for_confirmation(
    ledger: &mut Ledger,
    household_id: &str,
    actor: &Actor,
    intent: &Intent,
    spoken: &Spoken,
    now: DateTime<Utc>,
) -> Result<Pending, ExecutionError> {
    let Intent::SetPower { device_id, value } = intent;
    // Prepis, ktorý už nemá čo potvrdiť, nemá dôvod existovať. Čistí sa pri
    // každej novej citlivej akcii, aby nezostal ležať po nepoužitom potvrdení.
    forget_transcripts(ledger, household_id, now);
    let pending = Pending {
        confirmation_id: Uuid::new_v4().to_string(),
        expires_at: stamp(now + TimeDelta::seconds(CONFIRMATION_TTL_SECONDS)),
        device_id: device_id.clone(),
        value: *value,
        idempotency_key: spoken.idempotency_key.clone(),
        correlation_id: spoken.correlation_id.clone(),
        transcript: spoken.store_transcript.then(|| spoken.transcript.clone()),
    };
    ledger
        .connection()
        .execute(
            "INSERT INTO voice_confirmations
             (confirmation_id, household_id, actor_id, device_id, value, idempotency_key,
              correlation_id, transcript, requested_at, expires_at, used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)",
            params![
                pending.confirmation_id,
                household_id,
                actor.actor_id,
                pending.device_id,
                pending.value,
                pending.idempotency_key,
                pending.correlation_id,
                pending.transcript,
                stamp(now),
                pending.expires_at
            ],
        )
        .map_err(LedgerError::from)?;
    Ok(pending)
}

/// Overí potvrdenie a označí ho za použité. Vráti `Err(Refusal)` ako výsledok,
/// nie ako chybu: zamietnutie je odpoveď, ktorú treba zapísať a povedať.
fn claim_confirmation(
    ledger: &mut Ledger,
    household_id: &str,
    actor: &Actor,
    confirmation_id: &str,
    now: DateTime<Utc>,
) -> Result<Result<Pending, Refusal>, ExecutionError> {
    let row: Option<(
        String,
        String,
        String,
        bool,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
    )> = ledger
        .connection()
        .query_row(
            "SELECT confirmation_id, actor_id, device_id, value, idempotency_key,
                        correlation_id, transcript, expires_at, used_at
                 FROM voice_confirmations WHERE confirmation_id = ?1 AND household_id = ?2",
            params![confirmation_id, household_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(LedgerError::from)?;
    let Some((
        stored_id,
        actor_id,
        device_id,
        value,
        idempotency_key,
        correlation_id,
        transcript,
        expires_at,
        used_at,
    )) = row
    else {
        return Ok(Err(Refusal {
            audited: UNKNOWN_CONFIRMATION,
            told: UNKNOWN_CONFIRMATION,
        }));
    };
    // Identifikátor sa porovnáva v konštantnom čase; je to oprávnenie, nie kľúč
    // záznamu, ktorý by sa smel porovnávať skratkou.
    if stored_id
        .as_bytes()
        .ct_eq(confirmation_id.as_bytes())
        .unwrap_u8()
        != 1
    {
        return Ok(Err(Refusal {
            audited: UNKNOWN_CONFIRMATION,
            told: UNKNOWN_CONFIRMATION,
        }));
    }
    // Že potvrdenie existuje, ale patrí niekomu inému, sa volajúci nedozvie.
    if actor_id != actor.actor_id {
        return Ok(Err(Refusal {
            audited: FOREIGN_CONFIRMATION,
            told: UNKNOWN_CONFIRMATION,
        }));
    }
    if used_at.is_some() {
        return Ok(Err(Refusal {
            audited: CONFIRMATION_ALREADY_USED,
            told: CONFIRMATION_ALREADY_USED,
        }));
    }
    if stamp(now) > expires_at {
        return Ok(Err(Refusal {
            audited: EXPIRED_CONFIRMATION,
            told: EXPIRED_CONFIRMATION,
        }));
    }
    // Označenie za použité ide pred vykonaním: druhý pokus tak nemá čo použiť,
    // aj keby prvý skončil neisto. Cenou je, že po neistom výsledku treba
    // povedať povel znova — to je bezpečnejší smer než opakovateľné potvrdenie.
    ledger
        .connection()
        .execute(
            "UPDATE voice_confirmations SET used_at = ?2
             WHERE confirmation_id = ?1 AND used_at IS NULL",
            params![confirmation_id, stamp(now)],
        )
        .map_err(LedgerError::from)?;
    // Prepis ide odteraz ku vykonanému povelu, takže v potvrdení nemá čo robiť.
    forget_transcripts(ledger, household_id, now);
    Ok(Ok(Pending {
        confirmation_id: stored_id,
        expires_at,
        device_id,
        value,
        idempotency_key,
        correlation_id,
        transcript,
    }))
}

/// Zahodí prepisy z potvrdení, ktoré už nemôžu nič vykonať.
///
/// Riadok zostáva, pretože `used_at` a expirácia sú súčasťou auditu; slová
/// zostať nemusia.
fn forget_transcripts(ledger: &mut Ledger, household_id: &str, now: DateTime<Utc>) {
    let cleaned = ledger.connection().execute(
        "UPDATE voice_confirmations SET transcript = NULL
         WHERE household_id = ?1 AND transcript IS NOT NULL
           AND (used_at IS NOT NULL OR expires_at <= ?2)",
        params![household_id, stamp(now)],
    );
    if let Err(error) = cleaned {
        tracing::error!(%error, "clearing spent voice transcripts failed");
    }
}

/// Zapíše rozhodnutie do auditu.
///
/// Zápis auditu nesmie prebiť to, čo sa volajúcemu deje: keď sa nepodarí, je to
/// prevádzkový problém do logu, nie iná odpoveď.
#[allow(clippy::too_many_arguments)]
fn write_audit(
    ledger: &mut Ledger,
    household_id: &str,
    actor_id: &str,
    decision: &str,
    reason: &str,
    device_id: Option<&str>,
    command_id: Option<&str>,
    now: DateTime<Utc>,
) {
    let written = ledger.connection().execute(
        "INSERT INTO voice_audit
         (event_id, household_id, actor_id, decision, reason, device_id, command_id, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            household_id,
            actor_id,
            decision,
            reason,
            device_id,
            command_id,
            stamp(now)
        ],
    );
    if let Err(error) = written {
        tracing::error!(%error, decision, reason, "voice audit write failed");
    }
}

/// Audit hlasových akcií domácnosti, od najnovšej.
pub fn audit_trail(ledger: &Ledger, household_id: &str) -> Result<Vec<AuditEvent>, ExecutionError> {
    let mut statement = ledger
        .connection()
        .prepare(
            "SELECT event_id, household_id, actor_id, decision, reason, device_id, command_id, at
             FROM voice_audit WHERE household_id = ?1 ORDER BY at DESC, event_id LIMIT ?2",
        )
        .map_err(LedgerError::from)?;
    let rows = statement
        .query_map(params![household_id, AUDIT_LIMIT], |row| {
            Ok(AuditEvent {
                event_id: row.get(0)?,
                household_id: row.get(1)?,
                actor_id: row.get(2)?,
                decision: row.get(3)?,
                reason: row.get(4)?,
                device_id: row.get(5)?,
                command_id: row.get(6)?,
                at: row.get(7)?,
            })
        })
        .map_err(LedgerError::from)?;
    Ok(rows.collect::<Result<_, _>>().map_err(LedgerError::from)?)
}

/// Zaznamená zamietnutie, ktoré padlo ešte pred spracovaním povelu — rola bez
/// oprávnenia alebo cudzia domácnosť.
pub fn record_refusal(
    ledger: &mut Ledger,
    household_id: &str,
    actor_id: &str,
    reason: &'static str,
    now: DateTime<Utc>,
) {
    write_audit(
        ledger,
        household_id,
        actor_id,
        DECISION_REFUSED,
        reason,
        None,
        None,
        now,
    );
}

fn stamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Zapíše, že povel prišiel hlasom. Prepis sa pridá iba so súhlasom, takže
/// audit vie o hlase aj vtedy, keď o slovách vedieť nesmie.
fn record(
    ledger: &mut Ledger,
    command_id: &str,
    household_id: &str,
    intent: &Intent,
    transcript: Option<String>,
    now: DateTime<Utc>,
) -> Result<(), ExecutionError> {
    ledger
        .connection()
        .execute(
            "INSERT OR IGNORE INTO voice_commands
         (command_id, household_id, intent, transcript, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                command_id,
                household_id,
                serde_json::to_string(intent).map_err(LedgerError::from)?,
                transcript,
                now.to_rfc3339_opts(SecondsFormat::Millis, true)
            ],
        )
        .map_err(LedgerError::from)?;
    Ok(())
}

/// Čo si Genesis o hlasovom povele nechal.
pub fn voice_command(
    ledger: &Ledger,
    command_id: &str,
) -> Result<Option<VoiceRecord>, ExecutionError> {
    let row: Option<(String, String, String, Option<String>, String)> = ledger
        .connection()
        .query_row(
            "SELECT command_id, household_id, intent, transcript, at
             FROM voice_commands WHERE command_id = ?1",
            [command_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(LedgerError::from)?;
    let Some((command_id, household_id, intent, transcript, at)) = row else {
        return Ok(None);
    };
    let intent = serde_json::from_str(&intent).map_err(LedgerError::from)?;
    Ok(Some(VoiceRecord {
        command_id,
        household_id,
        intent,
        transcript,
        at,
    }))
}

fn resolved(device: &Device, value: bool) -> Resolution {
    Resolution::Resolved(Intent::SetPower {
        device_id: device.device_id.clone(),
        value,
    })
}

/// Odmietnutie, ktoré neukazuje na konkrétne zariadenia.
fn plain_unclear(reason: &'static str) -> Unclear {
    Unclear {
        reason,
        candidates: Vec::new(),
        message: explain_reason(reason).message,
    }
}

fn unclear(reason: &'static str, devices: &[&Device]) -> Unclear {
    Unclear {
        reason,
        candidates: devices
            .iter()
            .map(|device| device.device_id.clone())
            .collect(),
        message: explain_reason(reason).message,
    }
}

/// Veta k stavu povelu. `sent` nesmie znieť ako hotovo a `unknown` nesmie znieť
/// ako zlyhanie — ani jedno by nebola pravda.
fn explain_status(status: &Status) -> Explanation {
    let message = match status {
        Status::Accepted => "Povel som prijal, ešte som ho neodoslal.",
        Status::Sent => "Povel som odoslal, potvrdenie som ešte nedostal.",
        Status::ProviderConfirmed => {
            "Home Assistant povel prijal, stav zariadenia som ešte neoveril."
        }
        Status::DeviceConfirmed => "Zariadenie je v požadovanom stave.",
        Status::Unknown => "Výsledok neviem potvrdiť, stav zariadenia je neistý.",
        Status::Failed => "Povel sa nevykonal.",
    };
    Explanation {
        code: status_code(status),
        message,
    }
}

fn status_code(status: &Status) -> &'static str {
    match status {
        Status::Accepted => "accepted",
        Status::Sent => "sent",
        Status::ProviderConfirmed => "provider_confirmed",
        Status::DeviceConfirmed => "device_confirmed",
        Status::Unknown => "unknown",
        Status::Failed => "failed",
    }
}

fn explain_reason(reason: &'static str) -> Explanation {
    let message = match reason {
        UNRECOGNISED_ACTION => "Nerozumel som, čo mám urobiť.",
        CONFLICTING_ACTION => "V povele je zapnutie aj vypnutie, tak som neurobil nič.",
        NO_MATCHING_DEVICE => "Také zariadenie nepoznám.",
        SEVERAL_MATCHING_DEVICES => "Nevedel som, ktoré zariadenie máš na mysli.",
        CONFIRMATION_REQUIRED => "Toto je citlivá akcia. Potrebujem potvrdenie.",
        UNKNOWN_CONFIRMATION => "Také potvrdenie nepoznám.",
        EXPIRED_CONFIRMATION => "Potvrdenie už nie je platné.",
        CONFIRMATION_ALREADY_USED => "Toto potvrdenie je už použité.",
        DEVICE_UNKNOWN => "Také zariadenie nepoznám.",
        DEVICE_NOT_EXECUTABLE => "Zariadenie teraz povel prijať nemôže.",
        ROLE_NOT_PERMITTED => "Na ovládanie zariadení nemáš oprávnenie.",
        OTHER_HOUSEHOLD => "To nie je zariadenie tejto domácnosti.",
        _ => "Povel sa nevykonal.",
    };
    Explanation {
        code: reason,
        message,
    }
}

/// Zapnutie alebo vypnutie. Povel, ktorý obsahuje oboje, nie je povel.
fn action(words: &[String]) -> Result<bool, Unclear> {
    let mut on = false;
    let mut off = false;
    for pair in words.windows(2) {
        if ENGLISH_VERBS.contains(&pair[0].as_str()) {
            on |= pair[1] == "on";
            off |= pair[1] == "off";
        }
    }
    for word in words {
        on |= ON_WORDS.contains(&word.as_str());
        off |= OFF_WORDS.contains(&word.as_str());
    }
    match (on, off) {
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        (true, true) => Err(plain_unclear(CONFLICTING_ACTION)),
        (false, false) => Err(plain_unclear(UNRECOGNISED_ACTION)),
    }
}

/// Slová, ktoré môžu určovať cieľ, zredukované na stem.
fn target_stems(words: &[String]) -> Vec<String> {
    words
        .iter()
        .filter(|word| !is_action_word(word) && significant(word))
        .map(|word| stem(word))
        .collect()
}

fn is_action_word(word: &str) -> bool {
    ON_WORDS.contains(&word)
        || OFF_WORDS.contains(&word)
        || ENGLISH_VERBS.contains(&word)
        || word == "on"
        || word == "off"
}

fn significant(word: &str) -> bool {
    word.len() >= 3 && !FILLERS.contains(&word)
}

fn name_fully_matches(device: &Device, target: &[String]) -> bool {
    let name = name_stems(device);
    !name.is_empty() && name.iter().all(|word| target.contains(word))
}

fn name_partly_matches(device: &Device, target: &[String]) -> bool {
    name_stems(device).iter().any(|word| target.contains(word))
}

/// Významné slová názvu zariadenia. Názov dáva Home Assistant, takže je to
/// jediné, čím sa dá zariadenie osloviť menom.
fn name_stems(device: &Device) -> Vec<String> {
    normalize(&device.name)
        .iter()
        .filter(|word| significant(word))
        .map(|word| stem(word))
        .collect()
}

/// Predpona entity, ak je jediným cieľom v povele druh zariadenia.
fn sole_class(target: &[String]) -> Option<&'static str> {
    let mut found = None;
    for word in target {
        let class = CLASS_WORDS
            .iter()
            .find(|(noun, _)| noun == word)
            .map(|(_, prefix)| *prefix);
        match (class, found) {
            // Povel obsahuje aj niečo iné než druh: kde to je, Genesis nevie.
            (None, _) => return None,
            (Some(prefix), None) => found = Some(prefix),
            (Some(prefix), Some(previous)) if prefix == previous => {}
            (Some(_), Some(_)) => return None,
        }
    }
    found
}

/// Malé písmená bez diakritiky, rozdelené na slová. Diakritika sa odstraňuje,
/// pretože rozpoznávanie reči ju vracia nespoľahlivo.
///
/// Malé písmená musia byť prvé a podľa Unicode: `to_ascii_lowercase` by veľké
/// `Č` nechal tak, skladanie diakritiky by ho minulo a znak by vypadol ako
/// oddeľovač — z `OBÝVAČKA` by zostali úlomky.
fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(fold_accent)
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn fold_accent(character: char) -> char {
    match character {
        'á' | 'ä' | 'à' | 'â' => 'a',
        'č' | 'ć' => 'c',
        'ď' => 'd',
        'é' | 'ě' | 'è' | 'ê' => 'e',
        'í' | 'ì' | 'î' => 'i',
        'ľ' | 'ĺ' => 'l',
        'ň' | 'ń' => 'n',
        'ó' | 'ô' | 'ö' | 'ò' => 'o',
        'ŕ' | 'ř' => 'r',
        'š' | 'ś' => 's',
        'ť' => 't',
        'ú' | 'ů' | 'ü' | 'ù' | 'û' => 'u',
        'ý' => 'y',
        'ž' | 'ź' => 'z',
        other => other,
    }
}

/// Slovenčina skloňuje na konci slova, takže sa zahodí jedna koncová
/// samohláska: `obývačke` aj `obývačka` tak vedú na ten istý základ. Kratší
/// základ než štyri znaky by spájal nesúvisiace slová, preto sa nechá tak.
fn stem(word: &str) -> String {
    let trimmed = word.strip_suffix(['a', 'e', 'i', 'o', 'u', 'y']);
    match trimmed {
        Some(shorter) if shorter.len() >= 4 => shorter.to_owned(),
        _ => word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ha::Availability,
        ledger::{ActorType, Status},
    };

    fn device(entity: &str, name: &str) -> Device {
        Device {
            device_id: format!("ha:{entity}"),
            provider: "home_assistant",
            provider_device_ref: entity.to_owned(),
            name: name.to_owned(),
            capability_id: "power",
            capability_type: "switch",
            writable: true,
            power: Some(false),
            observed_at: None,
            availability: Availability::Online,
        }
    }

    fn living() -> Device {
        device("light.living", "Obývačka svetlo")
    }

    fn kitchen() -> Device {
        device("light.kitchen", "Kuchyňa lampa")
    }

    fn plug() -> Device {
        device("switch.plug", "Zásuvka pri stole")
    }

    fn power(device_id: &str, value: bool) -> Resolution {
        Resolution::Resolved(Intent::SetPower {
            device_id: device_id.to_owned(),
            value,
        })
    }

    fn refusal(resolution: &Resolution) -> &Unclear {
        match resolution {
            Resolution::Unclear(unclear) => unclear,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn owner() -> Actor {
        Actor {
            actor_type: ActorType::User,
            actor_id: "pilot-owner".to_owned(),
        }
    }

    fn member() -> Actor {
        Actor {
            actor_type: ActorType::User,
            actor_id: "pilot-member".to_owned(),
        }
    }

    fn spoken(transcript: &str, store_transcript: bool) -> Spoken {
        Spoken {
            transcript: transcript.to_owned(),
            store_transcript,
            idempotency_key: "voice-1".to_owned(),
            correlation_id: "assist-1".to_owned(),
        }
    }

    /// Adresa, na ktorej nikto nepočúva: povel sa zapíše do ledgeru, ale
    /// nedoručí, takže test nepotrebuje Home Assistanta.
    fn unreachable() -> HaConfig {
        HaConfig {
            websocket_url: "ws://127.0.0.1:1/api/websocket".to_owned(),
            token: "unused".to_owned(),
        }
    }

    #[test]
    fn a_named_device_and_an_action_become_a_typed_intent() {
        assert_eq!(
            resolve("Zhasni svetlo v obývačke", &[living(), kitchen()]),
            power("ha:light.living", false)
        );
    }

    #[test]
    fn slovak_inflection_does_not_break_the_name() {
        assert_eq!(
            resolve("Zapni lampu v kuchyni", &[living(), kitchen()]),
            power("ha:light.kitchen", true)
        );
    }

    #[test]
    fn the_only_device_of_its_kind_answers_a_plain_command() {
        assert_eq!(
            resolve("zapni svetlo", &[living()]),
            power("ha:light.living", true)
        );
        assert_eq!(
            resolve("zapni prosím svetlo", &[living()]),
            power("ha:light.living", true)
        );
        assert_eq!(
            resolve("turn off the plug", &[living(), plug()]),
            power("ha:switch.plug", false)
        );
    }

    #[test]
    fn capital_letters_with_diacritics_still_match() {
        assert_eq!(
            resolve("ZHASNI SVETLO V OBÝVAČKE", &[living(), kitchen()]),
            power("ha:light.living", false)
        );
    }

    #[test]
    fn a_bare_verb_does_not_toggle_the_only_device() {
        let resolution = resolve("zapni", &[living()]);
        assert_eq!(refusal(&resolution).reason, NO_MATCHING_DEVICE);
    }

    #[test]
    fn a_room_genesis_does_not_know_is_not_executed() {
        // Svetlo v obývačke je jediné svetlo, ale používateľ hovoril o spálni.
        // Zhasnúť iné svetlo, než myslel, je horšie než nespraviť nič.
        let resolution = resolve("zhasni svetlo v spálni", &[living()]);
        assert_eq!(refusal(&resolution).reason, NO_MATCHING_DEVICE);
    }

    #[test]
    fn several_matching_devices_are_never_executed() {
        let resolution = resolve("zapni svetlo", &[living(), kitchen()]);
        let unclear = refusal(&resolution);
        assert_eq!(unclear.reason, SEVERAL_MATCHING_DEVICES);
        assert_eq!(unclear.candidates, ["ha:light.living", "ha:light.kitchen"]);
    }

    #[test]
    fn an_utterance_without_a_clear_action_is_not_executed() {
        let nothing = resolve("svetlo v obývačke", &[living()]);
        assert_eq!(refusal(&nothing).reason, UNRECOGNISED_ACTION);
        let both = resolve("zapni a vypni svetlo", &[living()]);
        assert_eq!(refusal(&both).reason, CONFLICTING_ACTION);
    }

    #[test]
    fn an_empty_inventory_matches_nothing() {
        let resolution = resolve("zapni svetlo", &[]);
        assert_eq!(refusal(&resolution).reason, NO_MATCHING_DEVICE);
    }

    #[tokio::test]
    async fn a_voice_command_goes_through_the_same_ledger_as_the_panel() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        let inventory = Inventory::new();
        inventory.seed(vec![living()]).await;

        let outcome = execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &spoken("Zhasni svetlo v obývačke", false),
            &[],
            at("2026-09-29T18:30:00Z"),
        )
        .await
        .unwrap();
        let Outcome::Executed {
            intent,
            command,
            explanation,
        } = outcome
        else {
            panic!("a named device and an action must execute");
        };
        // Home Assistant nebol dostupný, takže vysvetlenie to musí priznať.
        assert_eq!(explanation.code, "failed");
        assert_eq!(
            intent,
            Intent::SetPower {
                device_id: "ha:light.living".to_owned(),
                value: false
            }
        );
        // Home Assistant nebol dostupný, takže výsledok je pravdivo `failed`.
        assert_eq!(command.status, Status::Failed);
        let stored = ledger.get(&command.request.command_id).unwrap().unwrap();
        assert_eq!(stored.request.actor, owner());
        assert_eq!(stored.request.idempotency_key, "voice-1");
        assert_eq!(stored.request.value, Value::Bool(false));
        // Audit vie, že povel prišiel hlasom, aj keď slová nepozná.
        let record = voice_command(&ledger, &command.request.command_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.intent, intent);
        assert_eq!(record.transcript, None);
    }

    #[tokio::test]
    async fn an_unclear_command_never_reaches_the_ledger() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        let inventory = Inventory::new();
        inventory.seed(vec![living(), kitchen()]).await;

        let outcome = execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &spoken("zapni svetlo", true),
            &[],
            at("2026-09-29T18:30:00Z"),
        )
        .await
        .unwrap();
        let Outcome::Unclear(unclear) = outcome else {
            panic!("two lights must not be guessed between");
        };
        assert_eq!(unclear.reason, SEVERAL_MATCHING_DEVICES);
        // Žiadny povel, žiadny záznam o hlase a žiadny prepis — ani so súhlasom.
        for table in ["commands", "voice_commands"] {
            let rows: i64 = ledger
                .connection()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 0, "{table} must stay empty");
        }
    }

    #[tokio::test]
    async fn an_existing_command_is_not_relabelled_as_spoken() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        let inventory = Inventory::new();
        inventory.seed(vec![living()]).await;
        // Panel už ten istý zámer pod tým istým kľúčom vykonal.
        let panel = ledger
            .accept(CommandRequest {
                household_id: "pilot-home".to_owned(),
                command_id: "cmd:panel".to_owned(),
                device_id: "ha:light.living".to_owned(),
                capability_id: "power".to_owned(),
                value: Value::Bool(false),
                actor: owner(),
                idempotency_key: "voice-1".to_owned(),
                correlation_id: "panel-1".to_owned(),
            })
            .unwrap();

        let outcome = execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &spoken("Zhasni svetlo v obývačke", true),
            &[],
            at("2026-09-29T18:30:00Z"),
        )
        .await
        .unwrap();
        let Outcome::Executed { command, .. } = outcome else {
            panic!("the intent is clear, so it must resolve");
        };
        // Vráti sa pôvodný povel a audit ho nevydáva za vyslovený.
        assert_eq!(command.request.command_id, panel.request.command_id);
        assert!(voice_command(&ledger, "cmd:panel").unwrap().is_none());
    }

    fn boiler() -> Device {
        device("switch.boiler", "Bojler")
    }

    fn declared() -> Vec<String> {
        vec!["ha:switch.boiler".to_owned()]
    }

    /// Prostredie pre citlivú akciu: bojler v inventári a v zozname citlivých.
    async fn sensitive_setup() -> (Ledger, Inventory) {
        let inventory = Inventory::new();
        inventory.seed(vec![living(), boiler()]).await;
        (Ledger::open(":memory:").unwrap(), inventory)
    }

    fn held_transcripts(ledger: &Ledger) -> i64 {
        ledger
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM voice_confirmations WHERE transcript IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn rows(ledger: &Ledger, table: &str) -> i64 {
        ledger
            .connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[tokio::test]
    async fn a_sensitive_device_is_not_touched_before_it_is_confirmed() {
        let (mut ledger, inventory) = sensitive_setup().await;
        let outcome = execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &spoken("vypni bojler", false),
            &declared(),
            at("2026-09-29T18:30:00Z"),
        )
        .await
        .unwrap();

        let Outcome::ConfirmationRequired {
            intent,
            confirmation_id,
            explanation,
            ..
        } = outcome
        else {
            panic!("a sensitive action must wait for a confirmation");
        };
        assert_eq!(
            intent,
            Intent::SetPower {
                device_id: "ha:switch.boiler".to_owned(),
                value: false
            }
        );
        assert!(!confirmation_id.is_empty());
        assert_eq!(explanation.code, CONFIRMATION_REQUIRED);
        // Nič sa nevykonalo: žiadny povel, žiadna hlasová značka.
        assert_eq!(rows(&ledger, "commands"), 0);
        assert_eq!(rows(&ledger, "voice_commands"), 0);

        let trail = audit_trail(&ledger, "pilot-home").unwrap();
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0].decision, "awaiting_confirmation");
        assert_eq!(trail[0].reason, CONFIRMATION_REQUIRED);
        assert_eq!(trail[0].device_id.as_deref(), Some("ha:switch.boiler"));
        assert_eq!(trail[0].command_id, None);
    }

    #[tokio::test]
    async fn a_confirmed_action_runs_once_and_the_confirmation_cannot_be_replayed() {
        let (mut ledger, inventory) = sensitive_setup().await;
        let requested = at("2026-09-29T18:30:00Z");
        let outcome = execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &spoken("vypni bojler", true),
            &declared(),
            requested,
        )
        .await
        .unwrap();
        let Outcome::ConfirmationRequired {
            confirmation_id, ..
        } = outcome
        else {
            panic!("a sensitive action must wait for a confirmation");
        };

        let confirmed = at("2026-09-29T18:30:30Z");
        let outcome = confirm(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &confirmation_id,
            confirmed,
        )
        .await
        .unwrap();
        let Outcome::Executed {
            intent,
            command,
            explanation,
        } = outcome
        else {
            panic!("a confirmed action must run");
        };
        assert_eq!(
            intent,
            Intent::SetPower {
                device_id: "ha:switch.boiler".to_owned(),
                value: false
            }
        );
        // Home Assistant nebol dostupný, takže vysvetlenie priznáva zlyhanie.
        assert_eq!(command.status, Status::Failed);
        assert_eq!(explanation.code, "failed");
        // Súhlas prežil potvrdenie: prepis sa uložil až k vykonanému povelu.
        let stored = voice_command(&ledger, &command.request.command_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.transcript.as_deref(), Some("vypni bojler"));

        // Druhé použitie toho istého potvrdenia už nič nevykoná.
        let outcome = confirm(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &confirmation_id,
            at("2026-09-29T18:30:40Z"),
        )
        .await
        .unwrap();
        let Outcome::Refused { explanation } = outcome else {
            panic!("a used confirmation must be refused");
        };
        assert_eq!(explanation.code, CONFIRMATION_ALREADY_USED);
        assert_eq!(rows(&ledger, "commands"), 1);
    }

    #[tokio::test]
    async fn a_confirmation_expires_and_a_foreign_one_is_never_admitted() {
        let (mut ledger, inventory) = sensitive_setup().await;
        let requested = at("2026-09-29T18:30:00Z");
        let outcome = execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &spoken("vypni bojler", false),
            &declared(),
            requested,
        )
        .await
        .unwrap();
        let Outcome::ConfirmationRequired {
            confirmation_id, ..
        } = outcome
        else {
            panic!("a sensitive action must wait for a confirmation");
        };

        // Iný aktér potvrdenie nepoužije a nedozvie sa, že vôbec existuje.
        let outcome = confirm(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            member(),
            &confirmation_id,
            at("2026-09-29T18:30:10Z"),
        )
        .await
        .unwrap();
        let Outcome::Refused { explanation } = outcome else {
            panic!("another actor must not confirm");
        };
        assert_eq!(explanation.code, UNKNOWN_CONFIRMATION);
        // Audit je presnejší než odpoveď.
        let trail = audit_trail(&ledger, "pilot-home").unwrap();
        assert!(trail
            .iter()
            .any(|event| event.reason == FOREIGN_CONFIRMATION));
        assert!(!trail
            .iter()
            .any(|event| event.reason == UNKNOWN_CONFIRMATION));

        // Cudzí pokus potvrdenie nespotreboval, ale platnosť uplynie.
        let outcome = confirm(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &confirmation_id,
            requested + chrono::TimeDelta::seconds(CONFIRMATION_TTL_SECONDS + 1),
        )
        .await
        .unwrap();
        let Outcome::Refused { explanation } = outcome else {
            panic!("an expired confirmation must be refused");
        };
        assert_eq!(explanation.code, EXPIRED_CONFIRMATION);
        assert_eq!(rows(&ledger, "commands"), 0);

        // Neznámy identifikátor nikam nevedie.
        let outcome = confirm(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            "5cf1d9f4-0000-4000-8000-000000000000",
            at("2026-09-29T18:31:00Z"),
        )
        .await
        .unwrap();
        let Outcome::Refused { explanation } = outcome else {
            panic!("an unknown confirmation must be refused");
        };
        assert_eq!(explanation.code, UNKNOWN_CONFIRMATION);
    }

    #[tokio::test]
    async fn a_transcript_does_not_outlive_the_confirmation_that_held_it() {
        let (mut ledger, inventory) = sensitive_setup().await;
        let requested = at("2026-09-29T18:30:00Z");
        for consent in [true, false] {
            execute(
                &unreachable(),
                &inventory,
                &mut ledger,
                "pilot-home",
                owner(),
                &spoken("vypni bojler", consent),
                &declared(),
                requested,
            )
            .await
            .unwrap();
        }
        // So súhlasom prepis drží iba to potvrdenie, ktoré ešte môže niečo vykonať.
        assert_eq!(held_transcripts(&ledger), 1);

        // Po expirácii nemá čo potvrdiť, takže slová sa zahodia. Vyvolá to ďalšia
        // citlivá akcia, ktorá je zároveň jediným miestom, kde na to príde reč.
        execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &spoken("vypni bojler", false),
            &declared(),
            requested + chrono::TimeDelta::seconds(CONFIRMATION_TTL_SECONDS + 1),
        )
        .await
        .unwrap();
        assert_eq!(held_transcripts(&ledger), 0);
    }

    #[tokio::test]
    async fn an_ordinary_device_still_runs_without_a_confirmation() {
        let (mut ledger, inventory) = sensitive_setup().await;
        let outcome = execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            &spoken("zhasni svetlo v obývačke", false),
            &declared(),
            at("2026-09-29T18:30:00Z"),
        )
        .await
        .unwrap();
        assert!(matches!(outcome, Outcome::Executed { .. }));
        assert_eq!(rows(&ledger, "voice_confirmations"), 0);
    }

    #[test]
    fn a_sensitive_home_assistant_domain_needs_no_declaration() {
        assert!(is_sensitive(&device("lock.front", "Vchod"), &[]));
        assert!(is_sensitive(&device("cover.garage", "Garáž"), &[]));
        // Svetlo citlivé nie je, kým to prevádzkovateľ nepovie.
        assert!(!is_sensitive(&living(), &[]));
        assert!(is_sensitive(&boiler(), &declared()));
        assert!(!is_sensitive(&boiler(), &[]));
    }

    #[test]
    fn the_outcome_names_itself_on_the_wire() {
        let unclear =
            serde_json::to_value(Outcome::Unclear(plain_unclear(NO_MATCHING_DEVICE))).unwrap();
        assert_eq!(unclear["outcome"], "unclear");
        assert_eq!(unclear["reason"], NO_MATCHING_DEVICE);
        assert!(unclear["message"].is_string());

        let refused = serde_json::to_value(Outcome::Refused {
            explanation: explain_reason(EXPIRED_CONFIRMATION),
        })
        .unwrap();
        assert_eq!(refused["outcome"], "refused");
        assert_eq!(refused["explanation"]["code"], EXPIRED_CONFIRMATION);

        // Citlivá akcia nesie intent a identifikátor potvrdenia, a **žiadny
        // povel**: nič sa nevykonalo. Tvar intentu je pripnutý preto, že na ňom
        // stojí panel (ELYSIUM-358) — `intent` je vnorený objekt s vlastným
        // diskriminátorom, nie reťazec, a zmena by sa inak ukázala až v UI.
        let awaiting = serde_json::to_value(Outcome::ConfirmationRequired {
            intent: Intent::SetPower {
                device_id: "ha:lock.front".to_owned(),
                value: false,
            },
            confirmation_id: "7f3a".to_owned(),
            expires_at: "2026-09-29T18:32:00.000Z".to_owned(),
            explanation: explain_reason(CONFIRMATION_REQUIRED),
        })
        .unwrap();
        assert_eq!(awaiting["outcome"], "confirmation_required");
        assert_eq!(awaiting["intent"]["intent"], "set_power");
        assert_eq!(awaiting["intent"]["device_id"], "ha:lock.front");
        assert_eq!(awaiting["intent"]["value"], false);
        assert_eq!(awaiting["confirmation_id"], "7f3a");
        assert_eq!(awaiting["explanation"]["code"], CONFIRMATION_REQUIRED);
        assert!(awaiting["command"].is_null());
    }

    #[test]
    fn the_explanation_separates_sending_from_confirming() {
        let sent = explain_status(&Status::Sent);
        let provider = explain_status(&Status::ProviderConfirmed);
        let device = explain_status(&Status::DeviceConfirmed);
        let unknown = explain_status(&Status::Unknown);
        assert_eq!(sent.code, "sent");
        assert_eq!(provider.code, "provider_confirmed");
        assert_eq!(device.code, "device_confirmed");
        assert_eq!(unknown.code, "unknown");
        // Odoslanie, potvrdenie poskytovateľom a potvrdenie zariadením nesmú
        // znieť rovnako, a neistý výsledok nesmie znieť ako hotovo.
        for pair in [
            (sent.message, provider.message),
            (provider.message, device.message),
            (unknown.message, device.message),
        ] {
            assert_ne!(pair.0, pair.1);
        }
        assert_eq!(explain_status(&Status::Failed).code, "failed");
    }

    #[tokio::test]
    async fn the_audit_records_the_refusal_without_the_words() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        let inventory = Inventory::new();
        inventory.seed(vec![living(), kitchen()]).await;
        let outcome = execute(
            &unreachable(),
            &inventory,
            &mut ledger,
            "pilot-home",
            owner(),
            // Súhlas je daný, a aj tak sa z odmietnutého povelu nič neuloží.
            &spoken("zapni svetlo", true),
            &[],
            at("2026-09-29T18:30:00Z"),
        )
        .await
        .unwrap();
        assert!(matches!(outcome, Outcome::Unclear(_)));

        let trail = audit_trail(&ledger, "pilot-home").unwrap();
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0].decision, "refused");
        assert_eq!(trail[0].reason, SEVERAL_MATCHING_DEVICES);
        assert_eq!(trail[0].actor_id, "pilot-owner");
        // Nič v zázname nesmie nesť to, čo bolo povedané.
        let serialized = serde_json::to_string(&trail[0]).unwrap();
        assert!(
            !serialized.contains("svetlo"),
            "audit leaked the transcript"
        );
        assert_eq!(rows(&ledger, "voice_commands"), 0);
        assert!(audit_trail(&ledger, "other-home").unwrap().is_empty());
    }

    #[test]
    fn a_transcript_is_stored_only_with_consent() {
        for consent in [false, true] {
            let mut ledger = Ledger::open(":memory:").unwrap();
            let command = ledger
                .accept(CommandRequest {
                    household_id: "pilot-home".to_owned(),
                    command_id: "cmd:1".to_owned(),
                    device_id: "ha:light.living".to_owned(),
                    capability_id: "power".to_owned(),
                    value: Value::Bool(true),
                    actor: owner(),
                    idempotency_key: "voice-1".to_owned(),
                    correlation_id: "assist-1".to_owned(),
                })
                .unwrap();
            let intent = Intent::SetPower {
                device_id: "ha:light.living".to_owned(),
                value: true,
            };
            let spoken = spoken("zapni svetlo", consent);
            record(
                &mut ledger,
                &command.request.command_id,
                "pilot-home",
                &intent,
                spoken.store_transcript.then(|| spoken.transcript.clone()),
                at("2026-09-29T18:30:00Z"),
            )
            .unwrap();

            let stored = voice_command(&ledger, "cmd:1").unwrap().unwrap();
            assert_eq!(stored.intent, intent);
            assert_eq!(
                stored.transcript,
                consent.then(|| "zapni svetlo".to_owned()),
                "consent {consent} stored the wrong transcript"
            );
        }
    }
}
