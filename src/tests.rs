use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

const VALID_REQUEST: &str = include_str!("../tests/fixtures/optimize_valid.json");
const INVALID_TRIM_REQUEST: &str = include_str!("../tests/fixtures/optimize_invalid_trim.json");
const MULTISHEET_OVERSIZED_REQUEST: &str =
    include_str!("../tests/fixtures/multisheet_oversized.json");
const MULTISHEET_QTY_LIMIT_REQUEST: &str =
    include_str!("../tests/fixtures/multisheet_qty_limit.json");

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

async fn post_json_owned(app: Router, uri: &'static str, body: String) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
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

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&apos;")
}

fn placement_text_snippet(placement: &Value, item_id: &str) -> Option<String> {
    let x = placement.get("x_mm")?.as_f64()? + 2.0;
    let y = placement.get("y_mm")?.as_f64()? + 12.0;
    Some(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"#1f4a6d\">{}</text>",
        fmt_mm(x),
        fmt_mm(y),
        escape_xml(item_id)
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

    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected status: {status}, body: {json}"
    );
    assert_eq!(json.get("status").and_then(Value::as_str), Some("ok"));
    assert!(json
        .pointer("/summary/restarts_requested")
        .and_then(Value::as_u64)
        .is_some());
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
    let mut text_checks = 0;
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
                    if let Some(item_id) = placement.get("item_id").and_then(Value::as_str) {
                        if let Some(text_snippet) = placement_text_snippet(placement, item_id) {
                            text_checks += 1;
                            assert!(
                                svg.contains(&text_snippet),
                                "svg missing placement text: {text_snippet}"
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(placement_checks > 0, "no placements found for svg check");
    assert!(text_checks > 0, "no placement labels found for svg check");
}

#[tokio::test]
async fn optimize_without_svg_omits_artifact_svg() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "unexpected status/body: {json}");
    assert_eq!(json.get("status").and_then(Value::as_str), Some("ok"));
    assert!(
        json.pointer("/artifacts/svg").is_none(),
        "expected artifacts.svg to be omitted when include_svg=false, body: {json}"
    );
}

#[tokio::test]
async fn optimize_exposes_candidate_selection_telemetry() {
    let app = app_for_test();
    let (status, json) = post_json(&app, "/v1/optimize", VALID_REQUEST).await;
    assert_eq!(status, StatusCode::OK, "unexpected status/body: {json}");
    let selection = json
        .pointer("/summary/candidate_selection")
        .cloned()
        .unwrap_or(Value::Null);
    assert!(selection.is_object(), "missing candidate_selection: {json}");
    let total = selection
        .get("candidates_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let valid = selection
        .get("candidates_valid")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let invalid = selection
        .get("candidates_invalid_fitness")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert_eq!(
        total,
        valid + invalid,
        "inconsistent candidate counters: {selection}"
    );
    assert!(
        selection
            .get("top_k_requested")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1,
        "top_k_requested must be >=1: {selection}"
    );
    let unique = selection
        .get("top_k_unique_signatures")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(unique >= 1, "expected at least one unique signature: {selection}");
    let waste_min = selection
        .get("top_k_waste_area_mm2_min")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let waste_max = selection
        .get("top_k_waste_area_mm2_max")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let waste_mean = selection
        .get("top_k_waste_area_mm2_mean")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    assert!(
        waste_min <= waste_max,
        "invalid waste range min/max: {selection}"
    );
    assert!(
        (waste_min..=waste_max).contains(&waste_mean),
        "waste mean should fall within min/max: {selection}"
    );
}

#[tokio::test]
async fn optimize_reproducible_seed() {
    let app = app_for_test();
    let (_, first) = post_json(&app, "/v1/optimize", VALID_REQUEST).await;
    let (_, second) = post_json(&app, "/v1/optimize", VALID_REQUEST).await;
    assert_eq!(
        first.pointer("/summary/used_seed").and_then(Value::as_u64),
        Some(12345)
    );
    assert_eq!(strip_time(first), strip_time(second));
}

#[tokio::test]
async fn optimize_auto_seed_changes_per_request() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.remove("seed");
    }
    let body = serde_json::to_string(&json).unwrap();

    let (_, first) = post_json(&app, "/v1/optimize", &body).await;
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let (_, second) = post_json(&app, "/v1/optimize", &body).await;

    let first_seed = first.pointer("/summary/used_seed").and_then(Value::as_u64);
    let second_seed = second.pointer("/summary/used_seed").and_then(Value::as_u64);
    assert!(first_seed.is_some(), "missing used_seed in first response");
    assert!(
        second_seed.is_some(),
        "missing used_seed in second response"
    );
    assert_ne!(
        first_seed, second_seed,
        "auto seed should change per request"
    );
}

#[tokio::test]
#[ignore = "manual snapshot for comparing phenotype diversity across seeds"]
async fn optimize_seed_diversity_snapshot() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    for seed in [1_u64, 2_u64] {
        if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
            params.insert("seed".to_string(), Value::from(seed));
        }
        let body = serde_json::to_string(&json).unwrap();
        let (status, resp) = post_json(&app, "/v1/optimize", &body).await;
        assert_eq!(status, StatusCode::OK, "unexpected status/body: {resp}");
        let selection = resp
            .pointer("/summary/candidate_selection")
            .cloned()
            .unwrap_or(Value::Null);
        let unique = selection
            .get("top_k_unique_signatures")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let waste_mean = selection
            .get("top_k_waste_area_mm2_mean")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        println!(
            "[seed_diversity] seed={seed} unique_signatures={unique} waste_mean_mm2={waste_mean:.3}"
        );
    }
}

