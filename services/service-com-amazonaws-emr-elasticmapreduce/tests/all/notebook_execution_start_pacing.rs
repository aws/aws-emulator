// SPDX-License-Identifier: CC0-1.0

//! Several notebook executions started on ONE cluster, each read to the end its own
//! notebook answered, end to end over the wire.
//!
//! THE RULE: `StartNotebookExecution` answers as soon as the cluster has RECEIVED the
//! request — "`START_PENDING` indicates that the cluster has received the execution request
//! but execution has not begun" (`NotebookExecution$Status` doc) — so the notebook itself is
//! run after that answer rather than inside it: a client lines several executions up on one
//! cluster, and each then advances through the statuses the model names ("`STARTING` …
//! starting on the cluster", "`RUNNING` … being processed by the cluster") and settles in the
//! end its own run answered. `ListNotebookExecutions` then PARTITIONS the executions of that
//! engine by `Status` — each one is reported under the status it reached and under no other —
//! and two consecutive listings of the same filter report the same order.
//!
//! The rows span what the rule generalises over: what the notebook comes to (every cell ran,
//! a cell raised, the run was stopped by its own bounds, nothing could be fetched to run),
//! how many cells it has (one, several sharing a namespace), and how long its run takes (a
//! cell that computes at once, a cell that never stops). The status filter is asserted over
//! every value `com.amazonaws.emr#NotebookExecutionStatus` declares, so the statuses no
//! execution of the engine is in select none of them.
//!
//! Nothing here reaches into the service's state struct: every execution is read back
//! through `DescribeNotebookExecution` and `ListNotebookExecutions`, and every notebook is
//! written through Amazon S3's own `CreateBucket`/`PutObject`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::NOTEBOOK_NOT_EXECUTED_REASON;
use smithy_server_runtime::MultiServiceRouter;

use crate::notebook_execution_run::{
    code_cell, described, emr_ok, emulator, notebook, notebook_cluster, s3_ok, BUCKET,
    NOTEBOOK_ROLE, TERMINAL,
};

/// How long the walks of every execution of one test are followed before the test gives up.
///
/// Deliberately far longer than the walks take: the test waits for the CONDITION — every
/// execution terminal — and this only bounds a walk that never arrives.
const DEADLINE: Duration = Duration::from_secs(120);

/// How long a pass of the poll loop sleeps before reading the executions again: short enough
/// that the phases of a progression are observed rather than jumped over.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Every value `com.amazonaws.emr#NotebookExecutionStatus` declares
/// (`smithy_query(query="shape", shape="NotebookExecutionStatus")`), which is the catalog
/// `ListNotebookExecutions$Status` filters on.
const NOTEBOOK_EXECUTION_STATUSES: [&str; 10] = [
    "START_PENDING",
    "STARTING",
    "RUNNING",
    "FINISHING",
    "FINISHED",
    "FAILING",
    "FAILED",
    "STOP_PENDING",
    "STOPPING",
    "STOPPED",
];

/// The status a just-recorded execution carries, the one the model gives for a request the
/// cluster has only received.
const START_PENDING: &str = "START_PENDING";

/// The status an execution is in while its notebook runs.
const RUNNING: &str = "RUNNING";

/// The walk of an execution whose notebook ran without raising: "`FINISHING` indicates that
/// execution processing is in the final stages", "`FINISHED` indicates that the execution has
/// completed without error".
const FINISHING_WALK: &[&str] = &["START_PENDING", "STARTING", "RUNNING", "FINISHING", "FINISHED"];

/// The walk of an execution that failed: "`FAILING` indicates that the execution is failing
/// and will not finish successfully", "`FAILED` indicates that the execution failed".
const FAILING_WALK: &[&str] = &["START_PENDING", "STARTING", "RUNNING", "FAILING", "FAILED"];

