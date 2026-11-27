// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The cache engines a replication group may run, from
/// `CreateReplicationGroup$Engine`: "The value must be set to valkey or redis",
/// and the operation itself "is valid for Valkey and Redis OSS only".
pub const REPLICATION_GROUP_ENGINES: [&str; 2] = [DEFAULT_ENGINE, ENGINE_VALKEY];

/// The largest `NumCacheClusters`, as the member documents it: "The maximum
/// permitted value for NumCacheClusters is 6 (1 primary plus 5 replicas)." — the
/// primary plus the replicas a node group may have, which is why it is derived
/// from that bound rather than written out a second time.
pub const MAX_CACHE_CLUSTERS_PER_GROUP: i32 = 1 + MAX_REPLICAS_PER_NODE_GROUP;

/// The largest `ReplicasPerNodeGroup` / `NodeGroupConfiguration$ReplicaCount`,
/// as `ReplicasPerNodeGroup` documents it: "Valid values are 0 to 5."
pub const MAX_REPLICAS_PER_NODE_GROUP: i32 = 5;

/// The smallest replica count a node group (shard) may be created with, from the
/// same sentence: "Valid values are 0 to 5" — a group of the primary alone.
pub const MIN_REPLICAS_PER_NODE_GROUP: i32 = 0;

/// How ElastiCache refuses a create whose request asks a node group (shard) for a
/// number of replicas outside the bounds that count documents.
///
/// The whole sentence is fixed text apart from the two BOUNDS, which the member
/// publishes; nothing of the value that missed them, and nothing of the member
/// that carried it, appears in it. Every member of a create that sets the count
/// sets the SAME count — `ReplicasPerNodeGroup` and a shard's `ReplicaCount`
/// state it directly, `NumCacheClusters` states it as the primary plus its
/// replicas — so one sentence states the rule for all of them.
pub fn replicas_per_node_group_range_message() -> String {
    format!(
        "The number of replicas per node group must be within \
         {MIN_REPLICAS_PER_NODE_GROUP} and {MAX_REPLICAS_PER_NODE_GROUP}."
    )
}

/// How many replicas a node group (shard) gets when the request asks for
/// AUTOMATIC FAILOVER but names no replica count of its own: one per shard.
///
/// Automatic failover promotes a replica, so a shard needs one to promote —
/// "If AutomaticFailoverEnabled is true, the value of [NumCacheClusters] must be
/// at least 2", a primary plus this one replica. The count is what the service
/// DEFAULTS to for the requested mode, which is why a request that enables
/// failover without naming `ReplicasPerNodeGroup` is served rather than refused.
pub const REPLICAS_PER_NODE_GROUP_FOR_FAILOVER: i32 = 1;

/// The smallest `NewReplicaCount` `IncreaseReplicaCount` accepts: an increase
/// ends with at least one replica, so a request to end with none asks this
/// operation for something it cannot do.
pub const INCREASE_REPLICA_COUNT_MIN: i32 = 1;

/// The smallest `NewReplicaCount` `DecreaseReplicaCount` accepts: "The minimum
/// number of replicas in a shard or replication group is … if Multi-AZ is not
/// enabled: 0" (`DecreaseReplicaCount$NewReplicaCount`), so a decrease may take a
/// shard down to none.
pub const DECREASE_REPLICA_COUNT_MIN: i32 = 0;

/// How ElastiCache states the replica-count range a request fell outside of: the
/// bounds the CALLING operation holds the count to, and nothing of the value that
/// missed them.
pub fn replica_count_range_message(minimum: i32) -> String {
    format!(
        "New replica count must be between {minimum} and {MAX_REPLICAS_PER_NODE_GROUP} replicas."
    )
}

/// How ElastiCache refuses `NumCacheClusters` on a request that also asks for
/// more than one shard: the member "is not used if there is more than one node
/// group (shard)", which is the rule the message states.
pub const NUM_CACHE_CLUSTERS_ONE_SHARD_MESSAGE: &str =
    "NumCacheClusters can only be specified for one node group.";

/// The largest `NumNodeGroups`, from
/// `NodeGroupsPerReplicationGroupQuotaExceededFault`: "The request cannot be
/// processed because it would exceed the maximum allowed number of node groups
/// (shards) in a single replication group. The default maximum is 90".
pub const MAX_NODE_GROUPS: i32 = 90;

/// How many hash slots a Valkey / Redis OSS keyspace holds, from
/// `NodeGroupConfiguration$Slots`: "Keyspaces range from 0 to 16,383."
pub const KEYSPACE_SLOTS: i32 = 16_384;

/// How many shards an account may test automatic failover on in one rolling
/// window, as `TestFailover` documents it: "A customer can use this operation to
/// test automatic failover on up to 15 shards (called node groups in the
/// ElastiCache API and Amazon CLI) in any rolling 24-hour period." Past it the
/// call is refused with the modelled `APICallRateForCustomerExceededFault`.
pub const MAX_TEST_FAILOVERS_PER_WINDOW: usize = 15;

/// The window [`MAX_TEST_FAILOVERS_PER_WINDOW`] is counted over, from the same
/// sentence: "any rolling 24-hour period".
pub const TEST_FAILOVER_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a test of automatic failover on one shard is still being carried out
/// after the call that asked for it, which is the window a second test of the
/// SAME shard is refused in.
///
/// `TestFailover`'s own documented sequence is the work: the primary role moves,
/// the old primary's node is replaced, and "the node replacement must complete
/// before a subsequent call can be made". The SPAN is this emulator's chosen one
/// — the same [`CLUSTER_TRANSITION`] every transient phase of this service is
/// held for, long enough that a caller acting straight after the accepted call
/// meets the refusal — and nothing sleeps for it: the instant it falls due is
/// stored on the group ([`ReplicationGroupRecord::test_failovers_in_flight`]).
/// Being that same span, the shard stops refusing a test exactly as the group
/// settles back out of the status the replacement runs in, so a caller retrying
/// the call is answered either the shard's own conflict or the acceptance,
/// never the group's state refusal for work of its own shard.
pub const TEST_FAILOVER_WORK: Duration = CLUSTER_TRANSITION;

/// How ElastiCache refuses a `TestFailover` on a shard it is already testing
/// failover on: the sentence states that the work is under way and names
/// neither the group, the shard nor a status — the conflict belongs to the shard
/// rather than to the group a caller would be polling.
pub const TEST_FAILOVER_IN_PROGRESS_MESSAGE: &str = "Test failover already in progress";

/// `AuthToken`'s documented length bounds: "Must be at least 16 characters and
/// no more than 128 characters in length."
pub const AUTH_TOKEN_MIN_LEN: usize = 16;
/// See [`AUTH_TOKEN_MIN_LEN`].
pub const AUTH_TOKEN_MAX_LEN: usize = 128;

