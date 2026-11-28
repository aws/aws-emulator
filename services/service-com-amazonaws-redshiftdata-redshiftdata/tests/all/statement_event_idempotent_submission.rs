// SPDX-License-Identifier: CC0-1.0

//! A submission that carries an idempotency token is announced exactly ONCE,
//! however many times the same token is submitted.
//!
//! The RULE asserted here: `ClientToken` is "a unique, case-sensitive identifier
//! that you provide to ensure the idempotency of the request"
//! (`smithy_query(query="doc",shape="ExecuteStatementInput",member="ClientToken")`),
//! so every submission of one token names ONE statement — and `WithEvent` is "a
//! value that indicates whether to send an event to the Amazon EventBridge event
//! bus after the SQL statement runs"
//! (`smithy_query(query="doc",shape="ExecuteStatementInput",member="WithEvent")`),
//! so that one statement has its terminal status change published once. A replayed
//! submission therefore adds no second announcement and loses the first: the status
//! change belongs to the statement, not to the request that asked for it again.
//!
//! Why this is the shape every real caller sends: the member carries
//! `@idempotencyToken`, so an AWS SDK and the AWS CLI FILL IT IN for a caller that
//! named none — a submission over the wire practically always carries one, while a
//! suite that builds the request by hand carries one only if it says so. The rows
//! here therefore span whether the submission carried a token at all, whether it
//! asked for an event, and how many times the token was submitted (once, twice,
//! and once more after the statement had already stopped and been announced).
//!
//! EventBridge and Amazon SQS are mounted BESIDE this service on the same engine,
//! so what is read back is a real delivery through the real bus rather than this
//! service's own queue.

use serde_json::{json, Value};

use crate::emulator::Warehouses;
use crate::statement_status_change_events::{
    collect, drain, given_queue, given_rule, statement_event_pattern, wait_for_terminal,
};

/// The warehouse every row runs on, the database it holds and the user a
/// statement authenticates as.
const CLUSTER: &str = "idempotent-event-cluster";
const DATABASE: &str = "devdb";
const DB_USER: &str = "adminuser";

/// A statement the engine runs to completion.
const SQL: &str = "SELECT 1";

/// How many times one row submits its token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Submits {
    /// Submitted once, as a caller that never retried.
    Once,
    /// Submitted twice in a row, as a caller whose first answer was lost.
    Twice,
    /// Submitted once, and once MORE after the statement had stopped — the replay
    /// that reaches a statement whose announcement has already been published.
    AgainAfterItStopped,
}

/// One row: the submission made, and whether it must be announced.
struct Row {
    case: &'static str,
    /// The `ClientToken` the request carries, or `None` for a request that omits
    /// the member — the shape a hand-built request has and an SDK's has not.
    client_token: Option<&'static str>,
    /// The `WithEvent` the request carries, or `None` for a request that omits it.
    with_event: Option<bool>,
    submits: Submits,
    /// Whether this submission's status change must be announced on the bus.
    announced: bool,
}

/// The table. The rows that must announce NOTHING come first, so a later row's
/// delivery is a drain of the same queue: a quiet row that had announced
/// something would be read by one of them.
const ROWS: &[Row] = &[
    Row {
        case: "a token submitted once, asking for no event",
        client_token: Some("quiet-token"),
        with_event: Some(false),
        submits: Submits::Once,
        announced: false,
    },
    Row {
        case: "a token submitted twice, omitting WithEvent",
        client_token: Some("quiet-replayed-token"),
        with_event: None,
        submits: Submits::Twice,
        announced: false,
    },
    Row {
        case: "a token submitted once",
        client_token: Some("loud-token"),
        with_event: Some(true),
        submits: Submits::Once,
        announced: true,
    },
    Row {
        case: "a token submitted twice in a row",
        client_token: Some("loud-replayed-token"),
        with_event: Some(true),
        submits: Submits::Twice,
        announced: true,
    },
    Row {
        case: "a token submitted again after its statement stopped",
        client_token: Some("loud-late-replayed-token"),
        with_event: Some(true),
        submits: Submits::AgainAfterItStopped,
        announced: true,
    },
    Row {
        case: "a submission carrying no token at all",
        client_token: None,
        with_event: Some(true),
        submits: Submits::Once,
        announced: true,
    },
];

