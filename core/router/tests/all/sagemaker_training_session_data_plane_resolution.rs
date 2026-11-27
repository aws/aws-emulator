// SPDX-License-Identifier: CC0-1.0

//! The training-session RESOLUTION hand-off: the SageMaker CONTROL plane is the only
//! thing that makes a training session exist, and the SageMaker Training Session Runtime
//! DATA plane serves a call only against a session it created — both over the wire, with
//! no in-process help.
//!
//! RULE: a data-plane call resolves the resource it names against the component that OWNS
//! it, so a name that component does not hold is answered with the modelled not-found
//! rather than served against a record the data plane invented. Here the owner is the
//! SageMaker control plane (`CreateTrainingSession` / `DescribeTrainingSession`) and the
//! data plane is this runtime's submit surfaces, whose API models no create at all.
//!
//! Only this suite can observe that hand-off: each half is a separate crate with a
//! separate store, and the two agree solely because the control plane records the sessions
//! it creates in ONE shared directory (`service_common::training_sessions`) the data plane
//! reads. So both halves are driven through the SAME [`MultiServiceRouter`] and the SAME
//! engine `core/router/src/lib.rs` mounts them on, and every session here comes to exist
//! the way a client makes one: an HTTP `CreateTrainingSession`.
//!
//! The rows span what the rule generalises over rather than one recorded call: both submit
//! surfaces, a session created and a name never created, the state the session is in when
//! the call arrives (still provisioning, then running), and the order the two planes are
//! called in (a refusal first, then the create, then the same call again).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for, which is also the Region the data plane's
/// not-found message composes the session identity from.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The host every request is addressed to; the claim each service makes is resolved from
/// the SigV4 credential scope and the URI path space, not from this name.
const HOST: &str = "emulator.local";

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
async fn create_session(router: &MultiServiceRouter, name: &str) -> Answer {
    control_plane(
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
    .await
}

/// `DescribeTrainingSession` — what the OWNER of the resource says about the name.
async fn describe_session(router: &MultiServiceRouter, name: &str) -> Answer {
    control_plane(
        router,
        "DescribeTrainingSession",
        &json!({ "TrainingSessionName": name }),
    )
    .await
}

/// Wait until the control plane reports `name` as `Running`.
///
/// A create "provisions compute resources", so the session passes through `Created` first
/// and accepts no operation until that compute is up (`ConflictException`: "The session
/// must be in Running or Blocked state to accept requests"). The wait is for the
/// CONDITION: the transition is owed at an instant the control plane stored, so the loop
/// looks again until it has happened or a generous deadline passes.
async fn await_running(router: &MultiServiceRouter, name: &str) {
    /// Far above the control plane's own transition span, so a loaded host still passes.
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
    /// How long the loop waits before looking again.
    const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(25);

    let started = std::time::Instant::now();
    loop {
        let answer = describe_session(router, name).await;
        let status = answer.json()["TrainingSessionDetail"]["TrainingSessionStatus"]
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

/// One data-plane submit surface: where it is addressed and what a valid body carries.
struct Surface {
    case: &'static str,
    path: &'static str,
    body: fn() -> Vec<u8>,
    /// The URI segment a minted operation of this surface is read back under.
    read_route: &'static str,
}

fn sampling_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "operationConfig": {"sample": {
            "prompt": {"chunks": [{"tokens": [7, 8], "type": "encoded_text"}]},
            "numSamples": 1,
            "samplingParams": {"maxTokens": 512, "temperature": 0.7},
        }},
    }))
    .expect("a serializable data-plane body")
}

fn training_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "sequenceNumber": 1,
        "operationConfig": {"saveState": {"description": "a resumable checkpoint"}},
    }))
    .expect("a serializable data-plane body")
}

fn surfaces() -> Vec<Surface> {
    vec![
        Surface {
            case: "ExecuteSamplingOperation",
            path: "/v1/training-sessions/{}/sampling-operations",
            body: sampling_body,
            read_route: "sampling-operations",
        },
        Surface {
            case: "ExecuteTrainingOperation",
            path: "/v1/training-sessions/{}/operations",
            body: training_body,
            read_route: "operations",
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
        (surface.body)(),
    )
    .await
}

/// RULE: the data plane serves a submit against a session the control plane created, and
/// refuses one against a name it never created — the two answers for the same request
/// differing only in whether the create was made.
#[tokio::test]
async fn the_data_plane_serves_only_the_sessions_the_control_plane_created() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    for surface in surfaces() {
        let created = format!("created-for-{}", surface.read_route);
        let absent = format!("absent-for-{}", surface.read_route);

        // Before any create, the data plane resolves NOTHING: the refusal states the
        // session's identity, and the control plane agrees it holds no such session.
        let refused = submit(&router, &surface, &created).await;
        assert_eq!(
            refused.status,
            404,
            "case {}: a submit before the create must be refused, got {} {}",
            surface.case,
            refused.status,
            refused.text()
        );
        assert_eq!(
            refused.member("message"),
            format!("Session not found: {created}-{ACCOUNT}-{REGION}"),
            "case {}: expected the session identity in the refusal, got {}",
            surface.case,
            refused.text()
        );
        let unknown = describe_session(&router, &created).await;
        assert_ne!(
            unknown.status, 200,
            "case {}: the control plane must not hold a session nothing created, got {}",
            surface.case,
            unknown.text()
        );

        // The create is what makes it exist, and the SAME submit is then served.
        let create = create_session(&router, &created).await;
        assert_eq!(
            create.status,
            200,
            "case {}: expected the create to succeed, got {} {}",
            surface.case,
            create.status,
            create.text()
        );
        let provisioning = submit(&router, &surface, &created).await;
        assert_eq!(
            provisioning.status,
            409,
            "case {}: a session whose compute is still being provisioned exists but accepts \
             no operation yet, which is the conflict and no longer the not-found, got {} {}",
            surface.case,
            provisioning.status,
            provisioning.text()
        );
        // And once its compute is up it accepts the SAME submit.
        await_running(&router, &created).await;
        let served = submit(&router, &surface, &created).await;
        assert_eq!(
            served.status,
            200,
            "case {}: a session the control plane created must be served, got {} {}",
            surface.case,
            served.status,
            served.text()
        );
        let operation_id = served.member("operationId");
        assert!(
            !operation_id.is_empty(),
            "case {}: expected the submit to mint an operation id, got {}",
            surface.case,
            served.text()
        );
        // The success is backed by state: the operation it minted resolves on the read
        // route of the same session.
        let polled = dispatch(
            &router,
            "GET",
            &format!(
                "/v1/training-sessions/{created}/{}/{operation_id}",
                surface.read_route
            ),
            &[("content-type", "application/json")],
            Vec::new(),
        )
        .await;
        assert_eq!(
            polled.status,
            200,
            "case {}: the minted operation must resolve, got {} {}",
            surface.case,
            polled.status,
            polled.text()
        );

        // And a name that was never created stays refused, so what changed above was the
        // create and not the call.
        let still_refused = submit(&router, &surface, &absent).await;
        assert_eq!(
            (still_refused.status, still_refused.member("message")),
            (
                404,
                format!("Session not found: {absent}-{ACCOUNT}-{REGION}")
            ),
            "case {}: expected a name nothing created to stay refused, got {} {}",
            surface.case,
            still_refused.status,
            still_refused.text()
        );
        // The refused submit created no session on either side.
        let after = describe_session(&router, &absent).await;
        assert_ne!(
            after.status, 200,
            "case {}: a refused submit must leave the control plane holding nothing, got {}",
            surface.case,
            after.text()
        );
    }
}
