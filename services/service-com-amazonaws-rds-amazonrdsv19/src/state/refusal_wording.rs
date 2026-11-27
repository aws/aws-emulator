// SPDX-License-Identifier: CC0-1.0

//! How this service classifies and words a refusal, for the rules more than one operation
//! holds a request to.
//!
//! A refusal is decided by WHAT makes the call impossible: what the request itself carries
//! (a value outside the domain a parameter accepts, or members that cannot hold together),
//! the resource a required identifier names not existing, or the addressed resource's own
//! lifecycle state. The sentences and the shared checks live here so the handlers that
//! answer the same refusal cannot drift from each other — the deletion-protection sentence
//! is one wording for the three resources that carry the setting, and the Aurora
//! Serverless v2 capacity rule is one check for the create that provisions a cluster and
//! the modification that rescales it.

use super::*;

/// Which KIND of DB cluster an operation serves, for the two that serve one kind only:
/// `StartDBCluster` and `StopDBCluster` "only appl[y] to Aurora DB clusters", and
/// `RebootDBCluster` is for "a non-Aurora Multi-AZ DB cluster".
///
/// `CreateDBCluster` documents the two kinds and the engines that make each one
/// ([`is_multi_az_db_cluster`]), so which kind a cluster IS follows from its engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterKind {
    /// An Aurora DB cluster — and the Aurora-style clusters of the sibling control planes
    /// this front door serves.
    Aurora,
    /// A Multi-AZ DB cluster: "Multi-AZ deployments with two readable standby DB
    /// instances".
    MultiAz,
    /// An Aurora MySQL DB cluster: a NARROWER kind than [`Self::Aurora`], which the two
    /// backtrack operations serve — both document "this action applies only to Aurora MySQL
    /// DB clusters" — because backtracking is a capability of that engine alone.
    AuroraMySql,
}

impl ClusterKind {
    /// How the kind is NAMED in a refusal, as the operations' own documentation names it.
    fn described(self) -> &'static str {
        match self {
            Self::Aurora => "an Aurora DB cluster",
            Self::MultiAz => "a non-Aurora Multi-AZ DB cluster",
            Self::AuroraMySql => "Aurora MySQL DB clusters",
        }
    }

    /// Whether a cluster running `engine` is of this kind.
    pub fn holds_for(self, engine: &str) -> bool {
        match self {
            Self::Aurora => !is_multi_az_db_cluster(engine),
            Self::MultiAz => is_multi_az_db_cluster(engine),
            // The one engine whose clusters support backtracking ([`BACKTRACK_ENGINE`]).
            Self::AuroraMySql => engine.eq_ignore_ascii_case(BACKTRACK_ENGINE),
        }
    }
}

/// What a point-in-time restore refuses a request that names NO instant to restore to with:
/// `RestoreTime` "can't be specified if the UseLatestRestorableTime parameter is enabled",
/// and one of the two has to name an instant for there to be anything to restore — so the
/// sentence states which of the two the request is missing. (Amazon RDS words the member in
/// this sentence as `UseLatestRestoreTime`.)
pub const RESTORE_TIME_REQUIRED_MESSAGE: &str =
    "If UseLatestRestoreTime is not true, the RestoreTime parameter must be specified.";

/// What it refuses a request that names BOTH with: the other half of the same rule, so the
/// two combinations are distinguishable by their sentence.
pub const RESTORE_TIME_CONFLICT_MESSAGE: &str =
    "If UseLatestRestoreTime is true, the RestoreTime parameter must be omitted.";

/// What a point-in-time restore refuses a `RestoreTime` EARLIER than the window the source
/// instance's automated backups cover with.
///
/// Amazon RDS words this one as a statement about the instance and the retention period that
/// decides the bound: `RestoreTime` "must be before the latest restorable time for the DB
/// instance" and a restore reaches "a point up to the number of days specified by the
/// BackupRetentionPeriod property", so a request for an earlier instant is refused with the
/// bound itself and the setting it follows from. The instant is written to the second, in the
/// UTC form the member itself is given in ("Must be a time in Universal Coordinated Time
/// (UTC) format. Example: `2009-09-07T23:45:00Z`").
pub fn restore_time_before_the_window_message(
    earliest: ::aws_smithy_types::DateTime,
    backup_retention_period: i32,
) -> String {
    format!(
        "The specified instance cannot be restored to a time earlier than {} because its \
         backup retention period is set to {backup_retention_period} days.",
        restore_time_text(earliest)
    )
}

/// What a point-in-time restore refuses a `RestoreTime` LATER than the instance's published
/// recovery point with: the sibling of the sentence above at the other end of the window,
/// where the bound is `LatestRestorableTime` itself — "you can restore to any point in time
/// before the time identified by the LatestRestorableTime property", and the instant that
/// member reports is the whole of what the sentence has to state.
pub fn restore_time_after_the_window_message(latest: ::aws_smithy_types::DateTime) -> String {
    format!(
        "The specified instance cannot be restored to a time later than {}.",
        restore_time_text(latest)
    )
}

/// How Amazon RDS refuses a point-in-time restore of a DB instance there is no instant to
/// restore to, which is what the modelled `PointInTimeRestoreNotEnabled` fault reports.
///
/// WHY the instance has no restorable instant decides the sentence, because the two reasons
/// ask the caller for different things. `BackupRetentionPeriod` "setting this parameter to 0
/// disables automated backups", so a period of ZERO means the instance keeps no continuous
/// backup at all and the caller has to turn backups on — the service's own fixed sentence for
/// that condition ([`AUTOMATED_BACKUPS_NOT_ENABLED_MESSAGE`]), which names the operation that
/// turns them on. A NON-ZERO period means backups are on and none has been taken yet, which
/// the caller clears by waiting rather than by changing anything: that sentence states the
/// period in effect — the only variable part of it — and that no backup covers any instant
/// yet, the state an instance is in while it is still being created and just after the period
/// was raised from zero.
pub fn point_in_time_restore_not_enabled_message(backup_retention_period: i32) -> String {
    if backup_retention_period < 1 {
        return AUTOMATED_BACKUPS_NOT_ENABLED_MESSAGE.to_string();
    }
    format!(
        "The backup retention period is set to {backup_retention_period}, but no backups are \
         available yet. Most likely this is because backup retention has only recently been \
         changed from zero to a non-zero value."
    )
}

/// An instant as a restore-window refusal writes one: the UTC date and time to the SECOND,
/// the form `RestoreTime` itself is given in — a recovery point's sub-second part is not part
/// of the bound the sentence states.
fn restore_time_text(at: ::aws_smithy_types::DateTime) -> String {
    let to_the_second = ::aws_smithy_types::DateTime::from_secs(at.secs());
    to_the_second
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| at.secs().to_string())
}

