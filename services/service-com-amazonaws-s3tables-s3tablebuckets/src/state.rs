// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3tables-s3tablebuckets service.
//!
//! The S3 Tables control plane is, at this level, a store of **table buckets**,
//! each holding **namespaces**, each holding **tables**. Everything the handlers
//! in `src/ops/` answer with is reconstructed from the records below: nothing is
//! echoed straight back out of a request and nothing is canned.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// A table's Iceberg metadata: the document the create writes for the table,
/// what a catalog commit makes of it, and the two questions asked of it — what
/// an Iceberg catalog read answers with, and whether the table carries a sort
/// order.
mod iceberg;
pub use iceberg::*;

/// The tables an Amazon Web Services service creates in a table bucket on the
/// account's behalf, and the one place such a table is written.
mod service_managed;
pub use service_managed::*;

/// The maintenance jobs that run on a table: which run, the schedule each runs
/// under, and the status a read of them reports.
mod maintenance_jobs;
pub use maintenance_jobs::*;

/// The state replication of a table reports for each of its destinations:
/// which configuration replicates the table, and what the clock and the
/// destination make of it.
mod replication_status;
pub use replication_status::*;

/// Whether the IAM role a replication configuration names can replicate at all:
/// whether the replication principal can assume it and whether it is allowed
/// the reads and writes replication performs. Named rather than glob-imported,
/// for the same reason as [`authorization`]: a read of the replication status
/// reads `crate::state::replication_role::engine_refusals(..)` and the question
/// is visible at the call site.
pub mod replication_role;

/// Whether the policies in force — the calling principal's own identity-based
/// IAM documents and the resource-based policy on what the request addresses —
/// allow the action a request asks for. Named rather than glob-imported, so the
/// gate that asks it reads `crate::state::authorization::refusal(..)` and is
/// visible at the call site.
pub mod authorization;

/// Whether KMS resolves the customer managed key an encryption configuration
/// names for the caller. Named rather than glob-imported, for the same reason
/// as [`authorization`]: a handler reads
/// `crate::state::kms_key_access::refusal(..)` and the gate is visible at the
/// call site.
pub mod kms_key_access;

/// The tagging rules: which resource a `resourceArn` addresses, and which tag
/// keys this service accepts.
mod tagging;
pub use tagging::*;

/// The migrations of a source table into a table of this store: the record each
/// one leaves behind, the destination table it creates and the states the clock
/// moves it through.
mod migrations;
pub use migrations::*;

/// The region a table bucket's ARN carries when the request carries no
/// parseable SigV4 credential scope to name one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most table buckets this emulator keeps resident for one account.
///
/// CHOSEN, not an AWS-published figure: the S3 Tables table-bucket limit is an
/// adjustable account quota, so the emulator does not claim a number AWS
/// guarantees. What matters is that the store is BOUNDED — a caller cannot grow
/// the process's memory without limit by creating table buckets it never
/// deletes — and that the refusal is the client-input error this operation
/// models, `BadRequestException`.
pub const TABLE_BUCKETS_PER_ACCOUNT: usize = 1_000;

/// The most namespaces this emulator keeps resident in one table bucket.
///
/// CHOSEN, for the same reason as [`TABLE_BUCKETS_PER_ACCOUNT`]: the store is
/// BOUNDED, so a caller cannot grow the process's memory without limit, and the
/// refusal is the client-input error the create models.
pub const NAMESPACES_PER_TABLE_BUCKET: usize = 1_000;

/// The most tables this emulator keeps resident in one namespace. CHOSEN, for
/// the same reason as [`TABLE_BUCKETS_PER_ACCOUNT`].
pub const TABLES_PER_NAMESPACE: usize = 1_000;

/// The most items any one page of a list operation carries.
///
/// Both paginated list operations model their page size (`maxBuckets`,
/// `maxTables`) with `@range(min: 1, max: 1000)`, so 1000 is the largest page a
/// caller can ask for and therefore the cap for a caller that asks for none.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// `TableBucketType` — the `type` member of a table bucket. Values from
/// `com.amazonaws.s3tables#TableBucketType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableBucketType {
    /// A bucket the caller created.
    Customer,
    /// A bucket Amazon Web Services created on the account's behalf.
    Aws,
}

impl TableBucketType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Customer, Self::Aws];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Customer => "customer",
            Self::Aws => "aws",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// `TableType` — the `type` member of a table. Values from
/// `com.amazonaws.s3tables#TableType`, a shape of its own even though its two
/// values read like `TableBucketType`'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableType {
    /// A table the caller created.
    Customer,
    /// A table Amazon Web Services created on the account's behalf.
    Aws,
}

impl TableType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Customer, Self::Aws];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Customer => "customer",
            Self::Aws => "aws",
        }
    }
}

/// `OpenTableFormat` — the open table format a table is stored in. Values from
/// `com.amazonaws.s3tables#OpenTableFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTableFormat {
    /// Apache Iceberg, the one format the model declares.
    Iceberg,
}

impl OpenTableFormat {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Iceberg];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Iceberg => "ICEBERG",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// `SSEAlgorithm` — the server-side encryption algorithm an encryption
/// configuration names. Values from `com.amazonaws.s3tables#SSEAlgorithm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseAlgorithm {
    /// S3-managed encryption keys.
    Aes256,
    /// Amazon Web Services KMS-managed encryption keys.
    AwsKms,
}

impl SseAlgorithm {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Aes256, Self::AwsKms];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Aes256 => "AES256",
            Self::AwsKms => "aws:kms",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }

    /// Whether this algorithm takes a KMS key: the model documents `kmsKeyArn`
    /// as "required only when `sseAlgorithm` is set to `aws:kms`".
    pub fn takes_kms_key(self) -> bool {
        matches!(self, Self::AwsKms)
    }
}

/// `StorageClass` — the storage class a storage-class configuration names.
/// Values from `com.amazonaws.s3tables#StorageClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageClass {
    Standard,
    IntelligentTiering,
}

impl StorageClass {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Standard, Self::IntelligentTiering];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Standard => "STANDARD",
            Self::IntelligentTiering => "INTELLIGENT_TIERING",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// The storage class a table or table bucket is stored in when nothing has
/// named one: the general-purpose class, which is what the model means by "the
/// service default storage class will be used"
/// (`smithy_query(query="doc", shape="CreateTableBucketRequest",
/// member="storageClassConfiguration")`). The other modeled class optimises an
/// access pattern a caller has to ask for, so it is never what a caller that
/// asked for nothing gets.
pub const DEFAULT_STORAGE_CLASS: StorageClass = StorageClass::Standard;

/// The storage class a table bucket's new tables are created in: the
/// configuration a create or a put persisted for the bucket, else
/// [`DEFAULT_STORAGE_CLASS`].
///
/// A table bucket ALWAYS has a default storage class, so a read reports one
/// whether or not a configuration was ever set on it — the stored `None` means
/// "no caller has named one", not "this bucket has no default".
pub fn effective_bucket_storage_class(stored: Option<StorageClass>) -> StorageClass {
    stored.unwrap_or(DEFAULT_STORAGE_CLASS)
}

/// The storage class a table is stored in: the one resolved for the table when
/// it was created, else — for a record that carries none — the default of the
/// bucket that holds it.
///
/// A table's class is settled at creation (the create's own
/// `storageClassConfiguration`, else the bucket's default at that moment), so a
/// later change to the bucket's default is the default for the tables created
/// AFTER it and moves no table that already exists.
pub fn effective_table_storage_class(
    table: Option<StorageClass>,
    table_bucket: Option<StorageClass>,
) -> StorageClass {
    effective_bucket_storage_class(table.or(table_bucket))
}

/// The stored form of the `EncryptionConfiguration` structure: the algorithm a
/// create named, and the KMS key when it named one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketEncryption {
    pub sse_algorithm: SseAlgorithm,
    pub kms_key_arn: Option<String>,
}

/// The encryption every table bucket carries when no configuration of its own
/// was ever set on it: server-side encryption with S3-managed keys, which is
/// the service default and takes no KMS key.
pub const DEFAULT_BUCKET_ENCRYPTION: BucketEncryption = BucketEncryption {
    sse_algorithm: SseAlgorithm::Aes256,
    kms_key_arn: None,
};

/// The encryption a table bucket is actually protected by: the configuration
/// the store holds for it, else [`DEFAULT_BUCKET_ENCRYPTION`].
///
/// A table bucket is ALWAYS encrypted, so a read reports an algorithm whether
/// or not a configuration was ever set — the stored `None` means "the caller
/// asked for nothing", not "nothing protects this bucket".
pub fn effective_bucket_encryption(stored: Option<&BucketEncryption>) -> BucketEncryption {
    stored.cloned().unwrap_or(DEFAULT_BUCKET_ENCRYPTION)
}

/// The encryption a table is actually protected by: the configuration stored
/// for the table itself, else the default encryption of the table bucket that
/// holds it ([`effective_bucket_encryption`]).
///
/// A table is ALWAYS encrypted, so a read reports an algorithm whether or not a
/// configuration of its own was ever set on it: a table whose record carries
/// `None` named no encryption, which is what the bucket's default encryption
/// configuration is for, and a bucket that carries none of its own is protected
/// by [`DEFAULT_BUCKET_ENCRYPTION`].
pub fn effective_table_encryption(
    table: Option<&BucketEncryption>,
    table_bucket: Option<&BucketEncryption>,
) -> BucketEncryption {
    effective_bucket_encryption(table.or(table_bucket))
}

/// The identifier of the single metrics configuration a table bucket can have.
///
/// A table bucket has exactly ONE metrics configuration and it covers the whole
/// bucket, so the service names it with this FIXED identifier rather than with
/// a per-bucket or per-call value: it carries nothing from the request and
/// nothing from state, and it is therefore the same text for every bucket, on
/// every read, and across any number of put/delete cycles.
pub const WHOLE_BUCKET_METRICS_CONFIGURATION_ID: &str = "EntireBucket";

