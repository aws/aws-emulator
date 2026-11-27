// SPDX-License-Identifier: CC0-1.0

//! What `DescribeInstanceTypes` serves for ONE network card of an instance type: the bandwidth
//! and the ENA queue figures AWS publishes for that type, rather than a share of its host read
//! off the size multiplier in its name.
//!
//! Every call goes over the wire the service speaks, through the shared `super::ec2_query`
//! harness.
//!
//! The rules under test:
//!
//! 1. A card reports the network bandwidth published for ITS instance type — the figure the
//!    type sustains and the figure it reaches — and the text of that same figure, so two sizes
//!    of one family do not stand in the ratio of their sizes and the two readings of one card
//!    cannot disagree.
//! 2. A card reports the ENA queues one interface of the type is given by default, which is a
//!    figure of the adapter the card carries: the same count across a family's whole ladder,
//!    whatever a size's vCPUs. A type whose published count the catalog does not hold reports
//!    the member absent rather than invented.
//! 3. A card also reports the queue CEILINGS its published row carries — the queues the whole
//!    card holds, the most one interface may be given and the interfaces it takes beyond the
//!    ones the type publishes — and reports all three absent where its row carries none.

use super::ec2_query::{
    self, described_instance_type, instance_type_member_number, reported_instance_type_member,
    required_instance_type_member,
};

/// The Region the catalog is read for.
const REGION: &str = "us-west-2";

/// The member path of one member of the type's FIRST (and, for every type below, only) network
/// card.
fn card(member: &str) -> [String; 4] {
    [
        "networkInfo".to_string(),
        "networkCards".to_string(),
        "item".to_string(),
        member.to_string(),
    ]
}

/// The member path above as the harness takes it.
fn card_path(path: &[String; 4]) -> Vec<&str> {
    path.iter().map(String::as_str).collect()
}

/// What one card publishes about its bandwidth and its queues.
struct CardRow {
    case: &'static str,
    instance_type: &'static str,
    /// `NetworkCardInfo$BaselineBandwidthInGbps` and `$PeakBandwidthInGbps`.
    baseline_gbps: f64,
    peak_gbps: f64,
    /// `NetworkCardInfo$NetworkPerformance`, as the published column writes the pair above.
    performance: &'static str,
    /// `NetworkCardInfo$DefaultEnaQueueCountPerInterface`, or `None` where the catalog holds no
    /// published count for the type and the member is absent.
    default_queues: Option<&'static str>,
}

