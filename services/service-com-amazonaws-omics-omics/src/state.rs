// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-omics-omics service.
//!
//! This slice holds the service's primary resources: the **private workflow**
//! (`CreateWorkflow` / `GetWorkflow` / `ListWorkflows` / `DeleteWorkflow`), the
//! **versions** of that workflow (`CreateWorkflowVersion` / `GetWorkflowVersion`), the
//! **run** started from it (`StartRun` / `GetRun` / `ListRuns` / `DeleteRun`), the
//! **run group** that caps the compute those runs may use (`CreateRunGroup` /
//! `GetRunGroup` / `ListRunGroups` / `DeleteRunGroup`), the **run cache** a run reads task outputs from (`CreateRunCache` /
//! `GetRunCache`) and the **sequence store** that holds read sets
//! (`CreateSequenceStore` / `GetSequenceStore` / `DeleteSequenceStore`), together with the **multipart read set
//! upload** a caller initiates against that store
//! (`CreateMultipartReadSetUpload`) and the **parts** uploaded into it
//! (`UploadReadSetPart` / `ListReadSetUploadParts`), and the **read set** that upload's
//! completion forms (`CompleteMultipartReadSetUpload` / `GetReadSetMetadata` /
//! `ListReadSets`). Each resource is stored exactly once, keyed by the numeric id
//! the model's `^[0-9]+$` patterns describe (a version by its name, inside its
//! workflow's record), so every operation reads and writes the one record.
//!
//! Every store is a `BTreeMap<u64, _>` keyed by that numeric id. The ids are minted
//! from a monotonic counter, so the map's own key order IS creation order: a page
//! token can simply name the id the next page starts at, and the order is stable
//! across calls even when records in between are deleted.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ContainerRegistryMap, DefinitionRepository, DefinitionRepositoryDetails, ETag, FileInformation,
    ReadSetFiles, ReadSetS3Access, SequenceInformation, SequenceStoreS3Access, ShareDetails,
    SseConfig, WorkflowParameter,
};

/// What this service derives from the bytes of a read set it ingested: the per-file
/// digests, the sequence counts, the alignment and the S3 location of each file.
mod read_set_ingest;
pub use read_set_ingest::*;

/// The ARN service namespace for this service (`arnNamespace`/sigv4 name `omics`).
pub const ARN_SERVICE: &str = "omics";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource portion a workflow ARN carries before its id.
pub const WORKFLOW_ARN_RESOURCE_PREFIX: &str = "workflow/";

/// What a workflow VERSION ARN carries between its workflow's id and the version
/// name: a version is named inside its workflow's resource path, which is what
/// `CreateWorkflowVersion` documents when it says "Version names appear in the workflow
/// version ARN".
pub const WORKFLOW_VERSION_ARN_INFIX: &str = "/version/";

/// The resource portion a run ARN carries before its id.
pub const RUN_ARN_RESOURCE_PREFIX: &str = "run/";

/// The resource portion a sequence store ARN carries before its id.
pub const SEQUENCE_STORE_ARN_RESOURCE_PREFIX: &str = "sequenceStore/";

/// What a read set ARN carries between its sequence store's id and its own: a read set
/// is named INSIDE the store that holds it — every read set operation names both ids —
/// so one ARN names the pair and no read set ARN can collide with a store's own.
pub const READ_SET_ARN_INFIX: &str = "/readSet/";

/// The resource portion a run cache ARN carries before its id.
pub const RUN_CACHE_ARN_RESOURCE_PREFIX: &str = "runCache/";

/// The resource portion a run group ARN carries before its id.
pub const RUN_GROUP_ARN_RESOURCE_PREFIX: &str = "runGroup/";

/// The first id either counter mints.
///
/// The modelled `WorkflowId` / `RunId` are `^[0-9]+$` with length 1..18, so any
/// decimal below 10^18 is admissible; starting at seven digits keeps every id this
/// emulator mints the same width as the ids the real service hands out, and
/// `u64`-sized for the whole life of a process.
pub const FIRST_RESOURCE_ID: u64 = 1_000_000;

/// How far above [`FIRST_RESOURCE_ID`] the run counter starts.
///
/// Workflow ids and run ids are separate namespaces on the real service too, so an
/// overlap would still be correct — but a cross-type mix-up (a workflow id handed to
/// `GetRun`) would then resolve to an unrelated record instead of answering "not found",
/// masking the mistake. Offsetting the run counter makes that mix-up visible.
pub const RUN_ID_OFFSET: u64 = 500_000;

/// How far above [`FIRST_RESOURCE_ID`] the run group counter starts, for the same
/// reason [`RUN_ID_OFFSET`] exists: a run group id handed to `GetRun` (or a run id
/// handed to `GetRunGroup`) resolves to nothing and is answered "not found" instead of
/// reaching an unrelated record. The offsets are far enough apart that no store
/// can reach another's range under the caps below.
pub const RUN_GROUP_ID_OFFSET: u64 = 250_000;

/// How far above [`FIRST_RESOURCE_ID`] the run cache counter starts, for the same
/// reason, and far enough from [`RUN_ID_OFFSET`] and [`RUN_GROUP_ID_OFFSET`] that no
/// cache id can reach a run's or a run group's range under the caps below.
pub const RUN_CACHE_ID_OFFSET: u64 = 750_000;

/// `RunCacheStatus` a run cache carries once it exists.
///
/// From the modelled `RunCacheStatus` enum (ACTIVE | DELETED | FAILED), none of whose
/// values is a transient "being created" state: a cache the service answers a create
/// with is already usable, so nothing is owed to the clock here.
pub const RUN_CACHE_STATUS_ACTIVE: &str = "ACTIVE";

/// `CacheBehavior` applied when `CreateRunCache` omits `cacheBehavior`.
///
/// From that member's own documentation: "If you don't specify a value, the default
/// behavior is CACHE_ON_FAILURE."
pub const DEFAULT_CACHE_BEHAVIOR: &str = "CACHE_ON_FAILURE";

/// `WorkflowStatus` a freshly created private workflow carries, from the modelled
/// `WorkflowStatus` enum.
pub const WORKFLOW_STATUS_CREATING: &str = "CREATING";
/// The `WorkflowStatus` the clock owes a `CREATING` workflow.
pub const WORKFLOW_STATUS_ACTIVE: &str = "ACTIVE";

/// The first id the sequence-store counter mints.
///
/// The modelled `SequenceStoreId` is `^[0-9]+$` with length 10..36, so — unlike a
/// workflow or run id — a sequence store id must be at least TEN digits and the
/// counter above cannot serve it. Starting here makes every id this emulator mints
/// exactly ten digits wide, inside that length and inside `u64` for the whole life of
/// a process.
pub const FIRST_SEQUENCE_STORE_ID: u64 = 1_000_000_000;

/// How far above [`FIRST_SEQUENCE_STORE_ID`] the multipart-read-set-upload counter
/// starts.
///
/// The modelled `UploadId` is the same `^[0-9]+$` with length 10..36 as a
/// `SequenceStoreId`, so every id this mints is ten digits wide too. Offsetting it keeps
/// an upload id and a store id in separate ranges, so a cross-type mix-up (a store id
/// handed to an upload operation) resolves to nothing and is answered "not found"
/// instead of reaching an unrelated record.
pub const UPLOAD_ID_OFFSET: u64 = 1_000_000_000;

/// How far above [`FIRST_SEQUENCE_STORE_ID`] the read-set counter starts.
///
/// The modelled `ReadSetId` is the same `^[0-9]+$` with length 10..36 as a
/// `SequenceStoreId` and an `UploadId`, so every id this mints is ten digits wide too.
/// Offsetting it past [`UPLOAD_ID_OFFSET`] keeps a read set id in its own range, so a
/// cross-type mix-up (an upload id handed to a read set operation) resolves to nothing
/// and is answered "not found" instead of reaching an unrelated record.
pub const READ_SET_ID_OFFSET: u64 = 2_000_000_000;

/// `SequenceStoreStatus` a freshly created sequence store carries, from the modelled
/// `SequenceStoreStatus` enum.
pub const SEQUENCE_STORE_STATUS_CREATING: &str = "CREATING";
/// The `SequenceStoreStatus` the clock owes a `CREATING` sequence store, from the same
/// enum.
pub const SEQUENCE_STORE_STATUS_ACTIVE: &str = "ACTIVE";

/// `ReadSetStatus` a read set carries from the moment the upload that formed it was
/// completed, from the modelled `ReadSetStatus` enum: the service has taken the uploaded
/// parts and is still turning them into the read set's files.
pub const READ_SET_STATUS_PROCESSING_UPLOAD: &str = "PROCESSING_UPLOAD";

/// The `ReadSetStatus` the clock owes a `PROCESSING_UPLOAD` read set, from the same
/// enum: the status a read set a caller can read back holds.
pub const READ_SET_STATUS_ACTIVE: &str = "ACTIVE";

/// The `ReadSetStatus` of a read set that has been deleted, from the same enum. A read
/// set in it holds nothing a store still needs to keep.
pub const READ_SET_STATUS_DELETED: &str = "DELETED";

/// The `CreationType` of a read set formed by completing a multipart read set upload,
/// from the modelled `CreationType` enum (IMPORT | UPLOAD): the caller uploaded it
/// rather than importing it from an object store.
pub const CREATION_TYPE_UPLOAD: &str = "UPLOAD";

/// The `ETagAlgorithmFamily` applied when `CreateSequenceStore` omits
/// `eTagAlgorithmFamily`.
///
/// From that member's own documentation: "The ETag algorithm family to use for ingested
/// read sets. The default value is MD5up."
pub const DEFAULT_E_TAG_ALGORITHM_FAMILY: &str = "MD5up";

/// `WorkflowType` of every workflow `CreateWorkflow` mints: the model's own
/// `PRIVATE`, the type that operation's documentation describes ("Creates a private
/// workflow").
pub const WORKFLOW_TYPE_PRIVATE: &str = "PRIVATE";

/// The `RunStatus` sequence a run walks, from the modelled `RunStatus` enum, in the
/// order the clock advances it through them. The last entry is terminal: no further
/// transition is owed once it is reached.
///
/// It ends in `FAILED`, not in `COMPLETED`. This emulator is a control plane: it
/// records a run faithfully — its workflow, role, parameters, storage, timestamps and
/// output location — but it runs no WDL, Nextflow or CWL engine and writes no output
/// object, so no run here can ever have succeeded. Reporting `COMPLETED` would tell a
/// caller a workflow produced outputs that do not exist, so the progression ends in
/// the modelled failure status carrying [`RUN_NOT_EXECUTED_FAILURE_REASON`] and
/// [`RUN_NOT_EXECUTED_STATUS_MESSAGE`], which say exactly why.
pub const RUN_STATUS_SEQUENCE: &[&str] = &["PENDING", "STARTING", "RUNNING", "FAILED"];

/// The `RunStatus` at which a run's `startTime` is stamped: the step at which the real
/// service considers the run started. From the same modelled `RunStatus` enum, and the
/// entry of [`RUN_STATUS_SEQUENCE`] the stamp belongs to, so the sequence and the
/// timestamp rule cannot drift apart.
pub const RUN_STATUS_RUNNING: &str = "RUNNING";

/// `RunStatus$failureReason` for a run no workflow engine executed.
///
/// Inside the modelled `RunFailureReason` length (1..64).
pub const RUN_NOT_EXECUTED_FAILURE_REASON: &str =
    "No workflow engine is emulated, so the run never executed.";

/// `RunStatus$statusMessage` for the same run, saying what the emulator did and did not
/// do.
pub const RUN_NOT_EXECUTED_STATUS_MESSAGE: &str =
    "This emulator records runs but executes no WDL, Nextflow or CWL workflow engine, \
     so the run ended without running and wrote no outputs.";

/// `RunStatus` values a run can hold once nothing further is owed to it.
///
/// From the modelled `RunStatus` enum: every value that is not one of the three
/// in-flight states (`PENDING`, `STARTING`, `RUNNING`) or the transient `STOPPING`.
pub const TERMINAL_RUN_STATUSES: &[&str] = &["COMPLETED", "DELETED", "CANCELLED", "FAILED"];

/// The `RunStatus` values `DeleteRun` admits.
///
/// From that operation's own documentation: "You can only delete a run that has reached
/// a COMPLETED, FAILED, or CANCELLED stage." A run in any other status is refused with
/// the modelled `ConflictException`.
pub const DELETABLE_RUN_STATUSES: &[&str] = &["COMPLETED", "FAILED", "CANCELLED"];

/// Whether `status` is one `DeleteRun` admits, from [`DELETABLE_RUN_STATUSES`].
pub fn is_deletable_run_status(status: &str) -> bool {
    DELETABLE_RUN_STATUSES.contains(&status)
}

/// What an operation answers when the workflow a request names is not one this emulator
/// holds — including an operation on that workflow's VERSIONS, where there is no version
/// collection to reach when its workflow is absent.
///
/// The variable part is the workflow id the REQUEST carried; the sentence states the kind
/// of resource and nothing else of the request, so one wording serves every operation
/// that resolves a `workflowId`.
pub fn workflow_not_found_message(workflow_id: &str) -> String {
    format!("Workflow {workflow_id} not found")
}

