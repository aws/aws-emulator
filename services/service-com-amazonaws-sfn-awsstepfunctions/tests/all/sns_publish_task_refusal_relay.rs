// SPDX-License-Identifier: CC0-1.0

//! What an `arn:aws:states:::sns:publish` Task reports when Amazon SNS refuses the
//! publish, driven over each service's own wire protocol through the production
//! `MultiServiceRouter` with Step Functions, IAM, SNS and SQS mounted on ONE engine —
//! the shape `core/router/src/lib.rs` mounts them in.
//!
//! Two rules are held here, both about the refusal an invoked service's SDK hands the
//! state:
//!
//! 1. the ERROR NAME is the invoked service as Step Functions names it in a task error
//!    and the exception class that service's own client resolves the answer to, which
//!    for an `awsQuery` service is the class of the ERROR SHAPE its model binds the
//!    legacy query code to — not that code with an `Exception` suffix;
//! 2. the CAUSE is that exception's own message: the service's sentence, then the
//!    metadata the answer carried, the id of the refused request included.
//!
//! The rows differ along what each rule generalises over: which refusal of SNS the
//! publish meets and therefore whether its query code names its own shape (`NotFound`,
//! `InvalidParameter`) or another (`ParameterValueInvalid`), which member of the request
//! the refusal is about, where the role sits in IAM and which state machine the work is
//! for, and whether the state carries a `Catch` that routes the failure or lets the
//! execution fail.
//!
//! The session the refusal blames is the one the ASSUME gave the role, so its NAME is a
//! value of that run's own: it is read back out of the cause and held to what a session
//! name may be (`tests/all/task_refusal_session_principal.rs`'s `session_principal`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_TASK_FAILED;
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::execution_data_encryption::{form_encode, xml_element};
use crate::state_machine_lifecycle::{string, REGION};
use crate::task_execution_role::trust_document;
use crate::task_refusal_session_principal::session_principal;

/// The access-key id every request below is signed with: the account's own published
/// credential, which no identity policy governs.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// SNS's signing name and awsQuery API version, and SQS's `awsJson1_0` wire facts
/// (`smithy_query(query="service_info")` on each service's own model).
const SNS_SIGNING_NAME: &str = "sns";
const SNS_API_VERSION: &str = "2010-03-31";
const SQS_TARGET_PREFIX: &str = "AmazonSQS";
const SQS_SIGNING_NAME: &str = "sqs";

/// IAM's signing name and awsQuery API version.
const IAM_SIGNING_NAME: &str = "iam";
const IAM_API_VERSION: &str = "2010-05-08";

/// Step Functions' own wire facts.
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";
const SFN_SIGNING_NAME: &str = "states";

/// How long a test waits for an execution to leave `RUNNING`, polled for the condition
/// rather than slept through.
const PATIENCE: Duration = Duration::from_secs(30);

