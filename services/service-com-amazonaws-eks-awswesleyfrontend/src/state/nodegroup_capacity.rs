// SPDX-License-Identifier: CC0-1.0

//! The capacity a managed node group runs on, in the services that own it.
//!
//! A managed node group is not a pool of nodes Amazon EKS keeps itself: it is an
//! Amazon EC2 Auto Scaling group that EKS creates and manages on the account's
//! behalf, launching from a launch template of its own — which is why a node
//! group REPORTS those resources ("the resources associated with the node group,
//! such as Auto Scaling groups and security groups for remote access",
//! `Nodegroup$resources`) and why a client scales or inspects its nodes through
//! them.
//!
//! So this service CREATES them where they live — the launch template and the
//! remote-access security group in Amazon EC2, the group in Amazon EC2 Auto
//! Scaling — over the same in-process callback every other cross-service reading
//! here goes through, and releases them when the node group goes. A node group
//! reports only what those services really hold: a resource one of them did not
//! create is not reported, rather than named with an id nothing would resolve.

use super::*;
// The two shapes a node group's `resources` member is made of. They are not among
// the types `super`'s own imports bring in, so this module names them itself.
use crate::ops::types::{AutoScalingGroup, NodegroupResources};

/// The resources of other services that a managed node group's capacity is made
/// of, as this service records them.
///
/// Empty for a node group whose capacity could not be created — an emulator with
/// no Amazon EC2 or no Auto Scaling mounted — which is the same silence every
/// other reading in this neighbourhood treats as nothing: the node group reports
/// no resource rather than one that does not exist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodegroupCapacity {
    /// The Auto Scaling groups that back the node group, by name — what
    /// `Nodegroup$resources.autoScalingGroups` reports. A managed node group is
    /// backed by exactly one, so this holds one name or none.
    pub auto_scaling_group_names: Vec<String>,
    /// The id of the launch template the Auto Scaling group launches from. Not
    /// reported by any member of `Nodegroup`: it is held so the delete can
    /// release it.
    pub launch_template_id: Option<String>,
    /// The security group Amazon EKS created for remote access to the nodes —
    /// `Nodegroup$resources.remoteAccessSecurityGroup` — which exists only for a
    /// node group whose `remoteAccess` asked for that access.
    pub remote_access_security_group_id: Option<String>,
    /// The instances the Auto Scaling group launched, as that group and Amazon
    /// EC2 reported them. Not reported by any member of `Nodegroup`: they are
    /// held because they are the cluster's NODES, which its Kubernetes API server
    /// reports ([`super::kubernetes_nodes`]). Empty for a node group whose
    /// capacity no such service created, so a cluster never reports a node no
    /// service holds.
    pub nodes: Vec<NodeInstance>,
}

impl NodegroupCapacity {
    /// This capacity as the modelled `NodegroupResources` shape, or `None` when
    /// there is nothing to report.
    pub fn to_nodegroup_resources(&self) -> Option<NodegroupResources> {
        if self.auto_scaling_group_names.is_empty()
            && self.remote_access_security_group_id.is_none()
        {
            return None;
        }
        Some(NodegroupResources {
            auto_scaling_groups: Some(
                self.auto_scaling_group_names
                    .iter()
                    .map(|name| AutoScalingGroup {
                        name: Some(name.clone()),
                    })
                    .collect(),
            ),
            remote_access_security_group: self.remote_access_security_group_id.clone(),
        })
    }
}

