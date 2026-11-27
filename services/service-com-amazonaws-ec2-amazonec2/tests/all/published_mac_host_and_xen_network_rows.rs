// SPDX-License-Identifier: CC0-1.0

//! What `DescribeInstanceTypes` serves for the instance types whose hardware facts AWS
//! publishes per type and no reading of the name yields: the EC2 Mac hosts, the fractional
//! size of the memory-rich graphics family, the first burstable family's one size, and the
//! network figures of the families that run on the pre-Nitro Xen hardware.
//!
//! The rules under test:
//!
//! 1. A network card reports the figures of the adapter the TYPE publishes, not a share of a
//!    later generation's host: a size of a pre-Nitro family reports the share of its own
//!    host's figure that it sustains beside the ceiling it bursts to, and a type that IS a
//!    whole host reports its own figure as both the figure it sustains and the figure it
//!    reaches.
//! 2. Every other per-type published fact — the EBS bandwidth and IOPS dedicated to the type,
//!    how many EBS volumes it holds and whether that limit is shared, its memory, its
//!    interfaces and the addresses each takes, the ENA queues one interface is given, the
//!    local devices it presents, its accelerators and the host features it is offered — is the
//!    one its published row carries rather than one derived from its size.
//!
//! The rows span the class along what the rules generalise over: three pre-Nitro families and
//! four sizes of each ladder, every EC2 Mac host (the Intel one and all eight Apple silicon
//! ones, which the issue cited only five of), an accelerated size with a fractional-family
//! name, and the two burstable families either side of the CPU credit model. Each type is read
//! back through the operation end to end, one ec2Query call per type, the way `ec2_query`
//! drives every suite in this crate.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{
    described_instance_type, instance_type_member_number, reported_instance_type_list,
    reported_instance_type_member, required_instance_type_member, MEGABITS_PER_MEGABYTE,
};

/// The emulator the tests drive. This operation reports facts of a type and stores nothing,
/// so the engine handle it hands back is not needed here.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The Region every call here is signed for. The published hardware of an instance type is the
/// same in every Region, so one Region covers the rules.
const REGION: &str = "us-west-2";

/// The member path of one figure of the first (and, for every type here, only) network card.
fn card(member: &str) -> [&str; 4] {
    ["networkInfo", "networkCards", "item", member]
}

/// How close two published figures in Gbps or MB/s have to be to be the same figure.
const FIGURE_TOLERANCE: f64 = 0.0001;

/// One row of the network table: the type, the Gigabit its card sustains and can reach, and
/// the words of its "Network performance" column.
type NetworkCase<'a> = (&'a str, f64, f64, &'a str);

/// Rule 1 over the families whose sizes share one pre-Nitro Xen adapter, the first burstable
/// family's own device, and the EC2 Mac hosts.
#[tokio::test]
async fn a_network_card_reports_the_adapter_its_own_type_publishes() {
    let router = emulator();
    let cases: [NetworkCase; 13] = [
        // C4 — the compute optimized fourth generation family: each size's share of the 10
        // Gigabit its host carries, beside the host's own figure, which every size can reach.
        ("c4.large", 0.625, 10.0, "Moderate"),
        ("c4.xlarge", 1.25, 10.0, "High"),
        ("c4.2xlarge", 2.5, 10.0, "High"),
        ("c4.4xlarge", 5.0, 10.0, "High"),
        ("c4.8xlarge", 10.0, 10.0, "10 Gigabit"),
        // M4 and R4 — the sizes of the same generation that hold a whole host publish their own
        // figure as both the one they sustain and the one they reach, while the size that holds
        // HALF a host publishes the 12 Gbps of its own row beside the round column published
        // for it.
        ("m4.10xlarge", 10.0, 10.0, "10 Gigabit"),
        ("m4.16xlarge", 25.0, 25.0, "25 Gigabit"),
        ("r4.8xlarge", 12.0, 12.0, "10 Gigabit"),
        ("r4.16xlarge", 25.0, 25.0, "25 Gigabit"),
        // T1 — the first burstable family's one size, whose column reads in words and which
        // bursts to four times what it sustains.
        ("t1.micro", 0.07, 0.28, "Very Low"),
        // The EC2 Mac hosts: one whole host, so the figure it sustains IS the figure it
        // reaches, whichever Mac it is.
        ("mac1.metal", 10.0, 10.0, "10 Gigabit"),
        ("mac2.metal", 10.0, 10.0, "10 Gigabit"),
        ("mac-m4max.metal", 10.0, 10.0, "10 Gigabit"),
    ];

    for (instance_type, baseline, peak, performance) in cases {
        let body = described_instance_type(instance_type, &router, REGION, instance_type).await;
        for (member, expected) in [
            ("baselineBandwidthInGbps", baseline),
            ("peakBandwidthInGbps", peak),
        ] {
            let actual = instance_type_member_number(instance_type, &body, &card(member));
            assert!(
                (actual - expected).abs() < FIGURE_TOLERANCE,
                "case {instance_type:?}: expected card {member} {expected}, got {actual}"
            );
        }
        // The card's words and the type's own are one published column.
        for path in [
            vec!["networkInfo", "networkPerformance"],
            card("networkPerformance").to_vec(),
        ] {
            let reported = required_instance_type_member(instance_type, &body, &path);
            assert_eq!(
                reported, performance,
                "case {instance_type:?}: expected {path:?} {performance:?}, got {reported:?}"
            );
        }
    }
}

