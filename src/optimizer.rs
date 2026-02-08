use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cut_optimizer_2d::{
    CutPiece, FitnessWeights as EngineFitnessWeights, Optimizer,
    PatternDirection as CutPatternDirection, PlacementBias as EnginePlacementBias, StockPiece,
};

use crate::config::AppConfig;
use crate::models::{
    AlnsOperatorTelemetry, AlnsTelemetry, Artifacts, BeamTelemetry, CandidateSelectionTelemetry,
    ErrorResponse, FitnessWeights, GaProfile, LayoutMode, Objective, OptimizeRequest,
    OptimizeResponse, PatternDirection, Placement, PlacementHeuristic, PortfolioTelemetry,
    RestartPolicyTelemetry, SlaProfile, Solution, Summary, Trim, UnplacedItem,
};
use crate::validation::item_fits_any_stock_public;

const SCALE: f64 = 1000.0;
const MIN_SLICE_MS: u64 = 80;
const SEED_STRIDE: u64 = 1_000_003;
const EARLY_STOP_NO_IMPROVE_PATIENCE: u32 = 4;
const PORTFOLIO_DEFAULT_CANDIDATES: u32 = 4;
const PORTFOLIO_MAX_CANDIDATES: u32 = 16;
const BEAM_DEFAULT_WIDTH: u32 = 2;
const BEAM_DEFAULT_DEPTH: u32 = 2;
const BEAM_DEFAULT_BRANCH_FACTOR: u32 = 2;
const BEAM_MAX_RESTARTS: u32 = 64;
const ALNS_DEFAULT_ITERATIONS: u32 = 24;
const ALNS_DEFAULT_SEGMENT_SIZE: u32 = 6;
const ALNS_DEFAULT_TEMP_START: f64 = 1.0;
const ALNS_DEFAULT_TEMP_END: f64 = 0.12;
const ALNS_DEFAULT_REACTION: f64 = 0.3;
const ALNS_MAX_RESTARTS: u32 = 64;
const ALNS_MIN_BASELINE_BUDGET_MS: u64 = 1_200;
const ALNS_MIN_ITER_BUDGET_MS: u64 = 500;
const ALNS_MAX_ITER_RESTARTS: u32 = 2;
const ALNS_MAX_TIMEOUT_STREAK: u32 = 2;
const STANDARD_FAST_MIN_EFFECTIVE_SLICE_MS: u64 = 700;
const STANDARD_FAST_BASELINE_SHARE_PERCENT: u64 = 50;
const STANDARD_BALANCED_MIN_EFFECTIVE_SLICE_MS: u64 = 550;
const STANDARD_BALANCED_BASELINE_SHARE_PERCENT: u64 = 40;
const STANDARD_QUALITY_MIN_EFFECTIVE_SLICE_MS: u64 = 400;
const STANDARD_QUALITY_BASELINE_SHARE_PERCENT: u64 = 35;
const GA_FAST_EPOCHS: u32 = 60;
const GA_BALANCED_EPOCHS: u32 = 100;
const GA_QUALITY_EPOCHS: u32 = 180;
const GA_FAST_BREED_FACTOR: f64 = 0.45;
const GA_BALANCED_BREED_FACTOR: f64 = 0.5;
const GA_QUALITY_BREED_FACTOR: f64 = 0.55;
const GA_FAST_SURVIVAL_FACTOR: f64 = 0.5;
const GA_BALANCED_SURVIVAL_FACTOR: f64 = 0.6;
const GA_QUALITY_SURVIVAL_FACTOR: f64 = 0.7;
const GA_FAST_TOP_K: usize = 3;
const GA_BALANCED_TOP_K: usize = 6;
const GA_QUALITY_TOP_K: usize = 12;

#[derive(Debug)]
pub enum OptimizeError {
    Timeout,
    Constraint {
        message: String,
        details: Option<serde_json::Value>,
    },
    Internal(String),
}

impl IntoResponse for OptimizeError {
    fn into_response(self) -> Response {
        match self {
            OptimizeError::Timeout => error_response(
                StatusCode::REQUEST_TIMEOUT,
                "TIMEOUT",
                "optimization timed out",
                None,
            ),
            OptimizeError::Constraint { message, details } => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "CONSTRAINT_ERROR",
                &message,
                details,
            ),
            OptimizeError::Internal(message) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL",
                &message,
                None,
            ),
        }
    }
}

#[derive(Clone)]
struct InstanceInfo {
    item_id: String,
    instance: u32,
    pattern_direction: PatternDirection,
}

#[derive(Clone)]
struct StockInfo {
    stock_id: String,
    full_width_mm: f64,
    full_height_mm: f64,
    /// User-specified qty limit (None = unlimited)
    qty_limit: Option<u32>,
}

#[derive(Clone)]
struct PreparedInput {
    stock_pieces: Vec<StockPiece>,
    cut_pieces: Vec<CutPiece>,
    instance_map: Vec<InstanceInfo>,
    stock_map: HashMap<(usize, usize), Vec<StockInfo>>,
    trim: Trim,
    cut_width: usize,
    /// Items that don't fit any stock sheet (oversized)
    oversized_items: Vec<UnplacedItem>,
}

struct Candidate {
    solution: cut_optimizer_2d::Solution,
    used_stock_count: u32,
    total_waste_area_units: u128,
    total_bbox_area_units: u128,
    total_bbox_void_area_units: u128,
    total_piece_perimeter_units: u128,
}

#[derive(Clone, Copy)]
struct GaRuntime {
    epochs: u32,
    breed_factor: f64,
    survival_factor: f64,
    top_k: usize,
}

#[derive(Clone)]
struct RestartPlan {
    profile: SlaProfile,
    min_effective_slice_ms: u64,
    cap_by_effective_slice: u64,
    baseline_budget_ms: u64,
    progressive_slicing: bool,
    effective_restarts: u64,
    fallback_slice_ms: u64,
    schedule_ms: Vec<u64>,
}

