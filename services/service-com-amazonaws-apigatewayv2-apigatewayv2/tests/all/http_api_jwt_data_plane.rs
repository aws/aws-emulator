// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` HTTP data plane: what a request to a deployed HTTP api stage is
//! answered with, and what a JWT authorizer in front of its route admits.
//!
//! An HTTP api reports `{api-id}.execute-api.{region}.amazonaws.com` as its `ApiEndpoint`,
//! "the stage name … typically appended to this URI to form a complete path to a deployed
//! API stage" (`Api$ApiEndpoint`), so a client of a deployed stage sends an ordinary HTTP
//! request to one of the api's routes. No service claimed such a request — its host names
//! no service and its path is no `/v2/…` route — so the front door answered `404`
//! `UnknownService` for the endpoint this service itself reported, and a client could
//! never observe the most security-sensitive behaviour an HTTP api has: whether the JWT
//! authorizer on a route actually gates traffic.
//!
//! The rule these cases assert, one level above the recorded case (one Cognito-backed JWT
//! authorizer, one `$default` stage, three tokens):
//!
//! > A request to a DEPLOYED stage of an HTTP api this service holds is answered by that
//! > api's own data plane, computed from its stored records: the route whose key most
//! > specifically matches the request's method and path serves it, and a route whose
//! > `AuthorizationType` is `JWT` admits the request only when the token its authorizer's
//! > identity source carries is a signed JWT whose `iss` is that authorizer's configured
//! > `Issuer`, whose `aud` (or `client_id`) matches one configured `Audience` entry and
//! > which is inside its validity window — otherwise `401` `{"message":"Unauthorized"}`
//! > and nothing the route fronts is reached; a token it admits whose scopes meet none of
//! > the route's `AuthorizationScopes` is `403` `{"message":"Forbidden"}`.
//!
//! What an ADMITTED request is answered with is the integration's answer, and every
//! integration type an HTTP api admits names a backend outside this process (`AWS_PROXY`
//! invokes a Lambda function, which this emulator has no runtime for; `HTTP_PROXY`
//! addresses the network, which it never dials). So an admitted request is answered the
//! way API Gateway answers an integration that produced no response — `500`
//! `{"message":"Internal Server Error"}` — and that is what these cases assert for it.
//! The assertion that matters either way is the SPLIT: a refused token is `401`/`403` and
//! never reaches the integration, while an admitted one does.
//!
//! The rows span what the rule generalises over: the form of the route key (a literal
//! path, a single-segment variable, the greedy remainder, the `$default` catch-all),
//! whether the stage is the one that serves the api's root or a named one, how the request
//! addresses the api (its own `execute-api` host, the emulator's own endpoint with the api
//! id in the path, and with the stage alone), the identity source the token travels in (a
//! header, a non-standard header, a query parameter), the shape of the `aud` claim, and
//! the route's authorization type.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api_lifecycle::{as_str, call, emulator};
use crate::websocket_handshake_data_plane::{message, send, Answer};

/// The region the control-plane requests are signed for, and so the region the api's own
/// endpoint host names.
const REGION: &str = "eu-west-1";

/// The issuer of a Cognito user pool, the form `JWTConfiguration$Issuer` documents ("an
/// Amazon Cognito user pool has the following format:
/// https://cognito-idp.{region}.amazonaws.com/{userPoolId}").
const ISSUER: &str = "https://cognito-idp.eu-west-1.amazonaws.com/eu-west-1_abc123def";

/// Another pool's issuer: a token from it is a FOREIGN issuer to the api above.
const FOREIGN_ISSUER: &str = "https://cognito-idp.eu-west-1.amazonaws.com/eu-west-1_other987";

/// The user-pool client id the authorizer records as its audience.
const AUDIENCE: &str = "1example23456789abcdef";

/// Another client's id: a token for it carries a FOREIGN audience.
const FOREIGN_AUDIENCE: &str = "9nobodyconfigured8765";

/// The status and sentence a refused token earns: the authorizer did not accept it, and
/// nothing the route fronts ran.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The status and sentence a token the authorizer accepts earns when the route requires a
/// scope it does not claim ("otherwise, the invocation is not authorized",
/// `Route$AuthorizationScopes`), and the answer for an endpoint that serves nothing.
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status and sentence a request matching no route of the api earns.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The status and sentence an ADMITTED request earns: its route's integration names a
/// backend this emulator cannot call, so the gateway answers as it does for an integration
/// that produced no response. Asserting it is how these cases observe that the
/// authorization step let the request THROUGH to the integration.
const INTEGRATION_REACHED: u16 = 500;
const INTEGRATION_REACHED_MESSAGE: &str = "Internal Server Error";

/// The status an admitted request to a route fronting an `HTTP_PROXY` upstream earns: the
/// caller is sent to the upstream rather than the gateway connecting to it.
const TEMPORARY_REDIRECT: u16 = 307;

