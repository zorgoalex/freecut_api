use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cut_optimizer_2d::{CutPiece, Optimizer, PatternDirection as CutPatternDirection, StockPiece};

use crate::config::AppConfig;
use crate::models::{
    Artifacts, ErrorResponse, LayoutMode, Objective, OptimizeRequest, OptimizeResponse,
    PatternDirection, Placement, Solution, Summary, Trim, UnplacedItem,
};
use crate::validation::item_fits_any_stock_public;

const SCALE: f64 = 1000.0;
const MIN_SLICE_MS: u64 = 80;
const SEED_STRIDE: u64 = 1_000_003;

#[derive(Debug)]
pub enum OptimizeError {
    Timeout,
    Constraint { message: String, details: Option<serde_json::Value> },
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
            OptimizeError::Constraint { message, details } => {
                error_response(StatusCode::UNPROCESSABLE_ENTITY, "CONSTRAINT_ERROR", &message, details)
            }
            OptimizeError::Internal(message) => {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL", &message, None)
            }
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
    stock_map: HashMap<(usize, usize), StockInfo>,
    trim: Trim,
    cut_width: usize,
    /// Items that don't fit any stock sheet (oversized)
    oversized_items: Vec<UnplacedItem>,
}

struct Candidate {
    solution: cut_optimizer_2d::Solution,
    used_stock_count: u32,
    total_waste_area_units: u128,
    total_stock_area_units: u128,
}

pub async fn optimize_request(
    req: OptimizeRequest,
    config: &AppConfig,
) -> Result<OptimizeResponse, OptimizeError> {
    let layout_mode = req.params.layout_mode.unwrap_or(LayoutMode::Nested);
    let used_seed = req.params.seed.unwrap_or_else(generate_seed);
    let prepared = prepare_input(&req)?;

    let time_limit_ms = req
        .params
        .time_limit_ms
        .unwrap_or(config.default_time_limit_ms);
    let restarts = req
        .params
        .restarts
        .unwrap_or(config.default_restarts)
        .max(1);

    let mut restarts = u64::from(restarts);
    let mut slice_ms = time_limit_ms / restarts;
    if slice_ms < MIN_SLICE_MS {
        restarts = (time_limit_ms / MIN_SLICE_MS).max(1);
        slice_ms = time_limit_ms / restarts;
    }

    let start = Instant::now();

    // Handle case where all items are oversized (nothing to optimize)
    if prepared.cut_pieces.is_empty() {
        let time_ms = start.elapsed().as_millis() as u64;
        let svg = build_svg(&[], &prepared.oversized_items, &prepared.trim);
        return Ok(OptimizeResponse {
            status: "ok",
            summary: Summary {
                objective: req.params.objective,
                used_stock_count: 0,
                total_waste_area_mm2: 0.0,
                waste_percent: 0.0,
                time_ms,
                restarts_used: 0,
                used_seed,
                layout_mode,
            },
            solutions: vec![],
            unplaced_items: prepared.oversized_items,
            artifacts: Artifacts { svg },
        });
    }

    let overall_limit = time_limit_ms.saturating_add(1000);

    let candidate = tokio::time::timeout(Duration::from_millis(overall_limit), async {
        run_restarts(&req, &prepared, restarts, slice_ms, layout_mode, used_seed).await
    })
    .await
    .map_err(|_| OptimizeError::Timeout)??;

    let time_ms = start.elapsed().as_millis() as u64;

    let all_solutions = build_solutions(&candidate.solution, &prepared);

    // Apply qty limits and collect unplaced items
    let (solutions, mut unplaced_items) = apply_qty_limits(all_solutions, &prepared);

    // Merge oversized items (items that didn't fit any stock)
    unplaced_items.extend(prepared.oversized_items.clone());

    // Recalculate stats for kept solutions only
    let (used_stock_count, total_stock_area, total_waste_area) = calculate_solution_stats(&solutions);

    let summary = Summary {
        objective: req.params.objective,
        used_stock_count,
        total_waste_area_mm2: total_waste_area,
        waste_percent: if total_stock_area > 0.0 { 100.0 * total_waste_area / total_stock_area } else { 0.0 },
        time_ms,
        restarts_used: restarts as u32,
        used_seed,
        layout_mode,
    };

    let svg = build_svg(&solutions, &unplaced_items, &prepared.trim);

    Ok(OptimizeResponse {
        status: "ok",
        summary,
        solutions,
        unplaced_items,
        artifacts: Artifacts { svg },
    })
}

