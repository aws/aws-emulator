// SPDX-License-Identifier: CC0-1.0

//! The per-instance-type rows the catalog publishes for the previous generation, the
//! inference accelerated and the memory optimized families, and the member set EVERY
//! reported instance type carries — driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rules under test:
//!
//! 1. AN INSTANCE TYPE'S ATTRIBUTES ARE THE FACTS AWS PUBLISHES FOR THAT TYPE, NOT A
//!    FUNCTION OF THE SIZE MULTIPLIER IN ITS NAME: its memory, its EBS bandwidth and IOPS
//!    and how many volumes it holds, its local devices (including a type that carries NONE
//!    where its family's name reads as having them), its network performance, its
//!    processor's sustained frequency, its accelerators, whether its family is the latest
//!    generation, which network adapter features it is offered and which boot modes it
//!    takes. Two sizes of one family do not scale by the multiplier and two families of one
//!    size do not agree.
//! 2. EVERY REPORTED TYPE CARRIES THE WHOLE MEMBER SET, whatever hardware it runs on: the
//!    burst model of its vCPUs, the default connection-tracking timeouts, the network card
//!    inventory and its index, ENA and ENA Express support, the flexible-queue and
//!    secondary-network flags, Traffic Mirroring, the PTP clock and reboot migration. A
//!    pre-Nitro type reports them as much as a Nitro one.
//! 3. THE CORE COUNTS A TYPE CAN BE CONFIGURED WITH ARE PUBLISHED PER TYPE: a type whose
//!    cores sit one per processor socket reports the multiples of those sockets up to its
//!    default, and a type that takes any count reports every count.
//!
//! Every table spans what the rules generalise over — the family, the size, the hypervisor,
//! the burstable and the bare metal size, a type with local devices, one without and one
//! whose family name reads as having them, an accelerated type, and a type the published
//! table holds no row for at all — so no single recorded call is the specification. The
//! published figures in the tables are the ones AWS states for each type on its family's
//! instance-type specification page, in the "Amazon EBS optimized instance types" and
//! "Instance volume limits" tables, and in the CPU options table of the EC2 User Guide.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, indexed};

/// The Region every call here is signed for; the catalog is the same in each.
const REGION: &str = "us-east-1";

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
    element(xml, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> reported, got {xml}"))
}

/// One member of a reported type as a number.
fn number(case: &str, xml: &str, name: &str) -> f64 {
    let text = required(case, xml, name);
    text.parse()
        .unwrap_or_else(|_| panic!("case {case:?}: <{name}> must be a number, got {text:?}"))
}

/// The published attributes of one instance type.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// `c4.large` and the like.
    instance_type: &'static str,
    /// Is the family the latest generation of its own?
    current_generation: bool,
    /// Its memory, in MiB.
    memory_mib: i64,
    /// (sustained Mbps, sustained IOPS, Mbps it reaches, IOPS it reaches).
    ebs: (i64, i64, i64, i64),
    /// The EBS volumes it holds attached, and whether that limit is shared or dedicated.
    ebs_attachments: (i64, &'static str),
    /// Its local devices as (count, GB each), or `None` where it carries none at all.
    instance_store: Option<(i64, i64)>,
    /// Its network performance, as the published table writes it.
    network_performance: &'static str,
    /// Its processor's sustained all-core frequency, in GHz.
    sustained_clock_speed_ghz: f64,
}

