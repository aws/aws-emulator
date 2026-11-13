// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-configservice-starlingdoveservice service.
//!
//! This holds the AWS Config setup-and-rule loop: the customer managed configuration
//! recorder, its status record, the delivery channel, the Config rules, and the
//! retention configuration that governs how long history is kept. Every
//! handler under `src/ops/` computes its response from these maps; nothing is
//! hardcoded and no response echoes the request that produced it.
//!
//! One engine's store stands for ONE account in ONE Region, so the quotas Config
//! documents per account per Region are enforced over the whole store.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccountAggregationSource, AggregationAuthorization, AggregatorFilters, Compliance,
    ComplianceContributorCount, ComplianceSummary, ConfigExportDeliveryInfo, ConfigRule,
    ConfigRuleEvaluationStatus, ConfigSnapshotDeliveryProperties, ConfigStreamDeliveryInfo,
    ConfigurationAggregator, ConfigurationRecorder, ConfigurationRecorderStatus,
    ConfigurationRecorderSummary, Connector, ConnectorConfiguration, ConnectorSummary,
    DeliveryChannel, DeliveryChannelStatus, EvaluationModeConfiguration, ExclusionByResourceTypes,
    ExecutionControls, OrganizationAggregationSource, PendingAggregationRequest, RecordingGroup,
    RecordingMode, RecordingStrategy, RemediationConfiguration, RemediationParameterValue,
    ResourceDetails, ResourceEvaluation, ResourceIdentifier, RetentionConfiguration, Scope, Source,
    StoredQuery, Tag,
};

// `ScopeConfiguration` is named here rather than in the import list above: that list
// is shared by every operation's state and is edited by whichever slice lands next, so
// staying out of it keeps this change from colliding with theirs.
use crate::ops::types::ScopeConfiguration;

// `BaseConfigurationItem` is named separately for the same reason as
// `ScopeConfiguration` above: it is what the aggregate read paths report a discovered
// resource's current configuration as.
use crate::ops::types::BaseConfigurationItem;

// The conformance pack shapes, named separately for the same reason: the pack surface
// landed on its own and stays out of the shared import list above.
use crate::ops::types::{
    ConformancePackDetail, ConformancePackInputParameter, ConformancePackStatusDetail,
};

// `ConformancePackEvaluationResult` is named separately for the same reason as the
// shapes above: it is how ONE evaluation result recorded for a rule a conformance pack
// deployed is REPORTED in that pack's compliance details.
use crate::ops::types::ConformancePackEvaluationResult;

// `AggregateResourceIdentifier` and `ConfigurationItem` are named separately for the
// same reason: the first is how an aggregate read path NAMES one resource, the second
// is the FULL configuration item the single-resource aggregate read reports.
use crate::ops::types::{AggregateResourceIdentifier, ConfigurationItem};

// `StoredQueryMetadata` is named separately for the same reason: it is what a listing
// of stored queries reports each one as.
use crate::ops::types::StoredQueryMetadata;

// `RemediationException` is named separately for the same reason: it is how one
// resource Config does not remediate under one rule is REPORTED.
use crate::ops::types::RemediationException;

// `AggregatedSourceStatus` is named separately for the same reason: it is the sync
// status an aggregator read reports one of its aggregation sources as.
use crate::ops::types::AggregatedSourceStatus;

// The remediation-execution shapes, named separately for the same reason: they are how
// a remediation execution and its one step are REPORTED, and `ResourceKey` is what
// names the resource an execution ran for.
use crate::ops::types::{RemediationExecutionStatus, RemediationExecutionStep, ResourceKey};

/// The ARN namespace of this service (`awsService.arnNamespace` in the model), used
/// for every ARN this emulator mints.
pub const ARN_SERVICE: &str = "config";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region generated emulators default to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The name Config assigns when a configuration recorder or delivery channel is
/// created without one (`ConfigurationRecorder$name` and `DeliveryChannel$name`:
/// "Config automatically assigns the name of \"default\" ... if you do not specify a
/// name at creation time").
pub const DEFAULT_NAME: &str = "default";

/// The name prefix reserved for service-linked configuration recorders
/// (`InvalidConfigurationRecorderNameException`: "The prefix
/// \"AWSConfigurationRecorderFor\" is reserved for service-linked configuration
/// recorders").
pub const SERVICE_LINKED_RECORDER_NAME_PREFIX: &str = "AWSConfigurationRecorderFor";

/// `RecorderStatus` values (`com.amazonaws.configservice#RecorderStatus`).
pub const RECORDER_STATUS_PENDING: &str = "Pending";
/// See [`RECORDER_STATUS_PENDING`].
pub const RECORDER_STATUS_SUCCESS: &str = "Success";

/// `ConfigRuleState` value a freshly created rule carries
/// (`com.amazonaws.configservice#ConfigRuleState`).
pub const CONFIG_RULE_STATE_ACTIVE: &str = "ACTIVE";

/// The default `EvaluationModes` entry for a new rule (`ConfigRule$EvaluationModes`:
/// "By default, the value is Detective evaluation mode only").
pub const EVALUATION_MODE_DETECTIVE: &str = "DETECTIVE";

/// The other `EvaluationMode` value (`com.amazonaws.configservice#EvaluationMode`),
/// and the ONLY one the resource-evaluation path accepts
/// (`StartResourceEvaluationRequest$EvaluationMode`: "The only valid value for this
/// API is PROACTIVE"; `ResourceEvaluationFilters$EvaluationMode`: "Currently,
/// DECTECTIVE is not supported as a valid value").
pub const EVALUATION_MODE_PROACTIVE: &str = "PROACTIVE";

/// `RecordingStrategyType` value the default recording group uses
/// (`com.amazonaws.configservice#RecordingStrategyType`).
pub const RECORDING_STRATEGY_ALL_SUPPORTED: &str = "ALL_SUPPORTED_RESOURCE_TYPES";
/// See [`RECORDING_STRATEGY_ALL_SUPPORTED`].
pub const RECORDING_STRATEGY_EXCLUSION: &str = "EXCLUSION_BY_RESOURCE_TYPES";
/// See [`RECORDING_STRATEGY_ALL_SUPPORTED`].
pub const RECORDING_STRATEGY_INCLUSION: &str = "INCLUSION_BY_RESOURCE_TYPES";

/// The `RecordingFrequency` a configuration recorder records at unless the request
/// asks for another one (`com.amazonaws.configservice#RecordingFrequency`;
/// `RecordingMode`: "Config supports Continuous recording and Daily recording", and
/// continuous recording is the recorder's default).
pub const RECORDING_FREQUENCY_CONTINUOUS: &str = "CONTINUOUS";

/// The other `RecordingFrequency` a recorder can record a resource type at
/// (`com.amazonaws.configservice#RecordingFrequency`, whose values are `CONTINUOUS`
/// and `DAILY`). A configuration item reports its `recordingFrequency` only under
/// this one (`BaseConfigurationItem$recordingFrequency`: "This field only appears in
/// the API response when DAILY recording is enabled for a resource type. If this
/// field is not present, CONTINUOUS recording is enabled for that resource type").
pub const RECORDING_FREQUENCY_DAILY: &str = "DAILY";

/// `ConfigurationItemStatus` values
/// (`com.amazonaws.configservice#ConfigurationItemStatus`), as that member's own
/// documentation states them: "the resource configuration has been updated" (`OK`), "the
/// resource was newly discovered" (`ResourceDiscovered`), and "the resource was deleted"
/// (`ResourceDeleted`).
pub const CONFIGURATION_ITEM_STATUS_OK: &str = "OK";
/// See [`CONFIGURATION_ITEM_STATUS_OK`].
pub const CONFIGURATION_ITEM_STATUS_RESOURCE_DISCOVERED: &str = "ResourceDiscovered";
/// See [`CONFIGURATION_ITEM_STATUS_OK`].
pub const CONFIGURATION_ITEM_STATUS_RESOURCE_DELETED: &str = "ResourceDeleted";

/// The only published `AggregatorFilterType` value
/// (`com.amazonaws.configservice#AggregatorFilterType`): an aggregator filter lists
/// the values it takes IN, so a filter that lists resource types aggregates those
/// types and no others.
pub const AGGREGATOR_FILTER_TYPE_INCLUDE: &str = "INCLUDE";

/// `AggregatedSourceType` values (`com.amazonaws.configservice#AggregatedSourceType`):
/// an aggregation source is either individual source accounts (`ACCOUNT`) or an
/// organization (`ORGANIZATION`), which is the pair of forms an aggregator aggregates
/// (see [`ConfigurationAggregatorEntry`]).
pub const AGGREGATED_SOURCE_TYPE_ACCOUNT: &str = "ACCOUNT";
/// See [`AGGREGATED_SOURCE_TYPE_ACCOUNT`].
pub const AGGREGATED_SOURCE_TYPE_ORGANIZATION: &str = "ORGANIZATION";

/// The `AggregatedSourceStatusType` of a source whose Config data reached the
/// aggregator (`com.amazonaws.configservice#AggregatedSourceStatusType`: "Valid value
/// SUCCEEDED indicates the data was successfully moved").
///
/// It is the status of every source relationship this store records, because the only
/// aggregation this store performs is the one it serves itself: the other two values
/// report a sync that failed (`FAILED`, "errors while moving data") or has fallen
/// behind (`OUTDATED`, "the data is not the most recent"), and no call served here
/// records either, so no source carries one.
pub const AGGREGATED_SOURCE_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The `RecordingScope` of a CUSTOMER MANAGED configuration recorder
/// (`com.amazonaws.configservice#RecordingScope`: `INTERNAL` when the recorded
/// `ConfigurationItems` are "recorded for free", `PAID` when recording them "impacts
/// the costs to your bill"). A recorder this account creates and pays for records in
/// the `PAID` scope; `INTERNAL` belongs to the service-linked recorders
/// `PutServiceLinkedConfigurationRecorder` creates.
pub const RECORDING_SCOPE_PAID: &str = "PAID";
/// See [`RECORDING_SCOPE_PAID`].
pub const RECORDING_SCOPE_INTERNAL: &str = "INTERNAL";

/// Every published `RecordingScope` value
/// (`com.amazonaws.configservice#RecordingScope`, whose values are `INTERNAL` and
/// `PAID`). A `recordingScope` filter value outside this list names no scope a
/// recorder can record in.
pub const RECORDING_SCOPES: [&str; 2] = [RECORDING_SCOPE_INTERNAL, RECORDING_SCOPE_PAID];

/// The only `ConfigurationRecorderFilterName` the model declares
/// (`com.amazonaws.configservice#ConfigurationRecorderFilterName`, whose single value
/// is `recordingScope`; `ConfigurationRecorderFilter$filterName`: "Currently, only
/// recordingScope is supported").
pub const CONFIGURATION_RECORDER_FILTER_RECORDING_SCOPE: &str = "recordingScope";

/// How many recorder summaries one `ListConfigurationRecorders` page carries when the
/// request asks for no fewer: the maximum of that operation's own `MaxResults` member
/// (range 0..20). A request that carries no `MaxResults`, or carries the member's own
/// default of 0, is served a page of this size — a page of zero would never report a
/// recorder the store holds.
pub const LIST_CONFIGURATION_RECORDERS_PAGE_SIZE: usize = 20;

/// The Amazon Web Services service principals that can own a THIRD-PARTY
/// service-linked configuration recorder — the ones
/// `PutThirdPartyServiceLinkedConfigurationRecorder` records a third-party cloud
/// service provider's resources for, through a connector.
///
/// That operation "can only be called by the Amazon Web Services service linked to the
/// configuration recorder", and Security Hub CSPM is the service documented as
/// recording a third-party cloud service provider's resources through a Config
/// connector, so a WELL-FORMED service principal naming any other service names no
/// third-party service-linked recorder and is refused
/// ([`unsupported_third_party_service_principal_message`]). Extend this table as Config
/// links further services.
///
/// This set bounds only the THIRD-PARTY operation; the services whose recorder
/// `PutServiceLinkedConfigurationRecorder` creates are
/// [`SERVICE_LINKED_PRINCIPALS`]. Both operations derive the recorder's name the
/// same way, so one service has ONE service-linked recorder however it was created.
pub const THIRD_PARTY_SERVICE_LINKED_PRINCIPALS: [&str; 1] = ["securityhub.amazonaws.com"];

/// Whether the service `service_principal` names can own a third-party service-linked
/// configuration recorder ([`THIRD_PARTY_SERVICE_LINKED_PRINCIPALS`]).
pub fn supports_third_party_service_linked_recording(service_principal: &str) -> bool {
    THIRD_PARTY_SERVICE_LINKED_PRINCIPALS.contains(&service_principal)
}

/// The Amazon Web Services service principals `PutServiceLinkedConfigurationRecorder`
/// creates a configuration recorder for — the services Config links a recorder of THIS
/// account's own Amazon Web Services resources to.
///
/// A service principal is NOT an arbitrary string here: the operation "creates a
/// service-linked configuration recorder that is linked to a specific Amazon Web
/// Services service based on the ServicePrincipal you specify", and points at the
/// published catalogue for which services those are ("For more information and a list
/// of supported services/service principals, see Working with the Configuration
/// Recorder in the Config Developer Guide" —
/// `smithy_query(query="doc", shape="PutServiceLinkedConfigurationRecorder")`). The
/// model cannot state that list as an enum, so it is the table below: the Observability
/// Admin service principals Config links a recorder for the account's own telemetry
/// resources to. A WELL-FORMED principal outside it names no service Config links a
/// recorder to, and is refused rather than stored
/// ([`UNRECOGNIZED_RECORDER_SERVICE_PRINCIPAL_MESSAGE`]; `ValidationException`: "You
/// have provided a service principal for service-linked configuration recorder that is
/// not valid").
/// Extend this table as Config links further services.
///
/// The services whose recorder records a THIRD-PARTY cloud service provider's resources
/// through a connector are a separate set, created by
/// `PutThirdPartyServiceLinkedConfigurationRecorder`
/// ([`THIRD_PARTY_SERVICE_LINKED_PRINCIPALS`]): that recorder cannot exist without the
/// connector this operation takes none of, so a connector-only service is no service
/// principal of THIS operation either.
pub const SERVICE_LINKED_PRINCIPALS: [&str; 2] = [
    "observabilityadmin.amazonaws.com",
    "telemetry-enablement.observabilityadmin.amazonaws.com",
];

/// Whether the service `service_principal` names can own the service-linked
/// configuration recorder `PutServiceLinkedConfigurationRecorder` creates
/// ([`SERVICE_LINKED_PRINCIPALS`]).
pub fn supports_service_linked_recording(service_principal: &str) -> bool {
    SERVICE_LINKED_PRINCIPALS.contains(&service_principal)
}

/// How Config words `ValidationException` for a `ServicePrincipal` whose service-linked
/// configuration recorder it cannot resolve — whether the principal names no service
/// Config links a recorder to, or the third-party recorder it names has no connector to
/// record through.
///
/// The model states the condition (`ValidationException`: "You have provided a service
/// principal for service-linked configuration recorder that is not valid") and Config
/// answers it with this FIXED sentence — the service-principal variant of the wording it
/// uses for a configuration recorder it cannot find
/// ([`no_such_configuration_recorder_message`]) — so the text is request-independent: it
/// quotes nothing the caller sent, and it names the DIMENSION the request selected the
/// recorder by. Every operation that resolves a recorder from a `ServicePrincipal`
/// refuses an unresolvable one this way; `NoSuchConfigurationRecorderException` is left
/// for a recorder Config COULD resolve the principal to and this account does not hold.
pub const UNRECOGNIZED_RECORDER_SERVICE_PRINCIPAL_MESSAGE: &str =
    "The service principal of the configuration recorder you entered cannot be found. \
     Verify the configuration recorder service principal, and try again.";

/// The `scopeType` values a `ScopeConfiguration` may carry
/// (`ScopeConfiguration$scopeType`: "Valid values include `tenant` and
/// `subscription`").
pub const SCOPE_TYPES: [&str; 2] = ["tenant", "subscription"];

/// Why `scope` is not a valid `ScopeConfiguration`, or `None` when it is.
///
/// The ONE constraint checked here is the one the shape documents: the scope type is
/// one of the published [`SCOPE_TYPES`] (`ScopeConfiguration$scopeType`). The message
/// is BUILT FROM THE REQUEST — it names the member and the constraint it failed.
///
/// `includedRegions` is NOT checked against `allRegions`: the shape documents it as
/// "the list of regions from the third-party cloud service provider to include when
/// recording resources. Used when allRegions is set to false", which says the list is
/// consulted only when every region is not recorded — not that carrying both is
/// refused. A recorder stores the list as sent and records every region while
/// `allRegions` is true, rather than this emulator inventing a refusal the service
/// may not make.
pub fn scope_configuration_violation(scope: &ScopeConfiguration) -> Option<String> {
    if !SCOPE_TYPES.contains(&scope.scope_type.as_str()) {
        return Some(format!(
            "Value '{}' at 'scopeConfiguration.scopeType' failed to satisfy constraint: \
             Member must be one of [{}]",
            scope.scope_type,
            SCOPE_TYPES.join(", "),
        ));
    }
    None
}

/// The `RuleEvaluationVisibility` of a rule whose `Evaluation`s the caller can read
/// back (`ConfigRule$RuleEvaluationVisibility`: "You can get Evaluations for the
/// Amazon Web Services Config rule if this value is EXTERNAL"). Every rule
/// `PutConfigRule` creates belongs to this account, so its evaluations are
/// retrievable; `INTERNAL` is for rules whose evaluations Config keeps to itself.
pub const RULE_EVALUATION_VISIBILITY_EXTERNAL: &str = "EXTERNAL";

/// `ComplianceType` values (`com.amazonaws.configservice#ComplianceType`).
pub const COMPLIANCE_TYPE_COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANCE_TYPE_COMPLIANT`].
pub const COMPLIANCE_TYPE_NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANCE_TYPE_COMPLIANT`].
pub const COMPLIANCE_TYPE_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";
/// See [`COMPLIANCE_TYPE_COMPLIANT`].
pub const COMPLIANCE_TYPE_NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// Every `ComplianceType` an evaluation-results filter may name: the published
/// `ComplianceType` values (`com.amazonaws.configservice#ComplianceType`) MINUS
/// `INSUFFICIENT_DATA`, which the filter members exclude
/// (`GetComplianceDetailsByConfigRuleRequest$ComplianceTypes`: "INSUFFICIENT_DATA is a
/// valid ComplianceType that is returned when an Config rule cannot be evaluated.
/// However, INSUFFICIENT_DATA cannot be used as a ComplianceType for filtering
/// results").
pub const FILTERABLE_COMPLIANCE_TYPES: [&str; 3] = [
    COMPLIANCE_TYPE_COMPLIANT,
    COMPLIANCE_TYPE_NON_COMPLIANT,
    COMPLIANCE_TYPE_NOT_APPLICABLE,
];

/// Every `ComplianceType` an AGGREGATE compliance read holds for a Config rule, and
/// therefore the only values a `ConfigRuleComplianceFilters` may name
/// (`ConfigRuleComplianceFilters$ComplianceType`: "Config supports only COMPLIANT and
/// NON_COMPLIANT. Config does not support the NOT_APPLICABLE and the
/// INSUFFICIENT_DATA values").
///
/// It is one constant for both because they are the same set: a rule whose evaluation
/// results place it in neither of these has no compliance result for an aggregate read
/// to report (`DescribeAggregateComplianceByConfigRules`: "Does not display rules that
/// do not have compliance results"), which is also why no filter may name such a
/// value.
pub const AGGREGATE_RULE_COMPLIANCE_TYPES: [&str; 2] =
    [COMPLIANCE_TYPE_COMPLIANT, COMPLIANCE_TYPE_NON_COMPLIANT];

/// How many evaluation results one page carries when the request asks for no size of
/// its own (`GetComplianceDetailsByConfigRuleRequest$Limit`: "The maximum number of
/// evaluation results returned on each page. The default is 10. ... If you specify 0,
/// Config uses the default").
pub const GET_COMPLIANCE_DETAILS_PAGE_SIZE: usize = 10;

/// How many evaluation results one page of a RESOURCE's evaluation details carries.
///
/// `GetComplianceDetailsByResource` has no `Limit` member at all
/// (`smithy_query(query="operation", operation="GetComplianceDetailsByResource")`), so
/// the page size is not the caller's to choose and the model documents none; it is the
/// page the API's other evaluation-details read serves by default
/// ([`GET_COMPLIANCE_DETAILS_PAGE_SIZE`]), so one resource's results are cut into pages
/// exactly as one rule's are.
pub const GET_COMPLIANCE_DETAILS_BY_RESOURCE_PAGE_SIZE: usize = GET_COMPLIANCE_DETAILS_PAGE_SIZE;

/// The cap a `ComplianceSummary` counts Config rules up to
/// (`ComplianceSummary$CompliantResourceCount`: "The number of Config rules or Amazon
/// Web Services resources that are compliant, up to a maximum of 25 for rules").
/// Reaching it is what sets `ComplianceContributorCount.CapExceeded`.
pub const COMPLIANCE_SUMMARY_RULE_CAP: i32 = 25;

/// The cap a count of contributing RESOURCES is reported up to
/// (`ComplianceSummary$NonCompliantResourceCount`: "The number of Config rules or
/// Amazon Web Services resources that are noncompliant, up to a maximum of 25 for
/// rules and 100 for resources"). A rule's `Compliance$ComplianceContributorCount`
/// counts resources, so it is reported up to this cap.
pub const COMPLIANCE_RESOURCE_CAP: i32 = 100;

/// `ConfigRuleComplianceSummaryGroupKey` values
/// (`com.amazonaws.configservice#ConfigRuleComplianceSummaryGroupKey`), which are what
/// an aggregate compliance summary names its groups by, and so what
/// `AggregateComplianceCount$GroupName` carries ("The 12-digit account ID or region
/// based on the GroupByKey value").
pub const COMPLIANCE_SUMMARY_GROUP_BY_ACCOUNT_ID: &str = "ACCOUNT_ID";
/// See [`COMPLIANCE_SUMMARY_GROUP_BY_ACCOUNT_ID`].
pub const COMPLIANCE_SUMMARY_GROUP_BY_AWS_REGION: &str = "AWS_REGION";

/// `ResourceCountGroupKey` values
/// (`com.amazonaws.configservice#ResourceCountGroupKey`), which are the keys a grouped
/// aggregate resource count may be grouped by, and so what
/// `GroupedResourceCount$GroupName` carries ("The name of the group that can be region,
/// account ID, or resource type"). `ACCOUNT_ID` and `AWS_REGION` are spelled the same
/// as in [`COMPLIANCE_SUMMARY_GROUP_BY_ACCOUNT_ID`], but they belong to this enum and
/// are named for it.
pub const RESOURCE_COUNT_GROUP_BY_RESOURCE_TYPE: &str = "RESOURCE_TYPE";
/// See [`RESOURCE_COUNT_GROUP_BY_RESOURCE_TYPE`].
pub const RESOURCE_COUNT_GROUP_BY_ACCOUNT_ID: &str = "ACCOUNT_ID";
/// See [`RESOURCE_COUNT_GROUP_BY_RESOURCE_TYPE`].
pub const RESOURCE_COUNT_GROUP_BY_AWS_REGION: &str = "AWS_REGION";

/// The largest `Limit` a grouped aggregate read may ask for, and the page size it
/// serves a request that asks for none (`GroupByAPILimit`, `@range(min: 0, max: 1000)`;
/// `GetAggregateConfigRuleComplianceSummaryRequest$Limit`: "The maximum number of
/// evaluation results returned on each page. The default is 1000. You cannot specify a
/// number greater than 1000. If you specify 0, Config uses the default").
pub const GROUP_BY_API_MAX_LIMIT: usize = 1000;

/// `Owner` value for a Custom Policy rule (`com.amazonaws.configservice#Owner`).
pub const OWNER_CUSTOM_POLICY: &str = "CUSTOM_POLICY";

/// `Owner` value for a Config Managed rule, i.e. one Amazon Web Services owns and
/// manages (`com.amazonaws.configservice#Owner`; `Source$Owner`: "Config Managed Rules
/// are predefined rules owned by Amazon Web Services").
pub const OWNER_AWS: &str = "AWS";

/// The `SourceIdentifier` of the Config Managed rule that evaluates a PROCESS rather
/// than a recorded resource configuration — the Config Process Check rule, whose
/// evaluations are delivered from outside Config
/// (`NoSuchConfigRuleException` on the external-evaluation path: "Verify that the rule
/// is an Config Process Check rule"). The identifier is the managed rule's entry in
/// *List of Config Managed Rules* (`Source$SourceIdentifier`: "For Config Managed
/// rules, a predefined identifier from a list"), which the model does not enumerate.
pub const SOURCE_IDENTIFIER_PROCESS_CHECK: &str = "AWS_CONFIG_PROCESS_CHECK";

/// The only `ComplianceResourceType` an externally delivered evaluation may name
/// (`ExternalEvaluation$ComplianceResourceType`: "Config accepts `AWS::::Account`
/// resource type"). A process check evaluates the account, not a recorded resource.
pub const EXTERNAL_EVALUATION_RESOURCE_TYPE: &str = "AWS::::Account";

/// Every `ComplianceType` an externally delivered evaluation may carry: the published
/// `ComplianceType` values MINUS `INSUFFICIENT_DATA`, which Config derives for a rule
/// it could not evaluate rather than accepting from a caller
/// (`ExternalEvaluation$ComplianceType`: "The valid values are COMPLIANT,
/// NON_COMPLIANT, and NOT_APPLICABLE").
pub const EXTERNAL_EVALUATION_COMPLIANCE_TYPES: [&str; 3] = [
    COMPLIANCE_TYPE_COMPLIANT,
    COMPLIANCE_TYPE_NON_COMPLIANT,
    COMPLIANCE_TYPE_NOT_APPLICABLE,
];

/// The `source` of every event Config publishes onto the EventBridge bus
/// (`EVENT_SOURCE_VALUES` in `ops::types::shared_types`, and the EventBridge
/// reference for Config).
pub const EVENT_SOURCE: &str = "aws.config";

/// The `detail-type` of the event Config publishes when a rule's evaluation verdict
/// changes (the EventBridge reference for Config, "Config Rules Compliance Change").
pub const COMPLIANCE_CHANGE_DETAIL_TYPE: &str = "Config Rules Compliance Change";

/// The `detail.messageType` that event carries: the notification Config sends when a
/// resource's compliance with a rule changes.
pub const COMPLIANCE_CHANGE_MESSAGE_TYPE: &str = "ComplianceChangeNotification";

/// The `detail.recordVersion` of a compliance-change notification, the version of the
/// notification body itself as Config stamps it.
pub const COMPLIANCE_CHANGE_RECORD_VERSION: &str = "1.0";

/// The event bus every account owns and that an Amazon Web Services service publishes
/// its own events onto ("Events generated by Amazon Web Services services go to your
/// account's default event bus").
pub const DEFAULT_EVENT_BUS_NAME: &str = "default";

/// The only `ResourceConfigurationSchemaType` the model publishes
/// (`com.amazonaws.configservice#ResourceConfigurationSchemaType`), and so the schema
/// type a resource definition is read as when the request names none.
pub const RESOURCE_CONFIGURATION_SCHEMA_TYPE_CFN: &str = "CFN_RESOURCE_SCHEMA";

/// How Config words `InvalidParameterValueException` for a resource definition that is
/// not the JSON document a resource configuration is.
///
/// `ResourceDetails$ResourceConfiguration` is "the resource definition to be evaluated
/// as per the resource configuration schema type", whose instances are JSON documents,
/// so a body that is not one cannot be evaluated against any schema. The sentence is
/// FIXED and request-independent: it quotes neither the body sent nor the member it was
/// sent at, because what the caller has to provide — valid JSON — is the whole of what
/// it says.
pub const RESOURCE_CONFIGURATION_NOT_JSON_MESSAGE: &str =
    "The provided resource configuration is not a valid JSON. Provide a valid JSON and \
     try again.";

/// `Owner` value for a Custom Lambda rule, whose evaluation Config performs by
/// invoking the caller's Lambda function (`com.amazonaws.configservice#Owner`).
/// `DescribeConfigRuleEvaluationStatus` reports no status for a rule of this owner
/// (`ConfigRuleEvaluationStatus`: "Status information for your Config Managed rules
/// and Config Custom Policy rules. … This operation does not return status
/// information about Config Custom Lambda rules.").
pub const OWNER_CUSTOM_LAMBDA: &str = "CUSTOM_LAMBDA";