/// What a node group's capacity is created FROM: the node group's own identity
/// and the configuration its nodes launch under.
///
/// A plain borrowed struct rather than a long argument list, so the call reads as
/// the node group it provisions for.
#[derive(Debug, Clone, Copy)]
pub struct NodegroupCapacitySpec<'a> {
    /// The cluster the node group belongs to, which tags every resource created
    /// here as the cluster's.
    pub cluster_name: &'a str,
    /// The node group's own name.
    pub nodegroup_name: &'a str,
    /// The node group's stable derived id ([`derive_nodegroup_id`]), which makes
    /// every name minted here unique per node group without being drawn from the
    /// request.
    pub nodegroup_id: &'a str,
    /// The subnets the group launches nodes into: "the subnets to use for the
    /// Auto Scaling group that is created for your node group"
    /// (`CreateNodegroupRequest$subnets`).
    pub subnets: &'a [String],
    /// The VPC those subnets sit in, as EC2 reported it, for the security group
    /// created here. `None` when EC2 did not agree on one, in which case no
    /// group is created.
    pub vpc_id: Option<&'a str>,
    /// The sizes the group is created with, from the node group's effective
    /// `scalingConfig`.
    pub min_size: i32,
    /// See [`Self::min_size`].
    pub max_size: i32,
    /// See [`Self::min_size`].
    pub desired_size: i32,
    /// The instance type the nodes launch as, from the node group's effective
    /// `instanceTypes`. `None` for a node group deployed with the caller's own
    /// launch template, which is what carries the type then
    /// (`Nodegroup$instanceTypes`).
    pub instance_type: Option<&'a str>,
    /// The EC2 key pair the nodes are launched with: "the Amazon EC2 SSH key name
    /// that provides access for SSH communication with the nodes"
    /// (`RemoteAccessConfig$ec2SshKey`). `None` when the node group asked for no
    /// remote access, which is also when no remote-access security group is
    /// created.
    pub ec2_ssh_key: Option<&'a str>,
}

/// The tag Amazon EKS stamps on the capacity of a node group, naming the cluster
/// it belongs to. It sits in AWS's own `eks:` namespace, so no caller sets it.
pub const NODEGROUP_CLUSTER_NAME_TAG: &str = "eks:cluster-name";

/// The same, naming the node group itself.
pub const NODEGROUP_NAME_TAG: &str = "eks:nodegroup-name";

/// How many of a node group's instances this service records as the cluster's
/// nodes.
///
/// A node group's sizes come from the caller's `scalingConfig`, so the number of
/// instances its Auto Scaling group launches is caller-chosen; what THIS service
/// retains for them must not be. The ceiling is CHOSEN rather than published: it
/// is above the handful of nodes a cluster of this emulator is driven with, and it
/// bounds both the records held per node group and the one read-back this service
/// makes about them.
pub const MAX_RECORDED_NODES_PER_NODEGROUP: usize = 16;

/// The name of the Auto Scaling group that backs the node group
/// `nodegroup_name`, derived from the node group's own stable id so it is
/// distinct per node group and the same on every read.
pub fn nodegroup_auto_scaling_group_name(nodegroup_name: &str, nodegroup_id: &str) -> String {
    format!("eks-{nodegroup_name}-{nodegroup_id}")
}

/// The name of the launch template that group launches from.
pub fn nodegroup_launch_template_name(nodegroup_id: &str) -> String {
    format!("eks-{nodegroup_id}")
}

/// The name of the security group remote access to the node group's nodes is
/// authorised through.
pub fn nodegroup_remote_access_security_group_name(nodegroup_id: &str) -> String {
    format!("eks-remoteAccess-{nodegroup_id}")
}

