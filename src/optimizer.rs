use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cut_optimizer_2d::{CutPiece, Optimizer, PatternDirection as CutPatternDirection, StockPiece};

use crate::config::AppConfig;
use crate::models::{
    Artifacts, BeamTelemetry, ErrorResponse, LayoutMode, Objective, OptimizeRequest,
    OptimizeResponse, PatternDirection, Placement, PortfolioTelemetry, Solution, Summary, Trim,
    UnplacedItem,
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

#[derive(Clone, Copy)]
enum SolveMode {
    Default,
    BeamEndpoint,
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

    let (restarts, slice_ms) = derive_restarts_and_slice(time_limit_ms, restarts_requested);

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
                portfolio: None,
                beam: None,
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
                    restarts,
                    slice_ms,
                    layout_mode,
                    used_seed,
                    time_limit_ms,
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
        portfolio: run_outcome.portfolio,
        beam: run_outcome.beam,
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
    portfolio: Option<PortfolioTelemetry>,
    beam: Option<BeamTelemetry>,
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
}

async fn run_restarts_with_budget(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    restarts: u64,
    slice_ms: u64,
    layout_mode: LayoutMode,
    base_seed: u64,
    total_budget_ms: u64,
) -> Result<RunOutcome, OptimizeError> {
    let mut best: Option<Candidate> = None;
    let mut timed_out = false;
    let mut budget_exhausted = false;
    let mut last_constraint: Option<OptimizeError> = None;
    let started_at = Instant::now();
    let mut restarts_used: u32 = 0;
    let mut no_improve_streak: u32 = 0;

    // Keep one shared copy per request and avoid allocating/cloning a full Vec on each restart.
    let stock_templates = Arc::new(prepared.stock_pieces.clone());
    let cut_templates = Arc::new(prepared.cut_pieces.clone());

    for i in 0..restarts {
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        if elapsed_ms >= total_budget_ms {
            budget_exhausted = true;
            break;
        }
        let remaining_ms = total_budget_ms - elapsed_ms;
        let this_slice_ms = remaining_ms.min(slice_ms).max(1);

        let seed = base_seed.wrapping_add(i.wrapping_mul(SEED_STRIDE));
        let mode = layout_mode;
        let stock_templates = Arc::clone(&stock_templates);
        let cut_templates = Arc::clone(&cut_templates);
        let cut_width = prepared.cut_width;

        let mut handle = tokio::task::spawn_blocking(move || {
            let mut optimizer = Optimizer::new();
            optimizer
                .set_random_seed(seed)
                .set_cut_width(cut_width)
                .add_stock_pieces(stock_templates.iter().copied())
                .add_cut_pieces(cut_templates.iter().cloned());
            match mode {
                LayoutMode::Nested => optimizer.optimize_nested(|_| {}),
                LayoutMode::Guillotine => optimizer.optimize_guillotine(|_| {}),
            }
        });

        let run = tokio::time::timeout(Duration::from_millis(this_slice_ms), &mut handle).await;
        match run {
            Ok(join_result) => match join_result {
                Ok(Ok(solution)) => {
                    restarts_used += 1;
                    if solution.fitness < 0.0 {
                        last_constraint = Some(OptimizeError::Constraint {
                            message: "no valid solution".to_string(),
                            details: None,
                        });
                        continue;
                    }
                    let candidate = build_candidate(solution);
                    best = match best {
                        None => {
                            no_improve_streak = 0;
                            Some(candidate)
                        }
                        Some(current) => {
                            if is_better(&candidate, &current, &req.params.objective) {
                                no_improve_streak = 0;
                                Some(candidate)
                            } else {
                                no_improve_streak = no_improve_streak.saturating_add(1);
                                Some(current)
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
                // Best-effort abort. The blocking task may still run to completion.
                handle.abort();
                // Do not launch more heavy tasks once one slice timed out.
                break;
            }
        }
    }

    if let Some(best) = best {
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
            portfolio: None,
            beam: None,
        });
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

    let mut best: Option<(Candidate, &'static str, u64, u32)> = None;

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
        )
        .await
        {
            Ok(outcome) => {
                candidates_completed += 1;
                let maybe_update = match &best {
                    None => true,
                    Some((current, _, _, _)) => {
                        is_better(&outcome.candidate, current, &req.params.objective)
                    }
                };
                if maybe_update {
                    best = Some((
                        outcome.candidate,
                        plan.name,
                        plan.seed,
                        outcome.restarts_used,
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

    if let Some((candidate, winner_strategy, winner_seed, winner_restarts_used)) = best {
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
        });
    }

    if nodes_timed_out > 0 || budget_exhausted {
        return Err(OptimizeError::Timeout);
    }

    Err(OptimizeError::Internal(
        "beam search could not produce a valid solution".to_string(),
    ))
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

    for stock in &solution.stock_pieces {
        let stock_area = area(stock.width, stock.length);
        let mut used_area: u128 = 0;
        for cut_piece in &stock.cut_pieces {
            used_area = used_area.saturating_add(area(cut_piece.width, cut_piece.length));
        }
        total_waste_area = total_waste_area.saturating_add(stock_area.saturating_sub(used_area));
    }

    Candidate {
        used_stock_count: solution.stock_pieces.len() as u32,
        total_waste_area_units: total_waste_area,
        solution,
    }
}

fn is_better(candidate: &Candidate, best: &Candidate, objective: &Objective) -> bool {
    is_better_stats(
        candidate.used_stock_count,
        candidate.total_waste_area_units,
        best.used_stock_count,
        best.total_waste_area_units,
        objective,
    )
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
    fn is_better_stats_prefers_fewer_sheets_for_min_sheets() {
        assert!(is_better_stats(2, 120, 3, 80, &Objective::MinSheets));
        assert!(!is_better_stats(4, 10, 3, 500, &Objective::MinSheets));
    }

    #[test]
    fn is_better_stats_prefers_lower_waste_for_min_waste() {
        assert!(is_better_stats(4, 100, 2, 120, &Objective::MinWaste));
        assert!(!is_better_stats(1, 300, 2, 200, &Objective::MinWaste));
    }
}
