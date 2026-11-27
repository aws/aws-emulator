// SPDX-License-Identifier: CC0-1.0

//! What a CloudWatch reader still finds of an instance's `AWS/EC2` series once that instance is
//! GONE: the history a client reads back after terminating the instance it measured.
//!
//! THE RULE: a datapoint of an instance's `AWS/EC2` series is a reading that was TAKEN on that
//! instance's guest at the boundary it is stamped with, so the series is a record of measurements
//! already made: it belongs to the account's metric store rather than to the instance, and
//! terminating the instance takes none of those readings away and adds none. An instance that is
//! gone is being measured by nothing — "Only a running instance is measured" is the other half of
//! the same fact — so no boundary past its last running instant carries a reading of it, while
//! every boundary it was running at still does, and `ListMetrics` still reports the series that
//! was published into, under exactly the `{InstanceId}` dimension it was published under. None of
//! that depends on the cadence the instance published at: the cadence decides WHICH boundaries
//! carry a reading ("Basic monitoring | Status check metrics are available in 1-minute periods.
//! All other metrics are available in 5-minute periods" against "Detailed monitoring | You can get
//! metrics in 1-minute periods"; Amazon EC2 User Guide, instance monitoring), not whether the
//! readings outlive the instance.
//!
//! That is what a client reading an instance's utilisation across its whole life observes: it
//! launches the instance, watches its series fill up, terminates it and then reads the series
//! again — for a post-mortem of a machine that is no longer there, which is only possible if the
//! history is still served for an instance the API no longer runs.
//!
//! The rows span what the rule generalises over: the monitoring state the instance published under
//! and so its period (five minutes while detailed monitoring is off, one minute while it is on),
//! how detailed monitoring was reached (asked for at the launch, or turned on afterwards by
//! `MonitorInstances`), the metric and the unit it is measured in (a percentage, bytes and a
//! count). Every expected period and unit is spelled out here rather than imported, so a wrong
//! constant in the implementation cannot make a row pass. The catalog, the units and the two
//! cadences come from the metrics Amazon EC2 documents for one INSTANCE (Amazon EC2 User Guide,
//! "Instance metrics") and from the monitoring sentences quoted above; they are documented metrics
//! rather than Smithy shapes, so there is no model member to read them from. A status-check
//! series, whose readings exist only from the instant its check reaches a verdict, has its own
//! window pinned by the sibling suite [`super::instance_metric_series_window`]; what is driven
//! here is the guest measurements, whose period the monitoring state selects.
//!
//! The harness is the sibling suite's ([`super::instance_metric_publication`]): Amazon CloudWatch
//! mounted BESIDE Amazon EC2 on one engine, as the assembled router mounts them, every datapoint
//! read back through CloudWatch's own `GetMetricStatistics` at the one-minute granularity a client
//! asks an instance's series for, and the sampler's own clock advanced over a span of periods
//! rather than waited for — so three periods of the 5-minute cadence are covered without spending
//! fifteen minutes on them. Each read opens WELL BEFORE the launch and closes WELL AFTER the span,
//! so a reading stamped outside the instance's running life is seen rather than hidden by the
//! window.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::instance_cpu_metric_read_periods::listed_series;
use super::instance_metric_publication::{
    datapoints, ec2_ok, emulator, launch, now_secs, p, poll_until_reported, sample_at, Datapoint,
    FIVE_MINUTES, INSTANCE_ID_DIMENSION, MINUTE, NAMESPACE,
};

/// The image every launch here names, as the sibling suites name it.
const AMI: &str = "ami-0123456789abcdef0";

/// How far into each minute of a span the sampler is asked for the readings due, in seconds: NOT
/// on a boundary of either cadence, so a datapoint stamped with a boundary can only have been
/// stamped with the instant its reading was TAKEN at rather than with the instant the sampler
/// happened to look.
const SAMPLE_OFFSET_SECS: i64 = 7;

/// How many minutes of the sampler's clock the instance is sampled over while it RUNS: wide enough
/// to hold several boundaries of the longest cadence, so the history that must survive the
/// termination is more than one reading.
const RUNNING_MINUTES: i64 = 11;

