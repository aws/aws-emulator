// SPDX-License-Identifier: CC0-1.0

//! The migrations of a source table into a table of this store: the record each
//! one leaves behind, the destination table it creates, and the states the CLOCK
//! moves it through.
//!
//! A migration is a control-plane resource of its own: `StartTableMigration`
//! "returns the migration ID and its initial state, and you track progress with
//! GetTableMigration", so every answer about a migration is read back out of the
//! record written here rather than recomputed from the request that started it.
//!
//! Two facts about a migration are this module's own, and both are facts about
//! THIS store:
//!
//! * the destination TABLE really stands in the bucket the request named. "The
//!   migration creates the table during planning and commits data to it as it
//!   runs", and planning begins when the request is accepted — so the table is
//!   created by the call that starts the migration
//!   ([`ComAmazonawsS3tablesS3tablebucketsState::create_migration_table`]) and is
//!   an ordinary table of this store from then on: `GetTable`, `ListTables` and
//!   every other read resolve it exactly as they resolve a table a caller
//!   created, and no second migration can take its name.
//! * the STATE a read reports is the one the clock has made due
//!   ([`Migration::advance_due`]). A migration "runs asynchronously", so the call
//!   that starts it answers while the migration is still at the state it was
//!   accepted in, and each later state becomes due a span after the one before
//!   it. Nothing sleeps: the transitions the clock already owes are materialised
//!   by the operations that read or write a migration, at the instant they were
//!   owed, so two reads of a migration nobody updated answer the same.
//!
//! What a migration MOVES is deliberately not claimed here. The emulator holds no
//! source objects to migrate, so a migration reports no planning or migration
//! statistics and no report objects: those members of `GetTableMigration` are
//! optional precisely because they are "returned after the migration writes the
//! report", and reporting counts for work nothing ran would be inventing them.
//! What is reported is the control plane — the migration's identity, its source,
//! its settings, where its reports are configured to go, and its state — all of
//! it read from this record.

use std::time::{Duration, SystemTime};

use super::*;

/// How long after a migration entered its current state the NEXT state of that
/// migration becomes due.
///
/// CHOSEN, not an AWS-published figure: S3 Tables publishes no migration timing,
/// and how long one takes depends on the source. What matters is that the span is
/// NOT zero — a caller that reads the migration straight after starting it still
/// sees the state the request was accepted in, and the state errors that go with
/// it — and that it is short enough to wait for rather than poll for hours.
pub const MIGRATION_STEP_DELAY: Duration = Duration::from_secs(3);

/// How long a migration waits in `pendingReview` for the caller to continue or
/// cancel it before it fails.
///
/// DOCUMENTED, not chosen: "If you don't continue or cancel the migration within
/// 30 days, the migration fails" (`StartTableMigration$reviewMigrationPlan`), so
/// the value is thirty days and not a span of this emulator's choosing.
pub const MIGRATION_REVIEW_DEADLINE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The most migrations this emulator keeps resident for one account.
///
/// CHOSEN, for the same reason as [`TABLE_BUCKETS_PER_ACCOUNT`]: the store is
/// BOUNDED, so a caller cannot grow the process's memory without limit by
/// starting migrations, and the refusal is the client-input error the operations
/// that start one model.
pub const MIGRATIONS_PER_ACCOUNT: usize = 1_000;

/// The most migrations one page of `ListTableMigrations` carries when the request
/// asks for no page size: "If you don't specify a value, the default is 100"
/// (`ListTableMigrations$maxResults`).
pub const MIGRATIONS_PAGE_SIZE_DEFAULT: i32 = 100;

/// `MigrationState` — the state of one migration. Values from
/// `com.amazonaws.s3tables#MigrationState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationState {
    /// S3 Tables accepted the request.
    New,
    /// The migration is checking the source and the destination for
    /// compatibility.
    Planning,
    /// Planning finished and the migration waits for the caller to continue or
    /// cancel it.
    PendingReview,
    /// The migration is waiting to start migrating data.
    Queued,
    /// The migration is running the plan planning produced.
    Migrating,
    /// Every planned object migrated.
    Succeeded,
    /// The migration failed.
    Failed,
    /// The migration is finishing the work in progress before it ends.
    Cancelling,
    /// The migration was cancelled.
    Cancelled,
}

