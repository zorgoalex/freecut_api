use std::collections::HashSet;

use crate::models::{
    OptimizeRequest, PatternDirection, Placement, Rotation, Solution, UnplacedItem,
    VacuumDirection, VacuumTelemetry, VacuumUsedBbox,
};

const EPS: f64 = 1.0e-7;
// A heterogeneous width-first row uses bounded two-part lookahead. Medium real
// jobs can legitimately need the full probe allowance; a smaller linear budget
// aborts after the first row and silently falls back to the legacy planner.
const MIN_WIDTH_FIRST_PROBES: usize = 2_000_000;
const MAX_WIDTH_FIRST_PROBES: usize = 2_000_000;
const WIDTH_FIRST_PROBES_PER_PART: usize = 2_000;
const MAX_WIDTH_FIRST_SEEDS: usize = 32;
const EXTREME_SEEDS_PER_METRIC: usize = 6;
const MAX_LEGACY_FIT_CHECKS: usize = 25_000_000;
const MAX_HOMOGENEOUS_COMBINATIONS: usize = 25_000_000;

pub(crate) struct VacuumLayoutResult {
    pub solutions: Vec<Solution>,
    pub unplaced_items: Vec<UnplacedItem>,
    pub telemetry: VacuumTelemetry,
}

#[derive(Clone)]
struct VacuumPart {
    seq: usize,
    item_id: String,
    instance: u32,
    width_mm: f64,
    height_mm: f64,
    can_rotate: bool,
    pattern_direction: PatternDirection,
}

#[derive(Clone)]
struct RowPart {
    part: VacuumPart,
    width_mm: f64,
    height_mm: f64,
    rotated: bool,
}

#[derive(Clone)]
struct VacuumRow {
    parts: Vec<RowPart>,
    cross_size_mm: f64,
}

#[derive(Clone)]
struct VacuumCandidate {
    strategy: &'static str,
    rows: Vec<VacuumRow>,
    score: VacuumScore,
}

#[derive(Clone)]
struct VacuumPlan {
    axis: VacuumDirection,
    strategy: &'static str,
    loads: Vec<Vec<VacuumRow>>,
    remaining: Vec<VacuumPart>,
}

struct VacuumSearchBudget {
    remaining: usize,
    exhausted: bool,
}

struct VacuumSeedPortfolio {
    orders: [Vec<usize>; 6],
    cursors: [usize; 6],
    stable_order: Vec<usize>,
    stable_cursor: usize,
}

impl VacuumSearchBudget {
    fn for_parts(part_count: usize) -> Self {
        Self {
            remaining: part_count
                .saturating_mul(WIDTH_FIRST_PROBES_PER_PART)
                .clamp(MIN_WIDTH_FIRST_PROBES, MAX_WIDTH_FIRST_PROBES),
            exhausted: false,
        }
    }

    fn legacy() -> Self {
        Self {
            remaining: MAX_LEGACY_FIT_CHECKS,
            exhausted: false,
        }
    }

    fn probe(&mut self) -> bool {
        if self.remaining == 0 {
            self.exhausted = true;
            return false;
        }
        self.remaining -= 1;
        true
    }
}

#[derive(Clone, Copy)]
struct VacuumScore {
    placed_count: usize,
    unplaced_count: usize,
    coverage_ratio: f64,
    bbox_width_ratio: f64,
    bbox_height_ratio: f64,
    bbox_area_ratio: f64,
    edge_offset: f64,
    homogeneous: bool,
}

#[derive(Clone, Copy)]
struct RowType {
    cross_mm: f64,
    width_mm: f64,
    height_mm: f64,
    rotated: bool,
    capacity: usize,
    max_rows: usize,
}

#[derive(Clone, Copy)]
enum VacuumSorter {
    Area,
    LongSide,
    ShortSide,
}

pub(crate) fn run_vacuum_layout(req: &OptimizeRequest) -> Result<VacuumLayoutResult, String> {
    if req.stock.len() != 1 {
        return Err("layout_mode=vacuum_table requires exactly one stock entry".to_string());
    }

    let stock = &req.stock[0];
    let trim = req.params.trim_mm;
    let usable_width = stock.width_mm - trim.left - trim.right;
    let usable_height = stock.height_mm - trim.top - trim.bottom;
    if usable_width <= 0.0 || usable_height <= 0.0 {
        return Err("trim exceeds stock dimensions".to_string());
    }

    let gap_mm = req.params.kerf_mm + req.params.spacing_mm;
    let requested_direction = req
        .params
        .vacuum
        .as_ref()
        .and_then(|vacuum| vacuum.direction)
        .unwrap_or(VacuumDirection::Optimal);

    let (mut remaining, mut unplaced_items) =
        collect_vacuum_parts(req, usable_width, usable_height);
    let max_sheets = match stock.qty {
        Some(qty) if qty > 0 => qty,
        _ => remaining.len().max(1) as u32,
    };

    let concrete_axis = match requested_direction {
        VacuumDirection::Optimal => physical_width_axis(stock.width_mm, stock.height_mm),
        direction => direction,
    };
    let baseline = build_legacy_plan(
        &remaining,
        concrete_axis,
        usable_width,
        usable_height,
        gap_mm,
        max_sheets,
    )
    .ok_or_else(|| "vacuum baseline search budget exceeded".to_string())?;
    let plan = if requested_direction == VacuumDirection::Optimal {
        build_width_first_plan(
            &remaining,
            concrete_axis,
            usable_width,
            usable_height,
            gap_mm,
            max_sheets,
        )
        .filter(|advanced| {
            plan_is_valid(advanced, &remaining, usable_width, usable_height, gap_mm)
                && width_first_plan_is_better(advanced, &baseline, gap_mm)
        })
        .unwrap_or(baseline)
    } else {
        baseline
    };

    let chosen_direction = plan.axis;
    let strategy = plan.strategy;
    remaining = plan.remaining;
    let solutions = plan
        .loads
        .iter()
        .enumerate()
        .map(|(sheet_index, rows)| Solution {
            stock_id: stock.id.clone(),
            index: sheet_index as u32,
            width_mm: stock.width_mm,
            height_mm: stock.height_mm,
            trim_mm: trim,
            placements: rows_to_placements(
                rows,
                chosen_direction,
                usable_width,
                usable_height,
                gap_mm,
            ),
        })
        .collect::<Vec<_>>();

    let sheet_limit_reached =
        matches!(stock.qty, Some(qty) if qty > 0 && solutions.len() as u32 >= qty);
    let leftover_reason = if sheet_limit_reached {
        "qty_limit"
    } else {
        "no_space"
    };
    unplaced_items.extend(remaining.into_iter().map(|part| UnplacedItem {
        item_id: part.item_id,
        instance: part.instance,
        width_mm: part.width_mm,
        height_mm: part.height_mm,
        reason: leftover_reason.to_string(),
    }));

    let telemetry = vacuum_telemetry(
        &solutions,
        &unplaced_items,
        chosen_direction,
        strategy,
        usable_width,
        usable_height,
    );

    Ok(VacuumLayoutResult {
        solutions,
        unplaced_items,
        telemetry,
    })
}

