// SPDX-License-Identifier: CC0-1.0

//! The security groups the launch templates of an Auto Scaling group name, read against the
//! VPC of the subnets that group places its instances in.
//!
//! `VPCZoneIdentifier` is "a comma-separated list of subnet IDs for a virtual private cloud
//! (VPC) where instances in the Auto Scaling group can be created"
//! (`CreateAutoScalingGroup$VPCZoneIdentifier`), and a security group "belongs to one VPC"
//! — its `SecurityGroup$VpcId`. An instance of the group sits in one of those subnets, so a
//! security group of ANOTHER VPC could never be applied to it: the group could launch
//! nothing, and Amazon EC2 Auto Scaling refuses the request that would create or update it
//! instead of storing a configuration whose instances cannot exist.
//!
//! This module is the one place that cross-check happens, for every launch template a
//! request names ([`super::ec2_instances::specifications`]). Both resources live in Amazon
//! EC2's own store, reached through EC2's own accessor, so the VPC a subnet or a security
//! group is in is never read two ways.
//!
//! A reference the account's own Amazon EC2 state does not resolve — a template it holds no
//! version of, a security group id it holds no group for, a subnet it holds no subnet for,
//! or an engine carrying no Amazon EC2 store at all — is no evidence of a mismatch, and
//! nothing is refused for it: a refusal has to be grounded in what that state answered.
//!
//! LOCKING. The launch-template versions are read under Amazon EC2's store lock
//! ([`super::ec2_instances::launch_template_data`]), which is released before it is taken
//! again for the one pass that resolves the subnets and the groups; nothing awaits while it
//! is held. Amazon EC2 never reaches this service, so no cycle exists.

use std::collections::BTreeSet;

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::ops::types::{LaunchTemplateSpecification, MixedInstancesPolicy};

/// One security group a launch template names.
///
/// A template names a group by id — `RequestLaunchTemplateData$SecurityGroupIds` ("the IDs
/// of the security groups") and the `Groups` of each network interface it declares — or by
/// name, `$SecurityGroups` ("the names of the security groups"), exactly as a launch does,
/// so both are read here.
#[derive(Debug, Clone)]
enum GroupReference {
    /// `SecurityGroupIds`: the id of ONE group, which belongs to one VPC whatever VPC the
    /// reader is placed in.
    Id(String),
    /// `SecurityGroups`: a name, which is unique WITHIN a VPC, so it is resolved in the VPC
    /// the group's subnets are in.
    Name(String),
}

/// The refusal for a group whose launch template names a security group outside the VPC of
/// the subnets the group places instances in: HTTP 400 `ValidationError`, the code Amazon
/// EC2 Auto Scaling answers a request it cannot satisfy with.
///
/// The sentence is FIXED: it states that one or more of the template's security groups are
/// not in the network the group launches into, and names neither the groups, the subnets nor
/// the VPC — a template may carry several groups, and the service reports that at least one
/// of them cannot be applied rather than which.
fn foreign_security_group_error<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::validation_error(
        "One or more security groups in the launch template are not linked to the VPC that the \
         Auto Scaling group will launch instances in.",
    )
}

/// Every security group the launch templates of a launch source name, in the order the
/// request names them.
fn references<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Vec<GroupReference> {
    super::ec2_instances::specifications(launch_template, mixed_instances_policy)
        .into_iter()
        .filter_map(|specification| {
            super::ec2_instances::launch_template_data(engine, specification)
        })
        .flat_map(|data| {
            let ids = data
                .security_group_ids
                .into_iter()
                .flatten()
                .chain(
                    data.network_interfaces
                        .into_iter()
                        .flatten()
                        .flat_map(|interface| interface.groups.into_iter().flatten()),
                )
                .map(GroupReference::Id);
            let names = data
                .security_groups
                .into_iter()
                .flatten()
                .map(GroupReference::Name);
            ids.chain(names).collect::<Vec<GroupReference>>()
        })
        .filter(|reference| match reference {
            GroupReference::Id(value) | GroupReference::Name(value) => !value.is_empty(),
        })
        .collect()
}

/// The refusal for a request whose launch template names a security group outside the VPC
/// of the subnets the group places instances in; `None` when every group the template names
/// sits in that VPC, and when the account's own Amazon EC2 state resolves neither side.
///
/// `vpc_zone_identifier` is the group's subnets AFTER the request — a group that names none
/// places by Availability Zone alone, so it is in no VPC this service can cross-check and
/// nothing is refused.
pub fn cross_vpc_security_group_error<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    vpc_zone_identifier: Option<&str>,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    let subnets = super::ec2_subnets::subnet_ids(vpc_zone_identifier.unwrap_or_default());
    if subnets.is_empty() {
        return None;
    }
    let references = references(engine, launch_template, mixed_instances_policy);
    if references.is_empty() {
        return None;
    }
    let state = ec2::service_state(engine)?;
    let store = state.lock();
    // The VPC the group places instances in, read off its own subnets: the first subnet the
    // account holds, every subnet of one group being in one VPC. A request naming only
    // subnets the account does not hold is refused for those subnets by the handler's own
    // zone resolution, never here.
    let vpc_id = subnets.iter().find_map(|subnet_id| {
        store
            .subnets
            .get(*subnet_id)
            .and_then(|subnet| subnet.vpc_id.clone())
    })?;
    for reference in &references {
        let GroupReference::Id(group_id) = reference else {
            continue;
        };
        // A group the account holds none of is not evidence that this one is in another
        // VPC: there is no VPC to compare.
        let Some(group) = store.security_groups.get(group_id.as_str()) else {
            continue;
        };
        if group.vpc_id.as_deref() != Some(vpc_id.as_str()) {
            return Some(foreign_security_group_error());
        }
    }

    // A NAME is resolved in the VPC the group launches into, a name being unique within one
    // VPC. Every requested name is answered in ONE pass over the account's groups rather
    // than one pass per name, so this costs the SUM of the two collections rather than
    // their product.
    let requested: BTreeSet<&str> = references
        .iter()
        .filter_map(|reference| match reference {
            GroupReference::Name(group_name) => Some(group_name.as_str()),
            GroupReference::Id(_) => None,
        })
        .collect();
    if requested.is_empty() {
        return None;
    }
    let mut in_this_vpc: BTreeSet<&str> = BTreeSet::new();
    let mut in_another_vpc: BTreeSet<&str> = BTreeSet::new();
    for group in store.security_groups.values() {
        let Some(group_name) = group.group_name.as_deref() else {
            continue;
        };
        let Some(name) = requested.get(group_name) else {
            continue;
        };
        if group.vpc_id.as_deref() == Some(vpc_id.as_str()) {
            in_this_vpc.insert(name);
        } else {
            in_another_vpc.insert(name);
        }
    }
    // A name the account holds a group under in SOME other VPC and in none of this group's
    // names a group that is not in the network this group launches into — the same mismatch
    // an id states. A name the account holds nowhere resolves to nothing at all, which is no
    // evidence.
    if in_another_vpc.difference(&in_this_vpc).next().is_some() {
        return Some(foreign_security_group_error());
    }
    None
}
