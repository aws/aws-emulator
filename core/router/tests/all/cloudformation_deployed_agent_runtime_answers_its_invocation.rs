// SPDX-License-Identifier: CC0-1.0

//! An agent runtime deployed from a CODE artifact — by a CloudFormation stack, or through
//! the control plane's own create — really answers an invocation with what its deployed code
//! returned.
//!
//! The rule under test: **an invocation of an agent runtime whose artifact is a
//! `codeConfiguration` delivers the request's payload to the entry point that code serves
//! and answers 200 with the body that code returned, in the media type it answered in; a
//! deployment nothing in this emulator can run answers no body at all, with the modelled
//! `RuntimeClientError`.** Five crates have to agree for the stack half to hold
//! (CloudFormation deploys the stack, IAM holds the execution role, Amazon S3 holds the code
//! artifact, the AgentCore control plane owns the runtime, the AgentCore data plane serves
//! the invocation), so the suite lives here: a crate-local test can only ever see one side
//! of it.
//!
//! The rows differ along what the rule generalises over: how the deployed code serves its
//! entry point (registered with the AgentCore app it imports, a module-level callable, a
//! callable the artifact's `entryPoint` names itself, one that takes the invocation's
//! context beside the payload), how the artifact is stored (one object at the prefix, or
//! several objects under it where the entry module imports a sibling), what media type the
//! payload is sent in, and what the code returns (a document, text, or a stream of chunks).
//! Every row sends a marker of its own in the payload and asserts the answer carries what
//! the code made of THAT marker, so a body this emulator composed itself cannot pass a row.
//!
//! Nothing here waits a fixed time: the one wait, for a stack to settle, polls the condition
//! until it holds or a generous deadline passes ([`poll_until`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery percent-encoder and the XML readers every awsQuery suite in this crate
// shares, written once in `all/query_wire.rs`.
use crate::query_wire::{form_encode, opt_element, outputs_of};
// The `@httpLabel` percent-encoder the AgentCore data-plane suites share.
use crate::bedrock_agentcore_data_plane_parent_resources::label;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The signing name both AgentCore planes share (`sigv4Name: bedrock-agentcore`): the route
/// decides which of the two serves a request.
const AGENTCORE: &str = "bedrock-agentcore";

/// The service principal an execution role of an agent runtime has to admit.
const AGENTCORE_PRINCIPAL: &str = "bedrock-agentcore.amazonaws.com";

/// The bucket every code artifact in this suite is stored in.
const CODE_BUCKET: &str = "agentcore-deployed-code";

/// The `AgentManagedRuntimeType` the deployed code is run under (source: smithy_query
/// query="shape" shape="AgentManagedRuntimeType", values PYTHON_3_10 | PYTHON_3_11 |
/// PYTHON_3_12 | PYTHON_3_13 | PYTHON_3_14 | NODE_22).
const PYTHON_RUNTIME: &str = "PYTHON_3_13";

/// A `runtimeSessionId` of the length that member admits (`SessionType`, 33..256).
const SESSION: &str = "deployed-agent-session-0000000001";

/// How long the one clock-owed transition — a stack settling — is waited for before the wait
/// is a failure: generously more than the spans the stack and the control plane owe, so a
/// loaded host still reaches the condition.
const SETTLE_DEADLINE: Duration = Duration::from_secs(90);

/// How often a wait looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The emulator with the five services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
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

/// `PUT /<bucket>/<key>` — S3's `PutObject`, the upload that puts the agent's code where its
/// artifact names it.
async fn put_object(router: &MultiServiceRouter, case: &str, key: &str, body: &str) {
    let (status, answered) = s3(router, "PUT", &format!("/{CODE_BUCKET}/{key}"), body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutObject({key:?}) must succeed, got {status} with {answered}"
    );
}

/// One CloudFormation action over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM action over its own awsQuery wire.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in members {
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
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The trust policy an execution role of an agent runtime carries.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": AGENTCORE_PRINCIPAL},
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// Create one IAM role the control plane can resolve, and answer its ARN.
async fn create_role(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole({name:?}) must succeed, got {status} with {created}"
    );
    opt_element(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole answered no ARN: {created}"))
}