impl MigrationState {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[
        Self::New,
        Self::Planning,
        Self::PendingReview,
        Self::Queued,
        Self::Migrating,
        Self::Succeeded,
        Self::Failed,
        Self::Cancelling,
        Self::Cancelled,
    ];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Planning => "planning",
            Self::PendingReview => "pendingReview",
            Self::Queued => "queued",
            Self::Migrating => "migrating",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }

    /// Whether the migration has ENDED: the three states `CompleteTableMigration`
    /// names as the ones a table's most recent migration must be in ("The table's
    /// most recent migration must be in the succeeded, failed or cancelled
    /// state"), and the states no further transition is owed from.
    pub fn has_ended(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// Whether a request can still cancel the migration: the five states
    /// `UpdateTableMigration` names for `cancelled` ("to cancel a migration in
    /// the new, planning, pendingReview, queued or migrating state").
    pub fn is_cancellable(self) -> bool {
        matches!(
            self,
            Self::New | Self::Planning | Self::PendingReview | Self::Queued | Self::Migrating
        )
    }
}

/// `MigrationType` — which operation started the migration. Values from
/// `com.amazonaws.s3tables#MigrationType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationType {
    /// "A migration that StartTableMigration started."
    Start,
    /// "A migration that SyncTableMigration started."
    Sync,
}

impl MigrationType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Start, Self::Sync];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Sync => "sync",
        }
    }
}

/// `MigrationRequestedState` — the state `UpdateTableMigration` asks a migration
/// to move to. Values from `com.amazonaws.s3tables#MigrationRequestedState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationRequestedState {
    /// Continue a migration that is waiting for a plan review.
    Queued,
    /// Cancel the migration.
    Cancelled,
}

impl MigrationRequestedState {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Queued, Self::Cancelled];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Cancelled => "cancelled",
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

/// `MigrationSourceType` — the kind of source a migration reads from. Values from
/// `com.amazonaws.s3tables#MigrationSourceType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationSourceType {
    /// An Amazon Web Services Glue Data Catalog table.
    GlueTable,
}

impl MigrationSourceType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::GlueTable];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::GlueTable => "glueTable",
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

/// The Amazon Web Services Glue Data Catalog table a migration reads from: the
/// catalog, the database and the table, as the request named them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlueTableSourceRecord {
    pub catalog_id: String,
    pub database_name: String,
    pub table_name: String,
}

/// The source a migration reads from — one variant per member of the modelled
/// `SourceConfiguration` union, so the stored source names exactly one source and
/// [`Self::source_type`] is what `sourceType` has to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationSource {
    GlueTable(GlueTableSourceRecord),
}

impl MigrationSource {
    /// The `sourceType` this source is of: "The type of the source. It must match
    /// the member that you set in source" (`StartTableMigration$sourceType`), so
    /// the type is READ off the source rather than taken from the request.
    pub fn source_type(&self) -> MigrationSourceType {
        match self {
            Self::GlueTable(_) => MigrationSourceType::GlueTable,
        }
    }
}

/// `MigrationPolicies$unmatchedSourceFieldsPolicy` — what a migration does with a
/// source field the destination schema has no column for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmatchedSourceFieldsPolicy {
    /// "Fails the migration. This is the default."
    Fail,
    /// "Leaves source columns that the destination schema doesn't have out of the
    /// destination table".
    Drop,
}

impl UnmatchedSourceFieldsPolicy {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Fail, Self::Drop];

    /// The policy a migration applies when the request names none: the member's
    /// own modelled `@default("fail")`.
    pub const DEFAULT: Self = Self::Fail;

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Drop => "drop",
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

