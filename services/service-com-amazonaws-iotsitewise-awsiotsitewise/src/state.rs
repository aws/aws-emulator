// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotsitewise-awsiotsitewise service.
//!
//! The asset-model / asset / property-value, gateway, dataset and Monitor-portal
//! slices of IoT SiteWise live here: one map per resource plus the indexes the
//! modelled lookup forms need
//! (`externalId:<id>` labels, property aliases, a gateway's platform device). A
//! resource's tags live on that resource's own record, reached by ARN through
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource`], so the
//! tagging operations and the creates share one tag set per resource. A data
//! stream no asset property owns lives in its own map
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::disassociated_time_series`]),
//! which is what `DisassociateTimeSeriesFromAssetProperty` leaves behind and what
//! alias-addressed ingestion creates. A workspace's pipelines live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::pipelines`], keyed by the
//! workspace and the pipeline name, each record carrying every version written
//! to it, bounded by [`MAX_PIPELINE_VERSIONS`], so a read naming an older
//! version still answers from it. A workspace's tasks live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tasks`], keyed and versioned
//! the same way, bounded by [`MAX_TASK_VERSIONS`]. A workspace's dataset export
//! jobs live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dataset_export_jobs`], keyed by
//! the workspace and the minted job id, each record carrying the request that
//! started it and the instant it started, from which a read DERIVES how far the
//! job has got. The data segments a dataset holds live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dataset_data_segments`], keyed
//! by the dataset, each record naming the dataset the data came from, the time
//! series it covers and the half-open span it covers of it, which is what a
//! listing reports and a deletion addresses one by. A workspace's searches live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::searches`], keyed by the
//! workspace and the search id, each record carrying the request the search was
//! started from and the instants its lifecycle transitions fall due, so the
//! status a poll reports is derived rather than stored. A workspace's
//! asynchronous queries live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::queries`], keyed the same way,
//! each record carrying what running its statement produced, the instant it was
//! submitted and the instant a cancel was recorded on it, from which a read
//! DERIVES the status the clock has made due. A
//! workspace's enrichment jobs live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::enrichment_jobs`], keyed by the
//! workspace and the minted job id, each record carrying the configuration it
//! was created from, the instant it was created and the instant a cancel was
//! recorded on it, from which a read DERIVES
//! the job's type, the data it analyses and the status the clock has made due. An
//! execution of an action lives in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::executions`], keyed by its own
//! id, which is what `ListExecutions` summarises for the target resource a
//! request names. An action taken on a target resource lives in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::actions`], keyed by its own
//! `actionId`, which is what `ListActions` summarises for the target resource a
//! request names. The service's own settings live here too:
//! the logging options, one entry per scope a call named
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::logging_options`]), and the
//! account's default encryption configuration
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::default_encryption`]). A
//! workspace's capture sources — the video streams `GetCaptureData` reads, and
//! the time series that workspace holds — live
//! in [`ComAmazonawsIotsitewiseAwsiotsitewiseState::capture_sources`], keyed by
//! the workspace and the source's `timeSeriesId`, with
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::capture_source_aliases`] the
//! index the alias form of that lookup resolves through. The account's
//! computation models live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::computation_models`], keyed by
//! `computationModelId`, each record carrying the configuration and data
//! binding it was created with and the actions that configuration's computation
//! type exposes. The Monitor access
//! policies live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::access_policies`], keyed by the
//! minted `accessPolicyId`, each record naming the identity, the Monitor
//! resource and the permission the last write to it stated. An interface asset
//! model applied to an asset model lives in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::interface_relationships`],
//! keyed by the two models, carrying the property and hierarchy mappings the
//! application resolved to. The applications of a workspace live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::applications`], keyed by the
//! minted `applicationId` — which identifies one across every workspace — each
//! record naming the workspace it was created in, the Identity Center instance
//! the create named and the instant the `CREATING` -> `ACTIVE` transition it
//! owes falls due. The Monitor dashboards live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dashboards`], keyed by the
//! minted `dashboardId` — which identifies one across every project — each
//! record naming the project it was created in, holding the definition
//! literal the last write to it stated and the idempotency token its create
//! carried, which is what a retry of that create is recognised by. The
//! account's bulk import jobs live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::bulk_import_jobs`], keyed by
//! the minted `jobId` — which identifies one across the account — each record
//! carrying the request that started it and the instant it was created, from
//! which a read DERIVES the status and the last-update date the clock has made
//! due. The mutating calls already served under a `clientToken` live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_calls`], keyed by
//! the operation and the token, each entry naming the request that spent the
//! token and the resource that call acted on, which is what a retry of it
//! replays from. Every
//! handler reads and writes THIS struct through [`service_state`]; nothing in
//! `src/ops/` keeps state of its own.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ActionDefinition, ActionPayload, ActionSummary, AggregatedValue, Aggregates, Alarms,
    AssetCompositeModelPathSegment, AssetCompositeModelSummary, AssetHierarchy,
    AssetModelCompositeModel,
    AssetModelCompositeModelPathSegment, AssetModelCompositeModelSummary, AssetModelHierarchy,
    AssetModelProperty, AssetModelPropertyDefinition, AssetModelStatus, AssetProperty,
    AssetPropertyPathSegment,
    AssetPropertyValue, AssetStatus, AssetSummary, ComputationModelConfiguration,
    ComputationModelDataBindingValue, ComputationModelStatus, ComputeNode,
    ComputeNodeExecutionDetails,
    ComputeNodeExecutionStatus, ConfigurationStatus, CustomerManagedS3Storage,
    DataSegmentEnrichment, DatasetConfig, DatasetEnrichment, DatasetEnrichmentEntry,
    DatasetSource, DatasetStatus, EnrichmentJobConfiguration, EnrichmentJobSummary,
    ErrorReportLocation,
    ExecutionEnvironmentVariables, ExecutionStatus, ExecutionSummary, ExportErrorReportLocation,
    ExpressionVariable, File, FileFormat, GatewayCapabilitySummary, GatewayPlatform,
    HierarchyMapping, Identity,
    ImageFile, ImageLocation, JobConfiguration, LoggingOptions, Mount, MountOverrides,
    MultiLayerStorage,
    PipelineExecutionStateDetails, PipelineExecutionStatus, PortalStatus,
    PortalTypeEntry, ProcessingInput, PropertyMapping, PropertyType, QueryStatistics, QuerySummary,
    ResolveTo, Resource, ResourceStatus, RetentionPeriod, SearchFilters, TargetResource,
    TaskConfiguration, TimeInNanos, TimeSeriesSummary, TrimSettings, Variant,
    WarmTierRetentionPeriod,
    WorkspaceEncryptionConfiguration, WorkspaceEncryptionConfigurationInfo, WorkspaceStatus,
    DISASSOCIATED_DATA_STORAGE_STATE_VALUES, ENCRYPTION_TYPE_VALUES, STORAGE_TYPE_VALUES,
    VIDEO_DATA_TYPE_VALUES, WARM_TIER_STATE_VALUES,
};
// The store is ONE struct; its parts live beside it, each file holding one
// concern, and every name they declare is re-exported here, so a handler goes on
// reaching all of it as `crate::state::…`.
//
// * `store` — the inherent methods of
//   [`ComAmazonawsIotsitewiseAwsiotsitewiseState`] itself: the resolution,
//   materialisation and propagation every handler asks the store for.
// * `asset_records` — the asset-model and asset records, and the time-series and
//   capture records an asset's samples live in.
// * `capture_ingest` — how an import's `mp4` files become the video a
//   workspace's capture sources hold.
// * `workspace_records` — the gateway-capability, dataset, data-segment,
//   workspace and compute-node-execution records.
// * `pipeline_task_records` — the pipeline, portal, project, access-policy,
//   search, task, action and execution records.
// * `computation_models` / `storage_configuration` / `jobs` — the computation
//   model records, the storage and encryption configuration, and the bulk
//   import, export and enrichment job records.
// * `property_formulas` — what a write holds a property definition to before
//   anything is stored: its name, its variables, its formula and its window.
// * `property_computation` — what the series of a `transform` or `metric`
//   property IS: the values its formula computes from the properties its
//   variables name, over the tumbling-window grid and the formula language its
//   own two parts hold.
// * `paths` — the structured `path` a read reports for a property or a composite
//   model, the notification block a reported asset property carries, and which
//   of a model's composite models are built-in.
mod asset_records;
mod capture_ingest;
mod computation_models;
mod id_references;
mod jobs;
mod paths;
mod pipeline_task_records;
mod property_computation;
mod property_formulas;
mod storage_configuration;
mod store;
mod workspace_records;

pub use asset_records::*;
pub use capture_ingest::*;
pub use computation_models::*;
pub use id_references::*;
pub use jobs::*;
pub use paths::*;
pub use pipeline_task_records::*;
pub use property_computation::*;
pub use property_formulas::*;
pub use storage_configuration::*;
pub use workspace_records::*;

/// The `iotsitewise` ARN service namespace, as `OPERATIONS.md` reports it.
pub const ARN_SERVICE: &str = "iotsitewise";

/// The region every ARN falls back to when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it this emulator takes to settle a
/// resource's owed transition — `CREATING` -> `ACTIVE` after a create, and
/// `UPDATING` -> `ACTIVE` after an update.
///
/// CHOSEN, not documented: AWS publishes no duration for these transitions, only
/// that `CreateAsset`/`CreateAssetModel` answer with the resource in `CREATING`,
/// that `UpdateAsset` answers with it in `UPDATING`, and that it becomes `ACTIVE`
/// later. The span is deliberately non-zero so a caller that reads the resource
/// straight after the call still observes the transient state the real service
/// reports.
pub const ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// `AssetState`/`AssetModelState` value a freshly created resource reports.
pub const STATE_CREATING: &str = "CREATING";
/// `AssetState`/`AssetModelState` value a settled resource reports.
pub const STATE_ACTIVE: &str = "ACTIVE";
/// `AssetState`/`AssetModelState` value a resource reports while a mutating call
/// it already accepted settles.
pub const STATE_UPDATING: &str = "UPDATING";
/// `AssetState`/`AssetModelState` value a deleted resource's last answer carries.
pub const STATE_DELETING: &str = "DELETING";
/// The state value a resource whose provisioning did not complete reports. This
/// emulator settles every owed transition, so nothing it holds reaches this
/// state; the operations whose rules are stated in terms of it still have to
/// recognise it (`UpdateWorkspace` may be called on a workspace in it).
pub const STATE_FAILED: &str = "FAILED";

/// The `LoggingLevel` a scope reports until a `PutLoggingOptions` sets one.
///
/// Logging is a setting, not a resource: a scope nobody has configured is simply
/// not logging, and `OFF` is the modelled verbosity level for that. A read of an
/// unset scope reports it WITHOUT storing anything, so the first read cannot
/// change what a later caller sees.
pub const DEFAULT_LOGGING_LEVEL: &str = "OFF";

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::logging_options`] holds
/// the account-and-region-wide logging options under — the ones a call that names
/// no `workspaceName` sets and reads.
///
/// The empty string, which collides with no workspace-scoped entry: the modelled
/// `WorkspaceName` is `len 1..64`, so no workspace can be named by it.
pub const ACCOUNT_LOGGING_SCOPE: &str = "";

/// How many characters an `ETag` carries: the modelled type is
/// `^[\w-]{43}$`, so every tag this service reports is exactly this long.
pub const ETAG_LENGTH: usize = 43;

/// The composite model types `assetModelCompositeModelType` accepts.
///
/// The member is modelled as a free `Name` string rather than an enum, so the
/// catalog comes from its documentation, which both
/// `CreateAssetModelCompositeModel` and `DescribeAssetModelCompositeModel` state
/// as "The composite model type. Valid values are AWS/ALARM, CUSTOM, or
/// AWS/L4E_ANOMALY".
pub const COMPOSITE_MODEL_TYPES: [&str; 3] = ["AWS/ALARM", "CUSTOM", "AWS/L4E_ANOMALY"];

/// The `AssetModelType` value an asset model must carry to be reused as the
/// component of a component-model-based composite model, as
/// `CreateAssetModelCompositeModel` documents it: "specify the
/// composedAssetModelId of an existing asset model with assetModelType of
/// COMPONENT_MODEL".
pub const MODEL_TYPE_COMPONENT: &str = "COMPONENT_MODEL";

/// How many property definitions one composite model may carry.
///
/// A PUBLISHED AWS quota, stated by `assetModelCompositeModelProperties`: "You
/// can specify up to 200 properties per composite model."
pub const MAX_PROPERTIES_PER_COMPOSITE_MODEL: usize = 200;

/// The ceiling on how many composite models one asset model holds.
///
/// The same bound, and for the same reason, as [`MAX_ASSET_MODELS`]: a create is
/// the only thing that adds a composite model and nothing evicts one, so the
/// per-model list needs a maximum entry count. The emulator's own bound, not a
/// published AWS default, and high enough that no ordinary workflow meets it;
/// `CreateAssetModelCompositeModel` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410).
pub const MAX_COMPOSITE_MODELS_PER_ASSET_MODEL: usize = 100;

/// How many property definitions one asset model may carry.
///
/// A PUBLISHED AWS quota, stated by both `CreateAssetModel`'s and
/// `UpdateAssetModel`'s `assetModelProperties` member: "You can specify up to 200
/// properties per asset model." A write that would leave a model carrying more is
/// refused with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_PROPERTIES_PER_ASSET_MODEL: usize = 200;

/// How many hierarchy definitions one asset model may carry.
///
/// A PUBLISHED AWS quota, stated by both `CreateAssetModel`'s and
/// `UpdateAssetModel`'s `assetModelHierarchies` member: "You can specify up to 10
/// hierarchies per asset model." Refused the same way as
/// [`MAX_PROPERTIES_PER_ASSET_MODEL`].
pub const MAX_HIERARCHIES_PER_ASSET_MODEL: usize = 10;

/// The `AssetModelVersionType` value naming a model's latest version, and the
/// documented default of every `matchForVersionType` member.
pub const VERSION_TYPE_LATEST: &str = "LATEST";

/// The `AssetModelVersionType` value naming a model's active version.
pub const VERSION_TYPE_ACTIVE: &str = "ACTIVE";

/// The one value a `SelectAll` member (`ifNoneMatch`) admits: its modelled
/// pattern is `^\*$`.
pub const SELECT_ALL: &str = "*";

/// The prefix a `CustomID` label uses to name a resource by its external id.
pub const EXTERNAL_ID_PREFIX: &str = "externalId:";

/// The ceiling on how many asset models and how many assets this store holds:
/// the two caps that bound caller-driven growth of the resource maps.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum
/// entry count — neither a TTL nor an eviction policy is available to it, because
/// `DescribeAssetModel`/`DescribeAsset` must still answer what the create created.
/// `CreateAssetModel` and `CreateAsset` refuse past these caps with the modelled
/// `LimitExceededException` (HTTP 410), which IoT SiteWise declares on exactly the
/// operations that grow the store and on no read. The numbers are the emulator's
/// own bound, not a published AWS default: AWS publishes both quotas as adjustable,
/// so no single value is the contract, and the caps are set high enough that no
/// ordinary workflow meets one.
pub const MAX_ASSET_MODELS: usize = 1_000;

/// See [`MAX_ASSET_MODELS`].
pub const MAX_ASSETS: usize = 10_000;

/// The greatest number of tags one IoT SiteWise resource holds.
///
/// The model's own ceiling: `TagMap` carries `@length(min: 1, max: 50)` wherever
/// it is used — `TagResource.tags`, `CreateAsset.tags`, `CreateAssetModel.tags`
/// and `ListTagsForResource.tags` — so no single call may carry more than this
/// many tags and no listing may report more.
///
/// The two refusals this bound drives are DIFFERENT failures and carry different
/// modelled errors. A single call carrying more than this many tags violates
/// `TagMap`'s own `@length`, which the boundary validator refuses before a handler
/// runs. A well-formed call whose tags would carry the TARGET RESOURCE's
/// accumulated count past the ceiling is a quota refusal, so it answers the
/// modelled `LimitExceededException` (HTTP 410) — "You've reached the quota for a
/// resource" — which is the error IoT SiteWise declares for every accumulated-count
/// ceiling in this service.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The `TagKey` bounds: `TagMap`'s key targets `TagKey`, which the model
/// declares as `@length(min: 1, max: 128)`.
pub const TAG_KEY_MIN_LENGTH: usize = 1;
/// See [`TAG_KEY_MIN_LENGTH`].
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// The ARN resource-type segment of an asset ARN, as [`asset_arn`] builds it.
pub const ASSET_RESOURCE_TYPE: &str = "asset";
/// The ARN resource-type segment of an asset-model ARN, as [`asset_model_arn`]
/// builds it.
pub const ASSET_MODEL_RESOURCE_TYPE: &str = "asset-model";
/// The ARN resource-type segment of a gateway ARN, as [`gateway_arn`] builds it.
pub const GATEWAY_RESOURCE_TYPE: &str = "gateway";
/// The ARN resource-type segment of a portal ARN, as [`portal_arn`] builds it.
pub const PORTAL_RESOURCE_TYPE: &str = "portal";
/// The ARN resource-type segment of a project ARN, as [`project_arn`] builds it.
pub const PROJECT_RESOURCE_TYPE: &str = "project";
/// The ARN resource-type segment of a pipeline ARN, as [`pipeline_arn`] builds it.
pub const PIPELINE_RESOURCE_TYPE: &str = "pipeline";

/// The ARN service namespace a workspace's customer managed key lives under.
pub const KMS_ARN_SERVICE: &str = "kms";

/// How many pipelines one workspace holds.
///
/// The same kind of bound, and for the same reason, as [`MAX_GATEWAYS`]: a create
/// is the only thing that adds a pipeline and nothing evicts one, so the map needs
/// a maximum entry count. The emulator's own bound, not a published AWS default,
/// and high enough that no ordinary workflow meets it; `CreatePipeline` refuses
/// past it with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_PIPELINES_PER_WORKSPACE: usize = 100;

/// How many pipelines the store holds in total, across every workspace.
///
/// The per-workspace cap above does not bound the store on its own: it bounds one
/// workspace's pipelines, and the number of workspaces is bounded in turn by
/// [`MAX_WORKSPACES`], so this cap is what states the whole map's own limit
/// directly. This is the whole map's entry count, refused past the cap with the
/// modelled `LimitExceededException` (HTTP 410) — the same bound, and for the same
/// reason, as [`MAX_GATEWAYS`].
pub const MAX_PIPELINES: usize = 1_000;

/// How many versions one pipeline keeps.
///
/// Unlike the entry-count caps above, a pipeline's version history HAS an
/// eviction policy available to it: a read answers from the newest version unless
/// it names an older one, so the OLDEST superseded version is the one that can go.
/// Past this many versions [`PipelineRecord::record_update`] drops the oldest, so
/// repeated updates cannot grow one record without limit, and a read naming an
/// evicted version is the same not-found answer as one naming a version no write
/// ever produced. The emulator's own bound; AWS publishes no retained-version
/// count.
pub const MAX_PIPELINE_VERSIONS: usize = 50;

/// The `version` a freshly created pipeline reports: its first version, counted
/// from one, as `CreatePipeline`'s `version` member ("The version of the newly
/// created pipeline") and `UpdatePipeline`'s ("The new version of the pipeline
/// created by this update") describe the counter.
pub const FIRST_PIPELINE_VERSION: u64 = 1;

/// The ARN resource-type segment of a task ARN, as [`task_arn`] builds it.
pub const TASK_RESOURCE_TYPE: &str = "task";

/// How many tasks one workspace holds.
///
/// The same kind of bound, and for the same reason, as
/// [`MAX_PIPELINES_PER_WORKSPACE`]: a create is the only thing that adds a task
/// and nothing evicts one, so the map needs a maximum entry count. The
/// emulator's own bound, not a published AWS default; `CreateTask` refuses past
/// it with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_TASKS_PER_WORKSPACE: usize = 100;

/// How many tasks the store holds in total, across every workspace.
///
/// The whole map's own limit, for the reason [`MAX_PIPELINES`] states: the
/// per-workspace cap above bounds one workspace's tasks, and this one bounds the
/// map every workspace's tasks share.
pub const MAX_TASKS: usize = 1_000;

/// How many versions one task keeps.
///
/// The same eviction policy, and for the same reason, as
/// [`MAX_PIPELINE_VERSIONS`]: a read answers from the newest version unless it
/// names an older one, so past this many versions
/// [`TaskRecord::record_update`] drops the OLDEST superseded version and no
/// number of updates grows one record without limit. The emulator's own bound;
/// AWS publishes no retained-version count.
pub const MAX_TASK_VERSIONS: usize = 50;

/// The `version` a freshly created task reports: its first version, counted from
/// one, as `CreateTask`'s `version` member ("The version of the newly created
/// task") and `UpdateTask`'s ("The new version of the task created by this
/// update") describe the counter.
pub const FIRST_TASK_VERSION: u64 = 1;

/// The ARN resource-type segment of a dataset ARN, as [`dataset_arn`] builds it.
pub const DATASET_RESOURCE_TYPE: &str = "dataset";
/// The ARN resource-type segment of a workspace ARN, as [`workspace_arn`] builds
/// it.
pub const WORKSPACE_RESOURCE_TYPE: &str = "workspace";
/// The ARN resource-type segment of an access policy's ARN, as
/// [`access_policy_arn`] builds it.
pub const ACCESS_POLICY_RESOURCE_TYPE: &str = "access-policy";

/// See [`MAX_ASSET_MODELS`]: the same bound, for the access-policy map.
/// `CreateAccessPolicy` is the only thing that adds an access policy and it
/// declares the modelled `LimitExceededException` (HTTP 410), so that is how this
/// store refuses past the cap. The emulator's own bound, not a published AWS
/// default.
pub const MAX_ACCESS_POLICIES: usize = 1_000;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the portal map. `CreatePortal`
/// is the only thing that adds a portal and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_PORTALS: usize = 1_000;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the project map. `CreateProject`
/// is the only thing that adds a project and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_PROJECTS: usize = 1_000;

/// How many assets one IoT SiteWise Monitor project holds.
///
/// The same kind of bound, and for the same reason, as
/// [`MAX_CHILDREN_PER_HIERARCHY`]: `BatchAssociateProjectAssets` is the only thing
/// that grows a project's asset membership, so the set needs a maximum size, and
/// that operation declares the modelled `LimitExceededException` (HTTP 410) — "this
/// can occur if you're trying to associate more than the allowed number of child
/// assets" — which is how this store refuses past the cap. The emulator's own
/// bound, not a published AWS default, and high enough that no ordinary workflow
/// meets it.
pub const MAX_ASSETS_PER_PROJECT: usize = 1_000;

/// The `AuthMode` value of a portal that authenticates its users through IAM
/// Identity Center, as `AuthMode` spells it (smithy_query query="shape"
/// shape="AuthMode": `IAM` | `SSO`).
///
/// The IAM Identity Center identifiers a portal reports — `ssoApplicationId` and
/// `portalClientId` — exist only for a portal in this mode: both members are
/// documented as present only "if the portal uses IAM Identity Center".
pub const PORTAL_AUTH_MODE_SSO: &str = "SSO";

/// The `portalAuthMode` a `CreatePortal` call that named none gets, as the
/// member's own documentation states: "Default: SSO" — so the default and the
/// identity-center test are one value and cannot drift apart.
pub const DEFAULT_PORTAL_AUTH_MODE: &str = PORTAL_AUTH_MODE_SSO;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the dataset map. `CreateDataset`
/// is the only thing that adds a dataset and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_DATASETS: usize = 1_000;

/// The `datasetVersion` a freshly created dataset reports.
///
/// A dataset's version counts the writes it has accepted — the create is the
/// first — so it is derived from the stored record rather than minted, and a
/// read never has to write for it to stay the same. The modelled `Version` type
/// is a decimal string (`^(0|([1-9]{1}\d*))$`), which is how the counter reaches
/// the wire.
pub const DATASET_INITIAL_VERSION: u32 = 1;

/// How many data segments ONE dataset holds.
///
/// A segment is added by a producer entry point and nothing evicts one, so the
/// writer refuses past this cap rather than dropping a segment a caller still
/// expects to find; together with [`MAX_DATASETS`] it bounds the whole registry.
/// The emulator's own bound, not a published AWS default.
pub const MAX_DATA_SEGMENTS_PER_DATASET: usize = 1_000;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the gateway map. `CreateGateway`
/// is the only thing that adds a gateway and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_GATEWAYS: usize = 1_000;

/// The `gatewayVersion` a `CreateGateway` call that named none gets, as the member's
/// own documentation states: "If not specified, the default is 2 (Classic streams, V2
/// gateway)".
pub const DEFAULT_GATEWAY_VERSION: &str = "2";

/// The `gatewayVersion` whose GreengrassV2 platform must also carry
/// `coreDeviceOperatingSystem`, as `GatewayPlatform`'s documentation states: "If
/// creating a V3 gateway (gatewayVersion=3), you must also specify the
/// coreDeviceOperatingSystem".
pub const GATEWAY_VERSION_V3: &str = "3";

/// The `CapabilitySyncStatus` a capability reports once the gateway has processed
/// the configuration the last write to it supplied, as the member documents it:
/// "IN_SYNC - The gateway is running with the latest configuration".
pub const CAPABILITY_SYNC_IN_SYNC: &str = "IN_SYNC";

/// The `CapabilitySyncStatus` a capability reports until then, as
/// `UpdateGatewayCapabilityConfiguration` documents it: "After updating a
/// capability configuration, the sync status becomes OUT_OF_SYNC until the
/// gateway processes the configuration".
pub const CAPABILITY_SYNC_OUT_OF_SYNC: &str = "OUT_OF_SYNC";

/// How many capability namespaces one gateway holds configurations for.
///
/// The same kind of bound, and for the same reason, as [`MAX_GATEWAYS`]: a write
/// naming a fresh namespace is the only thing that adds one and nothing evicts
/// one, so the list needs a maximum entry count. The emulator's own bound, not a
/// published AWS default, and high enough that no ordinary workflow meets it;
/// `UpdateGatewayCapabilityConfiguration` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410).
pub const MAX_CAPABILITIES_PER_GATEWAY: usize = 100;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the workspace map.
/// `CreateWorkspace` is the only thing that adds a workspace and it declares the
/// modelled `LimitExceededException` (HTTP 410), so that is how this store
/// refuses past the cap. The emulator's own bound, not a published AWS default.
pub const MAX_WORKSPACES: usize = 1_000;

/// See [`MAX_WORKSPACES`]: the same bound, for the pipeline-execution map.
/// `StartPipelineExecution` is the only thing that adds an execution and it
/// declares the modelled `LimitExceededException` (HTTP 410).
pub const MAX_PIPELINE_EXECUTIONS: usize = 1_000;

/// The `EncryptionType` that encrypts a workspace with the service's own key.
///
/// Source: `smithy_query(query="shape", shape="EncryptionType")` — the enum holds
/// exactly this value and [`ENCRYPTION_TYPE_KMS_BASED`].
pub const ENCRYPTION_TYPE_SITEWISE_DEFAULT: &str = "SITEWISE_DEFAULT_ENCRYPTION";

/// The `EncryptionType` that encrypts a workspace with a customer managed KMS
/// key; see [`ENCRYPTION_TYPE_SITEWISE_DEFAULT`].
pub const ENCRYPTION_TYPE_KMS_BASED: &str = "KMS_BASED_ENCRYPTION";

/// The `PipelineExecutionState` / `ComputeNodeExecutionState` value an execution
/// that has not begun reports. Both enums declare it
/// (`smithy_query(query="shape", shape="PipelineExecutionState,ComputeNodeExecutionState")`).
pub const EXECUTION_STATE_NOT_STARTED: &str = "NOT_STARTED";

/// The `PipelineExecutionState` an execution a cancel was recorded on reports
/// until that cancellation has taken effect: the value the enum carries for a
/// cancel in flight, which `CancelPipelineExecution` documents as the state such
/// an execution "transitions to" before it goes "asynchronously to CANCELLED".
pub const EXECUTION_STATE_CANCELLING: &str = "CANCELLING";

/// The `PipelineExecutionState` a cancelled execution reports once its
/// cancellation has taken effect. Terminal, and one of
/// [`EXECUTION_TERMINAL_STATES`].
pub const EXECUTION_STATE_CANCELLED: &str = "CANCELLED";

/// The `PipelineExecutionState` values an execution STAYS in once it reaches
/// one: the states in which the execution is over and nothing it does can move
/// it again.
///
/// Source: `smithy_query(query="shape", shape="PipelineExecutionState")` — the
/// enum holds `NOT_STARTED`, `RUNNING`, `SUCCEEDED`, `FAILED`, `CANCELLING` and
/// `CANCELLED`; the three below are the ones an execution does not leave, and
/// the other three are stages on the way to them. This is what
/// [`PipelineExecutionRecord::is_active`] reads, so "an execution that has not
/// finished" is one rule in one place.
pub const EXECUTION_TERMINAL_STATES: &[&str] = &["SUCCEEDED", "FAILED", "CANCELLED"];

/// The `executionPriority` an execution that named none runs at, as
/// `StartPipelineExecution`'s own member documents it: "Lower values indicate
/// higher priority. Defaults to 2 when not specified."
pub const DEFAULT_EXECUTION_PRIORITY: i32 = 2;

/// How many samples one `(assetId, propertyId)` series keeps.
///
/// Unlike the resource caps above, a time series DOES have an eviction policy
/// available to it: `GetAssetPropertyValue` reports the latest value, so the
/// OLDEST sample is the one that can go. Past this many samples
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::record_property_value`] drops the
/// oldest, so repeated writes cannot grow one series without limit. The emulator's
/// own bound; AWS publishes a retention period rather than a sample count.
pub const MAX_SAMPLES_PER_SERIES: usize = 1_000;