/// The printable characters an `AuthToken` may NOT carry: "Cannot contain any of
/// the following characters: '/', '\"', or '@', '%'"
/// (`smithy_query(query="doc", shape="ModifyReplicationGroupMessage",
/// member="AuthToken")`, the same password the create takes).
///
/// The member's older wording names an allow-list of eight printable specials
/// instead (`smithy_query(query="doc",
/// shape="CreateReplicationGroupMessage", member="AuthToken")`); the two state
/// one password rule, and this is the half of it the service enforces — a
/// password drawn from the URL-safe base64 alphabet, `_` included, is a password
/// ElastiCache accepts, while one carrying a character out of this list is not.
pub const AUTH_TOKEN_RESERVED_CHARACTERS: [char; 4] = ['/', '"', '@', '%'];

/// The inclusive range "Must be only printable ASCII characters" names: the
/// space through `~` (0x20..=0x7E). A token carrying a control character, or any
/// character outside ASCII at all, is not printable ASCII.
pub const AUTH_TOKEN_PRINTABLE_ASCII: ::std::ops::RangeInclusive<char> = ' '..='~';

/// The node-type family data tiering requires, from `DataTieringEnabled`: "Data
/// tiering is only supported for replication groups using the r6gd node type.
/// This parameter must be set to true when using r6gd nodes."
pub const DATA_TIERING_NODE_TYPE: &str = "r6gd";

/// The `enabled` / `disabled` of `AutomaticFailoverStatus`, `MultiAZStatus` and
/// `DataTieringStatus` (`smithy_query(query="shape", shape="MultiAZStatus")`).
pub const SETTING_ENABLED: &str = "enabled";
/// See [`SETTING_ENABLED`].
pub const SETTING_DISABLED: &str = "disabled";

/// `ClusterMode`'s third value (`smithy_query(query="shape",
/// shape="ClusterMode")`: `enabled | disabled | compatible`), the step a group
/// has to pass through to reach `enabled`: "To modify cluster mode from
/// Disabled to Enabled, you must first set the cluster mode to Compatible."
pub const CLUSTER_MODE_COMPATIBLE: &str = "compatible";

/// `TransitEncryptionMode`'s values (`smithy_query(query="shape",
/// shape="TransitEncryptionMode")`: `preferred | required`). "Setting
/// TransitEncryptionMode to required is a two-step process that requires you to
/// first set the TransitEncryptionMode to preferred."
pub const TRANSIT_ENCRYPTION_PREFERRED: &str = "preferred";
/// See [`TRANSIT_ENCRYPTION_PREFERRED`].
pub const TRANSIT_ENCRYPTION_REQUIRED: &str = "required";

/// `AuthTokenUpdateStrategyType`'s values (`smithy_query(query="shape",
/// shape="AuthTokenUpdateStrategyType")`: `SET | ROTATE | DELETE`), with the
/// documented order between them: "ROTATE - default, if no update strategy is
/// provided. SET - allowed only after ROTATE. DELETE - allowed only when
/// transitioning to RBAC."
pub const AUTH_TOKEN_ROTATE: &str = "ROTATE";
/// See [`AUTH_TOKEN_ROTATE`].
pub const AUTH_TOKEN_SET: &str = "SET";
/// See [`AUTH_TOKEN_ROTATE`].
pub const AUTH_TOKEN_DELETE: &str = "DELETE";

/// `AuthTokenUpdateStatus`'s values (`smithy_query(query="shape",
/// shape="AuthTokenUpdateStatus")`: `SETTING | ROTATING`): what a DEFERRED
/// auth-token update of each strategy is reported as until it is applied.
pub const AUTH_TOKEN_STATUS_ROTATING: &str = "ROTATING";
/// See [`AUTH_TOKEN_STATUS_ROTATING`].
pub const AUTH_TOKEN_STATUS_SETTING: &str = "SETTING";

/// The longest `ReplicationGroupDescription` a replication group may carry, as
/// `ModifyReplicationGroup$ReplicationGroupDescription` documents it: "A
/// description for the replication group. Maximum length is 255 characters."
/// The member carries no modelled `@length`, so the bound is enforced in the
/// handler.
pub const MAX_REPLICATION_GROUP_DESCRIPTION_LENGTH: usize = 255;

/// `NodeGroupMember$CurrentRole`'s values: "The role that is currently assigned
/// to the node - primary or replica."
pub const ROLE_PRIMARY: &str = "primary";
/// See [`ROLE_PRIMARY`].
pub const ROLE_REPLICA: &str = "replica";

/// `StorageEncryptionType`'s values (`smithy_query(query="shape",
/// shape="StorageEncryptionType")`: `none | sse-elasticache | sse-kms`), which
/// `ReplicationGroup$StorageEncryptionType` documents as "none if at-rest
/// encryption is not enabled, sse-elasticache if an ElastiCache service-managed
/// key is used, or sse-kms if a customer-managed KMS key is used". The two
/// key-bearing values are declared with the serverless-cache slice, which
/// documents them the same way.
pub const STORAGE_ENCRYPTION_NONE: &str = "none";

/// `Durability`'s `default`: "When set to default, the service determines the
/// effective durability based on the engine version, cluster mode, and other
/// parameters. The resolved setting is reflected in the EffectiveDurability
/// property."
pub const DURABILITY_DEFAULT: &str = "default";

/// What [`DURABILITY_DEFAULT`] resolves to here, and the only value
/// `EffectiveDurability` can take for a group this operation creates:
/// `CreateReplicationGroup`'s own documentation states the replication this
/// service performs — "Writes to the primary are asynchronously propagated to
/// the replicas" — and `async` is that setting's name in `EffectiveDurability`
/// (`async | sync | disabled`).
pub const EFFECTIVE_DURABILITY_ASYNC: &str = "async";

/// The id of the one cache node a Valkey / Redis OSS member cluster holds; node
/// ids are "a numeric identifier (0001, 0002, etc.)" and
/// [`REDIS_NUM_CACHE_NODES`] is 1, so a member cluster's node is always this one.
pub const FIRST_CACHE_NODE_ID: &str = "0001";

/// The ARN of a replication group:
/// `arn:aws:elasticache:<region>:<account>:replicationgroup:<id>`.
pub fn replication_group_arn(region: &str, replication_group_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("replicationgroup:{replication_group_id}"),
    )
}

/// `engine` lowercased when it names an engine a replication group may run,
/// else the reason it is refused. See [`REPLICATION_GROUP_ENGINES`].
pub fn normalize_replication_group_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if REPLICATION_GROUP_ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(format!(
            "Invalid value {engine} for Engine: a replication group must run one of {}.",
            REPLICATION_GROUP_ENGINES.join(" | ")
        ))
    }
}

