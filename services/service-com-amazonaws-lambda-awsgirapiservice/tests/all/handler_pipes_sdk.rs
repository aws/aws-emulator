// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a RUNNING handler drives and inspects an EventBridge PIPE through, end to end
//! over the wire: Lambda, EventBridge Pipes, IAM and the token service on ONE engine, a
//! `python3.x` function whose code does `import boto3`, and what its calls did read back
//! where a client reads it.
//!
//! RULE: a managed runtime ships the AWS SDK on the handler's import path with the catalog of
//! the services it can call, so a handler may build a client for ANY service this emulator
//! serves and call it — the request bound exactly as that service's own model binds it (for a
//! `restJson1` service: the method and URI of each operation's `@http` trait, its
//! `@httpLabel` members in the path, its `@httpQuery` members in the query, the rest as the
//! JSON body), the answer read back under the output shape's own member names, and a refusal
//! raised as `botocore.exceptions.ClientError` carrying the SERVICE's own error code, message
//! and status. The call is made as the function's EXECUTION ROLE, so the service serving it
//! decides it against the policies in force on that role: an action no statement allows is
//! `AccessDeniedException` (HTTP 403) with IAM's own denial sentence, which the handler can
//! catch and return as data — the invocation still answers 200 with no `FunctionError`.
//!
//! The steps differ along what the rule generalises over: WHICH binding class the operation
//! uses (a read addressed by one path label, a write addressed by a label and a sub-path, a
//! list addressed by nothing at all, an ARN-addressed write carrying a body map, an
//! ARN-addressed read), and what the answer is (served, the service's own `NotFoundException`
//! for an identifier that resolves to nothing, and the authorization refusal of an action the
//! role is not granted). They run in ONE invocation, because an invocation is the dearest
//! thing this target does; the last assertions read the whole thing back through EventBridge
//! Pipes' OWN API, so a call that answered without doing the work fails here.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;
// The one .zip writer this crate's suites package a handler with, and the one
// `x-www-form-urlencoded` encoder they write an awsQuery parameter and an ARN-valued
// `@httpLabel` with — each exported by the suite that first needed it rather than written
// again here.
use crate::handler_s3_object_access::form_encode;
use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for, and the one the invocation — and therefore
/// the client its handler builds — runs in.
const REGION: &str = "us-east-1";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// How long a wait for a state the clock owes may take before the suite gives up. Generous
/// on purpose: what is asserted is that the state ARRIVES, never how fast the host is.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long between two looks at a pipe whose state move is still owed.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The access-key id standing for the account's own published credential, which is what every
/// request this suite makes on its own behalf presents.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The name of the inline identity policy the execution role carries.
const INLINE_POLICY: &str = "inline";

/// The role the function runs as, and the function's own name — together they name the
/// session a handler's calls are made as.
const ROLE: &str = "pipes-handler-role";
const FUNCTION: &str = "pipes-sdk-caller";

/// The role each PIPE runs as, which is a different identity from the function's: a pipe's
/// stages act as its own `RoleArn`. This account holds no record of it, which is the position
/// an execution role provisioned outside the client that creates the resource is in.
const PIPE_ROLE: &str = "pipes-service-role";

/// The pipes the handler acts on: one resting, which it starts, and one running, which it
/// stops.
const RESTING_PIPE: &str = "ordersresting";
const RUNNING_PIPE: &str = "ordersrunning";

/// A pipe name this account never holds, so the read of it is the service's own 404.
const ABSENT_PIPE: &str = "ordersabsent";

/// The error code and status AWS states an authorization failure with.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: i64 = 403;

/// The state a pipe rests in and the one it runs in, as `PipeState` spells them
/// (`smithy_query(query="shape",shape="com.amazonaws.pipes#PipeState")`).
const STOPPED: &str = "STOPPED";
const RUNNING: &str = "RUNNING";