/// The stored form of a table bucket's metrics configuration.
///
/// `PutTableBucketMetricsConfiguration` carries no settings — its whole input is
/// the table bucket's ARN — so what a put persists is only that the bucket HAS
/// the whole-bucket metrics configuration. There is nothing per-call to hold:
/// the id a read reports is a property of that one configuration
/// ([`WHOLE_BUCKET_METRICS_CONFIGURATION_ID`]), not of the call that set it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricsConfiguration;

impl MetricsConfiguration {
    /// The identifier a read reports for this configuration.
    pub fn id(&self) -> &'static str {
        WHOLE_BUCKET_METRICS_CONFIGURATION_ID
    }
}

/// `PartitionDateSource` — which date the keys of a table bucket's log objects
/// are partitioned by. Values from `com.amazonaws.s3tables#PartitionDateSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionDateSource {
    /// "The time that the requests in the log were made."
    EventTime,
    /// "The time that the log was delivered."
    DeliveryTime,
}

impl PartitionDateSource {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::EventTime, Self::DeliveryTime];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EventTime => "EventTime",
            Self::DeliveryTime => "DeliveryTime",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// The stored form of a table bucket's server access logging configuration: the
/// general purpose bucket its logs are delivered to, the prefix the log object
/// keys carry and the date those keys are partitioned by.
///
/// A bucket holds one of these only while logging is ON: "To disable logging,
/// specify an empty `TableBucketLoggingStatus`"
/// (`PutTableBucketLogging$tableBucketLoggingStatus`), and a read of a bucket
/// that holds none reports a status with no `loggingEnabled` member — which is
/// how a client observes that logging is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketLogging {
    pub target_bucket: String,
    pub target_prefix: String,
    pub partition_date_source: PartitionDateSource,
}

/// `TableRecordExpirationStatus` — whether the records of a table expire.
/// Values from `com.amazonaws.s3tables#TableRecordExpirationStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordExpirationStatus {
    /// Records expire and are removed after the configured number of days.
    Enabled,
    /// Records never expire.
    Disabled,
}

impl RecordExpirationStatus {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Enabled, Self::Disabled];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// `TableRecordExpirationJobStatus` — the state of the record expiration job of
/// a table. Values from `com.amazonaws.s3tables#TableRecordExpirationJobStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordExpirationJobStatus {
    /// The job is configured but has not run yet.
    NotYetRun,
    /// The job's last run completed.
    Successful,
    /// The job's last run failed.
    Failed,
    /// Record expiration is not enabled for the table, so no job runs for it.
    Disabled,
}

impl RecordExpirationJobStatus {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[
        Self::NotYetRun,
        Self::Successful,
        Self::Failed,
        Self::Disabled,
    ];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::NotYetRun => "NotYetRun",
            Self::Successful => "Successful",
            Self::Failed => "Failed",
            Self::Disabled => "Disabled",
        }
    }
}

/// The stored form of the `TableRecordExpirationConfigurationValue` structure:
/// whether a table's records expire, and after how many days.
///
/// Both members are optional on the wire, so both are optional here: what a put
/// persists is what the caller asked for, and the operations that report it are
/// where an absent member's meaning is decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordExpiration {
    pub status: Option<RecordExpirationStatus>,
    /// How many days a record is retained, the `settings.days` member — a
    /// `PositiveInteger` the model bounds at `@range(1..2147483647)`, so the
    /// boundary has already refused a value outside it by the time one is
    /// stored.
    pub days: Option<i32>,
}

/// The record expiration a table has when no configuration was ever put on it:
/// records do not expire.
///
/// A table ALWAYS has an expiration state — the records of one nothing was
/// configured on are simply kept — so a read reports a status whether or not a
/// configuration was ever written, as [`DEFAULT_BUCKET_ENCRYPTION`] does for a
/// bucket's encryption.
pub const DEFAULT_RECORD_EXPIRATION: RecordExpiration = RecordExpiration {
    status: Some(RecordExpirationStatus::Disabled),
    days: None,
};

/// The record expiration in force for a table: the configuration the store
/// holds for it, else [`DEFAULT_RECORD_EXPIRATION`].
pub fn effective_record_expiration(stored: Option<&RecordExpiration>) -> RecordExpiration {
    stored.cloned().unwrap_or(DEFAULT_RECORD_EXPIRATION)
}

/// The status of the record expiration job of a table whose stored expiration
/// configuration is `stored`.
///
/// A table whose records do not expire runs no expiration job, which is the
/// modeled disabled status. Nothing in this emulator runs such a job, so a
/// table whose records DO expire reports the job as not yet run: a completed or
/// failed run would be a claim about work nothing performed.
pub fn record_expiration_job_status(stored: Option<&RecordExpiration>) -> RecordExpirationJobStatus {
    match effective_record_expiration(stored).status {
        Some(RecordExpirationStatus::Enabled) => RecordExpirationJobStatus::NotYetRun,
        _ => RecordExpirationJobStatus::Disabled,
    }
}

/// Whether the record expiration operations may be used on `table`.
///
/// Record expiration is a feature of the tables Amazon Web Services manages on
/// an account's behalf; on a table of any other type every one of those calls
/// is refused with [`RECORD_EXPIRATION_NOT_ALLOWED_MESSAGE`], whatever the
/// request asks for.
pub fn allows_record_expiration(table: &Table) -> bool {
    matches!(table.table_type, TableType::Aws)
}

/// `TableMaintenanceType` — the maintenance tasks S3 Tables runs on a table.
/// Values from `com.amazonaws.s3tables#TableMaintenanceType`.
///
/// Ordered, because a table's configurations are stored in a map keyed by this
/// type: a read then reports them in one stable order whatever order they were
/// written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TableMaintenanceType {
    /// Rewriting a table's small data files into fewer, larger ones.
    IcebergCompaction,
    /// Expiring a table's older snapshots.
    IcebergSnapshotManagement,
}

impl TableMaintenanceType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::IcebergCompaction, Self::IcebergSnapshotManagement];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::IcebergCompaction => "icebergCompaction",
            Self::IcebergSnapshotManagement => "icebergSnapshotManagement",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// `MaintenanceStatus` — whether a maintenance task runs on a table. Values
/// from `com.amazonaws.s3tables#MaintenanceStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceStatus {
    /// The task runs.
    Enabled,
    /// The task does not run.
    Disabled,
}

impl MaintenanceStatus {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Enabled, Self::Disabled];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// `IcebergCompactionStrategy` — how compaction picks the files it rewrites.
/// Values from `com.amazonaws.s3tables#IcebergCompactionStrategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcebergCompactionStrategy {
    /// The strategy is chosen from the table's own sort order.
    Auto,
    /// Files are packed together without reordering records.
    Binpack,
    /// Records are rewritten in the table's sort order.
    Sort,
    /// Records are rewritten in z-order over the table's sort columns.
    ZOrder,
}

impl IcebergCompactionStrategy {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Auto, Self::Binpack, Self::Sort, Self::ZOrder];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Binpack => "binpack",
            Self::Sort => "sort",
            Self::ZOrder => "z-order",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }

    /// Whether compaction under this strategy orders a table's records by the
    /// table's sort order, which it can only do for a table whose metadata
    /// carries one.
    ///
    /// The two that do are the ones that REWRITE records in an order: sorting
    /// them by the table's sort order, and interleaving them in z-order over
    /// the columns that order names. Packing files together reorders nothing,
    /// and the automatic strategy is the service choosing for itself what the
    /// table supports.
    pub fn requires_sort_order(self) -> bool {
        match self {
            Self::Auto | Self::Binpack => false,
            Self::Sort | Self::ZOrder => true,
        }
    }
}

/// The stored form of the `IcebergCompactionSettings` structure.
///
/// Both members are optional on the wire, so both are optional here: what a put
/// persists is what the caller asked for, and
/// [`effective_table_maintenance`] is where an absent member's value is
/// decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionSettings {
    /// The file size compaction rewrites towards, in MB — a `PositiveInteger`
    /// the model bounds at `@range(1..2147483647)`, so the boundary has already
    /// refused a value outside it by the time one is stored.
    pub target_file_size_mb: Option<i32>,
    pub strategy: Option<IcebergCompactionStrategy>,
}

/// The stored form of the `IcebergSnapshotManagementSettings` structure. Both
/// members are optional for the same reason as on [`CompactionSettings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotManagementSettings {
    pub min_snapshots_to_keep: Option<i32>,
    pub max_snapshot_age_hours: Option<i32>,
}

/// The stored form of the `TableMaintenanceSettings` union: the settings of ONE
/// maintenance task, so which task a stored configuration settles is carried by
/// the value itself and not only by the key it is filed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceSettings {
    IcebergCompaction(CompactionSettings),
    IcebergSnapshotManagement(SnapshotManagementSettings),
}

impl MaintenanceSettings {
    /// The maintenance task these settings configure — the union member's own
    /// name, which is what a put checks against the type it was addressed with.
    pub fn maintenance_type(&self) -> TableMaintenanceType {
        match self {
            Self::IcebergCompaction(..) => TableMaintenanceType::IcebergCompaction,
            Self::IcebergSnapshotManagement(..) => TableMaintenanceType::IcebergSnapshotManagement,
        }
    }
}

/// The stored form of the `TableMaintenanceConfigurationValue` structure: one
/// maintenance task's status and settings for one table.
///
/// Both members are optional on the wire, so both are optional here, as on
/// [`RecordExpiration`]: a put persists what the caller asked for and
/// [`effective_table_maintenance`] is where an absent member's meaning is
/// decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableMaintenance {
    pub status: Option<MaintenanceStatus>,
    pub settings: Option<MaintenanceSettings>,
}

/// The file size S3 Tables compacts a table towards when nothing asked for
/// one. The S3 Tables maintenance documentation states the default target file
/// size for compaction as 512 MB.
pub const DEFAULT_COMPACTION_TARGET_FILE_SIZE_MB: i32 = 512;

/// The compaction strategy in force when nothing asked for one: the strategy is
/// derived from the table's own sort order, which the S3 Tables maintenance
/// documentation states is what compaction does for a table whose strategy was
/// never set.
pub const DEFAULT_COMPACTION_STRATEGY: IcebergCompactionStrategy = IcebergCompactionStrategy::Auto;