/// What an operation that serves ONE kind of DB cluster refuses a `DBClusterIdentifier`
/// naming a cluster of the other kind with.
///
/// The identifier names a cluster the operation does not apply to at all, which is a VALUE
/// the parameter does not accept rather than a state the cluster is in — so it is the
/// `InvalidParameterValue` Query API error, and the sentence states the operation, the
/// cluster, the engine that decides its kind and the kind the operation serves. One wording
/// for every operation in that position, so none of them can drift from the others.
pub fn cluster_kind_unsupported_message(
    operation: &str,
    identifier: &str,
    engine: &str,
    serves: ClusterKind,
) -> String {
    format!(
        "invalid value '{identifier}' for DBClusterIdentifier: {operation} applies only to \
         {}, and DB cluster {identifier} runs engine {engine}",
        serves.described()
    )
}

/// What an operation on a DB cluster refuses a cluster whose STATUS is not one it can act
/// in with.
///
/// Amazon RDS words this one as a statement about the cluster's state and the states the
/// call needs: the cluster (written `DbCluster`), the status it is in, and the statuses it
/// would have to be in, all three from the record and the call itself. One wording for every
/// operation in that position — the stop, the start, the reboot and the backtrack — so none
/// of them can drift from the others.
pub fn cluster_state_message(identifier: &str, status: &str, expected: &str) -> String {
    format!(
        "DbCluster {identifier} is in {status} state but expected it to be one of {expected}."
    )
}

/// What a backtrack of a DB cluster that has no backtracking available is refused with.
///
/// A cluster whose backtrack window is zero kept no consistent time to be rewound to ("To
/// disable backtracking, set this value to 0"), so it is not in a state the backtrack can
/// be performed in — the modelled `InvalidDBClusterStateFault`. Amazon RDS words this one
/// as a statement about the CAPABILITY the cluster does not have, with NO variable part at
/// all: neither the identifier the caller wrote, the window stored for the cluster nor its
/// status is interpolated. So it is a constant rather than a `format!` over the request,
/// exactly as the promotion's precondition sentence
/// ([`PROMOTE_NOT_A_READ_REPLICA_MESSAGE`]) is, and a caller telling this refusal from the
/// status refusal of the same fault ([`cluster_state_message`]) by its text sees what the
/// real service sends.
pub const BACKTRACK_UNAVAILABLE_MESSAGE: &str =
    "Backtrack is not currently available to this cluster.";

/// What a report of a DB cluster's backtracks refuses a `BacktrackIdentifier` that names
/// none of them with.
///
/// `DBClusterBacktrackNotFoundFault` is modelled for exactly one thing — "BacktrackIdentifier
/// doesn't refer to an existing backtrack" — so the sentence states that, in the
/// `<resource type> <id> not found.` form this front door's REPORTS word a missing resource
/// in ([`db_cluster_not_found_message`]). `named` is the value the request carried, so a
/// caller matching the message against what it sent finds it there.
pub fn db_cluster_backtrack_not_found_message(named: &str) -> String {
    format!("DBClusterBacktrack {named} not found.")
}

/// The message a `DBInstanceNotFoundFault` carries where a call RESOLVES a DB instance it
/// reads something out of and the identifier names none: the `<resource type> <id> not
/// found.` wording this front door answers a named resource it cannot resolve with, the
/// same form a DB cluster's ([`db_cluster_not_found_message`]) and a DB shard group's
/// ([`db_shard_group_not_found_message`]) take.
///
/// `named` is the value the REQUEST carried, not the key the store folded it to, so a caller
/// matching the message against what it sent finds it there.
///
/// One reader for the sentence, so the calls that reach INTO an instance cannot drift from
/// each other. An identifier that resolves to no instance at all is a different miss, worded
/// by [`db_instance_not_found_colon_message`]: the two misses carry the same fault, the same
/// `DBInstanceNotFound` code and the same 404, so the sentence is the only thing that tells a
/// caller whether waiting will clear the refusal.
pub fn db_instance_not_found_message(named: &str) -> String {
    format!("DBInstance {named} not found.")
}

/// The message a `DBInstanceNotFoundFault` carries where the identifier resolves to NO DB
/// instance in the request's Region: the `<resource type> not found: <id>` form this front
/// door's control plane answers an unresolvable DB instance with, the same form a DB cluster's
/// ([`db_cluster_not_found_colon_message`]) and a DB snapshot's
/// ([`db_snapshot_not_found_message`]) take.
///
/// It is the OTHER half of the pair above: an identifier that names nothing is answered this
/// way, while a call that found an instance it cannot reach into yet — one that has not
/// finished being created — is answered the instance-scoped way
/// ([`db_instance_not_found_message`]). A client telling "retry, it is still coming up" from
/// "stop, that identifier does not exist" has only the sentence to go on.
///
/// `named` is the value the REQUEST carried, not the key the store folded it to, so a caller
/// matching the message against what it sent finds it there.
pub fn db_instance_not_found_colon_message(named: &str) -> String {
    format!("DBInstance not found: {named}")
}

/// The message `PromoteReadReplica` carries for a `DBInstanceIdentifier` that names no DB
/// instance of the request's Region: `DB Instance not found: <id>`.
///
/// Amazon RDS words `DBInstanceNotFoundFault` per FRONT DOOR rather than from one crate-wide
/// template — the report of an instance says `DBInstance <id> not found.`
/// ([`db_instance_not_found_message`]), the calls that reach into one say `DBInstance not
/// found: <id>` ([`db_instance_not_found_colon_message`]), and the sibling role change on the
/// same resource words it its own way again — so the promotion answers the sentence THIS
/// operation answers. The fixed text is the whole requirement; the only variable part is the
/// `DBInstanceIdentifier` the request carried, in the spelling the caller wrote it in (only
/// the lookup folds it, since the member "is stored as a lowercase string").
pub fn promote_read_replica_not_found_message(named: &str) -> String {
    format!("DB Instance not found: {named}")
}

/// The message `RebootDBCluster` carries for a `DBClusterIdentifier` that names no DB cluster
/// of the request's Region: `The source cluster could not be found or cannot be accessed:
/// <id>`.
///
/// This front door words `DBClusterNotFoundFault` per operation too: a call that REPORTS a
/// cluster, or moves its lifecycle, states that the cluster is absent
/// ([`crate::state::db_cluster_not_found_message`]), while the reboot states only what it can
/// tell the caller about the cluster whose engine it would restart — it could not be found,
/// or it cannot be reached — and deliberately does not distinguish the two. The only variable
/// part is the cluster the request named, in the spelling it wrote it in, with an ARN reduced
/// to the identifier it names as every reference of this member is
/// ([`crate::state::db_cluster_lookup_identifier`]).
pub fn reboot_cluster_source_not_found_message(named: &str) -> String {
    format!("The source cluster could not be found or cannot be accessed: {named}")
}

