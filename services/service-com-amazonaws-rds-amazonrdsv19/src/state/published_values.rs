// SPDX-License-Identifier: CC0-1.0

//! The values this service PUBLISHES on every resource it holds: the statuses, the
//! documented defaults, the fixed refusal sentences and the catalogs a handler reads them
//! from.
//!
//! Split out of [`super`] so that module keeps the store, the records and their
//! projections; every name here is re-exported from it, so a handler still reads them as
//! `crate::state::<name>`.

use super::*;

/// How long after the call that starts it a status transition this service owes
/// becomes due.
///
/// RDS takes minutes to bring a DB instance up; a generated emulator picks its own
/// span, and this one is deliberately non-zero so a caller that reads straight after
/// `CreateDBInstance` still observes the `creating` status (and the state errors that
/// go with it) exactly as it would against the real service. The due instant is
/// stored on the record and materialised by the next call — nothing sleeps.
pub const STATUS_TRANSITION_DELAY: Duration = Duration::from_millis(300);

/// The status a DB cluster's automated backup reports while the cluster it was taken from
/// is still here.
///
/// `DBClusterAutomatedBackup$Status` documents `retained` — "Automated backups for deleted
/// clusters" — for the other case, which is the one that outlives its cluster.
pub const DB_CLUSTER_AUTOMATED_BACKUP_STATUS_ACTIVE: &str = "active";

/// The status a DB instance reports once a delete has been accepted for it: "If you
/// request a final DB snapshot, the status of the Amazon RDS DB instance is deleting
/// until the DB snapshot is created" (`DeleteDBInstance`). It is a transient status —
/// the instance is removed [`STATUS_TRANSITION_DELAY`] later, and then it is not found.
pub const DB_INSTANCE_STATUS_DELETING: &str = "deleting";

/// The status a DB instance reports while it is running, and the one both an accepted
/// `StartDBInstance` and an accepted `RebootDBInstance` settle it on.
pub const DB_INSTANCE_STATUS_AVAILABLE: &str = "available";

/// The status a DB instance reports while `CreateDBInstance` is still provisioning it.
/// Transient: the clock owes [`DB_INSTANCE_STATUS_AVAILABLE`]
/// [`STATUS_TRANSITION_DELAY`] later.
pub const DB_INSTANCE_STATUS_CREATING: &str = "creating";

/// How far behind the clock a DB instance's `LatestRestorableTime` trails.
///
/// Amazon RDS documents the continuous-backup recovery point as being within five minutes
/// of the current time ("you can restore to any point in time during your backup retention
/// period … typically up to the last five minutes"), so the latest instant an instance can
/// be restored to is five minutes behind now rather than now itself. See
/// [`DbInstanceRecord::latest_restorable_time`].
pub const LATEST_RESTORABLE_LAG: Duration = Duration::from_secs(5 * 60);

/// The status an accepted `StopDBInstance` reports while the database engine shuts down.
/// Transient: the clock owes [`DB_INSTANCE_STATUS_STOPPED`] [`STATUS_TRANSITION_DELAY`]
/// later.
pub const DB_INSTANCE_STATUS_STOPPING: &str = "stopping";

/// The status of a DB instance whose stop has run its course, and the one status a
/// `StartDBInstance` is accepted from.
pub const DB_INSTANCE_STATUS_STOPPED: &str = "stopped";

/// The status an accepted `StartDBInstance` reports while the database engine comes up.
/// Transient: the clock owes [`DB_INSTANCE_STATUS_AVAILABLE`]
/// [`STATUS_TRANSITION_DELAY`] later.
pub const DB_INSTANCE_STATUS_STARTING: &str = "starting";

/// The one `DBInstanceStatusInfo$StatusType` a DB instance reports: the member documents
/// "this value is currently \"read replication\"".
pub const READ_REPLICATION_STATUS_TYPE: &str = "read replication";

/// The `DBInstanceStatusInfo$Status` of a read replica that is replaying its source's
/// writes, from the values the member publishes for the read-replication status type:
/// "replicating, replication stop point set, replication stop point reached, error,
/// stopped, or terminated".
///
/// The only one of them this service can report: nothing it does sets a stop point, stops
/// the replication or breaks it, so every replica it holds is replicating until the
/// relationship itself ends (a promotion, a delete), after which the instance is no read
/// replica and reports no status info at all.
pub const READ_REPLICATION_STATUS_REPLICATING: &str = "replicating";

/// The DB instance statuses a reboot is accepted from, in the order Amazon RDS lists them
/// in the refusal it answers a request made in any other status with
/// ([`reboot_db_instance_state_message`]).
///
/// "Rebooting a DB instance restarts the database engine service", so there has to be an
/// engine to restart: beyond the running instance, the states RDS reboots are the ones in
/// which the instance is up and only its storage or its configuration is unsettled —
/// including the two `incompatible-` states, where a reboot is how a caller picks up a
/// corrected credential or parameter. Every other status (coming up, going down, stopped)
/// is refused.
pub const REBOOTABLE_DB_INSTANCE_STATUSES: [&str; 5] = [
    DB_INSTANCE_STATUS_AVAILABLE,
    "storage-optimization",
    "storage-initialization",
    "incompatible-credentials",
    "incompatible-parameters",
];

/// Whether a reboot is accepted for an instance reporting `status`; see
/// [`REBOOTABLE_DB_INSTANCE_STATUSES`].
pub fn reboot_is_accepted_from(status: &str) -> bool {
    REBOOTABLE_DB_INSTANCE_STATUSES.contains(&status)
}

/// The message the `InvalidDBInstanceStateFault` carries for a reboot asked of an instance
/// in a status it is not accepted from.
///
/// Amazon RDS states the permitted set FIRST — it is what a caller reads to decide whether
/// to wait or to fail — and then names the instance the request addressed and the status
/// that instance is actually in, taken from this service's own state. The two sentences are
/// separated by two spaces, as the service writes them.
pub fn reboot_db_instance_state_message(identifier: &str, status: &str) -> String {
    format!(
        "Can only reboot db instances with state in: {}.  Instance {identifier} has state: \
         {status}.",
        REBOOTABLE_DB_INSTANCE_STATUSES.join(", ")
    )
}

/// The sentence `ModifyDBInstance` answers an `InvalidDBInstanceStateFault` with when the
/// instance is in a TRANSIENT status a modification cannot be taken in — one the instance
/// will leave on its own (coming up, going down, stopped, starting). A status it can never
/// leave is worded by [`crate::state::modify_db_instance_invalid_state_message`], which
/// picks between the two.
///
/// It has NO variable part: Amazon RDS interpolates neither the identifier the request
/// carried nor the status the instance is in — it states only the state a modification
/// needs — so this is a constant rather than a `format!` over the request and the store,
/// and a caller matching the sentence sees exactly what the real service sends.
pub const MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE: &str =
    "Database instance is not in available state.";

/// The sentence every `DBInstanceAlreadyExistsFault` of this service carries: the answer to
/// a request that would put a second DB instance under an identifier the region already
/// holds — the create that names a stored identifier, and the modification that renames an
/// instance onto one.
///
/// It has NO variable part: Amazon RDS states only that the instance exists and
/// interpolates neither the identifier the request carried nor the one it holds, so this is
/// a constant rather than a `format!` over the request, and a caller matching the sentence
/// reads exactly what the real service sends. It carries no terminal period.
///
/// It lives here rather than in one handler because the create and the rename answer the
/// same one fault, so neither can drift from the other.
pub const DB_INSTANCE_ALREADY_EXISTS_MESSAGE: &str = "DB instance already exists";

/// The message `StopDBInstance` answers an `InvalidDBInstanceStateFault` with when the
/// instance is not in the one state a stop is accepted from
/// ([`DB_INSTANCE_STATUS_AVAILABLE`]).
///
/// Amazon RDS names the instance the request addressed and states the state a stop needs;
/// it does NOT report the status the instance is actually in, so nothing of the store but
/// the instance's identifier is in it.
pub fn stop_db_instance_state_message(identifier: &str) -> String {
    format!("Instance {identifier} is not in available state.")
}

/// The message `PromoteReadReplica` answers an `InvalidDBInstanceStateFault` with for a DB
/// instance that is no read replica at all: one that was never created as one, or one an
/// earlier promotion already made standalone.
///
/// Like the cluster-level promotion's sentence for the same precondition
/// ([`crate::state::PROMOTE_NOT_A_READ_REPLICA_MESSAGE`]) it has NO variable part — Amazon
/// RDS states only the precondition the call has, interpolating neither the identifier the
/// request carried nor the instance's status — so it is a constant rather than a `format!`
/// over the request, and a caller matching on the text to tell this refusal from the status
/// refusal of the same fault sees exactly what the real service sends.
pub const PROMOTE_INSTANCE_NOT_A_READ_REPLICA_MESSAGE: &str =
    "DB Instance is not a read replica.";

/// The message `PromoteReadReplica` answers an `InvalidDBInstanceStateFault` with when the
/// read replica is not in the one state a promotion is accepted from
/// ([`DB_INSTANCE_STATUS_AVAILABLE`]) — "a read replica cannot be promoted to a standalone
/// instance when it is in the backing-up status", and no more so while it is still coming
/// up, going down or stopped.
///
/// It is the sentence Amazon RDS words for an instance-level call that needs the running
/// instance, so it delegates to [`stop_db_instance_state_message`] rather than restating
/// it: the two front doors cannot then drift apart.
pub fn promote_read_replica_state_message(identifier: &str) -> String {
    stop_db_instance_state_message(identifier)
}