/// The fewest snapshots snapshot management keeps when nothing asked for a
/// number: the S3 Tables maintenance documentation states the default minimum
/// as 1 snapshot.
pub const DEFAULT_MIN_SNAPSHOTS_TO_KEEP: i32 = 1;

/// The age at which snapshot management expires a snapshot when nothing asked
/// for an age: the S3 Tables maintenance documentation states the default
/// maximum snapshot age as 120 hours.
pub const DEFAULT_MAX_SNAPSHOT_AGE_HOURS: i32 = 120;

/// The maintenance configuration a table runs `kind` under when no put ever set
/// one on it.
///
/// Every S3 Tables table is maintained from the moment it is created — both
/// tasks are on, under the service's own settings — so a read reports a
/// configuration for each modeled task whether or not one was ever written, as
/// [`DEFAULT_RECORD_EXPIRATION`] does for a table's record expiration.
pub fn default_table_maintenance(kind: TableMaintenanceType) -> TableMaintenance {
    TableMaintenance {
        status: Some(MaintenanceStatus::Enabled),
        settings: Some(match kind {
            TableMaintenanceType::IcebergCompaction => {
                MaintenanceSettings::IcebergCompaction(CompactionSettings {
                    target_file_size_mb: Some(DEFAULT_COMPACTION_TARGET_FILE_SIZE_MB),
                    strategy: Some(DEFAULT_COMPACTION_STRATEGY),
                })
            }
            TableMaintenanceType::IcebergSnapshotManagement => {
                MaintenanceSettings::IcebergSnapshotManagement(SnapshotManagementSettings {
                    min_snapshots_to_keep: Some(DEFAULT_MIN_SNAPSHOTS_TO_KEEP),
                    max_snapshot_age_hours: Some(DEFAULT_MAX_SNAPSHOT_AGE_HOURS),
                })
            }
        }),
    }
}

/// `preferred`'s members where it names them and `fallback`'s where it does
/// not: the member-by-member overlay of one maintenance configuration on
/// another.
///
/// Every member of a `TableMaintenanceConfigurationValue` is optional in the
/// model — the status, the settings, and each member inside them — so reading
/// one configuration against another is this one walk, whether the configuration
/// underneath is the task's service default ([`effective_task_maintenance`]) or
/// the one a previous put left in force ([`applied_task_maintenance`]).
///
/// Settings of one task cannot be overlaid on settings of the other: such a pair
/// configures nothing of either task, so what lies underneath stands. The put is
/// what keeps that pair out of the store, by filing only settings of the task it
/// was addressed with, so the arm is reached for a configuration that carries no
/// settings at all.
fn overlay(preferred: &TableMaintenance, fallback: &TableMaintenance) -> TableMaintenance {
    let settings = match (preferred.settings, fallback.settings) {
        (
            Some(MaintenanceSettings::IcebergCompaction(preferred)),
            Some(MaintenanceSettings::IcebergCompaction(fallback)),
        ) => Some(MaintenanceSettings::IcebergCompaction(CompactionSettings {
            target_file_size_mb: preferred
                .target_file_size_mb
                .or(fallback.target_file_size_mb),
            strategy: preferred.strategy.or(fallback.strategy),
        })),
        (
            Some(MaintenanceSettings::IcebergSnapshotManagement(preferred)),
            Some(MaintenanceSettings::IcebergSnapshotManagement(fallback)),
        ) => Some(MaintenanceSettings::IcebergSnapshotManagement(
            SnapshotManagementSettings {
                min_snapshots_to_keep: preferred
                    .min_snapshots_to_keep
                    .or(fallback.min_snapshots_to_keep),
                max_snapshot_age_hours: preferred
                    .max_snapshot_age_hours
                    .or(fallback.max_snapshot_age_hours),
            },
        )),
        _ => fallback.settings,
    };
    TableMaintenance {
        status: preferred.status.or(fallback.status),
        settings,
    }
}

/// The maintenance configuration in force for one task of a table: the
/// configuration the store holds for it with every member a put left out filled
/// in from [`default_table_maintenance`], so a caller that set only a status
/// still reads the settings the task runs under.
pub fn effective_task_maintenance(
    kind: TableMaintenanceType,
    stored: Option<&TableMaintenance>,
) -> TableMaintenance {
    let default = default_table_maintenance(kind);
    match stored {
        None => default,
        Some(stored) => overlay(stored, &default),
    }
}

/// The whole maintenance configuration in force for a table: one entry per
/// modeled maintenance task, each resolved by [`effective_task_maintenance`].
pub fn effective_table_maintenance(
    stored: &BTreeMap<TableMaintenanceType, TableMaintenance>,
) -> BTreeMap<TableMaintenanceType, TableMaintenance> {
    TableMaintenanceType::ALL
        .iter()
        .copied()
        .map(|kind| (kind, effective_task_maintenance(kind, stored.get(&kind))))
        .collect()
}

/// The configuration in force for `kind` once the value a put carries has been
/// applied to it.
///
/// A member the value leaves out is not a member set to nothing: it is one the
/// request says nothing about, so the value in force for it STAYS — the
/// configuration the last put that named that member left, else the task's own
/// service default ([`effective_task_maintenance`]). So a put carrying only a
/// status settles the status and leaves the settings the task runs under alone,
/// and one carrying only a target file size leaves the strategy alone.
///
/// `in_force` is the entry the store holds for `kind`, and `requested` the value
/// this put carries, already read into its stored form.
pub fn applied_task_maintenance(
    kind: TableMaintenanceType,
    in_force: Option<&TableMaintenance>,
    requested: &TableMaintenance,
) -> TableMaintenance {
    overlay(requested, &effective_task_maintenance(kind, in_force))
}

/// `TableBucketMaintenanceType` — the maintenance tasks S3 Tables runs on a
/// table bucket, as opposed to the per-table tasks
/// [`TableMaintenanceType`] names. Values from
/// `com.amazonaws.s3tables#TableBucketMaintenanceType`.
///
/// Ordered, for the same reason as [`TableMaintenanceType`]: a bucket's
/// configurations are stored in a map keyed by this type, so a read reports them
/// in one stable order whatever order they were written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TableBucketMaintenanceType {
    /// Deleting the files a table bucket's tables no longer reference.
    IcebergUnreferencedFileRemoval,
}

impl TableBucketMaintenanceType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::IcebergUnreferencedFileRemoval];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::IcebergUnreferencedFileRemoval => "icebergUnreferencedFileRemoval",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// The stored form of the `IcebergUnreferencedFileRemovalSettings` structure.
///
/// Both members are optional on the wire, so both are optional here, as on
/// [`CompactionSettings`]: a put persists what the caller asked for and
/// [`effective_bucket_maintenance`] is where an absent member's value is
/// decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreferencedFileRemovalSettings {
    /// How many days a file has to have been unreferenced before it is marked
    /// noncurrent — a `PositiveInteger` the model bounds at
    /// `@range(1..2147483647)`, so the boundary has already refused a value
    /// outside it by the time one is stored.
    pub unreferenced_days: Option<i32>,
    /// How many days a noncurrent file is kept before it is deleted, bounded
    /// the same way.
    pub non_current_days: Option<i32>,
}

/// The stored form of the `TableBucketMaintenanceSettings` union: the settings
/// of ONE table bucket maintenance task, so which task a stored configuration
/// settles is carried by the value itself and not only by the key it is filed
/// under — the rule [`MaintenanceSettings`] follows for a table's tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketMaintenanceSettings {
    IcebergUnreferencedFileRemoval(UnreferencedFileRemovalSettings),
}

impl BucketMaintenanceSettings {
    /// The maintenance task these settings configure — the union member's own
    /// name, which is what a put checks against the type it was addressed with.
    pub fn maintenance_type(&self) -> TableBucketMaintenanceType {
        match self {
            Self::IcebergUnreferencedFileRemoval(..) => {
                TableBucketMaintenanceType::IcebergUnreferencedFileRemoval
            }
        }
    }
}

/// The stored form of the `TableBucketMaintenanceConfigurationValue` structure:
/// one table bucket maintenance task's status and settings for one bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableBucketMaintenance {
    pub status: Option<MaintenanceStatus>,
    pub settings: Option<BucketMaintenanceSettings>,
}

/// How long a file has to have been unreferenced before unreferenced file
/// removal marks it noncurrent when nothing asked for a number: the S3 Tables
/// maintenance documentation states the default as 3 days.
pub const DEFAULT_UNREFERENCED_DAYS: i32 = 3;

/// How long a noncurrent file is kept before unreferenced file removal deletes
/// it when nothing asked for a number: the same documentation states the
/// default as 10 days.
pub const DEFAULT_NON_CURRENT_DAYS: i32 = 10;

/// The maintenance configuration a table bucket runs `kind` under when no put
/// ever set one on it.
///
/// Every S3 Tables table bucket is maintained from the moment it is created —
/// the task is on, under the service's own settings — so a read reports a
/// configuration for each modeled task whether or not one was ever written, as
/// [`default_table_maintenance`] does for a table's tasks.
pub fn default_bucket_maintenance(kind: TableBucketMaintenanceType) -> TableBucketMaintenance {
    TableBucketMaintenance {
        status: Some(MaintenanceStatus::Enabled),
        settings: Some(match kind {
            TableBucketMaintenanceType::IcebergUnreferencedFileRemoval => {
                BucketMaintenanceSettings::IcebergUnreferencedFileRemoval(
                    UnreferencedFileRemovalSettings {
                        unreferenced_days: Some(DEFAULT_UNREFERENCED_DAYS),
                        non_current_days: Some(DEFAULT_NON_CURRENT_DAYS),
                    },
                )
            }
        }),
    }
}

