use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{
    behavior_decision::BehaviorDecision,
    grant::{self, Grant, GrantError, GrantState, RelockStep},
    ha::{Availability, Device, HaConfig, Inventory},
    ledger::{
        Actor, ActorType, CommandRequest, Evidence, EvidenceKind, Ledger, LedgerError, Snapshot,
        Status,
    },
};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug)]
enum CommandError {
    BeforeSend,
    AfterSend,
    Rejected,
}

pub async fn execute(
    config: &HaConfig,
    device: &Device,
    desired: bool,
    ledger: &mut Ledger,
    command_id: &str,
    actor: Actor,
) -> Result<Snapshot, LedgerError> {
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        run_command(config, device, desired, ledger, command_id, &actor),
    )
    .await;
    let current = ledger.get(command_id)?.ok_or(LedgerError::NotFound)?;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(CommandError::BeforeSend)) => {
            ledger.transition(
                command_id,
                Status::Failed,
                actor,
                None,
                Some("ha_unavailable".into()),
            )?;
        }
        Ok(Err(CommandError::Rejected)) => {
            ledger.transition(
                command_id,
                Status::Failed,
                actor,
                None,
                Some("provider_rejected".into()),
            )?;
        }
        Ok(Err(CommandError::AfterSend)) | Err(_) => {
            if current.status != Status::DeviceConfirmed {
                ledger.transition(
                    command_id,
                    Status::Unknown,
                    actor,
                    None,
                    Some("result_unverified".into()),
                )?;
            }
        }
    }
    ledger.get(command_id)?.ok_or(LedgerError::NotFound)
}

async fn run_command(
    config: &HaConfig,
    device: &Device,
    desired: bool,
    ledger: &mut Ledger,
    command_id: &str,
    actor: &Actor,
) -> Result<(), CommandError> {
    let (mut socket, _) = connect_async(&config.websocket_url)
        .await
        .map_err(|_| CommandError::BeforeSend)?;
    let greeting = next_json(&mut socket)
        .await
        .map_err(|_| CommandError::BeforeSend)?;
    if greeting["type"] != "auth_required" {
        return Err(CommandError::BeforeSend);
    }
    send_json(
        &mut socket,
        json!({"type":"auth","access_token":config.token}),
    )
    .await
    .map_err(|_| CommandError::BeforeSend)?;
    let auth = next_json(&mut socket)
        .await
        .map_err(|_| CommandError::BeforeSend)?;
    if auth["type"] != "auth_ok" {
        return Err(CommandError::BeforeSend);
    }
    send_json(
        &mut socket,
        json!({"id":1,"type":"subscribe_events","event_type":"state_changed"}),
    )
    .await
    .map_err(|_| CommandError::BeforeSend)?;
    let sub = next_json(&mut socket)
        .await
        .map_err(|_| CommandError::BeforeSend)?;
    if sub["type"] != "result" || sub["id"] != 1 || sub["success"] != true {
        return Err(CommandError::BeforeSend);
    }

    let domain = device
        .provider_device_ref
        .split('.')
        .next()
        .ok_or(CommandError::BeforeSend)?;
    if domain != "light" && domain != "switch" {
        return Err(CommandError::BeforeSend);
    }
    let service = if desired { "turn_on" } else { "turn_off" };
    send_json(
        &mut socket,
        json!({
            "id":2,
            "type":"call_service",
            "domain":domain,
            "service":service,
            "target":{"entity_id":device.provider_device_ref}
        }),
    )
    .await
    .map_err(|_| CommandError::AfterSend)?;
    ledger
        .transition(command_id, Status::Sent, actor.clone(), None, None)
        .map_err(|_| CommandError::AfterSend)?;

    let mut matching_events = Vec::new();
    let mut provider_context: Option<String> = None;
    loop {
        let message = next_json(&mut socket)
            .await
            .map_err(|_| CommandError::AfterSend)?;
        if message["type"] == "event" && message["id"] == 1 {
            if matching_state_event(&message, &device.provider_device_ref, desired) {
                if let Some(context) = event_context(&message) {
                    if provider_context.as_deref() == Some(context) {
                        ledger
                            .transition(
                                command_id,
                                Status::DeviceConfirmed,
                                actor.clone(),
                                Some(evidence(EvidenceKind::DeviceObservation, context)),
                                None,
                            )
                            .map_err(|_| CommandError::AfterSend)?;
                        return Ok(());
                    }
                    matching_events.push(context.to_owned());
                }
            }
        } else if message["type"] == "result" && message["id"] == 2 {
            if message["success"] != true {
                return Err(CommandError::Rejected);
            }
            let context = message["result"]["context"]["id"]
                .as_str()
                .filter(|id| valid_reference(id))
                .ok_or(CommandError::AfterSend)?;
            ledger
                .transition(
                    command_id,
                    Status::ProviderConfirmed,
                    actor.clone(),
                    Some(evidence(EvidenceKind::ProviderAck, context)),
                    None,
                )
                .map_err(|_| CommandError::AfterSend)?;
            provider_context = Some(context.to_owned());
            if matching_events.iter().any(|value| value == context) {
                ledger
                    .transition(
                        command_id,
                        Status::DeviceConfirmed,
                        actor.clone(),
                        Some(evidence(EvidenceKind::DeviceObservation, context)),
                        None,
                    )
                    .map_err(|_| CommandError::AfterSend)?;
                return Ok(());
            }
        }
    }
}