fn physical_width_axis(stock_width: f64, stock_height: f64) -> VacuumDirection {
    if stock_width <= stock_height {
        VacuumDirection::Width
    } else {
        VacuumDirection::Height
    }
}

fn build_legacy_plan(
    parts: &[VacuumPart],
    axis: VacuumDirection,
    usable_width: f64,
    usable_height: f64,
    gap_mm: f64,
    max_sheets: u32,
) -> Option<VacuumPlan> {
    let mut remaining = parts.to_vec();
    let mut loads = Vec::new();
    let mut strategy: Option<&'static str> = None;
    let mut budget = VacuumSearchBudget::legacy();

    for _ in 0..max_sheets {
        if remaining.is_empty() {
            break;
        }
        let Some(candidate) = choose_candidate(
            &remaining,
            usable_width,
            usable_height,
            gap_mm,
            axis,
            &mut budget,
        ) else {
            break;
        };
        if candidate.score.placed_count == 0 {
            break;
        }

        strategy = Some(match strategy {
            None => candidate.strategy,
            Some(previous) if previous == candidate.strategy => previous,
            Some(_) => "mixed",
        });
        let placed_seq = row_seqs(&candidate.rows);
        remaining = remove_placed_parts(remaining, &placed_seq, &mut budget)?;
        loads.push(candidate.rows);
    }

    if budget.exhausted {
        return None;
    }

    Some(VacuumPlan {
        axis,
        strategy: strategy.unwrap_or("none"),
        loads,
        remaining,
    })
}

fn build_width_first_plan(
    parts: &[VacuumPart],
    axis: VacuumDirection,
    usable_width: f64,
    usable_height: f64,
    gap_mm: f64,
    max_sheets: u32,
) -> Option<VacuumPlan> {
    let (primary_span, cross_span) = axis_spans(axis, usable_width, usable_height);
    let mut budget = VacuumSearchBudget::for_parts(parts.len());
    let mut seed_portfolio = (parts.len() > 256).then(|| VacuumSeedPortfolio::new(parts, axis));
    let mut remaining = parts.to_vec();
    let mut rows = Vec::new();

    while !remaining.is_empty() {
        let row = best_width_first_row(
            &remaining,
            axis,
            primary_span,
            cross_span,
            gap_mm,
            &mut budget,
            seed_portfolio.as_mut(),
        )?;
        let placed_seq = row_seqs(std::slice::from_ref(&row));
        remaining = remove_placed_parts(remaining, &placed_seq, &mut budget)?;
        rows.push(row);
    }
    if budget.exhausted {
        return None;
    }

    let (loads, rejected) = pack_rows_best_fit(rows, cross_span, gap_mm, max_sheets, &mut budget)?;
    remaining.extend(rejected.into_iter().flat_map(|row| {
        row.parts
            .into_iter()
            .map(|row_part| row_part.part)
            .collect::<Vec<_>>()
    }));
    remaining.sort_by_key(|part| part.seq);

    Some(VacuumPlan {
        axis,
        strategy: "width_first_rows",
        loads,
        remaining,
    })
}

fn best_width_first_row(
    parts: &[VacuumPart],
    axis: VacuumDirection,
    primary_span: f64,
    cross_span: f64,
    gap_mm: f64,
    budget: &mut VacuumSearchBudget,
    seed_portfolio: Option<&mut VacuumSeedPortfolio>,
) -> Option<VacuumRow> {
    let seed_indices = match seed_portfolio {
        Some(portfolio) => portfolio.seed_indices(parts, budget)?,
        None => (0..parts.len()).collect(),
    };
    let mut best: Option<VacuumRow> = None;

    for seed_idx in seed_indices {
        for rotated in [false, true] {
            let seed = &parts[seed_idx];
            if rotated && !seed.can_rotate {
                continue;
            }
            let (width_mm, height_mm) = oriented_size(seed, rotated);
            if primary_size(axis, width_mm, height_mm) > primary_span + EPS
                || cross_size(axis, width_mm, height_mm) > cross_span + EPS
            {
                continue;
            }
            if !budget.probe() {
                return None;
            }

            let candidate = build_width_first_row_from_seed(
                parts,
                seed_idx,
                rotated,
                axis,
                primary_span,
                gap_mm,
                budget,
            )?;
            if best
                .as_ref()
                .map(|current| width_first_row_is_better(&candidate, current, axis, gap_mm))
                .unwrap_or(true)
            {
                best = Some(candidate);
            }
        }
    }
    best
}

fn build_width_first_row_from_seed(
    parts: &[VacuumPart],
    seed_idx: usize,
    seed_rotated: bool,
    axis: VacuumDirection,
    primary_span: f64,
    gap_mm: f64,
    budget: &mut VacuumSearchBudget,
) -> Option<VacuumRow> {
    let seed = &parts[seed_idx];
    let (width_mm, height_mm) = oriented_size(seed, seed_rotated);
    let mut row = VacuumRow {
        cross_size_mm: cross_size(axis, width_mm, height_mm),
        parts: vec![RowPart {
            part: seed.clone(),
            width_mm,
            height_mm,
            rotated: seed_rotated,
        }],
    };
    let mut primary_used = primary_size(axis, width_mm, height_mm);
    let mut selected = HashSet::from([seed.seq]);

    loop {
        let primary_remaining = primary_span - primary_used - gap_mm;
        if primary_remaining <= EPS {
            break;
        }
        let next = best_width_first_next(
            parts,
            &selected,
            axis,
            primary_remaining,
            row.cross_size_mm,
            gap_mm,
            budget,
        )?;
        let Some(row_part) = next else {
            break;
        };
        primary_used += gap_mm + primary_size(axis, row_part.width_mm, row_part.height_mm);
        selected.insert(row_part.part.seq);
        row.parts.push(row_part);
    }
    Some(row)
}

