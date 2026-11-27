// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-agentregistrycontrol-agentregistrycontrol service.
//!
//! Besides the state struct itself this module holds the helpers every handler of
//! this service needs: identifier minting, identifier/ARN resolution, the request's
//! region, and the clock-owed status transitions the model documents (a registry is
//! created `CREATING` and becomes `READY`; a record is created `CREATING` and becomes
//! `DRAFT`; a deleted registry sits in `DELETING` and is then removed; a publisher is
//! created `ACTION_REQUIRED` and becomes `ACTIVE` once its domain verifies).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ApprovalConfiguration, AutoDetection, AutoDetectionConfiguration,
    CustomMetadataSchemaConfiguration, Descriptors, DiscoveryConfiguration,
    EncryptionConfiguration, Provenance, PublisherAssociation,
};

mod artifact;
mod custom_metadata;
mod descriptor_payload;
mod mcp_server_schema;
mod patch;
mod publishers;
mod schema_evolution;

/// The skill package a registry record carries: the bound and the checksum a write of one
/// has to satisfy, what the store keeps for it (its checksum alone), and what a read reports
/// for it. See [`artifact`].
pub use artifact::{
    artifact_checksum, artifact_download, artifact_refusal, stored_artifact_checksum,
    MAX_ARTIFACT_BYTES,
};

/// The publisher surface of this service: a publisher of a domain, the association that
/// binds a registry record to one, the identifiers both are addressed by, and how a request
/// about either is refused. See [`publishers`].
pub use publishers::*;

/// Applying the PATCH wrappers an update operation of this service takes: a leaf wrapper
/// over an optional field, and a wrapper over a structure whose members are each
/// independently patchable. See [`patch`].
pub use patch::{patch_optional, patch_structure};

/// The refusal message for a custom-metadata schema configuration that would WITHDRAW
/// something the registry's current one declares — a field, an allowed value of a field, or
/// a field's declared type — or `None` where the replacement keeps all three. See
/// [`schema_evolution`].
pub use schema_evolution::refusal as custom_metadata_schema_evolution_refusal;

/// The refusal message for a descriptor payload that is not the content a descriptor carries
/// — one that is not the JSON the content is serialized as, or one that is JSON no supported
/// version of the descriptor's own protocol schema accepts ([`mcp_server_schema`]) — or
/// `None` where every payload of the set is content this service admits. The ONE check a
/// record write runs its descriptors through, so a create and an edit admit the same
/// payloads.
pub use descriptor_payload::refusal as descriptor_payload_refusal;

/// Whether a record's custom metadata satisfies the schema its registry carries, the
/// compliance verdict a read reports for it, and how a write of metadata that does not
/// satisfy it is refused. A handler reads the registry's configuration out of the store,
/// releases it, and then asks for the verdict: the evaluation is caller-driven work, so it
/// runs neither under this service's lock nor on the worker serving the request. See
/// [`custom_metadata`].
pub use custom_metadata::{
    page_compliance_statuses as page_custom_metadata_compliance_statuses,
    refusal_of as custom_metadata_refusal_of, status_of as custom_metadata_status_of,
    validation_failure_message as custom_metadata_validation_failure_message,
    verdict_of as custom_metadata_verdict_of, SchemaEvaluation, SchemaVerdict, COMPLIANT,
    NON_COMPLIANT,
};

/// The region used for an ARN when the request carries no usable SigV4 credential
/// scope (an unsigned request). Every emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name, per `OPERATIONS.md`.
pub const ARN_SERVICE: &str = "agent-registry";

/// How long a resource this service creates asynchronously stays in its transient
/// status before the clock owes it the next one.
///
/// CHOSEN, not documented: AWS does not publish how long `CreateRegistry`,
/// `CreateRegistryRecord` or `DeleteRegistry` take, so this is the emulator's own
/// span inside the 250-750 ms band the project fixes for a clock-owed transition.
/// It is never zero, so a caller that acts immediately after a create still observes
/// the transient status and the state errors that go with it.
pub const ASYNC_TRANSITION: Duration = Duration::from_millis(500);

/// How many registries one account may hold before `CreateRegistry` answers the
/// modelled `ServiceQuotaExceededException`.
///
/// CHOSEN, not documented: AWS publishes no quota for this service, so the emulator
/// picks a generous one. It exists so a caller-driven store cannot grow without bound.
pub const MAX_REGISTRIES: usize = 1_000;

/// How many records one registry may hold before `CreateRegistryRecord` answers the
/// modelled `ServiceQuotaExceededException`. CHOSEN for the same reason as
/// [`MAX_REGISTRIES`].
pub const MAX_RECORDS_PER_REGISTRY: usize = 10_000;

/// How many tags one resource may hold before `TagResource` answers the modelled
/// `ServiceQuotaExceededException`.
///
/// MODELLED: it is `TagsMap`'s own `@length` maximum, which every operation carrying a
/// tag map declares for one request's worth of tags, applied to what the resource ends up
/// holding.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The fixed text a tagging operation refuses an ARN with when it is not the ARN of a
/// resource this service holds; the offending value follows it, taken from the request's
/// `resourceArn` label.
pub const UNRECOGNIZED_RESOURCE_ARN: &str = "Unrecognized Agent Registry resource ARN: ";

/// How this service states that the REGISTRY an identifier named is not there.
///
/// The ONE place this sentence is written, so no two operations of this service word the
/// same error class differently. Its fixed text names the resource class and the kind of
/// identifier the registry is addressed by, as a capitalised sentence ending in a period;
/// the one variable part is the identifier exactly as the request supplied it, since
/// `RegistryIdentifier` admits either the bare id or the full ARN and the value that
/// resolved to nothing is the value the caller sent.
pub fn registry_not_found_message(registry_identifier: &str) -> String {
    format!("Registry with ARN {registry_identifier} not found.")
}

/// How this service states that the REGISTRY RECORD an identifier named is not there, in
/// the same form as [`registry_not_found_message`] and for the same reason. Its one
/// variable part is the record's ID — the sentence says so — which is what a
/// `RecordIdentifier` carries whether it was given bare or as the record's ARN
/// ([`identifier_to_id`]). It names the record alone: the registry that was searched is
/// not part of this sentence, so the refusal carries no identifier of another resource.
pub fn record_not_found_message(record_id: &str) -> String {
    format!("Registry record with ID {record_id} not found.")
}

