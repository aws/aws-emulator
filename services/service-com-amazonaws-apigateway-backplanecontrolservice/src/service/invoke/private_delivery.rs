// SPDX-License-Identifier: CC0-1.0

//! Which interface VPC ENDPOINT a request an `HTTP_PROXY` integration carries over a VPC
//! LINK arrives at this api plane through.
//!
//! `Integration$uri` documents, of a private integration, that "if `connectionType` is
//! `VPC_LINK` specify the Network Load Balancer DNS name" and that "for a private HTTP
//! integration, the URI is not used for routing": such a request is delivered over the
//! LINK's own targets, so what decides which endpoint it arrives through is what is
//! registered behind those targets and never the host the `uri` happens to name. Where the
//! link's load balancers forward to a target group registered with ADDRESSES, and those
//! addresses are the endpoint network interfaces of an interface VPC endpoint, the request
//! arrives at the service behind that endpoint THROUGH it — which is the condition an api's
//! own resource policy gates a private invoke on
//! ([`super::super::resource_policy::SOURCE_VPCE_KEY`]).
//!
//! Three records decide it, each read where it is OWNED and none of them copied here:
//!
//! 1. the VPC LINK, and the load balancers it targets, are this service's own record
//!    ("the ARN of the network load balancer of the VPC targeted by the VPC link",
//!    `VpcLink$targetArns`), read under the same store lock the integration was read under;
//! 2. the target groups those load balancers forward to, and the addresses registered in
//!    them, are Elastic Load Balancing's, read from that service's own store on this one
//!    shared engine — the seam this plane already reads a web ACL's decision over;
//! 3. the endpoint those addresses belong to, and the DNS name it publishes, are Amazon
//!    EC2's, asked over the in-process callback the way a domain name access association's
//!    source is resolved ([`crate::state::vpc_endpoints`]). Nothing is cached: an endpoint
//!    created a moment ago resolves at once, and one that was deleted stops resolving at
//!    once.
//!
//! The ANSWER is the endpoint's own id, and an endpoint is only read as one of THIS plane's
//! when it publishes a name this plane reads as an endpoint's own address
//! ([`super::endpoint_of_host`]) — so no service name is spelled here and an endpoint to some
//! other service, which no request to this plane arrives through, is passed over.
//!
//! Nothing here writes to any store: it only reads what a create or a register persisted.
//!
//! LOCK ORDER: Elastic Load Balancing's store lock is taken and released on its own, with no
//! lock of this service held and before the EC2 hop is made, and that service reads none of
//! this one's state — so that order is the only one taken.

use std::collections::BTreeSet;

use ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state as load_balancing_store;
use ::smithy_server_runtime::OperationContext;

use super::{GetIntegrationOutput, DEFAULT_CONNECTION_TYPE, VPC_LINK_CONNECTION_TYPE};

/// The `TargetTypeEnum` value of a target group whose registrations are ADDRESSES: "ip -
/// Register targets by IP address" (`TargetGroup$TargetType`). An endpoint network interface
/// is reached by its private address, so this is the one target type a VPC link's load
/// balancer forwards to an interface VPC endpoint through; a group registering instances,
/// functions or load balancers names no endpoint interface.
const ADDRESS_TARGET_TYPE: &str = "ip";

/// The load balancers the private connection `integration` is carried over, or `None` when
/// it names no private connection or names a VPC link this account holds no record of.
///
/// Read from this service's own store, under the lock that read the integration itself, off
/// the configuration the stage's variables have already been resolved into — so a
/// `connectionId` written as a stage-variable reference names the link that variable's value
/// names, exactly as [`super::unusable_vpc_link`] reads it. Whether the link can carry the
/// call at all is that function's question, not this one's: this only reports WHAT the link
/// targets.
pub(super) fn connection_load_balancers(
    store: &crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    integration: &GetIntegrationOutput,
) -> Option<Vec<String>> {
    let connection_type = integration
        .connection_type
        .as_deref()
        .unwrap_or(DEFAULT_CONNECTION_TYPE);
    if connection_type != VPC_LINK_CONNECTION_TYPE {
        return None;
    }
    let connection_id = integration.connection_id.as_deref()?;
    let link = store.vpc_links.get(connection_id)?;
    Some(link.target_arns.clone())
}