/// Rule 1: the EBS, memory, instance store, network and processor members a type reports are
/// the figures AWS publishes for THAT type.
#[tokio::test]
async fn published_rows_report_the_facts_of_the_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: four sizes of a previous generation compute optimized family whose EBS figures
    // are dedicated per size and whose memory is not a whole number of GiB per vCPU; an
    // inference accelerated type with no local store at all; and four sizes of a memory
    // optimized family with local NVMe, including the bare metal one.
    let rows = [
        Row {
            instance_type: "c4.large",
            current_generation: false,
            memory_mib: 3_840,
            ebs: (500, 4_000, 500, 4_000),
            ebs_attachments: (40, "shared"),
            instance_store: None,
            network_performance: "Moderate",
            sustained_clock_speed_ghz: 2.9,
        },
        Row {
            instance_type: "c4.xlarge",
            current_generation: false,
            memory_mib: 7_680,
            ebs: (750, 6_000, 750, 6_000),
            ebs_attachments: (40, "shared"),
            instance_store: None,
            network_performance: "High",
            sustained_clock_speed_ghz: 2.9,
        },
        Row {
            instance_type: "c4.4xlarge",
            current_generation: false,
            memory_mib: 30_720,
            ebs: (2_000, 16_000, 2_000, 16_000),
            ebs_attachments: (40, "shared"),
            instance_store: None,
            network_performance: "High",
            sustained_clock_speed_ghz: 2.9,
        },
        Row {
            instance_type: "c4.8xlarge",
            current_generation: false,
            memory_mib: 61_440,
            ebs: (4_000, 32_000, 4_000, 32_000),
            ebs_attachments: (40, "shared"),
            instance_store: None,
            network_performance: "10 Gigabit",
            sustained_clock_speed_ghz: 2.9,
        },
        Row {
            instance_type: "dl2q.24xlarge",
            current_generation: true,
            memory_mib: 786_432,
            ebs: (19_000, 80_000, 19_000, 80_000),
            ebs_attachments: (19, "shared"),
            instance_store: None,
            network_performance: "100 Gigabit",
            sustained_clock_speed_ghz: 3.5,
        },
        Row {
            instance_type: "x2iedn.xlarge",
            current_generation: true,
            memory_mib: 131_072,
            ebs: (2_500, 8_125, 20_000, 65_000),
            // The shared Nitro pool less the primary interface and the one local device.
            ebs_attachments: (26, "shared"),
            instance_store: Some((1, 118)),
            network_performance: "Up to 25 Gigabit",
            sustained_clock_speed_ghz: 3.5,
        },
        Row {
            instance_type: "x2iedn.8xlarge",
            current_generation: true,
            memory_mib: 1_048_576,
            ebs: (20_000, 65_000, 20_000, 65_000),
            ebs_attachments: (26, "shared"),
            instance_store: Some((1, 950)),
            network_performance: "25 Gigabit",
            sustained_clock_speed_ghz: 3.5,
        },
        Row {
            instance_type: "x2iedn.24xlarge",
            current_generation: true,
            memory_mib: 3_145_728,
            ebs: (60_000, 195_000, 60_000, 195_000),
            ebs_attachments: (25, "shared"),
            instance_store: Some((2, 1_425)),
            network_performance: "75 Gigabit",
            sustained_clock_speed_ghz: 3.5,
        },
        Row {
            instance_type: "x2iedn.metal",
            current_generation: true,
            memory_mib: 4_194_304,
            ebs: (80_000, 260_000, 80_000, 260_000),
            ebs_attachments: (31, "shared"),
            instance_store: Some((2, 1_900)),
            network_performance: "100 Gigabit",
            sustained_clock_speed_ghz: 3.5,
        },
    ];

    for row in rows {
        let case = row.instance_type;
        let body = described(case, &router, row.instance_type).await;

        let reported = required(case, &body, "currentGeneration");
        assert_eq!(
            reported,
            row.current_generation.to_string(),
            "case {case:?}: expected currentGeneration {}, got {reported:?}",
            row.current_generation
        );

        let memory = required(case, &body, "memoryInfo");
        let reported = number(case, &memory, "sizeInMiB") as i64;
        assert_eq!(
            reported, row.memory_mib,
            "case {case:?}: expected memoryInfo.sizeInMiB {}, got {reported}",
            row.memory_mib
        );

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
        let supported = required(case, &body, "instanceStorageSupported");
        match row.instance_store {
            Some((devices, gb_each)) => {
                assert_eq!(
                    supported, "true",
                    "case {case:?}: a type with local devices reports instanceStorageSupported \
                     true, got {supported:?}"
                );
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
            None => {
                assert_eq!(
                    supported, "false",
                    "case {case:?}: a type with no local devices reports \
                     instanceStorageSupported false, got {supported:?}"
                );
                assert!(
                    storage.is_none(),
                    "case {case:?}: a type with no local devices reports no \
                     instanceStorageInfo, got {storage:?}"
                );
            }
        }

        let network_info = required(case, &body, "networkInfo");
        let reported = required(case, &network_info, "networkPerformance");
        assert_eq!(
            reported, row.network_performance,
            "case {case:?}: expected networkPerformance {:?}, got {reported:?}",
            row.network_performance
        );

        let processor_info = required(case, &body, "processorInfo");
        let reported = number(case, &processor_info, "sustainedClockSpeedInGhz");
        assert!(
            (reported - row.sustained_clock_speed_ghz).abs() < 0.001,
            "case {case:?}: expected sustainedClockSpeedInGhz {}, got {reported}",
            row.sustained_clock_speed_ghz
        );
    }
}

