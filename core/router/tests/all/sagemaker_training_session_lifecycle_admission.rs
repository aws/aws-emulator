// SPDX-License-Identifier: CC0-1.0

//! The training-session LIFECYCLE hand-off: the SageMaker CONTROL plane decides what state
//! a training session is in, and the SageMaker Training Session Runtime DATA plane accepts
//! work only in the states that accept it — both over the wire, with no in-process help.
//!
//! RULE: a data-plane call that submits work is admitted by the state the OWNING component
//! reports for the resource, so a stop made on the control plane stops the work the data
//! plane takes: `ConflictException` (HTTP 409) is documented as "The training session is in
//! a state that cannot accept new operations. The session must be in `Running` or
//! `Blocked` state to accept requests", and the refusal states the state the session is in.
//! Work accepted before the stop stays READABLE afterwards with its result unchanged — a
//! read reports what was accepted — and the refusal moves the session no further.
//!
//! Only this suite can observe that hand-off: each half is a separate crate with a separate
//! store, and the two agree solely because the control plane publishes the state of the
//! sessions it creates and stops into ONE shared directory
//! (`service_common::training_sessions`) the data plane reads. So both halves are driven
//! through the SAME [`MultiServiceRouter`] and the SAME engine `core/router/src/lib.rs`
//! mounts them on, and every session here comes to exist, come up and stop the way a client
//! makes it: `CreateTrainingSession`, `DescribeTrainingSession`, `StopTrainingSession`.
//!
//! The rows span what the rule generalises over rather than one recorded call: every
//! surface that submits work (the two asynchronous submits and the two inference calls),
//! the states a session passes through on its way out (`Created` before its compute is up,
//! `Stopping` while it quiesces, the terminal `Stopped`), and the state it accepts work in
//! between them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The host every request is addressed to; the claim each service makes is resolved from
/// the SigV4 credential scope and the URI path space, not from this name.
const HOST: &str = "emulator.local";

/// Far above either plane's own transition span, so a loaded host still reaches the
/// condition a loop is waiting for.
const DEADLINE: Duration = Duration::from_secs(60);
/// How long a wait loop pauses before it looks again.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// A role ARN inside `CreateTrainingSession`'s modelled `RoleArn` pattern.
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

/// A SigV4 `Authorization` header whose credential scope names the signing name the whole
/// SageMaker family shares.
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

