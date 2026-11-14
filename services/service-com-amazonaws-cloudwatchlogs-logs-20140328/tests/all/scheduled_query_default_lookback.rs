// SPDX-License-Identifier: CC0-1.0

//! The window a scheduled query that states NO `startTimeOffset` reads.
//!
//! The rule under test: a run of a query that states no lookback of its own reads back over one
//! whole PERIOD of that query's schedule — the gap between two instants the schedule expression
//! names, evaluated in the query's own timezone — ending at the instant the run was triggered.
//! The period is a property of the schedule alone, so it is the same whether or not the run that
//! reads it fell on the schedule's grid: the first run of a query is made due a fixed delay
//! after the query was created, and it reads a whole period too rather than whatever remained
//! until the expression's next instant.
//!
//! The table spans both forms of schedule expression (a rate and a cron), two periods (an hour
//! and a day) and a cron evaluated in a named timezone. Each row proves the width of the window
//! through the rows the run produced: an ERROR event inside one period of the trigger is read,
//! and one just outside it is not — so a window narrower or wider than a period fails the row.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! that is not computed from the shared store cannot pass.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{batch, group_with_stream, now_ms, Logs};

/// The operations under test, and the one a run's rows are read through.
const HISTORY: &str = "GetScheduledQueryHistory";
const CREATE: &str = "CreateScheduledQuery";
const RESULTS: &str = "GetQueryResults";

/// The IAM role every case executes under: "The ARN of the IAM role used to execute the query
/// and deliver results."
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// The pipeline every case runs: the ERROR events of the window, as rows.
const QUERY_STRING: &str = "fields @timestamp, @message | filter @message like /ERROR/";

/// The message of the event each row places INSIDE one period of its run, and of the one it
/// places just outside it. Both match the pipeline above, so which of them the run reports is
/// decided by the window alone.
const INSIDE: &str = "ERROR inside one period of the run";
const OUTSIDE: &str = "ERROR older than one period of the run";

/// How far inside and outside the period each row's two events fall.
///
/// The run itself is triggered a short, fixed delay after the query is created, which is after
/// the events are stamped — so the margin also absorbs however long this test takes to get from
/// stamping them to the create, and is generous on purpose: a loaded host reaches the same
/// verdict as an idle one.
const MARGIN_SECS: i64 = 600;

/// How long a read of the history is given to report the run the clock has made due. Generous
/// on purpose: the test waits for the CONDITION — a record being there — and never on a fixed
/// span.
const HISTORY_DEADLINE: Duration = Duration::from_secs(180);

/// How long to wait between two looks while polling for one.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One row: how the query is scheduled, and the period that schedule names.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `scheduleExpression` the query is created with.
    expression: &'static str,
    /// Its `timezone`, when the row names one.
    timezone: Option<&'static str>,
    /// The gap, in seconds, between two instants `expression` names — the period the run must
    /// read back over when the query states no lookback of its own.
    period_secs: i64,
}

/// The rows: both forms of expression, two periods, and a cron evaluated in a named timezone.
///
/// The named zone is one that keeps a fixed offset all year, so the gap between two instants a
/// daily cron names in it is a day whatever date the test runs on.
const ROWS: &[Row] = &[
    Row {
        case: "a rate of one hour",
        expression: "rate(1 hour)",
        timezone: None,
        period_secs: 3_600,
    },
    Row {
        case: "a rate of one day",
        expression: "rate(1 day)",
        timezone: None,
        period_secs: 86_400,
    },
    Row {
        case: "a cron naming one instant each hour",
        expression: "cron(0 * * * ? *)",
        timezone: None,
        period_secs: 3_600,
    },
    Row {
        case: "a cron naming one instant each day",
        expression: "cron(0 3 * * ? *)",
        timezone: None,
        period_secs: 86_400,
    },
    Row {
        case: "a cron naming one instant each day in a named timezone",
        expression: "cron(0 3 * * ? *)",
        timezone: Some("Asia/Kolkata"),
        period_secs: 86_400,
    },
];

/// The `triggerHistory` of one read of `name`'s history over every instant its runs can fall at.
async fn history(logs: &Logs, name: &str) -> Vec<Value> {
    let page = logs
        .ok(
            HISTORY,
            &json!({
                "identifier": name,
                "startTime": 0,
                "endTime": now_ms() / 1_000 + 24 * 60 * 60,
            }),
        )
        .await;
    page["triggerHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("a history page carries triggerHistory, got {page}"))
        .clone()
}

/// Poll `name`'s history until it reports a run, answering the OLDEST run it reports.
///
/// The runs are owed by the clock, so the condition — not a span — is what is waited for. The
/// oldest record is the first run, the one this suite is about: a schedule whose next instant
/// happens to fall while the test runs records a later run as well, and that one is not it.
async fn first_run(logs: &Logs, case: &str, name: &str) -> Value {
    let deadline = Instant::now() + HISTORY_DEADLINE;
    loop {
        // The history is reported newest run first, so the oldest is the last entry.
        if let Some(oldest) = history(logs, name).await.last() {
            return oldest.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the schedule to have made a run due in the history of {name}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `@message` value of every row of one run's results.
fn messages(case: &str, results: &Value) -> Vec<String> {
    results["results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the results are rows, got {results}"))
        .iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter(|field| field["field"].as_str() == Some("@message"))
        .map(|field| field["value"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A run of a query that states no lookback reads back over one whole period of its own
/// schedule: the event one period inside the trigger is read and the one just outside it is not.
#[tokio::test]
async fn a_run_stating_no_lookback_reads_one_whole_period_of_its_schedule() {
    for Row {
        case,
        expression,
        timezone,
        period_secs,
    } in ROWS
    {
        let logs = Logs::new();
        let group = format!("/aws/emulator/default-lookback/{period_secs}");
        let name = format!("errors-over-{period_secs}-seconds");
        group_with_stream(&logs, &group, "runs", None).await;

        // Stamped from the clock the run's window is measured against: one event inside one
        // period of it, one older than a period. Both are ERROR events, so the pipeline selects
        // whichever of them the window holds.
        let stamped_at = now_ms();
        let events: [(i64, &str); 2] = [
            (stamped_at - (period_secs + MARGIN_SECS) * 1_000, OUTSIDE),
            (stamped_at - (period_secs - MARGIN_SECS) * 1_000, INSIDE),
        ];
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": "runs",
                "logEvents": batch(&events),
            }),
        )
        .await;

        let mut request = json!({
            "name": name,
            "queryLanguage": "CWLI",
            "queryString": QUERY_STRING,
            "scheduleExpression": expression,
            "executionRoleArn": EXECUTION_ROLE,
            "logGroupIdentifiers": [group],
        });
        if let Some(timezone) = timezone {
            request["timezone"] = json!(timezone);
        }
        logs.ok(CREATE, &request).await;

        let record = first_run(&logs, case, &name).await;
        assert_eq!(
            record["executionStatus"].as_str(),
            Some("Complete"),
            "case {case:?}: a run that produced its results is complete, got {record}"
        );
        let query_id = record["queryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a record carries queryId, got {record}"))
            .to_string();

        let results = logs.ok(RESULTS, &json!({"queryId": query_id})).await;
        assert_eq!(
            results["status"].as_str(),
            Some("Complete"),
            "case {case:?}: the run's query has run, got {results}"
        );
        assert_eq!(
            messages(case, &results),
            vec![INSIDE.to_string()],
            "case {case:?}: expected a window one period ({period_secs}s) wide, which holds the \
             event {MARGIN_SECS}s inside it and not the one {MARGIN_SECS}s outside it, got \
             {results}"
        );
    }
}
