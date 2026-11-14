// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticache-amazonelasticachev9 service.
//!
//! The transient statuses this service reports — a cluster `creating`, a snapshot
//! `copying`, a group `modifying` — are held for a span the EMULATOR chooses, not
//! one AWS documents: [`CLUSTER_TRANSITION`] and its siblings compress what
//! ElastiCache takes minutes over into half a second, and
//! [`SETTLE_WAIT_CAP`] is how long a report waits for such a span to pass before
//! it answers ([`await_settled`]). Both are this emulator's deliberate choice,
//! stated here beside the durations it does take from AWS — a maintenance
//! window's minimum, a snapshot's retention, a reservation's term — which are
//! kept as the service publishes them.
//!
//! The cache cluster is ElastiCache's primary resource: `CreateCacheCluster`
//! mints one, `DescribeCacheClusters` reports it, `ModifyCacheCluster` changes
//! it and `DeleteCacheCluster` removes it. A cache subnet group is the
//! supporting resource a VPC cluster is placed in. The records below are the one
//! copy of each that every handler reads and writes, plus the projections into
//! the modelled `CacheCluster` / `CacheSubnetGroup` shapes, so the six handlers
//! agree on one implementation instead of carrying copies that can drift.
//!
//! The serverless cache is ElastiCache's other cache resource:
//! `CreateServerlessCache` mints one, `ModifyServerlessCache` changes it,
//! `DescribeServerlessCaches` reports it and `DeleteServerlessCache` takes it
//! away. Its record and its projection into the modelled `ServerlessCache`
//! shape live here too, so those handlers write the same
//! copy the reader reads instead of introducing a
//! second one. A deleted cache does not vanish at the call: it answers
//! `DELETING` and the clock owes its removal
//! ([`ServerlessCacheRecord::pending_deletion`]), the same way a created one is
//! owed `AVAILABLE`.
//!
//! A serverless cache's snapshot is its own resource, held in
//! [`ServerlessCacheSnapshotRecord`] beside the caches rather than on them,
//! because it outlives the cache it was taken of:
//! `CreateServerlessCacheSnapshot` mints one from a stored serverless cache (and
//! `DeleteServerlessCache`'s `FinalSnapshotName` mints the last one),
//! `DescribeServerlessCacheSnapshots` reports it, and the copy, export and
//! delete calls for one are still scaffolded 501s that will read and write this
//! same copy rather than a second one.
//!
//! A snapshot is a copy of a cluster as of the time it was taken:
//! `CreateSnapshot` mints one from a stored cluster, `CopySnapshot` mints a
//! second from a stored snapshot, `DescribeSnapshots` reports it and
//! `DeleteSnapshot` takes it away, so [`SnapshotRecord`] below is the one copy
//! all of them read. A deleted snapshot does not vanish at the call: it answers
//! `deleting` and the clock owes its removal
//! ([`SnapshotRecord::pending_removal`]), the same way a created one is owed
//! `available`.
//!
//! Beside the resources sits the event log: every change this service makes to
//! one of them records an [`EventRecord`], and `DescribeEvents` reports that log
//! over the window a request asks for. The log is the one copy, kept beside the
//! resources rather than on them, because an event outlives the resource it is
//! about and because a status change the CLOCK materialises
//! ([`CacheClusterRecord::settle`]) has to record one too.
//!
//! A resource's TAGS, by contrast, live ON its record
//! ([`CacheClusterRecord::tags`] and its siblings), because a tag belongs to one
//! resource and goes when it goes. That is the one copy a create's `Tags` member
//! writes, `ListTagsForResource` reports and `AddTagsToResource` /
//! `RemoveTagsFromResource` change; the three of them resolve an ARN to it
//! through [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`], so
//! they cannot disagree about which resource an ARN names.
//!
//! Beside those customer-created resources sit the catalogs this service
//! publishes rather than mints: the service updates AWS releases for its cache
//! engines, the cache engines and versions themselves, and the ENGINE DEFAULT
//! PARAMETERS of every cache parameter group family
//! ([`published_engine_default_parameters`]). No operation creates or removes an
//! entry of any of them (the API has no such call), so the store starts out
//! holding each catalog and `DescribeServiceUpdates` /
//! `DescribeCacheEngineVersions` / `DescribeEngineDefaultParameters` only report
//! it. The engine-version catalog is also the one statement of which version is
//! the newest of each engine and which parameter group family a version belongs
//! to, so `CreateCacheCluster`'s defaults are read off it rather than from a
//! second list.
//!
//! The reserved-cache-node offerings are a further such catalog, and the
//! reservations bought from one ([`ReservedCacheNodeRecord`]) ARE an account
//! resource: `PurchaseReservedCacheNodesOffering` mints one and
//! `DescribeReservedCacheNodes` reports it.
//!
//! A cache parameter group is a supporting resource:
//! `CreateCacheParameterGroup` mints a custom one and
//! `DescribeCacheParameterGroups` reports it beside the family defaults the
//! service publishes. The parameter VALUES a group has had SET on it live on
//! that record ([`CacheParameterGroupRecord::parameter_values`]), because a
//! group reports its family's engine defaults with exactly those values laid
//! over them — which is what `DescribeCacheParameters` answers with.
//! `ModifyCacheParameterGroup` is the operation that WRITES that map and
//! `ResetCacheParameterGroup` the one that removes entries from it, so a
//! parameter a group no longer holds a value for reports the engine's own
//! default again. Keeping the map on the group record is what lets the writers
//! write the copy the reader already reads instead of a second one beside it.
//!
//! Beside the cache resources sits the RBAC user: `CreateUser` mints one,
//! `ModifyUser` changes its permissions and the way it authenticates,
//! `DescribeUsers` reports it and `DeleteUser` takes it away, over the one
//! [`UserRecord`] map below, which also holds the built-in `default` user every
//! account already has. A modified user does not settle at the call: it answers
//! `modifying` and the clock owes it `active` ([`UserRecord::pending_status`])
//! the same way a deleted one is owed its removal.
//!
//! The update action is this service's one resource nothing creates: it exists
//! for every (published service update, cluster-or-replication-group) pair the
//! update applies to, so `DescribeUpdateActions` crosses the catalog with
//! [`ComAmazonawsElasticacheAmazonelasticachev9State::update_action_targets`]
//! and reports a pair with no [`UpdateActionRecord`] as `not-applied`;
//! `BatchApplyUpdateAction` and `BatchStopUpdateAction` are what store a record
//! for one, and the clock carries it from there.
//!
//! A user group is the RBAC resource built out of those users:
//! `CreateUserGroup` mints one, `DescribeUserGroups` reports it and
//! `ModifyUserGroup` changes the users that belong to it and the engine it is
//! listed for. A membership change is not applied at the call — the group
//! answers `modifying` and carries the change on
//! [`UserGroupRecord::pending_user_ids_to_add`] /
//! [`UserGroupRecord::pending_user_ids_to_remove`], which is what
//! `UserGroup$PendingChanges` reports until the clock applies it
//! ([`UserGroupRecord::settle`]) — so the membership still has ONE copy, the
//! group's `user_ids`, and `User$UserGroupIds` stays derived from it.
//!
//! Everything else in this service (`DeleteUserGroup`, the migration
//! operations other than `StartMigration`, and the parameter-group operations
//! that would change a parameter's value) is still a scaffolded 501 and owns its
//! own gap issue; where a member of an implemented operation belongs to one of
//! those resources, the comment beside it says so.
//!
//! A data migration is the work that moves a customer-run cache into one this
//! account runs: `StartMigration` starts the data sync into the target the
//! request names — a replication group or a serverless cache — and the ONE copy
//! of the open migration lives on that target's record
//! ([`ServerlessCacheRecord::migration`] /
//! [`ReplicationGroupRecord::migration`], both a [`MigrationRecord`]). That one
//! copy is what refuses a second start on the same target and what
//! `ServerlessCache$Migration` reports; `PrepareMigration`, `StopMigration`,
//! `CompleteMigration` and `TestMigration` are still scaffolded 501s with their
//! own gap issues and read and write the same record rather than a copy beside
//! it.
//!
//! The serverless cache is another customer-created resource this store holds:
//! `CreateServerlessCache` mints one, `ModifyServerlessCache` changes it,
//! `DescribeServerlessCaches` reports it and `DeleteServerlessCache` removes it.
//! The serverless snapshot operations are still scaffolded 501s with their own
//! gap issues, and every one of them reads and writes the SAME record rather
//! than a copy beside it.
//!
//! A replication group is the other resource this state holds:
//! `CreateReplicationGroup` mints one together with the member cache clusters it
//! is made of, and `DescribeReplicationGroups` reports it. Those member clusters
//! are ordinary [`CacheClusterRecord`]s in the same map as the standalone ones,
//! so there is one copy of a cluster however it was created.
//!
//! A Global datastore ([`GlobalReplicationGroupRecord`]) is built AROUND one
//! such replication group: `CreateGlobalReplicationGroup` mints it from the
//! primary that accepts writes, `DescribeGlobalReplicationGroups` reports it,
//! `ModifyGlobalReplicationGroup`, the failover, the (dis)association and the
//! slice-group operations change it, and `DeleteGlobalReplicationGroup` takes it
//! away. Which datastore a replication group belongs to and the role it has
//! there is held ONCE per record on each side — on the datastore's member list
//! and on [`ReplicationGroupRecord::global_replication_group_id`] — and every
//! operation that moves a membership writes both, so the two reads cannot
//! disagree about what the account runs.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    Authentication, AuthenticationMode, AvailabilityZone, CacheCluster, CacheEngineVersion,
    CacheNode, CacheNodeTypeSpecificParameter, CacheNodeTypeSpecificValue, CacheParameterGroup,
    CacheNodeUpdateStatus, CacheParameterGroupStatus, CacheSubnetGroup, CacheUsageLimits,
    ConfigureShard, CreateUserGroupOutput, CreateUserOutput, CustomerNodeEndpoint, DataStorage,
    DestinationDetails, ECPUPerSecond, Endpoint, Event, MigrationSummary,
    GlobalNodeGroup, GlobalReplicationGroup, GlobalReplicationGroupInfo,
    GlobalReplicationGroupMember, LogDeliveryConfiguration, LogDeliveryConfigurationRequest,
    NodeGroup, NodeGroupConfiguration, NodeGroupMember, NodeGroupMemberUpdateStatus,
    NodeGroupUpdateStatus, NodeSnapshot, NotificationConfiguration, Parameter,
    PendingLogDeliveryConfiguration, PendingModifiedValues, ProcessedUpdateAction, RecurringCharge,
    ReplicationGroup, ReplicationGroupPendingModifiedValues, ReservedCacheNode,
    ReservedCacheNodesOffering, ScaleConfig, SecurityGroupMembership, ServerlessCache,
    ServerlessCacheConfiguration, ServerlessCacheSnapshot,
    ServiceUpdate, Snapshot, Subnet, Tag, UnprocessedUpdateAction, UpdateAction,
    UserGroupPendingChanges, UserGroupsUpdateStatus,
};

pub mod ec2_client;
pub mod ec2_security_groups;
pub mod ec2_subnets;
mod event_notifications;
pub mod kms_key;
pub mod s3_export;
mod store;
mod parameters;
mod reserved_nodes;
mod cache_clusters;
mod cache_data_plane;
mod replication_groups;
mod shard_configuration;
mod engine_versions;
mod rbac_engine_versions;
mod serverless_caches;
mod migrations;
mod network_types;
mod settle_wait;
mod snapshots;
pub use settle_wait::*;
pub use network_types::*;
pub use snapshots::*;
pub use migrations::*;
pub use serverless_caches::*;
pub use engine_versions::*;
pub use rbac_engine_versions::*;
pub use replication_groups::*;
pub use shard_configuration::*;
pub use cache_clusters::*;
pub use cache_data_plane::*;
pub use reserved_nodes::*;
pub use parameters::*;
pub use event_notifications::*;

/// The region a request is treated as having been made in when its SigV4
/// credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` this service's ARNs carry, from the
/// model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: elasticache`).
pub const ARN_NAMESPACE: &str = "elasticache";

/// How a handler words the one condition that leaves it without a store.
///
/// This service's model declares NO server-side error (every modelled error is
/// `errorKind: client`), so there is no 5xx shape to answer with; the synthetic
/// `ValidationException` is used instead of the scaffold's 501, which would read
/// as "this operation is not written yet". The condition itself is unreachable in
/// the assembled emulator: the router builds one `engine::InMemoryEngine` and
/// hands it to every service, and that is the only engine [`service_state`]
/// resolves against.
pub const STORE_UNAVAILABLE: &str =
    "the ElastiCache store is unavailable: this service is mounted on an engine it cannot \
     keep state in";

/// The cache engine a cluster runs when `CreateCacheCluster` names none.
///
/// The model declares no `@default` for `Engine`, so this is the emulator's
/// choice; `redis` is also what the AWS CLI sends when the flag is omitted.
pub const DEFAULT_ENGINE: &str = "redis";

/// The Valkey engine, named wherever behaviour differs by engine (the version
/// catalogue, a replication group's at-rest encryption default).
pub const ENGINE_VALKEY: &str = "valkey";

/// The cache engines a cluster may run.
///
/// From the model's own `CreateCacheCluster` documentation: "All nodes in the
/// cluster run the same protocol-compliant cache engine software, either
/// Memcached, Valkey or Redis OSS."
pub const ENGINES: [&str; 3] = ["memcached", DEFAULT_ENGINE, ENGINE_VALKEY];

/// The `MaxRecords` of every paginated describe in this service, as each of
/// those members documents it: "Default: 100. Constraints: minimum 20; maximum
/// 100."
pub const MAX_RECORDS_MIN: i32 = 20;
/// See [`MAX_RECORDS_MIN`].
pub const MAX_RECORDS_MAX: i32 = 100;
/// See [`MAX_RECORDS_MIN`].
pub const MAX_RECORDS_DEFAULT: i32 = 100;

/// `DescribeSnapshots$MaxRecords`, as that member documents it: "Default: 50.
/// Constraints: minimum 20; maximum 50." The minimum is the same
/// [`MAX_RECORDS_MIN`] every paginated describe in this service documents; the
/// default and the maximum are this operation's own.
pub const SNAPSHOT_MAX_RECORDS_MAX: i32 = 50;
/// See [`SNAPSHOT_MAX_RECORDS_MAX`].
pub const SNAPSHOT_MAX_RECORDS_DEFAULT: i32 = 50;

/// The largest Memcached cluster, and the only legal Valkey / Redis OSS node
/// count, as `NumCacheNodes` documents it: "For clusters running Valkey or Redis
/// OSS, this value must be 1. For clusters running Memcached, this value must be
/// between 1 and 40."
pub const MEMCACHED_MAX_NODES: i32 = 40;

/// The most nodes this emulator will MATERIALISE for one cluster, whatever a
/// request asks for: the largest cluster any engine permits
/// ([`MEMCACHED_MAX_NODES`]).
///
/// The handlers refuse a larger `NumCacheNodes` before a record is built (see
/// [`validate_num_cache_nodes`]); this is the bound at the allocation site
/// itself, so no path through [`CacheClusterRecord::resize_nodes`] can be made
/// to allocate node records without end.
pub const MAX_CACHE_NODES_PER_CLUSTER: i32 = MEMCACHED_MAX_NODES;

/// The smallest cluster: a cluster always has at least one node, which is also
/// the floor [`CacheClusterRecord::resize_nodes`] clamps to.
pub const MIN_CACHE_NODES_PER_CLUSTER: i32 = 1;
/// See [`MEMCACHED_MAX_NODES`].
pub const REDIS_NUM_CACHE_NODES: i32 = 1;

/// The number of nodes a cluster is created with when the request names none.
///
/// The model declares no default; 1 is the only count Valkey / Redis OSS
/// permits and the smallest Memcached cluster, so it is the one value that is
/// legal for every engine.
pub const DEFAULT_NUM_CACHE_NODES: i32 = 1;

/// The `SnapshotRetentionLimit` a Valkey / Redis OSS cluster is created with
/// when the request names none: the member documents "Default: 0 (i.e.,
/// automatic backups are disabled for this cache cluster)". It is "only valid if
/// the Engine parameter is redis", so a Memcached cluster carries none.
pub const DEFAULT_SNAPSHOT_RETENTION_LIMIT: i32 = 0;

/// The weekly maintenance window a cluster is created with when the request
/// names none.
///
/// ElastiCache assigns a random 60-minute window per cluster; the emulator
/// assigns one fixed window instead of inventing a per-cluster random one, and
/// this is the format's own example from `PreferredMaintenanceWindow`
/// ("Example: sun:23:00-mon:01:30").
pub const DEFAULT_MAINTENANCE_WINDOW: &str = "sun:23:00-mon:01:30";

/// The day abbreviations a maintenance window may name, in week order.
///
/// `PreferredMaintenanceWindow` states them: "Valid values for `ddd` are: sun,
/// mon, tue, wed, thu, fri, sat". The position in this list is the day's offset
/// from the start of the week, which is what makes the span of a window that
/// crosses midnight computable ([`maintenance_window_refusal`]).
pub const MAINTENANCE_WINDOW_DAYS: [&str; 7] =
    ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// The documented minimum span of a maintenance window, in minutes:
/// `PreferredMaintenanceWindow` states "The minimum maintenance window is a 60
/// minute period."
pub const MAINTENANCE_WINDOW_MIN_MINUTES: i64 = 60;

/// Minutes in a day, and in a week: the arithmetic a `ddd:hh24:mi` point is
/// placed on.
const MINUTES_PER_DAY: i64 = 24 * 60;
/// See [`MINUTES_PER_DAY`].
const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;

/// How ElastiCache words a `PreferredMaintenanceWindow` it cannot read.
///
/// The format is the one the member documents — "a range in the format
/// ddd:hh24:mi-ddd:hh24:mi (24H Clock UTC)" — and this is the wording AWS was
/// recorded answering with. The HTTP status (400) and the awsQuery code
/// (`InvalidParameterValue`) are the contract.
pub const INVALID_MAINTENANCE_WINDOW_MESSAGE: &str =
    "Invalid maintenance window format. Should be specified as a range \
     ddd:hh24:mi-ddd:hh24:mi (24H Clock UTC). Example: Sun:23:15-Mon:03:15";

/// Minutes from the start of the week that the `ddd:hh24:mi` point `value`
/// names, or `None` when it is not one.
///
/// The day is matched case-insensitively against [`MAINTENANCE_WINDOW_DAYS`] and
/// the clock fields are exactly two digits each on a 24-hour clock, so neither
/// `notaday:99:99` nor `24:00` is a point.
fn week_minute(value: &str) -> Option<i64> {
    let mut fields = value.split(':');
    let day = fields.next()?;
    let hour = fields.next()?;
    let minute = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let day = MAINTENANCE_WINDOW_DAYS
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(day))?;
    let clock_field = |field: &str, max: i64| -> Option<i64> {
        if field.len() != 2 || !field.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        field.parse::<i64>().ok().filter(|value| *value <= max)
    };
    let hour = clock_field(hour, 23)?;
    let minute = clock_field(minute, 59)?;
    Some(i64::try_from(day).unwrap_or(0) * MINUTES_PER_DAY + hour * 60 + minute)
}

/// Why `value` cannot be a `PreferredMaintenanceWindow`, or `None` when it can.
///
/// The rule is the member's own: a range `ddd:hh24:mi-ddd:hh24:mi` on the 24-hour
/// UTC clock whose span is at least [`MAINTENANCE_WINDOW_MIN_MINUTES`]. A window
/// whose end lies before its start wraps to the next week, which is how
/// `sun:23:00-mon:01:30` is a 150-minute window; a window whose two ends are the
/// same point spans nothing and is refused.
///
/// Every operation that accepts a maintenance window goes through this one
/// helper, so a value refused by one is refused by all of them, and the refusal
/// runs before any state is written.
pub fn maintenance_window_refusal(value: &str) -> Option<String> {
    let invalid = || Some(INVALID_MAINTENANCE_WINDOW_MESSAGE.to_string());
    let Some((start, end)) = value.split_once('-') else {
        return invalid();
    };
    let (Some(start), Some(end)) = (week_minute(start), week_minute(end)) else {
        return invalid();
    };
    let span = match end.cmp(&start) {
        ::std::cmp::Ordering::Greater => end - start,
        ::std::cmp::Ordering::Equal => 0,
        ::std::cmp::Ordering::Less => end + MINUTES_PER_WEEK - start,
    };
    if span < MAINTENANCE_WINDOW_MIN_MINUTES {
        return Some(format!(
            "The maintenance window must be at least {MAINTENANCE_WINDOW_MIN_MINUTES} minutes."
        ));
    }
    None
}

/// How long a cluster stays in the status the call that started the work
/// returned (`creating` after a create, `modifying` after an immediate modify,
/// `rebooting cluster nodes` after a reboot) before the clock owes it
/// `available`.
///
/// ElastiCache takes minutes; the emulator chooses a span, and it is
/// deliberately NOT zero, so the call that starts the work answers with the
/// transient status and a MUTATION arriving inside the span is still refused by
/// the state gate that goes with it. The due instant is stored on the record and
/// materialised by whichever later call reads it; a REPORT issued inside the
/// span is answered once the transition has fallen due
/// ([`await_settled`]), so a client polling on an AWS-sized interval converges
/// on its first poll.
pub const CLUSTER_TRANSITION: Duration = Duration::from_millis(500);

/// How long a snapshot stays in the status the call that started the work
/// returned — `creating` after a create or a copy before the clock owes it
/// `available`, and `deleting` after a delete before the clock owes its removal.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: ElastiCache takes minutes, the
/// emulator chooses a span, and it is deliberately NOT zero so `CreateSnapshot`
/// / `CopySnapshot` / `DeleteSnapshot` answer with the transient status and a
/// second change inside the span is still refused.
pub const SNAPSHOT_TRANSITION: Duration = Duration::from_millis(500);

/// How many clusters, and how many subnet groups, the store holds before a
/// create is refused with the modelled quota fault.
///
/// ElastiCache publishes adjustable quotas per account per Region and this
/// emulator models neither Service Quotas nor an account's adjustments, so these
/// are the emulator's OWN bounds on the store rather than a claim about any
/// account's limit. They sit far above any workflow an emulator serves, and they
/// are what stops the state growing without end.
pub const CLUSTER_QUOTA: usize = 300;
/// See [`CLUSTER_QUOTA`].
pub const SUBNET_GROUP_QUOTA: usize = 150;
/// See [`CLUSTER_QUOTA`]; this one bounds the CUSTOM cache parameter groups the
/// store holds, the family defaults of [`default_parameter_groups`] being
/// published by the service rather than created by an account.
pub const PARAMETER_GROUP_QUOTA: usize = 150;
/// See [`CLUSTER_QUOTA`]; this is what `SnapshotQuotaExceededFault` refuses
/// past.
pub const SNAPSHOT_QUOTA: usize = 300;

/// How many subnets one cache subnet group may hold.
///
/// The model gives `SubnetIds` no length constraint; this is the emulator's own
/// bound on one group, set at the quota ElastiCache publishes by default, and it
/// is what `CacheSubnetQuotaExceededFault` refuses past.
pub const SUBNETS_PER_GROUP: usize = 20;

/// How many tags a resource may carry, as `TagQuotaPerResourceExceeded`
/// documents it: "The maximum number of tags permitted on a resource is 50."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest `CacheClusterId`: "A name must contain from 1 to 50 alphanumeric
/// characters or hyphens."
pub const CLUSTER_ID_MAX_LEN: usize = 50;

/// The longest `ReplicationGroupId`: "A name must contain from 1 to 40
/// alphanumeric characters or hyphens."
pub const REPLICATION_GROUP_ID_MAX_LEN: usize = 40;

/// The longest a `CacheParameterGroupName` may be: the 255-character bound this
/// service publishes for its names ("must contain no more than 255 alphanumeric
/// characters or hyphens"; see [`is_valid_subnet_group_name`]). The member
/// itself documents only that the value "is stored as a lowercase string", so
/// the bound is the one the service's names share.
pub const PARAMETER_GROUP_NAME_MAX_LEN: usize = 255;

