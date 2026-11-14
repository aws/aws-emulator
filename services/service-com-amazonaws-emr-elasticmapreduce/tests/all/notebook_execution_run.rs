// SPDX-License-Identifier: CC0-1.0

//! A headless notebook execution RUNS its notebook, end to end over the wire.
//!
//! THE RULE: an execution's end is what running its notebook answered. The notebook is the
//! object the request names in Amazon S3 ("the Amazon S3 location that the notebook
//! execution input is stored in"), its code cells are the program it submits, and they are
//! executed in order against one namespace: an execution whose cells all ran leaves
//! `START_PENDING` without failing and reaches `FINISHED`, one whose cell raised reaches
//! `FAILED` with the exception the cells produced as its `LastStateChangeReason`, and one
//! whose notebook could not be fetched, read or run keeps the emulator's own not-executed
//! failure rather than reporting an outcome for work nothing performed. A caller that asks a
//! running execution to stop takes it to `STOPPED` instead.
//!
//! The rows span what the rule generalises over: whether the notebook is there at all (an
//! object, a key the bucket holds nothing under, no Amazon S3 location in the request), what
//! the object is (a notebook, bytes that are no notebook, a notebook whose kernel is not a
//! Python one), how a cell's source is spelled (one string, the list of its lines), how many
//! code cells there are (none, one, several sharing a namespace), what the cells do (compute,
//! import the standard library, reach for the host, raise), and whether the caller stops the
//! execution.
//!
//! Amazon S3 is mounted BESIDE Amazon EMR on one engine, and every notebook is written
//! through S3's own `CreateBucket`/`PutObject` — so the cells that run are the ones that
//! bucket holds rather than ones this test handed EMR.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::NOTEBOOK_NOT_EXECUTED_REASON;
use smithy_server_runtime::MultiServiceRouter;

/// The Region both services are addressed in.
const REGION: &str = "us-east-1";

/// The bucket every notebook of this suite is written to.
///
/// `pub(crate)` so a sibling suite writes its notebooks to one bucket through one name
/// rather than its own copy of this harness.
pub(crate) const BUCKET: &str = "emr-notebooks";

/// The roles a launch and an execution name.
const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";
const SERVICE_ROLE: &str = "EMR_DefaultRole";
pub(crate) const NOTEBOOK_ROLE: &str = "EMR_Notebooks_DefaultRole";

/// How long a poll waits for an execution to reach a terminal status before giving up: the
/// four phases of a finishing walk at [`NOTEBOOK_PHASE_SPAN`] each, with room to spare.
///
/// `pub(crate)` with [`walked`] below: the sibling suite that stops an execution the clock
/// has already moved on reads the same bound rather than carrying its own.
pub(crate) const DEADLINE: Duration = Duration::from_secs(40);

/// How long a poll sleeps between reads: short enough that the phases of a progression are
/// observed rather than jumped over.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The statuses an execution never leaves (`NotebookExecutionStatus`).
pub(crate) const TERMINAL: [&str; 3] = ["FINISHED", "FAILED", "STOPPED"];

