// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-docdbelastic-chimeradblionfishservicelambda service.
//!
//! The elastic-cluster lifecycle is stateful: `CreateCluster` mints a record here,
//! every reader renders its response from the record, and `DeleteCluster` removes it.
//! The record types below are the persisted form; [`ClusterRecord::as_cluster`] and
//! [`SnapshotRecord::as_snapshot`] project them onto the modelled wire shapes, which is
//! the only place a response body is built from.

use crate::ops::types::{
    Cluster, ClusterInList, ClusterSnapshot, ClusterSnapshotInList, ConflictException,
    PendingMaintenanceActionDetails, ResourceNotFoundException, ResourcePendingMaintenanceAction,
    ServiceQuotaExceededException, Shard, ValidationException, SNAPSHOT_TYPE_VALUES,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

/// How long after the call that starts it a cluster (or snapshot) status transition the
/// clock owes becomes due.
///
/// The emulator never sleeps for it: the due instant is stored on the record and
/// materialised by whoever next looks at that record. The span is deliberately
/// non-zero, so a caller acting straight after `CreateCluster` still observes
/// `CREATING` and the state errors that go with it.
const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// Per-service state for com-amazonaws-docdbelastic-chimeradblionfishservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState {
    /// Every live elastic cluster, keyed by its `clusterArn`. `BTreeMap` so
    /// `ListClusters` pages in a stable, total order without sorting on every call.
    pub clusters: BTreeMap<String, ClusterRecord>,
    /// `clusterName` -> `clusterArn`, so a duplicate name is refused with
    /// `ConflictException` instead of minting a second cluster with the same name.
    pub cluster_names: BTreeMap<String, String>,
    /// Every cluster snapshot, keyed by its `snapshotArn`.
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    /// `snapshotName` -> `snapshotArn`, so a duplicate snapshot name is refused
    /// without a linear scan of `snapshots`.
    pub snapshot_names: BTreeMap<String, String>,
}

impl ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState {
    /// The tags persisted on the resource `arn` names, whichever kind of taggable
    /// resource this service owns it is, or `None` when this store holds no resource
    /// under that ARN.
    ///
    /// A tagging call is addressed by ARN alone, so the lookup spans every taggable
    /// collection rather than one kind's: the ARN is each record's own key, so at most
    /// one collection can hold it.
    pub fn tags_of(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        self.clusters
            .get(arn)
            .map(|cluster| &cluster.tags)
            .or_else(|| self.snapshots.get(arn).map(|snapshot| &snapshot.tags))
    }

    /// The same lookup as [`tags_of`](Self::tags_of), for the tagging operations that
    /// WRITE the map: the tags live on the resource's own record, so adding and
    /// removing them is an edit of that record and never of a map beside it.
    pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(cluster) = self.clusters.get_mut(arn) {
            return Some(&mut cluster.tags);
        }
        self.snapshots.get_mut(arn).map(|snapshot| &mut snapshot.tags)
    }
}

/// One persisted elastic cluster.
///
/// `admin_user_password` is held because `UpdateCluster` can change it, and is NEVER
/// projected onto a response: the modelled `Cluster` shape has no such member, and
/// [`as_cluster`](ClusterRecord::as_cluster) is the only projection. `Debug` is written
/// by hand rather than derived so the secret cannot reach a log or a panic message
/// either — the model marks it `@sensitive`.
#[derive(Clone)]
pub struct ClusterRecord {
    pub cluster_name: String,
    pub cluster_arn: String,
    pub status: String,
    pub cluster_endpoint: String,
    pub create_time: String,
    pub admin_user_name: String,
    pub admin_user_password: String,
    pub auth_type: String,
    pub shard_capacity: i32,
    pub shard_count: i32,
    pub shard_instance_count: Option<i32>,
    pub vpc_security_group_ids: Vec<String>,
    pub subnet_ids: Vec<String>,
    pub preferred_maintenance_window: String,
    pub preferred_backup_window: Option<String>,
    pub backup_retention_period: Option<i32>,
    pub kms_key_id: String,
    pub shards: Vec<ShardRecord>,
    pub tags: BTreeMap<String, String>,
    /// The maintenance actions that are pending for this cluster, as
    /// `GetPendingMaintenanceAction` reports them and `ApplyPendingMaintenanceAction`
    /// opts in to them.
    ///
    /// A pending maintenance action exists because the SERVICE scheduled an engine, OS
    /// or security update for the cluster — no request creates one — so a cluster this
    /// emulator mints starts with nothing pending, exactly as a cluster AWS has just
    /// created does. Both maintenance operations read the list from here rather than
    /// assuming what it holds.
    pub pending_maintenance_actions: Vec<PendingMaintenanceActionDetails>,
    /// When the clock owes this cluster a move to `ACTIVE`; `None` once it is settled.
    pub settles_at: Option<Instant>,
}

/// One shard of a persisted cluster.
#[derive(Debug, Clone)]
pub struct ShardRecord {
    pub shard_id: String,
    pub create_time: String,
    pub status: String,
}

/// The source cluster's configuration a snapshot carries so a restore can rebuild the
/// cluster from it.
///
/// These are the members `RestoreClusterFromSnapshot` needs that the modelled
/// `ClusterSnapshot` shape does NOT report on the wire; the ones it does report
/// (`adminUserName`, `kmsKeyId`, `subnetIds`, `vpcSecurityGroupIds`) stay on
/// [`SnapshotRecord`] itself and are not duplicated here. Holding them on the snapshot
/// is what lets a restore rebuild the cluster after the source cluster is gone.
///
/// `admin_user_password` is held for the same reason [`ClusterRecord`] holds it — a
/// restored cluster keeps the source's administrator credentials — and `Debug` is
/// written by hand so the `@sensitive` value cannot reach a log or a panic message.
#[derive(Clone)]
pub struct SourceClusterConfig {
    pub admin_user_password: String,
    pub auth_type: String,
    pub shard_capacity: i32,
    pub shard_count: i32,
    pub shard_instance_count: Option<i32>,
    pub preferred_maintenance_window: String,
    pub preferred_backup_window: Option<String>,
    pub backup_retention_period: Option<i32>,
}

impl ::std::fmt::Debug for SourceClusterConfig {
    /// Everything except the administrator password, which is `@sensitive` and so is
    /// rendered as a fixed placeholder.
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("SourceClusterConfig")
            .field("admin_user_password", &"<redacted>")
            .field("auth_type", &self.auth_type)
            .field("shard_capacity", &self.shard_capacity)
            .field("shard_count", &self.shard_count)
            .field("shard_instance_count", &self.shard_instance_count)
            .field(
                "preferred_maintenance_window",
                &self.preferred_maintenance_window,
            )
            .field("preferred_backup_window", &self.preferred_backup_window)
            .field("backup_retention_period", &self.backup_retention_period)
            .finish()
    }
}

