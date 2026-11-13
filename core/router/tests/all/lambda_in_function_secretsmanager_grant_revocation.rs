// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING function's own Secrets Manager read is authorized against: the policies in
//! force on the function's execution role AT THE INSTANT OF THE CALL.
//!
//! RULE: a call an AWS SDK makes from inside a running function is made as a session of that
//! function's execution role, and the service serving it decides each call afresh against the
//! identity-based documents then in force on that role — so a `GetSecretValue` the role's
//! documents allow answers that version's own `ARN`, `Name`, `VersionId`, `VersionStages` and
//! the stored `SecretString`, the identical call made after the grant is withdrawn is refused
//! the AWS-common `AccessDeniedException` naming the assumed-role session, the IAM action and
//! the identifier the request supplied, and the identical call made after the grant is put
//! back is served again. Nothing about the function, its environment or its held session
//! changes between the three calls: the only thing that changes is the document IAM holds.
//!
//! "The execution role grants the function permission to use Amazon Web Services services"
//! (`FunctionConfiguration$Role`), and the identity such a call speaks for is the session
//! `arn:aws:sts::<account>:assumed-role/<role>/<function>`; with neither an identity-based nor
//! a resource-based allow a request is denied (IAM User Guide, *Policy evaluation logic*), and
//! `secretsmanager:GetSecretValue` is the permission that read requires ("Required
//! permissions: secretsmanager:GetSecretValue", `GetSecretValue`, this service's own model).
//!
//! Five crates have to agree for that to hold — IAM holds the role and the inline document a
//! `PutRolePolicy` rewrites between calls, STS mints the session the function's calls are
//! signed with, CloudFormation archives the inline handler into a deployment package this
//! emulator can run, Lambda runs that handler and supplies it the SDK, and Secrets Manager
//! holds the secret and takes the decision — so the suite lives here: a crate-local suite can
//! only ever see one side of it. The Secrets Manager crate's own `identity_based_authorization`
//! suite pins the decision for a credential a client signs with directly, and
//! `cloudformation_condition_gated_secrets_policy` pins a grant that is fixed for the life of
//! one stack; what only this suite reaches is the same function's read being served, refused
//! and served again as the role's document is rewritten under it.
//!
//! The rows differ along what the rule generalises over: HOW the grant is written (the action
//! alone, the whole service's actions by wildcard, the action inside a list beside another of
//! this service's), WHICH resource that statement names (the secret's own ARN, or every
//! resource), and the FORM of the value the granted read must answer with (a plain string, a
//! JSON document, text carrying newlines and non-ASCII). Each row is probed on its own secret
//! AND on a second secret of the account, so a statement scoped to one secret is shown to
//! admit that secret and no other, and every probe is taken three times over — granted,
//! withdrawn, restored.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::stack_status;
use crate::query_wire::{elements, form_encode, opt_element, physical_ids_of};

/// The Region every request here is signed for, and so the Region the secrets and the
/// functions belong to.
const REGION: &str = "us-east-1";

/// The account this emulator serves, which every session's ARN carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The stack that declares one function per row.
const STACK: &str = "xsvc-in-fn-secret-probes";

/// The `StackStatus` that stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The IAM action the handler's read performs, which a grant names and a refusal blames
/// ("Actions defined by AWS Secrets Manager").
const READ_ACTION: &str = "secretsmanager:GetSecretValue";

/// The code and status Secrets Manager refuses an unauthorized call with: the AWS-common
/// `AccessDeniedException`, carried at this service's client-error status because awsJson1_1
/// discriminates a failure by the error type in the body rather than by the transport status.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// The staging label the version a read resolves by default carries: "If you don't specify
/// either a VersionStage or VersionId, Secrets Manager returns the AWSCURRENT version."
/// (`GetSecretValue`, this service's Smithy model.)
const STAGE_CURRENT: &str = "AWSCURRENT";

/// The name of the one inline policy each row's role carries, which every rewrite of that
/// row's grant replaces.
const POLICY_NAME: &str = "inline";

/// The second secret of the account, which no row's secret-scoped statement names.
const OTHER_SECRET_NAME: &str = "xsvc-in-fn-elsewhere";

/// The value that second secret holds, so a served read of it is asserted on its own material.
const OTHER_SECRET_VALUE: &str = "the-other-secrets-own-value";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the authorization decision and not how
/// fast an interpreter gets through the handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// How often to look while waiting for work the clock owes — the stack reaching
/// `CREATE_COMPLETE`, a created function reaching `Active` — and how many looks to take. The
/// spans are those services' own, so no fixed sleep names the instant one ends, and the
/// deadline is generous so a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 1_200;

