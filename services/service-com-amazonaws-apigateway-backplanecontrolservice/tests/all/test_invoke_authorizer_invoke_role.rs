// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeAuthorizer` against an authorizer that names an INVOKE ROLE: which role
//! configuration lets the simulated execution produce a decision at all.
//!
//! THE RULE UNDER TEST, one level above the recorded case:
//!
//! > `Authorizer$authorizerCredentials` is "the required credentials as an IAM role for API
//! > Gateway to invoke the authorizer. To specify an IAM role for API Gateway to assume, use
//! > the role's Amazon Resource Name (ARN). To use resource-based permissions on the Lambda
//! > function, specify null" — so an authorizer naming a role has its function invoked UNDER
//! > that role, and the invocation is made exactly when that role both lets the API Gateway
//! > service principal assume it and is allowed `lambda:InvokeFunction` on the function the
//! > authorizer's `authorizerUri` names; the function's own resource-based policy is not
//! > consulted for such an authorizer, and it is the only grant for one naming no role. A
//! > simulated execution reports what that invocation produced: `clientStatus` 0 beside the
//! > principal and the policy the function returned where the invocation was made, and
//! > otherwise no decision at all — the status of the gateway response the client would have
//! > met, with neither member reported and the execution log naming which half of the role's
//! > configuration withheld the call.
//!
//! The rows differ along what the rule generalises over: WHICH half of the role's
//! configuration admits the invocation — the trust relationship, the action its permissions
//! allow and the resource they allow it on — each varied on its own, with the two
//! resource-policy rows (an authorizer naming no role, with and without a grant on the
//! function) as the control the role rows are told apart from.
//!
//! Only a cross-service suite can pin this: the api and the authorizer record are API
//! Gateway's, the function and its resource policy are AWS Lambda's, and the role — its trust
//! policy and its permissions — is IAM's, all three on ONE engine, as `core/router/src/lib.rs`
//! mounts them.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The function every authorizer of the api names, the execution role a created function
/// carries, and the handler its package declares.
const FUNCTION: &str = "role-invoked-authorizer";
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The header every authorizer declares as its identity source, and the token the simulated
/// requests present at it.
const IDENTITY_HEADER: &str = "Authorization";
const PRESENTED: &str = "allow-me";

/// How long this suite waits for AWS Lambda's own clock-owed transition (a created function
/// becoming `Active`), and how often it looks. Generous: the wait is on the CONDITION, and
/// nothing here is graded on how fast the host runs.
const WAIT: Duration = Duration::from_secs(30);
const LOOK: Duration = Duration::from_millis(50);

/// The service principal API Gateway assumes an authorizer's invoke role as.
const SERVICE_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The action a role has to be allowed on the function for the invocation to be made under it.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// An action that is not that one, so a role allowed only it is allowed no invocation.
const OTHER_ACTION: &str = "logs:PutLogEvents";

/// The status a simulation reports for an authorizer that produced a decision: "value is 0 if
/// the authorizer succeeded" (`TestInvokeAuthorizerResponse$clientStatus`).
const SUCCEEDED: i64 = 0;

/// The status of the gateway response a call whose authorizer could not be ASKED would have
/// met: the `AUTHORIZER_CONFIGURATION_ERROR` response type's `500`.
const NOT_ASKED: i64 = 500;

/// The authorizer's function: it allows the method it was handed, so a row that reaches it is
/// told from one that did not by the decision being reported at all.
const HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    token = event.get("authorizationToken") or ""
    return {
        "principalId": "principal-" + token,
        "policyDocument": {
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Action": "execute-api:Invoke",
                    "Effect": "Allow",
                    "Resource": event["methodArn"],
                }
            ],
        },
        "context": {"ran": "yes"},
    }
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    /// The body as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is not JSON at all.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: each call carries a credential scope so the
    // router can name the target service, and the signature itself is never checked here.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// One API Gateway control-plane call.
async fn apigw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
async fn accepted(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let answered = apigw(router, method, path, body).await;
    assert_eq!(
        answered.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    (answered.status, answered.json())
}

/// One AWS IAM call over its `awsQuery` wire, asserting the 200 it has to answer.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) {
    let encoded = |value: &str| {
        ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
            .to_string()
    };
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in members {
        body.push('&');
        body.push_str(&encoded(key));
        body.push('=');
        body.push_str(&encoded(value));
    }
    let answered = dispatch(
        router,
        "POST",
        "iam.amazonaws.com",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("iam")),
        ],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "setup {action}: expected HTTP 200 from IAM, got {} with {:?}",
        answered.status,
        answered.text()
    );
}

/// One role of this account: `trust` as its trust policy and `policy` as its one inline
/// policy. Answers the ARN IAM holds it under.
async fn role(router: &MultiServiceRouter, name: &str, trust: &str, policy: &str) -> String {
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("Path", "/"),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "invoke"),
            ("PolicyDocument", policy),
        ],
    )
    .await;
    format!(
        "arn:aws:iam::{account}:role/{name}",
        account = engine::emulator_account_id()
    )
}

