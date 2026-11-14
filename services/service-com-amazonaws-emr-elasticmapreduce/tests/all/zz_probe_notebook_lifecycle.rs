// SPDX-License-Identifier: CC0-1.0

//! Stopping a notebook execution that is already UNDER WAY, end to end over the wire.
//!
//! THE RULE is the one `notebook_execution_run` pins for a stop asked for the instant the
//! execution was started: "`STOP_PENDING` indicates that the cluster has received a
//! `StopNotebookExecution` request and the stop is pending", then `STOPPING`, then
//! "`STOPPED` indicates that the execution stopped because of a `StopNotebookExecution`
//! request" (`NotebookExecution$Status` doc) — whatever the notebook was going to come to.
//! This suite drives the other side of what that rule generalises over: the caller READS a
//! status the clock has already moved the execution to and only then asks for the stop,
//! which is how a client that polls an execution before deciding to stop it behaves.
//!
//! The rows differ along what the rule generalises over: whether the notebook the execution
//! names could have been run at all (one whose cells would all have run, and one Amazon S3
//! holds nothing for).
//!
//! The harness is `notebook_execution_run`'s — Amazon EMR and Amazon S3 on one engine, every
//! notebook written through S3's own `PutObject`, and the same `walked` poll and bounds it
//! settles its own executions through; `tests/all.rs` is this binary's crate root, so each is
//! reached through `crate::notebook_execution_run` rather than copied.

use std::time::Instant;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::notebook_execution_run::{
    code_cell, described, emr_ok, emulator, notebook, notebook_cluster, s3_ok, walked, BUCKET,
    DEADLINE, NOTEBOOK_ROLE, POLL_INTERVAL,
};

/// The status this execution is in once the clock has moved it off `START_PENDING`, which is
/// what the caller below reads before it asks for the stop.
async fn under_way(router: &MultiServiceRouter, label: &str, id: &str) -> String {
    let began = Instant::now();
    loop {
        let execution = described(router, id).await;
        let status = execution["Status"].as_str().unwrap_or_default().to_string();
        if status != "START_PENDING" {
            return status;
        }
        assert!(
            began.elapsed() < DEADLINE,
            "case {label:?}: expected the execution to leave START_PENDING, got {execution}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn an_execution_stopped_after_it_has_begun_reaches_stopped() {
    let router = emulator();
    let cluster_id = notebook_cluster(&router).await;
    s3_ok(&router, "bucket", &format!("/{BUCKET}"), &[]).await;

    let rows: Vec<(&str, Option<Vec<u8>>)> = vec![
        (
            "a notebook whose cells would all have run",
            Some(notebook(json!([code_cell("total = sum(range(10))")]), None)),
        ),
        ("a notebook Amazon S3 holds nothing for", None),
    ];

    for (index, (label, object)) in rows.iter().enumerate() {
        let key = format!("stopped-under-way-{index}.ipynb");
        if let Some(object) = object.as_ref() {
            s3_ok(&router, label, &format!("/{BUCKET}/{key}"), object).await;
        }
        let answer = emr_ok(
            &router,
            "StartNotebookExecution",
            json!({
                "ExecutionEngine": { "Id": cluster_id.clone(), "Type": "EMR" },
                "ServiceRole": NOTEBOOK_ROLE,
                "NotebookExecutionName": label,
                "NotebookS3Location": { "Bucket": BUCKET, "Key": key },
            }),
        )
        .await;
        let id = answer["NotebookExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: an execution id, got {answer}"))
            .to_string();

        // The status the caller reads before it decides to stop: one the clock has already
        // moved the execution to, which is not a failing one for an execution that has only
        // just begun.
        let reading = under_way(&router, label, &id).await;
        assert!(
            !matches!(reading.as_str(), "FAILED" | "FAILING"),
            "case {label:?}: an execution that has only just begun is not failing, got \
             {reading}"
        );
        emr_ok(
            &router,
            "StopNotebookExecution",
            json!({ "NotebookExecutionId": id.clone() }),
        )
        .await;

        let (seen, execution) = walked(label, &router, &id).await;
        assert_eq!(
            execution["Status"].as_str(),
            Some("STOPPED"),
            "case {label:?}: an execution stopped from {reading} reaches STOPPED, saw {seen:?}"
        );
        assert_eq!(
            execution["NotebookExecutionId"].as_str(),
            Some(id.as_str()),
            "case {label:?}: the stopped execution is the one that was started, got {execution}"
        );
        for stopping in ["STOP_PENDING", "STOPPING"] {
            assert!(
                seen.iter().any(|status| status == stopping),
                "case {label:?}: the stop walk passes through {stopping}, saw {seen:?}"
            );
        }
    }
}
