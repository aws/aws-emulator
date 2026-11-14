// SPDX-License-Identifier: CC0-1.0

//! The per-instance-type EBS, instance store, network and host-feature rows the catalog
//! publishes for a compute optimized family with local NVMe devices and for a block storage
//! optimized one — driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rule under test: EVERY HARDWARE MEMBER `DescribeInstanceTypes` REPORTS IS THE FIGURE
//! AWS PUBLISHES FOR THAT ONE INSTANCE TYPE, not a function of the size multiplier in its
//! name. The EBS bandwidth and IOPS a type sustains and the figures it reaches
//! (`EbsInfo$EbsOptimizedInfo`), how many volumes it holds attached and whether that limit is
//! shared with its other devices or its own (`$MaximumEbsAttachments` / `$AttachmentLimitType`
//! / `$MaximumEbsCards` / `$EbsCards`), its local devices (`InstanceStorageInfo`), its network
//! performance, its interface and address limits, its Elastic Fabric Adapters, its processor's
//! sustained frequency, the idle timeouts its connection tracking applies
//! (`NetworkInfo$ConnectionTrackingConfiguration`) and the host features it is offered
//! (Dedicated Hosts, hibernation, reboot migration, a PTP hardware clock, an enclave) are each
//! a published fact of the type.
//!
//! The tables below span what the rule generalises over: the two families differ in
//! processor, in hypervisor generation, in having local devices at all and in whether their
//! EBS limit is shared or dedicated; inside each family every size of the ladder is a row, so
//! no one size is the specification. The figures are the ones AWS states for each type on its
//! family's instance-type specification page, in the "Amazon EBS optimized instance types"
//! and "Instance volume limits" tables and in the "IP addresses per network interface per
//! instance type" table of the EC2 User Guide.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, indexed};

/// The Region every call here is signed for; the catalog is the same in each.
const REGION: &str = "us-east-1";

/// The megabits in a megabyte, which is all that separates the `Mbps` figures of
/// `EbsOptimizedInfo` and `EbsCardInfo` from their `MB/s` ones.
const MEGABITS_PER_MEGABYTE: f64 = 8.0;

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

