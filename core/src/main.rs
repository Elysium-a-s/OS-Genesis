use std::{
    collections::BTreeMap,
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, Path, Query, Request, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Html, Response},
    routing::get,
    Json, Router,
};
use chrono::Utc;
use genesis_core::{
    backup,
    grant::{self, AccessState},
    ha::{self, HaConfig, Inventory, Link, LinkState, LinkStatus, Registry, RegistrySnapshot},
    ha_command::{self, ExecutionError},
    identity::{self, Role},
    ledger::{Actor, ActorType, CommandRequest, Ledger, LedgerError, Snapshot},
    voice,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

struct Config {
    bind_addr: SocketAddr,
    log_filter: EnvFilter,
    ledger_path: PathBuf,
    read_token: Option<String>,
    write_token: Option<String>,
    member_token: Option<String>,
    guest_token: Option<String>,
    household_id: String,
    sensitive_devices: Vec<String>,
    backup_dir: Option<PathBuf>,
    panel_dir: Option<PathBuf>,
    panel_index: Option<Arc<String>>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let mut config = Self::from_values(
            env::var("GENESIS_BIND_ADDR").ok().as_deref(),
            env::var("GENESIS_LOG").ok().as_deref(),
        )?;
        config.ledger_path = env::var_os("GENESIS_LEDGER_PATH")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./genesis-ledger.sqlite3"));
        config.read_token = env::var("GENESIS_READ_TOKEN").ok();
        config.write_token = env::var("GENESIS_WRITE_TOKEN").ok();
        config.member_token = env::var("GENESIS_MEMBER_TOKEN")
            .ok()
            .filter(|token| !token.is_empty());
        config.guest_token = env::var("GENESIS_GUEST_TOKEN")
            .ok()
            .filter(|token| !token.is_empty());
        config.household_id =
            env::var("GENESIS_HOUSEHOLD_ID").unwrap_or_else(|_| "pilot-home".to_owned());
        config.panel_dir = env::var_os("GENESIS_PANEL_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        if config
            .panel_dir
            .as_ref()
            .is_some_and(|dir| !dir.join("index.html").is_file())
        {
            return Err("GENESIS_PANEL_DIR must contain index.html".to_owned());
        }
        config.panel_index = config
            .panel_dir
            .as_ref()
            .map(|dir| {
                std::fs::read_to_string(dir.join("index.html"))
                    .map(Arc::new)
                    .map_err(|error| format!("cannot read Genesis panel index.html: {error}"))
            })
            .transpose()?;
        if config
            .panel_index
            .as_ref()
            .is_some_and(|html| !html.contains("<base href=\"/\">"))
        {
            return Err("Genesis panel index.html must contain <base href=\"/\">".to_owned());
        }
        config.backup_dir = env::var_os("GENESIS_BACKUP_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        config.sensitive_devices = parse_sensitive_devices(
            env::var("GENESIS_SENSITIVE_DEVICES")
                .unwrap_or_default()
                .as_str(),
        )?;
        if config
            .read_token
            .as_ref()
            .is_some_and(|token| token.len() < 32)
        {
            return Err("GENESIS_READ_TOKEN must have at least 32 characters".to_owned());
        }
        if config
            .write_token
            .as_ref()
            .is_some_and(|token| token.len() < 32)
        {
            return Err("GENESIS_WRITE_TOKEN must have at least 32 characters".to_owned());
        }
        for (name, token) in [
            ("GENESIS_MEMBER_TOKEN", &config.member_token),
            ("GENESIS_GUEST_TOKEN", &config.guest_token),
        ] {
            if token.as_ref().is_some_and(|value| value.len() < 32) {
                return Err(format!("{name} must have at least 32 characters"));
            }
        }
        let tokens = [
            config.read_token.as_deref(),
            config.write_token.as_deref(),
            config.member_token.as_deref(),
            config.guest_token.as_deref(),
        ];
        for left in 0..tokens.len() {
            for right in (left + 1)..tokens.len() {
                if tokens[left].is_some() && tokens[left] == tokens[right] {
                    return Err("Genesis access tokens must be distinct".to_owned());
                }
            }
        }
        if config.household_id.is_empty() || config.household_id.len() > 128 {
            return Err("GENESIS_HOUSEHOLD_ID must have 1-128 characters".to_owned());
        }
        Ok(config)
    }

    fn from_values(bind_addr: Option<&str>, log_filter: Option<&str>) -> Result<Self, String> {
        let bind_addr = bind_addr
            .unwrap_or("127.0.0.1:8080")
            .parse()
            .map_err(|_| "GENESIS_BIND_ADDR must be a valid IP address and port".to_owned())?;
        let log_filter = EnvFilter::try_new(log_filter.unwrap_or("genesis_core=info"))
            .map_err(|_| "GENESIS_LOG must be a valid tracing filter".to_owned())?;
        Ok(Self {
            bind_addr,
            log_filter,
            ledger_path: PathBuf::from("./genesis-ledger.sqlite3"),
            read_token: None,
            write_token: None,
            member_token: None,
            guest_token: None,
            household_id: "pilot-home".to_owned(),
            sensitive_devices: Vec::new(),
            backup_dir: None,
            panel_dir: None,
            panel_index: None,
        })
    }
}

/// Zariadenia, ktoré prevádzkovateľ označil za citlivé. Genesis nevie, čo je za
/// zásuvkou; vie to ten, kto ju zapojil.
fn parse_sensitive_devices(raw: &str) -> Result<Vec<String>, String> {
    let mut devices = Vec::new();
    for entry in raw
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if !valid_opaque_id(entry) {
            return Err(
                "GENESIS_SENSITIVE_DEVICES must be a comma-separated list of device ids".to_owned(),
            );
        }
        devices.push(entry.to_owned());
    }
    Ok(devices)
}

/// Rovnaké pravidlo pre identifikátor, aké prijme ledger. Čo by neprešlo tam,
/// nemá zmysel pustiť ani sem.
fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

#[derive(Clone)]
struct AppState {
    inventory: Inventory,
    read_token: Option<String>,
    write_token: Option<String>,
    member_token: Option<String>,
    guest_token: Option<String>,
    household_id: String,
    sensitive_devices: Arc<Vec<String>>,
    backup_dir: Option<Arc<PathBuf>>,
    panel_dir: Option<PathBuf>,
    panel_index: Option<Arc<String>>,
    ledger: Arc<Mutex<Ledger>>,
    ha_config: Option<HaConfig>,
    ha_link: Link,
    ha_registry: Registry,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewCommand {
    household_id: String,
    device_id: String,
    value: bool,
    idempotency_key: String,
    correlation_id: String,
}

fn app(state: AppState) -> Router {
    let panel_dir = state.panel_dir.clone();
    let mut router = Router::new()
        .route("/health", get(health))
        .route("/v1/me", get(me))
        .route("/v1/devices", get(devices))
        .route("/v1/inventory", get(inventory))
        .route("/v1/commands", get(list_commands).post(create_command))
        .route("/v1/commands/{command_id}", get(get_command))
        .route("/v1/diagnostics", get(diagnostics))
        .route("/v1/access", get(access))
        .route("/v1/voice/commands", axum::routing::post(voice_command))
        .route(
            "/v1/voice/confirmations",
            axum::routing::post(voice_confirmation),
        )
        .route("/v1/voice/audit", get(voice_audit))
        .route("/v1/pairings", axum::routing::post(create_pairing))
        .route("/v1/pairings/redeem", axum::routing::post(redeem_pairing))
        .route("/v1/credentials", get(list_credentials))
        .route("/v1/backup", axum::routing::post(create_backup))
        .route(
            "/v1/credentials/{credential_id}",
            axum::routing::delete(revoke_credential),
        );
    if panel_dir.is_some() {
        router = router.route("/", get(panel_home));
    }
    let router = router.with_state(state);
    if let Some(dir) = panel_dir {
        router.fallback_service(ServeDir::new(dir))
    } else {
        router
    }
}

async fn panel_home(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let html = state
        .panel_index
        .as_ref()
        .expect("panel index validated at startup");
    let base = headers
        .get("x-ingress-path")
        .and_then(|value| value.to_str().ok())
        .filter(|path| {
            path.starts_with('/')
                && path.len() <= 256
                && path
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/_-".contains(&byte))
        })
        .map(|path| format!("{}/", path.trim_end_matches('/')))
        .unwrap_or_else(|| "/".to_owned());
    Html(html.replacen("<base href=\"/\">", &format!("<base href=\"{base}\">"), 1))
}

async fn ingress_guard(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if peer.ip() != IpAddr::V4(Ipv4Addr::new(172, 30, 32, 2)) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(request).await)
}

