// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda service.
//!
//! The collaboration/membership lifecycle is persisted here and every response is
//! computed from it. The two stored records are deliberately split:
//!
//! * [`CollaborationRecord`] holds the collaboration as the CREATOR declared it —
//!   including the creator's own abilities and the immutable `members` list, plus each
//!   party's own member timestamps, since a member's `createTime`/`updateTime` move with
//!   that party's grant rather than with the collaboration. The
//!   modelled `Collaboration` shape is NOT stored, because two of its members
//!   (`memberStatus`, `membershipId`/`membershipArn`) describe the CALLER's relation
//!   to the collaboration rather than the collaboration itself, so they are derived
//!   per request by [`CleanRoomsState::collaboration_view`]. Which parties the creator
//!   has REMOVED is held there too (`removed_members`), because a removal takes a party's
//!   authorization to the collaboration away without taking its grant out of the
//!   `members` list — see [`CleanRoomsState::remove_member`].
//! * the modelled [`Membership`] shape IS stored as-is: every one of its members is
//!   absolute, so there is nothing to derive.
//! * the modelled [`AnalysisTemplate`] shape is likewise stored as-is, under the
//!   membership it was created for; `analysis_template_by_name` is the index that makes
//!   a repeat create under one name a conflict rather than a second resource.
//!
//! * the modelled [`ConfiguredTable`] is likewise stored as-is: a configured table is an
//!   account-level resource, so none of its members describes the caller. The analysis
//!   rule a configured table holds is stored the same way, as the modelled
//!   [`ConfiguredTableAnalysisRule`], keyed by the `(configured table id, analysis rule
//!   type)` pair the analysis-rule operations address it by; which types a table holds a
//!   rule for stays on the table, in `ConfiguredTable$analysisRuleTypes`, so
//!   [`CleanRoomsState::analysis_rule_of_configured_table`] (the record a further create
//!   collides with) is a lookup rather than a scan.
//!
//! Insertion order is kept in `collaboration_order` / `membership_order` /
//! `analysis_template_order` / `protected_query_order` / `configured_table_order` so
//! the list operations page deterministically and a pagination token stays valid across
//! calls.
//!
//! The privacy budget templates a collaboration holds are stored the same way the
//! modelled [`Membership`] is — as the modelled `PrivacyBudgetTemplate`, because every
//! one of its members is absolute — plus the
//! `privacy_budget_template_by_collaboration` index, keyed by the collaboration AND the
//! budget type, that makes "one template per `PrivacyBudgetType`" a lookup rather than a
//! scan.
//!
//! A configured table association is stored the same way, under the membership it was
//! created for, with `configured_table_association_by_name` as the index that makes a
//! repeat create under one table alias a conflict rather than a second resource. The
//! analysis rule an association holds is stored the same way the configured table's is —
//! as the modelled [`ConfiguredTableAssociationAnalysisRule`], keyed by the `(configured
//! table association id, analysis rule type)` pair the association analysis-rule
//! operations address it by — and which types an association holds a rule for stays on
//! the association, in `ConfiguredTableAssociation$analysisRuleTypes`.
//!
//! A collaboration change request is likewise stored as the modelled
//! [`CollaborationChangeRequest`]; what the service COMPUTES about one — the change
//! types a specification asks for, whether the collaboration auto-approves them, and the
//! parties whose approval it needs — is derived from the stored collaboration by
//! [`change_types_for`], [`change_request_is_auto_approved`] and
//! [`CleanRoomsState::change_request_approvals`], and a committed change is written back
//! onto the collaboration by [`CleanRoomsState::apply_collaboration_change`]. The status
//! a request's recorded votes put it in is derived the same way, by
//! [`change_request_status_from_approvals`], and [`change_request_is_terminal`] says
//! which statuses admit no modification at all.
//!
//! Tags are the one thing here that belongs to no single kind of resource: `tags` is
//! keyed by resource ARN, so the tagging operations read and write the same entry a
//! create filled in, whichever kind of resource the ARN names
//! ([`CleanRoomsState::holds_resource_at_arn`] is what resolves one, and
//! [`resource_type_of_arn`] what an ARN that resolves to nothing is reported as).

use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AccessBudgetsPrivacyTemplateParametersOutput, AnalysisParameter, AnalysisSchema, AnalysisSource,
    AnalysisTemplate, AnalysisTemplateSummary, ApprovalStatusDetails, Change, ChangeSpecification,
    Collaboration, CollaborationChangeRequest, CollaborationSummary, Column, ComputeConfiguration,
    ConfiguredTable, ConfiguredTableAnalysisRule, ConfiguredTableAnalysisRulePolicy,
    ConfiguredTableAnalysisRulePolicyV1, ConfiguredTableAssociation,
    ConfiguredTableAssociationAnalysisRule, ConfiguredTableAssociationAnalysisRulePolicy,
    ConfiguredTableAssociationAnalysisRulePolicyV1, ConfiguredTableAssociationSchemaTypeProperties,
    ConfiguredTableAssociationSummary, DataEncryptionMetadata,
    DifferentialPrivacyTemplateParametersOutput, ErrorMessageConfiguration, MLMemberAbilities,
    MemberChangeSpecification, MemberSpecification, MemberSummary, Membership,
    MembershipJobComputePaymentConfig, MembershipMLPaymentConfig,
    MembershipModelInferencePaymentConfig, MembershipModelTrainingPaymentConfig,
    MembershipPaymentConfiguration, MembershipProtectedJobResultConfiguration,
    MembershipProtectedQueryOutputConfiguration, MembershipProtectedQueryResultConfiguration,
    MembershipQueryComputePaymentConfig, MembershipSummary,
    MembershipSyntheticDataGenerationPaymentConfig, PaymentConfiguration, PrivacyBudgetTemplate,
    PrivacyBudgetTemplateParametersInput, PrivacyBudgetTemplateParametersOutput,
    PrivacyBudgetTemplateSummary, ProtectedQuery,
    ProtectedQueryError, ProtectedQueryOutputConfiguration, ProtectedQueryResultConfiguration,
    QueryComputePaymentConfig, ResourceNotFoundException, Schema, SchemaStatusDetail,
    SchemaStatusReason, SchemaTypeProperties, SnowflakeTableSchema, SyntheticDataParameters,
    TableReference, WorkerComputeConfiguration,
};

/// The schemas a collaboration holds: the relations its members contributed to it,
/// derived from the configured table associations and configured tables already stored
/// here rather than persisted a second time.
mod schemas;
pub use schemas::*;

/// The ARN service segment and the SigV4 signing name for Clean Rooms, from
/// `OPERATIONS.md` (`sigv4 name cleanrooms`) and the `CollaborationArn` /
/// `MembershipArn` patterns in the model.
pub const ARN_SERVICE: &str = "cleanrooms";

/// The region used when the request carries no usable one. The model's
/// `CollaborationArn`/`MembershipArn` patterns pin the ARN's region segment to
/// `[\w]{2}-[\w]{4,9}-[\d]`, so a scope region that cannot appear in one of those ARNs
/// is replaced by this emulator default rather than emitted.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `MemberStatus` values this slice produces (model shape `MemberStatus`:
/// INVITED | ACTIVE | LEFT | REMOVED).
pub const MEMBER_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`MEMBER_STATUS_ACTIVE`].
pub const MEMBER_STATUS_INVITED: &str = "INVITED";
/// See [`MEMBER_STATUS_ACTIVE`]. The status a party the collaboration's creator has
/// removed is reported in — `DeleteMember` "removes the specified member from a
/// collaboration. The removed member is placed in the Removed status and can't interact
/// with the collaboration".
pub const MEMBER_STATUS_REMOVED: &str = "REMOVED";

/// The `isMetricsEnabled` a create stores when the request omits it.
///
/// Documented on both `CreateCollaborationInput$isMetricsEnabled` and
/// `CreateMembershipInput$isMetricsEnabled`: "The default value is `false`." The
/// service, not the caller, decides it, so the created resource reports it rather than
/// leaving the member absent.
pub const DEFAULT_IS_METRICS_ENABLED: bool = false;

/// The `analyticsEngine` a collaboration created without one runs on.
///
/// The model's `AnalyticsEngine` enum declares `SPARK | CLEAN_ROOMS_SQL`, and
/// `CreateCollaborationInput$analyticsEngine` notes "After July 16, 2025, the
/// `CLEAN_ROOMS_SQL` parameter will no longer be available" — so `SPARK` is the engine
/// a collaboration gets when the caller selects none.
pub const DEFAULT_ANALYTICS_ENGINE: &str = "SPARK";

/// Whether a membership accepts a machine-learning cost the collaboration creator
/// assigned to nobody.
///
/// `QueryComputePaymentConfig$isResponsible` makes "the member who can query" the
/// default payer for query compute, and the same rule gives job compute to the member
/// with `CAN_RUN_JOB`; no ability in the `MemberAbility` catalog (CAN_QUERY |
/// CAN_RECEIVE_RESULTS | CAN_RUN_JOB | CAN_EXPORT_QUERY_ANALYSIS_LOG) nor in the
/// `CustomMLMemberAbility` catalog (CAN_RECEIVE_MODEL_OUTPUT |
/// CAN_RECEIVE_INFERENCE_OUTPUT) runs model training, model inference or synthetic data
/// generation, so nothing makes a member the default payer for an ML cost: an ML cost
/// the creator assigned nobody is accepted as not-responsible.
pub const DEFAULT_ML_IS_RESPONSIBLE: bool = false;

/// `ProtectedQueryStatus` values this slice produces (model shape
/// `ProtectedQueryStatus`: SUBMITTED | STARTED | CANCELLED | CANCELLING | FAILED |
/// SUCCESS | TIMED_OUT). A started query is SUBMITTED until the clock settles it, and
/// FAILED is the only terminal status this emulator can reach — see
/// [`PROTECTED_QUERY_FAILURE_CODE`].
pub const PROTECTED_QUERY_STATUS_SUBMITTED: &str = "SUBMITTED";
/// See [`PROTECTED_QUERY_STATUS_SUBMITTED`].
pub const PROTECTED_QUERY_STATUS_FAILED: &str = "FAILED";

/// How long after `StartProtectedQuery` the clock owes the query its terminal status.
///
/// AWS publishes no duration for a protected query's run, so this span is CHOSEN: 500
/// ms, inside this project's 250-750 ms band for a transition the clock owes. It is
/// deliberately non-zero, so a caller that reads the query straight after starting it
/// still sees it SUBMITTED, and the due instant is stored rather than slept on.
pub const PROTECTED_QUERY_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The `ProtectedQueryError.code` a settled query carries.
///
/// Clean Rooms executes a protected query against the configured tables a
/// collaboration associates; this emulator serves the Clean Rooms CONTROL plane only
/// and has no analytics data plane to run SQL on, so no protected query can be
/// executed. The run is therefore not performed and the query ends in the modelled
/// failure state ([`PROTECTED_QUERY_STATUS_FAILED`]) with an error that says exactly
/// that — never in a fabricated SUCCESS with results nothing computed.
pub const PROTECTED_QUERY_FAILURE_CODE: &str = "QUERY_EXECUTION_UNAVAILABLE";

/// The `ProtectedQueryError.message` that goes with [`PROTECTED_QUERY_FAILURE_CODE`].
pub const PROTECTED_QUERY_FAILURE_MESSAGE: &str =
    "this emulator serves the Clean Rooms control plane only: it has no analytics data \
     plane, so the protected query was accepted and recorded but never executed";

/// `MembershipStatus` values this slice produces (model shape `MembershipStatus`:
/// ACTIVE | REMOVED | COLLABORATION_DELETED).
pub const MEMBERSHIP_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`MEMBERSHIP_STATUS_ACTIVE`].
pub const MEMBERSHIP_STATUS_COLLABORATION_DELETED: &str = "COLLABORATION_DELETED";
/// See [`MEMBERSHIP_STATUS_ACTIVE`]. The status the membership of a party the
/// collaboration's creator has removed is moved to: the membership survives the removal,
/// but the account it belongs to is no longer part of the collaboration behind it.
pub const MEMBERSHIP_STATUS_REMOVED: &str = "REMOVED";

/// `MemberAbility` members the payment defaults below key on (model shape
/// `MemberAbility`: CAN_QUERY | CAN_RECEIVE_RESULTS | CAN_RUN_JOB |
/// CAN_EXPORT_QUERY_ANALYSIS_LOG).
pub const ABILITY_CAN_QUERY: &str = "CAN_QUERY";
/// See [`ABILITY_CAN_QUERY`].
pub const ABILITY_CAN_RUN_JOB: &str = "CAN_RUN_JOB";

/// `ChangeRequestStatus` values this slice produces (model shape `ChangeRequestStatus`:
/// PENDING | APPROVED | CANCELLED | DENIED | COMMITTED).
pub const CHANGE_REQUEST_STATUS_PENDING: &str = "PENDING";
/// See [`CHANGE_REQUEST_STATUS_PENDING`].
pub const CHANGE_REQUEST_STATUS_COMMITTED: &str = "COMMITTED";
/// See [`CHANGE_REQUEST_STATUS_PENDING`].
pub const CHANGE_REQUEST_STATUS_APPROVED: &str = "APPROVED";
/// See [`CHANGE_REQUEST_STATUS_PENDING`].
pub const CHANGE_REQUEST_STATUS_DENIED: &str = "DENIED";
/// See [`CHANGE_REQUEST_STATUS_PENDING`].
pub const CHANGE_REQUEST_STATUS_CANCELLED: &str = "CANCELLED";

/// `ApprovalStatus` values this slice produces (model shape `ApprovalStatus`:
/// APPROVED | DENIED | PENDING).
pub const APPROVAL_STATUS_PENDING: &str = "PENDING";
/// See [`APPROVAL_STATUS_PENDING`].
pub const APPROVAL_STATUS_APPROVED: &str = "APPROVED";
/// See [`APPROVAL_STATUS_PENDING`].
pub const APPROVAL_STATUS_DENIED: &str = "DENIED";

/// `ChangeRequestAction` values (model shape `ChangeRequestAction`: APPROVE | DENY |
/// CANCEL | COMMIT) — the actions `UpdateCollaborationChangeRequest` performs on a
/// change request.
pub const CHANGE_REQUEST_ACTION_APPROVE: &str = "APPROVE";
/// See [`CHANGE_REQUEST_ACTION_APPROVE`].
pub const CHANGE_REQUEST_ACTION_DENY: &str = "DENY";
/// See [`CHANGE_REQUEST_ACTION_APPROVE`].
pub const CHANGE_REQUEST_ACTION_CANCEL: &str = "CANCEL";
/// See [`CHANGE_REQUEST_ACTION_APPROVE`].
pub const CHANGE_REQUEST_ACTION_COMMIT: &str = "COMMIT";

/// `ChangeSpecificationType` values (model shape `ChangeSpecificationType`: MEMBER |
/// COLLABORATION) — one per member of the `ChangeSpecification` union, which is what
/// makes a `specificationType` that names the other member detectable.
pub const CHANGE_SPECIFICATION_TYPE_MEMBER: &str = "MEMBER";
/// See [`CHANGE_SPECIFICATION_TYPE_MEMBER`].
pub const CHANGE_SPECIFICATION_TYPE_COLLABORATION: &str = "COLLABORATION";

/// The `ChangeType` a specification naming an account that is not yet part of the
/// collaboration asks for (model shape `ChangeType`).
pub const CHANGE_TYPE_ADD_MEMBER: &str = "ADD_MEMBER";
/// The `ChangeType` a `CollaborationChangeSpecification` that moves the collaboration's
/// `autoApprovedChangeTypes` asks for (model shape `ChangeType`).
pub const CHANGE_TYPE_EDIT_AUTO_APPROVED_CHANGE_TYPES: &str = "EDIT_AUTO_APPROVED_CHANGE_TYPES";
/// The `ChangeType` for a member that becomes responsible for a cost it was not
/// responsible for (model shape `ChangeType`).
pub const CHANGE_TYPE_ADD_PAYER_CANDIDATE: &str = "ADD_PAYER_CANDIDATE";
/// The `ChangeType` for a member that stops being responsible for every cost (model
/// shape `ChangeType`).
pub const CHANGE_TYPE_REMOVE_PAYER_CANDIDATE: &str = "REMOVE_PAYER_CANDIDATE";

/// Each `MemberAbility` the `ChangeType` catalog can grant or revoke, as
/// `(ability, grant type, revoke type)`.
///
/// The `ChangeType` catalog (ADD_MEMBER | GRANT_RECEIVE_RESULTS_ABILITY |
/// REVOKE_RECEIVE_RESULTS_ABILITY | EDIT_AUTO_APPROVED_CHANGE_TYPES |
/// ADD_PAYER_CANDIDATE | REMOVE_PAYER_CANDIDATE | GRANT_CAN_RECEIVE_MODEL_OUTPUT |
/// GRANT_CAN_RECEIVE_INFERENCE_OUTPUT | REVOKE_CAN_RECEIVE_MODEL_OUTPUT |
/// REVOKE_CAN_RECEIVE_INFERENCE_OUTPUT | GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY |
/// REVOKE_EXPORT_QUERY_ANALYSIS_LOG_ABILITY) names a grant/revoke pair for exactly two
/// of the four `MemberAbility` values, so a change to CAN_QUERY or CAN_RUN_JOB has no
/// change type to report and cannot be asked for through a change request.
pub const MEMBER_ABILITY_CHANGE_TYPES: [(&str, &str, &str); 2] = [
    (
        "CAN_RECEIVE_RESULTS",
        "GRANT_RECEIVE_RESULTS_ABILITY",
        "REVOKE_RECEIVE_RESULTS_ABILITY",
    ),
    (
        "CAN_EXPORT_QUERY_ANALYSIS_LOG",
        "GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
        "REVOKE_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
    ),
];

/// Each `CustomMLMemberAbility` the `ChangeType` catalog can grant or revoke, as
/// `(ability, grant type, revoke type)`. The catalog
/// (CAN_RECEIVE_MODEL_OUTPUT | CAN_RECEIVE_INFERENCE_OUTPUT) has a grant and a revoke
/// change type for both of its values. See [`MEMBER_ABILITY_CHANGE_TYPES`].
pub const ML_MEMBER_ABILITY_CHANGE_TYPES: [(&str, &str, &str); 2] = [
    (
        "CAN_RECEIVE_MODEL_OUTPUT",
        "GRANT_CAN_RECEIVE_MODEL_OUTPUT",
        "REVOKE_CAN_RECEIVE_MODEL_OUTPUT",
    ),
    (
        "CAN_RECEIVE_INFERENCE_OUTPUT",
        "GRANT_CAN_RECEIVE_INFERENCE_OUTPUT",
        "REVOKE_CAN_RECEIVE_INFERENCE_OUTPUT",
    ),
];

/// `ResourceType` values this slice reports on a modelled error (model shape
/// `ResourceType`: CONFIGURED_TABLE | COLLABORATION | MEMBERSHIP |
/// CONFIGURED_TABLE_ASSOCIATION).
pub const RESOURCE_TYPE_COLLABORATION: &str = "COLLABORATION";
/// See [`RESOURCE_TYPE_COLLABORATION`].
pub const RESOURCE_TYPE_MEMBERSHIP: &str = "MEMBERSHIP";
/// See [`RESOURCE_TYPE_COLLABORATION`].
pub const RESOURCE_TYPE_CONFIGURED_TABLE: &str = "CONFIGURED_TABLE";