/// `MigrationPolicies$incompatibleValuesPolicy` — what a migration does with a
/// source value the destination column's type cannot hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompatibleValuesPolicy {
    /// "Fails the migration. This is the default."
    Fail,
    /// "Writes null for a value that the destination column's type can't hold."
    SetNull,
}

impl IncompatibleValuesPolicy {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Fail, Self::SetNull];

    /// The policy a migration applies when the request names none: the member's
    /// own modelled `@default("fail")`.
    pub const DEFAULT: Self = Self::Fail;

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::SetNull => "setNull",
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

/// The file size the data files a migration writes are sized towards when the
/// request names none: `MigrationCompactionSettings$targetFileSizeMB`'s own
/// modelled `@default(512)`.
pub const MIGRATION_TARGET_FILE_SIZE_MB_DEFAULT: i32 = 512;

/// The compaction strategy the destination table of a migration is compacted
/// under: "The compaction strategy. The only valid value is `binpack`, which is
/// the default" (`MigrationCompactionSettings$strategy`) — so it is both the
/// default and the only strategy a migration is accepted with.
pub const MIGRATION_COMPACTION_STRATEGY: IcebergCompactionStrategy =
    IcebergCompactionStrategy::Binpack;

/// The fewest snapshots a migration's destination table keeps when the request
/// names none: `MigrationSnapshotManagementSettings$minSnapshotsToKeep`'s own
/// modelled `@default(3)`.
pub const MIGRATION_MIN_SNAPSHOTS_TO_KEEP_DEFAULT: i32 = 3;

/// The oldest a snapshot of a migration's destination table gets before it can be
/// expired, in hours, when the request names none:
/// `MigrationSnapshotManagementSettings$maxSnapshotAgeHours`'s own modelled
/// `@default(120)`.
pub const MIGRATION_MAX_SNAPSHOT_AGE_HOURS_DEFAULT: i32 = 120;

/// The maintenance settings and migration policies a migration applies to its
/// destination table, with every member the request left out settled at the
/// model's own default for it — so what is stored is one whole configuration and
/// a read of it answers the same values whichever members the request carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataMigration {
    pub target_file_size_mb: i32,
    pub compaction_strategy: IcebergCompactionStrategy,
    pub min_snapshots_to_keep: i32,
    pub max_snapshot_age_hours: i32,
    pub unmatched_source_fields: UnmatchedSourceFieldsPolicy,
    pub incompatible_values: IncompatibleValuesPolicy,
}

impl Default for DataMigration {
    /// The settings a migration runs under when the request names none of them.
    fn default() -> Self {
        Self {
            target_file_size_mb: MIGRATION_TARGET_FILE_SIZE_MB_DEFAULT,
            compaction_strategy: MIGRATION_COMPACTION_STRATEGY,
            min_snapshots_to_keep: MIGRATION_MIN_SNAPSHOTS_TO_KEEP_DEFAULT,
            max_snapshot_age_hours: MIGRATION_MAX_SNAPSHOT_AGE_HOURS_DEFAULT,
            unmatched_source_fields: UnmatchedSourceFieldsPolicy::DEFAULT,
            incompatible_values: IncompatibleValuesPolicy::DEFAULT,
        }
    }
}

/// Where a migration writes its reports: the general purpose bucket every report
/// goes to, and the key prefix of each one.
///
/// A prefix is `None` for a report the request named none for, because "If you
/// don't specify a prefix for a report, its keys start at the root of the bucket"
/// — there is no prefix to report rather than an empty one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReports {
    pub bucket: String,
    pub table_compatibility_prefix: Option<String>,
    pub object_compatibility_prefix: Option<String>,
    pub object_migration_prefix: Option<String>,
}