/// The maintenance configuration in force for one task of a table bucket: the
/// configuration the store holds for it with every member a put left out filled
/// in from [`default_bucket_maintenance`], so a caller that set only a status
/// still reads the settings the task runs under — the rule
/// [`effective_task_maintenance`] follows for a table's tasks.
pub fn effective_bucket_task_maintenance(
    kind: TableBucketMaintenanceType,
    stored: Option<&TableBucketMaintenance>,
) -> TableBucketMaintenance {
    let default = default_bucket_maintenance(kind);
    let Some(stored) = stored else {
        return default;
    };
    let settings = match (stored.settings, default.settings) {
        // A stored setting for the task, with each member the put left out
        // taken from the task's default.
        (
            Some(BucketMaintenanceSettings::IcebergUnreferencedFileRemoval(stored)),
            Some(BucketMaintenanceSettings::IcebergUnreferencedFileRemoval(default)),
        ) => Some(BucketMaintenanceSettings::IcebergUnreferencedFileRemoval(
            UnreferencedFileRemovalSettings {
                unreferenced_days: stored.unreferenced_days.or(default.unreferenced_days),
                non_current_days: stored.non_current_days.or(default.non_current_days),
            },
        )),
        // The put carried no settings for the task, so it runs under the task's
        // own defaults.
        _ => default.settings,
    };
    TableBucketMaintenance {
        status: stored.status.or(default.status),
        settings,
    }
}

/// The whole maintenance configuration in force for a table bucket: one entry
/// per modeled table bucket maintenance task, each resolved by
/// [`effective_bucket_task_maintenance`].
pub fn effective_bucket_maintenance(
    stored: &BTreeMap<TableBucketMaintenanceType, TableBucketMaintenance>,
) -> BTreeMap<TableBucketMaintenanceType, TableBucketMaintenance> {
    TableBucketMaintenanceType::ALL
        .iter()
        .copied()
        .map(|kind| (kind, effective_bucket_task_maintenance(kind, stored.get(&kind))))
        .collect()
}

/// The stored form of a replication configuration — the one record behind both
/// `TableBucketReplicationConfiguration` and `TableReplicationConfiguration`,
/// which the model declares with the same two members (a role and rules naming
/// destination table buckets), one scoped to a whole table bucket and one to a
/// single table: the role S3 Tables replicates with, the rules that say where
/// to, and the token that names this state of the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicationConfiguration {
    /// The ARN of the IAM role S3 Tables assumes to replicate tables.
    pub role: String,
    /// The configuration's rules. The model bounds the list at exactly one
    /// (`@length(min: 1, max: 1)`), so the bound is enforced at the boundary
    /// and what is stored is the list the caller sent.
    pub rules: Vec<ReplicationRule>,
    /// Regenerated on every write of the configuration, so a caller holding an
    /// older token can tell its view is stale — the same rule a table's
    /// `versionToken` follows.
    pub version_token: String,
    /// When the write that stored this configuration was served. Replication
    /// of a table to a destination cannot have completed before the
    /// configuration that replicates it was set, which is why the instant is
    /// persisted here and the status a read reports is derived from it
    /// ([`ComAmazonawsS3tablesS3tablebucketsState::table_replication_status`]).
    pub configured_at: SystemTime,
}

/// One rule of a replication configuration: the destination table buckets the
/// tables it covers are replicated to, each by its ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicationRule {
    pub destinations: Vec<String>,
}

/// A table bucket, as this emulator stores it.
///
/// `encryption` and `storage_class` are `None` when the create named neither:
/// the model documents a service default for each, and which default that is
/// belongs to the operations that report them (`GetTableBucketEncryption`,
/// `GetTableBucketStorageClass`), so what is persisted here is what the caller
/// asked for and nothing invented on its behalf.
#[derive(Debug, Clone)]
pub struct TableBucket {
    pub name: String,
    pub arn: String,
    /// The region the bucket lives in — the one its ARN carries. Stored because
    /// `ListTableBuckets` takes no ARN, so the region it answers for can only
    /// come from the request's own endpoint.
    pub region: String,
    pub owner_account_id: String,
    pub bucket_id: String,
    pub bucket_type: TableBucketType,
    pub created_at: SystemTime,
    pub encryption: Option<BucketEncryption>,
    pub storage_class: Option<StorageClass>,
    /// The bucket's metrics configuration, `None` until a put turns metrics on:
    /// a table bucket collects no metrics until one is set on it, which is why
    /// a read of a bucket that has none is refused rather than answered with a
    /// default.
    pub metrics: Option<MetricsConfiguration>,
    /// The bucket's replication configuration, `None` until a put sets one on
    /// it: a table bucket replicates nothing until one is set, which is why a
    /// read or a delete of a bucket that has none is refused rather than
    /// answered with a default.
    pub replication: Option<ReplicationConfiguration>,
    /// The bucket's resource-based policy document, `None` until a put sets one
    /// on it: a table bucket carries no policy until a caller attaches one, so
    /// a read of a bucket that has none is refused rather than answered with an
    /// empty document, and a delete of one that has none is the no-op its
    /// `@idempotent` trait describes. Held in the one layout an attached policy
    /// document is reported in (`service_common::iam_policy::canonical_document`,
    /// applied by the put), so every read answers the same text.
    pub policy: Option<String>,
    /// The maintenance configurations a put has set on this bucket, keyed by
    /// the task each settles. A task with no entry runs under the service's own
    /// defaults, which [`effective_bucket_maintenance`] is where it is reported
    /// from.
    pub maintenance: BTreeMap<TableBucketMaintenanceType, TableBucketMaintenance>,
    /// The schedule each of this bucket's maintenance tasks runs under, keyed
    /// by the task, written by the put that turns a task on or off. A task with
    /// no entry has run under its default status since the bucket was created,
    /// which [`bucket_task_schedule`] is where it is resolved from.
    pub maintenance_schedules: BTreeMap<TableBucketMaintenanceType, MaintenanceSchedule>,
    pub tags: BTreeMap<String, String>,
    /// The namespaces resident in this bucket, keyed by namespace name, so a
    /// list has one stable order to page over.
    pub namespaces: BTreeMap<String, Namespace>,
    /// Where this bucket's server access logs are delivered, `None` until a put
    /// turns logging on: a table bucket delivers no logs until one is set, and a
    /// read of a bucket that has none reports a logging status with no
    /// `loggingEnabled` member rather than being refused ([`BucketLogging`]).
    pub logging: Option<BucketLogging>,
}

/// A namespace, as this emulator stores it: a logical grouping of tables inside
/// one table bucket.
///
/// `namespace` is the modeled `NamespaceList` the create was given — one name,
/// as the model's `@length(min: 1, max: 1)` on the input requires — and its
/// single element is the key this namespace is stored under.
#[derive(Debug, Clone)]
pub struct Namespace {
    pub namespace: Vec<String>,
    pub namespace_id: String,
    pub created_at: SystemTime,
    pub created_by: String,
    pub owner_account_id: String,
    /// The tables resident in this namespace, keyed by table name.
    pub tables: BTreeMap<String, Table>,
}

/// A table, as this emulator stores it.
///
/// `encryption` and `storage_class` are settled by the CREATE and persisted: a
/// table is created encrypted and in a storage class, with the configuration the
/// request named, else the bucket's default at that moment, else the service
/// default — which is why a later change to the bucket's default is the default
/// for the tables created after it and touches no table that already exists.
/// Either is `None` only on a record that carries none of its own, the tables an
/// Amazon Web Services service creates on the account's behalf, which are
/// protected and stored the way their bucket is
/// ([`effective_table_encryption`], [`effective_table_storage_class`]).
/// `metadata_location` is `Some` only once a metadata
/// file exists for the
/// table — the create writes one when the request carries table metadata, and
/// `UpdateTableMetadataLocation` is what moves it afterwards.
#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub table_arn: String,
    pub table_type: TableType,
    /// The service that manages the table (`GetTable$managedByService`), set only on the
    /// tables an Amazon Web Services service created in the bucket on the account's behalf
    /// ([`ComAmazonawsS3tablesS3tablebucketsState::create_service_managed_table`]). A table
    /// a caller created through `CreateTable` is managed by no service, so it stays `None`
    /// for the life of the record.
    pub managed_by_service: Option<String>,
    pub namespace: Vec<String>,
    pub namespace_id: String,
    pub table_bucket_id: String,
    pub format: OpenTableFormat,
    /// Regenerated on every mutation of the table, so a caller holding an older
    /// token can tell its view is stale.
    pub version_token: String,
    pub metadata_location: Option<String>,
    /// The table's own Iceberg metadata, held alongside the location of the
    /// file it is written to, and `None` for exactly the tables that have no
    /// metadata file. It is the table's description — its schema, partition
    /// spec, sort order and properties — so every answer about those is read
    /// from here rather than recomputed from a request.
    pub iceberg: Option<IcebergTableMetadata>,
    pub warehouse_location: String,
    pub created_at: SystemTime,
    pub created_by: String,
    pub modified_at: SystemTime,
    /// The account that last MODIFIED the table, `None` on a table nothing has
    /// modified since it was created: a table's modification is recorded when
    /// one happens, so a reader can tell a table that has been changed from one
    /// that still stands as created. Every handler that changes a table writes
    /// it ([`Table::record_modification`]).
    pub modified_by: Option<String>,
    pub owner_account_id: String,
    pub encryption: Option<BucketEncryption>,
    pub storage_class: Option<StorageClass>,
    /// The table's record expiration configuration, `None` until a put sets one
    /// on it. Only the tables Amazon Web Services manages take one at all, so
    /// on every other table this stays `None` for the life of the record.
    pub record_expiration: Option<RecordExpiration>,
    /// The table's OWN replication configuration, `None` until a put sets one on
    /// it: a table is replicated by a configuration of its own only once one is
    /// set, which is why a read or a delete of a table that has none is refused
    /// rather than answered with a default. It is independent of the
    /// configuration the bucket holding the table carries
    /// ([`TableBucket::replication`]), so setting one leaves the other alone.
    pub replication: Option<ReplicationConfiguration>,
    /// The maintenance configurations a put has set on this table, keyed by the
    /// task each settles. A task with no entry runs under the service's own
    /// defaults, which [`effective_table_maintenance`] is where it is reported
    /// from — so what is persisted here is the configuration in force for the
    /// task once each put's own members have been applied to it
    /// ([`applied_task_maintenance`]).
    pub maintenance: BTreeMap<TableMaintenanceType, TableMaintenance>,
    /// The schedule each of this table's maintenance tasks runs under, keyed by
    /// the task, written by the put that turns a task on or off. A task with no
    /// entry has run under its default status since the table was created,
    /// which [`table_task_schedule`] is where it is resolved from.
    pub maintenance_schedules: BTreeMap<TableMaintenanceType, MaintenanceSchedule>,
    /// When the migration into this table was COMPLETED
    /// (`CompleteTableMigration`), and `None` on every table whose migration is
    /// not complete — a table no migration created included. "You can't undo
    /// this operation", so the instant is written once and never cleared, and it
    /// is what a second complete, and a sync after one, are refused against
    /// ([`migrations`]).
    pub migration_completed_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
}

