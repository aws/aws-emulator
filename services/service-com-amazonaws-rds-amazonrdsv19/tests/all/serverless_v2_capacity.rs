// SPDX-License-Identifier: CC0-1.0

//! The CAPACITY an Aurora Serverless v2 DB cluster runs on, and the auto-pause an idle one is
//! owed.
//!
//! THE RULE: a DB cluster created with a `ServerlessV2ScalingConfiguration` publishes the
//! `AWS/RDS` `ServerlessDatabaseCapacity` series — dimensioned by its `DBClusterIdentifier`, and
//! by the `DBInstanceIdentifier` of each `db.serverless` member — once a minute, and what each
//! datapoint reads follows the configuration and the sessions the cluster is serving:
//!
//! * a RUNNING cluster holds its configured `MinCapacity`, and never less than the half an ACU
//!   that is the smallest capacity a running Serverless v2 instance has ("For versions that don't
//!   support Aurora Serverless v2 auto-pause, the smallest value that you can use is 0.5",
//!   `ServerlessV2ScalingConfiguration$MinCapacity`);
//! * a cluster whose `MinCapacity` is 0 and that has been IDLE for its `SecondsUntilAutoPause`
//!   ("the number of seconds an Aurora Serverless v2 DB instance must be idle before Aurora
//!   attempts to automatically pause it … This property is only shown when the minimum capacity
//!   for the cluster is set to 0 ACUs … The default is 300 seconds") is PAUSED and reports 0
//!   ACUs;
//! * a cluster with a session open is not idle, so it cannot pause however long that session is
//!   held; the next session on a paused cluster resumes it, with every row committed before the
//!   pause still there.
//!
//! The rows below span what the rule generalises over: the configured minimum (0, the half-ACU
//! floor and a whole number of ACUs), whether the configuration asks for auto-pause at all, the
//! length of the idle period (the documented default and a longer one the request names), and a
//! cluster with no serverless configuration at all — which publishes no capacity series,
//! because it runs on an instance class rather than on ACUs.
//!
//! Time is driven by the EMULATOR's clock (`service_common::clock`), which is what the capacity
//! publisher reads: a test crosses the idle period by advancing that clock rather than by
//! waiting for it, and then waits for the CONDITION — a fresh datapoint — with a generous
//! deadline.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_engine_endpoint::{wire_login, Session, Wire, DATABASE, MASTER_PASSWORD, MASTER_USER};
use crate::db_instances::{call, expect_ok, text, REGION};
use crate::lifecycle_poll_loops::poll_until;
use service_com_amazonaws_rds_amazonrdsv19::state::serverless_capacity::{
    DB_CLUSTER_IDENTIFIER_DIMENSION, DB_INSTANCE_IDENTIFIER_DIMENSION, METRIC_NAMESPACE,
    MIN_RUNNING_CAPACITY, PAUSED_CAPACITY, PUBLISH_INTERVAL,
    SERVERLESS_DATABASE_CAPACITY_METRIC,
};

/// The emulator these tests drive: Amazon RDS and CloudWatch on ONE shared engine, exactly as
/// the assembled router mounts them — the capacity series is published into the metric store
/// CloudWatch itself serves `GetMetricStatistics` from, so without that service mounted there is
/// nothing to read it back through.
///
/// `pub(crate)` so a sibling suite whose subject is another series of this namespace reads it
/// back through the same two services rather than standing up a router of its own.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// The `x-amz-target` prefix a CloudWatch client sends: that service's own wire protocol is
/// `awsJson1_0`, so its calls cannot go through this crate's `awsQuery` helper — which carries
/// the RDS API version and the `rds` credential scope.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// One CloudWatch call over the wire, answering the HTTP status and the parsed body.
///
/// `pub(crate)` beside the router above, so a sibling suite reading another series of this
/// namespace frames its calls the one way rather than writing the framing again.
pub(crate) async fn cloudwatch_call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"))
        // The SigV4 credential scope is what routes the request to the `monitoring` signing
        // name, which is the service that owns the metric store.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = match payload.is_empty() {
        true => Value::Null,
        false => serde_json::from_slice(&payload).unwrap_or(Value::Null),
    };
    (status, parsed)
}