/// The `lastStatus` a delivery that COMPLETED reports, and the one a delivery that was
/// refused reports (`ConfigExportDeliveryInfo$lastStatus`: "status of the last attempted
/// delivery").
///
/// RECORDED behaviour: a delivery's last status comes back upper-cased, which is the
/// spelling a client branches on; `com.amazonaws.configservice#DeliveryStatus` spells
/// its members `Success` and `Failure`.
pub const DELIVERY_STATUS_SUCCESS: &str = "SUCCESS";
/// See [`DELIVERY_STATUS_SUCCESS`].
pub const DELIVERY_STATUS_FAILURE: &str = "FAILURE";
/// The third `DeliveryStatus` value (`com.amazonaws.configservice#DeliveryStatus`).
/// `ConfigStreamDeliveryInfo$lastStatus` documents when it applies: "Providing an SNS
/// topic on a DeliveryChannel for Config is optional. If the SNS delivery is turned off,
/// the last status will be Not_Applicable."
pub const DELIVERY_STATUS_NOT_APPLICABLE: &str = "Not_Applicable";

/// The `deliveryFrequency` a delivery channel delivers configuration snapshots at
/// when its `ConfigSnapshotDeliveryProperties` names none.
///
/// The model declares no default for `ConfigSnapshotDeliveryProperties$deliveryFrequency`,
/// so this is CHOSEN: the least frequent published `MaximumExecutionFrequency` value
/// (`com.amazonaws.configservice#MaximumExecutionFrequency`), which is also the
/// snapshot frequency Config offers by default when a channel is set up without one.
pub const DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY: &str = "TwentyFour_Hours";

/// How often a delivery channel delivers a configuration history file.
///
/// AWS Config Developer Guide, *Delivering configuration items*: "Config sends a new
/// configuration history file to your Amazon S3 bucket every six hours." The history
/// schedule is fixed, so it is independent of the snapshot `deliveryFrequency`.
pub const CONFIG_HISTORY_DELIVERY_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// The interval a `MaximumExecutionFrequency` value names, or `None` for a value this
/// enum does not publish.
///
/// The five values (`One_Hour`, `Three_Hours`, `Six_Hours`, `Twelve_Hours`,
/// `TwentyFour_Hours`) come from `com.amazonaws.configservice#MaximumExecutionFrequency`
/// and each spells the number of hours between two deliveries.
pub fn delivery_frequency_interval(frequency: &str) -> Option<Duration> {
    let hours: u64 = match frequency {
        "One_Hour" => 1,
        "Three_Hours" => 3,
        "Six_Hours" => 6,
        "Twelve_Hours" => 12,
        "TwentyFour_Hours" => 24,
        _ => return None,
    };
    Some(Duration::from_secs(hours * 3600))
}

/// How long after `StartConfigurationRecorder` the recorder's `Pending` recording
/// event resolves to `Success`.
///
/// The model documents no duration for this, so the span is CHOSEN: it sits inside
/// the 250–750 ms window this project uses for a transition the clock owes, which
/// keeps the transient `Pending` status observable to a caller that describes the
/// status straight after starting the recorder.
pub const RECORDER_STATUS_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How long after `DeliverConfigSnapshot` Config performs the configuration-snapshot
/// delivery that call scheduled.
///
/// A snapshot delivery is in flight when the call returns — `DeliverConfigSnapshot`
/// answers with "the ID of the snapshot that is being created" and Config notifies
/// the start of the delivery and then its completion — so the delivery is work the
/// clock owes rather than work the call itself does. AWS documents no duration
/// for it, so the span is CHOSEN: it sits inside the 250–750 ms window this project
/// uses for such a transition, which keeps the in-flight delivery observable to a
/// caller that describes the channel's status straight after scheduling it.
pub const SNAPSHOT_DELIVERY_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most Config rules this account and Region can hold, after which
/// `PutConfigRule` refuses a new rule with `MaxNumberOfConfigRulesExceededException`.
/// 150 is the Config rules service quota (managed plus custom rules per Region).
pub const CONFIG_RULES_QUOTA: usize = 150;

/// The most resources the discovered-resource inventory holds at once, counting the
/// entries a deletion was recorded for.
///
/// `PutResourceConfig` refuses a new resource past
/// [`crate::ops::put_resource_config::ACTIVE_CUSTOM_RESOURCES_QUOTA`] ACTIVE resources
/// with the modelled `MaxActiveResourcesExceededException`, which is the bound a caller
/// meets; this one is the store's own, so the inventory stays bounded whatever writes it
/// (a resource whose deletion is recorded occupies no active quota but still occupies an
/// entry). It is the same number, because an account that holds that many active
/// resources is already at the quota Config publishes.
/// [`ComAmazonawsConfigserviceStarlingdoveserviceState::record_discovered_resource`]
/// enforces it.
pub const DISCOVERED_RESOURCES_KEPT: usize =
    crate::ops::put_resource_config::ACTIVE_CUSTOM_RESOURCES_QUOTA;

/// How many EARLIER configuration items one resource keeps, beside the current one.
///
/// `GetResourceConfigHistory` reports "each state of the resource during the specified
/// time interval", so a state the resource has left is kept rather than overwritten;
/// this bounds how many of them one resource holds, so a resource whose configuration
/// keeps changing cannot grow this process's state without end.
/// [`ComAmazonawsConfigserviceStarlingdoveserviceState::record_discovered_resource`]
/// enforces it, dropping the oldest state first.
pub const CONFIGURATION_ITEMS_KEPT: usize = 20;

/// The most conformance packs this account and Region can hold, after which
/// `PutConformancePack` refuses a NEW pack with
/// `MaxNumberOfConformancePacksExceededException` ("You have reached the limit of the
/// number of conformance packs you can create in an account. For more information, see
/// Service Limits in the Config Developer Guide").
///
/// 50 is the number of conformance packs per account per Region the Config Developer
/// Guide's "Service Limits" publishes. One engine's store stands for ONE account in ONE
/// Region, so — like [`CONFIG_RULES_QUOTA`] — it is enforced over the whole store, and
/// it is also what bounds how many packs [`ComAmazonawsConfigserviceStarlingdoveserviceState::conformance_packs`]
/// can hold.
pub const CONFORMANCE_PACKS_QUOTA: usize = 50;

/// `ConformancePackState` values (`com.amazonaws.configservice#ConformancePackState`,
/// whose documented set is `CREATE_IN_PROGRESS`, `CREATE_COMPLETE`, `CREATE_FAILED`,
/// `DELETE_IN_PROGRESS` and `DELETE_FAILED`).
///
/// A pack this emulator deploys reaches only the first two: `PutConformancePack`
/// refuses a template it cannot deploy BEFORE the pack exists, so no stored pack is in
/// a failed creation, and nothing holds a deletion in progress.
pub const CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
/// See [`CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS`].
pub const CONFORMANCE_PACK_STATE_CREATE_COMPLETE: &str = "CREATE_COMPLETE";
/// See [`CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS`]: the state of a pack whose
/// deployment could not create the resources its template declares, which is reported
/// with the `ConformancePackStatusReason` that says why.
pub const CONFORMANCE_PACK_STATE_CREATE_FAILED: &str = "CREATE_FAILED";

/// The service principal of the Config service that deploys a conformance pack's
/// resources — the service the pack's service-linked role
/// `AWSServiceRoleForConfigConforms` belongs to (`PutConformancePack`: "This API uses
/// the AWSServiceRoleForConfigConforms service-linked role in your account to create
/// conformance pack resources"). A Config rule a pack deployed reports it as its
/// `CreatedBy`, which is what tells such a rule apart from one `PutConfigRule` created.
pub const CONFORMANCE_PACK_SERVICE_PRINCIPAL: &str = "config-conforms.amazonaws.com";

/// The most characters a Config rule name CONFIG ITSELF generates may carry.
///
/// A rule's own name is a `ConfigRuleName` (1..128), but the compliance read paths take
/// the rule name as `StringWithCharLimit64` (1..64) — `GetComplianceDetailsByConfigRule`
/// and `DeleteEvaluationResults` both do — so 64 is the TIGHTEST published bound on a
/// member that names a Config rule. A name Config generates has to satisfy it: an
/// identifier the service mints and then refuses to accept back names a rule no caller
/// can read the evaluations of.
pub const GENERATED_CONFIG_RULE_NAME_MAX_LENGTH: usize = 64;

/// The name Config gives a Config rule a conformance pack deploys: the name the
/// template declares for it, suffixed with the pack's own id, which is what keeps two
/// packs declaring the same rule name from deploying one rule over the other and keeps
/// a redeploy of the SAME pack landing on the same rule.
///
/// The result stays within [`GENERATED_CONFIG_RULE_NAME_MAX_LENGTH`]: the suffix is
/// what identifies the pack, so it is the DECLARED half that gives way when the two
/// together would not fit — the rule is still readable through every path that takes a
/// rule name, which is what a generated name has to be.
pub fn conformance_pack_rule_name(declared: &str, pack_id: &str) -> String {
    let suffix = format!("-{pack_id}");
    let room = GENERATED_CONFIG_RULE_NAME_MAX_LENGTH.saturating_sub(suffix.chars().count());
    let kept: String = declared.chars().take(room).collect();
    format!("{kept}{suffix}")
}

/// How long after the call that requested it a conformance pack's deployment is due to
/// be complete — the `CREATE_IN_PROGRESS` -> `CREATE_COMPLETE` move
/// (`ConformancePackState`: "CREATE_IN_PROGRESS when a conformance pack creation is in
/// progress", "CREATE_COMPLETE when a conformance pack has been successfully created").
///
/// The move is one the CLOCK owes, not one the call itself makes: Config deploys the
/// pack's CloudFormation stack after answering. AWS documents no duration for it, so
/// the span is CHOSEN: it sits inside the 250–750 ms window this project uses for such
/// a transition (see [`RECORDER_STATUS_SETTLE_AFTER`]), which keeps the in-progress
/// state observable to a caller that describes the pack straight after creating it.
pub const CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How many conformance pack status records one `DescribeConformancePackStatus` page
/// carries when the request does not ask for a page size — `Limit` defaults to 0, which
/// is "unspecified" rather than "no results". 20 is `Limit`'s own maximum (range
/// 0..20), so it is the largest page the model lets one response carry.
pub const DESCRIBE_CONFORMANCE_PACK_STATUS_PAGE_SIZE: usize = 20;

/// How many conformance packs one `DescribeConformancePacks` page carries when the
/// request does not ask for a page size, for the same reason as
/// [`DESCRIBE_CONFORMANCE_PACK_STATUS_PAGE_SIZE`]: `Limit` defaults to 0, which is
/// "unspecified" rather than "no results", and 20 is that member's own maximum (range
/// 0..20).
pub const DESCRIBE_CONFORMANCE_PACKS_PAGE_SIZE: usize = 20;

/// How many conformance pack summaries one `GetConformancePackComplianceSummary` page
/// carries when the request does not ask for a page size — `Limit` defaults to 0, which
/// is "unspecified" rather than "no results". 5 is the length maximum of the
/// `ConformancePackComplianceSummaryList` one response carries (and of the
/// `ConformancePackNames` one request may name), so it is the largest page the model lets
/// one response carry whatever `Limit`'s own range (0..20) allows.
pub const GET_CONFORMANCE_PACK_COMPLIANCE_SUMMARY_PAGE_SIZE: usize = 5;

/// How Config words `NoSuchConformancePackException` ("You specified one or more
/// conformance packs that do not exist") for the `missing` names of a request that
/// named packs this account and Region does not hold.
///
/// RECORDED behaviour: the fixed sentence is "No such conformance pack exists.
/// Conformance Pack(s): ", and what follows it is BUILT FROM THE REQUEST — the names the
/// caller sent that resolved to no pack, in the order they were sent. An operation that
/// takes a LIST of pack names renders them as the list they are, bracketed
/// ([`no_such_conformance_pack_list_message`]); the operations that name ONE pack render
/// that name bare, which is this spelling.
pub fn no_such_conformance_pack_message(missing: &[String]) -> String {
    format!(
        "{NO_SUCH_CONFORMANCE_PACK_SENTENCE}{}",
        missing.join(", ")
    )
}

/// The fixed text every `NoSuchConformancePackException` of this service carries, before
/// the names the request asked about. ONE statement of it, so the two renderings below
/// cannot word the one refusal two ways.
pub const NO_SUCH_CONFORMANCE_PACK_SENTENCE: &str =
    "No such conformance pack exists. Conformance Pack(s): ";

/// [`no_such_conformance_pack_message`] as the operations that take a LIST of conformance
/// pack names render it: the same fixed sentence, and the names the request sent rendered
/// as the list they are, inside square brackets.
pub fn no_such_conformance_pack_list_message(missing: &[String]) -> String {
    format!(
        "{NO_SUCH_CONFORMANCE_PACK_SENTENCE}[{}]",
        missing.join(", ")
    )
}

/// How Config words `ResourceInUseException` for a conformance pack whose deployment
/// workflow — a creation, an update or a deletion — has not settled yet, so the call
/// cannot start another one for it.
///
/// The message is BUILT FROM THE REQUEST: the fixed sentence states that a workflow
/// holds the pack, and its one variable part is the conformance pack name the caller
/// named, so a client that fans several deployments out can tell WHICH pack is busy
/// from the refusal alone. ONE template for every operation that refuses a pack whose
/// workflow is in flight, so they cannot word the one refusal two ways.
pub fn conformance_pack_workflow_in_progress_message(name: &str) -> String {
    format!(
        "A workflow is currently in progress for the Conformance Pack: {name}. Please try later."
    )
}

/// How Config words `NoSuchConfigRuleInConformancePackException` ("Config rule that you
/// passed in the filter does not exist") for a filter that named a rule the pack holds
/// none of.
///
/// The message is BUILT FROM THE REQUEST: the fixed sentence names the class of failure,
/// and its two variable parts are the rule name the caller sent that resolved to no rule
/// of the pack and the pack the caller asked about. It is about ONE rule, so a filter
/// carrying several unknown names is reported for the first of them the request carried —
/// ONE template for every operation whose filter takes rule names of a pack, so the two
/// compliance reads cannot word the one refusal two ways.
pub fn no_such_config_rule_in_conformance_pack_message(rule_name: &str, pack_name: &str) -> String {
    format!("No rule with name {rule_name} exists in conformance pack {pack_name}")
}

/// The most rule names one `ConformancePackComplianceFilters` may name
/// (`ConformancePackConfigRuleNames`, `@length(min: 0, max: 10)`).
pub const CONFORMANCE_PACK_COMPLIANCE_FILTER_NAMES_MAX: usize = 10;

/// How many rule compliance records one `DescribeConformancePackCompliance` page
/// carries when the request does not ask for a page size, for the same reason as
/// [`DESCRIBE_CONFORMANCE_PACK_STATUS_PAGE_SIZE`]: `Limit` defaults to 0, which is
/// "unspecified" rather than "no results", and 1000 is that member's own maximum (range
/// 0..1000) — which is also the length maximum of the
/// `ConformancePackRuleComplianceList` one response carries, so it is the largest page
/// the model lets one response carry.
pub const DESCRIBE_CONFORMANCE_PACK_COMPLIANCE_PAGE_SIZE: usize = 1000;

/// How many evaluation results one `GetConformancePackComplianceDetails` page carries
/// when the request does not ask for a page size
/// (`GetConformancePackComplianceDetailsRequest$Limit`: "The maximum number of
/// evaluation results returned on each page. If you do no specify a number, Config uses
/// the default. The default is 100"), which is also that member's own maximum (range
/// 0..100) and the length maximum of the `ConformancePackRuleEvaluationResultsList` one
/// response carries.
pub const GET_CONFORMANCE_PACK_COMPLIANCE_DETAILS_PAGE_SIZE: usize = 100;

/// How many group summaries one `GetAggregateConformancePackComplianceSummary` page
/// carries when the request does not ask for a page size, for the same reason as
/// [`DESCRIBE_CONFORMANCE_PACK_STATUS_PAGE_SIZE`]: `Limit` defaults to 0, which is
/// "unspecified" rather than "no results"
/// (`GetAggregateConformancePackComplianceSummaryRequest$Limit`: "The maximum number of
/// results returned on each page. The default is maximum. If you specify 0, Config uses
/// the default"), and 100 is that member's own maximum (`Limit`, range 0..100), which is
/// what "the default is maximum" names.
pub const AGGREGATE_CONFORMANCE_PACK_COMPLIANCE_SUMMARY_PAGE_SIZE: usize = 100;

/// How many conformance pack compliance records one
/// `DescribeAggregateComplianceByConformancePacks` page carries when the request does
/// not ask for a page size, for the same reason as
/// [`AGGREGATE_CONFORMANCE_PACK_COMPLIANCE_SUMMARY_PAGE_SIZE`]: `Limit` defaults to 0,
/// which is "unspecified" rather than "no results"
/// (`DescribeAggregateComplianceByConformancePacksRequest$Limit`: "The maximum number of
/// conformance packs compliance details returned on each page. The default is maximum.
/// If you specify 0, Config uses the default"), and 100 is that member's own maximum
/// (`Limit`, range 0..100), which is what "the default is maximum" names.
pub const AGGREGATE_COMPLIANCE_BY_CONFORMANCE_PACKS_PAGE_SIZE: usize = 100;

/// Every `ComplianceType` a conformance pack's EVALUATION filter may name
/// (`ConformancePackEvaluationFilters$ComplianceType`: "The allowed values are COMPLIANT
/// and NON_COMPLIANT. INSUFFICIENT_DATA is not supported").
///
/// It is narrower than the values such a report can CARRY
/// (`ConformancePackComplianceType`, which `INSUFFICIENT_DATA` belongs to), so a filter
/// naming one outside it is a bad parameter rather than a filter matching nothing.
pub const CONFORMANCE_PACK_EVALUATION_FILTER_COMPLIANCE_TYPES: [&str; 2] =
    [COMPLIANCE_TYPE_COMPLIANT, COMPLIANCE_TYPE_NON_COMPLIANT];

/// The most resource ids one `ConformancePackEvaluationFilters` may name
/// (`ConformancePackComplianceResourceIds`, `@length(min: 0, max: 5)`).
pub const CONFORMANCE_PACK_EVALUATION_FILTER_RESOURCE_IDS_MAX: usize = 5;

/// The third-party cloud service provider a connector connects to
/// (`com.amazonaws.configservice#Provider`, whose only published value is `AZURE`;
/// `ConnectorSummary$provider`: "Currently, AZURE is supported").
pub const PROVIDER_AZURE: &str = "AZURE";

/// The only filter `ListConnectors` publishes
/// (`com.amazonaws.configservice#ConnectorFilterName`; `ConnectorFilter$filterName`:
/// "Currently, only provider is supported").
pub const CONNECTOR_FILTER_PROVIDER: &str = "provider";

/// How many connectors one `ListConnectors` page carries when the request does not
/// ask for a page size — `MaxResults` defaults to 0, which is "unspecified" rather
/// than "no results". 100 is the `ConnectorSummaries` length maximum (0..100), and
/// also `ListConnectorsMaxResults`'s own maximum, so it is the largest page the model
/// lets one response carry.
pub const LIST_CONNECTORS_PAGE_SIZE: usize = 100;

/// The most connectors this store can hold: the bound past which a connector could
/// not be reported any more, which is what `MaxNumberOfConnectorsExceededException`
/// stands for.
///
/// NOTHING ENFORCES IT TODAY, because nothing in `src/` inserts a connector:
/// `PutConnector` verifies a provider configuration's credentials WITH the provider
/// before it materializes anything, and verifying them is an outbound connection this
/// emulator does not make, so every configuration is refused (see
/// [`UNVERIFIABLE_AZURE_CREDENTIALS_MESSAGE`] and `crate::ops::put_connector`). It is
/// kept as the documented cap of the connector store — the number the read paths' page
/// size is taken from — and is where the quota check belongs if a connector ever
/// enters the store again.
///
/// The model carries no number for this quota, so the limit is CHOSEN: it is the
/// `ConnectorSummaries` length maximum (0..100), the most connectors the model can
/// report in one page. Config documents the connector quota per account per Region
/// and one engine's store stands for ONE account in ONE Region, so — like
/// [`CONFIG_RULES_QUOTA`] and [`CONFIGURATION_AGGREGATORS_QUOTA`] — it is enforced
/// over the WHOLE store: the Region a request is scoped to is the caller's own, so
/// counting per Region would let a caller keep one bucket of connectors per Region
/// string it invents. This is also what bounds the work `ListConnectors` does to
/// filter one Region's connectors out of the store.
pub const CONNECTORS_QUOTA: usize = 100;

/// The most customer managed configuration recorders this account and Region can
/// hold. Config allows exactly ONE ("One customer managed configuration recorder per
/// account per Region ... You can have only one customer managed configuration
/// recorder for each account for each Amazon Web Services Region", the
/// `PutConfigurationRecorder` doc), after which it refuses a second with
/// `MaxNumberOfConfigurationRecordersExceededException` — which states this limit
/// (see [`max_configuration_recorders_message`]).
pub const CUSTOMER_MANAGED_RECORDERS_QUOTA: usize = 1;

/// The most SERVICE-LINKED configuration recorders this account and Region can hold,
/// after which `PutServiceLinkedConfigurationRecorder` refuses a new one with
/// `LimitExceededException` (which the model documents for exactly that API: "this
/// exception is thrown if the number of service-linked roles in the account exceeds
/// the limit"), stated by [`service_linked_recorders_quota_message`].
///
/// The model carries no number for this quota, so the limit is CHOSEN — the same way
/// [`CONNECTORS_QUOTA`] is: it is [`LIST_CONFIGURATION_RECORDERS_PAGE_SIZE`], the most
/// recorder summaries one `ListConfigurationRecorders` page can carry, so every
/// service-linked recorder the store holds stays reportable. A recorder's name is
/// derived from the `ServicePrincipal` the request carries, so without this bound a
/// caller could grow the store by inventing service principals; Config documents its
/// recorder quotas per account per Region and one engine's store stands for ONE
/// account in ONE Region, so — like [`CONFIG_RULES_QUOTA`] — it is enforced over the
/// whole store rather than per Region string a caller invents.
pub const SERVICE_LINKED_RECORDERS_QUOTA: usize = LIST_CONFIGURATION_RECORDERS_PAGE_SIZE;

/// The most configuration aggregators this account and Region can hold, after which
/// `PutConfigurationAggregator` refuses a new one with `LimitExceededException`
/// (`LimitExceededException`: "For PutConfigurationAggregator API, this exception is
/// thrown if the number of accounts and aggregators exceeds the limit"). 50 is the
/// Config "maximum number of configuration aggregators" service quota.
pub const CONFIGURATION_AGGREGATORS_QUOTA: usize = 50;

/// How many aggregators one `DescribeConfigurationAggregators` page carries when the
/// request asks for no `Limit` (`DescribeConfigurationAggregatorsRequest$Limit`: "The
/// maximum number of configuration aggregators returned on each page. The default is
/// maximum. If you specify 0, Config uses the default") — the maximum being the
/// modelled `Limit` range maximum.
pub const DESCRIBE_CONFIGURATION_AGGREGATORS_PAGE_SIZE: i32 = 100;

/// How many authorizations one `DescribeAggregationAuthorizations` page carries when
/// the request asks for no `Limit` (`DescribeAggregationAuthorizationsRequest$Limit`:
/// "The maximum number of AggregationAuthorizations returned on each page. The default
/// is maximum. If you specify 0, Config uses the default") — the maximum being the
/// modelled `Limit` range maximum.
pub const DESCRIBE_AGGREGATION_AUTHORIZATIONS_PAGE_SIZE: i32 = 100;

/// How many source statuses one `DescribeConfigurationAggregatorSourcesStatus` page
/// carries when the request asks for no `Limit`
/// (`DescribeConfigurationAggregatorSourcesStatusRequest$Limit`: "The maximum number
/// of AggregatorSourceStatus returned on each page. The default is maximum. If you
/// specify 0, Config uses the default") — the maximum being the modelled `Limit` range
/// maximum.
pub const DESCRIBE_CONFIGURATION_AGGREGATOR_SOURCES_STATUS_PAGE_SIZE: i32 = 100;

/// The most aggregation authorizations this account and Region can hold, after which
/// `PutAggregationAuthorization` refuses a NEW authorized pair.
///
/// Config publishes no quota for aggregation authorizations, so this is the EMULATOR's
/// own bound, not a documented AWS one: the store's keys are whole-cloth caller input
/// (an authorized account and an authorized Region), so without a cap one caller could
/// grow it one entry per distinct pair without end. It is two orders of magnitude above
/// the aggregator quota Config does publish, so no faithful workload meets it.
pub const AGGREGATION_AUTHORIZATIONS_QUOTA: usize = 5_000;

/// The most source accounts one account aggregation source can name, after which
/// `PutConfigurationAggregator` refuses it with `LimitExceededException`
/// (`LimitExceededException`: "For PutConfigurationAggregator API, this exception is
/// thrown if the number of accounts and aggregators exceeds the limit").
///
/// The model bounds `AccountAggregationSourceAccountList` only from below (length
/// 1..), so this is the EMULATOR's own bound, like [`AGGREGATION_AUTHORIZATIONS_QUOTA`]
/// above: a source-status read reports one record per (account, Region) PAIR, so a
/// stored source whose account list were unbounded would let one write decide how much
/// work every later read of it has to do. It is two orders of magnitude above the
/// aggregator quota Config does publish, so no faithful workload meets it.
pub const AGGREGATION_SOURCE_ACCOUNTS_QUOTA: usize = 10_000;

/// The most Regions one aggregation source can list, after which
/// `PutConfigurationAggregator` refuses it. The emulator's own bound, for the same
/// reason as [`AGGREGATION_SOURCE_ACCOUNTS_QUOTA`] — the Regions are the other factor
/// of that product, and `AggregatorRegionList` is likewise bounded only from below
/// (length 1..).
///
/// It is above every count of Regions Amazon Web Services has published, and a source
/// that aggregates all of them says so with `AllAwsRegions` rather than by listing
/// them, so no faithful workload meets it either.
pub const AGGREGATION_SOURCE_REGIONS_QUOTA: usize = 100;

/// How many rules one `DescribeConfigRules` page carries.
///
/// The operation models no `Limit` member, so the page size is this emulator's:
/// it matches the `ConfigRuleNames` length maximum (0..25) the same operation
/// declares.
pub const DESCRIBE_CONFIG_RULES_PAGE_SIZE: usize = 25;

/// The most stored queries this account and Region can hold (`PutStoredQuery`: "You
/// can create upto 300 queries in a single Amazon Web Services account and a single
/// Amazon Web Services Region"). A request that would create one past it is refused
/// with `ValidationException`, which is the error the model lists for "You are trying
/// to create more than 300 queries".
pub const STORED_QUERIES_QUOTA: usize = 300;

/// How many stored queries one `ListStoredQueries` page carries when the request asks
/// for no page size (`ListStoredQueries`: "Lists the stored queries for a single
/// Amazon Web Services account and a single Amazon Web Services Region. The default is
/// 100", and `MaxResults` defaults to 0, which is "no page size asked for" rather than
/// "no results"). It is also `Limit`'s own published maximum (range 0..100), so it is
/// the largest page the model lets this operation report.
pub const LIST_STORED_QUERIES_PAGE_SIZE: usize = 100;

/// The timeout an evaluation runs under when the request asks for none
/// (`StartResourceEvaluationRequest$EvaluationTimeout`: "The timeout for an
/// evaluation. The default is 900 seconds. ... If you specify 0, Config uses the
/// default").
pub const DEFAULT_EVALUATION_TIMEOUT_SECONDS: i32 = 900;

/// How many resource evaluations one `ListResourceEvaluations` page carries when the
/// request asks for no `Limit` (`ListResourceEvaluationsRequest$Limit`: "The maximum
/// number of evaluations returned on each page. The default is 10. ... If you specify
/// 0, Config uses the default").
pub const LIST_RESOURCE_EVALUATIONS_DEFAULT_LIMIT: usize = 10;

/// The most resource evaluations this store keeps.
///
/// Config publishes no quota for started resource evaluations and
/// `StartResourceEvaluation` models no limit error, so this bound is this emulator's
/// and is CHOSEN: every record retains the caller's `ResourceDetails`, whose
/// `ResourceConfiguration` the model bounds at 51200 characters, and no operation of
/// this service deletes a resource evaluation, so the store is capped to keep this
/// process's memory bounded however many evaluations are started. Starting one past
/// the bound drops the OLDEST evaluation (and the client token that started it)
/// rather than refusing an evaluation AWS accepts, so the list path keeps reporting
/// the most recent evaluations and nothing grows without end. See
/// [`ComAmazonawsConfigserviceStarlingdoveserviceState::record_resource_evaluation`].
pub const RESOURCE_EVALUATIONS_KEPT: usize = 1_000;

/// How many rule evaluation statuses one `DescribeConfigRuleEvaluationStatus` page
/// carries when the request does not ask for a number of its own.
///
/// `RuleLimit`'s modelled default is 0 — "no stated limit" rather than "no results" —
/// so the page size for such a request is this emulator's: it matches the
/// `ConfigRuleNames` length maximum (0..25) the same operation declares.
pub const DESCRIBE_CONFIG_RULE_EVALUATION_STATUS_PAGE_SIZE: usize = 25;