/// One persisted cluster snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    pub snapshot_name: String,
    pub snapshot_arn: String,
    pub cluster_arn: String,
    pub snapshot_creation_time: String,
    pub cluster_creation_time: String,
    pub status: String,
    pub snapshot_type: String,
    pub admin_user_name: String,
    pub kms_key_id: String,
    pub subnet_ids: Vec<String>,
    pub vpc_security_group_ids: Vec<String>,
    pub tags: BTreeMap<String, String>,
    /// The rest of the source cluster's configuration, captured when the snapshot was
    /// taken, so `RestoreClusterFromSnapshot` can rebuild a cluster from it.
    pub source_config: SourceClusterConfig,
    /// When the clock owes this snapshot a move to `ACTIVE`; `None` once settled.
    pub settles_at: Option<Instant>,
}

impl ::std::fmt::Debug for ClusterRecord {
    /// Everything except the administrator password, which is `@sensitive` and so is
    /// rendered as a fixed placeholder.
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("ClusterRecord")
            .field("cluster_name", &self.cluster_name)
            .field("cluster_arn", &self.cluster_arn)
            .field("status", &self.status)
            .field("cluster_endpoint", &self.cluster_endpoint)
            .field("create_time", &self.create_time)
            .field("admin_user_name", &self.admin_user_name)
            .field("admin_user_password", &"<redacted>")
            .field("auth_type", &self.auth_type)
            .field("shard_capacity", &self.shard_capacity)
            .field("shard_count", &self.shard_count)
            .field("shard_instance_count", &self.shard_instance_count)
            .field("vpc_security_group_ids", &self.vpc_security_group_ids)
            .field("subnet_ids", &self.subnet_ids)
            .field(
                "preferred_maintenance_window",
                &self.preferred_maintenance_window,
            )
            .field("preferred_backup_window", &self.preferred_backup_window)
            .field("backup_retention_period", &self.backup_retention_period)
            .field("kms_key_id", &self.kms_key_id)
            .field("shards", &self.shards)
            .field("tags", &self.tags)
            .field(
                "pending_maintenance_actions",
                &self.pending_maintenance_actions,
            )
            .field("settles_at", &self.settles_at)
            .finish()
    }
}

impl ClusterRecord {
    /// Start a status transition the clock owes: the transient `status` now, and the
    /// settled status that transient one leads to ([`settled_status`]) once
    /// [`SETTLE_AFTER`] has elapsed.
    pub fn begin_transition(&mut self, status: &str) {
        self.status = status.to_string();
        self.settles_at = Some(settle_instant());
        for shard in &mut self.shards {
            shard.status = status.to_string();
        }
    }

    /// Materialise the transition the clock has already made due. Reporting
    /// operations call this: it only completes work that was owed at this instant
    /// whether or not anybody called.
    ///
    /// Which settled status is owed follows from the transient one the cluster is in
    /// ([`settled_status`]), so a cluster that is being stopped settles stopped and one
    /// that is being built, changed or restarted settles available.
    pub fn settle_due(&mut self) {
        if self.settles_at.is_some_and(|due| Instant::now() >= due) {
            let settled = settled_status(&self.status);
            self.settles_at = None;
            self.status = settled.to_string();
            for shard in &mut self.shards {
                shard.status = settled.to_string();
            }
        }
    }

    /// Whether this cluster is in its settled available status, so a call that acts
    /// only from that status may act on it.
    ///
    /// The one predicate both status refusals ask ([`ensure_modifiable`] and
    /// [`ensure_available`]), so the two cannot disagree about which cluster is settled
    /// while answering the refusal each of their operations models. Ask it after
    /// [`settle_due`](Self::settle_due), so the answer is about the status the clock owes
    /// the cluster rather than the one it was left in.
    pub fn is_settled_available(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    /// The modelled `Cluster` for this record. Every `@required` member is populated.
    pub fn as_cluster(&self) -> Cluster {
        Cluster {
            cluster_name: self.cluster_name.clone(),
            cluster_arn: self.cluster_arn.clone(),
            status: self.status.clone(),
            cluster_endpoint: self.cluster_endpoint.clone(),
            create_time: self.create_time.clone(),
            admin_user_name: self.admin_user_name.clone(),
            auth_type: self.auth_type.clone(),
            shard_capacity: self.shard_capacity,
            shard_count: self.shard_count,
            vpc_security_group_ids: self.vpc_security_group_ids.clone(),
            subnet_ids: self.subnet_ids.clone(),
            preferred_maintenance_window: self.preferred_maintenance_window.clone(),
            kms_key_id: self.kms_key_id.clone(),
            shards: Some(
                self.shards
                    .iter()
                    .map(|shard| Shard {
                        shard_id: shard.shard_id.clone(),
                        create_time: shard.create_time.clone(),
                        status: shard.status.clone(),
                    })
                    .collect(),
            ),
            backup_retention_period: self.backup_retention_period,
            preferred_backup_window: self.preferred_backup_window.clone(),
            shard_instance_count: self.shard_instance_count,
        }
    }

    /// The summary form `ListClusters` returns.
    pub fn as_cluster_in_list(&self) -> ClusterInList {
        ClusterInList {
            cluster_name: self.cluster_name.clone(),
            cluster_arn: self.cluster_arn.clone(),
            status: self.status.clone(),
        }
    }

    /// The modelled `ResourcePendingMaintenanceAction` for this cluster: the ARN the
    /// actions belong to, and every action currently pending for it.
    ///
    /// The details list is always present, and empty for a cluster with nothing
    /// pending, which is how the service reports "this resource has no pending
    /// maintenance action" — not as a missing member and not as a not-found refusal.
    pub fn as_resource_pending_maintenance_action(&self) -> ResourcePendingMaintenanceAction {
        ResourcePendingMaintenanceAction {
            resource_arn: Some(self.cluster_arn.clone()),
            pending_maintenance_action_details: Some(self.pending_maintenance_actions.clone()),
        }
    }

    /// The configuration a snapshot of this cluster captures, so a restore of that
    /// snapshot rebuilds the cluster even after this record is gone.
    pub fn source_config(&self) -> SourceClusterConfig {
        SourceClusterConfig {
            admin_user_password: self.admin_user_password.clone(),
            auth_type: self.auth_type.clone(),
            shard_capacity: self.shard_capacity,
            shard_count: self.shard_count,
            shard_instance_count: self.shard_instance_count,
            preferred_maintenance_window: self.preferred_maintenance_window.clone(),
            preferred_backup_window: self.preferred_backup_window.clone(),
            backup_retention_period: self.backup_retention_period,
        }
    }
}

impl SnapshotRecord {
    /// Materialise the transition the clock has already made due; see
    /// [`ClusterRecord::settle_due`].
    pub fn settle_due(&mut self) {
        if self.settles_at.is_some_and(|due| Instant::now() >= due) {
            self.settles_at = None;
            self.status = STATUS_ACTIVE.to_string();
        }
    }