/// The four services over ONE engine, so the publish a task makes is served by the very
/// store this suite's topics, queues and roles were created in.
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
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// One `awsJson` RPC call against the service `signing_name` names, requiring a 200.
    async fn json(&self, signing_name: &str, target_prefix: &str, operation: &str, body: &Value) -> Value {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", &host)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Call one Step Functions operation, requiring a 200.
    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        self.json(SFN_SIGNING_NAME, SFN_TARGET_PREFIX, operation, body)
            .await
    }

    /// Call one Amazon SQS operation, requiring a 200.
    async fn sqs(&self, operation: &str, body: &Value) -> Value {
        self.json(SQS_SIGNING_NAME, SQS_TARGET_PREFIX, operation, body)
            .await
    }

    /// One awsQuery form call, the way a client of such a service makes it: its status
    /// and its XML body.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization)
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, String::from_utf8_lossy(&response.into_body()).to_string())
    }

    /// One awsQuery call that must succeed, against the service `signing_name` names.
    async fn served(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let (status, body) = self.query(signing_name, version, action, params).await;
        assert_eq!(
            status, 200,
            "case {action}: expected 200 from {signing_name}, got {status} with {body}"
        );
        body
    }

    /// One SNS call that must succeed.
    async fn sns(&self, action: &str, params: &[(&str, String)]) -> String {
        self.served(SNS_SIGNING_NAME, SNS_API_VERSION, action, params)
            .await
    }

    /// One IAM call that must succeed.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        self.served(IAM_SIGNING_NAME, IAM_API_VERSION, action, params)
            .await
    }

    /// An IAM role under `path` that Step Functions may assume, carrying `policy` as its
    /// inline policy — or no policy document at all where there is none: its ARN.
    async fn role(&self, name: &str, path: &str, policy: Option<&Value>) -> String {
        let created = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", name.to_string()),
                    ("Path", path.to_string()),
                    ("AssumeRolePolicyDocument", trust_document().to_string()),
                ],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"));
        if let Some(policy) = policy {
            self.iam(
                "PutRolePolicy",
                &[
                    ("RoleName", name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        arn
    }

    /// A topic called `name`, by its ARN.
    async fn topic(&self, name: &str) -> String {
        let created = self.sns("CreateTopic", &[("Name", name.to_string())]).await;
        xml_element(&created, "TopicArn")
            .unwrap_or_else(|| panic!("CreateTopic answers a TopicArn, got {created}"))
    }

    /// A queue subscribed to `topic_arn` with raw delivery left off: the url the
    /// delivery is observed on.
    async fn subscriber(&self, name: &str, topic_arn: &str) -> String {
        let created = self.sqs("CreateQueue", &json!({ "QueueName": name })).await;
        let url = string(&created, "QueueUrl");
        let attributes = self
            .sqs(
                "GetQueueAttributes",
                &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
            )
            .await;
        let queue_arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("no QueueArn for {name} in {attributes}"))
            .to_string();
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "AllowSNSDelivery",
                "Effect": "Allow",
                "Principal": { "Service": "sns.amazonaws.com" },
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
                "Condition": { "ArnEquals": { "aws:SourceArn": topic_arn } },
            }],
        })
        .to_string();
        self.sqs(
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
        )
        .await;
        self.sns(
            "Subscribe",
            &[
                ("TopicArn", topic_arn.to_string()),
                ("Protocol", "sqs".to_string()),
                ("Endpoint", queue_arn),
            ],
        )
        .await;
        url
    }

    /// Every message waiting on the queue at `url`, as the body each carries.
    async fn drain(&self, url: &str) -> Vec<String> {
        let received = self
            .sqs(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
            )
            .await;
        received["Messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// Create a state machine running as `role_arn`, by its ARN.
    async fn machine(&self, name: &str, definition: &str, role_arn: &str) -> String {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({ "name": name, "definition": definition, "roleArn": role_arn }),
            )
            .await;
        string(&created, "stateMachineArn")
    }

    /// Start the execution `name` with `input` and report it once it has left
    /// `RUNNING`.
    async fn ran(
        &self,
        machine_arn: &str,
        name: &str,
        input: &Value,
        case: &str,
    ) -> (String, Value) {
        let started = self
            .sfn(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine_arn,
                    "name": name,
                    "input": input.to_string(),
                }),
            )
            .await;
        let execution_arn = string(&started, "executionArn");
        let began = Instant::now();
        loop {
            let described = self
                .sfn(
                    "DescribeExecution",
                    &json!({ "executionArn": execution_arn }),
                )
                .await;
            if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
                return (execution_arn, described);
            }
            assert!(
                began.elapsed() < PATIENCE,
                "case {case:?}: expected a terminal status within {PATIENCE:?}, got {described:?}"
            );
            ::tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The events of one execution's history.
    async fn history(&self, execution_arn: &str) -> Vec<Value> {
        let answered = self
            .sfn(
                "GetExecutionHistory",
                &json!({ "executionArn": execution_arn, "maxResults": 100 }),
            )
            .await;
        answered
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

/// A one-state definition whose `Task` publishes `parameters`, with a `Catch` that keeps
/// what failed when `caught`, and no handler otherwise.
fn definition(parameters: Value, caught: bool) -> String {
    let mut state = json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::sns:publish",
        "Parameters": parameters,
        "End": true,
    });
    if caught {
        state["Catch"] = json!([{
            "ErrorEquals": ["States.ALL"],
            "Next": "Caught",
            "ResultPath": "$.errorInfo",
        }]);
    }
    let mut states = json!({ "Publish": state });
    if caught {
        states["Caught"] = json!({ "Type": "Pass", "End": true });
    }
    json!({ "StartAt": "Publish", "States": states }).to_string()
}

/// A document granting `actions` on `resource`, or saying nothing about the publish at
/// all when `actions` is empty.
fn policy(actions: &[&str], resource: &str) -> Value {
    let (actions, resource) = match actions.is_empty() {
        // The log writes a state machine makes, which withhold every integration call by
        // stating nothing about it.
        true => (vec!["logs:PutLogEvents"], "*".to_string()),
        false => (actions.to_vec(), resource.to_string()),
    };
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }],
    })
}