/// How many child assets one `(asset, hierarchy)` holds.
///
/// `AssociateAssets` is the only thing that adds one and it declares the modelled
/// `LimitExceededException` (HTTP 410) for exactly this case — "this can occur if
/// you're trying to associate more than the allowed number of child assets" — so
/// that is how this store refuses past the cap. The emulator's own bound, not a
/// published AWS default (AWS publishes the quota as adjustable), set high enough
/// that no ordinary hierarchy meets it.
pub const MAX_CHILDREN_PER_HIERARCHY: usize = 500;

/// How many DISASSOCIATED time series this store keeps.
///
/// Two operations put one here: `DisassociateTimeSeriesFromAssetProperty` keeps
/// the data stream it detaches (see [`DisassociatedTimeSeriesRecord`]), and
/// `BatchPutAssetPropertyValue` creates one for a `propertyAlias` no asset
/// property carries. The alias is the caller's to choose, so the collection needs
/// a bound of its own: the ingest path REFUSES past this many series with the
/// `LimitExceededException` entry error code (the operation declares it), and the
/// detach path — which declares no limit error to refuse with — drops the least
/// recently updated series instead. The emulator's own bound, not a published AWS
/// quota, and high enough that no ordinary workflow meets it.
pub const MAX_DISASSOCIATED_TIME_SERIES: usize = 1_000;

/// How many logging-option scopes this store keeps.
///
/// `PutLoggingOptions` is the only thing that adds one and the scope key is the
/// caller's own `workspaceName`, so the collection needs a bound of its own.
/// Unlike the resource caps above, this one has an eviction policy available to
/// it rather than a refusal: the operation declares no limit error, and a scope
/// dropped from the store simply reports [`DEFAULT_LOGGING_LEVEL`] again — the
/// answer an unconfigured scope already gets — so past this many scopes
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::set_logging_options`] drops the
/// least recently set WORKSPACE scope. The account-and-region-wide scope
/// ([`ACCOUNT_LOGGING_SCOPE`]) is exempt, so the unnamed scope is never dropped.
/// The emulator's own bound, not a published AWS quota, and high enough that no
/// ordinary workflow meets it.
pub const MAX_LOGGING_SCOPES: usize = 1_000;

/// How many capture sources this store keeps, across every workspace.
///
/// A capture source is created by the producer path that first writes video for
/// it ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_capture_segment`]), and
/// the only thing that removes one is a `DeleteTimeSeries` scoped to its
/// workspace
/// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::forget_capture_source`]), so
/// the collection needs a bound of its own: past this many sources a write
/// naming a NEW one is refused rather than evicting a source a caller can still
/// read. The emulator's own bound, not a published AWS quota, and high enough
/// that no ordinary workspace meets it.
pub const MAX_CAPTURE_SOURCES: usize = 1_000;

/// How many video segments one capture source keeps.
///
/// Like a time series' samples ([`MAX_SAMPLES_PER_SERIES`]), a capture source's
/// video is an unbounded stream with an eviction policy available to it: the
/// OLDEST segment is the one that goes, so repeated ingestion cannot grow one
/// source without limit. The emulator's own bound; AWS publishes a retention
/// period rather than a segment count.
pub const MAX_CAPTURE_SEGMENTS_PER_SOURCE: usize = 1_000;

/// The shortest `TimeSeriesId` the model admits. Source:
/// `smithy_query(query="shape", shape="TimeSeriesId")` — length 36..73.
pub const MIN_TIME_SERIES_ID_LENGTH: usize = 36;

/// The longest `TimeSeriesId` the model admits; see
/// [`MIN_TIME_SERIES_ID_LENGTH`].
pub const MAX_TIME_SERIES_ID_LENGTH: usize = 73;

/// The shortest `AssetPropertyAlias` the model admits. Source:
/// `smithy_query(query="shape", shape="AssetPropertyAlias")` — length 1..2048.
pub const MIN_PROPERTY_ALIAS_LENGTH: usize = 1;

/// The longest `AssetPropertyAlias` the model admits; see
/// [`MIN_PROPERTY_ALIAS_LENGTH`].
pub const MAX_PROPERTY_ALIAS_LENGTH: usize = 2048;

/// The earliest `timeInSeconds` a `TimeInNanos` can carry. Source:
/// `smithy_query(query="shape", shape="TimeInNanos")` — `timeInSeconds` is
/// `range 1..9223372036854774`.
pub const MIN_TIME_IN_SECONDS: i64 = 1;

/// The latest `timeInSeconds` a `TimeInNanos` can carry; see
/// [`MIN_TIME_IN_SECONDS`].
pub const MAX_TIME_IN_SECONDS: i64 = 9_223_372_036_854_774;

/// The largest `offsetInNanos` a `TimeInNanos` can carry: the member is
/// `range 0..999999999`, one nanosecond short of a whole second.
pub const MAX_OFFSET_IN_NANOS: i32 = 999_999_999;

/// Whether `at` is a `TimeInNanos` the model can express — see
/// [`MIN_TIME_IN_SECONDS`] and [`MAX_OFFSET_IN_NANOS`].
///
/// What the producer entry point checks before it stores an instant, so no span
/// this store holds can be reported through a member its own model would refuse.
fn is_modelled_time(at: &TimeInNanos) -> bool {
    (MIN_TIME_IN_SECONDS..=MAX_TIME_IN_SECONDS).contains(&at.time_in_seconds)
        && (0..=MAX_OFFSET_IN_NANOS).contains(&at.offset_in_nanos.unwrap_or(0))
}

/// The instant ONE nanosecond after `at`, or `None` when that is past what the
/// modelled `TimeInNanos` can express.
///
/// `TimeInNanos` is nanosecond-precision, so this is the smallest instant later
/// than `at`: what the HALF-OPEN end of a span that has to cover `at` is.
fn next_nanosecond(at: &TimeInNanos) -> Option<TimeInNanos> {
    let offset = at.offset_in_nanos.unwrap_or(0);
    let next = if offset >= MAX_OFFSET_IN_NANOS {
        TimeInNanos {
            time_in_seconds: at.time_in_seconds.checked_add(1)?,
            offset_in_nanos: Some(0),
        }
    } else {
        TimeInNanos {
            time_in_seconds: at.time_in_seconds,
            offset_in_nanos: Some(offset + 1),
        }
    };
    is_modelled_time(&next).then_some(next)
}

/// One scope's logging options and the instant they were set.
///
/// The instant is what the bound on
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::logging_options`] reads: past
/// [`MAX_LOGGING_SCOPES`] scopes the least recently set workspace scope is the
/// one dropped.
#[derive(Debug, Clone)]
pub struct LoggingOptionsRecord {
    /// The options the last `PutLoggingOptions` on this scope stored.
    pub options: LoggingOptions,
    /// When that put landed.
    pub set_at: SystemTime,
}

/// Whether a stored `(id, externalId)` pair is the one a `CustomID` label names:
/// the raw id, or `externalId:` followed by the external id.
///
/// ONE definition of the two label forms, so every nested resource this service
/// addresses by label — a hierarchy on an asset, a hierarchy on its model — is
/// resolved the same way.
pub fn matches_id_label(id: Option<&str>, external_id: Option<&str>, label: &str) -> bool {
    match label.strip_prefix(EXTERNAL_ID_PREFIX) {
        Some(external) => external_id == Some(external),
        None => id == Some(label),
    }
}

/// A `(parent asset, hierarchy, child asset)` triple resolved to stored ids —
/// what the association operations address.
#[derive(Debug, Clone)]
pub struct HierarchyTriple {
    /// The parent asset's stored `assetId`.
    pub asset_id: String,
    /// The hierarchy's stored id, taken from the parent asset's hierarchy list.
    pub hierarchy_id: String,
    /// The child asset's stored `assetId`.
    pub child_asset_id: String,
}

/// Why an association triple does not name anything the store can act on.
///
/// Described structurally rather than as one error type, because the two
/// association operations declare their own modelled error sets and each maps
/// these onto its own variants; [`TripleRefusal::message`] is the shared wording.
#[derive(Debug, Clone)]
pub enum TripleRefusal {
    /// No stored asset answers to this label (the parent or the child).
    AssetNotFound(String),
    /// The parent asset carries no hierarchy this label names.
    HierarchyNotFound(String),
}

impl TripleRefusal {
    /// What the caller is told. Every variant here is an absent resource, so the
    /// wording is the one this service states that resource's absence in.
    pub fn message(&self) -> String {
        match self {
            Self::AssetNotFound(label) => asset_not_found_message(label),
            Self::HierarchyNotFound(label) => hierarchy_not_found_message(label),
        }
    }
}

/// `settling` before the owed transition falls due, `ACTIVE` once it has.
fn state_at(settling: &str, active_at: SystemTime, now: SystemTime) -> &str {
    if now < active_at {
        settling
    } else {
        STATE_ACTIVE
    }
}

/// A stored gateway: everything `DescribeGateway` reports, plus the `tags` the
/// tagging operations read.
///
/// `capabilities` starts EMPTY: a freshly created gateway has no capability
/// configured, and only `UpdateGatewayCapabilityConfiguration` adds one. The
/// summaries `DescribeGateway` and `ListGateways` report are DERIVED from that
/// list ([`Self::capability_summaries`]), so a summary and the configuration
/// `DescribeGatewayCapabilityConfiguration` answers with can never disagree about
/// a namespace or its sync status.
#[derive(Debug, Clone)]
pub struct GatewayRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub platform: GatewayPlatform,
    pub version: String,
    pub capabilities: Vec<GatewayCapabilityRecord>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    pub tags: HashMap<String, String>,
}

impl GatewayRecord {
    /// The capability configured under `namespace`, or `None` when no write to
    /// this gateway has named that namespace.
    ///
    /// A gateway holds one configuration per namespace and may hold several
    /// namespaces at once, so the namespace alone selects one.
    pub fn capability(&self, namespace: &str) -> Option<&GatewayCapabilityRecord> {
        self.capabilities
            .iter()
            .find(|capability| capability.namespace == namespace)
    }

    /// Every configured capability as the summary a gateway reader reports, in the
    /// order the namespaces were first configured in.
    pub fn capability_summaries(&self, now: SystemTime) -> Vec<GatewayCapabilitySummary> {
        self.capabilities
            .iter()
            .map(|capability| GatewayCapabilitySummary {
                capability_namespace: capability.namespace.clone(),
                capability_sync_status: capability.sync_status(now).to_string(),
            })
            .collect()
    }

    /// Configure `namespace` on this gateway with `configuration`, replacing
    /// whatever a previous write put there, and report the sync status the write
    /// answers with — or, past [`MAX_CAPABILITIES_PER_GATEWAY`], the message the
    /// quota refusal carries.
    ///
    /// A write is the last word on the namespace it names and says nothing about
    /// any other, so an existing entry is replaced IN PLACE — keeping the order a
    /// reader reports — and a namespace nothing has configured is appended. The
    /// gateway's `lastUpdateDate` moves with it, because the capability summaries
    /// a gateway reader reports are part of what this write changed.
    ///
    /// The bound lives HERE, at the only thing that lengthens the list: the
    /// namespace is caller-supplied and nothing evicts an entry, so without a
    /// ceiling at the writer a stream of writes naming fresh namespaces would
    /// grow one emulator process without limit. A write REPLACING a namespace
    /// already configured lengthens nothing, so it is never refused for the cap.
    /// The `Err` carries the refusal message, which the calling operation lifts
    /// into its own modelled `LimitExceededException`.
    pub fn set_capability(
        &mut self,
        namespace: &str,
        configuration: String,
        now: SystemTime,
    ) -> Result<&'static str, String> {
        let existing = self
            .capabilities
            .iter_mut()
            .find(|capability| capability.namespace == namespace);
        let record = GatewayCapabilityRecord {
            namespace: namespace.to_string(),
            configuration,
            in_sync_at: now + ACTIVATION_DELAY,
        };
        match existing {
            Some(existing) => *existing = record,
            None => {
                if self.capabilities.len() >= MAX_CAPABILITIES_PER_GATEWAY {
                    return Err(capability_limit_message());
                }
                self.capabilities.push(record);
            }
        }
        self.last_update_date = now;
        Ok(CAPABILITY_SYNC_OUT_OF_SYNC)
    }
}

/// The environment variables one compute node of one execution runs with: every
/// layer that can define them, folded lowest priority first.
///
/// The hierarchy is the one the members themselves document: a task defines
/// "environment variables passed to the container at runtime"; a pipeline's are
/// "shared across all compute nodes in the pipeline. Individual compute nodes can
/// override these values with their own environment variables"; and an
/// execution's overrides "take the highest priority in the environment variable
/// hierarchy", with `global` applying "to all compute nodes in the pipeline
/// execution" and `computeNodes` carrying "per-compute-node environment variable
/// overrides". A later layer replaces an earlier layer's value for the same key
/// and keeps every key it does not name, so the caller passes the layers it has
/// in that order, lowest first.
pub fn resolved_environment_variables(
    layers: &[Option<&HashMap<String, String>>],
) -> HashMap<String, String> {
    let mut resolved = HashMap::new();
    for layer in layers.iter().flatten() {
        for (key, value) in *layer {
            resolved.insert(key.clone(), value.clone());
        }
    }
    resolved
}

/// The mounts one compute node of one execution runs with: its task-defined
/// mounts with the execution's overrides for that node merged in.
///
/// The rule is the one `executionMountOverrides` documents: "Overrides are merged
/// by mount name into each listed compute node's task-defined mounts: a matching
/// name replaces the task-defined mount, a new name adds a mount, and
/// task-defined mounts not referenced remain unchanged. Compute nodes not listed
/// use their task-defined mounts as-is."
pub fn resolved_mounts(task_mounts: &[Mount], overrides: Option<&[Mount]>) -> Vec<Mount> {
    let overrides = overrides.unwrap_or(&[]);
    let mut resolved: Vec<Mount> = task_mounts
        .iter()
        .map(|mount| {
            overrides
                .iter()
                .rev()
                .find(|replacement| replacement.name == mount.name)
                .unwrap_or(mount)
                .clone()
        })
        .collect();
    for mount in overrides {
        if !task_mounts
            .iter()
            .any(|defined| defined.name == mount.name)
            && !resolved.iter().any(|added| added.name == mount.name)
        {
            resolved.push(mount.clone());
        }
    }
    resolved
}

/// The platform device a gateway runs on, as a single comparable key: the
/// Greengrass V1 group ARN, the Greengrass V2 core device thing name or the Siemens
/// Industrial Edge IoT Core thing name, whichever the platform names.
///
/// `None` when the platform names no device at all — a request
/// [`GatewayPlatform`]'s documentation refuses, since one gateway specifies exactly
/// one platform type.
pub fn gateway_device_key(platform: &GatewayPlatform) -> Option<String> {
    gateway_device_dimension(platform).map(|(member, value)| format!("{member}:{value}"))
}

/// The same device, as the REQUEST MEMBER that names it and that member's value:
/// `greengrass.groupArn`, `greengrassV2.coreDeviceThingName` or
/// `siemensIE.iotCoreThingName`.
///
/// A refusal about a device has to name the dimension the caller actually supplied
/// — a thing name, a group ARN — rather than an identifier of the gateway it
/// collided with, so the member and its value are kept together here and
/// [`gateway_device_key`] is derived from them.
pub fn gateway_device_dimension(platform: &GatewayPlatform) -> Option<(&'static str, String)> {
    if let Some(greengrass) = platform.greengrass.as_ref() {
        return Some(("greengrass.groupArn", greengrass.group_arn.clone()));
    }
    if let Some(greengrass_v2) = platform.greengrass_v2.as_ref() {
        return Some((
            "greengrassV2.coreDeviceThingName",
            greengrass_v2.core_device_thing_name.clone(),
        ));
    }
    platform.siemens_ie.as_ref().map(|siemens| {
        (
            "siemensIE.iotCoreThingName",
            siemens.iot_core_thing_name.clone(),
        )
    })
}

/// How many platform types `platform` names. Exactly one is valid; see
/// [`gateway_device_key`].
pub fn gateway_platform_count(platform: &GatewayPlatform) -> usize {
    [
        platform.greengrass.is_some(),
        platform.greengrass_v2.is_some(),
        platform.siemens_ie.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count()
}

/// A stored IoT SiteWise Monitor portal: everything `DescribePortal` reports,
/// plus the `tags` the tagging operations read.
///
/// The identity-center identifiers (`sso_application_id`, `client_id`) and the
/// logo image are minted ONCE, by the call that created or last replaced them,
/// and read back from here: nothing a reader reports about a portal is rebuilt
/// per request, so two reads of one portal answer the same.
#[derive(Debug, Clone)]
pub struct PortalRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub contact_email: String,
    /// An `AuthMode` value: `IAM` or `SSO`.
    pub auth_mode: String,
    pub notification_sender_email: Option<String>,
    pub alarms: Option<Alarms>,
    /// A `PortalType` value, when the create named one. AWS documents no default
    /// for it, so a portal created without one reports none.
    pub portal_type: Option<String>,
    pub portal_type_configuration: Option<HashMap<String, PortalTypeEntry>>,
    pub role_arn: Option<String>,
    /// The portal's own URL, as [`portal_start_url`] builds it.
    pub start_url: String,
    /// The IAM Identity Center application id, or EMPTY for a portal that
    /// authenticates through IAM: the member is documented as the application id
    /// "if the portal uses IAM Identity Center", and such a portal has no
    /// application to name.
    ///
    /// The empty value is a deliberate DEVIATION from the modelled
    /// `SSOApplicationId` length (1..64): the member is modelled required, so
    /// there is no "absent" to report, and naming an application that does not
    /// exist would be worse than reporting none.
    pub sso_application_id: String,
    /// The IAM Identity Center generated client id, empty for the same reason as
    /// [`Self::sso_application_id`] — including the same deviation from the
    /// modelled `PortalClientId` length (1..256).
    pub client_id: String,
    /// The id of the logo image the portal was given, if any. The reported
    /// location is derived from it by [`portal_logo_image_url`].
    pub logo_image_id: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `PortalState` this portal reports until [`Self::active_at`] falls due.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl PortalRecord {
    /// The portal's status as of `now`: its [`Self::settling_state`] until the
    /// owed transition falls due, `ACTIVE` from then on. Derived, so a read never
    /// has to write.
    pub fn status(&self, now: SystemTime) -> PortalStatus {
        PortalStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The portal's `PortalState` as of `now`.
    ///
    /// Through the same [`state_at`] rule every record in this store derives its
    /// state from: `PortalState`'s `CREATING` and `ACTIVE` are spelled exactly as
    /// [`STATE_CREATING`] and [`STATE_ACTIVE`].
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// Where the portal's logo image is reported to live, when it has one.
    pub fn logo_image_location(&self) -> Option<ImageLocation> {
        self.logo_image_id.as_ref().map(|image_id| ImageLocation {
            id: image_id.clone(),
            url: portal_logo_image_url(&self.id, image_id),
        })
    }
}

/// A stored IoT SiteWise Monitor project: everything `DescribeProject` reports,
/// plus the `tags` the tagging operations read.
///
/// A project has no lifecycle state of its own — no operation on one answers a
/// status member — so, unlike a portal, nothing here is owed a later transition;
/// the two dates are what a read derives from. The portal the project was created
/// in is stored on the record because every reader reports it and no operation
/// moves a project between portals.
#[derive(Debug, Clone)]
pub struct ProjectRecord {
    pub id: String,
    pub arn: String,
    /// The portal the project lives in, as the create named it.
    pub portal_id: String,
    pub name: String,
    pub description: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    pub tags: HashMap<String, String>,
    /// The assets associated with the project, which `ListProjectAssets` reports
    /// and `BatchAssociateProjectAssets` grows.
    ///
    /// A SET: an asset is either in the project or it is not, so associating one
    /// that is already there adds nothing and a request naming the same asset
    /// twice associates it once. Ordered by id — a `BTreeSet` rather than a
    /// `HashSet` — because the listing pages through this membership and a
    /// resumable `nextToken` needs a stable total order. Held on the project's own
    /// record, so a deleted project takes its membership with it and no second map
    /// can disagree about which assets a project holds; the other end of the same
    /// association is pruned by [`Self::forget_asset`].
    pub asset_ids: BTreeSet<String>,
}

impl ProjectRecord {
    /// Replace the caller-owned members with what an update stated, and record
    /// that instant as the project's last update.
    ///
    /// The request states the project's new name and description rather than
    /// patching selected members, so an omitted optional description clears the
    /// stored one; the project's identity and provenance — its id, its ARN, the
    /// portal it lives in, its tags and its creation date — are not that call's to
    /// change, and no member of its input carries them.
    pub fn apply_update(&mut self, name: String, description: Option<String>, now: SystemTime) {
        self.name = name;
        self.description = description;
        self.last_update_date = now;
    }
}

/// One IoT SiteWise Monitor access policy: which identity may reach which Monitor
/// resource, and with which permission.
///
/// An access policy has no lifecycle state of its own — no operation on one
/// answers a status member — so, unlike a portal or a dataset, nothing here is
/// owed a later transition; the two dates are what a read derives from.
#[derive(Debug, Clone)]
pub struct AccessPolicyRecord {
    pub id: String,
    pub arn: String,
    /// The identity the policy grants access to, exactly as the call that last
    /// wrote the policy stated it — see [`check_access_policy_identity`].
    pub identity: Identity,
    /// The Monitor resource the policy is about: a portal or a project.
    pub resource: Resource,
    /// A `Permission` value: `ADMINISTRATOR` or `VIEWER`.
    pub permission: String,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    pub tags: HashMap<String, String>,
}

impl AccessPolicyRecord {
    /// Replace the caller-owned members with what an update stated, and record
    /// that instant as the policy's last update.
    ///
    /// Every member `UpdateAccessPolicy` carries is required, so the request
    /// states the policy's new identity, resource and permission in full rather
    /// than patching selected members; the policy's identity and provenance — its
    /// id, its ARN, its tags and its creation date — are not that call's to
    /// change, and no member of its input carries them.
    pub fn apply_update(
        &mut self,
        identity: Identity,
        resource: Resource,
        permission: String,
        now: SystemTime,
    ) {
        self.identity = identity;
        self.resource = resource;
        self.permission = permission;
        self.last_update_date = now;
    }
}

/// The members of the `Identity` shape, in the order it declares them. Source:
/// `smithy_query(query="shape", shape="Identity")`.
pub const IDENTITY_MEMBERS: [&str; 4] = ["user", "group", "iamUser", "iamRole"];

/// The members of the `Resource` shape, in the order it declares them. Source:
/// `smithy_query(query="shape", shape="Resource")`.
pub const MONITOR_RESOURCE_MEMBERS: [&str; 2] = ["portal", "project"];

/// How many of [`IDENTITY_MEMBERS`] an `Identity` names.
fn identity_member_count(identity: &Identity) -> usize {
    usize::from(identity.user.is_some())
        + usize::from(identity.group.is_some())
        + usize::from(identity.iam_user.is_some())
        + usize::from(identity.iam_role.is_some())
}

/// Refuse an `Identity` that does not name EXACTLY ONE identity.
///
/// `Identity` is a choice spelled as a structure of optional members — "Contains
/// an identity that can access an IoT SiteWise Monitor resource", whose own
/// members are documented as the alternatives to "choose" between — so a request
/// naming none of them, or more than one, names no single identity for the policy
/// to be about. The `Err` carries the refusal message, which the calling
/// operation lifts into its own modelled client error.
pub fn check_access_policy_identity(identity: &Identity) -> Result<(), String> {
    let named = identity_member_count(identity);
    if named == 1 {
        return Ok(());
    }
    Err(format!(
        "accessPolicyIdentity must name exactly one of {}; this request named {named}.",
        catalog_names(IDENTITY_MEMBERS.into_iter())
    ))
}

/// Refuse a `Resource` that does not name EXACTLY ONE Monitor resource.
///
/// The same rule as [`check_access_policy_identity`], for the shape the policy's
/// resource is stated in: "Contains an IoT SiteWise Monitor resource ID for a
/// portal or project", one or the other.
pub fn check_access_policy_resource(resource: &Resource) -> Result<(), String> {
    let named = usize::from(resource.portal.is_some()) + usize::from(resource.project.is_some());
    if named == 1 {
        return Ok(());
    }
    Err(format!(
        "accessPolicyResource must name exactly one of {}; this request named {named}.",
        catalog_names(MONITOR_RESOURCE_MEMBERS.into_iter())
    ))
}

/// One version of a pipeline: the caller-owned configuration as of the call that
/// wrote it.
///
/// A pipeline is VERSIONED — `UpdatePipeline` answers with "The new version of
/// the pipeline created by this update", and `DescribePipeline`'s
/// `pipelineVersion` reads "The version number of the pipeline to retrieve. If
/// not specified, returns the latest version" — so an accepted write appends one
/// of these instead of overwriting the configuration in place, and a read naming
/// an older version still answers from the configuration that version carried.
#[derive(Debug, Clone)]
pub struct PipelineVersion {
    /// This version's number, as the modelled `Version` string spells it.
    pub version: u64,
    pub description: Option<String>,
    /// Kept as an `Option` so a pipeline that was never given environment
    /// variables reports none, rather than an empty map it never carried.
    pub environment_variables: Option<HashMap<String, String>>,
    pub computations: Vec<ComputeNode>,
    /// When the call that wrote this version landed — what `updatedAt` reports
    /// for it.
    pub recorded_at: SystemTime,
}

/// `true` when the `(TargetResourceType, ID)` pair a request carries addresses
/// `target_resource`.
///
/// One place, so every listing scoped to a target resource — the executions of
/// one and the actions taken on one — resolves that pair the same way: a
/// `TargetResource` names an asset or a computation model, and the pair
/// addresses it only when the type picks the member the id matches.
pub fn target_resource_addresses(
    target_resource: &TargetResource,
    resource_type: &str,
    resource_id: &str,
) -> bool {
    let addressed = match resource_type {
        TARGET_RESOURCE_TYPE_ASSET => target_resource.asset_id.as_deref(),
        // A record of a computation model is addressed by that model's own id,
        // the same way a record of an asset is addressed by the asset's.
        TARGET_RESOURCE_TYPE_COMPUTATION_MODEL => target_resource.computation_model_id.as_deref(),
        _ => None,
    };
    addressed == Some(resource_id)
}

/// `true` when either reference an action or an execution carries to a resource
/// of this service — the resource it was taken ON (`targetResource`) and the
/// detailed resource it RESOLVES TO (`resolveTo`) — names the asset `asset_id`.
///
/// One place, so the records that name one asset are the same set wherever that
/// question is asked: a `TargetResource` names an asset or a computation model,
/// and a record of either kind may in addition resolve to one asset, so an asset
/// is named in two places and a record naming it in either of them is a record
/// OF that asset.
pub fn references_asset(
    target_resource: &TargetResource,
    resolve_to: Option<&ResolveTo>,
    asset_id: &str,
) -> bool {
    target_resource_addresses(target_resource, TARGET_RESOURCE_TYPE_ASSET, asset_id)
        || resolve_to.is_some_and(|resolve_to| resolve_to.asset_id == asset_id)
}

/// One action taken on a target resource: what `ListActions` summarises, keyed
/// by its own `actionId` in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::actions`].
///
/// An action is a record of a request to act on a resource, so the instant it
/// ran ([`Self::execution_time`]) is stored rather than derived: it is what
/// orders the enumeration a listing pages through, and what the read form of a
/// single action reports.
#[derive(Debug, Clone)]
pub struct ActionRecord {
    /// The id this action is addressed by.
    pub action_id: String,
    /// The action definition this action runs.
    pub action_definition_id: String,
    /// The resource the action was taken on: an asset or a computation model.
    pub target_resource: TargetResource,
    /// The JSON payload the call that took this action supplied, which the read
    /// form of a single action reports back.
    pub action_payload: ActionPayload,
    /// The detailed resource this action resolves to, which only an action whose
    /// target resolves through a data binding carries.
    pub resolve_to: Option<ResolveTo>,
    /// When the action ran.
    pub execution_time: SystemTime,
    /// The `clientToken` the call that took this action carried, if it carried
    /// one, recorded as part of what that call asked for. What recognises a
    /// RETRY of that call is the store's registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
}