/// `TableReferenceType` values (model shape `TableReferenceType`: GLUE | SNOWFLAKE |
/// ATHENA | DATABRICKS) — the `tableType` a configured table reports, one per variant
/// of the `TableReference` union.
pub const TABLE_TYPE_GLUE: &str = "GLUE";
/// See [`TABLE_TYPE_GLUE`].
pub const TABLE_TYPE_SNOWFLAKE: &str = "SNOWFLAKE";
/// See [`TABLE_TYPE_GLUE`].
pub const TABLE_TYPE_ATHENA: &str = "ATHENA";
/// See [`TABLE_TYPE_GLUE`].
pub const TABLE_TYPE_DATABRICKS: &str = "DATABRICKS";

/// `ConfiguredTableAnalysisRuleType` values (model shape
/// `ConfiguredTableAnalysisRuleType`: AGGREGATION | LIST | CUSTOM) — the `type` an
/// analysis rule reports, one per member of the `ConfiguredTableAnalysisRulePolicyV1`
/// union, which is what makes a type/policy mismatch detectable.
pub const ANALYSIS_RULE_TYPE_AGGREGATION: &str = "AGGREGATION";
/// See [`ANALYSIS_RULE_TYPE_AGGREGATION`].
pub const ANALYSIS_RULE_TYPE_LIST: &str = "LIST";
/// See [`ANALYSIS_RULE_TYPE_AGGREGATION`].
pub const ANALYSIS_RULE_TYPE_CUSTOM: &str = "CUSTOM";

/// The smallest `selectedAnalysisMethods` a request may configure.
///
/// Documented on `CreateConfiguredTableInput$selectedAnalysisMethods` and
/// `UpdateConfiguredTableInput$selectedAnalysisMethods`: "The analysis methods to enable
/// for the configured table. When configured, you must specify at least two analysis
/// methods." The model cannot express it (`SelectedAnalysisMethods` carries no length
/// constraint), so the handler enforces it with the `ValidationException` the operation
/// declares.
pub const MIN_SELECTED_ANALYSIS_METHODS: usize = 2;

/// The resource type an analysis-template 404 reports.
///
/// `ResourceNotFoundException$resourceType` is REQUIRED, and the model's `ResourceType`
/// catalog (CONFIGURED_TABLE | COLLABORATION | MEMBERSHIP |
/// CONFIGURED_TABLE_ASSOCIATION) names no analysis template, so the member reports the
/// type of the resource that was not found rather than one of the four that is not it.
/// `ConflictException$resourceType`, which is optional, is left absent instead.
pub const RESOURCE_TYPE_ANALYSIS_TEMPLATE: &str = "ANALYSIS_TEMPLATE";

/// The resource type a privacy-budget-template 404 reports, for the same reason as
/// [`RESOURCE_TYPE_ANALYSIS_TEMPLATE`]: the member is required and the model's
/// `ResourceType` catalog names no privacy budget template, so it reports the type of
/// the resource that was not found rather than one of the four that is not it.
pub const RESOURCE_TYPE_PRIVACY_BUDGET_TEMPLATE: &str = "PRIVACY_BUDGET_TEMPLATE";

/// The resource type a 404 about one party to a collaboration reports, named the same
/// way as [`RESOURCE_TYPE_ANALYSIS_TEMPLATE`]: `ResourceNotFoundException$resourceType`
/// is required and the model's `ResourceType` catalog names no member, so the member
/// reports the type of the resource that was not found rather than one of the four that
/// is not it.
pub const RESOURCE_TYPE_MEMBER: &str = "MEMBER";

/// The resource types a tagging refusal reports for the remaining kinds of resource an
/// ARN can address, named the same way as [`RESOURCE_TYPE_ANALYSIS_TEMPLATE`].
pub const RESOURCE_TYPE_CONFIGURED_TABLE_ASSOCIATION: &str = "CONFIGURED_TABLE_ASSOCIATION";
/// See [`RESOURCE_TYPE_CONFIGURED_TABLE_ASSOCIATION`].
pub const RESOURCE_TYPE_ID_NAMESPACE_ASSOCIATION: &str = "ID_NAMESPACE_ASSOCIATION";
/// See [`RESOURCE_TYPE_CONFIGURED_TABLE_ASSOCIATION`].
pub const RESOURCE_TYPE_ID_MAPPING_TABLE: &str = "ID_MAPPING_TABLE";
/// See [`RESOURCE_TYPE_CONFIGURED_TABLE_ASSOCIATION`].
pub const RESOURCE_TYPE_CONFIGURED_AUDIENCE_MODEL_ASSOCIATION: &str =
    "CONFIGURED_AUDIENCE_MODEL_ASSOCIATION";
/// See [`RESOURCE_TYPE_CONFIGURED_TABLE_ASSOCIATION`].
pub const RESOURCE_TYPE_INTERMEDIATE_TABLE: &str = "INTERMEDIATE_TABLE";

/// Each kind of resource an ARN can address, as `(the keyword its ARN's resource
/// segment is introduced by, the resource type a refusal about it reports)`.
///
/// Read off the ARN patterns the model declares, one per kind: `CollaborationArn`
/// (`:collaboration/<id>`), `MembershipArn` (`:membership/<id>`), `ConfiguredTableArn`
/// (`:configuredtable/<id>`), `ConfiguredTableAssociationArn`
/// (`:configuredtableassociation/<id>/<id>`), `AnalysisTemplateArn`
/// (`:membership/<id>/analysistemplate/<id>`), `PrivacyBudgetTemplateArn`
/// (`:privacybudgettemplate/<id>`), `IdNamespaceAssociationArn`, `IdMappingTableArn`,
/// `ConfiguredAudienceModelAssociationArn` and `IntermediateTableArn` (each
/// `:membership/<id>/<keyword>/<id>`). A sub-resource's ARN carries its parent's keyword
/// too, so [`resource_type_of_arn`] takes the DEEPEST keyword the ARN names.
pub const ARN_RESOURCE_KEYWORDS: [(&str, &str); 10] = [
    ("collaboration", RESOURCE_TYPE_COLLABORATION),
    ("membership", RESOURCE_TYPE_MEMBERSHIP),
    ("configuredtable", RESOURCE_TYPE_CONFIGURED_TABLE),
    (
        "configuredtableassociation",
        RESOURCE_TYPE_CONFIGURED_TABLE_ASSOCIATION,
    ),
    ("analysistemplate", RESOURCE_TYPE_ANALYSIS_TEMPLATE),
    ("privacybudgettemplate", RESOURCE_TYPE_PRIVACY_BUDGET_TEMPLATE),
    (
        "idnamespaceassociation",
        RESOURCE_TYPE_ID_NAMESPACE_ASSOCIATION,
    ),
    ("idmappingtable", RESOURCE_TYPE_ID_MAPPING_TABLE),
    (
        "configuredaudiencemodelassociation",
        RESOURCE_TYPE_CONFIGURED_AUDIENCE_MODEL_ASSOCIATION,
    ),
    ("intermediatetable", RESOURCE_TYPE_INTERMEDIATE_TABLE),
];

/// `ConflictExceptionReason` for a resource that already exists (model shape
/// `ConflictExceptionReason`: ALREADY_EXISTS | SUBRESOURCES_EXIST | INVALID_STATE).
pub const CONFLICT_REASON_ALREADY_EXISTS: &str = "ALREADY_EXISTS";

/// `PrivacyBudgetType` values this slice keys on (model shape `PrivacyBudgetType`:
/// DIFFERENTIAL_PRIVACY | ACCESS_BUDGET). Each value names exactly one member of the
/// `PrivacyBudgetTemplateParametersInput` union, which is what makes a type/parameters
/// mismatch detectable.
pub const PRIVACY_BUDGET_TYPE_DIFFERENTIAL_PRIVACY: &str = "DIFFERENTIAL_PRIVACY";
/// See [`PRIVACY_BUDGET_TYPE_DIFFERENTIAL_PRIVACY`].
pub const PRIVACY_BUDGET_TYPE_ACCESS_BUDGET: &str = "ACCESS_BUDGET";

/// How often a privacy budget template refreshes when the create request names no
/// cadence.
///
/// `PrivacyBudgetTemplate$autoRefresh` is REQUIRED on the output while
/// `CreatePrivacyBudgetTemplateInput$autoRefresh` is optional, so the service decides
/// the value a template without one carries. The `PrivacyBudgetTemplateAutoRefresh`
/// catalog is CALENDAR_MONTH | NONE, and CALENDAR_MONTH is documented as an opt-in
/// ("you can use CALENDAR_MONTH to automatically get a new privacy budget … Avoid
/// choosing this if the same rows will be repeatedly queried"), so a template nobody
/// asked to refresh does not refresh. This is the emulator's choice of the value the
/// service supplies, not a published AWS default.
pub const DEFAULT_PRIVACY_BUDGET_TEMPLATE_AUTO_REFRESH: &str = "NONE";

/// `ConflictExceptionReason` for a resource whose current status does not admit the
/// requested change — a membership whose collaboration is gone, say (model shape
/// `ConflictExceptionReason`: ALREADY_EXISTS | SUBRESOURCES_EXIST | INVALID_STATE).
pub const CONFLICT_REASON_INVALID_STATE: &str = "INVALID_STATE";

/// The only `AccessDeniedExceptionReason` the model declares (model shape
/// `AccessDeniedExceptionReason`: INSUFFICIENT_PERMISSIONS).
pub const ACCESS_DENIED_REASON_INSUFFICIENT_PERMISSIONS: &str = "INSUFFICIENT_PERMISSIONS";

/// `ValidationExceptionReason` for a member value that failed validation (model shape
/// `ValidationExceptionReason`).
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// `ValidationExceptionReason` for a request that names no query the service can run —
/// SQL parameters with no query, or an analysis whose text is not a query (model shape
/// `ValidationExceptionReason`: FIELD_VALIDATION_FAILED | INVALID_CONFIGURATION |
/// INVALID_QUERY | IAM_SYNCHRONIZATION_DELAY).
pub const VALIDATION_REASON_INVALID_QUERY: &str = "INVALID_QUERY";

/// `ValidationExceptionReason` for a request whose own members are well formed but ask
/// for something the resource's configuration or state does not admit — an action on a
/// change request that is already settled, say (model shape
/// `ValidationExceptionReason`).
pub const VALIDATION_REASON_INVALID_CONFIGURATION: &str = "INVALID_CONFIGURATION";

/// `AnalysisFormat` values this slice keys on (model shape `AnalysisFormat`: SQL |
/// PYSPARK_1_0). Only a SQL analysis carries a query the service can read relations
/// out of; a PySpark analysis is program text, so its referenced tables are the ones
/// the caller declared.
pub const ANALYSIS_FORMAT_SQL: &str = "SQL";

/// The largest `members` list `CreateCollaboration` accepts.
///
/// The model declares `MemberList` as `len 0..` — no upper bound — so without a cap
/// the only limit on the list one request may carry is the transport body size, and
/// the per-request work the handler does would be inherited from that rather than
/// bounded by the service. This is an EMULATOR-ENFORCED resource bound, not a
/// documented AWS quota: it is set far above any realistic collaboration so it refuses
/// only requests that exist to make the handler do unbounded work, and it is reported
/// through the `ServiceQuotaExceededException` (402) `CreateCollaboration` declares.
pub const MAX_COLLABORATION_MEMBERS: usize = 1000;

/// The largest number of collaborations, and of memberships, the store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// a create is the only thing that adds one, so each store needs a maximum entry
/// count: neither a TTL nor an eviction policy is available to it, because
/// `GetCollaboration` and `GetMembership` must still answer what their creates
/// created. The two operations that grow these stores refuse past the caps with the
/// `ServiceQuotaExceededException` (HTTP 402) the model declares on exactly those two
/// operations and on no read. The numbers are the EMULATOR's own bound, not a
/// published AWS default — AWS publishes its Clean Rooms quotas as adjustable, so no
/// single value is the contract — and they are set high enough that no ordinary
/// workflow meets one.
pub const MAX_COLLABORATIONS: usize = 5_000;

/// See [`MAX_COLLABORATIONS`].
pub const MAX_MEMBERSHIPS: usize = 5_000;

/// See [`MAX_COLLABORATIONS`] — the same bound, for the analysis templates one
/// membership's creates add to the store.
pub const MAX_ANALYSIS_TEMPLATES: usize = 5_000;

/// The most relations this emulator reads out of ONE analysis.
///
/// The model declares `QueryTables` with no upper bound, and the relations are read out
/// of caller-supplied text, so without a cap the number of names one request makes the
/// service collect and store would be set by the request rather than by the service.
/// This is an EMULATOR-ENFORCED resource bound, not a documented AWS quota: it is far
/// above any realistic analysis (an AWS Clean Rooms collaboration's whole catalog is
/// orders of magnitude smaller), so it refuses only an analysis that exists to make the
/// service collect an unbounded relation set, through the
/// `ServiceQuotaExceededException` (402) `CreateAnalysisTemplate` declares. A query
/// over the cap is refused rather than reported with a truncated relation set, which
/// would misstate what the analysis reads. It sits below what
/// [`MAX_PARSED_ANALYSIS_TOKENS`] allows, so this is the bound a relation-heavy query
/// meets, and it is the one reported.
pub const MAX_REFERENCED_TABLES: usize = 200;

/// The most analysis text this service reads a relation set out of.
///
/// The model's `AnalysisTemplateText` allows up to 500,000 characters, and the cost of
/// parsing a query is a function of its length, so the length the SERVICE will parse is
/// capped here: the work one request can ask for is bounded BEFORE it is scheduled,
/// instead of only being waited on. A longer analysis is refused with the modelled
/// `ValidationException` naming `source.text`, rather than stored with a relation set
/// nothing read. The EMULATOR's own number, not an AWS one, and far above any
/// hand-written analysis (100,000 characters of SQL).
pub const MAX_PARSED_ANALYSIS_CHARS: usize = 100_000;

/// The most TOKENS this service parses in ONE analysis.
///
/// This is the bound that dominates the recursion, and it is counted on a FLAT token
/// pass before any tree exists. The length cap above bounds how much text the lexer
/// reads; it does not bound the shape of the tree a parse of that text would build, and
/// bracket nesting is not a bound on it either: a left-associative chain
/// (`1 + 1 + 1 + …`, `a OR a OR …`) carries no brackets at all and still builds a tree
/// one node deep per operator, which the parser itself builds in a LOOP rather than by
/// recursing. Every node of that tree consumes at least one token, so a cap on the
/// non-whitespace token count caps the tree's SIZE and therefore its DEPTH — the
/// recursive parse, this module's recursive walk of the tree and the tree's own
/// recursive drop all stay within this many frames, which is what keeps a deep analysis
/// from exhausting the stack (a Rust stack overflow aborts the process, so no later
/// check could contain it). An analysis over the cap is refused with the modelled
/// `ValidationException` naming `source.text`. The EMULATOR's own number, not an AWS
/// one, and far above any hand-written analysis (1,000 tokens is a query of a hundred
/// lines or so).
pub const MAX_PARSED_ANALYSIS_TOKENS: usize = 1_000;

/// The parser's own recursion limit, an INDEPENDENT second bound on nesting the parser
/// produces by recursing (a bracketed subquery or sub-expression).
///
/// [`MAX_PARSED_ANALYSIS_TOKENS`] already bounds the tree, so this is a backstop that
/// stops a deeply bracketed analysis inside the parse instead of after it; exceeding it
/// is a parse ERROR, reported as the same modelled refusal. The EMULATOR's own number,
/// not an AWS one.
pub const MAX_PARSER_RECURSION: usize = 200;

/// The most analyses this service parses at once.
///
/// Each parse runs on its own thread with [`ANALYSIS_PARSE_STACK_BYTES`] of stack, so
/// without a cap a burst of creates could start an unbounded number of them; a request
/// that finds every slot taken waits for one within [`ANALYSIS_PARSE_TIME_BUDGET`] and
/// is refused if none frees up. The EMULATOR's own number, not an AWS one.
pub const MAX_CONCURRENT_ANALYSIS_PARSES: usize = 4;

/// The stack one analysis parse is given.
///
/// Parsing an analysis, walking the tree for its relations and dropping that tree are
/// all recursive, so they cost stack in proportion to the tree's depth — and a Rust
/// stack overflow ABORTS the process instead of unwinding, so it cannot be caught after
/// the fact. [`MAX_PARSED_ANALYSIS_TOKENS`] bounds the tree to that many nodes, hence
/// that many frames, and this stack is sized with a wide margin over them (32 KiB of
/// stack per possible frame) — which is why the work runs on a thread of this service's
/// own making rather than on the runtime's blocking pool, whose stack size it does not
/// choose. Virtual address space, committed by the page as it is used. The EMULATOR's
/// own number, not an AWS one.
pub const ANALYSIS_PARSE_STACK_BYTES: usize = 32 * 1024 * 1024;

/// The wall-clock budget one analysis parse is given.
///
/// A BACKSTOP, not the bound: the work itself is bounded by
/// [`MAX_PARSED_ANALYSIS_CHARS`] (how much text is lexed),
/// [`MAX_PARSED_ANALYSIS_TOKENS`] (how big a tree the parse may build, and so how deep
/// the parse, the walk and the drop recurse), [`MAX_REFERENCED_TABLES`] (how many
/// relations are collected) and [`MAX_CONCURRENT_ANALYSIS_PARSES`] (how many parses run
/// at once). The caller is held to this budget as well, so a request is never left
/// waiting and the wait has an await point for a timeout or a shutdown to land on. The
/// EMULATOR's own number, not an AWS one.
pub const ANALYSIS_PARSE_TIME_BUDGET: Duration = Duration::from_secs(2);

/// See [`MAX_COLLABORATIONS`]: the same EMULATOR-enforced bound on the configured-table
/// store, reported through the `ServiceQuotaExceededException` (HTTP 402)
/// `CreateConfiguredTable` declares.
pub const MAX_CONFIGURED_TABLES: usize = 5_000;

/// See [`MAX_COLLABORATIONS`]: the same EMULATOR-enforced bound on the configured-table
/// association store, reported through the `ServiceQuotaExceededException` (HTTP 402)
/// `CreateConfiguredTableAssociation` declares.
pub const MAX_CONFIGURED_TABLE_ASSOCIATIONS: usize = 5_000;

/// The largest number of protected queries the store holds, for the same reason as
/// [`MAX_COLLABORATIONS`]: `StartProtectedQuery` is the only operation that adds one,
/// every one it adds lives for the life of the process so `GetProtectedQuery` can still
/// answer for it, and past the cap the service answers the
/// `ServiceQuotaExceededException` (HTTP 402) `StartProtectedQuery` declares. The
/// number is the EMULATOR's own bound, not a published AWS default.
pub const MAX_PROTECTED_QUERIES: usize = 5_000;

/// See [`MAX_COLLABORATIONS`]: the same EMULATOR-enforced bound on the privacy budget
/// template store, reported through the `ServiceQuotaExceededException` (HTTP 402)
/// `CreatePrivacyBudgetTemplate` declares.
///
/// A per-collaboration limit is not a bound on this store on its own: a collaboration
/// can be deleted and recreated, which frees the slot [`MAX_COLLABORATIONS`] counts, so
/// the template store needs its own maximum entry count like every other store here.
pub const MAX_PRIVACY_BUDGET_TEMPLATES: usize = 5_000;

/// See [`MAX_COLLABORATIONS`]: the same EMULATOR-enforced bound on the collaboration
/// change request store, reported through the `ServiceQuotaExceededException` (HTTP 402)
/// `CreateCollaborationChangeRequest` declares.
pub const MAX_COLLABORATION_CHANGE_REQUESTS: usize = 5_000;

