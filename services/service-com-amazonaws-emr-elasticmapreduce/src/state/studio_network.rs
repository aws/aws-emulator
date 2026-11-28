// SPDX-License-Identifier: CC0-1.0

//! The resources an EMR Studio is WIRED TO, which are not this service's.
//!
//! `CreateStudio` names an Amazon EC2 VPC, the subnets the Studio's Workspaces run in, the
//! two security groups they carry, and the IAM role the Studio assumes. None of those is a
//! resource of this service: the network objects live in Amazon EC2 and the role in IAM, and
//! they are resolved THERE — over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate. `service_common::network` states the contract this follows: Amazon EC2
//! is the ONE answer to which VPCs, subnets and security groups the account has.
//!
//! Every refusal built here is grounded in what the other service ANSWERED. An emulator
//! with no EC2 or no IAM mounted knows nothing about the network or the role, and nothing is
//! what it refuses on: a Studio GATES on existence and keeps nothing of the network, so
//! silence costs the create nothing to skip, and a refusal assumed from it would be
//! ungrounded.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// The AWS SDK for EC2 driven over the in-process callback into this same emulator.
///
/// The callback is the framework's seam for calling another emulated service: it routes in
/// process, so no network client is configured here. There is nothing to retry, time out or
/// watch for a stalled stream — each of those would need a sleep implementation this client
/// is not given — and the credentials the callback hands over are used directly rather than
/// cached.
pub(super) fn ec2_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    ec2_client_over(ctx.callback(), region)
}

