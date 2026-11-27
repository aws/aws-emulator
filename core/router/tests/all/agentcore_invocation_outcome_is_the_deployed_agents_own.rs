// SPDX-License-Identifier: CC0-1.0

//! What an invocation of a deployed agent runtime ANSWERS is decided by what the deployed
//! code did — never by how much host work this emulator spent getting there.
//!
//! The rule under test: **an invocation of an agent runtime deployed from a
//! `codeConfiguration` artifact is answered with what that code produced, and is refused on a
//! bound only when the RUN itself spent one — the span of an invocation is spent by the
//! agent's own doings (the events it delivers and the waits it asks for), while standing the
//! interpreter up and reading the deployment's modules is this emulator's own work on
//! whatever host it is running on and spends none of it.** Four crates have to agree for one
//! row to hold (Amazon S3 holds the code artifact, IAM holds the execution role, the
//! AgentCore control plane owns the runtime, the AgentCore data plane serves the invocation),
//! so the suite lives here.
//!
//! The rows differ along how much work one invocation COSTS, which is what the rule
//! generalises over: an agent that answers from its entry point alone, one that imports a
//! module of the standard library first, one that computes over a great many traced events
//! first, and one that asks for a wait first. Every row carries a marker of its own and
//! asserts the answer is what the code made of THAT marker, so no body this emulator composed
//! could pass a row. The last row crosses the span one invocation has — by ASKING to wait
//! longer than it — and is refused on that span, which is what says the span is the run's own
//! and is spent by what the run asked for.
//!
//! Nothing here waits a fixed time and nothing is asserted about the wall clock: a wait an
//! agent asks for is charged to the invocation's own span, so even the row that crosses that
//! span is answered as soon as the engine reads it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery percent-encoder and the XML reader every awsQuery suite in this crate shares.
use crate::query_wire::{form_encode, opt_element};
// The `@httpLabel` percent-encoder the AgentCore data-plane suites share.
use crate::bedrock_agentcore_data_plane_parent_resources::label;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The signing name both AgentCore planes share (`sigv4Name: bedrock-agentcore`): the route
/// decides which of the two serves a request.
const AGENTCORE: &str = "bedrock-agentcore";

/// The service principal an execution role of an agent runtime has to admit.
const AGENTCORE_PRINCIPAL: &str = "bedrock-agentcore.amazonaws.com";

/// The bucket every code artifact of this suite is stored in.
const CODE_BUCKET: &str = "agentcore-run-cost-code";

/// The `AgentManagedRuntimeType` the deployed code is run under (source: smithy_query
/// query="shape" shape="AgentManagedRuntimeType", values PYTHON_3_10 | PYTHON_3_11 |
/// PYTHON_3_12 | PYTHON_3_13 | PYTHON_3_14 | NODE_22).
const PYTHON_RUNTIME: &str = "PYTHON_3_13";

/// A `runtimeSessionId` of the length that member admits (`SessionType`, 33..256).
const SESSION: &str = "run-cost-agent-session-0000000001";

/// How many turns the agent that COMPUTES before it answers takes, and the remainder it sums
/// over them: enough traced events that the run costs this host real work, and far inside the
/// step budget one invocation is given.
const COMPUTED_TURNS: u64 = 20_000;
/// See [`COMPUTED_TURNS`].
const COMPUTED_MODULUS: u64 = 7;

/// How long the agent that WAITS inside its own span asks to wait for, in seconds: a wait one
/// invocation has room for.
const WAIT_INSIDE_THE_SPAN: u64 = 1;

/// How long the agent that waits PAST its own span asks to wait for, in seconds: more than
/// any span one invocation is given, so the invocation's own span is what it runs out of.
const WAIT_PAST_THE_SPAN: u64 = 600;

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=00000000000000000000000000000000000000\
         00000000000000000000000000"
    )
}

/// One request against Amazon S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// `PUT /<bucket>` — S3's `CreateBucket`, which holds every artifact of this suite.
async fn create_bucket(router: &MultiServiceRouter, case: &str) {
    let (status, answered) = s3(router, "PUT", &format!("/{CODE_BUCKET}"), "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBucket({CODE_BUCKET:?}) must succeed, got {status} with {answered}"
    );
}

/// `PUT /<bucket>/<key>` — S3's `PutObject`, which puts the agent's code where its artifact
/// names it.
async fn put_object(router: &MultiServiceRouter, case: &str, key: &str, body: &str) {
    let (status, answered) = s3(router, "PUT", &format!("/{CODE_BUCKET}/{key}"), body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutObject({key:?}) must succeed, got {status} with {answered}"
    );
}