/// Rules 1 and 2: each row's card serves the bandwidth and the queue count published for that
/// instance type.
///
/// The rows differ along what the rules generalise over: the family (two fifth generation Intel
/// compute families, one with local NVMe devices and one without, a fifth generation general
/// purpose one and the newest Graviton generation), the place of the size on its ladder (a size
/// that bursts above what it sustains, the two sizes carved out of one half of a host, a size
/// whose bandwidth is dedicated, the bare metal size that holds a whole host) and the adapter's
/// own published queue count. The figures are the published network column of each family's
/// specification page and the ENA queue counts of its adapter, read here through the catalog
/// `crate::state::instance_type_info` projects.
#[tokio::test]
async fn a_card_serves_the_bandwidth_and_queues_published_for_its_type() {
    let (router, _engine) = ec2_query::emulator();
    let rows: [CardRow; 11] = [
        CardRow {
            case: "a size that bursts above the bandwidth it sustains",
            instance_type: "c5d.large",
            baseline_gbps: 0.75,
            peak_gbps: 10.0,
            performance: "Up to 10 Gigabit",
            default_queues: Some("8"),
        },
        CardRow {
            case: "the next step up the same ladder",
            instance_type: "c5d.xlarge",
            baseline_gbps: 1.25,
            peak_gbps: 10.0,
            performance: "Up to 10 Gigabit",
            default_queues: Some("8"),
        },
        CardRow {
            case: "the narrower of the two sizes carved out of one half of a host",
            instance_type: "c5d.9xlarge",
            baseline_gbps: 12.0,
            peak_gbps: 12.0,
            performance: "12 Gigabit",
            default_queues: Some("8"),
        },
        CardRow {
            case: "the wider of those two, which reports the same published figure",
            instance_type: "c5d.12xlarge",
            baseline_gbps: 12.0,
            peak_gbps: 12.0,
            performance: "12 Gigabit",
            default_queues: Some("8"),
        },
        CardRow {
            case: "a size that holds a whole host's network",
            instance_type: "c5d.18xlarge",
            baseline_gbps: 25.0,
            peak_gbps: 25.0,
            performance: "25 Gigabit",
            default_queues: Some("8"),
        },
        CardRow {
            case: "the bare metal size of that family",
            instance_type: "c5d.metal",
            baseline_gbps: 25.0,
            peak_gbps: 25.0,
            performance: "25 Gigabit",
            default_queues: Some("8"),
        },
        CardRow {
            case: "the same hardware without local devices",
            instance_type: "c5.2xlarge",
            baseline_gbps: 2.5,
            peak_gbps: 10.0,
            performance: "Up to 10 Gigabit",
            default_queues: Some("8"),
        },
        CardRow {
            case: "a general purpose family of the same generation",
            instance_type: "m5d.large",
            baseline_gbps: 0.75,
            peak_gbps: 10.0,
            performance: "Up to 10 Gigabit",
            default_queues: Some("8"),
        },
        CardRow {
            case: "the bare metal size of the newest Graviton generation",
            instance_type: "c9gd.metal-48xl",
            baseline_gbps: 100.0,
            peak_gbps: 100.0,
            performance: "100 Gigabit",
            default_queues: Some("32"),
        },
        CardRow {
            case: "the virtualized size that holds the same whole host",
            instance_type: "c9gd.48xlarge",
            baseline_gbps: 100.0,
            peak_gbps: 100.0,
            performance: "100 Gigabit",
            default_queues: Some("32"),
        },
        CardRow {
            case: "the sibling family's own size carved out of half a host",
            instance_type: "c5.9xlarge",
            baseline_gbps: 12.0,
            peak_gbps: 12.0,
            performance: "12 Gigabit",
            default_queues: Some("8"),
        },
    ];

    for row in rows {
        let case = row.case;
        let body = described_instance_type(case, &router, REGION, row.instance_type).await;
        for (member, expected) in [
            ("baselineBandwidthInGbps", row.baseline_gbps),
            ("peakBandwidthInGbps", row.peak_gbps),
        ] {
            let path = card(member);
            let actual = instance_type_member_number(case, &body, &card_path(&path));
            assert!(
                (actual - expected).abs() < 0.005,
                "case {case:?}: expected the card of {} to report {member} {expected}, got \
                 {actual}",
                row.instance_type
            );
        }

        let path = card("networkPerformance");
        let performance = required_instance_type_member(case, &body, &card_path(&path));
        assert_eq!(
            performance, row.performance,
            "case {case:?}: expected the card of {} to report networkPerformance {:?}, got \
             {performance:?}",
            row.instance_type, row.performance
        );
        let instance_performance =
            required_instance_type_member(case, &body, &["networkInfo", "networkPerformance"]);
        assert_eq!(
            performance, instance_performance,
            "case {case:?}: the one card of {} carries the whole instance's network, so it must \
             report the instance's own networkPerformance {instance_performance:?}, got \
             {performance:?}",
            row.instance_type
        );

        let path = card("defaultEnaQueueCountPerInterface");
        let queues = reported_instance_type_member(case, &body, &card_path(&path));
        assert_eq!(
            queues.as_deref(),
            row.default_queues,
            "case {case:?}: expected the card of {} to report \
             defaultEnaQueueCountPerInterface {:?}, got {queues:?}",
            row.instance_type,
            row.default_queues
        );
    }
}

/// Rule 1, the invariant behind the rows: a published figure is a FACT of the type, so the
/// bandwidth of two sizes of one family does not stand in the ratio of their sizes.
///
/// Three quarters more vCPUs does not buy three quarters more bandwidth — the two sizes carved
/// out of the same half of a host report the same figure — and the size that holds the whole
/// host reports its own published figure rather than any multiple of a narrower size's.
#[tokio::test]
async fn one_family_s_cards_do_not_scale_by_their_size_multiplier() {
    let (router, _engine) = ec2_query::emulator();
    let case = "two sizes carved out of the same half of a host";
    let baseline = |body: &str| {
        let path = card("baselineBandwidthInGbps");
        instance_type_member_number(case, body, &card_path(&path))
    };
    let nine = described_instance_type(case, &router, REGION, "c5d.9xlarge").await;
    let twelve = described_instance_type(case, &router, REGION, "c5d.12xlarge").await;
    let (nine_gbps, twelve_gbps) = (baseline(&nine), baseline(&twelve));
    assert!(
        (twelve_gbps - nine_gbps).abs() < 0.005,
        "case {case:?}: both sizes publish one figure, got {twelve_gbps} for c5d.12xlarge \
         against {nine_gbps} for c5d.9xlarge"
    );

    let case = "a size against the one two thirds its width";
    let eighteen = described_instance_type(case, &router, REGION, "c5d.18xlarge").await;
    let eighteen_gbps = baseline(&eighteen);
    assert!(
        eighteen_gbps > twelve_gbps,
        "case {case:?}: the wider size must sustain more, got {eighteen_gbps} against \
         {twelve_gbps}"
    );
    assert!(
        (eighteen_gbps - twelve_gbps * 1.5).abs() > 0.005,
        "case {case:?}: c5d.18xlarge sustains {eighteen_gbps} Gbps, which is one and a half \
         times the {twelve_gbps} Gbps of c5d.12xlarge — a size-derived figure rather than a \
         published one"
    );
}