/// One member of a reported type, or a panic naming the case when it is absent.
fn required(case: &str, xml: &str, name: &str) -> String {
    element(xml, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> reported, got {xml}"))
}

/// One member of a reported type as a number.
fn number(case: &str, xml: &str, name: &str) -> f64 {
    let text = required(case, xml, name);
    text.parse()
        .unwrap_or_else(|_| panic!("case {case:?}: <{name}> must be a number, got {text:?}"))
}

/// The published row of one instance type: the figures a client sizes its storage and its
/// network from.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// `c5ad.8xlarge` and the like.
    instance_type: &'static str,
    /// (sustained Mbps, sustained IOPS, Mbps it reaches, IOPS it reaches).
    ebs: (i64, i64, i64, i64),
    /// The EBS volumes it holds attached, and whether that limit is shared with its
    /// interfaces and local devices or is the volumes' own.
    ebs_attachments: (i64, &'static str),
    /// Its local devices as (count, GB each), or `None` where it carries none at all.
    instance_store: Option<(i64, i64)>,
    /// Its network performance, as the published table writes it.
    network_performance: &'static str,
    /// Its processor's maker and that processor's sustained frequency, in GHz.
    processor: (&'static str, f64),
}

/// Every size of the two families, with the figures AWS publishes for each.
///
/// C5ad: one EPYC 7R32 at 3.3 GHz, local NVMe devices whose count and size are the type's
/// own, EBS figures dedicated from `8xlarge` up and, below it, a baseline of its own that
/// bursts to the dedicated one, and a volume limit that is what the shared attachment pool
/// leaves once its devices are counted.
/// C8gb: Graviton4 with EBS hardware of its own — a dedicated volume limit, and bandwidth and
/// IOPS an order of magnitude above the older family's at the same size.
const ROWS: [Row; 18] = [
    Row {
        instance_type: "c5ad.large",
        ebs: (200, 800, 3_170, 13_300),
        ebs_attachments: (26, "shared"),
        instance_store: Some((1, 75)),
        network_performance: "Up to 10 Gigabit",
        processor: ("AMD", 3.3),
    },
    Row {
        instance_type: "c5ad.xlarge",
        ebs: (400, 1_600, 3_170, 13_300),
        ebs_attachments: (26, "shared"),
        instance_store: Some((1, 150)),
        network_performance: "Up to 10 Gigabit",
        processor: ("AMD", 3.3),
    },
    Row {
        instance_type: "c5ad.2xlarge",
        ebs: (800, 3_200, 3_170, 13_300),
        ebs_attachments: (26, "shared"),
        instance_store: Some((1, 300)),
        network_performance: "Up to 10 Gigabit",
        processor: ("AMD", 3.3),
    },
    Row {
        instance_type: "c5ad.4xlarge",
        ebs: (1_580, 6_320, 3_170, 13_300),
        ebs_attachments: (25, "shared"),
        instance_store: Some((2, 300)),
        network_performance: "Up to 10 Gigabit",
        processor: ("AMD", 3.3),
    },
    Row {
        instance_type: "c5ad.8xlarge",
        ebs: (3_170, 13_300, 3_170, 13_300),
        ebs_attachments: (25, "shared"),
        instance_store: Some((2, 600)),
        network_performance: "10 Gigabit",
        processor: ("AMD", 3.3),
    },
    Row {
        instance_type: "c5ad.12xlarge",
        ebs: (4_750, 20_000, 4_750, 20_000),
        ebs_attachments: (25, "shared"),
        instance_store: Some((2, 900)),
        network_performance: "12 Gigabit",
        processor: ("AMD", 3.3),
    },
    Row {
        instance_type: "c5ad.16xlarge",
        ebs: (6_300, 26_700, 6_300, 26_700),
        ebs_attachments: (25, "shared"),
        instance_store: Some((2, 1_200)),
        network_performance: "20 Gigabit",
        processor: ("AMD", 3.3),
    },
    Row {
        instance_type: "c5ad.24xlarge",
        ebs: (9_500, 40_000, 9_500, 40_000),
        ebs_attachments: (25, "shared"),
        instance_store: Some((2, 1_900)),
        network_performance: "20 Gigabit",
        processor: ("AMD", 3.3),
    },
    Row {
        instance_type: "c8gb.medium",
        ebs: (1_562, 7_500, 25_000, 120_000),
        ebs_attachments: (32, "dedicated"),
        instance_store: None,
        network_performance: "Up to 12.5 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.large",
        ebs: (3_125, 15_000, 25_000, 120_000),
        ebs_attachments: (32, "dedicated"),
        instance_store: None,
        network_performance: "Up to 12.5 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.xlarge",
        ebs: (6_250, 30_000, 25_000, 120_000),
        ebs_attachments: (32, "dedicated"),
        instance_store: None,
        network_performance: "Up to 12.5 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.2xlarge",
        ebs: (12_500, 60_000, 25_000, 120_000),
        ebs_attachments: (32, "dedicated"),
        instance_store: None,
        network_performance: "Up to 15 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.4xlarge",
        ebs: (25_000, 120_000, 25_000, 120_000),
        ebs_attachments: (32, "dedicated"),
        instance_store: None,
        network_performance: "Up to 15 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.8xlarge",
        ebs: (50_000, 240_000, 50_000, 240_000),
        ebs_attachments: (32, "dedicated"),
        instance_store: None,
        network_performance: "15 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.12xlarge",
        ebs: (75_000, 360_000, 75_000, 360_000),
        ebs_attachments: (32, "dedicated"),
        instance_store: None,
        network_performance: "22.5 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.16xlarge",
        ebs: (100_000, 480_000, 100_000, 480_000),
        ebs_attachments: (48, "dedicated"),
        instance_store: None,
        network_performance: "30 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.24xlarge",
        ebs: (150_000, 720_000, 150_000, 720_000),
        ebs_attachments: (64, "dedicated"),
        instance_store: None,
        network_performance: "40 Gigabit",
        processor: ("AWS", 2.8),
    },
    Row {
        instance_type: "c8gb.48xlarge",
        ebs: (300_000, 1_440_000, 300_000, 1_440_000),
        ebs_attachments: (128, "dedicated"),
        instance_store: None,
        network_performance: "50 Gigabit",
        processor: ("AWS", 2.8),
    },
];

/// Rule: the EBS, instance store, network and processor members a type reports are the
/// figures AWS publishes for THAT type.
#[tokio::test]
async fn published_rows_report_the_hardware_facts_of_the_type() {
    let (router, _engine) = super::ec2_query::emulator();

    for row in ROWS {
        let case = row.instance_type;
        let body = described(case, &router, row.instance_type).await;
        let ebs_info = required(case, &body, "ebsInfo");
        let performance = required(case, &ebs_info, "ebsOptimizedInfo");
        let (baseline_mbps, baseline_iops, maximum_mbps, maximum_iops) = row.ebs;
        for (name, expected) in [
            ("baselineBandwidthInMbps", baseline_mbps),
            ("baselineIops", baseline_iops),
            ("maximumBandwidthInMbps", maximum_mbps),
            ("maximumIops", maximum_iops),
        ] {
            let reported = number(case, &performance, name) as i64;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {name} {expected}, got {reported}"
            );
        }
        // The throughput members are the bandwidth ones in the other unit, which is how the
        // published table's Mbps and MB/s columns relate.
        for (name, bandwidth) in [
            ("baselineThroughputInMBps", baseline_mbps),
            ("maximumThroughputInMBps", maximum_mbps),
        ] {
            let expected = bandwidth as f64 / MEGABITS_PER_MEGABYTE;
            let reported = number(case, &performance, name);
            assert!(
                (reported - expected).abs() < 0.001,
                "case {case:?}: expected {name} {expected}, got {reported}"
            );
        }

        let (attachments, limit_type) = row.ebs_attachments;
        let reported = number(case, &ebs_info, "maximumEbsAttachments") as i64;
        assert_eq!(
            reported, attachments,
            "case {case:?}: expected maximumEbsAttachments {attachments}, got {reported}"
        );
        let reported = required(case, &ebs_info, "attachmentLimitType");
        assert_eq!(
            reported, limit_type,
            "case {case:?}: expected attachmentLimitType {limit_type:?}, got {reported:?}"
        );

        let storage = element(&body, "instanceStorageInfo");
        match row.instance_store {
            Some((devices, gb_each)) => {
                let storage = storage.unwrap_or_else(|| {
                    panic!("case {case:?}: expected an instanceStorageInfo, got {body}")
                });
                let disks = required(case, &storage, "disks");
                let reported = number(case, &disks, "count") as i64;
                assert_eq!(
                    reported, devices,
                    "case {case:?}: expected {devices} local devices, got {reported}"
                );
                let reported = number(case, &disks, "sizeInGB") as i64;
                assert_eq!(
                    reported, gb_each,
                    "case {case:?}: expected local devices of {gb_each} GB, got {reported}"
                );
                let reported = number(case, &storage, "totalSizeInGB") as i64;
                assert_eq!(
                    reported,
                    devices * gb_each,
                    "case {case:?}: expected totalSizeInGB {}, got {reported}",
                    devices * gb_each
                );
            }
            None => assert!(
                storage.is_none(),
                "case {case:?}: a type with no local devices reports no instanceStorageInfo, \
                 got {storage:?}"
            ),
        }

        let network_info = required(case, &body, "networkInfo");
        let reported = required(case, &network_info, "networkPerformance");
        assert_eq!(
            reported, row.network_performance,
            "case {case:?}: expected networkPerformance {:?}, got {reported:?}",
            row.network_performance
        );

        let processor_info = required(case, &body, "processorInfo");
        let (manufacturer, sustained_ghz) = row.processor;
        let reported = required(case, &processor_info, "manufacturer");
        assert_eq!(
            reported, manufacturer,
            "case {case:?}: expected processor manufacturer {manufacturer:?}, got {reported:?}"
        );
        let reported = number(case, &processor_info, "sustainedClockSpeedInGhz");
        assert!(
            (reported - sustained_ghz).abs() < 0.001,
            "case {case:?}: expected sustainedClockSpeedInGhz {sustained_ghz}, got {reported}"
        );
    }
}

/// Rule, as an invariant over every row: a type sustains no more than it reaches, and the
/// figures do not follow the size multiplier — one family's sizes share a dedicated figure
/// while the other's grow with the size, and the two families disagree at the same size.
#[tokio::test]
async fn the_published_figures_are_bounded_and_do_not_follow_the_size_multiplier() {
    let (router, _engine) = super::ec2_query::emulator();

    let baseline = |body: &str, case: &str| -> (i64, i64) {
        let ebs_info = required(case, body, "ebsInfo");
        let performance = required(case, &ebs_info, "ebsOptimizedInfo");
        (
            number(case, &performance, "baselineBandwidthInMbps") as i64,
            number(case, &performance, "maximumBandwidthInMbps") as i64,
        )
    };

    for row in ROWS {
        let case = row.instance_type;
        let body = described(case, &router, row.instance_type).await;
        let (sustained, reached) = baseline(&body, case);
        assert!(
            sustained > 0 && sustained <= reached,
            "case {case:?}: a type sustains at most what it reaches, got {sustained}/{reached}"
        );
    }

    // One family, two sizes, and a sustained figure that does NOT double with them: the
    // ladder doubles the vCPUs from the 8xlarge to the 16xlarge of the older family while its
    // dedicated EBS figure grows by less.
    let left = described("c5ad.8xlarge", &router, "c5ad.8xlarge").await;
    let right = described("c5ad.16xlarge", &router, "c5ad.16xlarge").await;
    let (small, _) = baseline(&left, "c5ad.8xlarge");
    let (large, _) = baseline(&right, "c5ad.16xlarge");
    assert!(
        large < small * 2,
        "case \"c5ad\": the family's dedicated figures do not scale with the multiplier, got \
         {small} then {large}"
    );

    // Two families, ONE size: every figure of the pair differs, so the size cannot be what
    // produced either.
    let left = described("c5ad.16xlarge", &router, "c5ad.16xlarge").await;
    let right = described("c8gb.16xlarge", &router, "c8gb.16xlarge").await;
    assert_ne!(
        baseline(&left, "c5ad.16xlarge"),
        baseline(&right, "c8gb.16xlarge"),
        "two families of one size publish different EBS figures"
    );
    let left_network = required("c5ad.16xlarge", &left, "networkInfo");
    let right_network = required("c8gb.16xlarge", &right, "networkInfo");
    assert_ne!(
        required("c5ad.16xlarge", &left_network, "networkPerformance"),
        required("c8gb.16xlarge", &right_network, "networkPerformance"),
        "two families of one size publish different network performance"
    );
}

/// Rule: a type whose EBS volumes are attached to hardware of its own reports that hardware —
/// how many cards it has and what each one carries — and a type whose volumes come out of the
/// host's shared pool reports no cards at all.
#[tokio::test]
async fn ebs_cards_are_reported_for_the_types_that_carry_them() {
    let (router, _engine) = super::ec2_query::emulator();

    // The largest size of the block storage optimized family spreads its 300,000 Mbps and
    // 1,440,000 IOPS over two cards; every other row here has one card's worth or none.
    let case = "c8gb.48xlarge";
    let body = described(case, &router, case).await;
    let ebs_info = required(case, &body, "ebsInfo");
    let cards = number(case, &ebs_info, "maximumEbsCards") as i64;
    assert_eq!(
        cards, 2,
        "case {case:?}: expected maximumEbsCards 2, got {cards}"
    );
    let card_set = required(case, &ebs_info, "ebsCardSet");
    let items = elements(&card_set, "item");
    assert_eq!(
        items.len(),
        2,
        "case {case:?}: expected one ebsCardSet item per card, got {items:?}"
    );
    for (index, item) in items.iter().enumerate() {
        let reported = number(case, item, "ebsCardIndex") as i64;
        assert_eq!(
            reported, index as i64,
            "case {case:?}: expected ebsCardIndex {index}, got {reported}"
        );
        for (name, whole) in [
            ("baselineBandwidthInMbps", 300_000_i64),
            ("baselineIops", 1_440_000),
            ("maximumBandwidthInMbps", 300_000),
            ("maximumIops", 1_440_000),
        ] {
            let reported = number(case, item, name) as i64;
            assert_eq!(
                reported,
                whole / 2,
                "case {case:?}: expected card {index} to carry {name} {}, got {reported}",
                whole / 2
            );
        }
        let reported = number(case, item, "baselineThroughputInMBps");
        let expected = 150_000.0 / MEGABITS_PER_MEGABYTE;
        assert!(
            (reported - expected).abs() < 0.001,
            "case {case:?}: expected card {index} throughput {expected}, got {reported}"
        );
    }

    // Rows: a size of the same family below the one with its own cards, and two sizes of the
    // family whose volumes share the host's attachment pool.
    for instance_type in ["c8gb.24xlarge", "c5ad.8xlarge", "c5ad.24xlarge"] {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let ebs_info = required(case, &body, "ebsInfo");
        for name in ["maximumEbsCards", "ebsCardSet"] {
            let reported = element(&ebs_info, name);
            assert!(
                reported.is_none(),
                "case {case:?}: a type with no EBS cards of its own reports no <{name}>, got \
                 {reported:?}"
            );
        }
    }
}

/// Rule: the connection-tracking idle timeouts, the interface and address limits, the Elastic
/// Fabric Adapters and the host features a type is offered are published per type.
#[tokio::test]
async fn connection_tracking_limits_and_host_features_are_published_per_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: the established-TCP idle timeout of the two families — the newer hardware
    // generation holds a connection for 350 s where the older holds the 432,000 s the EC2
    // User Guide's "Connection tracking timeouts" publishes — with the UDP timeouts, which
    // both generations share, read on the same call.
    let timeout_rows = [
        ("c8gb.medium", 350),
        ("c8gb.4xlarge", 350),
        ("c8gb.48xlarge", 350),
        ("c5ad.large", 432_000),
        ("c5ad.24xlarge", 432_000),
    ];
    for (instance_type, expected) in timeout_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let conntrack = required(case, &network_info, "connectionTrackingConfiguration");
        let reported = number(case, &conntrack, "defaultTcpEstablishedTimeout") as i64;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected defaultTcpEstablishedTimeout {expected}, got {reported}"
        );
        for (name, shared) in [("defaultUdpTimeout", 30), ("defaultUdpStreamTimeout", 180)] {
            let reported = number(case, &conntrack, name) as i64;
            assert_eq!(
                reported, shared,
                "case {case:?}: expected {name} {shared}, got {reported}"
            );
        }
    }

    // Rows: the smallest size of the newer family carries the two interfaces of four
    // addresses its row publishes, rather than the three of six its step of the size ladder
    // gives; the sizes above it take the ladder's figures.
    let interface_rows = [
        ("c8gb.medium", 2, 4),
        ("c8gb.large", 3, 10),
        ("c8gb.xlarge", 4, 15),
        ("c5ad.large", 3, 10),
    ];
    for (instance_type, interfaces, addresses) in interface_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let reported = number(case, &network_info, "maximumNetworkInterfaces") as i64;
        assert_eq!(
            reported, interfaces,
            "case {case:?}: expected maximumNetworkInterfaces {interfaces}, got {reported}"
        );
        for name in ["ipv4AddressesPerInterface", "ipv6AddressesPerInterface"] {
            let reported = number(case, &network_info, name) as i64;
            assert_eq!(
                reported, addresses,
                "case {case:?}: expected {name} {addresses}, got {reported}"
            );
        }
    }

    // The largest size of the newer family carries a network card per processor socket, and
    // every smaller size one card; the first card is the default on every type.
    for (instance_type, cards) in [
        ("c8gb.48xlarge", 2),
        ("c8gb.24xlarge", 1),
        ("c5ad.24xlarge", 1),
    ] {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let reported = number(case, &network_info, "maximumNetworkCards") as i64;
        assert_eq!(
            reported, cards,
            "case {case:?}: expected maximumNetworkCards {cards}, got {reported}"
        );
        let card_set = required(case, &network_info, "networkCards");
        let indexes = elements(&card_set, "networkCardIndex");
        assert_eq!(
            indexes.len(),
            cards as usize,
            "case {case:?}: expected one networkCards item per card, got {indexes:?}"
        );
        let reported = number(case, &network_info, "defaultNetworkCardIndex") as i64;
        assert_eq!(
            reported, 0,
            "case {case:?}: expected defaultNetworkCardIndex 0, got {reported}"
        );
    }

    // Rows: the host features, each a flag a client branches on, over both families — an
    // Elastic Fabric Adapter on the three largest sizes of the newer family and on none of
    // the older one, ENA Express from the size of that family which carries a full network
    // share up (the `4xlarge`), no interface attachments from a secondary network (so no
    // address at all on
    // one) and no Traffic Mirroring on that family while the older one takes it, Dedicated
    // Hosts and hibernation offered on neither size of the older family, reboot migration on
    // the newer hardware only, a PTP hardware clock on neither, and an enclave offered from
    // two vCPUs up on that hardware while a single vCPU is too few for one.
    let feature_rows = [
        ("c8gb.12xlarge", "efaSupported", "false"),
        ("c8gb.16xlarge", "efaSupported", "true"),
        ("c8gb.24xlarge", "efaSupported", "true"),
        ("c8gb.48xlarge", "efaSupported", "true"),
        ("c5ad.24xlarge", "efaSupported", "false"),
        ("c8gb.2xlarge", "enaSrdSupported", "false"),
        ("c8gb.4xlarge", "enaSrdSupported", "true"),
        ("c8gb.8xlarge", "enaSrdSupported", "true"),
        ("c8gb.16xlarge", "enaSrdSupported", "true"),
        ("c8gb.medium", "secondaryNetworkSupported", "false"),
        ("c8gb.48xlarge", "secondaryNetworkSupported", "false"),
        ("c8gb.medium", "ipv4AddressesPerSecondaryInterface", "0"),
        ("c8gb.12xlarge", "ipv4AddressesPerSecondaryInterface", "0"),
        ("c5ad.large", "dedicatedHostsSupported", "false"),
        ("c5ad.24xlarge", "dedicatedHostsSupported", "false"),
        ("c8gb.16xlarge", "dedicatedHostsSupported", "true"),
        ("c5ad.large", "hibernationSupported", "false"),
        ("c5ad.8xlarge", "hibernationSupported", "false"),
        ("c5ad.2xlarge", "rebootMigrationSupport", "unsupported"),
        ("c5ad.16xlarge", "rebootMigrationSupport", "unsupported"),
        ("c8gb.2xlarge", "rebootMigrationSupport", "supported"),
        ("c5ad.4xlarge", "phcSupport", "unsupported"),
        ("c8gb.4xlarge", "phcSupport", "unsupported"),
        ("c8gb.medium", "nitroEnclavesSupport", "unsupported"),
        ("c8gb.large", "nitroEnclavesSupport", "supported"),
        ("c8gb.xlarge", "nitroEnclavesSupport", "supported"),
        ("c8gb.2xlarge", "trafficMirroringSupport", "unsupported"),
        ("c8gb.48xlarge", "trafficMirroringSupport", "unsupported"),
        ("c5ad.8xlarge", "trafficMirroringSupport", "supported"),
    ];
    for (instance_type, member, expected) in feature_rows {
        let case = format!("{instance_type} {member}");
        let body = described(&case, &router, instance_type).await;
        // A member of networkInfo is read out of that block; the rest are the type's own.
        let scope = if member.starts_with("efa")
            || member.starts_with("ena")
            || member.starts_with("secondaryNetwork")
            || member.starts_with("ipv4")
            || member.starts_with("trafficMirroring")
        {
            required(&case, &body, "networkInfo")
        } else {
            body
        };
        let reported = required(&case, &scope, member);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {member} {expected:?}, got {reported:?}"
        );
    }

    // The three sizes offered an adapter report its count too, and a size offered none
    // reports no efaInfo at all.
    for (instance_type, adapters) in [("c8gb.16xlarge", Some(1)), ("c8gb.8xlarge", None)] {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let reported = element(&network_info, "efaInfo");
        match adapters {
            Some(count) => {
                let info = reported.unwrap_or_else(|| {
                    panic!("case {case:?}: a type offered an adapter reports its efaInfo, got \
                            {network_info}")
                });
                let reported = number(case, &info, "maximumEfaInterfaces") as i64;
                assert_eq!(
                    reported, count,
                    "case {case:?}: expected maximumEfaInterfaces {count}, got {reported}"
                );
            }
            None => assert!(
                reported.is_none(),
                "case {case:?}: a type offered no adapter reports no efaInfo, got {reported:?}"
            ),
        }
    }
}
