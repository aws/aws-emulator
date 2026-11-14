// SPDX-License-Identifier: CC0-1.0

//! The per-instance-type facts `DescribeInstanceTypes` reports that NO function of the type's
//! name can produce: its accelerator inventory, the idle timeout its connection tracking
//! applies, whether Nitro Enclaves is offered on it, the core counts it can be configured
//! with, and the usage classes it is offered in. Driven over the wire the service speaks
//! (`ec2Query`, an `application/x-www-form-urlencoded` POST answered with XML; README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: EVERY MEMBER OF A REPORTED INSTANCE TYPE IS THE FACT AWS PUBLISHES FOR
//! THAT TYPE, not a figure derived from the size multiplier in its name. A type with
//! accelerators reports the devices it carries (`InstanceTypeInfo$GpuInfo`, whose
//! `TotalGpuMemoryInMiB` is one device's memory times the count) and a type with none reports
//! no inventory at all; the established-TCP idle timeout
//! (`DefaultConnectionTrackingConfiguration$DefaultTcpEstablishedTimeout`), Nitro Enclaves
//! support, the EBS performance and attachment limit, the core counts (`VCpuInfo$ValidCores`)
//! and the usage classes are each the type's own.
//!
//! Every table spans what the rule generalises over — families of four generations, the
//! accelerated and the unaccelerated type, the small size and the one that holds a whole host,
//! a limit that is shared and one that is dedicated — so no row is the specification.

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

/// The GPU accelerators AWS publishes for one instance type.
#[derive(Debug, Clone, Copy)]
struct PublishedGpus {
    /// How many devices the instance carries, and how big one device's memory is, in MiB.
    devices: (i64, i64),
    /// The device's name and its maker.
    device: (&'static str, &'static str),
    /// The fraction of a full GPU each one is, and the workload the inventory names.
    offered: (f64, &'static str),
}

/// Rule: a type's accelerator inventory is the devices AWS publishes for it, and a type that
/// carries none reports no inventory.
///
/// Rows: both accelerated types of the catalog's newest GPU family, each with its own device
/// and its own device memory, and five types from four other families — compute optimized,
/// memory optimized with EBS hardware of its own, burstable, and an FPGA accelerated type
/// whose accelerators are NOT GPUs — that carry no GPU at all.
#[tokio::test]
async fn the_gpu_inventory_is_the_published_one_of_the_type() {
    let (router, _engine) = super::ec2_query::emulator();

    let rows: [(&str, Option<PublishedGpus>); 7] = [
        (
            "p6-b200.48xlarge",
            Some(PublishedGpus {
                devices: (8, 183_359),
                device: ("B200", "NVIDIA"),
                offered: (1.0, "ml-ai"),
            }),
        ),
        (
            "p6-b300.48xlarge",
            Some(PublishedGpus {
                devices: (8, 275_040),
                device: ("B300", "NVIDIA"),
                offered: (1.0, "ml-ai"),
            }),
        ),
        ("c5a.xlarge", None),
        ("c5a.24xlarge", None),
        ("r8ib.large", None),
        ("t3.micro", None),
        ("f2.6xlarge", None),
    ];

    for (instance_type, expected) in rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let reported = element(&body, "gpuInfo");
        let Some(published) = expected else {
            assert!(
                reported.is_none(),
                "case {case:?}: a type with no GPU reports no gpuInfo, got {reported:?}"
            );
            continue;
        };
        let info = reported
            .unwrap_or_else(|| panic!("case {case:?}: a GPU type reports its gpuInfo, got {body}"));
        let (count, memory_each) = published.devices;
        let reported = number(case, &info, "count") as i64;
        assert_eq!(
            reported, count,
            "case {case:?}: expected {count} GPUs, got {reported}"
        );
        let reported = number(case, &info, "logicalGpuCount") as i64;
        assert_eq!(
            reported, count,
            "case {case:?}: expected logicalGpuCount {count}, got {reported}"
        );
        let reported = number(case, &info, "sizeInMiB") as i64;
        assert_eq!(
            reported, memory_each,
            "case {case:?}: expected GPU memory of {memory_each} MiB, got {reported}"
        );
        // The total is one device's memory times the count, so the inventory cannot disagree
        // with itself.
        let total = number(case, &info, "totalGpuMemoryInMiB") as i64;
        assert_eq!(
            total,
            memory_each * count,
            "case {case:?}: expected totalGpuMemoryInMiB {}, got {total}",
            memory_each * count
        );
        let (name, manufacturer) = published.device;
        let reported = required(case, &info, "name");
        assert_eq!(
            reported, name,
            "case {case:?}: expected GPU name {name:?}, got {reported:?}"
        );
        let reported = required(case, &info, "manufacturer");
        assert_eq!(
            reported, manufacturer,
            "case {case:?}: expected GPU manufacturer {manufacturer:?}, got {reported:?}"
        );
        let (partition, workload) = published.offered;
        let reported = number(case, &info, "gpuPartitionSize");
        assert!(
            (reported - partition).abs() < 0.001,
            "case {case:?}: expected gpuPartitionSize {partition}, got {reported}"
        );
        let workloads = required(case, &info, "workloadSet");
        assert_eq!(
            elements(&workloads, "item"),
            vec![workload.to_string()],
            "case {case:?}: expected the workload {workload:?}, got {workloads}"
        );
    }
}

