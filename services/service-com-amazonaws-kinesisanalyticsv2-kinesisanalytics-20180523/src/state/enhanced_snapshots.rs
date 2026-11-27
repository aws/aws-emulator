// SPDX-License-Identifier: CC0-1.0

//! The account's ENHANCED snapshots, and the operations performed on them.
//!
//! An enhanced snapshot is a resource of the ACCOUNT rather than of one application —
//! "Snapshots are not scoped to an application, so you can delete a snapshot independently
//! of the application that it was created from" (`DeleteSnapshot`'s doc) — so it is held
//! here, in a map of its own beside [`super::Application`], and not on the application it
//! was taken from the way a legacy [`super::Snapshot`] is.
//!
//! `CreateSnapshot` and `ImportSnapshot` write a [`EnhancedSnapshot`]; `DescribeSnapshot`
//! and `ListSnapshots` report it; `ExportSnapshot` and `DeleteSnapshot` add an operation to
//! it; `DescribeSnapshotOperation` and `ListSnapshotOperations` report those operations. The
//! projections every one of those readers needs ([`EnhancedSnapshot::details`],
//! [`EnhancedSnapshot::info`], [`SnapshotOperationRecord::details`],
//! [`SnapshotOperationRecord::info`]) live here, so no handler re-derives one.
//!
//! Each of those four writing operations is ASYNCHRONOUS: it answers while its operation is
//! `IN_PROGRESS` and the clock owes the rest — the snapshot becoming `READY`, the snapshot
//! being removed, the exported prefix being written. The span is the one this crate already
//! settles every transition at ([`super::TRANSITION_DELAY`]), and
//! [`EnhancedSnapshot::settle`] materialises the change at the instant it was OWED, so a
//! reporting operation reports it without the read being what caused it.

use std::collections::BTreeMap;
use std::time::Instant;

use ::aws_smithy_types::DateTime;

use super::{TRANSITION_DELAY, SNAPSHOT_CREATING, SNAPSHOT_READY};
use crate::ops::types::{
    ApplicationOperationDescription, EncryptionConfigurationDescription, S3PrefixLocation,
    SnapshotEncryptionConfiguration, SnapshotInfo, SnapshotInfoDetails,
    SnapshotOperationInfo, SnapshotOperationInfoDetails, SnapshotOperationLocationDescription,
    SnapshotOperationS3PrefixLocation, SnapshotSourceDescription, SourceApplicationDescription,
};

/// The `SnapshotStatus` values an enhanced snapshot takes beyond the two a legacy snapshot
/// drives ([`SNAPSHOT_CREATING`], [`SNAPSHOT_READY`]), spelled as the model's
/// `SnapshotStatus` enum does (`smithy_query(query="shape",shape="SnapshotStatus")`).
pub const SNAPSHOT_DELETING: &str = "DELETING";
pub const SNAPSHOT_FAILED: &str = "FAILED";

/// `SnapshotOperationStatus` values, from the model's enum of that name.
pub const SNAPSHOT_OPERATION_IN_PROGRESS: &str = "IN_PROGRESS";
pub const SNAPSHOT_OPERATION_SUCCESSFUL: &str = "SUCCESSFUL";

/// `SnapshotOperation` values, from the model's enum of that name: the API operation that
/// was performed on the snapshot, spelled as the operation is.
pub const SNAPSHOT_OPERATION_CREATE: &str = "CreateSnapshot";
pub const SNAPSHOT_OPERATION_DELETE: &str = "DeleteSnapshot";
pub const SNAPSHOT_OPERATION_IMPORT: &str = "ImportSnapshot";
pub const SNAPSHOT_OPERATION_EXPORT: &str = "ExportSnapshot";

/// The `SnapshotInitiator` of a snapshot a CALLER asked for: "`USER` indicates a snapshot
/// that you created or imported" (`SnapshotInfo$SnapshotInitiator`). The other value,
/// `SYSTEM`, marks a snapshot the service took on its own, which nothing in this emulator
/// does.
pub const SNAPSHOT_INITIATOR_USER: &str = "USER";

/// The only `SnapshotFormat` the model declares
/// (`smithy_query(query="shape",shape="SnapshotFormat")`), and so the format of every
/// snapshot this service holds.
pub const SNAPSHOT_FORMAT_CANONICAL: &str = "CANONICAL";

/// The only `StorageLocationType` the model declares: "The only supported value is `S3`"
/// (`ExportDestinationConfiguration$DestinationType`).
pub const STORAGE_LOCATION_S3: &str = "S3";

/// The `KeyType` snapshot data is encrypted under when the caller named no key of their own:
/// "If you don't specify this parameter, Managed Service for Apache Flink uses a key of type
/// `AWS_OWNED_KEY`" (`ExportDestinationConfiguration$DestinationEncryptionConfiguration`).
pub const AWS_OWNED_KEY: &str = "AWS_OWNED_KEY";

