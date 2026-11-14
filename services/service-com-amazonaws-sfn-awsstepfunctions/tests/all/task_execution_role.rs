// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EXECUTION ROLE a `Task` state's integration call is made
//! as, driven over both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions and IAM mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so the policies the call is judged against
//! are the documents IAM's own handlers wrote.
//!
//! The rule under test: a `Task` state's integration call is made as the state
//! machine's `roleArn`, so the identity-based policies in force on that role decide
//! whether it is made at all. A call they do not allow is NOT made: the task fails with
//! the invoked service's own access-denied refusal — reported under that service's base
//! SDK exception, with the IAM sentence naming the principal, the action and the
//! resource, and the error code the answer carried — the execution ends `FAILED`, and
//! the resource the call would have acted on is untouched. A call they do allow is made
//! exactly as before, which is the positive control in every row.
//!
//! The rows differ along what the rule generalises over: what the policies say (a grant
//! for one resource, a grant for another, an explicit `Deny` over a grant, a wildcard
//! grant, and no statement about the action at all), the integration PATTERN the
//! `Resource` asks for (the fire-and-forget `states:startExecution` and the run-a-job
//! `states:startExecution.sync:2`), and the TARGET the request names.
//!
//! The run-a-job pattern makes two further calls of its own — it REPORTS the job it
//! started and STOPS it — as the same role, so the role here grants those two actions
//! and every row varies only in what the policies say about STARTING the child.
//!
//! A wildcard grant lets the call be made, which the last test pins. Whether the role
//! can be ASSUMED at all is the question before this one, and
//! `tests/all/execution_role_assumption.rs` holds it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::state::{EVENT_TASK_FAILED, EVENT_TASK_SUBMITTED};
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::execution_data_encryption::{form_encode, xml_element};
use crate::state_machine_lifecycle::{string, REGION};

/// How Step Functions' own requests are addressed on the wire, from its model's
/// `service_info` (service shape `AWSStepFunctions`, `sigv4Name` `states`, `awsJson1_0`).
const TARGET_PREFIX: &str = "AWSStepFunctions";

/// The access-key id every request below is signed with: the account's own published
/// credential, which no identity policy governs.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// How long a test waits for an execution to leave `RUNNING`.
///
/// A parked task is resolved by polling the job, so a parent that really started one
/// reaches its own outcome a few poll intervals after the child does.
const PATIENCE: Duration = Duration::from_secs(20);

/// The error a `Task` state reports when Step Functions refuses the call it made: this
/// service's base SDK exception, which is what an error its operation does not model is
/// reported under.
const REFUSAL_ERROR: &str = "StepFunctions.AWSStepFunctionsException";

/// The fragments the cause of that refusal carries: the code the answer came back
/// under, and the resource the call was refused ON.
pub(crate) const REFUSAL_CODE: &str = "Error Code: AccessDeniedException";

