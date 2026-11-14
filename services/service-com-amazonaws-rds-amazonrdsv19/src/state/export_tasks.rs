// SPDX-License-Identifier: CC0-1.0

//! The snapshot- and cluster-export records `DescribeExportTasks` reports and
//! `CancelExportTask` moves.
//!
//! An export task is the record of one export of a DB snapshot, DB cluster snapshot or DB
//! cluster to Amazon S3: which source was exported, where it is being written, under which
//! IAM role and KMS key, and how far it has got. The record carries the whole of the
//! modelled `ExportTask` shape, because that one shape is what every export-task operation
//! answers with ([`ExportTaskRecord::to_wire`]), so no two of them can disagree about a
//! task.
//!
//! `StartExportTask` is what inserts here, the cancel path is what moves a task on its way
//! out, and the reporting operations run against these records rather than against a canned
//! answer, so they report exactly what is stored.

use std::time::Instant;

use super::{now_as_wire_timestamp, PendingChange, PendingStatus};
use crate::ops::types::CancelExportTaskOutput;

/// `ExportTask$Status` — "The progress status of the export task": the export has been
/// accepted and is being set up.
pub const EXPORT_TASK_STATUS_STARTING: &str = "STARTING";

/// `ExportTask$Status` — the export is writing to the bucket.
pub const EXPORT_TASK_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `ExportTask$Status` — a cancellation has been accepted and the export is winding down.
pub const EXPORT_TASK_STATUS_CANCELING: &str = "CANCELING";

/// `ExportTask$Status` — the export was cancelled. Terminal.
pub const EXPORT_TASK_STATUS_CANCELED: &str = "CANCELED";

/// `ExportTask$Status` — the export finished writing. Terminal.
pub const EXPORT_TASK_STATUS_COMPLETE: &str = "COMPLETE";

/// `ExportTask$Status` — the export stopped on an error. Terminal. The record then also
/// carries the `FailureCause` that stopped it.
pub const EXPORT_TASK_STATUS_FAILED: &str = "FAILED";

/// Every status an export task reports, as `ExportTask$Status` lists them: "CANCELED
/// CANCELING COMPLETE FAILED IN_PROGRESS STARTING". The lowercase of each is also what
/// `DescribeExportTasks`'s `status` filter accepts ("Must be lowercase").
pub const EXPORT_TASK_STATUS_VALUES: [&str; 6] = [
    EXPORT_TASK_STATUS_CANCELED,
    EXPORT_TASK_STATUS_CANCELING,
    EXPORT_TASK_STATUS_COMPLETE,
    EXPORT_TASK_STATUS_FAILED,
    EXPORT_TASK_STATUS_IN_PROGRESS,
    EXPORT_TASK_STATUS_STARTING,
];

/// The statuses an export task does not leave on its own: "Any data that has already been
/// written to the S3 bucket isn't removed", and nothing moves a finished export again.
const TERMINAL_STATUSES: [&str; 3] = [
    EXPORT_TASK_STATUS_CANCELED,
    EXPORT_TASK_STATUS_COMPLETE,
    EXPORT_TASK_STATUS_FAILED,
];

/// `ExportTask$PercentProgress` of an export that finished: all of the source was
/// exported.
pub const COMPLETE_PERCENT_PROGRESS: i32 = 100;

/// `ExportTask$PercentProgress` of an export that has not finished writing yet: nothing of
/// it is reported exported until the export is complete, which is what a caller polls the
/// pair of members for.
pub const INITIAL_PERCENT_PROGRESS: i32 = 0;

/// The sentence an `ExportTaskNotFoundFault` carries for an `ExportTaskIdentifier` that
/// names no stored export task.
///
/// Amazon RDS words this fault as a statement about the export task, naming the identifier
/// the request carried — unquoted — inside it, so a caller matching the sentence against
/// what it sent finds its own identifier there. It lives here beside the family's other
/// rules so every operation that resolves an `ExportTaskIdentifier` words the one fault the
/// same way.
pub fn export_task_not_found_message(named: &str) -> String {
    format!("The export task {named} doesn't exist.")
}

