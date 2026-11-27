// SPDX-License-Identifier: CC0-1.0

//! Which training sessions this data plane accepts work for, end to end over the wire.
//!
//! RULE: a training session accepts new operations only in the lifecycle states the
//! control plane admits them in — `ConflictException` (HTTP 409) is documented as "The
//! training session is in a state that cannot accept new operations. The session must be
//! in `Running` or `Blocked` state to accept requests", and every submit surface of this
//! service repeats it — so a call against a session in any other state is refused with
//! that error, stating the state the session is in, and nothing of the session changes
//! because of the refusal. A session the control plane never created is a different
//! answer: the modelled `ResourceNotFoundException`, because it does not exist at all.
//!
//! READS are not submits: `GetTrainingOperationResult` and `GetSamplingOperationResult`
//! report an operation that was accepted while the session was accepting work, whatever
//! state the session has reached since, so a client can still collect the result of work
//! it submitted before stopping the session.
//!
//! The rows span what the rule generalises over rather than one recorded call: both
//! accepting states, every other state of the session lifecycle the control plane reaches,
//! every surface that submits work (the two asynchronous submits and the two inference
//! calls), a state the clock has yet to reach as well as one it has, and a name nothing
//! created.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::state;
use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The Region every call here signs for.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// How many polls a terminal status is waited for, and how long each one waits.
const POLL_BUDGET: u32 = 400;
/// See [`POLL_BUDGET`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

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

    /// Register `name` in the lifecycle the control plane published for it, the way
    /// `CreateTrainingSession` and `StopTrainingSession` do.
    fn publish_session(
        &self,
        name: &str,
        lifecycle: service_common::training_sessions::TrainingSessionLifecycle,
    ) {
        self.engine
            .state::<service_common::training_sessions::TrainingSessionDirectory>()
            .lock()
            .register(service_common::training_sessions::TrainingSessionRecord {
                name: name.to_string(),
                arn: engine::emulator_arn("sagemaker", REGION, &format!("training-session/{name}")),
                lifecycle,
                // Of a base model family this emulator publishes no context window for, so
                // every refusal below is the lifecycle's and not a ceiling's.
                base_model_arn: format!(
                    "arn:aws:sagemaker:{REGION}:aws:hub-content/SageMakerPublicHub/Model/\
                     meta-textgeneration-llama-3-8b/1.0.0"
                ),
            });
    }

    /// Register `name` in a settled `status`.
    fn create_session(&self, name: &str, status: &str) {
        self.publish_session(
            name,
            service_common::training_sessions::TrainingSessionLifecycle::settled(status),
        );
    }
}

/// What the emulator answered.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
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
        body: response.body().to_vec(),
    }
}

/// One surface that SUBMITS work against a session: where it is addressed and what a valid
/// body carries.
struct Surface {
    case: &'static str,
    path: &'static str,
    body: fn() -> Vec<u8>,
}

fn sampling_body() -> Vec<u8> {
    serde_json::to_vec(&json!({"operationConfig": {"sample": {
        "prompt": {"chunks": [{"tokens": [464, 3290], "type": "encoded_text"}]},
        "numSamples": 1,
        "samplingParams": {"maxTokens": 512},
    }}}))
    .expect("serializes")
}

fn logprobs_body() -> Vec<u8> {
    serde_json::to_vec(&json!({"operationConfig": {"computeLogprobs": {
        "prompt": {"chunks": [{"tokens": [464, 3290], "type": "encoded_text"}]},
    }}}))
    .expect("serializes")
}

fn training_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "sequenceNumber": 1,
        "operationConfig": {"saveState": {"description": "a resumable checkpoint"}},
    }))
    .expect("serializes")
}

fn inference_body() -> Vec<u8> {
    serde_json::to_vec(&json!({"messages": [{"role": "user", "content": "hello"}]}))
        .expect("serializes")
}

