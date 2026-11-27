// SPDX-License-Identifier: CC0-1.0

//! The ADDRESSES a load balancer's own nodes carry, and the name it publishes for a client to
//! reach them at.
//!
//! "The nodes of an Internet-facing load balancer have public IP addresses. … The nodes of an
//! internal load balancer have only private IP addresses"
//! (`CreateLoadBalancerInput$Scheme`), and a NODE of a load balancer is one of the
//! service-managed elastic network interfaces Elastic Load Balancing puts in each subnet the
//! load balancer is attached to — ordinary describable EC2 network interfaces, which is why
//! this suite lives in the router crate: the load balancer is ELBv2's resource and the
//! interfaces are EC2's inventory, and the rule is about what the second holds for the first.
//!
//! Two rules are under test:
//!
//! * THE NODES' ADDRESSES. Each interface of a load balancer carries a private address out of
//!   its own subnet, and an interface of a load balancer whose nodes carry PUBLIC addresses
//!   carries an association with one as well — which is what `DescribeNetworkInterfaces`
//!   reports as `association.publicIp`. An internal load balancer's interfaces carry no such
//!   association, and neither do a Gateway Load Balancer's: "You can't specify a scheme for a
//!   Gateway Load Balancer".
//! * THE NAME IT PUBLISHES. `LoadBalancer$DNSName` is "the public DNS name of the load
//!   balancer" — the address a client then sends traffic to — so a load balancer whose
//!   listeners carry a client's own traffic publishes an address a client ON THIS MACHINE can
//!   dial, the `host:port` of the socket this emulator opened for it. Both the Application and
//!   the Network family's listeners carry that traffic, under EITHER scheme: `Scheme` decides
//!   which clients may reach the load balancer in a real network, and in this emulator every
//!   such client reaches it at the one address the emulator serves it on. A Gateway Load
//!   Balancer's `GENEVE` listeners carry no request of that kind, so it keeps the host Elastic
//!   Load Balancing's own zone gives it. The whole table of host FORMS is the ELBv2 crate's
//!   `tests/all/load_balancer_dns_name_form.rs`; what this suite adds is that a published
//!   address really ACCEPTS a connection, beside the addresses the same load balancer's nodes
//!   carry.
//!
//! The rows span what the rules generalise over: every family the model's
//! `LoadBalancerTypeEnum` names against both schemes its `LoadBalancerSchemeEnum` names, and
//! one and two subnets, since the interfaces are per subnet.
//!
//! Assembling the whole router costs stack, so the suite runs on a sized thread, exactly as
//! `elbv2_network_resolution.rs` does — whose wire plumbing it reuses rather than restating.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_network_resolution::{
    attach_internet_gateway, ec2_ok, elbv2, BOOT_STACK_SIZE, REGION,
};
use crate::query_wire::{element, elements, opt_element};

/// The alias `NetworkInterface$RequesterId` carries for an interface Elastic Load Balancing
/// created, which is also what the `requester-id` filter of a describe selects it by.
const SERVICE_REQUESTER: &str = "amazon-elb";

/// One row: the load balancer to create, and what must then hold for it.
struct Row {
    case: &'static str,
    name: &'static str,
    lb_type: &'static str,
    /// The `Scheme` the create names, or `None` where the family carries none — "you can't
    /// specify a scheme for a Gateway Load Balancer" (`CreateLoadBalancerInput$Scheme`).
    scheme: Option<&'static str>,
    /// How many of the network's subnets it is placed in — one interface each.
    subnets: usize,
    /// Whether its nodes carry PUBLIC addresses, which is what each interface then holds an
    /// association with.
    public_nodes: bool,
    /// Whether the name it publishes is an address a client on this machine can dial.
    dialable: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "an internet-facing application load balancer in two subnets",
        name: "nodes-alb-public",
        lb_type: "application",
        scheme: Some("internet-facing"),
        subnets: 2,
        public_nodes: true,
        dialable: true,
    },
    Row {
        case: "an internal application load balancer in two subnets",
        name: "nodes-alb-private",
        lb_type: "application",
        scheme: Some("internal"),
        subnets: 2,
        public_nodes: false,
        dialable: true,
    },
    Row {
        case: "an internet-facing network load balancer in one subnet",
        name: "nodes-nlb-public",
        lb_type: "network",
        scheme: Some("internet-facing"),
        subnets: 1,
        public_nodes: true,
        dialable: true,
    },
    Row {
        case: "an internal network load balancer in two subnets",
        name: "nodes-nlb-private",
        lb_type: "network",
        scheme: Some("internal"),
        subnets: 2,
        public_nodes: false,
        dialable: true,
    },
    Row {
        case: "a gateway load balancer, whose nodes carry no scheme at all",
        name: "nodes-gwlb",
        lb_type: "gateway",
        scheme: None,
        subnets: 1,
        public_nodes: false,
        dialable: false,
    },
];