/// The published queue ceilings of one card: the queues the whole card holds, the most one
/// interface of it may be given, and the interfaces it takes beyond the ones its type
/// publishes.
struct QueueLimits {
    held: &'static str,
    per_interface: &'static str,
    additional_interfaces: &'static str,
}

/// The ceilings the published row of a newest-generation size that holds a whole host carries,
/// as the catalog holds them for that hardware.
const WHOLE_HOST_QUEUE_LIMITS: QueueLimits = QueueLimits {
    held: "768",
    per_interface: "128",
    additional_interfaces: "0",
};

/// Rule 3: a card publishes the queue ceilings ITS published row carries, and a card whose row
/// carries none reports all three members absent rather than invented.
///
/// The rows differ along the hardware generation (the newest, whose rows publish the ceilings,
/// and a fifth generation one, whose rows publish the default count alone), along the hypervisor
/// (a bare metal card is the host's own hardware and publishes as much as the virtualized size
/// of the same host), and along what the catalog holds for the type: a row with ceilings, a row
/// with a default count and no ceiling, a row with neither, and a device that predates the
/// adapter altogether.
#[tokio::test]
async fn a_card_publishes_the_queue_ceilings_its_row_carries() {
    let (router, _engine) = ec2_query::emulator();
    let rows: [(&str, &str, Option<QueueLimits>); 6] = [
        (
            "the newest generation's card, which publishes its ceilings",
            "c9gd.metal-48xl",
            Some(WHOLE_HOST_QUEUE_LIMITS),
        ),
        (
            "the virtualized size that holds the same whole host",
            "c9gd.48xlarge",
            Some(WHOLE_HOST_QUEUE_LIMITS),
        ),
        (
            "a fifth generation card, which publishes the default count alone",
            "c5d.24xlarge",
            None,
        ),
        (
            "the bare metal size of that fifth generation family",
            "c5d.metal",
            None,
        ),
        (
            "a type whose published queue count the catalog does not hold",
            "c9gd.12xlarge",
            None,
        ),
        (
            "a device that predates the adapter, whose one queue is published",
            "c1.xlarge",
            None,
        ),
    ];

    for (case, instance_type, limits) in rows {
        let body = described_instance_type(case, &router, REGION, instance_type).await;
        let expected = [
            ("maximumEnaQueueCount", limits.as_ref().map(|l| l.held)),
            (
                "maximumEnaQueueCountPerInterface",
                limits.as_ref().map(|l| l.per_interface),
            ),
            (
                "additionalFlexibleNetworkInterfaces",
                limits.as_ref().map(|l| l.additional_interfaces),
            ),
        ];
        for (member, want) in expected {
            let path = card(member);
            let actual = reported_instance_type_member(case, &body, &card_path(&path));
            assert_eq!(
                actual.as_deref(),
                want,
                "case {case:?}: expected the card of {instance_type} to report {member} \
                 {want:?}, got {actual:?}"
            );
        }

        // A device that predates the adapter presents a single transmit/receive queue, which
        // is the count published for every one of them — and no ceiling on it.
        if instance_type == "c1.xlarge" {
            let path = card("defaultEnaQueueCountPerInterface");
            let queues = reported_instance_type_member(case, &body, &card_path(&path));
            assert_eq!(
                queues.as_deref(),
                Some("1"),
                "case {case:?}: expected the one queue of a pre-adapter device for \
                 {instance_type}, got {queues:?}"
            );
        }
    }
}

/// Reading a type twice reports it identically: an instance type is a published fact, so
/// neither call changes what the other sees.
#[tokio::test]
async fn describing_a_type_twice_reports_the_same_card() {
    let (router, _engine) = ec2_query::emulator();
    let case = "the same type read twice";
    for instance_type in ["c5d.9xlarge", "c9gd.metal-48xl"] {
        let first = described_instance_type(case, &router, REGION, instance_type).await;
        let second = described_instance_type(case, &router, REGION, instance_type).await;
        for member in [
            "baselineBandwidthInGbps",
            "peakBandwidthInGbps",
            "defaultEnaQueueCountPerInterface",
            "maximumEnaQueueCount",
            "maximumEnaQueueCountPerInterface",
            "additionalFlexibleNetworkInterfaces",
        ] {
            let path = card(member);
            let before = reported_instance_type_member(case, &first, &card_path(&path));
            let after = reported_instance_type_member(case, &second, &card_path(&path));
            assert_eq!(
                before, after,
                "case {case:?}: {instance_type} reported {member} {before:?} and then \
                 {after:?}"
            );
        }
    }
}
