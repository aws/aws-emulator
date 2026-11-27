// SPDX-License-Identifier: CC0-1.0

//! The training-session ACCOUNTING hand-off: the SageMaker CONTROL plane creates and
//! reports a training session, the SageMaker Training Session Runtime DATA plane serves
//! the operations submitted against it — both over the wire, with no in-process help.
//!
//! RULE: a member that reports a resource's own ACCOUNTED quantity — a cursor or a running
//! total the SERVICE keeps for it, not an echo of a request — is reported for every
//! resource that exists, at the value the service has counted, and that value is what the
//! component serving the traffic has really recorded; a resource with nothing accounted
//! reports zero. Here that is `DescribeTrainingSession`'s
//! `TrainingSessionDetail$LastReceivedSequenceNumber` ("The sequence number of the last
//! training operation received by the session") and `$BillableTokenUsage`
//! (`PrefillTokenCount` / `SampleTokenCount` / `TrainingTokenCount`, "The billable token
//! usage for the training session" —
//! `smithy_query(query="shape",shape="TrainingSessionDetail")`), reported against the
//! operations `ExecuteTrainingOperation` / `ExecuteSamplingOperation` accepted on the data
//! plane.
//!
//! Only this suite can observe that hand-off: each half is a separate crate with a
//! separate store, and the two agree solely because the data plane publishes what its own
//! records add up to into ONE shared account
//! (`service_common::training_session_usage`) the control plane reads. So both halves are
//! driven through the SAME [`MultiServiceRouter`] and the SAME engine
//! `core/router/src/lib.rs` mounts them on, and every session here comes to exist the way a
//! client makes one: an HTTP `CreateTrainingSession`.
//!
//! The rows span what the rule generalises over rather than one recorded call: how many
//! training operations the session received and at which sequence numbers (consecutive, a
//! single one, a gap the queue blocks on, and a submit the data plane REFUSES), whether the
//! traffic was training or sampling, and a session nothing was submitted against at all.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The host every request is addressed to; the claim each service makes is resolved from
/// the SigV4 credential scope and the URI path space, not from this name.
const HOST: &str = "emulator.local";

/// A role ARN inside `CreateTrainingSession`'s modelled `RoleArn` pattern
/// (`^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/training-session",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A base model ARN inside `TrainingSessionModelConfig$BaseModelArn`'s modelled pattern.
const BASE_MODEL_ARN: &str =
    "arn:aws:sagemaker:us-east-1:aws:hub-content/SageMakerPublicHub/Model/llama-3-8b/1.0.0";

/// The SageMaker control plane and the training-session runtime, mounted on one engine as
/// `core/router/src/lib.rs` mounts them.
fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(
        Arc::clone(engine),
    ));
    router.register_service(
        service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// One HTTP answer, reduced to what this suite asserts on.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A SigV4 `Authorization` header whose credential scope names the signing name the whole
/// SageMaker family shares. Request authentication is off on this router, so the signature
/// itself is never checked; the scope is what the router reads to resolve the route.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sagemaker/aws4_request, \
         SignedHeaders=host, \
         Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Send one request through the whole router.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{HOST}{path}"))
        .header("host", HOST)
        .header("authorization", authorization());
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: response.into_body().to_vec(),
    }
}

/// One control-plane call, in its awsJson1_1 RPC form.
async fn control_plane(router: &MultiServiceRouter, target: &str, body: &Value) -> Answer {
    dispatch(
        router,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &format!("SageMaker.{target}")),
        ],
        serde_json::to_vec(body).expect("a serializable control-plane body"),
    )
    .await
}