/// How long after a Config rule is created or updated the evaluation Config owes that
/// rule completes.
///
/// Config evaluates a rule as soon as it is written, with no call from the caller
/// (`StartConfigRulesEvaluation`: "You don't need to call the
/// StartConfigRulesEvaluation API to run an evaluation for a new rule. When you create
/// a rule, Config evaluates your resources against the rule automatically."). The
/// model documents no duration for that evaluation, so the span is CHOSEN: it sits
/// inside the 250–750 ms window this project uses for a transition the clock owes,
/// which keeps the not-yet-evaluated rule observable to a caller that reads the
/// evaluation status straight after writing the rule.
pub const RULE_EVALUATION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The `Parameters` key naming the role Config assumes to run a remediation
/// (`PutRemediationConfigurations`: "for automatic remediation configuration, the
/// only valid assumeRole field value is AutomationAssumeRole and you need to provide
/// a value for AutomationAssumeRole to remediate your resources").
pub const AUTOMATION_ASSUME_ROLE_PARAMETER: &str = "AutomationAssumeRole";

/// The other `Parameters` key a MANUAL remediation configuration may name that role
/// with (`PutRemediationConfigurations`: "For manual remediation configuration, you
/// need to provide a value for automationAssumeRole or use a value in the assumeRole
/// field to remediate your resources. The SSM automation document can use either as
/// long as it maps to a valid parameter").
pub const ASSUME_ROLE_PARAMETER: &str = "AssumeRole";

/// The `MaximumAutomaticAttempts` an automatic remediation configuration retries up
/// to when the request names no number
/// (`RemediationConfiguration$MaximumAutomaticAttempts`: "The maximum number of
/// failed attempts for auto-remediation. If you do not select a number, the default
/// is 5").
pub const DEFAULT_MAXIMUM_AUTOMATIC_ATTEMPTS: i32 = 5;

/// The `RetryAttemptSeconds` window an automatic remediation configuration retries
/// within when the request names no number
/// (`RemediationConfiguration$RetryAttemptSeconds`: "Time window to determine whether
/// or not to add a remediation exception to prevent infinite remediation attempts ...
/// If you do not select a number, the default is 60 seconds").
pub const DEFAULT_RETRY_ATTEMPT_SECONDS: i64 = 60;

/// How many entries one discovered-resource page carries when the request asks for no
/// limit of its own — resource identifiers for `ListDiscoveredResources`
/// (`ListDiscoveredResourcesRequest$limit`: "The maximum number of resource
/// identifiers returned on each page. The default is 100. ... If you specify 0, Config
/// uses the default"), `ResourceCount` objects for `GetDiscoveredResourceCounts`
/// ("By default, Config lists 100 ResourceCount objects on each page").
pub const DISCOVERED_RESOURCES_DEFAULT_LIMIT: i32 = 100;

/// The largest `limit` a discovered-resource page may ask for
/// (`com.amazonaws.configservice#Limit`, range 0..100; the same member's doc: "You
/// cannot specify a number greater than 100"). A value outside the range is what
/// `InvalidLimitException` reports.
pub const DISCOVERED_RESOURCES_MAX_LIMIT: i32 = 100;

/// The most resource IDs one discovered-resource read may narrow by
/// (`ListDiscoveredResourcesRequest$resourceIds`: "You can list a minimum of 1
/// resourceID and a maximum of 20 resourceIds").
pub const DISCOVERED_RESOURCES_MAX_RESOURCE_IDS: usize = 20;

/// How many rules one `DescribeComplianceByConfigRule` page carries.
///
/// That operation models no `Limit` member either, and it pages over the same rules
/// `DescribeConfigRules` does, so it serves them at the same page size.
pub const DESCRIBE_COMPLIANCE_BY_CONFIG_RULE_PAGE_SIZE: usize = DESCRIBE_CONFIG_RULES_PAGE_SIZE;

/// How many resources one `DescribeComplianceByResource` page carries when the request
/// leaves `Limit` at its modelled default of 0
/// (`DescribeComplianceByResourceRequest$Limit`: "The maximum number of evaluation
/// results returned on each page. The default is 10. … If you specify 0, Config uses
/// the default").
///
/// The same ten the API's evaluation-details reads default to
/// ([`GET_COMPLIANCE_DETAILS_PAGE_SIZE`]), which their own `Limit` documents with the
/// same sentence, so the default is written once.
pub const DESCRIBE_COMPLIANCE_BY_RESOURCE_PAGE_SIZE: usize = GET_COMPLIANCE_DETAILS_PAGE_SIZE;

/// The most tags one Config resource can carry, after which `TagResource` refuses
/// the request with `TooManyTagsException` ("You have reached the limit of the number
/// of tags you can use"). 50 is the tags-per-resource limit, which the model
/// corroborates: every `Tags` member it declares, and `TagResource$Tags` itself, has a
/// `TagList` length maximum of 50, so no resource can be given more than 50 tags in
/// one request either.
pub const TAGS_PER_RESOURCE_QUOTA: usize = 50;

/// The largest `Limit` `ListTagsForResource` accepts
/// (`ListTagsForResourceRequest$Limit`: "The limit maximum is 50. You cannot specify a
/// number greater than 50"), past which it answers `InvalidLimitException`. The
/// MODELLED range of `Limit` is 0..100, so a value in 51..=100 clears boundary
/// validation and this limit is what refuses it.
pub const LIST_TAGS_MAX_LIMIT: i32 = 50;

/// How many tags one `ListTagsForResource` page carries when the request leaves
/// `Limit` at its modelled default of 0 (`ListTagsForResourceRequest$Limit`: "If you
/// specify 0, Config uses the default").
///
/// The default itself is not published, so it is this emulator's: the limit maximum
/// the same member documents, [`LIST_TAGS_MAX_LIMIT`].
pub const LIST_TAGS_DEFAULT_LIMIT: i32 = LIST_TAGS_MAX_LIMIT;

/// The most retention configurations this account and Region can hold
/// (`DescribeRetentionConfigurations` and `PutRetentionConfiguration`: "Currently,
/// Config supports only one retention configuration per region in your account").
pub const RETENTION_CONFIGURATIONS_QUOTA: usize = 1;

/// How many retention configurations one `DescribeRetentionConfigurations` page
/// carries.
///
/// The operation models no `Limit` member, so the page size is this emulator's: it
/// matches [`RETENTION_CONFIGURATIONS_QUOTA`], so one page carries every retention
/// configuration the account is allowed to hold.
pub const DESCRIBE_RETENTION_CONFIGURATIONS_PAGE_SIZE: usize = RETENTION_CONFIGURATIONS_QUOTA;

/// The most pending aggregation requests one `DescribePendingAggregationRequests`
/// page carries: the maximum of that operation's own `Limit`
/// (`DescribePendingAggregationRequestsRequest$Limit`, range 0..20, "The default is
/// maximum. If you specify 0, Config uses the default"), so a request that asks for
/// no limit is served this many.
pub const DESCRIBE_PENDING_AGGREGATION_REQUESTS_MAX_LIMIT: usize = 20;

/// Wording for the (unreachable in the assembled emulator) case where this
/// service's store cannot be reached, i.e. the engine is not the shared
/// `InMemoryEngine`.
pub const STATE_UNAVAILABLE: &str =
    "the Config emulator could not reach its state store for this request";

/// The tags on ONE resource, keyed by tag key.
///
/// A map, not the modelled `TagList`, because a resource carries each tag key at most
/// once: re-tagging a key updates its value (`TagResource`: "If existing tags are
/// specified, however, then their values will be updated"). The key ordering is also
/// the stable ordering `ListTagsForResource` pages over, so a page token survives a
/// later tag or untag.
pub type TagSet = BTreeMap<String, String>;

/// The tag set a request's `TagList` describes: the key is taken VERBATIM (so the key
/// a write stores is the key an untag or a read names), the last value wins for a key
/// the list repeats, and an entry carrying no key is dropped (the model leaves
/// `Tag$Key` optional).
pub fn tag_set(tags: &[Tag]) -> TagSet {
    tags.iter()
        .filter_map(|tag| {
            let key = tag.key.as_deref()?;
            Some((key.to_string(), tag.value.clone().unwrap_or_default()))
        })
        .collect()
}

/// A tag set as the modelled `TagList` reports it, in tag-key order.
pub fn tag_list<'a>(tags: impl IntoIterator<Item = (&'a String, &'a String)>) -> Vec<Tag> {
    tags.into_iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// Whether `arn` is a well-formed Amazon Resource Name: the `arn` prefix and six
/// colon-separated fields (`arn:<partition>:<service>:<region>:<account>:<resource>`),
/// with a partition, a service and a resource. Region and account are empty in the
/// ARNs of some services, so neither is required here.
pub fn is_amazon_resource_name(arn: &str) -> bool {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && !fields[2].is_empty()
        && !fields[5].is_empty()
}

/// How Config words `ValidationException` for a tagging request whose `ResourceArn` is
/// not an ARN at all.
///
/// BUILT FROM THE REQUEST — it quotes the value the caller sent and names the member
/// to fix — so every operation that refuses a malformed `ResourceArn` words it the
/// same way.
pub fn malformed_resource_arn_message(arn: &str) -> String {
    format!(
        "The ResourceArn '{arn}' provided in the request is invalid. Please check the \
         resource ARN."
    )
}

/// How Config words `ResourceNotFoundException` for a tagging request whose
/// `ResourceArn` names a resource this account does not hold in this Region.
///
/// BUILT FROM THE REQUEST AND FROM WHERE THE LOOKUP HAPPENED — the ARN the caller sent
/// and the account it was looked up in — for the same reason as
/// [`malformed_resource_arn_message`]: the sentence tells the caller both what was not
/// found and that it was searched for among THEIR resources, which is what distinguishes
/// an ARN naming nothing from an ARN belonging to somebody else. The account is this
/// emulator's own ([`::emulator_server::emulator_account_id`]), the account every
/// resource it holds belongs to, so it is read from the store's identity rather than
/// from the ARN the request carried.
pub fn resource_not_found_message(arn: &str) -> String {
    format!(
        "Resource {arn} does not exist for account {}",
        ::emulator_server::emulator_account_id()
    )
}

/// One persisted customer managed configuration recorder.
#[derive(Debug, Clone, Default)]
pub struct RecorderEntry {
    pub name: String,
    /// The id Config assigned this recorder when it was created. It is part of the
    /// recorder's ARN and never changes once minted.
    pub id: String,
    pub arn: String,
    /// The IAM role Config records with. A CUSTOMER MANAGED recorder carries the role
    /// the request named; a SERVICE-LINKED one carries Config's own service-linked role
    /// ([`config_service_linked_role_arn`]), which it creates in the account rather
    /// than taking from the caller. The model leaves `roleARN` optional, so a recorder
    /// stored without one reports none rather than an invented role.
    pub role_arn: Option<String>,
    pub recording_group: Option<RecordingGroup>,
    pub recording_mode: Option<RecordingMode>,
    pub recording_scope: Option<String>,
    pub service_principal: Option<String>,
    /// The connector linking this recorder to a third-party cloud service provider,
    /// set only on a recorder
    /// `PutThirdPartyServiceLinkedConfigurationRecorder` created.
    pub connector_arn: Option<String>,
    /// The scope of third-party resources this recorder records, set only on a
    /// recorder linked to a connector.
    pub scope_configuration: Option<ScopeConfiguration>,
    /// The third-party cloud service provider whose resources a service-linked
    /// recorder records (`ConfigurationRecorderSummary$provider`: "For service-linked
    /// configuration recorders that record resources from a third-party cloud service
    /// provider, indicates the cloud service provider"), so it stays unset for every
    /// recorder this account creates itself. Only
    /// `PutThirdPartyServiceLinkedConfigurationRecorder` sets it.
    pub provider: Option<String>,
    /// The recorder's tags. `PutConfigurationRecorder` takes them at creation and
    /// does not update them afterwards ("Tags are added at creation and cannot be
    /// updated"); `TagResource` and `UntagResource` are what change them after that
    /// ("Use TagResource and UntagResource to update tags after creation").
    pub tags: TagSet,
}

impl RecorderEntry {
    /// The modelled `ConfigurationRecorder`, read back out of state.
    pub fn to_shape(&self) -> ConfigurationRecorder {
        ConfigurationRecorder {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            role_arn: self.role_arn.clone(),
            recording_group: self.recording_group.clone(),
            recording_mode: self.recording_mode.clone(),
            recording_scope: self.recording_scope.clone(),
            service_principal: self.service_principal.clone(),
            connector_arn: self.connector_arn.clone(),
            scope_configuration: self.scope_configuration.clone(),
        }
    }

    /// Whether this recorder is linked to (and managed by) an Amazon Web Services
    /// service rather than created and paid for by this account — which is what
    /// carrying a service principal means.
    pub fn is_service_linked(&self) -> bool {
        self.service_principal.is_some()
    }

    /// The modelled `ConfigurationRecorderSummary`, read back out of state: the
    /// recorder's identity and the scope it records in, without its configuration.
    ///
    /// `recordingScope` is required in the summary while state keeps it optional, so a
    /// recorder stored without one summarises with the scope
    /// [`materialised_recording_scope`] gives it.
    pub fn to_summary(&self) -> ConfigurationRecorderSummary {
        ConfigurationRecorderSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            service_principal: self.service_principal.clone(),
            recording_scope: materialised_recording_scope(self.recording_scope.clone()),
            provider: self.provider.clone(),
        }
    }
}

/// The recording status of one configuration recorder.
#[derive(Debug, Clone, Default)]
pub struct RecorderStatusEntry {
    pub name: String,
    pub arn: String,
    /// For a service-linked recorder, the service it is linked to; `None` for a
    /// customer managed one.
    pub service_principal: Option<String>,
    pub recording: bool,
    pub last_start_time: Option<SystemTime>,
    pub last_stop_time: Option<SystemTime>,
    pub last_status: Option<String>,
    pub last_status_change_time: Option<SystemTime>,
    /// When set, the instant at which the `Pending` recording event this recorder
    /// is processing becomes `Success`. Materialised by [`Self::settle`].
    pub status_settles_at: Option<SystemTime>,
}

impl RecorderStatusEntry {
    /// A status record for a recorder that has never been started.
    pub fn new(name: &str, arn: &str) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            recording: false,
            ..Self::default()
        }
    }

    /// A status record for a SERVICE-LINKED recorder that is ALREADY recording at
    /// `started_at`, because the service it is linked to turned it on when it created
    /// it: the caller cannot start or stop a service-linked configuration recorder
    /// (`UnmodifiableEntityException`: "For StartConfigurationRecorder and
    /// StopConfigurationRecorder ... these operations do not affect service-linked
    /// configuration recorders"), and the linked service holds it in use
    /// (`ConflictException`, for `DeleteServiceLinkedConfigurationRecorder`: "you
    /// cannot delete the service-linked recorder because it is currently in use by
    /// the linked Amazon Web Services service").
    ///
    /// `service_principal` is the service the recorder is linked to, which the status
    /// reports for exactly this kind of recorder
    /// (`ConfigurationRecorderStatus$servicePrincipal`).
    ///
    /// Like a start the caller asked for, the recording event is `Pending` until the
    /// clock makes the move to `Success` due; [`Self::settle`] materialises it.
    pub fn started(name: &str, arn: &str, service_principal: &str, started_at: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            service_principal: Some(service_principal.to_string()),
            recording: true,
            last_start_time: Some(started_at),
            last_stop_time: None,
            last_status: Some(RECORDER_STATUS_PENDING.to_string()),
            last_status_change_time: Some(started_at),
            status_settles_at: Some(started_at + RECORDER_STATUS_SETTLE_AFTER),
        }
    }

    /// Materialise the status change the clock has already made due.
    ///
    /// This is the ONLY write a reporting operation performs: the `Pending` ->
    /// `Success` move was owed at `status_settles_at` whether or not anybody
    /// described the recorder.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.status_settles_at {
            if now >= due {
                self.last_status = Some(RECORDER_STATUS_SUCCESS.to_string());
                self.last_status_change_time = Some(due);
                self.status_settles_at = None;
            }
        }
    }

    /// The modelled `ConfigurationRecorderStatus`, read back out of state.
    pub fn to_shape(&self) -> ConfigurationRecorderStatus {
        ConfigurationRecorderStatus {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            last_start_time: self.last_start_time.map(timestamp),
            last_stop_time: self.last_stop_time.map(timestamp),
            recording: self.recording,
            last_status: self.last_status.clone(),
            last_error_code: None,
            last_error_message: None,
            last_status_change_time: self.last_status_change_time.map(timestamp),
            service_principal: self.service_principal.clone(),
        }
    }
}

/// One persisted connector to a third-party cloud service provider.
#[derive(Debug, Clone)]
pub struct ConnectorEntry {
    /// The connector's name. `PutConnector` takes no name, so Config derives it from
    /// the configuration the connector was created with ([`connector_name`]).
    pub name: String,
    pub arn: String,
    /// The Region this connector belongs to — the Region of the request that created
    /// it, and the Region segment of [`Self::arn`]. A connector is a REGIONAL
    /// resource, so every operation serves only the connectors of the Region its
    /// request is scoped to.
    pub region: String,
    /// The `Provider` this connector's configuration names.
    pub provider: String,
    /// The provider-specific configuration the connector was created with. Connectors
    /// cannot be updated (`PutConnector`: "To update the connector configuration, you
    /// must delete all associated configuration recorders, delete the connector, and
    /// recreate it"), so this never changes once stored.
    pub connector_configuration: ConnectorConfiguration,
    pub created_at: SystemTime,
    /// Tags are taken at creation (`PutConnector`: "Tags are added at creation and
    /// cannot be updated with this operation").
    pub tags: Vec<Tag>,
}

impl ConnectorEntry {
    /// The identifier of the tenant this connector's configuration names, which is
    /// the identifier of the provider configuration it carries.
    pub fn tenant_identifier(&self) -> &str {
        self.connector_configuration
            .azure
            .as_ref()
            .map(|azure| azure.tenant_identifier.as_str())
            .unwrap_or_default()
    }

    /// The modelled `Connector`, read back out of state.
    pub fn to_shape(&self) -> Connector {
        Connector {
            name: self.name.clone(),
            arn: self.arn.clone(),
            connector_configuration: self.connector_configuration.clone(),
            created_time: timestamp(self.created_at),
        }
    }

    /// The modelled `ConnectorSummary` a listing reports for this connector.
    pub fn to_summary(&self) -> ConnectorSummary {
        ConnectorSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            provider: self.provider.clone(),
            tenant_identifier: self.tenant_identifier().to_string(),
            created_time: timestamp(self.created_at),
        }
    }
}

/// One persisted remediation configuration: the remediation action Config runs for
/// one Config rule.
///
/// Config holds at most one remediation configuration per Config rule — a second
/// `PutRemediationConfigurations` for the same rule UPDATES the first, and
/// `DeleteRemediationConfiguration` names only the rule — so the store keys them by
/// `ConfigRuleName`.
#[derive(Debug, Clone, Default)]
pub struct RemediationConfigurationEntry {
    pub config_rule_name: String,
    /// The id Config assigned this remediation configuration when it was created. It
    /// is part of the configuration's ARN and never changes once minted, so an update
    /// keeps the ARN the create reported.
    pub id: String,
    pub arn: String,
    pub target_type: String,
    pub target_id: String,
    pub target_version: Option<String>,
    /// The remediation parameters, keyed by parameter name. A `BTreeMap` so the
    /// stored configuration has one stable ordering however the request spelled it.
    pub parameters: BTreeMap<String, RemediationParameterValue>,
    pub resource_type: Option<String>,
    pub automatic: bool,
    pub execution_controls: Option<ExecutionControls>,
    pub maximum_automatic_attempts: Option<i32>,
    pub retry_attempt_seconds: Option<i64>,
}

impl RemediationConfigurationEntry {
    /// The remediation configuration `requested` creates, carrying the id and ARN
    /// Config assigned it.
    pub fn new(id: String, arn: String, requested: &RemediationConfiguration) -> Self {
        let mut entry = Self {
            config_rule_name: requested.config_rule_name.trim().to_string(),
            id,
            arn,
            ..Self::default()
        };
        entry.update_from(requested);
        entry
    }

    /// Apply `requested` to a remediation configuration that already exists, which
    /// keeps the id and ARN minted when it was created.
    pub fn update_from(&mut self, requested: &RemediationConfiguration) {
        self.target_type = requested.target_type.clone();
        self.target_id = requested.target_id.clone();
        self.target_version = requested.target_version.clone();
        self.parameters = requested
            .parameters
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        self.resource_type = requested.resource_type.clone();
        self.automatic = requested.automatic;
        self.execution_controls = requested.execution_controls.clone();
        // Both retry members govern AUTO-remediation, so Config's documented defaults
        // apply to a configuration that remediates automatically; a manual
        // configuration carries only what the request named.
        self.maximum_automatic_attempts = requested
            .maximum_automatic_attempts
            .or_else(|| requested.automatic.then_some(DEFAULT_MAXIMUM_AUTOMATIC_ATTEMPTS));
        self.retry_attempt_seconds = requested
            .retry_attempt_seconds
            .or_else(|| requested.automatic.then_some(DEFAULT_RETRY_ATTEMPT_SECONDS));
    }

    /// The modelled `RemediationConfiguration`, read back out of state.
    ///
    /// `CreatedByService` is reported ABSENT: it names the service that owns a
    /// service-linked rule, and `PutRemediationConfigurations` "does not support
    /// adding remediation configurations for service-linked Config Rules", so a
    /// configuration created through that API belongs to the account, not a service.
    pub fn to_shape(&self) -> RemediationConfiguration {
        RemediationConfiguration {
            config_rule_name: self.config_rule_name.clone(),
            target_type: self.target_type.clone(),
            target_id: self.target_id.clone(),
            target_version: self.target_version.clone(),
            parameters: if self.parameters.is_empty() {
                None
            } else {
                Some(
                    self.parameters
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect(),
                )
            },
            resource_type: self.resource_type.clone(),
            automatic: self.automatic,
            execution_controls: self.execution_controls.clone(),
            maximum_automatic_attempts: self.maximum_automatic_attempts,
            retry_attempt_seconds: self.retry_attempt_seconds,
            arn: Some(self.arn.clone()),
            created_by_service: None,
        }
    }
}

/// What identifies ONE remediation exception: the Config rule it exempts a resource
/// from, and the resource it exempts — the resource type and the resource id, which is
/// what `RemediationExceptionResourceKey` names a resource by.
pub type RemediationExceptionKey = (String, String, String);

/// The most remediation exceptions ONE Config rule holds
/// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::put_remediation_exception`]
/// enforces it).
///
/// Config publishes no quota for them, and one call adds one exception per resource key
/// it names, at most a hundred of them (`PutRemediationExceptionsRequest$ResourceKeys`,
/// length 1..100), so one call's worth is what one rule holds. Without a bound,
/// repeated calls naming new resources would grow this process's memory without end.
pub const REMEDIATION_EXCEPTIONS_PER_RULE: usize = 100;

/// One persisted remediation exception: one resource Config does not remediate under
/// one rule ("An object that represents the details about the remediation exception …
/// the rule name, an explanation of an exception, the time when the exception will be
/// deleted, the resource ID, and resource type").
#[derive(Debug, Clone, Default)]
pub struct RemediationExceptionEntry {
    pub config_rule_name: String,
    pub resource_type: String,
    pub resource_id: String,
    /// "An explanation of an remediation exception" (`RemediationException$Message`),
    /// optional because the request that creates the exception may carry none.
    pub message: Option<String>,
    /// When set, the instant this exception falls due for deletion
    /// (`RemediationException$ExpirationTime`: "The time when the remediation
    /// exception will be deleted"). An exception the clock has already taken past that
    /// instant is gone, which is what [`Self::expired_by`] decides.
    pub expires_at: Option<SystemTime>,
}

impl RemediationExceptionEntry {
    /// What identifies this exception in the store.
    pub fn key(&self) -> RemediationExceptionKey {
        (
            self.config_rule_name.clone(),
            self.resource_type.clone(),
            self.resource_id.clone(),
        )
    }

    /// Whether the clock has already taken this exception past the instant it falls due
    /// for deletion, so it exempts nothing any more.
    pub fn expired_by(&self, now: SystemTime) -> bool {
        self.expires_at.is_some_and(|due| now >= due)
    }

    /// The modelled `RemediationException`, read back out of state.
    pub fn to_shape(&self) -> RemediationException {
        RemediationException {
            config_rule_name: self.config_rule_name.clone(),
            resource_type: self.resource_type.clone(),
            resource_id: self.resource_id.clone(),
            message: self.message.clone(),
            expiration_time: self.expires_at.map(timestamp),
        }
    }
}

/// One state of a resource's configuration that the resource has since LEFT, as the
/// configuration item Config recorded for it.
///
/// A resource's identity (its type, id and name) does not change between its
/// configuration items, so only what a new item CHANGES is kept here; the rest is read
/// off the [`DiscoveredResourceEntry`] holding it.
#[derive(Debug, Clone, Default)]
pub struct RecordedConfigurationState {
    /// The `Configuration` document of that item.
    pub configuration: Option<String>,
    /// The `supplementaryConfiguration` blocks of that item.
    pub supplementary_configuration: BTreeMap<String, String>,
    /// The tags recorded with that item.
    pub tags: BTreeMap<String, String>,
    /// The relationships that item reported — the other resources the owning service
    /// linked this one to at the instant it was captured, which a link that appeared or
    /// went away is a change of.
    pub relationships: Vec<RecordedRelationship>,
    /// When that item was captured.
    pub recorded_at: Option<SystemTime>,
}

/// `RemediationExecutionState` values
/// (`com.amazonaws.configservice#RemediationExecutionState`), which are the states one
/// remediation execution moves through.
pub const REMEDIATION_EXECUTION_STATE_QUEUED: &str = "QUEUED";
/// See [`REMEDIATION_EXECUTION_STATE_QUEUED`].
pub const REMEDIATION_EXECUTION_STATE_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`REMEDIATION_EXECUTION_STATE_QUEUED`].
pub const REMEDIATION_EXECUTION_STATE_SUCCEEDED: &str = "SUCCEEDED";

/// `RemediationExecutionStepState` values
/// (`com.amazonaws.configservice#RemediationExecutionStepState`), which are the states
/// one STEP of an execution moves through.
pub const REMEDIATION_EXECUTION_STEP_STATE_PENDING: &str = "PENDING";
/// See [`REMEDIATION_EXECUTION_STEP_STATE_PENDING`].
pub const REMEDIATION_EXECUTION_STEP_STATE_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`REMEDIATION_EXECUTION_STEP_STATE_PENDING`].
pub const REMEDIATION_EXECUTION_STEP_STATE_SUCCEEDED: &str = "SUCCEEDED";

/// How long after the call that started it a remediation execution is due to leave
/// `QUEUED` and start running, and how long after that call it is due to finish.
///
/// Both moves are ones the CLOCK owes rather than ones the call makes:
/// "[r]emediation execution is asynchronous" (`StartRemediationExecution`), so the call
/// answers before the target has run. AWS documents no duration for either, so the two
/// spans are CHOSEN inside the 250–750 ms window this project uses for such a
/// transition (see [`RECORDER_STATUS_SETTLE_AFTER`]), which keeps both the queued and
/// the running state observable to a caller that reads the status straight after
/// starting the execution.
pub const REMEDIATION_EXECUTION_STARTS_AFTER: Duration = Duration::from_millis(250);
/// See [`REMEDIATION_EXECUTION_STARTS_AFTER`].
pub const REMEDIATION_EXECUTION_COMPLETES_AFTER: Duration = Duration::from_millis(500);

/// What identifies one remediation execution in the store: the rule it remediates and
/// the resource it ran for — the `(ConfigRuleName, resourceType, resourceId)` triple,
/// which is what `DescribeRemediationExecutionStatus` looks one up by.
///
/// Ordering the triple gives the store a stable total order, so a page of execution
/// statuses can resume at a key.
pub type RemediationExecutionKey = (String, String, String);

/// The most remediation executions the store keeps
/// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::record_remediation_execution`]
/// enforces it).
///
/// Config publishes no quota for them, and one call starts at most a hundred
/// (`StartRemediationExecutionRequest$ResourceKeys`, length 1..100), so one call's
/// worth is what the store keeps: the hundred most recently invoked. Without a bound,
/// repeated calls naming new resources would grow this process's memory without end.
pub const REMEDIATION_EXECUTIONS_KEPT: usize = 100;