/// Rule: the idle timeout a type's connection tracking applies to an established TCP
/// connection, and whether Nitro Enclaves is offered on it, are facts of the type.
///
/// Rows: the newest families, whose hardware publishes a far shorter established-TCP timeout
/// than every earlier type reports, beside four families that report the longer one — and the
/// enclave answer for an accelerated type that is offered none, a size too small to hold one,
/// and two sizes that are offered it.
#[tokio::test]
async fn connection_tracking_and_enclave_support_are_published_per_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // The published established-TCP idle timeouts, in seconds.
    let timeout_rows = [
        ("r8ib.large", 350),
        ("r8ib.24xlarge", 350),
        ("r8ib.96xlarge", 350),
        ("p6-b200.48xlarge", 350),
        ("p6-b300.48xlarge", 350),
        ("c5a.xlarge", 432_000),
        ("c5a.24xlarge", 432_000),
        ("t3.nano", 432_000),
        ("m6idn.large", 432_000),
        ("d3en.12xlarge", 432_000),
    ];

    for (instance_type, expected) in timeout_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let tracking = required(case, &network_info, "connectionTrackingConfiguration");
        let reported = number(case, &tracking, "defaultTcpEstablishedTimeout") as i64;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected defaultTcpEstablishedTimeout {expected}, got {reported}"
        );
    }

    let enclave_rows = [
        ("p6-b200.48xlarge", "unsupported"),
        ("p6-b300.48xlarge", "unsupported"),
        ("r8ib.large", "unsupported"),
        ("t3.micro", "unsupported"),
        ("r8ib.4xlarge", "supported"),
        ("c5a.xlarge", "supported"),
    ];

    for (instance_type, expected) in enclave_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let reported = required(case, &body, "nitroEnclavesSupport");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected nitroEnclavesSupport {expected:?}, got {reported:?}"
        );
    }

    // ENA Express is offered on the sizes that carry their family's full network share, which
    // is a published fact of the size and not of its generation alone: the smaller sizes of a
    // family that offers it do not have it, and a family that predates the adapter feature has
    // it nowhere.
    let ena_srd_rows = [
        ("r8ib.large", "false"),
        ("r8ib.4xlarge", "false"),
        ("r8ib.8xlarge", "true"),
        ("r8ib.96xlarge", "true"),
        ("x2iedn.xlarge", "false"),
        ("x2iedn.8xlarge", "true"),
        ("c5a.xlarge", "false"),
        ("c5a.24xlarge", "false"),
    ];

    for (instance_type, expected) in ena_srd_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let reported = required(case, &network_info, "enaSrdSupported");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected enaSrdSupported {expected:?}, got {reported:?}"
        );
    }

    // A published Traffic Mirroring answer is the type's own, and a type the catalog holds no
    // row for keeps the hypervisor's answer: nothing under a bare metal guest can copy a
    // session's packets.
    let mirroring_rows = [
        ("r8ib.large", "unsupported"),
        ("r8ib.96xlarge", "unsupported"),
        ("c5a.xlarge", "supported"),
        ("t3.micro", "supported"),
        ("x2iedn.metal", "unsupported"),
    ];

    for (instance_type, expected) in mirroring_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let reported = required(case, &network_info, "trafficMirroringSupport");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected trafficMirroringSupport {expected:?}, got {reported:?}"
        );
    }

    // Interface attachments from a secondary network are a published fact of the type too, and
    // a type that takes none reports no secondary interface limit at all.
    let secondary_rows = [
        ("r8ib.large", "false"),
        ("r8ib.96xlarge", "false"),
        ("c5a.xlarge", "false"),
        ("m7i.4xlarge", "true"),
    ];

    for (instance_type, expected) in secondary_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let network_info = required(case, &body, "networkInfo");
        let reported = required(case, &network_info, "secondaryNetworkSupported");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected secondaryNetworkSupported {expected:?}, got {reported:?}"
        );
        let limit = element(&network_info, "maximumSecondaryNetworkInterfaces");
        assert_eq!(
            limit.is_some(),
            expected == "true",
            "case {case:?}: a type that takes no secondary attachment reports no \
             maximumSecondaryNetworkInterfaces, got {limit:?}"
        );
    }

    // A local PTP hardware clock is the type's own hardware, not its generation: a family whose
    // hardware carries none reports none however new it is, while reboot migration is offered
    // on it.
    let host_feature_rows = [
        ("r8ib.large", "phcSupport", "unsupported"),
        ("r8ib.96xlarge", "phcSupport", "unsupported"),
        ("r8ib.large", "rebootMigrationSupport", "supported"),
        ("r8ib.96xlarge", "rebootMigrationSupport", "supported"),
        ("c5a.xlarge", "phcSupport", "unsupported"),
    ];

    for (instance_type, member, expected) in host_feature_rows {
        let case = format!("{instance_type} {member}");
        let body = described(&case, &router, instance_type).await;
        let reported = required(&case, &body, member);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {member} {expected:?}, got {reported:?}"
        );
    }
}