impl ActionRecord {
    /// `true` when `resource_type` and `resource_id` address THIS action's
    /// target resource — the pairing `ListActions` scopes its listing by.
    pub fn targets(&self, resource_type: &str, resource_id: &str) -> bool {
        target_resource_addresses(&self.target_resource, resource_type, resource_id)
    }

    /// `true` when this action NAMES the asset `asset_id` — through its target
    /// resource or through the resource it resolves to, as
    /// [`references_asset`] states it.
    pub fn references_asset(&self, asset_id: &str) -> bool {
        references_asset(&self.target_resource, self.resolve_to.as_ref(), asset_id)
    }

    /// This action as the summary a listing reports for it.
    ///
    /// Derived here rather than in the handler so every view of one stored
    /// action reports the same identifiers and references.
    pub fn summary(&self) -> ActionSummary {
        ActionSummary {
            action_id: Some(self.action_id.clone()),
            action_definition_id: Some(self.action_definition_id.clone()),
            target_resource: Some(self.target_resource.clone()),
            resolve_to: self.resolve_to.clone(),
        }
    }
}

/// The `kmsKeyArn` a workspace reports for the `kmsKeyId` it was given.
///
/// `kmsKeyId` "Accepts a key ID, key ARN, or key alias" while the read form
/// reports an ARN, so the three forms are normalised to one here: an ARN is
/// reported as it was supplied, and a bare key id or an `alias/<name>` becomes
/// the ARN of that same key in the request's region and this account. The id is
/// NOT resolved through KMS — this service holds no key registry — so an alias is
/// reported as the alias's own ARN rather than as the ARN of the key behind it.
pub fn kms_key_arn(region: &str, kms_key_id: &str) -> String {
    if kms_key_id.starts_with("arn:") {
        return kms_key_id.to_string();
    }
    let resource = if kms_key_id.starts_with("alias/") {
        kms_key_id.to_string()
    } else {
        format!("key/{kms_key_id}")
    };
    ::engine::emulator_arn(KMS_ARN_SERVICE, region, &resource)
}

/// The encryption configuration a read reports for `workspace`: the stored
/// `EncryptionType`, and the ARN of the key the create named.
///
/// Derived from the stored record alone — the region comes from the workspace's
/// own ARN rather than from the calling request — so every read of one workspace
/// reports the same value.
pub fn workspace_encryption_info(
    workspace: &WorkspaceRecord,
) -> WorkspaceEncryptionConfigurationInfo {
    WorkspaceEncryptionConfigurationInfo {
        encryption_type: workspace.encryption.encryption_type.clone(),
        kms_key_arn: workspace
            .encryption
            .kms_key_id
            .as_deref()
            .map(|kms_key_id| kms_key_arn(arn_region(&workspace.arn), kms_key_id)),
    }
}

/// The region section of an ARN this service minted, or [`DEFAULT_REGION`] for a
/// string that is not one.
fn arn_region(arn: &str) -> &str {
    // `arn:<partition>:<service>:<region>:<account>:<resource>`.
    arn.split(':').nth(3).unwrap_or(DEFAULT_REGION)
}

/// Refuse a workspace encryption configuration this service cannot accept, in the
/// wording the refusal carries; `Ok` for one it can.
///
/// The rules the shape states, in ONE place because the create and the update
/// both supply it: `encryptionType` is an `EncryptionType`, and `kmsKeyId` is
/// "Required for KMS_BASED_ENCRYPTION; must be omitted for
/// SITEWISE_DEFAULT_ENCRYPTION". The value set is the generated, model-synced
/// `ENCRYPTION_TYPE_VALUES`, the way `PutLoggingOptions` reads its own enum's.
pub fn check_workspace_encryption(
    configuration: &WorkspaceEncryptionConfiguration,
) -> Result<(), String> {
    check_encryption_selection(
        configuration.encryption_type.as_str(),
        configuration.kms_key_id.as_deref(),
    )
}

/// The same rules over the two members a caller selects encryption with,
/// wherever they are selected: the workspace's own configuration above, and the
/// account-wide default encryption configuration, which carries the same pair of
/// members at the top level.
pub fn check_encryption_selection(
    encryption_type: &str,
    kms_key_id: Option<&str>,
) -> Result<(), String> {
    if !ENCRYPTION_TYPE_VALUES.contains(&encryption_type) {
        return Err(format!(
            "The encryptionType {encryption_type} is not supported; it must be one of {}.",
            ENCRYPTION_TYPE_VALUES.join(" | ")
        ));
    }
    match (
        encryption_type == ENCRYPTION_TYPE_KMS_BASED,
        kms_key_id.is_some(),
    ) {
        (true, false) => Err(format!(
            "A kmsKeyId is required when encryptionType is {ENCRYPTION_TYPE_KMS_BASED}."
        )),
        (false, true) => Err(format!(
            "A kmsKeyId must be omitted when encryptionType is \
             {ENCRYPTION_TYPE_SITEWISE_DEFAULT}."
        )),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Dataset export jobs
//
// `CreateDatasetExportJob` "starts an asynchronous job that exports dataset and
// time-series data from a workspace to Amazon S3. The operation returns a jobId
// immediately; poll DescribeDatasetExportJob to track progress". So the job
// record and every rule over it live here ONCE: the lifecycle the CLOCK drives,
// and the identity a reader addresses a job by.
//
// What the record does NOT hold is the exported content: the destination is a
// bucket in the CALLER's account, which this emulator does not serve, and no
// operation of this API ever answers an exported object. The record — the job's
// identity, its request, and where its lifecycle has got to — is the whole of
// what a client of this API can observe.
// ---------------------------------------------------------------------------

/// The `DatasetExportJobStatus` value a job reports from the instant it is
/// created (`crate::ops::types::DATASET_EXPORT_JOB_STATUS_VALUES`).
pub const EXPORT_JOB_SUBMITTED: &str = "SUBMITTED";
/// The value a job reports once the clock has started it processing.
pub const EXPORT_JOB_RUNNING: &str = "RUNNING";
/// The value a job reports once the clock has finished it. Terminal.
pub const EXPORT_JOB_COMPLETED: &str = "COMPLETED";

/// How long each step of a dataset export job's lifecycle takes: a created job
/// stays `SUBMITTED` for this long, then reports `RUNNING` for this long, then is
/// `COMPLETED`.
///
/// CHOSEN, not documented: AWS publishes no duration for either step — an export
/// takes as long as the data it exports. The span is deliberately NON-ZERO, so a
/// caller that describes a job straight after creating it still observes the
/// transient status the real service reports, and short enough that a caller can
/// poll the job through to its terminal status.
pub const EXPORT_JOB_STEP: Duration = Duration::from_millis(500);

/// How many dataset export jobs one workspace holds.
///
/// CHOSEN: a create is the only thing that adds a job and nothing evicts one, so
/// without a bound a stream of creates would grow one emulator process without
/// limit.
pub const MAX_DATASET_EXPORT_JOBS_PER_WORKSPACE: usize = 100;

/// How many dataset export jobs the store holds in total, across every
/// workspace.
///
/// The whole map's own limit, for the reason [`MAX_PIPELINES`] states: the
/// per-workspace cap above bounds one workspace's jobs, and this one bounds the
/// map every workspace's jobs share.
pub const MAX_DATASET_EXPORT_JOBS: usize = 1_000;

/// One dataset export job, as `CreateDatasetExportJob` starts it and
/// `DescribeDatasetExportJob` reports it.
///
/// Its STATUS is not stored: it is DERIVED from the instant the job was created
/// ([`Self::status`]), because the lifecycle is owed by the clock rather than by
/// any call. So a reader reports the status the clock has made due without
/// writing to the store, and two readers of one job can never disagree about
/// where it has got to.
#[derive(Debug, Clone)]
pub struct DatasetExportJobRecord {
    /// The `jobId` this job was minted with ([`new_id`]).
    pub job_id: String,
    /// The workspace the job was created in. A read is scoped to a workspace, so
    /// the pair is the job's identity — see [`dataset_export_job_key`].
    pub workspace_name: String,
    /// The `destinationS3Uri` the request named: "The S3 URI where output clips
    /// will be written". This service does not write it.
    pub destination_s3_uri: String,
    /// The `errorReportLocation` the request named.
    pub error_report_location: ExportErrorReportLocation,
    /// "The processing input that was provided in the CreateDatasetExportJob
    /// request", kept verbatim so the reader answers the request's own input.
    pub input: ProcessingInput,
    /// The `clientToken` the create carried, if any, recorded as part of what
    /// that call asked for. What recognises a RETRY of that call is the store's
    /// registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
    /// When the job was created, which is the `startedAt` a read reports.
    pub started_at: SystemTime,
}

impl DatasetExportJobRecord {
    /// The job's `status` at `now`: `SUBMITTED` until the first
    /// [`EXPORT_JOB_STEP`] has elapsed, `RUNNING` until the second has,
    /// `COMPLETED` after that. `COMPLETED` is terminal, so the status never moves
    /// again.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match self.completed_at(now) {
            Some(_) => EXPORT_JOB_COMPLETED,
            None if now >= self.started_at + EXPORT_JOB_STEP => EXPORT_JOB_RUNNING,
            None => EXPORT_JOB_SUBMITTED,
        }
    }

    /// "The timestamp when the job completed, or null if the job is still
    /// running": the instant the terminal status fell due, and `None` until it
    /// has.
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        let completes_at = self.started_at + 2 * EXPORT_JOB_STEP;
        (now >= completes_at).then_some(completes_at)
    }
}

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dataset_export_jobs`]
/// holds a job under: the workspace it was created in and its own id, because
/// every operation on a job addresses it within a workspace, so a job of one
/// workspace is not reachable through another.
pub fn dataset_export_job_key(workspace_name: &str, job_id: &str) -> (String, String) {
    (workspace_name.to_string(), job_id.to_string())
}

/// The resource type section a dataset export job's ARN carries, which is what
/// the modelled conflict refusal reports for one.
pub const DATASET_EXPORT_JOB_RESOURCE_TYPE: &str = "dataset-export-job";

/// The canonical ARN of a dataset export job in `region`.
///
/// The resource section carries the WORKSPACE as well as the id, for the reason
/// [`query_arn`] states: a job is addressed only inside the workspace it was
/// created in.
pub fn dataset_export_job_arn(region: &str, workspace_name: &str, job_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DATASET_EXPORT_JOB_RESOURCE_TYPE}/{workspace_name}/{job_id}"),
    )
}

/// Refuse a `ProcessingInput` whose items break a rule the shapes state, in the
/// wording the refusal carries; `Ok` for one that keeps them.
///
/// The rules come from the shapes themselves: a `TimeseriesItem` is "a single
/// timeseries item to process. Exactly one of timeSeriesId or propertyAlias must
/// be provided" (each member is also documented as "mutually exclusive" with the
/// other), and a `TrimSettings` end time "must be greater than startTime". Both
/// hold wherever the shapes appear, so they are checked in ONE place.
pub fn check_processing_input(input: &ProcessingInput) -> Result<(), String> {
    match input {
        ProcessingInput::Timeseries(items) => {
            for (at, item) in items.iter().enumerate() {
                match (item.time_series_id.as_deref(), item.property_alias.as_deref()) {
                    (Some(_), Some(_)) => {
                        return Err(format!(
                            "input.timeseries[{at}]: timeSeriesId and propertyAlias are mutually exclusive."
                        ));
                    }
                    (None, None) => {
                        return Err(format!(
                            "input.timeseries[{at}]: exactly one of timeSeriesId or propertyAlias must be provided."
                        ));
                    }
                    _ => {}
                }
                if let Some(trim) = item.trim_settings.as_ref() {
                    check_trim_settings(trim)
                        .map_err(|why| format!("input.timeseries[{at}]: {why}"))?;
                }
            }
            Ok(())
        }
        ProcessingInput::Dataset(item) => match item.trim_settings.as_ref() {
            Some(trim) => check_trim_settings(trim).map_err(|why| format!("input.dataset: {why}")),
            None => Ok(()),
        },
    }
}

/// Refuse a `TrimSettings` whose end time does not follow its start time: "Must
/// be greater than startTime".
fn check_trim_settings(trim: &TrimSettings) -> Result<(), String> {
    if timestamp_key(&trim.end_time) > timestamp_key(&trim.start_time) {
        Ok(())
    } else {
        Err("trimSettings.endTime must be greater than trimSettings.startTime.".to_string())
    }
}

/// Refuse a `computations` list that is not a pipeline DAG, in the wording the
/// refusal carries; `Ok` for one that is.
///
/// The three rules `CreatePipeline` and the `ComputeNode` shape state, enforced
/// in ONE place because both the create and the update supply the same list:
/// `computeNodeName` is "The unique name for this compute node within the
/// pipeline", `dependsOn` is "A list of compute node names that must complete
/// successfully before this node can start" — so a dependency names a node of
/// this same pipeline — and "Cyclic dependencies are not allowed".
///
/// Linear in the nodes and their dependencies: the names go into a hashed set,
/// and the acyclicity check is one in-degree pass plus a worklist (Kahn's
/// algorithm), so no shape of list makes the walk cost more than the list itself.
pub fn check_compute_nodes(computations: &[ComputeNode]) -> Result<(), String> {
    let mut names: HashSet<&str> = HashSet::with_capacity(computations.len());
    for node in computations {
        if !names.insert(node.compute_node_name.as_str()) {
            return Err(format!(
                "The compute node name {} is used by more than one compute node; \
                 a compute node name is unique within the pipeline.",
                node.compute_node_name
            ));
        }
    }

    // How many dependencies each node is still waiting on, and which nodes wait
    // on it — built in the same pass that checks every dependency names a node of
    // this pipeline.
    let mut waiting: HashMap<&str, usize> = HashMap::with_capacity(computations.len());
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::with_capacity(computations.len());
    for node in computations {
        let name = node.compute_node_name.as_str();
        let mut outstanding = 0usize;
        for dependency in node.depends_on.iter().flatten() {
            if !names.contains(dependency.as_str()) {
                return Err(format!(
                    "The compute node {name} depends on {dependency}, which is not a \
                     compute node of this pipeline."
                ));
            }
            outstanding += 1;
            dependents
                .entry(dependency.as_str())
                .or_default()
                .push(name);
        }
        waiting.insert(name, outstanding);
    }

    // Kahn's algorithm: start from the nodes waiting on nothing and release the
    // nodes whose last dependency has been released. A node still waiting when
    // the worklist empties sits on a cycle — including a node that depends on
    // itself, which waits on a dependency that is never released.
    let mut ready: Vec<&str> = waiting
        .iter()
        .filter(|(_, outstanding)| **outstanding == 0)
        .map(|(name, _)| *name)
        .collect();
    let mut settled = 0usize;
    while let Some(name) = ready.pop() {
        settled += 1;
        for dependent in dependents.get(name).into_iter().flatten().copied() {
            if let Some(outstanding) = waiting.get_mut(dependent) {
                *outstanding -= 1;
                if *outstanding == 0 {
                    ready.push(dependent);
                }
            }
        }
    }
    if settled < computations.len() {
        let mut cyclic: Vec<&str> = waiting
            .iter()
            .filter(|(_, outstanding)| **outstanding > 0)
            .map(|(name, _)| *name)
            .collect();
        cyclic.sort_unstable();
        return Err(format!(
            "The compute nodes {} form a cyclic dependency; a pipeline is a \
             directed acyclic graph.",
            cyclic.join(", ")
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// The asynchronous queries of a workspace: what `StartQuery` starts,
// `CancelQuery` cancels and `DescribeQuery` reports. A query's STATUS is derived
// from the instant it was submitted — and, once one is recorded, from the instant
// its cancel was asked for — exactly as a dataset export job's is, so a read
// reports what the clock has made due without writing anything.
// ---------------------------------------------------------------------------

/// The `QueryStatus` a query reports from the instant it is submitted: "The
/// initial query status. The value is always SUBMITTED upon creation"
/// (`crate::ops::types::QUERY_STATUS_VALUES`).
pub const QUERY_SUBMITTED: &str = "SUBMITTED";

/// The value a query reports once the clock has started it running.
pub const QUERY_RUNNING: &str = "RUNNING";

/// The value a query whose statement produced a result reports. Terminal.
pub const QUERY_COMPLETED: &str = "COMPLETED";

/// The value a query whose run did not finish reports, beside the message saying
/// why. Terminal.
pub const QUERY_FAILED: &str = "FAILED";

/// The value a query a cancel was recorded on reports until that cancellation has
/// taken effect: the `QueryStatus` value the enum carries for a cancel in flight.
pub const QUERY_CANCELING: &str = "CANCELING";

/// The value a cancelled query reports once its cancellation has taken effect.
/// Terminal.
pub const QUERY_CANCELED: &str = "CANCELED";

/// The resource type section a query's ARN carries, which is what the modelled
/// conflict refusal reports for one.
pub const QUERY_RESOURCE_TYPE: &str = "query";

/// The canonical ARN of a query in `region`.
///
/// The resource section carries the WORKSPACE as well as the id, for the reason
/// [`pipeline_arn`] states: a query is addressed only inside the workspace it was
/// started in.
pub fn query_arn(region: &str, workspace_name: &str, query_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{QUERY_RESOURCE_TYPE}/{workspace_name}/{query_id}"),
    )
}

/// How long each step of a query's lifecycle takes: a submitted query stays
/// `SUBMITTED` for this long, then reports `RUNNING` for this long, then is in its
/// terminal status.
///
/// CHOSEN, not documented: AWS publishes no duration for either step — a query
/// takes as long as the data it reads. The span is deliberately NON-ZERO, so a
/// caller that describes a query straight after starting it still observes the
/// transient statuses the real service reports, and short enough that a caller can
/// poll the query through to its terminal one.
pub const QUERY_STEP: Duration = Duration::from_millis(250);

/// How many queries one workspace holds.
///
/// CHOSEN: starting a query is the only thing that adds one and nothing evicts
/// one, so without a bound a stream of queries would grow one emulator process
/// without limit.
pub const MAX_QUERIES_PER_WORKSPACE: usize = 100;

/// How many queries the store holds in total, across every workspace. The
/// per-workspace cap cannot bound the store on its own, for the reason
/// [`MAX_DATASET_EXPORT_JOBS`] states.
pub const MAX_QUERIES: usize = 1_000;

/// One query of a workspace, as `StartQuery` started it.
///
/// The statement is RUN while `StartQuery` is serving the request, so what the run
/// produced is known from the moment the query exists and nothing has to be
/// computed again later. What is NOT stored is the query's status: that is DERIVED
/// from [`Self::submitted_at`] ([`Self::status`]), because the lifecycle is owed by
/// the clock rather than by any call, so two readers of one query can never
/// disagree about where it has got to and a read can never move it on.
#[derive(Debug, Clone)]
pub struct QueryRecord {
    /// The `queryId` this query was minted with ([`new_id`]).
    pub query_id: String,
    /// The workspace the query was started in. A read is scoped to a workspace,
    /// so the pair is the query's identity — see [`query_key`].
    pub workspace_name: String,
    /// The `queryStatement` the request carried. Modelled `@sensitive`, so no
    /// operation reports it back; it is kept because it is what the query IS, and
    /// what a reader of the query's results re-reads the store with.
    pub query_statement: String,
    /// The `clientToken` the request carried, if any, recorded as part of what
    /// that call asked for. What recognises a RETRY of that call is the store's
    /// registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
    /// When the query was submitted, which is the `submittedAt` a read reports.
    pub submitted_at: SystemTime,
    /// When a cancel was recorded on this query, if one was. A cancel is recorded
    /// only while the query has not reached a terminal status, and from then on it
    /// is this instant — not [`Self::submitted_at`] — the query's remaining
    /// lifecycle is owed from.
    pub cancel_requested_at: Option<SystemTime>,
    /// What running the statement produced.
    pub outcome: QueryOutcome,
}

/// What running one query's statement over the store produced.
#[derive(Debug, Clone)]
pub enum QueryOutcome {
    /// The run answered rows: how many there were, how many bytes of stored cells
    /// it read and how long it took. These are the `QueryStatistics` a read
    /// reports, each measured by the run itself.
    Completed {
        /// "The total number of rows returned by the query."
        row_count: i64,
        /// "The total number of bytes scanned during query execution."
        bytes_scanned: i64,
        /// "The total query execution time, in milliseconds."
        execution_time_in_millis: i64,
    },
    /// The run did not finish, and the message saying why — the `errorMessage` a
    /// read reports beside the `FAILED` status.
    Failed {
        /// "A human-readable error description for a failed query."
        error_message: String,
    },
}

impl QueryRecord {
    /// The query's `status` at `now`: `SUBMITTED` until the first [`QUERY_STEP`]
    /// has elapsed, `RUNNING` until the second has, and then the terminal status
    /// its run earned — `COMPLETED` for a run that answered rows, `FAILED` for one
    /// that did not finish. A terminal status never moves again.
    ///
    /// A cancelled query reports `CANCELING` until its cancellation takes effect
    /// and `CANCELED` from then on, whatever its run produced: the cancel is
    /// recorded only on a query that had not reached a terminal status, so the
    /// cancellation is the terminal state it reaches.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match (self.cancel_requested_at, self.completed_at(now)) {
            (Some(_), None) => QUERY_CANCELING,
            (Some(_), Some(_)) => QUERY_CANCELED,
            (None, Some(_)) => match self.outcome {
                QueryOutcome::Completed { .. } => QUERY_COMPLETED,
                QueryOutcome::Failed { .. } => QUERY_FAILED,
            },
            (None, None) if now >= self.submitted_at + QUERY_STEP => QUERY_RUNNING,
            (None, None) => QUERY_SUBMITTED,
        }
    }

    /// "The date and time when the query reached a terminal state": the instant
    /// the terminal status fell due, and `None` until it has.
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        let completes_at = self.terminal_at();
        (now >= completes_at).then_some(completes_at)
    }

    /// The instant this query's terminal status falls due: one step after the
    /// cancel that was recorded on it, and otherwise the end of the run-through
    /// span [`QUERY_STEP`] describes. A recorded cancel REPLACES the span the
    /// submission owed, because it is what the query now settles into.
    fn terminal_at(&self) -> SystemTime {
        match self.cancel_requested_at {
            Some(requested_at) => requested_at + QUERY_STEP,
            None => self.submitted_at + 2 * QUERY_STEP,
        }
    }

    /// Whether a cancel may still be recorded on this query at `now`: one the
    /// clock has not yet taken to a terminal status. A query a cancel was already
    /// recorded on is not cancelled again — the repeat is a no-op reporting the
    /// status it stands in.
    pub fn is_cancelable(&self, now: SystemTime) -> bool {
        self.cancel_requested_at.is_none() && self.completed_at(now).is_none()
    }

    /// The query's `statistics` at `now`: "statistics about a COMPLETED query
    /// execution", so a query that has not reached its terminal status yet, one
    /// whose run failed, and one that was cancelled report none.
    pub fn statistics(&self, now: SystemTime) -> Option<QueryStatistics> {
        match (&self.outcome, self.status(now)) {
            (
                QueryOutcome::Completed {
                    row_count,
                    bytes_scanned,
                    execution_time_in_millis,
                },
                QUERY_COMPLETED,
            ) => Some(QueryStatistics {
                row_count: *row_count,
                bytes_scanned: *bytes_scanned,
                execution_time_in_millis: *execution_time_in_millis,
            }),
            _ => None,
        }
    }

    /// The query's `errorMessage` at `now`: the message its failed run carried,
    /// and `None` unless that failure is the status the query reports — a query
    /// cancelled before its run's failure fell due reports no message, because the
    /// status it reached is the cancellation and not the failure.
    pub fn error_message(&self, now: SystemTime) -> Option<String> {
        match (&self.outcome, self.status(now)) {
            (QueryOutcome::Failed { error_message }, QUERY_FAILED) => Some(error_message.clone()),
            _ => None,
        }
    }

    /// This query as the summary a listing reports for it, at `now`.
    ///
    /// Derived here rather than in the handler so a listing and a read of one
    /// stored query cannot report different statuses or instants for it.
    pub fn summary(&self, now: SystemTime) -> QuerySummary {
        QuerySummary {
            query_id: self.query_id.clone(),
            status: self.status(now).to_string(),
            submitted_at: to_date_time(self.submitted_at),
            completed_at: self.completed_at(now).map(to_date_time),
        }
    }
}

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::queries`] holds a query
/// under: the workspace it was started in and its own id, because every operation
/// on a query addresses it within a workspace, so a query of one workspace is not
/// reachable through another.
pub fn query_key(workspace_name: &str, query_id: &str) -> (String, String) {
    (workspace_name.to_string(), query_id.to_string())
}