/// The largest `approvals` map a change request can carry, from the MODEL: the
/// `ApprovalStatuses` map is declared `len 1..50`. A change request records one approval
/// entry per party to the collaboration (see
/// [`CleanRoomsState::change_request_approvals`]), so a collaboration with more parties
/// than this has no representable approvals map and the create refuses with the
/// `ServiceQuotaExceededException` it declares.
pub const MAX_CHANGE_REQUEST_APPROVALS: usize = 50;


/// A collaboration exactly as its creator declared it.
#[derive(Debug, Clone)]
pub struct CollaborationRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub creator_account_id: String,
    pub creator_display_name: String,
    pub creator_member_abilities: Vec<String>,
    pub creator_ml_member_abilities: Option<MLMemberAbilities>,
    pub creator_payment_configuration: Option<PaymentConfiguration>,
    pub create_time: DateTime,
    pub update_time: DateTime,
    pub data_encryption_metadata: Option<DataEncryptionMetadata>,
    pub query_log_status: String,
    pub job_log_status: Option<String>,
    /// The engine in force — the one the caller selected, else
    /// [`DEFAULT_ANALYTICS_ENGINE`]. Not optional: the service always has one.
    pub analytics_engine: String,
    pub auto_approved_change_types: Option<Vec<String>>,
    pub allowed_result_regions: Option<Vec<String>>,
    /// The metrics setting in force — the one the caller sent, else
    /// [`DEFAULT_IS_METRICS_ENABLED`]. Not optional, for the same reason.
    pub is_metrics_enabled: bool,
    /// The initial members, not including the creator. Immutable, per the
    /// `CreateCollaboration` `members` doc.
    pub members: Vec<MemberSpecification>,
    /// When each party's own member record was created and last updated, keyed by
    /// account id — the `MemberSummary$createTime`/`updateTime` a member listing
    /// reports.
    ///
    /// A member's timestamps are NOT the collaboration's: the creator and the declared
    /// members are created with the collaboration, but a committed change request can
    /// add a party later and can restate one party's grant without touching another's,
    /// so each party's pair is stamped on its own (see
    /// [`CleanRoomsState::apply_collaboration_change`]).
    pub member_times: HashMap<String, MemberTimes>,
    /// The parties the creator has removed with `DeleteMember`.
    ///
    /// A removal does NOT take the party's grant out of `members`: "the removed member is
    /// placed in the Removed status", so the collaboration still holds what that account
    /// was granted and a member listing still reports it — with
    /// [`MEMBER_STATUS_REMOVED`] as its status. What the removal takes away is the
    /// account's authorization to the collaboration ("can't interact with the
    /// collaboration"), which is why this set is consulted by
    /// [`CleanRoomsState::collaboration_grant`] rather than by
    /// [`CollaborationRecord::grant_for`].
    pub removed_members: HashSet<String>,
}

/// When one party's member record was created and last updated.
#[derive(Debug, Clone, Copy)]
pub struct MemberTimes {
    pub create_time: DateTime,
    pub update_time: DateTime,
}

/// What one account was granted in a collaboration: its abilities and the payment
/// responsibilities the creator set for it.
#[derive(Debug, Clone)]
pub struct MemberGrant {
    pub display_name: String,
    pub member_abilities: Vec<String>,
    pub ml_member_abilities: Option<MLMemberAbilities>,
    pub payment_configuration: Option<PaymentConfiguration>,
}

impl CollaborationRecord {
    /// The grant `account_id` holds in this collaboration, or `None` when that account
    /// is neither the creator nor one of the declared members — such an account is not
    /// part of the collaboration at all, so it can neither see nor join it.
    pub fn grant_for(&self, account_id: &str) -> Option<MemberGrant> {
        if account_id == self.creator_account_id {
            return Some(MemberGrant {
                display_name: self.creator_display_name.clone(),
                member_abilities: self.creator_member_abilities.clone(),
                ml_member_abilities: self.creator_ml_member_abilities.clone(),
                payment_configuration: self.creator_payment_configuration.clone(),
            });
        }
        self.members
            .iter()
            .find(|m| m.account_id == account_id)
            .map(|m| MemberGrant {
                display_name: m.display_name.clone(),
                member_abilities: m.member_abilities.clone(),
                ml_member_abilities: m.ml_member_abilities.clone(),
                payment_configuration: m.payment_configuration.clone(),
            })
    }

    /// Every account this collaboration holds a grant for, in declaration order: the
    /// creator first, then the declared members. One entry per account — the creator is
    /// not repeated by a member entry naming it.
    pub fn party_accounts(&self) -> Vec<String> {
        let mut accounts = Vec::with_capacity(self.members.len() + 1);
        accounts.push(self.creator_account_id.clone());
        for member in &self.members {
            if member.account_id != self.creator_account_id {
                accounts.push(member.account_id.clone());
            }
        }
        accounts
    }

    /// Whether `account_id` is a party this collaboration's creator has removed.
    pub fn is_removed(&self, account_id: &str) -> bool {
        self.removed_members.contains(account_id)
    }

    /// When `account_id`'s member record was created and last updated.
    ///
    /// A party the record holds no stamped pair for was declared with the collaboration,
    /// so it carries the collaboration's own timestamps.
    pub fn member_times_of(&self, account_id: &str) -> MemberTimes {
        self.member_times
            .get(account_id)
            .copied()
            .unwrap_or(MemberTimes {
                create_time: self.create_time,
                update_time: self.update_time,
            })
    }
}

/// A protected query as `StartProtectedQuery` recorded it, plus the terminal status the
/// clock owes it.
#[derive(Debug, Clone)]
pub struct ProtectedQueryRecord {
    /// The modelled shape, stored as-is: every one of its members is absolute, so a
    /// read reports it without deriving anything.
    pub query: ProtectedQuery,
    /// When the clock owes this query its terminal status, or `None` once that
    /// transition has been materialized. See [`PROTECTED_QUERY_SETTLE_DELAY`].
    pub terminal_due: Option<SystemTime>,
}

/// Per-service state for com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState {
    /// Collaborations by `Collaboration.id`.
    pub collaborations: HashMap<String, CollaborationRecord>,
    /// Collaboration ids in creation order — the order the list operation pages in.
    pub collaboration_order: Vec<String>,
    /// Memberships by `Membership.id`.
    pub memberships: HashMap<String, Membership>,
    /// Membership ids in creation order — the order the list operation pages in.
    pub membership_order: Vec<String>,
    /// `(collaboration id, account id) -> membership id`: the index that makes the
    /// duplicate-membership conflict and a `Collaboration`'s `membershipId` computable
    /// instead of guessed.
    pub membership_by_collaboration: HashMap<(String, String), String>,
    /// Configured tables by `ConfiguredTable.id`.
    ///
    /// The modelled `ConfiguredTable` is stored as-is: every one of its members is
    /// absolute (nothing about it depends on who is asking), so there is nothing to
    /// derive per request. `analysisRuleTypes` is the one member another operation
    /// maintains — a configured table is created with no analysis rules, and the
    /// analysis-rule operations add to and remove from this list.
    pub configured_tables: HashMap<String, ConfiguredTable>,
    /// Configured table ids in creation order — the order the list operation pages in.
    pub configured_table_order: Vec<String>,
    /// Configured table analysis rules by `(configured table id, analysis rule type)`.
    ///
    /// The modelled `ConfiguredTableAnalysisRule` is stored as-is, like the
    /// [`ConfiguredTable`] it belongs to: every one of its members is absolute. The key
    /// is the pair the analysis-rule operations address a rule by
    /// (`/configuredTables/{configuredTableIdentifier}/analysisRule/{analysisRuleType}`),
    /// so a read is a lookup and not a scan, and which types a table holds a rule for
    /// stays on the table itself, in `ConfiguredTable$analysisRuleTypes`.
    pub configured_table_analysis_rules: HashMap<(String, String), ConfiguredTableAnalysisRule>,
    /// Tags by resource ARN — one place every taggable resource's tags are read and
    /// written, whichever kind of resource the ARN names: a create stores the tags it
    /// was given here, the tagging operations add to and remove from the same entry
    /// ([`CleanRoomsState::add_tags`], [`CleanRoomsState::remove_tags`]), and a delete
    /// drops it with the resource. A resource carrying no tags holds NO entry, so the
    /// absent entry and the empty map are one state.
    pub tags: HashMap<String, HashMap<String, String>>,
    /// Analysis templates by `AnalysisTemplate.id`. The modelled shape is stored as-is:
    /// every one of its members is absolute, so nothing is derived per caller.
    pub analysis_templates: HashMap<String, AnalysisTemplate>,
    /// Analysis template ids in creation order — the order a listing pages in.
    pub analysis_template_order: Vec<String>,
    /// `(membership id, analysis template name) -> analysis template id`: the index that
    /// makes the duplicate-name conflict computable instead of guessed.
    pub analysis_template_by_name: HashMap<(String, String), String>,
    /// Privacy budget templates by `PrivacyBudgetTemplate.id`. The modelled shape is
    /// stored as-is: every one of its members is absolute, so there is nothing to
    /// derive per request.
    pub privacy_budget_templates: HashMap<String, PrivacyBudgetTemplate>,
    /// Privacy budget template ids in creation order — the order a list operation
    /// pages in.
    pub privacy_budget_template_order: Vec<String>,
    /// `(collaboration id, privacyBudgetType) -> privacy budget template id`: the index
    /// that makes "a collaboration holds at most one privacy budget template of a given
    /// budget type" computable instead of guessed.
    ///
    /// The budget TYPE is part of the key because a template is created FOR one type:
    /// `CreatePrivacyBudgetTemplate$privacyBudgetType` is required and names exactly one
    /// member of the parameters union, so a template of another type configures another
    /// budget rather than restating the same one.
    pub privacy_budget_template_by_collaboration: HashMap<(String, String), String>,
    /// Configured table associations by `ConfiguredTableAssociation.id`. The modelled
    /// shape is stored as-is: every one of its members is absolute, so nothing is derived
    /// per caller.
    pub configured_table_associations: HashMap<String, ConfiguredTableAssociation>,
    /// Configured table association ids in creation order — the order a listing pages in.
    pub configured_table_association_order: Vec<String>,
    /// `(membership id, table alias) -> configured table association id`: the index that
    /// makes a repeat create under one alias a conflict rather than a second resource.
    /// The alias is "used to query the underlying configured table"
    /// (`CreateConfiguredTableAssociationInput$name`), so two associations of one
    /// membership cannot share one.
    pub configured_table_association_by_name: HashMap<(String, String), String>,
    /// Configured table association analysis rules by `(configured table association id,
    /// analysis rule type)`.
    ///
    /// The modelled `ConfiguredTableAssociationAnalysisRule` is stored as-is, like the
    /// [`ConfiguredTableAssociation`] it belongs to: every one of its members is
    /// absolute. The key is the pair the association analysis-rule operations address a
    /// rule by
    /// (`/memberships/{membershipIdentifier}/configuredTableAssociations/{configuredTableAssociationIdentifier}/analysisRule/{analysisRuleType}`),
    /// so a read is a lookup and not a scan, and which types an association holds a rule
    /// for stays on the association itself, in
    /// `ConfiguredTableAssociation$analysisRuleTypes`. The membership is NOT part of the
    /// key: an association belongs to exactly one membership, which is what scopes the
    /// lookup (`Self::configured_table_association_in`).
    pub configured_table_association_analysis_rules:
        HashMap<(String, String), ConfiguredTableAssociationAnalysisRule>,
    /// Protected queries by `ProtectedQuery.id`.
    pub protected_queries: HashMap<String, ProtectedQueryRecord>,
    /// Protected query ids in start order — the order a listing pages in.
    pub protected_query_order: Vec<String>,
    /// Collaboration change requests by `CollaborationChangeRequest.id`.
    ///
    /// The modelled shape is stored as-is, like [`Membership`]: every one of its members
    /// is absolute (its `changes`, `status`, `approvals` and `isAutoApproved` describe
    /// the request, not who is asking), so a read reports it without deriving anything.
    /// The owning collaboration is on the record itself (`collaborationId`), which is
    /// what scopes a lookup to one collaboration.
    pub collaboration_change_requests: HashMap<String, CollaborationChangeRequest>,
    /// Change request ids in creation order — the order a listing pages in.
    pub collaboration_change_request_order: Vec<String>,
}

/// Shorthand for this service's state struct.
pub type CleanRoomsState = ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState;

impl ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState {
    /// The stored collaboration `collaboration_id` names together with the grant
    /// `account_id` holds in it, or `None` when this account has no authorization to
    /// that collaboration at all.
    ///
    /// `None` covers all three indistinguishable cases: no collaboration is stored
    /// under the id (it never existed, or `DeleteCollaboration` removed it), or one is
    /// stored but the account is neither its creator nor one of its declared members.
    /// A caller's authorization to a collaboration lives in the collaboration itself,
    /// so it disappears with it — which is why these cases are one answer and not
    /// three. Every operation that takes a `collaborationIdentifier` resolves it here,
    /// so the refusal it reports is the same whichever case holds and no operation
    /// leaks whether the id exists.
    ///
    /// A party the creator has REMOVED is the fourth case, and answers the same `None`: a
    /// removed member "can't interact with the collaboration", so it holds no
    /// authorization to it even though the collaboration still holds the grant it was
    /// declared with (which is what a member listing keeps reporting).
    pub fn collaboration_grant(
        &self,
        collaboration_id: &str,
        account_id: &str,
    ) -> Option<(&CollaborationRecord, MemberGrant)> {
        let record = self.collaborations.get(collaboration_id)?;
        if record.is_removed(account_id) {
            return None;
        }
        let grant = record.grant_for(account_id)?;
        Some((record, grant))
    }

    /// The stored collaboration `account_id` is authorized to see under
    /// `collaboration_id`. See [`Self::collaboration_grant`] for what `None` means.
    pub fn visible_collaboration(
        &self,
        collaboration_id: &str,
        account_id: &str,
    ) -> Option<&CollaborationRecord> {
        self.collaboration_grant(collaboration_id, account_id)
            .map(|(record, _)| record)
    }

    /// The membership `account_id` holds in `collaboration_id`, if any.
    pub fn membership_of(&self, collaboration_id: &str, account_id: &str) -> Option<&Membership> {
        let id = self
            .membership_by_collaboration
            .get(&(collaboration_id.to_string(), account_id.to_string()))?;
        self.memberships.get(id)
    }