fn best_width_first_next(
    parts: &[VacuumPart],
    selected: &HashSet<usize>,
    axis: VacuumDirection,
    primary_remaining: f64,
    row_cross_size: f64,
    gap_mm: f64,
    budget: &mut VacuumSearchBudget,
) -> Option<Option<RowPart>> {
    let mut best: Option<(RowPart, f64)> = None;
    for (idx, part) in parts.iter().enumerate() {
        if !budget.probe() {
            return None;
        }
        if selected.contains(&part.seq) {
            continue;
        }
        for rotated in [false, true] {
            if rotated && !part.can_rotate {
                continue;
            }
            let (width_mm, height_mm) = oriented_size(part, rotated);
            let primary = primary_size(axis, width_mm, height_mm);
            let cross = cross_size(axis, width_mm, height_mm);
            if primary > primary_remaining + EPS || cross > row_cross_size + EPS {
                continue;
            }
            let second_span = primary_remaining - primary - gap_mm;
            let lookahead = if second_span > EPS {
                best_lookahead_fill(
                    parts,
                    selected,
                    idx,
                    axis,
                    second_span,
                    row_cross_size,
                    budget,
                )?
            } else {
                0.0
            };
            let projected_fill = primary
                + if lookahead > 0.0 {
                    gap_mm + lookahead
                } else {
                    0.0
                };
            let candidate = RowPart {
                part: part.clone(),
                width_mm,
                height_mm,
                rotated,
            };
            let replace = best
                .as_ref()
                .map(|(current, current_fill)| {
                    projected_fill > *current_fill + EPS
                        || nearly_eq(projected_fill, *current_fill)
                            && (primary
                                > primary_size(axis, current.width_mm, current.height_mm) + EPS
                                || nearly_eq(
                                    primary,
                                    primary_size(axis, current.width_mm, current.height_mm),
                                ) && (cross
                                    < cross_size(axis, current.width_mm, current.height_mm) - EPS
                                    || nearly_eq(
                                        cross,
                                        cross_size(axis, current.width_mm, current.height_mm),
                                    ) && part.seq < current.part.seq))
                })
                .unwrap_or(true);
            if replace {
                best = Some((candidate, projected_fill));
            }
        }
    }
    Some(best.map(|(part, _)| part))
}

fn best_lookahead_fill(
    parts: &[VacuumPart],
    selected: &HashSet<usize>,
    excluded_idx: usize,
    axis: VacuumDirection,
    primary_limit: f64,
    cross_limit: f64,
    budget: &mut VacuumSearchBudget,
) -> Option<f64> {
    let mut best = 0.0_f64;
    for (idx, part) in parts.iter().enumerate() {
        if !budget.probe() {
            return None;
        }
        if idx == excluded_idx || selected.contains(&part.seq) {
            continue;
        }
        for rotated in [false, true] {
            if rotated && !part.can_rotate {
                continue;
            }
            let (width_mm, height_mm) = oriented_size(part, rotated);
            let primary = primary_size(axis, width_mm, height_mm);
            let cross = cross_size(axis, width_mm, height_mm);
            if primary <= primary_limit + EPS && cross <= cross_limit + EPS {
                best = best.max(primary);
            }
        }
    }
    Some(best)
}

impl VacuumSeedPortfolio {
    fn new(parts: &[VacuumPart], axis: VacuumDirection) -> Self {
        let mut orders: [Vec<usize>; 6] = std::array::from_fn(|_| (0..parts.len()).collect());
        for (metric, order) in orders.iter_mut().enumerate() {
            order.sort_by(|&a, &b| seed_metric_order(a, b, parts, axis, metric));
            for idx in order {
                *idx = parts[*idx].seq;
            }
        }
        Self {
            orders,
            cursors: [0; 6],
            stable_order: parts.iter().map(|part| part.seq).collect(),
            stable_cursor: 0,
        }
    }

    fn seed_indices(
        &mut self,
        parts: &[VacuumPart],
        budget: &mut VacuumSearchBudget,
    ) -> Option<Vec<usize>> {
        let mut extremes: [Vec<usize>; 6] = std::array::from_fn(|_| Vec::new());
        for (metric, selected) in extremes.iter_mut().enumerate() {
            while self.cursors[metric] < self.orders[metric].len() {
                if !budget.probe() {
                    return None;
                }
                let seq = self.orders[metric][self.cursors[metric]];
                if part_index_by_seq(parts, seq).is_some() {
                    break;
                }
                self.cursors[metric] += 1;
            }

            let mut position = self.cursors[metric];
            while position < self.orders[metric].len() && selected.len() < EXTREME_SEEDS_PER_METRIC
            {
                if !budget.probe() {
                    return None;
                }
                let seq = self.orders[metric][position];
                if let Some(idx) = part_index_by_seq(parts, seq) {
                    selected.push(idx);
                }
                position += 1;
            }
        }

        let mut seeds = Vec::with_capacity(MAX_WIDTH_FIRST_SEEDS);
        let mut seen = HashSet::new();
        for rank in 0..EXTREME_SEEDS_PER_METRIC {
            for selected in &extremes {
                if let Some(&idx) = selected.get(rank) {
                    if seen.insert(parts[idx].seq) {
                        seeds.push(idx);
                        if seeds.len() == MAX_WIDTH_FIRST_SEEDS {
                            return Some(seeds);
                        }
                    }
                }
            }
        }

        while self.stable_cursor < self.stable_order.len() {
            if !budget.probe() {
                return None;
            }
            let seq = self.stable_order[self.stable_cursor];
            if part_index_by_seq(parts, seq).is_some() {
                break;
            }
            self.stable_cursor += 1;
        }
        let mut position = self.stable_cursor;
        while position < self.stable_order.len() && seeds.len() < MAX_WIDTH_FIRST_SEEDS {
            if !budget.probe() {
                return None;
            }
            let seq = self.stable_order[position];
            if let Some(idx) = part_index_by_seq(parts, seq) {
                if seen.insert(seq) {
                    seeds.push(idx);
                }
            }
            position += 1;
        }
        Some(seeds)
    }
}

fn part_index_by_seq(parts: &[VacuumPart], seq: usize) -> Option<usize> {
    parts.binary_search_by_key(&seq, |part| part.seq).ok()
}

fn seed_metric_order(
    a: usize,
    b: usize,
    parts: &[VacuumPart],
    axis: VacuumDirection,
    metric: usize,
) -> std::cmp::Ordering {
    let primary_a = seed_primary_metric(&parts[a], axis);
    let primary_b = seed_primary_metric(&parts[b], axis);
    let cross_a = seed_cross_metric(&parts[a], axis);
    let cross_b = seed_cross_metric(&parts[b], axis);
    let area_a = parts[a].width_mm * parts[a].height_mm;
    let area_b = parts[b].width_mm * parts[b].height_mm;
    let order = match metric {
        0 => primary_a.total_cmp(&primary_b),
        1 => primary_b.total_cmp(&primary_a),
        2 => cross_a.total_cmp(&cross_b),
        3 => cross_b.total_cmp(&cross_a),
        4 => area_a.total_cmp(&area_b),
        5 => area_b.total_cmp(&area_a),
        _ => unreachable!("unknown vacuum seed metric"),
    };
    order.then_with(|| parts[a].seq.cmp(&parts[b].seq))
}