/// The `ListTimeSeriesType` value of a series associated with an asset property.
pub const TIME_SERIES_TYPE_ASSOCIATED: &str = "ASSOCIATED";

/// The `ListTimeSeriesType` value of a series associated with no asset property,
/// as the member's own documentation states it: "DISASSOCIATED \u2013 The time series
/// isn't associated with any asset property."
pub const TIME_SERIES_TYPE_DISASSOCIATED: &str = "DISASSOCIATED";

/// A stored record the tagging operations reach by ARN: it knows the ARN it was
/// minted with and owns the one tag set every operation that touches its tags
/// reads and writes.
///
/// Implemented once per taggable record, so
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource`] and its
/// `_mut` twin hold only the list of kinds and cannot drift in how a kind is
/// resolved.
pub trait TaggedRecord {
    /// The ARN this record was minted with.
    fn arn(&self) -> &str;
    /// The record's tag set.
    fn tags(&self) -> &HashMap<String, String>;
    /// The same tag set, for writing.
    fn tags_mut(&mut self) -> &mut HashMap<String, String>;
}

/// Implement [`TaggedRecord`] for a record carrying an `arn` and a `tags` field.
macro_rules! tagged_record {
    ($record:ty) => {
        impl TaggedRecord for $record {
            fn arn(&self) -> &str {
                &self.arn
            }

            fn tags(&self) -> &HashMap<String, String> {
                &self.tags
            }

            fn tags_mut(&mut self) -> &mut HashMap<String, String> {
                &mut self.tags
            }
        }
    };
}

tagged_record!(AssetRecord);
tagged_record!(AssetModelRecord);
tagged_record!(GatewayRecord);
tagged_record!(PortalRecord);
tagged_record!(ProjectRecord);
tagged_record!(PipelineRecord);
tagged_record!(TaskRecord);
tagged_record!(DatasetRecord);
tagged_record!(WorkspaceRecord);
tagged_record!(ComputationModelRecord);
tagged_record!(AccessPolicyRecord);

/// `record`'s tag set, but only when `record` is the one `arn` names exactly —
/// same partition, account and region, not merely the same id.
fn tags_of<'a, R: TaggedRecord>(
    record: Option<&'a R>,
    arn: &str,
) -> Option<&'a HashMap<String, String>> {
    record
        .filter(|record| record.arn() == arn)
        .map(TaggedRecord::tags)
}

/// The same, for writing. See [`tags_of`].
fn tags_of_mut<'a, R: TaggedRecord>(
    record: Option<&'a mut R>,
    arn: &str,
) -> Option<&'a mut HashMap<String, String>> {
    record
        .filter(|record| record.arn() == arn)
        .map(TaggedRecord::tags_mut)
}

/// Per-service state for com-amazonaws-iotsitewise-awsiotsitewise. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// Asset models by `assetModelId`.
    pub asset_models: HashMap<String, AssetModelRecord>,
    /// `assetModelExternalId` -> `assetModelId`, so `externalId:` labels resolve.
    pub asset_model_external_ids: HashMap<String, String>,
    /// Assets by `assetId`.
    pub assets: HashMap<String, AssetRecord>,
    /// `assetExternalId` -> `assetId`, so `externalId:` labels resolve.
    pub asset_external_ids: HashMap<String, String>,
    /// `(assetId, hierarchyId)` -> the child assets associated under that
    /// hierarchy. `ListAssets`'s `TOP_LEVEL` filter reads it to tell a root asset
    /// from one that is some other asset's child.
    pub asset_hierarchy_children: HashMap<(String, String), Vec<String>>,
    /// `assetId` -> `propertyId` -> the property's time series, kept sorted
    /// ascending by timestamp so the latest value is the last element.
    ///
    /// Nested rather than keyed by an `(assetId, propertyId)` tuple so a lookup
    /// borrows the two ids instead of allocating a pair of `String`s on every
    /// read and on every value of a batch write.
    pub property_values: HashMap<String, HashMap<String, Vec<AssetPropertyValue>>>,
    /// `propertyAlias` -> `(assetId, propertyId)`, so alias lookups resolve.
    pub property_alias_index: HashMap<String, (String, String)>,
    /// The time series no asset property owns, by the alias that identifies each
    /// one. Disjoint from [`Self::property_alias_index`] by construction: an
    /// alias names an associated stream or a disassociated one, never both — see
    /// [`Self::set_property_alias`] and [`Self::disassociate_time_series`].
    pub disassociated_time_series: HashMap<String, DisassociatedTimeSeriesRecord>,
    /// Gateways by `gatewayId`.
    pub gateways: HashMap<String, GatewayRecord>,
    /// [`gateway_device_key`] -> `gatewayId`: which gateway, if any, already runs
    /// on a given platform device. One device hosts at most one gateway, so this
    /// is what makes a second create naming the same device a conflict.
    pub gateway_devices: HashMap<String, String>,
    /// IoT SiteWise Monitor portals by `portalId`.
    pub portals: HashMap<String, PortalRecord>,
    /// IoT SiteWise Monitor projects by `projectId`. The id alone identifies one
    /// — every operation on a project addresses it by that id — and the record
    /// names the portal the project lives in, so the listing form filters on the
    /// record rather than on a second index.
    pub projects: HashMap<String, ProjectRecord>,
    /// IoT SiteWise Monitor access policies by `accessPolicyId`. The id alone
    /// identifies one, and the record names the identity and the Monitor
    /// resource the policy is about, so the listing forms filter on the record
    /// rather than on a second index.
    pub access_policies: HashMap<String, AccessPolicyRecord>,
    /// Pipelines by [`pipeline_key`] — the workspace they live in and their own
    /// name — because a pipeline name is unique only within its workspace.
    pub pipelines: HashMap<(String, String), PipelineRecord>,
    /// Tasks by [`task_key`] — the workspace they live in and their own name —
    /// because a task name is unique only within its workspace.
    pub tasks: HashMap<(String, String), TaskRecord>,
    /// The logging options each scope has been SET to, keyed by the
    /// `workspaceName` the call named and by [`ACCOUNT_LOGGING_SCOPE`] for a call
    /// that named none. An absent key is a scope no `PutLoggingOptions` has
    /// configured, which reports [`DEFAULT_LOGGING_LEVEL`] — see
    /// [`Self::logging_options_for`]. Bounded at [`MAX_LOGGING_SCOPES`] by
    /// evicting the least recently set workspace scope, which is why each entry
    /// carries the instant it was set.
    pub logging_options: HashMap<String, LoggingOptionsRecord>,
    /// Datasets by `datasetId`.
    pub datasets: HashMap<String, DatasetRecord>,
    /// The data segments of each dataset, by `datasetId` — a dataset id
    /// identifies one across every workspace, so the workspace a segment is
    /// addressed through is the one on the dataset's own record. Each dataset's
    /// segments are kept sorted by [`data_segment_key`] and single-valued on it,
    /// and bounded at [`MAX_DATA_SEGMENTS_PER_DATASET`]. Written by
    /// [`Self::put_dataset_data_segment`] and removed by
    /// [`Self::delete_dataset_data_segment`].
    pub dataset_data_segments: HashMap<String, Vec<DataSegmentRecord>>,
    /// Searches by [`search_key`] — the workspace they run against and their
    /// own id — because a search is addressed by that pair. Bounded at
    /// [`MAX_SEARCHES`] / [`MAX_SEARCHES_PER_WORKSPACE`], which `StartSearch`
    /// refuses past.
    pub searches: HashMap<(String, String), SearchRecord>,
    /// Executions of actions by `executionId`, which `ListExecutions`
    /// summarises for the target resource a request names.
    pub executions: HashMap<String, ExecutionRecord>,
    /// Actions taken on a target resource by `actionId`, which `ListActions`
    /// summarises for the target resource a request names.
    pub actions: HashMap<String, ActionRecord>,
    /// The account's default encryption configuration, as the last
    /// `PutDefaultEncryptionConfiguration` left it. `None` is an account nothing
    /// has configured, which reports [`ENCRYPTION_TYPE_SITEWISE_DEFAULT`] without storing
    /// anything — see [`Self::default_encryption_configuration`].
    pub default_encryption: Option<DefaultEncryptionRecord>,
    /// The account's storage configuration, as the last
    /// `PutStorageConfiguration` left it. `None` is an account nothing has
    /// configured, which reports the hot-tier default WITHOUT storing anything —
    /// see [`Self::storage_configuration`].
    pub storage: Option<StorageConfigurationRecord>,
    /// Dataset export jobs by [`dataset_export_job_key`] — the workspace they
    /// were created in and their own id — because every operation on a job
    /// addresses it within a workspace.
    pub dataset_export_jobs: HashMap<(String, String), DatasetExportJobRecord>,
    /// The asynchronous queries of every workspace, by [`query_key`] — the
    /// workspace they were started in and their own id — because every operation
    /// on a query addresses it within a workspace. Written by `StartQuery`,
    /// read by `DescribeQuery` and enumerated, one workspace at a time, by
    /// `ListQueries`; bounded by [`MAX_QUERIES`] and
    /// [`MAX_QUERIES_PER_WORKSPACE`].
    pub queries: HashMap<(String, String), QueryRecord>,
    /// The enrichment jobs of every workspace, by [`enrichment_job_key`] — the
    /// workspace they were created in and their own id — because every
    /// operation on a job addresses it within a workspace. Written by
    /// `CreateEnrichmentJob` and enumerated, one workspace at a time, by
    /// `ListEnrichmentJobs`; bounded by [`MAX_ENRICHMENT_JOBS`] and
    /// [`MAX_ENRICHMENT_JOBS_PER_WORKSPACE`].
    pub enrichment_jobs: HashMap<(String, String), EnrichmentJobRecord>,
    /// Workspaces by `workspaceName`, which is what identifies one in an
    /// account and region.
    pub workspaces: HashMap<String, WorkspaceRecord>,
    /// Applications by `applicationId`, which identifies one across every
    /// workspace; the workspace it was created in lives on its record, because
    /// the modelled reads address an application by `(workspaceName, id)`.
    /// Written by `CreateApplication` and read by `DescribeApplication`.
    pub applications: HashMap<String, ApplicationRecord>,
    /// Pipeline executions by `pipelineExecutionId`. The id alone identifies one,
    /// and the record names the workspace and pipeline it belongs to, so a read
    /// scoped to another pipeline resolves to nothing.
    pub pipeline_executions: HashMap<String, PipelineExecutionRecord>,
    /// The capture sources (video streams) of every workspace: the workspace
    /// they live in, then the source's own `timeSeriesId`, because a capture
    /// source is addressable only inside its workspace — nested so a lookup
    /// borrows the two names instead of allocating a key on every read. Written
    /// by [`Self::put_capture_segment`], read by `GetCaptureData` and by the
    /// workspace-scoped forms of the time-series operations, whose series these
    /// are, and removed by [`Self::forget_capture_source`].
    pub capture_sources: HashMap<String, HashMap<String, CaptureSourceRecord>>,
    /// `workspaceName` -> `propertyAlias` -> `timeSeriesId`, so the alias form of
    /// a capture-source lookup resolves to the same record the id form does.
    /// Written beside the source itself, and bounded by it: at most one entry per
    /// source that carries an alias.
    pub capture_source_aliases: HashMap<String, HashMap<String, String>>,
    /// Computation models by `computationModelId`.
    pub computation_models: HashMap<String, ComputationModelRecord>,
    /// The interface relationships, by [`interface_relationship_key`] — the asset
    /// model the interface is applied to and the interface asset model — because
    /// one asset model may implement several interfaces and one interface may be
    /// applied to several asset models. Bounded by
    /// [`MAX_INTERFACE_RELATIONSHIPS`].
    pub interface_relationships: HashMap<(String, String), InterfaceRelationshipRecord>,
    /// IoT SiteWise Monitor dashboards by `dashboardId`, which identifies one
    /// across every project; the project it was created in lives on its record,
    /// because that is what a read reports and what the listing form filters on.
    /// Written by `CreateDashboard` and `UpdateDashboard`, read by
    /// `DescribeDashboard`.
    pub dashboards: HashMap<String, DashboardRecord>,
    /// Bulk import jobs by `jobId`, which identifies one across the account:
    /// `DescribeBulkImportJob` addresses a job by that id alone and only
    /// NARROWS the read with an optional `workspaceName`, so the workspace a
    /// job was created for (a job ingesting into a session dataset has one,
    /// a job ingesting into time series outside a workspace has none) lives on
    /// the record. Written by `CreateBulkImportJob` and read by
    /// `DescribeBulkImportJob`; bounded by [`MAX_BULK_IMPORT_JOBS`]. A job whose
    /// destination is a session dataset is removed with that destination —
    /// [`Self::forget_dataset`] and [`Self::forget_workspace`] — because nothing
    /// deletes a job on its own and a read of one must not report a dataset or a
    /// workspace this store no longer holds.
    pub bulk_import_jobs: HashMap<String, BulkImportJobRecord>,
    /// The mutating calls this store has already served under a `clientToken`,
    /// by [`idempotency_key`] — the operation and the token — because a token is
    /// scoped to the operation it was sent to. Each entry says which request
    /// spent the token and which resource that call acted on, which is what
    /// lets a retry of it replay that answer instead of mutating a second time
    /// and a DIFFERENT request reusing the token be refused. Written by
    /// [`Self::record_idempotent_call`], read by [`Self::idempotent_verdict`]
    /// and bounded by [`MAX_IDEMPOTENT_CALLS`].
    pub idempotent_calls: HashMap<String, IdempotentCall>,
}

/// The resource kind a `ResourceNotFoundException` / `ResourceAlreadyExistsException`
/// names for an asset, as AWS words it.
pub const RESOURCE_ASSET: &str = "Asset";

/// The resource kind those errors name for an asset model.
pub const RESOURCE_ASSET_MODEL: &str = "Asset Model";

/// The resource kind those errors name for a pipeline.
pub const RESOURCE_PIPELINE: &str = "Pipeline";

/// The resource kind those errors name for a task.
pub const RESOURCE_TASK: &str = "Task";

/// The resource kind those errors name for a dataset.
pub const RESOURCE_DATASET: &str = "Dataset";

/// The resource kind those errors name for a data segment a dataset holds.
pub const RESOURCE_DATA_SEGMENT: &str = "Data Segment";

/// The resource kind those errors name for a search.
pub const RESOURCE_SEARCH: &str = "Search";

/// The resource kind those errors name for a dataset export job.
pub const RESOURCE_DATASET_EXPORT_JOB: &str = "Dataset Export Job";

/// The resource kind those errors name for a workspace.
pub const RESOURCE_WORKSPACE: &str = "Workspace";

/// The resource kind those errors name for a pipeline execution.
pub const RESOURCE_PIPELINE_EXECUTION: &str = "Pipeline Execution";

/// The resource kind those errors name for a query of a workspace.
pub const RESOURCE_QUERY: &str = "Query";

/// The resource kind those errors name for a composite model of an ASSET — the
/// instance of an asset model's composite model that lives on one asset.
///
/// A composite model is not a top-level resource, so it is named as what it is
/// rather than as an "Asset". The composite model of an asset MODEL is not here:
/// the service states its absence in its own form, naming the model it was
/// looked for in ([`composite_model_not_found_message`]).
pub const RESOURCE_ASSET_COMPOSITE_MODEL: &str = "Asset Composite Model";

/// The resource kind those errors name for a gateway.
///
/// AWS's own wording was observed for the asset and asset-model paths; for this
/// kind and the three below, the label is the service's resource name under that
/// same one form, so every handler addressing one states the absence identically.
pub const RESOURCE_GATEWAY: &str = "Gateway";

/// The resource kind those errors name for one CAPABILITY of a gateway — the
/// configuration a gateway holds under one namespace.
///
/// A capability is not a top-level resource, so it is named as what it is rather
/// than as a "Gateway", and it lives here beside its siblings so the operations
/// that address one cannot each invent a kind of their own.
pub const RESOURCE_GATEWAY_CAPABILITY: &str = "Gateway Capability";

/// The resource kind those errors name for a data stream, which every operation
/// that addresses one addresses by its `alias`.
pub const RESOURCE_TIME_SERIES: &str = "Time Series";

/// The resource kind those errors name for an IoT SiteWise Monitor portal.
pub const RESOURCE_PORTAL: &str = "Portal";

/// The resource kind those errors name for an IoT SiteWise Monitor project.
pub const RESOURCE_PROJECT: &str = "Project";

/// The resource kind those errors name for an IoT SiteWise Monitor access policy.
pub const RESOURCE_ACCESS_POLICY: &str = "Access Policy";

/// The resource kind those errors name for a computation model.
pub const RESOURCE_COMPUTATION_MODEL: &str = "Computation Model";

/// The resource kind those errors name for one action taken on a target
/// resource — what `ExecuteAction` records and `DescribeAction` reads back.
pub const RESOURCE_ACTION: &str = "Action";

/// The resource kind those errors name for one EXECUTION of an action — the run
/// `ExecuteAction` starts, which `DescribeExecution` reads back and
/// `ListExecutions` summarises.
pub const RESOURCE_EXECUTION: &str = "Execution";

/// The resource kind those errors name for an action DEFINITION: one of the
/// actions a target resource exposes, which an execute addresses by its
/// `actionDefinitionId`.
///
/// A definition is not a top-level resource but one of the actions a resource
/// publishes, so it is named as what it is rather than as the resource it
/// belongs to.
pub const RESOURCE_ACTION_DEFINITION: &str = "Action Definition";

/// The message a not-found refusal carries: the resource kind, then the
/// identifier the REQUEST supplied, echoed exactly as sent — whatever form that
/// identifier took for the resource in question (a raw id, an `externalId:`
/// label, a stream alias, a workspace name).
///
/// The form every absence this service has no resource-specific wording for is
/// stated in, so no handler invents its own template: AWS states such an absence
/// as `<Resource> with ID <id> not found`, which is why the identifier is echoed
/// rather than replaced by whatever the handler resolved. The families whose own
/// wording the service uses instead each have one function of their own beside
/// this one ([`asset_not_found_message`],
/// [`composite_model_not_found_message`], [`alias_not_found_message`],
/// [`gateway_not_found_message`], [`gateway_not_found_for_update_message`],
/// [`hierarchy_not_found_message`], [`not_found_arn_message`]).
pub fn not_found_message(resource: &str, requested: &str) -> String {
    format!("{resource} with ID {requested} not found")
}

/// The message a not-found refusal for an ASSET carries.
///
/// An asset is addressed either by its own id or by the `externalId:` selector
/// the `CustomID` pattern admits. Which of the two the REQUEST used is what
/// failed to resolve, so it is what the refusal reports: a selector-addressed
/// request failed to resolve an EXTERNAL ID, which the service states with the
/// bare external id — the selector is how the request named the dimension, not
/// part of its value — while a request that supplied the id itself keeps the one
/// form [`not_found_message`] states every absence in.
///
/// One place, so every operation that resolves an asset label words the same
/// absence identically whichever form the label took.
pub fn asset_not_found_message(requested: &str) -> String {
    match requested.strip_prefix(EXTERNAL_ID_PREFIX) {
        Some(external_id) => format!("asset with externalId: {external_id} doesn't exist."),
        None => not_found_message(RESOURCE_ASSET, requested),
    }
}

/// The message a not-found refusal for one COMPOSITE MODEL of an asset model
/// carries: a composite model is addressable only WITHIN an asset model, and the
/// same composite-model id can be absent from many models, so the absence names
/// both halves of the address — the composite model asked for and the asset model
/// it was looked for in — each echoed as the request supplied it.
///
/// One place, so the read, the create of a nested one, the update and the delete
/// cannot word the same absence four ways.
pub fn composite_model_not_found_message(asset_model: &str, composite_model: &str) -> String {
    format!("CompositeModel: {composite_model} not found for AssetModel: {asset_model}")
}

/// The message a not-found refusal for a data stream addressed by its ALIAS
/// carries: an alias is resolved as a `dataAlias` within ONE account, so the
/// absence names the account the lookup ran in and the alias it ran for, rather
/// than reporting the alias as an id of the stream.
///
/// The account is the one every ARN this emulator mints carries, so a refusal and
/// the resources a caller can see agree about which account it is addressing.
/// One place, so every operation that resolves the `alias` of a time series —
/// the read, the delete and the binding of one to an asset property — words that
/// absence identically.
pub fn alias_not_found_message(alias: &str) -> String {
    let account = ::engine::EMULATOR_ACCOUNT_ID;
    format!(
        "dataAlias with accountId AwsAccountId{{value={account}}}, \
         alias PropertyAlias(super=PropertyAlias{{value={alias}}}) not found"
    )
}

/// The message a READ of an absent gateway is refused with: the id that resolved
/// to nothing, and the service's guidance to check it.
///
/// A gateway's `gatewayId` is modelled as a plain `ID`, so the only identifier a
/// refusal can name is the id the request path carried.
pub fn gateway_not_found_message(gateway_id: &str) -> String {
    format!(
        "No gateway with ID {gateway_id} was found. Please verify that you supplied the correct \
         gateway ID and try again."
    )
}

/// The message an UPDATE of an absent gateway is refused with. The service words
/// this one differently from the read above ([`gateway_not_found_message`]), so
/// the two are kept as the two sentences they are rather than collapsed into one.
pub fn gateway_not_found_for_update_message(gateway_id: &str) -> String {
    format!(
        "A gateway with ID {gateway_id} could not be found. Please verify you supplied the \
         correct gateway ID and try again."
    )
}

/// The message a half-open span covering no time is refused with: a span's
/// start must be strictly before its end, so the refusal names the two members
/// the request stated it with.
///
/// One wording for every operation that takes a span as a pair of modelled
/// members, so the two batch data-segment operations cannot each state the same
/// refusal differently.
pub fn empty_span_message() -> String {
    "endTimestamp must be later than startTimestamp.".to_string()
}

/// The message a call that would drop a data stream a dataset still holds a
/// data segment of is refused with: the stream that cannot go, and one dataset
/// that still holds a segment of it.
///
/// One wording for every call that removes a stream — the delete of the time
/// series itself, the delete of the asset whose property owns it, the model
/// update that takes that property away — so the same dependency cannot be
/// stated three ways. AWS words the rule as "You can't delete a time series
/// until all of its data segments have been deleted from session datasets"
/// (`DeleteTimeSeries`'s doc), so the refusal names the stream and the dataset
/// whose segments have to go first.
pub fn data_segments_remain_message(time_series_id: &str, holder: &str) -> String {
    format!(
        "The time series {time_series_id} has data segments in the dataset {holder}. \
         Delete them before deleting it."
    )
}

/// The message a uniqueness refusal carries: AWS names the DIMENSION of the
/// request that collided (`externalId`, `ID`, `name`), not an identifier the
/// rejected request would have minted.
pub fn already_using_message(resource: &str, dimension: &str) -> String {
    format!("Another {resource} is already using the same {dimension}")
}

/// The message an absent HIERARCHY carries. A hierarchy is not a top-level
/// resource but a grouping an asset model declares, and AWS words its absence as
/// a hierarchy that "doesn't exist"; the identifier is still the one the request
/// supplied, echoed exactly as sent.
///
/// One place, so the operations that address a hierarchy — the two association
/// operations and the listing over one — cannot word the same absence three ways.
pub fn hierarchy_not_found_message(requested: &str) -> String {
    format!("A hierarchy with ID {requested} doesn't exist")
}

/// The message a not-found refusal carries where the request addressed the
/// resource by its ARN rather than by an id: the dimension the request named is
/// the ARN, so that is what the refusal states.
pub fn not_found_arn_message(requested_arn: &str) -> String {
    format!("Resource with ARN {requested_arn} not found")
}

/// `a, b, c` — the accepted values a refusal names, so a message states the RULE
/// that refused the request rather than only that it was refused.
///
/// One place, so every operation that refuses a value outside a modelled
/// catalogue words the catalogue the same way.
pub fn catalog_names<'a>(values: impl Iterator<Item = &'a str>) -> String {
    values.collect::<Vec<&str>>().join(", ")
}

/// Turn `AssetModelPropertyDefinition`s into the stored `AssetModelProperty`s,
/// minting an id for every definition that did not supply one.
///
/// Shared by every operation that takes property definitions, so one rule is
/// enforced in one place. Two rules the shapes' own documentation states are
/// enforced here: a property definition carries exactly ONE property type ("You
/// can only specify one type in a property definition"), and a `STRUCT` property
/// also carries `dataTypeSpec` ("This parameter is required on properties that
/// have the STRUCT data type"). The `Err` carries the refusal message, which the
/// calling operation lifts into its own modelled `InvalidRequestException`.
pub fn property_definitions_to_properties(
    definitions: &[AssetModelPropertyDefinition],
) -> Result<Vec<AssetModelProperty>, String> {
    let mut properties = Vec::with_capacity(definitions.len());
    for definition in definitions {
        check_property_shape(
            &definition.name,
            &definition.data_type,
            definition.data_type_spec.as_deref(),
            &definition.r#type,
        )?;
        properties.push(AssetModelProperty {
            id: Some(definition.id.clone().unwrap_or_else(new_id)),
            external_id: definition.external_id.clone(),
            name: definition.name.clone(),
            data_type: definition.data_type.clone(),
            data_type_spec: definition.data_type_spec.clone(),
            unit: definition.unit.clone(),
            r#type: definition.r#type.clone(),
            path: None,
        });
    }
    Ok(properties)
}

/// One model property materialised as the asset property an asset created from
/// that model carries: the asset property INHERITS the definition's id, external
/// id, name, data type and unit, so a value written against the asset resolves
/// to the definition it came from. An alias and a notification are not inherited
/// — they are set per asset property, by `UpdateAssetProperty`.
///
/// `None` when the stored model property carries no usable id, which would
/// materialise an asset property no request could address.
///
/// Shared by every operation that reports an asset's properties, so the asset
/// properties a create materialises and the ones a read derives from the same
/// definitions cannot disagree.
pub fn asset_property_from_model(property: &AssetModelProperty) -> Option<AssetProperty> {
    let id = property.id.clone().filter(|id| !id.is_empty())?;
    Some(AssetProperty {
        id,
        external_id: property.external_id.clone(),
        name: property.name.clone(),
        alias: None,
        notification: None,
        data_type: property.data_type.clone(),
        data_type_spec: property.data_type_spec.clone(),
        unit: property.unit.clone(),
        path: None,
    })
}

/// What an asset created from one asset model carries: the flat property list
/// [`AssetRecord::properties`] holds, and the value each of those properties
/// starts its series with.
///
/// Produced by
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::materialise_asset_properties`]
/// — the one rule for "the properties of an asset of this model" — so the create
/// that materialises an asset and the propagation that re-materialises it cannot
/// disagree.
#[derive(Debug, Default, Clone)]
pub struct MaterialisedAssetProperties {
    /// Every property the asset carries: the model's own top-level definitions
    /// first, then the ones each composite model of it contributes.
    pub properties: Vec<AssetProperty>,
    /// `(propertyId, value)` for each property whose definition declares the
    /// value it starts with; see [`initial_property_sample`].
    pub initial_values: Vec<(String, AssetPropertyValue)>,
}

