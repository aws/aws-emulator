// SPDX-License-Identifier: CC0-1.0

//! What a CUSTOMER of this emulator can do with a deployed REST API stage gated by a
//! `COGNITO_USER_POOLS` authorizer: dial the address the emulator PUBLISHES for that stage,
//! and read the refusal a token past its own expiry earns there.
//!
//! Two services and one engine, so this is a router suite: the tokens are minted by the
//! Amazon Cognito user pools service (signing name `cognito-idp`) and the gate that reads them
//! is API Gateway's own `execute-api` data plane (control plane signing name `apigateway`).
//! Every token presented below is one a pool MINTED, or that same pool's own signing key
//! applied to a claim set a row changed — nothing is written here that no pool would sign.
//!
//! The two RULES the rows assert, each one level above the recorded case:
//!
//! > 1. Every invoke address this emulator publishes for a deployed stage is an address the
//! >    publishing client can dial. Where the artifact states the address a client of THAT
//! >    artifact dials — a Swagger 2.0 document's `host` beside the stage's `basePath` — it is
//! >    the endpoint that client REACHED this emulator at, with the api named behind the
//! >    reserved data-plane prefix, and one of AWS's own `execute-api` hosts only for a client
//! >    that addressed one of AWS's own hosts. The ONE templated server URL of an OpenAPI 3.0
//! >    document is the api's own DEFAULT invoke endpoint instead, built from the api's id and
//! >    the Region (`{api-id}.execute-api.{region}.amazonaws.com`, no path ahead of the
//! >    `{basePath}` variable), because that single URL is what a reader parses to recover the
//! >    api and the Region. Either way a request to the published address reaches the method
//! >    the stage deployed, since this data plane claims both addressings.
//! > 2. A request to a `COGNITO_USER_POOLS`-gated method presenting a token that a pool the
//! >    authorizer trusts signed and whose own `exp` has elapsed is refused `401` with the
//! >    sentence AWS words an expired token with, `{"message":"The incoming token has
//! >    expired"}`; the generic `{"message":"Unauthorized"}` sentence is what a request that
//! >    presented NO readable credential earns, so the two classes are distinguishable by the
//! >    message alone.
//!
//! The existing suites of both rules drive the data plane at the api's own
//! `{api-id}.execute-api.{region}.amazonaws.com` host, which no client on the machine this
//! emulator runs on resolves. This one reads the address the emulator HANDS OUT and invokes
//! the gate there, which is the path a customer actually takes, so the two rules are asserted
//! together on the one address.
//!
//! Rule 1's rows span the endpoint the publishing client was configured with (a loopback
//! literal with a port, the same endpoint by its loopback NAME on another port, a loopback
//! literal with no port at all, and one of AWS's own service hosts) and the artifact that
//! names the address (the `servers[].url` of an OpenAPI 3.0 export and the `host`/`basePath`
//! pair of a Swagger 2.0 one). Rule 2's rows span what was presented at the authorizer's
//! identity source (the minted token, the same token under the RFC 6750 bearer scheme,
//! nothing, an empty header, a value that is not a token, and tokens the pool itself signed
//! whose `exp` elapsed a second, an hour and a month ago, whose `nbf` has not arrived, and
//! that carry no `exp` at all).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

// The reading of a compact JWS this test crate already keeps, so nothing here is a second copy
// of base64url decoding or of "the claim set is the second segment".
use crate::cognito_group_role_mapping::claims_of;

/// The Region every call is signed for, and so the Region of the pool's issuer and of the
/// api's own invoke host.
const REGION: &str = "us-west-2";

/// The stage the api is deployed to.
const STAGE: &str = "prod";

/// The resource the gated method sits on, and so the last segment of every invoke below.
const RESOURCE_PATH: &str = "test";

/// The permanent password this suite's fixture user signs in with — a value written for this
/// test and used nowhere else. It satisfies a pool's default password policy.
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// The header the authorizer names as its identity source.
const TOKEN_HEADER: &str = "Authorization";

/// The `401` a request this authorizer does not admit earns, and the sentence a request that
/// presented no readable credential carries.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The sentence a request whose token has expired earns, at that same `401`.
const EXPIRED_MESSAGE: &str = "The incoming token has expired";