/// How this service states that a registry cannot be deleted while it still holds
/// registry records.
///
/// The ONE place this sentence is written, for the same reason as the not-found sentences
/// above. Its fixed text names the precondition and what the caller has to do about it;
/// the one variable part is the registry's own ARN, taken from the stored registry (the
/// value its create answered) rather than from the request, since `RegistryIdentifier`
/// also admits the bare id and the sentence quotes an ARN.
pub fn registry_not_empty_message(registry_arn: &str) -> String {
    format!(
        "Cannot delete registry '{registry_arn}' because it contains registry records. \
         Delete all registry records before deleting the registry."
    )
}

/// How this service states that a name a create asked for is already held by a live
/// resource of the same class in the same scope: the class, the requested name in single
/// quotes so a client can read it back out of the sentence, and the scope the name has to
/// be unique within. The ONE place this sentence is written, for the same reason as the
/// not-found sentences above.
fn duplicate_name_message(resource: &str, name: &str, scope: &str) -> String {
    format!("A {resource} with name '{name}' already exists in {scope}")
}

/// [`duplicate_name_message`] for a record, whose name is unique within its registry —
/// named by the identifier the request addressed that registry by.
pub fn duplicate_record_name_message(name: &str, registry_identifier: &str) -> String {
    duplicate_name_message("record", name, &format!("registry {registry_identifier}"))
}

/// [`duplicate_name_message`] for a registry, whose name is unique within the account.
pub fn duplicate_registry_name_message(name: &str) -> String {
    duplicate_name_message("registry", name, "this account")
}

/// `ValidationExceptionReason`'s modelled default, which the member carries for a request
/// refused because one of its fields did not satisfy a requirement. The whole catalogue is
/// `crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`.
pub const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The `ValidationExceptionReason` a request refused over the RESOURCE's stored state
/// carries, rather than over the shape of one of its fields — the record lifecycle's
/// transition table and its terminal status. From the same catalogue as
/// [`FIELD_VALIDATION_FAILED`].
pub const RESOURCE_CONFLICT: &str = "ResourceConflict";

/// The fixed text this service refuses a submission with when the record it addresses is
/// not in a status a submission can be made from; the status the record IS in follows it,
/// read from the stored record. The ONE place this sentence is written.
///
/// It states a status the record can still leave. A record in the lifecycle's TERMINAL status
/// is retired instead, so every change to it — a submission included — is refused with the
/// sentence that states that ([`terminal_record_status_message`]).
pub const NOT_SUBMITTABLE_IN_STATUS: &str =
    "Cannot submit the record for approval in current status: ";

/// How many `@idempotencyToken` replays are remembered at once. Entries are dropped
/// when the resource they name is removed; this cap bounds what is left, oldest first,
/// so a long-lived process cannot accumulate tokens for ever.
const MAX_IDEMPOTENCY_ENTRIES: usize = 4_096;

/// The page `ListRegistryRecords` returns when the caller names no `maxResults`, so one
/// small request cannot serialise an entire registry. It is `MaxResults`' own documented
/// maximum (range 1..100), and the caller pages on with `nextToken` exactly as it would
/// with an explicit `maxResults`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

// Status values below are the wire values of `RegistryStatus` and
// `RegistryRecordStatus`; the full catalogues are
// `crate::ops::types::REGISTRY_STATUS_VALUES` and `REGISTRY_RECORD_STATUS_VALUES`.

/// `RegistryStatus` while `CreateRegistry`'s asynchronous creation is in flight.
pub const REGISTRY_CREATING: &str = "CREATING";
/// `RegistryStatus` of a usable registry.
pub const REGISTRY_READY: &str = "READY";
/// `RegistryStatus` while `UpdateRegistry`'s asynchronous update is in flight.
pub const REGISTRY_UPDATING: &str = "UPDATING";
/// `RegistryStatus` while `DeleteRegistry`'s asynchronous deletion is in flight.
pub const REGISTRY_DELETING: &str = "DELETING";

/// `RegistryRecordStatus` while `CreateRegistryRecord` is in flight.
pub const RECORD_CREATING: &str = "CREATING";
/// `RegistryRecordStatus` of a created record before it is submitted for approval.
pub const RECORD_DRAFT: &str = "DRAFT";
/// `RegistryRecordStatus` of a submitted record waiting for a curator's manual review.
pub const RECORD_PENDING_APPROVAL: &str = "PENDING_APPROVAL";
/// `RegistryRecordStatus` of a record the approval workflow has approved.
pub const RECORD_APPROVED: &str = "APPROVED";
/// `RegistryRecordStatus` of a record a curator turned down.
pub const RECORD_REJECTED: &str = "REJECTED";
/// `RegistryRecordStatus` of a retired record, which is no longer discoverable. It is the
/// record lifecycle's TERMINAL status: nothing moves a record out of it again.
pub const RECORD_DEPRECATED: &str = "DEPRECATED";
/// `RegistryRecordStatus` while `UpdateRegistryRecord` is in flight.
pub const RECORD_UPDATING: &str = "UPDATING";
/// `RegistryRecordStatus` of a record whose creation did not finish.
pub const RECORD_CREATE_FAILED: &str = "CREATE_FAILED";
/// `RegistryRecordStatus` of a record whose edit did not finish.
pub const RECORD_UPDATE_FAILED: &str = "UPDATE_FAILED";

/// The `RegistryRecordStatus` values that report an ASYNCHRONOUS change this service makes
/// itself rather than a step of the registry's curation workflow: a record is `CREATING`
/// while `CreateRegistryRecord` is in flight and `UPDATING` while `UpdateRegistryRecord`
/// is, and the two `*_FAILED` values report that one of those did not finish.
pub const RECORD_ASYNCHRONOUS_STATUSES: &[&str] = &[
    RECORD_CREATING,
    RECORD_UPDATING,
    RECORD_CREATE_FAILED,
    RECORD_UPDATE_FAILED,
];

/// Whether a record in `status` has a change of this service's own still in flight, so it
/// is not in the curation workflow yet.
pub fn is_asynchronous_record_status(status: &str) -> bool {
    RECORD_ASYNCHRONOUS_STATUSES.contains(&status)
}

