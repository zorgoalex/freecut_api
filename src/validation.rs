use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::models::{ErrorResponse, Item, OptimizeRequest, Rotation, StockItem, Trim};

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

pub fn validate_request(req: &OptimizeRequest, limits: &ValidationLimits) -> Result<(), ValidationError> {
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
        return Err(ValidationError::new("stock exceeds max allowed types").with_details(
            serde_json::json!({"max_stock_types": limits.max_stock_types}),
        ));
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
        if !item_fits_any_stock(item, &req.params.trim_mm, &req.stock) {
            return Err(ValidationError::new("item does not fit any stock with trim").with_details(
                serde_json::json!({"item_id": item.id}),
            ));
        }
    }

    Ok(())
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
        return Err(ValidationError::new("trim exceeds stock dimensions").with_details(
            serde_json::json!({"stock_id": stock.id}),
        ));
    }
    Ok(())
}

fn item_fits_any_stock(item: &Item, trim: &Trim, stock: &[StockItem]) -> bool {
    stock.iter().any(|sheet| item_fits_stock(item, trim, sheet))
}

fn item_fits_stock(item: &Item, trim: &Trim, stock: &StockItem) -> bool {
    let usable_w = stock.width_mm - trim.left - trim.right;
    let usable_h = stock.height_mm - trim.top - trim.bottom;
    if usable_w <= 0.0 || usable_h <= 0.0 {
        return false;
    }

    if item.width_mm <= usable_w && item.height_mm <= usable_h {
        return true;
    }

    let can_rotate = item.rotation == Rotation::Allow90 && item.pattern_direction == crate::models::PatternDirection::None;
    can_rotate && item.height_mm <= usable_w && item.width_mm <= usable_h
}