/// A trust policy letting `principal` assume the role it is attached to.
fn trusting(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity policy allowing `action` on `resource` and nothing else.
fn allowing(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
    })
    .to_string()
}

/// A .zip deployment package holding `source` as the module the handler names.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// Create the authorizer's function and answer its ARN, once Lambda reports it `Active`.
///
/// It carries NO resource policy of its own: every role row below is therefore decided by the
/// role its authorizer names, and the two control rows add the grant they need.
async fn function(router: &MultiServiceRouter) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": EXECUTION_ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected HTTP 201, got {status} with {created}"
    );
    let arn = member("CreateFunction", &created, "FunctionArn");
    let deadline = Instant::now() + WAIT;
    loop {
        let (_, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "setup CreateFunction: the function never became Active, last read {configuration}"
        );
        tokio::time::sleep(LOOK).await;
    }
}

/// Grant `apigateway.amazonaws.com` the invocation of the function, scoped by `aws:SourceArn`
/// to one AUTHORIZER of the api — which is the grant that admits an authorizer naming NO role.
async fn grant(router: &MultiServiceRouter, api: &str, authorizer: &str) {
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": format!("apigateway-invoke-{authorizer}"),
            "Action": INVOKE_ACTION,
            "Principal": SERVICE_PRINCIPAL,
            "SourceArn": format!(
                "arn:aws:execute-api:{REGION}:{account}:{api}/authorizers/{authorizer}",
                account = engine::emulator_account_id(),
            ),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup AddPermission({authorizer}): expected HTTP 201, got {status} with {granted}"
    );
}

/// What the invoke role of one row is, or that the row's authorizer names none.
enum Credentials {
    /// A role of this account: `(name, the principal its trust policy admits, the action its
    /// permissions allow, the resource they allow it on)`. A `{function}` resource is the
    /// function's own ARN and `{other}` is another function's of the same account, each
    /// resolved once the function is created.
    Role(&'static str, &'static str, &'static str, &'static str),
    /// No `authorizerCredentials` at all, so the FUNCTION's resource policy is the grant —
    /// added for this authorizer when the row says the function grants it.
    None { granted: bool },
}

/// One row: the invoke role its authorizer names, and what a simulation then owes.
struct Row {
    case: &'static str,
    /// The `name` the row's own authorizer is created with.
    name: &'static str,
    credentials: Credentials,
    /// Whether the authorizer PRODUCED a decision — its function was invoked and returned an
    /// authorizer output — which is what the principal and the policy are reported for.
    decided: bool,
    client_status: i64,
    /// A phrase the execution log has to carry, naming why the invocation was not made.
    log: Option<&'static str>,
}

/// Every way an authorizer's invoke role can decide whether its function is asked at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role this service can assume, allowed the invocation of everything",
            name: "role-over-everything",
            credentials: Credentials::Role(
                "apigw-authorizer-invoker",
                SERVICE_PRINCIPAL,
                INVOKE_ACTION,
                "*",
            ),
            decided: true,
            client_status: SUCCEEDED,
            log: None,
        },
        Row {
            case: "a role allowed the invocation of exactly this function",
            name: "role-over-this-function",
            credentials: Credentials::Role(
                "apigw-authorizer-scoped",
                SERVICE_PRINCIPAL,
                INVOKE_ACTION,
                "{function}",
            ),
            decided: true,
            client_status: SUCCEEDED,
            log: None,
        },
        Row {
            case: "a role allowed the invocation of another function only",
            name: "role-over-another-function",
            credentials: Credentials::Role(
                "apigw-authorizer-elsewhere",
                SERVICE_PRINCIPAL,
                INVOKE_ACTION,
                "{other}",
            ),
            decided: false,
            client_status: NOT_ASKED,
            log: Some("is allowed no lambda:InvokeFunction"),
        },
        Row {
            case: "a role allowed another action only",
            name: "role-over-another-action",
            credentials: Credentials::Role(
                "apigw-authorizer-powerless",
                SERVICE_PRINCIPAL,
                OTHER_ACTION,
                "*",
            ),
            decided: false,
            client_status: NOT_ASKED,
            log: Some("is allowed no lambda:InvokeFunction"),
        },
        Row {
            case: "a role this service cannot assume at all",
            name: "role-this-service-cannot-assume",
            credentials: Credentials::Role(
                "apigw-authorizer-untrusted",
                "lambda.amazonaws.com",
                INVOKE_ACTION,
                "*",
            ),
            decided: false,
            client_status: NOT_ASKED,
            log: Some("does not let apigateway.amazonaws.com assume it"),
        },
        Row {
            case: "no credentials at all, on a function granting this authorizer the invocation",
            name: "granted-without-a-role",
            credentials: Credentials::None { granted: true },
            decided: true,
            client_status: SUCCEEDED,
            log: None,
        },
        Row {
            case: "no credentials at all, on a function granting it nothing",
            name: "ungranted-without-a-role",
            credentials: Credentials::None { granted: false },
            decided: false,
            client_status: NOT_ASKED,
            log: Some("no invocation was made"),
        },
    ]
}