/// One migration, as this emulator stores it.
///
/// `table_arn` is always set: the destination table is created by the call that
/// starts the migration (see the module docs), so every migration this store
/// holds names a table that really stands in the bucket.
#[derive(Debug, Clone)]
pub struct Migration {
    pub id: String,
    pub migration_type: MigrationType,
    pub table_bucket_arn: String,
    pub namespace: String,
    pub name: String,
    pub table_arn: String,
    pub role_arn: String,
    pub review_migration_plan: bool,
    pub source: MigrationSource,
    pub data_migration: DataMigration,
    pub reports: MigrationReports,
    pub created_at: SystemTime,
    pub state: MigrationState,
    /// When the migration ENTERED [`Self::state`] — the instant that state became
    /// due, not the instant somebody asked about it, so a read never moves it.
    pub state_updated_at: SystemTime,
    /// Whether "planning produced a plan that the migration can run", and `None`
    /// while planning has not finished, which is exactly when the member is left
    /// out of a read ("If planning hasn't finished, this value isn't returned").
    pub has_valid_plan: Option<bool>,
    /// The order this store recorded the migration in, counting up, so the
    /// listing can hand them back newest first whatever the clock did.
    pub sequence: u64,
}

impl Migration {
    /// The next state this migration owes and the instant it is owed at, or
    /// `None` when the migration has ended and nothing further is due.
    ///
    /// `checked_add`, so a clock far enough in the future that the span does not
    /// fit cannot panic a read.
    fn next_due(&self) -> Option<(SystemTime, MigrationState)> {
        let (next, after) = match self.state {
            // Planning starts as soon as the request is accepted.
            MigrationState::New => (MigrationState::Planning, MIGRATION_STEP_DELAY),
            // Where planning leaves the migration is what the request asked for:
            // it waits for a review when `reviewMigrationPlan` is set, and is
            // queued for the data migration otherwise.
            MigrationState::Planning => (
                if self.review_migration_plan {
                    MigrationState::PendingReview
                } else {
                    MigrationState::Queued
                },
                MIGRATION_STEP_DELAY,
            ),
            // A migration waiting for a review waits for the CALLER
            // (`UpdateTableMigration`), and fails if nobody continues or cancels
            // it inside the documented window.
            MigrationState::PendingReview => (MigrationState::Failed, MIGRATION_REVIEW_DEADLINE),
            MigrationState::Queued => (MigrationState::Migrating, MIGRATION_STEP_DELAY),
            MigrationState::Migrating => (MigrationState::Succeeded, MIGRATION_STEP_DELAY),
            // A cancelled migration "finishes the work in progress, and then ends
            // in the cancelled state".
            MigrationState::Cancelling => (MigrationState::Cancelled, MIGRATION_STEP_DELAY),
            MigrationState::Succeeded | MigrationState::Failed | MigrationState::Cancelled => {
                return None
            }
        };
        let due = self
            .state_updated_at
            .checked_add(after)
            .unwrap_or(self.state_updated_at);
        Some((due, next))
    }

    /// Move the migration into `state`, recording `at` as the instant it entered
    /// it.
    ///
    /// The one place a migration's state is written, so every mover — the clock
    /// below and `UpdateTableMigration` — leaves the same facts behind it.
    /// Planning FINISHED when the migration leaves it for a state that waits on
    /// or runs the plan it produced, which is when "planning produced a plan that
    /// the migration can run" becomes an answerable question; a migration
    /// cancelled while it was still planning produced none, so it keeps reporting
    /// no value for it.
    pub fn enter(&mut self, state: MigrationState, at: SystemTime) {
        if self.state == MigrationState::Planning
            && matches!(
                state,
                MigrationState::PendingReview | MigrationState::Queued
            )
        {
            self.has_valid_plan = Some(true);
        }
        self.state = state;
        self.state_updated_at = at;
    }

    /// Carry out the state changes of this migration that the CLOCK has already
    /// made due, as of `at`.
    ///
    /// A migration "runs asynchronously": the call that starts it answers while
    /// the migration is still at the state it was accepted in, and each later
    /// state falls due a span after the one before it. Owed at the same instant
    /// whether or not anybody asked, which is why the operations that REPORT a
    /// migration take it, and IDEMPOTENT — every state is entered at the instant
    /// it was due, so two reads of a migration nobody updated answer the same.
    pub fn advance_due(&mut self, at: SystemTime) {
        while let Some((due, next)) = self.next_due() {
            if at < due {
                break;
            }
            self.enter(next, due);
        }
    }

