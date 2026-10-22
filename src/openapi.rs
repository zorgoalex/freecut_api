use utoipa::OpenApi;

use crate::models::VersionResponse;
use crate::models::{
    AlnsOperatorTelemetry, AlnsParams, AlnsTelemetry, Artifacts, BeamParams, BeamTelemetry,
    CandidateSelectionTelemetry, ConsolidateParams, ConsolidateTelemetry, CutQuality, Engine,
    ErrorResponse, GaOverrideParams, GaProfile, GroupShiftParams, GroupShiftTelemetry, Item,
    LayoutMode, LnsParams, Objective, OptimizeRequest, OptimizeResponse, Params, PartitionParams,
    PartitionTelemetry, PatternDirection, Placement, PortfolioParams, PortfolioTelemetry,
    ProfilePoolParams, ProfilePoolPreset, ProfilePoolTelemetry, RemnantTelemetry,
    RestartPolicyTelemetry,
    RetryStrategy, RetryTelemetry, Rotation, SlaProfile, Solution, StockItem, Summary, Trim, Units,
    UnplacedItem, VacuumAlgorithm, VacuumDirection, VacuumParams, VacuumTelemetry, VacuumUsedBbox,
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
            VacuumParams,
            VacuumDirection,
            VacuumAlgorithm,
            PortfolioParams,
            BeamParams,
            AlnsParams,
            LayoutMode,
            Engine,
            CutQuality,
            SlaProfile,
            GaProfile,
            GaOverrideParams,
            GroupShiftParams,
            ProfilePoolParams,
            ProfilePoolPreset,
            PartitionParams,
            ConsolidateParams,
            LnsParams,
            RetryStrategy,
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
            ProfilePoolTelemetry,
            PartitionTelemetry,
            ConsolidateTelemetry,
            RetryTelemetry,
            RemnantTelemetry,
            VacuumTelemetry,
            VacuumUsedBbox,
            Solution,
            Placement,
            UnplacedItem,
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