/// The sentence the `API_CONFIGURATION_ERROR` / `AUTHORIZER_CONFIGURATION_ERROR` gateway
/// responses carry: the `500` that must never answer a question about a token.
const CONFIGURATION_ERROR: &str = "Internal server error";

/// The scheme every published invoke address carries, which this emulator serves.
const INVOKE_SCHEME: &str = "https://";

/// The suffix of AWS's own hostnames, which a client of this emulator resolves nowhere.
const AWS_HOST_SUFFIX: &str = "amazonaws.com";

/// The body the `MOCK` integration behind the gated resource answers: the claims of the token
/// the authorizer accepted, which is how a row observes that the gate let the request THROUGH
/// and that the integration could read the signed-in user.
const CLAIMS_TEMPLATE: &str =
    "{\"sub\":\"$context.authorizer.claims.sub\",\"use\":\"$context.authorizer.claims.token_use\"}";

/// One request dispatched through the router, and what came back.
struct Answer {
    status: u16,
    body: Value,
    /// The body as it arrived, for a response that is not JSON at all.
    text: String,
}

impl Answer {
    /// One string member of a body, or `"<no member>"`.
    fn member(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("<no member>")
            .to_string()
    }

    fn summary(&self) -> String {
        format!("{} {}", self.status, self.text)
    }
}

/// API Gateway and the Amazon Cognito user pools service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the pool a token comes from is the pool this api's
/// authorizer reads its key set out of. The engine is answered too, because the pool's own
/// signing key is what the rows needing a token NO sign-in produces are signed with.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification belongs to the emulator, not to this check.
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes a
/// control-plane request. The signature itself is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request with the method, host, path, headers and body given.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
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
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer { status, body, text }
}

/// One `awsJson1_1` call to the Amazon Cognito user pools service, which must succeed.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let target = format!("AWSCognitoIdentityProviderService.{operation}");
    let credential = authorization("cognito-idp");
    let answer = dispatch(
        router,
        "POST",
        &format!("cognito-idp.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &target),
            ("authorization", &credential),
        ],
        Bytes::from(body.to_string()),
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

/// One API Gateway control-plane call, addressed at `endpoint` — the host the client is
/// configured with — which has to answer `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    endpoint: &str,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Answer {
    let credential = authorization("apigateway");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answer = dispatch(
        router,
        method,
        endpoint,
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &credential),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answer.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path} at {endpoint}, got {}",
        answer.summary()
    );
    answer
}

/// A required non-empty string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{what} must carry a non-empty {member:?}, got {body}"))
}

/// Seconds since the epoch, which is what a token's `exp`/`nbf` are (RFC 7519 §2
/// NumericDate).
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch second fits an i64")
}

/// `claims` with `members` applied — the one claim a row changes, and the claims it removes.
fn with_claims(claims: &Value, members: &[(&str, Option<Value>)]) -> Value {
    let mut changed = claims.clone();
    let object = changed
        .as_object_mut()
        .expect("a token's claim set is a JSON object");
    for (name, value) in members {
        match value {
            Some(value) => {
                object.insert((*name).to_string(), value.clone());
            }
            None => {
                object.remove(*name);
            }
        }
    }
    changed
}

/// `claims` as a token the POOL ITSELF signs, with the key it signs its identity tokens with.
///
/// This is how a row presents a token no sign-in produces — one whose `exp` has passed, whose
/// `nbf` has not arrived, or that carries no `exp` — without writing a signature by hand: the
/// bytes are signed by the same key `AdminInitiateAuth` signs with, so the only thing under
/// test is the claim the row changed.
fn signed_by_pool(engine: &Arc<engine::InMemoryEngine>, pool_id: &str, claims: &Value) -> String {
    let pools = cognito_idp::state::service_state(&**engine)
        .expect("the user pool service's state lives in the shared engine");
    let held = pools.lock();
    let pool = held
        .pool(pool_id)
        .unwrap_or_else(|| panic!("the pool {pool_id:?} created above must be held"));
    pool.signing_keys()
        .for_use(cognito_idp::state::SigningKeyUse::IdToken)
        .signed_compact_jws(claims)
        .expect("the pool's own key signs the token bytes")
}