    /// The caller's status as a member of `record`: REMOVED once the creator has removed
    /// it, ACTIVE once it holds a membership in the collaboration, INVITED while it is
    /// part of the collaboration — as the creator or as a declared member — without one.
    /// `None` when the caller is not part of the collaboration at all.
    ///
    /// A removal outranks the membership: `DeleteMember` "removes the specified member
    /// from a collaboration. The removed member is placed in the Removed status", so a
    /// party that had joined reports REMOVED rather than the ACTIVE its membership would
    /// otherwise give it.
    ///
    /// The creator is no exception: `CreateMembership` is the documented way to join a
    /// collaboration, and a collaboration's creator holds no membership until it calls
    /// it, so it is INVITED to the collaboration it just created and becomes ACTIVE only
    /// once that membership exists. A client that polls this member to learn whether it
    /// still has to join therefore sees the join it owes.
    pub fn member_status_for(
        &self,
        record: &CollaborationRecord,
        account_id: &str,
    ) -> Option<&'static str> {
        let part_of_collaboration = account_id == record.creator_account_id
            || record.members.iter().any(|m| m.account_id == account_id);
        if !part_of_collaboration {
            return None;
        }
        if record.is_removed(account_id) {
            return Some(MEMBER_STATUS_REMOVED);
        }
        Some(if self.membership_of(&record.id, account_id).is_some() {
            MEMBER_STATUS_ACTIVE
        } else {
            MEMBER_STATUS_INVITED
        })
    }

    /// The analysis template `membership_id` already holds under `name`, if any.
    ///
    /// A template's name is unique within its membership, so this is what makes a
    /// repeat create a conflict rather than a second resource.
    pub fn analysis_template_named(
        &self,
        membership_id: &str,
        name: &str,
    ) -> Option<&AnalysisTemplate> {
        let id = self
            .analysis_template_by_name
            .get(&(membership_id.to_string(), name.to_string()))?;
        self.analysis_templates.get(id)
    }

    /// The stored analysis template `template_id`, but only when it belongs to
    /// `membership_id`: an analysis template is a sub-resource of one membership, so a
    /// template read under another membership is not found there.
    pub fn analysis_template_in(
        &self,
        membership_id: &str,
        template_id: &str,
    ) -> Option<&AnalysisTemplate> {
        self.analysis_templates
            .get(template_id)
            .filter(|template| template.membership_id == membership_id)
    }

    /// The modelled `Collaboration` as `account_id` sees it, or `None` when that
    /// account is not part of the collaboration.
    pub fn collaboration_view(
        &self,
        record: &CollaborationRecord,
        account_id: &str,
    ) -> Option<Collaboration> {
        let member_status = self.member_status_for(record, account_id)?;
        let membership = self.membership_of(&record.id, account_id);
        Some(Collaboration {
            id: record.id.clone(),
            arn: record.arn.clone(),
            name: record.name.clone(),
            description: record.description.clone(),
            creator_account_id: record.creator_account_id.clone(),
            creator_display_name: record.creator_display_name.clone(),
            create_time: record.create_time,
            update_time: record.update_time,
            member_status: member_status.to_string(),
            membership_id: membership.map(|m| m.id.clone()),
            membership_arn: membership.map(|m| m.arn.clone()),
            data_encryption_metadata: record.data_encryption_metadata.clone(),
            query_log_status: record.query_log_status.clone(),
            job_log_status: record.job_log_status.clone(),
            analytics_engine: Some(record.analytics_engine.clone()),
            auto_approved_change_types: record.auto_approved_change_types.clone(),
            allowed_result_regions: record.allowed_result_regions.clone(),
            is_metrics_enabled: Some(record.is_metrics_enabled),
        })
    }

    /// The modelled `Collaboration` for a stored id, as `account_id` sees it.
    pub fn collaboration_view_by_id(
        &self,
        collaboration_id: &str,
        account_id: &str,
    ) -> Option<Collaboration> {
        let record = self.visible_collaboration(collaboration_id, account_id)?;
        self.collaboration_view(record, account_id)
    }

    /// The modelled `CollaborationSummary` as `account_id` sees it — the same derived
    /// members as [`Self::collaboration_view`], projected onto the summary shape.
    pub fn collaboration_summary(
        &self,
        record: &CollaborationRecord,
        account_id: &str,
    ) -> Option<CollaborationSummary> {
        let member_status = self.member_status_for(record, account_id)?;
        let membership = self.membership_of(&record.id, account_id);
        Some(CollaborationSummary {
            id: record.id.clone(),
            arn: record.arn.clone(),
            name: record.name.clone(),
            creator_account_id: record.creator_account_id.clone(),
            creator_display_name: record.creator_display_name.clone(),
            create_time: record.create_time,
            update_time: record.update_time,
            member_status: member_status.to_string(),
            membership_id: membership.map(|m| m.id.clone()),
            membership_arn: membership.map(|m| m.arn.clone()),
            analytics_engine: Some(record.analytics_engine.clone()),
        })
    }

    /// The modelled `MemberSummary` for one party to `record` — what a member listing
    /// reports about that account — or `None` when the account is not a party to the
    /// collaboration at all.
    ///
    /// Every member of the summary is derived from the collaboration as stored: the
    /// grant the creator declared for that account, the status it holds in the
    /// collaboration ([`Self::member_status_for`]), the membership it has joined with if
    /// any, and its own timestamps ([`CollaborationRecord::member_times_of`]).
    pub fn member_summary(
        &self,
        record: &CollaborationRecord,
        account_id: &str,
    ) -> Option<MemberSummary> {
        let grant = record.grant_for(account_id)?;
        let status = self.member_status_for(record, account_id)?;
        let membership = self.membership_of(&record.id, account_id);
        let times = record.member_times_of(account_id);
        Some(MemberSummary {
            account_id: account_id.to_string(),
            status: status.to_string(),
            display_name: grant.display_name,
            abilities: grant.member_abilities,
            ml_abilities: grant.ml_member_abilities,
            create_time: times.create_time,
            update_time: times.update_time,
            membership_id: membership.map(|membership| membership.id.clone()),
            membership_arn: membership.map(|membership| membership.arn.clone()),
            payment_configuration: member_payment_configuration(record, account_id),
        })
    }

    /// Every party to `record` as a [`MemberSummary`], in declaration order (see
    /// [`CollaborationRecord::party_accounts`]) — the order a member listing pages in,
    /// so a token handed out by one call resumes at the same row on the next one.
    pub fn member_summaries(&self, record: &CollaborationRecord) -> Vec<MemberSummary> {
        record
            .party_accounts()
            .iter()
            .filter_map(|account_id| self.member_summary(record, account_id))
            .collect()
    }

    /// Remove one party from a collaboration, as `DeleteMember` does.
    ///
    /// The party stays a member of the collaboration and keeps the grant it was declared
    /// with — "the removed member is placed in the Removed status", so a member listing
    /// still reports it, in [`MEMBER_STATUS_REMOVED`] — and its own `updateTime` moves to
    /// `at`, because the status that listing reports for it has changed. What it loses is
    /// its authorization to the collaboration ("can't interact with the collaboration"),
    /// which [`Self::collaboration_grant`] is what takes away, and the membership it had
    /// joined with moves to [`MEMBERSHIP_STATUS_REMOVED`]: the membership resource
    /// survives (it is deleted by `DeleteMembership`, with the sub-resources under it),
    /// but the account behind it is no longer part of the collaboration.
    ///
    /// Answers `false` when the collaboration holds no grant for `account_id`, so a
    /// caller refuses the call rather than recording a removal of a non-member, and
    /// `true` once the party is removed — including when it already was, since removing
    /// it again changes nothing.
    pub fn remove_member(&mut self, collaboration_id: &str, account_id: &str, at: DateTime) -> bool {
        let Some(record) = self.collaborations.get_mut(collaboration_id) else {
            return false;
        };
        if record.grant_for(account_id).is_none() {
            return false;
        }
        if !record.removed_members.insert(account_id.to_string()) {
            // Already removed: the collaboration is in the state this call asks for, so
            // nothing is restamped.
            return true;
        }
        let times = record.member_times_of(account_id);
        record.member_times.insert(
            account_id.to_string(),
            MemberTimes {
                create_time: times.create_time,
                update_time: at,
            },
        );
        let membership_id = self
            .membership_by_collaboration
            .get(&(collaboration_id.to_string(), account_id.to_string()))
            .cloned();
        if let Some(membership) = membership_id.and_then(|id| self.memberships.get_mut(&id)) {
            membership.status = MEMBERSHIP_STATUS_REMOVED.to_string();
            membership.update_time = at;
        }
        true
    }

    /// The analysis rule of `analysis_rule_type` stored for configured table
    /// `configured_table_id`, if any.
    pub fn configured_table_analysis_rule(
        &self,
        configured_table_id: &str,
        analysis_rule_type: &str,
    ) -> Option<&ConfiguredTableAnalysisRule> {
        self.configured_table_analysis_rules
            .get(&(configured_table_id.to_string(), analysis_rule_type.to_string()))
    }

    /// The analysis rule configured table `configured_table_id` already holds, if any.
    ///
    /// `CreateConfiguredTableAnalysisRule` documents "Currently, only one analysis rule
    /// can be created for a given configured table", so this is the record a further
    /// create collides with — whatever type it asks for. Which types the table holds a
    /// rule for lives on the table itself (`ConfiguredTable$analysisRuleTypes`), so this
    /// is a lookup rather than a scan of every stored rule.
    pub fn analysis_rule_of_configured_table(
        &self,
        configured_table_id: &str,
    ) -> Option<&ConfiguredTableAnalysisRule> {
        let table = self.configured_tables.get(configured_table_id)?;
        table
            .analysis_rule_types
            .iter()
            .find_map(|rule_type| self.configured_table_analysis_rule(configured_table_id, rule_type))
    }

    /// Drop the analysis rule `analysis_rule_type` of configured table
    /// `configured_table_id`, and stop reporting that type on the table.
    ///
    /// The mirror of [`Self::insert_configured_table_analysis_rule`]: the type leaves
    /// `ConfiguredTable$analysisRuleTypes` ("The types of analysis rules associated with
    /// this configured table") with the rule, and the configured table's `updateTime`
    /// moves to `at` because that member of it has changed. `None` means the pair held no
    /// rule, so nothing was dropped.
    pub fn remove_configured_table_analysis_rule(
        &mut self,
        configured_table_id: &str,
        analysis_rule_type: &str,
        at: ::aws_smithy_types::DateTime,
    ) -> Option<ConfiguredTableAnalysisRule> {
        let removed = self
            .configured_table_analysis_rules
            .remove(&(configured_table_id.to_string(), analysis_rule_type.to_string()))?;
        if let Some(table) = self.configured_tables.get_mut(configured_table_id) {
            table
                .analysis_rule_types
                .retain(|held| held != analysis_rule_type);
            table.update_time = at;
        }
        Some(removed)
    }

    /// Replace the policy of the analysis rule `analysis_rule_type` of configured table
    /// `configured_table_id`, and move the rule's `updateTime` to `at`.
    ///
    /// Answers `None` when the pair holds no rule, so a caller refuses the update rather
    /// than minting one: `UpdateConfiguredTableAnalysisRule` "updates a configured table
    /// analysis rule", and `CreateConfiguredTableAnalysisRule` is what mints one. The
    /// rule's `createTime` and the configured table's `analysisRuleTypes` are untouched —
    /// the set of types the table holds a rule for has not changed.
    pub fn update_configured_table_analysis_rule_policy(
        &mut self,
        configured_table_id: &str,
        analysis_rule_type: &str,
        policy: ConfiguredTableAnalysisRulePolicy,
        at: ::aws_smithy_types::DateTime,
    ) -> Option<&ConfiguredTableAnalysisRule> {
        let rule = self
            .configured_table_analysis_rules
            .get_mut(&(configured_table_id.to_string(), analysis_rule_type.to_string()))?;
        rule.policy = policy;
        rule.update_time = at;
        Some(rule)
    }

    /// Persist `rule` and record its type on the configured table it belongs to.
    ///
    /// `ConfiguredTable$analysisRuleTypes` is "The types of analysis rules associated
    /// with this configured table", so it is maintained here rather than recomputed by
    /// every reader, and the configured table's `updateTime` moves to the rule's instant
    /// because that member of it has changed.
    pub fn insert_configured_table_analysis_rule(&mut self, rule: ConfiguredTableAnalysisRule) {
        let configured_table_id = rule.configured_table_id.clone();
        let rule_type = rule.r#type.clone();
        if let Some(table) = self.configured_tables.get_mut(&configured_table_id) {
            if !table.analysis_rule_types.contains(&rule_type) {
                table.analysis_rule_types.push(rule_type.clone());
            }
            table.update_time = rule.update_time;
        }
        self.configured_table_analysis_rules
            .insert((configured_table_id, rule_type), rule);
    }

    /// The privacy budget template `collaboration_id` holds for `privacy_budget_type`,
    /// if any.
    ///
    /// A privacy budget template configures the budget of ONE `PrivacyBudgetType` (that
    /// member is required on the create and names exactly one member of the parameters
    /// union), so this — the template of the SAME type — is the record a second create
    /// collides with, and a template of another type is a different resource.
    pub fn privacy_budget_template_of_type(
        &self,
        collaboration_id: &str,
        privacy_budget_type: &str,
    ) -> Option<&PrivacyBudgetTemplate> {
        let id = self.privacy_budget_template_by_collaboration.get(&(
            collaboration_id.to_string(),
            privacy_budget_type.to_string(),
        ))?;
        self.privacy_budget_templates.get(id)
    }

    /// The configured table association `membership_id` already holds under the table
    /// alias `name`, if any — the record a repeat create collides with.
    pub fn configured_table_association_named(
        &self,
        membership_id: &str,
        name: &str,
    ) -> Option<&ConfiguredTableAssociation> {
        let id = self
            .configured_table_association_by_name
            .get(&(membership_id.to_string(), name.to_string()))?;
        self.configured_table_associations.get(id)
    }

    /// The stored configured table association a `configuredTableAssociationIdentifier`
    /// addresses under `membership_id`, or `None` when the membership holds none the
    /// identifier names.
    ///
    /// An association is a sub-resource of one membership (every operation addresses it as
    /// `/memberships/{membershipIdentifier}/configuredTableAssociations/{identifier}`), so
    /// one of another membership is not visible through this one.
    ///
    /// The identifier itself is NOT only the association's own id: the member is
    /// documented as accepting either the association ("Currently accepts the configured
    /// table association ID" —
    /// `CreateConfiguredTableAssociationAnalysisRuleInput$configuredTableAssociationIdentifier`)
    /// or the configured table the association was created for ("Currently accepts the
    /// configured table ID" — `GetConfiguredTableAssociationInput` and
    /// `DeleteConfiguredTableAssociationInput`). Both are resolved here, in that order, so
    /// every operation that addresses an association accepts both forms and no caller has
    /// to know which of the two an operation's doc happens to name. When a membership
    /// holds several associations of one configured table, the table's id resolves to the
    /// first of them in creation order, so the answer does not depend on map iteration
    /// order.
    pub fn configured_table_association_in(
        &self,
        membership_id: &str,
        association_identifier: &str,
    ) -> Option<&ConfiguredTableAssociation> {
        self.configured_table_associations
            .get(association_identifier)
            .filter(|association| association.membership_id == membership_id)
            .or_else(|| {
                self.configured_table_association_order
                    .iter()
                    .filter_map(|id| self.configured_table_associations.get(id))
                    .find(|association| {
                        association.membership_id == membership_id
                            && association.configured_table_id == association_identifier
                    })
            })
    }

    /// The analysis rule of `analysis_rule_type` stored for the configured table
    /// association `association_id`, if any.
    ///
    /// The pair is the rule's identity — every association analysis-rule operation
    /// addresses one through it — so this is a lookup rather than a scan.
    pub fn configured_table_association_analysis_rule(
        &self,
        association_id: &str,
        analysis_rule_type: &str,
    ) -> Option<&ConfiguredTableAssociationAnalysisRule> {
        self.configured_table_association_analysis_rules
            .get(&(association_id.to_string(), analysis_rule_type.to_string()))
    }

    /// Persist `rule` and record its type on the configured table association it belongs
    /// to.
    ///
    /// `ConfiguredTableAssociation$analysisRuleTypes` is "the analysis rule types for the
    /// configured table association", so it is maintained here rather than recomputed by
    /// every reader, and the association's `updateTime` moves to the rule's instant
    /// because that member of it has changed. The mirror of
    /// [`Self::insert_configured_table_analysis_rule`] for the association plane.
    pub fn insert_configured_table_association_analysis_rule(
        &mut self,
        rule: ConfiguredTableAssociationAnalysisRule,
    ) {
        let association_id = rule.configured_table_association_id.clone();
        let rule_type = rule.r#type.clone();
        if let Some(association) = self.configured_table_associations.get_mut(&association_id) {
            let held = association.analysis_rule_types.get_or_insert_with(Vec::new);
            if !held.contains(&rule_type) {
                held.push(rule_type.clone());
            }
            association.update_time = rule.update_time;
        }
        self.configured_table_association_analysis_rules
            .insert((association_id, rule_type), rule);
    }

    /// The stored template `template_id`, but only when it belongs to
    /// `collaboration_id`.
    ///
    /// A privacy budget template is a resource of the COLLABORATION, and the read
    /// operations address it through a membership ("the privacy budget template is
    /// retrieved from the collaboration that this membership belongs to"), so a
    /// template of another collaboration is not visible through this membership.
    pub fn privacy_budget_template_in_collaboration(
        &self,
        collaboration_id: &str,
        template_id: &str,
    ) -> Option<&PrivacyBudgetTemplate> {
        self.privacy_budget_templates
            .get(template_id)
            .filter(|template| template.collaboration_id == collaboration_id)
    }

    /// Whether this store holds a resource under `resource_arn`.
    ///
    /// Every tagging operation addresses its resource by the ARN that resource was
    /// minted with, so the ARN is resolved against the stored records that CARRY one
    /// rather than against the ARN's text: a well-formed ARN of a resource that was
    /// never created, or one a delete removed, holds nothing — which is what the 404
    /// those operations model reports. Every kind of resource this slice persists an
    /// ARN for is covered, so one kind's tags are not reachable while another's are.
    pub fn holds_resource_at_arn(&self, resource_arn: &str) -> bool {
        self.collaborations
            .values()
            .any(|record| record.arn == resource_arn)
            || self
                .memberships
                .values()
                .any(|membership| membership.arn == resource_arn)
            || self
                .configured_tables
                .values()
                .any(|table| table.arn == resource_arn)
            || self
                .analysis_templates
                .values()
                .any(|template| template.arn == resource_arn)
            || self
                .privacy_budget_templates
                .values()
                .any(|template| template.arn == resource_arn)
            || self
                .configured_table_associations
                .values()
                .any(|association| association.arn == resource_arn)
    }

    /// Drop every configured table association of `membership_id` — the records, their
    /// order entries, their name index entries, their analysis rules and their tags.
    ///
    /// An association is a sub-resource of ONE membership (both of its operations address
    /// it under that membership), so it is not reachable once the membership is gone: it
    /// goes with it rather than staying behind, counting against the store's cap and
    /// holding its tags alive. An analysis rule is a sub-resource of ONE association, so
    /// it goes the same way.
    pub fn remove_configured_table_associations_of(&mut self, membership_id: &str) {
        let ids: Vec<String> = self
            .configured_table_associations
            .values()
            .filter(|association| association.membership_id == membership_id)
            .map(|association| association.id.clone())
            .collect();
        for id in ids {
            if let Some(association) = self.configured_table_associations.remove(&id) {
                self.tags.remove(&association.arn);
            }
            self.configured_table_association_order.retain(|held| held != &id);
            // An association's analysis rules are reachable only through it, so they go
            // with it rather than staying behind under a key nothing resolves.
            self.configured_table_association_analysis_rules
                .retain(|(association, _), _| association != &id);
        }
        self.configured_table_association_by_name
            .retain(|(membership, _), _| membership != membership_id);
    }

    /// The tags stored for `resource_arn`: the empty map for a resource nobody has
    /// tagged, which is what a resource carrying no tags reports.
    pub fn tags_of(&self, resource_arn: &str) -> HashMap<String, String> {
        self.tags.get(resource_arn).cloned().unwrap_or_default()
    }

    /// How many tags `resource_arn` would carry if `tags` were added to it: a key it
    /// already carries takes a new value rather than adding an entry, so only the keys
    /// it does not yet carry grow the set.
    pub fn tag_count_after_adding(
        &self,
        resource_arn: &str,
        tags: &HashMap<String, String>,
    ) -> usize {
        match self.tags.get(resource_arn) {
            None => tags.len(),
            Some(stored) => {
                stored.len() + tags.keys().filter(|key| !stored.contains_key(*key)).count()
            }
        }
    }

    /// Add `tags` to the tags of `resource_arn`.
    ///
    /// A key the resource already carries takes the new value and every other key stays
    /// where it is, so tagging a resource twice leaves it carrying the union of both
    /// calls rather than only the second one's keys.
    pub fn add_tags(&mut self, resource_arn: &str, tags: HashMap<String, String>) {
        if tags.is_empty() {
            return;
        }
        self.tags
            .entry(resource_arn.to_string())
            .or_default()
            .extend(tags);
    }

    /// Remove `tag_keys` from the tags of `resource_arn`.
    ///
    /// A key the resource does not carry removes nothing, and a resource left with no
    /// tags holds no entry at all — the same state it was in before anybody tagged it,
    /// so the two are indistinguishable to a later read.
    pub fn remove_tags(&mut self, resource_arn: &str, tag_keys: &[String]) {
        let Some(tags) = self.tags.get_mut(resource_arn) else {
            return;
        };
        for key in tag_keys {
            tags.remove(key);
        }
        if tags.is_empty() {
            self.tags.remove(resource_arn);
        }
    }

    /// Materialize every protected-query transition the CLOCK already owes at `now`.
    ///
    /// A query's terminal status falls due [`PROTECTED_QUERY_SETTLE_DELAY`] after the
    /// call that started it, whether or not anybody calls again, so a reporting
    /// operation applies the ones already owed before it answers. It changes nothing
    /// that is not yet due: a caller reading a query inside that span sees it
    /// SUBMITTED.
    pub fn settle_protected_queries(&mut self, now: SystemTime) {
        for record in self.protected_queries.values_mut() {
            if record.terminal_due.is_none_or(|due| due > now) {
                continue;
            }
            record.terminal_due = None;
            record.query.status = PROTECTED_QUERY_STATUS_FAILED.to_string();
            // The run was never performed, so the query carries the failure's reason
            // and NO statistics or result: there is no duration to report for work
            // nothing did.
            record.query.error = Some(ProtectedQueryError {
                code: PROTECTED_QUERY_FAILURE_CODE.to_string(),
                message: PROTECTED_QUERY_FAILURE_MESSAGE.to_string(),
            });
        }
    }

    /// The protected query `query_id`, but only when the membership that started it is
    /// `membership_id`: a query is addressed under its own membership, so one
    /// membership never reads another's query.
    pub fn protected_query(
        &self,
        membership_id: &str,
        query_id: &str,
    ) -> Option<&ProtectedQueryRecord> {
        self.protected_queries
            .get(query_id)
            .filter(|record| record.query.membership_id == membership_id)
    }

    /// The change request `change_request_id`, but only when it was made against
    /// `collaboration_id`: a change request is addressed under its collaboration
    /// (`/collaborations/{collaborationIdentifier}/changeRequests/{changeRequestIdentifier}`),
    /// so one collaboration never reads another's change request.
    pub fn collaboration_change_request_in_collaboration(
        &self,
        collaboration_id: &str,
        change_request_id: &str,
    ) -> Option<&CollaborationChangeRequest> {
        self.collaboration_change_requests
            .get(change_request_id)
            .filter(|request| request.collaboration_id == collaboration_id)
    }

    /// Every account whose approval a change request against `record` needs, in a
    /// deterministic order: the creator first, then the declared members in the order
    /// the collaboration was created with.
    ///
    /// `CollaborationChangeRequest$approvals` is documented as "approval details from
    /// collaboration members, including approval status and multi-party approval
    /// workflow information" and is keyed by `AccountId`. The model names no finer rule
    /// for WHICH accounts those are, so this slice takes the parties to the
    /// collaboration — the accounts whose grants the collaboration holds — as the
    /// parties to a change of it. The account a change ADDS is not among them: it is the
    /// subject of the change, not yet a party to the collaboration.
    pub fn change_request_approvals(&self, record: &CollaborationRecord) -> Vec<String> {
        record.party_accounts()
    }

    /// Apply one COMMITTED change to the collaboration it was made against.
    ///
    /// A `MemberChangeSpecification` carries the member's whole intended grant
    /// (`memberAbilities` is required on it), so applying it REPLACES that account's
    /// stored grant — creating it when the account was not a member yet — and applying a
    /// `CollaborationChangeSpecification` replaces the collaboration's
    /// `autoApprovedChangeTypes`. The collaboration's `updateTime` is restamped, and
    /// when the affected account already holds a membership of this collaboration its
    /// `memberAbilities`/`mlMemberAbilities` are restated too, because a membership
    /// reports the same grant from the member's side.
    pub fn apply_collaboration_change(
        &mut self,
        collaboration_id: &str,
        change: &Change,
        at: DateTime,
    ) {
        let Some(record) = self.collaborations.get_mut(collaboration_id) else {
            return;
        };
        let member_spec = match &change.specification {
            ChangeSpecification::Collaboration(spec) => {
                record.auto_approved_change_types = spec.auto_approved_change_types.clone();
                record.update_time = at;
                return;
            }
            ChangeSpecification::Member(spec) => spec,
        };

        if member_spec.account_id == record.creator_account_id {
            record.creator_member_abilities = member_spec.member_abilities.clone();
            record.creator_ml_member_abilities = member_spec.ml_member_abilities.clone();
            if member_spec.payment_configuration.is_some() {
                record.creator_payment_configuration = member_spec.payment_configuration.clone();
            }
            if let Some(display_name) = &member_spec.display_name {
                record.creator_display_name = display_name.clone();
            }
        } else if let Some(member) = record
            .members
            .iter_mut()
            .find(|member| member.account_id == member_spec.account_id)
        {
            member.member_abilities = member_spec.member_abilities.clone();
            member.ml_member_abilities = member_spec.ml_member_abilities.clone();
            if member_spec.payment_configuration.is_some() {
                member.payment_configuration = member_spec.payment_configuration.clone();
            }
            if let Some(display_name) = &member_spec.display_name {
                member.display_name = display_name.clone();
            }
        } else if let Some(display_name) = &member_spec.display_name {
            // `MemberSpecification$displayName` is required, so a member can only be
            // materialized with the display name the specification carried;
            // `CreateCollaborationChangeRequest` refuses an add without one, so the
            // `else` case here is unreachable rather than silently guessed at.
            record.members.push(MemberSpecification {
                account_id: member_spec.account_id.clone(),
                member_abilities: member_spec.member_abilities.clone(),
                ml_member_abilities: member_spec.ml_member_abilities.clone(),
                display_name: display_name.clone(),
                payment_configuration: member_spec.payment_configuration.clone(),
            });
        }
        record.update_time = at;
        // A committed change that states a grant for an account makes that account part
        // of the collaboration again, so a removal recorded for it no longer holds.
        record.removed_members.remove(&member_spec.account_id);

        // The party's OWN timestamps move with the grant this change states: an account
        // the change added holds its member record from now on, and one whose grant the
        // change restated was last updated now. A party the change did not name keeps
        // its pair, which is why these are stamped per account rather than taken from
        // the collaboration's `updateTime`.
        if record.grant_for(&member_spec.account_id).is_some() {
            record
                .member_times
                .entry(member_spec.account_id.clone())
                .or_insert(MemberTimes {
                    create_time: at,
                    update_time: at,
                })
                .update_time = at;
        }

        let membership_id = self
            .membership_by_collaboration
            .get(&(
                collaboration_id.to_string(),
                member_spec.account_id.clone(),
            ))
            .cloned();
        if let Some(membership) = membership_id.and_then(|id| self.memberships.get_mut(&id)) {
            membership.member_abilities = member_spec.member_abilities.clone();
            membership.ml_member_abilities = member_spec.ml_member_abilities.clone();
            membership.update_time = at;
        }
    }
}