/// Rule 2 for the ENA queues one interface of a type is given: the count the type's own row
/// publishes for its adapter, and an absent member for a type whose count the catalog does not
/// hold.
///
/// The rows differ along what the rule generalises over: a pre-ENA family whose adapter
/// publishes one count at every size, two Mac generations publishing two different counts, a
/// Mac host with no published count, and a pre-ENA type the catalog holds no count for at all.
#[tokio::test]
async fn an_interface_reports_the_queues_its_published_adapter_gives_it() {
    let router = emulator();
    // (the type, its `enaSupport`, the queues one of its interfaces is given)
    let cases: [(&str, &str, Option<i32>); 8] = [
        // The compute optimized fourth generation family, whose adapter publishes the same
        // eight queues at every size of its ladder.
        ("c4.large", "unsupported", Some(8)),
        ("c4.xlarge", "unsupported", Some(8)),
        ("c4.8xlarge", "unsupported", Some(8)),
        // The Mac hosts: two generations' own counts, and the hosts whose count the catalog
        // does not hold, which report an absent member rather than an invented one.
        ("mac2.metal", "required", Some(32)),
        ("mac2-m2pro.metal", "required", Some(32)),
        ("mac-m4max.metal", "required", Some(8)),
        ("mac-m4.metal", "required", None),
        // A pre-ENA family the catalog holds no queue count for: the single queue its device
        // presents.
        ("t1.micro", "unsupported", Some(1)),
    ];

    for (instance_type, ena_support, queues) in cases {
        let body = described_instance_type(instance_type, &router, REGION, instance_type).await;
        let reported_ena =
            required_instance_type_member(instance_type, &body, &["networkInfo", "enaSupport"]);
        assert_eq!(
            reported_ena, ena_support,
            "case {instance_type:?}: expected enaSupport {ena_support:?}, got {reported_ena:?}"
        );
        let reported = reported_instance_type_member(
            instance_type,
            &body,
            &card("defaultEnaQueueCountPerInterface"),
        );
        let expected = queues.map(|queues| queues.to_string());
        assert_eq!(
            reported, expected,
            "case {instance_type:?}: expected defaultEnaQueueCountPerInterface {expected:?}, \
             got {reported:?}"
        );
    }
}

/// One row of the EC2 Mac table: the type, the Mbps and IOPS its Nitro controller dedicates to
/// EBS, how many EBS volumes it holds, its interfaces and the private addresses one of them
/// takes, and its vCPUs and memory in MiB.
type MacCase<'a> = (&'a str, f64, f64, f64, f64, f64, f64, f64);

