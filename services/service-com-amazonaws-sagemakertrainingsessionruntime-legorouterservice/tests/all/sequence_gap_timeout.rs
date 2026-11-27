// SPDX-License-Identifier: CC0-1.0

//! A gap in a training session's sequence, at either end of the range a submit may name,
//! driven over the wire.
//!
//! `ExecuteTrainingOperation$sequenceNumber` is `SequenceNumber`, `@range(min: 1)` with
//! no ceiling, and it documents "The training session processes operations strictly in
//! sequence number order. If a gap exists, the session blocks until the missing operation
//! arrives or times out" (`smithy_query(query="operation",
//! operation="ExecuteTrainingOperation")`). So a request may name any sequence number
//! from `1` to the largest the shape's `integer` holds; an operation behind a gap WAITS —
//! it is not failed for waiting — the slot at the head executes, and the queue has to
//! stay finite for every sequence number a request can state rather than driving itself
//! at the end of the range.
//!
//! The rows span the published floor, the largest value the shape admits, and the two
//! together in each order, with the gap between them: each submit is answered, each
//! operation's status is read back from stored state, and the operation behind the gap is
//! still non-terminal while the one at the head has settled.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The region the tests' SigV4 credential scope names.
const REGION: &str = "us-west-2";

/// `SequenceNumber` is `@range(min: 1)`, and its target shape is an `integer`, so the
/// largest value a request can state is the largest `i32`.
const SEQUENCE_FLOOR: i64 = 1;
const SEQUENCE_CEILING: i64 = i32::MAX as i64;

/// One emulator: a shared engine with this service mounted on the production router.
///
/// The engine is held beside the router because a training session EXISTS only because the
/// SageMaker control plane created it: this data plane models no create and resolves the
/// name a request carries through the directory both services read, so each case registers
/// the session it submits against ([`Emulator::create_session`]).
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// Register `name` the way the SageMaker control plane's `CreateTrainingSession` does.
    fn create_session(&self, name: &str) {
        self.engine
            .state::<service_common::training_sessions::TrainingSessionDirectory>()
            .lock()
            .register(service_common::training_sessions::TrainingSessionRecord {
                name: name.to_string(),
                arn: engine::emulator_arn(
                    "sagemaker",
                    REGION,
                    &format!("training-session/{name}"),
                ),
                // A session whose compute the control plane has brought up: "The session
                // must be in Running or Blocked state to accept requests."
                lifecycle: service_common::training_sessions::TrainingSessionLifecycle::settled(
                    "Running",
                ),
                // The session's base model decides the context window its sampling
                // operations are served over; this one is of a family this emulator
                // publishes no window for, so no ceiling is in play in the cases below.
                base_model_arn: format!(
                    "arn:aws:sagemaker:{REGION}:aws:hub-content/SageMakerPublicHub/Model/\
                     meta-textgeneration-llama-3-8b/1.0.0"
                ),
            });
    }
}

impl std::ops::Deref for Emulator {
    type Target = MultiServiceRouter;

    fn deref(&self) -> &Self::Target {
        &self.router
    }
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Value {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("the body serializes")),
        None => bytes::Bytes::new(),
    };
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sagemaker/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!(
            "https://training-session-runtime.sagemaker.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body().to_vec();
    let answered: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {path} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    json!({"status": status, "body": answered})
}

/// Submit one training operation at `sequence`, requiring the submit to be answered, and
/// answer the operation id it minted.
async fn submit(router: &MultiServiceRouter, session: &str, sequence: i64, case: &str) -> String {
    let body = json!({
        "sequenceNumber": sequence,
        "operationConfig": {"saveWeights": {}},
    });
    let answered = call(
        router,
        "POST",
        &format!("/v1/training-sessions/{session}/operations"),
        Some(body),
    )
    .await;
    assert_eq!(
        answered["status"], 200,
        "case {case:?}: the submit at sequence {sequence} must be accepted, got {answered}"
    );
    answered["body"]["operationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the submit must mint an id, got {answered}"))
        .to_string()
}

/// The status a poll reports for `id`, read back from stored state.
async fn status(router: &MultiServiceRouter, session: &str, id: &str, case: &str) -> String {
    let answered = call(
        router,
        "GET",
        &format!("/v1/training-sessions/{session}/operations/{id}"),
        None,
    )
    .await;
    assert_eq!(
        answered["status"], 200,
        "case {case:?}: the poll of {id} must be answered, got {answered}"
    );
    answered["body"]["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the poll must report a status, got {answered}"))
        .to_string()
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "Completed" | "Failed" | "Skipped")
}

/// How many times a condition is looked for before a case gives up, and how long the loop
/// waits before looking again — a budget far above the service's own execution span, so a
/// loaded host still passes.
const POLL_BUDGET: u32 = 60;
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Wait until the operation `id` of `session` reaches a terminal status.
///
/// Waits for the CONDITION rather than for a span of wall clock: the transition is owed at
/// an instant the service stored, so the loop looks again until it has happened or a
/// generous deadline passes.
async fn poll_to_terminal(router: &MultiServiceRouter, session: &str, id: &str, case: &str) {
    for _ in 0..POLL_BUDGET {
        if is_terminal(&status(router, session, id, case).await) {
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {case:?}: {id} did not settle within the poll budget");
}

/// A submit at either end of the sequence-number range is answered, and the queue stays
/// finite: the slot at the head executes, and an operation behind a gap waits.
#[tokio::test]
async fn a_submit_at_each_end_of_the_sequence_range_is_served() {
    // (case, the sequence numbers submitted in order, whether the queue's head executes)
    let rows: [(&str, &[i64]); 4] = [
        ("the published floor alone", &[SEQUENCE_FLOOR]),
        ("the largest sequence number there is alone", &[SEQUENCE_CEILING]),
        (
            "the floor and then the largest there is",
            &[SEQUENCE_FLOOR, SEQUENCE_CEILING],
        ),
        (
            "the largest there is and then the floor",
            &[SEQUENCE_CEILING, SEQUENCE_FLOOR],
        ),
    ];

    for (case, sequences) in rows {
        let router = emulator();
        let session = "sequence-ends";
        router.create_session(session);
        let mut submitted: Vec<(i64, String)> = Vec::new();
        for sequence in sequences {
            let id = submit(&router, session, *sequence, case).await;
            submitted.push((*sequence, id));
        }
        // Wait for the CONDITION rather than for a span of wall clock: an operation at
        // the head of ANOTHER session's queue settling is what tells this case a
        // transition has really fallen due, so a row behind a gap has had exactly that
        // long to execute and has not. Nothing here waits on the gap itself, which the
        // session blocks on until its operation arrives.
        let ticking = "sequence-ends-clock";
        router.create_session(ticking);
        let ticking_id = submit(&router, ticking, SEQUENCE_FLOOR, case).await;
        poll_to_terminal(&router, ticking, &ticking_id, case).await;

        for (sequence, id) in &submitted {
            let reported = status(&router, session, id, case).await;
            // The queue executes strictly in order from the floor, so only an operation
            // at the floor has reached the head and settled; one behind a gap waits.
            let expected_terminal = *sequence == SEQUENCE_FLOOR;
            assert_eq!(
                is_terminal(&reported),
                expected_terminal,
                "case {case:?}: the operation at sequence {sequence} must be \
                 {} the head of the queue, got status {reported:?}",
                if expected_terminal {
                    "settled at"
                } else {
                    "waiting behind"
                }
            );
        }
    }
}