/// One of a load balancer's own interfaces, reduced to what these rules read.
#[derive(Debug)]
struct Node {
    id: String,
    subnet: String,
    private_address: String,
    /// The public address its association carries, for an interface that has one.
    public_address: Option<String>,
}

/// Every interface of the account Elastic Load Balancing created, by id — the `requester-id`
/// filter selects exactly the service-managed ones.
fn service_interface_ids(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> Vec<String> {
    let described = ec2_ok(
        router,
        runtime,
        "DescribeNetworkInterfaces",
        &[
            ("Filter.1.Name", "requester-id"),
            ("Filter.1.Value.1", SERVICE_REQUESTER),
        ],
    );
    elements(&described, "networkInterfaceId")
}

/// What the inventory holds for the interface `id` names.
fn node(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, id: &str) -> Node {
    let described = ec2_ok(
        router,
        runtime,
        "DescribeNetworkInterfaces",
        &[("NetworkInterfaceId.1", id)],
    );
    Node {
        id: id.to_string(),
        subnet: element(&described, "subnetId"),
        private_address: element(&described, "privateIpAddress"),
        // The association an interface holding a public address carries; an interface with
        // none carries no `association` element at all.
        public_address: opt_element(&described, "publicIp"),
    }
}

/// Whether the published address really accepts a connection: the socket the emulator opened
/// for that load balancer is there, on this machine's loopback interface, as its `DNSName`
/// says.
fn accepts_a_connection(runtime: &tokio::runtime::Runtime, address: std::net::SocketAddr) -> bool {
    runtime.block_on(async move {
        tokio::time::timeout(
            CONNECT_DEADLINE,
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, address.port())),
        )
        .await
        .is_ok_and(|connected| connected.is_ok())
    })
}

/// How long a connection to the published address is given to be accepted — generous, because
/// what is under test is that the address is there, not how fast this host accepts.
const CONNECT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