/// The handler every row runs: it builds a Secrets Manager client with the SDK the runtime
/// supplies, reads the secret the EVENT names, and reports either the members that read
/// answered or the refusal Secrets Manager answered it with — its code, its sentence and the
/// HTTP status it came back on.
const PROBE_HANDLER: &str = r##"
import boto3
from botocore.exceptions import ClientError


def handler(event, context):
    client = boto3.client("secretsmanager")
    try:
        answered = client.get_secret_value(SecretId=event["secret_id"])
    except ClientError as refused:
        return {
            "ok": False,
            "code": refused.response["Error"]["Code"],
            "message": refused.response["Error"]["Message"],
            "status": refused.response["ResponseMetadata"]["HTTPStatusCode"],
            "reported": str(refused),
        }
    return {
        "ok": True,
        "arn": answered["ARN"],
        "name": answered["Name"],
        "secret": answered["SecretString"],
        "version_id": answered["VersionId"],
        "stages": answered["VersionStages"],
    }
"##;

/// The emulator as it is ASSEMBLED, on one engine: a handler's own SDK call is dispatched back
/// into this same router, so the secret it reads is the secret this suite created.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. Its credential is the long-term key every emulator publishes, so these calls
/// are the account's own and not any row's role's.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Lambda `restJson1` call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// One Secrets Manager operation over its `awsJson1_1` wire, made as the account's own
/// credential.
async fn secretsmanager(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("authorization", authorization("secretsmanager"))
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// WHICH resource a row's statement names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The row's own secret, by the ARN Secrets Manager minted for it.
    OwnSecret,
    /// Every resource, which the bare `*` wildcard stands for.
    Every,
}

/// One row: how the grant is written, which resource it names, and the value the row's own
/// secret holds.
struct Row {
    case: &'static str,
    /// The actions the grant allows, beside the log actions every execution role carries.
    actions: &'static [&'static str],
    scope: Scope,
    /// The `SecretString` the row's secret is created with, which a served read must answer
    /// verbatim. `SecretStringType`'s only constraint is a length of 1..65536
    /// (`smithy_query(query="shape", shape="SecretStringType")`), so its text is free.
    value: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "the action alone, on the secret's own ARN, over a plain string",
        actions: &[READ_ACTION],
        scope: Scope::OwnSecret,
        value: "a-plain-secret-string",
    },
    Row {
        case: "every action of the service, on every resource, over a JSON document",
        actions: &["secretsmanager:*"],
        scope: Scope::Every,
        value: r#"{"endpoint":"db.internal","port":5432,"username":"app"}"#,
    },
    Row {
        case: "the action inside a list beside another of this service's, on the secret's own \
               ARN, over text with newlines and non-ASCII",
        actions: &["secretsmanager:DescribeSecret", READ_ACTION],
        scope: Scope::OwnSecret,
        value: "line one\nline two with £ and a space\n",
    },
];

/// The role one row runs as, the function it runs in, and the secret it reads — named here
/// because the refusal's own sentence names the first two: the identity a function's call is
/// signed as is the session `arn:aws:sts::<account>:assumed-role/<role>/<function>`.
fn role_name(index: usize) -> String {
    format!("xsvc-in-fn-secret-role-{index}")
}
fn function_name(index: usize) -> String {
    format!("xsvc-in-fn-secret-probe-{index}")
}
fn secret_name(index: usize) -> String {
    format!("xsvc-in-fn-secret-{index}")
}

/// How many characters a token below carries: inside the published 32..64 bound of
/// `ClientRequestTokenType` (`smithy_query(query="shape", shape="ClientRequestTokenType")`).
const VERSION_TOKEN_LEN: usize = 40;

/// The `ClientRequestToken` one secret is created with, which becomes that version's own id —
/// "this parameter specifies a unique identifier for the new version"
/// (`CreateSecretRequest$ClientRequestToken`), so a served read's `VersionId` is this value and
/// not one the suite has to be told.
fn version_token(name: &str) -> String {
    // Every name here is ASCII and shorter than the token, so the token is that name followed
    // by filler out to the fixed length.
    let mut token = format!("{name}-version-");
    token.truncate(VERSION_TOKEN_LEN);
    while token.len() < VERSION_TOKEN_LEN {
        token.push('0');
    }
    token
}

/// The sentence Secrets Manager refuses a read nothing in force allows with. Written out here
/// rather than built from the service's own helper, so the wording has something to be wrong
/// about: the principal is the session the function's call is signed as, the action is the one
/// the read performs, and the resource is the identifier the request supplied for the secret.
fn refusal_message(index: usize, secret_id: &str) -> String {
    format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{}/{} is not authorized to perform: \
         {READ_ACTION} on resource: {secret_id} because no identity-based policy allows the \
         {READ_ACTION} action",
        role_name(index),
        function_name(index),
    )
}

