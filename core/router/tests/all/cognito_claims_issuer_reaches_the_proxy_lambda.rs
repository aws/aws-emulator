// SPDX-License-Identifier: CC0-1.0

//! The ISSUER a `COGNITO_USER_POOLS` authorizer hands an `AWS_PROXY` integration, as the
//! Lambda handler behind a deployed REST stage READS it in
//! `event.requestContext.authorizer.claims.iss` — and whether that value is one a relying
//! party can dereference.
//!
//! RULE: **the `iss` claim a user-pool authorizer publishes to its integration is the minting
//! pool's own stored issuer, passed through verbatim, and that issuer is a URL this emulator
//! ANSWERS on: its authority is the `host[:port]` of the endpoint the creating request reached
//! when that endpoint is the emulator's own and otherwise the address this emulator is served
//! at, its whole path is the minting pool's id, and `/.well-known/openid-configuration` and the
//! `/.well-known/jwks.json` that document locates are both served under it, the key set holding
//! the key the presented token names.**
//!
//! The issuer is token content, which no model describes. What states its FORM is API
//! Gateway V2's `JWTConfiguration$Issuer` — "the base domain of the identity provider that
//! issues JSON Web Tokens. For example, an Amazon Cognito user pool has the following format:
//! `https://cognito-idp.{region}.amazonaws.com/{userPoolId}`" — and what makes it a URL that
//! has to lead back HERE is that every OIDC relying party dereferences it: a verifier reads
//! the key set from `<iss>/.well-known/jwks.json` (Amazon Cognito Developer Guide, "Verifying
//! a JSON web token"), and a handler reading the claim to tell two providers apart fetches the
//! discovery document under it. A host in a zone AWS serves resolves nowhere near this
//! process, and neither does a name this emulator invents under any other zone, so an emulator
//! served on a port publishes the address it is reached at.
//!
//! Five crates have to agree for that to hold — Amazon Cognito user pools mint the token,
//! store the issuer and serve the two documents under it, IAM holds the execution role,
//! CloudFormation archives the inline handler into a package this emulator can run, Lambda
//! runs it, and API Gateway resolves the deployed stage, verifies the token and projects its
//! claims into the proxy event — so the suite lives here rather than in any one of them.
//!
//! What the neighbouring suites already pin, and this one does not repeat:
//! [`crate::cognito_claims_issuer_is_served_by_this_emulator`] pins the same claim behind a
//! `MOCK` integration's mapping template, and
//! [`crate::apigateway_cognito_authorizer_spares_the_proxy_lambda`] pins WHICH legs of a gated
//! proxy method reach the function at all. What only this suite reaches is the value a
//! handler itself READS out of the proxy event — the one a customer's code dereferences — and
//! that this emulator answers at it.
//!
//! The rows differ along what the rule generalises over: WHICH endpoint the pool was created
//! through (the emulator's served data port, another local address of it, and this service's
//! published regional host, which is a client that resolves AWS's own hostnames), WHICH
//! endpoint the token was minted on (a different one from the create in each row, so the
//! published issuer is the pool's STORED one rather than the sign-in's host), and WHICH token
//! of the sign-in is presented (the identity token and the access token).
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason the sibling router suites do.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::cognito_group_role_mapping::{decode_base64url, GOOD_PASSWORD};
use crate::query_wire::{form_encode, opt_element};

/// Keep this at or above the size the sibling router suites use.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every request here is signed for, and so the Region of each pool, of the
/// function and of the api's own invoke host. It is the Region the shared stack-settling
/// helper ([`wait_for_stack_status`]) reads a stack in, so the stack this suite deploys is the
/// one that helper waits for.
const REGION: &str = "us-east-1";

/// The base URL this emulator is SERVED at in this suite — the data port a client of the
/// emulator is configured with (`MultiServiceRouter::set_url_base`, which is what an emulator
/// served on a port reports as its endpoint), and the `host[:port]` of it.
const SERVED_BASE: &str = "https://127.0.0.1:9297";
const SERVED_AUTHORITY: &str = "127.0.0.1:9297";

/// Another address of THIS emulator a client can be configured with: the loopback zone RFC
/// 6761 reserves for the interface the core binds on, on a port of its own.
const OTHER_LOCAL_AUTHORITY: &str = "localhost:7700";

/// This service's published endpoint host in [`REGION`] — the address a client that resolves
/// AWS's own hostnames reaches a pool at.
fn published_host() -> String {
    format!("cognito-idp.{REGION}.amazonaws.com")
}

