// SPDX-License-Identifier: CC0-1.0

//! What the two result READS answer for an identifier that resolves to nothing.
//!
//! RULE: a read of an asynchronous operation's result is decided by RESOLVING the
//! identifiers in its URI, not by checking their syntax: whatever the labels look like,
//! one that resolves to nothing is answered `404 ResourceNotFoundException` naming the
//! OPERATION the read was for — `Operation not found: <operationId>` on the training
//! route and `Sample request not found: <operationId>` on the sampling route, the
//! operation id echoed verbatim — so a poller can tell which of the two sibling lookups
//! missed, and the session named is never what the refusal reports.
//!
//! The rows span what the rule generalises over rather than one recorded call: the shape
//! of each identifier (inside the modelled syntax, outside it, over its length maximum,
//! percent-encoded), which of the two resolution steps missed (an unknown session, a
//! session that exists with no such operation, an id minted on the OTHER route), and both
//! routes. A read of an operation that DOES resolve is driven alongside, so the refusal is
//! shown to be resolution rather than a blanket 404.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The Region every call here signs for.
const REGION: &str = "us-west-2";

/// One emulator: a shared engine with this service mounted on the production router.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Register `name` the way the SageMaker control plane's `CreateTrainingSession`
    /// does, which is the only thing that makes a training session exist.
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

/// What the emulator answered.
struct Answer {
    status: u16,
    error_type: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn error_code(&self) -> String {
        let raw = self.error_type.clone().unwrap_or_else(|| {
            self.json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        raw.rsplit('#')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn member(&self, member: &str) -> String {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

async fn call(emulator: &Emulator, method: &str, path: &str, body: &[u8]) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sagemaker/aws4_request, \
         SignedHeaders=host, Signature=00"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://training-session-runtime.sagemaker.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request builds");
    let response = emulator.router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        error_type: response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        body: response.body().to_vec(),
    }
}

/// Submit one training operation against `session` and answer the id it minted.
async fn submit_training(emulator: &Emulator, session: &str) -> String {
    let body = serde_json::to_vec(&json!({
        "sequenceNumber": 1,
        "operationConfig": {"saveState": {}},
    }))
    .expect("the body serializes");
    let answer = call(
        emulator,
        "POST",
        &format!("/v1/training-sessions/{session}/operations"),
        &body,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "the training submit must be accepted, got {} {}",
        answer.status,
        answer.text()
    );
    answer.member("operationId")
}

/// Submit one sampling operation against `session` and answer the id it minted.
async fn submit_sampling(emulator: &Emulator, session: &str) -> String {
    let body = serde_json::to_vec(&json!({
        "operationConfig": {"computeLogprobs": {
            "prompt": {"chunks": [{"tokens": [1, 2], "type": "encoded_text"}]},
        }},
    }))
    .expect("the body serializes");
    let answer = call(
        emulator,
        "POST",
        &format!("/v1/training-sessions/{session}/sampling-operations"),
        &body,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "the sampling submit must be accepted, got {} {}",
        answer.status,
        answer.text()
    );
    answer.member("operationId")
}

/// The two read routes: the URI segment the operations hang off, and the fixed text the
/// refusal of an unresolvable id opens with.
const ROUTES: [(&str, &str); 2] = [
    ("operations", "Operation not found: "),
    ("sampling-operations", "Sample request not found: "),
];

/// RULE: on both read routes an identifier that resolves to nothing is answered 404 with
/// that route's own fixed text plus the request's operation id — whatever the labels look
/// like, and whichever of the two resolution steps missed.
#[tokio::test]
async fn an_unresolvable_read_reports_the_operation_it_was_for() {
    let emulator = Emulator::new();
    emulator.create_session("live");
    // One operation of each kind, so an id minted on the OTHER route is a real id rather
    // than one this emulator never saw.
    let training = submit_training(&emulator, "live").await;
    let sampling = submit_sampling(&emulator, "live").await;

    // (case, the session label, the operation label as the URI carries it, the id the
    // refusal must echo). `TrainingSessionName` and `OperationId` are both
    // `@length(1..64)` with pattern `^[a-zA-Z0-9-]+$`
    // (smithy_query(query="shape",shape="OperationId")), so the rows walk an identifier
    // inside that syntax, two outside it, one over the length maximum, and one carrying
    // characters the URI must percent-encode.
    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "a session nothing created",
            "absent-session".to_string(),
            "op-1".to_string(),
            "op-1".to_string(),
        ),
        (
            "a session that exists, with no such operation",
            "live".to_string(),
            "op-2".to_string(),
            "op-2".to_string(),
        ),
        (
            "an operation id outside the identifier syntax",
            "live".to_string(),
            "not_an_id".to_string(),
            "not_an_id".to_string(),
        ),
        (
            "a session name outside the identifier syntax",
            "bad_name".to_string(),
            "op-3".to_string(),
            "op-3".to_string(),
        ),
        (
            "both labels percent-encoded and outside the syntax",
            "bad%20name%21".to_string(),
            "not%20an%20id%21".to_string(),
            "not an id!".to_string(),
        ),
        (
            "an operation id one character over the length maximum",
            "live".to_string(),
            "o".repeat(65),
            "o".repeat(65),
        ),
        (
            "an id minted on the other route",
            "live".to_string(),
            String::new(),
            String::new(),
        ),
    ];