/// The sentence an `ExportTaskAlreadyExistsFault` carries: "You can't start an export task
/// that's already running", named with the identifier the request asked to start a second
/// export under.
pub fn export_task_already_exists_message(named: &str) -> String {
    format!("The export task {named} already exists.")
}

/// The longest `ExportTaskIdentifier` this service stores. The member publishes no bound of
/// its own and the value is stored and keyed by, so it carries this emulator's cap for a
/// stored identifier — the same 255 characters every other identifier of this store is
/// bounded by.
pub const MAX_EXPORT_TASK_IDENTIFIER_LEN: usize = 255;

/// How many export tasks one region's store holds. Export tasks are records this emulator
/// keeps in memory, so the family is bounded like every other one here; the bound is this
/// emulator's, not a quota AWS publishes.
pub const EXPORT_TASK_QUOTA_PER_REGION: usize = 100;

/// What a request that would put a region past [`EXPORT_TASK_QUOTA_PER_REGION`] export
/// tasks is refused with.
pub fn export_task_quota_message(region: &str) -> String {
    format!(
        "This emulator holds at most {EXPORT_TASK_QUOTA_PER_REGION} export tasks per region, \
         and {region} already holds that many."
    )
}

/// What an `InvalidExportOnlyFault` carries for an `ExportOnly` entry outside the forms the
/// member publishes: it names the entry that cannot be read and the forms one may take.
pub fn invalid_export_only_message(entry: &str) -> String {
    format!(
        "The export {entry} is invalid. Each value of ExportOnly must name a database, \
         database.table, database.schema or database.schema.table."
    )
}

/// What an `InvalidExportSourceStateFault` carries: the one sentence the model states for
/// that error class, which states the single fact and has no variable part — the state the
/// source is in is reported by the read of the source itself, not spliced into this sentence.
pub fn invalid_export_source_state_message() -> String {
    "The state of the export snapshot is invalid for exporting to an Amazon S3 bucket.".to_string()
}

/// What an `InvalidS3BucketFault` carries for a destination bucket the export cannot write
/// to AT ALL — one this Region's object store holds nothing under, and one whose name is no
/// bucket name: Amazon RDS states the one fact and the two conditions that produce it, and
/// the only variable part is the `S3BucketName` the request carried.
pub fn invalid_s3_bucket_message(bucket: &str) -> String {
    format!(
        "The S3 bucket {bucket} isn't valid. Either it isn't located in the current AWS \
         Region or it doesn't exist."
    )
}

/// The same fault's SENTENCE does not cover a bucket that EXISTS outside the export's
/// Region: that one is refused as the parameter value it is, with the one condition stated
/// rather than the pair above. The bucket name is the only variable part — neither Region is
/// named.
pub fn cross_region_export_bucket_message(bucket: &str) -> String {
    format!("The S3 bucket {bucket} isn't located in the current AWS Region.")
}

/// What an `IamRoleNotFoundFault` carries: the export principal cannot assume the role the
/// request named — because the role's trust policy does not admit it, or because IAM holds
/// no such role — which is the one fault both conditions are reported as, naming the
/// principal and the `IamRoleArn` the request carried.
pub fn export_role_not_found_message(role_arn: &str) -> String {
    format!(
        "The principal {} isn't allowed to assume the IAM role {role_arn} or the IAM role \
         {role_arn} doesn't exist.",
        super::EXPORT_SERVICE_PRINCIPAL
    )
}

/// What an `IamRoleMissingPermissionsFault` carries: the role can be assumed but is not
/// authorized to perform the object write the export makes on the destination bucket.
///
/// Amazon RDS words this fault as ONE statement of the authorization that is missing — the
/// role, the action and the bucket — and nothing else: the two variable parts are the
/// request's `IamRoleArn` and `S3BucketName`, and the action is the object write the export
/// performs ([`super::EXPORT_OBJECT_WRITE_ACTION`]). It prescribes no remedy and says
/// nothing about WHICH policy withheld the authorization, so both halves of the decision —
/// a role whose own policies do not grant the write, and a destination bucket whose policy
/// denies it — report the same sentence.
pub fn export_role_missing_permissions_message(role_arn: &str, bucket: &str) -> String {
    format!(
        "The IAM role {role_arn} isn't authorized to call {} on the S3 bucket {bucket}.",
        super::EXPORT_OBJECT_WRITE_ACTION
    )
}