/// Create one IAM role the control plane can resolve, and answer its ARN.
async fn create_role(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": AGENTCORE_PRINCIPAL},
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string();
    let mut body = "Action=CreateRole&Version=2010-05-08".to_string();
    for (key, value) in [
        ("RoleName", name),
        ("AssumeRolePolicyDocument", policy.as_str()),
    ] {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let created = String::from_utf8_lossy(response.body()).into_owned();
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole({name:?}) must succeed, got {status} with {created}"
    );
    opt_element(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole answered no ARN: {created}"))
}

/// `PUT /runtimes/` — the control plane's `CreateAgentRuntime`, deployed from the code
/// artifact stored at `prefix`. Answers the ARN that service minted.
async fn create_runtime(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
    prefix: &str,
) -> String {
    let payload = json!({
        "agentRuntimeName": name,
        "roleArn": role_arn,
        "agentRuntimeArtifact": {
            "codeConfiguration": {
                "code": {"s3": {"bucket": CODE_BUCKET, "prefix": prefix}},
                "runtime": PYTHON_RUNTIME,
                "entryPoint": ["main.py"],
            }
        },
        "networkConfiguration": {"networkMode": "PUBLIC"},
        "protocolConfiguration": {"serverProtocol": "HTTP"}
    })
    .to_string();
    let request = http::Request::builder()
        .method("PUT")
        .uri("/runtimes/")
        .header(
            "host",
            format!("bedrock-agentcore-control.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/json")
        .header("authorization", authorization(AGENTCORE))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let created: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAgentRuntime({name:?}) must be accepted, got {status} with \
         {created}"
    );
    created["agentRuntimeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAgentRuntime answered no ARN: {created}"))
        .to_string()
}

/// What one invocation of the data plane answered.
#[derive(Debug)]
struct Invoked {
    status: u16,
    body: Vec<u8>,
}