/// How wide a window a read of the capacity series asks for, in seconds.
///
/// Generous at both ends: the series opens with datapoints stamped before the cluster became
/// available, and an advance of the emulator's clock stamps the datapoints it makes due ahead of
/// the wall clock, so a window of three hours either way holds every datapoint these tests
/// publish — and stays inside the 1,440 periods one read may answer ("You have requested up to
/// … datapoints, which exceeds the limit of 1440").
const READ_WINDOW_SECS: f64 = 10_800.0;

/// The datapoints of one capacity series as `(Timestamp, Average)`, oldest first.
async fn capacity_datapoints(
    router: &MultiServiceRouter,
    dimension: &str,
    value: &str,
) -> Vec<(f64, f64)> {
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default();
    let (status, body) = cloudwatch_call(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": SERVERLESS_DATABASE_CAPACITY_METRIC,
            "Dimensions": [{"Name": dimension, "Value": value}],
            "StartTime": now - READ_WINDOW_SECS,
            "EndTime": now + READ_WINDOW_SECS,
            "Period": 60,
            "Statistics": ["Average", "Maximum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMetricStatistics for {dimension}={value}: expected HTTP 200, got {status} with \
         body {body}"
    );
    let mut points: Vec<(f64, f64)> = body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            (
                point["Timestamp"].as_f64().unwrap_or_default(),
                point["Average"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    points.sort_by(|left, right| left.0.total_cmp(&right.0));
    points
}

/// Wait until the series of `dimension`=`value` has a datapoint stamped after `after`, and
/// answer that datapoint's average.
///
/// The CONDITION is waited for, never a span: the publisher's clock wakes up on its own
/// schedule, so the test looks again until the datapoint it is waiting for is there or a
/// generous deadline passes. This crate's own poll loop answers HTTP responses, which a
/// CloudWatch read of a series is not, so this is the same shape over the datapoints.
async fn next_capacity_after(
    router: &MultiServiceRouter,
    case: &str,
    dimension: &str,
    value: &str,
    after: f64,
) -> f64 {
    let deadline = ::std::time::Instant::now() + CAPACITY_POLL_DEADLINE;
    let mut last: Vec<(f64, f64)> = Vec::new();
    while ::std::time::Instant::now() < deadline {
        last = capacity_datapoints(router, dimension, value).await;
        if let Some((_, average)) = last.iter().rfind(|(stamp, _)| *stamp > after) {
            return *average;
        }
        ::tokio::time::sleep(CAPACITY_POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: no capacity datapoint of {dimension}={value} stamped after {after} \
         arrived within {CAPACITY_POLL_DEADLINE:?}; the series holds {last:?}"
    )
}

/// How long a wait for the next capacity datapoint may take.
///
/// Generous beside the publisher's own wake-up span, which is what decides how soon a datapoint
/// the clock has made due is written: the test waits for the datapoint, not for a span.
const CAPACITY_POLL_DEADLINE: Duration = Duration::from_secs(120);

/// The gap between two reads of the same series while waiting.
const CAPACITY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The newest stamp the series of `dimension`=`value` carries, or `None` for a series nothing has
/// published into.
async fn newest_stamp(
    router: &MultiServiceRouter,
    dimension: &str,
    value: &str,
) -> Option<f64> {
    capacity_datapoints(router, dimension, value)
        .await
        .last()
        .map(|(stamp, _)| *stamp)
}

/// One cluster of the table: how it is configured, and the capacity it runs on.
struct CapacityRow {
    case: &'static str,
    identifier: &'static str,
    /// The `ServerlessV2ScalingConfiguration` members the create carries, or none at all.
    scaling: &'static [(&'static str, &'static str)],
    /// The capacity the cluster reports while it is running.
    running: f64,
    /// The idle period after which it pauses, for a cluster that can pause at all.
    pauses_after: Option<u64>,
}

/// How long the documented default idle period is, in seconds: "The default is 300 seconds"
/// (`ServerlessV2ScalingConfigurationInfo$SecondsUntilAutoPause`).
const DEFAULT_SECONDS_UNTIL_AUTO_PAUSE: u64 = 300;

/// A longer idle period a request may name, inside the documented range ("This value ranges
/// between 300 seconds (five minutes) and 86,400 seconds (one day)").
const LONGER_SECONDS_UNTIL_AUTO_PAUSE: u64 = 900;

const CAPACITY_ROWS: &[CapacityRow] = &[
    CapacityRow {
        case: "scale-to-zero capacity with the default idle period",
        identifier: "emu-sv2-zero-default",
        scaling: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "1"),
        ],
        running: MIN_RUNNING_CAPACITY,
        pauses_after: Some(DEFAULT_SECONDS_UNTIL_AUTO_PAUSE),
    },
    CapacityRow {
        case: "scale-to-zero capacity with a longer idle period",
        identifier: "emu-sv2-zero-longer",
        scaling: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "2"),
            (
                "ServerlessV2ScalingConfiguration.SecondsUntilAutoPause",
                "900",
            ),
        ],
        running: MIN_RUNNING_CAPACITY,
        pauses_after: Some(LONGER_SECONDS_UNTIL_AUTO_PAUSE),
    },
    CapacityRow {
        case: "the half-ACU floor, which asks for no auto-pause",
        identifier: "emu-sv2-half",
        scaling: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "4"),
        ],
        running: MIN_RUNNING_CAPACITY,
        pauses_after: None,
    },
    CapacityRow {
        case: "a whole number of ACUs as the minimum",
        identifier: "emu-sv2-two-acus",
        scaling: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "2"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "8"),
        ],
        running: 2.0,
        pauses_after: None,
    },
];