/// Rule 1, on the members that say which network and host features a type is offered: each
/// is the fact AWS publishes for the type, not one its generation number implies.
#[tokio::test]
async fn published_rows_report_the_features_the_type_is_offered() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: (type, member, expected). They span a previous generation family that predates
    // the Elastic Network Adapter, a Nitro family that requires it, the sizes of one family
    // that are and are not offered ENA Express and the Elastic Fabric Adapter, a bare metal
    // size whose boot modes are its own, Dedicated Hosts on a previous generation family,
    // CloudWatch action based recovery on a family with local devices, and a family offered
    // neither reboot migration nor a PTP hardware clock.
    let rows = [
        ("c4.large", "enaSupport", "unsupported"),
        ("c4.8xlarge", "enaSupport", "unsupported"),
        ("x2iedn.xlarge", "enaSupport", "required"),
        ("x2iedn.xlarge", "enaSrdSupported", "false"),
        ("x2iedn.4xlarge", "enaSrdSupported", "false"),
        ("x2iedn.8xlarge", "enaSrdSupported", "true"),
        ("x2iedn.metal", "enaSrdSupported", "true"),
        ("x2iedn.16xlarge", "efaSupported", "false"),
        ("x2iedn.24xlarge", "efaSupported", "false"),
        ("x2iedn.32xlarge", "efaSupported", "true"),
        ("x2iedn.metal", "efaSupported", "true"),
        ("dl2q.24xlarge", "encryptionInTransitSupported", "true"),
        ("c4.large", "dedicatedHostsSupported", "true"),
        ("c4.4xlarge", "dedicatedHostsSupported", "true"),
        ("x2iedn.2xlarge", "autoRecoverySupported", "true"),
        ("x2iedn.metal", "autoRecoverySupported", "true"),
        ("x2iedn.8xlarge", "rebootMigrationSupport", "unsupported"),
        ("x2iedn.8xlarge", "phcSupport", "unsupported"),
        ("c4.2xlarge", "rebootMigrationSupport", "supported"),
        ("c4.2xlarge", "phcSupport", "unsupported"),
    ];

    for (instance_type, member, expected) in rows {
        let case = format!("{instance_type} {member}");
        let body = described(&case, &router, instance_type).await;
        // A member of networkInfo is read out of that block; the rest are the type's own.
        let scope = if matches!(
            member,
            "enaSupport" | "enaSrdSupported" | "efaSupported" | "encryptionInTransitSupported"
        ) {
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

    // The boot modes a type takes: the bare metal size of the memory optimized family is
    // offered the legacy BIOS alone, while its virtualized sizes take both.
    let rows = [
        ("x2iedn.metal", vec!["legacy-bios"]),
        ("x2iedn.32xlarge", vec!["legacy-bios", "uefi"]),
        ("c4.large", vec!["legacy-bios"]),
    ];
    for (instance_type, expected) in rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let modes = required(case, &body, "supportedBootModes");
        let reported = elements(&modes, "item");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected supportedBootModes {expected:?}, got {reported:?}"
        );
    }
}

