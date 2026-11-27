// SPDX-License-Identifier: CC0-1.0

use super::*;

/// What a master user password is reported AS wherever Amazon RDS reports that one is
/// pending: the member is `@sensitive` (`PendingModifiedValues$MasterUserPassword`), so
/// the credential itself is never published and the value stands in for it.
pub const MASKED_PENDING_MASTER_USER_PASSWORD: &str = "****";

/// The Availability Zone the standby of a Multi-AZ DB instance in `region` is placed in,
/// given the zone its primary runs in; `None` for an instance that is not a Multi-AZ
/// deployment, which has no standby at all.
///
/// Source: `DBInstance$SecondaryAvailabilityZone` — "the name of the secondary Availability
/// Zone for a DB instance with multi-AZ support" — reported for a Multi-AZ deployment,
/// whose standby is in a DIFFERENT zone of the same Region from its primary. WHICH of the
/// Region's other zones holds it is the service's own choice; this emulator takes the first
/// zone of the Region that is not the primary's ([`region_availability_zones`]), as it takes
/// the Region's first zone for a primary the request placed nowhere.
pub fn standby_availability_zone(region: &str, primary: &str, multi_az: bool) -> Option<String> {
    if !multi_az {
        return None;
    }
    region_availability_zones(region)
        .into_iter()
        .find(|zone| zone != primary)
}

/// Whether the attachment of the option group `named` is OWED to a DB instance that is
/// being provisioned with it, and from when — which is what the membership's status reports
/// ([`DbInstanceRecord::option_group_apply_status`],
/// [`DbInstanceRecord::option_group_applies_at`]).
///
/// A group the ACCOUNT created holds the options it was given, and RDS applies those options
/// to the instance as part of provisioning it: the attachment is outstanding when the call
/// that provisions the instance answers, and owed from that same instant. One of the groups
/// RDS provides ITSELF holds no option at all ([`is_default_option_group_name`]), so there
/// is nothing to apply and its membership is in effect from the start; so is the membership
/// of an instance that names no option group.
///
/// One reader for the rule, so every call that provisions a DB instance holding an option
/// group — the create, the restores and the read replica — reports the same membership.
pub fn option_group_attachment_owed(named: Option<&str>) -> Option<Instant> {
    named
        .filter(|name| !is_default_option_group_name(name))
        .map(|_| Instant::now())
}

/// Whether the value `setting` holds for the parameter `name` of the DB parameter group
/// `group` needs this instance's engine to come up again before it takes effect: one the
/// caller asked to apply `pending-reboot`, or one whose published apply type is static
/// ([`change_is_pending_for`] is the documented rule, and the family's own catalog is where
/// the apply type comes from).
///
/// One reader for that test, so the two halves of
/// [`DbInstanceRecord::parameter_apply_status`] — the group a modification moved the instance
/// to, and the group it is already in — ask the same question of a value.
fn setting_waits_for_a_reboot(
    group: &DbParameterGroupRecord,
    name: &str,
    setting: &ParameterSetting,
) -> bool {
    let spec = published_db_parameter(&group.family, name)
        .map(|spec| published_spec_for_family(&group.family, spec));
    change_is_pending_for(spec.as_ref(), &setting.apply_method)
}

/// Whether a DB cluster running `engine` MANAGES its members' storage, port, master user
/// and initial database, so that a member of it reports that cluster's values and none of
/// its own.
///
/// `CreateDBInstance` states the rule member by member: `AllocatedStorage` and `StorageType`
/// "doesn't apply to Amazon Aurora DB instances" because "storage is managed by the DB
/// cluster", `Port` "doesn't apply to Aurora DB instances. The port number is managed by the
/// cluster", `MasterUsername` "doesn't apply to Amazon Aurora DB instances. The name for the
/// master user is managed by the DB cluster", and `DBName` on such a create names "the
/// database to create when the primary DB instance of the … DB cluster is created", which is
/// the cluster's database.
///
/// So it holds for every AURORA-STYLE cluster — the Aurora engines, and the engines of the
/// two sibling control planes this front door serves, whose clusters are Aurora-style too —
/// and not for a Multi-AZ DB cluster, which is created with "the amount of storage in
/// gibibytes (GiB) to allocate to each DB instance in the Multi-AZ DB cluster" and whose
/// members therefore have storage of their own ([`is_multi_az_db_cluster`]).
pub fn cluster_manages_member_attributes(engine: &str) -> bool {
    !is_multi_az_db_cluster(engine)
}

/// The members an instance reports from its CLUSTER and its ENGINE rather than from its own
/// record.
///
/// Two kinds of instance report them. A member of a cluster that MANAGES those settings
/// ([`cluster_manages_member_attributes`]), whose create could carry none of them, so
/// whatever its record holds for one of them is the generic default of a member nothing
/// set. And an instance of an engine whose own control plane models none of them at all
/// ([`instance_attributes_are_cluster_managed`]), which additionally takes its licence model
/// and its public-access flag from what that engine publishes. Resolved once for one
/// projection, so every operation that reports such an instance reports the same values.
///
/// `cluster` is `None` for a member whose cluster this store no longer holds, which leaves
/// the members read OFF that record — its storage size, its storage type, its master user,
/// its database name and its encryption type — unreported rather than invented.
struct ClusterManagedAttributes {
    /// `AllocatedStorage`: the size the CLUSTER reports, since the instance's data sits on
    /// the cluster's storage ([`DbClusterRecord::reported_allocated_storage`]).
    allocated_storage: Option<i32>,
    /// `StorageType`: the type the cluster manages that storage on, which for a cluster
    /// created without one is the default its own `StorageType` publishes
    /// ([`default_db_cluster_storage_type`]).
    storage_type: Option<String>,
    /// `StorageThroughput`: throughput is provisioned with INSTANCE storage, and a
    /// cluster's storage throughput is "not configurable", so none is provisioned here —
    /// reported as zero, as it is for an instance on any storage type that provisions none
    /// ([`default_storage_throughput`]).
    storage_throughput: i32,
    /// `MasterUsername`: the master user of the cluster, the only one a member has.
    master_username: Option<String>,
    /// `DBName`: the database the CLUSTER was created with, the only one a member has.
    db_name: Option<String>,
    /// `StorageEncryptionType`: how the data at rest on the cluster's volume is protected,
    /// which is the CLUSTER's reading of it ([`DbClusterRecord::storage_encryption_type`]).
    storage_encryption_type: Option<&'static str>,
    /// `LicenseModel`: the one value the engine's catalogue publishes, for an engine whose
    /// control plane models no `LicenseModel` at all ([`cluster_managed_license_model`]);
    /// `None` leaves the instance reporting its own.
    license_model: Option<Option<&'static str>>,
    /// `PubliclyAccessible`: `com.amazonaws.docdb#DBInstance$PubliclyAccessible` states
    /// that the value "is always `false`" for that engine, which supports no public
    /// endpoint at all; `None` leaves the instance reporting its own flag, which the
    /// Aurora engines' own requests do carry.
    publicly_accessible: Option<bool>,
}

impl ClusterManagedAttributes {
    /// Resolve them for an instance of `engine` that belongs to `cluster`.
    fn of(engine: &str, cluster: Option<&DbClusterRecord>) -> Self {
        // The two members an engine whose own control plane models neither of them fixes,
        // which are the instance's own on every other engine.
        let engine_fixes_them = instance_attributes_are_cluster_managed(engine);
        Self {
            allocated_storage: cluster.and_then(DbClusterRecord::reported_allocated_storage),
            storage_type: cluster.and_then(|cluster| {
                cluster.storage_type.clone().or_else(|| {
                    default_db_cluster_storage_type(&cluster.engine).map(str::to_string)
                })
            }),
            storage_throughput: 0,
            master_username: cluster.and_then(|cluster| cluster.master_username.clone()),
            db_name: cluster.and_then(|cluster| cluster.database_name.clone()),
            storage_encryption_type: cluster.map(DbClusterRecord::storage_encryption_type),
            license_model: engine_fixes_them.then(|| cluster_managed_license_model(engine)),
            publicly_accessible: engine_fixes_them.then_some(false),
        }
    }
}

