// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetScheduledQueryHistory`.
//!
//! The rule under test: a scheduled query's execution history is the record of the runs its
//! own `scheduleExpression` has already made due — nothing a read decides — and one record
//! reports the id of the query that run produced, the status that run reached and the instant
//! it was triggered, plus one entry per destination the query delivers to, reported as
//! processed only once the delivery really happened. A read answers the query's own name and
//! ARN under either form of `ScheduledQueryIdentifier`, reports the records of the window the
//! request asks for (both ends included) narrowed to the statuses it names, newest run first,
//! in pages of `maxResults` resumed by `nextToken` — and reading it twice reports the same
//! runs.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! that is not computed from the shared store cannot pass.
//!
//! The tables span what the rule generalises over rather than one request: each form of
//! schedule expression (a rate, a cron, a cron evaluated in a named timezone), a lookback
//! stated and left to the documented default, each destination the model's
//! `DestinationConfiguration` carries (Amazon S3, a lookup table) and a destination the
//! delivery cannot reach, every status of the model's `ExecutionStatus` enum as a filter,
//! windows that cover every record, each end of one, the middle of them and none of them, and
//! page sizes from one record to more than the history holds.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    scheduled_query_arn, scheduled_query_not_found_message,
    ComAmazonawsCloudwatchlogsLogs20140328State, ScheduledQueryExecution,
    REVERSED_TIME_RANGE_MESSAGE,
};

use crate::harness::{base_ms, batch, group_with_stream, now_ms, Logs, REGION};

/// The operations under test, and the ones a history record is proved through.
const HISTORY: &str = "GetScheduledQueryHistory";
const CREATE: &str = "CreateScheduledQuery";
const GET: &str = "GetScheduledQuery";
const RESULTS: &str = "GetQueryResults";

/// The code answered when the scheduled query a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when a parameter of the request is not valid.
const VALIDATION: &str = "ValidationException";

/// The log group every case queries, and the stream its events are written to.
const GROUP: &str = "/aws/emulator/scheduled-query-history";
const STREAM: &str = "runs";

/// The IAM role every case executes under: "The ARN of the IAM role used to execute the query
/// and deliver results."
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// The pipeline every case runs: the ERROR events of the window, as rows.
const QUERY_STRING: &str = "fields @timestamp, @message | filter @message like /ERROR/";

/// The messages seeded into the group — two of them ERROR, which is what the pipeline above
/// selects, so a run that really read the log data produces exactly two rows.
const MESSAGES: [&str; 3] = [
    "ERROR order 1 failed",
    "INFO order 2 accepted",
    "ERROR order 3 failed",
];

/// How many of [`MESSAGES`] the pipeline selects.
const SELECTED_ROWS: usize = 2;

/// How long a read of the history is given to report a run the clock has made due. Generous
/// on purpose: the test waits for the CONDITION — a record being there — and never on a fixed
/// span, so a loaded or a fast host both reach the same verdict.
const HISTORY_DEADLINE: Duration = Duration::from_secs(180);

/// How long to wait between two looks while polling for one.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A window that covers every instant a case's runs can fall at, in the epoch SECONDS this
/// operation states its window in.
fn whole_window() -> (i64, i64) {
    (0, now_ms() / 1_000 + 24 * 60 * 60)
}

/// Seed [`GROUP`] with [`MESSAGES`], one event per second an hour back, so every run's
/// lookback window below holds them.
async fn seed_events(logs: &Logs) {
    group_with_stream(logs, GROUP, STREAM, None).await;
    let first_ms = base_ms();
    let events: Vec<(i64, &str)> = MESSAGES
        .iter()
        .enumerate()
        .map(|(index, message)| (first_ms + index as i64 * 1_000, *message))
        .collect();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&events),
        }),
    )
    .await;
}

/// The `triggerHistory` of one read of `identifier`'s history over the whole window.
async fn history(logs: &Logs, identifier: &str) -> Vec<Value> {
    let (start_time, end_time) = whole_window();
    let page = logs
        .ok(
            HISTORY,
            &json!({"identifier": identifier, "startTime": start_time, "endTime": end_time}),
        )
        .await;
    records(&page)
}