/// The interface VPC ENDPOINT the request this integration passes on ARRIVES through, when
/// it is carried over a VPC link whose targets are that endpoint's own network interfaces —
/// and `None` for every other delivery, which arrives through no endpoint this plane can
/// name.
///
/// `load_balancers` is what [`connection_load_balancers`] read for this integration, so a
/// call carried over no private connection asks neither service anything.
pub(super) async fn arrival_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    region: &str,
    load_balancers: Option<&[String]>,
) -> Option<String> {
    let addresses = registered_addresses(engine, load_balancers?);
    if addresses.is_empty() {
        return None;
    }
    publishing_endpoint(ctx, region, addresses).await
}

/// The addresses registered in the target groups `load_balancers` forward to.
///
/// A target group records the load balancers that forward to it
/// (`TargetGroup$LoadBalancerArns`), so the groups one load balancer reaches are the groups
/// naming it — which is the same association Elastic Load Balancing starts health checking a
/// group's targets on. Only a group registering ADDRESSES is read ([`ADDRESS_TARGET_TYPE`]);
/// an engine carrying no Elastic Load Balancing store holds no target group at all, so
/// nothing is registered anywhere and nothing is resolved.
fn registered_addresses<Eng: Send + Sync + 'static>(
    engine: &Eng,
    load_balancers: &[String],
) -> Vec<String> {
    let Some(state) = load_balancing_store::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    store
        .target_groups
        .values()
        .filter(|group| group.target_type == ADDRESS_TARGET_TYPE)
        .filter(|group| {
            group
                .load_balancer_arns
                .iter()
                .any(|arn| load_balancers.iter().any(|target| target == arn))
        })
        .flat_map(|group| group.targets.values().map(|target| target.id.clone()))
        .collect()
}

/// The id of the interface VPC endpoint holding one of `addresses` on its network interfaces
/// and publishing a name this plane is addressed on, or `None` when no endpoint of the
/// account does.
///
/// Two hops, both to the service that OWNS the records: the interfaces holding those
/// addresses ("addresses.private-ip-address - The private IPv4 address",
/// `DescribeNetworkInterfaces$Filters`), and then the endpoint whose own interfaces they are
/// ("the network interfaces for the endpoint", `VpcEndpoint$NetworkInterfaceIds`). An
/// endpoint that could not be asked about at all — no Region label, or a hop that failed —
/// is evidence of nothing, so nothing is resolved and the reason is said at `warn`.
///
/// Both answers are PAGED, so both are read page by page: the account's interfaces and
/// endpoints are its own and neither is bounded by what this request carries, so an answer
/// that depends on the page size would gate a correctly configured invoke on how many other
/// endpoints the account happens to hold.
async fn publishing_endpoint(
    ctx: &OperationContext<'_>,
    region: &str,
    addresses: Vec<String>,
) -> Option<String> {
    let client = crate::state::vpc_endpoints::ec2(ctx, region)?;
    let interface_ids = interfaces_holding(&client, region, addresses).await;
    if interface_ids.is_empty() {
        return None;
    }
    endpoint_of_interfaces(&client, region, &interface_ids).await
}

/// How many pages of one EC2 answer are read.
///
/// A page is the largest EC2 returns, so this is far above the interfaces and endpoints any
/// account of a developer's emulator holds, and it is what stops one invoke's resolution
/// walking an unbounded number of pages however much the account accumulates.
const MAX_PAGES_READ: usize = 32;

/// The ids of the network interfaces holding one of `addresses`.
async fn interfaces_holding(
    client: &::aws_sdk_ec2::Client,
    region: &str,
    addresses: Vec<String>,
) -> BTreeSet<String> {
    let holding = ::aws_sdk_ec2::types::Filter::builder()
        .name("addresses.private-ip-address")
        .set_values(Some(addresses))
        .build();
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES_READ {
        let answer = match client
            .describe_network_interfaces()
            .filters(holding.clone())
            .set_next_token(page.take())
            .send()
            .await
        {
            Ok(answer) => answer,
            Err(error) => {
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the interfaces holding the private connection's target addresses could \
                     not be resolved against EC2"
                );
                break;
            }
        };
        found.extend(
            answer
                .network_interfaces()
                .iter()
                .filter_map(::aws_sdk_ec2::types::NetworkInterface::network_interface_id)
                .map(str::to_string),
        );
        // "If there are no additional items to return, the string is empty": an empty token
        // ends the walk exactly as an absent one does.
        match answer.next_token().filter(|token| !token.is_empty()) {
            Some(next) => page = Some(next.to_string()),
            None => break,
        }
    }
    found
}

