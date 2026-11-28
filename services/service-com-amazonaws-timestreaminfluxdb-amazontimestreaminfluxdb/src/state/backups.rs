// SPDX-License-Identifier: CC0-1.0

//! The DB-backup slice of this service's state.
//!
//! A backup is a POINT-IN-TIME SNAPSHOT of a DB instance or a DB cluster: every member a
//! backup reports about "the resource at the time of backup" is copied off that resource's
//! stored record when the backup is taken, so a later change to the resource — or its
//! deletion — leaves the backup reporting what it captured. This module owns the persisted
//! backup record plus the helpers a backup handler shares: minting the service-generated
//! `id`, formatting the `arn`, taking the snapshot off either source resource, computing
//! the expiry date a retention implies, settling the clock-owed status transition, and
//! projecting a stored record into each operation's own output shape.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ClusterConfiguration, DeploymentConfiguration, LogDeliveryConfiguration, MaintenanceSchedule,
};

use super::influx_data_plane::TimeSeriesSnapshot;
use super::{
    engine_type, family_arn, service_managed_db_parameter_group,
    ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState, DbCluster, DbInstance,
    NamedResource, ARN_SEGMENT_DB_BACKUP, RESOURCE_TYPE_DB_BACKUP, RESOURCE_TYPE_DB_CLUSTER,
    RESOURCE_TYPE_DB_INSTANCE, TRANSITION_SPAN,
};

/// `DbBackupStatus` values this service's backup records use, each one a member of
/// `crate::ops::types::DB_BACKUP_STATUS_VALUES` (the modelled `DbBackupStatus` enum).
pub const DB_BACKUP_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`DB_BACKUP_STATUS_IN_PROGRESS`].
pub const DB_BACKUP_STATUS_COMPLETED: &str = "COMPLETED";

/// The `type` a backup this API creates carries: `CreateDbBackup` is documented "Creates a
/// new on-demand backup of a Timestream for InfluxDB resource", and `ON_DEMAND` is the
/// member of the modelled `DbBackupType` enum
/// (`crate::ops::types::DB_BACKUP_TYPE_VALUES`) that names exactly that. The other members
/// are the schedules an automated backup configuration runs on, which no call here takes.
pub const DB_BACKUP_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// See [`super::NamedResource`]: the DB backup family. A backup's name is unique within the
/// account and Region (see `CreateDbBackup`'s `name`), so a second create under a live
/// name is refused with the same already-exists sentence every other family uses.
pub const NAMED_DB_BACKUP: NamedResource = NamedResource {
    resource_type: RESOURCE_TYPE_DB_BACKUP,
    spelling: "DbBackup",
};

/// The `resourceType` a refusal about the resource a backup is taken FROM reports.
///
/// `dbResourceId` names "the id of the DB instance or DB cluster to back up", so an id
/// that resolves in NEITHER family identifies neither of them: the refusal reports the
/// kind of resource the member itself addresses — the one the model names `DbResourceId`
/// — rather than picking one of the two families arbitrarily. Like
/// [`super::RESOURCE_TYPE_DB_PARAMETER_GROUP`] and [`RESOURCE_TYPE_DB_BACKUP`], the
/// member is modelled as a plain `String` rather than as the two-member `ResourceType`
/// enum, so this type is reportable.
pub const RESOURCE_TYPE_DB_RESOURCE: &str = "DB_RESOURCE";

/// How many DB backups the store holds before `CreateDbBackup` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// Like [`super::DB_INSTANCE_QUOTA`], this is the emulator's OWN bound on the state rather
/// than a claim about any account's adjustable Service Quotas limit: it is what stops the
/// store (and the backup name index beside it) growing without end, and it sits far above
/// any workflow an emulator serves.
pub const DB_BACKUP_QUOTA: usize = 1_000;

/// Seconds in a day, for turning a retention in days into the instant a backup expires at.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// How many characters the `YYYY-MM-DD` form of the modelled `Date` carries.
const DATE_TEXT_LEN: usize = 10;

