// SPDX-License-Identifier: CC0-1.0

use super::*;

/// A stored DB CLUSTER snapshot: the cluster's settings as they stood when the snapshot
/// was taken, which is why they are copied rather than re-read later.
///
/// `CreateDBClusterSnapshot` writes one (a snapshot OF a cluster, through
/// [`DbClusterSnapshotRecord::from_cluster`]), `CopyDBClusterSnapshot` writes one (a copy
/// of another snapshot), the final snapshot `DeleteDBCluster` takes is one more and
/// `DeleteDBClusterSnapshot` removes one; `RestoreDBClusterFromSnapshot` is what READS one
/// back into a cluster. The reading operations are implemented against these records rather
/// than against a canned answer, so they report exactly what is stored.
#[derive(Debug, Clone)]
pub struct DbClusterSnapshotRecord {
    /// `DBClusterSnapshotIdentifier`, lowercased, and this record's key within its
    /// region: the member documents "This value is stored as a lowercase string".
    pub snapshot_identifier: String,
    /// The region the snapshot lives in; the scope of its ARN.
    pub region: String,
    /// `DBClusterIdentifier` of the cluster the snapshot was taken from.
    pub db_cluster_identifier: String,
    /// `DbClusterResourceId`, which `DescribeDBClusterSnapshots` can select on.
    pub db_cluster_resource_id: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub engine_mode: Option<String>,
    /// `DatabaseName` of the cluster the snapshot was taken from — the database its data
    /// lives in — which is the name a cluster restored from this snapshot carries when the
    /// restore names none of its own. Not a member of the `DBClusterSnapshot` wire shape:
    /// it is one of the cluster settings the snapshot copied, like the ones below.
    pub database_name: Option<String>,
    /// `DBClusterInstanceClass` of the cluster the snapshot was taken from — the compute
    /// and memory capacity of each DB instance in a Multi-AZ DB cluster — which is the
    /// class a DB instance restored from this snapshot is provisioned with when the
    /// request names none. `None` for a cluster that carries no instance class of its own
    /// (an Aurora cluster, whose snapshots a DB instance cannot be restored from).
    pub db_instance_class: Option<String>,
    /// `AllocatedStorage` of the cluster the snapshot was taken from, which the snapshot
    /// reports until its own size is known; see [`Self::to_wire`].
    pub allocated_storage: Option<i32>,
    /// Whether the cluster the snapshot was taken from had a DB INSTANCE in it, i.e.
    /// whether the snapshot holds any data at all.
    pub holds_data: bool,
    pub master_username: Option<String>,
    /// The master user's password as the source cluster held it, kept only as the DIGESTS a
    /// login can be verified against — the password itself is `@sensitive`, is never
    /// reported and is not stored ([`data_plane::wire_session::MasterPasswordHash`]). It is
    /// part of what the snapshot copied off the cluster, because a cluster restored from it
    /// keeps the source's master credential: the restore mints no new one. `None` for a
    /// source cluster that had no master password of its own.
    pub master_user_password: Option<data_plane::wire_session::MasterPasswordHash>,
    pub availability_zones: Vec<String>,
    pub port: i32,
    pub storage_type: Option<String>,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub license_model: Option<String>,
    /// `VpcId`: the VPC of the cluster the snapshot was taken from.
    pub vpc_id: String,
    /// `BackupRetentionPeriod`, as the source cluster retained automated backups when the
    /// snapshot was taken.
    pub backup_retention_period: Option<i32>,
    /// `PreferredBackupWindow`, as the source cluster carried it when the snapshot was
    /// taken.
    pub preferred_backup_window: Option<String>,
    /// `IAMDatabaseAuthenticationEnabled`, as the source cluster carried it.
    pub iam_database_authentication_enabled: bool,
    /// `SnapshotType`: `manual` for every snapshot a caller asked for.
    pub snapshot_type: String,
    /// `Status`: [`SNAPSHOT_STATUS_COPYING`] while a copy is being made,
    /// [`SNAPSHOT_STATUS_AVAILABLE`] once the clock has made it due.
    pub status: String,
    /// The status this record owes, and when; see [`PendingStatus`].
    pub pending_status: Option<PendingStatus>,
    /// The instant the clock makes — or made — this snapshot READY for the first time, as
    /// [`DbInstanceRecord::ready_at`] records it for a DB instance: what a resource
    /// provisioned FROM this snapshot (a copy of it, a restore of it, an export of it)
    /// charges its own span from ([`provisioning_to`]).
    pub ready_at: Instant,
    pub cluster_create_time: ::aws_smithy_types::DateTime,
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    /// `SourceDBClusterSnapshotArn`: the snapshot this one was copied from, absent for a
    /// snapshot taken from a cluster.
    pub source_snapshot_arn: Option<String>,
    /// The accounts the `restore` attribute authorizes, which
    /// `ModifyDBClusterSnapshotAttribute` adds to and removes from and
    /// `DescribeDBClusterSnapshotAttributes` reports; see [`SNAPSHOT_RESTORE_ATTRIBUTE`].
    /// A set, so authorizing an account twice changes nothing and the report is ordered.
    pub restore_attribute: BTreeSet<String>,
    /// A COPY of the data the source cluster's storage held when the snapshot was taken,
    /// taken by the engine itself ([`data_plane::sql_engine::InstanceDatabase::capture`]):
    /// this is what makes a cluster snapshot restorable, because
    /// `RestoreDBClusterFromSnapshot` loads it into the restored cluster's own storage.
    /// `None` for a snapshot of a cluster that has no engine endpoint at all — an engine
    /// whose wire this emulator serves no data plane for
    /// ([`data_plane::cluster_endpoint::cluster_engine_wire`]).
    ///
    /// The copy is in memory and private to this snapshot, it is never written to the host,
    /// and its size is bounded by the same per-database page ceiling every database of this
    /// service's data plane carries; the number of copies is bounded by
    /// [`DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION`] and [`MAX_REGIONS`], like every other record
    /// here. Deleting the snapshot drops it.
    pub storage: Option<::std::sync::Arc<data_plane::sql_engine::InstanceDatabase>>,
    /// A COPY of the databases the source cluster served through the RDS DATA API when the
    /// snapshot was taken, which is the other half of the data that cluster held: those
    /// databases are the Data API service's own state, so the copy is made and installed by
    /// that service and is opaque here ([`DataApiClusterData`]).
    ///
    /// `None` for a snapshot of a cluster no Data API statement had ever been run against,
    /// and for one taken on an emulator assembled without that service — both of which held
    /// no such database to copy. Carried by `CopyDBClusterSnapshot` with the rest of the
    /// record, and installed on the cluster `RestoreDBClusterFromSnapshot` creates.
    ///
    /// [`Self::data_bytes`] does NOT include it: that number is measured by walking the
    /// VOLUME copy's own tables ([`snapshot_data_bytes`]), and these databases belong to
    /// another service, which publishes a copy rather than a size. The two surfaces are to
    /// become one volume per cluster, at which point there is one thing to measure; until
    /// then the number covers the volume the cluster's own engine endpoint serves.
    pub data_api_storage: Option<::std::sync::Arc<DataApiClusterData>>,
    /// The full size, in bytes, of the data [`Self::storage`] holds.
    ///
    /// Summed ONCE, where the copy was taken ([`snapshot_data_bytes`], called off the
    /// request's worker thread by [`data_plane::captured_data_bytes`]), because the copy
    /// never changes afterwards: a read of the snapshot reads this number rather than
    /// walking its data again. Zero for a snapshot that captured no storage at all and for
    /// one whose databases were never written to.
    pub data_bytes: i64,
    pub tags: Vec<Tag>,
}