/// True when `value` satisfies the name rule ElastiCache documents for every one
/// of its identifiers — "A name must contain from 1 to `max_len` alphanumeric
/// characters or hyphens. The first character must be a letter. A name cannot
/// end with a hyphen or contain two consecutive hyphens." — the only thing that
/// differs between them being the length bound.
///
/// These members carry no modelled `@length` or `@pattern`, so the boundary
/// validator passes any string and the rule is enforced here.
pub fn is_valid_identifier(value: &str, max_len: usize) -> bool {
    (1..=max_len).contains(&value.len())
        && value.starts_with(|first: char| first.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--")
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// True when `value` satisfies `CacheClusterId`'s documented constraints; see
/// [`is_valid_identifier`].
pub fn is_valid_cluster_id(value: &str) -> bool {
    is_valid_identifier(value, CLUSTER_ID_MAX_LEN)
}

/// True when `value` can name a snapshot: 1 to 255 alphanumeric characters or
/// hyphens.
///
/// `SnapshotName` carries no modelled `@length` or `@pattern` and the model
/// documents only that the value "is stored as a lowercase string", so the only
/// form enforced here is the character set and the length bound this service's
/// names document (see [`is_valid_subnet_group_name`]: "must contain no more
/// than 255 alphanumeric characters or hyphens"). Nothing refuses a hyphen's
/// placement or a leading digit, because no ElastiCache documentation states
/// either of a snapshot name and a name AWS accepts must not be refused here.
pub fn is_valid_snapshot_name(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// True when `value` satisfies `CacheSubnetGroupName`'s documented constraint:
/// "Must contain no more than 255 alphanumeric characters or hyphens."
pub fn is_valid_subnet_group_name(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The longest tag key a caller may use: "Maximum key length — 128 Unicode
/// characters in UTF-8", from the tag restrictions ElastiCache publishes
/// ("Tagging your ElastiCache resources", the same restrictions AWS states for
/// tags generally). `Tag$Key` carries no modelled `@length` or `@pattern`, so
/// the boundary validator passes any string and the bound is enforced here.
pub const MAX_TAG_KEY_LEN: usize = 128;

/// The longest tag value a caller may use: "Maximum value length — 256 Unicode
/// characters in UTF-8"; see [`MAX_TAG_KEY_LEN`]. A value of length ZERO is
/// permitted — `Tag$Value` is "the tag's value. May be null".
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The tag-key prefix AWS reserves for itself: "Do not use the `aws:` prefix in
/// your tag names or values because it is reserved for AWS use. You can't edit
/// or delete tag names or values with this prefix." So a CALLER's tag key may
/// not carry it, in any letter case, wherever a request names tag keys — a
/// create's `Tags`, `AddTagsToResource`'s `Tags`, and
/// `RemoveTagsFromResource`'s `TagKeys`.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why a request's `Tags` cannot be stored.
#[derive(Debug, Clone)]
pub enum TagRefusal {
    /// A tag a caller may not write, with the rule it breaks: a `Key` the
    /// request omitted or left empty (`Tag$Key` is "the key for the tag. May not
    /// be null" — only the VALUE may be), a key or value past its length bound,
    /// or a key under the reserved [`RESERVED_TAG_KEY_PREFIX`].
    InvalidTag(String),
    /// More tags than [`MAX_TAGS_PER_RESOURCE`].
    TooMany(usize),
}

/// The symbols a tag key or value may carry besides letters, digits and
/// whitespace, from the character set ElastiCache publishes for tags and names
/// in its own refusal ("Tag values may only contain unicode letters, digits,
/// whitespace, or one of these symbols: `_ . : / = + \ - @`"). The
/// letters-and-digits half is UNICODE-wide, which is why
/// [`is_tag_character`] asks [`char::is_alphabetic`] / [`char::is_numeric`]
/// rather than their ASCII-only forms.
pub const TAG_SYMBOLS: [char; 9] = ['_', '.', ':', '/', '=', '+', '\\', '-', '@'];

/// Whether `character` is one a tag key or a tag value may carry; see
/// [`TAG_SYMBOLS`]. Neither `Tag$Key` nor `Tag$Value` carries a modelled
/// `@pattern`, so the boundary validator passes any string and the character set
/// is enforced here — once, for every request that names a tag.
pub fn is_tag_character(character: char) -> bool {
    character.is_alphabetic()
        || character.is_numeric()
        || character.is_whitespace()
        || TAG_SYMBOLS.contains(&character)
}

/// The sentence a charset refusal carries for `member` ("keys" or "values"),
/// in the service's own wording; see [`TAG_SYMBOLS`].
///
/// The published sentence spells the backslash of the symbol list DOUBLED and
/// closes with a stray double quote, so it is reproduced here as the service
/// words it rather than as a tidied transcription: a caller matching the
/// service's text has nothing else to match against.
fn tag_charset_message(member: &str) -> String {
    format!(
        "Tag {member} may only contain unicode letters, digits, whitespace, or one of these \
         symbols: _ . : / = + \\\\ - @\""
    )
}

/// What is wrong with a caller-supplied tag KEY, or `None` when the caller may
/// use it — the one statement of the rule, so a create's `Tags`,
/// `AddTagsToResource` and `RemoveTagsFromResource` cannot disagree about which
/// keys a caller owns.
pub fn tag_key_rejection(key: &str) -> Option<String> {
    // Both bounds of the key's length are the one rule, so the empty key and
    // the key past the published maximum are refused by the same sentence: it
    // states the window a key's length has to fall in.
    if key.is_empty() || key.chars().count() > MAX_TAG_KEY_LEN {
        return Some(format!(
            "Tag keys must be between 1 and {MAX_TAG_KEY_LEN} characters in length."
        ));
    }
    if !key.chars().all(is_tag_character) {
        return Some(tag_charset_message("keys"));
    }
    // Sliced with `get`, which answers `None` rather than panicking when the
    // key's first bytes are the middle of a multi-byte character.
    if key
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
    {
        // The refusal states the RULE — the prefix a caller may not use — and
        // not the key that broke it, which is how the service words it.
        return Some(format!(
            "Tag keys cannot start with the reserved prefix \"{RESERVED_TAG_KEY_PREFIX}\"."
        ));
    }
    None
}

/// What is wrong with a caller-supplied tag VALUE, or `None` when the caller may
/// use it; see [`tag_key_rejection`]. The empty value is permitted — `Tag$Value`
/// "may be null" — and what a non-empty one may contain is the character set
/// [`TAG_SYMBOLS`] states, which the model puts no `@pattern` on.
pub fn tag_value_rejection(value: &str) -> Option<String> {
    // See [`tag_key_rejection`]: the same sentence for the other half of the
    // pair, whose window starts at zero because the empty value is permitted.
    if value.chars().count() > MAX_TAG_VALUE_LEN {
        return Some(format!(
            "Tag values must be between 0 and {MAX_TAG_VALUE_LEN} characters in length."
        ));
    }
    (!value.chars().all(is_tag_character)).then(|| tag_charset_message("values"))
}

/// The tags a request was given as stored pairs, or why they are refused.
///
/// Order is preserved and duplicate keys are kept as sent, so a later
/// `ListTagsForResource` reports what the caller supplied rather than a set the
/// store quietly collapsed.
pub fn tag_pairs(tags: Option<Vec<Tag>>) -> Result<Vec<(String, String)>, TagRefusal> {
    let tags = tags.unwrap_or_default();
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(TagRefusal::TooMany(tags.len()));
    }
    tags.into_iter()
        .map(|tag| {
            // A tag with no `Value` carries the EMPTY value once stored, so
            // every tag this service reports has one.
            let key = tag.key.unwrap_or_default();
            let value = tag.value.unwrap_or_default();
            match tag_key_rejection(&key).or_else(|| tag_value_rejection(&value)) {
                Some(message) => Err(TagRefusal::InvalidTag(message)),
                None => Ok((key, value)),
            }
        })
        .collect()
}

/// The stored pairs as the modelled `TagList` every tagging operation answers
/// with ("a list of tags as key-value pairs"), in the order the store holds
/// them.
pub fn tag_list(tags: &[(String, String)]) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// Merge `requested` into `stored`, which is what `AddTagsToResource` does to a
/// resource's tags: a key the resource does not carry is APPENDED, and a key it
/// does carry takes the new value in place, so re-tagging a key adds no tag and
/// leaves the resource's tag order alone. A key the request does not name is
/// untouched.
pub fn merge_tags(stored: &mut Vec<(String, String)>, requested: Vec<(String, String)>) {
    for (key, value) in requested {
        match stored.iter_mut().find(|(held, _)| *held == key) {
            Some(slot) => slot.1 = value,
            None => stored.push((key, value)),
        }
    }
}

/// `CacheClusterStatus` / `CacheNodeStatus` values this slice uses, from
/// `CacheCluster$CacheClusterStatus` ("available, creating, deleted, deleting,
/// incompatible-network, modifying, rebooting cluster nodes, restore-failed, or
/// snapshotting") and `CacheNode$CacheNodeStatus` ("available, creating,
/// rebooting, or deleting").
pub const STATUS_CREATING: &str = "creating";
/// See [`STATUS_CREATING`].
pub const STATUS_AVAILABLE: &str = "available";
/// See [`STATUS_CREATING`].
pub const STATUS_MODIFYING: &str = "modifying";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "deleting";
/// The `CacheClusterStatus` of a cluster whose nodes `RebootCacheCluster` is
/// rebooting: "during the reboot, the cluster status is set to REBOOTING", and
/// `CacheCluster$CacheClusterStatus`'s own value list spells that status
/// `rebooting cluster nodes`. See [`STATUS_CREATING`].
pub const STATUS_REBOOTING_NODES: &str = "rebooting cluster nodes";
/// The `CacheNode$CacheNodeStatus` of a node being rebooted, from that member's
/// value list ("available, creating, rebooting, or deleting"). Only the nodes
/// `RebootCacheCluster$CacheNodeIdsToReboot` named carry it; the rest keep
/// running.
pub const NODE_STATUS_REBOOTING: &str = "rebooting";
/// See [`STATUS_CREATING`]: the status a cluster carries while a snapshot of it
/// is being taken.
pub const STATUS_SNAPSHOTTING: &str = "snapshotting";
/// The `CacheClusterStatus` of a cluster whose parameter group holds a value its
/// engine cannot run. No operation of this emulator puts a cluster in it — the
/// parameter writers refuse such a value outright — but it is one of the states
/// `RebootCacheCluster` accepts ([`REBOOTABLE_CLUSTER_STATES`]), so the refusal
/// that names them names it too.
pub const STATUS_INCOMPATIBLE_PARAMETERS: &str = "incompatible-parameters";

/// The `CacheClusterStatus` values `RebootCacheCluster` accepts.
///
/// A reboot is not gated on `available` alone: the service states the states it
/// accepts in the refusal itself (see [`reboot_state_refusal`]), and a cluster
/// whose parameters the engine rejected or that is being snapshotted can still be
/// rebooted — which is how a client recovers one.
pub const REBOOTABLE_CLUSTER_STATES: [&str; 3] = [
    STATUS_AVAILABLE,
    STATUS_INCOMPATIBLE_PARAMETERS,
    STATUS_SNAPSHOTTING,
];

/// How ElastiCache refuses a reboot of a cluster that is in none of
/// [`REBOOTABLE_CLUSTER_STATES`]: the refusal states the rule — the states a
/// reboot IS accepted in — and then the cluster and the state it is actually in.
pub fn reboot_state_refusal(cache_cluster_id: &str, status: &str) -> String {
    format!(
        "Can only reboot cache clusters with state in: {}. Cluster {cache_cluster_id} has state: \
         {status}.",
        REBOOTABLE_CLUSTER_STATES.join(", ")
    )
}

/// How ElastiCache refuses a MODIFICATION of a replication group that is not
/// `available`: the message states the requirement — the state the group has to
/// be in and the action it gates — and names neither the group nor the state it
/// is currently in.
///
/// One statement, so the operations that change a group's configuration (the two
/// replica-count operations and `ModifyReplicationGroup`) cannot word their
/// `InvalidReplicationGroupStateFault` differently from one another.
pub fn replication_group_modify_state_refusal() -> String {
    format!("Replication group must be in {STATUS_AVAILABLE} state to modify.")
}

/// How ElastiCache refuses a replica-count change that asks for the count a
/// shard already has: the `NoOperationFault` states that the requested count
/// already matches, naming neither the shard nor the group.
pub const REPLICA_COUNT_UNCHANGED_MESSAGE: &str =
    "Requested new replica count already matches current number of replicas.";

/// How ElastiCache refuses a SCALE-IN of a replication group's shards that does
/// not select which shards it keeps, or that selects them from both sides at
/// once: `NodeGroupsToRemove` and `NodeGroupsToRetain` are the two sides of one
/// selection ("either NodeGroupsToRemove or NodeGroupsToRetain is required"), so
/// a scale-in names exactly one of them.
pub const SCALE_IN_SELECTION_MESSAGE: &str =
    "Node groups to remove or node groups to retain must be specified for a scale-in call but \
     not both.";

/// The `SourceType` values this service records an event under, from the
/// model's `SourceType` enum (`smithy_query(query="shape",shape="SourceType")`:
/// `cache-cluster | cache-parameter-group | cache-security-group |
/// cache-subnet-group | replication-group | serverless-cache |
/// serverless-cache-snapshot | user | user-group`).
///
/// Only the resources this service both persists and changes can be the origin
/// of an event; the rest belong to operations that are still scaffolded 501s
/// and will record their own events when they land, and a published catalog
/// (service updates, reserved-cache-node offerings, the engines' default
/// parameter groups) is never changed by a call, so nothing happens to it to
/// record. A snapshot of a cluster is reported under the CLUSTER it was taken
/// from, which is the source the event happened to — the enum has no member
/// for a cache-cluster snapshot.
pub const SOURCE_TYPE_CACHE_CLUSTER: &str = "cache-cluster";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_CACHE_SUBNET_GROUP: &str = "cache-subnet-group";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_SERVERLESS_CACHE: &str = "serverless-cache";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`]. Unlike a cluster's snapshot, a serverless
/// cache's snapshot IS an event source of its own in the enum, so what happens
/// to one is recorded under the snapshot rather than under the cache it was
/// taken from.
pub const SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT: &str = "serverless-cache-snapshot";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_USER: &str = "user";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_REPLICATION_GROUP: &str = "replication-group";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_CACHE_PARAMETER_GROUP: &str = "cache-parameter-group";

/// The window `DescribeEvents` reports when the request bounds neither end of
/// it, as the operation documents: "By default, only the events occurring
/// within the last hour are returned."
pub const DEFAULT_EVENT_WINDOW: Duration = Duration::from_secs(60 * 60);

/// How long a recorded event is kept, and so the furthest back
/// `DescribeEvents` can look, as the operation documents: "you can retrieve up
/// to 14 days' worth of events if necessary."
pub const EVENT_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// What `Snapshot$SnapshotSource` reports — "whether the snapshot is from an
/// automatic backup (`automated`) or was created manually (`manual`)".
///
/// `CreateSnapshot` is the manual path. No `automated` snapshot is ever stored:
/// ElastiCache takes one during the source cluster's daily `SnapshotWindow`, so
/// the first one falls due a day after the cluster was created, and a cluster
/// whose `SnapshotRetentionLimit` is 0 has "automatic backups disabled" and is
/// never owed one at all.
pub const SNAPSHOT_SOURCE_MANUAL: &str = "manual";
/// See [`SNAPSHOT_SOURCE_MANUAL`].
pub const SNAPSHOT_SOURCE_AUTOMATED: &str = "automated";

/// The values `DescribeSnapshots$SnapshotSource` filters on: "If set to
/// `system`, the output shows snapshots that were automatically created by
/// ElastiCache. If set to `user` the output shows snapshots that were manually
/// created." They are the FILTER's spellings, not the ones
/// [`SNAPSHOT_SOURCE_MANUAL`] / [`SNAPSHOT_SOURCE_AUTOMATED`] are reported
/// under.
pub const SNAPSHOT_SOURCE_FILTER_SYSTEM: &str = "system";
/// See [`SNAPSHOT_SOURCE_FILTER_SYSTEM`].
pub const SNAPSHOT_SOURCE_FILTER_USER: &str = "user";

/// The stored snapshot source a `DescribeSnapshots$SnapshotSource` value
/// selects, or the reason it names no creation source.
///
/// `SnapshotSource` is a plain string with NO modelled enum or pattern, so the
/// vocabulary is enforced here, and it is a SUPERSET of the one the response
/// emits: each source is named either by the filter's spelling the member
/// documents ([`SNAPSHOT_SOURCE_FILTER_SYSTEM`] / [`SNAPSHOT_SOURCE_FILTER_USER`])
/// or by the spelling `Snapshot$SnapshotSource` reports it under
/// ([`SNAPSHOT_SOURCE_AUTOMATED`] / [`SNAPSHOT_SOURCE_MANUAL`]). That is what
/// makes "read a snapshot's `SnapshotSource`, then list only the snapshots of
/// that source" a round trip rather than a refusal. Matched ignoring ASCII
/// case, as this service's other unconstrained filter vocabularies are (see
/// [`canonical_offering_type`]): a creation source is a vocabulary word, not an
/// identity.
pub fn snapshot_source_filter(value: &str) -> Result<&'static str, String> {
    for (filter_spelling, reported_spelling) in [
        (SNAPSHOT_SOURCE_FILTER_SYSTEM, SNAPSHOT_SOURCE_AUTOMATED),
        (SNAPSHOT_SOURCE_FILTER_USER, SNAPSHOT_SOURCE_MANUAL),
    ] {
        if value.eq_ignore_ascii_case(filter_spelling)
            || value.eq_ignore_ascii_case(reported_spelling)
        {
            return Ok(reported_spelling);
        }
    }
    Err(format!(
        "Invalid value {value} for SnapshotSource: must be one of \
         {SNAPSHOT_SOURCE_FILTER_SYSTEM} | {SNAPSHOT_SOURCE_AUTOMATED} | \
         {SNAPSHOT_SOURCE_FILTER_USER} | {SNAPSHOT_SOURCE_MANUAL}."
    ))
}

/// The node type ElastiCache cannot snapshot a Valkey / Redis OSS cluster on,
/// from `SnapshotFeatureNotSupportedFault`: "Creating a snapshot of a Valkey or
/// Redis OSS cluster running on a `cache.t1.micro` cache node" is not
/// supported.
pub const UNSNAPSHOTTABLE_NODE_TYPE: &str = "cache.t1.micro";

/// The `ParameterApplyStatus` / `CacheNode$ParameterGroupStatus` of a cache
/// node that runs the current values of the parameter group its cluster is
/// associated with.
pub const PARAMETER_APPLY_STATUS: &str = "in-sync";

/// The `ParameterApplyStatus` / `CacheNode$ParameterGroupStatus` of a cache
/// node whose group has had a parameter the engine applies only at a restart
/// changed on it: the value is SET on the group, and the node keeps running the
/// one it was started with until it is rebooted.
///
/// `Parameter$ChangeType` is what says which parameters those are
/// ([`CHANGE_TYPE_REQUIRES_REBOOT`]), and `RebootCacheCluster` — which "applies
/// any modified cache parameter groups to the cluster" — is what ends it.
pub const PARAMETER_APPLY_STATUS_PENDING_REBOOT: &str = "pending-reboot";

/// The `SecurityGroupMembership$Status` of a VPC security group a cluster was
/// created with. The emulator models no EC2 security-group lifecycle, so a
/// membership is reported in the one state a created cluster's membership has.
pub const SECURITY_GROUP_STATUS: &str = "active";

/// `NotificationTopicStatus`'s two documented values: "The status of the Amazon
/// SNS notification topic … Notifications are sent only if the status is active.
/// Valid values: active | inactive." The member is a plain `String`, so these
/// are the spellings a request is held to, and the status is held PER CLUSTER
/// ([`CacheClusterRecord::notification_topic_status`]) because a modify moves a
/// configured topic between the two without detaching it.
pub const NOTIFICATION_STATUS_ACTIVE: &str = "active";
/// See [`NOTIFICATION_STATUS_ACTIVE`].
pub const NOTIFICATION_STATUS_INACTIVE: &str = "inactive";

/// The `LogDeliveryConfiguration$Status` of a log delivery this emulator holds,
/// from `LogDeliveryConfigurationStatus` (`active | enabling | modifying |
/// disabling | error`): a stored configuration is the settled one, and the
/// emulator delivers no logs for it to error on.
pub const LOG_DELIVERY_STATUS: &str = "active";

/// What `CacheCluster$PreferredAvailabilityZone` reports for a cluster whose
/// nodes are not all in one zone: "the name of the Availability Zone in which
/// the cluster is located or "Multiple" if the cache nodes are located in
/// different Availability Zones".
pub const MULTIPLE_AVAILABILITY_ZONES: &str = "Multiple";

/// `AZMode`'s values (`smithy_query(query="shape", shape="AZMode")`:
/// `single-az | cross-az`). `cross-az` is what spreads NEW nodes over zones.
pub const AZ_MODE_CROSS: &str = "cross-az";

/// The engine version a cluster runs when `CreateCacheCluster` names none: the
/// newest version this service publishes for that engine.
///
/// Read out of the ONE catalog `DescribeCacheEngineVersions` reports
/// ([`CACHE_ENGINE_VERSIONS`]) rather than from a second list beside it, so a
/// cluster is never created on a version the service does not say it supports.
/// An engine the catalog publishes nothing for falls back to
/// [`DEFAULT_ENGINE`]'s newest version, which is unreachable for the engines in
/// [`ENGINES`] — the only ones a request gets past [`normalize_engine`] with.
pub fn default_engine_version(engine: &str) -> &'static str {
    latest_published_version(engine)
        .or_else(|| latest_published_version(DEFAULT_ENGINE))
        .unwrap_or_default()
}

/// The port a cluster's nodes listen on when `CreateCacheCluster` names none:
/// the engine's own default port (Memcached 11211, Valkey / Redis OSS 6379).
pub fn default_port(engine: &str) -> i32 {
    match engine {
        "memcached" => 11211,
        _ => 6379,
    }
}

/// Every cache parameter group family this service publishes.
///
/// The order here is the catalog's own; what a describe pages is the STORED key
/// order of
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::cache_parameter_groups`].
///
/// Source: `smithy_query(query="doc", shape="CreateCacheParameterGroupMessage",
/// member="CacheParameterGroupFamily")` — "Valid values are: valkey8 | valkey7
/// | memcached1.4 | memcached1.5 | memcached1.6 | redis2.6 | redis2.8 |
/// redis3.2 | redis4.0 | redis5.0 | redis6.x | redis7". A family's name is the
/// engine followed by its version line, which is why the line is `<major>` for
/// Valkey and current Redis OSS, `<major>.<minor>` for Memcached and the older
/// Redis OSS lines, and `6.x` for Redis OSS 6.
///
/// `valkey9` is published beside them: the member's value list above does not
/// name the current Valkey generation yet, while the default parameter groups
/// ElastiCache answers `DescribeCacheParameterGroups` with do carry
/// `default.valkey9` and `default.valkey9.cluster.on`. A family is published
/// exactly when the engine line behind it is, and [`CACHE_ENGINE_VERSIONS`]
/// offers that line.
pub const PARAMETER_GROUP_FAMILIES: [&str; 13] = [
    "memcached1.4",
    "memcached1.5",
    "memcached1.6",
    "redis2.6",
    "redis2.8",
    "redis3.2",
    "redis4.0",
    "redis5.0",
    "redis6.x",
    "redis7",
    "valkey7",
    "valkey8",
    "valkey9",
];

/// The name prefix of the parameter group AWS itself provides for a family:
/// `default.<family>` (`DeleteCacheParameterGroup`: "You cannot delete the
/// default cache parameter groups in your account.").
pub const DEFAULT_PARAMETER_GROUP_PREFIX: &str = "default.";

/// The name suffix of the CLUSTER-MODE-ON default parameter group of a family:
/// `default.<family>.cluster.on`, the group `CreateReplicationGroup`'s own
/// documentation names ("To create a Valkey or Redis OSS (cluster mode enabled)
/// replication group, use CacheParameterGroupName=default.redis3.2.cluster.on").
pub const CLUSTER_MODE_ON_SUFFIX: &str = ".cluster.on";

/// The parameter whose value IS cluster mode, and the value that turns it on.
///
/// A family publishes it from the engine version that introduced cluster mode
/// onward, which is what makes the parameter the one statement of which families
/// have a cluster-mode-on default group — see [`family_publishes_cluster_mode`].
pub const CLUSTER_ENABLED_PARAMETER: &str = "cluster-enabled";
/// See [`CLUSTER_ENABLED_PARAMETER`].
pub const CLUSTER_ENABLED_ON: &str = "yes";

/// The published parameter group family `engine_version` of `engine` belongs
/// to, or `None` when no published family covers that version.
///
/// A published family covers a version line, so the version is matched against
/// the lines in [`PARAMETER_GROUP_FAMILIES`] from the most specific to the
/// least: `<major>.<minor>` (Memcached 1.6.22 is `memcached1.6`), then
/// `<major>.x` (Redis OSS 6.2 is `redis6.x`), then `<major>` (Redis OSS 7.1 is
/// `redis7`, Valkey 8.0 is `valkey8`).
///
/// This is also the one statement of which family a version in the published
/// engine-version catalog reports — see [`published_cache_engine_versions`].
pub fn parameter_group_family(engine: &str, engine_version: &str) -> Option<&'static str> {
    let mut parts = engine_version.split('.');
    let major = parts.next().unwrap_or_default();
    let minor = parts.next().unwrap_or("0");
    let candidates = [
        format!("{engine}{major}.{minor}"),
        format!("{engine}{major}.x"),
        format!("{engine}{major}"),
    ];
    candidates.iter().find_map(|candidate| {
        PARAMETER_GROUP_FAMILIES
            .iter()
            .find(|family| **family == candidate.as_str())
            .copied()
    })
}

/// The parameter group a cluster is associated with when the request names
/// none: "the default parameter group for the specified engine"
/// (`CacheParameterGroupName`) — the default group of the version's
/// [`parameter_group_family`], or `None` when no published family covers the
/// version, in which case there is no default group for it to join.
pub fn default_parameter_group_name(engine: &str, engine_version: &str) -> Option<String> {
    parameter_group_family(engine, engine_version)
        .map(|family| format!("{DEFAULT_PARAMETER_GROUP_PREFIX}{family}"))
}

/// True when `family` publishes the cluster-mode parameter, and so has a
/// cluster-mode-on default group beside its plain one.
///
/// Read off the family's OWN parameter catalog rather than from a second list of
/// family names: a family carries [`CLUSTER_ENABLED_PARAMETER`] exactly when its
/// line covers the engine version that introduced cluster mode, so the groups
/// this service holds and the parameters they report cannot disagree about which
/// families can run cluster mode.
pub fn family_publishes_cluster_mode(family: &str) -> bool {
    family_default_parameters(family).is_some_and(|parameters| {
        parameters
            .values()
            .any(|parameter| parameter.name == CLUSTER_ENABLED_PARAMETER)
    })
}

/// The default parameter groups an account holds the moment ElastiCache becomes
/// available in it: one per published family, plus the CLUSTER-MODE-ON default
/// of every family that can run cluster mode — which is why
/// `DescribeCacheParameterGroups` answers with groups before anything has been
/// created. Seeded into
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::cache_parameter_groups`]
/// when the store is created, so the groups a read reports and the
/// [`default_parameter_group_name`] a `CreateCacheCluster` applies cannot
/// disagree.
///
/// A family's two defaults differ by ONE value: the cluster-mode-on group holds
/// [`CLUSTER_ENABLED_ON`] for [`CLUSTER_ENABLED_PARAMETER`] and takes the
/// family's engine defaults for everything else, which is what makes it a
/// customised copy of the plain default rather than a second catalog.
pub fn default_parameter_groups() -> BTreeMap<String, CacheParameterGroupRecord> {
    let mut groups = BTreeMap::new();
    for family in PARAMETER_GROUP_FAMILIES {
        let plain = CacheParameterGroupRecord {
            name: format!("{DEFAULT_PARAMETER_GROUP_PREFIX}{family}"),
            family: family.to_string(),
            // The description AWS gives a family's own default group.
            description: format!("Default parameter group for {family}"),
            // Nothing tags a group the service itself provides.
            tags: Vec::new(),
            // A family default starts out being exactly its family's engine
            // defaults: nothing has set a value on it.
            parameter_values: BTreeMap::new(),
        };
        if family_publishes_cluster_mode(family) {
            let cluster_on = CacheParameterGroupRecord {
                name: format!("{}{CLUSTER_MODE_ON_SUFFIX}", plain.name),
                family: family.to_string(),
                description: format!(
                    "Customized default parameter group for {family} with cluster mode on"
                ),
                tags: Vec::new(),
                parameter_values: BTreeMap::from([(
                    CLUSTER_ENABLED_PARAMETER.to_string(),
                    CLUSTER_ENABLED_ON.to_string(),
                )]),
            };
            groups.insert(cluster_on.name.clone(), cluster_on);
        }
        groups.insert(plain.name.clone(), plain);
    }
    groups
}

/// One cache parameter group, as this emulator persists it: a family default
/// the store comes up holding, or a custom one `CreateCacheParameterGroup`
/// created.
///
/// Every group is one an account holds in the Region it is read from — a family
/// default is provided in every Region, and a custom one is created in the
/// Region of the create — so the ARN a read reports names the Region of the
/// request reading it; see [`CacheParameterGroupRecord::to_modeled`].
#[derive(Debug, Clone)]
pub struct CacheParameterGroupRecord {
    /// The group's identity, which "is stored as a lowercase string"
    /// (`CreateCacheParameterGroup$CacheParameterGroupName`).
    pub name: String,
    /// One of [`PARAMETER_GROUP_FAMILIES`] (`CacheParameterGroupFamily`).
    pub family: String,
    /// `CacheParameterGroup$Description`.
    pub description: String,
    /// The tags `CreateCacheParameterGroup` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    /// The value SET on this group for a parameter, keyed by
    /// `Parameter$ParameterName`.
    ///
    /// A group holds only the values it DIFFERS from its family's engine
    /// defaults by, never a copy of the catalog: that is what makes a group's
    /// effective parameter list "the family defaults with these laid over
    /// them", and what makes a parameter's `Parameter$Source` derivable — a
    /// name in this map is the customer's value ([`PARAMETER_SOURCE_USER`]),
    /// one that is not is the service's ([`PARAMETER_SOURCE_SYSTEM`]).
    /// `ModifyCacheParameterGroup` sets an entry and
    /// `ResetCacheParameterGroup` removes one, which puts the parameter back to
    /// the engine's own default.
    pub parameter_values: BTreeMap<String, String>,
}

impl CacheParameterGroupRecord {
    /// The value set on this group for the parameter `name`, or `None` when
    /// this group reports the engine's own default for it; see
    /// [`Self::parameter_values`].
    pub fn set_value(&self, name: &str) -> Option<&str> {
        self.parameter_values.get(name).map(String::as_str)
    }

    /// True when this group turns cluster mode ON: the value it holds for
    /// [`CLUSTER_ENABLED_PARAMETER`] is [`CLUSTER_ENABLED_ON`].
    ///
    /// Read off the group's own parameter value rather than off its NAME, so a
    /// group's reported parameters and what naming it does cannot disagree.
    pub fn cluster_mode_on(&self) -> bool {
        self.set_value(CLUSTER_ENABLED_PARAMETER) == Some(CLUSTER_ENABLED_ON)
    }

    /// This record as the modelled `CacheParameterGroup`, read from `region`.
    pub fn to_modeled(&self, region: &str) -> CacheParameterGroup {
        CacheParameterGroup {
            cache_parameter_group_name: Some(self.name.clone()),
            cache_parameter_group_family: Some(self.family.clone()),
            description: Some(self.description.clone()),
            // "Indicates whether the parameter group is associated with a
            // Global datastore": a global datastore is a global replication
            // group, and every global-replication-group operation in this
            // service is still a scaffolded 501, so no group is associated with
            // one.
            is_global: Some(false),
            arn: Some(parameter_group_arn(region, &self.name)),
        }
    }
}

/// The Availability Zone the emulator places the `index`-th subnet of a subnet
/// group, or a cluster that named no `PreferredAvailabilityZone`, in
/// ("Default: System chosen Availability Zone").
///
/// The emulator models no EC2 Availability Zones, so it names them the way AWS
/// does — the region followed by a letter — and spreads a subnet group's subnets
/// over the first few. The chosen zone is STORED on the record, so every later
/// read reports the same one.
pub fn availability_zone(region: &str, index: usize) -> String {
    // Six letters is more zones than any emulated workflow needs and keeps the
    // name inside the `<region><letter>` form AWS uses.
    let letter = char::from(b'a' + u8::try_from(index % 6).unwrap_or(0));
    format!("{region}{letter}")
}

/// True when `value` has the form of an EC2 subnet identifier: `subnet-` and at
/// least one alphanumeric character.
///
/// `SubnetIdentifier` carries no modelled constraint, so the boundary validator
/// passes any string; an id that cannot name a subnet is refused here with the
/// modelled `InvalidSubnet`.
pub fn is_subnet_id(value: &str) -> bool {
    value
        .strip_prefix("subnet-")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// One subnet of the account's EC2 network in one Region: the identifier a
/// request names, the VPC it belongs to and the Availability Zone it sits in.
///
/// Every field is read off the record Amazon EC2 — the service that OWNS subnets
/// — answered with; see [`ec2_subnets`], which is the only thing that builds one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountSubnet {
    /// The EC2 subnet identifier, as a request names it.
    pub subnet_id: String,
    /// The VPC this subnet belongs to.
    pub vpc_id: String,
    /// The Availability Zone this subnet sits in.
    pub availability_zone: String,
    /// Whether EC2 reports an IPv4 CIDR block for this subnet.
    pub has_ipv4_cidr: bool,
    /// Whether EC2 reports an IPv6 CIDR block for this subnet.
    pub has_ipv6_cidr: bool,
}

/// How ElastiCache words a request naming subnets the account does not have,
/// built from the offending ids: `Some input subnets in :[<ids>] are invalid.`
pub fn invalid_subnets_message(subnet_ids: &[&str]) -> String {
    format!("Some input subnets in :[{}] are invalid.", subnet_ids.join(", "))
}

/// The length bound a LOOKUP applies to an identifier before resolving it:
/// ElastiCache's own identifier bound (`CacheClusterId`: "A name must contain
/// from 1 to 50 alphanumeric characters or hyphens"), which is also the bound
/// the service states when it refuses an over-long `ReplicationGroupId` on a
/// describe ("Replication Group ID cannot be longer than 50 characters").
///
/// A CREATE applies the stricter bound its own member documents — see
/// [`REPLICATION_GROUP_ID_MAX_LEN`], 40 characters, because creating a
/// replication group also mints the member cluster ids from the group id — so
/// an id between the two bounds is refused by a create and merely not found by
/// a lookup.
pub const LOOKUP_IDENTIFIER_MAX_LEN: usize = CLUSTER_ID_MAX_LEN;

/// Why `value` cannot name a resource of this service, or `None` when it can:
/// the refusal a lookup answers BEFORE it resolves the identifier, so a value
/// that no resource could ever carry is reported as the bad parameter it is
/// rather than as a resource that does not exist.
///
/// `prose` is how the service names the member in the length refusal
/// ("Replication Group ID"), `parameter` how it names it in the identifier-rule
/// refusal ("ReplicationGroupIdentifier"); both texts are the service's own.
pub fn identifier_refusal(
    prose: &str,
    parameter: &str,
    value: &str,
    max_len: usize,
) -> Option<String> {
    if value.len() > max_len {
        return Some(format!("{prose} cannot be longer than {max_len} characters."));
    }
    if is_valid_identifier(value, max_len) {
        return None;
    }
    Some(format!(
        "The parameter {parameter} is not a valid identifier. Identifiers must begin with a \
         letter; must contain only ASCII letters, digits, and hyphens; and must not end with a \
         hyphen or contain two consecutive hyphens."
    ))
}

/// How `CreateSnapshot` words the cache cluster it is asked to snapshot when
/// this account holds no such cluster: `Cache cluster not found: <id>`, the
/// resource named as PROSE rather than as the shape.
///
/// The snapshot creator words this fault its own way, as the two writers of
/// parameter values word theirs (see
/// [`parameter_writer_group_not_found_message`]); the id is reported as the
/// request spelled it, because the message is built from the request.
pub fn snapshot_source_cluster_not_found_message(cache_cluster_id: &str) -> String {
    format!("Cache cluster not found: {cache_cluster_id}")
}

/// How ElastiCache words a cache cluster this account does not hold:
/// `CacheCluster not found: <id>`.
///
/// One helper, so every surface that reports a missing cache cluster — the
/// modelled `CacheClusterNotFoundFault` of a describe, modify, delete, reboot or
/// snapshot call, and the `ErrorMessage` a batch update action reports for a
/// resource it could not resolve — words it the one way the service does.
pub fn cache_cluster_not_found_message(cache_cluster_id: &str) -> String {
    format!("CacheCluster not found: {cache_cluster_id}")
}

/// How ElastiCache words a replication group this account does not hold:
/// `ReplicationGroup <id> not found.`
///
/// One helper, so every surface that answers `ReplicationGroupNotFoundFault` for
/// an id it could not resolve — the describe, the modifies, the failover test,
/// the creates that join a group, and the `ErrorMessage` a batch update action
/// reports — words it the one way the service does. The resource is named the way
/// the SHAPE spells it (`ReplicationGroup`), and the id is reported as the
/// request spelled it, because the message is built from the request.
pub fn replication_group_not_found_message(replication_group_id: &str) -> String {
    format!("ReplicationGroup {replication_group_id} not found.")
}

/// How ElastiCache words a global replication group (a Global datastore) this
/// account does not hold.
///
/// The template names NO identifier: the service states the class of resource
/// that could not be resolved and nothing else, so interpolating the requested id
/// would change the string for every caller.
pub const GLOBAL_REPLICATION_GROUP_NOT_FOUND_MESSAGE: &str = "Global Replication Group not found";

/// How ElastiCache words a `CreateReplicationGroup` whose `ReplicationGroupId` is
/// already taken. Like [`SNAPSHOT_ALREADY_EXISTS_MESSAGE`], the template names the
/// colliding name only as "specified name" rather than interpolating it.
pub const REPLICATION_GROUP_ALREADY_EXISTS_MESSAGE: &str =
    "Replication group with specified name already exists.";

/// How ElastiCache words a snapshot name that is already taken, whether the
/// snapshot would be taken from a cluster or copied from another snapshot; see
/// [`REPLICATION_GROUP_ALREADY_EXISTS_MESSAGE`].
pub const SNAPSHOT_ALREADY_EXISTS_MESSAGE: &str = "Snapshot with specified name already exists.";

/// How ElastiCache words a service update its published catalogue does not hold:
/// `ServiceUpdate <name> not found.`
///
/// One helper for the describe and the two batch update-action operations, so the
/// three cannot disagree. The resource is named the way the SHAPE spells it
/// (`ServiceUpdate`), not as English prose.
pub fn service_update_not_found_message(service_update_name: &str) -> String {
    format!("ServiceUpdate {service_update_name} not found.")
}

/// How ElastiCache words a `CacheParameterGroupName` that is already taken:
/// `Parameter group <name> already exists` — no trailing stop.
pub fn cache_parameter_group_already_exists_message(name: &str) -> String {
    format!("Parameter group {name} already exists")
}

/// How ElastiCache words a cache parameter group this account does not hold:
/// `CacheParameterGroup <name> not found.`
///
/// One helper, so every surface that resolves a `CacheParameterGroupName` — the
/// describes, the delete, and the creates and modifies that take a group for a
/// cluster or a replication group — words its `CacheParameterGroupNotFoundFault`
/// the one way the service does. The name is reported as the request spelled it,
/// because the message is built from the request.
///
/// The two operations that WRITE parameter values word the same fault the other
/// way round; see [`parameter_writer_group_not_found_message`].
pub fn cache_parameter_group_not_found_message(name: &str) -> String {
    format!("CacheParameterGroup {name} not found.")
}

/// How `ModifyCacheParameterGroup` and `ResetCacheParameterGroup` word a cache
/// parameter group this account does not hold: `CacheParameterGroup not found:
/// <name>`, the form the service's parameter writers answer with (and the form
/// [`cache_cluster_not_found_message`] carries).
///
/// One helper, so the two writers of parameter values cannot disagree about it.
pub fn parameter_writer_group_not_found_message(name: &str) -> String {
    format!("CacheParameterGroup not found: {name}")
}

/// How ElastiCache words a `CacheSubnetGroupName` that breaks the constraint the
/// member documents ("Must contain no more than 255 alphanumeric characters or
/// hyphens"), built from the value the request sent.
///
/// One helper, so the create and the delete of a cache subnet group state the one
/// rule the one way; the form the name must take is
/// [`is_valid_subnet_group_name`].
pub fn invalid_subnet_group_name_message(value: &str) -> String {
    format!(
        "Invalid value {value} for CacheSubnetGroupName: must contain no more than 255 \
         alphanumeric characters or hyphens."
    )
}

/// How ElastiCache words a request whose `SubnetIds` do not all belong to one
/// VPC: `Subnets <first> and <other> are not in the same VPC.`
///
/// The fixed text is `Subnets `, ` and ` and ` are not in the same VPC.`; the two
/// variable parts are the identifiers that disagree, in the order the request
/// listed them — the first subnet it named, whose VPC the group would be placed
/// in, and the first one belonging to another VPC.
///
/// One helper for the two operations that build a cache subnet group out of
/// subnet ids, so they cannot word one refusal two ways. `CreateServerlessCache`
/// states the same rule in the sentence ITS own member documents ("All the
/// subnetIds must belong to the same VPC"), which is why it is not this
/// template.
pub fn subnets_not_in_one_vpc_message(first: &str, other: &str) -> String {
    format!("Subnets {first} and {other} are not in the same VPC.")
}

/// The stored identity of a `CacheSubnetGroupName` the request sent.
///
/// "This value is stored as a lowercase string"
/// (`CreateCacheSubnetGroup$CacheSubnetGroupName`), so the name has ONE identity
/// whatever case a request spells it in: that is what a group is stored and
/// looked up under, and what the service names the group by — in a response and
/// in the message of a refusal alike. A refusal that echoed the request's
/// spelling instead would name a group under a name no read of this service ever
/// reports.
pub fn stored_subnet_group_name(value: &str) -> String {
    value.to_ascii_lowercase()
}

/// How ElastiCache words a cache subnet group that cannot be deleted because a
/// cluster is placed in it: `Cache subnet group <name> is currently in use.`
///
/// The one variable part is the group's stored name
/// ([`stored_subnet_group_name`]), as in
/// [`delete_subnet_group_not_found_message`] — the two refusals of one delete
/// name the group the one way.
pub fn subnet_group_in_use_message(name: &str) -> String {
    format!("Cache subnet group {name} is currently in use.")
}

/// How ElastiCache words a `CacheSubnetGroupName` that names no cache subnet
/// group of this account to an operation that would PLACE a resource in it:
/// `Cache subnet group '<name>' does not exist.`
///
/// The fixed text is `Cache subnet group '` and `' does not exist.`; the one
/// variable part is the group's stored name ([`stored_subnet_group_name`]),
/// QUOTED, because a client that named several resources in one call reads the
/// missing one out of the quotes.
///
/// One helper, so every create that places a resource in a cache subnet group —
/// a cache cluster, a replication group — words its
/// `CacheSubnetGroupNotFoundFault` the one way the service does. The operations
/// that address the GROUP itself word the same fault their own way: the describe
/// answers `Cache subnet group <name> not found.` and the delete
/// [`delete_subnet_group_not_found_message`], so the three cannot be served by
/// one template.
pub fn placement_subnet_group_not_found_message(name: &str) -> String {
    format!("Cache subnet group '{name}' does not exist.")
}

/// How ElastiCache words a cache subnet group a WRITE cannot find:
/// `Cache Subnet Group <name> does not exist.`
///
/// The fixed text is `Cache Subnet Group ` and ` does not exist.`; the one
/// variable part is the group's stored name ([`stored_subnet_group_name`]). One
/// helper for the two operations that address the group itself by name and change
/// it (`DeleteCacheSubnetGroup`, `ModifyCacheSubnetGroup`), so they cannot word
/// one fault two ways; the describe that resolves the same member answers `Cache
/// subnet group <name> not found.`, which is its own wording.
pub fn delete_subnet_group_not_found_message(name: &str) -> String {
    format!("Cache Subnet Group {name} does not exist.")
}

/// How ElastiCache words a `CacheSubnetGroupName` that names no cache subnet
/// group of this account to the DESCRIBE of groups: `Cache subnet group <name>
/// not found.`
///
/// The one variable part is the group's stored name
/// ([`stored_subnet_group_name`]). The sibling wordings of the same fault are
/// [`placement_subnet_group_not_found_message`] and
/// [`delete_subnet_group_not_found_message`].
pub fn describe_subnet_group_not_found_message(name: &str) -> String {
    format!("Cache subnet group {name} not found.")
}

/// How ElastiCache refuses a change to a family's OWN default parameter group,
/// whether the request asks to set a value or to put one back: the group's
/// values are the engine's, so there is no change to make. Stated once, so the
/// two writers of parameter values cannot disagree about it.
pub const DEFAULT_PARAMETER_GROUP_IMMUTABLE: &str = "Cannot modify a default parameter group.";

/// How ElastiCache words a `CacheParameterGroupFamily` that names none of the
/// families it publishes: `CacheParameterGroupFamily <family> is not a valid
/// parameter group family.`
///
/// The service answers the RULE the value broke — the family vocabulary is the
/// service's own, so a name outside it names no catalog — rather than listing
/// the families back. The requested family is the one variable part, taken from
/// the request. One helper, so every operation resolving a family (the engine
/// defaults describe and the group create) words it the one way the service
/// does; the published vocabulary itself is [`PARAMETER_GROUP_FAMILIES`].
pub fn invalid_parameter_group_family_message(family: &str) -> String {
    format!("CacheParameterGroupFamily {family} is not a valid parameter group family.")
}

/// How ElastiCache words a snapshot this account does not hold: the name the
/// request asked for, which is all the service knows about a snapshot that does
/// not exist.
///
/// One helper, so every operation that acts on a NAMED snapshot
/// (`DeleteSnapshot`, `CopySnapshot`, and the tag operations when they land)
/// words its `SnapshotNotFoundFault` the one way the service does. The name is
/// reported as the caller sent it rather than lowercased, because the message is
/// built from the request.
pub fn snapshot_not_found_message(snapshot_name: &str) -> String {
    format!("Snapshot with name {snapshot_name} not found")
}

/// The ARN of a cache cluster: `arn:aws:elasticache:<region>:<account>:cluster:<id>`.
pub fn cluster_arn(region: &str, cache_cluster_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("cluster:{cache_cluster_id}"))
}

/// The ARN of a cache subnet group:
/// `arn:aws:elasticache:<region>:<account>:subnetgroup:<name>`.
pub fn subnet_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("subnetgroup:{name}"))
}