/// The DB resource a backup was taken from, as it stood at that instant: the
/// configuration members every backup output shape reports "at the time of backup", and
/// the resource's own InfluxDB data.
///
/// It is a snapshot, not a reference: the values are copied off the source record when the
/// backup is created, so the backup keeps reporting them however the source changes later.
/// Restoring the backup builds the restored resource out of exactly these values.
#[derive(Debug, Clone)]
pub struct DbBackupSnapshot {
    /// Which family the resource the backup was taken from belongs to, a member of
    /// `crate::ops::types::RESOURCE_TYPE_VALUES` (the modelled `ResourceType` enum): what
    /// a restore of this backup reports as the restored resource's type when the request
    /// asks for no deployment type of its own.
    pub resource_type: String,
    /// The engine the source resource runs, a member of the modelled `EngineType` enum.
    pub engine_type: String,
    /// The source's deployment type, a member of the modelled `ResourceDeploymentType`
    /// enum — which covers both the DB instance deployment types and the cluster one.
    pub deployment_type: Option<String>,
    pub db_instance_type: String,
    pub db_storage_type: Option<String>,
    pub allocated_storage: i32,
    /// The id of the DB parameter group in force on the source resource.
    pub db_parameter_group_id: Option<String>,
    /// The node composition of the source, reported only for a DB cluster: a DB instance
    /// has no node set, so `ClusterConfiguration` describes nothing about one.
    pub cluster_configuration: Option<ClusterConfiguration>,
    /// The compute topology of the source as `deploymentConfiguration` reports it,
    /// captured under the same rule as the deprecated view beside it: only a DB cluster
    /// has a node set, so only a cluster's backup carries one (see
    /// [`DbCluster::deployment_configuration`]).
    pub deployment_configuration: Option<DeploymentConfiguration>,
    /// The source's failover mode, reported only for a DB cluster: `FailoverMode` is a
    /// member of the cluster shapes only, so a DB instance has none to capture.
    pub failover_mode: Option<String>,
    pub log_delivery_configuration: Option<LogDeliveryConfiguration>,
    pub maintenance_schedule: Option<MaintenanceSchedule>,
    pub vpc_subnet_ids: Vec<String>,
    pub vpc_security_group_ids: Vec<String>,
    pub publicly_accessible: Option<bool>,
    pub port: i32,
    pub network_type: Option<String>,
    pub influx_auth_parameters_secret_arn: String,
    pub kms_key_id: Option<String>,
    /// The source's own InfluxDB data as it stood at that instant: the admin user its
    /// plane authenticated and the points its buckets held, for a source this emulator
    /// serves an InfluxDB API for. `None` for a DB cluster, whose nodes are reached
    /// through the cluster's endpoints and publish no socket of their own, so there is no
    /// plane to capture.
    pub data: Option<TimeSeriesSnapshot>,
}

impl DbBackupSnapshot {
    /// The snapshot of a DB instance, with the engine resolved by the caller (it follows
    /// from the DB parameter group applied to the instance, which only the store can
    /// resolve — see
    /// [`ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState::db_instance_engine_type`])
    /// and `data` the copy of the instance's own InfluxDB store, which only the store can
    /// reach either.
    pub fn of_db_instance(
        instance: &DbInstance,
        engine_type: &str,
        data: Option<TimeSeriesSnapshot>,
    ) -> Self {
        Self {
            resource_type: RESOURCE_TYPE_DB_INSTANCE.to_string(),
            engine_type: engine_type.to_string(),
            deployment_type: instance.deployment_type.clone(),
            db_instance_type: instance.db_instance_type.clone(),
            db_storage_type: instance.db_storage_type.clone(),
            allocated_storage: instance.allocated_storage,
            db_parameter_group_id: instance.db_parameter_group_identifier.clone(),
            cluster_configuration: None,
            deployment_configuration: None,
            failover_mode: None,
            log_delivery_configuration: instance.log_delivery_configuration.clone(),
            maintenance_schedule: instance.maintenance_schedule.clone(),
            vpc_subnet_ids: instance.vpc_subnet_ids.clone(),
            vpc_security_group_ids: instance.vpc_security_group_ids.clone(),
            publicly_accessible: instance.publicly_accessible,
            // The port the backup reports is the one every READ of the source reports: the
            // port a client dials it on, which for a DB instance this emulator serves an
            // InfluxDB API for is that socket's ([`DbInstance::reported_port`]).
            port: instance.reported_port(),
            network_type: instance.network_type.clone(),
            influx_auth_parameters_secret_arn: instance.influx_auth_parameters_secret_arn.clone(),
            kms_key_id: instance.kms_key_id.clone(),
            data,
        }
    }