/// The message `RestoreDBInstanceToPointInTime` carries for a source that names no DB
/// instance of the request's Region: `The source instance could not be found: <id>`.
///
/// This operation names TWO instances — the source it restores FROM
/// (`SourceDBInstanceIdentifier` or `SourceDbiResourceId`) and the target it creates
/// (`TargetDBInstanceIdentifier`) — so a refusal that merely said an instance was not found
/// would leave a caller unable to tell which of the two the fault is about. Amazon RDS words
/// it as the SOURCE, exactly as the sibling cluster reboot words the cluster it reads
/// ([`reboot_cluster_source_not_found_message`]), rather than from the shared not-found
/// template the reports of an instance use ([`db_instance_not_found_colon_message`]).
///
/// The fixed text is the whole requirement; the only variable part is the source the request
/// named, in the spelling it wrote it in — whichever of the two members named it, since both
/// name the same thing.
pub fn restore_source_db_instance_not_found_message(named: &str) -> String {
    format!("The source instance could not be found: {named}")
}

/// What `RebootDBCluster` refuses an AURORA DB cluster with.
///
/// "Use this operation only for a non-Aurora Multi-AZ DB cluster": Amazon RDS words this
/// refusal as a statement about the operation and the ENGINE the cluster runs — an Aurora
/// cluster is rebooted through its member instances instead — so the engine is the one
/// variable part, taken from the cluster's own record.
pub const REBOOT_AURORA_CLUSTER_UNSUPPORTED_MESSAGE: &str =
    "Rebooting cluster isn't supported for DB engine ";

/// The whole sentence, for the cluster's own engine: the fixed text above with the engine
/// the cluster runs and the terminal period.
pub fn reboot_cluster_unsupported_message(engine: &str) -> String {
    format!("{REBOOT_AURORA_CLUSTER_UNSUPPORTED_MESSAGE}{engine}.")
}

/// The DB instance statuses the instance never leaves for a state a request can act in:
/// the one an accepted delete puts it in, which this service's clock follows with the
/// instance's removal ([`DB_INSTANCE_STATUS_DELETING`]). Every other status this service
/// reports is one the instance comes back out of.
pub const TERMINAL_DB_INSTANCE_STATUSES: [&str; 1] = [DB_INSTANCE_STATUS_DELETING];

/// Whether an instance reporting `status` can never leave it; see
/// [`TERMINAL_DB_INSTANCE_STATUSES`].
pub fn is_terminal_db_instance_status(status: &str) -> bool {
    TERMINAL_DB_INSTANCE_STATUSES.contains(&status)
}

/// The sentence `ModifyDBInstance` answers an `InvalidDBInstanceStateFault` with when the
/// instance is STOPPED.
///
/// A stopped instance is not waiting for anything: it stays stopped until a caller starts
/// it, so Amazon RDS words this refusal as the INSTRUCTION out of it — start the instance,
/// then modify it — rather than as the state a modification needs, which is what it says for
/// a status the instance leaves on its own
/// ([`MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE`]). It interpolates neither the identifier
/// the request carried nor the status, so it is a constant rather than a `format!` over the
/// request, and a caller that reads the message learns what to do next.
pub const MODIFY_STOPPED_DB_INSTANCE_MESSAGE: &str =
    "You can't modify a stopped DB instance. Start the DB instance, and then modify it.";
/// What a request that CHANGES a DB instance's configuration answers an
/// `InvalidDBInstanceStateFault` with for an instance in `status`.
///
/// Amazon RDS words the refusal by whether the status is one the instance will LEAVE or one
/// it cannot: for a transient status it states the state a modification needs
/// ([`MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE`]), which is what a caller polls on, and for
/// a terminal one ([`TERMINAL_DB_INSTANCE_STATUSES`]) it names the state the instance is in
/// — taken from the instance's own record — and that waiting for it to change is futile.
///
/// `ModifyDBInstance` itself words one status of its own beside these
/// ([`modify_db_instance_state_message`]).
pub fn modify_db_instance_invalid_state_message(status: &str) -> String {
    if is_terminal_db_instance_status(status) {
        return format!(
            "The specified database instance is currently in {status} state, which is a \
             terminal state."
        );
    }
    MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE.to_string()
}

/// The sentence `ModifyDBInstance` answers an `InvalidDBInstanceStateFault` with for an
/// instance in `status`.
///
/// `ModifyDBInstance` words the refusal by WHAT the blocking status IS, which is what tells
/// a caller whether to wait, to act, or to give up: a status the instance leaves on its own
/// states the state a modification needs, a status the instance stays in until a CALLER acts
/// states the instruction out of it ([`MODIFY_STOPPED_DB_INSTANCE_MESSAGE`]), and a terminal
/// status names the state itself ([`modify_db_instance_invalid_state_message`], which words
/// the other two for every request that changes an instance's configuration).
pub fn modify_db_instance_state_message(status: &str) -> String {
    if status == DB_INSTANCE_STATUS_STOPPED {
        return MODIFY_STOPPED_DB_INSTANCE_MESSAGE.to_string();
    }
    modify_db_instance_invalid_state_message(status)
}

/// What a delete is refused with when the resource it names has deletion protection
/// enabled: `subject` is the resource as Amazon RDS names it in this sentence (see
/// [`DELETION_PROTECTED_DB_INSTANCE`] and its siblings).
///
/// Deletion protection is a setting the REQUEST conflicts with rather than a state the
/// resource is passing through — "the database can't be deleted when deletion protection is
/// enabled", and the way out is to turn the setting off and call again, which is exactly
/// what the sentence says. One wording for every resource that carries the setting, so no
/// two deletes can drift.
///
/// It has no variable part beyond the resource KIND. The recorded answers this front door
/// is held to carry exactly `Cannot delete protected Cluster, please disable deletion
/// protection and try again.` for a DB cluster and the same sentence with `DB Instance` for
/// a DB instance — the identifier the request named is NOT in them — so neither the
/// identifier nor anything else of the request or the store is substituted here. The GLOBAL
/// cluster delete is worded apart ([`deletion_protected_global_cluster_message`]), because
/// each of these deletes is held to the answer ITS own front door is recorded giving, and the
/// global-database one names the global cluster it refused.
pub fn deletion_protection_refusal_message(subject: &str) -> String {
    format!(
        "Cannot delete protected {subject}, please disable deletion protection and try \
         again."
    )
}

/// How [`deletion_protection_refusal_message`] names a DB instance.
pub const DELETION_PROTECTED_DB_INSTANCE: &str = "DB Instance";

/// How [`deletion_protection_refusal_message`] names a DB cluster.
pub const DELETION_PROTECTED_CLUSTER: &str = "Cluster";