/// The error code the front door answers when NO service claims a request — the answer
/// this suite exists to keep off the invoke path.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// Now, in whole seconds since the epoch: the clock a token's `exp` is compared against.
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `value` as one base64url segment of a compact JWS: "base64url without padding"
/// (RFC 7515 §2).
fn segment(value: &Value) -> String {
    aws_smithy_types::base64::encode(value.to_string().as_bytes())
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// A compact JWS over `claims`, in the shape a user pool's token has: a JOSE header naming
/// a signing algorithm and a key, and a non-empty signature segment.
fn token(claims: Value) -> String {
    signed_with(
        json!({ "alg": "ES256", "kid": "e3b0c44298fc1c14", "typ": "JWT" }),
        claims,
    )
}

/// A compact JWS with the JOSE header given, so a case can present an unsecured token.
pub(crate) fn signed_with(header: Value, claims: Value) -> String {
    format!("{}.{}.c2lnbmF0dXJlLWJ5dGVz", segment(&header), segment(&claims))
}

/// An identity token of the api's own pool and client, valid for the next hour.
fn admitted_token(scope: Option<&str>) -> String {
    let mut claims = json!({
        "iss": ISSUER,
        "aud": AUDIENCE,
        "token_use": "id",
        "sub": "11111111-2222-3333-4444-555555555555",
        "iat": now(),
        "exp": now() + 3600
    });
    if let Some(scope) = scope {
        claims["scope"] = json!(scope);
    }
    token(claims)
}

/// What one api of the table was created with, and what it was deployed to.
struct Deployment {
    api_id: String,
    /// The host of the api's own `ApiEndpoint`, read back off `CreateApi` rather than
    /// spelled out here, so a request addresses the endpoint the service reported.
    endpoint_host: String,
    stage_name: String,
}

/// The authorizer, route and stage one row asks for.
struct Setup {
    /// The `CreateAuthorizer` request. `None` for a route that needs no authorizer.
    authorizer: Option<Value>,
    /// The `RouteKey` of the route the request must match.
    route_key: &'static str,
    /// The `AuthorizationType` of that route.
    authorization_type: &'static str,
    /// The route's `AuthorizationScopes`, if any.
    scopes: &'static [&'static str],
    /// The `StageName` the api is deployed to.
    stage_name: &'static str,
}

/// A JWT authorizer of the api's pool, extracting its token from `identity_source`.
fn jwt_authorizer(identity_source: &str, audience: Value) -> Value {
    json!({
        "name": "pool-authorizer",
        "authorizerType": "JWT",
        "identitySource": [identity_source],
        "jwtConfiguration": { "issuer": ISSUER, "audience": audience }
    })
}

/// Create the api, its authorizer, a Lambda-backed `AWS_PROXY` integration, the route that
/// fronts it and an auto-deploying stage; answer what a client needs to address it.
async fn deploy(emulator: &MultiServiceRouter, case: &str, setup: &Setup) -> Deployment {
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "jwt-gated-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let endpoint_host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    let authorizer_id = match &setup.authorizer {
        Some(request) => {
            let (status, authorizer) = call(
                emulator,
                "POST",
                &format!("/v2/apis/{api_id}/authorizers"),
                Some(request.clone()),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateAuthorizer must answer 201, got {status} {authorizer}"
            );
            Some(as_str(&authorizer, "authorizerId"))
        }
        None => None,
    };

    let (status, integration) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": format!(
                "arn:aws:lambda:{REGION}:000000000000:function:jwt-claims-echo"
            ),
            "payloadFormatVersion": "2.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    let mut route = json!({
        "routeKey": setup.route_key,
        "target": format!("integrations/{integration_id}"),
        "authorizationType": setup.authorization_type
    });
    if let Some(authorizer_id) = &authorizer_id {
        route["authorizerId"] = json!(authorizer_id);
    }
    if !setup.scopes.is_empty() {
        route["authorizationScopes"] = json!(setup.scopes);
    }
    let (status, created_route) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(route),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {status} {created_route}"
    );

    let (status, stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": setup.stage_name, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {status} {stage}"
    );
    assert!(
        stage.get("deploymentId").and_then(Value::as_str).is_some(),
        "case {case:?}: an auto-deploying stage must carry a deployment, got {stage}"
    );

    Deployment {
        api_id,
        endpoint_host,
        stage_name: setup.stage_name.to_string(),
    }
}

/// Send one invoke to the api's own endpoint host, carrying the headers given and no AWS
/// credential — which is what a client of a JWT-authorized route sends.
async fn invoke(
    emulator: &MultiServiceRouter,
    deployment: &Deployment,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        emulator,
        &deployment.endpoint_host,
        method,
        path,
        headers,
    )
    .await
}

/// Assert one invoke was answered with the status and `{"message": …}` body expected.
fn answered(case: &str, answer: &Answer, status: u16, body: &str) {
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the invoke URL must be served by this service, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected {status}, got {}",
        answer.summary()
    );
    assert_eq!(
        message(answer),
        body,
        "case {case:?}: expected the {body:?} body, got {}",
        answer.summary()
    );
}

/// One token presented to a gated route, and the status it earns.
struct TokenCase {
    case: &'static str,
    /// The value the identity source carries, or `None` to present nothing at all.
    presented: Option<String>,
    status: u16,
    message: &'static str,
}

