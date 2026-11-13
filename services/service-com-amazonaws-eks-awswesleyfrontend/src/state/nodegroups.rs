// SPDX-License-Identifier: CC0-1.0

//! The managed node group slice of this service's state: the record the store
//! keeps for one, the configuration an update holds until it is due, and the
//! projections a read answers with.

use super::*;

/// One managed node group, as this service stores it.
///
/// Everything a `Nodegroup` reports is held here — including the
/// server-generated `nodegroupArn`, `createdAt`/`modifiedAt` and every default
/// `CreateNodegroup` filled in — so `DescribeNodegroup` returns exactly what the
/// create wrote rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct NodegroupRecord {
    pub nodegroup_name: String,
    pub cluster_name: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    pub version: String,
    /// Set only when the caller named a `releaseVersion`: this emulator
    /// publishes no EKS-optimized AMI catalog, so for a node group that named
    /// none it reports no AMI version rather than inventing one.
    pub release_version: Option<String>,
    pub status: String,
    /// When set, the instant at which the clock owes this node group the move
    /// from `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    pub capacity_type: String,
    pub scaling_config: NodegroupScalingConfig,
    /// `None` when the node group was deployed with a launch template: "If the
    /// node group was deployed with a launch template, then this is `null`"
    /// (`Nodegroup$instanceTypes`).
    pub instance_types: Option<Vec<String>>,
    pub subnets: Vec<String>,
    /// `None` when the node group was deployed with a launch template, for the
    /// same reason as [`NodegroupRecord::instance_types`]
    /// (`Nodegroup$remoteAccess`).
    pub remote_access: Option<RemoteAccessConfig>,
    pub ami_type: String,
    pub node_role: String,
    pub labels: BTreeMap<String, String>,
    pub taints: Vec<Taint>,
    /// `None` when the node group was deployed with a launch template, for the
    /// same reason as [`NodegroupRecord::instance_types`] (`Nodegroup$diskSize`).
    pub disk_size: Option<i32>,
    pub update_config: NodegroupUpdateConfig,
    pub node_repair_config: Option<NodeRepairConfig>,
    pub launch_template: Option<LaunchTemplateSpecification>,
    pub tags: BTreeMap<String, String>,
    /// The capacity this node group runs on, as the services that own it created
    /// it: the Auto Scaling group that backs the node group, the launch template
    /// it launches from, and the remote-access security group when the node group
    /// asked for that access ([`NodegroupCapacity`]). Written once by the create
    /// and reported as `Nodegroup$resources`.
    pub capacity: NodegroupCapacity,
    pub warm_pool_config: Option<WarmPoolConfig>,
    pub client_request_token: Option<String>,
    /// The change the asynchronous update this node group is taking puts on it
    /// when the clock completes that update; `None` when it is taking none.
    ///
    /// The update itself is a record on the cluster ([`ClusterRecord::updates`]),
    /// which is where its own due instant and status live; this is the part of it
    /// that belongs to the node group, applied at [`Self::active_at`] — the
    /// instant the node group is owed its `ACTIVE` status back, which is the
    /// instant that update is owed its `Successful` one.
    pub pending_config: Option<PendingNodegroupConfig>,
}

/// The change an in-flight node group update will put on its node group when it
/// completes.
///
/// Both operations that update a node group are asynchronous — `UpdateNodegroupConfig`
/// ("you can update... the Kubernetes labels... and the scaling and version update
/// configuration") and `UpdateNodegroupVersion` ("updates the Kubernetes version or
/// AMI version"), each answering with an `Update` rather than with the node group —
/// so the values the request asked for are held HERE until the clock completes that
/// update, and only then written onto the [`NodegroupRecord`]. A member left `None`
/// (or an empty label/taint change) is one the request did not ask to change.
#[derive(Debug, Clone, Default)]
pub struct PendingNodegroupConfig {
    /// The id of the update that carries this change, so a caller tracking the
    /// update and the node group it changes sees one resource, not two copies.
    pub update_id: String,
    /// The Kubernetes labels to add or replace, by key.
    pub add_or_update_labels: BTreeMap<String, String>,
    /// The Kubernetes label keys to remove.
    pub remove_labels: Vec<String>,
    /// The taints to add or replace, matched on the `(key, effect)` pair a taint
    /// is identified by.
    pub add_or_update_taints: Vec<Taint>,
    /// The taints to remove, matched the same way.
    pub remove_taints: Vec<Taint>,
    pub scaling_config: Option<NodegroupScalingConfig>,
    pub update_config: Option<NodegroupUpdateConfig>,
    pub node_repair_config: Option<NodeRepairConfig>,
    pub warm_pool_config: Option<WarmPoolConfig>,
    /// The Kubernetes version an `UpdateNodegroupVersion` update moves the node
    /// group to — "the Kubernetes version to update to"
    /// (`UpdateNodegroupVersionRequest$version`) — so the node group reports the
    /// new version only once the clock completes that update.
    pub version: Option<String>,
    /// The AMI version that same update moves it to: "the AMI version of the
    /// Amazon EKS optimized AMI to use for the update" (`$releaseVersion`).
    pub release_version: Option<String>,
}

/// Whether two taints name the same taint of a node group: "The key of the
/// taint" and "the effect of the taint" are what a node group's scheduler keys
/// on, so a taint added for a `(key, effect)` pair replaces the one already
/// there and the `value` is what the change carries.
pub(super) fn same_taint(left: &Taint, right: &Taint) -> bool {
    left.key == right.key && left.effect == right.effect
}