/// A stored DB instance: everything the service persists about it.
///
/// The ARN, the endpoint DNS name and the hosted zone id are NOT stored as text:
/// they are functions of the stored region, identifier and minted endpoint host, and
/// [`DbInstanceRecord::to_wire`] computes them. That is what keeps them consistent
/// after `ModifyDBInstance` renames an instance — a stored ARN would still name the
/// old identifier.
#[derive(Debug, Clone)]
pub struct DbInstanceRecord {
    pub identifier: String,
    /// The region the instance was created in; the scope of its ARN and endpoint.
    pub region: String,
    pub dbi_resource_id: String,
    /// The server-assigned middle label of the endpoint DNS name, minted once.
    pub endpoint_host: String,
    pub db_instance_class: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingStatus>,
    /// The instant the clock makes — or made — this instance READY for the first time: the
    /// due instant of the provisioning transition it was stored with, or the instant it was
    /// stored for a record that was ready at once. What a resource provisioned FROM this
    /// instance (a snapshot of it, a replica of it, a restore of it) charges its own span
    /// from ([`provisioning_to`]).
    pub ready_at: Instant,
    pub db_name: Option<String>,
    pub allocated_storage: Option<i32>,
    pub max_allocated_storage: Option<i32>,
    pub master_username: Option<String>,
    /// The master user's password, as the DIGESTS a login on the instance's engine
    /// endpoint is verified against — never the password itself, which is `@sensitive`,
    /// is reported by no operation and is not stored (see
    /// [`data_plane::wire_session::MasterPasswordHash`]). `None` for an instance created
    /// without one, which no engine session can then authenticate against.
    pub master_user_password: Option<data_plane::wire_session::MasterPasswordHash>,
    /// `MasterUserSecret`: the secret RDS manages for this instance's master user
    /// password, for an instance created or modified with `ManageMasterUserPassword`
    /// turned on; `None` for one whose password the caller supplied, which has no managed
    /// secret to report.
    ///
    /// Only an operation that CREATES the secret for this instance sets it, so what is
    /// named here is the instance's OWN secret: an instance that inherits another's master
    /// credential — a read replica, an instance restored from a snapshot — manages none
    /// and reports none.
    pub master_user_secret: Option<ManagedMasterSecret>,
    /// The port the instance's engine accepts connections on as the request configured it:
    /// what `Port` (and `DBPortNumber`) set, else the documented default of its engine
    /// family. Reported by `DbInstancePort` for a cluster member
    /// ([`DbInstanceRecord::db_instance_port`]).
    pub endpoint_port: i32,
    /// Where the instance's own engine listener accepts connections IN THIS PROCESS, for
    /// the engines whose wire this emulator speaks: the address the modelled `Endpoint`
    /// publishes, because that is the address and port a client can actually reach the
    /// engine on. `None` for an engine this emulator serves no data plane for, which keeps
    /// the DNS-form endpoint and the configured port.
    pub served_endpoint: Option<ServedEndpoint>,
    pub multi_az: bool,
    /// Whether the instance owes a Multi-AZ FAILOVER: a reboot was conducted "through a
    /// Multi-AZ failover" (`RebootDBInstanceMessage$ForceFailover`), so the standby is
    /// promoted as the instance comes back up and the two zones trade places
    /// ([`Self::promote_standby`]). `false` for an instance with no failover owed, which is
    /// every instance between reboots.
    pub pending_failover: bool,
    pub storage_type: String,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub iops: Option<i32>,
    pub license_model: Option<String>,
    pub availability_zone: String,
    /// `SecondaryAvailabilityZone` — "If present, specifies the name of the secondary
    /// Availability Zone for a DB instance with multi-AZ support": the zone this instance's
    /// STANDBY sits in.
    ///
    /// It is another zone of [`Self::region`] than [`Self::availability_zone`] — a Multi-AZ
    /// deployment keeps its standby in a different zone from its primary, which is what
    /// makes it one — and `None` exactly when [`Self::multi_az`] is false, since a Single-AZ
    /// deployment has no standby to name. [`Self::place_multi_az_standby`] is what keeps the
    /// two in step.
    pub secondary_availability_zone: Option<String>,
    pub backup_retention_period: i32,
    pub preferred_backup_window: Option<String>,
    pub preferred_maintenance_window: Option<String>,
    pub publicly_accessible: bool,
    pub auto_minor_version_upgrade: bool,
    pub copy_tags_to_snapshot: bool,
    pub deletion_protection: bool,
    pub db_subnet_group_name: Option<String>,
    /// The DB parameter group applied to the instance: the one the request named, or
    /// the default group of the engine version it was created with.
    pub db_parameter_group_name: Option<String>,
    /// When the group a modification SWITCHED this instance to finishes being attached, for
    /// an instance that has been moved to another group and not rebooted since.
    ///
    /// `ModifyDBInstance$DBParameterGroupName` is the rule: "the parameter group name itself
    /// is changed immediately, but the actual parameter changes are not applied until you
    /// reboot the instance without failover" — so the switch is a change the group's
    /// membership reports until the engine comes up again
    /// ([`Self::parameter_apply_status`]), and the attach itself takes the span every
    /// transition this service owes takes. `None` for an instance whose group nothing has
    /// switched since its engine last came up, which has no such change to apply.
    pub db_parameter_group_attached_at: Option<Instant>,
    /// The option group applied to the instance: the one the request named, or the
    /// default group of its engine and major engine version. It is a REFERENCE, which
    /// is what keeps a group an instance uses from being deleted under it
    /// (`DeleteOptionGroup`).
    pub option_group_name: Option<String>,
    /// When the option group [`Self::option_group_name`] names finishes being APPLIED to
    /// this instance, for an instance whose attachment this service still owes.
    ///
    /// RDS applies the options of a group the ACCOUNT created to the instance as part of
    /// PROVISIONING it, so the attachment is outstanding at the instant the call that
    /// provisions the instance is accepted — which is what that call's own answer reports,
    /// `pending-apply` ([`Self::option_group_apply_status`]) — and owed from that same
    /// instant, so the next call materialises it
    /// ([`super::ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`]) and every later
    /// read reports the group applied. `None` for an instance whose group is applied, for
    /// one placed in a group RDS provides itself, which holds no option to apply, and for
    /// one in no option group at all ([`option_group_attachment_owed`]).
    pub option_group_applies_at: Option<Instant>,
    pub vpc_security_group_ids: Vec<String>,
    /// The legacy EC2-Classic DB security groups the request named, which is normally
    /// none: "This setting applies to the legacy EC2-Classic platform, which is no
    /// longer used to create new DB instances."
    pub db_security_group_names: Vec<String>,
    pub db_cluster_identifier: Option<String>,
    /// `PromotionTier`: "the order of priority in which an Aurora Replica is promoted to
    /// the primary instance after a failure of the existing primary instance" — what the
    /// request set, else the documented default [`DEFAULT_PROMOTION_TIER`].
    ///
    /// Stored for every instance, because every request that creates or modifies one can
    /// name it, and reported for the instances it orders: the members of a DB cluster
    /// ([`Self::promotion_tier`]).
    pub promotion_tier: i32,
    /// `ReadReplicaSourceDBInstanceIdentifier` — "the identifier of the source DB instance
    /// if this DB instance is a read replica", as `CreateDBInstanceReadReplica` resolved it,
    /// and `None` for an instance that replicates nothing.
    pub read_replica_source_db_instance_identifier: Option<String>,
    /// `ReadReplicaSourceDBClusterIdentifier` — "the identifier of the source DB cluster if
    /// this DB instance is a read replica", for the OTHER source
    /// `CreateDBInstanceReadReplica` creates a replica from: a Multi-AZ DB cluster
    /// (`SourceDBClusterIdentifier`). `None` for an instance that replicates no cluster.
    ///
    /// At most one of the two source members is ever set, because a replica replicates ONE
    /// source. The cluster's half of the relationship is NOT stored on the cluster: it is
    /// read off the instances that name it, as everything a cluster reports about the
    /// records around it is ([`ClusterMembership`]).
    pub read_replica_source_db_cluster_identifier: Option<String>,
    /// `ReadReplicaDBInstanceIdentifiers` — "the identifiers of the read replicas associated
    /// with this DB instance", in the order the replicas were created.
    ///
    /// The SOURCE's side of the same relationship, recorded on the source when a replica is
    /// created and dropped from it when that replica is deleted
    /// ([`Self::detach_read_replica`]), so the list never names an instance this store no
    /// longer holds.
    pub read_replica_db_instance_identifiers: Vec<String>,
    /// The OPEN MODE a read replica of an engine that has one was asked to run in:
    /// `ReplicaMode`, as `CreateDBInstanceReadReplica` or `ModifyDBInstance` named it, and
    /// `None` for a replica that named none — which runs in the documented default
    /// ([`DEFAULT_REPLICA_MODE`]), the mode [`Self::replica_mode`] then reports.
    ///
    /// Bounded by the model rather than by a check of this crate's: the member is an ENUM
    /// (`com.amazonaws.rds#ReplicaMode`), so a request carrying anything but one of its two
    /// values is refused before a handler runs and the stored text is one of those two.
    pub replica_mode: Option<String>,
    pub domain: Option<String>,
    /// The CA the instance's server certificate is issued under.
    pub ca_certificate_identifier: String,
    pub monitoring_interval: i32,
    pub performance_insights_enabled: bool,
    /// `PerformanceInsightsKMSKeyId`: the key the instance's Performance Insights data is
    /// encrypted under, for an instance whose request named one.
    pub performance_insights_kms_key_id: Option<String>,
    /// `PerformanceInsightsRetentionPeriod`: "the number of days to retain Performance
    /// Insights data", as the request configured it — the documented default
    /// ([`DEFAULT_PERFORMANCE_INSIGHTS_RETENTION_PERIOD`]) for an instance that has
    /// Performance Insights on and named no period, and `None` for one that collects no
    /// Performance Insights data at all, which retains none.
    pub performance_insights_retention_period: Option<i32>,
    pub iam_database_authentication_enabled: bool,
    pub database_insights_mode: String,
    pub dedicated_log_volume: bool,
    pub customer_owned_ip_enabled: bool,
    pub network_type: String,
    pub backup_target: String,
    pub engine_lifecycle_support: String,
    /// The storage throughput provisioned for the instance, in MiBps.
    pub storage_throughput: i32,
    pub instance_create_time: ::aws_smithy_types::DateTime,
    /// When this instance's database engine last came UP: the create that provisioned it,
    /// or the most recent restart of the engine (`RebootDBInstance`, `StartDBInstance`).
    ///
    /// Separate from [`Self::instance_create_time`], which is when the INSTANCE was
    /// created and never moves. This is the engine's run, which is what the data plane's
    /// `pg_postmaster_start_time()` reports and what decides whether a parameter change
    /// that waits for a reboot is in force yet
    /// ([`data_plane::pg_server::running_parameters`]).
    pub engine_started_at: ::aws_smithy_types::DateTime,
    /// The DB capacity reservation this instance uses, held as the reservation's own minted
    /// identifier.
    ///
    /// `StartDBInstance$DBCapacityReservationIdentifier` is "the identifier or the name of
    /// the DB capacity reservation for the DB instance to use after it starts", resolved
    /// against the reservations this region holds before it is stored here, so the
    /// association can only name a reservation some create really stored. `None` for an
    /// instance that uses no reserved capacity — which is what a `StopDBInstance` that
    /// detaches it leaves behind (`DetachFromDBCapacityReservation`).
    pub db_capacity_reservation_identifier: Option<String>,
    pub tags: Vec<Tag>,
    /// `AssociatedRoles`: the IAM roles associated with this instance, in the order they
    /// were associated — what `AddRoleToDBInstance` adds to and
    /// `RemoveRoleFromDBInstance` removes from, bounded by
    /// [`associated_roles::MAX_ASSOCIATED_ROLES`] entries.
    pub associated_roles: Vec<DBInstanceRole>,
    /// What a modification the caller did not ask to apply immediately staged for the next
    /// maintenance window: the settings the instance is NOT running under yet, which is
    /// what `PendingModifiedValues` reports ([`DeferredModification`]). `None` for an
    /// instance with nothing staged.
    pub deferred_modification: Option<DeferredModification>,
    /// `EnabledCloudwatchLogsExports`: "a list of log types that this DB instance is
    /// configured to export to CloudWatch Logs" — the types the instance was created with
    /// (`EnableCloudwatchLogsExports`) plus every type a modification enabled and minus every
    /// type one disabled (`CloudwatchLogsExportConfiguration`), in the order they were
    /// enabled. EMPTY for an instance exporting none, which exports nothing at all
    /// ([`data_plane::log_export`]).
    pub enabled_cloudwatch_logs_exports: Vec<String>,
    /// The instance's SERVER log: the lifecycle lines this service has recorded for it and
    /// the lines its engine wrote while serving sessions, which is what `DescribeDBLogFiles`
    /// lists and `DownloadDBLogFilePortion` serves. See [`engine_log`] for the bounds on it.
    pub engine_log: InstanceLog,
    /// The MySQL family's GENERAL QUERY LOG, a log file of its own: the statements the engine
    /// ran while `general_log` was on ([`data_plane::statement_log`]). EMPTY for an instance
    /// whose engine has written none, which has no such file at all.
    pub general_query_log: InstanceLog,
    /// The audit policy state of this instance's database activity stream, for an instance
    /// a `ModifyActivityStream` has set one on — what `ActivityStreamPolicyStatus` reports
    /// ([`ActivityStreamPolicy`]). `None` for an instance whose policy nothing has modified,
    /// which reports no policy status at all.
    pub activity_stream_policy: Option<ActivityStreamPolicy>,
    /// This instance's DATABASE ACTIVITY STREAM, for an instance a `StartActivityStream`
    /// started one on — what the `DBInstance$ActivityStream*` members report
    /// ([`ActivityStream`]). `None` for an instance whose stream nothing started, and for one
    /// whose stop this service's clock has made due, each of which reports the `stopped`
    /// status and no stream configuration at all.
    pub activity_stream: Option<ActivityStream>,
    /// The recovery points this instance's CONTINUOUS BACKUP holds, oldest first: the data it
    /// held at each instant a session committed a change, which is what
    /// `RestoreDBInstanceToPointInTime` restores an instant's data from
    /// ([`data_plane::continuous_backup`]). EMPTY for an instance with automated backups off,
    /// which keeps none, and for one nothing has written to yet.
    pub recovery_points: Vec<data_plane::continuous_backup::RecoveryPoint>,
}