/// `None` when `token` satisfies every documented `AuthToken` constraint, else
/// the sentence the request is refused with: a password is "only printable ASCII
/// characters", "at least 16 characters and no more than 128 characters in
/// length", and "[c]annot contain any of the following characters: '/', '\"', or
/// '@', '%'" — see [`AUTH_TOKEN_RESERVED_CHARACTERS`]. Every other printable
/// ASCII character is one a password may be drawn from, so a random URL-safe
/// secret is a token this service takes.
///
/// The constraints are several, and ElastiCache answers ONE sentence for all of
/// them — `Invalid AuthToken provided.` — so a token that is too short, too long
/// or carries a character the engine's password alphabet excludes is refused the
/// same way. The same rule holds wherever the API takes a cache's password
/// (`CreateReplicationGroup`, `ModifyReplicationGroup`, `CreateCacheCluster` and
/// `ModifyCacheCluster` all document these constraints), so every writer of one
/// comes through here.
pub fn auth_token_refusal(token: &str) -> Option<String> {
    let permitted = |character: char| {
        AUTH_TOKEN_PRINTABLE_ASCII.contains(&character)
            && !AUTH_TOKEN_RESERVED_CHARACTERS.contains(&character)
    };
    if !(AUTH_TOKEN_MIN_LEN..=AUTH_TOKEN_MAX_LEN).contains(&token.chars().count())
        || !token.chars().all(permitted)
    {
        return Some(INVALID_AUTH_TOKEN_MESSAGE.to_string());
    }
    None
}

/// The one sentence a token that is not a valid `AuthToken` is refused with.
pub const INVALID_AUTH_TOKEN_MESSAGE: &str = "Invalid AuthToken provided.";

/// The sentence a request carrying an `AuthToken` without encryption in transit
/// is refused with.
///
/// "AuthToken can be specified only on replication groups where
/// TransitEncryptionEnabled is true", worded as what it is — a password the
/// connection cannot carry — and with no trailing stop.
pub const AUTH_TOKEN_NEEDS_TRANSIT_ENCRYPTION_MESSAGE: &str =
    "The AUTH token is only supported when encryption-in-transit is enabled";

/// The sentence a request that asks for automatic failover on a replication
/// group holding ONE cache cluster is refused with.
///
/// "If AutomaticFailoverEnabled is true, the value of [NumCacheClusters] must be
/// at least 2" — failover promotes another cache cluster of the group, so a
/// group of one has nothing to promote. ElastiCache states that as the COUNT of
/// cache clusters such a group needs, in wholly fixed text.
pub const AUTOMATIC_FAILOVER_NEEDS_TWO_CLUSTERS_MESSAGE: &str =
    "When using automatic failover, there must be at least 2 cache clusters in the replication \
     group.";

/// The inclusive slot range a `Slots` string names, or the reason it names none.
///
/// "A string that specifies the keyspace for a particular node group. Keyspaces
/// range from 0 to 16,383. The string is in the format `startkey-endkey`.
/// Example: `"0-3999"`."
pub fn slot_range(slots: &str) -> Result<(i32, i32), String> {
    let refusal = || {
        format!(
            "Invalid value {slots} for Slots: a keyspace is written startkey-endkey, with both \
             keys between 0 and {}.",
            KEYSPACE_SLOTS - 1
        )
    };
    let (start, end) = slots.split_once('-').ok_or_else(refusal)?;
    let start: i32 = start.trim().parse().map_err(|_| refusal())?;
    let end: i32 = end.trim().parse().map_err(|_| refusal())?;
    if start < 0 || end < start || end >= KEYSPACE_SLOTS {
        return Err(refusal());
    }
    Ok((start, end))
}

/// The keyspace of shard `index` of `num_node_groups` when the request named
/// none: the 16,384 slots split as evenly as they divide, in order, the first
/// shards taking the remainder, so the shards together cover the whole keyspace
/// exactly once.
pub fn default_slots(num_node_groups: i32, index: i32) -> String {
    let total = i64::from(KEYSPACE_SLOTS);
    let shards = i64::from(num_node_groups.max(1));
    let index = i64::from(index);
    let size = total / shards;
    let remainder = total % shards;
    // Each shard before this one took `size` slots, plus one more while the
    // remainder lasted.
    let start = index * size + index.min(remainder);
    let end = start + size + i64::from(index < remainder) - 1;
    format!("{start}-{end}")
}

/// The id ElastiCache gives the `member_ordinal`-th member cluster (1-based) of
/// the node group (shard) `node_group_id` names.
///
/// A Valkey or Redis OSS (cluster mode disabled) group numbers its member
/// clusters `<group>-001`, `<group>-002`, …; a cluster mode enabled group names
/// the shard too, `<group>-0001-001`. Both are derived from the group id and the
/// shard's own id, so a member cluster is always resolvable from the group and
/// the shard a caller knows.
pub fn member_cluster_id(
    replication_group_id: &str,
    node_group_id: &str,
    member_ordinal: usize,
    cluster_enabled: bool,
) -> String {
    if cluster_enabled {
        format!("{replication_group_id}-{node_group_id}-{member_ordinal:03}")
    } else {
        format!("{replication_group_id}-{member_ordinal:03}")
    }
}

/// One member of a node group (shard), as this emulator persists it: which
/// cluster and node it is, where it sits and which role it plays.
#[derive(Debug, Clone)]
pub struct NodeGroupMemberRecord {
    pub cache_cluster_id: String,
    pub cache_node_id: String,
    pub availability_zone: Option<String>,
    /// [`ROLE_PRIMARY`] or [`ROLE_REPLICA`].
    pub current_role: String,
}

impl NodeGroupMemberRecord {
    /// This member as the modelled `NodeGroupMember`.
    ///
    /// `ReadEndpoint` and `CurrentRole` are reported only for a cluster mode
    /// DISABLED group: the model says of each that it "is only applicable for
    /// Valkey or Redis OSS (cluster mode disabled)" groups, which are reached
    /// per node rather than through a configuration endpoint.
    pub fn to_modeled(
        &self,
        cluster_enabled: bool,
        clusters: &BTreeMap<String, CacheClusterRecord>,
    ) -> NodeGroupMember {
        NodeGroupMember {
            cache_cluster_id: Some(self.cache_cluster_id.clone()),
            cache_node_id: Some(self.cache_node_id.clone()),
            read_endpoint: if cluster_enabled {
                None
            } else {
                self.read_endpoint(clusters)
            },
            preferred_availability_zone: self.availability_zone.clone(),
            // Outposts are not modelled by this service; see
            // `CacheClusterRecord::to_modeled`.
            preferred_outpost_arn: None,
            current_role: (!cluster_enabled).then(|| self.current_role.clone()),
        }
    }