/// One row: the notebook object written for this execution (when one is written), the walk it
/// must reach its end through, and what its reason must say.
struct Row {
    label: &'static str,
    /// The object written under this row's own key, or `None` for a key the bucket holds
    /// nothing under.
    object: Option<Vec<u8>>,
    walk: &'static [&'static str],
    /// A part of `LastStateChangeReason` the ended execution must carry, or `None` where it
    /// must carry none at all.
    reason: Option<String>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one cell that computes",
            object: Some(notebook(json!([code_cell("print(1 + 1)")]), None)),
            walk: FINISHING_WALK,
            reason: None,
        },
        Row {
            label: "several cells sharing one namespace",
            object: Some(notebook(
                json!([
                    code_cell("answer = 6 * 7"),
                    code_cell("assert answer == 42, answer"),
                ]),
                None,
            )),
            walk: FINISHING_WALK,
            reason: None,
        },
        Row {
            label: "a cell that raises",
            object: Some(notebook(
                json!([code_cell("raise ValueError('the notebook said no')")]),
                None,
            )),
            walk: FAILING_WALK,
            reason: Some("ValueError".to_string()),
        },
        Row {
            label: "a cell that never stops, which the run's own bounds stop",
            object: Some(notebook(
                json!([code_cell("total = 0\nwhile True:\n    total += 1")]),
                None,
            )),
            walk: FAILING_WALK,
            reason: Some("The notebook was stopped".to_string()),
        },
        Row {
            label: "a key the bucket holds nothing under",
            object: None,
            walk: FAILING_WALK,
            reason: Some(NOTEBOOK_NOT_EXECUTED_REASON.to_string()),
        },
    ]
}

/// The key one row's notebook is written under.
fn key_of(index: usize) -> String {
    format!("paced-{index}.ipynb")
}

/// `StartNotebookExecution` for one row, answering the id it minted.
async fn start(router: &MultiServiceRouter, label: &str, engine_id: &str, key: &str) -> String {
    let answer = emr_ok(
        router,
        "StartNotebookExecution",
        json!({
            "ExecutionEngine": { "Id": engine_id, "Type": "EMR" },
            "ServiceRole": NOTEBOOK_ROLE,
            "NotebookExecutionName": label,
            "NotebookS3Location": { "Bucket": BUCKET, "Key": key },
        }),
    )
    .await;
    answer["NotebookExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: StartNotebookExecution answers an id, got {answer}"))
        .to_string()
}

