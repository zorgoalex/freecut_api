use utoipa::OpenApi;

use crate::models::VersionResponse;
use crate::models::{
    AlnsOperatorTelemetry, AlnsParams, AlnsTelemetry, Artifacts, BeamParams, BeamTelemetry,
    ErrorResponse, Item, LayoutMode, Objective, OptimizeRequest, OptimizeResponse, Params,
    PatternDirection, Placement, PortfolioParams, PortfolioTelemetry, Rotation, Solution,
    StockItem, Summary, Trim, Units,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::optimize,
        crate::optimize_beam,
        crate::optimize_alns,
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
            AlnsParams,
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
            AlnsTelemetry,
            AlnsOperatorTelemetry,
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
