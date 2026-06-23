use std::collections::HashSet;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::models::{ErrorResponse, Item, LayoutMode, OptimizeRequest, Rotation, StockItem, Trim};

#[derive(Debug, Clone)]
pub struct ValidationLimits {
    pub max_instances: u32,
    pub max_stock_types: usize,
}

#[derive(Debug)]
pub struct ValidationError {
    pub error_code: &'static str,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error_code: "VALIDATION_ERROR",
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl IntoResponse for ValidationError {
    fn into_response(self) -> Response {
        let body = ErrorResponse {
            status: "error",
            error_code: self.error_code,
            message: self.message,
            details: self.details,
        };
        (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response()
    }
}

pub fn validate_request(
    req: &OptimizeRequest,
    limits: &ValidationLimits,
) -> Result<(), ValidationError> {
    match req.units {
        crate::models::Units::Mm => {}
    }

    if req.stock.is_empty() {
        return Err(ValidationError::new("stock must have at least one entry"));
    }
    if req.items.is_empty() {
        return Err(ValidationError::new("items must have at least one entry"));
    }
    if req.stock.len() > limits.max_stock_types {
        return Err(ValidationError::new("stock exceeds max allowed types")
            .with_details(serde_json::json!({"max_stock_types": limits.max_stock_types})));
    }
    if matches!(req.params.layout_mode, Some(LayoutMode::VacuumTable)) && req.stock.len() != 1 {
        return Err(ValidationError::new(
            "layout_mode=vacuum_table requires exactly one stock entry",
        ));
    }
    let mut seen_stock_ids: HashSet<String> = HashSet::new();
    let mut duplicate_stock_ids: Vec<String> = Vec::new();
    for stock in &req.stock {
        if !seen_stock_ids.insert(stock.id.clone()) {
            duplicate_stock_ids.push(stock.id.clone());
        }
    }
    if !duplicate_stock_ids.is_empty() {
        duplicate_stock_ids.sort();
        duplicate_stock_ids.dedup();
        return Err(ValidationError::new("stock ids must be unique")
            .with_details(serde_json::json!({"duplicate_stock_ids": duplicate_stock_ids})));
    }

    if req.params.kerf_mm < 0.0 || req.params.spacing_mm < 0.0 {
        return Err(ValidationError::new("kerf_mm and spacing_mm must be >= 0"));
    }

    if req.params.trim_mm.left < 0.0
        || req.params.trim_mm.right < 0.0
        || req.params.trim_mm.top < 0.0
        || req.params.trim_mm.bottom < 0.0
    {
        return Err(ValidationError::new("trim_mm values must be >= 0"));
    }

    if let Some(time_limit_ms) = req.params.time_limit_ms {
        if time_limit_ms < 100 {
            return Err(ValidationError::new("time_limit_ms must be >= 100"));
        }
    }

    if let Some(restarts) = req.params.restarts {
        if restarts < 1 {
            return Err(ValidationError::new("restarts must be >= 1"));
        }
    }

    if let Some(ga) = &req.params.ga_override {
        if let Some(epochs) = ga.epochs {
            if !(1..=2000).contains(&epochs) {
                return Err(ValidationError::new(
                    "ga_override.epochs must be in range 1..=2000",
                ));
            }
        }
        if let Some(breed_factor) = ga.breed_factor {
            if !breed_factor.is_finite() || breed_factor <= 0.0 || breed_factor > 1.0 {
                return Err(ValidationError::new(
                    "ga_override.breed_factor must be finite and in range (0, 1]",
                ));
            }
        }
        if let Some(survival_factor) = ga.survival_factor {
            if !survival_factor.is_finite() || !(0.0..=1.0).contains(&survival_factor) {
                return Err(ValidationError::new(
                    "ga_override.survival_factor must be finite and in range [0, 1]",
                ));
            }
        }
        if let Some(top_k) = ga.top_k_candidates {
            if !(1..=64).contains(&top_k) {
                return Err(ValidationError::new(
                    "ga_override.top_k_candidates must be in range 1..=64",
                ));
            }
        }
        if let Some(zone_penalty) = ga.zone_penalty {
            if !zone_penalty.is_finite() || !(0.0..=1.0).contains(&zone_penalty) {
                return Err(ValidationError::new(
                    "ga_override.zone_penalty must be finite and in range [0, 1]",
                ));
            }
        }
        if let Some(fill_penalty) = ga.fill_penalty {
            if !fill_penalty.is_finite() || !(0.0..=1.0).contains(&fill_penalty) {
                return Err(ValidationError::new(
                    "ga_override.fill_penalty must be finite and in range [0, 1]",
                ));
            }
        }
    }

    if let Some(pool) = &req.params.profile_pool {
        let pool_enabled = pool.enabled.unwrap_or(true);
        if pool_enabled {
            if let Some(zone_penalties) = &pool.zone_penalties {
                if zone_penalties.is_empty() || zone_penalties.len() > 8 {
                    return Err(ValidationError::new(
                        "profile_pool.zone_penalties must contain 1..=8 values",
                    ));
                }
                for zone_penalty in zone_penalties {
                    if !zone_penalty.is_finite() || !(0.0..=1.0).contains(zone_penalty) {
                        return Err(ValidationError::new(
                            "profile_pool.zone_penalties values must be finite and in range [0, 1]",
                        ));
                    }
                }
            }
            if let Some(zone_penalties) = &pool.rescue_zone_penalties {
                if zone_penalties.is_empty() || zone_penalties.len() > 8 {
                    return Err(ValidationError::new(
                        "profile_pool.rescue_zone_penalties must contain 1..=8 values",
                    ));
                }
                for zone_penalty in zone_penalties {
                    if !zone_penalty.is_finite() || !(0.0..=1.0).contains(zone_penalty) {
                        return Err(ValidationError::new(
                            "profile_pool.rescue_zone_penalties values must be finite and in range [0, 1]",
                        ));
                    }
                }
            }
            if let Some(fill_penalty) = pool.fill_penalty {
                if !fill_penalty.is_finite() || !(0.0..=1.0).contains(&fill_penalty) {
                    return Err(ValidationError::new(
                        "profile_pool.fill_penalty must be finite and in range [0, 1]",
                    ));
                }
            }
            if let Some(max_lead_drop_pp) = pool.max_lead_drop_pp {
                if !max_lead_drop_pp.is_finite() || !(0.0..=10.0).contains(&max_lead_drop_pp) {
                    return Err(ValidationError::new(
                        "profile_pool.max_lead_drop_pp must be finite and in range [0, 10]",
                    ));
                }
            }
            if let Some(seed_offsets) = &pool.seed_offsets {
                if seed_offsets.is_empty() || seed_offsets.len() > 8 {
                    return Err(ValidationError::new(
                        "profile_pool.seed_offsets must contain 1..=8 positive values",
                    ));
                }
                if seed_offsets.iter().any(|offset| *offset == 0) {
                    return Err(ValidationError::new(
                        "profile_pool.seed_offsets values must be positive",
                    ));
                }
            }
            if let Some(corner_threshold) = pool.rescue_when_max_corner_below_mm2 {
                if !corner_threshold.is_finite() || corner_threshold < 0.0 {
                    return Err(ValidationError::new(
                        "profile_pool.rescue_when_max_corner_below_mm2 must be finite and >= 0",
                    ));
                }
            }
            if let Some(corner_threshold) = pool.rescue_accept_min_max_corner_mm2 {
                if !corner_threshold.is_finite() || corner_threshold < 0.0 {
                    return Err(ValidationError::new(
                        "profile_pool.rescue_accept_min_max_corner_mm2 must be finite and >= 0",
                    ));
                }
            }
        }
    }

    if let Some(group_shift) = &req.params.group_shift {
        let group_shift_enabled = group_shift.enabled.unwrap_or(true);
        if group_shift_enabled {
            if let Some(min_shift_mm) = group_shift.min_shift_mm {
                if !min_shift_mm.is_finite() || min_shift_mm < 0.0 {
                    return Err(ValidationError::new(
                        "group_shift.min_shift_mm must be finite and >= 0",
                    ));
                }
            }
            if let Some(max_passes) = group_shift.max_passes {
                if !(1..=16).contains(&max_passes) {
                    return Err(ValidationError::new(
                        "group_shift.max_passes must be in range 1..=16",
                    ));
                }
            }
        }
    }

    if let Some(portfolio) = &req.params.portfolio {
        let portfolio_enabled = portfolio.enabled.unwrap_or(true);
        if portfolio_enabled {
            if let Some(deadline_ms) = portfolio.deadline_ms {
                if deadline_ms < 100 {
                    return Err(ValidationError::new("portfolio.deadline_ms must be >= 100"));
                }
            }
            if let Some(candidate_count) = portfolio.candidate_count {
                if candidate_count < 1 || candidate_count > 16 {
                    return Err(ValidationError::new(
                        "portfolio.candidate_count must be in range 1..=16",
                    ));
                }
            }
        }
    }

    if let Some(beam) = &req.params.beam {
        let beam_enabled = beam.enabled.unwrap_or(true);
        if beam_enabled {
            if let Some(deadline_ms) = beam.deadline_ms {
                if deadline_ms < 100 {
                    return Err(ValidationError::new("beam.deadline_ms must be >= 100"));
                }
            }
            if let Some(beam_width) = beam.beam_width {
                if beam_width < 1 || beam_width > 8 {
                    return Err(ValidationError::new(
                        "beam.beam_width must be in range 1..=8",
                    ));
                }
            }
            if let Some(beam_depth) = beam.beam_depth {
                if beam_depth < 1 || beam_depth > 8 {
                    return Err(ValidationError::new(
                        "beam.beam_depth must be in range 1..=8",
                    ));
                }
            }
            if let Some(branch_factor) = beam.branch_factor {
                if branch_factor < 1 || branch_factor > 8 {
                    return Err(ValidationError::new(
                        "beam.branch_factor must be in range 1..=8",
                    ));
                }
            }
        }
    }

    if let Some(alns) = &req.params.alns {
        let alns_enabled = alns.enabled.unwrap_or(true);
        if alns_enabled {
            if let Some(deadline_ms) = alns.deadline_ms {
                if deadline_ms < 100 {
                    return Err(ValidationError::new("alns.deadline_ms must be >= 100"));
                }
            }
            if let Some(iterations) = alns.iterations {
                if iterations < 1 || iterations > 512 {
                    return Err(ValidationError::new(
                        "alns.iterations must be in range 1..=512",
                    ));
                }
            }
            if let Some(segment_size) = alns.segment_size {
                if segment_size < 1 || segment_size > 64 {
                    return Err(ValidationError::new(
                        "alns.segment_size must be in range 1..=64",
                    ));
                }
            }
            if let Some(temperature_start) = alns.temperature_start {
                if !temperature_start.is_finite() || temperature_start <= 0.0 {
                    return Err(ValidationError::new(
                        "alns.temperature_start must be finite and > 0",
                    ));
                }
            }
            if let Some(temperature_end) = alns.temperature_end {
                if !temperature_end.is_finite() || temperature_end <= 0.0 {
                    return Err(ValidationError::new(
                        "alns.temperature_end must be finite and > 0",
                    ));
                }
            }
            if let (Some(ts), Some(te)) = (alns.temperature_start, alns.temperature_end) {
                if te > ts {
                    return Err(ValidationError::new(
                        "alns.temperature_end must be <= alns.temperature_start",
                    ));
                }
            }
            if let Some(reaction_factor) = alns.reaction_factor {
                if !reaction_factor.is_finite() || reaction_factor <= 0.0 || reaction_factor > 1.0 {
                    return Err(ValidationError::new(
                        "alns.reaction_factor must be finite and in range (0, 1]",
                    ));
                }
            }
        }
    }

    for stock in &req.stock {
        validate_stock(stock)?;
        validate_trim_against_stock(&req.params.trim_mm, stock)?;
    }

    let total_instances: u32 = req.items.iter().map(|item| item.qty).sum();
    if total_instances > limits.max_instances {
        return Err(ValidationError {
            error_code: "CONSTRAINT_ERROR",
            message: "total item instances exceed max".to_string(),
            details: Some(serde_json::json!({
                "max_instances": limits.max_instances,
                "total_instances": total_instances
            })),
        });
    }

    for item in &req.items {
        validate_item(item)?;
        // Note: items that don't fit are handled in optimizer (returned as unplaced_items)
        // We don't reject the whole request here
    }

    Ok(())
}

/// Check if an item fits any stock (considering trim, gap, and rotation)
/// gap_mm = kerf_mm + spacing_mm (space needed around the item for cutting)
pub fn item_fits_any_stock_public(
    item: &Item,
    trim: &Trim,
    gap_mm: f64,
    stock: &[StockItem],
) -> bool {
    stock
        .iter()
        .any(|sheet| item_fits_stock_with_gap(item, trim, gap_mm, sheet))
}

fn validate_stock(stock: &StockItem) -> Result<(), ValidationError> {
    if stock.width_mm <= 0.0 || stock.height_mm <= 0.0 {
        return Err(ValidationError::new("stock dimensions must be > 0"));
    }
    // qty: None or 0 means unlimited sheets, which is valid
    Ok(())
}

fn validate_item(item: &Item) -> Result<(), ValidationError> {
    if item.width_mm <= 0.0 || item.height_mm <= 0.0 {
        return Err(ValidationError::new("item dimensions must be > 0"));
    }
    if item.qty < 1 {
        return Err(ValidationError::new("item qty must be >= 1"));
    }
    Ok(())
}

fn validate_trim_against_stock(trim: &Trim, stock: &StockItem) -> Result<(), ValidationError> {
    let usable_w = stock.width_mm - trim.left - trim.right;
    let usable_h = stock.height_mm - trim.top - trim.bottom;
    if usable_w <= 0.0 || usable_h <= 0.0 {
        return Err(ValidationError::new("trim exceeds stock dimensions")
            .with_details(serde_json::json!({"stock_id": stock.id})));
    }
    Ok(())
}

/// Check if item fits stock considering trim, gap (kerf+spacing), and rotation
fn item_fits_stock_with_gap(item: &Item, trim: &Trim, gap_mm: f64, stock: &StockItem) -> bool {
    let usable_w = stock.width_mm - trim.left - trim.right;
    let usable_h = stock.height_mm - trim.top - trim.bottom;
    if usable_w <= 0.0 || usable_h <= 0.0 {
        return false;
    }

    // Item needs gap for the cut on each side that's not against sheet edge
    // For single item check, we account for gap on right and bottom (item placed top-left)
    let item_w_with_gap = item.width_mm + gap_mm;
    let item_h_with_gap = item.height_mm + gap_mm;

    if item_w_with_gap <= usable_w && item_h_with_gap <= usable_h {
        return true;
    }

    // Check rotated orientation
    let can_rotate = item.rotation == Rotation::Allow90
        && item.pattern_direction == crate::models::PatternDirection::None;
    if can_rotate {
        let rotated_w_with_gap = item.height_mm + gap_mm;
        let rotated_h_with_gap = item.width_mm + gap_mm;
        if rotated_w_with_gap <= usable_w && rotated_h_with_gap <= usable_h {
            return true;
        }
    }

    false
}