    /// The endpoint a client reads this member through: the endpoint of the one
    /// cache node of its member cluster, read out of that cluster's record so
    /// there is a single copy of it. It appears once the node is provisioned,
    /// exactly as `DescribeCacheClusters` reports the node itself.
    pub(super) fn read_endpoint(&self, clusters: &BTreeMap<String, CacheClusterRecord>) -> Option<Endpoint> {
        clusters
            .get(&self.cache_cluster_id)?
            .nodes
            .iter()
            .find(|node| node.cache_node_id == self.cache_node_id)?
            .to_modeled()
            .endpoint
    }
}

/// One node group (shard) of a replication group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct NodeGroupRecord {
    /// "The identifier for the node group (shard). A Valkey or Redis OSS
    /// (cluster mode disabled) replication group contains only 1 node group;
    /// therefore, the node group ID is 0001."
    pub node_group_id: String,
    /// One of `NodeGroup$Status`'s values ("creating, available, modifying,
    /// deleting").
    pub status: String,
    /// "The keyspace for this node group (shard)", reported for a cluster mode
    /// enabled group, where the keyspace is partitioned across shards.
    pub slots: Option<String>,
    /// This shard's members, the primary first.
    ///
    /// The ORDER is this emulator's own: `NodeGroupMembers` carries no modelled
    /// ordering, so a client must read a member's role from `CurrentRole`
    /// rather than from its position. Holding the primary first is what lets
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::set_shard_replica_count`]
    /// drop the newest replicas from the end while keeping the primary, and
    /// [`NodeGroupRecord::fail_over`] maintains it.
    pub members: Vec<NodeGroupMemberRecord>,
}

impl NodeGroupRecord {
    /// Fail this shard over: the first replica takes the primary role and the
    /// primary it had becomes a replica, which is what an automatic failover
    /// does to a shard. Hands back `(demoted cluster, promoted cluster)`, or
    /// `None` when the shard has no primary or no replica to promote.
    ///
    /// The promoted member is moved to the front, because this record holds its
    /// members "the primary first" and [`ReplicationGroupRecord`]'s replica
    /// scaling relies on that order.
    pub fn fail_over(&mut self) -> Option<(String, String)> {
        let primary = self
            .members
            .iter()
            .position(|member| member.current_role == ROLE_PRIMARY)?;
        let replica = self
            .members
            .iter()
            .position(|member| member.current_role != ROLE_PRIMARY)?;
        self.swap_roles(primary, replica)
    }

    /// Promote the member cluster `cache_cluster_id` of this shard to the
    /// primary role, demoting the primary it had — which is what
    /// `ModifyReplicationGroup$PrimaryClusterId` asks for. Hands back
    /// `(demoted cluster, promoted cluster)`, or `None` when this shard has no
    /// such replica, or no primary to demote.
    ///
    /// Promotion by NAME and [`Self::fail_over`] are the same move over this
    /// shard's members, so they go through the one implementation below rather
    /// than two copies that can disagree about the member order.
    pub fn promote(&mut self, cache_cluster_id: &str) -> Option<(String, String)> {
        let primary = self
            .members
            .iter()
            .position(|member| member.current_role == ROLE_PRIMARY)?;
        let replica = self.members.iter().position(|member| {
            member.current_role != ROLE_PRIMARY && member.cache_cluster_id == cache_cluster_id
        })?;
        self.swap_roles(primary, replica)
    }

    /// Give the member at `replica` the primary role and the one at `primary`
    /// the replica role, keeping the primary at the front of `members`.
    pub(super) fn swap_roles(&mut self, primary: usize, replica: usize) -> Option<(String, String)> {
        self.members[primary].current_role = ROLE_REPLICA.to_string();
        self.members[replica].current_role = ROLE_PRIMARY.to_string();
        let demoted = self.members[primary].cache_cluster_id.clone();
        let promoted = self.members[replica].cache_cluster_id.clone();
        let promoted_member = self.members.remove(replica);
        self.members.insert(0, promoted_member);
        Some((demoted, promoted))
    }

    /// This shard's members as a cluster-aware client routes to them: the member cluster id
    /// of each and whether it holds the PRIMARY role, the primary first.
    ///
    /// The role rather than the stored order is what decides it, so a shard that has been
    /// failed over reports the member holding the role now.
    pub fn nodes_by_role(&self) -> Vec<(String, bool)> {
        let mut nodes: Vec<(String, bool)> = self
            .members
            .iter()
            .map(|member| {
                (
                    member.cache_cluster_id.clone(),
                    member.current_role == ROLE_PRIMARY,
                )
            })
            .collect();
        nodes.sort_by_key(|(_, primary)| !*primary);
        nodes
    }

    /// This shard as the modelled `NodeGroup`.
    ///
    /// `PrimaryEndpoint` / `ReaderEndpoint` are the cluster mode DISABLED
    /// group's endpoints: a cluster mode enabled group is reached through the
    /// group's `ConfigurationEndpoint` instead. Both are reported for every
    /// cluster mode disabled shard — the reader endpoint is where a client sends
    /// its reads, and a shard with no replica to read from serves them from the
    /// member holding the primary role.
    pub fn to_modeled(
        &self,
        group: &ReplicationGroupRecord,
        clusters: &BTreeMap<String, CacheClusterRecord>,
    ) -> NodeGroup {
        let per_node_endpoints = !group.cluster_enabled;
        NodeGroup {
            node_group_id: Some(self.node_group_id.clone()),
            status: Some(self.status.clone()),
            primary_endpoint: per_node_endpoints.then(|| self.primary_endpoint(group, clusters)),
            reader_endpoint: per_node_endpoints.then(|| self.reader_endpoint(group, clusters)),
            slots: self.slots.clone(),
            node_group_members: Some(
                self.members
                    .iter()
                    .map(|member| member.to_modeled(group.cluster_enabled, clusters))
                    .collect(),
            ),
        }
    }

    /// This shard as the modelled `NodeGroupConfiguration`: the shard
    /// configuration a snapshot of a replication group reports for it, which
    /// `DescribeSnapshots$ShowNodeGroupConfig` asks for ("if true, the node group
    /// (shard) configuration is included in the snapshot description").
    ///
    /// Every value is derived from the shard's own membership — its keyspace, how
    /// many replicas it holds and where they sit — rather than from the request
    /// that created the group, so a shard that has been scaled or failed over
    /// since reports what it holds NOW.
    pub fn to_configuration(&self) -> NodeGroupConfiguration {
        let replicas: Vec<&NodeGroupMemberRecord> = self
            .members
            .iter()
            .filter(|member| member.current_role != ROLE_PRIMARY)
            .collect();
        let replica_zones: Vec<String> = replicas
            .iter()
            .filter_map(|member| member.availability_zone.clone())
            .collect();
        NodeGroupConfiguration {
            node_group_id: Some(self.node_group_id.clone()),
            slots: self.slots.clone(),
            replica_count: i32::try_from(replicas.len()).ok(),
            primary_availability_zone: self
                .members
                .iter()
                .find(|member| member.current_role == ROLE_PRIMARY)
                .and_then(|member| member.availability_zone.clone()),
            replica_availability_zones: (!replica_zones.is_empty()).then_some(replica_zones),
            // Outposts are not modelled by this service: `CreateCacheCluster`
            // refuses every Outpost member, so no member here sits in one.
            primary_outpost_arn: None,
            replica_outpost_arns: None,
        }
    }

    /// The endpoint a WRITER of this shard connects to.
    ///
    /// It is the address of the member currently holding the primary role: where this
    /// emulator serves that member's data plane, the socket it answers on
    /// ([`CacheClusterRecord::served_endpoint`]), because a published endpoint has to be
    /// one a client can reach. A group this emulator serves no data plane for keeps the
    /// `master.` name ElastiCache's own zone would carry.
    pub(super) fn primary_endpoint(
        &self,
        group: &ReplicationGroupRecord,
        clusters: &BTreeMap<String, CacheClusterRecord>,
    ) -> Endpoint {
        self.members
            .iter()
            .find(|member| member.current_role == ROLE_PRIMARY)
            .and_then(|member| served_member_endpoint(member, clusters))
            .unwrap_or_else(|| Endpoint {
                address: Some(group.primary_endpoint_address()),
                port: Some(group.port),
            })
    }

    /// The endpoint a READER of this shard connects to: one of its replicas' sockets, by
    /// the same reading as [`Self::primary_endpoint`]. A shard with no replica reads at the
    /// member holding the PRIMARY role, which is where its reads are served from; where this
    /// emulator serves no socket for the member a reader would reach, the endpoint keeps the
    /// `replica.` name ElastiCache's own zone carries for a reader rather than the writer's.
    pub(super) fn reader_endpoint(
        &self,
        group: &ReplicationGroupRecord,
        clusters: &BTreeMap<String, CacheClusterRecord>,
    ) -> Endpoint {
        let reader = self
            .members
            .iter()
            .find(|member| member.current_role != ROLE_PRIMARY)
            .or_else(|| {
                self.members
                    .iter()
                    .find(|member| member.current_role == ROLE_PRIMARY)
            });
        reader
            .and_then(|member| served_member_endpoint(member, clusters))
            .unwrap_or_else(|| Endpoint {
                address: Some(group.reader_endpoint_address()),
                port: Some(group.port),
            })
    }
}

/// The address this emulator's own socket for the member cluster `member` answers on, or
/// `None` when it serves no data plane for it.
fn served_member_endpoint(
    member: &NodeGroupMemberRecord,
    clusters: &BTreeMap<String, CacheClusterRecord>,
) -> Option<Endpoint> {
    let served = clusters
        .get(&member.cache_cluster_id)?
        .served_endpoint
        .as_ref()?;
    Some(Endpoint {
        address: Some(served.host.clone()),
        port: Some(served.port),
    })
}

/// The changes a `ModifyReplicationGroup` deferred past this call: the members
/// the modelled `ReplicationGroupPendingModifiedValues` carries, which are
/// exactly the ones `ApplyImmediately` governs. A member that shape does NOT
/// model (the description, the maintenance window, the snapshot settings, the
/// node type) has nowhere to be reported as pending, so it is applied at once.
#[derive(Debug, Clone, Default)]
pub struct ReplicationGroupPendingValues {
    pub primary_cluster_id: Option<String>,
    /// One of `PendingAutomaticFailoverStatus`'s values (`enabled |
    /// disabled`).
    pub automatic_failover_status: Option<String>,
    /// One of `AuthTokenUpdateStatus`'s values (`SETTING | ROTATING`).
    pub auth_token_status: Option<String>,
    pub user_group_ids_to_add: Vec<String>,
    pub user_group_ids_to_remove: Vec<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub transit_encryption_enabled: Option<bool>,
    /// One of `TransitEncryptionMode`'s values (`preferred | required`).
    pub transit_encryption_mode: Option<String>,
    /// One of `ClusterMode`'s values (`enabled | disabled | compatible`).
    pub cluster_mode: Option<String>,
}

