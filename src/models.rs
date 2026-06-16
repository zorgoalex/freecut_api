use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema, Clone)]
pub struct OptimizeRequest {
    pub units: Units,
    pub params: Params,
    pub stock: Vec<StockItem>,
    pub items: Vec<Item>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    #[serde(rename = "mm")]
    Mm,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct Params {
    pub kerf_mm: f64,
    pub spacing_mm: f64,
    pub trim_mm: Trim,
    pub time_limit_ms: Option<u64>,
    pub restarts: Option<u32>,
    pub objective: Objective,
    pub seed: Option<u64>,
    pub layout_mode: Option<LayoutMode>,
    /// Service-level profile for restart budgeting in `/v1/optimize`. Optional, defaults to `balanced`.
    pub sla_profile: Option<SlaProfile>,
    /// GA profile for optimizer internals. Optional, defaults to `balanced`.
    pub ga_profile: Option<GaProfile>,
    /// Optional GA parameter override for advanced tuning.
    pub ga_override: Option<GaOverrideParams>,
    /// Optional multi-profile zones-fitness orchestration for `/v1/optimize`.
    pub profile_pool: Option<ProfilePoolParams>,
    /// Include SVG artifact in response. Optional, defaults to true.
    pub include_svg: Option<bool>,
    /// Optional portfolio/anytime orchestration settings.
    pub portfolio: Option<PortfolioParams>,
    /// Optional beam search settings (used by `/v1/optimize/beam`).
    pub beam: Option<BeamParams>,
    /// Optional ALNS/LNS settings (used by `/v1/optimize/alns`).
    pub alns: Option<AlnsParams>,
    /// Fault-aware retry strategy. Optional, defaults to `smart` for the
    /// main `/v1/optimize` endpoint.  When set to `disabled`, the service
    /// returns the result of a single GA run with no recovery attempts.
    pub retry_strategy: Option<RetryStrategy>,
    /// Maximum number of attempts (including the initial one) when
    /// `retry_strategy = smart`.  Optional, defaults to 3.  Values below 1
    /// are clamped to 1 (no retry).
    pub max_retry_attempts: Option<u32>,
    /// V8a: dense-first sheet partition control via iterative peeling.
    /// When enabled, the optimizer repeatedly packs the remaining parts,
    /// freezes the densest sheet of the result as-is and re-optimizes the
    /// remainder, so the slack drains onto the last sheet instead of being
    /// smeared across all sheets.  Falls back to the regular pipeline when
    /// peeling would use more sheets.
    pub partition: Option<PartitionParams>,
    /// Optional post-process compaction that shifts peripheral side groups
    /// toward the denser anchor cluster after optimization.
    pub group_shift: Option<GroupShiftParams>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct PartitionParams {
    /// Enable dense-first peeling. Optional, defaults to true when `partition` object is provided.
    pub enabled: Option<bool>,
    /// Time budget per peeling iteration (ms). Optional, defaults to
    /// `time_limit_ms / planned_sheet_count`.
    pub sheet_budget_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct GroupShiftParams {
    /// Enable group-shift postprocess. Optional, defaults to true when
    /// `group_shift` object is provided.
    pub enabled: Option<bool>,
    /// Emit before/diff SVG artifacts for paired visual analysis. Optional,
    /// defaults to false.
    pub debug_artifacts: Option<bool>,
    /// Ignore moves smaller than this many millimeters. Optional, defaults to 5.0.
    pub min_shift_mm: Option<f64>,
    /// Maximum accepted side-group shifts. Optional, defaults to 4.
    pub max_passes: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetryStrategy {
    /// Single attempt, no retry.  Equivalent to the pre-V3 default.
    Disabled,
    /// Detect the failure mode of the first attempt and pick a recovery
    /// strategy: another seed for stochastic failures, a layout-mode swap
    /// for sheets-overflow or very-lumpy cases.  Picks the best of up to
    /// `max_retry_attempts` runs.
    Smart,
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

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy)]
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
    /// V15/V17 zones-aware GA fitness penalty. Optional, defaults to service env/default.
    pub zone_penalty: Option<f64>,
    /// V15/V17 largest-waste-component fill penalty. Optional, defaults to service env/default.
    pub fill_penalty: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
pub struct ProfilePoolParams {
    /// Enable profile-pool mode. Optional, defaults to true when `profile_pool` object is provided.
    pub enabled: Option<bool>,
    /// Named profile-pool preset. Explicit fields below override preset defaults.
    pub preset: Option<ProfilePoolPreset>,
    /// Zone-penalty profiles to evaluate. Optional, defaults to [0.2, 0.3, 0.4, 0.5].
    pub zone_penalties: Option<Vec<f64>>,
    /// Extra zone-penalty profiles evaluated only when profile-pool rescue is triggered.
    pub rescue_zone_penalties: Option<Vec<f64>>,
    /// Fill penalty used for every profile. Optional, defaults to ga_override/default 0.1.
    pub fill_penalty: Option<f64>,
    /// Maximum lead-utilisation drop allowed before a lower-zone candidate
    /// is rejected, except breakthrough layouts with <=4 zones. Optional, defaults to 0.8.
    pub max_lead_drop_pp: Option<f64>,
    /// Extra seed offsets to evaluate adaptively when the initial profile pool
    /// still has too many waste regions or too small a reusable corner.
    pub seed_offsets: Option<Vec<u64>>,
    /// Trigger adaptive seed rescue when the provisional winner has more than
    /// this many waste regions. Optional, defaults to 5 when seed_offsets exist.
    pub rescue_when_zones_gt: Option<u32>,
    /// Trigger adaptive seed rescue when the provisional winner's largest
    /// corner-free rectangle is below this area.
    pub rescue_when_max_corner_below_mm2: Option<f64>,
    /// Reject rescue candidates whose largest corner-free rectangle is below this area.
    pub rescue_accept_min_max_corner_mm2: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfilePoolPreset {
    /// V26 cheap/default mode: V20 base pool plus delayed zp=0.4 only for >5 zones.
    Cheap,
    /// V26 balanced mode: delayed zp=0.4 for 5+ zones with reusable-corner guard.
    BalancedQuality,
    /// V22 aggressive mode: always run [0.2, 0.3, 0.4, 0.5].
    Aggressive,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy)]
pub struct Trim {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

#[derive(Debug, Deserialize, ToSchema, Clone)]
pub struct StockItem {
    pub id: String,
    pub width_mm: f64,
    pub height_mm: f64,
    /// Quantity of this stock available. If omitted or 0, unlimited sheets will be used.
    pub qty: Option<u32>,
}

#[derive(Debug, Deserialize, ToSchema, Clone)]
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
    /// V17b multi-profile zones-fitness orchestration telemetry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_pool: Option<ProfilePoolTelemetry>,
    /// Fault-aware retry telemetry.  Populated only when
    /// `params.retry_strategy = smart` and at least one recovery attempt
    /// was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryTelemetry>,
    /// V8a pre-partition telemetry.  Populated when `params.partition` is
    /// enabled, including the fallback case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<PartitionTelemetry>,
    /// V29 side-group postprocess telemetry. Populated when
    /// `params.group_shift` is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_shift: Option<GroupShiftTelemetry>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct PartitionTelemetry {
    /// Whether the partitioned (peeled) packing was actually used for the
    /// final answer (false = fell back to the regular pipeline).
    pub applied: bool,
    /// Number of parts frozen on each sheet, peel order (last = remainder).
    pub group_sizes: Vec<u32>,
    /// Utilisation (%) of each frozen sheet, peel order.
    pub group_area_pct: Vec<f64>,
    /// Reason for falling back to the regular pipeline, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    /// V10: waste regions on the densest sheet for each peel iteration.
    /// Empty when partition was not applied.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub densest_zones: Vec<u32>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct GroupShiftTelemetry {
    pub enabled: bool,
    pub time_ms: u64,
    pub moves_applied: u32,
    pub parts_moved: u32,
    pub passes_run: u32,
    pub corridor_closed_area_mm2: f64,
    pub corridor_opportunity_before_mm2: f64,
    pub corridor_opportunity_after_mm2: f64,
    pub corridor_opportunity_delta_mm2: f64,
    pub max_shift_mm: f64,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct ProfilePoolTelemetry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<ProfilePoolPreset>,
    pub profiles_requested: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rescue_zone_penalties_requested: Vec<f64>,
    pub candidates_total: u32,
    pub candidates_completed: u32,
    pub candidates_timed_out: u32,
    pub candidates_failed: u32,
    pub rescue_candidates_rejected_by_guard: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub seed_offsets_requested: Vec<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub seed_offsets_used: Vec<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rescue_zone_penalties_used: Vec<f64>,
    pub rescue_triggered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescue_when_zones_gt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescue_when_max_corner_below_mm2: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescue_accept_min_max_corner_mm2: Option<f64>,
    pub winner_seed: u64,
    pub winner_zone_penalty: f64,
    /// Waste regions without part inflation (`gap=0`), closer to visual review.
    pub winner_visual_waste_regions: u32,
    /// Waste regions with kerf+spacing inflation, used by current selection.
    pub winner_waste_regions: u32,
    pub winner_lead_util_pct: f64,
    pub winner_max_corner_mm2: f64,
    pub winner_group_shift_opportunity_after_mm2: f64,
    pub winner_group_shift_opportunity_delta_mm2: f64,
    pub max_lead_drop_pp: f64,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct RetryTelemetry {
    /// Total number of optimization attempts actually executed (>= 1).
    pub attempts: u32,
    /// Number of recovery attempts after the initial one (i.e. retries).
    pub retries: u32,
    /// Strategies used for each attempt beyond the first, in order.
    /// Empty when no retry was needed.
    pub strategies: Vec<String>,
    /// Description of the failure mode detected on the first attempt, if
    /// any.  Drives which strategy is picked for the first retry.
    pub initial_failure: Option<String>,
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
    /// Rejected by tie-break on minimum per-sheet utilisation (unbalanced layout).
    pub candidates_rejected_tie_min_util: u32,
    /// Rejected by tie-break on corner free rectangle area (V9: waste consolidation).
    pub candidates_rejected_tie_corner_free: u32,
    /// Rejected because score was exactly equal to current best.
    pub candidates_rejected_equal: u32,
    /// Winner snapshot metrics.
    pub winner_used_stock_count: u32,
    pub winner_waste_area_mm2: f64,
    pub winner_bbox_void_area_mm2: f64,
    pub winner_bbox_area_mm2: f64,
    pub winner_piece_perimeter_mm: f64,
    /// Winner's minimum per-sheet utilisation (%). Higher = more balanced layout.
    pub winner_min_sheet_util_pct: f64,
    /// Winner's maximum edge gap across all sheets (mm). Telemetry only (V9).
    pub winner_max_edge_gap_mm: f64,
    /// Winner's per-sheet utilisation standard deviation (%). Telemetry only (V9).
    pub winner_sheet_util_spread_pct: f64,
    /// Winner's total corner-anchored free rectangle area (mm^2). Higher =
    /// waste consolidated into one reusable corner remnant (V9).
    pub winner_corner_free_area_mm2: f64,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct Solution {
    pub stock_id: String,
    pub index: u32,
    pub width_mm: f64,
    pub height_mm: f64,
    pub trim_mm: Trim,
    pub placements: Vec<Placement>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_shift_before_svg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_shift_diff_svg: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionResponse {
    pub service: &'static str,
    pub version: &'static str,
}