/// One remediation execution: the run of a rule's remediation action against ONE
/// resource, as `DescribeRemediationExecutionStatus` reports it.
///
/// The execution is asynchronous, so what is stored is when it was invoked and the
/// instants its two moves are due at; the state it is in is READ OFF the clock
/// ([`Self::state_at`]) rather than written by whoever reads it.
#[derive(Debug, Clone)]
pub struct RemediationExecutionEntry {
    /// The type of the remediated resource (`ResourceKey$resourceType`).
    pub resource_type: String,
    /// The id of the remediated resource (`ResourceKey$resourceId`).
    pub resource_id: String,
    /// The remediation target this execution runs — the automation document the rule's
    /// remediation configuration names (`RemediationConfiguration$TargetId`) — which is
    /// the one step the execution reports (`RemediationExecutionStep$Name`).
    pub step_name: String,
    /// When Config invoked the execution
    /// (`RemediationExecutionStatus$InvocationTime`), which is the instant of the call
    /// that started it.
    pub invoked_at: SystemTime,
    /// When the execution is due to leave `QUEUED`; see
    /// [`REMEDIATION_EXECUTION_STARTS_AFTER`].
    pub starts_at: SystemTime,
    /// When the execution is due to have finished; see
    /// [`REMEDIATION_EXECUTION_COMPLETES_AFTER`].
    pub completes_at: SystemTime,
    /// When this execution's own RUN of its action began and ended, where this emulator
    /// performed that action (`crate::state::remediation`).
    ///
    /// The run is what the execution reports the progress of, so where it happened these
    /// are the instants the reported state moves at — an action performed in process
    /// takes the time that call takes rather than the spans above. Both stay absent for
    /// an execution whose runbook names an action this emulator does not perform, and for
    /// one running where no pump could run it, and the reported state then follows the
    /// clock's own spans.
    pub ran_from: Option<SystemTime>,
    /// See [`Self::ran_from`].
    pub ran_to: Option<SystemTime>,
}

impl RemediationExecutionEntry {
    /// The execution `StartRemediationExecution` invoked at `at` for the resource
    /// `resource_type`/`resource_id`, running the target `step_name`.
    pub fn invoked(
        resource_type: &str,
        resource_id: &str,
        step_name: &str,
        at: SystemTime,
    ) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            step_name: step_name.to_string(),
            invoked_at: at,
            starts_at: at + REMEDIATION_EXECUTION_STARTS_AFTER,
            completes_at: at + REMEDIATION_EXECUTION_COMPLETES_AFTER,
            ran_from: None,
            ran_to: None,
        }
    }

    /// The key this execution is stored under within the rule `config_rule_name`.
    pub fn key(&self, config_rule_name: &str) -> RemediationExecutionKey {
        (
            config_rule_name.to_string(),
            self.resource_type.clone(),
            self.resource_id.clone(),
        )
    }

    /// The `RemediationExecutionState` this execution is in at `now`.
    ///
    /// Where this emulator RAN the execution's action ([`Self::ran_from`]), the run is
    /// what the state reports: running from the instant the action began, succeeded from
    /// the instant it was performed — so a client that reads the status after the action
    /// has changed the resource is not told the execution is still queued. Everything
    /// else follows the clock: queued until the instant it is due to start, running until
    /// the instant it is due to have finished, and succeeded from then on.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.ran_to.is_some_and(|ran_to| now >= ran_to) {
            return REMEDIATION_EXECUTION_STATE_SUCCEEDED;
        }
        if self.ran_from.is_some_and(|ran_from| now >= ran_from) {
            return REMEDIATION_EXECUTION_STATE_IN_PROGRESS;
        }
        if now < self.starts_at {
            REMEDIATION_EXECUTION_STATE_QUEUED
        } else if now < self.completes_at {
            REMEDIATION_EXECUTION_STATE_IN_PROGRESS
        } else {
            REMEDIATION_EXECUTION_STATE_SUCCEEDED
        }
    }

    /// When this execution last moved (`RemediationExecutionStatus$LastUpdatedTime`):
    /// the instant of the most recent transition — the instant its own run moved it where
    /// this emulator ran it, and the instant the clock made the move due otherwise, which
    /// is the invocation itself while the execution is still queued.
    pub fn last_updated_at(&self, now: SystemTime) -> SystemTime {
        match self.state_at(now) {
            REMEDIATION_EXECUTION_STATE_QUEUED => self.invoked_at,
            REMEDIATION_EXECUTION_STATE_IN_PROGRESS => {
                self.ran_from.unwrap_or(self.starts_at)
            }
            _ => self.ran_to.unwrap_or(self.completes_at),
        }
    }

    /// The modelled `RemediationExecutionStatus`, read back out of state at `now`.
    ///
    /// The one step is the target the remediation configuration names: it is pending
    /// while the execution is queued, running while the execution is, and succeeded
    /// once it has finished — and it carries no `ErrorMessage`, because nothing has
    /// failed.
    pub fn to_shape(&self, now: SystemTime) -> RemediationExecutionStatus {
        let state = self.state_at(now);
        let step_state = match state {
            REMEDIATION_EXECUTION_STATE_QUEUED => REMEDIATION_EXECUTION_STEP_STATE_PENDING,
            REMEDIATION_EXECUTION_STATE_IN_PROGRESS => {
                REMEDIATION_EXECUTION_STEP_STATE_IN_PROGRESS
            }
            _ => REMEDIATION_EXECUTION_STEP_STATE_SUCCEEDED,
        };
        RemediationExecutionStatus {
            resource_key: Some(ResourceKey {
                resource_type: self.resource_type.clone(),
                resource_id: self.resource_id.clone(),
            }),
            state: Some(state.to_string()),
            step_details: Some(vec![RemediationExecutionStep {
                name: Some(self.step_name.clone()),
                state: Some(step_state.to_string()),
                error_message: None,
                // A step that has not started has no start instant to report, and one
                // that has not finished no stop instant.
                start_time: (state != REMEDIATION_EXECUTION_STATE_QUEUED)
                    .then(|| timestamp(self.ran_from.unwrap_or(self.starts_at))),
                stop_time: (state == REMEDIATION_EXECUTION_STATE_SUCCEEDED)
                    .then(|| timestamp(self.ran_to.unwrap_or(self.completes_at))),
            }]),
            invocation_time: Some(timestamp(self.invoked_at)),
            last_updated_time: Some(timestamp(self.last_updated_at(now))),
        }
    }
}

/// One persisted retention configuration: the name Config gave the object and the
/// number of days it keeps historical information for.
#[derive(Debug, Clone, Default)]
pub struct RetentionConfigurationEntry {
    pub name: String,
    pub retention_period_in_days: i32,
}

impl RetentionConfigurationEntry {
    /// The modelled `RetentionConfiguration`, read back out of state.
    pub fn to_shape(&self) -> RetentionConfiguration {
        RetentionConfiguration {
            name: self.name.clone(),
            retention_period_in_days: self.retention_period_in_days,
        }
    }
}

/// One pending aggregation request this account holds.
///
/// A pending aggregation request is an aggregator account that has asked to aggregate
/// this account's data and has not been authorized yet
/// (`PendingAggregationRequest`: "An object that represents the account ID and region
/// of an aggregator account that is requesting authorization but is not yet
/// authorized"). The REQUESTER is therefore another account's aggregator, and the
/// call that creates the entry — that account's `PutConfigurationAggregator`, naming
/// this account as a source — reaches that account's Config, not this one: one
/// engine's store stands for ONE account in ONE Region, so no operation served here
/// mints one. `DeletePendingAggregationRequest` is the modelled operation that
/// removes one, and `DescribePendingAggregationRequests` reports what the store
/// holds, which for an account nobody has asked to aggregate is nothing.
#[derive(Debug, Clone, Default)]
pub struct PendingAggregationRequestEntry {
    /// The 12-digit account id of the account requesting to aggregate data
    /// (`PendingAggregationRequest$RequesterAccountId`).
    pub requester_account_id: String,
    /// The Region requesting to aggregate data
    /// (`PendingAggregationRequest$RequesterAwsRegion`).
    pub requester_aws_region: String,
}

impl PendingAggregationRequestEntry {
    /// A pending request from the aggregator account `requester_account_id` in
    /// `requester_aws_region`.
    pub fn new(requester_account_id: &str, requester_aws_region: &str) -> Self {
        Self {
            requester_account_id: requester_account_id.to_string(),
            requester_aws_region: requester_aws_region.to_string(),
        }
    }

    /// The key this entry is stored under: the (account, Region) pair that identifies
    /// the requesting aggregator, which is exactly what
    /// `DeletePendingAggregationRequest` names.
    pub fn key(&self) -> (String, String) {
        (
            self.requester_account_id.clone(),
            self.requester_aws_region.clone(),
        )
    }

    /// The modelled `PendingAggregationRequest`, read back out of state.
    pub fn to_shape(&self) -> PendingAggregationRequest {
        PendingAggregationRequest {
            requester_account_id: Some(self.requester_account_id.clone()),
            requester_aws_region: Some(self.requester_aws_region.clone()),
        }
    }
}

/// The sources one configuration aggregator takes in
/// ([`ConfigurationAggregatorEntry::aggregated_sources`]): every (source id, Region)
/// pair it holds a sync relationship for is one source id of [`Self::source_ids`]
/// collected from one Region of [`Self::aws_regions`].
///
/// The pairs are held as the two lists rather than as the pairs themselves, so a read
/// that serves one page of them walks the lists and builds only that page: neither the
/// account list nor the Region list of an aggregation source is bounded by the model,
/// and their PRODUCT is what a caller would otherwise have materialised by one call.
#[derive(Debug, Clone)]
pub struct AggregatedSources<'a> {
    /// Which kind of source the ids name ([`AGGREGATED_SOURCE_TYPE_ACCOUNT`] or
    /// [`AGGREGATED_SOURCE_TYPE_ORGANIZATION`]).
    pub source_type: &'static str,
    /// The `LastUpdateStatus` every one of these relationships is in, which is the one
    /// [`ConfigurationAggregatorEntry::source_status`] reports for each of them. A
    /// status filter is matched against THIS, once, so a filter that names no status
    /// these sources are in is answered without walking a single pair.
    pub last_update_status: &'static str,
    /// The sources, sorted and without repeats.
    pub source_ids: Vec<&'a str>,
    /// The Regions each source is collected from, sorted and without repeats.
    pub aws_regions: Vec<&'a str>,
}

/// `values` sorted and without repeats: one entry per distinct value, in the one
/// ordering a paged read can resume in.
fn sorted_unique(mut values: Vec<&str>) -> Vec<&str> {
    values.sort_unstable();
    values.dedup();
    values
}

/// The Regions one aggregation source aggregates: the ones it LISTS, or — when it
/// aggregates every Region (`AllAwsRegions`) — `aggregator_region`.
///
/// An aggregation source names its Regions one of those two ways, which is what
/// `PutConfigurationAggregator` holds it to. A source that names every Region takes in
/// whichever Regions hold Config data, and one engine's store stands for ONE account
/// in ONE Region, so the single Region it resolves to here is the aggregator's own.
fn aggregated_regions<'a>(
    all_aws_regions: bool,
    aws_regions: Option<&'a [String]>,
    aggregator_region: &'a str,
) -> Vec<&'a str> {
    let listed: Vec<&str> = aws_regions
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .collect();
    if listed.is_empty() && all_aws_regions {
        return vec![aggregator_region];
    }
    listed
}

/// The key one resource evaluation is listed at: the instant its execution started,
/// MOST RECENT FIRST, with the id breaking ties so the ordering is total and one page
/// token names exactly one evaluation.
pub type ResourceEvaluationPageKey = (::std::cmp::Reverse<i128>, String);

/// One resource evaluation `StartResourceEvaluation` started.
///
/// The evaluation's OUTCOME is not part of this record: deciding whether the resource
/// details comply means running every proactive Config rule against them, which this
/// emulator has no rule engine for — the same reason
/// [`ConfigRuleEntry::evaluation_results`] holds only the results something recorded.
/// `GetResourceEvaluationSummary` therefore DERIVES the status and the compliance it
/// reports from this record, the rules in state and the clock (see
/// [`resource_evaluation`]), and an execution that would have to run a rule's
/// implementation is reported as the modelled failure rather than as a synthesised
/// verdict. What is persisted here is what the control plane owns and what the list
/// path reports: the id Config minted, the mode the evaluation runs in, when its
/// execution started, the deployment context it belongs to, the resource details it
/// was handed and the timeout it runs under.
#[derive(Debug, Clone)]
pub struct ResourceEvaluationEntry {
    /// The `ResourceEvaluationId` Config minted for this execution.
    pub id: String,
    pub evaluation_mode: String,
    pub started_at: SystemTime,
    /// The `EvaluationContext$EvaluationContextIdentifier` the request carried, which
    /// is what `ResourceEvaluationFilters$EvaluationContextIdentifier` filters on.
    pub evaluation_context_identifier: Option<String>,
    pub resource_details: ResourceDetails,
    pub evaluation_timeout_seconds: i32,
    /// The `ClientToken` this evaluation was started with, if any; the token index
    /// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::resource_evaluation_tokens`])
    /// points back at this entry.
    pub client_token: Option<String>,
}

impl ResourceEvaluationEntry {
    /// The modelled `ResourceEvaluation`, read back out of state.
    ///
    /// A LISTED evaluation carries its identity and the instant its execution started.
    /// `ResourceEvaluation$EvaluationMode` is modelled but NOT populated here: the
    /// recorded `ListResourceEvaluations` entries carry only `ResourceEvaluationId` and
    /// `EvaluationStartTimestamp`, with the mode living in the request's
    /// `ResourceEvaluationFilters` vocabulary rather than in the listed item. The mode
    /// the execution runs in is persisted all the same
    /// ([`ResourceEvaluationEntry::evaluation_mode`]) — the list path filters on it,
    /// and `GetResourceEvaluationSummary` reports it, where the model and the
    /// recordings agree it is populated.
    pub fn to_shape(&self) -> ResourceEvaluation {
        ResourceEvaluation {
            resource_evaluation_id: Some(self.id.clone()),
            evaluation_mode: None,
            evaluation_start_timestamp: Some(timestamp(self.started_at)),
        }
    }

    /// The instant this evaluation's execution started, as nanoseconds since the Unix
    /// epoch.
    pub fn started_at_nanos(&self) -> i128 {
        epoch_nanos(timestamp(self.started_at))
    }

    /// See [`ResourceEvaluationPageKey`].
    pub fn page_key(&self) -> ResourceEvaluationPageKey {
        (::std::cmp::Reverse(self.started_at_nanos()), self.id.clone())
    }

    /// Whether this evaluation was started with exactly the parameters `other` carries
    /// — what decides between a retry carrying the same `ClientToken` succeeding
    /// idempotently and `IdempotentParameterMismatch`.
    pub fn started_with(
        &self,
        evaluation_mode: &str,
        resource_details: &ResourceDetails,
        evaluation_context_identifier: Option<&str>,
        evaluation_timeout_seconds: i32,
    ) -> bool {
        let details = &self.resource_details;
        self.evaluation_mode == evaluation_mode
            && self.evaluation_timeout_seconds == evaluation_timeout_seconds
            && self.evaluation_context_identifier.as_deref() == evaluation_context_identifier
            && details.resource_id == resource_details.resource_id
            && details.resource_type == resource_details.resource_type
            && details.resource_configuration == resource_details.resource_configuration
            && details.resource_configuration_schema_type
                == resource_details.resource_configuration_schema_type
    }
}

/// One persisted aggregation authorization.
///
/// An authorization is identified by the PAIR it authorizes — the aggregator account
/// and the aggregator Region (`AggregationAuthorization`: "the authorizations granted
/// to aggregator accounts and regions") — which is also what its ARN carries, so the
/// pair is the key [`ComAmazonawsConfigserviceStarlingdoveserviceState::aggregation_authorizations`]
/// holds it under.
#[derive(Debug, Clone)]
pub struct AggregationAuthorizationEntry {
    pub authorized_account_id: String,
    pub authorized_aws_region: String,
    pub arn: String,
    pub created_at: SystemTime,
    /// Tags are taken at creation and not updated afterwards
    /// (`PutAggregationAuthorization`: "Tags are added at creation and cannot be
    /// updated with this operation ... tags will not be updated, even if they are
    /// different. Use TagResource and UntagResource to update tags after creation").
    pub tags: Vec<Tag>,
}

impl AggregationAuthorizationEntry {
    /// The modelled `AggregationAuthorization`, read back out of state.
    pub fn to_shape(&self) -> AggregationAuthorization {
        AggregationAuthorization {
            aggregation_authorization_arn: Some(self.arn.clone()),
            authorized_account_id: Some(self.authorized_account_id.clone()),
            authorized_aws_region: Some(self.authorized_aws_region.clone()),
            creation_time: Some(timestamp(self.created_at)),
        }
    }
}

/// One persisted stored query (`StoredQuery`).
#[derive(Debug, Clone, Default)]
pub struct StoredQueryEntry {
    /// The `QueryName` this query is keyed by: "The QueryName must be unique for a
    /// single Amazon Web Services account and a single Amazon Web Services Region".
    pub name: String,
    /// The `QueryId` Config assigned when the query was created. It is part of the
    /// query's ARN and never changes once minted, so an update in place keeps it.
    pub id: String,
    pub arn: String,
    pub description: Option<String>,
    pub expression: String,
    /// The tags the query was CREATED with (`PutStoredQuery`: "Tags are added at
    /// creation and cannot be updated ... tags will not be updated, even if they are
    /// different"), and whatever the tagging operations have since made of them: they
    /// are the one tag set of this resource, reached through
    /// [`ComAmazonawsConfigserviceStarlingdoveserviceState::tags_of`].
    pub tags: TagSet,
}

impl StoredQueryEntry {
    /// The modelled `StoredQuery`, read back out of state.
    pub fn to_shape(&self) -> StoredQuery {
        StoredQuery {
            query_id: Some(self.id.clone()),
            query_arn: Some(self.arn.clone()),
            query_name: self.name.clone(),
            description: self.description.clone(),
            expression: Some(self.expression.clone()),
        }
    }

    /// The modelled `StoredQueryMetadata`, read back out of state: the query's
    /// identity and its description, which is every member that shape declares —
    /// metadata carries no `Expression`.
    pub fn to_metadata(&self) -> StoredQueryMetadata {
        StoredQueryMetadata {
            query_id: self.id.clone(),
            query_arn: self.arn.clone(),
            query_name: self.name.clone(),
            description: self.description.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-configservice-starlingdoveservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Every configuration recorder of this account and Region, keyed by name: the
    /// customer managed one `PutConfigurationRecorder` creates and the service-linked
    /// ones `PutServiceLinkedConfigurationRecorder` and
    /// `PutThirdPartyServiceLinkedConfigurationRecorder` create, which carry a
    /// `service_principal`.
    ///
    /// BOUNDED at [`CUSTOMER_MANAGED_RECORDERS_QUOTA`] customer managed entries plus
    /// [`SERVICE_LINKED_RECORDERS_QUOTA`] service-linked ones — one engine's store
    /// stands for one account in one Region, and both per-Region quotas are enforced
    /// over the whole store, so no sequence of calls can grow this map further. A
    /// third-party service-linked recorder is named from its service principal exactly
    /// as the other service-linked ones are, so it occupies one of the same entries
    /// rather than adding a class of its own.
    pub recorders: HashMap<String, RecorderEntry>,
    /// The status record of each recorder, keyed by recorder name. Holds at most one
    /// entry per recorder in [`Self::recorders`], so the same bound holds.
    pub recorder_status: HashMap<String, RecorderStatusEntry>,
    /// Delivery channels, keyed by name. Config allows one per account per Region.
    pub delivery_channels: HashMap<String, DeliveryChannelEntry>,
    /// Config rules, keyed by `ConfigRuleName`. A `BTreeMap` so `DescribeConfigRules`
    /// pages over a stable ordering and a page token survives inserts and deletes.
    pub config_rules: BTreeMap<String, ConfigRuleEntry>,
    /// Remediation configurations, keyed by the `ConfigRuleName` they remediate —
    /// Config holds at most one per rule.
    pub remediation_configurations: BTreeMap<String, RemediationConfigurationEntry>,
    /// Remediation exceptions, keyed by [`RemediationExceptionKey`] — the rule they
    /// exempt a resource from and the resource they exempt. A `BTreeMap` so a read of
    /// them pages over a stable ordering and a page token survives exceptions added or
    /// removed between pages. Config holds at most one exception per rule and resource,
    /// so writing another for the same pair revises it in place.
    ///
    /// BOUNDED at [`REMEDIATION_EXCEPTIONS_PER_RULE`] entries per rule the account HOLDS,
    /// of which it holds at most [`CONFIG_RULES_QUOTA`], so this map holds at most the
    /// product of the two however many rule names a caller invents: write it through
    /// [`Self::put_remediation_exception`], the ONE place it grows, which refuses both.
    /// `DeleteRemediationExceptions` only REMOVES from it, as does the expiry an entry
    /// carries ([`RemediationExceptionEntry::expired_by`]).
    pub remediation_exceptions: BTreeMap<RemediationExceptionKey, RemediationExceptionEntry>,
    /// Remediation executions `StartRemediationExecution` invoked, keyed by
    /// [`RemediationExecutionKey`] — the rule they remediate and the resource they ran
    /// for. A `BTreeMap` so `DescribeRemediationExecutionStatus` pages over a stable
    /// ordering and a page token survives executions started between pages. Config
    /// reports one execution per rule and resource, so starting another for the same
    /// pair REPLACES the record of the last one.
    ///
    /// BOUNDED at [`REMEDIATION_EXECUTIONS_KEPT`] entries: write it through
    /// [`Self::record_remediation_execution`].
    pub remediation_executions: BTreeMap<RemediationExecutionKey, RemediationExecutionEntry>,
    /// Stored queries, keyed by `QueryName` — unique per account per Region, which is
    /// what one engine's store stands for. A `BTreeMap` so a list of them pages over a
    /// stable ordering and a page token survives inserts and deletes.
    ///
    /// BOUNDED at [`STORED_QUERIES_QUOTA`] entries: `PutStoredQuery` refuses a NEW
    /// query past that quota, so no sequence of calls can grow this map further.
    pub stored_queries: BTreeMap<String, StoredQueryEntry>,
    /// Connectors to third-party cloud service providers, keyed by connector ARN. The
    /// ARN carries both the Region the connector belongs to and its name, so the key
    /// is what identifies one connector of one account in one Region. A `BTreeMap` so
    /// `ListConnectors` pages over a stable ordering — which, within one Region, is
    /// the connectors' name ordering — and a page token survives connectors created
    /// and deleted between pages.
    pub connectors: BTreeMap<String, ConnectorEntry>,
    /// Resource evaluations started by `StartResourceEvaluation`, keyed by
    /// `ResourceEvaluationId`. A `BTreeMap` so the store's iteration order is stable;
    /// `ListResourceEvaluations` reports them in [`ResourceEvaluationPageKey`] order.
    ///
    /// BOUNDED at [`RESOURCE_EVALUATIONS_KEPT`] entries: write it through
    /// [`Self::record_resource_evaluation`], which drops the oldest evaluation past
    /// that bound, so repeated starts cannot grow this process's state without end.
    pub resource_evaluations: BTreeMap<String, ResourceEvaluationEntry>,
    /// The `ResourceEvaluationId` each `ClientToken` already started an evaluation
    /// for, so a retry with the same token is idempotent rather than a second
    /// execution.
    ///
    /// Holds at most one entry per evaluation in [`Self::resource_evaluations`], so it
    /// is bounded by the same [`RESOURCE_EVALUATIONS_KEPT`]: dropping an evaluation
    /// drops the token that started it, and a caller varying only the token cannot
    /// grow this index on its own.
    pub resource_evaluation_tokens: HashMap<String, String>,
    /// The resources Config has discovered in this account and Region, keyed by
    /// `(resourceType, resourceId)` — the pair that identifies a resource in Config,
    /// which is why a resource read takes both. A `BTreeMap` so a discovered-resource
    /// read pages over a stable ordering (by type, then by resource id) and a page
    /// token survives resources recorded or deleted between pages.
    ///
    /// A deleted resource stays in the inventory carrying its deletion time, because a
    /// discovered-resource read can be asked to include deleted resources.
    ///
    /// `PutResourceConfig` is what WRITES the inventory, recording the configuration
    /// state of one custom resource, and `DeleteResourceConfig` records that
    /// resource's deletion onto the same entry, so an account that has recorded
    /// nothing has discovered nothing and a discovered-resource read reports an empty
    /// page for every type.
    ///
    /// BOUNDED at [`DISCOVERED_RESOURCES_KEPT`] entries: write it through
    /// [`Self::record_discovered_resource`], which drops the oldest configuration item
    /// past that bound — a resource whose deletion the inventory already recorded first
    /// — so no sequence of writes can grow this process's state without end. Each entry
    /// is bounded in turn by what `PutResourceConfig` records: a configuration document
    /// of at most
    /// [`crate::ops::put_resource_config::MAX_RESOURCE_CONFIGURATION_BYTES`] and at most
    /// [`TAGS_PER_RESOURCE_QUOTA`] tags.
    pub discovered_resources: BTreeMap<(String, String), DiscoveredResourceEntry>,
    /// Retention configurations, keyed by `RetentionConfigurationName`. A `BTreeMap`
    /// so `DescribeRetentionConfigurations` pages over a stable ordering; it holds at
    /// most [`RETENTION_CONFIGURATIONS_QUOTA`] entries, because one engine's store
    /// stands for one account in one Region.
    pub retention_configurations: BTreeMap<String, RetentionConfigurationEntry>,
    /// Aggregator accounts that have asked to aggregate this account's data and are
    /// not authorized yet, keyed by the (requester account, requester Region) pair
    /// that identifies one. A `BTreeMap` so `DescribePendingAggregationRequests`
    /// pages over a stable ordering and a page token survives entries added or
    /// removed between pages. See [`PendingAggregationRequestEntry`] for which call
    /// creates one.
    pub pending_aggregation_requests: BTreeMap<(String, String), PendingAggregationRequestEntry>,
    /// Configuration aggregators, keyed by `ConfigurationAggregatorName`. A
    /// `BTreeMap` so `DescribeConfigurationAggregators` pages over a stable ordering
    /// and a page token survives aggregators created and deleted between pages.
    pub configuration_aggregators: BTreeMap<String, ConfigurationAggregatorEntry>,
    /// Aggregation authorizations, keyed by
    /// [`aggregation_authorization_key`]. A `BTreeMap` so
    /// `DescribeAggregationAuthorizations` pages over a stable ordering and a page
    /// token survives authorizations granted and deleted between pages.
    pub aggregation_authorizations: BTreeMap<String, AggregationAuthorizationEntry>,
    /// Conformance packs, keyed by `ConformancePackName` — unique per account per
    /// Region, which is what one engine's store stands for. A `BTreeMap` so a status
    /// read pages over a stable ordering (the packs' name ordering) and a page token
    /// survives packs created and deleted between pages.
    ///
    /// BOUNDED at [`CONFORMANCE_PACKS_QUOTA`] entries: `PutConformancePack` refuses a
    /// NEW pack past that quota and an existing name is updated in place, so no
    /// sequence of calls can grow this map further.
    pub conformance_packs: BTreeMap<String, ConformancePackEntry>,
    /// When set, the instant the next recording-and-evaluation pass of this account's
    /// running configuration recorder falls due. Written through
    /// [`Self::owe_recording_pass`] and taken by
    /// [`Self::take_due_recording_pass`]; see `state::recording` for what a pass does.
    pub recording_pass_due: Option<SystemTime>,
    /// The lease on the in-process pump that runs the passes this store owes, so one
    /// store has at most one and a pump that is no longer running them holds it no longer
    /// ([`PumpLease`], taken through [`PumpClaim`]).
    pub recording_pump: PumpLease,
    /// Whether an in-process pump is already performing the configuration-snapshot
    /// deliveries this store owes, so one store has at most one
    /// ([`Self::claim_snapshot_delivery_pump`]); see `state::snapshot_delivery` for what
    /// performing one does.
    pub snapshot_delivery_pump: bool,
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// The CUSTOMER MANAGED configuration recorders this account holds: the ones it
    /// created and is billed for. A service-linked recorder records for the Amazon Web
    /// Services service it is linked to, so it neither counts against the customer
    /// managed recorder quota nor stands in for one where an operation needs one.
    pub fn customer_managed_recorders(&self) -> impl Iterator<Item = &RecorderEntry> {
        self.recorders
            .values()
            .filter(|recorder| !recorder.is_service_linked())
    }

    /// Whether this account holds a customer managed configuration recorder.
    pub fn has_customer_managed_recorder(&self) -> bool {
        self.customer_managed_recorders().next().is_some()
    }

    /// The ARN of the customer managed configuration recorder a delivery is made FOR —
    /// the source a delivery bucket's own policy may condition its grant on
    /// ([`delivery_authorization`]) — or `None` while this account holds none.
    pub fn customer_managed_recorder_arn(&self) -> Option<String> {
        self.customer_managed_recorders()
            .next()
            .map(|recorder| recorder.arn.clone())
    }

    /// The IAM role the customer managed configuration recorder a delivery is made for
    /// RUNS AS — the role Config assumes to make the delivery at all
    /// ([`delivery_authorization`]) — or `None` while this account holds no such recorder,
    /// or holds one that names no role.
    pub fn customer_managed_recorder_role_arn(&self) -> Option<String> {
        self.customer_managed_recorders()
            .next()
            .and_then(|recorder| recorder.role_arn.clone())
    }

    /// Record `entry` as a remediation execution of the rule `config_rule_name`.
    ///
    /// This is the ONE place [`Self::remediation_executions`] GROWS, so the store
    /// cannot grow this process's state without end: past
    /// [`REMEDIATION_EXECUTIONS_KEPT`] entries it drops the execution that was invoked
    /// EARLIEST (and then the first by key, so the choice is deterministic when two
    /// were invoked in the same instant).
    pub fn record_remediation_execution(
        &mut self,
        config_rule_name: &str,
        entry: RemediationExecutionEntry,
    ) {
        self.remediation_executions
            .insert(entry.key(config_rule_name), entry);
        while self.remediation_executions.len() > REMEDIATION_EXECUTIONS_KEPT {
            let oldest = self
                .remediation_executions
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    (left.invoked_at, *left_key).cmp(&(right.invoked_at, *right_key))
                })
                .map(|(key, _)| key.clone());
            let Some(key) = oldest else {
                break;
            };
            self.remediation_executions.remove(&key);
        }
    }

