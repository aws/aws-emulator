// SPDX-License-Identifier: CC0-1.0

//! The CONNECTIONS a DB instance is serving, as the series a client watches load on.
//!
//! THE RULE: an available DB instance publishes the `AWS/RDS` `DatabaseConnections` series —
//! dimensioned by its own `DBInstanceIdentifier`, in the `Count` unit, a datapoint every
//! publish interval — and what each datapoint reads is the number of client sessions the
//! instance is serving at that minute: it rises with each session opened against the instance
//! and falls again as they close. An instance that is not available publishes no datapoint at
//! all, and one brought back into service publishes again.
//!
//! The rows below span what the rule generalises over: the engine the instance runs (the two
//! wires this emulator serves a DB instance endpoint on, and an engine family it serves none
//! for, which holds no session and so reads zero), and how many sessions are open at once
//! (none, one, and several concurrent ones).
//!
//! Time is driven by the EMULATOR's clock (`service_common::clock`), which is what the metric
//! publisher reads: a test brings the next datapoint of a caught-up series due by advancing
//! that clock rather than by waiting a minute for it, and then waits for the CONDITION — the
//! datapoint itself — with a generous deadline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_rds_amazonrdsv19::state::instance_metrics::{
    DATABASE_CONNECTIONS_METRIC, DATABASE_CONNECTIONS_UNIT, DB_INSTANCE_IDENTIFIER_DIMENSION,
    METRIC_NAMESPACE, PUBLISH_INTERVAL,
};

use crate::db_instance_engine_endpoint::{available_instance, wire_login, Session, Wire, DATABASE};
use crate::db_instances::{call, expect_ok, text};
use crate::serverless_v2_capacity::{cloudwatch_call, emulator};

/// How wide a window a read of the connections series asks for, in seconds.
///
/// Generous at both ends: the series opens with datapoints stamped before the instance became
/// available, and an advance of the emulator's clock stamps the datapoints it makes due ahead of
/// the wall clock, so a window of three hours either way holds every datapoint these tests
/// publish — and stays inside the 1,440 periods one read may answer.
const READ_WINDOW_SECS: f64 = 10_800.0;

/// How long a wait for the next connections datapoint may take.
///
/// Generous beside the publisher's own wake-up span, which is what decides how soon a datapoint
/// the clock has made due is written: the test waits for the datapoint, not for a span.
const DATAPOINT_DEADLINE: Duration = Duration::from_secs(30);

/// The gap between two reads of the same series while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// One datapoint of a connections series, as a client reads it.
#[derive(Debug, Clone)]
struct Datapoint {
    /// The instant the datapoint is stamped at, in epoch seconds.
    stamp: f64,
    /// The largest observation of the period, which for a one-observation minute is the
    /// connection count itself.
    maximum: f64,
    /// How many observations the period holds.
    samples: f64,
    /// The unit the datapoint is reported in.
    unit: String,
}

/// Every datapoint of the connections series of `identifier`, oldest first.
async fn connection_datapoints(router: &MultiServiceRouter, identifier: &str) -> Vec<Datapoint> {
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default();
    let (status, body) = cloudwatch_call(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": DATABASE_CONNECTIONS_METRIC,
            "Dimensions": [{"Name": DB_INSTANCE_IDENTIFIER_DIMENSION, "Value": identifier}],
            "StartTime": now - READ_WINDOW_SECS,
            "EndTime": now + READ_WINDOW_SECS,
            "Period": 60,
            "Statistics": ["Maximum", "Average", "SampleCount"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMetricStatistics for {identifier}: expected HTTP 200, got {status} with body {body}"
    );
    let mut points: Vec<Datapoint> = body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| Datapoint {
            stamp: point["Timestamp"].as_f64().unwrap_or_default(),
            maximum: point["Maximum"].as_f64().unwrap_or_default(),
            samples: point["SampleCount"].as_f64().unwrap_or_default(),
            unit: point["Unit"].as_str().unwrap_or_default().to_string(),
        })
        .collect();
    points.sort_by(|left, right| left.stamp.total_cmp(&right.stamp));
    points
}