impl MaterialisedAssetProperties {
    /// The values properties start their series with, indexed by property id.
    ///
    /// What a caller re-materialising many assets from one model asks per
    /// property, so the lookup is constant-time rather than a scan of the list.
    pub fn initial_values_by_id(&self) -> HashMap<&str, &AssetPropertyValue> {
        self.initial_values
            .iter()
            .map(|(id, value)| (id.as_str(), value))
            .collect()
    }
}

/// Materialise ONE definition into `materialised`, under the composite-model
/// path `prefix` (`None` for one of the asset's own top-level definitions).
///
/// Two composite models may reuse the SAME component model, so one definition
/// can be contributed by more than one of them: the asset then carries that
/// property once per composite model that contributes it, each instance under
/// its own path, which is what the composite-model views of the asset report.
/// They share the definition's id, hence the one time series that id addresses,
/// so the value the series STARTS with is seeded once — `seen` is what keeps a
/// second instance from seeding it again.
fn materialise_one(
    definition: &AssetModelProperty,
    prefix: Option<&[AssetPropertyPathSegment]>,
    now: SystemTime,
    seen: &mut HashSet<String>,
    materialised: &mut MaterialisedAssetProperties,
) {
    let Some(mut property) = asset_property_from_model(definition) else {
        return;
    };
    let first = seen.insert(property.id.clone());
    // The STORED path is the chain WITHIN the asset: the composite models the
    // property sits inside, outermost first, then the property itself. A
    // property of the asset's own definitions sits inside none, so it carries no
    // stored path — which is also what tells the two apart wherever a response
    // has to (see [`composite_model_of_property`]). The asset's own hop, which
    // every read puts at the head of the path it reports, is NOT stored: these
    // properties are materialised once per asset model, for every asset of it
    // alike, so the hop is added by [`AssetRecord::property_path`].
    if let Some(prefix) = prefix {
        let mut path = prefix.to_vec();
        path.push(AssetPropertyPathSegment {
            id: Some(property.id.clone()),
            name: Some(property.name.clone()),
        });
        property.path = Some(path);
    }
    if first {
        if let Some(sample) = initial_property_sample(definition, now) {
            materialised
                .initial_values
                .push((property.id.clone(), sample));
        }
    }
    materialised.properties.push(property);
}

/// The `assetCompositeModelId` of the composite model that contributes
/// `property` to the asset carrying it, or `None` for one of the asset's own
/// (base) properties.
///
/// Read off the property's own STORED path — the chain within the asset, whose
/// last segment is the property and whose segment before it is the composite
/// model that holds it — so the membership and the path a response reports come
/// from ONE stored value and cannot drift apart. A base property carries no
/// stored path.
///
/// Asked of a STORED property, never of the projection a read reports
/// ([`AssetRecord::reported_property`]), whose path carries the asset's own hop
/// at its head as well.
pub fn composite_model_of_property(property: &AssetProperty) -> Option<&str> {
    let path = property.path.as_deref()?;
    let holder = path.len().checked_sub(2)?;
    path[holder].id.as_deref()
}

/// Which of the four property types a `PropertyType` sets, named as the shape's
/// own member; `None` when it sets none of them or more than one.
///
/// The ONE place this service decides what "the type of a property" is, so the
/// rule that a definition carries exactly one type and the rule that an existing
/// property's type cannot change ask the same question of the same shape.
pub fn property_type_kind(property_type: &PropertyType) -> Option<&'static str> {
    let kinds = [
        ("attribute", property_type.attribute.is_some()),
        ("measurement", property_type.measurement.is_some()),
        ("transform", property_type.transform.is_some()),
        ("metric", property_type.metric.is_some()),
    ];
    let mut found = None;
    for (name, set) in kinds {
        if set {
            if found.is_some() {
                return None;
            }
            found = Some(name);
        }
    }
    found
}

