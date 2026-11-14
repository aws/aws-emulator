// SPDX-License-Identifier: CC0-1.0

//! The NODES of a cluster, as its Kubernetes API server reports them.
//!
//! A cluster's nodes are not objects a client creates: they are the Amazon EC2
//! instances the cluster's managed node groups launched, which register
//! themselves with the cluster's API server as they come up. A node group IS "an
//! Amazon EC2 Auto Scaling group" ([`super::nodegroup_capacity`]), so the nodes
//! this module reports are the instances THOSE services hold — read back from
//! them when the node group's capacity is created and recorded on the node group
//! ([`NodegroupCapacity::nodes`]) — and never a count this service invents: a
//! cluster with no node group, or one whose capacity no Amazon EC2 service
//! created, reports no nodes.
//!
//! What each node reports about itself comes from the node group it belongs to:
//! the Kubernetes version its cluster runs, the instance type and Availability
//! Zone the instance really launched with, the capacity type the node group was
//! created with, and the labels and taints the node group applies to its nodes.

use super::*;

/// One node of a cluster: the Amazon EC2 instance a managed node group launched,
/// as the services that own it reported it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeInstance {
    /// The instance's id in Amazon EC2 (`i-…`), which is what the node's
    /// `spec.providerID` names and what makes the node resolvable in the service
    /// that owns it.
    pub instance_id: String,
    /// The instance's private DNS name, which is the name a node registers with
    /// its cluster under. Empty when EC2 reported none, in which case the node is
    /// named by its instance id — the other naming an Amazon EKS node registers
    /// under.
    pub private_dns_name: String,
    /// The Availability Zone the instance launched in, as the Auto Scaling group
    /// reported it.
    pub availability_zone: String,
    /// The instance type it launched as, as that group reported it.
    pub instance_type: String,
    /// Its private IPv4 address, as EC2 reported it, which is the node's
    /// `InternalIP` address.
    pub private_ip_address: String,
}

impl NodeInstance {
    /// The name this node registers with the cluster under: its private DNS name,
    /// or its instance id when EC2 published none.
    #[must_use]
    pub fn node_name(&self) -> &str {
        if self.private_dns_name.is_empty() {
            &self.instance_id
        } else {
            &self.private_dns_name
        }
    }
}

/// The node's `metadata.uid`.
///
/// A node is DERIVED from the instance behind it rather than stored, so the uid
/// is derived too — from that instance's id, through the one derivation every
/// UUID-shaped identifier of this service goes through — and every read of the
/// node reports the same value.
fn node_uid(instance_id: &str) -> String {
    derive_uuid_from_seed(&format!("node/{instance_id}"))
}

/// The `kubelet` version a node reports: the cluster's own Kubernetes version in
/// the form an Amazon EKS node publishes it, built from the version the cluster
/// record carries so a node and `DescribeCluster` cannot disagree about which
/// Kubernetes the cluster runs.
fn kubelet_version(cluster: &ClusterRecord) -> String {
    let version = cluster
        .version
        .clone()
        .unwrap_or_else(|| DEFAULT_KUBERNETES_VERSION.to_string());
    format!("v{version}.0-eks")
}

/// The processor architecture a node group's nodes run, read from the AMI type
/// the node group was created with: an `ARM` AMI runs `arm64` nodes, every other
/// published Amazon EKS AMI type runs `amd64` ones.
fn architecture(ami_type: &str) -> &'static str {
    if ami_type.contains("ARM") {
        "arm64"
    } else {
        "amd64"
    }
}

/// The operating system a node group's nodes run: every published Amazon EKS AMI
/// type whose name does not say Windows is a Linux one.
fn operating_system(ami_type: &str) -> &'static str {
    if ami_type.to_ascii_uppercase().contains("WINDOWS") {
        "windows"
    } else {
        "linux"
    }
}