/// What a `KMSKeyNotAccessibleFault` carries where a CREATE names a `KmsKeyId` KMS will not
/// encrypt the resource under: the key the request SPECIFIED, named in the spelling the
/// request carried, followed by the three reasons the one fault covers.
pub fn kms_key_not_accessible_message(key_id: &str) -> String {
    format!(
        "The specified KMS key [{key_id}] does not exist, is not enabled or you do not have \
         permissions to access it."
    )
}

/// The same fault on a COPY, where the key the request names is the key the COPY is
/// encrypted under rather than the only key in the request: a copy also has the source's
/// key, so Amazon RDS words this one about the TARGET key, and otherwise identically to
/// [`kms_key_not_accessible_message`].
pub fn target_kms_key_not_accessible_message(key_id: &str) -> String {
    format!(
        "The target KMS key [{key_id}] does not exist, is not enabled or you do not have \
         permissions to access it."
    )
}

/// What the same fault carries on the EXPORT path, where `KmsKeyId` is the key the exported
/// data is encrypted with: Amazon RDS words this one as a statement about the key itself,
/// naming the identifier the request carried and nothing else.
pub fn export_kms_key_not_accessible_message(key_id: &str) -> String {
    format!("The KMS key {key_id} doesn't exist or is disabled.")
}

/// The same fault where the key EXISTS but the role the export runs under is not authorized
/// to USE it: "the caller of this operation must be authorized to run the following
/// operations" on the key (`KmsKeyId`), and a key whose own policy grants that role none of
/// them encrypts nothing.
///
/// Worded as Amazon RDS words a missing authorization on an export — the role, the action
/// and the resource the action is not granted on — so the variable parts are the request's
/// `IamRoleArn`, the KMS action the export has to perform and the request's `KmsKeyId`.
pub fn export_kms_key_not_authorized_message(role_arn: &str, action: &str, key_id: &str) -> String {
    format!("The IAM role {role_arn} isn't authorized to call {action} on the KMS key {key_id}.")
}

/// What a request whose `SourceArn` names a resource of ANOTHER Region is refused with.
///
/// Amazon RDS is regional: an export reads its source at the endpoint the call was made to,
/// so an ARN whose Region segment is not that endpoint's names nothing this call can export,
/// which this Query API answers `InvalidParameterValue`. The sentence names the ARN the
/// request carried and nothing else — not the Region it names, and not the one the call was
/// made in.
pub fn cross_region_export_source_message(source_arn: &str) -> String {
    format!("The resource {source_arn} is located in a different AWS Region.")
}

/// `ExportTask$WarningMessage` for an export the object store would not take every object of:
/// "a warning about the snapshot or cluster export task", naming how much of it is missing
/// from the destination so a caller reading the task knows the export is incomplete.
pub fn partial_export_warning(unwritten: usize, bucket: &str) -> String {
    format!(
        "{unwritten} of the exported files could not be written to the Amazon S3 bucket \
         {bucket}, so the export is incomplete."
    )
}

/// What a request whose `SourceArn` names no exportable Amazon RDS resource is refused
/// with: the member is "the Amazon Resource Name (ARN) of the snapshot or cluster to export
/// to Amazon S3", so a value that is no such ARN names nothing to export.
pub fn invalid_export_source_arn_message(source_arn: &str) -> String {
    format!(
        "The parameter SourceArn must name the ARN of a DB snapshot, DB cluster snapshot or \
         DB cluster to export; got {source_arn}."
    )
}

/// The ARN resource type of a DB SNAPSHOT, as Amazon RDS spells it in an ARN
/// (`arn:aws:rds:<region>:<account>:snapshot:<identifier>`).
pub const EXPORT_SOURCE_DB_SNAPSHOT: &str = "snapshot";

/// The ARN resource type of a DB CLUSTER snapshot
/// (`arn:aws:rds:<region>:<account>:cluster-snapshot:<identifier>`).
pub const EXPORT_SOURCE_DB_CLUSTER_SNAPSHOT: &str = "cluster-snapshot";