/// Simulate `authorizer` of `api`, presenting the token at the authorizer's identity source.
async fn simulate(router: &MultiServiceRouter, api: &str, authorizer: &str) -> Answered {
    apigw(
        router,
        "POST",
        &format!("/restapis/{api}/authorizers/{authorizer}"),
        Some(json!({
            "pathWithQueryString": "/orders",
            "headers": { IDENTITY_HEADER: PRESENTED },
        })),
    )
    .await
}

/// A simulated execution is made under the role the authorizer names, so the role's trust and
/// its permissions are what decide whether a decision is produced at all.
#[tokio::test]
async fn a_simulation_is_made_under_the_invoke_role_the_authorizer_names() {
    let router = emulator();
    let function_arn = function(&router).await;
    let authorizer_uri = format!(
        "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/{function_arn}/invocations"
    );
    let api = accepted(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "authorizer-invoke-role-api" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api, "id");

    for row in rows() {
        let case = row.case;
        // What the row's authorizer names, and the grant it needs: a role is created through
        // IAM's own API, so its trust policy and its permissions are records rather than
        // values this suite asserts against itself.
        let credentials = match &row.credentials {
            Credentials::Role(name, principal, action, resource) => {
                let resource = match *resource {
                    "{function}" => function_arn.clone(),
                    "{other}" => format!(
                        "arn:aws:lambda:{REGION}:{account}:function:another-function",
                        account = engine::emulator_account_id(),
                    ),
                    literal => literal.to_string(),
                };
                Some(
                    role(
                        &router,
                        name,
                        &trusting(principal),
                        &allowing(action, &resource),
                    )
                    .await,
                )
            }
            Credentials::None { .. } => None,
        };
        let mut body = json!({
            "name": row.name,
            "type": "TOKEN",
            "authorizerUri": authorizer_uri,
            "identitySource": format!("method.request.header.{IDENTITY_HEADER}"),
            "authorizerResultTtlInSeconds": 0,
        });
        if let Some(credentials) = &credentials {
            body["authorizerCredentials"] = json!(credentials);
        }
        let created = accepted(
            &router,
            case,
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(body),
            201,
        )
        .await;
        let authorizer = member(case, &created, "id");
        // The record the simulation executes is read back through the control plane's own
        // reader, so it is the one a client can see.
        let read = accepted(
            &router,
            case,
            "GET",
            &format!("/restapis/{api}/authorizers/{authorizer}"),
            None,
            200,
        )
        .await;
        assert_eq!(
            read.get("authorizerCredentials").and_then(Value::as_str),
            credentials.as_deref(),
            "case {case:?}: expected GetAuthorizer to report the stored credentials, got {read}"
        );
        if matches!(row.credentials, Credentials::None { granted: true }) {
            grant(&router, &api, &authorizer).await;
        }

        let answered = simulate(&router, &api, &authorizer).await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected the simulation to be answered HTTP 200, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let reported = answered.json();
        assert_eq!(
            reported.get("clientStatus").and_then(Value::as_i64),
            Some(row.client_status),
            "case {case:?}: expected clientStatus {}, got {reported}",
            row.client_status
        );
        let principal = reported
            .get("principalId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let policy = reported
            .get("policy")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            (!principal.is_empty(), !policy.is_empty()),
            (row.decided, row.decided),
            "case {case:?}: expected the authorizer's own return value reported={}, got \
             {reported}",
            row.decided
        );
        if row.decided {
            assert_eq!(
                principal,
                format!("principal-{PRESENTED}"),
                "case {case:?}: expected the principal the function named, got {principal:?}"
            );
            assert!(
                policy.contains("execute-api:Invoke"),
                "case {case:?}: expected the policy the function returned, got {policy:?}"
            );
            // "The authorization response": the values the function published, which is what
            // tells an invocation that was MADE from one that never happened.
            let published = reported
                .get("authorization")
                .and_then(|values| values.get("ran"))
                .and_then(Value::as_array)
                .and_then(|values| values.first())
                .and_then(Value::as_str);
            assert_eq!(
                published,
                Some("yes"),
                "case {case:?}: expected the function's own published value, got {reported}"
            );
        }
        let log = reported
            .get("log")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            log.contains("Execution log for request"),
            "case {case:?}: expected an execution log, got {log:?}"
        );
        if let Some(phrase) = row.log {
            assert!(
                log.contains(phrase),
                "case {case:?}: expected the execution log to carry {phrase:?}, got {log:?}"
            );
        }
        // `claims` is a user-pool authorizer's own member, so a Lambda-backed one reports none
        // of it however its invocation was admitted.
        assert!(
            reported.get("claims").is_none(),
            "case {case:?}: expected no claims member for a Lambda-backed authorizer, got \
             {reported}"
        );
        assert!(
            reported.get("latency").and_then(Value::as_i64).is_some(),
            "case {case:?}: expected a measured latency, got {reported}"
        );
    }
}