/// What a delete of a GLOBAL cluster whose deletion protection is enabled is refused with:
/// the two halves [`deletion_protection_refusal_message`] words, with the global cluster the
/// request NAMED between them.
///
/// The global-database front door interpolates the identifier where the DB cluster and DB
/// instance deletes name the resource kind alone: a teardown that deletes several global
/// databases reads out of the sentence WHICH one is protected, and therefore whose
/// `DeletionProtection` to turn off. The identifier is the one variable part, and it is the
/// value the request carried rather than anything derived from the store.
pub fn deletion_protected_global_cluster_message(identifier: &str) -> String {
    format!(
        "Cannot delete protected Global Cluster {identifier}, please disable deletion \
         protection and try again."
    )
}

/// What a delete of a DB cluster that still holds member DB instances is refused with.
///
/// An Aurora DB cluster's DB instances are deleted one by one (`DeleteDBInstance`) before
/// the cluster can go, so a delete that would leave members behind is refused. Amazon RDS
/// words it as two sentences — that the cluster cannot be deleted, and the precondition that
/// blocks it: the cluster still contains DB instances that are not themselves being deleted,
/// which is the predicate the refusal is decided by. It names NEITHER the cluster nor the
/// instances that block it, so this is a constant rather than a `format!` over the request
/// and the store: the member identifiers are not disclosed, and a caller matching the
/// sentence reads what it has to do.
///
/// One wording for every front door of this model that deletes a cluster, so none of them
/// can drift from the others.
pub const CLUSTER_STILL_HAS_MEMBERS_MESSAGE: &str =
    "Cluster can't be deleted. It still contains DB instances in a non-deleting state.";

/// What a delete that names BOTH of its final-snapshot members is refused with.
///
/// Specifying `FinalDBSnapshotIdentifier` "and also skip the creation of a final DB
/// snapshot with the SkipFinalShapshot parameter" "results in an error": exactly one of the
/// two may be given, which is what the sentence states, naming both members and neither
/// value. One constant for every delete that carries the pair, for the same reason as
/// [`FINAL_SNAPSHOT_MEMBER_REQUIRED`] and [`CLUSTER_FINAL_SNAPSHOT_MEMBER_REQUIRED`], which
/// answer the other direction of the rule.
pub const ONLY_ONE_FINAL_SNAPSHOT_MEMBER: &str =
    "Only one of SkipFinalSnapshot and FinalDBSnapshotIdentifier can be specified";

/// What a `RebootDBInstance` that enables `ForceFailover` over an instance that is not
/// configured for Multi-AZ is refused with: "Constraint: You can't enable force failover if
/// the instance isn't configured for Multi-AZ."
///
/// Amazon RDS names the member and states the configuration it requires, naming the two
/// deployment options that provide it; it reports neither the instance the request
/// addressed nor the value it carried, so nothing of the request or the store is
/// substituted into it.
pub const FORCE_FAILOVER_REQUIRES_MULTI_AZ_MESSAGE: &str =
    "ForceFailover cannot be specified since the instance is not configured for either \
     MultiAZ or High Availability";

/// What a report refuses a `Filters` entry whose `Name` is not one it supports with.
///
/// Amazon RDS answers an unrecognized filter name with ONE sentence: fixed text, then the
/// name the request carried, and nothing after it — no enumeration of the names the
/// operation does support, and no mention of the operation. The only variable part is
/// `name`, read straight off the request's own `Filter$Name`, so a caller validating its
/// filter names against the refusal reads back the value it sent.
///
/// One wording for every report of this service that checks its filter names — the reports
/// that publish several names and the ones that publish none alike, since what is refused
/// is the same thing in both cases — so none of them can drift from the others.
pub fn unrecognized_filter_name_message(name: &str) -> String {
    format!("Unrecognized filter name: {name}")
}

/// What a request carrying a value one of the service's own member CONSTRAINTS does not
/// admit is refused with: the standard constraint report, over the one member that carried
/// it.
///
/// A constraint the service enforces on a member — rather than one the model declares, which
/// the boundary already reports — is still a constraint ON THAT MEMBER, so it is reported the
/// way every other one is: the count of findings, the offending value, the path of the member
/// it sat at and the requirement it broke. The envelope and the clause are the framework's
/// own (`aws_smithy_schema_server`), the same two this service restates a modelled refusal
/// with (`crate::service`), so a refusal a handler makes and one the boundary makes cannot
/// drift apart; `member_path` is spelled by the framework's path helpers for the same reason.
///
/// The requirement is stated as the regular expression the value had to match, which is what
/// lets a caller tell which entry of a list it sent is wrong and what would be accepted
/// instead.
pub fn member_pattern_violation_message(member_path: &str, value: &str, pattern: &str) -> String {
    let violation = ::aws_smithy_schema_server::Violation {
        path: member_path.to_string(),
        kind: ::aws_smithy_schema_server::ViolationKind::Pattern(pattern.to_string()),
        value: Some(value.to_string()),
        sensitive: false,
    };
    // `None` only for an EMPTY violation list, which this one is not; the clause itself is
    // the fallback so no refusal is ever left without a message.
    ::aws_smithy_schema_server::envelope(std::slice::from_ref(&violation))
        .unwrap_or_else(|| violation.message())
}

/// What a request that asks to move a database BACKWARD through its engine's releases is
/// refused with: the engine, the build the database runs and the build the request named.
///
/// `EngineVersion` asks for an upgrade — "upgrade the DB instance to a later engine version"
/// — and Amazon RDS has no way to take a database to an earlier build, so a request naming
/// one is refused with the sentence it words every such refusal as, whichever direction the
/// caller thought they were going: the fixed text `Cannot upgrade `, ` from ` and ` to `, with
/// the engine and the two builds taken from the request and from the record it addressed.
///
/// One wording here rather than in each handler, so a refusal of an instance's downgrade and
/// one of any other resource's cannot drift.
pub fn cannot_upgrade_engine_version_message(engine: &str, from: &str, to: &str) -> String {
    format!("Cannot upgrade {engine} from {from} to {to}")
}

/// Refuse a `BackupRetentionPeriod` outside the range the addressed resource accepts,
/// whichever operation carries the member: the creates, the modifications, the restores and
/// the promotion hold ONE rule, so none of them can drift from the others. `Err` carries the
/// sentence; `accepted` is the range the resource publishes — a DB cluster's
/// [`CLUSTER_BACKUP_RETENTION_RANGE`] ("Must be a value from 1 to 35") or a DB instance's
/// [`BACKUP_RETENTION_RANGE`] ("Must be a value from 0 to 35").
///
/// Amazon RDS words it as two sentences: the value the request carried, reported as the
/// period that is not valid, and then the bounds the engine holds the period to. The member
/// name is NOT in it — the sentence names the setting in prose ("backup retention period")
/// — and a caller parses the limits a period has to satisfy out of the second sentence,
/// which is why the bounds are interpolated from the range rather than written into the text.
pub fn check_backup_retention_period(
    period: i32,
    accepted: &std::ops::RangeInclusive<i32>,
) -> Result<(), String> {
    if accepted.contains(&period) {
        return Ok(());
    }
    Err(format!(
        "Invalid backup retention period: {period}. Retention period must be between {} and \
         {}.",
        accepted.start(),
        accepted.end()
    ))
}

