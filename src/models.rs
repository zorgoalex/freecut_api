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
    pub time_limit_ms: Option<u64>,
    pub restarts: Option<u32>,
    pub objective: Objective,
    pub seed: Option<u64>,
    pub layout_mode: Option<LayoutMode>,
    /// Optional placement heuristic preset. Defaults to engine preset mix.
    pub placement_heuristic: Option<PlacementHeuristic>,
    /// Optional placement bias weights (edge/center/bbox).
    pub placement_bias: Option<PlacementBias>,
    /// Optional weights for composite fitness in the optimizer.
    pub fitness_weights: Option<FitnessWeights>,
    /// Service-level profile for restart budgeting in `/v1/optimize`. Optional, defaults to `balanced`.
    pub sla_profile: Option<SlaProfile>,
    /// GA profile for optimizer internals. Optional, defaults to `balanced`.
    pub ga_profile: Option<GaProfile>,
    /// Optional GA parameter override for advanced tuning.
    pub ga_override: Option<GaOverrideParams>,
    /// Include SVG artifact in response. Optional, defaults to true.
    pub include_svg: Option<bool>,
    /// Optional portfolio/anytime orchestration settings.
    pub portfolio: Option<PortfolioParams>,
    /// Optional beam search settings (used by `/v1/optimize/beam`).
    pub beam: Option<BeamParams>,
    /// Optional ALNS/LNS settings (used by `/v1/optimize/alns`).
    pub alns: Option<AlnsParams>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct PortfolioParams {
    /// Enable portfolio mode. Optional, defaults to true when `portfolio` object is provided.
    pub enabled: Option<bool>,
    /// Total time budget for portfolio orchestration. Optional, defaults to `time_limit_ms`.
    pub deadline_ms: Option<u64>,
    /// Number of candidate strategies in portfolio. Optional, defaults to 4.
    pub candidate_count: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct BeamParams {
    /// Enable beam mode. Optional, defaults to true when `beam` object is provided.
    pub enabled: Option<bool>,
    /// Total time budget for beam orchestration. Optional, defaults to `time_limit_ms`.
    pub deadline_ms: Option<u64>,
    /// Beam width (how many states are kept per depth level). Optional, defaults to 2.
    pub beam_width: Option<u32>,
    /// Search depth (number of expansion levels). Optional, defaults to 2.
    pub beam_depth: Option<u32>,
    /// Branch factor (expansions per state). Optional, defaults to 2.
    pub branch_factor: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct AlnsParams {
    /// Enable ALNS/LNS mode. Optional, defaults to true when `alns` object is provided.
    pub enabled: Option<bool>,
    /// Total time budget for ALNS orchestration. Optional, defaults to `time_limit_ms`.
    pub deadline_ms: Option<u64>,
    /// Requested number of ALNS iterations. Optional, defaults to 24.
    pub iterations: Option<u32>,
    /// Adaptive weight update cadence (iterations per segment). Optional, defaults to 6.
    pub segment_size: Option<u32>,
    /// Start temperature for acceptance policy. Optional, defaults to 1.0.
    pub temperature_start: Option<f64>,
    /// End temperature for acceptance policy. Optional, defaults to 0.12.
    pub temperature_end: Option<f64>,
    /// Reaction factor for adaptive operator weights in range (0, 1]. Optional, defaults to 0.3.
    pub reaction_factor: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    MinWaste,
    MinSheets,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    Nested,
    Guillotine,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlacementHeuristic {
    BestArea,
    BestShortSide,
    BestLongSide,
    WorstArea,
    WorstShortSide,
    WorstLongSide,
    SmallestY,
    BottomLeft,
    ContactPoint,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct FitnessWeights {
    /// Weight for waste minimization (legacy fitness). Optional, defaults to 1.0 when omitted.
    pub waste: Option<f64>,
    /// Weight for internal void reduction (bbox void area). Optional.
    pub void: Option<f64>,
    /// Weight for compactness (used_area / bbox_area). Optional.
    pub compactness: Option<f64>,
    /// Weight for perimeter compactness (4*sqrt(area) / perimeter). Optional.
    pub perimeter: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct PlacementBias {
    /// Penalty for placements near sheet edges. Optional.
    pub edge_penalty: Option<f64>,
    /// Pull toward the sheet center. Optional.
    pub center_pull: Option<f64>,
    /// Penalty for expanding the occupied bounding box. Optional.
    pub bbox_weight: Option<f64>,
    /// Penalty for creating thin leftover slivers in the free rectangle. Optional.
    pub fragmentation_penalty: Option<f64>,
    /// Deterministic jitter to break ties when placement scores are equal. Optional.
    pub tie_break_jitter: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SlaProfile {
    Fast,
    Balanced,
    Quality,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GaProfile {
    Fast,
    Balanced,
    Quality,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct GaOverrideParams {
    /// Number of GA epochs in range 1..=2000.
    pub epochs: Option<u32>,
    /// Breed factor in range (0, 1].
    pub breed_factor: Option<f64>,
    /// Survival factor in range [0, 1].
    pub survival_factor: Option<f64>,
    /// Top-K population candidates to evaluate by business scorer in range 1..=64.
    pub top_k_candidates: Option<u32>,
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
    /// Quantity of this stock available. If omitted or 0, unlimited sheets will be used.
    pub qty: Option<u32>,
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
    #[serde(rename = "allow_90")]
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
    /// Items that did not fit on the requested number of sheets
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unplaced_items: Vec<UnplacedItem>,
    pub artifacts: Artifacts,
}

/// Item that could not be placed on the requested sheets
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UnplacedItem {
    pub item_id: String,
    pub instance: u32,
    pub width_mm: f64,
    pub height_mm: f64,
    /// Reason why item was not placed: "oversized" or "qty_limit"
    pub reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Summary {
    pub objective: Objective,
    pub used_stock_count: u32,
    pub total_waste_area_mm2: f64,
    pub waste_percent: f64,
    pub time_ms: u64,
    pub restarts_used: u32,
    pub restarts_requested: u32,
    pub used_seed: u64,
    pub layout_mode: LayoutMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<RestartPolicyTelemetry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portfolio: Option<PortfolioTelemetry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beam: Option<BeamTelemetry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alns: Option<AlnsTelemetry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_selection: Option<CandidateSelectionTelemetry>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct RestartPolicyTelemetry {
    pub profile: SlaProfile,
    pub min_slice_ms: u64,
    pub min_effective_slice_ms: u64,
    pub restarts_cap_by_effective_slice: u64,
    pub restarts_effective: u64,
    pub baseline_budget_ms: u64,
    pub progressive_slicing: bool,
    pub planned_slices_ms: Vec<u64>,
    pub timeouts_per_restart: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_timeout_at_restart: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_found_at_restart: Option<u32>,
    pub rescue_used: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescue_budget_ms: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PortfolioTelemetry {
    pub deadline_ms: u64,
    pub candidates_total: u32,
    pub candidates_completed: u32,
    pub candidates_timed_out: u32,
    pub candidates_failed: u32,
    pub candidates_skipped: u32,
    pub winner_strategy: String,
    pub winner_seed: u64,
    pub winner_restarts_used: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BeamTelemetry {
    pub deadline_ms: u64,
    pub beam_width: u32,
    pub beam_depth: u32,
    pub branch_factor: u32,
    pub nodes_evaluated: u32,
    pub nodes_timed_out: u32,
    pub nodes_failed: u32,
    pub nodes_pruned: u32,
    pub winner_depth: u32,
    pub winner_seed: u64,
    pub winner_restarts_used: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AlnsTelemetry {
    pub deadline_ms: u64,
    pub iterations_requested: u32,
    pub iterations_completed: u32,
    pub segment_size: u32,
    pub temperature_start: f64,
    pub temperature_end: f64,
    pub reaction_factor: f64,
    pub candidates_evaluated: u32,
    pub candidates_timed_out: u32,
    pub candidates_failed: u32,
    pub accepted_worse: u32,
    pub improved_best: u32,
    pub winner_seed: u64,
    pub winner_restarts_used: u32,
    pub operators: Vec<AlnsOperatorTelemetry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AlnsOperatorTelemetry {
    pub name: String,
    pub weight: f64,
    pub selected: u32,
    pub accepted: u32,
    pub improved_best: u32,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct CandidateSelectionTelemetry {
    /// Selection source (currently top-K population scoring inside restart).
    pub source: String,
    /// Configured top-K candidate pool size.
    pub top_k_requested: u32,
    /// Total candidates seen by scorer.
    pub candidates_total: u32,
    /// Candidates with valid fitness considered for ranking.
    pub candidates_valid: u32,
    /// Candidates discarded due to invalid fitness.
    pub candidates_invalid_fitness: u32,
    /// Rejected because primary objective was worse.
    pub candidates_rejected_primary_objective: u32,
    /// Rejected by tie-break on bbox internal void area.
    pub candidates_rejected_tie_bbox_void: u32,
    /// Rejected by tie-break on bbox area.
    pub candidates_rejected_tie_bbox_area: u32,
    /// Rejected by tie-break on total perimeter.
    pub candidates_rejected_tie_perimeter: u32,
    /// Rejected because score was exactly equal to current best.
    pub candidates_rejected_equal: u32,
    /// Unique phenotype signatures among valid top-K candidates.
    pub top_k_unique_signatures: u32,
    /// Pool stats for used stock count among valid candidates.
    pub top_k_used_stock_count_min: u32,
    pub top_k_used_stock_count_max: u32,
    pub top_k_used_stock_count_mean: f64,
    /// Pool stats for waste area among valid candidates.
    pub top_k_waste_area_mm2_min: f64,
    pub top_k_waste_area_mm2_max: f64,
    pub top_k_waste_area_mm2_mean: f64,
    /// Pool stats for bbox void area among valid candidates.
    pub top_k_bbox_void_area_mm2_min: f64,
    pub top_k_bbox_void_area_mm2_max: f64,
    pub top_k_bbox_void_area_mm2_mean: f64,
    /// Pool stats for bbox area among valid candidates.
    pub top_k_bbox_area_mm2_min: f64,
    pub top_k_bbox_area_mm2_max: f64,
    pub top_k_bbox_area_mm2_mean: f64,
    /// Pool stats for total piece perimeter among valid candidates.
    pub top_k_piece_perimeter_mm_min: f64,
    pub top_k_piece_perimeter_mm_max: f64,
    pub top_k_piece_perimeter_mm_mean: f64,
    /// Winner snapshot metrics.
    pub winner_used_stock_count: u32,
    pub winner_waste_area_mm2: f64,
    pub winner_bbox_void_area_mm2: f64,
    pub winner_bbox_area_mm2: f64,
    pub winner_piece_perimeter_mm: f64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svg: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionResponse {
    pub service: &'static str,
    pub version: &'static str,
}
