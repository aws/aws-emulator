// SPDX-License-Identifier: CC0-1.0

//! What a JWT-authorized route does with the SCOPES of a token its authorizer accepted,
//! driven with tokens a user pool really minted and a function that really ran.
//!
//! The rule these rows assert, one level above any single deployment:
//!
//! > A route of a deployed HTTP api whose `AuthorizationType` is `JWT` is SERVED — its
//! > integration is performed and that backend's own answer is returned — when the access
//! > token presented at its authorizer's identity source claims at least one of the route's
//! > `AuthorizationScopes` ("the method invocation is authorized if any route scope matches
//! > a claimed scope in the access token"), and when the route configures no scope at all;
//! > a token the authorizer ACCEPTS that claims none of them is refused `403`
//! > `{"message":"Forbidden"}` ("otherwise, the invocation is not authorized",
//! > `Route$AuthorizationScopes`), while a token the authorizer cannot accept at all — none
//! > presented, or one that is no token, or one outside the authorizer's `Audience` however
//! > good its scopes — is refused `401` `{"message":"Unauthorized"}`. Neither refusal
//! > reaches the integration.
//!
//! The rows span what that rule generalises over: how many scopes the route requires (none,
//! one, two) and WHERE in that list the claimed one sits (first, last, absent), how many
//! scopes the token claims (one, two), which resource-server scope that is, how the token is
//! presented (RFC 6750 bearer credentials, bare, absent, a value that is no token), and
//! whether the clause that refuses the token is the scope clause (`403`) or an earlier one
//! (`401`).
//!
//! Nothing asserted here is a value this emulator filled in for the assertion's sake. The
//! tokens are the ones `GetClientToken` minted for an app client of a user pool this suite
//! created with a resource server of its own — the machine-to-machine grant whose scopes
//! "authorize requests to your resource servers" — so Amazon Cognito user pools is mounted
//! beside this service; and what a SERVED route answers is what its Lambda function
//! returned for the event it was handed, so AWS Lambda is mounted beside it too, all three
//! on one engine.
//!
//! `http_api_jwt_data_plane` owns the gate's other clauses (issuer, audience, the validity
//! window) and `http_api_jwt_claims_reach_the_integration` owns the shape of what an
//! admitted token hands on; this suite owns the SCOPE intersection and the two statuses it
//! is told apart by.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as user_pools;

use crate::api_lifecycle::{as_str, call_at_host};
use crate::http_api_backend_dispatch::{emulator_with_lambda, given_function, lambda_integration};
use crate::websocket_handshake_data_plane::{message, send};

/// The Region every request below is signed for: the Region of the pool whose issuer the
/// authorizer records, and of the function the integration names.
const REGION: &str = "us-east-1";

/// This service's own endpoint host, and the user pools service's, in [`REGION`].
fn apigateway_host() -> String {
    format!("apigateway.{REGION}.amazonaws.com")
}
pub(crate) fn cognito_host() -> String {
    format!("cognito-idp.{REGION}.amazonaws.com")
}

/// The `X-Amz-Target` prefix a user pools call carries: the local name of that service's
/// Smithy shape.
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The resource server this suite's pool defines, and the scope names it defines on it.
///
/// "Amazon Cognito represents scopes in the access token in the format
/// `$resource-server-identifier/$scope`" (`CreateResourceServer.Identifier`), so a scope a
/// route requires is that identifier and one of these names joined by `/`.
pub(crate) const RESOURCE_SERVER: &str = "https://orders.example.test";
const ADMIN: &str = "orders.admin";
pub(crate) const READ: &str = "orders.read";
const WRITE: &str = "orders.write";
/// A scope the resource server defines and NO token of this suite is granted, so a route
/// requiring it is one the presented token cannot meet.
pub(crate) const AUDIT: &str = "orders.audit";