    /// Record `entry` as a started resource evaluation, with the `ClientToken` that
    /// started it indexed back to it.
    ///
    /// This is the ONE place [`Self::resource_evaluations`] and
    /// [`Self::resource_evaluation_tokens`] are written, so the two cannot drift: past
    /// [`RESOURCE_EVALUATIONS_KEPT`] entries it drops the OLDEST evaluation together
    /// with the token that started it, keeping both stores bounded whatever a caller
    /// does.
    pub fn record_resource_evaluation(&mut self, entry: ResourceEvaluationEntry) {
        if let Some(token) = entry.client_token.clone() {
            self.resource_evaluation_tokens
                .insert(token, entry.id.clone());
        }
        self.resource_evaluations.insert(entry.id.clone(), entry);

        while self.resource_evaluations.len() > RESOURCE_EVALUATIONS_KEPT {
            // One insert can take the store at most one entry past the bound, so this
            // runs once; it is a loop so the bound holds however the store was filled.
            // The oldest execution, by the instant it started and then by id so the
            // choice is deterministic when two started in the same instant.
            let oldest = self
                .resource_evaluations
                .values()
                .min_by(|left, right| {
                    (left.started_at_nanos(), left.id.as_str())
                        .cmp(&(right.started_at_nanos(), right.id.as_str()))
                })
                .map(|entry| (entry.id.clone(), entry.client_token.clone()));
            let Some((id, client_token)) = oldest else {
                break;
            };
            self.resource_evaluations.remove(&id);
            if let Some(token) = client_token {
                self.resource_evaluation_tokens.remove(&token);
            }
        }
    }

    /// Record `entry` as a discovered resource, keyed by the `(resourceType,
    /// resourceId)` pair that identifies it.
    ///
    /// This is the ONE place [`Self::discovered_resources`] GROWS, so the inventory
    /// cannot grow this process's state without end: past
    /// [`DISCOVERED_RESOURCES_KEPT`] entries it drops the resource whose configuration
    /// item is oldest, taking one whose DELETION the inventory already recorded before
    /// one that is still recorded — a deleted resource is reported only by a read that
    /// asks for deleted resources, so it is what an inventory at its bound gives up
    /// first. `PutResourceConfig`'s own
    /// [`crate::ops::put_resource_config::ACTIVE_CUSTOM_RESOURCES_QUOTA`] is what a
    /// caller meets first, with the modelled `MaxActiveResourcesExceededException`; this
    /// bound is what holds whatever any future writer does.
    ///
    /// Answers the configuration item this write RECORDED — the notification the delivery
    /// channel owes for it ([`change_notification`]) is derived from that — and `None` for
    /// an observation that recorded no item at all, because the resource is still in the
    /// state the held item records. Every writer of the inventory goes through here, so
    /// each owes the notification for what it recorded and nothing owes one for what it
    /// did not.
    pub fn record_discovered_resource(
        &mut self,
        entry: DiscoveredResourceEntry,
    ) -> Option<RecordedItemChange> {
        let key = (entry.resource_type.clone(), entry.resource_id.clone());
        let mut entry = entry;
        // What KIND of configuration item this write records, read off the item the
        // inventory holds BEFORE this one replaces it: the resource's first item, an item
        // recording a state it changed into, the item recording that it is gone, or no item
        // at all.
        let held = self.discovered_resources.get(&key);
        let recorded = if entry.last_recorded_at.is_none() {
            // This write captures no item: there is no instant one was captured at, so
            // there is no item to stream either.
            None
        } else {
            match held {
                // A resource the inventory holds no captured item for is being DISCOVERED
                // by this write, whatever else it already holds about it.
                None => Some((CHANGE_TYPE_CREATE, None)),
                Some(held) if held.last_recorded_at.is_none() => {
                    Some((CHANGE_TYPE_CREATE, None))
                }
                Some(held) if !held.records_another_state(&entry) => None,
                Some(held) if entry.deleted_at.is_some() && held.deleted_at.is_none() => {
                    Some((CHANGE_TYPE_DELETE, Some(held.recorded_state())))
                }
                Some(held) => Some((CHANGE_TYPE_UPDATE, Some(held.recorded_state()))),
            }
        };
        let change = recorded.map(|(change_type, previous)| RecordedItemChange {
            resource_type: entry.resource_type.clone(),
            resource_id: entry.resource_id.clone(),
            change_type,
            previous,
        });
        // A resource the inventory already holds keeps the instant Config FIRST
        // recorded it and the states it has already left; the state it is leaving now
        // joins them when this item records a DIFFERENT state, which is what makes the
        // resource's history the states it went through rather than the writes it
        // received. An observation of the state that is ALREADY recorded records no item,
        // so it leaves the held item's capture instant where it is.
        if let Some(held) = self.discovered_resources.get(&key) {
            entry.first_recorded_at = held.first_recorded_at.or(entry.last_recorded_at);
            entry.history = held.history.clone();
            if held.records_another_state(&entry) {
                if held.last_recorded_at.is_some() {
                    entry.history.push(held.recorded_state());
                }
            } else {
                // The resource is still in the state the held item records, so this
                // observation records NO configuration item — and the held one keeps the
                // instant it was captured at. A configuration item is the state the
                // resource was in at that instant, so nothing that records no item can
                // move it: a window or a page a caller built from an instant a read
                // reported goes on selecting the item it came from, however many times
                // the same state is observed afterwards. A resource the inventory holds
                // no captured item for yet takes this observation's instant, which is the
                // first one it has.
                entry.last_recorded_at = held.last_recorded_at.or(entry.last_recorded_at);
            }
            while entry.history.len() > CONFIGURATION_ITEMS_KEPT {
                entry.history.remove(0);
            }
        }
        self.discovered_resources.insert(key, entry);

        while self.discovered_resources.len() > DISCOVERED_RESOURCES_KEPT {
            // One insert can take the inventory at most one entry past the bound, so
            // this runs once; it is a loop so the bound holds however it was filled.
            let oldest = self
                .discovered_resources
                .iter()
                .min_by(|left, right| {
                    let key = |(key, entry): &(&(String, String), &DiscoveredResourceEntry)| {
                        (
                            // A resource still recorded is given up only after every
                            // deleted one.
                            entry.deleted_at.is_none(),
                            entry.last_recorded_at,
                            (*key).clone(),
                        )
                    };
                    key(left).cmp(&key(right))
                })
                .map(|(key, _)| key.clone());
            let Some(key) = oldest else {
                break;
            };
            self.discovered_resources.remove(&key);
        }
        change
    }

    /// The configuration recorder that is RECORDING right now, if any: Config
    /// discovers resources only while a recorder is turned on.
    pub fn recording_recorder(&self) -> Option<&RecorderEntry> {
        self.recorder_status
            .values()
            .filter(|status| status.recording)
            .find_map(|status| self.recorders.get(&status.name))
    }

    /// The CUSTOMER MANAGED configuration recorder that is RECORDING right now, if
    /// any: this account's delivery channel delivers what such a recorder records,
    /// while a service-linked recorder records for the Amazon Web Services service it
    /// is linked to.
    pub fn recording_customer_managed_recorder(&self) -> Option<&RecorderEntry> {
        self.customer_managed_recorders().find(|recorder| {
            self.recorder_status
                .get(&recorder.name)
                .is_some_and(|status| status.recording)
        })
    }

    /// The `RecordingFrequency` Config records `resource_type` at: the frequency the
    /// recording recorder's recording mode OVERRIDES for that resource type, else that
    /// mode's own frequency, and continuous recording while no recorder is recording
    /// (`RecordingMode`: "Config supports Continuous recording and Daily recording",
    /// with `RecordingModeOverride` setting "a specific recording frequency ... for
    /// all supported resource types in this override").
    pub fn recording_frequency_of(&self, resource_type: &str) -> &str {
        let Some(mode) = self
            .recording_recorder()
            .and_then(|recorder| recorder.recording_mode.as_ref())
        else {
            return RECORDING_FREQUENCY_CONTINUOUS;
        };
        let overridden = mode
            .recording_mode_overrides
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|rule| {
                !rule.recording_frequency.is_empty()
                    && rule.resource_types.iter().any(|named| named == resource_type)
            })
            .map(|rule| rule.recording_frequency.as_str());
        overridden
            .or(Some(mode.recording_frequency.as_str()))
            .filter(|frequency| !frequency.is_empty())
            .unwrap_or(RECORDING_FREQUENCY_CONTINUOUS)
    }

    /// The CURRENT configuration item the aggregator `aggregator` holds for the
    /// aggregate resource identifier `identifier`, where this store holds one.
    ///
    /// The ONE lookup every aggregate configuration-item read uses, so the batch path
    /// and the single-resource path cannot disagree about what an identifier names.
    ///
    /// The identity of an aggregated resource is the source account, the source
    /// Region, the resource type and the resource id — every required member of
    /// `AggregateResourceIdentifier`. `ResourceName` is optional, so it names no
    /// resource on its own and is not part of the lookup. This store stands for ONE
    /// account in ONE Region (`account_id` in `region`), so an identifier naming any
    /// other source names data this store does not hold, and nothing is held either
    /// for a source or a resource type outside the aggregator's own scope, for a
    /// resource the inventory does not hold, or for a deleted one ("The API does not
    /// return results for deleted resources").
    pub fn aggregated_resource(
        &self,
        aggregator: &ConfigurationAggregatorEntry,
        identifier: &AggregateResourceIdentifier,
        account_id: &str,
        region: &str,
    ) -> Option<&DiscoveredResourceEntry> {
        let aggregated = aggregator
            .aggregates_source(&identifier.source_account_id, &identifier.source_region)
            && aggregator.aggregates_resource_type(&identifier.resource_type)
            && identifier.source_account_id == account_id
            && identifier.source_region == region;
        if !aggregated {
            return None;
        }
        self.current_resource(&identifier.resource_type, &identifier.resource_id)
    }

    /// The CURRENT configuration item this store holds for the resource of type
    /// `resource_type` with id `resource_id`, where it holds one.
    ///
    /// The ONE lookup every configuration-item read uses to turn a `(resourceType,
    /// resourceId)` pair into the resource's current state, so the batch read, the
    /// aggregate reads and the history read cannot disagree about what a pair names:
    /// a resource the inventory does not hold has no current configuration item, and
    /// neither has one whose deletion the inventory recorded ("The API does not return
    /// results for deleted resources").
    pub fn current_resource(
        &self,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<&DiscoveredResourceEntry> {
        self.discovered_resources
            .get(&(resource_type.to_string(), resource_id.to_string()))
            .filter(|entry| entry.deleted_at.is_none())
    }

    /// How many discovered resources Config holds of each resource type it is
    /// RECORDING, in the order `GetDiscoveredResourceCounts` reports them: descending
    /// by the number of resources (`ResourceCounts`: "Each object is listed in
    /// descending order by the number of resources"), and by resource type among the
    /// types with equally many, so the ordering — and therefore a page token — is
    /// total.
    ///
    /// A type the recording recorder does not record contributes nothing, and no type
    /// at all is reported while no recorder is recording.
    pub fn discovered_resource_counts(&self) -> Vec<(String, i64)> {
        let mut per_type: BTreeMap<&str, i64> = BTreeMap::new();
        for entry in self.discovered_resources.values() {
            // A resource Config recorded the deletion of is no longer one of the
            // resources it is recording, so it contributes to no count.
            if entry.deleted_at.is_some() {
                continue;
            }
            *per_type.entry(entry.resource_type.as_str()).or_default() += 1;
        }
        // Which recorder is recording is one question for the whole call, so resolve
        // it once rather than per resource type.
        let recording_group = self
            .recording_recorder()
            .map(|recorder| recorder.recording_group.as_ref());
        let records = |resource_type: &str| match recording_group {
            None => false,
            Some(None) => true,
            Some(Some(group)) => group_records_resource_type(group, resource_type),
        };
        let mut counts: Vec<(String, i64)> = per_type
            .into_iter()
            .filter(|(resource_type, _)| records(resource_type))
            .map(|(resource_type, count)| (resource_type.to_string(), count))
            .collect();
        counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        counts
    }

    /// Write `entry` as the remediation exception its rule holds for its resource, and
    /// answer whether it was written.
    ///
    /// Config holds at most one exception per rule and resource ("This API adds a new
    /// exception or updates an existing exception for a specified resource with a
    /// specified Config rule"), so an entry for a pair the rule already holds REVISES
    /// that one in place rather than adding another.
    ///
    /// This is the ONE place [`Self::remediation_exceptions`] GROWS, so the store cannot
    /// grow this process's state without end, and `false` reports every case it refused
    /// to grow for: an entry whose rule this account does not hold — so the exceptions
    /// held are bounded by the [`CONFIG_RULES_QUOTA`] rules it may hold, not by the rule
    /// names a caller can invent — and a NEW resource once that rule already holds
    /// [`REMEDIATION_EXCEPTIONS_PER_RULE`] of them. Revising an exception the rule
    /// already holds is always written — it adds no entry.
    pub fn put_remediation_exception(&mut self, entry: RemediationExceptionEntry) -> bool {
        if !self.config_rules.contains_key(&entry.config_rule_name) {
            return false;
        }
        let key = entry.key();
        if !self.remediation_exceptions.contains_key(&key)
            && self.remediation_exceptions_held(&entry.config_rule_name)
                >= REMEDIATION_EXCEPTIONS_PER_RULE
        {
            return false;
        }
        self.remediation_exceptions.insert(key, entry);
        true
    }

    /// How many remediation exceptions the rule `config_rule_name` holds.
    pub fn remediation_exceptions_held(&self, config_rule_name: &str) -> usize {
        self.remediation_exceptions
            .values()
            .filter(|entry| entry.config_rule_name == config_rule_name)
            .count()
    }

    /// Drop every remediation exception the clock has already taken past the instant it
    /// falls due for deletion (`RemediationException$ExpirationTime`: "The time when the
    /// remediation exception will be deleted"), and answer how many went.
    ///
    /// This is the one write a READING path may perform on this map: the deletion was
    /// owed at that instant whether or not anybody looked, so materialising it is what
    /// keeps a read from reporting an exception that no longer exempts anything.
    pub fn expire_remediation_exceptions(&mut self, now: SystemTime) -> usize {
        let expired: Vec<RemediationExceptionKey> = self
            .remediation_exceptions
            .iter()
            .filter(|(_, entry)| entry.expired_by(now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &expired {
            self.remediation_exceptions.remove(key);
        }
        expired.len()
    }

    /// The `ComplianceType` each Config rule of the conformance pack `pack` HAS BEEN
    /// PLACED AT, keyed by the name Config gave that rule when the pack deployed it, and
    /// `None` for a rule nothing has evaluated yet.
    ///
    /// A conformance pack's compliance is the compliance of the rules it deployed, so
    /// each entry is the roll-up of the evaluation results recorded for that rule
    /// ([`ConfigRuleEntry::recorded_compliance`]) — the ONE precedence this service
    /// holds, so a pack's report cannot disagree with the rule's own. A rule the pack
    /// deployed and that is no longer in [`Self::config_rules`] is not a rule of this
    /// pack any more and is left out of the map altogether.
    ///
    /// The distinction between a rule PLACED at `INSUFFICIENT_DATA` and a rule nothing
    /// has evaluated is carried here rather than flattened, because the two are different
    /// answers: the pack's rule-compliance report has a row only for a rule Config has
    /// produced a result for, while the pack's own rule COUNTS count every rule it
    /// deployed.
    ///
    /// Keyed in NAME ordering, which is what a paged report of it pages over.
    pub fn conformance_pack_rule_compliance(
        &self,
        pack: &ConformancePackEntry,
    ) -> BTreeMap<String, Option<&'static str>> {
        pack.deployed_rule_names
            .iter()
            .filter_map(|name| {
                let rule = self.config_rules.get(name)?;
                Some((name.clone(), rule.recorded_compliance()))
            })
            .collect()
    }

    /// The `ComplianceType` the conformance pack `pack` ITSELF rolls up to: the
    /// compliance of the Config rules it deployed
    /// ([`Self::conformance_pack_rule_compliance`]), under the one precedence this
    /// service rolls compliance up by ([`resource_evaluation::roll_up_compliance`]) —
    /// noncompliant if any of those rules is, compliant if at least one is and none is
    /// not, and `INSUFFICIENT_DATA` where no rule places it either way, which includes a
    /// pack whose rules nothing has evaluated and a pack whose deployment created none.
    ///
    /// It is the SAME per-rule compliance the pack's own compliance report is built
    /// from, so a count of compliant packs and a report of one pack's rules cannot
    /// disagree.
    pub fn conformance_pack_compliance(&self, pack: &ConformancePackEntry) -> &'static str {
        roll_up_compliance(
            self.conformance_pack_rule_compliance(pack)
                .into_values()
                .flatten(),
        )
    }

    /// The tags of the resource `arn` identifies, or `None` when this store holds no
    /// resource with that ARN.
    ///
    /// This is the ONE place an ARN is resolved to a tag set, so the tagging
    /// operations agree with each other and with the resource's own read path: the
    /// tags live ON the resource, which is also why deleting the resource deletes its
    /// tags (`TagResource`: "When a resource is deleted, the tags associated with that
    /// resource are deleted as well"). Every taggable resource this emulator persists
    /// is reached here; a resource type whose write operation is still scaffolded
    /// cannot be in the store at all, so its ARN resolves to `None` exactly as an
    /// ARN of a deleted resource does.
    pub fn tags_of(&self, arn: &str) -> Option<&TagSet> {
        if let Some(recorder) = self.recorders.values().find(|entry| entry.arn == arn) {
            return Some(&recorder.tags);
        }
        if let Some(rule) = self.config_rules.values().find(|entry| entry.arn == arn) {
            return Some(&rule.tags);
        }
        if let Some(aggregator) = self
            .configuration_aggregators
            .values()
            .find(|entry| entry.arn == arn)
        {
            return Some(&aggregator.tags);
        }
        if let Some(stored_query) = self.stored_queries.values().find(|entry| entry.arn == arn) {
            return Some(&stored_query.tags);
        }
        if let Some(pack) = self.conformance_packs.values().find(|entry| entry.arn == arn) {
            return Some(&pack.tags);
        }
        None
    }

    /// The tags of the resource `arn` identifies, for a request that changes them.
    /// See [`Self::tags_of`].
    pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut TagSet> {
        if let Some(recorder) = self.recorders.values_mut().find(|entry| entry.arn == arn) {
            return Some(&mut recorder.tags);
        }
        if let Some(rule) = self.config_rules.values_mut().find(|entry| entry.arn == arn) {
            return Some(&mut rule.tags);
        }
        if let Some(aggregator) = self
            .configuration_aggregators
            .values_mut()
            .find(|entry| entry.arn == arn)
        {
            return Some(&mut aggregator.tags);
        }
        if let Some(stored_query) = self
            .stored_queries
            .values_mut()
            .find(|entry| entry.arn == arn)
        {
            return Some(&mut stored_query.tags);
        }
        if let Some(pack) = self
            .conformance_packs
            .values_mut()
            .find(|entry| entry.arn == arn)
        {
            return Some(&mut pack.tags);
        }
        None
    }
}

// The part of the resource-evaluation surface the SUMMARY derives — the status an
// execution has reached and the compliance it places the resource at — lives in
// `state/resource_evaluation.rs` and is re-exported here, so a handler reaches its
// names as `crate::state::<name>`. Declared beside the store accessor, away from the
// import list, the constants and the test module where other fixes land.
pub mod resource_evaluation;
pub use resource_evaluation::*;

// The Config query language — the SQL `SELECT` command a select operation takes, and
// the configuration-item documents it is run over — lives in `state/select_query.rs`
// for the same reason as `resource_evaluation` above, and is re-exported here so a
// handler reaches its names as `crate::state::<name>`.
pub mod select_query;
pub use select_query::*;

// What a RUNNING configuration recorder does between calls — observing the resources
// of the types it records, and evaluating the rules whose scope covers them — lives in
// `state/recording.rs`, re-exported here like the modules above so a handler reaches
// its names as `crate::state::<name>`.
pub mod recording;
pub use recording::*;

// What a pass OBSERVES in IAM — this account's roles, and the account's own password
// policy — lives in `state/observed_iam.rs`, re-exported here for the same reason.
pub mod observed_iam;
pub use observed_iam::*;

// The managed rules whose subject is the ACCOUNT rather than a recorded resource live in
// `state/account_rule.rs`, re-exported here for the same reason.
pub mod account_rule;
pub use account_rule::*;

// How Config invokes the EVALUATOR of a rule it does not evaluate itself lives in
// `state/custom_lambda_rule.rs`, re-exported here for the same reason.
pub mod custom_lambda_rule;
pub use custom_lambda_rule::*;

// How a RECORDED resource is resolved to its own ARN — the one member of a
// configuration item that carries it — lives in `state/resource_arn.rs`, re-exported
// here like the modules above so a handler reaches its names as `crate::state::<name>`.
pub mod resource_arn;
pub use resource_arn::*;

// What a started remediation execution DOES to the resource it names — the action of the
// runbook its configuration names — lives in `state/remediation.rs`, re-exported here
// like the modules above so a handler reaches its names as `crate::state::<name>`.
pub mod remediation;
pub use remediation::*;

// What a SCHEDULED configuration-snapshot delivery DOES — the gzip-framed snapshot
// object it writes into the delivery channel's bucket, and the outcome of that write the
// channel's status reports — lives in `state/snapshot_delivery.rs`, re-exported here like
// the modules above so a handler reaches its names as `crate::state::<name>`.
pub mod snapshot_delivery;
pub use snapshot_delivery::*;

// Whether Config MAY deliver into the bucket a delivery channel names — what that
// bucket's own policy says about the Config service principal — lives in
// `state/delivery_authorization.rs`, re-exported here like the modules above.
pub mod delivery_authorization;
pub use delivery_authorization::*;

// What a RECORDED configuration item is STREAMED as — the change notification the delivery
// channel publishes to its Amazon SNS topic, carrying the item and the diff from the item
// it replaced — lives in `state/change_notification.rs`, re-exported here like the modules
// above so a handler reaches its names as `crate::state::<name>`.
pub mod change_notification;
pub use change_notification::*;

// Whether the TARGET a remediation configuration names EXISTS — the Systems Manager
// automation document its `TargetId` names, asked of Systems Manager itself — lives in
// `state/remediation_target.rs`, re-exported here like the modules above so a handler
// reaches its names as `crate::state::<name>`.
pub mod remediation_target;
pub use remediation_target::*;

// The CLAIM an in-process pump holds while it runs the work this store owes — the lease
// that keeps a pump which is no longer running from wedging that work — lives in
// `state/pump_lease.rs`, re-exported here like the modules above so a handler reaches its
// names as `crate::state::<name>`.
pub mod pump_lease;
pub use pump_lease::*;

// Which resource type NAMES a discovered-resource listing can be asked about — the types
// this model enumerates and the CloudFormation registry type names a custom resource is
// recorded under — lives in `state/resource_type.rs`, re-exported here like the modules
// above.
pub mod resource_type;
pub use resource_type::*;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConfigserviceStarlingdoveserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConfigserviceStarlingdoveserviceState>)
}

/// The region this request is scoped to: the region of its SigV4 credential scope,
/// or [`DEFAULT_REGION`] when the request carries none.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN resource type of a configuration recorder, as Config spells it in the
/// ARNs it mints.
pub const RECORDER_ARN_RESOURCE_TYPE: &str = "configuration-recorder";

/// The ARN of a configuration recorder:
/// `arn:aws:config:<region>:<account>:configuration-recorder/<name>/<id>`. Both the
/// recorder's name and the id Config assigned it are part of the ARN.
pub fn recorder_arn(region: &str, name: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RECORDER_ARN_RESOURCE_TYPE}/{name}/{id}"),
    )
}

/// How many base-36 characters a configuration recorder's assigned id carries.
pub const RECORDER_ID_CHARS: usize = 16;

/// The id Config assigns a configuration recorder created in this emulator: sixteen
/// opaque base-36 characters, DRAWN ([`opaque_id`]) rather than derived from how many
/// recorders have been created, because an id a caller holds must tell it nothing
/// about any other.
pub fn mint_recorder_id() -> String {
    opaque_id(RECORDER_ID_CHARS)
}

/// The ARN resource type of a stored query, as Config spells it in the ARNs it mints
/// (`QueryArn`'s pattern: `stored-query/<name>/query-<id>`).
pub const STORED_QUERY_ARN_RESOURCE_TYPE: &str = "stored-query";

/// The ARN of a stored query:
/// `arn:aws:config:<region>:<account>:stored-query/<name>/query-<id>`, the form
/// `QueryArn`'s pattern requires — both the query's name and the id Config assigned it
/// are part of the ARN.
pub fn stored_query_arn(region: &str, name: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{STORED_QUERY_ARN_RESOURCE_TYPE}/{name}/query-{id}"),
    )
}

/// The `QueryId` Config assigns a stored query created in this emulator: an opaque
/// 32-character hexadecimal token ([`uuid_id`]), which sits inside `QueryId`'s 1..36
/// length, satisfies its `^\S+$` pattern and is admitted by the `query-<id>` segment
/// of `QueryArn`'s own pattern (`[a-zA-Z\d-_/]+`).
///
/// DRAWN rather than derived from how many queries have been created: a caller holding
/// one query's id learns nothing about another's.
pub fn mint_stored_query_id() -> String {
    uuid_id()
}

/// Whether `name` is a `QueryName` the model allows: 1..64 characters of
/// `^[a-zA-Z0-9-_]+$` (`StoredQuery$QueryName`). A listing resumes at a query NAME, so
/// this is what tells a `NextToken` this operation issued from a value it did not.
pub fn is_stored_query_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= STORED_QUERY_NAME_MAX_LENGTH
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
}

/// The published maximum length of a `QueryName` (`StoredQuery$QueryName`, length
/// 1..64).
pub const STORED_QUERY_NAME_MAX_LENGTH: usize = 64;

/// How Config words `ResourceNotFoundException` for a request that named a stored
/// query this account does not hold.
///
/// The message is BUILT FROM THE REQUEST — it quotes the `QueryName` the caller sent —
/// so every operation that refuses an absent stored query words it the same way.
pub fn no_such_stored_query_message(name: &str) -> String {
    format!("The query '{name}' provided in the request is invalid. Please check the query name.")
}

/// The ARN resource type of a conformance pack, as Config spells it in the ARNs it
/// mints (`ConformancePackArn`, e.g.
/// `arn:aws:config:<region>:<account>:conformance-pack/<name>/<id>`).
pub const CONFORMANCE_PACK_ARN_RESOURCE_TYPE: &str = "conformance-pack";

/// The ARN of a conformance pack: both the pack's name and the id Config assigned it
/// are part of the ARN, the way a configuration recorder's and a stored query's are.
pub fn conformance_pack_arn(region: &str, name: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONFORMANCE_PACK_ARN_RESOURCE_TYPE}/{name}/{id}"),
    )
}

/// The name prefix Config gives the CloudFormation stack a conformance pack is deployed
/// as (the Config Developer Guide: a conformance pack deploys "a CloudFormation stack
/// named awsconfigconforms-<conformance-pack-name>" in the account).
pub const CONFORMANCE_PACK_STACK_NAME_PREFIX: &str = "awsconfigconforms-";

