// SPDX-License-Identifier: CC0-1.0

//! How this DATA PLANE resolves the training session a request names.
//!
//! RULE: a training session exists only because the SageMaker control plane created it —
//! this runtime API models no create at all — so every data-plane call resolves the name
//! it carries against the sessions the control plane holds, and one that resolves to
//! nothing is answered `404 ResourceNotFoundException` with the session's own identity
//! (`Session not found: <trainingSessionName>-<accountId>-<region>`), never served
//! against a record this service invented for it.
//!
//! The rows span what the rule generalises over rather than one recorded call: every
//! surface that acts on a session (both submits, the synchronous sample and the streaming
//! one), the shape of the name (shortest, longest, a case difference from a session that
//! does exist, and — on the surface whose labels are resolved rather than syntax-checked —
//! a name outside the modelled identifier syntax), and the Region the call is signed for,
//! which is part of the identity the refusal states. Each surface is then driven against a
//! session the control plane DID create, so the gate is shown to be resolution and not a
//! blanket refusal.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The account id every generated emulator presents, which is the account half of the
/// session identity a refusal states.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region most rows sign for.
const REGION: &str = "us-west-2";

/// One emulator: a shared engine with this service mounted on the production router.
///
/// The engine is held beside the router because the control plane that creates a session
/// is another crate: a case registers the sessions it is about through
/// [`Emulator::create_session`], which is what `CreateTrainingSession` does.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The cases assert behaviour, not signatures; the emulator verifies signatures
        // itself and a service never checks them.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Register `name` the way the SageMaker control plane's `CreateTrainingSession`
    /// does: the one call that makes a training session exist.
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

    /// The wire error code a client branches on: the `x-amzn-errortype` header the
    /// restJson1 envelope carries, falling back to the body's `__type`.
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

    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Send one request whose SigV4 credential scope names `region`, which is the Region the
/// session identity in a refusal is composed from.
async fn call_in(
    emulator: &Emulator,
    method: &str,
    path: &str,
    body: &[u8],
    region: &str,
) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/aws4_request, \
         SignedHeaders=host, Signature=00"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://training-session-runtime.sagemaker.{region}.amazonaws.com{path}"
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

/// One surface that acts on a training session: where it is addressed, what a valid
/// request to it carries, and what it answers for a session that DOES exist but holds
/// nothing yet.
struct Surface {
    case: &'static str,
    method: &'static str,
    /// `{}` stands for the session name in the path.
    path: &'static str,
    body: fn() -> Vec<u8>,
    /// Whether this surface resolves a label outside the modelled identifier syntax
    /// rather than refusing it at the boundary.
    resolves_any_label: bool,
    /// The status a session the control plane created is answered with: the submits
    /// accept, and both inference surfaces conflict because the session holds no
    /// checkpoint to sample from.
    created_status: u16,
}

/// A valid `ExecuteSamplingOperation` body.
fn sampling_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "operationConfig": {"sample": {
            "prompt": {"chunks": [{"tokens": [7, 8], "type": "encoded_text"}]},
            "numSamples": 1,
            "samplingParams": {"maxTokens": 512, "temperature": 0.7},
        }},
    }))
    .expect("the body serializes")
}

/// A valid `ExecuteTrainingOperation` body.
fn training_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "sequenceNumber": 1,
        "operationConfig": {"saveState": {"description": "a resumable checkpoint"}},
    }))
    .expect("the body serializes")
}

/// The inference surfaces take an opaque payload.
fn payload() -> Vec<u8> {
    b"{}".to_vec()
}