/// The ARN of a cache parameter group:
/// `arn:aws:elasticache:<region>:<account>:parametergroup:<name>`.
pub fn parameter_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("parametergroup:{name}"))
}

/// The ARN of a snapshot: `arn:aws:elasticache:<region>:<account>:snapshot:<name>`.
pub fn snapshot_arn(region: &str, snapshot_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("snapshot:{snapshot_name}"))
}

/// The kind of resource an ElastiCache ARN names, which is what decides WHICH
/// not-found fault a tagging call answers with for an ARN this store holds no
/// resource for: each of `AddTagsToResource`, `ListTagsForResource` and
/// `RemoveTagsFromResource` models one per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    CacheCluster,
    CacheParameterGroup,
    CacheSecurityGroup,
    CacheSubnetGroup,
    GlobalReplicationGroup,
    ReplicationGroup,
    ReservedCacheNode,
    ServerlessCache,
    ServerlessCacheSnapshot,
    Snapshot,
    User,
    UserGroup,
}

impl TaggedResourceKind {
    /// How this kind is named in the message of the fault it answers with.
    pub fn label(self) -> &'static str {
        match self {
            Self::CacheCluster => "Cache cluster",
            Self::CacheParameterGroup => "Cache parameter group",
            Self::CacheSecurityGroup => "Cache security group",
            Self::CacheSubnetGroup => "Cache subnet group",
            Self::GlobalReplicationGroup => "Global datastore",
            Self::ReplicationGroup => "Replication group",
            Self::ReservedCacheNode => "Reserved cache node",
            Self::ServerlessCache => "Serverless cache",
            Self::ServerlessCacheSnapshot => "Serverless cache snapshot",
            Self::Snapshot => "Snapshot",
            Self::User => "User",
            Self::UserGroup => "User group",
        }
    }
}

/// The resource-type segment an ElastiCache ARN carries for each kind.
///
/// Every segment this service's ARN helpers mint ([`cluster_arn`],
/// [`subnet_group_arn`], [`parameter_group_arn`], [`snapshot_arn`],
/// [`replication_group_arn`], [`reservation_arn`], [`serverless_cache_arn`],
/// [`serverless_cache_snapshot_arn`], [`user_arn`], [`user_group_arn`],
/// [`global_replication_group_arn`]), plus the one kind this emulator mints no
/// resource of yet — the EC2-Classic cache security group. A segment in neither
/// list names no ElastiCache resource at all, which is what [`parse_resource_arn`]
/// answers `None` for.
const RESOURCE_TYPE_SEGMENTS: &[(&str, TaggedResourceKind)] = &[
    ("cluster", TaggedResourceKind::CacheCluster),
    ("parametergroup", TaggedResourceKind::CacheParameterGroup),
    ("securitygroup", TaggedResourceKind::CacheSecurityGroup),
    ("subnetgroup", TaggedResourceKind::CacheSubnetGroup),
    (
        "globalreplicationgroup",
        TaggedResourceKind::GlobalReplicationGroup,
    ),
    ("replicationgroup", TaggedResourceKind::ReplicationGroup),
    ("reserved-instance", TaggedResourceKind::ReservedCacheNode),
    ("serverlesscache", TaggedResourceKind::ServerlessCache),
    (
        "serverlesscachesnapshot",
        TaggedResourceKind::ServerlessCacheSnapshot,
    ),
    ("snapshot", TaggedResourceKind::Snapshot),
    ("user", TaggedResourceKind::User),
    ("usergroup", TaggedResourceKind::UserGroup),
];

/// What an ElastiCache resource ARN carries: the Region it names the resource
/// in, the kind of resource, and the resource's own name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceArnParts<'a> {
    pub region: &'a str,
    pub kind: TaggedResourceKind,
    pub name: &'a str,
}

impl ResourceArnParts<'_> {
    /// The rejection an ARN of this kind that names no stored resource is
    /// answered with: the kind decides which modelled not-found fault the
    /// calling operation lifts it into.
    pub fn not_found(&self) -> TagTargetRejection {
        TagTargetRejection::NotFound(self.kind, format!("{} is not present", self.name))
    }
}

/// The parts of an ElastiCache resource ARN, or `None` when `resource_arn` is
/// not one.
///
/// The shape is the one every ARN this service mints has (see [`cluster_arn`]):
/// `arn:<partition>:elasticache:<region>:<account>:<type>:<name>`, with a
/// non-empty partition, Region and account, a resource type from
/// [`RESOURCE_TYPE_SEGMENTS`] and a non-empty resource name. The service field
/// must be this service's ([`ARN_NAMESPACE`]): another service's ARN names no
/// ElastiCache resource.
pub fn parse_resource_arn(resource_arn: &str) -> Option<ResourceArnParts<'_>> {
    let mut fields = resource_arn.splitn(6, ':');
    let prefix = fields.next()?;
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    if prefix != "arn"
        || partition.is_empty()
        || service != ARN_NAMESPACE
        || region.is_empty()
        || account.is_empty()
    {
        return None;
    }
    let (resource_type, name) = resource.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    let (_, kind) = RESOURCE_TYPE_SEGMENTS
        .iter()
        .find(|(segment, _)| *segment == resource_type)?;
    Some(ResourceArnParts {
        region,
        kind: *kind,
        name,
    })
}