/// The ARN of the CloudFormation stack a conformance pack is deployed as:
/// `arn:aws:cloudformation:<region>:<account>:stack/awsconfigconforms-<name>-<id>/<stack_id>`,
/// the form a CloudFormation stack ARN takes. The ARN names the CLOUDFORMATION service
/// rather than Config, because the stack is CloudFormation's resource.
///
/// A CloudFormation stack ARN is `stack/<stack name>/<stack id>`, so the two segments
/// are different things and neither stands in for the other: the stack NAME is the one
/// Config gives the pack's stack — the prefix above followed by the pack's name and the
/// pack id, so two packs of the same name in different accounts do not name one stack —
/// and the stack ID is CloudFormation's own identifier for the stack, which is why it is
/// DRAWN here ([`stack_id`]) rather than built from anything the pack already carries.
pub fn conformance_pack_stack_arn(region: &str, name: &str, id: &str, stack_id: &str) -> String {
    ::emulator_server::emulator_arn(
        "cloudformation",
        region,
        &format!("stack/{CONFORMANCE_PACK_STACK_NAME_PREFIX}{name}-{id}/{stack_id}"),
    )
}

/// The `ConformancePackId` Config assigns a conformance pack created in this emulator:
/// `conformance-pack-` and [`CONFORMANCE_PACK_ID_CHARS`] opaque base-36 characters,
/// which satisfies `ConformancePackId`'s 1..1024 length and is ARN-safe.
///
/// DRAWN ([`opaque_id`]) rather than derived from how many packs have been created.
pub fn mint_conformance_pack_id() -> String {
    format!(
        "{CONFORMANCE_PACK_ARN_RESOURCE_TYPE}-{}",
        opaque_id(CONFORMANCE_PACK_ID_CHARS)
    )
}

/// How many opaque characters a `ConformancePackId` carries after its prefix.
///
/// EIGHT, rather than the sixteen this service's other ids carry, because this id is
/// also the suffix of the name Config gives every Config rule the pack deploys
/// ([`conformance_pack_rule_name`]): the id's length is what decides how much of a
/// declared rule name fits inside [`GENERATED_CONFIG_RULE_NAME_MAX_LENGTH`]. Recorded
/// behaviour: a pack-deployed rule is named `<declared>-conformance-pack-<short
/// random>` and the whole name stays inside that ceiling.
const CONFORMANCE_PACK_ID_CHARS: usize = 8;

/// The `configSnapshotId` of a configuration-snapshot delivery scheduled in this
/// emulator: sixteen opaque base-36 characters, the same shape this service's other
/// server-assigned ids take (see [`mint_recorder_id`]). The model constrains
/// `DeliverConfigSnapshotResponse$configSnapshotId` to a plain `String`, so the only
/// requirement is that it identifies one snapshot.
///
/// DRAWN ([`opaque_id`]) rather than derived from how many snapshots have been
/// delivered.
pub fn mint_config_snapshot_id() -> String {
    opaque_id(RECORDER_ID_CHARS)
}

/// The DNS suffix every Amazon Web Services service principal ends with, which is
/// what makes a value a service principal rather than an arbitrary string the
/// modelled `ServicePrincipal` pattern (`^[\w+=,.@-]+$`, which also admits `@`, `+`,
/// `=` and `,`) would accept.
pub const SERVICE_PRINCIPAL_SUFFIX: &str = ".amazonaws.com";

/// Config's own service principal, which is also the path segment IAM files Config's
/// service-linked role under.
pub const CONFIG_SERVICE_PRINCIPAL: &str = "config.amazonaws.com";

/// The IAM service-linked role Config creates in the account the first time a
/// service-linked configuration recorder is created, and the role every
/// service-linked configuration recorder then records with
/// (`PutServiceLinkedConfigurationRecorder`: "This API creates a service-linked role
/// AWSServiceRoleForConfig in your account. The service-linked role is created only
/// when the role does not exist in your account").
pub const CONFIG_SERVICE_LINKED_ROLE_NAME: &str = "AWSServiceRoleForConfig";

/// The ARN of [`CONFIG_SERVICE_LINKED_ROLE_NAME`]:
/// `arn:aws:iam::<account>:role/aws-service-role/config.amazonaws.com/AWSServiceRoleForConfig`,
/// the path IAM gives a service-linked role. IAM is global, so the ARN carries no
/// Region.
///
/// The role is created once per account and reused, so every service-linked
/// configuration recorder of one account records with the SAME role ARN.
pub fn config_service_linked_role_arn() -> String {
    ::emulator_server::emulator_arn(
        "iam",
        "",
        &format!(
            "role/aws-service-role/{CONFIG_SERVICE_PRINCIPAL}/{CONFIG_SERVICE_LINKED_ROLE_NAME}"
        ),
    )
}

/// The name Config assigns the service-linked configuration recorder of
/// `service_principal`, or `None` when the value names no Amazon Web Services service
/// principal.
///
/// The name is NOT chosen by the caller: Config assigns one carrying the reserved
/// prefix ([`SERVICE_LINKED_RECORDER_NAME_PREFIX`];
/// `PutServiceLinkedConfigurationRecorderResponse$Name`: "Config automatically
/// assigns a name that has the prefix \"AWSConfigurationRecorderFor\" to the new
/// service-linked configuration recorder"). The model documents no further structure,
/// so the suffix is DERIVED FROM THE REQUEST and from nothing else: the leading DNS
/// label of the service principal — the service's own name, the part that
/// distinguishes `ssm.amazonaws.com` from `securityhub.amazonaws.com` — with its
/// `-`/`_`-separated words upper-camel-cased, so one service principal always gets
/// one name and two different services never share one.
///
/// A value that is no service principal gets no name: it must end in
/// [`SERVICE_PRINCIPAL_SUFFIX`] and every label before that must be a DNS label.
pub fn service_linked_recorder_name(service_principal: &str) -> Option<String> {
    let host = service_principal.strip_suffix(SERVICE_PRINCIPAL_SUFFIX)?;
    if host.is_empty() || host.split('.').any(|label| !is_dns_label(label)) {
        return None;
    }
    let service = host.split('.').next()?;
    let suffix: String = service
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(capitalise)
        .collect();
    if suffix.is_empty() {
        return None;
    }
    Some(format!("{SERVICE_LINKED_RECORDER_NAME_PREFIX}{suffix}"))
}

/// Whether `label` is one DNS label of a service principal: ASCII alphanumerics,
/// `-` and `_`, neither empty nor hyphen-bounded.
fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `word` with its first character upper-cased and the rest left as it came.
fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
    }
}

/// How Config words `ValidationException` for a request whose `ServicePrincipal`
/// names no Amazon Web Services service at all (`ValidationException`: "You have
/// provided a service principal for service-linked configuration recorder that is not
/// valid").
///
/// BUILT FROM THE REQUEST — it quotes the value the caller sent and names the member
/// to fix. This is the refusal of a value that names no service, which is a different
/// thing from a value that names a service Config links no configuration recorder for:
/// that one is the recorder Config cannot resolve from the service principal it was
/// given, worded as [`UNRECOGNIZED_RECORDER_SERVICE_PRINCIPAL_MESSAGE`].
pub fn invalid_service_principal_message(service_principal: &str) -> String {
    format!(
        "The ServicePrincipal '{service_principal}' provided in the request is not a valid \
         service principal for a service-linked configuration recorder."
    )
}

/// How Config words `ConflictException` when the account already holds a
/// service-linked configuration recorder for the service a request names
/// (`ConflictException`: "For PutServiceLinkedConfigurationRecorder, you cannot
/// create a service-linked recorder because a service-linked recorder already exists
/// for the specified service").
///
/// A FIXED sentence: it states WHICH condition refused the call ("a service-linked
/// recorder already exists for the specified service") and quotes nothing the caller
/// sent, so the text is the same however the request named the service — a client
/// matching it separates this conflict leg from the operation's others, which the error
/// code alone does not.
pub const SERVICE_LINKED_RECORDER_CONFLICT_MESSAGE: &str =
    "A service-linked configuration recorder with the specified service principal \
     already exists.";

/// How Config words `LimitExceededException` when creating one more service-linked
/// configuration recorder would take the account past
/// [`SERVICE_LINKED_RECORDERS_QUOTA`].
///
/// The message states THE SERVICE'S OWN LIMIT, not anything from the request — the
/// same shape as [`max_configuration_recorders_message`] — so it is the same sentence
/// however the caller reached the limit.
pub fn service_linked_recorders_quota_message() -> String {
    format!(
        "Failed to create a service-linked configuration recorder because this account \
         already holds the maximum of {SERVICE_LINKED_RECORDERS_QUOTA} service-linked \
         configuration recorder(s) in this region"
    )
}

/// The ARN resource type of a connector, as Config spells it in the ARNs it mints.
pub const CONNECTOR_ARN_RESOURCE_TYPE: &str = "connector";

/// The `ConnectorName` length maximum (`com.amazonaws.configservice#ConnectorName`,
/// length 1..256), which is also the length of the resource portion of a connector's
/// ARN, because that portion is exactly the connector's name.
pub const CONNECTOR_NAME_MAX_LEN: usize = 256;

/// The ARN of a connector: `arn:aws:config:<region>:<account>:connector/<name>`.
pub fn connector_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONNECTOR_ARN_RESOURCE_TYPE}/{name}"),
    )
}

/// The name of the connector a configuration describes: the provider it connects to
/// and the identifier of the tenant within that provider.
///
/// `PutConnector` carries no name, so the name is DERIVED from the configuration —
/// which is also why one tenant can have only one connector: a second connector for
/// the same provider configuration would need the same name.
pub fn connector_name(provider: &str, tenant_identifier: &str) -> String {
    format!("{}-{tenant_identifier}", provider.to_lowercase())
}

/// The group lengths of the canonical, hyphenated form of a GUID — `8-4-4-4-12`
/// hexadecimal digits — which is the form Microsoft Entra ID (Azure AD) publishes a
/// directory (tenant) identifier and an application (client) identifier in.
pub const AZURE_IDENTIFIER_GUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// Whether `value` has the form an Azure tenant or client identifier takes: a GUID in
/// its canonical hyphenated form.
///
/// `AzureTenantIdentifier` and `AzureClientIdentifier` carry no `@pattern` — the model
/// constrains only their length (1..128) — so the FORM is checked here.
///
/// This is a FORM check, not a parse: what matters is the spelling the provider
/// publishes, and a UUID parser (the `uuid` crate's `try_parse`) accepts the simple
/// 32-character, braced and URN spellings too, so it would still need this group
/// check around it. Checking the five groups' lengths and their hexadecimal digits is
/// how the framework itself asserts this form (`vendor/server/src/lib.rs`,
/// `vendor/smithy-schema-server/src/query.rs`), and it keeps the check free of a
/// dependency the rest of this crate does not need.
pub fn is_azure_guid(value: &str) -> bool {
    let mut groups = value.split('-');
    AZURE_IDENTIFIER_GUID_GROUPS.iter().all(|length| {
        groups.next().is_some_and(|group| {
            group.len() == *length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && groups.next().is_none()
}

/// How Config words a `ConnectorConfiguration` that names no provider at all.
///
/// `ConnectorConfiguration` has one provider member and its doc states "You must
/// specify exactly one provider configuration", so a configuration carrying none
/// names nothing to connect to. The text is fixed — it is built from no part of the
/// request — because it is the only thing that tells this refusal apart from the other
/// `ValidationException` legs of `PutConnector`.
pub const CONNECTOR_CONFIGURATION_NO_PROVIDER_MESSAGE: &str =
    "ConnectorConfiguration must contain at least one provider";

/// How Config words a tenant identifier whose form is not an Azure one
/// ([`is_azure_guid`]). Fixed, and it withholds the value the caller sent.
pub const INVALID_AZURE_TENANT_IDENTIFIER_MESSAGE: &str = "Invalid tenant identifier";

/// How Config words a client identifier whose form is not an Azure one
/// ([`is_azure_guid`]). Fixed, for the same reason as
/// [`INVALID_AZURE_TENANT_IDENTIFIER_MESSAGE`].
pub const INVALID_AZURE_CLIENT_IDENTIFIER_MESSAGE: &str = "Invalid client identifier";

/// How Config words a provider credential it could not verify with the provider.
///
/// A connector IS the credential Config connects to the third-party cloud service
/// provider with, so the tenant/client pair is verified with that provider before any
/// connector is materialized, and a pair that does not verify is refused. Verifying
/// one means calling the provider's identity service — an outbound connection this
/// emulator does not make — so no pair presented here can be verified and every
/// well-formed configuration is refused with this text rather than answered with a
/// connector no credential stands behind.
pub const UNVERIFIABLE_AZURE_CREDENTIALS_MESSAGE: &str =
    "Unable to verify Azure credentials for the provided tenant configuration.";

/// The Region and the connector name an `Arn` names, or `None` when the value is not
/// a well-formed connector ARN of this service.
///
/// A well-formed ARN of another partition, Region or account parses here and simply
/// names no connector this store holds — "not found" rather than "not valid".
pub fn connector_arn_parts(arn: &str) -> Option<(&str, &str)> {
    let mut segments = arn.splitn(6, ':');
    if segments.next()? != "arn" {
        return None;
    }
    let partition = segments.next()?;
    let service = segments.next()?;
    let region = segments.next()?;
    let account = segments.next()?;
    let resource = segments.next()?;
    // A connector is a regional resource, so its ARN names a Region.
    if partition.is_empty() || service != ARN_SERVICE || region.is_empty() || account.is_empty() {
        return None;
    }
    let name = resource
        .strip_prefix(CONNECTOR_ARN_RESOURCE_TYPE)?
        .strip_prefix('/')?;
    // `ConnectorName` is 1..256 characters, and the resource portion of a connector
    // ARN is exactly that name.
    if name.is_empty() || name.len() > CONNECTOR_NAME_MAX_LEN || name.contains('/') {
        return None;
    }
    Some((region, name))
}

/// Why `arn` is not a well-formed connector ARN. BUILT FROM THE REQUEST: it quotes
/// the value the caller sent and names the form the member takes.
///
/// Reached from `PutThirdPartyServiceLinkedConfigurationRecorder` alone, and that is
/// deliberate: there the `ConnectorArn` is a reference to a dependency the recorder
/// needs, so the refusal says which value could not name one. The operations whose
/// `Arn` IS the connector they act on answer the single
/// [`CONNECTOR_ARN_NOT_VALID_MESSAGE`] instead. A well-formed ARN that names no
/// connector is not this refusal: there the recorder the service principal names
/// cannot be resolved at all, which is
/// [`UNRECOGNIZED_RECORDER_SERVICE_PRINCIPAL_MESSAGE`].
pub fn invalid_connector_arn_message(arn: &str) -> String {
    format!(
        "The Arn '{arn}' provided in the request is not a valid connector ARN. Please \
         provide an ARN of the form \
         arn:aws:{ARN_SERVICE}:<region>:<account>:{CONNECTOR_ARN_RESOURCE_TYPE}/<name>."
    )
}

/// The ONE text an operation whose `Arn` names the connector it acts on refuses that
/// `Arn` with when it does not resolve to a connector of this account in the Region
/// the request is scoped to.
///
/// Resolving the `Arn` is all `GetConnector` and `DeleteConnector` do, and a value
/// that is no connector ARN and a well-formed one that names no connector both fail to
/// resolve, so both are refused with this one text — fixed, carrying no part of the
/// request — and neither says which of the two it was: a caller cannot learn from a
/// refusal whether a connector by that ARN exists.
pub const CONNECTOR_ARN_NOT_VALID_MESSAGE: &str = "The connector ARN provided is not valid.";

/// The ARN resource type of a configuration aggregator, as Config spells it in the
/// ARNs it mints (`ConfigurationAggregatorArn`'s pattern:
/// `config-aggregator/config-aggregator-[a-z\d]+`).
pub const AGGREGATOR_ARN_RESOURCE_TYPE: &str = "config-aggregator";

/// The ARN of a configuration aggregator:
/// `arn:aws:config:<region>:<account>:config-aggregator/config-aggregator-<id>`.
pub fn configuration_aggregator_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{AGGREGATOR_ARN_RESOURCE_TYPE}/{id}"),
    )
}

/// The id Config assigns a configuration aggregator created in this emulator:
/// `config-aggregator-` plus eight opaque base-36 characters, the shape the
/// `ConfigurationAggregatorArn` pattern allows (`config-aggregator-[a-z\d]+`).
///
/// DRAWN ([`opaque_id`]) rather than derived from how many aggregators have been
/// created.
pub fn mint_configuration_aggregator_id() -> String {
    format!(
        "{AGGREGATOR_ARN_RESOURCE_TYPE}-{}",
        opaque_id(AGGREGATOR_ID_CHARS)
    )
}

/// How many base-36 characters a configuration aggregator's assigned id carries after
/// its prefix.
const AGGREGATOR_ID_CHARS: usize = 8;

/// The ARN resource type of an aggregation authorization, as Config spells it in the
/// ARNs it mints: an aggregation authorization's ARN is
/// `arn:aws:config:<region>:<account>:aggregation-authorization/`
/// `<authorizedAccountId>/<authorizedAwsRegion>` (the AWS Config resource ARN format
/// for `AggregationAuthorization`).
pub const AGGREGATION_AUTHORIZATION_ARN_RESOURCE_TYPE: &str = "aggregation-authorization";

/// The ARN of the aggregation authorization granted to `authorized_account_id` in
/// `authorized_aws_region`, held by the account and Region the request was signed for.
///
/// The authorized pair is what identifies the authorization, so it is what the ARN
/// carries: nothing is minted, and the same pair always names the same ARN.
pub fn aggregation_authorization_arn(
    region: &str,
    authorized_account_id: &str,
    authorized_aws_region: &str,
) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{AGGREGATION_AUTHORIZATION_ARN_RESOURCE_TYPE}/{authorized_account_id}/\
             {authorized_aws_region}"
        ),
    )
}

/// The separator between the two halves of the key one aggregation authorization is
/// held under. `AccountId`'s `^\d{12}$` pattern leaves no room for it in the account
/// id, so the key splits unambiguously at its FIRST occurrence even when the
/// authorized Region contains one.
const AGGREGATION_AUTHORIZATION_KEY_SEPARATOR: char = '/';

/// The exact length of an `AccountId` (`com.amazonaws.configservice#AccountId`,
/// pattern `^\d{12}$`).
const ACCOUNT_ID_LENGTH: usize = 12;

/// Whether `value` is a well formed Amazon Web Services account id
/// (`com.amazonaws.configservice#AccountId`, pattern `^\d{12}$`).
///
/// The model applies that pattern to the members it targets with `AccountId`, so this
/// is for the members that carry an account id under a WIDER target and therefore
/// reach a handler unchecked.
pub fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_LENGTH && value.chars().all(|digit| digit.is_ascii_digit())
}

/// The longest `AwsRegion` the model allows (`com.amazonaws.configservice#AwsRegion`,
/// `length 1..64`).
const AWS_REGION_MAX_LENGTH: usize = 64;

/// The key one aggregation authorization is held under: the authorized account id and
/// the authorized Region, in that order, so `DescribeAggregationAuthorizations` pages
/// by account and then by Region. `AccountId`'s `^\d{12}$` pattern leaves no room for
/// the separator in the account id, so distinct pairs give distinct keys.
pub fn aggregation_authorization_key(
    authorized_account_id: &str,
    authorized_aws_region: &str,
) -> String {
    format!(
        "{authorized_account_id}{AGGREGATION_AUTHORIZATION_KEY_SEPARATOR}\
         {authorized_aws_region}"
    )
}

/// Encode the key a `DescribeAggregationAuthorizations` page should resume at as an
/// opaque `NextToken`.
pub fn encode_aggregation_authorization_token(key: &str) -> String {
    encode_page_token(key)
}

/// The key a `NextToken` resumes at, or `None` when the token is malformed — i.e. not
/// something [`encode_aggregation_authorization_token`] produced, so it does not decode
/// to a key of the form [`aggregation_authorization_key`] builds: an authorized account
/// id of exactly twelve digits (`AccountId`, pattern `^\d{12}$`) and an authorized
/// Region within `AwsRegion`'s 1..64 length.
pub fn decode_aggregation_authorization_token(token: &str) -> Option<String> {
    let key = decode_page_token(token)?;
    let well_formed = match key.split_once(AGGREGATION_AUTHORIZATION_KEY_SEPARATOR) {
        Some((account, region)) => {
            is_account_id(account)
                && (1..=AWS_REGION_MAX_LENGTH).contains(&region.chars().count())
        }
        None => false,
    };
    well_formed.then_some(key)
}

/// The separator between the two halves of the key one aggregated source status is
/// held under ([`ConfigurationAggregatorEntry::source_statuses`]). A source id is an
/// account id (`^\d{12}$`), which leaves no room for it, so the key splits
/// unambiguously at its FIRST occurrence even when the Region contains one.
const AGGREGATED_SOURCE_STATUS_KEY_SEPARATOR: char = '/';

/// Encode the (source id, Region) pair a `DescribeConfigurationAggregatorSourcesStatus`
/// page should resume at as an opaque `NextToken`.
pub fn encode_aggregated_source_status_token(source_id: &str, aws_region: &str) -> String {
    encode_page_token(&format!(
        "{source_id}{AGGREGATED_SOURCE_STATUS_KEY_SEPARATOR}{aws_region}"
    ))
}

/// The (source id, Region) pair a `NextToken` resumes at, or `None` when the token is
/// malformed — i.e. not something [`encode_aggregated_source_status_token`] produced,
/// so it does not decode to a source id of exactly twelve digits (`AccountId`, pattern
/// `^\d{12}$`) and a Region within `AwsRegion`'s 1..64 length.
pub fn decode_aggregated_source_status_token(token: &str) -> Option<(String, String)> {
    let key = decode_page_token(token)?;
    let (source_id, aws_region) = key.split_once(AGGREGATED_SOURCE_STATUS_KEY_SEPARATOR)?;
    let well_formed = is_account_id(source_id)
        && (1..=AWS_REGION_MAX_LENGTH).contains(&aws_region.chars().count());
    well_formed.then(|| (source_id.to_string(), aws_region.to_string()))
}

/// The ARN of a Config rule: `arn:aws:config:<region>:<account>:config-rule/<id>`.
pub fn config_rule_arn(region: &str, rule_id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("config-rule/{rule_id}"))
}

/// The ARN resource type of a remediation configuration, as Config spells it in the
/// ARNs it mints.
pub const REMEDIATION_CONFIGURATION_ARN_RESOURCE_TYPE: &str = "remediation-configuration";

/// The ARN of a remediation configuration:
/// `arn:aws:config:<region>:<account>:remediation-configuration/<rule-name>/<id>`.
/// Both the remediated rule's name and the id Config assigned the configuration are
/// part of the ARN.
pub fn remediation_configuration_arn(region: &str, config_rule_name: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{REMEDIATION_CONFIGURATION_ARN_RESOURCE_TYPE}/{config_rule_name}/{id}"),
    )
}

/// The id Config assigns a remediation configuration created in this emulator:
/// sixteen opaque base-36 characters, drawn the same way a configuration recorder id
/// is (see [`mint_recorder_id`]).
pub fn mint_remediation_configuration_id() -> String {
    opaque_id(RECORDER_ID_CHARS)
}

/// Whether `parameter` carries a value at all: a static value with at least one
/// non-blank entry, or a dynamic (resource) value.
fn parameter_carries_value(parameter: &RemediationParameterValue) -> bool {
    parameter
        .static_value
        .as_ref()
        .is_some_and(|static_value| {
            static_value
                .values
                .iter()
                .any(|value| !value.trim().is_empty())
        })
        || parameter
            .resource_value
            .as_ref()
            .is_some_and(|resource_value| !resource_value.value.trim().is_empty())
}

/// The `Parameters` keys that can carry the role a remediation runs as: an AUTOMATIC
/// configuration remediates only through [`AUTOMATION_ASSUME_ROLE_PARAMETER`] ("the
/// only valid assumeRole field value is AutomationAssumeRole"), a MANUAL one through
/// either key, as the SSM document's own parameter names decide.
pub fn assume_role_parameter_names(automatic: bool) -> &'static [&'static str] {
    if automatic {
        &[AUTOMATION_ASSUME_ROLE_PARAMETER]
    } else {
        &[AUTOMATION_ASSUME_ROLE_PARAMETER, ASSUME_ROLE_PARAMETER]
    }
}

/// Whether `requested` provides the role Config assumes to remediate a resource,
/// under one of the keys [`assume_role_parameter_names`] accepts for it. An AUTOMATIC
/// remediation needs one — "you need to provide a value for AutomationAssumeRole to
/// remediate your resources" — while a manual one takes the role from the SSM document's
/// own parameters, so this is asked about a manual configuration only where the role
/// itself is being resolved, never to refuse one.
pub fn has_assume_role(requested: &RemediationConfiguration) -> bool {
    let Some(parameters) = requested.parameters.as_ref() else {
        return false;
    };
    assume_role_parameter_names(requested.automatic)
        .iter()
        .any(|name| parameters.get(*name).is_some_and(parameter_carries_value))
}

/// The name of a parameter of `requested` that does not select exactly ONE kind of
/// value, with what is wrong with it.
///
/// `RemediationParameterValue` is "either a dynamic (resource) value or a static
/// value. You must select either a dynamic value or a static value" — so a parameter
/// that selects both, or neither, is not a value Config can resolve at remediation
/// time. The modelled `Parameters` map is a `HashMap`, so the names are sorted before
/// they are visited: a request with several bad parameters is then always refused for
/// the same one.
pub fn ill_formed_parameter(requested: &RemediationConfiguration) -> Option<(String, &'static str)> {
    let parameters = requested.parameters.as_ref()?;
    let mut names: Vec<&String> = parameters.keys().collect();
    names.sort();
    names.into_iter().find_map(|name| {
        let parameter = parameters.get(name)?;
        match (
            parameter.static_value.is_some(),
            parameter.resource_value.is_some(),
        ) {
            (true, true) => Some((name.clone(), "both a StaticValue and a ResourceValue")),
            (false, false) => Some((name.clone(), "neither a StaticValue nor a ResourceValue")),
            _ => None,
        }
    })
}

/// The `ConfigRuleId` Config assigns a rule created in this emulator:
/// `config-rule-<six opaque base-36 characters>`, the shape Config's own rule ids
/// take.
///
/// DRAWN ([`opaque_id`]) rather than derived from how many rules have been created.
pub fn mint_config_rule_id() -> String {
    format!("config-rule-{}", opaque_id(CONFIG_RULE_ID_CHARS))
}

/// How many base-36 characters a Config rule's assigned id carries after its prefix.
const CONFIG_RULE_ID_CHARS: usize = 6;

/// The recording group Config records with when a recorder is put without one
/// (`PutConfigurationRecorder`: "Default is to record all supported resource types,
/// excluding the global IAM resource types").
pub fn default_recording_group() -> RecordingGroup {
    RecordingGroup {
        all_supported: true,
        include_global_resource_types: false,
        resource_types: None,
        exclusion_by_resource_types: None,
        recording_strategy: Some(RecordingStrategy {
            use_only: Some(RECORDING_STRATEGY_ALL_SUPPORTED.to_string()),
        }),
    }
}

/// The recording strategy a recording group RECORDS UNDER: the one the group states
/// itself, or the one its resource type lists imply ([`implied_recording_strategy`]).
pub fn recording_strategy_of(group: &RecordingGroup) -> &str {
    group
        .recording_strategy
        .as_ref()
        .and_then(|strategy| strategy.use_only.as_deref())
        .filter(|use_only| !use_only.is_empty())
        .unwrap_or_else(|| implied_recording_strategy(group))
}

/// Whether a recording group covers `resource_type`: only the types an inclusion
/// group enumerates, every type an exclusion group does not exclude, and every type
/// under `ALL_SUPPORTED_RESOURCE_TYPES`.
pub fn group_records_resource_type(group: &RecordingGroup, resource_type: &str) -> bool {
    let listed = |resource_types: Option<&[String]>| {
        resource_types
            .unwrap_or_default()
            .iter()
            .any(|listed| listed == resource_type)
    };
    match recording_strategy_of(group) {
        RECORDING_STRATEGY_INCLUSION => listed(group.resource_types.as_deref()),
        RECORDING_STRATEGY_EXCLUSION => !listed(
            group
                .exclusion_by_resource_types
                .as_ref()
                .and_then(|exclusion| exclusion.resource_types.as_deref()),
        ),
        // `ALL_SUPPORTED_RESOURCE_TYPES`, and any strategy this emulator does not
        // know: the recorder records every type it discovers.
        _ => true,
    }
}

/// The recording strategy a group's own resource type lists imply: the exclusion
/// strategy when the group excludes resource types, the inclusion strategy when it
/// enumerates them, and `ALL_SUPPORTED_RESOURCE_TYPES` otherwise.
fn implied_recording_strategy(group: &RecordingGroup) -> &'static str {
    let excludes = group
        .exclusion_by_resource_types
        .as_ref()
        .and_then(|exclusion| exclusion.resource_types.as_deref())
        .is_some_and(|resource_types| !resource_types.is_empty());
    let includes = group
        .resource_types
        .as_deref()
        .is_some_and(|resource_types| !resource_types.is_empty());
    if excludes {
        RECORDING_STRATEGY_EXCLUSION
    } else if includes {
        RECORDING_STRATEGY_INCLUSION
    } else {
        RECORDING_STRATEGY_ALL_SUPPORTED
    }
}

