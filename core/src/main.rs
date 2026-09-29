use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use genesis_core::{
    ha::{self, Availability, HaConfig, Inventory},
    ha_command,
    ledger::{Actor, ActorType, CommandRequest, Ledger, LedgerError, Snapshot},
};
use serde::Deserialize;
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
    household_id: String,
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
        config.household_id =
            env::var("GENESIS_HOUSEHOLD_ID").unwrap_or_else(|_| "pilot-home".to_owned());
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
            household_id: "pilot-home".to_owned(),
        })
    }
}

#[derive(Clone)]
struct AppState {
    inventory: Inventory,
    read_token: Option<String>,
    write_token: Option<String>,
    household_id: String,
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
        .route("/v1/devices", get(devices))
        .route("/v1/commands", axum::routing::post(create_command))
        .route("/v1/commands/{command_id}", get(get_command))
        .with_state(state)
}

async fn devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ha::Device>>, StatusCode> {
    authorize(&headers, state.read_token.as_deref())?;
    Ok(Json(state.inventory.devices().await))
}

fn authorize(headers: &HeaderMap, token: Option<&str>) -> Result<(), StatusCode> {
    let token = token.ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let supplied = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if supplied.ct_eq(token.as_bytes()).unwrap_u8() != 1 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

async fn create_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<NewCommand>,
) -> Result<Json<Snapshot>, StatusCode> {
    authorize(&headers, state.write_token.as_deref())?;
    if input.household_id != state.household_id {
        return Err(StatusCode::FORBIDDEN);
    }
    let ha_config = state
        .ha_config
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let device = state
        .inventory
        .devices()
        .await
        .into_iter()
        .find(|item| item.device_id == input.device_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    if device.availability != Availability::Online || !device.writable {
        return Err(StatusCode::CONFLICT);
    }
    let command_id = format!("cmd:{}", Uuid::new_v4());
    let actor = Actor {
        actor_type: ActorType::User,
        actor_id: "pilot-owner".to_owned(),
    };
    let request = CommandRequest {
        household_id: input.household_id,
        command_id: command_id.clone(),
        device_id: device.device_id.clone(),
        capability_id: "power".to_owned(),
        value: Value::Bool(input.value),
        actor: actor.clone(),
        idempotency_key: input.idempotency_key,
        correlation_id: input.correlation_id,
    };
    let mut ledger = state.ledger.lock().await;
    let snapshot = ledger.accept(request).map_err(map_ledger_error)?;
    if snapshot.request.command_id != command_id {
        return Ok(Json(snapshot));
    }
    let result = ha_command::execute(
        ha_config,
        &device,
        input.value,
        &mut ledger,
        &command_id,
        actor,
    )
    .await
    .map_err(map_ledger_error)?;
    Ok(Json(result))
}

async fn get_command(
    State(state): State<AppState>,
    Path(command_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Snapshot>, StatusCode> {
    authorize(&headers, state.read_token.as_deref())?;
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let ledger = Arc::new(Mutex::new(Ledger::open(&config.ledger_path)?));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(config.log_filter)
        .init();

    let inventory = Inventory::new();
    let ha_config = HaConfig::from_env().map_err(std::io::Error::other)?;
    if let Some(ha_config) = ha_config.clone() {
        tokio::spawn(ha::run(ha_config, inventory.clone()));
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
            household_id: config.household_id,
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
    use tower::ServiceExt;

    fn test_state(read_token: Option<String>) -> AppState {
        AppState {
            inventory: Inventory::new(),
            read_token,
            write_token: None,
            household_id: "pilot-home".to_owned(),
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
        }).to_string();
        let unauthorized = app(state.clone()).oneshot(
            Request::builder().method("POST").uri("/v1/commands")
                .header("content-type", "application/json")
                .body(Body::from(body.clone())).unwrap()
        ).await.unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let forbidden = app(state).oneshot(
            Request::builder().method("POST").uri("/v1/commands")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", "w".repeat(32)))
                .body(Body::from(body)).unwrap()
        ).await.unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
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
