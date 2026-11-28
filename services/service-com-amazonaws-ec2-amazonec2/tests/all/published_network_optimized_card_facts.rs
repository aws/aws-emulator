// SPDX-License-Identifier: CC0-1.0

//! What `DescribeInstanceTypes` serves for the network card of a NETWORK OPTIMIZED instance
//! type, driven over the wire this service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML — README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rule under test: every figure of a type's network card is the figure AWS publishes for
//! THAT type — the Gbps it sustains and the Gbps it reaches
//! (`NetworkCardInfo$BaselineBandwidthInGbps` / `$PeakBandwidthInGbps`), the words of its
//! published "Network performance" column (`$NetworkPerformance`) and the ENA queues each of
//! its interfaces is given (`$DefaultEnaQueueCountPerInterface`) — and none of them is computed
//! from the size's share of its host: a size's published bandwidth is neither its share of the
//! host nor a multiple of the size below it, and the queue count is a figure of the adapter
//! rather than of the vCPUs.
//!
//! The rows span what the rule generalises over, on the fifth generation network optimized
//! compute family (whose published figures the catalog holds row by row): the three narrowest
//! sizes and the widest one that still burst above the share they sustain, the size that holds
//! half of its host, the largest virtualized size that holds all of it, and the bare metal size
//! beside it. The two ends of the ladder differ in every one of the four readings — the queue
//! count rises along the ladder while the published column changes FORM, from the "Up to N
//! Gigabit" of a bursting size to the "N Gigabit" of a dedicated one. Each type is read back
//! through the operation end to end, one ec2Query call per type.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{
    described_instance_type, instance_type_member_number, reported_instance_type_member,
    required_instance_type_member,
};

/// The emulator the calls are made against. An instance type's published hardware is a fact of
/// the catalog rather than stored state, so the engine handle is not needed here.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The Region every call here is signed for: a type's published hardware is the same in each.
const REGION: &str = "eu-west-1";

/// The member path of one figure of the first (and, for every type here, only) network card.
fn card(member: &str) -> [&str; 4] {
    ["networkInfo", "networkCards", "item", member]
}

/// How close two published figures in Gbps have to be to be the same figure.
const FIGURE_TOLERANCE: f64 = 0.0001;

/// One row of the table: the type, the Gbps its card sustains and reaches, the words of its
/// published network performance column, and the ENA queues one of its interfaces is given.
type CardRow<'a> = (&'a str, f64, f64, &'a str, i64);

/// The rule, over the whole published ladder of the family.
#[tokio::test]
async fn a_network_optimized_card_reports_the_figures_published_for_its_type() {
    let router = emulator();
    let cases: [CardRow; 7] = [
        // The sizes that burst to the family's 25 Gigabit ceiling: each sustains the share
        // published for it, which rises faster than the size multiplier between the `xlarge`
        // and the `2xlarge` and slower between the `2xlarge` and the `4xlarge` — and that
        // `4xlarge` is given four times the queues of the size below it while its bandwidth
        // pair reads the same way, so the queue count is a figure of its own.
        ("c5n.large", 3.0, 25.0, "Up to 25 Gigabit", 8),
        ("c5n.xlarge", 5.0, 25.0, "Up to 25 Gigabit", 8),
        ("c5n.2xlarge", 10.0, 25.0, "Up to 25 Gigabit", 8),
        ("c5n.4xlarge", 15.0, 25.0, "Up to 25 Gigabit", 32),
        // Half of the host, held dedicated: the figure it sustains IS the figure it reaches,
        // and its published column carries no "Up to".
        ("c5n.9xlarge", 50.0, 50.0, "50 Gigabit", 32),
        // The largest virtualized size holds the whole 100 Gigabit host.
        ("c5n.18xlarge", 100.0, 100.0, "100 Gigabit", 32),
        // And the bare metal size, whose guest drives the host's own adapter: the same
        // bandwidth and the same queues as the size beside it.
        ("c5n.metal", 100.0, 100.0, "100 Gigabit", 32),
    ];

    for (instance_type, baseline, peak, performance, queues) in cases {
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
        // The type's own network performance is the same published column as its card's: the
        // instance's network IS that one card.
        let instance_performance =
            required_instance_type_member(case, &body, &["networkInfo", "networkPerformance"]);
        assert_eq!(
            instance_performance, performance,
            "case {case:?}: expected networkInfo.networkPerformance {performance:?}, got \
             {instance_performance:?}"
        );
        let reported_queues = reported_instance_type_member(
            case,
            &body,
            &card("defaultEnaQueueCountPerInterface"),
        )
        .map(|text| {
            text.parse::<i64>()
                .unwrap_or_else(|_| panic!("case {case:?}: a queue count, got {text:?}"))
        });
        assert_eq!(
            reported_queues,
            Some(queues),
            "case {case:?}: expected defaultEnaQueueCountPerInterface {queues}, got \
             {reported_queues:?}"
        );
    }
}