/// The stack that archives the inline handler, the status it must reach, the role the function
/// runs as and the function itself.
const STACK: &str = "xsvc-claims-issuer-proxy-stack";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const EXECUTION_ROLE: &str = "xsvc-claims-issuer-proxy-runner";
const FUNCTION: &str = "xsvc-claims-issuer-proxy-backend";

/// AWS's own managed execution-role policy for a function that writes nothing but its log,
/// ATTACHED rather than written out here, because that is what a deployment of a
/// Lambda-backed api grants its role.
const BASIC_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The stage the api is deployed to, and the resource path of its one gated method.
const STAGE: &str = "prod";
const RESOURCE_PATH: &str = "claims";

/// The header the authorizer declares as its identity source.
const TOKEN_HEADER: &str = "Authorization";

/// The user every pool signs in. The password is the shared fixture one ([`GOOD_PASSWORD`]),
/// which satisfies a pool's default password policy.
const USERNAME: &str = "claims-reader";

/// The two members of `AuthenticationResult` that carry a SIGNED token, both of which
/// announce the minting pool's issuer.
const TOKEN_MEMBERS: [&str; 2] = ["IdToken", "AccessToken"];

/// The claims the handler's view is compared against, name by name: the issuer this suite is
/// about, the subject the token names and what the token is for.
const PASSED_THROUGH: [&str; 3] = ["iss", "sub", "token_use"];

/// The paths under an issuer a relying party dereferences: its OpenID Connect discovery
/// document and the key set that document locates (RFC 8414 §3, and the Amazon Cognito
/// Developer Guide's "Verifying a JSON web token").
const OPENID_CONFIGURATION: &str = "/.well-known/openid-configuration";
const JWKS: &str = "/.well-known/jwks.json";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the claim the handler reads and not how
/// fast an interpreter gets through it.
const FUNCTION_TIMEOUT: u64 = 120;

/// How long to wait for work the clock owes — a function reaching `Active` — and how long
/// between two looks. Generous on purpose: everything waited for happens in this process, so
/// a loaded host does not decide the outcome.
const BUDGET: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The handler the gated method fronts: it answers a proxy envelope carrying the claim set the
/// authorizer published, which is what a customer's own code reads out of the event.
const BACKEND_HANDLER: &str = r#"
import json


def handler(event, context):
    authorizer = (event.get("requestContext") or {}).get("authorizer") or {}
    claims = authorizer.get("claims") or {}
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps(claims),
    }
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn summary(&self) -> String {
        format!("{} {}", self.status, self.text())
    }
}

/// The emulator as it is ASSEMBLED, on one engine, SERVED at [`SERVED_BASE`] — the deployment
/// every client of the emulator meets, and the one that has an address of its own to publish.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<::engine::InMemoryEngine> = Arc::new(::engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification belongs to the emulator, not to this check: the credential scope
    // is what routes an SDK request, and an invoke of the deployed stage carries none at all,
    // as a client of that URL sends none.
    router.set_request_authentication(false);
    router.set_url_base(
        EndpointUrl::parse(SERVED_BASE).expect("the suite's served base URL must parse"),
    );
    router
}

/// The access key id this emulator publishes, which speaks for the account's own identity.
const ACCOUNT_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// A SigV4 `Authorization` whose credential scope names `signing_name`. The signature itself
/// is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY_ID}/20260101/{REGION}/{signing_name}/\
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

/// One awsQuery call of `service` that has to be accepted, answered with its XML body.
async fn accepted_query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let answered = dispatch(
        router,
        "POST",
        &format!("{service}.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization(service)),
        ],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "setup: {service} {action} must answer 200, got {}",
        answered.summary()
    );
    answered.text()
}

/// One awsJson1_1 call to the Amazon Cognito user pools service, on the endpoint `host`
/// addresses, which must succeed.
async fn cognito(router: &MultiServiceRouter, host: &str, operation: &str, body: &Value) -> Value {
    let answered = dispatch(
        router,
        "POST",
        host,
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            (
                "x-amz-target",
                &format!("AWSCognitoIdentityProviderService.{operation}"),
            ),
            ("authorization", &authorization("cognito-idp")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "setup: cognito-idp {operation} on {host} must answer 200, got {}",
        answered.summary()
    );
    answered.json()
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
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
    .await
}

/// One API Gateway control-plane call that has to answer `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    what: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
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
    .await;
    assert_eq!(
        answered.status,
        expected,
        "setup {what:?}: must answer HTTP {expected}, got {}",
        answered.summary()
    );
    answered.json()
}

