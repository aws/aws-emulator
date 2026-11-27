// SPDX-License-Identifier: CC0-1.0

//! What `DescribeInstanceTypes` serves for the hardware facts AWS publishes PER INSTANCE TYPE
//! and no reading of the type's name yields, driven over the wire this service speaks
//! (`ec2Query`, an `application/x-www-form-urlencoded` POST answered with XML — README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. The ENA queues each of a type's interfaces is given
//!    (`NetworkCardInfo$DefaultEnaQueueCountPerInterface`) are the count the type's own row
//!    publishes for its adapter, whatever its vCPUs — and a type the catalog holds no count for
//!    reports the member ABSENT rather than invented.
//! 2. The bandwidth a card sustains and the bandwidth it can reach (`$BaselineBandwidthInGbps`
//!    / `$PeakBandwidthInGbps`) are the pair published for the type, not the size's share of
//!    its host; the card's `$NetworkPerformance` is the type's published column, which for a
//!    size whose figures do not read as that column is NOT the text of those figures.
//! 3. A card of a type offered an Elastic Fabric Adapter lists BOTH forms of it beside the
//!    ordinary interface — `interface`, `efa` and `efa-only`, the modelled
//!    `NetworkCardInterfaceType` values — and a card of a type offered none lists the ordinary
//!    interface alone.
//! 4. The core counts a type can be configured with (`VCpuInfo$ValidCores`) are the published
//!    set: every count up to its own on the sizes AWS publishes them for, the counts above a
//!    floor where its published list starts above its step, and the ONE count that is its
//!    default where the type takes its cores all at once.
//! 5. How many EBS volumes a type holds attached (`EbsInfo$MaximumEbsAttachments`), whether the
//!    packets of a session can be copied under its guest (`NetworkInfo$TrafficMirroringSupport`)
//!    and the I/O operations per second it sustains (`EbsOptimizedInfo$BaselineIops`) are the
//!    type's own published figures rather than its hypervisor's or its bandwidth's.
//!
//! The rows span what the rules generalise over: three hosts (a fifth generation Intel compute
//! ladder, the AMD EPYC compute one and a pre-Nitro memory optimized one), an accelerated pair,
//! and within each the smallest size, the sizes that burst above their baseline, the sizes whose
//! figure is dedicated, the largest virtualized size and a bare metal one. Each type is read
//! back through the operation end to end, one ec2Query call per type.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{
    described_instance_type, instance_type_member_number, reported_instance_type_list,
    reported_instance_type_member, required_instance_type_member,
};

/// The emulator the tests drive. The catalog is a fact rather than stored state, so the engine
/// handle this hands back is not needed here.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The Region every call here is signed for: an instance type's published hardware is the same
/// in each, so one Region covers the rules.
const REGION: &str = "eu-west-1";

/// The member path of one figure of the first (and, for every type here, only) network card.
fn card(member: &str) -> [&str; 4] {
    ["networkInfo", "networkCards", "item", member]
}

/// How close two published figures in Gbps have to be to be the same figure.
const FIGURE_TOLERANCE: f64 = 0.0001;

/// Rule 1, over four families on three hosts and both ends of each ladder, plus a type whose
/// published count the catalog does not hold.
#[tokio::test]
async fn an_interface_is_given_the_ena_queues_its_type_publishes() {
    let router = emulator();
    // (the type, the queues one of its interfaces is given)
    let cases: [(&str, Option<i64>); 10] = [
        // The pre-Nitro memory optimized family: one adapter, so one count at every size of a
        // ladder whose vCPUs go from two to sixty-four.
        ("r4.large", Some(8)),
        ("r4.2xlarge", Some(8)),
        ("r4.8xlarge", Some(8)),
        ("r4.16xlarge", Some(8)),
        // The memory-rich graphics pair, on a third host again.
        ("gr6.4xlarge", Some(8)),
        ("gr6.8xlarge", Some(8)),
        // The fifth generation Intel compute ladder, including the size that IS the host.
        ("c5.large", Some(8)),
        ("c5.metal", Some(8)),
        // The general purpose family of that same host is given FEWER queues at its narrow
        // sizes, which is why the count is a published figure rather than one per host.
        ("m5.large", Some(2)),
        // And a type the catalog holds no published count for reports the member absent.
        ("mac-m4.metal", None),
    ];

    for (instance_type, queues) in cases {
        let case = instance_type;
        let body = described_instance_type(case, &router, REGION, instance_type).await;
        let reported = reported_instance_type_member(case, &body, &card(
            "defaultEnaQueueCountPerInterface",
        ))
        .map(|text| {
            text.parse::<i64>()
                .unwrap_or_else(|_| panic!("case {case:?}: a queue count, got {text:?}"))
        });
        assert_eq!(
            reported, queues,
            "case {case:?}: expected defaultEnaQueueCountPerInterface {queues:?}, got \
             {reported:?}"
        );
    }
}