impl ReplicationGroupPendingValues {
    /// These values as the modelled shape.
    ///
    /// `ReplicationGroup$PendingModifiedValues` is a member ElastiCache always
    /// serializes, so a group with nothing deferred reports the EMPTY structure
    /// rather than no member at all; see [`PendingValues::to_modeled`].
    pub fn to_modeled(&self) -> ReplicationGroupPendingModifiedValues {
        ReplicationGroupPendingModifiedValues {
            primary_cluster_id: self.primary_cluster_id.clone(),
            automatic_failover_status: self.automatic_failover_status.clone(),
            // An online resharding is `ModifyReplicationGroupShardConfiguration`'s
            // to report, and that operation is a scaffolded 501 with its own
            // issue; nothing this operation defers is a resharding.
            resharding: None,
            auth_token_status: self.auth_token_status.clone(),
            user_groups: (!self.user_group_ids_to_add.is_empty()
                || !self.user_group_ids_to_remove.is_empty())
            .then(|| UserGroupsUpdateStatus {
                user_group_ids_to_add: (!self.user_group_ids_to_add.is_empty())
                    .then(|| self.user_group_ids_to_add.clone()),
                user_group_ids_to_remove: (!self.user_group_ids_to_remove.is_empty())
                    .then(|| self.user_group_ids_to_remove.clone()),
            }),
            log_delivery_configurations: (!self.log_delivery_configurations.is_empty()).then(|| {
                self.log_delivery_configurations
                    .iter()
                    .map(|record| PendingLogDeliveryConfiguration {
                        log_type: record.log_type.clone(),
                        destination_type: record.destination_type.clone(),
                        destination_details: record.destination_details.clone(),
                        log_format: record.log_format.clone(),
                    })
                    .collect()
            }),
            transit_encryption_enabled: self.transit_encryption_enabled,
            transit_encryption_mode: self.transit_encryption_mode.clone(),
            cluster_mode: self.cluster_mode.clone(),
        }
    }
}

