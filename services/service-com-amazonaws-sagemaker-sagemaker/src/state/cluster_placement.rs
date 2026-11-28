// SPDX-License-Identifier: CC0-1.0

//! WHERE a HyperPod cluster's nodes are launched, as the service that OWNS the network
//! reports it.
//!
//! A node of an instance group is launched into a SUBNET of the Amazon VPC configuration
//! that group runs under — the group's own `OverrideVpcConfig` when it declares one, and
//! otherwise the cluster's `VpcConfig` — so the Availability Zone a node reports is the zone
//! that subnet lives in. A subnet is not a resource of this service: it is Amazon EC2's, and
//! EC2 is the ONE answer to which zone holds a subnet and to how that zone's NAME
//! (`us-east-1a`) pairs with its account-independent ID (`use1-az1`). It is asked the way any
//! emulated service reaches another — the AWS SDK over the in-process callback, through this
//! crate's one EC2 client ([`super::domain_network::ec2`]) — so this crate names no other
//! service's crate and nothing leaves the process.
//!
//! # Resolved ONCE, at provisioning time
//!
//! The zones are resolved by the call that provisions the cluster and stored on it
//! ([`super::Cluster::group_placements`]), so every reader of a node — `DescribeClusterNode`'s
//! `Placement`, and the `AvailabilityZones` a `BatchAddClusterNodes` entry carries — reports
//! the same placement for the same node, and a reporting call resolves nothing of its own.
//!
//! # A group whose subnets resolve to nothing is still placed
//!
//! A cluster may name no VPC configuration at all, and a configuration may name a subnet this
//! account holds no record of. Such a group's instances still run somewhere — a cluster
//! created without a VPC configuration runs in a network the service manages — so the zone
//! reported for them is the Region's FIRST Availability Zone, read from EC2 with its ID where
//! EC2 answers and from the published zone names of the Region otherwise. What is never done
//! is reporting no placement for a node that is running: the zone is part of the node this
//! emulator provisioned, and a reader that cannot learn it cannot co-locate anything with the
//! node.

use std::collections::BTreeMap;

use crate::ops::types::{
    ClusterInstanceGroupSpecification, ClusterRestrictedInstanceGroupSpecification, VpcConfig,
};

/// The `DescribeSubnets` filter that selects subnets BY ID (`subnet-id`, listed by that
/// operation's own `Filters` documentation), so an identifier the account holds no subnet for
/// is an absent row rather than a failure of the whole read.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The most subnet identifiers ONE hop asks about, for the reason
/// [`super::domain_network`] gives: the owner evaluates a filter against every record the
/// account stores, so the cost of a hop is capped at a constant times what the account holds.
const IDS_PER_HOP: usize = 64;

/// One Availability Zone, by both identifiers AWS pairs: its account-scoped NAME and, where
/// the owning service stated it, its account-independent ID.
///
/// `ClusterInstancePlacement` reports exactly this pair (`AvailabilityZone` and
/// `AvailabilityZoneId`), and `NodeAdditionResult$AvailabilityZones` the names, so one record
/// answers both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterZone {
    /// The zone's name, inside `ClusterAvailabilityZone`'s modelled pattern.
    pub name: String,
    /// The zone's ID, where EC2 paired one with the name; `None` when it did not, since the
    /// pairing is the owning service's to state and is not derived here.
    pub id: Option<String>,
}

/// The subnets the instance group `group` runs under: its own `OverrideVpcConfig` when it
/// declares subnets, and otherwise the cluster's `VpcConfig`.
///
/// `OverrideVpcConfig` is documented as "the customized Amazon VPC configuration at the
/// instance group level that overrides the default Amazon VPC configuration of the SageMaker
/// HyperPod cluster", so a group that declares one is placed by it alone.
fn group_subnets<'a>(
    cluster_subnets: &'a [String],
    override_vpc_config: Option<&'a VpcConfig>,
) -> &'a [String] {
    match override_vpc_config {
        Some(config) if !config.subnets.is_empty() => config.subnets.as_slice(),
        _ => cluster_subnets,
    }
}