/// The newest stamp the connections series of `identifier` carries, or `None` for a series
/// nothing has published into.
async fn newest_stamp(router: &MultiServiceRouter, identifier: &str) -> Option<f64> {
    connection_datapoints(router, identifier)
        .await
        .last()
        .map(|point| point.stamp)
}

/// How many observations the whole connections series of `identifier` holds.
///
/// One datapoint is one observation, so this counts the datapoints the publisher has appended
/// however they are stamped — which is what tells "it published again" from "it published
/// nothing" when the stamps of two runs of one series need not be ordered against each other.
async fn published_observations(router: &MultiServiceRouter, identifier: &str) -> f64 {
    connection_datapoints(router, identifier)
        .await
        .iter()
        .map(|point| point.samples)
        .sum()
}

/// Wait until the connections series of `identifier` holds a datapoint at all, and answer the
/// newest stamp it carries.
///
/// The first datapoint of a series is owed the moment the instance becomes available, so the
/// CONDITION is waited for — the publisher's clock wakes up on its own schedule, and the test
/// looks again until the series it is waiting for is there or a generous deadline passes.
async fn series_opens(router: &MultiServiceRouter, case: &str, identifier: &str) -> f64 {
    let deadline = Instant::now() + DATAPOINT_DEADLINE;
    loop {
        if let Some(stamp) = newest_stamp(router, identifier).await {
            return stamp;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: an available instance must publish a DatabaseConnections series, got \
             no datapoint within {DATAPOINT_DEADLINE:?}"
        );
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Move the EMULATOR's clock on by ONE publish interval, which is what makes the next datapoint
/// of a caught-up series due.
fn cross_publish_interval(engine: &Arc<engine::InMemoryEngine>) {
    ::service_common::clock::advance(&**engine, PUBLISH_INTERVAL);
}

/// Wait until the connections series of `identifier` carries a datapoint stamped after `after`,
/// and answer the newest such datapoint.
///
/// The CONDITION is waited for, never a span: the clock is moved on by one publish interval —
/// which is what brings the next datapoint of a caught-up series due — and then the series is read
/// again until that datapoint is there or a generous deadline passes.
async fn next_datapoint_after(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    identifier: &str,
    after: f64,
) -> Datapoint {
    let deadline = Instant::now() + DATAPOINT_DEADLINE;
    cross_publish_interval(engine);
    loop {
        let arrived = connection_datapoints(router, identifier)
            .await
            .into_iter()
            .rfind(|point| point.stamp > after);
        if let Some(point) = arrived {
            return point;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {identifier} to publish a datapoint after {after} within \
             {DATAPOINT_DEADLINE:?}"
        );
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until the connections series of `identifier` carries a datapoint stamped after `after`
/// that reads `expected`, and answer the stamp it carried.
///
/// The CONDITION is waited for, never a span: each pass moves the emulator's clock on by one
/// publish interval — which is what brings the next datapoint of a caught-up series due — and
/// then looks again until the datapoint it was waiting for is there or a generous deadline
/// passes. A session that has just closed may still be being torn down when the first of those
/// datapoints is stamped, which is why the later minutes are read too rather than the first one
/// being taken as the answer.
async fn connections_settle_to(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    identifier: &str,
    after: f64,
    expected: f64,
) -> f64 {
    let deadline = Instant::now() + DATAPOINT_DEADLINE;
    let mut newest = after;
    let mut seen: Vec<Datapoint> = Vec::new();
    cross_publish_interval(engine);
    while Instant::now() < deadline {
        let arrived: Vec<Datapoint> = connection_datapoints(router, identifier)
            .await
            .into_iter()
            .filter(|point| point.stamp > newest)
            .collect();
        if let Some(point) = arrived.last().cloned() {
            newest = point.stamp;
            seen.extend(arrived);
            assert_eq!(
                point.unit, DATABASE_CONNECTIONS_UNIT,
                "case {case:?}: a connection count is reported in {DATABASE_CONNECTIONS_UNIT}, \
                 got {point:?}"
            );
            if (point.maximum - expected).abs() < f64::EPSILON {
                return point.stamp;
            }
            // That minute is published and settled; the next one may carry the count.
            cross_publish_interval(engine);
        }
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!(
        "case {case:?}: expected a DatabaseConnections datapoint reading {expected} after \
         {after}, got {seen:?}"
    )
}

/// One instance of the table: the engine it runs, the wire a client dials it on, and how many
/// concurrent sessions are opened against it.
struct ConnectionRow {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// The wire a session is opened on, or `None` for an engine family this emulator serves no
    /// DB instance endpoint for — such an instance holds no session at all.
    wire: Option<Wire>,
    /// How many sessions are held open at once.
    sessions: usize,
}

/// The instance class and storage an engine family with no served wire is created with: the same
/// ones its own suite uses, since those engines are not offered on the smallest burstable class.
const UNSERVED_CLASS_OVERRIDES: &[(&str, &str)] = &[
    ("DBInstanceClass", "db.m5.large"),
    ("AllocatedStorage", "100"),
];

const CONNECTION_ROWS: &[ConnectionRow] = &[
    ConnectionRow {
        case: "a PostgreSQL instance nobody is connected to",
        identifier: "emu-conn-pg-idle",
        engine: "postgres",
        wire: Some(Wire::Postgres),
        sessions: 0,
    },
    ConnectionRow {
        case: "a PostgreSQL instance under concurrent connection load",
        identifier: "emu-conn-pg-busy",
        engine: "postgres",
        wire: Some(Wire::Postgres),
        sessions: 3,
    },
    ConnectionRow {
        case: "a MySQL instance with one session",
        identifier: "emu-conn-mysql",
        engine: "mysql",
        wire: Some(Wire::MySql),
        sessions: 1,
    },
    ConnectionRow {
        case: "a MariaDB instance with two sessions",
        identifier: "emu-conn-mariadb",
        engine: "mariadb",
        wire: Some(Wire::MySql),
        sessions: 2,
    },
    ConnectionRow {
        case: "an engine family this emulator serves no instance wire for",
        identifier: "emu-conn-oracle",
        engine: "oracle-se2",
        wire: None,
        sessions: 0,
    },
];

/// Create the row's instance and answer the endpoint it publishes, waiting until it reports
/// `available` — the state it publishes a series in.
async fn available_row(router: &MultiServiceRouter, row: &ConnectionRow) -> (String, u16) {
    let extra: Vec<(&str, String)> = match row.wire {
        Some(_) => Vec::new(),
        None => UNSERVED_CLASS_OVERRIDES
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect(),
    };
    available_instance(router, row.identifier, row.engine, &extra).await
}

/// Hold `count` sessions open against `address:port` on `wire`.
async fn held_sessions(
    case: &str,
    wire: Option<Wire>,
    address: &str,
    port: u16,
    count: usize,
) -> Vec<Session> {
    let Some(wire) = wire else {
        assert_eq!(
            count, 0,
            "case {case:?}: an engine with no served wire cannot hold a session"
        );
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for _ in 0..count {
        sessions.push(wire_login(case, wire, address, port, DATABASE).await);
    }
    sessions
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_available_instance_publishes_the_connections_it_is_serving() {
    let (router, engine) = emulator();

    // Every row's instance, and the sessions each of them is holding: the series is read after
    // the connections are open, so what it reports is the load the emulator is really serving.
    let mut held: Vec<(&ConnectionRow, Vec<Session>, f64)> = Vec::new();
    for row in CONNECTION_ROWS {
        let (address, port) = available_row(&router, row).await;
        // Every instance publishes a series of its own from the moment it is available, whatever
        // it is serving: that is what an alarm created against it has to evaluate.
        let opened = series_opens(&router, row.case, row.identifier).await;
        let sessions = held_sessions(row.case, row.wire, &address, port, row.sessions).await;
        held.push((row, sessions, opened));
    }

    // With those sessions open, the next datapoint of each series reads the count that instance
    // is serving.
    for (row, sessions, opened) in &mut held {
        #[allow(clippy::cast_precision_loss)]
        let expected = row.sessions as f64;
        *opened = connections_settle_to(
            &router,
            &engine,
            row.case,
            row.identifier,
            *opened,
            expected,
        )
        .await;
        // And one of those sessions can still be used, so what was counted is a connection the
        // emulator is really serving rather than a number nothing is behind.
        if let Some(session) = sessions.first_mut() {
            let answer = session.value(row.case, "SELECT 1").await;
            assert_eq!(
                answer, "1",
                "case {:?}: a counted session must still serve a statement, got {answer:?}",
                row.case
            );
        }
    }

    // The sessions close. The count falls back to the connections the instance is still serving,
    // which is none.
    for (row, sessions, opened) in &mut held {
        sessions.clear();
        connections_settle_to(&router, &engine, row.case, row.identifier, *opened, 0.0).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_instance_publishes_only_while_it_is_available() {
    let (router, engine) = emulator();
    let case = "an instance stopped and started again";
    let identifier = "emu-conn-stopped";
    // A second instance that stays available throughout: its next datapoint is the CONDITION that
    // says a publisher pass has run since the clock was last moved, which is what makes "the
    // stopped instance published nothing" an observation rather than a wait.
    let witness = "emu-conn-witness";
    available_instance(&router, identifier, "postgres", &[]).await;
    available_instance(&router, witness, "postgres", &[]).await;
    connections_settle_to(&router, &engine, case, identifier, 0.0, 0.0).await;
    let mut watched = series_opens(&router, case, witness).await;

    // Stopped, the instance runs no database to hold a connection, so its series takes no
    // further datapoint however far the publisher's clock is moved on.
    expect_ok(
        case,
        call(
            &router,
            "StopDBInstance",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    let settled = Instant::now() + DATAPOINT_DEADLINE;
    let mut status = String::new();
    while Instant::now() < settled {
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await,
        );
        status = text(&described, "DBInstanceStatus");
        if status == "stopped" {
            break;
        }
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        status, "stopped",
        "case {case:?}: the instance must settle to stopped before its series is read again"
    );
    // The pass that publishes the witness's next datapoint is the pass that found this instance
    // stopped and closed its series, so what the crossings below measure is a stopped instance
    // rather than the minute it stopped in.
    watched = next_datapoint_after(&router, &engine, case, witness, watched)
        .await
        .stamp;
    let while_stopped = published_observations(&router, identifier).await;
    for crossing in 0..3 {
        // Each crossing is PROVED to have been published by the pass that answered the witness,
        // which publishes every series that is due in one go.
        watched = next_datapoint_after(&router, &engine, case, witness, watched)
            .await
            .stamp;
        let now = published_observations(&router, identifier).await;
        assert!(
            (now - while_stopped).abs() < f64::EPSILON,
            "case {case:?}: crossing {crossing} of the publish interval with the instance \
             stopped must publish nothing, expected {while_stopped} observations, got {now}"
        );
    }

    // Started again, it publishes again: the series resumes with the connections the instance is
    // serving, which is none.
    expect_ok(
        case,
        call(
            &router,
            "StartDBInstance",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    let deadline = Instant::now() + DATAPOINT_DEADLINE;
    let mut resumed = while_stopped;
    while Instant::now() < deadline {
        resumed = published_observations(&router, identifier).await;
        if resumed > while_stopped {
            break;
        }
        watched = next_datapoint_after(&router, &engine, case, witness, watched)
            .await
            .stamp;
    }
    assert!(
        resumed > while_stopped,
        "case {case:?}: an instance back in service must publish again, expected more than \
         {while_stopped} observations, got {resumed}"
    );
    let newest = connection_datapoints(&router, identifier)
        .await
        .last()
        .cloned();
    assert!(
        newest
            .as_ref()
            .is_some_and(|point| point.maximum.abs() < f64::EPSILON),
        "case {case:?}: the resumed series reads the connections the instance is serving, which \
         is none, got {newest:?}"
    );
}