    /// The snapshot of a DB cluster. A cluster stores the engine it runs, so nothing has
    /// to be resolved for it; the group captured is the one actually APPLIED to it.
    pub fn of_db_cluster(cluster: &DbCluster) -> Self {
        Self {
            resource_type: RESOURCE_TYPE_DB_CLUSTER.to_string(),
            engine_type: cluster.engine_type.clone(),
            deployment_type: Some(cluster.deployment_type.clone()),
            db_instance_type: cluster.db_instance_type.clone(),
            db_storage_type: cluster.db_storage_type.clone(),
            allocated_storage: cluster.allocated_storage,
            db_parameter_group_id: cluster.effective_db_parameter_group_identifier.clone(),
            cluster_configuration: Some(cluster.cluster_configuration()),
            deployment_configuration: cluster.deployment_configuration(),
            failover_mode: Some(cluster.failover_mode.clone()),
            log_delivery_configuration: cluster.log_delivery_configuration.clone(),
            maintenance_schedule: cluster.maintenance_schedule.clone(),
            vpc_subnet_ids: cluster.vpc_subnet_ids.clone(),
            vpc_security_group_ids: cluster.vpc_security_group_ids.clone(),
            publicly_accessible: cluster.publicly_accessible,
            port: cluster.port,
            network_type: cluster.network_type.clone(),
            influx_auth_parameters_secret_arn: cluster.influx_auth_parameters_secret_arn.clone(),
            kms_key_id: cluster.kms_key_id.clone(),
            // A cluster's nodes publish no InfluxDB socket, so a cluster has no plane to
            // capture the data of.
            data: None,
        }
    }
}

/// One persisted Timestream for InfluxDB DB backup: the single source of truth every
/// DB-backup operation reads and writes.
#[derive(Debug, Clone)]
pub struct DbBackup {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The Region the backup was taken in; its ARN is scoped to it, and the name
    /// uniqueness rule is per Region, so later operations read the Region from here.
    pub region: String,
    pub status: String,
    /// The id of the DB instance or DB cluster this backup was taken from.
    pub db_resource_id: String,
    /// The kind of backup, a member of the modelled `DbBackupType` enum.
    pub backup_type: String,
    /// The date after which the backup is automatically deleted, in the `YYYY-MM-DD` form
    /// the modelled `Date` gives it, or `None` when the create asked for no retention (see
    /// [`backup_expiry_date`]).
    pub expires_after: Option<String>,
    /// The source resource's configuration as it stood when the backup was taken.
    pub snapshot: DbBackupSnapshot,
    /// The tags the create request attached. No backup output shape carries them, so
    /// nothing echoes them back; they are kept for the tagging operations, which reach
    /// them through this record's own `arn`.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// When the clock owes this record its settle to `COMPLETED`; `None` once settled.
    pub settles_at: Option<SystemTime>,
}

impl DbBackup {
    /// Materialise the status change the clock already made due.
    ///
    /// A reporting operation calls this before answering: the transition was owed at
    /// `settles_at` whether or not anybody called, so applying it on read is not a
    /// read-time mutation of the resource's own lifecycle.
    pub fn settle(&mut self, now: SystemTime) {
        if self.settles_at.is_some_and(|due| now >= due) {
            self.status = DB_BACKUP_STATUS_COMPLETED.to_string();
            self.settles_at = None;
        }
    }

