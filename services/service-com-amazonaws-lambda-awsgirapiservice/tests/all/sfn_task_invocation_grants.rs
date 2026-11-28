// SPDX-License-Identifier: CC0-1.0

//! WHICH documents let a Step Functions `Task` state invoke a FUNCTION, driven over each
//! service's own wire protocol through the production `MultiServiceRouter` with Lambda,
//! IAM and Step Functions mounted on ONE engine — the shape `core/router/src/lib.rs`
//! mounts them in — so the documents the invocation is judged against are the ones IAM's
//! and this service's own handlers wrote, and an invocation that is made really runs the
//! handler.
//!
//! RULE: a call an IAM principal makes on a resource of its own account is allowed when
//! EITHER kind of document allows it — "if an identity-based policy or a resource-based
//! policy grants the request, then AWS allows the request" — so the two are evaluated
//! together: a grant in either is enough, an explicit `Deny` in either refuses the call
//! whatever the other says, and a statement naming the ACCOUNT delegates to that
//! account's own identity-based policies rather than granting. Step Functions invokes a
//! `Task` state's function ACTING AS the state machine's execution role, so the function's
//! own policy — the statements `AddPermission` and `PutResourcePolicy` wrote — decides
//! that invocation beside the policies in force on the role: a role carrying no
//! identity-based grant at all invokes a function whose policy names it, and the task
//! reports what the run answered.
//!
//! The rows differ along what the rule generalises over: WHICH document carries the grant
//! (the role's own inline policy, the function's resource-based policy, both, neither),
//! WHOM the function's policy names (the role itself, another role, the account alone),
//! which document carries an explicit `Deny` (the role's, the function's), and the form
//! the `Task` names its work in (the function's own ARN and the optimized
//! `arn:aws:states:::lambda:invoke` integration, which are the same invocation of the same
//! function).
//!
//! The IAM caller, its form encoding and the reader of one element of its XML answer are
//! `tests/all/event_source_mapping_role_access.rs`'s: `tests/all/main.rs` is one crate
//! root, so this suite borrows them rather than carrying a second copy.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{element, iam};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The account segment every ARN this emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Where `CreateFunction` is bound, and where the resource-policy writer is.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";
/// See [`FUNCTIONS_URI`].
const RESOURCE_POLICY_URI: &str = "/2026-07-09/resource-policy";

/// The `State` a function reports once it can be invoked (`FunctionConfiguration$State`:
/// "Active" once "the function is ready to be invoked").
const FUNCTION_ACTIVE: &str = "Active";

/// The runtime the handler below is written for, the module the package carries it in and
/// the handler its configuration names.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const MODULE: &str = "index.py";
/// See [`RUNTIME`].
const HANDLER: &str = "index.handler";

/// The role the FUNCTION runs as, which is not the state machine's.
const FUNCTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The GLOBAL service principal Step Functions assumes a state machine's execution role
/// as, which a trust policy has to name for the role to be assumable at all.
const STATES_PRINCIPAL: &str = "states.amazonaws.com";

/// The IAM action an invocation is subject to: the one Lambda PUBLISHES for `Invoke`
/// rather than the operation's own name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// An action that has nothing to do with an invocation, which is what a role whose own
/// documents say nothing about the call grants instead.
const UNRELATED_ACTION: &str = "logs:PutLogEvents";

/// The error a `Task` state reports when the invocation was refused: Lambda's base SDK
/// exception, which is what a refusal `Invoke` models no error of its own arrives under.
const REFUSED_ERROR: &str = "Lambda.AWSLambdaException";

/// The field the metadata of that refusal OPENS with, as the client that made the call
/// writes it: the optimized integration is served by the earlier generation of client,
/// whose own name the service carries and whose fields are separated by semicolons, while
/// the LEGACY form — the function's own ARN as the `Resource` — is made with the current AWS
/// SDK, which names the service as that SDK's own client does and separates its fields with
/// commas.
fn refused_service(form: Form) -> &'static str {
    match form {
        Form::FunctionArn => "Service: Lambda,",
        Form::Optimized => "Service: AWSLambda;",
    }
}

/// The status that answer came back with: an authorization refusal of `Invoke` is the
/// service-common `AccessDeniedException`, served as a Forbidden.
const REFUSED_STATUS: &str = "Status Code: 403";

/// The handler every row's function carries: it reports what it was handed, so a task
/// that really invoked it reports the RUN's own answer.
const HANDLER_SOURCE: &str = r#"
def handler(event, context):
    return {"order": event.get("order"), "handled": True}
