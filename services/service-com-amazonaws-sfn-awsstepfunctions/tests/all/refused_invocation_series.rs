// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `AWS/States` series a `Task` state publishes about an
//! attempt the EXECUTION ROLE is not allowed to make, driven over each service's own
//! wire protocol through the production `MultiServiceRouter` with Step Functions, IAM,
//! Lambda and CloudWatch mounted on ONE engine — the shape `core/router/src/lib.rs`
//! mounts them in — so the documents the attempt is judged against are the ones IAM's
//! handlers wrote, an allowed attempt really invokes the function, and every series is
//! read back through CloudWatch's own `GetMetricStatistics`.
//!
//! RULE: a `Task` state's attempt at its work is SCHEDULED and then STARTED whatever the
//! call turns out to do, because both are recorded before anything about the call's
//! outcome is known — so an attempt the policies in force on the state machine's
//! `roleArn` refuse is a started attempt that FAILED. Each such attempt is therefore
//! counted once in the per-function `LambdaFunctionsScheduled`, `LambdaFunctionsStarted`
//! and `LambdaFunctionsFailed` series under the `LambdaFunctionArn` dimension and in
//! none of the succeeded ones, the execution it belongs to is counted in
//! `ExecutionsFailed` under `StateMachineArn` and in no succeeded one — and because no
//! request was made, the function's own `AWS/Lambda` `Invocations` series stays empty.
//! The decision is taken per call rather than once per execution, so a grant added to
//! the same role afterwards lets the same state machine's next execution succeed.
//!
//! The rows differ along what the rule generalises over: why the policies refuse the
//! call (nothing stated about it at all, a statement about another action, a grant over
//! another function, and an explicit `Deny` over a grant), how many attempts the
//! definition makes (one, and three under a `Retry`), and whether they refuse it at all
//! — the allowed row being the positive control that the same series count a real
//! invocation.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::execution_data_encryption::{form_encode, xml_element};
use crate::lambda_task_control_flow::{package, parsed};
use crate::state_machine_lifecycle::{string, REGION};

/// The namespace Step Functions publishes its own series to, and the names the AWS Step
/// Functions metric reference gives the per-function counts, the per-execution counts and
/// the two dimensions they are reported under. Written as literals here rather than read
/// from the crate under test, so a series published under a name no AWS reader would ask
/// for fails here.
const STATES_NAMESPACE: &str = "AWS/States";
/// See [`STATES_NAMESPACE`].
const LAMBDA_FUNCTION_ARN_DIMENSION: &str = "LambdaFunctionArn";
/// See [`STATES_NAMESPACE`].
const STATE_MACHINE_ARN_DIMENSION: &str = "StateMachineArn";
/// See [`STATES_NAMESPACE`].
const SCHEDULED_SERIES: &str = "LambdaFunctionsScheduled";
/// See [`STATES_NAMESPACE`].
const STARTED_SERIES: &str = "LambdaFunctionsStarted";
/// See [`STATES_NAMESPACE`].
const FAILED_SERIES: &str = "LambdaFunctionsFailed";
/// See [`STATES_NAMESPACE`].
const SUCCEEDED_SERIES: &str = "LambdaFunctionsSucceeded";
/// See [`STATES_NAMESPACE`].
const EXECUTIONS_FAILED_SERIES: &str = "ExecutionsFailed";
/// See [`STATES_NAMESPACE`].
const EXECUTIONS_SUCCEEDED_SERIES: &str = "ExecutionsSucceeded";

/// The namespace Lambda publishes its own metrics to, the name the AWS Lambda metric
/// reference gives the invocation count and the dimension it is reported per.
const LAMBDA_NAMESPACE: &str = "AWS/Lambda";
/// See [`LAMBDA_NAMESPACE`].
const INVOCATIONS_SERIES: &str = "Invocations";
/// See [`LAMBDA_NAMESPACE`].
const FUNCTION_NAME_DIMENSION: &str = "FunctionName";

