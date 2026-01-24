use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod config;
mod models;
mod validation;

use config::AppConfig;
use models::{ErrorResponse, OptimizeRequest};
use validation::{validate_request, ValidationLimits};

#[derive(Clone)]
struct AppState {
    config: AppConfig,
}

#[tokio::main]
async fn main() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(filter))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env();
    let app_state = AppState { config: config.clone() };

    let app = Router::new()
        .route("/health/live", get(health))
        .route("/health/ready", get(health))
        .route("/version", get(version))
        .route("/v1/optimize", post(optimize))
        .with_state(app_state)
        .layer(DefaultBodyLimit::max(config.max_body_bytes));

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!(%addr, "listening");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    axum::serve(listener, app).await.expect("serve");
}

async fn health() -> &'static str {
    "ok"
}

#[derive(Serialize)]
struct VersionResponse {
    service: &'static str,
    version: &'static str,
}

async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        service: "freecut",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn optimize(
    State(state): State<AppState>,
    Json(req): Json<OptimizeRequest>,
) -> impl IntoResponse {
    let limits = ValidationLimits {
        max_instances: state.config.max_instances,
        max_stock_types: 50,
    };

    if let Err(err) = validate_request(&req, &limits) {
        return err.into_response();
    }

    let body = ErrorResponse {
        status: "error",
        error_code: "INTERNAL",
        message: "optimization not implemented".to_string(),
        details: None,
    };
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}
