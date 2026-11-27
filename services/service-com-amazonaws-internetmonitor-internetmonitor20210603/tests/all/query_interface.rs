// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the query interface: `StartQuery`, `GetQueryStatus` and
//! `StopQuery`.
//!
//! The rules under test:
//!
//! * A start against a monitor this account holds mints a `QueryId` and records the query in
//!   the non-terminal `QUEUED` state. The clock then carries the query to `RUNNING` and
//!   `SUCCEEDED`, which is what `GetQueryStatus` reports and what `StopQuery` cancels.
//! * What the monitor has COLLECTED is no part of that: a monitor that monitors no resource
//!   serves a start too, and the query it records reports the repository's columns with no
//!   rows.
//! * A `MonitorName` this account does not hold, and a time period that ends before it
//!   starts, are refused (400) and record no query.
//! * A `QueryId` no query of the named monitor carries is refused by both readers with the
//!   modelled `ValidationException` (400) — `StopQuery` never succeeds as a no-op for one.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator binary
//! mounts. The harness (`emulator`, `emulator_on`, `call`, `create_monitor`) lives in
//! `crate::monitor_lifecycle` and is reused here rather than copied.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::ops::types::{
    OPERATOR_VALUES, QUERY_TYPE_VALUES,
};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::state::{
    invalid_resource_id_message, monitor_resource_path, service_state, QueryRecord,
    INVALID_QUERY_ID_MESSAGE, INVALID_REQUEST_BODY_MESSAGE, MONITOR_STATE_ACTIVE,
    QUERY_STATUS_CANCELED, QUERY_STATUS_QUEUED, QUERY_STATUS_RUNNING, QUERY_STATUS_SUCCEEDED,
    QUERY_STEP_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::monitor_lifecycle::{
    call, create_monitor, emulator, emulator_on, error_code, error_message, settled_monitor,
};

/// A resource ARN a monitor can monitor: a VPC, one of the resource kinds the model names
/// ("Resources can be VPCs, NLBs, Amazon CloudFront distributions, or Amazon WorkSpaces
/// directories").
fn vpc_arn(suffix: &str) -> String {
    format!("arn:aws:ec2:us-east-1:000000000000:vpc/vpc-{suffix}")
}

/// A `date-time` timestamp, the form `StartQuery`'s `@timestampFormat` members take.
fn wire_time(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_secs();
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).expect("an in-range instant"))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable timestamp")
}

/// A `StartQuery` body for `query_type` over the two hours before `now`.
fn start_body(query_type: &str, now: SystemTime) -> Value {
    json!({
        "StartTime": wire_time(now - Duration::from_secs(2 * 60 * 60)),
        "EndTime": wire_time(now),
        "QueryType": query_type,
        "FilterParameters": [],
    })
}

/// `POST /v20210603/Monitors/{monitor}/Queries`.
async fn start_query(
    router: &MultiServiceRouter,
    monitor: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v20210603/Monitors/{monitor}/Queries"),
        Some(body),
    )
    .await
}

/// `GET /v20210603/Monitors/{monitor}/Queries/{query_id}/Status`.
async fn query_status(
    router: &MultiServiceRouter,
    monitor: &str,
    query_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v20210603/Monitors/{monitor}/Queries/{query_id}/Status"),
        None,
    )
    .await
}

/// `DELETE /v20210603/Monitors/{monitor}/Queries/{query_id}`.
async fn stop_query(
    router: &MultiServiceRouter,
    monitor: &str,
    query_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/v20210603/Monitors/{monitor}/Queries/{query_id}"),
        None,
    )
    .await
}

/// The reported `Status` of a query, asserting the read succeeded.
async fn reported_status(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    query_id: &str,
) -> String {
    let (status, body) = query_status(router, monitor, query_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueryStatus expected 200, got {status} with {body}"
    );
    body["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Status in {body}"))
        .to_string()
}

