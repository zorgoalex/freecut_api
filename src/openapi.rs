use utoipa::OpenApi;

use crate::models::VersionResponse;
use crate::models::{
    Artifacts, BeamParams, BeamTelemetry, ErrorResponse, Item, LayoutMode, Objective,
    OptimizeRequest, OptimizeResponse, Params, PatternDirection, Placement, PortfolioParams,
    PortfolioTelemetry, Rotation, Solution, StockItem, Summary, Trim, Units,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::optimize,
        crate::optimize_beam,
        crate::health_live,
        crate::health_ready,
        crate::version
    ),
    components(
        schemas(
            OptimizeRequest,
            Units,
            Params,
            PortfolioParams,
            BeamParams,
            LayoutMode,
            Trim,
            StockItem,
            Item,
            Rotation,
            PatternDirection,
            Objective,
            OptimizeResponse,
            Summary,
            PortfolioTelemetry,
            BeamTelemetry,
            Solution,
            Placement,
            Artifacts,
            ErrorResponse,
            VersionResponse
        )
    ),
    tags(
        (name = "health", description = "Health checks"),
        (name = "optimize", description = "Cut optimization")
    )
)]
pub struct ApiDoc;