/// The zones each instance group of a cluster places its nodes in, keyed by the group's name.
///
/// Every group a create declares — flexible and restricted alike — gets an entry, so a node
/// minted for any of them has a placement to take. The zones are the ones EC2 reports for the
/// group's subnets, in the order the configuration lists them and each zone once; a group
/// whose subnets EC2 resolves none of is placed in the Region's first zone (see this module's
/// own documentation).
///
/// ONE hop resolves the subnets of every group at once, so the work does not grow with how
/// many groups name the same subnet. Call it with NO state guard held — it awaits.
pub async fn cluster_group_placements(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_config: Option<&VpcConfig>,
    instance_groups: &[ClusterInstanceGroupSpecification],
    restricted_instance_groups: &[ClusterRestrictedInstanceGroupSpecification],
) -> BTreeMap<String, Vec<ClusterZone>> {
    let cluster_subnets: &[String] = vpc_config
        .map(|config| config.subnets.as_slice())
        .unwrap_or_default();
    // The groups paired with the subnets each of them runs under, read off the request once.
    let declared: Vec<(&str, &[String])> = instance_groups
        .iter()
        .map(|group| {
            (
                group.instance_group_name.as_str(),
                group_subnets(cluster_subnets, group.override_vpc_config.as_ref()),
            )
        })
        .chain(restricted_instance_groups.iter().map(|group| {
            (
                group.instance_group_name.as_str(),
                group_subnets(cluster_subnets, group.override_vpc_config.as_ref()),
            )
        }))
        .collect();

    let mut named: Vec<&str> = Vec::new();
    for (_, subnets) in &declared {
        for subnet_id in *subnets {
            if !subnet_id.is_empty() && !named.contains(&subnet_id.as_str()) {
                named.push(subnet_id);
            }
        }
    }
    let held = subnet_zones(ctx, region, &named).await;

    let mut placements: BTreeMap<String, Vec<ClusterZone>> = BTreeMap::new();
    // Resolved lazily and once: a request every group of which named a subnet EC2 holds asks
    // nothing about the Region's zones.
    let mut fallback: Option<Vec<ClusterZone>> = None;
    for (group_name, subnets) in declared {
        let mut zones: Vec<ClusterZone> = Vec::new();
        for subnet_id in subnets {
            let Some(zone) = held.get(subnet_id.as_str()) else {
                continue;
            };
            if !zones.iter().any(|seen| seen.name == zone.name) {
                zones.push(zone.clone());
            }
        }
        if zones.is_empty() {
            if fallback.is_none() {
                fallback = Some(region_zones(ctx, region).await);
            }
            zones = fallback.iter().flatten().take(1).cloned().collect();
        }
        placements.insert(group_name.to_string(), zones);
    }
    placements
}

/// The zone EC2 reports for each of `subnet_ids` it holds a subnet for, by identifier.
///
/// An identifier EC2 reports nothing for, and one whose record carries no zone, is simply
/// absent from the map: EC2 stated no placement for it, and no placement is what this
/// answers.
async fn subnet_zones(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[&str],
) -> BTreeMap<String, ClusterZone> {
    let mut held: BTreeMap<String, ClusterZone> = BTreeMap::new();
    if subnet_ids.is_empty() {
        return held;
    }
    let Some(client) = super::domain_network::ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return held;
    };
    for chunk in subnet_ids.chunks(IDS_PER_HOP) {
        let by_id = ::aws_sdk_ec2::types::Filter::builder()
            .name(SUBNET_ID_FILTER)
            .set_values(Some(chunk.iter().map(|id| (*id).to_string()).collect()))
            .build();
        match client.describe_subnets().filters(by_id).send().await {
            Ok(output) => {
                for subnet in output.subnets() {
                    let (Some(subnet_id), Some(zone)) =
                        (subnet.subnet_id(), subnet.availability_zone())
                    else {
                        continue;
                    };
                    held.insert(
                        subnet_id.to_string(),
                        ClusterZone {
                            name: zone.to_string(),
                            id: subnet.availability_zone_id().map(str::to_string),
                        },
                    );
                }
            }
            Err(error) => {
                // At `warn`: a deployment where this hop fails is one where no cluster's
                // nodes can be placed by their subnets, and it IS why they are not.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the subnets a HyperPod cluster named could not be resolved against EC2"
                );
            }
        }
    }
    held
}

/// The Availability Zones of `region`, in the order they are published: EC2's own answer,
/// with the zone IDs it pairs, and the published zone NAMES of the Region when EC2 could not
/// be asked or answered none.
///
/// `service_common::aws_availability_zones` is the one place this tree states which zones a
/// Region exposes, so the fallback names no zone that Region does not publish.
async fn region_zones(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Vec<ClusterZone> {
    let published = || {
        ::service_common::aws_availability_zones::availability_zones(region)
            .into_iter()
            .map(|name| ClusterZone { name, id: None })
            .collect::<Vec<ClusterZone>>()
    };
    let Some(client) = super::domain_network::ec2(ctx, region) else {
        return published();
    };
    let answered = match client.describe_availability_zones().send().await {
        Ok(output) => output
            .availability_zones()
            .iter()
            .filter_map(|zone| {
                Some(ClusterZone {
                    name: zone.zone_name()?.to_string(),
                    id: zone.zone_id().map(str::to_string),
                })
            })
            .collect::<Vec<ClusterZone>>(),
        Err(error) => {
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the Availability Zones of the Region could not be resolved against EC2"
            );
            Vec::new()
        }
    };
    if answered.is_empty() {
        return published();
    }
    answered
}

#[cfg(test)]
mod tests {
    use super::group_subnets;
    use crate::ops::types::VpcConfig;

    /// RULE: an instance group is placed by its OWN VPC configuration when that declares
    /// subnets, and by the cluster's otherwise.
    #[test]
    fn a_groups_subnets_are_its_override_when_it_declares_any() {
        let cluster = vec!["subnet-cluster-1".to_string(), "subnet-cluster-2".to_string()];
        let overridden = VpcConfig {
            security_group_ids: Vec::new(),
            subnets: vec!["subnet-group-1".to_string()],
        };
        let empty_override = VpcConfig::default();
        for (case, config, expected) in [
            ("no override at all", None, cluster.as_slice()),
            (
                "an override naming no subnet",
                Some(&empty_override),
                cluster.as_slice(),
            ),
            (
                "an override naming one subnet",
                Some(&overridden),
                overridden.subnets.as_slice(),
            ),
        ] {
            let resolved = group_subnets(&cluster, config);
            assert!(
                resolved == expected,
                "case {case:?}: expected the subnets {expected:?}, got {resolved:?}"
            );
        }
    }
}