/// The id of the interface VPC endpoint whose own network interfaces include one of
/// `interface_ids` and that publishes a name this plane is addressed on.
async fn endpoint_of_interfaces(
    client: &::aws_sdk_ec2::Client,
    region: &str,
    interface_ids: &BTreeSet<String>,
) -> Option<String> {
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES_READ {
        let answer = match client
            .describe_vpc_endpoints()
            .set_next_token(page.take())
            .send()
            .await
        {
            Ok(answer) => answer,
            Err(error) => {
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the VPC endpoint the private connection's targets belong to could not \
                     be resolved against EC2"
                );
                return None;
            }
        };
        let holding = answer
            .vpc_endpoints()
            .iter()
            .filter(|endpoint| {
                endpoint
                    .network_interface_ids()
                    .iter()
                    .any(|interface| interface_ids.contains(interface))
            })
            .find_map(|endpoint| {
                let id = endpoint.vpc_endpoint_id()?;
                endpoint
                    .dns_entries()
                    .iter()
                    .filter_map(::aws_sdk_ec2::types::DnsEntry::dns_name)
                    // The endpoint's OWN name, as this plane reads one: a name whose first
                    // label is this endpoint's id, under the `execute-api` label. An endpoint
                    // to another service publishes no such name, and a private hosted zone's
                    // name for this one names the api rather than the endpoint.
                    .any(|name| super::endpoint_of_host(name) == Some(id))
                    .then(|| id.to_string())
            });
        if holding.is_some() {
            return holding;
        }
        match answer.next_token().filter(|token| !token.is_empty()) {
            Some(next) => page = Some(next.to_string()),
            None => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    //! The pure half of this module: WHICH load balancers one integration's private
    //! connection targets. What the whole chain then resolves, and what a deployed stage
    //! SERVES behind the gate, is pinned end to end over the three services that own its
    //! records in `core/router/tests/all/apigateway_private_vpce_invoke.rs`.

    use super::*;

    /// One stored VPC link, under `id`, over `targets`.
    fn store_holding(
        id: &str,
        targets: &[&str],
    ) -> crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState {
        let mut store = crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState::default();
        store.vpc_links.insert(
            id.to_string(),
            crate::state::VpcLinkEntry {
                id: id.to_string(),
                target_arns: targets.iter().map(|arn| (*arn).to_string()).collect(),
                ..Default::default()
            },
        );
        store
    }

    /// RULE: the load balancers a call is carried over are the targets of the VPC link the
    /// integration's `connectionId` names, and only for a `VPC_LINK` `connectionType`.
    #[test]
    fn only_a_named_vpc_link_of_this_account_carries_the_call() {
        let held = "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/nlb/1";
        let store = store_holding("abcd12", &[held]);
        // Rows differ along the connection the integration declares: its type, whether it
        // names a link at all, and whether the account holds the link it names. `carried`
        // is whether that connection carries the call over the link's own target.
        let rows: &[(&str, Option<&str>, Option<&str>, bool)] = &[
            ("no connectionType at all", None, None, false),
            ("the public internet", Some("INTERNET"), Some("abcd12"), false),
            ("a private connection naming no link", Some("VPC_LINK"), None, false),
            (
                "a private connection naming a link this account does not hold",
                Some("VPC_LINK"),
                Some("nosuch"),
                false,
            ),
            (
                "a private connection naming a link this account holds",
                Some("VPC_LINK"),
                Some("abcd12"),
                true,
            ),
        ];
        for (case, connection_type, connection_id, carried) in rows {
            let integration = GetIntegrationOutput {
                r#type: Some("HTTP_PROXY".to_string()),
                connection_type: connection_type.map(str::to_string),
                connection_id: connection_id.map(str::to_string),
                ..Default::default()
            };
            let expected = carried.then(|| vec![held.to_string()]);
            let targeted = connection_load_balancers(&store, &integration);
            assert_eq!(
                targeted, expected,
                "case {case:?}: expected {expected:?}, got {targeted:?}"
            );
        }
    }
}