/// The `Period`, in seconds, every query reads at: the one-minute granularity these
/// series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a query's window reaches, in seconds — wide enough that the
/// minute a datapoint was published in is inside it however a row straddles a minute
/// boundary.
const WINDOW_SECS: f64 = 600.0;

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `State` a function reports once it can be invoked.
const FUNCTION_ACTIVE: &str = "Active";

/// The runtime the handler below is written for.
const RUNTIME: &str = "python3.12";

/// The handler the package's one module exports.
const HANDLER: &str = "index.handler";

/// The execution role the FUNCTION runs as, which is not the state machine's.
const LAMBDA_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/LambdaExecutionRole";

/// The IAM action an invocation is subject to: the one Lambda PUBLISHES for `Invoke`
/// rather than the operation's own name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The error a `Task` state reports when the role may not make the call: Lambda's base
/// SDK exception, which is what a refusal `Invoke` does not model arrives under.
const REFUSED_ERROR: &str = "Lambda.AWSLambdaException";

/// The fixed text of the sentence that refusal carries.
const REFUSED_TEXT: &str = "not authorized to perform: lambda:InvokeFunction";

/// How long one poll of a status the clock owes sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row waits for a function to reach [`FUNCTION_ACTIVE`], and for an execution
/// to leave `RUNNING`.
const PATIENCE: Duration = Duration::from_secs(40);

/// The handler every row's function exports: it echoes what the event carries, so an
/// allowed attempt's result is the run's own answer.
const HANDLER_SOURCE: &str = r#"
def handler(event, context):
    return {"echo": event["n"]}
"#;

