// SPDX-License-Identifier: CC0-1.0

//! The registry's own operations: every lookup, insertion, removal and page walk
//! over the maps [`ComAmazonawsKafkaKafkaState`](super::ComAmazonawsKafkaKafkaState)
//! holds, including the settling lookups that materialise what the clock already
//! owed ONE record.
//!
//! Split out of the parent state module, which keeps to the RECORDS and the
//! service's fixed values; the code here is unchanged.

use super::*;

impl ComAmazonawsKafkaKafkaState {
    /// The ARN registered for `name` in `region`, if any. This is the lookup that
    /// makes a repeated `CreateCluster` a conflict.
    pub fn arn_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.cluster_arn_by_name
            .get(&(region.to_string(), name.to_string()))
    }

    /// How many clusters `region` currently holds — what the per-region quota is
    /// checked against. O(1): the count is maintained as clusters come and go.
    pub fn cluster_count_in(&self, region: &str) -> usize {
        self.clusters_per_region.get(region).copied().unwrap_or(0)
    }

    /// Add `cluster` to every index that must know about it. The one insertion
    /// path, so the ARN registry, the (region, name) index and the per-region
    /// count cannot drift apart — and the one place the registry's ceilings are
    /// enforced, so they hold for every caller of this module rather than only
    /// for a handler that remembers to check first.
    ///
    /// Refuses, changing nothing, when the region is at
    /// [`MAX_CLUSTERS_PER_REGION`], when the whole registry is at
    /// [`MAX_CLUSTERS_IN_REGISTRY`] (the backstop that bounds growth however many
    /// region names a caller invents, since the region comes from the request),
    /// or when the ARN is already registered.
    pub fn register_cluster(&mut self, cluster: Cluster) -> Result<(), RegisterRejected> {
        if self.clusters.contains_key(&cluster.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        if self.clusters.len() >= MAX_CLUSTERS_IN_REGISTRY {
            return Err(RegisterRejected::RegistryCeiling);
        }
        if self.cluster_count_in(&cluster.region) >= MAX_CLUSTERS_PER_REGION {
            return Err(RegisterRejected::RegionQuota);
        }
        *self
            .clusters_per_region
            .entry(cluster.region.clone())
            .or_insert(0) += 1;
        self.cluster_arn_by_name.insert(
            (cluster.region.clone(), cluster.name.clone()),
            cluster.arn.clone(),
        );
        self.clusters.insert(cluster.arn.clone(), cluster);
        Ok(())
    }

    /// Remove the cluster `arn` names from every index, handing it back. The one
    /// removal path, for the same reason.
    ///
    /// Its operation history goes with it: an operation names the cluster it ran
    /// on, so keeping records of a cluster nobody can reach would grow the store
    /// without bound.
    ///
    /// A topic belongs to the cluster whose brokers hold it, so the cluster's
    /// topics leave with it: nothing in the registry can outlive its cluster.
    ///
    /// A channel reads from a cluster, so a cluster that is gone holds no
    /// channels: every channel registered under `arn` is dropped here too, which
    /// is why this is also the one place that can orphan a channel — and does
    /// not.
    pub fn unregister_cluster(&mut self, arn: &str) -> Option<Cluster> {
        let cluster = self.clusters.remove(arn)?;
        self.cluster_arn_by_name
            .remove(&(cluster.region.clone(), cluster.name.clone()));
        for operation_arn in &cluster.operation_arns {
            self.cluster_operations.remove(operation_arn);
        }
        self.topics.retain(|(topic_cluster, _), _| topic_cluster != arn);
        if let Some(count) = self.clusters_per_region.get_mut(&cluster.region) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.clusters_per_region.remove(&cluster.region);
            }
        }
        for channel_arn in self
            .channel_arns_by_cluster
            .remove(arn)
            .unwrap_or_default()
        {
            if let Some(channel) = self.channels.remove(&channel_arn) {
                self.channel_arn_by_name
                    .remove(&(channel.cluster_arn, channel.name));
            }
        }
        // The record of this cluster's removed channels goes with it: a channel
        // ARN is only ever asked about under its own cluster, and this one is no
        // longer in the registry to ask about.
        self.removed_channel_arns.remove(arn);
        Some(cluster)
    }

    /// The cluster `arn` names, as an endpoint for `region` sees it.
    ///
    /// A regional endpoint serves only its own region's clusters, so a
    /// well-formed ARN belonging to another region resolves to `None` exactly as
    /// an absent one does. Every reader resolves a cluster through this or
    /// [`Self::settled_cluster_in`], so none of them can disagree about which
    /// clusters this endpoint holds.
    pub fn cluster_in(&self, region: &str, arn: &str) -> Option<&Cluster> {
        self.clusters
            .get(arn)
            .filter(|cluster| cluster.region == region)
    }

    /// The same lookup, with the transitions the clock already owed THIS cluster
    /// materialised (`CREATING` -> `ACTIVE`, and an in-flight update whose
    /// completion is due).
    ///
    /// Scoped to the one cluster being reported: a read must not do work
    /// proportional to the whole registry, and no other cluster's status is this
    /// request's business.
    pub fn settled_cluster_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&Cluster> {
        self.settled_cluster_in_mut(region, arn, now)
            .map(|cluster| &*cluster)
    }

    /// The same settled lookup, handed back for update — what an operation that
    /// changes the cluster resolves it through, so a mutating call and a
    /// reporting one agree about which clusters this endpoint holds and about
    /// what the clock already owed them.
    pub fn settled_cluster_in_mut(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut Cluster> {
        let cluster = self.clusters.get_mut(arn)?;
        // A regional endpoint serves only its own region's clusters, so this
        // resolves a cluster of another region to `None` exactly as an absent one.
        if cluster.region != region {
            return None;
        }
        cluster.settle(now);
        Some(cluster)
    }

    /// The ARN registered for the configuration `name` in `region`, if any. This
    /// is the lookup that makes a repeated `CreateConfiguration` a conflict.
    ///
    /// A configuration whose removal the clock already owes holds its name no
    /// longer, so the name is free again even before anything pruned the record;
    /// one that is `DELETING` with its removal still ahead DOES still hold it,
    /// because it is still a configuration of this region.
    pub fn configuration_arn_for_name(
        &self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&String> {
        let arn = self
            .configuration_arn_by_name
            .get(&(region.to_string(), name.to_string()))?;
        self.configurations
            .get(arn)
            .filter(|configuration| configuration.is_live(now))
            .map(|_| arn)
    }

    /// How many configurations `region` currently holds — what the per-region
    /// quota is checked against. O(1): the count is maintained as they come and
    /// go.
    pub fn configuration_count_in(&self, region: &str) -> usize {
        self.configurations_per_region
            .get(region)
            .copied()
            .unwrap_or(0)
    }

    /// The configuration `arn` names, as an endpoint for `region` sees it at
    /// `now`.
    ///
    /// A regional endpoint serves only its own region's configurations, so a
    /// well-formed ARN belonging to another region resolves to `None` exactly as
    /// an absent one does — the same rule [`Self::cluster_in`] follows, so the
    /// readers and `ListConfigurations` agree on which configurations this
    /// endpoint holds. A configuration whose removal the clock already owes is
    /// gone for the same reason, whether or not the record has been pruned yet;
    /// a `DELETING` one whose removal is still ahead resolves, since the
    /// asynchronous delete has not taken it yet.
    pub fn configuration_in(
        &self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&ConfigurationRecord> {
        self.configurations
            .get(arn)
            .filter(|configuration| configuration.region == region && configuration.is_live(now))
    }

    /// The same lookup for a call that WRITES, with the removal the clock
    /// already owed THIS configuration materialised: a configuration whose
    /// removal is due leaves the registry here (so this answers `None` and its
    /// name is free again), and any other resolves mutably.
    ///
    /// Scoped to the one configuration being addressed, for the same reason
    /// [`Self::settled_cluster_in`] is: no other configuration's removal is this
    /// request's business.
    pub fn settled_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut ConfigurationRecord> {
        let configuration = self.configurations.get(arn)?;
        if configuration.region != region {
            return None;
        }
        if !configuration.is_live(now) {
            self.unregister_configuration(region, arn);
            return None;
        }
        self.configurations.get_mut(arn)
    }

    /// Accept the deletion of the configuration `arn` names, as an endpoint for
    /// `region` sees it, handing back the record as the caller must now see it:
    /// `DELETING`, with the instant the clock owes its removal set.
    ///
    /// Answers `None` when this region holds no such configuration — including
    /// one whose removal the clock already owed, which the settling lookup
    /// materialises first, so a configuration is never deleted twice. Idempotent
    /// through [`ConfigurationRecord::begin_deletion`].
    pub fn begin_configuration_deletion(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&ConfigurationRecord> {
        let configuration = self.settled_configuration_in_mut(region, arn, now)?;
        configuration.begin_deletion(now);
        Some(configuration)
    }

    /// Drop the configurations of `region` whose removal the clock owed by
    /// `now`.
    ///
    /// Every one of them was due at that same instant whether or not anybody
    /// addressed it, so this materialises nothing a scoped lookup would not:
    /// it is what lets the registry's ceilings bound the records actually held,
    /// rather than letting a caller looping create-and-delete under fresh names
    /// grow it past them.
    pub(super) fn prune_removed_configurations(&mut self, region: &str, now: SystemTime) {
        let removed: Vec<String> = self
            .configurations
            .values()
            .filter(|configuration| configuration.region == region && !configuration.is_live(now))
            .map(|configuration| configuration.arn.clone())
            .collect();
        for arn in removed {
            self.unregister_configuration(region, &arn);
        }
    }

    /// Add `configuration` to every index that must know about it: the one
    /// insertion path, so the ARN registry, the (region, name) index and the
    /// per-region count cannot drift apart, and the one place the registry's
    /// ceilings are enforced.
    ///
    /// Refuses, changing nothing, when the region is at
    /// [`MAX_CONFIGURATIONS_PER_REGION`], when the whole registry is at
    /// [`MAX_CONFIGURATIONS_IN_REGISTRY`], or when the ARN is already registered.
    pub fn register_configuration(
        &mut self,
        configuration: ConfigurationRecord,
        now: SystemTime,
    ) -> Result<(), RegisterRejected> {
        // Materialise every removal this region's configurations are already
        // owed before the ceilings are checked, so they bound the records the
        // registry really holds.
        self.prune_removed_configurations(&configuration.region, now);
        if self.configurations.contains_key(&configuration.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        if self.configurations.len() >= MAX_CONFIGURATIONS_IN_REGISTRY {
            return Err(RegisterRejected::RegistryCeiling);
        }
        if self.configuration_count_in(&configuration.region) >= MAX_CONFIGURATIONS_PER_REGION {
            return Err(RegisterRejected::RegionQuota);
        }
        *self
            .configurations_per_region
            .entry(configuration.region.clone())
            .or_insert(0) += 1;
        self.configuration_arn_by_name.insert(
            (
                configuration.region.clone(),
                configuration.name.clone(),
            ),
            configuration.arn.clone(),
        );
        self.configurations
            .insert(configuration.arn.clone(), configuration);
        Ok(())
    }

    /// Remove the configuration `arn` names from every index, as an endpoint for
    /// `region` sees it, handing the record back. The one removal path, so the
    /// ARN registry, the (region, name) index and the per-region count cannot
    /// drift apart — the counterpart of [`Self::register_configuration`], and
    /// what the settling lookups call once the clock owes a removal.
    ///
    /// Follows the same regional rule as [`Self::configuration_in`]: a
    /// well-formed ARN belonging to another region removes nothing and resolves
    /// to `None`, exactly as an absent one does, so a delete and a read agree on
    /// which configurations this endpoint holds. The name it held is free again,
    /// which is what lets a later create reuse it.
    pub fn unregister_configuration(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<ConfigurationRecord> {
        if self
            .configurations
            .get(arn)
            .is_none_or(|configuration| configuration.region != region)
        {
            return None;
        }
        let configuration = self.configurations.remove(arn)?;
        self.configuration_arn_by_name
            .remove(&(configuration.region.clone(), configuration.name.clone()));
        if let Some(count) = self.configurations_per_region.get_mut(&configuration.region) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.configurations_per_region.remove(&configuration.region);
            }
        }
        Some(configuration)
    }

    /// The ARNs of the clusters in `region` whose stored `ConfigurationInfo`
    /// names the configuration `arn`, in registry (ARN) order.
    ///
    /// A cluster keeps the configuration it was created with until a
    /// `UpdateClusterConfiguration` re-points it, so this is the association that
    /// decides whether a configuration is still being used: computed from the
    /// stored clusters rather than from a counter, so a cluster that has since
    /// left the registry stops holding its configuration down.
    ///
    /// The transitions the clock already owed those clusters are materialised
    /// FIRST, so a configuration a cluster has already been re-pointed away from
    /// — the re-pointing came due at some instant whether or not anybody read
    /// that cluster — is not reported as still in use. That write is owed at the
    /// same instant either way, which is why a reporting call may take it.
    pub fn clusters_using_configuration(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Vec<String> {
        self.clusters
            .values_mut()
            .filter(|cluster| cluster.region == region)
            .filter_map(|cluster| {
                cluster.settle(now);
                cluster
                    .configuration_info
                    .as_ref()
                    .is_some_and(|info| info.arn == arn)
                    .then(|| cluster.arn.clone())
            })
            .collect()
    }

    /// Add `connection` to the VPC-connection registry. The one insertion path,
    /// so it is also the one place the registry's ceiling is enforced — a create
    /// past it is refused here rather than for whichever handler remembered to
    /// check.
    ///
    /// Refuses, changing nothing, when the registry is at
    /// [`MAX_VPC_CONNECTIONS_IN_REGISTRY`] or when the ARN is already
    /// registered.
    pub fn register_vpc_connection(
        &mut self,
        connection: VpcConnection,
    ) -> Result<(), RegisterRejected> {
        if self.vpc_connections.contains_key(&connection.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        if self.vpc_connections.len() >= MAX_VPC_CONNECTIONS_IN_REGISTRY {
            return Err(RegisterRejected::RegistryCeiling);
        }
        self.vpc_connections
            .insert(connection.arn.clone(), connection);
        Ok(())
    }

    /// The VPC connection `arn` names, as an endpoint for `region` sees it: a
    /// regional endpoint serves only its own region's connections, so a
    /// well-formed ARN from another region resolves to `None` exactly as an
    /// absent one does.
    pub fn vpc_connection_in(&self, region: &str, arn: &str) -> Option<&VpcConnection> {
        self.vpc_connections
            .get(arn)
            .filter(|connection| connection.region == region)
    }

    /// Remove the VPC connection `arn` names, as an endpoint for `region` sees
    /// it: an ARN belonging to another region resolves to `None` exactly as an
    /// absent one does, so no endpoint can delete another region's connection.
    pub fn unregister_vpc_connection_in(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<VpcConnection> {
        self.vpc_connection_in(region, arn)?;
        self.vpc_connections.remove(arn)
    }

    /// The same lookup, with the transitions the clock already owed THIS
    /// connection materialised. Scoped to the one connection being reported, for
    /// the same reason as [`Self::settled_cluster_in`].
    pub fn settled_vpc_connection_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&VpcConnection> {
        self.settled_vpc_connection_mut_in(region, arn, now)
            .map(|connection| &*connection)
    }

    /// The write counterpart of [`Self::settled_vpc_connection_in`], with the
    /// same regional scoping and the same settling, so an operation that CHANGES
    /// a connection can only ever reach one a reader of this endpoint also sees,
    /// and decides on the state the clock already owed it.
    pub fn settled_vpc_connection_mut_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut VpcConnection> {
        let connection = self.vpc_connections.get_mut(arn)?;
        if connection.region != region {
            return None;
        }
        connection.settle(now);
        Some(connection)
    }

    /// The cluster `arn` names in `region`, mutably — the write counterpart of
    /// [`Self::cluster_in`], with the same regional scoping, so a mutating
    /// operation can only ever reach a cluster a reader of this endpoint also
    /// sees.
    pub fn cluster_mut_in(&mut self, region: &str, arn: &str) -> Option<&mut Cluster> {
        self.clusters
            .get_mut(arn)
            .filter(|cluster| cluster.region == region)
    }

    /// The stored tag set of the resource `arn` names in `region`, as a caller at
    /// `now` sees it.
    ///
    /// This is the ONE resolution the tagging operations share, so
    /// `TagResource`, `UntagResource` and `ListTagsForResource` cannot disagree
    /// about which ARNs name a taggable resource or about which failure a given
    /// ARN earns. Tags live on the resource record itself, which is why a tag
    /// written here is visible to every other reader of that resource (a
    /// cluster's tags are reported by `DescribeCluster` and `ListClusters`, a
    /// channel's by `DescribeChannel` and `ListChannels`) — and why `now` is
    /// needed: a channel whose removal the clock already owes is gone for the
    /// tagging operations exactly as it is for its own reader.
    pub fn resource_tags_in(
        &self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Result<&BTreeMap<String, String>, TagTargetRejected> {
        match resource_type_of_arn(arn)? {
            CLUSTER_RESOURCE_TYPE => self
                .cluster_in(region, arn)
                .map(|cluster| &cluster.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            CHANNEL_RESOURCE_TYPE => self
                .channel_by_arn_in(region, arn, now)
                .map(|channel| &channel.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            REPLICATOR_RESOURCE_TYPE => self
                .replicator_in(region, arn)
                .map(|replicator| &replicator.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            // Another MSK resource type: a well-formed ARN of a kind no
            // registry here carries a tag set for yet — the creates that accept
            // `Tags` for them (`CreateConfiguration`, `CreateVpcConnection`, …)
            // are still scaffolds — so this region holds no such taggable
            // resource. That is a miss, not a malformed request, and this arm is
            // the one place to extend when one of those registries starts
            // storing tags.
            _ => Err(TagTargetRejected::NoSuchResource),
        }
    }

    /// The same resolution, mutably — what `TagResource` and `UntagResource`
    /// write through.
    pub fn resource_tags_mut_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Result<&mut BTreeMap<String, String>, TagTargetRejected> {
        match resource_type_of_arn(arn)? {
            CLUSTER_RESOURCE_TYPE => self
                .cluster_mut_in(region, arn)
                .map(|cluster| &mut cluster.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            CHANNEL_RESOURCE_TYPE => self
                .channel_by_arn_mut_in(region, arn, now)
                .map(|channel| &mut channel.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            REPLICATOR_RESOURCE_TYPE => self
                .replicator_mut_in(region, arn)
                .map(|replicator| &mut replicator.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            _ => Err(TagTargetRejected::NoSuchResource),
        }
    }

    /// The channel `arn` names, as an endpoint for `region` sees it at `now`,
    /// resolved by its OWN ARN alone.
    ///
    /// That is the form an operation that takes any resource ARN addresses a
    /// channel in: the cluster is not a separate request member there, it is a
    /// segment of the channel ARN itself (`channel/<cluster>/<cluster-id>/...`),
    /// so a channel ARN already names the cluster that holds it.
    /// [`Self::channel_in`] stays the resolution for the paths that carry the
    /// cluster separately and must agree with it. A channel whose removal the
    /// clock already owes resolves to `None`, exactly as it does there.
    pub fn channel_by_arn_in(&self, region: &str, arn: &str, now: SystemTime) -> Option<&Channel> {
        self.channels
            .get(arn)
            .filter(|channel| channel.region == region && channel.is_live(now))
    }

    /// The same lookup, mutably — the write counterpart of
    /// [`Self::channel_by_arn_in`], with the same regional scoping.
    pub fn channel_by_arn_mut_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut Channel> {
        self.channels
            .get_mut(arn)
            .filter(|channel| channel.region == region && channel.is_live(now))
    }

    /// The replicator `arn` names in `region`, mutably — the write counterpart of
    /// [`Self::replicator_in`], with the same regional scoping, so a mutating
    /// operation can only ever reach a replicator a reader of this endpoint also
    /// sees.
    pub fn replicator_mut_in(&mut self, region: &str, arn: &str) -> Option<&mut Replicator> {
        self.replicators
            .get_mut(arn)
            .filter(|replicator| replicator.region == region)
    }

    /// The cluster operation `arn` names, as an endpoint for `region` sees it —
    /// `None` for an operation of another region, exactly as for an absent one.
    pub fn cluster_operation_in(&self, region: &str, arn: &str) -> Option<&ClusterOperation> {
        self.cluster_operations
            .get(arn)
            .filter(|operation| operation.region == region)
    }

    /// The operations `cluster` has recorded, latest first.
    ///
    /// The cluster's own index into the operation registry, resolved through it
    /// and ordered by [`ClusterOperation::history_key`]: ONE derivation of a
    /// cluster's operation history, so a listing of it reports exactly the
    /// records [`Self::cluster_operation_in`] resolves for the same ARNs, in
    /// one order. An index entry whose record the history bound has already
    /// evicted is skipped rather than reported as a gap.
    pub fn cluster_operation_history(&self, cluster: &Cluster) -> Vec<&ClusterOperation> {
        let mut history: Vec<&ClusterOperation> = cluster
            .operation_arns
            .iter()
            .filter_map(|arn| self.cluster_operations.get(arn))
            .collect();
        history.sort_by(|left, right| left.history_key().cmp(&right.history_key()));
        history
    }

    /// Start a cluster operation on the cluster `arn` names, as an endpoint for
    /// `region` sees it: record the cluster operation, move the cluster to the
    /// state that operation puts it in, and hand it the change the clock will
    /// apply at [`CLUSTER_UPDATE_DELAY`]. Answers the new operation's ARN, or
    /// `None` when this region holds no cluster for `arn`.
    ///
    /// The cluster is resolved through [`Self::settled_cluster_in_mut`], so a
    /// writer is scoped to its own region exactly as every reader is, and it
    /// updates a record whose clock-owed transitions have landed.
    ///
    /// `plan` turns the stored cluster into what the operation reports and what
    /// it changes: the attributes it touches as they are now, the same
    /// attributes as the operation leaves them, and the change itself.
    ///
    /// The ONE path that starts a cluster operation, so an operation ARN such a
    /// call hands back always names a record this registry holds — whichever call
    /// began it — and the operation record, the cluster's pending change,
    /// its active operation ARN and its operation history cannot drift apart.
    /// The history is bounded where every operation's record is filed, in
    /// [`Self::file_cluster_operation`].
    pub(super) fn start_cluster_operation<Plan>(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        operation_type: &str,
        plan: Plan,
    ) -> Option<String>
    where
        Plan: FnOnce(&Cluster) -> (MutableClusterInfo, MutableClusterInfo, ClusterChange),
    {
        let cluster = self.settled_cluster_in_mut(region, arn, now)?;
        let cluster_arn = cluster.arn.clone();
        let operation_region = cluster.region.clone();
        let cluster_type = cluster.cluster_type.clone();
        let (source_cluster_info, target_cluster_info, change) = plan(cluster);
        // Read before the change is handed over: how long the work takes is the
        // change's own property, and the operation record and the cluster's
        // pending change are both filed from this one value, so they cannot
        // disagree about when the operation finished.
        let settling = change.settling_delay();
        // The cluster mints the operation's ARN and holds the pending change:
        // one in-flight operation per cluster, whatever began it.
        let operation_arn = cluster.begin_operation(now, change);
        let operation = ClusterOperation {
            arn: operation_arn.clone(),
            cluster_arn,
            region: operation_region,
            cluster_type,
            operation_type: operation_type.to_string(),
            creation_time: now,
            end_due: now + settling,
            source_cluster_info,
            target_cluster_info,
        };
        self.file_cluster_operation(operation);
        Some(operation_arn)
    }

    /// Start a broker REBOOT: the brokers of the cluster `arn` names restart, so
    /// the cluster is `REBOOTING_BROKER` until the clock owes the operation its
    /// completion and `ACTIVE` again after it, at the version it already was.
    ///
    /// Filed through the same path every cluster operation is, under this call's
    /// own `OperationType`, so a reboot appears in the cluster's operation history
    /// and resolves for the cluster-operation readers exactly as an update does. It
    /// reports no mutable attribute on either side because it changes none: an
    /// operation describes what it touches.
    pub fn start_broker_reboot(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<String> {
        self.start_cluster_operation(region, arn, now, OPERATION_TYPE_REBOOT_NODE, |_cluster| {
            (
                MutableClusterInfo::default(),
                MutableClusterInfo::default(),
                ClusterChange::BrokerReboot,
            )
        })
    }

    /// Start a storage update: the volume size, provisioned throughput and
    /// storage mode `change` names, reported by an operation of
    /// `operation_type` — `UpdateStorage` and `UpdateBrokerStorage` both come
    /// through here, each naming its own type.
    pub fn start_storage_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        operation_type: &str,
        change: StorageChange,
    ) -> Option<String> {
        self.start_cluster_operation(region, arn, now, operation_type, |cluster| {
            let source_cluster_info = cluster.storage_cluster_info();
            let target_cluster_info = cluster.storage_cluster_info_after(&change);
            (
                source_cluster_info,
                target_cluster_info,
                ClusterChange::Storage(change),
            )
        })
    }

    /// Start a broker-type update: every broker of the cluster moves to
    /// `instance_type` once the clock owes the operation its completion.
    pub fn start_broker_type_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        instance_type: String,
    ) -> Option<String> {
        self.start_cluster_operation(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_BROKER_TYPE,
            |cluster| {
                let source_cluster_info = cluster.broker_type_cluster_info();
                let target_cluster_info = broker_type_cluster_info(&instance_type);
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::BrokerType { instance_type },
                )
            },
        )
    }

    /// Start a broker-count update: the cluster holds
    /// `target_number_of_broker_nodes` broker nodes once the clock owes the
    /// operation its completion.
    pub fn start_broker_count_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        target_number_of_broker_nodes: i32,
    ) -> Option<String> {
        // Bounded where the request value ENTERS the registry, so neither the
        // operation record it files nor the pending change it stores can carry a
        // count the model's range does not admit.
        let target = admissible_broker_count(target_number_of_broker_nodes);
        self.start_cluster_operation(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_BROKER_COUNT,
            |cluster| {
                let source_cluster_info = cluster.broker_count_cluster_info();
                let target_cluster_info = cluster.broker_count_cluster_info_after(target);
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::BrokerCount {
                        number_of_broker_nodes: target,
                    },
                )
            },
        )
    }

    /// Start an Apache Kafka version update: the brokers move to
    /// `kafka_version`, together with the custom configuration the request
    /// named for that version, once the clock owes the operation its
    /// completion.
    pub fn start_kafka_version_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        kafka_version: String,
        configuration_info: Option<ConfigurationInfo>,
    ) -> Option<String> {
        self.start_cluster_operation(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_CLUSTER_KAFKA_VERSION,
            |cluster| {
                let source_cluster_info = cluster.kafka_version_cluster_info();
                let target_cluster_info = kafka_version_cluster_info(
                    &kafka_version,
                    configuration_info
                        .clone()
                        .or_else(|| cluster.configuration_info.clone()),
                );
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::KafkaVersion {
                        kafka_version,
                        configuration_info,
                    },
                )
            },
        )
    }

    /// Start a configuration update: the brokers move to the MSK configuration
    /// revision `configuration_info` names once the clock owes the operation its
    /// completion.
    pub fn start_configuration_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        configuration_info: ConfigurationInfo,
    ) -> Option<String> {
        self.start_cluster_operation(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_CLUSTER_CONFIGURATION,
            |cluster| {
                let source_cluster_info = cluster.configuration_cluster_info();
                let target_cluster_info =
                    configuration_cluster_info(Some(configuration_info.clone()));
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::Configuration { configuration_info },
                )
            },
        )
    }

    /// Start a connectivity update: the broker access configuration, the
    /// ZooKeeper access setting, or both, as the request named them.
    pub fn start_connectivity_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        connectivity_info: Option<ConnectivityInfo>,
        zookeeper_access: Option<ZookeeperAccess>,
    ) -> Option<String> {
        self.start_cluster_operation(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_CONNECTIVITY,
            |cluster| {
                let source_cluster_info = cluster.connectivity_cluster_info();
                let target_cluster_info = cluster.connectivity_cluster_info_after(
                    connectivity_info.as_ref(),
                    zookeeper_access.as_ref(),
                );
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::Connectivity {
                        connectivity_info,
                        zookeeper_access,
                    },
                )
            },
        )
    }

    /// Start a security update: the client authentication the cluster accepts,
    /// the encryption it runs with, or both, as the request named them.
    pub fn start_security_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        client_authentication: Option<ClientAuthentication>,
        encryption_info: Option<EncryptionInfo>,
    ) -> Option<String> {
        self.start_cluster_operation(region, arn, now, OPERATION_TYPE_UPDATE_SECURITY, |cluster| {
            let source_cluster_info = cluster.security_cluster_info();
            let target_cluster_info = cluster.security_cluster_info_after(
                client_authentication.as_ref(),
                encryption_info.as_ref(),
            );
            (
                source_cluster_info,
                target_cluster_info,
                ClusterChange::Security {
                    client_authentication,
                    encryption_info,
                },
            )
        })
    }

    /// Start a monitoring update: the CloudWatch metrics level the cluster
    /// gathers, the open-monitoring exporters it runs, the broker log delivery
    /// it performs, or any combination the request named.
    pub fn start_monitoring_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        enhanced_monitoring: Option<String>,
        open_monitoring: Option<OpenMonitoringInfo>,
        logging_info: Option<LoggingInfo>,
    ) -> Option<String> {
        self.start_cluster_operation(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_MONITORING,
            |cluster| {
                let source_cluster_info = cluster.monitoring_cluster_info();
                let target_cluster_info = cluster.monitoring_cluster_info_after(
                    enhanced_monitoring.as_deref(),
                    open_monitoring.as_ref(),
                    logging_info.as_ref(),
                );
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::Monitoring {
                        enhanced_monitoring,
                        open_monitoring,
                        logging_info,
                    },
                )
            },
        )
    }

    /// Start a rebalancing update: the intelligent-rebalancing status the
    /// cluster runs its partition balancing under becomes the one the request
    /// named once the clock owes the operation its completion.
    pub fn start_rebalancing_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        rebalancing: Rebalancing,
    ) -> Option<String> {
        self.start_cluster_operation(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_REBALANCING,
            |cluster| {
                let source_cluster_info = cluster.rebalancing_cluster_info();
                let target_cluster_info = cluster.rebalancing_cluster_info_after(&rebalancing);
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::Rebalancing { rebalancing },
                )
            },
        )
    }

    /// File `operation` in the registry and in its own cluster's history, so the
    /// ARN a reader resolves and the history a cluster lists cannot drift apart.
    ///
    /// The history is bounded here: the records past
    /// [`MAX_OPERATIONS_PER_CLUSTER`] are that cluster's own oldest, so the trim
    /// costs one walk of THAT cluster's history rather than of the registry. The
    /// one filing path, so every update operation bounds its history the same way.
    pub(super) fn file_cluster_operation(&mut self, operation: ClusterOperation) {
        let operation_arn = operation.arn.clone();
        let evicted = match self.clusters.get_mut(&operation.cluster_arn) {
            Some(cluster) => {
                cluster.operation_arns.push(operation_arn.clone());
                let excess = cluster
                    .operation_arns
                    .len()
                    .saturating_sub(MAX_OPERATIONS_PER_CLUSTER);
                cluster.operation_arns.drain(..excess).collect::<Vec<_>>()
            }
            // Unreachable: an operation is filed for the cluster it was just
            // started on, under the same lock. Filed anyway rather than dropped,
            // so the ARN the caller was handed still resolves.
            None => Vec::new(),
        };
        self.cluster_operations
            .insert(operation_arn, operation);
        for stale in evicted {
            self.cluster_operations.remove(&stale);
        }
    }

    /// How many topics the cluster `cluster_arn` names holds as a reader at
    /// `now` sees it — what the per-cluster ceiling is checked against. A topic
    /// whose removal the clock already owes is NOT counted, so the ceiling
    /// agrees with what every reader reports rather than reserving room for
    /// records that are gone. The registry key leads with the cluster ARN, so
    /// one cluster's topics are a contiguous range.
    pub fn topic_count_in(&self, cluster_arn: &str, now: SystemTime) -> usize {
        self.topics_in(cluster_arn)
            .filter(|topic| topic.is_present(now))
            .count()
    }

    /// Every topic of the cluster `cluster_arn` names, in topic-name order.
    pub fn topics_in<'a>(&'a self, cluster_arn: &'a str) -> impl Iterator<Item = &'a Topic> + 'a {
        self.topics
            .range((cluster_arn.to_string(), String::new())..)
            .take_while(move |((arn, _), _)| arn == cluster_arn)
            .map(|(_, topic)| topic)
    }

    /// Add `topic` to the registry. The one insertion path, and the one place the
    /// per-cluster ceiling is enforced, so it holds for every caller of this
    /// module rather than only for a handler that remembers to check first.
    ///
    /// `now` is the instant the create is served: the removals this cluster's
    /// topics are already owed are materialised FIRST, so a name whose topic is
    /// gone as far as every reader is concerned is free here too (whether or not
    /// anybody read it in between) and the ceiling bounds what the registry
    /// actually holds.
    ///
    /// Refuses, changing nothing, when the registry holds no cluster for the
    /// ARN the topic is filed under (so nothing would ever reclaim the record),
    /// when the cluster still holds a topic of that name (a `DELETING` one whose
    /// removal is not yet due still counts: it is still there), when the cluster
    /// already holds [`MAX_TOPICS_PER_CLUSTER`] topics, or when the registry
    /// itself is at [`MAX_TOPICS_IN_REGISTRY`] — the backstop that bounds growth
    /// however many cluster ARNs a caller presents, since the ARN comes from the
    /// request.
    pub fn register_topic(
        &mut self,
        topic: Topic,
        now: SystemTime,
    ) -> Result<(), TopicRejected> {
        // Materialise every removal THIS CLUSTER's topics are already owed, not
        // only the addressed name: the ceiling below has to bound the records the
        // registry holds, so a caller looping create-and-delete under fresh names
        // cannot grow it past the ceiling. Each removal was due at the same
        // instant whether or not anybody addressed that topic.
        self.prune_removed_topics(&topic.cluster_arn, now);
        // A topic lives on the brokers of ONE cluster, and a cluster leaving the
        // registry takes its topics with it, so a record filed under an ARN the
        // registry holds no cluster for is a record no removal path reaches.
        if !self.clusters.contains_key(&topic.cluster_arn) {
            return Err(TopicRejected::NoSuchCluster);
        }
        let key = (topic.cluster_arn.clone(), topic.name.clone());
        if self.topics.contains_key(&key) {
            return Err(TopicRejected::DuplicateName);
        }
        if self.topics.len() >= MAX_TOPICS_IN_REGISTRY {
            return Err(TopicRejected::RegistryCeiling);
        }
        if self.topic_count_in(&topic.cluster_arn, now) >= MAX_TOPICS_PER_CLUSTER {
            return Err(TopicRejected::ClusterCeiling);
        }
        self.topics.insert(key, topic);
        Ok(())
    }

    /// Drop the topics of `cluster_arn` whose removal the clock owed by `now`.
    /// Scoped to the one cluster, which the registry key makes a contiguous
    /// range, so this never walks the whole registry.
    pub(super) fn prune_removed_topics(&mut self, cluster_arn: &str, now: SystemTime) {
        let removed: Vec<(String, String)> = self
            .topics_in(cluster_arn)
            .filter(|topic| !topic.is_present(now))
            .map(|topic| (topic.cluster_arn.clone(), topic.name.clone()))
            .collect();
        for key in removed {
            self.topics.remove(&key);
        }
    }

    /// The same lookup, with the transitions the clock already owed THIS topic
    /// materialised: a topic whose removal is due is gone (so this answers
    /// `None`), and one whose activation is due is `ACTIVE`.
    ///
    /// Scoped to the one topic being addressed: a read must not do work
    /// proportional to the registry, and no other topic's status is this
    /// request's business.
    pub fn settled_topic_in(
        &mut self,
        cluster_arn: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&Topic> {
        let key = (cluster_arn.to_string(), name.to_string());
        if self
            .topics
            .get(&key)
            .and_then(|topic| topic.removal_due)
            .is_some_and(|due| now >= due)
        {
            self.topics.remove(&key);
            return None;
        }
        let topic = self.topics.get_mut(&key)?;
        topic.settle(now);
        Some(topic)
    }

    /// Accept the deletion of the topic `name` in `cluster_arn`, handing back the
    /// record as the caller must now see it: `DELETING`, with the instant the
    /// clock owes its removal set.
    ///
    /// Answers `None` when the cluster holds no such topic — including one whose
    /// removal the clock already owed, which this materialises first so a topic
    /// is never deleted twice. A topic already `DELETING` keeps the removal
    /// instant it was given, so repeating the call cannot postpone it.
    pub fn begin_topic_deletion(
        &mut self,
        cluster_arn: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&Topic> {
        self.settled_topic_in(cluster_arn, name, now)?;
        let topic = self
            .topics
            .get_mut(&(cluster_arn.to_string(), name.to_string()))?;
        if topic.removal_due.is_none() {
            topic.state = TOPIC_STATE_DELETING.to_string();
            topic.removal_due = Some(now + TOPIC_DELETION_DELAY);
        }
        Some(topic)
    }

    /// Accept a CHANGE to the topic `name` of `cluster_arn`, handing back the
    /// record as the caller must now see it: carrying the partition count and the
    /// topic-level configuration the request named, `UPDATING`, with the instant
    /// the clock owes its return to `ACTIVE` set.
    ///
    /// `partition_count` is the new total the request named, `None` where it named
    /// none, which leaves the count as it is; `configs` is the configuration it
    /// submitted, `None` leaving the stored one. Answers `None` when the cluster
    /// holds no such topic — including one whose removal the clock already owed,
    /// which [`Self::settled_topic_in`] materialises first.
    ///
    /// The ONE path a topic change comes through, which is what serializes two
    /// concurrent ones: a caller holds this registry while it resolves the topic,
    /// judges the change against the state the record is in and applies it, so of
    /// two concurrent changes to one topic the second meets the state the first
    /// left behind rather than the state both started from.
    pub fn start_topic_update(
        &mut self,
        cluster_arn: &str,
        name: &str,
        partition_count: Option<i32>,
        configs: Option<String>,
        now: SystemTime,
    ) -> Option<&Topic> {
        self.settled_topic_in(cluster_arn, name, now)?;
        let topic = self
            .topics
            .get_mut(&(cluster_arn.to_string(), name.to_string()))?;
        // Apache Kafka adds the partitions as it admits the change, so the topic
        // HOLDS the new total from the instant the call is accepted; what the
        // `UPDATING` span below stands for is the broker-side reassignment that
        // follows it.
        if let Some(count) = partition_count {
            topic.partition_count = count;
        }
        if configs.is_some() {
            topic.configs = configs;
        }
        topic.state = TOPIC_STATE_UPDATING.to_string();
        // The span a created topic takes to become `ACTIVE` is the span an
        // updated one takes: the same clock-owed transition of the same record,
        // materialised by [`Topic::settle`].
        topic.active_due = now + TOPIC_ACTIVATION_DELAY;
        Some(topic)
    }

    /// The ARN registered for the replicator `name` in `region`, if any. This is
    /// the lookup that makes a repeated `CreateReplicator` a conflict.
    pub fn replicator_arn_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.replicator_arn_by_name
            .get(&(region.to_string(), name.to_string()))
    }

    /// The provisioning failure a replicator over `clusters` carries, or `None`
    /// when every Amazon MSK cluster its setup names resolves.
    ///
    /// Amazon MSK ADMITS a `CreateReplicator` request on what the request itself
    /// says and resolves the clusters while it provisions the replicator, so a
    /// setup naming a cluster that does not exist is not refused: the replicator
    /// is registered and the clock converges it to [`REPLICATOR_STATE_FAILED`]
    /// carrying this detail. The FIRST entry that does not resolve is the one
    /// reported, in submission order, so one setup yields one reason.
    ///
    /// Resolved against the whole registry rather than one region's clusters: a
    /// replicator replicates BETWEEN regions, so a cluster of another region is a
    /// cluster that exists. A `KafkaClusters` entry naming a self-managed Apache
    /// Kafka cluster resolves to nothing here — that cluster is not a resource
    /// this service holds.
    pub fn replicator_provisioning_failure(
        &self,
        clusters: &[KafkaCluster],
    ) -> Option<ReplicationStateInfo> {
        let unresolved = clusters
            .iter()
            .filter_map(|cluster| cluster.amazon_msk_cluster.as_ref())
            .map(|msk| msk.msk_cluster_arn.as_str())
            .find(|arn| !self.clusters.contains_key(*arn))?;
        Some(ReplicationStateInfo {
            code: Some(REPLICATION_CODE_INVALID_KAFKA_CLUSTER.to_string()),
            message: Some(invalid_kafka_cluster_message(unresolved)),
        })
    }

    /// Add `replicator` to every index that must know about it. The one insertion
    /// path, so the ARN registry and the (region, name) index cannot drift apart
    /// — and the one place the registry's ceiling is enforced, so it holds for
    /// every caller of this module rather than only for a handler that remembers
    /// to check first.
    ///
    /// Refuses, changing nothing, when the registry is at
    /// [`MAX_REPLICATORS_IN_REGISTRY`] or when the ARN is already registered.
    pub fn register_replicator(&mut self, replicator: Replicator) -> Result<(), RegisterRejected> {
        if self.replicators.contains_key(&replicator.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        if self.replicators.len() >= MAX_REPLICATORS_IN_REGISTRY {
            return Err(RegisterRejected::RegistryCeiling);
        }
        self.replicator_arn_by_name.insert(
            (replicator.region.clone(), replicator.name.clone()),
            replicator.arn.clone(),
        );
        // One entry per Amazon MSK cluster this replicator replicates INTO, which
        // is what a reader of that cluster's topics looks the replicator up by. A
        // flow whose target is a self-managed Apache Kafka cluster is indexed
        // under nothing: that cluster's topics are not this service's to report.
        for target in replicator.target_cluster_arns() {
            self.replicator_arns_by_target_cluster
                .entry(target)
                .or_default()
                .insert(replicator.arn.clone());
        }
        self.replicators.insert(replicator.arn.clone(), replicator);
        Ok(())
    }

    /// Remove the replicator `arn` names from every index, handing it back. The
    /// one removal path, for the same reason.
    pub fn unregister_replicator(&mut self, arn: &str) -> Option<Replicator> {
        let replicator = self.replicators.remove(arn)?;
        self.replicator_arn_by_name
            .remove(&(replicator.region.clone(), replicator.name.clone()));
        for target in replicator.target_cluster_arns() {
            if let Some(arns) = self.replicator_arns_by_target_cluster.get_mut(&target) {
                arns.remove(arn);
                if arns.is_empty() {
                    self.replicator_arns_by_target_cluster.remove(&target);
                }
            }
        }
        Some(replicator)
    }

    /// Every topic the cluster `cluster_arn` names reports to a reader at `now`:
    /// the topics it holds itself, plus the topics a running replicator
    /// replicates onto it. Keyed, and so ordered, by the name the cluster reports
    /// each topic under — the order `ListTopics` pages through.
    ///
    /// The ONE derivation the topic readers answer from, so a listing and a
    /// describe of the same cluster cannot disagree about which topics are on it.
    /// A name the cluster holds a topic of its own under is that topic: a
    /// replicated name never shadows a stored record.
    pub fn observable_topics_in(
        &self,
        cluster_arn: &str,
        now: SystemTime,
    ) -> BTreeMap<String, Topic> {
        let mut topics = self.replicated_topics_into(cluster_arn, None, now);
        for topic in self
            .topics_in(cluster_arn)
            .filter(|topic| topic.is_present(now))
        {
            topics.insert(topic.name.clone(), topic.clone());
        }
        topics
    }

    /// The same derivation for the ONE name a reader addresses: the cluster's own
    /// topic of that name, with the transitions the clock already owed it
    /// materialised, or else the topic a running replicator replicates onto this
    /// cluster under it.
    pub fn observable_topic_in(
        &mut self,
        cluster_arn: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<Topic> {
        if let Some(topic) = self.settled_topic_in(cluster_arn, name, now) {
            return Some(topic.clone());
        }
        self.replicated_topics_into(cluster_arn, Some(name), now)
            .remove(name)
    }

    /// The topics the registry's replicators replicate onto the cluster
    /// `target_cluster_arn` names, as a reader at `now` sees them, keyed by the
    /// name the target reports each one under; `wanted` narrows the derivation to
    /// one such name.
    ///
    /// A replicated topic is DERIVED, never a second stored record: it is
    /// reported exactly while a replicator that is replicating has a flow into
    /// this cluster whose filter covers a topic its source cluster still holds,
    /// so deleting that topic — or the replicator — withdraws it with nothing
    /// having to remember to. A flow's own source topics are reported from the
    /// moment the clock owes the propagation ([`TOPIC_REPLICATION_DELAY`] after
    /// the flow and the topic are both in place), and a flow that does not detect
    /// new topics carries only the ones its source cluster already held when the
    /// replicator was created.
    ///
    /// The work is bounded for every reader: the per-target index narrows it to
    /// the flows INTO this cluster, [`MAX_REPLICATION_FLOWS_PER_TARGET`] bounds how
    /// many of those one reader derives from, [`MAX_TOPICS_PER_CLUSTER`] bounds
    /// what it reports — the same ceiling the cluster's own topics obey — and the
    /// patterns were compiled when they were stored, so nothing here compiles a
    /// caller's pattern.
    pub(super) fn replicated_topics_into(
        &self,
        target_cluster_arn: &str,
        wanted: Option<&str>,
        now: SystemTime,
    ) -> BTreeMap<String, Topic> {
        let mut replicated = BTreeMap::new();
        // A cluster this endpoint does not hold reports nothing at all, and a
        // replicated topic's identity is derived from the target cluster's own.
        let Some(target) = self.clusters.get(target_cluster_arn) else {
            return replicated;
        };
        let mut flows_derived = 0usize;
        for replicator in self
            .replicator_arns_by_target_cluster
            .get(target_cluster_arn)
            .into_iter()
            .flatten()
            .filter_map(|arn| self.replicators.get(arn))
            .filter(|replicator| replicator.replicates_at(now))
        {
            for (index, info) in replicator.replication_info.iter().enumerate().filter(|(_, info)| {
                info.target_kafka_cluster_arn.as_deref() == Some(target_cluster_arn)
            }) {
                // The two bounds this derivation is capped by, checked before each
                // flow is walked: the flows one reader derives from, and the topics
                // one cluster reports.
                if flows_derived >= MAX_REPLICATION_FLOWS_PER_TARGET
                    || replicated.len() >= MAX_TOPICS_PER_CLUSTER
                {
                    return replicated;
                }
                flows_derived += 1;
                // Only an Amazon MSK source cluster's topics are this service's to
                // read: a self-managed Apache Kafka cluster's topics live on that
                // cluster, which this emulator does not hold.
                let Some(source_arn) = info.source_kafka_cluster_arn.as_deref() else {
                    continue;
                };
                // Compiled when the flow was stored, so a listing matches topics
                // rather than compiling what a request supplied.
                let Some(filter) = replicator.topic_filter(index) else {
                    continue;
                };
                let settings = &info.topic_replication;
                // Replicated names are prefixed with the source cluster's alias
                // unless the flow asks for names identical to the source's, which
                // is the one thing `ReplicationTopicNameConfiguration` configures.
                let prefixed = settings
                    .topic_name_configuration
                    .as_ref()
                    .and_then(|configuration| configuration.r#type.as_deref())
                    .is_none_or(|kind| kind != TOPIC_NAME_IDENTICAL);
                let alias = replicator
                    .alias_of(Some(source_arn), None)
                    .unwrap_or_default();
                // A flow that does not say periodically detects new topics and
                // copies the source topics' configurations: those are the
                // replicator's own defaults, so only an explicit `false` turns
                // either off.
                let detects_new_topics = settings.detect_and_copy_new_topics != Some(false);
                let copies_configurations = settings.copy_topic_configurations != Some(false);
                let flow_start = replicator.replication_start();
                for source in self
                    .topics_in(source_arn)
                    .filter(|topic| topic.is_present(now))
                    .filter(|topic| filter.covers(&topic.name))
                    .filter(|topic| {
                        detects_new_topics || topic.created_at <= replicator.creation_time
                    })
                {
                    if replicated.len() >= MAX_TOPICS_PER_CLUSTER {
                        return replicated;
                    }
                    let name = replicated_topic_name(&alias, &source.name, prefixed);
                    // An Apache Kafka topic name is at most
                    // `MAX_TOPIC_NAME_LENGTH` characters, so a prefixed name past
                    // that length is one no cluster could hold.
                    if name.chars().count() > MAX_TOPIC_NAME_LENGTH {
                        continue;
                    }
                    if wanted.is_some_and(|wanted| wanted != name) {
                        continue;
                    }
                    let visible_at = source.active_due.max(flow_start) + TOPIC_REPLICATION_DELAY;
                    if now < visible_at {
                        continue;
                    }
                    let configs = if copies_configurations {
                        source.configs.clone()
                    } else {
                        None
                    };
                    // The first flow to replicate a name keeps it: two flows that
                    // carry one name onto a cluster are carrying the same topic
                    // there, and the cluster holds one topic per name.
                    replicated.entry(name.clone()).or_insert_with(|| {
                        Topic::replicated(target, name, source, configs, visible_at)
                    });
                }
            }
        }
        replicated
    }

    /// The replicator `arn` names, as an endpoint for `region` sees it.
    ///
    /// A regional endpoint serves only its own region's replicators, so a
    /// well-formed ARN belonging to another region resolves to `None` exactly as
    /// an absent one does.
    pub fn replicator_in(&self, region: &str, arn: &str) -> Option<&Replicator> {
        self.replicators
            .get(arn)
            .filter(|replicator| replicator.region == region)
    }

    /// The same lookup, with the `CREATING` -> `RUNNING` transition the clock
    /// already owed THIS replicator materialised.
    ///
    /// Scoped to the one replicator being reported: a read must not do work
    /// proportional to the whole registry, and no other replicator's status is
    /// this request's business.
    pub fn settled_replicator_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&Replicator> {
        let replicator = self.replicators.get_mut(arn)?;
        if replicator.region != region {
            return None;
        }
        replicator.settle(now);
        Some(replicator)
    }

    /// The ARN registered for channel `name` under the cluster `cluster_arn` as a
    /// caller at `now` sees it — the lookup that makes a repeated `CreateChannel`
    /// a conflict.
    ///
    /// A channel whose removal the clock already owes has freed its name, so this
    /// answers `None` for it: the create that follows must see the same registry
    /// the insertion path does.
    pub fn channel_arn_for_name(
        &self,
        cluster_arn: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&String> {
        let arn = self
            .channel_arn_by_name
            .get(&(cluster_arn.to_string(), name.to_string()))?;
        self.channels
            .get(arn)
            .filter(|channel| channel.is_live(now))
            .map(|_| arn)
    }

    /// How many channels `cluster_arn` currently holds — what the per-cluster
    /// ceiling is checked against. O(1): the index is maintained as channels come
    /// and go.
    pub fn channel_count_in_cluster(&self, cluster_arn: &str) -> usize {
        self.channel_arns_by_cluster
            .get(cluster_arn)
            .map_or(0, BTreeSet::len)
    }

    /// Drop the channels of `cluster_arn` whose removal the clock owed by `now`.
    /// Scoped to the one cluster through the per-cluster index, so this never
    /// walks the whole registry.
    pub(super) fn prune_removed_channels(&mut self, cluster_arn: &str, now: SystemTime) {
        let removed: Vec<String> = self
            .channel_arns_by_cluster
            .get(cluster_arn)
            .into_iter()
            .flatten()
            .filter(|arn| {
                self.channels
                    .get(*arn)
                    .is_some_and(|channel| !channel.is_live(now))
            })
            .cloned()
            .collect();
        for arn in removed {
            self.unregister_channel(&arn);
        }
    }

    /// Add `channel` to every index that must know about it. The one insertion
    /// path, so the ARN registry, the (cluster, name) index and the per-cluster
    /// index cannot drift apart — and the one place the per-cluster ceiling is
    /// enforced, so it holds for every caller of this module.
    ///
    /// `now` is the instant the create is served: the removals this cluster's
    /// channels are already owed are materialised FIRST, so a name whose channel
    /// is gone as far as every reader is concerned is free here too (whether or
    /// not anybody read it in between) and the ceiling bounds what the registry
    /// actually holds.
    ///
    /// Refuses, changing nothing, when the registry holds no cluster for the
    /// ARN the channel is filed under (so nothing would ever reclaim the
    /// record), when the cluster is at [`MAX_CHANNELS_PER_CLUSTER`], when the
    /// registry itself is at [`MAX_CHANNELS_IN_REGISTRY`] — the backstop that
    /// bounds growth however many cluster ARNs a caller presents, since the ARN
    /// comes from the request — or when the ARN is already registered.
    pub fn register_channel(
        &mut self,
        channel: Channel,
        now: SystemTime,
    ) -> Result<(), ChannelRejected> {
        self.prune_removed_channels(&channel.cluster_arn, now);
        // A channel reads from ONE cluster, and a cluster leaving the registry
        // takes its channels with it, so a record filed under an ARN the
        // registry holds no cluster for is a record no removal path reaches.
        if !self.clusters.contains_key(&channel.cluster_arn) {
            return Err(ChannelRejected::NoSuchCluster);
        }
        if self.channels.contains_key(&channel.arn) {
            return Err(ChannelRejected::DuplicateArn);
        }
        if self.channels.len() >= MAX_CHANNELS_IN_REGISTRY {
            return Err(ChannelRejected::RegistryCeiling);
        }
        if self.channel_count_in_cluster(&channel.cluster_arn) >= MAX_CHANNELS_PER_CLUSTER {
            return Err(ChannelRejected::ClusterQuota);
        }
        self.channel_arns_by_cluster
            .entry(channel.cluster_arn.clone())
            .or_default()
            .insert(channel.arn.clone());
        self.channel_arn_by_name.insert(
            (channel.cluster_arn.clone(), channel.name.clone()),
            channel.arn.clone(),
        );
        let channel_arn = channel.arn.clone();
        let create_operation_arn = channel.cluster_operation_arn.clone();
        let active_due = channel.active_due;
        self.channels.insert(channel.arn.clone(), channel);
        // The create answers a `ClusterOperationArn`, so the record that ARN
        // names is filed here, on the one insertion path: the operation finishes
        // when the channel it created becomes `ACTIVE`, so it ends at the same
        // instant the clock owes that transition.
        self.file_channel_operation(
            &channel_arn,
            create_operation_arn,
            OPERATION_TYPE_CREATE_CHANNEL,
            active_due,
            now,
        );
        Ok(())
    }

    /// File the cluster operation a channel lifecycle call hands its caller, so
    /// the ARN that caller polls names a record `DescribeClusterOperation` and
    /// `DescribeClusterOperationV2` resolve rather than nothing.
    ///
    /// The operation is filed through [`Self::file_cluster_operation`], the one
    /// filing path, so it appears in its cluster's history and bounds that
    /// history exactly as every other cluster operation does. It reports no
    /// mutable cluster attribute on either side because it changes none: a
    /// channel operation acts on the CHANNEL, and an operation describes what it
    /// touches. `end_due` is the instant the clock owes the channel transition
    /// this operation carries out, so the operation and the channel cannot
    /// disagree about when the work finished.
    fn file_channel_operation(
        &mut self,
        channel_arn: &str,
        operation_arn: String,
        operation_type: &str,
        end_due: SystemTime,
        now: SystemTime,
    ) {
        let Some(channel) = self.channels.get(channel_arn) else {
            return;
        };
        let cluster_arn = channel.cluster_arn.clone();
        let region = channel.region.clone();
        // Which kind of cluster the operation acted on, off that cluster's own
        // record, as every other filed operation stores it.
        let cluster_type = self
            .clusters
            .get(&cluster_arn)
            .map(|cluster| cluster.cluster_type.clone())
            .unwrap_or_default();
        self.file_cluster_operation(ClusterOperation {
            arn: operation_arn,
            cluster_arn,
            region,
            cluster_type,
            operation_type: operation_type.to_string(),
            creation_time: now,
            end_due,
            source_cluster_info: MutableClusterInfo::default(),
            target_cluster_info: MutableClusterInfo::default(),
        });
    }

    /// Remove the channel `arn` names from every index, handing it back. The one
    /// removal path, for the same reason — and so the one place a removal is
    /// REMEMBERED ([`Self::channel_was_held`]): a channel that is gone is not a
    /// channel that never existed, and the two are refused differently.
    pub fn unregister_channel(&mut self, arn: &str) -> Option<Channel> {
        let channel = self.channels.remove(arn)?;
        self.channel_arn_by_name
            .remove(&(channel.cluster_arn.clone(), channel.name.clone()));
        if let Some(arns) = self.channel_arns_by_cluster.get_mut(&channel.cluster_arn) {
            arns.remove(arn);
            if arns.is_empty() {
                self.channel_arns_by_cluster.remove(&channel.cluster_arn);
            }
        }
        self.remember_removed_channel(&channel.cluster_arn, arn);
        Some(channel)
    }

    /// Record that `cluster_arn` HELD the channel `channel_arn` names and holds
    /// it no longer, bounded at [`MAX_CHANNELS_PER_CLUSTER`] removals per cluster
    /// — the number of channels one cluster can hold at once — by forgetting the
    /// oldest removal first.
    fn remember_removed_channel(&mut self, cluster_arn: &str, channel_arn: &str) {
        let removed = self
            .removed_channel_arns
            .entry(cluster_arn.to_string())
            .or_default();
        if removed.iter().any(|held| held == channel_arn) {
            return;
        }
        removed.push_back(channel_arn.to_string());
        while removed.len() > MAX_CHANNELS_PER_CLUSTER {
            removed.pop_front();
        }
    }

    /// Whether this registry once held the channel `channel_arn` names under
    /// `cluster_arn` and holds it no longer.
    ///
    /// A channel-scoped call that cannot resolve its channel asks this to tell the
    /// two absences apart: a channel this cluster held and lost is a resource that
    /// went missing, while an ARN it never held is a reference the request got
    /// wrong. Scoped to the cluster in the request path, as every channel lookup
    /// is, so one cluster's removals say nothing about another's.
    pub fn channel_was_held(&self, cluster_arn: &str, channel_arn: &str) -> bool {
        self.removed_channel_arns
            .get(cluster_arn)
            .is_some_and(|removed| removed.iter().any(|held| held == channel_arn))
    }

    /// The channel `channel_arn` names, as an endpoint for `region` sees it, and
    /// only when the cluster in the request's path is the one that holds it.
    ///
    /// A channel is addressed under its cluster, so a channel ARN presented with
    /// another cluster's ARN resolves to `None` exactly as an absent one does —
    /// the same way [`Self::cluster_in`] refuses another region's cluster. A
    /// channel whose removal the clock already owes is absent too.
    pub fn channel_in(
        &self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
        now: SystemTime,
    ) -> Option<&Channel> {
        self.channels.get(channel_arn).filter(|channel| {
            channel.region == region && channel.cluster_arn == cluster_arn && channel.is_live(now)
        })
    }

    /// The same lookup, with the transitions the clock already owed THIS channel
    /// materialised: a channel whose removal is due is gone (so this answers
    /// `None`), and one whose activation is due is `ACTIVE`.
    ///
    /// Scoped to the one channel being reported, for the same reason
    /// [`Self::settled_cluster_in`] is.
    pub fn settled_channel_in(
        &mut self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
        now: SystemTime,
    ) -> Option<&Channel> {
        let channel = self.channels.get(channel_arn)?;
        if channel.region != region || channel.cluster_arn != cluster_arn {
            return None;
        }
        if !channel.is_live(now) {
            self.unregister_channel(channel_arn);
            return None;
        }
        let channel = self.channels.get_mut(channel_arn)?;
        channel.settle(now);
        Some(channel)
    }

    /// The channels `cluster_arn` holds, as an endpoint for `region` sees them at
    /// `now`, ordered by CHANNEL NAME — the identity a channel has within its
    /// cluster ("Must be unique within the cluster",
    /// `CreateChannelRequest$ChannelName`), so the order is total and does not
    /// depend on the id the ARN carries. The listing counterpart of
    /// [`Self::settled_channel_in`].
    ///
    /// Reporting only, and the writes are the ones the clock already owed: the
    /// removals this cluster's channels were due are materialised first, so a
    /// channel every other reader calls gone is not listed, and each remaining
    /// channel's `CREATING` -> `ACTIVE` transition is applied if it was due — both
    /// owed at this instant whether or not anybody listed.
    pub fn settled_channels_in(
        &mut self,
        region: &str,
        cluster_arn: &str,
        now: SystemTime,
    ) -> Vec<&Channel> {
        self.prune_removed_channels(cluster_arn, now);
        let arns: Vec<String> = self
            .channel_arns_by_cluster
            .get(cluster_arn)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        for arn in &arns {
            if let Some(channel) = self.channels.get_mut(arn) {
                channel.settle(now);
            }
        }
        let mut channels: Vec<&Channel> = arns
            .iter()
            .filter_map(|arn| self.channels.get(arn))
            .filter(|channel| channel.region == region)
            .collect();
        channels.sort_by(|left, right| left.name.cmp(&right.name));
        channels
    }

    /// Accept the deletion of the channel `channel_arn` names under
    /// `cluster_arn`, as an endpoint for `region` sees it, handing back the record
    /// as the caller must now see it: `DELETING`, with the instant the clock owes
    /// its removal and the delete's own cluster operation set.
    ///
    /// Answers `None` when this region holds no such channel under that cluster —
    /// including one whose removal the clock already owed, which the settling
    /// lookup materialises first, so a channel is never deleted twice. Idempotent
    /// through [`Channel::begin_deletion`].
    pub fn begin_channel_deletion(
        &mut self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
        now: SystemTime,
    ) -> Option<&Channel> {
        self.settled_channel_in(region, cluster_arn, channel_arn, now)?;
        let channel = self.channels.get_mut(channel_arn)?;
        // The delete is idempotent, so the operation is filed only for the call
        // that actually accepted one: a repeat must not re-file the record under
        // a later instant and move the completion the first call promised.
        let accepting = channel.deletion_operation_arn.is_none();
        channel.begin_deletion(now);
        let filing = accepting
            .then(|| (channel.deletion_operation_arn.clone(), channel.removal_due))
            .and_then(|(arn, due)| Some((arn?, due?)));
        if let Some((operation_arn, removal_due)) = filing {
            self.file_channel_operation(
                channel_arn,
                operation_arn,
                OPERATION_TYPE_DELETE_CHANNEL,
                removal_due,
                now,
            );
        }
        self.channels.get(channel_arn)
    }

    /// Accept a destination change on the channel `channel_arn` names under
    /// `cluster_arn`, as an endpoint for `region` sees it, handing back the
    /// record as the caller must now see it: the new
    /// `DataFreshnessInSeconds` written onto the destination it was created with,
    /// `UPDATING`, with the instant the clock owes it `ACTIVE` again and the
    /// update's own cluster operation filed.
    ///
    /// Answers `None` when this region holds no such channel under that cluster,
    /// resolved exactly as every channel reader resolves it. The STATUS gate is
    /// the caller's: this is the write path, and it is reached only for a channel
    /// the handler has already found updatable.
    pub fn begin_channel_update(
        &mut self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
        now: SystemTime,
        data_freshness_in_seconds: i32,
    ) -> Option<&Channel> {
        self.settled_channel_in(region, cluster_arn, channel_arn, now)?;
        let channel = self.channels.get_mut(channel_arn)?;
        let operation_arn = channel
            .begin_update(now, data_freshness_in_seconds)
            .to_string();
        let end_due = channel.active_due;
        self.file_channel_operation(
            channel_arn,
            operation_arn,
            OPERATION_TYPE_UPDATE_CHANNEL,
            end_due,
            now,
        );
        self.channels.get(channel_arn)
    }

    /// Choose one page of this endpoint's clusters: the keys a listing reports,
    /// plus its `NextToken`.
    ///
    /// Every cluster listing is a REGIONAL api, so only the clusters `region`
    /// holds are considered — the same set the ARN readers resolve against. The
    /// registry is ordered by ARN, so a token resumes by SEEKING to its position
    /// rather than walking the keys already returned; the scan from there stops as
    /// soon as the page is full, and only entries that fail the region or the
    /// filter are skipped past. `page_size` is clamped into the `MaxResults` range
    /// the model declares, so no caller of this module can size the page from an
    /// unchecked value.
    pub fn cluster_page(
        &self,
        region: &str,
        filter: ClusterFilter<'_>,
        resume_after: Option<&str>,
        page_size: usize,
    ) -> ClusterPage {
        let page_size = page_size.clamp(1, MAX_RESULTS_LIMIT as usize);
        let mut keys: Vec<String> = Vec::with_capacity(page_size);
        let mut more_remain = false;
        let resume_from = match resume_after {
            Some(last) => Bound::Excluded(last),
            None => Bound::Unbounded,
        };
        for (arn, cluster) in self.clusters.range::<str, _>((resume_from, Bound::Unbounded)) {
            if cluster.region != region || !filter.keeps(cluster) {
                continue;
            }
            if keys.len() == page_size {
                more_remain = true;
                break;
            }
            keys.push(arn.clone());
        }
        let next_token = if more_remain {
            keys.last().map(|last| encode_cluster_page_token(last))
        } else {
            None
        };
        ClusterPage { keys, next_token }
    }
}