impl Table {
    /// Record that `account` modified this table `at`: the one place a table's
    /// modification bookkeeping is written, so every handler that changes a
    /// table leaves the same two facts behind it.
    pub fn record_modification(&mut self, account: String, at: SystemTime) {
        self.modified_at = at;
        self.modified_by = Some(account);
    }
}

/// Where a table lives: the index entry a `tableARN` resolves to, so the
/// ARN-addressed operations reach a record without scanning every bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableLocation {
    pub table_bucket_arn: String,
    pub namespace: String,
    pub name: String,
}

/// How many bytes of entropy a table's `versionToken` carries: ten, written two
/// lowercase hexadecimal characters per byte, so every token is twenty
/// characters wide.
///
/// The model constrains a `VersionToken` only by length (`@length(1, 2048)`), so
/// the FORM is recorded behaviour: the token this service hands back is a single
/// unbroken run of lowercase hexadecimal characters of exactly that width —
/// neither an identifier's hyphenated-UUID spelling nor a decimal number, whose
/// spelling would be as wide as the value it happened to draw. Stated once here,
/// so every operation that mints a version token mints one class of value.
const TOKEN_STAMP_BYTES: usize = 10;

/// Per-service state for com-amazonaws-s3tables-s3tablebuckets. A handler reaches it through [`service_state`].
///
/// `table_buckets` is keyed by the bucket's ARN — which carries its region and
/// its name — so a name is unique per region, as the service's naming rules
/// require, and a list operation has one stable order to page over.
#[derive(Debug, Default)]
pub struct ComAmazonawsS3tablesS3tablebucketsState {
    /// Every live table bucket, keyed by its `tableBucketARN`.
    pub table_buckets: BTreeMap<String, TableBucket>,
    /// Every live table, keyed by its `tableARN`: the index the `tableArn`
    /// addressing mode resolves through.
    pub tables_by_arn: BTreeMap<String, TableLocation>,
    /// Every migration this account has started, keyed by its `migrationId`.
    /// A migration outlives the call that started it and is the resource
    /// `GetTableMigration` and `ListTableMigrations` report
    /// ([`migrations`]).
    pub migrations: BTreeMap<String, Migration>,
    /// Counts the migrations this store has recorded, so the listing can hand
    /// them back newest first whatever the clock did. Never exposed on the wire.
    pub migrations_recorded: u64,
    /// Counts the ids this store has minted, so two ids minted inside one clock
    /// tick still differ. Never exposed on the wire.
    pub minted_ids: u64,
}

impl ComAmazonawsS3tablesS3tablebucketsState {
    /// The 16 bytes of entropy behind ONE minted value, whichever form it is
    /// then laid out in ([`Self::mint_id`], [`Self::mint_opaque_token`]).
    ///
    /// They are a digest of the instant the value is minted, this store's mint
    /// counter and `domain` (which kind of value it is), so what is minted
    /// carries nothing from the request, two mints never collide (the counter
    /// separates them inside one clock tick), and two emulator runs do not repeat
    /// each other's values.
    ///
    /// The derivation is DELIBERATELY deterministic — a clock reading and a
    /// counter, not a random source — because an emulator's minted values are
    /// test material: none of them is a secret, nothing is authorized by holding
    /// one, and a value a test can reason about is worth more here than an
    /// unguessable one.
    fn mint_entropy(&mut self, domain: &str) -> [u8; 16] {
        use ::sha2::{Digest, Sha256};
        self.minted_ids = self.minted_ids.wrapping_add(1);
        let since_epoch = SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let digest = Sha256::digest(
            format!("s3tables-{domain}:{since_epoch}:{}", self.minted_ids).as_bytes(),
        );
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes
    }

    /// A fresh server-generated id, in the canonical hyphenated UUID form every
    /// identifier this service mints carries (`tableBucketId`, `namespaceId`, the
    /// id segment of a `tableARN`, a table's metadata file id).
    ///
    /// The bytes [`Self::mint_entropy`] hands over are given to the `uuid` crate,
    /// which owns both remaining rules — the version and variant nibbles a
    /// random-bytes UUID carries, and the canonical lowercase hyphenated
    /// rendering — so neither is restated here to drift from the library's.
    fn mint_id(&mut self, domain: &str) -> String {
        ::uuid::Builder::from_random_bytes(self.mint_entropy(domain))
            .into_uuid()
            .hyphenated()
            .to_string()
    }

    /// A fresh opaque TOKEN, laid out in the form this service's version tokens
    /// take: the first [`TOKEN_STAMP_BYTES`] bytes [`Self::mint_entropy`] hands
    /// over, written as lowercase hexadecimal characters, so the token carries no
    /// structure a CLIENT could read anything out of (it is derived the
    /// deterministic way [`Self::mint_entropy`] describes, so it is not an
    /// unguessable value and nothing is authorized by holding one).
    ///
    /// Every token is the SAME width, because the width is part of the form a
    /// client sees: the bytes are rendered two characters each, including a
    /// leading zero, rather than as a number whose spelling shrinks with its
    /// value.
    ///
    /// A token is NOT an identifier: nothing addresses a resource by one, a
    /// caller only hands back the value it was given, and the service mints it in
    /// a form of its own — which is why this is not [`Self::mint_id`].
    fn mint_opaque_token(&mut self, domain: &str) -> String {
        let entropy = self.mint_entropy(domain);
        ::hex::encode(&entropy[..TOKEN_STAMP_BYTES])
    }