    /// Project this stored record into the `DbBackupSummary` the list operation reports.
    ///
    /// Every member of that shape is a member of this record; the summary is the subset
    /// of [`Self::to_get_output`], so one backup never reports two different values for
    /// the same member.
    pub fn to_summary(&self) -> crate::ops::types::DbBackupSummary {
        crate::ops::types::DbBackupSummary {
            id: self.id.clone(),
            name: Some(self.name.clone()),
            arn: self.arn.clone(),
            status: Some(self.status.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            expires_after: self.expires_after.clone(),
            db_resource_id: Some(self.db_resource_id.clone()),
            r#type: Some(self.backup_type.clone()),
            engine_type: Some(self.snapshot.engine_type.clone()),
            deployment_type: self.snapshot.deployment_type.clone(),
            kms_key_id: self.snapshot.kms_key_id.clone(),
        }
    }
}

/// The DB-backup operations whose output shape is the same full record.
///
/// They are distinct generated types with identical members, so one macro writes one
/// projection per shape instead of copies that could drift apart.
macro_rules! db_backup_record_projections {
    ($($method:ident -> $out:path),* $(,)?) => {
        impl DbBackup {
            $(
                /// Project this stored record into the operation's output shape. Every
                /// member comes from the record; nothing is recomputed from the request.
                pub fn $method(&self) -> $out {
                    $out {
                        id: self.id.clone(),
                        name: Some(self.name.clone()),
                        arn: self.arn.clone(),
                        status: Some(self.status.clone()),
                        created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
                        expires_after: self.expires_after.clone(),
                        db_resource_id: Some(self.db_resource_id.clone()),
                        r#type: Some(self.backup_type.clone()),
                        engine_type: Some(self.snapshot.engine_type.clone()),
                        deployment_type: self.snapshot.deployment_type.clone(),
                        kms_key_id: self.snapshot.kms_key_id.clone(),
                        cluster_configuration: self.snapshot.cluster_configuration.clone(),
                        deployment_configuration: self
                            .snapshot
                            .deployment_configuration
                            .clone(),
                        db_parameter_group_id: self.snapshot.db_parameter_group_id.clone(),
                        db_instance_type: Some(self.snapshot.db_instance_type.clone()),
                        log_delivery_configuration: self
                            .snapshot
                            .log_delivery_configuration
                            .clone(),
                        failover_mode: self.snapshot.failover_mode.clone(),
                        db_storage_type: self.snapshot.db_storage_type.clone(),
                        allocated_storage: Some(self.snapshot.allocated_storage),
                        vpc_subnet_ids: Some(self.snapshot.vpc_subnet_ids.clone()),
                        vpc_security_group_ids: Some(
                            self.snapshot.vpc_security_group_ids.clone(),
                        ),
                        publicly_accessible: self.snapshot.publicly_accessible,
                        port: Some(self.snapshot.port),
                        network_type: self.snapshot.network_type.clone(),
                        influx_auth_parameters_secret_arn: Some(
                            self.snapshot.influx_auth_parameters_secret_arn.clone(),
                        ),
                        maintenance_schedule: self.snapshot.maintenance_schedule.clone(),
                    }
                }
            )*
        }
    };
}

db_backup_record_projections! {
    to_create_output -> crate::ops::types::CreateDbBackupOutput,
    to_get_output -> crate::ops::types::GetDbBackupOutput,
}

/// The ARN of a DB backup:
/// `arn:aws:timestream-influxdb:<region>:<account>:db-backup/<id>`, one of the four
/// resource segments the modelled `Arn` pattern allows.
pub fn db_backup_arn(region: &str, id: &str) -> String {
    family_arn(ARN_SEGMENT_DB_BACKUP, region, id)
}

/// The refusal every call aimed at one DB backup gets when the identifier it names
/// resolves to no backup this account and Region holds — the one spelling of that
/// sentence, so a read of a backup and a restore from one refuse the same miss alike.
///
/// Its only variable part is the identifier the request asked for, which the `resourceId`
/// the model marks required on `ResourceNotFoundException` carries as well.
#[must_use]
pub fn db_backup_not_found(identifier: &str) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: format!("DB backup {identifier} not found"),
        resource_id: identifier.to_string(),
        resource_type: RESOURCE_TYPE_DB_BACKUP.to_string(),
    }
}

/// The date a retention of `retention_days` days puts a backup taken at `created_at`
/// beyond, in the `YYYY-MM-DD` form the modelled `Date` pattern gives it — the
/// `expiresAfter` the backup reports ("The date after which the backup will be
/// automatically deleted").
///
/// `None` when the create named no retention: `retentionDays` is optional and the model
/// gives it no default, so nothing has set a date past which such a backup is deleted.
/// `None` as well for an instant the calendar cannot render, which keeps the computation
/// total rather than panicking on a clock far outside the representable range.
pub fn backup_expiry_date(created_at: SystemTime, retention_days: Option<i32>) -> Option<String> {
    let days = u64::try_from(retention_days?).ok()?;
    let retained_for = Duration::from_secs(days.checked_mul(SECONDS_PER_DAY)?);
    let expires_at = created_at.checked_add(retained_for)?;
    let since_epoch = expires_at.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let seconds = i64::try_from(since_epoch.as_secs()).ok()?;
    let rendered = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    // `Format::DateTime` renders `YYYY-MM-DDThh:mm:ssZ`, so the date is its first ten
    // characters — and a rendering shorter than that is not one.
    rendered.get(..DATE_TEXT_LEN).map(str::to_string)
}

