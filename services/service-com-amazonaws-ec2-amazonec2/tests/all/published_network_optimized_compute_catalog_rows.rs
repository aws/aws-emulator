// SPDX-License-Identifier: CC0-1.0

//! The per-instance-type rows the catalog publishes for a NETWORK optimized compute family,
//! driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rule under test: EVERY PER-TYPE MEMBER `DescribeInstanceTypes` REPORTS IS THE FIGURE
//! AWS PUBLISHES FOR THAT ONE INSTANCE TYPE, and none of them is omitted. A type's memory
//! (`MemoryInfo$SizeInMiB`), its EBS bandwidth and IOPS (`EbsInfo$EbsOptimizedInfo`), how many
//! volumes it holds attached (`EbsInfo$MaximumEbsAttachments`), its network performance and
//! its Elastic Fabric Adapters (`NetworkInfo$NetworkPerformance` / `$EfaInfo`), its
//! processor's sustained frequency (`ProcessorInfo$SustainedClockSpeedInGhz`), the core counts
//! it can be configured with (`VCpuInfo$ValidCores`), and the host features it is offered
//! (hibernation, CloudWatch action based recovery, reboot migration, a PTP hardware clock, the
//! boot modes it takes) are each a published fact of the type, not a function of the size
//! multiplier in its name and not a feature its generation number implies.
//!
//! The tables span what the rule generalises over: the WHOLE size ladder of one family — from
//! the size that bursts above its baseline to the one with the host's whole share, and the
//! bare metal size whose features are its own — beside two sizes of two OTHER compute
//! families, one Intel and one AMD, whose figures at the same step of the ladder disagree with
//! it in memory, EBS performance, network performance and processor frequency. So no one row
//! is the specification.
//!
//! The figures are the ones AWS states for each type on its family's instance-type
//! specification page (vCPUs, memory, network performance), in the "Amazon EBS optimized
//! instance types" and "Instance volume limits" tables, in the CPU options table, and in the
//! lists of the types offered hibernation and an Elastic Fabric Adapter, of the EC2 User
//! Guide.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, indexed};

/// The Region every call here is signed for; the catalog is the same in each.
const REGION: &str = "us-east-1";

/// The megabits in a megabyte, which is all that separates the `Mbps` figures of
/// `EbsOptimizedInfo` from its `MB/s` ones.
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