/// How many further minutes of the sampler's clock are driven once the instance is GONE: past
/// several boundaries of BOTH cadences, so a reading published for an instance that is no longer
/// there would be caught whichever cadence it was publishing at.
const MINUTES_AFTER_TERMINATION: i64 = 10;

/// How far back of the launch every read opens, in seconds: many periods of the longest cadence,
/// so a datapoint stamped before the instance came up is inside the window rather than cut off by
/// its start.
const READ_FROM_BEFORE_LAUNCH: i64 = 6 * FIVE_MINUTES;

/// How far past the end of the whole driven span every read closes, in seconds.
const READ_PAST_THE_SPAN: i64 = 6 * FIVE_MINUTES;

/// How the instance of a row reached the monitoring state it published under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Monitoring {
    /// Detailed monitoring off, which is what a launch that says nothing about it leaves — the
    /// state an instance publishes its guest measurements at the 5-minute cadence in.
    Basic,
    /// Detailed monitoring asked for at the launch (`Monitoring.Enabled=true`).
    DetailedAtLaunch,
    /// Detailed monitoring turned on afterwards, by `MonitorInstances`.
    DetailedByCall,
}

impl Monitoring {
    /// The `Monitoring.State` `DescribeInstances` reports once the move this row makes has
    /// settled, and still reports after the termination: terminating an instance is a lifecycle
    /// move, so it does not take its detailed monitoring away.
    fn settled_state(self) -> &'static str {
        match self {
            Self::Basic => "disabled",
            Self::DetailedAtLaunch | Self::DetailedByCall => "enabled",
        }
    }
}

/// One row of the table: a metric, the unit it is measured in, the monitoring state the instance
/// published it under and the period its readings were taken at under that state.
struct Row {
    /// What the row is called in a failure.
    case: &'static str,
    /// How the instance reached its monitoring state.
    monitoring: Monitoring,
    /// The `MetricName` the series is published under.
    metric: &'static str,
    /// The `StandardUnit` its datapoints must carry.
    unit: &'static str,
    /// The period its readings were taken at, in seconds.
    period: i64,
}

/// The rows this suite drives: both cadences, both ways into detailed monitoring, and three of the
/// units a guest measurement is published in.
const ROWS: &[Row] = &[
    Row {
        case: "the CPU series of an instance that ran under basic monitoring",
        monitoring: Monitoring::Basic,
        metric: "CPUUtilization",
        unit: "Percent",
        period: FIVE_MINUTES,
    },
    Row {
        case: "the outbound network series of an instance that ran under basic monitoring",
        monitoring: Monitoring::Basic,
        metric: "NetworkOut",
        unit: "Bytes",
        period: FIVE_MINUTES,
    },
    Row {
        case: "a counted series of an instance that ran under basic monitoring",
        monitoring: Monitoring::Basic,
        metric: "MetadataNoToken",
        unit: "Count",
        period: FIVE_MINUTES,
    },
    Row {
        case: "the CPU series of an instance launched with detailed monitoring",
        monitoring: Monitoring::DetailedAtLaunch,
        metric: "CPUUtilization",
        unit: "Percent",
        period: MINUTE,
    },
    Row {
        case: "a counted series of an instance moved to detailed monitoring",
        monitoring: Monitoring::DetailedByCall,
        metric: "DiskWriteOps",
        unit: "Count",
        period: MINUTE,
    },
];

/// The start of the `period`-wide window `at` falls in, computed here rather than imported so the
/// expected grid is this suite's own.
fn window_start(at: i64, period: i64) -> i64 {
    at - at.rem_euclid(period)
}

/// The base of the span a row drives: a 5-minute boundary AHEAD of the wall clock, so the span is
/// laid over whole periods of both cadences and over instants the service's own clock has not
/// reached — what the sampler publishes into the span is therefore the span's own readings.
fn span_base() -> i64 {
    window_start(now_secs(), FIVE_MINUTES) + 2 * FIVE_MINUTES
}