impl PendingNodegroupConfig {
    /// Whether this change asks for anything at all — what tells a request that
    /// names no member from one that asks for a change.
    pub fn is_empty(&self) -> bool {
        self.add_or_update_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.add_or_update_taints.is_empty()
            && self.remove_taints.is_empty()
            && self.scaling_config.is_none()
            && self.update_config.is_none()
            && self.node_repair_config.is_none()
            && self.warm_pool_config.is_none()
            && self.version.is_none()
            && self.release_version.is_none()
    }

    /// Write this change onto `record`, which is what completing the update
    /// does. `at` is the instant the update was owed its completion, so
    /// `modifiedAt` is the same value whenever the read that materialises it
    /// comes.
    pub(super) fn apply(self, record: &mut NodegroupRecord, at: SystemTime) {
        for (key, value) in self.add_or_update_labels {
            record.labels.insert(key, value);
        }
        for key in &self.remove_labels {
            record.labels.remove(key);
        }
        for requested in self.add_or_update_taints {
            match record
                .taints
                .iter_mut()
                .find(|taint| same_taint(taint, &requested))
            {
                Some(existing) => existing.value = requested.value.clone(),
                None => record.taints.push(requested),
            }
        }
        record
            .taints
            .retain(|taint| !self.remove_taints.iter().any(|removed| same_taint(taint, removed)));
        if let Some(scaling_config) = self.scaling_config {
            record.scaling_config = scaling_config;
        }
        if let Some(update_config) = self.update_config {
            record.update_config = update_config;
        }
        if let Some(node_repair_config) = self.node_repair_config {
            record.node_repair_config = Some(node_repair_config);
        }
        if let Some(warm_pool_config) = self.warm_pool_config {
            record.warm_pool_config = Some(warm_pool_config);
        }
        if let Some(version) = self.version {
            record.version = version;
        }
        if let Some(release_version) = self.release_version {
            record.release_version = Some(release_version);
        }
        record.modified_at = at;
    }
}

impl NodegroupRecord {
    /// Materialise the status move the clock already owes at `now`, and write the
    /// change an update owed this record at the instant it was owed. See
    /// [`ClusterRecord::materialize_due_transitions`].
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        let Some(due) = self.active_at.filter(|due| now >= *due) else {
            return;
        };
        self.active_at = None;
        if let Some(pending) = self.pending_config.take() {
            pending.apply(self, due);
        }
        self.status = STATUS_ACTIVE.to_string();
    }

    /// This record as the modelled `Nodegroup` shape.
    pub fn to_nodegroup(&self) -> Nodegroup {
        Nodegroup {
            nodegroup_name: Some(self.nodegroup_name.clone()),
            nodegroup_arn: Some(self.arn.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            version: Some(self.version.clone()),
            release_version: self.release_version.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            status: Some(self.status.clone()),
            capacity_type: Some(self.capacity_type.clone()),
            scaling_config: Some(self.scaling_config.clone()),
            instance_types: self.instance_types.clone(),
            subnets: Some(self.subnets.clone()),
            remote_access: self.remote_access.clone(),
            ami_type: Some(self.ami_type.clone()),
            node_role: Some(self.node_role.clone()),
            labels: Some(
                self.labels
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            taints: Some(self.taints.clone()),
            disk_size: self.disk_size,
            // Nothing in this emulator reports a node group issue, so the
            // health block is present and empty rather than absent: "Any issues
            // that are associated with the node group" (`NodegroupHealth$issues`).
            health: Some(NodegroupHealth {
                issues: Some(Vec::new()),
            }),
            update_config: Some(self.update_config.clone()),
            node_repair_config: self.node_repair_config.clone(),
            launch_template: self.launch_template.clone(),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            warm_pool_config: self.warm_pool_config.clone(),
            // `resources` names the Auto Scaling groups and the remote-access
            // security group Amazon EKS creates for the node group. They are the
            // resources the create really made in Auto Scaling and EC2, read off
            // the record rather than recomputed — and `None` when neither service
            // created any, so nothing is reported that would resolve to nothing.
            resources: self.capacity.to_nodegroup_resources(),
        }
    }
}

/// Why the capacities a `scalingConfig` change would leave the node group with
/// are not ones it can scale between, or `None` when they are.
///
/// "The Cluster Autoscaler doesn't scale a managed node group lower than
/// `minSize` or higher than `maxSize`" (`NodegroupScalingConfig$desiredSize`), so
/// the desired size has to sit between the two bounds — and the refusal names the
/// BOUND the desired size falls outside of, with the capacities themselves, the
/// way Amazon EKS words each one. Each member's own lower bound is modelled and
/// so already refused before a handler asks this.
///
/// The two relations are the whole rule: a `minSize` above `maxSize` leaves no
/// desired size between them, so such a triple always falls outside one of the
/// two bounds and is reported as that bound.
pub fn scaling_capacity_violation(
    min_size: i32,
    max_size: i32,
    desired_size: i32,
) -> Option<String> {
    if desired_size < min_size {
        return Some(format!(
            "Minimum capacity {min_size} can't be greater than desired size {desired_size}"
        ));
    }
    if desired_size > max_size {
        return Some(format!(
            "Maximum capacity {max_size} can't be less than desired size {desired_size}"
        ));
    }
    None
}