fn matching_state_event(message: &Value, entity_id: &str, desired: bool) -> bool {
    let data = &message["event"]["data"];
    data["entity_id"] == entity_id
        && data["new_state"]["state"] == if desired { "on" } else { "off" }
}

fn event_context(message: &Value) -> Option<&str> {
    message["event"]["context"]["id"]
        .as_str()
        .or_else(|| message["event"]["data"]["new_state"]["context"]["id"].as_str())
        .filter(|id| valid_reference(id))
}

fn valid_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

fn evidence(kind: EvidenceKind, reference: &str) -> Evidence {
    Evidence {
        kind,
        reference: reference.to_owned(),
        at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    }
}

async fn next_json(socket: &mut Socket) -> Result<Value, ()> {
    loop {
        let frame = socket.next().await.ok_or(())?.map_err(|_| ())?;
        match frame {
            Message::Text(text) => return serde_json::from_str(&text).map_err(|_| ()),
            Message::Close(_) => return Err(()),
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }
}

async fn send_json(socket: &mut Socket, value: Value) -> Result<(), ()> {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|_| ())
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("the device is not in the inventory")]
    UnknownDevice,
    #[error("the device cannot take a command right now")]
    NotExecutable,
    #[error("ledger error: {0}")]
    Ledger(#[from] LedgerError),
}

/// Jediná cesta k vykonaniu povelu na zariadení.
///
/// Panel aj hlas ňou prechádzajú, takže kontrola vykonateľnosti, idempotencia,
/// ledger a dôkazy sú pre oboch rovnaké. Aktéra určuje volajúci server, nikdy
/// nie telo požiadavky.
pub async fn run_power_command(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    request: CommandRequest,
) -> Result<Snapshot, ExecutionError> {
    let device = inventory
        .devices()
        .await
        .into_iter()
        .find(|item| item.device_id == request.device_id)
        .ok_or(ExecutionError::UnknownDevice)?;
    // Neznáme zariadenie a zariadenie, ktoré povel práve nemôže prijať, sú pre
    // volajúceho dve rôzne odpovede.
    if !device.writable || device.availability != Availability::Online {
        return Err(ExecutionError::NotExecutable);
    }
    let desired = request
        .value
        .as_bool()
        .ok_or(LedgerError::Invalid("value"))?;
    let command_id = request.command_id.clone();
    let actor = request.actor.clone();
    let snapshot = ledger.accept(request)?;
    // Duplicitný zámer vráti pôvodný povel a druhýkrát sa neodosiela.
    if snapshot.request.command_id != command_id {
        return Ok(snapshot);
    }
    Ok(execute(config, &device, desired, ledger, &command_id, actor).await?)
}