/// An ANONYMOUS GET of `url`, addressed at the authority the URL itself names — which is what
/// a relying party holding a token does with the locators it reads.
async fn fetch(router: &MultiServiceRouter, what: &str, url: &str) -> Answered {
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("{what}: {url:?} must be an https URL"));
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    dispatch(router, "GET", authority, path, &[], Bytes::new()).await
}

/// A required non-empty string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{what}: must carry a non-empty {member:?}, got {body}"))
}

/// The JOSE header and the claim set of the compact JWS `token`, read with this test target's
/// own base64url coding of a JOSE segment ([`decode_base64url`]).
fn header_and_claims(what: &str, token: &str) -> (Value, Value) {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "{what}: a token is a compact JWS of three segments, got {token:?}"
    );
    let decoded = |segment: &str| -> Value {
        serde_json::from_slice(&decode_base64url(segment))
            .unwrap_or_else(|error| panic!("{what}: a JWS segment must carry JSON ({error})"))
    };
    (decoded(segments[0]), decoded(segments[1]))
}

/// One row: where the pool was created, where its tokens are minted, and the authority its
/// issuer therefore names.
struct Row {
    case: &'static str,
    /// The endpoint the CREATE is addressed at — `None` for this service's published regional
    /// host, which is resolved at run time from [`REGION`].
    create_host: Option<&'static str>,
    /// The endpoint the SIGN-IN is addressed at, which is never the one the create reached.
    sign_in_host: Option<&'static str>,
    /// The `host[:port]` the pool's issuer must name.
    expected_authority: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a pool created through the emulator's served data port, minting on this \
               service's published regional host",
        create_host: Some(SERVED_AUTHORITY),
        sign_in_host: None,
        expected_authority: SERVED_AUTHORITY,
    },
    Row {
        case: "a pool created through this service's published regional host, minting on the \
               served data port",
        create_host: None,
        sign_in_host: Some(SERVED_AUTHORITY),
        expected_authority: SERVED_AUTHORITY,
    },
    Row {
        case: "a pool created through another local address of this emulator, minting on the \
               served data port",
        create_host: Some(OTHER_LOCAL_AUTHORITY),
        sign_in_host: Some(SERVED_AUTHORITY),
        expected_authority: OTHER_LOCAL_AUTHORITY,
    },
];

/// One pool of the fixture and the handles the rows need.
struct Pool {
    case: &'static str,
    id: String,
    arn: String,
    client_id: String,
    sign_in_host: String,
    expected_issuer: String,
}

/// Create the pool a row asks for, through the endpoint that row names, with an app client and
/// a `CONFIRMED` user.
async fn pool(router: &MultiServiceRouter, row: &Row, index: usize) -> Pool {
    let create_host = row
        .create_host
        .map(str::to_string)
        .unwrap_or_else(published_host);
    let sign_in_host = row
        .sign_in_host
        .map(str::to_string)
        .unwrap_or_else(published_host);
    let name = format!("claims-issuer-proxy-pool-{index}");
    let created = cognito(
        router,
        &create_host,
        "CreateUserPool",
        &json!({ "PoolName": &name }),
    )
    .await;
    let what = format!("setup CreateUserPool for case {:?}", row.case);
    let id = as_str(&what, &created["UserPool"], "Id");
    let arn = as_str(&what, &created["UserPool"], "Arn");
    let client = cognito(
        router,
        &create_host,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": &id,
            "ClientName": format!("{name}-client"),
            // The flow the sign-in uses, plus the refresh flow AWS requires beside any other
            // `ALLOW_` flow.
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = as_str(&what, &client["UserPoolClient"], "ClientId");
    cognito(
        router,
        &create_host,
        "AdminCreateUser",
        &json!({ "UserPoolId": &id, "Username": USERNAME, "MessageAction": "SUPPRESS" }),
    )
    .await;
    cognito(
        router,
        &create_host,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": &id,
            "Username": USERNAME,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let expected_issuer = format!("https://{}/{id}", row.expected_authority);
    Pool {
        case: row.case,
        id,
        arn,
        client_id,
        sign_in_host,
        expected_issuer,
    }
}

/// Create the execution role the function runs as, carrying AWS's own managed execution-role
/// policy, and answer the role's ARN.
async fn create_execution_role(router: &MultiServiceRouter) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = accepted_query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", EXECUTION_ROLE),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    let role_arn = opt_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup: CreateRole({EXECUTION_ROLE:?}) must answer an ARN"));
    accepted_query(
        router,
        "iam",
        "2010-05-08",
        "AttachRolePolicy",
        &[
            ("RoleName", EXECUTION_ROLE),
            ("PolicyArn", BASIC_EXECUTION_POLICY),
        ],
    )
    .await;
    role_arn
}

