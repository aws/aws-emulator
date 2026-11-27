// SPDX-License-Identifier: CC0-1.0

//! WHICH of two faults `GetPredictiveScalingForecast` reports when a request names a group the
//! account does not hold AND asks for a span the operation would refuse.
//!
//! RULE: the resource REFERENCE a request names is resolved before the values the request
//! carries are weighed, so a forecast asked for a group this account holds none under is
//! refused `ValidationError` naming that group — `Group <name> not found.` — whatever
//! `StartTime` and `EndTime` state. A caller told its timestamps are wrong would adjust them
//! and retry forever while the real fault, the group it named, stayed invisible.
//!
//! The rows span what the rule generalises over: a span the operation refuses for reaching
//! further back than it may, one it refuses for closing before it opens, one it refuses for
//! being longer than it serves, and one it SERVES — the same refusal for all four is what says
//! the group is resolved first. Each row runs against a group the account does not hold and
//! against one it does, so a rule that refused the span too late could not pass both halves:
//! the held group is answered the span refusal the row's span earns, and the absent one is
//! answered the group.
//!
//! The bounds the spans are built from are the published ones, written out here rather than
//! taken from the crate — `tests/all/predictive_scaling_forecast.rs` owns the table that pins
//! each bound and the hour grid the forecast is answered over.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    create_launch_template, emulator, p, query, tag_value, xml_unescape, REGION,
};

/// The group the account HOLDS, the policy it carries, and the name no group answers to.
const HELD_GROUP: &str = "asg-forecast-precedence";
const POLICY: &str = "forecast-precedence-policy";
const ABSENT_GROUP: &str = "asg-forecast-precedence-never-created";

/// The launch template the held group launches from, created over Amazon EC2's own wire.
const TEMPLATE: &str = "lt-forecast-precedence";

/// One hour, the period a forecast reports one value per
/// (`GetPredictiveScalingForecast`'s own grid).
const PERIOD: Duration = Duration::from_secs(3600);

/// The published bounds the spans below are built from: a forecast may be asked for over at
/// most 30 periods-per-day's worth of days, and may reach at most one year back
/// (`GetPredictiveScalingForecastType$StartTime`, `$EndTime`).
const MAX_RANGE: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_LOOKBACK: Duration = Duration::from_secs(365 * 24 * 3600);

/// The sentence this operation answers a group the account does not hold with: fixed text
/// around the ONE name the request stated, closed as a sentence. Written out here rather than
/// taken from the implementation, which would assert nothing.
fn absent_group_message(name: &str) -> String {
    format!("Group {name} not found.")
}

/// `at` as the RFC 3339 instant a request states a timestamp in.
fn rfc3339(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).unwrap_or_default())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the instant renders")
}

/// The whole hour `at` falls in, so a span states the grid's own instants.
fn whole_hour(at: SystemTime) -> SystemTime {
    let secs = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    UNIX_EPOCH + Duration::from_secs(secs - secs % PERIOD.as_secs())
}

/// Post one `GetPredictiveScalingForecast` and answer its status, wire code and message.
async fn get_forecast(
    router: &MultiServiceRouter,
    group: &str,
    start: SystemTime,
    end: SystemTime,
) -> (u16, String, String, String) {
    let (status, body) = query(
        router,
        "GetPredictiveScalingForecast",
        &[
            ("AutoScalingGroupName", group),
            ("PolicyName", POLICY),
            ("StartTime", &rfc3339(start)),
            ("EndTime", &rfc3339(end)),
        ],
    )
    .await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
        body,
    )
}

/// One emulator holding the group and its predictive scaling policy.
async fn seeded() -> MultiServiceRouter {
    let (router, _engine) = emulator();
    create_launch_template(&router, TEMPLATE).await;
    let zone = format!("{REGION}a");
    let create = [
        p("AutoScalingGroupName", HELD_GROUP),
        p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
        p("MinSize", "0"),
        p("MaxSize", "4"),
        p("AvailabilityZones.member.1", zone.as_str()),
    ];
    let borrowed: Vec<(&str, &str)> = create
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed).await;
    assert_eq!(
        status, 200,
        "the group the rows read a forecast of must be created, got {status}: {body}"
    );

    let (status, body) = query(
        &router,
        "PutScalingPolicy",
        &[
            ("AutoScalingGroupName", HELD_GROUP),
            ("PolicyName", POLICY),
            ("PolicyType", "PredictiveScaling"),
            (
                "PredictiveScalingConfiguration.MetricSpecifications.member.1.TargetValue",
                "50",
            ),
            (
                "PredictiveScalingConfiguration.MetricSpecifications.member.1.\
                 PredefinedMetricPairSpecification.PredefinedMetricType",
                "ASGCPUUtilization",
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the predictive scaling policy must be put, got {status}: {body}"
    );
    router
}

#[tokio::test]
async fn the_group_reference_is_resolved_before_the_span_is_weighed() {
    let router = seeded().await;
    let next_hour = whole_hour(SystemTime::now()) + PERIOD;
    // Each row: the case, the span it asks for, and the sentence a request naming the HELD
    // group earns for that span — empty where the span is served.
    let rows: Vec<(&str, SystemTime, SystemTime, String)> = vec![
        (
            "a start further back than the operation looks",
            whole_hour(SystemTime::now()) - MAX_LOOKBACK - PERIOD,
            whole_hour(SystemTime::now()) - MAX_LOOKBACK,
            "At most, the date and time can be one year before the current date and time."
                .to_string(),
        ),
        (
            "a window that closes before it opens",
            next_hour,
            next_hour - PERIOD,
            "Given end time must be after start time".to_string(),
        ),
        (
            "a span longer than the operation serves",
            next_hour,
            next_hour + MAX_RANGE + PERIOD,
            format!(
                "The maximum time duration between the start and end time is {} days.",
                MAX_RANGE.as_secs() / (24 * PERIOD.as_secs())
            ),
        ),
        ("a span the operation serves", next_hour, next_hour + PERIOD, String::new()),
    ];

    for (case, start, end, span_refusal) in rows {
        // The group the account does not hold: the same sentence whatever the span states.
        let (status, code, message, body) = get_forecast(&router, ABSENT_GROUP, start, end).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "ValidationError",
                absent_group_message(ABSENT_GROUP).as_str()
            ),
            "case {case:?}: expected the group that does not exist to be reported, got \
             {status} {code:?} {message:?} with {body}"
        );

        // And the group it DOES hold: the span is weighed once the reference resolves, so the
        // row's own span refusal is reachable — which is what makes the answers above a
        // statement about precedence rather than about a check that never runs.
        let (status, code, message, body) = get_forecast(&router, HELD_GROUP, start, end).await;
        if span_refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: the served span must answer the forecast for the held group, \
                 got {status} {code:?} {message:?} with {body}"
            );
            continue;
        }
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationError", span_refusal.as_str()),
            "case {case:?}: expected the span refusal for the held group, got {status} \
             {code:?} {message:?} with {body}"
        );
    }
}