/// The ARN resource type of a DB CLUSTER
/// (`arn:aws:rds:<region>:<account>:cluster:<identifier>`).
pub const EXPORT_SOURCE_DB_CLUSTER: &str = "cluster";

/// `ExportTask$SourceType` for an export of a snapshot, as `ExportSourceType` publishes it
/// ([`crate::ops::types::EXPORT_SOURCE_TYPE_VALUES`]).
pub const EXPORT_SOURCE_TYPE_SNAPSHOT: &str = "SNAPSHOT";

/// `ExportTask$SourceType` for an export of a DB cluster.
pub const EXPORT_SOURCE_TYPE_CLUSTER: &str = "CLUSTER";

/// The `(resource type, name)` an export's `SourceArn` names, for the three Amazon RDS ARN
/// forms an export can read: a DB snapshot, a DB cluster snapshot or a DB cluster.
///
/// `None` for a value that is not an Amazon RDS ARN of one of those types — the ARN grammar
/// itself is [`::service_common::arn`]'s, this crate's one ARN parse, so nothing here reads
/// the six fields again.
#[must_use]
pub fn export_source_of(source_arn: &str) -> Option<(&'static str, &str)> {
    let arn = ::service_common::arn::parse_of(source_arn, super::ARN_NAMESPACE)?;
    let (resource_type, name) = arn.resource.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    let resource_type = match resource_type {
        EXPORT_SOURCE_DB_SNAPSHOT => EXPORT_SOURCE_DB_SNAPSHOT,
        EXPORT_SOURCE_DB_CLUSTER_SNAPSHOT => EXPORT_SOURCE_DB_CLUSTER_SNAPSHOT,
        EXPORT_SOURCE_DB_CLUSTER => EXPORT_SOURCE_DB_CLUSTER,
        _ => return None,
    };
    Some((resource_type, name))
}