#[derive(Default, Clone)]
struct CandidateSelectionCounters {
    top_k_requested: u32,
    candidates_total: u32,
    candidates_valid: u32,
    candidates_invalid_fitness: u32,
    candidates_rejected_primary_objective: u32,
    candidates_rejected_tie_bbox_void: u32,
    candidates_rejected_tie_bbox_area: u32,
    candidates_rejected_tie_perimeter: u32,
    candidates_rejected_equal: u32,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct CandidateSignature {
    used_stock_count: u32,
    total_waste_area_units: u128,
    total_bbox_area_units: u128,
    total_bbox_void_area_units: u128,
    total_piece_perimeter_units: u128,
}

#[derive(Default, Clone)]
struct CandidatePoolStats {
    count: u32,
    unique_signatures: HashSet<CandidateSignature>,
    used_stock_min: u32,
    used_stock_max: u32,
    used_stock_sum: u64,
    waste_min: u128,
    waste_max: u128,
    waste_sum: u128,
    bbox_void_min: u128,
    bbox_void_max: u128,
    bbox_void_sum: u128,
    bbox_area_min: u128,
    bbox_area_max: u128,
    bbox_area_sum: u128,
    perimeter_min: u128,
    perimeter_max: u128,
    perimeter_sum: u128,
}

impl CandidatePoolStats {
    fn observe(&mut self, candidate: &Candidate) {
        let signature = CandidateSignature {
            used_stock_count: candidate.used_stock_count,
            total_waste_area_units: candidate.total_waste_area_units,
            total_bbox_area_units: candidate.total_bbox_area_units,
            total_bbox_void_area_units: candidate.total_bbox_void_area_units,
            total_piece_perimeter_units: candidate.total_piece_perimeter_units,
        };
        self.unique_signatures.insert(signature);
        let first = self.count == 0;
        if first {
            self.used_stock_min = candidate.used_stock_count;
            self.used_stock_max = candidate.used_stock_count;
            self.waste_min = candidate.total_waste_area_units;
            self.waste_max = candidate.total_waste_area_units;
            self.bbox_void_min = candidate.total_bbox_void_area_units;
            self.bbox_void_max = candidate.total_bbox_void_area_units;
            self.bbox_area_min = candidate.total_bbox_area_units;
            self.bbox_area_max = candidate.total_bbox_area_units;
            self.perimeter_min = candidate.total_piece_perimeter_units;
            self.perimeter_max = candidate.total_piece_perimeter_units;
        } else {
            self.used_stock_min = self.used_stock_min.min(candidate.used_stock_count);
            self.used_stock_max = self.used_stock_max.max(candidate.used_stock_count);
            self.waste_min = self.waste_min.min(candidate.total_waste_area_units);
            self.waste_max = self.waste_max.max(candidate.total_waste_area_units);
            self.bbox_void_min = self.bbox_void_min.min(candidate.total_bbox_void_area_units);
            self.bbox_void_max = self.bbox_void_max.max(candidate.total_bbox_void_area_units);
            self.bbox_area_min = self.bbox_area_min.min(candidate.total_bbox_area_units);
            self.bbox_area_max = self.bbox_area_max.max(candidate.total_bbox_area_units);
            self.perimeter_min = self
                .perimeter_min
                .min(candidate.total_piece_perimeter_units);
            self.perimeter_max = self
                .perimeter_max
                .max(candidate.total_piece_perimeter_units);
        }
        self.used_stock_sum = self
            .used_stock_sum
            .saturating_add(candidate.used_stock_count as u64);
        self.waste_sum = self
            .waste_sum
            .saturating_add(candidate.total_waste_area_units);
        self.bbox_void_sum = self
            .bbox_void_sum
            .saturating_add(candidate.total_bbox_void_area_units);
        self.bbox_area_sum = self
            .bbox_area_sum
            .saturating_add(candidate.total_bbox_area_units);
        self.perimeter_sum = self
            .perimeter_sum
            .saturating_add(candidate.total_piece_perimeter_units);
        self.count = self.count.saturating_add(1);
    }
}

enum CandidateCompare {
    Better,
    WorseByPrimaryObjective,
    WorseByTieBboxVoid,
    WorseByTieBboxArea,
    WorseByTiePerimeter,
    Equal,
}

pub async fn optimize_request(
    req: OptimizeRequest,
    config: &AppConfig,
) -> Result<OptimizeResponse, OptimizeError> {
    optimize_request_internal(req, config, SolveMode::Default).await
}

pub async fn optimize_request_beam(
    req: OptimizeRequest,
    config: &AppConfig,
) -> Result<OptimizeResponse, OptimizeError> {
    optimize_request_internal(req, config, SolveMode::BeamEndpoint).await
}

pub async fn optimize_request_alns(
    req: OptimizeRequest,
    config: &AppConfig,
) -> Result<OptimizeResponse, OptimizeError> {
    optimize_request_internal(req, config, SolveMode::AlnsEndpoint).await
}

#[derive(Clone, Copy)]
enum SolveMode {
    Default,
    BeamEndpoint,
    AlnsEndpoint,
}

async fn optimize_request_internal(
    req: OptimizeRequest,
    config: &AppConfig,
    mode: SolveMode,
) -> Result<OptimizeResponse, OptimizeError> {
    let layout_mode = req.params.layout_mode.unwrap_or(LayoutMode::Guillotine);
    let include_svg = req.params.include_svg.unwrap_or(true);
    let used_seed = req.params.seed.unwrap_or_else(generate_seed);
    let prepared = prepare_input(&req)?;

    let time_limit_ms = req
        .params
        .time_limit_ms
        .unwrap_or(config.default_time_limit_ms);
    let restarts_requested = req
        .params
        .restarts
        .unwrap_or(config.default_restarts)
        .max(1);
    let sla_profile = req.params.sla_profile.unwrap_or(SlaProfile::Balanced);
    let standard_restart_plan =
        derive_standard_restart_plan(time_limit_ms, restarts_requested, sla_profile);

    let start = Instant::now();

    // Handle case where all items are oversized (nothing to optimize)
    if prepared.cut_pieces.is_empty() {
        let time_ms = start.elapsed().as_millis() as u64;
        let svg = if include_svg {
            Some(build_svg(&[], &prepared.oversized_items, &prepared.trim))
        } else {
            None
        };
        return Ok(OptimizeResponse {
            status: "ok",
            summary: Summary {
                objective: req.params.objective,
                used_stock_count: 0,
                total_waste_area_mm2: 0.0,
                waste_percent: 0.0,
                time_ms,
                restarts_used: 0,
                restarts_requested,
                used_seed,
                layout_mode,
                timeout_reason: None,
                restart_policy: None,
                portfolio: None,
                beam: None,
                alns: None,
                candidate_selection: None,
            },
            solutions: vec![],
            unplaced_items: prepared.oversized_items,
            artifacts: Artifacts { svg },
        });
    }

    let run_outcome = match mode {
        SolveMode::Default => match &req.params.portfolio {
            Some(portfolio_cfg) if portfolio_cfg.enabled.unwrap_or(true) => {
                let deadline_ms = portfolio_cfg.deadline_ms.unwrap_or(time_limit_ms).max(100);
                let candidates = portfolio_cfg
                    .candidate_count
                    .unwrap_or(PORTFOLIO_DEFAULT_CANDIDATES)
                    .clamp(1, PORTFOLIO_MAX_CANDIDATES);
                run_portfolio_anytime(
                    &req,
                    &prepared,
                    layout_mode,
                    used_seed,
                    deadline_ms,
                    candidates,
                    restarts_requested,
                )
                .await?
            }
            _ => {
                run_restarts_with_budget(
                    &req,
                    &prepared,
                    standard_restart_plan.effective_restarts,
                    standard_restart_plan.fallback_slice_ms,
                    layout_mode,
                    used_seed,
                    req.params.placement_heuristic,
                    time_limit_ms,
                    true,
                    Some(&standard_restart_plan),
                )
                .await?
            }
        },
        SolveMode::BeamEndpoint => {
            let beam_cfg = req.params.beam.as_ref();
            let deadline_ms = beam_cfg
                .and_then(|c| c.deadline_ms)
                .unwrap_or(time_limit_ms)
                .max(100);
            let beam_width = beam_cfg
                .and_then(|c| c.beam_width)
                .unwrap_or(BEAM_DEFAULT_WIDTH)
                .clamp(1, 8);
            let beam_depth = beam_cfg
                .and_then(|c| c.beam_depth)
                .unwrap_or(BEAM_DEFAULT_DEPTH)
                .clamp(1, 8);
            let branch_factor = beam_cfg
                .and_then(|c| c.branch_factor)
                .unwrap_or(BEAM_DEFAULT_BRANCH_FACTOR)
                .clamp(1, 8);
            run_beam_anytime(
                &req,
                &prepared,
                layout_mode,
                used_seed,
                deadline_ms,
                beam_width,
                beam_depth,
                branch_factor,
                restarts_requested,
            )
            .await?
        }
        SolveMode::AlnsEndpoint => {
            let alns_cfg = req.params.alns.as_ref();
            let deadline_ms = alns_cfg
                .and_then(|c| c.deadline_ms)
                .unwrap_or(time_limit_ms)
                .max(100);
            let iterations = alns_cfg
                .and_then(|c| c.iterations)
                .unwrap_or(ALNS_DEFAULT_ITERATIONS)
                .clamp(1, 512);
            let segment_size = alns_cfg
                .and_then(|c| c.segment_size)
                .unwrap_or(ALNS_DEFAULT_SEGMENT_SIZE)
                .clamp(1, 64);
            let temperature_start = alns_cfg
                .and_then(|c| c.temperature_start)
                .unwrap_or(ALNS_DEFAULT_TEMP_START)
                .max(0.0001);
            let temperature_end = alns_cfg
                .and_then(|c| c.temperature_end)
                .unwrap_or(ALNS_DEFAULT_TEMP_END)
                .max(0.0001)
                .min(temperature_start);
            let reaction_factor = alns_cfg
                .and_then(|c| c.reaction_factor)
                .unwrap_or(ALNS_DEFAULT_REACTION)
                .clamp(0.0001, 1.0);
            run_alns_anytime(
                &req,
                &prepared,
                layout_mode,
                used_seed,
                deadline_ms,
                iterations,
                segment_size,
                temperature_start,
                temperature_end,
                reaction_factor,
                restarts_requested,
            )
            .await?
        }
    };

    let time_ms = start.elapsed().as_millis() as u64;

    let all_solutions = build_solutions(&run_outcome.candidate.solution, &prepared);

    // Apply qty limits and collect unplaced items
    let (solutions, mut unplaced_items) = apply_qty_limits(all_solutions, &prepared);

    // Merge oversized items (items that didn't fit any stock)
    unplaced_items.extend(prepared.oversized_items.clone());

    // Recalculate stats for kept solutions only
    let (used_stock_count, total_stock_area, total_waste_area) =
        calculate_solution_stats(&solutions);

    let summary = Summary {
        objective: req.params.objective,
        used_stock_count,
        total_waste_area_mm2: total_waste_area,
        waste_percent: if total_stock_area > 0.0 {
            100.0 * total_waste_area / total_stock_area
        } else {
            0.0
        },
        time_ms,
        restarts_used: run_outcome.restarts_used,
        restarts_requested,
        used_seed,
        layout_mode,
        timeout_reason: run_outcome.timeout_reason,
        restart_policy: run_outcome.restart_policy,
        portfolio: run_outcome.portfolio,
        beam: run_outcome.beam,
        alns: run_outcome.alns,
        candidate_selection: run_outcome.candidate_selection,
    };

    let svg = if include_svg {
        Some(build_svg(&solutions, &unplaced_items, &prepared.trim))
    } else {
        None
    };

    Ok(OptimizeResponse {
        status: "ok",
        summary,
        solutions,
        unplaced_items,
        artifacts: Artifacts { svg },
    })
}

struct RunOutcome {
    candidate: Candidate,
    restarts_used: u32,
    timeout_reason: Option<String>,
    restart_policy: Option<RestartPolicyTelemetry>,
    portfolio: Option<PortfolioTelemetry>,
    beam: Option<BeamTelemetry>,
    alns: Option<AlnsTelemetry>,
    candidate_selection: Option<CandidateSelectionTelemetry>,
}

#[derive(Clone)]
struct PortfolioPlan {
    name: String,
    seed: u64,
    requested_restarts: u32,
    placement_heuristic: Option<PlacementHeuristic>,
}

#[derive(Clone)]
struct BeamPlan {
    seed: u64,
    requested_restarts: u32,
}

struct BeamCandidate {
    candidate: Candidate,
    seed: u64,
    requested_restarts: u32,
    depth: u32,
    restarts_used: u32,
    candidate_selection: Option<CandidateSelectionTelemetry>,
}

#[derive(Clone)]
struct AlnsPlan {
    seed: u64,
    requested_restarts: u32,
}

struct AlnsOperatorState {
    name: &'static str,
    weight: f64,
    score: f64,
    selected: u32,
    accepted: u32,
    improved_best: u32,
}

fn resolve_ga_runtime(req: &OptimizeRequest) -> GaRuntime {
    let profile = req.params.ga_profile.unwrap_or(GaProfile::Balanced);
    let mut runtime = match profile {
        GaProfile::Fast => GaRuntime {
            epochs: GA_FAST_EPOCHS,
            breed_factor: GA_FAST_BREED_FACTOR,
            survival_factor: GA_FAST_SURVIVAL_FACTOR,
            top_k: GA_FAST_TOP_K,
        },
        GaProfile::Balanced => GaRuntime {
            epochs: GA_BALANCED_EPOCHS,
            breed_factor: GA_BALANCED_BREED_FACTOR,
            survival_factor: GA_BALANCED_SURVIVAL_FACTOR,
            top_k: GA_BALANCED_TOP_K,
        },
        GaProfile::Quality => GaRuntime {
            epochs: GA_QUALITY_EPOCHS,
            breed_factor: GA_QUALITY_BREED_FACTOR,
            survival_factor: GA_QUALITY_SURVIVAL_FACTOR,
            top_k: GA_QUALITY_TOP_K,
        },
    };
    if let Some(override_cfg) = &req.params.ga_override {
        if let Some(epochs) = override_cfg.epochs {
            runtime.epochs = epochs.max(1);
        }
        if let Some(breed_factor) = override_cfg.breed_factor {
            runtime.breed_factor = breed_factor.clamp(0.0001, 1.0);
        }
        if let Some(survival_factor) = override_cfg.survival_factor {
            runtime.survival_factor = survival_factor.clamp(0.0, 1.0);
        }
        if let Some(top_k_candidates) = override_cfg.top_k_candidates {
            runtime.top_k = usize::try_from(top_k_candidates)
                .unwrap_or(GA_BALANCED_TOP_K)
                .max(1);
        }
    }
    runtime
}

fn resolve_fitness_weights(weights: Option<&FitnessWeights>) -> EngineFitnessWeights {
    match weights {
        None => EngineFitnessWeights::default(),
        Some(weights) => EngineFitnessWeights {
            waste: weights.waste.unwrap_or(1.0),
            void: weights.void.unwrap_or(0.0),
            compactness: weights.compactness.unwrap_or(0.0),
            perimeter: weights.perimeter.unwrap_or(0.0),
        },
    }
}

fn resolve_placement_bias(bias: Option<&crate::models::PlacementBias>) -> EnginePlacementBias {
    match bias {
        None => EnginePlacementBias::default(),
        Some(bias) => EnginePlacementBias {
            edge_penalty: bias.edge_penalty.unwrap_or(0.0),
            center_pull: bias.center_pull.unwrap_or(0.0),
            bbox_weight: bias.bbox_weight.unwrap_or(0.0),
            fragmentation_penalty: bias.fragmentation_penalty.unwrap_or(0.0),
            tie_break_jitter: bias.tie_break_jitter.unwrap_or(0.0),
        },
    }
}

fn pick_best_candidate(
    set: cut_optimizer_2d::SolutionSet,
    objective: &Objective,
    counters: &mut CandidateSelectionCounters,
    pool_stats: &mut CandidatePoolStats,
) -> Option<Candidate> {
    let mut best: Option<Candidate> = None;
    for solution in set.solutions {
        counters.candidates_total = counters.candidates_total.saturating_add(1);
        if solution.fitness < 0.0 {
            counters.candidates_invalid_fitness =
                counters.candidates_invalid_fitness.saturating_add(1);
            continue;
        }
        let candidate = build_candidate(solution);
        counters.candidates_valid = counters.candidates_valid.saturating_add(1);
        pool_stats.observe(&candidate);
        best = match best {
            None => Some(candidate),
            Some(current) => match compare_candidates(&candidate, &current, objective) {
                CandidateCompare::Better => Some(candidate),
                CandidateCompare::WorseByPrimaryObjective => {
                    counters.candidates_rejected_primary_objective = counters
                        .candidates_rejected_primary_objective
                        .saturating_add(1);
                    Some(current)
                }
                CandidateCompare::WorseByTieBboxVoid => {
                    counters.candidates_rejected_tie_bbox_void =
                        counters.candidates_rejected_tie_bbox_void.saturating_add(1);
                    Some(current)
                }
                CandidateCompare::WorseByTieBboxArea => {
                    counters.candidates_rejected_tie_bbox_area =
                        counters.candidates_rejected_tie_bbox_area.saturating_add(1);
                    Some(current)
                }
                CandidateCompare::WorseByTiePerimeter => {
                    counters.candidates_rejected_tie_perimeter =
                        counters.candidates_rejected_tie_perimeter.saturating_add(1);
                    Some(current)
                }
                CandidateCompare::Equal => {
                    counters.candidates_rejected_equal =
                        counters.candidates_rejected_equal.saturating_add(1);
                    Some(current)
                }
            },
        };
    }
    best
}

async fn run_restarts_with_budget(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    restarts: u64,
    slice_ms: u64,
    layout_mode: LayoutMode,
    base_seed: u64,
    placement_heuristic: Option<PlacementHeuristic>,
    total_budget_ms: u64,
    allow_timeout_rescue: bool,
    restart_plan: Option<&RestartPlan>,
) -> Result<RunOutcome, OptimizeError> {
    let ga_runtime = resolve_ga_runtime(req);
    let guillotine_preset = placement_heuristic.and_then(to_guillotine_preset);
    let nested_preset = placement_heuristic.and_then(to_nested_preset);
    let fitness_weights = resolve_fitness_weights(req.params.fitness_weights.as_ref());
    let placement_bias = resolve_placement_bias(req.params.placement_bias.as_ref());
    let mut best: Option<Candidate> = None;
    let mut selection_counters = CandidateSelectionCounters {
        top_k_requested: u32::try_from(ga_runtime.top_k).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut pool_stats = CandidatePoolStats::default();
    let mut timed_out = false;
    let mut budget_exhausted = false;
    let mut last_constraint: Option<OptimizeError> = None;
    let started_at = Instant::now();
    let mut restarts_used: u32 = 0;
    let mut no_improve_streak: u32 = 0;
    let mut timeouts_per_restart: u32 = 0;
    let mut first_timeout_at_restart: Option<u32> = None;
    let mut best_found_at_restart: Option<u32> = None;
    let mut rescue_used = false;
    let mut rescue_budget_ms: Option<u64> = None;

    // Keep one shared copy per request and avoid allocating/cloning a full Vec on each restart.
    let stock_templates = Arc::new(prepared.stock_pieces.clone());
    let cut_templates = Arc::new(prepared.cut_pieces.clone());

    let slice_schedule_ms = restart_plan.map(|p| p.schedule_ms.as_slice());
    let planned_restarts = slice_schedule_ms
        .map(|s| s.len() as u64)
        .unwrap_or(restarts.max(1));
    for i in 0..planned_restarts {
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        if elapsed_ms >= total_budget_ms {
            budget_exhausted = true;
            break;
        }
        let remaining_ms = total_budget_ms - elapsed_ms;
        let planned_slice_ms = slice_schedule_ms
            .and_then(|s| s.get(i as usize))
            .copied()
            .unwrap_or(slice_ms);
        let this_slice_ms = remaining_ms.min(planned_slice_ms).max(1);

        let seed = base_seed.wrapping_add(i.wrapping_mul(SEED_STRIDE));
        let mode = layout_mode;
        let stock_templates = Arc::clone(&stock_templates);
        let cut_templates = Arc::clone(&cut_templates);
        let cut_width = prepared.cut_width;
        let restart_idx = i;
        let ga_runtime = ga_runtime;
        let fitness_weights = fitness_weights;
        let placement_bias = placement_bias;

        let mut handle = tokio::task::spawn_blocking(move || {
            let diversified_stock = diversify_stock_order(&stock_templates, seed, restart_idx);
            let diversified_cut = diversify_cut_order(&cut_templates, seed, restart_idx);
            let mut optimizer = Optimizer::new();
            optimizer
                .set_random_seed(seed)
                .set_cut_width(cut_width)
                .set_fitness_weights(fitness_weights)
                .set_placement_bias(placement_bias)
                .set_ga_epochs(ga_runtime.epochs)
                .set_ga_breed_factor(ga_runtime.breed_factor)
                .set_ga_survival_factor(ga_runtime.survival_factor)
                .add_stock_pieces(diversified_stock.into_iter())
                .add_cut_pieces(diversified_cut.into_iter());
            match mode {
                LayoutMode::Nested => match nested_preset {
                    Some(preset) => optimizer.optimize_nested_top_k_with_heuristic(
                        ga_runtime.top_k,
                        preset,
                        |_| {},
                    ),
                    None => optimizer.optimize_nested_top_k(ga_runtime.top_k, |_| {}),
                },
                LayoutMode::Guillotine => match guillotine_preset {
                    Some(preset) => optimizer.optimize_guillotine_top_k_with_heuristic(
                        ga_runtime.top_k,
                        preset,
                        |_| {},
                    ),
                    None => optimizer.optimize_guillotine_top_k(ga_runtime.top_k, |_| {}),
                },
            }
        });

        let run = tokio::time::timeout(Duration::from_millis(this_slice_ms), &mut handle).await;
        match run {
            Ok(join_result) => match join_result {
                Ok(Ok(solution_set)) => {
                    restarts_used += 1;
                    let Some(candidate) = pick_best_candidate(
                        solution_set,
                        &req.params.objective,
                        &mut selection_counters,
                        &mut pool_stats,
                    ) else {
                        last_constraint = Some(OptimizeError::Constraint {
                            message: "no valid solution".to_string(),
                            details: None,
                        });
                        continue;
                    };
                    if best_found_at_restart.is_none() {
                        best_found_at_restart = Some((i + 1) as u32);
                    }
                    best = match best {
                        None => {
                            no_improve_streak = 0;
                            Some(candidate)
                        }
                        Some(current) => {
                            match compare_candidates(&candidate, &current, &req.params.objective) {
                                CandidateCompare::Better => {
                                    no_improve_streak = 0;
                                    Some(candidate)
                                }
                                CandidateCompare::WorseByPrimaryObjective
                                | CandidateCompare::WorseByTieBboxVoid
                                | CandidateCompare::WorseByTieBboxArea
                                | CandidateCompare::WorseByTiePerimeter
                                | CandidateCompare::Equal => {
                                    no_improve_streak = no_improve_streak.saturating_add(1);
                                    Some(current)
                                }
                            }
                        }
                    };
                    if no_improve_streak >= EARLY_STOP_NO_IMPROVE_PATIENCE {
                        break;
                    }
                }
                Ok(Err(err)) => {
                    restarts_used += 1;
                    last_constraint = Some(map_optimizer_error(err, prepared));
                    no_improve_streak = no_improve_streak.saturating_add(1);
                    if no_improve_streak >= EARLY_STOP_NO_IMPROVE_PATIENCE {
                        break;
                    }
                }
                Err(err) => {
                    return Err(OptimizeError::Internal(format!(
                        "optimizer task failed: {err}"
                    )));
                }
            },
            Err(_) => {
                timed_out = true;
                timeouts_per_restart = timeouts_per_restart.saturating_add(1);
                if first_timeout_at_restart.is_none() {
                    first_timeout_at_restart = Some((i + 1) as u32);
                }
                // Best-effort abort. The blocking task may still run to completion.
                handle.abort();
                // Do not launch more heavy tasks once one slice timed out.
                break;
            }
        }
    }

    if let Some(best) = best {
        let candidate_selection = Some(build_candidate_selection_telemetry(
            &selection_counters,
            &pool_stats,
            &best,
        ));
        let restart_policy = restart_plan.map(|plan| RestartPolicyTelemetry {
            profile: plan.profile,
            min_slice_ms: MIN_SLICE_MS,
            min_effective_slice_ms: plan.min_effective_slice_ms,
            restarts_cap_by_effective_slice: plan.cap_by_effective_slice,
            restarts_effective: plan.effective_restarts,
            baseline_budget_ms: plan.baseline_budget_ms,
            progressive_slicing: plan.progressive_slicing,
            planned_slices_ms: plan.schedule_ms.clone(),
            timeouts_per_restart,
            first_timeout_at_restart,
            best_found_at_restart,
            rescue_used,
            rescue_budget_ms,
        });
        let timeout_reason = if timed_out {
            Some("slice_timeout".to_string())
        } else if budget_exhausted {
            Some("time_budget_exhausted".to_string())
        } else {
            None
        };
        return Ok(RunOutcome {
            candidate: best,
            restarts_used,
            timeout_reason,
            restart_policy,
            portfolio: None,
            beam: None,
            alns: None,
            candidate_selection,
        });
    }

    if timed_out && allow_timeout_rescue {
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        let rescue_budget = total_budget_ms.saturating_sub(elapsed_ms);
        if rescue_budget >= MIN_SLICE_MS {
            let rescue_seed = base_seed.wrapping_add(planned_restarts.wrapping_mul(SEED_STRIDE));
            let mode = layout_mode;
            let stock_templates = Arc::clone(&stock_templates);
            let cut_templates = Arc::clone(&cut_templates);
            let cut_width = prepared.cut_width;
            let restart_idx = planned_restarts;
            let ga_runtime = ga_runtime;
            let fitness_weights = fitness_weights;
            let placement_bias = placement_bias;
            let mut rescue_handle = tokio::task::spawn_blocking(move || {
                let diversified_stock =
                    diversify_stock_order(&stock_templates, rescue_seed, restart_idx);
                let diversified_cut = diversify_cut_order(&cut_templates, rescue_seed, restart_idx);
                let mut optimizer = Optimizer::new();
                optimizer
                    .set_random_seed(rescue_seed)
                    .set_cut_width(cut_width)
                    .set_fitness_weights(fitness_weights)
                    .set_placement_bias(placement_bias)
                    .set_ga_epochs(ga_runtime.epochs)
                    .set_ga_breed_factor(ga_runtime.breed_factor)
                    .set_ga_survival_factor(ga_runtime.survival_factor)
                    .add_stock_pieces(diversified_stock.into_iter())
                    .add_cut_pieces(diversified_cut.into_iter());
                match mode {
                    LayoutMode::Nested => match nested_preset {
                        Some(preset) => optimizer.optimize_nested_top_k_with_heuristic(
                            ga_runtime.top_k,
                            preset,
                            |_| {},
                        ),
                        None => optimizer.optimize_nested_top_k(ga_runtime.top_k, |_| {}),
                    },
                    LayoutMode::Guillotine => match guillotine_preset {
                        Some(preset) => optimizer.optimize_guillotine_top_k_with_heuristic(
                            ga_runtime.top_k,
                            preset,
                            |_| {},
                        ),
                        None => optimizer.optimize_guillotine_top_k(ga_runtime.top_k, |_| {}),
                    },
                }
            });

            let rescue_run =
                tokio::time::timeout(Duration::from_millis(rescue_budget), &mut rescue_handle)
                    .await;
            match rescue_run {
                Ok(join_result) => match join_result {
                    Ok(Ok(solution_set)) => {
                        restarts_used = restarts_used.saturating_add(1);
                        if let Some(candidate) = pick_best_candidate(
                            solution_set,
                            &req.params.objective,
                            &mut selection_counters,
                            &mut pool_stats,
                        ) {
                            rescue_used = true;
                            rescue_budget_ms = Some(rescue_budget);
                            if best_found_at_restart.is_none() {
                                best_found_at_restart = Some(planned_restarts as u32 + 1);
                            }
                            let candidate_selection = Some(build_candidate_selection_telemetry(
                                &selection_counters,
                                &pool_stats,
                                &candidate,
                            ));
                            return Ok(RunOutcome {
                                candidate,
                                restarts_used: restarts_used.max(1),
                                timeout_reason: Some("slice_timeout".to_string()),
                                restart_policy: restart_plan.map(|plan| RestartPolicyTelemetry {
                                    profile: plan.profile,
                                    min_slice_ms: MIN_SLICE_MS,
                                    min_effective_slice_ms: plan.min_effective_slice_ms,
                                    restarts_cap_by_effective_slice: plan.cap_by_effective_slice,
                                    restarts_effective: plan.effective_restarts,
                                    baseline_budget_ms: plan.baseline_budget_ms,
                                    progressive_slicing: plan.progressive_slicing,
                                    planned_slices_ms: plan.schedule_ms.clone(),
                                    timeouts_per_restart,
                                    first_timeout_at_restart,
                                    best_found_at_restart,
                                    rescue_used,
                                    rescue_budget_ms,
                                }),
                                portfolio: None,
                                beam: None,
                                alns: None,
                                candidate_selection,
                            });
                        }
                        last_constraint = Some(OptimizeError::Constraint {
                            message: "no valid solution".to_string(),
                            details: None,
                        });
                    }
                    Ok(Err(err)) => {
                        last_constraint = Some(map_optimizer_error(err, prepared));
                    }
                    Err(err) => {
                        return Err(OptimizeError::Internal(format!(
                            "optimizer task failed in timeout rescue: {err}"
                        )));
                    }
                },
                Err(_) => {
                    rescue_handle.abort();
                }
            }
        }
    }

    if timed_out {
        return Err(OptimizeError::Timeout);
    }

    if budget_exhausted {
        return Err(OptimizeError::Timeout);
    }

    if let Some(err) = last_constraint {
        return Err(err);
    }

    Err(OptimizeError::Internal("no solution produced".to_string()))
}

async fn run_portfolio_anytime(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    layout_mode: LayoutMode,
    base_seed: u64,
    deadline_ms: u64,
    candidate_count: u32,
    default_restarts: u32,
) -> Result<RunOutcome, OptimizeError> {
    let plans = build_portfolio_plans(
        base_seed,
        default_restarts,
        candidate_count,
        layout_mode,
        req.params.placement_heuristic,
    );
    let candidates_total = plans.len() as u32;
    let mut candidates_started: u32 = 0;
    let mut candidates_completed: u32 = 0;
    let mut candidates_timed_out: u32 = 0;
    let mut candidates_failed: u32 = 0;
    let started_at = Instant::now();

    let mut best: Option<(
        Candidate,
        String,
        u64,
        u32,
        Option<CandidateSelectionTelemetry>,
    )> = None;

    for (idx, plan) in plans.iter().enumerate() {
        let elapsed = started_at.elapsed().as_millis() as u64;
        if elapsed >= deadline_ms {
            break;
        }

        candidates_started += 1;
        let remaining = deadline_ms.saturating_sub(elapsed);
        let slots_left = (plans.len() - idx) as u64;
        let candidate_budget = (remaining / slots_left).max(MIN_SLICE_MS);
        let (plan_restarts, plan_slice_ms) =
            derive_restarts_and_slice(candidate_budget, plan.requested_restarts);

        match run_restarts_with_budget(
            req,
            prepared,
            plan_restarts,
            plan_slice_ms,
            layout_mode,
            plan.seed,
            plan.placement_heuristic,
            candidate_budget,
            false,
            None,
        )
        .await
        {
            Ok(outcome) => {
                candidates_completed += 1;
                let maybe_update = match &best {
                    None => true,
                    Some((current, _, _, _, _)) => {
                        is_better(&outcome.candidate, current, &req.params.objective)
                    }
                };
                if maybe_update {
                    best = Some((
                        outcome.candidate,
                        plan.name.clone(),
                        plan.seed,
                        outcome.restarts_used,
                        outcome.candidate_selection,
                    ));
                }
            }
            Err(OptimizeError::Timeout) => {
                candidates_timed_out += 1;
            }
            Err(OptimizeError::Constraint { .. }) | Err(OptimizeError::Internal(_)) => {
                candidates_failed += 1;
            }
        }
    }

    let candidates_skipped = candidates_total.saturating_sub(candidates_started);

    if let Some((
        candidate,
        winner_strategy,
        winner_seed,
        winner_restarts_used,
        candidate_selection,
    )) = best
    {
        let timeout_reason = if candidates_timed_out > 0 {
            Some("slice_timeout".to_string())
        } else if candidates_skipped > 0 {
            Some("time_budget_exhausted".to_string())
        } else {
            None
        };
        return Ok(RunOutcome {
            candidate,
            restarts_used: winner_restarts_used,
            timeout_reason,
            restart_policy: None,
            portfolio: Some(PortfolioTelemetry {
                deadline_ms,
                candidates_total,
                candidates_completed,
                candidates_timed_out,
                candidates_failed,
                candidates_skipped,
                winner_strategy,
                winner_seed,
                winner_restarts_used,
            }),
            beam: None,
            alns: None,
            candidate_selection,
        });
    }

    if candidates_timed_out > 0 || candidates_skipped > 0 {
        return Err(OptimizeError::Timeout);
    }

    Err(OptimizeError::Internal(
        "portfolio could not produce a valid solution".to_string(),
    ))
}

async fn run_beam_anytime(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    layout_mode: LayoutMode,
    base_seed: u64,
    deadline_ms: u64,
    beam_width: u32,
    beam_depth: u32,
    branch_factor: u32,
    default_restarts: u32,
) -> Result<RunOutcome, OptimizeError> {
    let mut frontier_plans = vec![BeamPlan {
        seed: base_seed,
        requested_restarts: default_restarts.max(1),
    }];
    let mut frontier_results: Vec<BeamCandidate> = Vec::new();
    let started_at = Instant::now();
    let mut nodes_evaluated: u32 = 0;
    let mut nodes_timed_out: u32 = 0;
    let mut nodes_failed: u32 = 0;
    let mut nodes_pruned: u32 = 0;
    let mut budget_exhausted = false;

    for depth in 0..beam_depth {
        let elapsed = started_at.elapsed().as_millis() as u64;
        if elapsed >= deadline_ms {
            budget_exhausted = true;
            break;
        }
        if frontier_plans.is_empty() {
            break;
        }

        let remaining_ms = deadline_ms.saturating_sub(elapsed);
        let remaining_levels = u64::from(beam_depth.saturating_sub(depth).max(1));
        let level_budget = (remaining_ms / remaining_levels).max(MIN_SLICE_MS);
        let planned_runs =
            (frontier_plans.len() as u64).saturating_mul(u64::from(branch_factor).max(1));
        let run_budget_ms = (level_budget / planned_runs.max(1)).max(MIN_SLICE_MS);

        let mut level_results: Vec<BeamCandidate> = Vec::new();
        for parent in &frontier_plans {
            let expansions =
                build_beam_expansions(parent.seed, parent.requested_restarts, branch_factor, depth);
            for expansion in expansions {
                let now_elapsed = started_at.elapsed().as_millis() as u64;
                if now_elapsed >= deadline_ms {
                    budget_exhausted = true;
                    break;
                }

                nodes_evaluated = nodes_evaluated.saturating_add(1);
                let (run_restarts, run_slice_ms) =
                    derive_restarts_and_slice(run_budget_ms, expansion.requested_restarts);

                match run_restarts_with_budget(
                    req,
                    prepared,
                    run_restarts,
                    run_slice_ms,
                    layout_mode,
                    expansion.seed,
                    req.params.placement_heuristic,
                    run_budget_ms,
                    false,
                    None,
                )
                .await
                {
                    Ok(outcome) => {
                        if outcome.timeout_reason.is_some() {
                            nodes_timed_out = nodes_timed_out.saturating_add(1);
                        }
                        level_results.push(BeamCandidate {
                            candidate: outcome.candidate,
                            seed: expansion.seed,
                            requested_restarts: expansion.requested_restarts,
                            depth,
                            restarts_used: outcome.restarts_used,
                            candidate_selection: outcome.candidate_selection,
                        });
                    }
                    Err(OptimizeError::Timeout) => {
                        nodes_timed_out = nodes_timed_out.saturating_add(1);
                    }
                    Err(OptimizeError::Constraint { .. }) | Err(OptimizeError::Internal(_)) => {
                        nodes_failed = nodes_failed.saturating_add(1);
                    }
                }
            }
            if budget_exhausted {
                break;
            }
        }

        if level_results.is_empty() {
            continue;
        }

        level_results.sort_by(|a, b| {
            if is_better(&a.candidate, &b.candidate, &req.params.objective) {
                std::cmp::Ordering::Less
            } else if is_better(&b.candidate, &a.candidate, &req.params.objective) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });

        let total_level = level_results.len();
        let keep = usize::try_from(beam_width).unwrap_or(usize::MAX);
        let kept: Vec<BeamCandidate> = level_results.into_iter().take(keep).collect();
        let kept_len = kept.len();
        let level_pruned = total_level.saturating_sub(kept_len);
        nodes_pruned = nodes_pruned.saturating_add(u32::try_from(level_pruned).unwrap_or(0));
        frontier_plans = kept
            .iter()
            .map(|entry| BeamPlan {
                seed: entry.seed,
                requested_restarts: entry.requested_restarts,
            })
            .collect();
        frontier_results = kept;
    }

    if !frontier_results.is_empty() {
        frontier_results.sort_by(|a, b| {
            if is_better(&a.candidate, &b.candidate, &req.params.objective) {
                std::cmp::Ordering::Less
            } else if is_better(&b.candidate, &a.candidate, &req.params.objective) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        let best = frontier_results.remove(0);
        let timeout_reason = if nodes_timed_out > 0 {
            Some("slice_timeout".to_string())
        } else if budget_exhausted {
            Some("time_budget_exhausted".to_string())
        } else {
            None
        };
        return Ok(RunOutcome {
            candidate: best.candidate,
            restarts_used: best.restarts_used,
            timeout_reason,
            restart_policy: None,
            portfolio: None,
            beam: Some(BeamTelemetry {
                deadline_ms,
                beam_width,
                beam_depth,
                branch_factor,
                nodes_evaluated,
                nodes_timed_out,
                nodes_failed,
                nodes_pruned,
                winner_depth: best.depth,
                winner_seed: best.seed,
                winner_restarts_used: best.restarts_used,
            }),
            alns: None,
            candidate_selection: best.candidate_selection,
        });
    }

    if nodes_timed_out > 0 || budget_exhausted {
        return Err(OptimizeError::Timeout);
    }

    Err(OptimizeError::Internal(
        "beam search could not produce a valid solution".to_string(),
    ))
}

async fn run_alns_anytime(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    layout_mode: LayoutMode,
    base_seed: u64,
    deadline_ms: u64,
    iterations: u32,
    segment_size: u32,
    temperature_start: f64,
    temperature_end: f64,
    reaction_factor: f64,
    default_restarts: u32,
) -> Result<RunOutcome, OptimizeError> {
    let mut operators = vec![
        AlnsOperatorState {
            name: "destroy_small_repair_dense",
            weight: 1.0,
            score: 0.0,
            selected: 0,
            accepted: 0,
            improved_best: 0,
        },
        AlnsOperatorState {
            name: "destroy_large_repair_dense",
            weight: 1.0,
            score: 0.0,
            selected: 0,
            accepted: 0,
            improved_best: 0,
        },
        AlnsOperatorState {
            name: "destroy_small_repair_sparse",
            weight: 1.0,
            score: 0.0,
            selected: 0,
            accepted: 0,
            improved_best: 0,
        },
        AlnsOperatorState {
            name: "destroy_large_repair_sparse",
            weight: 1.0,
            score: 0.0,
            selected: 0,
            accepted: 0,
            improved_best: 0,
        },
        AlnsOperatorState {
            name: "destroy_focus_repair_boost",
            weight: 1.0,
            score: 0.0,
            selected: 0,
            accepted: 0,
            improved_best: 0,
        },
        AlnsOperatorState {
            name: "destroy_focus_repair_trim",
            weight: 1.0,
            score: 0.0,
            selected: 0,
            accepted: 0,
            improved_best: 0,
        },
    ];

    let started_at = Instant::now();
    let mut rng_state = base_seed ^ 0x9E37_79B9_7F4A_7C15;
    let base_plan = AlnsPlan {
        seed: base_seed,
        requested_restarts: default_restarts.max(1),
    };
    let mut incumbent: Option<(f64, AlnsPlan, u32)> = None;
    let mut best: Option<(Candidate, u64, u32, Option<CandidateSelectionTelemetry>)> = None;

    let mut candidates_evaluated: u32 = 0;
    let mut candidates_timed_out: u32 = 0;
    let mut candidates_failed: u32 = 0;
    let mut accepted_worse: u32 = 0;
    let mut improved_best: u32 = 0;
    let mut iterations_completed: u32 = 0;
    let mut budget_exhausted = false;
    let mut timeout_streak: u32 = 0;

    // Reserve a substantial first attempt to avoid cascades of tiny timed-out slices.
    let min_baseline = ALNS_MIN_BASELINE_BUDGET_MS
        .min(deadline_ms)
        .max(MIN_SLICE_MS);
    let bootstrap_budget = ((deadline_ms.saturating_mul(2)) / 3)
        .max(min_baseline)
        .min(deadline_ms.max(MIN_SLICE_MS));
    let (boot_restarts, boot_slice) =
        derive_restarts_and_slice(bootstrap_budget, base_plan.requested_restarts);
    match run_restarts_with_budget(
        req,
        prepared,
        boot_restarts,
        boot_slice,
        layout_mode,
        base_plan.seed,
        req.params.placement_heuristic,
        bootstrap_budget,
        false,
        None,
    )
    .await
    {
        Ok(outcome) => {
            candidates_evaluated = candidates_evaluated.saturating_add(1);
            if outcome.timeout_reason.is_some() {
                candidates_timed_out = candidates_timed_out.saturating_add(1);
            }
            let candidate = outcome.candidate;
            let score = objective_score(&candidate, &req.params.objective);
            incumbent = Some((score, base_plan.clone(), outcome.restarts_used.max(1)));
            best = Some((
                candidate,
                base_plan.seed,
                outcome.restarts_used.max(1),
                outcome.candidate_selection,
            ));
            timeout_streak = 0;
        }
        Err(OptimizeError::Timeout) => {
            candidates_timed_out = candidates_timed_out.saturating_add(1);
            timeout_streak = timeout_streak.saturating_add(1);
        }
        Err(OptimizeError::Constraint { .. }) | Err(OptimizeError::Internal(_)) => {
            candidates_failed = candidates_failed.saturating_add(1);
            timeout_streak = 0;
        }
    }

    for iter in 0..iterations {
        let elapsed = started_at.elapsed().as_millis() as u64;
        if elapsed >= deadline_ms {
            budget_exhausted = true;
            break;
        }
        let remaining_ms = deadline_ms.saturating_sub(elapsed);
        if remaining_ms < ALNS_MIN_ITER_BUDGET_MS {
            break;
        }
        let remaining_iters = u64::from(iterations.saturating_sub(iter).max(1));
        let max_iters_by_budget = (remaining_ms / ALNS_MIN_ITER_BUDGET_MS).max(1);
        let planned_iters = remaining_iters.min(max_iters_by_budget).max(1);
        let iter_budget_ms = (remaining_ms / planned_iters)
            .max(ALNS_MIN_ITER_BUDGET_MS)
            .min(remaining_ms);

        let op_idx = choose_operator(&operators, &mut rng_state);
        let operator = &mut operators[op_idx];
        operator.selected = operator.selected.saturating_add(1);

        let source_plan = incumbent
            .as_ref()
            .map(|(_, plan, _)| plan)
            .unwrap_or(&base_plan);
        let proposal = mutate_alns_plan(source_plan, op_idx, iter);
        let iter_restarts = proposal
            .requested_restarts
            .min(ALNS_MAX_ITER_RESTARTS)
            .max(1);
        let (run_restarts, run_slice) = derive_restarts_and_slice(iter_budget_ms, iter_restarts);

        match run_restarts_with_budget(
            req,
            prepared,
            run_restarts,
            run_slice,
            layout_mode,
            proposal.seed,
            req.params.placement_heuristic,
            iter_budget_ms,
            false,
            None,
        )
        .await
        {
            Ok(outcome) => {
                candidates_evaluated = candidates_evaluated.saturating_add(1);
                if outcome.timeout_reason.is_some() {
                    candidates_timed_out = candidates_timed_out.saturating_add(1);
                }
                iterations_completed = iterations_completed.saturating_add(1);
                let candidate = outcome.candidate;
                let candidate_score = objective_score(&candidate, &req.params.objective);
                timeout_streak = 0;

                let mut reward = 0.2_f64;
                let mut accepted = false;

                if let Some((inc_score, _, _)) = incumbent.as_ref() {
                    if candidate_score <= *inc_score {
                        accepted = true;
                        reward = 3.0;
                    } else {
                        let temp = annealing_temperature(
                            iter,
                            iterations.max(1),
                            temperature_start,
                            temperature_end,
                        );
                        let delta = (candidate_score - *inc_score).max(0.0);
                        let scale = (inc_score.abs().max(1.0)) * 0.001;
                        let prob = (-delta / (scale * temp.max(1e-6))).exp().clamp(0.0, 1.0);
                        if rand01(&mut rng_state) < prob {
                            accepted = true;
                            accepted_worse = accepted_worse.saturating_add(1);
                            reward = 1.0;
                        }
                    }
                } else {
                    accepted = true;
                    reward = 3.0;
                }

                let is_global_best = match best.as_ref() {
                    None => true,
                    Some((best_candidate, _, _, _)) => {
                        is_better(&candidate, best_candidate, &req.params.objective)
                    }
                };
                let mut moved_to_best = false;
                if is_global_best {
                    accepted = true;
                    best = Some((
                        candidate,
                        proposal.seed,
                        outcome.restarts_used.max(1),
                        outcome.candidate_selection,
                    ));
                    moved_to_best = true;
                    improved_best = improved_best.saturating_add(1);
                    operator.improved_best = operator.improved_best.saturating_add(1);
                    reward = reward.max(5.0);
                }

                if accepted {
                    operator.accepted = operator.accepted.saturating_add(1);
                    let new_score = if moved_to_best {
                        best.as_ref()
                            .map(|(c, _, _, _)| objective_score(c, &req.params.objective))
                            .unwrap_or(candidate_score)
                    } else {
                        candidate_score
                    };
                    incumbent = Some((new_score, proposal, outcome.restarts_used.max(1)));
                }

                operator.score += reward;
            }
            Err(OptimizeError::Timeout) => {
                candidates_timed_out = candidates_timed_out.saturating_add(1);
                iterations_completed = iterations_completed.saturating_add(1);
                timeout_streak = timeout_streak.saturating_add(1);
            }
            Err(OptimizeError::Constraint { .. }) | Err(OptimizeError::Internal(_)) => {
                candidates_failed = candidates_failed.saturating_add(1);
                iterations_completed = iterations_completed.saturating_add(1);
                timeout_streak = 0;
            }
        }

        if (iter + 1) % segment_size == 0 {
            update_operator_weights(&mut operators, reaction_factor);
        }

        // Timed-out blocking slices may continue in background; limit queue buildup.
        if timeout_streak >= ALNS_MAX_TIMEOUT_STREAK {
            break;
        }
    }

    if let Some((winner, winner_seed, winner_restarts_used, candidate_selection)) = best {
        let timeout_reason = if candidates_timed_out > 0 {
            Some("slice_timeout".to_string())
        } else if budget_exhausted {
            Some("time_budget_exhausted".to_string())
        } else {
            None
        };
        return Ok(RunOutcome {
            candidate: winner,
            restarts_used: winner_restarts_used.max(1),
            timeout_reason,
            restart_policy: None,
            portfolio: None,
            beam: None,
            alns: Some(AlnsTelemetry {
                deadline_ms,
                iterations_requested: iterations,
                iterations_completed,
                segment_size,
                temperature_start,
                temperature_end,
                reaction_factor,
                candidates_evaluated,
                candidates_timed_out,
                candidates_failed,
                accepted_worse,
                improved_best,
                winner_seed,
                winner_restarts_used,
                operators: operators
                    .iter()
                    .map(|op| AlnsOperatorTelemetry {
                        name: op.name.to_string(),
                        weight: op.weight,
                        selected: op.selected,
                        accepted: op.accepted,
                        improved_best: op.improved_best,
                    })
                    .collect(),
            }),
            candidate_selection,
        });
    }

    let fallback_seed = base_seed;
    let (fallback_restarts, fallback_slice) = derive_restarts_and_slice(deadline_ms, 1);
    match run_restarts_with_budget(
        req,
        prepared,
        fallback_restarts,
        fallback_slice,
        layout_mode,
        fallback_seed,
        req.params.placement_heuristic,
        deadline_ms,
        false,
        None,
    )
    .await
    {
        Ok(fallback) => {
            candidates_evaluated = candidates_evaluated.saturating_add(1);
            if fallback.timeout_reason.is_some() {
                candidates_timed_out = candidates_timed_out.saturating_add(1);
            }
            let timeout_reason = if candidates_timed_out > 0 {
                Some("slice_timeout".to_string())
            } else if budget_exhausted {
                Some("time_budget_exhausted".to_string())
            } else {
                None
            };
            return Ok(RunOutcome {
                candidate: fallback.candidate,
                restarts_used: fallback.restarts_used.max(1),
                timeout_reason,
                restart_policy: None,
                portfolio: None,
                beam: None,
                alns: Some(AlnsTelemetry {
                    deadline_ms,
                    iterations_requested: iterations,
                    iterations_completed,
                    segment_size,
                    temperature_start,
                    temperature_end,
                    reaction_factor,
                    candidates_evaluated,
                    candidates_timed_out,
                    candidates_failed,
                    accepted_worse,
                    improved_best,
                    winner_seed: fallback_seed,
                    winner_restarts_used: fallback.restarts_used.max(1),
                    operators: operators
                        .iter()
                        .map(|op| AlnsOperatorTelemetry {
                            name: op.name.to_string(),
                            weight: op.weight,
                            selected: op.selected,
                            accepted: op.accepted,
                            improved_best: op.improved_best,
                        })
                        .collect(),
                }),
                candidate_selection: fallback.candidate_selection,
            });
        }
        Err(OptimizeError::Timeout) => {
            candidates_timed_out = candidates_timed_out.saturating_add(1);
        }
        Err(OptimizeError::Constraint { .. }) | Err(OptimizeError::Internal(_)) => {
            candidates_failed = candidates_failed.saturating_add(1);
        }
    }

    if candidates_timed_out > 0 || budget_exhausted {
        return Err(OptimizeError::Timeout);
    }

    Err(OptimizeError::Internal(format!(
        "alns/lns could not produce a valid solution (failed_candidates={})",
        candidates_failed
    )))
}

fn choose_operator(operators: &[AlnsOperatorState], rng_state: &mut u64) -> usize {
    let total: f64 = operators.iter().map(|op| op.weight.max(0.0001)).sum();
    let mut r = rand01(rng_state) * total.max(0.0001);
    for (idx, op) in operators.iter().enumerate() {
        let w = op.weight.max(0.0001);
        if r <= w {
            return idx;
        }
        r -= w;
    }
    operators.len().saturating_sub(1)
}

fn mutate_alns_plan(base: &AlnsPlan, op_idx: usize, iter: u32) -> AlnsPlan {
    let step = u64::from(iter.saturating_add(1));
    let dense = base
        .requested_restarts
        .saturating_mul(2)
        .clamp(1, ALNS_MAX_RESTARTS);
    let sparse = (base.requested_restarts / 2).max(1);
    let boost = base
        .requested_restarts
        .saturating_add(2)
        .clamp(1, ALNS_MAX_RESTARTS);
    let trim = base
        .requested_restarts
        .saturating_sub(1)
        .max(1)
        .clamp(1, ALNS_MAX_RESTARTS);

    match op_idx % 6 {
        0 => AlnsPlan {
            seed: base
                .seed
                .wrapping_add(step.wrapping_mul(3).wrapping_mul(SEED_STRIDE)),
            requested_restarts: dense,
        },
        1 => AlnsPlan {
            seed: base
                .seed
                .wrapping_add(step.wrapping_mul(37).wrapping_mul(SEED_STRIDE)),
            requested_restarts: dense,
        },
        2 => AlnsPlan {
            seed: base
                .seed
                .wrapping_add(step.wrapping_mul(5).wrapping_mul(SEED_STRIDE)),
            requested_restarts: sparse,
        },
        3 => AlnsPlan {
            seed: base
                .seed
                .wrapping_add(step.wrapping_mul(41).wrapping_mul(SEED_STRIDE)),
            requested_restarts: sparse,
        },
        4 => AlnsPlan {
            seed: base.seed ^ step.wrapping_mul(13).wrapping_mul(SEED_STRIDE),
            requested_restarts: boost,
        },
        _ => AlnsPlan {
            seed: base.seed ^ step.wrapping_mul(19).wrapping_mul(SEED_STRIDE),
            requested_restarts: trim,
        },
    }
}

fn update_operator_weights(operators: &mut [AlnsOperatorState], reaction_factor: f64) {
    for op in operators {
        if op.selected > 0 {
            let avg_score = op.score / f64::from(op.selected);
            op.weight = ((1.0 - reaction_factor) * op.weight + reaction_factor * avg_score)
                .clamp(0.05, 20.0);
        }
    }
}

fn annealing_temperature(iter: u32, total_iters: u32, start: f64, end: f64) -> f64 {
    if total_iters <= 1 {
        return end.min(start).max(1e-6);
    }
    let alpha = f64::from(iter) / f64::from(total_iters.saturating_sub(1));
    (start + (end - start) * alpha).max(1e-6)
}

fn objective_score(candidate: &Candidate, objective: &Objective) -> f64 {
    match objective {
        Objective::MinSheets => {
            f64::from(candidate.used_stock_count) * 1_000_000_000.0
                + candidate.total_waste_area_units as f64
        }
        Objective::MinWaste => {
            candidate.total_waste_area_units as f64 * 1000.0 + f64::from(candidate.used_stock_count)
        }
    }
}

fn rand01(state: &mut u64) -> f64 {
    let x = lcg_next(state) >> 11;
    (x as f64) / ((1u64 << 53) as f64)
}

fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn build_beam_expansions(
    seed: u64,
    requested_restarts: u32,
    branch_factor: u32,
    depth: u32,
) -> Vec<BeamPlan> {
    let mut plans = Vec::with_capacity(branch_factor as usize);
    for branch in 0..branch_factor {
        let offset = u64::from(depth.saturating_add(1))
            .wrapping_mul(SEED_STRIDE)
            .wrapping_mul(u64::from(branch.saturating_add(1)));
        let branch_seed = seed.wrapping_add(offset);
        let restarts = match branch % 4 {
            0 => requested_restarts.max(1),
            1 => (requested_restarts / 2).max(1),
            2 => requested_restarts
                .saturating_add(1)
                .clamp(1, BEAM_MAX_RESTARTS),
            _ => requested_restarts
                .saturating_mul(2)
                .clamp(1, BEAM_MAX_RESTARTS),
        };
        plans.push(BeamPlan {
            seed: branch_seed,
            requested_restarts: restarts,
        });
    }
    plans
}

const PORTFOLIO_GUILLOTINE_HEURISTICS: &[PlacementHeuristic] = &[
    PlacementHeuristic::BestArea,
    PlacementHeuristic::BestShortSide,
    PlacementHeuristic::BestLongSide,
    PlacementHeuristic::SmallestY,
];

const PORTFOLIO_NESTED_HEURISTICS: &[PlacementHeuristic] = &[
    PlacementHeuristic::BestShortSide,
    PlacementHeuristic::BestLongSide,
    PlacementHeuristic::BestArea,
    PlacementHeuristic::BottomLeft,
    PlacementHeuristic::ContactPoint,
];

fn placement_heuristic_label(heuristic: PlacementHeuristic) -> &'static str {
    match heuristic {
        PlacementHeuristic::BestArea => "best_area",
        PlacementHeuristic::BestShortSide => "best_short_side",
        PlacementHeuristic::BestLongSide => "best_long_side",
        PlacementHeuristic::WorstArea => "worst_area",
        PlacementHeuristic::WorstShortSide => "worst_short_side",
        PlacementHeuristic::WorstLongSide => "worst_long_side",
        PlacementHeuristic::SmallestY => "smallest_y",
        PlacementHeuristic::BottomLeft => "bottom_left",
        PlacementHeuristic::ContactPoint => "contact_point",
    }
}

fn default_portfolio_heuristics(layout_mode: LayoutMode) -> &'static [PlacementHeuristic] {
    match layout_mode {
        LayoutMode::Guillotine => PORTFOLIO_GUILLOTINE_HEURISTICS,
        LayoutMode::Nested => PORTFOLIO_NESTED_HEURISTICS,
    }
}

fn to_guillotine_preset(
    heuristic: PlacementHeuristic,
) -> Option<cut_optimizer_2d::GuillotineHeuristicPreset> {
    match heuristic {
        PlacementHeuristic::BestArea => {
            Some(cut_optimizer_2d::GuillotineHeuristicPreset::BestAreaFit)
        }
        PlacementHeuristic::BestShortSide => {
            Some(cut_optimizer_2d::GuillotineHeuristicPreset::BestShortSideFit)
        }
        PlacementHeuristic::BestLongSide => {
            Some(cut_optimizer_2d::GuillotineHeuristicPreset::BestLongSideFit)
        }
        PlacementHeuristic::WorstArea => {
            Some(cut_optimizer_2d::GuillotineHeuristicPreset::WorstAreaFit)
        }
        PlacementHeuristic::WorstShortSide => {
            Some(cut_optimizer_2d::GuillotineHeuristicPreset::WorstShortSideFit)
        }
        PlacementHeuristic::WorstLongSide => {
            Some(cut_optimizer_2d::GuillotineHeuristicPreset::WorstLongSideFit)
        }
        PlacementHeuristic::SmallestY => {
            Some(cut_optimizer_2d::GuillotineHeuristicPreset::SmallestY)
        }
        PlacementHeuristic::BottomLeft | PlacementHeuristic::ContactPoint => None,
    }
}

fn to_nested_preset(
    heuristic: PlacementHeuristic,
) -> Option<cut_optimizer_2d::MaxRectsHeuristicPreset> {
    match heuristic {
        PlacementHeuristic::BestArea => {
            Some(cut_optimizer_2d::MaxRectsHeuristicPreset::BestAreaFit)
        }
        PlacementHeuristic::BestShortSide => {
            Some(cut_optimizer_2d::MaxRectsHeuristicPreset::BestShortSideFit)
        }
        PlacementHeuristic::BestLongSide => {
            Some(cut_optimizer_2d::MaxRectsHeuristicPreset::BestLongSideFit)
        }
        PlacementHeuristic::BottomLeft => {
            Some(cut_optimizer_2d::MaxRectsHeuristicPreset::BottomLeftRule)
        }
        PlacementHeuristic::ContactPoint => {
            Some(cut_optimizer_2d::MaxRectsHeuristicPreset::ContactPointRule)
        }
        PlacementHeuristic::WorstArea
        | PlacementHeuristic::WorstShortSide
        | PlacementHeuristic::WorstLongSide
        | PlacementHeuristic::SmallestY => None,
    }
}

fn build_portfolio_plans(
    base_seed: u64,
    requested_restarts: u32,
    candidate_count: u32,
    layout_mode: LayoutMode,
    placement_heuristic: Option<PlacementHeuristic>,
) -> Vec<PortfolioPlan> {
    let mut plans = Vec::with_capacity(candidate_count as usize);
    let heuristics = placement_heuristic
        .map(|h| vec![h])
        .unwrap_or_else(|| default_portfolio_heuristics(layout_mode).to_vec());
    let heuristics = if heuristics.is_empty() {
        default_portfolio_heuristics(layout_mode).to_vec()
    } else {
        heuristics
    };

    for idx in 0..candidate_count {
        let (base_name, restarts) = match idx % 4 {
            0 => ("baseline", requested_restarts.max(1)),
            1 => ("seed_explore_fast", (requested_restarts / 2).max(1)),
            2 => ("seed_explore_full", requested_restarts.max(1)),
            _ => (
                "restart_explore_dense",
                requested_restarts.saturating_mul(2).clamp(1, 64),
            ),
        };
        let seed = base_seed.wrapping_add(
            (idx as u64)
                .wrapping_add(1)
                .wrapping_mul(17)
                .wrapping_mul(SEED_STRIDE),
        );
        let heuristic = heuristics[idx as usize % heuristics.len()];
        let mut name = base_name.to_string();
        name.push('/');
        name.push_str(placement_heuristic_label(heuristic));
        plans.push(PortfolioPlan {
            name,
            seed,
            requested_restarts: restarts,
            placement_heuristic: Some(heuristic),
        });
    }
    plans
}

fn derive_restarts_and_slice(total_budget_ms: u64, requested_restarts: u32) -> (u64, u64) {
    let mut restarts = u64::from(requested_restarts.max(1));
    let mut slice_ms = total_budget_ms / restarts;
    if slice_ms < MIN_SLICE_MS {
        restarts = (total_budget_ms / MIN_SLICE_MS).max(1);
        slice_ms = total_budget_ms / restarts;
    }
    (restarts.max(1), slice_ms.max(1))
}

fn derive_standard_restart_plan(
    total_budget_ms: u64,
    requested_restarts: u32,
    profile: SlaProfile,
) -> RestartPlan {
    let requested = u64::from(requested_restarts.max(1));
    let (min_effective_slice_ms, baseline_share_percent) = match profile {
        SlaProfile::Fast => (
            STANDARD_FAST_MIN_EFFECTIVE_SLICE_MS,
            STANDARD_FAST_BASELINE_SHARE_PERCENT,
        ),
        SlaProfile::Balanced => (
            STANDARD_BALANCED_MIN_EFFECTIVE_SLICE_MS,
            STANDARD_BALANCED_BASELINE_SHARE_PERCENT,
        ),
        SlaProfile::Quality => (
            STANDARD_QUALITY_MIN_EFFECTIVE_SLICE_MS,
            STANDARD_QUALITY_BASELINE_SHARE_PERCENT,
        ),
    };
    let min_effective_slice_ms = min_effective_slice_ms.max(MIN_SLICE_MS);
    let reserve_ms = min_effective_slice_ms / 2;
    let cap_by_effective_slice =
        total_budget_ms.saturating_sub(reserve_ms) / min_effective_slice_ms;
    let effective_restarts = requested.min(cap_by_effective_slice.max(1)).max(1);
    let progressive_slicing = !matches!(profile, SlaProfile::Fast);
    let schedule_ms = build_restart_slice_schedule(
        total_budget_ms.max(1),
        effective_restarts,
        min_effective_slice_ms,
        baseline_share_percent,
        progressive_slicing,
    );
    let baseline_budget_ms = schedule_ms
        .first()
        .copied()
        .unwrap_or(total_budget_ms.max(1));
    let fallback_slice_ms = if schedule_ms.len() <= 1 {
        total_budget_ms.max(1)
    } else {
        let tail_sum: u64 = schedule_ms.iter().skip(1).sum();
        (tail_sum / (schedule_ms.len() as u64 - 1)).max(1)
    };
    RestartPlan {
        profile,
        min_effective_slice_ms,
        cap_by_effective_slice: cap_by_effective_slice.max(1),
        baseline_budget_ms,
        progressive_slicing,
        effective_restarts,
        fallback_slice_ms,
        schedule_ms,
    }
}

fn build_restart_slice_schedule(
    total_budget_ms: u64,
    restarts: u64,
    min_effective_slice_ms: u64,
    baseline_share_percent: u64,
    progressive: bool,
) -> Vec<u64> {
    let total_budget_ms = total_budget_ms.max(1);
    let restarts = restarts.max(1);
    if restarts == 1 {
        return vec![total_budget_ms];
    }

    let tail_count = restarts - 1;
    let min_slice_ms = MIN_SLICE_MS.max(1);
    let min_tail_total = min_slice_ms.saturating_mul(tail_count);
    let baseline_target = ((total_budget_ms.saturating_mul(baseline_share_percent)) / 100)
        .max(min_effective_slice_ms)
        .max(min_slice_ms);
    let baseline_cap = total_budget_ms
        .saturating_sub(min_tail_total)
        .max(min_slice_ms);
    let baseline_ms = baseline_target.min(baseline_cap).min(total_budget_ms);
    let tail_budget = total_budget_ms.saturating_sub(baseline_ms);

    let mut tail = vec![min_slice_ms; tail_count as usize];
    let tail_floor = min_slice_ms.saturating_mul(tail_count);
    let extra = tail_budget.saturating_sub(tail_floor);
    if extra > 0 {
        if progressive {
            let mut distributed: u64 = 0;
            let weight_sum = tail_count.saturating_mul(tail_count + 1) / 2;
            for (idx, slot) in tail.iter_mut().enumerate() {
                let weight = (tail_count as usize - idx) as u64;
                let add = extra.saturating_mul(weight) / weight_sum.max(1);
                *slot = slot.saturating_add(add);
                distributed = distributed.saturating_add(add);
            }
            let mut rem = extra.saturating_sub(distributed);
            let mut idx = 0usize;
            while rem > 0 && !tail.is_empty() {
                tail[idx] = tail[idx].saturating_add(1);
                rem -= 1;
                idx = (idx + 1) % tail.len();
            }
        } else {
            let even = extra / tail_count.max(1);
            let mut rem = extra % tail_count.max(1);
            for slot in &mut tail {
                *slot = slot.saturating_add(even);
                if rem > 0 {
                    *slot = slot.saturating_add(1);
                    rem -= 1;
                }
            }
        }
    }

    let mut schedule = Vec::with_capacity(restarts as usize);
    schedule.push(baseline_ms);
    schedule.extend(tail);

    let scheduled_total: u64 = schedule.iter().sum();
    if scheduled_total < total_budget_ms {
        let diff = total_budget_ms - scheduled_total;
        schedule[0] = schedule[0].saturating_add(diff);
    } else if scheduled_total > total_budget_ms {
        let mut diff = scheduled_total - total_budget_ms;
        for idx in (0..schedule.len()).rev() {
            if diff == 0 {
                break;
            }
            let floor = if idx == 0 { min_slice_ms } else { min_slice_ms };
            let reducible = schedule[idx].saturating_sub(floor);
            if reducible == 0 {
                continue;
            }
            let take = reducible.min(diff);
            schedule[idx] -= take;
            diff -= take;
        }
    }
    schedule
}

fn generate_seed() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis() as u64
}

fn prepare_input(req: &OptimizeRequest) -> Result<PreparedInput, OptimizeError> {
    let trim_left = to_units(req.params.trim_mm.left)?;
    let trim_right = to_units(req.params.trim_mm.right)?;
    let trim_top = to_units(req.params.trim_mm.top)?;
    let trim_bottom = to_units(req.params.trim_mm.bottom)?;

    let cut_width = to_units(req.params.kerf_mm + req.params.spacing_mm)?;

    let mut stock_pieces = Vec::new();
    let mut stock_map: HashMap<(usize, usize), Vec<StockInfo>> = HashMap::new();

    for stock in &req.stock {
        let width = to_units(stock.width_mm)?;
        let height = to_units(stock.height_mm)?;

        let usable_w = width.saturating_sub(trim_left + trim_right);
        let usable_h = height.saturating_sub(trim_top + trim_bottom);
        if usable_w == 0 || usable_h == 0 {
            return Err(OptimizeError::Constraint {
                message: "trim exceeds stock dimensions".to_string(),
                details: Some(serde_json::json!({"stock_id": stock.id})),
            });
        }

        // Save user-specified qty limit (None or 0 = unlimited)
        let qty_limit = match stock.qty {
            Some(q) if q > 0 => Some(q),
            _ => None,
        };

        // Always run optimizer with unlimited sheets - we'll trim later
        stock_pieces.push(StockPiece {
            width: usable_w,
            length: usable_h,
            pattern_direction: CutPatternDirection::None,
            price: 0,
            quantity: None, // Unlimited for optimizer
        });

        stock_map
            .entry((usable_w, usable_h))
            .or_default()
            .push(StockInfo {
                stock_id: stock.id.clone(),
                full_width_mm: stock.width_mm,
                full_height_mm: stock.height_mm,
                qty_limit,
            });
    }

    let mut cut_pieces = Vec::new();
    let mut instance_map = Vec::new();
    let mut oversized_items = Vec::new();

    let gap_mm = req.params.kerf_mm + req.params.spacing_mm;

    for item in &req.items {
        // Check if item fits any stock sheet (considering trim and gap for cutting)
        let fits = item_fits_any_stock_public(item, &req.params.trim_mm, gap_mm, &req.stock);

        if !fits {
            // Item doesn't fit any stock - add all instances to oversized
            for idx in 0..item.qty {
                oversized_items.push(UnplacedItem {
                    item_id: item.id.clone(),
                    instance: idx + 1,
                    width_mm: item.width_mm,
                    height_mm: item.height_mm,
                    reason: "oversized".to_string(),
                });
            }
            continue; // Skip this item for optimization
        }

        let width = to_units(item.width_mm)?;
        let height = to_units(item.height_mm)?;
        let can_rotate = item.rotation == crate::models::Rotation::Allow90
            && item.pattern_direction == PatternDirection::None;

        for idx in 0..item.qty {
            let external_id = instance_map.len();
            instance_map.push(InstanceInfo {
                item_id: item.id.clone(),
                instance: idx + 1,
                pattern_direction: item.pattern_direction,
            });

            cut_pieces.push(CutPiece {
                quantity: 1,
                external_id: Some(external_id),
                width,
                length: height,
                pattern_direction: CutPatternDirection::None,
                can_rotate,
            });
        }
    }

    Ok(PreparedInput {
        stock_pieces,
        cut_pieces,
        instance_map,
        stock_map,
        trim: req.params.trim_mm,
        cut_width,
        oversized_items,
    })
}

fn map_optimizer_error(err: cut_optimizer_2d::Error, prepared: &PreparedInput) -> OptimizeError {
    match err {
        cut_optimizer_2d::Error::NoFitForCutPiece(cut_piece) => {
            let item_info = cut_piece
                .external_id
                .and_then(|id| prepared.instance_map.get(id))
                .map(|info| info.item_id.clone());
            let mut details = serde_json::json!({
                "width_mm": from_units(cut_piece.width),
                "height_mm": from_units(cut_piece.length)
            });
            if let Some(item_id) = item_info {
                details["item_id"] = serde_json::Value::String(item_id);
            }
            OptimizeError::Constraint {
                message: "item does not fit stock".to_string(),
                details: Some(details),
            }
        }
    }
}

fn build_candidate(solution: cut_optimizer_2d::Solution) -> Candidate {
    let mut total_waste_area: u128 = 0;
    let mut total_bbox_area: u128 = 0;
    let mut total_bbox_void_area: u128 = 0;
    let mut total_piece_perimeter: u128 = 0;

    for stock in &solution.stock_pieces {
        let stock_area = area(stock.width, stock.length);
        let mut used_area: u128 = 0;
        let mut min_x: usize = usize::MAX;
        let mut min_y: usize = usize::MAX;
        let mut max_x: usize = 0;
        let mut max_y: usize = 0;
        for cut_piece in &stock.cut_pieces {
            used_area = used_area.saturating_add(area(cut_piece.width, cut_piece.length));
            total_piece_perimeter = total_piece_perimeter.saturating_add(
                (2_u128).saturating_mul((cut_piece.width + cut_piece.length) as u128),
            );
            min_x = min_x.min(cut_piece.x);
            min_y = min_y.min(cut_piece.y);
            max_x = max_x.max(cut_piece.x.saturating_add(cut_piece.width));
            max_y = max_y.max(cut_piece.y.saturating_add(cut_piece.length));
        }
        if !stock.cut_pieces.is_empty() {
            let bbox_w = max_x.saturating_sub(min_x);
            let bbox_h = max_y.saturating_sub(min_y);
            let bbox_area = area(bbox_w, bbox_h);
            total_bbox_area = total_bbox_area.saturating_add(bbox_area);
            total_bbox_void_area =
                total_bbox_void_area.saturating_add(bbox_area.saturating_sub(used_area));
        }
        total_waste_area = total_waste_area.saturating_add(stock_area.saturating_sub(used_area));
    }

    Candidate {
        used_stock_count: solution.stock_pieces.len() as u32,
        total_waste_area_units: total_waste_area,
        total_bbox_area_units: total_bbox_area,
        total_bbox_void_area_units: total_bbox_void_area,
        total_piece_perimeter_units: total_piece_perimeter,
        solution,
    }
}

fn is_better(candidate: &Candidate, best: &Candidate, objective: &Objective) -> bool {
    matches!(
        compare_candidates(candidate, best, objective),
        CandidateCompare::Better
    )
}

fn compare_candidates(
    candidate: &Candidate,
    best: &Candidate,
    objective: &Objective,
) -> CandidateCompare {
    if is_better_stats(
        candidate.used_stock_count,
        candidate.total_waste_area_units,
        best.used_stock_count,
        best.total_waste_area_units,
        objective,
    ) {
        return CandidateCompare::Better;
    }
    if is_better_stats(
        best.used_stock_count,
        best.total_waste_area_units,
        candidate.used_stock_count,
        candidate.total_waste_area_units,
        objective,
    ) {
        return CandidateCompare::WorseByPrimaryObjective;
    }

    // Tie-break for same primary objective: prefer denser occupied bounding region
    // to reduce recurring fragmented void patterns and ragged occupancy contours.
    if candidate.total_bbox_void_area_units != best.total_bbox_void_area_units {
        if candidate.total_bbox_void_area_units < best.total_bbox_void_area_units {
            return CandidateCompare::Better;
        }
        return CandidateCompare::WorseByTieBboxVoid;
    }
    if candidate.total_bbox_area_units != best.total_bbox_area_units {
        if candidate.total_bbox_area_units < best.total_bbox_area_units {
            return CandidateCompare::Better;
        }
        return CandidateCompare::WorseByTieBboxArea;
    }
    if candidate.total_piece_perimeter_units != best.total_piece_perimeter_units {
        if candidate.total_piece_perimeter_units < best.total_piece_perimeter_units {
            return CandidateCompare::Better;
        }
        return CandidateCompare::WorseByTiePerimeter;
    }
    CandidateCompare::Equal
}

fn build_candidate_selection_telemetry(
    counters: &CandidateSelectionCounters,
    pool_stats: &CandidatePoolStats,
    winner: &Candidate,
) -> CandidateSelectionTelemetry {
    let pool_count = pool_stats.count.max(1);
    let used_stock_mean = pool_stats.used_stock_sum as f64 / pool_count as f64;
    CandidateSelectionTelemetry {
        source: "top_k_population".to_string(),
        top_k_requested: counters.top_k_requested.max(1),
        candidates_total: counters.candidates_total,
        candidates_valid: counters.candidates_valid,
        candidates_invalid_fitness: counters.candidates_invalid_fitness,
        candidates_rejected_primary_objective: counters.candidates_rejected_primary_objective,
        candidates_rejected_tie_bbox_void: counters.candidates_rejected_tie_bbox_void,
        candidates_rejected_tie_bbox_area: counters.candidates_rejected_tie_bbox_area,
        candidates_rejected_tie_perimeter: counters.candidates_rejected_tie_perimeter,
        candidates_rejected_equal: counters.candidates_rejected_equal,
        top_k_unique_signatures: pool_stats.unique_signatures.len() as u32,
        top_k_used_stock_count_min: pool_stats.used_stock_min,
        top_k_used_stock_count_max: pool_stats.used_stock_max,
        top_k_used_stock_count_mean: used_stock_mean,
        top_k_waste_area_mm2_min: from_area_units(pool_stats.waste_min),
        top_k_waste_area_mm2_max: from_area_units(pool_stats.waste_max),
        top_k_waste_area_mm2_mean: mean_area_units(pool_stats.waste_sum, pool_count),
        top_k_bbox_void_area_mm2_min: from_area_units(pool_stats.bbox_void_min),
        top_k_bbox_void_area_mm2_max: from_area_units(pool_stats.bbox_void_max),
        top_k_bbox_void_area_mm2_mean: mean_area_units(pool_stats.bbox_void_sum, pool_count),
        top_k_bbox_area_mm2_min: from_area_units(pool_stats.bbox_area_min),
        top_k_bbox_area_mm2_max: from_area_units(pool_stats.bbox_area_max),
        top_k_bbox_area_mm2_mean: mean_area_units(pool_stats.bbox_area_sum, pool_count),
        top_k_piece_perimeter_mm_min: from_linear_units_u128(pool_stats.perimeter_min),
        top_k_piece_perimeter_mm_max: from_linear_units_u128(pool_stats.perimeter_max),
        top_k_piece_perimeter_mm_mean: mean_linear_units(pool_stats.perimeter_sum, pool_count),
        winner_used_stock_count: winner.used_stock_count,
        winner_waste_area_mm2: from_area_units(winner.total_waste_area_units),
        winner_bbox_void_area_mm2: from_area_units(winner.total_bbox_void_area_units),
        winner_bbox_area_mm2: from_area_units(winner.total_bbox_area_units),
        winner_piece_perimeter_mm: from_linear_units_u128(winner.total_piece_perimeter_units),
    }
}

fn cut_piece_area(piece: &CutPiece) -> u128 {
    area(piece.width, piece.length)
}

fn cut_piece_long_side(piece: &CutPiece) -> usize {
    piece.width.max(piece.length)
}

fn cut_piece_short_side(piece: &CutPiece) -> usize {
    piece.width.min(piece.length)
}

fn cut_piece_is_strip(piece: &CutPiece) -> bool {
    let short = cut_piece_short_side(piece).max(1);
    cut_piece_long_side(piece) / short >= 4
}

fn restart_diversify_variant(seed: u64, restart_idx: u64) -> u8 {
    let mixed = seed
        ^ restart_idx.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ restart_idx.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (mixed % 6) as u8
}

fn diversify_cut_order(base: &[CutPiece], seed: u64, restart_idx: u64) -> Vec<CutPiece> {
    let mut out = base.to_vec();
    match restart_diversify_variant(seed, restart_idx) {
        0 => {}
        1 => {
            out.sort_by(|a, b| {
                cut_piece_area(b)
                    .cmp(&cut_piece_area(a))
                    .then_with(|| cut_piece_long_side(b).cmp(&cut_piece_long_side(a)))
            });
        }
        2 => {
            out.sort_by(|a, b| {
                cut_piece_long_side(b)
                    .cmp(&cut_piece_long_side(a))
                    .then_with(|| cut_piece_short_side(b).cmp(&cut_piece_short_side(a)))
                    .then_with(|| cut_piece_area(b).cmp(&cut_piece_area(a)))
            });
        }
        3 => {
            out.sort_by(|a, b| {
                cut_piece_area(a)
                    .cmp(&cut_piece_area(b))
                    .then_with(|| cut_piece_short_side(a).cmp(&cut_piece_short_side(b)))
            });
        }
        4 => shuffle_with_seed(&mut out, seed ^ restart_idx.wrapping_mul(SEED_STRIDE)),
        _ => {
            out.sort_by(|a, b| {
                cut_piece_is_strip(b)
                    .cmp(&cut_piece_is_strip(a))
                    .then_with(|| cut_piece_area(b).cmp(&cut_piece_area(a)))
                    .then_with(|| cut_piece_long_side(b).cmp(&cut_piece_long_side(a)))
            });
        }
    }
    out
}

fn diversify_stock_order(base: &[StockPiece], seed: u64, restart_idx: u64) -> Vec<StockPiece> {
    let mut out = base.to_vec();
    if out.len() <= 1 {
        return out;
    }
    match restart_diversify_variant(seed ^ 0xD6E8_FD9A_3B6E_DA31, restart_idx) {
        0 => {}
        1 => {
            out.sort_by(|a, b| area(b.width, b.length).cmp(&area(a.width, a.length)));
        }
        2 => {
            out.sort_by(|a, b| area(a.width, a.length).cmp(&area(b.width, b.length)));
        }
        _ => shuffle_with_seed(&mut out, seed.wrapping_add(restart_idx.wrapping_mul(17))),
    }
    out
}

fn shuffle_with_seed<T>(items: &mut [T], seed: u64) {
    if items.len() <= 1 {
        return;
    }
    let mut state = seed ^ 0xA076_1D64_78BD_642F;
    for idx in (1..items.len()).rev() {
        let next = lcg_next(&mut state);
        let swap_idx = (next as usize) % (idx + 1);
        items.swap(idx, swap_idx);
    }
}

fn is_better_stats(
    candidate_used_stock: u32,
    candidate_waste_units: u128,
    best_used_stock: u32,
    best_waste_units: u128,
    objective: &Objective,
) -> bool {
    match objective {
        Objective::MinSheets => {
            if candidate_used_stock < best_used_stock {
                true
            } else if candidate_used_stock == best_used_stock {
                candidate_waste_units < best_waste_units
            } else {
                false
            }
        }
        Objective::MinWaste => {
            if candidate_waste_units < best_waste_units {
                true
            } else if candidate_waste_units == best_waste_units {
                candidate_used_stock < best_used_stock
            } else {
                false
            }
        }
    }
}

/// Apply user-specified qty limits to solutions and collect unplaced items
fn apply_qty_limits(
    solutions: Vec<Solution>,
    prepared: &PreparedInput,
) -> (Vec<Solution>, Vec<UnplacedItem>) {
    let mut kept_solutions = Vec::new();
    let mut unplaced_items = Vec::new();

    // Count sheets per stock_id and track qty limits
    let mut stock_counts: HashMap<String, u32> = HashMap::new();
    let mut stock_limits: HashMap<String, Option<u32>> = HashMap::new();

    // Build stock_limits from prepared.stock_map
    for infos in prepared.stock_map.values() {
        for info in infos {
            stock_limits
                .entry(info.stock_id.clone())
                .or_insert(info.qty_limit);
        }
    }

    for solution in solutions {
        let count = stock_counts.entry(solution.stock_id.clone()).or_insert(0);
        let limit = stock_limits.get(&solution.stock_id).copied().flatten();

        // Check if we've exceeded the limit for this stock type
        let within_limit = match limit {
            Some(max) => *count < max,
            None => true, // No limit
        };

        if within_limit {
            *count += 1;
            kept_solutions.push(solution);
        } else {
            // Collect placements as unplaced items (exceeded qty limit)
            for placement in solution.placements {
                unplaced_items.push(UnplacedItem {
                    item_id: placement.item_id,
                    instance: placement.instance,
                    width_mm: placement.width_mm,
                    height_mm: placement.height_mm,
                    reason: "qty_limit".to_string(),
                });
            }
        }
    }

    (kept_solutions, unplaced_items)
}

/// Calculate stats for kept solutions
fn calculate_solution_stats(solutions: &[Solution]) -> (u32, f64, f64) {
    let used_stock_count = solutions.len() as u32;
    let mut total_stock_area = 0.0;
    let mut total_items_area = 0.0;

    for solution in solutions {
        // Usable area (after trim)
        let usable_width = solution.width_mm - solution.trim_mm.left - solution.trim_mm.right;
        let usable_height = solution.height_mm - solution.trim_mm.top - solution.trim_mm.bottom;
        total_stock_area += usable_width * usable_height;

        // Items area
        for placement in &solution.placements {
            total_items_area += placement.width_mm * placement.height_mm;
        }
    }

    let total_waste_area = total_stock_area - total_items_area;
    (used_stock_count, total_stock_area, total_waste_area)
}

fn build_solutions(
    solution: &cut_optimizer_2d::Solution,
    prepared: &PreparedInput,
) -> Vec<Solution> {
    let mut index_map: HashMap<String, u32> = HashMap::new();
    let mut assigned_counts: HashMap<String, u32> = HashMap::new();
    let mut output = Vec::new();

    for stock in &solution.stock_pieces {
        let info = prepared
            .stock_map
            .get(&(stock.width, stock.length))
            .and_then(|infos| choose_stock_info(infos, &assigned_counts))
            .cloned();
        let (stock_id, full_width_mm, full_height_mm) = match info {
            Some(info) => (info.stock_id, info.full_width_mm, info.full_height_mm),
            None => (
                "unknown".to_string(),
                from_units(stock.width),
                from_units(stock.length),
            ),
        };
        *assigned_counts.entry(stock_id.clone()).or_insert(0) += 1;

        let index = index_map.entry(stock_id.clone()).or_insert(0);
        let sheet_index = *index;
        *index += 1;

        let placements = stock
            .cut_pieces
            .iter()
            .filter_map(|cut_piece| build_placement(cut_piece, prepared))
            .collect();

        output.push(Solution {
            stock_id,
            index: sheet_index,
            width_mm: full_width_mm,
            height_mm: full_height_mm,
            trim_mm: prepared.trim,
            placements,
        });
    }

    output
}

fn choose_stock_info<'a>(
    infos: &'a [StockInfo],
    assigned_counts: &HashMap<String, u32>,
) -> Option<&'a StockInfo> {
    for info in infos {
        if let Some(limit) = info.qty_limit {
            let used = assigned_counts.get(&info.stock_id).copied().unwrap_or(0);
            if used < limit {
                return Some(info);
            }
        }
    }

    infos
        .iter()
        .find(|info| info.qty_limit.is_none())
        .or_else(|| infos.first())
}