/// Refuse an endpoint PORT outside the range the engine accepts, whichever member of
/// whichever operation named it: a DB cluster's `Port`, a DB instance's `Port` and the
/// `DBPortNumber` a modification moves an instance to all configure the port the engine
/// answers on, so the creates, the modifications and the restores hold ONE rule and none of
/// them can drift from the others. `Err` carries the sentence; `accepted` is the range the
/// engine publishes for an endpoint port ([`PORT_RANGE`], "Valid Values: 1150-65535").
///
/// Amazon RDS words it as the port it would not accept and then the range an endpoint port
/// has to fall in, written `<floor>-<ceiling>` and closed by a trailing space. The sentence
/// names no member — its subject is the ENDPOINT port, which is what each of those members
/// configures — so the value comes from the request and the bounds from the range the engine
/// publishes, and nothing else of the request or the store is in it.
pub fn check_endpoint_port(
    port: i32,
    accepted: &std::ops::RangeInclusive<i32>,
) -> Result<(), String> {
    if accepted.contains(&port) {
        return Ok(());
    }
    // The trailing space closes the sentence this service answers; it is not a separator.
    Err(format!(
        "Invalid endpoint port {port}. Valid range is: {}-{} ",
        accepted.start(),
        accepted.end()
    ))
}

/// The DB cluster statuses a new Aurora Limitless Database DB shard group can be
/// provisioned on: the cluster is up, or it is still being provisioned and the shard group
/// comes up with it.
///
/// Aurora puts the shards of a Limitless Database cluster INTO that cluster, so there has
/// to be a cluster to put them in: a cluster that is stopping, stopped, rebooting, failing
/// over, taking a modification or on its way out is not one a shard group can be added to,
/// which is what [`shard_group_cluster_state_message`] states. The two members of the set
/// are the statuses that refusal names, in the order it names them.
pub const SHARD_GROUP_CLUSTER_STATUSES: [&str; 2] =
    [DB_CLUSTER_STATUS_AVAILABLE, DB_CLUSTER_STATUS_CREATING];

/// What `CreateDBShardGroup` refuses a DB cluster whose status is none of
/// [`SHARD_GROUP_CLUSTER_STATUSES`] with.
///
/// Amazon RDS words this one as two sentences — what it could not do, and the state the DB
/// cluster is not in — naming the statuses a create needs in upper case and taking the
/// cluster identifier from the call. Nothing else of the request or the store is in it: the
/// status the cluster IS in is what a describe of that cluster reports, not part of this
/// sentence.
pub fn shard_group_cluster_state_message(db_cluster_identifier: &str) -> String {
    format!(
        "You can't create the DB shard group. The DB cluster {db_cluster_identifier} isn't \
         in the AVAILABLE or CREATING state."
    )
}

/// What a request naming an `EngineMode` the resolved engine VERSION does not run is
/// refused with.
///
/// Which modes a version runs is the domain of the member for that version — the catalogue
/// publishes it as `SupportedEngineModes` — so a mode outside it is a value the parameter
/// does not accept. Amazon RDS states that the mode the request asked for is not available,
/// naming that mode and nothing else of the request: it is how `serverless` (Aurora
/// Serverless v1) is refused on an engine version that publishes `provisioned` alone.
pub fn unavailable_engine_mode_message(engine_mode: &str) -> String {
    format!("The engine mode {engine_mode} you requested is currently unavailable.")
}

/// Why a `ServerlessV2ScalingConfiguration` a request carries cannot be served, carrying
/// the message naming the rule it broke. The two variants are the two Query API codes the
/// refusal belongs to: a capacity VALUE outside what the member accepts, and a pair of
/// values — or a value and the cluster's engine version — that cannot hold together.
#[derive(Debug, Clone)]
pub enum ServerlessV2Rejection {
    /// A value outside the domain the member accepts — `InvalidParameterValue`.
    Value(String),
    /// Members that cannot hold together — `InvalidParameterCombination`.
    Combination(String),
}

/// What a scaling configuration whose capacity floor is above its ceiling is refused with.
///
/// Entirely FIXED text, and a VALUE refusal rather than a combination one: the pair names a
/// capacity floor the member does not accept, which is how this front door is held to
/// answer it — `InvalidParameterValue` with this sentence — whichever of the two members
/// the caller has to change, so nothing of the request is substituted into it.
pub const SERVERLESS_V2_FLOOR_ABOVE_CEILING_MESSAGE: &str =
    "Serverless v2 minimum capacity must be less than or equal to maximum capacity";

/// What a capacity value outside the range the cluster can scale in is refused with.
///
/// The service states the VALUE it would not take and the range it will, and points at the
/// call that changes that range; it names neither the member nor the engine version, so a
/// floor and a ceiling outside the range are refused with the same sentence shape. The two
/// capacities are written as the doubles the member takes (`256.0`, not `256`).
///
/// This is the refusal every cluster engine but Aurora answers a capacity outside its
/// scaling range with, and the one an Aurora cluster answers a capacity BELOW what it scales
/// down to with; an Aurora capacity ABOVE the ceiling its engine version reaches is worded by
/// that version ([`serverless_v2_capacity_above_engine_ceiling_message`]).
pub fn serverless_v2_capacity_out_of_range_message(value: f64, min: f64, max: f64) -> String {
    format!(
        "Serverless v2 capacity value {value:?} is not valid. The valid scaling range for \
         this cluster is {min:?} to {max:?}. To change the cluster's scaling range, use the \
         ModifyDBCluster call."
    )
}

/// Where Amazon RDS points a caller whose scaling configuration asks for more capacity than
/// the cluster's engine version reaches: the Aurora Serverless v2 requirements the Amazon
/// Aurora User Guide publishes, which is where the per-version ACU ceilings are written down.
const SERVERLESS_V2_REQUIREMENTS_DOC: &str = "https://docs.aws.amazon.com/AmazonRDS/latest/\
                                              AuroraUserGuide/aurora-serverless-v2.requirements.html";

/// What an AURORA capacity value ABOVE the ceiling the cluster's engine version reaches is
/// refused with.
///
/// This wording is the Aurora control plane's: it points the caller at the Aurora Serverless
/// v2 requirements, the page where the per-version ACU ceilings are published. The other
/// cluster engines this front door serves state the cluster's own scaling range instead
/// ([`serverless_v2_capacity_out_of_range_message`]), whichever end of it the value missed.
///
/// The ceiling is a property of the engine VERSION — `ServerlessV2FeaturesSupport` reports
/// the limits that "differ between Aurora engine versions" — so the service states the
/// ceiling, the version that imposes it and where the per-version requirements are
/// published, and does NOT quote the rejected value: the two ways out are a lower capacity
/// or a version whose ceiling reaches it, and naming the version is what tells the caller
/// which. The ceiling is written as the double the member takes (`256.0`, not `256`).
pub fn serverless_v2_capacity_above_engine_ceiling_message(
    max: f64,
    engine_version: &str,
) -> String {
    format!(
        "Invalid scaling configuration. The capacity values greater than {max:?} ACUs are \
         not supported by the current engine version {engine_version}. Please refer to this \
         doc {SERVERLESS_V2_REQUIREMENTS_DOC} for instructions on fixing the issue."
    )
}

