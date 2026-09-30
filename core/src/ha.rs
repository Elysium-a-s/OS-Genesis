use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

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

    /// Inventár známy dopredu, bez HA sedenia. Potrebujú to testy API, ktoré
    /// pracujú so zariadeniami bez WebSocket servera; sedenie si inventár plní
    /// samo z `get_states`.
    pub fn with(devices: Vec<Device>) -> Self {
        Self(Arc::new(RwLock::new(by_device_id(devices))))
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
        *self.0.write().await = by_device_id(devices);
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

fn by_device_id(devices: Vec<Device>) -> BTreeMap<String, Device> {
    devices
        .into_iter()
        .map(|device| (device.device_id.clone(), device))
        .collect()
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

/// Miestnosť domácnosti.
///
/// `area_id` nesie predponu providera rovnako ako `device_id`, takže sa na prvý
/// pohľad vidí, odkiaľ identifikátor je. Home Assistant area id je slug, ktorý
/// sa pri premenovaní miestnosti **nemení** — práve preto je stabilný a dá sa
/// na ňom držať výber v paneli, kým sa názov mení voľne.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Area {
    pub area_id: String,
    pub name: String,
}

/// Miestnosti domácnosti a to, do ktorej patrí ktoré zariadenie.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RegistrySnapshot {
    /// Názov domácnosti z Home Assistanta (`location_name`). Chýba, kým sedenie
    /// nenačíta konfiguráciu; vtedy si ho nikto nemá domýšľať.
    pub location_name: Option<String>,
    pub areas: Vec<Area>,
    /// Genesis `device_id` → `area_id`. Chýbajúci záznam znamená bez
    /// miestnosti, čo je legitímny stav domácnosti, nie chyba.
    pub area_of_device: BTreeMap<String, String>,
    /// Registre sa nepodarilo prečítať celé. Stáva sa to, keď token nepatrí
    /// administrátorovi: `config/*_registry/list` vtedy skončí neúspechom.
    /// Inventár a povely fungujú ďalej, len bez miestností — a panel to má
    /// povedať, nie tvrdiť, že domácnosť žiadne miestnosti nemá.
    pub partial: bool,
}

#[derive(Clone, Default)]
pub struct Registry(Arc<RwLock<RegistrySnapshot>>);

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mapovanie známe dopredu, bez HA sedenia. Rovnaký dôvod ako pri
    /// `Inventory::with`: API sa dá otestovať bez Home Assistanta.
    pub fn with(snapshot: RegistrySnapshot) -> Self {
        Self(Arc::new(RwLock::new(snapshot)))
    }

    pub async fn snapshot(&self) -> RegistrySnapshot {
        self.0.read().await.clone()
    }

    async fn replace(&self, snapshot: RegistrySnapshot) {
        *self.0.write().await = snapshot;
    }

    /// Naplní mapovanie priamo, bez HA sedenia. Potrebujú to testy API, ktoré
    /// pracujú s miestnosťami bez WebSocket servera.
    #[cfg(test)]
    pub(crate) async fn seed(&self, snapshot: RegistrySnapshot) {
        self.replace(snapshot).await;
    }
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

