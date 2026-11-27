// SPDX-License-Identifier: CC0-1.0

//! Changing how many node groups (shards) a replication group has.
//!
//! `ModifyReplicationGroupShardConfiguration` "modifies a replication group's shards
//! (node groups) by allowing you to add shards, remove shards, or rebalance the keyspaces
//! among existing shards", and all three are ONE write over the group's stored shards
//! ([`ReplicationGroupRecord::node_groups`]): the shards the request retains stay, the
//! ones it removes go with their member CLUSTERS, the ones it adds are minted with member
//! clusters of their own, and the keyspace is then partitioned across whatever shards the
//! group ends up with.
//!
//! A shard's member is a cache CLUSTER (which is why
//! [`ComAmazonawsElasticacheAmazonelasticachev9State::insert_replication_group`] puts
//! every member in [`ComAmazonawsElasticacheAmazonelasticachev9State::cache_clusters`]),
//! so a reshard creates and removes those clusters too — otherwise
//! `DescribeCacheClusters` and `DescribeReplicationGroups` would disagree about what the
//! account runs. That is the same reason
//! [`ComAmazonawsElasticacheAmazonelasticachev9State::set_shard_replica_count`] mints and
//! drops clusters when a shard's replica count changes; this slice is its shard-count
//! counterpart.

use super::*;

/// How long a shard reconfiguration of a replication group stays under way
/// before the clock owes the group `available` again.
///
/// Unlike [`CLUSTER_TRANSITION`], which is the span of the CALL that started the
/// work, this one out-lasts the call on purpose: ElastiCache carries out one
/// shard reconfiguration of a group at a time and refuses a second while one is
/// under way, and that refusal is observable only while the group is still
/// `modifying` — which is what makes two reshards released together resolve to
/// exactly one winner. It is the window that exclusion needs, and nothing more:
/// a caller polling the group reaches `available` from the read that follows it.
pub const RESHARD_WORK: Duration = Duration::from_millis(500);

/// How ElastiCache refuses a shard-configuration change on a replication group
/// whose keyspace is not partitioned: the operation configures the shards of a
/// cluster mode ENABLED group, and a group that is not one has a single keyspace
/// in a single shard, so the whole sentence is fixed text naming neither the
/// group nor the count that was asked for.
pub const SHARDS_NEED_CLUSTER_MODE_MESSAGE: &str =
    "Operation is only applicable for cluster mode enabled replication groups.";

/// How ElastiCache refuses a shard-configuration change that COLLIDED with
/// another one being admitted at the same time: the sentence states that the
/// reshard could not be carried out and that it is worth retrying, which is what
/// a caller whose reshard lost such a race acts on.
///
/// This is the answer only while the reshard that won has not answered its own
/// caller yet, so the status it left the group in has been published to nobody:
/// there is no state for the loser to be told to wait for, only the collision.
/// Once that status IS published, a later change to the group is refused on the
/// status itself, with the one sentence this service words that gate in
/// ([`replication_group_modify_state_refusal`]). Wholly fixed text: neither the
/// group nor the status it is in appears in it.
pub const RESHARD_COLLISION_MESSAGE: &str =
    "Could not reshard replication group. Please try again later.";

/// The admission of ONE shard-reconfiguration call on one replication group, held
/// for as long as that call has not answered.
///
/// Raising it is what tells a reshard arriving alongside this one that it collided
/// with a call whose status change has been published to nobody
/// ([`RESHARD_COLLISION_MESSAGE`]), and dropping it is what makes the group's
/// status one every later caller is refused on
/// ([`replication_group_modify_state_refusal`]). It is a GUARD rather than a pair
/// of calls so that the admission cannot outlive its call: a handler whose future
/// is dropped part-way — a caller that went away — gives it back here, instead of
/// leaving the group answering the collision sentence for good.
pub struct ReshardAdmission {
    state: ::engine::ServiceState<ComAmazonawsElasticacheAmazonelasticachev9State>,
    replication_group_id: String,
}

impl ReshardAdmission {
    /// Raise the admission on the stored group `replication_group_id`, through the
    /// store the caller already holds, and hand back the guard that gives it back.
    ///
    /// `state` is the handle the guard reaches the store through when it is
    /// dropped, which is why it is taken beside the `store` the caller holds: the
    /// store's guard cannot be locked a second time from here.
    pub fn raise(
        state: &::engine::ServiceState<ComAmazonawsElasticacheAmazonelasticachev9State>,
        store: &mut ComAmazonawsElasticacheAmazonelasticachev9State,
        replication_group_id: &str,
    ) -> Self {
        if let Some(group) = store.replication_groups.get_mut(replication_group_id) {
            group.begin_reshard_admission();
        }
        Self {
            state: state.clone(),
            replication_group_id: replication_group_id.to_string(),
        }
    }
}

