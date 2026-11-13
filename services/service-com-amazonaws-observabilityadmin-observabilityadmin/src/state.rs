// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-observabilityadmin-observabilityadmin service.
//!
//! The service's primary resource is the **telemetry rule**. A rule is created in
//! the region the request was signed for — its *home* region — and, when the rule
//! carries a multi-region scope (`Regions` or `AllRegions`), the service fans it out
//! into every other region the scope names. Both the original and each fan-out
//! replica are stored here as one record apiece, keyed by `(region, RuleName)`:
//! `RuleName` is unique per region on AWS, so the region is part of the key and a
//! read only ever resolves a rule that lives in the region the caller signed for.
//!
//! A replica does not exist the instant the create returns: its record carries the
//! epoch-millisecond instant at which the fan-out the create started comes due, and
//! every read treats a record whose instant has not arrived as not yet there. That
//! is also where the `PENDING` -> `ACTIVE` replication status a `GetTelemetryRule`
//! reports comes from, so no operation ever has to sleep and no read has to write.
//!
//! The service's second resource is the **telemetry pipeline**, stored the same way:
//! one record per pipeline, keyed by `(region, Name)`, carrying the configuration body
//! the caller supplied and the epoch-millisecond instant at which the `CREATING` ->
//! `ACTIVE` transition the create started comes due. A read derives the status from
//! that instant and the clock, so again no operation sleeps and no read writes.
//!
//! The service's other rule resource is the **organization centralization rule**,
//! held in [`ComAmazonawsObservabilityadminObservabilityadminState::centralization_rules`]
//! and keyed by `(CreatedRegion, RuleName)` for the same reason: the rule is created
//! in the region the request was signed for, and
//! `ListCentralizationRulesForOrganization`'s `AllRegions` flag is what decides
//! whether a list spans the other regions' rules or only that one. A centralization
//! rule is not fanned out — it names its source regions and its destination in its
//! own body — but it does carry the instant its provisioning comes due, which is
//! where the `Provisioning` -> `Healthy` `RuleHealth` a read reports comes from.
//!
//! The service's **S3 Table integration** is the one resource here a caller does not
//! name: `CreateS3TableIntegration` carries only an encryption configuration, a role
//! ARN and tags, so the service mints the integration's id, keys the record by
//! `(region, id)` and hands back the ARN that id goes into — the only handle
//! `GetS3TableIntegration` and `DeleteS3TableIntegration` take. No instant is stored
//! on it: the modelled `IntegrationStatus` carries no transient creating value, only
//! `ACTIVE` and `DELETING`, so a created integration is in service from the instant
//! the create returns and the clock owes it no transition. The integration is also a
//! SINGLETON: an account holds at most one of them at the regional endpoint it calls,
//! so a create while the caller's region already holds one is refused rather than
//! stored ([`ONE_INTEGRATION_PER_ACCOUNT_MESSAGE`]) and a delete is what frees the
//! slot.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::test_telemetry_pipeline::{
    declared_sections, parse_configuration, Node, PROCESSOR_KEY, SINK_KEY, SOURCE_KEY,
};
use crate::ops::types::{
    CentralizationRule, CentralizationRuleSummary, ConfigurationSummary, DataSource, Encryption,
    IntegrationSummary, RegionStatus, Source, TelemetryPipeline, TelemetryPipelineConfiguration,
    TelemetryPipelineProjection, TelemetryPipelineStatusReason, TelemetryPipelineSummary,
    TelemetryRule, TelemetryRuleSummary, ACTION_VALUES, DELETION_PROTECTION_VALUES,
    DESTINATION_TYPE_VALUES,
    ENCRYPTED_LOG_GROUP_STRATEGY_VALUES, ENCRYPTION_CONFLICT_RESOLUTION_STRATEGY_VALUES,
    ENCRYPTION_SCOPE_VALUES, ENCRYPTION_STRATEGY_VALUES, FILTER_BEHAVIOR_VALUES,
    FILTER_REQUIREMENT_VALUES, LOG_TYPE_VALUES, MSK_ENHANCED_MONITORING_LEVEL_VALUES,
    OUTPUT_FORMAT_VALUES, RESOURCE_TYPE_VALUES, S_S_E_ALGORITHM_VALUES,
    TAG_CONFLICT_RESOLUTION_STRATEGY_VALUES, TELEMETRY_SOURCE_TYPE_VALUES, TELEMETRY_TYPE_VALUES,
    W_A_F_LOG_TYPE_VALUES,
};

/// The ARN service namespace for this service (`arnNamespace` / sigv4 name
/// `observabilityadmin`).
pub const ARN_SERVICE: &str = "observabilityadmin";

/// The resource portion a telemetry-rule ARN carries before its name, so an ARN
/// reads `arn:aws:observabilityadmin:<region>:<account>:telemetry-rule/<RuleName>`.
pub const RULE_ARN_RESOURCE_PREFIX: &str = "telemetry-rule/";

/// The `ResourceType` a modelled error names the telemetry-rule RESOURCE by.
///
/// `ResourceNotFoundException$ResourceType` is "the type of the resource which could
/// not be found", so it carries the resource's own API name — the noun the service's
/// shapes and its own refusal sentences use — and never an ARN or store token.
pub const RULE_RESOURCE_TYPE: &str = "Rule";

/// The `ResourceType` a duplicate-name `ConflictException` names a telemetry rule by.
///
/// A create refused because the name is already in use conflicts on the INPUT MEMBER
/// that carried the name, not on the resource, so that member is what the refusal
/// names — `CreateTelemetryRule$RuleName` here.
pub const RULE_NAME_RESOURCE_TYPE: &str = "RuleName";

/// The `ResourceNotFoundException` message for a READ of a telemetry rule no record
/// answers: fixed text, with no part of the request in it.
pub const RULE_NOT_FOUND_MESSAGE: &str = "Rule not found.";

/// The `ConflictException` message for a create whose rule name is already in use:
/// fixed text, with the submitted name NOT interpolated.
pub const RULE_ALREADY_EXISTS_MESSAGE: &str = "Telemetry Rule already exists.";

/// The resource portion a telemetry-pipeline ARN carries before the id this service
/// assigns the pipeline, so an ARN reads
/// `arn:aws:observabilityadmin:<region>:<account>:telemetry-pipeline/<id>`.
///
/// The resource type is the resource's own API name, the same hyphenated form
/// [`RULE_ARN_RESOURCE_PREFIX`] takes. What follows it is NOT the submitted `Name`:
/// the pipeline's handle is the id the service assigns it ([`new_resource_id`]), and
/// the name is the OTHER identifier `PipelineIdentifier` ("name or ARN") accepts.
pub const PIPELINE_ARN_RESOURCE_PREFIX: &str = "telemetry-pipeline/";

/// The `ResourceNotFoundException` message for a read of a telemetry pipeline no
/// record answers: fixed text, with no part of the request in it.
pub const PIPELINE_NOT_FOUND_MESSAGE: &str = "Pipeline not found";

/// The `ConflictException` message for a create whose pipeline name is already in
/// use. Fixed text that names the INPUT MEMBER whose value collided
/// (`CreateTelemetryPipeline$Name` is modelled as the pipeline name), not the value
/// itself.
pub const PIPELINE_NAME_ALREADY_EXISTS_MESSAGE: &str = "The specified PipelineName already exists";

/// The resource portion an organization centralization rule's ARN carries before its
/// name, so an ARN reads
/// `arn:aws:observabilityadmin:<region>:<account>:centralization-rule/<RuleName>`.
pub const CENTRALIZATION_RULE_ARN_RESOURCE_PREFIX: &str = "centralization-rule/";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most telemetry rules a caller may create.
///
/// Emulator resource guard, NOT an AWS quota: this service publishes no rule quota.
/// Only caller-created (home) rules count against it; [`MAX_STORED_RULE_RECORDS`] is
/// the cap that covers the fan-out replicas too. A create past either cap is refused
/// with the modelled `ServiceQuotaExceededException`.
pub const MAX_TELEMETRY_RULES: usize = 500;

/// The most records — caller-created rules AND fan-out replicas together — this
/// emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. It is the cap that dominates the only
/// growth site a request can drive: a create or update fans the rule out into at most
/// [`MAX_REPLICATION_TARGETS`] spoke regions, so this bounds the PRODUCT of rules and
/// regions, which `MAX_TELEMETRY_RULES` alone does not. A create or update that would
/// take the store past it is refused with the modelled
/// `ServiceQuotaExceededException` before a single replica is allocated.
pub const MAX_STORED_RULE_RECORDS: usize = 8_000;

/// The most spoke regions one rule may be fanned out into.
///
/// Emulator resource guard, NOT an AWS quota: the model gives `Rule.Regions` a
/// minimum length and no maximum, so this is where the maximum lives. Every named
/// region must also be a region of the home region's own partition
/// ([`replication_rejection`]), which already bounds a valid list by the size of that
/// partition; this constant is the independent clamp, so the fan-out is bounded even
/// if a future caller of [`replication_targets`] forgets to validate first.
pub const MAX_REPLICATION_TARGETS: usize = 50;

/// How long after a create or update the fan-out into a spoke region comes due.
///
/// A CHOSEN span, not an AWS-documented duration: AWS reports a freshly replicated
/// rule as `PENDING` in its spoke regions and only later as `ACTIVE`, and this is
/// the span this emulator takes to get there. It is deliberately non-zero, so a
/// caller that reads straight after the create still observes `PENDING`.
pub const REPLICATION_DELAY_MS: i64 = 400;

/// Replication status of a rule in a spoke region the fan-out has not reached yet.
///
/// From `RegionStatus$Status`' own documentation: "For telemetry rules, valid values
/// include `PENDING`, `ACTIVE`, and `FAILED`."
pub const REGION_STATUS_PENDING: &str = "PENDING";

/// Replication status of a rule the fan-out has created in a spoke region.
pub const REGION_STATUS_ACTIVE: &str = "ACTIVE";

/// Replication status of a spoke region the fan-out could not create the rule in.
pub const REGION_STATUS_FAILED: &str = "FAILED";

/// The most telemetry pipelines a caller may hold in ONE region.
///
/// Emulator resource guard, NOT an AWS quota: this service publishes no pipeline
/// quota. It is the per-region budget; [`MAX_STORED_PIPELINE_RECORDS`] is the
/// store-wide cap that dominates it, because the region a record is keyed by comes
/// from the caller's own credential scope. A create past either cap is refused with
/// the modelled `ServiceQuotaExceededException` before a record is allocated.
pub const MAX_TELEMETRY_PIPELINES: usize = 200;

/// The most telemetry pipeline records this emulator will hold ACROSS every region.
///
/// Emulator resource guard, NOT an AWS quota, and the bound that dominates the only
/// growth site a request can drive: a create keys its record by the region of the
/// caller's SigV4 credential scope ([`request_region`]), so a per-region cap alone
/// would hand a caller a fresh budget for every distinct scope it signs with. This
/// cap bounds the PRODUCT of regions and pipelines, which `MAX_TELEMETRY_PIPELINES`
/// alone does not — the same reason [`MAX_STORED_RULE_RECORDS`] sits beside
/// [`MAX_TELEMETRY_RULES`]. Each record also holds a configuration body the model
/// bounds at 24000 characters, so this cap bounds the stored bytes too.
pub const MAX_STORED_PIPELINE_RECORDS: usize = 2_000;

/// How long after a `CreateTelemetryPipeline` the pipeline's `CREATING` -> `ACTIVE`
/// transition comes due.
///
/// A CHOSEN span, not an AWS-documented duration: AWS reports a freshly created
/// pipeline as `CREATING` and only later as `ACTIVE`, and this is the span this
/// emulator takes to get there. It is deliberately non-zero, so a caller that reads
/// straight after the create still observes `CREATING`.
pub const PIPELINE_ACTIVATION_DELAY_MS: i64 = 400;

/// Status of a pipeline whose creation has not come due yet. One of
/// `TelemetryPipelineStatus`' own values (`TELEMETRY_PIPELINE_STATUS_VALUES`).
pub const PIPELINE_STATUS_CREATING: &str = "CREATING";

/// Status of a pipeline whose creation has come due.
pub const PIPELINE_STATUS_ACTIVE: &str = "ACTIVE";

/// Status of a pipeline whose accepted configuration update has not come due yet.
/// One of `TelemetryPipelineStatus`' own values
/// (`TELEMETRY_PIPELINE_STATUS_VALUES`), reported for the same span, and for the same
/// reason, as [`PIPELINE_STATUS_CREATING`]: the work an accepted update starts is not
/// finished the instant the call returns.
pub const PIPELINE_STATUS_UPDATING: &str = "UPDATING";

/// The `StatusReason$Description` a read reports for an ACTIVE pipeline.
///
/// Wholly fixed text, with no part of the request or of the record in it: the sentence
/// the recorded response carries for this status.
pub const PIPELINE_ACTIVE_STATUS_REASON: &str = "Pipeline is active and processing data";

/// The `StatusReason$Description` a read reports while the pipeline's creation has not
/// come due yet. Only the ACTIVE sentence is recorded; this one is this emulator's own,
/// in the same form, for the status it reports in the meantime.
pub const PIPELINE_CREATING_STATUS_REASON: &str = "Pipeline is being created";

/// The `StatusReason$Description` a read reports while an accepted update has not come
/// due yet. This emulator's own sentence, in the same form as the one above, for the
/// status it reports in the meantime.
pub const PIPELINE_UPDATING_STATUS_REASON: &str = "Pipeline is being updated";

/// The `ValidationException` message a telemetry-pipeline operation refuses a
/// configuration with: fixed text, with no part of the request in it, because the
/// per-field detail travels in `ValidationException$Errors` — "[t]he errors in the
/// input which caused the exception" — rather than in the sentence.
pub const PIPELINE_VALIDATION_ERRORS_MESSAGE: &str = "Validation errors found";

/// The value a data source's `Type` carries when the configuration document names none
/// and the source is one that reads log events AWS vends (see
/// [`VENDED_LOG_SOURCE_PLUGINS`]).
///
/// The recorded summary reports this token for such a data source.
pub const DEFAULT_DATA_SOURCE_TYPE: &str = "default";

/// The property a source plugin lists its data sources under.
const DATA_SOURCES_KEY: &str = "data_sources";

/// The source plugins that read log events AWS itself vends, as opposed to a
/// third-party source the pipeline pulls from.
///
/// `DataSource`'s own documentation (`smithy_query(query="shape",
/// shape="DataSource")`) reports a data source differently for the two kinds: "For
/// CloudWatch Logs sources, this includes both a name and type extracted from the log
/// event metadata. For third-party sources (such as S3), this includes only a name, with
/// the type field left empty." `DataSource$Name` adds that for a third-party source the
/// name "is either the configured `data_source_name` or defaults to the plugin name if
/// not specified". [`data_source`] below is that split.
///
/// The model states no catalogue of source plugins, so this set is not derivable from
/// it: it holds the AWS-vended source types the recorded validation report names as its
/// restrictions' own `source_type` descriptors, and a plugin it does not list is read as
/// a third-party source. A recording naming another vended source type extends it.
const VENDED_LOG_SOURCE_PLUGINS: &[&str] = &[
    "cloudwatch_logs",
    "amazon_vpc_flow",
    "amazon_cloudfront",
];

