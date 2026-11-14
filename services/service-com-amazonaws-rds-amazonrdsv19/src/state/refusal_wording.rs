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
}

impl ClusterKind {
    /// How the kind is NAMED in a refusal, as the operations' own documentation names it.
    fn described(self) -> &'static str {
        match self {
            Self::Aurora => "an Aurora DB cluster",
            Self::MultiAz => "a non-Aurora Multi-AZ DB cluster",
        }
    }

    /// Whether a cluster running `engine` is of this kind.
    pub fn holds_for(self, engine: &str) -> bool {
        match self {
            Self::Aurora => !is_multi_az_db_cluster(engine),
            Self::MultiAz => is_multi_az_db_cluster(engine),
        }
    }
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

/// The sentence `ModifyDBInstance` answers an `InvalidDBInstanceStateFault` with for an
/// instance in `status`.
///
/// Amazon RDS words the refusal by whether the status is one the instance will LEAVE or one
/// it cannot: for a transient status it states the state a modification needs
/// ([`MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE`]), which is what a caller polls on, and for
/// a terminal one ([`TERMINAL_DB_INSTANCE_STATUSES`]) it names the state the instance is in
/// — taken from the instance's own record — and that waiting for it to change is futile.
pub fn modify_db_instance_invalid_state_message(status: &str) -> String {
    if is_terminal_db_instance_status(status) {
        return format!(
            "The specified database instance is currently in {status} state, which is a \
             terminal state."
        );
    }
    MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE.to_string()
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
/// identifier nor anything else of the request or the store is substituted here.
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

/// How [`deletion_protection_refusal_message`] names a global cluster.
pub const DELETION_PROTECTED_GLOBAL_CLUSTER: &str = "Global Cluster";

/// What a delete that names BOTH of its final-snapshot members is refused with.
///
/// Specifying `FinalDBSnapshotIdentifier` "and also skip the creation of a final DB
/// snapshot with the SkipFinalShapshot parameter" "results in an error": exactly one of the
/// two may be given, which is what the sentence states, naming both members and neither
/// value. One constant for every delete that carries the pair, for the same reason as
/// [`FINAL_SNAPSHOT_MEMBER_REQUIRED`], which answers the other direction of the rule.
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
pub fn serverless_v2_capacity_out_of_range_message(value: f64, min: f64, max: f64) -> String {
    format!(
        "Serverless v2 capacity value {value:?} is not valid. The valid scaling range for \
         this cluster is {min:?} to {max:?}. To change the cluster's scaling range, use the \
         ModifyDBCluster call."
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
/// A capacity value is checked against that range
/// ([`serverless_v2_capacity_out_of_range_message`]) and against the half-step
/// increments the member documents ("You can specify ACU values in half-step increments,
/// such as 8, 8.5, 9, and so on"), and the pair against each other (the floor cannot exceed
/// the ceiling).
pub fn check_serverless_v2_scaling(
    capacity: Option<(f64, f64)>,
    min_capacity: Option<f64>,
    max_capacity: Option<f64>,
    seconds_until_auto_pause: Option<i32>,
) -> Result<(), ServerlessV2Rejection> {
    let (min_acu, max_acu) = capacity.unwrap_or(SERVERLESS_V2_PUBLISHED_CAPACITY);
    for value in [min_capacity, max_capacity] {
        let Some(value) = value else { continue };
        if !(min_acu..=max_acu).contains(&value) {
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