    /// The key this migration is listed under: the order the listing pages over.
    ///
    /// "Migrations are listed newest first" (`ListTableMigrations`), so the key
    /// counts DOWN from the order the store recorded them in — a fixed-width
    /// decimal, so the keys sort by the number they spell rather than by its
    /// width, and the listing's ascending walk over them is newest first.
    pub fn listing_key(&self) -> String {
        format!("{:020}", u64::MAX - self.sequence)
    }
}

impl ComAmazonawsS3tablesS3tablebucketsState {
    /// A fresh `migrationId`, retried against the store so an id is never reused.
    ///
    /// The model constrains a `MigrationId` by length (`@length(1, 64)`) and by
    /// character set (`@pattern(^[a-z0-9-]+$)`), which the canonical hyphenated
    /// UUID form every identifier this service mints carries satisfies — so a
    /// migration is identified the way every other resource of this service is.
    pub fn mint_migration_id(&mut self) -> String {
        loop {
            let candidate = self.mint_id("table-migration");
            if !self.migrations.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Record `migration` and answer the id it is held under.
    ///
    /// The one place a migration is written, so the sequence that orders the
    /// listing is counted here and nowhere else.
    pub fn record_migration(&mut self, mut migration: Migration) -> String {
        self.migrations_recorded = self.migrations_recorded.wrapping_add(1);
        migration.sequence = self.migrations_recorded;
        let id = migration.id.clone();
        self.migrations.insert(id.clone(), migration);
        id
    }

    /// Whether the store already holds [`MIGRATIONS_PER_ACCOUNT`] migrations, so
    /// a further one must be refused.
    pub fn is_at_migration_quota(&self) -> bool {
        self.migrations.len() >= MIGRATIONS_PER_ACCOUNT
    }

    /// The migrations this store holds, newest first — the order
    /// `ListTableMigrations` reports them in, beside the key each is paged over
    /// by.
    pub fn migrations_newest_first(&self) -> impl Iterator<Item = (String, &Migration)> {
        let mut ordered: Vec<&Migration> = self.migrations.values().collect();
        ordered.sort_by_key(|migration| migration.listing_key());
        ordered
            .into_iter()
            .map(|migration| (migration.listing_key(), migration))
    }

    /// The MOST RECENT migration whose destination is the table `table_arn`, or
    /// `None` when no migration of this store has that table as its destination.
    ///
    /// What the operations that address a migration "by the tableARN of its
    /// destination table" resolve to, and what the rules a table's migration
    /// history carries — whether a sync may start, whether the migration may be
    /// completed — are decided against.
    pub fn latest_migration_of_table(&self, table_arn: &str) -> Option<&Migration> {
        self.migrations
            .values()
            .filter(|migration| migration.table_arn == table_arn)
            .max_by_key(|migration| migration.sequence)
    }

    /// Whether any migration into the table `table_arn` produced a plan the
    /// migration could run.
    ///
    /// The condition a sync takes: "You can sync after a start or sync that
    /// succeeded, failed or was cancelled, if an earlier start or sync of the
    /// table has a valid plan (hasValidPlan)".
    pub fn table_has_a_valid_migration_plan(&self, table_arn: &str) -> bool {
        self.migrations
            .values()
            .filter(|migration| migration.table_arn == table_arn)
            .any(|migration| migration.has_valid_plan == Some(true))
    }

    /// Carry out the state changes of every resident migration that the CLOCK has
    /// already made due, as of `at`.
    ///
    /// The materialisation each migration operation takes before it reads or
    /// writes anything, for the reason [`Migration::advance_due`] states: the
    /// transitions were owed at the same instant whether or not anybody asked.
    pub fn advance_migrations_due(&mut self, at: SystemTime) {
        for migration in self.migrations.values_mut() {
            migration.advance_due(at);
        }
    }

    /// Create the table a migration migrates INTO — `name` in `namespace` of the
    /// table bucket `table_bucket_arn` — and answer its `tableARN`.
    ///
    /// The record is built the way `CreateTable` builds one: the table is a
    /// table of the ACCOUNT's, not one Amazon Web Services manages on its behalf,
    /// because once the migration is complete "you can write to and maintain the
    /// table as you do any other table". `encryption` and `storage_class` are
    /// what the migration's `destinationTable` named, so an absent one leaves the
    /// table protected and stored the way its bucket is — the same thing a create
    /// that names neither settles.
    ///
    /// The table carries no metadata file: the document that describes a table's
    /// data is written by the commit that puts data in it, which is the
    /// migration's own work and is not something this emulator runs. So a
    /// freshly migrated table reads exactly as a table created with no
    /// `metadata` does, and the Iceberg catalog is where a metadata document for
    /// it comes from.
    ///
    /// The refusals are the ones a create meets, reported as facts about this
    /// store ([`ServiceManagedTableRefusal`]) for the operation that asked to
    /// word in its own modelled error class.
    #[allow(clippy::too_many_arguments)]
    pub fn create_migration_table(
        &mut self,
        table_bucket_arn: &str,
        namespace: &str,
        name: &str,
        encryption: Option<BucketEncryption>,
        storage_class: Option<StorageClass>,
        tags: BTreeMap<String, String>,
        now: SystemTime,
    ) -> Result<String, ServiceManagedTableRefusal> {
        let bucket = self
            .table_buckets
            .get(table_bucket_arn)
            .ok_or(ServiceManagedTableRefusal::NoSuchTableBucket)?;
        let bucket_arn = bucket.arn.clone();
        let bucket_id = bucket.bucket_id.clone();
        let namespace_record = bucket
            .namespaces
            .get(namespace)
            .ok_or(ServiceManagedTableRefusal::NoSuchNamespace)?;
        let namespace_id = namespace_record.namespace_id.clone();
        let namespace_names = namespace_record.namespace.clone();
        // "The namespace must already exist, and a table with the specified name
        // must not" (`StartTableMigration`): a name any table stands under is
        // taken, whoever created it, and no table is replaced.
        if namespace_record.tables.contains_key(name) {
            return Err(ServiceManagedTableRefusal::NameTaken);
        }
        // The store is BOUNDED, as it is for a create that arrives on the wire.
        if namespace_record.tables.len() >= TABLES_PER_NAMESPACE {
            return Err(ServiceManagedTableRefusal::NamespaceFull);
        }

        let table_id = self.mint_table_id();
        let version_token = self.mint_version_token();
        let table_arn = table_arn(&bucket_arn, &table_id);
        let warehouse_location = warehouse_location(&table_id);
        let account = ::engine::emulator_account_id().to_string();
        let table = Table {
            name: name.to_string(),
            table_arn: table_arn.clone(),
            // The migrated table is the account's own, not one Amazon Web
            // Services manages for it.
            table_type: TableType::Customer,
            managed_by_service: None,
            namespace: namespace_names,
            namespace_id,
            table_bucket_id: bucket_id,
            // The one format the model declares, and the one a migration
            // migrates into.
            format: OpenTableFormat::Iceberg,
            version_token,
            metadata_location: None,
            iceberg: None,
            warehouse_location,
            created_at: now,
            created_by: account.clone(),
            modified_at: now,
            modified_by: None,
            owner_account_id: account,
            encryption,
            storage_class,
            record_expiration: None,
            replication: None,
            maintenance: BTreeMap::new(),
            maintenance_schedules: BTreeMap::new(),
            // The migration of the table is not complete until
            // `CompleteTableMigration` is called for it.
            migration_completed_at: None,
            tags,
        };

        let Some(namespace_record) = self
            .table_buckets
            .get_mut(table_bucket_arn)
            .and_then(|bucket| bucket.namespaces.get_mut(namespace))
        else {
            // Unreachable: the namespace was resident a few lines above and this
            // call holds the store.
            return Err(ServiceManagedTableRefusal::NoSuchNamespace);
        };
        namespace_record.tables.insert(name.to_string(), table);
        // The index the `tableArn` addressing mode resolves through, so every
        // ARN-addressed read reaches this table too.
        self.tables_by_arn.insert(
            table_arn.clone(),
            TableLocation {
                table_bucket_arn: bucket_arn,
                namespace: namespace.to_string(),
                name: name.to_string(),
            },
        );
        Ok(table_arn)
    }
}

/// How a request addresses ONE migration: by the migration's own id, or by the
/// ARN of its destination table.
///
/// The two forms are whole and exclusive — "Specify either migrationId or
/// tableARN, not both" (`GetTableMigration`, `UpdateTableMigration`) — so this is
/// what a request that names both, or neither, fails to build: such a request
/// addresses no migration and is refused with
/// [`MIGRATION_ADDRESSING_MESSAGE`] rather than resolved from whichever member it
/// happened to carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressedMigration {
    /// The migration held under this `migrationId`.
    Id(String),
    /// The MOST RECENT migration whose destination is the table this `tableARN`
    /// names: "Gets the table's most recent migration" (`GetTableMigration`).
    DestinationTable(String),
}

impl AddressedMigration {
    /// What the two members address, or `None` when they name both migrations or
    /// none.
    pub fn of(migration_id: Option<&str>, table_arn: Option<&str>) -> Option<Self> {
        match (migration_id, table_arn) {
            (Some(id), None) => Some(Self::Id(id.to_string())),
            (None, Some(table_arn)) => Some(Self::DestinationTable(table_arn.to_string())),
            (Some(_), Some(_)) | (None, None) => None,
        }
    }
}

impl ComAmazonawsS3tablesS3tablebucketsState {
    /// The migration `addressed` names, and `None` when the store holds none.
    pub fn addressed_migration(&self, addressed: &AddressedMigration) -> Option<&Migration> {
        match addressed {
            AddressedMigration::Id(id) => self.migrations.get(id),
            AddressedMigration::DestinationTable(table_arn) => {
                self.latest_migration_of_table(table_arn)
            }
        }
    }

