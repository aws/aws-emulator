// SPDX-License-Identifier: CC0-1.0

//! What a pipeline run DOES with a callback step: the message it hands the caller's own
//! Amazon SQS queue, the token it waits for, and how reporting that token back settles the
//! step and then the run.
//!
//! The rules under test:
//!
//! * a dispatched callback step really hands its work over — ONE message on the queue the
//!   step names, carrying the token the run minted and the step's own arguments — and the
//!   step is reported `Executing`, with the queue and the token in its
//!   `Metadata.Callback`, for as long as it waits; the run stays `Executing` with it,
//!   whatever the clock says;
//! * reporting that token back settles the step and then the run:
//!   `SendPipelineExecutionStepSuccess` with output parameters makes both `Succeeded` and
//!   reports those parameters, and `SendPipelineExecutionStepFailure` makes both `Failed`
//!   with the reason the worker gave;
//! * a token no step is waiting for settles nothing: it is the modelled `ResourceNotFound`,
//!   and a token whose step has already stopped executing is the modelled
//!   `ConflictException`;
//! * a run that performs no step hands no work over: one whose `RoleArn` resolves to
//!   nothing, and one whose steps are of a kind this emulator has no runtime for, leave the
//!   queue empty and end `Failed` with the reason that says why.
//!
//! The first rules reach ANOTHER service (Amazon SQS, and IAM for the role), so this suite
//! mounts those beside SageMaker on ONE shared engine, exactly as the router does, and
//! every case goes over the wire through the same `MultiServiceRouter`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state;
use smithy_server_runtime::MultiServiceRouter;

use super::inference_deployment_lifecycle::{error_code, message, str_field, ACCOUNT};
use super::pipeline_lifecycle::{token, DEFINITION_VERSION};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// How long [`Emulator::settled_execution`] looks for a terminal status before giving up:
/// generous, because what it waits for is the condition rather than a span, and the spans
/// the clock owes a run here are sub-second.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long [`Emulator::settled_execution`] waits between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The `PipelineExecutionStatus` values a run never leaves
/// (`smithy_query(query="shape",shape="PipelineExecutionStatus")`).
const TERMINAL_RUN_STATUSES: [&str; 4] = ["Failed", "Succeeded", "Stopped", "CompletedWithFailures"];