/// What an operation that resolves a modelled `RunGroupId` answers when no run group this
/// emulator holds carries it.
///
/// The variable part is the run group id the REQUEST carried; the sentence names the kind
/// of resource and nothing else of the request, the same fixed form
/// [`workflow_not_found_message`] states for the other numeric-id resource of this API, so
/// one wording serves every operation that resolves a `runGroupId` (`GetRunGroup`,
/// `UpdateRunGroup`, `DeleteRunGroup`, and the run operations that place a run in a group).
pub fn run_group_not_found_message(run_group_id: &str) -> String {
    format!("Run group {run_group_id} not found")
}

/// The same fixed form, for the modelled `RunId` every run operation resolves (`GetRun`,
/// `DeleteRun`, and the `StartRun` that duplicates a run).
pub fn run_not_found_message(run_id: &str) -> String {
    format!("Run {run_id} not found")
}

/// The same fixed form again, for the modelled `RunCacheId` a run cache operation resolves
/// (`GetRunCache`, and the `StartRun` that binds a run to a cache).
pub fn run_cache_not_found_message(run_cache_id: &str) -> String {
    format!("Run cache {run_cache_id} not found")
}

/// What `GetWorkflowVersion` answers when the workflow version the request names is
/// not one this emulator holds — either the workflow or the version is missing, which
/// are the same answer: the pair was not found.
///
/// The variable parts are the workflow id and the version name the REQUEST carried.
/// `DeleteWorkflowVersion` words the same condition its own way, so each operation
/// carries its own wording rather than sharing one.
pub fn get_workflow_version_not_found_message(workflow_id: &str, version_name: &str) -> String {
    format!("Workflow {workflow_id} with version name {version_name} not found")
}

/// The same condition as [`get_workflow_version_not_found_message`], as
/// `DeleteWorkflowVersion` words it.
pub fn delete_workflow_version_not_found_message(
    workflow_id: &str,
    version_name: &str,
) -> String {
    format!("Workflow {workflow_id} version {version_name} not found")
}

/// What a share operation answers when the share the request names is not one it can act
/// on — either no share this emulator holds carries that id, or (for `DeleteShare`) the
/// share's delete has already been asked for, which is what the recorded service's choice
/// of the modelled `ResourceNotFoundException` for that retry says: to a delete, a spent
/// share is simply not there, even though `GetShare` still reports its terminal status.
///
/// The service words this refusal as FIXED text that echoes no id back: the sentence names
/// the kind of resource that was requested and nothing of the request. Stated once because
/// every share operation answering that condition words it the same way.
pub const SHARE_NOT_FOUND_MESSAGE: &str = "Requested share not found";

/// What every operation that resolves a modelled `SequenceStoreId` answers when this
/// emulator holds no store for it.
///
/// The service words this refusal as FIXED text for the resource class, echoing no id
/// back, so the one sentence stands for every operation that looks a sequence store up
/// (`GetSequenceStore`, `DeleteSequenceStore`, and an operation reaching a store through
/// its path).
pub const SEQUENCE_STORE_NOT_FOUND_MESSAGE: &str = "The specified sequence store does not exist.";

/// How this service states that no multipart read set upload the request could reach
/// carries the `uploadId` it named: the same wording class as
/// [`SEQUENCE_STORE_NOT_FOUND_MESSAGE`], for the other resource the upload operations
/// resolve. An upload belongs to the store it was initiated against, so an upload id
/// presented under another store is "does not exist" too.
pub const MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE: &str =
    "The specified upload does not exist.";

/// The same wording class again, for the read set an operation resolves inside a
/// sequence store. A read set belongs to the store it was created in, so a read set id
/// presented under another store is "does not exist" too, and so is one whose read set a
/// delete has already taken away.
pub const READ_SET_NOT_FOUND_MESSAGE: &str = "The specified readset does not exist.";

/// How this service states that the read set a data read reached holds no such file or no
/// such part — a part number past the parts the file was uploaded in, or a file the read
/// set does not hold at all.
///
/// The storage data plane words this refusal as the error code itself: fixed text that
/// carries nothing of the request, which is the wording the recorded refusal of an
/// out-of-range part number carries.
pub const READ_SET_PART_NOT_FOUND_MESSAGE: &str = "ResourceNotFoundException";

/// What a delete answers when the resource it names still has resources of its own
/// hanging off it — a sequence store that still holds read sets, say.
///
/// The service words this refusal as FIXED text for the whole condition: the sentence
/// states that the resource is associated with others and echoes nothing of the request,
/// so one wording serves every delete that is blocked by what the resource still holds.
pub const RESOURCE_HAS_DEPENDENTS_MESSAGE: &str =
    "The specified resource is associated with other resources and cannot be deleted.";

/// What an operation that resolves a CALLER-SUPPLIED resource ARN answers when the ARN
/// names no resource this emulator holds.
///
/// The variable part is the resource ARN the REQUEST carried. One sentence for the whole
/// condition, so `CreateShare` (the resource it is offered) and the tagging operations
/// (the resource whose tags were addressed) state it identically.
pub fn resource_arn_not_found_message(resource_arn: &str) -> String {
    format!("Resource {resource_arn} not found")
}

/// What a tagging call answers when the tags it would add take the resource past
/// [`MAX_TAGS`].
///
/// The variable parts are how many tags the resource would then carry and the ceiling
/// itself, so the sentence states the quota that was exceeded and nothing else of the
/// request.
pub fn tags_past_quota_message(would_carry: usize) -> String {
    format!(
        "tagging this resource would leave it carrying {would_carry} tags, more than the \
         {MAX_TAGS} tags one resource may have"
    )
}

/// What `DeleteWorkflow` answers when the workflow still holds versions.
///
/// Fixed text: the refusal names no resource, because the rule is about the workflow in
/// the request's own path.
pub const WORKFLOW_HAS_VERSIONS_MESSAGE: &str = "Cannot delete the requested workflow, as it \
     has one or more versions. Delete the versions and try again.";

/// The modelled `StorageType` value that allocates a fixed amount of storage.
///
/// Source: `smithy_query(query="shape",shape="StorageType")` -> STATIC | DYNAMIC.
pub const STORAGE_TYPE_STATIC: &str = "STATIC";

/// `StorageType` applied when `StartRun` omits `storageType`.
///
/// From that member's own documentation: "By default, the run uses `STATIC` storage
/// type, which allocates a fixed amount of storage."
pub const DEFAULT_RUN_STORAGE_TYPE: &str = STORAGE_TYPE_STATIC;

/// The STATIC `storageCapacity` (in gibibytes) a run gets when nothing names one.
///
/// From the `storageCapacity` member's own documentation: "The default run storage
/// capacity is 1200 GiB." The member is optional, so a run that resolves to the STATIC
/// storage type and carries no capacity from the request, the run it duplicates or the
/// workflow's default is given this one rather than refused. Inside the modelled
/// `@range(0..100000)`, so a defaulted run is never out of range.
pub const DEFAULT_RUN_STORAGE_CAPACITY: i32 = 1200;

/// `WorkflowType` applied when `StartRun` omits `workflowType`.
///
/// From that member's own documentation: "If you are running a `PRIVATE` workflow
/// (default), you do not need to include the workflow type."
pub const DEFAULT_RUN_WORKFLOW_TYPE: &str = WORKFLOW_TYPE_PRIVATE;

/// `RunRetentionMode` applied when `StartRun` omits `retentionMode`.
///
/// From that member's own documentation: "The default value is `RETAIN`."
pub const DEFAULT_RUN_RETENTION_MODE: &str = "RETAIN";

/// The modelled `RunOutputUriPrefixMode` value that adds no prefix beneath `outputUri`.
///
/// Source: `smithy_query(query="shape",shape="RunOutputUriPrefixMode")` ->
/// SERVICE_DEFINED | NONE_WITH_OVERWRITE. That value's own documentation: "Adds no
/// prefix and overwrites existing objects at the run's output keys".
pub const OUTPUT_URI_PREFIX_MODE_NONE_WITH_OVERWRITE: &str = "NONE_WITH_OVERWRITE";

/// `RunOutputUriPrefixMode` applied when `StartRun` omits `outputUriPrefixMode`.
///
/// From that member's own documentation: "If you don't specify a value, the service uses
/// `SERVICE_DEFINED`, which adds a run-specific prefix." `GetRun`'s own member says the
/// same from the reporting side, so a run that named no mode still reports this one.
pub const DEFAULT_RUN_OUTPUT_URI_PREFIX_MODE: &str = "SERVICE_DEFINED";

/// Whether a run may read inputs from Requester Pays buckets when `StartRun` omits
/// `requesterPaysEnabled`.
///
/// From that member's own documentation: "The default is `false`."
pub const DEFAULT_RUN_REQUESTER_PAYS_ENABLED: bool = false;

/// Why `StartRun` refuses a request that carries both an inline parameters document and
/// a parameters file URI.
///
/// Fixed text: the `parametersUri` member's own documentation states the rule —
/// "Specify either `parameters` or `parametersUri`, not both." — and the refusal names
/// neither value, because the rule is about which members the request set.
pub const RUN_PARAMETERS_AND_URI_MESSAGE: &str =
    "Specify either parameters or parametersUri, not both.";

/// `ShareStatus` a share carries from the moment `CreateShare` answers.
///
/// From the modelled `ShareStatus` enum's documentation for this value: "The share has
/// been created but is not yet active". `CreateShare` makes an OFFER the principal
/// subscriber accepts, so what moves a share out of this status is a caller's
/// `AcceptShare` and never the clock: a share stays here until someone acts on it.
pub const SHARE_STATUS_PENDING: &str = "PENDING";

/// `ShareStatus` a share carries while the delete it was asked for is in flight: "The
/// share is being deleted".
pub const SHARE_STATUS_DELETING: &str = "DELETING";

/// `ShareStatus` a deleted share settles on: "The share has been deleted".
pub const SHARE_STATUS_DELETED: &str = "DELETED";

/// The `ShareStatus` values in which a share no longer offers anything: the delete it
/// was asked for is in flight or already complete, so the resource is free to be offered
/// to that subscriber again.
pub const SPENT_SHARE_STATUSES: &[&str] = &[SHARE_STATUS_DELETING, SHARE_STATUS_DELETED];

/// The resource TYPES that can be shared across accounts, as the resource-type token a
/// resource ARN carries (`service_common::arn::Arn::resource_type`).
///
/// From `CreateShare`'s own documentation: "The following resources support
/// cross-account sharing: HealthOmics variant stores HealthOmics annotation stores
/// Private workflows".
pub const SHAREABLE_RESOURCE_TYPES: &[&str] = &[
    VARIANT_STORE_RESOURCE_TYPE,
    ANNOTATION_STORE_RESOURCE_TYPE,
    WORKFLOW_RESOURCE_TYPE,
];

/// The resource type a workflow ARN names: what [`WORKFLOW_ARN_RESOURCE_PREFIX`] carries
/// before its separator. Named on its own because a CALLER-supplied ARN is read by
/// resource type through `service_common::arn`, not by prefix.
pub const WORKFLOW_RESOURCE_TYPE: &str = "workflow";

/// The resource type a variant store ARN names.
pub const VARIANT_STORE_RESOURCE_TYPE: &str = "variantStore";

/// The resource type an annotation store ARN names.
pub const ANNOTATION_STORE_RESOURCE_TYPE: &str = "annotationStore";

/// The resource type a run ARN names: what [`RUN_ARN_RESOURCE_PREFIX`] carries before its
/// separator, named on its own for the same reason [`WORKFLOW_RESOURCE_TYPE`] is — a
/// CALLER-supplied ARN is read by resource type, not by prefix.
pub const RUN_RESOURCE_TYPE: &str = "run";

/// The resource type a run group ARN names.
pub const RUN_GROUP_RESOURCE_TYPE: &str = "runGroup";

/// The resource type a run cache ARN names.
pub const RUN_CACHE_RESOURCE_TYPE: &str = "runCache";

/// The resource type a sequence store ARN names.
pub const SEQUENCE_STORE_RESOURCE_TYPE: &str = "sequenceStore";

/// How long after the call that starts it a status transition this emulator owes to
/// the clock becomes due.
///
/// This is the emulator's own chosen span, NOT a duration AWS publishes: HealthOmics
/// documents no time for `CREATING` -> `ACTIVE` or for a run's progression, and a real
/// run takes minutes to hours. It is deliberately non-zero so a caller that reads
/// straight after the call still observes the transient status and the state rules
/// that go with it.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// DeleteWorkflow/DeleteRun, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked. HealthOmics publishes no public quota
// for either resource, so these are this emulator's resource guards, set well above
// any realistic use. Every other caller-sized member is already bounded by the
// model and refused by the centralized walk before a handler runs:
// `parameterTemplate` (1..2000 entries), `definitionUri` (1..256), `outputUri`
// (1..750), `name` (1..128) and so on.
// ---------------------------------------------------------------------------

/// The most workflows this emulator will hold. Emulator resource guard, not an AWS
/// quota. A create past it is refused with the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_WORKFLOWS: usize = 500;

/// The most runs this emulator will hold. Emulator resource guard, not an AWS quota.
pub const MAX_RUNS: usize = 1000;

/// The most versions this emulator will hold for one workflow. Emulator resource
/// guard, not an AWS quota: each version is created by a caller and reclaimed only by
/// an explicit delete, so the per-workflow collection carries a cap too. A create past
/// it is refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_WORKFLOW_VERSIONS: usize = 100;