/// The `error` and `cause` a terminal execution reports, taken from the report the
/// failure reached: the execution's own members when nothing handled it, and the
/// `Catch` path's output when one did.
fn reported(case: &str, described: &Value) -> (String, String) {
    if described.get("status").and_then(Value::as_str) == Some("FAILED") {
        return (string(described, "error"), string(described, "cause"));
    }
    let output: Value = described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
    let caught = output
        .get("errorInfo")
        .unwrap_or_else(|| panic!("case {case:?}: expected the Catch output in {output}"));
    (
        caught
            .get("Error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        caught
            .get("Cause")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// The metadata an AWS SDK closes an exception's message with, up to the identifier of
/// the refused request — which is one call's own value, so what is asserted is that the
/// segment is there and carries one.
fn metadata(status: u16, code: &str) -> String {
    format!("(Service: AmazonSNS; Status Code: {status}; Error Code: {code}; Request ID: ")
}

/// Hold `cause` to `sentence` followed by that metadata: the id is read out of it and
/// required to be a non-empty value of this call's own.
fn assert_cause(case: &str, cause: &str, sentence: &str, status: u16, code: &str) {
    let expected = format!("{sentence} {}", metadata(status, code));
    assert!(
        cause.starts_with(&expected),
        "case {case:?}: expected the cause to open {expected:?}, got {cause:?}"
    );
    assert!(
        cause.ends_with("; Proxy: null)"),
        "case {case:?}: expected the cause to close \"; Proxy: null)\", got {cause:?}"
    );
    let request_id = cause[expected.len()..cause.len() - "; Proxy: null)".len()].to_string();
    assert!(
        !request_id.is_empty() && !request_id.contains(';'),
        "case {case:?}: expected the metadata to name the refused request, got {cause:?}"
    );
}

/// A state machine name as long as one may be: `Name` is `length 1..80`
/// (`smithy_query(query="shape",shape="Name")` on this crate's own model), which is
/// longer than a session name may be — and no part of the session the role is assumed
/// under, whose name is the assume's own.
const LONGEST_MACHINE_NAME: &str =
    "relay-named-right-up-to-the-eighty-characters-a-state-machine-name-may-carry-abc";

/// One row of the refusal table: the publish the `Task` makes, and the answer SNS gives
/// it — the error name the state reports and the sentence its cause carries.
struct Refused {
    case: &'static str,
    /// The state machine the work is for. Its name is NOT the session's: the session the
    /// role is assumed under carries a name of the assume's own, which is why a row reads
    /// the principal back out of the cause rather than building it from here.
    machine: &'static str,
    /// The IAM actions the execution role is granted, over the topic the publish
    /// addresses; `None` for a role carrying no policy document at all.
    granted: Option<&'static [&'static str]>,
    /// The attributes the publish carries, as `(name, data type, value)`.
    attributes: &'static [(&'static str, &'static str, &'static str)],
    /// Whether the topic the parameters name is the one that was created.
    real_topic: bool,
    /// Whether the state carries a `Catch`.
    caught: bool,
    /// The error name the task must report.
    error: &'static str,
    /// The wire code the cause's metadata must name, and the status it carries.
    code: &'static str,
    status: u16,
}