/// A SigV4 credential scope naming `service`, which is what routes a request to it. The key
/// id and the signature are not credentials: this harness turns request authentication off,
/// so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// SageMaker, Amazon SQS and IAM on ONE shared engine, the way the router mounts every
/// service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One JSON-RPC call to `service`, as a client makes it.
    async fn call(&self, service: &str, target: &str, version: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{service}.{REGION}.amazonaws.com/"))
            .header("host", format!("{service}.{REGION}.amazonaws.com"))
            .header("content-type", format!("application/x-amz-json-{version}"))
            .header("x-amz-target", target)
            .header("authorization", authorization(service))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One SageMaker call (`awsJson1_1`).
    async fn sagemaker(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call(
            "sagemaker",
            &format!("SageMaker.{operation}"),
            "1.1",
            body,
        )
        .await
    }

    /// One SageMaker call that must be served.
    async fn sagemaker_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, answered) = self.sagemaker(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {answered}"
        );
        answered
    }

    /// One Amazon SQS call (`awsJson1_0`) that must be served.
    async fn sqs_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, answered) = self
            .call("sqs", &format!("AmazonSQS.{operation}"), "1.0", body)
            .await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {answered}"
        );
        answered
    }

    /// A queue named `name`, as its url.
    async fn given_queue(&self, name: &str) -> String {
        let created = self.sqs_ok("CreateQueue", &json!({ "QueueName": name })).await;
        str_field(&created, "QueueUrl")
    }

    /// Every message on `queue_url` right now, as the bodies they carry.
    async fn messages(&self, queue_url: &str) -> Vec<String> {
        let received = self
            .sqs_ok(
                "ReceiveMessage",
                &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
            )
            .await;
        received
            .get("Messages")
            .and_then(Value::as_array)
            .map(|messages| {
                messages
                    .iter()
                    .map(|held| str_field(held, "Body"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The steps of `execution_arn`, as `ListPipelineExecutionSteps` reports them.
    async fn steps(&self, execution_arn: &str) -> Vec<Value> {
        let listed = self
            .sagemaker_ok(
                "ListPipelineExecutionSteps",
                &json!({ "PipelineExecutionArn": execution_arn }),
            )
            .await;
        listed
            .get("PipelineExecutionSteps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// `DescribePipelineExecution` for `execution_arn`.
    async fn execution(&self, execution_arn: &str) -> Value {
        self.sagemaker_ok(
            "DescribePipelineExecution",
            &json!({ "PipelineExecutionArn": execution_arn }),
        )
        .await
    }

    /// `DescribePipelineExecution` for `execution_arn`, looked at again until it reports a
    /// TERMINAL status or `SETTLE_DEADLINE` passes.
    ///
    /// The condition is what is waited for, not a span: the run's terminal status is owed by
    /// the clock, and polling for it keeps the wait right on a host of any speed.
    async fn settled_execution(&self, case: &str, execution_arn: &str) -> Value {
        let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let reported = self.execution(execution_arn).await;
            let status = str_field(&reported, "PipelineExecutionStatus");
            if TERMINAL_RUN_STATUSES.contains(&status.as_str()) {
                return reported;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: the run must reach a terminal status, last reported \
                 {reported}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Wait until the span the clock owes a run has certainly passed, by starting a run
    /// `witness` that has nothing to perform and looking again until IT has settled.
    ///
    /// The condition is the witness's own terminal status, not a span: the witness is
    /// started AFTER the run under test and the clock owes both the same span
    /// (`state::PIPELINE_EXECUTION_ENDS_AFTER`), so a witness that has settled proves the
    /// run under test is past its span too — on a host of any speed.
    async fn past_the_execution_span(&self, case: &str, witness: &str) {
        let execution = self
            .given_run(
                witness,
                &format!("arn:aws:iam::{ACCOUNT}:role/{witness}"),
                json!([{
                    "Name": "Nothing",
                    "Type": "Training",
                    "Arguments": { "TrainingJobName": witness },
                }]),
            )
            .await;
        self.settled_execution(case, &execution).await;
    }

    /// Register `name` around `role`, with `steps`, and start a run of it: answers the
    /// run's ARN.
    async fn given_run(&self, name: &str, role: &str, steps: Value) -> String {
        let definition =
            json!({ "Version": DEFINITION_VERSION, "Metadata": {}, "Steps": steps }).to_string();
        self.sagemaker_ok(
            "CreatePipeline",
            &json!({
                "PipelineName": name,
                "ClientRequestToken": token(name),
                "RoleArn": role,
                "PipelineDefinition": definition,
            }),
        )
        .await;
        let started = self
            .sagemaker_ok(
                "StartPipelineExecution",
                &json!({ "PipelineName": name, "ClientRequestToken": token(&format!("run{name}")) }),
            )
            .await;
        str_field(&started, "PipelineExecutionArn")
    }
}

/// A role this account's IAM holds, whose trust document admits SageMaker — so a run under
/// it is not blamed on its role.
async fn given_role(emulator: &Emulator, name: &str) -> String {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sagemaker.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://iam.{REGION}.amazonaws.com/"))
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(bytes::Bytes::from(format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}&AssumeRolePolicyDocument={}",
            form_encode(&policy)
        )))
        .expect("a well-formed request");
    let response = emulator.router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "the role {name} must be created, got {}",
        String::from_utf8_lossy(response.body())
    );
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// `value`, percent-encoded for an `application/x-www-form-urlencoded` body with the
/// maintained RFC 3986 encoder, leaving only the unreserved characters literal.
fn form_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A `Callback` step named `name`, handing its work over through `queue_url` with
/// `arguments`, in the shape the SageMaker Python SDK's `CallbackStep` serialises (the queue
/// beside the step's `Arguments`).
fn callback_step(name: &str, queue_url: &str, arguments: Value) -> Value {
    json!({
        "Name": name,
        "Type": "Callback",
        "SqsQueueUrl": queue_url,
        "Arguments": arguments,
        "OutputParameters": [{ "OutputName": "result", "OutputParameterType": "String" }],
    })
}

/// The sole step of `steps`, panicking with `case` when the run reports another number.
fn sole_step(case: &str, steps: &[Value]) -> Value {
    assert_eq!(
        steps.len(),
        1,
        "case {case:?}: the run performs one step, got {steps:?}"
    );
    steps[0].clone()
}

/// The `Metadata.Callback` of a step, which only a dispatched callback step carries.
fn callback_metadata(case: &str, step: &Value) -> Value {
    step.get("Metadata")
        .and_then(|metadata| metadata.get("Callback"))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a dispatched callback step reports its Metadata.Callback, got {step}")
        })
}

/// RULE: a dispatched callback step hands its work to the queue it names — one message
/// carrying the token the run minted and the step's own arguments — reports itself
/// `Executing` with that queue and token while it waits, and holds the run open whatever
/// the clock says; reporting the token back settles the step and then the run, with the
/// outcome and the output parameters the worker reported.
#[tokio::test]
async fn a_callback_step_hands_its_work_over_and_waits_for_its_token() {
    // Rows differ along what the worker reports back and what the step handed over: a
    // success with output parameters, a success with none, and a failure with its own
    // reason.
    struct Row {
        case: &'static str,
        arguments: Value,
        /// `Some(output parameters)` reports success; `None` reports failure.
        outputs: Option<Value>,
        failure_reason: Option<&'static str>,
        expected_step_status: &'static str,
        expected_run_status: &'static str,
    }
    let rows = [
        Row {
            case: "a worker that succeeds with output parameters",
            arguments: json!({ "input_location": "s3://bucket/in", "retries": 2 }),
            outputs: Some(json!([
                { "Name": "result", "Value": "ok" },
                { "Name": "rows", "Value": "17" },
            ])),
            failure_reason: None,
            expected_step_status: "Succeeded",
            expected_run_status: "Succeeded",
        },
        Row {
            case: "a worker that succeeds with no output parameters",
            arguments: json!({}),
            outputs: Some(json!([])),
            failure_reason: None,
            expected_step_status: "Succeeded",
            expected_run_status: "Succeeded",
        },
        Row {
            case: "a worker that fails with its own reason",
            arguments: json!({ "input_location": "s3://bucket/other" }),
            outputs: None,
            failure_reason: Some("the worker could not read its input"),
            expected_step_status: "Failed",
            expected_run_status: "Failed",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let queue_url = emulator.given_queue(&format!("callbacks{index}")).await;
        let role = given_role(&emulator, &format!("pipeexec{index}")).await;
        let execution = emulator
            .given_run(
                &format!("callbackpipe{index}"),
                &role,
                json!([callback_step("HandOver", &queue_url, row.arguments.clone())]),
            )
            .await;

        // ONE message, carrying the token this run minted and the step's own arguments.
        let delivered = emulator.messages(&queue_url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the step hands its work over with one message, got {delivered:?}"
        );
        let body: Value =
            serde_json::from_str(&delivered[0]).expect("the callback message is JSON");
        assert_eq!(
            body.get("arguments"),
            Some(&row.arguments),
            "case {case:?}: the message carries the step's arguments, got {body}"
        );
        let token_in_message = str_field(&body, "token");

        // While it waits, the step is executing and names the queue and the token it was
        // dispatched with; the run is held open with it, even once the span the clock owes
        // it has passed — which a witness run, not a timer, is what establishes.
        emulator
            .past_the_execution_span(case, &format!("witness{index}"))
            .await;
        let step = sole_step(case, &emulator.steps(&execution).await);
        assert_eq!(
            str_field(&step, "StepStatus"),
            "Executing",
            "case {case:?}: a step waiting for its token is executing, got {step}"
        );
        assert!(
            step.get("StartTime").is_some() && step.get("EndTime").is_none(),
            "case {case:?}: a waiting step started and has not stopped, got {step}"
        );
        let metadata = callback_metadata(case, &step);
        assert_eq!(
            (
                str_field(&metadata, "SqsQueueUrl"),
                str_field(&metadata, "CallbackToken")
            ),
            (queue_url.clone(), token_in_message.clone()),
            "case {case:?}: the step names the queue it handed its work to and the token \
             the message carried, got {metadata}"
        );
        assert_eq!(
            str_field(&emulator.execution(&execution).await, "PipelineExecutionStatus"),
            "Executing",
            "case {case:?}: a step waiting for its token holds the run open"
        );

        // Reporting the token back settles the step and then the run.
        let answered = match (&row.outputs, row.failure_reason) {
            (Some(outputs), _) => {
                emulator
                    .sagemaker_ok(
                        "SendPipelineExecutionStepSuccess",
                        &json!({
                            "CallbackToken": token_in_message,
                            "OutputParameters": outputs,
                            "ClientRequestToken": token(&format!("cb{index}")),
                        }),
                    )
                    .await
            }
            (None, reason) => {
                emulator
                    .sagemaker_ok(
                        "SendPipelineExecutionStepFailure",
                        &json!({
                            "CallbackToken": token_in_message,
                            "FailureReason": reason.unwrap_or_default(),
                            "ClientRequestToken": token(&format!("cb{index}")),
                        }),
                    )
                    .await
            }
        };
        assert_eq!(
            str_field(&answered, "PipelineExecutionArn"),
            execution,
            "case {case:?}: the call answers the run the token belonged to, got {answered}"
        );

        let step = sole_step(case, &emulator.steps(&execution).await);
        assert_eq!(
            str_field(&step, "StepStatus"),
            row.expected_step_status,
            "case {case:?}: the step's settled status, got {step}"
        );
        assert!(
            step.get("EndTime").is_some(),
            "case {case:?}: a settled step stopped executing, got {step}"
        );
        assert_eq!(
            step.get("AttemptCount").and_then(Value::as_i64),
            Some(1),
            "case {case:?}: the step was attempted once, got {step}"
        );
        if let Some(outputs) = &row.outputs {
            assert_eq!(
                callback_metadata(case, &step).get("OutputParameters"),
                Some(outputs),
                "case {case:?}: the step reports the output parameters the worker sent, got \
                 {step}"
            );
        }
        let settled = emulator.execution(&execution).await;
        assert_eq!(
            str_field(&settled, "PipelineExecutionStatus"),
            row.expected_run_status,
            "case {case:?}: the run's settled status, got {settled}"
        );
        match row.failure_reason {
            Some(reason) => {
                // The STEP carries the worker's own reason; the RUN reports the service's
                // one sentence for a run a step failure ended.
                assert_eq!(
                    step.get("FailureReason").and_then(Value::as_str),
                    Some(reason),
                    "case {case:?}: the step reports the worker's own reason, got {step}"
                );
                assert_eq!(
                    str_field(&settled, "FailureReason"),
                    state::PIPELINE_EXECUTION_STEP_FAILURE,
                    "case {case:?}: the run reports that a step of it failed, got {settled}"
                );
            }
            None => assert!(
                settled.get("FailureReason").is_none(),
                "case {case:?}: a succeeded run reports no failure reason, got {settled}"
            ),
        }
    }
}

/// RULE: a callback token settles a step exactly once — a token no step is waiting for is
/// the modelled `ResourceNotFound`, and one whose step has already stopped executing is the
/// modelled `ConflictException`.
#[tokio::test]
async fn a_callback_token_settles_its_step_exactly_once() {
    let emulator = Emulator::new();
    let queue_url = emulator.given_queue("callbacks-once").await;
    let role = given_role(&emulator, "pipeexec-once").await;
    let execution = emulator
        .given_run(
            "callbackpipeonce",
            &role,
            json!([callback_step("HandOver", &queue_url, json!({}))]),
        )
        .await;
    let delivered = emulator.messages(&queue_url).await;
    let body: Value = serde_json::from_str(&delivered[0]).expect("the callback message is JSON");
    let held = str_field(&body, "token");

    // Rows: a token nothing ever minted, and the token of a step that has already settled.
    // Both are the modelled length (`CallbackToken` is `len 10..10`), so each reaches the
    // handler rather than the boundary validator.
    let settling_token = token("settle-once");
    emulator
        .sagemaker_ok(
            "SendPipelineExecutionStepSuccess",
            &json!({
                "CallbackToken": held,
                "OutputParameters": [],
                "ClientRequestToken": settling_token,
            }),
        )
        .await;
    // A RETRY of that very call completes no more than once: it answers what it answered.
    let replayed = emulator
        .sagemaker_ok(
            "SendPipelineExecutionStepSuccess",
            &json!({
                "CallbackToken": held,
                "OutputParameters": [],
                "ClientRequestToken": settling_token,
            }),
        )
        .await;
    assert_eq!(
        str_field(&replayed, "PipelineExecutionArn"),
        execution,
        "case \"a retry of the settling call\": it answers that run again, got {replayed}"
    );
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a token nothing minted, reported successful",
            "SendPipelineExecutionStepSuccess",
            "0123456789",
            "ResourceNotFound",
        ),
        (
            "a token nothing minted, reported failed",
            "SendPipelineExecutionStepFailure",
            "abcdefghij",
            "ResourceNotFound",
        ),
        (
            "a settled step's token, reported successful again",
            "SendPipelineExecutionStepSuccess",
            held.as_str(),
            "ConflictException",
        ),
        (
            "a settled step's token, reported failed",
            "SendPipelineExecutionStepFailure",
            held.as_str(),
            "ConflictException",
        ),
    ];
    for (case, operation, presented, expected) in rows {
        let (status, answered) = emulator
            .sagemaker(
                operation,
                &json!({ "CallbackToken": presented, "FailureReason": "late" }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} with {answered}"
        );
        assert!(
            !message(&answered).contains(presented) || expected == "ConflictException",
            "case {case:?}: an unknown token is not quoted back, got {answered}"
        );
    }

    // The step the first report settled still reports that outcome: a refused report
    // changes nothing.
    let step = sole_step("settled-once", &emulator.steps(&execution).await);
    assert_eq!(
        str_field(&step, "StepStatus"),
        "Succeeded",
        "case \"settled-once\": the step keeps the outcome its token reported, got {step}"
    );
}

/// RULE: a run that performs no step hands no work over to any queue — one whose `RoleArn`
/// resolves to nothing, one whose steps are of a kind this emulator has no runtime for, and
/// one whose callback step names no queue of this account — and each ends `Failed`: the STEP
/// carries the reason that says why, and the run carries that same reason unless a step of
/// it FAILED, which the run reports with the service's own step-failure sentence.
#[tokio::test]
async fn a_run_that_performs_no_step_hands_no_work_over() {
    let absent_role = format!("arn:aws:iam::{ACCOUNT}:role/never-created");
    let no_queue =
        service_com_amazonaws_sagemaker_sagemaker::state::callback_step_names_no_queue();

    // Rows: the role nothing holds with a callback step to perform, a held role with a step
    // kind nothing here performs, and a callback step naming no queue of this account.
    let rows: Vec<(&str, String, Value, String, String, bool)> = vec![
        (
            "a role this account holds nothing under",
            absent_role.clone(),
            json!("callback"),
            format!("Role {absent_role} does not exist."),
            format!("Role {absent_role} does not exist."),
            false,
        ),
        (
            "a step kind nothing here performs",
            String::new(),
            json!("training"),
            service_com_amazonaws_sagemaker_sagemaker::state::PIPELINE_EXECUTION_NOT_RUN.to_string(),
            service_com_amazonaws_sagemaker_sagemaker::state::PIPELINE_EXECUTION_NOT_RUN.to_string(),
            false,
        ),
        (
            "a callback step naming no queue of this account",
            String::new(),
            json!("unaddressable"),
            no_queue,
            state::PIPELINE_EXECUTION_STEP_FAILURE.to_string(),
            true,
        ),
    ];

    for (index, (case, role, kind, expected_reason, expected_run_reason, dispatched)) in
        rows.iter().enumerate()
    {
        let emulator = Emulator::new();
        let queue_url = emulator.given_queue(&format!("unperformed{index}")).await;
        let role = if role.is_empty() {
            given_role(&emulator, &format!("heldrole{index}")).await
        } else {
            role.clone()
        };
        let steps = match kind.as_str().unwrap_or_default() {
            "callback" => {
                json!([callback_step("HandOver", &queue_url, json!({ "marker": case }))])
            }
            // A url whose path names no queue of this account at all: the step could hand
            // its work to nothing, whatever host it wrote.
            "unaddressable" => {
                json!([callback_step("HandOver", "https://example.test/", json!({}))])
            }
            _ => json!([{
                "Name": "Train",
                "Type": "Training",
                "Arguments": { "TrainingJobName": "nothing-runs-this" },
            }]),
        };
        let execution = emulator
            .given_run(&format!("unperformed{index}"), &role, steps)
            .await;

        assert!(
            emulator.messages(&queue_url).await.is_empty(),
            "case {case:?}: a run that performs no step delivers no message"
        );
        let settled = emulator.settled_execution(case, &execution).await;
        assert_eq!(
            (
                str_field(&settled, "PipelineExecutionStatus"),
                str_field(&settled, "FailureReason")
            ),
            ("Failed".to_string(), expected_run_reason.clone()),
            "case {case:?}: the run's terminal status and reason, got {settled}"
        );
        let step = sole_step(case, &emulator.steps(&execution).await);
        assert_eq!(
            (
                str_field(&step, "StepStatus"),
                str_field(&step, "FailureReason")
            ),
            ("Failed".to_string(), expected_reason.clone()),
            "case {case:?}: the step's status and reason, got {step}"
        );
        assert_eq!(
            step.get("Metadata").is_some(),
            *dispatched,
            "case {case:?}: only a step the run DISPATCHED reports metadata, got {step}"
        );
    }
}

/// RULE: a stop of a run with a callback step still waiting hands that step the additional
/// message its doc describes — the same token and arguments with the `Status` field set to
/// `Stopping`, at the address the dispatch's own message went to whatever form the step's
/// declared url took — and the run stays `Stopping` until the token is reported back, after
/// which it is `Stopped` rather than succeeded.
#[tokio::test]
async fn a_stop_tells_a_waiting_callback_step_and_waits_for_its_token() {
    let emulator = Emulator::new();
    let queue_url = emulator.given_queue("callbacks-stop").await;
    let role = given_role(&emulator, "pipeexec-stop").await;
    let arguments = json!({ "input_location": "s3://bucket/in" });
    // The step declares a NON-CANONICAL url for that very queue — another Region's
    // authority over the same `/<account>/<name>` path — so both messages are addressed by
    // the queue the url names rather than by the url itself.
    let declared_url = queue_url.replace("sqs.us-east-1", "sqs.ap-south-1");
    assert_ne!(
        declared_url, queue_url,
        "case \"stop\": the declared url must differ from the canonical one"
    );
    let execution = emulator
        .given_run(
            "callbackpipestop",
            &role,
            json!([callback_step("HandOver", &declared_url, arguments.clone())]),
        )
        .await;
    let dispatched = emulator.messages(&queue_url).await;
    let body: Value = serde_json::from_str(&dispatched[0]).expect("the callback message is JSON");
    let held = str_field(&body, "token");

    let stopped = emulator
        .sagemaker_ok(
            "StopPipelineExecution",
            &json!({
                "PipelineExecutionArn": execution,
                "ClientRequestToken": token("stop-once"),
            }),
        )
        .await;
    assert_eq!(
        str_field(&stopped, "PipelineExecutionArn"),
        execution,
        "case \"stop\": the call answers the run it stopped, got {stopped}"
    );
    assert_eq!(
        str_field(&emulator.execution(&execution).await, "PipelineExecutionStatus"),
        "Stopping",
        "case \"stop\": a run with a step still waiting is stopping, not stopped"
    );

    // The additional message: the same token and arguments, plus the documented `Status`.
    let told = emulator.messages(&queue_url).await;
    assert_eq!(
        told.len(),
        1,
        "case \"stop\": the stop hands the waiting step one additional message, got {told:?}"
    );
    let stopping: Value = serde_json::from_str(&told[0]).expect("the stop message is JSON");
    assert_eq!(
        (
            str_field(&stopping, "token"),
            stopping.get("arguments").cloned(),
            str_field(&stopping, "Status")
        ),
        (held.clone(), Some(arguments), "Stopping".to_string()),
        "case \"stop\": the additional message repeats the token and arguments and names \
         the status, got {stopping}"
    );

    // A retry of the stop changes nothing and tells the worker nothing further.
    emulator
        .sagemaker_ok(
            "StopPipelineExecution",
            &json!({
                "PipelineExecutionArn": execution,
                "ClientRequestToken": token("stop-again"),
            }),
        )
        .await;
    assert!(
        emulator.messages(&queue_url).await.is_empty(),
        "case \"stop-again\": a retry of the stop hands no second message over"
    );
    emulator
        .past_the_execution_span("stop-again", "stopwitness")
        .await;
    assert_eq!(
        str_field(&emulator.execution(&execution).await, "PipelineExecutionStatus"),
        "Stopping",
        "case \"stop-again\": the run stops only once its waiting step reports back"
    );

    // Only when the token is reported back does the run stop.
    emulator
        .sagemaker_ok(
            "SendPipelineExecutionStepSuccess",
            &json!({ "CallbackToken": held, "OutputParameters": [] }),
        )
        .await;
    let settled = emulator.settled_execution("stopped", &execution).await;
    assert_eq!(
        str_field(&settled, "PipelineExecutionStatus"),
        "Stopped",
        "case \"stopped\": a run the caller stopped is stopped, not succeeded, got {settled}"
    );
    let step = sole_step("stopped", &emulator.steps(&execution).await);
    assert_eq!(
        str_field(&step, "StepStatus"),
        "Succeeded",
        "case \"stopped\": the step keeps the outcome its worker reported, got {step}"
    );

    // A run that has stopped is past stopping.
    let (status, answered) = emulator
        .sagemaker(
            "StopPipelineExecution",
            &json!({
                "PipelineExecutionArn": execution,
                "ClientRequestToken": token("stop-terminal"),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ConflictException"),
        "case \"stop-terminal\": expected 400 ConflictException, got {status} with {answered}"
    );
    assert!(
        message(&answered).contains(&execution) && message(&answered).contains("Stopped"),
        "case \"stop-terminal\": the refusal names the run and the status it is in, got \
         {answered}"
    );
}

/// RULE: a stop resolves the run it names — an ARN nothing holds is the modelled
/// `ResourceNotFound` — and a run that has already ended cannot be stopped.
#[tokio::test]
async fn a_stop_resolves_its_run_and_refuses_one_that_has_ended() {
    let emulator = Emulator::new();
    let role = given_role(&emulator, "pipeexec-ended").await;
    let execution = emulator
        .given_run(
            "endedpipe",
            &role,
            json!([{
                "Name": "Train",
                "Type": "Training",
                "Arguments": { "TrainingJobName": "nothing-runs-this" },
            }]),
        )
        .await;

    // Rows: an ARN of this account's own form that names no run, one of another pipeline's
    // run, and the run that has ended.
    assert_eq!(
        str_field(
            &emulator.settled_execution("ended", &execution).await,
            "PipelineExecutionStatus"
        ),
        "Failed",
        "case \"ended\": the run with nothing to perform has ended"
    );
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "an execution nothing started",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:pipeline/endedpipe/execution/absent"),
            "ResourceNotFound",
        ),
        (
            "an execution of a pipeline nobody created",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:pipeline/nopipe/execution/absent"),
            "ResourceNotFound",
        ),
        (
            "a run that has already ended",
            execution.clone(),
            "ConflictException",
        ),
    ];
    for (case, arn, expected) in rows {
        let (status, answered) = emulator
            .sagemaker(
                "StopPipelineExecution",
                &json!({ "PipelineExecutionArn": arn, "ClientRequestToken": token(case) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} with {answered}"
        );
        assert!(
            message(&answered).contains(&arn),
            "case {case:?}: the refusal names the run it was given, got {answered}"
        );
    }
}

/// RULE: a run performs the steps it CAN and still ends `Failed` for the ones nothing here
/// performs: the callback step of a mixed definition hands its work over and settles on its
/// token, while the undispatched step keeps the run from succeeding.
#[tokio::test]
async fn a_mixed_definition_performs_its_callback_step_and_still_fails() {
    let emulator = Emulator::new();
    let queue_url = emulator.given_queue("mixed-callbacks").await;
    let role = given_role(&emulator, "mixedexec").await;
    let execution = emulator
        .given_run(
            "mixedpipe",
            &role,
            json!([
                callback_step("HandOver", &queue_url, json!({ "marker": "mixed" })),
                {
                    "Name": "Train",
                    "Type": "Training",
                    "Arguments": { "TrainingJobName": "nothing-runs-this" },
                },
            ]),
        )
        .await;

    // Only the callback step was dispatched, so only it is reported while the run executes.
    let delivered = emulator.messages(&queue_url).await;
    assert_eq!(
        delivered.len(),
        1,
        "case \"mixed\": the callback step hands its work over, got {delivered:?}"
    );
    let body: Value = serde_json::from_str(&delivered[0]).expect("the callback message is JSON");
    let step = sole_step("mixed-while-executing", &emulator.steps(&execution).await);
    assert_eq!(
        str_field(&step, "StepName"),
        "HandOver",
        "case \"mixed\": only the dispatched step is reported while the run executes, got \
         {step}"
    );

    emulator
        .sagemaker_ok(
            "SendPipelineExecutionStepSuccess",
            &json!({ "CallbackToken": str_field(&body, "token"), "OutputParameters": [] }),
        )
        .await;
    let settled = emulator.settled_execution("mixed", &execution).await;
    assert_eq!(
        (
            str_field(&settled, "PipelineExecutionStatus"),
            str_field(&settled, "FailureReason")
        ),
        (
            "Failed".to_string(),
            service_com_amazonaws_sagemaker_sagemaker::state::PIPELINE_EXECUTION_NOT_RUN.to_string()
        ),
        "case \"mixed\": a step nothing performed keeps the run from succeeding, got \
         {settled}"
    );
    let steps = emulator.steps(&execution).await;
    assert_eq!(
        steps.len(),
        2,
        "case \"mixed\": the settled run reports both declared steps, got {steps:?}"
    );
    let performed = steps
        .iter()
        .find(|step| str_field(step, "StepName") == "HandOver")
        .unwrap_or_else(|| panic!("case \"mixed\": the performed step is listed, got {steps:?}"));
    assert_eq!(
        str_field(performed, "StepStatus"),
        "Succeeded",
        "case \"mixed\": the performed step keeps the outcome its worker reported, got \
         {steps:?}"
    );
}