/// A monitor with one resource, so a query against it has traffic to report on.
async fn monitor_with_resources(router: &MultiServiceRouter, name: &str) {
    create_monitor(
        router,
        json!({ "MonitorName": name, "Resources": [vpc_arn("aaaa0001")] }),
    )
    .await;
    let (status, got) = call(
        router,
        "GET",
        &format!("/v20210603/Monitors/{name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "{name}: expected 200 from GetMonitor, got {status} with {got}"
    );
}

/// One seeded query: the two instants the clock owes it, whether it was stopped, and the
/// status a poll must then report.
struct SeededCase {
    /// Names the row in every assertion message.
    case: &'static str,
    /// When the `QUEUED` -> `RUNNING` transition is due.
    running_at: Option<SystemTime>,
    /// When the `RUNNING` -> `SUCCEEDED` transition is due.
    succeeded_at: Option<SystemTime>,
    /// Whether the row stops the query before reading it.
    stopped: bool,
    /// The status the row must report.
    expected: &'static str,
}

/// One refused `StartQuery`: how it is sent, and the shape and wording it must come back
/// with. Every row is a 400; the rows differ in which refusal family answers.
struct StartCase {
    /// Names the row in every assertion message.
    case: &'static str,
    /// The `MonitorName` in the path.
    monitor: &'static str,
    /// The request body.
    body: Value,
    /// The error code the refusal must carry.
    code: &'static str,
    /// The message the refusal must carry.
    message: String,
}

// ---------------------------------------------------------------------------------------
// A query is started against any monitor this account holds
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn start_query_accepts_a_monitor_that_monitors_resources() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    monitor_with_resources(&router, "one-resource-monitor").await;
    create_monitor(
        &router,
        json!({
            "MonitorName": "many-resource-monitor",
            "Resources": [vpc_arn("bbbb0001"), vpc_arn("bbbb0002"), vpc_arn("bbbb0003")],
        }),
    )
    .await;

    // Rows differ along how many resources the monitor holds — one, and several — and
    // along every `QueryType` the model declares, so the rule is checked over the whole
    // catalog rather than the one type a recording happened to use
    // (`smithy_query(query="shape",shape="QueryType")`, re-exported as
    // `QUERY_TYPE_VALUES`).
    let now = SystemTime::now();
    let mut minted: Vec<(String, String)> = Vec::new();
    for monitor in ["one-resource-monitor", "many-resource-monitor"] {
        for query_type in QUERY_TYPE_VALUES {
            let case = format!("{monitor} asked for {query_type}");
            let (status, started) =
                start_query(&router, monitor, start_body(query_type, now)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {started}"
            );
            let query_id = started["QueryId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no QueryId in {started}"))
                .to_string();
            assert!(
                !query_id.is_empty(),
                "case {case:?}: the minted QueryId must not be empty, got {started}"
            );

            // The id is not just handed back: the query is recorded against the monitor it
            // names, in the non-terminal state its lifecycle starts in.
            assert_eq!(
                reported_status(&router, &case, monitor, &query_id).await,
                QUERY_STATUS_QUEUED,
                "case {case:?}: a started query must first report QUEUED"
            );
            minted.push((case, query_id));
        }
    }

    // Every start minted its own id, so no row was served another row's query.
    let distinct: BTreeSet<&String> = minted.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "each start must mint its own QueryId, got {minted:?}"
    );
}

