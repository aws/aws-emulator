// SPDX-License-Identifier: CC0-1.0

//! An Amazon CloudWatch metric alarm over the health series an Amazon Redshift cluster
//! publishes about ITSELF — evaluated, where a client reads the verdict.
//!
//! The rule under test: **a metric alarm is evaluated against the datapoints its whole metric
//! identity selects — namespace, metric name AND the complete dimension set — whoever
//! published them, a service's own `AWS/*` series included, so an alarm over a series that is
//! being published converges to the state its `ComparisonOperator`/`Threshold` pair and its
//! m-of-n predicate imply over that series, while an alarm whose dimension set selects a
//! series NOBODY published keeps the `INSUFFICIENT_DATA` the write that created it left
//! behind and records no `StateUpdate` in its history.** Which per-node dimension sets a
//! cluster publishes follows from the cluster's own topology and from nothing else, so the
//! same `NodeID` is a live series on one cluster shape and an empty one on another.
//!
//! Two crates have to agree for that to hold — Redshift publishes the observations, CloudWatch
//! evaluates the alarms over them and answers for both `DescribeAlarms` and
//! `DescribeAlarmHistory` — so the suite lives here: a crate-local test target can only ever
//! see one side of it.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the cluster's SHAPE, which is what decides which node series exist — a `single-node`
//!   cluster (one `Shared` node) and a `multi-node` one (a `Leader` node and one series per
//!   compute node);
//! * the dimension set the alarm watches — cluster-wide, each node the cluster really has, and
//!   `NodeID` values that are well formed but name no node of THAT cluster;
//! * the comparison the alarm is decided by: every single-threshold `ComparisonOperator` the
//!   model publishes (`smithy_query` shape `ComparisonOperator`), at a threshold on each side
//!   of the value the series carries, so each operator is exercised in both of the states it
//!   can produce over the same datapoints.
//!
//! The configuration an issue happens to record (one single-node cluster, three alarms) is a
//! single row of this table. The value those thresholds are placed around is not assumed: each
//! published series is read once through `GetMetricStatistics` and asserted to carry it, so the
//! table's premise is checked rather than taken on trust.
//!
//! Every call goes over the wire of the service that owns it — Redshift's awsQuery form wire,
//! CloudWatch's awsJson one — through the same `MultiServiceRouter` type the production router
//! builds, on ONE shared engine the way `core/router/src/lib.rs` mounts every service on one.
//! Only the three services a row reaches are registered, so what the suite drives is a SUBSET
//! of the production assembly rather than the whole of it: a cluster, the subnets it is
//! launched into and the metric store its health lands in.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state as redshift;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{authorization, cw};
use crate::query_wire::{elements, form_encode};

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// The `Version` every Redshift awsQuery request carries.
const REDSHIFT_VERSION: &str = "2012-12-01";

/// How close two numbers that travelled as JSON have to be to count as the same value.
const TOLERANCE: f64 = 1e-6;

/// How far back the premise read below starts: an hour, which holds far more than a freshly
/// created cluster has published and stays well inside the datapoints one `GetMetricStatistics`
/// call may return.
const READ_WINDOW_SECONDS: f64 = 60.0 * 60.0;

/// The node type every cluster below is created with.
const NODE_TYPE: &str = "ra3.xlplus";

/// The period, evaluation-period count and datapoint count every alarm below is configured
/// with: one minute is the cadence the health check publishes at and the finest period a
/// regular-resolution metric can be alarmed at, and one-of-one is the predicate that makes a
/// single published datapoint decide the alarm.
const PERIOD_SECONDS: i64 = 60;
const EVALUATION_PERIODS: i64 = 1;
const DATAPOINTS_TO_ALARM: i64 = 1;

/// How long a poll loop is given before it calls the condition it waits on unobservable, and
/// how often it looks.
///
/// Generous on purpose: what is waited for is a cluster's provisioning span followed by the
/// accelerated publication of its series and the alarm evaluation that span owes — seconds at
/// most — so the deadline is only here to turn a hang into a failure, and a loaded host costs
/// a loop more polls rather than failing it.
const DEADLINE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);

