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

fn app_with_config(config: AppConfig) -> Router {
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

async fn get_text(app: &Router, uri: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    (status, text)
}

fn strip_time(mut value: Value) -> Value {
    if let Some(summary) = value.get_mut("summary") {
        if let Some(obj) = summary.as_object_mut() {
            obj.remove("time_ms");
        }
    }
    value
}

fn fmt_mm(value: f64) -> String {
    format!("{:.3}", value)
}

fn placement_rect_snippet(placement: &Value) -> Option<String> {
    let x = placement.get("x_mm")?.as_f64()?;
    let y = placement.get("y_mm")?.as_f64()?;
    let w = placement.get("width_mm")?.as_f64()?;
    let h = placement.get("height_mm")?.as_f64()?;
    Some(format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#cfe8ff\"",
        fmt_mm(x),
        fmt_mm(y),
        fmt_mm(w),
        fmt_mm(h)
    ))
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

    let mut placement_checks = 0;
    if let Some(solutions) = json.get("solutions").and_then(Value::as_array) {
        for solution in solutions {
            if let Some(placements) = solution.get("placements").and_then(Value::as_array) {
                for placement in placements {
                    if let Some(snippet) = placement_rect_snippet(placement) {
                        placement_checks += 1;
                        assert!(
                            svg.contains(&snippet),
                            "svg missing placement rect: {snippet}"
                        );
                    }
                }
            }
        }
    }
    assert!(placement_checks > 0, "no placements found for svg check");
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

#[tokio::test]
async fn docs_available() {
    let app = app_for_test();
    let (status, text) = get_text(&app, "/docs").await;
    if status == StatusCode::SEE_OTHER || status == StatusCode::MOVED_PERMANENTLY {
        let request = Request::builder()
            .method("GET")
            .uri("/docs/")
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(text.to_lowercase().contains("swagger"));
    } else {
        assert_eq!(status, StatusCode::OK);
        assert!(text.to_lowercase().contains("swagger"));
    }
}

#[tokio::test]
async fn max_instances_limit_enforced() {
    let config = AppConfig {
        port: 0,
        max_body_bytes: 5_242_880,
        max_instances: 2,
        default_time_limit_ms: 1200,
        default_restarts: 7,
    };
    let app = app_with_config(config);
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(items) = json.get_mut("items").and_then(Value::as_array_mut) {
        if let Some(first) = items.get_mut(0) {
            if let Some(obj) = first.as_object_mut() {
                obj.insert("qty".to_string(), Value::from(3));
            }
        }
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("CONSTRAINT_ERROR")
    );
}

#[tokio::test]
async fn max_stock_types_limit_enforced() {
    let mut stock = Vec::new();
    for i in 0..51 {
        stock.push(serde_json::json!({
            "id": format!("sheet-{i}"),
            "width_mm": 100.0,
            "height_mm": 100.0,
            "qty": 1
        }));
    }
    let body = serde_json::json!({
        "units": "mm",
        "params": {
            "kerf_mm": 1.0,
            "spacing_mm": 1.0,
            "trim_mm": { "left": 0.0, "right": 0.0, "top": 0.0, "bottom": 0.0 },
            "time_limit_ms": 200,
            "restarts": 2,
            "objective": "min_waste",
            "seed": 1
        },
        "stock": stock,
        "items": [
            { "id": "A", "width_mm": 10.0, "height_mm": 10.0, "qty": 1, "rotation": "forbid", "pattern_direction": "none" }
        ]
    });
    let app = app_for_test();
    let (status, json) = post_json(&app, "/v1/optimize", &body.to_string()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("VALIDATION_ERROR")
    );
}

#[tokio::test]
async fn body_size_limit_enforced() {
    let config = AppConfig {
        port: 0,
        max_body_bytes: 200,
        max_instances: 5000,
        default_time_limit_ms: 1200,
        default_restarts: 7,
    };
    let app = app_with_config(config);
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    json["padding"] = Value::from("x".repeat(1024));
    let body = serde_json::to_string(&json).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/optimize")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("CONSTRAINT_ERROR")
    );
}