/// The most sequence stores this emulator will hold. Emulator resource guard, not an
/// AWS quota — `CreateSequenceStore` documents that "You can create multiple sequence
/// stores per region per account" and publishes no ceiling. A create past it is refused
/// with the modelled `ServiceQuotaExceededException`.
pub const MAX_SEQUENCE_STORES: usize = 500;

/// The most run caches this emulator will hold. Emulator resource guard, not an AWS
/// quota — HealthOmics publishes no run cache ceiling. A create past it is refused with
/// the modelled `ServiceQuotaExceededException`.
pub const MAX_RUN_CACHES: usize = 500;

/// The most run groups this emulator will hold. Emulator resource guard, not an AWS
/// quota. A create past it is refused with the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_RUN_GROUPS: usize = 500;

/// The most resource shares this emulator will hold. Emulator resource guard, not an AWS
/// quota — HealthOmics publishes no share ceiling. A share is created by a caller and a
/// deleted one is kept so its terminal status stays readable, so this growth site carries
/// a cap like every other; a create past it is refused with the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_SHARES: usize = 500;

/// The most multipart read set uploads this emulator will hold for ONE sequence store.
///
/// Emulator resource guard, not an AWS quota: an upload is initiated by a caller and
/// reclaimed only by an explicit abort or completion, so the per-store collection
/// carries a cap like every other growth site here. A create past it is refused with the
/// modelled `ServiceQuotaExceededException`.
pub const MAX_MULTIPART_READ_SET_UPLOADS: usize = 100;

/// The most part bytes this emulator will hold for ONE multipart read set upload.
///
/// Emulator resource guard, not an AWS quota: a part's bytes are kept so the upload is
/// backed by the content the caller sent, and that content lives in this process's
/// memory, so the per-upload collection carries a cap like every other growth site
/// here. It is far below the 5 GiB one part may be under the modelled `partSize` range,
/// so a part past it is refused with the modelled `ServiceQuotaExceededException`
/// instead of being held.
pub const MAX_READ_SET_UPLOAD_PART_BYTES: usize = 64 * 1024 * 1024;

/// The most read sets this emulator will hold for ONE sequence store.
///
/// Emulator resource guard, not an AWS quota: a read set is created by a caller
/// completing an upload and reclaimed only by an explicit delete, so the per-store
/// collection carries a cap like every other growth site here. A completion past it is
/// refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_READ_SETS: usize = 100;

/// The most read set content bytes this emulator will hold for ONE sequence store.
///
/// Emulator resource guard, for the same reason [`MAX_READ_SET_UPLOAD_PART_BYTES`]
/// exists: a read set's files are kept as the bytes the caller uploaded, and that content
/// lives in this process's memory. A completion that would take the store past it is
/// refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_READ_SET_BYTES: usize = 256 * 1024 * 1024;

/// The most tags one workflow or run may carry.
///
/// From the `tags` member's own documentation on `CreateWorkflow`: "You can define
/// up to 50 tags for the workflow."
pub const MAX_TAGS: usize = 50;

/// The most idempotency tokens this emulator remembers per operation.
///
/// Emulator resource guard: the map is keyed by caller-supplied `requestId`, so it
/// is capped at one entry per resource either store can hold, and a token whose
/// resource is gone is reclaimed with it.
pub const MAX_IDEMPOTENCY_TOKENS: usize =
    MAX_WORKFLOWS + MAX_RUNS + MAX_SEQUENCE_STORES + MAX_RUN_CACHES + MAX_RUN_GROUPS;

/// One stored private workflow: every member `GetWorkflow` reports.
#[derive(Debug, Clone, Default)]
pub struct Workflow {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub uuid: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub engine: Option<String>,
    pub main: Option<String>,
    /// The S3 URI the definition was read from, when the create named one.
    pub definition: Option<String>,
    /// Content digest of the definition the create supplied.
    pub digest: Option<String>,
    pub parameter_template: BTreeMap<String, WorkflowParameter>,
    pub storage_capacity: Option<i32>,
    pub storage_type: Option<String>,
    pub accelerators: Option<String>,
    pub readme_path: Option<String>,
    pub container_registry_map: Option<ContainerRegistryMap>,
    /// The code repository the create sourced the definition from, when it named
    /// one; reported back as `definitionRepositoryDetails`.
    pub definition_repository: Option<DefinitionRepository>,
    /// Always [`WORKFLOW_TYPE_PRIVATE`] for a workflow minted by `CreateWorkflow`.
    pub r#type: String,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time_ms: i64,
    /// When the transition this record owes the clock becomes due, in epoch millis;
    /// `None` once it has settled.
    pub status_due_at_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
    /// Every version `CreateWorkflowVersion` minted for this workflow, keyed by the
    /// modelled `WorkflowVersionName` — which that operation documents as unique for
    /// the workflow and unchangeable afterwards, so the name IS the key and the map
    /// holds each version exactly once.
    pub versions: BTreeMap<String, WorkflowVersion>,
}

impl Workflow {
    /// Apply every status change the clock already owed this workflow, or any version
    /// it holds, by `now_ms`.
    ///
    /// Reporting-only: it materialises a change that was due at the same instant
    /// whether or not anybody called, and the status it lands on does not depend on
    /// when the read happened.
    pub fn settle(&mut self, now_ms: i64) {
        settle_creating(
            &mut self.status,
            &mut self.status_due_at_ms,
            now_ms,
            WORKFLOW_STATUS_ACTIVE,
        );
        for version in self.versions.values_mut() {
            version.settle(now_ms);
        }
    }

    /// `parameterTemplate` as the wire shape wants it: omitted when the workflow
    /// carries no parameter at all.
    pub fn parameter_template_output(&self) -> Option<HashMap<String, WorkflowParameter>> {
        parameter_template_output(&self.parameter_template)
    }

    /// `tags` as the wire shape wants it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }

    /// `definitionRepositoryDetails` for the repository the create named.
    pub fn definition_repository_details(&self) -> Option<DefinitionRepositoryDetails> {
        definition_repository_details(self.definition_repository.as_ref())
    }
}

/// One stored workflow version: every member `GetWorkflowVersion` reports.
///
/// A version carries its own configuration rather than the workflow's:
/// `CreateWorkflowVersion` documents that "It doesn't inherit any configuration values
/// from the workflow", so no member here is filled in from the parent record.
#[derive(Debug, Clone, Default)]
pub struct WorkflowVersion {
    /// The modelled `WorkflowVersionName`: this record's key inside its workflow.
    pub version_name: String,
    pub arn: String,
    pub uuid: String,
    pub description: Option<String>,
    pub engine: Option<String>,
    pub main: Option<String>,
    /// The S3 URI the definition was read from, when the create named one.
    pub definition: Option<String>,
    /// Content digest of the definition the create supplied.
    pub digest: Option<String>,
    pub parameter_template: BTreeMap<String, WorkflowParameter>,
    pub storage_capacity: Option<i32>,
    pub storage_type: Option<String>,
    pub accelerators: Option<String>,
    pub readme_path: Option<String>,
    pub container_registry_map: Option<ContainerRegistryMap>,
    pub definition_repository: Option<DefinitionRepository>,
    pub workflow_bucket_owner_id: Option<String>,
    /// The `WorkflowType` of the workflow this version belongs to.
    pub r#type: String,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time_ms: i64,
    /// When the transition this record owes the clock becomes due, in epoch millis;
    /// `None` once it has settled.
    pub status_due_at_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
    /// The `@idempotencyToken` of the request that created this version, so a replay
    /// of that token answers with this record instead of minting a second one. The
    /// token's namespace is the workflow's own version collection, which
    /// [`MAX_WORKFLOW_VERSIONS`] already bounds, so no separate token map grows here.
    pub request_id: String,
}

impl WorkflowVersion {
    /// Apply every status change the clock already owed this version by `now_ms`,
    /// under the same rule a workflow settles by.
    pub fn settle(&mut self, now_ms: i64) {
        settle_creating(
            &mut self.status,
            &mut self.status_due_at_ms,
            now_ms,
            WORKFLOW_STATUS_ACTIVE,
        );
    }

    /// `parameterTemplate` as the wire shape wants it: omitted when the version
    /// carries no parameter at all.
    pub fn parameter_template_output(&self) -> Option<HashMap<String, WorkflowParameter>> {
        parameter_template_output(&self.parameter_template)
    }

    /// `tags` as the wire shape wants it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }

    /// `definitionRepositoryDetails` for the repository the create named.
    pub fn definition_repository_details(&self) -> Option<DefinitionRepositoryDetails> {
        definition_repository_details(self.definition_repository.as_ref())
    }
}

/// Apply the `CREATING` -> active move the clock owes a freshly created record: one
/// rule, stated once, so every record that owes one settles identically. `active` is the
/// status that record's own modelled status enum calls active, so a workflow, a workflow
/// version and a sequence store each land on a value its own enum admits.
fn settle_creating(
    status: &mut String,
    status_due_at_ms: &mut Option<i64>,
    now_ms: i64,
    active: &str,
) {
    if status_due_at_ms.is_some_and(|due| due <= now_ms) {
        *status = active.to_string();
        *status_due_at_ms = None;
    }
}

/// One stored sequence store: every member `GetSequenceStore` reports.
#[derive(Debug, Clone, Default)]
pub struct SequenceStore {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The server-side encryption settings the create named, when it named any.
    pub sse_config: Option<SseConfig>,
    pub fallback_location: Option<String>,
    /// The ETag algorithm family in force, which is
    /// [`DEFAULT_E_TAG_ALGORITHM_FAMILY`] when the create named none.
    pub e_tag_algorithm_family: String,
    /// The tag keys that propagate to the objects of this store's read sets, exactly as
    /// the create listed them; `None` when it listed none.
    pub propagated_set_level_tags: Option<Vec<String>>,
    /// `s3AccessConfig$accessLogLocation` the create named, reported back as part of
    /// `s3Access`.
    pub access_log_location: Option<String>,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time_ms: i64,
    /// When the store was last changed, in epoch millis; `None` until something
    /// changes it, since a create is not an update.
    pub update_time_ms: Option<i64>,
    /// When the transition this record owes the clock becomes due, in epoch millis;
    /// `None` once it has settled.
    pub status_due_at_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
}

impl SequenceStore {
    /// Apply the `CREATING` -> `ACTIVE` move the clock already owed this store by
    /// `now_ms`, under the same rule every other record settles by.
    pub fn settle(&mut self, now_ms: i64) {
        settle_creating(
            &mut self.status,
            &mut self.status_due_at_ms,
            now_ms,
            SEQUENCE_STORE_STATUS_ACTIVE,
        );
    }

    /// `s3Access` as the wire shapes want it.
    ///
    /// Only the access-log location the create named is reported. `s3Uri` and
    /// `s3AccessPointArn` name an S3 bucket and access point the real service
    /// provisions for the store; this emulator serves no object store and mints no
    /// access point, so there is no location a client could reach and the members are
    /// omitted rather than filled with an address nothing answers on. The structure
    /// itself is omitted when the create named no S3 access configuration.
    pub fn s3_access_output(&self) -> Option<SequenceStoreS3Access> {
        let access_log_location = self.access_log_location.clone()?;
        Some(SequenceStoreS3Access {
            s3_uri: None,
            s3_access_point_arn: None,
            access_log_location: Some(access_log_location),
        })
    }

    /// `tags` as the wire shapes want it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// One initiated multipart read set upload: every member
/// `CreateMultipartReadSetUpload` answers with, and the record the parts uploaded for
/// that upload id are collected against.
///
/// The model gives an upload no status member at all, so nothing here is owed to the
/// clock: an upload is usable for `UploadReadSetPart` from the moment its create
/// answered.
#[derive(Debug, Clone, Default)]
pub struct MultipartReadSetUpload {
    /// The decimal numeric id: this record's store key, rendered.
    pub upload_id: String,
    /// The sequence store this upload writes into, so one listing can be narrowed to
    /// the store the request names and an upload id cannot be reached through another
    /// store.
    pub sequence_store_id: String,
    pub source_file_type: String,
    pub subject_id: String,
    pub sample_id: String,
    pub generated_from: Option<String>,
    /// The reference the create named, when it named one. HealthOmics needs a reference
    /// genome only for the aligned source formats, so an upload of an unaligned one
    /// carries none.
    pub reference_arn: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub creation_time_ms: i64,
    pub tags: BTreeMap<String, String>,
    /// The `clientToken` of the request that initiated this upload, so a replay of that
    /// token answers with this record instead of initiating a second upload — which is
    /// what that member documents ("An idempotency token that can be used to avoid
    /// triggering multiple multipart uploads"). The token's namespace is the store's own
    /// upload collection, which [`MAX_MULTIPART_READ_SET_UPLOADS`] already bounds, so no
    /// separate token map grows here.
    pub client_token: Option<String>,
    /// The parts uploaded into this upload, keyed by the pair that identifies a part
    /// within it: its source file and its part number. A second upload of a pair already
    /// held REPLACES the part, which is what `UploadReadSetPart` documents ("When you a
    /// upload a read set part with a part number that already exists, the new part
    /// replaces the existing one"), and the key order is source then ascending part
    /// number, so one source's parts are listed in part-number order and a page token can
    /// name the part number the next page resumes at.
    pub parts: BTreeMap<(String, i32), ReadSetUploadPart>,
}

impl MultipartReadSetUpload {
    /// `tags` as the wire shapes want it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }

