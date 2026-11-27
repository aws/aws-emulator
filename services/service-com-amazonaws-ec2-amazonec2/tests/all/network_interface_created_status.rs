// SPDX-License-Identifier: CC0-1.0

//! The STATUS every READ of a created network interface reports.
//!
//! Driven over the wire this service speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`, answered
//! with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: an elastic network interface a caller creates comes to rest in the
//! status of an interface nothing holds — `available` — the instant it exists, so EVERY READ of
//! it reports that status, the very first one after the create included. There is nothing for a
//! caller to wait out and no earlier status a read can be answered; only an attachment moves the
//! interface off that resting status. (What the create's own answer carries, which is a status
//! no read reports, is held by the sibling suite
//! `tests/all/network_interface_create_comes_up_pending.rs`.)
//!
//! The rows differ along what the create carries (nothing beyond the subnet, a description and
//! a tag, a second interface beside the first, a secondary private address) and along which of
//! the account's subnets it sits in, because none of that bears on the status; and the interface
//! is read back both by `NetworkInterfaceId` and enumerated by a `vpc-id` filter, because
//! neither bears on it either.
//!
//! Nothing here waits for a span, and nothing is asserted about elapsed time: the status every
//! read reports is the same whichever instant the read happens at, so no row depends on how
//! fast the host runs.
//!
//! Beside the status, each row pins the two members a created interface carries that follow
//! from the interface rather than from the request: the principal that created it
//! (`NetworkInterface$RequesterId`) and its public hostname
//! (`NetworkInterface$PublicDnsName`), which is the EMPTY name for an interface holding no
//! public address — reported either way, so an absent member cannot be read as a name.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, NETWORK_INTERFACE_STATUS_AVAILABLE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, filters, indexed};

/// The Region every call here is signed for; the account's default subnets are seeded for it.
const REGION: &str = "us-east-1";

/// The values `NetworkInterfaceStatus` publishes
/// (`smithy_query(query="shape",shape="NetworkInterfaceStatus")`): a status a READ reports is
/// one of these, whichever path reported it.
const NETWORK_INTERFACE_STATUS_VALUES: [&str; 5] = [
    "available",
    "associated",
    "attaching",
    "in-use",
    "detaching",
];

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The account's default subnet ids, in store order (one per zone of the Region).
fn subnet_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .subnets
        .keys()
        .cloned()
        .collect()
}

/// The VPC of one of the account's subnets.
fn vpc_of_subnet(engine: &engine::InMemoryEngine, subnet_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .subnets
        .get(subnet_id)
        .and_then(|subnet| subnet.vpc_id.clone())
        .unwrap_or_default()
}

/// One call that must be served, as its body.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The status `DescribeNetworkInterfaces` reports for one interface.
async fn described_status(case: &str, router: &MultiServiceRouter, id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[id]),
    )
    .await;
    element(&body, "status")
        .unwrap_or_else(|| panic!("case {case:?}: the reported interface carries no status: {body}"))
}

/// One create whose interface must be `available` to every read of it.
#[derive(Debug, Clone)]
struct CreateRow {
    /// What the row is about.
    case: &'static str,
    /// Which of the account's subnets the interface sits in.
    subnet_index: usize,
    /// The fields the create carries beyond `SubnetId`.
    extra: Vec<(String, String)>,
}

/// Rule: a created interface is `available` to every read, the first one included.
#[tokio::test]
async fn a_created_interface_is_available_to_every_read() {
    let (router, engine) = super::ec2_query::emulator();
    let subnets = subnet_ids(&engine);
    assert!(
        subnets.len() >= 2,
        "the account's default VPC must have a subnet per zone, got {subnets:?}"
    );

    let rows = [
        CreateRow {
            case: "the subnet alone",
            subnet_index: 0,
            extra: Vec::new(),
        },
        CreateRow {
            case: "a description and a tag",
            subnet_index: 1,
            extra: vec![
                param("Description", "a described interface"),
                param("TagSpecification.1.ResourceType", "network-interface"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "web"),
            ],
        },
        CreateRow {
            case: "a second interface in the first subnet",
            subnet_index: 0,
            extra: vec![param("Description", "a second interface")],
        },
        CreateRow {
            case: "a secondary private address beside the primary",
            subnet_index: 1,
            extra: vec![param("SecondaryPrivateIpAddressCount", "1")],
        },
    ];

    for CreateRow {
        case,
        subnet_index,
        extra,
    } in rows
    {
        let subnet_id = subnets[subnet_index].clone();
        let mut params = vec![param("SubnetId", &subnet_id)];
        params.extend(extra);
        let created = ok(case, &router, "CreateNetworkInterface", &params).await;
        let network_interface_id = element(&created, "networkInterfaceId")
            .unwrap_or_else(|| panic!("case {case:?}: the create must answer an id: {created}"));

        // Nothing holds the interface, so the create reports no attachment.
        assert!(
            element(&created, "attachment").is_none(),
            "case {case:?}: an interface nothing holds carries no attachment, got {created}"
        );
        // And the two members that follow from the interface rather than the request: the
        // principal that created it, and the empty public hostname of an interface holding no
        // public address.
        let requester = element(&created, "requesterId");
        assert!(
            requester.as_deref().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected the create to name the principal that created the \
             interface, got {requester:?} in {created}"
        );
        assert!(
            created.contains("<publicDnsName></publicDnsName>")
                || created.contains("<publicDnsName/>"),
            "case {case:?}: an interface with no public address reports the EMPTY public \
             hostname, got {created}"
        );

        // The FIRST read, taken immediately after the create with nothing in between: the
        // interface nothing holds is already at rest.
        let first_read = described_status(case, &router, &network_interface_id).await;
        assert_eq!(
            first_read, NETWORK_INTERFACE_STATUS_AVAILABLE,
            "case {case:?}: the first read of a created interface reports \
             {NETWORK_INTERFACE_STATUS_AVAILABLE:?}, got {first_read:?}"
        );
        assert!(
            NETWORK_INTERFACE_STATUS_VALUES.contains(&first_read.as_str()),
            "case {case:?}: a reported status is one of the modelled \
             {NETWORK_INTERFACE_STATUS_VALUES:?}, got {first_read:?}"
        );

        // And it STAYS there, however many further calls the account makes: `available` is
        // the interface's resting state, not a transient the next caller moves on.
        ok(case, &router, "DescribeVpcs", &[]).await;
        ok(case, &router, "DescribeSubnets", &[]).await;
        let again = described_status(case, &router, &network_interface_id).await;
        assert_eq!(
            again, first_read,
            "case {case:?}: a later read must report the same status, got {again:?} after \
             {first_read:?}"
        );

        // Enumerated by the VPC rather than by its own id — a different read path, same
        // status: the interface is reported, and every interface the VPC enumerates that is
        // held by nothing is reported at rest.
        let vpc_id = vpc_of_subnet(&engine, &subnet_id);
        let enumerated = ok(
            case,
            &router,
            "DescribeNetworkInterfaces",
            &filters(&[("vpc-id", &[vpc_id.as_str()])]),
        )
        .await;
        assert!(
            enumerated.contains(&network_interface_id),
            "case {case:?}: the VPC must enumerate the interface, got {enumerated}"
        );
        let statuses = elements(&enumerated, "status");
        assert!(
            !statuses.is_empty()
                && statuses
                    .iter()
                    .all(|status| status == NETWORK_INTERFACE_STATUS_AVAILABLE),
            "case {case:?}: every interface the VPC enumerates is held by nothing, so each one \
             is reported {NETWORK_INTERFACE_STATUS_AVAILABLE:?}, got {statuses:?}"
        );
    }
}
