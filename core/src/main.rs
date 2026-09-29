use std::{env, net::SocketAddr, path::PathBuf};

use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use genesis_core::{
    ha::{self, HaConfig, Inventory},
    ledger::Ledger,
};
use subtle::ConstantTimeEq;
use tracing_subscriber::EnvFilter;

struct Config {
    bind_addr: SocketAddr,
    log_filter: EnvFilter,
    ledger_path: PathBuf,
    read_token: Option<String>,
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
        if config
            .read_token
            .as_ref()
            .is_some_and(|token| token.len() < 32)
        {
            return Err("GENESIS_READ_TOKEN must have at least 32 characters".to_owned());
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
        })
    }
}

#[derive(Clone)]
struct AppState {
    inventory: Inventory,
    read_token: Option<String>,
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/devices", get(devices))
        .with_state(state)
}

async fn devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ha::Device>>, StatusCode> {
    let Some(token) = state.read_token.as_ref() else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    let supplied = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if supplied.ct_eq(token.as_bytes()).unwrap_u8() != 1 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(state.inventory.devices().await))
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
    let _ledger = Ledger::open(&config.ledger_path)?;
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(config.log_filter)
        .init();

    let inventory = Inventory::new();
    let ha_config = HaConfig::from_env().map_err(std::io::Error::other)?;
    if let Some(ha_config) = ha_config {
        tokio::spawn(ha::run(ha_config, inventory.clone()));
    }
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %listener.local_addr()?, "Genesis core listening");
    axum::serve(
        listener,
        app(AppState {
            inventory,
            read_token: config.read_token,
        }),
    )
    .with_graceful_shutdown(async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "Shutdown signal handler failed");
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
        let state = AppState {
            inventory: Inventory::new(),
            read_token: Some("a".repeat(32)),
        };
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
    async fn health_returns_json() {
        let response = app(AppState {
            inventory: Inventory::new(),
            read_token: None,
        })
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