/// Amazon EMR and Amazon S3 on one engine, the shape the router mounts them in.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One `awsJson1_1` Amazon EMR call.
async fn emr(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let host = format!("elasticmapreduce.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/\
                 elasticmapreduce/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// One Amazon EMR call that must be served.
pub(crate) async fn emr_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, payload) = emr(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One path-style Amazon S3 request that must be served.
pub(crate) async fn s3_ok(router: &MultiServiceRouter, case: &str, uri: &str, body: &[u8]) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("PUT")
        .uri(format!("https://{host}{uri}"))
        .header("host", host.clone())
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    assert!(
        (200..300).contains(&parts.status.as_u16()),
        "case {case:?}: S3 PUT {uri} must succeed, got {} {}",
        parts.status.as_u16(),
        String::from_utf8_lossy(&answered)
    );
}

/// A cluster that runs the applications a headless notebook execution needs, kept alive.
pub(crate) async fn notebook_cluster(router: &MultiServiceRouter) -> String {
    let launched = emr_ok(
        router,
        "RunJobFlow",
        json!({
            "Name": "notebooks",
            "ReleaseLabel": "emr-6.15.0",
            "JobFlowRole": INSTANCE_PROFILE,
            "ServiceRole": SERVICE_ROLE,
            "Applications": [
                { "Name": "Hadoop" },
                { "Name": "Spark" },
                { "Name": "Livy" },
                { "Name": "JupyterEnterpriseGateway" },
            ],
            "Instances": {
                "InstanceCount": 2,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
            },
        }),
    )
    .await;
    launched["JobFlowId"]
        .as_str()
        .expect("RunJobFlow answers a JobFlowId")
        .to_string()
}

/// A notebook document in the Jupyter format, holding `cells` as they are given.
pub(crate) fn notebook(cells: Value, kernel_language: Option<&str>) -> Vec<u8> {
    let mut document = json!({
        "cells": cells,
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5,
    });
    if let Some(language) = kernel_language {
        document["metadata"] = json!({
            "kernelspec": { "language": language, "name": language },
        });
    }
    serde_json::to_vec(&document).expect("the notebook serializes")
}

/// One code cell whose source is the string `source`.
pub(crate) fn code_cell(source: &str) -> Value {
    json!({ "cell_type": "code", "source": source, "metadata": {}, "outputs": [] })
}

/// One code cell whose source is the LIST of its lines, which is the other spelling the
/// notebook format allows.
fn code_cell_lines(lines: &[&str]) -> Value {
    json!({ "cell_type": "code", "source": lines, "metadata": {}, "outputs": [] })
}

/// The execution `DescribeNotebookExecution` reports.
pub(crate) async fn described(router: &MultiServiceRouter, id: &str) -> Value {
    emr_ok(
        router,
        "DescribeNotebookExecution",
        json!({ "NotebookExecutionId": id }),
    )
    .await["NotebookExecution"]
        .clone()
}

/// Poll `DescribeNotebookExecution` until the execution reaches a terminal status, answering
/// every status seen on the way (in order, without repeats) and the execution as last read.
///
/// `pub(crate)`: the sibling suite that stops an execution already under way settles it
/// through this one walk rather than a copy of it.
pub(crate) async fn walked(
    case: &str,
    router: &MultiServiceRouter,
    id: &str,
) -> (Vec<String>, Value) {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let execution = described(router, id).await;
        let status = execution["Status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an execution reports a Status"))
            .to_string();
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if TERMINAL.contains(&status.as_str()) {
            return (seen, execution);
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {case:?}: no terminal status after {DEADLINE:?}, saw {seen:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What a row expects of its execution's end.
enum Ends {
    /// Every code cell ran: the execution finishes, and no failing status is ever seen.
    Finished,
    /// A cell raised: the execution fails, and its reason carries each of these.
    Failed(&'static [&'static str]),
    /// Nothing ran: the execution fails with the emulator's own not-executed sentence.
    NotRun,
}

/// One row: the notebook object this execution names (when it names one), and the end it
/// must reach.
struct Case {
    label: &'static str,
    /// The object written into the bucket for this row, under the row's own key.
    object: Option<Vec<u8>>,
    /// Whether the `StartNotebookExecution` request names the Amazon S3 location at all.
    names_location: bool,
    ends: Ends,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one cell that computes",
            object: Some(notebook(json!([code_cell("print(1 + 1)")]), None)),
            names_location: true,
            ends: Ends::Finished,
        },
        Case {
            label: "cells sharing one namespace, in order",
            object: Some(notebook(
                json!([
                    code_cell("answer = 6 * 7"),
                    code_cell("assert answer == 42, answer"),
                ]),
                None,
            )),
            names_location: true,
            ends: Ends::Finished,
        },
        Case {
            label: "a cell whose source is the list of its lines",
            object: Some(notebook(
                json!([code_cell_lines(&["total = 0\n", "for i in range(4):\n", "    total += i\n", "assert total == 6, total\n"])]),
                None,
            )),
            names_location: true,
            ends: Ends::Finished,
        },
        Case {
            label: "a cell that imports the standard library",
            object: Some(notebook(
                json!([code_cell(
                    "import json\nassert json.loads(json.dumps({'a': 1})) == {'a': 1}"
                )]),
                None,
            )),
            names_location: true,
            ends: Ends::Finished,
        },
        Case {
            label: "a notebook of prose only, with no code cell to run",
            object: Some(notebook(
                json!([{ "cell_type": "markdown", "source": "# report", "metadata": {} }]),
                Some("python"),
            )),
            names_location: true,
            ends: Ends::Finished,
        },
        Case {
            label: "a cell that raises",
            object: Some(notebook(
                json!([code_cell("raise ValueError('the third cell failed')")]),
                None,
            )),
            names_location: true,
            ends: Ends::Failed(&["ValueError", "the third cell failed"]),
        },
        Case {
            label: "a later cell that raises after an earlier one ran",
            object: Some(notebook(
                json!([
                    code_cell("rows = [1, 2, 3]"),
                    code_cell("rows[7]"),
                ]),
                None,
            )),
            names_location: true,
            ends: Ends::Failed(&["IndexError"]),
        },
        Case {
            label: "a cell reaching for a host file, which the run does not reach",
            object: Some(notebook(
                json!([code_cell("open('/etc/hostname').read()")]),
                None,
            )),
            names_location: true,
            ends: Ends::Failed(&[]),
        },
        Case {
            label: "a cell reaching for a socket, which the run does not reach",
            object: Some(notebook(
                json!([code_cell(
                    "import socket\nsocket.create_connection(('169.254.169.254', 80))"
                )]),
                None,
            )),
            names_location: true,
            ends: Ends::Failed(&[]),
        },
        Case {
            label: "a cell starting a process, which the run does not start",
            object: Some(notebook(
                json!([code_cell("import subprocess\nsubprocess.run(['/bin/echo', 'hi'])")]),
                None,
            )),
            names_location: true,
            ends: Ends::Failed(&[]),
        },
        Case {
            label: "a cell loading a host library, which the run does not load",
            object: Some(notebook(
                json!([code_cell("import ctypes\nctypes.CDLL('libc.so.6')")]),
                None,
            )),
            names_location: true,
            ends: Ends::Failed(&[]),
        },
        Case {
            label: "cells that see the execution's own environment and no host one",
            object: Some(notebook(
                json!([code_cell(
                    "import os\n\
                     assert os.environ.get('NOTEBOOK_SCOPE') == 'execution', os.environ\n\
                     for name in ('AWS_SECRET_ACCESS_KEY', 'AWS_SESSION_TOKEN', 'HOME', 'PATH'):\n\
                     \x20   assert name not in os.environ, name"
                )]),
                None,
            )),
            names_location: true,
            ends: Ends::Finished,
        },
        Case {
            label: "a cell that never stops, which the run's own bounds stop",
            object: Some(notebook(
                json!([code_cell("total = 0\nwhile True:\n    total += 1")]),
                None,
            )),
            names_location: true,
            ends: Ends::Failed(&["The notebook was stopped"]),
        },
        Case {
            label: "a kernel this emulator runs no cells of",
            object: Some(notebook(json!([code_cell("val x = 1")]), Some("scala"))),
            names_location: true,
            ends: Ends::NotRun,
        },
        Case {
            label: "an object that is no notebook at all",
            object: Some(b"this is not a notebook".to_vec()),
            names_location: true,
            ends: Ends::NotRun,
        },
        Case {
            label: "a key the bucket holds nothing under",
            object: None,
            names_location: true,
            ends: Ends::NotRun,
        },
        Case {
            label: "a request naming no Amazon S3 notebook",
            object: None,
            names_location: false,
            ends: Ends::NotRun,
        },
    ]
}