/// The name prefix no caller may create or import a snapshot under: "Names that begin with
/// SYSTEMSNAPSHOT are reserved" (`CreateSnapshotRequest$SnapshotName`).
pub const RESERVED_SNAPSHOT_NAME_PREFIX: &str = "SYSTEMSNAPSHOT";

/// The `RuntimeEnvironment` generation an application has to run to take enhanced snapshots:
/// "To use `ENHANCED_SNAPSHOTS`, the application must use Apache Flink version 1.20 or
/// later" (`ApplicationSnapshotConfiguration$SnapshotMode`).
pub const MIN_ENHANCED_SNAPSHOT_GENERATION: (u32, u32) = (1, 20);

/// The `SnapshotMode` an application whose snapshots are enhanced ones runs under, from the
/// model's `SnapshotMode` enum.
pub const ENHANCED_SNAPSHOTS_MODE: &str = "ENHANCED_SNAPSHOTS";

/// The ceiling on how many enhanced snapshots this store holds. Same reasoning as
/// [`super::MAX_APPLICATIONS`]: a snapshot is held for the life of the process and only a
/// caller adds one, so the count needs a maximum — and `CreateSnapshot` and `ImportSnapshot`
/// both declare the modelled `LimitExceededException` (HTTP 400) for a caller that reaches
/// it, so the refusal is the service's own rather than a trim a reader could not explain.
pub const MAX_ENHANCED_SNAPSHOTS: usize = 1_000;

/// The ceiling on how many operation records one snapshot retains, trimmed oldest-first.
/// Same reasoning as [`super::MAX_OPERATIONS_PER_APPLICATION`]: the log is one no caller can
/// drain, and no snapshot operation declares a quota error for reading it, so losing the
/// oldest COMPLETED record is not a refusal a real client would see.
pub const MAX_OPERATIONS_PER_SNAPSHOT: usize = 200;

/// The type of resource an enhanced snapshot's ARN names, the `snapshot` of
/// `snapshot/<name>`.
const SNAPSHOT_RESOURCE_TYPE: &str = "snapshot";

/// The ARN of an enhanced snapshot: `arn:aws:kinesisanalytics:<region>:<account>:snapshot/<name>`.
///
/// The name alone identifies the resource — "The name must be unique among the snapshots in
/// your account and Region" (`CreateSnapshotRequest$SnapshotName`) — so the ARN names the
/// snapshot directly rather than through the application it was taken from, which it is not
/// scoped to. `kinesisanalytics` is this service's `arnNamespace`
/// (`smithy_query(query="service_info")`) and the account segment is the emulator's
/// canonical one, both of which [`super::application_arn`] already mints an application's ARN
/// from.
pub fn snapshot_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_NAMESPACE,
        region,
        &format!("{SNAPSHOT_RESOURCE_TYPE}/{name}"),
    )
}

/// Whether an application on `runtime` can run its snapshots as ENHANCED ones: a Managed
/// Service for Apache Flink application ([`super::is_flink_runtime`]) on Apache Flink 1.20 or
/// later ([`MIN_ENHANCED_SNAPSHOT_GENERATION`]).
///
/// The generation is read off the `RuntimeEnvironment` value itself
/// ([`super::runtime_generation`]), so a generation the enum gains later is judged by the
/// same rule rather than needing an entry here.
pub fn runtime_takes_enhanced_snapshots(runtime: &str) -> bool {
    super::is_flink_runtime(runtime)
        && super::runtime_generation(runtime)
            .is_some_and(|generation| generation >= MIN_ENHANCED_SNAPSHOT_GENERATION)
}

/// The `SnapshotMode` a stored application's snapshots are taken under: the one its
/// configuration description carries, else the one an application that selected none runs
/// under ([`super::DEFAULT_SNAPSHOT_MODE`]).
///
/// Read off the STORED description, so what decides is the mode the application runs under
/// rather than a value a request carried.
pub fn application_snapshot_mode(application: &super::Application) -> String {
    application
        .configuration
        .as_ref()
        .and_then(|configuration| {
            configuration
                .application_snapshot_configuration_description
                .as_ref()
        })
        .and_then(|snapshots| snapshots.snapshot_mode.clone())
        .unwrap_or_else(|| super::DEFAULT_SNAPSHOT_MODE.to_string())
}

/// Whether a stored application takes ENHANCED snapshots: the mode its configuration
/// describes is the enhanced one.
pub fn application_takes_enhanced_snapshots(application: &super::Application) -> bool {
    application_snapshot_mode(application) == ENHANCED_SNAPSHOTS_MODE
}

/// How this service words a request refused because the application's runtime does not run
/// enhanced snapshots: the runtime, read off the stored application or taken from the request
/// that would put the application on it, and the version the mode needs.
pub fn enhanced_snapshots_unsupported_message(runtime: &str) -> String {
    let (major, minor) = MIN_ENHANCED_SNAPSHOT_GENERATION;
    format!(
        "{ENHANCED_SNAPSHOTS_MODE} is not supported for applications with runtime environment \
         {runtime}. The application must use Apache Flink version {major}.{minor} or later."
    )
}