/// RULE 1 and 2 together: a refused `sns:publish` Task reports the exception class SNS's
/// own client resolves the answer to, with that exception's own message as its cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_sns_publish_task_reports_the_modelled_exception_and_its_message() {
    let chain = Chain::new();
    // The row below is about a name no session name can hold whole, so the constant has
    // to be one: past the 64 a `RoleSessionName` carries, and within the 80 a state
    // machine's `Name` admits.
    let longest = LONGEST_MACHINE_NAME.chars().count();
    assert!(
        (65..=80).contains(&longest),
        "the long-name row needs a name past a session name's length and within a state \
         machine's, got {longest}"
    );
    let topic = chain.topic("relay-orders").await;
    let url = chain.subscriber("relay-orders-queue", &topic).await;
    let absent = format!("arn:aws:sns:{REGION}:000000000000:no-such-topic");

    let rows: &[Refused] = &[
        Refused {
            // The role may publish, and the attribute is one SNS will not take: its
            // query code `ParameterValueInvalid` names the shape
            // `InvalidParameterValueException`
            // (`smithy_query(query="operation",operation="Publish",model_path="services/
            // service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
            // com-amazonaws-sns-amazonsimplenotificationservice.json")`), so the class
            // is that shape's and not the code's.
            case: "a Number attribute that cannot be cast",
            machine: "relay-uncastable",
            granted: Some(&["SNS:Publish"]),
            attributes: &[("count", "Number", "not-a-number")],
            real_topic: true,
            caught: false,
            error: "SNS.InvalidParameterValueException",
            code: "ParameterValueInvalid",
            status: 400,
        },
        Refused {
            // The same refusal reached through a `Catch`, which routes on that name.
            case: "the same refusal routed by a Catch",
            machine: "relay-caught",
            granted: Some(&["SNS:Publish"]),
            attributes: &[("spent", "Number", "1.2.3")],
            real_topic: true,
            caught: true,
            error: "SNS.InvalidParameterValueException",
            code: "ParameterValueInvalid",
            status: 400,
        },
        Refused {
            // A code that DOES name its own shape, for contrast: `NotFound` is
            // `NotFoundException`.
            case: "a topic the account does not hold",
            machine: "relay-absent-topic",
            granted: Some(&["SNS:Publish"]),
            attributes: &[],
            real_topic: false,
            caught: false,
            error: "SNS.NotFoundException",
            code: "NotFound",
            status: 404,
        },
        Refused {
            // And the refusal of the call itself, which `Publish` models: the role is
            // granted nothing about the publish.
            case: "a publish the execution role may not make",
            machine: "relay-denied",
            granted: Some(&[]),
            attributes: &[],
            real_topic: true,
            caught: true,
            error: "SNS.AuthorizationErrorException",
            code: "AuthorizationError",
            status: 403,
        },
        Refused {
            // The other way the documents in force withhold the publish: a role that
            // carries no policy document at all, which allows nothing.
            case: "a publish by a role carrying no policy at all",
            machine: "relay-no-policy",
            granted: None,
            attributes: &[],
            real_topic: true,
            caught: false,
            error: "SNS.AuthorizationErrorException",
            code: "AuthorizationError",
            status: 403,
        },
        Refused {
            // A state machine whose NAME is longer than a session name may be: a state
            // machine's name is `length 1..80`
            // (`smithy_query(query="shape",shape="Name")` on this crate's own model)
            // while the session the role is assumed under carries a `RoleSessionName`,
            // which is `length 2..64`
            // (`smithy_query(query="shape",shape="roleSessionNameType",
            // model_path="services/service-com-amazonaws-sts-
            // awssecuritytokenservicev20110615/smithy-model/com-amazonaws-sts-
            // awssecuritytokenservicev20110615.json")`), so whatever the state machine
            // is called, the principal the refusal blames is a session
            // `sts:AssumeRole` could really have minted.
            case: "a state machine named past the length a session name may be",
            machine: LONGEST_MACHINE_NAME,
            granted: Some(&[]),
            attributes: &[],
            real_topic: true,
            caught: false,
            error: "SNS.AuthorizationErrorException",
            code: "AuthorizationError",
            status: 403,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let addressed = match row.real_topic {
            true => topic.clone(),
            false => absent.clone(),
        };
        let role_arn = chain
            .role(
                &format!("RelayRole{index}"),
                "/service-role/",
                row.granted.map(|actions| policy(actions, &addressed)).as_ref(),
            )
            .await;
        let mut parameters = json!({ "TopicArn": addressed, "Message.$": "$.message" });
        if !row.attributes.is_empty() {
            parameters["MessageAttributes.$"] = json!("$.attributes");
        }
        let machine_name = row.machine.to_string();
        let machine = chain
            .machine(
                &machine_name,
                &definition(parameters, row.caught),
                &role_arn,
            )
            .await;
        let mut attributes = serde_json::Map::new();
        for (name, data_type, value) in row.attributes {
            attributes.insert(
                (*name).to_string(),
                json!({ "DataType": data_type, "StringValue": value }),
            );
        }
        let input = json!({ "message": "a notification", "attributes": attributes });
        let (execution_arn, described) = chain.ran(&machine, "only", &input, case).await;
        let (error, cause) = reported(case, &described);
        assert_eq!(
            error, row.error,
            "case {case:?}: expected the error {:?}, got {error:?} with cause {cause:?}",
            row.error
        );

        // The sentence the cause opens with is the invoked service's own: the publish's
        // refusal of the attribute names it, and a refusal of the CALL names the session
        // of the execution role, the action SNS publishes and the topic addressed.
        let sentence = match row.granted.is_none_or(<[&str]>::is_empty) {
            true => format!(
                "User: {} is not authorized to perform: SNS:Publish on resource: {} because no \
                 identity-based policy allows the SNS:Publish action",
                session_principal(&role_arn, &cause),
                addressed
            ),
            false => match row.attributes.first() {
                Some((name, _, _)) => {
                    format!("Could not cast message attribute '{name}' value to number.")
                }
                None => "Topic does not exist".to_string(),
            },
        };
        assert_cause(case, &cause, &sentence, row.status, row.code);

        // The same message reaches the history the failed work was reported in.
        let events = chain.history(&execution_arn).await;
        let failed: Vec<String> = events
            .iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_FAILED))
            .filter_map(|event| event.get("taskFailedEventDetails").cloned())
            .map(|details| string(&details, "cause"))
            .collect();
        assert_eq!(
            failed,
            vec![cause.clone()],
            "case {case:?}: expected exactly one {EVENT_TASK_FAILED} carrying the cause \
             {cause:?}, got {events:?}"
        );

        // A refused publish publishes nothing: the subscribed queue stays empty.
        let delivered = chain.drain(&url).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: a refused publish delivers nothing, got {delivered:?}"
        );
    }
}