/// The increment a serverless capacity value is specified in: "You can specify ACU values
/// in half-step increments, such as 8, 8.5, 9, and so on"
/// (`ServerlessV2ScalingConfiguration$MinCapacity`), which the Neptune model states in the
/// same words for its own capacity units.
pub const SERVERLESS_V2_CAPACITY_STEP: f64 = 0.5;

/// What a capacity value off that grid is refused with: the service states the VALUE it
/// would not take and the increment every capacity is a multiple of, naming neither the
/// member nor the engine version — the same sentence shape as
/// [`serverless_v2_capacity_out_of_range_message`].
pub fn serverless_v2_capacity_step_message(value: f64) -> String {
    format!(
        "Serverless v2 capacity value {value:?} is not valid. It must be a multiple of \
         {SERVERLESS_V2_CAPACITY_STEP:?}."
    )
}

/// The Aurora Serverless v2 capacity range the SERVICE publishes, for a cluster whose
/// engine version's catalogue entry publishes none of its own.
///
/// `ServerlessV2FeaturesSupport` reports the limits that "differ between Aurora engine
/// versions" — the floor a version scales to (0 ACUs where it supports the automatic
/// pause/resume feature, half an ACU otherwise) and the ceiling it reaches ("might be 256
/// or 128") — so the widest published pair is the envelope every scaling configuration
/// falls in, whatever version the cluster runs.
const SERVERLESS_V2_PUBLISHED_CAPACITY: (f64, f64) = (0.0, 256.0);

/// Refuse an Aurora Serverless v2 scaling configuration a cluster cannot run, whichever
/// operation carries it: the create that provisions the cluster and the modification that
/// rescales it hold one rule, so neither can drift from the other.
///
/// `capacity` is the capacity unit range the cluster's engine VERSION publishes
/// (`DescribeDBEngineVersions`' capacity profile), and the range the service publishes
/// overall for a version that publishes none ([`SERVERLESS_V2_PUBLISHED_CAPACITY`]): the
/// member is the capacity range of the CLUSTER, which the control plane takes for any
/// Aurora cluster, so a version whose catalogue entry carries no range of its own is not
/// one the configuration itself is refused for — whether a `db.serverless` DB instance can
/// then be created in that cluster is that create's own answer.
///
/// `running` is the engine and the version the cluster will be RUNNING — the version is what
/// the Aurora refusal of a capacity above the ceiling names ("the current engine version"),
/// and the engine is what decides which wording that refusal takes; `None` where this call
/// resolved no version to name, and then that refusal falls back to the range sentence rather
/// than naming a version the service does not know.
///
/// A capacity value is checked against that range
/// ([`serverless_v2_capacity_above_engine_ceiling_message`] for an Aurora capacity above the
/// ceiling the version reaches, [`serverless_v2_capacity_out_of_range_message`] for every
/// other capacity outside the range)
/// and against the half-step
/// increments the member documents ("You can specify ACU values in half-step increments,
/// such as 8, 8.5, 9, and so on"), and the pair against each other (the floor cannot exceed
/// the ceiling).
pub fn check_serverless_v2_scaling(
    capacity: Option<(f64, f64)>,
    running: Option<(&str, &str)>,
    min_capacity: Option<f64>,
    max_capacity: Option<f64>,
    seconds_until_auto_pause: Option<i32>,
) -> Result<(), ServerlessV2Rejection> {
    let (min_acu, max_acu) = capacity.unwrap_or(SERVERLESS_V2_PUBLISHED_CAPACITY);
    for value in [min_capacity, max_capacity] {
        let Some(value) = value else { continue };
        if value > max_acu {
            // On Aurora the ceiling belongs to the engine version, so the refusal names that
            // version where this call resolved one; every other cluster engine states the
            // range the cluster scales in, the same sentence a capacity below the floor gets.
            let aurora_version = running
                .filter(|(engine, _)| super::is_aurora(engine))
                .map(|(_, version)| version);
            return Err(ServerlessV2Rejection::Value(match aurora_version {
                Some(version) => {
                    serverless_v2_capacity_above_engine_ceiling_message(max_acu, version)
                }
                None => serverless_v2_capacity_out_of_range_message(value, min_acu, max_acu),
            }));
        }
        if value < min_acu {
            return Err(ServerlessV2Rejection::Value(
                serverless_v2_capacity_out_of_range_message(value, min_acu, max_acu),
            ));
        }
        if (value / SERVERLESS_V2_CAPACITY_STEP).fract() != 0.0 {
            return Err(ServerlessV2Rejection::Value(
                serverless_v2_capacity_step_message(value),
            ));
        }
    }
    if let (Some(min), Some(max)) = (min_capacity, max_capacity) {
        if min > max {
            return Err(ServerlessV2Rejection::Value(
                SERVERLESS_V2_FLOOR_ABOVE_CEILING_MESSAGE.to_string(),
            ));
        }
    }
    if let Some(seconds) = seconds_until_auto_pause {
        if !SECONDS_UNTIL_AUTO_PAUSE_RANGE.contains(&seconds) {
            return Err(ServerlessV2Rejection::Value(format!(
                "invalid value {seconds} for \
                 ServerlessV2ScalingConfiguration.SecondsUntilAutoPause: must be a value from \
                 {} to {} seconds",
                SECONDS_UNTIL_AUTO_PAUSE_RANGE.start(),
                SECONDS_UNTIL_AUTO_PAUSE_RANGE.end()
            )));
        }
    }
    Ok(())
}

/// How this service words the refusal of a member whose value has to be an ARN and is not
/// one: `arn` is the value exactly as the request submitted it and `parameter` the member it
/// arrived in, as Amazon RDS names that member in this sentence.
///
/// A member documented as "the Amazon Resource Name (ARN) of …" carries a value with a
/// GRAMMAR, so a value outside it names no resource of any Region or account and is a bad
/// REQUEST — the Query API's `InvalidParameterValue`, answered before any lookup — rather
/// than a resource that happens to be missing. The distinction is client-visible: a
/// validation refusal is permanent and caller-side, a not-found is neither.
///
/// One wording for every such member of this front door, so no two operations can drift on
/// it.
pub fn invalid_arn_for_parameter_message(arn: &str, parameter: &str) -> String {
    format!("The provided ARN ({arn}) is invalid for this parameter ({parameter}).")
}