#[tokio::test]
async fn optimize_portfolio_returns_telemetry() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert(
            "portfolio".to_string(),
            serde_json::json!({
                "enabled": true,
                "deadline_ms": 1200,
                "candidate_count": 3
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let portfolio = json
        .pointer("/summary/portfolio")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        portfolio.get("candidates_total").and_then(Value::as_u64),
        Some(3)
    );
    assert!(
        portfolio
            .get("candidates_completed")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );
    assert!(
        portfolio
            .get("winner_strategy")
            .and_then(Value::as_str)
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "expected non-empty winner_strategy, body: {json}"
    );
}

#[tokio::test]
async fn optimize_portfolio_supports_multisheet_oversized() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert(
            "portfolio".to_string(),
            serde_json::json!({
                "enabled": true,
                "deadline_ms": 6000,
                "candidate_count": 2
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert!(
        json.pointer("/summary/portfolio").is_some(),
        "expected summary.portfolio, body: {json}"
    );
    let oversized = json
        .get("unplaced_items")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("reason")
                    .and_then(Value::as_str)
                    .map(|reason| reason == "oversized")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    assert!(
        oversized,
        "expected oversized items in unplaced_items, body: {json}"
    );
}

#[tokio::test]
async fn optimize_beam_returns_telemetry() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert(
            "beam".to_string(),
            serde_json::json!({
                "enabled": true,
                "deadline_ms": 2000,
                "beam_width": 2,
                "beam_depth": 2,
                "branch_factor": 2
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize/beam", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let beam = json
        .pointer("/summary/beam")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    assert_eq!(beam.get("beam_width").and_then(Value::as_u64), Some(2));
    assert_eq!(beam.get("beam_depth").and_then(Value::as_u64), Some(2));
    assert_eq!(beam.get("branch_factor").and_then(Value::as_u64), Some(2));
    assert!(
        beam.get("winner_seed")
            .and_then(Value::as_u64)
            .map(|s| s > 0)
            .unwrap_or(false),
        "expected winner_seed in beam telemetry, body: {json}"
    );
}

#[tokio::test]
async fn optimize_beam_supports_multisheet_oversized() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert("time_limit_ms".to_string(), Value::from(5000));
        params.insert("restarts".to_string(), Value::from(2));
        params.insert(
            "beam".to_string(),
            serde_json::json!({
                "deadline_ms": 5000,
                "beam_width": 2,
                "beam_depth": 2,
                "branch_factor": 2
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize/beam", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert!(
        json.pointer("/summary/beam").is_some(),
        "expected summary.beam, body: {json}"
    );
    let oversized = json
        .get("unplaced_items")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("reason")
                    .and_then(Value::as_str)
                    .map(|reason| reason == "oversized")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    assert!(
        oversized,
        "expected oversized items in unplaced_items, body: {json}"
    );
}

#[tokio::test]
async fn optimize_alns_returns_telemetry() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert(
            "alns".to_string(),
            serde_json::json!({
                "enabled": true,
                "deadline_ms": 2000,
                "iterations": 12,
                "segment_size": 4,
                "temperature_start": 1.0,
                "temperature_end": 0.2,
                "reaction_factor": 0.3
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize/alns", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let alns = json
        .pointer("/summary/alns")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        alns.get("iterations_requested").and_then(Value::as_u64),
        Some(12)
    );
    assert_eq!(alns.get("segment_size").and_then(Value::as_u64), Some(4));
    assert!(
        alns.get("operators")
            .and_then(Value::as_array)
            .map(|ops| !ops.is_empty())
            .unwrap_or(false),
        "expected non-empty operators telemetry, body: {json}"
    );
}

#[tokio::test]
async fn optimize_alns_supports_multisheet_oversized() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert("time_limit_ms".to_string(), Value::from(10000));
        params.insert("restarts".to_string(), Value::from(2));
        params.insert(
            "alns".to_string(),
            serde_json::json!({
                "deadline_ms": 10000,
                "iterations": 4,
                "segment_size": 2
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize/alns", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert!(
        json.pointer("/summary/alns").is_some(),
        "expected summary.alns, body: {json}"
    );
    let oversized = json
        .get("unplaced_items")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("reason")
                    .and_then(Value::as_str)
                    .map(|reason| reason == "oversized")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    assert!(
        oversized,
        "expected oversized items in unplaced_items, body: {json}"
    );
}

#[tokio::test]
async fn optimize_multisheet_restarts_4_uses_timeout_rescue() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert("time_limit_ms".to_string(), Value::from(2400));
        params.insert("restarts".to_string(), Value::from(4));
        params.insert("seed".to_string(), Value::from(12345));
        params.remove("portfolio");
        params.remove("beam");
        params.remove("alns");
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert_eq!(
        json.pointer("/summary/timeout_reason")
            .and_then(Value::as_str),
        Some("slice_timeout"),
        "expected timeout rescue path marker, body: {json}"
    );
    let restarts_used = json
        .pointer("/summary/restarts_used")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(
        restarts_used >= 1,
        "expected at least one successful restart after rescue, body: {json}"
    );
}

#[tokio::test]
async fn optimize_multisheet_restarts_4_no_longer_returns_408() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert("time_limit_ms".to_string(), Value::from(2000));
        params.insert("restarts".to_string(), Value::from(4));
        params.insert("seed".to_string(), Value::from(12345));
        params.remove("portfolio");
        params.remove("beam");
        params.remove("alns");
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
}

#[tokio::test]
async fn optimize_standard_includes_restart_policy_telemetry() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert("time_limit_ms".to_string(), Value::from(2000));
        params.insert("restarts".to_string(), Value::from(4));
        params.insert("seed".to_string(), Value::from(12345));
        params.remove("portfolio");
        params.remove("beam");
        params.remove("alns");
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let rp = json
        .pointer("/summary/restart_policy")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        rp.get("profile").and_then(Value::as_str),
        Some("balanced"),
        "expected default profile in restart_policy, body: {json}"
    );
    assert!(
        rp.get("planned_slices_ms")
            .and_then(Value::as_array)
            .map(|arr| !arr.is_empty())
            .unwrap_or(false),
        "expected non-empty planned_slices_ms, body: {json}"
    );
    assert!(
        rp.get("restarts_effective")
            .and_then(Value::as_u64)
            .map(|v| v >= 1)
            .unwrap_or(false),
        "expected restarts_effective >= 1, body: {json}"
    );
}

#[tokio::test]
async fn optimize_sla_profile_changes_effective_restart_plan() {
    let app = app_for_test();
    let mut base: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    if let Some(params) = base.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert("time_limit_ms".to_string(), Value::from(2000));
        params.insert("restarts".to_string(), Value::from(6));
        params.insert("seed".to_string(), Value::from(12345));
        params.remove("portfolio");
        params.remove("beam");
        params.remove("alns");
    }

    let mut fast = base.clone();
    if let Some(params) = fast.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("sla_profile".to_string(), Value::from("fast"));
    }
    let fast_body = serde_json::to_string(&fast).unwrap();
    let (status_fast, json_fast) = post_json(&app, "/v1/optimize", &fast_body).await;
    assert_eq!(status_fast, StatusCode::OK, "body: {json_fast}");

    let mut quality = base;
    if let Some(params) = quality.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("sla_profile".to_string(), Value::from("quality"));
    }
    let quality_body = serde_json::to_string(&quality).unwrap();
    let (status_quality, json_quality) = post_json(&app, "/v1/optimize", &quality_body).await;
    assert_eq!(status_quality, StatusCode::OK, "body: {json_quality}");

    assert_eq!(
        json_fast
            .pointer("/summary/restart_policy/profile")
            .and_then(Value::as_str),
        Some("fast")
    );
    assert_eq!(
        json_quality
            .pointer("/summary/restart_policy/profile")
            .and_then(Value::as_str),
        Some("quality")
    );

    let fast_effective = json_fast
        .pointer("/summary/restart_policy/restarts_effective")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let quality_effective = json_quality
        .pointer("/summary/restart_policy/restarts_effective")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(
        quality_effective >= fast_effective,
        "expected quality profile to allow >= fast restarts, fast={fast_effective}, quality={quality_effective}"
    );
}

#[tokio::test]
async fn optimize_layout_mode_default_guillotine() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.remove("layout_mode");
        params.insert("time_limit_ms".to_string(), Value::from(2000));
        params.insert("restarts".to_string(), Value::from(2));
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json.pointer("/summary/layout_mode").and_then(Value::as_str),
        Some("guillotine")
    );
}