/// The recording group a recorder RECORDS WITH, derived from the one a request
/// carried (or its absence).
///
/// Config stores the recorder as a complete recording group rather than as the
/// subset the caller sent, so a read reports concrete values: the recording strategy
/// implied by the group — the exclusion strategy when the group excludes resource
/// types, the inclusion strategy when it enumerates them, and
/// `ALL_SUPPORTED_RESOURCE_TYPES` otherwise — and an empty exclusion list when the
/// group excludes nothing. A value the request carried is kept as it was sent.
pub fn materialised_recording_group(requested: Option<RecordingGroup>) -> RecordingGroup {
    let mut group = requested.unwrap_or_else(default_recording_group);

    if group.recording_strategy.is_none() {
        group.recording_strategy = Some(RecordingStrategy {
            use_only: Some(implied_recording_strategy(&group).to_string()),
        });
    }
    if group.exclusion_by_resource_types.is_none() {
        group.exclusion_by_resource_types = Some(ExclusionByResourceTypes {
            resource_types: Some(Vec::new()),
        });
    }
    group
}

/// Which way a request changes the resource types a configuration recorder records:
/// `AssociateResourceTypes` adds types to what it records, `DisassociateResourceTypes`
/// removes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceTypeAssociation {
    /// The types are to be RECORDED after the call ("Adds all resource types specified
    /// in the ResourceTypes list to the RecordingGroup of specified configuration
    /// recorder and includes those resource types when recording").
    Associate,
    /// The types are NOT to be recorded after the call ("Removes all resource types
    /// specified in the ResourceTypes list from the RecordingGroup of configuration
    /// recorder and excludes these resource types when recording").
    Disassociate,
}

impl ResourceTypeAssociation {
    /// Whether the recorder records a type this change names once the change is
    /// applied.
    fn records_after(self) -> bool {
        matches!(self, Self::Associate)
    }
}

/// How Config words `ValidationException` for a resource type the recorder already
/// treats the way the request asks for.
///
/// The MODEL STATES THIS MESSAGE for exactly these two operations
/// (`ValidationException`: "For AssociateResourceTypes and DisassociateResourceTypes,
/// one of the following errors: … One or more of the specified resource types are
/// already associated or disassociated with the configuration recorder"), so the text is
/// part of the contract: ONE sentence covering both directions, which is why neither
/// direction words its own and why no resource type of the request is quoted into it.
pub const RESOURCE_TYPES_ALREADY_SETTLED: &str =
    "One or more of the specified resource types are already associated or disassociated \
     with the configuration recorder.";

/// How Config words `ValidationException` for an association or disassociation of
/// resource types on a recorder whose recording strategy has no place for one.
///
/// The MODEL STATES THIS MESSAGE for exactly these two operations
/// (`ValidationException`: "For AssociateResourceTypes and DisassociateResourceTypes,
/// one of the following errors: Your configuraiton recorder has a recording strategy
/// that does not allow the association or disassociation of resource types"), so the
/// text is part of the contract — including the service's own spelling of
/// "configuraiton", which is how the sentence reads in the API reference.
pub const RECORDING_STRATEGY_FORBIDS_ASSOCIATION: &str =
    "Your configuraiton recorder has a recording strategy that does not allow the \
     association or disassociation of resource types.";

/// Apply `change` to the resource types `group` records, or refuse it with the message
/// Config words for the rule it breaks.
///
/// ONE rule, written once for both operations that change a recorder's recorded
/// resource types: the change is expressed in the recording strategy the recorder
/// RECORDS UNDER — an inclusion group lists the types it records, so a type joins or
/// leaves that list; an exclusion group records every type it does not list, so a type
/// LEAVES the exclusion list to become recorded and joins it to stop being recorded —
/// and a strategy that enumerates nothing (`ALL_SUPPORTED_RESOURCE_TYPES`) records
/// every type there is, so no list expresses the change and it is refused. A type the
/// recorder already treats the way the request asks for is refused too: there is no
/// change to make.
///
/// `group` is left UNTOUCHED when the change is refused — every type is checked before
/// any is applied — so a refused call changes nothing about what the recorder records.
///
/// The strategy's list is BOUNDED by the resource types there are: a type the recorder
/// already treats the way the request asks for is refused, so the list holds at most one
/// entry per distinct type a request can name, and `ResourceTypeList`'s member is the
/// modelled `ResourceType` enum, which the request boundary holds a request to. No
/// sequence of calls can grow a recorder's recording group further.
pub fn change_recorded_resource_types(
    group: &mut RecordingGroup,
    change: ResourceTypeAssociation,
    resource_types: &[String],
) -> Result<(), String> {
    let strategy = recording_strategy_of(group).to_string();
    if strategy != RECORDING_STRATEGY_INCLUSION && strategy != RECORDING_STRATEGY_EXCLUSION {
        return Err(RECORDING_STRATEGY_FORBIDS_ASSOCIATION.to_string());
    }

    // Every named type is judged against what the recorder records NOW, so a request
    // naming the same type twice is refused for the same reason one naming an
    // already-recorded type is: the second mention asks for a change the first already
    // made.
    let mut named: Vec<&str> = Vec::with_capacity(resource_types.len());
    for resource_type in resource_types {
        let settled = group_records_resource_type(group, resource_type) == change.records_after();
        if settled || named.contains(&resource_type.as_str()) {
            return Err(RESOURCE_TYPES_ALREADY_SETTLED.to_string());
        }
        named.push(resource_type.as_str());
    }

    // The list the change is expressed in, and whether the named types belong to it
    // afterwards: an inclusion group lists what it RECORDS, an exclusion group lists
    // what it does NOT.
    let listed_types = if strategy == RECORDING_STRATEGY_INCLUSION {
        group.resource_types.get_or_insert_with(Vec::new)
    } else {
        group
            .exclusion_by_resource_types
            .get_or_insert_with(ExclusionByResourceTypes::default)
            .resource_types
            .get_or_insert_with(Vec::new)
    };
    let belongs = (strategy == RECORDING_STRATEGY_INCLUSION) == change.records_after();
    for resource_type in resource_types {
        if belongs {
            listed_types.push(resource_type.clone());
        } else {
            listed_types.retain(|listed| listed != resource_type);
        }
    }
    Ok(())
}

/// The recording mode a recorder RECORDS WITH, derived from the one a request
/// carried (or its absence): continuous recording with no per-resource-type
/// overrides unless the request asked for another frequency or listed overrides.
pub fn materialised_recording_mode(requested: Option<RecordingMode>) -> RecordingMode {
    let mut mode = requested.unwrap_or_else(|| RecordingMode {
        recording_frequency: RECORDING_FREQUENCY_CONTINUOUS.to_string(),
        recording_mode_overrides: None,
    });
    if mode.recording_frequency.is_empty() {
        mode.recording_frequency = RECORDING_FREQUENCY_CONTINUOUS.to_string();
    }
    if mode.recording_mode_overrides.is_none() {
        mode.recording_mode_overrides = Some(Vec::new());
    }
    mode
}

/// The recording scope a CUSTOMER MANAGED recorder records in: the one the request
/// carried, or [`RECORDING_SCOPE_PAID`].
pub fn materialised_recording_scope(requested: Option<String>) -> String {
    requested
        .filter(|scope| !scope.is_empty())
        .unwrap_or_else(|| RECORDING_SCOPE_PAID.to_string())
}

/// The compliance roll-up of `rules`: how many are compliant and how many are
/// noncompliant, each counted up to [`COMPLIANCE_SUMMARY_RULE_CAP`], as of `at`.
pub fn summarise_rule_compliance<'a>(
    rules: impl Iterator<Item = &'a ConfigRuleEntry>,
    at: SystemTime,
) -> ComplianceSummary {
    summarise_compliance(
        rules.map(ConfigRuleEntry::compliance),
        COMPLIANCE_SUMMARY_RULE_CAP,
        at,
    )
}

/// The compliance roll-up of `verdicts` — one `ComplianceType` per item summarised:
/// how many of them are compliant and how many are noncompliant, each counted up to
/// `cap`, as of `at`.
///
/// The cap is a parameter for the same reason [`capped_count`]'s is: Config counts the
/// RULES a summary covers up to [`COMPLIANCE_SUMMARY_RULE_CAP`] and the RESOURCES one
/// covers up to [`COMPLIANCE_RESOURCE_CAP`], while the summarising rule itself — which
/// verdicts count towards which member, and when the count is capped — is the same
/// one, written here once.
pub fn summarise_compliance<'a>(
    verdicts: impl Iterator<Item = &'a str>,
    cap: i32,
    at: SystemTime,
) -> ComplianceSummary {
    let mut compliant = 0i64;
    let mut non_compliant = 0i64;
    for verdict in verdicts {
        match verdict {
            COMPLIANCE_TYPE_COMPLIANT => compliant += 1,
            COMPLIANCE_TYPE_NON_COMPLIANT => non_compliant += 1,
            // Something no evaluation result places either way contributes to neither
            // count.
            _ => {}
        }
    }
    ComplianceSummary {
        compliant_resource_count: Some(capped_count(compliant, cap)),
        non_compliant_resource_count: Some(capped_count(non_compliant, cap)),
        compliance_summary_timestamp: Some(timestamp(at)),
    }
}

/// The `ComplianceType` every RESOURCE the account's `rules` have evaluated rolls up
/// to, keyed by the `(resourceType, resourceId)` pair that identifies a resource in
/// Config — so one resource several rules evaluated appears ONCE, whichever rules
/// placed it.
///
/// A resource falls in the scope of every rule that evaluates its type, so its own
/// compliance is the roll-up of the verdicts all of those rules recorded for it, under
/// the same precedence a single rule's roll-up follows
/// ([`resource_evaluation::roll_up_compliance`]): noncompliant if any rule placed it
/// so, compliant if at least one did and none placed it noncompliant, and
/// `INSUFFICIENT_DATA` where no verdict places it either way.
pub fn resource_compliance<'a>(
    rules: impl Iterator<Item = &'a ConfigRuleEntry>,
) -> BTreeMap<(String, String), &'static str> {
    verdicts_by_resource(rules)
        .into_iter()
        .map(|(resource, verdicts)| {
            (
                resource,
                resource_evaluation::roll_up_compliance(
                    verdicts.into_iter().map(|(_, verdict)| verdict),
                ),
            )
        })
        .collect()
}

/// The modelled `Compliance` every RESOURCE the account's `rules` have evaluated rolls
/// up to, keyed the way [`resource_compliance`] keys it.
///
/// The `ComplianceType` is that same roll-up. `Compliance$ComplianceContributorCount` is
/// "the number of Amazon Web Services resources or Config rules that cause a result of
/// NON_COMPLIANT", and what contributes to a RESOURCE's result is the rules that placed
/// it there ("If a resource is noncompliant, this operation returns the number of Config
/// rules that the resource does not comply with", `DescribeComplianceByResource`) — so
/// the count is of RULES — one rule that placed the resource noncompliant in more than
/// one evaluation mode contributes once, because what the member counts is rules and not
/// results — reported only where such rules exist and counted up to the cap a count of
/// rules is reported to ([`COMPLIANCE_SUMMARY_RULE_CAP`]). That mirrors a rule's own
/// `Compliance` ([`ConfigRuleEntry::compliance_shape`]), where the contributors are
/// resources and the resource cap applies instead.
pub fn resource_compliance_shapes<'a>(
    rules: impl Iterator<Item = &'a ConfigRuleEntry>,
) -> BTreeMap<(String, String), Compliance> {
    verdicts_by_resource(rules)
        .into_iter()
        .map(|(resource, verdicts)| {
            let contributors: BTreeSet<&str> = verdicts
                .iter()
                .filter(|(_, verdict)| *verdict == COMPLIANCE_TYPE_NON_COMPLIANT)
                .map(|(rule_name, _)| *rule_name)
                .collect();
            let contributors = contributors.len();
            let compliance = Compliance {
                compliance_type: Some(
                    resource_evaluation::roll_up_compliance(
                        verdicts.into_iter().map(|(_, verdict)| verdict),
                    )
                    .to_string(),
                ),
                compliance_contributor_count: (contributors > 0).then(|| {
                    capped_count(
                        i64::try_from(contributors).unwrap_or(i64::MAX),
                        COMPLIANCE_SUMMARY_RULE_CAP,
                    )
                }),
            };
            (resource, compliance)
        })
        .collect()
}

/// Every verdict the account's `rules` recorded for each RESOURCE they evaluated — the
/// name of the rule that recorded it and the `ComplianceType` it placed the resource at —
/// keyed by the `(resourceType, resourceId)` pair that identifies a resource in Config.
///
/// The rule's NAME travels with its verdict because a rule holds one result per
/// evaluation mode ([`EvaluationResultKey`]), so counting results would count one rule
/// that placed the same resource in both modes twice.
///
/// The ONE place a resource's verdicts are gathered out of the rules that recorded them,
/// so what a resource rolls up to ([`resource_compliance`]) and how many rules put it
/// there ([`resource_compliance_shapes`]) are read off the same grouping.
fn verdicts_by_resource<'a>(
    rules: impl Iterator<Item = &'a ConfigRuleEntry>,
) -> BTreeMap<(String, String), Vec<(&'a str, &'a str)>> {
    let mut recorded: BTreeMap<(String, String), Vec<(&str, &str)>> = BTreeMap::new();
    for rule in rules {
        for result in rule.evaluation_results.values() {
            recorded
                .entry((result.resource_type.clone(), result.resource_id.clone()))
                .or_default()
                .push((rule.name.as_str(), result.compliance_type.as_str()));
        }
    }
    recorded
}

/// `count` contributors reported as a `ComplianceContributorCount` against `cap`: the
/// count itself while it is under the cap, the cap once it is past it, and
/// `CapExceeded` set exactly then.
///
/// The cap is a parameter because Config reports a count of rules and a count of
/// resources up to different maxima ([`COMPLIANCE_SUMMARY_RULE_CAP`] and
/// [`COMPLIANCE_RESOURCE_CAP`]), while the capping rule itself is the same one.
fn capped_count(count: i64, cap: i32) -> ComplianceContributorCount {
    let limit = i64::from(cap);
    ComplianceContributorCount {
        capped_count: i32::try_from(count.min(limit)).unwrap_or(cap),
        cap_exceeded: count > limit,
    }
}

/// The evaluation modes a rule carries when it is put without any
/// (`ConfigRule$EvaluationModes`: "By default, the value is Detective evaluation
/// mode only").
pub fn default_evaluation_modes() -> Vec<EvaluationModeConfiguration> {
    vec![EvaluationModeConfiguration {
        mode: Some(EVALUATION_MODE_DETECTIVE.to_string()),
    }]
}

/// A modelled timestamp for a wall-clock instant.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The wall-clock instant a modelled timestamp a REQUEST carried names, or `None` for
/// a timestamp this service cannot place in its history: one that is not finite, is
/// before the epoch, or is further from it than an instant reaches.
///
/// The partial inverse of [`timestamp`]: this service stores every instant as a
/// `SystemTime`, so an instant a caller supplies is converted once, on the way in, and
/// read back out through [`timestamp`]. The conversion is the FALLIBLE one
/// `aws_smithy_types` provides — the JSON wire accepts any numeric epoch, so an
/// unplaceable value has to come back as a refusal for the caller rather than as a
/// panic in the arithmetic.
pub fn instant(at: ::aws_smithy_types::DateTime) -> Option<SystemTime> {
    let seconds = at.as_secs_f64();
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    SystemTime::try_from(at).ok()
}

/// How Config words `InvalidParameterValueException` for a timestamp member this
/// service cannot place as an instant ([`instant`]).
///
/// The message is BUILT FROM THE REQUEST — it names the member and the epoch seconds
/// the caller sent — so every operation that takes an instant from a request words the
/// refusal the same way.
pub fn unplaceable_timestamp_message(member: &str, at: ::aws_smithy_types::DateTime) -> String {
    format!(
        "'{}' is not a valid value for {member}; the value must be an instant \
         representable as a time.",
        at.as_secs_f64()
    )
}

/// Encode the key the next page of a paged read should resume at as an opaque
/// `NextToken`, with the base64 codec the framework already carries. Every paged read
/// of this service pages over its store's own key ordering, so one codec serves them
/// all.
pub fn encode_page_token(resume_at: &str) -> String {
    ::aws_smithy_types::base64::encode(resume_at)
}

/// How Config words `InvalidNextTokenException` for a `NextToken` it did not issue.
///
/// FIXED, and it carries NO part of the request: the token the caller sent is not
/// echoed back. One sentence serves every paged read of this service — the token is
/// opaque, so the only thing a caller can learn from the refusal is that the value is
/// not a page of this query, and the page it should resume from is the one the previous
/// response handed it.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The nextToken provided is invalid";

/// The key a `NextToken` resumes at, or `None` when the token is malformed — i.e.
/// not something [`encode_page_token`] produced.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = ::aws_smithy_types::base64::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// The separator inside the page token of a read that reports COUNTS per group, which
/// orders its groups by the count before the group's name. Neither a resource type
/// (alphanumeric segments joined by `::`), an account id (twelve digits) nor a Region
/// name can carry this byte, so the split is unambiguous.
const GROUPED_COUNT_TOKEN_SEPARATOR: char = '|';

/// Encode the place a page of per-group counts resumes at — the count and the group
/// name of the first group it should report — as an opaque `NextToken`, with the codec
/// [`encode_page_token`] carries.
///
/// The whole sort key is in the token rather than an offset, so a page resumes at the
/// same place even when resources are recorded or deleted between pages.
pub fn encode_grouped_count_token(count: i64, group_name: &str) -> String {
    encode_page_token(&format!(
        "{count}{GROUPED_COUNT_TOKEN_SEPARATOR}{group_name}"
    ))
}

/// The count and group name a `NextToken` resumes at, or `None` when the token is not
/// one [`encode_grouped_count_token`] produced: a count no inventory can hold is not
/// one of ours either.
pub fn decode_grouped_count_token(token: &str) -> Option<(i64, String)> {
    let decoded = decode_page_token(token)?;
    let (count, group_name) = decoded.split_once(GROUPED_COUNT_TOKEN_SEPARATOR)?;
    if group_name.is_empty() {
        return None;
    }
    let count: i64 = count.parse().ok()?;
    (count >= 0).then_some((count, group_name.to_string()))
}

/// The `detail` of the `Config Rules Compliance Change` event for one verdict change.
///
/// The body is the one the EventBridge reference for Config documents: the rule and
/// the resource the verdict is about, `messageType`
/// [`COMPLIANCE_CHANGE_MESSAGE_TYPE`], the result that now holds under
/// `newEvaluationResult`, and — when the change REPLACED a verdict rather than
/// recording the first one — the result it replaced under `oldEvaluationResult`, which
/// is the only place a consumer can read what the verdict changed FROM. Members are
/// lowerCamelCase, as an event detail's are.
pub fn compliance_change_detail(
    rule_name: &str,
    rule_arn: &str,
    region: &str,
    new_result: &EvaluationResultEntry,
    old_result: Option<&EvaluationResultEntry>,
    notification_time: SystemTime,
) -> ::serde_json::Value {
    let mut detail = ::serde_json::json!({
        "configRuleName": rule_name,
        "configRuleARN": rule_arn,
        "awsAccountId": ::engine::emulator_account_id(),
        "awsRegion": region,
        "resourceId": new_result.resource_id,
        "resourceType": new_result.resource_type,
        "recordVersion": COMPLIANCE_CHANGE_RECORD_VERSION,
        "messageType": COMPLIANCE_CHANGE_MESSAGE_TYPE,
        "notificationCreationTime": event_instant(notification_time),
        "newEvaluationResult": evaluation_result_detail(rule_name, new_result),
    });
    if let Some(old_result) = old_result {
        detail["oldEvaluationResult"] = evaluation_result_detail(rule_name, old_result);
    }
    detail
}

/// One evaluation result as a compliance-change notification carries it: the
/// `EvaluationResult` shape's members, in the lowerCamelCase an event detail uses.
fn evaluation_result_detail(
    rule_name: &str,
    result: &EvaluationResultEntry,
) -> ::serde_json::Value {
    let mut rendered = ::serde_json::json!({
        "evaluationResultIdentifier": {
            "evaluationResultQualifier": {
                "configRuleName": rule_name,
                "resourceType": result.resource_type,
                "resourceId": result.resource_id,
                "evaluationMode": result.evaluation_mode,
            },
            "orderingTimestamp": event_instant(result.ordering_timestamp),
        },
        "complianceType": result.compliance_type,
        "resultRecordedTime": event_instant(result.result_recorded_time),
        "configRuleInvokedTime": event_instant(result.config_rule_invoked_time),
    });
    if let Some(annotation) = result.annotation.as_deref() {
        rendered["annotation"] = ::serde_json::Value::String(annotation.to_string());
    }
    rendered
}

