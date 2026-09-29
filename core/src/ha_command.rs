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

    #[test]
    fn only_matching_entity_and_value_count_as_observation() {
        let event = json!({"type":"event","id":1,"event":{"data":{"entity_id":"light.living","new_state":{"state":"on"}},"context":{"id":"ctx-1"}}});
        assert!(matching_state_event(&event, "light.living", true));
        assert!(!matching_state_event(&event, "light.other", true));
        assert!(!matching_state_event(&event, "light.living", false));
        assert_eq!(event_context(&event), Some("ctx-1"));
    }
}