/// How this service words a snapshot operation refused because the application does not run
/// its snapshots as enhanced ones: the application the request named and the mode the
/// operation needs it to be in.
pub fn enhanced_snapshots_not_configured_message(application: &str) -> String {
    format!(
        "Application {application} does not use the {ENHANCED_SNAPSHOTS_MODE} snapshot mode. \
         Update the application to that snapshot mode and try again."
    )
}

/// How this service words a snapshot-mode update that would take an application BACK to the
/// legacy mode: "You can't use `UpdateApplication` to change the snapshot mode back to
/// `APPLICATION_SNAPSHOTS_LEGACY`" (`ApplicationSnapshotConfigurationUpdate$SnapshotModeUpdate`).
pub fn snapshot_mode_not_reversible_message(application: &str) -> String {
    format!(
        "Application {application} cannot be updated from {ENHANCED_SNAPSHOTS_MODE} to {}. Use \
         RollbackApplication instead.",
        super::DEFAULT_SNAPSHOT_MODE
    )
}

/// How this service words a sentence about one ENHANCED snapshot: the snapshot the request
/// named, quoted, and what is true of it, over the account it was resolved in — the scope a
/// snapshot that is not scoped to an application is unique within.
fn enhanced_snapshot_message(snapshot: &str, fact: &str) -> String {
    format!(
        "Snapshot '{snapshot}' {fact} under account '{}'.",
        ::engine::emulator_account_id()
    )
}

/// An enhanced snapshot this account holds no record of, for the operations that read or act
/// on one by name.
pub fn missing_enhanced_snapshot_message(snapshot: &str) -> String {
    enhanced_snapshot_message(snapshot, "does not exist")
}

/// How this service words a name this account already holds an enhanced snapshot under, for
/// the operations that create or import one: "You can't reuse the name of an existing
/// snapshot, including a snapshot in the FAILED status, until you delete that snapshot"
/// (`CreateSnapshotRequest$SnapshotName`).
pub fn existing_enhanced_snapshot_message(snapshot: &str) -> String {
    enhanced_snapshot_message(snapshot, "already exists")
}

/// How this service words a lookup miss on one OPERATION of an enhanced snapshot: the id the
/// caller asked for and the snapshot it was looked for in, in the sentence shape
/// [`super::missing_application_version_message`] uses for one version of an application.
pub fn missing_snapshot_operation_message(operation_id: &str, snapshot: &str) -> String {
    format!("Snapshot operation '{operation_id}' does not exist for snapshot {snapshot}.")
}

/// How this service words a name whose prefix the service reserves for its own snapshots.
pub fn reserved_snapshot_name_message(snapshot: &str) -> String {
    format!(
        "The snapshot name '{snapshot}' is not valid. Names that begin with \
         {RESERVED_SNAPSHOT_NAME_PREFIX} are reserved."
    )
}

/// How this service words a snapshot operation refused because the snapshot is not in a
/// status that operation is accepted from: the status it is in, read off the stored snapshot,
/// and the statuses the operation needs.
///
/// One sentence for the delete ("You can delete a snapshot in the `READY` or `FAILED`
/// status") and for the export ("The snapshot must be in the `READY` status"), so two
/// operations cannot report the same ineligible snapshot differently.
pub fn ineligible_snapshot_status_message(
    snapshot: &str,
    status: &str,
    required: &[&str],
) -> String {
    format!(
        "Snapshot '{snapshot}' is in {status} status. This operation is only allowed when the \
         snapshot is in {} status.",
        required.join(" or ")
    )
}

/// How this service words a `SnapshotCreationTimestamp` that does not identify the snapshot
/// the caller addressed the delete to.
///
/// The stamp is the caller's proof that it is deleting the snapshot it last read — "Use the
/// `DescribeSnapshot` operation to get this value"
/// (`DeleteSnapshotRequest$SnapshotCreationTimestamp`) — so a value that does not match sends
/// the caller back to that reader rather than telling it the value is malformed. The stored
/// stamp is not stated: naming it would hand a client a way past the check.
pub fn stale_snapshot_creation_timestamp_message(snapshot: &str) -> String {
    format!(
        "The snapshot with name: '{snapshot}' has a different creation timestamp than the one \
         you provided. Please call DescribeSnapshot to fetch the valid creation timestamp."
    )
}

/// How this service words an operation refused because one of its kind is already running on
/// the resource: `CreateSnapshot`'s "An application can have only one `CreateSnapshot`
/// operation in progress at a time" and `ExportSnapshot`'s "A snapshot can have only one
/// export in progress at a time".
pub fn operation_already_in_progress_message(operation: &str, resource: &str) -> String {
    format!(
        "The number of allowed resources has been exceeded: {resource} already has a \
         {operation} operation in progress. Wait for that operation to finish and try again."
    )
}