/// Step Functions, IAM, Lambda and CloudWatch over ONE engine.
struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                engine,
            ),
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

    /// One `awsJson` call to the service `signing_name` routes to, requiring a 200.
    async fn rpc(
        &self,
        case: &str,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", Self::authorization(signing_name))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = parsed(response.into_body());
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {answered}"
        );
        answered
    }

    async fn sfn(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc(case, "states", "AWSStepFunctions", operation, body)
            .await
    }

    async fn cloudwatch(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc(
            case,
            "monitoring",
            "GraniteServiceVersion20100801",
            operation,
            body,
        )
        .await
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

    /// One `awsQuery` call to IAM, requiring a 200.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) -> String {
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

    /// A role this service's own principal may assume, carrying `statements` as its
    /// inline policy; answers the ARN IAM's own `CreateRole` reported for it.
    async fn role(&self, case: &str, name: &str, statements: &Value) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "states.amazonaws.com" },
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
        self.put_role_policy(case, name, statements).await;
        xml_element(&created, "Arn").unwrap_or_else(|| {
            panic!("case {case:?}: CreateRole answers the role Arn, got {created}")
        })
    }

    /// Write `statements` as the role's inline policy, which is also how a row GRANTS
    /// something to a role that already exists.
    async fn put_role_policy(&self, case: &str, name: &str, statements: &Value) {
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
    }

    /// The row's function, waited out to [`FUNCTION_ACTIVE`] so an invocation of it is one
    /// Lambda admits; answers its ARN.
    async fn given_function(&self, name: &str) -> String {
        let (status, created) = self
            .lambda(
                "POST",
                FUNCTIONS_URI,
                &json!({
                    "FunctionName": name,
                    "Role": LAMBDA_ROLE_ARN,
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Code": { "ZipFile": package(HANDLER_SOURCE) },
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "CreateFunction must answer 201, got {status} with {created}"
        );
        let arn = string(&created, "FunctionArn");
        let began = Instant::now();
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

    /// Create `definition` as a state machine under `role_arn`; answers its ARN.
    async fn machine(&self, case: &str, name: &str, definition: &Value, role_arn: &str) -> String {
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
        string(&created, "stateMachineArn")
    }

    /// Start one execution of `machine` named `name` with `input`, and report it once it
    /// has left `RUNNING`.
    async fn ran(&self, case: &str, machine: &str, name: &str, input: &Value) -> Value {
        let started = self
            .sfn(
                case,
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": name,
                    "input": input.to_string(),
                }),
            )
            .await;
        let execution = string(&started, "executionArn");
        let began = Instant::now();
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
                began.elapsed() < PATIENCE,
                "case {case:?}: expected a terminal status within {PATIENCE:?}, last saw \
                 {described}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// The SUM the series `metric` of `namespace` reports over the window for the
    /// dimension set `dimensions` — the magnitude a client charts or alarms on. A series
    /// with no datapoint sums to zero, which is what "the series stayed empty" reads as.
    async fn summed(
        &self,
        case: &str,
        namespace: &str,
        metric: &str,
        dimensions: &Value,
    ) -> f64 {
        let now = now_secs();
        let stats = self
            .cloudwatch(
                case,
                "GetMetricStatistics",
                &json!({
                    "Namespace": namespace,
                    "MetricName": metric,
                    "Dimensions": dimensions,
                    "StartTime": now - WINDOW_SECS,
                    "EndTime": now + WINDOW_SECS,
                    "Period": PERIOD_SECS,
                    "Statistics": ["Sum"],
                }),
            )
            .await;
        stats["Datapoints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum()
    }
}

/// Now, in whole epoch seconds, which is how an `awsJson` request carries an instant.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("now is after the epoch")
        .as_secs_f64()
        .trunc()
}

/// The dimension set of a series reported per invoked FUNCTION, as `AWS/States` names it.
fn function_arn_dimensions(function_arn: &str) -> Value {
    json!([{ "Name": LAMBDA_FUNCTION_ARN_DIMENSION, "Value": function_arn }])
}

/// The dimension set of a series reported per STATE MACHINE.
fn machine_dimensions(machine_arn: &str) -> Value {
    json!([{ "Name": STATE_MACHINE_ARN_DIMENSION, "Value": machine_arn }])
}

/// The dimension set `AWS/Lambda` reports an invocation count per.
fn function_name_dimensions(name: &str) -> Value {
    json!([{ "Name": FUNCTION_NAME_DIMENSION, "Value": name }])
}

/// A definition whose one `Task` state's `Resource` is the function's own ARN, retried
/// `retries` times — so the attempts it makes are `retries + 1`.
fn invoking(function_arn: &str, retries: u64) -> Value {
    let mut task = json!({ "Type": "Task", "Resource": function_arn, "End": true });
    if retries > 0 {
        task["Retry"] = json!([{
            "ErrorEquals": ["States.ALL"],
            "IntervalSeconds": 1,
            "MaxAttempts": retries,
            "BackoffRate": 1.0,
        }]);
    }
    json!({ "StartAt": "Invoke", "States": { "Invoke": task } })
}

/// A statement allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> Value {
    json!({ "Effect": "Allow", "Action": action, "Resource": resource })
}

/// One row: what the role's policy says, how many attempts the definition makes, and
/// whether the policies let the call be made.
struct Row {
    case: &'static str,
    /// The statements the role's inline policy carries, over the row's own function ARN.
    statements: fn(&str) -> Value,
    /// How many attempts the definition makes: one, plus the retries it declares.
    attempts: f64,
    /// Whether the policies allow the invocation.
    allowed: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role granted the invocation on the function's own ARN",
            statements: |function| json!([allow(INVOKE_ACTION, function)]),
            attempts: 1.0,
            allowed: true,
        },
        Row {
            case: "a role whose policy says nothing about the invocation",
            statements: |_| json!([allow("logs:PutLogEvents", "*")]),
            attempts: 1.0,
            allowed: false,
        },
        Row {
            case: "a role granted the action on another function",
            statements: |_| {
                json!([allow(
                    INVOKE_ACTION,
                    &format!("arn:aws:lambda:{REGION}:000000000000:function:someone-else")
                )])
            },
            attempts: 1.0,
            allowed: false,
        },
        Row {
            case: "a role whose grant an explicit deny covers",
            statements: |function| {
                json!([
                    allow(INVOKE_ACTION, function),
                    { "Effect": "Deny", "Action": INVOKE_ACTION, "Resource": function },
                ])
            },
            attempts: 1.0,
            allowed: false,
        },
        Row {
            case: "a refused invocation retried twice, so three attempts",
            statements: |_| json!([allow("logs:PutLogEvents", "*")]),
            attempts: 3.0,
            allowed: false,
        },
    ]
}

