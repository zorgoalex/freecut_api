use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

const VALID_REQUEST: &str = include_str!("../tests/fixtures/optimize_valid.json");
const INVALID_TRIM_REQUEST: &str = include_str!("../tests/fixtures/optimize_invalid_trim.json");

fn app_for_test() -> Router {
    let config = AppConfig::from_env();
    build_app(config)
}

async fn post_json(app: &Router, uri: &str, body: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

fn strip_time(mut value: Value) -> Value {
    if let Some(summary) = value.get_mut("summary") {
        if let Some(obj) = summary.as_object_mut() {
            obj.remove("time_ms");
        }
    }
    value
}

#[tokio::test]
async fn optimize_returns_svg() {
    let app = app_for_test();
    let parsed: Result<OptimizeRequest, _> = serde_json::from_str(VALID_REQUEST);
    assert!(
        parsed.is_ok(),
        "fixture does not deserialize: {}",
        parsed.err().unwrap()
    );
    let (status, json) = post_json(&app, "/v1/optimize", VALID_REQUEST).await;

    assert_eq!(status, StatusCode::OK, "unexpected status: {status}, body: {json}");
    assert_eq!(json.get("status").and_then(Value::as_str), Some("ok"));
    assert!(json
        .get("solutions")
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false));
    let svg = json
        .pointer("/artifacts/svg")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(svg.contains("<svg"));
    assert!(svg.contains("</svg>"));
}

#[tokio::test]
async fn optimize_reproducible_seed() {
    let app = app_for_test();
    let (_, first) = post_json(&app, "/v1/optimize", VALID_REQUEST).await;
    let (_, second) = post_json(&app, "/v1/optimize", VALID_REQUEST).await;
    assert_eq!(strip_time(first), strip_time(second));
}

#[tokio::test]
async fn optimize_invalid_trim_returns_422() {
    let app = app_for_test();
    let (status, json) = post_json(&app, "/v1/optimize", INVALID_TRIM_REQUEST).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unexpected status: {status}, body: {json}"
    );
    assert_eq!(json.get("status").and_then(Value::as_str), Some("error"));
}

#[tokio::test]
async fn invalid_json_returns_400() {
    let app = app_for_test();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/optimize")
        .header("content-type", "application/json")
        .body(Body::from("{".to_string()))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json.get("status").and_then(Value::as_str), Some("error"));
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("VALIDATION_ERROR")
    );
}

#[tokio::test]
async fn openapi_available() {
    let app = app_for_test();
    let (status, json) = get_json(&app, "/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.get("openapi").is_some());
}