/// One AgentCore CONTROL-plane operation over its restJson1 wire.
async fn agentcore_control(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|document| document.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
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
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// What one invocation of the DATA plane answered: the status, the `Content-Type` the answer
/// reports and the `response` payload as it arrived.
#[derive(Debug)]
struct Invoked {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

impl Invoked {
    /// The answer's payload as text, which every agent of this suite answers in.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The error code a refused invocation carries, read off the `__type` its restJson1
    /// error document names the shape in.
    fn error_code(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .unwrap_or(Value::Null)
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// `POST /runtimes/{agentRuntimeArn}/invocations` — `InvokeAgentRuntime`, with the payload as
/// the request body and the media types in the headers the model binds them to.
async fn invoke(
    router: &MultiServiceRouter,
    agent_runtime_arn: &str,
    content_type: &str,
    accept: &str,
    payload: &str,
) -> Invoked {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/runtimes/{}/invocations", label(agent_runtime_arn)))
        .header("host", format!("bedrock-agentcore.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("accept", accept)
        .header("x-amzn-bedrock-agentcore-runtime-session-id", SESSION)
        .header("authorization", authorization(AGENTCORE))
        .body(Bytes::from(payload.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let (parts, body) = response.into_parts();
    let content_type = parts
        .headers
        .get("content-type")
        .and_then(|held| held.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Invoked {
        status,
        content_type,
        body: body.to_vec(),
    }
}

/// Look again until `condition` holds or [`SETTLE_DEADLINE`] passes, panicking with `case`
/// and whatever the last look saw.
async fn poll_until<F, Fut>(case: &str, waiting_for: &str, mut look: F) -> String
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = (bool, String)>,
{
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (held, seen) = look().await;
        if held {
            return seen;
        }
        if Instant::now() >= deadline {
            panic!("case {case:?}: timed out waiting for {waiting_for}; last saw {seen}");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a stack and answer the `DescribeStacks` body once it rests in a terminal status.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    poll_until(case, "the stack to rest in a terminal status", || async {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        if status != 200 {
            return (false, format!("HTTP {status} from DescribeStacks: {body}"));
        }
        let settled = opt_element(&body, "StackStatus")
            .is_some_and(|held| held.ends_with("_COMPLETE") || held.ends_with("_FAILED"));
        (settled, body)
    })
    .await
}

/// One output the stack published, as the text it carries.
fn published(case: &str, body: &str, key: &str) -> String {
    outputs_of(body)
        .get(key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
        .clone()
}

/// The agent a stack deploys: it registers its entry point with the AgentCore app it
/// imports, which is how an agent serves `POST /invocations`, and answers a document made
/// from the payload it was handed.
const APP_AGENT: &str = r#"
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    prompt = payload.get("prompt", "")
    return {"answer": prompt.upper(), "served_by": "app"}


app.run()
"#;

/// An agent whose entry point is the callable the artifact's own `entryPoint` names, and
/// which reads the invocation's context beside the payload.
const NAMED_AGENT: &str = r#"
def answer(payload, context):
    return {
        "answer": payload["prompt"].upper(),
        "served_by": "named",
        "session": context.session_id,
    }
"#;

/// An agent stored as SEVERAL objects under the artifact's prefix: the entry module imports
/// a sibling module of the same deployment.
const PACKAGE_AGENT_ENTRY: &str = r#"
import shout
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    return {"answer": shout.louder(payload["prompt"]), "served_by": "package"}


app.run()
"#;

/// See [`PACKAGE_AGENT_ENTRY`].
const PACKAGE_AGENT_HELPER: &str = r#"
def louder(text):
    return text.upper()
"#;

/// An agent handed a payload the request stated a non-JSON media type for, which reaches it
/// as the text it is, and which answers text.
const TEXT_AGENT: &str = r#"
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    return payload.upper()


app.run()
"#;

/// An agent that YIELDS its answer, which is served as the stream of events each chunk is.
const STREAM_AGENT: &str = r#"
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    for piece in payload["prompt"].split("-"):
        yield piece.upper()


app.run()
"#;

/// An agent that serves the invocation route itself, with its own HTTP request handler —
/// the contract AWS delivers an invocation to: `POST /invocations` on the server the
/// deployed code stands up. Its server is stood up under the `__main__` guard a container
/// would run the file under.
const HTTP_AGENT: &str = r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        request = json.loads(self.rfile.read(length) or b"{}")
        answer = json.dumps(
            {"answer": request["prompt"].upper(), "served_by": "http", "path": self.path}
        ).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    def log_message(self, template, *held):
        return None


if __name__ == "__main__":
    HTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
"#;

/// The same contract, with the server stood up as the module's own last act rather than
/// under the `__main__` guard.
const SERVING_AGENT: &str = r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer


class Served(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        request = json.loads(self.rfile.read(length) or b"{}")
        answer = json.dumps(
            {"answer": request["prompt"].upper(), "served_by": "serving"}
        ).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    def log_message(self, template, *held):
        return None


HTTPServer(("0.0.0.0", 8080), Served).serve_forever()
"#;

/// An agent whose own handler answers a NON-SUCCESS status, which the invocation reports as
/// its own: "a status code of 200 indicates a successful operation. Other status codes
/// indicate various error conditions" (`InvokeAgentRuntimeResponse$statusCode`).
const REFUSING_HTTP_AGENT: &str = r#"
import json
from http.server import BaseHTTPRequestHandler


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        answer = json.dumps({"error": "this agent is not ready"}).encode("utf-8")
        self.send_response(503)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    def log_message(self, template, *held):
        return None
"#;

/// An agent whose own code raises, which is a failure of the deployment rather than of this
/// emulator.
const RAISING_AGENT: &str = r#"
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    raise ValueError("the deployed agent decided against it")


app.run()
"#;

/// A deployment that serves the invocation route in none of the ways the contract admits:
/// it stands up no server, registers nothing with the AgentCore app, and its entry point
/// names no callable.
const SILENT_AGENT: &str = r#"
GREETING = "this module serves no invocation"
"#;

/// Create an agent runtime through the control plane's own `CreateAgentRuntime`, deployed
/// from the code artifact at `prefix`, and answer the ARN that service minted.
#[allow(clippy::too_many_arguments)]
async fn create_runtime(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
    prefix: &str,
    entry_point: Value,
    runtime: &str,
) -> String {
    let (status, created) = agentcore_control(
        router,
        "PUT",
        "/runtimes/",
        Some(json!({
            "agentRuntimeName": name,
            "roleArn": role_arn,
            "agentRuntimeArtifact": {
                "codeConfiguration": {
                    "code": {"s3": {"bucket": CODE_BUCKET, "prefix": prefix}},
                    "runtime": runtime,
                    "entryPoint": entry_point,
                }
            },
            "networkConfiguration": {"networkMode": "PUBLIC"},
            "protocolConfiguration": {"serverProtocol": "HTTP"}
        })),
    )
    .await;
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

/// The end-to-end requirement: a runtime deployed by a CloudFormation stack is invokable
/// through the ARN that stack exported, and the body the invocation answers with is what the
/// deployed code produced from the payload the request carried.
#[tokio::test]
async fn a_stack_deployed_runtime_answers_an_invocation_with_what_its_code_returned() {
    let router = build_router();
    let case = "a runtime deployed from an S3 code artifact by a stack";
    create_bucket(&router, case).await;
    put_object(&router, case, "stack-agent/main.py", APP_AGENT).await;

    let template = json!({
        "Resources": {
            "AgentRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "stack-deployed-agent-role",
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": AGENTCORE_PRINCIPAL},
                            "Action": "sts:AssumeRole"
                        }]
                    }
                }
            },
            "Runtime": {
                "Type": "AWS::BedrockAgentCore::Runtime",
                "Properties": {
                    "AgentRuntimeName": "stackDeployedCodeAgent",
                    "RoleArn": {"Fn::GetAtt": ["AgentRole", "Arn"]},
                    "AgentRuntimeArtifact": {
                        "CodeConfiguration": {
                            "Code": {"S3": {"Bucket": CODE_BUCKET, "Prefix": "stack-agent/main.py"}},
                            "Runtime": PYTHON_RUNTIME,
                            "EntryPoint": ["main.py"]
                        }
                    },
                    // The resource type states the server protocol as the scalar its API
                    // takes in `protocolConfiguration.serverProtocol`.
                    "ProtocolConfiguration": "HTTP",
                    "NetworkConfiguration": {"NetworkMode": "PUBLIC"}
                }
            }
        },
        "Outputs": {
            "RuntimeArn": {"Value": {"Fn::GetAtt": ["Runtime", "AgentRuntimeArn"]}},
            "RuntimeId": {"Value": {"Fn::GetAtt": ["Runtime", "AgentRuntimeId"]}}
        }
    })
    .to_string();

    let body = create_and_settle(&router, case, "deployed-agent-stack", &template).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let runtime_arn = published(case, &body, "RuntimeArn");
    let runtime_id = published(case, &body, "RuntimeId");

    // The exported ARN resolves to a runtime the control plane itself holds.
    let (status, reported) =
        agentcore_control(&router, "GET", &format!("/runtimes/{runtime_id}/"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the control plane to hold the runtime the stack exported \
         ({runtime_arn}), got {status} with {reported}"
    );
    assert_eq!(
        reported["agentRuntimeArn"].as_str(),
        Some(runtime_arn.as_str()),
        "case {case:?}: expected the exported ARN to be the one the control plane minted, got \
         {reported}"
    );

    // Two invocations, each carrying its own marker: the answer is what the deployed code
    // made of the marker THAT request sent.
    for marker in ["first-marker", "second-marker"] {
        let answered = invoke(
            &router,
            &runtime_arn,
            "application/json",
            "application/json",
            &json!({"prompt": marker}).to_string(),
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected HTTP 200 from InvokeAgentRuntime for {marker:?}, got {} \
             with {:?}",
            answered.status,
            answered.text()
        );
        let document: Value = serde_json::from_slice(&answered.body).unwrap_or(Value::Null);
        assert_eq!(
            document["answer"].as_str(),
            Some(marker.to_uppercase().as_str()),
            "case {case:?}: expected the deployed code's answer for {marker:?}, got {:?}",
            answered.text()
        );
        assert_eq!(
            document["served_by"].as_str(),
            Some("app"),
            "case {case:?}: expected the entry point the deployed code registered to have \
             served the invocation, got {:?}",
            answered.text()
        );
        assert_eq!(
            answered.content_type, "application/json",
            "case {case:?}: expected the media type the invocation asked for, got {:?}",
            answered.content_type
        );
    }
}

/// What a row's invocation must answer with.
#[derive(Debug)]
enum Expected {
    /// The answer is exactly this JSON document — what the entry point returned, serialized.
    Document(Value),
    /// The answer is exactly this text.
    Text(String),
}

/// One deployment form a row drives.
struct Row {
    case: &'static str,
    /// The runtime's name, which is also the prefix its artifact is stored under.
    name: &'static str,
    /// The objects the artifact is stored as: `(key under the prefix, source)`. A single
    /// entry whose key is empty is stored AT the prefix, as one object.
    objects: &'static [(&'static str, &'static str)],
    /// The artifact's `entryPoint`.
    entry_point: &'static [&'static str],
    /// The media type the payload is sent in, and the one the answer is asked for.
    content_type: &'static str,
    accept: &'static str,
    /// The payload the invocation carries.
    payload: &'static str,
    /// What the answer must carry, and the media type it must be reported in.
    expected: Expected,
    expected_content_type: &'static str,
}

/// Drive every row: store its artifact, create the runtime it deploys, invoke it, and assert
/// the answer is what the deployed code produced.
async fn each_row(rows: &[Row]) {
    let router = build_router();
    create_bucket(&router, "the shared code bucket").await;
    for row in rows {
        let role = create_role(&router, row.case, &format!("{}-role", row.name)).await;
        let prefix = match row.objects {
            [("", source)] => {
                let key = format!("{}/main.py", row.name);
                put_object(&router, row.case, &key, source).await;
                key
            }
            objects => {
                for (key, source) in objects {
                    put_object(&router, row.case, &format!("{}/{key}", row.name), source).await;
                }
                row.name.to_string()
            }
        };
        let entry_point = Value::Array(
            row.entry_point
                .iter()
                .map(|held| Value::String((*held).to_string()))
                .collect(),
        );
        let runtime_arn = create_runtime(
            &router,
            row.case,
            row.name,
            &role,
            &prefix,
            entry_point,
            PYTHON_RUNTIME,
        )
        .await;
        let answered = invoke(
            &router,
            &runtime_arn,
            row.content_type,
            row.accept,
            row.payload,
        )
        .await;
        let answered_text = answered.text();
        assert_eq!(
            answered.status, 200,
            "case {:?}: expected HTTP 200 from InvokeAgentRuntime, got {} with \
             {answered_text:?}",
            row.case, answered.status
        );
        assert_eq!(
            answered.content_type, row.expected_content_type,
            "case {:?}: expected the answer in {:?}, got {:?}",
            row.case, row.expected_content_type, answered.content_type
        );
        match &row.expected {
            Expected::Document(expected) => {
                let answered_document: Value =
                    serde_json::from_slice(&answered.body).unwrap_or(Value::Null);
                assert_eq!(
                    &answered_document, expected,
                    "case {:?}: expected {expected}, got {answered_text:?}",
                    row.case
                );
            }
            Expected::Text(expected) => {
                assert_eq!(
                    &answered_text, expected,
                    "case {:?}: expected {expected:?}, got {answered_text:?}",
                    row.case
                );
            }
        }
    }
}

/// RULE: the payload reaches the entry point the deployed code serves and the answer is what
/// that code returned — here for every form a CALLABLE entry point takes, and for either way
/// the artifact is stored.
#[tokio::test]
async fn the_invocation_reaches_a_callable_entry_point_in_every_form() {
    let rows: Vec<Row> = vec![
        Row {
            case: "an entry point the code registered with the AgentCore app",
            name: "appAgent",
            objects: &[("", APP_AGENT)],
            entry_point: &["main.py"],
            content_type: "application/json",
            accept: "application/json",
            payload: r#"{"prompt": "registered"}"#,
            expected: Expected::Document(json!({"answer": "REGISTERED", "served_by": "app"})),
            expected_content_type: "application/json",
        },
        Row {
            case: "an entry point the artifact names, taking the invocation's context",
            name: "namedAgent",
            objects: &[("", NAMED_AGENT)],
            entry_point: &["main.py", "answer"],
            content_type: "application/json",
            accept: "application/json",
            payload: r#"{"prompt": "named"}"#,
            // The session this invocation runs under reaches the entry point's context.
            expected: Expected::Document(json!({
                "answer": "NAMED",
                "served_by": "named",
                "session": SESSION,
            })),
            expected_content_type: "application/json",
        },
        Row {
            case: "an artifact stored as several objects under its prefix",
            name: "packageAgent",
            objects: &[
                ("agent.py", PACKAGE_AGENT_ENTRY),
                ("shout.py", PACKAGE_AGENT_HELPER),
            ],
            entry_point: &["agent.py"],
            content_type: "application/json",
            accept: "application/json",
            payload: r#"{"prompt": "package"}"#,
            expected: Expected::Document(json!({"answer": "PACKAGE", "served_by": "package"})),
            expected_content_type: "application/json",
        },
        Row {
            case: "a payload sent as text, answered as text",
            name: "textAgent",
            objects: &[("", TEXT_AGENT)],
            entry_point: &["main.py"],
            content_type: "text/plain",
            accept: "text/plain",
            payload: "a plain request",
            expected: Expected::Text("A PLAIN REQUEST".to_string()),
            expected_content_type: "text/plain",
        },
        Row {
            case: "an entry point that yields its answer",
            name: "streamAgent",
            objects: &[("", STREAM_AGENT)],
            entry_point: &["main.py"],
            content_type: "application/json",
            accept: "application/json",
            payload: r#"{"prompt": "one-two"}"#,
            expected: Expected::Text("data: ONE\n\ndata: TWO\n\n".to_string()),
            expected_content_type: "text/event-stream",
        },
    ];
    each_row(&rows).await;
}

/// RULE: the same, for a deployment that serves the invocation ROUTE itself — the contract
/// AWS delivers an invocation to, `POST /invocations` on the server the code stands up —
/// whether that server is stood up under the `__main__` guard or as the module's own last
/// act.
#[tokio::test]
async fn the_invocation_is_delivered_to_the_route_the_deployed_code_serves() {
    let rows: Vec<Row> = vec![
        Row {
            case: "an agent serving the invocation route with its own HTTP request handler",
            name: "httpAgent",
            objects: &[("", HTTP_AGENT)],
            entry_point: &["main.py"],
            content_type: "application/json",
            accept: "application/json",
            payload: r#"{"prompt": "over-http"}"#,
            // The route the invocation was delivered on is the one AWS delivers it to, as
            // the deployed code read it off the request.
            expected: Expected::Document(json!({
                "answer": "OVER-HTTP",
                "served_by": "http",
                "path": "/invocations",
            })),
            expected_content_type: "application/json",
        },
        Row {
            case: "an agent that stands its server up as the module's own last act",
            name: "servingAgent",
            objects: &[("", SERVING_AGENT)],
            entry_point: &["main.py"],
            content_type: "application/json",
            accept: "application/json",
            payload: r#"{"prompt": "stood-up"}"#,
            expected: Expected::Document(json!({"answer": "STOOD-UP", "served_by": "serving"})),
            expected_content_type: "application/json",
        },
    ];
    each_row(&rows).await;
}

/// RULE: an invocation that produced no answer from the deployed code is reported as the
/// dependency failure it is — the modelled `RuntimeClientError` (424) — and an ARN the
/// control plane holds no runtime for is still the modelled `ResourceNotFoundException`
/// (404). No body is composed for either.
#[tokio::test]
async fn an_invocation_with_no_answer_from_the_deployment_is_refused_as_modelled() {
    let router = build_router();
    let case = "the refusal classes of an invocation";
    create_bucket(&router, case).await;
    let role = create_role(&router, case, "refusal-agent-role").await;

    // An agent whose own code raises.
    put_object(&router, case, "raising/main.py", RAISING_AGENT).await;
    let raising = create_runtime(
        &router,
        case,
        "raisingAgent",
        &role,
        "raising/main.py",
        json!(["main.py"]),
        PYTHON_RUNTIME,
    )
    .await;

    // A deployment that serves the invocation route in none of the ways the contract admits.
    put_object(&router, case, "silent/main.py", SILENT_AGENT).await;
    let silent = create_runtime(
        &router,
        case,
        "silentAgent",
        &role,
        "silent/main.py",
        json!(["main.py"]),
        PYTHON_RUNTIME,
    )
    .await;

    // A deployment whose declared runtime this emulator holds no engine for.
    put_object(&router, case, "node/main.js", "exports.invoke = () => ({});").await;
    let node = create_runtime(
        &router,
        case,
        "nodeAgent",
        &role,
        "node/main.js",
        json!(["main.js"]),
        "NODE_22",
    )
    .await;

    // A runtime deployed from a CONTAINER IMAGE, which this emulator starts none of.
    let (status, created) = agentcore_control(
        &router,
        "PUT",
        "/runtimes/",
        Some(json!({
            "agentRuntimeName": "containerAgent",
            "roleArn": role,
            "agentRuntimeArtifact": {
                "containerConfiguration": {
                    "containerUri": "public.ecr.aws/deployed/agent:1"
                }
            },
            "networkConfiguration": {"networkMode": "PUBLIC"}
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: the container-deployed runtime must be created, got {status} with \
         {created}"
    );
    let container = created["agentRuntimeArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, the ARN invoked, the status it answers, the modelled error it carries)
    let rows: [(&str, &str, u16, &str); 5] = [
        (
            "an agent whose own code raised",
            raising.as_str(),
            424,
            "RuntimeClientError",
        ),
        (
            "a deployment that serves the invocation route in no way the contract admits",
            silent.as_str(),
            424,
            "RuntimeClientError",
        ),
        (
            "a deployment whose runtime no engine here serves",
            node.as_str(),
            424,
            "RuntimeClientError",
        ),
        (
            "a deployment that is a container image",
            container.as_str(),
            424,
            "RuntimeClientError",
        ),
        (
            "an ARN the control plane holds no runtime for",
            "arn:aws:bedrock-agentcore:us-east-1:000000000000:runtime/absent-0123456789",
            404,
            "ResourceNotFoundException",
        ),
    ];
    for (row, arn, expected_status, expected_code) in rows {
        let answered = invoke(
            &router,
            arn,
            "application/json",
            "application/json",
            r#"{"prompt": "refused"}"#,
        )
        .await;
        assert_eq!(
            (answered.status, answered.error_code().as_str()),
            (expected_status, expected_code),
            "case {row:?}: expected HTTP {expected_status} {expected_code}, got {} with {:?}",
            answered.status,
            answered.text()
        );
    }
}

/// RULE: the status the deployed code answered its invocation with is the invocation's own
/// status, and the body is still that code's — the answer is passed through rather than
/// restated by this emulator.
#[tokio::test]
async fn the_status_the_deployed_code_answered_is_the_invocations_own() {
    let router = build_router();
    let case = "an agent whose handler answers a non-success status";
    create_bucket(&router, case).await;
    let role = create_role(&router, case, "refusing-agent-role").await;
    put_object(&router, case, "refusing/main.py", REFUSING_HTTP_AGENT).await;
    let runtime_arn = create_runtime(
        &router,
        case,
        "refusingAgent",
        &role,
        "refusing/main.py",
        json!(["main.py"]),
        PYTHON_RUNTIME,
    )
    .await;

    let answered = invoke(
        &router,
        &runtime_arn,
        "application/json",
        "application/json",
        r#"{"prompt": "not-ready"}"#,
    )
    .await;
    let answered_text = answered.text();
    assert_eq!(
        answered.status, 503,
        "case {case:?}: expected the status the deployed code answered, got {} with \
         {answered_text:?}",
        answered.status
    );
    let document: Value = serde_json::from_slice(&answered.body).unwrap_or(Value::Null);
    assert_eq!(
        document,
        json!({"error": "this agent is not ready"}),
        "case {case:?}: expected the body the deployed code wrote, got {answered_text:?}"
    );
}
