// SPDX-License-Identifier: CC0-1.0

//! Which APP CLIENT's token a `COGNITO_USER_POOLS` authorizer admits, driven end to end
//! through the whole assembled router.
//!
//! Two services and one engine, so this is a router suite: the tokens are minted by the Amazon
//! Cognito user pools service (signing name `cognito-idp`) and the gate that reads them is API
//! Gateway's own `execute-api` data plane (control plane signing name `apigateway`). Every
//! token presented here is one `AdminInitiateAuth` answered with — nothing is written that a
//! pool did not mint, and no claim is rewritten.
//!
//! The RULE the rows assert, one level above the recorded case:
//!
//! > A deployed method whose `authorizationType` is `COGNITO_USER_POOLS` admits a presented
//! > user-pool token when its `iss` names ANY pool the authorizer's `providerARNs` list, its
//! > signature checks out against that pool's published key set, it is inside its own window,
//! > and — where the authorizer declares an `identityValidationExpression` — its audience
//! > matches that expression: the `aud` of an identity token or the `client_id` of an access
//! > token. The issuer rule and the audience rule are INDEPENDENT, so neither how many pools
//! > the authorizer lists nor which app client of a trusted pool minted the token changes
//! > either verdict. An admitted request reaches the integration, which reads the presented
//! > token's own claims as `$context.authorizer.claims.<name>`; a token whose audience the
//! > expression does not match authorized nobody, which is `401`
//! > `{"message":"Unauthorized"}` with `x-amzn-errortype: UnauthorizedException`, and never
//! > the `500` of an authorizer whose decision could not be obtained.
//!
//! The rows span what that rule generalises over: WHICH app client of a trusted pool minted
//! the presented token (the pinned one, a SECOND app client of that same pool, an app client
//! of a second trusted pool), WHICH kind of token it is (the identity token, whose audience is
//! `aud`, and the access token of the same sign-in, whose audience is `client_id`), HOW MANY
//! pools the authorizer lists (one, two) and WHETHER it pins an audience at all (none, one app
//! client's id, another app client's id).
//!
//! The sibling suite `apigateway_cognito_user_pools_authorizer` spans the other half of the
//! same rule — how the token is presented, what was changed in it, and the method's own
//! `authorizationScopes`. This one spans the app-client audience, which is what the two pools
//! and the two clients of one pool are for.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every call is signed for, and so the region of each pool's issuer and of the
/// api's own invoke host.
const REGION: &str = "us-west-2";

/// The permanent password this suite's fixture users sign in with — a value written for this
/// test and used nowhere else. It satisfies a pool's default password policy.
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// The stage the api is deployed to.
const STAGE: &str = "prod";

/// The header each authorizer's identity source names.
const TOKEN_HEADER: &str = "Authorization";

/// What a request this authorizer does not admit earns, and that nothing the method fronts
/// ran.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";
const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";

/// The header the data plane reports a refusal's exception class in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The sentence the `API_CONFIGURATION_ERROR` / `AUTHORIZER_CONFIGURATION_ERROR` gateway
/// responses carry — the `500` that must never answer a token question.
const CONFIGURATION_ERROR: &str = "Internal server error";

/// The body the `MOCK` integration behind every gated resource answers: three claims of the
/// token the authorizer accepted, which is how a row observes that the gate let the request
/// THROUGH and that the integration reads the claims of the token actually presented.
const CLAIMS_TEMPLATE: &str = "{\"sub\":\"$context.authorizer.claims.sub\",\
     \"use\":\"$context.authorizer.claims.token_use\",\
     \"iss\":\"$context.authorizer.claims.iss\"}";

/// One request dispatched through the router, and what came back.
struct Answer {
    status: u16,
    body: Value,
    /// The body as it arrived, for a response that is not JSON at all.
    text: String,
    /// The exception class the response reported, or `"<no error type>"`.
    error_type: String,
}

impl Answer {
    /// One string member of a data-plane body, or `"<no member>"`.
    fn member(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("<no member>")
            .to_string()
    }