/// A step of the record lifecycle that no `RegistryRecordStatus` member names: an edit of
/// a record waiting for authorisation before it is applied. It is never a value on the
/// wire; it appears only in the list of statuses a status allows a record to move to,
/// which the service states as it is ([`record_status_transitions`]).
const RECORD_UPDATE_PENDING_AUTH: &str = "UPDATE_PENDING_AUTH";

/// The statuses a caller may name as the TARGET of a status change: the curation steps of
/// the registry's workflow. Every other `RegistryRecordStatus` is reached only by the
/// service itself — `PENDING_APPROVAL` by a submission, and `CREATING`, `UPDATING` and the
/// two `*_FAILED` values by this service's own asynchronous create and update — so a
/// request naming one is refused rather than performed. The whole catalogue is
/// `crate::ops::types::REGISTRY_RECORD_STATUS_VALUES`.
pub const RECORD_CURATION_TARGETS: &[&str] = &[
    RECORD_DRAFT,
    RECORD_APPROVED,
    RECORD_REJECTED,
    RECORD_DEPRECATED,
];

/// Whether `status` is one a caller may ask a record to be moved to.
pub fn is_record_curation_target(status: &str) -> bool {
    RECORD_CURATION_TARGETS.contains(&status)
}

/// Whether a record in `status` is retired, so no later change applies to it.
pub fn is_terminal_record_status(status: &str) -> bool {
    status == RECORD_DEPRECATED
}

/// The statuses a record has been through the registry's approval workflow in, so an EDIT
/// of its content takes it back out of that workflow: one waiting for a curator, one a
/// curator approved, and one a curator turned down. A `DRAFT` has not entered the workflow,
/// so there is nothing for an edit to withdraw it from.
pub const RECORD_WITHDRAWN_BY_EDIT: &[&str] =
    &[RECORD_PENDING_APPROVAL, RECORD_APPROVED, RECORD_REJECTED];

/// The status an edit settles a record at once this service has processed it.
///
/// An edit decides nothing about a record's curation, but it does invalidate the review the
/// record has already been through — the content a curator saw is not the content the record
/// now holds — so a record that is in the approval workflow ([`RECORD_WITHDRAWN_BY_EDIT`])
/// is withdrawn from it and settles at `DRAFT`, where it can be submitted for approval
/// again; a record that is not settles back at the status it was in.
///
/// The ONE place this rule is written, so an edit of any record of this service withdraws
/// it on the same terms. The approved revision a withdrawn record had published stays
/// published until the edit is approved in turn ([`RegistryRecord::settle_at_status`]).
pub fn status_after_edit(status: &str) -> &str {
    if RECORD_WITHDRAWN_BY_EDIT.contains(&status) {
        RECORD_DRAFT
    } else {
        status
    }
}

/// The statuses the record lifecycle allows a record in `status` to move to, by ANY means:
/// a curator's step, a submission, or this service's own asynchronous work. Its order is
/// the order the service states them in — the workflow's next steps, then retirement, then
/// the status itself (a repeat of a step applies again), then the two steps an edit of the
/// record goes through.
///
/// The `DRAFT` row is the list AWS itself states for that status, down to its order and to
/// `UPDATE_PENDING_AUTH`, which no enum member names. The other rows are that same shape
/// read off the workflow the operation documents, so a later observation of one of them
/// corrects that row alone.
///
/// A record whose own asynchronous change is still in flight is held before this table is
/// consulted, so the statuses that report one carry no row of their own.
pub fn record_status_transitions(status: &str) -> &'static [&'static str] {
    match status {
        RECORD_CREATING => &[RECORD_DRAFT, RECORD_CREATE_FAILED],
        RECORD_DRAFT => &[
            RECORD_PENDING_APPROVAL,
            RECORD_DEPRECATED,
            RECORD_DRAFT,
            RECORD_UPDATING,
            RECORD_UPDATE_PENDING_AUTH,
        ],
        RECORD_PENDING_APPROVAL => &[
            RECORD_APPROVED,
            RECORD_REJECTED,
            RECORD_DEPRECATED,
            RECORD_PENDING_APPROVAL,
            RECORD_UPDATING,
            RECORD_UPDATE_PENDING_AUTH,
        ],
        RECORD_APPROVED => &[
            RECORD_REJECTED,
            RECORD_DEPRECATED,
            RECORD_APPROVED,
            RECORD_UPDATING,
            RECORD_UPDATE_PENDING_AUTH,
        ],
        RECORD_REJECTED => &[
            RECORD_DRAFT,
            RECORD_DEPRECATED,
            RECORD_REJECTED,
            RECORD_UPDATING,
            RECORD_UPDATE_PENDING_AUTH,
        ],
        _ => &[],
    }
}

/// Whether the record lifecycle lets a record in `current` move to `target`.
///
/// The table also carries the statuses this service's own work reaches, which no request may
/// name, so a caller's target is admitted by [`is_record_curation_target`] first.
pub fn record_status_transition_allowed(current: &str, target: &str) -> bool {
    record_status_transitions(current).contains(&target)
}

/// How this service refuses a target status a caller may not name: the fixed lead-in and
/// the value the request asked for.
pub fn invalid_target_status_message(status: &str) -> String {
    format!("Invalid target status: {status}")
}

/// How this service refuses a status change the record lifecycle does not allow: the
/// status the record is stored in, the status the request asked for, and the statuses that
/// stored status does allow, listed in the order [`record_status_transitions`] states them.
pub fn invalid_status_transition_message(current: &str, target: &str) -> String {
    format!(
        "Invalid status transition from {current} to {target}. Valid transitions from \
         {current}: {}",
        record_status_transitions(current).join(", ")
    )
}

/// How this service refuses any later change to a record in a terminal status: the record is
/// retired, so an edit, a status change and a submission for approval all state this. The one
/// variable part is the status the record is stored in.
///
/// A retired record is past the whole lifecycle, so WHICH change was asked for does not
/// change the answer; the sentences an operation words for the statuses a record can still
/// leave ([`NOT_SUBMITTABLE_IN_STATUS`], [`invalid_status_transition_message`]) are about
/// those statuses alone.
pub fn terminal_record_status_message(status: &str) -> String {
    format!("Cannot update registry record in {status} status (terminal state)")
}