/// `at` as the RFC 3339 instant an event detail states a time as, or `None` when `at`
/// is not an instant that form can state (a year outside the four-digit range, say).
///
/// A caller-supplied instant is checked through this before it is recorded, so no
/// notification can carry a member whose value is an empty string.
pub fn rfc3339_instant(at: SystemTime) -> Option<String> {
    timestamp(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// `at` as the RFC 3339 instant an event detail states a time as.
///
/// Every instant that reaches here has already been checked ([`rfc3339_instant`]) or
/// comes from this process's own clock, so the fallback is unreachable; it is logged
/// rather than silently emitted, because a member with an empty value would be
/// indistinguishable from one a notification does not carry.
pub(crate) fn event_instant(at: SystemTime) -> String {
    rfc3339_instant(at).unwrap_or_else(|| {
        ::tracing::warn!("an evaluation instant could not be stated as RFC 3339");
        String::new()
    })
}

/// Publish ONE event of this service's own source onto the account's default
/// EventBridge bus.
///
/// An Amazon Web Services service's events reach a consumer through EventBridge, so
/// this is a `PutEvents` call to the EventBridge the emulator is serving, made the way
/// any emulated service reaches another: through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no
/// other service crate.
///
/// Publishing is BEST EFFORT and never changes what the operation answers: AWS
/// delivers a service event asynchronously, after the write that produced it was
/// accepted, so an emulator with no EventBridge mounted (a router carrying only this
/// service, say) still accepts the write. A failure to publish is logged, because a
/// deployment where no consumer can be reached is worth seeing.
pub async fn publish_service_event(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    detail_type: &str,
    detail: &::serde_json::Value,
) {
    publish_service_event_via(ctx.callback(), region, detail_type, detail).await;
}

/// [`publish_service_event`] over the callback alone, so the recording pump
/// ([`recording::drive_recorder_clock`]) — which outlives the request that started it
/// and has no `OperationContext` — publishes through exactly the same path a handler
/// does.
pub async fn publish_service_event_via(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    detail_type: &str,
    detail: &::serde_json::Value,
) {
    let config = ::aws_sdk_eventbridge::Config::builder()
        .behavior_version(::aws_sdk_eventbridge::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_eventbridge::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://events.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch
        // for a stalled stream, and each of those would need a sleep implementation
        // this client is not given. The same goes for caching the identity: the
        // credentials are handed over directly.
        .retry_config(::aws_sdk_eventbridge::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eventbridge::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eventbridge::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_eventbridge::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
        .event_bus_name(DEFAULT_EVENT_BUS_NAME)
        .source(EVENT_SOURCE)
        .detail_type(detail_type)
        .detail(detail.to_string())
        .build();
    let answer = ::aws_sdk_eventbridge::Client::from_conf(config)
        .put_events()
        .entries(entry)
        .send()
        .await;
    match answer {
        Ok(output) if output.failed_entry_count() == 0 => {}
        Ok(output) => ::tracing::warn!(
            detail_type,
            failed_entry_count = output.failed_entry_count(),
            "EventBridge refused the event this service published"
        ),
        Err(error) => ::tracing::warn!(
            detail_type,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the event this service published did not reach EventBridge"
        ),
    }
}

/// The `ResourceEvaluationId` Config assigns a resource evaluation started in this
/// emulator.
///
/// The model gives `ResourceEvaluationId` no pattern (only a 1..128 length), and the
/// recorded ids are opaque and unordered, so the id takes the same opaque
/// 32-character hexadecimal form a stored query's does ([`uuid_id`]): DRAWN rather
/// than derived from how many evaluations have been started, so one evaluation's id
/// tells a caller nothing about another's.
pub fn mint_resource_evaluation_id() -> String {
    uuid_id()
}

/// `at` as nanoseconds since the Unix epoch: one totally ordered integer for an
/// instant, so a time-window filter and a page ordering compare instants the same way,
/// at the full resolution a modelled timestamp carries.
pub fn epoch_nanos(at: ::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}

/// The resolution an instant this service REPORTS is read back at, in nanoseconds.
///
/// A response carries an instant as an epoch-seconds number, which a client holds as a
/// 64-bit float: around an instant of this decade such a float resolves well under a
/// microsecond but nowhere near a nanosecond, so the instant a client sends back is the
/// one it was given only down to the millisecond AWS reports an instant at. Every bound
/// of a time window a request states is therefore matched at this resolution
/// ([`window_from_nanos`], [`window_to_nanos`]).
pub const REPORTED_INSTANT_NANOS: i128 = 1_000_000;

/// The nanosecond instant a time window that STARTS at `at` opens: the beginning of the
/// millisecond `at` names.
///
/// This is what makes a window bound a client took from an instant this service reported
/// select the item it came from: the number the client read back is that instant to the
/// millisecond, so a window opening at it opens no later than the item it names.
pub fn window_from_nanos(at: ::aws_smithy_types::DateTime) -> i128 {
    let nanos = epoch_nanos(at);
    nanos - nanos.rem_euclid(REPORTED_INSTANT_NANOS)
}

/// The nanosecond instant a time window that ENDS at `at` closes: the end of the
/// millisecond `at` names, which is the other half of [`window_from_nanos`].
pub fn window_to_nanos(at: ::aws_smithy_types::DateTime) -> i128 {
    window_from_nanos(at) + REPORTED_INSTANT_NANOS - 1
}

/// Encode the [`ResourceEvaluationPageKey`] a `ListResourceEvaluations` page should
/// resume at as an opaque `NextToken`.
pub fn encode_resource_evaluation_token(resume_at: &ResourceEvaluationPageKey) -> String {
    encode_page_token(&format!("{}:{}", resume_at.0 .0, resume_at.1))
}

/// The [`ResourceEvaluationPageKey`] a `NextToken` resumes at, or `None` when the
/// token is malformed — i.e. not something [`encode_resource_evaluation_token`]
/// produced.
pub fn decode_resource_evaluation_token(token: &str) -> Option<ResourceEvaluationPageKey> {
    let decoded = decode_page_token(token)?;
    let (nanos, id) = decoded.split_once(':')?;
    let nanos: i128 = nanos.parse().ok()?;
    if id.is_empty() {
        return None;
    }
    Some((::std::cmp::Reverse(nanos), id.to_string()))
}

/// How Config words `NoAvailableConfigurationRecorderException` on an operation for
/// which no per-operation text is known: a paraphrase of the shape's own doc ("There
/// are no customer managed configuration recorders available to record your
/// resources. Use the PutConfigurationRecorder operation to create the customer
/// managed configuration recorder").
///
/// `PutConfigRule` and `PutDeliveryChannel` do NOT use this: each carries the fixed
/// message AWS emits for its own precondition
/// ([`NO_RECORDER_TO_PUT_CONFIG_RULE`], [`NO_RECORDER_TO_PUT_DELIVERY_CHANNEL`]), so
/// a client can tell from the message which precondition failed.
pub const NO_AVAILABLE_CONFIGURATION_RECORDER_MESSAGE: &str =
    "there are no customer managed configuration recorders available; use \
     PutConfigurationRecorder to create one";

/// How Config words `NoRunningConfigurationRecorderException` for the operation `api`
/// that records a resource's configuration state.
///
/// ONE template for the two operations that record the configuration state of a
/// custom resource — `PutResourceConfig` and `DeleteResourceConfig` — so the two
/// halves of that lifecycle cannot report the one precondition two ways. The sentence
/// is what AWS answers each of them with: it states the precondition and names the API
/// the caller reached for, which is the only part that differs between them. The
/// shape's own doc ("There is no configuration recorder running") states the CONDITION
/// rather than the message, so it is not the text to emit here. `DeliverConfigSnapshot`
/// keeps its own wording: AWS publishes no text for that operation's refusal, so there
/// the requirement is only the status and the error code.
pub fn no_running_configuration_recorder_message(api: &str) -> String {
    format!("You must create and start a configuration recorder before you call the {api} API.")
}

/// The separator between the two fields of the key a pending aggregation request page
/// resumes at. An `AccountId` is twelve digits (`PendingAggregationRequest$
/// RequesterAccountId`, pattern `^\d{12}$`), so it never contains this character and
/// the key splits unambiguously.
const PENDING_AGGREGATION_REQUEST_KEY_SEPARATOR: char = '#';

/// Encode the (account, Region) key a `DescribePendingAggregationRequests` page should
/// resume at as an opaque `NextToken`.
pub fn encode_pending_aggregation_request_token(key: &(String, String)) -> String {
    encode_page_token(&format!(
        "{}{PENDING_AGGREGATION_REQUEST_KEY_SEPARATOR}{}",
        key.0, key.1
    ))
}

/// The (account, Region) key a `NextToken` resumes at, or `None` when the token is
/// malformed — i.e. not something [`encode_pending_aggregation_request_token`]
/// produced, so it does not decode to an account id of twelve digits and a non-empty
/// Region.
pub fn decode_pending_aggregation_request_token(token: &str) -> Option<(String, String)> {
    let key = decode_page_token(token)?;
    let (account, region) = key.split_once(PENDING_AGGREGATION_REQUEST_KEY_SEPARATOR)?;
    let well_formed = is_account_id(account) && !region.is_empty();
    well_formed.then(|| (account.to_string(), region.to_string()))
}

/// Encode `fields` as one opaque page token.
///
/// Every field is LENGTH-PREFIXED rather than joined on a separator: a `ResourceType`
/// (`StringWithCharLimit256`), a `ResourceId` (`BaseResourceId`) and a rule name are
/// unconstrained strings, so no character can be claimed as a separator without a value
/// that contains it splitting the key wrongly. The ONE place that encoding is written,
/// so the paged reads that resume at a multi-field key cannot drift apart.
fn encode_length_prefixed_token(fields: &[&str]) -> String {
    let mut encoded = String::new();
    for field in fields {
        encoded.push_str(&format!("{}:{field}", field.len()));
    }
    encode_page_token(&encoded)
}

/// The `count` fields a length-prefixed token carries, or `None` when the token is
/// malformed — i.e. not something [`encode_length_prefixed_token`] produced for that
/// many fields, so its length-prefixed fields do not account for exactly its decoded
/// bytes.
fn decode_length_prefixed_token(token: &str, count: usize) -> Option<Vec<String>> {
    let decoded = decode_page_token(token)?;
    let mut rest = decoded.as_str();
    let mut fields: Vec<String> = Vec::with_capacity(count);
    for _ in 0..count {
        let (length, tail) = rest.split_once(':')?;
        let length: usize = length.parse().ok()?;
        fields.push(tail.get(..length)?.to_string());
        rest = tail.get(length..)?;
    }
    rest.is_empty().then_some(fields)
}

/// Encode the [`EvaluationResultKey`] a `GetComplianceDetailsByConfigRule` page should
/// resume at as an opaque `NextToken`.
pub fn encode_evaluation_result_token(key: &EvaluationResultKey) -> String {
    encode_length_prefixed_token(&[&key.0, &key.1, &key.2])
}

/// The [`EvaluationResultKey`] a `NextToken` resumes at, or `None` when the token is
/// malformed — i.e. not something [`encode_evaluation_result_token`] produced.
pub fn decode_evaluation_result_token(token: &str) -> Option<EvaluationResultKey> {
    let mut fields = decode_length_prefixed_token(token, 3)?.into_iter();
    Some((fields.next()?, fields.next()?, fields.next()?))
}

/// What identifies one evaluation result within ONE RESOURCE's evaluation details: the
/// Config rule that recorded it and the mode that rule was evaluated in.
///
/// The resource is fixed by the request a `GetComplianceDetailsByResource` report
/// answers, so this pair is what the results of one resource differ along; ordering it
/// gives the report a stable total order a page can resume at.
pub type ResourceComplianceDetailKey = (String, String);

/// Encode the [`ResourceComplianceDetailKey`] a `GetComplianceDetailsByResource` page
/// should resume at as an opaque `NextToken`.
pub fn encode_resource_compliance_detail_token(key: &ResourceComplianceDetailKey) -> String {
    encode_length_prefixed_token(&[&key.0, &key.1])
}

/// The [`ResourceComplianceDetailKey`] a `NextToken` resumes at, or `None` when the
/// token is malformed — i.e. not something
/// [`encode_resource_compliance_detail_token`] produced.
pub fn decode_resource_compliance_detail_token(
    token: &str,
) -> Option<ResourceComplianceDetailKey> {
    let mut fields = decode_length_prefixed_token(token, 2)?.into_iter();
    Some((fields.next()?, fields.next()?))
}

/// What identifies one evaluation result within a CONFORMANCE PACK's compliance
/// details: the Config rule the pack deployed that recorded it, and that result's own
/// identity within the rule ([`EvaluationResultKey`]).
///
/// A pack's details span every rule it deployed, so the rule name is part of the key;
/// ordering the pair gives the report a stable total order a page can resume at.
pub type ConformancePackComplianceDetailKey = (String, EvaluationResultKey);

/// Encode the [`ConformancePackComplianceDetailKey`] a
/// `GetConformancePackComplianceDetails` page should resume at as an opaque `NextToken`.
pub fn encode_conformance_pack_compliance_detail_token(
    key: &ConformancePackComplianceDetailKey,
) -> String {
    let (rule_name, (resource_type, resource_id, evaluation_mode)) = key;
    encode_length_prefixed_token(&[rule_name, resource_type, resource_id, evaluation_mode])
}

/// The [`ConformancePackComplianceDetailKey`] a `NextToken` resumes at, or `None` when
/// the token is malformed — i.e. not something
/// [`encode_conformance_pack_compliance_detail_token`] produced.
pub fn decode_conformance_pack_compliance_detail_token(
    token: &str,
) -> Option<ConformancePackComplianceDetailKey> {
    let mut fields = decode_length_prefixed_token(token, 4)?.into_iter();
    Some((
        fields.next()?,
        (fields.next()?, fields.next()?, fields.next()?),
    ))
}

/// What identifies one resource in a report ABOUT resources: the
/// `(resourceType, resourceId)` pair that identifies a resource in Config, which is
/// also the key [`resource_compliance`] reports a resource under and therefore the
/// ordering a `DescribeComplianceByResource` page resumes at.
pub type ComplianceByResourceKey = (String, String);

/// Encode the [`ComplianceByResourceKey`] a `DescribeComplianceByResource` page should
/// resume at as an opaque `NextToken`.
pub fn encode_compliance_by_resource_token(key: &ComplianceByResourceKey) -> String {
    encode_length_prefixed_token(&[&key.0, &key.1])
}

/// The [`ComplianceByResourceKey`] a `NextToken` resumes at, or `None` when the token is
/// malformed — i.e. not something [`encode_compliance_by_resource_token`] produced.
pub fn decode_compliance_by_resource_token(token: &str) -> Option<ComplianceByResourceKey> {
    let mut fields = decode_length_prefixed_token(token, 2)?.into_iter();
    Some((fields.next()?, fields.next()?))
}

/// Encode the resource key a `DescribeRemediationExecutionStatus` page should resume at
/// as an opaque `NextToken`.
pub fn encode_remediation_execution_token(resume_at: &(String, String)) -> String {
    encode_length_prefixed_token(&[&resume_at.0, &resume_at.1])
}

/// The resource key a `NextToken` resumes at, or `None` when the token is malformed —
/// i.e. not something [`encode_remediation_execution_token`] produced.
pub fn decode_remediation_execution_token(token: &str) -> Option<(String, String)> {
    let mut fields = decode_length_prefixed_token(token, 2)?.into_iter();
    Some((fields.next()?, fields.next()?))
}

/// Encode the resource key a `DescribeRemediationExceptions` page should resume at as an
/// opaque `NextToken`.
pub fn encode_remediation_exception_token(resume_at: &(String, String)) -> String {
    encode_length_prefixed_token(&[&resume_at.0, &resume_at.1])
}

/// The resource key a `NextToken` resumes at, or `None` when the token is malformed —
/// i.e. not something [`encode_remediation_exception_token`] produced.
pub fn decode_remediation_exception_token(token: &str) -> Option<(String, String)> {
    let mut fields = decode_length_prefixed_token(token, 2)?.into_iter();
    Some((fields.next()?, fields.next()?))
}

/// The most evaluation results one Config rule's record keeps
/// ([`ConfigRuleEntry::record_evaluation_result`] enforces it).
///
/// Config publishes no quota for evaluation results per rule, and a delivery carries at
/// most a hundred of them (`PutEvaluationsRequest$Evaluations`, length 0..100), so one
/// delivery's worth is what a rule keeps: the hundred most recently evaluated
/// resources. Without a bound, repeated deliveries naming new resource ids would grow
/// this process's memory without end.
pub const EVALUATION_RESULTS_KEPT_PER_RULE: usize = 100;

/// Every `ComplianceType` an evaluation DELIVERED to Config may carry: the published
/// values minus `INSUFFICIENT_DATA`, which Config derives for a rule it could not
/// evaluate rather than accepting from a caller (`Evaluation$ComplianceType`: "For the
/// Evaluation data type, Config supports only the COMPLIANT, NON_COMPLIANT, and
/// NOT_APPLICABLE values. … Config does not accept INSUFFICIENT_DATA as the value for
/// ComplianceType from a PutEvaluations request").
pub const DELIVERED_EVALUATION_COMPLIANCE_TYPES: [&str; 3] = [
    COMPLIANCE_TYPE_COMPLIANT,
    COMPLIANCE_TYPE_NON_COMPLIANT,
    COMPLIANCE_TYPE_NOT_APPLICABLE,
];

/// The `ResultToken` Config mints when it invokes the evaluation of the rule
/// `config_rule_name` at `invoked_at`.
///
/// The token "identifies the rule and the event that triggered the evaluation"
/// (`PutEvaluationsRequest$ResultToken`), so it is built from exactly those two: the
/// rule's name and the instant of the invocation it belongs to, encoded opaquely with
/// the codec this service's other tokens use. The rule name is LENGTH-PREFIXED because
/// a `ConfigRuleName` is otherwise unconstrained in its characters, so no separator can
/// be claimed as one.
pub fn mint_result_token(config_rule_name: &str, invoked_at: SystemTime) -> String {
    let since_epoch = invoked_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    encode_page_token(&format!(
        "{}:{config_rule_name}{since_epoch}",
        config_rule_name.len()
    ))
}

/// The Config rule a `ResultToken` identifies, or `None` when the value is not a token
/// [`mint_result_token`] produced — i.e. it does not decode to a length-prefixed rule
/// name followed by the digits of an instant.
pub fn result_token_rule(token: &str) -> Option<String> {
    let decoded = decode_page_token(token)?;
    let (length, rest) = decoded.split_once(':')?;
    let length: usize = length.parse().ok()?;
    let name = rest.get(..length)?;
    let invoked_at = rest.get(length..)?;
    if name.is_empty() || invoked_at.is_empty() || !invoked_at.bytes().all(|at| at.is_ascii_digit())
    {
        return None;
    }
    Some(name.to_string())
}

/// How Config words `InvalidParameterValueException` for a member whose value is not
/// one that member accepts.
///
/// The message is BUILT FROM THE REQUEST — it quotes the value the caller sent, names
/// the member and lists the values that member accepts — so every operation that
/// refuses an out-of-range value words the refusal the same way.
pub fn unaccepted_value_message(member: &str, value: &str, accepted: &[&str]) -> String {
    format!(
        "'{value}' is not a valid value for {member}; the valid values are {}.",
        accepted.join(", ")
    )
}

/// How Config words `InvalidParameterValueException` for a results filter that named a
/// `ComplianceType` no filter may name.
///
/// The message is BUILT FROM THE REQUEST — it quotes the value the caller sent, names
/// the member and lists the values that member accepts
/// ([`FILTERABLE_COMPLIANCE_TYPES`]).
pub fn unfilterable_compliance_type_message(member: &str, value: &str) -> String {
    unaccepted_value_message(member, value, &FILTERABLE_COMPLIANCE_TYPES)
}

/// How a resource-scoped evaluation-details read words `InvalidParameterValueException`
/// for a request that did not say WHICH resource it is about.
///
/// A resource is named either by the execution that evaluated it or by its own type and
/// id, and nothing else identifies one, so a request carrying neither is refused naming
/// both ways ("You need to only provide either a ResourceEvaluationID or a ResourceID
/// and ResourceType", `GetComplianceDetailsByResourceRequest$ResourceEvaluationId`).
pub const RESOURCE_NOT_IDENTIFIED_MESSAGE: &str =
    "Either ResourceEvaluationId or both ResourceType and ResourceId must be provided.";

/// How a compliance report words `InvalidParameterValueException` for a request that
/// named a resource id without the resource type that completes it.
///
/// A resource is identified by its type AND its id, so an id on its own names no
/// resource. AWS publishes no text for this refusal, so the sentence is the REQUIREMENT
/// as the member that carries the id states it ("You can specify only one resource ID.
/// If you specify a resource ID, you must also specify a type for ResourceType",
/// `DescribeComplianceByResourceRequest$ResourceId`), which names both members a caller
/// has to fix. FIXED, and it carries no part of the request: the id it was sent with is
/// already in the caller's hand.
pub const RESOURCE_TYPE_REQUIRED_MESSAGE: &str =
    "If you specify a resource ID, you must also specify a type for ResourceType.";

/// How Config words `NoSuchConfigRuleException` for a request that named a rule this
/// account does not hold.
///
/// The message is BUILT FROM THE REQUEST — it quotes the name the caller sent and
/// names the member to check — so every operation that resolves a rule it is to ACT on
/// words it the same way, naming the one name it could not resolve however many the
/// request carried: a caller that submitted several learns which of them to fix. The
/// LIST-wide form [`NO_SUCH_CONFIG_RULES`] belongs to the reads that answer about the
/// list as a whole (`DescribeConfigRules`, `DescribeComplianceByConfigRule`), which
/// report on every name at once and so name none of them.
pub fn no_such_config_rule_message(name: &str) -> String {
    format!(
        "The ConfigRule '{name}' provided in the request is invalid. Please check the \
         configRule name."
    )
}

/// How the resource-evaluation path words `InvalidParameterValueException` for a
/// request that asked for an `EvaluationMode` it does not accept.
///
/// The message is BUILT FROM THE REQUEST — it names the mode the caller asked for and
/// the modes this path supports (`PROACTIVE` alone, per
/// `StartResourceEvaluationRequest$EvaluationMode` and
/// `ResourceEvaluationFilters$EvaluationMode`) — so every operation whose
/// `EvaluationMode` is documented as `PROACTIVE`-only words the refusal the same way.
pub fn unsupported_evaluation_mode_message(requested: &str) -> String {
    format!(
        "{requested} Evaluation Mode is not supported. The supported Evaluation Modes \
         are: [{EVALUATION_MODE_PROACTIVE}]."
    )
}

/// How Config words `NoSuchConfigRuleException` for a READ that answers about a list of
/// rules (`ConfigRuleNames`), one of which this account does not hold.
///
/// Such a read reports on every name at once, so its refusal is about the list as a
/// whole: it reports the plural form, names the member to check and quotes no name,
/// leaving the caller to compare the list it sent against the rules it holds. An
/// operation that resolves the rules it is to ACT on names the one it could not resolve
/// instead — see [`no_such_config_rule_message`].
pub const NO_SUCH_CONFIG_RULES: &str = "One or more ConfigRules provided in the request are \
                                        invalid. Please check the configRule names.";

/// Which member a request named one configuration recorder BY, which is what the
/// refusal that cannot find it states: a caller is told to verify the input it actually
/// sent.
///
/// The selectors are the ones the recorder operations take —
/// `ConfigurationRecorderName`/`ConfigurationRecorderNames`, `Arn` and
/// `ServicePrincipal` (`DescribeConfigurationRecorders`,
/// `DeleteServiceLinkedConfigurationRecorder`) — and a service-linked recorder, whose
/// name is not the caller's to choose, is addressed only by the last two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderSelector {
    /// The request named the recorder by name.
    Name,
    /// The request named the recorder by ARN.
    Arn,
    /// The request named the service the recorder is linked to.
    ServicePrincipal,
}

impl RecorderSelector {
    /// What a refusal calls this selector: the member the caller supplied, in the words
    /// the refusal uses for it.
    fn member(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Arn => "arn",
            Self::ServicePrincipal => "service principal",
        }
    }

    /// How a refusal that states WHICH recorder of this account was looked for names the
    /// member the selector came from.
    fn quoted_member(self) -> &'static str {
        match self {
            Self::Name => "RecorderName",
            Self::Arn => "RecorderArn",
            Self::ServicePrincipal => "Service Principal",
        }
    }
}

/// How Config words `NoSuchConfigurationRecorderException` for a lookup by `selector`.
///
/// The message is built from the SELECTOR the request carried rather than from the
/// value it sent: it names the member the caller has to verify, so a lookup by ARN
/// cannot be refused with the wording of a lookup by name — which is unactionable for a
/// service-linked recorder, addressed only by ARN or by service principal. The
/// service-principal wording is the sentence
/// [`UNRECOGNIZED_RECORDER_SERVICE_PRINCIPAL_MESSAGE`] already carries in the refusal
/// class that one belongs to; the arn wording ends in the trailing space the recorded
/// arn-variant refusal carries.
pub fn no_such_configuration_recorder_message(selector: RecorderSelector) -> String {
    let member = selector.member();
    let trailing = if matches!(selector, RecorderSelector::Arn) {
        " "
    } else {
        ""
    };
    format!(
        "The {member} of the configuration recorder you entered cannot be found. Verify \
         the configuration recorder {member}, and try again.{trailing}"
    )
}

/// How the SERVICE-LINKED recorder path words `NoSuchConfigurationRecorderException`
/// for a recorder it cannot resolve from `selector` carrying `value`.
///
/// `DeleteServiceLinkedConfigurationRecorder` resolves ONE recorder of ONE account from
/// the selector the request carried, so its refusal states what it looked for and where
/// — the account and the selector's own value — rather than telling the caller to verify
/// a member, which is how [`no_such_configuration_recorder_message`] words a lookup that
/// takes several selectors at once. Recorded behaviour on the ARN and the
/// service-principal selectors; each selector names the member the request carried, in
/// the words the refusal spells it with ([`RecorderSelector::quoted_member`]).
pub fn service_linked_recorder_not_found_message(
    selector: RecorderSelector,
    value: &str,
) -> String {
    format!(
        "Configuration recorder (AccountId: {}, {}: {value}) cannot be found",
        ::emulator_server::emulator_account_id(),
        selector.quoted_member(),
    )
}

/// How `PutConfigurationRecorder` words `InvalidConfigurationRecorderNameException`
/// for a name carrying the prefix Config reserves for the recorders it creates itself
/// ([`SERVICE_LINKED_RECORDER_NAME_PREFIX`]).
///
/// The MODEL STATES THIS MESSAGE, so the text is part of the contract rather than
/// wording this emulator is free to choose: it is
/// `InvalidConfigurationRecorderNameException`'s own documentation ("The configuration
/// recorder name is not valid. The prefix \"AWSConfigurationRecorderFor\" is reserved
/// for service-linked configuration recorders"), which the service itself renders as
/// ONE sentence with the prefix unquoted — the form the recorded refusal carries. It is
/// request-independent: it names the reserved prefix, not the name the caller sent,
/// which is the only thing that was wrong.
pub fn reserved_recorder_name_prefix_message() -> String {
    format!(
        "The configuration recorder name is not valid, the prefix \
         {SERVICE_LINKED_RECORDER_NAME_PREFIX} is reserved for service-linked configuration \
         recorders."
    )
}

/// How `StartConfigurationRecorder` and `StopConfigurationRecorder` word
/// `UnmodifiableEntityException` for a SERVICE-LINKED configuration recorder.
///
/// The MODEL STATES THIS MESSAGE, verbatim, for exactly these two operations
/// (`UnmodifiableEntityException`: "For StartConfigurationRecorder and
/// StopConfigurationRecorder, you will see this exception because these operations do
/// not affect service-linked configuration recorders. Service-linked configuration
/// recorders are ALWAYS RECORDING. To stop recording, you must delete the
/// service-linked configuration recorder. Use the
/// DeleteServiceLinkedConfigurationRecorder operation to delete a service-linked
/// configuration recorder"). The property the refusal is about is that the recorder
/// always records — so the call is not permitted for this recorder class, rather than a
/// call that had no effect — and the recovery it indicates is the service-linked delete.
///
/// A FIXED sentence for the class: it quotes nothing the caller sent, because what is
/// wrong is the KIND of recorder named, not its name.
pub const SERVICE_LINKED_RECORDER_ALWAYS_RECORDING: &str =
    "This operation does not affect service-linked configuration recorders. \
     Service-linked configuration recorders are always recording. To stop recording, \
     you must delete the service-linked configuration recorder. Use the \
     DeleteServiceLinkedConfigurationRecorder operation to delete a service-linked \
     configuration recorder.";

/// How `PutConfigurationRecorder` words `UnmodifiableEntityException` for a request
/// asking it for a service-linked configuration recorder — also stated by the model
/// ("For PutConfigurationRecorder, you will see this exception because you cannot use
/// this operation to create a service-linked configuration recorder. Use the
/// PutServiceLinkedConfigurationRecorder operation to create a service-linked
/// configuration recorder"), and fixed for the same reason as
/// [`SERVICE_LINKED_RECORDER_ALWAYS_RECORDING`].
pub const PUT_RECORDER_IS_NOT_SERVICE_LINKED: &str =
    "You cannot use this operation to create a service-linked configuration recorder. \
     Use the PutServiceLinkedConfigurationRecorder operation to create a service-linked \
     configuration recorder.";

/// How `DeleteConfigurationRecorder` words `UnmodifiableEntityException` for a
/// service-linked configuration recorder — the model's own sentence again ("For
/// DeleteConfigurationRecorder, you will see this exception because you cannot use this
/// operation to delete a service-linked configuration recorder. Use the
/// DeleteServiceLinkedConfigurationRecorder operation to delete a service-linked
/// configuration recorder").
pub const DELETE_RECORDER_IS_NOT_SERVICE_LINKED: &str =
    "You cannot use this operation to delete a service-linked configuration recorder. \
     Use the DeleteServiceLinkedConfigurationRecorder operation to delete a \
     service-linked configuration recorder.";

/// How `PutConfigRule` words `NoAvailableConfigurationRecorderException`.
///
/// A FIXED message for this operation's precondition: the class is shared with
/// `PutDeliveryChannel` (see [`NO_RECORDER_TO_PUT_DELIVERY_CHANNEL`]), and each
/// operation states the precondition that failed rather than the class, so a client
/// can tell the two apart from the message alone.
pub const NO_RECORDER_TO_PUT_CONFIG_RULE: &str =
    "You must create a configuration recorder before you can create or update a Config rule.";

/// How `PutDeliveryChannel` words `NoAvailableConfigurationRecorderException`. See
/// [`NO_RECORDER_TO_PUT_CONFIG_RULE`] for why the two differ.
pub const NO_RECORDER_TO_PUT_DELIVERY_CHANNEL: &str =
    "Configuration recorder is not available to put delivery channel.";

/// How `StartConfigurationRecorder` words `NoAvailableDeliveryChannelException`: a
/// FIXED message naming the precondition that failed.
pub const NO_DELIVERY_CHANNEL_TO_START_RECORDER: &str =
    "Delivery channel is not available to start configuration recorder.";

/// How Config words `NoSuchDeliveryChannelException` for a request that named a
/// delivery channel this account does not hold.
///
/// BUILT FROM THE REQUEST — it quotes the channel name the caller sent — so every
/// operation that refuses an absent channel words it the same way.
pub fn no_such_delivery_channel_message(name: &str) -> String {
    format!("Cannot find delivery channel with specified name '{name}'.")
}

/// How Config words `LastDeliveryChannelDeleteFailedException`: the delete of the
/// account's last delivery channel while its configuration recorder is running.
///
/// BUILT FROM THE REQUEST — it quotes the channel name the caller asked to delete.
pub fn last_delivery_channel_delete_failed_message(name: &str) -> String {
    format!(
        "Failed to delete last specified delivery channel with name '{name}', because there \
         is a running configuration recorder."
    )
}

/// How Config words `MaxNumberOfConfigurationRecordersExceededException`.
///
/// The message states THE SERVICE'S OWN LIMIT ([`CUSTOMER_MANAGED_RECORDERS_QUOTA`])
/// and nothing of the request or the environment: naming the Region would make one
/// account's refusal read differently from another's for the same cause. The two
/// spaces after "recorder" are the service's own rendering (it interpolates an empty
/// recorder designation there), not a typo.
pub fn max_configuration_recorders_message() -> String {
    format!(
        "Failed to put configuration recorder  because you have reached the limit for the \
         maximum number of customer managed configuration records: \
         ({CUSTOMER_MANAGED_RECORDERS_QUOTA})"
    )
}

/// How Config words `NoSuchRemediationConfigurationException` for a request naming a
/// Config rule that holds no remediation configuration — none at all, or none for the
/// `resource_type` the request named.
///
/// The message is BUILT FROM THE REQUEST — it carries the `ConfigRuleName` the caller
/// sent, and the `ResourceType` when the caller named one, so the two refusals are
/// told apart — for the case the modelled error documents ("You specified an Config
/// rule without a remediation configuration"). The rule's name is interpolated UNQUOTED
/// and the sentence carries no trailing stop, which is how Config words it (the sibling
/// refusal of a rule holding no remediation configuration,
/// `ops::put_remediation_exceptions`, carries its name the same way).
pub fn no_such_remediation_configuration_message(
    name: &str,
    resource_type: Option<&str>,
) -> String {
    match resource_type {
        None => format!("No RemediationConfiguration for rule {name} exists"),
        Some(resource_type) => format!(
            "No RemediationConfiguration for rule {name} and resource type {resource_type} \
             exists"
        ),
    }
}

/// How Config words `NoSuchRetentionConfigurationException` for a request that named
/// a retention configuration this account does not hold.
///
/// The message is BUILT FROM THE REQUEST — it quotes the name the caller sent — so
/// every operation that refuses an absent retention configuration words it the same
/// way.
pub fn no_such_retention_configuration_message(name: &str) -> String {
    format!("Cannot find retention configuration with specified name '{name}'.")
}

/// How Config words `MaxNumberOfRetentionConfigurationsExceededException`: the write
/// would take the account past [`RETENTION_CONFIGURATIONS_QUOTA`] retention
/// configurations in this Region.
pub fn retention_configurations_quota_message() -> String {
    format!(
        "Failed to add the retention configuration because this account already holds \
         the maximum of {RETENTION_CONFIGURATIONS_QUOTA} retention configuration(s) in \
         this region."
    )
}

/// How Config words `ResourceNotDiscoveredException` for an aggregate resource
/// identifier the named aggregator holds no current configuration item for.
///
/// FIXED, and it carries NO part of the request: the identifier the read asked about is
/// not echoed, so the refusal reads the same across every identifier a caller tries. It
/// is the sentence the error shape itself is documented with ("You have specified a
/// resource that is either unknown or has not been discovered").
pub const RESOURCE_NOT_DISCOVERED_MESSAGE: &str =
    "You have specified a resource that is either unknown or has not been discovered.";

/// How Config words `NoSuchConfigurationAggregatorException` for a request that named
/// an aggregator this account does not hold.
///
/// FIXED, and it carries NO part of the request: the name the caller sent is not
/// echoed, so a refusal tells a caller only that the aggregator it named is not one of
/// this account's. Every operation that models this error answers this one sentence,
/// which is what lets a client tell this refusal from the other client errors those
/// operations share.
pub const NO_SUCH_CONFIGURATION_AGGREGATOR_MESSAGE: &str =
    "The configuration aggregator does not exist. Check the configuration aggregator \
     name and try again.";

/// How Config words `NoSuchRemediationExceptionException` ("You tried to delete a
/// remediation exception that does not exist").
///
/// RECORDED behaviour, and it carries no part of the request: the refusal names the
/// resource class rather than echoing the rule and resource keys the caller sent, which
/// is what the recorded refusal of a delete of an exception no rule holds answers.
pub const NO_SUCH_REMEDIATION_EXCEPTION_MESSAGE: &str =
    "Specified RemediationException could not be found.";

// This module's own unit tests, in `state/tests.rs`.
#[cfg(test)]
mod tests;

// One stored entry per resource of this service, each with the projections its read
// paths report it through, in a file of its own: a Config rule and the evaluation
// results it holds, a delivery channel and the snapshot delivery last scheduled for it,
// a discovered resource and its configuration items, a configuration aggregator, and a
// conformance pack. Each is re-exported here like the modules above, so a handler
// reaches its names as `crate::state::<name>`.
mod aggregator;
mod config_rule;
mod conformance_pack;
mod delivery_channel;
mod discovered_resource;
// How the ids above are DRAWN, in one place, so no mint derives one from a counter.
mod identifier;
pub use aggregator::*;
pub use config_rule::*;
pub use conformance_pack::*;
pub use delivery_channel::*;
pub use discovered_resource::*;
pub use identifier::*;