#[tokio::test]
async fn optimize_defaults_time_limit_and_restarts() {
    let config = AppConfig {
        port: 0,
        max_body_bytes: 5_242_880,
        max_instances: 5000,
        default_time_limit_ms: 2000,
        default_restarts: 10,
        max_concurrent_optimize: 4,
    };
    let app = app_with_config(config);
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.remove("time_limit_ms");
        params.remove("restarts");
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK);
    let restarts_used = json
        .pointer("/summary/restarts_used")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert_eq!(
        json.pointer("/summary/restarts_requested")
            .and_then(Value::as_u64),
        Some(10)
    );
    assert!(
        restarts_used >= 1,
        "expected at least one restart, got {restarts_used}"
    );
    assert!(
        restarts_used <= 10,
        "expected restarts_used <= restarts_requested (10), got {restarts_used}"
    );
    assert!(
        json.pointer("/summary/timeout_reason").is_none(),
        "timeout_reason should be absent for non-timeout result"
    );
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
async fn optimize_invalid_portfolio_candidate_count_returns_422() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert(
            "portfolio".to_string(),
            serde_json::json!({
                "enabled": true,
                "deadline_ms": 1200,
                "candidate_count": 0
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {json}");
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("VALIDATION_ERROR")
    );
}

#[tokio::test]
async fn optimize_invalid_beam_width_returns_422() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert(
            "beam".to_string(),
            serde_json::json!({
                "deadline_ms": 1200,
                "beam_width": 0,
                "beam_depth": 2,
                "branch_factor": 2
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize/beam", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {json}");
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("VALIDATION_ERROR")
    );
}

#[tokio::test]
async fn optimize_invalid_alns_iterations_returns_422() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert(
            "alns".to_string(),
            serde_json::json!({
                "deadline_ms": 1200,
                "iterations": 0,
                "segment_size": 4
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize/alns", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {json}");
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("VALIDATION_ERROR")
    );
}

#[tokio::test]
async fn optimize_invalid_sla_profile_returns_422() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("sla_profile".to_string(), Value::from("ultra"));
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {json}");
}

#[tokio::test]
async fn optimize_invalid_ga_override_returns_422() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert(
            "ga_override".to_string(),
            serde_json::json!({
                "epochs": 0,
                "breed_factor": 1.2,
                "survival_factor": -0.1,
                "top_k_candidates": 100
            }),
        );
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {json}");
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("VALIDATION_ERROR")
    );
}

#[tokio::test]
async fn optimize_accepts_ga_profile_and_override() {
    let app = app_for_test();
    let mut json: Value = serde_json::from_str(VALID_REQUEST).unwrap();
    if let Some(params) = json.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("ga_profile".to_string(), Value::from("quality"));
        params.insert(
            "ga_override".to_string(),
            serde_json::json!({
                "epochs": 120,
                "breed_factor": 0.55,
                "survival_factor": 0.7,
                "top_k_candidates": 8
            }),
        );
        params.insert("include_svg".to_string(), Value::from(false));
    }
    let body = serde_json::to_string(&json).unwrap();
    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert_eq!(json.get("status").and_then(Value::as_str), Some("ok"));
}