/// One row of the bandwidth table: the type, the Gbps its card sustains and can reach, and the
/// words of its published "Network performance" column.
type CardRow<'a> = (&'a str, f64, f64, &'a str);

/// Rule 2, over a whole pre-Nitro ladder and an accelerated pair: the sizes that burst above
/// their baseline, the two that hold a share of their host outright, and the one whose
/// published column and published figures do not read as each other.
#[tokio::test]
async fn a_card_reports_the_bandwidth_pair_published_for_its_type() {
    let router = emulator();
    let cases: [CardRow; 8] = [
        ("r4.large", 0.75, 10.0, "Up to 10 Gigabit"),
        ("r4.xlarge", 1.25, 10.0, "Up to 10 Gigabit"),
        ("r4.2xlarge", 2.5, 10.0, "Up to 10 Gigabit"),
        ("r4.4xlarge", 5.0, 10.0, "Up to 10 Gigabit"),
        // The size that holds half its host publishes 12 Gbps as both the figure it sustains
        // and the figure it reaches, while the column published for it reads as the round 10
        // Gigabit: the card's words are that COLUMN, not the text of the figures beside them.
        ("r4.8xlarge", 12.0, 12.0, "10 Gigabit"),
        ("r4.16xlarge", 25.0, 25.0, "25 Gigabit"),
        // The accelerated pair: the smaller size sustains 10 of the 25 Gbps it can reach, which
        // is neither its share of the host nor the ceiling the derivation would burst it to.
        ("gr6.4xlarge", 10.0, 25.0, "Up to 25 Gigabit"),
        ("gr6.8xlarge", 25.0, 25.0, "25 Gigabit"),
    ];

    for (instance_type, baseline, peak, performance) in cases {
        let case = instance_type;
        let body = described_instance_type(case, &router, REGION, instance_type).await;
        for (member, expected) in [
            ("baselineBandwidthInGbps", baseline),
            ("peakBandwidthInGbps", peak),
        ] {
            let actual = instance_type_member_number(case, &body, &card(member));
            assert!(
                (actual - expected).abs() < FIGURE_TOLERANCE,
                "case {case:?}: expected card {member} {expected}, got {actual}"
            );
        }
        let reported = required_instance_type_member(case, &body, &card("networkPerformance"));
        assert_eq!(
            reported, performance,
            "case {case:?}: expected the card's networkPerformance {performance:?}, got \
             {reported:?}"
        );
    }
}

/// Rule 3, over three families: the two sizes of a ladder either side of the size an adapter is
/// first offered at, an accelerated size that is offered one and one that is not, and a type
/// whose family is offered none at all.
#[tokio::test]
async fn a_card_lists_both_adapter_forms_exactly_where_one_is_offered() {
    let router = emulator();
    // (the type, the interface types its card lists)
    let cases: [(&str, &[&str]); 6] = [
        // The network optimized family of the fifth generation: the Elastic Fabric Adapter is
        // offered from the `9xlarge` up and not below it.
        ("c5n.9xlarge", &["interface", "efa", "efa-only"]),
        ("c5n.metal", &["interface", "efa", "efa-only"]),
        ("c5n.large", &["interface"]),
        // The accelerated pair: the larger size is offered an adapter, the smaller is not.
        ("gr6.8xlarge", &["interface", "efa", "efa-only"]),
        ("gr6.4xlarge", &["interface"]),
        // And a family offered no adapter at any size.
        ("r4.16xlarge", &["interface"]),
    ];

    for (instance_type, interface_types) in cases {
        let case = instance_type;
        let body = described_instance_type(case, &router, REGION, instance_type).await;
        let reported = reported_instance_type_list(
            case,
            &body,
            &["networkInfo", "networkCards", "item", "interfaceTypeSet"],
        );
        assert_eq!(
            reported, interface_types,
            "case {case:?}: expected the card's interfaceTypeSet {interface_types:?}, got \
             {reported:?}"
        );
        // The adapter the list reports is the one the type itself reports being offered.
        let supported = required_instance_type_member(case, &body, &["networkInfo", "efaSupported"]);
        assert_eq!(
            supported,
            reported.contains(&"efa".to_string()).to_string(),
            "case {case:?}: efaSupported {supported:?} disagrees with the card's \
             {reported:?}"
        );
    }
}