/// Create the capacity a managed node group runs on, and report what was
/// created.
///
/// Called with NO lock on this service's store held: every step is a call out to
/// another service. A step whose service did not answer leaves its resource out
/// of the result and is reported in a warning — the node group itself still
/// exists, exactly as a cluster whose security group EC2 did not create still
/// exists ([`mint_cluster_security_group`]).
pub async fn provision_nodegroup_capacity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    spec: NodegroupCapacitySpec<'_>,
) -> NodegroupCapacity {
    let mut capacity = NodegroupCapacity::default();
    let Some(ec2_endpoint) = peer_endpoint("ec2", region) else {
        return capacity;
    };
    let ec2 = callback_client!(aws_sdk_ec2, ctx, ec2_endpoint, region);

    // The remote-access security group FIRST, because the launch template the
    // nodes come up under is what authorises them against it: it exists only for
    // a node group that asked for remote access.
    if let (Some(ssh_key), Some(vpc_id)) = (spec.ec2_ssh_key, spec.vpc_id) {
        let _ = ssh_key;
        let group_name = nodegroup_remote_access_security_group_name(spec.nodegroup_id);
        let created = ec2
            .create_security_group()
            .group_name(&group_name)
            .description(format!(
                "Security group for remote access to the nodes of node group {} of cluster {}",
                spec.nodegroup_name, spec.cluster_name
            ))
            .vpc_id(vpc_id)
            .send()
            .await;
        match created {
            Ok(output) => capacity.remote_access_security_group_id = output.group_id,
            Err(error) => ::tracing::warn!(
                nodegroup = %spec.nodegroup_name,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the node group's remote-access security group could not be created in EC2"
            ),
        }
    }

    // The launch template the group launches its nodes from: the instance type and
    // the key pair the node group was created with, and the remote-access group
    // when there is one.
    let mut data = ::aws_sdk_ec2::types::RequestLaunchTemplateData::builder();
    if let Some(instance_type) = spec.instance_type {
        data = data.instance_type(::aws_sdk_ec2::types::InstanceType::from(instance_type));
    }
    if let Some(ssh_key) = spec.ec2_ssh_key {
        data = data.key_name(ssh_key);
    }
    if let Some(group_id) = capacity.remote_access_security_group_id.as_deref() {
        data = data.security_group_ids(group_id);
    }
    let template_name = nodegroup_launch_template_name(spec.nodegroup_id);
    let template = ec2
        .create_launch_template()
        .launch_template_name(&template_name)
        .launch_template_data(data.build())
        .send()
        .await;
    match template {
        Ok(output) => {
            capacity.launch_template_id = output
                .launch_template
                .and_then(|template| template.launch_template_id)
        }
        Err(error) => ::tracing::warn!(
            nodegroup = %spec.nodegroup_name,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the node group's launch template could not be created in EC2"
        ),
    }

    // And the group itself. It launches from the template above, so a node group
    // whose template was not created gets no group either: a group without a
    // launch source would launch nothing.
    let Some(launch_template_id) = capacity.launch_template_id.clone() else {
        return capacity;
    };
    let Some(autoscaling_endpoint) = peer_endpoint("autoscaling", region) else {
        return capacity;
    };
    let group_name = nodegroup_auto_scaling_group_name(spec.nodegroup_name, spec.nodegroup_id);
    let tags: Vec<::aws_sdk_autoscaling::types::Tag> = [
        (
            format!("{KUBERNETES_CLUSTER_TAG_PREFIX}{}", spec.cluster_name),
            KUBERNETES_CLUSTER_OWNED.to_string(),
        ),
        (
            NODEGROUP_CLUSTER_NAME_TAG.to_string(),
            spec.cluster_name.to_string(),
        ),
        (
            NODEGROUP_NAME_TAG.to_string(),
            spec.nodegroup_name.to_string(),
        ),
    ]
    .into_iter()
    .map(|(key, value)| {
        ::aws_sdk_autoscaling::types::Tag::builder()
            .key(key)
            .value(value)
            // The nodes the group launches carry the cluster's tags too: that is
            // how the in-cluster AWS cloud provider finds them.
            .propagate_at_launch(true)
            .build()
    })
    .collect();
    let created = callback_client!(aws_sdk_autoscaling, ctx, autoscaling_endpoint, region)
        .create_auto_scaling_group()
        .auto_scaling_group_name(&group_name)
        .launch_template(
            ::aws_sdk_autoscaling::types::LaunchTemplateSpecification::builder()
                .launch_template_id(launch_template_id)
                .build(),
        )
        .min_size(spec.min_size)
        .max_size(spec.max_size)
        .desired_capacity(spec.desired_size)
        .vpc_zone_identifier(spec.subnets.join(","))
        .set_tags(Some(tags))
        .send()
        .await;
    match created {
        Ok(_) => capacity.auto_scaling_group_names.push(group_name.clone()),
        Err(error) => ::tracing::warn!(
            nodegroup = %spec.nodegroup_name,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the node group's Auto Scaling group could not be created"
        ),
    }
    // The instances that group launched are the cluster's NODES, so they are read
    // back from the services that own them: a node this service reports is an
    // instance Amazon EC2 Auto Scaling and Amazon EC2 really hold.
    if !capacity.auto_scaling_group_names.is_empty() {
        capacity.nodes = launched_nodes(ctx, region, &group_name).await;
    }
    capacity
}