    fn summary(&self) -> String {
        format!("{} [{}] {}", self.status, self.error_type, self.text)
    }
}

/// Dispatch one request with the host, path, headers and body given.
async fn dispatch(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<no error type>")
        .to_string();
    let bytes = response.into_body();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer {
        status,
        body,
        text,
        error_type,
    }
}

/// The credential scope a signed SDK request carries. The signature is never checked — this
/// router runs with authentication off.
fn credential_scope(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsJson1_1 call to the Amazon Cognito user pools service, which must succeed.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let target = format!("AWSCognitoIdentityProviderService.{operation}");
    let authorization = credential_scope("cognito-idp");
    let answer = dispatch(
        router,
        &format!("cognito-idp.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &target),
            ("authorization", &authorization),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup: cognito-idp {operation} must answer 200, got {}",
        answer.summary()
    );
    answer.body
}

/// One API Gateway REST control-plane call, which must answer `expected_status`.
async fn control_plane(
    router: &MultiServiceRouter,
    what: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected_status: u16,
) -> Value {
    let authorization = credential_scope("apigateway");
    let answer = dispatch(
        router,
        &format!("apigateway.{REGION}.amazonaws.com"),
        method,
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization),
        ],
        body.map(|body| body.to_string().into_bytes())
            .unwrap_or_default(),
    )
    .await;
    assert_eq!(
        answer.status, expected_status,
        "{what} must answer HTTP {expected_status}, got {}",
        answer.summary()
    );
    answer.body
}

/// A required string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{what} must carry a non-empty {member:?}, got {body}"))
}

/// One base64url segment, decoded: "base64url without padding" (RFC 7515 §2), which is
/// RFC 4648 §5 — the two alphabet substitutions and the restored padding turn it back into the
/// standard alphabet this workspace already decodes.
fn decode_segment(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|e| panic!("a minted token's segment must be base64url ({e})"))
}

/// The claim set of a minted compact JWS: its second segment.
fn claims_of(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a minted token must be a compact JWS of three segments, got {} segment(s)",
        segments.len()
    );
    serde_json::from_slice(&decode_segment(segments[1]))
        .unwrap_or_else(|e| panic!("a minted token's claim set must be JSON ({e})"))
}

/// One claim of a minted token, as a string.
fn claim(token: &str, name: &str) -> String {
    let claims = claims_of(token);
    claims
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a minted token must carry a string {name:?} claim, got {claims}"))
        .to_string()
}

/// One app client of one pool, and the tokens of one sign-in through it.
struct AppClient {
    client_id: String,
    id_token: String,
    access_token: String,
}

/// One pool: what `Authorizer$providerARNs` names it by, and its app clients.
struct Pool {
    arn: String,
}

/// Create a pool holding one confirmed user, and return it.
async fn pool_with_user(router: &MultiServiceRouter, pool_name: &str, username: &str) -> (Pool, String) {
    let created = cognito(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let pool_id = as_str("CreateUserPool", &created["UserPool"], "Id");
    let arn = as_str("CreateUserPool", &created["UserPool"], "Arn");
    cognito(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": pool_id, "Username": username }),
    )
    .await;
    cognito(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true
        }),
    )
    .await;
    (Pool { arn }, pool_id)
}

/// Add one app client to `pool_id` and sign `username` in through it, so the suite holds that
/// client's own identity and access token.
async fn app_client_with_sign_in(
    router: &MultiServiceRouter,
    pool_id: &str,
    client_name: &str,
    username: &str,
) -> AppClient {
    let client = cognito(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": pool_id,
            "ClientName": client_name,
            // The flow the sign-in below uses, plus the refresh flow AWS requires beside any
            // other `ALLOW_` flow.
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]
        }),
    )
    .await;
    let client_id = as_str("CreateUserPoolClient", &client["UserPoolClient"], "ClientId");
    let signed_in = cognito(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD }
        }),
    )
    .await;
    let result = &signed_in["AuthenticationResult"];
    AppClient {
        client_id,
        id_token: as_str("AdminInitiateAuth", result, "IdToken"),
        access_token: as_str("AdminInitiateAuth", result, "AccessToken"),
    }
}