fn seed_primary_metric(part: &VacuumPart, axis: VacuumDirection) -> f64 {
    let unrotated = primary_size(axis, part.width_mm, part.height_mm);
    if part.can_rotate {
        unrotated.max(primary_size(axis, part.height_mm, part.width_mm))
    } else {
        unrotated
    }
}

fn seed_cross_metric(part: &VacuumPart, axis: VacuumDirection) -> f64 {
    let unrotated = cross_size(axis, part.width_mm, part.height_mm);
    if part.can_rotate {
        unrotated.max(cross_size(axis, part.height_mm, part.width_mm))
    } else {
        unrotated
    }
}

fn width_first_row_is_better(
    candidate: &VacuumRow,
    current: &VacuumRow,
    axis: VacuumDirection,
    gap_mm: f64,
) -> bool {
    let candidate_used = row_primary_used(candidate, axis, gap_mm);
    let current_used = row_primary_used(current, axis, gap_mm);
    candidate_used > current_used + EPS
        || nearly_eq(candidate_used, current_used)
            && (candidate.cross_size_mm < current.cross_size_mm - EPS
                || nearly_eq(candidate.cross_size_mm, current.cross_size_mm)
                    && (candidate.parts.len() > current.parts.len()
                        || candidate.parts.len() == current.parts.len()
                            && row_min_seq(candidate) < row_min_seq(current)))
}

fn pack_rows_best_fit(
    mut rows: Vec<VacuumRow>,
    cross_span: f64,
    gap_mm: f64,
    max_sheets: u32,
    budget: &mut VacuumSearchBudget,
) -> Option<(Vec<Vec<VacuumRow>>, Vec<VacuumRow>)> {
    rows.sort_by(|a, b| {
        b.cross_size_mm
            .total_cmp(&a.cross_size_mm)
            .then_with(|| row_min_seq(a).cmp(&row_min_seq(b)))
    });
    let mut loads: Vec<Vec<VacuumRow>> = Vec::new();
    let mut load_cross_used = Vec::new();
    let mut rejected = Vec::new();

    for row in rows {
        let mut best_load: Option<(usize, f64)> = None;
        for (idx, load) in loads.iter().enumerate() {
            if !budget.probe() {
                return None;
            }
            let used = load_cross_used[idx];
            let next_used = used + if load.is_empty() { 0.0 } else { gap_mm } + row.cross_size_mm;
            if next_used > cross_span + EPS {
                continue;
            }
            let remaining = cross_span - next_used;
            if best_load
                .map(|(_, best_remaining)| remaining < best_remaining)
                .unwrap_or(true)
            {
                best_load = Some((idx, remaining));
            }
        }

        if let Some((load_idx, _)) = best_load {
            load_cross_used[load_idx] += gap_mm + row.cross_size_mm;
            loads[load_idx].push(row);
        } else if loads.len() < max_sheets as usize {
            load_cross_used.push(row.cross_size_mm);
            loads.push(vec![row]);
        } else {
            rejected.push(row);
        }
    }

    for load in &mut loads {
        load.sort_by_key(row_min_seq);
    }
    loads.sort_by_key(|load| load.iter().map(row_min_seq).min().unwrap_or(usize::MAX));
    Some((loads, rejected))
}

fn plan_is_valid(
    plan: &VacuumPlan,
    original_parts: &[VacuumPart],
    usable_width: f64,
    usable_height: f64,
    gap_mm: f64,
) -> bool {
    let expected = original_parts
        .iter()
        .map(|part| part.seq)
        .collect::<HashSet<_>>();
    let mut actual = HashSet::new();
    for load in &plan.loads {
        let placements = rows_to_placements(load, plan.axis, usable_width, usable_height, gap_mm);
        if placements.iter().any(|placement| {
            placement.x_mm < -EPS
                || placement.y_mm < -EPS
                || placement.x_mm + placement.width_mm > usable_width + EPS
                || placement.y_mm + placement.height_mm > usable_height + EPS
        }) {
            return false;
        }
        for seq in row_seqs(load) {
            if !actual.insert(seq) {
                return false;
            }
        }
    }
    for part in &plan.remaining {
        if !actual.insert(part.seq) {
            return false;
        }
    }
    actual == expected
}

fn width_first_plan_is_better(advanced: &VacuumPlan, baseline: &VacuumPlan, gap_mm: f64) -> bool {
    let advanced_placed = plan_placed_count(advanced);
    let baseline_placed = plan_placed_count(baseline);
    if advanced_placed < baseline_placed
        || advanced.remaining.len() > baseline.remaining.len()
        || advanced.loads.len() > baseline.loads.len()
    {
        return false;
    }
    if advanced_placed > baseline_placed
        || advanced.remaining.len() < baseline.remaining.len()
        || advanced.loads.len() < baseline.loads.len()
    {
        return true;
    }

    let advanced_feed = plan_feed_used(advanced, gap_mm);
    let baseline_feed = plan_feed_used(baseline, gap_mm);
    advanced_feed < baseline_feed - EPS
        || nearly_eq(advanced_feed, baseline_feed)
            && (plan_primary_fill(advanced, gap_mm) > plan_primary_fill(baseline, gap_mm) + EPS
                || nearly_eq(
                    plan_primary_fill(advanced, gap_mm),
                    plan_primary_fill(baseline, gap_mm),
                ) && plan_sequence(advanced) < plan_sequence(baseline))
}

fn plan_placed_count(plan: &VacuumPlan) -> usize {
    plan.loads
        .iter()
        .flat_map(|rows| rows.iter())
        .map(|row| row.parts.len())
        .sum()
}

fn plan_feed_used(plan: &VacuumPlan, gap_mm: f64) -> f64 {
    plan.loads
        .iter()
        .map(|rows| rows_cross_used(rows, gap_mm))
        .sum()
}

fn plan_primary_fill(plan: &VacuumPlan, gap_mm: f64) -> f64 {
    plan.loads
        .iter()
        .flat_map(|rows| rows.iter())
        .map(|row| row_primary_used(row, plan.axis, gap_mm))
        .sum()
}