"#;

/// The `order` every execution's input carries, which the handler echoes.
const ORDER: &str = "o-1";

/// How long a row waits for a condition the emulator's own CLOCK owes — a created
/// function reaching [`FUNCTION_ACTIVE`], an execution leaving `RUNNING`. Far above the
/// span the services take, so a loaded host waits longer rather than failing.
const PATIENCE: Duration = Duration::from_secs(60);

/// How long one look of such a poll waits before looking again.
const LOOK: Duration = Duration::from_millis(25);

/// Lambda, IAM and Step Functions over ONE shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Lambda `restJson1` call, as its status and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&answered).to_string()))
    };
    (parts.status.as_u16(), parsed)
}

/// One Step Functions call (`awsJson1_0`), requiring a 200.
async fn sfn(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("states.{REGION}.amazonaws.com"))
        .header("authorization", authorization("states"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from Step Functions {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The text a member of a response carries.
fn string(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a {name} in {body}"))
        .to_string()
}

/// A .zip deployment package holding [`HANDLER_SOURCE`] as [`MODULE`], written in memory
/// the way a caller uploads one.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer.start_file(MODULE, options).expect("the entry starts");
        writer
            .write_all(HANDLER_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the function `name` and wait out its provisioning window; answers its ARN.
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": FUNCTION_ROLE_ARN,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateFunction, got {status} with {created}"
    );
    let arn = string(&created, "FunctionArn");
    // The activation is owed by the clock, so this LOOKS until it holds rather than
    // sleeping a span of its own.
    let began = Instant::now();
    loop {
        let (status, configuration) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetFunctionConfiguration, got {status} with \
             {configuration}"
        );
        if configuration.get("State").and_then(Value::as_str) == Some(FUNCTION_ACTIVE) {
            return arn;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected the function to reach {FUNCTION_ACTIVE} within \
             {PATIENCE:?}, last saw {configuration}"
        );
        tokio::time::sleep(LOOK).await;
    }
}

/// A role Step Functions may assume, carrying `statements` as its one inline policy;
/// answers the ARN IAM's own `CreateRole` reported for it.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    statements: &Value,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": STATES_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateRole, got {status} with {created}"
    );
    let document = json!({ "Version": "2012-10-17", "Statement": statements }).to_string();
    let (status, written) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "task-work"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from PutRolePolicy, got {status} with {written}"
    );
    element(&created, "Arn")
}