/// How this service words a create or import refused because the store is full.
pub fn snapshot_ceiling_message() -> String {
    format!(
        "The number of allowed resources has been exceeded: at most {MAX_ENHANCED_SNAPSHOTS} \
         snapshots can be stored"
    )
}

/// How this service words a bucket ARN that is not the ARN of an Amazon S3 bucket at all — a
/// value that names no location to read or write snapshot data at.
pub fn not_a_bucket_arn_message(bucket_arn: &str) -> String {
    format!("The BucketARN '{bucket_arn}' is not the ARN of an Amazon S3 bucket")
}

/// How this service words a snapshot location whose bucket Amazon S3 ANSWERED it does not
/// hold: "The bucket must be owned by your account and must be in the same Region as the
/// request" (`ExportDestinationConfiguration$DestinationS3PrefixLocation` and
/// `ImportSourceConfiguration$SourceS3PrefixLocation`).
pub fn missing_bucket_message(bucket_arn: &str) -> String {
    format!(
        "Bucket '{bucket_arn}' does not exist. The bucket must be owned by your account and \
         must be in the same Region as the request."
    )
}

/// The refusal a snapshot location earns because its bucket is not one this account's object
/// store holds, or `None` when the location names a bucket Amazon S3 answered for.
///
/// An import reads its savepoint data from the location and an export writes the snapshot
/// data to it, so both RESOLVE the bucket where Amazon S3 holds it before the operation is
/// accepted — otherwise a caller is told data moved to or from a location that does not
/// exist. The question is the tree's shared one
/// ([`::service_common::s3_client::bucket_is_absent`]), put over the emulator's own
/// in-process transport, so it opens no connection and resolves no host, and it refuses only
/// what the object store ANSWERED: an emulator assembled with no Amazon S3 to ask refuses
/// nothing.
///
/// Call it with NO state guard held — it awaits.
pub async fn snapshot_bucket_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket_arn: &str,
) -> Option<String> {
    let Some(bucket) = ::service_common::arn::resource(bucket_arn).filter(|name| !name.is_empty())
    else {
        return Some(not_a_bucket_arn_message(bucket_arn));
    };
    ::service_common::s3_client::bucket_is_absent(ctx, region, bucket)
        .await
        .then(|| missing_bucket_message(bucket_arn))
}

/// One operation performed on an enhanced snapshot.
///
/// The `SnapshotOperationId` `CreateSnapshot`, `ImportSnapshot`, `ExportSnapshot` and
/// `DeleteSnapshot` return is the id of a record held HERE, so the value handed to a client
/// is backed by state rather than minted and forgotten, and `DescribeSnapshotOperation`
/// answers from it.
#[derive(Debug, Clone)]
pub struct SnapshotOperationRecord {
    pub operation_id: String,
    /// A `SnapshotOperation` value: which API operation this record is of.
    pub operation: String,
    /// A `SnapshotOperationStatus` value.
    pub status: String,
    pub start_time: DateTime,
    pub end_time: Option<DateTime>,
    /// When this operation finishes on the monotonic clock, while it is still outstanding.
    pub due_at: Option<Instant>,
    /// The same instant on the wall clock, so the completion timestamp a reader sees is the
    /// instant the change was OWED and does not drift with when it is observed.
    pub due_wall: DateTime,
    /// The encryption of the data this operation READ, which only an import has.
    pub source_encryption: Option<EncryptionConfigurationDescription>,
    /// The encryption of the data this operation WROTE, which an import and an export have.
    pub destination_encryption: Option<EncryptionConfigurationDescription>,
    /// Where this operation read the snapshot data from, which only an import has.
    pub source_location: Option<SnapshotOperationLocationDescription>,
    /// Where this operation wrote the snapshot data to, which only an export has.
    pub destination_location: Option<SnapshotOperationLocationDescription>,
}

impl SnapshotOperationRecord {
    /// The `SnapshotOperationInfo` `ListSnapshotOperations` reports.
    pub fn info(&self) -> SnapshotOperationInfo {
        SnapshotOperationInfo {
            snapshot_operation_id: self.operation_id.clone(),
            snapshot_operation: self.operation.clone(),
            snapshot_operation_status: self.status.clone(),
            start_time: Some(self.start_time),
            end_time: self.end_time,
        }
    }

    /// The `SnapshotOperationInfoDetails` `DescribeSnapshotOperation` reports, over the ARN
    /// of the snapshot the operation was performed on.
    ///
    /// Each optional member is present for the operations the model says carries it and
    /// absent for the rest: the two locations and the two encryption descriptions are the
    /// ones this operation recorded when it was accepted, and
    /// `ApplicationOperationDescription` is "present only for `SystemCreateSnapshot`
    /// operations" — which nothing in this emulator performs, since no snapshot here is taken
    /// by the service on its own.
    pub fn details(&self, snapshot_arn: &str) -> SnapshotOperationInfoDetails {
        SnapshotOperationInfoDetails {
            snapshot_operation_id: self.operation_id.clone(),
            snapshot_operation: self.operation.clone(),
            snapshot_operation_status: self.status.clone(),
            snapshot_arn: snapshot_arn.to_string(),
            source_encryption_configuration_description: self.source_encryption.clone(),
            destination_encryption_configuration_description: self.destination_encryption.clone(),
            snapshot_source_location_description: self.source_location.clone(),
            snapshot_destination_location_description: self.destination_location.clone(),
            application_operation_description: None::<ApplicationOperationDescription>,
            start_time: Some(self.start_time),
            end_time: self.end_time,
            // "This field is present only if `SnapshotOperationStatus` is `FAILED`", and no
            // operation this service accepts fails: every one it does not accept is refused
            // with a modelled error instead.
            snapshot_operation_failure_details: None,
        }
    }
}