/// What one pool's sign-in produced: the pool's id and ARN, its app client id and the identity
/// token of the sign-in.
struct Pool {
    id: String,
    arn: String,
    client_id: String,
    id_token: String,
}

/// Mint one pool, one app client and one confirmed user, and sign that user in.
async fn pool_with_signed_in_user(
    router: &MultiServiceRouter,
    pool_name: &str,
    username: &str,
) -> Pool {
    let created = cognito(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let id = as_str("CreateUserPool", &created["UserPool"], "Id");
    let arn = as_str("CreateUserPool", &created["UserPool"], "Arn");

    let client = cognito(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": id,
            "ClientName": format!("{pool_name}-client"),
            // The flow the sign-in below uses, plus the refresh flow AWS requires beside any
            // other `ALLOW_` flow.
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]
        }),
    )
    .await;
    let client_id = as_str("CreateUserPoolClient", &client["UserPoolClient"], "ClientId");

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
            "Password": GOOD_PASSWORD,
            "Permanent": true
        }),
    )
    .await;
    let signed_in = cognito(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD }
        }),
    )
    .await;
    Pool {
        id,
        arn,
        client_id,
        id_token: as_str(
            "AdminInitiateAuth",
            &signed_in["AuthenticationResult"],
            "IdToken",
        ),
    }
}

/// One REST api whose [`RESOURCE_PATH`] resource carries a `COGNITO_USER_POOLS`-gated `MOCK`
/// `GET`, deployed to [`STAGE`]. Answers the api id.
///
/// Every call is made at `endpoint`, the host the developer's client is configured with.
async fn gated_stage(
    router: &MultiServiceRouter,
    endpoint: &str,
    api_name: &str,
    pool_arn: &str,
) -> String {
    let api = apigw(
        router,
        endpoint,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": api_name })),
        201,
    )
    .await
    .body;
    let api_id = as_str("CreateRestApi", &api, "id");
    let root = as_str("CreateRestApi", &api, "rootResourceId");

    let authorizer = apigw(
        router,
        endpoint,
        "CreateAuthorizer",
        "POST",
        &format!("/restapis/{api_id}/authorizers"),
        Some(json!({
            "name": "pool-gate",
            "type": "COGNITO_USER_POOLS",
            "providerARNs": [pool_arn],
            "identitySource": format!("method.request.header.{TOKEN_HEADER}")
        })),
        201,
    )
    .await
    .body;
    let authorizer_id = as_str("CreateAuthorizer", &authorizer, "id");

    let resource = apigw(
        router,
        endpoint,
        "CreateResource",
        "POST",
        &format!("/restapis/{api_id}/resources/{root}"),
        Some(json!({ "pathPart": RESOURCE_PATH })),
        201,
    )
    .await
    .body;
    let resource_id = as_str("CreateResource", &resource, "id");

    let method_base = format!("/restapis/{api_id}/resources/{resource_id}/methods/GET");
    let calls: &[(&str, String, Value)] = &[
        (
            "PutMethod",
            method_base.clone(),
            json!({
                "authorizationType": "COGNITO_USER_POOLS",
                "authorizerId": authorizer_id
            }),
        ),
        (
            "PutIntegration",
            format!("{method_base}/integration"),
            json!({ "type": "MOCK" }),
        ),
        (
            "PutMethodResponse",
            format!("{method_base}/responses/200"),
            json!({}),
        ),
        (
            "PutIntegrationResponse",
            format!("{method_base}/integration/responses/200"),
            json!({ "responseTemplates": { "application/json": CLAIMS_TEMPLATE } }),
        ),
    ];
    for (case, path, body) in calls {
        apigw(
            router,
            endpoint,
            case,
            "PUT",
            path,
            Some(body.clone()),
            201,
        )
        .await;
    }

    apigw(
        router,
        endpoint,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api_id}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;
    api_id
}

/// The invoke address of [`STAGE`] as one artifact names it: the base URL a client built from
/// that artifact calls, with the stage already in it.
struct PublishedAddress {
    /// Which artifact named it, for the assertion messages.
    artifact: &'static str,
    /// The base URL, `{scheme}://{host}{path}`, with no trailing slash.
    base_url: String,
    /// Whether this artifact states the address a client of IT dials — the endpoint the
    /// reading client reached — rather than a host of AWS's own that a client of this
    /// emulator resolves nowhere.
    names_the_reached_endpoint: bool,
}

