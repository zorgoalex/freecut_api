use utoipa::OpenApi;

use crate::models::VersionResponse;
use crate::models::{
    Artifacts, ErrorResponse, Item, LayoutMode, Objective, OptimizeRequest, OptimizeResponse,
    Params, PatternDirection, Placement, Rotation, Solution, StockItem, Summary, Trim, Units,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::optimize,
        crate::health_live,
        crate::health_ready,
        crate::version
    ),
    components(
        schemas(
            OptimizeRequest,
            Units,
            Params,
            LayoutMode,
            Trim,
            StockItem,
            Item,
            Rotation,
            PatternDirection,
            Objective,
            OptimizeResponse,
            Summary,
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
