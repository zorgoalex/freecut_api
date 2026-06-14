use utoipa::OpenApi;

use crate::models::VersionResponse;
use crate::models::{
    AlnsOperatorTelemetry, AlnsParams, AlnsTelemetry, Artifacts, BeamParams, BeamTelemetry,
    CandidateSelectionTelemetry, ErrorResponse, GaOverrideParams, GaProfile, GroupShiftParams,
    GroupShiftTelemetry, Item, LayoutMode, Objective, OptimizeRequest, OptimizeResponse, Params,
    PatternDirection, Placement, PortfolioParams, PortfolioTelemetry, RestartPolicyTelemetry,
    Rotation, SlaProfile, Solution, StockItem, Summary, Trim, Units,
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
            SlaProfile,
            GaProfile,
            GaOverrideParams,
            GroupShiftParams,
            Trim,
            StockItem,
            Item,
            Rotation,
            PatternDirection,
            Objective,
            OptimizeResponse,
            Summary,
            RestartPolicyTelemetry,
            PortfolioTelemetry,
            BeamTelemetry,
            AlnsTelemetry,
            AlnsOperatorTelemetry,
            CandidateSelectionTelemetry,
            GroupShiftTelemetry,
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