/// One replication group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ReplicationGroupRecord {
    pub replication_group_id: String,
    pub description: String,
    pub region: String,
    pub arn: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    /// One of `ReplicationGroup$Status`'s values ("creating, available,
    /// modifying, deleting, create-failed, snapshotting").
    pub status: String,
    /// The status change the clock owes this group, if any.
    pub pending_status: Option<PendingTransition>,
    /// When the teardown a `DeleteReplicationGroup` began completes, after which
    /// the group stops resolving at all; see [`ReplicationGroupRecord::begin_deletion`].
    /// `None` for a group nothing has asked to delete.
    pub pending_removal: Option<SystemTime>,
    pub created_at: SystemTime,
    /// The DNS label every endpoint of this group carries, minted once and
    /// stored, so an endpoint is stable across reads.
    pub dns_token: String,
    pub port: i32,
    /// `ClusterEnabled` — "whether its data can be partitioned across multiple
    /// shards (API/CLI: node groups)".
    pub cluster_enabled: bool,
    /// One of `ClusterMode`'s values (`enabled | disabled | compatible`).
    pub cluster_mode: String,
    /// [`SETTING_ENABLED`] or [`SETTING_DISABLED`].
    pub automatic_failover: String,
    /// See [`ReplicationGroupRecord::automatic_failover`].
    pub multi_az: String,
    pub node_groups: Vec<NodeGroupRecord>,
    pub preferred_maintenance_window: String,
    pub cache_subnet_group_name: Option<String>,
    pub cache_parameter_group_name: String,
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    /// Whether an `AuthToken` was set; the token itself is a SECRET and is NOT
    /// stored. See [`CacheClusterRecord::auth_token_enabled`].
    pub auth_token_enabled: bool,
    pub auth_token_last_modified: Option<SystemTime>,
    /// The digests of the auth tokens that authenticate a client of this group;
    /// see [`crate::state::credential_digest`]. At most TWO: a rotation leaves the
    /// token being replaced live beside the new one, which is what makes a
    /// rotation not interrupt access, and a `SET` leaves only the new one. A
    /// second rotation replaces the older of the two rather than adding to them,
    /// so no number of calls grows this list.
    pub auth_token_digests: Vec<String>,
    pub transit_encryption_enabled: bool,
    pub transit_encryption_mode: Option<String>,
    pub at_rest_encryption_enabled: bool,
    pub kms_key_id: Option<String>,
    /// [`SETTING_ENABLED`] or [`SETTING_DISABLED`].
    pub data_tiering: String,
    /// Whether this group opts in to the next auto minor version upgrade
    /// campaign, and which network families it runs on. All three are RESOLVED
    /// when the group is created ([`DEFAULT_AUTO_MINOR_VERSION_UPGRADE`],
    /// [`DEFAULT_CACHE_NETWORK_TYPE`], [`DEFAULT_CACHE_IP_DISCOVERY`]), because
    /// ElastiCache populates each of them on every read of a group.
    pub auto_minor_version_upgrade: Option<bool>,
    /// See [`ReplicationGroupRecord::auto_minor_version_upgrade`].
    pub network_type: Option<String>,
    /// See [`ReplicationGroupRecord::auto_minor_version_upgrade`].
    pub ip_discovery: Option<String>,
    pub user_group_ids: Vec<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    /// One of `Durability`'s values (`default | async | sync | disabled`), as
    /// the request set it.
    pub durability: String,
    /// One of `EffectiveDurability`'s values (`async | sync | disabled`): what
    /// [`ReplicationGroupRecord::durability`] resolves to.
    pub effective_durability: String,
    /// The tags `CreateReplicationGroup` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    /// The change `ModifyReplicationGroup` deferred past the call, which is
    /// what `ReplicationGroup$PendingModifiedValues` reports; see
    /// [`ReplicationGroupPendingValues`].
    pub pending_modified_values: ReplicationGroupPendingValues,
    /// "The cluster ID that is used as the daily snapshot source for the
    /// replication group", as `ModifyReplicationGroup$SnapshottingClusterId`
    /// set it.
    pub snapshotting_cluster_id: Option<String>,
    /// The `AuthTokenUpdateStrategyType` the last accepted auth-token update
    /// used, which is what makes the documented `SET` step reachable: "SET -
    /// allowed only after ROTATE".
    pub auth_token_last_strategy: Option<String>,
    /// The open data migration INTO this group, which is what refuses a second
    /// start on it and what `CompleteMigration` finishes; see
    /// [`MigrationRecord`]. `ReplicationGroup` models no member for it, so it is
    /// state only. `None` until a migration is opened on it.
    pub migration: Option<MigrationRecord>,
    /// The shards of this group a `TestFailover` is still being carried out on,
    /// each with the instant that work falls due ([`TEST_FAILOVER_WORK`]): what
    /// refuses a second test of the same shard while the first one runs, with
    /// [`TEST_FAILOVER_IN_PROGRESS_MESSAGE`]. Keyed by `NodeGroupId`, so it
    /// holds at most one entry per shard the group has.
    pub test_failovers_in_flight: BTreeMap<String, SystemTime>,
    /// How many shard-reconfiguration calls on this group have been ADMITTED but
    /// have not answered yet: what tells a second such call that it collided with
    /// one being admitted alongside it, rather than that it found the group in a
    /// status it had already published (see
    /// [`ReplicationGroupRecord::reshard_admission_in_flight`]). `ReplicationGroup`
    /// models no member for it, so it is state only, and it is back to zero as soon
    /// as the call that raised it answers.
    pub reshard_admissions: usize,
    /// The Global datastore this group is a member of, and its role in it,
    /// which is what `ReplicationGroup$GlobalReplicationGroupInfo` reports.
    /// `CreateGlobalReplicationGroup` writes them for the primary it builds a
    /// datastore around, and the Global datastore operations that MOVE a
    /// membership (associate, disassociate, failover, delete) write them for
    /// every other member.
    pub global_replication_group_id: Option<String>,
    /// See [`ReplicationGroupRecord::global_replication_group_id`].
    pub global_member_role: Option<String>,
}

impl ReplicationGroupRecord {
    /// The primary endpoint address of a cluster mode disabled group: the
    /// `master.` name a writer connects to.
    pub(super) fn primary_endpoint_address(&self) -> String {
        format!(
            "master.{}.{}.{}.cache.amazonaws.com",
            self.replication_group_id, self.dns_token, self.region
        )
    }

    /// The reader endpoint address of a cluster mode disabled group: the
    /// `replica.` name a reader connects to.
    pub(super) fn reader_endpoint_address(&self) -> String {
        format!(
            "replica.{}.{}.{}.cache.amazonaws.com",
            self.replication_group_id, self.dns_token, self.region
        )
    }

    /// The configuration endpoint address of a cluster mode enabled group.
    ///
    /// `CacheCluster$ConfigurationEndpoint` documents the shape of the name —
    /// "The configuration endpoint will always have `.cfg` in it" — which the
    /// `clustercfg` label a sharded group's endpoint carries satisfies.
    pub(super) fn configuration_endpoint_address(&self) -> String {
        format!(
            "{}.{}.clustercfg.{}.cache.amazonaws.com",
            self.replication_group_id, self.dns_token, self.region
        )
    }