/// One comparison a row is decided by: the `ComparisonOperator`, the `Threshold` beside it,
/// and the state that pair implies for a series holding the healthy flag.
type Comparison = (&'static str, f64, &'static str);

/// One dimension set an alarm table is built over: how a failure message names it, the `NodeID`
/// it carries (`None` for the cluster-wide series), and the comparisons it is watched through.
type WatchedSet = (String, Option<String>, &'static [Comparison]);

/// The `StateValue` members the model publishes (`smithy_query` shape `StateValue`:
/// `OK | ALARM | INSUFFICIENT_DATA`).
const STATE_OK: &str = "OK";
const STATE_ALARM: &str = "ALARM";
const STATE_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// Every single-threshold `ComparisonOperator` the model publishes (`smithy_query` shape
/// `ComparisonOperator`), each at a threshold on both sides of the healthy flag the series
/// carries, paired with the state that comparison then puts the alarm in.
///
/// The remaining three members of that enum — `LessThanLowerOrGreaterThanUpperThreshold`,
/// `LessThanLowerThreshold` and `GreaterThanUpperThreshold` — compare against an anomaly
/// detection BAND rather than a number (`MetricAlarm$ThresholdMetricId`), so they are not rows
/// of this table: an alarm naming one carries no `Threshold` at all.
///
/// (operator, threshold, the state it implies for a series holding the healthy flag)
const COMPARISONS: &[Comparison] = &[
    ("GreaterThanThreshold", 0.5, STATE_ALARM),
    ("GreaterThanThreshold", 1.0, STATE_OK),
    ("GreaterThanOrEqualToThreshold", 1.0, STATE_ALARM),
    ("GreaterThanOrEqualToThreshold", 1.5, STATE_OK),
    ("LessThanThreshold", 1.0, STATE_OK),
    ("LessThanThreshold", 1.5, STATE_ALARM),
    ("LessThanOrEqualToThreshold", 1.0, STATE_ALARM),
    ("LessThanOrEqualToThreshold", 0.5, STATE_OK),
];

/// The two comparisons every FURTHER dimension set is watched through: one that breaches the
/// healthy flag and one that does not, so each series is checked in both directions without
/// repeating the whole table per node.
const BOTH_DIRECTIONS: &[Comparison] = &[
    ("GreaterThanThreshold", 0.5, STATE_ALARM),
    ("LessThanThreshold", 1.0, STATE_OK),
];

/// The three services this suite exercises, mounted on ONE shared engine the way the assembled
/// router mounts every service on one — a SUBSET of that assembly, holding just the services a
/// row reaches.
///
/// Amazon EC2 is there because a provisioned cluster is launched into a VPC: `CreateCluster`
/// resolves the subnets of the account's default VPC through that service.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_redshift_redshiftserviceversion20121201::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one Redshift awsQuery action with these form parameters, requiring it to succeed, and
/// answer the XML body it served.
async fn redshift_call(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={REDSHIFT_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("redshift.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("redshift", REGION))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let served = String::from_utf8_lossy(response.body()).into_owned();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Redshift {action}, got {status} with body {served}"
    );
    served
}

/// One CloudWatch operation over its awsJson wire, requiring it to succeed: the SHARED caller
/// this test crate already has for that service, so the target header and the credential scope
/// that routes the call are written in one place.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    cw(router, case, REGION, operation, request).await
}

/// One cluster shape: what the case creates, and which `NodeID` values its topology does and
/// does not publish a series for.
struct ClusterRow {
    case: &'static str,
    identifier: &'static str,
    /// The `CreateCluster` members beyond the three every row passes.
    params: Vec<(&'static str, &'static str)>,
    /// The `NodeID` of every node this cluster publishes a series for.
    node_ids: Vec<&'static str>,
    /// `NodeID` values that are well formed but name no node of THIS cluster.
    absent_node_ids: Vec<&'static str>,
}

/// One alarm: the dimension set it watches, the comparison that decides it, and the
/// `StateValue` the rule says `DescribeAlarms` must converge to.
#[derive(Debug, Clone)]
struct AlarmRow {
    case: String,
    name: String,
    /// The cluster whose series this alarm watches.
    cluster: &'static str,
    /// `None` for the cluster-wide series, `Some` for the series of that `NodeID`.
    node_id: Option<String>,
    comparison: &'static str,
    threshold: f64,
    expected: &'static str,
    /// Whether any datapoint at all answers this alarm's dimension set, which is what decides
    /// whether an evaluation can move it off the state its creation left behind.
    published: bool,
}

/// The dimension set one alarm watches, as `PutMetricAlarm` takes it.
fn dimensions(cluster: &str, node_id: Option<&str>) -> Value {
    let mut set = vec![json!({
        "Name": redshift::CLUSTER_IDENTIFIER_DIMENSION,
        "Value": cluster,
    })];
    if let Some(node_id) = node_id {
        set.push(json!({
            "Name": redshift::NODE_ID_DIMENSION,
            "Value": node_id,
        }));
    }
    Value::Array(set)
}

/// The alarm name one row is created under: unique over the whole table, and readable in a
/// failure message.
fn alarm_name(cluster: &str, node_id: Option<&str>, comparison: &str, threshold: f64) -> String {
    let scope = node_id.unwrap_or("cluster-wide");
    format!("{cluster}-{scope}-{comparison}-{threshold}")
}

/// Poll `DescribeClusters` until the cluster is available, answering the
/// `ClusterNode$NodeRole` list it reports then.
///
/// The loop makes no mutating call, so only the transition the clock already owed can end it.
async fn poll_until_available(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let read = redshift_call(
            router,
            case,
            "DescribeClusters",
            &[("ClusterIdentifier", cluster)],
        )
        .await;
        let statuses = elements(&read, "ClusterStatus");
        if statuses.first().map(String::as_str) == Some(redshift::STATUS_AVAILABLE) {
            return elements(&read, "NodeRole");
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {cluster:?} never became available within {DEADLINE:?}; last \
             answer: {read}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `StateValue` CloudWatch reports for this alarm, with the whole described alarm beside
/// it for an assertion to quote.
async fn alarm_state(router: &MultiServiceRouter, case: &str, name: &str) -> (String, Value) {
    let described = cloudwatch(
        router,
        case,
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    let state = described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (state, described)
}

/// Poll `DescribeAlarms` until this alarm reports `row.expected`.
async fn poll_until_state(router: &MultiServiceRouter, row: &AlarmRow) {
    let case = &row.case;
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (state, described) = alarm_state(router, case, &row.name).await;
        if state == row.expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the alarm {} to converge to {}, got {state:?} within \
             {DEADLINE:?} from {described}",
            row.name,
            row.expected
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// The history items of one type CloudWatch holds for this alarm.
async fn history(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    item_type: &str,
) -> Vec<Value> {
    let answered = cloudwatch(
        router,
        case,
        "DescribeAlarmHistory",
        &json!({"AlarmName": name, "HistoryItemType": item_type}),
    )
    .await;
    answered["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Poll the published series at this dimension set until it holds a datapoint, and assert the
/// value every threshold of [`COMPARISONS`] is placed around — so the table's PREMISE is
/// checked against what the series really carries rather than assumed.
///
/// Read where a client reads it: `GetMetricStatistics` over the same metric identity the alarms
/// below watch, at the same one-minute period.
async fn assert_published_value(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    node_id: Option<&str>,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let end = ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64());
        let read = cloudwatch(
            router,
            case,
            "GetMetricStatistics",
            &json!({
                "Namespace": redshift::METRIC_NAMESPACE,
                "MetricName": redshift::HEALTH_STATUS_METRIC,
                "Dimensions": dimensions(cluster, node_id),
                "StartTime": end - READ_WINDOW_SECONDS,
                "EndTime": end,
                "Period": PERIOD_SECONDS,
                "Statistics": ["Average", "Maximum", "Minimum"],
            }),
        )
        .await;
        let points = read["Datapoints"].as_array().cloned().unwrap_or_default();
        if let Some(point) = points.first() {
            for statistic in ["Average", "Maximum", "Minimum"] {
                let value = point[statistic].as_f64().unwrap_or_default();
                assert!(
                    (value - redshift::HEALTHY).abs() < TOLERANCE,
                    "case {case:?}: the thresholds of this suite are placed around {}, so the \
                     {node_id:?} series has to carry it; {statistic} read {value} in {point}",
                    redshift::HEALTHY
                );
            }
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the {node_id:?} series of {cluster:?} published no datapoint within \
             {DEADLINE:?}; last answer: {read}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Every alarm the cluster rows below are watched through.
fn alarm_rows(clusters: &[ClusterRow]) -> Vec<AlarmRow> {
    let mut rows: Vec<AlarmRow> = Vec::new();
    for cluster in clusters {
        let case = cluster.case;
        // The whole comparison table over the cluster-wide series and over the first node's,
        // and both directions over every further node: each published dimension set is checked
        // in the two states it can be in, and each operator in both of its outcomes.
        let mut watched: Vec<WatchedSet> =
            vec![("the cluster-wide series".to_string(), None, COMPARISONS)];
        for (position, node_id) in cluster.node_ids.iter().enumerate() {
            let comparisons: &'static [Comparison] = if position == 0 {
                COMPARISONS
            } else {
                BOTH_DIRECTIONS
            };
            watched.push((
                format!("the {node_id:?} node series"),
                Some((*node_id).to_string()),
                comparisons,
            ));
        }
        // And the same two directions over a dimension set no node of THIS cluster has: a
        // series nobody published, whatever a datapoint would have been compared to.
        for absent in &cluster.absent_node_ids {
            watched.push((
                format!("the {absent:?} series this cluster has no node for"),
                Some((*absent).to_string()),
                BOTH_DIRECTIONS,
            ));
        }
        let absent: BTreeSet<&str> = cluster.absent_node_ids.iter().copied().collect();
        for (scope, node_id, comparisons) in watched {
            let published = node_id
                .as_deref()
                .is_none_or(|node_id| !absent.contains(node_id));
            for (comparison, threshold, implied) in comparisons {
                rows.push(AlarmRow {
                    case: format!("{case}: {scope} under {comparison} {threshold}"),
                    name: alarm_name(
                        cluster.identifier,
                        node_id.as_deref(),
                        comparison,
                        *threshold,
                    ),
                    cluster: cluster.identifier,
                    node_id: node_id.clone(),
                    comparison,
                    threshold: *threshold,
                    expected: if published {
                        implied
                    } else {
                        STATE_INSUFFICIENT_DATA
                    },
                    published,
                });
            }
        }
    }
    rows
}

#[tokio::test]
async fn alarms_over_a_cluster_health_series_evaluate_at_every_dimension_set() {
    let router = build_router();

    let clusters: Vec<ClusterRow> = vec![
        ClusterRow {
            case: "a single-node cluster, whose one node is the shared one",
            identifier: "alarm-health-single",
            params: vec![
                ("ClusterType", "single-node"),
                ("Encrypted", "true"),
                ("PubliclyAccessible", "false"),
            ],
            node_ids: vec!["Shared"],
            absent_node_ids: vec!["Leader", "Compute-0"],
        },
        ClusterRow {
            case: "a two-node cluster, with a leader node and one series per compute node",
            identifier: "alarm-health-multi",
            params: vec![
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "2"),
                ("PubliclyAccessible", "false"),
            ],
            node_ids: vec!["Leader", "Compute-0", "Compute-1"],
            absent_node_ids: vec!["Shared", "Compute-2"],
        },
    ];

    for cluster in &clusters {
        let case = cluster.case;
        let mut params: Vec<(&str, &str)> = vec![
            ("ClusterIdentifier", cluster.identifier),
            ("NodeType", NODE_TYPE),
            ("MasterUsername", "admin"),
        ];
        params.extend(cluster.params.iter().copied());
        redshift_call(&router, case, "CreateCluster", &params).await;

        // The topology the CONTROL PLANE reports is what decides which node series exist, so
        // the dimension sets below are checked against it rather than assumed.
        let roles = poll_until_available(&router, case, cluster.identifier).await;
        let reported: BTreeSet<String> = roles.iter().map(|role| redshift::node_id(role)).collect();
        let expected: BTreeSet<String> =
            cluster.node_ids.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the cluster to report the nodes {expected:?}, got \
             {reported:?} from the roles {roles:?}"
        );

        // The PREMISE the comparison table rests on, read where a client reads it: every
        // threshold below is placed around the value the series really carries, cluster-wide
        // and at each node of this cluster's own topology.
        assert_published_value(&router, case, cluster.identifier, None).await;
        for node_id in &cluster.node_ids {
            assert_published_value(&router, case, cluster.identifier, Some(node_id)).await;
        }
    }

    let alarms = alarm_rows(&clusters);

    // Every alarm is created before any is read, so the evaluations the clock owes them come
    // due alongside one another rather than one poll loop at a time.
    for row in &alarms {
        let created = cloudwatch(
            &router,
            &row.case,
            "PutMetricAlarm",
            &json!({
                "AlarmName": row.name,
                "Namespace": redshift::METRIC_NAMESPACE,
                "MetricName": redshift::HEALTH_STATUS_METRIC,
                "Dimensions": dimensions(row.cluster, row.node_id.as_deref()),
                "Statistic": "Average",
                "Period": PERIOD_SECONDS,
                "EvaluationPeriods": EVALUATION_PERIODS,
                "DatapointsToAlarm": DATAPOINTS_TO_ALARM,
                "Threshold": row.threshold,
                "ComparisonOperator": row.comparison,
                "ActionsEnabled": false,
            }),
        )
        .await;
        assert!(
            created.is_object(),
            "case {:?}: expected PutMetricAlarm to answer an empty object, got {created}",
            row.case
        );
    }

    // The alarms over a series that IS published converge to the state their own comparison
    // implies over the healthy flag the cluster publishes.
    for row in alarms.iter().filter(|row| row.published) {
        poll_until_state(&router, row).await;
    }

    // Only now are the alarms over an absent series read: the loops above have already proved
    // that evaluations ran against this very namespace and metric, so an alarm still parked at
    // its creation state is parked because its dimension set selects no datapoint — not
    // because nothing evaluates.
    for row in alarms.iter().filter(|row| !row.published) {
        let (state, described) = alarm_state(&router, &row.case, &row.name).await;
        assert_eq!(
            state, STATE_INSUFFICIENT_DATA,
            "case {:?}: expected an alarm over a series nobody published to stay \
             {STATE_INSUFFICIENT_DATA}, got {state:?} from {described}",
            row.case
        );
        let updates = history(&router, &row.case, &row.name, "StateUpdate").await;
        assert!(
            updates.is_empty(),
            "case {:?}: expected no StateUpdate history for an alarm that never left its \
             creation state, got {updates:?}",
            row.case
        );
        // The empty list above is the ABSENCE of a transition, not an alarm whose history is
        // unreadable: the write that created it is in the history under its own item type.
        let configuration = history(&router, &row.case, &row.name, "ConfigurationUpdate").await;
        assert!(
            !configuration.is_empty(),
            "case {:?}: expected the alarm's creation to be in its ConfigurationUpdate \
             history, got {configuration:?}",
            row.case
        );
    }

    // An alarm that DID evaluate recorded the transition, so the two histories are told apart
    // by what happened rather than by the reader.
    for row in alarms
        .iter()
        .filter(|row| row.published && row.expected != STATE_INSUFFICIENT_DATA)
    {
        let updates = history(&router, &row.case, &row.name, "StateUpdate").await;
        assert!(
            !updates.is_empty(),
            "case {:?}: expected the evaluation that moved this alarm to {} to be in its \
             StateUpdate history, got {updates:?}",
            row.case,
            row.expected
        );
    }
}
