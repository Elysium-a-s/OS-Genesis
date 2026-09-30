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

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ha::{Device, HaConfig, Inventory},
    ha_command::{self, ExecutionError},
    ledger::{Actor, CommandRequest, Ledger, LedgerError, Snapshot},
};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS voice_commands (
       command_id TEXT PRIMARY KEY REFERENCES commands(command_id),
       household_id TEXT NOT NULL,
       intent TEXT NOT NULL,
       transcript TEXT,
       at TEXT NOT NULL
     );";

/// Povel sa nevykonal, pretože mu Genesis nerozumel jednoznačne.
pub const UNRECOGNISED_ACTION: &str = "unrecognised_action";
/// Povel obsahoval zapnutie aj vypnutie.
pub const CONFLICTING_ACTION: &str = "conflicting_action";
/// Žiadne zariadenie nezodpovedá tomu, čo bolo povedané.
pub const NO_MATCHING_DEVICE: &str = "no_matching_device";
/// Zodpovedá viac zariadení a Genesis si nevybral.
pub const SEVERAL_MATCHING_DEVICES: &str = "several_matching_devices";

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
pub enum Outcome {
    Executed { intent: Intent, command: Snapshot },
    Unclear(Unclear),
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
    Resolution::Unclear(Unclear {
        reason: NO_MATCHING_DEVICE,
        candidates: Vec::new(),
    })
}

/// Vykoná hlasový povel tou istou cestou ako panel.
///
/// Nejednoznačný povel sa nedostane k ledgeru vôbec — nevznikne povel, nezapíše
/// sa nič a neuloží sa ani prepis.
pub async fn execute(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    household_id: &str,
    actor: Actor,
    spoken: &Spoken,
    now: DateTime<Utc>,
) -> Result<Outcome, ExecutionError> {
    let intent = match resolve(&spoken.transcript, &inventory.devices().await) {
        Resolution::Resolved(intent) => intent,
        Resolution::Unclear(unclear) => return Ok(Outcome::Unclear(unclear)),
    };
    let Intent::SetPower { device_id, value } = &intent;
    let command_id = format!("cmd:{}", Uuid::new_v4());
    let command = ha_command::run_power_command(
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
            idempotency_key: spoken.idempotency_key.clone(),
            correlation_id: spoken.correlation_id.clone(),
        },
    )
    .await?;
    // Idempotency kľúč mohol patriť už existujúcemu povelu — aj takému, ktorý
    // prišiel z panela. Hlasom sa preto označí iba povel, ktorý vznikol týmto
    // povelom; inak by audit tvrdil o panelovom povele, že ho niekto vyslovil.
    if command.request.command_id == command_id {
        record(ledger, &command_id, household_id, &intent, spoken, now)?;
    }
    Ok(Outcome::Executed { intent, command })
}

/// Zapíše, že povel prišiel hlasom. Prepis sa pridá iba so súhlasom, takže
/// audit vie o hlase aj vtedy, keď o slovách vedieť nesmie.
fn record(
    ledger: &mut Ledger,
    command_id: &str,
    household_id: &str,
    intent: &Intent,
    spoken: &Spoken,
    now: DateTime<Utc>,
) -> Result<(), ExecutionError> {
    let transcript = spoken.store_transcript.then(|| spoken.transcript.clone());
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

fn unclear(reason: &'static str, devices: &[&Device]) -> Unclear {
    Unclear {
        reason,
        candidates: devices
            .iter()
            .map(|device| device.device_id.clone())
            .collect(),
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
        (true, true) => Err(Unclear {
            reason: CONFLICTING_ACTION,
            candidates: Vec::new(),
        }),
        (false, false) => Err(Unclear {
            reason: UNRECOGNISED_ACTION,
            candidates: Vec::new(),
        }),
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
            at("2026-09-29T18:30:00Z"),
        )
        .await
        .unwrap();
        let Outcome::Executed { intent, command } = outcome else {
            panic!("a named device and an action must execute");
        };
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
            record(
                &mut ledger,
                &command.request.command_id,
                "pilot-home",
                &intent,
                &spoken("zapni svetlo", consent),
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