#[tokio::test]
async fn optimize_returns_429_when_overloaded() {
    let config = AppConfig {
        port: 0,
        max_body_bytes: 5_242_880,
        max_instances: 5000,
        default_time_limit_ms: 2000,
        default_restarts: 10,
        max_concurrent_optimize: 1,
    };
    let app = app_with_config(config);

    let mut items = Vec::new();
    for i in 0..10 {
        items.push(serde_json::json!({
            "id": format!("L{i}"),
            "width_mm": 120.0 + (i as f64) * 10.0,
            "height_mm": 160.0 + (i as f64) * 8.0,
            "qty": 3,
            "rotation": "allow_90",
            "pattern_direction": "none"
        }));
    }

    let heavy_body = serde_json::json!({
        "units": "mm",
        "params": {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": { "left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0 },
            "time_limit_ms": 2000,
            "restarts": 10,
            "objective": "min_waste",
            "seed": 12345,
            "layout_mode": "guillotine"
        },
        "stock": [{ "id": "sheet", "width_mm": 2500.0, "height_mm": 1250.0, "qty": 0 }],
        "items": items
    })
    .to_string();

    let first = tokio::spawn(post_json_owned(app.clone(), "/v1/optimize", heavy_body));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (status, json) = post_json(&app, "/v1/optimize", VALID_REQUEST).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "body: {json}");
    assert_eq!(
        json.get("error_code").and_then(Value::as_str),
        Some("OVERLOADED")
    );

    let _ = first.await.unwrap();
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
    if status.is_redirection() {
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
        max_concurrent_optimize: 4,
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
            "seed": 1,
            "layout_mode": "nested"
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
async fn duplicate_stock_ids_rejected() {
    let body = serde_json::json!({
        "units": "mm",
        "params": {
            "kerf_mm": 1.0,
            "spacing_mm": 1.0,
            "trim_mm": { "left": 0.0, "right": 0.0, "top": 0.0, "bottom": 0.0 },
            "time_limit_ms": 200,
            "restarts": 2,
            "objective": "min_waste",
            "seed": 1,
            "layout_mode": "nested"
        },
        "stock": [
            { "id": "sheet-A", "width_mm": 1000.0, "height_mm": 500.0, "qty": 1 },
            { "id": "sheet-A", "width_mm": 2000.0, "height_mm": 1000.0, "qty": 1 }
        ],
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
    let duplicates = json
        .pointer("/details/duplicate_stock_ids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        duplicates
            .iter()
            .any(|v| v.as_str().map(|s| s == "sheet-A").unwrap_or(false)),
        "expected duplicate_stock_ids to contain sheet-A, body: {json}"
    );
}

#[tokio::test]
async fn same_usable_size_different_stock_ids_do_not_trigger_false_qty_limit() {
    let app = app_for_test();
    let body = serde_json::json!({
        "units": "mm",
        "params": {
            "kerf_mm": 0.0,
            "spacing_mm": 0.0,
            "trim_mm": { "left": 0.0, "right": 0.0, "top": 0.0, "bottom": 0.0 },
            "time_limit_ms": 3000,
            "restarts": 3,
            "objective": "min_sheets",
            "seed": 42,
            "layout_mode": "guillotine"
        },
        "stock": [
            { "id": "oak", "width_mm": 1000.0, "height_mm": 1000.0, "qty": 1 },
            { "id": "pine", "width_mm": 1000.0, "height_mm": 1000.0, "qty": 0 }
        ],
        "items": [
            { "id": "P", "width_mm": 490.0, "height_mm": 490.0, "qty": 5, "rotation": "allow_90", "pattern_direction": "none" }
        ]
    })
    .to_string();

    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");

    let solutions = json
        .get("solutions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        solutions.len() >= 2,
        "expected at least two sheets, got {}",
        solutions.len()
    );

    let has_pine = solutions.iter().any(|s| {
        s.get("stock_id")
            .and_then(Value::as_str)
            .map(|id| id == "pine")
            .unwrap_or(false)
    });
    assert!(has_pine, "expected at least one sheet mapped to 'pine'");

    let has_qty_limit = json
        .get("unplaced_items")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("reason")
                    .and_then(Value::as_str)
                    .map(|reason| reason == "qty_limit")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    assert!(
        !has_qty_limit,
        "unexpected qty_limit in unplaced_items for unlimited alternative stock"
    );
}

#[tokio::test]
async fn early_stop_no_improve_can_reduce_restarts() {
    let app = app_for_test();
    let body = serde_json::json!({
        "units": "mm",
        "params": {
            "kerf_mm": 0.0,
            "spacing_mm": 0.0,
            "trim_mm": { "left": 0.0, "right": 0.0, "top": 0.0, "bottom": 0.0 },
            "time_limit_ms": 5000,
            "restarts": 20,
            "objective": "min_waste",
            "seed": 7,
            "layout_mode": "guillotine"
        },
        "stock": [
            { "id": "sheet", "width_mm": 1000.0, "height_mm": 1000.0, "qty": 1 }
        ],
        "items": [
            { "id": "A", "width_mm": 100.0, "height_mm": 100.0, "qty": 1, "rotation": "forbid", "pattern_direction": "none" }
        ]
    })
    .to_string();

    let (status, json) = post_json(&app, "/v1/optimize", &body).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");

    let restarts_requested = json
        .pointer("/summary/restarts_requested")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let restarts_used = json
        .pointer("/summary/restarts_used")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    assert_eq!(restarts_requested, 20);
    assert!(restarts_used >= 1);
    assert!(
        restarts_used < restarts_requested,
        "expected early-stop to reduce restarts, used={restarts_used}, requested={restarts_requested}, body: {json}"
    );
    assert!(
        json.pointer("/summary/timeout_reason").is_none(),
        "early-stop by no-improve should not set timeout_reason"
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
        max_concurrent_optimize: 4,
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

#[tokio::test]
#[ignore = "perf-gate heavy fixture; run manually to guard restart regression"]
async fn perf_gate_standard_heavy_restarts_4() {
    const N: usize = 40;
    let app = app_for_test();
    let mut base: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    if let Some(params) = base.get_mut("params").and_then(Value::as_object_mut) {
        params.insert("include_svg".to_string(), Value::Bool(false));
        params.insert("time_limit_ms".to_string(), Value::from(2000));
        params.insert("restarts".to_string(), Value::from(4));
        params.remove("portfolio");
        params.remove("beam");
        params.remove("alns");
    }

    let mut ok_runs = 0usize;
    let mut p95_samples: Vec<f64> = Vec::with_capacity(N);
    for i in 0..N {
        if let Some(params) = base.get_mut("params").and_then(Value::as_object_mut) {
            params.insert("seed".to_string(), Value::from((10_000 + i) as u64));
        }
        let body = serde_json::to_string(&base).unwrap();
        let (status, json) = post_json(&app, "/v1/optimize", &body).await;
        if status == StatusCode::OK {
            ok_runs += 1;
            if let Some(v) = json.pointer("/summary/time_ms").and_then(Value::as_f64) {
                p95_samples.push(v);
            }
        }
    }

    p95_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p95 = if p95_samples.is_empty() {
        None
    } else {
        Some(p95_samples[((p95_samples.len() - 1) as f64 * 0.95) as usize])
    };

    assert_eq!(
        ok_runs, N,
        "expected no 408 regressions for heavy fixture with restarts=4; ok_runs={ok_runs}/{N}, p95={p95:?}"
    );
}

#[tokio::test]
#[ignore = "benchmark-style test; run manually for stage metrics"]
async fn benchmark_portfolio_vs_standard_250() {
    const N: usize = 250;
    let app = app_for_test();

    async fn run_series(app: &Router, mut base: Value, use_portfolio: bool, n: usize) -> Value {
        if let Some(params) = base.get_mut("params").and_then(Value::as_object_mut) {
            params.insert("include_svg".to_string(), Value::Bool(false));
            params.insert("time_limit_ms".to_string(), Value::from(2000));
            params.insert("restarts".to_string(), Value::from(2));
            if use_portfolio {
                params.insert(
                    "portfolio".to_string(),
                    serde_json::json!({
                        "enabled": true,
                        "deadline_ms": 2000,
                        "candidate_count": 2
                    }),
                );
            } else {
                params.remove("portfolio");
            }
        }

        let mut ok_runs: u64 = 0;
        let mut status_counts: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();
        let mut time_ms: Vec<f64> = Vec::new();
        let mut waste_percent: Vec<f64> = Vec::new();
        let mut sheets_used: Vec<f64> = Vec::new();
        let mut placeable_ratios: Vec<f64> = Vec::new();
        let mut full_placeable: u64 = 0;
        let mut timeout_reasons: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();

        for _ in 0..n {
            let body = serde_json::to_string(&base).unwrap();
            let (status, json) = post_json(app, "/v1/optimize", &body).await;
            *status_counts
                .entry(status.as_u16().to_string())
                .or_insert(0) += 1;

            if status == StatusCode::OK {
                ok_runs += 1;
                let summary = json.get("summary").cloned().unwrap_or(Value::Null);
                if let Some(v) = summary.get("time_ms").and_then(Value::as_f64) {
                    time_ms.push(v);
                }
                if let Some(v) = summary.get("waste_percent").and_then(Value::as_f64) {
                    waste_percent.push(v);
                }
                if let Some(v) = summary.get("used_stock_count").and_then(Value::as_f64) {
                    sheets_used.push(v);
                }
                if let Some(reason) = summary.get("timeout_reason").and_then(Value::as_str) {
                    *timeout_reasons.entry(reason.to_string()).or_insert(0) += 1;
                }

                let placed = json
                    .get("solutions")
                    .and_then(Value::as_array)
                    .map(|solutions| {
                        solutions
                            .iter()
                            .map(|s| {
                                s.get("placements")
                                    .and_then(Value::as_array)
                                    .map(|p| p.len())
                                    .unwrap_or(0)
                            })
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                let placeable_unplaced = json
                    .get("unplaced_items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter(|item| {
                                item.get("reason")
                                    .and_then(Value::as_str)
                                    .map(|r| r != "oversized")
                                    .unwrap_or(false)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                let placeable_total = placed + placeable_unplaced;
                let ratio = if placeable_total > 0 {
                    placed as f64 / placeable_total as f64
                } else {
                    1.0
                };
                placeable_ratios.push(ratio);
                if placeable_unplaced == 0 {
                    full_placeable += 1;
                }
            }
        }

        fn avg(vals: &[f64]) -> Option<f64> {
            if vals.is_empty() {
                None
            } else {
                Some(vals.iter().sum::<f64>() / vals.len() as f64)
            }
        }
        fn percentile(vals: &mut [f64], q: f64) -> Option<f64> {
            if vals.is_empty() {
                return None;
            }
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = ((vals.len() - 1) as f64 * q) as usize;
            vals.get(idx).copied()
        }

        let mut time_sorted = time_ms.clone();
        let mut waste_sorted = waste_percent.clone();

        serde_json::json!({
            "runs": n,
            "ok_runs": ok_runs,
            "status_counts": status_counts,
            "avg_time_ms": avg(&time_ms),
            "p50_time_ms": percentile(&mut time_sorted, 0.50),
            "p95_time_ms": percentile(&mut time_sorted, 0.95),
            "avg_waste_percent": avg(&waste_percent),
            "p50_waste_percent": percentile(&mut waste_sorted, 0.50),
            "avg_sheets_used": avg(&sheets_used),
            "avg_placeable_placed_ratio": avg(&placeable_ratios),
            "full_placeable_rate": if ok_runs > 0 { Some(full_placeable as f64 / ok_runs as f64) } else { None::<f64> },
            "timeout_reasons": timeout_reasons,
        })
    }

    let base: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    let standard = run_series(&app, base.clone(), false, N).await;
    let portfolio = run_series(&app, base, true, N).await;

    let report = serde_json::json!({
        "fixture": "tests/fixtures/multisheet_oversized.json",
        "standard": standard,
        "portfolio": portfolio,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[tokio::test]
#[ignore = "benchmark-style test; run manually for stage metrics"]
async fn benchmark_portfolio_vs_standard_250_qty_limit() {
    const N: usize = 250;
    let app = app_for_test();

    async fn run_series(app: &Router, mut base: Value, use_portfolio: bool, n: usize) -> Value {
        if let Some(params) = base.get_mut("params").and_then(Value::as_object_mut) {
            params.insert("include_svg".to_string(), Value::Bool(false));
            if use_portfolio {
                params.insert(
                    "portfolio".to_string(),
                    serde_json::json!({
                        "enabled": true,
                        "deadline_ms": 1200,
                        "candidate_count": 2
                    }),
                );
            } else {
                params.remove("portfolio");
            }
        }

        let mut ok_runs: u64 = 0;
        let mut status_counts: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();
        let mut time_ms: Vec<f64> = Vec::new();
        let mut waste_percent: Vec<f64> = Vec::new();
        let mut sheets_used: Vec<f64> = Vec::new();
        let mut placeable_ratios: Vec<f64> = Vec::new();
        let mut full_placeable: u64 = 0;
        let mut timeout_reasons: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();

        for _ in 0..n {
            let body = serde_json::to_string(&base).unwrap();
            let (status, json) = post_json(app, "/v1/optimize", &body).await;
            *status_counts
                .entry(status.as_u16().to_string())
                .or_insert(0) += 1;

            if status == StatusCode::OK {
                ok_runs += 1;
                let summary = json.get("summary").cloned().unwrap_or(Value::Null);
                if let Some(v) = summary.get("time_ms").and_then(Value::as_f64) {
                    time_ms.push(v);
                }
                if let Some(v) = summary.get("waste_percent").and_then(Value::as_f64) {
                    waste_percent.push(v);
                }
                if let Some(v) = summary.get("used_stock_count").and_then(Value::as_f64) {
                    sheets_used.push(v);
                }
                if let Some(reason) = summary.get("timeout_reason").and_then(Value::as_str) {
                    *timeout_reasons.entry(reason.to_string()).or_insert(0) += 1;
                }

                let placed = json
                    .get("solutions")
                    .and_then(Value::as_array)
                    .map(|solutions| {
                        solutions
                            .iter()
                            .map(|s| {
                                s.get("placements")
                                    .and_then(Value::as_array)
                                    .map(|p| p.len())
                                    .unwrap_or(0)
                            })
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                let placeable_unplaced = json
                    .get("unplaced_items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter(|item| {
                                item.get("reason")
                                    .and_then(Value::as_str)
                                    .map(|r| r != "oversized")
                                    .unwrap_or(false)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                let placeable_total = placed + placeable_unplaced;
                let ratio = if placeable_total > 0 {
                    placed as f64 / placeable_total as f64
                } else {
                    1.0
                };
                placeable_ratios.push(ratio);
                if placeable_unplaced == 0 {
                    full_placeable += 1;
                }
            }
        }

        fn avg(vals: &[f64]) -> Option<f64> {
            if vals.is_empty() {
                None
            } else {
                Some(vals.iter().sum::<f64>() / vals.len() as f64)
            }
        }
        fn percentile(vals: &mut [f64], q: f64) -> Option<f64> {
            if vals.is_empty() {
                return None;
            }
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = ((vals.len() - 1) as f64 * q) as usize;
            vals.get(idx).copied()
        }

        let mut time_sorted = time_ms.clone();
        let mut waste_sorted = waste_percent.clone();

        serde_json::json!({
            "runs": n,
            "ok_runs": ok_runs,
            "status_counts": status_counts,
            "avg_time_ms": avg(&time_ms),
            "p50_time_ms": percentile(&mut time_sorted, 0.50),
            "p95_time_ms": percentile(&mut time_sorted, 0.95),
            "avg_waste_percent": avg(&waste_percent),
            "p50_waste_percent": percentile(&mut waste_sorted, 0.50),
            "avg_sheets_used": avg(&sheets_used),
            "avg_placeable_placed_ratio": avg(&placeable_ratios),
            "full_placeable_rate": if ok_runs > 0 { Some(full_placeable as f64 / ok_runs as f64) } else { None::<f64> },
            "timeout_reasons": timeout_reasons,
        })
    }

    let base: Value = serde_json::from_str(MULTISHEET_QTY_LIMIT_REQUEST).unwrap();
    let standard = run_series(&app, base.clone(), false, N).await;
    let portfolio = run_series(&app, base, true, N).await;

    let report = serde_json::json!({
        "fixture": "tests/fixtures/multisheet_qty_limit.json",
        "standard": standard,
        "portfolio": portfolio,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[tokio::test]
#[ignore = "probe helper for selecting benchmark scenario"]
async fn probe_standard_status_on_mdf_scenarios() {
    let app = app_for_test();
    let scenarios = [
        ("multisheet_qty_limit", MULTISHEET_QTY_LIMIT_REQUEST),
        ("multisheet_oversized", MULTISHEET_OVERSIZED_REQUEST),
    ];
    let time_limits = [1000_u64, 2000_u64, 5000_u64, 10000_u64];

    for (name, body) in scenarios {
        let base: Value = serde_json::from_str(body).unwrap();
        for tl in time_limits {
            let mut status_counts: std::collections::BTreeMap<String, u64> =
                std::collections::BTreeMap::new();
            for _ in 0..10 {
                let mut req = base.clone();
                if let Some(params) = req.get_mut("params").and_then(Value::as_object_mut) {
                    params.insert("include_svg".to_string(), Value::Bool(false));
                    params.insert("time_limit_ms".to_string(), Value::from(tl));
                    params.insert("restarts".to_string(), Value::from(2));
                    params.remove("portfolio");
                }
                let (status, _) =
                    post_json(&app, "/v1/optimize", &serde_json::to_string(&req).unwrap()).await;
                *status_counts
                    .entry(status.as_u16().to_string())
                    .or_insert(0) += 1;
            }
            println!("scenario={name}, time_limit_ms={tl}, statuses={status_counts:?}");
        }
    }
}

#[tokio::test]
#[ignore = "benchmark-style test; run manually for stage metrics"]
async fn benchmark_beam_vs_standard_250() {
    const N: usize = 250;
    let app = app_for_test();

    async fn run_series(
        app: &Router,
        mut base: Value,
        endpoint: &str,
        with_beam: bool,
        n: usize,
    ) -> Value {
        if let Some(params) = base.get_mut("params").and_then(Value::as_object_mut) {
            params.insert("include_svg".to_string(), Value::Bool(false));
            params.insert("time_limit_ms".to_string(), Value::from(2000));
            params.insert("restarts".to_string(), Value::from(2));
            if with_beam {
                params.insert(
                    "beam".to_string(),
                    serde_json::json!({
                        "enabled": true,
                        "deadline_ms": 2000,
                        "beam_width": 2,
                        "beam_depth": 2,
                        "branch_factor": 2
                    }),
                );
            } else {
                params.remove("beam");
            }
            // Ensure portfolio does not affect baseline comparison
            params.remove("portfolio");
        }

        let mut ok_runs: u64 = 0;
        let mut status_counts: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();
        let mut time_ms: Vec<f64> = Vec::new();
        let mut waste_percent: Vec<f64> = Vec::new();
        let mut sheets_used: Vec<f64> = Vec::new();
        let mut placeable_ratios: Vec<f64> = Vec::new();
        let mut full_placeable: u64 = 0;
        let mut timeout_reasons: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();

        for idx in 0..n {
            if idx % 25 == 0 {
                println!("progress endpoint={} run={}/{}", endpoint, idx, n);
            }
            let body = serde_json::to_string(&base).unwrap();
            let (status, json) = post_json(app, endpoint, &body).await;
            *status_counts
                .entry(status.as_u16().to_string())
                .or_insert(0) += 1;

            if status == StatusCode::OK {
                ok_runs += 1;
                let summary = json.get("summary").cloned().unwrap_or(Value::Null);
                if let Some(v) = summary.get("time_ms").and_then(Value::as_f64) {
                    time_ms.push(v);
                }
                if let Some(v) = summary.get("waste_percent").and_then(Value::as_f64) {
                    waste_percent.push(v);
                }
                if let Some(v) = summary.get("used_stock_count").and_then(Value::as_f64) {
                    sheets_used.push(v);
                }
                if let Some(reason) = summary.get("timeout_reason").and_then(Value::as_str) {
                    *timeout_reasons.entry(reason.to_string()).or_insert(0) += 1;
                }

                let placed = json
                    .get("solutions")
                    .and_then(Value::as_array)
                    .map(|solutions| {
                        solutions
                            .iter()
                            .map(|s| {
                                s.get("placements")
                                    .and_then(Value::as_array)
                                    .map(|p| p.len())
                                    .unwrap_or(0)
                            })
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                let placeable_unplaced = json
                    .get("unplaced_items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter(|item| {
                                item.get("reason")
                                    .and_then(Value::as_str)
                                    .map(|r| r != "oversized")
                                    .unwrap_or(false)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                let placeable_total = placed + placeable_unplaced;
                let ratio = if placeable_total > 0 {
                    placed as f64 / placeable_total as f64
                } else {
                    1.0
                };
                placeable_ratios.push(ratio);
                if placeable_unplaced == 0 {
                    full_placeable += 1;
                }
            }
        }

        fn avg(vals: &[f64]) -> Option<f64> {
            if vals.is_empty() {
                None
            } else {
                Some(vals.iter().sum::<f64>() / vals.len() as f64)
            }
        }
        fn percentile(vals: &mut [f64], q: f64) -> Option<f64> {
            if vals.is_empty() {
                return None;
            }
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = ((vals.len() - 1) as f64 * q) as usize;
            vals.get(idx).copied()
        }

        let mut time_sorted = time_ms.clone();
        let mut waste_sorted = waste_percent.clone();

        serde_json::json!({
            "endpoint": endpoint,
            "runs": n,
            "ok_runs": ok_runs,
            "status_counts": status_counts,
            "avg_time_ms": avg(&time_ms),
            "p50_time_ms": percentile(&mut time_sorted, 0.50),
            "p95_time_ms": percentile(&mut time_sorted, 0.95),
            "avg_waste_percent": avg(&waste_percent),
            "p50_waste_percent": percentile(&mut waste_sorted, 0.50),
            "avg_sheets_used": avg(&sheets_used),
            "avg_placeable_placed_ratio": avg(&placeable_ratios),
            "full_placeable_rate": if ok_runs > 0 { Some(full_placeable as f64 / ok_runs as f64) } else { None::<f64> },
            "timeout_reasons": timeout_reasons,
        })
    }

    let base: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    let standard = run_series(&app, base.clone(), "/v1/optimize", false, N).await;
    let beam = run_series(&app, base, "/v1/optimize/beam", true, N).await;

    let report = serde_json::json!({
        "fixture": "tests/fixtures/multisheet_oversized.json",
        "standard": standard,
        "beam": beam,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[tokio::test]
#[ignore = "benchmark-style test; run manually for stage metrics"]
async fn benchmark_alns_vs_standard_250() {
    const N: usize = 250;
    let app = app_for_test();

    async fn run_series(
        app: &Router,
        mut base: Value,
        endpoint: &str,
        with_alns: bool,
        n: usize,
    ) -> Value {
        if let Some(params) = base.get_mut("params").and_then(Value::as_object_mut) {
            params.insert("include_svg".to_string(), Value::Bool(false));
            params.insert("restarts".to_string(), Value::from(2));
            if with_alns {
                params.insert("time_limit_ms".to_string(), Value::from(3000));
                params.insert(
                    "alns".to_string(),
                    serde_json::json!({
                        "enabled": true,
                        "deadline_ms": 3000,
                        "iterations": 24,
                        "segment_size": 6,
                        "temperature_start": 1.0,
                        "temperature_end": 0.12,
                        "reaction_factor": 0.3
                    }),
                );
            } else {
                params.insert("time_limit_ms".to_string(), Value::from(2000));
                params.remove("alns");
            }
            params.remove("portfolio");
            params.remove("beam");
        }

        let mut ok_runs: u64 = 0;
        let mut status_counts: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();
        let mut time_ms: Vec<f64> = Vec::new();
        let mut waste_percent: Vec<f64> = Vec::new();
        let mut sheets_used: Vec<f64> = Vec::new();
        let mut placeable_ratios: Vec<f64> = Vec::new();
        let mut full_placeable: u64 = 0;
        let mut timeout_reasons: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();

        for idx in 0..n {
            if idx % 25 == 0 {
                println!("progress endpoint={} run={}/{}", endpoint, idx, n);
            }
            let body = serde_json::to_string(&base).unwrap();
            let (status, json) = post_json(app, endpoint, &body).await;
            *status_counts
                .entry(status.as_u16().to_string())
                .or_insert(0) += 1;

            if status == StatusCode::OK {
                ok_runs += 1;
                let summary = json.get("summary").cloned().unwrap_or(Value::Null);
                if let Some(v) = summary.get("time_ms").and_then(Value::as_f64) {
                    time_ms.push(v);
                }
                if let Some(v) = summary.get("waste_percent").and_then(Value::as_f64) {
                    waste_percent.push(v);
                }
                if let Some(v) = summary.get("used_stock_count").and_then(Value::as_f64) {
                    sheets_used.push(v);
                }
                if let Some(reason) = summary.get("timeout_reason").and_then(Value::as_str) {
                    *timeout_reasons.entry(reason.to_string()).or_insert(0) += 1;
                }

                let placed = json
                    .get("solutions")
                    .and_then(Value::as_array)
                    .map(|solutions| {
                        solutions
                            .iter()
                            .map(|s| {
                                s.get("placements")
                                    .and_then(Value::as_array)
                                    .map(|p| p.len())
                                    .unwrap_or(0)
                            })
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                let placeable_unplaced = json
                    .get("unplaced_items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter(|item| {
                                item.get("reason")
                                    .and_then(Value::as_str)
                                    .map(|r| r != "oversized")
                                    .unwrap_or(false)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                let placeable_total = placed + placeable_unplaced;
                let ratio = if placeable_total > 0 {
                    placed as f64 / placeable_total as f64
                } else {
                    1.0
                };
                placeable_ratios.push(ratio);
                if placeable_unplaced == 0 {
                    full_placeable += 1;
                }
            }
        }

        fn avg(vals: &[f64]) -> Option<f64> {
            if vals.is_empty() {
                None
            } else {
                Some(vals.iter().sum::<f64>() / vals.len() as f64)
            }
        }
        fn percentile(vals: &mut [f64], q: f64) -> Option<f64> {
            if vals.is_empty() {
                return None;
            }
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = ((vals.len() - 1) as f64 * q) as usize;
            vals.get(idx).copied()
        }

        let mut time_sorted = time_ms.clone();
        let mut waste_sorted = waste_percent.clone();

        serde_json::json!({
            "endpoint": endpoint,
            "runs": n,
            "ok_runs": ok_runs,
            "status_counts": status_counts,
            "avg_time_ms": avg(&time_ms),
            "p50_time_ms": percentile(&mut time_sorted, 0.50),
            "p95_time_ms": percentile(&mut time_sorted, 0.95),
            "avg_waste_percent": avg(&waste_percent),
            "p50_waste_percent": percentile(&mut waste_sorted, 0.50),
            "avg_sheets_used": avg(&sheets_used),
            "avg_placeable_placed_ratio": avg(&placeable_ratios),
            "full_placeable_rate": if ok_runs > 0 { Some(full_placeable as f64 / ok_runs as f64) } else { None::<f64> },
            "timeout_reasons": timeout_reasons,
        })
    }

    let base: Value = serde_json::from_str(MULTISHEET_OVERSIZED_REQUEST).unwrap();
    let standard = run_series(&app, base.clone(), "/v1/optimize", false, N).await;
    let alns = run_series(&app, base, "/v1/optimize/alns", true, N).await;

    let report = serde_json::json!({
        "fixture": "tests/fixtures/multisheet_oversized.json",
        "standard": standard,
        "alns": alns,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