    /// A fresh `tableBucketId`, retried against the store so an id is never
    /// reused.
    pub fn mint_bucket_id(&mut self) -> String {
        loop {
            let candidate = self.mint_id("table-bucket");
            if !self
                .table_buckets
                .values()
                .any(|bucket| bucket.bucket_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// A fresh `namespaceId`, retried against the store so an id is never
    /// reused.
    pub fn mint_namespace_id(&mut self) -> String {
        loop {
            let candidate = self.mint_id("namespace");
            if !self.namespaces().any(|namespace| namespace.namespace_id == candidate) {
                return candidate;
            }
        }
    }

    /// A fresh table id — the last segment of a `tableARN`, which is what
    /// identifies a table for the life of the record whatever it is renamed to.
    /// Retried against the ARN index so an id is never reused.
    pub fn mint_table_id(&mut self) -> String {
        loop {
            let candidate = self.mint_id("table");
            let suffix = format!("/table/{candidate}");
            if !self.tables_by_arn.keys().any(|arn| arn.ends_with(&suffix)) {
                return candidate;
            }
        }
    }

    /// A fresh `versionToken` for a table that is being moved to another
    /// metadata document: the table's create, a catalog commit of a new
    /// metadata file for it, and a write that points the table at a metadata
    /// file the caller wrote.
    ///
    /// A table's version token is the precondition handle `CreateTable` hands
    /// back and `GetTable` echoes, which `DeleteTable`, `RenameTable` and
    /// `UpdateTableMetadataLocation` compare a caller's view against — an opaque
    /// token, not an identifier of the table, so it is minted in this service's
    /// token form ([`Self::mint_opaque_token`]) rather than in the id form
    /// [`Self::mint_id`] lays out.
    ///
    /// It is NOT minted by a write that leaves the table at the metadata
    /// document it was already at — a rename, a maintenance configuration, a
    /// tag — so a caller's view of the table survives a write of the control
    /// plane around it.
    pub fn mint_version_token(&mut self) -> String {
        self.mint_opaque_token("table-version")
    }

    /// A fresh `versionToken` for a replication configuration — a table
    /// bucket's or a single table's — minted on every write of one.
    ///
    /// The same opaque token form a TABLE's own version token takes
    /// ([`Self::mint_version_token`]), which is the form the recorded responses of
    /// this service's version tokens carry, minted in its own domain so no two
    /// tokens of one store collide.
    pub fn mint_replication_version_token(&mut self) -> String {
        self.mint_opaque_token("table-bucket-replication-version")
    }

    /// A fresh id for a table's metadata file, which names the file under the
    /// table's warehouse location.
    pub fn mint_metadata_id(&mut self) -> String {
        self.mint_id("table-metadata")
    }

    /// Every resident namespace, in bucket then namespace order.
    pub fn namespaces(&self) -> impl Iterator<Item = &Namespace> {
        self.table_buckets
            .values()
            .flat_map(|bucket| bucket.namespaces.values())
    }

    /// The table `table_arn` names, resolved through the ARN index.
    pub fn table_by_arn(&self, table_arn: &str) -> Option<&Table> {
        let location = self.tables_by_arn.get(table_arn)?;
        self.table_buckets
            .get(&location.table_bucket_arn)?
            .namespaces
            .get(&location.namespace)?
            .tables
            .get(&location.name)
    }

    /// The table `table_arn` names, for a caller that is about to change it.
    pub fn table_by_arn_mut(&mut self, table_arn: &str) -> Option<&mut Table> {
        let location = self.tables_by_arn.get(table_arn)?.clone();
        self.table_buckets
            .get_mut(&location.table_bucket_arn)?
            .namespaces
            .get_mut(&location.namespace)?
            .tables
            .get_mut(&location.name)
    }

    /// Whether the store already holds [`TABLE_BUCKETS_PER_ACCOUNT`] table
    /// buckets, so a further create must be refused.
    pub fn is_at_bucket_quota(&self) -> bool {
        self.table_buckets.len() >= TABLE_BUCKETS_PER_ACCOUNT
    }
}

/// One page of a `@paginated` list operation.
#[derive(Debug)]
pub struct Page<T> {
    /// The page's items, in key order.
    pub items: Vec<T>,
    /// The opaque cursor the next page resumes from, set only when items the
    /// caller has not seen remain.
    pub continuation_token: Option<String>,
}

/// Read one page out of an ordered sequence of `(key, item)` pairs.
///
/// Both list operations are `@paginated` with
/// `inputToken: continuationToken, outputToken: continuationToken`, so: items
/// come back in key order, at most `max_items` of them, the page starts at the
/// key a caller's token decoded to, and a token comes back only when a later key
/// remains. `max_items` is modeled `1..1000` and refused outside that range
/// before a handler runs; an absent one falls back to [`MAX_PAGE_SIZE`], so no
/// response renders more than one page however large the store is.
pub fn page<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    resume_from: Option<&str>,
    max_items: Option<i32>,
) -> Page<T> {
    let cap = max_items
        .filter(|requested| *requested > 0)
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(MAX_PAGE_SIZE)
        .min(MAX_PAGE_SIZE);

    let mut items = Vec::new();
    let mut continuation_token = None;
    for (key, item) in ordered {
        if resume_from.is_some_and(|start| key.as_str() < start) {
            continue;
        }
        if items.len() >= cap {
            continuation_token = Some(encode_continuation_token(&key));
            break;
        }
        items.push(item);
    }

    Page {
        items,
        continuation_token,
    }
}

/// The `continuationToken` for a page that starts at `key`.
///
/// Hex of the key rather than the key itself, so a caller can read no structure
/// into the token and has nothing to build one from: the token is opaque and
/// only ever travels back unchanged.
pub fn encode_continuation_token(key: &str) -> String {
    ::hex::encode(key.as_bytes())
}

/// The inclusive start key a token minted by [`encode_continuation_token`]
/// carries, or `None` when the token was not minted by this service (which a
/// list operation refuses rather than silently treating as a start key).
pub fn decode_continuation_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// The stored form of an `encryptionConfiguration`, or the reason the request is
/// refused.
///
/// The algorithm must be one the model declares, and the model documents
/// `kmsKeyArn` as "required only when `sseAlgorithm` is set to `aws:kms`" — so
/// it is required with that algorithm and meaningless without it. A value
/// outside the enum is refused at the boundary, so reaching here with an unknown
/// one would mean the model changed under us; it is refused rather than silently
/// picking an algorithm.
///
/// Each refusal is stated with the one FIXED sentence this service words that
/// member's class with — [`INVALID_SSE_ALGORITHM_MESSAGE`] for the algorithm,
/// [`INVALID_KMS_KEY_ARN_MESSAGE`] for the key — the same sentences the
/// boundary's own constraint refusals carry ([`crate::service`]), so one member
/// is refused one way however the value failed: a `kmsKeyArn` the configuration
/// cannot encrypt under (absent where the algorithm takes one, present where it
/// takes none, or outside the modelled form) names no key ARN of the one form
/// this service takes, and the sentence that says so takes nothing from the
/// request.
pub fn stored_encryption(
    sse_algorithm: &str,
    kms_key_arn: Option<String>,
) -> Result<BucketEncryption, String> {
    let algorithm = SseAlgorithm::from_wire(sse_algorithm)
        .ok_or_else(|| INVALID_SSE_ALGORITHM_MESSAGE.to_string())?;
    match (algorithm.takes_kms_key(), &kms_key_arn) {
        (true, None) | (false, Some(_)) => Err(INVALID_KMS_KEY_ARN_MESSAGE.to_string()),
        _ => Ok(BucketEncryption {
            sse_algorithm: algorithm,
            kms_key_arn,
        }),
    }
}

/// The stored form of a `storageClassConfiguration`, or the reason the request is
/// refused: one of the modeled storage classes, or the request names a class
/// this service has no storage for.
pub fn stored_storage_class(storage_class: &str) -> Result<StorageClass, String> {
    StorageClass::from_wire(storage_class)
        .ok_or_else(|| format!("The storage class {storage_class} is not supported."))
}

/// The region this request is for: the one its SigV4 credential scope names,
/// else [`DEFAULT_REGION`] when the request carries no parseable scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `target` every audit record this service emits carries, so a consumer
/// can select the access trail without reading the rest of the emulator's
/// traces.
///
/// A state-changing operation emits ONE record, at `INFO`, naming the
/// operation, the caller's presented access key id, the resource it changed and
/// the outcome. Metadata only — never a request body, a tag value or a secret.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// The identity a request presents, for an audit record: the access key id in
/// its SigV4 credential scope. `emulator_server::extract_akid` answers
/// `"default"` when the request carries no scope. An access key id NAMES a
/// caller and is not a secret — the signature and the secret key are never
/// read here.
pub fn request_caller(parts: &::http::request::Parts) -> String {
    ::emulator_server::extract_akid(&parts.headers)
}

/// This service's ARN namespace, the value its model publishes as
/// `aws.api#service$arnNamespace` (`smithy_query(query="service_info")`): the
/// vendor field of every ARN this service mints and resolves.
pub const ARN_NAMESPACE: &str = "s3tables";

/// The ARN of a table bucket: `arn:aws:s3tables:<region>:<account>:bucket/<name>`.
pub fn table_bucket_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("bucket/{name}"))
}

/// The Region an ARN of this service's own namespace names, and `None` for a
/// value that names none — one that is no ARN, one of another vendor, and one
/// whose Region field is empty (a partition-global ARN) or outside the Region
/// grammar.
///
/// The parse is the workspace's one reader of a caller-supplied ARN
/// ([`::service_common::arn`]), so this service cannot disagree with another
/// about what an ARN is or about which text is a Region label.
pub fn arn_region(arn: &str) -> Option<&str> {
    ::service_common::arn::region_of(arn, ARN_NAMESPACE)
}

/// Whether `arn` names a resource of a Region other than `region`, the Region
/// the request was made against.
///
/// An ARN carries the Region its resource lives in, and this endpoint serves one
/// Region, so an ARN naming another one addresses no resource reachable here —
/// which is [`REGION_MISMATCH_MESSAGE`]. A value that names no Region of this
/// service at all is not a mismatch: there is nothing to compare, and what such
/// a value does name is for the operation that reads it to say.
pub fn arn_names_another_region(arn: &str, region: &str) -> bool {
    arn_region(arn).is_some_and(|named| named != region)
}

/// The ARN of a table: its bucket's ARN, then `/table/<table id>`.
///
/// Built from the bucket's own ARN rather than from the request, so the table's
/// ARN sits under the bucket's in the same region and account, and the last
/// segment is the server-minted table id rather than the name — the name can be
/// changed by `RenameTable`, the ARN cannot.
pub fn table_arn(table_bucket_arn: &str, table_id: &str) -> String {
    format!("{table_bucket_arn}/table/{table_id}")
}

/// The warehouse location of a table: the S3 location its data and metadata are
/// written under, named for the table's own id.
pub fn warehouse_location(table_id: &str) -> String {
    format!("s3://{table_id}--table-s3")
}

/// The location of a table's metadata file, under its warehouse location.
///
/// `metadata_id` is server-minted, and `version` — counted from zero for the
/// file the create writes, one higher for each metadata file a commit writes
/// after it — is the five-digit version prefix the Iceberg table specification
/// names metadata files with.
pub fn metadata_location(warehouse_location: &str, version: u32, metadata_id: &str) -> String {
    format!("{warehouse_location}/metadata/{version:05}-{metadata_id}.metadata.json")
}

/// The file names an Apache Iceberg table's metadata file may carry, as
/// `UpdateTableMetadataLocation` documents them: the metadata document itself,
/// or the same document Gzip-compressed.
pub const ICEBERG_METADATA_SUFFIXES: [&str; 2] = [".metadata.json", ".metadata.json.gz"];

/// Whether `location` can be where the metadata file of an Apache Iceberg table
/// whose warehouse is `warehouse_location` lives.
///
/// The rule `UpdateTableMetadataLocation` documents, restated structurally: a
/// metadata location is an S3 URI UNDER the table's warehouse location — the
/// table's own files live there and nowhere else — and, the table being an
/// Iceberg one, it names a metadata file ([`ICEBERG_METADATA_SUFFIXES`]).
pub fn is_iceberg_metadata_location(location: &str, warehouse_location: &str) -> bool {
    location
        .strip_prefix(warehouse_location)
        .and_then(|under| under.strip_prefix('/'))
        .is_some_and(|under| {
            ICEBERG_METADATA_SUFFIXES
                .iter()
                .any(|suffix| under.ends_with(suffix))
        })
}

/// How this service states a table bucket name it will not accept.
///
/// FIXED text: it names the CLASS of the failure and takes nothing from the
/// request, so the same sentence answers every unacceptable name — one outside
/// the modelled length or character set, and one that opens or closes with a
/// hyphen. The member it is stated for is the one the model constrains with
/// `TableBucketName` (`@length(3, 63)`, `@pattern(^[0-9a-z-]*$)`).
pub const INVALID_TABLE_BUCKET_NAME_MESSAGE: &str =
    "The specified bucket name isn't valid. Specify a different bucket name, and then try again.";

/// How this service states a table name it will not accept — the members the
/// model constrains with `TableName` (`@length(1, 255)`,
/// `@pattern(^[0-9a-z_]*$)`). FIXED text, as [`INVALID_TABLE_BUCKET_NAME_MESSAGE`].
pub const INVALID_TABLE_NAME_MESSAGE: &str = "The specified table name is not valid.";

/// How this service states a namespace name it will not accept — the members
/// the model constrains with `NamespaceName` (`@length(1, 255)`,
/// `@pattern(^[0-9a-z_]*$)`), which are the elements of a namespace and the
/// label the namespace and table operations address one by. FIXED text, as
/// [`INVALID_TABLE_BUCKET_NAME_MESSAGE`].
pub const INVALID_NAMESPACE_NAME_MESSAGE: &str =
    "The specified namespace name isn’t valid. Specify a different namespace name, and then try \
     again.";

/// How this service states a namespace naming more than one part: the model
/// bounds `CreateNamespaceRequest.namespace` (a `NamespaceList`) at
/// `@length(min: 1, max: 1)`, so a namespace is one name and nesting is not a
/// thing this service has.
pub const MULTIPART_NAMESPACE_MESSAGE: &str = "Multipart namespaces are not supported.";