/// The export document of the stage in `dialect`, as the JSON file a caller downloads, read
/// at `endpoint`.
async fn export(
    router: &MultiServiceRouter,
    endpoint: &str,
    api_id: &str,
    dialect: &str,
) -> Value {
    apigw(
        router,
        endpoint,
        &format!("GetExport({dialect})"),
        "GET",
        &format!("/restapis/{api_id}/stages/{STAGE}/exports/{dialect}"),
        None,
        200,
    )
    .await
    .body
}

/// Every address the stage's downloadable artifacts publish, read at `endpoint`.
///
/// A 3.0 document carries ONE server URL whose `{basePath}` variable stands where the stage
/// goes, and a 2.0 document the `host` a client dials beside the stage's own `basePath`
/// ("`{host}{basePath}` is the address a client of this document calls"), so resolving each
/// gives the same kind of value: the base URL of the deployed stage. What each is built on
/// differs, which is what the flag below records: the 2.0 `host` is the address this emulator
/// announces for the stage, so it names the endpoint the reading client reached, while the one
/// server URL of a 3.0 document states the api's OWN default invoke endpoint — the api id as
/// the leading DNS label and no path ahead of the `{basePath}` variable — whatever endpoint
/// the export was asked at. This data plane claims both addressings, so both are served.
async fn published_addresses(
    router: &MultiServiceRouter,
    endpoint: &str,
    api_id: &str,
) -> Vec<PublishedAddress> {
    let oas30 = export(router, endpoint, api_id, "oas30").await;
    let server = &oas30["servers"][0];
    let templated = as_str("the oas30 export's server", server, "url");
    let base_path = as_str(
        "the oas30 export's basePath variable",
        &server["variables"]["basePath"],
        "default",
    );
    assert_eq!(
        base_path, STAGE,
        "the oas30 export must carry the stage as its basePath variable's default, got {oas30}"
    );

    let swagger = export(router, endpoint, api_id, "swagger").await;
    let host = as_str("the swagger export", &swagger, "host");
    let swagger_base = as_str("the swagger export", &swagger, "basePath");
    assert_eq!(
        swagger_base,
        format!("/{STAGE}"),
        "the swagger export must carry the stage as its basePath, got {swagger}"
    );

    vec![
        PublishedAddress {
            artifact: "the oas30 export's server URL",
            base_url: templated.replace("{basePath}", &base_path),
            names_the_reached_endpoint: false,
        },
        PublishedAddress {
            artifact: "the swagger export's host and basePath",
            base_url: format!("{INVOKE_SCHEME}{host}{swagger_base}"),
            names_the_reached_endpoint: true,
        },
    ]
}

/// One request to the gated resource at a published base URL, presenting `token` at the
/// authorizer's identity source — or nothing at all when it is `None`.
async fn invoke(router: &MultiServiceRouter, base_url: &str, token: Option<&str>) -> Answer {
    let url = format!("{base_url}/{RESOURCE_PATH}");
    let authority = url
        .strip_prefix(INVOKE_SCHEME)
        .unwrap_or_else(|| panic!("a published address must be an https URL, got {url:?}"));
    let (host, path) = match authority.find('/') {
        Some(cut) => (&authority[..cut], &authority[cut..]),
        None => (authority, "/"),
    };
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(token) = token {
        headers.push((TOKEN_HEADER, token));
    }
    dispatch(router, "GET", host, path, &headers, Bytes::new()).await
}

/// Assert `answer` is the gate admitting the request and the integration answering it with the
/// claims of the token presented.
fn assert_served(case: &str, answer: &Answer, sub: &str) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the integration's 200, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.member("sub"),
        sub,
        "case {case:?}: expected the token's sub {sub:?} to reach the integration, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.member("use"),
        "id",
        "case {case:?}: expected the token's token_use to reach the integration, got {}",
        answer.summary()
    );
}

/// Where a publishing client was configured to reach this emulator, and the address the
/// artifacts of its stage must then name.
struct Endpoint {
    case: &'static str,
    /// The `Host` the control-plane request carries.
    host: &'static str,
    /// Whether that host is the emulator's own endpoint — a client that resolves nothing else
    /// — or one of AWS's own service hosts.
    own_endpoint: bool,
}