/// Vykoná rozhodnutie Behavior enginu ako časovo obmedzený unlock.
///
/// Opakované doručenie toho istého rozhodnutia nevytvorí druhý povel — grant aj
/// ledger sú kľúčované idempotency kľúčom z rozhodnutia.
pub async fn apply_decision(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    decision: &BehaviorDecision,
    now: DateTime<Utc>,
) -> Result<Grant, GrantError> {
    let grant = grant::open(ledger, decision, now)?;
    if grant.state != GrantState::Granted {
        return Ok(grant);
    }
    // Grant v stave `Granted` s povelom, ktorý už výsledok má, znamená, že sa
    // unlock vykonal, ale do grantu sa to nezapísalo — napríklad keď jednotka
    // spadla medzi vykonaním a zápisom. Druhý povel sa v takom prípade
    // neposiela: na zámku to nie je no-op, je to druhá zmena fyzického stavu.
    // Grant sa dorovná z toho, čo v ledgeri naozaj je.
    if let Some(settled) = ledger
        .get(&grant.unlock_command_id)?
        .filter(|snapshot| snapshot.status != Status::Accepted)
    {
        return grant::settle_unlock(ledger, &grant.decision_id, &settled, now);
    }
    let outcome = drive(
        config,
        inventory,
        ledger,
        &grant.device_id,
        grant.granted_value,
        &grant.unlock_command_id,
    )
    .await?;
    grant::settle_unlock(ledger, &grant.decision_id, &outcome, now)
}

/// Uzavrie prístup na základe rozhodnutia `revert`: zmazaný cieľ alebo override.
///
/// Jeden povel uzatvára všetky granty, ktoré na danom zariadení a schopnosti
/// ešte môžu byť otvorené. Prázdny výsledok znamená, že Genesis na tom
/// zariadení nemá čo uzavrieť.
pub async fn withdraw_decision(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    decision: &BehaviorDecision,
    now: DateTime<Utc>,
) -> Result<Vec<Grant>, GrantError> {
    let Some(withdrawal) = grant::begin_withdrawal(ledger, decision, now)? else {
        return Ok(Vec::new());
    };
    let outcome = if withdrawal.accepted.status == Status::Accepted {
        drive(
            config,
            inventory,
            ledger,
            &withdrawal.device_id,
            withdrawal.value,
            &withdrawal.command_id,
        )
        .await?
    } else {
        withdrawal.accepted.clone()
    };
    grant::settle_withdrawal(ledger, &withdrawal, &outcome, now)
}

/// Jeden prechod zosúladenia fyzického stavu.
///
/// Splatné granty aj neisté relocky idú tou istou cestou; líšia sa len tým, čo
/// ich do prechodu vybralo. Prechod najprv prevezme granty, ktoré zostali bez
/// výsledku, takže rovnaký kód dorovnáva stav po reštarte aj po chybe za behu.
pub async fn reconcile(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    now: DateTime<Utc>,
) -> Result<Vec<Grant>, GrantError> {
    // Povel sa odosiela iba pod zámkom ledgeru, ktorý tento prechod drží, takže
    // čokoľvek bez výsledku ho už nedostane. Prevzatie tu zachytí aj grant,
    // ktorý zostal otvorený pre chybu medzi prijatím unlocku a jeho výsledkom.
    let mut settled = grant::resume(ledger, now)?;
    let mut queue = grant::due(ledger, now)?;
    queue.extend(grant::pending(ledger)?);
    for grant in queue {
        if let Some(closed) = close_grant(config, inventory, ledger, &grant, now).await? {
            settled.push(closed);
        }
    }
    Ok(settled)
}

