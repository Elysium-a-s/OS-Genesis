use std::{collections::BTreeMap, sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Clone)]
pub struct Inventory(Arc<RwLock<BTreeMap<String, Device>>>);

impl Inventory {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(BTreeMap::new())))
    }

    pub async fn devices(&self) -> Vec<Device> {
        self.0.read().await.values().cloned().collect()
    }

    async fn replace(&self, states: &[Value]) {
        let mut devices = BTreeMap::new();
        for state in states {
            if let Some(device) = map_state(state) {
                devices.insert(device.device_id.clone(), device);
            }
        }
        *self.0.write().await = devices;
    }

    async fn apply_event(&self, event: &Value) {
        let Some(data) = event.get("event").and_then(|value| value.get("data")) else {
            return;
        };
        let Some(entity_id) = data.get("entity_id").and_then(Value::as_str) else {
            return;
        };
        if !supported_entity(entity_id) {
            return;
        }
        let mut devices = self.0.write().await;
        match data.get("new_state") {
            Some(Value::Null) | None => {
                devices.remove(&format!("ha:{entity_id}"));
            }
            Some(state) => {
                if let Some(device) = map_state(state) {
                    devices.insert(device.device_id.clone(), device);
                }
            }
        }
    }

    async fn mark_unknown(&self) {
        for device in self.0.write().await.values_mut() {
            device.availability = Availability::Unknown;
            device.power = None;
        }
    }
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Online,
    Offline,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Device {
    pub device_id: String,
    pub provider: &'static str,
    pub provider_device_ref: String,
    pub name: String,
    pub capability_id: &'static str,
    pub capability_type: &'static str,
    pub writable: bool,
    pub power: Option<bool>,
    pub availability: Availability,
}

pub struct HaConfig {
    pub websocket_url: String,
    pub token: String,
}