/// One stored enhanced snapshot of application state.
#[derive(Debug, Clone)]
pub struct EnhancedSnapshot {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    /// A `SnapshotStatus` enum value.
    pub status: String,
    pub creation_timestamp: DateTime,
    /// "The Flink runtime of the application that the snapshot was created from. This field
    /// is not present for imported snapshots" (`SnapshotInfoDetails$RuntimeEnvironment`).
    pub runtime_environment: Option<String>,
    /// The key the snapshot data is encrypted under.
    pub encryption: Option<SnapshotEncryptionConfiguration>,
    /// A `SnapshotInitiator` enum value.
    pub initiator: String,
    /// A `SnapshotInitiatorOperation` enum value: the operation that created the snapshot.
    pub initiator_operation: String,
    /// The application the snapshot was created from, absent for an imported snapshot.
    pub source_application: Option<SourceApplicationDescription>,
    /// The Amazon S3 location an imported snapshot was read from, absent for one created
    /// from an application.
    pub source_location: Option<S3PrefixLocation>,
    /// A `SnapshotFormat` enum value.
    pub format: String,
    pub tags: BTreeMap<String, String>,
    /// Every operation performed on this snapshot, oldest first, so a walk of the log is in
    /// the order the operations were started on every call and a pagination token can resume
    /// it by naming the operation the next page starts at. Bounded by
    /// [`MAX_OPERATIONS_PER_SNAPSHOT`].
    pub operations: Vec<SnapshotOperationRecord>,
}

impl EnhancedSnapshot {
    /// Apply every change the clock owes this snapshot, if any is due at `now`, and answer
    /// whether the snapshot has been DELETED by one of them.
    ///
    /// The same rule as [`super::Application::settle`]: each change was owed at its
    /// operation's `due_at` whether or not anybody called, so materialising it on a read is
    /// not a read-driven mutation. Everything derived from it — the completion timestamp, the
    /// exported prefix — is stamped at `due_wall`, so repeated reads report identical values.
    fn settle(&mut self, now: Instant) -> bool {
        let due: Vec<usize> = self
            .operations
            .iter()
            .enumerate()
            .filter(|(_, operation)| operation.due_at.is_some_and(|at| now >= at))
            .map(|(index, _)| index)
            .collect();
        let mut deleted = false;
        for index in due {
            let operation = &mut self.operations[index];
            operation.status = SNAPSHOT_OPERATION_SUCCESSFUL.to_string();
            operation.end_time = Some(operation.due_wall);
            operation.due_at = None;
            let kind = operation.operation.clone();
            let operation_id = operation.operation_id.clone();
            match kind.as_str() {
                // The snapshot the operation was taking, or importing, now holds its data.
                SNAPSHOT_OPERATION_CREATE | SNAPSHOT_OPERATION_IMPORT => {
                    self.status = SNAPSHOT_READY.to_string();
                }
                // "The response returns the snapshot ARN with the DELETING status": the
                // snapshot is gone once the delete completes.
                SNAPSHOT_OPERATION_DELETE => deleted = true,
                // "This value includes the unique prefix that Managed Service for Apache
                // Flink appends to KeyPrefix. This field is present only after an
                // ExportSnapshot operation completes successfully"
                // (`SnapshotOperationS3PrefixLocation$ExportedPrefix`). The unique segment is
                // the id of the operation that wrote the data, so the prefix a reader is
                // given names the export it came from and is stable across reads.
                SNAPSHOT_OPERATION_EXPORT => {
                    if let Some(location) = self.operations[index].destination_location.as_mut() {
                        let prefix = &location.s3_prefix_location.key_prefix;
                        location.s3_prefix_location.exported_prefix =
                            Some(format!("{prefix}/{operation_id}"));
                    }
                }
                _ => {}
            }
        }
        deleted
    }

    /// Whether an operation of `kind` is still outstanding on this snapshot.
    pub fn has_operation_in_progress(&self, kind: &str) -> bool {
        self.operations
            .iter()
            .any(|operation| operation.operation == kind && operation.due_at.is_some())
    }