    /// Every part uploaded from `part_source`, in ascending part-number order.
    pub fn parts_from<'a>(
        &'a self,
        part_source: &'a str,
    ) -> impl Iterator<Item = &'a ReadSetUploadPart> + 'a {
        self.parts
            .iter()
            .filter(move |((source, _), _)| source == part_source)
            .map(|(_, part)| part)
    }

    /// The bytes every part of this upload holds together.
    pub fn held_bytes(&self) -> usize {
        self.parts.values().map(|part| part.payload.len()).sum()
    }
}

/// One part uploaded into a multipart read set upload: every member
/// `ListReadSetUploadParts` reports for it, plus the bytes the caller sent.
///
/// The content is kept, not just measured: the part's size and checksum are read off
/// these bytes, and the read set a completed upload forms is the content of its parts.
#[derive(Debug, Clone, Default)]
pub struct ReadSetUploadPart {
    pub part_number: i32,
    pub part_source: String,
    /// The part's content, exactly as the request carried it.
    pub payload: Vec<u8>,
    /// The digest of [`ReadSetUploadPart::payload`], from [`read_set_part_checksum`].
    pub checksum: String,
    /// When this part number was FIRST uploaded, in epoch millis: a part that replaces
    /// an earlier one keeps the instant the first upload of that number created.
    pub creation_time_ms: i64,
    /// When this part was last uploaded, in epoch millis — the same instant as
    /// `creation_time_ms` until a later upload replaces it.
    pub last_updated_time_ms: i64,
}

impl ReadSetUploadPart {
    /// The part's size, as the modelled `partSize` (a `Long`) reports it: the length of
    /// the content this part holds.
    pub fn part_size(&self) -> i64 {
        self.payload.len() as i64
    }
}

/// One read set held by a sequence store: every member `GetReadSetMetadata` reports,
/// plus the content of each of its files.
///
/// A read set is what `CompleteMultipartReadSetUpload` forms out of an initiated upload
/// and the parts uploaded into it: the descriptive members are the ones that upload
/// carried, and the files are the parts' content, so nothing here is invented for a
/// member the upload never named.
#[derive(Debug, Clone, Default)]
pub struct ReadSet {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    /// The sequence store that holds this read set, so one listing can be narrowed to the
    /// store the request names and a read set id cannot be reached through another store.
    pub sequence_store_id: String,
    pub subject_id: String,
    pub sample_id: String,
    pub name: String,
    pub description: Option<String>,
    /// Where the sequence originated, when the upload named it; reported inside
    /// `sequenceInformation`.
    pub generated_from: Option<String>,
    pub reference_arn: Option<String>,
    /// The modelled `FileType` of the uploaded source files.
    pub file_type: String,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time_ms: i64,
    /// When the transition this record owes the clock becomes due, in epoch millis;
    /// `None` once it has settled.
    pub status_due_at_ms: Option<i64>,
    /// How this read set came to exist, from the modelled `CreationType` enum.
    pub creation_type: String,
    /// The IMPORT job that created it, when one did; `None` for a read set a direct
    /// upload formed, which no import job produced.
    pub creation_job_id: Option<String>,
    /// The modelled `ETagAlgorithm` this read set's files were ingested under — the
    /// format of its data joined to the ETag algorithm family of the store that holds it
    /// — when that pair names a modelled value.
    pub e_tag_algorithm: Option<String>,
    /// How many reads the ingested files hold together, when the format is one whose
    /// records this service reads.
    pub total_read_count: Option<i64>,
    /// How many bases those reads hold together, counted the same way.
    pub total_base_count: Option<i64>,
    /// The alignment setting the source format implies, from [`alignment`].
    pub alignment: Option<String>,
    /// This read set's files, keyed by the modelled `ReadSetPartSource` their parts were
    /// uploaded from (`SOURCE1` / `SOURCE2`), so one source is held exactly once and the
    /// key order is the order `ReadSetFiles` lists them in.
    pub sources: BTreeMap<String, ReadSetSource>,
    /// The tags the upload asked for, which `CreateMultipartReadSetUpload` documents as
    /// "The tags to add to the read set".
    pub tags: BTreeMap<String, String>,
}

impl ReadSet {
    /// Apply the `PROCESSING_UPLOAD` -> `ACTIVE` move the clock already owed this read
    /// set by `now_ms`, under the same rule every other record settles by.
    pub fn settle(&mut self, now_ms: i64) {
        settle_creating(
            &mut self.status,
            &mut self.status_due_at_ms,
            now_ms,
            READ_SET_STATUS_ACTIVE,
        );
    }

    /// The bytes every file of this read set holds together.
    pub fn held_bytes(&self) -> usize {
        self.sources.values().map(|file| file.content.len()).sum()
    }

    /// `files` as the wire shapes want it: one `FileInformation` per source this read set
    /// holds.
    ///
    /// `index` is left out: HealthOmics builds a read set's index from the sequence data
    /// it ingests, and this emulator holds the ingested files without building an index
    /// beside them, so there is no index file to report rather than one standing for a
    /// file nothing produced.
    pub fn files_output(&self) -> Option<ReadSetFiles> {
        let files = ReadSetFiles {
            source1: self.sources.get(READ_SET_PART_SOURCE1).map(file_information),
            source2: self.sources.get(READ_SET_PART_SOURCE2).map(file_information),
            index: None,
        };
        (files.source1.is_some() || files.source2.is_some()).then_some(files)
    }

    /// `etag` as the wire shapes want it: the algorithm this read set's data was ingested
    /// under and the digest of each source file's sequence data under it.
    ///
    /// Both are read off the stored record, where the ingest computed them, so a client
    /// that digests the records it uploaded under the store's own family gets the same
    /// value back. `source2` is omitted for a read set formed from one source file, and
    /// the structure itself when nothing was ingested under a modelled algorithm.
    pub fn e_tag_output(&self) -> Option<ETag> {
        let e_tag = ETag {
            algorithm: self.e_tag_algorithm.clone(),
            source1: self.source_e_tag(READ_SET_PART_SOURCE1),
            source2: self.source_e_tag(READ_SET_PART_SOURCE2),
        };
        (e_tag.algorithm.is_some() || e_tag.source1.is_some() || e_tag.source2.is_some())
            .then_some(e_tag)
    }

    /// The ETag of one source file, when this read set holds that source and the ingest
    /// computed a digest for it.
    fn source_e_tag(&self, part_source: &str) -> Option<String> {
        let e_tag = &self.sources.get(part_source)?.e_tag;
        (!e_tag.is_empty()).then(|| e_tag.clone())
    }

    /// `sequenceInformation` as the wire shapes want it: what the ingest read out of the
    /// sequence records, beside where the upload said the sequence came from.
    ///
    /// Every member is read off the stored record. A count is reported only where the
    /// ingest really counted the records (see [`fastq_counts`]), so a format whose records
    /// this service does not read carries the alignment its format implies and no counts
    /// rather than numbers nothing counted. The structure itself is omitted when there is
    /// nothing to report at all.
    pub fn sequence_information_output(&self) -> Option<SequenceInformation> {
        let sequence_information = SequenceInformation {
            total_read_count: self.total_read_count,
            total_base_count: self.total_base_count,
            generated_from: self.generated_from.clone(),
            alignment: self.alignment.clone(),
        };
        (sequence_information.total_read_count.is_some()
            || sequence_information.total_base_count.is_some()
            || sequence_information.generated_from.is_some()
            || sequence_information.alignment.is_some())
        .then_some(sequence_information)
    }

    /// `tags` as the wire shapes want it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// One file of a read set: the content uploaded from one `ReadSetPartSource`, what
/// `FileInformation` reports for it, and what the ingest derived from its bytes.
#[derive(Debug, Clone, Default)]
pub struct ReadSetSource {
    /// The modelled `ReadSetPartSource` this file's parts were uploaded from: this
    /// record's key inside its read set, rendered.
    pub part_source: String,
    /// The file's content: the parts that formed it, in ascending part-number order.
    pub content: Vec<u8>,
    /// The length of each part that formed this file, in that same ascending part-number
    /// order. The boundaries are kept, not just the total: a read of one part of this file
    /// answers exactly the bytes that part carried, and how many parts the file holds is
    /// read off this list ([`ReadSetSource::total_parts`]).
    pub part_lengths: Vec<usize>,
    /// The size of the part the file was uploaded in, as the modelled `partSize` reports
    /// it: the part size the upload itself is conducted with
    /// ([`READ_SET_UPLOAD_PART_SIZE`]), not the length of any part a caller sent.
    pub part_size: i64,
    /// The MD5 digest of [`ReadSetSource::content`], from [`content_md5`]: the digest of
    /// the file as it was ingested.
    pub md5: String,
    /// The digest of this file's sequence data under the ETag algorithm family of the
    /// store that holds the read set, from [`e_tag_digest`]; empty when that family names
    /// no digest this service computes.
    pub e_tag: String,
    /// The `s3Uri` this file was ingested to, from [`read_set_file_s3_uri`].
    pub s3_uri: String,
}

impl ReadSetSource {
    /// The file's length, as the modelled `contentLength` (a `Long`) reports it.
    pub fn content_length(&self) -> i64 {
        self.content.len() as i64
    }

    /// How many parts formed this file, as the modelled `totalParts` reports it.
    pub fn total_parts(&self) -> i32 {
        self.part_lengths.len() as i32
    }

