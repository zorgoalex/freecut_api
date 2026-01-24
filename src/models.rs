use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct OptimizeRequest {
    pub units: Units,
    pub params: Params,
    pub stock: Vec<StockItem>,
    pub items: Vec<Item>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    #[serde(rename = "mm")]
    Mm,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct Params {
    pub kerf_mm: f64,
    pub spacing_mm: f64,
    pub trim_mm: Trim,
    pub time_limit_ms: u64,
    pub restarts: u32,
    pub objective: Objective,
    pub seed: u64,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    MinWaste,
    MinSheets,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy)]
pub struct Trim {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StockItem {
    pub id: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub qty: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct Item {
    pub id: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub qty: u32,
    pub rotation: Rotation,
    pub pattern_direction: PatternDirection,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    Forbid,
    Allow90,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatternDirection {
    None,
    AlongWidth,
    AlongHeight,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub status: &'static str,
    pub error_code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OptimizeResponse {
    pub status: &'static str,
    pub summary: Summary,
    pub solutions: Vec<Solution>,
    pub artifacts: Artifacts,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Summary {
    pub objective: Objective,
    pub used_stock_count: u32,
    pub total_waste_area_mm2: f64,
    pub waste_percent: f64,
    pub time_ms: u64,
    pub restarts_used: u32,
    pub seed: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Solution {
    pub stock_id: String,
    pub index: u32,
    pub width_mm: f64,
    pub height_mm: f64,
    pub trim_mm: Trim,
    pub placements: Vec<Placement>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Placement {
    pub item_id: String,
    pub instance: u32,
    pub x_mm: f64,
    pub y_mm: f64,
    pub width_mm: f64,
    pub height_mm: f64,
    pub rotated: bool,
    pub pattern_direction: PatternDirection,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Artifacts {
    pub svg: String,
}