/// The `RegistryAuthorizerType` a registry whose creation named none is authorized under —
/// the type that controls "how consumers access the registry's search and MCP invoke
/// operations".
///
/// The catalogue is `crate::ops::types::REGISTRY_AUTHORIZER_TYPE_VALUES`, and of its two
/// values this is the one a registry needs nothing else for: `CUSTOM_JWT` is the one whose
/// `authorizerConfiguration` is "required when authorizerType is CUSTOM_JWT", which a
/// request that named no discovery configuration at all did not supply. So a registry
/// created without one is reached under the account's own IAM authorization, which is how
/// every request to this service is authorized to begin with.
pub const DEFAULT_REGISTRY_AUTHORIZER_TYPE: &str = "AWS_IAM";

/// The discovery configuration a registry is created with: the one the request named, with
/// the service's own default filled in for what it left out.
///
/// A registry HAS a discovery configuration whether or not the create named one — a consumer
/// of the registry is authorized one way or another — so this is what a read reports rather
/// than nothing at all. Only an absent `authorizerType` is filled
/// ([`DEFAULT_REGISTRY_AUTHORIZER_TYPE`]); a value the request carried is kept as it came.
pub fn effective_discovery_configuration(
    requested: Option<DiscoveryConfiguration>,
) -> DiscoveryConfiguration {
    let mut configuration = requested.unwrap_or_default();
    if configuration.authorizer_type.is_none() {
        configuration.authorizer_type = Some(DEFAULT_REGISTRY_AUTHORIZER_TYPE.to_string());
    }
    configuration
}

/// The approval configuration a registry is created with: the one the request named, with an
/// EMPTY rule list filled in for what it left out.
///
/// `autoApprovalRules` are "the rules that determine which registry records are automatically
/// approved on submission. When omitted or empty, submitted records require manual review" —
/// so a registry created without them has an approval workflow all the same, the one with no
/// rule in it, and that is what a read reports. A list the request carried is kept as it
/// came, empty or not.
pub fn effective_approval_configuration(
    requested: Option<ApprovalConfiguration>,
) -> ApprovalConfiguration {
    let mut configuration = requested.unwrap_or_default();
    if configuration.auto_approval_rules.is_none() {
        configuration.auto_approval_rules = Some(Vec::new());
    }
    configuration
}

/// The `AutoDetectionStatus` of a registry whose auto-detection is running, from the
/// catalogue `crate::ops::types::AUTO_DETECTION_STATUS_VALUES`.
pub const AUTO_DETECTION_ACTIVE: &str = "ACTIVE";

/// The `AutoDetectionStatus` of a registry whose auto-detection is not running, from the
/// same catalogue as [`AUTO_DETECTION_ACTIVE`].
pub const AUTO_DETECTION_INACTIVE: &str = "INACTIVE";

/// The `AutoDetection` a registry carries once a request configures auto-detection on it:
/// "the requested configuration and the current detection status".
///
/// `enabled` is "whether auto-detection is requested for the registry", and the status is
/// `ACTIVE` when "the registry is actively being populated with detected resources" — so a
/// configuration that asks for detection is reported ACTIVE and one that does not is
/// INACTIVE. The ONE place this is derived, so a registry's create and its update report the
/// same status for the same configuration.
pub fn auto_detection_for(configuration: AutoDetectionConfiguration) -> AutoDetection {
    AutoDetection {
        status: if configuration.enabled {
            AUTO_DETECTION_ACTIVE.to_string()
        } else {
            AUTO_DETECTION_INACTIVE.to_string()
        },
        configuration,
        status_reason: None,
    }
}

/// The one `AutoApprovalRule` the model declares — the catalogue it comes from is
/// `crate::ops::types::AUTO_APPROVAL_RULE_VALUES`. It selects every submitted record,
/// so a registry configured with it approves a submission without manual review.
pub const AUTO_APPROVAL_RULE_APPROVE_ALL: &str = "APPROVE_ALL";

/// A status change the clock owes a resource: what it becomes, when that is due on
/// the monotonic clock, and the wall-clock instant to stamp on `updatedAt` when it
/// is materialised. Nothing sleeps for it — the next caller that touches the state
/// applies every transition whose instant has passed.
#[derive(Debug, Clone)]
pub struct DueTransition<T> {
    /// When the transition becomes due, on the monotonic clock.
    pub due_at: Instant,
    /// The wall-clock instant the transition happens at (`updatedAt`).
    pub at: DateTime,
    /// What the resource becomes.
    pub to: T,
}

impl<T> DueTransition<T> {
    /// A transition due [`ASYNC_TRANSITION`] from now.
    pub fn after_async_span(to: T) -> Self {
        Self {
            due_at: Instant::now() + ASYNC_TRANSITION,
            at: DateTime::from_secs_f64(unix_secs() + ASYNC_TRANSITION.as_secs_f64()),
            to,
        }
    }
}

/// What a registry's pending transition turns it into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryTransition {
    /// `CREATING` -> `READY`: creation finished.
    Ready,
    /// `DELETING` -> gone: the registry and its records are removed.
    Removed,
}

/// What a record's pending transition turns it into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordTransition {
    /// `CREATING` -> `DRAFT`: creation finished; the record can be submitted for approval.
    Draft,
    /// `UPDATING` -> the status the update settles the record at: the one it held when
    /// the update was accepted, unless the edit took it out of the approval workflow.
    Settled(String),
}

/// A registry, exactly as `CreateRegistry` stored it.
#[derive(Debug)]
pub struct Registry {
    pub registry_id: String,
    pub registry_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub discovery_configuration: Option<DiscoveryConfiguration>,
    pub approval_configuration: Option<ApprovalConfiguration>,
    pub custom_metadata_schema_configuration: Option<CustomMetadataSchemaConfiguration>,
    pub auto_detection: Option<AutoDetection>,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub tags: BTreeMap<String, String>,
    /// Keyed by record id, so `ListRegistryRecords` has a stable order and its
    /// `nextToken` names a well-defined resume point.
    pub records: BTreeMap<String, RegistryRecord>,
    pub pending: Option<DueTransition<RegistryTransition>>,
}