/// `CreateTrainingSession` — the ONE call that makes a session exist.
async fn create_session(router: &MultiServiceRouter, name: &str) {
    let answer = control_plane(
        router,
        "CreateTrainingSession",
        &json!({
            "TrainingSessionName": name,
            "LoraConfig": { "Rank": 8, "Alpha": 16 },
            "RoleArn": role_arn(),
            "ModelConfig": { "BaseModelArn": BASE_MODEL_ARN, "AcceptEula": true },
            "OutputConfig": { "S3Location": { "Path": "s3://session-outputs/run" } },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {name}: expected HTTP 200 from the create, got {} with {}",
        answer.status,
        answer.text()
    );
}

/// The `TrainingSessionDetail` the control plane reports for `name`.
async fn describe_session(router: &MultiServiceRouter, name: &str) -> Value {
    let answer = control_plane(
        router,
        "DescribeTrainingSession",
        &json!({ "TrainingSessionName": name }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {name}: expected HTTP 200 from the describe, got {} with {}",
        answer.status,
        answer.text()
    );
    answer.json()["TrainingSessionDetail"].clone()
}

/// Wait until the control plane reports `name` as `Running`, which is when the data plane
/// accepts work for it (`ConflictException`: "The session must be in Running or Blocked
/// state to accept requests").
///
/// A create "provisions compute resources", so the session is `Created` until that compute
/// is up. The wait is for the CONDITION: the transition is owed at an instant the control
/// plane stored, so the loop looks again until it has happened or a generous deadline
/// passes.
async fn await_running(router: &MultiServiceRouter, name: &str) {
    /// Far above the control plane's own transition span, so a loaded host still passes.
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
    /// How long the loop waits before looking again.
    const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(25);

    let started = std::time::Instant::now();
    loop {
        let status = describe_session(router, name).await["TrainingSessionStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status == "Running" {
            return;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {name}: expected the session to reach Running before the deadline, got \
             {status:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One data-plane call, in its restJson1 form.
async fn data_plane(router: &MultiServiceRouter, method: &str, path: &str, body: &Value) -> Answer {
    dispatch(
        router,
        method,
        path,
        &[("content-type", "application/json")],
        serde_json::to_vec(body).expect("a serializable data-plane body"),
    )
    .await
}

/// `ExecuteTrainingOperation` at `sequence`, as a runtime client submits one.
async fn submit_training(router: &MultiServiceRouter, session: &str, sequence: i64) -> Answer {
    data_plane(
        router,
        "POST",
        &format!("/v1/training-sessions/{session}/operations"),
        // `saveState` saves "full training state", which this emulator really performs:
        // the row's point is the sequence number the session RECEIVED, so the operation is
        // one whose own outcome needs no model runtime.
        &json!({
            "sequenceNumber": sequence,
            "operationConfig": { "saveState": { "description": "a resumable checkpoint" } },
        }),
    )
    .await
}

/// `ExecuteSamplingOperation`, which carries no sequence number at all.
async fn submit_sampling(router: &MultiServiceRouter, session: &str) -> Answer {
    data_plane(
        router,
        "POST",
        &format!("/v1/training-sessions/{session}/sampling-operations"),
        &json!({
            "operationConfig": { "sample": {
                "prompt": { "chunks": [{ "tokens": [7, 8], "type": "encoded_text" }] },
                "numSamples": 1,
                "samplingParams": { "maxTokens": 512, "temperature": 0.7 },
            }},
        }),
    )
    .await
}

/// A numeric member of the reported detail, which `awsJson1_1` carries as a JSON number.
fn number(detail: &Value, member: &str) -> i64 {
    detail
        .get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected {member} to be reported as a number in {detail}"))
}

/// The three `BillableTokenUsage` counters the control plane reports.
fn billable_tokens(detail: &Value) -> (i64, i64, i64) {
    let usage = detail
        .get("BillableTokenUsage")
        .unwrap_or_else(|| panic!("expected BillableTokenUsage to be reported in {detail}"));
    (
        number(usage, "PrefillTokenCount"),
        number(usage, "SampleTokenCount"),
        number(usage, "TrainingTokenCount"),
    )
}

/// One row of the sequence-number table: the traffic a session takes and the cursor the
/// control plane must then report for it.
struct SequenceRow {
    case: &'static str,
    session: &'static str,
    /// The sequence numbers submitted in order, each with whether the data plane accepts
    /// that submit.
    submits: Vec<(i64, bool)>,
    expected: i64,
}

/// RULE: the sequence number the control plane reports is the one the data plane last
/// RECEIVED for that session — zero before any, moving on each accepted submit, and
/// unmoved by one the data plane refused.
#[tokio::test]
async fn the_reported_sequence_number_is_the_one_the_data_plane_last_received() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    let rows = vec![
        SequenceRow {
            case: "nothing-submitted",
            session: "idle",
            submits: vec![],
            expected: 0,
        },
        SequenceRow {
            case: "one-operation",
            session: "single",
            submits: vec![(1, true)],
            expected: 1,
        },
        SequenceRow {
            case: "consecutive-operations",
            session: "consecutive",
            submits: vec![(1, true), (2, true), (3, true)],
            expected: 3,
        },
        SequenceRow {
            // A gap is accepted — the queue blocks on the missing slot — and what the
            // session RECEIVED is the number the submit carried.
            case: "a-gap-the-queue-blocks-on",
            session: "gapped",
            submits: vec![(1, true), (7, true)],
            expected: 7,
        },
        SequenceRow {
            // A slot already taken is refused, so nothing was received: the cursor stays
            // where the accepted submit left it.
            case: "a-refused-duplicate-slot",
            session: "refused",
            submits: vec![(1, true), (2, true), (2, false)],
            expected: 2,
        },
    ];

    // Every session is created and brought up BEFORE any traffic: a session accepts
    // operations only once its compute is running, and the waits overlap when the creates
    // are made together.
    for row in &rows {
        create_session(&router, row.session).await;
    }
    for row in &rows {
        await_running(&router, row.session).await;
    }

    for row in rows {
        let SequenceRow {
            case,
            session,
            submits,
            expected,
        } = row;
        // Before any traffic the session reports the zero it has accounted.
        assert_eq!(
            number(
                &describe_session(&router, session).await,
                "LastReceivedSequenceNumber"
            ),
            0,
            "case {case:?}: a session with no accounted traffic reports zero"
        );

        for (sequence, accepted) in &submits {
            let answer = submit_training(&router, session, *sequence).await;
            let ok = answer.status == 200;
            assert_eq!(
                ok, *accepted,
                "case {case:?}: submit of sequence {sequence} expected accepted={accepted}, \
                 got HTTP {} with {}",
                answer.status,
                answer.text()
            );
        }

        let detail = describe_session(&router, session).await;
        assert_eq!(
            number(&detail, "LastReceivedSequenceNumber"),
            expected,
            "case {case:?}: expected the last received sequence number {expected}, \
             got {detail}"
        );
        // Reading it again reports the same: a describe accounts nothing of its own.
        let reread = describe_session(&router, session).await;
        assert_eq!(
            number(&reread, "LastReceivedSequenceNumber"),
            expected,
            "case {case:?}: a second describe must report the same cursor, got {reread}"
        );
    }
}

/// RULE: the billable token counters the control plane reports are the totals the data
/// plane has really measured for that session — so an operation that reports no token usage
/// adds none, whatever traffic the session took.
#[tokio::test]
async fn the_reported_token_usage_is_what_the_data_plane_measured() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // (case, the session's name, what is submitted against it)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("nothing-submitted", "untouched", "none"),
        ("a-training-operation", "trained", "training"),
        ("a-sampling-operation", "sampled", "sampling"),
        ("both-kinds-of-traffic", "mixed", "both"),
    ];

    for (case, session, traffic) in rows {
        create_session(&router, session).await;
        await_running(&router, session).await;
        let mut operations: Vec<(String, String)> = Vec::new();
        if traffic == "training" || traffic == "both" {
            let answer = submit_training(&router, session, 1).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the training submit to be accepted, got {}",
                answer.text()
            );
            operations.push((
                "operations".to_string(),
                answer.json()["operationId"]
                    .as_str()
                    .expect("the minted operation id")
                    .to_string(),
            ));
        }
        if traffic == "sampling" || traffic == "both" {
            let answer = submit_sampling(&router, session).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the sampling submit to be accepted, got {}",
                answer.text()
            );
            operations.push((
                "sampling-operations".to_string(),
                answer.json()["operationId"]
                    .as_str()
                    .expect("the minted operation id")
                    .to_string(),
            ));
        }

        // What the data plane MEASURED for each operation, read from the data plane's own
        // result: the control plane's counters must be the sum of exactly these. A
        // training operation reports its usage as one `tokenUsage` on the result; a
        // sampling one reports a `prefill` / `decode` pair per sampled sequence.
        let mut measured = (0_i64, 0_i64, 0_i64);
        for (route, id) in &operations {
            let polled = poll_until_terminal(&router, session, route, id).await;
            if let Some(tokens) = polled
                .get("tokenUsage")
                .and_then(|usage| usage.get("trainingTokens"))
                .and_then(Value::as_i64)
            {
                measured.2 += tokens;
            }
            // `SampleOperationResult`'s members are keyed by their WIRE names, so the
            // sampled sequences hang off `sample` — `smithy_query(query="shape",
            // shape="SampleOperationResult")`.
            let sequences = polled
                .get("operationResult")
                .and_then(|result| result.get("sample"))
                .and_then(|sample| sample.get("sequences"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for sequence in sequences {
                let Some(usage) = sequence.get("tokenUsage") else {
                    continue;
                };
                measured.0 += usage.get("prefill").and_then(Value::as_i64).unwrap_or(0);
                measured.1 += usage.get("decode").and_then(Value::as_i64).unwrap_or(0);
            }
        }

        let detail = describe_session(&router, session).await;
        let reported = billable_tokens(&detail);
        assert_eq!(
            reported, measured,
            "case {case:?}: expected the counters the data plane measured {measured:?}, \
             got {detail}"
        );
        // Reading it again reports the same: a describe accounts nothing of its own.
        let reread = billable_tokens(&describe_session(&router, session).await);
        assert_eq!(
            reread, measured,
            "case {case:?}: a second describe must report the same counters, got {reread:?}"
        );
    }
}

/// Poll one data-plane operation until its status is terminal, answering the last body.
///
/// Waits for the CONDITION rather than for a fixed time: the operation's own transition is
/// owed at an instant the data plane stored, so the loop looks again until it has happened
/// or a generous deadline passes.
async fn poll_until_terminal(
    router: &MultiServiceRouter,
    session: &str,
    route: &str,
    id: &str,
) -> Value {
    /// Far above the data plane's own execution span, so a loaded host still passes.
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
    /// How long the loop waits before looking again.
    const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(25);

    let started = std::time::Instant::now();
    loop {
        let answer = dispatch(
            router,
            "GET",
            &format!("/v1/training-sessions/{session}/{route}/{id}"),
            &[("content-type", "application/json")],
            Vec::new(),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {id}: a poll of a stored operation must answer HTTP 200, got {} with {}",
            answer.status,
            answer.text()
        );
        let body = answer.json();
        let status = body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // The terminal statuses of the data plane's own `RequestStatus`
        // (`GetTrainingOperationResult`: "Completed, Failed, or Skipped").
        if matches!(status.as_str(), "Completed" | "Failed" | "Skipped") {
            return body;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {id}: expected a terminal status before the deadline, got {body}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}