#[tokio::test]
async fn a_gated_stage_is_published_at_an_address_this_emulator_serves() {
    let (router, _engine) = emulator();
    let pool = pool_with_signed_in_user(&router, "published-address-pool", "gate-user").await;
    let claims = claims_of(&pool.id_token);
    let sub = as_str("the pool's identity token", &claims, "sub");
    assert_eq!(
        claims.get("aud").and_then(Value::as_str),
        Some(pool.client_id.as_str()),
        "the pool's identity token must name the client it was minted for, got {claims}"
    );
    // The api is built once: the address an artifact names is a function of the request that
    // ASKS for the artifact, so one deployed stage is read at every endpoint below.
    let api_id = gated_stage(
        &router,
        &format!("apigateway.{REGION}.{AWS_HOST_SUFFIX}"),
        "published-address-api",
        &pool.arn,
    )
    .await;

    // Rows differ along the endpoint the reading client is configured with: each spelling of
    // the emulator's own endpoint a client points at, and one of AWS's own hosts.
    let endpoints: &[Endpoint] = &[
        Endpoint {
            case: "the emulator's own loopback endpoint on a port",
            host: "127.0.0.1:9297",
            own_endpoint: true,
        },
        Endpoint {
            case: "the same endpoint by its loopback NAME on another port",
            host: "localhost:39461",
            own_endpoint: true,
        },
        Endpoint {
            case: "a loopback endpoint carrying no port at all",
            host: "127.0.0.1",
            own_endpoint: true,
        },
        Endpoint {
            case: "a client that resolves AWS's own hosts",
            host: "apigateway.us-west-2.amazonaws.com",
            own_endpoint: false,
        },
    ];

    for endpoint in endpoints {
        let addresses = published_addresses(&router, endpoint.host, &api_id).await;
        for published in &addresses {
            let case = format!("{} via {}", endpoint.case, published.artifact);
            // 1. The address names an authority the reading client can dial: the endpoint it
            //    reached, where the artifact states that, and otherwise the api's own default
            //    invoke endpoint — which this data plane claims host-style.
            let own_default = format!("{api_id}.execute-api.{REGION}.{AWS_HOST_SUFFIX}");            let expected_authority = if endpoint.own_endpoint && published.names_the_reached_endpoint
            {
                endpoint.host.to_string()
            } else {
                own_default
            };
            let authority = published
                .base_url
                .strip_prefix(INVOKE_SCHEME)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default();
            assert_eq!(
                authority, expected_authority,
                "case {case:?}: expected the published address to name {expected_authority:?}, \
                 got {:?}",
                published.base_url
            );
            if endpoint.own_endpoint && published.names_the_reached_endpoint {
                assert!(
                    !authority.ends_with(AWS_HOST_SUFFIX),
                    "case {case:?}: a client that reached the emulator's own endpoint must not \
                     be published an AWS host it resolves nowhere, got {:?}",
                    published.base_url
                );
            }
            // 2. And the address SERVES the gated method: the pool's token is admitted there
            //    and the integration behind it answers the signed-in user's claims.
            let answer = invoke(&router, &published.base_url, Some(&pool.id_token)).await;
            assert_served(&case, &answer, &sub);
            // Nothing else is reachable at a published address by presenting no credential:
            // the same address refuses an unauthenticated call rather than serving it.
            let refused = invoke(&router, &published.base_url, None).await;
            assert_eq!(
                (refused.status, refused.member("message")),
                (UNAUTHORIZED, UNAUTHORIZED_MESSAGE.to_string()),
                "case {case:?}: expected {UNAUTHORIZED} {UNAUTHORIZED_MESSAGE:?} with no token \
                 presented, got {}",
                refused.summary()
            );
        }
    }
}

/// One request to the published address: what it presents at the authorizer's identity source
/// and the sentence the gate owes it.
struct Row {
    case: String,
    /// The value the `Authorization` header carries, or `None` to present nothing at all.
    presented: Option<String>,
    /// The sentence the `401` must carry, or `None` for a row the gate admits.
    refusal: Option<&'static str>,
}