/// The change types one specification asks of the collaboration as stored — the
/// `Change$types` the service reports back ("the list of change types that were
/// applied").
///
/// EMPTY means the specification asks for nothing the collaboration does not already
/// have, or asks for something the `ChangeType` catalog cannot name (a CAN_QUERY or
/// CAN_RUN_JOB ability change, say): `ChangeTypeList` is `len 1..`, so such a change is
/// not representable and the create refuses it.
pub fn change_types_for(
    record: &CollaborationRecord,
    specification: &ChangeSpecification,
) -> Vec<String> {
    match specification {
        ChangeSpecification::Member(spec) => match record.grant_for(&spec.account_id) {
            // An account the collaboration holds no grant for is being added.
            None => vec![CHANGE_TYPE_ADD_MEMBER.to_string()],
            Some(current) => member_change_types(&current, spec),
        },
        ChangeSpecification::Collaboration(spec) => {
            let requested = spec.auto_approved_change_types.as_deref().unwrap_or(&[]);
            let current = record.auto_approved_change_types.as_deref().unwrap_or(&[]);
            if same_change_type_set(requested, current) {
                Vec::new()
            } else {
                vec![CHANGE_TYPE_EDIT_AUTO_APPROVED_CHANGE_TYPES.to_string()]
            }
        }
    }
}

/// The change types a specification asks for an account that is ALREADY a party to the
/// collaboration: the difference between the grant it holds and the one the
/// specification states, named from the `ChangeType` catalog. See
/// [`MEMBER_ABILITY_CHANGE_TYPES`] and [`ML_MEMBER_ABILITY_CHANGE_TYPES`].
fn member_change_types(current: &MemberGrant, requested: &MemberChangeSpecification) -> Vec<String> {
    let mut types = Vec::new();
    for (ability, grant, revoke) in MEMBER_ABILITY_CHANGE_TYPES {
        let held = current.member_abilities.iter().any(|held| held == ability);
        let wanted = requested
            .member_abilities
            .iter()
            .any(|wanted| wanted == ability);
        match (held, wanted) {
            (false, true) => types.push(grant.to_string()),
            (true, false) => types.push(revoke.to_string()),
            _ => {}
        }
    }
    for (ability, grant, revoke) in ML_MEMBER_ABILITY_CHANGE_TYPES {
        let held = holds_ml_ability(current.ml_member_abilities.as_ref(), ability);
        let wanted = holds_ml_ability(requested.ml_member_abilities.as_ref(), ability);
        match (held, wanted) {
            (false, true) => types.push(grant.to_string()),
            (true, false) => types.push(revoke.to_string()),
            _ => {}
        }
    }
    // A member is a payer candidate while its payment configuration makes it
    // responsible for any cost; the catalog's ADD_PAYER_CANDIDATE /
    // REMOVE_PAYER_CANDIDATE name entering and leaving that set.
    let was_payer = is_payer_candidate(current.payment_configuration.as_ref());
    let is_payer = is_payer_candidate(requested.payment_configuration.as_ref());
    match (was_payer, is_payer) {
        (false, true) => types.push(CHANGE_TYPE_ADD_PAYER_CANDIDATE.to_string()),
        (true, false) => types.push(CHANGE_TYPE_REMOVE_PAYER_CANDIDATE.to_string()),
        _ => {}
    }
    types
}

/// Whether `abilities` carries the `CustomMLMemberAbility` `ability`.
fn holds_ml_ability(abilities: Option<&MLMemberAbilities>, ability: &str) -> bool {
    abilities.is_some_and(|abilities| {
        abilities
            .custom_ml_member_abilities
            .iter()
            .any(|held| held == ability)
    })
}

/// Whether a payment configuration makes its member responsible for any cost: query
/// compute, job compute, or one of the machine-learning costs.
fn is_payer_candidate(configuration: Option<&PaymentConfiguration>) -> bool {
    let Some(configuration) = configuration else {
        return false;
    };
    let ml = configuration.machine_learning.as_ref();
    configuration.query_compute.is_responsible
        || configuration
            .job_compute
            .as_ref()
            .is_some_and(|compute| compute.is_responsible)
        || ml.is_some_and(|ml| {
            ml.model_training
                .as_ref()
                .is_some_and(|cost| cost.is_responsible)
                || ml
                    .model_inference
                    .as_ref()
                    .is_some_and(|cost| cost.is_responsible)
                || ml
                    .synthetic_data_generation
                    .as_ref()
                    .is_some_and(|cost| cost.is_responsible)
        })
}

/// Whether two `autoApprovedChangeTypes` lists name the same set — the lists are
/// `uniqueItems`, so set equality is length plus containment, and ORDER is not a change.
fn same_change_type_set(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().all(|value| right.contains(value))
}

/// Whether a change request needs NO decision from another party, so the service settles
/// it at creation — the `isAutoApproved` the created change request reports ("whether the
/// change request was automatically approved based on the collaboration's auto-approval
/// settings").
///
/// A request is automatically approved when no approval it needs is still outstanding,
/// which happens two ways:
///
/// * the requesting account is the only party whose approval the request needs, so there
///   is no other party left to decide — the request is its own approval; or
/// * the collaboration's `autoApprovedChangeTypes` covers every change type the request
///   asks for, so no party has to decide at all. `AutoApprovedChangeType` is a 5-value
///   subset of the 12-value `ChangeType` catalog, so a change type outside that subset is
///   never covered this way.
///
/// `approvers` are the parties whose approval the request needs (see
/// [`CleanRoomsState::change_request_approvals`]) and `requester` is the account that
/// created it.
pub fn change_request_is_auto_approved(
    record: &CollaborationRecord,
    changes: &[Change],
    approvers: &[String],
    requester: &str,
) -> bool {
    if changes.is_empty() {
        return false;
    }
    if approvers.iter().all(|account| account == requester) {
        return true;
    }
    let auto_approved = record.auto_approved_change_types.as_deref().unwrap_or(&[]);
    changes.iter().all(|change| {
        change
            .types
            .iter()
            .all(|change_type| auto_approved.contains(change_type))
    })
}

/// Whether a change request's status admits no modification at all: a COMMITTED request
/// has been applied to the collaboration and a CANCELLED one was withdrawn, so neither
/// can be moved again.
///
/// A DENIED request is NOT terminal in this sense — the service's own refusal states the
/// rule as "Change requests in COMMITTED or CANCELLED status can't be modified", so a
/// denied request can still be cancelled.
pub fn change_request_is_terminal(status: &str) -> bool {
    status == CHANGE_REQUEST_STATUS_COMMITTED || status == CHANGE_REQUEST_STATUS_CANCELLED
}

/// The message `UpdateCollaborationChangeRequest` reports when the change request the
/// request addresses can no longer be modified.
///
/// Composed from the request's `changeRequestIdentifier` and the status that request is
/// stored in, plus the rule those two fail: the id says WHICH request was refused, and
/// the status says why and which statuses the rule names.
pub fn change_request_not_modifiable_message(change_request_id: &str, status: &str) -> String {
    format!(
        "You can't update change request {change_request_id} because it's in {status} status. \
         Change requests in {} or {} status can't be modified.",
        CHANGE_REQUEST_STATUS_COMMITTED, CHANGE_REQUEST_STATUS_CANCELLED
    )
}

/// The `ChangeRequestStatus` a change request's recorded votes put it in.
///
/// `ApprovalStatusDetails$status` is "the approval status of a member's vote on the
/// change request … PENDING (if they haven't voted), APPROVED, or DENIED", and
/// `UpdateCollaborationChangeRequest` documents DENY as rejecting the change, so one
/// DENIED vote settles the request as DENIED, a request every party has APPROVED is
/// APPROVED (ready for the owner to COMMIT), and one still holding a PENDING vote stays
/// PENDING. An EMPTY tally is PENDING: there is no vote to conclude anything from.
pub fn change_request_status_from_approvals(
    approvals: &HashMap<String, ApprovalStatusDetails>,
) -> &'static str {
    if approvals
        .values()
        .any(|details| details.status == APPROVAL_STATUS_DENIED)
    {
        return CHANGE_REQUEST_STATUS_DENIED;
    }
    if !approvals.is_empty()
        && approvals
            .values()
            .all(|details| details.status == APPROVAL_STATUS_APPROVED)
    {
        return CHANGE_REQUEST_STATUS_APPROVED;
    }
    CHANGE_REQUEST_STATUS_PENDING
}

/// The modelled `MembershipSummary` projection of a stored `Membership`.
pub fn membership_summary(membership: &Membership) -> MembershipSummary {
    MembershipSummary {
        id: membership.id.clone(),
        arn: membership.arn.clone(),
        collaboration_arn: membership.collaboration_arn.clone(),
        collaboration_id: membership.collaboration_id.clone(),
        collaboration_creator_account_id: membership.collaboration_creator_account_id.clone(),
        collaboration_creator_display_name: membership.collaboration_creator_display_name.clone(),
        collaboration_name: membership.collaboration_name.clone(),
        create_time: membership.create_time,
        update_time: membership.update_time,
        status: membership.status.clone(),
        member_abilities: membership.member_abilities.clone(),
        ml_member_abilities: membership.ml_member_abilities.clone(),
        payment_configuration: membership.payment_configuration.clone(),
    }
}

/// The modelled `AnalysisTemplateSummary` projection of a stored `AnalysisTemplate`.
///
/// `isSyntheticData` ("Indicates if this analysis template summary generated synthetic
/// data") is derived from whether the template carries `syntheticDataParameters`, which
/// is what makes an analysis generate synthetic data.
pub fn analysis_template_summary(template: &AnalysisTemplate) -> AnalysisTemplateSummary {
    AnalysisTemplateSummary {
        arn: template.arn.clone(),
        create_time: template.create_time,
        id: template.id.clone(),
        name: template.name.clone(),
        update_time: template.update_time,
        membership_arn: template.membership_arn.clone(),
        membership_id: template.membership_id.clone(),
        collaboration_arn: template.collaboration_arn.clone(),
        collaboration_id: template.collaboration_id.clone(),
        description: template.description.clone(),
        is_synthetic_data: Some(template.synthetic_data_parameters.is_some()),
    }
}

/// The modelled `PrivacyBudgetTemplateSummary` projection of a stored
/// `PrivacyBudgetTemplate` — the same record, narrowed to the members the summary shape
/// declares (it carries no `autoRefresh` and no `parameters`).
pub fn privacy_budget_template_summary(
    template: &PrivacyBudgetTemplate,
) -> PrivacyBudgetTemplateSummary {
    PrivacyBudgetTemplateSummary {
        id: template.id.clone(),
        arn: template.arn.clone(),
        membership_id: template.membership_id.clone(),
        membership_arn: template.membership_arn.clone(),
        collaboration_id: template.collaboration_id.clone(),
        collaboration_arn: template.collaboration_arn.clone(),
        privacy_budget_type: template.privacy_budget_type.clone(),
        create_time: template.create_time,
        update_time: template.update_time,
    }
}

/// The modelled `ConfiguredTableAssociationSummary` projection of a stored
/// `ConfiguredTableAssociation` — the same record, narrowed to the members the summary
/// shape declares.
pub fn configured_table_association_summary(
    association: &ConfiguredTableAssociation,
) -> ConfiguredTableAssociationSummary {
    ConfiguredTableAssociationSummary {
        id: association.id.clone(),
        arn: association.arn.clone(),
        configured_table_id: association.configured_table_id.clone(),
        membership_id: association.membership_id.clone(),
        membership_arn: association.membership_arn.clone(),
        name: association.name.clone(),
        create_time: association.create_time,
        update_time: association.update_time,
        analysis_rule_types: association.analysis_rule_types.clone(),
    }
}

/// The account id every emulated caller presents, from the vendored runtime's single
/// source of truth. Never a literal.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The principal every request to this emulator is authenticated as: the one IAM user of
/// the single account it serves.
///
/// The emulator verifies a request's signature but holds no IAM directory behind the
/// credential it was signed with, so there is one principal an authorization decision can
/// name — the identity `sts:GetCallerIdentity` answers with for a credential that names
/// no session, which every other service of this emulator names the same way.
pub fn caller_principal() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The resource an authorization failure names when the operation it refused addresses no
/// single resource — one whose identifiers all travel in the body, so the decision was
/// taken on the action and not on a resource.
pub const UNSCOPED_RESOURCE: &str = "*";

/// The message an operation reports when the caller is not authorized to perform it —
/// see [`CleanRoomsState::collaboration_grant`] for the cases that reach it.
///
/// One wording for the whole class, in the three parts an authorization failure is made
/// of: WHO was refused (the calling principal's ARN), WHAT was asked for (this service's
/// IAM action, the service prefix plus the operation name) and WHICH resource it was asked
/// for (that resource's ARN, or [`UNSCOPED_RESOURCE`] where the operation addresses no
/// single resource). Those three are what a caller reads a denial for, so each is a fact
/// of THIS request and nothing is kept from one and replayed into another. It
/// deliberately does not say whether a resource is stored under the identifier.
pub fn authorization_denied_message(operation: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {ARN_SERVICE}:{operation} on resource: {resource}",
        caller_principal()
    )
}

/// The message a membership lookup reports when the id names no membership of the
/// caller's.
///
/// Composed from the request's `membershipIdentifier` and the resolved caller account:
/// membership ids are account-scoped, so the account is what says the lookup failed in
/// THIS account's namespace rather than that the id exists nowhere.
pub fn membership_not_found_message(membership_id: &str, account_id: &str) -> String {
    format!("No such membership: {membership_id} for account {account_id}")
}

/// The message an analysis-template lookup reports when the identifier names no template
/// of the membership in the path.
///
/// Composed from the request's two identifiers: a template is a sub-resource of ONE
/// membership, so the membership is what says the lookup failed in that membership's
/// namespace rather than that the template id exists nowhere. Shared by every operation
/// that addresses a stored template, so a read and a delete of the same resource report
/// the same thing.
pub fn analysis_template_not_found_message(template_id: &str, membership_id: &str) -> String {
    format!("analysis template {template_id} not found in membership {membership_id}")
}

/// The message `CreateMembership` reports when the account it would create a membership
/// for is not [`MEMBER_STATUS_INVITED`] to the collaboration — because it already holds
/// one, so it is [`MEMBER_STATUS_ACTIVE`].
///
/// Composed from the resolved caller account plus the precondition that account fails: a
/// membership is creatable only for a member the collaboration still has outstanding, so
/// the account is the value that fails validation and the INVITED state is what would
/// make the call succeed.
pub fn membership_not_invited_message(account_id: &str) -> String {
    format!(
        "Cannot create membership for account: {account_id}. \
         Memberships can only be created for members in {MEMBER_STATUS_INVITED} state"
    )
}

/// The message `DeleteMember` reports when the account it would remove is the
/// collaboration's own creator.
///
/// Entirely FIXED text, like [`CREATOR_MEMBERSHIP_ACTIVE_CONFLICT_MESSAGE`]: the refusal
/// states the rule the call broke — the creator owns the collaboration ("DeleteCollaboration
/// … can only be called by the collaboration owner" is how the collaboration itself goes),
/// so it is the one party a removal cannot take out — and names no value of the request.
/// The resource involved travels in the exception's `resourceId` / `resourceType` members,
/// which is where a client reads it.
pub const CREATOR_MEMBER_REMOVAL_CONFLICT_MESSAGE: &str =
    "Cannot delete member for the creator of the collaboration.";

/// The message `DeleteMember` reports when the `accountId` names no party to the
/// collaboration in the path.
///
/// Composed from the request's two identifiers: a member exists WITHIN one collaboration,
/// so the collaboration is what says the lookup failed in that collaboration's roster
/// rather than that the account exists nowhere.
pub fn member_not_found_message(account_id: &str, collaboration_id: &str) -> String {
    format!("No such member: {account_id} in collaboration {collaboration_id}")
}

/// The message `DeleteMembership` reports when the membership it addresses is the one the
/// collaboration's own creator holds in a collaboration that still exists.
///
/// Entirely FIXED text: the refusal states the rule the call broke — a creator keeps its
/// membership for as long as that membership is [`MEMBERSHIP_STATUS_ACTIVE`] — and names
/// no value of the request, so no membership, collaboration or account id is interpolated
/// into it. The ids of the resources involved travel in the exception's `resourceId` /
/// `resourceType` members, which is where a client reads them.
pub const CREATOR_MEMBERSHIP_ACTIVE_CONFLICT_MESSAGE: &str =
    "Cannot delete membership for the creator of the collaboration while their membership \
     is still ACTIVE.";