    /// The instant the LAST outstanding operation on this snapshot finishes, if any is.
    ///
    /// `DeleteSnapshot` waits on it: "If a snapshot operation, such as an export, uses the
    /// snapshot, Managed Service for Apache Flink waits for that operation to finish before
    /// it deletes the snapshot."
    pub fn operations_settled_at(&self) -> Option<Instant> {
        self.operations
            .iter()
            .filter_map(|operation| operation.due_at)
            .max()
    }

    /// One operation of this snapshot by id.
    pub fn operation(&self, operation_id: &str) -> Option<&SnapshotOperationRecord> {
        self.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
    }

    /// Append one operation record and answer it, trimming the log oldest-first at
    /// [`MAX_OPERATIONS_PER_SNAPSHOT`].
    fn push_operation(&mut self, record: SnapshotOperationRecord) {
        self.operations.push(record);
        if self.operations.len() > MAX_OPERATIONS_PER_SNAPSHOT {
            let excess = self.operations.len() - MAX_OPERATIONS_PER_SNAPSHOT;
            self.operations.drain(..excess);
        }
    }

    /// The `SnapshotInfo` `ListSnapshots` reports.
    pub fn info(&self) -> SnapshotInfo {
        SnapshotInfo {
            snapshot_name: self.name.clone(),
            snapshot_arn: self.arn.clone(),
            snapshot_status: self.status.clone(),
            snapshot_creation_timestamp: self.creation_timestamp,
            snapshot_initiator: self.initiator.clone(),
            snapshot_initiator_operation: self.initiator_operation.clone(),
            source_application_name: self
                .source_application
                .as_ref()
                .map(|application| application.application_name.clone()),
            snapshot_format: self.format.clone(),
        }
    }

    /// The `SnapshotInfoDetails` `DescribeSnapshot` reports.
    pub fn details(&self) -> SnapshotInfoDetails {
        SnapshotInfoDetails {
            snapshot_name: self.name.clone(),
            snapshot_arn: self.arn.clone(),
            snapshot_description: self.description.clone(),
            snapshot_status: self.status.clone(),
            snapshot_creation_timestamp: self.creation_timestamp,
            runtime_environment: self.runtime_environment.clone(),
            // "The size of the snapshot, in bytes. The value is 0 until the snapshot status
            // is READY" (`SnapshotInfoDetails$SnapshotSizeInBytes`). This emulator runs no
            // Apache Flink job, so a snapshot of one captures no state data and its size is
            // the member's modelled default: a size invented for data nothing wrote would be
            // a number no reader could check.
            snapshot_size_in_bytes: 0,
            snapshot_encryption_configuration_description: self.encryption.clone(),
            snapshot_initiator: self.initiator.clone(),
            snapshot_initiator_operation: self.initiator_operation.clone(),
            // "For a snapshot that was created from an application,
            // `SourceApplicationDescription` is present. For an imported snapshot,
            // `S3PrefixLocation` is present" (`SnapshotSourceDescription`).
            snapshot_source_description: SnapshotSourceDescription {
                s3_prefix_location: self.source_location.clone(),
                source_application_description: self.source_application.clone(),
            },
            snapshot_format: self.format.clone(),
        }
    }
}

/// The location an operation read or wrote snapshot data at, as the model describes one.
pub fn operation_location(location: &S3PrefixLocation) -> SnapshotOperationLocationDescription {
    SnapshotOperationLocationDescription {
        location_type: STORAGE_LOCATION_S3.to_string(),
        s3_prefix_location: SnapshotOperationS3PrefixLocation {
            bucket_arn: location.bucket_arn.clone(),
            key_prefix: location.key_prefix.clone(),
            // Written when the export that wrote the data completes
            // ([`EnhancedSnapshot::settle`]).
            exported_prefix: None,
        },
    }
}

/// The encryption of snapshot data the caller configured, else the key AWS owns, which is
/// what the service encrypts data under when a request names no key ([`AWS_OWNED_KEY`]).
pub fn described_encryption(
    configured: Option<&SnapshotEncryptionConfiguration>,
) -> EncryptionConfigurationDescription {
    match configured {
        Some(encryption) => EncryptionConfigurationDescription {
            key_id: encryption.key_id.clone(),
            key_type: encryption.key_type.clone(),
        },
        None => EncryptionConfigurationDescription {
            key_id: None,
            key_type: AWS_OWNED_KEY.to_string(),
        },
    }
}

/// The encryption an enhanced snapshot is STORED under: the caller's configuration, else the
/// key AWS owns.
pub fn stored_encryption(
    configured: Option<&SnapshotEncryptionConfiguration>,
) -> SnapshotEncryptionConfiguration {
    configured
        .cloned()
        .unwrap_or_else(|| SnapshotEncryptionConfiguration {
            key_id: None,
            key_type: AWS_OWNED_KEY.to_string(),
        })
}