    /// The content of the part this file holds under the 1-based `part_number`, or `None`
    /// when the file holds no part with that number.
    ///
    /// The bytes are the ones that part carried, sliced at the boundaries the parts that
    /// formed the file left behind, so a part read back is the part as uploaded — this
    /// service stores what it ingested and transforms none of it.
    pub fn part(&self, part_number: i32) -> Option<&[u8]> {
        let index = usize::try_from(part_number.checked_sub(1)?).ok()?;
        let length = *self.part_lengths.get(index)?;
        let start: usize = self.part_lengths.iter().take(index).sum();
        self.content.get(start..start + length)
    }
}

/// `FileInformation` for one read set file, every member read off that stored file.
///
/// `s3Access` carries the location the file was ingested to, which the ingest derived
/// from the store's access point alias and the ids of the store and the read set, and
/// `md5` the digest of the ingested bytes — the member the service reports for a file
/// although this service's on-disk `FileInformation` does not declare it (see
/// `types::read_set_file_information`). Either is omitted for a record that carries
/// none, which is what a read set whose files are gone (a deleted one) holds.
fn file_information(file: &ReadSetSource) -> FileInformation {
    FileInformation {
        total_parts: Some(file.total_parts()),
        part_size: Some(file.part_size),
        content_length: Some(file.content_length()),
        s3_access: (!file.s3_uri.is_empty()).then(|| ReadSetS3Access {
            s3_uri: Some(file.s3_uri.clone()),
        }),
        md5: (!file.md5.is_empty()).then(|| file.md5.clone()),
    }
}

/// The modelled `ReadSetPartSource` of a read set's first file.
/// Source: `smithy_query(query="shape",shape="ReadSetPartSource")` -> SOURCE1 | SOURCE2.
pub const READ_SET_PART_SOURCE1: &str = "SOURCE1";

/// The modelled `ReadSetPartSource` of a read set's second file, which a paired-end
/// upload carries beside the first.
pub const READ_SET_PART_SOURCE2: &str = "SOURCE2";

/// The modelled `ReadSetFile` naming the index HealthOmics builds from a read set's
/// sequence data, which is neither of the source files a caller uploads.
/// Source: `smithy_query(query="shape",shape="ReadSetFile")` -> SOURCE1 | SOURCE2 | INDEX.
pub const READ_SET_FILE_INDEX: &str = "INDEX";

/// The `ReadSetFile` a read set data read serves when the request names none: the read
/// set's first source file, the only file a single-source read set holds.
pub const DEFAULT_READ_SET_FILE: &str = READ_SET_PART_SOURCE1;

/// One stored run cache: every member `GetRunCache` reports.
///
/// A run cache is a reference to a caller-owned S3 location, not a resource this
/// emulator provisions, so every member below is either what the create carried or a
/// value derived from it and the id this emulator minted.
#[derive(Debug, Clone, Default)]
pub struct RunCache {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The S3 location the create named, reported back as `cacheS3Uri`.
    pub cache_s3_uri: String,
    /// The default cache behavior in force, which is [`DEFAULT_CACHE_BEHAVIOR`] when
    /// the create named none.
    pub cache_behavior: String,
    /// The account that owns the cache bucket, which is this emulator's own account
    /// when the create named none.
    pub cache_bucket_owner_id: String,
    pub status: String,
    pub creation_time_ms: i64,
    pub tags: BTreeMap<String, String>,
}

impl RunCache {
    /// `tags` as the wire shapes want it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// A parameter template as the wire shapes want it: omitted when it defines nothing.
fn parameter_template_output(
    parameter_template: &BTreeMap<String, WorkflowParameter>,
) -> Option<HashMap<String, WorkflowParameter>> {
    (!parameter_template.is_empty()).then(|| {
        parameter_template
            .iter()
            .map(|(name, parameter)| (name.clone(), parameter.clone()))
            .collect()
    })
}

/// `definitionRepositoryDetails` for the repository a create named.
///
/// `providerType` and `providerEndpoint` are reported by the real service from the
/// CodeConnections connection the ARN names; this emulator resolves no connection, so
/// it reports only what the request itself carried.
fn definition_repository_details(
    repository: Option<&DefinitionRepository>,
) -> Option<DefinitionRepositoryDetails> {
    let repository = repository?;
    Some(DefinitionRepositoryDetails {
        connection_arn: Some(repository.connection_arn.clone()),
        full_repository_id: Some(repository.full_repository_id.clone()),
        source_reference: repository.source_reference.clone(),
        provider_type: None,
        provider_endpoint: None,
    })
}

/// One stored run: every member `GetRun` reports.
#[derive(Debug, Clone, Default)]
pub struct Run {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub uuid: String,
    pub workflow_id: String,
    pub workflow_type: String,
    pub workflow_uuid: Option<String>,
    pub workflow_name: Option<String>,
    pub workflow_version_name: Option<String>,
    pub networking_mode: Option<String>,
    pub scratch_storage_mode: Option<String>,
    /// The run this one re-ran, when `StartRun` named one.
    pub source_run_id: Option<String>,
    pub role_arn: String,
    pub name: Option<String>,
    pub run_group_id: Option<String>,
    pub cache_id: Option<String>,
    pub cache_behavior: Option<String>,
    pub priority: Option<i32>,
    pub parameters: Option<::aws_smithy_types::Document>,
    /// The S3 URI of the file the run's parameters are in, when `StartRun` named one
    /// instead of an inline `parameters` document. The two are mutually exclusive, so a
    /// run carries at most one of this and `parameters`.
    pub parameters_uri: Option<String>,
    pub engine_settings: Option<::aws_smithy_types::Document>,
    pub storage_capacity: Option<i32>,
    pub storage_type: String,
    pub output_uri: String,
    pub run_output_uri: String,
    /// Which prefixes the service adds beneath [`Run::output_uri`], resolved at
    /// `StartRun` and reported unchanged afterwards: it is what
    /// [`Run::run_output_uri`] was derived under.
    pub output_uri_prefix_mode: String,
    /// Whether this run may read inputs from Requester Pays buckets, as `StartRun` set
    /// it.
    pub requester_pays_enabled: bool,
    pub log_level: Option<String>,
    pub retention_mode: String,
    pub accelerators: Option<String>,
    pub session_policy: Option<String>,
    /// The workflow's definition URI and digest, as of the moment the run started.
    pub definition: Option<String>,
    pub digest: Option<String>,
    pub status: String,
    pub status_message: Option<String>,
    pub failure_reason: Option<String>,
    pub creation_time_ms: i64,
    pub start_time_ms: Option<i64>,
    pub stop_time_ms: Option<i64>,
    /// When the next status in [`RUN_STATUS_SEQUENCE`] becomes due, in epoch millis;
    /// `None` once the run has reached a terminal status.
    pub status_due_at_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
}

impl Run {
    /// Apply every status change the clock already owed this run by `now_ms`.
    ///
    /// Each step's timestamp is the instant it was DUE, not the instant it was
    /// noticed, so `startTime` and `stopTime` are the same values whenever the run
    /// is next read and a caller that waits longer sees no later clock.
    pub fn settle(&mut self, now_ms: i64) {
        while let Some(due) = self.status_due_at_ms.filter(|due| *due <= now_ms) {
            let next = RUN_STATUS_SEQUENCE
                .iter()
                .position(|status| *status == self.status)
                .and_then(|at| RUN_STATUS_SEQUENCE.get(at + 1));
            let Some(next) = next else {
                self.status_due_at_ms = None;
                return;
            };
            self.status = (*next).to_string();
            if self.status == RUN_STATUS_RUNNING {
                self.start_time_ms = Some(due);
            }
            if is_terminal_run_status(&self.status) {
                self.stop_time_ms = Some(due);
                self.status_due_at_ms = None;
                // Nothing executed, so the terminal status says so rather than
                // claiming an outcome no engine produced.
                self.failure_reason = Some(RUN_NOT_EXECUTED_FAILURE_REASON.to_string());
                self.status_message = Some(RUN_NOT_EXECUTED_STATUS_MESSAGE.to_string());
            } else {
                self.status_due_at_ms = Some(due + TRANSITION_DELAY.as_millis() as i64);
            }
        }
    }

    /// `tags` as the wire shape wants it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// One stored run group: every member `GetRunGroup` reports.
///
/// A run group carries no status member in the model — `CreateRunGroup` answers with the
/// group's ARN, id and tags and nothing is provisioned for it — so no record here owes
/// the clock a transition.
#[derive(Debug, Clone, Default)]
pub struct RunGroup {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    /// The caps the group enforces on the runs added to it, exactly as the request set
    /// them; a cap the request omitted is unset, which is the group not capping that
    /// resource at all rather than capping it at zero.
    pub max_cpus: Option<i32>,
    pub max_runs: Option<i32>,
    pub max_duration: Option<i32>,
    pub max_gpus: Option<i32>,
    pub creation_time_ms: i64,
    pub tags: BTreeMap<String, String>,
}

impl RunGroup {
    /// `tags` as the wire shapes want it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// One stored cross-account resource share: every member `GetShare` reports.
#[derive(Debug, Clone, Default)]
pub struct Share {
    /// The id HealthOmics generates for the share; this record's identity on the wire.
    pub id: String,
    /// The ARN of the shared resource, exactly as the create named it.
    pub resource_arn: String,
    /// The id of the shared resource: the part of that ARN's resource path after its
    /// type, so one value is never derived twice from the same ARN.
    pub resource_id: String,
    /// The account being offered access.
    pub principal_subscriber: String,
    /// The account that owns the resource and made the offer.
    pub owner_id: String,
    pub share_name: Option<String>,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time_ms: i64,
    /// When this record last changed, in epoch millis: the create, and then each status
    /// move it makes.
    pub update_time_ms: i64,
    /// When the `DELETING` -> `DELETED` move the clock owes this share becomes due, in
    /// epoch millis; `None` when it owes none.
    pub status_due_at_ms: Option<i64>,
}

impl Share {
    /// Apply the `DELETING` -> `DELETED` move the clock already owed this share by
    /// `now_ms`.
    ///
    /// Reporting-only, like every other `settle` here: the move was due at the same
    /// instant whether or not anybody called, and `updateTime` is stamped with the instant
    /// it fell due rather than with the moment somebody noticed, so when the status landed
    /// does not depend on when the read happened.
    pub fn settle(&mut self, now_ms: i64) {
        if let Some(due) = self.status_due_at_ms.filter(|due| *due <= now_ms) {
            self.status = SHARE_STATUS_DELETED.to_string();
            self.status_due_at_ms = None;
            self.update_time_ms = due;
        }
    }

    /// Whether this share still offers access: it has not been asked to go away, from
    /// [`SPENT_SHARE_STATUSES`].
    pub fn offer_stands(&self) -> bool {
        !SPENT_SHARE_STATUSES.contains(&self.status.as_str())
    }

