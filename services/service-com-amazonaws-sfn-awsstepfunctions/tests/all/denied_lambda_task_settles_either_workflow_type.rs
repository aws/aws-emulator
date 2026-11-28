// SPDX-License-Identifier: CC0-1.0

//! That a `Task` state's Lambda invocation the state machine's EXECUTION ROLE may not make
//! SETTLES the run — and is reported to a client that only reads — whichever workflow type
//! ran it, driven over both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions and IAM mounted on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in, so the documents the call is judged against are
//! the ones IAM's own handlers wrote.
//!
//! RULE: a `Task` state's integration call is made as the state machine's `roleArn`
//! (`CreateStateMachine$roleArn`) and is decided against the documents in force on that role
//! BEFORE the request goes anywhere, so a call no document in force allows is not made and
//! the state FAILS at once — never waits for an answer nothing will send. The run therefore
//! reaches a TERMINAL status carrying that failure's `error` and `cause`, reported by
//! whichever API serves the workflow type that ran it: `DescribeExecution`'s `status`,
//! `error` and `cause` for a `STANDARD` execution, and `StartSyncExecution`'s own answer for
//! an EXPRESS one ("StartSyncExecution is not available for STANDARD workflows"). Because
//! those reads only REPORT, every later read of a settled run answers the same three values.
//!
//! The rows differ along what the rule generalises over: the form the `Task` names the
//! invocation in (the function's own ARN, the optimized `lambda:invoke` integration and the
//! AWS SDK `aws-sdk:lambda:invoke` one), what the documents in force say about it (nothing at
//! all, a statement about another action only, and an explicit `Deny` beside a grant of the
//! very action — which is also what the `cause`'s closing clause names), and the workflow
//! type the same definition is run as.
//!
//! What the neighbours pin and this one does not repeat: `denied_lambda_task_integration_patterns`
//! holds the decision over the integration PATTERN a `Resource` asks the call in, and
//! `task_lifecycle_under_execution_role` the events an attempt records. What only this suite
//! reaches is that the refusal TERMINATES the run under either workflow type, so a client
//! waiting on it is answered rather than left polling a run that never settles.
//!
//! The harness (`Chain`), the refusal metadata and the request-id masking are
//! `tests/all/task_execution_role.rs`'s and the session reader is
//! `tests/all/task_refusal_session_principal.rs`'s; `tests/all/main.rs` is this binary's crate
//! root, so each is reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::execution_data_encryption::xml_element;
use crate::state_machine_lifecycle::{string, ACCOUNT, REGION};
use crate::task_execution_role::{
    masked, refusal_metadata, trust_document, Chain, ACCESS_DENIED_CODE,
};
use crate::task_refusal_session_principal::session_principal;

/// The IAM action an invocation is subject to: the permission Lambda PUBLISHES for `Invoke`,
/// which is not the operation's own name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// An action a least-privilege orchestration role carries that says nothing about any
/// invocation, which is what one row's document speaks about instead.
const UNRELATED_ACTION: &str = "logs:PutLogEvents";

/// The error a `Task` state reports for an invocation the documents withheld: Lambda's base
/// SDK exception, which is what a refusal `Invoke` models no error of its own arrives under
/// (`smithy_query(query="operation",operation="Invoke",model_path="services/
/// service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
/// com-amazonaws-lambda-awsgirapiservice.json")`). Both families name the service `Lambda`.
const REFUSED_ERROR: &str = "Lambda.AWSLambdaException";

/// The name the client each form is served by gives Lambda in the metadata of a refusal:
/// the earlier generation of client serves the OPTIMIZED integration token and names the
/// service as that client is named, while the function's own ARN and the AWS SDK family are
/// both served by the current SDK, which names it as that SDK's own client does.
const REFUSAL_SERVICE: &str = "AWSLambda";
/// See [`REFUSAL_SERVICE`].
const SDK_REFUSAL_SERVICE: &str = "Lambda";

/// The status Lambda answers an unauthorized call with.
const FORBIDDEN: u16 = 403;

/// The terminal status a refused run reaches, and the non-terminal one it leaves
/// (`smithy_query(query="shape",shape="ExecutionStatus")`: `RUNNING`, `SUCCEEDED`, `FAILED`,
/// `TIMED_OUT`, `ABORTED`, `PENDING_REDRIVE`).
const FAILED: &str = "FAILED";

/// The workflow type `StartSyncExecution` serves, as `StateMachineType` spells it.
const EXPRESS: &str = "EXPRESS";