/// Rule 1, on the accelerators: an inference accelerated type reports the devices AWS
/// publishes for it, and a type with none reports no accelerator block at all.
#[tokio::test]
async fn an_inference_accelerated_type_reports_its_accelerators() {
    let (router, _engine) = super::ec2_query::emulator();

    let case = "dl2q.24xlarge";
    let body = described(case, &router, case).await;
    let info = element(&body, "inferenceAcceleratorInfo").unwrap_or_else(|| {
        panic!("case {case:?}: an inference accelerated type reports its accelerators, got {body}")
    });
    let count = number(case, &info, "count") as i64;
    assert_eq!(
        count, 8,
        "case {case:?}: expected 8 inference accelerators, got {count}"
    );
    let name = required(case, &info, "name");
    let manufacturer = required(case, &info, "manufacturer");
    assert_eq!(
        (name.as_str(), manufacturer.as_str()),
        ("Qualcomm AI100", "Qualcomm"),
        "case {case:?}: expected the published accelerator, got {info}"
    );
    let each = number(case, &info, "sizeInMiB") as i64;
    let total = number(case, &info, "totalInferenceMemoryInMiB") as i64;
    assert_eq!(
        total,
        each * count,
        "case {case:?}: the total inference memory is one device's ({each} MiB) times the \
         count ({count}), got {total}"
    );

    // Rows: types of four other families, none of which carries an inference accelerator.
    for instance_type in ["c4.large", "x2iedn.8xlarge", "t3.micro", "m5.large"] {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let reported = element(&body, "inferenceAcceleratorInfo");
        assert!(
            reported.is_none(),
            "case {case:?}: a type with no inference accelerator reports no \
             inferenceAcceleratorInfo, got {reported:?}"
        );
    }
}

/// Rule 2: every reported type carries the whole member set, whatever hardware it runs on.
#[tokio::test]
async fn every_reported_type_carries_the_whole_member_set() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: a pre-Nitro type, a pre-Nitro type at its largest size, a burstable Nitro type,
    // a Nitro type with local devices, a bare metal one, an accelerated one, and a type the
    // published table holds no row for at all.
    let rows = [
        "c4.large",
        "c4.8xlarge",
        "t3.micro",
        "x2iedn.8xlarge",
        "x2iedn.metal",
        "dl2q.24xlarge",
        "m5.large",
    ];

    for instance_type in rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");

        // The members of the type itself, and of its network settings, that AWS reports for
        // every instance type: each must be present, whatever the type runs on.
        for member in [
            "cpuBurstModel",
            "phcSupport",
            "rebootMigrationSupport",
            "nitroTpmSupport",
        ] {
            let reported = element(&body, member);
            assert!(
                reported.is_some(),
                "case {case:?}: every reported type carries <{member}>, got {body}"
            );
        }
        for member in [
            "defaultNetworkCardIndex",
            "maximumNetworkCards",
            "networkCards",
            "enaSupport",
            "enaSrdSupported",
            "flexibleEnaQueuesSupport",
            "secondaryNetworkSupported",
            "trafficMirroringSupport",
            "connectionTrackingConfiguration",
        ] {
            let reported = element(&network_info, member);
            assert!(
                reported.is_some(),
                "case {case:?}: every reported type carries networkInfo <{member}>, got \
                 {network_info}"
            );
        }

        // The default connection-tracking timeouts are the same for every type.
        let tracking = required(case, &network_info, "connectionTrackingConfiguration");
        for (member, expected) in [
            ("defaultTcpEstablishedTimeout", 432_000),
            ("defaultUdpTimeout", 30),
            ("defaultUdpStreamTimeout", 180),
        ] {
            let reported = number(case, &tracking, member) as i64;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {member} {expected}, got {reported}"
            );
        }

        // The first card is the default on every type, and a type with one card reports that
        // card carrying the whole instance's network performance.
        let cards = number(case, &network_info, "maximumNetworkCards") as i64;
        let default_index = number(case, &network_info, "defaultNetworkCardIndex") as i64;
        assert_eq!(
            default_index, 0,
            "case {case:?}: the default network card is the first, got {default_index}"
        );
        let card = required(case, &network_info, "networkCards");
        let reported = elements(&card, "networkCardIndex");
        assert_eq!(
            reported.len() as i64,
            cards,
            "case {case:?}: expected {cards} network card(s), got {reported:?}"
        );
        if cards == 1 {
            let performance = required(case, &network_info, "networkPerformance");
            let card_performance = required(case, &card, "networkPerformance");
            assert_eq!(
                card_performance, performance,
                "case {case:?}: one card carries the whole instance's network performance \
                 ({performance:?}), got {card_performance:?}"
            );
        }
    }

    // The burst model of the vCPUs: a burstable performance type spends CPU credits above a
    // baseline, and every other type runs at its full rate with no burst model at all.
    let rows = [
        ("t3.micro", "credit"),
        ("t3.2xlarge", "credit"),
        ("t2.small", "credit"),
        ("c4.large", "none"),
        ("x2iedn.metal", "none"),
        ("m5.large", "none"),
    ];
    for (instance_type, expected) in rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let reported = required(case, &body, "cpuBurstModel");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected cpuBurstModel {expected:?}, got {reported:?}"
        );
    }

    // Traffic Mirroring copies a session's packets under the guest, which a bare metal
    // instance has nothing to do.
    let rows = [
        ("c4.large", "supported"),
        ("t3.micro", "supported"),
        ("x2iedn.32xlarge", "supported"),
        ("x2iedn.metal", "unsupported"),
        ("m5.metal", "unsupported"),
    ];
    for (instance_type, expected) in rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let reported = required(case, &network_info, "trafficMirroringSupport");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected trafficMirroringSupport {expected:?}, got {reported:?}"
        );
    }
}