/// The properties a data source's NAME and TYPE are declared under on a source plugin's
/// own block, which `DataSource`'s documentation names: `Name` "corresponds to the
/// `data_source_name` from the log event metadata" or is "the configured
/// `data_source_name`", and `Type` "corresponds to the `data_source_type`".
const DATA_SOURCE_NAME_KEY: &str = "data_source_name";
const DATA_SOURCE_TYPE_KEY: &str = "data_source_type";

/// The properties an ENTRY of a `data_sources` list declares its name and type under:
/// inside that list each entry is a data source already, so it carries the modelled
/// member names rather than repeating the `data_source_` prefix.
const NAME_KEY: &str = "name";
const TYPE_KEY: &str = "type";

/// The `ResourceType` a modelled error names the telemetry-pipeline RESOURCE by —
/// its own API name, for the reason [`RULE_RESOURCE_TYPE`] states.
pub const PIPELINE_RESOURCE_TYPE: &str = "Pipeline";

/// The `ResourceType` a duplicate-name `ConflictException` names a pipeline by: the
/// input member that carried the colliding name, for the reason
/// [`RULE_NAME_RESOURCE_TYPE`] states.
pub const PIPELINE_NAME_RESOURCE_TYPE: &str = "PipelineName";

/// The resource portion an S3 Table integration's ARN carries before its id, so an
/// ARN reads
/// `arn:aws:observabilityadmin:<region>:<account>:s3tableintegration/<id>`.
///
/// The resource type is this service's own name for the resource, written as one
/// unseparated token: that is the form an integration is addressed by, so an ARN a
/// caller stores, names in a policy or hands back to
/// `GetS3TableIntegration`/`DeleteS3TableIntegration` is the same string on both
/// sides.
///
/// `CreateS3TableIntegration` takes no caller-supplied name — only an `Encryption`
/// configuration, a `RoleArn` and `Tags` — so the id is minted by the service
/// ([`new_s3_table_integration_id`]) and the ARN it goes into is the only handle a
/// caller ever has on the integration: `GetS3TableIntegration` and
/// `DeleteS3TableIntegration` both take `Arn`.
pub const S3_TABLE_INTEGRATION_ARN_RESOURCE_PREFIX: &str = "s3tableintegration/";

/// The `ResourceType` a modelled error names the S3 Table integration RESOURCE by —
/// its own API name (the noun its summary, status and refusal shapes use), for the
/// reason [`RULE_RESOURCE_TYPE`] states.
pub const S3_TABLE_INTEGRATION_RESOURCE_TYPE: &str = "Integration";

/// The `ResourceNotFoundException` message for a read of an S3 Table integration no
/// record answers: fixed text, with the requested ARN NOT interpolated.
pub const INTEGRATION_NOT_FOUND_MESSAGE: &str = "Integration not found";

/// The `ResourceNotFoundException` message a MUTATING S3 Table integration operation
/// reports for an `Arn` no record answers: the integration it would have changed,
/// named by the id its ARN carries, and the account that asked for it.
///
/// Its two variable parts are that id ([`arn_resource_id`], read off the request) and
/// the requester's own 12-digit account id (`engine::emulator_account_id`, the
/// identity every request here authenticates as); the rest is fixed text. It is
/// deliberately a different sentence from the read path's
/// [`INTEGRATION_NOT_FOUND_MESSAGE`], the same way a mutating telemetry-rule refusal
/// words itself differently from that resource's read path
/// ([`resource_does_not_exist_message`] beside [`RULE_NOT_FOUND_MESSAGE`]).
pub fn integration_not_found_for_requester_message(
    integration_id: &str,
    requester_account_id: &str,
) -> String {
    format!("Integration {integration_id} from requesterAccountId {requester_account_id} not found")
}

/// The `ConflictException` message for a create while the account already holds an
/// integration at the regional endpoint the request was signed for: the integration
/// is a SINGLETON, and the whole sentence is fixed text.
pub const ONE_INTEGRATION_PER_ACCOUNT_MESSAGE: &str =
    "Only one S3 Table integration is allowed per account";

/// `IntegrationStatus` of an S3 Table integration that is in service. One of
/// `IntegrationStatus`' own two values (`INTEGRATION_STATUS_VALUES`).
///
/// The modelled enum carries NO transient creating value — only `ACTIVE` and
/// `DELETING` — so a created integration is in service from the instant the create
/// returns, and no clock transition is owed on it.
pub const INTEGRATION_STATUS_ACTIVE: &str = "ACTIVE";

/// The service namespace of the S3 Tables table bucket an integration delivers
/// into, as reported by `GetS3TableIntegration$DestinationTableBucketArn`.
pub const TABLE_BUCKET_ARN_SERVICE: &str = "s3tables";

/// The resource portion a table bucket's ARN carries before its name, so the ARN
/// reads `arn:aws:s3tables:<region>:<account>:bucket/<name>`.
pub const TABLE_BUCKET_ARN_RESOURCE_PREFIX: &str = "bucket/";

/// The name prefix of the table bucket an integration's create NAMES. The bucket is
/// the integration's own, so its name carries the integration id and two integrations
/// never share one.
pub const TABLE_BUCKET_NAME_PREFIX: &str = "aws-cloudwatch-telemetry-";

/// The `SSEAlgorithm` value that encrypts with a KMS key; the other one (`AES256`) is
/// S3 managed encryption and takes none.
pub const SSE_ALGORITHM_KMS: &str = "aws:kms";

/// The most S3 Table integration records this emulator will hold ACROSS every
/// region. Emulator resource guard, NOT an AWS quota.
pub const MAX_STORED_S3_TABLE_INTEGRATION_RECORDS: usize = 500;

/// How long after a `StartTelemetryEvaluation` / `StopTelemetryEvaluation` the
/// telemetry-config feature of one region reaches its terminal status.
///
/// A CHOSEN span, not an AWS-documented duration: the modelled `Status` enum carries
/// the transient `STARTING` and `STOPPING` values either side of the terminal
/// `RUNNING` and `STOPPED`, and this is how long this emulator reports the transient
/// one for. Deliberately non-zero, so a caller that reads straight after the start
/// still observes `STARTING` — which is what makes the feature's own poll loop
/// observable.
pub const EVALUATION_TRANSITION_DELAY_MS: i64 = 400;

/// `Status` of a telemetry-config feature no `Start` has ever been called for.
///
/// This and the four constants below are values of the modelled `Status` enum, whose
/// full value list is what `smithy_query(query="shape",shape="Status")` reports:
/// `NOT_STARTED`, `STARTING`, `FAILED_START`, `RUNNING`, `STOPPING`, `FAILED_STOP`,
/// `STOPPED`. The two `FAILED_*` values are the ones this emulator never reports:
/// nothing here can fail to onboard or offboard a region, and `FailureReason`'s own
/// documentation says that member "will only be populated if `Status` is
/// `FAILED_START` or `FAILED_STOP`" — so it is never populated either.
pub const STATUS_NOT_STARTED: &str = "NOT_STARTED";

/// `Status` while the onboarding a `Start` began has not come due.
pub const STATUS_STARTING: &str = "STARTING";

/// `Status` of a region whose onboarding has come due.
pub const STATUS_RUNNING: &str = "RUNNING";

/// `Status` while the offboarding a `Stop` began has not come due.
pub const STATUS_STOPPING: &str = "STOPPING";

/// `Status` of a region whose offboarding has come due.
pub const STATUS_STOPPED: &str = "STOPPED";

/// `TelemetryEnrichmentStatus` of an enrichment feature that is enabled.
///
/// This and [`ENRICHMENT_STATUS_STOPPED`] are values of the modelled
/// `TelemetryEnrichmentStatus` enum, whose values
/// `smithy_query(query="shape",shape="TelemetryEnrichmentStatus")` reports as
/// `Running`, `Stopped` and `Impaired`. `Impaired` is the value this emulator never
/// reports: it describes a managed view this emulator cannot break, since nothing
/// here can degrade the Resource Explorer view the feature reads.
pub const ENRICHMENT_STATUS_RUNNING: &str = "Running";

/// `TelemetryEnrichmentStatus` of an enrichment feature that is disabled.
pub const ENRICHMENT_STATUS_STOPPED: &str = "Stopped";

/// The ARN service namespace of the Resource Explorer managed view the telemetry
/// enrichment feature reads, as the modelled `AwsResourceExplorerManagedViewArn`
/// pattern (`^arn:aws([a-z0-9\-]+)?:resource-explorer-2:...`) fixes it.
pub const ENRICHMENT_VIEW_ARN_SERVICE: &str = "resource-explorer-2";

/// The resource portion of that managed view's ARN, up to the id the service assigns
/// the view it creates.
///
/// One managed view per account and region — the feature is a single per-account
/// switch — and the view is a RESOURCE the service creates in Resource Explorer when
/// the feature is first started, so its ARN carries a service-assigned id after the
/// view name, the way every Resource Explorer view ARN names the view and then the id
/// that tells two views of one name apart. The id is minted ONCE per region and kept
/// on the record ([`new_managed_view_id`]), so the ARN a `Start` answers is the one a
/// status read reports and the one a caller takes to `resource-explorer-2`. The view
/// NAME is this emulator's own choice: the model fixes only the ARN's shape (the
/// `AwsResourceExplorerManagedViewArn` pattern, which admits any `managed-view/<x>`),
/// so nothing here is a recorded value — the partition, region and account come from
/// the request, through `engine::emulator_arn`.
pub const ENRICHMENT_VIEW_RESOURCE_PREFIX: &str =
    "managed-view/AWSManagedViewForResourceTagsForTelemetry/";

/// The most regions ONE telemetry-feature store will hold a record for.
///
/// Emulator resource guard, NOT an AWS quota: this service publishes no quota on
/// enabling a feature. Every region-keyed feature store
/// ([`ComAmazonawsObservabilityadminObservabilityadminState::telemetry_evaluations`],
/// its organization twin and
/// [`ComAmazonawsObservabilityadminObservabilityadminState::telemetry_enrichments`])
/// is keyed by a region that can only come from [`request_region`], which admits
/// nothing outside `emulator_telemetry::policy::KNOWN_REGIONS` — so the catalog's own
/// size IS the bound, and this constant says so rather than repeating a number. It is
/// the independent clamp as well: a `Start` or `Stop` that would take a store past it
/// is refused BEFORE a single record is allocated, so the guard holds even if a future
/// caller writes a key that did not come from `request_region`.
///
/// DEFENCE IN DEPTH, and deliberately so: because `request_region` already admits
/// nothing outside the catalog, no request reaching the current handlers can take a
/// store past this bound, so the refusal each write path answers with is unreachable
/// from the wire today. It is the clamp that keeps it unreachable — a later key
/// source, or a larger catalog, meets a bound rather than an unbounded map.
pub const MAX_TELEMETRY_FEATURE_REGIONS: usize = ::emulator_telemetry::policy::KNOWN_REGIONS.len();

/// The most organization centralization rules this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: this service publishes no centralization
/// rule quota. A create past it is refused with the modelled
/// `ServiceQuotaExceededException` before a record is allocated. A centralization rule
/// is stored exactly once (it is never fanned out), so this one cap bounds the store.
pub const MAX_CENTRALIZATION_RULES: usize = 500;

/// How long after a create or an update an organization centralization rule's
/// provisioning comes due.
///
/// A CHOSEN span, not an AWS-documented duration: `ContextGraphStatus`' own
/// documentation says a status "Returns `Provisioning` while the context graph is
/// being set up, `Healthy` once it is active", and this is the span this emulator
/// takes to get there. It is deliberately non-zero, so a caller that reads straight
/// after the create still observes `Provisioning`.
pub const CENTRALIZATION_PROVISIONING_DELAY_MS: i64 = 400;

/// The health of a centralization rule whose provisioning has not come due, from
/// `RuleHealth`'s own value set (`Healthy`, `Unhealthy`, `Provisioning`).
pub const RULE_HEALTH_PROVISIONING: &str = "Provisioning";

/// The health of a centralization rule this emulator has finished provisioning.
pub const RULE_HEALTH_HEALTHY: &str = "Healthy";

/// The tag-propagation health of a rule whose destination configures tag propagation:
/// `TagPropagationStatus$Healthy`, which that member documents as "the most recent
/// tag-propagation attempt succeeded".
pub const TAG_PROPAGATION_STATUS_HEALTHY: &str = "Healthy";

/// A stored rule's key: the region it lives in, then its name. `RuleName` is unique
/// per region, and ordering by region then name is what gives `ListTelemetryRules` a
/// stable, name-ordered page sequence within one region.
pub type RuleKey = (String, String);

/// One stored telemetry rule: either the rule a caller created in its home region,
/// or one of the fan-out replicas that rule's multi-region scope asked for.
#[derive(Debug, Clone, Default)]
pub struct StoredTelemetryRule {
    pub rule_name: String,
    /// The region this record lives in — the home region for a caller-created rule,
    /// the spoke region for a replica.
    pub region: String,
    /// `arn:aws:observabilityadmin:<region>:<account>:telemetry-rule/<RuleName>`.
    pub rule_arn: String,
    /// The rule body exactly as the caller supplied it.
    ///
    /// Shared with this rule's fan-out replicas rather than copied into each of
    /// them: the body is caller-sized, and one request must not cost its size times
    /// the number of regions it names.
    pub rule: ::std::sync::Arc<TelemetryRule>,
    /// `CreatedTimeStamp`, in epoch milliseconds. For a replica this is the instant
    /// the fan-out comes due, which is also when the record starts existing.
    pub created_timestamp_ms: i64,
    /// `LastUpdateTimeStamp`, in epoch milliseconds.
    pub last_update_timestamp_ms: i64,
    pub home_region: String,
    /// Whether this record is a fan-out replica rather than the rule the caller
    /// created. Reported as `IsReplicated`, whose own documentation defines it as
    /// "whether this telemetry rule is a replica that was created in this region
    /// through multi-region fan-out from the home region".
    pub is_replica: bool,
    /// The spoke regions this rule's scope asks for. Empty on a replica and on a
    /// rule with no multi-region scope.
    pub replication_targets: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

impl StoredTelemetryRule {
    /// This record's store key.
    pub fn key(&self) -> RuleKey {
        (self.region.clone(), self.rule_name.clone())
    }

    /// Whether this record exists yet at `now_ms`. Always true for a caller-created
    /// rule; false for a replica whose fan-out has not come due.
    pub fn exists_at(&self, now_ms: i64) -> bool {
        self.created_timestamp_ms <= now_ms
    }

    /// This record as a `TelemetryRuleSummary`, the shape `ListTelemetryRules`
    /// reports one rule as.
    pub fn summary(&self) -> TelemetryRuleSummary {
        TelemetryRuleSummary {
            rule_name: Some(self.rule_name.clone()),
            rule_arn: Some(self.rule_arn.clone()),
            created_time_stamp: Some(self.created_timestamp_ms),
            last_update_time_stamp: Some(self.last_update_timestamp_ms),
            resource_type: self.rule.resource_type.clone(),
            telemetry_type: Some(self.rule.telemetry_type.clone()),
            telemetry_source_types: self.rule.telemetry_source_types.clone(),
        }
    }
}

/// A stored pipeline's key: the region it lives in, then its name. A pipeline name
/// identifies the pipeline within the account in one region, so the region is part
/// of the key and a read only ever resolves a pipeline in the region the caller
/// signed for.
pub type PipelineKey = (String, String);

/// Which call started the transition a pipeline record's due instant belongs to, and
/// so which status the pipeline reports until that instant arrives.
///
/// `TelemetryPipelineStatus` carries a transient value either side of `ACTIVE` —
/// `CREATING` and `UPDATING` — and they are told apart by what started the work, not
/// by the clock: one record therefore carries one due instant and this, rather than
/// two instants that could both be pending at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PipelineTransition {
    /// The create that minted the record started it.
    #[default]
    Creating,
    /// An accepted `UpdateTelemetryPipeline` started it.
    Updating,
}