/// Deploy the stack declaring the function: the inline source is archived into the deployment
/// package by CloudFormation, which is how a function whose handler is written here comes to
/// exist at all. Answers the ARN Lambda reports for it, once it reports it `Active`.
async fn deploy_function(router: &MultiServiceRouter, role_arn: &str) -> String {
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Backend": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": role_arn,
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": { "ZipFile": BACKEND_HANDLER },
                },
            },
        },
    })
    .to_string();
    accepted_query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[("StackName", STACK), ("TemplateBody", &template)],
    )
    .await;
    let (reached, described) = wait_for_stack_status(router, STACK, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {described}"
    );
    // The invoke the integration makes needs the function RUNNABLE, which is a transition its
    // own service owes: look until it reports it rather than sleeping a span on its behalf.
    let deadline = Instant::now() + BUDGET;
    let configuration = loop {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await
        .json();
        if reported.get("State").and_then(Value::as_str) == Some("Active")
            || Instant::now() >= deadline
        {
            break reported;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    };
    assert_eq!(
        configuration.get("State").and_then(Value::as_str),
        Some("Active"),
        "setup: Lambda must report {FUNCTION:?} Active, got {configuration}"
    );
    as_str(
        "setup GetFunctionConfiguration",
        &configuration,
        "FunctionArn",
    )
}

/// The whole wiring a deployment of a pool-gated Lambda-backed api builds: the api, the
/// resource, the user-pool authorizer over every pool of the fixture, the gated method, the
/// proxy integration, the grant that admits the invocation and the deployment that mints the
/// stage. Answers the api id.
async fn deployed(router: &MultiServiceRouter, pools: &[Pool], function_arn: &str) -> String {
    let api_record = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "claims-issuer-proxy-api" })),
        201,
    )
    .await;
    let api = as_str("setup CreateRestApi", &api_record, "id");
    let root = as_str("setup CreateRestApi", &api_record, "rootResourceId");
    let resource = apigw(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": RESOURCE_PATH })),
        201,
    )
    .await;
    let resource_id = as_str("setup CreateResource", &resource, "id");
    let authorizer = apigw(
        router,
        "CreateAuthorizer",
        "POST",
        &format!("/restapis/{api}/authorizers"),
        Some(json!({
            "name": "pool-gate",
            "type": "COGNITO_USER_POOLS",
            "providerARNs": pools.iter().map(|pool| pool.arn.clone()).collect::<Vec<String>>(),
            "identitySource": format!("method.request.header.{TOKEN_HEADER}"),
            "authorizerResultTtlInSeconds": 0,
        })),
        201,
    )
    .await;
    let authorizer_id = as_str("setup CreateAuthorizer", &authorizer, "id");
    let method = format!("/restapis/{api}/resources/{resource_id}/methods/GET");
    apigw(
        router,
        "PutMethod",
        "PUT",
        &method,
        Some(json!({
            "authorizationType": "COGNITO_USER_POOLS",
            "authorizerId": authorizer_id,
        })),
        201,
    )
    .await;
    apigw(
        router,
        "PutIntegration",
        "PUT",
        &format!("{method}/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/{function_arn}/\
                 invocations"
            ),
        })),
        201,
    )
    .await;
    // The grant a deployment scopes by the api's OWN resources rather than by one method of
    // it: `aws:SourceArn` is matched as a PATTERN, so this one statement admits every stage
    // and every method of this api.
    let granted = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": "apigw-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": format!(
                "arn:aws:execute-api:{REGION}:{account}:{api}/*/*",
                account = ::engine::EMULATOR_ACCOUNT_ID
            ),
        })),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "setup: AddPermission must answer 201, got {}",
        granted.summary()
    );
    apigw(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;
    api
}