/// Why a tagging call's `ResourceName` names no resource of this service (see
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`]).
///
/// Carries the MESSAGE and, for a well-formed ARN, the kind — not a modelled
/// error: each of the three tagging operations declares its own error enum, so
/// each lifts this into its own `InvalidARNFault` / per-kind not-found variant.
#[derive(Debug, Clone)]
pub enum TagTargetRejection {
    /// The value is not an ElastiCache resource ARN at all: the 400
    /// `InvalidARNFault`, "the requested Amazon Resource Name (ARN) does not
    /// refer to an existing resource".
    InvalidArn(String),
    /// A well-formed ARN naming no resource in this store: that kind's own
    /// not-found fault.
    NotFound(TaggedResourceKind, String),
}

/// How many colon-separated components an ElastiCache resource ARN has:
/// `arn:<partition>:elasticache:<region>:<account>:<type>:<name>` — the form
/// every ARN this service mints carries (see [`parse_resource_arn`]).
pub const RESOURCE_ARN_COMPONENTS: usize = 7;

/// How ElastiCache words a `ResourceName` that names no resource of this
/// service: it reports the STRUCTURAL rule it applied and interpolates no
/// caller value at all.
///
/// The rule a value that is not ARN-shaped breaks is the component count, which
/// is the only part of the form that can be counted without interpreting any
/// field; a value that does carry the components but names no ElastiCache
/// resource is answered with the rule the modelled `InvalidARNFault` itself
/// states ("The requested Amazon Resource Name (ARN) does not refer to an
/// existing resource"). Neither template carries the request's value, so the
/// message is the same sentence for every caller.
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    if resource_arn.split(':').count() != RESOURCE_ARN_COMPONENTS {
        return format!("Input ARN string does not have {RESOURCE_ARN_COMPONENTS} components.");
    }
    "The requested Amazon Resource Name (ARN) does not refer to an existing resource.".to_string()
}

/// `resource_arn` parsed, or the `InvalidArn` rejection for a value that is not
/// an ElastiCache resource ARN.
fn tag_target_parts(resource_arn: &str) -> Result<ResourceArnParts<'_>, TagTargetRejection> {
    parse_resource_arn(resource_arn).ok_or_else(|| {
        TagTargetRejection::InvalidArn(invalid_resource_arn_message(resource_arn))
    })
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is
/// not signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The pagination `Marker` for a page that starts at `key`.
///
/// Hex of the key rather than the key itself, so a caller cannot read structure
/// into the token and has nothing to build one from: a `Marker` is opaque and
/// only ever travels back unchanged. Encoding the key (rather than an offset)
/// is what makes a page resumable after the records before it changed.
pub fn encode_marker(key: &str) -> String {
    ::hex::encode(key.as_bytes())
}

/// The inclusive start key a marker minted by [`encode_marker`] carries, or
/// `None` when the token was not minted by this service.
pub fn decode_marker(marker: &str) -> Option<String> {
    String::from_utf8(::hex::decode(marker).ok()?).ok()
}

/// The page size a describe serves, or the reason the page-size member is
/// refused.
///
/// A page-size member carries no modelled `@range`, so the boundary validator
/// passes any integer and the bounds the asking operation's own member
/// documents are enforced here — `member` names it in the refusal, and
/// `default`, `min` and `max` are that operation's. It is refused rather than
/// clamped: a caller that asked for a page size the service does not serve gets
/// told so instead of a silently different page.
fn page_size_bounded(
    requested: Option<i32>,
    member: &str,
    default: i32,
    min: i32,
    max: i32,
) -> Result<usize, String> {
    let requested = requested.unwrap_or(default);
    if !(min..=max).contains(&requested) {
        // The wording ElastiCache itself answers a page size outside the bounds
        // its member documents with: the member's own constraint, stated in the
        // service's words, with the operation's own maximum. `MaxRecords` keeps
        // the recorded wording exactly; another member names itself.
        return Err(if member == "MaxRecords" {
            format!("Invalid value for max records. Must be between {min} and {max}")
        } else {
            format!(
                "Invalid value {requested} for {member}: must be at least {min} and no more than \
                 {max}."
            )
        });
    }
    Ok(usize::try_from(requested).unwrap_or(min.max(1) as usize))
}

/// The page size a `MaxRecords` describe serves; see [`page_size_bounded`]. The
/// minimum is the [`MAX_RECORDS_MIN`] every `MaxRecords` member in this service
/// documents.
pub fn page_size_within(max_records: Option<i32>, default: i32, max: i32) -> Result<usize, String> {
    page_size_bounded(max_records, "MaxRecords", default, MAX_RECORDS_MIN, max)
}

/// The page size `DescribeCacheClusters` / `DescribeCacheSubnetGroups` /
/// `DescribeCacheParameterGroups` serve, whose `MaxRecords` documents "Default:
/// 100. Constraints: minimum 20; maximum 100".
pub fn page_size(max_records: Option<i32>) -> Result<usize, String> {
    page_size_within(max_records, MAX_RECORDS_DEFAULT, MAX_RECORDS_MAX)
}

/// The page size `DescribeSnapshots` serves, whose `MaxRecords` documents
/// "Default: 50. Constraints: minimum 20; maximum 50".
pub fn snapshot_page_size(max_records: Option<i32>) -> Result<usize, String> {
    page_size_within(
        max_records,
        SNAPSHOT_MAX_RECORDS_DEFAULT,
        SNAPSHOT_MAX_RECORDS_MAX,
    )
}

/// The page size `DescribeServerlessCacheSnapshots` serves, whose `MaxResults`
/// documents "The default is 50. The Validation Constraints are a maximum of
/// 50." — a maximum with no stated minimum, so the smallest page it serves is
/// one record; see [`SERVERLESS_SNAPSHOT_MAX_RESULTS_DEFAULT`].
pub fn serverless_snapshot_page_size(max_results: Option<i32>) -> Result<usize, String> {
    page_size_bounded(
        max_results,
        "MaxResults",
        SERVERLESS_SNAPSHOT_MAX_RESULTS_DEFAULT,
        1,
        SERVERLESS_SNAPSHOT_MAX_RESULTS_MAX,
    )
}

/// The page size a describe whose `MaxRecords` documents no bounds serves, or
/// the reason the requested one is refused.
///
/// `DescribeServiceUpdates$MaxRecords` documents only "The maximum number of
/// records to include in the response" — no "minimum 20; maximum 100" like the
/// cluster and subnet-group describes carry (`smithy_query(query="doc",
/// shape="DescribeServiceUpdatesMessage", member="MaxRecords")`) — so any
/// positive count is a page it serves, and only a count that cannot be one is
/// refused. [`MAX_RECORDS_DEFAULT`] is the page it serves when the request names
/// no `MaxRecords` at all.
pub fn unbounded_page_size(max_records: Option<i32>) -> Result<usize, String> {
    unbounded_page_size_with_default(max_records, MAX_RECORDS_DEFAULT)
}

/// [`unbounded_page_size`], for a describe whose `MaxResults` documents its own
/// default page (`DescribeServerlessCaches`: "The default is 50.").
pub fn unbounded_page_size_with_default(
    max_records: Option<i32>,
    default: i32,
) -> Result<usize, String> {
    let requested = max_records.unwrap_or(default);
    match usize::try_from(requested) {
        Ok(size) if size > 0 => Ok(size),
        _ => Err(format!(
            "Invalid value {requested} for MaxRecords: must be at least 1."
        )),
    }
}

/// `engine` lowercased, or the reason it names no cache engine.
///
/// ElastiCache stores the engine name in lowercase, so the comparison and the
/// stored value are both lowercase.
pub fn normalize_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(format!(
            "Invalid value {engine} for Engine: must be one of {}.",
            ENGINES.join(" | ")
        ))
    }
}

/// How ElastiCache refuses an `Engine` a serverless cache cannot run: the
/// CAPABILITY rule, stating that the engine named does not support serverless
/// caches at all.
///
/// Wholly fixed text with no request value: the rule is about the engine's
/// capability, not about a vocabulary the caller could pick another entry from.
pub const SERVERLESS_ENGINE_UNSUPPORTED: &str = "Specified engine does not support serverless cache.";

/// `engine` lowercased when it names an engine a serverless cache can run, else
/// the capability refusal ([`SERVERLESS_ENGINE_UNSUPPORTED`]).
///
/// The engines a serverless cache runs are the ones
/// `ModifyServerlessCache$Engine` documents ("The options are valkey, memcached
/// or redis" \u2014 `smithy_query(query="doc", shape="ModifyServerlessCacheRequest",
/// member="Engine")`), which is [`ENGINES`]; anything else is an engine without
/// the capability rather than a value outside a list. One implementation, so the
/// create and the modify of a serverless cache cannot disagree about it.
pub fn normalize_serverless_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(SERVERLESS_ENGINE_UNSUPPORTED.to_string())
    }
}

/// How ElastiCache words a cache engine name that names no engine at all, which
/// is what the engine-and-version DISCOVERY describe answers for an `Engine`
/// filter the published catalog holds nothing for: `Invalid engine name: <name>`.
///
/// Distinct from [`normalize_engine`], which is how the operations that CREATE a
/// resource running an engine word a value that is not one they serve.
pub fn invalid_engine_name_message(engine: &str) -> String {
    format!("Invalid engine name: {engine}")
}

/// `Ok(())` when `num` is a node count `engine` permits, else the reason it is
/// refused.
///
/// From `NumCacheNodes`: "For clusters running Valkey or Redis OSS, this value
/// must be 1. For clusters running Memcached, this value must be between 1 and
/// 40."
pub fn validate_num_cache_nodes(engine: &str, num: i32) -> Result<(), String> {
    if engine == "memcached" {
        if (1..=MEMCACHED_MAX_NODES).contains(&num) {
            return Ok(());
        }
        return Err(format!(
            "Invalid value {num} for NumCacheNodes: a memcached cluster must have between 1 and \
             {MEMCACHED_MAX_NODES} cache nodes."
        ));
    }
    if num == REDIS_NUM_CACHE_NODES {
        return Ok(());
    }
    Err(format!(
        "Invalid value {num} for NumCacheNodes: a {engine} cluster must have \
         {REDIS_NUM_CACHE_NODES} cache node."
    ))
}

/// A status change that has just been materialised by a settle: the instant it
/// fell due, the status the record carried until then, and the one it carries
/// from then on.
///
/// Handed back so the one place a transition becomes real is also the place the
/// event recording it is written, dated at the instant the clock owed the
/// change rather than at the call that happened to notice it.
#[derive(Debug, Clone)]
pub struct SettledTransition {
    pub due: SystemTime,
    pub from: String,
    pub to: String,
}

/// One event this service recorded: something that happened to a resource of
/// it, at the instant it happened.
///
/// The log is the one copy `DescribeEvents` reports, and it outlives the
/// resource an event is about: ElastiCache still reports the events of a
/// cluster that has since been deleted.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// `Event$SourceIdentifier`: "the identifier for the source of the event.
    /// For example, if the event occurred at the cluster level, the identifier
    /// would be the name of the cluster."
    pub source_identifier: String,
    /// One of [`SOURCE_TYPE_CACHE_CLUSTER`] / [`SOURCE_TYPE_CACHE_SUBNET_GROUP`].
    pub source_type: String,
    pub message: String,
    /// `Event$Date`: "the date and time when the event occurred."
    pub at: SystemTime,
}

impl EventRecord {
    /// This record as the modelled `Event`.
    pub fn to_modeled(&self) -> Event {
        Event {
            source_identifier: Some(self.source_identifier.clone()),
            source_type: Some(self.source_type.clone()),
            message: Some(self.message.clone()),
            date: Some(::aws_smithy_types::DateTime::from(self.at)),
        }
    }
}

/// The key an event is stored under: the instant it happened, then the sequence
/// it was recorded in.
///
/// Ordering the key by time is what puts the log in the chronological order
/// `DescribeEvents` reports and its `Marker` indexes into; the sequence breaks
/// the tie between two events recorded within the same millisecond, so no event
/// can displace another. Both fields are zero-padded so the `BTreeMap`'s
/// lexicographic order over the keys IS their numeric order.
pub fn event_key(at: SystemTime, seq: u64) -> String {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{millis:020}-{seq:020}")
}

// ---------------------------------------------------------------------------
// Reserved cache node offerings
//
// An offering is not an account resource: it is an item of the catalog the
// service publishes, which every account sees the same and which no operation
// creates. The catalog below is therefore built once, stocked into the store by
// [`ComAmazonawsElasticacheAmazonelasticachev9State::default`], and read from
// there by `DescribeReservedCacheNodesOfferings` — and by
// `PurchaseReservedCacheNodesOffering` and `DescribeReservedCacheNodes`, which
// filter on the same five members and price a reservation from the same entry.
// ---------------------------------------------------------------------------

/// One subnet of a cache subnet group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct SubnetRecord {
    pub subnet_identifier: String,
    /// The Availability Zone chosen for this subnet when the group was created
    /// and stored since, so every read reports the same one.
    pub availability_zone: String,
    /// The network types this subnet supports, from the blocks EC2 reported for
    /// it when the group was created — see [`network_types`].
    pub supported_network_types: Vec<String>,
}

/// One cache subnet group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct CacheSubnetGroupRecord {
    pub name: String,
    pub description: String,
    pub subnets: Vec<SubnetRecord>,
    /// The VPC the group's subnets belong to, as the service that OWNS subnets
    /// reported it when the group was created (see [`ec2_subnets`]) — never a
    /// value built here.
    pub vpc_id: String,
    pub arn: String,
    /// The tags `CreateCacheSubnetGroup` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
}

impl CacheSubnetGroupRecord {
    /// This record as the modelled `CacheSubnetGroup`.
    pub fn to_modeled(&self) -> CacheSubnetGroup {
        CacheSubnetGroup {
            cache_subnet_group_name: Some(self.name.clone()),
            cache_subnet_group_description: Some(self.description.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            subnets: Some(
                self.subnets
                    .iter()
                    .map(|subnet| Subnet {
                        subnet_identifier: Some(subnet.subnet_identifier.clone()),
                        subnet_availability_zone: Some(AvailabilityZone {
                            name: Some(subnet.availability_zone.clone()),
                        }),
                        subnet_outpost: None,
                        supported_network_types: reported_network_types(
                            subnet.supported_network_types.clone(),
                        ),
                    })
                    .collect(),
            ),
            arn: Some(self.arn.clone()),
            // What the GROUP supports is what all of its subnets support; see
            // [`group_network_types`].
            supported_network_types: reported_network_types(group_network_types(&self.subnets)),
        }
    }
}

/// One cache node, as the snapshot of its cluster holds it.
///
/// A snapshot is "a copy of an entire cluster as of the time when the snapshot
/// was taken", so these values are COPIED from the source node rather than read
/// back from it: a later `ModifyCacheCluster` cannot change what a snapshot
/// already taken reports.
#[derive(Debug, Clone)]
pub struct NodeSnapshotRecord {
    pub cache_cluster_id: String,
    pub cache_node_id: String,
    /// The node group (shard) the node belonged to. Always absent for a
    /// snapshot of a cache cluster, which belongs to no node group; it is what
    /// a replication-group snapshot would carry.
    pub node_group_id: Option<String>,
    /// The shard configuration `DescribeSnapshots$ShowNodeGroupConfig` asks
    /// for, absent for the same reason as [`NodeSnapshotRecord::node_group_id`].
    pub node_group_configuration: Option<NodeGroupConfiguration>,
    pub cache_node_create_time: SystemTime,
    pub snapshot_create_time: SystemTime,
}

impl NodeSnapshotRecord {
    /// This record as the modelled `NodeSnapshot`. The shard configuration is
    /// reported only when the caller asked for it
    /// (`DescribeSnapshots$ShowNodeGroupConfig`: "if true, the node group
    /// (shard) configuration is included in the snapshot description").
    pub fn to_modeled(&self, include_node_group_config: bool) -> NodeSnapshot {
        NodeSnapshot {
            cache_cluster_id: Some(self.cache_cluster_id.clone()),
            node_group_id: self.node_group_id.clone(),
            cache_node_id: Some(self.cache_node_id.clone()),
            node_group_configuration: include_node_group_config
                .then(|| self.node_group_configuration.clone())
                .flatten(),
            // "The size of the cache on the source cache node": this emulator
            // serves no cache data plane, so it holds no data whose size it
            // could report rather than invent.
            cache_size: None,
            cache_node_create_time: Some(::aws_smithy_types::DateTime::from(
                self.cache_node_create_time,
            )),
            snapshot_create_time: Some(::aws_smithy_types::DateTime::from(
                self.snapshot_create_time,
            )),
        }
    }
}

/// The published cache-engine-version catalog, keyed by
/// [`engine_version_key`].
///
/// An entry's two descriptions come from two different names: the label
/// ElastiCache publishes for the ENGINE describes the engine
/// ([`engine_description`]), and the engine's API name — the `Engine` value
/// itself — is what the version description is built from, `<engine> version
/// <release>`, naming the full release the entry's version stands for
/// ([`patch_release`]) rather than the version line the entry is published
/// under.
///
/// A `BTreeMap` is what gives `DescribeCacheEngineVersions` the stable total
/// order its `Marker` indexes into, and the per-engine order its `DefaultOnly`
/// reads the newest version off.
pub fn published_cache_engine_versions() -> BTreeMap<String, CacheEngineVersionRecord> {
    CACHE_ENGINE_VERSIONS
        .iter()
        .map(|row| {
            (
                engine_version_key(row.engine, row.engine_version),
                CacheEngineVersionRecord {
                    engine: row.engine.to_string(),
                    engine_version: row.engine_version.to_string(),
                    cache_parameter_group_family: parameter_group_family(
                        row.engine,
                        row.engine_version,
                    )
                    .map(str::to_string),
                    cache_engine_description: engine_description(row.engine).to_string(),
                    cache_engine_version_description: format!(
                        "{} version {}",
                        row.engine,
                        patch_release(row.engine, row.engine_version)
                    ),
                },
            )
        })
        .collect()
}

/// The validated values a new serverless cache is built from — one argument so
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::insert_serverless_cache`]
/// does not take two dozen positional strings.
#[derive(Debug, Clone)]
pub struct NewServerlessCache {
    pub name: String,
    pub region: String,
    pub description: String,
    pub engine: String,
    pub major_engine_version: String,
    pub cache_usage_limits: Option<CacheUsageLimits>,
    pub kms_key_id: Option<String>,
    pub security_group_ids: Vec<String>,
    pub subnet_ids: Vec<String>,
    pub user_group_id: Option<String>,
    pub snapshot_retention_limit: Option<i32>,
    pub daily_snapshot_time: Option<String>,
    pub network_type: String,
    pub connection_type: String,
    pub port: i32,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// `ServiceUpdateStatus`'s values (`smithy_query(query="shape",
/// shape="ServiceUpdateStatus")`: `available | cancelled | expired`). A filter
/// naming anything else is refused by the boundary validator before a handler
/// runs, because the member is a modelled enum.
pub const SERVICE_UPDATE_AVAILABLE: &str = "available";
/// See [`SERVICE_UPDATE_AVAILABLE`].
pub const SERVICE_UPDATE_CANCELLED: &str = "cancelled";
/// See [`SERVICE_UPDATE_AVAILABLE`].
pub const SERVICE_UPDATE_EXPIRED: &str = "expired";

/// The only value `ServiceUpdateType` declares
/// (`smithy_query(query="shape", shape="ServiceUpdateType")`:
/// `security-update`).
pub const SERVICE_UPDATE_TYPE: &str = "security-update";

/// The type an END-OF-LIFE update carries: the update that asks for an engine
/// major version to be left before it stops being served.
///
/// `ServiceUpdateType`'s enum lists only [`SERVICE_UPDATE_TYPE`], while
/// ElastiCache publishes this second type beside it; `ServiceUpdate$Type` is a
/// plain string on the wire, so an update reports the type of its own kind
/// rather than the one value the enum lists.
pub const SERVICE_UPDATE_TYPE_ENGINE_MAJOR_VERSION: &str = "engine-major-version-update";

/// The `EstimatedUpdateTime` every published update carries: "The estimated
/// length of time the service update will take".
///
/// Nothing in the model or the docs states what that length is for a given
/// update, so the emulator publishes one estimate for all of them rather than
/// inventing a per-update one.
pub const SERVICE_UPDATE_ESTIMATED_TIME: &str = "15 minutes";

/// A day, the unit the published catalog's dates are spaced by.
pub const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// One row of the service-update catalog this emulator publishes, as offsets in
/// whole days from the instant the store came up.
///
/// AWS publishes ElastiCache service updates itself — the API has no operation
/// that creates, cancels or expires one — and neither the model nor the docs
/// name any particular update, so this table is the emulator's OWN published
/// catalog rather than a claim about what AWS has released. The dates are
/// relative to the store's own anchor so the catalog keeps the shape a real
/// account sees (a recent update still inside its availability window, and an
/// earlier one the clock has already carried past its end date) however long
/// after this build the emulator runs.
struct PublishedUpdate {
    /// The tail of this row's `ServiceUpdateName`, FIXED by the catalog: an
    /// update's identity is part of what the service publishes, so it is the
    /// same string on every store this build brings up and is never derived
    /// from the clock — a client that discovered a name, or that was written
    /// against one, can still name it in `BatchApplyUpdateAction` /
    /// `BatchStopUpdateAction` a day later. See [`published_service_updates`]
    /// for the whole name.
    slug: &'static str,
    /// One of `ServiceUpdateSeverity`'s values (`critical | important | medium |
    /// low`).
    severity: &'static str,
    /// How long before the anchor the update was released.
    released_days_before: u64,
    /// How long after its release the update is recommended to be applied by.
    apply_by_days_after_release: u64,
    /// How long after its release the update stops being available.
    end_days_after_release: u64,
    /// `AutoUpdateAfterRecommendedApplyByDate`: "whether the service update
    /// will be automatically applied once the recommended apply-by date has
    /// expired".
    auto_update_after_recommended_apply_by_date: bool,
}

/// The catalog's rows. Every engine gets one update per row, so the set spans
/// both the availability window a published update is inside and the one it has
/// fallen out of.
const PUBLISHED_UPDATES: [PublishedUpdate; 2] = [
    PublishedUpdate {
        slug: "security-update-001",
        severity: "critical",
        released_days_before: 180,
        apply_by_days_after_release: 30,
        end_days_after_release: 120,
        auto_update_after_recommended_apply_by_date: true,
    },
    PublishedUpdate {
        slug: "security-update-002",
        severity: "important",
        released_days_before: 7,
        apply_by_days_after_release: 30,
        end_days_after_release: 120,
        auto_update_after_recommended_apply_by_date: false,
    },
];

/// The engine major versions ElastiCache publishes an END-OF-LIFE update for,
/// as `(engine, how that engine is spelled in an update's name, the major
/// version segment of the name, the engine version LINE the update applies
/// to)`.
///
/// Redis OSS 6 is the line ElastiCache has announced the end of life of; the
/// engine's name in an update's name is its published product name, which is
/// why Redis OSS carries the `-oss` of it. A line added here is published the
/// moment it is announced, one update per [`EOL_RESOURCE_KINDS`] entry, so the
/// catalog states each end of life once instead of once per resource kind.
const PUBLISHED_EOL_UPDATES: [(&str, &str, &str, &str); 1] =
    [(DEFAULT_ENGINE, "redis-oss", "v6", "6.x")];

/// The KINDS of resource an end-of-life update is published for, as the tail of
/// its `ServiceUpdateName` and as the words its description names them by.
///
/// One update per kind, because the resources that run an engine version are
/// carried onto a later major version by different operations — a cache cluster
/// by `ModifyCacheCluster` and a replication group by `ModifyReplicationGroup` —
/// so a client applies the update that names the kind of resource it holds.
const EOL_RESOURCE_KINDS: [(&str, &str); 2] = [
    ("cache-clusters", "cache clusters"),
    ("replication-groups", "replication groups"),
];

/// The window an end-of-life update is published over, in whole days from the
/// store's anchor, and the severity it carries.
///
/// Longer than a security update's ([`PUBLISHED_UPDATES`]): leaving a major
/// version is a planned upgrade of the resources that run it rather than a
/// patch, so the recommended apply-by date and the end date are further out.
/// Like the security rows, only the DATES hang off the anchor — the names, the
/// types and the severities are fixed by the catalog.
const EOL_RELEASED_DAYS_BEFORE: u64 = 60;
/// See [`EOL_RELEASED_DAYS_BEFORE`].
const EOL_APPLY_BY_DAYS_AFTER_RELEASE: u64 = 180;
/// See [`EOL_RELEASED_DAYS_BEFORE`].
const EOL_END_DAYS_AFTER_RELEASE: u64 = 365;
/// See [`EOL_RELEASED_DAYS_BEFORE`]; one of `ServiceUpdateSeverity`'s values.
const EOL_SEVERITY: &str = "important";

/// One published service update, as this emulator holds it.
///
/// `ServiceUpdateStatus` is NOT stored: it follows from the update's dates and
/// the clock, so [`ServiceUpdateRecord::status`] derives it on every read
/// instead of letting a stored copy drift from the dates beside it.
#[derive(Debug, Clone)]
pub struct ServiceUpdateRecord {
    /// "The unique ID of the service update", and the identity
    /// `ServiceUpdateName` names it by.
    pub service_update_name: String,
    /// "The Elasticache engine to which the update applies."
    pub engine: String,
    /// "The Elasticache engine version to which the update applies."
    pub engine_version: String,
    /// One of `ServiceUpdateSeverity`'s values.
    pub severity: String,
    /// One of `ServiceUpdateType`'s values — see [`SERVICE_UPDATE_TYPE`].
    pub update_type: String,
    /// "Provides details of the service update."
    pub description: String,
    /// "The date when the service update is initially available."
    pub release_date: SystemTime,
    /// "The recommendend date to apply the service update in order to ensure
    /// compliance."
    pub recommended_apply_by_date: SystemTime,
    /// "The date after which the service update is no longer available."
    pub end_date: SystemTime,
    /// See [`PublishedUpdate::auto_update_after_recommended_apply_by_date`].
    pub auto_update_after_recommended_apply_by_date: bool,
    /// See [`SERVICE_UPDATE_ESTIMATED_TIME`].
    pub estimated_update_time: String,
}

impl ServiceUpdateRecord {
    /// This update's `ServiceUpdateStatus` at `now`.
    ///
    /// `ServiceUpdateEndDate` is "the date after which the service update is no
    /// longer available", so an update the clock has carried past it is
    /// `expired` and one still inside its window is `available`. The third
    /// status, `cancelled`, is the status of an update AWS withdrew before its
    /// end date; this emulator publishes none, and no operation in this service
    /// withdraws one, so no record ever reports it.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.end_date {
            SERVICE_UPDATE_EXPIRED
        } else {
            SERVICE_UPDATE_AVAILABLE
        }
    }

    /// This record as the modelled `ServiceUpdate`, with its status taken from
    /// the clock at `now`.
    pub fn to_modeled(&self, now: SystemTime) -> ServiceUpdate {
        ServiceUpdate {
            service_update_name: Some(self.service_update_name.clone()),
            service_update_release_date: Some(::aws_smithy_types::DateTime::from(self.release_date)),
            service_update_end_date: Some(::aws_smithy_types::DateTime::from(self.end_date)),
            service_update_severity: Some(self.severity.clone()),
            service_update_recommended_apply_by_date: Some(::aws_smithy_types::DateTime::from(
                self.recommended_apply_by_date,
            )),
            service_update_status: Some(self.status(now).to_string()),
            service_update_description: Some(self.description.clone()),
            service_update_type: Some(self.update_type.clone()),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            auto_update_after_recommended_apply_by_date: Some(
                self.auto_update_after_recommended_apply_by_date,
            ),
            estimated_update_time: Some(self.estimated_update_time.clone()),
        }
    }
}