impl PipelineTransition {
    /// The `Status` a pipeline reports while this transition has not come due.
    pub fn transient_status(self) -> &'static str {
        match self {
            Self::Creating => PIPELINE_STATUS_CREATING,
            Self::Updating => PIPELINE_STATUS_UPDATING,
        }
    }
}

/// One stored telemetry pipeline.
#[derive(Debug, Clone, Default)]
pub struct StoredTelemetryPipeline {
    /// The id this service assigned the pipeline, which its ARN carries.
    pub id: String,
    pub name: String,
    pub region: String,
    /// `arn:aws:observabilityadmin:<region>:<account>:telemetry-pipeline/<id>`.
    pub arn: String,
    /// The configuration body exactly as the caller supplied it: the YAML document
    /// `TelemetryPipelineConfiguration$Body` carries, stored verbatim so a read
    /// reports back what was written rather than a re-rendering of it.
    pub configuration_body: String,
    /// `CreatedTimeStamp`, in epoch milliseconds.
    pub created_timestamp_ms: i64,
    /// `LastUpdateTimeStamp`, in epoch milliseconds.
    pub last_update_timestamp_ms: i64,
    /// The instant the pipeline's transition to `ACTIVE` comes due. The status a read
    /// reports is derived from this and the clock, which is why no read has to write
    /// and no operation has to sleep.
    pub active_at_ms: i64,
    /// Which call started the transition `active_at_ms` belongs to, and so which
    /// transient status a read reports until it comes due.
    pub transition: PipelineTransition,
    pub tags: BTreeMap<String, String>,
}

impl StoredTelemetryPipeline {
    /// This record's store key.
    pub fn key(&self) -> PipelineKey {
        (self.region.clone(), self.name.clone())
    }

    /// This pipeline's `Status` at `now_ms`: the transient status of the transition in
    /// flight until that transition comes due, `ACTIVE` from then on.
    pub fn status_at(&self, now_ms: i64) -> &'static str {
        if now_ms < self.active_at_ms {
            self.transition.transient_status()
        } else {
            PIPELINE_STATUS_ACTIVE
        }
    }

    /// The `StatusReason` of the `Status` this pipeline reports at `now_ms`.
    ///
    /// `StatusReason` is "additional information about the pipeline status", so EVERY
    /// status has one: the member describes the state the pipeline is in, which it is
    /// in whether or not that state is transient, and a read that reported it only
    /// for some states would leave a caller unable to tell "no reason" from "not
    /// reported".
    pub fn status_reason_at(&self, now_ms: i64) -> TelemetryPipelineStatusReason {
        let description = match self.status_at(now_ms) {
            PIPELINE_STATUS_CREATING => PIPELINE_CREATING_STATUS_REASON,
            PIPELINE_STATUS_UPDATING => PIPELINE_UPDATING_STATUS_REASON,
            PIPELINE_STATUS_ACTIVE => PIPELINE_ACTIVE_STATUS_REASON,
            // Unreachable while a stored pipeline reports one of the two statuses
            // above: a status added here needs its own sentence rather than
            // inheriting another status's.
            _ => "The pipeline status has no further context to report.",
        };
        TelemetryPipelineStatusReason {
            description: Some(description.to_string()),
        }
    }

    /// The tags this pipeline carries, as the modelled `TagMapOutput` a read reports.
    ///
    /// Always a map, EMPTY when nothing tagged the pipeline: the member reports the
    /// resource's tag set, and an untagged resource has an empty one — a different
    /// observation from a member a response never carried.
    pub fn tag_map(&self) -> ::std::collections::HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The `ConfigurationSummary` this pipeline's stored configuration document
    /// declares: see [`configuration_summary`].
    pub fn configuration_summary(&self) -> Option<ConfigurationSummary> {
        configuration_summary(self)
    }

    /// This record as the pipeline a read reports, as it stands at `now_ms`: every
    /// member the model snapshot's `TelemetryPipeline` shape declares, plus the
    /// `ConfigurationSummary` the recorded responses carry beside them (see
    /// [`TelemetryPipelineProjection`]).
    pub fn as_pipeline(&self, now_ms: i64) -> TelemetryPipelineProjection {
        TelemetryPipelineProjection {
            pipeline: TelemetryPipeline {
                created_time_stamp: Some(self.created_timestamp_ms),
                last_update_time_stamp: Some(self.last_update_timestamp_ms),
                arn: Some(self.arn.clone()),
                name: Some(self.name.clone()),
                configuration: Some(TelemetryPipelineConfiguration {
                    body: self.configuration_body.clone(),
                }),
                status: Some(self.status_at(now_ms).to_string()),
                status_reason: Some(self.status_reason_at(now_ms)),
                tags: Some(self.tag_map()),
            },
            configuration_summary: self.configuration_summary(),
        }
    }

    /// This record as the summary a listing reports, as it stands at `now_ms`: every
    /// member the model's `TelemetryPipelineSummary` shape declares, each read from
    /// the same record a `GetTelemetryPipeline` of this pipeline reports, so a listing
    /// and a read of one pipeline never disagree.
    ///
    /// The shape declares no `Configuration`, so the body itself is not in a summary —
    /// `ConfigurationSummary` is the "summary of the pipeline configuration components"
    /// that stands in for it, and it is the one [`Self::configuration_summary`] derives
    /// from that body.
    pub fn summary(&self, now_ms: i64) -> TelemetryPipelineSummary {
        TelemetryPipelineSummary {
            created_time_stamp: Some(self.created_timestamp_ms),
            last_update_time_stamp: Some(self.last_update_timestamp_ms),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status_at(now_ms).to_string()),
            tags: Some(self.tag_map()),
            configuration_summary: self.configuration_summary(),
        }
    }
}

/// The `ConfigurationSummary` of the pipeline `record`: the components its stored
/// configuration document declares, so a caller can render the pipeline's shape without
/// re-parsing the document itself.
///
/// Every member is DERIVED from the document, through the one bounded reader this crate
/// has ([`parse_configuration`], which `TestTelemetryPipeline` and
/// `ValidateTelemetryPipelineConfiguration` read the same documents with):
///
/// * `Sources` is one entry per source plugin a section declares. `Source`'s own
///   documentation says the pipeline "currently supports a single source per pipeline,
///   but is structured as a list to accommodate multiple pipelines in the
///   configuration", so the list spans the sections the document declares.
/// * `DataSources` is one entry per data source those source plugins declare, each with
///   the name and the type it was declared with. A source that declares none still has
///   one, named the way `DataSource$Name`'s documentation says that source kind names
///   it: see [`undeclared_data_source`].
/// * `Processors` is the processor names in document order and `ProcessorCount` is how
///   many there are, so the two never disagree.
/// * `Sinks` is the sink plugin names those sections declare.
///
/// A body the reader cannot read declares no components, so it has no summary: a
/// document that was never parsed is a different observation from one declaring nothing.
fn configuration_summary(record: &StoredTelemetryPipeline) -> Option<ConfigurationSummary> {
    let root = parse_configuration(&record.configuration_body).ok()?;
    let mut sources = Vec::new();
    let mut data_sources = Vec::new();
    let mut processors = Vec::new();
    let mut sinks = Vec::new();

    for (_, section) in declared_sections(&root) {
        for (plugin, options) in plugins_declared(section.get(SOURCE_KEY)) {
            sources.push(Source {
                r#type: Some(plugin.to_string()),
            });
            data_sources.extend(data_sources_declared(plugin, options, &record.name));
        }
        processors.extend(
            processors_declared(section.get(PROCESSOR_KEY))
                .into_iter()
                .map(str::to_string),
        );
        for (plugin, _) in plugins_declared(section.get(SINK_KEY)) {
            sinks.push(plugin.to_string());
        }
    }

    Some(ConfigurationSummary {
        // The reader this walked bounds a document to `MAX_CONFIGURATION_NODES` nodes,
        // which is far inside `i32`, so the count is the list's length; the fallback
        // only keeps the cast total.
        processor_count: Some(i32::try_from(processors.len()).unwrap_or(i32::MAX)),
        sources: Some(sources),
        data_sources: Some(data_sources),
        processors: Some(processors),
        sinks: Some(sinks),
    })
}

/// The SOURCE TYPES a configuration document declares, each with the full dotted path
/// of the declaration in the document.
///
/// A pipeline's source type is the plugin its section's `source` property names — the
/// token `ConfigurationSummary$Sources` reports as a `Source$Type` — so this is the
/// same walk [`configuration_summary`] makes, through the one bounded reader this crate
/// has, and it spans every section the document declares. A body the reader cannot read
/// declares none: there is no document to read a source type out of.
pub fn declared_source_types(body: &str) -> Vec<(String, String)> {
    let Ok(root) = parse_configuration(body) else {
        return Vec::new();
    };
    let mut declared = Vec::new();
    for (section, node) in declared_sections(&root) {
        for (plugin, _) in plugins_declared(node.get(SOURCE_KEY)) {
            let path = if section.is_empty() {
                format!("{SOURCE_KEY}.{plugin}")
            } else {
                format!("{section}.{SOURCE_KEY}.{plugin}")
            };
            declared.push((path, plugin.to_string()));
        }
    }
    declared
}

/// The plugins one `source` or `sink` property declares, each with the options it
/// carries: the entries of the mapping a source is written as, and the entries of each
/// mapping of the sequence a sink is written as — the two forms this format writes a
/// plugin in.
fn plugins_declared(property: Option<&Node>) -> Vec<(&str, &Node)> {
    let Some(property) = property else {
        return Vec::new();
    };
    match property {
        Node::Mapping(entries) => entries
            .iter()
            .map(|(plugin, options)| (plugin.as_str(), options))
            .collect(),
        Node::Sequence(items) => items
            .iter()
            .flat_map(|item| plugins_declared(Some(item)))
            .collect(),
        Node::Null | Node::Scalar { .. } => Vec::new(),
    }
}

/// The processor names a section's `processor` sequence declares, in document order: the
/// name each position names, whether that position carries the processor's options (a
/// mapping) or only its name (a scalar).
fn processors_declared(property: Option<&Node>) -> Vec<&str> {
    let Some(Node::Sequence(items)) = property else {
        return Vec::new();
    };
    items
        .iter()
        .flat_map(|item| match item {
            Node::Mapping(entries) => entries
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<&str>>(),
            Node::Scalar { text, .. } => vec![text.as_str()],
            Node::Null | Node::Sequence(_) => Vec::new(),
        })
        .collect()
}

/// The data sources the source plugin `plugin` declares in `options`, for a pipeline
/// named `pipeline`.
///
/// A source declares them as the entries of its `data_sources` list — each entry either
/// the data source's name on its own or a mapping carrying that name and its type — or
/// as the `data_source_name`/`data_source_type` pair `DataSource`'s own documentation
/// names, on the plugin's own block. A source declaring neither still has one data
/// source, and what it is named and typed is [`data_source`]'s business.
fn data_sources_declared(plugin: &str, options: &Node, pipeline: &str) -> Vec<DataSource> {
    let declared = options
        .get(DATA_SOURCES_KEY)
        .and_then(Node::as_sequence)
        .unwrap_or_default();
    if declared.is_empty() {
        return vec![data_source(
            plugin,
            pipeline,
            declared_text(options, DATA_SOURCE_NAME_KEY),
            declared_text(options, DATA_SOURCE_TYPE_KEY),
        )];
    }
    declared
        .iter()
        .map(|entry| match entry.as_str() {
            // An entry that is just a name names the data source and declares no type.
            Some(name) => data_source(plugin, pipeline, Some(name), None),
            None => data_source(
                plugin,
                pipeline,
                declared_text(entry, NAME_KEY)
                    .or_else(|| declared_text(entry, DATA_SOURCE_NAME_KEY)),
                declared_text(entry, TYPE_KEY)
                    .or_else(|| declared_text(entry, DATA_SOURCE_TYPE_KEY)),
            ),
        })
        .collect()
}

/// The text `node` declares under `key`, for a mapping that declares one as a scalar.
fn declared_text<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.get(key).and_then(Node::as_str)
}

/// One data source of a summary: the name and the type the document declared for it, and
/// what each is when the document declared it not.
///
/// `DataSource`'s documentation splits BOTH of those by the KIND of source, which is why
/// the source plugin and the pipeline's name reach here. For a source reading log events
/// AWS vends ([`VENDED_LOG_SOURCE_PLUGINS`]), the data source "includes both a name and
/// type extracted from the log event metadata": those events are the ones vended for the
/// pipeline itself, so a name the document leaves out is the pipeline's own and a type it
/// leaves out is [`DEFAULT_DATA_SOURCE_TYPE`]. "For third-party sources (such as S3),
/// this includes only a name", which "defaults to the plugin name if not specified",
/// "with the type field left empty" — so such a source reports no type at all unless the
/// document declares one.
fn data_source(
    plugin: &str,
    pipeline: &str,
    name: Option<&str>,
    declared_type: Option<&str>,
) -> DataSource {
    let vended = VENDED_LOG_SOURCE_PLUGINS.contains(&plugin);
    DataSource {
        name: Some(name.unwrap_or(if vended { pipeline } else { plugin }).to_string()),
        r#type: declared_type
            .map(str::to_string)
            .or_else(|| vended.then(|| DEFAULT_DATA_SOURCE_TYPE.to_string())),
    }
}

/// A stored S3 Table integration's key: the region it lives in, then the id this
/// service minted for it. The region is part of the key for the same reason it is
/// part of every other key here — it comes from the caller's own credential scope,
/// and an integration is reachable only from the region it was created in.
pub type S3TableIntegrationKey = (String, String);

/// One stored S3 Table integration: the CloudWatch -> S3 Tables analytics
/// integration a `CreateS3TableIntegration` established.
#[derive(Debug, Clone, Default)]
pub struct StoredS3TableIntegration {
    /// The service-minted id the integration's ARN carries.
    pub id: String,
    pub region: String,
    /// `arn:aws:observabilityadmin:<region>:<account>:s3tableintegration/<id>`.
    pub arn: String,
    /// `RoleArn` exactly as the caller supplied it: the IAM role the integration
    /// assumes to reach the destination table bucket.
    pub role_arn: String,
    /// `Encryption$SseAlgorithm` as supplied — one of `SSEAlgorithm`'s own values.
    pub sse_algorithm: String,
    /// `Encryption$KmsKeyArn` as supplied, which only a KMS-encrypted integration
    /// carries.
    pub kms_key_arn: Option<String>,
    /// The table bucket this integration delivers into, named by the create
    /// ([`table_bucket_arn`]) because the caller names none. REPORTED-ONLY: no bucket
    /// is created behind it — see [`table_bucket_arn`] for why.
    pub destination_table_bucket_arn: String,
    /// `CreatedTimeStamp`, in epoch milliseconds.
    pub created_timestamp_ms: i64,
    pub tags: BTreeMap<String, String>,
}