/// Rule 4, over two families of one generation: a two-socket size whose published list starts
/// above its step, the size that IS the host and publishes every count, the sizes of the AMD
/// ladder that take their cores all at once, and the sizes below them that do not.
#[tokio::test]
async fn valid_cores_are_the_published_set_of_the_type() {
    let router = emulator();
    let every_count_to = |cores: i64| (1..=cores).collect::<Vec<i64>>();
    let step_two_from = |first: i64, last: i64| {
        (first..=last)
            .filter(|count| count % 2 == 0)
            .collect::<Vec<i64>>()
    };
    // (the type, the core counts it publishes)
    let cases: [(&str, Vec<i64>); 9] = [
        // Two sockets of eighteen cores: the counts go up in twos and start at FOUR.
        ("c5.18xlarge", step_two_from(4, 36)),
        // Two sockets of twenty-four, whose published list starts at its step instead.
        ("c5.24xlarge", step_two_from(2, 48)),
        // The size that holds the whole host publishes every count.
        ("c5.metal", every_count_to(48)),
        // The AMD ladder: every count up to its own on the three narrowest sizes, and the one
        // count that is its default from the `4xlarge` up, which takes its cores all at once.
        ("c5a.xlarge", every_count_to(2)),
        ("c5a.2xlarge", every_count_to(4)),
        ("c5a.4xlarge", vec![8]),
        ("c5a.8xlarge", vec![16]),
        ("c5a.16xlarge", vec![32]),
        ("c5a.24xlarge", vec![48]),
    ];

    for (instance_type, valid_cores) in cases {
        let case = instance_type;
        let body = described_instance_type(case, &router, REGION, instance_type).await;
        let reported: Vec<i64> = reported_instance_type_list(
            case,
            &body,
            &["vCpuInfo", "validCores"],
        )
        .iter()
        .map(|count| {
            count
                .parse()
                .unwrap_or_else(|_| panic!("case {case:?}: a core count, got {count:?}"))
        })
        .collect();
        assert_eq!(
            reported, valid_cores,
            "case {case:?}: expected validCores {valid_cores:?}, got {reported:?}"
        );
    }
}

/// Rule 5: the attachment limit, the mirroring answer and the sustained IOPS of a type are its
/// own published figures.
///
/// The rows hold a bare metal size against the virtualized size of the same host — the volumes
/// of the one come out of the larger pool a whole host carries while the other's come out of the
/// shared Nitro pool, and the packets of a session are copied by the host's own card on both —
/// and a size whose sustained IOPS is not four times its sustained bandwidth as its narrower
/// siblings' are.
#[tokio::test]
async fn attachment_mirroring_and_iops_are_published_per_type() {
    let router = emulator();
    // (the type, its EBS volume limit, how that limit is shared, whether packets can be copied)
    let cases: [(&str, i64, &str, &str); 4] = [
        ("c5.metal", 31, "shared", "supported"),
        ("c5.24xlarge", 27, "shared", "supported"),
        ("c5a.4xlarge", 27, "shared", "supported"),
        // An accelerated type whose volumes are attached to EBS hardware of its own.
        ("gr6.8xlarge", 32, "dedicated", "supported"),
    ];

    for (instance_type, attachments, limit_type, mirroring) in cases {
        let case = instance_type;
        let body = described_instance_type(case, &router, REGION, instance_type).await;
        let reported = instance_type_member_number(
            case,
            &body,
            &["ebsInfo", "maximumEbsAttachments"],
        ) as i64;
        assert_eq!(
            reported, attachments,
            "case {case:?}: expected maximumEbsAttachments {attachments}, got {reported}"
        );
        let reported = required_instance_type_member(case, &body, &["ebsInfo", "attachmentLimitType"]);
        assert_eq!(
            reported, limit_type,
            "case {case:?}: expected attachmentLimitType {limit_type:?}, got {reported:?}"
        );
        let reported =
            required_instance_type_member(case, &body, &["networkInfo", "trafficMirroringSupport"]);
        assert_eq!(
            reported, mirroring,
            "case {case:?}: expected trafficMirroringSupport {mirroring:?}, got {reported:?}"
        );
    }

    // The sustained EBS pair of the AMD size that publishes HALF the dedicated figures of the
    // size above it, beside the narrower size whose IOPS is four times its bandwidth.
    for (case, bandwidth_mbps, iops) in [("c5a.4xlarge", 1_580.0, 6_600.0), ("c5a.2xlarge", 800.0, 3_200.0)] {
        let body = described_instance_type(case, &router, REGION, case).await;
        for (member, expected) in [
            ("baselineBandwidthInMbps", bandwidth_mbps),
            ("baselineIops", iops),
        ] {
            let actual = instance_type_member_number(
                case,
                &body,
                &["ebsInfo", "ebsOptimizedInfo", member],
            );
            assert!(
                (actual - expected).abs() < FIGURE_TOLERANCE,
                "case {case:?}: expected {member} {expected}, got {actual}"
            );
        }
    }
}
