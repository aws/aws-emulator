// SPDX-License-Identifier: CC0-1.0

//! The Kubernetes service CIDR Amazon EKS ASSIGNS a cluster that was created without
//! one, against the address space of the VPC it is created in.
//!
//! RULE under test: "If you didn't specify a CIDR block when you created the cluster,
//! then Kubernetes assigns addresses from either the `10.100.0.0/16` or
//! `172.20.0.0/16` CIDR blocks"
//! (`smithy_query(query="shape", shape="KubernetesNetworkConfigResponse")`) — the
//! block is one of those two and WHICH of them is chosen per cluster, by the same
//! constraint the member states for a block a CALLER names: it does not overlap any
//! CIDR block assigned to the VPC the cluster is placed in. So the first of the two
//! that overlaps none of that VPC's ranges is the one the cluster reports, and a
//! cluster that named a block of its own keeps exactly that one.
//!
//! That makes the VPC's ranges the requirement, which is why this suite lives here
//! rather than in the EKS crate: a VPC's associated CIDR blocks are Amazon EC2's to
//! report, so only an emulator with EC2 mounted beside Amazon EKS can answer the
//! question at all. The half that needs no EC2 — the choice itself over a table of
//! VPC ranges, and the block a cluster that named one keeps — is
//! `services/service-com-amazonaws-eks-awswesleyfrontend/tests/all/cluster_plane_reported_members.rs`.
//!
//! The table spans what the rule generalises over: a VPC that overlaps neither block,
//! one whose range IS the preferred block, one that sits inside it, one that overlaps
//! only the other block, and one whose SECOND associated range is what overlaps — so
//! no row is about one recorded VPC. (A range CONTAINING the preferred block is in the
//! crate-local table instead: Amazon EC2 admits no VPC wider than a `/16`.) Each row
//! also asserts the invariant behind the rule: the assigned block overlaps none of the
//! ranges EC2 reports for that cluster's VPC.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built —
//! and the requests driven — on a sized thread.
//!
//! The request harness is the test crate's own: `eks_wire` for the drivers and the
//! answer, `query_wire` for the XML readers.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use service_com_amazonaws_eks_awswesleyfrontend::state::SERVICE_IPV4_CIDR_BLOCKS;

use crate::eks_wire::{ec2, eks, role_arn, REGION};
use crate::query_wire::element;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The two blocks the model documents, as this service orders them: the one a cluster
/// is assigned when the VPC leaves it free, and the one it falls back to.
const PREFERRED: &str = "172.20.0.0/16";
/// See [`PREFERRED`].
const ALTERNATE: &str = "10.100.0.0/16";

/// One row: the VPC a cluster is created in, the two subnets it gets, and the service
/// CIDR Amazon EKS must assign it.
struct Row {
    case: &'static str,
    cluster: &'static str,
    /// The VPC's primary range.
    vpc_cidr: &'static str,
    /// A second range associated with that VPC, for the row where the overlap is with
    /// a block that is not the primary one.
    secondary_cidr: Option<&'static str>,
    /// Two subnets inside the VPC, one per Availability Zone.
    subnet_cidrs: [&'static str; 2],
    assigned: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a VPC that overlaps neither block",
        cluster: "default-cidr-free",
        vpc_cidr: "10.30.0.0/16",
        secondary_cidr: None,
        subnet_cidrs: ["10.30.1.0/24", "10.30.2.0/24"],
        assigned: PREFERRED,
    },
    Row {
        case: "a VPC whose range IS the preferred block",
        cluster: "default-cidr-same",
        vpc_cidr: "172.20.0.0/16",
        secondary_cidr: None,
        subnet_cidrs: ["172.20.1.0/24", "172.20.2.0/24"],
        assigned: ALTERNATE,
    },
    Row {
        case: "a VPC whose range sits INSIDE the preferred block",
        cluster: "default-cidr-inside",
        vpc_cidr: "172.20.8.0/22",
        secondary_cidr: None,
        subnet_cidrs: ["172.20.8.0/24", "172.20.9.0/24"],
        assigned: ALTERNATE,
    },
    Row {
        case: "a VPC that overlaps only the other block",
        cluster: "default-cidr-other",
        vpc_cidr: "10.100.0.0/16",
        secondary_cidr: None,
        subnet_cidrs: ["10.100.1.0/24", "10.100.2.0/24"],
        assigned: PREFERRED,
    },
    Row {
        case: "a VPC whose SECOND associated range overlaps the preferred block",
        cluster: "default-cidr-secondary",
        vpc_cidr: "10.40.0.0/16",
        secondary_cidr: Some("172.20.0.0/16"),
        subnet_cidrs: ["10.40.1.0/24", "10.40.2.0/24"],
        assigned: ALTERNATE,
    },
    Row {
        case: "a VPC in another private range altogether",
        cluster: "default-cidr-elsewhere",
        vpc_cidr: "192.168.0.0/16",
        secondary_cidr: None,
        subnet_cidrs: ["192.168.1.0/24", "192.168.2.0/24"],
        assigned: PREFERRED,
    },
];

/// Two CIDR blocks overlap exactly when one contains the other — read here without a
/// parser, by comparing the first `prefix` bits of the two networks, so the suite
/// checks the invariant independently of the service's own reading.
fn overlaps(left: &str, right: &str) -> bool {
    let (left_bits, left_prefix) = network_bits(left);
    let (right_bits, right_prefix) = network_bits(right);
    let shared = left_prefix.min(right_prefix);
    let mask = if shared == 0 {
        0
    } else {
        u32::MAX << (32 - shared)
    };
    left_bits & mask == right_bits & mask
}