/// How a request that would turn a DB instance's automated backups off while another
/// instance is built from them is refused: `BackupRetentionPeriod` "can't be set to 0 if the
/// DB instance is a source to read replicas", so the member and that constraint are the
/// whole sentence.
///
/// It has NO variable part — the published constraint names neither the instance nor the
/// replicas built from it — so it is a constant rather than a `format!` over the request and
/// the store, and a caller matching the sentence reads the constraint it has to satisfy.
pub const BACKUP_RETENTION_ZERO_WITH_READ_REPLICAS_MESSAGE: &str =
    "BackupRetentionPeriod can't be set to 0 if the DB instance is a source to read replicas.";

/// The DB instance statuses a start is accepted from, as Amazon RDS lists them INSIDE the
/// quotes of [`start_db_instance_state_message`] — the stopped instance a start brings back
/// up, plus the two states a start is how a caller recovers from, the second with the
/// engine qualification RDS attaches to it.
///
/// The list is the refusal's fixed text, so it is one string rather than an array of
/// statuses: its last entry carries a parenthetical that qualifies the entry and is not
/// part of any status value. The only one of them this service ever puts an instance in is
/// [`DB_INSTANCE_STATUS_STOPPED`], which is what `StartDBInstance` accepts.
pub(super) const STARTABLE_DB_INSTANCE_STATUSES: &str =
    "stopped, inaccessible-encryption-credentials-recoverable, incompatible-network (only \
     valid for non-SqlServer instances)";

/// The message `StartDBInstance` answers an `InvalidDBInstanceStateFault` with when the
/// instance is in none of the statuses a start is accepted from.
///
/// Amazon RDS names the instance the request addressed and then the permitted set — what a
/// caller reads to decide whether to wait or to fail — single-quoted as a whole; it does
/// NOT report the status the instance is actually in.
pub fn start_db_instance_state_message(identifier: &str) -> String {
    format!(
        "Instance {identifier} cannot be started as it is not in one of the following \
         statuses: '{STARTABLE_DB_INSTANCE_STATUSES}'."
    )
}

/// The message `CreateDBSnapshot` answers an `InvalidDBInstanceStateFault` with when the
/// source instance is not in the one state a snapshot can be taken from
/// ([`DB_INSTANCE_STATUS_AVAILABLE`]).
///
/// Amazon RDS words this one as a statement about the snapshot it will not create, naming
/// the source instance and the state the instance has to be in; the status it is actually
/// in is not part of it.
pub fn create_db_snapshot_instance_state_message(identifier: &str) -> String {
    format!(
        "Cannot create a snapshot because the database instance {identifier} is not \
         currently in the available state."
    )
}

/// The ARN namespace and sigv4 signing name of this service, as its Smithy model's
/// `aws.api#service` trait declares them (`arnNamespace: rds`).
pub(super) const ARN_NAMESPACE: &str = "rds";

/// How Amazon RDS refuses a request that needs a DB instance's automated backups and finds
/// them turned off: ONE fixed sentence, which is why it is a constant — it names neither
/// the instance nor the member, only the operation that turns backups on, so the same text
/// answers every such request.
///
/// `CreateDBInstanceReadReplica$SourceDBInstanceIdentifier` states the constraint the
/// sentence reports: "The specified DB instance must have automatic backups enabled, that
/// is, its backup retention period must be greater than 0."
pub const AUTOMATED_BACKUPS_NOT_ENABLED_MESSAGE: &str =
    "Automated backups are not enabled for this database instance. To enable automated \
     backups, use ModifyDBInstance to set the backup retention period to a non-zero value.";

/// How Amazon RDS refuses a read replica of a DB instance that is a member of a DB
/// cluster: ONE fixed sentence, which is why it is a constant — it names neither the
/// instance nor the cluster, so the same text answers every such request.
///
/// A cluster member's replicas belong to the CLUSTER, which has its own operations for
/// them, so `CreateDBInstanceReadReplica` cannot serve one.
pub const READ_REPLICA_OF_A_CLUSTER_MEMBER_MESSAGE: &str =
    "Cannot create a read replica for a DB Instance that belongs to a DB Cluster";

/// Whether a DB instance of `engine` is one a read replica can be created FROM.
///
/// `CreateDBInstanceReadReplicaMessage$SourceDBInstanceIdentifier` states the constraint:
/// the source "must be the identifier of an existing Db2, MariaDB, MySQL, Oracle,
/// PostgreSQL, or SQL Server DB instance" — the six database products this service's own DB
/// instances run, RDS Custom's variants of them included, since an RDS Custom instance runs
/// the same Oracle or SQL Server engine.
///
/// What that list leaves out is every engine whose instances are members of a CLUSTER: the
/// Aurora engines and the two sibling control planes' ([`SIBLING_CONTROL_PLANE_ENGINES`]).
/// A replica of one of those is a cluster member its own cluster operations create, so this
/// operation cannot serve it.
pub fn engine_serves_db_instance_read_replica(engine: &str) -> bool {
    let engine = engine.to_ascii_lowercase();
    !engine.starts_with("aurora") && !is_sibling_control_plane_engine(&engine)
}

/// The engines a read replica's source may run, as
/// `CreateDBInstanceReadReplicaMessage$SourceDBInstanceIdentifier` names them — the text a
/// refusal lists, so a caller can fix the request from the message alone.
pub const READ_REPLICA_SOURCE_PRODUCTS: &str =
    "Db2, MariaDB, MySQL, Oracle, PostgreSQL, or SQL Server";

/// How many read replicas one DB instance of `engine` may have.
///
/// `CreateDBInstanceReadReplica$SourceDBInstanceIdentifier` publishes the limits itself:
/// "Each DB instance can have up to 15 read replicas, except for the following engines:
/// Db2 - Can have up to three replicas. Oracle - Can have up to five read replicas. SQL
/// Server - Can have up to five read replicas." The engine is matched the way every other
/// per-engine rule here matches one, by the engine FAMILY its name carries, so an edition
/// and its RDS Custom counterpart carry the same limit.
pub fn read_replica_limit(engine: &str) -> usize {
    let engine = engine.to_ascii_lowercase();
    if engine.starts_with("db2") {
        return 3;
    }
    if engine.contains("oracle") || engine.contains("sqlserver") {
        return 5;
    }
    15
}

/// Whether the DB parameter group family `family` is a family of `engine`.
///
/// A family is one major version of ONE engine, and the engine half is what its name
/// begins with ([`crate::ops::describe_engine_default_parameters::family_engine`] carries
/// that reading). Two engines that share a parameter vocabulary do NOT share a family: a
/// `mysql8.0` group is not a group an Aurora MySQL instance can be given, which is why the
/// engine the family names is compared rather than the catalog it publishes.
///
/// A value that names no family at all belongs to no engine.
pub fn family_belongs_to_engine(family: &str, engine: &str) -> bool {
    crate::ops::describe_engine_default_parameters::family_engine(family)
        .is_some_and(|named| named.eq_ignore_ascii_case(engine))
}

/// How many DB instances one region may hold: Amazon RDS's published default quota
/// ("DB instances": 40 per Region). Reaching it is the modelled
/// `InstanceQuotaExceededFault` — "The request would result in the user exceeding the
/// allowed number of DB instances".
pub const DB_INSTANCE_QUOTA_PER_REGION: usize = 40;

/// How many manual DB snapshots one region may hold: Amazon RDS's published default
/// quota ("Manual DB snapshots": 100 per Region). Reaching it is the modelled
/// `SnapshotQuotaExceededFault`, which both `CreateDBSnapshot` and `DeleteDBInstance`
/// declare.
pub const DB_SNAPSHOT_QUOTA_PER_REGION: usize = 100;

/// How many manual DB CLUSTER snapshots one region may hold: Amazon RDS's published
/// default quota ("Manual DB cluster snapshots": 100 per Region). Reaching it is the
/// modelled `SnapshotQuotaExceededFault` that `CopyDBClusterSnapshot` declares;
/// `DeleteDBClusterSnapshot` is the reclamation path.
pub const DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION: usize = 100;

/// The one cluster-snapshot attribute AWS defines, as
/// `DBClusterSnapshotAttribute$AttributeName` states: "The attribute named `restore`
/// refers to the list of Amazon Web Services accounts that have permission to copy or
/// restore the manual DB cluster snapshot."
pub const SNAPSHOT_RESTORE_ATTRIBUTE: &str = "restore";

/// The `restore` value that makes a manual snapshot public:
/// `DBClusterSnapshotAttribute$AttributeValues` — "If a value of `all` is in the list,
/// then the manual DB cluster snapshot is public and available for any Amazon Web
/// Services account to copy or restore."
pub const SNAPSHOT_ATTRIBUTE_ALL: &str = "all";

/// The digits in an Amazon Web Services account id, the other value
/// `ModifyDBClusterSnapshotAttributeMessage$ValuesToAdd` accepts ("set this list to
/// include one or more Amazon Web Services account IDs").
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// How many accounts one manual snapshot may be authorized for: AWS's published limit on
/// the accounts a manual snapshot can be shared with (20). Reaching it is the modelled
/// `SharedSnapshotQuotaExceededFault` that `ModifyDBClusterSnapshotAttribute` declares.
pub const MAX_SNAPSHOT_SHARED_ACCOUNTS: usize = 20;

/// The status a cluster snapshot carries while `CopyDBClusterSnapshot` is still copying
/// it. The operation's own documentation names it: "To cancel the copy operation after it
/// is in progress, delete the target DB cluster snapshot ... while that DB cluster
/// snapshot is in the copying status."
pub const SNAPSHOT_STATUS_COPYING: &str = "copying";