/// The id the part files of ONE export carry, so the objects of two exports of the same
/// source do not collide.
///
/// Amazon RDS names each part file after the export that wrote it
/// (`part-00000-<id>-c000.gz.parquet`); the id here is a digest of the export task's own
/// identifier, so the objects of one export are named deterministically from the export
/// that wrote them and two exports never write the same key.
#[must_use]
pub fn export_part_id(export_task_identifier: &str) -> String {
    use ::sha2::{Digest, Sha256};

    let digest = Sha256::digest(format!("export:{export_task_identifier}"));
    let hex = ::hex::encode(&digest[..16]);
    // The 8-4-4-4-12 form AWS's own part ids carry.
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A persisted snapshot or cluster export task.
///
/// Every member of the modelled `ExportTask` shape is stored rather than recomputed per
/// read: an export task has no identifier-derived fields (no ARN of its own), and the
/// progress fields belong to the export that is writing the objects, so a reader reports
/// exactly what the writer last left.
#[derive(Debug, Clone)]
pub struct ExportTaskRecord {
    /// `ExportTaskIdentifier`, which is half this record's key.
    pub identifier: String,
    /// The region whose credential scope the request that started the export was signed
    /// with; the scope a report of it is visible in.
    pub region: String,
    /// `SourceArn`: the snapshot or cluster being exported.
    pub source_arn: String,
    /// `SourceType`, one of [`crate::ops::types::EXPORT_SOURCE_TYPE_VALUES`].
    pub source_type: String,
    /// `ExportOnly`: the databases, schemas or tables the export is limited to, empty when
    /// the whole source is exported.
    pub export_only: Vec<String>,
    /// `SnapshotTime`: when the exported snapshot was created.
    pub snapshot_time: ::aws_smithy_types::DateTime,
    /// `TaskStartTime`.
    pub task_start_time: ::aws_smithy_types::DateTime,
    /// `TaskEndTime`, once the task has reached a terminal status.
    pub task_end_time: Option<::aws_smithy_types::DateTime>,
    /// `S3Bucket`.
    pub s3_bucket: String,
    /// `S3Prefix`.
    pub s3_prefix: Option<String>,
    /// `IamRoleArn`.
    pub iam_role_arn: String,
    /// `KmsKeyId`.
    pub kms_key_id: String,
    /// `Status`, one of [`EXPORT_TASK_STATUS_VALUES`].
    pub status: String,
    /// The status change the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `PercentProgress`: how much of the source the export has written.
    pub percent_progress: i32,
    /// `TotalExtractedDataInGB`.
    pub total_extracted_data_in_gb: i32,
    /// `FailureCause`, for a task that failed.
    pub failure_cause: Option<String>,
    /// `WarningMessage`.
    pub warning_message: Option<String>,
}

impl ExportTaskRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as every other record's `settle` does.
    ///
    /// A task that reaches a terminal status has ENDED, so this is also where
    /// `TaskEndTime` is taken: the instant the clock made the terminal status due is the
    /// instant the task stopped, whether or not anybody was reading.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        if self
            .pending_status
            .as_ref()
            .is_none_or(|pending| now < pending.due_at)
        {
            return false;
        }
        let pending = self.pending_status.take().expect("checked just above");
        match pending.change {
            PendingChange::Status(status) => {
                self.status = status;
                // An export that has STARTED writing is not finished: the clock owes it the
                // `COMPLETE` it ends in, so a caller polling the task walks the same
                // STARTING → IN_PROGRESS → COMPLETE progression the real service reports.
                if self.status == EXPORT_TASK_STATUS_IN_PROGRESS {
                    self.pending_status =
                        Some(PendingStatus::in_a_moment(EXPORT_TASK_STATUS_COMPLETE));
                }
                if self.status == EXPORT_TASK_STATUS_COMPLETE {
                    // "The progress of the snapshot or cluster export task as a
                    // percentage": an export that finished exported all of its source.
                    self.percent_progress = COMPLETE_PERCENT_PROGRESS;
                }
                if self.is_terminal() && self.task_end_time.is_none() {
                    self.task_end_time = Some(now_as_wire_timestamp());
                }
            }
            PendingChange::Removal => return true,
        }
        false
    }

    /// Whether this task has stopped for good — see [`TERMINAL_STATUSES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `CancelExportTask` can still act on this task: "You can't cancel an export
    /// task that has completed", and a cancellation already accepted is not accepted
    /// twice.
    pub fn is_cancelable(&self) -> bool {
        !self.is_terminal() && self.status != EXPORT_TASK_STATUS_CANCELING
    }

    /// Accept a cancellation: the task reports `CANCELING` and the clock owes it
    /// `CANCELED`, so a caller reading straight after the cancel still sees the transient
    /// status, exactly as it would against the real service.
    pub fn begin_cancellation(&mut self) {
        self.status = EXPORT_TASK_STATUS_CANCELING.to_string();
        self.pending_status = Some(PendingStatus::in_a_moment(EXPORT_TASK_STATUS_CANCELED));
    }

    /// Project the stored record onto the modelled `ExportTask` shape — the output of
    /// `CancelExportTask` and `StartExportTask`, and one entry of
    /// `DescribeExportTasks$ExportTasks`.
    ///
    /// `ExportOnly` is reported as the stored list, EMPTY where the whole source is
    /// exported, because that is what the member documents: "The data exported from the
    /// snapshot or cluster", which is all of it when the request named nothing.
    pub fn to_wire(&self) -> CancelExportTaskOutput {
        CancelExportTaskOutput {
            export_task_identifier: Some(self.identifier.clone()),
            source_arn: Some(self.source_arn.clone()),
            export_only: Some(self.export_only.clone()),
            snapshot_time: Some(self.snapshot_time),
            task_start_time: Some(self.task_start_time),
            task_end_time: self.task_end_time,
            s3_bucket: Some(self.s3_bucket.clone()),
            s3_prefix: self.s3_prefix.clone(),
            iam_role_arn: Some(self.iam_role_arn.clone()),
            kms_key_id: Some(self.kms_key_id.clone()),
            status: Some(self.status.clone()),
            percent_progress: Some(self.percent_progress),
            total_extracted_data_in_gb: Some(self.total_extracted_data_in_gb),
            failure_cause: self.failure_cause.clone(),
            warning_message: self.warning_message.clone(),
            source_type: Some(self.source_type.clone()),
        }
    }
}
