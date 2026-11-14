// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeAuthorizer` against a `COGNITO_USER_POOLS` authorizer over SEVERAL user pools:
//! which pools' tokens it accepts, and which audience its validation expression admits.
//!
//! THE RULE UNDER TEST, one level above the recorded case:
//!
//! > `Authorizer$providerARNs` is "a list of the Amazon Cognito user pool ARNs for the
//! > `COGNITO_USER_POOLS` authorizer", so such an authorizer accepts an identity token exactly
//! > when the pool that minted it is ONE OF the pools that list names — every entry of it,
//! > however many it carries — and `Authorizer$identityValidationExpression` says "for
//! > `COGNITO_USER_POOLS` authorizers, API Gateway will match the `aud` field of the incoming
//! > token from the client against the specified regular expression", so where the authorizer
//! > declares one, only a token minted for an app client that expression matches is accepted.
//! > A token the authorizer accepts is an authorizer that succeeded, reported at `clientStatus`
//! > 0 with that token's claim set; a token it does not accept earned no decision at all, so
//! > the report carries the status of the gateway response the client would have met and no
//! > claims.
//!
//! The rows differ along what the rule generalises over: WHICH pool of the list minted the
//! presented token (the first entry, a later entry, a pool the list does not name), and WHICH
//! app client of a listed pool it was minted for (the one the validation expression names,
//! another client of the same pool, a client of another listed pool) — so the size of the
//! `providerARNs` list and the audience of the token vary independently.
//!
//! Only a cross-service suite can pin this: the api and the authorizer record are API
//! Gateway's, while every pool that minted a token and publishes the key set it is verified
//! against is Amazon Cognito's — both on ONE engine, as `core/router/src/lib.rs` mounts them.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The Region every call below is signed for, and so the Region of every pool's issuer.
const REGION: &str = "us-west-2";

/// The header every authorizer declares as its identity source.
const IDENTITY_HEADER: &str = "Authorization";

/// The permanent password every fixture user signs in with — written for this suite and used
/// nowhere else. It satisfies a pool's default password policy.
const PASSWORD: &str = "Str0ng-Passw0rd!";

/// The status a simulation reports for an authorizer that SUCCEEDED: "value is 0 if the
/// authorizer succeeded" (`TestInvokeAuthorizerResponse$clientStatus`).
const SUCCEEDED: i64 = 0;

/// The status of the gateway response a request this authorizer does not admit would have met:
/// it authorized nobody, which is the `401` of the `UNAUTHORIZED` response type.
const UNAUTHORIZED: i64 = 401;

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
}

/// The two services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: a call carries a credential scope so the
    // router can name the target service, and the signature itself is never checked here.
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
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

/// One awsJson1_1 call to the user pools service, under the `X-Amz-Target` its protocol names
/// the operation by. It must succeed: every one of them is setup.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let host = format!("cognito-idp.{REGION}.amazonaws.com");
    let answered = dispatch(
        router,
        "POST",
        &host,
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("authorization", &authorization("cognito-idp")),
            (
                "x-amz-target",
                &format!("AWSCognitoIdentityProviderService.{operation}"),
            ),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "setup: cognito-idp {operation} must answer HTTP 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
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

/// A required non-empty string member of a response body.
fn as_str(what: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup {what:?}: expected a non-empty {name:?}, got {body}"))
}

/// One user pool of this account: the id it is addressed by and the ARN a `providerARNs` entry
/// names it with.
struct Pool {
    id: String,
    arn: String,
}

/// Mint one pool and one confirmed user in it, who signs in below.
async fn pool(router: &MultiServiceRouter, name: &str, username: &str) -> Pool {
    let created = cognito(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    let id = as_str("CreateUserPool", &created["UserPool"], "Id");
    let arn = as_str("CreateUserPool", &created["UserPool"], "Arn");
    cognito(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": id, "Username": username }),
    )
    .await;
    cognito(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": id,
            "Username": username,
            "Password": PASSWORD,
            "Permanent": true
        }),
    )
    .await;
    Pool { id, arn }
}

/// One app client of `pool`, allowed the sign-in flow below.
async fn app_client(router: &MultiServiceRouter, pool: &Pool, name: &str) -> String {
    let client = cognito(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": pool.id,
            "ClientName": name,
            // The flow the sign-in below uses, plus the refresh flow AWS requires beside any
            // other `ALLOW_` flow.
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]
        }),
    )
    .await;
    as_str("CreateUserPoolClient", &client["UserPoolClient"], "ClientId")
}