/// `block` as its network address and prefix length.
fn network_bits(block: &str) -> (u32, u32) {
    let (address, prefix) = block
        .split_once('/')
        .expect("every block this suite names carries a netmask");
    let octets: Vec<u32> = address
        .split('.')
        .map(|octet| octet.parse().expect("an IPv4 octet"))
        .collect();
    let bits = octets
        .iter()
        .fold(0_u32, |packed, octet| (packed << 8) | octet);
    (bits, prefix.parse().expect("a netmask length"))
}

#[test]
fn eks_assigns_a_service_cidr_that_avoids_the_vpc_address_space() {
    std::thread::Builder::new()
        .name("eks-default-service-cidr".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                // `enable_io` and `enable_time`: creating a cluster opens the
                // core-owned listener its Kubernetes API server answers on, and the
                // clock owes the cluster its `ACTIVE` status.
                .enable_io()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            for row in ROWS {
                // Setup: the row's VPC and its two subnets, through EC2 itself.
                let created = ec2(
                    &router,
                    &runtime,
                    "CreateVpc",
                    &[("CidrBlock", row.vpc_cidr)],
                );
                assert_eq!(
                    created.status, 200,
                    "case {:?}: CreateVpc {} must succeed, got {created:?}",
                    row.case, row.vpc_cidr
                );
                let vpc_id = element(&created.text, "vpcId");
                let mut vpc_ranges = vec![row.vpc_cidr.to_string()];
                if let Some(secondary) = row.secondary_cidr {
                    let associated = ec2(
                        &router,
                        &runtime,
                        "AssociateVpcCidrBlock",
                        &[("VpcId", vpc_id.as_str()), ("CidrBlock", secondary)],
                    );
                    assert_eq!(
                        associated.status, 200,
                        "case {:?}: AssociateVpcCidrBlock {secondary} must succeed, got \
                         {associated:?}",
                        row.case
                    );
                    vpc_ranges.push(secondary.to_string());
                }
                // Each subnet in its OWN Availability Zone: a control plane is placed
                // across at least two zones.
                let zones = [format!("{REGION}a"), format!("{REGION}b")];
                let mut subnets: Vec<String> = Vec::new();
                for (cidr_block, zone) in row.subnet_cidrs.iter().zip(&zones) {
                    let answer = ec2(
                        &router,
                        &runtime,
                        "CreateSubnet",
                        &[
                            ("VpcId", vpc_id.as_str()),
                            ("CidrBlock", cidr_block),
                            ("AvailabilityZone", zone.as_str()),
                        ],
                    );
                    assert_eq!(
                        answer.status, 200,
                        "case {:?}: CreateSubnet {cidr_block} must succeed, got {answer:?}",
                        row.case
                    );
                    subnets.push(element(&answer.text, "subnetId"));
                }

                // A create that names NO `kubernetesNetworkConfig` at all, which is
                // what makes the service choose the block.
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    "/clusters",
                    Some(serde_json::json!({
                        "name": row.cluster,
                        "roleArn": role_arn("eksClusterRole"),
                        "resourcesVpcConfig": { "subnetIds": subnets },
                    })),
                );
                assert_eq!(
                    answer.status, 200,
                    "case {:?}: the create must be served, got {answer:?} ({})",
                    row.case,
                    answer.message()
                );
                let reported =
                    &answer.body["cluster"]["kubernetesNetworkConfig"]["serviceIpv4Cidr"];
                assert_eq!(
                    reported,
                    &serde_json::json!(row.assigned),
                    "case {:?}: CreateCluster must assign {}, got {}",
                    row.case,
                    row.assigned,
                    answer.text
                );
                assert!(
                    SERVICE_IPV4_CIDR_BLOCKS.contains(&row.assigned),
                    "case {:?}: the assigned block must be one the model documents, got {}",
                    row.case,
                    row.assigned
                );
                // The invariant behind the rule: the assigned block overlaps none of
                // the ranges EC2 reports for the VPC the cluster sits in.
                for range in &vpc_ranges {
                    assert!(
                        !overlaps(row.assigned, range),
                        "case {:?}: the assigned block {} must not overlap the VPC range \
                         {range}",
                        row.case,
                        row.assigned
                    );
                }

                // ROUND TRIP: the block is settled at create and read back from the
                // stored cluster, which also reports the VPC whose ranges decided it.
                let read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{}", row.cluster),
                    None,
                );
                assert_eq!(
                    read.status, 200,
                    "case {:?}: the cluster must read back, got {read:?}",
                    row.case
                );
                assert_eq!(
                    read.body["cluster"]["kubernetesNetworkConfig"]["serviceIpv4Cidr"],
                    serde_json::json!(row.assigned),
                    "case {:?}: DescribeCluster must report {}, got {}",
                    row.case,
                    row.assigned,
                    read.text
                );
                assert_eq!(
                    read.body["cluster"]["resourcesVpcConfig"]["vpcId"],
                    serde_json::json!(vpc_id),
                    "case {:?}: the cluster must sit in the VPC whose ranges were read, \
                     got {}",
                    row.case,
                    read.text
                );
            }
        })
        .expect("spawn the eks default-service-cidr thread")
        .join()
        .expect("the eks default-service-cidr thread");
}