impl DbInstanceRecord {
    /// The lifecycle change this instance is in the MIDDLE of — the published event it is and
    /// the instant the clock makes it happen — or `None` for an instance owing no such
    /// change.
    ///
    /// Each row is a transient status the instance is on its way OUT of, and each of their
    /// events names the state the instance REACHES: the engine is restarted when the reboot's
    /// momentary outage is over, the instance is stopped when it reports `stopped`, and it is
    /// started when it is back in service. So the change happens at the due instant — which
    /// is after the call that ordered it was answered — and is recorded with that instant,
    /// never with the instant the call was accepted.
    ///
    /// The transient statuses NOT here complete no such change: an instance reaching
    /// `available` out of `creating` or `modifying` was already created or modified by the
    /// call that stored it, which recorded that change itself.
    pub(super) fn lifecycle_event_due_at(&self) -> Option<(events::LifecycleEvent, Instant)> {
        let pending = self.pending_status.as_ref()?;
        if !matches!(pending.change, PendingChange::Status(_)) {
            return None;
        }
        let completing = match self.status.as_str() {
            DB_INSTANCE_STATUS_REBOOTING => events::DB_INSTANCE_RESTARTED,
            DB_INSTANCE_STATUS_STOPPING => events::DB_INSTANCE_STOPPED,
            DB_INSTANCE_STATUS_STARTING => events::DB_INSTANCE_STARTED,
            _ => return None,
        };
        Some((completing, pending.due_at))
    }