fn surfaces() -> Vec<Surface> {
    vec![
        Surface {
            case: "ExecuteSamplingOperation",
            path: "/v1/training-sessions/{}/sampling-operations",
            body: sampling_body,
        },
        Surface {
            case: "ExecuteSamplingOperation with computeLogprobs",
            path: "/v1/training-sessions/{}/sampling-operations",
            body: logprobs_body,
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

async fn submit(emulator: &Emulator, surface: &Surface, session: &str) -> Answer {
    call(
        emulator,
        "POST",
        &surface.path.replace("{}", session),
        &(surface.body)(),
    )
    .await
}

/// RULE: every surface that submits work refuses a session outside the states that accept
/// requests, with the modelled 409 and the state the session is in — and accepts one in
/// either state that does.
#[tokio::test]
async fn a_session_outside_the_accepting_states_refuses_every_submit() {
    let emulator = Emulator::new();

    // Every state of the training-session lifecycle the control plane reaches, plus the
    // other members of `TrainingSessionStatus` a session can be reported in
    // (smithy_query(query="shape",shape="TrainingSessionStatus") of the SageMaker control
    // plane): only `Running` and `Blocked` accept requests.
    let states: Vec<(&str, bool)> = vec![
        ("Running", true),
        ("Blocked", true),
        ("Created", false),
        ("Scheduled", false),
        ("Stopping", false),
        ("Stopped", false),
        ("Failed", false),
        ("Expired", false),
        ("Deleting", false),
    ];

    for (status, accepts) in states {
        let session = format!("session-{}", status.to_lowercase());
        emulator.create_session(&session, status);
        for surface in surfaces() {
            let answer = submit(&emulator, &surface, &session).await;
            let case = format!("{} against a {status} session", surface.case);
            if accepts {
                assert_ne!(
                    answer.member("message"),
                    state::session_cannot_accept_requests(status),
                    "case {case:?}: a session in an accepting state must not be refused for \
                     its state, got {}",
                    answer.text()
                );
            } else {
                assert_eq!(
                    (answer.status, answer.member("message")),
                    (409, state::session_cannot_accept_requests(status)),
                    "case {case:?}: expected the modelled conflict naming the state, got {} \
                     {}",
                    answer.status,
                    answer.text()
                );
            }
        }
    }
}

/// RULE: the two asynchronous submits are SERVED by a session in an accepting state, so
/// the refusal above is about the state and not about the call.
#[tokio::test]
async fn a_session_in_an_accepting_state_is_served() {
    let emulator = Emulator::new();

    for status in ["Running", "Blocked"] {
        let session = format!("accepting-{}", status.to_lowercase());
        emulator.create_session(&session, status);
        for surface in surfaces().into_iter().take(3) {
            let answer = submit(&emulator, &surface, &session).await;
            let case = format!("{} against a {status} session", surface.case);
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the submit to be served, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                !answer.member("operationId").is_empty(),
                "case {case:?}: a served submit mints an operation id, got {}",
                answer.text()
            );
        }
    }
}

/// RULE: a session's state is the control plane's, including the transition the clock owes
/// it — so a session whose compute has not come up yet refuses work in the state it is
/// still in, and the same record accepts it once that instant has passed.
#[tokio::test]
async fn the_state_the_clock_owes_decides_the_answer() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    let lifecycle = service_common::training_sessions::TrainingSessionLifecycle::owing;

    // (case, the published lifecycle, the expected HTTP status)
    let rows = vec![
        (
            "a session whose compute has not come up yet",
            lifecycle("Created", "Running", now + Duration::from_secs(3_600)),
            409,
        ),
        (
            "a session whose compute came up",
            lifecycle("Created", "Running", now - Duration::from_secs(3_600)),
            200,
        ),
        (
            "a session that has finished stopping",
            lifecycle("Stopping", "Stopped", now - Duration::from_secs(3_600)),
            409,
        ),
    ];

    for (index, (case, lifecycle, expected)) in rows.into_iter().enumerate() {
        let session = format!("owed-{index}");
        emulator.publish_session(&session, lifecycle);
        let answer = submit(&emulator, &surfaces()[0], &session).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// RULE: a session the control plane never created does not exist, which is the modelled
/// 404 and not the state conflict — the two refusals are different answers.
#[tokio::test]
async fn a_session_nothing_created_is_not_found() {
    let emulator = Emulator::new();

    for surface in surfaces() {
        let answer = submit(&emulator, &surface, "never-created").await;
        assert_eq!(
            (answer.status, answer.member("message")),
            (
                404,
                format!("Session not found: never-created-{ACCOUNT}-{REGION}")
            ),
            "case {:?}: expected the modelled not-found naming the session identity, got {} \
             {}",
            surface.case,
            answer.status,
            answer.text()
        );
    }
}

/// RULE: a read reports what was accepted, so work that completed while the session was
/// accepting requests stays readable with its result unchanged after the session has left
/// that state — and the refusal of a new submit does not move the session either.
#[tokio::test]
async fn work_accepted_before_a_stop_stays_readable_after_it() {
    let emulator = Emulator::new();
    let session = "stopped-later";
    emulator.create_session(session, "Running");

    let accepted = submit(&emulator, &surfaces()[0], session).await;
    assert_eq!(
        accepted.status,
        200,
        "the operation must be accepted while the session is Running, got {}",
        accepted.text()
    );
    let id = accepted.member("operationId");
    let mut settled = None;
    for _ in 0..POLL_BUDGET {
        let answer = call(
            &emulator,
            "GET",
            &format!("/v1/training-sessions/{session}/sampling-operations/{id}"),
            &[],
        )
        .await;
        if matches!(answer.member("status").as_str(), "Completed" | "Failed" | "Skipped") {
            settled = Some(answer.json());
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let before = settled.expect("the accepted operation reaches a terminal status");

    // The control plane stops the session: what it publishes is what this plane reads.
    emulator.create_session(session, "Stopped");

    let refused = submit(&emulator, &surfaces()[0], session).await;
    assert_eq!(
        (refused.status, refused.member("message")),
        (409, state::session_cannot_accept_requests("Stopped")),
        "a stopped session refuses a new submit, got {} {}",
        refused.status,
        refused.text()
    );
    let after = call(
        &emulator,
        "GET",
        &format!("/v1/training-sessions/{session}/sampling-operations/{id}"),
        &[],
    )
    .await;
    assert_eq!(
        after.status,
        200,
        "the operation accepted before the stop must stay readable, got {} {}",
        after.status,
        after.text()
    );
    assert_eq!(
        after.json(),
        before,
        "the stored result must be unchanged by the stop and by the refusal, got {}",
        after.text()
    );
}