impl StoredS3TableIntegration {
    /// This record's store key.
    pub fn key(&self) -> S3TableIntegrationKey {
        (self.region.clone(), self.id.clone())
    }

    /// This integration's `IntegrationStatus`.
    ///
    /// Always [`INTEGRATION_STATUS_ACTIVE`]: the modelled enum's only other value is
    /// `DELETING`, which belongs to an integration a `DeleteS3TableIntegration` has
    /// been called for, and a stored record is one no delete has retired.
    pub fn status(&self) -> &'static str {
        INTEGRATION_STATUS_ACTIVE
    }

    /// This record's encryption configuration, as the modelled `Encryption` a read
    /// reports.
    pub fn encryption(&self) -> Encryption {
        Encryption {
            sse_algorithm: self.sse_algorithm.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
        }
    }

    /// This record as an `IntegrationSummary` — the shape a listing reports one
    /// integration as: the ARN that is the only handle on it, and its status.
    pub fn summary(&self) -> IntegrationSummary {
        IntegrationSummary {
            arn: Some(self.arn.clone()),
            status: Some(self.status().to_string()),
        }
    }
}

/// Which way the last `Start`/`Stop` moved a telemetry-config feature, and so which
/// pair of the modelled `Status` values that region reports either side of the
/// instant that call comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationTransition {
    /// A `Start`: [`STATUS_STARTING`] until due, [`STATUS_RUNNING`] after.
    Onboarding,
    /// A `Stop`: [`STATUS_STOPPING`] until due, [`STATUS_STOPPED`] after.
    Offboarding,
}

impl EvaluationTransition {
    /// The `Status` this transition reports at `now_ms`, given the instant it comes
    /// due. The transient value before, the terminal one from then on — no write is
    /// needed to get from one to the other, so a status read never mutates.
    pub fn status_at(self, due_ms: i64, now_ms: i64) -> &'static str {
        match (self, now_ms < due_ms) {
            (Self::Onboarding, true) => STATUS_STARTING,
            (Self::Onboarding, false) => STATUS_RUNNING,
            (Self::Offboarding, true) => STATUS_STOPPING,
            (Self::Offboarding, false) => STATUS_STOPPED,
        }
    }
}

/// The telemetry-config (telemetry evaluation) feature of ONE region of the caller's
/// account, as the last `Start`/`Stop` left it.
///
/// A record exists only for a region some call named; a region with no record has
/// never been onboarded and reports [`STATUS_NOT_STARTED`]. The configuration's
/// multi-region shape is carried on every record of that configuration, so a read
/// signed for any of its regions reports the same `HomeRegion` and the same
/// `RegionStatuses` list and differs only in its own `Status`.
#[derive(Debug, Clone)]
pub struct TelemetryEvaluation {
    /// The transition the last `Start`/`Stop` for this region began.
    pub transition: EvaluationTransition,
    /// The epoch-millisecond instant that transition comes due, which is when this
    /// region starts reporting the terminal status of [`Self::transition`].
    pub due_ms: i64,
    /// The region the `Start` that configured this feature was signed for — "the
    /// single management point for all multi-region operations on this account", in
    /// `HomeRegion`'s own words.
    pub home_region: String,
    /// The spoke regions that `Start`'s multi-region scope named, empty when it
    /// named none. `HomeRegion` and `RegionStatuses` are each "only present when
    /// multi-region telemetry evaluation is active", which is exactly this list
    /// being non-empty.
    pub spoke_regions: Vec<String>,
}

impl TelemetryEvaluation {
    /// The `Status` this region reports at `now_ms`.
    pub fn status_at(&self, now_ms: i64) -> &'static str {
        self.transition.status_at(self.due_ms, now_ms)
    }

    /// Whether this feature was configured with a multi-region scope, which is what
    /// decides whether `HomeRegion` and `RegionStatuses` are reported at all.
    pub fn is_multi_region(&self) -> bool {
        !self.spoke_regions.is_empty()
    }
}

/// The resource tags for telemetry (telemetry enrichment) feature of one region of
/// the caller's account.
///
/// A record exists only once a `Start` or a `Stop` has named the region; before
/// that the feature "has never been started" and the status read answers the
/// modelled 404. The modelled `TelemetryEnrichmentStatus` carries no transient
/// value — only `Running`, `Stopped` and `Impaired` — so this feature has no
/// in-flight state and no due instant: a `Start` leaves it running and a `Stop`
/// leaves it stopped, both at once.
#[derive(Debug, Clone)]
pub struct TelemetryEnrichment {
    /// Whether the feature is enabled: [`ENRICHMENT_STATUS_RUNNING`] when it is,
    /// [`ENRICHMENT_STATUS_STOPPED`] when it is not.
    pub running: bool,
    /// The Resource Explorer managed view this feature reads, minted by the first
    /// `Start` in the region and kept across a `Stop`, so a feature stopped and
    /// started again reads the same view. `None` until a `Start` mints it, which is
    /// the state a `Stop` of a never-started feature leaves behind.
    pub managed_view_arn: Option<String>,
}

impl TelemetryEnrichment {
    /// The `TelemetryEnrichmentStatus` this feature reports.
    pub fn status(&self) -> &'static str {
        if self.running {
            ENRICHMENT_STATUS_RUNNING
        } else {
            ENRICHMENT_STATUS_STOPPED
        }
    }

    /// The managed view ARN to report, which
    /// `GetTelemetryEnrichmentStatusOutput$AwsResourceExplorerManagedViewArn`'s own
    /// documentation qualifies with "if the feature is enabled" — so a stopped
    /// feature reports none even though the view it would read again is still
    /// recorded.
    pub fn reported_view_arn(&self) -> Option<String> {
        self.running.then(|| self.managed_view_arn.clone()).flatten()
    }
}

/// One stored organization centralization rule.
///
/// The rule body is held behind an `Arc` for the same reason the telemetry rule's is:
/// it is caller-sized, and an update must not cost a second copy of it while a reader
/// still holds the old one.
#[derive(Debug, Clone, Default)]
pub struct StoredCentralizationRule {
    pub rule_name: String,
    /// `CreatedRegion`: "the Amazon Web Services region where the organization
    /// centralization rule was created", which is the region the create was signed
    /// for and the region field of this rule's ARN.
    pub created_region: String,
    /// `arn:aws:observabilityadmin:<region>:<account>:centralization-rule/<RuleName>`.
    pub rule_arn: String,
    /// The rule body exactly as the caller supplied it.
    pub rule: ::std::sync::Arc<CentralizationRule>,
    /// `CreatorAccountId`: "the Amazon Web Services Account that created the
    /// organization centralization rule".
    pub creator_account_id: String,
    /// `CreatedTimeStamp`, in epoch milliseconds.
    pub created_timestamp_ms: i64,
    /// `LastUpdateTimeStamp`, in epoch milliseconds.
    pub last_update_timestamp_ms: i64,
    /// The epoch-millisecond instant at which provisioning the CURRENT rule body
    /// comes due. Until it arrives every read reports `Provisioning`; after it, the
    /// rule is `Healthy`. A create and an update each set it, so no operation sleeps
    /// and no read has to write.
    pub provisioned_at_ms: i64,
    pub tags: BTreeMap<String, String>,
}

impl StoredCentralizationRule {
    /// This record's store key.
    pub fn key(&self) -> RuleKey {
        (self.created_region.clone(), self.rule_name.clone())
    }

    /// `RuleHealth` at `now_ms`: `Provisioning` until this rule's provisioning comes
    /// due, `Healthy` from then on.
    pub fn rule_health(&self, now_ms: i64) -> String {
        if now_ms < self.provisioned_at_ms {
            RULE_HEALTH_PROVISIONING.to_string()
        } else {
            RULE_HEALTH_HEALTHY.to_string()
        }
    }

    /// `ContextGraphStatus` at `now_ms`, or `None` when the member does not apply:
    /// it is "the status of context graph centralization for this rule", so it is
    /// reported only for a rule whose source asks for context graph centralization.
    /// It follows the same `Provisioning` -> `Healthy` span as [`Self::rule_health`],
    /// which is what that member documents: `Provisioning` "while the context graph
    /// is being set up", `Healthy` "once it is active".
    pub fn context_graph_status(&self, now_ms: i64) -> Option<String> {
        self.rule
            .source
            .source_context_graph_configuration
            .as_ref()
            .map(|_| self.rule_health(now_ms))
    }

    /// `TagPropagationStatus`, or `None` when the member does not apply: it is "the
    /// health status of tag propagation for this rule", so it is reported only for a
    /// rule whose destination configures tag propagation. Nothing in this emulator
    /// can fail such an attempt, so the status is the `Healthy` that member defines
    /// as "the most recent tag-propagation attempt succeeded".
    pub fn tag_propagation_status(&self) -> Option<String> {
        self.rule
            .destination
            .destination_logs_configuration
            .as_ref()
            .and_then(|logs| logs.tag_propagation_configuration.as_ref())
            .map(|_| TAG_PROPAGATION_STATUS_HEALTHY.to_string())
    }

    /// The destination account this rule centralizes into: the account its
    /// destination names, or — when it names none — the account that created it,
    /// since the data then stays in the creator's own account.
    pub fn destination_account_id(&self) -> String {
        self.rule
            .destination
            .account
            .clone()
            .unwrap_or_else(|| self.creator_account_id.clone())
    }

