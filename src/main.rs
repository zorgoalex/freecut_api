use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod config;
mod models;
mod openapi;
mod optimizer;
mod validation;

use config::AppConfig;
use models::{ErrorResponse, OptimizeRequest, VersionResponse};
use openapi::ApiDoc;
use optimizer::optimize_request;
use validation::{validate_request, ValidationLimits};

#[derive(Clone)]
struct AppState {
    config: AppConfig,
    optimize_semaphore: Arc<Semaphore>,
}

#[tokio::main]
async fn main() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(filter))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env();
    tracing::info!(
        default_time_limit_ms = config.default_time_limit_ms,
        default_restarts = config.default_restarts,
        max_concurrent_optimize = config.max_concurrent_optimize,
        "defaults loaded"
    );
    let app = build_app(config.clone());

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!(%addr, "listening");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    axum::serve(listener, app).await.expect("serve");
}

fn build_app(config: AppConfig) -> Router {
    let app_state = AppState {
        optimize_semaphore: Arc::new(Semaphore::new(config.max_concurrent_optimize.max(1))),
        config: config.clone(),
    };
    let openapi = ApiDoc::openapi();

    Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/version", get(version))
        .route("/v1/optimize", post(optimize))
        .merge(SwaggerUi::new("/docs").url("/openapi.json", openapi))
        .with_state(app_state)
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
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
        (status = 429, description = "Too many concurrent optimize requests", body = ErrorResponse),
        (status = 422, description = "Validation error", body = ErrorResponse),
        (status = 408, description = "Optimization timeout", body = ErrorResponse),
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

    let _permit = match state.optimize_semaphore.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse {
                    status: "error",
                    error_code: "OVERLOADED",
                    message: "too many concurrent optimize requests".to_string(),
                    details: Some(serde_json::json!({
                        "max_concurrent_optimize": state.config.max_concurrent_optimize
                    })),
                }),
            )
                .into_response();
        }
    };

    match optimize_request(req, &state.config).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(err) => err.into_response(),
    }
}

fn json_rejection(rejection: JsonRejection) -> Response {
    let status = rejection.status();
    let error_code = if status == StatusCode::PAYLOAD_TOO_LARGE {
        "CONSTRAINT_ERROR"
    } else {
        "VALIDATION_ERROR"
    };
    let body = ErrorResponse {
        status: "error",
        error_code,
        message: rejection.to_string(),
        details: None,
    };
    (status, Json(body)).into_response()
}

#[cfg(test)]
mod tests;