fn with_ingress_guard(router: Router, ingress_only: bool) -> Router {
    if ingress_only {
        router.layer(middleware::from_fn(ingress_guard))
    } else {
        router
    }
}

/// Zariadenie tak, ako ho vydá API: pozorovaný stav plus miestnosť z registrov.
///
/// Miestnosť sa spája až tu. Inventár drží pozorovaný stav a mení sa s každou
/// udalosťou; miestnosti sa menia zriedka a prichádzajú z iného zdroja. Keby
/// `area_id` žilo v samotnom zariadení, každá zmena stavu by ho musela niesť so
/// sebou a presun jednej entity by znamenal prepísať celý inventár.
#[derive(Serialize)]
struct DeviceView {
    #[serde(flatten)]
    device: ha::Device,
    /// Chýba, keď zariadenie v Home Assistante miestnosť nemá. Je to legitímny
    /// stav domácnosti, nie chyba.
    area_id: Option<String>,
    area_name: Option<String>,
}

fn device_views(devices: Vec<ha::Device>, rooms: &RegistrySnapshot) -> Vec<DeviceView> {
    let names: BTreeMap<&str, &str> = rooms
        .areas
        .iter()
        .map(|area| (area.area_id.as_str(), area.name.as_str()))
        .collect();
    devices
        .into_iter()
        .map(|device| {
            let area_id = rooms.area_of_device.get(&device.device_id).cloned();
            let area_name = area_id
                .as_deref()
                .and_then(|id| names.get(id))
                .map(|name| (*name).to_owned());
            DeviceView {
                device,
                area_id,
                area_name,
            }
        })
        .collect()
}

async fn devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceView>>, StatusCode> {
    principal(&headers, &state).await?;
    let rooms = state.ha_registry.snapshot().await;
    Ok(Json(device_views(state.inventory.devices().await, &rooms)))
}

#[derive(Serialize)]
struct InventoryView {
    household: HouseholdView,
    areas: Vec<AreaView>,
    devices: Vec<DeviceView>,
    home_assistant: InventorySource,
}

#[derive(Serialize)]
struct HouseholdView {
    household_id: String,
    /// Názov domácnosti z Home Assistanta. Chýba, kým sedenie nenačíta
    /// konfiguráciu, a panel si ho vtedy nesmie domyslieť.
    name: Option<String>,
}

#[derive(Serialize)]
struct AreaView {
    area_id: String,
    name: String,
    /// Zariadenia v tejto miestnosti. Prázdna miestnosť sa neskrýva — v
    /// domácnosti existuje aj vtedy, keď v nej zatiaľ nič nie je.
    device_ids: Vec<String>,
}

#[derive(Serialize)]
struct InventorySource {
    /// Stav prepojenia. Je v tejto odpovedi preto, aby sa prázdny inventár nedal
    /// prečítať ako prázdna domácnosť: bez neho „nič nevidíme" a „nič tam nie je"
    /// vyzerajú úplne rovnako.
    state: LinkState,
    /// Registre sa nepodarilo prečítať celé, takže miestnosti chýbajú aj vtedy,
    /// keď ich domácnosť má. Stáva sa to pri tokene bez administrátorských práv.
    rooms_incomplete: bool,
}