/// The status of a snapshot the clock has finished.
pub const SNAPSHOT_STATUS_AVAILABLE: &str = "available";

/// The status a snapshot carries while it is still being TAKEN of its source, the
/// counterpart of [`SNAPSHOT_STATUS_COPYING`] for a snapshot that is not a copy of another
/// one: `DBClusterSnapshot$Status` ("specifies the status of this DB cluster snapshot")
/// carries it, and `CreateDBClusterSnapshot` answers with it, since the snapshot is not
/// restorable the instant the call returns.
pub const SNAPSHOT_STATUS_CREATING: &str = "creating";

/// The `SnapshotType` of every snapshot a CALLER asked for, from the values
/// [`SNAPSHOT_TYPES`] lists: AWS classes one the caller took as `manual`, reserving
/// `automated` for the backups it schedules itself — and this service schedules none.
pub const SNAPSHOT_TYPE_MANUAL: &str = "manual";

/// How many cluster parameter groups one region may hold: Amazon RDS's published default
/// quota ("DB cluster parameter groups": 50 per Region), which
/// `DescribeAccountAttributes` reports as the `DBClusterParameterGroups` maximum, so the
/// report and the refusal state one and the same number. Reaching it is the modelled
/// `DBParameterGroupQuotaExceededFault` — "this request would cause you to exceed the
/// allowed number of parameter groups".
pub const DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION: usize = 50;

/// How many DB (instance) parameter groups one region may hold: Amazon RDS's published
/// default quota ("DB parameter groups": 50 per Region), reported as the
/// `DBParameterGroups` maximum for the same reason as
/// [`DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION`]. Reaching it is the modelled
/// `DBParameterGroupQuotaExceededFault` — "The request would result in the user exceeding
/// the allowed number of DB parameter groups".
pub const DB_PARAMETER_GROUP_QUOTA_PER_REGION: usize = 50;

/// How many parameters one cluster parameter group may carry.
///
/// Emulator resource guard, NOT an AWS quota: a group's parameter map is a growth site a
/// caller drives (`ModifyDBClusterParameterGroup` sets up to 20 parameters per call, and a
/// family this emulator publishes no catalog for admits any parameter name), so the group's
/// TOTAL is capped as well as each request's. It is comfortably above the seven parameters
/// the DocumentDB families publish; a modify that would leave a group past it is refused
/// with a 400 rather than stored.
pub const MAX_PARAMETERS_PER_GROUP: usize = 64;

/// How many option groups one region may hold: the allowance `CreateOptionGroup`
/// documents ("You can create up to 20 option groups"), which its modelled
/// `OptionGroupQuotaExceededFault` names in turn ("The quota of 20 option groups was
/// exceeded for this Amazon Web Services account"). AWS counts that allowance per
/// account and Region; this store is keyed by region, so it is enforced per region.
pub const OPTION_GROUP_QUOTA_PER_REGION: usize = 20;

/// How many DB subnet groups one region may hold: Amazon RDS's published default quota
/// ("DB subnet groups": 50 per Region). Reaching it is the modelled
/// `DBSubnetGroupQuotaExceededFault` — "The request would result in the user exceeding
/// the allowed number of DB subnet groups".
pub const DB_SUBNET_GROUP_QUOTA_PER_REGION: usize = 50;

/// How many subnets ONE DB subnet group may name: Amazon RDS's published default quota
/// ("Subnets per DB subnet group": 20). Reaching it is the modelled
/// `DBSubnetQuotaExceededFault` — "The request would result in the user exceeding the
/// allowed number of subnets in a DB subnet groups".
pub const MAX_SUBNETS_PER_DB_SUBNET_GROUP: usize = 20;

/// How many DB security groups one region may hold: Amazon RDS's published default quota
/// ("DB security groups": 25), the one `DescribeAccountAttributes` reports as
/// `DBSecurityGroups`. Reaching it is the modelled `DBSecurityGroupQuotaExceededFault` —
/// "The request would result in the user exceeding the allowed number of DB security
/// groups". AWS counts that allowance per account and Region; this store is keyed by
/// region, so it is enforced per region.
pub const DB_SECURITY_GROUP_QUOTA_PER_REGION: usize = 25;

/// How many ingress rules ONE DB security group may hold: Amazon RDS's published default
/// quota ("Authorizations per DB security group": 20), the one
/// `DescribeAccountAttributes` reports as `AuthorizationsPerDBSecurityGroup`. It bounds
/// the two ingress lists a stored group carries together, so one group's rules are capped
/// whichever form they take.
pub const MAX_AUTHORIZATIONS_PER_DB_SECURITY_GROUP: usize = 20;

/// How many event notification subscriptions one region may hold: Amazon RDS's published
/// default quota ("Event subscriptions": 20 per Region). Reaching it is the modelled
/// `EventSubscriptionQuotaExceededFault`, which `CreateEventSubscription` declares ("You
/// have reached the maximum number of event subscriptions").
pub const EVENT_SUBSCRIPTION_QUOTA_PER_REGION: usize = 20;

/// How many Aurora global database clusters this account may hold: the allowance Amazon
/// Aurora publishes for global databases (5). Reaching it is the modelled
/// `GlobalClusterQuotaExceededFault` — "The number of global database clusters for this
/// account is already at the maximum allowed" — which is worded per ACCOUNT, as is the
/// map this bounds: a global cluster is account-scoped and carries no region.
pub const GLOBAL_CLUSTER_QUOTA_PER_ACCOUNT: usize = 5;

/// How many DB shard groups one region may hold.
///
/// Aurora Limitless Database publishes no public allowance for DB shard groups, and each
/// one is request data that enters the store, so this bound is this emulator's — the same
/// reason [`MAX_FILTER_TERMS`] bounds a describe's filter terms. A create past it answers
/// the modelled `MaxDBShardGroupLimitReached` ("The maximum number of DB shard groups for
/// your Amazon Web Services account in the specified Amazon Web Services Region has been
/// reached"), which is what AWS answers when a create would exceed the allowance.
pub const DB_SHARD_GROUP_QUOTA_PER_REGION: usize = 50;

/// The status a DB shard group reports from the create that was accepted for it until the
/// clock makes its transition to [`DB_SHARD_GROUP_STATUS_AVAILABLE`] due.
///
/// `CreateDBShardGroup` "creates a new DB shard group for Aurora Limitless Database" and
/// answers before the shard group is usable, which is why the transition is owed at all;
/// the value is the `creating` status RDS reports for a resource it is still
/// provisioning, as `DBShardGroup$Status` ("the status of the DB shard group") carries it.
pub const DB_SHARD_GROUP_STATUS_CREATING: &str = "creating";

/// The status a DB shard group reports once it is usable, as `DBShardGroup$Status`
/// carries it.
pub const DB_SHARD_GROUP_STATUS_AVAILABLE: &str = "available";

/// The status a DB shard group reports once a reboot has been accepted for it, the same
/// `rebooting` status the other reboot path of this service reports: "Rebooting a DB
/// instance results in a momentary outage, during which the DB instance status is set to
/// rebooting", and a DB shard group is restarted the same way.
///
/// It is a TRANSIENT status: the clock owes the shard group
/// [`DB_SHARD_GROUP_STATUS_AVAILABLE`] again once the outage is over.
pub const DB_SHARD_GROUP_STATUS_REBOOTING: &str = "rebooting";

/// The status a DB shard group reports once a delete has been accepted for it — the same
/// `deleting` status the other delete paths of this service report
/// ([`DB_INSTANCE_STATUS_DELETING`], [`DB_CLUSTER_STATUS_DELETING`]).
///
/// It is a TRANSIENT status: the clock owes the record's removal from the store, after
/// which every read of the identifier is a `DBShardGroupNotFoundFault`.
pub const DB_SHARD_GROUP_STATUS_DELETING: &str = "deleting";

/// The values `CreateDBShardGroupMessage$ComputeRedundancy` documents: "Valid values are
/// the following: 0 - Creates a DB shard group without a standby DB data access shard.
/// This is the default value. 1 - Creates a DB shard group with a standby DB data access
/// shard in a different Availability Zone (AZ). 2 - Creates a DB shard group with two
/// standby DB data access shard in two different AZs."
pub const COMPUTE_REDUNDANCY_VALUES: [i32; 3] = [0, 1, 2];

/// The `ComputeRedundancy` a DB shard group created without one gets: "0 ... This is the
/// default value."
pub const DEFAULT_COMPUTE_REDUNDANCY: i32 = 0;

/// Refuse a `ComputeRedundancy` outside [`COMPUTE_REDUNDANCY_VALUES`], naming the value
/// and the set it is not in.
pub fn check_compute_redundancy(value: i32) -> Result<(), String> {
    if !COMPUTE_REDUNDANCY_VALUES.contains(&value) {
        return Err(format!(
            "invalid value {value} for ComputeRedundancy: must be one of {}",
            COMPUTE_REDUNDANCY_VALUES
                .iter()
                .map(i32::to_string)
                .collect::<Vec<String>>()
                .join(", ")
        ));
    }
    Ok(())
}

/// Refuse an Aurora capacity unit count that is not a capacity: a capacity in ACUs is a
/// positive, finite quantity.
pub fn check_capacity_units(member: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!(
            "invalid value {value} for {member}: a capacity in Aurora capacity units must \
             be a positive number"
        ));
    }
    Ok(())
}