impl Registry {
    /// Whether this registry's approval configuration approves a record on submission
    /// instead of sending it to manual review.
    ///
    /// `ApprovalConfiguration.autoApprovalRules` are "the rules that determine which
    /// registry records are automatically approved on submission. When omitted or
    /// empty, submitted records require manual review." A rule covers a submission when
    /// its selector holds for the record; [`AUTO_APPROVAL_RULE_APPROVE_ALL`], the only
    /// rule the model declares, selects every record. A rule outside that catalogue
    /// covers nothing, so an unrecognised one leaves the submission for manual review
    /// rather than approving it unseen.
    pub fn auto_approves_submissions(&self) -> bool {
        self.approval_configuration
            .as_ref()
            .and_then(|configuration| configuration.auto_approval_rules.as_ref())
            .is_some_and(|rules| {
                rules
                    .iter()
                    .any(|rule| rule == AUTO_APPROVAL_RULE_APPROVE_ALL)
            })
    }
}

/// The content of a registry record as of the moment it was last APPROVED: the record's
/// published revision.
///
/// A record enters the discovery surface when the approval workflow approves it, and what
/// that surface serves is the content that was approved. So a record carries the revision
/// its approval published, which the discovery reads report until the workflow publishes
/// another one: an edit of an approved record withdraws the record from the workflow
/// ([`status_after_edit`]) without publishing the edited content, so consumers keep
/// resolving the last approved revision while the edit waits to be approved in turn.
///
/// It holds the content members alone. A record's identity (its ids and ARNs, the account
/// that created it and when) does not change with a revision, so it is read from the record
/// itself; the control plane's own reads always report the record as it stands now.
#[derive(Debug, Clone)]
pub struct ApprovedRevision {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub record_type: String,
    pub descriptors: Option<Descriptors>,
    pub record_version: String,
    pub custom_metadata: Option<::aws_smithy_types::Document>,
    /// When this revision was published — the instant the approval that published it
    /// stamped on the record.
    pub updated_at: DateTime,
}

/// A registry record, exactly as `CreateRegistryRecord` stored it.
#[derive(Debug)]
pub struct RegistryRecord {
    pub record_id: String,
    pub record_arn: String,
    pub registry_arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub record_type: String,
    pub descriptors: Option<Descriptors>,
    pub record_version: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub provenance: Vec<Provenance>,
    pub custom_metadata: Option<::aws_smithy_types::Document>,
    pub created_by: Option<String>,
    pub created_by_auto_detection: Option<bool>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub tags: BTreeMap<String, String>,
    pub pending: Option<DueTransition<RecordTransition>>,
    /// The SHA-256 checksum of the skill package uploaded with this record, or `None` for a
    /// record that carries no package. The archive's own bytes are NOT kept: see
    /// [`artifact`].
    pub artifact_sha256: Option<String>,
    /// The revision the approval workflow last published, or `None` for a record no
    /// approval has published: see [`ApprovedRevision`].
    pub approved_revision: Option<ApprovedRevision>,
}

impl RegistryRecord {
    /// Move this record into `status`, keeping its published revision in step.
    ///
    /// Every settled status this service writes goes through here — a submission's verdict,
    /// a curator's step, and the status an asynchronous change settles at — so what the
    /// discovery surface serves follows from the workflow alone: entering `APPROVED`
    /// publishes the content the record holds at that moment, and leaving the approved
    /// state for a status that is not discoverable withdraws what was published. Any other
    /// status leaves the published revision alone, which is what lets a withdrawn edit keep
    /// the last approved revision available to consumers.
    pub fn settle_at_status(&mut self, status: &str) {
        self.status = status.to_string();
        match status {
            RECORD_APPROVED => {
                let published = self.revision();
                self.approved_revision = Some(published);
            }
            RECORD_REJECTED | RECORD_DEPRECATED => self.approved_revision = None,
            _ => {}
        }
    }

    /// The content this record holds now, as a revision.
    fn revision(&self) -> ApprovedRevision {
        ApprovedRevision {
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            record_type: self.record_type.clone(),
            descriptors: self.descriptors.clone(),
            record_version: self.record_version.clone(),
            custom_metadata: self.custom_metadata.clone(),
            updated_at: self.updated_at,
        }
    }
}

/// What one `@idempotencyToken` replay remembers: the ANSWER the first call returned, a
/// fingerprint of the parameters it was presented with ([`idempotency_fingerprint`]), and
/// the order it was remembered in, so the oldest entry is the one evicted at the cap.
#[derive(Debug, Clone)]
pub struct IdempotentAnswer {
    answer: RememberedCreate,
    fingerprint: String,
    seq: u64,
}

/// The answer a create returned, remembered so a replay of its `@idempotencyToken` can
/// return it again.
///
/// A replay "ignores the request", so it decides nothing and computes nothing from the
/// resource as it stands now: the members it answers are the ones the first call answered,
/// which is what makes two calls carrying one token indistinguishable to their caller. A
/// create of this service answers the new resource's ARN and, where the operation models
/// the member, the status that call reported — the transient status an asynchronous create
/// reports, which the resource itself has since left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RememberedCreate {
    /// The ARN the first call minted.
    pub arn: String,
    /// The resource status the first call reported, for a create whose output models one.
    pub status: Option<String>,
}

impl RememberedCreate {
    /// The answer of a create whose output carries the new resource's ARN alone.
    pub fn from_arn(arn: &str) -> Self {
        Self {
            arn: arn.to_string(),
            status: None,
        }
    }

    /// The answer of a create whose output carries the new resource's ARN and the status it
    /// reported it in.
    pub fn from_arn_and_status(arn: &str, status: &str) -> Self {
        Self {
            arn: arn.to_string(),
            status: Some(status.to_string()),
        }
    }
}

/// What a presented `@idempotencyToken` means for the call presenting it.
///
/// `ClientToken` is `@idempotencyToken`: "A unique, case-sensitive identifier to ensure
/// that the operation completes no more than one time. If this token matches a previous
/// request, the service ignores the request, but does not return an error" — which binds
/// the token to the parameters of the request that first used it. A second call presenting
/// it therefore has three possible meanings, and only one of them creates anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenReplay {
    /// No remembered call used this token: the request is a first call and creates.
    Unused,
    /// A remembered call used it with the SAME parameters: the request is ignored and
    /// answered with what that call answered.
    Replayed(RememberedCreate),
    /// A remembered call used it with DIFFERENT parameters: the token is already bound to
    /// another request, which is a conflict rather than a create
    /// ([`IDEMPOTENCY_PARAMETER_MISMATCH`]).
    ParametersDiffer,
}

/// What a create answers when an `@idempotencyToken` is presented with parameters other
/// than the ones it was first used with. Wholly fixed text: it names neither the token nor
/// any parameter, because the token is bound to the whole request and not to one member of
/// it.
pub const IDEMPOTENCY_PARAMETER_MISMATCH: &str =
    "Idempotency token already used with different request parameters";