fn build_placement(
    cut_piece: &cut_optimizer_2d::ResultCutPiece,
    prepared: &PreparedInput,
) -> Option<Placement> {
    let instance = cut_piece
        .external_id
        .and_then(|id| prepared.instance_map.get(id));
    let info = instance?;

    Some(Placement {
        item_id: info.item_id.clone(),
        instance: info.instance,
        x_mm: from_units(cut_piece.x),
        y_mm: from_units(cut_piece.y),
        width_mm: from_units(cut_piece.width),
        height_mm: from_units(cut_piece.length),
        rotated: cut_piece.is_rotated,
        pattern_direction: info.pattern_direction,
    })
}

fn build_svg(solutions: &[Solution], unplaced_items: &[UnplacedItem], trim: &Trim) -> String {
    const SHEET_GAP: f64 = 50.0; // Gap between sheets in SVG
    const UNPLACED_SECTION_GAP: f64 = 80.0; // Gap before unplaced items section
    const UNPLACED_ITEM_GAP: f64 = 30.0; // Gap between unplaced items

    // Calculate max width and total height for all sheets
    let mut max_width = 0.0_f64;
    let mut total_height = 0.0_f64;

    for (i, solution) in solutions.iter().enumerate() {
        if solution.width_mm > max_width {
            max_width = solution.width_mm;
        }
        total_height += solution.height_mm;
        if i > 0 {
            total_height += SHEET_GAP;
        }
    }

    // Calculate space needed for unplaced items at REAL scale (1:1)
    let mut unplaced_max_height = 0.0_f64;
    let mut unplaced_total_width = 0.0_f64;

    for (i, item) in unplaced_items.iter().enumerate() {
        if item.height_mm > unplaced_max_height {
            unplaced_max_height = item.height_mm;
        }
        unplaced_total_width += item.width_mm;
        if i > 0 {
            unplaced_total_width += UNPLACED_ITEM_GAP;
        }
    }

    // Add space for unplaced section if there are any
    if !unplaced_items.is_empty() {
        total_height += UNPLACED_SECTION_GAP + 40.0 + unplaced_max_height + 50.0; // gap + title + items + labels
        if unplaced_total_width > max_width {
            max_width = unplaced_total_width;
        }
    }

    // Ensure minimum size if no solutions
    if max_width == 0.0 {
        max_width = unplaced_total_width.max(500.0);
    }
    if total_height == 0.0 {
        total_height = 200.0;
    }

    let min_x = -trim.left;
    let min_y = -trim.top;
    let view_w = max_width;
    let view_h = total_height;

    let mut svg = String::new();
    svg.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\" ");
    svg.push_str(&format!(
        "viewBox=\"{} {} {} {}\">",
        fmt_mm(min_x),
        fmt_mm(min_y),
        fmt_mm(view_w),
        fmt_mm(view_h)
    ));

    let mut y_offset = 0.0_f64;

    for (sheet_idx, solution) in solutions.iter().enumerate() {
        let sheet_x = -trim.left;
        let sheet_y = -trim.top + y_offset;
        let sheet_w = solution.width_mm;
        let sheet_h = solution.height_mm;

        // Sheet background (light gray for waste area)
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#f5f5f5\" stroke=\"#333\" stroke-width=\"1\"/>",
            fmt_mm(sheet_x),
            fmt_mm(sheet_y),
            fmt_mm(sheet_w),
            fmt_mm(sheet_h)
        ));

        // Sheet label
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"14\" font-weight=\"bold\" fill=\"#333\">Sheet {} ({})</text>",
            fmt_mm(sheet_x + 5.0),
            fmt_mm(sheet_y + 20.0),
            sheet_idx + 1,
            escape_xml(&solution.stock_id)
        ));

        for placement in &solution.placements {
            let px = placement.x_mm;
            let py = placement.y_mm + y_offset;
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#cfe8ff\" stroke=\"#1f4a6d\" stroke-width=\"0.5\"/>",
                fmt_mm(px),
                fmt_mm(py),
                fmt_mm(placement.width_mm),
                fmt_mm(placement.height_mm)
            ));
            let text_x = px + 2.0;
            let text_y = py + 12.0;
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"#1f4a6d\">{}</text>",
                fmt_mm(text_x),
                fmt_mm(text_y),
                escape_xml(&placement.item_id)
            ));
        }

        y_offset += solution.height_mm + SHEET_GAP;
    }

    // Render unplaced items section at REAL scale (1:1)
    if !unplaced_items.is_empty() {
        let section_y = y_offset + UNPLACED_SECTION_GAP - trim.top;

        // Section title
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"20\" font-weight=\"bold\" fill=\"#c00\">Unplaced Items ({}) - shown at real scale:</text>",
            fmt_mm(-trim.left),
            fmt_mm(section_y),
            unplaced_items.len()
        ));

        let items_y = section_y + 35.0;
        let mut item_x = -trim.left;

        for item in unplaced_items {
            let item_w = item.width_mm;
            let item_h = item.height_mm;

            // Item rectangle at real size (red-tinted for unplaced)
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#ffe0e0\" stroke=\"#c00\" stroke-width=\"2\" stroke-dasharray=\"10,5\"/>",
                fmt_mm(item_x),
                fmt_mm(items_y),
                fmt_mm(item_w),
                fmt_mm(item_h)
            ));

            // Item label (id) - larger font for real scale
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"14\" font-weight=\"bold\" fill=\"#c00\">{} #{}</text>",
                fmt_mm(item_x + 5.0),
                fmt_mm(items_y + 20.0),
                escape_xml(&item.item_id),
                item.instance
            ));

            // Size label inside the item
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"#666\">{}x{}mm</text>",
                fmt_mm(item_x + 5.0),
                fmt_mm(items_y + 38.0),
                item.width_mm as i32,
                item.height_mm as i32
            ));

            // Reason label
            let reason_text = match item.reason.as_str() {
                "oversized" => "TOO LARGE FOR SHEET",
                "qty_limit" => "SHEET LIMIT EXCEEDED",
                _ => &item.reason,
            };
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"#c00\">({})</text>",
                fmt_mm(item_x + 5.0),
                fmt_mm(items_y + 54.0),
                reason_text
            ));

            item_x += item_w + UNPLACED_ITEM_GAP;
        }
    }

    svg.push_str("</svg>");
    svg
}