/// Sign `username` in to `pool` through the app client `client_id`, and answer the IDENTITY
/// token of that sign-in — the token whose `aud` is that app client.
async fn id_token(
    router: &MultiServiceRouter,
    pool: &Pool,
    client_id: &str,
    username: &str,
) -> String {
    let signed_in = cognito(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": pool.id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": PASSWORD }
        }),
    )
    .await;
    as_str(
        "AdminInitiateAuth",
        &signed_in["AuthenticationResult"],
        "IdToken",
    )
}

/// The claim set of a compact JWS: its second segment, decoded.
fn claims_of(token: &str) -> Value {
    let segment = token
        .split('.')
        .nth(1)
        .unwrap_or_else(|| panic!("a minted token is a compact JWS of three segments: {token:?}"));
    let bytes = ::base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .unwrap_or_else(|e| panic!("a minted token's claim segment must be base64url ({e})"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("a minted token's claim segment must be JSON ({e})"))
}

/// The claim set as the caller reads it back: one entry per claim, a string claim as its own
/// text and every other value as the compact JSON it is.
fn as_published(claims: &Value) -> BTreeMap<String, String> {
    claims
        .as_object()
        .unwrap_or_else(|| panic!("a token's claim set is a JSON object, got {claims}"))
        .iter()
        .map(|(name, value)| {
            let text = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            (name.clone(), text)
        })
        .collect()
}

/// The `claims` member of a report, as a map; `None` where the report carries no such member.
fn reported_claims(reported: &Value) -> Option<BTreeMap<String, String>> {
    let member = reported.get("claims")?;
    Some(
        member
            .as_object()
            .unwrap_or_else(|| panic!("the claims member is a map, got {member}"))
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect(),
    )
}

/// Create one `COGNITO_USER_POOLS` authorizer over `provider_arns`, with `expression` as its
/// `identityValidationExpression` where it declares one, and answer its id — read back through
/// the control plane's own reader, which is where the stored `providerARNs` are asserted.
async fn authorizer(
    router: &MultiServiceRouter,
    api: &str,
    name: &str,
    provider_arns: &[&str],
    expression: Option<&str>,
) -> String {
    let mut body = json!({
        "name": name,
        "type": "COGNITO_USER_POOLS",
        "providerARNs": provider_arns,
        "identitySource": format!("method.request.header.{IDENTITY_HEADER}"),
    });
    if let Some(expression) = expression {
        body["identityValidationExpression"] = json!(expression);
    }
    let created = accepted(
        router,
        name,
        "POST",
        &format!("/restapis/{api}/authorizers"),
        Some(body),
        201,
    )
    .await;
    let id = as_str("CreateAuthorizer", &created, "id");
    let read = accepted(
        router,
        name,
        "GET",
        &format!("/restapis/{api}/authorizers/{id}"),
        None,
        200,
    )
    .await;
    let stored: Vec<String> = read
        .get("providerARNs")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("setup {name:?}: expected the stored providerARNs, got {read}"))
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        stored,
        provider_arns
            .iter()
            .map(|arn| (*arn).to_string())
            .collect::<Vec<String>>(),
        "setup {name:?}: expected GetAuthorizer to report every pool the create named, got {read}"
    );
    assert_eq!(
        read.get("identityValidationExpression").and_then(Value::as_str),
        expression,
        "setup {name:?}: expected the stored validation expression, got {read}"
    );
    id
}

/// Simulate `authorizer` of `api` with `presented` at its identity source.
async fn simulate(
    router: &MultiServiceRouter,
    api: &str,
    authorizer: &str,
    presented: &str,
) -> Answered {
    apigw(
        router,
        "POST",
        &format!("/restapis/{api}/authorizers/{authorizer}"),
        Some(json!({
            "pathWithQueryString": "/orders",
            "headers": { IDENTITY_HEADER: presented },
        })),
    )
    .await
}

/// Which of the two authorizers a row is simulated against.
#[derive(Debug, Clone, Copy)]
enum Which {
    /// The authorizer over both pools that declares no validation expression.
    EveryPool,
    /// The authorizer over the same two pools whose validation expression names ONE app client
    /// of the first of them.
    OneAudience,
}

/// One row: the authorizer, the token the simulated request presents, and what the simulation
/// then owes.
struct Row {
    case: &'static str,
    which: Which,
    /// The token presented at the identity source.
    presented: String,
    client_status: i64,
    /// Whether the report carries that token's claim set, rather than no `claims` member.
    claims_served: bool,
}