    /// What `StorageEncryptionType` reports: the kind of key the group's
    /// at-rest encryption uses, derived from whether it is enabled at all and
    /// whether the request named a KMS key.
    pub fn storage_encryption_type(&self) -> &'static str {
        if !self.at_rest_encryption_enabled {
            STORAGE_ENCRYPTION_NONE
        } else if self.kms_key_id.is_some() {
            STORAGE_ENCRYPTION_SSE_KMS
        } else {
            STORAGE_ENCRYPTION_SSE_ELASTICACHE
        }
    }

    /// "The names of all the cache clusters that are part of this replication
    /// group", derived from the shards rather than held a second time.
    pub fn member_clusters(&self) -> Vec<String> {
        let mut members: Vec<String> = self
            .node_groups
            .iter()
            .flat_map(|shard| shard.members.iter())
            .map(|member| member.cache_cluster_id.clone())
            .collect();
        members.sort();
        members.dedup();
        members
    }

    /// Materialise the status change the clock owes this group, if `now` is
    /// past its due instant, and hand back the change it made. See
    /// [`CacheClusterRecord::settle`]: this is the one write a reporting
    /// operation may make.
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        for shard in &mut self.node_groups {
            shard.status = self.status.clone();
        }
        Some(settled)
    }

    /// Whether a `TestFailover` already accepted on the shard `node_group_id`
    /// is still being carried out at `now` — the per-shard conflict
    /// [`TEST_FAILOVER_IN_PROGRESS_MESSAGE`] refuses a second test with.
    pub fn test_failover_in_flight(&self, node_group_id: &str, now: SystemTime) -> bool {
        self.test_failovers_in_flight
            .get(node_group_id)
            .is_some_and(|due| now < *due)
    }

    /// Whether a shard reconfiguration of this group has been admitted and has
    /// not answered yet: the conflict between two such calls being admitted at
    /// once, which ElastiCache tells the one that lost to try again
    /// ([`RESHARD_COLLISION_MESSAGE`]) rather than reporting the status the
    /// winner has not published to anybody yet.
    pub fn reshard_admission_in_flight(&self) -> bool {
        self.reshard_admissions > 0
    }

    /// Record that a shard reconfiguration of this group has been admitted, and
    /// has not answered yet; see
    /// [`ReplicationGroupRecord::reshard_admission_in_flight`].
    pub fn begin_reshard_admission(&mut self) {
        self.reshard_admissions = self.reshard_admissions.saturating_add(1);
    }

    /// Record that an admitted shard reconfiguration of this group is answering,
    /// so the status it left the group in is one every later caller reads as
    /// published; the other half of
    /// [`ReplicationGroupRecord::begin_reshard_admission`].
    pub fn end_reshard_admission(&mut self) {
        self.reshard_admissions = self.reshard_admissions.saturating_sub(1);
    }

    /// Record that a test of automatic failover on the shard `node_group_id` is
    /// being carried out from `now`, over [`TEST_FAILOVER_WORK`].
    ///
    /// The tests the clock has already carried through are dropped as this one
    /// is recorded: such a test stops running at a fixed instant whether or not
    /// anybody calls again, so nothing is kept for a shard that is idle.
    pub fn begin_test_failover(&mut self, node_group_id: &str, now: SystemTime) {
        self.test_failovers_in_flight.retain(|_, due| now < *due);
        self.test_failovers_in_flight
            .insert(node_group_id.to_string(), now + TEST_FAILOVER_WORK);
    }

    /// How many replicas the shard `node_group_id` has: every member of it that
    /// is not the primary.
    pub fn shard_replica_count(&self, node_group_id: &str) -> Option<i32> {
        let shard = self
            .node_groups
            .iter()
            .find(|shard| shard.node_group_id == node_group_id)?;
        Some(
            i32::try_from(
                shard
                    .members
                    .iter()
                    .filter(|member| member.current_role != ROLE_PRIMARY)
                    .count(),
            )
            .unwrap_or(0),
        )
    }

    /// Whether `cache_cluster_id` is a REPLICA of the shard `node_group_id`:
    /// what `ReplicasToRemove` has to name.
    pub fn is_shard_replica(&self, node_group_id: &str, cache_cluster_id: &str) -> bool {
        self.node_groups
            .iter()
            .filter(|shard| shard.node_group_id == node_group_id)
            .flat_map(|shard| shard.members.iter())
            .any(|member| {
                member.current_role != ROLE_PRIMARY
                    && member.cache_cluster_id == cache_cluster_id
            })
    }

    /// Whether `cache_cluster_id` is the PRIMARY of the shard it belongs to:
    /// the member `DeleteCacheCluster` cannot delete, because the group is what
    /// owns it.
    pub fn is_shard_primary(&self, cache_cluster_id: &str) -> bool {
        self.node_groups
            .iter()
            .flat_map(|shard| shard.members.iter())
            .any(|member| {
                member.current_role == ROLE_PRIMARY
                    && member.cache_cluster_id == cache_cluster_id
            })
    }

    /// Whether `cache_cluster_id` is a member cluster of this group — what
    /// `PrimaryClusterId` and `SnapshottingClusterId` have to name.
    pub fn is_member_cluster(&self, cache_cluster_id: &str) -> bool {
        self.node_groups
            .iter()
            .flat_map(|shard| shard.members.iter())
            .any(|member| member.cache_cluster_id == cache_cluster_id)
    }

    /// Promote the member cluster `cache_cluster_id` to the primary role of
    /// the shard it belongs to, demoting that shard's current primary — which
    /// is what `ModifyReplicationGroup$PrimaryClusterId` asks for. Hands back
    /// `(demoted cluster, promoted cluster)`, or `None` when no shard of this
    /// group holds it as a replica (a cluster that is already the primary is
    /// such a case: there is nothing to move).
    pub fn promote_primary(&mut self, cache_cluster_id: &str) -> Option<(String, String)> {
        self.node_groups
            .iter_mut()
            .find_map(|shard| shard.promote(cache_cluster_id))
    }

    /// Start the span after which the clock owes this group `available`, leaving
    /// it and its shards in `status` until then — the same transition a create
    /// gets, so the call that starts the work answers with the transient status.
    pub fn begin_transition(&mut self, status: &str, now: SystemTime) {
        self.begin_transition_over(status, now, CLUSTER_TRANSITION);
    }

    /// [`Self::begin_transition`] over a span of its caller's choosing: for work
    /// this service carries out one of at a time, whose window a second call is
    /// refused in has to out-last the call that started it (see
    /// [`RESHARD_WORK`]).
    pub fn begin_transition_over(&mut self, status: &str, now: SystemTime, span: Duration) {
        self.status = status.to_string();
        self.pending_status = Some(PendingTransition {
            due: now + span,
            status: STATUS_AVAILABLE.to_string(),
        });
        for shard in &mut self.node_groups {
            shard.status = status.to_string();
        }
    }

    /// Put this group and its shards into [`STATUS_DELETING`], and start the span
    /// after which the teardown is complete and the group stops resolving.
    ///
    /// `DeleteReplicationGroup` documents exactly this: "when you receive a
    /// successful response from this operation, Amazon ElastiCache immediately
    /// begins deleting the selected resources; you cannot cancel or revert this
    /// operation" — a delete that is only BEGUN, so `Status` is one of the states
    /// the shape documents ("creating, available, modifying, deleting, …") and
    /// every reader still reports the group in it until the removal the clock owes
    /// falls due ([`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_replication_groups`]).
    /// Any status change that was still pending is dropped: a group that is going
    /// away is not owed `available` any more.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = STATUS_DELETING.to_string();
        self.pending_status = None;
        self.pending_removal = Some(now + REPLICATION_GROUP_TEARDOWN);
        for shard in &mut self.node_groups {
            shard.status = STATUS_DELETING.to_string();
        }
    }

    /// This record as the modelled `ReplicationGroup`.
    ///
    /// `clusters` is the store's cluster map, which the members' read endpoints
    /// are read out of: a member cluster's endpoint is held once, on the cluster.
    pub fn to_modeled(
        &self,
        clusters: &BTreeMap<String, CacheClusterRecord>,
    ) -> ReplicationGroup {
        ReplicationGroup {
            replication_group_id: Some(self.replication_group_id.clone()),
            description: Some(self.description.clone()),
            // The Global datastore this group is a member of, if the Global
            // datastore operations have made it one; see
            // [`ReplicationGroupRecord::global_replication_group_id`]. The
            // structure is always serialized, so a group in no datastore reports
            // it EMPTY rather than withholding the member.
            global_replication_group_info: Some(GlobalReplicationGroupInfo {
                global_replication_group_id: self.global_replication_group_id.clone(),
                global_replication_group_member_role: self.global_member_role.clone(),
            }),
            status: Some(self.status.clone()),
            pending_modified_values: Some(self.pending_modified_values.to_modeled()),
            member_clusters: Some(self.member_clusters()),
            node_groups: Some(
                self.node_groups
                    .iter()
                    .map(|shard| shard.to_modeled(self, clusters))
                    .collect(),
            ),
            // The daily snapshot source, as `ModifyReplicationGroup` set it.
            snapshotting_cluster_id: self.snapshotting_cluster_id.clone(),
            automatic_failover: Some(self.automatic_failover.clone()),
            multi_az: Some(self.multi_az.clone()),
            // "Use the configuration endpoint to connect to this replication
            // group": the sharded group's one endpoint. A cluster mode disabled
            // group is reached through its shard's primary and reader
            // endpoints instead.
            configuration_endpoint: self.cluster_enabled.then(|| {
                // The sharded group's clients reach it at the socket of a
                // primary, where this emulator serves one; see
                // [`NodeGroupRecord::primary_endpoint`].
                self.node_groups
                    .first()
                    .map(|shard| shard.primary_endpoint(self, clusters))
                    .unwrap_or_else(|| Endpoint {
                        address: Some(self.configuration_endpoint_address()),
                        port: Some(self.port),
                    })
            }),
            snapshot_retention_limit: self.snapshot_retention_limit,
            snapshot_window: self.snapshot_window.clone(),
            cluster_enabled: Some(self.cluster_enabled),
            cache_node_type: self.cache_node_type.clone(),
            auth_token_enabled: Some(self.auth_token_enabled),
            auth_token_last_modified_date: self
                .auth_token_last_modified
                .map(::aws_smithy_types::DateTime::from),
            transit_encryption_enabled: Some(self.transit_encryption_enabled),
            at_rest_encryption_enabled: Some(self.at_rest_encryption_enabled),
            // Outposts are not modelled by this service.
            member_clusters_outpost_arns: None,
            kms_key_id: self.kms_key_id.clone(),
            storage_encryption_type: Some(self.storage_encryption_type().to_string()),
            arn: Some(self.arn.clone()),
            user_group_ids: (!self.user_group_ids.is_empty())
                .then(|| self.user_group_ids.clone()),
            // "Returns the destination, format and type of the logs": always
            // serialized, so a group delivering none reports the EMPTY list.
            log_delivery_configurations: Some(
                self.log_delivery_configurations
                    .iter()
                    .map(LogDeliveryRecord::to_modeled)
                    .collect(),
            ),
            replication_group_create_time: Some(::aws_smithy_types::DateTime::from(
                self.created_at,
            )),
            data_tiering: Some(self.data_tiering.clone()),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            network_type: self.network_type.clone(),
            ip_discovery: self.ip_discovery.clone(),
            transit_encryption_mode: self.transit_encryption_mode.clone(),
            cluster_mode: Some(self.cluster_mode.clone()),
            engine: Some(self.engine.clone()),
            durability: Some(self.durability.clone()),
            effective_durability: Some(self.effective_durability.clone()),
        }
    }
}

