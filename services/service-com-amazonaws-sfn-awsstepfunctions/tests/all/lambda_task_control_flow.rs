// SPDX-License-Identifier: CC0-1.0

//! What a `Task` state whose work is a LAMBDA FUNCTION lets a definition DECIDE: the
//! control flow around that task, driven over both services' own wire protocols
//! through the production `MultiServiceRouter` with Step Functions and Lambda mounted
//! on ONE engine — the shape `core/router/src/lib.rs` mounts them in — so the function
//! really runs and the task's outcome is the run's own.
//!
//! RULE: an execution's outcome is decided by its DEFINITION. A `Task` state reports
//! what the work its `Resource` names answered — and a Lambda invocation whose function
//! ERRORED is a failed task, named by the error document the function produced — so the
//! state's `Retry` and `Catch`, the `Choice` that branches on its result and the `Fail`
//! state either routes to all act on that outcome, with the execution reporting the
//! `Fail` state's own `Error` and `Cause`; and a definition-level `TimeoutSeconds` ends
//! a run that is still going at that span as `TIMED_OUT`, with no error of its own
//! because what ran out is the execution's allowance rather than anything one state did.
//! None of the emulator's own
//! interpretation ceilings is an outcome a definition can reach while its states' work
//! is being performed, however many invocations the definition asks for.
//!
//! The rows differ along what the rule generalises over: the outcome class a definition
//! reaches (a poll loop that succeeds on a later attempt, a `Choice` routing to a `Fail`
//! state, a `Retry` that exhausts into a `Catch`, a task error nothing catches, a loop
//! the machine-level `TimeoutSeconds` aborts, and the same loop finishing inside it) and
//! the number of invocations each asks for.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::execution_data_encryption::{form_encode, xml_element};
use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN};

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `State` a function reports once it can be invoked (`FunctionConfiguration$State`:
/// "Active" once "the function is ready to be invoked").
const FUNCTION_ACTIVE: &str = "Active";

/// The runtime the handler below is written for.
const RUNTIME: &str = "python3.12";

/// The handler the package's one module exports, as `Handler` names it.
const HANDLER: &str = "index.handler";

/// How long one poll of a status that the clock owes sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row waits for a function to reach [`FUNCTION_ACTIVE`].
const PATIENCE: Duration = Duration::from_secs(20);

/// The `Error` the poller's own `Fail` state carries, which is what the execution must
/// report once anything routes to that state.
const FAIL_STATE_ERROR: &str = "DescribeJob returned FAILED";

/// The `Cause` it carries, beside that error.
const FAIL_STATE_CAUSE: &str = "the job's last report was FAILED";

/// The exception class the handler raises when a row asks it to, which is the
/// `errorType` of the error document the run produces — and so the name the failed task
/// reports.
const RAISED_ERROR_TYPE: &str = "DescribeJobUnavailable";

/// The message that exception carries.
const RAISED_MESSAGE: &str = "the job service could not be reached";

/// The handler every row invokes: one poll of a job whose status the EVENT decides, so
/// one deployment package serves every row.
///
/// `mode` selects what the poll answers — a job that becomes `SUCCEEDED` on the
/// `ready_at`th poll, one that stays `RUNNING` for ever, one that reports `FAILED`, and
/// one the poll itself cannot make — and `attempt` is the count the state machine feeds
/// back round the loop, so the function's answer depends on the data the definition
/// carries rather than on anything the emulator holds.
const HANDLER_SOURCE: &str = r#"
class DescribeJobUnavailable(Exception):
    pass


def handler(event, context):
    mode = event.get("mode", "ready")
    attempt = int(event.get("attempt", 0)) + 1
    if mode == "unavailable":
        raise DescribeJobUnavailable("the job service could not be reached")
    if mode == "running":
        status = "RUNNING"
    elif mode == "failed":
        status = "FAILED"
    else:
        status = "SUCCEEDED" if attempt >= int(event.get("ready_at", 1)) else "RUNNING"
    return {
        "mode": mode,
        "ready_at": event.get("ready_at", 1),
        "attempt": attempt,
        "status": status,
    }
"#;

/// How one service's requests are addressed on the wire: `smithy_query(query=
/// "service_info")` of each model — the signing name, the `X-Amz-Target` prefix and the
/// `awsJson` version Step Functions declares.
const SFN_SIGNING_NAME: &str = "states";
/// See [`SFN_SIGNING_NAME`].
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";

/// The same, for the service a sibling suite mounts beside the two through
/// [`Chain::with_dynamodb`] so a handler's own SDK call is served for real: DynamoDB's
/// signing name and `X-Amz-Target` prefix, from its model's own `service_info`.
const DYNAMODB_SIGNING_NAME: &str = "dynamodb";
/// See [`DYNAMODB_SIGNING_NAME`].
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// The same, for the service a sibling suite mounts beside the two through
/// [`Chain::with_eventbridge`] so a rule's own delivery starts a real execution:
/// EventBridge's signing name and `X-Amz-Target` prefix, from its model's own
/// `service_info`.
const EVENTS_SIGNING_NAME: &str = "events";
/// See [`EVENTS_SIGNING_NAME`].
const EVENTS_TARGET_PREFIX: &str = "AWSEvents";

/// Step Functions and Lambda over ONE engine, so the invocation a task makes is served
/// by the very function the test created.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a sibling
/// suite that drives a Lambda task mounts the two services through this rather than
/// carrying a second copy of the setup.
pub(crate) struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    pub(crate) fn new() -> Self {
        Self::mounted(false, false, false)
    }

    /// [`Self::new`] with DynamoDB mounted beside the two, for a suite whose handlers
    /// call it through the SDK their runtime supplies: what a run wrote is then read
    /// back through DynamoDB's own API rather than claimed by the handler's answer.
    pub(crate) fn with_dynamodb() -> Self {
        Self::mounted(true, false, false)
    }

    /// [`Self::new`] with IAM mounted beside the two, for a suite whose rule is about the
    /// EXECUTION ROLE a `Task` state's call is made as: with IAM there the `roleArn` is a
    /// role this account really holds, so the access gate in front of the call really
    /// runs ([`Self::role`]).
    pub(crate) fn with_iam() -> Self {
        Self::mounted(false, true, false)
    }

    /// [`Self::new`] with EventBridge mounted beside the two, for a suite whose rule is
    /// about an execution a RULE started: with EventBridge there a matched event is
    /// really delivered to its state-machine target, so the execution under test is the
    /// delivery's own ([`Self::events`]).
    pub(crate) fn with_eventbridge() -> Self {
        Self::mounted(false, false, true)
    }

    fn mounted(dynamodb: bool, iam: bool, events: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        if dynamodb {
            router.register_service(
                service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
                    &engine,
                )),
            );
        }
        if iam {
            router.register_service(
                service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        if events {
            router.register_service(
                service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(
                    &engine,
                )),
            );
        }
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` header whose credential scope names `service`, which is
    /// what routes the request. The signature is never checked: this router runs with
    /// request authentication off.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
             SignedHeaders=host, Signature=0123456789abcdef"
        )
    }

    /// One Step Functions call, requiring a 200.
    pub(crate) async fn sfn(&self, case: &str, operation: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{SFN_SIGNING_NAME}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{SFN_TARGET_PREFIX}.{operation}"))
            .header("authorization", Self::authorization(SFN_SIGNING_NAME))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = parsed(response.into_body());
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One EventBridge call (`awsJson1_1`), requiring a 200: how a suite wires the rule
    /// whose delivery starts the execution under test. Only a
    /// [`Chain::with_eventbridge`] chain serves one.
    pub(crate) async fn events(&self, case: &str, operation: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "host",
                format!("{EVENTS_SIGNING_NAME}.{REGION}.amazonaws.com"),
            )
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{EVENTS_TARGET_PREFIX}.{operation}"))
            .header("authorization", Self::authorization(EVENTS_SIGNING_NAME))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = parsed(response.into_body());
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One DynamoDB call (`awsJson1_0`), requiring a 200: how a suite reads back what a
    /// handler's own SDK call really wrote. Only a [`Chain::with_dynamodb`] chain serves
    /// one.
    pub(crate) async fn ddb(&self, case: &str, operation: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "host",
                format!("{DYNAMODB_SIGNING_NAME}.{REGION}.amazonaws.com"),
            )
            .header("content-type", "application/x-amz-json-1.0")
            .header(
                "x-amz-target",
                format!("{DYNAMODB_TARGET_PREFIX}.{operation}"),
            )
            .header("authorization", Self::authorization(DYNAMODB_SIGNING_NAME))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = parsed(response.into_body());
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One `restJson1` call to Lambda, as its status and parsed body.
    async fn lambda(&self, method: &str, uri: &str, body: &Value) -> (u16, Value) {
        let payload = if body.is_null() {
            Vec::new()
        } else {
            body.to_string().into_bytes()
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
            .header("host", format!("lambda.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("lambda"))
            .body(Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed(response.into_body()))
    }

    /// The poller's function, waited out to [`FUNCTION_ACTIVE`] so an invocation of it
    /// is one Lambda admits; answers its ARN.
    async fn given_function(&self, name: &str) -> String {
        self.given_function_of(name, HANDLER_SOURCE, None).await
    }

    /// [`Self::given_function`] over a package of `source` and the function's own
    /// `Timeout` where one is asked for, for a suite whose rule is about what the
    /// function's own bounds do to the task that invoked it.
    pub(crate) async fn given_function_of(
        &self,
        name: &str,
        source: &str,
        timeout_seconds: Option<u64>,
    ) -> String {
        self.given_function_in(name, RUNTIME, MODULE, source, timeout_seconds)
            .await
    }

    /// [`Self::given_function_of`] for a function of ANOTHER runtime: `runtime` is the
    /// identifier the function declares, `module` the file the package carries the
    /// handler's module in (the module [`HANDLER`] names, with the runtime's own
    /// extension), and `source` that module's text.
    pub(crate) async fn given_function_in(
        &self,
        name: &str,
        runtime: &str,
        module: &str,
        source: &str,
        timeout_seconds: Option<u64>,
    ) -> String {
        let mut request = json!({
            "FunctionName": name,
            "Role": LAMBDA_ROLE_ARN,
            "Runtime": runtime,
            "Handler": HANDLER,
            "Code": { "ZipFile": package_of(module, source) },
        });
        if let Some(seconds) = timeout_seconds {
            request["Timeout"] = json!(seconds);
        }
        let (status, created) = self.lambda("POST", FUNCTIONS_URI, &request).await;
        assert_eq!(
            status, 201,
            "CreateFunction must answer 201, got {status} with {created}"
        );
        let arn = string(&created, "FunctionArn");
        let began = std::time::Instant::now();
        loop {
            let (status, configuration) = self
                .lambda(
                    "GET",
                    &format!("{FUNCTIONS_URI}/{name}/configuration"),
                    &Value::Null,
                )
                .await;
            assert_eq!(
                status, 200,
                "GetFunctionConfiguration must resolve the created function, got {status} with \
                 {configuration}"
            );
            if configuration.get("State").and_then(Value::as_str) == Some(FUNCTION_ACTIVE) {
                return arn;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "the function must reach {FUNCTION_ACTIVE} within {PATIENCE:?}, last saw \
                 {configuration}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Publish a version of the function `name` and answer the version label Lambda
    /// minted for it.
    ///
    /// `pub(crate)` for the reason the helpers above are: a sibling suite whose rule is
    /// about which VERSION an invocation ran reaches the version and alias plane through
    /// these rather than opening a second seam onto Lambda's wire protocol.
    pub(crate) async fn published(&self, name: &str) -> String {
        let (status, version) = self
            .lambda(
                "POST",
                &format!("{FUNCTIONS_URI}/{name}/versions"),
                &json!({}),
            )
            .await;
        assert_eq!(
            status, 201,
            "PublishVersion must answer 201, got {status} with {version}"
        );
        string(&version, "Version")
    }

    /// Point the alias `alias` of the function `name` at `version`.
    pub(crate) async fn aliased(&self, name: &str, alias: &str, version: &str) {
        let (status, created) = self
            .lambda(
                "POST",
                &format!("{FUNCTIONS_URI}/{name}/aliases"),
                &json!({ "Name": alias, "FunctionVersion": version }),
            )
            .await;
        assert_eq!(
            status, 201,
            "CreateAlias must answer 201, got {status} with {created}"
        );
    }

    /// Create `definition` as a state machine, start one execution of it with `input`,
    /// and report that execution once it has left `RUNNING`.
    pub(crate) async fn ran(
        &self,
        case: &str,
        name: &str,
        definition: &Value,
        input: &Value,
    ) -> Value {
        self.ran_as(case, name, definition, input, ROLE_ARN).await
    }

    /// [`Self::ran`] under a NAMED execution role, for a suite whose rule is about what
    /// that role lets the definition's tasks do ([`Self::role`]).
    pub(crate) async fn ran_as(
        &self,
        case: &str,
        name: &str,
        definition: &Value,
        input: &Value,
        role_arn: &str,
    ) -> Value {
        let created = self
            .sfn(
                case,
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": role_arn,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = self
            .sfn(
                case,
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "only",
                    "input": input.to_string(),
                }),
            )
            .await;
        self.settled(case, &string(&started, "executionArn")).await
    }

    /// `execution` once it has left `RUNNING`, which is also what a suite waits out after
    /// asking for a further attempt of it (`RedriveExecution`).
    pub(crate) async fn settled(&self, case: &str, execution: &str) -> Value {
        let began = std::time::Instant::now();
        loop {
            let described = self
                .sfn(
                    case,
                    "DescribeExecution",
                    &json!({ "executionArn": execution }),
                )
                .await;
            if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
                return described;
            }
            assert!(
                began.elapsed() < TERMINAL_PATIENCE,
                "case {case:?}: expected a terminal status within {TERMINAL_PATIENCE:?}, last \
                 saw {described}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// One `awsQuery` call to IAM, requiring a 200. Only a [`Chain::with_iam`] chain
    /// serves one.
    pub(crate) async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization("iam"))
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: {action} must answer 200 from IAM, got {status} with {text}"
        );
        text
    }

    /// A role this service's own principal may assume, whose inline policy carries
    /// `statements`; answers the ARN IAM's own `CreateRole` reported for it.
    ///
    /// The trust policy names [`SFN_SERVICE_PRINCIPAL`], because acting as the role is
    /// itself a call this service makes before any of the task's own.
    pub(crate) async fn role(&self, case: &str, name: &str, statements: &Value) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": SFN_SERVICE_PRINCIPAL },
                "Action": "sts:AssumeRole",
            }],
        });
        let created = self
            .iam(
                case,
                "CreateRole",
                &[
                    ("RoleName", name.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
        self.iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "TaskWork".to_string()),
                (
                    "PolicyDocument",
                    json!({ "Version": "2012-10-17", "Statement": statements }).to_string(),
                ),
            ],
        )
        .await;
        // The ARN is IAM's, read out of the answer its own handler wrote rather than
        // built here: the role is that service's resource.
        xml_element(&created, "Arn").unwrap_or_else(|| {
            panic!("case {case:?}: CreateRole answers the role Arn, got {created}")
        })
    }
}

/// The GLOBAL service principal Step Functions assumes a state machine's execution role
/// as, which a trust policy has to name for the role to be assumable at all.
const SFN_SERVICE_PRINCIPAL: &str = "states.amazonaws.com";

/// The execution role the function runs as. No IAM is mounted here, so nothing is known
/// about it and Lambda serves the create — which is the documented reading of a role
/// provisioned outside the client that creates the function.
const LAMBDA_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/LambdaExecutionRole";

/// How long a row waits for an execution to leave `RUNNING`: the span this emulator
/// holds a started execution in `RUNNING` for, plus the longest a row's own `Wait` and
/// `Retry` intervals add to it, plus room for the invocations themselves.
const TERMINAL_PATIENCE: Duration = PAST_THE_EXECUTION_DELAY.saturating_add(Duration::from_secs(30));

/// A response body, parsed; the null document for an empty one.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a sibling
/// suite reads a response through this rather than carrying a second copy of it.
pub(crate) fn parsed(bytes: Bytes) -> Value {
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error} for {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    }
}

/// The file the python package carries its one module in, named after the module
/// [`HANDLER`] names.
const MODULE: &str = "index.py";

/// `source` as a deployment package: one `.py` module named after the handler's own
/// module, in a .zip archive, base64-encoded because `FunctionCode$ZipFile` is a blob
/// and a `restJson1` request carries a blob base64-encoded in its JSON body.
///
/// `pub(crate)` for the reason [`parsed`] is.
pub(crate) fn package(source: &str) -> String {
    package_of(MODULE, source)
}

/// [`package`] over the file ANOTHER runtime's handler module lives in (`index.js` for a
/// node one), for a suite whose rows span more than one runtime.
fn package_of(module: &str, source: &str) -> String {
    let mut archive = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
    let options: ::zip::write::SimpleFileOptions = ::zip::write::SimpleFileOptions::default()
        .compression_method(::zip::CompressionMethod::Deflated);
    archive
        .start_file(module, options)
        .expect("the entry starts");
    ::std::io::Write::write_all(&mut archive, source.as_bytes()).expect("the entry is written");
    let bytes = archive.finish().expect("the archive finishes").into_inner();
    ::base64::Engine::encode(&::base64::engine::general_purpose::STANDARD, bytes)
}

/// The poller every row runs: one `Task` state invoking the function, a `Choice` that
/// branches on the status it answered, a `Wait` that re-enters the task, a `Succeed` and
/// a `Fail` state carrying its own `Error` and `Cause`.
///
/// `timeout_seconds` is the definition-level `TimeoutSeconds` when a row declares one.
fn poller(function_arn: &str, timeout_seconds: Option<u64>) -> Value {
    let mut definition = json!({
        "StartAt": "CheckStatus",
        "States": {
            "CheckStatus": { "Type": "Task", "Resource": function_arn, "Next": "Branch" },
            "Branch": {
                "Type": "Choice",
                "Choices": [
                    { "Variable": "$.status", "StringEquals": "SUCCEEDED", "Next": "JobDone" },
                    { "Variable": "$.status", "StringEquals": "FAILED", "Next": "JobFailed" },
                ],
                "Default": "Pause",
            },
            "Pause": { "Type": "Wait", "Seconds": 1, "Next": "CheckStatus" },
            "JobDone": { "Type": "Succeed" },
            "JobFailed": {
                "Type": "Fail",
                "Error": FAIL_STATE_ERROR,
                "Cause": FAIL_STATE_CAUSE,
            },
        },
    });
    if let Some(seconds) = timeout_seconds {
        definition["TimeoutSeconds"] = json!(seconds);
    }
    definition
}

/// The same task with a `Retry` and a `Catch`: the retrier names the wildcard the
/// Amazon States Language documents for a task failure, and the catcher routes to the
/// `Fail` state once the retrier's attempts are spent.
fn retried_poller(function_arn: &str, caught: bool) -> Value {
    let retry = json!([{
        "ErrorEquals": ["States.TaskFailed"],
        "IntervalSeconds": 1,
        "MaxAttempts": 2,
        "BackoffRate": 1.0,
    }]);
    if !caught {
        return json!({
            "StartAt": "CheckStatus",
            "States": {
                "CheckStatus": {
                    "Type": "Task",
                    "Resource": function_arn,
                    "Retry": retry,
                    "End": true,
                },
            },
        });
    }
    json!({
        "StartAt": "CheckStatus",
        "States": {
            "CheckStatus": {
                "Type": "Task",
                "Resource": function_arn,
                "Retry": retry,
                "Catch": [{ "ErrorEquals": ["States.ALL"], "Next": "JobFailed" }],
                "Next": "JobDone",
            },
            "JobDone": { "Type": "Succeed" },
            "JobFailed": {
                "Type": "Fail",
                "Error": FAIL_STATE_ERROR,
                "Cause": FAIL_STATE_CAUSE,
            },
        },
    })
}

/// What one row expects of the execution it ran.
struct Expected {
    status: &'static str,
    error: Option<&'static str>,
    cause: Option<&'static str>,
    /// How many times the poller's own task must have invoked the function, read off
    /// the `attempt` the last answer carried; `None` for a row whose execution reports
    /// no output.
    attempts: Option<u64>,
}

/// One row: a definition, the execution input, and what the execution must end as.
struct Row {
    case: &'static str,
    /// Whether the machine is the plain poller (`None` for no definition-level timeout,
    /// `Some(seconds)` for one) or the retried task.
    machine: Machine,
    input: Value,
    expected: Expected,
}

/// Which definition a row runs.
enum Machine {
    /// The poll loop, with the definition-level `TimeoutSeconds` a row declares.
    Poller(Option<u64>),
    /// The single retried task, with a `Catch` when `true`.
    Retried(bool),
}

/// The poller's `Wait` is a second long, so the loop advances one second per revisit
/// and a run that never leaves it is still going when this falls due.
const ABORTING_TIMEOUT_SECONDS: u64 = 3;

/// A definition-level `TimeoutSeconds` far wider than the poll loop needs, so the same
/// loop finishing inside it is not aborted.
const AMPLE_TIMEOUT_SECONDS: u64 = 300;

/// The whole table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a poll loop whose job is ready at once",
            machine: Machine::Poller(None),
            input: json!({ "mode": "ready", "ready_at": 1 }),
            expected: Expected {
                status: "SUCCEEDED",
                error: None,
                cause: None,
                attempts: Some(1),
            },
        },
        Row {
            case: "a poll loop that re-enters the wait twice before the job is ready",
            machine: Machine::Poller(None),
            input: json!({ "mode": "ready", "ready_at": 3 }),
            expected: Expected {
                status: "SUCCEEDED",
                error: None,
                cause: None,
                attempts: Some(3),
            },
        },
        Row {
            case: "a job reporting FAILED routes to the terminal Fail state",
            machine: Machine::Poller(None),
            input: json!({ "mode": "failed" }),
            expected: Expected {
                status: "FAILED",
                error: Some(FAIL_STATE_ERROR),
                cause: Some(FAIL_STATE_CAUSE),
                attempts: None,
            },
        },
        Row {
            case: "a task error nothing catches is named by the function's own error document",
            machine: Machine::Retried(false),
            input: json!({ "mode": "unavailable" }),
            expected: Expected {
                status: "FAILED",
                error: Some(RAISED_ERROR_TYPE),
                cause: None,
                attempts: None,
            },
        },
        Row {
            case: "a Retry that exhausts its attempts is caught into the Fail state",
            machine: Machine::Retried(true),
            input: json!({ "mode": "unavailable" }),
            expected: Expected {
                status: "FAILED",
                error: Some(FAIL_STATE_ERROR),
                cause: Some(FAIL_STATE_CAUSE),
                attempts: None,
            },
        },
        Row {
            case: "a loop that never finishes is aborted by the machine's TimeoutSeconds",
            machine: Machine::Poller(Some(ABORTING_TIMEOUT_SECONDS)),
            input: json!({ "mode": "running" }),
            expected: Expected {
                status: "TIMED_OUT",
                error: None,
                cause: None,
                attempts: None,
            },
        },
        Row {
            case: "the same loop finishing inside an ample TimeoutSeconds is not aborted",
            machine: Machine::Poller(Some(AMPLE_TIMEOUT_SECONDS)),
            input: json!({ "mode": "ready", "ready_at": 2 }),
            expected: Expected {
                status: "SUCCEEDED",
                error: None,
                cause: None,
                attempts: Some(2),
            },
        },
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lambda_task_lets_the_definition_decide_the_outcome() {
    let chain = Chain::new();
    let function_arn = chain.given_function("job-poller").await;
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let definition = match row.machine {
            Machine::Poller(timeout_seconds) => poller(&function_arn, timeout_seconds),
            Machine::Retried(caught) => retried_poller(&function_arn, caught),
        };
        let described = chain
            .ran(case, &format!("machine-{index}"), &definition, &row.input)
            .await;
        let status = described.get("status").and_then(Value::as_str);
        assert_eq!(
            status,
            Some(row.expected.status),
            "case {case:?}: expected status {:?}, got {described}",
            row.expected.status
        );
        match row.expected.error {
            Some(error) => assert_eq!(
                described.get("error").and_then(Value::as_str),
                Some(error),
                "case {case:?}: expected error {error:?}, got {described}"
            ),
            None => assert!(
                described.get("error").is_none(),
                "case {case:?}: expected no error, got {described}"
            ),
        }
        if let Some(cause) = row.expected.cause {
            assert_eq!(
                described.get("cause").and_then(Value::as_str),
                Some(cause),
                "case {case:?}: expected cause {cause:?}, got {described}"
            );
        }
        if let Some(attempts) = row.expected.attempts {
            let output: Value = described
                .get("output")
                .and_then(Value::as_str)
                .map(|text| serde_json::from_str(text).expect("the output is JSON"))
                .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described}"));
            assert_eq!(
                output.get("attempt").and_then(Value::as_u64),
                Some(attempts),
                "case {case:?}: expected {attempts} invocations, got {output}"
            );
        }
    }
}

/// A task error nothing catches reports the error document the function produced as its
/// `cause`, so a client reads the message the handler raised rather than a sentence this
/// emulator wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_uncaught_task_error_carries_the_functions_own_error_document() {
    let chain = Chain::new();
    let function_arn = chain.given_function("job-poller").await;
    let described = chain
        .ran(
            "the raised error's document",
            "raised",
            &retried_poller(&function_arn, false),
            &json!({ "mode": "unavailable" }),
        )
        .await;
    let cause = described
        .get("cause")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a cause in {described}"));
    let document: Value = serde_json::from_str(cause)
        .unwrap_or_else(|error| panic!("the cause must be the error document, got {cause:?}: {error}"));
    assert_eq!(
        document.get("errorType").and_then(Value::as_str),
        Some(RAISED_ERROR_TYPE),
        "expected the raised class as errorType, got {document}"
    );
    assert_eq!(
        document.get("errorMessage").and_then(Value::as_str),
        Some(RAISED_MESSAGE),
        "expected the raised message as errorMessage, got {document}"
    );
}
