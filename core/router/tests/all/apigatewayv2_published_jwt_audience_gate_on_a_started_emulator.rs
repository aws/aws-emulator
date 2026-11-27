// SPDX-License-Identifier: CC0-1.0

//! The JWT-audience gate of an HTTP api, driven at the address that api PUBLISHED by a stock
//! HTTP client of the STARTED binary: a real TLS connection to the `ApiEndpoint` value and
//! nothing else, as a client that was handed that value has.
//!
//! RULE: **a value this emulator publishes as an api's invoke address is an address this
//! process itself answers on — `host:port` of one of its own listeners, with the api named in
//! the path — because a client DIALS the published value verbatim ("the URI of the API … the
//! stage name is typically appended to this URI to form a complete path to a deployed API
//! stage", `Api$ApiEndpoint`); and behind that address the route's JWT authorizer decides the
//! request from the api's own records: a token whose `iss` is the authorizer's `Issuer` and
//! whose `aud` matches one entry of its `Audience` ("a list of the intended recipients of the
//! JWT. A valid JWT must provide an aud that matches at least one entry in this list",
//! `JWTConfiguration$Audience`) reaches the route's `AWS_PROXY` function, which RUNS and
//! answers the client with what it returned, while a token issued to a client the list does
//! not hold — of the same pool or of another — and a request carrying no token at all are
//! answered `401` `{"message":"Unauthorized"}` with the function never invoked.**
//!
//! A published name no resolver on this machine answers for makes every one of those decisions
//! unobservable: the client never gets a connection, so there is no status, no body and no
//! invocation either way. That is only visible from OUTSIDE the process — a suite that hands a
//! request to a router it built in its own process carries the authority as a header and never
//! resolves or dials it — so this suite starts the binary and dials what the control plane
//! published.
//!
//! The rows span what the rule generalises over: which app client minted the presented ID
//! token (the one entry of the authorizer's `Audience`, a second client of the SAME pool, a
//! client of ANOTHER pool, and no token at all) and the invoke URL built from the published
//! value (the value with the route path alone, which is how the reserved stage that serves the
//! api's root is reached, and with the stage name appended). The admitted row is driven again
//! after the refused ones, so an admission that only held while nothing had been refused fails
//! here.
//!
//! Four crates have to agree for the rule to hold — Amazon Cognito user pools mints the ID
//! tokens and publishes the issuer, CloudFormation archives the inline handler into a
//! deployment package this emulator can run, AWS Lambda holds the function and RUNS it, API
//! Gateway v2 mints the api, publishes the address and decides each request made there — and
//! Amazon CloudWatch Logs holds the record of every run, which is where a refused row observes
//! that no function was reached. So the suite lives here rather than in any one crate.
//!
//! What the neighbouring suites own and this one does not restate: the same gate's full answer
//! table in process, including the scope clause and a tampered `exp`
//! ([`crate::apigatewayv2_published_endpoint_jwt_lambda_route`]), and what the published
//! address enforces for a route with no authorizer in front of it
//! ([`crate::apigatewayv2_published_endpoint_enforcements_on_a_started_emulator`]). What is
//! added here is the audience decision at an address a stock client had to RESOLVE and DIAL.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cfn_deployed_pipeline_on_a_started_emulator::{Started, REGION};
use crate::cloudformation_lambda_function_log_destination::REPORT_PREFIX;
use crate::cognito_group_role_mapping::claims_of;
use crate::query_wire::opt_element;

/// The account this emulator serves, which the invocation grant's `SourceArn` carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The stack the backend function is deployed as, the name it is given and the status the
/// deployment must reach.
const STACK: &str = "apigwv2-published-jwt-gate";
const FUNCTION: &str = "apigwv2-published-jwt-gate-backend";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The permanent password this suite's fixture users sign in with: a value written for this
/// test and used nowhere else, satisfying a pool's default password policy (a length, an upper
/// and a lower case letter, a digit and a symbol).
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// The route the api carries, the path within the stage that matches it, the reserved stage
/// "that serves an api's root" and the identity source the authorizer reads the token from.
const ROUTE_KEY: &str = "GET /test";
const ROUTE_PATH: &str = "/test";
const DEFAULT_STAGE: &str = "$default";
const IDENTITY_SOURCE: &str = "$request.header.Authorization";