/// How this service words the refusal of a member whose value is a well-formed ARN of a
/// resource in ANOTHER Region than the one the endpoint serving the request serves.
///
/// Amazon RDS is regional: an endpoint serves the resources of its own Region only, so an
/// ARN whose Region field names another one names no resource this endpoint could ever act
/// on, whatever it holds — which is a bad REQUEST rather than a resource that happens to be
/// missing, and is answered before any lookup. The sentence continues the one every unusable
/// ARN value is refused with ([`invalid_arn_for_parameter_message`]) and then states both
/// Regions, because the pair is what the caller has to reconcile: `expected` is the Region
/// the endpoint serves and `actual` the Region read out of the value.
pub fn arn_region_mismatch_message(
    arn: &str,
    parameter: &str,
    expected: &str,
    actual: &str,
) -> String {
    format!(
        "{} Expected region = {expected}, actual region = {actual}",
        invalid_arn_for_parameter_message(arn, parameter)
    )
}

/// Whether the ARN `value` arrived in `parameter` with names the Region the endpoint serving
/// this request serves, which is the only Region a regional endpoint can act in.
///
/// `Err` carries the sentence the refusal answers with
/// ([`arn_region_mismatch_message`]), so a handler lifts it into its own
/// `InvalidParameterValue` channel BEFORE any lookup — the same reading this front door
/// holds every member that must be an ARN to ([`check_arn_parameter`]). A value written in
/// no ARN form at all carries no Region field to compare and is left to the member's own
/// refusal of an unusable value ([`check_arn_parameter`], the resource-type readings); whether
/// the resource the ARN names EXISTS is the lookup's question, and a same-Region ARN naming
/// none is that operation's not-found fault.
pub fn check_arn_parameter_region(
    parameter: &str,
    value: &str,
    region: &str,
) -> Result<(), String> {
    let Some(arn_region) = arn_region(value) else {
        return Ok(());
    };
    match arn_region == region {
        true => Ok(()),
        false => Err(arn_region_mismatch_message(
            value, parameter, region, arn_region,
        )),
    }
}

/// Whether `value` is an ARN of THIS service at all, which is what a member documented as
/// one has to carry: the fields an Amazon Resource Name of this service is written in — this
/// front door's one reading of that form ([`parse_rds_resource_arn`]).
///
/// `Err` carries the sentence the refusal answers with
/// ([`invalid_arn_for_parameter_message`]), so a handler lifts it into its own
/// `InvalidParameterValue` channel and nothing re-words it. Whether the resource the ARN
/// names EXISTS is the lookup's question, not this one's: a well-formed ARN of this service
/// that names no stored resource is that operation's not-found fault.
///
/// A member whose value must name ONE KIND of resource is checked against that kind too, by
/// the reading that owns the ARN form in question
/// ([`check_automated_backups_arn`]).
pub fn check_arn_parameter(parameter: &str, value: &str) -> Result<(), String> {
    match parse_rds_resource_arn(value) {
        Some(_) => Ok(()),
        None => Err(invalid_arn_for_parameter_message(value, parameter)),
    }
}

/// How Amazon RDS names the `DBInstanceAutomatedBackupsArn` member in the sentence above.
///
/// The modelled member is spelled `DBInstanceAutomatedBackupsArn`; the service's own
/// validation names it `DbInstanceAutomatedBackupsArn`, and a caller matching the message
/// reads what the service sends rather than the model's spelling of the same member.
pub const AUTOMATED_BACKUPS_ARN_PARAMETER: &str = "DbInstanceAutomatedBackupsArn";

/// Whether an `Engine` a RESTORE asks for may run the data of a snapshot taken of `source`.
///
/// The restores that take an `Engine` state the one constraint on it — "Must be compatible
/// with the engine of the source. For example, you can restore a MariaDB 10.1 DB instance
/// from a MySQL 5.6 snapshot" — so what the member admits is the source's own engine and the
/// engines that ARE that engine with different management around them: the MySQL-compatible
/// engines are one such family and the PostgreSQL-compatible ones another, which is this
/// front door's one reading of "the same engine"
/// (`crate::ops::describe_engine_default_parameters::engines_of_one_family`, the reading its
/// published parameter vocabularies are grouped by).
///
/// Every other pair is a different database: its storage format, its catalogs and its wire
/// are another engine's, so the restored instance could not serve the snapshot's data at all
/// — which is why the constraint is checked BEFORE anything is created
/// ([`restore_engine_incompatible_message`]).
pub fn restore_engine_is_compatible(engine: &str, source: &str) -> bool {
    engine.eq_ignore_ascii_case(source)
        || crate::ops::describe_engine_default_parameters::engines_of_one_family(engine, source)
}

/// How this service words the refusal of a restore whose requested `engine` cannot run the
/// data of a snapshot taken of `source`; see [`restore_engine_is_compatible`].
///
/// The sentence states the constraint the member documents and names both engines — the one
/// the request asked for and the one the snapshot was taken of, read from the stored snapshot
/// — because those are the two values the caller has to reconcile.
pub fn restore_engine_incompatible_message(engine: &str, source: &str) -> String {
    format!(
        "The engine {engine} isn't compatible with the engine of the source DB snapshot \
         ({source})."
    )
}

/// How `DescribeDBInstanceAutomatedBackups` words `DBInstanceAutomatedBackupNotFoundFault`:
/// a statement about the automated backup of ONE DB instance, with the value the request
/// named that instance by as its only variable part.
///
/// The model carries no message template for the fault, so each of the two operations that
/// answer it is worded as Amazon RDS is recorded answering THAT operation, which is not one
/// sentence: the report speaks of the `DBInstanceAutomatedBackup` of a `DBInstance`
/// (here), and the delete speaks of the backups themselves
/// ([`delete_automated_backup_not_found_message`]). A caller that matches on either reads
/// what the service sends for the call it made.
pub fn describe_automated_backup_not_found_message(named: &str) -> String {
    format!("DBInstanceAutomatedBackup for DBInstance {named} not found.")
}

/// How `DeleteDBInstanceAutomatedBackup` words the same fault; see
/// [`describe_automated_backup_not_found_message`] for why the two differ.
pub fn delete_automated_backup_not_found_message(named: &str) -> String {
    format!("Automated backups could not be found for: {named}")
}

/// How every front door that RESOLVES an `OptionGroupName` words the not-found fault: the
/// member, then the value it could not resolve.
///
/// One sentence for every operation that names an option group — the calls whose subject IS
/// the group and the DB instance calls that place one in it — so a caller reads the same
/// answer wherever the name went unresolved.
pub fn option_group_not_found_message(named: &str) -> String {
    format!("Specified OptionGroupName: {named} not found.")
}