/// Which form a `Task` state's `Resource` names the invocation in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// The function's own ARN, which is the same call the optimized integration makes.
    FunctionArn,
    /// The optimized integration token, whose `Parameters` name the function.
    Optimized,
    /// The AWS SDK integration for the same Lambda operation.
    AwsSdk,
}

impl Named {
    /// The `Resource` this form writes for an invocation of `function_arn`.
    fn resource(self, function_arn: &str) -> String {
        match self {
            Self::FunctionArn => function_arn.to_string(),
            Self::Optimized => "arn:aws:states:::lambda:invoke".to_string(),
            Self::AwsSdk => "arn:aws:states:::aws-sdk:lambda:invoke".to_string(),
        }
    }

    /// The definition of ONE `Task` state invoking `function_arn` in this form, with no
    /// `Retry` and no `Catch`, so the task's outcome is the run's.
    fn definition(self, function_arn: &str) -> String {
        let state = match self {
            Self::FunctionArn => json!({
                "Type": "Task",
                "Resource": function_arn,
                "End": true,
            }),
            Self::Optimized | Self::AwsSdk => json!({
                "Type": "Task",
                "Resource": self.resource(function_arn),
                "Parameters": { "FunctionName": function_arn, "Payload.$": "$" },
                "End": true,
            }),
        };
        json!({ "StartAt": "Invoke", "States": { "Invoke": state } }).to_string()
    }
}

/// What the documents in force on the execution role say about the invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Documents {
    /// None at all: a grant has to come from a document, so such a role allows nothing.
    NoneAtAll,
    /// One statement, about another action entirely.
    AnotherAction,
    /// A grant of the very action, with an explicit `Deny` of it beside the grant.
    DeniedBesideAGrant,
}

impl Documents {
    /// The inline document this shape puts on the role, and `None` where it puts none.
    fn policy(self) -> Option<Value> {
        match self {
            Self::NoneAtAll => None,
            Self::AnotherAction => Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": UNRELATED_ACTION,
                    "Resource": "*",
                }],
            })),
            Self::DeniedBesideAGrant => Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": INVOKE_ACTION, "Resource": "*" },
                    { "Effect": "Deny", "Action": INVOKE_ACTION, "Resource": "*" },
                ],
            })),
        }
    }

    /// The clause IAM's own sentence ends with for this shape: the document that withheld
    /// the call where nothing allowed it, and the explicit `Deny` where one refused it.
    fn because(self) -> String {
        match self {
            Self::NoneAtAll | Self::AnotherAction => {
                format!("because no identity-based policy allows the {INVOKE_ACTION} action")
            }
            Self::DeniedBesideAGrant => {
                "with an explicit deny in an identity-based policy".to_string()
            }
        }
    }

    /// The role name this shape's documents are held under.
    fn role_name(self) -> &'static str {
        match self {
            Self::NoneAtAll => "DeniedInvokeNoDocumentRole",
            Self::AnotherAction => "DeniedInvokeOtherActionRole",
            Self::DeniedBesideAGrant => "DeniedInvokeExplicitDenyRole",
        }
    }
}

/// One row: the case it is named by, the form the `Task` names the invocation in, and what
/// the documents in force say about it.
struct Row {
    case: String,
    named: Named,
    documents: Documents,
}

/// Every form of the invocation against every shape of document, which is the whole table
/// both workflow types are run over.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (form_case, named) in [
        ("the function's own ARN", Named::FunctionArn),
        ("the optimized lambda:invoke integration", Named::Optimized),
        ("the AWS SDK lambda invocation", Named::AwsSdk),
    ] {
        for (document_case, documents) in [
            ("a role carrying no identity-based document", Documents::NoneAtAll),
            ("a role granting another action only", Documents::AnotherAction),
            ("a role whose grant is denied explicitly", Documents::DeniedBesideAGrant),
        ] {
            rows.push(Row {
                case: format!("{form_case} under {document_case}"),
                named,
                documents,
            });
        }
    }
    rows
}

/// The ARN of the function every row's `Task` names. Nothing is created for it: the call is
/// decided before it is made, so the function it would have reached need not exist and
/// Lambda need not be mounted at all — which is itself part of the rule.
fn function_arn() -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:denied-invoke-target")
}

/// The three roles the rows run as, each carrying its shape's documents, by their own ARNs.
async fn roles(chain: &Chain) -> Vec<(Documents, String)> {
    let mut held = Vec::new();
    for documents in [
        Documents::NoneAtAll,
        Documents::AnotherAction,
        Documents::DeniedBesideAGrant,
    ] {
        let arn = match documents.policy() {
            Some(policy) => chain.role(documents.role_name(), &policy).await,
            None => undocumented_role(chain, documents.role_name()).await,
        };
        held.push((documents, arn));
    }
    held
}