fn plan_sequence(plan: &VacuumPlan) -> Vec<usize> {
    plan.loads
        .iter()
        .flat_map(|rows| rows.iter())
        .flat_map(|row| row.parts.iter().map(|part| part.part.seq))
        .collect()
}

fn row_primary_used(row: &VacuumRow, axis: VacuumDirection, gap_mm: f64) -> f64 {
    row.parts
        .iter()
        .map(|part| primary_size(axis, part.width_mm, part.height_mm))
        .sum::<f64>()
        + gap_mm * row.parts.len().saturating_sub(1) as f64
}

fn row_min_seq(row: &VacuumRow) -> usize {
    row.parts
        .iter()
        .map(|part| part.part.seq)
        .min()
        .unwrap_or(usize::MAX)
}

fn row_seqs(rows: &[VacuumRow]) -> HashSet<usize> {
    rows.iter()
        .flat_map(|row| row.parts.iter().map(|part| part.part.seq))
        .collect()
}

fn remove_placed_parts(
    parts: Vec<VacuumPart>,
    placed_seq: &HashSet<usize>,
    budget: &mut VacuumSearchBudget,
) -> Option<Vec<VacuumPart>> {
    let mut remaining = Vec::with_capacity(parts.len().saturating_sub(placed_seq.len()));
    for part in parts {
        if !budget.probe() {
            return None;
        }
        if !placed_seq.contains(&part.seq) {
            remaining.push(part);
        }
    }
    Some(remaining)
}

fn collect_vacuum_parts(
    req: &OptimizeRequest,
    usable_width: f64,
    usable_height: f64,
) -> (Vec<VacuumPart>, Vec<UnplacedItem>) {
    let mut parts = Vec::new();
    let mut unplaced = Vec::new();
    let mut seq = 0_usize;

    for item in &req.items {
        let can_rotate =
            item.rotation == Rotation::Allow90 && item.pattern_direction == PatternDirection::None;
        for instance_idx in 0..item.qty {
            let part = VacuumPart {
                seq,
                item_id: item.id.clone(),
                instance: instance_idx + 1,
                width_mm: item.width_mm,
                height_mm: item.height_mm,
                can_rotate,
                pattern_direction: item.pattern_direction,
            };
            seq += 1;

            if part_fits_table(&part, usable_width, usable_height) {
                parts.push(part);
            } else {
                unplaced.push(UnplacedItem {
                    item_id: item.id.clone(),
                    instance: instance_idx + 1,
                    width_mm: item.width_mm,
                    height_mm: item.height_mm,
                    reason: "oversized".to_string(),
                });
            }
        }
    }

    (parts, unplaced)
}

fn part_fits_table(part: &VacuumPart, usable_width: f64, usable_height: f64) -> bool {
    part.width_mm <= usable_width + EPS && part.height_mm <= usable_height + EPS
        || part.can_rotate
            && part.height_mm <= usable_width + EPS
            && part.width_mm <= usable_height + EPS
}

fn choose_candidate(
    parts: &[VacuumPart],
    usable_width: f64,
    usable_height: f64,
    gap_mm: f64,
    requested_direction: VacuumDirection,
    budget: &mut VacuumSearchBudget,
) -> Option<VacuumCandidate> {
    let axes: &[VacuumDirection] = match requested_direction {
        VacuumDirection::Optimal => &[VacuumDirection::Width, VacuumDirection::Height],
        VacuumDirection::Width => &[VacuumDirection::Width],
        VacuumDirection::Height => &[VacuumDirection::Height],
    };

    let mut best: Option<VacuumCandidate> = None;
    for &axis in axes {
        if let Some(candidate) =
            pack_homogeneous(parts, axis, usable_width, usable_height, gap_mm, budget)
        {
            replace_if_better(&mut best, candidate);
        }
        if budget.exhausted {
            break;
        }
        for sorter in [
            VacuumSorter::Area,
            VacuumSorter::LongSide,
            VacuumSorter::ShortSide,
        ] {
            if let Some(candidate) = pack_general(
                parts,
                axis,
                usable_width,
                usable_height,
                gap_mm,
                sorter,
                budget,
            ) {
                replace_if_better(&mut best, candidate);
            }
            if budget.exhausted {
                break;
            }
        }
        if budget.exhausted {
            break;
        }
    }
    best
}

fn replace_if_better(best: &mut Option<VacuumCandidate>, candidate: VacuumCandidate) {
    if best
        .as_ref()
        .map(|current| score_better(candidate.score, current.score))
        .unwrap_or(true)
    {
        *best = Some(candidate);
    }
}

fn pack_homogeneous(
    parts: &[VacuumPart],
    axis: VacuumDirection,
    usable_width: f64,
    usable_height: f64,
    gap_mm: f64,
    budget: &mut VacuumSearchBudget,
) -> Option<VacuumCandidate> {
    let first = parts.first()?;
    if !parts.iter().all(|part| {
        nearly_eq(part.width_mm, first.width_mm)
            && nearly_eq(part.height_mm, first.height_mm)
            && part.can_rotate == first.can_rotate
    }) {
        return None;
    }

    let (primary_span, cross_span) = axis_spans(axis, usable_width, usable_height);
    let mut row_types = Vec::new();
    if let Some(row_type) = row_type_for(first, axis, false, primary_span, cross_span, gap_mm) {
        row_types.push(row_type);
    }
    if first.can_rotate && !nearly_eq(first.width_mm, first.height_mm) {
        if let Some(row_type) = row_type_for(first, axis, true, primary_span, cross_span, gap_mm) {
            row_types.push(row_type);
        }
    }
    if row_types.is_empty() {
        return None;
    }

    row_types.sort_by(|a, b| {
        b.capacity
            .cmp(&a.capacity)
            .then_with(|| b.cross_mm.total_cmp(&a.cross_mm))
    });

    let counts = best_homogeneous_row_counts(&row_types, parts.len(), cross_span, gap_mm, budget)?;
    let mut rows = Vec::new();
    let mut next_part = 0_usize;
    for (row_type, &row_count) in row_types.iter().zip(counts.iter()) {
        for _ in 0..row_count {
            if next_part >= parts.len() {
                break;
            }
            let take_count = row_type.capacity.min(parts.len() - next_part);
            if take_count == 0 {
                continue;
            }
            let row_parts = parts[next_part..next_part + take_count]
                .iter()
                .cloned()
                .map(|part| RowPart {
                    part,
                    width_mm: row_type.width_mm,
                    height_mm: row_type.height_mm,
                    rotated: row_type.rotated,
                })
                .collect();
            next_part += take_count;
            rows.push(VacuumRow {
                parts: row_parts,
                cross_size_mm: row_type.cross_mm,
            });
        }
    }

    candidate_from_rows(
        axis,
        "homogeneous",
        rows,
        parts.len(),
        usable_width,
        usable_height,
        gap_mm,
        true,
    )
}

