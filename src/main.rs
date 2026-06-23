use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod config;
mod models;
mod openapi;
mod optimizer;
mod vacuum;
mod validation;

use config::AppConfig;
use models::{ErrorResponse, OptimizeRequest, VersionResponse};
use openapi::ApiDoc;
use optimizer::{optimize_request, optimize_request_alns, optimize_request_beam};
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
        optimize_queue_wait_ms = config.optimize_queue_wait_ms,
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
        .route("/v1/optimize/beam", post(optimize_beam))
        .route("/v1/optimize/alns", post(optimize_alns))
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
    optimize_common(state, payload, OptimizeFlavor::Default).await
}

#[utoipa::path(
    post,
    path = "/v1/optimize/beam",
    tag = "optimize",
    request_body = OptimizeRequest,
    responses(
        (status = 200, description = "Beam optimization result", body = models::OptimizeResponse),
        (status = 400, description = "Invalid JSON", body = ErrorResponse),
        (status = 429, description = "Too many concurrent optimize requests", body = ErrorResponse),
        (status = 422, description = "Validation error", body = ErrorResponse),
        (status = 408, description = "Optimization timeout", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse)
    )
)]
pub(crate) async fn optimize_beam(
    State(state): State<AppState>,
    payload: Result<Json<OptimizeRequest>, JsonRejection>,
) -> impl IntoResponse {
    optimize_common(state, payload, OptimizeFlavor::Beam).await
}

#[utoipa::path(
    post,
    path = "/v1/optimize/alns",
    tag = "optimize",
    request_body = OptimizeRequest,
    responses(
        (status = 200, description = "ALNS/LNS optimization result", body = models::OptimizeResponse),
        (status = 400, description = "Invalid JSON", body = ErrorResponse),
        (status = 429, description = "Too many concurrent optimize requests", body = ErrorResponse),
        (status = 422, description = "Validation error", body = ErrorResponse),
        (status = 408, description = "Optimization timeout", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse)
    )
)]
pub(crate) async fn optimize_alns(
    State(state): State<AppState>,
    payload: Result<Json<OptimizeRequest>, JsonRejection>,
) -> impl IntoResponse {
    optimize_common(state, payload, OptimizeFlavor::Alns).await
}

#[derive(Clone, Copy)]
enum OptimizeFlavor {
    Default,
    Beam,
    Alns,
}

async fn optimize_common(
    state: AppState,
    payload: Result<Json<OptimizeRequest>, JsonRejection>,
    flavor: OptimizeFlavor,
) -> Response {
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

    // Admission control: acquire one of `max_concurrent_optimize` permits. When
    // all permits are busy (e.g. several deep `cut_quality=max` jobs saturating
    // the CPU) the request waits in the queue up to `optimize_queue_wait_ms`
    // for one to free, instead of being rejected outright — a short burst is
    // queued rather than failed. Only if the wait is exhausted (or queueing is
    // disabled with `optimize_queue_wait_ms = 0`) do we return `429 OVERLOADED`.
    let sem = state.optimize_semaphore.clone();
    let queue_wait_ms = state.config.optimize_queue_wait_ms;
    let permit = if queue_wait_ms == 0 {
        sem.try_acquire_owned().ok()
    } else {
        // `acquire_owned` only errors if the semaphore is closed, which never
        // happens here; a timeout means the queue wait was exhausted.
        match tokio::time::timeout(Duration::from_millis(queue_wait_ms), sem.acquire_owned()).await
        {
            Ok(Ok(permit)) => Some(permit),
            Ok(Err(_)) | Err(_) => None,
        }
    };
    let _permit = match permit {
        Some(permit) => permit,
        None => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse {
                    status: "error",
                    error_code: "OVERLOADED",
                    message: "optimize queue is full; try again later".to_string(),
                    details: Some(serde_json::json!({
                        "max_concurrent_optimize": state.config.max_concurrent_optimize,
                        "queue_wait_ms": queue_wait_ms
                    })),
                }),
            )
                .into_response();
        }
    };

    let result = match flavor {
        OptimizeFlavor::Default => optimize_request(req, &state.config).await,
        OptimizeFlavor::Beam => optimize_request_beam(req, &state.config).await,
        OptimizeFlavor::Alns => optimize_request_alns(req, &state.config).await,
    };

    match result {
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