    /// This record as the `ShareDetails` every reader of a share reports, so each one
    /// answers from the same members.
    pub fn details(&self) -> ShareDetails {
        ShareDetails {
            share_id: Some(self.id.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            resource_id: Some(self.resource_id.clone()),
            principal_subscriber: Some(self.principal_subscriber.clone()),
            owner_id: Some(self.owner_id.clone()),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            share_name: self.share_name.clone(),
            creation_time: Some(timestamp(self.creation_time_ms)),
            update_time: Some(timestamp(self.update_time_ms)),
        }
    }
}

/// Which resource `resource_arn` names when it names one that supports cross-account
/// sharing, as the `(resource type, resource id)` pair — `None` for anything else.
///
/// A caller-supplied ARN is read through the ONE parse,
/// `service_common::arn::parse_of`, against this service's own vendor namespace; the
/// resource type must be one of [`SHAREABLE_RESOURCE_TYPES`] and must name a non-empty
/// id, because a type with no id addresses no resource to offer.
pub fn shareable_resource(resource_arn: &str) -> Option<(&'static str, &str)> {
    let arn = ::service_common::arn::parse_of(resource_arn, ARN_SERVICE)?;
    let resource_type = SHAREABLE_RESOURCE_TYPES
        .iter()
        .find(|shareable| **shareable == arn.resource_type())?;
    let id = ::service_common::arn::resource_id_of_type(resource_arn, resource_type)?;
    (!id.is_empty()).then_some((*resource_type, id))
}

/// Which resource a CALLER-supplied tagging `resourceArn` addresses: the store to look
/// in and the key inside it.
///
/// A version is named inside its workflow's resource path (see
/// [`WORKFLOW_VERSION_ARN_INFIX`]), so one ARN names the pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'a> {
    /// A private workflow, by its `WorkflowId`.
    Workflow(&'a str),
    /// One version of that workflow, by the workflow's id and the version's name.
    WorkflowVersion {
        workflow_id: &'a str,
        version_name: &'a str,
    },
    /// A run, by its `RunId`.
    Run(&'a str),
    /// A run group, by its `RunGroupId`.
    RunGroup(&'a str),
    /// A run cache, by its `RunCacheId`.
    RunCache(&'a str),
    /// A sequence store, by its `SequenceStoreId`.
    SequenceStore(&'a str),
    /// A resource of this service that no store here holds: an ARN naming a resource
    /// kind this emulator does not mint. It addresses nothing, so a call against it is
    /// answered "not found" rather than refused as a bad value.
    Unheld,
}

/// Which resource `resource_arn` names, or `None` when the value is no ARN of a resource
/// this service's API can address at all.
///
/// The ARN is read through the ONE parse, `service_common::arn::parse_of`, against this
/// service's own namespace: a well-formed ARN of another vendor, or one naming no
/// resource id, addresses nothing in this API and is a bad REQUEST rather than a missing
/// resource. Every tagging operation resolves an ARN here, so the three cannot disagree
/// about which record one names.
pub fn tagged_resource(resource_arn: &str) -> Option<TaggedResource<'_>> {
    let arn = ::service_common::arn::parse_of(resource_arn, ARN_SERVICE)?;
    let resource_type = arn.resource_type();
    let id = ::service_common::arn::resource_id_of_type(resource_arn, resource_type)
        .filter(|id| !id.is_empty())?;
    Some(match resource_type {
        WORKFLOW_RESOURCE_TYPE => match id.split_once(WORKFLOW_VERSION_ARN_INFIX) {
            Some((workflow_id, version_name))
                if !workflow_id.is_empty() && !version_name.is_empty() =>
            {
                TaggedResource::WorkflowVersion {
                    workflow_id,
                    version_name,
                }
            }
            // A workflow id carrying anything else after it names no workflow and no
            // version of one.
            Some(_) => TaggedResource::Unheld,
            None => TaggedResource::Workflow(id),
        },
        RUN_RESOURCE_TYPE => TaggedResource::Run(id),
        RUN_GROUP_RESOURCE_TYPE => TaggedResource::RunGroup(id),
        RUN_CACHE_RESOURCE_TYPE => TaggedResource::RunCache(id),
        SEQUENCE_STORE_RESOURCE_TYPE => TaggedResource::SequenceStore(id),
        _ => TaggedResource::Unheld,
    })
}

/// Whether `status` is one a run stays in until something other than the clock moves
/// it, from [`TERMINAL_RUN_STATUSES`].
pub fn is_terminal_run_status(status: &str) -> bool {
    TERMINAL_RUN_STATUSES.contains(&status)
}

/// A tag map as the wire shapes want it: omitted when empty.
fn tags_output(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    (!tags.is_empty()).then(|| {
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
}

/// Which operation an idempotency token was presented to. The token namespace is
/// per-operation, so the same `requestId` on `CreateWorkflow` and on `StartRun` names
/// two different resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdempotencyScope {
    CreateWorkflow,
    StartRun,
    CreateSequenceStore,
    CreateRunCache,
    CreateRunGroup,
}

/// Per-service state for com-amazonaws-omics-omics. A handler reaches it through
/// [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsOmicsOmicsState {
    /// Every private workflow, keyed by its numeric id. The key order is creation
    /// order, which is what makes `ListWorkflows`' paging stable.
    pub workflows: BTreeMap<u64, Workflow>,
    /// Every run, keyed by its numeric id, in creation order for the same reason.
    pub runs: BTreeMap<u64, Run>,
    /// Every sequence store, keyed by its numeric id, in creation order for the same
    /// reason.
    pub sequence_stores: BTreeMap<u64, SequenceStore>,
    /// Every multipart read set upload, keyed by its numeric upload id, in creation
    /// order for the same reason. Each record names the sequence store it writes into,
    /// so one store's uploads are the records that name it.
    pub multipart_read_set_uploads: BTreeMap<u64, MultipartReadSetUpload>,
    /// Every read set, keyed by its numeric read set id, in creation order for the same
    /// reason. Each record names the sequence store that holds it, so one store's read
    /// sets are the records that name it.
    pub read_sets: BTreeMap<u64, ReadSet>,
    /// Every run cache, keyed by its numeric id, in creation order for the same reason.
    pub run_caches: BTreeMap<u64, RunCache>,
    /// Every run group, keyed by its numeric id, in creation order for the same reason.
    pub run_groups: BTreeMap<u64, RunGroup>,
    /// Every cross-account resource share, keyed by a counter so the map's own key order
    /// is creation order, as it is for every other store here. A share's id is a minted
    /// UUID rather than the decimal the other resources carry — the modelled `shareId` is
    /// a plain string with no pattern — so the id a caller presents is matched against
    /// the records themselves ([`ComAmazonawsOmicsOmicsState::share`]) and the key stays
    /// the creation order a listing pages over.
    pub shares: BTreeMap<u64, Share>,
    /// `(operation, requestId)` -> the numeric id of the resource that request
    /// created, so a replayed `@idempotencyToken` answers with the original resource
    /// instead of minting a second one.
    pub idempotency: HashMap<(IdempotencyScope, String), u64>,
    next_workflow_id: u64,
    next_run_id: u64,
    next_sequence_store_id: u64,
    next_multipart_read_set_upload_id: u64,
    next_read_set_id: u64,
    next_run_cache_id: u64,
    next_run_group_id: u64,
    next_share_key: u64,
}

impl ComAmazonawsOmicsOmicsState {
    /// Apply every status change the clock already owed any stored record by
    /// `now_ms`. Called at the top of each handler so no operation can observe a
    /// status the clock has already moved past.
    pub fn settle(&mut self, now_ms: i64) {
        for workflow in self.workflows.values_mut() {
            workflow.settle(now_ms);
        }
        for run in self.runs.values_mut() {
            run.settle(now_ms);
        }
        for store in self.sequence_stores.values_mut() {
            store.settle(now_ms);
        }
        for share in self.shares.values_mut() {
            share.settle(now_ms);
        }
        for read_set in self.read_sets.values_mut() {
            read_set.settle(now_ms);
        }
        // A run cache owes the clock nothing: no value of the modelled
        // `RunCacheStatus` enum is a transient "being created" state, so a cache is
        // usable from the moment its create answered and no transition is ever due.
    }

    /// The workflow the modelled `WorkflowId` `id` names, or `None`.
    pub fn workflow(&self, id: &str) -> Option<&Workflow> {
        self.workflows.get(&numeric_id(id)?)
    }

    /// The same record, for an operation that CHANGES the workflow or the versions it
    /// holds. One lookup rule for both, so a create and a read cannot disagree about
    /// which workflow an id names.
    pub fn workflow_mut(&mut self, id: &str) -> Option<&mut Workflow> {
        self.workflows.get_mut(&numeric_id(id)?)
    }

    /// The run the modelled `RunId` `id` names, or `None`.
    pub fn run(&self, id: &str) -> Option<&Run> {
        self.runs.get(&numeric_id(id)?)
    }

    /// The same record, for an operation that CHANGES the run. One lookup rule for both,
    /// so a read and a write cannot disagree about which run an id names.
    pub fn run_mut(&mut self, id: &str) -> Option<&mut Run> {
        self.runs.get_mut(&numeric_id(id)?)
    }

    /// The sequence store the modelled `SequenceStoreId` `id` names, or `None`.
    pub fn sequence_store(&self, id: &str) -> Option<&SequenceStore> {
        self.sequence_stores.get(&numeric_id(id)?)
    }

    /// The same record, for an operation that CHANGES the store, resolved by the same
    /// rule.
    pub fn sequence_store_mut(&mut self, id: &str) -> Option<&mut SequenceStore> {
        self.sequence_stores.get_mut(&numeric_id(id)?)
    }

    /// Remove the sequence store the modelled `SequenceStoreId` `id` names, together
    /// with everything held INSIDE it, and answer the record that was removed (`None`
    /// when no store carries that id).
    ///
    /// The store's own tags, the multipart read set uploads initiated against it and the
    /// read sets it holds live inside the store: every upload and read set operation names
    /// the store its record belongs to, so a record left behind here would be one nothing
    /// could ever read, finish or abort. `DeleteSequenceStore` documents that reclamation
    /// for the tags ("When a sequence store is deleted, all tags associated with the store
    /// are also deleted"), and it is the same rule for every other record scoped to the
    /// store.
    pub fn remove_sequence_store(&mut self, id: &str) -> Option<SequenceStore> {
        let key = numeric_id(id)?;
        let store = self.sequence_stores.remove(&key)?;
        // Matched on the store's NUMERIC key, the same normalisation every lookup in this
        // crate uses, so an upload that spells its store's id differently (the modelled
        // `^[0-9]+$` admits leading zeros) is still reclaimed with it.
        self.multipart_read_set_uploads
            .retain(|_, upload| numeric_id(&upload.sequence_store_id) != Some(key));
        self.read_sets
            .retain(|_, read_set| numeric_id(&read_set.sequence_store_id) != Some(key));
        Some(store)
    }

    /// The multipart read set upload the modelled `UploadId` `upload_id` names INSIDE
    /// the sequence store `sequence_store_id`, or `None`.
    ///
    /// An upload belongs to the store it was initiated against — every upload operation
    /// names both ids — so an upload id presented under another store resolves to
    /// nothing here rather than reaching that record.
    pub fn multipart_read_set_upload(
        &self,
        sequence_store_id: &str,
        upload_id: &str,
    ) -> Option<&MultipartReadSetUpload> {
        self.multipart_read_set_uploads
            .get(&numeric_id(upload_id)?)
            .filter(|upload| upload.sequence_store_id == sequence_store_id)
    }

    /// Every multipart read set upload initiated against `sequence_store_id`, in
    /// creation order: the map's own key order.
    pub fn multipart_read_set_uploads_for<'a>(
        &'a self,
        sequence_store_id: &'a str,
    ) -> impl Iterator<Item = (&'a u64, &'a MultipartReadSetUpload)> + 'a {
        self.multipart_read_set_uploads
            .iter()
            .filter(move |(_, upload)| upload.sequence_store_id == sequence_store_id)
    }

    /// The same record, for an operation that CHANGES the upload — above all the parts
    /// it holds. One lookup rule for both, so a part write and a part read cannot
    /// disagree about which upload a pair of ids names.
    pub fn multipart_read_set_upload_mut(
        &mut self,
        sequence_store_id: &str,
        upload_id: &str,
    ) -> Option<&mut MultipartReadSetUpload> {
        self.multipart_read_set_uploads
            .get_mut(&numeric_id(upload_id)?)
            .filter(|upload| upload.sequence_store_id == sequence_store_id)
    }

    /// Remove the multipart read set upload `upload_id` from the store
    /// `sequence_store_id`, and answer the record that was removed (`None` when that pair
    /// names no upload this emulator holds).
    ///
    /// An upload is spent once it is completed or aborted: the parts it collected are no
    /// longer something a caller can add to, list or finish, so the record goes rather
    /// than lingering as an upload nothing can act on.
    pub fn remove_multipart_read_set_upload(
        &mut self,
        sequence_store_id: &str,
        upload_id: &str,
    ) -> Option<MultipartReadSetUpload> {
        let key = numeric_id(upload_id)?;
        // Resolved by the same rule every other upload lookup uses, so an upload id
        // presented under another store is not removed from the store it belongs to.
        self.multipart_read_set_upload(sequence_store_id, upload_id)?;
        self.multipart_read_set_uploads.remove(&key)
    }

    /// The read set the modelled `ReadSetId` `id` names INSIDE the sequence store
    /// `sequence_store_id`, or `None`.
    ///
    /// A read set belongs to the store that holds it — every read set operation names both
    /// ids — so a read set id presented under another store resolves to nothing here
    /// rather than reaching that record.
    pub fn read_set(&self, sequence_store_id: &str, id: &str) -> Option<&ReadSet> {
        self.read_sets
            .get(&numeric_id(id)?)
            .filter(|read_set| read_set.sequence_store_id == sequence_store_id)
    }

    /// Remove the read set `id` from the store `sequence_store_id`, and answer the record
    /// that was removed (`None` when that pair names no read set this emulator holds).
    ///
    /// A deleted read set is GONE, not a record in a terminal status: nothing resolves its
    /// id afterwards, so the per-id reads and the listing agree that the store no longer
    /// holds it, the files it held stop counting against the store's ceiling, and the
    /// store's own delete is no longer blocked by it.
    pub fn remove_read_set(&mut self, sequence_store_id: &str, id: &str) -> Option<ReadSet> {
        let key = numeric_id(id)?;
        // Resolved by the same rule every other read set lookup uses, so a read set id
        // presented under another store is not removed from the store that holds it.
        self.read_set(sequence_store_id, id)?;
        self.read_sets.remove(&key)
    }

    /// Every read set held by `sequence_store_id`, in creation order: the map's own key
    /// order.
    pub fn read_sets_for<'a>(
        &'a self,
        sequence_store_id: &'a str,
    ) -> impl Iterator<Item = (&'a u64, &'a ReadSet)> + 'a {
        self.read_sets
            .iter()
            .filter(move |(_, read_set)| read_set.sequence_store_id == sequence_store_id)
    }

    /// Whether `sequence_store_id` still holds a read set, which is what stands in the way
    /// of deleting the store itself.
    pub fn holds_read_sets(&self, sequence_store_id: &str) -> bool {
        self.read_sets_for(sequence_store_id).next().is_some()
    }

    /// The content bytes every read set of `sequence_store_id` holds together.
    pub fn read_set_bytes_held(&self, sequence_store_id: &str) -> usize {
        self.read_sets_for(sequence_store_id)
            .map(|(_, read_set)| read_set.held_bytes())
            .sum()
    }

    /// The run cache the modelled `RunCacheId` `id` names, or `None`.
    pub fn run_cache(&self, id: &str) -> Option<&RunCache> {
        self.run_caches.get(&numeric_id(id)?)
    }

    /// The same record, for an operation that CHANGES the cache, resolved by the same
    /// rule.
    pub fn run_cache_mut(&mut self, id: &str) -> Option<&mut RunCache> {
        self.run_caches.get_mut(&numeric_id(id)?)
    }

    /// The run group the modelled `RunGroupId` `id` names, or `None`.
    pub fn run_group(&self, id: &str) -> Option<&RunGroup> {
        self.run_groups.get(&numeric_id(id)?)
    }

    /// The same record, for an operation that CHANGES the group, resolved by the same
    /// rule.
    pub fn run_group_mut(&mut self, id: &str) -> Option<&mut RunGroup> {
        self.run_groups.get_mut(&numeric_id(id)?)
    }

    /// The share the modelled `shareId` `id` names, or `None`.
    ///
    /// A share id is not a store key (see [`ComAmazonawsOmicsOmicsState::shares`]), so
    /// the match is against the records; the store is bounded by [`MAX_SHARES`], and this
    /// is the ONE place a share id is resolved, so every share operation agrees about
    /// which record an id names.
    pub fn share(&self, id: &str) -> Option<&Share> {
        self.shares.values().find(|share| share.id == id)
    }

    /// The same record, for an operation that CHANGES the share, resolved by the same
    /// rule.
    pub fn share_mut(&mut self, id: &str) -> Option<&mut Share> {
        self.shares.values_mut().find(|share| share.id == id)
    }

    /// The share that already offers `resource_arn` to `principal_subscriber`, if one
    /// stands: a resource can be offered to an account once at a time, and a share whose
    /// delete is in flight or complete no longer holds the offer.
    pub fn standing_share(&self, resource_arn: &str, principal_subscriber: &str) -> Option<&Share> {
        self.shares.values().find(|share| {
            share.resource_arn == resource_arn
                && share.principal_subscriber == principal_subscriber
                && share.offer_stands()
        })
    }

    /// The tags of the resource `resource_arn` names, or `None` when this emulator holds
    /// no resource under that ARN.
    ///
    /// Tags live ON the resource they belong to — every record that carries them is read
    /// back by its own `Get`, and a resource's tags are reclaimed with it — so a tagging
    /// call reaches the one record rather than a directory beside it. The ARN is matched
    /// against the record's OWN ARN, so one carrying a held id in another Region or
    /// account names nothing here, which is what the modelled `ResourceNotFoundException`
    /// states ("not found in the current Region").
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        match tagged_resource(resource_arn)? {
            TaggedResource::Workflow(id) => {
                let workflow = self.workflow(id)?;
                (workflow.arn == resource_arn).then_some(&workflow.tags)
            }
            TaggedResource::WorkflowVersion {
                workflow_id,
                version_name,
            } => {
                let version = self.workflow(workflow_id)?.versions.get(version_name)?;
                (version.arn == resource_arn).then_some(&version.tags)
            }
            TaggedResource::Run(id) => {
                let run = self.run(id)?;
                (run.arn == resource_arn).then_some(&run.tags)
            }
            TaggedResource::RunGroup(id) => {
                let group = self.run_group(id)?;
                (group.arn == resource_arn).then_some(&group.tags)
            }
            TaggedResource::RunCache(id) => {
                let cache = self.run_cache(id)?;
                (cache.arn == resource_arn).then_some(&cache.tags)
            }
            TaggedResource::SequenceStore(id) => {
                let store = self.sequence_store(id)?;
                (store.arn == resource_arn).then_some(&store.tags)
            }
            TaggedResource::Unheld => None,
        }
    }