impl super::ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State {
    /// Apply every change the clock owes the enhanced snapshot named, if there is one.
    ///
    /// Call this FIRST in a handler that names a single snapshot, so the operation decides
    /// against the status that snapshot has now. It touches only that entry, so the work a
    /// request does under the state lock does not grow with the store.
    pub fn settle_snapshot_named(&mut self, name: &str) {
        let now = Instant::now();
        let deleted = self
            .snapshots_v2
            .get_mut(name)
            .is_some_and(|snapshot| snapshot.settle(now));
        if deleted {
            self.snapshots_v2.remove(name);
        }
    }

    /// Apply every change the clock owes any enhanced snapshot. For `ListSnapshots`, which
    /// reports on every stored snapshot at once; the walk is bounded by
    /// [`MAX_ENHANCED_SNAPSHOTS`].
    pub fn settle_snapshots(&mut self) {
        let now = Instant::now();
        let mut deleted: Vec<String> = Vec::new();
        for (name, snapshot) in self.snapshots_v2.iter_mut() {
            if snapshot.settle(now) {
                deleted.push(name.clone());
            }
        }
        for name in deleted {
            self.snapshots_v2.remove(&name);
        }
    }

    /// Whether any snapshot of this account is still being created from `application`.
    ///
    /// "An application can have only one `CreateSnapshot` operation in progress at a time"
    /// (`CreateSnapshot`'s doc), judged over the snapshots the store holds rather than a
    /// counter beside them, so the answer follows the snapshots themselves.
    pub fn application_has_snapshot_in_progress(&self, application: &str) -> bool {
        self.snapshots_v2.values().any(|snapshot| {
            snapshot
                .source_application
                .as_ref()
                .is_some_and(|source| source.application_name == application)
                && snapshot.has_operation_in_progress(SNAPSHOT_OPERATION_CREATE)
        })
    }

