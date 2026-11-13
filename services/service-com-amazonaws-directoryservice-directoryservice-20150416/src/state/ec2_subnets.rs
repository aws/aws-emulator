// SPDX-License-Identifier: CC0-1.0

//! THE HOP: asking the service that OWNS subnets what the subnets a create names really are.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. So the ONE answer to what
//! block a subnet carries, which zone it sits in and whether the account has it at all is
//! EC2 — asked the way any emulated service reaches another, over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process (`services/common/src/network.rs` states
//! that contract, and the services that already place a resource in a subnet keep this hop
//! in a module of this name).
//!
//! The VPC those subnets sit in is EC2's resource too, and [`vpc_absence`] is the same hop
//! asked about it: a create that places a directory in a VPC the account does not have is
//! naming a resource that does not exist, which is not something this service can decide on
//! its own either.
//!
//! What the answer is KEPT as, and what a server's address is counted out of it, is
//! [`super::directory_network`]; this module only asks.

use std::collections::BTreeMap;

use super::directory_network::SubnetNetwork;

/// The `DescribeSubnets` filter that selects subnets BY ID (`subnet-id`, listed by that
/// operation's own `Filters` documentation).
///
/// A filter rather than `SubnetIds`: an identifier the account has no subnet for is then an
/// absent row instead of a failure of the whole call, so every identifier the create named
/// is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The most subnet identifiers ONE hop asks about, as values of one `subnet-id` filter —
/// and the most this resolution asks about at all.
///
/// The owner evaluates a filter against every subnet the account stores, so the cost of a hop
/// is (identifiers asked x subnets stored): capping the first factor is what keeps this
/// resolution a CONSTANT times what the account stores rather than something the request
/// chooses. The modelled `SubnetIds` carries no length bound, and a directory's servers sit
/// in [`super::REQUIRED_SUBNETS`] subnets, so a settings block naming more than this is
/// naming far more than any placement can use and the surplus resolves to nothing.
const SUBNET_IDS_PER_HOP: usize = 64;

/// An AWS SDK client for EC2 over the in-process callback into this same emulator.
///
/// `region` is the request's own signing region, so the question is asked in the Region the
/// caller addressed. The endpoint URL is only the address the callback's transport routes on
/// IN PROCESS: no socket is opened, no name is resolved and nothing leaves this process. The
/// credentials are the callback's, which sign this in-process hop and never appear in a
/// response.
///
/// The Region a request is signed for is CALLER-SUPPLIED text and here it ends up in an
/// endpoint URL's authority, so it is read as a Region LABEL
/// (`service_common::arn::region_label`, the one place this tree states that grammar)
/// instead of being interpolated as given: `None` for a value that would name an authority
/// the caller chose rather than the peer service this emulator asks.
fn ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    let region = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials the callback
        // hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// The `DescribeVpcs` filter that selects VPCs BY ID (`vpc-id`, listed by that operation's
/// own `Filters` documentation).
///
/// A filter rather than `VpcIds`: an identifier the account has no VPC for is then an empty
/// result instead of a failure of the whole call, so "the account does not have this VPC"
/// and "the owner of VPCs could not be asked" stay two different answers.
const VPC_ID_FILTER: &str = "vpc-id";

/// Why `vpc_id` cannot hold a directory's servers — that the account has no such VPC in
/// `region` — or `None` when it has one, and `None` when the owner of VPCs could not be
/// asked at all.
///
/// A VPC is not a resource of this service either, so whether the account has the one a
/// create names is EC2's answer, asked over the same in-process hop the subnets are resolved
/// through. Nothing is refused where EC2 could not be asked: that is the same direction
/// [`resolve_subnet_networks`] takes, so an emulator mounted without the owner of VPCs still
/// serves a create rather than refusing every one of them.
///
/// Called by a create BEFORE it takes this service's store, so no guard is held across the
/// hop. ONE hop about ONE identifier, whatever the request body carries.
pub async fn vpc_absence(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Option<&'static str> {
    let client = ec2(ctx, region)?;
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(VPC_ID_FILTER)
        .values(vpc_id)
        .build();
    match client.describe_vpcs().filters(by_id).send().await {
        Ok(output) => output
            .vpcs()
            .iter()
            .all(|vpc| vpc.vpc_id() != Some(vpc_id))
            .then_some(super::INVALID_VPC_ID),
        Err(error) => {
            // At `warn` for the same reason the subnet hop logs one: a create served without
            // the owner of VPCs having been asked is worth seeing, and this IS why.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the VPC a directory is placed in could not be resolved against EC2"
            );
            None
        }
    }
}

/// The network of every subnet of `subnet_ids` the account has in `region`, keyed by
/// identifier, as EC2 reports it — an identifier it holds no subnet for is simply absent
/// from the map, and so is every identifier when the hop could not be made at all.
///
/// Called by a create BEFORE it takes this service's store, so no guard is held across the
/// hop. ONE hop of at most [`SUBNET_IDS_PER_HOP`] identifiers, whatever the request body
/// carries: nothing caller-sized is asked about, allocated or stored here.
pub async fn resolve_subnet_networks(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> BTreeMap<String, SubnetNetwork> {
    let mut resolved: BTreeMap<String, SubnetNetwork> = BTreeMap::new();
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return resolved;
    };
    // Asked for once each, and at most a hop's worth: an identifier the settings block
    // repeats costs no second value in the filter, and a block longer than one hop is
    // resolved as far as that hop reaches.
    let mut distinct: Vec<&str> = Vec::with_capacity(SUBNET_IDS_PER_HOP);
    for subnet_id in subnet_ids {
        if distinct.len() >= SUBNET_IDS_PER_HOP {
            break;
        }
        if !distinct.contains(&subnet_id.as_str()) {
            distinct.push(subnet_id);
        }
    }
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(SUBNET_ID_FILTER)
        .set_values(Some(distinct.iter().map(|id| (*id).to_string()).collect()))
        .build();
    match client.describe_subnets().filters(by_id).send().await {
        Ok(output) => {
            for subnet in output.subnets() {
                let Some(subnet_id) = subnet.subnet_id() else {
                    continue;
                };
                resolved.insert(
                    subnet_id.to_string(),
                    SubnetNetwork {
                        subnet_id: subnet_id.to_string(),
                        cidr_block: subnet.cidr_block().map(str::to_string),
                        availability_zone: subnet.availability_zone().map(str::to_string),
                    },
                );
            }
        }
        Err(error) => {
            // At `warn`: a directory created where the owner of subnets cannot be asked
            // serves no DNS address, which is worth seeing, and this IS why.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnets a directory is placed in could not be resolved against EC2"
            );
        }
    }
    resolved
}
