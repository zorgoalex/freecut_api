use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use utoipa::ToSchema;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod config;
mod models;
mod openapi;
mod validation;

use config::AppConfig;
use models::{ErrorResponse, OptimizeRequest};
use openapi::ApiDoc;
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
    let openapi = ApiDoc::openapi();

    let app = Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/version", get(version))
        .route("/v1/optimize", post(optimize))
        .route("/openapi.json", get(openapi_json))
        .merge(SwaggerUi::new("/docs").url("/openapi.json", openapi.clone()))
        .with_state(app_state)
        .layer(DefaultBodyLimit::max(config.max_body_bytes));

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!(%addr, "listening");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    axum::serve(listener, app).await.expect("serve");
}

#[utoipa::path(
    get,
    path = "/health/live",
    tag = "health",
    responses(
        (status = 200, description = "Service is live", body = String)
    )
)]
pub(crate) async fn health_live() -> &'static str {
    "ok"
}

#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "health",
    responses(
        (status = 200, description = "Service is ready", body = String)
    )
)]
pub(crate) async fn health_ready() -> &'static str {
    "ok"
}

#[derive(Serialize, ToSchema)]
pub(crate) struct VersionResponse {
    service: &'static str,
    version: &'static str,
}

#[utoipa::path(
    get,
    path = "/version",
    tag = "health",
    responses(
        (status = 200, description = "Service version", body = VersionResponse)
    )
)]
pub(crate) async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        service: "freecut",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[utoipa::path(
    post,
    path = "/v1/optimize",
    tag = "optimize",
    request_body = OptimizeRequest,
    responses(
        (status = 200, description = "Optimization result", body = models::OptimizeResponse),
        (status = 400, description = "Invalid JSON", body = ErrorResponse),
        (status = 422, description = "Validation error", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse)
    )
)]
pub(crate) async fn optimize(
    State(state): State<AppState>,
    payload: Result<Json<OptimizeRequest>, JsonRejection>,
) -> impl IntoResponse {
    let req = match payload {
        Ok(Json(req)) => req,
        Err(rejection) => return json_rejection(rejection),
    };

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

pub(crate) async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

fn json_rejection(rejection: JsonRejection) -> Response {
    let body = ErrorResponse {
        status: "error",
        error_code: "VALIDATION_ERROR",
        message: rejection.to_string(),
        details: None,
    };
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}