impl HaConfig {
    pub fn from_env() -> Result<Option<Self>, &'static str> {
        let url = std::env::var("GENESIS_HA_WS_URL").ok();
        let token = std::env::var("GENESIS_HA_TOKEN").ok();
        match (url, token) {
            (None, None) => Ok(None),
            (Some(websocket_url), Some(token))
                if (websocket_url.starts_with("ws://") || websocket_url.starts_with("wss://"))
                    && !websocket_url.contains('@')
                    && !token.is_empty() =>
            {
                Ok(Some(Self { websocket_url, token }))
            }
            _ => Err("set both GENESIS_HA_WS_URL and GENESIS_HA_TOKEN; URL must be ws:// or wss:// without credentials"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum HaError {
    #[error("connection error")]
    Connection,
    #[error("authentication error")]
    Authentication,
    #[error("protocol error")]
    Protocol,
    #[error("disconnected")]
    Disconnected,
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn run(config: HaConfig, inventory: Inventory) {
    loop {
        let result = run_session(&config, &inventory).await;
        inventory.mark_unknown().await;
        if let Err(error) = result {
            tracing::warn!(category = %error, "Home Assistant connection ended");
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run_session(config: &HaConfig, inventory: &Inventory) -> Result<(), HaError> {
    let (mut socket, _) = connect_async(&config.websocket_url)
        .await
        .map_err(|_| HaError::Connection)?;
    let greeting = next_json(&mut socket).await?;
    if greeting["type"] != "auth_required" {
        return Err(HaError::Protocol);
    }
    send_json(
        &mut socket,
        json!({"type": "auth", "access_token": config.token}),
    )
    .await?;
    let auth = next_json(&mut socket).await?;
    if auth["type"] != "auth_ok" {
        return Err(HaError::Authentication);
    }

    send_json(
        &mut socket,
        json!({"id": 1, "type": "subscribe_events", "event_type": "state_changed"}),
    )
    .await?;
    let subscription = next_json(&mut socket).await?;
    if subscription["type"] != "result"
        || subscription["id"] != 1
        || subscription["success"] != true
    {
        return Err(HaError::Protocol);
    }
    send_json(&mut socket, json!({"id": 2, "type": "get_states"})).await?;
    let mut buffered_events = Vec::new();
    loop {
        let message = next_json(&mut socket).await?;
        if message["type"] == "event" && message["id"] == 1 {
            buffered_events.push(message);
        } else if message["type"] == "result" && message["id"] == 2 {
            if message["success"] != true {
                return Err(HaError::Protocol);
            }
            let states = message["result"].as_array().ok_or(HaError::Protocol)?;
            inventory.replace(states).await;
            for event in &buffered_events {
                inventory.apply_event(event).await;
            }
            let count = inventory.devices().await.len();
            tracing::info!(count, "Home Assistant inventory loaded");
            break;
        } else {
            return Err(HaError::Protocol);
        }
    }

    loop {
        let message = next_json(&mut socket).await?;
        if message["type"] == "event" && message["id"] == 1 {
            inventory.apply_event(&message).await;
        }
    }
}

async fn next_json(socket: &mut Socket) -> Result<Value, HaError> {
    loop {
        let frame = socket
            .next()
            .await
            .ok_or(HaError::Disconnected)?
            .map_err(|_| HaError::Disconnected)?;
        match frame {
            Message::Text(text) => {
                return serde_json::from_str(&text).map_err(|_| HaError::Protocol)
            }
            Message::Close(_) => return Err(HaError::Disconnected),
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {
                continue
            }
        }
    }
}

async fn send_json(socket: &mut Socket, value: Value) -> Result<(), HaError> {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|_| HaError::Disconnected)
}

fn supported_entity(entity_id: &str) -> bool {
    entity_id.starts_with("light.") || entity_id.starts_with("switch.")
}

fn map_state(state: &Value) -> Option<Device> {
    let entity_id = state.get("entity_id")?.as_str()?;
    if !supported_entity(entity_id) {
        return None;
    }
    let (power, availability) = match state.get("state")?.as_str()? {
        "on" => (Some(true), Availability::Online),
        "off" => (Some(false), Availability::Online),
        "unavailable" => (None, Availability::Offline),
        _ => (None, Availability::Unknown),
    };
    let name = state
        .get("attributes")
        .and_then(|attributes| attributes.get("friendly_name"))
        .and_then(Value::as_str)
        .unwrap_or(entity_id)
        .to_owned();
    Some(Device {
        device_id: format!("ha:{entity_id}"),
        provider: "home_assistant",
        provider_device_ref: entity_id.to_owned(),
        name,
        capability_id: "power",
        capability_type: "switch",
        writable: true,
        power,
        availability,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::accept_async;

    #[test]
    fn maps_light_state_and_unknown_without_claiming_power() {
        let light = map_state(&json!({"entity_id":"light.living","state":"on","attributes":{"friendly_name":"Living"}})).unwrap();
        assert_eq!(light.device_id, "ha:light.living");
        assert_eq!(light.power, Some(true));
        assert_eq!(light.capability_id, "power");
        let unknown = map_state(&json!({"entity_id":"switch.plug","state":"unavailable"})).unwrap();
        assert_eq!(unknown.power, None);
        assert_eq!(unknown.availability, Availability::Offline);
        assert!(map_state(&json!({"entity_id":"sensor.temperature","state":"20"})).is_none());
    }

    #[tokio::test]
    async fn authenticates_subscribes_and_reloads_after_disconnect() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                send_server(&mut socket, json!({"type":"auth_required"})).await;
                let auth = read_server(&mut socket).await;
                assert_eq!(auth["type"], "auth");
                assert_eq!(auth["access_token"], "test-secret");
                send_server(&mut socket, json!({"type":"auth_ok"})).await;
                let subscription = read_server(&mut socket).await;
                assert_eq!(subscription["type"], "subscribe_events");
                send_server(
                    &mut socket,
                    json!({"type":"result","id":1,"success":true,"result":null}),
                )
                .await;
                let get_states = read_server(&mut socket).await;
                assert_eq!(get_states["type"], "get_states");
                send_server(&mut socket, json!({"type":"result","id":2,"success":true,"result":[{"entity_id":"light.living","state":"on","attributes":{"friendly_name":"Living"}}]})).await;
                socket.close(None).await.unwrap();
            }
        });
        let inventory = Inventory::new();
        let config = HaConfig {
            websocket_url: format!("ws://{address}/api/websocket"),
            token: "test-secret".to_owned(),
        };
        run_session(&config, &inventory).await.unwrap_err();
        assert_eq!(inventory.devices().await[0].power, Some(true));
        inventory.mark_unknown().await;
        assert_eq!(inventory.devices().await[0].power, None);
        run_session(&config, &inventory).await.unwrap_err();
        assert_eq!(inventory.devices().await[0].power, Some(true));
        server.await.unwrap();
    }

    async fn send_server(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        value: Value,
    ) {
        socket
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }

    async fn read_server(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ) -> Value {
        let frame = socket.next().await.unwrap().unwrap();
        serde_json::from_str(frame.to_text().unwrap()).unwrap()
    }
}