    /// The lifecycle change a settle at `now` COMPLETES, out of the one
    /// [`Self::lifecycle_event_due_at`] names: `None` while that change is still to come.
    ///
    /// Read BEFORE [`Self::settle`], which is what performs it.
    pub(super) fn completing_lifecycle_event(
        &self,
        now: Instant,
    ) -> Option<(events::LifecycleEvent, Instant)> {
        self.lifecycle_event_due_at()
            .filter(|(_, due_at)| now >= *due_at)
    }

    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone — an accepted delete that has run its course — so the caller drops it from
    /// the store.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        self.settle_deferred_modification();
        // The option group the instance is provisioned with is attached as part of that
        // provisioning, owed from the instant the call that started it was accepted, so it
        // is applied here — on the first call after that one
        // ([`Self::option_group_applies_at`]).
        if self
            .option_group_applies_at
            .is_some_and(|applies_at| now >= applies_at)
        {
            self.option_group_applies_at = None;
        }
        // A change of the activity stream's audit policy state settles on its own clock,
        // whatever the instance's status is doing: the policy state is not the instance's
        // status ([`ActivityStreamPolicy`]).
        if let Some(policy) = self.activity_stream_policy.as_mut() {
            policy.settle(now);
        }
        // And so does the start or stop of the stream itself: a stop that has run its course
        // leaves no stream on the instance ([`ActivityStream`]).
        if self
            .activity_stream
            .as_mut()
            .is_some_and(|stream| stream.settle(now, self.preferred_maintenance_window.as_deref()))
        {
            self.activity_stream = None;
        }
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => {
                // The instance's own log records the lifecycle events this service
                // performs on it, and a status the clock just made due is one of them.
                self.engine_log
                    .record(format!("DB instance status is now {status}"));
                // "Rebooting a DB instance results in a momentary outage, during which the
                // DB instance status is set to rebooting" and "restarts the database engine
                // service": the engine's new run begins when that outage is OVER, not when
                // the reboot was accepted, so a parameter change the reboot applies is
                // still waiting for it while the instance is out
                // ([`Self::parameter_apply_status`]). A STARTING instance is the same instant
                // read the other way round — a stopped instance's engine was not running, and
                // the run it starts begins when the instance is back in service — so both
                // transitions bring the engine up here and nowhere else.
                if self.status == DB_INSTANCE_STATUS_REBOOTING
                    || self.status == DB_INSTANCE_STATUS_STARTING
                {
                    self.restart_engine();
                }
                self.status = status;
                // A reboot "conducted through a Multi-AZ failover" promotes the standby, and
                // the promotion is part of coming back up: the zones have traded places by
                // the time the instance reports itself available again, never while it is
                // still out.
                if self.pending_failover {
                    self.pending_failover = false;
                    self.promote_standby();
                }
            }
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
        }
        false
    }

    /// Apply a staged modification the clock has made due: the instant `ApplyImmediately`
    /// asked for it "as soon as possible", or the maintenance window a deferred one waits
    /// for — "changes to the DB instance are applied during the next maintenance window" —
    /// after which the instance runs under the staged settings and has nothing pending.
    ///
    /// Nothing is applied before then: a modification staged for a window that has not come
    /// round stays pending however often the instance is read.
    fn settle_deferred_modification(&mut self) {
        let due = self
            .deferred_modification
            .as_ref()
            .is_some_and(|staged| {
                staged.is_due(self.preferred_maintenance_window.as_deref(), Instant::now())
            });
        if !due {
            return;
        }
        if let Some(staged) = self.deferred_modification.take() {
            self.engine_log.record("applying pending modified values");
            staged.apply(self);
        }
    }

    /// Apply whatever is staged NOW, whether or not its window has come round: what
    /// `ApplyImmediately` asks for ("the modifications in this request and any pending
    /// modifications are asynchronously applied as soon as possible"), and what the reboot
    /// a deferred change waits for does ("Some parameter changes ... are applied on the
    /// next call to RebootDBInstance").
    ///
    /// Returns whether anything was applied, so a caller can report the instance
    /// `modifying` for a change this call really made.
    pub fn apply_deferred_modification(&mut self) -> bool {
        match self.deferred_modification.take() {
            Some(staged) => {
                staged.apply(self);
                true
            }
            None => false,
        }
    }

    /// Drop every value a modification staged that the instance is ALREADY running under,
    /// and the whole staged record when nothing is left of it.
    ///
    /// `PendingModifiedValues` reports "changes that will be applied", so a staged value the
    /// instance already carries belongs in it no more than a setting nobody asked about:
    /// applying it would change nothing. Naming the live value is therefore how a caller
    /// WITHDRAWS a change an earlier modification queued for that setting, and an instance
    /// left with nothing staged keeps no record at all
    /// ([`DeferredModification::withdraw_values_already_live`]).
    pub fn withdraw_pending_values_already_live(&mut self) {
        let Some(mut staged) = self.deferred_modification.take() else {
            return;
        };
        staged.withdraw_values_already_live(self);
        if !staged.is_empty() {
            self.deferred_modification = Some(staged);
        }
    }

    /// The instance's database engine comes UP: the run every parameter value the engine is
    /// serving is read against, which is what `pg_postmaster_start_time()` reports.
    ///
    /// A parameter change that was waiting for a reboot is in force from this instant, and so
    /// is the group a modification switched the instance to — "the actual parameter changes
    /// are not applied until you reboot the instance without failover"
    /// ([`Self::parameter_apply_status`]), and this is that reboot having happened.
    ///
    /// Called where an instance comes back INTO SERVICE, which is the instant its engine is
    /// running again: the settle that ends a reboot's momentary outage and the one that ends a
    /// start ([`Self::settle`]), never when either call was merely accepted.
    pub fn restart_engine(&mut self) {
        self.engine_started_at = now_as_wire_timestamp();
        self.db_parameter_group_attached_at = None;
    }

    /// Record that a modification MOVED this instance to another DB parameter group.
    ///
    /// `ModifyDBInstance$DBParameterGroupName`: "The parameter group name itself is changed
    /// immediately, but the actual parameter changes are not applied until you reboot the
    /// instance without failover." So the attach is owed the span every transition this
    /// service owes takes, and the association then waits for the reboot that applies it,
    /// whatever the new group holds ([`Self::parameter_apply_status`] reports both halves).
    pub fn switch_db_parameter_group(&mut self, name: String) {
        if self.db_parameter_group_name.as_deref() == Some(name.as_str()) {
            // The group the instance is already in: nothing is attached and nothing waits
            // for a reboot, because nothing changed.
            return;
        }
        self.db_parameter_group_name = Some(name);
        self.db_parameter_group_attached_at = Some(Instant::now() + STATUS_TRANSITION_DELAY);
    }

    /// Place this instance's Multi-AZ standby, or take it away: a Multi-AZ deployment has a
    /// standby in another Availability Zone of its own Region and names that zone, and a
    /// Single-AZ deployment has no standby and names none.
    ///
    /// Call it wherever `MultiAZ` is set — a create, a restore, a replica, a modification —
    /// so the setting and the zone pair cannot drift apart. A standby already placed in a
    /// zone of this Region other than the primary's STAYS where it is, so a failover that
    /// moved the pair is not undone by the next request that sets `MultiAZ` again.
    pub fn place_multi_az_standby(&mut self) {
        let already_placed = self
            .secondary_availability_zone
            .as_deref()
            .is_some_and(|zone| {
                zone != self.availability_zone
                    && region_availability_zones(&self.region)
                        .iter()
                        .any(|known| known == zone)
            });
        if self.multi_az && already_placed {
            return;
        }
        self.secondary_availability_zone =
            standby_availability_zone(&self.region, &self.availability_zone, self.multi_az);
    }

    /// Turn Multi-AZ on or off, placing or removing the standby with it: the deployment a
    /// `MultiAZ` setting asks for is the one with a standby, so the two are set together.
    pub fn set_multi_az(&mut self, multi_az: bool) {
        self.multi_az = multi_az;
        self.place_multi_az_standby();
    }

    /// Promote the standby to primary, which is what a Multi-AZ failover DOES: the instance
    /// runs in the zone its standby was in, and its standby is in the zone the former
    /// primary was in.
    ///
    /// Returns whether anything was promoted — `false` for an instance with no standby,
    /// whose placement is left alone.
    pub fn promote_standby(&mut self) -> bool {
        let Some(standby) = self.secondary_availability_zone.take() else {
            return false;
        };
        self.secondary_availability_zone =
            Some(::std::mem::replace(&mut self.availability_zone, standby));
        true
    }

    /// Whether a delete has already been accepted for this instance, so it is on its
    /// way out and nothing further can be done to it.
    pub fn is_deleting(&self) -> bool {
        self.status == DB_INSTANCE_STATUS_DELETING
    }

    /// Accept a delete: the instance reports `deleting` from now on, and the clock
    /// owes its removal from the store [`STATUS_TRANSITION_DELAY`] from now, after
    /// which every read of the identifier is a `DBInstanceNotFoundFault`. Nothing
    /// sleeps: the due instant is stored and materialised by
    /// [`ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`].
    pub fn begin_deletion(&mut self) {
        self.status = DB_INSTANCE_STATUS_DELETING.to_string();
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
    }

    /// Record that `identifier` replicates this instance: the source's half of the
    /// relationship `CreateDBInstanceReadReplica` creates.
    ///
    /// A replica is recorded once, so a create that is refused after this point cannot leave
    /// the identifier named twice.
    pub fn attach_read_replica(&mut self, identifier: &str) {
        if !self
            .read_replica_db_instance_identifiers
            .iter()
            .any(|held| held == identifier)
        {
            self.read_replica_db_instance_identifiers
                .push(identifier.to_string());
        }
    }

    /// Drop `identifier` from the replicas of this instance, for a replica that is being
    /// deleted: the source reports the replicas it HAS, so a deleted one is not one of them.
    pub fn detach_read_replica(&mut self, identifier: &str) {
        self.read_replica_db_instance_identifiers
            .retain(|held| held != identifier);
    }

    /// Stop replicating: an instance whose source DB instance is being deleted is promoted
    /// to a standalone DB instance, so it reports no source from then on.
    ///
    /// Both sources are cleared, because a promotion ends the replication whichever kind of
    /// resource it was from — a DB instance or a Multi-AZ DB cluster.
    pub fn promote_from_read_replica(&mut self) {
        self.read_replica_source_db_instance_identifier = None;
        self.read_replica_source_db_cluster_identifier = None;
        // The open mode goes with the replication: an instance that replicates nothing is not
        // open in a REPLICA mode at all, so a later role change does not inherit the mode this
        // instance ran in while it was a standby ([`Self::replica_mode`]).
        self.replica_mode = None;
    }

    /// Whether this instance REPLICATES another resource, which is what makes it a read
    /// replica: the source DB instance or the source Multi-AZ DB cluster
    /// `CreateDBInstanceReadReplica` resolved for it, and nothing once a promotion has
    /// cleared the relationship.
    pub fn is_read_replica(&self) -> bool {
        self.read_replica_source_db_instance_identifier.is_some()
            || self.read_replica_source_db_cluster_identifier.is_some()
    }

    /// The name of the log file a DB instance running `engine` writes, which follows from its
    /// engine; see [`engine_log::log_file_name`].
    pub fn log_file_name(&self) -> &'static str {
        engine_log::log_file_name(&self.engine)
    }

    /// The log FILES this instance has: its engine's server log, which every instance has,
    /// and the MySQL general query log where that engine has written one.
    ///
    /// A file the engine has written nothing to is not a file the instance has, which is why
    /// the general query log appears only once a statement has been logged to it
    /// ([`data_plane::statement_log`]): `DescribeDBLogFiles` lists "the DB log files for the
    /// DB instance", and `DownloadDBLogFilePortion` serves exactly those.
    pub fn log_files(&self) -> Vec<(&'static str, &InstanceLog)> {
        let mut files: Vec<(&'static str, &InstanceLog)> =
            vec![(self.log_file_name(), &self.engine_log)];
        if !self.general_query_log.entries.is_empty() {
            files.push((engine_log::MYSQL_GENERAL_LOG_FILE, &self.general_query_log));
        }
        files
    }

    /// `DBParameterGroupStatus$ParameterApplyStatus` for the group this instance names:
    /// whether the values that group holds are the ones its engine is RUNNING.
    ///
    /// `ModifyDBInstance$DBParameterGroupName` is the first rule: when a modification moves
    /// the instance to another group, "the parameter group name itself is changed immediately,
    /// but the actual parameter changes are not applied until you reboot the instance without
    /// failover. In this case, the DB instance isn't rebooted automatically, and the parameter
    /// changes aren't applied during the next maintenance window" — so while RDS is attaching
    /// that group the membership is `applying`, and once attached it is `pending-reboot` for
    /// as long as the association is one this instance's engine has not come up under, WHATEVER
    /// the newly associated group carries: the reboot is what puts the group in force, so
    /// nothing short of one synchronizes the change ([`Self::switch_db_parameter_group`],
    /// [`Self::restart_engine`]). The "however, if you modify dynamic parameters in the newly
    /// associated DB parameter group, these changes are applied immediately without a reboot"
    /// that follows is about a later change to a VALUE of that group, which is the second rule
    /// below — it does not apply the association itself.
    ///
    /// `ModifyDBParameterGroup` is the second rule, for the group an instance is already in:
    /// "changes to dynamic parameters are applied immediately" while "changes to static
    /// parameters require a reboot ... before the change can take effect", so a group holding
    /// a change this instance's engine has not come up again since — one that needs a reboot
    /// ([`setting_waits_for_a_reboot`]), set after the engine last started — is
    /// `pending-reboot` too. Every other membership is `in-sync`.
    ///
    /// The second half is decided by the SAME comparison the engine data plane makes when it
    /// picks the value it is running ([`data_plane::pg_server::running_parameters`]), so the
    /// status a caller reads and the value a session reads cannot disagree.
    pub fn parameter_apply_status(
        &self,
        db_parameter_group: Option<&DbParameterGroupRecord>,
    ) -> &'static str {
        if let Some(attached_at) = self.db_parameter_group_attached_at {
            return if Instant::now() < attached_at {
                PARAMETER_APPLY_STATUS_APPLYING
            } else {
                PARAMETER_APPLY_STATUS_PENDING_REBOOT
            };
        }
        let pending = db_parameter_group.is_some_and(|group| {
            group.parameters.iter().any(|(name, setting)| {
                setting_waits_for_a_reboot(group, name, setting)
                    && setting.set_at.as_secs_f64() > self.engine_started_at.as_secs_f64()
            })
        });
        if pending {
            PARAMETER_APPLY_STATUS_PENDING_REBOOT
        } else {
            MEMBERSHIP_IN_SYNC
        }
    }

    /// `OptionGroupMembership$Status`: "the status of the DB instance's option group
    /// membership. Valid values are: `in-sync`, `pending-apply`, …".
    ///
    /// The membership reports the group as still waiting to be applied while this service
    /// owes its attachment — RDS applies the options of a group the account created as part
    /// of provisioning the instance, so the call that provisions it answers with nothing
    /// applied yet ([`Self::option_group_applies_at`]) — and `in-sync` once it is attached,
    /// which every read after that provisioning call reports.
    pub fn option_group_apply_status(&self) -> &'static str {
        match self.option_group_applies_at {
            Some(_) => OPTION_GROUP_STATUS_PENDING_APPLY,
            None => MEMBERSHIP_IN_SYNC,
        }
    }

    /// `CACertificateIdentifier`: "the identifier of the CA certificate for this DB
    /// instance" — the CA the instance's server certificate is issued under, for an instance
    /// that HAS one.
    ///
    /// An instance whose delete has been accepted no longer has one: its certificate goes
    /// with the instance that was issued it, so the member comes back EMPTY from the moment
    /// the delete is accepted ([`Self::is_deleting`]) rather than naming a CA that certifies
    /// nothing. `CertificateDetails` still describes the certificate the instance ran under,
    /// which is a fact about that certificate rather than about the association this member
    /// reports.
    pub fn reported_ca_certificate_identifier(&self) -> &str {
        if self.is_deleting() {
            ""
        } else {
            &self.ca_certificate_identifier
        }
    }

    /// The log file `named`, or `None` when this instance has no such file.
    pub fn log_file(&self, named: &str) -> Option<&InstanceLog> {
        self.log_files()
            .into_iter()
            .find(|(name, _)| *name == named)
            .map(|(_, log)| log)
    }

    /// `ActivityStreamStatus` for this instance's database activity stream: the status the
    /// stream stands in, and the `stopped` member for an instance that holds none
    /// ([`Self::activity_stream`]).
    ///
    /// One reader for the status, so the instance's own report
    /// (`DBInstance$ActivityStreamStatus`) and what a call on the stream answers cannot
    /// drift apart.
    pub fn activity_stream_status(&self) -> &str {
        self.activity_stream
            .as_ref()
            .map_or(ACTIVITY_STREAM_STOPPED, ActivityStream::status)
    }

    /// The instance's activity stream as every report of it reads it, with the Kinesis stream
    /// name derived from the instance's immutable `DbiResourceId`; `None` for an instance that
    /// holds no stream.
    pub fn activity_stream_delivery(&self) -> Option<ActivityStreamDelivery> {
        self.activity_stream
            .as_ref()
            .map(|stream| stream.delivery(&self.dbi_resource_id))
    }

    /// `arn:aws:rds:<region>:<account>:db:<identifier>`, from the stored region and
    /// the instance's current identifier.
    pub fn arn(&self) -> String {
        db_instance_arn(&self.region, &self.identifier)
    }

    /// The DNS name this instance's endpoint resolves to:
    /// `<identifier>.<minted host>.<region>.<domain>`, with the domain of the control plane
    /// that owns the instance's engine — see [`endpoint_domain`].
    ///
    /// An instance whose engine this emulator SERVES publishes the host its own listener
    /// is bound to instead: that is the address a client connects to, and a name this
    /// machine does not resolve would be no endpoint at all.
    pub fn endpoint_address(&self) -> String {
        if let Some(served) = &self.served_endpoint {
            return served.host.clone();
        }
        format!(
            "{}.{}.{}.{}",
            self.identifier,
            self.endpoint_host,
            self.region,
            endpoint_domain(&self.engine)
        )
    }

    /// The port the instance's database engine listens on: the port of the listener this
    /// emulator bound for the instance's OWN engine wire where it bound one
    /// ([`Self::own_wire_listener_port`]), else the port the instance is CONFIGURED on.
    ///
    /// The core owns the port range the listener is bound in and takes no port from this
    /// service (`vendor/server/src/listeners.rs`), so the configured port
    /// ([`Self::endpoint_port`], which `Port` and `DBPortNumber` set) is what the instance
    /// records and the port the listener landed on is what it publishes — an engine endpoint
    /// whose port nothing answers on would be no endpoint at all. For an instance with no
    /// listener the two are the same value.
    ///
    /// An instance of an engine whose data plane this emulator serves as an API
    /// ([`engine_data_plane_is_an_api`]) has no listener of its own: that plane is reached at
    /// this process's address, which the instance publishes through `Endpoint$Address`
    /// ([`Self::endpoint_address`]), while its port stays the instance's own — the same
    /// reading its cluster follows ([`DbClusterRecord::published_port`]).
    pub fn published_port(&self) -> i32 {
        self.own_wire_listener_port().unwrap_or(self.endpoint_port)
    }

    /// The port of the listener this emulator bound for the instance's own engine wire, or
    /// `None` for an instance it bound none for — including one whose data plane it serves as
    /// an AWS API, whose published address is this PROCESS's own
    /// ([`crate::state::api_plane_endpoint`]) and whose port therefore answers that API
    /// rather than this instance's database engine.
    fn own_wire_listener_port(&self) -> Option<i32> {
        if engine_data_plane_is_an_api(&self.engine) {
            return None;
        }
        self.served_endpoint.as_ref().map(|served| served.port)
    }

    /// `Endpoint$Port`: the port a client reaches this instance's endpoint on.
    ///
    /// The port of the listener opened for the instance's own engine wire where it has one,
    /// which is the port that wire answers on ([`Self::published_port`]). Otherwise the
    /// configured port — and for a member whose cluster MANAGES its port
    /// ([`cluster_manages_member_attributes`]) that is the CLUSTER's published port: `Port`
    /// "doesn't apply to Aurora DB instances. The port number is managed by the cluster", so
    /// such a member has no port of its own and a client reaches it where the cluster says
    /// its engine listens ([`DbClusterRecord::published_port`]).
    ///
    /// `db_cluster` is `None` for an instance that belongs to no cluster and for a member
    /// whose cluster this store no longer holds, both of which report the instance's own.
    pub fn endpoint_published_port(&self, db_cluster: Option<&DbClusterRecord>) -> i32 {
        if let Some(port) = self.own_wire_listener_port() {
            return port;
        }
        db_cluster
            .filter(|cluster| cluster_manages_member_attributes(&cluster.engine))
            .map_or(self.endpoint_port, DbClusterRecord::published_port)
    }

    /// When the server certificate issued at creation expires:
    /// [`SERVER_CERTIFICATE_VALIDITY`] after the instance was created. A function of
    /// the stored creation time rather than a stored instant, so it cannot drift from
    /// it.
    pub fn certificate_valid_till(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from_secs_f64(
            self.instance_create_time.as_secs_f64() + SERVER_CERTIFICATE_VALIDITY.as_secs_f64(),
        )
    }

    /// `StatusInfos`: "The status of a read replica. If the DB instance isn't a read
    /// replica, the value is blank."
    ///
    /// So an instance reports one entry exactly while it REPLICATES another resource — the
    /// relationship [`Self::is_read_replica`] reads, which `PromoteReadReplica` clears — and
    /// an instance that replicates nothing reports no member at all, which is the blank value
    /// the member documents. The entry's
    /// `StatusType` "is currently \"read replication\"", `Normal` "indicates whether the
    /// instance is operating normally (TRUE) or is in an error state (FALSE)" and `Status`
    /// is one of the values the member publishes for that status type
    /// ([`READ_REPLICATION_STATUS_REPLICATING`]).
    ///
    /// Nothing this service does stops or breaks the replication it set up — the replica's
    /// storage is loaded from the source's and the replica stays attached to it until a
    /// promotion or a delete ends the relationship — so a replica this store holds is
    /// replicating normally, and `Message` ("details of the error if there is an error for
    /// the instance ... if the instance isn't in an error state, this value is blank") is
    /// left out rather than carrying an error nothing reported.
    pub fn status_infos(&self) -> Option<Vec<DBInstanceStatusInfo>> {
        self.is_read_replica().then(|| {
            vec![DBInstanceStatusInfo {
                status_type: Some(READ_REPLICATION_STATUS_TYPE.to_string()),
                normal: Some(true),
                status: Some(READ_REPLICATION_STATUS_REPLICATING.to_string()),
                message: None,
            }]
        })
    }

    /// `ReplicaMode`: "the open mode of a Db2 or an Oracle read replica. The default is
    /// `open-read-only`", and only supported for those engine families
    /// ([`replica_mode_is_supported_for`]).
    ///
    /// So it is reported for an instance that IS a read replica of one of those engines —
    /// the mode the request asked for, else the documented default — and for no other
    /// instance: an instance that replicates nothing is in no replica mode at all, which is
    /// what reporting nothing says, and the mode follows the stored relationship rather than
    /// being copied onto a record a promotion or a switchover has made primary
    /// ([`Self::promote_from_read_replica`]).
    pub fn replica_mode(&self) -> Option<String> {
        if !self.is_read_replica() || !replica_mode_is_supported_for(&self.engine) {
            return None;
        }
        Some(
            self.replica_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_REPLICA_MODE.to_string()),
        )
    }

    /// `PendingModifiedValues`: what this service has accepted for the instance and not
    /// finished applying.
    ///
    /// Two things are pending. A modification the caller did not ask to apply immediately
    /// is staged for the next maintenance window and is pending until that window comes
    /// round ([`DeferredModification`]) — the structure's own documentation, "changes that
    /// will be applied during the next maintenance window". And an instance being
    /// provisioned with a master credential has that credential pending until it is up,
    /// which is what `PendingModifiedValues$MasterUserPassword` ("the master credentials
    /// for the DB instance") reports, masked because the member is `@sensitive`
    /// ([`MASKED_PENDING_MASTER_USER_PASSWORD`]).
    ///
    /// The element is reported on EVERY instance envelope, as the recorded service reports
    /// it: an instance with nothing pending carries it EMPTY — "specific changes are
    /// identified by subelements", and an element with no subelement identifies no change —
    /// rather than being left out.
    pub fn pending_modified_values(&self) -> Option<PendingModifiedValues> {
        let staged = self
            .deferred_modification
            .as_ref()
            .filter(|staged| !staged.is_empty())
            .map(DeferredModification::to_wire);
        let password_pending =
            self.status == DB_INSTANCE_STATUS_CREATING && self.master_user_password.is_some();
        match (staged, password_pending) {
            (Some(staged), false) => Some(staged),
            (Some(staged), true) => Some(PendingModifiedValues {
                master_user_password: Some(MASKED_PENDING_MASTER_USER_PASSWORD.to_string()),
                ..staged
            }),
            (None, true) => Some(PendingModifiedValues {
                master_user_password: Some(MASKED_PENDING_MASTER_USER_PASSWORD.to_string()),
                ..PendingModifiedValues::default()
            }),
            (None, false) => Some(PendingModifiedValues::default()),
        }
    }

    /// `VPCNetworkingEnabled`: whether the instance runs inside a virtual private cloud.
    ///
    /// Every DB instance RDS creates now runs in one — `DBSecurityGroups` "applies to the
    /// legacy EC2-Classic platform, which is no longer used to create new DB instances" —
    /// so the flag is set for every instance this service holds. A MEMBER of a DB cluster
    /// reports its cluster's network instead, which is the network it is placed in: a
    /// cluster restored without a VPC network interface
    /// (`RestoreDBClusterFromSnapshot$EnableVPCNetworking`) has it off, and so do its
    /// members.
    pub fn vpc_networking_enabled(&self, db_cluster: Option<&DbClusterRecord>) -> bool {
        db_cluster.is_none_or(|cluster| cluster.vpc_networking_enabled)
    }

    /// `HttpEndpointEnabled`: whether an HTTP endpoint answers for the instance.
    ///
    /// The HTTP endpoint is the RDS Data API, and `EnableHttpEndpoint` is a DB CLUSTER
    /// setting (`ModifyDBCluster$EnableHttpEndpoint`, `EnableHttpEndpoint`/
    /// `DisableHttpEndpoint`): no operation enables one for a DB instance. So a member of a
    /// cluster reports whether that cluster's endpoint answers, and an instance that belongs
    /// to no cluster has no HTTP endpoint at all.
    pub fn http_endpoint_enabled(&self, db_cluster: Option<&DbClusterRecord>) -> bool {
        db_cluster.is_some_and(|cluster| cluster.http_endpoint_enabled)
    }

    /// `StorageEncryptionType`: how the data at rest of this instance is protected, or
    /// `None` for an instance whose data at rest is not encrypted, which reports no type.
    ///
    /// The member publishes three values: "`sse-kms` - The DB instance is encrypted using a
    /// customer managed KMS key or Amazon Web Services managed KMS key", which is what an
    /// instance created with `StorageEncrypted` is, and "`sse-rds` - The DB instance is
    /// encrypted using an Amazon Web Services owned KMS key", which is what a cluster volume
    /// RDS encrypts itself is. A MEMBER of a cluster has its data on that CLUSTER's volume,
    /// so it reports the cluster's reading of the same storage
    /// ([`DbClusterRecord::storage_encryption_type`]) and the two cannot disagree.
    fn storage_encryption_type(
        &self,
        cluster_managed: Option<&ClusterManagedAttributes>,
    ) -> Option<&'static str> {
        let reported = match cluster_managed.and_then(|managed| managed.storage_encryption_type) {
            Some(cluster) => cluster,
            None if self.storage_encrypted => STORAGE_ENCRYPTION_TYPE_SSE_KMS,
            None => STORAGE_ENCRYPTION_TYPE_NONE,
        };
        (reported != STORAGE_ENCRYPTION_TYPE_NONE).then_some(reported)
    }

    /// `DbInstancePort`: "The port that the DB instance listens on. If the DB instance
    /// is part of a DB cluster, this can be a different port than the DB cluster port."
    ///
    /// The member reports the port a cluster member listens on OF ITS OWN, which is why it
    /// is separate from `Endpoint.Port`: an instance that belongs to no DB cluster has no
    /// such port and RDS reports zero for it, and so does a member whose port its CLUSTER
    /// manages — `Port` "doesn't apply to Aurora DB instances. The port number is managed by
    /// the cluster", so such a member cannot be listening on a port of its own and the port
    /// a client dials is the cluster's. Only a member whose own request could set a port
    /// reports the CONFIGURED one — the port `Port` and `DBPortNumber` set, which the record
    /// keeps — where `Endpoint.Port` reports the port the instance's engine listener was
    /// assigned ([`Self::published_port`]).
    pub fn db_instance_port(&self, db_cluster: Option<&DbClusterRecord>) -> i32 {
        match db_cluster {
            Some(cluster) if cluster_manages_member_attributes(&cluster.engine) => 0,
            Some(_) => self.endpoint_port,
            // An instance that belongs to no DB cluster has no cluster-member port at all.
            None => 0,
        }
    }

    /// `PromotionTier`: "the order of priority in which an Aurora Replica is promoted to
    /// the primary instance after a failure of the existing primary instance."
    ///
    /// The order is the one a DB CLUSTER promotes its members in, so it is reported for an
    /// instance that belongs to one and not for an instance that belongs to none — the same
    /// reading [`Self::db_instance_port`] takes of the port a cluster member listens on.
    /// A cluster member always reports one, the value the request set or the documented
    /// default ([`DEFAULT_PROMOTION_TIER`]), which is what the stored value holds.
    pub fn promotion_tier(&self) -> Option<i32> {
        self.db_cluster_identifier
            .as_ref()
            .map(|_| self.promotion_tier)
    }

    /// `LatestRestorableTime`: "the latest time to which a database in this DB instance
    /// can be restored with point-in-time restore."
    ///
    /// An instance has one exactly while it keeps continuous backups, which is what
    /// `BackupRetentionPeriod` decides: "To disable automated backups, set this value to
    /// 0", and a non-zero period is what makes point-in-time restore available at all. So
    /// `None` for an instance with automated backups off, and `None` while the instance is
    /// still being created, which has taken no backup yet — in both cases there is no
    /// instant the instance can be restored to, which is what reporting no member says.
    ///
    /// The value ADVANCES with the clock and never predates the instance, in the STEPS the
    /// published point advances in ([`RECOVERY_POINT_INTERVAL`]): a client reads this instant,
    /// works out the window it may restore in from it and then calls the restore, so the two
    /// calls have to read the same value. This emulator's continuous backup records a recovery
    /// point at every commit ([`data_plane::continuous_backup`]) and nothing changes the
    /// instance's data between two commits, so the data of every instant the published point
    /// covers is held. Computed from the clock rather than stored, so it cannot go stale
    /// between two describes.
    pub fn latest_restorable_time(&self) -> Option<::aws_smithy_types::DateTime> {
        if !self.keeps_continuous_backups() || self.status == DB_INSTANCE_STATUS_CREATING {
            return None;
        }
        let step = RECOVERY_POINT_INTERVAL.as_secs().max(1) as i64;
        let now = now_as_wire_timestamp().secs();
        let published = now - now.rem_euclid(step);
        Some(::aws_smithy_types::DateTime::from_secs(
            published.max(self.instance_create_time.secs()),
        ))
    }

    /// The window a point-in-time restore of this instance may target: "you can restore to
    /// any point in time before the time identified by the `LatestRestorableTime` property.
    /// You can restore to a point up to the number of days specified by the
    /// `BackupRetentionPeriod` property."
    ///
    /// The lower bound is the retention period measured back from the published recovery point
    /// ([`Self::latest_restorable_time`]) — the span the automated backups cover, which is
    /// what the refusal of an earlier instant attributes the bound to
    /// ([`restore_time_before_the_window_message`]). The upper bound is that published point
    /// itself: "you can restore to any point in time BEFORE the time identified by the
    /// LatestRestorableTime property", so an instant the point has not reached yet is outside
    /// the window however recent the data this service holds is
    /// ([`restore_time_after_the_window_message`]).
    ///
    /// `None` for an instance that keeps no continuous backup, which is the case
    /// `RestoreDBInstanceToPointInTime` answers `PointInTimeRestoreNotEnabled` for.
    pub fn restorable_window(
        &self,
    ) -> Option<(::aws_smithy_types::DateTime, ::aws_smithy_types::DateTime)> {
        let published = self.latest_restorable_time()?;
        let retention = f64::from(self.backup_retention_period) * 24.0 * 60.0 * 60.0;
        Some((
            ::aws_smithy_types::DateTime::from_secs_f64(published.as_secs_f64() - retention),
            published,
        ))
    }

    /// Project the stored record onto the modelled `DBInstance` output shape.
    ///
    /// Every member RDS reports for a DB instance it holds is projected here, from the
    /// stored record or from the documented default the record was created with; the
    /// nested `Endpoint`, `CertificateDetails`, `PendingModifiedValues` and membership
    /// lists are projected from the same record, so no two of them can disagree. A member
    /// whose value this service has no state for at all is left out rather than invented —
    /// a list it does have state for is reported EMPTY, which is what RDS reports for an
    /// instance that has none of those associations.
    ///
    /// `db_subnet_group` is the DB SUBNET GROUP record the instance's own
    /// `DBSubnetGroupName` resolves to, which the store looks up for this projection
    /// ([`ComAmazonawsRdsAmazonrdsv19State::instance_to_wire`], the one way a handler
    /// reaches it): a nested resource of another record is reported FROM that record, so
    /// the group an instance reports and the one `DescribeDBSubnetGroups` reports are one
    /// answer. `None` where the instance names a group this store no longer resolves, which
    /// reports no group at all rather than a name with nothing behind it.
    /// `db_parameter_group` is the DB PARAMETER GROUP record the instance's own
    /// `DBParameterGroupName` resolves to, read by the store for this projection
    /// ([`ComAmazonawsRdsAmazonrdsv19State::db_parameter_group_of`]): whether the values that
    /// group holds are the ones the engine is RUNNING is what the membership's
    /// `ParameterApplyStatus` reports ([`Self::parameter_apply_status`]), and that is a fact
    /// about the group and this instance's engine run together.
    /// `db_cluster` is the DB CLUSTER this instance is a member of, read by the store for
    /// this projection ([`ComAmazonawsRdsAmazonrdsv19State::db_cluster_of`]): an instance
    /// whose storage, port, master user and database its cluster manages
    /// ([`cluster_manages_member_attributes`]) reports THAT record's values, so the member
    /// and the cluster cannot disagree about storage the cluster manages for both.
    pub fn to_wire(
        &self,
        db_subnet_group: Option<&DbSubnetGroupRecord>,
        db_parameter_group: Option<&DbParameterGroupRecord>,
        db_cluster: Option<&DbClusterRecord>,
    ) -> DBInstance {
        // The members this instance reports from its CLUSTER rather than from its own
        // record: a member of a cluster that manages its storage, port, master user and
        // database reports that cluster's, and an instance of an engine whose own control
        // plane models none of those members takes its licence model and public-access flag
        // from what that engine publishes as well. `None` for an instance that belongs to
        // no such cluster, whose own request carried each of them.
        let cluster_managed = (db_cluster
            .is_some_and(|cluster| cluster_manages_member_attributes(&cluster.engine))
            || instance_attributes_are_cluster_managed(&self.engine))
        .then_some(db_cluster)
        .map(|cluster| ClusterManagedAttributes::of(&self.engine, cluster));
        // One reading of the instance's database activity stream, so every member it fills
        // describes the same stream.
        let activity_stream = self.activity_stream_delivery();
        DBInstance {
            db_instance_identifier: Some(self.identifier.clone()),
            db_instance_class: Some(self.db_instance_class.clone()),
            engine: Some(self.engine.clone()),
            db_instance_status: Some(self.status.clone()),
            master_username: cluster_managed.as_ref().map_or_else(
                || self.master_username.clone(),
                |managed| managed.master_username.clone(),
            ),
            // `MasterUserSecret`: reported for an instance whose master user password RDS
            // manages, and only for one — an instance whose password the caller supplied
            // has no secret of RDS's own, which is what reporting nothing says.
            master_user_secret: self
                .master_user_secret
                .as_ref()
                .map(ManagedMasterSecret::to_wire),
            // `DBName`: "the initial database name that you provided (if required) when you
            // created the DB instance" — and for a member whose database its cluster
            // manages, the database that CLUSTER was created with.
            db_name: cluster_managed.as_ref().map_or_else(
                || self.db_name.clone(),
                |managed| managed.db_name.clone(),
            ),
            // How the data at rest of this instance is protected
            // ([`Self::storage_encryption_type`]).
            storage_encryption_type: self
                .storage_encryption_type(cluster_managed.as_ref())
                .map(str::to_string),
            endpoint: Some(Endpoint {
                address: Some(self.endpoint_address()),
                // The port a client reaches this endpoint on, which for a member whose port
                // its cluster manages is the CLUSTER's
                // ([`Self::endpoint_published_port`]).
                port: Some(self.endpoint_published_port(db_cluster)),
                hosted_zone_id: Some(hosted_zone_id(
                    &self.region,
                    endpoint_domain(&self.engine),
                )),
            }),
            // The storage this instance's data sits on, in gibibytes: the size its CLUSTER
            // reports for the storage it manages where the cluster manages it — the setting
            // "doesn't apply" to such an instance, so its own request could carry no size —
            // and otherwise the size the request asked for.
            allocated_storage: cluster_managed.as_ref().map_or_else(
                || self.allocated_storage,
                |managed| managed.allocated_storage.or(self.allocated_storage),
            ),
            // Whether the instance runs in a virtual private cloud, and whether an HTTP
            // endpoint answers for it: reported for every instance, each read off the
            // network and the endpoint the instance actually has
            // ([`Self::vpc_networking_enabled`], [`Self::http_endpoint_enabled`]).
            vpc_networking_enabled: Some(self.vpc_networking_enabled(db_cluster)),
            http_endpoint_enabled: Some(self.http_endpoint_enabled(db_cluster)),
            instance_create_time: Some(self.instance_create_time),
            preferred_backup_window: self.preferred_backup_window.clone(),
            backup_retention_period: Some(self.backup_retention_period),
            latest_restorable_time: self.latest_restorable_time(),
            // The legacy EC2-Classic memberships: empty for every instance created
            // through the VPC platform, which is every instance this service holds
            // unless a request named one.
            db_security_groups: Some(
                self.db_security_group_names
                    .iter()
                    .map(|name| DBSecurityGroupMembership {
                        db_security_group_name: Some(name.clone()),
                        status: Some(MEMBERSHIP_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            vpc_security_groups: Some(vpc_security_group_memberships(
                &self.region,
                &self.vpc_security_group_ids,
            )),
            db_parameter_groups: Some(
                self.db_parameter_group_name
                    .iter()
                    .map(|name| DBParameterGroupStatus {
                        db_parameter_group_name: Some(name.clone()),
                        parameter_apply_status: Some(
                            self.parameter_apply_status(db_parameter_group).to_string(),
                        ),
                    })
                    .collect(),
            ),
            option_group_memberships: Some(
                self.option_group_name
                    .iter()
                    .map(|name| OptionGroupMembership {
                        option_group_name: Some(name.clone()),
                        // Whether the group is attached YET, which is what the membership
                        // reports ([`Self::option_group_apply_status`]).
                        status: Some(self.option_group_apply_status().to_string()),
                    })
                    .collect(),
            ),
            availability_zone: Some(self.availability_zone.clone()),
            // `SecondaryAvailabilityZone`: "If present, specifies the name of the secondary
            // Availability Zone for a DB instance with multi-AZ support" — the zone the
            // instance's standby sits in, reported from the stored placement, and reported
            // not at all for a Single-AZ deployment, which has no standby to name.
            secondary_availability_zone: self.secondary_availability_zone.clone(),
            // The DB subnet group the instance was created in, as the GROUP's own record
            // reports it: its description, its VPC, its status and its `Subnets` — each
            // subnet with the Availability Zone the service that owns subnets answered
            // with — are properties of that record, and a client reads them here to tell
            // which subnets and zones the instance can be placed in. Projected through
            // the group's one projection, so this copy cannot drift from what
            // `DescribeDBSubnetGroups` reports for the same group.
            db_subnet_group: db_subnet_group.map(DbSubnetGroupRecord::to_wire),
            preferred_maintenance_window: self.preferred_maintenance_window.clone(),
            // `PendingModifiedValues`: what this instance still has pending, reported on
            // every envelope and EMPTY for one with nothing pending — "specific changes
            // are identified by subelements", so an element with no subelement is the form
            // that identifies no change ([`Self::pending_modified_values`]).
            pending_modified_values: self.pending_modified_values(),
            upgrade_rollout_order: Some(DEFAULT_UPGRADE_ROLLOUT_ORDER.to_string()),
            multi_az: Some(self.multi_az),
            engine_version: self.engine_version.clone(),
            auto_minor_version_upgrade: Some(self.auto_minor_version_upgrade),
            // The replicas of this instance, in the order they were created, and the source
            // it replicates itself. Both are stored halves of one relationship
            // `CreateDBInstanceReadReplica` creates, so an instance with no replica reports
            // an EMPTY list rather than none, and one that replicates nothing reports no
            // source at all.
            read_replica_db_instance_identifiers: Some(
                self.read_replica_db_instance_identifiers.clone(),
            ),
            read_replica_source_db_instance_identifier: self
                .read_replica_source_db_instance_identifier
                .clone(),
            // The other source a replica may have: the Multi-AZ DB cluster it replicates.
            // Reported from the same stored relationship, so a replica of a cluster names
            // its cluster here exactly as a replica of an instance names its instance above.
            read_replica_source_db_cluster_identifier: self
                .read_replica_source_db_cluster_identifier
                .clone(),
            // `StatusInfos`: the replication status of a READ REPLICA, and blank for an
            // instance that is not one ([`Self::status_infos`]).
            status_infos: self.status_infos(),
            // `ReplicaMode`: the open mode of a read replica of an engine that has one
            // ([`Self::replica_mode`]).
            replica_mode: self.replica_mode(),
            // `LicenseModel` and `PubliclyAccessible`: the values the instance's own
            // request carried, except on an engine whose control plane models neither and
            // publishes one value for each.
            license_model: match cluster_managed
                .as_ref()
                .and_then(|managed| managed.license_model)
            {
                Some(published) => published.map(str::to_string),
                None => self.license_model.clone(),
            },
            iops: cluster_managed.as_ref().map_or(self.iops, |_| None),
            storage_throughput: Some(cluster_managed.as_ref().map_or(
                self.storage_throughput,
                |managed| managed.storage_throughput,
            )),
            publicly_accessible: Some(
                cluster_managed
                    .as_ref()
                    .and_then(|managed| managed.publicly_accessible)
                    .unwrap_or(self.publicly_accessible),
            ),
            storage_type: cluster_managed.as_ref().map_or_else(
                || Some(self.storage_type.clone()),
                |managed| managed.storage_type.clone(),
            ),
            db_instance_port: Some(self.db_instance_port(db_cluster)),
            // `PromotionTier`: the failover order of a cluster member
            // ([`Self::promotion_tier`]).
            promotion_tier: self.promotion_tier(),
            db_cluster_identifier: self.db_cluster_identifier.clone(),
            storage_encrypted: Some(self.storage_encrypted),
            kms_key_id: self.kms_key_id.clone(),
            dbi_resource_id: Some(self.dbi_resource_id.clone()),
            ca_certificate_identifier: Some(
                self.reported_ca_certificate_identifier().to_string(),
            ),
            certificate_details: Some(CertificateDetails {
                ca_identifier: Some(self.ca_certificate_identifier.clone()),
                valid_till: Some(self.certificate_valid_till()),
            }),
            domain_memberships: Some(
                self.domain
                    .iter()
                    .map(|domain| DomainMembership {
                        domain: Some(domain.clone()),
                        status: Some("joined".to_string()),
                        ..DomainMembership::default()
                    })
                    .collect(),
            ),
            copy_tags_to_snapshot: Some(self.copy_tags_to_snapshot),
            monitoring_interval: Some(self.monitoring_interval),
            db_instance_arn: Some(self.arn()),
            iam_database_authentication_enabled: Some(self.iam_database_authentication_enabled),
            database_insights_mode: Some(self.database_insights_mode.clone()),
            performance_insights_enabled: Some(self.performance_insights_enabled),
            // The Performance Insights configuration the instance runs under: the
            // retention period it keeps that data for, and the key it is encrypted under
            // where the request named one. Reported from the instance's own record, so
            // what a create or a modification set is what a later read answers.
            performance_insights_kms_key_id: self.performance_insights_kms_key_id.clone(),
            performance_insights_retention_period: self.performance_insights_retention_period,
            deletion_protection: Some(self.deletion_protection),
            // "A list of Amazon Web Services Identity and Access Management (IAM) roles
            // associated with the DB instance": the associations this instance carries, in
            // the order they were made, and EMPTY for an instance that carries none.
            associated_roles: Some(self.associated_roles.clone()),
            max_allocated_storage: self.max_allocated_storage,
            // `EnabledCloudwatchLogsExports`: "a list of log types that this DB instance is
            // configured to export to CloudWatch Logs" — the types the instance exports NOW,
            // and an EMPTY list for one that exports none, which is what RDS reports for an
            // instance with no export configured.
            enabled_cloudwatch_logs_exports: Some(self.enabled_cloudwatch_logs_exports.clone()),
            tag_list: Some(self.tags.clone()),
            customer_owned_ip_enabled: Some(self.customer_owned_ip_enabled),
            network_type: Some(self.network_type.clone()),
            activity_stream_status: Some(self.activity_stream_status().to_string()),
            // The configuration of the stream a `StartActivityStream` recorded, which only an
            // instance that HAS one running carries: its mode, the key its messages are
            // encrypted under, the Kinesis data stream they are delivered to and whether
            // engine-native audit fields travel with them.
            activity_stream_mode: activity_stream.as_ref().map(|stream| stream.mode.clone()),
            activity_stream_kms_key_id: activity_stream
                .as_ref()
                .map(|stream| stream.kms_key_id.clone()),
            activity_stream_kinesis_stream_name: activity_stream
                .as_ref()
                .map(|stream| stream.kinesis_stream_name.clone()),
            activity_stream_engine_native_audit_fields_included: activity_stream
                .as_ref()
                .and_then(|stream| stream.engine_native_audit_fields_included),
            // `ActivityStreamPolicyStatus`: the state of the audit policy of this instance's
            // activity stream, for an instance a `ModifyActivityStream` has set one on, and
            // absent for one whose policy nothing has modified.
            activity_stream_policy_status: self
                .activity_stream_policy
                .as_ref()
                .map(|policy| policy.status().to_string()),
            backup_target: Some(self.backup_target.clone()),
            dedicated_log_volume: Some(self.dedicated_log_volume),
            // Nothing this service does changes an instance's storage file system
            // configuration, so no upgrade of it is ever recommended.
            is_storage_config_upgrade_available: Some(false),
            engine_lifecycle_support: Some(self.engine_lifecycle_support.clone()),
            // The DB capacity reservation this instance uses, for one that was started into
            // a reservation; absent for an instance that uses no reserved capacity.
            db_capacity_reservation_identifier: self.db_capacity_reservation_identifier.clone(),
            ..DBInstance::default()
        }
    }
}