/// An instance running under the monitoring state `monitoring` names, and the instant its launch
/// was asked for.
///
/// The instant is taken BEFORE the launch call, so it is at or before the launch the service
/// recorded — which is what makes "nothing stamped before the launch" an assertion that cannot
/// pass by a rounding.
async fn running_instance(
    router: &MultiServiceRouter,
    case: &str,
    monitoring: Monitoring,
) -> (String, i64) {
    let launched_at = now_secs();
    let instance_id = if monitoring == Monitoring::DetailedAtLaunch {
        let body = ec2_ok(
            router,
            case,
            "RunInstances",
            &[
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("Monitoring.Enabled", "true"),
            ],
        )
        .await;
        super::ec2_query::element(&body, "instanceId").unwrap_or_else(|| {
            panic!("case {case:?}: RunInstances must report the instance it minted: {body}")
        })
    } else {
        launch(router, case, 1).await.remove(0)
    };
    if monitoring == Monitoring::DetailedByCall {
        ec2_ok(
            router,
            case,
            "MonitorInstances",
            &[p("InstanceId.1", instance_id.clone())],
        )
        .await;
    }
    poll_until_reported(
        router,
        case,
        &instance_id,
        "running",
        monitoring.settled_state(),
    )
    .await;
    (instance_id, launched_at)
}

/// Ask the sampler for the readings due at each minute of `minutes` of the span based at `base`,
/// [`SAMPLE_OFFSET_SECS`] into each — the sequence of looks the running emulator's own clock
/// takes, asked for at instants of this suite's choosing.
fn sample_minutes(engine: &engine::InMemoryEngine, base: i64, minutes: std::ops::Range<i64>) {
    for minute in minutes {
        sample_at(engine, base + minute * MINUTE + SAMPLE_OFFSET_SECS);
    }
}

/// Terminate the instance `instance_id` names and wait until the service reports it `terminated`,
/// with the monitoring state the row's move left it in.
async fn terminate(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    monitoring: Monitoring,
) {
    ec2_ok(
        router,
        case,
        "TerminateInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    poll_until_reported(
        router,
        case,
        instance_id,
        "terminated",
        monitoring.settled_state(),
    )
    .await;
}

/// The boundaries of `period` inside `[base, last_look]` — the instants a series published at
/// `period` was read at over a span the sampler was asked about up to `last_look`.
fn boundaries_in(base: i64, period: i64, last_look: i64) -> Vec<i64> {
    let mut boundaries = Vec::new();
    let mut at = base;
    while at <= last_look {
        boundaries.push(at);
        at += period;
    }
    boundaries
}

/// Rule: the readings taken while an instance ran are still served after that instance is
/// terminated, and no boundary past its last running instant carries a reading of it — whichever
/// cadence it published at.
#[tokio::test]
async fn the_series_of_a_terminated_instance_still_serves_what_it_measured_while_it_ran() {
    for row in ROWS {
        let case = row.case;
        let (router, engine) = emulator();
        let (instance_id, launched_at) = running_instance(&router, case, row.monitoring).await;

        // The span the instance is sampled over while it RUNS, and the last instant the sampler
        // was asked about in it: every reading of this instance was taken at a boundary at or
        // before that instant, since a reading is taken at a boundary the sampler's clock has
        // passed.
        let base = span_base();
        sample_minutes(&engine, base, 0..RUNNING_MINUTES);
        let last_look = base + (RUNNING_MINUTES - 1) * MINUTE + SAMPLE_OFFSET_SECS;

        terminate(&router, case, &instance_id, row.monitoring).await;

        // Read only AFTER the instance is gone: nothing is measuring it any more, so this is the
        // history the termination left — which is what a post-mortem of the machine reads.
        let start = launched_at - READ_FROM_BEFORE_LAUNCH;
        let end =
            base + (RUNNING_MINUTES + MINUTES_AFTER_TERMINATION) * MINUTE + READ_PAST_THE_SPAN;
        let kept = datapoints(&router, case, &instance_id, row.metric, start, end).await;
        let stamps: Vec<i64> = kept.iter().map(|point| point.at).collect();
        for boundary in boundaries_in(base, row.period, last_look) {
            assert!(
                stamps.contains(&boundary),
                "case {case:?}: the reading taken at {boundary}, a boundary of this \
                 {}-second period the instance was running at, must still be served after the \
                 termination, got {kept:?}",
                row.period
            );
        }
        assert_point_invariants(case, row, &kept, launched_at, last_look);

        // Several boundaries of BOTH cadences past the termination: an instance that is gone is
        // measured by nothing, so none of them carries a reading of it and the history is
        // unchanged.
        sample_minutes(
            &engine,
            base,
            RUNNING_MINUTES..(RUNNING_MINUTES + MINUTES_AFTER_TERMINATION),
        );
        let after = datapoints(&router, case, &instance_id, row.metric, start, end).await;
        assert_eq!(
            after, kept,
            "case {case:?}: the series of a terminated instance keeps the readings taken while it \
             ran and gains no other, expected {kept:?}, got {after:?}"
        );
        assert_point_invariants(case, row, &after, launched_at, last_look);
    }
}