/// The published row of one instance type.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// `c5n.9xlarge` and the like.
    instance_type: &'static str,
    /// Its memory, in MiB.
    memory_mib: i64,
    /// (sustained Mbps, sustained IOPS, Mbps it reaches, IOPS it reaches).
    ebs: (i64, i64, i64, i64),
    /// The EBS volumes it holds attached, and whether that limit is shared with its
    /// interfaces and local devices or is the volumes' own.
    ebs_attachments: (i64, &'static str),
    /// Its network performance, as the published table writes it.
    network_performance: &'static str,
    /// Its processor's maker and that processor's sustained frequency, in GHz.
    processor: (&'static str, f64),
    /// Is On-Demand hibernation offered on it?
    hibernation: bool,
    /// The core counts it can be configured with.
    valid_cores: &'static [i64],
    /// The Elastic Fabric Adapters it is offered, or `None` where it is offered none.
    efa_interfaces: Option<i64>,
    /// The boot modes it takes.
    boot_modes: &'static [&'static str],
    /// Is CloudWatch action based recovery offered on it, and is reboot migration during a
    /// user-initiated reboot?
    recovery: (bool, &'static str),
}

/// Every size of the network optimized compute family, and two sizes of two other compute
/// families at the same steps of the ladder.
///
/// C5n: 5.25 GiB of memory per two vCPUs, a 100 Gbps adapter whose share the four smallest
/// sizes burst up to 25 Gigabit of, EBS figures that are a baseline below the `4xlarge` and
/// dedicated from it up, an Elastic Fabric Adapter from the `9xlarge` up, no hibernation at
/// any size, core counts in twos from the `xlarge` that start at four on the `18xlarge`, and a
/// bare metal size that holds 31 volumes, takes the legacy BIOS boot mode alone and is offered
/// both action based recovery and reboot migration.
/// C5 and C5a at the `2xlarge` and `4xlarge`: 2 GiB of memory per vCPU, a 25 Gbps host share
/// that bursts to 10 Gigabit, their own EBS rows, their own processors — and hibernation
/// offered on the Intel family and on neither of the other two.
const ROWS: [Row; 9] = [
    Row {
        instance_type: "c5n.large",
        memory_mib: 5_376,
        ebs: (650, 4_000, 4_750, 20_000),
        ebs_attachments: (27, "shared"),
        network_performance: "Up to 25 Gigabit",
        processor: ("Intel", 3.4),
        hibernation: false,
        valid_cores: &[1],
        efa_interfaces: None,
        boot_modes: &["legacy-bios", "uefi"],
        recovery: (true, "supported"),
    },
    Row {
        instance_type: "c5n.xlarge",
        memory_mib: 10_752,
        ebs: (1_150, 6_000, 4_750, 20_000),
        ebs_attachments: (27, "shared"),
        network_performance: "Up to 25 Gigabit",
        processor: ("Intel", 3.4),
        hibernation: false,
        valid_cores: &[2],
        efa_interfaces: None,
        boot_modes: &["legacy-bios", "uefi"],
        recovery: (true, "supported"),
    },
    Row {
        instance_type: "c5n.2xlarge",
        memory_mib: 21_504,
        ebs: (2_300, 10_000, 4_750, 20_000),
        ebs_attachments: (27, "shared"),
        network_performance: "Up to 25 Gigabit",
        processor: ("Intel", 3.4),
        hibernation: false,
        valid_cores: &[2, 4],
        efa_interfaces: None,
        boot_modes: &["legacy-bios", "uefi"],
        recovery: (true, "supported"),
    },
    Row {
        instance_type: "c5n.4xlarge",
        memory_mib: 43_008,
        ebs: (4_750, 20_000, 4_750, 20_000),
        ebs_attachments: (27, "shared"),
        network_performance: "Up to 25 Gigabit",
        processor: ("Intel", 3.4),
        hibernation: false,
        valid_cores: &[2, 4, 6, 8],
        efa_interfaces: None,
        boot_modes: &["legacy-bios", "uefi"],
        recovery: (true, "supported"),
    },
    Row {
        instance_type: "c5n.9xlarge",
        memory_mib: 98_304,
        ebs: (9_500, 40_000, 9_500, 40_000),
        ebs_attachments: (27, "shared"),
        network_performance: "50 Gigabit",
        processor: ("Intel", 3.4),
        hibernation: false,
        valid_cores: &[2, 4, 6, 8, 10, 12, 14, 16, 18],
        efa_interfaces: Some(1),
        boot_modes: &["legacy-bios", "uefi"],
        recovery: (true, "supported"),
    },
    Row {
        instance_type: "c5n.18xlarge",
        memory_mib: 196_608,
        ebs: (19_000, 80_000, 19_000, 80_000),
        ebs_attachments: (27, "shared"),
        network_performance: "100 Gigabit",
        processor: ("Intel", 3.4),
        hibernation: false,
        valid_cores: &[
            4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 36,
        ],
        efa_interfaces: Some(1),
        boot_modes: &["legacy-bios", "uefi"],
        recovery: (true, "supported"),
    },
    Row {
        instance_type: "c5n.metal",
        memory_mib: 196_608,
        ebs: (19_000, 80_000, 19_000, 80_000),
        ebs_attachments: (31, "shared"),
        network_performance: "100 Gigabit",
        processor: ("Intel", 3.4),
        hibernation: false,
        valid_cores: &[
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36,
        ],
        efa_interfaces: Some(1),
        boot_modes: &["legacy-bios"],
        recovery: (true, "supported"),
    },
    Row {
        instance_type: "c5.2xlarge",
        memory_mib: 16_384,
        ebs: (2_300, 10_000, 4_750, 20_000),
        ebs_attachments: (27, "shared"),
        network_performance: "Up to 10 Gigabit",
        processor: ("Intel", 3.4),
        hibernation: true,
        valid_cores: &[2, 4],
        efa_interfaces: None,
        boot_modes: &["legacy-bios", "uefi"],
        recovery: (true, "supported"),
    },
    Row {
        instance_type: "c5a.4xlarge",
        memory_mib: 32_768,
        ebs: (1_580, 6_600, 3_170, 13_300),
        ebs_attachments: (27, "shared"),
        network_performance: "Up to 10 Gigabit",
        processor: ("AMD", 3.3),
        hibernation: false,
        // The AMD sizes from this one up take their cores ALL at once, so the one count
        // published for the size is its default.
        valid_cores: &[8],
        efa_interfaces: None,
        boot_modes: &["legacy-bios", "uefi"],
        recovery: (true, "supported"),
    },
];

/// Rule: the memory, EBS, network, processor and core-count members a type reports are the
/// figures AWS publishes for THAT type.
#[tokio::test]
async fn published_rows_report_the_hardware_of_the_type() {
    let (router, _engine) = super::ec2_query::emulator();

    for row in ROWS {
        let case = row.instance_type;
        let body = described(case, &router, row.instance_type).await;

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
        // The throughput members are the bandwidth ones in the other unit.
        for (name, mbps) in [
            ("baselineThroughputInMBps", baseline_mbps),
            ("maximumThroughputInMBps", maximum_mbps),
        ] {
            let reported = number(case, &performance, name);
            let expected = mbps as f64 / MEGABITS_PER_MEGABYTE;
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

        let vcpu_info = required(case, &body, "vCpuInfo");
        let valid_cores = required(case, &vcpu_info, "validCores");
        let reported: Vec<i64> = elements(&valid_cores, "item")
            .iter()
            .map(|count| {
                count.parse().unwrap_or_else(|_| {
                    panic!("case {case:?}: a core count must be a number, got {count:?}")
                })
            })
            .collect();
        assert_eq!(
            reported,
            row.valid_cores.to_vec(),
            "case {case:?}: expected validCores {:?}, got {reported:?}",
            row.valid_cores
        );
    }
}

/// Rule, on the features a type is offered: hibernation, the adapters, the boot modes, action
/// based recovery and reboot migration are published per type.
#[tokio::test]
async fn published_rows_report_the_features_the_type_is_offered() {
    let (router, _engine) = super::ec2_query::emulator();

    for row in ROWS {
        let case = row.instance_type;
        let body = described(case, &router, row.instance_type).await;

        let reported = required(case, &body, "hibernationSupported");
        assert_eq!(
            reported,
            row.hibernation.to_string(),
            "case {case:?}: expected hibernationSupported {}, got {reported:?}",
            row.hibernation
        );

        let network_info = required(case, &body, "networkInfo");
        let reported = required(case, &network_info, "efaSupported");
        assert_eq!(
            reported,
            row.efa_interfaces.is_some().to_string(),
            "case {case:?}: expected efaSupported {}, got {reported:?}",
            row.efa_interfaces.is_some()
        );
        let info = element(&network_info, "efaInfo");
        match row.efa_interfaces {
            Some(adapters) => {
                let info = info.unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a type offered an adapter reports its efaInfo, got \
                         {network_info}"
                    )
                });
                let reported = number(case, &info, "maximumEfaInterfaces") as i64;
                assert_eq!(
                    reported, adapters,
                    "case {case:?}: expected maximumEfaInterfaces {adapters}, got {reported}"
                );
            }
            None => assert!(
                info.is_none(),
                "case {case:?}: a type offered no adapter reports no efaInfo, got {info:?}"
            ),
        }

        let modes = required(case, &body, "supportedBootModes");
        let reported = elements(&modes, "item");
        assert_eq!(
            reported,
            row.boot_modes.to_vec(),
            "case {case:?}: expected supportedBootModes {:?}, got {reported:?}",
            row.boot_modes
        );

        let (auto_recovery, reboot_migration) = row.recovery;
        let reported = required(case, &body, "autoRecoverySupported");
        assert_eq!(
            reported,
            auto_recovery.to_string(),
            "case {case:?}: expected autoRecoverySupported {auto_recovery}, got {reported:?}"
        );
        let reported = required(case, &body, "rebootMigrationSupport");
        assert_eq!(
            reported, reboot_migration,
            "case {case:?}: expected rebootMigrationSupport {reboot_migration:?}, got \
             {reported:?}"
        );
    }
}