#[tokio::test]
async fn start_query_serves_a_monitor_that_monitors_no_resource() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    // Each row is a monitor that monitors nothing, reached a different way: a create that
    // sent no `Resources` member at all, one that sent the member empty, and one whose only
    // resource `UpdateMonitor` took away. What a monitor MONITORS is not what a start is
    // weighed on, however the monitor came to be empty.
    create_monitor(&router, json!({ "MonitorName": "no-member-monitor" })).await;
    create_monitor(&router, json!({ "MonitorName": "empty-list-monitor", "Resources": [] })).await;
    monitor_with_resources(&router, "emptied-monitor").await;
    let (status, emptied) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/emptied-monitor",
        Some(json!({ "ResourcesToRemove": [vpc_arn("aaaa0001")] })),
    )
    .await;
    assert_eq!(
        status, 200,
        "emptying a monitor: expected 200, got {status} with {emptied}"
    );

    let now = SystemTime::now();
    let mut minted: Vec<(String, String)> = Vec::new();
    for monitor in ["no-member-monitor", "empty-list-monitor", "emptied-monitor"] {
        // The monitor the start is served for is a settled, ACTIVE one: it is where the
        // monitor is in its own lifecycle, not what it monitors, that a start is read
        // against.
        settled_monitor(&router, monitor, monitor, MONITOR_STATE_ACTIVE).await;

        for query_type in QUERY_TYPE_VALUES {
            let case = format!("{monitor} asked for {query_type}");
            let (status, started) =
                start_query(&router, monitor, start_body(query_type, now)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {started}"
            );
            let query_id = started["QueryId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no QueryId in {started}"))
                .to_string();
            assert!(
                !query_id.is_empty(),
                "case {case:?}: the minted QueryId must not be empty, got {started}"
            );
            assert_eq!(
                reported_status(&router, &case, monitor, &query_id).await,
                QUERY_STATUS_QUEUED,
                "case {case:?}: a started query must first report QUEUED"
            );
            minted.push((case, query_id));
        }

        // The start recorded the query it answered with, and left the monitor itself where
        // it was: it still reads back ACTIVE, monitoring nothing.
        let recorded: Vec<String> = {
            let state = service_state(&*engine).expect("the monitor store");
            let store = state.lock();
            store
                .queries
                .values()
                .filter(|query| query.monitor_name == monitor)
                .map(|query| query.query_id.clone())
                .collect()
        };
        assert_eq!(
            recorded.len(),
            QUERY_TYPE_VALUES.len(),
            "case {monitor:?}: every served start must record its query, got {recorded:?}"
        );
        let got = settled_monitor(&router, monitor, monitor, MONITOR_STATE_ACTIVE).await;
        assert_eq!(
            got["Resources"],
            json!([]),
            "case {monitor:?}: the monitor must still monitor nothing, got {got}"
        );
    }

    // Every start minted its own id, so no caller was served another caller's query.
    let distinct: BTreeSet<&String> = minted.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "each start must mint its own QueryId, got {minted:?}"
    );

    // A query of a monitor that collected nothing reports the repository's own columns and
    // no rows — the empty result the start promised, rather than a refusal.
    let (case, query_id) = minted
        .first()
        .cloned()
        .expect("a served start to read back");
    let (status, results) = call(
        &router,
        "GET",
        &format!("/v20210603/Monitors/no-member-monitor/Queries/{query_id}/Results"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueryResults expected 200, got {status} with {results}"
    );
    assert!(
        results["Fields"].as_array().is_some_and(|fields| !fields.is_empty()),
        "case {case:?}: the repository's columns must be reported, got {results}"
    );
    assert!(
        results
            .get("Data")
            .and_then(Value::as_array)
            .is_none_or(|rows| rows.is_empty()),
        "case {case:?}: a monitor that collected nothing reports no row, got {results}"
    );
}