/// What an admitted request earns — the function's own answer — and what a token the
/// authorizer cannot accept earns.
const SERVED: u16 = 200;
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// How long one invocation of the function is given, as its own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 120;

/// How long to wait for work the emulator owes — a stack settling, a function reaching
/// `Active`, a run reaching its log — and how long between two looks. Generous on purpose, so
/// a loaded host never decides the outcome; nothing here asserts on elapsed time.
const BUDGET: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// How many consecutive readings make an unchanged run count: a refusal that reached no
/// function is an ABSENCE, so it is read more than once before it is believed.
const SETTLED_SAMPLES: usize = 3;

/// The handler the route fronts: it answers from the request it was handed, so a body carrying
/// what it computed is the FUNCTION's answer rather than one the gateway decided.
const ANSWERS_THE_PATH: &str = "import json\n\
     def handler(event, context):\n    \
     return {'statusCode': 200, 'headers': {'content-type': 'application/json'},\n            \
     'body': json.dumps({'ok': True, 'path': event['rawPath']})}\n";

/// The template the function is deployed by: CloudFormation archives the inline source into a
/// deployment package this emulator can run.
fn template() -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Backend": {
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": FUNCTION_TIMEOUT,
                    "InlineCode": ANSWERS_THE_PATH,
                },
            },
        },
    })
    .to_string()
}

/// A required string member of an answer.
fn member(what: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup {what:?}: must carry a non-empty {name:?}, got {body}"))
}