/// Grant `principal` the invocation of the function `name` through `AddPermission`, the
/// call a client makes to write one statement of a function's own policy.
async fn given_permission(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    principal: &str,
) {
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/policy"),
        &json!({
            "StatementId": "task-invocation",
            "Action": INVOKE_ACTION,
            "Principal": principal,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from AddPermission, got {status} with {granted}"
    );
}

/// Store `document` as the whole resource-based policy of the function `arn` names, which
/// is how a policy carrying a `Deny` comes to be in force on one.
async fn given_resource_policy(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    document: &Value,
) {
    let (status, written) = lambda(
        router,
        "PUT",
        &format!("{RESOURCE_POLICY_URI}/{arn}"),
        &json!({ "Policy": document.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from PutResourcePolicy, got {status} with {written}"
    );
}

/// Create `definition` as a state machine under `role_arn`, start one execution of it and
/// report that execution once it has left `RUNNING`.
async fn ran(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    definition: &Value,
    role_arn: &str,
) -> Value {
    let created = sfn(
        router,
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
    let started = sfn(
        router,
        case,
        "StartExecution",
        &json!({
            "stateMachineArn": machine,
            "name": "only",
            "input": json!({ "order": ORDER }).to_string(),
        }),
    )
    .await;
    let execution = string(&started, "executionArn");
    let began = Instant::now();
    loop {
        let described = sfn(
            router,
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
            "case {case:?}: expected a terminal status within {PATIENCE:?}, last saw {described}"
        );
        tokio::time::sleep(LOOK).await;
    }
}

/// The two forms a `Task` state names one function's invocation in: the function's own
/// ARN, and the optimized integration whose `FunctionName` parameter addresses it.
#[derive(Debug, Clone, Copy)]
enum Form {
    /// `"Resource": "<the function's ARN>"`.
    FunctionArn,
    /// `"Resource": "arn:aws:states:::lambda:invoke"` with a `FunctionName` parameter.
    Optimized,
}

/// A one-`Task` definition naming the invocation of `function_arn` in `form`.
fn definition(form: Form, function_arn: &str) -> Value {
    let mut state = match form {
        Form::FunctionArn => json!({ "Type": "Task", "Resource": function_arn, "End": true }),
        Form::Optimized => json!({
            "Type": "Task",
            "Resource": "arn:aws:states:::lambda:invoke",
            "Parameters": {
                "FunctionName": function_arn,
                "Payload": { "order": ORDER },
            },
            "End": true,
        }),
    };
    state["ResultPath"] = json!("$.result");
    json!({ "StartAt": "Invoke", "States": { "Invoke": state } })
}

/// What a row's documents say about the invocation.
#[derive(Debug, Clone, Copy)]
enum Identity {
    /// The role's own policy grants the invocation of the row's function.
    Grants,
    /// It speaks only about this service's log writes, which says nothing about the
    /// invocation.
    Silent,
    /// It explicitly DENIES the invocation.
    Denies,
}

/// What the FUNCTION's own resource-based policy says about it.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The function carries no policy at all.
    Nothing,
    /// It grants the invocation to the row's own execution role.
    GrantsTheRole,
    /// It grants it to ANOTHER role of the same account.
    GrantsAnotherRole,
    /// It grants it to the ACCOUNT, which delegates to that account's own identity-based
    /// policies rather than granting.
    GrantsTheAccount,
    /// It explicitly DENIES the invocation to the row's role.
    DeniesTheRole,
}

/// What the documents decide, and so what the execution reports.
#[derive(Debug, Clone, Copy)]
enum Decided {
    /// The invocation was made: the execution reports what the run answered.
    Made,
    /// It was not made: the execution fails with the clause the refusal's sentence ends
    /// with.
    Refused(&'static str),
}

/// One row: what each kind of document says, the form the `Task` names the work in, and
/// what that decides.
struct Row {
    case: &'static str,
    identity: Identity,
    target: Target,
    form: Form,
    decided: Decided,
}

/// The clause IAM's sentence ends with where no document allows the call.
const NO_IDENTITY_GRANT: &str =
    "because no identity-based policy allows the lambda:InvokeFunction action";

/// The clause it ends with where an identity-based policy denies it.
const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";

/// The clause it ends with where the resource's own policy denies it.
const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";

/// RULE: either kind of document in force grants a `Task` state's invocation, and an
/// explicit `Deny` in either refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn either_document_in_force_decides_a_task_states_invocation() {
    let rows = [
        Row {
            case: "the role's own policy grants the invocation",
            identity: Identity::Grants,
            target: Target::Nothing,
            form: Form::FunctionArn,
            decided: Decided::Made,
        },
        Row {
            case: "the function's own policy grants the role, whose policy grants nothing",
            identity: Identity::Silent,
            target: Target::GrantsTheRole,
            form: Form::FunctionArn,
            decided: Decided::Made,
        },
        Row {
            case: "the same grant, through the optimized integration",
            identity: Identity::Silent,
            target: Target::GrantsTheRole,
            form: Form::Optimized,
            decided: Decided::Made,
        },
        Row {
            case: "both documents grant it",
            identity: Identity::Grants,
            target: Target::GrantsTheRole,
            form: Form::Optimized,
            decided: Decided::Made,
        },
        Row {
            case: "neither document says anything about it",
            identity: Identity::Silent,
            target: Target::Nothing,
            form: Form::FunctionArn,
            decided: Decided::Refused(NO_IDENTITY_GRANT),
        },
        Row {
            case: "the function's policy grants another role",
            identity: Identity::Silent,
            target: Target::GrantsAnotherRole,
            form: Form::FunctionArn,
            decided: Decided::Refused(NO_IDENTITY_GRANT),
        },
        Row {
            case: "the function's policy names the account alone, which delegates",
            identity: Identity::Silent,
            target: Target::GrantsTheAccount,
            form: Form::Optimized,
            decided: Decided::Refused(NO_IDENTITY_GRANT),
        },
        Row {
            case: "the function's policy grants it and the role's own denies it",
            identity: Identity::Denies,
            target: Target::GrantsTheRole,
            form: Form::FunctionArn,
            decided: Decided::Refused(IDENTITY_DENY),
        },
        Row {
            case: "the role's policy grants it and the function's own denies it",
            identity: Identity::Grants,
            target: Target::DeniesTheRole,
            form: Form::Optimized,
            decided: Decided::Refused(RESOURCE_DENY),
        },
    ];

    let router = router();
    // One role whose policy says nothing about any invocation, which is the principal the
    // rows that grant ANOTHER role name: it exists so the grant names a real role of this
    // account rather than an ARN nothing holds.
    let other_role = given_role(
        &router,
        "the other role",
        "TaskInvocationOtherRole",
        &json!([{ "Effect": "Allow", "Action": UNRELATED_ACTION, "Resource": "*" }]),
    )
    .await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One function and one role per row, so no row reads another's documents.
        let name = format!("task-invocation-{index}");
        let function_arn = given_function(&router, case, &name).await;
        let statements = match row.identity {
            Identity::Grants => json!([
                { "Effect": "Allow", "Action": INVOKE_ACTION, "Resource": function_arn.clone() },
            ]),
            Identity::Silent => json!([
                { "Effect": "Allow", "Action": UNRELATED_ACTION, "Resource": "*" },
            ]),
            Identity::Denies => json!([
                { "Effect": "Allow", "Action": INVOKE_ACTION, "Resource": function_arn.clone() },
                { "Effect": "Deny", "Action": INVOKE_ACTION, "Resource": function_arn.clone() },
            ]),
        };
        let role_arn = given_role(
            &router,
            case,
            &format!("TaskInvocationRole{index}"),
            &statements,
        )
        .await;
        match row.target {
            Target::Nothing => {}
            Target::GrantsTheRole => given_permission(&router, case, &name, &role_arn).await,
            Target::GrantsAnotherRole => given_permission(&router, case, &name, &other_role).await,
            Target::GrantsTheAccount => given_permission(&router, case, &name, ACCOUNT).await,
            Target::DeniesTheRole => {
                given_resource_policy(
                    &router,
                    case,
                    &function_arn,
                    &json!({
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Sid": "DenyTheRole",
                            "Effect": "Deny",
                            "Principal": { "AWS": role_arn.clone() },
                            "Action": INVOKE_ACTION,
                            "Resource": function_arn.clone(),
                        }],
                    }),
                )
                .await;
            }
        }

        let described = ran(
            &router,
            case,
            &format!("task-invocation-machine-{index}"),
            &definition(row.form, &function_arn),
            &role_arn,
        )
        .await;
        let status = described.get("status").and_then(Value::as_str);

        match row.decided {
            Decided::Made => {
                // The invocation really happened, so what the execution reports is the
                // RUN's own answer: the handler echoes the order it was handed.
                assert_eq!(
                    status,
                    Some("SUCCEEDED"),
                    "case {case:?}: expected the invocation the documents allow to be made, got \
                     {described}"
                );
                let output: Value = serde_json::from_str(&string(&described, "output"))
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: expected a JSON output ({error}): {described}")
                    });
                let answered = &output["result"];
                // The optimized integration reports the whole `Invoke` answer, whose
                // `Payload` is what the run produced; naming the function directly reports
                // that payload itself.
                let payload = match answered.get("Payload") {
                    Some(payload) => payload,
                    None => answered,
                };
                assert_eq!(
                    (payload.get("order"), payload.get("handled")),
                    (Some(&json!(ORDER)), Some(&json!(true))),
                    "case {case:?}: expected the run's own answer, got {output}"
                );
            }
            Decided::Refused(because) => {
                assert_eq!(
                    (status, described.get("error").and_then(Value::as_str)),
                    (Some("FAILED"), Some(REFUSED_ERROR)),
                    "case {case:?}: expected FAILED with {REFUSED_ERROR}, got {described}"
                );
                let cause = string(&described, "cause");
                // The principal the refusal blames is the SESSION of the execution role
                // the invocation would have been made as; its NAME is the assume's own
                // value, so what is held here is everything around it.
                let principal = format!("User: arn:aws:sts::{ACCOUNT}:assumed-role/");
                assert!(
                    cause.starts_with(&principal),
                    "case {case:?}: expected the cause to blame a session of the role, got \
                     {cause:?}"
                );
                let sentence = format!(
                    " is not authorized to perform: {INVOKE_ACTION} on resource: {function_arn} \
                     {because} ({}",
                    refused_service(row.form)
                );
                assert!(
                    cause.contains(&sentence),
                    "case {case:?}: expected the cause to carry {sentence:?}, got {cause:?}"
                );
                assert!(
                    cause.contains(REFUSED_STATUS),
                    "case {case:?}: expected the cause to name the status the answer carried \
                     ({REFUSED_STATUS}), got {cause:?}"
                );
            }
        }
    }
}