/// One member cluster a new replication group is to have: the cluster id, the
/// zone it is placed in, and whether it is an EXISTING cluster being adopted as
/// the group's primary (the `PrimaryClusterId` path) rather than minted.
#[derive(Debug, Clone)]
pub struct NewMember {
    pub cache_cluster_id: String,
    pub availability_zone: Option<String>,
    pub adopt: bool,
}

/// One node group (shard) a new replication group is to have.
#[derive(Debug, Clone)]
pub struct NewShard {
    pub node_group_id: String,
    pub slots: Option<String>,
    /// The shard's members, the primary first.
    pub members: Vec<NewMember>,
}

/// The validated values a new replication group is built from — one argument so
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::insert_replication_group`]
/// does not take three dozen positional ones.
#[derive(Debug, Clone)]
pub struct NewReplicationGroup {
    pub replication_group_id: String,
    pub description: String,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    pub port: i32,
    pub cluster_enabled: bool,
    pub cluster_mode: String,
    pub automatic_failover: bool,
    pub multi_az: bool,
    pub shards: Vec<NewShard>,
    pub preferred_maintenance_window: String,
    pub cache_subnet_group_name: Option<String>,
    pub cache_parameter_group_name: String,
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    pub auth_token_enabled: bool,
    /// The digests of the auth tokens a client of this group authenticates with;
    /// see [`ReplicationGroupRecord::auth_token_digests`].
    pub auth_token_digests: Vec<String>,
    pub transit_encryption_enabled: bool,
    pub transit_encryption_mode: Option<String>,
    pub at_rest_encryption_enabled: bool,
    pub kms_key_id: Option<String>,
    pub data_tiering: bool,
    pub auto_minor_version_upgrade: Option<bool>,
    pub security_group_ids: Vec<String>,
    pub notification_topic_arn: Option<String>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub user_group_ids: Vec<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub durability: String,
    pub effective_durability: String,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}