/// A role this account holds that carries NO identity-based document, by the ARN IAM minted
/// for it: `Chain::role` always puts one, and a role with none is a row of its own.
async fn undocumented_role(chain: &Chain, name: &str) -> String {
    let created = chain
        .iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust_document().to_string()),
            ],
        )
        .await;
    xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"))
}

/// The whole sentence a refused invocation's `cause` carries: IAM's own not-authorized
/// sentence over the session the call was attempted as, the action the invoked operation
/// publishes and the ARN of the function the request addressed, closed by the metadata the
/// client that form is served through renders.
fn expected_cause(session: &str, named: Named, function: &str, because: &str) -> String {
    format!(
        "User: {session} is not authorized to perform: {INVOKE_ACTION} on resource: \
         {function} {because}{}",
        refusal_metadata(
            &named.resource(function),
            match named {
                Named::AwsSdk | Named::FunctionArn => SDK_REFUSAL_SERVICE,
                Named::Optimized => REFUSAL_SERVICE,
            },
            ACCESS_DENIED_CODE,
            FORBIDDEN,
        )
    )
}

/// The `status` and `error` of one answer beside its `cause` with the refused request's id
/// masked, which is the triple every row is held to.
fn reported(answer: &Value) -> (Option<&str>, Option<&str>, String) {
    (
        answer.get("status").and_then(Value::as_str),
        answer.get("error").and_then(Value::as_str),
        masked(answer.get("cause").and_then(Value::as_str).unwrap_or_default()),
    )
}

/// A `STANDARD` execution whose `Task` may not make its invocation SETTLES `FAILED`, and
/// `DescribeExecution` reports the refusal — the same three values however often it is read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_invocation_settles_a_standard_execution_and_is_reported_by_describe() {
    let chain = Chain::new();
    let function = function_arn();
    let held = roles(&chain).await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = &row.case;
        let role_arn = &held
            .iter()
            .find(|(documents, _)| *documents == row.documents)
            .expect("every row's documents are held")
            .1;
        let machine = chain
            .machine(
                &format!("standard-denied-invoke-{index}"),
                &row.named.definition(&function),
                role_arn,
            )
            .await;

        let (execution_arn, described) = chain.ran(&machine, "only", case).await;
        let (status, error, cause) = reported(&described);
        let session = session_principal(role_arn, &cause);
        let expected = expected_cause(&session, row.named, &function, &row.documents.because());
        assert_eq!(
            (status, error, cause.as_str()),
            (Some(FAILED), Some(REFUSED_ERROR), expected.as_str()),
            "case {case:?}: expected {FAILED}/{REFUSED_ERROR} with {expected:?}, got \
             {described:?}"
        );

        // The read only REPORTS: a client that looks again at the settled run is answered
        // the same status, error and cause, which is what tells "denied" from "running".
        let again = chain
            .sfn(
                "DescribeExecution",
                &json!({ "executionArn": execution_arn }),
            )
            .await;
        let (status, error, cause) = reported(&again);
        assert_eq!(
            (status, error, cause.as_str()),
            (Some(FAILED), Some(REFUSED_ERROR), expected.as_str()),
            "case {case:?}: a second read must report the settled run unchanged, got {again:?}"
        );
    }
}

/// The SYNCHRONOUS run of an EXPRESS workflow whose `Task` may not make its invocation is
/// answered the same refusal inside the one call: the terminal status, the error and the
/// cause, so the caller is never left waiting on a run that cannot happen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_invocation_is_answered_by_a_synchronous_express_run() {
    let chain = Chain::new();
    let function = function_arn();
    let held = roles(&chain).await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = &row.case;
        let role_arn = &held
            .iter()
            .find(|(documents, _)| *documents == row.documents)
            .expect("every row's documents are held")
            .1;
        let created = chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("express-denied-invoke-{index}"),
                    "definition": row.named.definition(&function),
                    "roleArn": role_arn,
                    "type": EXPRESS,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");

        let answered = chain
            .sfn(
                "StartSyncExecution",
                &json!({ "stateMachineArn": machine, "name": "only" }),
            )
            .await;
        let (status, error, cause) = reported(&answered);
        let session = session_principal(role_arn, &cause);
        let expected = expected_cause(&session, row.named, &function, &row.documents.because());
        assert_eq!(
            (status, error, cause.as_str()),
            (Some(FAILED), Some(REFUSED_ERROR), expected.as_str()),
            "case {case:?}: expected the synchronous answer to carry {FAILED}/{REFUSED_ERROR} \
             with {expected:?}, got {answered:?}"
        );
    }
}