/// Refuse a DB shard group capacity range that is not a range: `MinACU` is "the minimum
/// capacity of the DB shard group in Aurora capacity units (ACUs)" and `MaxACU` the
/// maximum, so a minimum above the maximum names no range at all. A shard group created or
/// modified without a minimum has none to compare, which is why the minimum is optional
/// here, and the pair is checked as it WOULD stand rather than one member at a time.
pub fn check_capacity_range(min_acu: Option<f64>, max_acu: f64) -> Result<(), String> {
    match min_acu {
        Some(min_acu) if min_acu > max_acu => Err(format!(
            "MinACU ({min_acu}) must not exceed MaxACU ({max_acu}): the minimum capacity of \
             a DB shard group cannot be above its maximum"
        )),
        _ => Ok(()),
    }
}

/// How many DB clusters one region may hold: Amazon RDS's published default quota ("DB
/// clusters": 40 per Region). Reaching it is the modelled `DBClusterQuotaExceededFault` —
/// "The user attempted to create a new DB cluster and the user has already reached the
/// maximum allowed DB cluster quota".
pub const DB_CLUSTER_QUOTA_PER_REGION: usize = 40;

/// The status a DB cluster reports from the create that was accepted for it until the
/// clock makes its transition to [`DB_CLUSTER_STATUS_AVAILABLE`] due.
///
/// `CreateDBCluster` "creates a new Amazon Aurora DB cluster" and answers before the
/// cluster is usable, which is why the transition below is owed at all; the value is the
/// `creating` status RDS reports for a cluster it is still provisioning, as
/// `DBCluster$Status` ("the current state of this DB cluster") carries it.
pub const DB_CLUSTER_STATUS_CREATING: &str = "creating";

/// The status a DB cluster reports once it is usable, as `DBCluster$Status` carries it.
pub const DB_CLUSTER_STATUS_AVAILABLE: &str = "available";

/// The status a DB cluster reports while it applies a modification, as `DBCluster$Status`
/// carries it.
///
/// `ModifyDBCluster` answers with the cluster in this status and the clock owes its return
/// to [`DB_CLUSTER_STATUS_AVAILABLE`]: a modification is applied to a cluster, not
/// instantly, and while one is being applied the cluster is not in a state another
/// modification can be taken in — which is what makes two concurrent `ModifyDBCluster`
/// calls on one cluster serialise, one accepted and the other refused with
/// `InvalidDBClusterStateFault`.
pub const DB_CLUSTER_STATUS_MODIFYING: &str = "modifying";

/// The status a DB cluster reports once a delete has been accepted for it, as
/// `DBCluster$Status` carries it — the same `deleting` status the other delete paths of
/// this service report ([`DB_INSTANCE_STATUS_DELETING`],
/// [`GLOBAL_CLUSTER_STATUS_DELETING`]).
///
/// It is a TRANSIENT status: `DeleteDBCluster` "deletes a previously provisioned DB
/// cluster", answering with the cluster it accepted the delete for, and the record then
/// leaves the store [`STATUS_TRANSITION_DELAY`] later — after which the identifier names
/// nothing and every read of it is the modelled `DBClusterNotFoundFault`, the signal the
/// AWS waiters poll for.
pub const DB_CLUSTER_STATUS_DELETING: &str = "deleting";

/// The status a DB cluster reports while a backup of it is being taken, as the published
/// cluster-status list carries it (Amazon Aurora User Guide, "Viewing Amazon Aurora DB
/// cluster status": `backing-up` — "the DB cluster is currently being backed up").
///
/// This service reaches it on one path: a `DeleteDBCluster` that asks for a final DB
/// cluster snapshot takes that snapshot before the cluster goes, and the cluster is backing
/// up while it is taken. Transient, like the statuses above — the clock owes the cluster
/// [`DB_CLUSTER_STATUS_DELETING`] once the snapshot is made, and its removal after that.
pub const DB_CLUSTER_STATUS_BACKING_UP: &str = "backing-up";

/// The status a DB cluster reports while a stop is running, as the published cluster-status
/// list carries it (Amazon Aurora User Guide, "Viewing Amazon Aurora DB cluster status").
///
/// Transient: `StopDBCluster` answers with the cluster in this status and the clock owes it
/// [`DB_CLUSTER_STATUS_STOPPED`] [`STATUS_TRANSITION_DELAY`] later, so a caller that reads
/// the cluster straight back sees the stop in progress, exactly as it would against the
/// real service.
pub const DB_CLUSTER_STATUS_STOPPING: &str = "stopping";

/// The status a DB cluster reports once its stop has finished, from the same published list
/// as [`DB_CLUSTER_STATUS_STOPPING`]. A stopped cluster keeps every other fact about it —
/// "Aurora retains the DB cluster's metadata, including its endpoints and DB parameter
/// groups".
pub const DB_CLUSTER_STATUS_STOPPED: &str = "stopped";

/// The status a DB cluster reports while a failover is running, from the same published
/// list as [`DB_CLUSTER_STATUS_STOPPING`].
///
/// Transient, like the other two: `FailoverDBCluster` "forces a failover" and answers
/// before it is over ("failover times are typically less than 35 seconds"), so the clock
/// owes the cluster [`DB_CLUSTER_STATUS_AVAILABLE`] and a caller polling straight after the
/// call observes the failover.
pub const DB_CLUSTER_STATUS_FAILING_OVER: &str = "failing-over";

/// The status a DB cluster reports while a read replica cluster is being promoted to a
/// standalone one, from the same published list as [`DB_CLUSTER_STATUS_STOPPING`] (Amazon
/// Aurora User Guide, "Viewing Amazon Aurora DB cluster status": `promoting` — the read
/// replica DB cluster is being promoted to a standalone DB cluster).
///
/// Transient, like the statuses above: `PromoteReadReplicaDBCluster` "promotes a read replica
/// DB cluster to a standalone DB cluster" and answers before the cluster is a standalone one,
/// so the clock owes it [`DB_CLUSTER_STATUS_AVAILABLE`] and a caller polling straight after
/// the call observes the promotion — and the state errors that status carries.
pub const DB_CLUSTER_STATUS_PROMOTING: &str = "promoting";

/// The status an event notification subscription reports once it is set up and
/// delivering, from `EventSubscription$Status`'s published list ("creating | modifying |
/// deleting | active | no-permission | topic-not-exist").
///
/// It is the status a subscription has from its create onwards: RDS answers
/// `CreateEventSubscription` with the subscription already active — creating one writes a
/// notification rule and provisions no resource — so no transient status is owed for it
/// and the clock owes it no transition. A later `ModifyEventSubscription` leaves it here
/// too: the change is in force by the time that call answers.
pub const EVENT_SUBSCRIPTION_STATUS_ACTIVE: &str = "active";

/// The status an Aurora global database cluster reports once it is usable, as
/// `GlobalCluster$Status` carries it. A global database created EMPTY provisions no DB
/// cluster — "You can create a global database that is initially empty, and then create
/// the primary and secondary DB clusters in the global database"
/// (`CreateGlobalCluster`) — so there is nothing to wait for and no transient status is
/// owed for it.
pub const GLOBAL_CLUSTER_STATUS_AVAILABLE: &str = "available";

/// The status a global database cluster reports in the answer to the delete that was
/// accepted for it, as `GlobalCluster$Status` carries it — the same `deleting` status the
/// other delete paths of this service report.
pub const GLOBAL_CLUSTER_STATUS_DELETING: &str = "deleting";

/// The status an Aurora global database cluster reports while a change to it is being
/// applied, the `modifying` status the other modify paths of this service report
/// ([`DB_CLUSTER_STATUS_MODIFYING`]).
///
/// Transient: `ModifyGlobalCluster` "modifies a setting for an Amazon Aurora global
/// database cluster" and answers before the setting is in force, so the clock owes the
/// global cluster [`GLOBAL_CLUSTER_STATUS_AVAILABLE`] [`STATUS_TRANSITION_DELAY`] later and
/// a caller reading straight after the modify sees the change in progress — and the state
/// errors that status carries, since a global cluster is not in a state another request can
/// be taken in while one is being applied.
pub const GLOBAL_CLUSTER_STATUS_MODIFYING: &str = "modifying";

/// The status an event notification subscription reports in the answer to the delete that
/// was accepted for it, from `EventSubscription$Status`'s published list.
pub const EVENT_SUBSCRIPTION_STATUS_DELETING: &str = "deleting";

/// `GlobalCluster$StorageEncryptionType` for one encrypted under the key the service
/// applies itself: "`sse-rds` - The global database cluster is encrypted using an Amazon
/// Web Services owned KMS key."
pub const STORAGE_ENCRYPTION_TYPE_SSE_RDS: &str = "sse-rds";

/// `GlobalCluster$StorageEncryptionType` for one encrypted under a managed key:
/// "`sse-kms` - The global database cluster is encrypted using a customer managed KMS key
/// or Amazon Web Services managed KMS key."
pub const STORAGE_ENCRYPTION_TYPE_SSE_KMS: &str = "sse-kms";

/// `DBCluster$StorageEncryptionType` for a cluster whose data at rest is not encrypted at
/// all: "`none` - The DB cluster is not encrypted." Aurora storage is always encrypted, so
/// only a Multi-AZ DB cluster created without `StorageEncrypted` reports it.
pub const STORAGE_ENCRYPTION_TYPE_NONE: &str = "none";