/// The instances the Auto Scaling group `group_name` launched, as the two
/// services that own them report them: the group names its instances and the
/// Availability Zone and instance type each launched with, and Amazon EC2 names
/// each instance's private DNS name and address.
///
/// An instance neither service reports is not a node: the list is whatever they
/// answered, and empty when either could not be asked. At most
/// [`MAX_RECORDED_NODES_PER_NODEGROUP`] of them are taken, so a node group whose
/// caller-chosen sizes launched more does not grow this service's state — nor the
/// one read-back below — by a number the caller picked.
async fn launched_nodes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_name: &str,
) -> Vec<NodeInstance> {
    let Some(endpoint) = peer_endpoint("autoscaling", region) else {
        return Vec::new();
    };
    let described = callback_client!(aws_sdk_autoscaling, ctx, endpoint, region)
        .describe_auto_scaling_groups()
        .auto_scaling_group_names(group_name)
        .send()
        .await;
    let groups = match described {
        Ok(output) => output.auto_scaling_groups,
        Err(error) => {
            ::tracing::warn!(
                %group_name,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the node group's launched instances could not be read from Auto Scaling"
            );
            return Vec::new();
        }
    };
    let mut nodes: Vec<NodeInstance> = groups
        .into_iter()
        .flatten()
        .flat_map(|group| group.instances.unwrap_or_default())
        .map(|instance| NodeInstance {
            instance_id: instance.instance_id.unwrap_or_default(),
            availability_zone: instance.availability_zone.unwrap_or_default(),
            instance_type: instance.instance_type.unwrap_or_default(),
            ..NodeInstance::default()
        })
        .filter(|node| !node.instance_id.is_empty())
        .take(MAX_RECORDED_NODES_PER_NODEGROUP)
        .collect();
    if nodes.is_empty() {
        return nodes;
    }
    let Some(endpoint) = peer_endpoint("ec2", region) else {
        return nodes;
    };
    let instance_ids: Vec<String> = nodes.iter().map(|node| node.instance_id.clone()).collect();
    let described = callback_client!(aws_sdk_ec2, ctx, endpoint, region)
        .describe_instances()
        .set_instance_ids(Some(instance_ids))
        .send()
        .await;
    match described {
        Ok(output) => {
            for instance in output
                .reservations
                .unwrap_or_default()
                .into_iter()
                .flat_map(|reservation| reservation.instances.unwrap_or_default())
            {
                let Some(id) = instance.instance_id.as_deref() else {
                    continue;
                };
                let Some(node) = nodes.iter_mut().find(|node| node.instance_id == id) else {
                    continue;
                };
                node.private_dns_name = instance.private_dns_name.clone().unwrap_or_default();
                node.private_ip_address =
                    instance.private_ip_address.clone().unwrap_or_default();
            }
        }
        Err(error) => ::tracing::warn!(
            %group_name,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the node group's instances could not be read from EC2"
        ),
    }
    nodes
}

/// Release the capacity a deleted node group ran on.
///
/// Nothing holds any of it once the node group is gone, so the group is deleted
/// with its instances ("specifies that the group is to be deleted along with all
/// instances associated with the group", `DeleteAutoScalingGroup$ForceDelete`),
/// then the template it launched from and the security group that authorised
/// remote access to it. A resource another service would not delete is reported
/// rather than refused: the node group has already left this service's state, and
/// `DeleteNodegroup` has no failure to answer the caller with for a resource of
/// another service.
pub async fn release_nodegroup_capacity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    capacity: &NodegroupCapacity,
) {
    if capacity == &NodegroupCapacity::default() {
        return;
    }
    if let Some(endpoint) = peer_endpoint("autoscaling", region) {
        let autoscaling = callback_client!(aws_sdk_autoscaling, ctx, endpoint, region);
        for group_name in &capacity.auto_scaling_group_names {
            let released = autoscaling
                .delete_auto_scaling_group()
                .auto_scaling_group_name(group_name)
                .force_delete(true)
                .send()
                .await;
            if let Err(error) = released {
                ::tracing::warn!(
                    %group_name,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the node group's Auto Scaling group could not be deleted"
                );
            }
        }
    }
    let Some(endpoint) = peer_endpoint("ec2", region) else {
        return;
    };
    let ec2 = callback_client!(aws_sdk_ec2, ctx, endpoint, region);
    if let Some(launch_template_id) = capacity.launch_template_id.as_deref() {
        let released = ec2
            .delete_launch_template()
            .launch_template_id(launch_template_id)
            .send()
            .await;
        if let Err(error) = released {
            ::tracing::warn!(
                %launch_template_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the node group's launch template could not be deleted from EC2"
            );
        }
    }
    if let Some(group_id) = capacity.remote_access_security_group_id.as_deref() {
        let released = ec2.delete_security_group().group_id(group_id).send().await;
        if let Err(error) = released {
            ::tracing::warn!(
                %group_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the node group's remote-access security group could not be deleted from EC2"
            );
        }
    }
}
