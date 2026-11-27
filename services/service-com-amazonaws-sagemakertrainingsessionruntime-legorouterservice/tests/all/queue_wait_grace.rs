// SPDX-License-Identifier: CC0-1.0

//! What a store FULL of sessions whose queues wait on an operation nobody submitted does
//! with the next submit.
//!
//! A training session "processes operations strictly in sequence number order. If a gap
//! exists, the session blocks until the missing operation arrives", so no instant the clock
//! reaches settles a session whose next slot is empty. This emulator keeps only
//! `state::MAX_SESSIONS` sessions, so what it does with such a session when it is full is
//! what decides whether a caller naming a fresh session can still be served — the one part
//! of the gap's behaviour the suites beside this one do not reach
//! (`sequence_gap_timeout.rs` covers an operation behind a gap waiting rather than failing,
//! and `training_session_runtime.rs` covers the queue draining once the gap is filled).
//!
//! The rule under test: a session waiting on an operation nobody submitted does not hold
//! its slot for ever. With the store full of such sessions, a submit naming a session the
//! store does not hold is SERVED — the OLDEST waiting session is released, so its
//! operations answer the modelled 404 while a session that was not the oldest keeps
//! reporting its own.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::{
    state, EmulatedService,
};

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// The non-terminal `RequestStatus` an operation behind a gap reports, from the model's
/// own enum.
const PENDING: &str = "Pending";

/// One emulator: a shared engine with this service mounted on the production router, so
/// every case drives the same dispatch path the binary does.
///
/// The engine is held beside the router because a training session EXISTS only because the
/// SageMaker control plane created it: this data plane models no create and resolves the
/// name a request carries through the directory both services read, so each case registers
/// the sessions it submits against ([`Emulator::create_session`]).
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
    // The cases assert behaviour, not signatures; the emulator verifies signatures itself
    // and a service never checks them.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let bytes = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("test body serializes")),
        None => bytes::Bytes::new(),
    };
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/sagemaker/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=00"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://sagemaker.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header("authorization", authorization)
        .body(bytes)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Submit one training operation at `sequence` without asserting the outcome.
async fn submit(router: &MultiServiceRouter, session: &str, sequence: i64) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/training-sessions/{session}/operations"),
        Some(json!({
            "sequenceNumber": sequence,
            "operationConfig": { "saveState": {} },
        })),
    )
    .await
}

/// Submit one training operation at `sequence`, asserting the submit was accepted, and
/// answer the minted operation id.
async fn submit_ok(router: &MultiServiceRouter, case: &str, session: &str, sequence: i64) -> String {
    let (status, response) = submit(router, session, sequence).await;
    assert_eq!(
        status, 200,
        "case {case:?}: submitting sequence {sequence} to {session} must answer HTTP 200, \
         got {status} {response}"
    );
    member(&response, "operationId")
}

/// Poll one training operation, answering the status and body unchanged.
async fn poll(router: &MultiServiceRouter, session: &str, id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v1/training-sessions/{session}/operations/{id}"),
        None,
    )
    .await
}

/// A string member, read as the empty string when absent — what a client sees for a member
/// the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a failed call.
fn error_code(response: &Value) -> String {
    response["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The rows differ in how far ahead of the gap the submitted operations sit and how many of
/// them each waiting session holds, which is what the rule generalises over: a session is
/// waiting whenever the slot it would execute next holds nothing, whatever is queued after
/// it.
#[tokio::test]
async fn a_store_full_of_waiting_sessions_still_serves_a_submit_naming_a_new_one() {
    for queued in [&[2i64] as &[i64], &[3, 4], &[5, 9, 12]] {
        let case = &format!("sessions waiting with sequence number(s) {queued:?} queued");
        let case = case.as_str();
        let router = emulator();

        // Sequence number 1 — the slot each queue would execute next — is left empty, so
        // every one of these sessions is waiting on an operation nobody submitted.
        let waiting: Vec<String> = (0..state::MAX_SESSIONS)
            .map(|index| format!("waiting-{index:04}"))
            .collect();
        let mut held = Vec::new();
        for session in &waiting {
            router.create_session(session);
            let mut ids = Vec::new();
            for sequence in queued {
                ids.push(submit_ok(&router, case, session, *sequence).await);
            }
            held.push(ids);
        }

        // The store is full and nothing in it can settle, yet a submit naming a session it
        // does not hold is served.
        let newest = waiting.len() - 1;
        router.create_session("one-more");
        let (status, response) = submit(&router, "one-more", 1).await;
        assert_eq!(
            status, 200,
            "case {case:?}: with {} waiting sessions held, a submit naming a new session \
             must still be served, got {status} {response}",
            state::MAX_SESSIONS
        );

        // What that cost is visible, and bounded to the OLDEST waiting session: its
        // operations are gone, while the newest waiting session still reports its own,
        // still waiting.
        for id in &held[0] {
            let (status, response) = poll(&router, &waiting[0], id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: an operation of the released session must answer HTTP 404, \
                 got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected the modelled not-found code, got {response}"
            );
        }
        for id in &held[newest] {
            let (status, response) = poll(&router, &waiting[newest], id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: only the oldest waiting session is released, so {} must \
                 still report its operations, got {status} {response}",
                waiting[newest]
            );
            assert_eq!(
                member(&response, "status"),
                PENDING,
                "case {case:?}: an operation behind a gap is waiting, not failed, \
                 got {response}"
            );
        }
    }
}