/// The message `CreateConfiguredTableAnalysisRule` reports when the configured table it
/// addresses already holds an analysis rule.
///
/// Two clauses: the constraint the request broke ("only one analysis rule per configured
/// table") and what the stored table already holds. Composed from the request's
/// `configuredTableIdentifier` and the TYPE of the rule the table is holding — the second
/// clause is a statement about stored state, so the type is read from that state rather
/// than echoed from the request, which is what makes it true when the request asks for
/// another type.
pub fn configured_table_analysis_rule_exists_message(
    configured_table_identifier: &str,
    held_analysis_rule_type: &str,
) -> String {
    format!(
        "Only one Analysis Rule is allowed per Configured Table Configured Table \
         {configured_table_identifier} already has a {held_analysis_rule_type} analysis rule."
    )
}

/// The message a privacy-budget-template lookup reports when the identifier names no
/// template reachable through the membership in the path.
///
/// Composed from the request's two identifiers: a template is read and deleted through ONE
/// membership (`/memberships/{membershipIdentifier}/privacybudgettemplates/{id}`), so the
/// membership is what says the lookup failed in the namespace that membership reaches
/// rather than that the template id exists nowhere. Shared by every operation that
/// addresses a stored template, so a read and a delete of the same resource report the
/// same thing.
pub fn privacy_budget_template_not_found_message(template_id: &str, membership_id: &str) -> String {
    format!("No privacy budget template found with id {template_id} for membership {membership_id}.")
}

/// The message `CreatePrivacyBudgetTemplate` reports when the collaboration already holds
/// a template of the budget type the request asks for.
///
/// Composed from the request's `privacyBudgetType` and the collaboration the membership
/// belongs to: those two are the uniqueness key the create collides on, so naming them is
/// what tells a caller that a template of ANOTHER budget type would still be accepted.
pub fn privacy_budget_template_exists_message(
    privacy_budget_type: &str,
    collaboration_id: &str,
) -> String {
    format!(
        "A Privacy Budget Template for {} already exists in collaboration {collaboration_id}.",
        privacy_budget_type_display(privacy_budget_type)
    )
}

/// A `PrivacyBudgetType` as prose: the catalog value's underscore-separated words, each
/// capitalised (DIFFERENTIAL_PRIVACY -> "Differential Privacy"). The whole catalog is
/// covered, so a value added to `PrivacyBudgetType` reads correctly without a second
/// table to maintain.
pub fn privacy_budget_type_display(privacy_budget_type: &str) -> String {
    capitalised_words(privacy_budget_type, " ")
}

/// A SHOUTY_SNAKE_CASE catalog value as capitalised words joined by `separator`: each
/// underscore-separated word keeps its first character upper-cased and the rest lowered
/// (DIFFERENTIAL_PRIVACY -> "Differential Privacy" with " ", "DifferentialPrivacy" with
/// "").
///
/// One implementation of the capitalisation rule, so the refusal messages that render a
/// catalog value as prose and the ones that render it as a class name cannot drift apart.
fn capitalised_words(catalog_value: &str, separator: &str) -> String {
    catalog_value
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(separator)
}

/// The account segment of `role_arn`, when the value is an IAM role ARN at all.
///
/// The form is the one the model's `RoleArn` pattern declares,
/// `arn:aws:iam::<account>:role/<name>`: six colon-separated fields, an empty region
/// (IAM is global), and a resource segment introduced by `role/`. A role name carries no
/// colon (the pattern admits `[\w+=./@-]+`), so a value with more fields than that names
/// no role. `None` means the value is not an IAM role ARN.
pub fn iam_role_arn_account(role_arn: &str) -> Option<&str> {
    let mut fields = role_arn.splitn(6, ':');
    let (
        Some("arn"),
        Some("aws"),
        Some("iam"),
        Some(""),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    let name = resource.strip_prefix("role/")?;
    let account_is_word = !account.is_empty()
        && account
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    let name_is_role = !name.is_empty() && !name.contains(':');
    (account_is_word && name_is_role).then_some(account)
}

/// Whether `role_arn` names a role `account_id` can pass to this service.
///
/// The service "will assume this role to access catalog metadata and query the table"
/// (`CreateConfiguredTableAssociationInput$roleArn`), so the value is used as a role
/// rather than parsed as a string: what decides is whether it names a role OF THE
/// CALLER'S OWN account. A value that names no IAM role names no role in this account
/// either, so both it and a role of another account are roles this caller cannot pass.
pub fn role_arn_is_passable(role_arn: &str, account_id: &str) -> bool {
    iam_role_arn_account(role_arn).is_some_and(|account| account == account_id)
}

/// The message an operation reports when the `roleArn` it was given is not a role the
/// caller can pass to the service. Fixed text: the refusal is about the pass-role
/// decision, which names no value of the request.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// The message a create reports when the membership it addresses already holds the
/// resource it would create — the "already exists" conflict class
/// ([`CONFLICT_REASON_ALREADY_EXISTS`]) of every resource a membership owns.
///
/// Fixed text: the leading `The `, the `: with membership id: ` that introduces the
/// membership, and the trailing ` already exists.`. The variable parts are `resource` —
/// the STORED resource this create collided with, named by its kind and the identifier
/// the service minted for it, so it is read from the store rather than echoed from the
/// request — and the membership the request addressed, which is the scope the resource is
/// unique in. Written once here, so every membership-scoped create reports one sentence
/// with its own resource in it; a conflict scoped to something OTHER than a membership —
/// a collaboration-wide one, say — is a different class and words itself.
pub fn resource_exists_in_membership_message(resource: &str, membership_id: &str) -> String {
    format!("The {resource}: with membership id: {membership_id} already exists.")
}

/// A stored resource named by its kind and the identifier the service minted for it,
/// which is how [`resource_exists_in_membership_message`] names the resource a create
/// collided with.
fn resource_with_id(kind: &str, id: &str) -> String {
    format!("{kind} with id: {id}")
}

/// The message `CreateAnalysisTemplate` reports when the membership already holds a
/// template under the name the request carries.
///
/// A template's `name` identifies it within its membership, so the conflict is reported
/// as the stored template already existing in that membership: the id is the one the
/// service minted for the template that is in the way, read from the store.
pub fn analysis_template_exists_message(template_id: &str, membership_id: &str) -> String {
    resource_exists_in_membership_message(
        &resource_with_id("analysis template", template_id),
        membership_id,
    )
}

/// The message `CreateConfiguredTableAssociation` reports when the membership already
/// holds an association under the table alias the request carries.
///
/// The alias is "used to query the underlying configured table", so it is the value whose
/// uniqueness the create collides on, and the conflict is reported as the association
/// stored under it already existing in that membership.
pub fn configured_table_association_exists_message(
    association_id: &str,
    membership_id: &str,
) -> String {
    resource_exists_in_membership_message(
        &resource_with_id("configured table association", association_id),
        membership_id,
    )
}

/// The internal, SHOUTY_SNAKE_CASE name of the analysis rule CLASS an analysis-rule type
/// names, which is how the service names a rule TYPE in a "no such analysis rule" refusal
/// (the modelled catalog value is what the request, the stored rule and the service's
/// other messages carry).
///
/// Catalog source: the model shapes `ConfiguredTableAnalysisRuleType` and
/// `ConfiguredTableAssociationAnalysisRuleType` (both AGGREGATION | LIST | CUSTOM). The
/// rule applied to the whole catalog is "the class is named for its catalog value", so a
/// value added to either reads correctly without a second table to maintain. LIST is the
/// one value whose class does not follow its catalog name: a list analysis returns the
/// intersection of the joined tables, and the class is named for that, which is why it is
/// the one branch written out here.
///
/// Only LIST's internal name is observed: the other two are derived by the rule above and
/// are UNVERIFIED against the service, because neither the model nor any recorded refusal
/// in this repository names them. The derivation, not the two derived strings, is what
/// this function claims.
pub fn analysis_rule_type_internal_name(analysis_rule_type: &str) -> &str {
    if analysis_rule_type == ANALYSIS_RULE_TYPE_LIST {
        return INTERNAL_ANALYSIS_RULE_CLASS_LIST;
    }
    analysis_rule_type
}

/// The internal name of the rule class the LIST analysis-rule type names — the one class
/// not named for its catalog value. See [`analysis_rule_type_internal_name`].
const INTERNAL_ANALYSIS_RULE_CLASS_LIST: &str = "INTERSECTION_LIST";

/// The internal rule-class name ([`analysis_rule_type_internal_name`]) as the association
/// plane renders it: the class name as one CamelCase word, the form that plane's refusals
/// report a rule class by.
pub fn configured_table_association_analysis_rule_type_internal_name(
    analysis_rule_type: &str,
) -> String {
    capitalised_words(analysis_rule_type_internal_name(analysis_rule_type), "")
}

/// The most tags one resource can carry.
///
/// `TagMap` is `len 0..200`, and `ListTagsForResourceOutput$tags` IS a `TagMap`, so a
/// stored set past that many entries could not be reported at all; a call that would push
/// a resource past it is refused rather than accepted. Tags are also the one thing in
/// this store that grows per call WITHOUT a new resource, so this is the bound on that
/// growth.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The message a tagging operation reports when its `resourceArn` names no resource
/// this service holds.
///
/// Entirely FIXED text: the tagging plane states that the addressed resource is absent and
/// interpolates nothing — no ARN, no id, no resource kind. The ARN the caller sent is
/// already known to it, and the kind travels in the exception's `resourceType` member.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error code the tagging plane names a resource it cannot find by.
///
/// The CRUD operations of this service answer `ResourceNotFoundException`, the shape its
/// model declares; the three ARN-addressed tagging operations answer the shorter
/// `NotFoundException`, which the model declares nowhere — the tagging plane has its own
/// error vocabulary, and an SDK generates a distinct exception class for it, so a client
/// catching one of the two does not catch the other. Here rather than on an operation,
/// because the code is a property of the refusal and of the plane that makes it rather
/// than of whichever of the three operations was called.
pub const TAGGING_NOT_FOUND_ERROR_CODE: &str = "NotFoundException";

/// A refusal the ARN-addressed tagging plane words itself, carrying the ONE mapping from
/// the refusal to the wire code, the HTTP status and the body it answers with.
///
/// `TagResource`, `UntagResource` and `ListTagsForResource` are one plane, so they cannot
/// word one condition three ways: an ARN naming no resource is
/// [`TAGGING_NOT_FOUND_ERROR_CODE`] carrying the modelled `ResourceNotFoundException`
/// payload and its 404. Each of the three operations' error channels wraps this type and
/// delegates every `smithy_server_runtime::OperationError` method to the methods below, so
/// the mapping is declared once here instead of once per operation.
#[derive(Debug)]
pub enum TaggingRefusal {
    /// "That ARN names no resource of this service" — the modelled shape's payload,
    /// worded with [`TAGGED_RESOURCE_NOT_FOUND_MESSAGE`].
    NotFound(ResourceNotFoundException),
}

impl TaggingRefusal {
    /// The not-found refusal for a `resourceArn` naming no resource this service holds.
    ///
    /// The carried payload is the modelled shape's, so the answer still reports which
    /// resource was addressed (`resource_id`, the ARN) and of which kind
    /// (`resource_type`); only the code on the wire is the tagging plane's own.
    pub fn not_found(resource_arn: &str, resource_type: &str) -> Self {
        Self::NotFound(ResourceNotFoundException {
            message: TAGGED_RESOURCE_NOT_FOUND_MESSAGE.to_string(),
            resource_id: resource_arn.to_string(),
            resource_type: resource_type.to_string(),
        })
    }

    /// The body this refusal serializes: the modelled shape itself, so its own schema
    /// decides which members the answer carries.
    pub fn as_serializable(&self) -> &dyn ::aws_smithy_schema::serde::SerializableStruct {
        match self {
            Self::NotFound(e) => e,
        }
    }

    /// The schema the answer is written against: the modelled shape's own.
    pub fn variant_schema(&self) -> &'static ::aws_smithy_schema::Schema {
        match self {
            Self::NotFound(_) => &crate::ops::types::SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
        }
    }

    /// The code on the wire, which the framework reads from an operation's `shape_name`
    /// rather than from the schema — the one respect in which this refusal differs from
    /// the modelled shape it carries.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => TAGGING_NOT_FOUND_ERROR_CODE,
        }
    }

    /// The HTTP status: the carried shape's own `@httpError`, so the code on the wire
    /// changes and the status does not.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
        }
    }

    /// No `awsQueryError` code: restJson1 is this service's only protocol, and the carried
    /// shape carries no such trait to honour in any case.
    pub fn query_error_code(&self) -> Option<&'static str> {
        None
    }

    /// The sentence the body states, for the `Display` rendering below.
    fn message(&self) -> &str {
        match self {
            Self::NotFound(e) => &e.message,
        }
    }
}

impl ::std::fmt::Display for TaggingRefusal {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{}: {}", self.error_code(), self.message())
    }
}

impl ::std::error::Error for TaggingRefusal {}

/// The error channel of a tagging operation: one of the operation's own modelled errors,
/// or a refusal the tagging plane words itself ([`TaggingRefusal`]).
///
/// `TagResource`, `UntagResource` and `ListTagsForResource` each name this as their
/// `Operation::Error`, so the three answer one condition one way. It replaces
/// `crate::scaffolded::Scaffolded` on them: all three are implemented, so they need
/// neither the 501 (that variant is the stub signal) nor the framework's synthetic
/// `ValidationException` (this service's model declares its own); what they do need, and
/// what the generic wrapper cannot express, is the tagging plane's own wire code.
#[derive(Debug)]
pub enum TaggingErrorChannel<E> {
    /// One of the operation's modelled errors, rendered exactly as the model declares it.
    Modeled(E),
    /// A refusal the tagging plane words itself: the code, the status and the carried body
    /// are [`TaggingRefusal`]'s.
    Refused(TaggingRefusal),
}

impl<E> TaggingErrorChannel<E> {
    /// The tagging plane's not-found refusal for a `resourceArn` naming no resource this
    /// service holds.
    pub fn not_found(resource_arn: &str, resource_type: &str) -> Self {
        Self::Refused(TaggingRefusal::not_found(resource_arn, resource_type))
    }
}

impl<E: ::std::fmt::Display> ::std::fmt::Display for TaggingErrorChannel<E> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Modeled(e) => write!(f, "{e}"),
            Self::Refused(refusal) => write!(f, "{refusal}"),
        }
    }
}

/// `source()` forwards to the wrapped error rather than taking the default `None`, so a
/// modelled error's own `source()` is preserved through the channel.
impl<E: ::std::error::Error + 'static> ::std::error::Error for TaggingErrorChannel<E> {
    fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Modeled(e) => e.source(),
            Self::Refused(refusal) => refusal.source(),
        }
    }
}

/// Every method delegates: a modelled failure to the modelled error, so it is rendered
/// exactly as `#[operation_error]` renders it, and a plane refusal to [`TaggingRefusal`],
/// so each of the three operations answers that condition identically.
impl<E: ::smithy_server_runtime::OperationError> ::smithy_server_runtime::OperationError
    for TaggingErrorChannel<E>
{
    fn as_serializable(&self) -> &dyn ::aws_smithy_schema::serde::SerializableStruct {
        match self {
            Self::Modeled(e) => ::smithy_server_runtime::OperationError::as_serializable(e),
            Self::Refused(refusal) => refusal.as_serializable(),
        }
    }

    fn variant_schema(&self) -> &'static ::aws_smithy_schema::Schema {
        match self {
            Self::Modeled(e) => ::smithy_server_runtime::OperationError::variant_schema(e),
            Self::Refused(refusal) => refusal.variant_schema(),
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::Modeled(e) => ::smithy_server_runtime::OperationError::shape_name(e),
            Self::Refused(refusal) => refusal.error_code(),
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Modeled(e) => ::smithy_server_runtime::OperationError::http_status(e),
            Self::Refused(refusal) => refusal.http_status(),
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        match self {
            Self::Modeled(e) => ::smithy_server_runtime::OperationError::query_error_code(e),
            Self::Refused(refusal) => refusal.query_error_code(),
        }
    }
}

/// The message a tagging operation reports when its `resourceArn` addresses no kind of
/// Clean Rooms resource at all, so there is nothing it could name.
///
/// The boundary validator already refuses an ARN outside `CleanroomsArn`'s pattern; what
/// reaches here is a well-formed Clean Rooms ARN whose resource segment names none of
/// the kinds in [`ARN_RESOURCE_KEYWORDS`], which is a fault in the member's VALUE rather
/// than a resource that is missing.
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    format!("resourceArn does not identify a Clean Rooms resource: {resource_arn}")
}

/// The message `TagResource` reports when the call would leave a resource carrying more
/// than [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// Composed from the limit and the number of tags the resource would have carried: those
/// two are what the decision was made on.
pub fn tag_limit_exceeded_message(limit: usize, would_carry: usize) -> String {
    format!("a resource can carry at most {limit} tags, and this request would leave it carrying {would_carry}")
}

/// The kind of resource `resource_arn`'s resource segment names, as the resource type a
/// refusal about it reports, or `None` when it names none of them.
///
/// An ARN's last colon-separated field is its resource segment, `<keyword>/<id>`, nested
/// under its parent for a sub-resource, so the kind is the DEEPEST keyword of
/// [`ARN_RESOURCE_KEYWORDS`] the segment carries: the parent keyword a sub-resource ARN
/// also carries names the parent, not the resource the ARN addresses.
pub fn resource_type_of_arn(resource_arn: &str) -> Option<&'static str> {
    let resource_segment = resource_arn.splitn(6, ':').nth(5)?;
    resource_segment
        .split('/')
        .filter_map(|segment| {
            ARN_RESOURCE_KEYWORDS
                .iter()
                .find(|(keyword, _)| *keyword == segment)
                .map(|(_, resource_type)| *resource_type)
        })
        .next_back()
}

/// The `isMetricsEnabled` a created resource carries: what the caller asked for, else
/// the documented [`DEFAULT_IS_METRICS_ENABLED`].
pub fn effective_is_metrics_enabled(requested: Option<bool>) -> bool {
    requested.unwrap_or(DEFAULT_IS_METRICS_ENABLED)
}

/// The `analyticsEngine` a created collaboration carries: what the caller selected, else
/// the [`DEFAULT_ANALYTICS_ENGINE`].
pub fn effective_analytics_engine(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_ANALYTICS_ENGINE.to_string())
}

/// Now, as the modelled `Timestamp` type.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// A fresh resource id in the `UUID` shape's form: lowercase hex, 36 characters,
/// matching `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
pub fn new_uuid() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// `arn:aws:cleanrooms:<region>:<account>:collaboration/<id>`, the form the model's
/// `CollaborationArn` pattern declares.
pub fn collaboration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("collaboration/{id}"))
}

/// `arn:aws:cleanrooms:<region>:<account>:membership/<id>`, the form the model's
/// `MembershipArn` pattern declares.
pub fn membership_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("membership/{id}"))
}

/// `arn:aws:cleanrooms:<region>:<account>:membership/<membership id>/analysistemplate/<id>`,
/// the form the model's `AnalysisTemplateArn` pattern declares — an analysis template
/// is a sub-resource of the membership it was created under, so its ARN is nested in
/// that membership's.
pub fn analysis_template_arn(region: &str, membership_id: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("membership/{membership_id}/analysistemplate/{id}"),
    )
}