/// What every datapoint of a row's series must satisfy, before and after the boundaries past the
/// termination are driven: it is stamped on its own period's grid, inside the instance's running
/// life, and carries that metric's own unit as one observation.
fn assert_point_invariants(
    case: &str,
    row: &Row,
    points: &[Datapoint],
    launched_at: i64,
    last_look: i64,
) {
    assert!(
        !points.is_empty(),
        "case {case:?}: the instance published while it ran, so its series cannot be empty"
    );
    for point in points {
        assert_eq!(
            point.at.rem_euclid(row.period),
            0,
            "case {case:?}: a reading is taken at a boundary of its own {}-second period, got \
             {point:?}",
            row.period
        );
        assert!(
            point.at > launched_at,
            "case {case:?}: nothing was measured before the launch at {launched_at}, got {point:?}"
        );
        assert!(
            point.at <= last_look,
            "case {case:?}: every reading was taken at a boundary the sampler had passed while \
             the instance ran, the last of which was {last_look}, got {point:?}"
        );
        assert_eq!(
            point.unit, row.unit,
            "case {case:?}: expected the unit {:?}, got {point:?}",
            row.unit
        );
        assert_eq!(
            point.sample_count, 1.0,
            "case {case:?}: one reading of the instance was taken per period, got {point:?}"
        );
    }
}

/// Rule: a series that was published into is a metric of the account, so `ListMetrics` reports it
/// after the instance that published it is gone too — under exactly the `{InstanceId}` dimension
/// of that instance.
#[tokio::test]
async fn list_metrics_reports_the_series_of_a_terminated_instance() {
    let case = "the series of an instance terminated after it published";
    let (router, engine) = emulator();
    let (instance_id, _launched_at) = running_instance(&router, case, Monitoring::Basic).await;
    let base = span_base();
    sample_minutes(&engine, base, 0..RUNNING_MINUTES);
    terminate(&router, case, &instance_id, Monitoring::Basic).await;

    let listed = listed_series(
        &router,
        case,
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": INSTANCE_ID_DIMENSION, "Value": instance_id.clone()}],
        }),
    )
    .await;
    // The series of the metrics this suite's rows read, each still reported for the instance that
    // is gone, dimensioned on that instance alone.
    for metric in ["CPUUtilization", "NetworkOut", "MetadataNoToken"] {
        let expected = (
            metric.to_string(),
            vec![(INSTANCE_ID_DIMENSION.to_string(), instance_id.clone())],
        );
        assert!(
            listed.contains(&expected),
            "case {case:?}: {metric} was published for {instance_id} while it ran, so it is still \
             a metric of the account after the termination, got {listed:?}"
        );
    }
    for (metric, dimensions) in &listed {
        assert_eq!(
            dimensions,
            &vec![(INSTANCE_ID_DIMENSION.to_string(), instance_id.clone())],
            "case {case:?}: {metric} is dimensioned on that instance alone, got {dimensions:?}"
        );
    }
}
