use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{
    ha::{Device, HaConfig},
    ledger::{Actor, Evidence, EvidenceKind, Ledger, LedgerError, Snapshot, Status},
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn service_ack_and_correlated_event_confirm_device() {
        use crate::ledger::{ActorType, CommandRequest};
        use crate::ha::Availability;
        use tokio_tungstenite::accept_async;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            socket.send(Message::Text(json!({"type":"auth_required"}).to_string().into())).await.unwrap();
            let auth = socket.next().await.unwrap().unwrap();
            assert_eq!(serde_json::from_str::<Value>(auth.to_text().unwrap()).unwrap()["type"], "auth");
            socket.send(Message::Text(json!({"type":"auth_ok"}).to_string().into())).await.unwrap();
            let sub = socket.next().await.unwrap().unwrap();
            assert_eq!(serde_json::from_str::<Value>(sub.to_text().unwrap()).unwrap()["type"], "subscribe_events");
            socket.send(Message::Text(json!({"id":1,"type":"result","success":true}).to_string().into())).await.unwrap();
            let command = socket.next().await.unwrap().unwrap();
            let command: Value = serde_json::from_str(command.to_text().unwrap()).unwrap();
            assert_eq!(command["type"], "call_service");
            assert_eq!(command["domain"], "light");
            assert_eq!(command["service"], "turn_on");
            assert_eq!(command["target"]["entity_id"], "light.living");
            socket.send(Message::Text(json!({"id":1,"type":"event","event":{"context":{"id":"ctx-1"},"data":{"entity_id":"light.living","new_state":{"state":"on"}}}}).to_string().into())).await.unwrap();
            socket.send(Message::Text(json!({"id":2,"type":"result","success":true,"result":{"context":{"id":"ctx-1"},"response":null}}).to_string().into())).await.unwrap();
        });
        let actor = Actor { actor_type: ActorType::User, actor_id: "pilot-owner".into() };
        let mut ledger = Ledger::open(":memory:").unwrap();
        ledger.accept(CommandRequest {
            household_id: "pilot-home".into(),
            command_id: "cmd-1".into(),
            device_id: "ha:light.living".into(),
            capability_id: "power".into(),
            value: Value::Bool(true),
            actor: actor.clone(),
            idempotency_key: "idem-1".into(),
            correlation_id: "corr-1".into(),
        }).unwrap();
        let device = Device {
            device_id: "ha:light.living".into(),
            provider: "home_assistant",
            provider_device_ref: "light.living".into(),
            name: "Living".into(),
            capability_id: "power",
            capability_type: "switch",
            writable: true,
            power: Some(false),
            availability: Availability::Online,
        };
        let config = HaConfig {
            websocket_url: format!("ws://{address}/api/websocket"),
            token: "test-secret".into(),
        };
        let snapshot = execute(&config, &device, true, &mut ledger, "cmd-1", actor).await.unwrap();
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
}