/// Rule 2 over every EC2 Mac host: the EBS, attachment, interface, address and host-size
/// figures each one publishes.
#[tokio::test]
async fn an_ec2_mac_host_serves_its_published_figures() {
    let router = emulator();
    let cases: [MacCase; 9] = [
        // The Intel Mac mini host, whose controller dedicates more of both figures than the
        // Apple silicon ones and whose volumes are limited to 16 attachments.
        ("mac1.metal", 14_000.0, 80_000.0, 16.0, 8.0, 30.0, 12.0, 32_768.0),
        ("mac2.metal", 10_000.0, 55_000.0, 27.0, 8.0, 30.0, 8.0, 16_384.0),
        (
            "mac2-m1ultra.metal",
            10_000.0,
            55_000.0,
            27.0,
            8.0,
            30.0,
            20.0,
            131_072.0,
        ),
        ("mac2-m2.metal", 10_000.0, 55_000.0, 27.0, 8.0, 30.0, 8.0, 24_576.0),
        (
            "mac2-m2pro.metal",
            10_000.0,
            55_000.0,
            27.0,
            8.0,
            30.0,
            12.0,
            32_768.0,
        ),
        (
            "mac-m3ultra.metal",
            10_000.0,
            55_000.0,
            27.0,
            8.0,
            30.0,
            28.0,
            262_144.0,
        ),
        ("mac-m4.metal", 10_000.0, 55_000.0, 27.0, 8.0, 30.0, 10.0, 24_576.0),
        (
            "mac-m4pro.metal",
            10_000.0,
            55_000.0,
            27.0,
            8.0,
            30.0,
            14.0,
            49_152.0,
        ),
        // The M4 Max host, whose volumes are limited to 10 attachments.
        (
            "mac-m4max.metal",
            10_000.0,
            55_000.0,
            10.0,
            8.0,
            30.0,
            16.0,
            131_072.0,
        ),
    ];

    for (instance_type, mbps, iops, attachments, interfaces, addresses, vcpus, memory_mib) in
        cases
    {
        let body = described_instance_type(instance_type, &router, REGION, instance_type).await;
        // A whole host has no larger share to burst into, so its dedicated figure is both the
        // baseline and the maximum of its published row, and the MB/s columns are the Mbps
        // ones in the other unit.
        for (member, expected) in [
            ("baselineBandwidthInMbps", mbps),
            ("maximumBandwidthInMbps", mbps),
            ("baselineIops", iops),
            ("maximumIops", iops),
            ("baselineThroughputInMBps", mbps / MEGABITS_PER_MEGABYTE),
            ("maximumThroughputInMBps", mbps / MEGABITS_PER_MEGABYTE),
        ] {
            let path = ["ebsInfo", "ebsOptimizedInfo", member];
            let actual = instance_type_member_number(instance_type, &body, &path);
            assert!(
                (actual - expected).abs() < FIGURE_TOLERANCE,
                "case {instance_type:?}: expected {member} {expected}, got {actual}"
            );
        }
        for (path, expected) in [
            (vec!["ebsInfo", "maximumEbsAttachments"], attachments),
            (vec!["networkInfo", "maximumNetworkInterfaces"], interfaces),
            (card("maximumNetworkInterfaces").to_vec(), interfaces),
            (vec!["networkInfo", "ipv4AddressesPerInterface"], addresses),
            (vec!["networkInfo", "ipv6AddressesPerInterface"], addresses),
            (
                vec!["networkInfo", "ipv4AddressesPerSecondaryInterface"],
                0.0,
            ),
            (vec!["vCpuInfo", "defaultVCpus"], vcpus),
            (vec!["memoryInfo", "sizeInMiB"], memory_mib),
        ] {
            let actual = instance_type_member_number(instance_type, &body, &path);
            assert!(
                (actual - expected).abs() < FIGURE_TOLERANCE,
                "case {instance_type:?}: expected {path:?} {expected}, got {actual}"
            );
        }
        // Every Mac host shares its EBS attachments with its interfaces and its local devices,
        // and the packets of a session are copied by the Nitro controller it is attached to.
        for (path, expected) in [
            (vec!["ebsInfo", "attachmentLimitType"], "shared"),
            (vec!["networkInfo", "trafficMirroringSupport"], "supported"),
        ] {
            let reported = required_instance_type_member(instance_type, &body, &path);
            assert_eq!(
                reported, expected,
                "case {instance_type:?}: expected {path:?} {expected:?}, got {reported:?}"
            );
        }
    }
}