    /// This record as a `CentralizationRuleSummary`, the shape
    /// `ListCentralizationRulesForOrganization` reports one rule as.
    ///
    /// `FailureReason` and `TagPropagationFailureReason` are left unset: each is the
    /// reason an attempt failed, and nothing in this emulator fails one.
    pub fn summary(&self, now_ms: i64) -> CentralizationRuleSummary {
        CentralizationRuleSummary {
            rule_name: Some(self.rule_name.clone()),
            rule_arn: Some(self.rule_arn.clone()),
            creator_account_id: Some(self.creator_account_id.clone()),
            created_time_stamp: Some(self.created_timestamp_ms),
            created_region: Some(self.created_region.clone()),
            last_update_time_stamp: Some(self.last_update_timestamp_ms),
            rule_health: Some(self.rule_health(now_ms)),
            failure_reason: None,
            tag_propagation_status: self.tag_propagation_status(),
            tag_propagation_failure_reason: None,
            context_graph_status: self.context_graph_status(now_ms),
            destination_account_id: Some(self.destination_account_id()),
            destination_region: Some(self.rule.destination.region.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-observabilityadmin-observabilityadmin. A
/// handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsObservabilityadminObservabilityadminState {
    /// Every stored rule, keyed by `(region, RuleName)`.
    pub telemetry_rules: BTreeMap<RuleKey, StoredTelemetryRule>,
    /// Rule ARN -> store key, so a `RuleIdentifier` resolves as an ARN as well as
    /// as a bare name.
    pub arn_index: BTreeMap<String, RuleKey>,
    /// Every stored pipeline, keyed by `(region, Name)`. Ordering by region then
    /// name is what gives `ListTelemetryPipelines` a stable page sequence.
    pub telemetry_pipelines: BTreeMap<PipelineKey, StoredTelemetryPipeline>,
    /// Pipeline ARN -> store key, so a `PipelineIdentifier` resolves as an ARN as
    /// well as as a bare name — the two forms its documentation names.
    pub pipeline_arn_index: BTreeMap<String, PipelineKey>,
    /// Every stored S3 Table integration, keyed by `(region, id)`. Ordering by
    /// region then id is what gives `ListS3TableIntegrations` a stable page
    /// sequence.
    pub s3_table_integrations: BTreeMap<S3TableIntegrationKey, StoredS3TableIntegration>,
    /// Integration ARN -> store key. The ARN is the ONLY handle the model gives a
    /// caller on an integration (`GetS3TableIntegration$Arn`,
    /// `DeleteS3TableIntegration$Arn`), so this is how a request resolves one.
    pub s3_table_integration_arn_index: BTreeMap<String, S3TableIntegrationKey>,
    /// The account's telemetry-config feature, keyed by region: the region a
    /// `StartTelemetryEvaluation` was signed for, plus every spoke region its
    /// multi-region scope named.
    pub telemetry_evaluations: BTreeMap<String, TelemetryEvaluation>,
    /// The organization's telemetry-config feature, keyed by region exactly as
    /// [`Self::telemetry_evaluations`] is. A separate map because the two features
    /// are onboarded by separate operations and reported by separate reads: the
    /// account's own evaluation is not the organization's.
    pub organization_telemetry_evaluations: BTreeMap<String, TelemetryEvaluation>,
    /// The account's resource tags for telemetry feature, keyed by region.
    pub telemetry_enrichments: BTreeMap<String, TelemetryEnrichment>,
    /// Every organization centralization rule, keyed by `(CreatedRegion, RuleName)`.
    /// Ordering by region then name is what gives
    /// `ListCentralizationRulesForOrganization` a stable, name-ordered page sequence
    /// within one region and a stable region-then-name one across all of them.
    pub centralization_rules: BTreeMap<RuleKey, StoredCentralizationRule>,
    /// Centralization rule ARN -> store key, so a `RuleIdentifier` resolves as an
    /// ARN as well as as a bare name.
    pub centralization_arn_index: BTreeMap<String, RuleKey>,
}

impl ComAmazonawsObservabilityadminObservabilityadminState {
    /// The key of the rule `identifier` names for a caller signed for `region`,
    /// accepting both forms `RuleIdentifier`'s documentation allows: the bare rule
    /// name, or the full rule ARN.
    ///
    /// An ARN must match a stored ARN in EVERY field and name a rule in the
    /// caller's own region, so an ARN for another partition, region or account
    /// resolves to nothing rather than to the rule that shares its name.
    pub fn resolve_key(&self, region: &str, identifier: &str) -> Option<RuleKey> {
        if identifier.starts_with("arn:") {
            return self
                .arn_index
                .get(identifier)
                .filter(|(rule_region, _)| rule_region == region)
                .cloned();
        }
        let key = (region.to_string(), identifier.to_string());
        self.telemetry_rules.contains_key(&key).then_some(key)
    }

    /// The rule `identifier` names in `region`, as it exists at `now_ms`.
    pub fn rule(
        &self,
        region: &str,
        identifier: &str,
        now_ms: i64,
    ) -> Option<&StoredTelemetryRule> {
        let key = self.resolve_key(region, identifier)?;
        self.telemetry_rules
            .get(&key)
            .filter(|record| record.exists_at(now_ms))
    }

    /// Whether `name` is already in use in `region` — by a stored rule or by a
    /// replica whose fan-out has not come due, since both occupy the name.
    pub fn name_taken(&self, region: &str, name: &str) -> bool {
        self.telemetry_rules
            .contains_key(&(region.to_string(), name.to_string()))
    }

    /// How many rules callers have created, which is what [`MAX_TELEMETRY_RULES`]
    /// bounds; fan-out replicas are not counted.
    pub fn home_rule_count(&self) -> usize {
        self.telemetry_rules
            .values()
            .filter(|record| !record.is_replica)
            .count()
    }

    /// How many records the store holds in total — caller-created rules and fan-out
    /// replicas alike — which is what [`MAX_STORED_RULE_RECORDS`] bounds.
    pub fn record_count(&self) -> usize {
        self.telemetry_rules.len()
    }

    /// Whether storing `records` more would take the store past
    /// [`MAX_STORED_RULE_RECORDS`]. A write path asks this BEFORE it allocates, so
    /// the refusal costs nothing.
    pub fn would_exceed_store(&self, records: usize) -> bool {
        self.record_count().saturating_add(records) > MAX_STORED_RULE_RECORDS
    }

    /// Store a record and index it by ARN.
    ///
    /// The key is `(region, RuleName)`, so a caller must have established that the
    /// name is free in that region first (`name_taken`) — every write path here does
    /// — because inserting over an occupied key would replace that record.
    pub fn insert(&mut self, record: StoredTelemetryRule) {
        self.arn_index.insert(record.rule_arn.clone(), record.key());
        self.telemetry_rules.insert(record.key(), record);
    }

    /// Remove one record, with its ARN index entry.
    fn remove(&mut self, key: &RuleKey) -> Option<StoredTelemetryRule> {
        let record = self.telemetry_rules.remove(key)?;
        self.arn_index.remove(&record.rule_arn);
        Some(record)
    }

    /// The replica of the rule `home` owns in `region`, if the fan-out put one
    /// there. A record in that region that belongs to somebody else — a rule
    /// created directly in it, or a replica of another home region's rule of the
    /// same name — is NOT this rule's replica.
    fn replica_of<'a>(
        &'a self,
        home: &StoredTelemetryRule,
        region: &str,
    ) -> Option<&'a StoredTelemetryRule> {
        self.telemetry_rules
            .get(&(region.to_string(), home.rule_name.clone()))
            .filter(|candidate| candidate.is_replica && candidate.home_region == home.home_region)
    }

    /// The per-spoke-region replication status of `record`, or `None` when the
    /// member does not apply: `RegionStatuses`' own documentation says it "is only
    /// present for rules created with multi-region scope", and a replica was not —
    /// it IS the fan-out of one.
    pub fn region_statuses(
        &self,
        record: &StoredTelemetryRule,
        now_ms: i64,
    ) -> Option<Vec<RegionStatus>> {
        if record.is_replica || !has_multi_region_scope(&record.rule) {
            return None;
        }
        Some(
            record
                .replication_targets
                .iter()
                .map(|region| match self.replica_of(record, region) {
                    // The fan-out reached this region, and `RuleArn` is reported
                    // because that member "is populated when the rule has been
                    // successfully created in the spoke region (status is ACTIVE)".
                    Some(replica) if replica.exists_at(now_ms) => RegionStatus {
                        region: Some(region.clone()),
                        status: Some(REGION_STATUS_ACTIVE.to_string()),
                        failure_reason: None,
                        rule_arn: Some(replica.rule_arn.clone()),
                    },
                    Some(_) => RegionStatus {
                        region: Some(region.clone()),
                        status: Some(REGION_STATUS_PENDING.to_string()),
                        failure_reason: None,
                        rule_arn: None,
                    },
                    // Nothing of ours is there and nothing ever will be: the name
                    // was already in use in that region when the fan-out ran.
                    None => RegionStatus {
                        region: Some(region.clone()),
                        status: Some(REGION_STATUS_FAILED.to_string()),
                        failure_reason: Some(format!(
                            "a telemetry rule named '{}' already exists in {region}",
                            record.rule_name
                        )),
                        rule_arn: None,
                    },
                })
                .collect(),
        )
    }

    /// Fan `home`'s multi-region scope out: create a replica in every spoke region
    /// the scope names and whose name is free, leaving the ones already taken to be
    /// reported as `FAILED`. Each new replica starts existing `REPLICATION_DELAY_MS`
    /// after `now_ms`, so until then the spoke region reports `PENDING`.
    ///
    /// A replica is a small record: it SHARES its parent's rule body (an `Arc`,
    /// never a copy) and carries no tags of its own, since the tags belong to the
    /// resource the home rule's ARN names. One request therefore cannot cost its
    /// body size times the number of regions it named.
    fn replicate(&mut self, home: &StoredTelemetryRule, now_ms: i64) {
        let due = now_ms.saturating_add(REPLICATION_DELAY_MS);
        for region in home
            .replication_targets
            .iter()
            .take(MAX_REPLICATION_TARGETS)
        {
            if self.name_taken(region, &home.rule_name) {
                continue;
            }
            self.insert(StoredTelemetryRule {
                rule_name: home.rule_name.clone(),
                region: region.clone(),
                rule_arn: rule_arn(region, &home.rule_name),
                rule: ::std::sync::Arc::clone(&home.rule),
                created_timestamp_ms: due,
                last_update_timestamp_ms: due,
                home_region: home.home_region.clone(),
                is_replica: true,
                replication_targets: Vec::new(),
                tags: BTreeMap::new(),
            });
        }
    }

    /// Store a newly created rule and start the fan-out its scope asks for.
    pub fn create_rule(&mut self, record: StoredTelemetryRule, now_ms: i64) {
        self.replicate(&record, now_ms);
        self.insert(record);
    }

    /// Replace the body of the rule at `key`, bumping `LastUpdateTimeStamp` and
    /// leaving `CreatedTimeStamp` and the tags where they were, then bring its
    /// replicas in line with the new scope: a spoke region the new scope drops loses
    /// its replica, one it adds gains a `PENDING` one, and one it keeps gets the new
    /// body. Answers the rule's ARN.
    pub fn replace_rule(
        &mut self,
        key: &RuleKey,
        rule: TelemetryRule,
        now_ms: i64,
    ) -> Option<String> {
        let home = self.telemetry_rules.get_mut(key)?;
        // Only the rule in its home region owns a fan-out. Every caller refuses a
        // replica before reaching here; this is the invariant stated in one place, so
        // a spoke-region update can never start a fan-out of its own.
        if home.is_replica {
            return None;
        }
        let previous_targets = std::mem::take(&mut home.replication_targets);
        home.rule = ::std::sync::Arc::new(rule);
        home.last_update_timestamp_ms = now_ms;
        home.replication_targets = replication_targets(&home.rule, &home.region);
        let home = home.clone();

        for region in &previous_targets {
            if home.replication_targets.contains(region) {
                continue;
            }
            if self.replica_of(&home, region).is_some() {
                self.remove(&(region.clone(), home.rule_name.clone()));
            }
        }
        let kept: Vec<String> = home
            .replication_targets
            .iter()
            .filter(|region| previous_targets.contains(region))
            .cloned()
            .collect();
        for region in kept {
            let replica_key = (region, home.rule_name.clone());
            let is_ours = self
                .telemetry_rules
                .get(&replica_key)
                .is_some_and(|replica| {
                    replica.is_replica && replica.home_region == home.home_region
                });
            if is_ours {
                if let Some(replica) = self.telemetry_rules.get_mut(&replica_key) {
                    replica.rule = ::std::sync::Arc::clone(&home.rule);
                    // A replica that is still pending starts existing at its own due
                    // instant, so its last update can never predate that.
                    replica.last_update_timestamp_ms = now_ms.max(replica.created_timestamp_ms);
                }
            }
        }
        let added = StoredTelemetryRule {
            replication_targets: home
                .replication_targets
                .iter()
                .filter(|region| !previous_targets.contains(region))
                .cloned()
                .collect(),
            ..home.clone()
        };
        self.replicate(&added, now_ms);
        Some(home.rule_arn)
    }

    /// Remove the rule at `key` together with every replica it owns. Answers the
    /// removed rule, or `None` when nothing was stored there.
    pub fn remove_rule_tree(&mut self, key: &RuleKey) -> Option<StoredTelemetryRule> {
        let home = self.telemetry_rules.get(key)?.clone();
        let replicas: Vec<RuleKey> = home
            .replication_targets
            .iter()
            .filter(|region| self.replica_of(&home, region).is_some())
            .map(|region| (region.clone(), home.rule_name.clone()))
            .collect();
        for replica in &replicas {
            self.remove(replica);
        }
        self.remove(key)
    }

    /// The key of the pipeline `identifier` names for a caller signed for `region`,
    /// accepting both forms `PipelineIdentifier`'s documentation allows ("the
    /// identifier (name or ARN)").
    ///
    /// An ARN must match a stored ARN in EVERY field and name a pipeline in the
    /// caller's own region, so an ARN for another partition, region or account
    /// resolves to nothing rather than to the pipeline that shares its name.
    pub fn resolve_pipeline_key(&self, region: &str, identifier: &str) -> Option<PipelineKey> {
        if identifier.starts_with("arn:") {
            return self
                .pipeline_arn_index
                .get(identifier)
                .filter(|(pipeline_region, _)| pipeline_region == region)
                .cloned();
        }
        let key = (region.to_string(), identifier.to_string());
        self.telemetry_pipelines.contains_key(&key).then_some(key)
    }

    /// The pipeline `identifier` names in `region`.
    pub fn pipeline(&self, region: &str, identifier: &str) -> Option<&StoredTelemetryPipeline> {
        let key = self.resolve_pipeline_key(region, identifier)?;
        self.telemetry_pipelines.get(&key)
    }

    /// Whether `name` is already in use by a pipeline in `region`.
    pub fn pipeline_name_taken(&self, region: &str, name: &str) -> bool {
        self.telemetry_pipelines
            .contains_key(&(region.to_string(), name.to_string()))
    }

    /// How many pipelines `region` holds, which is what
    /// [`MAX_TELEMETRY_PIPELINES`] bounds.
    pub fn pipeline_count(&self, region: &str) -> usize {
        self.telemetry_pipelines
            .keys()
            .filter(|(pipeline_region, _)| pipeline_region == region)
            .count()
    }

    /// How many pipeline records the store holds in total, across every region,
    /// which is what [`MAX_STORED_PIPELINE_RECORDS`] bounds.
    pub fn pipeline_record_count(&self) -> usize {
        self.telemetry_pipelines.len()
    }

    /// Whether storing `records` more pipelines would take the store past
    /// [`MAX_STORED_PIPELINE_RECORDS`]. A write path asks this BEFORE it allocates,
    /// so the refusal costs nothing — and it is the cap that dominates
    /// [`MAX_TELEMETRY_PIPELINES`], which a caller could otherwise reset by signing
    /// for another region.
    pub fn would_exceed_pipeline_store(&self, records: usize) -> bool {
        self.pipeline_record_count().saturating_add(records) > MAX_STORED_PIPELINE_RECORDS
    }

    /// Store a pipeline record and index it by ARN.
    ///
    /// The key is `(region, Name)`, so a caller must have established that the name
    /// is free in that region first ([`Self::pipeline_name_taken`]) — every write
    /// path here does — because inserting over an occupied key would replace that
    /// record.
    pub fn insert_pipeline(&mut self, record: StoredTelemetryPipeline) {
        self.pipeline_arn_index
            .insert(record.arn.clone(), record.key());
        self.telemetry_pipelines.insert(record.key(), record);
    }

    /// Replace the configuration body of the pipeline at `key` with `body`, bump its
    /// `LastUpdateTimeStamp` and start the transition the accepted update owes, so a
    /// read inside the span reports `UPDATING` and one past it reports `ACTIVE`.
    ///
    /// The record's name, id, ARN, creation instant and tags are left where they are:
    /// an update changes the configuration, and the pipeline's name and ARN are
    /// immutable. A key no record answers is left alone — every caller resolves the
    /// record under this same lock first, so there is no such key to answer for.
    pub fn replace_pipeline_configuration(
        &mut self,
        key: &PipelineKey,
        body: String,
        now_ms: i64,
    ) {
        let Some(record) = self.telemetry_pipelines.get_mut(key) else {
            return;
        };
        record.configuration_body = body;
        record.last_update_timestamp_ms = now_ms;
        record.transition = PipelineTransition::Updating;
        record.active_at_ms = now_ms.saturating_add(PIPELINE_ACTIVATION_DELAY_MS);
    }

    /// Remove the pipeline `identifier` names in `region`, answering the record that
    /// was removed, or `None` when the identifier names none.
    ///
    /// Both the record and its ARN index entry go, so every handle a caller holds on
    /// the pipeline — its name and its ARN alike — stops resolving at the same instant
    /// the record stops existing, the way a retired telemetry rule
    /// ([`Self::remove_rule_tree`]) and a retired S3 Table integration
    /// ([`Self::remove_s3_table_integration`]) leave no index entry behind either.
    /// The freed name is then available to a later create, which is what makes a
    /// delete the way a caller reuses a name.
    pub fn remove_pipeline(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Option<StoredTelemetryPipeline> {
        let key = self.resolve_pipeline_key(region, identifier)?;
        let record = self.telemetry_pipelines.remove(&key)?;
        self.pipeline_arn_index.remove(&record.arn);
        Some(record)
    }

    /// The key of the S3 Table integration `arn` names for a caller signed for
    /// `region`.
    ///
    /// The ARN must match a stored ARN in EVERY field and name an integration in the
    /// caller's own region, so an ARN for another partition, region or account
    /// resolves to nothing rather than to the integration that shares its id.
    pub fn resolve_s3_table_integration_key(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<S3TableIntegrationKey> {
        self.s3_table_integration_arn_index
            .get(arn)
            .filter(|(integration_region, _)| integration_region == region)
            .cloned()
    }

    /// The S3 Table integration `arn` names in `region`.
    pub fn s3_table_integration(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&StoredS3TableIntegration> {
        let key = self.resolve_s3_table_integration_key(region, arn)?;
        self.s3_table_integrations.get(&key)
    }

    /// How many S3 Table integrations `region` holds.
    ///
    /// The create path reads this to enforce the singleton the integration is (see
    /// [`ONE_INTEGRATION_PER_ACCOUNT_MESSAGE`]), so through the wire it is 0 or 1;
    /// a store pre-filled directly may hold more.
    pub fn s3_table_integration_count(&self, region: &str) -> usize {
        self.s3_table_integrations
            .keys()
            .filter(|(integration_region, _)| integration_region == region)
            .count()
    }

    /// How many S3 Table integration records the store holds in total, across every
    /// region, which is what [`MAX_STORED_S3_TABLE_INTEGRATION_RECORDS`] bounds.
    pub fn s3_table_integration_record_count(&self) -> usize {
        self.s3_table_integrations.len()
    }

    /// Whether storing `records` more integrations would take the store past
    /// [`MAX_STORED_S3_TABLE_INTEGRATION_RECORDS`]. A write path asks this BEFORE it
    /// allocates, so the refusal costs nothing.
    pub fn would_exceed_s3_table_integration_store(&self, records: usize) -> bool {
        self.s3_table_integration_record_count()
            .saturating_add(records)
            > MAX_STORED_S3_TABLE_INTEGRATION_RECORDS
    }

    /// Store an S3 Table integration record and index it by ARN.
    ///
    /// The key is `(region, id)` and the id is freshly minted per create
    /// ([`new_s3_table_integration_id`]), so an insert never lands on an occupied
    /// key.
    pub fn insert_s3_table_integration(&mut self, record: StoredS3TableIntegration) {
        self.s3_table_integration_arn_index
            .insert(record.arn.clone(), record.key());
        self.s3_table_integrations.insert(record.key(), record);
    }

    /// Retire the S3 Table integration `arn` names in `region`, answering the record
    /// that was removed, or `None` when the ARN names none.
    ///
    /// Both the record and its ARN index entry go, so the handle a caller holds stops
    /// resolving at the same instant the record stops existing — the way a retired
    /// telemetry rule leaves no index entry behind either
    /// ([`Self::remove_rule_tree`]). A freed id is never minted again
    /// ([`new_s3_table_integration_id`]), so the key cannot come back under a
    /// different record.
    pub fn remove_s3_table_integration(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<StoredS3TableIntegration> {
        let key = self.resolve_s3_table_integration_key(region, arn)?;
        let record = self.s3_table_integrations.remove(&key)?;
        self.s3_table_integration_arn_index.remove(&record.arn);
        Some(record)
    }

    /// The telemetry-config feature records of `scope`.
    fn evaluations(&self, scope: EvaluationScope) -> &BTreeMap<String, TelemetryEvaluation> {
        match scope {
            EvaluationScope::Account => &self.telemetry_evaluations,
            EvaluationScope::Organization => &self.organization_telemetry_evaluations,
        }
    }

    /// The telemetry-config feature records of `scope`, to write.
    fn evaluations_mut(
        &mut self,
        scope: EvaluationScope,
    ) -> &mut BTreeMap<String, TelemetryEvaluation> {
        match scope {
            EvaluationScope::Account => &mut self.telemetry_evaluations,
            EvaluationScope::Organization => &mut self.organization_telemetry_evaluations,
        }
    }

    /// The `scope` telemetry-config feature of `region`, or `None` when no `Start`
    /// or `Stop` has ever named that region.
    pub fn telemetry_evaluation(
        &self,
        scope: EvaluationScope,
        region: &str,
    ) -> Option<&TelemetryEvaluation> {
        self.evaluations(scope).get(region)
    }

    /// Record the onboarding a `Start` signed for `home_region` began: that region
    /// and every spoke region its multi-region scope named are onboarded together,
    /// reaching [`STATUS_RUNNING`] [`EVALUATION_TRANSITION_DELAY_MS`] later.
    ///
    /// Repeating the same `Start` is the same onboarding: a region already onboarding
    /// under this very configuration keeps the instant the first call came due, so a
    /// caller cannot push a running feature back into `STARTING` by asking for what
    /// it already has.
    ///
    /// Answers `false`, having written NOTHING, when the regions this call would add
    /// would take the store past [`MAX_TELEMETRY_FEATURE_REGIONS`]: the count is
    /// checked before the first insert, and the spoke list is re-clamped to
    /// [`MAX_REPLICATION_TARGETS`] here as well as in [`multi_region_targets`], so one
    /// request can never store more than that many records however it reached this
    /// method.
    #[must_use]
    pub fn start_telemetry_evaluation(
        &mut self,
        scope: EvaluationScope,
        home_region: &str,
        spoke_regions: Vec<String>,
        now_ms: i64,
    ) -> bool {
        let due = now_ms.saturating_add(EVALUATION_TRANSITION_DELAY_MS);
        let spoke_regions: Vec<String> = spoke_regions
            .into_iter()
            .take(MAX_REPLICATION_TARGETS)
            .collect();
        let regions: Vec<String> = ::std::iter::once(home_region.to_string())
            .chain(spoke_regions.iter().cloned())
            .collect();
        let feature = self.evaluations_mut(scope);
        if would_exceed_feature_store(feature, &regions) {
            return false;
        }
        for region in regions {
            let due_ms = feature
                .get(&region)
                .filter(|current| {
                    current.transition == EvaluationTransition::Onboarding
                        && current.home_region == home_region
                        && current.spoke_regions == spoke_regions
                })
                .map_or(due, |current| current.due_ms);
            feature.insert(
                region,
                TelemetryEvaluation {
                    transition: EvaluationTransition::Onboarding,
                    due_ms,
                    home_region: home_region.to_string(),
                    spoke_regions: spoke_regions.clone(),
                },
            );
        }
        true
    }

    /// Record the offboarding a `Stop` signed for `region` began, reaching
    /// [`STATUS_STOPPED`] [`EVALUATION_TRANSITION_DELAY_MS`] later.
    ///
    /// A `Stop` signed for the home region of a multi-region configuration offboards
    /// its spoke regions with it, since that region is where the whole configuration
    /// is managed; a `Stop` signed for a spoke region offboards that region alone.
    /// Repeating the `Stop` is the same offboarding and keeps its due instant, and a
    /// `Stop` of a region no `Start` ever named still records the offboarding — the
    /// caller asked for the feature to be off, which is what the status read then
    /// reports.
    ///
    /// Answers `false`, having written NOTHING, when that would take the store past
    /// [`MAX_TELEMETRY_FEATURE_REGIONS`].
    #[must_use]
    pub fn stop_telemetry_evaluation(
        &mut self,
        scope: EvaluationScope,
        region: &str,
        now_ms: i64,
    ) -> bool {
        let due = now_ms.saturating_add(EVALUATION_TRANSITION_DELAY_MS);
        let feature = self.evaluations_mut(scope);
        let mut regions = vec![region.to_string()];
        if let Some(record) = feature.get(region) {
            if record.home_region == region {
                regions.extend(record.spoke_regions.iter().take(MAX_REPLICATION_TARGETS).cloned());
            }
        }
        if would_exceed_feature_store(feature, &regions) {
            return false;
        }
        for target in regions {
            let current = feature.get(&target);
            let offboarding = TelemetryEvaluation {
                transition: EvaluationTransition::Offboarding,
                due_ms: current
                    .filter(|record| record.transition == EvaluationTransition::Offboarding)
                    .map_or(due, |record| record.due_ms),
                home_region: current
                    .map_or_else(|| target.clone(), |record| record.home_region.clone()),
                spoke_regions: current
                    .map_or_else(Vec::new, |record| record.spoke_regions.clone()),
            };
            feature.insert(target, offboarding);
        }
        true
    }

    /// The per-spoke-region statuses of the configuration `record` belongs to, or
    /// `None` when the member does not apply: `RegionStatuses` "is only present when
    /// multi-region telemetry evaluation is active", and a configuration with no
    /// spoke region is not.
    pub fn evaluation_region_statuses(
        &self,
        scope: EvaluationScope,
        record: &TelemetryEvaluation,
        now_ms: i64,
    ) -> Option<Vec<RegionStatus>> {
        if !record.is_multi_region() {
            return None;
        }
        let feature = self.evaluations(scope);
        Some(
            record
                .spoke_regions
                .iter()
                .map(|region| RegionStatus {
                    region: Some(region.clone()),
                    status: Some(
                        feature
                            .get(region)
                            .map_or(STATUS_NOT_STARTED, |spoke| spoke.status_at(now_ms))
                            .to_string(),
                    ),
                    // Never populated: nothing here can fail to onboard a region, and
                    // the member is "only populated when `Status` indicates a failure".
                    failure_reason: None,
                    // "This field is only present for telemetry rule region statuses".
                    rule_arn: None,
                })
                .collect(),
        )
    }

    /// The resource tags for telemetry feature of `region`, or `None` when it has
    /// never been started or stopped there.
    pub fn telemetry_enrichment(&self, region: &str) -> Option<&TelemetryEnrichment> {
        self.telemetry_enrichments.get(region)
    }

    /// Enable the resource tags for telemetry feature in `region` and answer the
    /// record that leaves behind.
    ///
    /// The managed view is minted by the first `Start` in the region and kept from
    /// then on, so a feature started, stopped and started again reads the one view
    /// its first start created — the ARN it answers is the one every later read of
    /// that region reports.
    ///
    /// Answers `None`, having written NOTHING, when a record for a region this store
    /// does not hold yet would take it past [`MAX_TELEMETRY_FEATURE_REGIONS`].
    #[must_use]
    pub fn start_telemetry_enrichment(&mut self, region: &str) -> Option<TelemetryEnrichment> {
        let record = self.enrichment_entry(region, true)?;
        record.running = true;
        // Minted only where there is no view yet, so a restart draws no id it would
        // throw away.
        record
            .managed_view_arn
            .get_or_insert_with(|| enrichment_view_arn(region, &new_managed_view_id()));
        Some(record.clone())
    }

    /// Disable the resource tags for telemetry feature in `region` and answer the
    /// record that leaves behind.
    ///
    /// Idempotent: the feature has one off state, so stopping an already-stopped
    /// feature — or one that was never started — leaves it stopped and changes
    /// nothing else.
    ///
    /// Answers `None`, having written NOTHING, when a record for a region this store
    /// does not hold yet would take it past [`MAX_TELEMETRY_FEATURE_REGIONS`].
    #[must_use]
    pub fn stop_telemetry_enrichment(&mut self, region: &str) -> Option<TelemetryEnrichment> {
        let record = self.enrichment_entry(region, false)?;
        record.running = false;
        Some(record.clone())
    }

    /// The enrichment record of `region`, inserting one in the `running` state given
    /// when the store holds none yet — unless that insert would take the store past
    /// [`MAX_TELEMETRY_FEATURE_REGIONS`], which answers `None` and writes nothing.
    ///
    /// The one place either enrichment write path can grow the store, so the bound is
    /// stated once.
    fn enrichment_entry(
        &mut self,
        region: &str,
        running: bool,
    ) -> Option<&mut TelemetryEnrichment> {
        if !self.telemetry_enrichments.contains_key(region)
            && self.telemetry_enrichments.len() >= MAX_TELEMETRY_FEATURE_REGIONS
        {
            return None;
        }
        Some(
            self.telemetry_enrichments
                .entry(region.to_string())
                .or_insert_with(|| TelemetryEnrichment {
                    running,
                    managed_view_arn: None,
                }),
        )
    }

    /// The key of the centralization rule `identifier` names for a caller signed for
    /// `region`, accepting both forms `RuleIdentifier` allows: the bare rule name, or
    /// the full rule ARN.
    ///
    /// An ARN must match a stored ARN in EVERY field and name a rule created in the
    /// caller's own region, so an ARN for another partition, region or account
    /// resolves to nothing rather than to the rule that shares its name.
    pub fn resolve_centralization_key(&self, region: &str, identifier: &str) -> Option<RuleKey> {
        if identifier.starts_with("arn:") {
            return self
                .centralization_arn_index
                .get(identifier)
                .filter(|(rule_region, _)| rule_region == region)
                .cloned();
        }
        let key = (region.to_string(), identifier.to_string());
        self.centralization_rules.contains_key(&key).then_some(key)
    }

    /// The centralization rule `identifier` names in `region`.
    pub fn centralization_rule(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&StoredCentralizationRule> {
        let key = self.resolve_centralization_key(region, identifier)?;
        self.centralization_rules.get(&key)
    }

    /// Whether a centralization rule named `name` already exists in `region`.
    pub fn centralization_name_taken(&self, region: &str, name: &str) -> bool {
        self.centralization_rules
            .contains_key(&(region.to_string(), name.to_string()))
    }

    /// How many centralization rules the store holds, which is what
    /// [`MAX_CENTRALIZATION_RULES`] bounds.
    pub fn centralization_rule_count(&self) -> usize {
        self.centralization_rules.len()
    }

    /// Store a newly created centralization rule and index it by ARN.
    ///
    /// The key is `(CreatedRegion, RuleName)`, so a caller must have established that
    /// the name is free in that region first ([`Self::centralization_name_taken`]) —
    /// every write path here does — because inserting over an occupied key would
    /// replace that record.
    pub fn insert_centralization_rule(&mut self, record: StoredCentralizationRule) {
        self.centralization_arn_index
            .insert(record.rule_arn.clone(), record.key());
        self.centralization_rules.insert(record.key(), record);
    }

    /// Replace the body of the centralization rule at `key`, bumping
    /// `LastUpdateTimeStamp` and restarting its provisioning, and leaving
    /// `CreatedTimeStamp`, `CreatorAccountId`, `CreatedRegion` and the tags where they
    /// were. Answers the rule's ARN, or `None` when nothing is stored at `key`.
    pub fn replace_centralization_rule(
        &mut self,
        key: &RuleKey,
        rule: CentralizationRule,
        now_ms: i64,
    ) -> Option<String> {
        let record = self.centralization_rules.get_mut(key)?;
        record.rule = ::std::sync::Arc::new(rule);
        record.last_update_timestamp_ms = now_ms;
        record.provisioned_at_ms = now_ms.saturating_add(CENTRALIZATION_PROVISIONING_DELAY_MS);
        Some(record.rule_arn.clone())
    }

    /// Remove the centralization rule at `key`, with its ARN index entry. Answers the
    /// removed rule, or `None` when nothing was stored there.
    pub fn remove_centralization_rule(
        &mut self,
        key: &RuleKey,
    ) -> Option<StoredCentralizationRule> {
        let record = self.centralization_rules.remove(key)?;
        self.centralization_arn_index.remove(&record.rule_arn);
        Some(record)
    }
}

/// Whether storing a record for each of `regions` would take `feature` past
/// [`MAX_TELEMETRY_FEATURE_REGIONS`].
///
/// Only a region the store does not hold yet grows it, so re-recording a region
/// never trips the guard. Asked BEFORE the first insert, so a refusal costs nothing
/// and never leaves a half-written configuration behind.
fn would_exceed_feature_store(
    feature: &BTreeMap<String, TelemetryEvaluation>,
    regions: &[String],
) -> bool {
    let additions = regions
        .iter()
        .filter(|region| !feature.contains_key(*region))
        .count();
    feature.len().saturating_add(additions) > MAX_TELEMETRY_FEATURE_REGIONS
}

/// Which telemetry-config feature a `Start`, `Stop` or status read is about: the
/// caller's own account, or its organization.
///
/// The two are onboarded by separate operations (`StartTelemetryEvaluation` versus
/// `StartTelemetryEvaluationForOrganization`) and reported by separate reads, so they
/// are separate records; everything else about them — the `Status` values, the
/// transient span, the multi-region scope — is the same, which is why one set of
/// methods takes this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationScope {
    /// The caller's own account (`StartTelemetryEvaluation` and friends).
    Account,
    /// The caller's organization (`StartTelemetryEvaluationForOrganization`).
    Organization,
}

/// Whether `rule` asks to be replicated beyond its home region.
///
/// `Regions` and `AllRegions` are the two forms of multi-region scope the model
/// carries; a rule with neither lives only in its home region.
pub fn has_multi_region_scope(rule: &TelemetryRule) -> bool {
    rule.all_regions == Some(true)
        || rule
            .regions
            .as_ref()
            .is_some_and(|regions| !regions.is_empty())
}

/// The spoke regions `rule` asks to be replicated into from `home_region`.
///
/// `Regions` names them outright; `AllRegions` means "all Amazon Web Services
/// Regions where Amazon CloudWatch Observability Admin is available in the current
/// partition", which this emulator reads as every region of the home region's
/// partition. The home region itself is never a spoke of its own rule, and the
/// result is de-duplicated and ordered so two calls fan out identically.
///
/// The set is CLAMPED to [`MAX_REPLICATION_TARGETS`] and to the regions of the home
/// region's partition, so the number of replicas a request can ask for is decided
/// here, by the server, and not by the length of the list it sent:
/// [`replication_rejection`] refuses such a list outright, and this clamp holds even
/// if a caller of this function has not run that check.
pub fn replication_targets(rule: &TelemetryRule, home_region: &str) -> Vec<String> {
    multi_region_targets(
        rule.all_regions == Some(true),
        rule.regions.as_deref().unwrap_or_default(),
        home_region,
    )
}

/// The spoke regions a multi-region scope names, from `home_region`.
///
/// The scope is the `AllRegions` / `Regions` pair — the one every multi-region member
/// of this service carries, on a telemetry rule and on a `Start` of the
/// telemetry-config feature alike. `Regions` names the spoke regions outright;
/// `AllRegions` means "all Amazon Web Services Regions where Amazon CloudWatch
/// Observability Admin is available in the current partition", which this emulator
/// reads as every region of the home region's partition. The home region is never a
/// spoke of its own scope, and the result is de-duplicated and ordered so two calls
/// with the same scope reach the same regions.
///
/// The set is CLAMPED to [`MAX_REPLICATION_TARGETS`] and to the regions of the home
/// region's partition, so the number of regions a request can reach is decided here,
/// by the server, and not by the length of the list it sent:
/// [`multi_region_rejection`] refuses such a list outright, and this clamp holds even
/// if a caller of this function has not run that check.
pub fn multi_region_targets(all_regions: bool, named: &[String], home_region: &str) -> Vec<String> {
    let partition = partition_regions(home_region);
    let listed: BTreeSet<String> = if all_regions {
        partition
    } else {
        named
            .iter()
            .filter(|region| partition.contains(*region))
            .cloned()
            .collect()
    };
    listed
        .into_iter()
        .filter(|region| region != home_region)
        .take(MAX_REPLICATION_TARGETS)
        .collect()
}

/// Every AWS region of `region`'s own partition.
///
/// The catalog is `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the
/// wire may carry"), partitioned by [`partition_of`].
fn partition_regions(region: &str) -> BTreeSet<String> {
    let partition = partition_of(region);
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .iter()
        .filter(|candidate| partition_of(candidate) == partition)
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// The AWS partition a region belongs to, by its region-code prefix.
///
/// The prefixes and partition names are AWS's own (`aws`, `aws-cn`, `aws-us-gov` and
/// the isolated partitions); the longer `us-isob-` / `us-isof-` prefixes are matched
/// before `us-iso-` so each lands in its own partition.
fn partition_of(region: &str) -> &'static str {
    const PARTITION_PREFIXES: &[(&str, &str)] = &[
        ("cn-", "aws-cn"),
        ("us-gov-", "aws-us-gov"),
        ("us-isob-", "aws-iso-b"),
        ("us-isof-", "aws-iso-f"),
        ("us-iso-", "aws-iso"),
        ("eu-isoe-", "aws-iso-e"),
    ];
    PARTITION_PREFIXES
        .iter()
        .find(|(prefix, _)| region.starts_with(prefix))
        .map_or("aws", |(_, partition)| *partition)
}

/// Why `rule` is not a rule this service will store, or `None` when it is fine.
///
/// Two rules the model states but cannot express as a constraint, so the centralized
/// walk lets both through and this is where they live. Every operation that stores a
/// `TelemetryRule` applies this one function, each rendering the refusal as its own
/// modelled `ValidationException`:
///
/// 1. every member targeting a modelled enum carries one of that enum's values —
///    the generated `*_VALUES` catalogs are exactly what
///    `smithy_query(query="shape",shape="<Enum>")` reports;
/// 2. `Regions` and `AllRegions` are, in each member's own words, "mutually
///    exclusive", and a named region must be a region of the home region's own
///    partition, within [`MAX_REPLICATION_TARGETS`] of them — that text decides how
///    many records one request stores and lands in the region field of each
///    replica's ARN.
pub fn telemetry_rule_rejection(rule: &TelemetryRule, home_region: &str) -> Option<String> {
    enum_rejection(
        "Rule.TelemetryType",
        Some(rule.telemetry_type.as_str()),
        TELEMETRY_TYPE_VALUES,
    )
    .or_else(|| {
        enum_rejection(
            "Rule.ResourceType",
            rule.resource_type.as_deref(),
            RESOURCE_TYPE_VALUES,
        )
    })
    .or_else(|| {
        list_enum_rejection(
            "Rule.TelemetrySourceTypes",
            rule.telemetry_source_types.as_deref(),
            TELEMETRY_SOURCE_TYPE_VALUES,
        )
    })
    .or_else(|| destination_rejection(rule))
    .or_else(|| replication_rejection(rule, home_region))
}

/// The telemetry source type a `ResourceType` CORRELATES with, for every resource
/// type exactly one of the modelled source types names.
///
/// `TelemetrySourceTypes` is documented as "the specific telemetry source types to
/// configure for the resource, such as VPC_FLOW_LOGS or EKS_AUDIT_LOGS.
/// TelemetrySourceTypes must be correlated with the specific resource type", so the
/// correlation below is the one the two enums' own values state — each entry pairs a
/// `ResourceType` value with the `TelemetrySourceType` value that names the SAME
/// resource (`smithy_query(query="shape", shape="ResourceType,TelemetrySourceType")`,
/// mirrored in `types::RESOURCE_TYPE_VALUES` / `types::TELEMETRY_SOURCE_TYPE_VALUES`).
///
/// Only an unambiguous correlation is listed: an EKS cluster is named by five of the
/// source types (`EKS_AUDIT_LOGS`, `EKS_AUTHENTICATOR_LOGS`,
/// `EKS_CONTROLLER_MANAGER_LOGS`, `EKS_SCHEDULER_LOGS`, `EKS_API_LOGS`), so no single
/// one of them is what a rule for that resource asked for, and a resource type no
/// source type names correlates with none.
const CORRELATED_TELEMETRY_SOURCE_TYPES: &[(&str, &str)] = &[
    ("AWS::EC2::VPC", "VPC_FLOW_LOGS"),
    (
        "AWS::Route53Resolver::ResolverEndpoint",
        "ROUTE53_RESOLVER_QUERY_LOGS",
    ),
];

/// The `TelemetryType` every modelled telemetry source type belongs to: all of
/// `TelemetrySourceType`'s values name a LOG source, so a rule configuring any other
/// kind of telemetry has no source type correlated with its resource.
const TELEMETRY_TYPE_LOGS: &str = "Logs";

/// The telemetry source types `resource_type` determines for a `telemetry_type` rule,
/// or `None` when it determines none.
pub fn correlated_telemetry_source_types(
    resource_type: Option<&str>,
    telemetry_type: &str,
) -> Option<Vec<String>> {
    if telemetry_type != TELEMETRY_TYPE_LOGS {
        return None;
    }
    let resource_type = resource_type?;
    CORRELATED_TELEMETRY_SOURCE_TYPES
        .iter()
        .find(|(resource, _)| *resource == resource_type)
        .map(|(_, source_type)| vec![(*source_type).to_string()])
}

/// `rule` as the service RECORDS it: the body the caller supplied, with every member
/// the caller left to the service filled in.
///
/// `TelemetrySourceTypes` is the one such member. It is "the specific telemetry source
/// types to configure for the resource" and it "must be correlated with the specific
/// resource type", so a rule whose resource type correlates with exactly one source
/// type has asked for that source type whether or not it spelled the member out — and
/// a resource is configured for it either way, so the record keeps it and every read
/// of the rule reports it. That the SERVICE settles the member, rather than leaving a
/// rule with none, is what the recorded `GetTelemetryRule` and `ListTelemetryRules`
/// responses for a rule created without it show: both report the source type its
/// resource type correlates with. A rule that named its own source types keeps those:
/// the request is what the resource is configured for.
///
/// Every operation that stores a `TelemetryRule` records it through this one function,
/// so a read, a listing and the rule a later update replaces all agree.
pub fn recorded_rule(rule: TelemetryRule) -> TelemetryRule {
    let mut rule = rule;
    if rule.telemetry_source_types.is_none() {
        rule.telemetry_source_types = correlated_telemetry_source_types(
            rule.resource_type.as_deref(),
            rule.telemetry_type.as_str(),
        );
    }
    rule
}

/// Why a member that targets a modelled enum is unacceptable. A member the request
/// omits is one the model admits, so it is not refused here.
pub fn enum_rejection(member: &str, value: Option<&str>, catalog: &[&str]) -> Option<String> {
    let supplied = value?;
    if catalog.contains(&supplied) {
        return None;
    }
    Some(format!(
        "{member} must be one of [{}], got '{supplied}'",
        catalog.join(", ")
    ))
}

/// [`enum_rejection`] for every entry of a list whose member targets an enum.
fn list_enum_rejection(member: &str, values: Option<&[String]>, catalog: &[&str]) -> Option<String> {
    values?.iter().enumerate().find_map(|(index, value)| {
        enum_rejection(&format!("{member}[{index}]"), Some(value.as_str()), catalog)
    })
}

/// The enum members carried under `Rule.DestinationConfiguration`, down to the WAF
/// logging filter's own conditions.
fn destination_rejection(rule: &TelemetryRule) -> Option<String> {
    let destination = rule.destination_configuration.as_ref()?;
    enum_rejection(
        "Rule.DestinationConfiguration.DestinationType",
        destination.destination_type.as_deref(),
        DESTINATION_TYPE_VALUES,
    )
    .or_else(|| {
        enum_rejection(
            "Rule.DestinationConfiguration.ELBLoadBalancerLoggingParameters.OutputFormat",
            destination
                .elb_load_balancer_logging_parameters
                .as_ref()
                .and_then(|parameters| parameters.output_format.as_deref()),
            OUTPUT_FORMAT_VALUES,
        )
    })
    .or_else(|| {
        list_enum_rejection(
            "Rule.DestinationConfiguration.LogDeliveryParameters.LogTypes",
            destination
                .log_delivery_parameters
                .as_ref()
                .and_then(|parameters| parameters.log_types.as_deref()),
            LOG_TYPE_VALUES,
        )
    })
    .or_else(|| {
        enum_rejection(
            "Rule.DestinationConfiguration.MskMonitoringParameters.EnhancedMonitoring",
            destination
                .msk_monitoring_parameters
                .as_ref()
                .and_then(|parameters| parameters.enhanced_monitoring.as_deref()),
            MSK_ENHANCED_MONITORING_LEVEL_VALUES,
        )
    })
    .or_else(|| {
        let waf = destination.waf_logging_parameters.as_ref()?;
        enum_rejection(
            "Rule.DestinationConfiguration.WAFLoggingParameters.LogType",
            waf.log_type.as_deref(),
            W_A_F_LOG_TYPE_VALUES,
        )
        .or_else(|| {
            let filter = waf.logging_filter.as_ref()?;
            enum_rejection(
                "Rule.DestinationConfiguration.WAFLoggingParameters.LoggingFilter.DefaultBehavior",
                filter.default_behavior.as_deref(),
                FILTER_BEHAVIOR_VALUES,
            )
            .or_else(|| {
                filter
                    .filters
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .find_map(|(index, entry)| {
                        let at = format!(
                            "Rule.DestinationConfiguration.WAFLoggingParameters.LoggingFilter.Filters[{index}]"
                        );
                        enum_rejection(
                            &format!("{at}.Behavior"),
                            entry.behavior.as_deref(),
                            FILTER_BEHAVIOR_VALUES,
                        )
                        .or_else(|| {
                            enum_rejection(
                                &format!("{at}.Requirement"),
                                entry.requirement.as_deref(),
                                FILTER_REQUIREMENT_VALUES,
                            )
                        })
                        .or_else(|| {
                            entry
                                .conditions
                                .as_deref()
                                .unwrap_or_default()
                                .iter()
                                .enumerate()
                                .find_map(|(nth, condition)| {
                                    enum_rejection(
                                        &format!("{at}.Conditions[{nth}].ActionCondition.Action"),
                                        condition
                                            .action_condition
                                            .as_ref()
                                            .and_then(|action| action.action.as_deref()),
                                        ACTION_VALUES,
                                    )
                                })
                        })
                    })
            })
        })
    })
}

/// Why `rule`'s multi-region scope is unacceptable, or `None` when it is fine.
///
/// The two forms of the scope are mutually exclusive, a named list may be no longer
/// than [`MAX_REPLICATION_TARGETS`], and each region it names must be a region of
/// the home region's own partition. The partition catalog is
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may
/// carry"), split by [`partition_of`]: a region outside it is refused rather than
/// stored, because the number of regions the list names is the number of records
/// the request stores, and because a replica's ARN carries that region in a field
/// the modelled `ResourceArn` pattern admits only `[a-z0-9-]+` in — and this
/// emulator mints ARNs in the `aws` partition only.
fn replication_rejection(rule: &TelemetryRule, home_region: &str) -> Option<String> {
    multi_region_rejection(
        "Rule.",
        rule.all_regions == Some(true),
        rule.regions.as_deref().unwrap_or_default(),
        home_region,
    )
}

/// The `ValidationException` message for a request that supplies BOTH forms of a
/// multi-region scope.
///
/// Fixed text that names the two parameters in their lowerCamel wire form, which is
/// how a client reads them off its own payload. Every member pair this service
/// refuses here is named `Regions` / `AllRegions` — on `TelemetryRule` and on the
/// `StartTelemetryEvaluation` family alike — so the one sentence covers them all.
pub const MUTUALLY_EXCLUSIVE_SCOPE_MESSAGE: &str =
    "Parameters 'regions' and 'allRegions' are mutually exclusive. Provide one or the \
     other, not both.";

/// Why a multi-region scope is unacceptable, or `None` when it is fine.
///
/// The scope is the `AllRegions` / `Regions` pair, whose members this service names
/// under different prefixes — `Rule.Regions` on a telemetry rule, plain `Regions` on
/// a `Start` of the telemetry-config feature — which is what `member_prefix` carries.
/// The two forms are, in each member's own words, "mutually exclusive"; a named list
/// may be no longer than [`MAX_REPLICATION_TARGETS`]; and each region it names must
/// be a region of the home region's own partition. The partition catalog is
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may
/// carry"), split by [`partition_of`]: a region outside it is refused rather than
/// stored, because the number of regions the list names is the number of records the
/// request stores, and because a stored region lands in an ARN field the modelled
/// `ResourceArn` pattern admits only `[a-z0-9-]+` in — and this emulator mints ARNs
/// in the `aws` partition only.
pub fn multi_region_rejection(
    member_prefix: &str,
    all_regions: bool,
    named: &[String],
    home_region: &str,
) -> Option<String> {
    if all_regions && !named.is_empty() {
        return Some(MUTUALLY_EXCLUSIVE_SCOPE_MESSAGE.to_string());
    }
    if named.len() > MAX_REPLICATION_TARGETS {
        return Some(format!(
            "{member_prefix}Regions names {} regions, more than the {MAX_REPLICATION_TARGETS} \
             this service propagates to",
            named.len()
        ));
    }
    let partition = partition_regions(home_region);
    named
        .iter()
        .enumerate()
        .find(|(_, region)| !partition.contains(*region))
        .map(|(index, region)| {
            format!(
                "{member_prefix}Regions[{index}] must be an Amazon Web Services Region of the \
                 {} partition, got '{region}'",
                partition_of(home_region)
            )
        })
}

/// The ARN of the Resource Explorer managed view `view_id` that the resource tags for
/// telemetry feature of `region` reads.
pub fn enrichment_view_arn(region: &str, view_id: &str) -> String {
    ::engine::emulator_arn(
        ENRICHMENT_VIEW_ARN_SERVICE,
        region,
        &format!("{ENRICHMENT_VIEW_RESOURCE_PREFIX}{view_id}"),
    )
}

/// A newly minted id for the managed view a first `StartTelemetryEnrichment` in a
/// region creates.
///
/// The view is a Resource Explorer resource, and a Resource Explorer view's ARN carries
/// a UUID after the view's name — so this id keeps the hyphenated UUID form the same
/// already-vendored generator produces, rather than the unseparated form this service's
/// OWN handles take ([`new_resource_id`]).
pub fn new_managed_view_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The ARN of the telemetry rule `name` in `region`.
pub fn rule_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RULE_ARN_RESOURCE_PREFIX}{name}"),
    )
}

/// The ARN of the telemetry rule `identifier` names for a caller signed for `region`,
/// whichever of the two forms `RuleIdentifier`'s documentation allows it took: an ARN
/// is already one, and a bare name is resolved into the ARN of that name in the
/// caller's own region.
///
/// This is the ARN a refusal reports for an identifier the store does not hold, so it
/// is derived from the REQUEST and the emulator's partition, region and account —
/// there is no record to read it off.
pub fn identified_rule_arn(region: &str, identifier: &str) -> String {
    if identifier.starts_with("arn:") {
        return identifier.to_string();
    }
    rule_arn(region, identifier)
}

/// The `ResourceNotFoundException` message a MUTATING telemetry-rule operation reports
/// for an identifier no record answers: the resource it would have changed, named by
/// its ARN. Its only variable part is that ARN ([`identified_rule_arn`]); the rest is
/// fixed text, and it is deliberately a different sentence from the read path's
/// [`RULE_NOT_FOUND_MESSAGE`].
pub fn resource_does_not_exist_message(arn: &str) -> String {
    format!("Resource {arn} does not exist.")
}

/// The ARN of the telemetry pipeline `id` in `region`.
pub fn pipeline_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PIPELINE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the S3 Table integration `id` in `region`.
pub fn s3_table_integration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{S3_TABLE_INTEGRATION_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The resource IDENTIFIER an ARN carries, which is what a refusal naming the
/// resource by its own id reports.
///
/// The ARN's six fields are read by the tree's ONE parse
/// (`service_common::arn::resource`, which keeps the resource field's own colons);
/// the identifier is the TRAILING segment of that field, since AWS writes a resource
/// as a bare identifier or as a resource type and an identifier separated by `/` or
/// `:`. The modelled `ResourceArn` pattern
/// (`^arn:aws([a-z0-9\-]+)?:([a-zA-Z0-9\-]+):([a-z0-9\-]+)?:([0-9]{12})?:(.+)$`)
/// fixes the five leading fields and a non-empty resource field, so an `Arn` the
/// constraint walk admitted always parses; a value that does not, or whose trailing
/// segment is empty, answers the text it was given, which names no resource here
/// anyway.
pub fn arn_resource_id(arn: &str) -> &str {
    let resource = ::service_common::arn::resource(arn).unwrap_or(arn);
    resource
        .rsplit(['/', ':'])
        .next()
        .filter(|identifier| !identifier.is_empty())
        .unwrap_or(resource)
}

/// A newly minted id for a resource this service names itself.
///
/// The id is OPAQUE — the model gives it no form of its own — and it is a token of
/// [`MINTED_ID_DIGITS`] hexadecimal digits: the entropy is a random v4 UUID's, reached
/// through `emulator_telemetry`'s `BatchId::random()` constructor
/// (`uuid::Uuid::new_v4().as_hyphenated()` verbatim,
/// `vendor/emulator-telemetry/src/types.rs`), because `emulator-telemetry` is already
/// one of this crate's dependencies, so nothing new enters the workspace lockfile.
/// Only the generator is borrowed; the newtype's telemetry meaning is dropped here,
/// and so are the hyphens that make a UUID a UUID rather than an id of this service's
/// own.
///
/// Used wherever this service assigns a resource its own handle:
/// [`new_s3_table_integration_id`] and the telemetry pipeline's id.
pub fn new_resource_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().replace('-', "")
}

/// How many hexadecimal digits an id [`new_resource_id`] mints carries, which is the
/// entropy of the v4 UUID it draws it from.
pub const MINTED_ID_DIGITS: usize = 32;

/// Whether `id` has the form [`new_resource_id`] mints: [`MINTED_ID_DIGITS`] lowercase
/// hexadecimal digits.
///
/// It lives beside the generator so that the operations reading an id back — the page
/// token `ListS3TableIntegrations` hands out carries one — cannot drift from the form
/// this service mints.
pub fn is_minted_resource_id(id: &str) -> bool {
    id.len() == MINTED_ID_DIGITS
        && id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The ARN of the S3 Tables table bucket the integration `id` in `region` delivers
/// into.
///
/// The caller names no bucket, so this service names the integration's own and reports
/// it as `DestinationTableBucketArn`; the name carries the integration id, so it is
/// unique per integration and a later read of the same integration reports the same
/// bucket.
///
/// REPORTED-ONLY: no bucket is created in the `s3tables` emulator behind this ARN, so
/// a caller cannot follow it into that service and find one. The bucket is that
/// service's resource, its `CreateTableBucket` is a scaffold stub with its own issue,
/// and this service owns no part of its state — so this ARN names where the
/// integration's data WOULD land, which is all `GetS3TableIntegration` reports about
/// it. Nothing here claims the bucket exists.
pub fn table_bucket_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        TABLE_BUCKET_ARN_SERVICE,
        region,
        &format!("{TABLE_BUCKET_ARN_RESOURCE_PREFIX}{TABLE_BUCKET_NAME_PREFIX}{id}"),
    )
}

/// A newly minted S3 Table integration id.
///
/// `CreateS3TableIntegration` carries no caller-supplied identifier, so the service
/// mints one per create and the integration's ARN is what the caller holds it by:
/// the id is [`new_resource_id`]'s, the one form this service's own handles take.
pub fn new_s3_table_integration_id() -> String {
    new_resource_id()
}

/// Why `encryption` and `role_arn` are not an S3 Table integration this service will
/// store, or `None` when they are fine.
///
/// The modelled constraints — both members' presence, `ResourceArn`'s 1..1011 length
/// and its `^arn:aws…` pattern — are refused by the centralized walk before a handler
/// runs. What is left is what the model STATES but cannot express as a constraint:
///
/// 1. `SseAlgorithm` carries one of `SSEAlgorithm`'s own values — the
///    `S_S_E_ALGORITHM_VALUES` catalog is exactly what
///    `smithy_query(query="shape",shape="SSEAlgorithm")` reports;
/// 2. `KmsKeyArn` belongs to the KMS algorithm alone. It is "the ARN of the KMS key
///    used for encryption when using customer-managed keys", and `AES256` is S3
///    managed encryption, which takes no key — so a request that names `AES256` AND a
///    key cannot be served as asked. A request that names `aws:kms` and NO key is
///    accepted: the member is modelled optional and its documentation ties it to
///    customer-managed keys, so omitting it is "encrypt with KMS under the Amazon Web
///    Services managed key", not an incomplete request.
///
/// `RoleArn` and `KmsKeyArn` are not read beyond that. Both target `ResourceArn`,
/// whose modelled `@pattern` the centralized walk already enforces, and this operation
/// RESOLVES neither — it stores both verbatim and reports them back — so there is
/// nothing here that could tell a role this emulator would assume from one it would
/// not. A service that does resolve a caller-supplied role before writing reads it
/// through `service_common::iam_role` instead of judging the text.
///
/// Every operation that stores or updates an integration applies this one function,
/// rendering the refusal as its own modelled `ValidationException`.
pub fn s3_table_integration_rejection(encryption: &Encryption, role_arn: &str) -> Option<String> {
    let _ = role_arn;
    enum_rejection(
        "Encryption.SseAlgorithm",
        Some(encryption.sse_algorithm.as_str()),
        S_S_E_ALGORITHM_VALUES,
    )
    .or_else(|| {
        encryption.kms_key_arn.as_deref()?;
        (encryption.sse_algorithm != SSE_ALGORITHM_KMS).then(|| {
            format!(
                "Encryption.KmsKeyArn is only valid with Encryption.SseAlgorithm \
                 '{SSE_ALGORITHM_KMS}', got '{}'",
                encryption.sse_algorithm
            )
        })
    })
}

/// The ARN of the organization centralization rule `name` created in `region`.
pub fn centralization_rule_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CENTRALIZATION_RULE_ARN_RESOURCE_PREFIX}{name}"),
    )
}