/// Follow the walks of every execution in `ids` TOGETHER — one read of each per pass — until
/// all of them are terminal, answering the statuses each one was seen in, in order and
/// without repeats.
///
/// Together rather than one after another, because that is what the rule is about: the
/// executions were all started before any of them was read, so each one's walk is observed
/// from the instant it was started.
async fn walked(router: &MultiServiceRouter, labels: &[&str], ids: &[String]) -> Vec<Vec<String>> {
    let started = Instant::now();
    let mut seen: Vec<Vec<String>> = vec![Vec::new(); ids.len()];
    loop {
        let mut still_walking = false;
        for (index, id) in ids.iter().enumerate() {
            let execution = described(router, id).await;
            let status = execution["Status"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: an execution reports a Status, got {execution}",
                        labels[index]
                    )
                })
                .to_string();
            if seen[index].last() != Some(&status) {
                seen[index].push(status.clone());
            }
            if !TERMINAL.contains(&status.as_str()) {
                still_walking = true;
            }
        }
        if !still_walking {
            return seen;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "every execution must reach a terminal status, saw {seen:?} for {labels:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The execution ids `ListNotebookExecutions` reports for `filter`, in the order it reports
/// them.
async fn listed(router: &MultiServiceRouter, filter: Value) -> Vec<String> {
    let page = emr_ok(router, "ListNotebookExecutions", filter).await;
    page["NotebookExecutions"]
        .as_array()
        .map(|executions| {
            executions
                .iter()
                .filter_map(|execution| {
                    execution["NotebookExecutionId"]
                        .as_str()
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// That `seen` walked `walk`: every status it reported belongs to that progression, in that
/// order, and it ended where the progression ends.
fn assert_walks(label: &str, seen: &[String], walk: &[&str]) {
    let mut reached = 0usize;
    for status in seen {
        let position = walk
            .iter()
            .position(|step| step == status)
            .unwrap_or_else(|| {
                panic!("case {label:?}: {status:?} is not one of {walk:?}, saw {seen:?}")
            });
        assert!(
            position >= reached,
            "case {label:?}: the walk went backwards to {status:?}, saw {seen:?}"
        );
        reached = position;
    }
    assert_eq!(
        seen.first().map(String::as_str),
        Some(START_PENDING),
        "case {label:?}: an execution begins where the request was received, saw {seen:?}"
    );
    assert!(
        seen.iter().any(|status| status == RUNNING),
        "case {label:?}: an execution is processed by the cluster before it ends, saw {seen:?}"
    );
    assert_eq!(
        seen.last().map(String::as_str),
        walk.last().copied(),
        "case {label:?}: expected the walk to end at {:?}, saw {seen:?}",
        walk.last()
    );
}

#[tokio::test]
async fn executions_started_one_after_another_each_walk_to_the_end_their_notebook_answered() {
    let router = emulator();
    let engine_id = notebook_cluster(&router).await;
    s3_ok(&router, "bucket", &format!("/{BUCKET}"), &[]).await;

    let rows = rows();
    for (index, row) in rows.iter().enumerate() {
        if let Some(object) = row.object.as_ref() {
            s3_ok(
                &router,
                row.label,
                &format!("/{BUCKET}/{}", key_of(index)),
                object,
            )
            .await;
        }
    }

    // EVERY execution is started before any of them is read: the start call answers for a
    // request the cluster has received, so nothing of one execution's notebook stands between
    // a client and the start of the next.
    let mut ids: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let id = start(&router, row.label, &engine_id, &key_of(index)).await;
        let recorded = described(&router, &id).await;
        assert_eq!(
            recorded["Status"].as_str(),
            Some(START_PENDING),
            "case {:?}: the cluster has received the request and the execution has not \
             begun, got {recorded}",
            row.label
        );
        assert!(
            recorded["EndTime"].is_null(),
            "case {:?}: an execution that has not ended reports no EndTime, got {recorded}",
            row.label
        );
        assert!(
            recorded["LastStateChangeReason"].is_null(),
            "case {:?}: an execution that has not ended has nothing to report, got {recorded}",
            row.label
        );
        ids.push(id);
    }

    let labels: Vec<&str> = rows.iter().map(|row| row.label).collect();
    let seen = walked(&router, &labels, &ids).await;

    for (index, row) in rows.iter().enumerate() {
        let label = row.label;
        assert_walks(label, &seen[index], row.walk);

        let ended = described(&router, &ids[index]).await;
        let reason = ended["LastStateChangeReason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match row.reason.as_deref() {
            Some(expected) => assert!(
                reason.contains(expected),
                "case {label:?}: expected a reason carrying {expected:?}, got {reason:?}"
            ),
            None => assert!(
                reason.is_empty(),
                "case {label:?}: an execution that completed without error reports no \
                 reason, got {reason:?}"
            ),
        }
        assert!(
            ended["EndTime"].as_f64().is_some(),
            "case {label:?}: an ended execution reports when it ended, got {ended}"
        );
        // A read reports; it does not move the execution on.
        let again = described(&router, &ids[index]).await;
        assert_eq!(
            again, ended,
            "case {label:?}: a second read reports the same execution, got {again}"
        );
    }

    // An execution of ANOTHER engine, so the listings below are asserted against a store
    // that holds more than this cluster's executions.
    let other_engine = notebook_cluster(&router).await;
    let other = start(&router, "another engine", &other_engine, &key_of(0)).await;

    // The status filter partitions this engine's executions: each one is reported under the
    // status it reached and under no other, including the statuses none of them is in.
    for status in NOTEBOOK_EXECUTION_STATUSES {
        let selected = listed(
            &router,
            json!({ "ExecutionEngineId": engine_id.clone(), "Status": status }),
        )
        .await;
        for (index, row) in rows.iter().enumerate() {
            let expected = row.walk.last() == Some(&status);
            assert_eq!(
                selected.contains(&ids[index]),
                expected,
                "case {:?}: the {status} filter must select it only under {:?}, got \
                 {selected:?}",
                row.label,
                row.walk.last()
            );
        }
        assert!(
            !selected.contains(&other),
            "case {status:?}: an execution of another engine is not this engine's, got \
             {selected:?}"
        );
    }

    // Two consecutive listings of the same filter report the same executions in the same
    // order, and that order is every execution of the engine, each exactly once.
    let first = listed(&router, json!({ "ExecutionEngineId": engine_id.clone() })).await;
    let second = listed(&router, json!({ "ExecutionEngineId": engine_id.clone() })).await;
    assert_eq!(
        first, second,
        "two consecutive listings of one engine report the same order, got {first:?} then \
         {second:?}"
    );
    for id in &ids {
        assert_eq!(
            first.iter().filter(|listed| *listed == id).count(),
            1,
            "the listing reports {id} exactly once, got {first:?}"
        );
    }
    assert_eq!(
        first.len(),
        ids.len(),
        "the listing reports this engine's executions and no others, got {first:?} against \
         {ids:?}"
    );
}