/// How the data at rest of a cluster running `engine` — and of every snapshot taken of it —
/// is protected, as `StorageEncryptionType` publishes it.
///
/// `sse-kms` where the resource was created encrypted, "encrypted using a customer managed
/// KMS key or Amazon Web Services managed KMS key". Otherwise it follows the cluster TYPE:
/// Aurora-style storage is encrypted at rest either way, so it reports
/// [`STORAGE_ENCRYPTION_TYPE_SSE_RDS`], "encrypted using an Amazon Web Services owned KMS
/// key" (`DBCluster$StorageEncrypted`: "If the DB cluster is encrypted with an Amazon Web
/// Services owned key, this value is false and StorageEncryptionType is sse-rds"), while a
/// Multi-AZ DB cluster created without encryption is not encrypted at all and reports
/// [`STORAGE_ENCRYPTION_TYPE_NONE`] ([`is_multi_az_db_cluster`]).
///
/// The ONE reading for every record that reports the member, so a cluster, its automated
/// backup and its snapshots cannot disagree about how the same data at rest is protected.
pub fn storage_encryption_type(storage_encrypted: bool, engine: &str) -> &'static str {
    if storage_encrypted {
        STORAGE_ENCRYPTION_TYPE_SSE_KMS
    } else if is_multi_az_db_cluster(engine) {
        STORAGE_ENCRYPTION_TYPE_NONE
    } else {
        STORAGE_ENCRYPTION_TYPE_SSE_RDS
    }
}

/// The lifecycle types a global database cluster may be enrolled in, as
/// `CreateGlobalClusterMessage$EngineLifecycleSupport` publishes them ("Valid Values:
/// open-source-rds-extended-support | open-source-rds-extended-support-disabled").
pub const ENGINE_LIFECYCLE_SUPPORT_VALUES: [&str; 2] = [
    DEFAULT_ENGINE_LIFECYCLE_SUPPORT,
    "open-source-rds-extended-support-disabled",
];

/// Refuse an `EngineLifecycleSupport` outside the two values the member publishes,
/// naming the member, the value rejected and the set it is not in.
pub fn check_engine_lifecycle_support(value: &str) -> Result<(), String> {
    if ENGINE_LIFECYCLE_SUPPORT_VALUES.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "invalid value {value} for EngineLifecycleSupport: must be one of {}",
        ENGINE_LIFECYCLE_SUPPORT_VALUES.join(", ")
    ))
}

/// The engines an Aurora global database cluster may run, as
/// `CreateGlobalClusterMessage$Engine` publishes them ("Valid Values: aurora-mysql |
/// aurora-postgresql"). A global database is an Aurora resource — "This operation applies
/// only to Aurora DB clusters" — so the list is narrower than the DB instance engines in
/// [`DB_INSTANCE_ENGINES`].
pub const GLOBAL_CLUSTER_ENGINES: [&str; 2] = ["aurora-mysql", "aurora-postgresql"];

/// The longest `EngineVersion` this service accepts as a version string at all. AWS's
/// longest published version strings are the SQL Server ones (`15.00.4382.1.v1`) and the
/// Aurora MySQL ones (`8.0.mysql_aurora.3.08.0`); 64 leaves room for any form AWS adds.
pub const MAX_ENGINE_VERSION_LEN: usize = 64;

/// The one sentence a global database cluster's `Engine` / `EngineVersion` refusal carries:
/// the engine the request named cannot back a global database, or the version it named is
/// no version of that engine at all. Amazon RDS answers both with this single text.
///
/// It has NO variable part — neither the rejected value nor the engines a global database
/// can run are interpolated — and it carries no terminal period, so a caller branching on
/// the sentence to tell this refusal from another of the same `InvalidParameterValue` code
/// sees exactly what the real service sends.
pub const GLOBAL_CLUSTER_ENGINE_NOT_FOUND_MESSAGE: &str =
    "The requested engine version was not found or does not support global functionality";

/// Refuse a value that is not an engine version at all: AWS writes every published
/// engine version as a dotted number, optionally carrying a lettered qualifier
/// (`16.6`, `15.00.4382.1.v1`, `8.0.mysql_aurora.3.08.0`, `16.4-limitless`), so a
/// version starts with a digit and is drawn from that alphabet.
///
/// This is a check on the FORM, for a member whose value this service cannot resolve
/// against a catalogue of its own. It is deliberately not a claim that the version
/// exists: the engine-version catalogue this emulator publishes is explicitly not the
/// real service's live list, so a version it does not carry is not a version AWS refuses.
///
/// Only the global database cluster operations resolve a version this way, and the version
/// they could not resolve is one a global database cannot be created or upgraded on, so the
/// refusal is their one sentence for that ([`GLOBAL_CLUSTER_ENGINE_NOT_FOUND_MESSAGE`]) —
/// which states exactly that the requested engine version was not found.
pub fn check_engine_version_form(value: &str) -> Result<(), String> {
    let length = value.chars().count();
    let shaped = value.starts_with(|c: char| c.is_ascii_digit())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !shaped || length > MAX_ENGINE_VERSION_LEN {
        return Err(GLOBAL_CLUSTER_ENGINE_NOT_FOUND_MESSAGE.to_string());
    }
    Ok(())
}

/// Refuse an `Engine` a global database cluster cannot run.
///
/// The refusal is [`GLOBAL_CLUSTER_ENGINE_NOT_FOUND_MESSAGE`]: this front door states
/// neither the value it rejected nor the engines it would have taken.
pub fn check_global_cluster_engine(engine: &str) -> Result<(), String> {
    if GLOBAL_CLUSTER_ENGINES.contains(&engine) {
        return Ok(());
    }
    Err(GLOBAL_CLUSTER_ENGINE_NOT_FOUND_MESSAGE.to_string())
}

/// The engines an option group may be associated with, as
/// `CreateOptionGroupMessage$EngineName` lists them — the identical list
/// `DescribeOptionGroupsMessage$EngineName` publishes for its filter
/// (`smithy_query(query="doc",shape="CreateOptionGroupMessage",member="EngineName")`:
/// "Valid Values: db2-ae db2-ce db2-se mariadb mysql oracle-ee oracle-ee-cdb
/// oracle-se2 oracle-se2-cdb postgres sqlserver-ee sqlserver-se sqlserver-ex
/// sqlserver-web"). A value AWS allocated, so the only honest source is the model's
/// own list; extend it when the model does.
pub const OPTION_GROUP_ENGINE_NAMES: [&str; 14] = [
    "db2-ae",
    "db2-ce",
    "db2-se",
    "mariadb",
    "mysql",
    "oracle-ee",
    "oracle-ee-cdb",
    "oracle-se2",
    "oracle-se2-cdb",
    "postgres",
    "sqlserver-ee",
    "sqlserver-se",
    "sqlserver-ex",
    "sqlserver-web",
];

/// Refuse an `EngineName` outside the value list its member publishes
/// ([`OPTION_GROUP_ENGINE_NAMES`]). Returns the message to refuse the request with,
/// naming the member and the constraint it broke.
pub fn check_option_group_engine_name(engine_name: &str) -> Result<(), String> {
    if OPTION_GROUP_ENGINE_NAMES.contains(&engine_name) {
        return Ok(());
    }
    Err(format!(
        "invalid value '{engine_name}' for EngineName: valid values are {}",
        OPTION_GROUP_ENGINE_NAMES.join(", ")
    ))
}

/// The engines a DB instance can run: the union of the engine value lists this model's own
/// members publish, because one member's list is only as new as the release that wrote it.
/// `CreateDBInstanceMessage$Engine`
/// (`smithy_query(query="doc",shape="CreateDBInstanceMessage",member="Engine")`:
/// "Valid Values: aurora-mysql aurora-postgresql custom-oracle-ee custom-oracle-ee-cdb
/// custom-oracle-se2 custom-oracle-se2-cdb custom-sqlserver-ee custom-sqlserver-se
/// custom-sqlserver-web custom-sqlserver-dev db2-ae db2-ce db2-se mariadb mysql
/// oracle-ee oracle-ee-cdb oracle-se2 oracle-se2-cdb postgres sqlserver-dev-ee
/// sqlserver-ee sqlserver-se sqlserver-ex sqlserver-web") names all but one of them, and
/// `CreateDBCapacityReservationRequest$Engine`
/// (`smithy_query(query="doc",shape="CreateDBCapacityReservationRequest",member="Engine")`)
/// names the remaining one, the second Developer Edition engine: "…  sqlserver-dev-ee |
/// sqlserver-dev-se | db2-ae …". Capacity is reserved for DB instances of an engine, so an
/// engine that list carries is an engine a DB instance runs. These are values AWS
/// allocated, so the only honest source is the model's own lists; extend it when the
/// model does.
pub const DB_INSTANCE_ENGINES: [&str; 26] = [
    "aurora-mysql",
    "aurora-postgresql",
    "custom-oracle-ee",
    "custom-oracle-ee-cdb",
    "custom-oracle-se2",
    "custom-oracle-se2-cdb",
    "custom-sqlserver-dev",
    "custom-sqlserver-ee",
    "custom-sqlserver-se",
    "custom-sqlserver-web",
    "db2-ae",
    "db2-ce",
    "db2-se",
    "mariadb",
    "mysql",
    "oracle-ee",
    "oracle-ee-cdb",
    "oracle-se2",
    "oracle-se2-cdb",
    "postgres",
    "sqlserver-dev-ee",
    "sqlserver-dev-se",
    "sqlserver-ee",
    "sqlserver-ex",
    "sqlserver-se",
    "sqlserver-web",
];

/// The engines the two sibling control planes this same front door serves publish for
/// their own `CreateDBInstance` (`src/service.rs` claims the Amazon DocumentDB and
/// Amazon Neptune Query surfaces too, and `DescribeOrderableDBInstanceOptions` reports
/// their classes): `com.amazonaws.docdb#CreateDBInstanceMessage$Engine` ("Valid value:
/// docdb") and Neptune's ("neptune"). A request for one of them is a request to THAT
/// service, so its engine is as published as the ones above.
pub const SIBLING_CONTROL_PLANE_ENGINES: [&str; 2] = ["docdb", "neptune"];

