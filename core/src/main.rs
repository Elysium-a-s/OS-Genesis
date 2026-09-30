use std::{env, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use chrono::Utc;
use genesis_core::{
    grant::{self, AccessState},
    ha::{self, HaConfig, Inventory},
    ha_command::{self, ExecutionError},
    ledger::{Actor, ActorType, CommandRequest, Ledger, LedgerError, Snapshot},
    voice,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
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
        if entry.len() > 128
            || !entry.as_bytes()[0].is_ascii_alphanumeric()
            || !entry
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err(
                "GENESIS_SENSITIVE_DEVICES must be a comma-separated list of device ids".to_owned(),
            );
        }
        devices.push(entry.to_owned());
    }
    Ok(devices)
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
    ledger: Arc<Mutex<Ledger>>,
    ha_config: Option<HaConfig>,
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
    Router::new()
        .route("/health", get(health))
        .route("/v1/me", get(me))
        .route("/v1/devices", get(devices))
        .route("/v1/commands", axum::routing::post(create_command))
        .route("/v1/commands/{command_id}", get(get_command))
        .route("/v1/access", get(access))
        .route("/v1/voice/commands", axum::routing::post(voice_command))
        .route(
            "/v1/voice/confirmations",
            axum::routing::post(voice_confirmation),
        )
        .route("/v1/voice/audit", get(voice_audit))
        .with_state(state)
}

async fn devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ha::Device>>, StatusCode> {
    principal(&headers, &state)?;
    Ok(Json(state.inventory.devices().await))
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Role {
    Owner,
    Member,
    Guest,
    Service,
}

#[derive(Serialize)]
struct Principal {
    household_id: String,
    actor_id: &'static str,
    role: Role,
    can_control_devices: bool,
}

fn principal(headers: &HeaderMap, state: &AppState) -> Result<Principal, StatusCode> {
    let tokens = [
        (state.write_token.as_deref(), Role::Owner, "pilot-owner"),
        (state.member_token.as_deref(), Role::Member, "pilot-member"),
        (state.guest_token.as_deref(), Role::Guest, "pilot-guest"),
        (state.read_token.as_deref(), Role::Service, "pilot-service"),
    ];
    if tokens.iter().all(|(token, _, _)| token.is_none()) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let supplied = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    for (token, role, actor_id) in tokens {
        if token.is_some_and(|value| supplied.ct_eq(value.as_bytes()).unwrap_u8() == 1) {
            return Ok(Principal {
                household_id: state.household_id.clone(),
                actor_id,
                role,
                can_control_devices: matches!(role, Role::Owner | Role::Member),
            });
        }
    }
    Err(StatusCode::UNAUTHORIZED)
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Principal>, StatusCode> {
    Ok(Json(principal(&headers, &state)?))
}

/// Posledný potvrdený stav a otvorené incidenty časovo obmedzených grantov.
///
/// Prehľad je iba na čítanie, takže stačí ktorýkoľvek platný token; zosúladenie
/// beží v plánovači, nie z tejto požiadavky.
async fn access(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AccessState>>, StatusCode> {
    principal(&headers, &state)?;
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
    principal(&headers, &state)?;
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
    attributable: Option<(&'static str, &'static str)>,
}

/// Aktér, ktorý smie ovládať zariadenia v pilotnej domácnosti. Panel aj hlas
/// prechádzajú touto kontrolou a identitu aktéra určuje server.
fn authorized_controller(
    headers: &HeaderMap,
    state: &AppState,
    household_id: &str,
) -> Result<Actor, ControlRefusal> {
    let caller = principal(headers, state).map_err(|status| ControlRefusal {
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
        actor_id: caller.actor_id.to_owned(),
    })
}

/// Autorizácia hlasovej akcie. Na rozdiel od panela sa pripísateľné zamietnutie
/// zapíše do auditu, aby sa dalo doložiť, že a prečo asistent odmietol.
async fn authorized_speaker(
    headers: &HeaderMap,
    state: &AppState,
    household_id: &str,
) -> Result<Actor, StatusCode> {
    match authorized_controller(headers, state, household_id) {
        Ok(actor) => Ok(actor),
        Err(refusal) => {
            if let Some((actor_id, reason)) = refusal.attributable {
                let mut ledger = state.ledger.lock().await;
                voice::record_refusal(
                    &mut ledger,
                    &state.household_id,
                    actor_id,
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
    principal(&headers, &state)?;
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

    let inventory = Inventory::new();
    let ha_config = HaConfig::from_env().map_err(std::io::Error::other)?;
    if let Some(ha_config) = ha_config.clone() {
        tokio::spawn(ha::run(ha_config.clone(), inventory.clone()));
        tokio::spawn(reconcile_sweeper(
            ha_config,
            inventory.clone(),
            Arc::clone(&ledger),
        ));
    }
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %listener.local_addr()?, "Genesis core listening");
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    axum::serve(
        listener,
        app(AppState {
            inventory,
            read_token: config.read_token,
            write_token: config.write_token,
            member_token: config.member_token,
            guest_token: config.guest_token,
            household_id: config.household_id,
            sensitive_devices: Arc::new(config.sensitive_devices),
            ledger,
            ha_config,
        }),
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
            ledger: Arc::new(Mutex::new(Ledger::open(":memory:").unwrap())),
            ha_config: None,
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
}