/// The key one row's notebook is written under.
fn key_of(index: usize) -> String {
    format!("notebook-{index}.ipynb")
}

#[tokio::test]
async fn a_notebook_execution_ends_in_what_running_its_notebook_answered() {
    let router = emulator();
    let cluster_id = notebook_cluster(&router).await;
    s3_ok(&router, "bucket", &format!("/{BUCKET}"), &[]).await;

    let cases = cases();
    let mut executions: Vec<(usize, String)> = Vec::new();

    // Every row's notebook is written first, then every execution is started, so the
    // clock walks them all over the same span rather than one row at a time.
    for (index, case) in cases.iter().enumerate() {
        if let Some(object) = case.object.as_ref() {
            s3_ok(
                &router,
                case.label,
                &format!("/{BUCKET}/{}", key_of(index)),
                object,
            )
            .await;
        }
    }
    for (index, case) in cases.iter().enumerate() {
        let mut request = json!({
            "ExecutionEngine": { "Id": cluster_id.clone(), "Type": "EMR" },
            "ServiceRole": NOTEBOOK_ROLE,
            "NotebookExecutionName": case.label,
            // "The environment variables associated with the notebook execution"
            // (`StartNotebookExecutionInput$EnvironmentVariables` doc): what the cells see,
            // and the only environment they see.
            "EnvironmentVariables": { "NOTEBOOK_SCOPE": "execution" },
        });
        if case.names_location {
            request["NotebookS3Location"] = json!({ "Bucket": BUCKET, "Key": key_of(index) });
        } else {
            request["EditorId"] = json!("e-NOTEBOOKEDITOR");
            request["RelativePath"] = json!("notebooks/demo.ipynb");
        }
        let answer = emr_ok(&router, "StartNotebookExecution", request).await;
        let id = answer["NotebookExecutionId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: StartNotebookExecution answers an id, got {answer}",
                    case.label
                )
            })
            .to_string();
        executions.push((index, id));
    }

    for (index, id) in executions {
        let case = &cases[index];
        let label = case.label;
        let (seen, execution) = walked(label, &router, &id).await;
        let reason = execution["LastStateChangeReason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let status = execution["Status"].as_str().unwrap_or_default().to_string();

        match case.ends {
            Ends::Finished => {
                assert_eq!(
                    status, "FINISHED",
                    "case {label:?}: expected FINISHED, got {status} with reason {reason:?}"
                );
                // The cause this suite exists for: an execution whose notebook RAN never
                // passes through a failing status.
                for failing in ["FAILING", "FAILED"] {
                    assert!(
                        !seen.iter().any(|status| status == failing),
                        "case {label:?}: a notebook that ran never reports {failing}, saw \
                         {seen:?}"
                    );
                }
                assert!(
                    reason.is_empty(),
                    "case {label:?}: a finished execution reports no failure reason, got \
                     {reason:?}"
                );
            }
            Ends::Failed(expected) => {
                assert_eq!(
                    status, "FAILED",
                    "case {label:?}: expected FAILED, got {status} with reason {reason:?}"
                );
                assert_ne!(
                    reason, NOTEBOOK_NOT_EXECUTED_REASON,
                    "case {label:?}: a notebook that RAN and failed reports the engine's own \
                     reason, not the not-executed sentence"
                );
                assert!(
                    !reason.is_empty(),
                    "case {label:?}: a failed run reports the reason it failed for"
                );
                for part in expected {
                    assert!(
                        reason.contains(part),
                        "case {label:?}: expected the reason to carry {part:?}, got {reason:?}"
                    );
                }
            }
            Ends::NotRun => {
                assert_eq!(
                    status, "FAILED",
                    "case {label:?}: an execution nothing ran ends FAILED, got {status}"
                );
                assert_eq!(
                    reason, NOTEBOOK_NOT_EXECUTED_REASON,
                    "case {label:?}: an execution nothing ran says so, got {reason:?}"
                );
            }
        }

        // However it ended, the execution reports when it did and still names its own id.
        assert_eq!(
            execution["NotebookExecutionId"].as_str(),
            Some(id.as_str()),
            "case {label:?}: the execution reports its own id, got {execution}"
        );
        assert!(
            execution["EndTime"].as_f64().is_some(),
            "case {label:?}: an ended execution reports an EndTime, got {execution}"
        );
    }
}

