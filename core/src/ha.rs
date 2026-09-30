use std::{collections::BTreeMap, sync::Arc, time::Duration};

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
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

    /// Naplní inventár priamo, bez HA sedenia. Potrebujú to testy zosúladenia
    /// v `ha_command`, ktoré pracujú s pozorovaným stavom bez WebSocket servera.
    #[cfg(test)]
    pub(crate) async fn seed(&self, devices: Vec<Device>) {
        *self.0.write().await = devices
            .into_iter()
            .map(|device| (device.device_id.clone(), device))
            .collect();
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
    pub observed_at: Option<String>,
    pub availability: Availability,
}

/// Stav prepojenia Genesis↔Home Assistant.
///
/// Je to zámerne niečo iné než `/health`. Core môže odpovedať a byť úplne
/// zdravý aj vtedy, keď k Home Assistantovi nevidí — WebSocket sedenie beží
/// vedľa HTTP servera a spadne samo. Kto tie dve veci zlúči do jednej
/// kontrolky, dostane zelenú nad mŕtvym inventárom, a to je presne ten druh
/// falošného úspechu, ktorý sa nesmie zobraziť.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// Prepojenie nie je nastavené — jednotka nemá URL ani token. Nie je to
    /// porucha a nesmie sa tak javiť.
    NotConfigured,
    /// Sedenie sa otvára a inventár ešte neprišiel.
    Connecting,
    /// Sedenie je otvorené a inventár sa načítal.
    Connected,
    /// Sedenie spadlo alebo sa nepodarilo otvoriť.
    Disconnected,
}

#[derive(Clone, Debug, Serialize)]
pub struct LinkStatus {
    pub state: LinkState,
    /// Odkedy je prepojenie v tomto stave, RFC 3339. Pri výpadku sa nehýbe s
    /// každým opakovaným pokusom, takže sa z neho dá prečítať, ako dlho je to
    /// dole.
    pub since: String,
    /// Kedy sa naposledy načítal celý inventár.
    pub last_inventory_at: Option<String>,
    /// Koľko zariadení vtedy prišlo.
    pub last_inventory_devices: usize,
    /// Kategória posledného ukončenia sedenia. Kategória, nie správa: do
    /// diagnostiky nesmie presiaknuť token ani adresa.
    pub last_error: Option<&'static str>,
}

#[derive(Clone)]
pub struct Link(Arc<RwLock<LinkStatus>>);

impl Link {
    pub fn new(state: LinkState) -> Self {
        Self(Arc::new(RwLock::new(LinkStatus {
            state,
            since: Utc::now().to_rfc3339(),
            last_inventory_at: None,
            last_inventory_devices: 0,
            last_error: None,
        })))
    }

    pub async fn status(&self) -> LinkStatus {
        self.0.read().await.clone()
    }

    /// Prepojenie prešlo do stavu. `since` sa mení len pri skutočnej zmene
    /// stavu — opakované pokusy každé tri sekundy by inak každý výpadok robili
    /// večne čerstvým a nikto by z toho nevyčítal, ako dlho trvá.
    async fn entered(&self, state: LinkState, last_error: Option<&'static str>) {
        let mut status = self.0.write().await;
        if status.state != state {
            status.state = state;
            status.since = Utc::now().to_rfc3339();
        }
        if last_error.is_some() {
            status.last_error = last_error;
        }
    }

    /// Inventár prišiel celý. Až toto je dôkaz, že na druhej strane je Home
    /// Assistant, ktorý odpovedá — nie otvorený socket.
    async fn inventory_loaded(&self, devices: usize) {
        let at = Utc::now().to_rfc3339();
        let mut status = self.0.write().await;
        if status.state != LinkState::Connected {
            status.state = LinkState::Connected;
            status.since = at.clone();
        }
        status.last_inventory_at = Some(at);
        status.last_inventory_devices = devices;
    }
}

impl Default for Link {
    fn default() -> Self {
        Self::new(LinkState::NotConfigured)
    }
}

#[derive(Clone)]
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

impl HaError {
    /// Kategória pre log a diagnostiku. Zámerne bez detailu: ani adresa, ani
    /// token, ani telo odpovede sa odtiaľto nedostanú ďalej.
    fn category(&self) -> &'static str {
        match self {
            HaError::Connection => "connection",
            HaError::Authentication => "authentication",
            HaError::Protocol => "protocol",
            HaError::Disconnected => "disconnected",
        }
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn run(config: HaConfig, inventory: Inventory, link: Link) {
    loop {
        let result = run_session(&config, &inventory, &link).await;
        // Po strate sedenia sa inventár prizná ako neznámy a prepojenie ako
        // spadnuté. Poradie je dôležité: keby sa najprv ohlásilo spadnuté
        // prepojenie, ostal by okamih, v ktorom panel čítal starý stav
        // zariadení ako platný.
        inventory.mark_unknown().await;
        link.entered(
            LinkState::Disconnected,
            result.as_ref().err().map(HaError::category),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(category = %error, "Home Assistant connection ended");
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run_session(config: &HaConfig, inventory: &Inventory, link: &Link) -> Result<(), HaError> {
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
            link.inventory_loaded(count).await;
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
        observed_at: state
            .get("last_updated")
            .and_then(Value::as_str)
            .map(str::to_owned),
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
        let link = Link::new(LinkState::Connecting);
        let config = HaConfig {
            websocket_url: format!("ws://{address}/api/websocket"),
            token: "test-secret".to_owned(),
        };
        run_session(&config, &inventory, &link).await.unwrap_err();
        assert_eq!(inventory.devices().await[0].power, Some(true));
        // Prepojenie sa hlási ako spojené až po načítanom inventári, a vie
        // povedať, koľko zariadení vtedy prišlo.
        let loaded = link.status().await;
        assert_eq!(loaded.state, LinkState::Connected);
        assert_eq!(loaded.last_inventory_devices, 1);
        assert!(loaded.last_inventory_at.is_some());
        inventory.mark_unknown().await;
        assert_eq!(inventory.devices().await[0].power, None);
        run_session(&config, &inventory, &link).await.unwrap_err();
        assert_eq!(inventory.devices().await[0].power, Some(true));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_dropped_link_keeps_since_across_retries_and_records_only_a_category() {
        let link = Link::new(LinkState::Connecting);
        link.entered(
            LinkState::Disconnected,
            Some(HaError::Authentication.category()),
        )
        .await;
        let first = link.status().await;
        assert_eq!(first.state, LinkState::Disconnected);
        assert_eq!(first.last_error, Some("authentication"));
        // Druhý neúspešný pokus o to isté nesmie `since` posunúť, inak by sa z
        // dĺžky výpadku nedalo nič prečítať.
        link.entered(
            LinkState::Disconnected,
            Some(HaError::Connection.category()),
        )
        .await;
        let second = link.status().await;
        assert_eq!(second.since, first.since);
        assert_eq!(second.last_error, Some("connection"));
        // Kategória je celá pravda, ktorú diagnostika o chybe vydá — žiadna
        // adresa, žiadny token.
        for category in [
            HaError::Connection.category(),
            HaError::Authentication.category(),
            HaError::Protocol.category(),
            HaError::Disconnected.category(),
        ] {
            assert!(category
                .chars()
                .all(|character| character.is_ascii_lowercase()));
        }
    }

    #[tokio::test]
    async fn an_unconfigured_link_is_not_a_failure() {
        let link = Link::default();
        let status = link.status().await;
        assert_eq!(status.state, LinkState::NotConfigured);
        assert_eq!(status.last_error, None);
        assert_eq!(status.last_inventory_at, None);
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