/// The two rules the property shapes' own documentation states, checked wherever
/// a model property arrives — as a definition on a create, or as a full
/// `AssetModelProperty` on `UpdateAssetModel`:
///
/// * exactly ONE property type ("You can only specify one type in a property
///   definition"), and
/// * `dataTypeSpec` on a `STRUCT` ("This parameter is required on properties that
///   have the STRUCT data type").
///
/// The `Err` carries the refusal message, which the calling operation lifts into
/// its own modelled `InvalidRequestException`.
pub fn check_property_shape(
    name: &str,
    data_type: &str,
    data_type_spec: Option<&str>,
    property_type: &PropertyType,
) -> Result<(), String> {
    if property_type_kind(property_type).is_none() {
        let set = [
            property_type.attribute.is_some(),
            property_type.measurement.is_some(),
            property_type.transform.is_some(),
            property_type.metric.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count();
        return Err(format!(
            "The property definition {name} must specify exactly one property type \
             (attribute, measurement, transform or metric), but specifies {set}."
        ));
    }
    if data_type == DATA_TYPE_STRUCT && data_type_spec.is_none() {
        return Err(format!(
            "The property definition {name} has the STRUCT data type, so dataTypeSpec is required."
        ));
    }
    Ok(())
}

/// The one `PropertyDataType` value that also requires `dataTypeSpec`; see
/// [`check_property_shape`].
pub const DATA_TYPE_STRUCT: &str = "STRUCT";

/// The `PropertyDataType` of a time series whose samples are video — what a
/// workspace's capture source reports (see
/// [`CaptureSourceRecord::time_series_summary`]). Source:
/// `smithy_query(query="shape", shape="PropertyDataType")`, whose values are
/// STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON.
pub const DATA_TYPE_VIDEO: &str = "VIDEO";

/// `id`, unless it is absent or empty.
pub fn non_empty_id(id: Option<&str>) -> Option<String> {
    id.filter(|id| !id.is_empty()).map(ToString::to_string)
}

/// Whether a request's `(id, externalId)` pair names the stored member whose own
/// pair is `(held_id, held_external_id)`.
///
/// A member of an asset model's definition — a property, a hierarchy, a composite
/// model — is addressed by its `id`, or by its `externalId` when the request
/// carries no id for it; a request that carries neither describes a NEW member,
/// which is why a pair of absent ids never matches anything.
pub fn same_definition_member(
    held_id: Option<&str>,
    held_external_id: Option<&str>,
    id: Option<&str>,
    external_id: Option<&str>,
) -> bool {
    match (non_empty_id(id), non_empty_id(external_id)) {
        // The `id` member of a definition is a `CustomID`, so it carries either
        // label form — "the actual ID in UUID format, or else `externalId:`
        // followed by the external ID" — and both name the stored member.
        (Some(id), _) => matches_id_label(held_id, held_external_id, &id),
        (None, Some(external_id)) => held_external_id == Some(external_id.as_str()),
        (None, None) => false,
    }
}

/// The property list a request's `AssetModelProperty`s become, resolved against
/// the `stored` properties they replace.
///
/// Each requested property is checked by the same rule every arriving model
/// property is ([`check_property_shape`]). A property the holder already carries
/// keeps its stored id — so its asset-level data and its alias survive — and may
/// change its name, unit and the inside of its type, but not its TYPE or its DATA
/// TYPE: "You can’t change the type or data type of an existing property". A
/// property the request carries no stored id for is new and gets one minted,
/// unless the request supplied one. A property the request does not name at all is
/// gone, which is how a caller removes one.
///
/// Shared by every operation that rewrites a stored property list from a full
/// `AssetModelProperty` list, so the properties of a model and the properties of
/// one of its composite models follow one rule. The `Err` carries the refusal
/// message, which the calling operation lifts into its own modelled
/// `InvalidRequestException`.
pub fn resolve_updated_properties(
    stored: &[AssetModelProperty],
    requested: &[AssetModelProperty],
) -> Result<Vec<AssetModelProperty>, String> {
    let mut resolved: Vec<AssetModelProperty> = Vec::with_capacity(requested.len());
    for property in requested {
        check_property_shape(
            &property.name,
            &property.data_type,
            property.data_type_spec.as_deref(),
            &property.r#type,
        )?;
        let held = stored.iter().find(|held| {
            same_definition_member(
                held.id.as_deref(),
                held.external_id.as_deref(),
                property.id.as_deref(),
                property.external_id.as_deref(),
            )
        });
        let id = match held {
            Some(held) => {
                // `dataTypeSpec` is part of a STRUCT property's data type ("the data
                // type of the structure for this property"), so it cannot move either.
                if held.data_type != property.data_type
                    || held.data_type_spec != property.data_type_spec
                {
                    return Err(format!(
                        "The existing property {} cannot change its data type from {} to {}.",
                        property.name,
                        describe_data_type(&held.data_type, held.data_type_spec.as_deref()),
                        describe_data_type(&property.data_type, property.data_type_spec.as_deref()),
                    ));
                }
                let held_kind = property_type_kind(&held.r#type);
                let kind = property_type_kind(&property.r#type);
                if held_kind != kind {
                    return Err(format!(
                        "The existing property {} cannot change its property type from {} to {}.",
                        property.name,
                        held_kind.unwrap_or("none"),
                        kind.unwrap_or("none"),
                    ));
                }
                held.id.clone().unwrap_or_else(new_id)
            }
            None => non_empty_id(property.id.as_deref()).unwrap_or_else(new_id),
        };
        if held.is_none() {
            // A definition that matches no stored property CREATES one. A
            // creation whose name a stored property already holds would take
            // that property's place under a new id — and the id is what every
            // time series, variable reference and ingest path addressing it
            // names — so it is refused instead, naming the property it would
            // have replaced. The caller either names the stored property by its
            // id (or `externalId:`) to modify it, or removes it in one call and
            // adds the new one in a second, which is the sequence
            // `UpdateAssetModel` documents for replacing a property by name.
            if let Some(replaced) = stored
                .iter()
                .find(|stored| stored.name == property.name)
                .and_then(|stored| stored.id.as_deref())
            {
                return Err(property_replacement_message(&property.name, replaced));
            }
        }
        resolved.push(AssetModelProperty {
            id: Some(id),
            external_id: property.external_id.clone(),
            name: property.name.clone(),
            data_type: property.data_type.clone(),
            data_type_spec: property.data_type_spec.clone(),
            unit: property.unit.clone(),
            r#type: property.r#type.clone(),
            // `path` is server-computed, never caller-supplied; the reads derive
            // what they report.
            path: None,
        });
    }
    // One request cannot describe the same stored property twice: both entries
    // would be that one property, so the later one would silently win.
    for (at, property) in resolved.iter().enumerate() {
        if resolved[..at]
            .iter()
            .any(|earlier| earlier.id == property.id)
        {
            return Err(format!(
                "The request names the property {} more than once.",
                property.id.as_deref().unwrap_or_default()
            ));
        }
    }
    // Two properties of one model may not carry one NAME, whichever write
    // states them; see [`check_property_names_unique`].
    check_property_names_unique(&resolved)?;
    Ok(resolved)
}

/// A property's data type as a refusal names it: the `PropertyDataType`, plus the
/// `dataTypeSpec` a STRUCT carries.
fn describe_data_type(data_type: &str, data_type_spec: Option<&str>) -> String {
    match data_type_spec {
        Some(spec) => format!("{data_type} ({spec})"),
        None => data_type.to_string(),
    }
}

/// The `ETag` the `If-Match` / `If-None-Match` preconditions of an asset-model
/// write are evaluated against, as of `now`, or `None` when the version
/// `version_type` names does not exist.
///
/// `matchForVersionType` targets the modelled `AssetModelVersionType` enum
/// (`LATEST | ACTIVE`) and [`VERSION_TYPE_LATEST`] is its documented default, so
/// the version it names is resolved by [`AssetModelRecord::version_view`] — the
/// same resolution a read's `assetModelVersion` goes through — and the tag is
/// the one of THAT version's definition. The `None` is a model that has no such
/// version, which a precondition can only evaluate to false. Shared by every
/// operation that carries those headers, so `CreateAssetModelCompositeModel`,
/// `UpdateAssetModel` and `DeleteAssetModel` cannot disagree about which tag a
/// precondition targets.
pub fn asset_model_target_etag(
    record: &AssetModelRecord,
    version_type: Option<&str>,
    now: SystemTime,
) -> Option<String> {
    record
        .version_view(version_type, now)
        .map(|view| asset_model_etag(&view))
}

/// The conditional request header an asset-model write carries the expected
/// entity tag in, spelled the way the model binds it (`@httpHeader("If-Match")`).
pub const HEADER_IF_MATCH: &str = "If-Match";

/// The conditional request header an asset-model write asks for the ABSENCE of
/// the targeted version with, spelled the way the model binds it
/// (`@httpHeader("If-None-Match")`).
pub const HEADER_IF_NONE_MATCH: &str = "If-None-Match";

/// The message a failed asset-model precondition carries on an operation that
/// WRITES the model's definition — the creates and the updates: the conditional
/// request header whose precondition evaluated to FALSE, and the version
/// `matchForVersionType` targeted ([`VERSION_TYPE_LATEST`] by default).
///
/// This is the sentence IoT SiteWise was observed answering those writes with.
/// Nothing of the REQUEST's own values goes into it — not the entity tag the
/// caller sent, which is the value the precondition rejected and which the
/// caller already holds, not the spelling it sent the header in, and not the
/// resource's own id — so the refusal states the condition that failed and the
/// version it was evaluated on, and leaves the caller to re-read the resource
/// for the tag it now carries.
///
/// One wording for every one of those writes, so `CreateAssetModelCompositeModel`,
/// `UpdateAssetModel` and `UpdateAssetModelCompositeModel` cannot word one
/// condition on one resource three ways. The REMOVALS answer the sentence of
/// their own that IoT SiteWise was observed answering them with; it is
/// [`precondition_failed_removal_message`].
pub fn precondition_failed_message(header: &str, version_type: Option<&str>) -> String {
    let version_type = version_type.unwrap_or(VERSION_TYPE_LATEST).to_lowercase();
    format!("Provided {header} value does not match the current {version_type} version ETag.")
}

/// The message a failed asset-model precondition carries on an operation that
/// REMOVES part of the model — `DeleteAssetModel` and
/// `DeleteAssetModelCompositeModel`: the conditional request header whose
/// precondition evaluated to FALSE, and nothing else.
///
/// This is the sentence IoT SiteWise was observed answering the removals with:
/// it names no version, so the version selector does not reach it either. As
/// with [`precondition_failed_message`], nothing of the REQUEST's own values
/// goes into it. One wording for both removals, so neither can word the
/// condition its own way.
pub fn precondition_failed_removal_message(header: &str) -> String {
    format!("Precondition failed for provided {header} value.")
}

/// The message a read or a precondition carries when the asset model has no
/// ACTIVE version: the model has never finished settling, so the only
/// definition it holds is one no version alias but `LATEST` names.
///
/// One wording for every operation that resolves the selector, so the reads of
/// a model, of its properties and of one of its composite models cannot word
/// the same absence three ways. `state` is the `AssetModelState` the model does
/// report, which is what tells the caller to wait rather than to look elsewhere.
pub fn no_active_version_message(asset_model_id: &str, state: &str) -> String {
    format!("The asset model {asset_model_id} has no ACTIVE version yet; it is {state}.")
}

/// The entity tag of an asset model's stored definition.
///
/// CONTENT-DERIVED, never minted: the SHA-256 digest of the fields a write to the
/// model changes, rendered as hex and cut to the 43 characters the modelled
/// `ETag` type fixes (`^[\w-]{43}$`, and a hex digit is a `\w`). Two reads of an
/// unchanged model therefore report the same tag and any change to the definition
/// reports a different one, which is what the `If-Match` / `If-None-Match`
/// preconditions on the asset-model writes compare against. 43 hex characters
/// carry 172 bits of the digest, and the tag is opaque to the caller: nothing but
/// equality is ever asked of it. Hex because this crate already mints its ids and
/// pagination tokens with `hex`, so the tag needs no further encoder.
///
/// The stored definition goes into the fingerprint through `{:?}`, which covers
/// every member of these shapes — a hand-listed set of fields would silently stop
/// covering a member the model later grows, and a tag that misses a change is the
/// one failure mode a precondition cannot tolerate. The exact bytes are not a
/// contract: a caller reads a tag and writes it back within one process.
///
/// The version's own NUMBER is part of the fingerprint, so the latest and the
/// active version of a model mid-update — two different definitions — report
/// two different tags, and a caller that read one writes back a precondition
/// targeting that one (see [`AssetModelRecord::version_view`]).
pub fn asset_model_etag(record: &AssetModelRecord) -> String {
    // The links are a map, so they go into the fingerprint in key order: two
    // models with the same definition must hash to the same tag whatever order
    // their maps happen to iterate in.
    let mut links: Vec<_> = record.composite_model_links.iter().collect();
    links.sort_by_key(|(id, _)| *id);
    let fingerprint = format!(
        "{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}",
        record.id,
        record.version,
        record.name,
        record.description.as_deref().unwrap_or(""),
        record.model_type,
        record.properties,
        record.hierarchies,
        record.composite_models,
        links,
    );
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(fingerprint.as_bytes());
    let mut tag = ::hex::encode(digest);
    tag.truncate(ETAG_LENGTH);
    tag
}

/// The expression variables of a property's formula: those of its `transform`
/// or of its `metric`.
///
/// An `attribute` or a `measurement` carries no formula and so contributes no
/// variables. ONE definition of what a property's formula depends on, so every
/// rule about formula dependencies asks the same question of the same members.
pub fn formula_variables(
    property: &AssetModelProperty,
) -> impl Iterator<Item = &ExpressionVariable> {
    let transform = property
        .r#type
        .transform
        .iter()
        .flat_map(|transform| transform.variables.iter());
    let metric = property
        .r#type
        .metric
        .iter()
        .flat_map(|metric| metric.variables.iter().flatten());
    transform.chain(metric)
}

/// The expression variables of a property's formula, as mutable references: the
/// counterpart of [`formula_variables`], for the ONE write that rewrites what a
/// variable points at ([`DefinitionUnderWrite::resolve_variable_references`]).
///
/// Shares that function's definition of which members carry a formula, so a
/// member read as a dependency cannot be a member a resolution skips.
pub fn formula_variables_mut(
    property: &mut AssetModelProperty,
) -> impl Iterator<Item = &mut ExpressionVariable> {
    let PropertyType {
        transform, metric, ..
    } = &mut property.r#type;
    let transform = transform
        .iter_mut()
        .flat_map(|transform| transform.variables.iter_mut());
    let metric = metric
        .iter_mut()
        .flat_map(|metric| metric.variables.iter_mut().flatten());
    transform.chain(metric)
}

/// Total order on a `TimeInNanos`: whole seconds first, then the nanosecond
/// offset, exactly as the modelled members are defined.
pub fn timestamp_key(timestamp: &TimeInNanos) -> (i64, i32) {
    (
        timestamp.time_in_seconds,
        timestamp.offset_in_nanos.unwrap_or(0),
    )
}

/// Total order on a data segment inside the dataset that holds it: the time
/// series it covers, then its span.
///
/// This is the segment's IDENTITY — what `BatchDeleteDatasetDataSegments`
/// addresses one by — so the one comparison serves the sorted insert, the
/// single-valued replace and the lookup a deletion makes.
fn data_segment_key(segment: &DataSegmentRecord) -> (&str, (i64, i32), (i64, i32)) {
    (
        segment.time_series_id.as_str(),
        timestamp_key(&segment.start_timestamp),
        timestamp_key(&segment.end_timestamp),
    )
}

/// Where in a dataset's sorted segment log the segment `time_series_id` and the
/// span `[start, end)` name EXACTLY sits, or `None` when the log holds none.
///
/// The one lookup every operation addressing a single segment goes through —
/// the read and the removal alike — so a segment is found by its identity in
/// exactly one place.
fn data_segment_at(
    held: &[DataSegmentRecord],
    time_series_id: &str,
    start: &TimeInNanos,
    end: &TimeInNanos,
) -> Option<usize> {
    let wanted = (time_series_id, timestamp_key(start), timestamp_key(end));
    held.binary_search_by(|stored| data_segment_key(stored).cmp(&wanted))
        .ok()
}

/// A wall-clock instant as the modelled `TimeInNanos`: whole seconds since the
/// Unix epoch, plus the nanosecond offset inside that second.
///
/// An instant before the epoch reports second 0 — no stored instant this service
/// mints is one, since every one of them comes from the wall clock.
pub fn time_in_nanos(at: SystemTime) -> TimeInNanos {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    TimeInNanos {
        time_in_seconds: i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        offset_in_nanos: i32::try_from(since_epoch.subsec_nanos()).ok(),
    }
}

/// The initial sample an asset property materialised from `definition` carries,
/// or `None` when the definition declares none.
///
/// An `attribute` definition's `defaultValue` IS that property's value on every
/// asset made from the model — "All assets that you create from the asset model
/// contain this attribute value. You can update an attribute's value after you
/// create an asset" (`smithy_query(query="doc", shape="Attribute",
/// member="defaultValue")`) — so the property's series starts with that value at
/// the instant the property was materialised, with the quality an ordinary
/// reading carries ([`DEFAULT_QUALITY`]). A later write at a newer timestamp
/// takes over as the property's current value, which is what "you can update an
/// attribute's value" means.
///
/// `None` for every other property type (a measurement, transform or metric
/// declares no default), for an attribute that declares no `defaultValue`, and
/// for a default that does not read as the property's `PropertyDataType`: the
/// declared text is then no value of that type, and nothing is invented for it.
pub fn initial_property_sample(
    definition: &AssetModelProperty,
    at: SystemTime,
) -> Option<AssetPropertyValue> {
    let default = definition.r#type.attribute.as_ref()?.default_value.as_ref()?;
    Some(AssetPropertyValue {
        value: typed_variant(&definition.data_type, default)?,
        timestamp: time_in_nanos(at),
        quality: Some(DEFAULT_QUALITY.to_string()),
    })
}

/// `text` read as the one `Variant` member the `PropertyDataType` `data_type`
/// names, or `None` when it is no value of that type.
///
/// The pairing of data type to `Variant` member is the model's: the
/// `PropertyDataType` values are STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT |
/// VIDEO | ANNOTATION | JSON (`smithy_query(query="shape",
/// shape="PropertyDataType,Variant")`), of which only the first four have a
/// `Variant` member to carry a scalar reading, so a default declared on any
/// other type reads as no value at all.
fn typed_variant(data_type: &str, text: &str) -> Option<Variant> {
    let variant = match data_type {
        "STRING" => Variant {
            string_value: Some(text.to_string()),
            ..Variant::default()
        },
        "INTEGER" => Variant {
            integer_value: Some(text.parse().ok()?),
            ..Variant::default()
        },
        // A non-finite reading (`inf`, `NaN`) parses as an `f64` but is no number
        // the JSON wire can carry, so it is no value of this type either.
        "DOUBLE" => Variant {
            double_value: Some(text.parse().ok().filter(|value: &f64| value.is_finite())?),
            ..Variant::default()
        },
        "BOOLEAN" => Variant {
            boolean_value: Some(text.parse().ok()?),
            ..Variant::default()
        },
        _ => return None,
    };
    Some(variant)
}

/// The opaque pagination token that resumes at the [`timestamp_key`] `resume` of
/// the enumeration `scope` identifies: the hex of `seconds.nanos` beside that
/// scope.
///
/// ONE recipe for every time-ranged read of this service that pages by the
/// instant it stopped at, so an encoding fix cannot drift between operations.
/// Hex keeps the text inside the `^[A-Za-z0-9+/=]+$` pattern the paging members
/// of this service carry.
///
/// The token carries `scope` — a [`pagination_scope`] of the request members that
/// decide WHICH entries are enumerated and in what order — beside the instant,
/// because an instant alone names a position in every enumeration that happens to
/// hold it. See [`pagination_scope`] for what that binding is for.
pub fn encode_timestamp_token(resume: (i64, i32), scope: &str) -> String {
    let (seconds, nanos) = resume;
    encode_scoped_token(
        &format!("{seconds}{TIMESTAMP_TOKEN_SEPARATOR}{nanos}"),
        scope,
    )
}

/// The [`timestamp_key`] `token` resumes the enumeration `scope` identifies at,
/// or `None` for text this service never issued as a token OF THAT enumeration —
/// which its caller answers as the modelled refusal for an invalid `nextToken`.
pub fn decode_timestamp_token(token: &str, scope: &str) -> Option<(i64, i32)> {
    let resume = decode_scoped_token(token, scope)?;
    let (seconds, nanos) = resume.split_once(TIMESTAMP_TOKEN_SEPARATOR)?;
    Some((token_number(seconds)?, token_number(nanos)?))
}

/// The opaque pagination token that resumes at `resume` within the enumeration
/// `scope` identifies: the hex of the resume point beside that scope.
///
/// ONE recipe for binding a token to its enumeration, so every paginated read of
/// this service that carries a resume point in its token binds that point the
/// same way. See [`pagination_scope`] for what the binding is for.
pub fn encode_scoped_token(resume: &str, scope: &str) -> String {
    ::hex::encode(format!("{resume}{TOKEN_SCOPE_SEPARATOR}{scope}").as_bytes())
}

/// The resume point `token` names within the enumeration `scope` identifies, or
/// `None` for text this service never issued as a token OF THAT enumeration —
/// which its caller answers as the modelled refusal for an invalid `nextToken`.
pub fn decode_scoped_token(token: &str, scope: &str) -> Option<String> {
    let decoded = decode_token_text(token)?;
    let (resume, carried) = decoded.rsplit_once(TOKEN_SCOPE_SEPARATOR)?;
    (carried == scope).then(|| resume.to_string())
}

/// What joins the two halves of the resume point in a token
/// [`encode_timestamp_token`] builds.
const TIMESTAMP_TOKEN_SEPARATOR: char = '.';

/// What joins a token's resume point to the [`pagination_scope`] it was minted
/// in. Neither half can contain it: a resume point is digits and the separators
/// above, and a scope is hex.
const TOKEN_SCOPE_SEPARATOR: char = '|';

/// A short digest of what a paginated read asks of its ENUMERATION: the request
/// members that decide which entries it enumerates and in what order.
///
/// A pagination token carries this beside its resume point, so a token is only
/// honoured against the request that produced it. A resume point on its own —
/// an instant, a window start, a position — names something in any enumeration
/// that happens to hold it, so a token replayed against another one (another
/// stream, another filter, another ordering, another range) would silently serve
/// a slice of that other enumeration and drop everything before the point it
/// resumed at. Bound to the scope it was minted in, such a token names no page at
/// all and its caller refuses it. The page SIZE is not part of a scope: it
/// changes where a page ends, not which entries are enumerated or in what order.
///
/// `members` render through the shared [`request_fingerprint`], so one member
/// renders one way wherever it is bound.
pub fn pagination_scope(members: &[&dyn ::std::fmt::Debug]) -> String {
    let rendered = request_fingerprint(members, None);
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(rendered.as_bytes());
    let mut scope = ::hex::encode(digest);
    scope.truncate(PAGINATION_SCOPE_LENGTH);
    scope
}

/// How many hex characters of the digest a [`pagination_scope`] carries: short
/// enough to keep a token small, wide enough that two enumerations of one service
/// do not collide.
const PAGINATION_SCOPE_LENGTH: usize = 16;

/// Where a page of a BATCH read resumes: the entry still being answered and how
/// many of that entry's data points earlier pages already carried.
///
/// ONE recipe for every batch read of this service that pages ACROSS its entries
/// (`BatchGetAssetPropertyValue`, `BatchGetAssetPropertyAggregates`,
/// `BatchGetAssetPropertyValueHistory`), so the token format and what counts as a
/// token this service never issued cannot drift between them.
///
/// A cursor is a POSITION in the entry list ONE operation was sent, so the token
/// carries the [`pagination_scope`] of that operation and that list beside the
/// position: a position alone names something in every entry list long enough to
/// hold it, and honouring it against another list would report that list's leading
/// entries as "completely processed by previous paginated requests" and drop their
/// data. Hence [`BatchCursor::encode`] and [`BatchCursor::decode`] both take the
/// operation and the entries the token belongs to, and a token replayed against a
/// different list, or against another operation, names no page at all.
#[derive(Debug, Clone, Copy)]
pub struct BatchCursor {
    /// Index into the request's `entries` of the first entry not yet finished.
    pub entry: usize,
    /// How many of THAT entry's data points previous pages already reported.
    pub value: usize,
}

impl BatchCursor {
    /// The cursor of a request that carries no `nextToken`.
    pub const START: Self = Self { entry: 0, value: 0 };

    /// This cursor as a `nextToken` of the `operation` request whose entry list is
    /// `entries`: hex, so the token stays opaque to the caller and inside the
    /// `^[A-Za-z0-9+/=]+$` pattern the paging members of this service carry.
    pub fn encode(self, operation: &str, entries: &[impl ::std::fmt::Debug]) -> String {
        encode_scoped_token(
            &format!("{}{BATCH_CURSOR_SEPARATOR}{}", self.entry, self.value),
            &batch_entries_scope(operation, entries),
        )
    }

    /// The cursor `token` encodes for the `operation` request whose entry list is
    /// `entries`, or `None` for text this service never issued as a token OF THAT
    /// LIST — which its caller answers as the modelled refusal for an invalid
    /// `nextToken`.
    ///
    /// A cursor resuming at or past the END of `entries` is such text too: the list
    /// it is replayed against has no entry there, so it names no page of it. That
    /// check lives here rather than in each caller, so the readers cannot drift in
    /// what they honour.
    pub fn decode(
        token: &str,
        operation: &str,
        entries: &[impl ::std::fmt::Debug],
    ) -> Option<Self> {
        let resume = decode_scoped_token(token, &batch_entries_scope(operation, entries))?;
        let (entry, value) = resume.split_once(BATCH_CURSOR_SEPARATOR)?;
        let cursor = Self {
            entry: token_number(entry)?,
            value: token_number(value)?,
        };
        (cursor.entry < entries.len()).then_some(cursor)
    }
}

/// The [`pagination_scope`] a batch read's page token is bound to: the operation it
/// was sent to, because one text sent to two operations is two tokens, and the entry
/// list of the request, in order and with every member of every entry, because those
/// members decide which data points each entry enumerates and in what order.
fn batch_entries_scope(operation: &str, entries: &[impl ::std::fmt::Debug]) -> String {
    pagination_scope(&[&operation, &entries])
}

/// What joins the two halves of a token [`BatchCursor::encode`] builds.
const BATCH_CURSOR_SEPARATOR: char = ':';

/// One page of an ordered enumeration: the half-open `[start, end)` span of it
/// this call answers with, and the token the next call resumes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Index of the first entry of this page in the ordered enumeration.
    pub start: usize,
    /// Index one past the last entry of this page.
    pub end: usize,
    /// The token that resumes at the entry after this page, or `None` when the
    /// page exhausts the enumeration — "the token for the next set of results,
    /// or null if there are no additional results".
    pub next_token: Option<String>,
}

/// The page of `items` a paginated listing answers with: resume where
/// `next_token` names, take at most `max_results` entries (or
/// `default_max_results` when the caller sends none), and mint the next token
/// from the entry that follows.
///
/// ONE recipe for every listing of this service that pages by a resume KEY taken
/// off its entries, so the token format, the page size and what counts as a
/// token this service never issued cannot drift between operations. `key` is the
/// entry's resume key — unique within the enumeration and stable across calls,
/// which is what makes a token resumable; it is carried in the token hex-encoded,
/// keeping the text inside the `^[A-Za-z0-9+/=]+$` pattern the paging members of
/// this service carry.
///
/// `None` for a `next_token` this service never issued — text that is not the hex
/// of a resume key, hex in a spelling [`encode_page_token`] does not write, or a
/// key no entry of this enumeration carries. Its caller
/// answers that with the modelled refusal for an invalid `nextToken`, rather than
/// with an empty page, which would report absence as success.
pub fn page_of<T, K, S>(
    items: &[T],
    next_token: Option<&str>,
    max_results: Option<i32>,
    default_max_results: i32,
    key: K,
) -> Option<Page>
where
    K: Fn(&T) -> S,
    S: AsRef<str>,
{
    let start = match next_token {
        Some(token) => {
            let resume = decode_page_token(token)?;
            items.iter().position(|item| key(item).as_ref() == resume)?
        }
        None => 0,
    };
    // A page size the model's own range forbids cannot reach a handler — the
    // runtime validates `maxResults` before one runs — so `max(1)` only keeps
    // the slice arithmetic below total.
    let max_results = max_results.unwrap_or(default_max_results).max(1) as usize;
    let end = start.saturating_add(max_results).min(items.len());
    let next_token = items
        .get(end)
        .map(|item| encode_page_token(key(item).as_ref()));
    Some(Page {
        start,
        end,
        next_token,
    })
}

/// A resume key as the opaque `nextToken` that resumes at it.
pub fn encode_page_token(resume: &str) -> String {
    ::hex::encode(resume.as_bytes())
}

/// The resume key `token` encodes, or `None` for text this service never issued
/// as a token.
pub fn decode_page_token(token: &str) -> Option<String> {
    decode_token_text(token)
}

/// The bytes `token` spells, or `None` unless `token` is the ONE spelling this
/// service writes for them.
///
/// Every token of this service is the hex of its resume payload, and hex writes a
/// byte exactly one way, so text is a token this service issued only when
/// re-encoding what it decodes gives back the very text that arrived. Decoding hex
/// alone would also read back every other spelling of the same bytes — the
/// uppercase and mixed-case ones, which no encoder here mints — and serve a page
/// for text the service never issued; the callers below lift the `None` into the
/// modelled refusal for an invalid `nextToken` instead.
pub fn decode_token_bytes(token: &str) -> Option<Vec<u8>> {
    let bytes = ::hex::decode(token).ok()?;
    (::hex::encode(&bytes) == token).then_some(bytes)
}

/// The text `token` spells, under the rule [`decode_token_bytes`] states.
pub fn decode_token_text(token: &str) -> Option<String> {
    String::from_utf8(decode_token_bytes(token)?).ok()
}

/// The number one field of a decoded token holds, or `None` unless `field` is the
/// one spelling this service writes for that number.
///
/// The same rule [`decode_token_bytes`] applies to a token's hex, applied to the
/// numbers inside it: a signed zero, a zero-padded or `+`-prefixed ordinal parses
/// as the same number, but is text no encoder of this service writes, so it is text
/// it never issued as a token.
pub fn token_number<T>(field: &str) -> Option<T>
where
    T: ::std::str::FromStr + ::std::fmt::Display,
{
    let value: T = field.parse().ok()?;
    (value.to_string() == field).then_some(value)
}

/// The `Quality` a TQV that names none carries.
///
/// `AssetPropertyValue$quality` is optional and the enum's values are `GOOD`,
/// `BAD` and `UNCERTAIN` (`smithy_query(query="shape", shape="Quality")`). An
/// ordinary device reading is `GOOD`, so a TQV ingested without an explicit
/// quality is treated as `GOOD`: an omitted quality and an explicit `GOOD` name
/// the SAME quality, and neither can shadow the other at a timestamp the series
/// already holds.
pub const DEFAULT_QUALITY: &str = "GOOD";

/// The quality `value` names, with [`DEFAULT_QUALITY`] standing in for an absent
/// one. This is the comparison the quality-aware write in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::record_property_value`] makes.
pub fn quality_of(value: &AssetPropertyValue) -> &str {
    value.quality.as_deref().unwrap_or(DEFAULT_QUALITY)
}

/// Which stored data stream a request's identifiers name.
///
/// A time series is either ASSOCIATED with an asset property \u2014 the pair of ids
/// names it, and so does any alias bound to that property \u2014 or DISASSOCIATED,
/// which only its alias names. Resolved once in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::resolve_alias_series`] so every
/// operation addressed by an alias reads and writes the same stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesKey {
    /// The stream of the `(assetId, propertyId)` asset property, by stored ids.
    Property(String, String),
    /// A stream no asset property owns, by the alias that identifies it.
    Disassociated(String),
}

/// Put `value` into `series`, which stays sorted ascending by timestamp.
///
/// A value at an already-stored timestamp is QUALITY-AWARE: "For each asset
/// property, IoT SiteWise overwrites TQVs with duplicate timestamps unless the
/// newer TQV has a different quality" (the `BatchPutAssetPropertyValue` model
/// doc), so the stored sample is replaced only when the two name the same
/// quality, and a differing quality leaves the series exactly as it was. Either
/// way the series stays single-valued at that timestamp.
///
/// The series is BOUNDED at [`MAX_SAMPLES_PER_SERIES`]: past that many samples the
/// oldest goes, so no sequence of writes can grow one series without limit. A
/// sample newer than everything stored \u2014 the shape real ingest takes \u2014 takes an
/// append-only fast path and costs no element move, so the sorted insert cannot be
/// driven to quadratic cost under the shared state lock.
fn record_sample(series: &mut Vec<AssetPropertyValue>, value: AssetPropertyValue) {
    let key = timestamp_key(&value.timestamp);
    if series
        .last()
        .is_none_or(|last| timestamp_key(&last.timestamp) < key)
    {
        series.push(value);
    } else {
        match series.binary_search_by(|existing| timestamp_key(&existing.timestamp).cmp(&key)) {
            Ok(at) => {
                if quality_of(&series[at]) != quality_of(&value) {
                    // Accepted and discarded: the stored TQV wins at a timestamp a
                    // differing-quality TQV re-puts.
                    return;
                }
                series[at] = value;
            }
            Err(at) => series.insert(at, value),
        }
    }
    if series.len() > MAX_SAMPLES_PER_SERIES {
        let excess = series.len() - MAX_SAMPLES_PER_SERIES;
        series.drain(..excess);
    }
}

/// A wall-clock instant as the modelled `Timestamp` type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The region to build this request's ARNs in: the one in the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The canonical ARN of an asset model in `region`.
pub fn asset_model_arn(region: &str, asset_model_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("asset-model/{asset_model_id}"))
}