#[test]
fn a_load_balancers_nodes_carry_the_addresses_its_scheme_gives_them() {
    std::thread::Builder::new()
        .name("elbv2-public-nodes".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // The network the rows run in, created through EC2 itself, with the internet
            // gateway an internet-facing load balancer needs.
            let vpc = element(
                &ec2_ok(
                    &router,
                    &runtime,
                    "CreateVpc",
                    &[("CidrBlock", "10.60.0.0/16")],
                ),
                "vpcId",
            );
            let mut subnets: Vec<(String, String)> = Vec::new();
            for (cidr, zone) in [("10.60.1.0/24", "us-west-2a"), ("10.60.2.0/24", "us-west-2b")] {
                subnets.push((
                    element(
                        &ec2_ok(
                            &router,
                            &runtime,
                            "CreateSubnet",
                            &[
                                ("VpcId", vpc.as_str()),
                                ("CidrBlock", cidr),
                                ("AvailabilityZone", zone),
                            ],
                        ),
                        "subnetId",
                    ),
                    cidr.to_string(),
                ));
            }
            attach_internet_gateway(&router, &runtime, &vpc);

            // Every public address seen, so no two nodes are handed the same one.
            let mut public_addresses: Vec<String> = Vec::new();

            for row in ROWS {
                let before = service_interface_ids(&router, &runtime);
                let mut params: Vec<(&str, &str)> =
                    vec![("Name", row.name), ("Type", row.lb_type)];
                if let Some(scheme) = row.scheme {
                    params.push(("Scheme", scheme));
                }
                if row.lb_type == "gateway" {
                    // A Gateway Load Balancer carries IPv4 addresses alone.
                    params.push(("IpAddressType", "ipv4"));
                }
                let subnet_keys = ["Subnets.member.1", "Subnets.member.2"];
                for (key, (subnet, _)) in subnet_keys.iter().zip(subnets.iter()).take(row.subnets) {
                    params.push((key, subnet.as_str()));
                }
                let created = elbv2(&router, &runtime, "CreateLoadBalancer", &params);
                assert_eq!(
                    created.status, 200,
                    "case {:?}: expected the create to be served, got {created:?}",
                    row.case
                );
                let dns_name = element(&created.text, "DNSName");

                // The name it published is an address a client can dial exactly where its
                // listeners carry a client's own traffic — and where it is, the address really
                // ACCEPTS a connection. What each family then serves on it, over a dialled
                // connection, is `elbv2_serves_an_instance_targets_own_response.rs`; what is
                // under test here is the address.
                let address = dns_name.parse::<std::net::SocketAddr>().ok();
                assert_eq!(
                    address.is_some(),
                    row.dialable,
                    "case {:?}: expected a dialable published address {}, got {dns_name:?}",
                    row.case,
                    row.dialable
                );
                match address {
                    Some(address) => assert!(
                        accepts_a_connection(&runtime, address),
                        "case {:?}: the published address {dns_name} accepted no connection",
                        row.case
                    ),
                    None => assert!(
                        dns_name.ends_with("elb.amazonaws.com")
                            || dns_name.ends_with(&format!("elb.{REGION}.amazonaws.com")),
                        "case {:?}: expected the host its own zone gives it, got {dns_name:?}",
                        row.case
                    ),
                }

                // And its nodes: one interface per subnet, each with a private address out of
                // that subnet, and a public one exactly where the scheme gives it one.
                let placed = service_interface_ids(&router, &runtime);
                let mine: Vec<Node> = placed
                    .iter()
                    .filter(|id| !before.contains(id))
                    .map(|id| node(&router, &runtime, id))
                    .collect();
                assert_eq!(
                    mine.len(),
                    row.subnets,
                    "case {:?}: expected one interface per subnet, got {mine:?}",
                    row.case
                );
                for placed in &mine {
                    let (_, cidr) = subnets
                        .iter()
                        .find(|(subnet, _)| *subnet == placed.subnet)
                        .unwrap_or_else(|| {
                            panic!(
                                "case {:?}: the interface sits in a subnet of the network, got \
                                 {placed:?}",
                                row.case
                            )
                        });
                    let prefix = cidr
                        .split_once(".0/")
                        .map(|(network_part, _)| format!("{network_part}."))
                        .expect("the subnet CIDR names its own range");
                    assert!(
                        placed.private_address.starts_with(&prefix),
                        "case {:?}: a node's private address comes out of its own subnet \
                         ({cidr}), got {placed:?}",
                        row.case
                    );
                    assert_eq!(
                        placed.public_address.is_some(),
                        row.public_nodes,
                        "case {:?}: expected a public address on the node {}, got {placed:?}",
                        row.case,
                        row.public_nodes
                    );
                    if let Some(public) = &placed.public_address {
                        assert!(
                            public.parse::<std::net::Ipv4Addr>().is_ok(),
                            "case {:?}: expected a public IPv4 address, got {placed:?}",
                            row.case
                        );
                        assert!(
                            !public.starts_with(&prefix),
                            "case {:?}: a public address is not one of the subnet's own, got \
                             {placed:?}",
                            row.case
                        );
                        assert!(
                            !public_addresses.contains(public),
                            "case {:?}: every node carries its own public address, {public} \
                             repeated on {}",
                            row.case,
                            placed.id
                        );
                        public_addresses.push(public.clone());
                    }
                }
            }
        })
        .expect("the sized thread")
        .join()
        .expect("the suite ran");
}