    /// The migration `addressed` names, for a caller that is about to change it.
    pub fn addressed_migration_mut(
        &mut self,
        addressed: &AddressedMigration,
    ) -> Option<&mut Migration> {
        let id = self.addressed_migration(addressed)?.id.clone();
        self.migrations.get_mut(&id)
    }
}

/// How this service states a migration the store does not hold: the
/// `NotFoundException` (HTTP 404) the migration operations model. FIXED text, as
/// [`BUCKET_NOT_FOUND_MESSAGE`] does for a table bucket — it names the CLASS of
/// the failure and takes nothing from the request, and it covers both ways a
/// request names none: an id nothing is stored under, and a destination table no
/// migration of this account ever migrated into.
pub const MIGRATION_NOT_FOUND_MESSAGE: &str = "The specified migration does not exist.";

/// How this service states a request that addresses a migration by neither of the
/// two whole forms the migration operations offer — the migration's own id, or
/// the ARN of its destination table ("Specify either migrationId or tableARN, not
/// both"). FIXED text, in the form this service's own addressing refusal takes
/// ([`TABLE_ADDRESSING_MESSAGE`]): it names both forms and takes nothing from the
/// request.
pub const MIGRATION_ADDRESSING_MESSAGE: &str =
    "Invalid input: Must provide either migrationId alone OR tableARN alone";

/// How this service states a `ListTableMigrations` request that narrows the
/// listing by a member whose own coordinates it leaves out: "The namespace to
/// list migrations for. Requires tableBucketARN", "The name of the table to list
/// migrations for. Requires tableBucketARN and namespace", and "Specify either
/// tableARN or tableBucketARN, not both". FIXED text, as
/// [`MIGRATION_ADDRESSING_MESSAGE`].
pub const MIGRATION_LISTING_ADDRESSING_MESSAGE: &str =
    "Invalid input: Must provide either tableARN alone OR a table bucket arn, optionally narrowed \
     by namespace and then table name";

/// How this service states a `sourceType` that does not name the source the
/// request set: "The type of the source. It must match the member that you set in
/// source" (`StartTableMigration$sourceType`). The client request error the
/// model's `BadRequestException` (HTTP 400) names, in FIXED text that names the
/// two members and echoes neither.
pub const MIGRATION_SOURCE_TYPE_MISMATCH_MESSAGE: &str =
    "The specified sourceType does not match the source.";

/// How this service states a request to continue a migration that is not waiting
/// for one: `UpdateTableMigration` moves a migration to `queued` only "to
/// continue a migration in the pendingReview state", so a migration in any other
/// state conflicts with the request — the `ConflictException` (HTTP 409) the
/// operation models. FIXED text: it names the class and takes neither the
/// migration nor its state from the request.
pub const MIGRATION_NOT_PENDING_REVIEW_MESSAGE: &str =
    "The specified migration is not waiting for a plan review.";

/// How this service states a request to cancel a migration that has already
/// ended — one in the succeeded, failed or cancelled state, which
/// `UpdateTableMigration` does not list for `cancelled`. FIXED text, as
/// [`MIGRATION_NOT_PENDING_REVIEW_MESSAGE`].
pub const MIGRATION_ALREADY_ENDED_MESSAGE: &str = "The specified migration has already ended.";

/// How this service states a `CompleteTableMigration` on a table whose most
/// recent migration is still running: "The table's most recent migration must be
/// in the succeeded, failed or cancelled state", so a migration that has not
/// ended conflicts with the request. FIXED text, as
/// [`MIGRATION_NOT_PENDING_REVIEW_MESSAGE`].
pub const MIGRATION_HAS_NOT_ENDED_MESSAGE: &str =
    "The most recent migration of the specified table has not ended.";

/// How this service states a call on a table whose migration is already complete:
/// "You can't undo this operation. Calling it again for the same table returns a
/// ConflictException" (`CompleteTableMigration`), and a sync is accepted only
/// "until you call CompleteTableMigration" for the table. FIXED text, as
/// [`MIGRATION_NOT_PENDING_REVIEW_MESSAGE`].
pub const MIGRATION_ALREADY_COMPLETE_MESSAGE: &str =
    "The migration of the specified table is already complete.";

/// How this service states a sync asked for while a migration of the table is
/// still running: "You can't sync a table while a start or sync is in progress
/// for it" (`SyncTableMigration`). FIXED text, as
/// [`MIGRATION_NOT_PENDING_REVIEW_MESSAGE`].
pub const MIGRATION_IN_PROGRESS_MESSAGE: &str =
    "A migration of the specified table is already in progress.";

/// How this service states a sync of a table no migration of which produced a
/// runnable plan: a sync is accepted only "if an earlier start or sync of the
/// table has a valid plan (hasValidPlan)" (`SyncTableMigration`). FIXED text, as
/// [`MIGRATION_NOT_PENDING_REVIEW_MESSAGE`].
pub const MIGRATION_HAS_NO_VALID_PLAN_MESSAGE: &str =
    "No migration of the specified table has a valid plan.";

/// How this service states a table that is not a replica, asked to be
/// disassociated from replication: "You can only disassociate a replica table. If
/// you call this operation on a table that isn't a replica table, including a
/// table that you have already disassociated, the request fails"
/// (`DisassociateTableReplication`) — the `ConflictException` (HTTP 409) the
/// operation models, since what refuses the call is the state of a previous
/// write. FIXED text: it names the class and takes nothing from the request.
pub const TABLE_IS_NOT_A_REPLICA_MESSAGE: &str = "The specified table is not a replica table.";
