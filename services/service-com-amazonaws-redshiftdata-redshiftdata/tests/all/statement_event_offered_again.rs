// SPDX-License-Identifier: CC0-1.0

//! An announcement the bus REFUSED is offered again by this service, so the
//! submission that asked for it is told how its statement ended as soon as the bus
//! takes announcements again — without another statement having to be submitted.
//!
//! The RULE asserted here: `WithEvent` is "a value that indicates whether to send
//! an event to the Amazon EventBridge event bus after the SQL statement runs"
//! (`smithy_query(query="doc",shape="ExecuteStatementInput",member="WithEvent")`),
//! so a submission that asked for one is OWED one until the bus takes it. The
//! sibling suite `statement_status_change_events` pins what the bus is handed when
//! it takes the first offer, and `statement_event_still_owed` pins that a refused
//! announcement is kept rather than discarded; THIS suite pins the half between
//! them: the keeping is not passive. The service offers the owed announcement again
//! itself, so a client that submitted ONE statement — which makes no second
//! terminal transition, and so no second drain — still receives it.
//!
//! What makes the bus refuse here is the bus's OWN resource policy: an explicit
//! deny of `events:PutEvents` on the default bus, installed with `PutPermission`
//! and then withdrawn with `RemovePermission`, which is the configuration fault a
//! client really recovers from. Nothing in this suite touches this service's queue
//! of owed announcements directly: the delivery is read off the SQS queue of a rule
//! that matches the announcement, and the count of what is still owed is only ever
//! read.
//!
//! The rows span what the rule generalises over: WHICH submit operation made the
//! submission (one statement, a batch) and WHICH terminal status the statement
//! reached (a statement that runs FINISHES, one the engine refuses FAILS). Each row
//! runs on its own emulator, because what is asserted is what becomes of the ONE
//! announcement a row is owed.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::emulator::Warehouses;
use crate::statement_status_change_events::{
    collect, drain, given_queue, given_rule, statement_event_pattern, wait_for_terminal,
    POLL_BUDGET, POLL_INTERVAL,
};

/// The warehouse every row runs on, the database it holds and the user a statement
/// authenticates as.
const CLUSTER: &str = "refused-event-cluster";
const DATABASE: &str = "devdb";
const DB_USER: &str = "adminuser";

/// A statement the engine runs to completion, and one it refuses.
const FINISHING_SQL: &str = "SELECT 1";
const FAILING_SQL: &str = "SELECT * FROM a_table_no_statement_ever_created";

/// The bus every statement event of this service is published onto, and the
/// statement id of the deny installed on it.
const DEFAULT_BUS: &str = "default";
const DENY_SID: &str = "DenyEveryPublication";

/// How long the queue is watched while the bus is refusing. A bound on an ABSENCE,
/// so it is the one span a slow host may spend here without changing the verdict:
/// it is several times the pacing of the first further offers, so an offer the bus
/// refused really was made inside it.
const ABSENCE_WINDOW: Duration = Duration::from_secs(2);

/// One row: the submission made, and the terminal status its statement reaches.
struct Row {
    case: &'static str,
    /// The submit operation: `ExecuteStatement` or `BatchExecuteStatement`.
    operation: &'static str,
    /// The statements submitted: exactly one for `ExecuteStatement`.
    sqls: &'static [&'static str],
    /// The terminal status the statement must reach.
    status: &'static str,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a statement that ran",
        operation: "ExecuteStatement",
        sqls: &[FINISHING_SQL],
        status: "FINISHED",
    },
    Row {
        case: "a statement the engine refused",
        operation: "ExecuteStatement",
        sqls: &[FAILING_SQL],
        status: "FAILED",
    },
    Row {
        case: "a batch that ran",
        operation: "BatchExecuteStatement",
        sqls: &[FINISHING_SQL, "SELECT 2"],
        status: "FINISHED",
    },
];

/// The submit request one row makes.
fn submit_request(row: &Row) -> Value {
    let mut request = json!({
        "ClusterIdentifier": CLUSTER,
        "Database": DATABASE,
        "DbUser": DB_USER,
        "WithEvent": true,
        "StatementName": row.case,
    });
    if row.operation == "BatchExecuteStatement" {
        request["Sqls"] = json!(row.sqls);
    } else {
        request["Sql"] = json!(row.sqls[0]);
    }
    request
}