impl DbClusterSnapshotRecord {
    /// Take a snapshot OF `cluster`: its settings as they stand now, copied onto the
    /// snapshot so a later change to the cluster cannot rewrite what the snapshot holds.
    ///
    /// `SnapshotType` is `manual` — the class AWS gives every snapshot a caller asked for,
    /// `automated` being reserved for the backups the service schedules itself — and the
    /// snapshot answers [`SNAPSHOT_STATUS_CREATING`] with the clock owing it
    /// [`SNAPSHOT_STATUS_AVAILABLE`], so a caller that reads it straight back sees the
    /// snapshot still being taken. `source_snapshot_arn` is absent because nothing was
    /// copied: that member names "the DB cluster snapshot that was copied to create this
    /// one".
    /// `storage` is the copy of the cluster's storage the caller took before it locked the
    /// store ([`capture_cluster_storage`]), so the settings the snapshot reports and the
    /// data it holds are from one instant, and `data_bytes` is the size of that copy as the
    /// same caller measured it ([`captured_data_bytes`]).
    /// `data_api_storage` is the copy of the cluster's DATA API databases the caller took
    /// beside that one ([`capture_data_api_cluster_data`]), for the same reason: they are
    /// part of the data the cluster held when the snapshot was taken.
    pub fn from_cluster(
        cluster: &DbClusterRecord,
        cluster_has_instances: bool,
        snapshot_identifier: &str,
        storage: Option<::std::sync::Arc<data_plane::sql_engine::InstanceDatabase>>,
        data_api_storage: Option<::std::sync::Arc<DataApiClusterData>>,
        data_bytes: i64,
        tags: Vec<Tag>,
    ) -> Self {
        // A snapshot is taken OF the cluster, so the span it owes runs from the instant that
        // cluster became ready ([`provisioning_to`]).
        let (pending_status, ready_at) =
            provisioning_to(SNAPSHOT_STATUS_AVAILABLE, Some(cluster.ready_at));
        Self {
            snapshot_identifier: snapshot_identifier.to_string(),
            region: cluster.region.clone(),
            db_cluster_identifier: cluster.identifier.clone(),
            db_cluster_resource_id: cluster.resource_id.clone(),
            engine: cluster.engine.clone(),
            engine_version: cluster.engine_version.clone(),
            engine_mode: Some(cluster.engine_mode.clone()),
            database_name: cluster.database_name.clone(),
            db_instance_class: cluster.db_cluster_instance_class.clone(),
            // "The allocated storage size of the DB cluster snapshot in gibibytes (GiB)":
            // the storage of the cluster the snapshot is being taken from, which is what
            // the API reports until the snapshot's own size is known ([`Self::to_wire`]).
            allocated_storage: cluster.reported_allocated_storage(),
            holds_data: cluster_has_instances,
            master_username: cluster.master_username.clone(),
            // The credential a restored cluster logs its master user in with; see the
            // field's own note.
            master_user_password: cluster.master_user_password.clone(),
            availability_zones: cluster.availability_zones.clone(),
            // "The port that the DB cluster was listening on at the time of the snapshot":
            // a fact about the cluster's DB INSTANCES, so it is the port the cluster
            // PUBLISHES — the one a client was connecting to. A cluster with none had
            // nothing listening, so the snapshot was taken on no port at all.
            port: if cluster_has_instances {
                cluster.published_port()
            } else {
                0
            },
            storage_type: cluster.storage_type.clone(),
            storage_encrypted: cluster.storage_encrypted,
            kms_key_id: cluster.kms_key_id.clone(),
            // The license the engine the snapshot was taken from runs under; see
            // [`cluster_license_model`] for where a cluster's comes from.
            license_model: cluster_license_model(&cluster.engine),
            // The cluster settings the snapshot reports members of its own for, copied here
            // with the rest so a later change to the cluster cannot rewrite what the
            // snapshot reports.
            vpc_id: cluster.vpc_id(),
            backup_retention_period: Some(cluster.backup_retention_period),
            preferred_backup_window: Some(cluster.preferred_backup_window.clone()),
            iam_database_authentication_enabled: cluster.iam_database_authentication_enabled,
            snapshot_type: SNAPSHOT_TYPE_MANUAL.to_string(),
            status: SNAPSHOT_STATUS_CREATING.to_string(),
            pending_status: Some(pending_status),
            ready_at,
            cluster_create_time: cluster.cluster_create_time,
            snapshot_create_time: now_as_wire_timestamp(),
            source_snapshot_arn: None,
            restore_attribute: BTreeSet::new(),
            storage,
            data_api_storage,
            data_bytes,
            tags,
        }
    }

    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as [`DbInstanceRecord::settle`] does.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
        }
        false
    }

    /// `arn:aws:rds:<region>:<account>:cluster-snapshot:<identifier>`.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster-snapshot:{}", self.snapshot_identifier),
        )
    }

    /// Whether this snapshot is public — its `restore` attribute authorizes every
    /// account; see [`SNAPSHOT_ATTRIBUTE_ALL`].
    pub fn is_public(&self) -> bool {
        self.restore_attribute.contains(SNAPSHOT_ATTRIBUTE_ALL)
    }

    /// `StorageEncryptionType`: "the type of encryption used to protect data at rest in the
    /// DB cluster snapshot".
    ///
    /// A snapshot's data at rest is protected exactly as the storage it was copied from is,
    /// so this is the source cluster's own reading ([`storage_encryption_type`]) over the
    /// encryption setting and the engine the snapshot copied off it: the two cannot
    /// disagree, and a snapshot of an encrypted cluster is never reported as unencrypted.
    pub fn storage_encryption_type(&self) -> &'static str {
        storage_encryption_type(self.storage_encrypted, &self.engine)
    }

    /// Project the stored record onto the modelled `DBClusterSnapshot` output shape.
    pub fn to_wire(&self) -> DBClusterSnapshot {
        DBClusterSnapshot {
            availability_zones: Some(self.availability_zones.clone()),
            db_cluster_snapshot_identifier: Some(self.snapshot_identifier.clone()),
            db_cluster_identifier: Some(self.db_cluster_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            engine_mode: self.engine_mode.clone(),
            // The storage the snapshot holds. That size is only known once the snapshot has
            // been taken: while it is still being made the API reports the allocated
            // storage of the CLUSTER it is being taken from, and a finished snapshot of a
            // cluster that had no DB instance in it holds no data at all.
            allocated_storage: if self.status == SNAPSHOT_STATUS_AVAILABLE {
                Some(if self.holds_data {
                    self.allocated_storage.unwrap_or(0)
                } else {
                    0
                })
            } else {
                self.allocated_storage
            },
            status: Some(self.status.clone()),
            port: Some(self.port),
            cluster_create_time: Some(self.cluster_create_time),
            master_username: self.master_username.clone(),
            engine_version: self.engine_version.clone(),
            license_model: self.license_model.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            // The progress of the copy this record stands for: complete once the
            // snapshot is available, and not yet begun while it is still being made.
            percent_progress: Some(if self.status == SNAPSHOT_STATUS_AVAILABLE {
                100
            } else {
                0
            }),
            storage_encrypted: Some(self.storage_encrypted),
            // How the data this snapshot holds is protected at rest — see
            // [`Self::storage_encryption_type`]. Reported beside `StorageEncrypted`, which
            // says only whether a KMS key of the caller's was named: a client can tell
            // none/sse-rds/sse-kms apart only from this member.
            storage_encryption_type: Some(self.storage_encryption_type().to_string()),
            kms_key_id: self.kms_key_id.clone(),
            db_cluster_snapshot_arn: Some(self.arn()),
            source_db_cluster_snapshot_arn: self.source_snapshot_arn.clone(),
            tag_list: Some(self.tags.clone()),
            storage_type: self.storage_type.clone(),
            db_cluster_resource_id: Some(self.db_cluster_resource_id.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            backup_retention_period: self.backup_retention_period,
            preferred_backup_window: self.preferred_backup_window.clone(),
            iam_database_authentication_enabled: Some(self.iam_database_authentication_enabled),
            ..DBClusterSnapshot::default()
        }
    }
}

/// How many rows of one table are summed at a time while measuring a snapshot's size.
///
/// A page rather than the whole table: [`data_plane::sql_engine::InstanceDatabase::execute`]
/// refuses a result past its own caps (`MAX_RESULT_ROWS`, `MAX_RESULT_BYTES`), so a table
/// larger than one answer is summed page by page instead of being dropped from the total.
const SNAPSHOT_SIZE_PAGE_ROWS: usize = 500;

/// The most VALUES one snapshot's size is summed over, bounding the work the measurement
/// costs however much data the copy holds.
///
/// The copy is itself bounded — [`data_plane::sql_engine::MAX_DATABASES_PER_INSTANCE`]
/// databases, each under the page ceiling every database of this data plane carries — but
/// the product of those bounds is large, so the walk carries its own ceiling and says in the
/// log when it stopped there rather than reading on.
const SNAPSHOT_SIZE_MAX_VALUES: usize = 1_000_000;

/// The full size, in bytes, of the data a snapshot holds.
///
/// Measured the way `DBSnapshot$FullSnapshotSizeInBytes` defines the quantity — "the full
/// size of the snapshot, in bytes … the size of all the blocks that were written to the
/// snapshot" — so it is the size of the data the snapshot's own copy carries: every value in
/// every table of every database that copy holds ([`table_names`],
/// [`data_plane::sql_engine::Cell::byte_cost`]). A snapshot that captured no storage at all —
/// one of a cluster with no DB instance in it — and one whose databases were never written to
/// both hold no data, so both measure zero rather than nothing.
///
/// Bounded three ways, because a caller decides how much data a copy holds:
/// [`MAX_EXPORTED_TABLES`] tables — the ceiling the export of a snapshot's tables already
/// carries — [`SNAPSHOT_SIZE_MAX_VALUES`] values, and [`SNAPSHOT_SIZE_PAGE_ROWS`] rows per
/// statement. Each ceiling reached is logged, so a total that stopped short is visible
/// rather than silent.
///
/// Engine work: the caller runs it on a blocking task, with no state lock held
/// ([`data_plane::captured_data_bytes`]), and ONCE — the copy never changes after it was
/// taken, so the snapshot's record keeps the answer ([`DbClusterSnapshotRecord::data_bytes`])
/// and no later read of that snapshot walks its data again.
pub fn snapshot_data_bytes(storage: Option<&data_plane::sql_engine::InstanceDatabase>) -> i64 {
    let Some(storage) = storage else {
        return 0;
    };
    let mut bytes: i64 = 0;
    let mut values = 0usize;
    let mut tables = 0usize;
    for database in storage.database_names() {
        for table in table_names(storage, &database) {
            tables += 1;
            if tables > MAX_EXPORTED_TABLES || values >= SNAPSHOT_SIZE_MAX_VALUES {
                ::tracing::warn!(
                    tables,
                    values,
                    "the snapshot holds more data than its size is summed over, so the size \
                     reported is of the data that was summed"
                );
                return bytes;
            }
            let mut read = 0usize;
            loop {
                let page = match storage.execute(
                    &database,
                    &format!(
                        "SELECT * FROM \"{table}\" LIMIT {SNAPSHOT_SIZE_PAGE_ROWS} OFFSET {read}"
                    ),
                    &[],
                ) {
                    Ok(page) => page,
                    Err(error) => {
                        // A table the engine will not read is left out of the total, exactly
                        // as an export leaves it out of what it writes — and it is logged
                        // for the same reason.
                        ::tracing::warn!(
                            table = ?table,
                            error = %error.message,
                            "a table of the snapshot's data could not be read, so its size is \
                             not part of the total"
                        );
                        break;
                    }
                };
                let rows = page.rows.len();
                for row in &page.rows {
                    for cell in row {
                        bytes = bytes
                            .saturating_add(i64::try_from(cell.byte_cost()).unwrap_or(i64::MAX));
                    }
                    values = values.saturating_add(row.len());
                }
                read += rows;
                // The last page of a table is a short one; a copy nothing writes to while
                // this runs reads its rows in one order, so the pages do not overlap.
                if rows < SNAPSHOT_SIZE_PAGE_ROWS || values >= SNAPSHOT_SIZE_MAX_VALUES {
                    break;
                }
            }
        }
    }
    bytes
}

/// The tags a snapshot COPY carries: the ones the request named, plus the source's where
/// `CopyTags` asked for them.
///
/// `CopyTags` "specifies whether to copy all tags from the source DB snapshot to the target
/// DB snapshot" and `Tags` is the caller's own list; the two members are independent, so a
/// request that sets both gets both. A key the request names keeps the value the REQUEST
/// gave it — the copy's own tag wins over the source's for that key — so nothing a caller
/// asked for is silently replaced.
///
/// The one rule for both copies, the DB snapshot's and the DB cluster snapshot's.
pub fn copied_snapshot_tags(copy_tags: Option<bool>, requested: &[Tag], source: &[Tag]) -> Vec<Tag> {
    let mut tags = requested.to_vec();
    if copy_tags.unwrap_or(false) {
        for tag in source {
            if !tags.iter().any(|held| held.key == tag.key) {
                tags.push(tag.clone());
            }
        }
    }
    tags
}

/// The tags a RESTORE that names none of its own gives the resource it creates.
///
/// `CopyTagsToSnapshot` states the rule: "RDS checks whether you specify new tags. If yes,
/// the new tags are added to the restored DB instance. If there are no new tags, RDS looks
/// for the tags from the source DB instance for the DB snapshot, and then adds those tags to
/// the restored DB instance." `source` is that resource's tags where this region still holds
/// it; where it is gone — the final snapshot of a deleted instance or cluster — what is left
/// of them is what the snapshot itself carries.
///
/// The one rule for every restore, the DB instance's and the DB cluster's, beside
/// [`copied_snapshot_tags`] — the same question answered for a snapshot COPY.
pub fn inherited_snapshot_tags(
    source: Option<Vec<Tag>>,
    snapshot: &[Tag],
) -> Vec<Tag> {
    match source {
        Some(tags) if !tags.is_empty() => tags,
        _ => snapshot.to_vec(),
    }
}

/// The REGION a snapshot copy resolves its SOURCE in: the Region the FORM of the reference
/// names.
///
/// "If the source snapshot is in the same Amazon Web Services Region as the copy, specify a
/// valid DB snapshot identifier … If the source snapshot is in a different Amazon Web
/// Services Region than the copy, specify a valid DB snapshot ARN" — so an ARN is looked for
/// in the Region written in it, whether or not that is the Region the request was signed
/// for, and an unqualified identifier (and a region-less ARN, whose Region segment is empty)
/// means `request_region`, the Region the request was signed for.
///
/// The ONE reading for both copies, the DB snapshot's and the DB cluster snapshot's, and for
/// the refusal they word ([`snapshot_copy_source_not_found_message`]): the Region a source is
/// looked for in and the Region a refusal names are the same question, so they cannot drift
/// apart.
pub fn snapshot_copy_source_region<'a>(requested: &'a str, request_region: &'a str) -> &'a str {
    arn_region(requested)
        .filter(|region| !region.is_empty())
        .unwrap_or(request_region)
}

/// How Amazon RDS words the not-found fault for the SOURCE of a snapshot copy, looked for
/// from a request signed for `request_region`.
///
/// A copy names its source either by identifier or by ARN, and the Region it is looked for in
/// follows that form ([`snapshot_copy_source_region`]) — a copy whose source lives in another
/// region is exactly what the copy operations exist for — so the refusal names BOTH the
/// snapshot that was asked for and the region it was looked for in. `requested` is carried in
/// the request's own spelling.
///
/// The one wording for both copies, the DB snapshot's and the DB cluster snapshot's: the
/// sentence names the resource it could not find rather than the operation, through the one
/// region-qualified template this crate has (`cross_region_source_not_found_message`).
pub fn snapshot_copy_source_not_found_message(requested: &str, request_region: &str) -> String {
    let source_region = snapshot_copy_source_region(requested, request_region);
    cross_region_source_not_found_message(&format!("snapshot {requested}"), source_region)
}

/// How Amazon RDS words the DB-snapshot not-found fault: the `<type> not found: <name>`
/// form it uses for a resource a request NAMED, carrying the request's own spelling of the
/// identifier. Stated once here so every operation that resolves a DB snapshot answers the
/// one text.
pub fn db_snapshot_not_found_message(named: &str) -> String {
    format!("DBSnapshot not found: {named}")
}

/// The `DBSnapshotAttributesResult` a DB snapshot's `restore` attribute projects onto.
///
/// `restore` is the one attribute AWS defines for a DB snapshot, and it is always reported:
/// a snapshot nobody has been authorized for carries the attribute with an empty value
/// list, which is how a caller reads "shared with no account". Both
/// `DescribeDBSnapshotAttributes` and `ModifyDBSnapshotAttribute` answer with it, so
/// neither can report it differently from the other — the reading
/// [`restore_attribute_to_wire`] takes of a DB CLUSTER snapshot's attribute, onto this
/// kind's own output shape.
pub fn db_snapshot_restore_attribute_to_wire(
    record: &DbSnapshotRecord,
) -> DBSnapshotAttributesResult {
    DBSnapshotAttributesResult {
        db_snapshot_identifier: Some(record.snapshot_identifier.clone()),
        db_snapshot_attributes: Some(vec![DBSnapshotAttribute {
            attribute_name: Some(SNAPSHOT_RESTORE_ATTRIBUTE.to_string()),
            attribute_values: Some(record.restore_attribute.iter().cloned().collect()),
        }]),
    }
}

/// How Amazon RDS words the DB CLUSTER snapshot not-found fault: the same
/// `<type> not found: <name>` form [`db_snapshot_not_found_message`] words, for the cluster
/// snapshot a request NAMED, carrying the request's own spelling of the identifier.
pub fn db_cluster_snapshot_not_found_message(named: &str) -> String {
    format!("DBClusterSnapshot not found: {named}")
}

/// The sentence the `DBSnapshotAlreadyExistsFault` carries for `named`:
/// "DBSnapshotIdentifier is already used by an existing snapshot", so the snapshot cannot be
/// created under it.
///
/// Amazon RDS words it as a statement about the snapshot it will NOT create and the
/// identifier that is taken — the identifier the request carried is the only variable part,
/// and the sentence carries a terminal period.
///
/// One wording for every operation that MINTS a DB snapshot — `CreateDBSnapshot`,
/// `CopyDBSnapshot` and the final snapshot a `DeleteDBInstance`, a `StopDBInstance` or a
/// `DeleteTenantDatabase` takes — so no two of them word the same collision differently.
pub fn db_snapshot_already_exists_message(named: &str) -> String {
    format!(
        "Cannot create the snapshot because a snapshot with the identifier {named} already \
         exists."
    )
}

/// The sentence the `DBClusterSnapshotAlreadyExistsFault` carries for `named`: "the user
/// already has a DB cluster snapshot with the given identifier", worded as the service words
/// it — a statement about the snapshot that cannot be created, whose only variable part is
/// the identifier the snapshot would be stored under.
///
/// One wording for every operation that MINTS a cluster snapshot — `CreateDBClusterSnapshot`,
/// `CopyDBClusterSnapshot` and the final snapshot `DeleteDBCluster` takes — so no two of them
/// word the same collision differently.
pub fn db_cluster_snapshot_already_exists_message(named: &str) -> String {
    format!(
        "Cannot create the cluster snapshot because one with the identifier {named} already \
         exists."
    )
}

/// The sentence the `SnapshotQuotaExceededFault` carries when another manual DB cluster
/// snapshot cannot be stored: the bound this emulator writes under, stated as the per-region
/// quota and the number of regions it keeps records for
/// ([`ComAmazonawsRdsAmazonrdsv19State::cluster_snapshot_room_is_exhausted`], which is the
/// predicate this sentence reports). Shared by the same three operations as
/// [`db_cluster_snapshot_already_exists_message`].
pub fn db_cluster_snapshot_quota_message() -> String {
    format!(
        "the request would exceed the allowed number of manual DB cluster snapshots ({} per \
         region, in at most {} regions)",
        DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION, MAX_REGIONS
    )
}

/// A stored DB snapshot: the source instance's settings as they stood when the
/// snapshot was taken, which is why they are copied rather than re-read later.
#[derive(Debug, Clone)]
pub struct DbSnapshotRecord {
    pub snapshot_identifier: String,
    /// The region the snapshot lives in; the scope of its ARN.
    pub region: String,
    pub db_instance_identifier: String,
    pub dbi_resource_id: String,
    pub engine: String,
    pub engine_version: Option<String>,
    /// `DBInstanceClass` of the instance the snapshot was taken from, which is what a
    /// restore of this snapshot provisions when the request names no class of its own
    /// ("Default: The same DBInstanceClass as the original DB instance").
    pub db_instance_class: String,
    /// `DBName` of the instance the snapshot was taken from: the database the snapshot's
    /// data lives in, so a restore of it serves the same database name.
    pub db_name: Option<String>,
    pub allocated_storage: Option<i32>,
    pub master_username: Option<String>,
    /// The master user's credential, as the DIGESTS a login is verified against — never
    /// the password itself, exactly as [`DbInstanceRecord::master_user_password`] holds
    /// it. A restore keeps the source's master credential, so the snapshot has to carry
    /// it.
    pub master_user_password: Option<data_plane::wire_session::MasterPasswordHash>,
    pub availability_zone: String,
    pub port: i32,
    pub storage_type: String,
    /// The storage throughput the instance the snapshot was taken from was provisioned
    /// with, in MiBps, which a restore provisions again.
    pub storage_throughput: i32,
    /// `BackupRetentionPeriod` as the source instance carried it, which is what a restore
    /// uses when the request names none ("Default: Uses existing setting").
    pub backup_retention_period: i32,
    /// `PreferredBackupWindow` as the source instance carried it, for the same reason.
    pub preferred_backup_window: Option<String>,
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub iops: Option<i32>,
    pub license_model: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingStatus>,
    /// The instant the clock makes — or made — this snapshot READY for the first time, as
    /// [`DbInstanceRecord::ready_at`] records it for a DB instance: what a resource
    /// provisioned FROM this snapshot (a copy of it, a restore of it, an export of it)
    /// charges its own span from ([`provisioning_to`]).
    pub ready_at: Instant,
    pub snapshot_type: String,
    pub instance_create_time: ::aws_smithy_types::DateTime,
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    /// A COPY of the data the source instance's storage held when the snapshot was taken,
    /// taken by the engine itself ([`data_plane::sql_engine::InstanceDatabase::capture`]):
    /// this is what makes a snapshot restorable, because a restore loads it into the new
    /// instance's own storage. `None` for a snapshot of an instance that has no engine
    /// endpoint at all — an engine whose wire this emulator serves no data plane for.
    ///
    /// The copy is in memory and private to this snapshot, it is never written to the
    /// host, and its size is bounded by the same per-database page ceiling every database
    /// of this service's data plane carries; the number of copies is bounded by
    /// [`DB_SNAPSHOT_QUOTA_PER_REGION`] and [`MAX_REGIONS`], like every other record here.
    /// Deleting the snapshot drops it.
    pub storage: Option<::std::sync::Arc<data_plane::sql_engine::InstanceDatabase>>,
    /// The tenant databases the source instance held when the snapshot was taken, copied
    /// here like every other setting the snapshot carries — what
    /// `DescribeDBSnapshotTenantDatabases` reports and what a restore of this snapshot
    /// brings back. Empty for a snapshot of a single-tenant instance, which holds none; see
    /// [`SnapshotTenantDatabaseRecord`].
    pub tenant_databases: Vec<SnapshotTenantDatabaseRecord>,
    /// The accounts the `restore` attribute authorizes, which `ModifyDBSnapshotAttribute`
    /// adds to and removes from and `DescribeDBSnapshotAttributes` reports; see
    /// [`SNAPSHOT_RESTORE_ATTRIBUTE`]. A set, so authorizing an account twice changes
    /// nothing and the report is ordered — exactly what
    /// [`DbClusterSnapshotRecord::restore_attribute`] holds for a DB cluster snapshot.
    pub restore_attribute: BTreeSet<String>,
    pub tags: Vec<Tag>,
}

impl DbSnapshotRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as [`DbInstanceRecord::settle`] does.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
        }
        false
    }

    /// Snapshot the source instance's settings and a copy of its data. `snapshot_type` is
    /// `manual` for a `CreateDBSnapshot` and for the final snapshot `DeleteDBInstance`
    /// takes — `automated` is reserved for the backups RDS itself schedules.
    ///
    /// `storage` is the copy of the instance's storage the caller took under the same
    /// state lock ([`ComAmazonawsRdsAmazonrdsv19State::capture_instance_storage`]), so the
    /// settings the snapshot reports and the data it holds are from one instant.
    ///
    /// `tenant_databases` is the copy of the instance's tenant databases taken under that
    /// same lock ([`ComAmazonawsRdsAmazonrdsv19State::capture_tenant_databases`]), for the
    /// same reason: the snapshot holds the tenant databases the instance had when it was
    /// taken, not the ones it has later.
    pub fn from_instance(
        instance: &DbInstanceRecord,
        snapshot_identifier: &str,
        tags: Vec<Tag>,
        storage: Option<::std::sync::Arc<data_plane::sql_engine::InstanceDatabase>>,
        tenant_databases: Vec<SnapshotTenantDatabaseRecord>,
    ) -> Self {
        // A snapshot is taken OF the instance, so the span it owes runs from the instant that
        // instance became ready ([`provisioning_to`]).
        let (pending_status, ready_at) =
            provisioning_to(SNAPSHOT_STATUS_AVAILABLE, Some(instance.ready_at));
        Self {
            snapshot_identifier: snapshot_identifier.to_string(),
            region: instance.region.clone(),
            db_instance_identifier: instance.identifier.clone(),
            dbi_resource_id: instance.dbi_resource_id.clone(),
            engine: instance.engine.clone(),
            engine_version: instance.engine_version.clone(),
            db_instance_class: instance.db_instance_class.clone(),
            db_name: instance.db_name.clone(),
            allocated_storage: instance.allocated_storage,
            master_username: instance.master_username.clone(),
            master_user_password: instance.master_user_password.clone(),
            availability_zone: instance.availability_zone.clone(),
            port: instance.endpoint_port,
            storage_type: instance.storage_type.clone(),
            storage_throughput: instance.storage_throughput,
            backup_retention_period: instance.backup_retention_period,
            preferred_backup_window: instance.preferred_backup_window.clone(),
            encrypted: instance.storage_encrypted,
            kms_key_id: instance.kms_key_id.clone(),
            iops: instance.iops,
            license_model: instance.license_model.clone(),
            status: SNAPSHOT_STATUS_CREATING.to_string(),
            pending_status: Some(pending_status),
            ready_at,
            snapshot_type: SNAPSHOT_TYPE_MANUAL.to_string(),
            instance_create_time: instance.instance_create_time,
            snapshot_create_time: now_as_wire_timestamp(),
            storage,
            tenant_databases,
            // A snapshot is taken authorized for nobody: `restore` carries an account only
            // once `ModifyDBSnapshotAttribute` has put it there.
            restore_attribute: BTreeSet::new(),
            tags,
        }
    }

    /// Whether this snapshot is public — its `restore` attribute authorizes every account;
    /// see [`SNAPSHOT_ATTRIBUTE_ALL`]. The same reading
    /// [`DbClusterSnapshotRecord::is_public`] takes of a DB cluster snapshot's attribute.
    pub fn is_public(&self) -> bool {
        self.restore_attribute.contains(SNAPSHOT_ATTRIBUTE_ALL)
    }

    /// `arn:aws:rds:<region>:<account>:snapshot:<identifier>`.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("snapshot:{}", self.snapshot_identifier),
        )
    }

    /// Project the stored record onto the modelled `DBSnapshot` output shape.
    pub fn to_wire(&self) -> DBSnapshot {
        DBSnapshot {
            db_snapshot_identifier: Some(self.snapshot_identifier.clone()),
            db_instance_identifier: Some(self.db_instance_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            allocated_storage: self.allocated_storage,
            status: Some(self.status.clone()),
            port: Some(self.port),
            availability_zone: Some(self.availability_zone.clone()),
            instance_create_time: Some(self.instance_create_time),
            master_username: self.master_username.clone(),
            engine_version: self.engine_version.clone(),
            license_model: self.license_model.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            iops: self.iops,
            // 0 while the snapshot is still being taken, 100 once it is available:
            // the progress of the copy this record stands for, not a stored field.
            percent_progress: Some(if self.status == "available" { 100 } else { 0 }),
            storage_type: Some(self.storage_type.clone()),
            storage_throughput: Some(self.storage_throughput),
            // The automated-backup settings the source instance carried when the snapshot
            // was taken, which is what a restore of it uses when the request names none.
            backup_retention_period: Some(self.backup_retention_period),
            preferred_backup_window: self.preferred_backup_window.clone(),
            encrypted: Some(self.encrypted),
            kms_key_id: self.kms_key_id.clone(),
            db_snapshot_arn: Some(self.arn()),
            dbi_resource_id: Some(self.dbi_resource_id.clone()),
            tag_list: Some(self.tags.clone()),
            ..DBSnapshot::default()
        }
    }
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// Whether this store already holds a DB cluster snapshot under `key` in `region` —
    /// what every operation that MINTS one is held to, since a second snapshot under one
    /// identifier would overwrite the cluster state the first one holds.
    ///
    /// One reading for the three operations that mint a cluster snapshot
    /// (`CreateDBClusterSnapshot`, `CopyDBClusterSnapshot`, and the final snapshot
    /// `DeleteDBCluster` takes), each of which lifts it into its own modelled
    /// `DBClusterSnapshotAlreadyExistsFault`.
    pub fn holds_cluster_snapshot(&self, region: &str, key: &str) -> bool {
        self.db_cluster_snapshots
            .contains_key(&(region.to_string(), key.to_string()))
    }

    /// Whether another manual DB cluster snapshot can be stored for `region` at all: the
    /// region's own quota ([`DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION`]) and the number of
    /// regions this store keeps records for ([`MAX_REGIONS`]).
    ///
    /// The one bound the three minting operations write under, so no two of them differ
    /// about how many snapshots a region holds; each lifts it into its own modelled
    /// `SnapshotQuotaExceededFault`, worded by
    /// [`db_cluster_snapshot_quota_message`].
    pub fn cluster_snapshot_room_is_exhausted(&self, region: &str) -> bool {
        self.cluster_snapshots_in(region).len() >= DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION
            || !self.has_room_for_region(region)
    }

    /// The key of the DB snapshot `value` names IN `region`: either its identifier, as the
    /// snapshot was taken under it, or its ARN — `RestoreDBInstanceFromDBSnapshot`
    /// documents both ("If you are restoring from a shared manual DB snapshot, the
    /// `DBSnapshotIdentifier` must be the ARN of the shared DB snapshot").
    ///
    /// A DB snapshot's identifier is stored as the caller spelled it — `DBSnapshotIdentifier`
    /// documents no case folding, unlike the instance and group names — so the identifier
    /// form matches exactly, the way `DescribeDBSnapshots` and `DeleteDBSnapshot` read it.
    pub fn resolve_db_snapshot_key(&self, region: &str, value: &str) -> Option<String> {
        if self
            .db_snapshots
            .contains_key(&(region.to_string(), value.to_string()))
        {
            return Some(value.to_string());
        }
        // Only an ARN can match the scan below, so a plain identifier that named nothing
        // does not pay for rendering a region's stored ARNs.
        if !value.starts_with("arn:") {
            return None;
        }
        self.snapshots_in(region)
            .into_iter()
            .find(|record| record.arn() == value)
            .map(|record| record.snapshot_identifier.clone())
    }
}