/// `arn:aws:cleanrooms:<region>:<account>:privacybudgettemplate/<id>`, the form the
/// model's `PrivacyBudgetTemplateArn` pattern declares.
pub fn privacy_budget_template_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("privacybudgettemplate/{id}"),
    )
}

/// The `privacyBudgetType` the parameters union itself declares: each member of
/// `PrivacyBudgetTemplateParametersInput` carries the parameters of exactly one
/// `PrivacyBudgetType`, so a request whose `privacyBudgetType` names the other one
/// cannot be stored coherently.
pub fn privacy_budget_type_of_parameters(
    parameters: &PrivacyBudgetTemplateParametersInput,
) -> &'static str {
    match parameters {
        PrivacyBudgetTemplateParametersInput::differentialPrivacy(_) => {
            PRIVACY_BUDGET_TYPE_DIFFERENTIAL_PRIVACY
        }
        PrivacyBudgetTemplateParametersInput::accessBudget(_) => PRIVACY_BUDGET_TYPE_ACCESS_BUDGET,
    }
}

/// The stored `PrivacyBudgetTemplateParametersOutput` for the parameters a create
/// carried. The input and output unions declare the same members with the same
/// members inside them, so this is a projection, not a reinterpretation.
pub fn privacy_budget_template_parameters_output(
    parameters: &PrivacyBudgetTemplateParametersInput,
) -> PrivacyBudgetTemplateParametersOutput {
    match parameters {
        PrivacyBudgetTemplateParametersInput::differentialPrivacy(input) => {
            PrivacyBudgetTemplateParametersOutput::differentialPrivacy(
                DifferentialPrivacyTemplateParametersOutput {
                    epsilon: input.epsilon,
                    users_noise_per_query: input.users_noise_per_query,
                },
            )
        }
        PrivacyBudgetTemplateParametersInput::accessBudget(input) => {
            PrivacyBudgetTemplateParametersOutput::accessBudget(
                AccessBudgetsPrivacyTemplateParametersOutput {
                    budget_parameters: input.budget_parameters.clone(),
                    resource_arn: input.resource_arn.clone(),
                },
            )
        }
    }
}

/// The refresh cadence a created privacy budget template carries: the one the caller
/// asked for, else [`DEFAULT_PRIVACY_BUDGET_TEMPLATE_AUTO_REFRESH`].
pub fn effective_template_auto_refresh(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_PRIVACY_BUDGET_TEMPLATE_AUTO_REFRESH.to_string())
}

/// `arn:aws:cleanrooms:<region>:<account>:configuredtable/<id>`, the form the model's
/// `ConfiguredTableArn` pattern declares
/// (`^arn:aws:[\w]+:[\w]{2}-[\w]{4,9}-[\d]:[\d]{12}:configuredtable/[\d\w-]+$`).
pub fn configured_table_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("configuredtable/{id}"))
}

/// `arn:aws:cleanrooms:<region>:<account>:configuredtableassociation/<membership id>/<id>`,
/// the form the model's `ConfiguredTableAssociationArn` pattern declares — an association
/// is a sub-resource of the membership it was created under, so its ARN is nested in that
/// membership's id.
pub fn configured_table_association_arn(region: &str, membership_id: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("configuredtableassociation/{membership_id}/{id}"),
    )
}

/// The `tableType` a configured table reports: the `TableReferenceType` of the
/// `TableReference` union variant the table actually points at, so the member is
/// computed from the stored reference rather than taken from the request.
pub fn table_reference_type(reference: &TableReference) -> &'static str {
    match reference {
        TableReference::Glue(_) => TABLE_TYPE_GLUE,
        TableReference::Snowflake(_) => TABLE_TYPE_SNOWFLAKE,
        TableReference::Athena(_) => TABLE_TYPE_ATHENA,
        TableReference::Databricks(_) => TABLE_TYPE_DATABRICKS,
    }
}

/// What the service says when the `(configured table, analysis rule type)` pair a
/// request addresses holds no analysis rule.
///
/// The variable parts are the identifier the REQUEST carried, echoed as sent, and the rule
/// type rendered by the INTERNAL name of its rule class
/// ([`analysis_rule_type_internal_name`]) rather than by the catalog value the path
/// carried: the pair is the rule's identity ("Configured table analysis rules are uniquely
/// identified by their configured table identifier and analysis rule type"), so the
/// refusal names the pair that resolved to nothing rather than describing the table.
/// Every analysis-rule operation that addresses a rule through that pair refuses a miss
/// with this one sentence.
pub fn no_such_configured_table_analysis_rule_message(
    configured_table_identifier: &str,
    analysis_rule_type: &str,
) -> String {
    format!(
        "No such analysis rule with configuredTableId: {configured_table_identifier} and type {}",
        analysis_rule_type_internal_name(analysis_rule_type)
    )
}

/// The `ConfiguredTableAnalysisRuleType` a `ConfiguredTableAnalysisRulePolicy` carries:
/// each member of the `ConfiguredTableAnalysisRulePolicyV1` union is the policy of
/// exactly one rule type, so the pair is what makes a `analysisRuleType` naming another
/// type detectable.
pub fn configured_table_analysis_rule_policy_type(
    policy: &ConfiguredTableAnalysisRulePolicy,
) -> &'static str {
    match policy {
        ConfiguredTableAnalysisRulePolicy::V1(v1) => match v1 {
            ConfiguredTableAnalysisRulePolicyV1::Aggregation(_) => ANALYSIS_RULE_TYPE_AGGREGATION,
            ConfiguredTableAnalysisRulePolicyV1::List(_) => ANALYSIS_RULE_TYPE_LIST,
            ConfiguredTableAnalysisRulePolicyV1::Custom(_) => ANALYSIS_RULE_TYPE_CUSTOM,
        },
    }
}

/// The `ConfiguredTableAssociationAnalysisRuleType` a
/// `ConfiguredTableAssociationAnalysisRulePolicy` carries: each member of the
/// `ConfiguredTableAssociationAnalysisRulePolicyV1` union is the policy of exactly one
/// rule type, so the pair is what makes an `analysisRuleType` naming another type
/// detectable.
pub fn configured_table_association_analysis_rule_policy_type(
    policy: &ConfiguredTableAssociationAnalysisRulePolicy,
) -> &'static str {
    match policy {
        ConfiguredTableAssociationAnalysisRulePolicy::V1(v1) => match v1 {
            ConfiguredTableAssociationAnalysisRulePolicyV1::Aggregation(_) => {
                ANALYSIS_RULE_TYPE_AGGREGATION
            }
            ConfiguredTableAssociationAnalysisRulePolicyV1::List(_) => ANALYSIS_RULE_TYPE_LIST,
            ConfiguredTableAssociationAnalysisRulePolicyV1::Custom(_) => ANALYSIS_RULE_TYPE_CUSTOM,
        },
    }
}

/// What the service says when the configured table association a request addresses is not
/// one the membership in its path holds.
///
/// The two variable parts are the identifiers the REQUEST carried: an association is a
/// sub-resource of one membership, so the refusal names the pair that resolved to
/// nothing.
pub fn configured_table_association_not_found_message(
    association_id: &str,
    membership_id: &str,
) -> String {
    format!("configured table association {association_id} not found in membership {membership_id}")
}

/// What the service says when the `(configured table association, analysis rule type)`
/// pair a request addresses holds no analysis rule.
///
/// The variable parts are the identifier the REQUEST carried, echoed as sent, and the rule
/// type rendered by the INTERNAL name of its rule class
/// ([`configured_table_association_analysis_rule_type_internal_name`]) rather than by the
/// catalog value from the path: the pair is the rule's identity, so the refusal names the
/// pair that resolved to nothing rather than describing the association. Worded like the
/// configured-table refusal ([`no_such_configured_table_analysis_rule_message`]), which
/// names the same rule class by the same internal name, because the two name the same
/// kind of miss.
pub fn no_such_configured_table_association_analysis_rule_message(
    configured_table_association_identifier: &str,
    analysis_rule_type: &str,
) -> String {
    format!(
        "No such analysis rule with configuredTableAssociationId: {configured_table_association_identifier} and type {}",
        configured_table_association_analysis_rule_type_internal_name(analysis_rule_type)
    )
}

/// The columns a configured table's analysis rule policy references that the table's
/// `allowedColumns` do NOT contain, each named once, in the order the policy names them.
///
/// `allowedColumns` is documented as "the columns of the underlying table that can be
/// used by collaborations or analysis rules", so every column an analysis rule names has
/// to be one of them: this is what the analysis-rule and configured-table operations
/// refuse a policy, or a narrowed column list, by. The answer is empty for a consistent
/// pair, so an empty list is what "nothing to refuse" looks like.
pub fn analysis_rule_columns_outside_allowed(
    policy: &ConfiguredTableAnalysisRulePolicy,
    allowed_columns: &[String],
) -> Vec<String> {
    let allowed: HashSet<&str> = allowed_columns.iter().map(String::as_str).collect();
    let mut outside = Vec::new();
    let mut seen = HashSet::new();
    for column in analysis_rule_referenced_columns(policy) {
        if !allowed.contains(column.as_str()) && seen.insert(column.clone()) {
            outside.push(column);
        }
    }
    outside
}

/// What the service says when a configured table's analysis rule would reference columns
/// the table's `allowedColumns` do not carry — the refusal every operation that can
/// reach that state shares, whether it is the POLICY that names a column the table does
/// not expose or the COLUMN LIST that stops exposing one the stored policy names.
///
/// The sentence states the unsatisfied constraint and the remedy, and its single variable
/// part is the list of referenced-but-not-allowed column names
/// ([`analysis_rule_columns_outside_allowed`]), rendered bracketed and comma separated.
/// Nothing else is interpolated: neither the configured table addressed nor the analysis
/// rule type is part of the diagnosis, because the constraint is between a rule's columns
/// and a column list, not about one resource.
pub fn analysis_rule_columns_outside_allowed_message(columns: &[String]) -> String {
    format!(
        "The following columns in the analysis rule aren't present in the allowed columns \
         list: [{}]. To be used in the analysis rule, columns must be specified as allowed \
         columns.",
        columns.join(", ")
    )
}

/// Every column name a configured table's analysis rule policy references, member by
/// member of the policy union the rule carries.
///
/// One arm per member of `ConfiguredTableAnalysisRulePolicyV1`, and within each arm every
/// member whose target is a column name: `AnalysisRuleList`'s `joinColumns` and
/// `listColumns`; `AnalysisRuleAggregation`'s `aggregateColumns[].columnNames`,
/// `joinColumns`, `dimensionColumns` and `outputConstraints[].columnName`;
/// `AnalysisRuleCustom`'s `disallowedOutputColumns` and
/// `differentialPrivacy.columns[].name`. A member that names an analysis, a function, a
/// provider account or a threshold names no column and is not collected.
fn analysis_rule_referenced_columns(policy: &ConfiguredTableAnalysisRulePolicy) -> Vec<String> {
    let ConfiguredTableAnalysisRulePolicy::V1(v1) = policy;
    match v1 {
        ConfiguredTableAnalysisRulePolicyV1::List(list) => list
            .join_columns
            .iter()
            .chain(list.list_columns.iter())
            .cloned()
            .collect(),
        ConfiguredTableAnalysisRulePolicyV1::Aggregation(aggregation) => aggregation
            .aggregate_columns
            .iter()
            .flat_map(|aggregate| aggregate.column_names.iter())
            .chain(aggregation.join_columns.iter())
            .chain(aggregation.dimension_columns.iter())
            .cloned()
            .chain(
                aggregation
                    .output_constraints
                    .iter()
                    .map(|constraint| constraint.column_name.clone()),
            )
            .collect(),
        ConfiguredTableAnalysisRulePolicyV1::Custom(custom) => custom
            .disallowed_output_columns
            .iter()
            .flatten()
            .cloned()
            .chain(
                custom
                    .differential_privacy
                    .iter()
                    .flat_map(|privacy| privacy.columns.iter())
                    .map(|column| column.name.clone()),
            )
            .collect(),
    }
}

/// How many DISTINCT analysis methods a `selectedAnalysisMethods` list configures.
///
/// The list is a set of enum values, so a method repeated in the request still
/// configures one method — see [`MIN_SELECTED_ANALYSIS_METHODS`].
///
/// The model puts no length constraint on `SelectedAnalysisMethods`, so the list one
/// request may carry is bounded only by the transport body size: the count is taken
/// through a hash set, never by scanning what has been seen already, so the pass stays
/// LINEAR in the list's length however many repeats it carries (a scan would make the
/// per-request work quadratic in a list the model does not bound above). The distinct
/// count it answers is bounded by the model's `AnalysisMethod` catalog.
pub fn distinct_selected_analysis_methods(methods: &[String]) -> usize {
    methods
        .iter()
        .map(String::as_str)
        .collect::<HashSet<&str>>()
        .len()
}

/// Whether `region` can appear in the ARN region segment the model's
/// `CollaborationArn`/`MembershipArn` patterns declare: `[\w]{2}-[\w]{4,9}-[\d]`.
fn region_fits_arn_pattern(region: &str) -> bool {
    let mut parts = region.split('-');
    let (Some(a), Some(b), Some(c), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let word = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    a.len() == 2
        && word(a)
        && (4..=9).contains(&b.len())
        && word(b)
        && c.len() == 1
        && c.chars().all(|c| c.is_ascii_digit())
}

/// The region this request is signed for, used for the ARNs the response carries.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| region_fits_arn_pattern(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Map a collaboration-side `PaymentConfiguration` (what the creator SET for a member)
/// onto the membership-side `MembershipPaymentConfiguration` (what that member
/// ACCEPTED) — the two shapes carry the same `isResponsible` flags.
pub fn accept_payment_configuration(
    config: &PaymentConfiguration,
) -> MembershipPaymentConfiguration {
    let machine_learning = config.machine_learning.as_ref().map(|ml| {
        MembershipMLPaymentConfig {
            model_training: ml.model_training.as_ref().map(|c| {
                MembershipModelTrainingPaymentConfig {
                    is_responsible: c.is_responsible,
                }
            }),
            model_inference: ml.model_inference.as_ref().map(|c| {
                MembershipModelInferencePaymentConfig {
                    is_responsible: c.is_responsible,
                }
            }),
            synthetic_data_generation: ml.synthetic_data_generation.as_ref().map(|c| {
                MembershipSyntheticDataGenerationPaymentConfig {
                    is_responsible: c.is_responsible,
                }
            }),
        }
    });
    let job_compute = config.job_compute.as_ref().map(|c| {
        MembershipJobComputePaymentConfig {
            is_responsible: c.is_responsible,
        }
    });
    MembershipPaymentConfiguration {
        query_compute: MembershipQueryComputePaymentConfig {
            is_responsible: config.query_compute.is_responsible,
        },
        machine_learning,
        job_compute,
    }
}

/// The payment responsibilities a new membership carries.
///
/// Precedence: what the joining member accepted in the request, else what the
/// collaboration creator set for that member, else the documented default —
/// `QueryComputePaymentConfig.isResponsible`: "If the collaboration creator hasn't
/// specified anyone as the member paying for query compute costs, then the member who
/// can query is the default payer." The same rule is applied to job compute, whose
/// paying ability is `CAN_RUN_JOB`.
///
/// Every OPTIONAL sub-config of the shape is then materialized, whichever of those three
/// sources supplied the rest: a membership reports the responsibility it accepted for
/// each cost the model names, so a cost nobody was assigned is reported as
/// not-responsible instead of being left absent (see [`DEFAULT_ML_IS_RESPONSIBLE`]).
pub fn membership_payment_configuration(
    requested: Option<&MembershipPaymentConfiguration>,
    grant: &MemberGrant,
) -> MembershipPaymentConfiguration {
    let has = |ability: &str| grant.member_abilities.iter().any(|a| a == ability);
    let accepted = requested.cloned().or_else(|| {
        grant
            .payment_configuration
            .as_ref()
            .map(accept_payment_configuration)
    });
    let (query_compute, machine_learning, job_compute) = match accepted {
        Some(accepted) => (
            accepted.query_compute,
            accepted.machine_learning,
            accepted.job_compute,
        ),
        None => (
            MembershipQueryComputePaymentConfig {
                is_responsible: has(ABILITY_CAN_QUERY),
            },
            None,
            None,
        ),
    };
    MembershipPaymentConfiguration {
        query_compute,
        machine_learning: Some(materialize_ml_payment_config(machine_learning)),
        job_compute: Some(job_compute.unwrap_or(MembershipJobComputePaymentConfig {
            is_responsible: has(ABILITY_CAN_RUN_JOB),
        })),
    }
}

/// The machine-learning block a membership reports, with one accepted sub-config per ML
/// cost `MembershipMLPaymentConfig` names (`modelTraining`, `modelInference`,
/// `syntheticDataGeneration`): whatever was accepted for that cost, else
/// [`DEFAULT_ML_IS_RESPONSIBLE`].
fn materialize_ml_payment_config(
    accepted: Option<MembershipMLPaymentConfig>,
) -> MembershipMLPaymentConfig {
    let accepted = accepted.unwrap_or_default();
    MembershipMLPaymentConfig {
        model_training: Some(accepted.model_training.unwrap_or(
            MembershipModelTrainingPaymentConfig {
                is_responsible: DEFAULT_ML_IS_RESPONSIBLE,
            },
        )),
        model_inference: Some(accepted.model_inference.unwrap_or(
            MembershipModelInferencePaymentConfig {
                is_responsible: DEFAULT_ML_IS_RESPONSIBLE,
            },
        )),
        synthetic_data_generation: Some(accepted.synthetic_data_generation.unwrap_or(
            MembershipSyntheticDataGenerationPaymentConfig {
                is_responsible: DEFAULT_ML_IS_RESPONSIBLE,
            },
        )),
    }
}

/// The account that pays for a collaboration's query compute costs.
///
/// `QueryComputePaymentConfig$isResponsible` documents the rule: the creator configures
/// payer candidates ("One or more members can be configured as payer candidates for
/// query compute costs"), and "if the collaboration creator hasn't specified anyone as
/// the member paying for query compute costs, then the member who can query is the
/// default payer". So the payer is the first account the creator configured as
/// responsible for query compute, else the first account granted
/// [`ABILITY_CAN_QUERY`], in declaration order (the creator, then the `members` list)
/// — and `None` when the collaboration configured neither, which leaves the optional
/// member absent rather than guessed.
pub fn query_compute_payer(record: &CollaborationRecord) -> Option<String> {
    let accounts = record.party_accounts();
    let configured = accounts.iter().find(|account| {
        record.grant_for(account).is_some_and(|grant| {
            grant
                .payment_configuration
                .is_some_and(|payment| payment.query_compute.is_responsible)
        })
    });
    if let Some(account) = configured {
        return Some(account.clone());
    }
    accounts.into_iter().find(|account| {
        record.grant_for(account).is_some_and(|grant| {
            grant
                .member_abilities
                .iter()
                .any(|ability| ability == ABILITY_CAN_QUERY)
        })
    })
}

/// The `PaymentConfiguration` a member listing reports for one party to `record`.
///
/// `MemberSummary$paymentConfiguration` is documented as "the collaboration member's
/// payment responsibilities SET BY THE COLLABORATION CREATOR", so a party the creator
/// configured reports that configuration as stored, untouched.
///
/// The member is required, so a party the creator configured for nothing reports the
/// documented default instead: `QueryComputePaymentConfig$isResponsible` makes "the
/// member who can query" the default payer for query compute, which is the one account
/// [`query_compute_payer`] resolves for the collaboration — the other parties are not
/// responsible for it. The costs the shape leaves OPTIONAL stay absent, because nobody
/// was made responsible for them.
pub fn member_payment_configuration(
    record: &CollaborationRecord,
    account_id: &str,
) -> PaymentConfiguration {
    if let Some(configured) = record
        .grant_for(account_id)
        .and_then(|grant| grant.payment_configuration)
    {
        return configured;
    }
    PaymentConfiguration {
        query_compute: QueryComputePaymentConfig {
            is_responsible: query_compute_payer(record).as_deref() == Some(account_id),
        },
        machine_learning: None,
        job_compute: None,
    }
}

/// The `WorkerComputeConfiguration$type` a worker compute configuration runs at when
/// the caller names none: the model declares `default: "CR.1X"` on that member (an
/// `addedDefault`, so the client may omit it), out of the `WorkerComputeType` catalog
/// CR.1X | CR.4X | CR.8X.
pub const DEFAULT_WORKER_COMPUTE_TYPE: &str = "CR.1X";

/// The `WorkerComputeConfiguration$number` of workers a configuration runs with when
/// the caller names none: the model declares `default: 16` on that member, inside its
/// `range 2..1024`.
pub const DEFAULT_WORKER_COMPUTE_NUMBER: i32 = 16;

/// The compute configuration a protected query runs with.
///
/// A caller that named no compute configuration selected none, and the member stays
/// absent. A caller that named a `worker` configuration gets the one in force: each of
/// its optional members is materialized at the default the model declares for it
/// ([`DEFAULT_WORKER_COMPUTE_TYPE`], [`DEFAULT_WORKER_COMPUTE_NUMBER`]), so a later
/// read reports the compute the query runs on rather than an absent member.
pub fn effective_compute_configuration(
    requested: Option<ComputeConfiguration>,
) -> Option<ComputeConfiguration> {
    let ComputeConfiguration::Worker(worker) = requested?;
    Some(ComputeConfiguration::Worker(WorkerComputeConfiguration {
        r#type: Some(
            worker
                .r#type
                .unwrap_or_else(|| DEFAULT_WORKER_COMPUTE_TYPE.to_string()),
        ),
        number: Some(worker.number.unwrap_or(DEFAULT_WORKER_COMPUTE_NUMBER)),
        properties: worker.properties,
    }))
}