#[tokio::test]
async fn an_expired_pool_token_is_refused_in_its_own_sentence_at_the_published_address() {
    let (router, engine) = emulator();
    let pool = pool_with_signed_in_user(&router, "expired-token-pool", "gate-user").await;
    let claims = claims_of(&pool.id_token);
    let sub = as_str("the pool's identity token", &claims, "sub");
    // The address a customer of this emulator is handed, which is where every row below is
    // sent: the endpoint the reading client reached, never one of AWS's own hosts.
    let endpoint = "127.0.0.1:9297";
    let api_id = gated_stage(&router, endpoint, "expired-token-api", &pool.arn).await;
    let published = published_addresses(&router, endpoint, &api_id).await;
    let base_url = published
        .iter()
        .find(|address| address.names_the_reached_endpoint)
        .expect("an artifact of the stage publishes the address the reading client reached")
        .base_url
        .clone();
    assert!(
        !base_url.contains(AWS_HOST_SUFFIX),
        "the published address must be one this emulator serves, got {base_url:?}"
    );

    // The tokens no sign-in produces, each signed by the pool's OWN key over a claim set that
    // differs from the minted one in exactly what the row is about. The offsets span the
    // elapsed-expiry class from the narrowest reading of it to a token long dead.
    let now = now_seconds();
    let expired = |ago: i64| {
        signed_by_pool(
            &engine,
            &pool.id,
            &with_claims(&claims, &[("exp", Some(json!(now - ago)))]),
        )
    };
    let rows: Vec<Row> = vec![
        Row {
            case: "the pool's identity token, bare".to_string(),
            presented: Some(pool.id_token.clone()),
            refusal: None,
        },
        Row {
            case: "the same token under the RFC 6750 bearer scheme".to_string(),
            presented: Some(format!("Bearer {}", pool.id_token)),
            refusal: None,
        },
        Row {
            case: "no token at all".to_string(),
            presented: None,
            refusal: Some(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "an empty token header".to_string(),
            presented: Some(String::new()),
            refusal: Some(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a value that is not a token at all".to_string(),
            presented: Some("not-a-token".to_string()),
            refusal: Some(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a token the pool signed whose exp passed a second ago".to_string(),
            presented: Some(expired(1)),
            refusal: Some(EXPIRED_MESSAGE),
        },
        Row {
            case: "a token the pool signed whose exp passed an hour ago".to_string(),
            presented: Some(expired(3_600)),
            refusal: Some(EXPIRED_MESSAGE),
        },
        Row {
            case: "a token the pool signed whose exp passed a month ago".to_string(),
            presented: Some(expired(30 * 24 * 3_600)),
            refusal: Some(EXPIRED_MESSAGE),
        },
        Row {
            case: "a token the pool signed whose window has not begun".to_string(),
            presented: Some(signed_by_pool(
                &engine,
                &pool.id,
                &with_claims(
                    &claims,
                    &[
                        ("exp", Some(json!(now + 3_600))),
                        ("nbf", Some(json!(now + 600))),
                    ],
                ),
            )),
            refusal: Some(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a token the pool signed that names no expiry".to_string(),
            presented: Some(signed_by_pool(
                &engine,
                &pool.id,
                &with_claims(&claims, &[("exp", None)]),
            )),
            refusal: Some(UNAUTHORIZED_MESSAGE),
        },
    ];

    for row in &rows {
        let answer = invoke(&router, &base_url, row.presented.as_deref()).await;
        // A token question is never answered by the 500 of an authorizer whose decision could
        // not be obtained: the distinction between "fix the token" and "the authorizer is
        // broken" is what the two response classes are for.
        assert_ne!(
            answer.member("message"),
            CONFIGURATION_ERROR,
            "case {:?}: the authorizer must decide the call, not report a configuration error; \
             got {}",
            row.case,
            answer.summary()
        );
        match row.refusal {
            Some(sentence) => {
                assert_eq!(
                    (answer.status, answer.member("message")),
                    (UNAUTHORIZED, sentence.to_string()),
                    "case {:?}: expected {UNAUTHORIZED} {sentence:?}, got {}",
                    row.case,
                    answer.summary()
                );
            }
            None => assert_served(&row.case, &answer, &sub),
        }
    }
}