/// One `COGNITO_USER_POOLS`-gated `MOCK` method under the api's root, answering the claims the
/// authorizer published.
async fn gated_resource(
    router: &MultiServiceRouter,
    api: &str,
    root: &str,
    path_part: &str,
    authorizer: Value,
) -> String {
    let created = control_plane(
        router,
        &format!("CreateAuthorizer({path_part})"),
        "POST",
        &format!("/restapis/{api}/authorizers"),
        Some(authorizer),
        201,
    )
    .await;
    let authorizer_id = as_str("CreateAuthorizer", &created, "id");

    let resource = control_plane(
        router,
        &format!("CreateResource({path_part})"),
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": path_part })),
        201,
    )
    .await;
    let resource_id = as_str("CreateResource", &resource, "id");

    control_plane(
        router,
        &format!("PutMethod({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
        Some(json!({
            "authorizationType": "COGNITO_USER_POOLS",
            "authorizerId": authorizer_id
        })),
        201,
    )
    .await;
    control_plane(
        router,
        &format!("PutIntegration({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/integration"),
        Some(json!({ "type": "MOCK" })),
        201,
    )
    .await;
    control_plane(
        router,
        &format!("PutMethodResponse({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/responses/200"),
        Some(json!({})),
        201,
    )
    .await;
    control_plane(
        router,
        &format!("PutIntegrationResponse({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/integration/responses/200"),
        Some(json!({ "responseTemplates": { "application/json": CLAIMS_TEMPLATE } })),
        201,
    )
    .await;
    authorizer_id
}

/// What the gate owes one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The gate admitted the request, so the `MOCK` integration answered it with the claims of
    /// the token the row presented.
    Served,
    /// The authorizer does not admit what the request presented, so it authorized nobody.
    Unauthorized,
}

/// One request to the deployed stage: the gated resource it addresses, the token presented at
/// that resource's identity source, and what the gate owes it.
struct Row {
    case: String,
    path_part: &'static str,
    token: String,
    expected: Expected,
}