/// The resource policy that refuses every publication onto the default bus,
/// whoever makes it: an explicit deny of the one action an event bus's policy is
/// about.
fn deny_every_publication() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": DENY_SID,
            "Effect": "Deny",
            "Principal": "*",
            "Action": "events:PutEvents",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// Look at this service's count of the announcements still owed until it is
/// `expected`, and answer what was last read.
///
/// Read by looking again rather than once: a drain takes an announcement out before
/// it offers it to the bus and gives it back when the bus refuses, so between those
/// two instants nothing is owed yet. The deadline is generous and the COUNT is what
/// is asserted, so a count that never settles fails on the count it was stuck at
/// and never on how fast the host ran.
async fn owed(emulator: &Warehouses, expected: usize) -> usize {
    let started = Instant::now();
    loop {
        let queued = emulator.store().lock().pending_statement_event_count();
        if queued == expected || started.elapsed() >= POLL_BUDGET {
            return queued;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: a submission that asked for an event whose announcement the bus refused is
/// announced once the bus takes it — offered again by this service, with no further
/// statement submitted and nothing lost in between.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_announcement_the_bus_refused_is_offered_again_until_it_is_taken() {
    for row in ROWS {
        let case = row.case;
        let emulator = Warehouses::new();
        emulator
            .create_cluster_with_database(CLUSTER, Some(DATABASE))
            .await;
        let (url, queue_arn) = given_queue(&emulator, "refused-statement-events").await;
        given_rule(
            &emulator,
            "refused-statement-status-changes",
            &statement_event_pattern(),
            &[queue_arn.as_str()],
        )
        .await;

        // From here the bus takes no publication at all, this service's own
        // included.
        emulator
            .events_ok(
                "PutPermission",
                &json!({"EventBusName": DEFAULT_BUS, "Policy": deny_every_publication()}),
            )
            .await;

        let answered = emulator.data_ok(row.operation, &submit_request(row)).await;
        let id = answered["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {answered}"))
            .to_string();

        // The statement's own record is what the Data API reports whatever came of
        // the announcement: a publication the bus refused changes no answer.
        let record = wait_for_terminal(&emulator, case, &id).await;
        assert_eq!(
            record["Status"],
            json!(row.status),
            "case {case:?}: expected the statement to end {}, got {record}",
            row.status
        );

        // While the bus refuses, the announcement is OWED and nothing reaches the
        // rule's queue — however many times it is offered.
        let queued = owed(&emulator, 1).await;
        assert_eq!(
            queued, 1,
            "case {case:?}: the announcement the bus refused must still be owed, got {queued} \
             queued"
        );
        let watching = Instant::now();
        while watching.elapsed() < ABSENCE_WINDOW {
            let leaked = drain(&emulator, &url).await;
            assert!(
                leaked.is_empty(),
                "case {case:?}: a bus that refuses the publication delivers nothing, got \
                 {leaked:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // The bus takes publications again — and NOTHING else happens: no further
        // statement is submitted, so the only thing that can publish the owed
        // announcement is this service offering it again.
        emulator
            .events_ok(
                "RemovePermission",
                &json!({"EventBusName": DEFAULT_BUS, "RemoveAllPermissions": true}),
            )
            .await;

        let delivered = collect(&emulator, &url, 1).await;
        let mine: Vec<&Value> = delivered
            .iter()
            .filter(|envelope| envelope.pointer("/detail/statementId") == Some(&json!(&id)))
            .collect();
        assert_eq!(
            mine.len(),
            1,
            "case {case:?}: expected exactly one announcement of {id} once the bus took \
             publications again, got {mine:?} among {delivered:?}"
        );
        assert_eq!(
            mine[0].pointer("/detail/state"),
            Some(&json!(row.status)),
            "case {case:?}: expected the announced state {}, got {}",
            row.status,
            mine[0]
        );

        // And nothing is owed any more: what was kept has been published.
        let queued = owed(&emulator, 0).await;
        assert_eq!(
            queued, 0,
            "case {case:?}: every kept announcement must have been published, got {queued} \
             waiting"
        );
    }
}