/// Uzavretie jedného grantu. `None` znamená, že sa v tomto prechode nič
/// nezmenilo a grant čaká ďalej.
async fn close_grant(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    grant: &Grant,
    now: DateTime<Utc>,
) -> Result<Option<Grant>, GrantError> {
    let closing = grant.closing_value();
    // Prvé uzavretie sa zapíše vždy, aj keď zariadenie nie je dostupné — inak by
    // o možnom otvorenom prístupe nič nesvedčilo. Opakovaný pokus na nedostupné
    // zariadenie by pridal len ďalšie zlyhanie, takže sa neposiela a pokus sa
    // nepočíta; incident zostáva otvorený.
    if grant.state == GrantState::RelockPending
        && writable_device(inventory, &grant.device_id).await.is_none()
    {
        return Ok(None);
    }
    // Pozorovanie je lacnejšie aj bezpečnejšie než ďalší povel: keď zariadenie
    // už vidíme v uzatváracej hodnote, výsledok zosúladí dôkaz.
    if let Some(observed_at) = observed_value(inventory, &grant.device_id, closing).await {
        if let Some(closed) =
            grant::close_from_observation(ledger, &grant.decision_id, &observed_at, now)?
        {
            return Ok(Some(closed));
        }
    }
    match grant::begin_relock(ledger, &grant.decision_id, now)? {
        RelockStep::Settled(settled) => Ok(Some(settled)),
        RelockStep::Wait(_) => Ok(None),
        RelockStep::Send(accepted) => {
            // Ak predchádzajúci prechod spadol až po odoslaní, povel je už
            // uzavretý a druhýkrát sa neposiela; iba sa dopíše výsledok.
            let outcome = if accepted.status == Status::Accepted {
                drive(
                    config,
                    inventory,
                    ledger,
                    &grant.device_id,
                    closing,
                    &accepted.request.command_id,
                )
                .await?
            } else {
                accepted
            };
            Ok(Some(grant::settle_relock(
                ledger,
                &grant.decision_id,
                &outcome,
                now,
            )?))
        }
    }
}

async fn drive(
    config: &HaConfig,
    inventory: &Inventory,
    ledger: &mut Ledger,
    device_id: &str,
    desired: bool,
    command_id: &str,
) -> Result<Snapshot, LedgerError> {
    let actor = Actor {
        actor_type: ActorType::Automation,
        actor_id: "genesis-core".to_owned(),
    };
    match writable_device(inventory, device_id).await {
        Some(device) => execute(config, &device, desired, ledger, command_id, actor).await,
        None => ledger.transition(
            command_id,
            Status::Failed,
            actor,
            None,
            Some("device_unavailable".to_owned()),
        ),
    }
}

async fn writable_device(inventory: &Inventory, device_id: &str) -> Option<Device> {
    inventory.devices().await.into_iter().find(|device| {
        device.device_id == device_id
            && device.writable
            && device.availability == Availability::Online
    })
}

