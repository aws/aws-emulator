// SPDX-License-Identifier: CC0-1.0

//! Whether the subnets a Fargate profile names are subnets its Pods may run in.
//!
//! A Fargate profile's Pods get no public address of their own, so Amazon EKS
//! only accepts PRIVATE subnets for one: "you must specify at least one subnet...
//! The subnets must be private subnets" (`CreateFargateProfileRequest$subnets`).
//! Whether a subnet is private is a fact about the network, which is EC2's — a
//! subnet is public exactly when the route table that governs it carries a route
//! to an internet gateway (Amazon VPC User Guide, "Subnets": "If a subnet is
//! associated with a route table that has a route to an internet gateway, it's
//! known as a public subnet") — so EC2 is what answers it, over the same
//! in-process callback every other cross-service reading of this service goes
//! through.
//!
//! As everywhere else in that neighbourhood, a refusal is grounded in what EC2
//! ANSWERED: an emulator with no EC2 mounted, or a read it refused, says nothing
//! about the network and refuses nothing.

use super::*;

/// The id prefix EC2 gives an internet gateway, which is how a route's target
/// says it is one: `RouteTable$Routes` reports the gateway in `GatewayId`, and
/// EC2's own ids for that resource are `igw-<hex>` (`AttachInternetGateway`'s
/// `InternetGatewayId` example). The one other value this service could see
/// there is the `local` target of a VPC's own CIDR route, which this prefix does
/// not match.
pub const INTERNET_GATEWAY_ID_PREFIX: &str = "igw-";

/// The EC2 filter that selects the route tables EXPLICITLY associated with a
/// subnet, and the two that select a VPC's main table — the table that governs
/// every subnet without an association of its own ("a subnet ID is not returned
/// for an implicit association", `RouteTableAssociation$SubnetId`). Spelled as
/// `DescribeRouteTablesRequest$Filters` documents them.
const FILTER_ASSOCIATION_SUBNET_ID: &str = "association.subnet-id";
/// See [`FILTER_ASSOCIATION_SUBNET_ID`].
const FILTER_ASSOCIATION_MAIN: &str = "association.main";
/// See [`FILTER_ASSOCIATION_SUBNET_ID`].
const FILTER_VPC_ID: &str = "vpc-id";

/// The sentence a `subnets` entry that is not a private subnet is refused with.
///
/// Fixed text around the one variable part the caller has to act on — the subnet
/// at fault, read off the request — as Amazon EKS words this refusal.
pub fn public_subnet_message(subnet_id: &str) -> String {
    format!("Subnet {subnet_id} provided in Fargate Profile is not a private subnet")
}

/// Why the subnets a Fargate profile names are not ones its Pods may run in, or
/// `None` when every one of them is private — and also when EC2 answered nothing
/// a refusal could be grounded in.
///
/// WHICH subnet a refusal names: a request can carry more than one public
/// subnet, and the one reported is the FIRST of them in the order the request
/// listed them, so the answer does not depend on the order EC2 happens to report
/// the subnets in.
///
/// The work is BOUNDED where it grows rather than by the size of the request: the
/// subnets are read in ONE `DescribeSubnets`, a repeated id is the same question
/// and is asked once, and at most [`MAX_SUBNETS_RESOLVED_INDIVIDUALLY`] distinct
/// ids are resolved to their route tables at all — the same ceiling
/// [`resolve_subnets`] holds its own per-id walk to.
pub async fn public_subnet_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<String> {
    if subnet_ids.is_empty() {
        return None;
    }
    let endpoint = peer_endpoint("ec2", region)?;
    let ec2 = callback_client!(aws_sdk_ec2, ctx, endpoint, region);
    // The subnets as EC2 holds them: a subnet's VPC is what says which main route
    // table governs it when it has no association of its own. A read EC2 refused
    // (an id it holds no subnet for, which the create reports on its own through
    // [`unresolved_subnet_refusal`]) leaves nothing to ground a refusal in.
    let described = ec2
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await;
    let output = match described {
        Ok(output) => output,
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the requested subnets could not be read from EC2"
            );
            return None;
        }
    };
    let vpc_of: ::std::collections::BTreeMap<&str, Option<&str>> = output
        .subnets()
        .iter()
        .filter_map(|subnet| subnet.subnet_id().map(|id| (id, subnet.vpc_id())))
        .collect();

    let mut asked: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    for subnet_id in subnet_ids {
        if !asked.insert(subnet_id.as_str()) {
            continue;
        }
        if asked.len() > MAX_SUBNETS_RESOLVED_INDIVIDUALLY {
            break;
        }
        // An id EC2 did not report is one it holds no subnet for, which is not this
        // question's to answer.
        let Some(vpc_id) = vpc_of.get(subnet_id.as_str()) else {
            continue;
        };
        if reaches_an_internet_gateway(&ec2, subnet_id, *vpc_id).await {
            return Some(public_subnet_message(subnet_id));
        }
    }
    None
}

/// Whether the route table that governs `subnet_id` carries a route to an
/// internet gateway — which is what makes the subnet a PUBLIC one.
///
/// The governing table is the one EXPLICITLY associated with the subnet, and the
/// VPC's main table for a subnet with no association of its own: "if you don't
/// explicitly associate a subnet with a particular route table, the subnet is
/// implicitly associated with the main route table" (Amazon VPC User Guide,
/// "Route tables"). Both are read through EC2's own filters, so this service
/// reads the network the way any other client of EC2 would.
///
/// `false` when nothing answered: a table this emulator could not read says
/// nothing about the subnet, and a subnet is never refused on nothing.
async fn reaches_an_internet_gateway(
    ec2: &::aws_sdk_ec2::Client,
    subnet_id: &str,
    vpc_id: Option<&str>,
) -> bool {
    let explicit = route_tables_matching(
        ec2,
        &[(FILTER_ASSOCIATION_SUBNET_ID, vec![subnet_id.to_string()])],
    )
    .await;
    let governing = if explicit.is_empty() {
        let Some(vpc_id) = vpc_id else {
            return false;
        };
        route_tables_matching(
            ec2,
            &[
                (FILTER_VPC_ID, vec![vpc_id.to_string()]),
                (FILTER_ASSOCIATION_MAIN, vec!["true".to_string()]),
            ],
        )
        .await
    } else {
        explicit
    };
    governing.iter().any(|route_table| {
        route_table.routes().iter().any(|route| {
            route
                .gateway_id()
                .is_some_and(|gateway| gateway.starts_with(INTERNET_GATEWAY_ID_PREFIX))
        })
    })
}

/// The route tables EC2 reports for `filters`, or none when the read answered
/// nothing.
async fn route_tables_matching(
    ec2: &::aws_sdk_ec2::Client,
    filters: &[(&str, Vec<String>)],
) -> Vec<::aws_sdk_ec2::types::RouteTable> {
    let filters: Vec<::aws_sdk_ec2::types::Filter> = filters
        .iter()
        .map(|(name, values)| {
            ::aws_sdk_ec2::types::Filter::builder()
                .name(*name)
                .set_values(Some(values.clone()))
                .build()
        })
        .collect();
    let described = ec2
        .describe_route_tables()
        .set_filters(Some(filters))
        .send()
        .await;
    match described {
        Ok(output) => output.route_tables.unwrap_or_default(),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnet's route tables could not be read from EC2"
            );
            Vec::new()
        }
    }
}
