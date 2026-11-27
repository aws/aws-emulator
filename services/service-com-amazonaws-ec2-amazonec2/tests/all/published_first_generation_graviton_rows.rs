// SPDX-License-Identifier: CC0-1.0

//! The published EBS-optimized figures and host features of the FIRST generation AWS Graviton
//! general purpose family, driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rule under test: THE EBS-OPTIMIZED PERFORMANCE OF AN INSTANCE TYPE IS THE FIGURE AWS
//! PUBLISHES FOR THAT TYPE, NOT A FUNCTION OF ITS SIZE LADDER. `EbsInfo$EbsOptimizedInfo`
//! ("describes the optimized EBS performance for the instance type",
//! `smithy_query(query="doc",shape="EbsInfo",member="EbsOptimizedInfo")`) carries the four
//! figures of the type's row in the published "Amazon EBS optimized instance types" table —
//! the bandwidth and IOPS it sustains, and the bandwidth and IOPS it can reach — and the two
//! throughput members are those bandwidths in the other unit, a megabyte being eight megabits.
//! Neither the sustained figures of one family scale by the size multiplier nor do two
//! families of one size agree, and the ceiling a size bursts to is the family's own hardware
//! rather than one figure every Nitro type shares.
//!
//! The rows below span what the rule generalises over: the whole ladder of the first
//! generation Graviton family, whose host sustains far less than any later generation and
//! whose smaller sizes do not sustain a proportional share of it, beside the sizes of three
//! other families — a sixth generation Graviton general purpose one, a burstable one and a
//! fifth generation Intel one, none of which the blocked recordings cited — so no row is the
//! specification.
//!
//! The second test pins the host features that family publishes, which likewise follow the
//! type rather than its generation number: its volumes can be encrypted, it is offered on a
//! Dedicated Host and every placement group strategy, and its hardware carries neither a
//! virtual trusted platform module, nor an enclave, nor a local PTP hardware clock, and no
//! reboot migration.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, indexed};

/// The Region every call here is signed for; the catalog is the same in each.
const REGION: &str = "us-west-2";

/// The megabits in a megabyte, which is all that separates the `Mbps` members of
/// `EbsOptimizedInfo` from its `MB/s` ones.
const MEGABITS_PER_MEGABYTE: f64 = 8.0;

/// One row of the published EBS table: an instance type and its four figures.
#[derive(Debug, Clone, Copy)]
struct Published {
    /// `a1.xlarge` and the like.
    instance_type: &'static str,
    /// The bandwidth, in Mbps, the type sustains.
    baseline_mbps: i64,
    /// The I/O operations per second it sustains.
    baseline_iops: i64,
    /// The bandwidth, in Mbps, it can reach.
    maximum_mbps: i64,
    /// The I/O operations per second it can reach.
    maximum_iops: i64,
}

/// The published rows under test.
///
/// Source: the "Amazon EBS optimized instance types" table, one row per instance type — the
/// first generation Graviton ladder, whose two widest sizes sustain the whole host's 3,500
/// Mbps and 20,000 IOPS while every smaller size bursts up to them, and four sizes of three
/// other families whose figures are their own.
const ROWS: [Published; 10] = [
    Published {
        instance_type: "a1.medium",
        baseline_mbps: 300,
        baseline_iops: 2_500,
        maximum_mbps: 3_500,
        maximum_iops: 20_000,
    },
    Published {
        instance_type: "a1.large",
        baseline_mbps: 525,
        baseline_iops: 4_000,
        maximum_mbps: 3_500,
        maximum_iops: 20_000,
    },
    Published {
        instance_type: "a1.xlarge",
        baseline_mbps: 800,
        baseline_iops: 6_000,
        maximum_mbps: 3_500,
        maximum_iops: 20_000,
    },
    Published {
        instance_type: "a1.2xlarge",
        baseline_mbps: 1_750,
        baseline_iops: 10_000,
        maximum_mbps: 3_500,
        maximum_iops: 20_000,
    },
    Published {
        instance_type: "a1.4xlarge",
        baseline_mbps: 3_500,
        baseline_iops: 20_000,
        maximum_mbps: 3_500,
        maximum_iops: 20_000,
    },
    Published {
        instance_type: "a1.metal",
        baseline_mbps: 3_500,
        baseline_iops: 20_000,
        maximum_mbps: 3_500,
        maximum_iops: 20_000,
    },
    // Another Graviton general purpose family, four generations later: one size of its ladder
    // sustains more than the whole first generation host and bursts to a different ceiling.
    Published {
        instance_type: "m6g.medium",
        baseline_mbps: 315,
        baseline_iops: 2_500,
        maximum_mbps: 4_750,
        maximum_iops: 20_000,
    },
    Published {
        instance_type: "m6g.xlarge",
        baseline_mbps: 1_188,
        baseline_iops: 6_000,
        maximum_mbps: 4_750,
        maximum_iops: 20_000,
    },
    // A burstable size of the same vCPU count as `a1.medium`, and a fifth generation Intel
    // one: two more families whose four figures are neither each other's nor the ladder's.
    Published {
        instance_type: "t3.micro",
        baseline_mbps: 87,
        baseline_iops: 500,
        maximum_mbps: 2_085,
        maximum_iops: 11_800,
    },
    Published {
        instance_type: "t4g.large",
        baseline_mbps: 695,
        baseline_iops: 4_000,
        maximum_mbps: 2_780,
        maximum_iops: 15_700,
    },
];