fn fmt_mm(value: f64) -> String {
    format!("{:.3}", value)
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn to_units(value: f64) -> Result<usize, OptimizeError> {
    if !value.is_finite() {
        return Err(OptimizeError::Constraint {
            message: "invalid numeric value".to_string(),
            details: None,
        });
    }
    if value < 0.0 {
        return Err(OptimizeError::Constraint {
            message: "negative size not allowed".to_string(),
            details: None,
        });
    }
    let scaled = value * SCALE;
    let rounded = scaled.round();
    if rounded < 0.0 || rounded > (usize::MAX as f64) {
        return Err(OptimizeError::Internal("numeric overflow".to_string()));
    }
    Ok(rounded as usize)
}

fn from_units(value: usize) -> f64 {
    value as f64 / SCALE
}

fn from_linear_units_u128(value: u128) -> f64 {
    value as f64 / SCALE
}

fn from_area_units(value: u128) -> f64 {
    value as f64 / (SCALE * SCALE)
}

fn mean_linear_units(sum: u128, count: u32) -> f64 {
    if count == 0 {
        return 0.0;
    }
    (sum as f64 / count as f64) / SCALE
}

fn mean_area_units(sum: u128, count: u32) -> f64 {
    if count == 0 {
        return 0.0;
    }
    (sum as f64 / count as f64) / (SCALE * SCALE)
}

fn area(width: usize, length: usize) -> u128 {
    (width as u128).saturating_mul(length as u128)
}

fn error_response(
    status: StatusCode,
    code: &'static str,
    message: &str,
    details: Option<serde_json::Value>,
) -> Response {
    let body = ErrorResponse {
        status: "error",
        error_code: code,
        message: message.to_string(),
        details,
    };
    (status, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_restarts_and_slice_respects_min_slice() {
        let (restarts, slice_ms) = derive_restarts_and_slice(200, 10);
        assert_eq!(restarts, 2);
        assert_eq!(slice_ms, 100);
    }

    #[test]
    fn derive_restarts_and_slice_keeps_requested_when_possible() {
        let (restarts, slice_ms) = derive_restarts_and_slice(4000, 10);
        assert_eq!(restarts, 10);
        assert_eq!(slice_ms, 400);
    }

    #[test]
    fn derive_standard_restart_plan_caps_restarts_by_effective_slice() {
        let plan = derive_standard_restart_plan(2000, 10, SlaProfile::Balanced);
        assert!(
            plan.effective_restarts <= 3,
            "expected dynamic cap for 2000ms budget, got {}",
            plan.effective_restarts
        );
        assert_eq!(plan.schedule_ms.len(), plan.effective_restarts as usize);
        assert_eq!(plan.schedule_ms.iter().sum::<u64>(), 2000);
    }

    #[test]
    fn build_restart_slice_schedule_is_progressive() {
        let schedule = build_restart_slice_schedule(2400, 3, 550, 40, true);
        assert_eq!(schedule.len(), 3);
        assert_eq!(schedule.iter().sum::<u64>(), 2400);
        assert!(
            schedule[1] >= schedule[2],
            "expected progressive tail slices, got {:?}",
            schedule
        );
    }

    #[test]
    fn is_better_stats_prefers_fewer_sheets_for_min_sheets() {
        assert!(is_better_stats(2, 120, 3, 80, &Objective::MinSheets));
        assert!(!is_better_stats(4, 10, 3, 500, &Objective::MinSheets));
    }

    #[test]
    fn is_better_stats_prefers_lower_waste_for_min_waste() {
        assert!(is_better_stats(4, 100, 2, 120, &Objective::MinWaste));
        assert!(!is_better_stats(1, 300, 2, 200, &Objective::MinWaste));
    }

    #[test]
    fn annealing_temperature_decreases() {
        let t0 = annealing_temperature(0, 10, 1.0, 0.1);
        let t5 = annealing_temperature(5, 10, 1.0, 0.1);
        let t9 = annealing_temperature(9, 10, 1.0, 0.1);
        assert!(t0 >= t5);
        assert!(t5 >= t9);
        assert!(t9 > 0.0);
    }

    #[test]
    fn mutate_alns_plan_changes_seed_or_restarts() {
        let base = AlnsPlan {
            seed: 123,
            requested_restarts: 4,
        };
        let next = mutate_alns_plan(&base, 0, 1);
        assert!(
            next.seed != base.seed || next.requested_restarts != base.requested_restarts,
            "mutation should change seed or restarts"
        );
    }

    #[test]
    fn choose_operator_returns_valid_index() {
        let ops = vec![
            AlnsOperatorState {
                name: "a",
                weight: 0.5,
                score: 0.0,
                selected: 0,
                accepted: 0,
                improved_best: 0,
            },
            AlnsOperatorState {
                name: "b",
                weight: 1.5,
                score: 0.0,
                selected: 0,
                accepted: 0,
                improved_best: 0,
            },
        ];
        let mut rng = 42_u64;
        let idx = choose_operator(&ops, &mut rng);
        assert!(idx < ops.len());
    }
}