/// Every surface of this service that acts on a named session.
fn surfaces() -> Vec<Surface> {
    vec![
        Surface {
            case: "ExecuteSamplingOperation",
            method: "POST",
            path: "/v1/training-sessions/{}/sampling-operations",
            body: sampling_body,
            resolves_any_label: false,
            created_status: 200,
        },
        Surface {
            case: "ExecuteTrainingOperation",
            method: "POST",
            path: "/v1/training-sessions/{}/operations",
            body: training_body,
            resolves_any_label: false,
            created_status: 200,
        },
        Surface {
            case: "SampleTrainingSession",
            method: "POST",
            path: "/training-sessions/{}/sample",
            body: payload,
            resolves_any_label: false,
            created_status: 409,
        },
        Surface {
            case: "SampleTrainingSessionWithResponseStream",
            method: "POST",
            path: "/training-sessions/{}/sample-with-response-stream",
            body: payload,
            resolves_any_label: true,
            created_status: 409,
        },
    ]
}

/// The path a surface is addressed at for `session`.
fn path_for(surface: &Surface, session: &str) -> String {
    surface.path.replace("{}", session)
}

/// RULE: a data-plane call naming a session the control plane never created is answered
/// `404 ResourceNotFoundException` with `Session not found: <name>-<account>-<region>`,
/// on every surface that acts on a session.
#[tokio::test]
async fn a_call_against_a_session_nothing_created_is_not_found() {
    // (case, the session name, whether the name is inside the modelled identifier syntax)
    // `TrainingSessionName` is `@length(1..64)` with pattern `^[a-zA-Z0-9-]+$`
    // (smithy_query(query="shape",shape="TrainingSessionName")), so the rows walk the
    // shortest name, the longest one, a name differing only in case from a session that
    // does exist, and two outside that syntax.
    let rows: [(&str, String, bool); 6] = [
        ("a name nothing created", "never-created".to_string(), true),
        ("the shortest name there is", "a".to_string(), true),
        ("the longest name there is", "n".repeat(64), true),
        (
            "a name differing only in case from one that exists",
            "Created".to_string(),
            true,
        ),
        (
            "a name outside the identifier syntax",
            "bad_name".to_string(),
            false,
        ),
        (
            "a name one character over the length maximum",
            "n".repeat(65),
            false,
        ),
    ];

    for surface in surfaces() {
        let emulator = Emulator::new();
        // One session that does exist, so a near miss is a real miss and not an empty
        // store answering everything the same way.
        emulator.create_session("created");

        for (case, session, well_formed) in &rows {
            if !well_formed && !surface.resolves_any_label {
                continue;
            }
            let answer = call_in(
                &emulator,
                surface.method,
                &path_for(&surface, session),
                &(surface.body)(),
                REGION,
            )
            .await;
            assert_eq!(
                answer.status,
                404,
                "case {} / {case:?}: expected HTTP 404 for a session nothing created, \
                 got {} {}",
                surface.case,
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {} / {case:?}: expected the modelled not-found code, got {}",
                surface.case,
                answer.text()
            );
            assert_eq!(
                answer.message(),
                format!("Session not found: {session}-{ACCOUNT}-{REGION}"),
                "case {} / {case:?}: expected the session identity in the refusal, got {:?}",
                surface.case,
                answer.message()
            );
        }

        // The same surface serves the session that WAS created, so the refusal above is
        // resolution and not a blanket one.
        let served = call_in(
            &emulator,
            surface.method,
            &path_for(&surface, "created"),
            &(surface.body)(),
            REGION,
        )
        .await;
        assert_eq!(
            served.status,
            surface.created_status,
            "case {}: a session the control plane created must be served, got {} {}",
            surface.case,
            served.status,
            served.text()
        );
    }
}

/// RULE: the Region half of the identity is the one the call was signed for, so the same
/// unknown session is reported under whichever Region asked for it.
#[tokio::test]
async fn the_refusal_states_the_region_the_call_was_signed_for() {
    let emulator = Emulator::new();

    for region in ["us-west-2", "eu-central-1", "ap-southeast-3"] {
        let answer = call_in(
            &emulator,
            "POST",
            "/v1/training-sessions/elsewhere/sampling-operations",
            &sampling_body(),
            region,
        )
        .await;
        assert_eq!(
            answer.message(),
            format!("Session not found: elsewhere-{ACCOUNT}-{region}"),
            "case {region:?}: expected the signing Region in the session identity, got {:?}",
            answer.message()
        );
    }
}