/// Rule, on the other half of the defect: every one of these types reports the WHOLE member
/// set, each member carrying the answer its hardware generation gives.
#[tokio::test]
async fn every_size_of_the_family_carries_the_whole_member_set() {
    let (router, _engine) = super::ec2_query::emulator();

    for row in ROWS {
        let case = row.instance_type;
        let body = described(case, &router, row.instance_type).await;
        let network_info = required(case, &body, "networkInfo");

        // The burst model of a type that runs at its full rate: these three families are not
        // burstable performance families, so none of them has a burst to model.
        let reported = required(case, &body, "cpuBurstModel");
        assert_eq!(
            reported, "none",
            "case {case:?}: expected cpuBurstModel \"none\", got {reported:?}"
        );

        // The features of a hardware generation older than the one that introduced them: no
        // PTP hardware clock, no configurable ENA queue count, no ENA Express and no
        // interface attachment from a secondary network.
        for (member, expected) in [
            ("phcSupport", "unsupported"),
            ("flexibleEnaQueuesSupport", "unsupported"),
            ("enaSrdSupported", "false"),
            ("secondaryNetworkSupported", "false"),
        ] {
            let scope = if member == "phcSupport" {
                &body
            } else {
                &network_info
            };
            let reported = required(case, scope, member);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {member} {expected:?}, got {reported:?}"
            );
        }

        // Traffic Mirroring copies a session's packets under the guest, which a bare metal
        // instance — reporting no hypervisor at all — has nothing to do.
        let bare_metal = required(case, &body, "bareMetal") == "true";
        let expected = if bare_metal {
            "unsupported"
        } else {
            "supported"
        };
        let reported = required(case, &network_info, "trafficMirroringSupport");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected trafficMirroringSupport {expected:?}, got {reported:?}"
        );

        // One network card, the first being the default, and that card carries the whole
        // instance's network performance.
        let reported = number(case, &network_info, "maximumNetworkCards") as i64;
        assert_eq!(
            reported, 1,
            "case {case:?}: expected maximumNetworkCards 1, got {reported}"
        );
        let reported = number(case, &network_info, "defaultNetworkCardIndex") as i64;
        assert_eq!(
            reported, 0,
            "case {case:?}: expected defaultNetworkCardIndex 0, got {reported}"
        );
        let cards = required(case, &network_info, "networkCards");
        let indexes = elements(&cards, "networkCardIndex");
        assert_eq!(
            indexes,
            vec!["0".to_string()],
            "case {case:?}: expected one network card at index 0, got {indexes:?}"
        );
        let reported = required(case, &cards, "networkPerformance");
        assert_eq!(
            reported, row.network_performance,
            "case {case:?}: one card carries the whole instance's network performance \
             ({:?}), got {reported:?}",
            row.network_performance
        );
    }
}