/// Rule 2 for the two facts the Mac hosts differ on: the local SSD the M4 generation presents
/// to its guest, and the queues one interface is given.
///
/// The rows differ along what the rule generalises over: a host that publishes local devices
/// and one that publishes none, and hosts whose rows publish two different queue counts beside
/// ones whose count the catalog does not hold.
#[tokio::test]
async fn a_mac_host_serves_the_devices_and_queues_its_row_publishes() {
    let router = emulator();
    // (the type, the GB of its one local device where it has one, the ENA queues one of its
    // interfaces is given)
    let cases: [(&str, Option<f64>, Option<i32>); 6] = [
        ("mac1.metal", None, None),
        ("mac2.metal", None, Some(32)),
        ("mac2-m2pro.metal", None, Some(32)),
        ("mac-m3ultra.metal", None, None),
        ("mac-m4.metal", Some(1_900.0), None),
        ("mac-m4max.metal", None, Some(8)),
    ];

    for (instance_type, local_gb, queues) in cases {
        let body = described_instance_type(instance_type, &router, REGION, instance_type).await;
        let supported =
            required_instance_type_member(instance_type, &body, &["instanceStorageSupported"]);
        let expected_support = if local_gb.is_some() { "true" } else { "false" };
        assert_eq!(
            supported, expected_support,
            "case {instance_type:?}: expected instanceStorageSupported {expected_support:?}, \
             got {supported:?}"
        );
        match local_gb {
            Some(gb) => {
                for (path, expected) in [
                    (
                        vec!["instanceStorageInfo", "disks", "item", "count"],
                        1.0_f64,
                    ),
                    (vec!["instanceStorageInfo", "disks", "item", "sizeInGB"], gb),
                    (vec!["instanceStorageInfo", "totalSizeInGB"], gb),
                ] {
                    let actual = instance_type_member_number(instance_type, &body, &path);
                    assert!(
                        (actual - expected).abs() < FIGURE_TOLERANCE,
                        "case {instance_type:?}: expected {path:?} {expected}, got {actual}"
                    );
                }
                for member in ["nvmeSupport", "encryptionSupport"] {
                    let path = ["instanceStorageInfo", member];
                    let reported = required_instance_type_member(instance_type, &body, &path);
                    assert_eq!(
                        reported, "required",
                        "case {instance_type:?}: expected {member} \"required\", got {reported:?}"
                    );
                }
            }
            None => {
                let reported = reported_instance_type_member(
                    instance_type,
                    &body,
                    &["instanceStorageInfo", "totalSizeInGB"],
                );
                assert_eq!(
                    reported, None,
                    "case {instance_type:?}: a host with no local device reports no \
                     instanceStorageInfo, got {reported:?}"
                );
            }
        }
        let reported = reported_instance_type_member(
            instance_type,
            &body,
            &card("defaultEnaQueueCountPerInterface"),
        );
        let expected = queues.map(|queues| queues.to_string());
        assert_eq!(
            reported, expected,
            "case {instance_type:?}: expected defaultEnaQueueCountPerInterface {expected:?}, \
             got {reported:?}"
        );
    }
}

/// Rule 2 for the host features the catalog publishes per type, each reported as the enum
/// member or flag its own member takes.
///
/// The rows differ along what the rule generalises over: the one Mac host that carries a local
/// clock beside ones that do not, an accelerated type whose row publishes neither the clock nor
/// reboot migration nor ENA Express, and a pre-Nitro type for contrast.
#[tokio::test]
async fn a_published_row_serves_the_host_features_of_its_type() {
    let router = emulator();
    // (the type, the member path, the value its row publishes)
    let cases: [(&str, [&str; 2], &str); 9] = [
        ("mac-m3ultra.metal", ["", "phcSupport"], "supported"),
        ("mac2.metal", ["", "phcSupport"], "unsupported"),
        ("mac-m4.metal", ["", "phcSupport"], "unsupported"),
        ("gr6f.4xlarge", ["", "phcSupport"], "unsupported"),
        ("gr6f.4xlarge", ["", "rebootMigrationSupport"], "unsupported"),
        ("c4.large", ["", "rebootMigrationSupport"], "supported"),
        ("gr6f.4xlarge", ["networkInfo", "enaSrdSupported"], "false"),
        ("gr6f.4xlarge", ["networkInfo", "enaSupport"], "required"),
        ("c4.large", ["networkInfo", "enaSupport"], "unsupported"),
    ];

    for (instance_type, path, expected) in cases {
        let body = described_instance_type(instance_type, &router, REGION, instance_type).await;
        let path: Vec<&str> = path.iter().copied().filter(|name| !name.is_empty()).collect();
        let reported = required_instance_type_member(instance_type, &body, &path);
        assert_eq!(
            reported, expected,
            "case {instance_type:?}: expected {path:?} {expected:?}, got {reported:?}"
        );
    }
}