/// The token table every gated row is driven with: the legs the recorded checks exercise
/// (a valid pool token, a foreign issuer, a foreign audience, an expired token) plus the
/// forms a client can otherwise present.
fn token_cases(scope: Option<&str>) -> Vec<TokenCase> {
    let claims = |overrides: Value| {
        let mut claims = json!({
            "iss": ISSUER,
            "aud": AUDIENCE,
            "exp": now() + 3600
        });
        if let Some(scope) = scope {
            claims["scope"] = json!(scope);
        }
        for (member, value) in overrides.as_object().expect("the overrides are an object") {
            claims[member] = value.clone();
        }
        token(claims)
    };
    vec![
        TokenCase {
            case: "a valid token of the configured issuer and audience: the integration is \
                   reached",
            presented: Some(admitted_token(scope)),
            status: INTEGRATION_REACHED,
            message: INTEGRATION_REACHED_MESSAGE,
        },
        TokenCase {
            case: "an aud ARRAY, one of whose entries the authorizer records",
            presented: Some(claims(json!({ "aud": [FOREIGN_AUDIENCE, AUDIENCE] }))),
            status: INTEGRATION_REACHED,
            message: INTEGRATION_REACHED_MESSAGE,
        },
        TokenCase {
            case: "an access token, whose client_id is read as its audience",
            presented: Some(claims(
                json!({ "aud": Value::Null, "client_id": AUDIENCE, "token_use": "access" }),
            )),
            status: INTEGRATION_REACHED,
            message: INTEGRATION_REACHED_MESSAGE,
        },
        TokenCase {
            case: "a token from a foreign issuer",
            presented: Some(claims(json!({ "iss": FOREIGN_ISSUER }))),
            status: UNAUTHORIZED,
            message: UNAUTHORIZED_MESSAGE,
        },
        TokenCase {
            case: "a token with a foreign audience",
            presented: Some(claims(json!({ "aud": FOREIGN_AUDIENCE }))),
            status: UNAUTHORIZED,
            message: UNAUTHORIZED_MESSAGE,
        },
        TokenCase {
            case: "an expired token",
            presented: Some(claims(json!({ "exp": now() - 60 }))),
            status: UNAUTHORIZED,
            message: UNAUTHORIZED_MESSAGE,
        },
        TokenCase {
            case: "a token that is not valid yet",
            presented: Some(claims(json!({ "nbf": now() + 600 }))),
            status: UNAUTHORIZED,
            message: UNAUTHORIZED_MESSAGE,
        },
        TokenCase {
            case: "a token carrying no expiry at all",
            presented: Some(claims(json!({ "exp": Value::Null }))),
            status: UNAUTHORIZED,
            message: UNAUTHORIZED_MESSAGE,
        },
        TokenCase {
            case: "an unsecured token (RFC 7519 §6), which nothing signed",
            presented: Some(signed_with(
                json!({ "alg": "none" }),
                json!({ "iss": ISSUER, "aud": AUDIENCE, "exp": now() + 3600 }),
            )),
            status: UNAUTHORIZED,
            message: UNAUTHORIZED_MESSAGE,
        },
        TokenCase {
            case: "a value that is not a token at all",
            presented: Some("letmein".to_string()),
            status: UNAUTHORIZED,
            message: UNAUTHORIZED_MESSAGE,
        },
        TokenCase {
            case: "no token presented",
            presented: None,
            status: UNAUTHORIZED,
            message: UNAUTHORIZED_MESSAGE,
        },
    ]
}

/// One row: how the api is configured, where the token travels, and the request that
/// reaches the gated route.
struct Row {
    case: &'static str,
    setup: Setup,
    /// The identity source the authorizer records, and the header or query the test puts
    /// the token in.
    presentation: Presentation,
    method: &'static str,
    /// The path the request carries, built from the stage name the row deploys.
    path: fn(&Deployment) -> String,
}