/// One node of `nodegroup` as the `Node` object a Kubernetes client reads.
fn node_json(
    cluster: &ClusterRecord,
    region: &str,
    nodegroup: &NodegroupRecord,
    node: &NodeInstance,
) -> ::serde_json::Value {
    let name = node.node_name();
    let architecture = architecture(&nodegroup.ami_type);
    let operating_system = operating_system(&nodegroup.ami_type);
    // The labels every Amazon EKS node carries: the well-known Kubernetes ones
    // that name the node, its platform and its placement, and the Amazon EKS ones
    // that name the node group it belongs to. The node group's OWN labels are
    // applied over them, which is what "the Kubernetes labels applied to the
    // nodes in the node group" (`Nodegroup$labels`) means.
    let mut labels: BTreeMap<String, String> = [
        ("kubernetes.io/hostname", name.to_string()),
        ("kubernetes.io/os", operating_system.to_string()),
        ("kubernetes.io/arch", architecture.to_string()),
        ("beta.kubernetes.io/os", operating_system.to_string()),
        ("beta.kubernetes.io/arch", architecture.to_string()),
        (
            "node.kubernetes.io/instance-type",
            node.instance_type.clone(),
        ),
        (
            "beta.kubernetes.io/instance-type",
            node.instance_type.clone(),
        ),
        ("topology.kubernetes.io/region", region.to_string()),
        (
            "topology.kubernetes.io/zone",
            node.availability_zone.clone(),
        ),
        (
            "failure-domain.beta.kubernetes.io/region",
            region.to_string(),
        ),
        (
            "failure-domain.beta.kubernetes.io/zone",
            node.availability_zone.clone(),
        ),
        (
            "eks.amazonaws.com/nodegroup",
            nodegroup.nodegroup_name.clone(),
        ),
        (
            "eks.amazonaws.com/capacityType",
            nodegroup.capacity_type.clone(),
        ),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    labels.extend(
        nodegroup
            .labels
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    let taints: Vec<::serde_json::Value> = nodegroup
        .taints
        .iter()
        .map(|taint| {
            ::serde_json::json!({
                "key": taint.key,
                "value": taint.value,
                "effect": taint.effect,
            })
        })
        .collect();
    let mut addresses = vec![::serde_json::json!({
        "type": "Hostname",
        "address": name,
    })];
    if !node.private_ip_address.is_empty() {
        addresses.push(::serde_json::json!({
            "type": "InternalIP",
            "address": node.private_ip_address,
        }));
    }
    if !node.private_dns_name.is_empty() {
        addresses.push(::serde_json::json!({
            "type": "InternalDNS",
            "address": node.private_dns_name,
        }));
    }
    // A node of a node group that has come up is a node that joined: the node
    // group reports `ACTIVE` only once its nodes are registered with the cluster,
    // which is the condition `kubelet` posts.
    let joined_at = super::kubernetes_objects::rfc3339_seconds(nodegroup.created_at);
    ::serde_json::json!({
        "kind": "Node",
        "apiVersion": super::kubernetes_objects::CORE_API_VERSION,
        "metadata": {
            "name": name,
            "uid": node_uid(&node.instance_id),
            "creationTimestamp": joined_at,
            "labels": labels,
        },
        "spec": {
            // "aws:///<availability zone>/<instance id>": how the in-cluster AWS
            // cloud provider names the instance behind a node.
            "providerID": format!("aws:///{}/{}", node.availability_zone, node.instance_id),
            "taints": taints,
        },
        "status": {
            "conditions": [{
                "type": "Ready",
                "status": "True",
                "reason": "KubeletReady",
                "message": "kubelet is posting ready status",
                "lastHeartbeatTime": joined_at,
                "lastTransitionTime": joined_at,
            }],
            "addresses": addresses,
            "nodeInfo": {
                "architecture": architecture,
                "operatingSystem": operating_system,
                "kubeletVersion": kubelet_version(cluster),
                "kubeProxyVersion": kubelet_version(cluster),
                "systemUUID": node.instance_id,
            },
        },
    })
}

/// One node of a cluster, as the SCHEDULER needs it: the name it registered
/// under and the instant it joined the cluster.
///
/// Reported without rendering the node's whole document, because binding a pod to
/// a node needs neither the node's labels nor its conditions — and the instant is
/// what makes the pod's own start instant a value read from stored state rather
/// than from whichever call noticed it.
#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub name: String,
    pub joined_at: SystemTime,
}

impl ComAmazonawsEksAwswesleyfrontendState {
    /// Every node of the cluster `cluster_name` in `region`, in node-name order:
    /// the instances of each of its node groups that has come up.
    ///
    /// A node group that is still being created has not registered its nodes with
    /// the cluster yet, and one that is being deleted is taking them away, so
    /// neither contributes a node — the nodes a client sees are the nodes of the
    /// node groups the cluster reports as `ACTIVE`.
    pub fn cluster_nodes(
        &self,
        region: &str,
        cluster_name: &str,
    ) -> Vec<::serde_json::Value> {
        let Some(cluster) = self.clusters.get(&(region.to_string(), cluster_name.to_string()))
        else {
            return Vec::new();
        };
        let mut nodes: Vec<(String, ::serde_json::Value)> = Vec::new();
        for ((key_region, key_cluster, _), nodegroup) in &self.nodegroups {
            if key_region != region || key_cluster != cluster_name {
                continue;
            }
            if nodegroup.removed_at.is_some() || nodegroup.status != STATUS_ACTIVE {
                continue;
            }
            for node in &nodegroup.capacity.nodes {
                nodes.push((
                    node.node_name().to_string(),
                    node_json(cluster, region, nodegroup, node),
                ));
            }
        }
        nodes.sort_by(|left, right| left.0.cmp(&right.0));
        nodes.into_iter().map(|(_, document)| document).collect()
    }

    /// The nodes of that cluster a pod can be scheduled onto, in the order
    /// [`Self::cluster_nodes`] reports them: empty for a cluster whose node groups
    /// have registered none.
    ///
    /// Reads the node group records directly rather than the rendered documents:
    /// this runs on every request to the cluster's API server, and rendering every
    /// node's document to take a name and an instant off it is work no request
    /// needs.
    pub fn scheduler_nodes(&self, region: &str, cluster_name: &str) -> Vec<ClusterNode> {
        let mut nodes: Vec<ClusterNode> = self
            .nodegroups
            .iter()
            .filter(|((key_region, key_cluster, _), nodegroup)| {
                key_region == region
                    && key_cluster == cluster_name
                    && nodegroup.removed_at.is_none()
                    && nodegroup.status == STATUS_ACTIVE
            })
            .flat_map(|(_, nodegroup)| {
                nodegroup.capacity.nodes.iter().map(|node| ClusterNode {
                    name: node.node_name().to_string(),
                    // The instant the node joined the cluster, which is what the
                    // node itself reports as its `creationTimestamp`.
                    joined_at: nodegroup.created_at,
                })
            })
            .collect();
        nodes.sort_by(|left, right| left.name.cmp(&right.name));
        nodes
    }

    /// The node `name` of that cluster, for a read of one node.
    pub fn cluster_node(
        &self,
        region: &str,
        cluster_name: &str,
        name: &str,
    ) -> Option<::serde_json::Value> {
        self.cluster_nodes(region, cluster_name)
            .into_iter()
            .find(|node| {
                node.get("metadata")
                    .and_then(|metadata| metadata.get("name"))
                    .and_then(::serde_json::Value::as_str)
                    == Some(name)
            })
    }
}