/// Rule 3: the core counts a type can be configured with are the ones AWS publishes for it.
#[tokio::test]
async fn the_core_counts_a_type_takes_are_published_per_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: (type, vCPUs, cores, the counts the type can be configured with). The largest
    // size of the previous generation compute optimized family carries 36 vCPUs on two
    // sockets rather than the 32 its step of the size ladder would give, and its cores are
    // taken one per socket; so are the accelerated type's and the memory optimized family's
    // from its 2xlarge up, while the smaller sizes of that family and of the compute
    // optimized one take every count.
    let rows: [(&str, i64, i64, Vec<i64>); 6] = [
        ("c4.large", 2, 1, vec![1]),
        ("c4.4xlarge", 16, 8, vec![1, 2, 3, 4, 5, 6, 7, 8]),
        (
            "c4.8xlarge",
            36,
            18,
            vec![2, 4, 6, 8, 10, 12, 14, 16, 18],
        ),
        ("x2iedn.xlarge", 4, 2, vec![1, 2]),
        ("x2iedn.2xlarge", 8, 4, vec![2, 4]),
        (
            "dl2q.24xlarge",
            96,
            48,
            (1..=24).map(|pair| pair * 2).collect(),
        ),
    ];

    for (instance_type, vcpus, cores, valid) in rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let vcpu_info = required(case, &body, "vCpuInfo");
        let reported = number(case, &vcpu_info, "defaultVCpus") as i64;
        assert_eq!(
            reported, vcpus,
            "case {case:?}: expected defaultVCpus {vcpus}, got {reported}"
        );
        let reported = number(case, &vcpu_info, "defaultCores") as i64;
        assert_eq!(
            reported, cores,
            "case {case:?}: expected defaultCores {cores}, got {reported}"
        );
        let valid_cores = required(case, &vcpu_info, "validCores");
        let reported: Vec<i64> = elements(&valid_cores, "item")
            .iter()
            .map(|count| {
                count
                    .parse()
                    .unwrap_or_else(|_| panic!("case {case:?}: a core count must be a number, \
                                                got {count:?}"))
            })
            .collect();
        assert_eq!(
            reported, valid,
            "case {case:?}: expected validCores {valid:?}, got {reported:?}"
        );
    }
}