fn row_type_for(
    part: &VacuumPart,
    axis: VacuumDirection,
    rotated: bool,
    primary_span: f64,
    cross_span: f64,
    gap_mm: f64,
) -> Option<RowType> {
    let (width_mm, height_mm) = oriented_size(part, rotated);
    let primary_mm = primary_size(axis, width_mm, height_mm);
    let cross_mm = cross_size(axis, width_mm, height_mm);
    if primary_mm > primary_span + EPS || cross_mm > cross_span + EPS {
        return None;
    }
    let capacity = max_count_in_span(primary_span, primary_mm, gap_mm);
    let max_rows = max_count_in_span(cross_span, cross_mm, gap_mm);
    (capacity > 0 && max_rows > 0).then_some(RowType {
        cross_mm,
        width_mm,
        height_mm,
        rotated,
        capacity,
        max_rows,
    })
}

fn best_homogeneous_row_counts(
    row_types: &[RowType],
    requested_count: usize,
    cross_span: f64,
    gap_mm: f64,
    budget: &mut VacuumSearchBudget,
) -> Option<Vec<usize>> {
    let mut best_counts: Option<Vec<usize>> = None;
    let mut best_key: Option<(usize, usize, usize, usize)> = None;

    let mut visit = |counts: Vec<usize>| {
        let rows_total: usize = counts.iter().sum();
        if rows_total == 0 {
            return;
        }
        let capacity_total =
            counts
                .iter()
                .zip(row_types.iter())
                .fold(0_usize, |total, (count, row_type)| {
                    total.saturating_add(count.saturating_mul(row_type.capacity))
                });
        let placed = capacity_total.min(requested_count);
        if placed == 0 {
            return;
        }
        let cross_used: f64 = counts
            .iter()
            .zip(row_types.iter())
            .map(|(count, row_type)| *count as f64 * row_type.cross_mm)
            .sum::<f64>()
            + if rows_total > 1 {
                gap_mm * (rows_total - 1) as f64
            } else {
                0.0
            };
        if cross_used > cross_span + EPS {
            return;
        }
        let cross_key = ((cross_used / cross_span.max(EPS)) * 1_000_000.0).round() as usize;
        let over_capacity = capacity_total.saturating_sub(requested_count);
        let key = (
            placed,
            usize::MAX - over_capacity,
            usize::MAX - rows_total,
            usize::MAX - cross_key,
        );
        if best_key.map(|current| key > current).unwrap_or(true) {
            best_key = Some(key);
            best_counts = Some(counts);
        }
    };

    match row_types {
        [a] => {
            let a_limit = a.max_rows.min(requested_count);
            for a_count in 1..=a_limit {
                if !budget.probe() {
                    return None;
                }
                visit(vec![a_count]);
            }
        }
        [a, b] => {
            let a_limit = a.max_rows.min(requested_count);
            let b_limit = b.max_rows.min(requested_count);
            let combinations = a_limit
                .saturating_add(1)
                .saturating_mul(b_limit.saturating_add(1));
            if combinations > MAX_HOMOGENEOUS_COMBINATIONS {
                return None;
            }
            for a_count in 0..=a_limit {
                for b_count in 0..=b_limit {
                    if !budget.probe() {
                        return None;
                    }
                    visit(vec![a_count, b_count]);
                }
            }
        }
        _ => {}
    }

    best_counts
}

fn pack_general(
    parts: &[VacuumPart],
    axis: VacuumDirection,
    usable_width: f64,
    usable_height: f64,
    gap_mm: f64,
    sorter: VacuumSorter,
    budget: &mut VacuumSearchBudget,
) -> Option<VacuumCandidate> {
    let (primary_span, cross_span) = axis_spans(axis, usable_width, usable_height);
    let mut remaining = parts.to_vec();
    sort_parts(&mut remaining, sorter);

    let mut rows = Vec::new();
    loop {
        if remaining.is_empty() {
            break;
        }
        let cross_used = rows_cross_used(&rows, gap_mm);
        let cross_remaining = if rows.is_empty() {
            cross_span
        } else {
            cross_span - cross_used - gap_mm
        };
        if cross_remaining <= EPS {
            break;
        }

        let Some((seed_idx, seed_part)) =
            find_seed_part(&remaining, axis, primary_span, cross_remaining, budget)
        else {
            break;
        };

        remaining.remove(seed_idx);
        let mut row = VacuumRow {
            cross_size_mm: cross_size(axis, seed_part.width_mm, seed_part.height_mm),
            parts: vec![seed_part],
        };
        let mut primary_used = row
            .parts
            .iter()
            .map(|part| primary_size(axis, part.width_mm, part.height_mm))
            .sum::<f64>();

        loop {
            let remaining_primary = primary_span - primary_used - gap_mm;
            if remaining_primary <= EPS {
                break;
            }
            let Some((idx, row_part)) = find_next_row_part(
                &remaining,
                axis,
                remaining_primary,
                row.cross_size_mm,
                budget,
            ) else {
                break;
            };
            primary_used += gap_mm + primary_size(axis, row_part.width_mm, row_part.height_mm);
            remaining.remove(idx);
            row.parts.push(row_part);
        }

        rows.push(row);
    }
    if budget.exhausted {
        return None;
    }

    candidate_from_rows(
        axis,
        "general_shelf",
        rows,
        parts.len(),
        usable_width,
        usable_height,
        gap_mm,
        false,
    )
}

fn sort_parts(parts: &mut [VacuumPart], sorter: VacuumSorter) {
    match sorter {
        VacuumSorter::Area => {
            parts.sort_by(|a, b| (b.width_mm * b.height_mm).total_cmp(&(a.width_mm * a.height_mm)))
        }
        VacuumSorter::LongSide => parts.sort_by(|a, b| {
            b.width_mm
                .max(b.height_mm)
                .total_cmp(&a.width_mm.max(a.height_mm))
        }),
        VacuumSorter::ShortSide => parts.sort_by(|a, b| {
            b.width_mm
                .min(b.height_mm)
                .total_cmp(&a.width_mm.min(a.height_mm))
        }),
    }
}

fn find_seed_part(
    parts: &[VacuumPart],
    axis: VacuumDirection,
    primary_span: f64,
    cross_remaining: f64,
    budget: &mut VacuumSearchBudget,
) -> Option<(usize, RowPart)> {
    for (idx, part) in parts.iter().enumerate() {
        if !budget.probe() {
            return None;
        }
        if part.can_rotate && !budget.probe() {
            return None;
        }
        if let Some(row_part) = best_orientation(part, axis, primary_span, cross_remaining) {
            return Some((idx, row_part));
        }
    }
    None
}