/// Every scope [`RESOURCE_SERVER`] defines, which is also the set every app client
/// [`m2m_client`] creates is allowed: a grant may only ask for a scope the pool's resource
/// server really defines, so the two lists are one list.
pub(crate) const DEFINED_SCOPES: &[&str] = &[ADMIN, READ, WRITE, AUDIT];

/// Those scopes as `CreateResourceServer$Scopes` takes them.
pub(crate) fn resource_server_scopes() -> Vec<Value> {
    DEFINED_SCOPES
        .iter()
        .map(|name| {
            json!({
                "ScopeName": name,
                "ScopeDescription": format!("the {name} scope")
            })
        })
        .collect()
}

/// A `ClientSecretType`-shaped secret (pattern `^[\w+]+$`, length 24..64) this suite's app
/// clients are created with, so the grant can present the stored one.
///
/// A value written for this test and used nowhere else: it names no real credential, and the
/// pool it is created in lives only in this test's own in-memory engine.
const CLIENT_SECRET: &str = "m2mscopegate0000000000001";

/// The identity source the authorizer extracts the token from: "for HTTP APIs, use selection
/// expressions prefixed with $, for example, $request.header.Auth"
/// (`Authorizer$IdentitySource`).
const IDENTITY_SOURCE: &str = "$request.header.Authorization";

/// The status and sentence a token the authorizer cannot accept earns, and the one a token
/// it accepts earns at a route requiring a scope that token does not claim.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The handler every served route's integration runs: it reports the path it was invoked at
/// and what the authorizer told it about the caller, so a served row is asserted on what the
/// FUNCTION returned.
const SCOPE_REPORT: &str = "import json\n\
     def lambda_handler(event, context):\n\
     \x20   decided = event['requestContext']['authorizer']['jwt']\n\
     \x20   return {'statusCode': 200, 'headers': {'content-type': 'application/json'},\n\
     \x20            'body': json.dumps({'path': event['rawPath'],\n\
     \x20                                'scopes': decided['scopes'],\n\
     \x20                                'client_id': decided['claims']['client_id']})}\n";

/// A scope of [`RESOURCE_SERVER`], in the format a token and a route both spell it in.
pub(crate) fn scope(name: &str) -> String {
    format!("{RESOURCE_SERVER}/{name}")
}

/// What the gate owes one row.
#[derive(Debug, Clone)]
enum Expected {
    /// The route was served: the integration ran and answered, reporting these scopes (the
    /// ones the presented token claims) to the function.
    Served(Vec<String>),
    /// The authorizer accepted the token and the route requires a scope it does not claim.
    Forbidden,
    /// The authorizer could not accept the presented token at all.
    Unauthorized,
}

/// One request to the deployed stage: the path within the stage, what the `Authorization`
/// header carried, and what the gate owes it.
struct Row {
    case: String,
    path: &'static str,
    /// The header value presented, or `None` to present nothing at all.
    presented: Option<String>,
    expected: Expected,
}

/// One control-plane call against this service, signed for [`REGION`].
async fn control(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_at_host(emulator, &apigateway_host(), REGION, method, uri, body).await
}

/// One Amazon Cognito user pools call that must succeed; its credential scope is what routes
/// it to that service rather than to this one.
pub(crate) async fn cognito(emulator: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{}/", cognito_host()))
        .header("host", cognito_host())
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{COGNITO_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/cognito-idp/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the user pools request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "setup: cognito-idp {operation} must answer 200, got {status} {parsed}"
    );
    parsed
}

/// The CLAIM SET of a minted token: the second segment of its compact JWS, decoded with
/// the tree's one base64url reader (`service_common::jwt::decode_base64url`).
pub(crate) fn claims_of(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a minted token must be a compact JWS of three segments, got {} segment(s)",
        segments.len()
    );
    let claims = service_common::jwt::decode_base64url(segments[1])
        .unwrap_or_else(|| panic!("a minted token's claim set must be base64url"));
    serde_json::from_slice(&claims)
        .unwrap_or_else(|error| panic!("a minted token's claim set must be JSON ({error})"))
}