/// The canonical ARN of an asset in `region`.
pub fn asset_arn(region: &str, asset_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("asset/{asset_id}"))
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The ARN a refusal names the stored asset `asset_id` by: the one that
    /// asset CARRIES, read back from the record.
    ///
    /// An ARN is minted once, by the call that created the resource, and stamped
    /// with the region THAT call was signed for; it is then a member of the
    /// resource, reported identically to every later reader. So an error whose
    /// `resourceId`/`resourceArn` point at a resource of this store reports that
    /// stored ARN rather than one rebuilt from the refused request, which would
    /// name a resource no reader of this store can find whenever the two calls
    /// reached it through different regional endpoints.
    ///
    /// Only an id this store holds no asset for leaves nothing to read, and then
    /// the canonical ARN of that id in `region` is the one honest answer — the
    /// case a refusal naming a resource its own call removed is always in.
    pub fn stored_asset_arn(&self, region: &str, asset_id: &str) -> String {
        self.assets
            .get(asset_id)
            .map(|asset| asset.arn.clone())
            .unwrap_or_else(|| asset_arn(region, asset_id))
    }

    /// [`Self::stored_asset_arn`] for an asset model: the ARN the stored model
    /// carries, or the canonical one in `region` when this store holds no such
    /// model.
    pub fn stored_asset_model_arn(&self, region: &str, asset_model_id: &str) -> String {
        self.asset_models
            .get(asset_model_id)
            .map(|model| model.arn.clone())
            .unwrap_or_else(|| asset_model_arn(region, asset_model_id))
    }
}

/// The resource type and resource id an `AmazonResourceName` names, when the
/// string is an ARN of THIS service in the `<type>/<id>` resource form every
/// IoT SiteWise ARN takes (`asset/<assetId>`, `asset-model/<assetModelId>`,
/// `portal/<portalId>`, …).
///
/// `None` for anything that is not such an ARN — a string that is not an ARN at
/// all, or one whose service segment is another service's. The resource type is
/// returned as it was written rather than matched here, so a caller decides
/// which types it holds.
pub fn parse_resource_arn(arn: &str) -> Option<(&str, &str)> {
    // `arn:<partition>:<service>:<region>:<account>:<resource>` — the resource
    // section is the only one that may itself contain a `:`, so it takes the
    // remainder.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, id) = fields.next()?.split_once('/')?;
    if resource_type.is_empty() || id.is_empty() {
        return None;
    }
    Some((resource_type, id))
}

/// The tag-key namespace Amazon Web Services keeps for the tags it applies
/// itself.
///
/// The tag restrictions the `tags` member of every create links to — "Tagging
/// your IoT SiteWise resources" in the IoT SiteWise User Guide — state that this
/// prefix is reserved for Amazon Web Services use and that a caller can neither
/// set nor remove a tag whose key carries it. `TagKey` declares `@length` and no
/// `@pattern`, so no boundary constraint can express the reservation and it is
/// enforced in [`check_tag_key`] below instead, which is what every operation
/// taking caller-supplied tag keys calls.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] names.
///
/// Matched without regard to ASCII case, because the reservation is of the
/// NAMESPACE: a differently cased spelling of the prefix names the same reserved
/// tags, so it may not be used to slip one past the check. The prefix is matched
/// on the key's leading bytes, so a key whose first characters are multi-byte —
/// and therefore cannot spell the ASCII prefix — is not in the namespace.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Why a caller-supplied `key` in the reserved namespace is refused, naming the
/// key the request carried so a caller sending several tags learns which one
/// failed.
pub fn reserved_tag_key_message(key: &str) -> String {
    format!(
        "The tag key '{key}' is not valid: tag keys beginning with \
         '{RESERVED_TAG_KEY_PREFIX}' are reserved for Amazon Web Services use."
    )
}

/// Refuse a tag key a caller may not supply: one outside the modelled `TagKey`
/// bounds, in the wording the boundary validator uses for a map's keys, or one in
/// the namespace [`RESERVED_TAG_KEY_PREFIX`] names.
///
/// The framework's constraint walk reaches a map's VALUES but not its KEYS, so
/// `TagMap`'s `@length(min: 1, max: 128)` on `TagKey` is the handler's to
/// enforce; the length is in characters, as `@length` counts them. The reserved
/// namespace is no constraint the model can carry at all, so it is enforced here
/// too — one function, so every operation that accepts caller-supplied tag keys
/// answers one key the same way, whether the keys arrive on a create or on
/// `TagResource`.
pub fn check_tag_key(key: &str) -> Result<(), String> {
    let length = key.chars().count();
    if !(TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(&length) {
        return Err(format!(
            "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
             Map keys must satisfy constraint: [Member must have length greater than or equal \
             to {TAG_KEY_MIN_LENGTH}, Member must have length less than or equal to \
             {TAG_KEY_MAX_LENGTH}]"
        ));
    }
    if is_reserved_tag_key(key) {
        return Err(reserved_tag_key_message(key));
    }
    Ok(())
}

/// Refuse a caller-supplied value for a member whose modelled documentation
/// states the value IS JSON — a dashboard's definition literal, a gateway
/// capability's configuration document — unless it parses as a JSON document.
///
/// Such a member is held verbatim and reported back verbatim, so a body that is
/// no JSON document defines nothing the client that later reads it could render
/// or apply. No modelled constraint can express the JSON form — the length
/// bounds and patterns the boundary validator enforces before a handler runs say
/// nothing about it — so it is the handler's to check, and one function so every
/// member the documented form holds for is answered the same way. `member` is
/// the member's wire name, which is what the refusal names.
pub fn check_json_document(member: &str, value: &str) -> Result<(), String> {
    if ::serde_json::from_str::<::serde_json::Value>(value).is_err() {
        return Err(format!("The {member} must be a JSON document."));
    }
    Ok(())
}

/// The canonical ARN of a gateway in `region`, in the form `CreateGateway`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:gateway/${GatewayId}`.
pub fn gateway_arn(region: &str, gateway_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("gateway/{gateway_id}"))
}

/// The canonical ARN of a portal in `region`, in the form `CreatePortal`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:portal/${PortalId}`.
pub fn portal_arn(region: &str, portal_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PORTAL_RESOURCE_TYPE}/{portal_id}"),
    )
}

/// The canonical ARN of a project in `region`, in the form `CreateProject`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:project/${ProjectId}`.
pub fn project_arn(region: &str, project_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PROJECT_RESOURCE_TYPE}/{project_id}"),
    )
}

/// The host every IoT SiteWise Monitor portal is served under, as the one
/// `portalStartUrl` is built from.
///
/// A portal's URL is per-portal — `CreatePortal` documents `portalStartUrl` as
/// "The URL for the IoT SiteWise Monitor portal" — so the portal's own id is the
/// subdomain and this is the suffix it sits under.
const PORTAL_HOST_SUFFIX: &str = "app.iotsitewise.aws";

/// The `portalStartUrl` of the portal `portal_id` names.
///
/// DERIVED from the portal id rather than minted, so it is stable across reads
/// and unique per portal without anything being written on a read. Within the
/// modelled `Url` bounds (1..256, `^(http|https)://\S+$`) for every `ID`, which
/// is 36 characters.
pub fn portal_start_url(portal_id: &str) -> String {
    format!("https://{portal_id}.{PORTAL_HOST_SUFFIX}")
}

/// Where a portal's logo image is reported to live: a path under that portal's
/// own URL, keyed by the image id the create minted.
///
/// Derived for the same reason as [`portal_start_url`], and from the portal that
/// owns the image rather than from any store this emulator does not run.
pub fn portal_logo_image_url(portal_id: &str, image_id: &str) -> String {
    format!("{}/logo/{image_id}", portal_start_url(portal_id))
}

/// The canonical ARN of an access policy in `region`, in the form
/// `CreateAccessPolicy` documents:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:access-policy/${AccessPolicyId}`.
pub fn access_policy_arn(region: &str, access_policy_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ACCESS_POLICY_RESOURCE_TYPE}/{access_policy_id}"),
    )
}

/// The canonical ARN of a dataset in `region`, in the form `CreateDataset`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:dataset/${DatasetId}`.
pub fn dataset_arn(region: &str, dataset_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DATASET_RESOURCE_TYPE}/{dataset_id}"),
    )
}

/// The character joining the two ids a `TimeSeriesId` is built from; see
/// [`time_series_id`].
///
/// A `.`, because the id is carried inside `timeSeriesArn` and the modelled `ARN`
/// pattern is `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$` — a separator outside
/// that character class would make every ARN this service reports violate a
/// constraint its own model states.
pub const TIME_SERIES_ID_SEPARATOR: char = '.';

/// The `TimeSeriesId` of the time series (data stream) an asset property owns.
///
/// Every asset property IS a data stream in IoT SiteWise's time-series store, so
/// the series it owns needs an id that is stable across reads and unique per
/// `(assetId, propertyId)` pair. The modelled `TimeSeriesId` is 36..73 characters
/// — one 36-character `ID`, or two of them joined by a single character — so the
/// pair that owns the series names it, derived rather than minted: nothing has to
/// be written on a read for the id to stay the same. Both ids are modelled `ID`s,
/// whose pattern admits neither [`TIME_SERIES_ID_SEPARATOR`] nor any other
/// character outside `[0-9a-f-]`, so one pair cannot be spelled as another.
pub fn time_series_id(asset_id: &str, property_id: &str) -> String {
    format!("{asset_id}{TIME_SERIES_ID_SEPARATOR}{property_id}")
}

/// The canonical ARN of a time series in `region`, in the form the
/// `TimeSeriesSummary$timeSeriesArn` doc states:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:time-series/${TimeSeriesId}`.
pub fn time_series_arn(region: &str, time_series_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("time-series/{time_series_id}"))
}

/// The MQTT topic IoT SiteWise publishes one asset property's value updates to.
///
/// The form is the one the IoT SiteWise User Guide documents for property-value
/// update notifications ("Interacting with other AWS services"):
/// `$aws/sitewise/asset-models/{assetModelId}/assets/{assetId}/properties/{propertyId}`.
/// Built from the three ids, so the topic a property reports always names the
/// property it belongs to rather than being stored and able to drift.
pub fn property_notification_topic(
    asset_model_id: &str,
    asset_id: &str,
    property_id: &str,
) -> String {
    format!(
        "$aws/sitewise/asset-models/{asset_model_id}/assets/{asset_id}/properties/{property_id}"
    )
}

/// The `PropertyNotificationState` a property takes when `UpdateAssetProperty`
/// omits one: the model's own documented default for that member.
pub const NOTIFICATION_STATE_DISABLED: &str = "DISABLED";

// ---------------------------------------------------------------------------
// Selecting one series' stored samples.
//
// A history read asks the same question of a data stream whether it names one
// property or a batch of them: which stored samples fall inside the queried
// range, which qualities are kept and which way round they are reported. The
// selection therefore lives here, beside the series it reads, so
// `GetAssetPropertyValueHistory` and `BatchGetAssetPropertyValueHistory` cannot
// disagree about what a window covers or what a quality filter keeps.
// ---------------------------------------------------------------------------

/// What one history read asks of one stored series.
///
/// Built only by [`HistoryQuery::new`], so a window whose end precedes its start
/// — which names no range at all — cannot reach a series: the constructor hands
/// back `None` for it instead.
#[derive(Debug, Clone, Copy)]
pub struct HistoryQuery<'a> {
    /// The EXCLUSIVE start of the queried range, in nanoseconds of Unix epoch
    /// time: "The exclusive start of the range from which to query historical
    /// data" (the `startDate` member of either history reader). `None` leaves
    /// that side of the range open.
    start_exclusive_nanos: Option<i128>,
    /// The INCLUSIVE end of the queried range: "The inclusive end of the range
    /// from which to query historical data" (the `endDate` member). `None`
    /// leaves that side open.
    end_inclusive_nanos: Option<i128>,
    /// The qualities a sample may carry to be reported — "the quality by which
    /// to filter asset data". A read that names none asks for every sample: a
    /// history read reports the samples themselves, so an absent filter filters
    /// nothing out.
    qualities: Option<&'a [String]>,
    /// `true` for `TimeOrdering` DESCENDING; ASCENDING is the modelled default.
    descending: bool,
}

impl<'a> HistoryQuery<'a> {
    /// The query a request's window, quality filter and ordering name, or `None`
    /// when `end_date` PRECEDES `start_date` — the caller's cue to refuse the
    /// request (or the batch entry) with the modelled `InvalidRequestException`
    /// rather than answer an impossible range with an empty history.
    pub fn new(
        start_date: Option<&::aws_smithy_types::DateTime>,
        end_date: Option<&::aws_smithy_types::DateTime>,
        qualities: Option<&'a [String]>,
        descending: bool,
    ) -> Option<Self> {
        let start_exclusive_nanos = start_date.map(date_time_nanos);
        let end_inclusive_nanos = end_date.map(date_time_nanos);
        if let (Some(start), Some(end)) = (start_exclusive_nanos, end_inclusive_nanos) {
            if end < start {
                return None;
            }
        }
        Some(Self {
            start_exclusive_nanos,
            end_inclusive_nanos,
            qualities,
            descending,
        })
    }

    /// Whether `sample` survives this read's quality filter. The quality a
    /// sample stored without one counts as comes from the shared
    /// [`quality_of`], so this filter and the quality-aware write in
    /// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::record_property_value`]
    /// cannot disagree about it.
    fn keeps_quality(&self, sample: &AssetPropertyValue) -> bool {
        match self.qualities {
            // A filter naming no quality — absent, or the empty list — asks for
            // all of them.
            None | Some([]) => true,
            Some(wanted) => {
                let quality = quality_of(sample);
                wanted.iter().any(|candidate| candidate == quality)
            }
        }
    }
}

/// The samples of the stream `stream` names that `query` selects, in the
/// order it asks for — what either history reader reports for one data stream,
/// associated with an asset property or not.
///
/// The series is the one every data-plane read of this service takes
/// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::resolved_series`]), so a
/// property whose definition carries a formula reports the data points that
/// formula computes as of `now` and an ingested stream reports its samples.
///
/// The series is kept sorted ascending by timestamp, so the range and the
/// quality filter select from it in order and ASCENDING needs no sort at all; a
/// series is bounded at [`MAX_SAMPLES_PER_SERIES`], so the work one read can ask
/// for is bounded however wide a range it names.
pub fn history_values(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    stream: &SeriesKey,
    query: &HistoryQuery<'_>,
    now: SystemTime,
) -> Vec<AssetPropertyValue> {
    let mut selected: Vec<AssetPropertyValue> = store
        .resolved_series(stream, now)
        .iter()
        .filter(|sample| {
            let at = timestamp_nanos(&sample.timestamp);
            query.start_exclusive_nanos.is_none_or(|start| at > start)
                && query.end_inclusive_nanos.is_none_or(|end| at <= end)
        })
        .filter(|sample| query.keeps_quality(sample))
        .cloned()
        .collect();
    if query.descending {
        selected.reverse();
    }
    selected
}

// ---------------------------------------------------------------------------
// Deriving one series' aggregates.
//
// HOISTED HERE from `src/ops/batch_get_asset_property_aggregates.rs`, which asked
// for exactly that once a second caller appeared: "the fixer that implements
// [`GetAssetPropertyAggregates`] should hoist this block into `crate::state` and
// call it from both". `GetAssetPropertyAggregates` and
// `BatchGetAssetPropertyAggregates` now read a series through this one engine, so
// the two cannot disagree about what a window covers or what it reduces to.
// ---------------------------------------------------------------------------

/// Every `Resolution` an aggregate query may name, with the length in seconds of
/// the window it names.
///
/// Source: the `Resolution` shape — `smithy_query(query="shape",
/// shape="Resolution")` reports length 2..3 and pattern `^1m|15m|1h|1d$`, i.e.
/// the four names below ("1m" one minute, "15m" fifteen minutes, "1h" one hour,
/// "1d" one day). The window LENGTHS are what the generated `Resolution` string
/// does not carry, which is why this table exists at all.
pub const RESOLUTIONS: [(&str, i64); 4] =
    [("1m", 60), ("15m", 900), ("1h", 3_600), ("1d", 86_400)];

/// The window length, in seconds, that `resolution` names — `None` when it is not
/// one of [`RESOLUTIONS`].
pub fn resolution_seconds(resolution: &str) -> Option<i64> {
    RESOLUTIONS
        .iter()
        .find(|(name, _)| *name == resolution)
        .map(|(_, seconds)| *seconds)
}

/// A `TimeInNanos` as whole nanoseconds since the Unix epoch, so a sample and a
/// modelled `Timestamp` bound compare at the precision the sample carries.
///
/// Also what an operation that has to do ARITHMETIC on a span reads it through
/// (the instant inside a data segment `GetSearchResults` reports), with
/// [`timestamp_at_nanos`] the inverse — the service's conversion pair, so a
/// handler that converts a whole instant uses these rather than writing the
/// arithmetic again. The interpolation path keeps its own inverse, because an
/// instant the model cannot express has to make that operation SKIP the interval
/// rather than report a saturated one.
pub fn timestamp_nanos(timestamp: &TimeInNanos) -> i128 {
    i128::from(timestamp.time_in_seconds) * NANOS_PER_SECOND
        + i128::from(timestamp.offset_in_nanos.unwrap_or(0))
}

/// How many nanoseconds one whole second is, as the two members of a
/// `TimeInNanos` divide an instant: `timeInSeconds` whole seconds plus an
/// `offsetInNanos` of `0..=999999999`.
const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// The inverse of [`timestamp_nanos`]: whole nanoseconds since the Unix epoch as
/// the modelled `TimeInNanos` — whole seconds plus the nanosecond offset inside
/// that second.
///
/// The split is EUCLIDEAN, so the offset is never negative and never the
/// `1_000_000_000` the modelled `OffsetInNanos` range (0..999999999) excludes,
/// whichever side of the epoch the instant falls.
pub fn timestamp_at_nanos(nanos: i128) -> TimeInNanos {
    let seconds = nanos.div_euclid(NANOS_PER_SECOND);
    TimeInNanos {
        // Saturating in BOTH directions: an instant too far past the epoch reports
        // the largest second the member can carry and one too far before it the
        // smallest, rather than the wrong end of the range.
        time_in_seconds: i64::try_from(seconds)
            .unwrap_or(if seconds.is_negative() { i64::MIN } else { i64::MAX }),
        offset_in_nanos: i32::try_from(nanos.rem_euclid(NANOS_PER_SECOND)).ok(),
    }
}

/// A modelled `Timestamp` as whole nanoseconds since the Unix epoch; see
/// [`timestamp_nanos`].
pub fn date_time_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * NANOS_PER_SECOND + i128::from(at.subsec_nanos())
}

/// What one aggregate query asks of one stored series: the window grid to bucket
/// its samples into, the range to read, the quality to count and which aggregates
/// to compute.
///
/// Built only by [`AggregateQuery::new`], so `window_seconds` can only ever be a
/// length [`resolution_seconds`] returned — the invariant the bucketing arithmetic
/// relies on is therefore unforgeable rather than merely documented.
#[derive(Debug, Clone, Copy)]
pub struct AggregateQuery<'a> {
    /// The window length in seconds. Windows are aligned to the Unix epoch, so
    /// one window covers the same instants whoever asks. Always positive.
    window_seconds: i64,
    /// The EXCLUSIVE start of the queried range, in nanoseconds of Unix epoch
    /// time: "The exclusive start of the range from which to query historical
    /// data" (the `startDate` member of either aggregate operation).
    start_exclusive_nanos: i128,
    /// The INCLUSIVE end of the queried range: "The inclusive end of the range
    /// from which to query historical data" (the `endDate` member).
    end_inclusive_nanos: i128,
    /// The one `Quality` a sample must carry to be counted; an aggregate reports
    /// exactly one quality, and [`DEFAULT_QUALITY`] is what a request naming none
    /// asks for.
    quality: &'a str,
    /// The `AggregateType`s to compute. Every other member of `Aggregates` is
    /// left unset, because the caller did not ask for it.
    aggregate_types: &'a [String],
    /// `true` for `TimeOrdering` DESCENDING; ASCENDING is the modelled default.
    descending: bool,
}

impl<'a> AggregateQuery<'a> {
    /// The query a request's `resolution` names, or `None` when `resolution` is
    /// not one of [`RESOLUTIONS`] — which is the caller's cue to refuse the
    /// request with the modelled `InvalidRequestException` rather than answer it.
    pub fn new(
        resolution: &str,
        start_exclusive_nanos: i128,
        end_inclusive_nanos: i128,
        quality: &'a str,
        aggregate_types: &'a [String],
        descending: bool,
    ) -> Option<Self> {
        Some(Self {
            window_seconds: resolution_seconds(resolution)?,
            start_exclusive_nanos,
            end_inclusive_nanos,
            quality,
            aggregate_types,
            descending,
        })
    }
}

/// The aggregated values of the stream `stream` names under `query` — what either
/// aggregate operation reports for one data stream, associated with an asset
/// property or not.
///
/// DERIVED, never stored: the data points the stream holds — the samples
/// `BatchPutAssetPropertyValue` wrote, or the ones a formula property's formula
/// computes as of `now` — are bucketed into the epoch-aligned windows `query`
/// names and each bucket is reduced to the aggregates the caller asked for,
/// stamped with the instant the window it summarises ENDS. A window that no
/// in-range sample of the asked-for quality falls in is not reported at all, so
/// the length of the answer is a function of the stored data and not of the width
/// of the queried range.
///
/// The pass is over the SAMPLES, not over the windows the range spans: a series is
/// bounded at [`MAX_SAMPLES_PER_SERIES`], so the work one query can ask for is
/// bounded however wide a range it names.
pub fn aggregated_values(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    stream: &SeriesKey,
    query: &AggregateQuery<'_>,
    now: SystemTime,
) -> Vec<AggregatedValue> {
    let series = store.resolved_series(stream, now);
    if series.is_empty() {
        return Vec::new();
    }

    // The series is kept sorted ascending, so one window's samples are contiguous
    // and a single `last_mut()` bucket is all the grouping needs.
    let mut windows: Vec<(i64, Bucket)> = Vec::new();
    for sample in series.iter() {
        let at = timestamp_nanos(&sample.timestamp);
        if at <= query.start_exclusive_nanos || at > query.end_inclusive_nanos {
            continue;
        }
        if quality_of(sample) != query.quality {
            continue;
        }
        // A `nullValue` reports a reading that is absent, so it is not a data
        // point of the window it falls in.
        if sample.value.null_value.is_some() {
            continue;
        }
        let seconds = sample.timestamp.time_in_seconds;
        // `window_seconds` is positive by construction (see
        // [`AggregateQuery::new`]), so this is total.
        let start = seconds.saturating_sub(seconds.rem_euclid(query.window_seconds));
        if windows.last().map(|(at, _)| *at) != Some(start) {
            windows.push((start, Bucket::default()));
        }
        if let Some((_, bucket)) = windows.last_mut() {
            bucket.count = bucket.count.saturating_add(1);
            if let Some(numeric) = numeric_value(&sample.value) {
                bucket.numeric.push(numeric);
            }
        }
    }

    let mut values: Vec<AggregatedValue> = windows
        .iter()
        .filter_map(|(start, bucket)| {
            let value = bucket.reduce(query.aggregate_types);
            reports_an_aggregate(&value).then(|| AggregatedValue {
                // A window is reported at the instant it ENDS — the end of the
                // interval the aggregate summarises, which for a window starting at
                // `start` is `start` plus the length of the resolution's window. The
                // saturating add keeps the last window of the representable range on
                // the largest second the member can carry rather than wrapping.
                timestamp: ::aws_smithy_types::DateTime::from_secs(
                    start.saturating_add(query.window_seconds),
                ),
                quality: Some(query.quality.to_string()),
                value,
            })
        })
        .collect();
    // ASCENDING is the modelled default; DESCENDING is the same windows read the
    // other way.
    if query.descending {
        values.reverse();
    }
    values
}

/// One window's contributing samples, accumulated in a single pass over the
/// series.
#[derive(Debug, Default)]
struct Bucket {
    /// How many data points fell in the window, whatever their type: `count` is
    /// the count of the data points the window's aggregates were computed from.
    count: u64,
    /// The numeric value of each sample that has one. A `STRING` or `BOOLEAN`
    /// sample has none, so it is counted but cannot be summed or averaged.
    numeric: Vec<f64>,
}