/// THE RULE's own walk, on ONE execution polled from the instant it was started so every
/// phase is observed: a notebook that ran goes `START_PENDING` → `STARTING` → `RUNNING` →
/// `FINISHING` → `FINISHED` and never reports a failing status, while a notebook that raised
/// walks the same way onto `FAILING` → `FAILED`.
#[tokio::test]
async fn the_walk_of_an_execution_whose_notebook_ran_passes_through_no_failure() {
    let router = emulator();
    let cluster_id = notebook_cluster(&router).await;
    s3_ok(&router, "bucket", &format!("/{BUCKET}"), &[]).await;

    let rows: Vec<(&str, Vec<u8>, Vec<&str>)> = vec![
        (
            "a notebook whose cells all run",
            notebook(json!([code_cell("answer = 6 * 7")]), None),
            vec![
                "START_PENDING",
                "STARTING",
                "RUNNING",
                "FINISHING",
                "FINISHED",
            ],
        ),
        (
            "a notebook whose cell raises",
            notebook(json!([code_cell("raise RuntimeError('cell one')")]), None),
            vec!["START_PENDING", "STARTING", "RUNNING", "FAILING", "FAILED"],
        ),
    ];

    for (index, (label, object, expected)) in rows.iter().enumerate() {
        let key = format!("walked-{index}.ipynb");
        s3_ok(&router, label, &format!("/{BUCKET}/{key}"), object).await;
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
        let (seen, execution) = walked(label, &router, &id).await;
        assert_eq!(
            seen, *expected,
            "case {label:?}: expected the walk {expected:?}, got {seen:?} ending in \
             {execution}"
        );
    }
}