/// How this service states a namespace name that is already taken in the table
/// bucket the create addresses: the conflict with a previous write the model's
/// `ConflictException` (HTTP 409) names. FIXED text — it states that the name is
/// taken, not which name was asked for.
pub const NAMESPACE_ALREADY_EXISTS_MESSAGE: &str =
    "A namespace with an identical name already exists in the bucket.";

/// How this service states a table bucket name that is already taken in the
/// account and Region the create addresses: the conflict with a previous write
/// the model's `ConflictException` (HTTP 409) names. FIXED text, as
/// [`NAMESPACE_ALREADY_EXISTS_MESSAGE`] — it states that the name is taken and
/// owned by this account, not which name was asked for, and it says "bucket"
/// rather than "table bucket".
pub const BUCKET_ALREADY_EXISTS_MESSAGE: &str =
    "The bucket that you tried to create already exists, and you own it.";

/// How this service states a table name that is already taken in the namespace
/// the create addresses: the conflict with a previous write the model's
/// `ConflictException` (HTTP 409) names. FIXED text, as
/// [`NAMESPACE_ALREADY_EXISTS_MESSAGE`] — it states that the name is taken, not
/// which name was asked for and not which namespace holds it.
pub const TABLE_ALREADY_EXISTS_MESSAGE: &str =
    "A table with an identical name already exists in the namespace.";

/// How this service states a value that cannot be a table bucket ARN at all —
/// the members the model constrains with `TableBucketARN`. FIXED text, as
/// [`INVALID_TABLE_BUCKET_NAME_MESSAGE`].
pub const INVALID_TABLE_BUCKET_ARN_MESSAGE: &str = "The specified table bucket ARN is not valid.";

/// How this service states a table bucket the store does not hold: the
/// `NotFoundException` (HTTP 404) the ARN-addressed operations model. FIXED
/// text — it names the CLASS of the failure and takes nothing from the request,
/// and it says "bucket" rather than "table bucket".
pub const BUCKET_NOT_FOUND_MESSAGE: &str = "The specified bucket does not exist.";

/// How this service states a request whose resource ARN names a Region other
/// than the one the endpoint serves: the client request error the model's
/// `BadRequestException` (HTTP 400) names, since re-pointing the client — not
/// retrying and not creating anything — is what makes the call possible. FIXED
/// text, as [`BUCKET_NOT_FOUND_MESSAGE`]: it names the class of the failure and
/// takes neither the Region of the request nor the Region of the ARN from it.
///
/// It is NOT a not-found: an ARN of another Region is an addressing mistake,
/// and whether a resource of that name exists in the Region it names is not
/// something this endpoint answers.
pub const REGION_MISMATCH_MESSAGE: &str =
    "The provided Region does not match the Region of the resource.";

/// How this service states a table bucket that has no resource-based policy:
/// the `NotFoundException` (HTTP 404) the bucket policy operations model. FIXED
/// text — it names the CLASS of the failure (there is no such policy) and takes
/// nothing from the request, as [`BUCKET_NOT_FOUND_MESSAGE`] does for the
/// bucket itself.
pub const BUCKET_POLICY_NOT_FOUND_MESSAGE: &str = "The specified bucket policy does not exist.";

/// How this service states a resource-based policy document it will not attach:
/// text that is not a resource policy at all (not JSON, no statement, a
/// statement naming no principal, action or effect). FIXED text, as
/// [`INVALID_TABLE_BUCKET_NAME_MESSAGE`]: it names the CLASS of the failure, says
/// nothing about which clause of the policy grammar the document broke, and
/// quotes nothing of the document back.
///
/// The wording is the one the recorded refusal of a malformed policy document
/// states, not this service's own phrasing — down to the absent full stop — and
/// the shared policy grammar
/// (`service_common::iam_policy::PolicyDefect`) deliberately leaves the sentence
/// to the service that refuses the document.
pub const INVALID_RESOURCE_POLICY_MESSAGE: &str = "The specified policy is malformed";

/// How this service states a `GetTable` request that addresses a table by
/// neither of the two whole forms the operation offers — the table's own ARN
/// alone, or the table bucket ARN, the namespace and the table name together.
/// FIXED text: it names both forms and takes nothing from the request.
pub const TABLE_ADDRESSING_MESSAGE: &str = "Invalid input: Must provide either table arn alone OR \
                                           all of (table bucket arn, namespace, table name) \
                                           together";

/// How this service states a table bucket that has no metrics configuration:
/// the `NotFoundException` (HTTP 404) the metrics operations model. FIXED
/// text — it names the CLASS of the failure (there is no such configuration)
/// and takes nothing from the request, as [`BUCKET_NOT_FOUND_MESSAGE`] does for
/// the bucket itself. The wording, down to the singular "metric" and the
/// absent full stop, is the one the recorded response for a bucket whose
/// metrics are off states, not this service's own phrasing.
pub const METRICS_CONFIGURATION_NOT_FOUND_MESSAGE: &str = "No such metric configuration exists";

/// How this service states a table bucket that has no replication
/// configuration: the `NotFoundException` (HTTP 404) the replication operations
/// model. FIXED text, as [`METRICS_CONFIGURATION_NOT_FOUND_MESSAGE`] — it names
/// the CLASS of the failure and takes nothing from the request.
pub const REPLICATION_CONFIGURATION_NOT_FOUND_MESSAGE: &str =
    "The specified replication configuration does not exist.";

/// How this service states a table bucket that has no encryption configuration
/// of its OWN to act on: the `NotFoundException` (HTTP 404) the encryption
/// operations model. FIXED text, as
/// [`REPLICATION_CONFIGURATION_NOT_FOUND_MESSAGE`] — it names the CLASS of the
/// failure (there is no such configuration) and takes nothing from the request
/// or from state.
///
/// A bucket nothing was ever put on is protected by
/// [`DEFAULT_BUCKET_ENCRYPTION`], which is the service's own default rather
/// than a configuration the bucket holds: there is nothing of the bucket's to
/// remove, which is the class this sentence states.
pub const ENCRYPTION_CONFIGURATION_NOT_FOUND_MESSAGE: &str =
    "The specified encryption configuration does not exist.";

/// How this service states a namespace the addressed table bucket does not
/// hold: the `NotFoundException` (HTTP 404) the namespace and table operations
/// model. FIXED text, as [`BUCKET_NOT_FOUND_MESSAGE`] does for a table bucket.
pub const NAMESPACE_NOT_FOUND_MESSAGE: &str = "The specified namespace does not exist.";

/// How this service states the namespace a move would put a resource INTO when
/// the addressed table bucket does not hold it: the same `NotFoundException`
/// (HTTP 404) as [`NAMESPACE_NOT_FOUND_MESSAGE`], stated as its own fixed
/// sentence because the error carries no code that tells the two apart — the
/// namespace the resource lives in is gone, or the namespace it is being moved
/// into was never created — and the sentence is the only discriminator a client
/// can match on. FIXED text, as [`NAMESPACE_NOT_FOUND_MESSAGE`]: it names the
/// class of the failure and takes nothing from the request.
pub const DESTINATION_NAMESPACE_NOT_FOUND_MESSAGE: &str =
    "The specified destination namespace does not exist.";

/// How this service states a table the store does not hold: the
/// `NotFoundException` (HTTP 404) the table operations model. FIXED text, as
/// [`BUCKET_NOT_FOUND_MESSAGE`] does for a table bucket.
pub const TABLE_NOT_FOUND_MESSAGE: &str = "The specified table does not exist.";

/// How this service states a delete of a table bucket that still holds
/// namespaces: the client request error the model's `BadRequestException`
/// (HTTP 400) names, since emptying the bucket — not retrying — is what makes
/// the delete possible. FIXED text, as [`BUCKET_NOT_FOUND_MESSAGE`] — it names
/// the class of the failure, takes nothing from the request, and says "bucket"
/// rather than "table bucket".
pub const BUCKET_NOT_EMPTY_MESSAGE: &str = "The bucket that you tried to delete is not empty.";

/// How this service states a delete of a namespace that still holds tables,
/// the namespace-level counterpart of [`BUCKET_NOT_EMPTY_MESSAGE`]: the same
/// client request error the model's `BadRequestException` (HTTP 400) names —
/// emptying the namespace, not retrying, is what makes the delete possible —
/// stated in the same fixed form, naming the resource by its class noun and
/// taking nothing from the request.
pub const NAMESPACE_NOT_EMPTY_MESSAGE: &str =
    "The namespace that you tried to delete is not empty.";

/// How this service states a rename that names neither a new namespace nor a
/// new table name: the client request error the model's `BadRequestException`
/// (HTTP 400) names, since such a request describes no move at all. FIXED
/// text — it names the two members that were both left out and takes nothing
/// from the request.
pub const NOTHING_TO_RENAME_MESSAGE: &str =
    "Neither a new namespace name nor a new table name is specified.";

/// How this service states a presented `versionToken` that is not the one the
/// configuration it addresses currently carries: the conflict with a previous
/// write the model's `ConflictException` (HTTP 409) names. FIXED text — the
/// token is neither echoed nor named.
///
/// This is the sentence of the operations that WRITE a replication
/// configuration under a precondition — `PutTableReplication` and
/// `PutTableBucketReplication` — and of `DeleteTableBucketReplication`, whose
/// token names a bucket's configuration and no table. An operation whose
/// refusal is stated against the TABLE it addresses carries
/// [`TABLE_VERSION_TOKEN_MISMATCH_MESSAGE`] instead; the two sentences are each
/// the one their own operations are recorded stating, so neither stands in for
/// the other.
pub const VERSION_TOKEN_MISMATCH_MESSAGE: &str =
    "The specified version token does not match the configuration.";

/// How this service states a presented `versionToken` that is not the one the
/// table it addresses currently carries, the table counterpart of
/// [`VERSION_TOKEN_MISMATCH_MESSAGE`]: the conflict with a previous write the
/// model's `ConflictException` (HTTP 409) names. FIXED text — the token is
/// neither echoed nor named, and neither is the table.
///
/// This is the sentence of every operation whose version-token refusal is
/// stated against the table the request addresses: `DeleteTable`,
/// `RenameTable`, `UpdateTableMetadataLocation` and the delete of a table's own
/// replication record, `DeleteTableReplication`.
pub const TABLE_VERSION_TOKEN_MISMATCH_MESSAGE: &str =
    "Provided version token does not match the table version token.";