    for (route, opening) in ROUTES {
        for (case, session, label, echoed) in &rows {
            // The last row names the id the OTHER route minted, which only this loop
            // knows: a training id never resolves on the sampling route or the reverse.
            let (label, echoed) = if label.is_empty() {
                let other = if route == "operations" {
                    sampling.clone()
                } else {
                    training.clone()
                };
                (other.clone(), other)
            } else {
                (label.clone(), echoed.clone())
            };
            let answer = call(
                &emulator,
                "GET",
                &format!("/v1/training-sessions/{session}/{route}/{label}"),
                &[],
            )
            .await;
            assert_eq!(
                answer.status,
                404,
                "case {route} / {case:?}: expected HTTP 404, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {route} / {case:?}: expected the modelled not-found code, got {}",
                answer.text()
            );
            assert_eq!(
                answer.member("message"),
                format!("{opening}{echoed}"),
                "case {route} / {case:?}: expected the operation the read was for, got {:?}",
                answer.member("message")
            );
        }
    }
}

/// RULE: the two sibling routes word the same condition differently, so one answer can be
/// told from the other for the very same request.
#[tokio::test]
async fn the_two_read_routes_word_their_refusal_differently() {
    let emulator = Emulator::new();
    emulator.create_session("live");

    let mut messages = Vec::new();
    for (route, opening) in ROUTES {
        let answer = call(
            &emulator,
            "GET",
            &format!("/v1/training-sessions/live/{route}/shared-id"),
            &[],
        )
        .await;
        assert_eq!(
            answer.member("message"),
            format!("{opening}shared-id"),
            "case {route}: expected that route's own wording, got {:?}",
            answer.member("message")
        );
        messages.push(answer.member("message"));
    }
    assert_ne!(
        messages[0], messages[1],
        "case the two routes: expected distinguishable refusals, got {messages:?}"
    );
}

/// RULE: a read resolves, so an operation that DOES resolve is reported and a read never
/// brings anything into existence — the same unresolvable read answers the same way after
/// one has been tried.
#[tokio::test]
async fn a_read_reports_what_resolves_and_creates_nothing() {
    let emulator = Emulator::new();
    emulator.create_session("live");
    let training = submit_training(&emulator, "live").await;

    let resolved = call(
        &emulator,
        "GET",
        &format!("/v1/training-sessions/live/operations/{training}"),
        &[],
    )
    .await;
    assert_eq!(
        resolved.status,
        200,
        "case an operation that resolves: expected HTTP 200, got {} {}",
        resolved.status,
        resolved.text()
    );
    assert!(
        !resolved.member("status").is_empty(),
        "case an operation that resolves: expected its stored status, got {}",
        resolved.text()
    );

    for attempt in ["the first read", "the second read"] {
        let answer = call(
            &emulator,
            "GET",
            "/v1/training-sessions/absent-session/operations/op-9",
            &[],
        )
        .await;
        assert_eq!(
            (answer.status, answer.member("message")),
            (404, "Operation not found: op-9".to_string()),
            "case {attempt:?}: a read must report the same and materialise nothing, \
             got {} {}",
            answer.status,
            answer.text()
        );
    }
    // Nothing the reads touched became a session: a submit against that name is still
    // refused, which is what proves the read wrote no record.
    let submit = call(
        &emulator,
        "POST",
        "/v1/training-sessions/absent-session/operations",
        &serde_json::to_vec(&json!({
            "sequenceNumber": 1,
            "operationConfig": {"saveState": {}},
        }))
        .expect("the body serializes"),
    )
    .await;
    assert_eq!(
        submit.status,
        404,
        "case a submit after the reads: expected the session still absent, got {} {}",
        submit.status,
        submit.text()
    );
}