impl ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// Mint the next service-generated DB backup id: 10 lowercase hex characters, which
    /// satisfies the modelled `DbBackupId` pattern `^[a-zA-Z0-9]+$` and length 3..64.
    ///
    /// Opaque to the caller but deterministic for a given (name, region, sequence), and
    /// unique because the monotonic sequence advances until the id is unused.
    pub fn mint_db_backup_id(&mut self, name: &str, region: &str) -> String {
        loop {
            self.db_backup_seq += 1;
            let seq = self.db_backup_seq;
            let digest = <sha2::Sha256 as sha2::Digest>::digest(
                format!("db-backup|{region}|{name}|{seq}").as_bytes(),
            );
            let id = hex::encode(digest)[..10].to_string();
            if !self.db_backups.contains_key(&id) {
                return id;
            }
        }
    }

    /// The id of the DB backup holding `name` in `region`, if any.
    pub fn db_backup_id_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.db_backup_names
            .get(&(region.to_string(), name.to_string()))
    }

    /// Claim `name` in `region` for `id`.
    pub fn claim_db_backup_name(&mut self, region: &str, name: &str, id: String) {
        self.db_backup_names
            .insert((region.to_string(), name.to_string()), id);
    }

    /// The engine a DB instance runs: the one configured by the parameters of the DB
    /// parameter group applied to it, whether that group is one this account created or
    /// one of the service-managed catalog's.
    ///
    /// An instance created without a group runs the default engine, exactly as
    /// [`super::engine_type`] reports for a cluster whose create named none.
    pub fn db_instance_engine_type(&self, instance: &DbInstance) -> &'static str {
        let parameters = match instance.db_parameter_group_identifier.as_deref() {
            None => None,
            Some(identifier) => match service_managed_db_parameter_group(identifier) {
                Some(group) => Some(&group.parameters),
                None => self
                    .db_parameter_groups
                    .get(identifier)
                    .map(|group| &group.parameters),
            },
        };
        engine_type(parameters)
    }

    /// Take a backup of the DB instance or DB cluster `db_resource_id` names, or `None`
    /// when this account and Region holds neither.
    ///
    /// `dbResourceId` is documented as "the id of the DB instance or DB cluster to back
    /// up", so both families are resolved, and the snapshot is whichever record answers.
    pub fn db_resource_snapshot(&self, db_resource_id: &str) -> Option<DbBackupSnapshot> {
        if let Some(instance) = self.db_instances.get(db_resource_id) {
            let engine = self.db_instance_engine_type(instance);
            // A backup of a DB instance captures the instance's own InfluxDB data as well
            // as its configuration, so restoring it brings the points back: that is what
            // backing a resource up MEANS, and the restore request carries no password, so
            // the admin user the captured plane authenticated comes with them.
            //
            // The copy is bounded by the store it is taken from: one DB instance's plane
            // holds at most
            // [`super::influx_data_plane::time_series::MAX_POINTS_PER_BUCKET`] points per
            // bucket and mints no bucket after its create, so a snapshot can never be
            // larger than the instance it captures, and the number of snapshots the state
            // holds is capped by [`DB_BACKUP_QUOTA`].
            let data = self
                .db_instance_planes
                .get(db_resource_id)
                .and_then(|plane| plane.series().lock().ok().map(|series| series.snapshot()));
            return Some(DbBackupSnapshot::of_db_instance(instance, engine, data));
        }
        self.db_clusters
            .get(db_resource_id)
            .map(DbBackupSnapshot::of_db_cluster)
    }

    /// Store a new backup of `db_resource_id`, taken at `now` in `region` under `name`,
    /// retained for `retention_days` and carrying `tags`; hands back the stored record.
    ///
    /// The backup starts `IN_PROGRESS` — taking one is work the service does, not an
    /// instant event — and the clock settles it to `COMPLETED` one transition span later.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_db_backup(
        &mut self,
        region: &str,
        name: &str,
        db_resource_id: &str,
        snapshot: DbBackupSnapshot,
        retention_days: Option<i32>,
        tags: BTreeMap<String, String>,
        now: SystemTime,
    ) -> DbBackup {
        let id = self.mint_db_backup_id(name, region);
        let record = DbBackup {
            arn: db_backup_arn(region, &id),
            region: region.to_string(),
            status: DB_BACKUP_STATUS_IN_PROGRESS.to_string(),
            settles_at: Some(now + TRANSITION_SPAN),
            created_at: now,
            name: name.to_string(),
            db_resource_id: db_resource_id.to_string(),
            backup_type: DB_BACKUP_TYPE_ON_DEMAND.to_string(),
            expires_after: backup_expiry_date(now, retention_days),
            snapshot,
            tags,
            id: id.clone(),
        };
        self.claim_db_backup_name(region, name, id.clone());
        self.db_backups.insert(id, record.clone());
        record
    }
}