/// The `triggerHistory` of one answered page.
fn records(page: &Value) -> Vec<Value> {
    page["triggerHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("a history page carries triggerHistory, got {page}"))
        .clone()
}

/// Poll `identifier`'s history until it holds at least `wanted` records, answering them.
///
/// The runs are owed by the clock, so the condition — not a span — is what is waited for.
async fn history_of_at_least(
    logs: &Logs,
    case: &str,
    identifier: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + HISTORY_DEADLINE;
    loop {
        let found = history(logs, identifier).await;
        if found.len() >= wanted {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected at least {wanted} execution(s) in the history of \
             {identifier}, got {found:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The value of `member` on `record`, as a string.
fn member(case: &str, record: &Value, name: &str) -> String {
    record[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a record carries {name}, got {record}"))
        .to_string()
}

/// One row of the schedule table: how the query is scheduled, and what lookback it states.
struct Schedule {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The name the scheduled query is created under.
    name: &'static str,
    /// Its `scheduleExpression`.
    expression: &'static str,
    /// Its `timezone`, when the row names one.
    timezone: Option<&'static str>,
    /// Its `startTimeOffset` — the lookback — or `None` to take the documented default, which
    /// is the period of the schedule itself.
    start_time_offset: Option<i64>,
}

/// The runs a scheduled query's own schedule makes due are recorded, run over the log data of
/// their own window, and reported by the history with the id of the query each produced.
#[tokio::test]
async fn the_history_reports_the_runs_the_schedule_made_due() {
    let logs = Logs::new();
    seed_events(&logs).await;

    // Each row's schedule names its SECOND run a day or more away, so the history it reports
    // holds exactly the first run and two reads of it report the same one.
    let rows: [Schedule; 4] = [
        Schedule {
            case: "a rate schedule with a stated lookback",
            name: "errors-every-day",
            expression: "rate(1 day)",
            timezone: None,
            start_time_offset: Some(7_200),
        },
        Schedule {
            case: "a cron schedule taking the default lookback",
            name: "errors-at-three",
            expression: "cron(0 3 * * ? *)",
            timezone: None,
            start_time_offset: None,
        },
        Schedule {
            // The default lookback is the schedule's own PERIOD — the gap between two
            // instants it names — and not the time until its next instant, so a weekly cron
            // reads a week back whatever time of day its first run is triggered at.
            case: "a weekly cron schedule taking the default lookback",
            name: "errors-on-monday",
            expression: "cron(0 3 ? * MON *)",
            timezone: None,
            start_time_offset: None,
        },
        Schedule {
            case: "a cron schedule evaluated in a named timezone",
            name: "errors-at-three-in-new-york",
            expression: "cron(0 3 * * ? *)",
            timezone: Some("America/New_York"),
            start_time_offset: Some(7_200),
        },
    ];

    for Schedule {
        case,
        name,
        expression,
        timezone,
        start_time_offset,
    } in rows
    {
        let mut request = json!({
            "name": name,
            "queryLanguage": "CWLI",
            "queryString": QUERY_STRING,
            "scheduleExpression": expression,
            "executionRoleArn": EXECUTION_ROLE,
            "logGroupIdentifiers": [GROUP],
        });
        if let Some(timezone) = timezone {
            request["timezone"] = json!(timezone);
        }
        if let Some(offset) = start_time_offset {
            request["startTimeOffset"] = json!(offset);
        }
        let created_at = now_ms();
        logs.ok(CREATE, &request).await;

        let found = history_of_at_least(&logs, case, name, 1).await;
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: the schedule has made exactly one run due, got {found:?}"
        );
        let record = &found[0];
        assert_eq!(
            member(case, record, "executionStatus"),
            "Complete",
            "case {case:?}: a run that produced its results is complete, got {record}"
        );
        assert!(
            record.get("errorMessage").is_none(),
            "case {case:?}: a run that did not fail reports no error message, got {record}"
        );
        let triggered = record["triggeredTimestamp"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: a record is triggered at an instant, got {record}"));
        assert!(
            triggered >= created_at / 1_000 && triggered <= now_ms() / 1_000,
            "case {case:?}: expected the run to be triggered between {created_at} and now, got \
             {triggered}"
        );

        // The id the record carries is a handle on THIS run's results: the rows are the ERROR
        // events of the window the run read, and nothing else.
        let query_id = member(case, record, "queryId");
        let results = logs.ok(RESULTS, &json!({"queryId": query_id})).await;
        assert_eq!(
            results["status"].as_str(),
            Some("Complete"),
            "case {case:?}: the run's query has run, got {results}"
        );
        let rows = results["results"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the results are rows, got {results}"))
            .clone();
        assert_eq!(
            rows.len(),
            SELECTED_ROWS,
            "case {case:?}: expected the {SELECTED_ROWS} ERROR events of the window, got {rows:?}"
        );
        let messages: Vec<String> = rows
            .iter()
            .flat_map(|row| row.as_array().cloned().unwrap_or_default())
            .filter(|field| field["field"].as_str() == Some("@message"))
            .map(|field| field["value"].as_str().unwrap_or_default().to_string())
            .collect();
        for expected in MESSAGES.iter().filter(|message| message.contains("ERROR")) {
            assert!(
                messages.contains(&expected.to_string()),
                "case {case:?}: expected the run's rows to hold {expected:?}, got {messages:?}"
            );
        }

        // The query itself now reports that run as its last: both members report an execution
        // that happened. `lastTriggeredTime` is an instant of the RESOURCE, so it carries the
        // epoch milliseconds its `creationTime` does, while the history states the same run in
        // the epoch seconds its own window is stated in.
        let reported = logs.ok(GET, &json!({"identifier": name})).await;
        assert_eq!(
            reported["lastTriggeredTime"].as_i64().map(|ms| ms / 1_000),
            Some(triggered),
            "case {case:?}: the query's last triggered instant is that run's, got {reported}"
        );
        assert_eq!(
            reported["lastExecutionStatus"].as_str(),
            Some("Complete"),
            "case {case:?}: the query's last execution status is that run's, got {reported}"
        );

        // Reading the history changes nothing a later caller sees: the same run, under the ARN
        // form of the identifier too.
        let again = history(&logs, &scheduled_query_arn(REGION, name)).await;
        assert_eq!(
            again, found,
            "case {case:?}: a second read reports the same runs, got {again:?}"
        );
    }
}

/// One row of the schedule-window table: how the query's window and state stand, and whether
/// a run is due at all.
struct Window {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The name the scheduled query is created under.
    name: &'static str,
    /// Its `scheduleStartTime`, in the epoch seconds this surface states its instants in.
    start: Option<i64>,
    /// Its `scheduleEndTime`, in the same unit.
    end: Option<i64>,
    /// Its `state`.
    state: &'static str,
    /// Whether the schedule owes this query a run by now.
    due: bool,
}

/// Whether a run is due at all follows the query's own schedule window and state: a query is
/// eligible from its `scheduleStartTime` ("The query will not execute before this time"), stops
/// at its `scheduleEndTime` ("The query will stop executing after this time") and runs nothing
/// at all while it is `DISABLED`.
#[tokio::test]
async fn the_schedule_window_and_the_state_decide_whether_a_run_is_due() {
    let logs = Logs::new();
    seed_events(&logs).await;
    // The window's ends are stated in the epoch SECONDS this surface states its instants in.
    let now_secs = now_ms() / 1_000;

    let rows: [Window; 4] = [
        Window {
            case: "a query inside its schedule window",
            name: "inside-its-window",
            start: Some(now_secs - 60),
            end: Some(now_secs + 3_600),
            state: "ENABLED",
            due: true,
        },
        Window {
            case: "a query whose window has not begun",
            name: "before-its-window",
            start: Some(now_secs + 3_600),
            end: None,
            state: "ENABLED",
            due: false,
        },
        Window {
            case: "a query whose window has ended",
            name: "after-its-window",
            start: None,
            end: Some(now_secs - 60),
            state: "ENABLED",
            due: false,
        },
        Window {
            case: "a disabled query",
            name: "disabled-query",
            start: None,
            end: None,
            state: "DISABLED",
            due: false,
        },
    ];

    for row in &rows {
        let mut request = json!({
            "name": row.name,
            "queryLanguage": "CWLI",
            "queryString": QUERY_STRING,
            "scheduleExpression": "rate(1 day)",
            "executionRoleArn": EXECUTION_ROLE,
            "logGroupIdentifiers": [GROUP],
            "startTimeOffset": 7_200,
            "state": row.state,
        });
        if let Some(start) = row.start {
            request["scheduleStartTime"] = json!(start);
        }
        if let Some(end) = row.end {
            request["scheduleEndTime"] = json!(end);
        }
        logs.ok(CREATE, &request).await;
    }

    // The query inside its window is the one whose run falls due; every other query has had
    // the same chance of the clock by the time that run is recorded, so one that still reports
    // nothing is one the schedule owes nothing.
    let eligible = &rows[0];
    let found = history_of_at_least(&logs, eligible.case, eligible.name, 1).await;
    assert_eq!(
        found.len(),
        1,
        "case {:?}: expected the one run its schedule made due, got {found:?}",
        eligible.case
    );
    for row in rows.iter().filter(|row| !row.due) {
        let found = history(&logs, row.name).await;
        assert!(
            found.is_empty(),
            "case {:?}: expected no run to be due, got {found:?}",
            row.case
        );
    }
}

/// Record, in the store, that `name`'s scheduled query ran at each of `runs` — what its
/// schedule makes due once the instants it names have passed.
///
/// A suite whose subject is how the history is FILTERED, ORDERED and PAGED needs several runs
/// at instants it chooses, and no request triggers one: the runs are the clock's. Each seeded
/// run is recorded as already landed, so a read reports it rather than re-running it.
fn record_runs(logs: &Logs, name: &str, runs: &[(&str, i64, &str)]) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let query = store
        .scheduled_query_mut(name)
        .unwrap_or_else(|| panic!("the scheduled query {name} is stored"));
    for (query_id, triggered_ms, status) in runs {
        let mut execution = ScheduledQueryExecution::new((*query_id).to_string(), *triggered_ms, None);
        execution.status = (*status).to_string();
        execution.run_landed = true;
        query.record_execution(execution);
    }
}

/// The one scheduled query the filtering, ordering and paging tables read, whose own schedule
/// names its next run a day away — so the only runs in its history are the ones the case
/// recorded.
async fn query_with_recorded_runs(logs: &Logs, name: &str, runs: &[(&str, i64, &str)]) {
    logs.ok(
        CREATE,
        &json!({
            "name": name,
            "queryLanguage": "CWLI",
            "queryString": QUERY_STRING,
            "scheduleExpression": "rate(1 day)",
            "executionRoleArn": EXECUTION_ROLE,
            "logGroupIdentifiers": [GROUP],
            "startTimeOffset": 7_200,
        }),
    )
    .await;
    record_runs(logs, name, runs);
}

/// The instant the recorded runs of the tables below are spaced around: each run one minute
/// apart, within the hour before now, and each at a whole second — the resolution this
/// operation states its window and its records' instants in.
fn run_instants() -> [i64; 5] {
    let newest = (now_ms() - 60 * 1_000) / 1_000 * 1_000;
    [
        newest - 4 * 60 * 1_000,
        newest - 3 * 60 * 1_000,
        newest - 2 * 60 * 1_000,
        newest - 60 * 1_000,
        newest,
    ]
}

/// Those instants as the epoch SECONDS a window names them in.
fn run_seconds() -> [i64; 5] {
    run_instants().map(|instant| instant / 1_000)
}

/// The statuses the recorded runs carry, one per instant: every value of the model's
/// `ExecutionStatus` enum
/// (`smithy_query(query="shape",shape="ExecutionStatus")`: `Running` | `InvalidQuery` |
/// `Complete` | `Failed` | `Timeout`), so a filter is tested on a status no case cites as well
/// as on the one it does.
const RUN_STATUSES: [&str; 5] = ["Complete", "Failed", "Complete", "Timeout", "InvalidQuery"];

/// The ids of the recorded runs, oldest first.
const RUN_IDS: [&str; 5] = [
    "00000000-0000-4000-8000-000000000001",
    "00000000-0000-4000-8000-000000000002",
    "00000000-0000-4000-8000-000000000003",
    "00000000-0000-4000-8000-000000000004",
    "00000000-0000-4000-8000-000000000005",
];

/// The runs the tables record: each id at its own instant, carrying its own status.
fn recorded_runs(instants: &[i64; 5]) -> Vec<(&'static str, i64, &'static str)> {
    RUN_IDS
        .iter()
        .zip(instants.iter())
        .zip(RUN_STATUSES.iter())
        .map(|((id, instant), status)| (*id, *instant, *status))
        .collect()
}

/// One row of the filtering table: the window and statuses a read asks for, and the records
/// it must then report, newest run first.
struct Filter {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `startTime` the read asks for, in epoch seconds.
    start_time: i64,
    /// Its `endTime`, in the same unit.
    end_time: i64,
    /// The `executionStatuses` it narrows by, or none at all.
    statuses: Vec<&'static str>,
    /// The ids of the runs the read must report, in the order it must report them.
    expected: Vec<&'static str>,
}

/// The history is reported newest run first, holds exactly the records of the window the
/// request asks for — both of its ends included — and is narrowed to the statuses it names.
#[tokio::test]
async fn the_history_is_ordered_newest_first_and_filtered_by_window_and_status() {
    let logs = Logs::new();
    seed_events(&logs).await;
    let name = "filtered-runs";
    let instants = run_instants();
    // The window names its ends in the epoch SECONDS this operation states them in.
    let seconds = run_seconds();
    query_with_recorded_runs(&logs, name, &recorded_runs(&instants)).await;
    let newest_first: Vec<&str> = RUN_IDS.iter().rev().copied().collect();

    let rows: Vec<Filter> = vec![
        Filter {
            case: "a window covering every run",
            start_time: seconds[0] - 1,
            end_time: seconds[4] + 1,
            statuses: vec![],
            expected: newest_first.clone(),
        },
        Filter {
            case: "a window whose ends are the oldest and the newest run",
            start_time: seconds[0],
            end_time: seconds[4],
            statuses: vec![],
            expected: newest_first.clone(),
        },
        Filter {
            case: "a window of the one instant the oldest run was triggered at",
            start_time: seconds[0],
            end_time: seconds[0],
            statuses: vec![],
            expected: vec![RUN_IDS[0]],
        },
        Filter {
            case: "a window of the one instant the newest run was triggered at",
            start_time: seconds[4],
            end_time: seconds[4],
            statuses: vec![],
            expected: vec![RUN_IDS[4]],
        },
        Filter {
            case: "a window of the middle runs",
            start_time: seconds[1],
            end_time: seconds[3],
            statuses: vec![],
            expected: vec![RUN_IDS[3], RUN_IDS[2], RUN_IDS[1]],
        },
        Filter {
            case: "a window wholly before every run",
            start_time: 0,
            end_time: seconds[0] - 1,
            statuses: vec![],
            expected: vec![],
        },
        Filter {
            case: "a window wholly after every run",
            start_time: seconds[4] + 1,
            end_time: seconds[4] + 60,
            statuses: vec![],
            expected: vec![],
        },
        Filter {
            case: "every run of one status",
            start_time: seconds[0] - 1,
            end_time: seconds[4] + 1,
            statuses: vec!["Complete"],
            expected: vec![RUN_IDS[2], RUN_IDS[0]],
        },
        Filter {
            case: "a status one run carries",
            start_time: seconds[0] - 1,
            end_time: seconds[4] + 1,
            statuses: vec!["Timeout"],
            expected: vec![RUN_IDS[3]],
        },
        Filter {
            case: "two statuses at once",
            start_time: seconds[0] - 1,
            end_time: seconds[4] + 1,
            statuses: vec!["Failed", "InvalidQuery"],
            expected: vec![RUN_IDS[4], RUN_IDS[1]],
        },
        Filter {
            case: "a status no run carries",
            start_time: seconds[0] - 1,
            end_time: seconds[4] + 1,
            statuses: vec!["Running"],
            expected: vec![],
        },
        Filter {
            case: "a status filter narrowed further by the window",
            start_time: seconds[0],
            end_time: seconds[1],
            statuses: vec!["Complete"],
            expected: vec![RUN_IDS[0]],
        },
    ];

    for Filter {
        case,
        start_time,
        end_time,
        statuses,
        expected,
    } in rows
    {
        let mut request = json!({
            "identifier": name,
            "startTime": start_time,
            "endTime": end_time,
            "maxResults": 1_000,
        });
        if !statuses.is_empty() {
            request["executionStatuses"] = json!(statuses);
        }
        let page = logs.ok(HISTORY, &request).await;
        assert_eq!(
            page["name"].as_str(),
            Some(name),
            "case {case:?}: the history names its own query, got {page}"
        );
        assert_eq!(
            page["scheduledQueryArn"].as_str(),
            Some(scheduled_query_arn(REGION, name).as_str()),
            "case {case:?}: the history names its own query's ARN, got {page}"
        );
        let reported: Vec<String> = records(&page)
            .iter()
            .map(|record| member(case, record, "queryId"))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert!(
            page.get("nextToken").is_none(),
            "case {case:?}: a page holding every record is the last one, got {page}"
        );
    }
}

/// One page of the history holds at most `maxResults` records and is resumed by the token the
/// page before it answered, so the pages of one enumeration concatenate to the whole of it in
/// its own order.
#[tokio::test]
async fn the_history_is_paged_by_max_results_and_resumed_by_its_token() {
    let logs = Logs::new();
    seed_events(&logs).await;
    let name = "paged-runs";
    let instants = run_instants();
    let seconds = run_seconds();
    query_with_recorded_runs(&logs, name, &recorded_runs(&instants)).await;
    let whole: Vec<&str> = RUN_IDS.iter().rev().copied().collect();

    // (case, the page size asked for, how many pages the enumeration takes)
    let cases: [(&str, usize, usize); 5] = [
        ("one record per page", 1, 5),
        ("two records per page", 2, 3),
        ("a page size that divides the history", 5, 1),
        ("a page larger than the history", 10, 1),
        ("the largest page the member admits", 1_000, 1),
    ];
    for (case, page_size, pages) in cases {
        let mut token: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        let mut walked = 0usize;
        loop {
            let mut request = json!({
                "identifier": name,
                "startTime": seconds[0],
                "endTime": seconds[4],
                "maxResults": page_size,
            });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = logs.ok(HISTORY, &request).await;
            let records = records(&page);
            assert!(
                records.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} records, got {records:?}"
            );
            seen.extend(
                records
                    .iter()
                    .map(|record| member(case, record, "queryId")),
            );
            walked = walked.saturating_add(1);
            token = page["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                walked <= whole.len(),
                "case {case:?}: the enumeration must end, got {seen:?}"
            );
        }
        assert_eq!(
            walked, pages,
            "case {case:?}: expected {pages} page(s), got {walked} holding {seen:?}"
        );
        assert_eq!(
            seen, whole,
            "case {case:?}: the pages concatenate to the whole history, got {seen:?}"
        );
    }
}

/// What a read of the history refuses, and in which words.
#[tokio::test]
async fn a_read_of_the_history_refuses_what_it_cannot_answer() {
    let logs = Logs::new();
    seed_events(&logs).await;
    let name = "refused-reads";
    let instants = run_instants();
    let seconds = run_seconds();
    query_with_recorded_runs(&logs, name, &recorded_runs(&instants)).await;
    let (start_time, end_time) = whole_window();

    // An identifier the account holds no query under, in either form it could take.
    for (case, identifier) in [
        ("a name no query was created under", "no-such-query".to_string()),
        (
            "an ARN no query was created under",
            scheduled_query_arn(REGION, "no-such-query"),
        ),
    ] {
        let (code, message) = logs
            .refusal(
                HISTORY,
                &json!({"identifier": identifier, "startTime": start_time, "endTime": end_time}),
            )
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code} saying {message:?}"
        );
        assert_eq!(
            message,
            scheduled_query_not_found_message(&identifier),
            "case {case:?}: the refusal names the identifier the request asked for"
        );
    }

    // A window whose end precedes its start includes no instant at all.
    let (code, message) = logs
        .refusal(
            HISTORY,
            &json!({"identifier": name, "startTime": end_time, "endTime": start_time}),
        )
        .await;
    assert_eq!(
        code, VALIDATION,
        "a reversed window is refused as a validation failure, got {code} saying {message:?}"
    );
    assert_eq!(
        message, REVERSED_TIME_RANGE_MESSAGE,
        "the refusal states what a window requires"
    );

    // A token this enumeration did not mint: one that is not a token at all, one another
    // operation minted, and one minted for a different filtering of this same history.
    let other = logs
        .ok(
            "ListScheduledQueries",
            &json!({"maxResults": 1}),
        )
        .await;
    let first_page = logs
        .ok(
            HISTORY,
            &json!({
                "identifier": name,
                "startTime": seconds[0],
                "endTime": seconds[4],
                "maxResults": 1,
            }),
        )
        .await;
    let page_token = first_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a history with more records answers a token, got {first_page}"))
        .to_string();
    let cases: Vec<(&str, Value)> = vec![
        ("a token that is not one at all", json!("not-a-token")),
        (
            "a token another operation minted",
            other["nextToken"].clone(),
        ),
        ("a token of a different filtering", json!(page_token)),
    ];
    for (case, token) in cases {
        let token = match token.as_str() {
            Some(token) => token.to_string(),
            // `ListScheduledQueries` answers no token when it has no further page; such a row
            // has no token to re-present.
            None => continue,
        };
        let (code, message) = logs
            .refusal(
                HISTORY,
                &json!({
                    "identifier": name,
                    // A window other than the one the token was minted for, so a token of this
                    // operation is refused as the position in another enumeration it is.
                    "startTime": start_time,
                    "endTime": end_time,
                    "nextToken": token,
                }),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {code} saying {message:?}"
        );
        assert_eq!(
            message,
            service_com_amazonaws_cloudwatchlogs_logs_20140328::state::INVALID_NEXT_TOKEN_MESSAGE,
            "case {case:?}: the refusal states the one sentence of the class"
        );
    }
}