/// A child definition that finishes at once.
const CHILD: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// Step Functions and IAM over ONE engine, so a state machine's execution role is the
/// role IAM's own handlers hold.
///
/// `pub(crate)` because `tests/all/task_target_authorization.rs` holds the same rule over
/// the other integrations and drives it through this same pair of services: `tests/all.rs`
/// is one crate root, so the wiring, the two wire protocols and the trust policy are
/// stated here once.
pub(crate) struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call one Step Functions operation, requiring a 200.
    pub(crate) async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/states/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("states.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// One awsQuery call to IAM, signed with the same credential.
    pub(crate) async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
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
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {action}: expected 200 from IAM, got {status} with {text}"
        );
        text
    }

    /// An IAM role carrying `policy` as its inline policy, by its own ARN.
    pub(crate) async fn role(&self, name: &str, policy: &Value) -> String {
        let created = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", name.to_string()),
                    ("AssumeRolePolicyDocument", trust_document().to_string()),
                ],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"));
        self.iam(
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
        arn
    }

    /// Create a state machine under `role_arn` and return its ARN.
    pub(crate) async fn machine(&self, name: &str, definition: &str, role_arn: &str) -> String {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({ "name": name, "definition": definition, "roleArn": role_arn }),
            )
            .await;
        string(&created, "stateMachineArn")
    }

    /// Start one execution and report it once it has left `RUNNING`.
    pub(crate) async fn ran(&self, machine_arn: &str, name: &str, case: &str) -> (String, Value) {
        let started = self
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine_arn, "name": name }),
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
    pub(crate) async fn history(&self, execution_arn: &str) -> Vec<Value> {
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

    /// How many executions `ListExecutions` reports for `machine_arn`.
    async fn started_count(&self, machine_arn: &str) -> usize {
        let listed = self
            .sfn(
                "ListExecutions",
                &json!({ "stateMachineArn": machine_arn, "maxResults": 10 }),
            )
            .await;
        listed
            .get("executions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }
}

/// The trust document a Step Functions execution role carries, which is what lets this
/// service act as it.
pub(crate) fn trust_document() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "states.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

// An awsQuery parameter value, percent-encoded as that protocol takes it, and the text
// of one element of an awsQuery response, are `crate::execution_data_encryption`'s:
// `tests/all.rs` is this binary's crate root, so this suite drives IAM through those
// readers rather than carrying a second copy of them.

/// A definition whose one `Task` state starts `child_arn` through `resource`.
///
/// The parameters are the state machine ARN alone, under the integration's own
/// PascalCase: both families reach the same member of the same operation with it
/// (the optimized one re-spells it, the AWS SDK one is handed the model's own
/// spelling), so one definition serves every row.
fn parent_definition(child_arn: &str, resource: &str) -> String {
    json!({
        "StartAt": "CallChild",
        "States": {
            "CallChild": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": { "StateMachineArn": child_arn },
                "End": true,
            }
        }
    })
    .to_string()
}

/// A statement over one child state machine.
fn statement(effect: &str, resource: &str) -> Value {
    json!({
        "Effect": effect,
        "Action": "states:StartExecution",
        "Resource": resource
    })
}

/// The statement the run-a-job (`.sync`) pattern's OWN calls need: the pattern reports
/// the child execution it started and stops it, both as the execution role, so a role
/// that runs the pattern grants those two actions over the executions it starts.
fn poll_statement() -> Value {
    json!({
        "Effect": "Allow",
        "Action": ["states:DescribeExecution", "states:StopExecution"],
        "Resource": "*"
    })
}

/// Which child a row targets, the `Resource` its `Task` names, and what the role's
/// policy says about starting it.
type AuthorizationRow = (&'static str, usize, &'static str, bool);

/// The three forms of the same call a row may name: the optimized integration, its
/// run-a-job pattern, and the AWS SDK integration of the same operation — all three
/// start the state machine their request names, so all three are the same access
/// request.
const FIRE_AND_FORGET: &str = "arn:aws:states:::states:startExecution";
const RUN_A_JOB: &str = "arn:aws:states:::states:startExecution.sync:2";
const AWS_SDK: &str = "arn:aws:states:::aws-sdk:sfn:startExecution";

/// A `Task` state's call is made only when the execution role's policies allow it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tasks_call_is_made_only_when_the_execution_role_allows_it() {
    let chain = Chain::new();
    // Three children: the one the role is granted, one it is not, and one it is
    // explicitly denied over a grant that would otherwise cover it.
    let mut children = Vec::new();
    for index in 0..3 {
        children.push(
            chain
                .machine(
                    &format!("role-child-{index}"),
                    CHILD,
                    "arn:aws:iam::000000000000:role/unused",
                )
                .await,
        );
    }
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [
            statement("Allow", &children[0]),
            statement("Allow", &children[2]),
            statement("Deny", &children[2]),
            // The run-a-job rows poll the child execution their task started, which
            // is a call of the execution's own: the pattern reports the job through
            // `DescribeExecution` and stops it through `StopExecution`, so a role that
            // may run the pattern at all carries both. What each row below varies is
            // the grant on starting the child, which is the rule under test.
            poll_statement(),
        ]
    });
    let role_arn = chain.role("StepFunctionsRole", &policy).await;

    // (case, which child, the Resource the task names, whether the role may start it)
    let rows: [AuthorizationRow; 8] = [
        (
            "the permitted child, fire and forget",
            0,
            FIRE_AND_FORGET,
            true,
        ),
        (
            "the permitted child through the run-a-job pattern",
            0,
            RUN_A_JOB,
            true,
        ),
        (
            "the permitted child through the AWS SDK integration",
            0,
            AWS_SDK,
            true,
        ),
        (
            "a child no statement is about, fire and forget",
            1,
            FIRE_AND_FORGET,
            false,
        ),
        (
            "the same child through the run-a-job pattern",
            1,
            RUN_A_JOB,
            false,
        ),
        (
            // The other family makes the same call on the same resource, so the deny
            // must hold through it too.
            "the same child through the AWS SDK integration",
            1,
            AWS_SDK,
            false,
        ),
        (
            "a child an explicit deny covers, over a grant that would allow it",
            2,
            FIRE_AND_FORGET,
            false,
        ),
        (
            "the same denied child through the run-a-job pattern",
            2,
            RUN_A_JOB,
            false,
        ),
    ];

    for (index, (case, child, resource, allowed)) in rows.into_iter().enumerate() {
        let child_arn = children[child].clone();
        let machine_name = format!("role-parent-{index}");
        let parent = chain
            .machine(
                &machine_name,
                &parent_definition(&child_arn, resource),
                &role_arn,
            )
            .await;
        let before = chain.started_count(&child_arn).await;
        let (execution_arn, described) = chain.ran(&parent, "only", case).await;
        let events = chain.history(&execution_arn).await;
        let types: Vec<&str> = events
            .iter()
            .filter_map(|event| event.get("type").and_then(Value::as_str))
            .collect();

        if allowed {
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected the call the role allows to be made, got {described:?}"
            );
            assert_eq!(
                chain.started_count(&child_arn).await,
                before + 1,
                "case {case:?}: expected the child execution the task started"
            );
            continue;
        }

        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(REFUSAL_ERROR),
            "case {case:?}: expected {REFUSAL_ERROR}, got {described:?}"
        );
        // Exactly one TaskFailed, carrying the refusal; and nothing claiming the job
        // was submitted, because the call was never made.
        let failed: Vec<&Value> = events
            .iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_FAILED))
            .collect();
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one {EVENT_TASK_FAILED}, got {types:?}"
        );
        let details = failed[0]
            .get("taskFailedEventDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected taskFailedEventDetails"));
        assert_eq!(
            details.get("error").and_then(Value::as_str),
            Some(REFUSAL_ERROR),
            "case {case:?}: expected the task's own error to name the refusal, got {details:?}"
        );
        let cause = string(details, "cause");
        assert!(
            cause.contains(REFUSAL_CODE),
            "case {case:?}: expected the cause to carry {REFUSAL_CODE:?}, got {cause:?}"
        );
        // The resource it was refused ON is the one the request named, taken from the
        // state's own parameters.
        assert!(
            cause.contains(&format!("on resource: {child_arn} ")),
            "case {case:?}: expected the cause to name the resource {child_arn}, got {cause:?}"
        );
        assert!(
            cause.contains("states:StartExecution"),
            "case {case:?}: expected the cause to name the action, got {cause:?}"
        );
        // The principal is the SESSION of the role the call was made as, under the name
        // the assume gave it — not the role's own IAM ARN.
        let principal =
            crate::task_refusal_session_principal::session_principal(&role_arn, &cause);
        assert!(
            cause.contains(&principal),
            "case {case:?}: expected the cause to name the principal {principal}, got {cause:?}"
        );
        assert!(
            !types.contains(&EVENT_TASK_SUBMITTED),
            "case {case:?}: nothing was submitted, so no {EVENT_TASK_SUBMITTED} may appear, got \
             {types:?}"
        );
        // And the child the role may not start did NOT run.
        assert_eq!(
            chain.started_count(&child_arn).await,
            before,
            "case {case:?}: expected no execution of the child the role may not start"
        );
    }
}

/// A wildcard grant allows the call, so it is made.
///
/// The row is the way a call goes ahead for a reason other than a statement naming its
/// exact resource: a grant written with IAM's own wildcards.
#[tokio::test]
async fn a_wildcard_grant_lets_the_call_be_made() {
    let chain = Chain::new();
    let child_arn = chain
        .machine(
            "wildcard-child",
            CHILD,
            "arn:aws:iam::000000000000:role/unused",
        )
        .await;
    let wildcard = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "states:*", "Resource": "*" }]
    });
    let wildcard_role = chain.role("WildcardRole", &wildcard).await;
    let rows: [(&str, String); 1] = [("a wildcard grant", wildcard_role)];

    for (index, (case, role_arn)) in rows.into_iter().enumerate() {
        let parent = chain
            .machine(
                &format!("wildcard-parent-{index}"),
                &parent_definition(&child_arn, FIRE_AND_FORGET),
                &role_arn,
            )
            .await;
        let before = chain.started_count(&child_arn).await;
        let (_execution_arn, described) = chain.ran(&parent, "only", case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the call to be made, got {described:?}"
        );
        assert_eq!(
            chain.started_count(&child_arn).await,
            before + 1,
            "case {case:?}: expected the child execution the task started"
        );
    }
}
