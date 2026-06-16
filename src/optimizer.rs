use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cut_optimizer_2d::{
    CutPiece, GaFitnessConfig, Optimizer, PatternDirection as CutPatternDirection, StockPiece,
};

use crate::config::AppConfig;
use crate::models::{
    AlnsOperatorTelemetry, AlnsTelemetry, Artifacts, BeamTelemetry, CandidateSelectionTelemetry,
    ErrorResponse, GaOverrideParams, GaProfile, GroupShiftTelemetry, LayoutMode, Objective,
    OptimizeRequest, OptimizeResponse, Params, PartitionTelemetry, PatternDirection, Placement,
    PortfolioTelemetry, ProfilePoolPreset, ProfilePoolTelemetry, RestartPolicyTelemetry,
    RetryStrategy, RetryTelemetry, SlaProfile, Solution, Summary, Trim, UnplacedItem,
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
const GROUP_SHIFT_EPS: f64 = 1.0e-7;

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
    /// Minimum per-sheet utilisation in basis-points (0..10000 = 0..100%).
    /// Higher is better — penalises layouts where one sheet is much looser than the rest.
    min_sheet_util_bps: u64,
    /// Maximum edge gap across all sheets, in vendor units. Lower is better —
    /// penalises layouts where pieces hug one corner and leave a big strip
    /// along an opposite edge (the "staircase" / "corner waste" pattern).
    max_edge_gap_units: u64,
    /// Sum of squared per-sheet utilisation deviations from the mean, in bps^2.
    /// Proxy for stddev; lower means sheets are more evenly packed.
    /// We store the un-scaled sum-squared-diff to keep tie-breaks integer-clean.
    sheet_util_sum_sq_diff_bps2: u64,
    /// V9: sum over sheets of the largest free rectangle anchored at the
    /// bottom-right corner, in area units. Higher is better — rewards layouts
    /// where the waste is consolidated into one reusable corner remnant
    /// instead of being scattered as thin corridors between pieces.
    corner_free_area_units: u128,
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
    candidates_rejected_tie_min_util: u32,
    candidates_rejected_tie_corner_free: u32,
    candidates_rejected_equal: u32,
}

enum CandidateCompare {
    Better,
    WorseByPrimaryObjective,
    WorseByTieBboxVoid,
    WorseByTieBboxArea,
    WorseByTiePerimeter,
    WorseByTieMinUtil,
    WorseByTieCornerFree,
    Equal,
}