/// RULE: the `iss` an `AWS_PROXY` handler reads out of
/// `event.requestContext.authorizer.claims` is the minting pool's stored issuer, and this
/// emulator answers the discovery document and the key set under it.
#[test]
fn a_proxy_handler_reads_an_issuer_claim_this_emulator_answers_on() {
    std::thread::Builder::new()
        .name("cognito-claims-issuer-proxy".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The IO and TIME drivers a service's background work needs.
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(suite());
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}

async fn suite() {
    let router = emulator();

    // 1. The role, the function it runs as, and one pool per row.
    let role_arn = create_execution_role(&router).await;
    let function_arn = deploy_function(&router, &role_arn).await;
    let mut pools: Vec<Pool> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        pools.push(pool(&router, row, index).await);
    }

    // 2. One api over every pool, deployed to its stage.
    let api = deployed(&router, &pools, &function_arn).await;
    let invoke_host = format!("{api}.execute-api.{REGION}.amazonaws.com");

    // 3. The rows: each pool's sign-in, and each signed token of it presented at the gate.
    let mut published: Vec<String> = Vec::new();
    for pool in &pools {
        let case = pool.case;
        let signed_in = cognito(
            &router,
            &pool.sign_in_host,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": &pool.id,
                "ClientId": &pool.client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        let result = &signed_in["AuthenticationResult"];

        for token_member in TOKEN_MEMBERS {
            let what = format!("case {case:?} / {token_member}");
            let token = as_str(&what, result, token_member);
            let (header, minted) = header_and_claims(&what, &token);
            let minted_issuer = as_str(&what, &minted, "iss");
            assert_eq!(
                minted_issuer, pool.expected_issuer,
                "{what}: the minted token must announce iss {:?}, got {minted_issuer:?}",
                pool.expected_issuer
            );

            // The gate admits the pool's own token and hands the handler the claim set.
            let answered = dispatch(
                &router,
                "GET",
                &invoke_host,
                &format!("/{STAGE}/{RESOURCE_PATH}"),
                &[(TOKEN_HEADER, token.as_str())],
                Bytes::new(),
            )
            .await;
            assert_eq!(
                answered.status,
                200,
                "{what}: the gated proxy method must answer the handler's own 200, got {}",
                answered.summary()
            );
            let read_by_handler = answered.json();
            for claim in PASSED_THROUGH {
                let seen = as_str(
                    &format!("{what}: the claims the handler read"),
                    &read_by_handler,
                    claim,
                );
                let as_minted = as_str(&what, &minted, claim);
                assert_eq!(
                    seen, as_minted,
                    "{what}: the handler must read the {claim:?} claim as minted \
                     ({as_minted:?}), got {seen:?} in {}",
                    answered.text()
                );
            }
            let issuer = as_str(
                &format!("{what}: the claims the handler read"),
                &read_by_handler,
                "iss",
            );
            published.push(issuer.clone());

            // And that claim is a locator this emulator ANSWERS: the discovery document
            // fetched from it names it and locates the key set under it, and that key set
            // holds the key the presented token's `kid` names.
            let discovery_url = format!("{issuer}{OPENID_CONFIGURATION}");
            let discovered = fetch(&router, &what, &discovery_url).await;
            assert_eq!(
                discovered.status,
                200,
                "{what}: {discovery_url} must be served by this emulator, got {}",
                discovered.summary()
            );
            let stated = as_str(&what, &discovered.json(), "issuer");
            assert_eq!(
                stated, issuer,
                "{what}: the discovery document must state issuer {issuer:?}, got {stated:?}"
            );
            let jwks_uri = as_str(&what, &discovered.json(), "jwks_uri");
            assert_eq!(
                jwks_uri,
                format!("{issuer}{JWKS}"),
                "{what}: the discovery document must locate the key set under the issuer, got \
                 {jwks_uri:?}"
            );
            let key_set = fetch(&router, &what, &jwks_uri).await;
            assert_eq!(
                key_set.status,
                200,
                "{what}: {jwks_uri} must be served by this emulator, got {}",
                key_set.summary()
            );
            let key_id = as_str(&what, &header, "kid");
            let held: Vec<String> = key_set.json()["keys"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "{what}: a key set holds a keys array, got {}",
                        key_set.text()
                    )
                })
                .iter()
                .map(|key| as_str(&what, key, "kid"))
                .collect();
            assert!(
                held.contains(&key_id),
                "{what}: the key set at {jwks_uri} must hold the key {key_id:?} the token \
                 names, got {held:?}"
            );
        }
    }

    // One authorizer over several pools tells them apart by the issuer it publishes: the
    // claims the gate handed the handler are one issuer per pool, each naming its own pool.
    let distinct: std::collections::BTreeSet<&String> = published.iter().collect();
    assert_eq!(
        (published.len(), distinct.len()),
        (pools.len() * TOKEN_MEMBERS.len(), pools.len()),
        "each pool's tokens must publish one issuer of its own, got {published:?}"
    );
    for pool in &pools {
        assert!(
            distinct.contains(&pool.expected_issuer),
            "case {:?}: the handler must have read the pool's own issuer {:?}, got \
             {published:?}",
            pool.case,
            pool.expected_issuer
        );
    }
}