/// Why `rule` is not an organization centralization rule this service will store, or
/// `None` when it is fine.
///
/// The rules the model states but cannot express as a constraint, so the centralized
/// walk lets them through and this is where they live. Every operation that stores a
/// `CentralizationRule` applies this one function, each rendering the refusal as its
/// own modelled `ValidationException`:
///
/// 1. every member targeting a modelled enum carries one of that enum's values — the
///    generated `*_VALUES` catalogs are exactly what
///    `smithy_query(query="shape",shape="<Enum>")` reports;
/// 2. every member targeting `Region` names an Amazon Web Services Region of the
///    caller's own partition — the source regions the rule centralizes FROM, the
///    destination it centralizes INTO and each backup region — because a region is
///    the field a later read reports as `DestinationRegion` and the field an ARN
///    carries, and this emulator mints ARNs in the `aws` partition only.
pub fn centralization_rule_rejection(rule: &CentralizationRule, home_region: &str) -> Option<String> {
    let partition = partition_regions(home_region);
    rule.source
        .regions
        .iter()
        .enumerate()
        .find_map(|(index, region)| {
            region_rejection(
                &format!("Rule.Source.Regions[{index}]"),
                Some(region.as_str()),
                &partition,
                home_region,
            )
        })
        .or_else(|| {
            region_rejection(
                "Rule.Destination.Region",
                Some(rule.destination.region.as_str()),
                &partition,
                home_region,
            )
        })
        .or_else(|| source_logs_rejection(rule))
        .or_else(|| destination_logs_rejection(rule, &partition, home_region))
        .or_else(|| destination_metrics_rejection(rule, &partition, home_region))
}