/// The `TrainingSessionStatus` the control plane reports for `name`.
async fn reported_status(router: &MultiServiceRouter, name: &str) -> String {
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
    answer.json()["TrainingSessionDetail"]["TrainingSessionStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Wait until the control plane reports `name` in `status`.
///
/// The wait is for the CONDITION: each of the control plane's transitions is owed at an
/// instant it stored, so the loop looks again until the state has been reached or a
/// generous deadline passes.
async fn await_status(router: &MultiServiceRouter, name: &str, status: &str) {
    let started = Instant::now();
    loop {
        let reported = reported_status(router, name).await;
        if reported == status {
            return;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {name}: expected the session to reach {status} before the deadline, got \
             {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `StopTrainingSession` — "stops a running training session and sets the session status to
/// Stopping".
async fn stop_session(router: &MultiServiceRouter, name: &str) {
    let answer = control_plane(
        router,
        "StopTrainingSession",
        &json!({ "TrainingSessionName": name }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {name}: expected HTTP 200 from the stop, got {} with {}",
        answer.status,
        answer.text()
    );
}

/// One surface that submits work against a session, and a body it accepts.
struct Surface {
    case: &'static str,
    path: &'static str,
    body: fn() -> Value,
}

fn sampling_body() -> Value {
    json!({"operationConfig": {"sample": {
        "prompt": {"chunks": [{"tokens": [7, 8], "type": "encoded_text"}]},
        "numSamples": 1,
        "samplingParams": {"maxTokens": 512, "temperature": 0.7},
    }}})
}

fn training_body() -> Value {
    json!({
        "sequenceNumber": 1,
        "operationConfig": {"saveState": {"description": "a resumable checkpoint"}},
    })
}

fn inference_body() -> Value {
    json!({"messages": [{"role": "user", "content": "hello"}]})
}

fn surfaces() -> Vec<Surface> {
    vec![
        Surface {
            case: "ExecuteSamplingOperation",
            path: "/v1/training-sessions/{}/sampling-operations",
            body: sampling_body,
        },
        Surface {
            case: "ExecuteTrainingOperation",
            path: "/v1/training-sessions/{}/operations",
            body: training_body,
        },
        Surface {
            case: "SampleTrainingSession",
            path: "/training-sessions/{}/sample",
            body: inference_body,
        },
        Surface {
            case: "SampleTrainingSessionWithResponseStream",
            path: "/training-sessions/{}/sample-with-response-stream",
            body: inference_body,
        },
    ]
}

/// One data-plane submit against `session`.
async fn submit(router: &MultiServiceRouter, surface: &Surface, session: &str) -> Answer {
    dispatch(
        router,
        "POST",
        &surface.path.replace("{}", session),
        &[("content-type", "application/json")],
        serde_json::to_vec(&(surface.body)()).expect("a serializable data-plane body"),
    )
    .await
}

/// How a session in `status` refuses a call that submits work.
fn cannot_accept(status: &str) -> String {
    format!("Session is in {status} state and cannot accept requests")
}

/// RULE: the state the control plane reports is what the data plane admits work in — so a
/// session still provisioning and a session the control plane has stopped both refuse
/// every submit surface with the modelled conflict naming that state, while the same
/// surfaces are admitted in between.
#[tokio::test]
async fn a_control_plane_stop_stops_the_work_the_data_plane_takes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);
    let session = "stopped-session";

    create_session(&router, session).await;
    // Still provisioning: the session exists, and the state it is in refuses work.
    assert_eq!(
        reported_status(&router, session).await,
        "Created",
        "a session straight after its create is still provisioning"
    );
    for surface in surfaces() {
        let answer = submit(&router, &surface, session).await;
        assert_eq!(
            (answer.status, answer.member("message")),
            (409, cannot_accept("Created")),
            "case {:?} against a provisioning session: expected the modelled conflict \
             naming the state, got {} {}",
            surface.case,
            answer.status,
            answer.text()
        );
    }

    await_status(&router, session, "Running").await;
    let accepted = submit(&router, &surfaces()[0], session).await;
    assert_eq!(
        accepted.status,
        200,
        "a running session must accept the submit, got {} {}",
        accepted.status,
        accepted.text()
    );
    let operation_id = accepted.member("operationId");
    let before = poll_until_terminal(&router, session, "sampling-operations", &operation_id).await;
    assert_eq!(
        before["status"].as_str(),
        Some("Completed"),
        "the accepted operation must settle before the stop, got {before}"
    );

    stop_session(&router, session).await;
    // The stop is a transition of its own: the session quiesces through `Stopping` on its
    // way to the terminal `Stopped`, and refuses work in whichever of the two it is in
    // when the call arrives.
    for surface in surfaces() {
        let answer = submit(&router, &surface, session).await;
        let message = answer.member("message");
        assert_eq!(
            answer.status,
            409,
            "case {:?} against a stopping session: expected the modelled conflict, got {} {}",
            surface.case,
            answer.status,
            answer.text()
        );
        assert!(
            message == cannot_accept("Stopping") || message == cannot_accept("Stopped"),
            "case {:?} against a stopping session: expected the refusal to name the state \
             it is quiescing through, got {message:?}",
            surface.case
        );
    }

    await_status(&router, session, "Stopped").await;
    for surface in surfaces() {
        let answer = submit(&router, &surface, session).await;
        assert_eq!(
            (answer.status, answer.member("message")),
            (409, cannot_accept("Stopped")),
            "case {:?} against a stopped session: expected the modelled conflict naming \
             the terminal state, got {} {}",
            surface.case,
            answer.status,
            answer.text()
        );
    }
    // The refusals moved the session no further: the control plane still reports it
    // stopped.
    assert_eq!(
        reported_status(&router, session).await,
        "Stopped",
        "a refused submit must not move the session's own state"
    );
    // And the work accepted while it was running is still readable, unchanged.
    let after = poll_until_terminal(&router, session, "sampling-operations", &operation_id).await;
    assert_eq!(
        after, before,
        "the operation accepted before the stop must stay readable with its result \
         unchanged, got {after}"
    );
}

/// RULE: a session the control plane never created does not exist at all, which is the
/// modelled not-found rather than the state conflict — so the two refusals stay
/// distinguishable once the state is consulted.
#[tokio::test]
async fn a_session_nothing_created_is_still_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    for surface in surfaces() {
        let answer = submit(&router, &surface, "never-created").await;
        assert_eq!(
            (
                answer.status,
                answer.member("message").starts_with("Session not found: ")
            ),
            (404, true),
            "case {:?}: expected the modelled not-found, got {} {}",
            surface.case,
            answer.status,
            answer.text()
        );
    }
}

/// Poll one data-plane operation until its status is terminal, answering the last body.
async fn poll_until_terminal(
    router: &MultiServiceRouter,
    session: &str,
    route: &str,
    id: &str,
) -> Value {
    let started = Instant::now();
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