/// The first generation Graviton sizes, whose host features the second test pins.
const FIRST_GENERATION_GRAVITON: [&str; 6] = [
    "a1.medium",
    "a1.large",
    "a1.xlarge",
    "a1.2xlarge",
    "a1.4xlarge",
    "a1.metal",
];

/// The body `DescribeInstanceTypes` answers for exactly one instance type.
async fn described(case: &str, router: &MultiServiceRouter, instance_type: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstanceTypes",
        &indexed("InstanceType", &[instance_type]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for {instance_type}, got {status} with body {body}"
    );
    body
}

/// The `<name>` block of a reported type — everything between its tags — or a panic naming
/// the case.
fn block(case: &str, xml: &str, name: &str) -> String {
    element(xml, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> reported, got {xml}"))
}

/// One member of a block as a number.
fn number(case: &str, xml: &str, name: &str) -> f64 {
    let text = block(case, xml, name);
    text.parse()
        .unwrap_or_else(|_| panic!("case {case:?}: <{name}> must be a number, got {text:?}"))
}

/// Rule: the four EBS figures a type reports are the ones AWS publishes for THAT type, and the
/// throughput members are the bandwidth ones in megabytes.
#[tokio::test]
async fn the_ebs_optimized_figures_are_the_published_row_of_the_type() {
    let (router, _engine) = super::ec2_query::emulator();

    for row in ROWS {
        let case = row.instance_type;
        let body = described(case, &router, row.instance_type).await;
        // Read inside the type's own `<ebsOptimizedInfo>`: a card of a type that publishes EBS
        // hardware of its own carries members of the same names.
        let reported = block(case, &body, "ebsOptimizedInfo");
        for (name, expected) in [
            ("baselineBandwidthInMbps", row.baseline_mbps),
            ("baselineIops", row.baseline_iops),
            ("maximumBandwidthInMbps", row.maximum_mbps),
            ("maximumIops", row.maximum_iops),
        ] {
            let actual = number(case, &reported, name);
            let expected = expected as f64;
            assert!(
                (actual - expected).abs() < f64::EPSILON,
                "case {case:?}: expected {name} {expected}, got {actual}"
            );
        }
        for (name, bandwidth) in [
            ("baselineThroughputInMBps", row.baseline_mbps),
            ("maximumThroughputInMBps", row.maximum_mbps),
        ] {
            let actual = number(case, &reported, name);
            let expected = bandwidth as f64 / MEGABITS_PER_MEGABYTE;
            assert!(
                (actual - expected).abs() < f64::EPSILON,
                "case {case:?}: expected {name} {expected}, got {actual}"
            );
        }
    }
}

/// Rule: the host features of an instance type are published per type too — the first
/// generation Graviton family offers EBS encryption, Dedicated Hosts and every placement group
/// strategy, and offers neither NitroTPM, nor Nitro Enclaves, nor a PTP hardware clock, while
/// reboot migration is the hypervisor's to perform, so its virtualized sizes are offered it and
/// its bare metal size — which has no hypervisor under it — is not.
#[tokio::test]
async fn the_first_generation_graviton_host_features_are_the_published_ones() {
    let (router, _engine) = super::ec2_query::emulator();

    for instance_type in FIRST_GENERATION_GRAVITON {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let ebs = block(case, &body, "ebsInfo");
        let encryption = block(case, &ebs, "encryptionSupport");
        assert_eq!(
            encryption, "supported",
            "case {case:?}: expected ebsInfo encryptionSupport \"supported\", got \
             {encryption:?}"
        );
        for (name, expected) in [
            ("dedicatedHostsSupported", "true"),
            ("nitroTpmSupport", "unsupported"),
            ("nitroEnclavesSupport", "unsupported"),
            ("phcSupport", "unsupported"),
            (
                "rebootMigrationSupport",
                if instance_type.ends_with(".metal") {
                    "unsupported"
                } else {
                    "supported"
                },
            ),
        ] {
            let actual = block(case, &body, name);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {name} {expected:?}, got {actual:?}"
            );
        }
        let placement = block(case, &body, "placementGroupInfo");
        let strategies = elements(&placement, "item");
        assert_eq!(
            strategies,
            vec![
                "cluster".to_string(),
                "partition".to_string(),
                "spread".to_string()
            ],
            "case {case:?}: expected every placement group strategy, got {strategies:?}"
        );
        // IPv6 is offered on the family, so an interface of one of its instances takes as many
        // IPv6 addresses as it takes private IPv4 ones.
        let network = block(case, &body, "networkInfo");
        let ipv6 = block(case, &network, "ipv6Supported");
        assert_eq!(
            ipv6, "true",
            "case {case:?}: expected ipv6Supported \"true\", got {ipv6:?}"
        );
        let ipv4_addresses = number(case, &network, "ipv4AddressesPerInterface");
        let ipv6_addresses = number(case, &network, "ipv6AddressesPerInterface");
        assert!(
            (ipv4_addresses - ipv6_addresses).abs() < f64::EPSILON,
            "case {case:?}: expected ipv6AddressesPerInterface {ipv4_addresses}, got \
             {ipv6_addresses}"
        );
    }
}