/// Čas pozorovania, ak inventár vidí zariadenie online v danej hodnote.
async fn observed_value(inventory: &Inventory, device_id: &str, value: bool) -> Option<String> {
    inventory
        .devices()
        .await
        .into_iter()
        .find(|device| {
            device.device_id == device_id
                && device.availability == Availability::Online
                && device.power == Some(value)
        })
        .and_then(|device| device.observed_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn service_ack_and_correlated_event_confirm_device() {
        use crate::ha::Availability;
        use crate::ledger::{ActorType, CommandRequest};
        use tokio_tungstenite::accept_async;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            socket
                .send(Message::Text(
                    json!({"type":"auth_required"}).to_string().into(),
                ))
                .await
                .unwrap();
            let auth = socket.next().await.unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(auth.to_text().unwrap()).unwrap()["type"],
                "auth"
            );
            socket
                .send(Message::Text(json!({"type":"auth_ok"}).to_string().into()))
                .await
                .unwrap();
            let sub = socket.next().await.unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(sub.to_text().unwrap()).unwrap()["type"],
                "subscribe_events"
            );
            socket
                .send(Message::Text(
                    json!({"id":1,"type":"result","success":true})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let command = socket.next().await.unwrap().unwrap();
            let command: Value = serde_json::from_str(command.to_text().unwrap()).unwrap();
            assert_eq!(command["type"], "call_service");
            assert_eq!(command["domain"], "light");
            assert_eq!(command["service"], "turn_on");
            assert_eq!(command["target"]["entity_id"], "light.living");
            socket.send(Message::Text(json!({"id":1,"type":"event","event":{"context":{"id":"ctx-1"},"data":{"entity_id":"light.living","new_state":{"state":"on"}}}}).to_string().into())).await.unwrap();
            socket.send(Message::Text(json!({"id":2,"type":"result","success":true,"result":{"context":{"id":"ctx-1"},"response":null}}).to_string().into())).await.unwrap();
        });
        let actor = Actor {
            actor_type: ActorType::User,
            actor_id: "pilot-owner".into(),
        };
        let mut ledger = Ledger::open(":memory:").unwrap();
        ledger
            .accept(CommandRequest {
                household_id: "pilot-home".into(),
                command_id: "cmd-1".into(),
                device_id: "ha:light.living".into(),
                capability_id: "power".into(),
                value: Value::Bool(true),
                actor: actor.clone(),
                idempotency_key: "idem-1".into(),
                correlation_id: "corr-1".into(),
            })
            .unwrap();
        let device = Device {
            device_id: "ha:light.living".into(),
            provider: "home_assistant",
            provider_device_ref: "light.living".into(),
            name: "Living".into(),
            capability_id: "power",
            capability_type: "switch",
            writable: true,
            power: Some(false),
            observed_at: None,
            availability: Availability::Online,
        };
        let config = HaConfig {
            websocket_url: format!("ws://{address}/api/websocket"),
            token: "test-secret".into(),
        };
        let snapshot = execute(&config, &device, true, &mut ledger, "cmd-1", actor)
            .await
            .unwrap();
        assert_eq!(snapshot.status, Status::DeviceConfirmed);
        assert_eq!(ledger.events("cmd-1").unwrap().len(), 4);
        server.await.unwrap();
    }

    #[test]
    fn only_matching_entity_and_value_count_as_observation() {
        let event = json!({"type":"event","id":1,"event":{"data":{"entity_id":"light.living","new_state":{"state":"on"}},"context":{"id":"ctx-1"}}});
        assert!(matching_state_event(&event, "light.living", true));
        assert!(!matching_state_event(&event, "light.other", true));
        assert!(!matching_state_event(&event, "light.living", false));
        assert_eq!(event_context(&event), Some("ctx-1"));
    }

    const AFTER_EXPIRY: &str = "2026-09-29T19:30:00Z";

    fn timed_decision() -> BehaviorDecision {
        decision(
            "apply",
            true,
            "behavior:ff77bdb0",
            "ff77bdb0-70af-4f2a-a913-76609b66761b",
        )
    }

    fn decision(
        operation: &str,
        requested_value: bool,
        key: &str,
        decision_id: &str,
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
                "valid_from": "2026-09-29T18:00:00Z",
                "expires_at": "2026-09-29T19:00:00Z",
                "reason_code": "goal_verified",
                "idempotency_key": key,
                "required_confirmation": "device"
            })
            .to_string(),
        )
        .unwrap()
    }

    fn moment(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    /// Adresa, na ktorej nikto nepočúva: keby sa test predsa len pokúsil
    /// pripojiť, zlyhá rýchlo namiesto čakania na časový limit.
    fn unreachable() -> HaConfig {
        HaConfig {
            websocket_url: "ws://127.0.0.1:1/api/websocket".to_owned(),
            token: "unused".to_owned(),
        }
    }

    /// Svetlo tak, ako ho vidí inventár po HA sedení.
    fn light(power: bool, observed_at: &str) -> Device {
        Device {
            device_id: "ha:light.living".into(),
            provider: "home_assistant",
            provider_device_ref: "light.living".into(),
            name: "Living".into(),
            capability_id: "power",
            capability_type: "switch",
            writable: true,
            power: Some(power),
            observed_at: Some(observed_at.to_owned()),
            availability: Availability::Online,
        }
    }

    /// Odomkne grant a potvrdí unlock bez HA: testy zosúladenia potrebujú len
    /// otvorený prístup ako vstup.
    fn unlocked(ledger: &mut Ledger, decision: &BehaviorDecision) -> Grant {
        let now = moment("2026-09-29T18:30:00Z");
        let grant = grant::open(ledger, decision, now).unwrap();
        let actor = Actor {
            actor_type: ActorType::Automation,
            actor_id: "behavior-engine".to_owned(),
        };
        ledger
            .transition(
                &grant.unlock_command_id,
                Status::Sent,
                actor.clone(),
                None,
                None,
            )
            .unwrap();
        let unlocked = ledger
            .transition(
                &grant.unlock_command_id,
                Status::DeviceConfirmed,
                actor,
                Some(evidence(EvidenceKind::DeviceObservation, "obs-1")),
                None,
            )
            .unwrap();
        grant::settle_unlock(ledger, &grant.decision_id, &unlocked, now).unwrap()
    }

    #[tokio::test]
    async fn a_device_missing_from_the_inventory_fails_the_unlock() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        let grant = apply_decision(
            &unreachable(),
            &Inventory::new(),
            &mut ledger,
            &timed_decision(),
            moment("2026-09-29T18:30:00Z"),
        )
        .await
        .unwrap();
        assert_eq!(grant.state, GrantState::UnlockFailed);
        assert!(!grant.unlock_confirmed);
        // Nič nebolo odomknuté, takže zosúladenie nemá čo vracať späť.
        assert!(grant::due(&ledger, moment(AFTER_EXPIRY))
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_grant_left_without_a_result_is_taken_over_by_the_next_pass() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        // Unlock bol prijatý, ale výsledok sa nikdy nezapísal — pád alebo chyba
        // ledgeru medzi prijatím povelu a jeho výsledkom.
        let grant = grant::open(
            &mut ledger,
            &timed_decision(),
            moment("2026-09-29T18:30:00Z"),
        )
        .unwrap();
        assert_eq!(grant.state, GrantState::Granted);

        let settled = reconcile(
            &unreachable(),
            &Inventory::new(),
            &mut ledger,
            moment("2026-09-29T18:40:00Z"),
        )
        .await
        .unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].state, GrantState::Active);
        assert!(!settled[0].unlock_confirmed);
        let state = grant::access_state(&ledger, &grant.decision_id)
            .unwrap()
            .unwrap();
        assert_eq!(state.open_incidents.len(), 1);
        assert_eq!(state.open_incidents[0].kind, "result_lost");

        // Taký grant sa pri expirácii zamyká ako každý iný.
        let settled = reconcile(
            &unreachable(),
            &Inventory::new(),
            &mut ledger,
            moment(AFTER_EXPIRY),
        )
        .await
        .unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].state, GrantState::RelockPending);
    }

    #[tokio::test]
    async fn a_device_offline_at_expiry_leaves_the_relock_pending_with_an_incident() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        let grant = unlocked(&mut ledger, &timed_decision());

        let settled = reconcile(
            &unreachable(),
            &Inventory::new(),
            &mut ledger,
            moment(AFTER_EXPIRY),
        )
        .await
        .unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].state, GrantState::RelockPending);

        let raised = grant::incidents(&ledger, &grant.decision_id).unwrap();
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].kind, "relock_uncertain");

        // Kým je zariadenie nedostupné, ďalší prechod neposiela nič, nespotrebuje
        // pokus a nezaloží druhý incident.
        assert!(reconcile(
            &unreachable(),
            &Inventory::new(),
            &mut ledger,
            moment("2026-09-29T21:00:00Z"),
        )
        .await
        .unwrap()
        .is_empty());
        let state = grant::access_state(&ledger, &grant.decision_id)
            .unwrap()
            .unwrap();
        assert_eq!(state.close_attempts, 1);
        assert_eq!(state.open_incidents.len(), 1);
    }

    #[tokio::test]
    async fn a_device_seen_in_the_closing_value_is_reconciled_without_another_command() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        let grant = unlocked(&mut ledger, &timed_decision());
        reconcile(
            &unreachable(),
            &Inventory::new(),
            &mut ledger,
            moment(AFTER_EXPIRY),
        )
        .await
        .unwrap();

        // Home Assistant sa vráti a hlási svetlo vypnuté. Grant sa uzavrie
        // dôkazom, nie ďalším povelom, takže sa nikam nepripája.
        let inventory = Inventory::new();
        inventory
            .seed(vec![light(false, "2026-09-29T19:35:00.500+00:00")])
            .await;
        let settled = reconcile(
            &unreachable(),
            &inventory,
            &mut ledger,
            moment("2026-09-29T19:40:00Z"),
        )
        .await
        .unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].state, GrantState::Relocked);

        let state = grant::access_state(&ledger, &grant.decision_id)
            .unwrap()
            .unwrap();
        assert!(state.open_incidents.is_empty());
        let last = state.last_confirmed.unwrap();
        assert_eq!(last.value, json!(false));
        assert_eq!(last.confirmation, "device");

        // Prvý povel skončil ako `failed` a z toho stavu ledger nikam nepustí,
        // takže dôkaz nesie nový povel. Ten sa ale nikdy neodoslal: v jeho
        // histórii nie je `sent`.
        assert_eq!(state.close_attempts, 2);
        let statuses: Vec<Status> = ledger
            .events(&state.grant.relock_command_id.clone().unwrap())
            .unwrap()
            .into_iter()
            .map(|event| event.status)
            .collect();
        assert_eq!(
            statuses,
            [Status::Accepted, Status::Unknown, Status::DeviceConfirmed]
        );
    }

    #[tokio::test]
    async fn a_deleted_goal_closes_the_grant_even_when_home_assistant_is_gone() {
        let mut ledger = Ledger::open(":memory:").unwrap();
        let grant = unlocked(&mut ledger, &timed_decision());
        let cancel = decision(
            "revert",
            false,
            "behavior:b0c3f0d1",
            "b0c3f0d1-2f4a-4a53-9f7c-6ac1d1b2e3f4",
        );

        let settled = withdraw_decision(
            &unreachable(),
            &Inventory::new(),
            &mut ledger,
            &cancel,
            moment("2026-09-29T18:40:00Z"),
        )
        .await
        .unwrap();
        assert_eq!(settled.len(), 1);
        // Zariadenie sa nedalo osloviť, takže prístup nie je preukázateľne
        // zatvorený; stav to priznáva a incident zostáva otvorený.
        assert_eq!(settled[0].state, GrantState::RelockPending);
        let state = grant::access_state(&ledger, &grant.decision_id)
            .unwrap()
            .unwrap();
        assert_eq!(state.open_incidents.len(), 1);
        // Zrušený grant už nie je splatný pri expirácii.
        assert!(grant::due(&ledger, moment(AFTER_EXPIRY))
            .unwrap()
            .is_empty());

        // Keď sa svetlo ukáže vypnuté, zosúladenie grant uzavrie.
        let inventory = Inventory::new();
        inventory
            .seed(vec![light(false, "2026-09-29T18:45:00+00:00")])
            .await;
        let settled = reconcile(
            &unreachable(),
            &inventory,
            &mut ledger,
            moment("2026-09-29T18:50:00Z"),
        )
        .await
        .unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].state, GrantState::Relocked);
    }
}