    /// The modelled `ClusterSnapshot` for this record.
    pub fn as_snapshot(&self) -> ClusterSnapshot {
        ClusterSnapshot {
            subnet_ids: self.subnet_ids.clone(),
            snapshot_name: self.snapshot_name.clone(),
            snapshot_arn: self.snapshot_arn.clone(),
            snapshot_creation_time: self.snapshot_creation_time.clone(),
            cluster_arn: self.cluster_arn.clone(),
            cluster_creation_time: self.cluster_creation_time.clone(),
            status: self.status.clone(),
            vpc_security_group_ids: self.vpc_security_group_ids.clone(),
            admin_user_name: self.admin_user_name.clone(),
            kms_key_id: self.kms_key_id.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
        }
    }

    /// The summary form `ListClusterSnapshots` returns. Every member of
    /// `ClusterSnapshotInList` is `@required`, so all five are populated from the
    /// record.
    pub fn as_snapshot_in_list(&self) -> ClusterSnapshotInList {
        ClusterSnapshotInList {
            snapshot_name: self.snapshot_name.clone(),
            snapshot_arn: self.snapshot_arn.clone(),
            cluster_arn: self.cluster_arn.clone(),
            status: self.status.clone(),
            snapshot_creation_time: self.snapshot_creation_time.clone(),
        }
    }
}

/// `Status` values this crate names. The full value set is `AUTH_VALUES`' sibling
/// `STATUS_VALUES` in `ops::types`; these are the ones the cluster and snapshot
/// lifecycles use.
pub const STATUS_CREATING: &str = "CREATING";
pub const STATUS_ACTIVE: &str = "ACTIVE";
pub const STATUS_UPDATING: &str = "UPDATING";
pub const STATUS_DELETING: &str = "DELETING";
pub const STATUS_STOPPING: &str = "STOPPING";
pub const STATUS_STOPPED: &str = "STOPPED";
pub const STATUS_STARTING: &str = "STARTING";

/// The status a snapshot being copied from another snapshot carries until the clock
/// owes it `ACTIVE`; `CopyClusterSnapshot` is the only operation that starts it.
pub const STATUS_COPYING: &str = "COPYING";

/// The settled status the clock owes a cluster that is now in the transient status
/// `transient`.
///
/// A transient status names work the service is carrying out, and the work decides
/// where it ends: stopping a cluster ends with it stopped, a cluster being deleted is
/// going away and so is owed no other status, and every other transient status this
/// emulator starts — building, changing or restarting a cluster — ends with it
/// available. All of them are modelled `Status` values (`ops::types::STATUS_VALUES`).
pub fn settled_status(transient: &str) -> &'static str {
    match transient {
        STATUS_STOPPING => STATUS_STOPPED,
        STATUS_DELETING => STATUS_DELETING,
        _ => STATUS_ACTIVE,
    }
}

/// The `OptInType` values the maintenance-action opt-in path names.
///
/// Source: `smithy_query(query="shape", shape="OptInType")` reports the wire values as
/// `IMMEDIATE` / `NEXT_MAINTENANCE` / `APPLY_ON` / `UNDO_OPT_IN`, which is also the
/// scaffold's own `ops::types::OPT_IN_TYPE_VALUES`.
pub const OPT_IN_APPLY_ON: &str = "APPLY_ON";
pub const OPT_IN_IMMEDIATE: &str = "IMMEDIATE";
pub const OPT_IN_NEXT_MAINTENANCE: &str = "NEXT_MAINTENANCE";
pub const OPT_IN_UNDO_OPT_IN: &str = "UNDO_OPT_IN";

/// The `SnapshotType` a caller-requested snapshot carries (`CreateClusterSnapshot`);
/// AWS reserves `AUTOMATED` for the ones it takes on the backup window.
///
/// The spelling is the model's: `smithy_query(query="shape", shape="SnapshotType")`
/// reports the wire values as `MANUAL` / `AUTOMATED`, and the scaffold's own
/// `ops::types::SNAPSHOT_TYPE_VALUES` ("the values `SnapshotType` accepts on the
/// wire") agrees. The lowercase `manual` / `automated` in the member's prose is how
/// the documentation writes them in a sentence, not a second wire spelling.
pub const SNAPSHOT_TYPE_MANUAL: &str = "MANUAL";

/// The signing / ARN namespace for this service, as `OPERATIONS.md` records it.
pub const ARN_SERVICE: &str = "docdb-elastic";

/// The region this request is scoped to, read from its SigV4 credential scope.
///
/// A request that carries no parseable scope (an unsigned call to an emulator with
/// authentication off) is treated as `us-east-1`, the emulator-wide default.
pub fn request_region(parts: &::smithy_server_runtime::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// The instant a transition started now becomes due.
pub fn settle_instant() -> Instant {
    Instant::now() + SETTLE_AFTER
}

/// Now, as the UTC instant string every `*Time` member of this service carries.
///
/// The modelled members are `String`, not `timestamp`, and documented as "in
/// Universal Coordinated Time (UTC)", so this renders RFC 3339 / ISO 8601 with a `Z`
/// offset through `aws_smithy_types`' own formatter rather than inventing a format.
pub fn now_utc() -> String {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Monotonic counter mixed into every minted id, so two ids minted inside the same
/// clock tick still differ. Only [`new_resource_id`] reads it.
static MINT_COUNTER: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// A fresh server-generated resource id, in the 8-4-4-4-12 lowercase-hex shape an
/// elastic cluster or snapshot id has.
///
/// Uniqueness comes from the monotonic counter, not from the clock, so two ids minted
/// within one tick (or after the clock steps back) are still distinct; the counter is
/// hashed only so a caller cannot read a sequence number out of an id and predict the
/// next one. This mirrors `mint_id` in the pinpoint service's `state.rs`.
pub fn new_resource_id() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::sync::atomic::Ordering;

    let counter = MINT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let digest = Sha256::digest(format!("{counter}:{nanos}").as_bytes());
    // 16 bytes is exactly the 32 hex characters the five groups below consume.
    let rendered = ::hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[0..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..32]
    )
}

/// `arn:aws:docdb-elastic:<region>:<account>:<resource>` for this emulator's account.
pub fn docdb_elastic_arn(region: &str, resource: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, resource)
}

/// The host name a client connects to this cluster on.
///
/// Derived from the cluster's own identity, never from anything the caller passed as
/// an endpoint: `<name>.<cluster id>.<region>.docdb-elastic.amazonaws.com`.
pub fn cluster_endpoint(cluster_name: &str, cluster_id: &str, region: &str) -> String {
    format!("{cluster_name}.{cluster_id}.{region}.{ARN_SERVICE}.amazonaws.com")
}

/// The tag this service stamps on every pagination cursor it issues, so a token it
/// did not issue is rejected rather than read as an arbitrary start key.
const PAGE_TOKEN_TAG: &str = "docdb-elastic/clusters#";

/// Encode a pagination cursor: the key the NEXT page starts at.
///
/// Tagged, then hex-encoded, so the token a caller gets back is opaque, round-trips
/// through a URI query parameter unchanged whatever the key contains, and is
/// distinguishable from any other string a caller might send.
pub fn encode_page_token(next_key: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_TAG}{next_key}").as_bytes())
}

/// Decode a cursor produced by [`encode_page_token`]; `None` when it is not one.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_TAG).map(str::to_string)
}