/// Lambda, EventBridge Pipes, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router and signed as the session of its own execution role, so the pipe it steers is
/// the pipe this suite created and the policies that decide its calls are the ones written
/// here with IAM's own operations.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        // The handler's OWN call is signed by the runtime, with the credential it was given.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
    /// the request and where the Region comes from. The signature is never checked here.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{service}/\
             aws4_request, SignedHeaders=host, Signature=not-verified"
        )
    }

    /// One `restJson1` call to the service answering at the endpoint host `signing_name`: its
    /// status, its headers and its parsed body.
    async fn rest(
        &self,
        signing_name: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> (u16, http::HeaderMap, Value) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", Self::authorization(signing_name))
            .header("content-type", "application/json")
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        let parsed: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
        };
        (parts.status.as_u16(), parts.headers, parsed)
    }

    /// One EventBridge Pipes call made as a CLIENT does, which must be served.
    async fn pipes(&self, what: &str, method: &str, path: &str, body: Option<&Value>) -> Value {
        let payload = body
            .map(|value| serde_json::to_vec(value).expect("the request body serializes"))
            .unwrap_or_default();
        let (status, _headers, parsed) = self.rest("pipes", method, path, &payload).await;
        assert_eq!(
            status, 200,
            "setup {what}: expected HTTP 200 from {method} {path}, got {status} ({parsed})"
        );
        parsed
    }

    /// One awsQuery call to IAM that must be served.
    async fn iam(&self, what: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "setup {what}: IAM {action} must succeed, got {status} {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// Create the execution role, trusted by Lambda and carrying `policy` as its inline
    /// identity policy — written with IAM's own operations, so the role a session is minted
    /// of is the record `iam:CreateRole` wrote.
    async fn execution_role(&self, policy: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "lambda.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            "the execution role",
            "CreateRole",
            &[
                ("RoleName", ROLE.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        self.iam(
            "the role's inline policy",
            "PutRolePolicy",
            &[
                ("RoleName", ROLE.to_string()),
                ("PolicyName", INLINE_POLICY.to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
        role_arn()
    }

    /// Create the function, running as `role_arn`, and wait out the provisioning window.
    async fn function(&self, role_arn: &str, source: &str) {
        let (status, _headers, created) = self
            .rest(
                "lambda",
                "POST",
                FUNCTIONS_URI,
                &serde_json::to_vec(&json!({
                    "FunctionName": FUNCTION,
                    "Role": role_arn,
                    "Runtime": "python3.12",
                    "Handler": HANDLER,
                    "Timeout": 30,
                    "Code": {
                        "ZipFile": base64::engine::general_purpose::STANDARD
                            .encode(package(&[("lambda_function.py", source)])),
                    },
                }))
                .expect("the create body serializes"),
            )
            .await;
        assert_eq!(
            status, 201,
            "setup: expected HTTP 201 from CreateFunction, got {status} ({created})"
        );
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
    }

    /// `Invoke` the function once with `event`, asserting the invocation was served and the
    /// handler ran to completion — which is what a refusal the handler CAUGHT looks like from
    /// outside — and answering what the handler reported.
    async fn invoke(&self, case: &str, event: &Value) -> Value {
        let (status, headers, payload) = self
            .rest(
                "lambda",
                "POST",
                &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
                event.to_string().as_bytes(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
        );
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        payload
    }

    /// Create the pipe `name` with `desired_state` and wait until the create's own state move
    /// — which the CLOCK owes — has landed, so a later call is not refused as a move already
    /// in flight.
    async fn pipe(&self, name: &str, desired_state: &str) {
        self.pipes(
            name,
            "POST",
            &format!("/v1/pipes/{name}"),
            Some(&json!({
                "Source": format!("arn:aws:sqs:{REGION}:{}:orders", engine::emulator_account_id()),
                "Target": format!(
                    "arn:aws:lambda:{REGION}:{}:function:sink",
                    engine::emulator_account_id()
                ),
                "RoleArn": pipe_role_arn(),
                "DesiredState": desired_state,
            })),
        )
        .await;
        self.pipe_reaches(name, desired_state).await;
    }

    /// Wait until `name` reports `state` as its `CurrentState`, looking again until it does or
    /// [`SETTLE_DEADLINE`] passes: the move is owed by the clock, so what is waited for is the
    /// CONDITION and not a span.
    async fn pipe_reaches(&self, name: &str, state: &str) -> Value {
        let started = Instant::now();
        loop {
            let described = self
                .pipes(name, "GET", &format!("/v1/pipes/{name}"), None)
                .await;
            if described["CurrentState"] == json!(state) {
                return described;
            }
            assert!(
                started.elapsed() < SETTLE_DEADLINE,
                "setup {name}: expected the pipe to reach {state}, got {described}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The ARN the account holds the execution role under.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", engine::emulator_account_id())
}

/// The ARN of the role a PIPE runs as.
fn pipe_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{PIPE_ROLE}",
        engine::emulator_account_id()
    )
}

/// The ARN this emulator mints for the pipe `name`, as a client addresses one in a tagging
/// call — spelled out here rather than read off the crate under test.
fn pipe_arn(name: &str) -> String {
    format!(
        "arn:aws:pipes:{REGION}:{}:pipe/{name}",
        engine::emulator_account_id()
    )
}

/// The assumed-role session a function's own calls are made as:
/// `arn:aws:sts::<account>:assumed-role/<role name>/<function name>`, the identity AWS reports
/// for a function's calls.
fn session_principal() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{FUNCTION}",
        engine::emulator_account_id()
    )
}

/// The sentence IAM refuses a request with, built here from the principal, the action and the
/// resource rather than read off the crates under test.
fn denial(action: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} because no \
         identity-based policy allows the {action} action",
        session_principal()
    )
}

/// The handler the suite runs: one step per binding class, each reported under its own case
/// name, and every refusal caught and reported as the code, message and status the SERVICE
/// answered — the shape a function that drives the pipe it belongs to has.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions

PIPES = boto3.client("pipes")


def _call(step):
    name = step["call"]
    if name == "describe_pipe":
        answered = PIPES.describe_pipe(Name=step["pipe"])
        return {
            "arn": answered.get("Arn"),
            "name": answered.get("Name"),
            "state": answered.get("CurrentState"),
            "source": answered.get("Source"),
            "target": answered.get("Target"),
            "role": answered.get("RoleArn"),
        }
    if name == "start_pipe":
        answered = PIPES.start_pipe(Name=step["pipe"])
        return {"state": answered.get("CurrentState"), "desired": answered.get("DesiredState")}
    if name == "stop_pipe":
        answered = PIPES.stop_pipe(Name=step["pipe"])
        return {"state": answered.get("CurrentState"), "desired": answered.get("DesiredState")}
    if name == "list_pipes":
        answered = PIPES.list_pipes()
        return {"names": sorted(pipe.get("Name") for pipe in answered.get("Pipes", []))}
    if name == "tag_resource":
        PIPES.tag_resource(resourceArn=step["arn"], tags=step["tags"])
        return {}
    if name == "list_tags_for_resource":
        return {"tags": PIPES.list_tags_for_resource(resourceArn=step["arn"]).get("tags")}
    if name == "update_pipe":
        answered = PIPES.update_pipe(Name=step["pipe"], RoleArn=step["role"])
        return {"state": answered.get("CurrentState")}
    raise ValueError("no such step: " + name)


def lambda_handler(event, context):
    steps = {}
    for step in event["steps"]:
        try:
            answered = _call(step)
        except botocore.exceptions.ClientError as error:
            steps[step["case"]] = {
                "ok": False,
                "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"],
                "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
            }
        else:
            answered["ok"] = True
            steps[step["case"]] = answered
    return {"steps": steps, "region": PIPES.meta.region_name}
"#;

/// Every binding class of this service's protocol, driven from inside a handler in one
/// invocation, each step asserted on what the SDK gave the guest — and the last assertions on
/// what EventBridge Pipes itself holds.
#[tokio::test]
async fn a_handler_drives_a_pipe_through_the_sdk_the_runtime_supplies() {
    let _stream = one_invocation_stream_at_a_time().await;
    let case = "a handler calling EventBridge Pipes through the runtime's SDK";
    let emulator = Emulator::new();

    // The role may do everything this suite's served steps ask for and nothing else, so the
    // one action it is not granted is the one the denied step asks for.
    let granted = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": [
                "pipes:DescribePipe",
                "pipes:StartPipe",
                "pipes:StopPipe",
                "pipes:ListPipes",
                "pipes:TagResource",
                "pipes:ListTagsForResource",
            ],
            "Resource": "*",
        }],
    })
    .to_string();
    let role = emulator.execution_role(&granted).await;
    emulator.pipe(RESTING_PIPE, STOPPED).await;
    emulator.pipe(RUNNING_PIPE, RUNNING).await;
    emulator.function(&role, HANDLER_SOURCE).await;

    let tags = json!({"owner": "handler", "env": "prod"});
    let steps = json!([
        // A read addressed by ONE path label.
        {"case": "a read addressed by a path label", "call": "describe_pipe", "pipe": RESTING_PIPE},
        // A write addressed by a label AND a sub-path.
        {"case": "a write addressed by a label and a sub-path", "call": "start_pipe",
         "pipe": RESTING_PIPE},
        {"case": "another write on that binding", "call": "stop_pipe", "pipe": RUNNING_PIPE},
        // A list addressed by nothing at all, whose output carries a list of structures.
        {"case": "a list addressed by nothing", "call": "list_pipes"},
        // An ARN-addressed write carrying a body map, and the read of what it wrote.
        {"case": "an ARN-addressed write carrying a body map", "call": "tag_resource",
         "arn": pipe_arn(RESTING_PIPE), "tags": tags.clone()},
        {"case": "an ARN-addressed read", "call": "list_tags_for_resource",
         "arn": pipe_arn(RESTING_PIPE)},
        // The service's OWN refusal of an identifier that resolves to nothing.
        {"case": "an identifier that resolves to nothing", "call": "describe_pipe",
         "pipe": ABSENT_PIPE},
        // An action the execution role is not granted: the service's authorization refusal.
        {"case": "an action the execution role is not granted", "call": "update_pipe",
         "pipe": RESTING_PIPE, "role": pipe_role_arn()},
    ]);
    let payload = emulator
        .invoke(case, &json!({ "steps": steps }))
        .await;
    let reported = payload["steps"].clone();

    // The client the handler built is the one the invocation runs in.
    assert_eq!(
        payload["region"],
        json!(REGION),
        "case {case:?}: expected the client to report the run's Region, got {payload}"
    );

    // A read addressed by one path label: the answer carries the stored record's own members.
    let read = reported["a read addressed by a path label"].clone();
    assert_eq!(
        (read["ok"].clone(), read["arn"].clone(), read["state"].clone()),
        (
            json!(true),
            json!(pipe_arn(RESTING_PIPE)),
            json!(STOPPED)
        ),
        "case {case:?}: expected the pipe the label addressed, got {read}"
    );
    assert_eq!(
        read["role"],
        json!(pipe_role_arn()),
        "case {case:?}: expected the stored RoleArn, got {read}"
    );

    // The two writes addressed by a label and a sub-path: each answers the state its own call
    // asked for.
    let started = reported["a write addressed by a label and a sub-path"].clone();
    assert_eq!(
        (started["ok"].clone(), started["desired"].clone()),
        (json!(true), json!(RUNNING)),
        "case {case:?}: expected the start to answer the state it asked for, got {started}"
    );
    let stopped = reported["another write on that binding"].clone();
    assert_eq!(
        (stopped["ok"].clone(), stopped["desired"].clone()),
        (json!(true), json!(STOPPED)),
        "case {case:?}: expected the stop to answer the state it asked for, got {stopped}"
    );

    // A list addressed by nothing at all: both pipes, under the output member's own name.
    assert_eq!(
        reported["a list addressed by nothing"]["names"],
        json!([RESTING_PIPE, RUNNING_PIPE]),
        "case {case:?}: expected both pipes listed, got {}",
        reported["a list addressed by nothing"]
    );

    // The ARN-addressed write, and the read of what it wrote — inside the same invocation.
    assert_eq!(
        reported["an ARN-addressed write carrying a body map"]["ok"],
        json!(true),
        "case {case:?}: expected the tagging call to be served, got {}",
        reported["an ARN-addressed write carrying a body map"]
    );
    assert_eq!(
        reported["an ARN-addressed read"]["tags"],
        tags,
        "case {case:?}: expected the tags the handler wrote, got {}",
        reported["an ARN-addressed read"]
    );

    // The service's own refusal of an identifier that resolves to nothing reaches the guest
    // as that service's code, message and status.
    let missing = reported["an identifier that resolves to nothing"].clone();
    assert_eq!(
        (missing["ok"].clone(), missing["code"].clone(), missing["status"].clone()),
        (json!(false), json!("NotFoundException"), json!(404)),
        "case {case:?}: expected the service's own 404, got {missing}"
    );
    assert!(
        missing["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected the service's own message, got {missing}"
    );

    // An action the execution role is not granted: the authorization refusal, naming the
    // session the call was made as — so a missing grant is distinguishable from every other
    // fault, and from a broken deployment package.
    let denied = reported["an action the execution role is not granted"].clone();
    assert_eq!(
        (
            denied["ok"].clone(),
            denied["code"].clone(),
            denied["status"].clone(),
            denied["message"].clone()
        ),
        (
            json!(false),
            json!(ACCESS_DENIED),
            json!(ACCESS_DENIED_STATUS),
            json!(denial("pipes:UpdatePipe", &pipe_arn(RESTING_PIPE)))
        ),
        "case {case:?}: expected IAM's own denial of the ungranted action, got {denied}"
    );

    // What EVENTBRIDGE PIPES itself holds: the work the handler's calls reported is the work
    // that happened — each pipe in the state its call asked for, and the tags the handler
    // wrote where a client reads them.
    emulator.pipe_reaches(RESTING_PIPE, RUNNING).await;
    emulator.pipe_reaches(RUNNING_PIPE, STOPPED).await;
    let listed = emulator
        .pipes(
            "the tags a client reads",
            "GET",
            &format!("/tags/{}", form_encode(&pipe_arn(RESTING_PIPE))),
            None,
        )
        .await;
    assert_eq!(
        listed["tags"], tags,
        "case {case:?}: expected the handler's own tags where a client reads them, got {listed}"
    );
    // The pipe the role was never granted `pipes:UpdatePipe` on still carries the role it was
    // created with: the refused update wrote nothing.
    let described = emulator
        .pipes(
            "the record after the refused update",
            "GET",
            &format!("/v1/pipes/{RESTING_PIPE}"),
            None,
        )
        .await;
    assert_eq!(
        described["RoleArn"],
        json!(pipe_role_arn()),
        "case {case:?}: expected the refused update to change nothing, got {described}"
    );
}