#[test]
fn a_cognito_authorizer_admits_the_app_client_its_validation_expression_pins() {
    std::thread::Builder::new()
        .name("apigateway-cognito-app-client-audience".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The IO and TIME drivers a service's background work needs; see the sibling
                // `apigateway_cognito_user_pools_authorizer` suite.
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check.
                router.set_request_authentication(false);

                // 1. One pool with TWO app clients, and a second pool with its own, so a row
                //    can vary which app client of which trusted pool minted the token.
                let (first, first_id) =
                    pool_with_user(&router, "audience-gate-pool", "audience-user").await;
                let pinned =
                    app_client_with_sign_in(&router, &first_id, "pinned-client", "audience-user")
                        .await;
                let sibling =
                    app_client_with_sign_in(&router, &first_id, "sibling-client", "audience-user")
                        .await;
                let (second, second_id) =
                    pool_with_user(&router, "audience-gate-second-pool", "second-user").await;
                let other_pool_client =
                    app_client_with_sign_in(&router, &second_id, "second-client", "second-user")
                        .await;

                // The audience each minted token carries is the app client it was minted for:
                // `aud` on an identity token, `client_id` on an access token. Read off the
                // tokens rather than written here, so every row below is about state the pools
                // hold.
                for (what, client) in [
                    ("the pinned app client", &pinned),
                    ("a sibling app client of the same pool", &sibling),
                    ("an app client of the second pool", &other_pool_client),
                ] {
                    assert_eq!(
                        claim(&client.id_token, "aud"),
                        client.client_id,
                        "case {what:?}: the identity token's aud must name the app client it \
                         was minted for, got {}",
                        claims_of(&client.id_token)
                    );
                    assert_eq!(
                        claim(&client.access_token, "client_id"),
                        client.client_id,
                        "case {what:?}: the access token's client_id must name the app client \
                         it was minted for, got {}",
                        claims_of(&client.access_token)
                    );
                }
                assert_ne!(
                    pinned.client_id, sibling.client_id,
                    "the two app clients of one pool must be distinct"
                );

                // 2. The api, one gated method per authorizer shape, and the stage.
                let api = control_plane(
                    &router,
                    "CreateRestApi",
                    "POST",
                    "/restapis",
                    Some(json!({ "name": "cognito-audience-gated-api" })),
                    201,
                )
                .await;
                let api_id = as_str("CreateRestApi", &api, "id");
                let root = as_str("CreateRestApi", &api, "rootResourceId");
                let header_source = format!("method.request.header.{TOKEN_HEADER}");

                // The four authorizer shapes the rows span: one pool or two in
                // `providerARNs`, crossed with which app client's id (if any) the
                // `identityValidationExpression` pins.
                let shapes: [(&str, Value); 4] = [
                    (
                        "one-pool-pinned",
                        json!({
                            "name": "one-pool-pinned",
                            "type": "COGNITO_USER_POOLS",
                            "providerARNs": [first.arn],
                            "identitySource": header_source,
                            "identityValidationExpression": format!("^{}$", pinned.client_id)
                        }),
                    ),
                    (
                        "two-pools-pinned",
                        json!({
                            "name": "two-pools-pinned",
                            "type": "COGNITO_USER_POOLS",
                            "providerARNs": [first.arn, second.arn],
                            "identitySource": header_source,
                            "identityValidationExpression": format!("^{}$", pinned.client_id)
                        }),
                    ),
                    (
                        "two-pools-unpinned",
                        json!({
                            "name": "two-pools-unpinned",
                            "type": "COGNITO_USER_POOLS",
                            "providerARNs": [first.arn, second.arn],
                            "identitySource": header_source
                        }),
                    ),
                    (
                        "two-pools-pinned-elsewhere",
                        json!({
                            "name": "two-pools-pinned-elsewhere",
                            "type": "COGNITO_USER_POOLS",
                            "providerARNs": [first.arn, second.arn],
                            "identitySource": header_source,
                            "identityValidationExpression":
                                format!("^{}$", other_pool_client.client_id)
                        }),
                    ),
                ];
                let mut two_pools_pinned = String::new();
                for (path_part, authorizer) in shapes {
                    let id = gated_resource(&router, &api_id, &root, path_part, authorizer).await;
                    if path_part == "two-pools-pinned" {
                        two_pools_pinned = id;
                    }
                }

                // The authorizer the api stored is what a later read reports, which is what
                // makes each verdict below one about persisted records.
                let read = control_plane(
                    &router,
                    "GetAuthorizer(two-pools-pinned)",
                    "GET",
                    &format!("/restapis/{api_id}/authorizers/{two_pools_pinned}"),
                    None,
                    200,
                )
                .await;
                assert_eq!(
                    read.get("providerARNs"),
                    Some(&json!([first.arn, second.arn])),
                    "GetAuthorizer must report both stored providerARNs, got {read}"
                );
                assert_eq!(
                    read.get("identityValidationExpression")
                        .and_then(Value::as_str),
                    Some(format!("^{}$", pinned.client_id).as_str()),
                    "GetAuthorizer must report the stored identityValidationExpression, got \
                     {read}"
                );

                control_plane(
                    &router,
                    "CreateDeployment",
                    "POST",
                    &format!("/restapis/{api_id}/deployments"),
                    Some(json!({ "stageName": STAGE })),
                    201,
                )
                .await;

                // 3. The rows.
                let rows: Vec<Row> = vec![
                    Row {
                        case: "the pinned app client's identity token, one pool listed"
                            .to_string(),
                        path_part: "one-pool-pinned",
                        token: pinned.id_token.clone(),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "a sibling app client of the same trusted pool, one pool listed"
                            .to_string(),
                        path_part: "one-pool-pinned",
                        token: sibling.id_token.clone(),
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "the pinned app client's identity token, two pools listed"
                            .to_string(),
                        path_part: "two-pools-pinned",
                        token: pinned.id_token.clone(),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "the pinned app client's ACCESS token, whose audience is client_id"
                            .to_string(),
                        path_part: "two-pools-pinned",
                        token: pinned.access_token.clone(),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "a sibling app client of the same trusted pool, two pools listed"
                            .to_string(),
                        path_part: "two-pools-pinned",
                        token: sibling.id_token.clone(),
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "the second trusted pool's app client, which is not the pinned one"
                            .to_string(),
                        path_part: "two-pools-pinned",
                        token: other_pool_client.id_token.clone(),
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "the pinned app client at an authorizer pinning no audience"
                            .to_string(),
                        path_part: "two-pools-unpinned",
                        token: pinned.id_token.clone(),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "a sibling app client at an authorizer pinning no audience"
                            .to_string(),
                        path_part: "two-pools-unpinned",
                        token: sibling.id_token.clone(),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "the second pool's app client at an authorizer pinning no audience"
                            .to_string(),
                        path_part: "two-pools-unpinned",
                        token: other_pool_client.id_token.clone(),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "the second pool's app client, which this authorizer pins"
                            .to_string(),
                        path_part: "two-pools-pinned-elsewhere",
                        token: other_pool_client.id_token.clone(),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "the second pool's ACCESS token, which this authorizer pins"
                            .to_string(),
                        path_part: "two-pools-pinned-elsewhere",
                        token: other_pool_client.access_token.clone(),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "the first pool's app client where another pool's client is pinned"
                            .to_string(),
                        path_part: "two-pools-pinned-elsewhere",
                        token: pinned.id_token.clone(),
                        expected: Expected::Unauthorized,
                    },
                ];

                let invoke_host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
                for row in &rows {
                    let answer = dispatch(
                        &router,
                        &invoke_host,
                        "GET",
                        &format!("/{STAGE}/{}", row.path_part),
                        &[(TOKEN_HEADER, row.token.as_str())],
                        Vec::new(),
                    )
                    .await;
                    // A token question is never answered by the 500 of an authorizer whose
                    // decision could not be obtained.
                    assert_ne!(
                        answer.member("message"),
                        CONFIGURATION_ERROR,
                        "case {:?}: the authorizer must decide the call, not report a \
                         configuration error; got {}",
                        row.case,
                        answer.summary()
                    );
                    match row.expected {
                        Expected::Unauthorized => {
                            assert_eq!(
                                (answer.status, answer.member("message"), &answer.error_type),
                                (
                                    UNAUTHORIZED,
                                    UNAUTHORIZED_MESSAGE.to_string(),
                                    &UNAUTHORIZED_ERROR_TYPE.to_string()
                                ),
                                "case {:?}: expected {UNAUTHORIZED} {UNAUTHORIZED_MESSAGE:?} \
                                 ({UNAUTHORIZED_ERROR_TYPE}), got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                        Expected::Served => {
                            assert_eq!(
                                answer.status,
                                200,
                                "case {:?}: expected the integration's 200, got {}",
                                row.case,
                                answer.summary()
                            );
                            // What reached the integration is the claim set of the token THIS
                            // row presented, which is what makes the row's admission an
                            // admission of that token and not of another.
                            for member in ["sub", "use", "iss"] {
                                let claim_name = if member == "use" { "token_use" } else { member };
                                let expected = claim(&row.token, claim_name);
                                assert_eq!(
                                    answer.body.get(member).and_then(Value::as_str),
                                    Some(expected.as_str()),
                                    "case {:?}: expected the presented token's {claim_name} \
                                     {expected:?} to reach the integration, got {}",
                                    row.case,
                                    answer.summary()
                                );
                            }
                        }
                    }
                }
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}