/// The wording every handler uses when the shared engine does not carry this
/// service's store. It is a server-side fault, so it is answered as the modelled
/// `InternalServerException` (HTTP 500) — never with a freshly built engine.
pub const STORE_UNREACHABLE: &str =
    "the DocDB Elastic store is not reachable from this engine instance";

/// The default `kmsKeyId` a cluster created without one reports.
///
/// `CreateClusterInput$kmsKeyId` documents that "If an encryption key is not
/// specified, Amazon DocumentDB uses the default encryption key that KMS creates for
/// your account"; DocumentDB reports that case as this sentinel rather than as an
/// account-specific key ARN.
pub const AWS_OWNED_KMS_KEY: &str = "AWS_OWNED_KMS_KEY";

/// The days `preferredMaintenanceWindow` may name.
///
/// Source: `CreateClusterInput$preferredMaintenanceWindow` doc — "Valid days: Mon,
/// Tue, Wed, Thu, Fri, Sat, Sun".
pub const MAINTENANCE_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// The inclusive bounds `shardCapacity` lies within; inside them it must be a power
/// of two.
///
/// Source: `CreateClusterInput$shardCapacity` doc — "Maximum is 64. Allowed values
/// are 2, 4, 8, 16, 32, 64", which is exactly the powers of two from 2 to 64, and the
/// form the service's own refusal of the member states the rule in.
pub const SHARD_CAPACITY_RANGE: (i32, i32) = (2, 64);

/// The largest `shardCount` a cluster may have.
///
/// Source: `CreateClusterInput$shardCount` doc — "Maximum is 32".
pub const SHARD_COUNT_MAX: i32 = 32;

/// The inclusive length bounds of `adminUserPassword`, and the characters it may not
/// contain.
///
/// Source: `CreateClusterInput$adminUserPassword` doc — "Must contain from 8 to 100
/// characters. Cannot contain a forward slash (/), double quote ("), or the "at"
/// symbol (@)".
pub const PASSWORD_LEN: (usize, usize) = (8, 100);
pub const PASSWORD_FORBIDDEN: &[char] = &['/', '"', '@'];

/// The inclusive length bound shared by `clusterName` and `adminUserName`.
///
/// Source: their `CreateClusterInput` docs — "from 1 to 63".
pub const NAME_LEN: (usize, usize) = (1, 63);

/// The page size `ListClusters` uses when the caller names no `maxResults`.
///
/// `ListClustersInput$maxResults` is modelled `range(min: 1, max: 100)`, so the
/// largest page the member can ask for is the natural default.
pub const DEFAULT_PAGE_SIZE: i32 = 100;

// ---------------------------------------------------------------------------
// Resource guards
//
// Every record below is created by a caller and reclaimed only by an explicit
// delete, so each growth site carries a cap: the store lives for the life of the
// process and a caller must not be able to grow it without limit. The record
// COUNTS are emulator resource guards (AWS publishes soft, account-level limits for
// elastic clusters that an emulator cannot meaningfully mirror); the per-request
// value bounds are the service's own documented or modelled limits where it has
// them.
// ---------------------------------------------------------------------------

/// The most elastic clusters this emulator holds at once. Emulator resource guard:
/// `CreateCluster` answers `ServiceQuotaExceededException` past it, which is the
/// error the operation models for a quota, and `DeleteCluster` frees a slot.
pub const MAX_CLUSTERS: usize = 100;

/// The most cluster snapshots this emulator holds at once. Emulator resource guard;
/// `DeleteClusterSnapshot` frees a slot.
pub const MAX_CLUSTER_SNAPSHOTS: usize = 500;

/// The most tags one resource carries.
///
/// Source: `UntagResourceInput$tagKeys` is modelled `length(min: 0, max: 50)`, so no
/// resource can hold more than 50 distinct tag keys.
pub const MAX_TAGS: usize = 50;

/// The inclusive length bounds of a tag key and a tag value (the AWS-wide tag limits).
pub const MAX_TAG_KEY_LEN: usize = 128;
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The most entries `subnetIds` or `vpcSecurityGroupIds` may carry, and the longest
/// one entry may be. Emulator resource guards: the model bounds neither, and both are
/// stored verbatim on the record.
pub const MAX_NETWORK_IDS: usize = 64;
pub const MAX_NETWORK_ID_LEN: usize = 255;

/// The inclusive length bounds of `snapshotName`.
///
/// Source: `CreateClusterSnapshotInput$snapshotName` carries a member-level
/// `length(min: 1, max: 63)` (`smithy_query(query="operation",
/// operation="CreateClusterSnapshot")`). The name is stored verbatim on the record
/// AND as the key of the name index, so it is bounded here too rather than relying on
/// the boundary validator alone.
pub const SNAPSHOT_NAME_LEN: (usize, usize) = (1, 63);

/// The longest `kmsKeyId` this service stores: an ARN, an alias or a key id, all of
/// which fit inside the AWS maximum ARN length.
pub const MAX_KMS_KEY_ID_LEN: usize = 2048;

/// A `ValidationException` (HTTP 400) for a request value this service refuses.
///
/// The service states such a refusal as one sentence: the subject it is about, then
/// the rule that subject broke — `Invalid Input - '<subject>'. <rule>` — and it
/// carries that sentence in `message` and nothing else, which is why
/// `ValidationException` declares `reason` and `fieldList` unsupported (see
/// `ops::types::shared_types`).
///
/// `subject` is the rejected value as the request spelled it, which is what a caller
/// recognises. Where the value cannot be stated back — it is `@sensitive`, it is a
/// list or a map rather than a scalar, or the rule it broke is its own length, so
/// restating it would repeat the oversized input — the subject is the name of the
/// member it arrived on instead. `rule` is a full sentence, ending in a period, that
/// names the member and states what it must be.
pub fn invalid_input(subject: &str, rule: &str) -> ValidationException {
    ValidationException {
        message: format!("Invalid Input - '{subject}'. {rule}"),
    }
}

/// A `ValidationException` (HTTP 400) for a `clusterName` outside the form this
/// service documents for that member.
///
/// The service does NOT state this one in the `Invalid Input - '<subject>'. <rule>`
/// form [`invalid_input`] builds: it words a rejected cluster name as its own
/// sentence, the fixed text `Invalid cluster name - ` followed by the name exactly as
/// the request spelled it and a period, and carries that in `message` and nothing
/// else. The name is the only variable part.
pub fn invalid_cluster_name(value: &str) -> ValidationException {
    ValidationException {
        message: format!("Invalid cluster name - {value}."),
    }
}