impl Drop for ReshardAdmission {
    fn drop(&mut self) {
        if let Some(group) = self
            .state
            .lock()
            .replication_groups
            .get_mut(&self.replication_group_id)
        {
            group.end_reshard_admission();
        }
    }
}

/// How ElastiCache words a node group (shard) the replication group a request
/// named does not hold: `Node group <node-group-id> was not found for replication
/// group <replication-group-id>` — no trailing stop.
///
/// The message of the modelled `NodeGroupNotFoundFault` ("the node group specified
/// by the NodeGroupId parameter could not be found"), so one helper words every
/// surface that answers it. Both variable parts are taken from the request: the
/// node group it asked about and the group it asked about it in, each as the
/// request spelled it.
pub fn node_group_not_found_message(node_group_id: &str, replication_group_id: &str) -> String {
    format!("Node group {node_group_id} was not found for replication group {replication_group_id}")
}

/// How many replicas a shard a reshard ADDS gets when the group it joins has no shard to
/// take a count from: none, the primary alone.
///
/// A new shard is built to match the group it joins — `ReshardingConfiguration` names the
/// zones of the new shards but no replica count, so the count comes from the group's own
/// shards — and a group with no shard left to read it off is one that retains nothing,
/// which the caller cannot ask for (`NodeGroupCount` is at least 1 and a scale-in retains
/// that many).
const REPLICAS_WITHOUT_A_SHARD_TO_COPY: usize = 0;

/// One node group (shard) a reshard adds: the id it is to carry and the zones its member
/// clusters are to be placed in, as `ReshardingConfiguration$PreferredAvailabilityZones`
/// named them ("the preferred availability zones for each node group in the cluster"), or
/// empty for a shard whose zones the service picks.
#[derive(Debug, Clone)]
pub struct NewNodeGroup {
    pub node_group_id: String,
    pub preferred_availability_zones: Vec<String>,
}