pub async fn optimize_request(
    req: OptimizeRequest,
    config: &AppConfig,
) -> Result<OptimizeResponse, OptimizeError> {
    let strategy = req.params.retry_strategy.unwrap_or(RetryStrategy::Smart);
    let max_attempts = req.params.max_retry_attempts.unwrap_or(3).max(1) as usize;
    if matches!(strategy, RetryStrategy::Disabled) || max_attempts <= 1 {
        return optimize_request_internal(req, config, SolveMode::Default).await;
    }
    optimize_with_smart_retry(req, config, max_attempts).await
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
    if matches!(mode, SolveMode::Default)
        && req
            .params
            .profile_pool
            .as_ref()
            .map(|pool| pool.enabled.unwrap_or(true))
            .unwrap_or(false)
    {
        return optimize_profile_pool(req, config, used_seed).await;
    }
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
            Some(build_svg(
                &[],
                &prepared.oversized_items,
                &prepared.trim,
                0.0,
            ))
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
                profile_pool: None,
                retry: None,
                partition: None,
                group_shift: group_shift_options(&req.params).map(|options| GroupShiftTelemetry {
                    enabled: options.enabled,
                    time_ms: 0,
                    moves_applied: 0,
                    parts_moved: 0,
                    passes_run: 0,
                    corridor_closed_area_mm2: 0.0,
                    corridor_opportunity_before_mm2: 0.0,
                    corridor_opportunity_after_mm2: 0.0,
                    corridor_opportunity_delta_mm2: 0.0,
                    max_shift_mm: 0.0,
                }),
            },
            solutions: vec![],
            unplaced_items: prepared.oversized_items,
            artifacts: Artifacts {
                svg,
                group_shift_before_svg: None,
                group_shift_diff_svg: None,
            },
        });
    }

    // V8a: dense-first pre-partition path.  When enabled and successful it
    // bypasses the portfolio/standard pipeline entirely; on failure the
    // regular pipeline below acts as the fallback.
    let mut partition_telemetry: Option<PartitionTelemetry> = None;
    let mut partitioned_outcome: Option<RunOutcome> = None;
    if matches!(mode, SolveMode::Default) {
        if let Some(partition_cfg) = &req.params.partition {
            if partition_cfg.enabled.unwrap_or(true) {
                let (outcome, telemetry) =
                    run_partitioned(&req, &prepared, layout_mode, used_seed, time_limit_ms).await?;
                partitioned_outcome = outcome;
                partition_telemetry = Some(telemetry);
            }
        }
    }

    let run_outcome = match (partitioned_outcome, mode) {
        (Some(outcome), _) => outcome,
        (None, SolveMode::Default) => match &req.params.portfolio {
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
                    time_limit_ms,
                    true,
                    Some(&standard_restart_plan),
                )
                .await?
            }
        },
        (None, SolveMode::BeamEndpoint) => {
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
        (None, SolveMode::AlnsEndpoint) => {
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

    Ok(build_response_from_outcome(
        &req,
        &prepared,
        run_outcome,
        time_ms,
        restarts_requested,
        used_seed,
        layout_mode,
        include_svg,
        partition_telemetry,
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_response_from_outcome(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    run_outcome: RunOutcome,
    time_ms: u64,
    restarts_requested: u32,
    used_seed: u64,
    layout_mode: LayoutMode,
    include_svg: bool,
    partition: Option<PartitionTelemetry>,
) -> OptimizeResponse {
    let all_solutions = build_solutions(&run_outcome.candidate.solution, prepared);

    // Apply qty limits and collect unplaced items
    let (mut solutions, mut unplaced_items) = apply_qty_limits(all_solutions, prepared);

    // Merge oversized items (items that didn't fit any stock)
    unplaced_items.extend(prepared.oversized_items.clone());

    let gap_mm = req.params.kerf_mm + req.params.spacing_mm;
    let group_shift_options = group_shift_options(&req.params);
    let group_shift_debug_artifacts = include_svg
        && group_shift_options
            .as_ref()
            .is_some_and(|options| options.enabled)
        && req
            .params
            .group_shift
            .as_ref()
            .and_then(|group_shift| group_shift.debug_artifacts)
            .unwrap_or(false);
    let group_shift_before_solutions = group_shift_debug_artifacts.then(|| solutions.clone());
    let group_shift = group_shift_options
        .map(|options| apply_group_shift_postprocess(&mut solutions, gap_mm, options));

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
        profile_pool: None,
        retry: None,
        partition,
        group_shift,
    };

    let svg = if include_svg {
        Some(build_svg(
            &solutions,
            &unplaced_items,
            &prepared.trim,
            gap_mm,
        ))
    } else {
        None
    };
    let group_shift_before_svg = group_shift_before_solutions
        .as_ref()
        .map(|before| build_svg(before, &unplaced_items, &prepared.trim, gap_mm));
    let group_shift_diff_svg = group_shift_before_solutions
        .as_ref()
        .map(|before| build_group_shift_diff_svg(before, &solutions, &prepared.trim));

    OptimizeResponse {
        status: "ok",
        summary,
        solutions,
        unplaced_items,
        artifacts: Artifacts {
            svg,
            group_shift_before_svg,
            group_shift_diff_svg,
        },
    }
}

struct ProfilePoolCandidate {
    response: OptimizeResponse,
    seed: u64,
    zone_penalty: f64,
    is_rescue: bool,
    visual_waste_regions: u32,
    waste_regions: u32,
    lead_util_pct: f64,
    max_corner_mm2: f64,
    group_shift_opportunity_after_mm2: f64,
    group_shift_opportunity_delta_mm2: f64,
}

fn preset_zone_penalties(preset: ProfilePoolPreset) -> Vec<f64> {
    match preset {
        ProfilePoolPreset::Cheap | ProfilePoolPreset::BalancedQuality => vec![0.2, 0.3, 0.5],
        ProfilePoolPreset::Aggressive => vec![0.2, 0.3, 0.4, 0.5],
    }
}

fn preset_rescue_zone_penalties(preset: ProfilePoolPreset) -> Vec<f64> {
    match preset {
        ProfilePoolPreset::Cheap | ProfilePoolPreset::BalancedQuality => vec![0.4],
        ProfilePoolPreset::Aggressive => Vec::new(),
    }
}

fn preset_rescue_when_zones_gt(preset: ProfilePoolPreset) -> Option<u32> {
    match preset {
        ProfilePoolPreset::Cheap => Some(5),
        ProfilePoolPreset::BalancedQuality => Some(4),
        ProfilePoolPreset::Aggressive => None,
    }
}

fn preset_rescue_accept_min_max_corner_mm2(preset: ProfilePoolPreset) -> Option<f64> {
    match preset {
        ProfilePoolPreset::BalancedQuality => Some(300_000.0),
        ProfilePoolPreset::Cheap | ProfilePoolPreset::Aggressive => None,
    }
}

async fn optimize_profile_pool(
    req: OptimizeRequest,
    config: &AppConfig,
    base_seed: u64,
) -> Result<OptimizeResponse, OptimizeError> {
    let started_at = Instant::now();
    let pool_cfg = req
        .params
        .profile_pool
        .as_ref()
        .expect("optimize_profile_pool called without profile_pool params");
    let preset = pool_cfg.preset;
    let profiles = pool_cfg
        .zone_penalties
        .clone()
        .or_else(|| preset.map(preset_zone_penalties))
        .unwrap_or_else(|| vec![0.2, 0.3, 0.4, 0.5]);
    let rescue_profiles = pool_cfg
        .rescue_zone_penalties
        .clone()
        .or_else(|| preset.map(preset_rescue_zone_penalties))
        .unwrap_or_default();
    let fill_penalty = pool_cfg
        .fill_penalty
        .or_else(|| {
            req.params
                .ga_override
                .as_ref()
                .and_then(|ga| ga.fill_penalty)
        })
        .unwrap_or(0.1);
    let max_lead_drop_pp = pool_cfg.max_lead_drop_pp.unwrap_or(0.8);
    let seed_offsets = pool_cfg.seed_offsets.clone().unwrap_or_default();
    let rescue_when_zones_gt = pool_cfg
        .rescue_when_zones_gt
        .or_else(|| preset.and_then(preset_rescue_when_zones_gt))
        .or_else(|| (!seed_offsets.is_empty() || !rescue_profiles.is_empty()).then_some(5));
    let rescue_when_max_corner_below_mm2 = pool_cfg.rescue_when_max_corner_below_mm2;
    let rescue_accept_min_max_corner_mm2 = pool_cfg
        .rescue_accept_min_max_corner_mm2
        .or_else(|| preset.and_then(preset_rescue_accept_min_max_corner_mm2));

    let mut candidates: Vec<ProfilePoolCandidate> = Vec::new();
    let mut timed_out = 0_u32;
    let mut failed = 0_u32;
    let mut last_error: Option<OptimizeError> = None;

    for &zone_penalty in &profiles {
        record_profile_pool_candidate_result(
            run_profile_pool_candidate(&req, config, base_seed, zone_penalty, fill_penalty, false)
                .await,
            &mut candidates,
            &mut timed_out,
            &mut failed,
            &mut last_error,
        );
    }

    if candidates.is_empty() {
        if timed_out > 0 {
            return Err(OptimizeError::Timeout);
        }
        return Err(last_error.unwrap_or_else(|| {
            OptimizeError::Internal("profile_pool could not produce a valid solution".to_string())
        }));
    }

    let provisional_idx = profile_pool_winner_idx(&candidates, max_lead_drop_pp, None);
    let provisional = &candidates[provisional_idx];
    let zones_rescue = rescue_when_zones_gt
        .map(|threshold| provisional.waste_regions > threshold)
        .unwrap_or(false);
    let corner_rescue = rescue_when_max_corner_below_mm2
        .map(|threshold| provisional.max_corner_mm2 < threshold)
        .unwrap_or(false);
    let rescue_triggered = (!seed_offsets.is_empty() || !rescue_profiles.is_empty())
        && (zones_rescue || corner_rescue);
    let mut seed_offsets_used: Vec<u64> = Vec::new();
    let mut rescue_zone_penalties_used: Vec<f64> = Vec::new();

    if rescue_triggered {
        for &zone_penalty in &rescue_profiles {
            rescue_zone_penalties_used.push(zone_penalty);
            record_profile_pool_candidate_result(
                run_profile_pool_candidate(
                    &req,
                    config,
                    base_seed,
                    zone_penalty,
                    fill_penalty,
                    true,
                )
                .await,
                &mut candidates,
                &mut timed_out,
                &mut failed,
                &mut last_error,
            );
        }
        for offset in &seed_offsets {
            let seed = base_seed.wrapping_add(*offset);
            seed_offsets_used.push(*offset);
            for &zone_penalty in &profiles {
                record_profile_pool_candidate_result(
                    run_profile_pool_candidate(
                        &req,
                        config,
                        seed,
                        zone_penalty,
                        fill_penalty,
                        true,
                    )
                    .await,
                    &mut candidates,
                    &mut timed_out,
                    &mut failed,
                    &mut last_error,
                );
            }
        }
    }

    let rescue_candidates_rejected_by_guard =
        count_rescue_candidates_rejected_by_guard(&candidates, rescue_accept_min_max_corner_mm2);
    let winner_idx = profile_pool_winner_idx(
        &candidates,
        max_lead_drop_pp,
        rescue_accept_min_max_corner_mm2,
    );
    let completed = candidates.len() as u32;
    let mut winner = candidates.swap_remove(winner_idx);
    winner.response.summary.time_ms = started_at.elapsed().as_millis() as u64;
    winner.response.summary.profile_pool = Some(ProfilePoolTelemetry {
        preset,
        profiles_requested: profiles,
        rescue_zone_penalties_requested: rescue_profiles,
        candidates_total: completed.saturating_add(timed_out).saturating_add(failed),
        candidates_completed: completed,
        candidates_timed_out: timed_out,
        candidates_failed: failed,
        rescue_candidates_rejected_by_guard,
        seed_offsets_requested: seed_offsets,
        seed_offsets_used,
        rescue_zone_penalties_used,
        rescue_triggered,
        rescue_when_zones_gt,
        rescue_when_max_corner_below_mm2,
        rescue_accept_min_max_corner_mm2,
        winner_seed: winner.seed,
        winner_zone_penalty: winner.zone_penalty,
        winner_visual_waste_regions: winner.visual_waste_regions,
        winner_waste_regions: winner.waste_regions,
        winner_lead_util_pct: winner.lead_util_pct,
        winner_max_corner_mm2: winner.max_corner_mm2,
        winner_group_shift_opportunity_after_mm2: winner.group_shift_opportunity_after_mm2,
        winner_group_shift_opportunity_delta_mm2: winner.group_shift_opportunity_delta_mm2,
        max_lead_drop_pp,
    });
    Ok(winner.response)
}

async fn run_profile_pool_candidate(
    req: &OptimizeRequest,
    config: &AppConfig,
    seed: u64,
    zone_penalty: f64,
    fill_penalty: f64,
    is_rescue: bool,
) -> Result<ProfilePoolCandidate, OptimizeError> {
    let mut profile_req = req.clone();
    profile_req.params.profile_pool = None;
    profile_req.params.seed = Some(seed);
    let mut ga_override = profile_req
        .params
        .ga_override
        .clone()
        .unwrap_or(GaOverrideParams {
            epochs: None,
            breed_factor: None,
            survival_factor: None,
            top_k_candidates: None,
            zone_penalty: None,
            fill_penalty: None,
        });
    ga_override.zone_penalty = Some(zone_penalty);
    ga_override.fill_penalty = Some(fill_penalty);
    profile_req.params.ga_override = Some(ga_override);

    let response = Box::pin(optimize_request_internal(
        profile_req,
        config,
        SolveMode::Default,
    ))
    .await?;
    let gap_mm = req.params.kerf_mm + req.params.spacing_mm;
    Ok(ProfilePoolCandidate {
        visual_waste_regions: response_waste_regions(&response, 0.0),
        waste_regions: response_waste_regions(&response, gap_mm),
        lead_util_pct: response_lead_util_pct(&response),
        max_corner_mm2: response_max_corner_mm2(&response),
        group_shift_opportunity_after_mm2: response_group_shift_opportunity_after_mm2(&response),
        group_shift_opportunity_delta_mm2: response_group_shift_opportunity_delta_mm2(&response),
        response,
        seed,
        zone_penalty,
        is_rescue,
    })
}

fn record_profile_pool_candidate_result(
    result: Result<ProfilePoolCandidate, OptimizeError>,
    candidates: &mut Vec<ProfilePoolCandidate>,
    timed_out: &mut u32,
    failed: &mut u32,
    last_error: &mut Option<OptimizeError>,
) {
    match result {
        Ok(candidate) => {
            candidates.push(candidate);
        }
        Err(OptimizeError::Timeout) => {
            *timed_out = timed_out.saturating_add(1);
        }
        Err(err @ OptimizeError::Constraint { .. }) | Err(err @ OptimizeError::Internal(_)) => {
            *failed = failed.saturating_add(1);
            *last_error = Some(err);
        }
    }
}

fn count_rescue_candidates_rejected_by_guard(
    candidates: &[ProfilePoolCandidate],
    rescue_accept_min_max_corner_mm2: Option<f64>,
) -> u32 {
    candidates
        .iter()
        .filter(|candidate| {
            profile_pool_candidate_rejected_by_rescue_guard(
                candidate,
                rescue_accept_min_max_corner_mm2,
            )
        })
        .count() as u32
}

fn profile_pool_candidate_rejected_by_rescue_guard(
    candidate: &ProfilePoolCandidate,
    rescue_accept_min_max_corner_mm2: Option<f64>,
) -> bool {
    candidate.is_rescue
        && rescue_accept_min_max_corner_mm2
            .map(|threshold| candidate.max_corner_mm2 < threshold)
            .unwrap_or(false)
}

fn profile_pool_winner_idx(
    candidates: &[ProfilePoolCandidate],
    max_lead_drop_pp: f64,
    rescue_accept_min_max_corner_mm2: Option<f64>,
) -> usize {
    let best_lead = candidates
        .iter()
        .filter(|candidate| {
            !profile_pool_candidate_rejected_by_rescue_guard(
                candidate,
                rescue_accept_min_max_corner_mm2,
            )
        })
        .map(|candidate| candidate.lead_util_pct)
        .fold(0.0_f64, f64::max);
    let mut winner_idx: Option<usize> = None;
    for (idx, candidate) in candidates.iter().enumerate() {
        if profile_pool_candidate_rejected_by_rescue_guard(
            candidate,
            rescue_accept_min_max_corner_mm2,
        ) {
            continue;
        }
        let eligible =
            candidate.lead_util_pct + max_lead_drop_pp >= best_lead || candidate.waste_regions <= 4;
        if !eligible {
            continue;
        }
        if let Some(current_idx) = winner_idx {
            if profile_pool_candidate_better(candidate, &candidates[current_idx]) {
                winner_idx = Some(idx);
            }
        } else {
            winner_idx = Some(idx);
        }
    }
    winner_idx.unwrap_or_else(|| {
        candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                !profile_pool_candidate_rejected_by_rescue_guard(
                    candidate,
                    rescue_accept_min_max_corner_mm2,
                )
            })
            .min_by(|(_, a), (_, b)| {
                profile_pool_candidate_order(a, b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    })
}

fn profile_pool_candidate_better(
    candidate: &ProfilePoolCandidate,
    best: &ProfilePoolCandidate,
) -> bool {
    matches!(
        profile_pool_candidate_order(candidate, best),
        Some(std::cmp::Ordering::Less)
    )
}

fn profile_pool_candidate_order(
    a: &ProfilePoolCandidate,
    b: &ProfilePoolCandidate,
) -> Option<std::cmp::Ordering> {
    Some(
        (a.response.summary.used_stock_count, a.waste_regions)
            .cmp(&(b.response.summary.used_stock_count, b.waste_regions))
            .then_with(|| {
                a.group_shift_opportunity_after_mm2
                    .partial_cmp(&b.group_shift_opportunity_after_mm2)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                b.group_shift_opportunity_delta_mm2
                    .partial_cmp(&a.group_shift_opportunity_delta_mm2)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                b.lead_util_pct
                    .partial_cmp(&a.lead_util_pct)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                b.max_corner_mm2
                    .partial_cmp(&a.max_corner_mm2)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
    )
}

fn response_lead_util_pct(resp: &OptimizeResponse) -> f64 {
    let mut utils = per_sheet_utils(resp);
    if utils.is_empty() {
        return 0.0;
    }
    utils.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    if utils.len() == 1 {
        utils[0]
    } else {
        utils[..utils.len() - 1].iter().sum::<f64>() / (utils.len() - 1) as f64
    }
}

fn response_waste_regions(resp: &OptimizeResponse, gap_mm: f64) -> u32 {
    let gap = mm_to_units_lossy(gap_mm);
    response_stock_pieces(resp)
        .iter()
        .map(|stock| waste_region_count(stock, gap))
        .sum()
}

fn response_max_corner_mm2(resp: &OptimizeResponse) -> f64 {
    response_stock_pieces(resp)
        .iter()
        .map(corner_free_rect_area)
        .max()
        .map(from_area_units)
        .unwrap_or(0.0)
}

fn response_group_shift_opportunity_after_mm2(resp: &OptimizeResponse) -> f64 {
    resp.summary
        .group_shift
        .as_ref()
        .map(|group_shift| group_shift.corridor_opportunity_after_mm2)
        .unwrap_or(0.0)
}

fn response_group_shift_opportunity_delta_mm2(resp: &OptimizeResponse) -> f64 {
    resp.summary
        .group_shift
        .as_ref()
        .map(|group_shift| group_shift.corridor_opportunity_delta_mm2)
        .unwrap_or(0.0)
}

fn response_stock_pieces(resp: &OptimizeResponse) -> Vec<cut_optimizer_2d::ResultStockPiece> {
    resp.solutions
        .iter()
        .map(|solution| {
            let usable_width = solution.width_mm - solution.trim_mm.left - solution.trim_mm.right;
            let usable_height = solution.height_mm - solution.trim_mm.top - solution.trim_mm.bottom;
            let cut_pieces = solution
                .placements
                .iter()
                .map(|placement| cut_optimizer_2d::ResultCutPiece {
                    external_id: None,
                    x: mm_to_units_lossy(placement.x_mm),
                    y: mm_to_units_lossy(placement.y_mm),
                    width: mm_to_units_lossy(placement.width_mm),
                    length: mm_to_units_lossy(placement.height_mm),
                    pattern_direction: CutPatternDirection::None,
                    is_rotated: placement.rotated,
                })
                .collect();
            cut_optimizer_2d::ResultStockPiece {
                width: mm_to_units_lossy(usable_width),
                length: mm_to_units_lossy(usable_height),
                pattern_direction: CutPatternDirection::None,
                cut_pieces,
                waste_pieces: Vec::new(),
                price: 0,
            }
        })
        .collect()
}

fn mm_to_units_lossy(value: f64) -> usize {
    if value <= 0.0 || !value.is_finite() {
        0
    } else {
        (value * SCALE).round() as usize
    }
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

#[derive(Debug, Clone, PartialEq)]
enum FailureMode {
    /// No solution was produced at all.
    NoSolution,
    /// Used more sheets than the request's "ideal" (currently 4 for the
    /// heavy fixture, but in general we use whatever the first attempt did
    /// as the baseline to beat).
    TooManySheets { sheets: u32 },
    /// 4 sheets but the worst sheet has very low util — likely a "lumpy"
    /// layout that the GA failed to balance.
    VeryLumpy { min_util: f64, range: f64 },
    /// 4 sheets, min_util is below 90% but not catastrophic.
    Lumpy { min_util: f64, range: f64 },
    /// min_util >= 90% but the spread between sheets is large — looks
    /// "lopsided" even though the worst sheet is OK.
    Imbalanced { min_util: f64, range: f64 },
}

impl FailureMode {
    fn description(&self) -> String {
        match self {
            FailureMode::NoSolution => "no_solution".to_string(),
            FailureMode::TooManySheets { sheets } => format!("too_many_sheets({})", sheets),
            FailureMode::VeryLumpy { min_util, range } => {
                format!("very_lumpy(min={:.2}%,range={:.2}%)", min_util, range)
            }
            FailureMode::Lumpy { min_util, range } => {
                format!("lumpy(min={:.2}%,range={:.2}%)", min_util, range)
            }
            FailureMode::Imbalanced { min_util, range } => {
                format!("imbalanced(min={:.2}%,range={:.2}%)", min_util, range)
            }
        }
    }
}

fn per_sheet_utils(resp: &OptimizeResponse) -> Vec<f64> {
    resp.solutions
        .iter()
        .map(|sol| {
            let uw = sol.width_mm - sol.trim_mm.left - sol.trim_mm.right;
            let uh = sol.height_mm - sol.trim_mm.top - sol.trim_mm.bottom;
            let sheet_a = uw * uh;
            let used: f64 = sol
                .placements
                .iter()
                .map(|p| p.width_mm * p.height_mm)
                .sum();
            if sheet_a > 0.0 {
                used / sheet_a * 100.0
            } else {
                0.0
            }
        })
        .collect()
}

fn assess_failure(resp: &OptimizeResponse) -> Option<FailureMode> {
    let utils = per_sheet_utils(resp);
    if utils.is_empty() {
        return Some(FailureMode::NoSolution);
    }
    let n_sheets = resp.summary.used_stock_count as u32;
    let min_util = utils.iter().copied().fold(f64::INFINITY, f64::min);
    let max_util = utils.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = max_util - min_util;

    // "Too many sheets" is the worst case — it means we failed to find a
    // 4-sheet solution at all.  Switch to nested mode to find a tighter
    // packing.
    if n_sheets >= 5 {
        return Some(FailureMode::TooManySheets { sheets: n_sheets });
    }
    // Very lumpy: one sheet is much looser than the others.  Try a
    // different search strategy (nested mode).
    if min_util < 88.0 {
        return Some(FailureMode::VeryLumpy { min_util, range });
    }
    // Slightly lumpy: just under the 90% bar.  A different seed is often
    // enough to find a better seed of the search.
    if min_util < 90.0 {
        return Some(FailureMode::Lumpy { min_util, range });
    }
    // All sheets are at least 90% but the spread is wide.  A different
    // seed may find a more even packing.
    if range > 5.0 {
        return Some(FailureMode::Imbalanced { min_util, range });
    }
    None
}

fn choose_strategy(
    failure: &FailureMode,
    retry_idx: usize,
    rebalance_enabled: bool,
) -> &'static str {
    match failure {
        FailureMode::NoSolution => "different_seed",
        FailureMode::Imbalanced { .. } | FailureMode::Lumpy { .. } => {
            // Lumpy / imbalanced layouts are a SEARCH problem - the GA
            // just didn't explore the right region.  V8b: when the caller
            // opted into partition control, first try the inter-sheet
            // rebalance (move parts from the sparsest sheet onto the denser
            // ones) — it attacks the partition directly instead of hoping a
            // wider search stumbles on a better one.  Otherwise: doubling
            // the restart count gives the GA more diverse starting points
            // per attempt; different seed is the second retry.
            match retry_idx {
                1 if rebalance_enabled => "rebalance",
                1 => "more_restarts",
                _ => "different_seed",
            }
        }
        FailureMode::TooManySheets { .. } => "switch_to_nested",
        FailureMode::VeryLumpy { .. } => {
            // First retry: rebalance when enabled (the very-lumpy sheet is a
            // partition defect), else nested mode (often finds a tighter
            // pack).  Second+ retry: more restarts in the same mode.
            match retry_idx {
                1 if rebalance_enabled => "rebalance",
                1 => "switch_to_nested",
                _ => "more_restarts",
            }
        }
    }
}

fn apply_strategy(req: &mut OptimizeRequest, strategy: &str, retry_idx: usize) {
    let seed_offset = (retry_idx as u64).saturating_mul(100);
    let current_seed = req.params.seed.unwrap_or(0);
    match strategy {
        "different_seed" => {
            req.params.seed = Some(current_seed.wrapping_add(seed_offset));
        }
        "switch_to_nested" => {
            req.params.layout_mode = Some(LayoutMode::Nested);
            req.params.seed = Some(current_seed.wrapping_add(seed_offset));
        }
        "more_restarts" => {
            // V5: double the restart count (capped at 20) to give the GA
            // a wider search space.  The slice_ms = time_limit_ms /
            // restarts shrinks accordingly, but the diversity gain
            // usually outweighs the per-restart time loss.
            let current_restarts = req.params.restarts.unwrap_or(5);
            let new_restarts = current_restarts.saturating_mul(2).min(20);
            req.params.restarts = Some(new_restarts);
            req.params.seed = Some(current_seed.wrapping_add(seed_offset));
        }
        _ => {}
    }
}

/// Lower is better.  We rank by (sheets, -min_util, range).  An empty
/// solution is the worst possible.
fn response_score(resp: &OptimizeResponse) -> (i32, f64, f64) {
    let utils = per_sheet_utils(resp);
    if utils.is_empty() {
        return (i32::MAX, 0.0, 100.0);
    }
    let min_util = utils.iter().copied().fold(f64::INFINITY, f64::min);
    let max_util = utils.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = max_util - min_util;
    (resp.summary.used_stock_count as i32, -min_util, range)
}

fn is_better_response(a: &OptimizeResponse, b: &OptimizeResponse) -> bool {
    response_score(a) < response_score(b)
}

fn is_response_ok(resp: &OptimizeResponse) -> bool {
    assess_failure(resp).is_none()
}

async fn optimize_with_smart_retry(
    req: OptimizeRequest,
    config: &AppConfig,
    max_attempts: usize,
) -> Result<OptimizeResponse, OptimizeError> {
    // Attempt 1: original params.
    let mut best = optimize_request_internal(req.clone(), config, SolveMode::Default).await?;
    let initial_failure = assess_failure(&best);

    // V8a: a successfully applied dense-first partition is the intended
    // shape — its utils are deliberately skewed ([~96, ~96, ~96, slack]),
    // which the balance-oriented failure assessor would misread as lumpy.
    // Never retry over it.
    if best
        .summary
        .partition
        .as_ref()
        .map(|p| p.applied)
        .unwrap_or(false)
    {
        return Ok(best);
    }

    // If attempt 1 already passes, no retry needed.
    if initial_failure.is_none() {
        return Ok(best);
    }

    let rebalance_enabled = req
        .params
        .partition
        .as_ref()
        .map(|p| p.enabled.unwrap_or(true))
        .unwrap_or(false);
    let mut strategies: Vec<String> = Vec::new();
    let total_attempts = max_attempts.max(2);

    for retry_idx in 1..total_attempts {
        if is_response_ok(&best) {
            break;
        }
        // The "current" failure mode is recomputed from `best` because a
        // successful retry may have shifted the situation.
        let current_failure = assess_failure(&best).unwrap();
        let mut strategy = choose_strategy(&current_failure, retry_idx, rebalance_enabled);
        if strategy == "rebalance" {
            match rebalance_attempt(&req, &best, retry_idx).await? {
                Some(attempt) => {
                    strategies.push("rebalance".to_string());
                    // Dense-first acceptance: the rebalance intentionally
                    // worsens min_util while raising lead utilisation, so it
                    // is judged by dense_score, not the balance score.  An
                    // accepted rebalance is terminal — further balance-driven
                    // retries would only undo it.
                    if dense_score(&attempt) < dense_score(&best) {
                        best = attempt;
                        break;
                    }
                    continue;
                }
                // No verified move possible — fall back to a regular
                // strategy for this retry slot.
                None => strategy = "different_seed",
            }
        }
        let mut req_retry = req.clone();
        apply_strategy(&mut req_retry, strategy, retry_idx);
        let attempt = optimize_request_internal(req_retry, config, SolveMode::Default).await?;
        strategies.push(strategy.to_string());
        if is_better_response(&attempt, &best) {
            best = attempt;
        }
    }

    // Attach retry telemetry.
    best.summary.retry = Some(RetryTelemetry {
        attempts: (strategies.len() as u32) + 1,
        retries: strategies.len() as u32,
        strategies,
        initial_failure: initial_failure.map(|f| f.description()),
    });
    Ok(best)
}

// ---------------------------------------------------------------------------
// V8a: dense-first partition control via iterative peeling.
//
// The GA distributes parts across sheets implicitly, which is what produces
// lumpy layouts (CONTEXT.md ЭТАП 13).  A naive fix — pre-partition by area,
// then pack each forced group — does NOT work: the GA cannot re-pack even a
// known-feasible 95.9%-utilisation group into one sheet (verified
// empirically; its single-sheet ceiling for forced groups is ~90-93%).  Dense
// sheets only emerge when the GA is free to choose WHICH parts spill over.
//
// Peeling exploits exactly that freedom: pack all remaining parts, freeze the
// densest sheet of the result AS-IS (geometry included, no re-pack), drop its
// parts from the pool and re-optimize the remainder.  Each iteration the GA
// concentrates its best packing on one sheet, and the slack drains onto the
// final remainder sheet as one large reusable zone.
// ---------------------------------------------------------------------------

/// Run the GA restart machinery on a subset of the prepared cut pieces.
async fn run_subset(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    group: &[usize],
    layout_mode: LayoutMode,
    seed: u64,
    budget_ms: u64,
) -> Result<Option<(Candidate, u32)>, OptimizeError> {
    let sub_prepared = PreparedInput {
        stock_pieces: prepared.stock_pieces.clone(),
        cut_pieces: group
            .iter()
            .map(|&i| prepared.cut_pieces[i].clone())
            .collect(),
        instance_map: prepared.instance_map.clone(),
        stock_map: prepared.stock_map.clone(),
        trim: prepared.trim,
        cut_width: prepared.cut_width,
        oversized_items: Vec::new(),
    };
    let restarts: u64 = 4;
    let slice_ms = (budget_ms / restarts).max(MIN_SLICE_MS);
    match run_restarts_with_budget(
        req,
        &sub_prepared,
        restarts,
        slice_ms,
        layout_mode,
        seed,
        budget_ms,
        false,
        None,
    )
    .await
    {
        Ok(outcome) => Ok(Some((outcome.candidate, outcome.restarts_used))),
        Err(OptimizeError::Internal(e)) => Err(OptimizeError::Internal(e)),
        // Constraint/timeout on a sub-problem just means this subset is not
        // packable within the budget — the caller falls back.
        Err(_) => Ok(None),
    }
}

/// Pack one group of cut pieces and accept the result only if it fits a
/// single sheet.
async fn pack_group_single_sheet(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    group: &[usize],
    layout_mode: LayoutMode,
    seed: u64,
    budget_ms: u64,
) -> Result<Option<(Candidate, u32)>, OptimizeError> {
    match run_subset(req, prepared, group, layout_mode, seed, budget_ms).await? {
        Some((candidate, used)) if candidate.used_stock_count == 1 => Ok(Some((candidate, used))),
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// V9b: width-matched column constructor.
//
// The GA's guillotine packings leave thin vertical corridors because column
// widths are not coordinated with the sheet width.  The logs/perfect etalons
// all share one structure: vertical columns whose widths sum to ~the sheet
// width, pieces of near-identical width stacked inside each column, waste
// collected as one staircase at the corner.  This deterministic constructor
// builds exactly that shape; its candidates compete with the GA attempts in
// the peeling loop under the same selection rules, so it only ever replaces a
// GA layout when it is at least as good.
// ---------------------------------------------------------------------------

/// Build one column-structured sheet from `pool`, removing the placed pieces.
/// `tol` is the maximum allowed gap between the column width and a stacked
/// piece's width (vendor units); larger tol packs more but leaves slivers.
fn build_column_stock_piece(
    prepared: &PreparedInput,
    pool: &mut Vec<usize>,
    sheet_w: usize,
    sheet_h: usize,
    gap: usize,
    tol: usize,
    anchor_by_area: bool,
) -> Option<cut_optimizer_2d::ResultStockPiece> {
    // (placed width, placed length) candidates per orientation.
    let orientations = |idx: usize| -> Vec<(usize, usize)> {
        let p = &prepared.cut_pieces[idx];
        let mut v = vec![(p.width, p.length)];
        if p.can_rotate && p.width != p.length {
            v.push((p.length, p.width));
        }
        v
    };

    let mut placed: Vec<cut_optimizer_2d::ResultCutPiece> = Vec::new();
    let mut x: usize = 0;
    loop {
        let space = sheet_w.saturating_sub(x);
        if space == 0 || pool.is_empty() {
            break;
        }
        // Anchor piece defines the column width.
        let mut anchor: Option<(usize, usize, usize, u128)> = None; // (pool_pos, w, l, area)
        for (pos, &idx) in pool.iter().enumerate() {
            for (w, l) in orientations(idx) {
                if w > space || l > sheet_h {
                    continue;
                }
                let piece_area = area(w, l);
                let better = match anchor {
                    None => true,
                    Some((_, bw, _, barea)) => {
                        if anchor_by_area {
                            piece_area > barea || (piece_area == barea && w > bw)
                        } else {
                            w > bw || (w == bw && piece_area > barea)
                        }
                    }
                };
                if better {
                    anchor = Some((pos, w, l, piece_area));
                }
            }
        }
        let Some((anchor_pos, col_w, anchor_l, _)) = anchor else {
            break;
        };
        let anchor_idx = pool.swap_remove(anchor_pos);
        placed.push(cut_optimizer_2d::ResultCutPiece {
            external_id: prepared.cut_pieces[anchor_idx].external_id,
            x,
            y: 0,
            width: col_w,
            length: anchor_l,
            pattern_direction: CutPatternDirection::None,
            is_rotated: col_w != prepared.cut_pieces[anchor_idx].width,
        });
        let mut y = anchor_l.saturating_add(gap);

        // Stack width-matched pieces below the anchor.
        loop {
            let height_left = sheet_h.saturating_sub(y);
            if height_left == 0 || pool.is_empty() {
                break;
            }
            // Pick the piece whose width is closest to the column width
            // (within tol), tie-break on the largest area placed.
            let mut pick: Option<(usize, usize, usize, usize, u128)> = None; // (pos, w, l, sliver, area)
            for (pos, &idx) in pool.iter().enumerate() {
                for (w, l) in orientations(idx) {
                    if w > col_w || w + tol < col_w || l > height_left {
                        continue;
                    }
                    let sliver = col_w - w;
                    let piece_area = area(w, l);
                    let better = match pick {
                        None => true,
                        Some((_, _, _, bsliver, barea)) => {
                            sliver < bsliver || (sliver == bsliver && piece_area > barea)
                        }
                    };
                    if better {
                        pick = Some((pos, w, l, sliver, piece_area));
                    }
                }
            }
            let Some((pos, w, l, _, _)) = pick else {
                break;
            };
            let idx = pool.swap_remove(pos);
            placed.push(cut_optimizer_2d::ResultCutPiece {
                external_id: prepared.cut_pieces[idx].external_id,
                x,
                y,
                width: w,
                length: l,
                pattern_direction: CutPatternDirection::None,
                is_rotated: w != prepared.cut_pieces[idx].width,
            });
            y = y.saturating_add(l).saturating_add(gap);
        }

        x = x.saturating_add(col_w).saturating_add(gap);
    }

    if placed.is_empty() {
        return None;
    }
    Some(cut_optimizer_2d::ResultStockPiece {
        width: sheet_w,
        length: sheet_h,
        pattern_direction: CutPatternDirection::None,
        cut_pieces: placed,
        waste_pieces: Vec::new(),
        price: 0,
    })
}

/// Build one shelf-structured (FFDH) sheet from `pool`, removing the placed
/// pieces.  Pieces are oriented landscape (`wide_orient`) or portrait, sorted
/// by height, packed into horizontal shelves first-fit; shelves are then
/// reordered by used width (widest at the top) and pieces inside each shelf
/// by height, so the free space forms one monotonic staircase running to the
/// bottom-right corner — the logs/perfect waste shape.
fn build_shelf_stock_piece(
    prepared: &PreparedInput,
    pool: &mut Vec<usize>,
    sheet_w: usize,
    sheet_h: usize,
    gap: usize,
    wide_orient: bool,
) -> Option<cut_optimizer_2d::ResultStockPiece> {
    struct ShelfPiece {
        idx: usize,
        w: usize,
        l: usize,
    }
    struct Shelf {
        height: usize,
        used_w: usize,
        pieces: Vec<ShelfPiece>,
    }

    let orient = |idx: usize| -> (usize, usize) {
        let p = &prepared.cut_pieces[idx];
        if !p.can_rotate || p.width == p.length {
            return (p.width, p.length);
        }
        let (a, b) = (p.width.max(p.length), p.width.min(p.length));
        if wide_orient {
            (a, b)
        } else {
            (b, a)
        }
    };

    // Tallest pieces first so each shelf's height is set by its first piece.
    let mut order: Vec<usize> = pool.clone();
    order.sort_by(|&a, &b| {
        let (wa, la) = orient(a);
        let (wb, lb) = orient(b);
        lb.cmp(&la).then(wb.cmp(&wa))
    });

    let mut shelves: Vec<Shelf> = Vec::new();
    let mut used_h: usize = 0;
    let mut placed_ids: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for idx in order {
        let (mut w, mut l) = orient(idx);
        if w > sheet_w {
            // Fall back to the other orientation if the preferred one is too
            // wide for the sheet.
            std::mem::swap(&mut w, &mut l);
            if w > sheet_w {
                continue;
            }
        }
        let mut target: Option<usize> = None;
        for (s, shelf) in shelves.iter().enumerate() {
            if l <= shelf.height && shelf.used_w + gap + w <= sheet_w {
                target = Some(s);
                break;
            }
        }
        match target {
            Some(s) => {
                let shelf = &mut shelves[s];
                shelf.used_w += gap + w;
                shelf.pieces.push(ShelfPiece { idx, w, l });
            }
            None => {
                let next_h = used_h + if shelves.is_empty() { 0 } else { gap } + l;
                if next_h > sheet_h {
                    continue;
                }
                used_h = next_h;
                shelves.push(Shelf {
                    height: l,
                    used_w: w,
                    pieces: vec![ShelfPiece { idx, w, l }],
                });
            }
        }
        placed_ids.insert(idx);
    }
    if shelves.is_empty() {
        return None;
    }

    // Monotonic staircase: widest shelf on top, tallest piece on the left.
    shelves.sort_by(|a, b| b.used_w.cmp(&a.used_w));
    let mut placed: Vec<cut_optimizer_2d::ResultCutPiece> = Vec::new();
    let mut y: usize = 0;
    for shelf in &mut shelves {
        shelf.pieces.sort_by(|a, b| b.l.cmp(&a.l));
        let mut x: usize = 0;
        for p in &shelf.pieces {
            placed.push(cut_optimizer_2d::ResultCutPiece {
                external_id: prepared.cut_pieces[p.idx].external_id,
                x,
                y,
                width: p.w,
                length: p.l,
                pattern_direction: CutPatternDirection::None,
                is_rotated: p.w != prepared.cut_pieces[p.idx].width,
            });
            x = x.saturating_add(p.w).saturating_add(gap);
        }
        y = y.saturating_add(shelf.height).saturating_add(gap);
    }
    pool.retain(|i| !placed_ids.contains(i));
    Some(cut_optimizer_2d::ResultStockPiece {
        width: sheet_w,
        length: sheet_h,
        pattern_direction: CutPatternDirection::None,
        cut_pieces: placed,
        waste_pieces: Vec::new(),
        price: 0,
    })
}

/// Column-layout candidates over `remaining`.  When `multi_sheet` is false
/// the candidate holds one best-effort dense sheet (peel competitor); when
/// true it must place every remaining piece (slack/remainder competitor).
fn build_column_candidates(
    prepared: &PreparedInput,
    remaining: &[usize],
    sheet_w: usize,
    sheet_h: usize,
    gap: usize,
    multi_sheet: bool,
) -> Vec<Candidate> {
    // Generators: width-matched columns (tol in vendor units = mm * SCALE)
    // and monotonic FFDH shelves (landscape / portrait orientation).
    enum Gen {
        Columns { tol: usize, by_area: bool },
        Shelves { wide: bool },
    }
    const COLUMN_VARIANTS: [(usize, bool); 4] =
        [(0, false), (40_000, false), (80_000, false), (40_000, true)];
    let mut generators: Vec<Gen> = COLUMN_VARIANTS
        .iter()
        .map(|&(tol, by_area)| Gen::Columns { tol, by_area })
        .collect();
    generators.push(Gen::Shelves { wide: true });
    generators.push(Gen::Shelves { wide: false });

    let mut out = Vec::new();
    for generator in &generators {
        let mut pool: Vec<usize> = remaining.to_vec();
        let mut stocks = Vec::new();
        loop {
            let stock = match generator {
                Gen::Columns { tol, by_area } => build_column_stock_piece(
                    prepared, &mut pool, sheet_w, sheet_h, gap, *tol, *by_area,
                ),
                Gen::Shelves { wide } => {
                    build_shelf_stock_piece(prepared, &mut pool, sheet_w, sheet_h, gap, *wide)
                }
            };
            match stock {
                Some(stock) => stocks.push(stock),
                None => break,
            }
            if !multi_sheet || pool.is_empty() {
                break;
            }
        }
        if stocks.is_empty() || (multi_sheet && !pool.is_empty()) {
            continue;
        }
        let solution = cut_optimizer_2d::Solution::from_components(1.0, stocks, 0);
        out.push(build_candidate(solution));
    }
    out
}

/// Count connected free regions (>= 5000 mm^2) on a sheet, 10mm grid, with
/// pieces inflated by the kerf gap.  Mirrors the benchmark's flood-fill
/// metric closely enough to rank candidates by waste fragmentation.
fn waste_region_count(stock: &cut_optimizer_2d::ResultStockPiece, gap: usize) -> u32 {
    const CELL: usize = 10_000; // 10mm in vendor units
    let nx = stock.width / CELL;
    let ny = stock.length / CELL;
    if nx == 0 || ny == 0 {
        return 0;
    }
    let mut occ = vec![false; nx * ny];
    for p in &stock.cut_pieces {
        let x0 = p.x.saturating_sub(gap);
        let y0 = p.y.saturating_sub(gap);
        let x1 = (p.x + p.width + gap).min(stock.width);
        let y1 = (p.y + p.length + gap).min(stock.length);
        let i0 = x0 / CELL;
        let j0 = y0 / CELL;
        let i1 = (x1.saturating_sub(1) / CELL).min(nx - 1);
        let j1 = (y1.saturating_sub(1) / CELL).min(ny - 1);
        for j in j0..=j1 {
            for i in i0..=i1 {
                occ[j * nx + i] = true;
            }
        }
    }
    let mut seen = vec![false; nx * ny];
    let mut regions = 0_u32;
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            if occ[j * nx + i] || seen[j * nx + i] {
                continue;
            }
            let mut cells = 0_u64;
            stack.push((i, j));
            seen[j * nx + i] = true;
            while let Some((ci, cj)) = stack.pop() {
                cells += 1;
                if ci > 0 && !occ[cj * nx + ci - 1] && !seen[cj * nx + ci - 1] {
                    seen[cj * nx + ci - 1] = true;
                    stack.push((ci - 1, cj));
                }
                if ci + 1 < nx && !occ[cj * nx + ci + 1] && !seen[cj * nx + ci + 1] {
                    seen[cj * nx + ci + 1] = true;
                    stack.push((ci + 1, cj));
                }
                if cj > 0 && !occ[(cj - 1) * nx + ci] && !seen[(cj - 1) * nx + ci] {
                    seen[(cj - 1) * nx + ci] = true;
                    stack.push((ci, cj - 1));
                }
                if cj + 1 < ny && !occ[(cj + 1) * nx + ci] && !seen[(cj + 1) * nx + ci] {
                    seen[(cj + 1) * nx + ci] = true;
                    stack.push((ci, cj + 1));
                }
            }
            // 5000 mm^2 = 50 cells of 10x10mm.
            if cells >= 50 {
                regions += 1;
            }
        }
    }
    regions
}

fn candidate_waste_regions(candidate: &Candidate, gap: usize) -> u32 {
    candidate
        .solution
        .stock_pieces
        .iter()
        .map(|s| waste_region_count(s, gap))
        .sum()
}

fn densest_sheet_waste_regions(sheets: &[cut_optimizer_2d::ResultStockPiece], gap: usize) -> u32 {
    if sheets.is_empty() {
        return u32::MAX;
    }
    let idx = sheets
        .iter()
        .enumerate()
        .max_by(|a, b| {
            let ua = stock_piece_util_pct(&sheets[a.0]);
            let ub = stock_piece_util_pct(&sheets[b.0]);
            ua.partial_cmp(&ub).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    waste_region_count(&sheets[idx], gap)
}

/// Peel-loop selection rule, shared by GA attempts and column candidates.
fn peel_candidate_better(
    candidate: &Candidate,
    best: Option<&Candidate>,
    last_iteration: bool,
    remaining_area: u128,
    usable_area: u128,
    sheets_left_after: usize,
    gap: usize,
) -> bool {
    if last_iteration {
        // Remainder (slack sheet): fewer sheets, then the LEAST fragmented
        // waste (V9b: one staircase beats a slightly larger but scattered
        // corner rect), then the larger corner zone.
        match best {
            None => true,
            Some(best) => {
                (
                    candidate.used_stock_count,
                    candidate_waste_regions(candidate, gap),
                    std::cmp::Reverse(candidate.corner_free_area_units),
                ) < (
                    best.used_stock_count,
                    candidate_waste_regions(best, gap),
                    std::cmp::Reverse(best.corner_free_area_units),
                )
            }
        }
    } else {
        // V13: zones-penalised peel selection.  When nested candidates have
        // higher util but more waste regions, a pure util comparison lets
        // them win even though they produce fragmented frozen sheets (9 zones
        // vs 6.8 for guillotine).  We penalise each waste region beyond 1
        // (the ideal = single corner remnant) by ZONES_PENALTY_PP per zone.
        // This makes the effective comparison:
        //   effective_util = densest_util - max(0, zones - 1) * ZONES_PENALTY_PP
        // So a candidate with 3 zones needs to be >0.6pp denser than a
        // candidate with 1 zone to win — balancing density vs. consolidation.
        const ZONES_PENALTY_PP: f64 = 0.8;

        let densest_util = candidate
            .solution
            .stock_pieces
            .iter()
            .map(stock_piece_util_pct)
            .fold(0.0_f64, f64::max);
        let frozen_used = (usable_area as f64 * densest_util / 100.0) as u128;
        let feasible = remaining_area.saturating_sub(frozen_used)
            <= usable_area.saturating_mul(sheets_left_after as u128);
        if !feasible {
            return false;
        }
        let candidate_zones = densest_sheet_waste_regions(&candidate.solution.stock_pieces, gap);
        let candidate_effective =
            densest_util - (candidate_zones.saturating_sub(1) as f64) * ZONES_PENALTY_PP;
        match best {
            None => true,
            Some(best) => {
                let best_densest_util = best
                    .solution
                    .stock_pieces
                    .iter()
                    .map(stock_piece_util_pct)
                    .fold(0.0_f64, f64::max);
                let best_zones = densest_sheet_waste_regions(&best.solution.stock_pieces, gap);
                let best_effective =
                    best_densest_util - (best_zones.saturating_sub(1) as f64) * ZONES_PENALTY_PP;
                if (candidate_effective - best_effective).abs() < 0.01 {
                    // Effective utils are equal — fall back to raw util then zones
                    if (densest_util - best_densest_util).abs() < 0.01 {
                        candidate_zones <= best_zones
                    } else {
                        densest_util > best_densest_util
                    }
                } else {
                    candidate_effective > best_effective
                }
            }
        }
    }
}

fn stock_piece_util_pct(stock: &cut_optimizer_2d::ResultStockPiece) -> f64 {
    let stock_area = area(stock.width, stock.length);
    if stock_area == 0 {
        return 0.0;
    }
    let used: u128 = stock
        .cut_pieces
        .iter()
        .map(|p| area(p.width, p.length))
        .sum();
    used as f64 / stock_area as f64 * 100.0
}

async fn run_partitioned(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    layout_mode: LayoutMode,
    base_seed: u64,
    time_limit_ms: u64,
) -> Result<(Option<RunOutcome>, PartitionTelemetry), OptimizeError> {
    let cfg = req
        .params
        .partition
        .as_ref()
        .expect("run_partitioned called without partition params");
    let fallback = |reason: String| PartitionTelemetry {
        applied: false,
        group_sizes: vec![],
        group_area_pct: vec![],
        fallback_reason: Some(reason),
        densest_zones: vec![],
    };

    // Peeling reasons about a single uniform sheet size.
    let Some(template) = prepared.stock_pieces.first().copied() else {
        return Ok((None, fallback("no_stock".to_string())));
    };
    if prepared
        .stock_pieces
        .iter()
        .any(|s| s.width != template.width || s.length != template.length)
    {
        return Ok((None, fallback("mixed_stock_sizes".to_string())));
    }

    let usable_area = area(template.width, template.length);
    let total_area: u128 = prepared.cut_pieces.iter().map(cut_piece_area).sum();
    if usable_area == 0 || total_area == 0 {
        return Ok((None, fallback("zero_area".to_string())));
    }
    let n_min_sheets = total_area.div_ceil(usable_area) as usize;
    if n_min_sheets < 2 {
        return Ok((None, fallback("single_sheet_problem".to_string())));
    }

    let started = Instant::now();
    let peel_budget_ms = cfg
        .sheet_budget_ms
        .unwrap_or_else(|| (time_limit_ms / n_min_sheets as u64).max(500))
        .max(200);
    // Quality over latency: when the caller explicitly raises the per-peel
    // budget, let the run take up to one full budget per planned sheet
    // (plus one for the remainder) instead of clipping at 2x time_limit.
    let total_budget_ms = time_limit_ms
        .saturating_mul(2)
        .max(peel_budget_ms.saturating_mul(n_min_sheets as u64 + 1))
        .max(1_000);

    let mut remaining: Vec<usize> = (0..prepared.cut_pieces.len()).collect();
    let mut frozen: Vec<cut_optimizer_2d::ResultStockPiece> = Vec::new();
    let mut group_sizes: Vec<u32> = Vec::new();
    let mut group_area_pct: Vec<f64> = Vec::new();
    let mut densest_zones: Vec<u32> = Vec::new();
    let mut restarts_total: u32 = 0;
    let mut fitness = 1.0_f64;
    let mut peel_idx: u64 = 0;

    loop {
        let remaining_area: u128 = remaining
            .iter()
            .map(|&i| cut_piece_area(&prepared.cut_pieces[i]))
            .sum();
        let last_iteration = remaining_area <= usable_area;
        let elapsed = started.elapsed().as_millis() as u64;
        if elapsed >= total_budget_ms {
            return Ok((None, fallback("peel_budget_exhausted".to_string())));
        }
        let budget = peel_budget_ms.min(total_budget_ms - elapsed).max(200);
        peel_idx += 1;

        // Best-of-K within the peel budget: a single subset run converges in
        // well under a second (early-stop), so extra budget is converted into
        // independent re-seeded attempts; keep the attempt whose densest
        // sheet is the tightest.  An attempt is only eligible if the parts
        // left after freezing still fit the remaining sheet count by area —
        // freezing a loose sheet now forces a 5th sheet later.
        let sheets_left_after = n_min_sheets.saturating_sub(frozen.len() + 1);
        let peel_started = Instant::now();
        // Many short attempts beat few long ones: a subset GA run converges
        // (early-stop) in ~1-2s, so longer slices are wasted while a fresh
        // re-seed explores a genuinely different region.
        let attempt_budget_ms = (budget / 8).max(600);
        let mut best_attempt: Option<Candidate> = None;
        let mut attempt_idx: u64 = 0;
        const MAX_PEEL_ATTEMPTS: u64 = 16;
        while attempt_idx < MAX_PEEL_ATTEMPTS {
            let peel_elapsed = peel_started.elapsed().as_millis() as u64;
            if attempt_idx > 0 && peel_elapsed >= budget {
                break;
            }
            let this_budget = attempt_budget_ms.min((budget - peel_elapsed.min(budget)).max(200));
            let seed = base_seed
                .wrapping_add(((peel_idx << 8) + attempt_idx + 1).wrapping_mul(SEED_STRIDE));
            // V11: alternate guillotine/nested in peel attempts for non-last
            // iterations.  Nested is not constrained by guillotine cuts and
            // can pack the densest sheet more tightly, potentially raising
            // lead util from 94% toward 95%+.  For the slack iteration we
            // keep guillotine only (shelf/column constructors handle it).
            let attempt_mode = if last_iteration {
                layout_mode
            } else if attempt_idx % 2 == 0 {
                layout_mode
            } else {
                LayoutMode::Nested
            };
            attempt_idx += 1;
            let Some((candidate, used)) =
                run_subset(req, prepared, &remaining, attempt_mode, seed, this_budget).await?
            else {
                continue;
            };
            restarts_total += used;
            if peel_candidate_better(
                &candidate,
                best_attempt.as_ref(),
                last_iteration,
                remaining_area,
                usable_area,
                sheets_left_after,
                prepared.cut_width,
            ) {
                best_attempt = Some(candidate);
            }
        }
        // V9b: deterministic width-matched column layouts compete with the
        // GA attempts under the same selection rule — they win only when at
        // least as dense (peels) / at least as corner-consolidated (slack).
        for candidate in build_column_candidates(
            prepared,
            &remaining,
            template.width,
            template.length,
            prepared.cut_width,
            last_iteration,
        ) {
            if peel_candidate_better(
                &candidate,
                best_attempt.as_ref(),
                last_iteration,
                remaining_area,
                usable_area,
                sheets_left_after,
                prepared.cut_width,
            ) {
                best_attempt = Some(candidate);
            }
        }
        let Some(candidate) = best_attempt else {
            return Ok((None, fallback(format!("peel_{}_failed", peel_idx))));
        };
        fitness = fitness.min(candidate.solution.fitness);

        if last_iteration {
            // Remainder fits one sheet by area; keep the whole sub-solution
            // (normally a single slack sheet).
            for stock in candidate.solution.stock_pieces {
                group_sizes.push(stock.cut_pieces.len() as u32);
                group_area_pct.push(stock_piece_util_pct(&stock));
                densest_zones.push(waste_region_count(&stock, prepared.cut_width));
                frozen.push(stock);
            }
            break;
        }

        // Freeze the densest sheet as-is and drop its parts from the pool.
        let Some(densest) = candidate.solution.stock_pieces.into_iter().max_by(|a, b| {
            stock_piece_util_pct(a)
                .partial_cmp(&stock_piece_util_pct(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            return Ok((None, fallback(format!("peel_{}_empty", peel_idx))));
        };
        if densest.cut_pieces.is_empty() {
            return Ok((None, fallback(format!("peel_{}_empty_sheet", peel_idx))));
        }
        let frozen_ids: std::collections::HashSet<usize> = densest
            .cut_pieces
            .iter()
            .filter_map(|p| p.external_id)
            .collect();
        if frozen_ids.len() != densest.cut_pieces.len() {
            return Ok((None, fallback("missing_external_ids".to_string())));
        }
        remaining.retain(|i| !frozen_ids.contains(i));
        group_sizes.push(densest.cut_pieces.len() as u32);
        let densest_util_pct = stock_piece_util_pct(&densest);
        group_area_pct.push(densest_util_pct);
        densest_zones.push(waste_region_count(&densest, prepared.cut_width));
        frozen.push(densest);
        if remaining.is_empty() {
            break;
        }
    }

    // Reject peelings that need more sheets than the area lower bound — the
    // regular pipeline reaches that bound reliably, so anything above it is
    // a regression.
    if frozen.len() > n_min_sheets {
        return Ok((
            None,
            fallback(format!(
                "peeled_{}_sheets_gt_min_{}",
                frozen.len(),
                n_min_sheets
            )),
        ));
    }

    let merged = cut_optimizer_2d::Solution::from_components(fitness, frozen, 0);
    let candidate = build_candidate(merged);
    let counters = CandidateSelectionCounters {
        top_k_requested: u32::try_from(resolve_ga_runtime(req).top_k).unwrap_or(u32::MAX),
        candidates_total: group_sizes.len() as u32,
        candidates_valid: group_sizes.len() as u32,
        ..Default::default()
    };
    let mut selection = build_candidate_selection_telemetry(&counters, &candidate);
    selection.source = "dense_first_peeling".to_string();
    let telemetry = PartitionTelemetry {
        applied: true,
        group_sizes,
        group_area_pct,
        fallback_reason: None,
        densest_zones,
    };
    Ok((
        Some(RunOutcome {
            candidate,
            restarts_used: restarts_total,
            timeout_reason: None,
            restart_policy: None,
            portfolio: None,
            beam: None,
            alns: None,
            candidate_selection: Some(selection),
        }),
        telemetry,
    ))
}

// ---------------------------------------------------------------------------
// V8b: post-GA rebalance (dense-first direction).
//
// Smart-retry strategy for lumpy fallback layouts: move parts from the
// sparsest sheet onto the denser sheets (verified by an actual single-sheet
// repack), draining the slack onto one remainder sheet.  Accepted by the
// dense-first score (lead utilisation), not the balance score.
// ---------------------------------------------------------------------------

/// Dense-first quality score, lower is better: (sheets, -lead_util, -min_util)
/// where lead_util is the mean utilisation of the n-1 densest sheets.
fn dense_score(resp: &OptimizeResponse) -> (i32, f64, f64) {
    let mut utils = per_sheet_utils(resp);
    if utils.is_empty() {
        return (i32::MAX, 0.0, 0.0);
    }
    utils.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let lead = if utils.len() > 1 {
        utils[..utils.len() - 1].iter().sum::<f64>() / (utils.len() - 1) as f64
    } else {
        utils[0]
    };
    (
        resp.summary.used_stock_count as i32,
        -lead,
        -utils[utils.len() - 1],
    )
}

async fn rebalance_attempt(
    req: &OptimizeRequest,
    best: &OptimizeResponse,
    retry_idx: usize,
) -> Result<Option<OptimizeResponse>, OptimizeError> {
    let started = Instant::now();
    let layout_mode = req.params.layout_mode.unwrap_or(LayoutMode::Guillotine);
    let prepared = prepare_input(req)?;
    if best.solutions.len() < 2 || !best.unplaced_items.is_empty() {
        return Ok(None);
    }
    let Some(template) = prepared.stock_pieces.first().copied() else {
        return Ok(None);
    };
    if prepared
        .stock_pieces
        .iter()
        .any(|s| s.width != template.width || s.length != template.length)
    {
        return Ok(None);
    }

    // (item_id, instance) -> index into prepared.cut_pieces (== external_id
    // by construction in prepare_input).
    let mut piece_index: HashMap<(&str, u32), usize> = HashMap::new();
    for (i, info) in prepared.instance_map.iter().enumerate() {
        piece_index.insert((info.item_id.as_str(), info.instance), i);
    }
    let mut sheets: Vec<Vec<usize>> = Vec::with_capacity(best.solutions.len());
    for sol in &best.solutions {
        let mut ids = Vec::with_capacity(sol.placements.len());
        for p in &sol.placements {
            match piece_index.get(&(p.item_id.as_str(), p.instance)) {
                Some(&i) => ids.push(i),
                None => return Ok(None),
            }
        }
        sheets.push(ids);
    }

    let utils = per_sheet_utils(best);
    let donor = utils
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut receivers: Vec<usize> = (0..sheets.len()).filter(|&i| i != donor).collect();
    receivers.sort_by(|&a, &b| {
        utils[b]
            .partial_cmp(&utils[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let time_limit_ms = req.params.time_limit_ms.unwrap_or(10_000);
    let pack_budget_ms = (time_limit_ms / 8).clamp(500, 2_000);
    let total_budget_ms = time_limit_ms.saturating_mul(2).max(2_000);
    let base_seed = req
        .params
        .seed
        .unwrap_or(0)
        .wrapping_add(77_777)
        .wrapping_add(retry_idx as u64);

    // Donor pieces, largest first: moving big slabs onto dense sheets frees
    // the most slack per verified repack.
    let mut donor_pieces = sheets[donor].clone();
    donor_pieces.sort_by(|&a, &b| {
        cut_piece_area(&prepared.cut_pieces[b]).cmp(&cut_piece_area(&prepared.cut_pieces[a]))
    });

    const MAX_PACK_TRIALS: usize = 10;
    let mut packed: HashMap<usize, Candidate> = HashMap::new();
    let mut moved = 0_usize;
    let mut packs_tried = 0_usize;
    for piece in donor_pieces {
        if packs_tried >= MAX_PACK_TRIALS || started.elapsed().as_millis() as u64 >= total_budget_ms
        {
            break;
        }
        for &r in &receivers {
            if packs_tried >= MAX_PACK_TRIALS {
                break;
            }
            let mut trial = sheets[r].clone();
            trial.push(piece);
            packs_tried += 1;
            let seed = base_seed.wrapping_add((packs_tried as u64).wrapping_mul(SEED_STRIDE));
            if let Some((candidate, _)) =
                pack_group_single_sheet(req, &prepared, &trial, layout_mode, seed, pack_budget_ms)
                    .await?
            {
                sheets[r] = trial;
                packed.insert(r, candidate);
                sheets[donor].retain(|&x| x != piece);
                moved += 1;
                break;
            }
        }
    }
    if moved == 0 {
        return Ok(None);
    }

    // Re-pack the remaining sheets (donor + untouched receivers) to obtain
    // vendor-unit geometry for the merged solution.  If the donor emptied
    // completely the sheet is dropped.
    let mut merged_stocks = Vec::new();
    let mut fitness = 1.0_f64;
    let order: Vec<usize> = receivers
        .iter()
        .copied()
        .chain(std::iter::once(donor))
        .collect();
    for idx in order {
        if sheets[idx].is_empty() {
            continue;
        }
        let candidate = match packed.remove(&idx) {
            Some(c) => c,
            None => {
                let seed = base_seed.wrapping_add(((1_000 + idx) as u64).wrapping_mul(SEED_STRIDE));
                match pack_group_single_sheet(
                    req,
                    &prepared,
                    &sheets[idx],
                    layout_mode,
                    seed,
                    pack_budget_ms,
                )
                .await?
                {
                    Some((c, _)) => c,
                    None => return Ok(None),
                }
            }
        };
        fitness = fitness.min(candidate.solution.fitness);
        merged_stocks.extend(candidate.solution.stock_pieces);
    }
    let merged = cut_optimizer_2d::Solution::from_components(fitness, merged_stocks, 0);
    let candidate = build_candidate(merged);
    let outcome = RunOutcome {
        candidate,
        restarts_used: packs_tried as u32,
        timeout_reason: None,
        restart_policy: None,
        portfolio: None,
        beam: None,
        alns: None,
        candidate_selection: None,
    };
    let include_svg = req.params.include_svg.unwrap_or(true);
    let restarts_requested = req.params.restarts.unwrap_or(1).max(1);
    let used_seed = req.params.seed.unwrap_or(0);
    let time_ms = started.elapsed().as_millis() as u64;
    Ok(Some(build_response_from_outcome(
        req,
        &prepared,
        outcome,
        time_ms,
        restarts_requested,
        used_seed,
        layout_mode,
        include_svg,
        None,
    )))
}

#[derive(Clone)]
struct PortfolioPlan {
    name: &'static str,
    seed: u64,
    requested_restarts: u32,
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

fn resolve_ga_fitness_config(req: &OptimizeRequest) -> Option<GaFitnessConfig> {
    let override_cfg = req.params.ga_override.as_ref()?;
    match (override_cfg.zone_penalty, override_cfg.fill_penalty) {
        (None, None) => None,
        (zone_penalty, fill_penalty) => Some(GaFitnessConfig {
            zone_penalty: zone_penalty.unwrap_or(0.3),
            fill_penalty: fill_penalty.unwrap_or(0.1),
        }),
    }
}

fn pick_best_candidate(
    set: cut_optimizer_2d::SolutionSet,
    objective: &Objective,
    counters: &mut CandidateSelectionCounters,
    kerf_gap_units: usize,
) -> Option<Candidate> {
    let mut best: Option<Candidate> = None;
    for mut solution in set.solutions {
        counters.candidates_total = counters.candidates_total.saturating_add(1);
        if solution.fitness < 0.0 {
            counters.candidates_invalid_fitness =
                counters.candidates_invalid_fitness.saturating_add(1);
            continue;
        }
        // V9.1: compact BEFORE building the candidate so corner_free_area and
        // the other tie-breakers compare candidates in their final (anchored)
        // geometry.  Previously only the winner was compacted, which made the
        // corner_free tie-break see pre-compaction noise.
        compact_solution(&mut solution, kerf_gap_units);
        let candidate = build_candidate(solution);
        counters.candidates_valid = counters.candidates_valid.saturating_add(1);
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
                CandidateCompare::WorseByTieMinUtil => {
                    counters.candidates_rejected_tie_min_util =
                        counters.candidates_rejected_tie_min_util.saturating_add(1);
                    Some(current)
                }
                CandidateCompare::WorseByTieCornerFree => {
                    counters.candidates_rejected_tie_corner_free = counters
                        .candidates_rejected_tie_corner_free
                        .saturating_add(1);
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
    total_budget_ms: u64,
    allow_timeout_rescue: bool,
    restart_plan: Option<&RestartPlan>,
) -> Result<RunOutcome, OptimizeError> {
    let ga_runtime = resolve_ga_runtime(req);
    let ga_fitness_config = resolve_ga_fitness_config(req);
    // V9.1: kerf+spacing gap in vendor units, used to compact every candidate
    // before ranking (see pick_best_candidate).
    let kerf_gap_units = ((req.params.kerf_mm + req.params.spacing_mm) * SCALE).round() as usize;
    let mut best: Option<Candidate> = None;
    let mut selection_counters = CandidateSelectionCounters {
        top_k_requested: u32::try_from(ga_runtime.top_k).unwrap_or(u32::MAX),
        ..Default::default()
    };
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
        let ga_fitness_config = ga_fitness_config;

        let mut handle = tokio::task::spawn_blocking(move || {
            let diversified_stock = diversify_stock_order(&stock_templates, seed, restart_idx);
            let diversified_cut = diversify_cut_order(&cut_templates, seed, restart_idx);
            let mut optimizer = Optimizer::new();
            optimizer
                .set_random_seed(seed)
                .set_cut_width(cut_width)
                .set_ga_epochs(ga_runtime.epochs)
                .set_ga_breed_factor(ga_runtime.breed_factor)
                .set_ga_survival_factor(ga_runtime.survival_factor)
                .add_stock_pieces(diversified_stock.into_iter())
                .add_cut_pieces(diversified_cut.into_iter());
            let run_optimizer = || match mode {
                LayoutMode::Nested => optimizer.optimize_nested_top_k(ga_runtime.top_k, |_| {}),
                LayoutMode::Guillotine => {
                    optimizer.optimize_guillotine_top_k(ga_runtime.top_k, |_| {})
                }
            };
            let ga_set = if let Some(config) = ga_fitness_config {
                cut_optimizer_2d::with_ga_fitness_config(config, run_optimizer)
            } else {
                run_optimizer()
            };
            // V4 heuristic seeding: also run a pure First-Fit-Decreasing
            // heuristic and prepend it to the candidate pool.  The
            // service-level compare_candidates tie-breakers will then pick
            // the best between the GA-evolved solutions and the
            // hand-crafted heuristic.
            let mut combined = cut_optimizer_2d::SolutionSet { solutions: vec![] };
            if let Ok(set) = ga_set {
                let mut heuristic_solutions = match mode {
                    LayoutMode::Nested => optimizer.build_nested_heuristic(),
                    LayoutMode::Guillotine => optimizer.build_guillotine_heuristic(),
                };
                combined.solutions.append(&mut heuristic_solutions);
                combined.solutions.extend(set.solutions);
            } else {
                combined = cut_optimizer_2d::SolutionSet { solutions: vec![] };
            }
            Ok(combined)
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
                        kerf_gap_units,
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
                                | CandidateCompare::WorseByTieMinUtil
                                | CandidateCompare::WorseByTieCornerFree
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
        // V9.1: candidates are already compacted inside pick_best_candidate,
        // so the winner needs no extra compaction pass here.
        let candidate_selection = Some(build_candidate_selection_telemetry(
            &selection_counters,
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
            let ga_fitness_config = ga_fitness_config;
            let mut rescue_handle = tokio::task::spawn_blocking(move || {
                let diversified_stock =
                    diversify_stock_order(&stock_templates, rescue_seed, restart_idx);
                let diversified_cut = diversify_cut_order(&cut_templates, rescue_seed, restart_idx);
                let mut optimizer = Optimizer::new();
                optimizer
                    .set_random_seed(rescue_seed)
                    .set_cut_width(cut_width)
                    .set_ga_epochs(ga_runtime.epochs)
                    .set_ga_breed_factor(ga_runtime.breed_factor)
                    .set_ga_survival_factor(ga_runtime.survival_factor)
                    .add_stock_pieces(diversified_stock.into_iter())
                    .add_cut_pieces(diversified_cut.into_iter());
                let run_optimizer = || match mode {
                    LayoutMode::Nested => optimizer.optimize_nested_top_k(ga_runtime.top_k, |_| {}),
                    LayoutMode::Guillotine => {
                        optimizer.optimize_guillotine_top_k(ga_runtime.top_k, |_| {})
                    }
                };
                if let Some(config) = ga_fitness_config {
                    cut_optimizer_2d::with_ga_fitness_config(config, run_optimizer)
                } else {
                    run_optimizer()
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
                            kerf_gap_units,
                        ) {
                            rescue_used = true;
                            rescue_budget_ms = Some(rescue_budget);
                            if best_found_at_restart.is_none() {
                                best_found_at_restart = Some(planned_restarts as u32 + 1);
                            }
                            let candidate_selection = Some(build_candidate_selection_telemetry(
                                &selection_counters,
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
    let plans = build_portfolio_plans(base_seed, default_restarts, candidate_count);
    let candidates_total = plans.len() as u32;
    let mut candidates_started: u32 = 0;
    let mut candidates_completed: u32 = 0;
    let mut candidates_timed_out: u32 = 0;
    let mut candidates_failed: u32 = 0;
    let started_at = Instant::now();

    let mut best: Option<(
        Candidate,
        &'static str,
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
                        plan.name,
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
                winner_strategy: winner_strategy.to_string(),
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

fn build_portfolio_plans(
    base_seed: u64,
    requested_restarts: u32,
    candidate_count: u32,
) -> Vec<PortfolioPlan> {
    let mut plans = Vec::with_capacity(candidate_count as usize);
    for idx in 0..candidate_count {
        let (name, restarts) = match idx % 4 {
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
        plans.push(PortfolioPlan {
            name,
            seed,
            requested_restarts: restarts,
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

/// Slide each cut piece on every sheet as far left and as far up as it can
/// go while keeping the kerf gap from every other piece, then translate each
/// sheet's bounding box to balance the four edge gaps.
///
/// Mutates the solution in place.  Returns the total number of pieces that
/// were moved (a proxy for "how much the layout improved").
///
/// Two passes:
/// 1. **Per-piece compaction** — for each piece, compute the leftmost x and
///    topmost y it can occupy without colliding with any neighbour (using the
///    kerf+spacing gap).  Iterate until fixed point so a single move can
///    unlock further moves for other pieces.
/// 2. **Bbox centering** — once pieces are tight against each other, shift the
///    whole sheet's bounding box so the largest of the four edge gaps is
///    minimised.  If the bbox already fills the sheet, this is a no-op.
///
/// This does NOT change which pieces are placed, only their (x, y).  After
/// compaction `build_candidate` should be re-run to refresh the metrics.
fn compact_solution(solution: &mut cut_optimizer_2d::Solution, kerf_gap_units: usize) -> u32 {
    let mut total_moved: u32 = 0;

    for stock in solution.stock_pieces.iter_mut() {
        if stock.cut_pieces.is_empty() {
            continue;
        }
        let n = stock.cut_pieces.len();

        // Pass 1: per-piece compaction (slide each piece as far left+up as
        // possible while keeping kerf_gap from every other piece).  Loop
        // until a full pass produces no moves — order matters: sliding one
        // piece left can unlock further moves for pieces above/below it.
        //
        // For each piece we compute its leftmost valid x as MAX(0, all
        // "right_edge + kerf" of pieces that are currently to the left of
        // it in y-overlap).  We use MAX — not MIN — because each such value
        // is a LOWER bound on where the piece can sit, and the tightest
        // (largest) lower bound wins.  Using MIN here would let a piece
        // slide past a neighbour that was itself just slid into place, which
        // produces overlap.
        loop {
            let mut moved_this_pass = false;
            for i in 0..n {
                let cur_x = stock.cut_pieces[i].x;
                let cur_y = stock.cut_pieces[i].y;
                let cur_w = stock.cut_pieces[i].width;
                let cur_h = stock.cut_pieces[i].length;

                let mut min_x_lower_bound: usize = 0;
                let mut min_y_lower_bound: usize = 0;

                for j in 0..n {
                    if i == j {
                        continue;
                    }
                    // Read other's CURRENT position (may have been moved
                    // earlier in this same pass).
                    let other_x = stock.cut_pieces[j].x;
                    let other_y = stock.cut_pieces[j].y;
                    let other_w = stock.cut_pieces[j].width;
                    let other_h = stock.cut_pieces[j].length;

                    // y-axis overlap: if our y-range intersects the other's
                    // y-range, sliding in x is constrained by the other.
                    let y_overlap = cur_y < other_y.saturating_add(other_h)
                        && cur_y.saturating_add(cur_h) > other_y;
                    if y_overlap {
                        let right_edge = other_x
                            .saturating_add(other_w)
                            .saturating_add(kerf_gap_units);
                        if right_edge <= cur_x {
                            // other is to the left of us with at least kerf;
                            // right_edge is a LOWER bound on where we can be.
                            if right_edge > min_x_lower_bound {
                                min_x_lower_bound = right_edge;
                            }
                        }
                    }

                    // x-axis overlap: symmetric constraint for vertical slide.
                    let x_overlap = cur_x < other_x.saturating_add(other_w)
                        && cur_x.saturating_add(cur_w) > other_x;
                    if x_overlap {
                        let top_edge = other_y
                            .saturating_add(other_h)
                            .saturating_add(kerf_gap_units);
                        if top_edge <= cur_y {
                            if top_edge > min_y_lower_bound {
                                min_y_lower_bound = top_edge;
                            }
                        }
                    }
                }

                let new_x = min_x_lower_bound; // already clamped to >= 0
                let new_y = min_y_lower_bound;
                // Only ever move LEFT or UP, never right/down.
                if new_x < cur_x || new_y < cur_y {
                    stock.cut_pieces[i].x = new_x;
                    stock.cut_pieces[i].y = new_y;
                    total_moved = total_moved.saturating_add(1);
                    moved_this_pass = true;
                }
            }
            if !moved_this_pass {
                break;
            }
        }

        // Pass 2 (V9): corner anchoring — pieces stay slid to the top-left
        // corner after pass 1.  The old bbox-centering pass was removed: it
        // smeared the waste evenly around the perimeter, whereas the target
        // quality (logs/perfect) is a single consolidated remnant at the
        // bottom-right corner.
    }

    total_moved
}

/// V9: largest free rectangle anchored at the bottom-right corner of a sheet.
/// Candidate left boundaries are the right edges of pieces (plus 0); for each
/// boundary L the rect spans x in [L, W] and its height is limited by the
/// lowest piece bottom within that x-band. O(n^2), n is small (<= ~40).
fn corner_free_rect_area(stock: &cut_optimizer_2d::ResultStockPiece) -> u128 {
    let w = stock.width;
    let h = stock.length;
    if stock.cut_pieces.is_empty() {
        return area(w, h);
    }
    let mut lefts: Vec<usize> = stock
        .cut_pieces
        .iter()
        .map(|p| p.x.saturating_add(p.width))
        .filter(|&r| r < w)
        .collect();
    lefts.push(0);
    let mut best: u128 = 0;
    for &l in &lefts {
        // Height of the free band [l, w): sheet bottom minus the lowest piece
        // bottom among pieces overlapping that band.
        let mut max_bottom: usize = 0;
        for p in &stock.cut_pieces {
            let overlaps = p.x < w && p.x.saturating_add(p.width) > l;
            if overlaps {
                max_bottom = max_bottom.max(p.y.saturating_add(p.length));
            }
        }
        let band_h = h.saturating_sub(max_bottom);
        let rect = area(w.saturating_sub(l), band_h);
        if rect > best {
            best = rect;
        }
    }
    best
}

fn build_candidate(solution: cut_optimizer_2d::Solution) -> Candidate {
    let mut total_waste_area: u128 = 0;
    let mut total_bbox_area: u128 = 0;
    let mut total_bbox_void_area: u128 = 0;
    let mut total_piece_perimeter: u128 = 0;
    // Track per-sheet utilisation so we can penalise unbalanced layouts.
    let mut min_sheet_util_bps: u64 = u64::MAX;
    // Track max edge gap (vendor units) across all sheets — visual "staircase" / corner waste.
    let mut max_edge_gap_units: u64 = 0;
    // Collect per-sheet utilisation in bps to compute stddev-like spread metric.
    let mut sheet_utils_bps: Vec<u64> = Vec::with_capacity(solution.stock_pieces.len());
    // V9: total corner-anchored free rectangle area across sheets.
    let mut corner_free_area_units: u128 = 0;

    for stock in &solution.stock_pieces {
        corner_free_area_units =
            corner_free_area_units.saturating_add(corner_free_rect_area(stock));
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
            // Per-sheet utilisation in basis-points (0..10000).
            if stock_area > 0 {
                let util_bps = (used_area.saturating_mul(10_000) / stock_area) as u64;
                min_sheet_util_bps = min_sheet_util_bps.min(util_bps);
                sheet_utils_bps.push(util_bps);
            }
            // Max edge gap = max distance from piece bbox to any of the 4 sheet edges.
            // Lower means pieces fill the sheet more evenly in all directions.
            let gap_left = min_x as u64;
            let gap_right = (stock.width as u64).saturating_sub(max_x as u64);
            let gap_top = min_y as u64;
            let gap_bottom = (stock.length as u64).saturating_sub(max_y as u64);
            let sheet_edge_gap = gap_left.max(gap_right).max(gap_top).max(gap_bottom);
            if sheet_edge_gap > max_edge_gap_units {
                max_edge_gap_units = sheet_edge_gap;
            }
        }
        total_waste_area = total_waste_area.saturating_add(stock_area.saturating_sub(used_area));
    }
    // If no sheets had pieces, treat utilisation as 0.
    if min_sheet_util_bps == u64::MAX {
        min_sheet_util_bps = 0;
    }

    // Spread metric: sum of squared deviations of per-sheet util from the mean.
    // Integer-clean (no sqrt) and monotonically related to stddev, so it's a
    // valid tie-breaker: higher = more lumpy layout.
    let n = sheet_utils_bps.len() as u64;
    let sum: u64 = sheet_utils_bps.iter().copied().sum();
    let mean = if n > 0 { sum / n } else { 0 };
    let sum_sq_diff: u64 = sheet_utils_bps
        .iter()
        .map(|&x| {
            let diff = if x > mean { x - mean } else { mean - x };
            diff.saturating_mul(diff)
        })
        .sum();

    Candidate {
        used_stock_count: solution.stock_pieces.len() as u32,
        total_waste_area_units: total_waste_area,
        total_bbox_area_units: total_bbox_area,
        total_bbox_void_area_units: total_bbox_void_area,
        total_piece_perimeter_units: total_piece_perimeter,
        min_sheet_util_bps,
        max_edge_gap_units,
        sheet_util_sum_sq_diff_bps2: sum_sq_diff,
        corner_free_area_units,
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

    // Tie-break for same primary objective:
    // 1. V9: prefer larger consolidated corner remnant (dense-first goal).
    //    Concentrating the fixed total waste into one large corner rectangle
    //    both maximises utilisation of the leading sheets and produces a
    //    business-reusable offcut, matching the logs/perfect reference shape.
    if candidate.corner_free_area_units != best.corner_free_area_units {
        if candidate.corner_free_area_units > best.corner_free_area_units {
            return CandidateCompare::Better;
        }
        return CandidateCompare::WorseByTieCornerFree;
    }
    // 2. Prefer higher minimum per-sheet utilisation among otherwise equal
    //    candidates (avoids degenerate near-empty sheets).
    if candidate.min_sheet_util_bps != best.min_sheet_util_bps {
        if candidate.min_sheet_util_bps > best.min_sheet_util_bps {
            return CandidateCompare::Better;
        }
        return CandidateCompare::WorseByTieMinUtil;
    }
    // NOTE (V9): max_edge_gap and util-spread tie-breakers were removed from
    // the comparison — both reward smearing the waste evenly across sheets /
    // around the perimeter, which directly contradicts corner consolidation.
    // The metrics are still computed for telemetry.
    // 3. Prefer denser occupied bounding region to reduce internal voids.
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
    winner: &Candidate,
) -> CandidateSelectionTelemetry {
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
        candidates_rejected_tie_min_util: counters.candidates_rejected_tie_min_util,
        candidates_rejected_tie_corner_free: counters.candidates_rejected_tie_corner_free,
        candidates_rejected_equal: counters.candidates_rejected_equal,
        winner_used_stock_count: winner.used_stock_count,
        winner_waste_area_mm2: from_area_units(winner.total_waste_area_units),
        winner_bbox_void_area_mm2: from_area_units(winner.total_bbox_void_area_units),
        winner_bbox_area_mm2: from_area_units(winner.total_bbox_area_units),
        winner_piece_perimeter_mm: from_linear_units_u128(winner.total_piece_perimeter_units),
        winner_min_sheet_util_pct: winner.min_sheet_util_bps as f64 / 100.0,
        winner_max_edge_gap_mm: winner.max_edge_gap_units as f64 / SCALE,
        // Display stddev in % units; the stored metric is n*variance (bps^2),
        // so stddev_pct = sqrt(metric / n) / 100.  We compute a safe integer
        // approximation for the telemetry only — the tie-break itself uses the
        // raw sum-squared-diff which is monotonically related to stddev.
        winner_sheet_util_spread_pct: {
            let n = winner.used_stock_count.max(1) as f64;
            (winner.sheet_util_sum_sq_diff_bps2 as f64 / n).sqrt() / 100.0
        },
        winner_corner_free_area_mm2: from_area_units(winner.corner_free_area_units),
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

#[derive(Debug, Clone, Copy)]
struct GroupShiftOptions {
    enabled: bool,
    min_shift_mm: f64,
    max_passes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShiftDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone)]
struct GroupShiftMove {
    solution_index: usize,
    placement_indices: Vec<usize>,
    direction: ShiftDirection,
    shift_mm: f64,
    corridor_closed_area_mm2: f64,
    selected_area_mm2: f64,
}

fn group_shift_options(params: &Params) -> Option<GroupShiftOptions> {
    let group_shift = params.group_shift.as_ref()?;
    let enabled = group_shift.enabled.unwrap_or(true);
    if !enabled {
        return None;
    }
    Some(GroupShiftOptions {
        enabled,
        min_shift_mm: group_shift.min_shift_mm.unwrap_or(5.0),
        max_passes: group_shift.max_passes.unwrap_or(4),
    })
}

fn apply_group_shift_postprocess(
    solutions: &mut [Solution],
    gap_mm: f64,
    options: GroupShiftOptions,
) -> GroupShiftTelemetry {
    let started = Instant::now();
    let mut telemetry = GroupShiftTelemetry {
        enabled: options.enabled,
        time_ms: 0,
        moves_applied: 0,
        parts_moved: 0,
        passes_run: 0,
        corridor_closed_area_mm2: 0.0,
        corridor_opportunity_before_mm2: 0.0,
        corridor_opportunity_after_mm2: 0.0,
        corridor_opportunity_delta_mm2: 0.0,
        max_shift_mm: 0.0,
    };
    if !options.enabled {
        telemetry.time_ms = started.elapsed().as_millis() as u64;
        return telemetry;
    }

    telemetry.corridor_opportunity_before_mm2 =
        group_shift_opportunity_score(solutions, gap_mm, options.min_shift_mm);

    for pass_idx in 0..options.max_passes {
        telemetry.passes_run = pass_idx + 1;
        let best_move = solutions
            .iter()
            .enumerate()
            .filter_map(|(solution_index, solution)| {
                best_group_shift_for_solution(
                    solution,
                    solution_index,
                    gap_mm,
                    options.min_shift_mm,
                )
            })
            .max_by(|a, b| compare_group_shift_moves(a, b));

        let Some(best_move) = best_move else {
            break;
        };
        apply_group_shift_move(solutions, &best_move);
        telemetry.moves_applied += 1;
        telemetry.parts_moved += best_move.placement_indices.len() as u32;
        telemetry.corridor_closed_area_mm2 += best_move.corridor_closed_area_mm2;
        telemetry.max_shift_mm = telemetry.max_shift_mm.max(best_move.shift_mm);
    }

    telemetry.corridor_opportunity_after_mm2 = normalize_group_shift_metric(
        group_shift_opportunity_score(solutions, gap_mm, options.min_shift_mm),
    );
    telemetry.corridor_opportunity_delta_mm2 = normalize_group_shift_metric(
        telemetry.corridor_opportunity_before_mm2 - telemetry.corridor_opportunity_after_mm2,
    );
    telemetry.time_ms = started.elapsed().as_millis() as u64;
    telemetry
}

fn normalize_group_shift_metric(value: f64) -> f64 {
    if value.abs() <= GROUP_SHIFT_EPS {
        0.0
    } else {
        value
    }
}

fn group_shift_opportunity_score(solutions: &[Solution], gap_mm: f64, min_shift_mm: f64) -> f64 {
    solutions
        .iter()
        .enumerate()
        .filter_map(|(solution_index, solution)| {
            best_group_shift_for_solution(solution, solution_index, gap_mm, min_shift_mm)
        })
        .map(|group_shift_move| group_shift_move.corridor_closed_area_mm2)
        .sum()
}

fn best_group_shift_for_solution(
    solution: &Solution,
    solution_index: usize,
    gap_mm: f64,
    min_shift_mm: f64,
) -> Option<GroupShiftMove> {
    if solution.placements.len() < 2 {
        return None;
    }

    let components = placement_components(solution, gap_mm);
    let anchor_component_idx = largest_component_index(solution, &components);
    let anchor_mask = (components.len() > 1).then(|| {
        let mut mask = vec![false; solution.placements.len()];
        for idx in &components[anchor_component_idx] {
            mask[*idx] = true;
        }
        mask
    });

    let mut best: Option<GroupShiftMove> = None;
    for cut in unique_edges(solution.placements.iter().map(|placement| placement.x_mm)) {
        let selected = selected_indices(solution, |placement| {
            placement.x_mm >= cut - GROUP_SHIFT_EPS
        });
        consider_group_shift_candidate(
            solution,
            solution_index,
            gap_mm,
            min_shift_mm,
            ShiftDirection::Left,
            selected,
            anchor_mask.as_deref(),
            &mut best,
        );
    }
    for cut in unique_edges(
        solution
            .placements
            .iter()
            .map(|placement| placement.x_mm + placement.width_mm),
    ) {
        let selected = selected_indices(solution, |placement| {
            placement.x_mm + placement.width_mm <= cut + GROUP_SHIFT_EPS
        });
        consider_group_shift_candidate(
            solution,
            solution_index,
            gap_mm,
            min_shift_mm,
            ShiftDirection::Right,
            selected,
            anchor_mask.as_deref(),
            &mut best,
        );
    }
    for cut in unique_edges(solution.placements.iter().map(|placement| placement.y_mm)) {
        let selected = selected_indices(solution, |placement| {
            placement.y_mm >= cut - GROUP_SHIFT_EPS
        });
        consider_group_shift_candidate(
            solution,
            solution_index,
            gap_mm,
            min_shift_mm,
            ShiftDirection::Up,
            selected,
            anchor_mask.as_deref(),
            &mut best,
        );
    }
    for cut in unique_edges(
        solution
            .placements
            .iter()
            .map(|placement| placement.y_mm + placement.height_mm),
    ) {
        let selected = selected_indices(solution, |placement| {
            placement.y_mm + placement.height_mm <= cut + GROUP_SHIFT_EPS
        });
        consider_group_shift_candidate(
            solution,
            solution_index,
            gap_mm,
            min_shift_mm,
            ShiftDirection::Down,
            selected,
            anchor_mask.as_deref(),
            &mut best,
        );
    }

    if let Some(anchor_mask) = anchor_mask.as_deref() {
        for (component_idx, component) in components.iter().enumerate() {
            if component_idx == anchor_component_idx {
                continue;
            }
            for direction in component_anchor_shift_directions(
                solution,
                component,
                &components[anchor_component_idx],
            ) {
                consider_group_shift_candidate(
                    solution,
                    solution_index,
                    gap_mm,
                    min_shift_mm,
                    direction,
                    component.clone(),
                    Some(anchor_mask),
                    &mut best,
                );
            }
        }
    }

    best
}

fn selected_indices<F>(solution: &Solution, mut predicate: F) -> Vec<usize>
where
    F: FnMut(&Placement) -> bool,
{
    solution
        .placements
        .iter()
        .enumerate()
        .filter_map(|(idx, placement)| predicate(placement).then_some(idx))
        .collect()
}

fn unique_edges<I>(edges: I) -> Vec<f64>
where
    I: IntoIterator<Item = f64>,
{
    let mut out: Vec<f64> = edges.into_iter().filter(|edge| edge.is_finite()).collect();
    out.sort_by(|a, b| a.total_cmp(b));
    out.dedup_by(|a, b| (*a - *b).abs() <= GROUP_SHIFT_EPS);
    out
}

fn placement_components(solution: &Solution, gap_mm: f64) -> Vec<Vec<usize>> {
    let n = solution.placements.len();
    let mut visited = vec![false; n];
    let mut components = Vec::new();
    for start in 0..n {
        if visited[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        visited[start] = true;
        while let Some(idx) = stack.pop() {
            component.push(idx);
            for next in 0..n {
                if visited[next] {
                    continue;
                }
                if placements_near_component_gap(
                    &solution.placements[idx],
                    &solution.placements[next],
                    gap_mm,
                ) {
                    visited[next] = true;
                    stack.push(next);
                }
            }
        }
        component.sort_unstable();
        components.push(component);
    }
    components
}

fn largest_component_index(solution: &Solution, components: &[Vec<usize>]) -> usize {
    components
        .iter()
        .enumerate()
        .max_by(|(idx_a, a), (idx_b, b)| {
            component_area(solution, a)
                .total_cmp(&component_area(solution, b))
                .then_with(|| idx_b.cmp(idx_a))
        })
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn component_area(solution: &Solution, component: &[usize]) -> f64 {
    component
        .iter()
        .map(|idx| placement_area(&solution.placements[*idx]))
        .sum()
}

fn placements_near_component_gap(a: &Placement, b: &Placement, gap_mm: f64) -> bool {
    let horizontal_gap = axis_gap(a.x_mm, a.x_mm + a.width_mm, b.x_mm, b.x_mm + b.width_mm);
    let vertical_gap = axis_gap(a.y_mm, a.y_mm + a.height_mm, b.y_mm, b.y_mm + b.height_mm);
    (horizontal_gap <= gap_mm + GROUP_SHIFT_EPS && vertical_gap <= GROUP_SHIFT_EPS)
        || (vertical_gap <= gap_mm + GROUP_SHIFT_EPS && horizontal_gap <= GROUP_SHIFT_EPS)
}

fn axis_gap(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> f64 {
    if a_max < b_min {
        b_min - a_max
    } else if b_max < a_min {
        a_min - b_max
    } else {
        0.0
    }
}

fn component_anchor_shift_directions(
    solution: &Solution,
    component: &[usize],
    anchor_component: &[usize],
) -> Vec<ShiftDirection> {
    let (component_cx, component_cy) = component_center(solution, component);
    let (anchor_cx, anchor_cy) = component_center(solution, anchor_component);
    let dx = anchor_cx - component_cx;
    let dy = anchor_cy - component_cy;
    let horizontal = if dx < -GROUP_SHIFT_EPS {
        Some(ShiftDirection::Left)
    } else if dx > GROUP_SHIFT_EPS {
        Some(ShiftDirection::Right)
    } else {
        None
    };
    let vertical = if dy < -GROUP_SHIFT_EPS {
        Some(ShiftDirection::Up)
    } else if dy > GROUP_SHIFT_EPS {
        Some(ShiftDirection::Down)
    } else {
        None
    };

    let mut directions = Vec::with_capacity(2);
    if dx.abs() >= dy.abs() {
        if let Some(direction) = horizontal {
            directions.push(direction);
        }
        if let Some(direction) = vertical {
            directions.push(direction);
        }
    } else {
        if let Some(direction) = vertical {
            directions.push(direction);
        }
        if let Some(direction) = horizontal {
            directions.push(direction);
        }
    }
    directions
}

fn component_center(solution: &Solution, component: &[usize]) -> (f64, f64) {
    let (min_x, min_y, max_x, max_y) = component.iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |(min_x, min_y, max_x, max_y), idx| {
            let placement = &solution.placements[*idx];
            (
                min_x.min(placement.x_mm),
                min_y.min(placement.y_mm),
                max_x.max(placement.x_mm + placement.width_mm),
                max_y.max(placement.y_mm + placement.height_mm),
            )
        },
    );
    ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0)
}

fn consider_group_shift_candidate(
    solution: &Solution,
    solution_index: usize,
    gap_mm: f64,
    min_shift_mm: f64,
    direction: ShiftDirection,
    selected: Vec<usize>,
    anchor_mask: Option<&[bool]>,
    best: &mut Option<GroupShiftMove>,
) {
    if anchor_mask.is_some_and(|mask| selected.iter().any(|idx| mask[*idx])) {
        return;
    }
    if let Some(candidate) = evaluate_group_shift_candidate(
        solution,
        solution_index,
        gap_mm,
        min_shift_mm,
        direction,
        selected,
    ) {
        let replace = best
            .as_ref()
            .map(|current| compare_group_shift_moves(&candidate, current).is_gt())
            .unwrap_or(true);
        if replace {
            *best = Some(candidate);
        }
    }
}

fn evaluate_group_shift_candidate(
    solution: &Solution,
    solution_index: usize,
    gap_mm: f64,
    min_shift_mm: f64,
    direction: ShiftDirection,
    selected: Vec<usize>,
) -> Option<GroupShiftMove> {
    let n = solution.placements.len();
    if selected.is_empty() || selected.len() == n {
        return None;
    }

    let mut selected_mask = vec![false; n];
    for idx in &selected {
        selected_mask[*idx] = true;
    }

    let total_area = solution.placements.iter().map(placement_area).sum::<f64>();
    let selected_area = selected
        .iter()
        .map(|idx| placement_area(&solution.placements[*idx]))
        .sum::<f64>();
    let anchor_area = total_area - selected_area;
    if selected_area > anchor_area + GROUP_SHIFT_EPS {
        return None;
    }

    let usable_width = usable_width_mm(solution);
    let usable_height = usable_height_mm(solution);
    let mut shift_limit = match direction {
        ShiftDirection::Left => selected
            .iter()
            .map(|idx| solution.placements[*idx].x_mm)
            .fold(f64::INFINITY, f64::min),
        ShiftDirection::Right => selected
            .iter()
            .map(|idx| {
                usable_width - (solution.placements[*idx].x_mm + solution.placements[*idx].width_mm)
            })
            .fold(f64::INFINITY, f64::min),
        ShiftDirection::Up => selected
            .iter()
            .map(|idx| solution.placements[*idx].y_mm)
            .fold(f64::INFINITY, f64::min),
        ShiftDirection::Down => selected
            .iter()
            .map(|idx| {
                usable_height
                    - (solution.placements[*idx].y_mm + solution.placements[*idx].height_mm)
            })
            .fold(f64::INFINITY, f64::min),
    };
    if !shift_limit.is_finite() || shift_limit <= GROUP_SHIFT_EPS {
        return None;
    }

    let mut has_anchor_obstacle = false;
    for selected_idx in &selected {
        let selected_placement = &solution.placements[*selected_idx];
        for (other_idx, other_placement) in solution.placements.iter().enumerate() {
            if selected_mask[other_idx] {
                continue;
            }
            if let Some(limit) =
                obstacle_shift_limit(selected_placement, other_placement, direction, gap_mm)
            {
                has_anchor_obstacle = true;
                shift_limit = shift_limit.min(limit);
            }
        }
    }

    if !has_anchor_obstacle || shift_limit + GROUP_SHIFT_EPS < min_shift_mm {
        return None;
    }

    let shift_mm = shift_limit.max(0.0);
    let span_mm = selected_group_perpendicular_span(solution, &selected, direction)?;
    let corridor_closed_area_mm2 = shift_mm * span_mm;
    if corridor_closed_area_mm2 <= GROUP_SHIFT_EPS {
        return None;
    }

    Some(GroupShiftMove {
        solution_index,
        placement_indices: selected,
        direction,
        shift_mm,
        corridor_closed_area_mm2,
        selected_area_mm2: selected_area,
    })
}

fn obstacle_shift_limit(
    selected: &Placement,
    obstacle: &Placement,
    direction: ShiftDirection,
    gap_mm: f64,
) -> Option<f64> {
    match direction {
        ShiftDirection::Left => {
            if ranges_overlap(
                selected.y_mm,
                selected.y_mm + selected.height_mm,
                obstacle.y_mm,
                obstacle.y_mm + obstacle.height_mm,
            ) {
                let obstacle_right = obstacle.x_mm + obstacle.width_mm + gap_mm;
                (obstacle_right <= selected.x_mm + GROUP_SHIFT_EPS)
                    .then_some(selected.x_mm - obstacle_right)
            } else {
                None
            }
        }
        ShiftDirection::Right => {
            if ranges_overlap(
                selected.y_mm,
                selected.y_mm + selected.height_mm,
                obstacle.y_mm,
                obstacle.y_mm + obstacle.height_mm,
            ) {
                let selected_right = selected.x_mm + selected.width_mm + gap_mm;
                (selected_right <= obstacle.x_mm + GROUP_SHIFT_EPS)
                    .then_some(obstacle.x_mm - selected_right)
            } else {
                None
            }
        }
        ShiftDirection::Up => {
            if ranges_overlap(
                selected.x_mm,
                selected.x_mm + selected.width_mm,
                obstacle.x_mm,
                obstacle.x_mm + obstacle.width_mm,
            ) {
                let obstacle_bottom = obstacle.y_mm + obstacle.height_mm + gap_mm;
                (obstacle_bottom <= selected.y_mm + GROUP_SHIFT_EPS)
                    .then_some(selected.y_mm - obstacle_bottom)
            } else {
                None
            }
        }
        ShiftDirection::Down => {
            if ranges_overlap(
                selected.x_mm,
                selected.x_mm + selected.width_mm,
                obstacle.x_mm,
                obstacle.x_mm + obstacle.width_mm,
            ) {
                let selected_bottom = selected.y_mm + selected.height_mm + gap_mm;
                (selected_bottom <= obstacle.y_mm + GROUP_SHIFT_EPS)
                    .then_some(obstacle.y_mm - selected_bottom)
            } else {
                None
            }
        }
    }
}

fn selected_group_perpendicular_span(
    solution: &Solution,
    selected: &[usize],
    direction: ShiftDirection,
) -> Option<f64> {
    let (min_a, max_b) =
        selected
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(min_a, max_b), idx| {
                let placement = &solution.placements[*idx];
                match direction {
                    ShiftDirection::Left | ShiftDirection::Right => (
                        min_a.min(placement.y_mm),
                        max_b.max(placement.y_mm + placement.height_mm),
                    ),
                    ShiftDirection::Up | ShiftDirection::Down => (
                        min_a.min(placement.x_mm),
                        max_b.max(placement.x_mm + placement.width_mm),
                    ),
                }
            });
    let span = max_b - min_a;
    (span > GROUP_SHIFT_EPS).then_some(span)
}

fn apply_group_shift_move(solutions: &mut [Solution], group_shift_move: &GroupShiftMove) {
    let solution = &mut solutions[group_shift_move.solution_index];
    for idx in &group_shift_move.placement_indices {
        let placement = &mut solution.placements[*idx];
        match group_shift_move.direction {
            ShiftDirection::Left => placement.x_mm -= group_shift_move.shift_mm,
            ShiftDirection::Right => placement.x_mm += group_shift_move.shift_mm,
            ShiftDirection::Up => placement.y_mm -= group_shift_move.shift_mm,
            ShiftDirection::Down => placement.y_mm += group_shift_move.shift_mm,
        }
    }
}

fn compare_group_shift_moves(a: &GroupShiftMove, b: &GroupShiftMove) -> std::cmp::Ordering {
    a.corridor_closed_area_mm2
        .total_cmp(&b.corridor_closed_area_mm2)
        .then_with(|| a.shift_mm.total_cmp(&b.shift_mm))
        .then_with(|| b.selected_area_mm2.total_cmp(&a.selected_area_mm2))
        .then_with(|| b.placement_indices.len().cmp(&a.placement_indices.len()))
}

fn usable_width_mm(solution: &Solution) -> f64 {
    (solution.width_mm - solution.trim_mm.left - solution.trim_mm.right).max(0.0)
}

fn usable_height_mm(solution: &Solution) -> f64 {
    (solution.height_mm - solution.trim_mm.top - solution.trim_mm.bottom).max(0.0)
}

fn placement_area(placement: &Placement) -> f64 {
    placement.width_mm * placement.height_mm
}

fn ranges_overlap(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> bool {
    a_min < b_max - GROUP_SHIFT_EPS && a_max > b_min + GROUP_SHIFT_EPS
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

fn build_svg(
    solutions: &[Solution],
    unplaced_items: &[UnplacedItem],
    trim: &Trim,
    kerf_gap_mm: f64,
) -> String {
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

        // Draw kerf-fill lines between adjacent pieces to look like thin cut lines
        if kerf_gap_mm > 0.0 {
            const KERF_FILL: &str = "#8B0000";
            const KERF_STROKE: &str = "#3a0000";
            let tolerance = 1.5;
            let n = solution.placements.len();
            for i in 0..n {
                let a = &solution.placements[i];
                for j in (i + 1)..n {
                    let b = &solution.placements[j];
                    // Horizontal adjacency (left-right)
                    let h_gap = b.x_mm - (a.x_mm + a.width_mm);
                    if h_gap > kerf_gap_mm - tolerance && h_gap < kerf_gap_mm + tolerance {
                        let y_top = a.y_mm.max(b.y_mm);
                        let y_bot = (a.y_mm + a.height_mm).min(b.y_mm + b.height_mm);
                        if y_bot > y_top {
                            svg.push_str(&format!(
                                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"0.3\"/>",
                                fmt_mm(a.x_mm + a.width_mm),
                                fmt_mm(y_top + y_offset),
                                fmt_mm(h_gap),
                                fmt_mm(y_bot - y_top),
                                KERF_FILL,
                                KERF_STROKE
                            ));
                        }
                    }
                    let h_gap_rev = a.x_mm - (b.x_mm + b.width_mm);
                    if h_gap_rev > kerf_gap_mm - tolerance && h_gap_rev < kerf_gap_mm + tolerance {
                        let y_top = a.y_mm.max(b.y_mm);
                        let y_bot = (a.y_mm + a.height_mm).min(b.y_mm + b.height_mm);
                        if y_bot > y_top {
                            svg.push_str(&format!(
                                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"0.3\"/>",
                                fmt_mm(b.x_mm + b.width_mm),
                                fmt_mm(y_top + y_offset),
                                fmt_mm(h_gap_rev),
                                fmt_mm(y_bot - y_top),
                                KERF_FILL,
                                KERF_STROKE
                            ));
                        }
                    }
                    // Vertical adjacency (top-bottom)
                    let v_gap = b.y_mm - (a.y_mm + a.height_mm);
                    if v_gap > kerf_gap_mm - tolerance && v_gap < kerf_gap_mm + tolerance {
                        let x_left = a.x_mm.max(b.x_mm);
                        let x_right = (a.x_mm + a.width_mm).min(b.x_mm + b.width_mm);
                        if x_right > x_left {
                            svg.push_str(&format!(
                                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"0.3\"/>",
                                fmt_mm(x_left),
                                fmt_mm(a.y_mm + a.height_mm + y_offset),
                                fmt_mm(x_right - x_left),
                                fmt_mm(v_gap),
                                KERF_FILL,
                                KERF_STROKE
                            ));
                        }
                    }
                    let v_gap_rev = a.y_mm - (b.y_mm + b.height_mm);
                    if v_gap_rev > kerf_gap_mm - tolerance && v_gap_rev < kerf_gap_mm + tolerance {
                        let x_left = a.x_mm.max(b.x_mm);
                        let x_right = (a.x_mm + a.width_mm).min(b.x_mm + b.width_mm);
                        if x_right > x_left {
                            svg.push_str(&format!(
                                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"0.3\"/>",
                                fmt_mm(x_left),
                                fmt_mm(b.y_mm + b.height_mm + y_offset),
                                fmt_mm(x_right - x_left),
                                fmt_mm(v_gap_rev),
                                KERF_FILL,
                                KERF_STROKE
                            ));
                        }
                    }
                }
            }
        }

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

fn build_group_shift_diff_svg(
    before_solutions: &[Solution],
    after_solutions: &[Solution],
    trim: &Trim,
) -> String {
    const SHEET_GAP: f64 = 50.0;

    let mut max_width = 0.0_f64;
    let mut total_height = 0.0_f64;
    for (i, solution) in before_solutions.iter().enumerate() {
        max_width = max_width.max(solution.width_mm);
        total_height += solution.height_mm;
        if i > 0 {
            total_height += SHEET_GAP;
        }
    }
    if max_width == 0.0 {
        max_width = 500.0;
    }
    if total_height == 0.0 {
        total_height = 200.0;
    }

    let min_x = -trim.left;
    let min_y = -trim.top;
    let mut svg = String::new();
    svg.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\" ");
    svg.push_str(&format!(
        "viewBox=\"{} {} {} {}\">",
        fmt_mm(min_x),
        fmt_mm(min_y),
        fmt_mm(max_width),
        fmt_mm(total_height)
    ));
    svg.push_str("<title>Group shift diff: red before, green after</title>");

    let mut y_offset = 0.0_f64;
    for (sheet_idx, before_solution) in before_solutions.iter().enumerate() {
        let sheet_x = -trim.left;
        let sheet_y = -trim.top + y_offset;
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#f5f5f5\" stroke=\"#333\" stroke-width=\"1\"/>",
            fmt_mm(sheet_x),
            fmt_mm(sheet_y),
            fmt_mm(before_solution.width_mm),
            fmt_mm(before_solution.height_mm)
        ));

        let Some(after_solution) = after_solutions.get(sheet_idx) else {
            y_offset += before_solution.height_mm + SHEET_GAP;
            continue;
        };
        let after_by_key: HashMap<(&str, u32), &Placement> = after_solution
            .placements
            .iter()
            .map(|placement| ((placement.item_id.as_str(), placement.instance), placement))
            .collect();

        for before in &before_solution.placements {
            let Some(after) = after_by_key.get(&(before.item_id.as_str(), before.instance)) else {
                continue;
            };
            if !placement_geometry_changed(before, after) {
                continue;
            }
            let before_x = before.x_mm;
            let before_y = before.y_mm + y_offset;
            let after_x = after.x_mm;
            let after_y = after.y_mm + y_offset;
            let before_cx = before_x + before.width_mm / 2.0;
            let before_cy = before_y + before.height_mm / 2.0;
            let after_cx = after_x + after.width_mm / 2.0;
            let after_cy = after_y + after.height_mm / 2.0;

            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#ff4d4d\" fill-opacity=\"0.10\" stroke=\"#c52222\" stroke-width=\"6\" stroke-dasharray=\"24 18\"/>",
                fmt_mm(before_x),
                fmt_mm(before_y),
                fmt_mm(before.width_mm),
                fmt_mm(before.height_mm)
            ));
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#2fbd5a\" fill-opacity=\"0.12\" stroke=\"#12833a\" stroke-width=\"6\"/>",
                fmt_mm(after_x),
                fmt_mm(after_y),
                fmt_mm(after.width_mm),
                fmt_mm(after.height_mm)
            ));
            svg.push_str(&format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#ff9800\" stroke-width=\"8\" stroke-linecap=\"round\"/>",
                fmt_mm(before_cx),
                fmt_mm(before_cy),
                fmt_mm(after_cx),
                fmt_mm(after_cy)
            ));
        }

        y_offset += before_solution.height_mm + SHEET_GAP;
    }

    svg.push_str("</svg>");
    svg
}

fn placement_geometry_changed(before: &Placement, after: &Placement) -> bool {
    (before.x_mm - after.x_mm).abs() > GROUP_SHIFT_EPS
        || (before.y_mm - after.y_mm).abs() > GROUP_SHIFT_EPS
        || (before.width_mm - after.width_mm).abs() > GROUP_SHIFT_EPS
        || (before.height_mm - after.height_mm).abs() > GROUP_SHIFT_EPS
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

    fn test_solution(placements: Vec<Placement>) -> Solution {
        Solution {
            stock_id: "sheet".to_string(),
            index: 0,
            width_mm: 300.0,
            height_mm: 200.0,
            trim_mm: Trim {
                left: 0.0,
                right: 0.0,
                top: 0.0,
                bottom: 0.0,
            },
            placements,
        }
    }

    fn test_placement(id: &str, x_mm: f64, y_mm: f64, width_mm: f64, height_mm: f64) -> Placement {
        Placement {
            item_id: id.to_string(),
            instance: 1,
            x_mm,
            y_mm,
            width_mm,
            height_mm,
            rotated: false,
            pattern_direction: PatternDirection::None,
        }
    }

    fn test_profile_pool_candidate(
        waste_regions: u32,
        lead_util_pct: f64,
        group_shift_opportunity_after_mm2: f64,
        group_shift_opportunity_delta_mm2: f64,
    ) -> ProfilePoolCandidate {
        ProfilePoolCandidate {
            response: OptimizeResponse {
                status: "ok",
                summary: Summary {
                    objective: Objective::MinWaste,
                    used_stock_count: 4,
                    total_waste_area_mm2: 0.0,
                    waste_percent: 0.0,
                    time_ms: 0,
                    restarts_used: 1,
                    restarts_requested: 1,
                    used_seed: 1,
                    layout_mode: LayoutMode::Guillotine,
                    timeout_reason: None,
                    restart_policy: None,
                    portfolio: None,
                    beam: None,
                    alns: None,
                    candidate_selection: None,
                    profile_pool: None,
                    retry: None,
                    partition: None,
                    group_shift: None,
                },
                solutions: Vec::new(),
                unplaced_items: Vec::new(),
                artifacts: Artifacts {
                    svg: None,
                    group_shift_before_svg: None,
                    group_shift_diff_svg: None,
                },
            },
            seed: 1,
            zone_penalty: 0.3,
            is_rescue: false,
            visual_waste_regions: waste_regions,
            waste_regions,
            lead_util_pct,
            max_corner_mm2: 100_000.0,
            group_shift_opportunity_after_mm2,
            group_shift_opportunity_delta_mm2,
        }
    }

    #[test]
    fn profile_pool_tie_breaks_on_group_shift_residual_then_delta() {
        let cleaner = test_profile_pool_candidate(5, 94.0, 10_000.0, 40_000.0);
        let corridor_left = test_profile_pool_candidate(5, 95.0, 80_000.0, 120_000.0);

        assert!(
            profile_pool_candidate_better(&cleaner, &corridor_left),
            "same sheets/zones should prefer lower residual group-shift opportunity"
        );

        let same_residual_more_delta = test_profile_pool_candidate(5, 94.0, 10_000.0, 80_000.0);
        assert!(
            profile_pool_candidate_better(&same_residual_more_delta, &cleaner),
            "same residual should prefer the candidate where group_shift closed more opportunity"
        );
    }

    #[test]
    fn group_shift_moves_side_parts_as_one_group_toward_anchor_cluster() {
        let mut solutions = vec![test_solution(vec![
            test_placement("anchor_top", 0.0, 0.0, 100.0, 80.0),
            test_placement("anchor_bottom", 0.0, 90.0, 100.0, 80.0),
            test_placement("side_top", 180.0, 0.0, 50.0, 70.0),
            test_placement("side_bottom", 180.0, 80.0, 50.0, 70.0),
        ])];

        let telemetry = apply_group_shift_postprocess(
            &mut solutions,
            10.0,
            GroupShiftOptions {
                enabled: true,
                min_shift_mm: 5.0,
                max_passes: 4,
            },
        );

        assert_eq!(telemetry.moves_applied, 1);
        assert_eq!(telemetry.parts_moved, 2);
        assert_eq!(telemetry.max_shift_mm, 70.0);
        assert_eq!(telemetry.corridor_closed_area_mm2, 10_500.0);
        assert_eq!(telemetry.corridor_opportunity_before_mm2, 10_500.0);
        assert_eq!(telemetry.corridor_opportunity_after_mm2, 0.0);
        assert_eq!(telemetry.corridor_opportunity_delta_mm2, 10_500.0);
        assert!(telemetry.time_ms < 1_000);
        assert_eq!(solutions[0].placements[2].x_mm, 110.0);
        assert_eq!(solutions[0].placements[3].x_mm, 110.0);
        assert_eq!(solutions[0].placements[0].x_mm, 0.0);
        assert_eq!(solutions[0].placements[1].x_mm, 0.0);
    }

    #[test]
    fn group_shift_disabled_leaves_coordinates_unchanged() {
        let mut solutions = vec![test_solution(vec![
            test_placement("anchor", 0.0, 0.0, 100.0, 80.0),
            test_placement("side", 180.0, 0.0, 50.0, 70.0),
        ])];

        let telemetry = apply_group_shift_postprocess(
            &mut solutions,
            10.0,
            GroupShiftOptions {
                enabled: false,
                min_shift_mm: 5.0,
                max_passes: 4,
            },
        );

        assert_eq!(telemetry.moves_applied, 0);
        assert_eq!(telemetry.parts_moved, 0);
        assert_eq!(telemetry.corridor_opportunity_before_mm2, 0.0);
        assert_eq!(telemetry.corridor_opportunity_after_mm2, 0.0);
        assert_eq!(telemetry.corridor_opportunity_delta_mm2, 0.0);
        assert!(telemetry.time_ms < 1_000);
        assert_eq!(solutions[0].placements[1].x_mm, 180.0);
    }

    #[test]
    fn group_shift_moves_disconnected_component_without_dragging_same_side_obstacle() {
        let mut solutions = vec![test_solution(vec![
            test_placement("anchor", 0.0, 0.0, 100.0, 100.0),
            test_placement("side_top", 180.0, 0.0, 40.0, 40.0),
            test_placement("side_bottom", 180.0, 50.0, 40.0, 40.0),
            test_placement("same_side_obstacle", 180.0, 130.0, 150.0, 60.0),
        ])];

        let telemetry = apply_group_shift_postprocess(
            &mut solutions,
            10.0,
            GroupShiftOptions {
                enabled: true,
                min_shift_mm: 5.0,
                max_passes: 4,
            },
        );

        assert_eq!(telemetry.moves_applied, 1);
        assert_eq!(telemetry.parts_moved, 2);
        assert_eq!(solutions[0].placements[1].x_mm, 110.0);
        assert_eq!(solutions[0].placements[2].x_mm, 110.0);
        assert_eq!(solutions[0].placements[3].x_mm, 180.0);
    }

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