/// Per-service state for com-amazonaws-agentregistrycontrol-agentregistrycontrol. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAgentregistrycontrolAgentregistrycontrolState {
    /// Every registry, keyed by its registry id.
    pub registries: HashMap<String, Registry>,
    /// Registry name -> registry id, so a duplicate name is a `ConflictException`.
    pub registry_name_index: HashMap<String, String>,
    /// Every publisher, keyed by its domain folded to lower case ([`publisher_key`]) — the
    /// account holds one publisher per domain, and a DNS name is case-insensitive. Ordered,
    /// so `ListPublishers` has a stable order and its `nextToken` names a well-defined
    /// resume point.
    pub publishers: BTreeMap<String, Publisher>,
    /// Every publisher association, keyed by the ARN of the registry record it binds — "a
    /// registry record can have only one publisher association". Ordered for the same reason
    /// as [`Self::publishers`].
    pub associations: BTreeMap<String, PublisherAssociation>,
    /// `<operation>#<clientToken>` -> what that call answered, and the parameters it
    /// answered them for. The operation is part of the key, so one token reused across
    /// two different creates cannot make one replay the other's answer.
    idempotency: HashMap<String, IdempotentAnswer>,
    /// Insertion order for the idempotency map, for eviction at the cap.
    idempotency_seq: u64,
    /// The earliest instant at which some stored transition becomes due, so a request
    /// that arrives before then does no scanning at all. `None` when nothing is pending.
    next_due: Option<Instant>,
}

impl ComAmazonawsAgentregistrycontrolAgentregistrycontrolState {
    /// Apply every status transition the clock already owes.
    ///
    /// Call this at the top of every handler, before reading or writing a resource:
    /// it only materialises a change that was already due at this instant, so a read
    /// that calls it reports what a caller would see whether or not anybody asked.
    pub fn materialise_due_transitions(&mut self) {
        let now = Instant::now();
        // Nothing is due: no store is walked, so this costs the same whatever the
        // stores hold.
        if self.next_due.is_none_or(|due_at| due_at > now) {
            return;
        }

        let mut earliest: Option<Instant> = None;
        let mut arm = |due_at: Instant| {
            earliest = Some(match earliest {
                Some(current) if current <= due_at => current,
                _ => due_at,
            });
        };
        let mut removed: Vec<(String, String)> = Vec::new();
        self.registries.retain(|_, registry| {
            if let Some(pending) = registry.pending.clone() {
                if pending.due_at <= now {
                    match pending.to {
                        RegistryTransition::Ready => {
                            registry.status = REGISTRY_READY.to_string();
                            registry.updated_at = pending.at;
                            registry.pending = None;
                        }
                        RegistryTransition::Removed => {
                            removed.push((registry.name.clone(), registry.registry_arn.clone()));
                            return false;
                        }
                    }
                } else {
                    arm(pending.due_at);
                }
            }
            for record in registry.records.values_mut() {
                if let Some(pending) = record.pending.clone() {
                    if pending.due_at <= now {
                        let status = match &pending.to {
                            RecordTransition::Draft => RECORD_DRAFT.to_string(),
                            RecordTransition::Settled(status) => status.clone(),
                        };
                        record.updated_at = pending.at;
                        record.settle_at_status(&status);
                        record.pending = None;
                    } else {
                        arm(pending.due_at);
                    }
                }
            }
            true
        });
        // The publisher surface's own clock-owed steps: a domain verification that has run
        // its course, and a delete that has.
        let mut removed_publishers: Vec<String> = Vec::new();
        self.publishers.retain(|_, publisher| {
            if let Some(pending) = publisher.pending.clone() {
                if pending.due_at <= now {
                    match pending.to {
                        PublisherTransition::Verified => {
                            publisher.status = PUBLISHER_ACTIVE.to_string();
                            publisher.status_reason = None;
                            publisher.updated_at = pending.at;
                            publisher.pending = None;
                        }
                        PublisherTransition::Removed => {
                            removed_publishers.push(publisher.publisher_arn.clone());
                            return false;
                        }
                    }
                } else {
                    arm(pending.due_at);
                }
            }
            true
        });
        self.next_due = earliest;
        for (name, registry_arn) in removed {
            self.registry_name_index.remove(&name);
            // The registry and its records are gone, so the tokens that named them are
            // of no use to anybody: a replay of that create mints a new resource.
            self.forget_idempotency_under(&registry_arn);
        }
        for publisher_arn in removed_publishers {
            self.forget_idempotency_under(&publisher_arn);
        }
    }

    /// Record that a transition becomes due at `due_at`, so
    /// [`materialise_due_transitions`](Self::materialise_due_transitions) wakes for it.
    pub fn arm_transition(&mut self, due_at: Instant) {
        self.next_due = Some(match self.next_due {
            Some(current) if current <= due_at => current,
            _ => due_at,
        });
    }

    /// What `token` means for a call of `operation` whose parameters fingerprint as
    /// `fingerprint`: unused, a replay of the same request, or the same token bound to a
    /// different one. See [`TokenReplay`].
    pub fn token_replay(&self, operation: &str, token: &str, fingerprint: &str) -> TokenReplay {
        match self.idempotency.get(&idempotency_key(operation, token)) {
            None => TokenReplay::Unused,
            Some(answer) if answer.fingerprint == fingerprint => {
                TokenReplay::Replayed(answer.answer.clone())
            }
            Some(_) => TokenReplay::ParametersDiffer,
        }
    }