/// The values a new snapshot carries that do not come from the source cluster —
/// one argument so [`SnapshotRecord::from_cluster`] does not take a row of
/// positional strings.
#[derive(Debug, Clone)]
pub struct NewSnapshot {
    /// Already lowercased, as the stored name is.
    pub snapshot_name: String,
    /// [`SNAPSHOT_SOURCE_MANUAL`] for a `CreateSnapshot` call.
    pub source: String,
    pub kms_key_id: Option<String>,
    /// The VPC of the source cluster's subnet group, resolved by the caller
    /// (which holds the store) rather than guessed here.
    pub vpc_id: Option<String>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// The values a copied snapshot carries that are not the source snapshot's —
/// one argument so [`SnapshotRecord::copy_to`] does not take a row of
/// positional strings.
#[derive(Debug, Clone)]
pub struct SnapshotCopy {
    /// `CopySnapshot$TargetSnapshotName`, already lowercased: "this value is
    /// stored as a lowercase string".
    pub target_snapshot_name: String,
    /// The region the copy's ARN names — the request's, as every other ARN this
    /// service mints.
    pub region: String,
    /// `CopySnapshot$KmsKeyId`, "the ID of the KMS key used to encrypt the
    /// target snapshot", or `None` to keep the source snapshot's.
    pub kms_key_id: Option<String>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// The service-update catalog as of `anchor`, keyed by `ServiceUpdateName`.
///
/// Two kinds of update make it up, which is what gives
/// `BatchApplyUpdateAction` / `DescribeUpdateActions` the names a client reads
/// off this catalog:
///
/// * a SECURITY update per engine per row of [`PUBLISHED_UPDATES`], named
///   `elasticache-<engine>-<slug>`: the service, the engine the update applies
///   to and the row's own [`PublishedUpdate::slug`];
/// * an END-OF-LIFE update per resource kind per row of
///   [`PUBLISHED_EOL_UPDATES`], named
///   `elasticache-<engine-name>-<major>-eol-<resource-kind>`.
///
/// Every name is FIXED by the catalog, so a store built at any instant
/// publishes the same set of names and a name a client holds stays resolvable;
/// only the DATES hang off `anchor`, which is what keeps the window shape a
/// real account sees. A `BTreeMap` is what gives `DescribeServiceUpdates` the
/// stable total order its `Marker` indexes into.
pub fn published_service_updates(anchor: SystemTime) -> BTreeMap<String, ServiceUpdateRecord> {
    let mut catalog = BTreeMap::new();
    for row in &PUBLISHED_UPDATES {
        let release_date = anchor
            .checked_sub(DAY * u32::try_from(row.released_days_before).unwrap_or(u32::MAX))
            .unwrap_or(anchor);
        for engine in ENGINES {
            let name = format!("{ARN_NAMESPACE}-{engine}-{}", row.slug);
            let engine_version = default_engine_version(engine);
            let severity = row.severity;
            catalog.insert(
                name.clone(),
                ServiceUpdateRecord {
                    service_update_name: name,
                    engine: engine.to_string(),
                    engine_version: engine_version.to_string(),
                    severity: severity.to_string(),
                    update_type: SERVICE_UPDATE_TYPE.to_string(),
                    description: format!(
                        "A {severity} security update for {engine} {engine_version}."
                    ),
                    release_date,
                    recommended_apply_by_date: release_date
                        + DAY * u32::try_from(row.apply_by_days_after_release).unwrap_or(u32::MAX),
                    end_date: release_date
                        + DAY * u32::try_from(row.end_days_after_release).unwrap_or(u32::MAX),
                    auto_update_after_recommended_apply_by_date: row
                        .auto_update_after_recommended_apply_by_date,
                    estimated_update_time: SERVICE_UPDATE_ESTIMATED_TIME.to_string(),
                },
            );
        }
    }
    let eol_release_date = anchor
        .checked_sub(DAY * u32::try_from(EOL_RELEASED_DAYS_BEFORE).unwrap_or(u32::MAX))
        .unwrap_or(anchor);
    for (engine, engine_in_name, major_in_name, version_line) in PUBLISHED_EOL_UPDATES {
        for (kind_in_name, kind_words) in EOL_RESOURCE_KINDS {
            let name =
                format!("{ARN_NAMESPACE}-{engine_in_name}-{major_in_name}-eol-{kind_in_name}");
            catalog.insert(
                name.clone(),
                ServiceUpdateRecord {
                    service_update_name: name,
                    engine: engine.to_string(),
                    // "The Elasticache engine version to which the update
                    // applies": an end-of-life update applies to a whole
                    // version LINE rather than to one release, so the version
                    // it reports is the engine and that line.
                    engine_version: format!("{engine} {version_line}"),
                    severity: EOL_SEVERITY.to_string(),
                    update_type: SERVICE_UPDATE_TYPE_ENGINE_MAJOR_VERSION.to_string(),
                    description: format!(
                        "{engine} {version_line} is reaching end of life: upgrade your \
                         {kind_words} to a later major version."
                    ),
                    release_date: eol_release_date,
                    recommended_apply_by_date: eol_release_date
                        + DAY * u32::try_from(EOL_APPLY_BY_DAYS_AFTER_RELEASE).unwrap_or(u32::MAX),
                    end_date: eol_release_date
                        + DAY * u32::try_from(EOL_END_DAYS_AFTER_RELEASE).unwrap_or(u32::MAX),
                    // A major version upgrade changes what the resource runs,
                    // so it is applied when the customer asks for it rather
                    // than automatically once the apply-by date has passed.
                    auto_update_after_recommended_apply_by_date: false,
                    estimated_update_time: SERVICE_UPDATE_ESTIMATED_TIME.to_string(),
                },
            );
        }
    }
    catalog
}

/// `UpdateActionStatus`'s values (`smithy_query(query="shape",
/// shape="UpdateActionStatus")`: `not-applied | waiting-to-start | in-progress
/// | stopping | stopped | complete | scheduling | scheduled |
/// not-applicable`).
///
/// `not-applied` is the status of an update action nothing has started, so no
/// record is stored for one: it is derived from the absence of a record rather
/// than kept. The four below it are the ones this slice moves through —
/// `BatchApplyUpdateAction` starts an action, `BatchStopUpdateAction` stops one
/// and the clock carries each transient status to the next. `scheduling` /
/// `scheduled` belong to applying an update at a chosen time, which no
/// operation in this API asks for, and `not-applicable` to an update that does
/// not apply to the resource at all.
pub const UPDATE_ACTION_NOT_APPLIED: &str = "not-applied";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_WAITING_TO_START: &str = "waiting-to-start";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_IN_PROGRESS: &str = "in-progress";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_STOPPING: &str = "stopping";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_STOPPED: &str = "stopped";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_COMPLETE: &str = "complete";

/// How long an update action stays in each transient status before the clock
/// owes it the next one.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: a real service update takes
/// minutes, the emulator chooses a span, and it is deliberately NOT zero, so a
/// caller acting straight after `BatchApplyUpdateAction` still observes the
/// action under way — and can stop it. Nothing sleeps: the due instant is
/// stored on the record and materialised by whichever later call reads it.
pub const UPDATE_ACTION_TRANSITION: Duration = Duration::from_millis(500);

/// What `UnprocessedUpdateAction$ErrorType` carries when the resource an update
/// action was named for is not one this account holds.
///
/// `ErrorType` is a plain string with no modelled vocabulary, so these are the
/// names of the modelled fault shapes that refuse exactly those resources
/// elsewhere in this service, rather than a second spelling of the same
/// conditions.
pub const CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE: &str = "CacheClusterNotFoundFault";
/// See [`CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE`].
pub const REPLICATION_GROUP_NOT_FOUND_ERROR_TYPE: &str = "ReplicationGroupNotFoundFault";
/// See [`CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE`]: what `ErrorType` carries when
/// the resource is one this account holds but cannot take the command — the
/// code of the only parameter-value error both batch operations model.
pub const UPDATE_ACTION_INVALID_ERROR_TYPE: &str = "InvalidParameterValue";

/// The status the clock owes an update action now in `status`, or `None` when
/// `status` is where the action stops.
fn next_update_action_status(status: &str) -> Option<&'static str> {
    match status {
        UPDATE_ACTION_WAITING_TO_START => Some(UPDATE_ACTION_IN_PROGRESS),
        UPDATE_ACTION_IN_PROGRESS => Some(UPDATE_ACTION_COMPLETE),
        UPDATE_ACTION_STOPPING => Some(UPDATE_ACTION_STOPPED),
        _ => None,
    }
}

/// True while an update action is still on its way through the update: the
/// statuses an apply does not start over, because work is already under way.
///
/// The statuses OUTSIDE this set — `not-applied`, `stopped` and `complete` —
/// all mean no work is under way on that resource, so an apply starts work from
/// any of them: applying a service update is a request to run it now, and
/// neither the model nor the ElastiCache documentation refuses one for a
/// resource whose previous run finished or was stopped. The stop path is the
/// mirror image and NOT symmetric for a reason: it needs work under way to act
/// on, which is why it refuses exactly those three statuses (see
/// [`update_action_stoppable`]).
fn update_action_in_flight(status: &str) -> bool {
    matches!(
        status,
        UPDATE_ACTION_WAITING_TO_START | UPDATE_ACTION_IN_PROGRESS | UPDATE_ACTION_STOPPING
    )
}

/// True when a stop has something left to stop: an action already `stopping` is
/// on its way to [`UPDATE_ACTION_STOPPED`], so stopping it again would move
/// nothing — and must not push the due instant of that status out.
fn update_action_stoppable(status: &str) -> bool {
    matches!(
        status,
        UPDATE_ACTION_WAITING_TO_START | UPDATE_ACTION_IN_PROGRESS
    )
}

/// True when an update action has finished running on its nodes: `complete`
/// (the update ran to the end) or `stopped` (it was stopped), the two statuses
/// the clock owes nothing beyond. These are the statuses a node reports an
/// update END date for — `NodeUpdateEndDate` is "the end date of the update for
/// a node", and an update still under way has none.
fn update_action_finished(status: &str) -> bool {
    matches!(status, UPDATE_ACTION_COMPLETE | UPDATE_ACTION_STOPPED)
}

/// `SlaMet`'s values (`smithy_query(query="shape", shape="SlaMet")`: `yes | no
/// | n/a`), as `UpdateAction$SlaMet` documents them: "If yes, all nodes in the
/// replication group have been updated by the recommended apply-by date. If no,
/// at least one node in the replication group have not been updated by the
/// recommended apply-by date."
///
/// `n/a` is the third value, and the answer while the question has none yet: the
/// recommended apply-by date has not passed and the update has not finished, so
/// no node is late.
pub const SLA_MET_YES: &str = "yes";
/// See [`SLA_MET_YES`].
pub const SLA_MET_NO: &str = "no";
/// See [`SLA_MET_YES`].
pub const SLA_MET_NOT_APPLICABLE: &str = "n/a";

/// `NodeUpdateInitiatedBy`'s values (`smithy_query(query="shape",
/// shape="NodeUpdateInitiatedBy")`: `system | customer`), which "reflects
/// whether the update was initiated by the customer or automatically applied".
///
/// Every update action this service starts was asked for by a
/// `BatchApplyUpdateAction` call, so `customer` is what it reports. `system`
/// belongs to the automatic apply after the recommended apply-by date, which no
/// operation in this API schedules and this emulator therefore never runs — an
/// action nothing applied stays [`UPDATE_ACTION_NOT_APPLIED`] however long the
/// clock carries it past that date, and reports no initiator at all.
pub const NODE_UPDATE_INITIATED_BY_CUSTOMER: &str = "customer";

/// Which of the two resource lists a batch update-action call named a resource
/// in: `ProcessedUpdateAction` / `UnprocessedUpdateAction` report a cache
/// cluster's id and a replication group's under different members, each kind is
/// resolved against its own state, and the kind is part of an update action's
/// stored key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpdateActionResource {
    /// A `CacheClusterIds` entry.
    CacheCluster,
    /// A `ReplicationGroupIds` entry.
    ReplicationGroup,
}

impl UpdateActionResource {
    /// How a message names this kind of resource.
    fn label(self) -> &'static str {
        match self {
            Self::CacheCluster => "cache cluster",
            Self::ReplicationGroup => "replication group",
        }
    }
}

// ---------------------------------------------------------------------------
// RBAC users
// ---------------------------------------------------------------------------

/// The cache engines an RBAC user may be created for, as `CreateUser$Engine`
/// documents it: "The options are valkey or redis". RBAC is a Valkey / Redis
/// OSS feature, so `memcached` — a legal CLUSTER engine, see [`ENGINES`] — is
/// not one of them.
pub const USER_ENGINES: [&str; 2] = ["redis", "valkey"];

/// How ElastiCache refuses an `Engine` that access control (RBAC) does not
/// exist for: the CAPABILITY rule, naming the engine version from which the
/// feature is available.
///
/// Wholly fixed text with no request value. The RBAC surface is "for Valkey
/// engine version 7.2 onwards and Redis OSS 6.0 to 7.1" (the user and
/// user-group operations' own documentation), so an engine outside it is
/// refused for lacking the feature rather than for being outside a vocabulary
/// the caller could pick another entry from.
pub const ACCESS_CONTROL_ENGINE_UNSUPPORTED: &str =
    "Access control options are available for Redis version 6.0 onwards.";

/// `engine` lowercased when it names an engine access control exists for, else
/// the capability refusal ([`ACCESS_CONTROL_ENGINE_UNSUPPORTED`]).
///
/// ElastiCache stores engine names in lowercase, so the comparison and the
/// stored value are both lowercase. One implementation, so every operation that
/// creates or changes a user or a user group answers the one sentence for an
/// engine the feature does not exist for; the set itself is [`USER_ENGINES`].
pub fn normalize_access_control_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if USER_ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(ACCESS_CONTROL_ENGINE_UNSUPPORTED.to_string())
    }
}

/// `engine` lowercased, or the reason it names no engine a Global datastore can
/// be moved to.
///
/// A Global datastore is "for Valkey or Redis OSS only", so the engine set is
/// the narrower one [`USER_ENGINES`] names; unlike the access-control surface
/// above, the rule here IS the vocabulary, so the refusal states it.
pub fn normalize_global_datastore_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if USER_ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(format!(
            "Invalid value {engine} for Engine: must be one of {}.",
            USER_ENGINES.join(" | ")
        ))
    }
}

/// `User$Status` of a user the service has accepted: "Indicates the user status.
/// Can be \"active\", \"modifying\" or \"deleting\"". A created user is usable at
/// once — ElastiCache answers `CreateUser` with it already active — so there is
/// no transition for the clock to owe; only a later modify or delete moves it.
pub const USER_STATUS_ACTIVE: &str = "active";

/// `User$Status` of a user `DeleteUser` has accepted: "Indicates the user status.
/// Can be \"active\", \"modifying\" or \"deleting\"". A delete answers with the
/// user in this status — it is the LAST thing this API reports about it, because
/// the clock then owes its removal from the store ([`USER_DELETION_TRANSITION`]),
/// after which the user is gone and every lookup of it answers the modelled
/// `UserNotFoundFault`.
pub const USER_STATUS_DELETING: &str = "deleting";

/// How long a user stays `deleting` before the clock owes its removal from the
/// store.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: ElastiCache takes its own time,
/// the emulator chooses a span, and it is deliberately NOT zero so `DeleteUser`
/// answers with the transient `deleting` status and a change arriving inside the
/// span is still refused by the state error that goes with it. The due instant is
/// stored on the record and materialised by whichever later call reads it; a
/// report issued inside the span is answered once it has fallen due.
pub const USER_DELETION_TRANSITION: Duration = CLUSTER_TRANSITION;

/// How many passwords one user may hold, as `CreateUser$Passwords` documents it:
/// "Passwords used for this user. You can create up to two passwords for each
/// user."
pub const MAX_PASSWORDS_PER_USER: usize = 2;

/// How many users the store holds before a create is refused with the modelled
/// `UserQuotaExceededFault`. The emulator's OWN bound on the store, for the
/// reason [`CLUSTER_QUOTA`] gives.
pub const USER_QUOTA: usize = 100;

/// The LONGEST `UserId` the service accepts: ElastiCache documents an RBAC user
/// id as 1 to 40 characters, so an id of exactly this length is ACCEPTED and
/// only a longer one is refused. The model constrains `UserId` with
/// `@length(min: 1)` and a pattern only, so the upper bound is enforced here, by
/// every operation that NAMES a user and can report the modelled
/// `InvalidParameterValueException` — before the id is looked up, because an id
/// the service would never have stored names no user to look up.
///
/// The refusal carries the service's own recorded wording, "User id length must
/// be less than 40.", which states the bound loosely; the BOUND is the
/// documented maximum rather than that sentence read literally.
pub const MAX_USER_ID_LEN: usize = 40;

/// Why `user_id` cannot name a user, or `None` when it can.
///
/// Stated once so `CreateUser` and `DeleteUser` cannot drift about which ids the
/// service accepts: a create that stores an id and a delete that refuses the
/// same one would leave a user nothing could remove.
pub fn user_id_rejection(user_id: &str) -> Option<String> {
    (user_id.chars().count() > MAX_USER_ID_LEN)
        .then(|| format!("User id length must be less than {MAX_USER_ID_LEN}."))
}

/// The engine version RBAC itself arrived in: "The minimum engine version
/// required, which is Redis OSS 6.0" (`User$MinimumEngineVersion`).
///
/// This is the FEATURE's floor, stated on the Redis OSS version line; what a
/// record reports is the oldest version ITS engine publishes at or above the
/// floor — see [`rbac_minimum_engine_version`].
pub const USER_MIN_ENGINE_VERSION: &str = "6.0";

/// The engine version IAM authentication for an RBAC user arrived in:
/// `InputAuthenticationType` accepts `iam`, and ElastiCache supports IAM
/// authentication only from Redis OSS 7.0 onwards, so a user that authenticates
/// that way cannot be attached to a cache running an older engine.
///
/// A feature floor on the same line as [`USER_MIN_ENGINE_VERSION`], read the
/// same way.
pub const USER_MIN_ENGINE_VERSION_IAM: &str = "7.0";

/// `AuthenticationType`'s values (`smithy_query(query="shape",
/// shape="AuthenticationType")`: `password | no-password | iam`), named so the
/// handlers branch on a constant rather than a literal. The full accepted set
/// stays the GENERATED catalog `crate::ops::types::AUTHENTICATION_TYPE_VALUES`.
pub mod authentication_type {
    pub const PASSWORD: &str = "password";
    pub const NO_PASSWORD: &str = "no-password";
    pub const IAM: &str = "iam";
}

/// `InputAuthenticationType`'s values — what an `AuthenticationMode` may ASK for
/// (`smithy_query(query="shape", shape="InputAuthenticationType")`: `password |
/// no-password-required | iam`). They are NOT the same tokens a user reports
/// back (`no-password-required` is asked for, `no-password` is reported).
pub mod input_authentication_type {
    pub const PASSWORD: &str = "password";
    pub const NO_PASSWORD_REQUIRED: &str = "no-password-required";
    pub const IAM: &str = "iam";
}

/// The user every ElastiCache account already has.
///
/// ElastiCache creates one RBAC user, with the id and the name `default`, in
/// every account; it is the user a cache serves unauthenticated traffic as and
/// the one every user group must contain. No call creates it, so the store is
/// stocked with it when it is built — see [`built_in_users`] — and no reporting
/// operation has to write it into existence.
pub const DEFAULT_USER_ID: &str = "default";

/// The user NAME every user group must hold a member carrying.
///
/// "Redis user group needs to contain a user with the user name default"
/// ([`DEFAULT_USER_REQUIRED_MESSAGE`]) is a rule about the NAME, not about which
/// user holds it: the built-in user ([`DEFAULT_USER_ID`]) carries it, and so may
/// a user of the caller's own, which is how a caller takes the engine's
/// unauthenticated access away from the built-in user. It is also the name no
/// two members of one group may share.
pub const DEFAULT_USER_NAME: &str = "default";

/// How ElastiCache words a `DefaultUserRequired` refusal — "You must add default
/// user to a user group."
///
/// One sentence for the whole error class, shared by every call that would leave
/// a user group without the built-in user ([`DEFAULT_USER_ID`]): it carries no
/// request-supplied value and no stored one, so there is nothing to interpolate
/// and no caller-visible wording to diverge between handlers. The HTTP status
/// (400) and the awsQuery code (`DefaultUserRequired`) are the modelled part of
/// the contract; this is the wording AWS was recorded answering with.
pub const DEFAULT_USER_REQUIRED_MESSAGE: &str =
    "Redis user group needs to contain a user with the user name default.";

/// The access string that built-in user carries: every key (`~*`), every pub/sub
/// channel (`&*`) and every command (`+@all`), in the Valkey / Redis OSS ACL
/// syntax `User$AccessString` is written in.
pub const DEFAULT_USER_ACCESS_STRING: &str = "on ~* &* +@all";

/// The `User` members a `DescribeUsers` `Filter` may name, as `Filter$Name`
/// documents it: "The property being filtered. For example, UserId." A filter
/// names a property of the resource being reported, so the accepted set is the
/// scalar members of the `User` shape.
pub const USER_FILTER_NAMES: [&str; 7] = [
    "UserId",
    "UserName",
    "Engine",
    "Status",
    "AccessString",
    "MinimumEngineVersion",
    "ARN",
];

/// The ARN of an RBAC user: `arn:aws:elasticache:<region>:<account>:user:<id>`.
pub fn user_arn(region: &str, user_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("user:{user_id}"))
}

/// The ACL rule keywords a Valkey / Redis OSS access string may carry on their
/// own — the rules that take no argument.
///
/// `AccessString` is "Access permissions string used for this user", written in
/// the engine's own ACL syntax (the syntax `DEFAULT_USER_ACCESS_STRING` is one
/// example of), and the engine's keyword rules are the catalog below. Rules that
/// take an argument (`~`/`%`/`&` patterns and `+`/`-` command rules) are
/// recognised by [`is_access_rule`] instead.
const ACCESS_STRING_KEYWORDS: [&str; 14] = [
    "on",
    "off",
    "nopass",
    "resetpass",
    "allkeys",
    "allchannels",
    "allcommands",
    "nocommands",
    "resetkeys",
    "resetchannels",
    "clearselectors",
    "reset",
    "sanitize-payload",
    "skip-sanitize-payload",
];

/// True when `rule` is one rule of a Valkey / Redis OSS ACL rule list:
///
/// * a keyword from [`ACCESS_STRING_KEYWORDS`];
/// * a key pattern `~<glob>`, or a read/write-qualified one `%R~`/`%W~`/`%RW~`;
/// * a pub/sub channel pattern `&<glob>`;
/// * a command rule `+<command>` / `-<command>`, or a category rule
///   `+@<category>` / `-@<category>`.
///
/// A selector (`(+get ~key1)`) is a rule list in parentheses rather than a
/// single rule, so [`access_string_rejection`] groups it and checks its inner
/// rules through here.
fn is_access_rule(rule: &str) -> bool {
    if ACCESS_STRING_KEYWORDS.contains(&rule) {
        return true;
    }
    // A key or channel pattern: the glob itself is any non-empty run of
    // non-whitespace, because the engine matches it verbatim.
    if let Some(glob) = rule.strip_prefix('~').or_else(|| rule.strip_prefix('&')) {
        return !glob.is_empty();
    }
    // `%RW~<glob>`: the qualifier is a non-empty run of `R`/`W` before the `~`.
    if let Some(rest) = rule.strip_prefix('%') {
        let Some((qualifier, glob)) = rest.split_once('~') else {
            return false;
        };
        return !qualifier.is_empty()
            && qualifier
                .bytes()
                .all(|byte| byte.eq_ignore_ascii_case(&b'r') || byte.eq_ignore_ascii_case(&b'w'))
            && !glob.is_empty();
    }
    let Some(rest) = rule
        .strip_prefix('+')
        .or_else(|| rule.strip_prefix('-'))
    else {
        return false;
    };
    match rest.strip_prefix('@') {
        // A category name: letters and hyphens (`@read`, `@sanitize-payload`).
        Some(category) => {
            !category.is_empty()
                && category
                    .bytes()
                    .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
        }
        // A command, optionally one subcommand of it (`+config|get`).
        None => {
            rest.starts_with(|first: char| first.is_ascii_alphabetic())
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'|' || byte == b'-')
        }
    }
}

/// Which batch operation is driving an update action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateActionCommand {
    /// `BatchApplyUpdateAction`: start the update on each named resource.
    Apply,
    /// `BatchStopUpdateAction`: stop the update under way on each named
    /// resource.
    Stop,
}

/// What a batch update-action call names: the service update, and the resources
/// it acts on.
///
/// `BatchApplyUpdateAction` and `BatchStopUpdateAction` model the same three
/// members, so both handlers hand the store this one request rather than
/// carrying two copies of the accounting below.
#[derive(Debug, Clone, Default)]
pub struct BatchUpdateActionRequest {
    /// "The unique ID of the service update", as sent.
    pub service_update_name: String,
    /// `CacheClusterIds` — "the cache cluster IDs".
    pub cache_cluster_ids: Vec<String>,
    /// `ReplicationGroupIds` — "the replication group IDs".
    pub replication_group_ids: Vec<String>,
}

impl BatchUpdateActionRequest {
    /// Why this request names no resource for `command` to act on, or `None`
    /// when it names at least one.
    ///
    /// Both id lists are modelled `@length(min: 0)`, so the boundary validator
    /// passes a request that names none — and there is then nothing for either
    /// command to act on. The sentence is built here so both handlers refuse
    /// the same condition in the same words and only map it onto their own
    /// modelled error variant.
    pub fn no_resource_refusal(&self, command: UpdateActionCommand) -> Option<String> {
        if !self.cache_cluster_ids.is_empty() || !self.replication_group_ids.is_empty() {
            return None;
        }
        let (verb, preposition) = match command {
            UpdateActionCommand::Apply => ("apply", "to"),
            UpdateActionCommand::Stop => ("stop", "on"),
        };
        Some(format!(
            "CacheClusterIds or ReplicationGroupIds must name at least one resource to {verb} \
             service update {} {preposition}.",
            self.service_update_name
        ))
    }
}