/// How this service states a record expiration call on a table that takes no
/// record expiration configuration: the `MethodNotAllowedException` (HTTP 405)
/// the record expiration operations model. FIXED text — it names the class of
/// the failure and takes nothing from the request.
pub const RECORD_EXPIRATION_NOT_ALLOWED_MESSAGE: &str =
    "Table record expiration configuration API calls to this table are not allowed";

/// How this service states a value that cannot be a taggable resource's ARN at
/// all — the `resourceArn` member of the tagging operations, which the model
/// constrains with `ResourceArn`. FIXED text, as
/// [`INVALID_TABLE_BUCKET_NAME_MESSAGE`], and it names the member the way the
/// service does.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "The specified resourceArn is not valid.";

/// How this service states a table format it will not accept — the members the
/// model targets at the `OpenTableFormat` enum. FIXED text, as
/// [`INVALID_TABLE_BUCKET_NAME_MESSAGE`]: it names the CLASS of the failure and
/// neither echoes the value nor lists the formats this service does accept.
pub const INVALID_OPEN_TABLE_FORMAT_MESSAGE: &str = "The specified open table format is not valid";

/// How this service states a server-side encryption algorithm it will not
/// encrypt under — the `sseAlgorithm` member of every `encryptionConfiguration`,
/// which the model targets at the `SSEAlgorithm` enum. FIXED text, as
/// [`INVALID_OPEN_TABLE_FORMAT_MESSAGE`]: it names the CLASS of the failure and
/// neither echoes the value nor lists the algorithms this service does accept.
pub const INVALID_SSE_ALGORITHM_MESSAGE: &str = "The specified SSE algorithm is invalid.";

/// How this service states a KMS key it cannot encrypt under — the `kmsKeyArn`
/// member of every `encryptionConfiguration`, which the model bounds and
/// patterns. FIXED text: the form it quotes is the key ARN's SHAPE, with a
/// placeholder for each of its variable parts, so the sentence is the same
/// whatever key the request named and nothing of that key appears in it.
///
/// It words every way a configuration can name no usable key: a value outside
/// the modelled length or pattern, and a configuration whose algorithm and key
/// disagree — `aws:kms` with no key ARN at all, or a key ARN under an algorithm
/// that takes none ([`stored_encryption`]). In each the request names no key ARN
/// of the one form this service encrypts under, which is what the sentence
/// states.
pub const INVALID_KMS_KEY_ARN_MESSAGE: &str =
    "The specified KMS key ARN is invalid. Only key ARN format \
     'arn:<partition>:kms:<region>:<account-id>:key/<key-id>' is supported.";

/// How this service states a compaction strategy that orders a table's records
/// by its sort order, asked for on a table whose metadata carries no sort
/// order: the client request error the model's `BadRequestException` (HTTP 400)
/// names, since adding a sort order to the table — not retrying — is what makes
/// the configuration possible. FIXED text, as
/// [`INVALID_TABLE_BUCKET_NAME_MESSAGE`]: it names the class of the failure and
/// takes neither the strategy nor the table from the request.
pub const COMPACTION_SORT_ORDER_REQUIRED_MESSAGE: &str =
    "The specified compaction strategy requires sort order in the table metadata. Please refer to \
     the S3 Tables documentation for steps on configuring sort order.";

/// How this service states table metadata it will not accept: a `metadata`
/// document that does not describe a table this service can create — a column
/// of a type no open table format has, a sort or partition field naming a
/// column the schema does not declare, a transform no open table format
/// applies. FIXED text — it names the CLASS of the failure and takes nothing
/// from the request, as [`INVALID_TABLE_BUCKET_NAME_MESSAGE`] does for a name.
pub const INVALID_METADATA_MESSAGE: &str = "The specified metadata is not valid";

/// How this service states a metadata location it will not accept: one that is
/// not under the table's warehouse location, or that does not name a metadata
/// file of the table's open table format — the two rules
/// `UpdateTableMetadataLocation` documents for the member. FIXED text — it names
/// the CLASS of the failure and takes neither the location nor the warehouse
/// from the request, as [`INVALID_METADATA_MESSAGE`] does for a metadata
/// document.
pub const INVALID_METADATA_LOCATION_MESSAGE: &str =
    "The specified metadata location is not valid.";

/// How this service states a maintenance put that carries no configuration
/// value at all — the `value` member the model makes `@required` on both
/// maintenance puts. FIXED text: it names the member by WHAT it is and takes
/// nothing from the request.
pub const MAINTENANCE_VALUE_MISSING_MESSAGE: &str =
    "The table maintenance configuration value is not specified.";

/// How this service states a value outside the positive-integer range the model
/// bounds a counted member of a configuration at (`PositiveInteger`,
/// `@range(1, 2147483647)`): the member names ITSELF, under the name the wire
/// carries it by, and the requirement is that range stated in words.
///
/// `member` is a member name of the model, taken from the path of the violation
/// the boundary reported, so nothing of the request reaches the sentence.
pub fn invalid_positive_integer_message(member: &str) -> String {
    format!("The specified {member} is invalid. Value must be a positive integer.")
}

/// The smallest and largest file size S3 Tables compacts a table towards, in MB.
///
/// The model bounds `IcebergCompactionSettings.targetFileSizeMB` only as a
/// `PositiveInteger` (`@range(1..2147483647)`), so this window is the SERVICE's
/// own: the recorded refusal of a size outside it states both of these bounds,
/// which is where they come from, and [`invalid_compaction_file_size_message`]
/// is the one sentence that quotes them.
pub const COMPACTION_TARGET_FILE_SIZE_MIN_MB: i32 = 64;
pub const COMPACTION_TARGET_FILE_SIZE_MAX_MB: i32 = 512;

/// Whether `target_file_size_mb` is a file size S3 Tables compacts towards: one
/// inside the window above.
pub fn compaction_file_size_is_supported(target_file_size_mb: i32) -> bool {
    (COMPACTION_TARGET_FILE_SIZE_MIN_MB..=COMPACTION_TARGET_FILE_SIZE_MAX_MB)
        .contains(&target_file_size_mb)
}

/// How this service states a compaction file size outside the window it
/// compacts towards.
///
/// The sentence names the window's two bounds and takes NOTHING from the
/// request — the numbers are this service's constants, interpolated from the
/// constants above so the sentence cannot state a window other than the one
/// that is enforced.
pub fn invalid_compaction_file_size_message() -> String {
    format!(
        "The specified compaction file size is invalid. Minimum value: \
         {COMPACTION_TARGET_FILE_SIZE_MIN_MB} MB, maximum value: \
         {COMPACTION_TARGET_FILE_SIZE_MAX_MB} MB."
    )
}

/// How this service states a compaction strategy that names none of the ones S3
/// Tables compacts under — the `strategy` member of an `icebergCompaction`
/// maintenance setting, which the model targets at the
/// `IcebergCompactionStrategy` enum. FIXED text, as
/// [`INVALID_OPEN_TABLE_FORMAT_MESSAGE`]: it names the member class and neither
/// echoes the value nor lists the strategies this service does compact under.
pub const INVALID_COMPACTION_STRATEGY_MESSAGE: &str =
    "The compaction strategy specified is not valid.";

/// How this service states a status that is neither of the two a configuration's
/// `status` member takes: the members the model targets at `MaintenanceStatus`
/// (a maintenance configuration's) and at `TableRecordExpirationStatus` (a record
/// expiration configuration's), each an enum of `enabled` and `disabled`
/// (`smithy_query(query="shape",
/// shape="MaintenanceStatus,TableRecordExpirationStatus")`). FIXED text, as
/// [`INVALID_OPEN_TABLE_FORMAT_MESSAGE`]: it names the member and the two values
/// it takes, and echoes nothing of the request.
pub const INVALID_CONFIGURATION_STATUS_MESSAGE: &str =
    "The specified status is invalid. Value must be either 'enabled' or 'disabled.'";

/// How this service states a maintenance type that names no maintenance task of
/// the resource the request addresses — the `type` label of the maintenance
/// puts, which the model targets at `TableMaintenanceType` for a table's tasks
/// and at `TableBucketMaintenanceType` for a table bucket's.
///
/// The prefix is FIXED and the type is the one the request's path carried, so a
/// caller reads back which of its own words named no task. The echo is quoted to
/// the ceiling every clause of this workspace quotes a submitted value to
/// ([`::service_common::validation::quoted_echo`]), so no refusal is sized by
/// the request.
pub fn invalid_maintenance_type_message(addressed: &str) -> String {
    format!(
        "Invalid table maintenance type: {addressed}",
        addressed = ::service_common::validation::quoted_echo(addressed)
    )
}

/// How this service states a request body it cannot read into the operation's
/// modelled input at all — a member carrying a value of a JSON type the model
/// does not declare for it, or a body that is not the operation's input. FIXED
/// text: it points the caller at the reference that states the constraints
/// rather than describing the one value that failed, so nothing of the request
/// (and nothing of this emulator's codec) appears in it.
pub const INVALID_INPUT_MESSAGE: &str = "Invalid input. Consult the S3 Tables and/or Iceberg REST \
                                         Catalog API reference for constraints on request \
                                         parameters.";

/// Whether `name` satisfies the table-bucket naming rule the model's
/// `@pattern` cannot express: a name begins and ends with a letter or a digit,
/// so a hyphen may only appear between them.
///
/// The modeled pattern (`^[0-9a-z-]*$`) and length (3..63) are already enforced
/// at the boundary; this is the remaining rule, restated structurally rather
/// than as a second regex.
pub fn name_begins_and_ends_alphanumeric(name: &str) -> bool {
    let alphanumeric = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    name.chars().next().is_some_and(alphanumeric)
        && name.chars().next_back().is_some_and(alphanumeric)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3tablesS3tablebucketsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3tablesS3tablebucketsState>)
}