/// Why a member that targets `Region` is unacceptable: it has to name a region of the
/// caller's own partition. A member the request omits is one the model admits, so it
/// is not refused here.
fn region_rejection(
    member: &str,
    value: Option<&str>,
    partition: &BTreeSet<String>,
    home_region: &str,
) -> Option<String> {
    let supplied = value?;
    if partition.contains(supplied) {
        return None;
    }
    Some(format!(
        "{member} must be an Amazon Web Services Region of the {} partition, got '{supplied}'",
        partition_of(home_region)
    ))
}

/// The enum members carried under `Rule.Source`.
fn source_logs_rejection(rule: &CentralizationRule) -> Option<String> {
    let logs = rule.source.source_logs_configuration.as_ref()?;
    enum_rejection(
        "Rule.Source.SourceLogsConfiguration.EncryptedLogGroupStrategy",
        Some(logs.encrypted_log_group_strategy.as_str()),
        ENCRYPTED_LOG_GROUP_STRATEGY_VALUES,
    )
}

/// The enum and `Region` members carried under
/// `Rule.Destination.DestinationLogsConfiguration`.
fn destination_logs_rejection(
    rule: &CentralizationRule,
    partition: &BTreeSet<String>,
    home_region: &str,
) -> Option<String> {
    let logs = rule
        .destination
        .destination_logs_configuration
        .as_ref()?;
    enum_rejection(
        "Rule.Destination.DestinationLogsConfiguration.LogGroupDeletionProtection",
        logs.log_group_deletion_protection.as_deref(),
        DELETION_PROTECTION_VALUES,
    )
    .or_else(|| {
        let encryption = logs.logs_encryption_configuration.as_ref()?;
        enum_rejection(
            "Rule.Destination.DestinationLogsConfiguration.LogsEncryptionConfiguration.\
             EncryptionStrategy",
            Some(encryption.encryption_strategy.as_str()),
            ENCRYPTION_STRATEGY_VALUES,
        )
        .or_else(|| {
            enum_rejection(
                "Rule.Destination.DestinationLogsConfiguration.LogsEncryptionConfiguration.\
                 EncryptionConflictResolutionStrategy",
                encryption.encryption_conflict_resolution_strategy.as_deref(),
                ENCRYPTION_CONFLICT_RESOLUTION_STRATEGY_VALUES,
            )
        })
        .or_else(|| {
            enum_rejection(
                "Rule.Destination.DestinationLogsConfiguration.LogsEncryptionConfiguration.\
                 EncryptionScope",
                encryption.encryption_scope.as_deref(),
                ENCRYPTION_SCOPE_VALUES,
            )
        })
    })
    .or_else(|| {
        enum_rejection(
            "Rule.Destination.DestinationLogsConfiguration.TagPropagationConfiguration.\
             TagConflictResolutionStrategy",
            logs.tag_propagation_configuration
                .as_ref()
                .and_then(|tags| tags.tag_conflict_resolution_strategy.as_deref()),
            TAG_CONFLICT_RESOLUTION_STRATEGY_VALUES,
        )
    })
    .or_else(|| {
        region_rejection(
            "Rule.Destination.DestinationLogsConfiguration.BackupConfiguration.Region",
            logs.backup_configuration
                .as_ref()
                .map(|backup| backup.region.as_str()),
            partition,
            home_region,
        )
    })
}