/// Every attempt a `Task` state makes on the function its `Resource` named is counted as
/// scheduled and started, and closed in the series its outcome names — so an attempt the
/// execution role may not make is counted as a failure of that function's own series
/// while nothing is invoked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_attempt_is_counted_as_a_started_and_failed_invocation() {
    let chain = Chain::new();

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            statements,
            attempts,
            allowed,
        } = row;
        // One function, one role and one state machine per row, so each row's series are
        // its own.
        let name = format!("refused-{index}");
        let function = chain.given_function(&name).await;
        let role = chain
            .role(case, &format!("RefusedRole{index}"), &statements(&function))
            .await;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let retries = attempts as u64 - 1;
        let machine = chain
            .machine(case, &name, &invoking(&function, retries), &role)
            .await;
        let described = chain.ran(case, &machine, "only", &json!({ "n": 7 })).await;

        if allowed {
            assert_eq!(
                (
                    described.get("status").and_then(Value::as_str),
                    described.get("output").and_then(Value::as_str),
                ),
                (Some("SUCCEEDED"), Some(json!({ "echo": 7 }).to_string().as_str())),
                "case {case:?}: expected the allowed attempt's own answer, got {described}"
            );
        } else {
            assert_eq!(
                (
                    described.get("status").and_then(Value::as_str),
                    described.get("error").and_then(Value::as_str),
                ),
                (Some("FAILED"), Some(REFUSED_ERROR)),
                "case {case:?}: expected the refusal to end the execution, got {described}"
            );
            let cause = string(&described, "cause");
            // The principal is the SESSION of the role the call was made as, under the
            // name the assume gave it.
            let principal =
                crate::task_refusal_session_principal::session_principal(&role, &cause);
            for fragment in [REFUSED_TEXT, function.as_str(), principal.as_str()] {
                assert!(
                    cause.contains(fragment),
                    "case {case:?}: expected the cause to carry {fragment:?}, got {cause:?}"
                );
            }
        }

        // Every attempt is scheduled and started, whatever it then did.
        for metric in [SCHEDULED_SERIES, STARTED_SERIES] {
            let counted = chain
                .summed(
                    case,
                    STATES_NAMESPACE,
                    metric,
                    &function_arn_dimensions(&function),
                )
                .await;
            assert!(
                (counted - attempts).abs() < f64::EPSILON,
                "case {case:?}: expected {metric} to count {attempts} attempt(s) of {function}, \
                 got {counted}"
            );
        }
        // And each closes in the series its outcome names, and in no other.
        for (metric, expected) in [
            (FAILED_SERIES, if allowed { 0.0 } else { attempts }),
            (SUCCEEDED_SERIES, if allowed { attempts } else { 0.0 }),
        ] {
            let counted = chain
                .summed(
                    case,
                    STATES_NAMESPACE,
                    metric,
                    &function_arn_dimensions(&function),
                )
                .await;
            assert!(
                (counted - expected).abs() < f64::EPSILON,
                "case {case:?}: expected {metric} to count {expected} of {function}, got {counted}"
            );
        }
        // The execution itself is counted under its state machine, in the one series its
        // own outcome names.
        for (metric, expected) in [
            (EXECUTIONS_FAILED_SERIES, f64::from(u8::from(!allowed))),
            (EXECUTIONS_SUCCEEDED_SERIES, f64::from(u8::from(allowed))),
        ] {
            let counted = chain
                .summed(
                    case,
                    STATES_NAMESPACE,
                    metric,
                    &machine_dimensions(&machine),
                )
                .await;
            assert!(
                (counted - expected).abs() < f64::EPSILON,
                "case {case:?}: expected {metric} to count {expected} for {machine}, got {counted}"
            );
        }
        // Lambda meters the invocations it ADMITTED, so a refused attempt leaves the
        // function's own series empty however many times it was attempted.
        let invoked = chain
            .summed(
                case,
                LAMBDA_NAMESPACE,
                INVOCATIONS_SERIES,
                &function_name_dimensions(&name),
            )
            .await;
        let expected = if allowed { 1.0 } else { 0.0 };
        assert!(
            (invoked - expected).abs() < f64::EPSILON,
            "case {case:?}: expected {expected} invocation(s) of {name} in \
             {LAMBDA_NAMESPACE}/{INVOCATIONS_SERIES}, got {invoked}"
        );
    }
}