/// The member name as this service spells it inside a refusal sentence: the modelled
/// member name with an initial capital (`shardCapacity` -> `ShardCapacity`, the
/// spelling the recorded refusal of that member uses).
fn member_label(member: &str) -> String {
    let mut chars = member.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Refuse a `shardCapacity` outside the documented value domain.
pub fn validate_shard_capacity(member: &str, value: i32) -> Result<(), ValidationException> {
    let (min, max) = SHARD_CAPACITY_RANGE;
    if (min..=max).contains(&value) && (value as u32).is_power_of_two() {
        return Ok(());
    }
    Err(invalid_input(
        &value.to_string(),
        &format!(
            "{} must be a power of 2 within [{min}, {max}].",
            member_label(member)
        ),
    ))
}

/// Refuse a `shardCount` outside 1..=[`SHARD_COUNT_MAX`].
pub fn validate_shard_count(member: &str, value: i32) -> Result<(), ValidationException> {
    if (1..=SHARD_COUNT_MAX).contains(&value) {
        return Ok(());
    }
    Err(invalid_input(
        &value.to_string(),
        &format!(
            "{} should be between 1 and {SHARD_COUNT_MAX}.",
            member_label(member)
        ),
    ))
}

/// Refuse a `shardInstanceCount` below 1. The docs name no upper bound, so none is
/// imposed here.
pub fn validate_shard_instance_count(member: &str, value: i32) -> Result<(), ValidationException> {
    if value >= 1 {
        return Ok(());
    }
    Err(invalid_input(
        &value.to_string(),
        &format!("{} must be at least 1.", member_label(member)),
    ))
}

/// Refuse an `adminUserPassword` outside the documented length or character rules.
///
/// The value is `@sensitive`, so the refusal names the member and never states the
/// password back.
pub fn validate_admin_user_password(
    member: &str,
    value: &str,
) -> Result<(), ValidationException> {
    let (min, max) = PASSWORD_LEN;
    let label = member_label(member);
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid_input(
            member,
            &format!("{label} must contain from {min} to {max} characters."),
        ));
    }
    if value.chars().any(|c| PASSWORD_FORBIDDEN.contains(&c)) {
        return Err(invalid_input(
            member,
            &format!(
                "{label} must not contain a forward slash (/), double quote (\") or the at symbol (@)."
            ),
        ));
    }
    Ok(())
}

/// Refuse a `clusterName` outside the documented form.
///
/// Source: `CreateClusterInput$clusterName` doc — "Must contain from 1 to 63 letters,
/// numbers, or hyphens. The first character must be a letter. Cannot end with a
/// hyphen or contain two consecutive hyphens".
///
/// That is ONE form, so every way of breaking it — the length bound, the character
/// set, the first character, a trailing or a doubled hyphen — is the same refusal,
/// and the service states it in the sentence [`invalid_cluster_name`] builds rather
/// than in the per-member rule sentence the other members' refusals carry.
pub fn validate_cluster_name(value: &str) -> Result<(), ValidationException> {
    let (min, max) = NAME_LEN;
    let well_formed = (min..=max).contains(&value.chars().count())
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && value.starts_with(|c: char| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--");
    if well_formed {
        Ok(())
    } else {
        Err(invalid_cluster_name(value))
    }
}

/// Refuse an `adminUserName` outside the documented form.
pub fn validate_admin_user_name(member: &str, value: &str) -> Result<(), ValidationException> {
    let (min, max) = NAME_LEN;
    let label = member_label(member);
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid_input(
            member,
            &format!("{label} must contain from {min} to {max} letters or numbers."),
        ));
    }
    let well_formed = value.chars().all(|c| c.is_ascii_alphanumeric())
        && value.starts_with(|c: char| c.is_ascii_alphabetic());
    if well_formed {
        Ok(())
    } else {
        Err(invalid_input(
            value,
            &format!("{label} must begin with a letter and contain only letters or numbers."),
        ))
    }
}

/// A kind of resource this service owns, named the way AWS names it.
///
/// Each kind has TWO spellings, and they are not interchangeable:
///
/// * the RESOURCE TYPE NAME, which is the capitalised name this service's API gives the
///   resource class itself — the class each operation reads and writes one of, as its
///   own input and output members name it: a `Cluster` (`GetCluster` answers `cluster`,
///   addressed by `clusterArn`) and a `Snapshot` (`GetClusterSnapshot` answers
///   `snapshot`, addressed by `snapshotArn`; `smithy_query(query="operation",
///   operation="GetCluster,GetClusterSnapshot")`). It is what the error shapes'
///   `resourceType` member carries: "The type of the resource that could not be found"
///   (`ResourceNotFoundException$resourceType`) and "The type of the resource where
///   there was an access conflict" (`ConflictException$resourceType`), both
///   `@required`. It is NOT the operation's own name, nor the name of the Smithy
///   structure the class is projected onto on the wire;
/// * the ARN RESOURCE SEGMENT, which is lowercase and hyphenated — `cluster/<id>`,
///   `cluster-snapshot/<id>`.
///
/// Holding both on one value is what keeps an error body from reporting the ARN
/// segment (or this store's own key) where a client reads a resource type, and keeps
/// the ARN segment spelled in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// An elastic cluster, projected onto the modelled `Cluster` shape.
    Cluster,
    /// An elastic cluster snapshot, projected onto the modelled `ClusterSnapshot` shape.
    ClusterSnapshot,
}

impl ResourceKind {
    /// The AWS resource type NAME, as every `resourceType` member reports it.
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Cluster => "Cluster",
            Self::ClusterSnapshot => "Snapshot",
        }
    }

    /// The resource segment an ARN of this kind carries, ahead of the resource id.
    pub const fn arn_segment(self) -> &'static str {
        match self {
            Self::Cluster => "cluster",
            Self::ClusterSnapshot => "cluster-snapshot",
        }
    }

    /// The sentence the service answers when a resource of this kind cannot be found.
    ///
    /// The WHOLE message is fixed text, one sentence per resource class: the
    /// identifier the request named is reported in `ResourceNotFoundException$resourceId`
    /// and is never spliced into the message, so the refusal a client logs, localizes or
    /// deduplicates on does not vary per request.
    pub const fn not_found_message(self) -> &'static str {
        match self {
            Self::Cluster => "Unable to find the cluster",
            Self::ClusterSnapshot => "Unable to find the snapshot",
        }
    }

    /// The sentence the service answers when the name a create asked for is already
    /// taken by a resource of this kind.
    ///
    /// Like [`not_found_message`](Self::not_found_message) the WHOLE message is fixed
    /// text, one sentence per resource class: neither the name the request asked for nor
    /// the ARN of the resource already holding it is spliced into it — that ARN is
    /// reported in `ConflictException$resourceId` — so the refusal a client matches,
    /// logs or deduplicates on is the same sentence for every colliding request.
    pub const fn duplicate_name_message(self) -> &'static str {
        match self {
            Self::Cluster => "A cluster already exists with given name",
            Self::ClusterSnapshot => "A snapshot already exists with given name",
        }
    }

    /// The ARN of the resource of this kind with id `resource_id`, in `region`.
    pub fn arn(self, region: &str, resource_id: &str) -> String {
        docdb_elastic_arn(region, &format!("{}/{resource_id}", self.arn_segment()))
    }
}

