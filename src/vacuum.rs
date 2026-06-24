use std::collections::HashSet;

use crate::models::{
    OptimizeRequest, PatternDirection, Placement, Rotation, Solution, UnplacedItem,
    VacuumDirection, VacuumTelemetry, VacuumUsedBbox,
};

const EPS: f64 = 1.0e-7;

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
    axis: VacuumDirection,
    strategy: &'static str,
    rows: Vec<VacuumRow>,
    score: VacuumScore,
}

#[derive(Clone, Copy)]
struct VacuumScore {
    placed_count: usize,
    unplaced_count: usize,
    coverage_ratio: f64,
    direction_penalty: u8,
    bbox_width_ratio: f64,
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

    let mut solutions = Vec::new();
    let mut chosen_direction = match requested_direction {
        VacuumDirection::Optimal => VacuumDirection::Width,
        direction => direction,
    };
    let mut strategy: Option<&'static str> = None;

    for sheet_index in 0..max_sheets {
        if remaining.is_empty() {
            break;
        }

        let Some(candidate) = choose_candidate(
            &remaining,
            usable_width,
            usable_height,
            gap_mm,
            requested_direction,
        ) else {
            break;
        };

        if candidate.score.placed_count == 0 {
            break;
        }

        if solutions.is_empty() {
            chosen_direction = candidate.axis;
        }
        strategy = Some(match strategy {
            None => candidate.strategy,
            Some(prev) if prev == candidate.strategy => prev,
            Some(_) => "mixed",
        });

        let placements = rows_to_placements(
            &candidate.rows,
            candidate.axis,
            usable_width,
            usable_height,
            gap_mm,
        );
        let placed_seq: HashSet<usize> = candidate
            .rows
            .iter()
            .flat_map(|row| row.parts.iter().map(|part| part.part.seq))
            .collect();
        remaining.retain(|part| !placed_seq.contains(&part.seq));

        solutions.push(Solution {
            stock_id: stock.id.clone(),
            index: sheet_index,
            width_mm: stock.width_mm,
            height_mm: stock.height_mm,
            trim_mm: trim,
            placements,
        });
    }

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
        strategy.unwrap_or("none"),
        usable_width,
        usable_height,
    );

    Ok(VacuumLayoutResult {
        solutions,
        unplaced_items,
        telemetry,
    })
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
) -> Option<VacuumCandidate> {
    let axes: &[VacuumDirection] = match requested_direction {
        VacuumDirection::Optimal => &[VacuumDirection::Width, VacuumDirection::Height],
        VacuumDirection::Width => &[VacuumDirection::Width],
        VacuumDirection::Height => &[VacuumDirection::Height],
    };
    let preferred_axis = preferred_optimal_axis(parts, requested_direction);

    let mut best: Option<VacuumCandidate> = None;
    for &axis in axes {
        if let Some(mut candidate) =
            pack_homogeneous(parts, axis, usable_width, usable_height, gap_mm)
        {
            apply_axis_preference(&mut candidate, preferred_axis);
            replace_if_better(&mut best, candidate);
        }
        for sorter in [
            VacuumSorter::Area,
            VacuumSorter::LongSide,
            VacuumSorter::ShortSide,
        ] {
            if let Some(mut candidate) =
                pack_general(parts, axis, usable_width, usable_height, gap_mm, sorter)
            {
                apply_axis_preference(&mut candidate, preferred_axis);
                replace_if_better(&mut best, candidate);
            }
        }
    }
    best
}

fn preferred_optimal_axis(
    parts: &[VacuumPart],
    requested_direction: VacuumDirection,
) -> Option<VacuumDirection> {
    if requested_direction != VacuumDirection::Optimal {
        return None;
    }
    if parts.iter().all(|part| part.can_rotate) {
        Some(VacuumDirection::Height)
    } else {
        Some(VacuumDirection::Width)
    }
}

fn apply_axis_preference(candidate: &mut VacuumCandidate, preferred_axis: Option<VacuumDirection>) {
    candidate.score.direction_penalty = match preferred_axis {
        Some(axis) if axis != candidate.axis => 1,
        _ => 0,
    };
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

    let counts = best_homogeneous_row_counts(&row_types, parts.len(), cross_span, gap_mm)?;
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
) -> Option<Vec<usize>> {
    let mut best_counts: Option<Vec<usize>> = None;
    let mut best_key: Option<(usize, usize, usize, usize)> = None;

    let mut visit = |counts: Vec<usize>| {
        let rows_total: usize = counts.iter().sum();
        if rows_total == 0 {
            return;
        }
        let capacity_total: usize = counts
            .iter()
            .zip(row_types.iter())
            .map(|(count, row_type)| count * row_type.capacity)
            .sum();
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
            for a_count in 1..=a.max_rows {
                visit(vec![a_count]);
            }
        }
        [a, b] => {
            for a_count in 0..=a.max_rows {
                for b_count in 0..=b.max_rows {
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
            find_seed_part(&remaining, axis, primary_span, cross_remaining)
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
            let Some((idx, row_part)) =
                find_next_row_part(&remaining, axis, remaining_primary, row.cross_size_mm)
            else {
                break;
            };
            primary_used += gap_mm + primary_size(axis, row_part.width_mm, row_part.height_mm);
            remaining.remove(idx);
            row.parts.push(row_part);
        }

        rows.push(row);
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
) -> Option<(usize, RowPart)> {
    parts.iter().enumerate().find_map(|(idx, part)| {
        best_orientation(part, axis, primary_span, cross_remaining).map(|row_part| (idx, row_part))
    })
}

fn find_next_row_part(
    parts: &[VacuumPart],
    axis: VacuumDirection,
    primary_remaining: f64,
    row_cross_size: f64,
) -> Option<(usize, RowPart)> {
    parts.iter().enumerate().find_map(|(idx, part)| {
        best_orientation(part, axis, primary_remaining, row_cross_size)
            .map(|row_part| (idx, row_part))
    })
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
    let edge_offset = bbox.x_mm / usable_width.max(EPS) + bbox.y_mm / usable_height.max(EPS);

    Some(VacuumCandidate {
        axis,
        strategy,
        rows,
        score: VacuumScore {
            placed_count,
            unplaced_count: requested_count.saturating_sub(placed_count),
            coverage_ratio,
            direction_penalty: 0,
            bbox_width_ratio,
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
    if candidate.direction_penalty != current.direction_penalty {
        return candidate.direction_penalty < current.direction_penalty;
    }
    if (candidate.bbox_width_ratio - current.bbox_width_ratio).abs() > EPS {
        return candidate.bbox_width_ratio < current.bbox_width_ratio;
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