/// RULE: a refusal creates nothing — a submit refused for an unresolvable session leaves
/// the session as absent as it was, so the operation id it would have minted exists
/// nowhere and the next identical call is refused identically.
#[tokio::test]
async fn a_refused_submit_creates_no_session() {
    let emulator = Emulator::new();
    let session = "phantom";

    let first = call_in(
        &emulator,
        "POST",
        &format!("/v1/training-sessions/{session}/sampling-operations"),
        &sampling_body(),
        REGION,
    )
    .await;
    assert_eq!(
        first.status,
        404,
        "case the first submit: expected HTTP 404, got {} {}",
        first.status,
        first.text()
    );
    assert!(
        first.json().get("operationId").is_none(),
        "case the first submit: a refusal mints no operation id, got {}",
        first.text()
    );

    // Nothing was stored for the session, so the same call is answered the same way, and
    // the training surface agrees.
    for (case, path, body) in [
        (
            "the same submit again",
            format!("/v1/training-sessions/{session}/sampling-operations"),
            sampling_body(),
        ),
        (
            "the training surface",
            format!("/v1/training-sessions/{session}/operations"),
            training_body(),
        ),
    ] {
        let answer = call_in(&emulator, "POST", &path, &body, REGION).await;
        assert_eq!(
            (answer.status, answer.message()),
            (
                404,
                format!("Session not found: {session}-{ACCOUNT}-{REGION}")
            ),
            "case {case:?}: expected the same refusal, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // And once the control plane does create it, the very same submit is served: what was
    // missing was the session, not the request.
    emulator.create_session(session);
    let served = call_in(
        &emulator,
        "POST",
        &format!("/v1/training-sessions/{session}/sampling-operations"),
        &sampling_body(),
        REGION,
    )
    .await;
    assert_eq!(
        served.status,
        200,
        "case the session created: expected the submit to be served, got {} {}",
        served.status,
        served.text()
    );
    assert!(
        served
            .json()
            .get("operationId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("sampling-")),
        "case the session created: expected a minted sampling operation id, got {}",
        served.text()
    );
}

/// RULE: the submit surfaces still refuse a label outside the modelled identifier syntax
/// at the boundary (`400 ValidationException`), which is the asymmetry with the surfaces
/// that resolve it: the same name reaches two different refusals by design.
#[tokio::test]
async fn a_malformed_label_diverges_between_the_submit_and_the_streaming_surface() {
    let emulator = Emulator::new();
    let malformed = "bad_name";

    let submit = call_in(
        &emulator,
        "POST",
        &format!("/v1/training-sessions/{malformed}/sampling-operations"),
        &sampling_body(),
        REGION,
    )
    .await;
    assert_eq!(
        (submit.status, submit.error_code()),
        (400, "ValidationException".to_string()),
        "case the submit surface: expected the boundary refusal, got {} {}",
        submit.status,
        submit.text()
    );
    assert_eq!(
        submit.message(),
        format!(
            "1 validation error detected: Value '{malformed}' at 'trainingSessionName' \
             failed to satisfy constraint: Member must satisfy regular expression \
             pattern: [a-zA-Z0-9-]+"
        ),
        "case the submit surface: expected the pattern requirement, got {:?}",
        submit.message()
    );

    let streamed = call_in(
        &emulator,
        "POST",
        &format!("/training-sessions/{malformed}/sample-with-response-stream"),
        &payload(),
        REGION,
    )
    .await;
    assert_eq!(
        (streamed.status, streamed.error_code()),
        (404, "ResourceNotFoundException".to_string()),
        "case the streaming surface: expected the resolution refusal, got {} {}",
        streamed.status,
        streamed.text()
    );
    assert_eq!(
        streamed.message(),
        format!("Session not found: {malformed}-{ACCOUNT}-{REGION}"),
        "case the streaming surface: expected the session identity, got {:?}",
        streamed.message()
    );
}