/// The role's documents are read for each call, so granting the action to the SAME role
/// afterwards lets the SAME state machine's next execution succeed — and the series count
/// each execution's own attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn granting_the_action_lets_the_same_machine_succeed_afterwards() {
    let case = "the grant arrives after the first execution";
    let chain = Chain::new();
    let name = "regranted";
    let function = chain.given_function(name).await;
    let role = chain
        .role(
            case,
            "RegrantedRole",
            &json!([allow("logs:PutLogEvents", "*")]),
        )
        .await;
    let machine = chain
        .machine(case, name, &invoking(&function, 0), &role)
        .await;

    let refused = chain.ran(case, &machine, "denied", &json!({ "n": 7 })).await;
    assert_eq!(
        (
            refused.get("status").and_then(Value::as_str),
            refused.get("error").and_then(Value::as_str),
        ),
        (Some("FAILED"), Some(REFUSED_ERROR)),
        "case {case:?}: expected the first execution to be refused, got {refused}"
    );

    chain
        .put_role_policy(
            case,
            "RegrantedRole",
            &json!([
                allow("logs:PutLogEvents", "*"),
                allow(INVOKE_ACTION, &function),
            ]),
        )
        .await;

    let allowed = chain
        .ran(case, &machine, "granted", &json!({ "n": 9 }))
        .await;
    assert_eq!(
        (
            allowed.get("status").and_then(Value::as_str),
            allowed.get("output").and_then(Value::as_str),
        ),
        (
            Some("SUCCEEDED"),
            Some(json!({ "echo": 9 }).to_string().as_str())
        ),
        "case {case:?}: expected the granted execution to answer the handler's payload, got \
         {allowed}"
    );

    // Two executions, two attempts: each scheduled and started, one closed as a failure
    // and one as a success, and the one invocation Lambda admitted metered once.
    for (metric, expected) in [
        (SCHEDULED_SERIES, 2.0),
        (STARTED_SERIES, 2.0),
        (FAILED_SERIES, 1.0),
        (SUCCEEDED_SERIES, 1.0),
    ] {
        let counted = chain
            .summed(
                case,
                STATES_NAMESPACE,
                metric,
                &function_arn_dimensions(&function),
            )
            .await;
        assert!(
            (counted - expected).abs() < f64::EPSILON,
            "case {case:?}: expected {metric} to count {expected} of {function}, got {counted}"
        );
    }
    for (metric, expected) in [
        (EXECUTIONS_FAILED_SERIES, 1.0),
        (EXECUTIONS_SUCCEEDED_SERIES, 1.0),
    ] {
        let counted = chain
            .summed(
                case,
                STATES_NAMESPACE,
                metric,
                &machine_dimensions(&machine),
            )
            .await;
        assert!(
            (counted - expected).abs() < f64::EPSILON,
            "case {case:?}: expected {metric} to count {expected} for {machine}, got {counted}"
        );
    }
    let invoked = chain
        .summed(
            case,
            LAMBDA_NAMESPACE,
            INVOCATIONS_SERIES,
            &function_name_dimensions(name),
        )
        .await;
    assert!(
        (invoked - 1.0).abs() < f64::EPSILON,
        "case {case:?}: expected the one admitted invocation of {name} to be metered, got \
         {invoked}"
    );
}