/// The same client built over the callback ALONE, for a caller that is not inside a request:
/// the cluster clock pump outlives the request that started it and reaches Amazon EC2 over
/// the same callback ([`super::managed_security_groups`]).
pub(super) fn ec2_client_over(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// The prefix of every Amazon EC2 error code that says the account holds no VPC under an
/// identifier a read was given, whether because none carries it or because it is no VPC id
/// at all.
const INVALID_VPC_ID: &str = "InvalidVpcID.";

/// The same for a subnet identifier.
///
/// `pub(super)` because the same reading grounds the CLUSTER launch's own subnet resolution
/// ([`super::launch_validation`]): one prefix for what Amazon EC2 answers about a subnet it
/// holds none of, whichever of this service's resources named it.
pub(super) const INVALID_SUBNET_ID: &str = "InvalidSubnetID.";

/// The same for a security group identifier, which Amazon EC2 spells under TWO names —
/// `InvalidGroup.NotFound` for an identifier no group of the account carries, and
/// `InvalidGroupId.Malformed` for a value that is no group id at all — so the prefix stops
/// before the one letter that differs. Both say the same thing about a member: the value
/// resolves to no security group.
const INVALID_GROUP: &str = "InvalidGroup";

/// The service principal an EMR Studio's `ServiceRole` has to trust: the Studio "assumes"
/// that role (`CreateStudio$ServiceRole` doc), and what assumes it is Amazon EMR, so the
/// trust policy on the role has to admit the Amazon EMR service principal.
const STUDIO_SERVICE_PRINCIPAL: &str = "elasticmapreduce.amazonaws.com";

/// The input member a Studio names its VPC under.
pub const VPC_ID_MEMBER: &str = "VpcId";

/// The input member a Studio names the subnets its Workspaces run in under.
pub const SUBNET_IDS_MEMBER: &str = "SubnetIds";

/// What a network reference of a Studio that resolves to nothing usable is refused with.
///
/// RECORDED behavior for the text: AWS names the input MEMBER and states that it is not
/// valid, and does NOT echo the identifier the request carried, inside the same
/// service-detail wrapper every refusal this backend raises carries
/// ([`super::editors_refusal_message`]). The fixed text is the member name,
/// ` is not valid.` and that wrapper; its ONE per-run part is the request id of the
/// RESPONSE the sentence is carried by ([`super::REQUEST_ID_PLACEHOLDER`]), which the
/// response reports in its own header too, so an operator correlating the failure by the
/// id it read finds the call that was answered.
///
/// ONE template for every member, so what a caller reads off the sentence is which member
/// of its request to fix — and that is all that differs between the members, whether the
/// value is outside the form Amazon EC2 gives the resource, names nothing the account
/// holds, or (a subnet) names a resource of another VPC than the Studio's.
pub fn invalid_network_member_message(member: &str) -> String {
    super::editors_refusal_message(&format!("{member} is not valid."))
}

/// What a `ServiceRole` the Studio could not assume is refused with.
///
/// Recorded behavior for the text: the backend's own wrapper
/// ([`super::editors_refusal_message`]) in front of the statement that it has no
/// permission to assume the role. The one value taken from the request is the role it was
/// handed, which is what a caller has to fix.
pub fn unassumable_service_role_message(service_role: &str) -> String {
    super::editors_refusal_message(&format!(
        "This entity does not have permissions to assume role '{service_role}'"
    ))
}

/// The refusal the `VpcId` a Studio names earns, or `None` when Amazon EC2 reported that
/// VPC — and also when the hop could not be made, for the reason this module's docs give.
pub async fn unresolved_vpc(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Option<String> {
    let answer = ec2_client(ctx, region)
        .describe_vpcs()
        .vpc_ids(vpc_id)
        .send()
        .await;
    let absent = match &answer {
        // A read that resolved and reports no VPC under the id says the same thing the
        // refusal below does, about the same id.
        Ok(output) => (!output.vpcs().iter().any(|vpc| vpc.vpc_id() == Some(vpc_id)))
            .then(|| invalid_network_member_message(VPC_ID_MEMBER)),
        Err(error) => answered_absent(error, INVALID_VPC_ID, "VPC", vpc_id, VPC_ID_MEMBER),
    };
    absent
}

/// The refusal the `SubnetIds` of a Studio earn, by the rule [`unresolved_vpc`] states.
///
/// Each entry is resolved on its own, so the first entry Amazon EC2 answered about decides
/// the refusal — a read batching every entry is refused as a whole the moment one of them
/// does not resolve, which says nothing about WHICH.
///
/// A subnet also has to be one of the VPC the Studio is in — "The subnets must belong to
/// the VPC specified by VpcId" (`CreateStudio$SubnetIds` doc) — so when `vpc_id` names the
/// Studio's VPC, a subnet Amazon EC2 reports under ANOTHER VPC earns the same refusal as
/// one it reports nothing for: neither names a subnet this Studio could run in. `None`
/// asks only that the subnets exist, for a caller that has no VPC to hold them to.
pub async fn unresolved_subnets(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
    vpc_id: Option<&str>,
) -> Option<String> {
    let client = ec2_client(ctx, region);
    for subnet_id in subnet_ids {
        let answer = client
            .describe_subnets()
            .subnet_ids(subnet_id)
            .send()
            .await;
        let absent = match &answer {
            Ok(output) => {
                let reported = output
                    .subnets()
                    .iter()
                    .find(|subnet| subnet.subnet_id() == Some(subnet_id.as_str()));
                let unusable = match (reported, vpc_id) {
                    (None, _) => true,
                    // Amazon EC2 ANSWERED which VPC holds the subnet: a subnet of another
                    // VPC is not one this Studio may use. A subnet EC2 reported without a
                    // VPC says nothing about membership, so nothing is refused on it.
                    (Some(subnet), Some(vpc_id)) => {
                        subnet.vpc_id().is_some_and(|holder| holder != vpc_id)
                    }
                    (Some(_), None) => false,
                };
                unusable.then(|| invalid_network_member_message(SUBNET_IDS_MEMBER))
            }
            Err(error) => answered_absent(
                error,
                INVALID_SUBNET_ID,
                "subnet",
                subnet_id,
                SUBNET_IDS_MEMBER,
            ),
        };
        if let Some(message) = absent {
            return Some(message);
        }
    }
    None
}

/// The refusal the security groups a Studio names earn, by the rule [`unresolved_vpc`]
/// states, each resolved on its own for the reason [`unresolved_subnets`] gives. Each entry
/// is the member name the refusal has to state and the group id that member carried.
pub async fn unresolved_security_groups(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    groups: &[(&str, &str)],
) -> Option<String> {
    let client = ec2_client(ctx, region);
    for (member, group_id) in groups {
        let answer = client
            .describe_security_groups()
            .group_ids(*group_id)
            .send()
            .await;
        let absent = match &answer {
            Ok(output) => (!output
                .security_groups()
                .iter()
                .any(|group| group.group_id() == Some(*group_id)))
            .then(|| invalid_network_member_message(member)),
            Err(error) => {
                answered_absent(error, INVALID_GROUP, "security group", group_id, member)
            }
        };
        if let Some(message) = absent {
            return Some(message);
        }
    }
    None
}

/// The refusal the `ServiceRole` a Studio names earns, or `None` when IAM proved the
/// Studio may assume it — and also when there is no IAM to ask, for the reason this
/// module's docs give.
///
/// The role is the one the Studio RUNS AS, so the question is the tree's one reading of it
/// ([`service_common::iam_role::role_is_unassumable`]): IAM holds a role of this account
/// under the name the value carries, and the trust policy that role carries admits Amazon
/// EMR.
pub async fn unassumable_service_role(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    service_role: &str,
) -> Option<String> {
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        service_role,
        STUDIO_SERVICE_PRINCIPAL,
    )
    .await
    .then(|| unassumable_service_role_message(service_role))
}

/// The refusal a resource a Studio named earns when Amazon EC2 refused the read of it, or
/// `None` when that error is not EC2 ANSWERING that the account holds no such resource —
/// every code under `prefix` says that — but this emulator failing to ask it at all, which
/// refuses nothing and is traced instead.
///
/// What EC2 answered decides WHETHER the member is refused; the refusal itself is the
/// member's own ([`invalid_network_member_message`]), because that is the one sentence the
/// Studio backend states for a member that resolves to nothing usable, whatever the
/// service holding the resource said about it.
fn answered_absent<E: ProvideErrorMetadata>(
    error: &E,
    prefix: &str,
    kind: &str,
    requested: &str,
    member: &str,
) -> Option<String> {
    let code = error.code().unwrap_or_default();
    if code.starts_with(prefix) {
        return Some(invalid_network_member_message(member));
    }
    // The caller's own value is rendered in its escaped `Debug` form, so an identifier
    // carrying a newline cannot forge a log line.
    ::tracing::debug!(
        requested = ?requested,
        code = ?code,
        "the {kind} a Studio named could not be resolved against EC2"
    );
    None
}
