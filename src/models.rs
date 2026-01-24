use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct OptimizeRequest {
    pub units: Units,
    pub params: Params,
    pub stock: Vec<StockItem>,
    pub items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    #[serde(rename = "mm")]
    Mm,
}

#[derive(Debug, Deserialize)]
pub struct Params {
    pub kerf_mm: f64,
    pub spacing_mm: f64,
    pub trim_mm: Trim,
    pub time_limit_ms: u64,
    pub restarts: u32,
    pub objective: Objective,
    pub seed: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    MinWaste,
    MinSheets,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct Trim {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

#[derive(Debug, Deserialize)]
pub struct StockItem {
    pub id: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub qty: u32,
}

#[derive(Debug, Deserialize)]
pub struct Item {
    pub id: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub qty: u32,
    pub rotation: Rotation,
    pub pattern_direction: PatternDirection,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    Forbid,
    Allow90,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum PatternDirection {
    None,
    AlongWidth,
    AlongHeight,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub status: &'static str,
    pub error_code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