/// The resource field of an ARN — what follows the account field — with any resource
/// id stripped, or `None` when `value` is not shaped like an ARN at all.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and this
/// service spells `<resource>` as `<segment>/<id>`.
fn arn_resource_segment(value: &str) -> Option<&str> {
    let mut fields = value.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    // partition, service, region, account: a tagging call addresses a resource by the
    // whole ARN, which is this store's key, so none of them is read here.
    let resource = fields.nth(4)?;
    Some(resource.split('/').next().unwrap_or(resource))
}

/// Which kind of resource an ARN handed to a tagging call addresses.
///
/// A tagging operation names no resource kind of its own, so the kind comes from the
/// ARN's own resource segment ([`ResourceKind::arn_segment`]). An identifier spelling
/// no segment this service owns addresses this service's primary taggable resource,
/// the elastic cluster its tagging operations are documented over ("Lists all tags on
/// a elastic cluster resource").
pub fn tagged_resource_kind(arn: &str) -> ResourceKind {
    match arn_resource_segment(arn) {
        Some(segment) if segment == ResourceKind::ClusterSnapshot.arn_segment() => {
            ResourceKind::ClusterSnapshot
        }
        _ => ResourceKind::Cluster,
    }
}

/// The `ResourceNotFoundException` (HTTP 404) for an ARN this store does not hold.
///
/// The two structured members are what a client branches on: `resourceType` is the
/// AWS resource type name of the kind that was looked up (see [`ResourceKind`]) and
/// `resourceId` is the ARN the request named. The message is the fixed sentence for
/// that resource class ([`ResourceKind::not_found_message`]) and carries neither.
pub fn not_found(kind: ResourceKind, arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: kind.not_found_message().to_string(),
        resource_id: arn.to_string(),
        resource_type: kind.type_name().to_string(),
    }
}

/// What an ARN-ADDRESSED call answers for a `resourceArn` this store holds no record
/// under: fixed text that names no resource class and interpolates nothing.
///
/// The tagging surface (`POST`, `DELETE` and `GET /tags/{resourceArn}`) addresses its
/// resource GENERICALLY — by the whole ARN of ANY resource class this service owns,
/// rather than by a typed identifier of one named class — so it words a miss
/// generically, where each resource's own reader
/// ([`ResourceKind::not_found_message`]) was asked for a resource of one named kind and
/// says which. Like every sentence here the WHOLE message is fixed: the ARN the request
/// named travels in `ResourceNotFoundException$resourceId`.
pub const TAGGED_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The `ResourceNotFoundException` (HTTP 404) an ARN-addressed tagging call answers for
/// an ARN this store holds no resource under.
///
/// The two structured members are built exactly as [`not_found`] builds them — the ARN
/// in `resourceId`, and in `resourceType` the class the ARN's own resource segment
/// names ([`tagged_resource_kind`]), which is the one thing a generic sentence does not
/// state — while the message is the generic [`TAGGED_RESOURCE_NOT_FOUND`]. One
/// constructor for the whole tagging surface, so a tag write, a tag removal and a tag
/// read of the same ARN cannot word one miss three ways.
pub fn tagged_resource_not_found(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: TAGGED_RESOURCE_NOT_FOUND.to_string(),
        ..not_found(tagged_resource_kind(arn), arn)
    }
}

/// The `ConflictException` (HTTP 409) for a request that collides with the already
/// existing resource `arn` of kind `kind`.
///
/// Same contract as [`not_found`] for the two structured members: the type name of the
/// resource kind and the ARN of the resource the request collided with.
pub fn conflict(kind: ResourceKind, arn: &str, message: String) -> ConflictException {
    ConflictException {
        message,
        resource_id: arn.to_string(),
        resource_type: kind.type_name().to_string(),
    }
}

/// The `ConflictException` (HTTP 409) for a create whose requested name is already held
/// by the resource `existing_arn` of kind `kind`.
///
/// Every operation that mints a NAMED resource of a kind this service owns refuses a
/// name already in use through this one constructor, so the duplicate-name conflict is
/// worded identically wherever it is raised: the fixed sentence of the colliding
/// resource's class ([`ResourceKind::duplicate_name_message`]), with the requested name
/// nowhere in it and the ARN of the resource that already holds the name in `resourceId`.
pub fn name_conflict(kind: ResourceKind, existing_arn: &str) -> ConflictException {
    conflict(
        kind,
        existing_arn,
        kind.duplicate_name_message().to_string(),
    )
}

/// Refuse a modification of a cluster whose status does not admit one.
///
/// An elastic cluster carries out one change at a time. A change the service is still
/// applying leaves the cluster in a transient status — [`STATUS_CREATING`],
/// [`STATUS_UPDATING`], [`STATUS_DELETING`] and the rest of the modelled `Status`
/// values that are not [`STATUS_ACTIVE`] — until the clock settles it, and only a
/// settled cluster takes the next one: a second modification arriving while one is
/// still in flight is the access conflict `ConflictException` (HTTP 409) reports, not
/// a second write. That is what makes the outcome of two overlapping modifications
/// decided rather than a race: exactly one is accepted.
///
/// Call [`ClusterRecord::settle_due`] first, so a cluster whose transition the clock
/// has already made due is judged by the status it is owed and not by the one it was
/// left in.
pub fn ensure_modifiable(record: &ClusterRecord) -> Result<(), ConflictException> {
    if record.is_settled_available() {
        return Ok(());
    }
    Err(conflict(
        ResourceKind::Cluster,
        &record.cluster_arn,
        format!(
            "The elastic cluster {} is in {} status and cannot be modified until it is {}",
            record.cluster_name, record.status, STATUS_ACTIVE
        ),
    ))
}

/// Refuse a lifecycle action on a cluster that is not in its available status.
///
/// A cluster acts on a stop (and, once it is written, a start) only from its settled
/// available status: `StopCluster` documents that "the elastic cluster must be in the
/// available state", so a cluster the service is still building or changing, or one
/// that is already stopping or stopped, is refused instead of taking the action. The
/// refusal is `ValidationException` (HTTP 400) because that is the client refusal those
/// operations model for a request they will not act on — they model no
/// `ConflictException` (`smithy_query(query="operation", operation="StopCluster")`).
///
/// `action` is the past participle of the action being refused, so the sentence names
/// what the caller asked for; the status the cluster is actually in is reported too, so
/// a caller can tell "not settled yet" from "already stopped". Call
/// [`ClusterRecord::settle_due`] first, so the judgement is on the status the clock
/// owes the cluster rather than the one it was left in.
pub fn ensure_available(record: &ClusterRecord, action: &str) -> Result<(), ValidationException> {
    if record.is_settled_available() {
        return Ok(());
    }
    Err(invalid_input(
        &record.cluster_arn,
        &format!(
            "The elastic cluster must be in {STATUS_ACTIVE} status to be {action}, but {} is in \
             {} status.",
            record.cluster_name, record.status
        ),
    ))
}