/// The stop leg of the SAME rule: an execution a caller stops before it has ended reaches
/// `STOPPED`, under the id `StartNotebookExecution` answered — whatever its notebook was
/// going to come to.
///
/// A stop asked for once the clock has already moved the execution ON is the sibling suite
/// `crate::zz_probe_notebook_lifecycle`'s row of the same rule.
#[tokio::test]
async fn an_execution_the_caller_stops_reaches_stopped() {
    let router = emulator();
    let cluster_id = notebook_cluster(&router).await;
    s3_ok(&router, "bucket", &format!("/{BUCKET}"), &[]).await;

    // Two rows: a notebook that would have finished, and one nothing could run. Both stop.
    let rows: Vec<(&str, Option<Vec<u8>>)> = vec![
        (
            "a notebook that would have finished",
            Some(notebook(json!([code_cell("total = sum(range(10))")]), None)),
        ),
        ("a notebook nothing could run", None),
    ];

    for (index, (label, object)) in rows.iter().enumerate() {
        let key = format!("stopped-{index}.ipynb");
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
        emr_ok(
            &router,
            "StopNotebookExecution",
            json!({ "NotebookExecutionId": id.clone() }),
        )
        .await;

        // Polled from the instant the stop was asked for, so the stop's own phases are
        // observed rather than jumped over.
        let (seen, execution) = walked(label, &router, &id).await;
        assert_eq!(
            execution["Status"].as_str(),
            Some("STOPPED"),
            "case {label:?}: a stopped execution reaches STOPPED, saw {seen:?}"
        );
        assert_eq!(
            execution["NotebookExecutionId"].as_str(),
            Some(id.as_str()),
            "case {label:?}: the stopped execution is the one that was started, got \
             {execution}"
        );
        for stopping in ["STOP_PENDING", "STOPPING"] {
            assert!(
                seen.iter().any(|status| status == stopping),
                "case {label:?}: the stop walk passes through {stopping}, saw {seen:?}"
            );
        }
    }
}