/// The two lists `UpdateActionResultsMessage` reports: the update actions a
/// batch call processed, and the ones it could not.
#[derive(Debug, Clone, Default)]
pub struct UpdateActionResults {
    /// "Update actions that have been processed successfully."
    pub processed: Vec<ProcessedUpdateAction>,
    /// "Update actions that haven't been processed successfully."
    pub unprocessed: Vec<UnprocessedUpdateAction>,
}

/// One update action: the service update being applied, the resource it is
/// being applied to, and how far it has got.
#[derive(Debug, Clone)]
pub struct UpdateActionRecord {
    /// The `ServiceUpdateName` this action applies.
    pub service_update_name: String,
    /// The cache cluster the update is being applied to, when the action was
    /// named through `CacheClusterIds`.
    pub cache_cluster_id: Option<String>,
    /// The replication group the update is being applied to, when the action
    /// was named through `ReplicationGroupIds`.
    pub replication_group_id: Option<String>,
    /// One of `UpdateActionStatus`'s values — see [`UPDATE_ACTION_NOT_APPLIED`].
    pub status: String,
    /// The status change the clock owes this action, if any.
    pub pending_status: Option<PendingTransition>,
    /// When the `BatchApplyUpdateAction` call that started this run asked for
    /// it, which is what `NodeUpdateInitiatedDate` ("the date when the update is
    /// triggered") reports. A stop acts on the run already under way, so it
    /// keeps the instant the apply set.
    pub started_at: SystemTime,
    /// When this action's status last changed — set by the call that moved it
    /// and by [`Self::settle`] to the instant the clock made the change due, so
    /// `UpdateActionStatusModifiedDate` reports when the change HAPPENED rather
    /// than when somebody read it.
    pub status_modified_at: SystemTime,
    /// When the update actually started running on this action's nodes: the
    /// instant it reached [`UPDATE_ACTION_IN_PROGRESS`], which is what
    /// `NodeUpdateStartDate` reports. `None` for a run that was stopped before
    /// it got that far, which never ran on a node.
    pub in_progress_at: Option<SystemTime>,
}

impl UpdateActionRecord {
    /// Materialise every status change the clock owes this action, up to `now`.
    ///
    /// An update action walks a chain of statuses rather than settling once, so
    /// a record nobody read for several spans ends up where the clock has
    /// carried it rather than one step along. This is the ONE write a reporting
    /// operation may make; see [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) {
        while let Some(pending) = self.pending_status.clone() {
            if now < pending.due {
                return;
            }
            self.status = pending.status;
            // The change fell due at `pending.due`, so that — and not the
            // instant of the call that noticed it — is when the status changed.
            self.status_modified_at = pending.due;
            if self.status == UPDATE_ACTION_IN_PROGRESS {
                self.in_progress_at = Some(pending.due);
            }
            self.pending_status =
                next_update_action_status(&self.status).map(|next| PendingTransition {
                    due: pending.due + UPDATE_ACTION_TRANSITION,
                    status: next.to_string(),
                });
        }
    }

    /// This record as the modelled `ProcessedUpdateAction`.
    pub fn to_processed(&self) -> ProcessedUpdateAction {
        ProcessedUpdateAction {
            replication_group_id: self.replication_group_id.clone(),
            cache_cluster_id: self.cache_cluster_id.clone(),
            service_update_name: Some(self.service_update_name.clone()),
            update_action_status: Some(self.status.clone()),
        }
    }
}

/// One shard of a replication group an update action runs on: the shard's
/// `NodeGroupId` and its members' `(CacheClusterId, CacheNodeId)`, which is what
/// `NodeGroupMemberUpdateStatus` reports a node by.
#[derive(Debug, Clone)]
pub struct UpdateActionNodeGroup {
    pub node_group_id: String,
    pub members: Vec<(String, String)>,
}

/// The nodes a service update runs on for one resource — which is also WHICH
/// node-level list reports them.
///
/// `UpdateAction` carries two: `CacheNodeUpdateStatus` is "the status of the
/// service update on the cache node", reported by `CacheNodeId` alone, and
/// `NodeGroupUpdateStatus` is "the status of the service update on the node
/// group", whose members name their cluster as well. A cache cluster's update
/// action has the first, a replication group's the second.
#[derive(Debug, Clone)]
pub enum UpdateActionNodes {
    /// One `CacheNodeId` per node of a cache cluster.
    CacheNodes(Vec<String>),
    /// One entry per node group (shard) of a replication group.
    NodeGroups(Vec<UpdateActionNodeGroup>),
}

impl UpdateActionNodes {
    /// How many nodes the update has to run on, which is the denominator of
    /// `UpdateAction$NodesUpdated` — "the progress of the service update on the
    /// replication group".
    fn count(&self) -> usize {
        match self {
            Self::CacheNodes(ids) => ids.len(),
            Self::NodeGroups(groups) => groups.iter().map(|group| group.members.len()).sum(),
        }
    }
}

/// One resource a published service update can have an update action for: the
/// identity the action is reported under, the engine that decides which updates
/// apply to it, when it came into existence and the nodes the update runs on.
#[derive(Debug, Clone)]
pub struct UpdateActionTarget {
    /// Which of `UpdateAction`'s two id members reports this resource.
    pub kind: UpdateActionResource,
    /// The stored id: the key an update action is held under, and the id
    /// reported under `CacheClusterId` / `ReplicationGroupId`.
    pub id: String,
    /// The resource's cache engine, which `ServiceUpdate$Engine` ("the
    /// Elasticache engine to which the update applies") is matched against.
    pub engine: String,
    /// When the resource was created: an update is available to a resource only
    /// once the resource exists, which is what
    /// [`update_action_view`] dates `UpdateActionAvailableDate` from.
    pub created_at: SystemTime,
    pub nodes: UpdateActionNodes,
}

impl UpdateActionTarget {
    /// The key this resource's action for `service_update_name` is held under in
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::update_actions`].
    pub fn action_key(
        &self,
        service_update_name: &str,
    ) -> (String, UpdateActionResource, String) {
        (
            service_update_name.to_string(),
            self.kind,
            self.id.clone(),
        )
    }
}

/// The node-level view of an update action: the status and the instants every
/// node the action runs on carries.
///
/// This service moves an update action as a whole rather than node by node —
/// nothing in this API addresses one node of an update — so every node of the
/// resource carries the action's own status and instants, and the two per-node
/// lists are this one view repeated for each node the resource holds.
#[derive(Debug, Clone)]
struct NodeUpdateView {
    status: String,
    /// `NodeUpdateStartDate` / `NodeUpdateInitiatedDate`: the instant the update
    /// started running on the node, and the instant it was triggered on it.
    start_date: Option<SystemTime>,
    end_date: Option<SystemTime>,
    initiated_by: Option<String>,
    initiated_date: Option<SystemTime>,
    status_modified_date: SystemTime,
}

impl NodeUpdateView {
    /// The node-level view of `action`, which is `None` for a pair nothing has
    /// applied; `available` is when its action became
    /// [`UPDATE_ACTION_NOT_APPLIED`], which is the only status change such a
    /// pair has had.
    fn of(action: Option<&UpdateActionRecord>, available: SystemTime) -> Self {
        let Some(action) = action else {
            return Self {
                status: UPDATE_ACTION_NOT_APPLIED.to_string(),
                start_date: None,
                end_date: None,
                initiated_by: None,
                initiated_date: None,
                status_modified_date: available,
            };
        };
        Self {
            status: action.status.clone(),
            start_date: action.in_progress_at,
            // "The end date of the update for a node": only a run that has
            // finished has one.
            end_date: update_action_finished(&action.status).then_some(action.status_modified_at),
            initiated_by: Some(NODE_UPDATE_INITIATED_BY_CUSTOMER.to_string()),
            initiated_date: Some(action.started_at),
            status_modified_date: action.status_modified_at,
        }
    }

    /// This view as the modelled `CacheNodeUpdateStatus` of the node
    /// `cache_node_id` names.
    fn to_cache_node(&self, cache_node_id: &str) -> CacheNodeUpdateStatus {
        CacheNodeUpdateStatus {
            cache_node_id: Some(cache_node_id.to_string()),
            node_update_status: Some(self.status.clone()),
            // Applying a service update deletes no node, so there is no
            // deletion date to report.
            node_deletion_date: None,
            node_update_start_date: self.start_date.map(::aws_smithy_types::DateTime::from),
            node_update_end_date: self.end_date.map(::aws_smithy_types::DateTime::from),
            node_update_initiated_by: self.initiated_by.clone(),
            node_update_initiated_date: self.initiated_date.map(::aws_smithy_types::DateTime::from),
            node_update_status_modified_date: Some(::aws_smithy_types::DateTime::from(
                self.status_modified_date,
            )),
        }
    }

    /// This view as the modelled `NodeGroupMemberUpdateStatus` of one node group
    /// member: the same status and instants, reported with the member's cluster.
    fn to_node_group_member(
        &self,
        cache_cluster_id: &str,
        cache_node_id: &str,
    ) -> NodeGroupMemberUpdateStatus {
        let node = self.to_cache_node(cache_node_id);
        NodeGroupMemberUpdateStatus {
            cache_cluster_id: Some(cache_cluster_id.to_string()),
            cache_node_id: node.cache_node_id,
            node_update_status: node.node_update_status,
            node_deletion_date: node.node_deletion_date,
            node_update_start_date: node.node_update_start_date,
            node_update_end_date: node.node_update_end_date,
            node_update_initiated_by: node.node_update_initiated_by,
            node_update_initiated_date: node.node_update_initiated_date,
            node_update_status_modified_date: node.node_update_status_modified_date,
        }
    }
}

/// `UpdateAction$SlaMet` for an action in `status`: whether the update ran on
/// every node by the service update's recommended apply-by date.
///
/// A finished update met the SLA when it finished before that date and missed it
/// otherwise; one that has not finished has missed it only once the date has
/// passed, and until then the question has no answer yet
/// ([`SLA_MET_NOT_APPLICABLE`]).
fn sla_met(
    update: &ServiceUpdateRecord,
    status: &str,
    status_modified: SystemTime,
    now: SystemTime,
) -> &'static str {
    if status == UPDATE_ACTION_COMPLETE {
        if status_modified <= update.recommended_apply_by_date {
            SLA_MET_YES
        } else {
            SLA_MET_NO
        }
    } else if now > update.recommended_apply_by_date {
        SLA_MET_NO
    } else {
        SLA_MET_NOT_APPLICABLE
    }
}

/// The modelled `UpdateAction` for one (service update, resource) pair: what
/// `DescribeUpdateActions` reports about applying `update` to `target`.
///
/// `action` is the record a batch call persisted, or `None` for a pair nothing
/// has started — which IS the `not-applied` status the API reports for it, so
/// the view is computed from the published update and the stored resource rather
/// than requiring a record to exist. Node-level status is reported only when the
/// request asked for it: `ShowNodeLevelUpdateStatus` "dictates whether to
/// include node level update status in the response".
pub fn update_action_view(
    update: &ServiceUpdateRecord,
    target: &UpdateActionTarget,
    action: Option<&UpdateActionRecord>,
    now: SystemTime,
    show_node_level_status: bool,
) -> UpdateAction {
    // "The date that the service update is available to a replication group":
    // the update is available to a resource once BOTH exist, so a resource
    // created after the update was released gains the action when it was
    // created.
    let available = update.release_date.max(target.created_at);
    let node_view = NodeUpdateView::of(action, available);
    let status = node_view.status.clone();
    let total = target.nodes.count();
    // "The progress of the service update on the replication group", read off
    // the node-level statuses: every node of the resource carries the action's
    // own status, so the nodes the update has run on are all of them once it is
    // complete and none of them before.
    let updated = if status == UPDATE_ACTION_COMPLETE { total } else { 0 };
    let (node_groups, cache_nodes) = match (&target.nodes, show_node_level_status) {
        (_, false) => (None, None),
        (UpdateActionNodes::CacheNodes(ids), true) => (
            None,
            Some(
                ids.iter()
                    .map(|id| node_view.to_cache_node(id))
                    .collect::<Vec<CacheNodeUpdateStatus>>(),
            ),
        ),
        (UpdateActionNodes::NodeGroups(groups), true) => (
            Some(
                groups
                    .iter()
                    .map(|group| NodeGroupUpdateStatus {
                        node_group_id: Some(group.node_group_id.clone()),
                        node_group_member_update_status: Some(
                            group
                                .members
                                .iter()
                                .map(|(cluster, node)| {
                                    node_view.to_node_group_member(cluster, node)
                                })
                                .collect(),
                        ),
                    })
                    .collect::<Vec<NodeGroupUpdateStatus>>(),
            ),
            None,
        ),
    };
    UpdateAction {
        replication_group_id: (target.kind == UpdateActionResource::ReplicationGroup)
            .then(|| target.id.clone()),
        cache_cluster_id: (target.kind == UpdateActionResource::CacheCluster)
            .then(|| target.id.clone()),
        service_update_name: Some(update.service_update_name.clone()),
        service_update_release_date: Some(::aws_smithy_types::DateTime::from(update.release_date)),
        service_update_severity: Some(update.severity.clone()),
        service_update_status: Some(update.status(now).to_string()),
        service_update_recommended_apply_by_date: Some(::aws_smithy_types::DateTime::from(
            update.recommended_apply_by_date,
        )),
        service_update_type: Some(update.update_type.clone()),
        update_action_available_date: Some(::aws_smithy_types::DateTime::from(available)),
        update_action_status: Some(status.clone()),
        nodes_updated: Some(format!("{updated}/{total}")),
        update_action_status_modified_date: Some(::aws_smithy_types::DateTime::from(
            node_view.status_modified_date,
        )),
        sla_met: Some(sla_met(update, &status, node_view.status_modified_date, now).to_string()),
        node_group_update_status: node_groups,
        cache_node_update_status: cache_nodes,
        estimated_update_time: Some(update.estimated_update_time.clone()),
        engine: Some(update.engine.clone()),
    }
}

/// How long an access string the store keeps, in characters.
///
/// The model constrains `AccessString` with `@pattern "\S"` alone and
/// `ModifyUser$AppendAccessString` "adds additional user permissions to the
/// access string" a user already has, so the stored rule list grows from call to
/// call and nothing in the model bounds it. This is therefore the emulator's OWN
/// bound on the store, for the reason [`CLUSTER_QUOTA`] gives: it sits far above
/// any rule list an emulator serves, and it is what stops one user's permissions
/// growing without end.
pub const MAX_ACCESS_STRING_LEN: usize = 8192;

/// How the service words the refusal of a rule list it cannot read.
///
/// ONE fixed sentence, whatever is wrong with the list and whichever member
/// carried it: the service states that the access string as a whole is not one,
/// and names neither the member nor the token that is not a rule, so nothing of
/// the request is read back out of the refusal. It is borrowed rather than built,
/// so a caller cannot compose anything into it.
pub const ACCESS_STRING_INVALID: &str = "The access-string is invalid.";

/// Why `access_string` is not an access string, or `None` when it is one.
///
/// The model constrains `AccessString` only with `@pattern "\S"` — any string
/// carrying a non-space character passes the boundary validator — so the rule
/// list itself is parsed here, and a request naming something that is not an ACL
/// rule is refused instead of being stored as a permission set the engine could
/// never apply ([`ACCESS_STRING_INVALID`] is how that refusal reads). The caller
/// maps the refusal onto its own modelled `InvalidParameterValueException`.
///
/// Every way a rule list can fail this test is the SAME condition — the access
/// string is not one this service accepts — so all of them are refused with the
/// one sentence [`ACCESS_STRING_INVALID`] holds, which carries no part of the
/// request: neither the rule that offended nor the member it arrived on is
/// reported, and a caller therefore does not say which member it is checking.
///
/// A SELECTOR — the Redis OSS 7 / Valkey form `(+get ~key1)` — is one rule that
/// carries a rule list of its own, so it is read as a parenthesised group whose
/// inner rules are checked by the same per-rule test. A group that is never
/// closed is refused.
pub fn access_string_rejection(access_string: &str) -> Option<&'static str> {
    // Checked before the list is walked: the length is the one thing about a
    // rule list a caller can grow without end (see [`MAX_ACCESS_STRING_LEN`]),
    // and it bounds the work the walk below does.
    if access_string.chars().count() > MAX_ACCESS_STRING_LEN {
        return Some(ACCESS_STRING_INVALID);
    }
    // Every refusal below is the same one sentence, so the walk only has to
    // decide WHETHER the list is a rule list.
    let mut rules = 0usize;
    // The selector being read, once its `(` has been seen and before its `)`.
    let mut selector: Option<String> = None;
    for token in access_string.split_whitespace() {
        if let Some(group) = selector.as_mut() {
            group.push(' ');
            group.push_str(token);
            if !token.ends_with(')') {
                continue;
            }
            let group = selector.take().unwrap_or_default();
            if !is_selector(&group) {
                return Some(ACCESS_STRING_INVALID);
            }
            rules += 1;
            continue;
        }
        if token.starts_with('(') {
            if token.ends_with(')') && token.len() > 1 {
                if !is_selector(token) {
                    return Some(ACCESS_STRING_INVALID);
                }
                rules += 1;
            } else {
                selector = Some(token.to_string());
            }
            continue;
        }
        if !is_access_rule(token) {
            return Some(ACCESS_STRING_INVALID);
        }
        rules += 1;
    }
    // A selector whose `)` never arrived, and a string that names no rule at
    // all, are the same condition as a rule the engine does not know: neither is
    // an access string the service can apply.
    if selector.is_some() || rules == 0 {
        return Some(ACCESS_STRING_INVALID);
    }
    None
}

/// True when `group` is a selector: a parenthesised, non-empty rule list, every
/// rule of which is one [`is_access_rule`] accepts.
fn is_selector(group: &str) -> bool {
    let Some(inner) = group
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let mut rules = inner.split_whitespace().peekable();
    rules.peek().is_some() && rules.all(is_access_rule)
}

/// The ACL rules that state a rule list's COMMAND BASELINE outright: whether the
/// user starts from every command allowed or every command denied.
///
/// `allcommands` and `nocommands` are the engine's own aliases for `+@all` and
/// `-@all` — they are in [`ACCESS_STRING_KEYWORDS`] for that reason — so a rule
/// list naming any of the four has already said where its command rules start
/// from. Matched case-insensitively, as the engine reads its rule names.
const ACCESS_STRING_COMMAND_BASELINE: [&str; 4] = ["+@all", "-@all", "allcommands", "nocommands"];

/// The baseline term the normalised form carries when the request named none: a
/// new user is created with every command denied, so the expanded rule list
/// starts by denying them all and the rules that follow are what it grants back.
pub const ACCESS_STRING_DENY_ALL: &str = "-@all";

/// True when `rule` states the command baseline — see
/// [`ACCESS_STRING_COMMAND_BASELINE`].
fn is_command_baseline(rule: &str) -> bool {
    ACCESS_STRING_COMMAND_BASELINE
        .iter()
        .any(|baseline| rule.eq_ignore_ascii_case(baseline))
}

/// True when `rule` grants or revokes a command or a command category (`+get`,
/// `-@admin`): the rules whose effect depends on the baseline they start from.
fn is_command_rule(rule: &str) -> bool {
    (rule.starts_with('+') || rule.starts_with('-')) && is_access_rule(rule)
}

/// An access string as the service STORES and REPORTS it: the EXPANDED rule
/// list, which states its command baseline explicitly.
///
/// A Valkey / Redis OSS user is created with no permissions at all — every
/// command denied — and the rules of an access string are applied on top of that
/// starting point, so the expanded form of a rule list that grants commands
/// without saying where it starts from carries the deny-all term
/// ([`ACCESS_STRING_DENY_ALL`]) before its command rules. A list that already
/// states its baseline ([`ACCESS_STRING_COMMAND_BASELINE`]) expands to itself.
/// The term goes in immediately before the FIRST command rule, so the rules it
/// governs follow it, and at the end of a list that names no command rule, whose
/// whole command baseline it then is.
///
/// Nothing else is changed: no other rule is added, removed or reordered, and a
/// selector (`(+get ~key1)`) is one rule of the list it sits in, so the baseline
/// of the list is decided by the rules AROUND it and the selector's own rules are
/// kept exactly as sent. Beyond that only the whitespace between rules is
/// normalised.
pub fn canonical_access_string(access_string: &str) -> String {
    let mut rules: Vec<&str> = access_string.split_whitespace().collect();
    // Where the deny-all term goes, if it is needed at all: before the first
    // command rule of the list ITSELF, so a command rule inside a selector —
    // which governs that selector alone — does not decide it.
    let mut first_command = None;
    let mut inside_selector = false;
    for (index, rule) in rules.iter().enumerate() {
        if inside_selector {
            inside_selector = !rule.ends_with(')');
            continue;
        }
        if rule.starts_with('(') {
            inside_selector = !(rule.ends_with(')') && rule.len() > 1);
            continue;
        }
        if is_command_baseline(rule) {
            // The request said where its command rules start from, so the
            // expanded form is the list as sent.
            return rules.join(" ");
        }
        if first_command.is_none() && is_command_rule(rule) {
            first_command = Some(index);
        }
    }
    rules.insert(first_command.unwrap_or(rules.len()), ACCESS_STRING_DENY_ALL);
    rules.join(" ")
}

/// Why a request's authentication members cannot make a user's `Authentication`.
#[derive(Debug, Clone)]
pub enum AuthRefusal {
    /// A member's value is outside what the service accepts — the caller's
    /// modelled `InvalidParameterValueException`.
    Value(String),
    /// The members supplied cannot hold together — the caller's modelled
    /// `InvalidParameterCombinationException`.
    Combination(String),
}

/// How a stored user authenticates: `Authentication`'s "Type" and, for a
/// password user, "The number of passwords belonging to the user" — plus the
/// one-way DIGEST of each password, which is what a cache's own socket checks an
/// `AUTH` against.
///
/// The passwords THEMSELVES are never stored: ElastiCache reports only the count
/// (`Authentication$PasswordCount`), and a secret the API never reads back has no
/// business sitting in emulator state. What is kept is
/// [`credential_digest`] of each, because a cache that authenticates a client has
/// to be able to check what the client sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationRecord {
    /// An `AuthenticationType` token — see [`authentication_type`].
    pub r#type: String,
    /// How many passwords the user holds, for a password user; `None` for one
    /// that authenticates with IAM or without a password.
    pub password_count: Option<i32>,
    /// The digest of each password the user holds, in the order they were given;
    /// see [`credential_digest`]. Empty for a user that holds none.
    pub password_digests: Vec<String>,
}

impl AuthenticationRecord {
    /// The user's `Authentication`, as the API reports it.
    pub fn to_modeled(&self) -> Authentication {
        Authentication {
            r#type: Some(self.r#type.clone()),
            password_count: self.password_count,
        }
    }

    /// The engine version FLOOR authenticating this way requires: IAM
    /// authentication arrived after RBAC itself, so a user that authenticates
    /// with it needs the later feature's floor, and one that authenticates with
    /// a password or without one needs only RBAC's.
    ///
    /// A floor is not yet a version an engine publishes; what the record reports
    /// is [`rbac_minimum_engine_version`] of its engine and this floor.
    pub fn engine_version_floor(&self) -> &'static str {
        if self.r#type == authentication_type::IAM {
            USER_MIN_ENGINE_VERSION_IAM
        } else {
            USER_MIN_ENGINE_VERSION
        }
    }
}

/// The modelled `UnprocessedUpdateAction` for one resource a batch call could
/// not act on, carrying the id under the member its kind is reported by.
fn unprocessed_update_action(
    kind: UpdateActionResource,
    id: &str,
    service_update_name: &str,
    error_type: &str,
    error_message: String,
) -> UnprocessedUpdateAction {
    UnprocessedUpdateAction {
        replication_group_id: (kind == UpdateActionResource::ReplicationGroup)
            .then(|| id.to_string()),
        cache_cluster_id: (kind == UpdateActionResource::CacheCluster).then(|| id.to_string()),
        service_update_name: Some(service_update_name.to_string()),
        error_type: Some(error_type.to_string()),
        error_message: Some(error_message),
    }
}

/// The `Authentication` the passwords in `passwords` give a user, or why they
/// are refused.
///
/// Both published rules are enforced here, so every request that carries
/// passwords — `CreateUser`'s `Passwords` and `AuthenticationMode$Passwords`,
/// and `ModifyUser`'s — takes the same ones: at most
/// [`MAX_PASSWORDS_PER_USER`] passwords, each [`PASSWORD_MIN_LENGTH`] to
/// [`PASSWORD_MAX_LENGTH`] characters. A password outside the length rule is
/// refused rather than stored: a user whose password the service would never
/// have accepted must not exist. The passwords themselves are never kept; see
/// [`AuthenticationRecord`].
fn password_authentication(passwords: &[String]) -> Result<AuthenticationRecord, AuthRefusal> {
    if passwords.len() > MAX_PASSWORDS_PER_USER {
        return Err(AuthRefusal::Value(format!(
            "{} passwords were supplied; you can create up to {MAX_PASSWORDS_PER_USER} passwords \
             for each user.",
            passwords.len()
        )));
    }
    for password in passwords {
        if !(PASSWORD_MIN_LENGTH..=PASSWORD_MAX_LENGTH).contains(&password.chars().count()) {
            return Err(AuthRefusal::Value(format!(
                "Passwords length must be between \
                 {PASSWORD_MIN_LENGTH}-{PASSWORD_MAX_LENGTH} characters."
            )));
        }
    }
    // At most `MAX_PASSWORDS_PER_USER`, refused above, so the conversion is
    // exact; the fallback only keeps the handler panic-free.
    let count = i32::try_from(passwords.len()).unwrap_or(i32::MAX);
    Ok(AuthenticationRecord {
        r#type: authentication_type::PASSWORD.to_string(),
        password_count: Some(count),
        // What a cache's socket checks an `AUTH` against: the digest of each
        // password, never the password.
        password_digests: passwords.iter().map(|password| credential_digest(password)).collect(),
    })
}

/// Why an `AuthenticationMode` cannot stand beside one of the older
/// authentication members, stated from what the request submitted.
///
/// The two statements of how a user authenticates contradict each other, and the
/// refusal is the only place the caller is told WHICH two it sent: so it names
/// the older directive it carried — the submitted value of the
/// `NoPasswordRequired` flag, or the passwords it supplied — together with the
/// `AuthenticationMode$Type` the same request asked for. `no_password_required`
/// is `None` only when the older directive was `Passwords`; one of the two is
/// always present, since that is what makes this a conflict at all.
fn conflicting_authentication_message(no_password_required: Option<bool>, mode_type: &str) -> String {
    match no_password_required {
        Some(flag) => format!(
            "No password required flag is {flag} but provided authentication type is {mode_type}"
        ),
        None => {
            format!("Passwords are provided but provided authentication type is {mode_type}")
        }
    }
}

/// The `Authentication` a request's authentication members describe, or why they
/// are refused.
///
/// A user authenticates in exactly ONE way, so the three members that can say
/// which are mutually exclusive: `AuthenticationMode` ("Specifies how to
/// authenticate the user") states the whole of it on its own, while the older
/// `Passwords` ("Passwords used for this user") and `NoPasswordRequired`
/// ("Indicates a password is not required for this user") are the two halves of
/// the same choice. A request that names none of them has not said how the user
/// authenticates and is refused rather than given a default the caller did not
/// ask for.
///
/// `CreateUser` and `ModifyUser` take the same members, so both resolve them
/// here and cannot drift about which combinations they refuse.
pub fn authentication_from_request(
    passwords: Option<&[String]>,
    no_password_required: Option<bool>,
    mode: Option<&AuthenticationMode>,
) -> Result<AuthenticationRecord, AuthRefusal> {
    let passwords = passwords.unwrap_or(&[]);
    if let Some(mode) = mode {
        let mode_passwords = mode.passwords.as_deref().unwrap_or(&[]);
        // The authentication type the mode asks for, resolved BEFORE anything
        // else about the mode: every refusal below reports it back, so a request
        // that names no type to report is refused for that first.
        let mode_type = match mode.r#type.as_deref().unwrap_or_default() {
            token @ (input_authentication_type::PASSWORD
            | input_authentication_type::NO_PASSWORD_REQUIRED
            | input_authentication_type::IAM) => token,
            other => {
                return Err(AuthRefusal::Value(format!(
                    "AuthenticationMode.Type must be one of {} — got {other:?}.",
                    // The GENERATED catalog, so a refusal cannot name a
                    // different set from the one the model declares.
                    // `InputAuthenticationType` IS a modelled enum, so the
                    // boundary validator normally refuses an unknown token
                    // before a handler runs; this arm is what keeps an absent
                    // `Type` from being read as one of the three.
                    crate::ops::types::INPUT_AUTHENTICATION_TYPE_VALUES.join(", ")
                )))
            }
        };
        if !passwords.is_empty() || no_password_required.is_some() {
            // Two statements of how the user authenticates: the refusal is built
            // from the pair the request submitted, not from the rule alone.
            return Err(AuthRefusal::Combination(conflicting_authentication_message(
                no_password_required,
                mode_type,
            )));
        }
        return match mode_type {
            input_authentication_type::PASSWORD => {
                if mode_passwords.is_empty() {
                    return Err(AuthRefusal::Combination(format!(
                        "AuthenticationMode.Passwords is required when AuthenticationMode.Type \
                         is {mode_type}."
                    )));
                }
                password_authentication(mode_passwords)
            }
            token => {
                if !mode_passwords.is_empty() {
                    return Err(AuthRefusal::Combination(format!(
                        "AuthenticationMode.Passwords cannot be supplied when \
                         AuthenticationMode.Type is {token}."
                    )));
                }
                Ok(AuthenticationRecord {
                    // `no-password-required` is what a request ASKS for;
                    // `no-password` is what the user reports back.
                    r#type: if token == input_authentication_type::IAM {
                        authentication_type::IAM.to_string()
                    } else {
                        authentication_type::NO_PASSWORD.to_string()
                    },
                    password_count: None,
                    // A user that holds no password has none to check an `AUTH`
                    // against.
                    password_digests: Vec::new(),
                })
            }
        };
    }
    if no_password_required == Some(true) {
        if !passwords.is_empty() {
            return Err(AuthRefusal::Combination(
                "Passwords cannot be supplied when NoPasswordRequired is true.".to_string(),
            ));
        }
        return Ok(AuthenticationRecord {
            r#type: authentication_type::NO_PASSWORD.to_string(),
            password_count: None,
            password_digests: Vec::new(),
        });
    }
    if !passwords.is_empty() {
        return password_authentication(passwords);
    }
    Err(AuthRefusal::Combination(
        "No password was supplied and NoPasswordRequired was not set: name Passwords, \
         NoPasswordRequired or an AuthenticationMode."
            .to_string(),
    ))
}

/// One stored RBAC user — the one copy `CreateUser` writes and every user
/// operation reads.
#[derive(Debug, Clone)]
pub struct UserRecord {
    /// `UserId`, as stored: "This value is stored as a lowercase string".
    pub user_id: String,
    pub user_name: String,
    /// The engine, lowercased — see [`normalize_access_control_engine`].
    pub engine: String,
    /// The ACL rule list, as [`canonical_access_string`] stores it.
    pub access_string: String,
    pub authentication: AuthenticationRecord,
    /// A `User$Status` token — see [`USER_STATUS_ACTIVE`].
    pub status: String,
    pub arn: String,
    /// The tags the create was given, for `ListTagsForResource` to report and the
    /// two tag mutations to change; see [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    /// When the clock owes this user's removal from the store: the instant
    /// [`USER_DELETION_TRANSITION`] after the `DeleteUser` that moved it to
    /// [`USER_STATUS_DELETING`], and `None` for a user nothing has deleted.
    /// Materialised by [`ComAmazonawsElasticacheAmazonelasticachev9State::settle_users`].
    pub pending_removal: Option<SystemTime>,
    /// The status change the clock owes this user: [`USER_STATUS_ACTIVE`] a span
    /// after the `ModifyUser` that moved it to [`STATUS_MODIFYING`], and `None`
    /// for a user with nothing outstanding. The CHANGE itself is applied at the
    /// call — the `User` shape carries no pending-value member to report it
    /// through — so only the status is owed. Materialised by
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::settle_users`].
    pub pending_status: Option<PendingTransition>,
}