impl ComAmazonawsElasticacheAmazonelasticachev9State {
    /// The node group ids the replication group `group_id` holds, in the order it holds
    /// them — what a reshard's `NodeGroupsToRemove` / `NodeGroupsToRetain` are resolved
    /// against, and what the ids of the shards it adds are minted after.
    pub fn node_group_ids(&self, group_id: &str) -> Vec<String> {
        self.replication_groups
            .get(group_id)
            .map(|group| {
                group
                    .node_groups
                    .iter()
                    .map(|shard| shard.node_group_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The id a shard added to the replication group `group_id` carries, for the
    /// `offset`-th shard being added in one reshard (0 for the first).
    ///
    /// "A Valkey or Redis OSS (cluster mode disabled) replication group contains only 1
    /// node group; therefore, the node group ID is 0001" — the ids are that four-digit
    /// form, counted up from the highest the group already holds, so an id is never reused
    /// by a later reshard of the same group while the shard it named is still there.
    pub fn next_node_group_id(&self, group_id: &str, offset: usize) -> String {
        let highest = self
            .node_group_ids(group_id)
            .iter()
            .filter_map(|id| id.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        let ordinal = highest
            .saturating_add(1)
            .saturating_add(u32::try_from(offset).unwrap_or(0));
        format!("{ordinal:04}")
    }

    /// Reconfigure the shards of the replication group `group_id`: keep the shards
    /// `retained` names (in the order the group holds them), remove every other shard
    /// together with its member clusters, mint `added` with member clusters of their own,
    /// and partition the keyspace across the shards the group then has.
    ///
    /// The group is left `modifying` with the clock owing it `available`
    /// ([`ReplicationGroupRecord::begin_transition_over`]) for [`RESHARD_WORK`], because
    /// resharding is work ElastiCache carries out after the call returns and carries out
    /// one of at a time — a caller that asks for a second reshard while one is under way
    /// still sees the transient status and the state refusal that goes with it, which is
    /// what makes two concurrent reshards resolve to exactly one winner.
    pub fn reshard_replication_group(
        &mut self,
        group_id: &str,
        retained: &[String],
        added: &[NewNodeGroup],
        now: SystemTime,
    ) {
        let Some(group) = self.replication_groups.get(group_id) else {
            return;
        };
        // Decided before anything is written: which shards stay, which go, and which
        // member clusters leave with the ones that go.
        let mut keep: Vec<NodeGroupRecord> = Vec::new();
        let mut drop_clusters: Vec<String> = Vec::new();
        for shard in &group.node_groups {
            if retained.contains(&shard.node_group_id) {
                keep.push(shard.clone());
            } else {
                drop_clusters.extend(
                    shard
                        .members
                        .iter()
                        .map(|member| member.cache_cluster_id.clone()),
                );
            }
        }
        // A shard the reshard adds is built to match the group it joins: as many members
        // as the group's own shards carry, running what the group runs.
        let members_per_shard = keep
            .first()
            .map(|shard| shard.members.len())
            .unwrap_or(REPLICAS_WITHOUT_A_SHARD_TO_COPY + 1)
            .max(1);
        let template = group.clone();

        for cluster_id in &drop_clusters {
            self.cache_clusters.remove(cluster_id);
        }

        let mut minted: Vec<NodeGroupRecord> = Vec::with_capacity(added.len());
        for shard in added {
            let mut members = Vec::with_capacity(members_per_shard);
            for ordinal in 1..=members_per_shard {
                let cache_cluster_id = member_cluster_id(
                    &template.replication_group_id,
                    &shard.node_group_id,
                    ordinal,
                    // Only a cluster mode ENABLED group has its shards reconfigured, and
                    // its member cluster ids name the shard they belong to.
                    true,
                );
                // The zone the request asked for this member, or the one the service
                // picks when it asked for none ("If you omit this parameter ElastiCache
                // selects availability zones for you").
                let zone = shard
                    .preferred_availability_zones
                    .get(ordinal - 1)
                    .cloned()
                    .unwrap_or_else(|| availability_zone(&template.region, ordinal - 1));
                let _ = self.insert_cluster(NewCluster {
                    cache_cluster_id: cache_cluster_id.clone(),
                    region: template.region.clone(),
                    engine: template.engine.clone(),
                    engine_version: template.engine_version.clone(),
                    cache_node_type: template.cache_node_type.clone(),
                    // "For clusters running Valkey or Redis OSS, this value must be 1": a
                    // shard's replicas are further member CLUSTERS.
                    num_cache_nodes: REDIS_NUM_CACHE_NODES,
                    port: template.port,
                    availability_zones: vec![zone.clone()],
                    az_mode: None,
                    preferred_maintenance_window: template.preferred_maintenance_window.clone(),
                    cache_subnet_group_name: template.cache_subnet_group_name.clone(),
                    cache_parameter_group_name: template.cache_parameter_group_name.clone(),
                    snapshot_retention_limit: template.snapshot_retention_limit,
                    snapshot_window: template.snapshot_window.clone(),
                    auto_minor_version_upgrade: template.auto_minor_version_upgrade,
                    security_group_ids: Vec::new(),
                    notification_topic_arn: None,
                    auth_token_enabled: template.auth_token_enabled,
                    // A member minted into an existing group authenticates its clients
                    // exactly as the group does.
                    auth_token_digests: template.auth_token_digests.clone(),
                    transit_encryption_enabled: Some(template.transit_encryption_enabled),
                    transit_encryption_mode: template.transit_encryption_mode.clone(),
                    at_rest_encryption_enabled: Some(template.at_rest_encryption_enabled),
                    network_type: template.network_type.clone(),
                    ip_discovery: template.ip_discovery.clone(),
                    log_delivery_configurations: template.log_delivery_configurations.clone(),
                    tags: template.tags.clone(),
                    now,
                    replication_group_id: Some(template.replication_group_id.clone()),
                    replication_group_log_delivery_enabled: Some(
                        !template.log_delivery_configurations.is_empty(),
                    ),
                });
                members.push(NodeGroupMemberRecord {
                    cache_cluster_id,
                    cache_node_id: FIRST_CACHE_NODE_ID.to_string(),
                    availability_zone: Some(zone),
                    // The first member of a shard is its primary; see
                    // [`NodeGroupRecord::members`].
                    current_role: if ordinal == 1 { ROLE_PRIMARY } else { ROLE_REPLICA }
                        .to_string(),
                });
            }
            minted.push(NodeGroupRecord {
                node_group_id: shard.node_group_id.clone(),
                status: STATUS_MODIFYING.to_string(),
                // Filled in below, once the shard count is final: a shard's keyspace is a
                // share of the group's.
                slots: None,
                members,
            });
        }

        let Some(group) = self.replication_groups.get_mut(group_id) else {
            return;
        };
        keep.extend(minted);
        group.node_groups = keep;
        // "Rebalance the keyspaces among existing shards": the keyspace is partitioned
        // across the shards the group NOW has, exactly once and with no gap, which is the
        // one statement of that partition ([`default_slots`]) — the same one a create
        // applies.
        let shards = i32::try_from(group.node_groups.len()).unwrap_or(1);
        for (index, shard) in group.node_groups.iter_mut().enumerate() {
            shard.slots = Some(default_slots(shards, i32::try_from(index).unwrap_or(0)));
        }
        group.begin_transition_over(STATUS_MODIFYING, now, RESHARD_WORK);
    }
}