/// Where a protected query's results are written: the configuration the request
/// carried, else the membership's stored `defaultResultConfiguration` — documented on
/// `CreateMembershipInput$defaultResultConfiguration` as "the default protected query
/// result configuration" — projected onto the query-side shape. `None` when the request
/// named none and the membership holds no default.
pub fn protected_query_result_configuration(
    requested: Option<ProtectedQueryResultConfiguration>,
    membership: &Membership,
) -> Option<ProtectedQueryResultConfiguration> {
    requested.or_else(|| {
        let default = membership.default_result_configuration.as_ref()?;
        // `MembershipProtectedQueryOutputConfiguration` declares one member, `s3`, and
        // it is the same shape the query-side union's `s3` member targets.
        let output_configuration = match &default.output_configuration {
            MembershipProtectedQueryOutputConfiguration::S3(s3) => {
                ProtectedQueryOutputConfiguration::S3(s3.clone())
            }
        };
        Some(ProtectedQueryResultConfiguration {
            output_configuration,
        })
    })
}

/// Everything a new `Membership` is built from: the ids minted for it, the stored
/// collaboration it joins, the grant the joining account holds there, and the
/// membership-only settings the request carried.
pub struct NewMembership<'a> {
    pub id: String,
    pub arn: String,
    pub record: &'a CollaborationRecord,
    pub grant: MemberGrant,
    pub query_log_status: String,
    pub job_log_status: Option<String>,
    pub default_result_configuration: Option<MembershipProtectedQueryResultConfiguration>,
    pub default_job_result_configuration: Option<MembershipProtectedJobResultConfiguration>,
    pub payment_configuration: MembershipPaymentConfiguration,
    /// The metrics setting in force — the one the caller sent, else
    /// [`DEFAULT_IS_METRICS_ENABLED`]. Not optional: the service always has one.
    pub is_metrics_enabled: bool,
    pub created_at: DateTime,
}

impl NewMembership<'_> {
    /// The stored `Membership`. Every collaboration-derived member is copied FROM the
    /// stored collaboration, never from the request.
    pub fn build(self) -> Membership {
        Membership {
            id: self.id,
            arn: self.arn,
            collaboration_arn: self.record.arn.clone(),
            collaboration_id: self.record.id.clone(),
            collaboration_creator_account_id: self.record.creator_account_id.clone(),
            collaboration_creator_display_name: self.record.creator_display_name.clone(),
            collaboration_name: self.record.name.clone(),
            create_time: self.created_at,
            update_time: self.created_at,
            status: MEMBERSHIP_STATUS_ACTIVE.to_string(),
            member_abilities: self.grant.member_abilities,
            ml_member_abilities: self.grant.ml_member_abilities,
            query_log_status: self.query_log_status,
            job_log_status: self.job_log_status,
            default_result_configuration: self.default_result_configuration,
            default_job_result_configuration: self.default_job_result_configuration,
            payment_configuration: self.payment_configuration,
            is_metrics_enabled: Some(self.is_metrics_enabled),
        }
    }
}

/// Everything a new `AnalysisTemplate` is built from: the ids minted for it, the stored
/// membership it is created under (which is where its collaboration identity comes
/// from), and the analysis the request carried.
pub struct NewAnalysisTemplate<'a> {
    pub id: String,
    pub arn: String,
    pub membership: &'a Membership,
    pub name: String,
    pub description: Option<String>,
    pub format: String,
    pub source: AnalysisSource,
    /// The relation set in force, from [`analysis_template_schema`].
    pub schema: AnalysisSchema,
    pub analysis_parameters: Option<Vec<AnalysisParameter>>,
    pub error_message_configuration: Option<ErrorMessageConfiguration>,
    pub synthetic_data_parameters: Option<SyntheticDataParameters>,
    pub created_at: DateTime,
}

impl NewAnalysisTemplate<'_> {
    /// The stored `AnalysisTemplate`. Every collaboration/membership-derived member is
    /// copied FROM the stored membership, never from the request.
    pub fn build(self) -> AnalysisTemplate {
        AnalysisTemplate {
            id: self.id,
            arn: self.arn,
            collaboration_id: self.membership.collaboration_id.clone(),
            collaboration_arn: self.membership.collaboration_arn.clone(),
            membership_id: self.membership.id.clone(),
            membership_arn: self.membership.arn.clone(),
            description: self.description,
            name: self.name,
            create_time: self.created_at,
            update_time: self.created_at,
            schema: self.schema,
            format: self.format,
            source: self.source,
            // `sourceMetadata` reports hashes of the S3 artifacts an artifact-sourced
            // analysis points at (`AnalysisTemplateArtifactMetadata$entryPointHash`).
            // Nothing in this emulator reads those objects, so the member is left
            // absent rather than filled with a hash of something that was never
            // hashed.
            source_metadata: None,
            analysis_parameters: self.analysis_parameters,
            // `validations` carries the DIFFERENTIAL_PRIVACY validation detail (model
            // shape `AnalysisTemplateValidationType`), which only a differential-privacy
            // analysis acquires; no validation has run here, so no status is claimed.
            validations: None,
            error_message_configuration: self.error_message_configuration,
            synthetic_data_parameters: self.synthetic_data_parameters,
        }
    }
}

/// The `schema` a created analysis template reports — a REQUIRED member of
/// `AnalysisTemplate`, so the service always has one.
///
/// Precedence: the schema the caller declared, else the relations the analysis itself
/// reads. Only a `SQL`-format analysis carries a query the service can read those out
/// of; for any other analysis (PySpark program text, or artifacts in S3 this emulator
/// cannot open) no relation set is derived and `referencedTables` is left absent rather
/// than asserted empty.
///
/// `Err` says why no relation set could be read out of the analysis.
pub fn analysis_template_schema(
    requested: Option<AnalysisSchema>,
    format: &str,
    source: &AnalysisSource,
) -> Result<AnalysisSchema, SchemaError> {
    if let Some(schema) = requested {
        return Ok(schema);
    }
    let referenced_tables = match source {
        AnalysisSource::Text(text) if format == ANALYSIS_FORMAT_SQL => {
            Some(sql_referenced_tables(text)?)
        }
        _ => None,
    };
    Ok(AnalysisSchema { referenced_tables })
}

/// Whether reading this analysis's relation set means parsing a query at all.
///
/// Only a `SQL`-format analysis given as TEXT carries one, and only when the caller
/// declared no schema of its own; every other case is answered without the parse
/// machinery (no thread, no parse slot), because [`analysis_template_schema`] returns
/// immediately for it.
pub fn analysis_needs_parse(
    requested: Option<&AnalysisSchema>,
    format: &str,
    source: &AnalysisSource,
) -> bool {
    requested.is_none()
        && format == ANALYSIS_FORMAT_SQL
        && matches!(source, AnalysisSource::Text(_))
}

/// Why an analysis has no relation set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// The SQL analysis is more text than [`MAX_PARSED_ANALYSIS_CHARS`], so the service
    /// does not read a relation set out of it; carries that cap.
    TooMuchText(usize),
    /// The SQL analysis carries more tokens than [`MAX_PARSED_ANALYSIS_TOKENS`], so the
    /// tree a parse would build is not one this service walks; carries that cap.
    TooComplex(usize),
    /// The SQL analysis nests deeper than the parser's [`MAX_PARSER_RECURSION`]; carries
    /// that cap.
    TooDeep(usize),
    /// The SQL analysis is not a query; carries the parser's complaint.
    NotAQuery(String),
    /// The query names more relations than [`MAX_REFERENCED_TABLES`].
    TooManyRelations,
    /// The query reads a relation whose name cannot appear in the model's `QueryTables`
    /// member type (`TableAlias`), so the relation set could not be reported in full;
    /// carries that name.
    UnreportableRelation(String),
}

/// The relations a SQL analysis reads, in the order the query first names them.
///
/// Parsed with the `sqlparser` crate rather than scanned for keywords, so a subquery,
/// a join or a qualified name is found where it really is. A name the query defines
/// itself (a common table expression) is not a referenced table, and a name that cannot
/// appear in the model's `QueryTables` member type (`TableAlias`) is left out so the
/// response stays within the modelled constraints.
///
/// SYNCHRONOUS CPU WORK over caller-supplied text: callers run it on the blocking pool,
/// under [`ANALYSIS_PARSE_TIME_BUDGET`] and never with the store locked. The work is
/// bounded before it is scheduled, in size, in depth and in volume, each bound measured
/// by a FLAT pass that recurses over nothing: at most [`MAX_PARSED_ANALYSIS_CHARS`] of
/// text is lexed, at most [`MAX_PARSED_ANALYSIS_TOKENS`] tokens are parsed — which caps
/// the tree's node count and so its depth, for the recursive parse, this module's
/// recursive walk and the tree's own recursive drop alike — the parser additionally
/// carries [`MAX_PARSER_RECURSION`], each relation is looked up in a set rather than
/// scanned for in a list, and the number of relations collected stops at
/// [`MAX_REFERENCED_TABLES`].
fn sql_referenced_tables(sql: &str) -> Result<Vec<String>, SchemaError> {
    // The INPUT is bounded before the parser runs, so the work one request can ask for
    // is bounded BEFORE it is scheduled rather than only waited on: the parse cost is a
    // function of this text, and a caller cannot make it longer than the cap.
    if sql.chars().count() > MAX_PARSED_ANALYSIS_CHARS {
        return Err(SchemaError::TooMuchText(MAX_PARSED_ANALYSIS_CHARS));
    }
    // The TREE is bounded the same way, from outside: lexing is a flat pass, every node
    // of the tree a parse would build consumes at least one token, so a token count is
    // an upper bound on the tree's size and therefore on its depth. Refusing here keeps
    // the recursive parse, the recursive walk and the tree's recursive drop off a tree
    // deeper than the cap — including the shapes no bracket count would catch, such as
    // a left-associative `1 + 1 + …` chain, which the parser builds iteratively into a
    // tree one node deep per operator.
    if analysis_token_count(sql)? > MAX_PARSED_ANALYSIS_TOKENS {
        return Err(SchemaError::TooComplex(MAX_PARSED_ANALYSIS_TOKENS));
    }
    let statements = ::sqlparser::parser::Parser::new(&::sqlparser::dialect::GenericDialect {})
        .with_recursion_limit(MAX_PARSER_RECURSION)
        .try_with_sql(sql)
        .and_then(|mut parser| parser.parse_statements())
        .map_err(|error| match error {
            ::sqlparser::parser::ParserError::RecursionLimitExceeded => {
                SchemaError::TooDeep(MAX_PARSER_RECURSION)
            }
            other => SchemaError::NotAQuery(other.to_string()),
        })?;
    let mut collector = RelationCollector::default();
    // The only thing that breaks the walk is the relation cap, and a truncated relation
    // set would misreport what the analysis reads.
    if ::sqlparser::ast::Visit::visit(&statements, &mut collector).is_break() {
        return Err(SchemaError::TooManyRelations);
    }
    collector.finish()
}

/// How many tokens, not counting whitespace and comments, the analysis carries.
///
/// Lexing is a FLAT pass — the tokenizer recurses over nothing — so this can be measured
/// before anything recursive runs, which is what makes it usable as the bound in front
/// of the parse. Whitespace and comments carry no node, so they are not counted. Text
/// the tokenizer cannot lex at all (an unterminated string, say) is not a query, and is
/// reported as such.
fn analysis_token_count(sql: &str) -> Result<usize, SchemaError> {
    use ::sqlparser::tokenizer::{Token, Tokenizer};

    let tokens = Tokenizer::new(&::sqlparser::dialect::GenericDialect {}, sql)
        .tokenize()
        .map_err(|error| SchemaError::NotAQuery(error.to_string()))?;
    Ok(tokens
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .count())
}

/// The walk stopped because the query names more relations than
/// [`MAX_REFERENCED_TABLES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TooManyRelations;

/// Collects the relation names a parsed SQL statement reads, and the names it defines
/// for itself.
///
/// `relations` keeps first-appearance order; `seen` is what makes the de-duplication a
/// set lookup rather than a scan of `relations`, so the walk stays linear in the number
/// of relations the caller's query names.
#[derive(Debug, Default)]
struct RelationCollector {
    relations: Vec<String>,
    seen: HashSet<String>,
    defined_names: HashSet<String>,
}

impl RelationCollector {
    /// The referenced tables: every relation the query reads that it did not define
    /// itself.
    ///
    /// A name that cannot appear in the model's `QueryTables` member type (`TableAlias`)
    /// is an ERROR rather than an omission: dropping it would report a relation set that
    /// misstates what the analysis reads, with a 200.
    fn finish(self) -> Result<Vec<String>, SchemaError> {
        let defined = self.defined_names;
        let mut referenced = Vec::with_capacity(self.relations.len());
        for name in self.relations {
            if defined.contains(&name) {
                continue;
            }
            if !fits_table_alias(&name) {
                return Err(SchemaError::UnreportableRelation(name));
            }
            referenced.push(name);
        }
        Ok(referenced)
    }
}

impl ::sqlparser::ast::Visitor for RelationCollector {
    type Break = TooManyRelations;

    fn pre_visit_query(&mut self, query: &::sqlparser::ast::Query) -> ControlFlow<Self::Break> {
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.defined_names.insert(cte.alias.name.value.clone());
            }
            if self.defined_names.len() > MAX_REFERENCED_TABLES {
                return ControlFlow::Break(TooManyRelations);
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(
        &mut self,
        relation: &::sqlparser::ast::ObjectName,
    ) -> ControlFlow<Self::Break> {
        // The last segment of a possibly-qualified name (`db.schema.table`) is the
        // relation itself; quoting is syntax, not part of the name.
        let rendered = relation.to_string();
        let name = rendered
            .rsplit('.')
            .next()
            .unwrap_or(rendered.as_str())
            .trim_matches(|c| matches!(c, '"' | '`' | '[' | ']'))
            .to_string();
        if name.is_empty() {
            return ControlFlow::Continue(());
        }
        if self.seen.insert(name.clone()) {
            if self.relations.len() >= MAX_REFERENCED_TABLES {
                return ControlFlow::Break(TooManyRelations);
            }
            self.relations.push(name);
        }
        ControlFlow::Continue(())
    }
}

/// Whether `name` can appear in a `QueryTables` list, whose member is the model's
/// `TableAlias`: `len ..128`, pattern
/// `^[a-zA-Z0-9_](([a-zA-Z0-9_ ]+-)*([a-zA-Z0-9_ ]+))?$` — it starts with a letter,
/// digit or underscore, carries only those characters plus space and `-`, and does not
/// end in `-`.
fn fits_table_alias(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    // `len 0..128` is a CHARACTER bound, and the pattern's first group is narrower than
    // the rest: no space and no hyphen may start the name.
    if name.chars().count() > 128 || !(first.is_ascii_alphanumeric() || first == '_') {
        return false;
    }
    // After the first character the pattern is `[a-zA-Z0-9_ ]+` segments joined by a
    // SINGLE `-`, so a doubled or trailing hyphen does not match it.
    if name.ends_with('-') {
        return false;
    }
    let mut previous_was_hyphen = false;
    for character in name.chars() {
        if character == '-' {
            if previous_was_hyphen {
                return false;
            }
            previous_was_hyphen = true;
            continue;
        }
        if !(character.is_ascii_alphanumeric() || matches!(character, '_' | ' ')) {
            return false;
        }
        previous_was_hyphen = false;
    }
    true
}

/// Why a page request could not be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageError {
    /// `nextToken` does not decode, or names a row this listing does not hold.
    UnknownToken,
    /// `maxResults` is not a page size. Unreachable through the wire: the model's
    /// `MaxResults` range 1..100 is refused at the boundary before any handler runs.
    InvalidMaxResults,
}

/// Encode an opaque pagination token that resumes at `id`.
///
/// Opaque to the caller (hex, so it is not a readable id) but resolved back to a
/// position by LOOKUP, not by index: an item deleted between two pages shifts every
/// index after it, which would silently skip or repeat a row.
pub fn encode_page_token(id: &str) -> String {
    ::hex::encode(id.as_bytes())
}

/// The id an opaque pagination token resumes at.
pub fn decode_page_token(token: &str) -> Result<String, PageError> {
    let bytes = ::hex::decode(token).map_err(|_| PageError::UnknownToken)?;
    String::from_utf8(bytes).map_err(|_| PageError::UnknownToken)
}

/// One page of `items`, plus the token for the page after it.
///
/// `items` is already filtered and in insertion order; `key` names each item's id.
/// `max_results` is the caller's page size (the model bounds it to 1..100); absent, the
/// whole remainder is returned. A token naming an id that is not in `items` is refused
/// rather than silently restarting at the beginning.
pub fn page<T>(
    items: Vec<T>,
    key: impl Fn(&T) -> &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<T>, Option<String>), PageError> {
    let start = match next_token {
        Some(token) => {
            let resume_at = decode_page_token(token)?;
            items
                .iter()
                .position(|item| key(item) == resume_at)
                .ok_or(PageError::UnknownToken)?
        }
        None => 0,
    };
    // The model's `MaxResults` range 1..100 is refused at the boundary before any
    // handler runs, so a non-positive page size cannot reach here; this states the
    // invariant instead of a clamp that would quietly absorb a future bypass.
    let limit = match max_results {
        Some(max) if max >= 1 => max as usize,
        Some(_) => return Err(PageError::InvalidMaxResults),
        None => items.len(),
    };
    let end = start.saturating_add(limit).min(items.len());
    let token = items.get(end).map(|item| encode_page_token(key(item)));
    let page: Vec<T> = items.into_iter().skip(start).take(end - start).collect();
    Ok((page, token))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState>)
}