impl UserRecord {
    /// `User$MinimumEngineVersion`: the lowest engine version that supports this
    /// user's access configuration.
    ///
    /// RBAC is an engine feature, so the floor is the one the user's own engine
    /// publishes — the oldest version of it at or above the floor of the latest
    /// feature the configuration uses, which is RBAC itself for a password or
    /// passwordless user and IAM authentication for one that authenticates that
    /// way. See [`rbac_minimum_engine_version`].
    pub fn minimum_engine_version(&self) -> &'static str {
        rbac_minimum_engine_version(&self.engine, self.authentication.engine_version_floor())
    }

    /// The user as the API reports it — `CreateUser`'s output shape is the `User`
    /// shape, so this is also what `DescribeUsers` answers with.
    pub fn to_modeled(&self) -> CreateUserOutput {
        CreateUserOutput {
            user_id: Some(self.user_id.clone()),
            user_name: Some(self.user_name.clone()),
            status: Some(self.status.clone()),
            engine: Some(self.engine.clone()),
            minimum_engine_version: Some(self.minimum_engine_version().to_string()),
            access_string: Some(self.access_string.clone()),
            // "Returns a list of the user group IDs the user belongs to":
            // membership is held by the user GROUPS that list the user, so it is
            // derived from those records rather than copied onto the user — see
            // [`ComAmazonawsElasticacheAmazonelasticachev9State::user_group_ids_of`],
            // which is what the handlers pass to [`UserRecord::to_modeled_in`].
            user_group_ids: Some(Vec::new()),
            authentication: Some(self.authentication.to_modeled()),
            arn: Some(self.arn.clone()),
        }
    }

    /// The user as the API reports it, belonging to `user_group_ids`.
    ///
    /// `User$UserGroupIds` is "a list of the user group IDs the user belongs
    /// to", which the user-group records are the one copy of; a handler reads
    /// them with `user_group_ids_of` and hands them here.
    pub fn to_modeled_in(&self, user_group_ids: Vec<String>) -> CreateUserOutput {
        CreateUserOutput {
            user_group_ids: Some(user_group_ids),
            ..self.to_modeled()
        }
    }

    /// The value of the `User` property a `DescribeUsers` `Filter` names, or
    /// `None` when the name is not one of [`USER_FILTER_NAMES`].
    pub fn property(&self, name: &str) -> Option<&str> {
        match name {
            "UserId" => Some(self.user_id.as_str()),
            "UserName" => Some(self.user_name.as_str()),
            "Engine" => Some(self.engine.as_str()),
            "Status" => Some(self.status.as_str()),
            "AccessString" => Some(self.access_string.as_str()),
            "MinimumEngineVersion" => Some(self.minimum_engine_version()),
            "ARN" => Some(self.arn.as_str()),
            _ => None,
        }
    }
}

/// The users a store holds before any call of this API: the built-in `default`
/// user ElastiCache gives every account.
///
/// Stocked when the store is BUILT, like the service-update and offering
/// catalogs, so no reporting operation has to write the built-in into existence
/// and no request's region can decide what its ARN says. The ARN therefore names
/// [`DEFAULT_REGION`]: the user is not the product of any request, so there is no
/// request region to take.
pub fn built_in_users() -> BTreeMap<String, UserRecord> {
    let mut users = BTreeMap::new();
    users.insert(
        DEFAULT_USER_ID.to_string(),
        UserRecord {
            user_id: DEFAULT_USER_ID.to_string(),
            user_name: DEFAULT_USER_ID.to_string(),
            // The built-in user predates Valkey support, so it runs the engine
            // RBAC arrived on.
            engine: DEFAULT_ENGINE.to_string(),
            access_string: DEFAULT_USER_ACCESS_STRING.to_string(),
            authentication: AuthenticationRecord {
                r#type: authentication_type::NO_PASSWORD.to_string(),
                password_count: None,
                // The built-in user holds no password, which is what makes it the
                // user a cache serves unauthenticated traffic as.
                password_digests: Vec::new(),
            },
            status: USER_STATUS_ACTIVE.to_string(),
            arn: user_arn(DEFAULT_REGION, DEFAULT_USER_ID),
            tags: Vec::new(),
            // No call creates the built-in user and none deletes it, so the
            // clock never owes its removal.
            pending_removal: None,
            // Nothing has modified it either, so no status change is owed.
            pending_status: None,
        },
    );
    users
}

/// Per-service state for com-amazonaws-elasticache-amazonelasticachev9. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsElasticacheAmazonelasticachev9State {
    /// Every stored cluster, keyed by its (lowercased) `CacheClusterId` — the
    /// resource's identity in this API. A `BTreeMap` is what gives
    /// `DescribeCacheClusters` the stable total order its `Marker` indexes into.
    pub cache_clusters: BTreeMap<String, CacheClusterRecord>,
    /// Every stored subnet group, keyed by its (lowercased) name, ordered for
    /// the same reason.
    pub cache_subnet_groups: BTreeMap<String, CacheSubnetGroupRecord>,
    /// Every stored user group, keyed by its `UserGroupId` EXACTLY as the
    /// request spelled it: unlike `CacheClusterId` / `UserId` /
    /// `ReplicationGroupId`, the model documents no lowercasing for it, so the
    /// identity a later describe or association resolves is case-sensitive.
    pub user_groups: BTreeMap<String, UserGroupRecord>,
    /// Every stored Global datastore, keyed by its (lowercased)
    /// `GlobalReplicationGroupId`.
    pub global_replication_groups: BTreeMap<String, GlobalReplicationGroupRecord>,
    /// Every stored replication group, keyed by its (lowercased)
    /// `ReplicationGroupId`, ordered for the same reason: it is what gives
    /// `DescribeReplicationGroups` the stable total order its `Marker` indexes
    /// into. A group's member clusters live in
    /// [`Self::cache_clusters`] beside the standalone ones, so there is one copy
    /// of each cluster however it was created.
    pub replication_groups: BTreeMap<String, ReplicationGroupRecord>,
    /// Every cache parameter group, keyed by its (lowercased) name, ordered for
    /// the same reason. Unlike the maps above this one is SEEDED when the store
    /// is created, with the default group of every published family (see
    /// [`default_parameter_groups`]): those groups exist in an account before
    /// anything has been created, and seeding them HERE rather than from a
    /// handler is what keeps `DescribeCacheParameterGroups` a pure read.
    /// `CreateCacheParameterGroup` adds the account's own custom groups beside
    /// them.
    pub cache_parameter_groups: BTreeMap<String, CacheParameterGroupRecord>,
    /// Every stored snapshot, keyed by its (lowercased) `SnapshotName` — the
    /// resource's identity in this API, which `CreateSnapshot$SnapshotName`
    /// "is stored as a lowercase string". Ordered for the same reason as
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::cache_clusters`]:
    /// `DescribeSnapshots`'s `Marker` indexes into that total order.
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    /// The service updates this emulator publishes, keyed by
    /// `ServiceUpdateName` and ordered for the same reason. AWS releases these
    /// rather than an account creating them, so the store holds the catalog
    /// from the moment it comes up — see [`published_service_updates`].
    pub service_updates: BTreeMap<String, ServiceUpdateRecord>,
    /// Every update action a batch call started, keyed by
    /// `(ServiceUpdateName, resource kind, resource key)` — the identity of an
    /// update action in this API is the update together with the resource it is
    /// being applied to, and the KIND is part of that key because a cache
    /// cluster and a replication group may carry the same name while being
    /// different resources. A `BTreeMap` keeps the stable total order a page of
    /// them would index into. A key with no entry is an action nothing has
    /// started, which is the `not-applied` status; see
    /// [`UPDATE_ACTION_NOT_APPLIED`].
    pub update_actions: BTreeMap<(String, UpdateActionResource, String), UpdateActionRecord>,
    /// Every event this service recorded, keyed by [`event_key`] so the map's
    /// order IS the chronological order `DescribeEvents` reports and its
    /// `Marker` indexes into. The log is kept beside the resources rather than
    /// on them, because an event outlives the resource it is about.
    pub events: BTreeMap<String, EventRecord>,
    /// The counter that breaks the tie between two events recorded in the same
    /// millisecond; see [`event_key`].
    pub event_seq: u64,
    /// The notifications recorded cluster events owe their clusters' topics and
    /// that have not been published yet, oldest first — written through
    /// [`Self::record_cluster_event`] and drained by
    /// [`deliver_cluster_notifications`], which every call this service serves
    /// runs once it has answered.
    pub pending_cluster_notifications: Vec<ClusterNotification>,
    /// Every stored RBAC user, keyed by its (lowercased) `UserId` — the
    /// resource's identity in this API — including the built-in `default` user
    /// [`built_in_users`] stocks the store with. A `BTreeMap`, so `DescribeUsers`
    /// has the stable total order its `Marker` indexes into.
    pub users: BTreeMap<String, UserRecord>,
    /// The cache engine versions this emulator publishes, keyed by
    /// [`engine_version_key`]. AWS publishes these rather than an account
    /// creating them, so the store holds the catalog from the moment it comes
    /// up — see [`published_cache_engine_versions`].
    pub cache_engine_versions: BTreeMap<String, CacheEngineVersionRecord>,
    /// Every stored serverless cache, keyed by its (lowercased)
    /// `ServerlessCacheName`, ordered for the same reason.
    pub serverless_caches: BTreeMap<String, ServerlessCacheRecord>,
    /// Every stored serverless cache snapshot, keyed by its (lowercased)
    /// `ServerlessCacheSnapshotName` — the resource's identity in this API. A
    /// `BTreeMap`, so `DescribeServerlessCacheSnapshots` has the stable total
    /// order its `NextToken` indexes into. A snapshot outlives the cache it was
    /// taken of, so the records sit beside the caches rather than on them.
    pub serverless_cache_snapshots: BTreeMap<String, ServerlessCacheSnapshotRecord>,
    /// The counter endpoint DNS labels are minted from.
    pub dns_token_seq: u64,
    /// The reserved-cache-node offerings this service publishes, keyed for the
    /// same stable total order. Stocked from [`offering_catalog`] when the store
    /// is created, because the catalog is the service's own and no operation
    /// creates or removes an entry: a describe only reports it, and
    /// `PurchaseReservedCacheNodesOffering` prices a reservation from it.
    pub reserved_cache_nodes_offerings: BTreeMap<String, ReservedCacheNodesOffering>,
    /// The engine default parameters this service publishes, keyed by cache
    /// parameter group family and, inside a family, by
    /// [`ParameterRecord::page_key`] — the total order
    /// `DescribeEngineDefaultParameters`' `Marker` indexes into. Stocked from
    /// [`published_engine_default_parameters`] when the store is created, for the
    /// same reason as the offerings above: the defaults are the engine's, not an
    /// account's, so no operation creates or removes one.
    pub engine_default_parameters: BTreeMap<String, BTreeMap<String, ParameterRecord>>,
    /// Every reservation this account bought, keyed by its
    /// `ReservedCacheNodeId` — "the unique identifier for the reservation",
    /// which is "a customer-specified identifier" and so is held exactly as it
    /// was given. A `BTreeMap` is what gives `DescribeReservedCacheNodes` the
    /// stable total order its `Marker` indexes into.
    pub reserved_cache_nodes: BTreeMap<String, ReservedCacheNodeRecord>,
    /// When each shard this account has tested automatic failover on was tested.
    ///
    /// Not a resource but a RATE: `TestFailover` is bounded at
    /// [`MAX_TEST_FAILOVERS_PER_WINDOW`] shards per [`TEST_FAILOVER_WINDOW`], so
    /// the instants of the accepted calls are what that bound is counted over,
    /// and the ones that have left the window are dropped (see
    /// [`Self::test_failovers_in_window`]).
    pub test_failovers: Vec<SystemTime>,
    /// The SOCKETS this emulator has open for its caches, one per served cache —
    /// the data plane of each, keyed by the cache it serves; see
    /// [`cache_data_plane`].
    ///
    /// Kept beside the cache records rather than on them, because a record is a
    /// cloneable value and a live socket is not. A cache's entry is opened by its
    /// create and dropped by its delete, and dropping it closes the port.
    pub cache_data_planes: CacheDataPlanes,
}

impl Default for ComAmazonawsElasticacheAmazonelasticachev9State {
    /// A fresh store is an account ElastiCache has just become available in: no
    /// customer resources, the default parameter group of every family the
    /// service publishes, the account's built-in `default` user, and the
    /// catalogs this service publishes rather than an account creating them —
    /// the service updates and the reserved-cache-node offerings.
    ///
    /// The engine builds this the first time a handler of this service asks for
    /// its state, so that instant is the anchor the service-update catalog's
    /// release and end dates hang off — fixed for the life of the process, which
    /// is what makes every later read of an update report the same dates.
    ///
    /// The `default` user is seeded here for the same reason as the catalogs:
    /// ElastiCache creates it with the ACCOUNT, before any API call (see
    /// [`DEFAULT_USER_ID`]), so no reporting operation has to write it into the
    /// store on first read — which would put the region of whichever request
    /// read first into its ARN. Its ARN names [`DEFAULT_REGION`], the region
    /// this service treats a request as made in when nothing says otherwise.
    fn default() -> Self {
        Self {
            cache_clusters: BTreeMap::new(),
            cache_subnet_groups: BTreeMap::new(),
            user_groups: BTreeMap::new(),
            serverless_caches: BTreeMap::new(),
            serverless_cache_snapshots: BTreeMap::new(),
            replication_groups: BTreeMap::new(),
            global_replication_groups: BTreeMap::new(),
            cache_parameter_groups: default_parameter_groups(),
            snapshots: BTreeMap::new(),
            service_updates: published_service_updates(SystemTime::now()),
            update_actions: BTreeMap::new(),
            events: BTreeMap::new(),
            event_seq: 0,
            pending_cluster_notifications: Vec::new(),
            users: built_in_users(),
            cache_engine_versions: published_cache_engine_versions(),
            dns_token_seq: 0,
            reserved_cache_nodes_offerings: offering_catalog(),
            engine_default_parameters: published_engine_default_parameters(),
            reserved_cache_nodes: BTreeMap::new(),
            test_failovers: Vec::new(),
            // No cache exists yet, so no socket is open: a create opens one.
            cache_data_planes: CacheDataPlanes::default(),
        }
    }
}

/// The validated values a new cluster is built from — one argument so
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::insert_cluster`] does not
/// take a dozen positional strings.
#[derive(Debug, Clone)]
pub struct NewCluster {
    pub cache_cluster_id: String,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    pub num_cache_nodes: i32,
    pub port: i32,
    pub availability_zones: Vec<String>,
    pub az_mode: Option<String>,
    pub preferred_maintenance_window: String,
    pub cache_subnet_group_name: Option<String>,
    pub cache_parameter_group_name: String,
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    pub auto_minor_version_upgrade: Option<bool>,
    pub security_group_ids: Vec<String>,
    pub notification_topic_arn: Option<String>,
    pub auth_token_enabled: bool,
    /// The digests of the auth tokens a client of this cluster authenticates with;
    /// see [`CacheClusterRecord::auth_token_digests`].
    pub auth_token_digests: Vec<String>,
    pub transit_encryption_enabled: Option<bool>,
    /// See [`CacheClusterRecord::transit_encryption_mode`].
    pub transit_encryption_mode: Option<String>,
    /// See [`CacheClusterRecord::at_rest_encryption_enabled`].
    pub at_rest_encryption_enabled: Option<bool>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
    /// The replication group this cluster is a member of, if any; see
    /// [`CacheClusterRecord::replication_group_id`].
    pub replication_group_id: Option<String>,
    /// See [`CacheClusterRecord::replication_group_log_delivery_enabled`].
    pub replication_group_log_delivery_enabled: Option<bool>,
}

/// Why every operation on the legacy cache-security-group surface is refused.
///
/// A cache security group is the pre-VPC (EC2-Classic) access-control mechanism:
/// `CreateCacheSecurityGroup` documents that "cache security groups are only used
/// when you are creating a cluster outside of an Amazon Virtual Private Cloud
/// (Amazon VPC). If you are creating a cluster inside of a VPC, use a cache
/// subnet group instead." This emulator provisions no EC2-Classic network — a
/// cluster is always placed in a VPC subnet group ([`CacheSubnetGroupRecord`]) —
/// so the account it presents is VPC-only, which after EC2-Classic retirement is
/// the only kind of account AWS has. Every operation on the surface is therefore
/// refused account-wide with the modelled `InvalidParameterValueException`
/// (awsQuery code `InvalidParameterValue`, HTTP 400) rather than a per-name
/// not-found: the construct is unavailable, not the name. The status and the code
/// are the contract; this wording is the one AWS was recorded answering with.
pub const CACHE_SECURITY_GROUPS_UNSUPPORTED: &str =
    "Use of cache security groups is not permitted in this API version for your account.";

/// The length a user's password must have.
///
/// ElastiCache enforces the same password rule for a user as `AuthToken`
/// documents for a cluster: "Must be at least 16 characters and no more than 128
/// characters in length." A password outside it is refused with
/// `InvalidParameterValueException`, and no user is created — see
/// [`password_authentication`], which is where every password-bearing request
/// goes through it.
pub const PASSWORD_MIN_LENGTH: usize = 16;
/// See [`PASSWORD_MIN_LENGTH`].
pub const PASSWORD_MAX_LENGTH: usize = 128;

/// How many user groups the store holds before a create is refused with the
/// modelled quota fault. The emulator's own bound on the store, for the reason
/// [`CLUSTER_QUOTA`] gives, and the counterpart of [`USER_QUOTA`].
pub const USER_GROUP_QUOTA: usize = 100;

/// A replication group's and a Global datastore's statuses, from
/// `ReplicationGroup$Status` ("creating, available, modifying, deleting,
/// create-failed, snapshotting") and `GlobalReplicationGroup$Status`.
pub const STATUS_PRIMARY_ONLY: &str = "primary-only";

/// The `AutomaticFailoverStatus` / `MultiAZStatus` values, from those enums.
pub const FAILOVER_ENABLED: &str = "enabled";
/// See [`FAILOVER_ENABLED`].
pub const FAILOVER_DISABLED: &str = "disabled";

/// The `ClusterMode` values, from that enum (`enabled`, `disabled`,
/// `compatible`).
pub const CLUSTER_MODE_DISABLED: &str = "disabled";
/// See [`CLUSTER_MODE_DISABLED`].
pub const CLUSTER_MODE_ENABLED: &str = "enabled";

/// The role a member of a Global datastore has, from
/// `GlobalReplicationGroupMember$Role`: "Indicates the role of the replication
/// group, primary or secondary."
///
/// The member's prose names the two roles in lower case; the wire value is UPPER
/// case, which is what a client comparing the role it read against the role it
/// expects sees. The same value is what
/// `GlobalReplicationGroupInfo$GlobalReplicationGroupMemberRole` reports for a
/// replication group's own membership, so both are the one pair of constants.
pub const GLOBAL_ROLE_PRIMARY: &str = "PRIMARY";
/// See [`GLOBAL_ROLE_PRIMARY`].
pub const GLOBAL_ROLE_SECONDARY: &str = "SECONDARY";

/// Materialise a status change the clock owes a resource, if it is due.
///
/// The ONE write a reporting operation may make: the transition was owed at this
/// instant whether or not anybody called. Shared by every record that carries a
/// [`PendingTransition`] so the rule has one implementation.
fn settle_status(
    status: &mut String,
    pending: &mut Option<PendingTransition>,
    now: SystemTime,
) -> bool {
    let Some(transition) = pending.as_ref() else {
        return false;
    };
    if now < transition.due {
        return false;
    }
    *status = transition.status.clone();
    *pending = None;
    true
}