#[tokio::test]
async fn start_query_refuses_a_request_it_cannot_serve() {
    let router = emulator();
    monitor_with_resources(&router, "queryable-monitor").await;

    let now = SystemTime::now();
    let inverted = json!({
        "StartTime": wire_time(now),
        "EndTime": wire_time(now - Duration::from_secs(60)),
        "QueryType": QUERY_TYPE_VALUES[0],
        "FilterParameters": [],
    });

    let cases = vec![
        StartCase {
            case: "a time period that ends before it starts",
            monitor: "queryable-monitor",
            body: inverted,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        StartCase {
            case: "a monitor this account does not hold",
            monitor: "no-such-monitor",
            body: start_body(QUERY_TYPE_VALUES[0], now),
            code: "ValidationException",
            message: invalid_resource_id_message(&monitor_resource_path("no-such-monitor")),
        },
        StartCase {
            case: "a QueryType outside the modelled value set",
            monitor: "queryable-monitor",
            body: json!({
                "StartTime": wire_time(now - Duration::from_secs(3600)),
                "EndTime": wire_time(now),
                "QueryType": "MEASUREMENT",
            }),
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        StartCase {
            case: "a LinkedAccountId this monitoring account is not linked to",
            monitor: "queryable-monitor",
            body: json!({
                "StartTime": wire_time(now - Duration::from_secs(3600)),
                "EndTime": wire_time(now),
                "QueryType": QUERY_TYPE_VALUES[0],
                "LinkedAccountId": "123456789012",
            }),
            code: "ValidationException",
            message: "LinkedAccountId 123456789012 is not an account linked to this monitoring account."
                .to_string(),
        },
    ];

    // Every row is refused with 400: what differs is the shape, and that is what each row
    // names.
    for StartCase {
        case,
        monitor,
        body,
        code,
        message,
    } in cases
    {
        let (status, refused) = start_query(&router, monitor, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            code,
            "case {case:?}: expected {code}, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            message,
            "case {case:?}: expected message {message:?}, got {refused}"
        );
    }
}

#[tokio::test]
async fn a_start_query_carrying_filter_parameters_keeps_what_it_filtered_on() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    monitor_with_resources(&router, "filtered-monitor").await;

    // `FilterParameters` is a list of structures — the member kind the rest of this suite
    // only ever sends empty — and its `Operator` is one of `OPERATOR_VALUES`
    // (`smithy_query(query="shape",shape="Operator")`). The rows below walk the forms a
    // request can send it in: one filter, several, and one carrying several values.
    let now = SystemTime::now();
    let filters = json!([
        { "Field": "city", "Operator": OPERATOR_VALUES[0], "Values": ["Seattle"] },
        {
            "Field": "aso",
            "Operator": OPERATOR_VALUES[1],
            "Values": ["AS1", "AS2", "AS3"],
        },
    ]);
    let (status, started) = start_query(
        &router,
        "filtered-monitor",
        json!({
            "StartTime": wire_time(now - Duration::from_secs(3600)),
            "EndTime": wire_time(now),
            "QueryType": QUERY_TYPE_VALUES[0],
            "FilterParameters": filters,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a filtered start: expected 200, got {status} with {started}"
    );
    let query_id = started["QueryId"]
        .as_str()
        .expect("a minted QueryId")
        .to_string();

    // The query the store holds carries the filters the request sent, so the list member
    // was read whole rather than dropped. Read inside its own scope, so the store's guard
    // is released before the next call.
    let held_filters: Vec<(String, String, Vec<String>)> = {
        let store = service_state(&*engine).expect("the service's own store");
        let guard = store.lock();
        let held = guard
            .query_for("filtered-monitor", &query_id)
            .unwrap_or_else(|| {
                panic!("the started query must be stored, got none for {query_id}")
            });
        held.filter_parameters
            .iter()
            .map(|filter| {
                (
                    filter.field.clone().unwrap_or_default(),
                    filter.operator.clone().unwrap_or_default(),
                    filter.values.clone().unwrap_or_default(),
                )
            })
            .collect()
    };
    assert_eq!(
        held_filters,
        vec![
            (
                "city".to_string(),
                OPERATOR_VALUES[0].to_string(),
                vec!["Seattle".to_string()]
            ),
            (
                "aso".to_string(),
                OPERATOR_VALUES[1].to_string(),
                vec!["AS1".to_string(), "AS2".to_string(), "AS3".to_string()]
            ),
        ],
        "the stored query must carry the filters the request sent, got {held_filters:?}"
    );

    // And it is a query of that monitor like any other.
    let reported =
        reported_status(&router, "a filtered start", "filtered-monitor", &query_id).await;
    assert_eq!(
        reported, QUERY_STATUS_QUEUED,
        "a filtered start must report QUEUED first, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------------------
// The lifecycle the model documents: QUEUED -> RUNNING -> SUCCEEDED
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_started_query_is_queued_and_the_clock_carries_it_to_succeeded() {
    let router = emulator();
    monitor_with_resources(&router, "lifecycle-monitor").await;

    // One query per `QueryType`, so the lifecycle is checked over the whole catalog, and
    // each start mints its own identifier.
    let now = SystemTime::now();
    let mut minted: Vec<(&str, String)> = Vec::new();
    for query_type in QUERY_TYPE_VALUES {
        let (status, started) =
            start_query(&router, "lifecycle-monitor", start_body(query_type, now)).await;
        assert_eq!(
            status, 200,
            "case {query_type:?}: expected 200, got {status} with {started}"
        );
        let query_id = started["QueryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {query_type:?}: no QueryId in {started}"))
            .to_string();
        assert!(
            query_id.len() == 36
                && query_id
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == '-'),
            "case {query_type:?}: expected a minted identifier, got {query_id:?}"
        );
        assert_eq!(
            reported_status(&router, query_type, "lifecycle-monitor", &query_id).await,
            QUERY_STATUS_QUEUED,
            "case {query_type:?}: a started query must first report QUEUED"
        );
        minted.push((query_type, query_id));
    }

    let distinct: BTreeSet<&String> = minted.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "each start must mint its own QueryId, got {minted:?}"
    );

    // The clock owes each query the rest of its lifecycle, so polling reports the terminal
    // status without anything else having been called. The poll loop has a deadline rather
    // than one fixed sleep, so the case does not depend on how long the machine takes
    // between calls; what it asserts is that every status it ever observes is one of the
    // lifecycle's, and that the query ends `SUCCEEDED`.
    let deadline = std::time::Instant::now()
        + QUERY_STEP_DELAY
        + QUERY_STEP_DELAY
        + Duration::from_secs(10);
    for (query_type, query_id) in &minted {
        loop {
            let seen = reported_status(&router, query_type, "lifecycle-monitor", query_id).await;
            assert!(
                [
                    QUERY_STATUS_QUEUED,
                    QUERY_STATUS_RUNNING,
                    QUERY_STATUS_SUCCEEDED
                ]
                .contains(&seen.as_str()),
                "case {query_type:?}: a started query must report a lifecycle status, got {seen:?}"
            );
            if seen == QUERY_STATUS_SUCCEEDED {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {query_type:?}: the clock never carried the query past {seen:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // Polling is a report: it does not move the query any further.
        let again = reported_status(&router, query_type, "lifecycle-monitor", query_id).await;
        assert_eq!(
            again, QUERY_STATUS_SUCCEEDED,
            "case {query_type:?}: a second poll must report the same status, got {again:?}"
        );
    }
}

#[tokio::test]
async fn the_reported_status_is_the_one_the_clock_already_owed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    monitor_with_resources(&router, "seeded-monitor").await;

    let now = SystemTime::now();
    let span = Duration::from_secs(60);

    // A due instant in the past is a transition the clock already owed; one in the future
    // is not yet owed.
    let cases = vec![
        SeededCase {
            case: "neither transition due yet",
            running_at: Some(now + span),
            succeeded_at: Some(now + span + span),
            stopped: false,
            expected: QUERY_STATUS_QUEUED,
        },
        SeededCase {
            case: "only the first transition due",
            running_at: Some(now - span),
            succeeded_at: Some(now + span),
            stopped: false,
            expected: QUERY_STATUS_RUNNING,
        },
        SeededCase {
            case: "both transitions due",
            running_at: Some(now - span - span),
            succeeded_at: Some(now - span),
            stopped: false,
            expected: QUERY_STATUS_SUCCEEDED,
        },
        SeededCase {
            case: "a stopped query the clock owes nothing",
            running_at: None,
            succeeded_at: None,
            stopped: true,
            expected: QUERY_STATUS_CANCELED,
        },
    ];

    for (
        index,
        SeededCase {
            case,
            running_at,
            succeeded_at,
            stopped,
            expected,
        },
    ) in cases.into_iter().enumerate()
    {
        let query_id = format!("seeded-query-{index}");
        let mut record = QueryRecord::new(
            query_id.clone(),
            "seeded-monitor",
            QUERY_TYPE_VALUES[0],
            0.0,
            60.0,
            Vec::new(),
            now,
        );
        record.running_at = running_at;
        record.succeeded_at = succeeded_at;
        if stopped {
            record.cancel();
        }
        service_state(&*engine)
            .expect("the service's own store")
            .lock()
            .record_query(record);

        let reported = reported_status(&router, case, "seeded-monitor", &query_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// StopQuery
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn stop_query_cancels_a_query_in_progress_and_leaves_a_terminal_one() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    monitor_with_resources(&router, "stopped-monitor").await;

    let now = SystemTime::now();
    let span = Duration::from_secs(60);

    // A stop reaches a query wherever the clock has carried it; one that has already
    // finished keeps the status it reached.
    let cases = vec![
        SeededCase {
            case: "a queued query",
            running_at: Some(now + span),
            succeeded_at: Some(now + span + span),
            stopped: false,
            expected: QUERY_STATUS_CANCELED,
        },
        SeededCase {
            case: "a running query",
            running_at: Some(now - span),
            succeeded_at: Some(now + span),
            stopped: false,
            expected: QUERY_STATUS_CANCELED,
        },
        SeededCase {
            case: "a query that already succeeded",
            running_at: Some(now - span - span),
            succeeded_at: Some(now - span),
            stopped: false,
            expected: QUERY_STATUS_SUCCEEDED,
        },
    ];

    for (
        index,
        SeededCase {
            case,
            running_at,
            succeeded_at,
            stopped: _,
            expected,
        },
    ) in cases.into_iter().enumerate()
    {
        let query_id = format!("stopped-query-{index}");
        let mut record = QueryRecord::new(
            query_id.clone(),
            "stopped-monitor",
            QUERY_TYPE_VALUES[0],
            0.0,
            60.0,
            Vec::new(),
            now,
        );
        record.running_at = running_at;
        record.succeeded_at = succeeded_at;
        service_state(&*engine)
            .expect("the service's own store")
            .lock()
            .record_query(record);

        let (status, stopped) = stop_query(&router, "stopped-monitor", &query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopQuery expected 200, got {status} with {stopped}"
        );
        let reported = reported_status(&router, case, "stopped-monitor", &query_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected} after the stop, got {reported:?}"
        );

        // Stopping again is the same answer: the status it reached is terminal.
        let (status, again) = stop_query(&router, "stopped-monitor", &query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second StopQuery expected 200, got {status} with {again}"
        );
        let still = reported_status(&router, case, "stopped-monitor", &query_id).await;
        assert_eq!(
            still, expected,
            "case {case:?}: a second stop must leave the terminal status, got {still:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// A QueryId the named monitor has no query for
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_query_id_the_named_monitor_has_no_query_for_is_refused_by_both_readers() {
    let router = emulator();
    monitor_with_resources(&router, "owning-monitor").await;
    monitor_with_resources(&router, "other-monitor").await;
    monitor_with_resources(&router, "doomed-monitor").await;

    let now = SystemTime::now();
    let (status, started) =
        start_query(&router, "owning-monitor", start_body(QUERY_TYPE_VALUES[0], now)).await;
    assert_eq!(
        status, 200,
        "starting the query under test: expected 200, got {status} with {started}"
    );
    let owned = started["QueryId"]
        .as_str()
        .expect("a minted QueryId")
        .to_string();

    // A query of a monitor that is then deleted goes with the monitor: its id is no longer
    // reportable, even once a monitor of the same name exists again.
    let (status, doomed) =
        start_query(&router, "doomed-monitor", start_body(QUERY_TYPE_VALUES[0], now)).await;
    assert_eq!(
        status, 200,
        "starting the deleted monitor's query: expected 200, got {status} with {doomed}"
    );
    let orphaned = doomed["QueryId"]
        .as_str()
        .expect("a minted QueryId")
        .to_string();
    let (status, deleted) = call(
        &router,
        "DELETE",
        "/v20210603/Monitors/doomed-monitor",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the monitor: expected 200, got {status} with {deleted}"
    );
    monitor_with_resources(&router, "doomed-monitor").await;

    // (case, monitor, query id). Each row names an id that resolves no query OF THAT
    // monitor: one never minted, one minted for another monitor, one whose monitor was
    // deleted, and ids that are not of the minted form at all.
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "a well-formed id nothing minted",
            "owning-monitor",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "another monitor's query id",
            "other-monitor",
            owned.clone(),
        ),
        (
            "the query id of a deleted monitor",
            "doomed-monitor",
            orphaned,
        ),
        ("an id of no particular form", "owning-monitor", "not-a-query-id".to_string()),
        (
            "the owned id with one character changed",
            "owning-monitor",
            {
                let mut flipped = owned.clone();
                flipped.pop();
                format!("{flipped}0")
            },
        ),
    ];

    for (case, monitor, query_id) in cases {
        let (status, polled) = query_status(&router, monitor, &query_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetQueryStatus expected 400, got {status} with {polled}"
        );
        assert_eq!(
            error_code(&polled),
            "ValidationException",
            "case {case:?}: GetQueryStatus expected ValidationException, got {polled}"
        );
        assert_eq!(
            error_message(&polled),
            INVALID_QUERY_ID_MESSAGE,
            "case {case:?}: GetQueryStatus unexpected message in {polled}"
        );

        let (status, stopped) = stop_query(&router, monitor, &query_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: StopQuery expected 400, got {status} with {stopped}"
        );
        assert_eq!(
            error_code(&stopped),
            "ValidationException",
            "case {case:?}: StopQuery expected ValidationException, got {stopped}"
        );
        assert_eq!(
            error_message(&stopped),
            INVALID_QUERY_ID_MESSAGE,
            "case {case:?}: StopQuery unexpected message in {stopped}"
        );
    }

    // The query that WAS started for the monitor that owns it is still reportable, and the
    // monitors are all readable afterwards.
    let reported = reported_status(&router, "the owned query", "owning-monitor", &owned).await;
    assert!(
        [
            QUERY_STATUS_QUEUED,
            QUERY_STATUS_RUNNING,
            QUERY_STATUS_SUCCEEDED
        ]
        .contains(&reported.as_str()),
        "the owned query must still report a lifecycle status, got {reported:?}"
    );
    for monitor in ["owning-monitor", "other-monitor", "doomed-monitor"] {
        let (status, got) = call(
            &router,
            "GET",
            &format!("/v20210603/Monitors/{monitor}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {monitor:?}: the monitor must stay readable, got {status} with {got}"
        );
    }
}