    /// Store a snapshot that is being CREATED from the application `source` describes, with
    /// its create operation outstanding, and answer the operation's id.
    ///
    /// The snapshot is `CREATING` when this call answers and `READY` once the clock makes
    /// that change due, so a caller reading straight back still sees the transient status —
    /// which is what `DescribeSnapshot` is for: "The response returns the snapshot ARN with
    /// the CREATING status. To check the status of the snapshot, use `DescribeSnapshot`."
    ///
    /// The version, the runtime and the creation timestamp recorded are the application's as
    /// it stands now, so the details a reader gets are about the application that was
    /// snapshotted and not about whatever it is later updated to.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_snapshot_create(
        &mut self,
        source: SourceApplicationDescription,
        runtime_environment: String,
        name: &str,
        arn: String,
        description: Option<String>,
        tags: BTreeMap<String, String>,
        now: DateTime,
    ) -> String {
        let operation_id = self.mint_id(SNAPSHOT_OPERATION_CREATE);
        let snapshot = EnhancedSnapshot {
            name: name.to_string(),
            arn,
            description,
            status: SNAPSHOT_CREATING.to_string(),
            creation_timestamp: now,
            runtime_environment: Some(runtime_environment),
            encryption: Some(stored_encryption(None)),
            initiator: SNAPSHOT_INITIATOR_USER.to_string(),
            initiator_operation: SNAPSHOT_OPERATION_CREATE.to_string(),
            source_application: Some(source),
            source_location: None,
            format: SNAPSHOT_FORMAT_CANONICAL.to_string(),
            tags,
            operations: Vec::new(),
        };
        self.insert_snapshot(
            snapshot,
            SnapshotOperationRecord {
                operation_id: operation_id.clone(),
                operation: SNAPSHOT_OPERATION_CREATE.to_string(),
                status: SNAPSHOT_OPERATION_IN_PROGRESS.to_string(),
                start_time: now,
                end_time: None,
                due_at: Some(Instant::now() + TRANSITION_DELAY),
                due_wall: completion_stamp(now),
                source_encryption: None,
                destination_encryption: None,
                source_location: None,
                destination_location: None,
            },
        );
        operation_id
    }

    /// Store a snapshot that is being IMPORTED from an Amazon S3 location, with its import
    /// operation outstanding, and answer the operation's id.
    ///
    /// The snapshot carries no runtime environment — "This field is not present for imported
    /// snapshots" — and its source is the location the savepoint data was read from.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_snapshot_import(
        &mut self,
        name: &str,
        arn: String,
        description: Option<String>,
        format: String,
        source: S3PrefixLocation,
        source_encryption: EncryptionConfigurationDescription,
        destination_encryption: EncryptionConfigurationDescription,
        tags: BTreeMap<String, String>,
        now: DateTime,
    ) -> String {
        let operation_id = self.mint_id(SNAPSHOT_OPERATION_IMPORT);
        let snapshot = EnhancedSnapshot {
            name: name.to_string(),
            arn,
            description,
            status: SNAPSHOT_CREATING.to_string(),
            creation_timestamp: now,
            runtime_environment: None,
            encryption: Some(SnapshotEncryptionConfiguration {
                key_id: destination_encryption.key_id.clone(),
                key_type: destination_encryption.key_type.clone(),
            }),
            initiator: SNAPSHOT_INITIATOR_USER.to_string(),
            initiator_operation: SNAPSHOT_OPERATION_IMPORT.to_string(),
            source_application: None,
            source_location: Some(source.clone()),
            format,
            tags,
            operations: Vec::new(),
        };
        self.insert_snapshot(
            snapshot,
            SnapshotOperationRecord {
                operation_id: operation_id.clone(),
                operation: SNAPSHOT_OPERATION_IMPORT.to_string(),
                status: SNAPSHOT_OPERATION_IN_PROGRESS.to_string(),
                start_time: now,
                end_time: None,
                due_at: Some(Instant::now() + TRANSITION_DELAY),
                due_wall: completion_stamp(now),
                source_encryption: Some(source_encryption),
                destination_encryption: Some(destination_encryption),
                source_location: Some(operation_location(&source)),
                destination_location: None,
            },
        );
        operation_id
    }

    /// Record an EXPORT of a stored snapshot to an Amazon S3 location and answer the
    /// operation's id, or `None` when no snapshot is stored under `name`.
    ///
    /// The snapshot's own status does not move: an export reads the snapshot's data, so the
    /// snapshot stays `READY` while the export runs and only the operation is outstanding.
    pub fn begin_snapshot_export(
        &mut self,
        name: &str,
        destination: &S3PrefixLocation,
        destination_encryption: EncryptionConfigurationDescription,
        now: DateTime,
    ) -> Option<String> {
        let operation_id = self.mint_id(SNAPSHOT_OPERATION_EXPORT);
        let snapshot = self.snapshots_v2.get_mut(name)?;
        snapshot.push_operation(SnapshotOperationRecord {
            operation_id: operation_id.clone(),
            operation: SNAPSHOT_OPERATION_EXPORT.to_string(),
            status: SNAPSHOT_OPERATION_IN_PROGRESS.to_string(),
            start_time: now,
            end_time: None,
            due_at: Some(Instant::now() + TRANSITION_DELAY),
            due_wall: completion_stamp(now),
            source_encryption: None,
            destination_encryption: Some(destination_encryption),
            source_location: None,
            destination_location: Some(operation_location(destination)),
        });
        Some(operation_id)
    }

    /// Record a DELETE of a stored snapshot and answer the operation's id, or `None` when no
    /// snapshot is stored under `name`.
    ///
    /// The snapshot takes the `DELETING` status now and is removed once the delete completes.
    /// A delete waits on the operations already running on the snapshot — "Managed Service
    /// for Apache Flink waits for that operation to finish before it deletes the snapshot" —
    /// so it becomes due no earlier than the last of them.
    pub fn begin_snapshot_delete(&mut self, name: &str, now: DateTime) -> Option<String> {
        let operation_id = self.mint_id(SNAPSHOT_OPERATION_DELETE);
        let snapshot = self.snapshots_v2.get_mut(name)?;
        let own_due = Instant::now() + TRANSITION_DELAY;
        let due_at = snapshot
            .operations_settled_at()
            .map_or(own_due, |running| running.max(own_due));
        snapshot.status = SNAPSHOT_DELETING.to_string();
        snapshot.push_operation(SnapshotOperationRecord {
            operation_id: operation_id.clone(),
            operation: SNAPSHOT_OPERATION_DELETE.to_string(),
            status: SNAPSHOT_OPERATION_IN_PROGRESS.to_string(),
            start_time: now,
            end_time: None,
            due_at: Some(due_at),
            due_wall: completion_stamp(now),
            source_encryption: None,
            destination_encryption: None,
            source_location: None,
            destination_location: None,
        });
        Some(operation_id)
    }

    /// Store `snapshot` with `operation` as the operation that is bringing it into being.
    fn insert_snapshot(&mut self, mut snapshot: EnhancedSnapshot, operation: SnapshotOperationRecord) {
        snapshot.push_operation(operation);
        self.snapshots_v2.insert(snapshot.name.clone(), snapshot);
    }

    /// The enhanced snapshots of this account in the order `ListSnapshots` reports them:
    /// "The results are sorted in descending order of snapshot creation timestamp."
    ///
    /// Two snapshots created in the same second are ordered by name, so one call's walk is
    /// the next call's walk and a pagination token naming a position in it resumes the same
    /// sequence.
    pub fn snapshots_newest_first(&self) -> Vec<&EnhancedSnapshot> {
        let mut snapshots: Vec<&EnhancedSnapshot> = self.snapshots_v2.values().collect();
        snapshots.sort_by(|left, right| {
            right
                .creation_timestamp
                .secs()
                .cmp(&left.creation_timestamp.secs())
                .then_with(|| left.name.cmp(&right.name))
        });
        snapshots
    }
}

/// The wall-clock instant an operation accepted at `now` completes at: the next whole
/// second, the resolution this service reports timestamps at, which is the same stamp
/// [`super::ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State::begin_transition`]
/// gives an application's clock-owed transition.
fn completion_stamp(now: DateTime) -> DateTime {
    DateTime::from_secs(now.secs() + 1)
}