impl Bucket {
    /// This window reduced to the requested aggregates. An aggregate the caller
    /// did not ask for is left unset, and so is a numeric aggregate that no sample
    /// in the window can feed.
    ///
    /// The arms name the `AggregateType` members the model declares, generated as
    /// `AGGREGATE_TYPE_VALUES` in `crate::ops::types` — which is what each
    /// operation checks a request against before a series is read.
    fn reduce(&self, aggregate_types: &[String]) -> Aggregates {
        let samples = self.numeric.len();
        let sum: f64 = self.numeric.iter().sum();
        let mean = (samples > 0).then(|| sum / samples as f64);
        let mut aggregates = Aggregates::default();
        for aggregate_type in aggregate_types {
            match aggregate_type.as_str() {
                "COUNT" => aggregates.count = Some(self.count as f64),
                "SUM" => aggregates.sum = (samples > 0).then_some(sum),
                "AVERAGE" => aggregates.average = mean,
                "MAXIMUM" => aggregates.maximum = self.numeric.iter().copied().reduce(f64::max),
                "MINIMUM" => aggregates.minimum = self.numeric.iter().copied().reduce(f64::min),
                // The POPULATION standard deviation: a window's data points are
                // all of that window's data, so nothing is being sampled from a
                // wider set and there is no degree of freedom to lose.
                "STANDARD_DEVIATION" => {
                    aggregates.standard_deviation = mean.map(|mean| {
                        (self
                            .numeric
                            .iter()
                            .map(|value| (value - mean) * (value - mean))
                            .sum::<f64>()
                            / samples as f64)
                            .sqrt()
                    });
                }
                // Unreachable: each operation checks a request's aggregate types
                // against the generated catalogue before the series is read.
                _ => {}
            }
        }
        aggregates
    }
}

/// Whether `aggregates` reports any value at all. A window that can feed none of
/// the requested aggregates is not reported, rather than reported as an empty
/// `Aggregates`.
fn reports_an_aggregate(aggregates: &Aggregates) -> bool {
    aggregates.average.is_some()
        || aggregates.count.is_some()
        || aggregates.maximum.is_some()
        || aggregates.minimum.is_some()
        || aggregates.sum.is_some()
        || aggregates.standard_deviation.is_some()
}

/// The numeric value a `Variant` holds, or `None` when it holds none. A `Variant`
/// sets exactly one member, and only `integerValue` and `doubleValue` are numbers.
fn numeric_value(value: &Variant) -> Option<f64> {
    value
        .double_value
        .or_else(|| value.integer_value.map(f64::from))
}

/// How many ids this process has minted. Part of every id's seed, so no two
/// minted ids collide even when two calls land in the same nanosecond.
static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// A fresh UUIDv4 in the lowercase hyphenated form every `ID` member models.
///
/// Built the way the other services in this workspace mint ids (cf. `mint_hex` in
/// the ivs service): a seed of a monotonic mint counter, the wall clock and the
/// process id, hashed with SHA-256, then given RFC 4122's version-4 and variant
/// bits. Setting the version nibble also makes the all-zero UUID — the one value
/// the modelled `ID` pattern refuses — unreachable.
pub fn new_id() -> String {
    let minted = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seed = format!("iotsitewise-id|{minted}|{}|{nanos}", ::std::process::id());
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The `ConfigurationState` of a configuration that is in force.
pub const CONFIGURATION_STATE_ACTIVE: &str = "ACTIVE";

/// The `ConfigurationState` of a configuration a put has accepted and the
/// service has not finished applying yet.
pub const CONFIGURATION_STATE_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// The resource prefix of a KMS alias, in both the bare `alias/<name>` form and
/// inside an alias ARN.
const KMS_ALIAS_PREFIX: &str = "alias/";

/// The resource prefix of a KMS key's ARN.
const KMS_KEY_PREFIX: &str = "key/";

/// The number of digits in an Amazon Web Services account id, which the account
/// field of an ARN carries.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The number of dash-separated groups in the id KMS mints for a single-Region
/// key.
const KEY_ID_GROUPS: usize = 5;

/// The `mrk-` prefix of the id KMS mints for a multi-Region key, and the number
/// of hexadecimal digits that follow it.
const MULTI_REGION_KEY_PREFIX: &str = "mrk-";
const MULTI_REGION_KEY_ID_DIGITS: usize = 32;

/// The longest ARN this service reports, from the modelled `ARN` type
/// (`len 1..1600`): a key identifier whose ARN would be longer than this names a
/// key the service could not report at all, since `kmsKeyArn` is where it would
/// be reported.
pub const ARN_MAX_LENGTH: usize = 1600;

/// Whether `candidate` is a KMS key id.
///
/// The emulator resolves no KMS key, so only the FORM is judged, and no more
/// narrowly than the id's two shapes: [`KEY_ID_GROUPS`] non-empty groups of
/// hexadecimal digits for a single-Region key, or [`MULTI_REGION_KEY_PREFIX`]
/// and [`MULTI_REGION_KEY_ID_DIGITS`] hexadecimal digits for a multi-Region one.
fn is_kms_key_id(candidate: &str) -> bool {
    let hexadecimal =
        |group: &str| !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_hexdigit());
    if let Some(digits) = candidate.strip_prefix(MULTI_REGION_KEY_PREFIX) {
        return digits.len() == MULTI_REGION_KEY_ID_DIGITS && hexadecimal(digits);
    }
    let mut groups = 0usize;
    for group in candidate.split('-') {
        if !hexadecimal(group) {
            return false;
        }
        groups += 1;
    }
    groups == KEY_ID_GROUPS
}

/// Whether `candidate` is a KMS alias path: [`KMS_ALIAS_PREFIX`] followed by a
/// non-empty name of the characters a KMS alias name admits (`[a-zA-Z0-9:/_-]`).
fn is_kms_alias(candidate: &str) -> bool {
    candidate.strip_prefix(KMS_ALIAS_PREFIX).is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":/_-".contains(&byte))
    })
}

/// Whether `candidate` is the full ARN of a KMS key or alias —
/// `arn:<partition>:kms:<region>:<account>:key/<key id>` or `…:alias/<name>`.
fn is_kms_arn(candidate: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(KMS_ARN_SERVICE), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty()
        && !region.is_empty()
        && account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && (is_kms_alias(resource) || resource.strip_prefix(KMS_KEY_PREFIX).is_some_and(is_kms_key_id))
}

/// The ARN of the KMS key the identifier `key_id` selects, or `None` when it
/// selects none.
///
/// A key identifier comes in the forms AWS KMS accepts for one: a key id, an
/// `alias/<name>` path, or the full ARN of either. The ARN itself is built by
/// [`kms_key_arn`], the one normalisation this service has for a key identifier;
/// what this adds is the JUDGEMENT a member that must carry an ARN needs — the
/// emulator resolves no KMS key, so the form of the identifier is all there is to
/// judge, and an identifier in none of those forms, or one whose ARN names a
/// partition or a Region the modelled `ARN` pattern excludes (see
/// [`is_modelled_arn`]), selects nothing this service could report.
pub fn selected_kms_key_arn(region: &str, key_id: &str) -> Option<String> {
    if !is_kms_key_identifier(key_id) {
        return None;
    }
    let arn = kms_key_arn(region, key_id);
    is_modelled_arn(&arn).then_some(arn)
}

/// Whether `key_id` is a KMS key identifier at all: a key id, an `alias/<name>`
/// path, or the full ARN of either.
fn is_kms_key_identifier(key_id: &str) -> bool {
    if key_id.starts_with("arn:") {
        return is_kms_arn(key_id);
    }
    is_kms_alias(key_id) || is_kms_key_id(key_id)
}

/// The ARN prefixes the modelled `ARN` type's pattern admits, one per partition
/// it names: `^arn:aws(-cn|-us-gov)?:`.
const ARN_PARTITION_PREFIXES: [&str; 3] = ["arn:aws:", "arn:aws-cn:", "arn:aws-us-gov:"];

/// Whether `value` is an ARN this service may report: the whole modelled `ARN`
/// pattern, `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`.
///
/// A caller-supplied ARN is read against this before it is reported back in an
/// ARN member, because a value outside the pattern is one no member of that type
/// can carry, whatever the resource it names.
pub fn is_modelled_arn(value: &str) -> bool {
    ARN_PARTITION_PREFIXES
        .into_iter()
        .find_map(|prefix| value.strip_prefix(prefix))
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-:/_.".contains(&byte))
        })
}

/// The default encryption configuration a put stored: which encryption the
/// account selected and the key it selected for it.
///
/// The record keeps no due instant, because this setting owes no transition: a
/// put of the account's default encryption takes effect with the call, so the
/// configuration it stores is in force from its own response onwards (see
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::default_encryption_configuration`]).
#[derive(Debug, Clone)]
pub struct DefaultEncryptionRecord {
    /// The modelled `EncryptionType` the put named.
    pub encryption_type: String,
    /// The ARN of the KMS key the put selected (see [`kms_key_arn`]); `None`
    /// under [`ENCRYPTION_TYPE_SITEWISE_DEFAULT`], which uses no customer managed key.
    pub kms_key_arn: Option<String>,
}

/// The `ConfigurationState` a configuration whose transition falls due at
/// `active_at` reports at `now`: in force from that instant, and being applied
/// until it.
///
/// Read by every account-wide setting whose change this service applies
/// ASYNCHRONOUSLY — the one that reaches the caller settled is reported
/// [`CONFIGURATION_STATE_ACTIVE`] without consulting the clock at all.
pub fn configuration_state(active_at: SystemTime, now: SystemTime) -> &'static str {
    if now >= active_at {
        CONFIGURATION_STATE_ACTIVE
    } else {
        CONFIGURATION_STATE_UPDATE_IN_PROGRESS
    }
}

/// What the two default-encryption operations report: they answer with the same
/// three members, so they read them from one place.
#[derive(Debug, Clone)]
pub struct DefaultEncryptionConfiguration {
    /// The modelled `encryptionType`.
    pub encryption_type: String,
    /// The modelled `kmsKeyArn`, absent unless the configuration uses a customer
    /// managed key.
    pub kms_key_arn: Option<String>,
    /// The modelled `configurationStatus`.
    pub status: ConfigurationStatus,
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The default encryption configuration the account reports: the one the
    /// last put stored, or the service-owned-key default while nothing has
    /// stored any.
    ///
    /// The state is [`CONFIGURATION_STATE_ACTIVE`] either way, and reads no
    /// clock: this setting takes effect with the put that stores it, so the
    /// configuration reported is always the one in force. Nothing here is
    /// materialised, so an unconfigured account stays unconfigured however
    /// often it is read.
    pub fn default_encryption_configuration(&self) -> DefaultEncryptionConfiguration {
        let status = ConfigurationStatus {
            state: CONFIGURATION_STATE_ACTIVE.to_string(),
            // No error: nothing in this emulator fails to apply a configuration,
            // so the modelled `error` details have nothing to report.
            error: None,
        };
        match &self.default_encryption {
            Some(record) => DefaultEncryptionConfiguration {
                encryption_type: record.encryption_type.clone(),
                kms_key_arn: record.kms_key_arn.clone(),
                status,
            },
            None => DefaultEncryptionConfiguration {
                encryption_type: ENCRYPTION_TYPE_SITEWISE_DEFAULT.to_string(),
                kms_key_arn: None,
                status,
            },
        }
    }

    /// Select `encryption_type` (and `kms_key_arn` with it) as the account's
    /// default encryption configuration, and report what that call answers with.
    ///
    /// The whole setting is replaced: a put is the last word on the account's
    /// encryption, so one naming the service-owned key drops the customer managed
    /// key a previous put selected. The selection is in force as soon as it is
    /// stored, so the answer reports it [`CONFIGURATION_STATE_ACTIVE`].
    pub fn set_default_encryption_configuration(
        &mut self,
        encryption_type: &str,
        kms_key_arn: Option<String>,
    ) -> DefaultEncryptionConfiguration {
        self.default_encryption = Some(DefaultEncryptionRecord {
            encryption_type: encryption_type.to_string(),
            kms_key_arn,
        });
        self.default_encryption_configuration()
    }
}

// ---------------------------------------------------------------------------
// Computation models.
// ---------------------------------------------------------------------------

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotsitewiseAwsiotsitewiseState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotsitewiseAwsiotsitewiseState>)
}

// ---------------------------------------------------------------------------
// The application slice: the record `CreateApplication` writes and
// `DescribeApplication` reports, its bound, the resource kind a refusal names
// and the values an application derives (its ARN, its DNS subdomain and the
// Identity Center application it reports).
//
// One section, like the history-selection and aggregate sections above: what an
// application IS and every value derived from it sit beside each other, so the
// create and the read cannot disagree about either.
// ---------------------------------------------------------------------------

/// The ARN resource-type segment of an application ARN, as [`application_arn`]
/// builds it.
pub const APPLICATION_RESOURCE_TYPE: &str = "application";

/// See [`MAX_ASSET_MODELS`]: the same bound, for the application map.
/// `CreateApplication` is the only thing that adds an application and it declares
/// the modelled `LimitExceededException` (HTTP 410), so that is how this store
/// refuses past the cap. The emulator's own bound, not a published AWS default.
pub const MAX_APPLICATIONS: usize = 1_000;

/// The resource kind a not-found refusal names for an application of a
/// workspace; see [`not_found_message`].
pub const RESOURCE_APPLICATION: &str = "Application";

/// The resource segment an IAM Identity Center application ARN carries, which is
/// what an instance ARN's own `instance/` segment becomes in the ARN of an
/// application created in that instance; see [`idc_application_arn`].
const IDC_INSTANCE_SEGMENT: &str = "instance/";
/// See [`IDC_INSTANCE_SEGMENT`].
const IDC_APPLICATION_SEGMENT: &str = "application/";

/// A stored application of a workspace: everything `DescribeApplication`
/// reports, plus the `tags` the tagging operations read.
///
/// An application is minted with an id of its own and `ListApplications` spans
/// every workspace, so the id identifies it on its own; the workspace it was
/// created in is held here because the modelled reads address an application by
/// `(workspaceName, id)`.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    pub id: String,
    pub arn: String,
    /// The workspace the create named. A read naming another workspace names no
    /// application this store holds, however the id resolves.
    pub workspace_name: String,
    pub name: String,
    pub description: Option<String>,
    /// The Identity Center instance ARN the create named — what
    /// [`Self::idc_application_arn`] was derived from.
    pub idc_instance_arn: String,
    /// The Identity Center application this application reports, derived once
    /// here so every later read reports the same ARN; see
    /// [`idc_application_arn`].
    pub idc_application_arn: String,
    pub dns_subdomain: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The instant the transition the clock owes — `CREATING` to `ACTIVE` —
    /// falls due.
    pub active_at: SystemTime,
    pub tags: HashMap<String, String>,
}

impl ApplicationRecord {
    /// The application's `ApplicationStatus` as of `now`: [`STATE_CREATING`]
    /// until the transition the create owes falls due, [`STATE_ACTIVE`] from
    /// then on. Derived, so a read never has to write.
    ///
    /// `ApplicationStatus` spells only `CREATING`, `ACTIVE` and `DELETING`
    /// (smithy_query query="shape" shape="ApplicationStatus"), so an application
    /// owes exactly this one transition and the settling state is the same for
    /// every record rather than stored per record.
    pub fn status(&self, now: SystemTime) -> String {
        state_at(STATE_CREATING, self.active_at, now).to_string()
    }
}

/// The canonical ARN of an application in `region`.
///
/// The resource section carries the application's own id and not its workspace:
/// unlike a pipeline or a task, an application is minted with an id that
/// identifies it across every workspace, which is also why `ListApplications`
/// takes no workspace.
pub fn application_arn(region: &str, application_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{APPLICATION_RESOURCE_TYPE}/{application_id}"),
    )
}

/// The `dnsSubdomain` an application reports.
///
/// DERIVED from the application's own id rather than minted, for the reason
/// [`portal_start_url`] states: it is then stable across reads and unique per
/// application without anything being written on a read. The modelled
/// `DnsSubdomain` is a DNS label (len 1..63, `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`)
/// and every id [`new_id`] mints is one already — 36 characters of lowercase hex
/// and hyphens, beginning and ending with a hex digit. Derived through this one
/// function so the form has a single place to change if a subdomain ever has to
/// differ from the id.
pub fn application_dns_subdomain(application_id: &str) -> String {
    application_id.to_string()
}

/// The `idcApplicationArn` an application reports: the Identity Center
/// application that was created in the instance `idc_instance_arn` names.
///
/// DERIVED from the two things that identify it — the instance the create named
/// and the application's own id — rather than minted, so it is stable across
/// reads and unique per application. Identity Center nests an application under
/// its instance, so the instance ARN's own resource segment becomes the
/// application one and the application's id is appended; an instance ARN
/// carrying no such segment gets the application segment and the id appended
/// instead, so every application still reports an ARN that names it uniquely.
///
/// Split at the FIRST `instance/`, which in an instance ARN is the one its
/// resource section opens with: an ARN's resource section is its last field, so
/// nothing ahead of that segment can be a second one.
pub fn idc_application_arn(idc_instance_arn: &str, application_id: &str) -> String {
    match idc_instance_arn.split_once(IDC_INSTANCE_SEGMENT) {
        Some((prefix, instance_id)) => {
            format!("{prefix}{IDC_APPLICATION_SEGMENT}{instance_id}/{application_id}")
        }
        None => format!("{idc_instance_arn}/{IDC_APPLICATION_SEGMENT}{application_id}"),
    }
}

// An application's tags live on its own record, reached by ARN like every other
// taggable resource's; the shared `TaggedRecord` seam is what
// `ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource` resolves it
// through.
tagged_record!(ApplicationRecord);

// ---------------------------------------------------------------------------
// The dashboard slice: the record `CreateDashboard` writes, `UpdateDashboard`
// rewrites and `DescribeDashboard` reports, its bound, the resource kind a
// refusal names and its ARN.
//
// One section, like the application slice above: what a dashboard IS and every
// value derived from it sit beside each other, so the create, the update and the
// read cannot disagree about either.
// ---------------------------------------------------------------------------

/// The ARN resource-type segment of a dashboard ARN, as [`dashboard_arn`] builds
/// it.
pub const DASHBOARD_RESOURCE_TYPE: &str = "dashboard";

/// See [`MAX_ASSET_MODELS`]: the same bound, for the dashboard map.
/// `CreateDashboard` is the only thing that adds a dashboard and it declares the
/// modelled `LimitExceededException` (HTTP 410), so that is how this store
/// refuses past the cap. The emulator's own bound, not a published AWS default.
pub const MAX_DASHBOARDS: usize = 1_000;

/// The resource kind a not-found refusal names for an IoT SiteWise Monitor
/// dashboard; see [`not_found_message`].
pub const RESOURCE_DASHBOARD: &str = "Dashboard";

/// A stored IoT SiteWise Monitor dashboard: everything `DescribeDashboard`
/// reports, plus the `tags` the tagging operations read.
///
/// A dashboard has no lifecycle state of its own — no operation on one answers a
/// status member — so, unlike a portal or an application, nothing here is owed a
/// later transition; the two dates are what a read derives from.
#[derive(Debug, Clone)]
pub struct DashboardRecord {
    pub id: String,
    pub arn: String,
    /// The project the create named, which every read of the dashboard reports
    /// and which the listing form filters on. Not the update's to change: no
    /// member of `UpdateDashboard` carries a project.
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The dashboard definition JSON literal, held exactly as the last write
    /// stated it: the service stores the literal and reports it back unparsed,
    /// so each write path checks the member's documented form — see
    /// [`check_json_document`] — before a value reaches here.
    pub definition: String,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The `clientToken` the create that minted this dashboard carried, if it
    /// carried one, recorded as part of what that call asked for. Not the
    /// update's to change. What recognises a RETRY of that create is the store's
    /// registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
    pub tags: HashMap<String, String>,
}

impl DashboardRecord {
    /// Replace the caller-owned members with what an update stated, and record
    /// that instant as the dashboard's last update.
    ///
    /// The request describes the dashboard's NEW state rather than patching
    /// selected members, so an omitted optional member clears the stored one —
    /// the same rule `UpdateAsset` applies to an asset's description. The
    /// dashboard's identity and provenance — its id, its ARN, its project, its
    /// tags and its creation date — are not that call's to change, and no member
    /// of its input carries them.
    pub fn apply_update(
        &mut self,
        name: String,
        description: Option<String>,
        definition: String,
        now: SystemTime,
    ) {
        self.name = name;
        self.description = description;
        self.definition = definition;
        self.last_update_date = now;
    }
}

/// The canonical ARN of a dashboard in `region`, in the form `CreateDashboard`
/// documents:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:dashboard/${DashboardId}`.
pub fn dashboard_arn(region: &str, dashboard_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DASHBOARD_RESOURCE_TYPE}/{dashboard_id}"),
    )
}

// A dashboard's tags live on its own record, reached by ARN like every other
// taggable resource's; the shared `TaggedRecord` seam is what
// `ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource` resolves it
// through.
tagged_record!(DashboardRecord);

// ---------------------------------------------------------------------------
// The bulk import job slice: the record `CreateBulkImportJob` writes and
// `DescribeBulkImportJob` reports, the lifecycle a created job is owed, the
// bounds the request is read against and the resource kind a refusal names.
//
// One section, like the slices above: what a job IS and every value derived
// from it sit beside each other, so the create and the read cannot disagree
// about either.
// ---------------------------------------------------------------------------

/// The `JobStatus` a job reports while the import it asked for has not started:
/// "PENDING – IoT SiteWise is waiting for the current bulk import job to
/// finish".
///
/// Source: `smithy_query(query="shape", shape="JobStatus")`, whose values are
/// also in `crate::ops::types::JOB_STATUS_VALUES`.
pub const BULK_IMPORT_JOB_PENDING: &str = "PENDING";

/// The `JobStatus` a job reports while it is being worked on: "RUNNING – IoT
/// SiteWise is processing your request to import your data from Amazon S3".
pub const BULK_IMPORT_JOB_RUNNING: &str = "RUNNING";

/// The terminal `JobStatus` a job reaches: "COMPLETED – IoT SiteWise
/// successfully completed your request to import data from Amazon S3".
///
/// WHY the terminal record is this value and not one of the failure ones: the
/// three remaining values report outcomes something would have had to DIAGNOSE.
/// `CANCELLED` needs a call that cancels a job, which this service models none
/// of. `FAILED` and `COMPLETED_WITH_FAILURES` state that the import was
/// attempted and that the reasons are "saved in the specified error report
/// location in Amazon S3" — a report nothing here writes — so reporting either
/// would invent a diagnosis, and would make every job a caller starts look
/// broken when nothing about the request was. The status this emulator records
/// is the lifecycle the create OWED (see [`BulkImportJobRecord`]), and the
/// lifecycle every other owed transition in this service settles into is its
/// success state, so this is the consistent terminal record for one.
pub const BULK_IMPORT_JOB_COMPLETED: &str = "COMPLETED";

/// How long each step of a bulk import job's lifecycle takes: a created job
/// stays [`BULK_IMPORT_JOB_PENDING`] for this long, then reports
/// [`BULK_IMPORT_JOB_RUNNING`] for this long, then is
/// [`BULK_IMPORT_JOB_COMPLETED`].
///
/// CHOSEN, not documented: AWS publishes no duration for either step — an import
/// takes as long as the data it moves. The span is deliberately NON-ZERO, so a
/// caller that describes a job straight after creating it still observes the
/// transient status the real service reports, and short enough that a caller can
/// poll the job through to its terminal status.
pub const BULK_IMPORT_JOB_STEP: Duration = Duration::from_millis(500);

/// How many files one bulk import job may name.
///
/// DOCUMENTED, not chosen: the `files` member states it — "You can specify up to
/// 100 files for each bulk import job" — and the list shape carries no length
/// constraint of its own, so the create is what enforces it.
pub const MAX_FILES_PER_BULK_IMPORT_JOB: usize = 100;

/// How many bulk import jobs the store holds.
///
/// The same kind of bound, and for the same reason, as [`MAX_DASHBOARDS`]: a
/// create is the only thing that adds a job and nothing evicts one, so without a
/// cap a stream of creates would grow one emulator process without limit. The
/// emulator's own bound, not a published AWS quota; `CreateBulkImportJob`
/// refuses past it with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_BULK_IMPORT_JOBS: usize = 1_000;

/// The resource kind a not-found refusal names for a bulk import job; see
/// [`not_found_message`].
pub const RESOURCE_BULK_IMPORT_JOB: &str = "Bulk Import Job";