/// The trust policy a Lambda execution role needs.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The statement every execution role carries whatever else it is granted: the log actions a
/// function writes its own log stream with, which grant nothing of Secrets Manager.
fn log_statement() -> Value {
    json!({
        "Effect": "Allow",
        "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
        "Resource": "*",
    })
}

/// The document a row's role carries while its grant is IN FORCE: the log actions, and the
/// row's own statement on the resource it names.
fn granting_document(row: &Row, own_secret_arn: &str) -> String {
    let resource = match row.scope {
        Scope::OwnSecret => own_secret_arn.to_string(),
        Scope::Every => "*".to_string(),
    };
    json!({
        "Version": "2012-10-17",
        "Statement": [
            log_statement(),
            {"Effect": "Allow", "Action": row.actions, "Resource": resource},
        ],
    })
    .to_string()
}

/// The document that replaces it when the grant is WITHDRAWN: the same least-privilege
/// baseline every execution role needs, and nothing of Secrets Manager at all.
fn logs_only_document() -> String {
    json!({"Version": "2012-10-17", "Statement": [log_statement()]}).to_string()
}

/// Put `document` in force as the one inline policy of the row's role.
async fn put_role_policy(router: &MultiServiceRouter, case: &str, index: usize, document: &str) {
    let name = role_name(index);
    let (status, answered) = query(
        router,
        "iam",
        "2010-05-08",
        "PutRolePolicy",
        &[
            ("RoleName", &name),
            ("PolicyName", POLICY_NAME),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy({name:?}) must succeed, got {status} with {answered}"
    );
}

/// Create the role one row runs as, and answer the ARN IAM minted for it. The grant itself is
/// put in force later, so one role is rewritten by `PutRolePolicy` between the probes.
async fn create_role(router: &MultiServiceRouter, index: usize) -> String {
    let name = role_name(index);
    let (status, created) = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", &name),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole({name:?}) must succeed, got {status} with {created}"
    );
    opt_element(&created, "Arn").unwrap_or_else(|| {
        panic!("setup: CreateRole({name:?}) must answer the role's ARN, got {created}")
    })
}

/// Create one secret holding `value` and answer the ARN Secrets Manager minted for it.
async fn create_secret(router: &MultiServiceRouter, name: &str, value: &str) -> String {
    let (status, created) = secretsmanager(
        router,
        "CreateSecret",
        &json!({
            "Name": name,
            "SecretString": value,
            "ClientRequestToken": version_token(name),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateSecret({name:?}) must succeed, got {status} with {created}"
    );
    let arn = created["ARN"].as_str().unwrap_or_else(|| {
        panic!("setup: CreateSecret({name:?}) must answer the secret's ARN, got {created}")
    });
    assert_eq!(
        created["VersionId"].as_str(),
        Some(version_token(name).as_str()),
        "setup: the created version is the one the ClientRequestToken named, got {created}"
    );
    arn.to_string()
}

/// The template the suite deploys: one function per row, running as that row's role. The
/// inline source is archived into the deployment package by CloudFormation, which is how a
/// function whose handler is written here comes to exist at all.
fn stack_template(role_arns: &[String]) -> String {
    let mut resources = serde_json::Map::new();
    for (index, role_arn) in role_arns.iter().enumerate() {
        resources.insert(
            format!("Probe{index}"),
            json!({
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": function_name(index),
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": role_arn,
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": {"ZipFile": PROBE_HANDLER},
                },
            }),
        );
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": Value::Object(resources),
    })
    .to_string()
}

/// Deploy the stack, wait for `CREATE_COMPLETE` and answer the physical id of each declared
/// function.
async fn deploy(router: &MultiServiceRouter, role_arns: &[String]) -> BTreeMap<String, String> {
    let template = stack_template(role_arns);
    let (status, answered) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[("StackName", STACK), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({STACK:?}) must be accepted, got {status} with {answered}"
    );
    // The stack's completion is owed from the instant Lambda reports its functions created,
    // which no fixed sleep can name: this looks until it reports it, with a deadline generous
    // enough that a loaded host does not decide the outcome.
    let mut reported = stack_status(router, STACK).await;
    for _ in 0..LOOKS {
        if reported.0 == CREATE_COMPLETE {
            break;
        }
        tokio::time::sleep(LOOK).await;
        reported = stack_status(router, STACK).await;
    }
    let failures = if reported.0 == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStackEvents",
            &[("StackName", STACK)],
        )
        .await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {} with resource failures \
         {failures:?}",
        reported.1
    );
    let (status, resources) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "DescribeStackResources",
        &[("StackName", STACK)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: DescribeStackResources({STACK:?}) must succeed, got {status} with {resources}"
    );
    physical_ids_of(&resources)
}