/// What a request that names `DBSecurityGroups` for a DB instance placed in a VPC is
/// refused with, as Amazon RDS words it.
///
/// What a request that names a `DBSecurityGroups` entry for a DB instance is refused with.
///
/// A DB security group is the EC2-Classic association the member was introduced for, and
/// every DB instance this service creates is placed in a DB subnet group — the one the
/// request named, its DB cluster's, or the account's `default` group — so every one of them
/// is a VPC DB instance, and the API versions that carry the association to such an instance
/// are long past.
///
/// Fixed text, naming neither the group nor the instance: like the sibling refusal of a
/// CHANGE to such a group's rules ([`VPC_DB_SECURITY_GROUP_NOT_MODIFIABLE`]) it is a
/// statement about which API versions carry the association at all, which is why it holds
/// whether or not the account has a group of that name.
pub const DB_SECURITY_GROUPS_NOT_SUPPORTED_IN_VPC_MESSAGE: &str =
    "DB Security Groups can only be associated with VPC DB Instances using API versions \
     2012-01-15 through 2012-04-23.";

/// Whether the Availability Zone `zone` is one the DB subnet group `group` covers.
///
/// A DB instance is placed in a SUBNET of the group it is placed in, and a subnet lies in
/// exactly one Availability Zone, so the zone a request asks for has to be one of the zones
/// the group's own subnets lie in ([`DbSubnetGroupRecord::subnet_zones`], read off the
/// records the service that owns subnets answered with).
///
/// A group this service knows NO zone for — one whose subnets EC2 could not be asked about —
/// is evidence of nothing: `true`, so no zone is refused on silence.
pub fn db_subnet_group_covers_zone(group: &DbSubnetGroupRecord, zone: &str) -> bool {
    let covered = db_subnet_group_zones(group);
    covered.is_empty() || covered.iter().any(|known| known.eq_ignore_ascii_case(zone))
}

/// The Availability Zones a DB subnet group COVERS, in zone order: the distinct zones of the
/// subnets it holds, as the service that owns subnets reported them.
pub fn db_subnet_group_zones(group: &DbSubnetGroupRecord) -> BTreeSet<String> {
    group.subnet_zones.values().cloned().collect()
}

/// What a request that asks for an Availability Zone the DB subnet group it places the
/// instance in covers no subnet in is refused with.
///
/// Amazon RDS words it as a statement about the DB instance that cannot be created: the class
/// it would have run, the zone the request asked for — which the group holds no subnet in, so
/// there is no address in it to give the instance — and the two ways out, a subnet in that
/// zone or a request that leaves the zone to the service.
pub fn availability_zone_not_in_db_subnet_group_message(
    db_instance_class: &str,
    zone: &str,
) -> String {
    format!(
        "You can't create a {db_instance_class} database instance because there are no \
         subnets with available IP addresses in the specified Availability Zone {zone}. \
         Create a new subnet in {zone} and target that Availability Zone again, or retry \
         the launch with no availability zone specified."
    )
}

/// What a delete of a parameter group that is still applied to a database is refused with:
/// "can't be associated with any DB instances" for a DB parameter group, and the same of a DB
/// cluster for a DB cluster parameter group.
///
/// ONE sentence for both levels, with the group's name as its only variable part: Amazon RDS
/// words the state fault of a parameter group still in use the same way whichever kind of
/// database holds it, so the two deletes cannot drift apart.
pub fn parameter_group_in_use_message(name: &str) -> String {
    format!(
        "One or more database instances are still members of this parameter group {name}, \
         so the group cannot be deleted"
    )
}

/// The engines `ModifyDBInstance$Engine` names, and the engine each one converts an instance
/// FROM: "the target Oracle DB engine when you convert a non-CDB to a CDB", where "you
/// specify `oracle-ee-cdb` or `oracle-se2-cdb`" — each being the container-database form of
/// one Oracle edition.
const CDB_CONVERSION_ENGINES: &[(&str, &str)] =
    &[("oracle-ee-cdb", "oracle-ee"), ("oracle-se2-cdb", "oracle-se2")];

/// Whether an `Engine` a modification names is a change this operation can make to an
/// instance currently running `current_engine`.
///
/// `ModifyDBInstance$Engine` exists for ONE change — converting an Oracle non-CDB instance to
/// a container database — so the only values it admits are the CDB form of the edition the
/// instance already runs, and the engine the instance runs itself (a request that changes
/// nothing). Every other value asks for a database of another engine over the storage of this
/// one, which no modification can do: the instance would have to be created again, and the
/// member documents no such move.
pub fn engine_change_is_supported(current_engine: &str, requested: &str) -> bool {
    if requested.eq_ignore_ascii_case(current_engine) {
        return true;
    }
    CDB_CONVERSION_ENGINES.iter().any(|(cdb, non_cdb)| {
        requested.eq_ignore_ascii_case(cdb) && current_engine.eq_ignore_ascii_case(non_cdb)
    })
}

/// How this service words the refusal of an `Engine` a modification cannot make; see
/// [`engine_change_is_supported`].
///
/// Amazon RDS words it as a statement about the MOVE the request asked for, naming the engine
/// the instance runs and the engine it was asked to run.
pub fn engine_change_message(current_engine: &str, requested: &str) -> String {
    format!(
        "Modification of database from engine {current_engine} to engine {requested} is not \
         supported."
    )
}

/// What `CreateDBShardGroup` refuses a primary DB cluster whose ENGINE VERSION does not run
/// Aurora Limitless Database with.
///
/// A DB shard group holds the shards of a Limitless database, and the Limitless attributes a
/// shard group is made of are offered on the engine versions AWS ships them on
/// (`SupportsLimitlessDatabase`, which the engine-version report publishes per build) — so a
/// cluster on any other build has nothing to provision them into. Amazon RDS words it as a
/// statement about the REQUEST's attributes, naming the version the cluster runs by the
/// description the engine-version report gives it and what to do about it.
pub fn shard_group_engine_version_message(engine_version_description: &str) -> String {
    format!(
        "The request includes Aurora Limitless Database attributes, which aren't available \
         for the engine version of your DB cluster ({engine_version_description}). Upgrade \
         to an engine version that's compatible with Aurora Limitless Database."
    )
}

/// What `CreateDBShardGroup` refuses a primary DB cluster that is not an Aurora Limitless
/// Database cluster with.
///
/// A DB shard group holds the shards of a Limitless database, and a cluster runs one only
/// "when [`ClusterScalabilityType`] is set to `limitless`"
/// ([`DbClusterRecord::runs_limitless_database`]) — so a cluster of any other scalability
/// type has no Limitless database for a shard group to be provisioned into.
pub fn shard_group_requires_limitless_cluster_message(db_cluster_identifier: &str) -> String {
    format!(
        "The DB cluster {db_cluster_identifier} isn't an Aurora Limitless Database cluster. \
         Create the DB cluster with ClusterScalabilityType set to \
         {CLUSTER_SCALABILITY_TYPE_LIMITLESS} to create DB shard groups in it."
    )
}