/// Refuse a start of a cluster whose status does not admit one.
///
/// `StartCluster` "restarts the stopped elastic cluster that is specified by clusterARN",
/// so [`STATUS_STOPPED`] is the only status it acts from: a cluster that is already
/// running, one the service is still building or changing, and one that is still stopping
/// are refused instead of taking the action, and the refused cluster keeps the status it
/// had. The refusal is `ValidationException` (HTTP 400), the client refusal the operation
/// models for a request it will not act on (`smithy_query(query="operation",
/// operation="StartCluster")`).
///
/// The model does not state the state machine, so the sentence is recorded behaviour: the
/// service states this one as fixed text that interpolates nothing — not the cluster, not
/// the status it is in.
///
/// Call [`ClusterRecord::settle_due`] first, so a cluster whose stop the clock has
/// already made due is judged by the status it is owed rather than the one it was left in.
pub fn ensure_startable(record: &ClusterRecord) -> Result<(), ValidationException> {
    if record.status == STATUS_STOPPED {
        return Ok(());
    }
    Err(ValidationException {
        message: "Can't start the cluster as it's currently in state unavailable for starting."
            .to_string(),
    })
}

/// The cluster statuses a delete is refused from: a cluster that has been stopped, and
/// one that is on its way to stopped.
///
/// A stopped elastic cluster is not running, so the service cannot tear it down: it has
/// to be started again before it can be deleted. Every other modelled `Status` value
/// (`ops::types::STATUS_VALUES`) either names a running cluster or names one the
/// service could not bring up — [`STATUS_CREATING`], [`STATUS_UPDATING`] and the
/// configuration-failure statuses — and a delete is how a caller gets rid of those, so
/// none of them blocks one.
pub const UNDELETABLE_STATUSES: &[&str] = &[STATUS_STOPPING, STATUS_STOPPED];

/// Refuse a delete of a cluster whose stored status does not admit one
/// ([`UNDELETABLE_STATUSES`]).
///
/// The refusal is `ValidationException` (HTTP 400) — the client refusal `DeleteCluster`
/// models for a request it will not act on (`smithy_query(query="operation",
/// operation="DeleteCluster")`) — and the service states it as one sentence whose only
/// variable part is the status the cluster is stored in. Ask it after
/// [`ClusterRecord::settle_due`] and BEFORE anything is removed, so the judgement is on
/// the status the clock owes the cluster and a refused delete leaves the record
/// readable in that status.
pub fn ensure_deletable(record: &ClusterRecord) -> Result<(), ValidationException> {
    if !UNDELETABLE_STATUSES.contains(&record.status.as_str()) {
        return Ok(());
    }
    Err(ValidationException {
        message: format!(
            "Can't delete the cluster because it is in {} state.",
            record.status
        ),
    })
}

/// Refuse a create that would take the cluster count past [`MAX_CLUSTERS`].
pub fn ensure_cluster_capacity(current: usize) -> Result<(), ServiceQuotaExceededException> {
    if current < MAX_CLUSTERS {
        return Ok(());
    }
    Err(ServiceQuotaExceededException {
        message: format!(
            "The maximum number of elastic clusters ({MAX_CLUSTERS}) has been reached; delete one before creating another"
        ),
    })
}

/// Refuse a create that would take the snapshot count past [`MAX_CLUSTER_SNAPSHOTS`].
pub fn ensure_snapshot_capacity(current: usize) -> Result<(), ServiceQuotaExceededException> {
    if current < MAX_CLUSTER_SNAPSHOTS {
        return Ok(());
    }
    Err(ServiceQuotaExceededException {
        message: format!(
            "The maximum number of cluster snapshots ({MAX_CLUSTER_SNAPSHOTS}) has been reached; delete one before creating another"
        ),
    })
}

/// Refuse a tag map larger than [`MAX_TAGS`], or a key or value longer than the
/// AWS-wide tag limits. Checked before the map is stored, so one request's stored
/// footprint is bounded.
pub fn validate_tags(
    member: &str,
    tags: &::std::collections::HashMap<String, String>,
) -> Result<(), ValidationException> {
    let label = member_label(member);
    ensure_tag_capacity(member, tags.len())?;
    for (key, value) in tags {
        if key.is_empty() || key.chars().count() > MAX_TAG_KEY_LEN {
            return Err(invalid_input(
                member,
                &format!("{label} keys must contain from 1 to {MAX_TAG_KEY_LEN} characters."),
            ));
        }
        if value.chars().count() > MAX_TAG_VALUE_LEN {
            return Err(invalid_input(
                member,
                &format!("{label} values must contain at most {MAX_TAG_VALUE_LEN} characters."),
            ));
        }
    }
    Ok(())
}

/// Refuse a tag count past [`MAX_TAGS`].
///
/// `total` is how many DISTINCT keys the resource would carry once the request is
/// applied, so a request that only overwrites keys the resource already has is not
/// refused for a count it does not change.
pub fn ensure_tag_capacity(member: &str, total: usize) -> Result<(), ValidationException> {
    if total <= MAX_TAGS {
        return Ok(());
    }
    Err(invalid_input(
        member,
        &format!(
            "{} must not contain more than {MAX_TAGS} entries.",
            member_label(member)
        ),
    ))
}

/// Refuse a `subnetIds` / `vpcSecurityGroupIds` list longer than
/// [`MAX_NETWORK_IDS`], or an entry longer than [`MAX_NETWORK_ID_LEN`].
pub fn validate_id_list(member: &str, ids: &[String]) -> Result<(), ValidationException> {
    let label = member_label(member);
    if ids.len() > MAX_NETWORK_IDS {
        return Err(invalid_input(
            member,
            &format!("{label} must not contain more than {MAX_NETWORK_IDS} entries."),
        ));
    }
    if ids
        .iter()
        .any(|id| id.is_empty() || id.chars().count() > MAX_NETWORK_ID_LEN)
    {
        return Err(invalid_input(
            member,
            &format!(
                "{label} entries must contain from 1 to {MAX_NETWORK_ID_LEN} characters."
            ),
        ));
    }
    Ok(())
}

/// Refuse a `snapshotName` outside [`SNAPSHOT_NAME_LEN`].
pub fn validate_snapshot_name(member: &str, value: &str) -> Result<(), ValidationException> {
    let (min, max) = SNAPSHOT_NAME_LEN;
    if (min..=max).contains(&value.chars().count()) {
        return Ok(());
    }
    Err(invalid_input(
        member,
        &format!(
            "{} must contain from {min} to {max} characters.",
            member_label(member)
        ),
    ))
}

/// The wire spelling of a `snapshotType` filter value, or a refusal when the value
/// names no snapshot type this service has.
///
/// The member is modelled as a plain `String` while the type a record carries is the
/// `SnapshotType` enum, and the documentation writes the two values in lowercase
/// prose (`ListClusterSnapshotsInput$snapshotType`: "automated" / "manual") while the
/// enum spells them uppercase — so the filter is matched case-insensitively and
/// normalised onto the enum spelling the record holds. A value outside the documented
/// set is a bad request rather than a silently empty page.
pub fn canonical_snapshot_type(member: &str, value: &str) -> Result<String, ValidationException> {
    SNAPSHOT_TYPE_VALUES
        .iter()
        .find(|known| known.eq_ignore_ascii_case(value))
        .map(|known| (*known).to_string())
        .ok_or_else(|| {
            invalid_input(
                value,
                &format!(
                    "{} must be one of {}.",
                    member_label(member),
                    SNAPSHOT_TYPE_VALUES.join(", ")
                ),
            )
        })
}