/// Rule: the EBS performance and the attachment limit of a type are the published figures of
/// that type, and the limit's KIND says whether those volumes have hardware of their own.
///
/// Rows: five sizes of a compute optimized AMD family whose sizes burst to its dedicated row's
/// figure and whose volumes come out of the shared attachment pool, four sizes of a memory
/// optimized family whose volumes have a limit of their own, and an accelerated type with a
/// limit of its own an order of magnitude above the pool's.
#[tokio::test]
async fn the_ebs_performance_and_attachment_limit_are_published_per_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // (type, (sustained Mbps, sustained IOPS, Mbps it reaches, IOPS it reaches), (volumes, kind))
    let rows = [
        ("c5a.large", (200, 800, 3_170, 13_300), (27, "shared")),
        ("c5a.xlarge", (400, 1_600, 3_170, 13_300), (27, "shared")),
        (
            "c5a.8xlarge",
            (3_170, 13_300, 3_170, 13_300),
            (27, "shared"),
        ),
        (
            "c5a.24xlarge",
            (9_500, 40_000, 9_500, 40_000),
            (27, "shared"),
        ),
        (
            "r8ib.large",
            (1_563, 7_500, 25_000, 120_000),
            (32, "dedicated"),
        ),
        (
            "r8ib.2xlarge",
            (6_250, 30_000, 25_000, 120_000),
            (32, "dedicated"),
        ),
        (
            "r8ib.12xlarge",
            (37_500, 180_000, 37_500, 180_000),
            (32, "dedicated"),
        ),
        (
            "r8ib.96xlarge",
            (300_000, 1_200_000, 300_000, 1_200_000),
            (32, "dedicated"),
        ),
        (
            "p6-b200.48xlarge",
            (100_000, 400_000, 100_000, 400_000),
            (64, "dedicated"),
        ),
    ];

    for (instance_type, ebs, attachments) in rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let ebs_info = required(case, &body, "ebsInfo");
        let performance = required(case, &ebs_info, "ebsOptimizedInfo");
        let (baseline_mbps, baseline_iops, maximum_mbps, maximum_iops) = ebs;
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
        let (volumes, kind) = attachments;
        let reported = number(case, &ebs_info, "maximumEbsAttachments") as i64;
        assert_eq!(
            reported, volumes,
            "case {case:?}: expected maximumEbsAttachments {volumes}, got {reported}"
        );
        let reported = required(case, &ebs_info, "attachmentLimitType");
        assert_eq!(
            reported, kind,
            "case {case:?}: expected attachmentLimitType {kind:?}, got {reported:?}"
        );
    }
}

/// Rule: the core counts a type can be configured with are the counts AWS publishes for it —
/// every count up to its default where its cores are taken one at a time, and the multiples of
/// the step where they are taken in pairs — and the usage classes are the classes it is
/// offered in, in the order AWS lists them.
#[tokio::test]
async fn valid_cores_and_usage_classes_are_published_per_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // (type, default cores, the step its published counts go up in)
    let core_rows = [
        ("c5a.xlarge", 2, 1),
        ("c5a.2xlarge", 4, 1),
        ("r8ib.16xlarge", 32, 1),
        ("r8ib.24xlarge", 48, 2),
        ("p6-b200.48xlarge", 96, 2),
        ("t3.large", 1, 1),
    ];

    for (instance_type, cores, step) in core_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let vcpu_info = required(case, &body, "vCpuInfo");
        let reported = number(case, &vcpu_info, "defaultCores") as i64;
        assert_eq!(
            reported, cores,
            "case {case:?}: expected defaultCores {cores}, got {reported}"
        );
        let valid = required(case, &vcpu_info, "validCores");
        let reported: Vec<i64> = elements(&valid, "item")
            .iter()
            .map(|count| {
                count.parse().unwrap_or_else(|_| {
                    panic!("case {case:?}: a core count must be a number, got {count:?}")
                })
            })
            .collect();
        let expected: Vec<i64> = (1..=cores).filter(|count| count % step == 0).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected validCores {expected:?}, got {reported:?}"
        );
    }

    // Capacity Blocks reserve the accelerated families, and a type offered them reports that
    // class first; every other type is offered On-Demand and Spot alone.
    let usage_rows: [(&str, &[&str]); 5] = [
        (
            "p6-b200.48xlarge",
            &["capacity-block", "on-demand", "spot"],
        ),
        (
            "p6-b300.48xlarge",
            &["capacity-block", "on-demand", "spot"],
        ),
        ("c5a.xlarge", &["on-demand", "spot"]),
        ("r8ib.large", &["on-demand", "spot"]),
        ("t3.micro", &["on-demand", "spot"]),
    ];

    for (instance_type, expected) in usage_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let classes = required(case, &body, "supportedUsageClasses");
        let reported = elements(&classes, "item");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected supportedUsageClasses {expected:?}, got {reported:?}"
        );
    }
}