/// One signed API Gateway v2 control-plane call that has to answer `expected`, parsed.
fn apigatewayv2(
    emulator: &Started,
    what: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let (status, text) = emulator.call(
        "apigateway",
        method,
        path,
        &[("content-type", "application/json")],
        payload.into_bytes(),
    );
    assert_eq!(
        status, expected,
        "setup {what:?}: must answer HTTP {expected}, got {status} with {text}; the log:\n{}",
        emulator.log()
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One signed Amazon Cognito user pools call over its `awsJson1_1` wire, parsed.
fn cognito(emulator: &Started, operation: &str, body: &Value) -> Value {
    let (status, text) = emulator.rpc(
        "cognito-idp",
        &format!("AWSCognitoIdentityProviderService.{operation}"),
        "application/x-amz-json-1.1",
        body,
    );
    assert_eq!(
        status, 200,
        "setup: cognito-idp {operation} must answer 200, got {status} with {text}; the log:\n{}",
        emulator.log()
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// Deploy the backend function as a stack of its own and answer the ARN Lambda reports for it,
/// once it reports it `Active` — "the function is ready to be invoked"
/// (`FunctionConfiguration$State`), which is the condition this waits on.
fn given_function(emulator: &Started) -> String {
    let body = template();
    let (status, text) = emulator.cloudformation(
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &body),
            // The role the transform synthesizes for the function, and the macro the template
            // declares.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ],
    );
    assert_eq!(
        status, 200,
        "setup: CreateStack must be accepted, got {status} with {text}; the log:\n{}",
        emulator.log()
    );

    let deadline = Instant::now() + BUDGET;
    let mut reported = String::new();
    let mut described = String::new();
    while Instant::now() < deadline {
        let (status, body) = emulator.cloudformation("DescribeStacks", &[("StackName", STACK)]);
        assert_eq!(
            status, 200,
            "setup: DescribeStacks must answer 200, got {status} with {body}"
        );
        reported = opt_element(&body, "StackStatus").unwrap_or_default();
        described = body;
        if reported == CREATE_COMPLETE
            || reported.ends_with("_FAILED")
            || reported.contains("ROLLBACK")
        {
            break;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
    assert_eq!(
        reported, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {reported:?} with {described}; the \
         log:\n{}",
        emulator.log()
    );

    let deadline = Instant::now() + BUDGET;
    let configuration = loop {
        let (status, text) = emulator.call(
            "lambda",
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            &[],
            Vec::new(),
        );
        assert_eq!(
            status, 200,
            "setup: GetFunctionConfiguration({FUNCTION:?}) must answer 200, got {status} {text}"
        );
        let reported: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if reported.get("State").and_then(Value::as_str) == Some("Active")
            || Instant::now() >= deadline
        {
            break reported;
        }
        std::thread::sleep(LOOK_AGAIN);
    };
    assert_eq!(
        configuration.get("State").and_then(Value::as_str),
        Some("Active"),
        "setup: Lambda must report {FUNCTION:?} Active, got {configuration}"
    );
    member("GetFunctionConfiguration", &configuration, "FunctionArn")
}

/// Grant `apigateway.amazonaws.com` the invocation of the backend function, narrowed to every
/// stage, method and path of `api_id` — the grant a client makes beside the api it fronts.
fn grant(emulator: &Started, api_id: &str) {
    let source_arn = format!("arn:aws:execute-api:{REGION}:{ACCOUNT}:{api_id}/*/*");
    let (status, text) = emulator.call(
        "lambda",
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        &[("content-type", "application/json")],
        json!({
            "StatementId": format!("may-invoke-{api_id}"),
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": source_arn,
        })
        .to_string()
        .into_bytes(),
    );
    assert_eq!(
        status, 201,
        "setup: AddPermission({FUNCTION:?}, {source_arn:?}) must answer 201, got {status} {text}"
    );
}

/// Mint one user pool, one app client per entry of `client_names` and one user with a permanent
/// password, and answer the pool id, the client ids and the ID TOKEN each client's sign-in
/// produced — the token a client of a JWT-protected route presents, whose `aud` names the app
/// client it was minted for.
fn pool_with_signed_in_user(
    emulator: &Started,
    pool_name: &str,
    client_names: &[&str],
    username: &str,
) -> (String, Vec<String>, Vec<String>) {
    let pool = cognito(emulator, "CreateUserPool", &json!({ "PoolName": pool_name }));
    let pool_id = member("CreateUserPool", &pool["UserPool"], "Id");

    let mut client_ids = Vec::new();
    for client_name in client_names {
        let client = cognito(
            emulator,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": pool_id,
                "ClientName": client_name,
                // The flow the sign-in below uses, plus the refresh flow AWS requires beside
                // any other `ALLOW_` flow.
                "ExplicitAuthFlows": [
                    "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                    "ALLOW_REFRESH_TOKEN_AUTH"
                ],
            }),
        );
        client_ids.push(member(
            "CreateUserPoolClient",
            &client["UserPoolClient"],
            "ClientId",
        ));
    }

    cognito(
        emulator,
        "AdminCreateUser",
        &json!({ "UserPoolId": pool_id, "Username": username }),
    );
    cognito(
        emulator,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    );

    let mut id_tokens = Vec::new();
    for client_id in &client_ids {
        let signed_in = cognito(
            emulator,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": pool_id,
                "ClientId": client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
            }),
        );
        id_tokens.push(member(
            "AdminInitiateAuth",
            &signed_in["AuthenticationResult"],
            "IdToken",
        ));
    }

    (pool_id, client_ids, id_tokens)
}

/// The authority a client dials and the path it carries ahead of the stage, out of a published
/// endpoint.
fn published_parts(published: &str) -> (String, String) {
    let rest = published
        .split_once("://")
        .map_or(published, |(_scheme, rest)| rest);
    match rest.find('/') {
        Some(at) => (rest[..at].to_string(), rest[at..].to_string()),
        None => (rest.to_string(), String::new()),
    }
}

/// The host of an authority, without the port it carries and without the brackets an IPv6
/// literal is written in.
fn host_of(authority: &str) -> &str {
    let host = match authority.starts_with('[') {
        true => authority
            .split_once(']')
            .map_or(authority, |(host, _port)| host),
        false => authority
            .rsplit_once(':')
            .map_or(authority, |(host, _port)| host),
    };
    host.trim_start_matches('[').trim_end_matches(']')
}

/// What one dialled request was answered with.
struct Answered {
    status: u16,
    body: String,
}

impl Answered {
    /// The `message` a gateway refusal states, or the empty string for an answer that is not
    /// one.
    fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .as_ref()
            .and_then(|body| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// One request a CLIENT of the published address makes: dialled at that address over a real
/// connection, carrying its authority as the `Host`, no credential at all, and whatever the row
/// presents at the authorizer's identity source.
///
/// A dial that cannot be made at all is the defect this suite pins, so it fails HERE, naming
/// the address that was published.
fn dialled(
    emulator: &Started,
    case: &str,
    authority: &str,
    path: &str,
    presented: Option<&str>,
) -> Answered {
    let url = format!("https://{authority}{path}");
    let mut request = emulator.client().get(&url).header("host", authority);
    if let Some(token) = presented {
        request = request.header("authorization", token);
    }
    let response = request.send().unwrap_or_else(|error| {
        panic!(
            "case {case:?}: a published invoke address must be one a client can resolve and dial \
             as given, but nothing answered at {url}: {error}; the log:\n{}",
            emulator.log()
        )
    });
    Answered {
        status: response.status().as_u16(),
        body: response.text().unwrap_or_default(),
    }
}

/// How many runs of the function Amazon CloudWatch Logs holds: one `REPORT` line per
/// invocation, which is the record a run leaves whatever it answered.
fn runs(emulator: &Started) -> usize {
    emulator.logged(FUNCTION).matches(REPORT_PREFIX).count()
}

/// Look until the function's log holds `expected` runs, so an admitted row asserts on the
/// CONDITION rather than on how fast this host runs.
fn wait_for_runs(emulator: &Started, case: &str, expected: usize) {
    let deadline = Instant::now() + BUDGET;
    let mut held = runs(emulator);
    while held < expected && Instant::now() < deadline {
        std::thread::sleep(LOOK_AGAIN);
        held = runs(emulator);
    }
    assert_eq!(
        held, expected,
        "case {case:?}: expected the function to have run {expected} time(s), got {held}; its \
         log:\n{}",
        emulator.logged(FUNCTION)
    );
}

/// Assert the function's log still holds exactly `expected` runs, read [`SETTLED_SAMPLES`]
/// times over: a refused request reached no function, which is an absence a client confirms by
/// looking again.
fn still_runs(emulator: &Started, case: &str, expected: usize) {
    for sample in 0..SETTLED_SAMPLES {
        if sample > 0 {
            std::thread::sleep(LOOK_AGAIN);
        }
        let held = runs(emulator);
        assert_eq!(
            held, expected,
            "case {case:?}: a refused request must reach no function, so the run count must \
             stay {expected}, got {held} at reading {sample}; its log:\n{}",
            emulator.logged(FUNCTION)
        );
    }
}

/// One row: what it presents at the identity source, where under the published endpoint it
/// dials, and what it is owed.
struct Row {
    case: String,
    /// The path under the published endpoint, built from the value a client was handed.
    within: String,
    /// The `Authorization` the request carries, or `None` for the row that carries none.
    presented: Option<String>,
    status: u16,
    /// Whether the route's function must have run for it.
    reaches_the_function: bool,
}

/// RFC 6750 bearer credentials, which is how a client presents an ID token at the identity
/// source.
fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// RULE: the address an HTTP api publishes is one a stock client resolves and dials as given,
/// and behind it the route's JWT authorizer admits an ID token whose `aud` is a listed app
/// client of the configured issuer — the function running and answering — while a token of an
/// unlisted client of that pool, a token of another pool and a request carrying none are
/// refused `401` with no function reached.
#[test]
fn a_started_emulator_decides_the_jwt_audience_at_the_address_the_api_published() {
    let emulator = Started::start();
    let function_arn = given_function(&emulator);

    // The pool the authorizer is configured for, with the app client its `Audience` lists and a
    // second client of the same pool, which it does not.
    let (_pool_id, client_ids, id_tokens) = pool_with_signed_in_user(
        &emulator,
        "published-jwt-gate-pool",
        &["listed-client", "unlisted-client"],
        "gate-user",
    );
    let listed_client = client_ids[0].clone();
    let listed_token = id_tokens[0].clone();
    let unlisted_token = id_tokens[1].clone();

    // Another pool: a token of it names an issuer this authorizer does not record and a client
    // its audience does not hold.
    let (_other_pool, _other_clients, other_tokens) = pool_with_signed_in_user(
        &emulator,
        "published-jwt-gate-other-pool",
        &["other-pool-client"],
        "other-pool-user",
    );
    let other_pool_token = other_tokens[0].clone();

    // The issuer the pool announces, taken from the token it minted: `JWTConfiguration$Issuer`
    // is "the base domain of the identity provider that issues JSON Web Tokens", which is the
    // value a relying party configures and the value the token states.
    let claims = claims_of(&listed_token);
    let issuer = claims
        .get("iss")
        .and_then(Value::as_str)
        .filter(|issuer| issuer.starts_with("https://"))
        .unwrap_or_else(|| {
            panic!("setup: a minted ID token must name an https issuer, got {claims}")
        })
        .to_string();
    assert_eq!(
        claims.get("aud").and_then(Value::as_str),
        Some(listed_client.as_str()),
        "setup: an ID token's aud must name the app client it was minted for, got {claims}"
    );

    let api = apigatewayv2(
        &emulator,
        "CreateApi",
        "POST",
        "/v2/apis",
        Some(json!({ "name": "published-jwt-gate-api", "protocolType": "HTTP" })),
        201,
    );
    let api_id = member("CreateApi", &api, "apiId");
    let published = member("CreateApi", &api, "apiEndpoint");
    grant(&emulator, &api_id);

    let integration = apigatewayv2(
        &emulator,
        "CreateIntegration",
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": function_arn,
            "integrationMethod": "POST",
            "payloadFormatVersion": "2.0",
        })),
        201,
    );
    let integration_id = member("CreateIntegration", &integration, "integrationId");
    let authorizer = apigatewayv2(
        &emulator,
        "CreateAuthorizer",
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "pool-authorizer",
            "authorizerType": "JWT",
            "identitySource": [IDENTITY_SOURCE],
            "jwtConfiguration": { "issuer": issuer, "audience": [listed_client] },
        })),
        201,
    );
    let authorizer_id = member("CreateAuthorizer", &authorizer, "authorizerId");
    apigatewayv2(
        &emulator,
        "CreateRoute",
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": ROUTE_KEY,
            "authorizationType": "JWT",
            "authorizerId": authorizer_id,
            "target": format!("integrations/{integration_id}"),
        })),
        201,
    );
    apigatewayv2(
        &emulator,
        "CreateStage",
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": DEFAULT_STAGE, "autoDeploy": true })),
        201,
    );

    // The value is the api's own, settled when it was minted, so the reporter a client reads it
    // back from states the same address.
    let fetched = apigatewayv2(
        &emulator,
        "GetApi",
        "GET",
        &format!("/v2/apis/{api_id}"),
        None,
        200,
    );
    assert_eq!(
        member("GetApi", &fetched, "apiEndpoint"),
        published,
        "GetApi must report the address CreateApi published ({published:?}), got {fetched}"
    );

    // The FORM of the published value: an https address whose host is one this process answers
    // on — an address literal of its own interface — with the api named in the path. A name
    // this emulator minted in a zone of its own would pass no resolver here.
    let (authority, base_path) = published_parts(&published);
    assert!(
        published.starts_with("https://"),
        "an HTTP api is invoked over https, got {published:?}"
    );
    let host = host_of(&authority);
    assert!(
        host.parse::<std::net::IpAddr>().is_ok() || host == host_of(&emulator.authority()),
        "the published authority must be an address this process answers on — an address \
         literal, or the host this emulator is reached at ({:?}) — got {published:?}",
        emulator.authority()
    );
    assert_eq!(
        base_path,
        format!("/_aws/execute-api/{api_id}"),
        "the published value must name the api behind the reserved data-plane path, got \
         {published:?}"
    );

    // The invoke URL a client builds from the published value: the route path alone, which is
    // how the reserved stage that serves the api's root is reached, and the stage name appended
    // ahead of it.
    let at_root = format!("{base_path}{ROUTE_PATH}");
    let at_stage = format!("{base_path}/{DEFAULT_STAGE}{ROUTE_PATH}");
    let rows = vec![
        Row {
            case: "the ID token of the app client the authorizer's Audience lists".to_string(),
            within: at_root.clone(),
            presented: Some(bearer(&listed_token)),
            status: SERVED,
            reaches_the_function: true,
        },
        Row {
            case: "the same token, at the published value with the stage name appended"
                .to_string(),
            within: at_stage.clone(),
            presented: Some(bearer(&listed_token)),
            status: SERVED,
            reaches_the_function: true,
        },
        Row {
            case: "a second app client of the SAME pool, outside the one-entry Audience"
                .to_string(),
            within: at_root.clone(),
            presented: Some(bearer(&unlisted_token)),
            status: UNAUTHORIZED,
            reaches_the_function: false,
        },
        Row {
            case: "an app client of ANOTHER pool: an issuer the authorizer does not record"
                .to_string(),
            within: at_stage.clone(),
            presented: Some(bearer(&other_pool_token)),
            status: UNAUTHORIZED,
            reaches_the_function: false,
        },
        Row {
            case: "no token at all at the authorizer's identity source".to_string(),
            within: at_root.clone(),
            presented: None,
            status: UNAUTHORIZED,
            reaches_the_function: false,
        },
        Row {
            case: "the listed client's token again, once refusals have been answered".to_string(),
            within: at_root.clone(),
            presented: Some(bearer(&listed_token)),
            status: SERVED,
            reaches_the_function: true,
        },
    ];

    let mut owed_runs = 0;
    for row in &rows {
        let case = row.case.as_str();
        let answered = dialled(
            &emulator,
            case,
            &authority,
            &row.within,
            row.presented.as_deref(),
        );
        assert_eq!(
            answered.status, row.status,
            "case {case:?}: expected HTTP {} at {}{}, got {} with {}",
            row.status, authority, row.within, answered.status, answered.body
        );
        match row.reaches_the_function {
            true => {
                let body: Value = serde_json::from_str(&answered.body).unwrap_or(Value::Null);
                assert_eq!(
                    body.get("ok"),
                    Some(&json!(true)),
                    "case {case:?}: expected the function's own answer, got {}",
                    answered.body
                );
                let reported = body
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    reported.ends_with(ROUTE_PATH),
                    "case {case:?}: expected the function to report the route path it was \
                     reached at ({ROUTE_PATH:?}), got {reported:?}"
                );
                owed_runs += 1;
                wait_for_runs(&emulator, case, owed_runs);
            }
            false => {
                assert_eq!(
                    answered.message(),
                    UNAUTHORIZED_MESSAGE,
                    "case {case:?}: expected the gateway's own {UNAUTHORIZED_MESSAGE:?}, got {}",
                    answered.body
                );
                still_runs(&emulator, case, owed_runs);
            }
        }
    }
}