/// One app client of `user_pool_id` configured for the machine-to-machine grant, with every
/// scope of the pool's resource server allowed. Answers its `ClientId`.
pub(crate) async fn m2m_client(
    emulator: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
) -> String {
    let created = cognito(
        emulator,
        "CreateUserPoolClient",
        json!({
            "UserPoolId": user_pool_id,
            "ClientName": name,
            "ClientSecret": CLIENT_SECRET,
            // "To use this operation, you must configure the app client with a client
            // secret and enable the ALLOW_CLIENT_TOKEN_AUTH authentication flow"
            // (`GetClientToken`), which "must be the only authentication flow that you
            // configure for the app client".
            "ExplicitAuthFlows": ["ALLOW_CLIENT_TOKEN_AUTH"],
            "AllowedOAuthFlows": ["client_credentials"],
            "AllowedOAuthFlowsUserPoolClient": true,
            "AllowedOAuthScopes": [scope(ADMIN), scope(READ), scope(WRITE), scope(AUDIT)],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a ClientId, got {created}"))
        .to_string()
}

/// The access token `GetClientToken` mints for `client_id` carrying `scopes`, checked to be
/// a token whose `scope` claim really carries them: every scope a row relies on is the
/// POOL's, not a value written here.
pub(crate) async fn granted_token(
    emulator: &MultiServiceRouter,
    client_id: &str,
    scopes: &[String],
) -> String {
    let granted = cognito(
        emulator,
        "GetClientToken",
        json!({ "ClientId": client_id, "Secret": CLIENT_SECRET, "Scopes": scopes }),
    )
    .await;
    let token = granted["ClientAuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("GetClientToken must mint an AccessToken, got {granted}"))
        .to_string();
    let claims = claims_of(&token);
    let claimed: Vec<String> = claims
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    assert_eq!(
        claimed, scopes,
        "the granted token's scope claim must carry the scopes the grant authorized, got \
         {claims}"
    );
    token
}

/// The api this suite deploys: a JWT authorizer of the pool, a Lambda proxy integration on
/// the reporting handler, one route per scope configuration below, and the stage that serves
/// the api's root. Answers the host its `ApiEndpoint` names.
async fn deploy(
    emulator: &MultiServiceRouter,
    issuer: &str,
    audience: &str,
    function_arn: &str,
    routes: &[(&str, Vec<String>)],
) -> String {
    let (status, api) = control(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "jwt-scope-gated-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: CreateApi must answer 201, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let host = as_str(&api, "apiEndpoint")
        .split_once("://")
        .map(|(_scheme, rest)| rest.to_string())
        .unwrap_or_default();

    let (status, authorizer) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "pool-authorizer",
            "authorizerType": "JWT",
            "identitySource": [IDENTITY_SOURCE],
            "jwtConfiguration": { "issuer": issuer, "audience": [audience] }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: CreateAuthorizer must answer 201, got {status} {authorizer}"
    );
    let authorizer_id = as_str(&authorizer, "authorizerId");

    let (status, integration) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(lambda_integration(function_arn, "2.0")),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: CreateIntegration must answer 201, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    for (route_key, scopes) in routes {
        let mut request = json!({
            "routeKey": route_key,
            "target": format!("integrations/{integration_id}"),
            "authorizationType": "JWT",
            "authorizerId": authorizer_id
        });
        if !scopes.is_empty() {
            request["authorizationScopes"] = json!(scopes);
        }
        let (status, created) = control(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 201,
            "setup: CreateRoute({route_key}) must answer 201, got {status} {created}"
        );
        // The scopes the gate reads are the STORED ones: what the create answered is what a
        // later reader reports, so the route was persisted rather than echoed.
        let route_id = as_str(&created, "routeId");
        let (status, fetched) = control(
            emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: GetRoute({route_key}) must answer 200, got {status} {fetched}"
        );
        let reported = fetched
            .get("authorizationScopes")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            reported,
            json!(scopes),
            "case {route_key:?}: GetRoute must report the authorizationScopes the create \
             stored, expected {scopes:?}, got {reported} in {fetched}"
        );
    }

    let (status, stage) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default", "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: CreateStage must answer 201, got {status} {stage}"
    );
    host
}

/// Mount this service, AWS Lambda and Amazon Cognito user pools on one engine.
fn emulator_with_lambda_and_user_pools() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut emulator, engine) = emulator_with_lambda();
    emulator.register_service(user_pools::EmulatedService::new(Arc::clone(&engine)));
    (emulator, engine)
}