/// The submit request one row makes.
fn submit_request(row: &Row) -> Value {
    let mut request = json!({
        "ClusterIdentifier": CLUSTER,
        "Database": DATABASE,
        "DbUser": DB_USER,
        "Sql": SQL,
        "StatementName": row.case,
    });
    if let Some(token) = row.client_token {
        request["ClientToken"] = json!(token);
    }
    if let Some(with_event) = row.with_event {
        request["WithEvent"] = json!(with_event);
    }
    request
}

/// Submit `row` and answer the `Id` every submission of it was answered with —
/// which must be ONE id, because one token names one statement.
async fn submit(emulator: &Warehouses, row: &Row) -> String {
    let case = row.case;
    let first = emulator.data_ok("ExecuteStatement", &submit_request(row)).await;
    let id = first["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Id in {first}"))
        .to_string();
    if row.submits == Submits::Twice {
        let replayed = emulator.data_ok("ExecuteStatement", &submit_request(row)).await;
        assert_eq!(
            replayed["Id"],
            json!(&id),
            "case {case:?}: one token names one statement, got {replayed} after {first}"
        );
    }
    id
}

/// RULE: one idempotency token names one statement, so a submission that asked for
/// an event is announced exactly once however often that token is submitted — and a
/// submission that asked for none is announced not at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replayed_submission_is_announced_exactly_once() {
    let emulator = Warehouses::new();
    emulator
        .create_cluster_with_database(CLUSTER, Some(DATABASE))
        .await;
    let (url, queue_arn) = given_queue(&emulator, "idempotent-statement-events").await;
    given_rule(
        &emulator,
        "idempotent-statement-status-changes",
        &statement_event_pattern(),
        &[queue_arn.as_str()],
    )
    .await;

    let mut submitted: Vec<(&Row, String)> = Vec::new();
    for row in ROWS {
        let id = submit(&emulator, row).await;
        let record = wait_for_terminal(&emulator, row.case, &id).await;
        assert_eq!(
            record["Status"],
            json!("FINISHED"),
            "case {:?}: expected the statement to finish, got {record}",
            row.case
        );
        // The replay that arrives once the statement has already stopped — and so
        // once its announcement has already been published — must still answer
        // that one statement and announce nothing further.
        if row.submits == Submits::AgainAfterItStopped {
            let replayed = emulator
                .data_ok("ExecuteStatement", &submit_request(row))
                .await;
            assert_eq!(
                replayed["Id"],
                json!(&id),
                "case {:?}: a replay after the statement stopped names that statement, got \
                 {replayed}",
                row.case
            );
        }
        submitted.push((row, id));
    }

    let wanted = ROWS.iter().filter(|row| row.announced).count();
    let delivered = collect(&emulator, &url, wanted).await;
    assert_eq!(
        delivered.len(),
        wanted,
        "expected {wanted} announcement(s) on the rule's queue, got {delivered:?}"
    );

    for (row, id) in &submitted {
        let announcements: Vec<&Value> = delivered
            .iter()
            .filter(|envelope| envelope.pointer("/detail/statementId") == Some(&json!(id)))
            .collect();
        let expected = usize::from(row.announced);
        assert_eq!(
            announcements.len(),
            expected,
            "case {:?}: expected {expected} announcement(s) of {id}, got {announcements:?} among \
             {delivered:?}",
            row.case
        );
    }

    // Nothing arrives afterwards either: a replayed submission adds no delayed
    // second copy of an announcement already made.
    let later = drain(&emulator, &url).await;
    assert!(
        later.is_empty(),
        "a replayed submission announces nothing further, got {later:?}"
    );

    // And every announcement that was owed has been published: the queue of owed
    // status changes is empty.
    let queued = emulator.store().lock().pending_statement_event_count();
    assert_eq!(
        queued, 0,
        "every queued status change must have been published, got {queued} waiting"
    );
}