/// Domácnosť, jej miestnosti a jej zariadenia v jednej odpovedi.
///
/// V jednej preto, že miestnosti a zariadenia musia byť z toho istého okamihu.
/// Dvoma dotazmi sa dá dostať zoznam miestností a k nemu zariadenie, ktoré
/// ukazuje do miestnosti, čo medzitým zanikla.
///
/// Čítanie stačí s ktorýmkoľvek platným tokenom, rovnako ako `GET /v1/access`:
/// rola rozhoduje o ovládaní, nie o tom, či člen domácnosti vidí, čo v nej je.
/// Domácnosť je jedna na jednotku a token je na ňu naviazaný, takže iná
/// domácnosť sa do odpovede dostať nemôže.
async fn inventory(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<InventoryView>, StatusCode> {
    principal(&headers, &state).await?;
    let rooms = state.ha_registry.snapshot().await;
    let devices = device_views(state.inventory.devices().await, &rooms);
    let areas = rooms
        .areas
        .iter()
        .map(|area| AreaView {
            area_id: area.area_id.clone(),
            name: area.name.clone(),
            device_ids: devices
                .iter()
                .filter(|view| view.area_id.as_deref() == Some(area.area_id.as_str()))
                .map(|view| view.device.device_id.clone())
                .collect(),
        })
        .collect();
    Ok(Json(InventoryView {
        household: HouseholdView {
            household_id: state.household_id.clone(),
            name: rooms.location_name.clone(),
        },
        areas,
        devices,
        home_assistant: InventorySource {
            state: state.ha_link.status().await.state,
            rooms_incomplete: rooms.partial,
        },
    }))
}

#[derive(Serialize)]
struct Principal {
    household_id: String,
    actor_id: String,
    role: Role,
    can_control_devices: bool,
    /// Vyplnené pri vydanej kreditíve. Tokeny z konfigurácie kreditívu nemajú,
    /// pretože ich nikto nevydal — sú bootstrapom tejto jednotky.
    credential_id: Option<String>,
}

/// Kto požiadavku poslal.
///
/// Najprv sa skúšajú tokeny z konfigurácie; sú bootstrapom pilota a nesiahajú do
/// databázy. Až potom sa hľadá vydaná kreditíva, a to podľa odtlačku, nie podľa
/// tajomstva.
async fn principal(headers: &HeaderMap, state: &AppState) -> Result<Principal, StatusCode> {
    let tokens = [
        (state.write_token.as_deref(), Role::Owner, "pilot-owner"),
        (state.member_token.as_deref(), Role::Member, "pilot-member"),
        (state.guest_token.as_deref(), Role::Guest, "pilot-guest"),
        (state.read_token.as_deref(), Role::Service, "pilot-service"),
    ];
    // Jednotka bez jediného nastaveného tokenu nie je nakonfigurovaná, nie
    // neoprávnená; bez hlavičky sa to inak nedá rozlíšiť.
    let configured = tokens.iter().any(|(token, _, _)| token.is_some());
    let supplied = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "))
        .ok_or(if configured {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    for (token, role, actor_id) in tokens {
        if token.is_some_and(|value| supplied.ct_eq(value.as_bytes()).unwrap_u8() == 1) {
            return Ok(Principal {
                household_id: state.household_id.clone(),
                actor_id: actor_id.to_owned(),
                role,
                can_control_devices: role.can_control_devices(),
                credential_id: None,
            });
        }
    }
    // Zámok sa drží iba na dobu overenia a pustí sa pred návratom, aby ho mohla
    // obsluha požiadavky vzápätí vziať znova.
    let identity = {
        let mut ledger = state.ledger.lock().await;
        identity::authenticate(&mut ledger, &state.household_id, supplied, Utc::now()).map_err(
            |error| {
                tracing::error!(%error, "checking the presented credential failed");
                StatusCode::INTERNAL_SERVER_ERROR
            },
        )?
    };
    match identity {
        Some(identity) => Ok(Principal {
            household_id: identity.household_id,
            actor_id: identity.actor_id,
            role: identity.role,
            can_control_devices: identity.role.can_control_devices(),
            credential_id: Some(identity.credential_id),
        }),
        // Predložené tajomstvo nepatrí ani konfigurácii, ani žiadnej platnej
        // kreditíve. Na nenakonfigurovanej jednotke to nie je otázka oprávnenia.
        None if !configured => Err(StatusCode::SERVICE_UNAVAILABLE),
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Principal>, StatusCode> {
    Ok(Json(principal(&headers, &state).await?))
}

/// Posledný potvrdený stav a otvorené incidenty časovo obmedzených grantov.
///
/// Prehľad je iba na čítanie, takže stačí ktorýkoľvek platný token; zosúladenie
/// beží v plánovači, nie z tejto požiadavky.
async fn access(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AccessState>>, StatusCode> {
    principal(&headers, &state).await?;
    let ledger = state.ledger.lock().await;
    grant::overview(&ledger, &state.household_id)
        .map(Json)
        .map_err(|error| {
            tracing::error!(%error, "reading the timed access overview failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn create_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<NewCommand>,
) -> Result<Json<Snapshot>, StatusCode> {
    let caller = authorized_controller(&headers, &state, &input.household_id)
        .await
        .map_err(|refusal| refusal.status)?;
    let ha_config = state
        .ha_config
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let mut ledger = state.ledger.lock().await;
    let snapshot = ha_command::run_power_command(
        ha_config,
        &state.inventory,
        &mut ledger,
        CommandRequest {
            household_id: input.household_id,
            command_id: format!("cmd:{}", Uuid::new_v4()),
            device_id: input.device_id,
            capability_id: "power".to_owned(),
            value: Value::Bool(input.value),
            actor: caller,
            idempotency_key: input.idempotency_key,
            correlation_id: input.correlation_id,
        },
    )
    .await
    .map_err(map_execution_error)?;
    Ok(Json(snapshot))
}

/// Maximálna dĺžka prepisu. Hlasový povel je krátky; dlhší text je chyba
/// klienta, nie povel.
const MAX_TRANSCRIPT: usize = 200;

/// Identifikátor potvrdenia je UUID; dlhší vstup netreba ani čítať.
const MAX_CONFIRMATION_ID: usize = 64;

/// Kód aj token majú 64 hexadecimálnych znakov; dlhší vstup netreba čítať.
const MAX_SECRET: usize = 128;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewPairing {
    household_id: String,
    role: Role,
    /// Koho bude vydaná kreditíva predstavovať. Vďaka tomu audit vie povedať
    /// kto, nie iba akou rolou.
    actor_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeemPairing {
    household_id: String,
    code: String,
}

/// Spustí párovanie a vráti jednorazový kód.
///
/// Kód je v odpovedi práve raz. Do logu nepatrí a v databáze je iba jeho
/// odtlačok, takže ho Genesis už nikdy nevie zopakovať — kto ho stratí, spraví
/// nové párovanie.
async fn create_pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<NewPairing>,
) -> Result<(StatusCode, Json<identity::StartedPairing>), StatusCode> {
    let owner = household_owner(&headers, &state).await?;
    if input.household_id != state.household_id {
        return Err(StatusCode::FORBIDDEN);
    }
    if !valid_opaque_id(&input.actor_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut ledger = state.ledger.lock().await;
    let pairing = identity::start_pairing(
        &mut ledger,
        &state.household_id,
        input.role,
        &input.actor_id,
        &owner.actor_id,
        Utc::now(),
    )
    .map_err(map_identity_error)?;
    tracing::info!(
        pairing_id = %pairing.pairing_id,
        role = pairing.role.as_str(),
        actor_id = %pairing.actor_id,
        "pairing started"
    );
    Ok((StatusCode::CREATED, Json(pairing)))
}

/// Vymení jednorazový kód za prístupový token.
///
/// Nepotrebuje token, pretože kód sám je oprávnenie. Nepoužiteľný kód vracia 422
/// bez toho, aby prezradil, čo mu chýba: či neexistuje, uplynul, bol použitý
/// alebo patrí inej domácnosti.
async fn redeem_pairing(
    State(state): State<AppState>,
    Json(input): Json<RedeemPairing>,
) -> Result<Json<identity::IssuedCredential>, StatusCode> {
    if input.code.is_empty() || input.code.len() > MAX_SECRET {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut ledger = state.ledger.lock().await;
    let issued = identity::redeem(&mut ledger, &input.household_id, &input.code, Utc::now())
        .map_err(map_identity_error)?
        .filter(|issued| issued.household_id == state.household_id)
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    tracing::info!(
        credential_id = %issued.credential_id,
        role = issued.role.as_str(),
        actor_id = %issued.actor_id,
        "credential issued"
    );
    Ok(Json(issued))
}

/// Vydané kreditívy domácnosti. Token medzi nimi nie je, pretože ho Genesis
/// nemá — v databáze je iba odtlačok.
async fn list_credentials(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<identity::CredentialView>>, StatusCode> {
    household_owner(&headers, &state).await?;
    let ledger = state.ledger.lock().await;
    identity::credentials(&ledger, &state.household_id)
        .map(Json)
        .map_err(map_identity_error)
}

/// Odoberie prístup. Ďalšia požiadavka s tým tokenom je 401.
async fn revoke_credential(
    State(state): State<AppState>,
    Path(credential_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let owner = household_owner(&headers, &state).await?;
    if credential_id.is_empty() || credential_id.len() > MAX_SECRET {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut ledger = state.ledger.lock().await;
    let revoked = identity::revoke(
        &mut ledger,
        &state.household_id,
        &credential_id,
        &owner.actor_id,
        Utc::now(),
    )
    .map_err(map_identity_error)?;
    if !revoked {
        return Err(StatusCode::NOT_FOUND);
    }
    tracing::info!(credential_id = %credential_id, "credential revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// Vypíše konzistentnú zálohu databázy.
///
/// Zálohu robí SQLite `VACUUM INTO`, takže je celá a platná aj keď sa práve
/// zapisuje — na rozdiel od `cp` cez otvorený súbor. Obnova sem nepatrí: služba
/// nedokáže bezpečne podsunúť súbor sama sebe. Postup je v `core/README.md`.
async fn create_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<backup::Backup>), StatusCode> {
    household_owner(&headers, &state).await?;
    let directory = state
        .backup_dir
        .as_deref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let ledger = state.ledger.lock().await;
    let written = backup::write(&ledger, directory, Utc::now()).map_err(|error| {
        tracing::error!(%error, "writing the backup failed");
        match error {
            backup::BackupError::AlreadyExists => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    })?;
    tracing::info!(path = %written.path, bytes = written.bytes, "backup written");
    Ok((StatusCode::CREATED, Json(written)))
}

/// Vlastník domácnosti. Párovanie ani odobranie prístupu nie je nič, čo by mal
/// robiť člen, hosť alebo služba.
async fn household_owner(headers: &HeaderMap, state: &AppState) -> Result<Principal, StatusCode> {
    let caller = principal(headers, state).await?;
    if caller.role != Role::Owner {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(caller)
}

fn map_identity_error(error: identity::IdentityError) -> StatusCode {
    tracing::error!(%error, "the credential store failed");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpokenCommand {
    household_id: String,
    /// Text z rozpoznávania reči. Genesis neprijíma zvuk, takže pole pre audio
    /// neexistuje a telo, ktoré ho nesie, sa odmietne.
    transcript: String,
    idempotency_key: String,
    correlation_id: String,
    /// Súhlas používateľa s uložením prepisu. Bez neho sa prepis neuloží.
    #[serde(default)]
    store_transcript: bool,
}

/// Hlasový povel pre jedno zariadenie.
///
/// Ide tou istou autorizáciou aj tým istým execution ledgerom ako panel; líši
/// sa len tým, že cieľ a hodnotu treba najprv odvodiť z prepisu. Nejednoznačný
/// povel sa nevykoná a odpoveď to priznáva stavovým kódom, nie iba telom.
async fn voice_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SpokenCommand>,
) -> Result<(StatusCode, Json<voice::Outcome>), StatusCode> {
    let caller = authorized_speaker(&headers, &state, &input.household_id).await?;
    let transcript = input.transcript.trim();
    if transcript.is_empty() || transcript.chars().count() > MAX_TRANSCRIPT {
        return Err(StatusCode::BAD_REQUEST);
    }
    let ha_config = state
        .ha_config
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let mut ledger = state.ledger.lock().await;
    let outcome = voice::execute(
        ha_config,
        &state.inventory,
        &mut ledger,
        &state.household_id,
        caller,
        &voice::Spoken {
            transcript: transcript.to_owned(),
            store_transcript: input.store_transcript,
            idempotency_key: input.idempotency_key,
            correlation_id: input.correlation_id,
        },
        &state.sensitive_devices,
        Utc::now(),
    )
    .await
    .map_err(map_execution_error)?;
    Ok(answer(outcome))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpokenConfirmation {
    household_id: String,
    confirmation_id: String,
}

/// Potvrdenie citlivej hlasovej akcie.
///
/// Potvrdenie platí raz, krátko a iba pre toho, kto o akciu požiadal. Zamietnutie
/// sa vracia s 422, nie 200, a do auditu ide presnejšie, než sa povie volajúcemu.
async fn voice_confirmation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SpokenConfirmation>,
) -> Result<(StatusCode, Json<voice::Outcome>), StatusCode> {
    let caller = authorized_speaker(&headers, &state, &input.household_id).await?;
    if input.confirmation_id.is_empty() || input.confirmation_id.len() > MAX_CONFIRMATION_ID {
        return Err(StatusCode::BAD_REQUEST);
    }
    let ha_config = state
        .ha_config
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let mut ledger = state.ledger.lock().await;
    let outcome = voice::confirm(
        ha_config,
        &state.inventory,
        &mut ledger,
        &state.household_id,
        caller,
        &input.confirmation_id,
        Utc::now(),
    )
    .await
    .map_err(map_execution_error)?;
    Ok(answer(outcome))
}

/// Audit hlasových akcií: čo sa rozhodlo, kým a prečo.
///
/// Iba na čítanie, takže stačí ktorýkoľvek platný token. Odpoveď neobsahuje
/// prepis ani identifikátor potvrdenia — audit dokladá rozhodnutie, nie obsah.
async fn voice_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<voice::AuditEvent>>, StatusCode> {
    principal(&headers, &state).await?;
    let ledger = state.ledger.lock().await;
    voice::audit_trail(&ledger, &state.household_id)
        .map(Json)
        .map_err(|error| {
            tracing::error!(%error, "reading the voice audit failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// Stavový kód k výsledku. Iba vykonaný povel je 200; čokoľvek iné sa nesmie dať
/// pochopiť ako hotovo. Log nesie dôvod, nikdy nie to, čo bolo povedané.
fn answer(outcome: voice::Outcome) -> (StatusCode, Json<voice::Outcome>) {
    let status = match &outcome {
        voice::Outcome::Executed { .. } => StatusCode::OK,
        voice::Outcome::ConfirmationRequired { .. } => StatusCode::ACCEPTED,
        voice::Outcome::Unclear(unclear) => {
            tracing::info!(reason = unclear.reason, "voice command not executed");
            StatusCode::UNPROCESSABLE_ENTITY
        }
        voice::Outcome::Refused { explanation } => {
            tracing::info!(reason = explanation.code, "voice command refused");
            StatusCode::UNPROCESSABLE_ENTITY
        }
    };
    (status, Json(outcome))
}

/// Zamietnutie ovládania. `attributable` je vyplnené len vtedy, keď token
/// platil — bez platného tokenu Genesis nevie, komu by zápis pripísal, a audit
/// od anonymných volajúcich by sa dal beztrestne nafúknuť.
struct ControlRefusal {
    status: StatusCode,
    attributable: Option<(String, &'static str)>,
}

/// Aktér, ktorý smie ovládať zariadenia v pilotnej domácnosti. Panel aj hlas
/// prechádzajú touto kontrolou a identitu aktéra určuje server.
async fn authorized_controller(
    headers: &HeaderMap,
    state: &AppState,
    household_id: &str,
) -> Result<Actor, ControlRefusal> {
    let caller = principal(headers, state)
        .await
        .map_err(|status| ControlRefusal {
            status,
            attributable: None,
        })?;
    if !caller.can_control_devices {
        return Err(ControlRefusal {
            status: StatusCode::FORBIDDEN,
            attributable: Some((caller.actor_id, voice::ROLE_NOT_PERMITTED)),
        });
    }
    if household_id != state.household_id {
        return Err(ControlRefusal {
            status: StatusCode::FORBIDDEN,
            attributable: Some((caller.actor_id, voice::OTHER_HOUSEHOLD)),
        });
    }
    Ok(Actor {
        actor_type: ActorType::User,
        actor_id: caller.actor_id,
    })
}

/// Autorizácia hlasovej akcie. Na rozdiel od panela sa pripísateľné zamietnutie
/// zapíše do auditu, aby sa dalo doložiť, že a prečo asistent odmietol.
async fn authorized_speaker(
    headers: &HeaderMap,
    state: &AppState,
    household_id: &str,
) -> Result<Actor, StatusCode> {
    match authorized_controller(headers, state, household_id).await {
        Ok(actor) => Ok(actor),
        Err(refusal) => {
            if let Some((actor_id, reason)) = refusal.attributable {
                let mut ledger = state.ledger.lock().await;
                voice::record_refusal(
                    &mut ledger,
                    &state.household_id,
                    &actor_id,
                    reason,
                    Utc::now(),
                );
            }
            Err(refusal.status)
        }
    }
}

async fn get_command(
    State(state): State<AppState>,
    Path(command_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Snapshot>, StatusCode> {
    principal(&headers, &state).await?;
    let ledger = state.ledger.lock().await;
    let snapshot = ledger
        .get(&command_id)
        .map_err(map_ledger_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if snapshot.request.household_id != state.household_id {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(snapshot))
}

/// Koľko povelov vydá prehľad naraz.
///
/// Snapshot sa deserializuje po jednom, takže strop je tu preto, aby odpoveď
/// nerástla s databázou — pilotná domácnosť ich za týždeň nazbiera viac, než sa
/// dá zmysluplne prečítať na jednej obrazovke.
const COMMAND_PAGE_DEFAULT: usize = 20;
const COMMAND_PAGE_LIMIT: usize = 50;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandPage {
    limit: Option<usize>,
}

/// Posledné povely domácnosti, najnovší prvý.
///
/// Bez tohto sa dá zistiť len stav povelu, ktorého identifikátor už niekto má.
/// Po obnovení panela alebo po reštarte jednotky ten identifikátor nikto nemá,
/// takže neistý povel by zostal ležať bez toho, aby o ňom niekto vedel.
async fn list_commands(
    State(state): State<AppState>,
    Query(page): Query<CommandPage>,
    headers: HeaderMap,
) -> Result<Json<Vec<Snapshot>>, StatusCode> {
    principal(&headers, &state).await?;
    let limit = page
        .limit
        .unwrap_or(COMMAND_PAGE_DEFAULT)
        .clamp(1, COMMAND_PAGE_LIMIT);
    let ledger = state.ledger.lock().await;
    let commands = ledger
        .recent(&state.household_id, limit)
        .map_err(map_ledger_error)?;
    Ok(Json(commands))
}

#[derive(Serialize)]
struct Diagnostics {
    unit: UnitDiagnostics,
    home_assistant: LinkStatus,
}

#[derive(Serialize)]
struct UnitDiagnostics {
    version: &'static str,
    household_id: String,
    /// Či je prepojenie na Home Assistant vôbec nastavené. Jednotka bez neho
    /// nie je pokazená, len nespojená, a panel to má povedať inak.
    home_assistant_configured: bool,
}

/// Diagnostika je zámerne niečo iné než `/health`.
///
/// `/health` odpovedá na jednu otázku — či beží HTTP server — a musí zostať bez
/// tokenu, pretože ho volá watchdog Supervisora. Stav prepojenia na Home
/// Assistant sa z neho prečítať nedá a ani nemá: je to údaj o domácnosti, a
/// keby ho `/health` niesol, čítal by ho každý, kto sa dostane na port.
async fn diagnostics(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Diagnostics>, StatusCode> {
    principal(&headers, &state).await?;
    Ok(Json(Diagnostics {
        unit: UnitDiagnostics {
            version: env!("CARGO_PKG_VERSION"),
            household_id: state.household_id.clone(),
            home_assistant_configured: state.ha_config.is_some(),
        },
        home_assistant: state.ha_link.status().await,
    }))
}

fn map_execution_error(error: ExecutionError) -> StatusCode {
    match error {
        ExecutionError::UnknownDevice => StatusCode::NOT_FOUND,
        ExecutionError::NotExecutable => StatusCode::CONFLICT,
        ExecutionError::Ledger(error) => map_ledger_error(error),
    }
}

fn map_ledger_error(error: LedgerError) -> StatusCode {
    match error {
        LedgerError::Invalid(_) => StatusCode::BAD_REQUEST,
        LedgerError::IdempotencyConflict => StatusCode::CONFLICT,
        LedgerError::NotFound => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "genesis-core",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// Grant sa musí vrátiť späť aj vtedy, keď v čase expirácie nikto nevolá API,
/// takže zosúladenie beží z vlastného plánovača. Tik určuje, o koľko neskôr než
/// `expires_at` sa zariadenie zamkne a ako často sa skúša neistý relock; pilot
/// beží s jednou domácnosťou, takže prechod drží zámok ledgeru rovnako ako
/// obsluha povelu.
const RECONCILE_TICK: Duration = Duration::from_secs(30);

async fn reconcile_sweeper(config: HaConfig, inventory: Inventory, ledger: Arc<Mutex<Ledger>>) {
    let mut ticker = tokio::time::interval(RECONCILE_TICK);
    loop {
        ticker.tick().await;
        let mut ledger = ledger.lock().await;
        match ha_command::reconcile(&config, &inventory, &mut ledger, Utc::now()).await {
            Ok(settled) => {
                for grant in settled {
                    tracing::info!(
                        decision_id = %grant.decision_id,
                        state = ?grant.state,
                        "timed access reconciled"
                    );
                }
            }
            Err(error) => tracing::warn!(%error, "reconciliation pass failed"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let ledger = Arc::new(Mutex::new(Ledger::open(&config.ledger_path)?));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(config.log_filter)
        .init();

    // Po štarte nie je nič na ceste: prerušené povely sa priznajú ako neznáme a
    // grant sa radšej považuje za otvorený, než by zostal bez zámku. Beží to aj
    // bez Home Assistanta, pretože samo nič neposiela.
    for resumed in grant::resume(&mut *ledger.lock().await, Utc::now())? {
        tracing::warn!(
            decision_id = %resumed.decision_id,
            state = ?resumed.state,
            "timed access resumed after a restart"
        );
    }
    // Granty si svoje povely prevzali vyššie a spravili pri tom viac — incident
    // a rozhodnutie o stave grantu. Toto doberie ostatné, teda panel a hlas, aby
    // po reštarte nezostal otvorený povel, ktorý sa už nedokončí.
    for command_id in ledger.lock().await.adopt_interrupted(
        Actor {
            actor_type: ActorType::Service,
            actor_id: "genesis-core".to_owned(),
        },
        "interrupted_before_result",
    )? {
        tracing::warn!(%command_id, "command adopted as unknown after a restart");
    }

    let inventory = Inventory::new();
    let ha_config = HaConfig::from_env().map_err(std::io::Error::other)?;
    // Nenastavené prepojenie je iný stav než spadnuté. Jednotka bez HA tokenu
    // nie je pokazená a diagnostika to nesmie hlásiť ako poruchu.
    let ha_link = Link::new(if ha_config.is_some() {
        LinkState::Connecting
    } else {
        LinkState::NotConfigured
    });
    let ha_registry = Registry::new();
    if let Some(ha_config) = ha_config.clone() {
        tokio::spawn(ha::run(
            ha_config.clone(),
            inventory.clone(),
            ha_registry.clone(),
            ha_link.clone(),
        ));
        tokio::spawn(reconcile_sweeper(
            ha_config,
            inventory.clone(),
            Arc::clone(&ledger),
        ));
    }
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %listener.local_addr()?, "Genesis core listening");
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let router = app(AppState {
        inventory,
        read_token: config.read_token,
        write_token: config.write_token,
        member_token: config.member_token,
        guest_token: config.guest_token,
        household_id: config.household_id,
        sensitive_devices: Arc::new(config.sensitive_devices),
        backup_dir: config.backup_dir.map(Arc::new),
        panel_dir: config.panel_dir,
        panel_index: config.panel_index,
        ledger,
        ha_config,
        ha_link,
        ha_registry,
    });
    let router = with_ingress_guard(
        router,
        env::var("GENESIS_INGRESS_ONLY").ok().as_deref() == Some("true"),
    );
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    })
    .await?;
    tracing::info!("Genesis core stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use genesis_core::behavior_decision::BehaviorDecision;
    use tower::ServiceExt;

    fn test_state(read_token: Option<String>) -> AppState {
        AppState {
            inventory: Inventory::new(),
            read_token,
            write_token: None,
            member_token: None,
            guest_token: None,
            household_id: "pilot-home".to_owned(),
            sensitive_devices: Arc::new(Vec::new()),
            backup_dir: None,
            panel_dir: None,
            panel_index: None,
            ledger: Arc::new(Mutex::new(Ledger::open(":memory:").unwrap())),
            ha_config: None,
            ha_link: Link::default(),
            ha_registry: Registry::new(),
        }
    }

    #[test]
    fn config_defaults_to_loopback() {
        let config = Config::from_values(None, None).unwrap();
        assert_eq!(config.bind_addr, "127.0.0.1:8080".parse().unwrap());
    }

    #[test]
    fn config_rejects_invalid_bind_address() {
        assert!(Config::from_values(Some("0.0.0.0"), None).is_err());
    }

    #[test]
    fn sensitive_devices_are_parsed_and_validated() {
        assert!(parse_sensitive_devices("").unwrap().is_empty());
        assert_eq!(
            parse_sensitive_devices(" ha:switch.boiler , ha:switch.gate ").unwrap(),
            ["ha:switch.boiler", "ha:switch.gate"]
        );
        // Čokoľvek, čo by ledger neprijal ako identifikátor, zastaví štart.
        assert!(parse_sensitive_devices("ha:switch.boiler, nie platné").is_err());
        assert!(parse_sensitive_devices("-leading").is_err());
    }

    #[test]
    fn config_rejects_invalid_log_filter() {
        assert!(Config::from_values(None, Some("[")).is_err());
    }

    #[tokio::test]
    async fn devices_require_token() {
        let state = test_state(Some("a".repeat(32)));
        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/v1/devices")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app(state)
            .oneshot(
                Request::builder()
                    .uri("/v1/devices")
                    .header("authorization", format!("Bearer {}", "a".repeat(32)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn command_rejects_missing_token_and_other_household() {
        let mut state = test_state(Some("r".repeat(32)));
        state.write_token = Some("w".repeat(32));
        let body = serde_json::json!({
            "household_id": "other-home",
            "device_id": "ha:light.living",
            "value": true,
            "idempotency_key": "idem-1",
            "correlation_id": "corr-1"
        })
        .to_string();
        let unauthorized = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/commands")
                    .header("content-type", "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let forbidden = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/commands")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", "w".repeat(32)))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn roles_are_enforced_by_api_and_bound_to_household() {
        let mut state = test_state(Some("r".repeat(32)));
        state.write_token = Some("o".repeat(32));
        state.member_token = Some("m".repeat(32));
        state.guest_token = Some("g".repeat(32));
        for (token, role, controls) in [
            ("o".repeat(32), "owner", true),
            ("m".repeat(32), "member", true),
            ("g".repeat(32), "guest", false),
            ("r".repeat(32), "service", false),
        ] {
            let response = app(state.clone())
                .oneshot(
                    Request::builder()
                        .uri("/v1/me")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            let me: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(me["role"], role);
            assert_eq!(me["household_id"], "pilot-home");
            assert_eq!(me["can_control_devices"], controls);
        }
        let body = serde_json::json!({
            "household_id": "pilot-home", "device_id": "ha:light.living",
            "value": true, "idempotency_key": "idem-role", "correlation_id": "corr-role"
        })
        .to_string();
        for token in ["g".repeat(32), "r".repeat(32)] {
            let response = app(state.clone())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/commands")
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::from(body.clone()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let response = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/commands")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", "m".repeat(32)))
                    .body(Body::from(
                        serde_json::json!({
                            "household_id": "other-home", "device_id": "ha:light.living",
                            "value": true, "idempotency_key": "idem-other", "correlation_id": "corr-other"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// Otvorí grant priamo v ledgeri stavu, aby prehľad mal čo ukázať.
    async fn seed_grant(state: &AppState) -> String {
        let decision = BehaviorDecision::parse(
            &serde_json::json!({
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
                "valid_from": "2026-09-29T18:00:00Z",
                "expires_at": "2026-09-29T19:00:00Z",
                "reason_code": "goal_verified",
                "idempotency_key": "behavior:ff77bdb0",
                "required_confirmation": "device"
            })
            .to_string(),
        )
        .unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-29T18:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut ledger = state.ledger.lock().await;
        grant::open(&mut ledger, &decision, now)
            .unwrap()
            .decision_id
    }

    #[tokio::test]
    async fn access_overview_needs_a_token_and_shows_the_grant() {
        let state = test_state(Some("a".repeat(32)));
        let decision_id = seed_grant(&state).await;
        let unauthorized = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/v1/access")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let response = app(state)
            .oneshot(
                Request::builder()
                    .uri("/v1/access")
                    .header("authorization", format!("Bearer {}", "a".repeat(32)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let overview: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(overview[0]["grant"]["decision_id"], decision_id);
        assert_eq!(overview[0]["grant"]["state"], "granted");
        assert_eq!(overview[0]["required_confirmation"], "device");
        assert_eq!(overview[0]["close_attempts"], 0);
        assert!(overview[0]["last_confirmed"].is_null());
        assert_eq!(overview[0]["open_incidents"], serde_json::json!([]));
    }

    fn spoken_body(household_id: &str, transcript: &str) -> String {
        serde_json::json!({
            "household_id": household_id,
            "transcript": transcript,
            "idempotency_key": "voice-1",
            "correlation_id": "assist-1"
        })
        .to_string()
    }

    async fn post_voice(state: AppState, token: Option<&str>, body: String) -> StatusCode {
        let mut request = Request::builder()
            .method("POST")
            .uri("/v1/voice/commands")
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        app(state)
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn a_voice_command_needs_a_controlling_role_and_the_pilot_household() {
        let mut state = test_state(Some("r".repeat(32)));
        state.write_token = Some("o".repeat(32));
        state.member_token = Some("m".repeat(32));
        state.guest_token = Some("g".repeat(32));
        let body = spoken_body("pilot-home", "zapni svetlo");

        assert_eq!(
            post_voice(state.clone(), None, body.clone()).await,
            StatusCode::UNAUTHORIZED
        );
        // Hlas nedáva viac práv než panel: guest ani service neovládajú zariadenia.
        for token in ["g".repeat(32), "r".repeat(32)] {
            assert_eq!(
                post_voice(state.clone(), Some(&token), body.clone()).await,
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            post_voice(
                state.clone(),
                Some(&"m".repeat(32)),
                spoken_body("other-home", "zapni svetlo")
            )
            .await,
            StatusCode::FORBIDDEN
        );
        // Owner prejde autorizáciou aj kontrolou vstupu a zastaví sa až na tom,
        // že Home Assistant nie je nakonfigurovaný.
        assert_eq!(
            post_voice(state, Some(&"o".repeat(32)), body).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn a_voice_body_cannot_carry_audio_or_an_unusable_transcript() {
        let mut state = test_state(Some("r".repeat(32)));
        state.write_token = Some("o".repeat(32));
        let token = "o".repeat(32);

        // Genesis neprijíma zvuk, takže telo so zvukom nie je nepodstatné pole
        // navyše, ale zamietnutá požiadavka.
        let with_audio = serde_json::json!({
            "household_id": "pilot-home",
            "transcript": "zapni svetlo",
            "idempotency_key": "voice-1",
            "correlation_id": "assist-1",
            "audio": "UklGRg=="
        })
        .to_string();
        assert_eq!(
            post_voice(state.clone(), Some(&token), with_audio).await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            post_voice(
                state.clone(),
                Some(&token),
                spoken_body("pilot-home", "   ")
            )
            .await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_voice(
                state,
                Some(&token),
                spoken_body("pilot-home", &"a".repeat(MAX_TRANSCRIPT + 1))
            )
            .await,
            StatusCode::BAD_REQUEST
        );
    }

    async fn post_confirmation(state: AppState, token: Option<&str>, body: String) -> StatusCode {
        let mut request = Request::builder()
            .method("POST")
            .uri("/v1/voice/confirmations")
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        app(state)
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
            .status()
    }

    async fn audit_trail(state: AppState, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::builder().uri("/v1/voice/audit");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = app(state)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        let parsed = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, parsed)
    }

    fn confirmation_body(household_id: &str, confirmation_id: &str) -> String {
        serde_json::json!({
            "household_id": household_id,
            "confirmation_id": confirmation_id
        })
        .to_string()
    }

    #[tokio::test]
    async fn a_confirmation_needs_a_controlling_role_and_a_usable_id() {
        let mut state = test_state(Some("r".repeat(32)));
        state.write_token = Some("o".repeat(32));
        state.guest_token = Some("g".repeat(32));
        let body = confirmation_body("pilot-home", "5cf1d9f4-0000-4000-8000-000000000000");

        assert_eq!(
            post_confirmation(state.clone(), None, body.clone()).await,
            StatusCode::UNAUTHORIZED
        );
        // Potvrdiť citlivú akciu môže iba ten, kto smie ovládať zariadenia.
        assert_eq!(
            post_confirmation(state.clone(), Some(&"g".repeat(32)), body.clone()).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            post_confirmation(
                state.clone(),
                Some(&"o".repeat(32)),
                confirmation_body("pilot-home", "")
            )
            .await,
            StatusCode::BAD_REQUEST
        );
        // Owner prejde kontrolami a zastaví sa až na chýbajúcom Home Assistantovi.
        assert_eq!(
            post_confirmation(state, Some(&"o".repeat(32)), body).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn an_attributable_refusal_reaches_the_audit() {
        let mut state = test_state(Some("r".repeat(32)));
        state.write_token = Some("o".repeat(32));
        state.guest_token = Some("g".repeat(32));
        let body = spoken_body("pilot-home", "zapni svetlo");

        // Bez tokenu Genesis nevie, komu by zápis pripísal, takže nezapíše nič.
        assert_eq!(
            post_voice(state.clone(), None, body.clone()).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            audit_trail(state.clone(), Some(&"r".repeat(32))).await.1,
            serde_json::json!([])
        );

        // Guest je zamietnutý a to sa dá doložiť.
        assert_eq!(
            post_voice(state.clone(), Some(&"g".repeat(32)), body).await,
            StatusCode::FORBIDDEN
        );
        let (status, trail) = audit_trail(state.clone(), Some(&"r".repeat(32))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(trail[0]["decision"], "refused");
        assert_eq!(trail[0]["reason"], "role_not_permitted");
        assert_eq!(trail[0]["actor_id"], "pilot-guest");
        assert!(trail[1].is_null());

        // Audit je chránený tokenom ako každé čítanie.
        assert_eq!(audit_trail(state, None).await.0, StatusCode::UNAUTHORIZED);
    }

    /// Jedna požiadavka s tokenom alebo bez neho, s telom alebo bez.
    async fn call(
        state: AppState,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<String>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            None => request.body(Body::empty()).unwrap(),
        };
        let response = app(state).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 16384).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn pairing_and_revocation_walk_the_whole_way() {
        let mut state = test_state(Some("r".repeat(32)));
        state.write_token = Some("o".repeat(32));
        state.member_token = Some("m".repeat(32));
        let owner = "o".repeat(32);
        let wanted = serde_json::json!({
            "household_id": "pilot-home",
            "role": "member",
            "actor_id": "lukas"
        })
        .to_string();

        // Párovať smie iba vlastník.
        for token in [None, Some("m".repeat(32)), Some("r".repeat(32))] {
            let expected = if token.is_none() {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            };
            let status = call(
                state.clone(),
                "POST",
                "/v1/pairings",
                token.as_deref(),
                Some(wanted.clone()),
            )
            .await
            .0;
            assert_eq!(status, expected);
        }

        let (status, pairing) = call(
            state.clone(),
            "POST",
            "/v1/pairings",
            Some(&owner),
            Some(wanted),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let code = pairing["code"].as_str().unwrap().to_owned();
        assert_eq!(pairing["role"], "member");
        assert_eq!(pairing["actor_id"], "lukas");

        // Kód tejto domácnosti nevydá prístup k inej.
        let foreign =
            serde_json::json!({"household_id": "other-home", "code": code.clone()}).to_string();
        assert_eq!(
            call(
                state.clone(),
                "POST",
                "/v1/pairings/redeem",
                None,
                Some(foreign)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let redeem = serde_json::json!({"household_id": "pilot-home", "code": code}).to_string();
        let (status, issued) = call(
            state.clone(),
            "POST",
            "/v1/pairings/redeem",
            None,
            Some(redeem.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token = issued["token"].as_str().unwrap().to_owned();
        let credential_id = issued["credential_id"].as_str().unwrap().to_owned();

        // Jednorazový kód sa druhýkrát vymeniť nedá.
        assert_eq!(
            call(
                state.clone(),
                "POST",
                "/v1/pairings/redeem",
                None,
                Some(redeem)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        // Vydaný token funguje a nesie svojho aktéra, nie zástupné meno roly.
        let (status, me) = call(state.clone(), "GET", "/v1/me", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["role"], "member");
        assert_eq!(me["actor_id"], "lukas");
        assert_eq!(me["household_id"], "pilot-home");
        assert_eq!(me["credential_id"], credential_id);
        assert_eq!(me["can_control_devices"], true);

        // Vydaná kreditíva neovláda inú domácnosť, aj keď rolu na to má.
        let elsewhere = serde_json::json!({
            "household_id": "other-home",
            "device_id": "ha:light.living",
            "value": true,
            "idempotency_key": "idem-cross",
            "correlation_id": "corr-cross"
        })
        .to_string();
        assert_eq!(
            call(
                state.clone(),
                "POST",
                "/v1/commands",
                Some(&token),
                Some(elsewhere)
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );

        // Prehľad kreditív patrí vlastníkovi a token v ňom nie je.
        assert_eq!(
            call(state.clone(), "GET", "/v1/credentials", Some(&token), None)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let (status, listed) =
            call(state.clone(), "GET", "/v1/credentials", Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed[0]["credential_id"], credential_id);
        assert_eq!(listed[0]["actor_id"], "lukas");
        assert!(!serde_json::to_string(&listed).unwrap().contains(&token));

        // Odobrať smie vlastník, a potom je ten token 401.
        let path = format!("/v1/credentials/{credential_id}");
        assert_eq!(
            call(state.clone(), "DELETE", &path, Some(&token), None)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(state.clone(), "DELETE", &path, Some(&owner), None)
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            call(state.clone(), "GET", "/v1/me", Some(&token), None)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(state, "DELETE", &path, Some(&owner), None).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_backup_is_owner_only_and_lands_as_an_openable_database() {
        let mut state = test_state(Some("r".repeat(32)));
        state.write_token = Some("o".repeat(32));
        let owner = "o".repeat(32);

        assert_eq!(
            call(state.clone(), "POST", "/v1/backup", None, None)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(
                state.clone(),
                "POST",
                "/v1/backup",
                Some(&"r".repeat(32)),
                None
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        // Bez nastaveného priečinka nie je kam zálohovať.
        assert_eq!(
            call(state.clone(), "POST", "/v1/backup", Some(&owner), None)
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );

        let directory =
            std::env::temp_dir().join(format!("genesis-api-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        state.backup_dir = Some(Arc::new(directory.clone()));
        let (status, written) = call(state, "POST", "/v1/backup", Some(&owner), None).await;
        assert_eq!(status, StatusCode::CREATED);
        assert!(written["bytes"].as_u64().unwrap() > 0);
        // Záloha nie je len súbor — je to databáza, ktorú sa dá otvoriť.
        assert!(Ledger::open(written["path"].as_str().unwrap()).is_ok());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn an_unconfigured_unit_reports_unavailable_not_unauthorized() {
        let state = test_state(None);
        // Bez jediného nastaveného tokenu nie je čo autorizovať — ani keď
        // hlavička chýba, ani keď niečo nesie.
        for token in [None, Some("whatever")] {
            assert_eq!(
                call(state.clone(), "GET", "/v1/devices", token, None)
                    .await
                    .0,
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
    }

    /// Povel v ledgeri pre testy prehľadu. Pomenované polia zostávajú v teste,
    /// aby bolo vidieť, že prehľad nemieša domácnosti.
    async fn seed_command(state: &AppState, household_id: &str, command_id: &str, device_id: &str) {
        state
            .ledger
            .lock()
            .await
            .accept(CommandRequest {
                household_id: household_id.to_owned(),
                command_id: command_id.to_owned(),
                device_id: device_id.to_owned(),
                capability_id: "power".to_owned(),
                value: Value::Bool(true),
                actor: Actor {
                    actor_type: ActorType::User,
                    actor_id: "pilot-member".to_owned(),
                },
                idempotency_key: format!("key-{command_id}"),
                correlation_id: format!("corr-{command_id}"),
            })
            .unwrap();
    }

    #[tokio::test]
    async fn the_command_overview_is_scoped_newest_first_and_admits_an_uncertain_result() {
        let token = "c".repeat(32);
        let state = test_state(Some(token.clone()));
        seed_command(&state, "pilot-home", "cmd-one", "ha:light.hall").await;
        seed_command(&state, "pilot-home", "cmd-two", "ha:light.living").await;
        // Iná domácnosť je v tej istej databáze a do prehľadu sa dostať nesmie.
        seed_command(&state, "other-home", "cmd-three", "ha:light.attic").await;
        // Prvý povel sa prizná ako neistý. Prijatý bol ako prvý, ale hýbal sa
        // naposledy, a prehľad má viesť to, čo sa hýbalo naposledy.
        state
            .ledger
            .lock()
            .await
            .transition(
                "cmd-one",
                genesis_core::ledger::Status::Unknown,
                Actor {
                    actor_type: ActorType::Service,
                    actor_id: "genesis-core".to_owned(),
                },
                None,
                Some("no_result".to_owned()),
            )
            .unwrap();

        let (status, body) = call(state.clone(), "GET", "/v1/commands", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        let commands = body.as_array().unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0]["request"]["command_id"], "cmd-one");
        assert_eq!(commands[0]["status"], "unknown");
        assert_eq!(commands[0]["reason"], "no_result");
        assert_eq!(commands[0]["request"]["correlation_id"], "corr-cmd-one");
        assert_eq!(commands[1]["request"]["command_id"], "cmd-two");

        // Prehľad je za tokenom ako všetko ostatné o domácnosti.
        let (unauthorized, _) = call(state.clone(), "GET", "/v1/commands", None, None).await;
        assert_eq!(unauthorized, StatusCode::UNAUTHORIZED);
        // Strop drží aj vtedy, keď si ho volajúci nastaví sám.
        let (capped, body) =
            call(state, "GET", "/v1/commands?limit=5000", Some(&token), None).await;
        assert_eq!(capped, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 2);
    }

    fn pilot_light(device_id: &str, name: &str) -> ha::Device {
        ha::Device {
            device_id: device_id.to_owned(),
            provider: "home_assistant",
            provider_device_ref: device_id.trim_start_matches("ha:").to_owned(),
            name: name.to_owned(),
            capability_id: "power",
            capability_type: "switch",
            writable: true,
            power: Some(true),
            observed_at: Some("2026-09-30T09:05:00Z".to_owned()),
            availability: ha::Availability::Online,
        }
    }

    fn pilot_rooms() -> RegistrySnapshot {
        RegistrySnapshot {
            location_name: Some("Doma".to_owned()),
            areas: vec![
                ha::Area {
                    area_id: "ha:living_room".to_owned(),
                    name: "Obývačka".to_owned(),
                },
                // Miestnosť, v ktorej zatiaľ nič nie je. V domácnosti existuje.
                ha::Area {
                    area_id: "ha:bedroom".to_owned(),
                    name: "Spálňa".to_owned(),
                },
            ],
            area_of_device: [("ha:light.living".to_owned(), "ha:living_room".to_owned())]
                .into_iter()
                .collect(),
            partial: false,
        }
    }

    #[tokio::test]
    async fn the_inventory_joins_rooms_onto_devices_and_keeps_empty_rooms() {
        let token = "f".repeat(32);
        let mut state = test_state(Some(token.clone()));
        state.inventory = Inventory::with(vec![
            pilot_light("ha:light.living", "Veľké svetlo"),
            // Bez miestnosti — v Home Assistante ju nemá.
            pilot_light("ha:switch.plug", "Zásuvka"),
        ]);
        state.ha_registry = Registry::with(pilot_rooms());
        state.ha_link = Link::new(LinkState::Connected);

        let (status, body) = call(state.clone(), "GET", "/v1/inventory", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        // Domácnosť sa pomenúva podľa Home Assistanta, nie podľa panela.
        assert_eq!(body["household"]["household_id"], "pilot-home");
        assert_eq!(body["household"]["name"], "Doma");

        let areas = body["areas"].as_array().unwrap();
        assert_eq!(areas.len(), 2);
        assert_eq!(areas[0]["area_id"], "ha:living_room");
        assert_eq!(
            areas[0]["device_ids"],
            serde_json::json!(["ha:light.living"])
        );
        // Prázdna miestnosť zostáva v odpovedi.
        assert_eq!(areas[1]["area_id"], "ha:bedroom");
        assert_eq!(areas[1]["device_ids"], serde_json::json!([]));

        let devices = body["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0]["device_id"], "ha:light.living");
        assert_eq!(devices[0]["area_name"], "Obývačka");
        // Zariadenie bez miestnosti sa k žiadnej nepriradí, ani k vymyslenej.
        assert_eq!(devices[1]["device_id"], "ha:switch.plug");
        assert_eq!(devices[1]["area_id"], Value::Null);
        assert_eq!(devices[1]["area_name"], Value::Null);
        // Pôvodný tvar zariadenia zostáva; miestnosť sú dve nové polia.
        assert_eq!(devices[0]["availability"], "online");
        assert_eq!(devices[0]["power"], true);

        assert_eq!(body["home_assistant"]["state"], "connected");
        assert_eq!(body["home_assistant"]["rooms_incomplete"], false);

        // To isté pripojenie miestnosti vidí aj pôvodný inventár zariadení.
        let (status, body) = call(state, "GET", "/v1/devices", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[0]["area_name"], "Obývačka");
    }

    #[tokio::test]
    async fn an_empty_household_is_not_the_same_as_a_lost_connection() {
        let token = "g".repeat(32);
        // Spojené a naozaj prázdne: v domácnosti nie je žiadne svetlo ani zásuvka.
        let mut empty = test_state(Some(token.clone()));
        empty.ha_link = Link::new(LinkState::Connected);
        let (status, body) = call(empty, "GET", "/v1/inventory", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["devices"], serde_json::json!([]));
        assert_eq!(body["areas"], serde_json::json!([]));
        assert_eq!(body["home_assistant"]["state"], "connected");

        // Nespojené: zariadenia môžu existovať, len ich nevidíme. Odpoveď je
        // rovnako prázdna, a práve preto musí niesť stav prepojenia.
        let mut lost = test_state(Some(token.clone()));
        lost.ha_link = Link::new(LinkState::Disconnected);
        let (status, body) = call(lost, "GET", "/v1/inventory", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["devices"], serde_json::json!([]));
        assert_eq!(body["home_assistant"]["state"], "disconnected");

        // Registre neprečítané celé: domácnosť miestnosti má, my ich nevieme.
        let mut blind = test_state(Some(token.clone()));
        blind.ha_link = Link::new(LinkState::Connected);
        blind.inventory = Inventory::with(vec![pilot_light("ha:light.living", "Veľké svetlo")]);
        blind.ha_registry = Registry::with(RegistrySnapshot {
            partial: true,
            ..Default::default()
        });
        let (status, body) = call(blind, "GET", "/v1/inventory", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["areas"], serde_json::json!([]));
        assert_eq!(body["home_assistant"]["rooms_incomplete"], true);
        assert_eq!(body["household"]["name"], Value::Null);
        // Zariadenie je ovládateľné aj bez miestnosti.
        assert_eq!(body["devices"][0]["writable"], true);
    }

    #[tokio::test]
    async fn the_inventory_needs_a_token_and_a_guest_may_read_but_not_command() {
        let mut state = test_state(None);
        let guest = "h".repeat(32);
        state.guest_token = Some(guest.clone());
        state.inventory = Inventory::with(vec![pilot_light("ha:light.living", "Veľké svetlo")]);
        state.ha_registry = Registry::with(pilot_rooms());

        let (unauthorized, _) = call(state.clone(), "GET", "/v1/inventory", None, None).await;
        assert_eq!(unauthorized, StatusCode::UNAUTHORIZED);

        // Rola rozhoduje o ovládaní, nie o tom, či člen domácnosti vidí, čo v nej
        // je — rovnako ako pri prehľade prístupov.
        let (readable, body) =
            call(state.clone(), "GET", "/v1/inventory", Some(&guest), None).await;
        assert_eq!(readable, StatusCode::OK);
        assert_eq!(body["devices"][0]["area_name"], "Obývačka");

        let (forbidden, _) = call(
            state,
            "POST",
            "/v1/commands",
            Some(&guest),
            Some(
                serde_json::json!({
                    "household_id": "pilot-home",
                    "device_id": "ha:light.living",
                    "value": false,
                    "idempotency_key": "guest-1",
                    "correlation_id": "guest-1"
                })
                .to_string(),
            ),
        )
        .await;
        assert_eq!(forbidden, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_healthy_unit_does_not_claim_home_assistant_is_connected() {
        let token = "d".repeat(32);
        let state = test_state(Some(token.clone()));
        // `/health` je zelené a nemá o Home Assistantovi čo povedať; presne
        // preto stav prepojenia nesmie visieť na ňom.
        let health = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        let bytes = to_bytes(health.into_body(), 1024).await.unwrap();
        let health: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(health["status"], "ok");
        assert!(health.get("home_assistant").is_none());

        let (unauthorized, _) = call(state.clone(), "GET", "/v1/diagnostics", None, None).await;
        assert_eq!(unauthorized, StatusCode::UNAUTHORIZED);

        let (status, body) = call(state, "GET", "/v1/diagnostics", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        // Jednotka bez HA konfigurácie nie je pokazená, je nespojená — a hlási
        // to inak než spadnuté sedenie.
        assert_eq!(body["unit"]["home_assistant_configured"], false);
        assert_eq!(body["home_assistant"]["state"], "not_configured");
        assert_eq!(body["home_assistant"]["last_error"], Value::Null);
        assert_eq!(body["home_assistant"]["last_inventory_at"], Value::Null);
        assert_eq!(body["unit"]["household_id"], "pilot-home");
    }

    #[tokio::test]
    async fn diagnostics_reports_a_dropped_link_while_the_unit_still_answers() {
        let token = "e".repeat(32);
        let mut state = test_state(Some(token.clone()));
        state.ha_config = Some(HaConfig {
            websocket_url: "ws://supervisor/core/websocket".to_owned(),
            token: "supervisor-token".to_owned(),
        });
        state.ha_link = Link::new(LinkState::Disconnected);

        let (status, body) = call(state, "GET", "/v1/diagnostics", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["unit"]["home_assistant_configured"], true);
        assert_eq!(body["home_assistant"]["state"], "disconnected");
        assert_eq!(body["home_assistant"]["last_inventory_devices"], 0);
        // Diagnostika nesmie vydať ani adresu, ani token prepojenia.
        let serialized = body.to_string();
        assert!(!serialized.contains("supervisor-token"));
        assert!(!serialized.contains("ws://"));
    }

    #[tokio::test]
    async fn health_returns_json() {
        let response = app(test_state(None))
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "genesis-core");
    }

    #[tokio::test]
    async fn panel_index_uses_the_ingress_base_path() {
        let mut state = test_state(None);
        state.panel_index = Some(Arc::new(
            "<base href=\"/\"><script src=\"flutter_bootstrap.js\"></script>".to_owned(),
        ));
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ingress-path",
            "/api/hassio_ingress/pilot".parse().unwrap(),
        );
        let Html(html) = panel_home(State(state.clone()), headers).await;
        assert!(html.contains("<base href=\"/api/hassio_ingress/pilot/\">"));

        let mut forged = HeaderMap::new();
        forged.insert("x-ingress-path", "/\"><script>".parse().unwrap());
        let Html(html) = panel_home(State(state), forged).await;
        assert!(html.contains("<base href=\"/\">"));
    }

    #[tokio::test]
    async fn ingress_rejects_requests_outside_the_supervisor_proxy() {
        for (peer, expected) in [
            ("172.30.32.2:40000", StatusCode::OK),
            ("172.30.32.3:40000", StatusCode::FORBIDDEN),
        ] {
            let response = with_ingress_guard(app(test_state(None)), true)
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .extension(ConnectInfo(peer.parse::<SocketAddr>().unwrap()))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
    }
}