#[tokio::test]
async fn a_jwt_route_serves_the_token_that_claims_one_of_its_scopes_and_forbids_the_rest() {
    let (emulator, engine) = emulator_with_lambda_and_user_pools();

    // 1. The pool, its resource server and two app clients of it, with the tokens their
    //    machine-to-machine grants mint.
    let pool = cognito(
        &emulator,
        "CreateUserPool",
        json!({ "PoolName": "jwt-scope-gate-pool" }),
    )
    .await;
    let pool_id = pool["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an Id, got {pool}"))
        .to_string();
    let defined_scopes: Vec<Value> = resource_server_scopes();
    cognito(
        &emulator,
        "CreateResourceServer",
        json!({
            "UserPoolId": pool_id,
            "Identifier": RESOURCE_SERVER,
            "Name": "the orders api",
            "Scopes": defined_scopes,
        }),
    )
    .await;
    let client_id = m2m_client(&emulator, &pool_id, "the-apis-client").await;
    let other_client_id = m2m_client(&emulator, &pool_id, "another-client").await;

    let admin_token = granted_token(&emulator, &client_id, &[scope(ADMIN)]).await;
    let read_write_token =
        granted_token(&emulator, &client_id, &[scope(READ), scope(WRITE)]).await;
    // The same scope, minted for an app client the authorizer's `Audience` does not list: a
    // token whose scopes WOULD meet the route's, refused by an earlier clause.
    let other_client_token = granted_token(&emulator, &other_client_id, &[scope(ADMIN)]).await;

    // The issuer a JWT authorizer records for a user pool: "an Amazon Cognito user pool has
    // the following format: https://cognito-idp.{region}.amazonaws.com/{userPoolId}"
    // (`JWTConfiguration$Issuer`). The minted token has to name exactly that, or no
    // authorizer configured from the documented form could accept it.
    let issuer = format!("https://{}/{pool_id}", cognito_host());
    let claims = claims_of(&admin_token);
    assert_eq!(
        claims.get("iss").and_then(Value::as_str),
        Some(issuer.as_str()),
        "the pool's access token must name the documented issuer, got {claims}"
    );

    // 2. The api: one route per scope configuration the rule generalises over.
    let function = given_function(
        &emulator,
        &engine,
        "the scope-gated routes",
        "jwt-scope-report",
        SCOPE_REPORT,
    )
    .await;
    let routes: Vec<(&str, Vec<String>)> = vec![
        ("GET /admin-scope", vec![scope(ADMIN)]),
        ("GET /read-scope", vec![scope(READ)]),
        ("GET /audit-or-read", vec![scope(AUDIT), scope(READ)]),
        ("GET /admin-or-audit", vec![scope(ADMIN), scope(AUDIT)]),
        ("GET /open", Vec::new()),
    ];
    let host = deploy(&emulator, &issuer, &client_id, &function, &routes).await;

    // 3. The table. The scopes a served row must see are the token's own, read back as the
    //    SET the event reports them as.
    let admin_claimed = vec![scope(ADMIN)];
    let read_write_claimed = vec![scope(READ), scope(WRITE)];
    let bearer = |token: &str| Some(format!("Bearer {token}"));
    let rows = vec![
        Row {
            case: "the token that claims the route's one scope".to_string(),
            path: "/admin-scope",
            presented: bearer(&admin_token),
            expected: Expected::Served(admin_claimed.clone()),
        },
        Row {
            case: "a token claiming other scopes of the same resource server".to_string(),
            path: "/admin-scope",
            presented: bearer(&read_write_token),
            expected: Expected::Forbidden,
        },
        Row {
            case: "a token claiming two scopes, one of them the route's".to_string(),
            path: "/read-scope",
            presented: bearer(&read_write_token),
            expected: Expected::Served(read_write_claimed.clone()),
        },
        Row {
            case: "the claimed scope as the LAST of two route scopes".to_string(),
            path: "/audit-or-read",
            presented: bearer(&read_write_token),
            expected: Expected::Served(read_write_claimed.clone()),
        },
        Row {
            case: "the claimed scope as the FIRST of two route scopes".to_string(),
            path: "/admin-or-audit",
            presented: bearer(&admin_token),
            expected: Expected::Served(admin_claimed.clone()),
        },
        Row {
            case: "neither of the two scopes a route requires".to_string(),
            path: "/audit-or-read",
            presented: bearer(&admin_token),
            expected: Expected::Forbidden,
        },
        Row {
            case: "a route that configures no scope at all".to_string(),
            path: "/open",
            presented: bearer(&read_write_token),
            expected: Expected::Served(read_write_claimed.clone()),
        },
        Row {
            case: "the same token presented bare, with no bearer scheme".to_string(),
            path: "/admin-scope",
            presented: Some(admin_token.clone()),
            expected: Expected::Served(admin_claimed.clone()),
        },
        Row {
            case: "nothing presented at the authorizer's identity source".to_string(),
            path: "/admin-scope",
            presented: None,
            expected: Expected::Unauthorized,
        },
        Row {
            case: "a presented value that is no token".to_string(),
            path: "/admin-scope",
            presented: bearer("not-a-token-at-all"),
            expected: Expected::Unauthorized,
        },
        Row {
            case: "another app client's token, which claims the route's scope but is \
                   outside the authorizer's Audience"
                .to_string(),
            path: "/admin-scope",
            presented: bearer(&other_client_token),
            expected: Expected::Unauthorized,
        },
    ];

    for row in &rows {
        let case = &row.case;
        let headers: Vec<(&str, &str)> = match &row.presented {
            Some(value) => vec![("authorization", value.as_str())],
            None => Vec::new(),
        };
        let answer = send(&emulator, &host, "GET", row.path, &headers).await;
        match &row.expected {
            Expected::Served(scopes) => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} at {}: the route must be served by its integration, got {}",
                    row.path,
                    answer.summary()
                );
                let reported: Value = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
                assert_eq!(
                    reported.get("path").and_then(Value::as_str),
                    Some(row.path),
                    "case {case:?}: the function must have been invoked for {}, got {}",
                    row.path,
                    answer.text
                );
                assert_eq!(
                    reported.get("scopes").cloned().unwrap_or(Value::Null),
                    json!(scopes),
                    "case {case:?}: expected the authorizer to hand the function the \
                     token's scopes {scopes:?}, got {}",
                    answer.text
                );
                assert_eq!(
                    reported.get("client_id").and_then(Value::as_str),
                    Some(client_id.as_str()),
                    "case {case:?}: expected the admitted token's own client, got {}",
                    answer.text
                );
            }
            Expected::Forbidden => {
                assert_eq!(
                    (answer.status, message(&answer).as_str()),
                    (FORBIDDEN, FORBIDDEN_MESSAGE),
                    "case {case:?} at {}: expected ({FORBIDDEN}, {FORBIDDEN_MESSAGE:?}), got {}",
                    row.path,
                    answer.summary()
                );
            }
            Expected::Unauthorized => {
                assert_eq!(
                    (answer.status, message(&answer).as_str()),
                    (UNAUTHORIZED, UNAUTHORIZED_MESSAGE),
                    "case {case:?} at {}: expected ({UNAUTHORIZED}, \
                     {UNAUTHORIZED_MESSAGE:?}), got {}",
                    row.path,
                    answer.summary()
                );
            }
        }
    }
}