/// RULE: the documents in force on the execution role are read at the moment of each
/// call, so the SAME state machine publishes while its role is granted the action and is
/// refused — under SNS's own error and sentence — once that grant is withdrawn.
///
/// The positive leg is the control: it is the same definition, the same role and the same
/// topic, so the refusal below is the grant's absence and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_publish_task_is_refused_once_its_grant_is_withdrawn() {
    let chain = Chain::new();
    let topic = chain.topic("withdrawn-orders").await;
    let url = chain.subscriber("withdrawn-orders-queue", &topic).await;
    let role_name = "WithdrawnGrantRole";
    let role_arn = chain
        .role(role_name, "/", Some(&policy(&["sns:Publish"], &topic)))
        .await;
    let machine_name = "withdrawn-grant";
    let machine = chain
        .machine(
            machine_name,
            &definition(
                json!({ "TopicArn": topic, "Message.$": "$.message" }),
                true,
            ),
            &role_arn,
        )
        .await;

    // The granted leg: the publish happens and reaches the subscribed queue.
    let input = json!({ "message": "a granted notification" });
    let (_, described) = chain.ran(&machine, "granted", &input, "granted").await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"granted\": expected the permitted publish to succeed, got {described:?}"
    );
    let delivered = chain.drain(&url).await;
    assert_eq!(
        delivered.len(),
        1,
        "case \"granted\": expected exactly one delivery, got {delivered:?}"
    );

    // The grant is withdrawn, and nothing else changes.
    chain
        .iam(
            "DeleteRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", "inline".to_string()),
            ],
        )
        .await;

    let (execution_arn, described) = chain
        .ran(&machine, "withdrawn", &input, "withdrawn")
        .await;
    // The state's `Catch` routes the refusal, so the execution itself still succeeds.
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"withdrawn\": the Catch routes the refusal, got {described:?}"
    );
    let (error, cause) = reported("withdrawn", &described);
    assert_eq!(
        error, "SNS.AuthorizationErrorException",
        "case \"withdrawn\": expected SNS's own modelled refusal, got {error:?} with cause \
         {cause:?}"
    );
    let sentence = format!(
        "User: {} is not authorized to perform: SNS:Publish on resource: {topic} because no \
         identity-based policy allows the SNS:Publish action",
        session_principal(&role_arn, &cause)
    );
    assert_cause("withdrawn", &cause, &sentence, 403, "AuthorizationError");
    let events = chain.history(&execution_arn).await;
    let failed: Vec<String> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_FAILED))
        .filter_map(|event| event.get("taskFailedEventDetails").cloned())
        .map(|details| string(&details, "cause"))
        .collect();
    assert_eq!(
        failed,
        vec![cause.clone()],
        "case \"withdrawn\": expected exactly one {EVENT_TASK_FAILED} carrying {cause:?}, got \
         {events:?}"
    );
    let delivered = chain.drain(&url).await;
    assert!(
        delivered.is_empty(),
        "case \"withdrawn\": a refused publish delivers nothing, got {delivered:?}"
    );
}