/// The Amazon DocumentDB engine, the first of [`SIBLING_CONTROL_PLANE_ENGINES`]: named
/// rather than spelled out wherever that control plane publishes a value of its own for
/// one of this model's members.
pub const DOCDB_ENGINE: &str = SIBLING_CONTROL_PLANE_ENGINES[0];

/// The Amazon Neptune engine, the second of [`SIBLING_CONTROL_PLANE_ENGINES`]; see
/// [`DOCDB_ENGINE`].
pub const NEPTUNE_ENGINE: &str = SIBLING_CONTROL_PLANE_ENGINES[1];

/// Whether `engine` belongs to one of the two sibling control planes this front door also
/// serves ([`SIBLING_CONTROL_PLANE_ENGINES`]).
///
/// A resource of such an engine is a resource of THAT service: the defaults and the value
/// lists its own API publishes are the ones that hold for it, not this model's, so every
/// per-engine rule below asks this first.
pub fn is_sibling_control_plane_engine(engine: &str) -> bool {
    SIBLING_CONTROL_PLANE_ENGINES
        .iter()
        .any(|sibling| sibling.eq_ignore_ascii_case(engine))
}

/// The engines a DB CLUSTER can run, as `CreateDBClusterMessage$Engine` lists them
/// (`smithy_query(query="doc",shape="CreateDBClusterMessage",member="Engine")`: "Valid
/// Values: aurora-mysql aurora-postgresql mysql postgres neptune"). It is a different,
/// much shorter list than [`DB_INSTANCE_ENGINES`] — only the engines that have a cluster
/// form — which is why `CreateDBCluster` resolves its `Engine` against this one.
///
/// `docdb` joins them for the same reason it joins [`SIBLING_CONTROL_PLANE_ENGINES`]: this
/// front door serves the Amazon DocumentDB control plane too, and
/// `com.amazonaws.docdb#CreateDBClusterMessage$Engine` publishes "Valid values: docdb".
pub const DB_CLUSTER_ENGINES: [&str; 6] = [
    "aurora-mysql",
    "aurora-postgresql",
    "docdb",
    "mysql",
    "neptune",
    "postgres",
];

/// Refuse an `Engine` outside the value list `CreateDBClusterMessage$Engine` publishes
/// ([`DB_CLUSTER_ENGINES`]).
pub fn check_db_cluster_engine(engine: &str) -> Result<(), String> {
    if DB_CLUSTER_ENGINES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(engine))
    {
        return Ok(());
    }
    Err(format!(
        "Invalid DB engine: {engine} is not one of {}",
        DB_CLUSTER_ENGINES.join(", ")
    ))
}

/// Whether a cluster of `engine` is a MULTI-AZ DB CLUSTER rather than an Aurora-style one.
///
/// `CreateDBCluster` documents two cluster TYPES and gates most of its members on which
/// one is being created ("Valid for Cluster Type: Aurora DB clusters only", "This setting
/// is required to create a Multi-AZ DB cluster"). The type follows from the engine: the
/// Aurora engines — and the `docdb` and `neptune` engines of the two sibling control
/// planes this front door serves, whose clusters are Aurora-style too — are Aurora DB
/// clusters, while the RDS engines that have a cluster form (`mysql`, `postgres`) are
/// Multi-AZ DB clusters, "Multi-AZ deployments with two readable standby DB instances".
pub fn is_multi_az_db_cluster(engine: &str) -> bool {
    !is_aurora(engine)
        && !SIBLING_CONTROL_PLANE_ENGINES
            .iter()
            .any(|sibling| sibling.eq_ignore_ascii_case(engine))
}

/// The longest `DBClusterIdentifier` a cluster of `engine` may carry.
///
/// `DBClusterIdentifier`: "Must contain from 1 to 63 (for Aurora DB clusters) or 1 to 52
/// (for Multi-AZ DB clusters) letters, numbers, or hyphens."
pub fn db_cluster_identifier_max_len(engine: &str) -> usize {
    if is_multi_az_db_cluster(engine) {
        52
    } else {
        63
    }
}

/// The storage types a cluster of `engine` may be created with, as
/// `CreateDBClusterMessage$StorageType` publishes them ("Aurora DB clusters - aurora |
/// aurora-iopt1", "Multi-AZ DB clusters - io1 | io2 | gp3").
///
/// A cluster of one of [`SIBLING_CONTROL_PLANE_ENGINES`] takes the value list its OWN
/// control plane publishes instead: `com.amazonaws.docdb#CreateDBClusterMessage$StorageType`
/// ("Valid values for storage type - standard | iopt1 … Default value is standard"), and
/// Neptune's member states the same two values.
pub fn db_cluster_storage_types(engine: &str) -> &'static [&'static str] {
    if is_sibling_control_plane_engine(engine) {
        SIBLING_CLUSTER_STORAGE_TYPES
    } else if is_multi_az_db_cluster(engine) {
        &["io1", "io2", "gp3"]
    } else {
        &[AURORA_DEFAULT_STORAGE_TYPE, "aurora-iopt1"]
    }
}

/// The storage types a cluster of a sibling control plane's engine may be created with;
/// see [`db_cluster_storage_types`]. The first is that engine's default.
pub(super) const SIBLING_CLUSTER_STORAGE_TYPES: &[&str] = &["standard", "iopt1"];

/// The `AllocatedStorage` a cluster whose storage size is NOT fixed reports; see
/// [`DbClusterRecord::reported_allocated_storage`]. One gibibyte is the unit the member is
/// expressed in, so it is the smallest size such a cluster can report while its real size
/// "adjusts as needed".
pub const ELASTIC_CLUSTER_ALLOCATED_STORAGE: i32 = 1;

/// The storage type a DB cluster of `engine` gets when the request names none, or `None`
/// for a cluster type that has no default because the member is required to create it.
///
/// It is also the one value that is NOT reported back for that engine — `StorageType`:
/// "When you create an Aurora DB cluster with the storage type set to `aurora-iopt1`, the
/// storage type is returned in the response. The storage type isn't returned when you set
/// it to `aurora`", and the sibling control planes' own member states the same of
/// `standard` — so one function answers both questions and the two cannot disagree.
pub fn default_db_cluster_storage_type(engine: &str) -> Option<&'static str> {
    if is_sibling_control_plane_engine(engine) {
        Some(SIBLING_CLUSTER_STORAGE_TYPES[0])
    } else if is_multi_az_db_cluster(engine) {
        None
    } else {
        Some(AURORA_DEFAULT_STORAGE_TYPE)
    }
}

/// The storage type an Aurora DB cluster gets when the request names none
/// (`StorageType` "Default: Aurora DB clusters - `aurora`").
///
/// It is also the one value the member documents as NOT reported back: "When you create an
/// Aurora DB cluster with the storage type set to `aurora-iopt1`, the storage type is
/// returned in the response. The storage type isn't returned when you set it to
/// `aurora`."
pub const AURORA_DEFAULT_STORAGE_TYPE: &str = "aurora";

/// The engine modes a DB cluster may be created in, as
/// `CreateDBClusterMessage$EngineMode` publishes them: "The DB engine mode of the DB
/// cluster, either `provisioned` or `serverless`."
pub const DB_CLUSTER_ENGINE_MODES: [&str; 2] = [DEFAULT_DB_CLUSTER_ENGINE_MODE, "serverless"];

/// The engine mode a DB cluster created without one runs in.
///
/// `EngineMode` names two values and says the `serverless` one "only applies for Aurora
/// Serverless v1 DB clusters. Aurora Serverless v2 DB clusters use the `provisioned`
/// engine mode" — so a cluster the request named no mode for is a provisioned one, which
/// is also the mode every version of this service's engine-version catalogue publishes.
pub const DEFAULT_DB_CLUSTER_ENGINE_MODE: &str = "provisioned";

/// The scalability mode an Aurora DB cluster created without one runs in
/// (`ClusterScalabilityType`: "When set to `standard` (the default), the cluster uses
/// normal DB instance creation").
pub const DEFAULT_CLUSTER_SCALABILITY_TYPE: &str = "standard";

/// `DBCluster$LocalWriteForwardingStatus` for a cluster that forwards writes from a reader
/// to its writer, and for one that does not — the two settled values of the published
/// `LocalWriteForwardingStatus` list ("enabled | disabled | enabling | disabling |
/// requested"), which is what `CreateDBCluster$EnableLocalWriteForwarding` selects between.
pub const LOCAL_WRITE_FORWARDING_ENABLED: &str = "enabled";

/// See [`LOCAL_WRITE_FORWARDING_ENABLED`].
pub const LOCAL_WRITE_FORWARDING_DISABLED: &str = "disabled";

/// The status an IAM role associated with a DB cluster reports once the association is in
/// place, as `DBClusterRole$Status` publishes it ("ACTIVE | PENDING | INVALID").
pub const DB_CLUSTER_ROLE_STATUS_ACTIVE: &str = "ACTIVE";

/// Refuse a `MasterUsername` outside the constraints its member documents: "Must be 1 to
/// 16 letters or numbers. First character must be a letter."
///
/// The third documented constraint — "Can't be a reserved word for the chosen database
/// engine" — is NOT enforced: the reserved words are each engine's own SQL keyword list,
/// which this service does not hold, and refusing a name no published list carries would
/// refuse names AWS accepts.
pub fn check_master_username(value: &str) -> Result<(), String> {
    if !value.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "MasterUsername must be 1 to 16 letters or numbers; got '{value}'"
        ));
    }
    let length = value.chars().count();
    if length == 0 || length > MAX_MASTER_USERNAME_LEN {
        return Err(format!(
            "MasterUsername must be 1 to {MAX_MASTER_USERNAME_LEN} letters or numbers; \
             got {length} characters"
        ));
    }
    if !value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Err(format!(
            "the first character of MasterUsername must be a letter; got '{value}'"
        ));
    }
    Ok(())
}