async fn run_restarts(
    req: &OptimizeRequest,
    prepared: &PreparedInput,
    restarts: u64,
    slice_ms: u64,
    layout_mode: LayoutMode,
    base_seed: u64,
) -> Result<Candidate, OptimizeError> {
    let mut best: Option<Candidate> = None;
    let mut timed_out = false;
    let mut last_constraint: Option<OptimizeError> = None;

    for i in 0..restarts {
        let seed = base_seed.wrapping_add(i.wrapping_mul(SEED_STRIDE));
        let mode = layout_mode;
        let stock_pieces = prepared.stock_pieces.clone();
        let cut_pieces = prepared.cut_pieces.clone();
        let cut_width = prepared.cut_width;

        let mut handle = tokio::task::spawn_blocking(move || {
            let mut optimizer = Optimizer::new();
            optimizer
                .set_random_seed(seed)
                .set_cut_width(cut_width)
                .add_stock_pieces(stock_pieces)
                .add_cut_pieces(cut_pieces);
            match mode {
                LayoutMode::Nested => optimizer.optimize_nested(|_| {}),
                LayoutMode::Guillotine => optimizer.optimize_guillotine(|_| {}),
            }
        });

        let run = tokio::time::timeout(Duration::from_millis(slice_ms), &mut handle).await;
        match run {
            Ok(join_result) => match join_result {
                Ok(Ok(solution)) => {
                    if solution.fitness < 0.0 {
                        last_constraint = Some(OptimizeError::Constraint {
                            message: "no valid solution".to_string(),
                            details: None,
                        });
                        continue;
                    }
                    let candidate = build_candidate(solution);
                    best = match best {
                        None => Some(candidate),
                        Some(current) => {
                            if is_better(&candidate, &current, &req.params.objective) {
                                Some(candidate)
                            } else {
                                Some(current)
                            }
                        }
                    };
                }
                Ok(Err(err)) => {
                    last_constraint = Some(map_optimizer_error(err, prepared));
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
            }
        }
    }

    if let Some(best) = best {
        return Ok(best);
    }

    if timed_out {
        return Err(OptimizeError::Timeout);
    }

    if let Some(err) = last_constraint {
        return Err(err);
    }

    Err(OptimizeError::Internal("no solution produced".to_string()))
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
    let mut stock_map = HashMap::new();

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

        stock_map.entry((usable_w, usable_h)).or_insert_with(|| StockInfo {
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

fn map_optimizer_error(
    err: cut_optimizer_2d::Error,
    prepared: &PreparedInput,
) -> OptimizeError {
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
    let mut total_stock_area: u128 = 0;
    let mut total_waste_area: u128 = 0;

    for stock in &solution.stock_pieces {
        let stock_area = area(stock.width, stock.length);
        let mut used_area: u128 = 0;
        for cut_piece in &stock.cut_pieces {
            used_area = used_area.saturating_add(area(cut_piece.width, cut_piece.length));
        }
        total_stock_area = total_stock_area.saturating_add(stock_area);
        total_waste_area = total_waste_area.saturating_add(stock_area.saturating_sub(used_area));
    }

    Candidate {
        used_stock_count: solution.stock_pieces.len() as u32,
        total_waste_area_units: total_waste_area,
        total_stock_area_units: total_stock_area,
        solution,
    }
}

fn is_better(candidate: &Candidate, best: &Candidate, objective: &Objective) -> bool {
    match objective {
        Objective::MinSheets => {
            if candidate.used_stock_count < best.used_stock_count {
                true
            } else if candidate.used_stock_count == best.used_stock_count {
                candidate.total_waste_area_units < best.total_waste_area_units
            } else {
                false
            }
        }
        Objective::MinWaste => {
            if candidate.total_waste_area_units < best.total_waste_area_units {
                true
            } else if candidate.total_waste_area_units == best.total_waste_area_units {
                candidate.used_stock_count < best.used_stock_count
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
    for info in prepared.stock_map.values() {
        stock_limits.insert(info.stock_id.clone(), info.qty_limit);
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

fn build_solutions(solution: &cut_optimizer_2d::Solution, prepared: &PreparedInput) -> Vec<Solution> {
    let mut index_map: HashMap<String, u32> = HashMap::new();
    let mut output = Vec::new();

    for stock in &solution.stock_pieces {
        let info = prepared
            .stock_map
            .get(&(stock.width, stock.length))
            .cloned();
        let (stock_id, full_width_mm, full_height_mm) = match info {
            Some(info) => (info.stock_id, info.full_width_mm, info.full_height_mm),
            None => (
                "unknown".to_string(),
                from_units(stock.width),
                from_units(stock.length),
            ),
        };

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

fn build_placement(
    cut_piece: &cut_optimizer_2d::ResultCutPiece,
    prepared: &PreparedInput,
) -> Option<Placement> {
    let instance = cut_piece.external_id.and_then(|id| prepared.instance_map.get(id));
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

fn units_area_to_mm2(units_area: u128) -> f64 {
    let scale_sq = SCALE * SCALE;
    (units_area as f64) / scale_sq
}

fn waste_percent(waste_area: u128, stock_area: u128) -> f64 {
    if stock_area == 0 {
        return 0.0;
    }
    let waste_mm2 = units_area_to_mm2(waste_area);
    let stock_mm2 = units_area_to_mm2(stock_area);
    (waste_mm2 / stock_mm2) * 100.0
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