/// Where a row's token travels, which is the request part the authorizer's
/// `IdentitySource` names.
enum Presentation {
    /// The `Authorization` header, with the RFC 6750 `Bearer` scheme in front of the token
    /// — the example `Authorizer$IdentitySource` itself gives.
    BearerHeader,
    /// Another header, carrying the token bare.
    Header(&'static str),
    /// A query-string parameter, the other form the same member documents for a JWT
    /// authorizer.
    Query(&'static str),
}

impl Presentation {
    /// The `IdentitySource` entry the authorizer is created with.
    fn identity_source(&self) -> String {
        match self {
            Self::BearerHeader => "$request.header.Authorization".to_string(),
            Self::Header(name) => format!("$request.header.{name}"),
            Self::Query(name) => format!("$request.querystring.{name}"),
        }
    }

    /// The request this presentation turns `path` and `presented` into: the headers to
    /// send, and the path (which carries the token for a query-string source).
    fn request(&self, path: &str, presented: Option<&str>) -> (String, Vec<(String, String)>) {
        match (self, presented) {
            (Self::BearerHeader, Some(token)) => (
                path.to_string(),
                vec![("authorization".to_string(), format!("Bearer {token}"))],
            ),
            (Self::Header(name), Some(token)) => (
                path.to_string(),
                vec![((*name).to_ascii_lowercase(), token.to_string())],
            ),
            (Self::Query(name), Some(token)) => (
                format!("{path}?{name}={token}"),
                Vec::new(),
            ),
            (_, None) => (path.to_string(), Vec::new()),
        }
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a JWT route on the stage that serves the api's root, \
                   its token in the Authorization header",
            setup: Setup {
                authorizer: None,
                route_key: "GET /hello",
                authorization_type: "JWT",
                scopes: &[],
                stage_name: "$default",
            },
            presentation: Presentation::BearerHeader,
            method: "GET",
            // The `$default` stage serves the api's root, so no stage segment is in the
            // path (`Api$ApiEndpoint`: the stage name is "typically" appended).
            path: |_deployment| "/hello".to_string(),
        },
        Row {
            case: "a named stage, whose name the path carries ahead of the route",
            setup: Setup {
                authorizer: None,
                route_key: "POST /orders",
                authorization_type: "JWT",
                scopes: &[],
                stage_name: "prod",
            },
            presentation: Presentation::BearerHeader,
            method: "POST",
            path: |deployment| format!("/{}/orders", deployment.stage_name),
        },
        Row {
            case: "the greedy remainder under the ANY catch-all, with the token in a \
                   non-standard header",
            setup: Setup {
                authorizer: None,
                route_key: "ANY /{proxy+}",
                authorization_type: "JWT",
                scopes: &[],
                stage_name: "live",
            },
            presentation: Presentation::Header("X-Pool-Token"),
            method: "DELETE",
            path: |deployment| format!("/{}/orders/7/items", deployment.stage_name),
        },
        Row {
            case: "a single-segment path variable, with the token in a query parameter",
            setup: Setup {
                authorizer: None,
                route_key: "GET /orders/{orderId}",
                authorization_type: "JWT",
                scopes: &[],
                stage_name: "$default",
            },
            presentation: Presentation::Query("access_token"),
            method: "GET",
            path: |_deployment| "/orders/42".to_string(),
        },
        Row {
            case: "the $default route, which catches what no other route matches",
            setup: Setup {
                authorizer: None,
                route_key: "$default",
                authorization_type: "JWT",
                scopes: &[],
                stage_name: "dev",
            },
            presentation: Presentation::BearerHeader,
            method: "PATCH",
            path: |deployment| format!("/{}/anything/at/all", deployment.stage_name),
        },
    ]
}

#[tokio::test]
async fn a_jwt_route_admits_only_a_token_its_authorizer_accepts() {
    for row in rows() {
        // Every row gets the authorizer its presentation names, so the identity source and
        // the request that carries the token cannot disagree.
        let setup = Setup {
            authorizer: Some(jwt_authorizer(
                &row.presentation.identity_source(),
                json!([AUDIENCE]),
            )),
            ..row.setup
        };
        let emulator = emulator();
        let deployment = deploy(&emulator, row.case, &setup).await;
        let path = (row.path)(&deployment);

        for token_case in token_cases(None) {
            let case = format!("{} / {}", row.case, token_case.case);
            let (path, headers) = row
                .presentation
                .request(&path, token_case.presented.as_deref());
            let headers: Vec<(&str, &str)> = headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            let answer = invoke(&emulator, &deployment, row.method, &path, &headers).await;
            answered(&case, &answer, token_case.status, token_case.message);

            // A data-plane read changes nothing: the same request answers the same way,
            // and the stage is still the one the control plane reported.
            let again = invoke(&emulator, &deployment, row.method, &path, &headers).await;
            answered(&case, &again, token_case.status, token_case.message);
        }

        let (status, stage) = call(
            &emulator,
            "GET",
            &format!(
                "/v2/apis/{}/stages/{}",
                deployment.api_id, deployment.stage_name
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the invokes must leave the stage readable, got {status} {stage}",
            row.case
        );
        assert!(
            stage.get("deploymentId").and_then(Value::as_str).is_some(),
            "case {:?}: the invokes must not take the stage's deployment away, got {stage}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_route_scope_the_token_does_not_claim_is_forbidden_not_unauthorized() {
    // `Route$AuthorizationScopes`: "the method invocation is authorized if any route scope
    // matches a claimed scope in the access token. Otherwise, the invocation is not
    // authorized." The scope clause is reached only by a token the authorizer ACCEPTS, so
    // it is a `403` — the caller is authenticated and merely not authorized here — while
    // every refusal of the token itself stays a `401`.
    let scopes: &[&str] = &["orders/read", "orders/write"];
    let setup = Setup {
        authorizer: Some(jwt_authorizer(
            "$request.header.Authorization",
            json!([AUDIENCE]),
        )),
        route_key: "GET /orders",
        authorization_type: "JWT",
        scopes,
        stage_name: "scoped",
    };
    let emulator = emulator();
    let deployment = deploy(&emulator, "a scoped JWT route", &setup).await;
    let path = format!("/{}/orders", deployment.stage_name);

    let cases: Vec<(&str, Option<String>, u16, &str)> = vec![
        (
            "a token claiming one of the route's scopes",
            Some(admitted_token(Some("openid orders/read profile"))),
            INTEGRATION_REACHED,
            INTEGRATION_REACHED_MESSAGE,
        ),
        (
            "a token claiming the other one",
            Some(admitted_token(Some("orders/write"))),
            INTEGRATION_REACHED,
            INTEGRATION_REACHED_MESSAGE,
        ),
        (
            "a token claiming other scopes only",
            Some(admitted_token(Some("openid profile email"))),
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        ),
        (
            "a token claiming no scope at all",
            Some(admitted_token(None)),
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        ),
        (
            // The scope clause is never reached by a token the gate refuses.
            "an expired token claiming the route's scope",
            Some(token(json!({
                "iss": ISSUER,
                "aud": AUDIENCE,
                "exp": now() - 1,
                "scope": "orders/read"
            }))),
            UNAUTHORIZED,
            UNAUTHORIZED_MESSAGE,
        ),
    ];

    for (case, presented, status, body) in cases {
        let headers: Vec<(&str, &str)> = Vec::new();
        let bearer = presented.map(|token| format!("Bearer {token}"));
        let headers = match &bearer {
            Some(value) => vec![("authorization", value.as_str())],
            None => headers,
        };
        let answer = invoke(&emulator, &deployment, "GET", &path, &headers).await;
        answered(case, &answer, status, body);
    }
}

#[tokio::test]
async fn an_authorizer_with_no_audience_requires_none() {
    // "A list of the intended recipients of the JWT. A valid JWT must provide an aud that
    // matches at least one entry in this list" (`JWTConfiguration$Audience`) — a
    // configuration that lists none records no recipient to require, which the create
    // accepts, so the issuer and the validity window are the whole gate.
    let setup = Setup {
        authorizer: Some(json!({
            "name": "issuer-only-authorizer",
            "authorizerType": "JWT",
            "identitySource": ["$request.header.Authorization"],
            "jwtConfiguration": { "issuer": ISSUER }
        })),
        route_key: "GET /open",
        authorization_type: "JWT",
        scopes: &[],
        stage_name: "$default",
    };
    let emulator = emulator();
    let deployment = deploy(&emulator, "an authorizer naming no audience", &setup).await;

    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "any audience of the configured issuer",
            token(json!({ "iss": ISSUER, "aud": FOREIGN_AUDIENCE, "exp": now() + 60 })),
            INTEGRATION_REACHED,
            INTEGRATION_REACHED_MESSAGE,
        ),
        (
            "a token carrying no audience at all",
            token(json!({ "iss": ISSUER, "exp": now() + 60 })),
            INTEGRATION_REACHED,
            INTEGRATION_REACHED_MESSAGE,
        ),
        (
            "the issuer written with the trailing slash the configuration omits",
            token(json!({ "iss": format!("{ISSUER}/"), "exp": now() + 60 })),
            INTEGRATION_REACHED,
            INTEGRATION_REACHED_MESSAGE,
        ),
        (
            "a foreign issuer, which is still refused",
            token(json!({ "iss": FOREIGN_ISSUER, "exp": now() + 60 })),
            UNAUTHORIZED,
            UNAUTHORIZED_MESSAGE,
        ),
    ];

    for (case, presented, status, body) in cases {
        let bearer = format!("Bearer {presented}");
        let answer = invoke(
            &emulator,
            &deployment,
            "GET",
            "/open",
            &[("authorization", bearer.as_str())],
        )
        .await;
        answered(case, &answer, status, body);
    }
}

#[tokio::test]
async fn an_http_proxy_upstream_sends_the_caller_there_rather_than_connecting_to_it() {
    // An `HTTP_PROXY` integration names "an endpoint over the network"
    // (`Integration$IntegrationUri`), which this emulator opens no connection to. The
    // upstream is a URI the CALLER can reach, so the caller is sent there: `307`, the
    // redirect that preserves the request's method and body (RFC 7231 §6.4.7), carrying
    // the stored `IntegrationUri` in `Location`. Nothing about the upstream is invented,
    // and the route's authorization still runs FIRST — a refused token never learns where
    // the backend is.
    let emulator = emulator();
    let (status, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "upstream-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(status, 201, "CreateApi must answer 201, got {status} {api}");
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let endpoint_host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    let (status, authorizer) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(jwt_authorizer(
            "$request.header.Authorization",
            json!([AUDIENCE]),
        )),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateAuthorizer must answer 201, got {status} {authorizer}"
    );
    let authorizer_id = as_str(&authorizer, "authorizerId");

    let upstream = "https://upstream.example.com/orders";
    let (status, integration) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": upstream,
            "integrationMethod": "ANY",
            "payloadFormatVersion": "1.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateIntegration must answer 201, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    let (status, route) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "ANY /orders",
            "target": format!("integrations/{integration_id}"),
            "authorizationType": "JWT",
            "authorizerId": authorizer_id
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateRoute must answer 201, got {status} {route}");
    let (status, stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default", "autoDeploy": true })),
    )
    .await;
    assert_eq!(status, 201, "CreateStage must answer 201, got {status} {stage}");

    // The authorization step first: a refused token is the authorizer's 401 and learns
    // nothing about the upstream.
    let expired = format!(
        "Bearer {}",
        token(json!({ "iss": ISSUER, "aud": AUDIENCE, "exp": now() - 1 }))
    );
    let refused = send(
        &emulator,
        &endpoint_host,
        "POST",
        "/orders",
        &[("authorization", expired.as_str())],
    )
    .await;
    answered(
        "an expired token on a route fronting an HTTP_PROXY upstream",
        &refused,
        UNAUTHORIZED,
        UNAUTHORIZED_MESSAGE,
    );
    assert_eq!(
        refused.location, None,
        "a refused request must not be told where the upstream is, got {}",
        refused.summary()
    );

    // And an admitted one is sent to the upstream, with its method preserved.
    let admitted = format!("Bearer {}", admitted_token(None));
    for method in ["GET", "POST", "DELETE"] {
        let answer = send(
            &emulator,
            &endpoint_host,
            method,
            "/orders",
            &[("authorization", admitted.as_str())],
        )
        .await;
        assert_ne!(
            answer.error_type.as_deref(),
            Some(UNKNOWN_SERVICE),
            "case {method:?}: the invoke URL must be served by this service, got {}",
            answer.summary()
        );
        assert_eq!(
            answer.status, TEMPORARY_REDIRECT,
            "case {method:?}: expected {TEMPORARY_REDIRECT} to the upstream, got {}",
            answer.summary()
        );
        assert_eq!(
            answer.location.as_deref(),
            Some(upstream),
            "case {method:?}: expected the stored IntegrationUri in Location, got {:?}",
            answer.location
        );
    }
}

#[tokio::test]
async fn an_unauthorized_route_admits_every_request_and_an_iam_route_needs_a_credential() {
    // The other two `AuthorizationType` values an HTTP api's route takes: "NONE for open
    // access … AWS_IAM for using AWS IAM permissions" (`Route$AuthorizationType`). `NONE`
    // reaches the integration with no token at all; `AWS_IAM` needs a SigV4 credential,
    // whose signature the emulator itself verifies, and refuses a request carrying none.
    let emulator = emulator();
    let open = deploy(
        &emulator,
        "an open route",
        &Setup {
            authorizer: None,
            route_key: "GET /open",
            authorization_type: "NONE",
            scopes: &[],
            stage_name: "$default",
        },
    )
    .await;
    let answer = invoke(&emulator, &open, "GET", "/open", &[]).await;
    answered(
        "an open route reached with no token",
        &answer,
        INTEGRATION_REACHED,
        INTEGRATION_REACHED_MESSAGE,
    );

    let signed = deploy(
        &emulator,
        "an AWS_IAM route",
        &Setup {
            authorizer: None,
            route_key: "GET /signed",
            authorization_type: "AWS_IAM",
            scopes: &[],
            stage_name: "$default",
        },
    )
    .await;
    let answer = invoke(&emulator, &signed, "GET", "/signed", &[]).await;
    answered(
        "an AWS_IAM route reached with no credential",
        &answer,
        FORBIDDEN,
        FORBIDDEN_MESSAGE,
    );
    // A bearer token is not an AWS credential: an IAM-authorized route is not reachable
    // with one.
    let bearer = format!("Bearer {}", admitted_token(None));
    let answer = invoke(
        &emulator,
        &signed,
        "GET",
        "/signed",
        &[("authorization", bearer.as_str())],
    )
    .await;
    answered(
        "an AWS_IAM route reached with a bearer token",
        &answer,
        FORBIDDEN,
        FORBIDDEN_MESSAGE,
    );
    let credentialed = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/execute-api/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    );
    let answer = invoke(
        &emulator,
        &signed,
        "GET",
        "/signed",
        &[("authorization", credentialed.as_str())],
    )
    .await;
    answered(
        "an AWS_IAM route reached with a SigV4 credential",
        &answer,
        INTEGRATION_REACHED,
        INTEGRATION_REACHED_MESSAGE,
    );
}

#[tokio::test]
async fn the_stage_that_serves_the_api_root_is_reached_with_no_stage_segment() {
    // The `$default` stage is "the stage that serves an api's root"
    // (`crate::state::DEFAULT_STAGE_NAME`), which is the "typically" of `Api$ApiEndpoint`:
    // the stage name is NOT appended for it, so a client's invoke URL carries the route
    // path alone. Every addressing has to reach it that way — including the rewritten
    // endpoint a client that cannot resolve the api's host uses, where the route path is
    // all that follows the api id.
    let emulator = emulator();
    let deployment = deploy(
        &emulator,
        "an api whose root stage serves it",
        &Setup {
            authorizer: Some(jwt_authorizer(
                "$request.header.Authorization",
                json!([AUDIENCE]),
            )),
            route_key: "GET /hello",
            authorization_type: "JWT",
            scopes: &[],
            stage_name: "$default",
        },
    )
    .await;
    let expired = format!(
        "Bearer {}",
        token(json!({ "iss": ISSUER, "aud": AUDIENCE, "exp": now() - 1 }))
    );
    let admitted = format!("Bearer {}", admitted_token(None));
    let own_endpoint = "127.0.0.1:9297";

    let addressings: Vec<(&str, String, String)> = vec![
        (
            "the api's own host, with no stage segment in the path",
            deployment.endpoint_host.clone(),
            "/hello".to_string(),
        ),
        (
            "the rewritten endpoint, with no stage segment after the api id",
            own_endpoint.to_string(),
            format!("/_aws/execute-api/{}/hello", deployment.api_id),
        ),
        (
            "path-style, with no stage segment after the api id",
            own_endpoint.to_string(),
            format!("/{}/hello", deployment.api_id),
        ),
    ];

    for (case, host, path) in addressings {
        let refused = send(
            &emulator,
            &host,
            "GET",
            &path,
            &[("authorization", expired.as_str())],
        )
        .await;
        answered(case, &refused, UNAUTHORIZED, UNAUTHORIZED_MESSAGE);
        let allowed = send(
            &emulator,
            &host,
            "GET",
            &path,
            &[("authorization", admitted.as_str())],
        )
        .await;
        answered(case, &allowed, INTEGRATION_REACHED, INTEGRATION_REACHED_MESSAGE);
    }

    // The same api also serves a NAMED stage, and the root stage does not swallow its
    // path: the stage segment wins where the api holds a stage of that name.
    let (status, stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{}/stages", deployment.api_id),
        Some(json!({ "stageName": "hello", "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateStage must answer 201 for a stage named like a route segment, got {stage}"
    );
    let shadowed = send(
        &emulator,
        &deployment.endpoint_host,
        "GET",
        "/hello",
        &[("authorization", admitted.as_str())],
    )
    .await;
    answered(
        "a path whose first segment names a stage of the api",
        &shadowed,
        NOT_FOUND,
        NOT_FOUND_MESSAGE,
    );
}

#[tokio::test]
async fn what_the_invoke_endpoint_answers_before_any_route_is_reached() {
    // The steps ahead of the route: the api, the stage, its deployment, and the route
    // match. Each resolves from stored records alone, so each is observable without a
    // token.
    let emulator = emulator();
    let deployment = deploy(
        &emulator,
        "the taxonomy table's api",
        &Setup {
            authorizer: Some(jwt_authorizer(
                "$request.header.Authorization",
                json!([AUDIENCE]),
            )),
            route_key: "GET /hello",
            authorization_type: "JWT",
            scopes: &[],
            stage_name: "prod",
        },
    )
    .await;
    let bearer = format!("Bearer {}", admitted_token(None));
    let with_token: &[(&str, &str)] = &[("authorization", bearer.as_str())];

    let cases: Vec<(&str, Answer, u16, &str)> = vec![
        (
            "a stage of the api that does not exist",
            invoke(&emulator, &deployment, "GET", "/nosuchstage/hello", with_token).await,
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        ),
        (
            "the deployed stage, with a path no route of the api matches",
            invoke(&emulator, &deployment, "GET", "/prod/nowhere", with_token).await,
            NOT_FOUND,
            NOT_FOUND_MESSAGE,
        ),
        (
            "the matching path under a method no route of the api names",
            invoke(&emulator, &deployment, "PUT", "/prod/hello", with_token).await,
            NOT_FOUND,
            NOT_FOUND_MESSAGE,
        ),
        (
            "the stage root, which the route's own path does not match",
            invoke(&emulator, &deployment, "GET", "/prod", with_token).await,
            NOT_FOUND,
            NOT_FOUND_MESSAGE,
        ),
    ];
    for (case, answer, status, body) in cases {
        answered(case, &answer, status, body);
    }

    // A stage with no deployment released to it serves nothing: `403`, the answer an
    // `execute-api` endpoint gives for an api it does not serve.
    let (status, stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{}/stages", deployment.api_id),
        Some(json!({ "stageName": "undeployed" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateStage must answer 201 for a stage asking for no deployment, got {status} {stage}"
    );
    let answer = invoke(&emulator, &deployment, "GET", "/undeployed/hello", with_token).await;
    answered(
        "a stage with no deployment released to it",
        &answer,
        FORBIDDEN,
        FORBIDDEN_MESSAGE,
    );
}

#[tokio::test]
async fn an_api_this_service_does_not_serve_on_that_host_is_not_claimed() {
    // The claim is narrowed to an api this service HOLDS, plus the one address no api of
    // this emulator has at all. Everything else is left to the front door — which, with
    // only this service registered, answers its own `UnknownService`. What is pinned is that
    // this service did not answer, not the front door's wording.
    let emulator = emulator();
    let deployment = deploy(
        &emulator,
        "the unclaimed table's api",
        &Setup {
            authorizer: None,
            route_key: "GET /hello",
            authorization_type: "NONE",
            scopes: &[],
            stage_name: "$default",
        },
    )
    .await;

    // An api whose default endpoint is disabled: "To require that clients use a custom
    // domain name to invoke your API, disable the default endpoint"
    // (`Api$DisableExecuteApiEndpoint`).
    let (status, disabled) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "endpoint-disabled",
            "protocolType": "HTTP",
            "disableExecuteApiEndpoint": true
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateApi must answer 201 for an api that disables its default endpoint, got {disabled}"
    );
    let disabled_id = as_str(&disabled, "apiId");
    let (status, stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{disabled_id}/stages"),
        Some(json!({ "stageName": "$default", "autoDeploy": true })),
    )
    .await;
    assert_eq!(status, 201, "CreateStage must answer 201, got {stage}");
    let disabled_host = format!("{disabled_id}.execute-api.{REGION}.amazonaws.com");

    let unclaimed: Vec<(&str, Answer)> = vec![
        (
            "the @connections path space, which ApiGatewayManagementApi models",
            invoke(
                &emulator,
                &deployment,
                "POST",
                "/@connections/Lx9ZKcvPmjQCJhA=",
                &[],
            )
            .await,
        ),
        (
            "this service's own control-plane path space on the api's invoke host",
            invoke(&emulator, &deployment, "GET", "/v2/apis", &[]).await,
        ),
    ];
    for (case, answer) in unclaimed {
        assert_eq!(
            answer.error_type.as_deref(),
            Some(UNKNOWN_SERVICE),
            "case {case:?}: this data plane must not claim the request, got {}",
            answer.summary()
        );
    }

    // An api that turned its OWN default endpoint off is still the api that endpoint
    // addresses, so the request is claimed and REFUSED there: the endpoint serves nothing of
    // the api while the member is set. (The rule over a table of apis, stages and both
    // protocols is `disabled_execute_api_endpoint.rs`.)
    answered(
        "an api that turned its own default endpoint off",
        &send(&emulator, &disabled_host, "GET", "/hello", &[]).await,
        FORBIDDEN,
        FORBIDDEN_MESSAGE,
    );

    // The one `execute-api` address this data plane DOES answer without holding the api:
    // an endpoint whose api id resolves to no api of this emulator — the address a client
    // keeps calling after the api behind it was deleted — which API Gateway answers
    // `404 {"message": "Not Found"}` rather than leaving to nothing.
    answered(
        "an execute-api host whose api id no api of this emulator has",
        &send(
            &emulator,
            &format!("zzz9999999.execute-api.{REGION}.amazonaws.com"),
            "GET",
            "/hello",
            &[],
        )
        .await,
        NOT_FOUND,
        NOT_FOUND_MESSAGE,
    );
}

#[tokio::test]
async fn a_stage_name_two_apis_hold_names_no_one_api_but_the_api_id_still_does() {
    // The STAGE-ONLY addressing is the only one that carries no api id, so it is the only
    // one that can be ambiguous: when two apis hold a stage of that name, the request
    // names no ONE api and this data plane resolves none of them rather than guessing —
    // the front door then answers that nothing claimed it. Every addressing that NAMES the
    // api keeps working, which is how a client with a colliding stage name reaches its api.
    // (The same collision across SERVICES — a REST api and an HTTP api both holding the
    // name — is the limit documented on `resolve`: two services claim it and the router
    // answers `AmbiguousService`, so the api-id-bearing forms are the ones to use.)
    let emulator = emulator();
    let mut deployments = Vec::new();
    for case in ["the first api", "the second api"] {
        deployments.push(
            deploy(
                &emulator,
                case,
                &Setup {
                    authorizer: None,
                    route_key: "GET /hello",
                    authorization_type: "NONE",
                    scopes: &[],
                    stage_name: "shared",
                },
            )
            .await,
        );
    }

    let stage_only = send(&emulator, "127.0.0.1:9297", "GET", "/shared/hello", &[]).await;
    assert_eq!(
        stage_only.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "a stage name two apis hold names no one api, so nothing may be served for it, got {}",
        stage_only.summary()
    );

    for deployment in &deployments {
        for (case, host, path) in [
            (
                "the api's own host",
                deployment.endpoint_host.clone(),
                "/shared/hello".to_string(),
            ),
            (
                "the api id in the path",
                "127.0.0.1:9297".to_string(),
                format!("/{}/shared/hello", deployment.api_id),
            ),
            (
                "the rewritten endpoint",
                "127.0.0.1:9297".to_string(),
                format!("/_aws/execute-api/{}/shared/hello", deployment.api_id),
            ),
        ] {
            let answer = send(&emulator, &host, "GET", &path, &[]).await;
            answered(case, &answer, INTEGRATION_REACHED, INTEGRATION_REACHED_MESSAGE);
        }
    }
}

#[tokio::test]
async fn every_addressing_of_one_stage_reaches_the_same_route() {
    // A client on the machine this emulator runs on resolves
    // `{api-id}.execute-api.{region}.amazonaws.com` nowhere, so it reaches the emulator's
    // own endpoint and carries in the PATH what it cannot carry in the host: the api id
    // ahead of the stage, or the stage alone. All three addressings must reach the same
    // route and earn the same answer, or a client that cannot resolve the host cannot
    // observe the gate at all.
    let emulator = emulator();
    let deployment = deploy(
        &emulator,
        "the addressing table's api",
        &Setup {
            authorizer: Some(jwt_authorizer(
                "$request.header.Authorization",
                json!([AUDIENCE]),
            )),
            route_key: "GET /hello",
            authorization_type: "JWT",
            scopes: &[],
            stage_name: "prod",
        },
    )
    .await;
    let expired = format!(
        "Bearer {}",
        token(json!({ "iss": ISSUER, "aud": AUDIENCE, "exp": now() - 1 }))
    );
    let admitted = format!("Bearer {}", admitted_token(None));

    // The emulator's own endpoint, which is the only host a client here can reach.
    let own_endpoint = "127.0.0.1:9297";
    let addressings: Vec<(&str, String, String)> = vec![
        (
            "host-style: the api's own execute-api host, as ApiEndpoint reports it",
            deployment.endpoint_host.clone(),
            "/prod/hello".to_string(),
        ),
        (
            "path-style: the api id ahead of the stage on the emulator's own endpoint",
            own_endpoint.to_string(),
            format!("/{}/prod/hello", deployment.api_id),
        ),
        (
            "stage-only: the invoke path alone on the emulator's own endpoint",
            own_endpoint.to_string(),
            "/prod/hello".to_string(),
        ),
        (
            // The form a client that cannot resolve the api's host rewrites the endpoint
            // into: the emulator's own address, with the endpoint it was given carried in
            // the path behind the `_aws` marker.
            "local-endpoint style: the endpoint prefix and the api id in the path",
            own_endpoint.to_string(),
            format!("/_aws/execute-api/{}/prod/hello", deployment.api_id),
        ),
        (
            "local-endpoint style spelled with the whole endpoint host",
            own_endpoint.to_string(),
            format!("/_aws/{}/prod/hello", deployment.endpoint_host),
        ),
        (
            // The marker is read on ANY host, including the api's own.
            "local-endpoint style on the api's own host",
            deployment.endpoint_host.clone(),
            format!("/_aws/execute-api/{}/prod/hello", deployment.api_id),
        ),
    ];

    for (case, host, path) in addressings {
        let refused = send(
            &emulator,
            &host,
            "GET",
            &path,
            &[("authorization", expired.as_str())],
        )
        .await;
        answered(case, &refused, UNAUTHORIZED, UNAUTHORIZED_MESSAGE);
        let allowed = send(
            &emulator,
            &host,
            "GET",
            &path,
            &[("authorization", admitted.as_str())],
        )
        .await;
        answered(case, &allowed, INTEGRATION_REACHED, INTEGRATION_REACHED_MESSAGE);
    }
}