/// Rule 2 for the fractional size of the memory-rich graphics family: its accelerator, its
/// local device and the EBS hardware of its own its volumes are attached to.
#[tokio::test]
async fn the_fractional_graphics_size_serves_its_published_row() {
    let router = emulator();
    let case = "gr6f.4xlarge";
    let body = described_instance_type(case, &router, REGION, case).await;

    for (path, expected) in [
        (vec!["ebsInfo", "ebsOptimizedInfo", "baselineBandwidthInMbps"], 8_000.0),
        (vec!["ebsInfo", "ebsOptimizedInfo", "maximumBandwidthInMbps"], 8_000.0),
        (vec!["ebsInfo", "ebsOptimizedInfo", "baselineIops"], 32_000.0),
        (vec!["ebsInfo", "ebsOptimizedInfo", "maximumIops"], 32_000.0),
        (vec!["ebsInfo", "maximumEbsAttachments"], 32.0),
        (vec!["instanceStorageInfo", "disks", "item", "count"], 1.0),
        (vec!["instanceStorageInfo", "disks", "item", "sizeInGB"], 450.0),
        (vec!["gpuInfo", "gpus", "item", "count"], 1.0),
        (
            vec!["networkInfo", "ipv4AddressesPerSecondaryInterface"],
            0.0,
        ),
    ] {
        let actual = instance_type_member_number(case, &body, &path);
        assert!(
            (actual - expected).abs() < FIGURE_TOLERANCE,
            "case {case:?}: expected {path:?} {expected}, got {actual}"
        );
    }
    for (path, expected) in [
        (vec!["ebsInfo", "attachmentLimitType"], "dedicated"),
        (vec!["instanceStorageInfo", "disks", "item", "type"], "ssd"),
        (vec!["gpuInfo", "gpus", "item", "name"], "L4"),
        (vec!["gpuInfo", "gpus", "item", "manufacturer"], "NVIDIA"),
        (vec!["processorInfo", "manufacturer"], "AMD"),
    ] {
        let reported = required_instance_type_member(case, &body, &path);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {path:?} {expected:?}, got {reported:?}"
        );
    }
}

/// Rule 2 for the burstable families either side of the CPU credit model: the first one's size
/// is published as NOT a burstable performance type, with the memory, the root device types,
/// the Amazon EBS encryption and the shared attachment limit of its own row, while the family
/// after it reports the credit model.
#[tokio::test]
async fn the_first_burstable_family_serves_its_published_row() {
    let router = emulator();
    // (the type, whether it is a burstable performance type, its burst model, its memory in
    // MiB, whether its volumes can be encrypted, the kind of its EBS attachment limit)
    let cases: [(&str, &str, &str, f64, &str, &str); 3] = [
        ("t1.micro", "false", "none", 627.0, "supported", "shared"),
        ("t2.micro", "true", "credit", 1_024.0, "supported", "shared"),
        ("t2.large", "true", "credit", 8_192.0, "supported", "shared"),
    ];

    for (instance_type, burstable, burst_model, memory_mib, encryption, limit_kind) in cases {
        let body = described_instance_type(instance_type, &router, REGION, instance_type).await;
        for (path, expected) in [
            (vec!["burstablePerformanceSupported"], burstable),
            (vec!["cpuBurstModel"], burst_model),
            (vec!["ebsInfo", "encryptionSupport"], encryption),
            (vec!["ebsInfo", "attachmentLimitType"], limit_kind),
        ] {
            let reported = required_instance_type_member(instance_type, &body, &path);
            assert_eq!(
                reported, expected,
                "case {instance_type:?}: expected {path:?} {expected:?}, got {reported:?}"
            );
        }
        let actual = instance_type_member_number(instance_type, &body, &["memoryInfo", "sizeInMiB"]);
        assert!(
            (actual - memory_mib).abs() < FIGURE_TOLERANCE,
            "case {instance_type:?}: expected memory {memory_mib} MiB, got {actual}"
        );
        // Both families' AMIs are EBS-backed ones alone: the first one shipped no local
        // devices to boot from, which is what its row publishes.
        let devices = reported_instance_type_list(
            instance_type,
            &body,
            &["supportedRootDeviceTypes"],
        );
        assert_eq!(
            devices,
            vec!["ebs".to_string()],
            "case {instance_type:?}: expected supportedRootDeviceTypes [\"ebs\"], got {devices:?}"
        );
    }
}