/// Refuse a `kmsKeyId` longer than [`MAX_KMS_KEY_ID_LEN`].
pub fn validate_kms_key_id(member: &str, value: &str) -> Result<(), ValidationException> {
    if !value.is_empty() && value.chars().count() <= MAX_KMS_KEY_ID_LEN {
        return Ok(());
    }
    Err(invalid_input(
        member,
        &format!(
            "{} must contain from 1 to {MAX_KMS_KEY_ID_LEN} characters.",
            member_label(member)
        ),
    ))
}

/// Refuse a `preferredMaintenanceWindow` outside the documented
/// `ddd:hh24:mi-ddd:hh24:mi` form, and answer the form the service STORES: the
/// day-of-week abbreviation is matched without regard to case and normalised to the
/// one capitalisation the service reports it in.
///
/// Source: `CreateClusterInput$preferredMaintenanceWindow` doc — "Format:
/// ddd:hh24:mi-ddd:hh24:mi … Valid days: Mon, Tue, Wed, Thu, Fri, Sat, Sun". The doc
/// names the days in one capitalisation but the member carries no `@pattern`, so the
/// day is a case-insensitive abbreviation on the way in and is echoed in the
/// documented spelling on the way out.
pub fn validate_maintenance_window(
    member: &str,
    value: &str,
) -> Result<String, ValidationException> {
    if let Some(normalized) = normalized_maintenance_window(value) {
        Ok(normalized)
    } else {
        Err(invalid_input(
            value,
            &format!(
                "{} must be of the form ddd:hh24:mi-ddd:hh24:mi, with ddd one of {days}.",
                member_label(member),
                days = MAINTENANCE_DAYS.join(", ")
            ),
        ))
    }
}

/// Refuse a `preferredBackupWindow` outside the documented `hh24:mi-hh24:mi` form.
///
/// Source: `CreateClusterInput$preferredBackupWindow` doc — it is "The daily time
/// range during which automated backups are created", so it names a time of day and
/// no day of the week.
pub fn validate_backup_window(member: &str, value: &str) -> Result<(), ValidationException> {
    let well_formed = value
        .split_once('-')
        .is_some_and(|(start, end)| is_time_of_day(start) && is_time_of_day(end));
    if well_formed {
        Ok(())
    } else {
        Err(invalid_input(
            value,
            &format!(
                "{} must be of the form hh24:mi-hh24:mi.",
                member_label(member)
            ),
        ))
    }
}

/// The `ddd:hh24:mi-ddd:hh24:mi` window `value` names with each day abbreviation in
/// the spelling [`MAINTENANCE_DAYS`] holds, or `None` when it is not of that form.
fn normalized_maintenance_window(value: &str) -> Option<String> {
    let (start, end) = value.split_once('-')?;
    Some(format!(
        "{}-{}",
        normalized_day_time(start)?,
        normalized_day_time(end)?
    ))
}

/// `ddd:hh24:mi` — a valid day name, whatever case it was written in, followed by a
/// wall-clock time; answered with the day in its documented spelling.
fn normalized_day_time(value: &str) -> Option<String> {
    let (day, time) = value.split_once(':')?;
    let day = MAINTENANCE_DAYS
        .iter()
        .find(|known| known.eq_ignore_ascii_case(day))?;
    is_time_of_day(time).then(|| format!("{day}:{time}"))
}

/// `hh24:mi` — two digits of hour below 24 and two of minute below 60.
fn is_time_of_day(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    let hour_ok = hour.len() == 2 && matches!(hour.parse::<u32>(), Ok(h) if h < 24);
    let minute_ok = minute.len() == 2 && matches!(minute.parse::<u32>(), Ok(m) if m < 60);
    hour_ok && minute_ok
}

/// The `preferredMaintenanceWindow` a cluster created without one reports.
///
/// AWS picks "a 30-minute window selected at random from an 8-hour block of time for
/// each Amazon Web Services Region, occurring on a random day of the week"
/// (`CreateClusterInput$preferredMaintenanceWindow`). The emulator derives it from
/// the cluster's own name instead, so it is stable across reads of one cluster and
/// still differs between clusters; the shape is the documented
/// `ddd:hh24:mi-ddd:hh24:mi` and the span is the documented 30-minute minimum.
pub fn default_maintenance_window(cluster_name: &str) -> String {
    let digest = name_digest(cluster_name);
    let day_index = (digest % MAINTENANCE_DAYS.len() as u64) as usize;
    let start_hour = ((digest / 7) % 24) as u32;
    let start_minute = if (digest / 168).is_multiple_of(2) { 0 } else { 30 };
    let (end_hour, end_minute) = if start_minute == 0 {
        (start_hour, 30)
    } else {
        ((start_hour + 1) % 24, 0)
    };
    // Rolling past midnight rolls the day with it, so the window stays 30 minutes.
    let end_day_index = if end_hour < start_hour {
        (day_index + 1) % MAINTENANCE_DAYS.len()
    } else {
        day_index
    };
    format!(
        "{}:{start_hour:02}:{start_minute:02}-{}:{end_hour:02}:{end_minute:02}",
        MAINTENANCE_DAYS[day_index], MAINTENANCE_DAYS[end_day_index]
    )
}

/// A deterministic digest of a resource name, used only to spread a derived default
/// (the maintenance window) across names.
///
/// `DefaultHasher::new()` starts from fixed keys, so one name always yields one
/// window; the value is stored on the record when the cluster is created, so nothing
/// depends on the digest being the same in a different build of the emulator.
fn name_digest(name: &str) -> u64 {
    use ::std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

/// The shard list a cluster of `shard_count` shards has.
///
/// `existing` is kept as far as it goes, so resizing a cluster neither renames the
/// shards it keeps nor loses their creation times; shards beyond the new count are
/// dropped and new ones are minted at `created_at`. Shard STATUS is owned by
/// [`ClusterRecord::begin_transition`], which the caller runs after resizing, so a
/// new shard starts at the same status as the cluster it belongs to.
///
/// The count is clamped to the published ceiling here as well as refused at the
/// boundary by [`validate_shard_count`], so what one request can make this function
/// allocate is bounded by that ceiling rather than by the caller remembering to check
/// ("Maximum is 32", [`SHARD_COUNT_MAX`]).
pub fn shards_for(
    shard_count: i32,
    created_at: &str,
    existing: &[ShardRecord],
) -> Vec<ShardRecord> {
    let wanted = shard_count.clamp(0, SHARD_COUNT_MAX) as usize;
    let mut shards: Vec<ShardRecord> = existing.iter().take(wanted).cloned().collect();
    shards.reserve(wanted - shards.len());
    while shards.len() < wanted {
        shards.push(ShardRecord {
            shard_id: new_resource_id(),
            create_time: created_at.to_string(),
            status: STATUS_CREATING.to_string(),
        });
    }
    shards
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState>)
}