    /// The same tags, for a tagging call that CHANGES them. One resolution rule for both,
    /// so a write and a read cannot disagree about which record an ARN names.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match tagged_resource(resource_arn)? {
            TaggedResource::Workflow(id) => {
                let workflow = self.workflow_mut(id)?;
                (workflow.arn == resource_arn).then_some(&mut workflow.tags)
            }
            TaggedResource::WorkflowVersion {
                workflow_id,
                version_name,
            } => {
                let version = self
                    .workflow_mut(workflow_id)?
                    .versions
                    .get_mut(version_name)?;
                (version.arn == resource_arn).then_some(&mut version.tags)
            }
            TaggedResource::Run(id) => {
                let run = self.run_mut(id)?;
                (run.arn == resource_arn).then_some(&mut run.tags)
            }
            TaggedResource::RunGroup(id) => {
                let group = self.run_group_mut(id)?;
                (group.arn == resource_arn).then_some(&mut group.tags)
            }
            TaggedResource::RunCache(id) => {
                let cache = self.run_cache_mut(id)?;
                (cache.arn == resource_arn).then_some(&mut cache.tags)
            }
            TaggedResource::SequenceStore(id) => {
                let store = self.sequence_store_mut(id)?;
                (store.arn == resource_arn).then_some(&mut store.tags)
            }
            TaggedResource::Unheld => None,
        }
    }

    /// Mint the next share store key, which is the creation order the map is read in.
    pub fn next_share_key(&mut self) -> u64 {
        let key = self.next_share_key;
        self.next_share_key += 1;
        key
    }

    /// Mint the next workflow id, as the decimal string the model's pattern wants
    /// and the numeric key the store is ordered by.
    pub fn next_workflow_id(&mut self) -> (u64, String) {
        let key = FIRST_RESOURCE_ID + self.next_workflow_id;
        self.next_workflow_id += 1;
        (key, key.to_string())
    }

    /// Mint the next run id, the same way, from a counter offset by
    /// [`RUN_ID_OFFSET`] so no run shares an id with a workflow.
    pub fn next_run_id(&mut self) -> (u64, String) {
        let key = FIRST_RESOURCE_ID + RUN_ID_OFFSET + self.next_run_id;
        self.next_run_id += 1;
        (key, key.to_string())
    }

    /// Mint the next sequence store id, from [`FIRST_SEQUENCE_STORE_ID`] so every one
    /// is the ten-digit decimal the modelled `SequenceStoreId` length admits.
    pub fn next_sequence_store_id(&mut self) -> (u64, String) {
        let key = FIRST_SEQUENCE_STORE_ID + self.next_sequence_store_id;
        self.next_sequence_store_id += 1;
        (key, key.to_string())
    }

    /// Mint the next multipart-read-set-upload id, from [`FIRST_SEQUENCE_STORE_ID`] plus
    /// [`UPLOAD_ID_OFFSET`] so every one is the ten-digit decimal the modelled
    /// `UploadId` length admits and no upload shares an id with a sequence store.
    pub fn next_multipart_read_set_upload_id(&mut self) -> (u64, String) {
        let key =
            FIRST_SEQUENCE_STORE_ID + UPLOAD_ID_OFFSET + self.next_multipart_read_set_upload_id;
        self.next_multipart_read_set_upload_id += 1;
        (key, key.to_string())
    }

    /// Mint the next read set id, from [`FIRST_SEQUENCE_STORE_ID`] plus
    /// [`READ_SET_ID_OFFSET`] so every one is the ten-digit decimal the modelled
    /// `ReadSetId` length admits and no read set shares an id with a sequence store or an
    /// upload.
    pub fn next_read_set_id(&mut self) -> (u64, String) {
        let key = FIRST_SEQUENCE_STORE_ID + READ_SET_ID_OFFSET + self.next_read_set_id;
        self.next_read_set_id += 1;
        (key, key.to_string())
    }

    /// Mint the next run cache id, the same way as a run's, from a counter offset by
    /// [`RUN_CACHE_ID_OFFSET`] so no cache shares an id with a workflow, a run or a run
    /// group. The modelled `RunCacheId` is `^[0-9]+$` with length 1..18, which every
    /// value this mints satisfies.
    pub fn next_run_cache_id(&mut self) -> (u64, String) {
        let key = FIRST_RESOURCE_ID + RUN_CACHE_ID_OFFSET + self.next_run_cache_id;
        self.next_run_cache_id += 1;
        (key, key.to_string())
    }

    /// Mint the next run group id, the same way, from a counter offset by
    /// [`RUN_GROUP_ID_OFFSET`] so no run group shares an id with a workflow or a run.
    /// The modelled `RunGroupId` is `^[0-9]+$` with length 1..18, which every value this
    /// mints satisfies.
    pub fn next_run_group_id(&mut self) -> (u64, String) {
        let key = FIRST_RESOURCE_ID + RUN_GROUP_ID_OFFSET + self.next_run_group_id;
        self.next_run_group_id += 1;
        (key, key.to_string())
    }

    /// The resource a replayed `requestId` already created, if this emulator still
    /// holds it. A token whose resource has since been deleted is forgotten, so the
    /// same `requestId` can create a fresh one — the service cannot answer with a
    /// resource that no longer exists.
    pub fn replayed(&mut self, scope: IdempotencyScope, request_id: &str) -> Option<u64> {
        let key = (scope, request_id.to_string());
        let id = *self.idempotency.get(&key)?;
        let still_held = match scope {
            IdempotencyScope::CreateWorkflow => self.workflows.contains_key(&id),
            IdempotencyScope::StartRun => self.runs.contains_key(&id),
            IdempotencyScope::CreateSequenceStore => self.sequence_stores.contains_key(&id),
            IdempotencyScope::CreateRunCache => self.run_caches.contains_key(&id),
            IdempotencyScope::CreateRunGroup => self.run_groups.contains_key(&id),
        };
        if still_held {
            return Some(id);
        }
        self.idempotency.remove(&key);
        None
    }

    /// Remember that `request_id` created the resource `id`.
    ///
    /// The map is capped: once it is full, entries whose resource is gone are dropped
    /// first, and a token is only remembered if that leaves room. Forgetting a token
    /// can only cause a duplicate create on a retry, never a wrong answer.
    pub fn remember(&mut self, scope: IdempotencyScope, request_id: String, id: u64) {
        if self.idempotency.len() >= MAX_IDEMPOTENCY_TOKENS {
            let workflows = &self.workflows;
            let runs = &self.runs;
            let sequence_stores = &self.sequence_stores;
            let run_caches = &self.run_caches;
            let run_groups = &self.run_groups;
            self.idempotency.retain(|key, id| match key.0 {
                IdempotencyScope::CreateWorkflow => workflows.contains_key(id),
                IdempotencyScope::StartRun => runs.contains_key(id),
                IdempotencyScope::CreateSequenceStore => sequence_stores.contains_key(id),
                IdempotencyScope::CreateRunCache => run_caches.contains_key(id),
                IdempotencyScope::CreateRunGroup => run_groups.contains_key(id),
            });
        }
        if self.idempotency.len() < MAX_IDEMPOTENCY_TOKENS {
            self.idempotency.insert((scope, request_id), id);
        }
    }
}

/// The numeric key a modelled `WorkflowId`/`RunId` names.
///
/// Both are `^[0-9]+$` with length 1..18, so every value the centralized walk admits
/// parses; an id outside that (which cannot reach a handler) answers `None` and is
/// then reported as "not found" rather than panicking. The modelled `SequenceStoreId`
/// is the same decimal with length 10..36, so an id wider than `u64` also answers
/// `None`: no store this emulator minted can carry one, so "not found" is the right
/// answer for it too.
pub fn numeric_id(id: &str) -> Option<u64> {
    id.parse::<u64>().ok()
}

/// The largest page either of this crate's listings will ever build, and the page size
/// applied when a request omits `maxResults`.
///
/// It is the maximum of the modelled `maxResults` range (1..100) that `ListRuns`,
/// `ListWorkflows` and `ListRunGroups` share, stated once here. Each handler clamps to
/// it, so the page it allocates is decided by the server, never by the request — the
/// handlers do not rely on the centralized walk having refused a larger value.
pub const MAX_LIST_PAGE_SIZE: usize = 100;

/// The tag a `ListRuns` page token carries, so a token issued by another listing is
/// refused rather than read as a resume point into the run store.
pub const RUN_PAGE_TOKEN_TAG: &str = "run_";

/// The same, for `ListWorkflows`.
pub const WORKFLOW_PAGE_TOKEN_TAG: &str = "wf_";

/// The same, for `ListRunGroups`.
pub const RUN_GROUP_PAGE_TOKEN_TAG: &str = "rg_";

/// The same, for `ListReadSetUploadParts`. A part is keyed by its part NUMBER inside
/// the one upload and source the request names, so that listing's token names the part
/// number the next page resumes at; the tag is still distinct from every other, so no
/// token can resume another listing's walk.
pub const READ_SET_UPLOAD_PART_PAGE_TOKEN_TAG: &str = "rsup_";

/// The same, for `ListWorkflowVersions`. A version is keyed by its NAME inside its
/// workflow, so that listing's token names a name rather than a numeric id; the tag is
/// still distinct from every other, so no token can resume another listing's walk.
pub const WORKFLOW_VERSION_PAGE_TOKEN_TAG: &str = "wfv_";

/// The same, for `ListMultipartReadSetUploads`. An upload is keyed by its numeric
/// `uploadId`, so that listing's token names the upload the next page resumes at.
pub const MULTIPART_READ_SET_UPLOAD_PAGE_TOKEN_TAG: &str = "mrsu_";

/// The same, for `ListReadSets`. A read set is keyed by its numeric `readSetId`, so that
/// listing's token names the read set the next page resumes at.
pub const READ_SET_PAGE_TOKEN_TAG: &str = "rs_";

/// A page token naming the record the NEXT page of a listing starts at, by that
/// record's own key.
///
/// One codec for every listing in this crate, so each mints and reads tokens the same
/// way. Every list token this service models is opaque, so the key is hex-encoded
/// rather than handed back in the clear — opaque to a caller, not secret: hex is
/// trivially reversible and the token carries nothing beyond a record key. `tag` names
/// the collection the token belongs to, so one listing's token cannot resume another's
/// walk.
///
/// Every modelled token member's pattern is
/// `^[\p{L}||\p{M}||\p{Z}||\p{S}||\p{N}||\p{P}]+$`, and every character this produces is
/// admitted: the hex digits are `\p{N}`/`\p{L}` and a tag's `_` is `\p{Pc}`, a `\p{P}`
/// subcategory. The result is the tag plus two hex characters per byte of the key, which
/// stays inside each member's modelled length: a numeric id is at most the 20 digits of
/// `u64::MAX` (40 characters, inside the 1..128 of `RunListToken` and
/// `WorkflowListToken`) and a `WorkflowVersionName` at most 64 (128 characters, inside
/// the 1..256 of `WorkflowVersionListToken`).
pub fn encode_name_page_token(tag: &str, key: &str) -> String {
    format!("{tag}{}", hex::encode(key))
}

/// The record key a page token tagged `tag` names, or `None` when the token is not one
/// THAT listing issued. The caller renders that as its own 400 refusal.
pub fn decode_name_page_token(tag: &str, token: &str) -> Option<String> {
    let body = token.strip_prefix(tag)?;
    let key = hex::decode(body)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    // A token that names no key at all resumes nothing, so it is not one this service
    // issued either.
    (!key.is_empty()).then_some(key)
}

/// The same token, for a listing whose records are keyed by the numeric id
/// [`numeric_id`] reads.
pub fn encode_page_token(tag: &str, id: u64) -> String {
    encode_name_page_token(tag, &id.to_string())
}

/// The record id a page token tagged `tag` names, or `None` when the token is not one
/// THAT listing issued. The caller renders that as its own 400 refusal.
pub fn decode_page_token(tag: &str, token: &str) -> Option<u64> {
    decode_name_page_token(tag, token)
        .as_deref()
        .and_then(numeric_id)
}

/// The ARN of the workflow `id` in `region`.
pub fn workflow_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{WORKFLOW_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the version `version_name` of the workflow `workflow_id` in `region`.
///
/// The version is named inside its workflow's own resource path, so one ARN names the
/// pair and no version ARN can collide with a workflow ARN. The result is well inside
/// the modelled `WorkflowVersionArn` length (1..150): the account and region are
/// fixed, the workflow id is at most 18 characters and the name at most 64.
pub fn workflow_version_arn(region: &str, workflow_id: &str, version_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{WORKFLOW_ARN_RESOURCE_PREFIX}{workflow_id}{WORKFLOW_VERSION_ARN_INFIX}{version_name}"
        ),
    )
}

/// The ARN of the run `id` in `region`.
pub fn run_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{RUN_ARN_RESOURCE_PREFIX}{id}"))
}

/// The ARN of the run cache `id` in `region`.
///
/// Well inside the modelled `RunCacheArn` length (1..128): the account and region are
/// fixed and the id is at most 18 characters.
pub fn run_cache_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RUN_CACHE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the run group `id` in `region`.
///
/// Well inside the modelled `RunGroupArn` length (1..128): the account and region are
/// fixed and the id is at most 18 characters.
pub fn run_group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RUN_GROUP_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the sequence store `id` in `region`.
///
/// Well inside the modelled `SequenceStoreArn` length (1..127): the account and region
/// are fixed and the id is at most 36 characters.
pub fn sequence_store_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{SEQUENCE_STORE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the read set `read_set_id` held by the sequence store `sequence_store_id`
/// in `region`.
///
/// The read set is named inside its store's own resource path, so one ARN names the pair.
/// Well inside the modelled `ReadSetArn` length (1..127): the account and region are
/// fixed and each id is at most 36 characters.
pub fn read_set_arn(region: &str, sequence_store_id: &str, read_set_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{SEQUENCE_STORE_ARN_RESOURCE_PREFIX}{sequence_store_id}\
             {READ_SET_ARN_INFIX}{read_set_id}"
        ),
    )
}