fn find_next_row_part(
    parts: &[VacuumPart],
    axis: VacuumDirection,
    primary_remaining: f64,
    row_cross_size: f64,
    budget: &mut VacuumSearchBudget,
) -> Option<(usize, RowPart)> {
    for (idx, part) in parts.iter().enumerate() {
        if !budget.probe() {
            return None;
        }
        if part.can_rotate && !budget.probe() {
            return None;
        }
        if let Some(row_part) = best_orientation(part, axis, primary_remaining, row_cross_size) {
            return Some((idx, row_part));
        }
    }
    None
}

fn best_orientation(
    part: &VacuumPart,
    axis: VacuumDirection,
    primary_limit: f64,
    cross_limit: f64,
) -> Option<RowPart> {
    let mut best: Option<RowPart> = None;
    for rotated in [false, true] {
        if rotated && !part.can_rotate {
            continue;
        }
        let (width_mm, height_mm) = oriented_size(part, rotated);
        let primary = primary_size(axis, width_mm, height_mm);
        let cross = cross_size(axis, width_mm, height_mm);
        if primary > primary_limit + EPS || cross > cross_limit + EPS {
            continue;
        }
        let candidate = RowPart {
            part: part.clone(),
            width_mm,
            height_mm,
            rotated,
        };
        if best
            .as_ref()
            .map(|current| {
                primary > primary_size(axis, current.width_mm, current.height_mm) + EPS
                    || nearly_eq(
                        primary,
                        primary_size(axis, current.width_mm, current.height_mm),
                    ) && cross > cross_size(axis, current.width_mm, current.height_mm) + EPS
            })
            .unwrap_or(true)
        {
            best = Some(candidate);
        }
    }
    best
}

fn candidate_from_rows(
    axis: VacuumDirection,
    strategy: &'static str,
    rows: Vec<VacuumRow>,
    requested_count: usize,
    usable_width: f64,
    usable_height: f64,
    gap_mm: f64,
    homogeneous: bool,
) -> Option<VacuumCandidate> {
    let placed_count: usize = rows.iter().map(|row| row.parts.len()).sum();
    if placed_count == 0 {
        return None;
    }
    let placed_area = rows
        .iter()
        .flat_map(|row| &row.parts)
        .map(|part| part.width_mm * part.height_mm)
        .sum::<f64>();
    let bbox = used_bbox_for_rows(&rows, axis, usable_width, usable_height, gap_mm);
    let sheet_area = usable_width * usable_height;
    let coverage_ratio = if sheet_area > 0.0 {
        placed_area / sheet_area
    } else {
        0.0
    };
    let bbox_area_ratio = if sheet_area > 0.0 {
        (bbox.width_mm * bbox.height_mm) / sheet_area
    } else {
        0.0
    };
    let bbox_width_ratio = bbox.width_mm / usable_width.max(EPS);
    let bbox_height_ratio = bbox.height_mm / usable_height.max(EPS);
    let edge_offset = bbox.x_mm / usable_width.max(EPS) + bbox.y_mm / usable_height.max(EPS);

    Some(VacuumCandidate {
        strategy,
        rows,
        score: VacuumScore {
            placed_count,
            unplaced_count: requested_count.saturating_sub(placed_count),
            coverage_ratio,
            bbox_width_ratio,
            bbox_height_ratio,
            bbox_area_ratio,
            edge_offset,
            homogeneous,
        },
    })
}

fn score_better(candidate: VacuumScore, current: VacuumScore) -> bool {
    if candidate.placed_count != current.placed_count {
        return candidate.placed_count > current.placed_count;
    }
    if candidate.unplaced_count != current.unplaced_count {
        return candidate.unplaced_count < current.unplaced_count;
    }
    if (candidate.coverage_ratio - current.coverage_ratio).abs() > EPS {
        return candidate.coverage_ratio > current.coverage_ratio;
    }
    // Vacuum film has a fixed roll/table width. Once the same parts fit on a
    // load, material consumption is driven by occupied length, not by the
    // smallest two-dimensional bounding-box area. Prefer rows that fill the
    // available width and therefore finish sooner along the feed axis.
    if (candidate.bbox_height_ratio - current.bbox_height_ratio).abs() > EPS {
        return candidate.bbox_height_ratio < current.bbox_height_ratio;
    }
    if (candidate.bbox_width_ratio - current.bbox_width_ratio).abs() > EPS {
        return candidate.bbox_width_ratio > current.bbox_width_ratio;
    }
    if (candidate.bbox_area_ratio - current.bbox_area_ratio).abs() > EPS {
        return candidate.bbox_area_ratio < current.bbox_area_ratio;
    }
    if (candidate.edge_offset - current.edge_offset).abs() > EPS {
        return candidate.edge_offset < current.edge_offset;
    }
    candidate.homogeneous && !current.homogeneous
}

fn rows_to_placements(
    rows: &[VacuumRow],
    axis: VacuumDirection,
    _usable_width: f64,
    _usable_height: f64,
    gap_mm: f64,
) -> Vec<Placement> {
    let row_sizes = rows.iter().map(|row| row.cross_size_mm).collect::<Vec<_>>();
    let row_positions = compact_positions(&row_sizes, gap_mm);
    let mut placements = Vec::new();

    for (row, &cross_pos) in rows.iter().zip(row_positions.iter()) {
        let part_sizes = row
            .parts
            .iter()
            .map(|part| primary_size(axis, part.width_mm, part.height_mm))
            .collect::<Vec<_>>();
        let part_positions = compact_positions(&part_sizes, gap_mm);
        for (part, &primary_pos) in row.parts.iter().zip(part_positions.iter()) {
            let (x_mm, y_mm) = match axis {
                VacuumDirection::Width => (primary_pos, cross_pos),
                VacuumDirection::Height => (cross_pos, primary_pos),
                VacuumDirection::Optimal => unreachable!("optimal is not a concrete axis"),
            };
            placements.push(Placement {
                item_id: part.part.item_id.clone(),
                instance: part.part.instance,
                x_mm,
                y_mm,
                width_mm: part.width_mm,
                height_mm: part.height_mm,
                rotated: part.rotated,
                pattern_direction: part.part.pattern_direction,
            });
        }
    }

    placements
}