/// The `Region` member carried under
/// `Rule.Destination.DestinationMetricsConfiguration`.
fn destination_metrics_rejection(
    rule: &CentralizationRule,
    partition: &BTreeSet<String>,
    home_region: &str,
) -> Option<String> {
    let metrics = rule
        .destination
        .destination_metrics_configuration
        .as_ref()?;
    region_rejection(
        "Rule.Destination.DestinationMetricsConfiguration.BackupConfiguration.Region",
        metrics
            .backup_configuration
            .as_ref()
            .map(|backup| backup.region.as_str()),
        partition,
        home_region,
    )
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none the service recognises.
///
/// The scope is caller-controlled text, and it is BOTH a store key and a field of
/// every ARN this service mints, so only a region of
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may
/// carry", the same catalog [`partition_regions`] projects) is taken and anything
/// else falls back to [`DEFAULT_REGION`]. That is what bounds the key space of every
/// region-keyed store in this module — a caller cannot invent a region to grow one
/// with — and it also keeps a generated ARN from ever carrying text the modelled
/// `ResourceArn` pattern does not admit.
///
/// Every store this module keys by region shares that fold, the rule stores
/// included: a request whose scope names a region AWS does not have is served in
/// [`DEFAULT_REGION`], so its rules and its features live where a request carrying no
/// scope at all would put them. The fold is the same for reads and writes, so such a
/// request still sees exactly what it wrote; what it does NOT get is a private
/// namespace per invented region code.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            ::emulator_telemetry::policy::KNOWN_REGIONS
                .binary_search(&region.as_str())
                .is_ok()
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, in epoch milliseconds — the unit the modelled `CreatedTimeStamp` /
/// `LastUpdateTimeStamp` longs carry.
pub fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsObservabilityadminObservabilityadminState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsObservabilityadminObservabilityadminState>)
}