/// Where this run's outputs land: the caller's `outputUri` under the prefix the run's
/// `outputUriPrefixMode` asks the service to add.
///
/// `SERVICE_DEFINED` — the mode a run that named none resolves to — "adds a run-specific
/// prefix", so the run's own id is appended and two runs writing to one `outputUri` do
/// not collide. `NONE_WITH_OVERWRITE` "adds no prefix and overwrites existing objects at
/// the run's output keys", so the destination is the `outputUri` the request submitted.
///
/// Derived from the request and the minted id — nothing here is invented per run.
pub fn run_output_uri(output_uri: &str, id: &str, output_uri_prefix_mode: &str) -> String {
    if output_uri_prefix_mode == OUTPUT_URI_PREFIX_MODE_NONE_WITH_OVERWRITE {
        return output_uri.to_string();
    }
    format!("{}/{id}", output_uri.trim_end_matches('/'))
}

/// Mixed into every [`fresh_uuid`] so two calls in the same nanosecond still differ.
static UUID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A fresh UUID in the form the modelled `WorkflowUuid` pattern describes
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
///
/// The 16 bytes come from SHA-256 (the `sha2` crate — no hashing is written here) over a
/// seed no other call can repeat: a monotonically increasing per-process counter, the
/// wall clock and the process id. Bytes 6 and 8 then carry the RFC 4122 version (4) and
/// variant (binary 10) nibbles, so the result is a well-formed v4-shaped UUID rather
/// than 16 arbitrary bytes, and the five groups are `hex`-encoded (the `hex` crate).
///
/// A UUID is used here to NAME a resource, not to authenticate anything, so a value
/// derived this way is the right strength: it is unique and unpredictable enough to
/// identify a workflow or a run, and it is never a credential.
pub fn fresh_uuid() -> String {
    use sha2::Digest;
    let sequence = UUID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut hasher = sha2::Sha256::new();
    hasher.update(sequence.to_be_bytes());
    hasher.update(now_epoch_millis().to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The content digest of a workflow definition, or `None` when this emulator never saw
/// the definition's content.
///
/// SHA-256 over the ZIP archive the create carried, rendered as the 64 lowercase hex
/// characters that exactly fill the modelled `WorkflowDigest` length (1..64) — the same
/// value a client gets by digesting the archive it uploaded.
///
/// A create that named only a `definitionUri` (or a `definitionRepository`) gets NO
/// digest: this emulator serves no S3 object store and no code repository, so it never
/// read the archive and has nothing to digest. Digesting the URI string instead would be
/// indistinguishable on the wire from a content digest, and a client comparing it with
/// its own hash of the archive would see a plausible but wrong mismatch — so the member
/// is omitted rather than filled with something nothing computed from content. A later
/// slice that really reads the object must digest the bytes it read.
pub fn definition_digest(definition_zip: Option<&[u8]>) -> Option<String> {
    use sha2::Digest;
    let zip = definition_zip?;
    Some(hex::encode(sha2::Sha256::digest(zip)))
}

/// The checksum of one uploaded read set part: the content digest
/// `UploadReadSetPart` answers with, `ListReadSetUploadParts` reports for that part and
/// `CompleteMultipartReadSetUpload` accepts for it.
///
/// SHA-256 over the part's bytes (the `sha2` crate — no hashing is written here), rendered
/// as standard, `=`-padded base64 (`aws_smithy_types::base64`, the codec this workspace
/// already carries), which is the encoding an AWS content checksum travels in: the 32
/// digest bytes as 44 characters. That is the value a caller compares against, since a
/// client digests the bytes it sent and base64-encodes them; the same digest rendered as
/// hex is a different string and would fail that comparison. This is NOT the rendering of
/// a workflow definition's digest ([`definition_digest`]), whose modelled length of at
/// most 64 characters is the hex form a client gets from hashing the archive it uploaded.
///
/// It is computed from the content alone, so a caller that digests the bytes it sent gets
/// the same value back and can confirm the part the service holds is the part it uploaded;
/// re-uploading a part number with the same content yields the same checksum, and
/// different content a different one.
pub fn read_set_part_checksum(payload: &[u8]) -> String {
    use sha2::Digest;
    ::aws_smithy_types::base64::encode(sha2::Sha256::digest(payload).as_slice())
}

/// Whether `account_id` names the one account this emulator serves
/// (`engine::emulator_account_id()`).
///
/// A `workflowOwnerId` naming any other account asks for a workflow SHARED from that
/// account. A share this emulator holds is an offer made BY this account, and the
/// subscriber's side of it (`AcceptShare`) is its own slice, so no workflow owned
/// elsewhere can be resolved and the request is answered "not found" rather than silently
/// served from this account's own store.
pub fn is_emulator_account(account_id: &str) -> bool {
    account_id == ::engine::emulator_account_id()
}

/// The parameter names `parameters` carries that the workflow's `parameterTemplate` does
/// not define, rendered for a refusal message; `None` when there are none.
///
/// From `StartRun`'s own documentation: "The run cannot include any parameters that are
/// not defined in the parameter template." A workflow that declared NO template
/// constrains nothing — HealthOmics parses the template from the definition file in that
/// case, which this slice does not read — so this rule has nothing to say about it. A
/// `parameters` document that is not an object names no parameters at all.
pub fn undefined_parameters(
    parameter_template: &BTreeMap<String, WorkflowParameter>,
    parameters: Option<&::aws_smithy_types::Document>,
) -> Option<String> {
    if parameter_template.is_empty() {
        return None;
    }
    let ::aws_smithy_types::Document::Object(supplied) = parameters? else {
        return None;
    };
    let undefined: Vec<&str> = supplied
        .keys()
        .filter(|name| !parameter_template.contains_key(name.as_str()))
        .map(String::as_str)
        .collect();
    (!undefined.is_empty()).then(|| undefined.join(", "))
}

/// Why the caller-sized collections on a create or start request are unacceptable,
/// or `None` when each is within the store's bounds.
///
/// The model publishes no length for the `tags` map, so the centralized walk cannot
/// refuse an oversized one and this is where the bound lives. Every operation that
/// stores tags applies this one rule, each rendering the refusal as its own modelled
/// `ValidationException`.
pub fn tag_bounds_rejection(tags: Option<&HashMap<String, String>>) -> Option<String> {
    let count = tags.map_or(0, HashMap::len);
    (count > MAX_TAGS).then(|| {
        format!("tags carries {count} entries, more than the {MAX_TAGS} this service accepts")
    })
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not region-shaped is refused here and the default used instead:
/// a generated ARN can never carry text the modelled `^arn:.+$` would not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, in milliseconds since the Unix epoch — the resolution this service's
/// `date-time` timestamps are stored and compared at.
///
/// A host clock set before 1970 is the only way `duration_since` fails. It is rendered as
/// the NEGATIVE offset rather than collapsed to 0: every reading then stays ordered with
/// every other, so a due instant is still in the future of the record that owes it,
/// instead of every owed transition becoming instantly due.
pub fn now_epoch_millis() -> i64 {
    let clamp = |millis: u128| millis.min(i64::MAX as u128) as i64;
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since_epoch) => clamp(since_epoch.as_millis()),
        Err(before_epoch) => -clamp(before_epoch.duration().as_millis()),
    }
}

/// An epoch-millis instant as the `date-time` timestamp the model binds.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Emit the structured access record for one call to this service.
///
/// Every operation in this slice calls it exactly once, on the branch it returns from —
/// above all the four that CHANGE the store (`CreateWorkflow`, `StartRun`, `DeleteRun`,
/// `DeleteWorkflow`), whose effect on it is otherwise invisible after the fact. The
/// record carries the operation, the identity the request PRESENTED (its SigV4 access key
/// id, which is an identifier and not a secret), the request target it named, the
/// resource the call resolved when it resolved one, and the outcome; the `tracing`
/// subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given one:
/// [`smithy_server_runtime::OperationContext::parts`] carries the request's method, URI,
/// headers and extensions, and nothing in this framework puts a peer address in them. The
/// presented credential is therefore what identifies the caller.
///
/// Deliberately metadata only: no tag, run parameter, workflow definition, output URI or
/// idempotency token is logged, so a caller's own data never reaches the log. Every
/// interpolated value is request-derived, so [`audit_field`] bounds it and replaces the
/// control characters that would otherwise let one request forge a second record.
pub fn audit(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
    resource: Option<&str>,
) {
    let parts = ctx.parts();
    ::tracing::info!(
        target: "emulator::audit",
        service = ctx.identity().signing_name(),
        operation,
        access_key_id = %audit_field(&::engine::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        resource = %audit_field(resource.unwrap_or(AUDIT_NO_RESOURCE)),
        outcome = outcome.as_str(),
        "omics api call"
    );
}

/// What the `resource` field reads as when the call resolved no resource (a refusal
/// before any lookup, or a listing, which names none).
pub const AUDIT_NO_RESOURCE: &str = "-";

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id, a request path and a resource id are all caller-controlled, so
/// an unbounded echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters (above
/// all CR and LF, which would forge a second record) replaced, and the whole value
/// bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A private workflow was minted and stored.
    WorkflowCreated,
    /// A stored workflow was removed from the store.
    WorkflowDeleted,
    /// A version was minted and stored against a stored workflow.
    WorkflowVersionCreated,
    /// A stored workflow version was removed from its workflow.
    WorkflowVersionDeleted,
    /// A run was minted and stored against a stored workflow.
    RunStarted,
    /// A stored run was removed from the store.
    RunDeleted,
    /// A sequence store was minted and stored.
    SequenceStoreCreated,
    /// A stored sequence store was removed from the store, with everything held inside it.
    SequenceStoreDeleted,
    /// A multipart read set upload was initiated and stored against a sequence store.
    MultipartReadSetUploadCreated,
    /// A part was stored against a multipart read set upload, replacing whatever that
    /// source and part number held before.
    ReadSetUploadPartStored,
    /// A multipart read set upload was completed: its parts were formed into a read set
    /// held by the sequence store, and the upload itself is spent.
    ReadSetCreated,
    /// A stored read set was deleted: it is in the terminal `DELETED` status and its
    /// store no longer holds its files.
    ReadSetDeleted,
    /// A run cache was minted and stored.
    RunCacheCreated,
    /// A run group was minted and stored.
    RunGroupCreated,
    /// A stored run group's settings were changed in place.
    RunGroupUpdated,
    /// A stored run group was deleted: the registry no longer holds the record.
    RunGroupDeleted,
    /// A cross-account resource share was minted and stored.
    ShareCreated,
    /// A stored share was asked to go away, and carries the status that move leaves it in.
    ShareDeleted,
    /// Tags were written onto a stored resource.
    ResourceTagged,
    /// Tags were removed from a stored resource.
    ResourceUntagged,
    /// A replayed `@idempotencyToken` changed nothing and answered with the resource the
    /// first call created.
    NoChange,
    /// A reporting operation answered from state and changed nothing beyond the status
    /// transitions the clock already owed.
    Reported,
    /// The request was refused as invalid (400).
    ValidationRefused,
    /// The named resource does not exist, or is not one this emulator holds (404).
    NotFound,
    /// The request cannot be applied to the resource in its current state (409).
    Conflict,
    /// A quota — the service's or this emulator's own ceiling — was met (402).
    QuotaExceeded,
    /// This service's own store was unreachable (500).
    StoreUnavailable,
    /// The request asked for behaviour this emulator does not implement (501).
    NotImplemented,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowCreated => "workflow-created",
            Self::WorkflowDeleted => "workflow-deleted",
            Self::WorkflowVersionCreated => "workflow-version-created",
            Self::WorkflowVersionDeleted => "workflow-version-deleted",
            Self::RunStarted => "run-started",
            Self::RunDeleted => "run-deleted",
            Self::SequenceStoreCreated => "sequence-store-created",
            Self::SequenceStoreDeleted => "sequence-store-deleted",
            Self::MultipartReadSetUploadCreated => "multipart-read-set-upload-created",
            Self::ReadSetUploadPartStored => "read-set-upload-part-stored",
            Self::ReadSetCreated => "read-set-created",
            Self::ReadSetDeleted => "read-set-deleted",
            Self::RunCacheCreated => "run-cache-created",
            Self::RunGroupCreated => "run-group-created",
            Self::RunGroupUpdated => "run-group-updated",
            Self::RunGroupDeleted => "run-group-deleted",
            Self::ShareCreated => "share-created",
            Self::ShareDeleted => "share-deleted",
            Self::ResourceTagged => "resource-tagged",
            Self::ResourceUntagged => "resource-untagged",
            Self::NoChange => "no-change",
            Self::Reported => "reported",
            Self::ValidationRefused => "validation-refused",
            Self::NotFound => "not-found",
            Self::Conflict => "conflict",
            Self::QuotaExceeded => "quota-exceeded",
            Self::StoreUnavailable => "store-unavailable",
            Self::NotImplemented => "not-implemented",
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOmicsOmicsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOmicsOmicsState>)
}