/// The transition the clock owes a resource that a call has just put in a
/// transient status: `status` once [`CLUSTER_TRANSITION`] has passed.
fn transition_to(status: &str, now: SystemTime) -> Option<PendingTransition> {
    Some(PendingTransition {
        due: now + CLUSTER_TRANSITION,
        status: status.to_string(),
    })
}

/// The transition the clock owes a just-created or just-modified user group:
/// [`USER_STATUS_ACTIVE`], which is what `UserGroup$Status` settles at.
pub fn user_group_transition(now: SystemTime) -> Option<PendingTransition> {
    transition_to(USER_STATUS_ACTIVE, now)
}

/// The transition the clock owes a just-modified user: [`USER_STATUS_ACTIVE`],
/// which is what `User$Status` ("Can be \"active\", \"modifying\" or
/// \"deleting\"") settles at once the change has been carried through.
pub fn user_transition(now: SystemTime) -> Option<PendingTransition> {
    transition_to(USER_STATUS_ACTIVE, now)
}

/// Why the user cannot take a change at this instant, or `None` when it can.
///
/// `InvalidUserStateFault` is "The user is not in active state", so a user still
/// carrying a change the clock has not settled, or one on its way out, takes no
/// further change. Stated once, over the settled status, so every operation that
/// changes a user agrees about which statuses it accepts.
pub fn user_state_refusal(user: &UserRecord) -> Option<String> {
    (user.status != USER_STATUS_ACTIVE).then(|| {
        format!(
            "User {} is in {} state, not active.",
            user.user_id, user.status
        )
    })
}

/// Why a user group cannot take a change at this instant, or `None` when it
/// can.
///
/// `InvalidUserGroupStateFault` is "the user group is not in an active state",
/// so a group that is still `creating`, already `modifying` or on its way out
/// is refused. Stated once, over the settled status, so every operation that
/// changes a group agrees about which statuses it accepts.
pub fn user_group_state_refusal(group: &UserGroupRecord) -> Option<String> {
    (group.status != USER_STATUS_ACTIVE).then(|| {
        format!(
            "User group {} is in status {}; it must be {} to be changed.",
            group.user_group_id, group.status, USER_STATUS_ACTIVE
        )
    })
}

/// `UserGroup$Status` of a group whose creation did not complete. Nothing in
/// this emulator puts a group in it — a create either is refused or settles
/// `active` — but it is one of the two statuses a DELETE admits, so the delete
/// guard below names it rather than assuming `active` is the only one.
pub const USER_GROUP_STATUS_CREATE_FAILED: &str = "create-failed";

/// Why the user group cannot be DELETED at this instant, or `None` when it can.
///
/// A delete is not a change to the group: `InvalidUserGroupStateFault` ("the
/// user group is not in an active state") refuses it while the group is still
/// being created, is carrying a change the clock has not settled, or is already
/// on its way out — but a group whose creation FAILED can be deleted, so the
/// refusal names BOTH admissible statuses rather than only `active`. The
/// group's own id and its current status are the variable parts; the tail
/// states the retry condition, which is what a caller waiting to delete needs.
pub fn user_group_delete_state_refusal(group: &UserGroupRecord) -> Option<String> {
    let deletable = [USER_STATUS_ACTIVE, USER_GROUP_STATUS_CREATE_FAILED];
    (!deletable.contains(&group.status.as_str())).then(|| {
        format!(
            "User group {} has status {}. Only {} or {} user groups can be deleted.",
            group.user_group_id,
            group.status,
            USER_STATUS_ACTIVE,
            USER_GROUP_STATUS_CREATE_FAILED
        )
    })
}

/// True when `value` is one of the engines access control exists for, lowercased
/// by the caller; see [`USER_ENGINES`].
pub fn is_user_engine(value: &str) -> bool {
    USER_ENGINES.contains(&value)
}

/// The ARN of a user group.
pub fn user_group_arn(region: &str, user_group_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("usergroup:{user_group_id}"))
}

/// The ARN of a Global datastore.
pub fn global_replication_group_arn(region: &str, global_replication_group_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("globalreplicationgroup:{global_replication_group_id}"),
    )
}

/// How many characters the prefix a Global datastore id carries has; see
/// [`global_datastore_prefix`].
pub const GLOBAL_ID_PREFIX_LEN: usize = 5;

/// The Global datastore id prefixes this service's own sources name: the one the
/// model publishes — "a Global datastore ID created in the US-West-1 region will
/// begin with `dsdfu`"
/// (`CreateGlobalReplicationGroupMessage$GlobalReplicationGroupIdSuffix`) — and
/// the one every Global datastore id AWS was recorded answering with in
/// `us-east-1` carries. Every other region's prefix is minted by
/// [`global_datastore_prefix`].
const PUBLISHED_GLOBAL_ID_PREFIXES: [(&str, &str); 2] =
    [("us-east-1", "ldgnf"), ("us-west-1", "dsdfu")];

/// The prefix a Global datastore id created in `region` begins with.
///
/// "Amazon ElastiCache automatically applies a prefix to the Global datastore ID
/// when it is created. Each Amazon Region has its own prefix … The suffix,
/// combined with the auto-generated prefix, guarantees uniqueness of the Global
/// datastore name across multiple regions." The model publishes the prefix of
/// one region ([`PUBLISHED_GLOBAL_ID_PREFIXES`]); for every other region the
/// prefix is MINTED here from the region name, so it has the published shape
/// ([`GLOBAL_ID_PREFIX_LEN`] lowercase letters), differs from region to region,
/// and is the same every time the same region is asked — which is what makes a
/// stored id stable across reads.
pub fn global_datastore_prefix(region: &str) -> String {
    let region = region.to_ascii_lowercase();
    if let Some((_, prefix)) = PUBLISHED_GLOBAL_ID_PREFIXES
        .iter()
        .find(|(published, _)| *published == region)
    {
        return (*prefix).to_string();
    }
    // FNV-1a over the region name: a stable, well-spread function of the whole
    // name, so two regions that differ anywhere get different prefixes.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in region.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (0..GLOBAL_ID_PREFIX_LEN)
        .map(|index| {
            let letter = (hash >> (index * 8)) % 26;
            char::from(b'a' + u8::try_from(letter).unwrap_or(0))
        })
        .collect()
}

/// The id a Global datastore created in `region` with `suffix` is stored and
/// reported under: the region's [`global_datastore_prefix`] and the suffix the
/// request named, lowercased like every other identifier of this service.
pub fn global_replication_group_id(region: &str, suffix: &str) -> String {
    format!(
        "{}-{}",
        global_datastore_prefix(region),
        suffix.to_ascii_lowercase()
    )
}

/// The keyspace each of `count` node groups owns, as `NodeGroup$Slots` and
/// `GlobalNodeGroup$Slots` report it.
///
/// One rule, one implementation: each range is [`default_slots`], the split a
/// replication group's shards already take, so a Global datastore's slice
/// groups and a replication group's shards cannot divide the keyspace
/// differently.
pub fn even_slot_ranges(count: usize) -> Vec<String> {
    let shards = i32::try_from(count).unwrap_or(i32::MAX);
    (0..shards)
        .map(|index| default_slots(shards, index))
        .collect()
}

/// The statuses a Global datastore accepts a change in: settled ones. A
/// datastore still `creating`, `modifying` or on its way out is refused with
/// `InvalidGlobalReplicationGroupStateFault`.
pub const GLOBAL_ACTIONABLE_STATUSES: [&str; 2] = [STATUS_AVAILABLE, STATUS_PRIMARY_ONLY];

/// How this service refuses a change to a Global datastore whose status cannot
/// take one: the datastore, the status it is in, and the statuses the change
/// needs ([`GLOBAL_ACTIONABLE_STATUSES`]).
///
/// One statement, so every operation that changes a Global datastore — the
/// associate a secondary create performs, the disassociate, the failover, the
/// two node-group changes, the rebalance, the modify and the delete — cannot
/// word its `InvalidGlobalReplicationGroupStateFault` differently from the
/// others.
pub fn global_replication_group_state_refusal(
    global_replication_group_id: &str,
    status: &str,
) -> String {
    format!(
        "Global Replication Group {global_replication_group_id} is in {status} state, not {}.",
        GLOBAL_ACTIONABLE_STATUSES.join(" or ")
    )
}

/// Why this service refuses a deferred change.
///
/// `ApplyImmediately` chooses between applying a change now and applying it "during
/// the next maintenance window". The `GlobalReplicationGroup` shape models NO
/// pending-values member, so a deferred change would have nowhere to be
/// reported: a caller would get a 200 and then read back the old settings with
/// nothing saying a change is coming. It is refused with the modelled
/// `InvalidParameterValueException` instead of being accepted and lost.
pub const APPLY_IMMEDIATELY_REQUIRED: &str =
    "Invalid value false for ApplyImmediately: this emulator applies a Global datastore change \
     at once, and the GlobalReplicationGroup shape has no member to report a pending one.";

/// Why a replica-count request is refused, and with which modelled error.
///
/// `IncreaseReplicaCount` and `DecreaseReplicaCount` declare the same two
/// client-input errors, so the shared argument check names which one its answer
/// is and each handler lifts it into its own error enum.
#[derive(Debug, Clone)]
pub enum ReplicaCountRefusal {
    /// `InvalidParameterValueException`: one value is not one the service
    /// serves.
    Value(String),
    /// `InvalidParameterCombinationException`: two members cannot hold
    /// together.
    Combination(String),
}

/// How many replicas a replica-count request asks each shard to end up with.
#[derive(Debug, Clone)]
pub enum ReplicaTargets {
    /// `NewReplicaCount`: "the number of read replica nodes you want at the end
    /// of this operation" — the same count for every shard of the group.
    EveryShard(i32),
    /// `ReplicaConfiguration`: a count per named shard.
    PerShard(Vec<(String, i32)>),
}

/// The per-shard replica counts a request asks for, or why it is refused.
///
/// The argument check both replica-count operations share, and it runs BEFORE
/// the replication group is resolved: `ReplicationGroupId` is validated as an
/// argument by these two operations rather than looked up first, which is why an
/// id the account does not hold is answered `InvalidParameterValue` here while
/// the rest of the replication-group surface answers
/// `ReplicationGroupNotFoundFault`.
///
/// CC-02 growth bound: the replicas a request asks for are allocated per
/// request, so every count — `NewReplicaCount` and each
/// `ReplicaConfiguration` entry's — is checked against
/// [`MAX_REPLICAS_PER_NODE_GROUP`] here, before any member is created, and a
/// configuration entry can only name a shard the group already has (the
/// handlers resolve it), which [`MAX_NODE_GROUPS`] bounds.
///
/// `minimum` is the smallest count the CALLING operation can leave a shard with
/// — see [`INCREASE_REPLICA_COUNT_MIN`] and [`DECREASE_REPLICA_COUNT_MIN`] — and
/// is what the refusal states, so the bound a client is told is the one its call
/// is actually held to.
pub fn replica_count_targets(
    minimum: i32,
    new_replica_count: Option<i32>,
    replica_configuration: &[ConfigureShard],
) -> Result<ReplicaTargets, ReplicaCountRefusal> {
    if new_replica_count.is_some() && !replica_configuration.is_empty() {
        return Err(ReplicaCountRefusal::Combination(
            "NewReplicaCount cannot be combined with ReplicaConfiguration.".to_string(),
        ));
    }
    let in_range = |count: i32| -> Result<(), ReplicaCountRefusal> {
        if (minimum..=MAX_REPLICAS_PER_NODE_GROUP).contains(&count) {
            Ok(())
        } else {
            Err(ReplicaCountRefusal::Value(replica_count_range_message(
                minimum,
            )))
        }
    };
    if let Some(count) = new_replica_count {
        in_range(count)?;
        return Ok(ReplicaTargets::EveryShard(count));
    }
    if replica_configuration.is_empty() {
        return Err(ReplicaCountRefusal::Value(
            "Invalid value for NewReplicaCount: name either NewReplicaCount or \
             ReplicaConfiguration."
                .to_string(),
        ));
    }
    let mut targets: Vec<(String, i32)> = Vec::new();
    for shard in replica_configuration {
        in_range(shard.new_replica_count)?;
        if targets.iter().any(|(id, _)| id == &shard.node_group_id) {
            return Err(ReplicaCountRefusal::Value(format!(
                "Invalid value {} for ReplicaConfiguration.NodeGroupId: it is configured more \
                 than once.",
                shard.node_group_id
            )));
        }
        // "The number of PreferredAvailabilityZone values must equal the value
        // of NewReplicaCount plus 1 to account for the primary node."
        if let Some(zones) = shard.preferred_availability_zones.as_deref() {
            let expected = usize::try_from(shard.new_replica_count.saturating_add(1)).unwrap_or(1);
            if zones.len() != expected {
                return Err(ReplicaCountRefusal::Value(format!(
                    "Invalid value for ReplicaConfiguration.PreferredAvailabilityZones: {} zones \
                     were given for a node group of {expected} nodes.",
                    zones.len()
                )));
            }
        }
        // Outposts: the emulator provisions no Outpost capacity, so a replica
        // cannot be placed in one.
        if let Some(arn) = shard
            .preferred_outpost_arns
            .as_deref()
            .unwrap_or_default()
            .first()
        {
            return Err(ReplicaCountRefusal::Value(format!(
                "Invalid value {arn} for ReplicaConfiguration.PreferredOutpostArns: this \
                 emulator provisions no Outpost capacity."
            )));
        }
        targets.push((shard.node_group_id.clone(), shard.new_replica_count));
    }
    Ok(ReplicaTargets::PerShard(targets))
}

/// One user group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct UserGroupRecord {
    pub user_group_id: String,
    pub engine: String,
    pub user_ids: Vec<String>,
    pub status: String,
    /// The status change the clock owes this group, if any.
    pub pending_status: Option<PendingTransition>,
    /// When the clock owes this group's REMOVAL from the store, set by the
    /// `DeleteUserGroup` that put it in [`STATUS_DELETING`]; `None` for a group
    /// nothing has deleted — the same rule a deleted user follows.
    pub pending_removal: Option<SystemTime>,
    /// The membership change the clock owes this group, as
    /// `ModifyUserGroup` accepted it: the ids to be added and the ids to be
    /// removed, which is what `UserGroup$PendingChanges` reports until
    /// [`Self::settle`] applies them to `user_ids` together with the status.
    pub pending_user_ids_to_add: Vec<String>,
    /// See [`Self::pending_user_ids_to_add`].
    pub pending_user_ids_to_remove: Vec<String>,
    /// The replication groups and serverless caches this group is attached to,
    /// which is what `UserGroup$ReplicationGroups` / `$ServerlessCaches` report.
    pub replication_group_ids: Vec<String>,
    pub serverless_cache_names: Vec<String>,
    pub region: String,
    pub arn: String,
    pub tags: Vec<(String, String)>,
}

impl UserGroupRecord {
    /// Materialise the change the clock owes this group: the status, and with it
    /// the membership change `ModifyUserGroup` accepted.
    ///
    /// The two fall due together — the group is `modifying` exactly while the
    /// change is pending — so one settle applies both and no read can see an
    /// `active` group that still has a change outstanding.
    pub fn settle(&mut self, now: SystemTime) {
        if !settle_status(&mut self.status, &mut self.pending_status, now) {
            return;
        }
        for user_id in ::std::mem::take(&mut self.pending_user_ids_to_add) {
            if !self.user_ids.contains(&user_id) {
                self.user_ids.push(user_id);
            }
        }
        let removed = ::std::mem::take(&mut self.pending_user_ids_to_remove);
        self.user_ids.retain(|member| !removed.contains(member));
    }

    /// `UserGroup$MinimumEngineVersion`: the lowest engine version that supports
    /// this group.
    ///
    /// A user group is an RBAC resource of one engine, so its floor is the
    /// oldest version that engine publishes at or above RBAC's own floor — the
    /// group itself uses no later feature, and the extra floor a member that
    /// authenticates with IAM needs is reported on that user's record. See
    /// [`rbac_minimum_engine_version`].
    pub fn minimum_engine_version(&self) -> &'static str {
        rbac_minimum_engine_version(&self.engine, USER_MIN_ENGINE_VERSION)
    }

    /// True while the clock still owes this group a membership change, which is
    /// what `UserGroup$PendingChanges` reports.
    pub fn has_pending_changes(&self) -> bool {
        !self.pending_user_ids_to_add.is_empty() || !self.pending_user_ids_to_remove.is_empty()
    }

    /// This record as the modelled `UserGroup`.
    pub fn to_modeled(&self) -> CreateUserGroupOutput {
        CreateUserGroupOutput {
            user_group_id: Some(self.user_group_id.clone()),
            status: Some(self.status.clone()),
            engine: Some(self.engine.clone()),
            user_ids: Some(self.user_ids.clone()),
            minimum_engine_version: Some(self.minimum_engine_version().to_string()),
            // "Returns the updates being applied to the user group": reported
            // only while `ModifyUserGroup` has left one outstanding, so a group
            // with nothing pending reports no `PendingChanges` at all.
            pending_changes: self.has_pending_changes().then(|| UserGroupPendingChanges {
                user_ids_to_add: Some(self.pending_user_ids_to_add.clone()),
                user_ids_to_remove: Some(self.pending_user_ids_to_remove.clone()),
            }),
            replication_groups: Some(self.replication_group_ids.clone()),
            serverless_caches: Some(self.serverless_cache_names.clone()),
            arn: Some(self.arn.clone()),
        }
    }
}

/// One member of a Global datastore, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct GlobalMemberRecord {
    pub replication_group_id: String,
    pub region: String,
    /// [`GLOBAL_ROLE_PRIMARY`] or [`GLOBAL_ROLE_SECONDARY`].
    pub role: String,
    pub automatic_failover: String,
    pub status: String,
}

/// One slice group of a Global datastore, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct GlobalNodeGroupRecord {
    pub global_node_group_id: String,
    pub slots: String,
}

/// The validated values a new Global datastore is built from.
///
/// A Global datastore has the configuration of the replication group it is built
/// around — its engine, engine version, node type, whether its keyspace is
/// partitioned and over how many slice groups, its encryption and its
/// auth-token setting — so `CreateGlobalReplicationGroup` reads every one of
/// those off that group rather than from the request.
#[derive(Debug, Clone)]
pub struct NewGlobalReplicationGroup {
    pub global_replication_group_id: String,
    pub description: Option<String>,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    /// The replication group that accepts writes, which becomes the datastore's
    /// primary member.
    pub primary_replication_group_id: String,
    /// The primary member's `AutomaticFailover`, which is the primary
    /// replication group's own: see [`ReplicationGroupRecord::automatic_failover`].
    pub primary_automatic_failover: String,
    pub node_group_count: i32,
    pub cluster_enabled: bool,
    /// Whether the primary was created with an `AuthToken`; the token itself is
    /// a SECRET and is never stored. See
    /// [`CacheClusterRecord::auth_token_enabled`].
    pub auth_token_enabled: bool,
    pub transit_encryption_enabled: Option<bool>,
    pub at_rest_encryption_enabled: Option<bool>,
    /// The tags the create was given; see [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// One Global datastore, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct GlobalReplicationGroupRecord {
    pub global_replication_group_id: String,
    pub description: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingTransition>,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    pub members: Vec<GlobalMemberRecord>,
    pub cluster_enabled: bool,
    pub global_node_groups: Vec<GlobalNodeGroupRecord>,
    pub auth_token_enabled: bool,
    pub transit_encryption_enabled: Option<bool>,
    pub at_rest_encryption_enabled: Option<bool>,
    pub arn: String,
    pub tags: Vec<(String, String)>,
}

impl GlobalReplicationGroupRecord {
    /// A Global datastore with one primary member, in the status a just-created
    /// datastore has.
    ///
    /// It answers `creating` until the clock settles it at
    /// [`Self::settled_status`] — `primary-only` for a datastore that has only
    /// the primary it was built around, which is what that status means.
    pub fn new(new: NewGlobalReplicationGroup) -> Self {
        let mut record = Self {
            arn: global_replication_group_arn(&new.region, &new.global_replication_group_id),
            global_replication_group_id: new.global_replication_group_id,
            description: new.description,
            status: STATUS_CREATING.to_string(),
            pending_status: None,
            engine: new.engine,
            engine_version: new.engine_version,
            cache_node_type: new.cache_node_type,
            members: vec![GlobalMemberRecord {
                replication_group_id: new.primary_replication_group_id,
                region: new.region.clone(),
                role: GLOBAL_ROLE_PRIMARY.to_string(),
                automatic_failover: new.primary_automatic_failover,
                status: STATUS_ASSOCIATED.to_string(),
            }],
            cluster_enabled: new.cluster_enabled,
            global_node_groups: Vec::new(),
            auth_token_enabled: new.auth_token_enabled,
            transit_encryption_enabled: new.transit_encryption_enabled,
            at_rest_encryption_enabled: new.at_rest_encryption_enabled,
            region: new.region,
            tags: new.tags,
        };
        record.pending_status = transition_to(record.settled_status(), new.now);
        record.set_node_group_count(new.node_group_count);
        record
    }

    /// Materialise the status change the clock owes this datastore.
    pub fn settle(&mut self, now: SystemTime) {
        settle_status(&mut self.status, &mut self.pending_status, now);
    }

    /// Start the span after which the clock owes this datastore its settled
    /// status, leaving it in `status` until then.
    ///
    /// The settled status is [`STATUS_AVAILABLE`] while the datastore has a
    /// secondary member and [`STATUS_PRIMARY_ONLY`] once it has none, which is
    /// what that status means.
    pub fn begin_transition(&mut self, status: &str, now: SystemTime) {
        let settled = self.settled_status();
        self.status = status.to_string();
        self.pending_status = transition_to(settled, now);
    }

    /// The status this datastore settles at; see
    /// [`GlobalReplicationGroupRecord::begin_transition`].
    pub fn settled_status(&self) -> &'static str {
        if self
            .members
            .iter()
            .any(|member| member.role != GLOBAL_ROLE_PRIMARY)
        {
            STATUS_AVAILABLE
        } else {
            STATUS_PRIMARY_ONLY
        }
    }

    /// The member that accepts writes, if this datastore still has one.
    pub fn primary(&self) -> Option<&GlobalMemberRecord> {
        self.members
            .iter()
            .find(|member| member.role == GLOBAL_ROLE_PRIMARY)
    }

    /// Give this datastore `count` slice groups, keeping the ones it already has
    /// and re-slicing the keyspace evenly over the result.
    ///
    /// CC-02 growth bound: `count` is clamped to [`MAX_NODE_GROUPS`] here as
    /// well as refused above it by the callers, so this allocation cannot be
    /// driven past the shard quota by any request.
    pub fn set_node_group_count(&mut self, count: i32) {
        let target = usize::try_from(count.clamp(0, MAX_NODE_GROUPS)).unwrap_or(0);
        while self.global_node_groups.len() > target {
            self.global_node_groups.pop();
        }
        while self.global_node_groups.len() < target {
            let ordinal = self.global_node_groups.len() + 1;
            self.global_node_groups.push(GlobalNodeGroupRecord {
                global_node_group_id: format!(
                    "{}-{ordinal:04}",
                    self.global_replication_group_id
                ),
                slots: String::new(),
            });
        }
        self.rebalance_slots();
    }

    /// Re-slice the keyspace evenly over this datastore's slice groups, which is
    /// what `RebalanceSlotsInGlobalReplicationGroup` does.
    pub fn rebalance_slots(&mut self) {
        let ranges = even_slot_ranges(self.global_node_groups.len());
        for (group, range) in self.global_node_groups.iter_mut().zip(ranges) {
            group.slots = range;
        }
    }

    /// Keep only the slice groups `retain` names, then re-slice the keyspace.
    pub fn retain_node_groups(&mut self, retain: &[String]) {
        self.global_node_groups
            .retain(|group| retain.contains(&group.global_node_group_id));
        self.rebalance_slots();
    }

    /// The member `replication_group_id` has in `region`, if it is one.
    pub fn member(&self, replication_group_id: &str, region: &str) -> Option<&GlobalMemberRecord> {
        self.members.iter().find(|member| {
            member.replication_group_id == replication_group_id && member.region == region
        })
    }

    /// This record as the modelled `GlobalReplicationGroup`.
    ///
    /// `Members` is reported only when the caller asked for it
    /// (`DescribeGlobalReplicationGroups$ShowMemberInfo`: "Returns the list of
    /// members that comprise the Global datastore"); the mutating operations
    /// return the datastore they changed, members and all.
    pub fn to_modeled(&self, include_members: bool) -> GlobalReplicationGroup {
        GlobalReplicationGroup {
            global_replication_group_id: Some(self.global_replication_group_id.clone()),
            global_replication_group_description: self.description.clone(),
            status: Some(self.status.clone()),
            cache_node_type: self.cache_node_type.clone(),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            members: include_members.then(|| {
                self.members
                    .iter()
                    .map(|member| GlobalReplicationGroupMember {
                        replication_group_id: Some(member.replication_group_id.clone()),
                        replication_group_region: Some(member.region.clone()),
                        role: Some(member.role.clone()),
                        automatic_failover: Some(member.automatic_failover.clone()),
                        status: Some(member.status.clone()),
                    })
                    .collect()
            }),
            cluster_enabled: Some(self.cluster_enabled),
            global_node_groups: Some(
                self.global_node_groups
                    .iter()
                    .map(|group| GlobalNodeGroup {
                        global_node_group_id: Some(group.global_node_group_id.clone()),
                        slots: Some(group.slots.clone()),
                    })
                    .collect(),
            ),
            auth_token_enabled: Some(self.auth_token_enabled),
            transit_encryption_enabled: self.transit_encryption_enabled,
            at_rest_encryption_enabled: self.at_rest_encryption_enabled,
            arn: Some(self.arn.clone()),
        }
    }
}

/// The status a replication group has as a member of a Global datastore, which
/// is what `GlobalReplicationGroupMember$Status` reports.
pub const STATUS_ASSOCIATED: &str = "associated";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticacheAmazonelasticachev9State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticacheAmazonelasticachev9State>)
}