/// The longest `MasterUsername` a DB cluster accepts ("Must be 1 to 16 letters or
/// numbers").
pub const MAX_MASTER_USERNAME_LEN: usize = 16;

/// The longest `RoleArn` an `AssociatedRoles` entry may carry: the bound the member's own
/// `@length` publishes (`DBClusterAssociatedRole$RoleArn`, 20..2048). The boundary
/// validator refuses a longer one before a handler runs; a handler that STORES the value
/// bounds it again where it enters the store, so the stored size is bounded locally rather
/// than only upstream.
pub const MAX_IAM_ROLE_ARN_LEN: usize = 2048;

/// Refuse a `MasterUserPassword` outside the constraints its member documents: "Can
/// include any printable ASCII character except \"/\", \"'\", \"\"\", or the \"@\" symbol",
/// and the LENGTH the engine the password belongs to publishes
/// ([`master_user_password_len`]).
///
/// The refusal is worded the way this Query API words a password it will not take: it names
/// the member and the constraint the value broke ("The parameter MasterUserPassword is not
/// a valid password because it is shorter than 8 characters."), because that text is all
/// the caller has to fix the request from.
///
/// The password itself is never stored or reported — the member is `@sensitive` and RDS
/// reports no member carrying it — so this is the only place it is looked at at all, and
/// the refusal quotes the constraint rather than the value.
///
/// This is the ALPHABET and LENGTH policy alone, which the password RDS generates for a
/// managed secret satisfies too. Whether the engine takes a caller-supplied password at all
/// is [`check_supplied_master_user_password`], the check every operation that reads the
/// member off a REQUEST runs.
pub fn check_master_user_password(engine: &str, value: &str) -> Result<(), String> {
    let length = value.chars().count();
    let allowed = master_user_password_len(engine);
    if length < *allowed.start() {
        return Err(format!(
            "The parameter MasterUserPassword is not a valid password because it is \
             shorter than {} characters.",
            allowed.start()
        ));
    }
    if length > *allowed.end() {
        return Err(format!(
            "The parameter MasterUserPassword is not a valid password because it is \
             longer than {} characters.",
            allowed.end()
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_graphic() && !matches!(c, '/' | '"' | '@'))
    {
        return Err(
            "The parameter MasterUserPassword is not a valid password. Only printable \
             ASCII characters besides '/', '@', '\"', ' ' may be used."
                .to_string(),
        );
    }
    Ok(())
}

/// How long a `MasterUserPassword` may be on `engine`.
///
/// `MasterUserPassword` publishes the bound PER ENGINE
/// ("MariaDB: Must contain from 8 to 41 characters", "Microsoft SQL Server: Must contain
/// from 8 to 128 characters", "MySQL: Must contain from 8 to 41 characters", "Oracle: Must
/// contain from 8 to 30 characters", "PostgreSQL: Must contain from 8 to 128 characters"),
/// so the bound is read from the engine the password is being set on rather than from one
/// engine's line. Every engine publishes the SAME lower bound of 8, which is why an engine
/// string this does not recognise still carries it, with the widest published upper bound.
pub fn master_user_password_len(engine: &str) -> std::ops::RangeInclusive<usize> {
    let lowered = engine.to_ascii_lowercase();
    let longest = match lowered.as_str() {
        _ if lowered.contains("mysql") || lowered.starts_with("mariadb") => 41,
        _ if lowered.starts_with("oracle") || lowered.starts_with("custom-oracle") => 30,
        // Amazon DocumentDB, whose SDK clients this front door serves too: "Must contain
        // from 8 to 100 characters".
        _ if lowered.starts_with("docdb") => 100,
        _ => 128,
    };
    8..=longest
}

/// Refuse a `MasterUserPassword` the REQUEST carried, wherever an operation of this front
/// door reads one: the create that sets the credential and the modification that changes
/// it, on a DB cluster and on a DB instance alike, so one rule holds across every operation
/// that takes the member.
///
/// An engine whose own API documents the member as one it does not take
/// ([`engine_takes_a_master_user_password`]) accepts no value for it at all, so every
/// password supplied for such an engine is refused with [`INVALID_MASTER_PASSWORD_MESSAGE`]
/// before any length or character rule is read — the request names a credential the
/// resource has no place for, and a create that accepted it would store state AWS would
/// never have created. Everything else is the engine's published password policy
/// ([`check_master_user_password`]).
pub fn check_supplied_master_user_password(engine: &str, value: &str) -> Result<(), String> {
    if !engine_takes_a_master_user_password(engine) {
        return Err(INVALID_MASTER_PASSWORD_MESSAGE.to_string());
    }
    check_master_user_password(engine, value)
}

/// Whether `engine` has a master user password a request may set at all.
///
/// The member is not offered by every control plane this front door serves: Amazon
/// Neptune's own API documents `MasterUserPassword` as "Not supported by Neptune" on every
/// operation that carries it (`smithy_query(query="doc",shape="CreateDBInstanceMessage",
/// member="MasterUserPassword")` on that model, and the same sentence on
/// `CreateDBClusterMessage`, `ModifyDBInstanceMessage` and `ModifyDBClusterMessage`), so
/// that engine's resources have no master user credential for a request to name and no
/// value is acceptable for it. Compared case-insensitively, the way every per-engine rule
/// here compares an engine name.
pub fn engine_takes_a_master_user_password(engine: &str) -> bool {
    !engine.eq_ignore_ascii_case(NEPTUNE_ENGINE)
}
/// What a `MasterUserPassword` supplied for an engine that takes none
/// ([`engine_takes_a_master_user_password`]) is refused with.
///
/// Entirely FIXED text: the value is `@sensitive`, and this front door echoes no part of it
/// — not its length, not a character of it — so the sentence names neither the value nor
/// the member's constraints, which the engine publishes none of.
pub const INVALID_MASTER_PASSWORD_MESSAGE: &str = "Invalid master password";

/// What a delete that names NEITHER of its two final-snapshot members is refused with.
///
/// `SkipFinalSnapshot`: "If you set this value to false or don't specify it, RDS creates a
/// DB cluster snapshot before it deletes the DB cluster … If SkipFinalSnapshot is disabled,
/// you must specify a value for the FinalDBSnapshotIdentifier parameter" — exactly one of
/// the two has to be given, which is what the sentence states: it names the member the
/// request has to carry and the member that waives it, and nothing of the request or the
/// store. One constant for every delete that carries the pair, so one refusal class is
/// worded one way whichever resource is being deleted.
pub const FINAL_SNAPSHOT_MEMBER_REQUIRED: &str =
    "FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.";


/// The `BackupRetentionPeriod` a DB cluster accepts ("Constraints: Must be a value from 1
/// to 35") — a different lower bound than a DB instance's
/// [`BACKUP_RETENTION_RANGE`], whose own member documents "0 to 35".
pub const CLUSTER_BACKUP_RETENTION_RANGE: std::ops::RangeInclusive<i32> = 1..=35;

/// The `BacktrackWindow` an Aurora MySQL DB cluster accepts, in seconds ("If specified,
/// this value must be set to a number from 0 to 259,200 (72 hours)"). Zero disables
/// backtracking, which is also the documented default.
pub const BACKTRACK_WINDOW_RANGE: std::ops::RangeInclusive<i64> = 0..=259_200;

/// How long an Aurora Serverless v2 DB instance may be idle before Aurora pauses it
/// (`ServerlessV2ScalingConfiguration$SecondsUntilAutoPause`: "Specify a value between 300
/// seconds (five minutes) and 86,400 seconds (one day)").
pub const SECONDS_UNTIL_AUTO_PAUSE_RANGE: std::ops::RangeInclusive<i32> = 300..=86_400;

/// The `PerformanceInsightsRetentionPeriod` the `advanced` Database Insights mode requires
/// (`DatabaseInsightsMode`: "If you set this value to advanced, you must also set the
/// PerformanceInsightsEnabled parameter to true and the
/// PerformanceInsightsRetentionPeriod parameter to 465").
pub const ADVANCED_INSIGHTS_RETENTION_PERIOD: i32 = 465;

/// The `DatabaseInsightsMode` that requires Performance Insights and the 465-day retention
/// period above.
pub const DATABASE_INSIGHTS_MODE_ADVANCED: &str = "advanced";

/// Whether `engine` names a database engine this front door publishes at all: one of the
/// engines a DB instance can run ([`DB_INSTANCE_ENGINES`]) or one of the sibling control
/// planes' ([`SIBLING_CONTROL_PLANE_ENGINES`]). Compared case-insensitively, the way every
/// per-engine rule below compares an engine name.
pub fn is_published_engine(engine: &str) -> bool {
    DB_INSTANCE_ENGINES
        .iter()
        .chain(SIBLING_CONTROL_PLANE_ENGINES.iter())
        .any(|known| known.eq_ignore_ascii_case(engine))
}

/// Refuse an `Engine` that names no database engine ([`is_published_engine`]) where the
/// member RESOLVES an engine family for a catalogue read.
///
/// A report narrowed by an engine name is not asking "which of these entries has this
/// engine" but "report this engine's entries": a value that is no engine names no engine
/// family at all, so Amazon RDS refuses it rather than answering the empty report that
/// would read as "this engine has no versions". The message is the one the real service
/// answers that with, naming the value the request carried. What it is resolved against is
/// the WHOLE published vocabulary ([`is_published_engine`]), so an engine the model
/// publishes on one member and not on another is still an engine here.
pub fn check_published_engine(engine: &str) -> Result<(), String> {
    if is_published_engine(engine) {
        return Ok(());
    }
    Err(format!("Unrecognized engine name: {engine}"))
}

/// The text Amazon RDS refuses an `Engine` that names no database engine it serves with:
/// ONE fixed sentence, stated once here because every operation that resolves an engine
/// name answers the same one. It states no rule and carries nothing from the request, so a
/// caller cannot read the rejected value back out of it.
pub const INVALID_DB_ENGINE_MESSAGE: &str = "Invalid DB engine";

/// Refuse an `Engine` outside the value list its member publishes
/// ([`DB_INSTANCE_ENGINES`], plus the sibling control planes'
/// [`SIBLING_CONTROL_PLANE_ENGINES`]). A value that is not an engine names no database
/// engine at all, which is what the real service refuses with `InvalidParameterValue`.
pub fn check_engine(engine: &str) -> Result<(), String> {
    if is_published_engine(engine) {
        return Ok(());
    }
    Err(INVALID_DB_ENGINE_MESSAGE.to_string())
}

/// Refuse a `MajorEngineVersion` the engine-version catalogue does not carry for
/// `engine_name`.
///
/// The major version is resolved against the SAME catalogue `DescribeDBEngineVersions`
/// reports, so the two cannot disagree: a major version no entry of that engine carries
/// is one this service cannot associate an option group with, and the group must not be
/// stored under it. The message is the real service's own wording for this refusal,
/// built from the request's engine name and major version.
pub fn check_major_engine_version(engine_name: &str, major: &str) -> Result<(), String> {
    if crate::ops::describe_db_engine_versions::offers_major_engine_version(engine_name, major) {
        return Ok(());
    }
    Err(format!(
        "Cannot find major version {major} for {engine_name}"
    ))
}

/// Refuse a `DBInstanceClass` that is not of the published RDS DB instance class form.
///
/// A DB instance class is written `db.<family>.<size>` — `db.m5.large`,
/// `db.t4g.micro`, `db.r5b.4xlarge.tpc2.mem8x` — plus the one exception
/// [`SERVERLESS_INSTANCE_CLASS`], the Aurora Serverless v2 capacity class. The SIZE
/// token is drawn from the published instance-size vocabulary ([`INSTANCE_SIZES`], or
/// `<n>xlarge`), so a value whose size token is not one of those names no class at all
/// and is what the real service refuses with `InvalidParameterValue`.
///
/// The FORM is the rule rather than a list of classes: which classes exist is AWS's
/// per-region, per-engine matrix, which moves with every instance-type launch and which
/// this service does not hold — `DescribeOrderableDBInstanceOptions` reports an
/// explicitly partial subset of it, so refusing everything outside that subset would
/// refuse classes AWS offers (`db.m6i.large`, `db.m5.2xlarge`).
pub fn check_db_instance_class(db_instance_class: &str) -> Result<(), String> {
    let malformed = || format!("Invalid DB Instance class: {db_instance_class}");
    if db_instance_class == SERVERLESS_INSTANCE_CLASS {
        return Ok(());
    }
    let Some(rest) = db_instance_class.strip_prefix("db.") else {
        return Err(malformed());
    };
    let parts: Vec<&str> = rest.split('.').collect();
    let [family, size, ..] = parts.as_slice() else {
        return Err(malformed());
    };
    let family_ok = !family.is_empty()
        && family
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if !family_ok || !is_instance_size(size) {
        return Err(malformed());
    }
    Ok(())
}

/// The Aurora Serverless v2 capacity class, the one DB instance class that is not
/// `db.<family>.<size>` (`DescribeOrderableDBInstanceOptions` offers it for the Aurora
/// engines).
pub const SERVERLESS_INSTANCE_CLASS: &str = "db.serverless";

/// The Aurora Serverless v2 platform version a cluster configured with Serverless v2
/// capacity runs on: `DBCluster$ServerlessV2PlatformVersion` is "the version of the Aurora
/// Serverless V2 platform used by the DB cluster", a service-managed version AWS publishes
/// for a Region and upgrades on its own, written as a whole number
/// (`DescribeServerlessV2PlatformVersions$ServerlessV2PlatformVersion`: "Example: 3").
///
/// Reference data, like the engine-version catalogue: no request names it and nothing here
/// chooses it per cluster, so every cluster this service holds that HAS a Serverless v2
/// scaling configuration runs on the one version this emulator publishes — which is what
/// the real service's "default platform version … the version used for new DB clusters"
/// (`DescribeServerlessV2PlatformVersions$DefaultOnly`) is. A cluster with no Serverless v2
/// capacity configured runs on no Serverless v2 platform and reports none.
pub const DEFAULT_SERVERLESS_V2_PLATFORM_VERSION: &str = "4";

/// Refuse a DB instance this service offers no orderable option for.
///
/// A DB instance is created as ONE combination — a DB instance class, an engine, a version
/// of that engine and a license model — and the real service creates it only where that
/// combination is offered, which is what `DescribeOrderableDBInstanceOptions` reports
/// ("the orderable DB instance options for a specified DB engine"). So a create resolves
/// its combination against that report: a class whose instance generation the report
/// publishes no option in at all
/// ([`crate::ops::describe_orderable_db_instance_options::publishes_instance_family`]), or
/// a `LicenseModel` the engine is offered under by none of them, is a combination that can
/// be ordered nowhere.
///
/// This is NOT [`check_db_instance_class`], which decides whether the value is a DB
/// instance class at ALL (an `InvalidParameterValue` on the member) — here the value is a
/// class and the request is refused for the COMBINATION the four members make, which is
/// the `InvalidParameterCombination` the real service answers, in the sentence it words it
/// with: the four values, each named by its member, so a caller can see which part of the
/// combination to change. The version is the one the create resolved against the
/// engine-version catalogue (what it would provision), and the licence the one it would
/// create the instance under when the request named none ([`default_license_model`]).
///
/// A licence the request LEFT OUT is not checked: the service picks it itself, so it is
/// offered by construction, and only a value the CALLER chose can disagree with the report.
pub fn check_orderable_db_instance(
    engine: &str,
    engine_version: &str,
    db_instance_class: &str,
    license_model: Option<&str>,
) -> Result<(), String> {
    let class_offered =
        crate::ops::describe_orderable_db_instance_options::publishes_instance_family(
            db_instance_class,
        );
    let license_offered = license_model.is_none_or(|named| {
        crate::ops::describe_orderable_db_instance_options::offers_license_model(engine, named)
    });
    if class_offered && license_offered {
        return Ok(());
    }
    let reported_license = license_model
        .or_else(|| default_license_model(engine))
        .or_else(|| {
            crate::ops::describe_orderable_db_instance_options::license_models(engine)
                .first()
                .copied()
        })
        .unwrap_or_default();
    Err(format!(
        "RDS does not support creating a DB instance with the following combination: \
         DBInstanceClass={db_instance_class}, Engine={engine}, \
         EngineVersion={engine_version}, LicenseModel={reported_license}. For supported \
         combinations of instance class and database engine version, see the documentation."
    ))
}

/// The named instance sizes a DB instance class's size token may be, as AWS publishes
/// them for its instance types. The remaining sizes are written `<n>xlarge`, which
/// [`is_instance_size`] accepts on the form rather than by listing every `n`.
pub const INSTANCE_SIZES: [&str; 6] = ["nano", "micro", "small", "medium", "large", "xlarge"];

/// Whether `size` is a published instance-size token: one of [`INSTANCE_SIZES`] or
/// `<n>xlarge` for a one- to three-digit `n` that is not zero-led (`2xlarge` …
/// `112xlarge` — there is no `0xlarge` and no `04xlarge`).
pub(super) fn is_instance_size(size: &str) -> bool {
    if INSTANCE_SIZES.contains(&size) {
        return true;
    }
    match size.strip_suffix("xlarge") {
        Some(count) => {
            (1..=3).contains(&count.len())
                && count.bytes().all(|b| b.is_ascii_digit())
                && !count.starts_with('0')
        }
        None => false,
    }
}

/// How many distinct regions this store keeps records for.
///
/// The per-region quotas alone would not bound the store: the region keying them is
/// the request's own SigV4 credential-scope region, which this emulator does not
/// authenticate, so without this cap a caller could open a fresh quota bucket per
/// request. With it the store holds at most `MAX_REGIONS` buckets, and so at most
/// `MAX_REGIONS * DB_INSTANCE_QUOTA_PER_REGION` DB instances and
/// `MAX_REGIONS * DB_SNAPSHOT_QUOTA_PER_REGION` DB snapshots in total. AWS operates
/// on the order of 36 commercial regions, so this bound is this emulator's and is
/// comfortably above the real surface; reaching it answers the same modelled quota
/// fault, which is what AWS answers when a create would exceed an allowance.
pub const MAX_REGIONS: usize = 64;

/// How many filter VALUES one describe request may carry in total, across all its
/// filters.
///
/// RDS documents no bound on `Filters`, and the scan runs while this service's one
/// state lock is held, so the work must be capped on both factors: the record count
/// is capped by the quotas above and the term count by this. A request past it is
/// refused with a 400 rather than served slowly; the bound is this emulator's, and
/// generous next to the five filter names either describe operation recognises.
pub const MAX_FILTER_TERMS: usize = 256;

/// A record's key: the region the request that created it was signed for, and the
/// resource's own identifier.
pub type RegionalKey = (String, String);