pub async fn run(config: HaConfig, inventory: Inventory, registry: Registry, link: Link) {
    loop {
        let result = run_session(&config, &inventory, &registry, &link).await;
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

/// Predplatenia sedenia.
///
/// `STATES_SUBSCRIPTION` nesie pozorovaný stav. Tri registrové predplatenia
/// nenesú nový záznam, iba to, že sa register zmenil, takže na ne Genesis
/// odpovedá novým prečítaním registrov. Presun entity do inej miestnosti aj
/// premenovanie miestnosti tak idú jednou cestou, bez skladania čiastkovej
/// udalosti do vlastného modelu.
const STATES_SUBSCRIPTION: u64 = 1;
const REGISTRY_SUBSCRIPTIONS: [(u64, &str); 3] = [
    (2, "area_registry_updated"),
    (3, "device_registry_updated"),
    (4, "entity_registry_updated"),
];

/// Dotazy, z ktorých sa skladá mapovanie miestností.
///
/// Všetky tri registre sú potrebné: entita môže mať miestnosť priamo, alebo ju
/// dediť od zariadenia, ku ktorému patrí. Bez `device_registry` by entita
/// priradená len cez zariadenie vyšla ako bez miestnosti — a to je tá častejšia
/// z dvoch možností, pretože v Home Assistante človek priraďuje zariadenie.
const REGISTRY_COMMANDS: [&str; 4] = [
    "get_config",
    "config/area_registry/list",
    "config/device_registry/list",
    "config/entity_registry/list",
];

const FIRST_LOAD: u64 = 10;
const STATES_REQUEST: u64 = 14;
const RELOAD_BASE: u64 = 20;

/// Rozbehnuté čítanie registrov.
///
/// Home Assistant odpovedá na každý dotaz zvlášť, takže mapovanie sa prepíše až
/// keď dorazia všetky štyri odpovede. Inak by medzi nimi existoval okamih, v
/// ktorom panel vidí miestnosti bez zariadení alebo zariadenia bez miestností.
struct RegistryLoad {
    base: u64,
    answers: [Option<Value>; 4],
    partial: bool,
}

impl RegistryLoad {
    async fn request(socket: &mut Socket, base: u64) -> Result<Self, HaError> {
        for (offset, command) in REGISTRY_COMMANDS.iter().enumerate() {
            send_json(socket, json!({"id": base + offset as u64, "type": command})).await?;
        }
        Ok(Self {
            base,
            answers: [None, None, None, None],
            partial: false,
        })
    }

    fn owns(&self, id: u64) -> bool {
        id >= self.base && id < self.base + REGISTRY_COMMANDS.len() as u64
    }

    /// Vezme jednu odpoveď a vráti hotové mapovanie, keď dorazila posledná.
    ///
    /// Neúspešná odpoveď sedenie nezhodí. `config/*_registry/list` vyžaduje
    /// administrátorský token, a keby na ňom padlo celé sedenie, jednotka by
    /// prestala vidieť zariadenia a prijímať povely len preto, že nevie názvy
    /// miestností. Tá strata je neúmerná: inventár beží ďalej a chýbajúce
    /// mapovanie sa prizná ako `partial`.
    fn accept(&mut self, message: &Value) -> Option<RegistrySnapshot> {
        let id = message["id"].as_u64()?;
        let slot = (id - self.base) as usize;
        if message["success"] == true {
            self.answers[slot] = Some(message["result"].clone());
        } else {
            self.answers[slot] = Some(Value::Null);
            self.partial = true;
        }
        if self.answers.iter().any(Option::is_none) {
            return None;
        }
        Some(map_registry(&self.answers, self.partial))
    }
}

async fn run_session(
    config: &HaConfig,
    inventory: &Inventory,
    registry: &Registry,
    link: &Link,
) -> Result<(), HaError> {
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

    subscribe(&mut socket, STATES_SUBSCRIPTION, "state_changed").await?;
    for (id, event_type) in REGISTRY_SUBSCRIPTIONS {
        subscribe(&mut socket, id, event_type).await?;
    }

    let mut load = RegistryLoad::request(&mut socket, FIRST_LOAD).await?;
    send_json(
        &mut socket,
        json!({"id": STATES_REQUEST, "type": "get_states"}),
    )
    .await?;
    let mut buffered_events = Vec::new();
    let mut states_loaded = false;
    let mut registry_loaded = false;
    while !(states_loaded && registry_loaded) {
        let message = next_json(&mut socket).await?;
        if message["type"] == "event" {
            if message["id"] == STATES_SUBSCRIPTION {
                buffered_events.push(message);
            }
            // Zmena registra počas prvého načítania sa nespracúva tu. Prebiehajúce
            // čítanie ju možno už zachytilo a možno nie; hlavná slučka nižšie na
            // ďalšiu takú udalosť prečíta registre znova.
            continue;
        }
        if message["type"] != "result" {
            return Err(HaError::Protocol);
        }
        let id = message["id"].as_u64().ok_or(HaError::Protocol)?;
        if id == STATES_REQUEST {
            if message["success"] != true {
                return Err(HaError::Protocol);
            }
            let states = message["result"].as_array().ok_or(HaError::Protocol)?;
            inventory.replace(states).await;
            for event in &buffered_events {
                inventory.apply_event(event).await;
            }
            states_loaded = true;
        } else if load.owns(id) {
            if let Some(snapshot) = load.accept(&message) {
                if snapshot.partial {
                    tracing::warn!(
                        "Home Assistant registries were not readable in full; rooms are unavailable"
                    );
                }
                registry.replace(snapshot).await;
                registry_loaded = true;
            }
        } else {
            return Err(HaError::Protocol);
        }
    }
    let count = inventory.devices().await.len();
    link.inventory_loaded(count).await;
    tracing::info!(count, "Home Assistant inventory loaded");

    let mut generation = 0u64;
    let mut reload: Option<RegistryLoad> = None;
    loop {
        let message = next_json(&mut socket).await?;
        let id = message["id"].as_u64().unwrap_or_default();
        if message["type"] == "event" {
            if id == STATES_SUBSCRIPTION {
                inventory.apply_event(&message).await;
            } else if REGISTRY_SUBSCRIPTIONS.iter().any(|(known, _)| *known == id) {
                // Nové čítanie aj vtedy, keď už jedno beží: tá staršia odpoveď
                // by opisovala stav pred touto zmenou.
                generation += 1;
                reload =
                    Some(RegistryLoad::request(&mut socket, RELOAD_BASE + generation * 10).await?);
            }
            continue;
        }
        if message["type"] == "result" {
            let finished = match reload.as_mut() {
                Some(pending) if pending.owns(id) => pending.accept(&message),
                _ => None,
            };
            if let Some(snapshot) = finished {
                registry.replace(snapshot).await;
                reload = None;
            }
        }
    }
}

async fn subscribe(socket: &mut Socket, id: u64, event_type: &str) -> Result<(), HaError> {
    send_json(
        socket,
        json!({"id": id, "type": "subscribe_events", "event_type": event_type}),
    )
    .await?;
    let result = next_json(socket).await?;
    if result["type"] != "result" || result["id"] != id || result["success"] != true {
        return Err(HaError::Protocol);
    }
    Ok(())
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

/// Zloží mapovanie miestností z odpovedí na `REGISTRY_COMMANDS`.
///
/// Entita má miestnosť priamo (`area_id` v entity registri) alebo ju dedí od
/// svojho zariadenia. Priame priradenie vyhráva, pretože je konkrétnejšie: je to
/// výnimka, ktorú človek nastavil práve pre túto entitu.
fn map_registry(answers: &[Option<Value>; 4], partial: bool) -> RegistrySnapshot {
    let value = |index: usize| answers[index].as_ref().unwrap_or(&Value::Null);
    let rows =
        |index: usize| -> &[Value] { value(index).as_array().map(Vec::as_slice).unwrap_or(&[]) };

    let location_name = value(0)
        .get("location_name")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let mut areas = Vec::new();
    for row in rows(1) {
        let Some(area_id) = row.get("area_id").and_then(Value::as_str) else {
            continue;
        };
        // Miestnosť bez názvu by v paneli bola prázdny riadok, na ktorý sa nedá
        // kliknúť s významom.
        let Some(name) = row.get("name").and_then(Value::as_str) else {
            continue;
        };
        areas.push(Area {
            area_id: format!("ha:{area_id}"),
            name: name.to_owned(),
        });
    }
    areas.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.area_id.cmp(&right.area_id))
    });

    let mut area_of_ha_device = BTreeMap::new();
    for row in rows(2) {
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(area_id) = row.get("area_id").and_then(Value::as_str) {
            area_of_ha_device.insert(id.to_owned(), area_id.to_owned());
        }
    }

    let known: BTreeSet<&str> = areas.iter().map(|area| area.area_id.as_str()).collect();
    let mut area_of_device = BTreeMap::new();
    for row in rows(3) {
        let Some(entity_id) = row.get("entity_id").and_then(Value::as_str) else {
            continue;
        };
        if !supported_entity(entity_id) {
            continue;
        }
        let direct = row.get("area_id").and_then(Value::as_str);
        let inherited = row
            .get("device_id")
            .and_then(Value::as_str)
            .and_then(|device| area_of_ha_device.get(device))
            .map(String::as_str);
        let Some(area_id) = direct.or(inherited) else {
            continue;
        };
        let area_id = format!("ha:{area_id}");
        // Priradenie do miestnosti, ktorú register nepozná, by v paneli vyrobilo
        // skupinu bez názvu. Zariadenie potom vyjde ako bez miestnosti, čo je
        // pravda o tom, čo o ňom vieme.
        if !known.contains(area_id.as_str()) {
            continue;
        }
        area_of_device.insert(format!("ha:{entity_id}"), area_id);
    }

    RegistrySnapshot {
        location_name,
        areas,
        area_of_device,
        partial,
    }
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

    #[test]
    fn a_room_comes_from_the_entity_or_from_its_device() {
        let snapshot = map_registry(
            &[
                Some(json!({"location_name":"Doma"})),
                Some(json!([
                    {"area_id":"living_room","name":"Obývačka"},
                    {"area_id":"bedroom","name":"Spálňa"},
                    // Miestnosť bez názvu by bola prázdny riadok v paneli.
                    {"area_id":"nameless"}
                ])),
                Some(json!([{"id":"dev-1","area_id":"bedroom"}])),
                Some(json!([
                    // Priame priradenie je konkrétnejšie než to od zariadenia.
                    {"entity_id":"light.living","area_id":"living_room","device_id":"dev-1"},
                    // Toto svetlo miestnosť dedí od zariadenia.
                    {"entity_id":"switch.plug","device_id":"dev-1"},
                    // Doména, ktorú Genesis neovláda, do inventára nepatrí.
                    {"entity_id":"sensor.temperature","area_id":"bedroom"},
                    // Miestnosť, ktorú register nepozná, sa nedá pomenovať.
                    {"entity_id":"light.hall","area_id":"nowhere"},
                    // Bez miestnosti je legitímny stav.
                    {"entity_id":"light.attic"}
                ])),
            ],
            false,
        );
        assert_eq!(snapshot.location_name.as_deref(), Some("Doma"));
        // Zoradené podľa názvu, nie podľa poradia z registra.
        assert_eq!(
            snapshot.areas,
            vec![
                Area {
                    area_id: "ha:living_room".to_owned(),
                    name: "Obývačka".to_owned()
                },
                Area {
                    area_id: "ha:bedroom".to_owned(),
                    name: "Spálňa".to_owned()
                },
            ]
        );
        assert_eq!(
            snapshot
                .area_of_device
                .get("ha:light.living")
                .map(String::as_str),
            Some("ha:living_room")
        );
        assert_eq!(
            snapshot
                .area_of_device
                .get("ha:switch.plug")
                .map(String::as_str),
            Some("ha:bedroom")
        );
        assert!(!snapshot
            .area_of_device
            .contains_key("ha:sensor.temperature"));
        assert!(!snapshot.area_of_device.contains_key("ha:light.hall"));
        assert!(!snapshot.area_of_device.contains_key("ha:light.attic"));
        assert!(!snapshot.partial);
    }

    type TestSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    fn one_light(state: &str, name: &str) -> Value {
        json!([{"entity_id":"light.living","state":state,"attributes":{"friendly_name":name}}])
    }

    fn pilot_registries(area: &str) -> [Value; 4] {
        [
            json!({"location_name":"Doma"}),
            json!([
                {"area_id":"living_room","name":"Obývačka"},
                {"area_id":"bedroom","name":"Spálňa"}
            ]),
            json!([]),
            json!([{"entity_id":"light.living","area_id":area}]),
        ]
    }

    /// Ručný Home Assistant: prejde overením a všetkými štyrmi predplateniami.
    async fn handshake(socket: &mut TestSocket) {
        send_server(socket, json!({"type":"auth_required"})).await;
        let auth = read_server(socket).await;
        assert_eq!(auth["type"], "auth");
        assert_eq!(auth["access_token"], "test-secret");
        send_server(socket, json!({"type":"auth_ok"})).await;
        for expected in [STATES_SUBSCRIPTION, 2, 3, 4] {
            let subscription = read_server(socket).await;
            assert_eq!(subscription["type"], "subscribe_events");
            assert_eq!(subscription["id"], expected);
            send_server(
                socket,
                json!({"type":"result","id":expected,"success":true,"result":null}),
            )
            .await;
        }
    }

    /// Odpovie na `count` dotazov, každému podľa jeho typu. `registry_success`
    /// false napodobní token bez administrátorských práv.
    async fn answer(
        socket: &mut TestSocket,
        count: usize,
        registries: [Value; 4],
        states: Value,
        registry_success: bool,
    ) {
        for _ in 0..count {
            let request = read_server(socket).await;
            let id = request["id"].clone();
            let (success, result) = match request["type"].as_str().unwrap() {
                "get_states" => (true, states.clone()),
                "get_config" => (true, registries[0].clone()),
                "config/area_registry/list" => (registry_success, registries[1].clone()),
                "config/device_registry/list" => (registry_success, registries[2].clone()),
                "config/entity_registry/list" => (registry_success, registries[3].clone()),
                other => panic!("unexpected request {other}"),
            };
            send_server(
                socket,
                json!({"type":"result","id":id,"success":success,"result":result}),
            )
            .await;
        }
    }

    fn pilot_config(address: std::net::SocketAddr) -> HaConfig {
        HaConfig {
            websocket_url: format!("ws://{address}/api/websocket"),
            token: "test-secret".to_owned(),
        }
    }

    #[tokio::test]
    async fn authenticates_subscribes_and_reloads_after_disconnect() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                handshake(&mut socket).await;
                answer(
                    &mut socket,
                    5,
                    pilot_registries("living_room"),
                    one_light("on", "Living"),
                    true,
                )
                .await;
                socket.close(None).await.unwrap();
            }
        });
        let inventory = Inventory::new();
        let registry = Registry::new();
        let link = Link::new(LinkState::Connecting);
        let config = pilot_config(address);
        run_session(&config, &inventory, &registry, &link)
            .await
            .unwrap_err();
        assert_eq!(inventory.devices().await[0].power, Some(true));
        // Prepojenie sa hlási ako spojené až po načítanom inventári, a vie
        // povedať, koľko zariadení vtedy prišlo.
        let loaded = link.status().await;
        assert_eq!(loaded.state, LinkState::Connected);
        assert_eq!(loaded.last_inventory_devices, 1);
        assert!(loaded.last_inventory_at.is_some());
        // Miestnosti prišli tým istým sedením.
        let rooms = registry.snapshot().await;
        assert_eq!(rooms.location_name.as_deref(), Some("Doma"));
        assert_eq!(rooms.areas.len(), 2);
        assert_eq!(
            rooms
                .area_of_device
                .get("ha:light.living")
                .map(String::as_str),
            Some("ha:living_room")
        );
        inventory.mark_unknown().await;
        assert_eq!(inventory.devices().await[0].power, None);
        run_session(&config, &inventory, &registry, &link)
            .await
            .unwrap_err();
        assert_eq!(inventory.devices().await[0].power, Some(true));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_entity_moved_to_another_room_is_read_from_the_registry_again() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            handshake(&mut socket).await;
            answer(
                &mut socket,
                5,
                pilot_registries("living_room"),
                one_light("on", "Living"),
                true,
            )
            .await;
            // Človek presunul svetlo do spálne. Udalosť nenesie nový záznam, iba
            // to, že sa register zmenil.
            send_server(
                &mut socket,
                json!({"type":"event","id":4,"event":{"event_type":"entity_registry_updated","data":{"action":"update","entity_id":"light.living"}}}),
            )
            .await;
            answer(
                &mut socket,
                4,
                pilot_registries("bedroom"),
                Value::Null,
                true,
            )
            .await;
            socket.close(None).await.unwrap();
        });
        let inventory = Inventory::new();
        let registry = Registry::new();
        let link = Link::new(LinkState::Connecting);
        run_session(&pilot_config(address), &inventory, &registry, &link)
            .await
            .unwrap_err();
        assert_eq!(
            registry
                .snapshot()
                .await
                .area_of_device
                .get("ha:light.living")
                .map(String::as_str),
            Some("ha:bedroom")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_renamed_entity_keeps_its_room_and_takes_the_new_name() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            handshake(&mut socket).await;
            answer(
                &mut socket,
                5,
                pilot_registries("living_room"),
                one_light("on", "Living"),
                true,
            )
            .await;
            // Premenovanie entity prepíše `friendly_name` v stave, takže prichádza
            // ako state_changed — nie ako registrová udalosť.
            send_server(
                &mut socket,
                json!({"type":"event","id":1,"event":{"event_type":"state_changed","data":{"entity_id":"light.living","new_state":{"entity_id":"light.living","state":"on","attributes":{"friendly_name":"Veľké svetlo"}}}}}),
            )
            .await;
            socket.close(None).await.unwrap();
        });
        let inventory = Inventory::new();
        let registry = Registry::new();
        let link = Link::new(LinkState::Connecting);
        run_session(&pilot_config(address), &inventory, &registry, &link)
            .await
            .unwrap_err();
        let device = &inventory.devices().await[0];
        assert_eq!(device.name, "Veľké svetlo");
        assert_eq!(device.device_id, "ha:light.living");
        // Premenovanie nie je presun.
        assert_eq!(
            registry
                .snapshot()
                .await
                .area_of_device
                .get("ha:light.living")
                .map(String::as_str),
            Some("ha:living_room")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn registries_that_are_not_readable_leave_the_inventory_usable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            handshake(&mut socket).await;
            answer(
                &mut socket,
                5,
                pilot_registries("living_room"),
                one_light("on", "Living"),
                false,
            )
            .await;
            socket.close(None).await.unwrap();
        });
        let inventory = Inventory::new();
        let registry = Registry::new();
        let link = Link::new(LinkState::Connecting);
        run_session(&pilot_config(address), &inventory, &registry, &link)
            .await
            .unwrap_err();
        // Token bez administrátorských práv stojí názvy miestností, nie ovládanie.
        assert_eq!(inventory.devices().await[0].power, Some(true));
        assert_eq!(link.status().await.state, LinkState::Connected);
        let rooms = registry.snapshot().await;
        assert!(rooms.partial);
        assert!(rooms.areas.is_empty());
        assert!(rooms.area_of_device.is_empty());
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