    /// Remember what `operation` answered for `token`, and the parameters it was presented
    /// with, evicting the oldest entry when the cap is reached so this store cannot grow
    /// without bound.
    pub fn remember_idempotent_answer(
        &mut self,
        operation: &str,
        token: &str,
        fingerprint: &str,
        answer: RememberedCreate,
    ) {
        self.idempotency_seq = self.idempotency_seq.wrapping_add(1);
        let key = idempotency_key(operation, token);
        if self.idempotency.len() >= MAX_IDEMPOTENCY_ENTRIES && !self.idempotency.contains_key(&key)
        {
            let oldest = self
                .idempotency
                .iter()
                .min_by_key(|(_, answer)| answer.seq)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.idempotency.remove(&oldest);
            }
        }
        self.idempotency.insert(
            key,
            IdempotentAnswer {
                answer,
                fingerprint: fingerprint.to_string(),
                seq: self.idempotency_seq,
            },
        );
    }

    /// Forget every remembered answer naming a removed resource: the resource itself and
    /// anything nested under it, since a registry's ARN is the prefix of its records'
    /// ARNs. The prefix only counts when the ARN either ends there or continues with the
    /// `/` that starts a nested resource, so one id that is a text prefix of another
    /// (`RegistryId` is 12..16 characters) cannot take the other's tokens with it.
    pub fn forget_idempotency_under(&mut self, arn: &str) {
        self.idempotency.retain(|_, remembered| {
            remembered.answer.arn != arn
                && !remembered
                    .answer
                    .arn
                    .strip_prefix(arn)
                    .is_some_and(|nested| nested.starts_with('/'))
        });
    }

    /// A registry id no registry holds: 12 characters of `[0-9a-f]`, which satisfies
    /// `RegistryId`'s `^[a-zA-Z0-9]{12,16}$`.
    pub fn mint_registry_id(&self) -> String {
        loop {
            let candidate = mint_id();
            if !self.registries.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// A record id no record of `registry_id` holds: 12 characters of `[0-9a-f]`,
    /// which satisfies `RegistryRecordId`'s `^[a-zA-Z0-9]{12}$`.
    pub fn mint_record_id(&self, registry_id: &str) -> String {
        loop {
            let candidate = mint_id();
            let taken = self
                .registries
                .get(registry_id)
                .is_some_and(|registry| registry.records.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// The tag set of the resource `addressed` names, to read, or `None` when this state
    /// holds no such resource.
    ///
    /// The reading half of [`Self::resource_tags_mut`]: one lookup for every tagging
    /// operation, so what a read reports is the set a write wrote.
    pub fn resource_tags(&self, addressed: &TaggableResource) -> Option<&BTreeMap<String, String>> {
        match addressed {
            TaggableResource::Registry { registry_id } => {
                Some(&self.registries.get(registry_id)?.tags)
            }
            TaggableResource::Record {
                registry_id,
                record_id,
            } => Some(
                &self
                    .registries
                    .get(registry_id)?
                    .records
                    .get(record_id)?
                    .tags,
            ),
        }
    }

    /// The tag set of the resource `addressed` names, to read or to write, or `None` when
    /// this state holds no such resource.
    ///
    /// One lookup for every tagging operation, so the resource a tag is written to is the
    /// one a later read reports it from.
    pub fn resource_tags_mut(
        &mut self,
        addressed: &TaggableResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match addressed {
            TaggableResource::Registry { registry_id } => {
                Some(&mut self.registries.get_mut(registry_id)?.tags)
            }
            TaggableResource::Record {
                registry_id,
                record_id,
            } => Some(
                &mut self
                    .registries
                    .get_mut(registry_id)?
                    .records
                    .get_mut(record_id)?
                    .tags,
            ),
        }
    }
}

/// The key an idempotent answer is remembered under: the operation and the token, so a
/// token reused across two operations cannot cross answers.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}#{token}")
}

/// A fingerprint of the parameters an `@idempotencyToken` was presented with: the request
/// serialized against its own modelled schema, with every object's members in key order.
///
/// The caller hands the request in with its token member CLEARED, so what is fingerprinted
/// is the parameters the token names and not the token itself.
///
/// Rendered through the protocol's own JSON serializer and the model's schema, so the
/// member names and the value forms are the model's rather than a second reading of the
/// request held here. Sorted because neither rendering is a function of the value on its
/// own: a map member (`tags`, and the `Document` a record's `customMetadata` is) is written
/// in its `HashMap` iteration order, which two maps holding the same entries do not share,
/// so one request would otherwise fingerprint two ways. `Debug` is the fallback for a
/// request the serializer cannot render, which leaves such a request compared by value
/// rather than compared not at all.
pub fn idempotency_fingerprint<T>(parameters: &T, schema: &::aws_smithy_schema::Schema) -> String
where
    T: ::aws_smithy_schema::serde::SerializableStruct + ::std::fmt::Debug,
{
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer =
        ::aws_smithy_schema_server::json::JsonShapeSerializer::new_with_config(true);
    if serializer.write_struct(schema, parameters).is_err() {
        return format!("{parameters:?}");
    }
    let rendered = serializer.into_string();
    // `serde_json`'s own parse bounds how deep a document it will read, so this walk is
    // bounded by that crate rather than by the request; a document past that bound is not
    // sorted and is compared as rendered.
    match ::serde_json::from_str::<::serde_json::Value>(&rendered) {
        Ok(parsed) => with_sorted_members(parsed).to_string(),
        Err(_) => rendered,
    }
}

/// `value` with the members of every object in key order, which is what makes two
/// renderings of one request the same text.
fn with_sorted_members(value: ::serde_json::Value) -> ::serde_json::Value {
    match value {
        ::serde_json::Value::Object(members) => ::serde_json::Value::Object(
            members
                .into_iter()
                .collect::<BTreeMap<String, ::serde_json::Value>>()
                .into_iter()
                .map(|(name, member)| (name, with_sorted_members(member)))
                .collect(),
        ),
        ::serde_json::Value::Array(items) => {
            ::serde_json::Value::Array(items.into_iter().map(with_sorted_members).collect())
        }
        scalar => scalar,
    }
}

/// A resource of this service an ARN names: what the tagging operations' `resourceArn`
/// addresses, which the model documents as "registries and registry records".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableResource {
    /// `arn:<partition>:agent-registry:<region>:<account>:registry/<registryId>`.
    Registry { registry_id: String },
    /// A record, whose ARN the model nests under its registry's.
    Record {
        registry_id: String,
        record_id: String,
    },
}

/// The resource of this service `arn` names, or `None` when `arn` is not the ARN of one.
///
/// Recognised exactly where the model's two ARN shapes are: `RegistryArn`
/// (`^arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/[a-zA-Z0-9]{12,16}$`)
/// and `RegistryRecordArn` (the same, continued with `/record/[a-zA-Z0-9]{12}`). The
/// `ResourceArn` the tagging operations bind declares no pattern at all — any string of
/// 1..1011 characters reaches the handler — so this is the one place an ARN of another
/// vendor, of a resource type this service does not hold, or with an identifier outside
/// those classes is told apart from a resource it could hold. Splitting the value into its
/// ARN fields is `service-common`'s single ARN parse.
pub fn taggable_resource(arn: &str) -> Option<TaggableResource> {
    let parsed = ::service_common::arn::parse(arn)?;
    let partitioned = parsed.partition == "aws" || parsed.partition.starts_with("aws-");
    let regional = !parsed.region.is_empty()
        && parsed
            .region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let accounted =
        parsed.account.len() == 12 && parsed.account.bytes().all(|byte| byte.is_ascii_digit());
    if parsed.vendor != ARN_SERVICE || !partitioned || !regional || !accounted {
        return None;
    }
    let segments: Vec<&str> = parsed.resource.split('/').collect();
    match segments.as_slice() {
        ["registry", registry_id] if is_registry_id(registry_id) => {
            Some(TaggableResource::Registry {
                registry_id: (*registry_id).to_string(),
            })
        }
        ["registry", registry_id, "record", record_id]
            if is_registry_id(registry_id) && is_record_id(record_id) =>
        {
            Some(TaggableResource::Record {
                registry_id: (*registry_id).to_string(),
                record_id: (*record_id).to_string(),
            })
        }
        _ => None,
    }
}

/// Whether `id` is a `RegistryId`: `^[a-zA-Z0-9]{12,16}$`.
fn is_registry_id(id: &str) -> bool {
    (12..=16).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Whether `id` is a `RegistryRecordId`: `^[a-zA-Z0-9]{12}$`.
fn is_record_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

impl TaggableResource {
    /// How this service states that the resource an ARN named is not there: the same two
    /// sentences every other operation of this service states that error class in — a
    /// registry by the identifier the request supplied (here its ARN), a record by its id.
    pub fn not_found_message(&self, resource_arn: &str) -> String {
        match self {
            Self::Registry { .. } => registry_not_found_message(resource_arn),
            Self::Record { record_id, .. } => record_not_found_message(record_id),
        }
    }
}

/// A process-wide mint counter, so two ids minted in the same nanosecond still differ.
static MINT_SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// An identifier of 12 characters drawn from `[0-9a-f]`, which is 48 bits of the value
/// below — enough for `RegistryId`'s `^[a-zA-Z0-9]{12,16}$` and `RegistryRecordId`'s
/// `^[a-zA-Z0-9]{12}$`, and the caller re-mints on a collision with a stored id, so
/// uniqueness is guaranteed by the store rather than by the draw.
///
/// The bits come from the standard library's
/// [`std::collections::hash_map::RandomState`], whose SipHash keys the host seeds from
/// the operating system when it is constructed, mixed with the wall clock and a
/// process-wide counter so two draws in the same instant cannot coincide.
fn mint_id() -> String {
    use ::std::hash::BuildHasher;

    let sequence = MINT_SEQUENCE.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let drawn = ::std::collections::hash_map::RandomState::new().hash_one((sequence, nanos));
    format!("{:012x}", drawn & 0x0000_ffff_ffff_ffff)
}

/// The value of a DNS-01 challenge: 48 characters of `[0-9a-f]`, which satisfies
/// `ChallengeRecordValue`'s `@length(1, 1024)`.
///
/// A new value is issued every time domain verification starts and the earlier one stops
/// verifying the domain, so what matters is that two draws differ. The bits come from the
/// same source as [`mint_id`], drawn four times over.
pub fn mint_challenge_value() -> String {
    (0..4).map(|_| mint_id()).collect::<String>()
}

/// The id named by a `RegistryIdentifier` / `RecordIdentifier` label, which the model
/// lets a caller give either bare or as the resource's full ARN. The id is the ARN's
/// last `/`-separated segment, and a bare id has no `/` at all.
pub fn identifier_to_id(identifier: &str) -> &str {
    identifier
        .split('/')
        .next_back()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(identifier)
}

/// The region to build an ARN in: the one the request's SigV4 credential scope names,
/// else [`DEFAULT_REGION`]. A scope value that is not shaped like a region is ignored,
/// so a minted ARN always satisfies the `[a-z0-9-]+` region segment the model declares.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The current wall-clock instant, as the `DateTimestamp` the model binds to
/// `createdAt` / `updatedAt`.
pub fn now() -> DateTime {
    DateTime::from_secs_f64(unix_secs())
}

/// Seconds since the Unix epoch, as a float, so a timestamp and a timestamp offset
/// by a span are built the same way.
fn unix_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The ARN of a registry: `arn:aws:agent-registry:<region>:<account>:registry/<id>`.
pub fn registry_arn(region: &str, registry_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("registry/{registry_id}"))
}

/// The ARN of a record, which the model nests under its registry's ARN.
pub fn record_arn(registry_arn: &str, record_id: &str) -> String {
    format!("{registry_arn}/record/{record_id}")
}

/// The opaque `nextToken` a paginated list of this service hands out: it carries the SCOPE
/// it was issued for — the registry whose records are being listed, or the name of the
/// collection ([`PUBLISHERS_PAGE_SCOPE`], [`ASSOCIATIONS_PAGE_SCOPE`]) — and the last key of
/// the page, hex-encoded (the `hex` crate this crate already depends on) so a caller reads
/// it as nothing but a cursor — and hex satisfies `NextToken`'s `^\S*$`, holding no
/// whitespace.
///
/// The scope is the part before the first `/`, so a cursor that itself carries `/` (a
/// record's ARN) survives the round trip.
pub fn encode_page_token(scope: &str, cursor: &str) -> String {
    ::hex::encode(format!("{scope}/{cursor}"))
}

/// The scope [`encode_page_token`] names a page of `ListPublishers` with. It is no registry
/// id (`RegistryId` is 12..16 alphanumerics), so one list's token cannot resume another's.
pub const PUBLISHERS_PAGE_SCOPE: &str = "publishers";

/// The scope [`encode_page_token`] names a page of `ListPublisherAssociations` with, for the
/// same reason as [`PUBLISHERS_PAGE_SCOPE`].
pub const ASSOCIATIONS_PAGE_SCOPE: &str = "associations";

/// The cursor a page token resumes after, or `None` when the token is not one this
/// service issued for `scope` — which is a refusal, not an implicit first page.
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (issued_for, cursor) = decoded.split_once('/')?;
    (issued_for == scope && !cursor.is_empty()).then(|| cursor.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>)
}