/// Wait until Lambda reports `name` `Active` — "the function is ready to be invoked"
/// (`FunctionConfiguration$State`) — which is the state an invocation needs.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    let mut last = Value::Null;
    for _ in 0..LOOKS {
        let (_, _, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &[],
        )
        .await;
        if configuration["State"].as_str() == Some("Active") {
            return;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: Lambda must report {name:?} Active, last reported {last}");
}

/// One secret, as a probe addresses it and as a served read must report it.
struct Secret {
    /// The identifier the handler's request carries, which is also what a refusal names.
    id: String,
    name: String,
    value: &'static str,
}

/// Invoke one row's function on one secret and answer the document its handler returned.
async fn probe(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    secret: &Secret,
) -> Value {
    let event = json!({"secret_id": secret.id});
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the invocation must be served, got {status} with {payload}"
    );
    let faulted = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        faulted, None,
        "case {case:?}: the handler must run to completion, got {faulted:?} with {payload}"
    );
    payload
}

/// What the handler reports for a read the policies in force ALLOW: the version's own members,
/// read out of the secret this suite created.
fn served(secret: &Secret) -> Value {
    json!({
        "ok": true,
        "arn": secret.id,
        "name": secret.name,
        "secret": secret.value,
        "version_id": version_token(&secret.name),
        "stages": [STAGE_CURRENT],
    })
}

/// What it reports for a read nothing in force allows: the code, the sentence and the status
/// the service answered, and the one line a handler that logs the exception writes — the SDK's
/// own rendering of a refusal, "An error occurred (<Code>) when calling the <Operation>
/// operation: <Message>".
fn refused(index: usize, secret: &Secret) -> Value {
    let message = refusal_message(index, &secret.id);
    json!({
        "ok": false,
        "code": ACCESS_DENIED,
        "message": message,
        "status": ACCESS_DENIED_STATUS,
        "reported": format!(
            "An error occurred ({ACCESS_DENIED}) when calling the GetSecretValue operation: \
             {message}"
        ),
    })
}

/// Every row: the function's own read is decided against the document in force on its
/// execution role when the call is made, so withdrawing the grant refuses the identical call
/// and putting it back serves it again.
#[tokio::test]
async fn an_in_function_secret_read_follows_its_execution_roles_grant() {
    let router = build_router();

    let other = Secret {
        id: create_secret(&router, OTHER_SECRET_NAME, OTHER_SECRET_VALUE).await,
        name: OTHER_SECRET_NAME.to_string(),
        value: OTHER_SECRET_VALUE,
    };

    let mut secrets = Vec::new();
    let mut role_arns = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let name = secret_name(index);
        secrets.push(Secret {
            id: create_secret(&router, &name, row.value).await,
            name,
            value: row.value,
        });
        role_arns.push(create_role(&router, index).await);
    }
    let physical = deploy(&router, &role_arns).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let own = &secrets[index];
        let logical = format!("Probe{index}");
        let function = physical
            .get(&logical)
            .cloned()
            .unwrap_or_else(|| panic!("setup: the stack must report a physical id for {logical}"));
        assert_eq!(
            function,
            function_name(index),
            "setup: the stack must name the function {:?}, got {function:?}",
            function_name(index)
        );
        wait_until_active(&router, &function).await;

        // Phase 1: the grant is in force, so the read is served — and a statement scoped to
        // this one secret admits this secret and no other.
        put_role_policy(&router, case, index, &granting_document(row, &own.id)).await;
        let answered = probe(&router, case, &function, own).await;
        let expected = served(own);
        assert_eq!(
            answered, expected,
            "case {case:?}: granted, expected the handler to report {expected}, got {answered}"
        );

        let answered = probe(&router, case, &function, &other).await;
        let expected = if row.scope == Scope::Every {
            served(&other)
        } else {
            refused(index, &other)
        };
        assert_eq!(
            answered, expected,
            "case {case:?}: granted, expected the read of the other secret to report \
             {expected}, got {answered}"
        );

        // Phase 2: the same role, carrying the logs-only baseline instead, is granted nothing
        // of this service — so the identical call is refused, with IAM's own sentence.
        put_role_policy(&router, case, index, &logs_only_document()).await;
        let answered = probe(&router, case, &function, own).await;
        let expected = refused(index, own);
        assert_eq!(
            answered, expected,
            "case {case:?}: withdrawn, expected the handler to report {expected}, got {answered}"
        );

        let answered = probe(&router, case, &function, &other).await;
        let expected = refused(index, &other);
        assert_eq!(
            answered, expected,
            "case {case:?}: withdrawn, expected the read of the other secret to report \
             {expected}, got {answered}"
        );

        // Phase 3: the grant is put back, and the identical call is served again.
        put_role_policy(&router, case, index, &granting_document(row, &own.id)).await;
        let answered = probe(&router, case, &function, own).await;
        let expected = served(own);
        assert_eq!(
            answered, expected,
            "case {case:?}: restored, expected the handler to report {expected}, got {answered}"
        );
    }
}
