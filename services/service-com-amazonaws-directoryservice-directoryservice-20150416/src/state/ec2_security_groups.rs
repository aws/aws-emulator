// SPDX-License-Identifier: CC0-1.0

//! THE HOP: minting the security group a directory's servers sit behind, in the service that
//! OWNS security groups.
//!
//! A directory this service places in a VPC does not sit on that network bare: Directory
//! Service provisions a security group for the servers it runs there and reports it back —
//! `DirectoryVpcSettingsDescription$SecurityGroupId` is "the domain controller security
//! group identifier for the directory" and `DirectoryConnectSettingsDescription$SecurityGroupId`
//! is "the security group identifier for the AD Connector directory". A client that has to
//! authorize traffic to the directory's servers (a trust between two directories in one VPC,
//! any reconciliation of the directory against EC2) works from that id, so the member is part
//! of the placement this service reports, not an optional extra.
//!
//! A security group is not a resource of this service: it is Amazon EC2's. So the group is
//! CREATED THERE, over the same in-process callback the subnets a directory is placed in are
//! resolved through ([`super::ec2_subnets`], whose client builder this module shares) — the
//! id this service then reports names a group the owner of security groups really holds and
//! answers `DescribeSecurityGroups` for. Nothing is minted here: an id this service invented
//! would name a group nothing could authorize a rule against.
//!
//! The group goes in the VPC the directory's servers really sit in, which is the VPC of the
//! subnets the create resolved ([`placement_vpc_id`]) rather than the `VpcId` the request
//! spelled: the subnets are what the servers are placed in, and EC2 is what says which
//! network they belong to. A create whose subnets the owner could not resolve therefore
//! provisions no group and the directory reports none, exactly as it reports no address for
//! a subnet the account does not have.
//!
//! A group provisioned FOR a directory's servers lives exactly as long as they do, so the
//! teardown `DeleteDirectory` starts releases it again ([`release_controllers_group`]) — in
//! the same owner, over the same hop — and the directory stops naming a group EC2 no longer
//! holds.

use std::collections::BTreeMap;

use super::directory_network::SubnetNetwork;
use super::ec2_subnets::ec2;

/// The suffix the name of a directory's AWS-provisioned security group carries after the
/// directory's own identifier.
///
/// Directory Service names the group it creates for a directory's servers
/// `<DirectoryId>_controllers` (the Directory Service Administration Guide's "what gets
/// created" for a directory placed in a VPC), so the group is recognisable from the
/// directory it belongs to and two directories in one VPC never collide on it — which is
/// what EC2 requires of a group name.
const CONTROLLERS_GROUP_SUFFIX: &str = "_controllers";

/// The name of the security group this service provisions for the directory `directory_id`.
#[must_use]
pub fn controllers_group_name(directory_id: &str) -> String {
    format!("{directory_id}{CONTROLLERS_GROUP_SUFFIX}")
}

/// The description that group carries: a sentence naming the service that created it and the
/// directory whose servers it is for.
#[must_use]
pub fn controllers_group_description(directory_id: &str) -> String {
    format!("AWS created security group for {directory_id} directory controllers")
}

/// Release the security group this service provisioned for a directory's servers: delete it
/// in the service that OWNS security groups, and report whether that owner no longer holds
/// it.
///
/// The group exists FOR the directory's servers — nothing else of the account was ever given
/// it — so the teardown that takes those servers away takes the group with them: a group left
/// behind would keep occupying the account's per-VPC group quota and keep answering
/// `DescribeSecurityGroups` for a directory that no longer exists. `group_id` is the id the
/// create recorded off EC2's own answer, so this deletes the group EC2 minted and never an id
/// of this service's own.
///
/// `false` when the owner still holds it — it could not be asked, or it refused the delete
/// because something of the account still references the group — and the directory then goes
/// on naming the group that is still there, because a report this service withdrew while the
/// group remained would be the same defect in the other direction.
///
/// Called with this service's store RELEASED, so no guard is held across the hop. ONE call,
/// with no caller-sized value in it.
pub async fn release_controllers_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_id: &str,
) -> bool {
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!(
            "the request names no Region label, so the security group of the directory's \
             servers cannot be released"
        );
        return false;
    };
    match client
        .delete_security_group()
        .group_id(group_id)
        .send()
        .await
    {
        Ok(_) => true,
        Err(error) => {
            // At `warn` for the same reason the create's hop logs one: a directory torn down
            // with its security group left behind in EC2 is worth seeing, and this IS why.
            ::tracing::warn!(
                %region,
                %group_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the security group of a directory's servers could not be deleted in EC2"
            );
            false
        }
    }
}

/// The VPC a directory placed in `subnet_networks` really sits in: the VPC the subnets the
/// create resolved belong to, as EC2 reports it.
///
/// `None` when no subnet of the placement resolved, or when the owner of subnets reports no
/// VPC for any of them — there is then no network to provision a group in, and nothing is
/// guessed from the request. The subnets of one placement are all in one VPC (a subnet
/// belongs to exactly one), so the first one that resolved answers for the placement.
#[must_use]
pub fn placement_vpc_id(subnet_networks: &BTreeMap<String, SubnetNetwork>) -> Option<String> {
    subnet_networks
        .values()
        .find_map(|network| network.vpc_id.clone())
}

/// Provision the security group the directory `directory_id`'s servers sit behind and report
/// the id EC2 minted for it, or `None` when there is no group to report.
///
/// `vpc_id` is the network the group goes in — [`placement_vpc_id`] of the subnets the create
/// resolved — so a create that could not resolve its placement asks for nothing at all.
///
/// Called by a create BEFORE it takes, or AFTER it has released, this service's store, so no
/// guard is held across the hop. ONE call, with no caller-sized value in it: the group's name
/// and description are built from the directory's own identifier.
///
/// A hop that could not be made is evidence of nothing and gates no create: the directory
/// keeps the placement the request named and simply reports no security group, rather than an
/// id that names no group anywhere.
pub async fn provision_controllers_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    directory_id: &str,
    vpc_id: Option<&str>,
) -> Option<String> {
    let vpc_id = vpc_id?;
    let client = ec2(ctx, region).or_else(|| {
        ::tracing::warn!(
            "the request names no Region label, so no security group can be created for the \
             directory's servers"
        );
        None
    })?;
    match client
        .create_security_group()
        .group_name(controllers_group_name(directory_id))
        .description(controllers_group_description(directory_id))
        .vpc_id(vpc_id)
        .send()
        .await
    {
        // The id EC2 minted, read off its own answer: this service reports the group the
        // owner of security groups holds and never a value of its own.
        Ok(output) => output.group_id,
        Err(error) => {
            // At `warn`: a directory whose servers no security group was created for reports
            // none, which is worth seeing, and this IS why.
            ::tracing::warn!(
                %region,
                %vpc_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the security group for a directory's servers could not be created in EC2"
            );
            None
        }
    }
}