impl Invoked {
    /// The answer's payload as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The answer as the document the deployed code returned, or `Null`.
    fn document(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The error code a refused invocation carries, read off the `__type` its restJson1 error
    /// document names the shape in.
    fn error_code(&self) -> String {
        self.document()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The `message` a refused invocation carries.
    fn message(&self) -> String {
        self.document()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// `POST /runtimes/{agentRuntimeArn}/invocations` — `InvokeAgentRuntime`, with the payload as
/// the request body and the media types in the headers the model binds them to.
async fn invoke(router: &MultiServiceRouter, agent_runtime_arn: &str, payload: &str) -> Invoked {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/runtimes/{}/invocations", label(agent_runtime_arn)))
        .header("host", format!("bedrock-agentcore.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .header("x-amzn-bedrock-agentcore-runtime-session-id", SESSION)
        .header("authorization", authorization(AGENTCORE))
        .body(Bytes::from(payload.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Invoked {
        status: response.status().as_u16(),
        body: response.into_body().to_vec(),
    }
}

/// An agent that answers from its entry point alone: the cheapest run there is.
const PLAIN_AGENT: &str = r#"
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    return {"answer": payload["prompt"].upper(), "cost": "none"}


app.run()
"#;

/// An agent that IMPORTS a module of the standard library before it answers: the import chain
/// is read in the interpreter this emulator stands up, which is this emulator's own work.
const IMPORTING_AGENT: &str = r#"
from http.server import BaseHTTPRequestHandler
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    return {
        "answer": payload["prompt"].upper(),
        "cost": "import",
        "imported": BaseHTTPRequestHandler.__name__,
    }


app.run()
"#;

/// An agent that COMPUTES over a great many traced events before it answers, and answers the
/// total it computed — a value this suite computes for itself, so a body that did not come
/// from the run cannot pass the row.
fn computing_agent() -> String {
    format!(
        r#"
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    total = 0
    for turn in range({COMPUTED_TURNS}):
        total += turn % {COMPUTED_MODULUS}
    return {{"answer": payload["prompt"].upper(), "cost": "events", "total": total}}


app.run()
"#
    )
}

/// An agent that asks for a WAIT before it answers, inside the span one invocation has.
fn waiting_agent(seconds: u64) -> String {
    format!(
        r#"
import time

from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    time.sleep({seconds})
    return {{"answer": payload["prompt"].upper(), "cost": "wait"}}


app.run()
"#
    )
}

/// The total [`computing_agent`] computes, as this suite computes it.
fn computed_total() -> u64 {
    (0..COMPUTED_TURNS).map(|turn| turn % COMPUTED_MODULUS).sum()
}

/// RULE: an invocation of a deployed agent runtime is answered with what the deployed code
/// produced, however much work the run cost — the span of an invocation is spent by the
/// agent's own doings and not by what this emulator spent standing the interpreter up and
/// reading the deployment's modules.
///
/// The rows differ along that cost: no work at all, an import chain of the standard library,
/// a great many traced events, and a wait the agent asked for.
#[tokio::test]
async fn an_invocation_is_answered_however_much_work_the_run_cost() {
    let router = build_router();
    create_bucket(&router, "the shared code bucket").await;

    // (case, the runtime's name, the deployed source, what the answer's `cost` reports)
    let rows: [(&str, &str, String, &str); 4] = [
        (
            "an agent that answers from its entry point alone",
            "plainAgent",
            PLAIN_AGENT.to_string(),
            "none",
        ),
        (
            "an agent that imports a module of the standard library before it answers",
            "importingAgent",
            IMPORTING_AGENT.to_string(),
            "import",
        ),
        (
            "an agent that computes over many traced events before it answers",
            "computingAgent",
            computing_agent(),
            "events",
        ),
        (
            "an agent that asks for a wait inside its own span before it answers",
            "waitingAgent",
            waiting_agent(WAIT_INSIDE_THE_SPAN),
            "wait",
        ),
    ];

    for (case, name, source, cost) in rows {
        let role = create_role(&router, case, &format!("{name}-role")).await;
        let key = format!("{name}/main.py");
        put_object(&router, case, &key, &source).await;
        let runtime_arn = create_runtime(&router, case, name, &role, &key).await;

        // A marker of this row's own, so the answer has to have been made from THIS request.
        let marker = format!("{name}-marker");
        let answered = invoke(
            &router,
            &runtime_arn,
            &json!({"prompt": marker}).to_string(),
        )
        .await;
        let answered_text = answered.text();
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200 from InvokeAgentRuntime, got {} with \
             {answered_text:?}",
            answered.status
        );
        let document = answered.document();
        assert_eq!(
            document["answer"].as_str(),
            Some(marker.to_uppercase().as_str()),
            "case {case:?}: expected the deployed code's answer for {marker:?}, got \
             {answered_text:?}"
        );
        assert_eq!(
            document["cost"].as_str(),
            Some(cost),
            "case {case:?}: expected the answer of the agent this row deployed, got \
             {answered_text:?}"
        );
        if cost == "events" {
            assert_eq!(
                document["total"].as_u64(),
                Some(computed_total()),
                "case {case:?}: expected the total the run computed, got {answered_text:?}"
            );
        }
        if cost == "import" {
            assert_eq!(
                document["imported"].as_str(),
                Some("BaseHTTPRequestHandler"),
                "case {case:?}: expected the module the run imported to have been read, got \
                 {answered_text:?}"
            );
        }
    }
}

/// RULE: the span one invocation has is the RUN's own and is spent by what the run asks for,
/// so an agent that asks to wait longer than that span runs out of it and the invocation is
/// refused as the dependency failure it is — the modelled `RuntimeClientError` (424), worded
/// on the span the agent ran past rather than on anything this host spent.
#[tokio::test]
async fn an_agent_that_waits_past_its_own_span_is_refused_on_that_span() {
    let router = build_router();
    let case = "an agent that asks to wait longer than one invocation's own span";
    create_bucket(&router, case).await;
    let role = create_role(&router, case, "oversleepingAgent-role").await;
    let key = "oversleepingAgent/main.py";
    put_object(&router, case, key, &waiting_agent(WAIT_PAST_THE_SPAN)).await;
    let runtime_arn = create_runtime(&router, case, "oversleepingAgent", &role, key).await;

    let answered = invoke(
        &router,
        &runtime_arn,
        &json!({"prompt": "slept-too-long"}).to_string(),
    )
    .await;
    let answered_text = answered.text();
    assert_eq!(
        (answered.status, answered.error_code().as_str()),
        (424, "RuntimeClientError"),
        "case {case:?}: expected the modelled 424 RuntimeClientError, got {} with \
         {answered_text:?}",
        answered.status
    );
    let message = answered.message();
    assert!(
        message.contains("did not answer within"),
        "case {case:?}: expected the refusal to be worded on the span the agent ran past, got \
         {message:?}"
    );
}