/// A simulated user-pool authorizer accepts a token from any pool its `providerARNs` name, and
/// only an audience its `identityValidationExpression` matches.
#[tokio::test]
async fn a_simulated_user_pool_authorizer_accepts_the_pools_and_the_audience_it_names() {
    let router = emulator();

    // Three pools: two the authorizers name, and one they name neither of. The first listed
    // pool carries TWO app clients, so a token of a listed pool can differ in its audience
    // alone.
    let first = pool(&router, "audience-first-pool", "first-user").await;
    let second = pool(&router, "audience-second-pool", "second-user").await;
    let unlisted = pool(&router, "audience-unlisted-pool", "unlisted-user").await;
    let named_client = app_client(&router, &first, "named-client").await;
    let other_client = app_client(&router, &first, "other-client").await;
    let second_client = app_client(&router, &second, "second-client").await;
    let unlisted_client = app_client(&router, &unlisted, "unlisted-client").await;

    let named_token = id_token(&router, &first, &named_client, "first-user").await;
    let other_token = id_token(&router, &first, &other_client, "first-user").await;
    let second_token = id_token(&router, &second, &second_client, "second-user").await;
    let unlisted_token = id_token(&router, &unlisted, &unlisted_client, "unlisted-user").await;

    let api = accepted(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "multi-pool-authorizer-api" })),
        201,
    )
    .await;
    let api = as_str("CreateRestApi", &api, "id");
    let pools = [first.arn.as_str(), second.arn.as_str()];
    let every_pool = authorizer(&router, &api, "over-both-pools", &pools, None).await;
    // The expression AWS matches the `aud` of the incoming token against, anchored on the app
    // client the api admits — which is how an app-client-id audience restriction is written.
    let audience = format!("^{named_client}$");
    let one_audience =
        authorizer(&router, &api, "over-one-audience", &pools, Some(&audience)).await;

    let rows = vec![
        Row {
            case: "a token of the first pool the list names",
            which: Which::EveryPool,
            presented: named_token.clone(),
            client_status: SUCCEEDED,
            claims_served: true,
        },
        Row {
            case: "a token of a later pool the list names",
            which: Which::EveryPool,
            presented: second_token.clone(),
            client_status: SUCCEEDED,
            claims_served: true,
        },
        Row {
            case: "a token of another app client of a listed pool",
            which: Which::EveryPool,
            presented: other_token.clone(),
            client_status: SUCCEEDED,
            claims_served: true,
        },
        Row {
            case: "a token of a pool the list does not name",
            which: Which::EveryPool,
            presented: unlisted_token.clone(),
            client_status: UNAUTHORIZED,
            claims_served: false,
        },
        Row {
            case: "the audience the validation expression names",
            which: Which::OneAudience,
            presented: named_token.clone(),
            client_status: SUCCEEDED,
            claims_served: true,
        },
        Row {
            case: "another app client of the same listed pool",
            which: Which::OneAudience,
            presented: other_token.clone(),
            client_status: UNAUTHORIZED,
            claims_served: false,
        },
        Row {
            case: "an app client of the other listed pool",
            which: Which::OneAudience,
            presented: second_token.clone(),
            client_status: UNAUTHORIZED,
            claims_served: false,
        },
        Row {
            case: "a token of an unlisted pool, under the validation expression",
            which: Which::OneAudience,
            presented: unlisted_token.clone(),
            client_status: UNAUTHORIZED,
            claims_served: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let which = match row.which {
            Which::EveryPool => &every_pool,
            Which::OneAudience => &one_audience,
        };
        let answered = simulate(&router, &api, which, &row.presented).await;
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
        let claims = reported_claims(&reported);
        if row.claims_served {
            let published = as_published(&claims_of(&row.presented));
            assert_eq!(
                claims.as_ref(),
                Some(&published),
                "case {case:?}: expected the accepted token's claim set, got {reported}"
            );
        } else {
            assert!(
                claims.is_none(),
                "case {case:?}: expected no claims member at all, got {reported}"
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
        // A caller's credential never travels into a log: the header NAMES are reported and
        // the token the row presented is not.
        assert!(
            !log.contains(&row.presented),
            "case {case:?}: expected the presented token to stay out of the execution log, got \
             {log:?}"
        );
        assert!(
            reported.get("latency").and_then(Value::as_i64).is_some(),
            "case {case:?}: expected a measured latency, got {reported}"
        );
    }
}