fn compact_positions(sizes: &[f64], min_gap: f64) -> Vec<f64> {
    match sizes {
        [] => Vec::new(),
        [_] => vec![0.0],
        _ => {
            let mut positions = Vec::with_capacity(sizes.len());
            let mut cursor = 0.0;
            for size in sizes {
                positions.push(cursor);
                cursor += *size + min_gap;
            }
            positions
        }
    }
}

fn used_bbox_for_rows(
    rows: &[VacuumRow],
    axis: VacuumDirection,
    usable_width: f64,
    usable_height: f64,
    gap_mm: f64,
) -> VacuumUsedBbox {
    let placements = rows_to_placements(rows, axis, usable_width, usable_height, gap_mm);
    used_bbox_for_placements(&placements)
}

fn vacuum_telemetry(
    solutions: &[Solution],
    unplaced_items: &[UnplacedItem],
    chosen_direction: VacuumDirection,
    strategy: &'static str,
    usable_width: f64,
    usable_height: f64,
) -> VacuumTelemetry {
    let placed_count = solutions
        .iter()
        .map(|solution| solution.placements.len() as u32)
        .sum::<u32>();
    let placed_area = solutions
        .iter()
        .flat_map(|solution| &solution.placements)
        .map(|placement| placement.width_mm * placement.height_mm)
        .sum::<f64>();
    let sheet_area = usable_width * usable_height * solutions.len() as f64;
    let coverage_ratio = if sheet_area > 0.0 {
        placed_area / sheet_area
    } else {
        0.0
    };
    let used_bbox = solutions
        .first()
        .map(|solution| used_bbox_for_placements(&solution.placements))
        .unwrap_or(VacuumUsedBbox {
            x_mm: 0.0,
            y_mm: 0.0,
            width_mm: 0.0,
            height_mm: 0.0,
        });

    VacuumTelemetry {
        chosen_direction,
        strategy: strategy.to_string(),
        placed_count,
        unplaced_count: unplaced_items.len() as u32,
        coverage_ratio,
        min_clearance_mm: min_clearance(solutions),
        used_bbox,
    }
}

fn used_bbox_for_placements(placements: &[Placement]) -> VacuumUsedBbox {
    if placements.is_empty() {
        return VacuumUsedBbox {
            x_mm: 0.0,
            y_mm: 0.0,
            width_mm: 0.0,
            height_mm: 0.0,
        };
    }
    let min_x = placements
        .iter()
        .map(|placement| placement.x_mm)
        .fold(f64::INFINITY, f64::min);
    let min_y = placements
        .iter()
        .map(|placement| placement.y_mm)
        .fold(f64::INFINITY, f64::min);
    let max_x = placements
        .iter()
        .map(|placement| placement.x_mm + placement.width_mm)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = placements
        .iter()
        .map(|placement| placement.y_mm + placement.height_mm)
        .fold(f64::NEG_INFINITY, f64::max);
    VacuumUsedBbox {
        x_mm: min_x,
        y_mm: min_y,
        width_mm: max_x - min_x,
        height_mm: max_y - min_y,
    }
}

fn min_clearance(solutions: &[Solution]) -> f64 {
    let mut best = f64::INFINITY;
    for solution in solutions {
        let placements = &solution.placements;
        for i in 0..placements.len() {
            for j in (i + 1)..placements.len() {
                best = best.min(rect_clearance(&placements[i], &placements[j]));
            }
        }
    }
    if best.is_finite() {
        best
    } else {
        0.0
    }
}

fn rect_clearance(a: &Placement, b: &Placement) -> f64 {
    let a_right = a.x_mm + a.width_mm;
    let a_bottom = a.y_mm + a.height_mm;
    let b_right = b.x_mm + b.width_mm;
    let b_bottom = b.y_mm + b.height_mm;

    let dx = if a_right <= b.x_mm {
        b.x_mm - a_right
    } else if b_right <= a.x_mm {
        a.x_mm - b_right
    } else {
        0.0
    };
    let dy = if a_bottom <= b.y_mm {
        b.y_mm - a_bottom
    } else if b_bottom <= a.y_mm {
        a.y_mm - b_bottom
    } else {
        0.0
    };

    if dx == 0.0 {
        dy
    } else if dy == 0.0 {
        dx
    } else {
        (dx * dx + dy * dy).sqrt()
    }
}

fn rows_cross_used(rows: &[VacuumRow], gap_mm: f64) -> f64 {
    if rows.is_empty() {
        return 0.0;
    }
    rows.iter().map(|row| row.cross_size_mm).sum::<f64>() + gap_mm * (rows.len() - 1) as f64
}

fn axis_spans(axis: VacuumDirection, usable_width: f64, usable_height: f64) -> (f64, f64) {
    match axis {
        VacuumDirection::Width => (usable_width, usable_height),
        VacuumDirection::Height => (usable_height, usable_width),
        VacuumDirection::Optimal => unreachable!("optimal is not a concrete axis"),
    }
}

fn oriented_size(part: &VacuumPart, rotated: bool) -> (f64, f64) {
    if rotated {
        (part.height_mm, part.width_mm)
    } else {
        (part.width_mm, part.height_mm)
    }
}

fn primary_size(axis: VacuumDirection, width_mm: f64, height_mm: f64) -> f64 {
    match axis {
        VacuumDirection::Width => width_mm,
        VacuumDirection::Height => height_mm,
        VacuumDirection::Optimal => unreachable!("optimal is not a concrete axis"),
    }
}

fn cross_size(axis: VacuumDirection, width_mm: f64, height_mm: f64) -> f64 {
    match axis {
        VacuumDirection::Width => height_mm,
        VacuumDirection::Height => width_mm,
        VacuumDirection::Optimal => unreachable!("optimal is not a concrete axis"),
    }
}

fn max_count_in_span(span: f64, size: f64, gap_mm: f64) -> usize {
    if size <= 0.0 || span + EPS < size {
        return 0;
    }
    (((span + gap_mm + EPS) / (size + gap_mm)).floor() as usize).max(1)
}

fn nearly_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= EPS
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn vacuum_search_budgets_have_hard_caps() {
        let mut advanced = VacuumSearchBudget::for_parts(5_000);
        assert_eq!(advanced.remaining, MAX_WIDTH_FIRST_PROBES);
        for _ in 0..MAX_WIDTH_FIRST_PROBES {
            assert!(advanced.probe());
        }
        assert!(!advanced.probe());
        assert!(advanced.exhausted);

        let baseline = VacuumSearchBudget::legacy();
        assert_eq!(baseline.remaining, MAX_LEGACY_FIT_CHECKS);
        assert!(!baseline.exhausted);
        assert_eq!(MAX_HOMOGENEOUS_COMBINATIONS, 25_000_000);
    }
}