/// The cluster with NO serverless configuration, which publishes no capacity series at all.
const PROVISIONED_CLUSTER: &str = "emu-sv2-provisioned";

/// Create the row's cluster and wait until it reports `available`, which is the state it
/// publishes a capacity series in.
async fn available_cluster(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    scaling: &[(&str, &str)],
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", "aurora-postgresql".to_string()),
        ("DatabaseName", DATABASE.to_string()),
        ("MasterUsername", MASTER_USER.to_string()),
        ("MasterUserPassword", MASTER_PASSWORD.to_string()),
    ];
    params.extend(
        scaling
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    expect_ok(case, call(router, "CreateDBCluster", &params).await);
    let read = [("DBClusterIdentifier", identifier)];
    let (body, _polls) = poll_until(
        case,
        "the cluster",
        || call(router, "DescribeDBClusters", &read),
        |status, body| status == 200 && text(body, "Status") == "available",
    )
    .await;
    body
}

/// Advance the EMULATOR's clock past `seconds`, which is how the idle period a pause is owed
/// after is crossed without waiting for it.
fn cross_idle_period(engine: &Arc<engine::InMemoryEngine>, seconds: u64) {
    ::service_common::clock::advance(&**engine, Duration::from_secs(seconds + 1));
}

/// Move the EMULATOR's clock on by ONE publish interval, which is what makes the next datapoint
/// of a running series due — and is well inside every idle period of this suite, so it pauses
/// nothing by itself.
fn cross_publish_interval(engine: &Arc<engine::InMemoryEngine>) {
    ::service_common::clock::advance(&**engine, PUBLISH_INTERVAL);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_serverless_v2_cluster_publishes_the_capacity_it_runs_on() {
    let (router, _engine) = emulator();
    for row in CAPACITY_ROWS {
        available_cluster(&router, row.case, row.identifier, row.scaling).await;
        let capacity =
            next_capacity_after(&router, row.case, DB_CLUSTER_IDENTIFIER_DIMENSION, row.identifier, 0.0)
                .await;
        assert!(
            (capacity - row.running).abs() < f64::EPSILON,
            "case {:?}: expected the cluster to report {} ACUs while it runs, got {capacity}",
            row.case,
            row.running
        );
    }

    // A cluster with no serverless configuration runs on an instance class, not on ACUs, so it
    // publishes no capacity series: the read answers no datapoint at all.
    available_cluster(
        &router,
        "a provisioned Aurora cluster",
        PROVISIONED_CLUSTER,
        &[],
    )
    .await;
    let provisioned = capacity_datapoints(
        &router,
        DB_CLUSTER_IDENTIFIER_DIMENSION,
        PROVISIONED_CLUSTER,
    )
    .await;
    assert!(
        provisioned.is_empty(),
        "case a provisioned Aurora cluster: expected no capacity series, got {provisioned:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_serverless_member_publishes_the_capacity_of_the_cluster_it_runs_in() {
    let (router, _engine) = emulator();
    let case = "a db.serverless writer of a scale-to-zero cluster";
    let cluster = "emu-sv2-member-cluster";
    let member = "emu-sv2-member-writer";
    available_cluster(&router, case, cluster, CAPACITY_ROWS[0].scaling).await;
    expect_ok(
        case,
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", member),
                ("DBClusterIdentifier", cluster),
                ("Engine", "aurora-postgresql"),
                ("DBInstanceClass", "db.serverless"),
            ],
        )
        .await,
    );
    // The member's own series carries the same observation, under the dimension the capacity
    // member's documented subject — "a DB instance in an Aurora Serverless v2 cluster" — is
    // named by.
    let capacity =
        next_capacity_after(&router, case, DB_INSTANCE_IDENTIFIER_DIMENSION, member, 0.0).await;
    assert!(
        (capacity - MIN_RUNNING_CAPACITY).abs() < f64::EPSILON,
        "case {case:?}: expected the member to report {MIN_RUNNING_CAPACITY} ACUs while it \
         runs, got {capacity}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_scale_to_zero_cluster_pauses_and_only_after_its_own_idle_period() {
    let (router, engine) = emulator();
    for row in CAPACITY_ROWS {
        available_cluster(&router, row.case, row.identifier, row.scaling).await;
        next_capacity_after(
            &router,
            row.case,
            DB_CLUSTER_IDENTIFIER_DIMENSION,
            row.identifier,
            0.0,
        )
        .await;
    }
    // Past the DEFAULT idle period: the cluster that asked for it reports no capacity at all,
    // and every other cluster — one with a longer idle period, and the two that asked for no
    // auto-pause by naming a nonzero minimum — is still running on its own minimum.
    let before: Vec<Option<f64>> = {
        let mut stamps = Vec::new();
        for row in CAPACITY_ROWS {
            stamps.push(
                newest_stamp(&router, DB_CLUSTER_IDENTIFIER_DIMENSION, row.identifier).await,
            );
        }
        stamps
    };
    cross_idle_period(&engine, DEFAULT_SECONDS_UNTIL_AUTO_PAUSE);
    for (row, stamp) in CAPACITY_ROWS.iter().zip(&before) {
        let expected = match row.pauses_after {
            Some(idle) if idle <= DEFAULT_SECONDS_UNTIL_AUTO_PAUSE => PAUSED_CAPACITY,
            _ => row.running,
        };
        let capacity = next_capacity_after(
            &router,
            row.case,
            DB_CLUSTER_IDENTIFIER_DIMENSION,
            row.identifier,
            stamp.unwrap_or_default(),
        )
        .await;
        assert!(
            (capacity - expected).abs() < f64::EPSILON,
            "case {:?}: {} after {DEFAULT_SECONDS_UNTIL_AUTO_PAUSE}s idle, expected \
             {expected} ACUs, got {capacity}",
            row.case,
            match row.pauses_after {
                Some(idle) => format!("an idle period of {idle}s"),
                None => "no auto-pause".to_string(),
            }
        );
    }
    // And past the LONGER one the second row named, that cluster pauses too while the two with a
    // nonzero minimum never do.
    let before: Vec<Option<f64>> = {
        let mut stamps = Vec::new();
        for row in CAPACITY_ROWS {
            stamps.push(
                newest_stamp(&router, DB_CLUSTER_IDENTIFIER_DIMENSION, row.identifier).await,
            );
        }
        stamps
    };
    cross_idle_period(&engine, LONGER_SECONDS_UNTIL_AUTO_PAUSE);
    for (row, stamp) in CAPACITY_ROWS.iter().zip(&before) {
        let expected = match row.pauses_after {
            Some(_) => PAUSED_CAPACITY,
            None => row.running,
        };
        let capacity = next_capacity_after(
            &router,
            row.case,
            DB_CLUSTER_IDENTIFIER_DIMENSION,
            row.identifier,
            stamp.unwrap_or_default(),
        )
        .await;
        assert!(
            (capacity - expected).abs() < f64::EPSILON,
            "case {:?}: past {LONGER_SECONDS_UNTIL_AUTO_PAUSE}s idle, expected {expected} \
             ACUs, got {capacity}",
            row.case
        );
    }
}

/// The `Endpoint` and `Port` a client opens a session on the cluster at.
fn dialable(case: &str, cluster: &str) -> (String, u16) {
    let endpoint = text(cluster, "Endpoint");
    let port: u16 = text(cluster, "Port")
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected the port the engine listens on"));
    (endpoint, port)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_session_keeps_a_cluster_awake_and_a_resumed_one_still_holds_its_rows() {
    let (router, engine) = emulator();
    let case = "a scale-to-zero cluster with one session held open";
    let identifier = "emu-sv2-held-session";
    let cluster = available_cluster(&router, case, identifier, CAPACITY_ROWS[0].scaling).await;
    let (endpoint, port) = dialable(case, &cluster);

    // A session, and a row committed through it.
    let mut session: Session = wire_login(case, Wire::Postgres, &endpoint, port, DATABASE).await;
    session
        .run(case, "CREATE TABLE ledger (id integer PRIMARY KEY, note varchar(64))")
        .await;
    session
        .run(
            case,
            "INSERT INTO ledger (id, note) VALUES (1, 'committed before the pause')",
        )
        .await;

    // While that session is held, the cluster is not idle: the datapoints keep arriving and
    // none of them is zero, however far past the idle period the clock is moved.
    let mut stamp = newest_stamp(&router, DB_CLUSTER_IDENTIFIER_DIMENSION, identifier)
        .await
        .unwrap_or_default();
    for crossing in 0..2 {
        cross_idle_period(&engine, DEFAULT_SECONDS_UNTIL_AUTO_PAUSE);
        let capacity = next_capacity_after(
            &router,
            case,
            DB_CLUSTER_IDENTIFIER_DIMENSION,
            identifier,
            stamp,
        )
        .await;
        assert!(
            (capacity - MIN_RUNNING_CAPACITY).abs() < f64::EPSILON,
            "case {case:?}: crossing {crossing} of the idle period with a session held open \
             must keep the cluster running on {MIN_RUNNING_CAPACITY} ACUs, got {capacity}"
        );
        stamp = newest_stamp(&router, DB_CLUSTER_IDENTIFIER_DIMENSION, identifier)
            .await
            .unwrap_or(stamp);
    }

    // The session closes. The cluster is idle from then on, so one publish interval later it is
    // still running — the idle period has not passed yet — and only past that period does it
    // pause.
    drop(session);
    cross_publish_interval(&engine);
    let closing = next_capacity_after(
        &router,
        case,
        DB_CLUSTER_IDENTIFIER_DIMENSION,
        identifier,
        stamp,
    )
    .await;
    assert!(
        (closing - MIN_RUNNING_CAPACITY).abs() < f64::EPSILON,
        "case {case:?}: a cluster whose session has just closed is still running on \
         {MIN_RUNNING_CAPACITY} ACUs, got {closing}"
    );
    let stamp = newest_stamp(&router, DB_CLUSTER_IDENTIFIER_DIMENSION, identifier)
        .await
        .unwrap_or(stamp);
    cross_idle_period(&engine, DEFAULT_SECONDS_UNTIL_AUTO_PAUSE);
    let paused = next_capacity_after(
        &router,
        case,
        DB_CLUSTER_IDENTIFIER_DIMENSION,
        identifier,
        stamp,
    )
    .await;
    assert!(
        (paused - PAUSED_CAPACITY).abs() < f64::EPSILON,
        "case {case:?}: an idle cluster past its idle period must report {PAUSED_CAPACITY} \
         ACUs, got {paused}"
    );

    // And the next session resumes it with its rows intact: the row committed before the pause
    // is readable, the cluster takes a further write, and its capacity is back off zero.
    let stamp = newest_stamp(&router, DB_CLUSTER_IDENTIFIER_DIMENSION, identifier)
        .await
        .unwrap_or_default();
    let mut resumed: Session = wire_login(case, Wire::Postgres, &endpoint, port, DATABASE).await;
    let read = resumed
        .run(case, "SELECT note FROM ledger WHERE id = 1")
        .await;
    assert_eq!(
        read.only(),
        Some("committed before the pause"),
        "case {case:?}: a resumed cluster must still hold the rows committed before it \
         paused, got {read:?}"
    );
    resumed
        .run(
            case,
            "INSERT INTO ledger (id, note) VALUES (2, 'written after the resume')",
        )
        .await;
    let rows = resumed.value(case, "SELECT count(*) FROM ledger").await;
    assert_eq!(
        rows, "2",
        "case {case:?}: a resumed cluster must accept further writes, got {rows} rows"
    );
    cross_publish_interval(&engine);
    let running = next_capacity_after(
        &router,
        case,
        DB_CLUSTER_IDENTIFIER_DIMENSION,
        identifier,
        stamp,
    )
    .await;
    assert!(
        (running - MIN_RUNNING_CAPACITY).abs() < f64::EPSILON,
        "case {case:?}: a resumed cluster must report {MIN_RUNNING_CAPACITY} ACUs again, got \
         {running}"
    );
}
