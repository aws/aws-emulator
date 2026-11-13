// SPDX-License-Identifier: CC0-1.0

//! What a JWT authorizer's REFUSAL does to the route it fronts: nothing at all beyond the
//! one request it refused.
//!
//! The rule these cases assert, one level above the recorded case (one pool-issued token a
//! `JWTConfiguration` does not admit, followed by one it does):
//!
//! > A JWT authorizer decides each request on its own: the request it cannot admit is
//! > answered by the gateway itself — `401` `{"message":"Unauthorized"}` for a token the
//! > gate cannot accept and `403` `{"message":"Forbidden"}` for one it accepts whose scopes
//! > meet none of the route's `AuthorizationScopes` — and the route's integration is NOT
//! > invoked for it, while the route, its authorizer and its integration dispatch are left
//! > exactly as they were: the next request presenting a token whose `iss` is the recorded
//! > `Issuer` and whose audience matches a recorded `Audience` entry is forwarded to the
//! > integration, which runs, however many refusals came before it.
//!
//! The rows span what the rule generalises over: WHICH clause refused the token (a foreign
//! issuer, a foreign audience, an absent token, a value that is no token, an unsecured
//! token, a validity window that has passed, one that has not opened, and the route-scope
//! clause that is a `403` rather than a `401`), and how many refusals precede the request
//! that must still be served (one, and a whole run of them).
//!
//! Nothing asserted here is a value this emulator filled in for the assertion's sake. The
//! token the authorizer admits is one a user pool really minted for an app client of it, so
//! Amazon Cognito user pools is mounted beside this service; whether the INTEGRATION ran is
//! read from the line the function itself wrote, delivered to its own log group, so AWS
//! Lambda and CloudWatch Logs are mounted beside it too, all four on one engine. A refused
//! request leaves no such line, and a served one does.
//!
//! `http_api_jwt_data_plane` owns which token earns which status and
//! `http_api_jwt_claims_reach_the_integration` owns the shape of what an admitted token
//! hands on; this suite owns the CONFINEMENT of a refusal to the request it refused.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as user_pools;

use crate::api_lifecycle::{as_str, call_at_host};
// The three-service mount (this service, AWS Lambda and CloudWatch Logs on one engine) and
// the Logs wire this tree already states, rather than a second copy of either.
use crate::http_api_access_logging::{delivered_to, emulator as emulator_with_logs};
use crate::http_api_backend_dispatch::{given_function, lambda_integration};
// The token writer and the clock this tree already states for a JWT case: one copy of "a
// compact JWS with the JOSE header given" and of "now, in whole seconds since the epoch".
use crate::http_api_jwt_data_plane::{now, signed_with};
use crate::http_api_jwt_scope_gate::{
    claims_of, cognito, cognito_host, granted_token, m2m_client, resource_server_scopes, scope,
    AUDIT, READ, RESOURCE_SERVER,
};
use crate::websocket_handshake_data_plane::{message, send, Answer};

/// The Region every request below is signed for: the Region of the pool whose issuer the
/// authorizer records, of the function the integration names, and of the log group that
/// function writes to.
const REGION: &str = "us-east-1";

/// The identity source the authorizer extracts the token from: "for HTTP APIs, use selection
/// expressions prefixed with $, for example, $request.header.Auth"
/// (`Authorizer$IdentitySource`).
const IDENTITY_SOURCE: &str = "$request.header.Authorization";

/// The status and sentence a token the authorizer cannot accept earns, and the ones a token
/// it accepts earns at a route requiring a scope that token does not claim.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The route that fronts the integration with no scope requirement, and the one that
/// requires a scope no token of this suite claims.
const OPEN_ROUTE: &str = "GET /items";
const OPEN_PATH: &str = "/items";
const SCOPED_ROUTE: &str = "GET /admin";
const SCOPED_PATH: &str = "/admin";

/// The function behind both routes, and the log group its invocations are delivered to —
/// "By default, Lambda functions send logs to a default log group named
/// `/aws/lambda/<function name>`" (`LoggingConfig$LogGroup`).
const FUNCTION: &str = "jwt-refusal-per-request-backend";
fn log_group() -> String {
    format!("/aws/lambda/{FUNCTION}")
}

/// What the handler prefixes the marker of the request it was handed with, so one line in
/// the function's own log says WHICH request reached it.
const HIT: &str = "HIT";

/// The handler behind the route: it writes one line naming the request's marker and answers
/// with that marker and everything the authorizer told it about the caller — the token's own
/// claims and the scopes it claims. A request that never reaches it leaves no such line.
const MARKER_HANDLER: &str = "import json\n\
     def lambda_handler(event, context):\n\
     \x20   marker = (event.get('queryStringParameters') or {}).get('marker', 'none')\n\
     \x20   print('HIT ' + marker)\n\
     \x20   decided = event['requestContext']['authorizer']['jwt']\n\
     \x20   return {'statusCode': 200, 'headers': {'content-type': 'application/json'},\n\
     \x20            'body': json.dumps({'served': marker,\n\
     \x20                                'claims': decided['claims'],\n\
     \x20                                'scopes': decided['scopes']})}\n";

/// How long a wait for a CONDITION may take before a case gives up, and how long it waits
/// between two readings: the deadline is far beyond any span this emulator owes, so a loaded
/// host reaches the same outcome as an idle one. No case asserts on elapsed time.
const CONDITION_DEADLINE: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// This service, AWS Lambda and CloudWatch Logs on ONE engine (the mount
/// [`crate::http_api_access_logging`] states), with Amazon Cognito user pools beside them:
/// the token the authorizer admits is minted by that service.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator_with_logs();
    router.register_service(user_pools::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One control-plane call against this service, signed for [`REGION`].
async fn control(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_at_host(
        emulator,
        &format!("apigateway.{REGION}.amazonaws.com"),
        REGION,
        method,
        uri,
        body,
    )
    .await
}

/// Everything one case drives its requests against: the host the api announced, the token
/// the authorizer admits and the client that token names.
struct Deployment {
    host: String,
    /// The access token a user pool minted for the app client the authorizer's `Audience`
    /// lists.
    admitted: String,
    /// That app client's own id, which the admitted token's `client_id` claim names.
    client_id: String,
    /// The issuer the authorizer records, which is the pool's own.
    issuer: String,
}

/// The pool, its app clients, the function and the api every case below drives: a JWT
/// authorizer of that pool and that client, a Lambda proxy integration on
/// [`MARKER_HANDLER`], the two routes above behind the authorizer, and the stage that serves
/// the api's root.
///
/// Answers the deployment, plus the token minted for ANOTHER app client of the same pool —
/// the configured issuer with an audience the authorizer does not list.
async fn given_deployment(
    emulator: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
) -> (Deployment, String) {
    let pool = cognito(
        emulator,
        "CreateUserPool",
        json!({ "PoolName": "jwt-refusal-pool" }),
    )
    .await;
    let pool_id = pool["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateUserPool must mint an Id, got {pool}"))
        .to_string();
    let defined: Vec<Value> = resource_server_scopes();
    cognito(
        emulator,
        "CreateResourceServer",
        json!({
            "UserPoolId": pool_id,
            "Identifier": RESOURCE_SERVER,
            "Name": "the orders api",
            "Scopes": defined,
        }),
    )
    .await;
    let client_id = m2m_client(emulator, &pool_id, "the-apis-client").await;
    let other_client_id = m2m_client(emulator, &pool_id, "another-client").await;
    // The token the gate admits, and one minted for an app client the `Audience` does not
    // list. Both carry the pool's own `iss`, so what tells them apart is the audience clause.
    let admitted = granted_token(emulator, &client_id, &[scope(READ)]).await;
    let foreign_audience = granted_token(emulator, &other_client_id, &[scope(READ)]).await;

    // "An Amazon Cognito user pool has the following format:
    // https://cognito-idp.{region}.amazonaws.com/{userPoolId}" (`JWTConfiguration$Issuer`):
    // the issuer the authorizer records is the one the minted token names.
    let issuer = format!("https://{}/{pool_id}", cognito_host());
    let claims = claims_of(&admitted);
    assert_eq!(
        claims.get("iss").and_then(Value::as_str),
        Some(issuer.as_str()),
        "case {case:?}: the minted token must name the pool's documented issuer, got {claims}"
    );

    let function = given_function(emulator, engine, case, FUNCTION, MARKER_HANDLER).await;
    let (status, api) = control(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "jwt-refusal-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {api}"
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
            "jwtConfiguration": { "issuer": issuer, "audience": [client_id] }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateAuthorizer must answer 201, got {status} {authorizer}"
    );
    let authorizer_id = as_str(&authorizer, "authorizerId");

    let (status, integration) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(lambda_integration(&function, "2.0")),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    for (route_key, scopes) in [
        (OPEN_ROUTE, Vec::new()),
        (SCOPED_ROUTE, vec![scope(AUDIT)]),
    ] {
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
            "case {case:?}: CreateRoute({route_key}) must answer 201, got {status} {created}"
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
        "case {case:?}: CreateStage must answer 201, got {status} {stage}"
    );

    (
        Deployment {
            host,
            admitted,
            client_id,
            issuer,
        },
        foreign_audience,
    )
}

/// The claim set of a token of the configured issuer and audience, with the window members
/// the row names.
fn pool_shaped_claims(deployment: &Deployment, expiry: i64, not_before: Option<i64>) -> Value {
    let mut claims = json!({
        "iss": deployment.issuer,
        "client_id": deployment.client_id,
        "token_use": "access",
        "sub": "e8bb4a84-2b26-4a66-9a2e-6a5ad0e4e0a4",
        "scope": scope(READ),
        "iat": now() - 10,
        "exp": expiry,
    });
    if let Some(not_before) = not_before {
        claims["nbf"] = json!(not_before);
    }
    claims
}

/// One refused request: the path it is sent to, what its `Authorization` header carried, and
/// the refusal it earns.
struct Refusal {
    case: &'static str,
    path: &'static str,
    /// The header value presented, or `None` to present nothing at all.
    presented: Option<String>,
    status: u16,
    sentence: &'static str,
}

/// Every refusal clause, over one deployment: a token of another pool, one of another app
/// client of the same pool, no token, a value that is no token, an unsecured token, a window
/// that has passed, one that has not opened, and the route-scope clause.
fn refusals(deployment: &Deployment, foreign_issuer: &str, foreign_audience: &str) -> Vec<Refusal> {
    vec![
        Refusal {
            case: "a token minted by ANOTHER user pool: a foreign issuer and audience",
            path: OPEN_PATH,
            presented: Some(format!("Bearer {foreign_issuer}")),
            status: UNAUTHORIZED,
            sentence: UNAUTHORIZED_MESSAGE,
        },
        Refusal {
            case: "a token of another app client of the same pool: a foreign audience",
            path: OPEN_PATH,
            presented: Some(format!("Bearer {foreign_audience}")),
            status: UNAUTHORIZED,
            sentence: UNAUTHORIZED_MESSAGE,
        },
        Refusal {
            case: "no token at all",
            path: OPEN_PATH,
            presented: None,
            status: UNAUTHORIZED,
            sentence: UNAUTHORIZED_MESSAGE,
        },
        Refusal {
            case: "a value that is no token",
            path: OPEN_PATH,
            presented: Some("Bearer not-a-json-web-token".to_string()),
            status: UNAUTHORIZED,
            sentence: UNAUTHORIZED_MESSAGE,
        },
        Refusal {
            case: "an unsecured token (RFC 7519 §6: the alg none no provider issues)",
            path: OPEN_PATH,
            presented: Some(format!(
                "Bearer {}",
                signed_with(
                    json!({ "alg": "none", "typ": "JWT" }),
                    pool_shaped_claims(deployment, now() + 3600, None),
                )
            )),
            status: UNAUTHORIZED,
            sentence: UNAUTHORIZED_MESSAGE,
        },
        // The two window rows below carry the same placeholder signature segment every token
        // written in this tree carries: the gate verifies no signature (it fetches no
        // provider key), so the refusal each earns is the `exp`/`nbf` clause and nothing
        // about the signature.
        Refusal {
            case: "a token of the configured issuer whose validity window has passed",
            path: OPEN_PATH,
            presented: Some(format!(
                "Bearer {}",
                signed_with(
                    json!({ "alg": "RS256", "typ": "JWT" }),
                    pool_shaped_claims(deployment, now() - 60, None),
                )
            )),
            status: UNAUTHORIZED,
            sentence: UNAUTHORIZED_MESSAGE,
        },
        Refusal {
            case: "a token of the configured issuer whose validity window has not opened",
            path: OPEN_PATH,
            presented: Some(format!(
                "Bearer {}",
                signed_with(
                    json!({ "alg": "RS256", "typ": "JWT" }),
                    pool_shaped_claims(deployment, now() + 7200, Some(now() + 3600)),
                )
            )),
            status: UNAUTHORIZED,
            sentence: UNAUTHORIZED_MESSAGE,
        },
        Refusal {
            case: "the ADMITTED token at a route requiring a scope it does not claim",
            path: SCOPED_PATH,
            presented: Some(format!("Bearer {}", deployment.admitted)),
            status: FORBIDDEN,
            sentence: FORBIDDEN_MESSAGE,
        },
    ]
}

/// Send one request to the deployed stage, carrying `marker` as the query parameter the
/// handler reports and `presented` as its `Authorization` header when it has one.
async fn request(
    emulator: &MultiServiceRouter,
    deployment: &Deployment,
    path: &str,
    marker: &str,
    presented: Option<&str>,
) -> Answer {
    let path = format!("{path}?marker={marker}");
    match presented {
        Some(presented) => {
            send(
                emulator,
                &deployment.host,
                "GET",
                &path,
                &[("authorization", presented)],
            )
            .await
        }
        None => send(emulator, &deployment.host, "GET", &path, &[]).await,
    }
}

/// Every line the function's invocations have written to its own log group so far, or an
/// empty list while CloudWatch Logs holds no such group — which is what "the function has
/// not run" looks like to a client, since the group is opened by the first invocation whose
/// log Lambda delivers.
///
/// The read itself is [`crate::http_api_access_logging::delivered_to`], the one
/// `FilterLogEvents` reader this crate's suites share: it pages through the group and tells
/// "no such group" from a failed read.
async fn written(emulator: &MultiServiceRouter) -> Vec<String> {
    delivered_to(emulator, &log_group())
        .await
        .unwrap_or_default()
}

/// Read the function's log until the line naming `marker` is there.
///
/// The wait is for the CONDITION — the group is re-read until the line the case waits for is
/// in it — never for a fixed time, so a fast and a loaded host observe the same thing. The
/// deadline is [`CONDITION_DEADLINE`] and is a budget no case asserts on.
async fn written_until(emulator: &MultiServiceRouter, case: &str, marker: &str) {
    let wanted = format!("{HIT} {marker}");
    let deadline = Instant::now() + CONDITION_DEADLINE;
    loop {
        let lines = written(emulator).await;
        if lines.iter().any(|line| line.contains(&wanted)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the integration never wrote {wanted:?} to {}, lines so far {lines:?}",
            log_group()
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Assert that the integration wrote a line for `served` and none for any marker in
/// `not_served`.
///
/// The log is read once more AFTER the served request's own line is there, so a refused
/// request that had been forwarded cannot escape the assertion by logging late: every
/// refusal was sent before the served request, and this second reading is taken after the
/// served request's line arrived.
async fn only_served(
    emulator: &MultiServiceRouter,
    case: &str,
    served: &str,
    not_served: &[String],
) {
    written_until(emulator, case, served).await;
    let lines = written(emulator).await;
    for marker in not_served {
        let refused = format!("{HIT} {marker}");
        assert!(
            !lines.iter().any(|line| line.contains(&refused)),
            "case {case:?}: a refused request must not reach the integration, but {refused:?} \
             is in {lines:?}"
        );
    }
}

/// Assert that a request the gate refused was answered by the gateway itself.
fn refused(case: &str, answer: &Answer, row: &Refusal) {
    assert_eq!(
        answer.status,
        row.status,
        "case {case:?}: expected HTTP {} from the authorizer, got {}",
        row.status,
        answer.summary()
    );
    let sentence = message(answer);
    assert_eq!(
        sentence, row.sentence,
        "case {case:?}: expected the sentence {:?}, got {}",
        row.sentence,
        answer.summary()
    );
}

/// The claim set of `token` as the event's claims map carries it: every claim by name, as
/// TEXT — a string claim as the string it is, and any other claim as the JSON text it had.
fn reported_claims(token: &str) -> Value {
    let claims = claims_of(token);
    let members = claims
        .as_object()
        .unwrap_or_else(|| panic!("a minted token's claim set is a JSON object, got {claims}"))
        .clone();
    Value::Object(
        members
            .into_iter()
            .map(|(claim, value)| {
                let text = match value {
                    Value::String(text) => text,
                    other => other.to_string(),
                };
                (claim, Value::String(text))
            })
            .collect(),
    )
}

/// Assert that the control request was served by the integration, which reported the marker
/// it was handed and everything the authorizer told it about the caller: the admitted token's
/// OWN claim set and the scopes that token claims.
fn served(case: &str, answer: &Answer, marker: &str, deployment: &Deployment) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the integration's own 200 after the refusal, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.body,
        json!({
            "served": marker,
            "claims": reported_claims(&deployment.admitted),
            "scopes": [scope(READ)],
        }),
        "case {case:?}: expected the function's answer for this request, carrying the minted \
         token's own claims, got {}",
        answer.summary()
    );
}

/// Each refusal clause in turn, each followed by a request the authorizer admits: the
/// refused request is answered by the gateway and never reaches the integration, and the
/// request after it is served by that integration.
#[tokio::test]
async fn a_refused_request_is_not_forwarded_and_the_next_admitted_one_is() {
    let (emulator, engine) = emulator();
    let (deployment, foreign_audience) =
        given_deployment(&emulator, &engine, "a refusal and the request after it").await;
    // A token of ANOTHER pool entirely: its own issuer and its own app client, so both
    // recorded clauses are foreign to this authorizer.
    let foreign = given_foreign_pool(&emulator).await;

    for (index, row) in refusals(&deployment, &foreign, &foreign_audience)
        .into_iter()
        .enumerate()
    {
        let case = row.case;
        let refused_marker = format!("refused-{index}");
        let answer = request(
            &emulator,
            &deployment,
            row.path,
            &refused_marker,
            row.presented.as_deref(),
        )
        .await;
        refused(case, &answer, &row);

        // The control request: the same route, a token the authorizer admits.
        let control_marker = format!("control-{index}");
        let answer = request(
            &emulator,
            &deployment,
            OPEN_PATH,
            &control_marker,
            Some(&format!("Bearer {}", deployment.admitted)),
        )
        .await;
        served(case, &answer, &control_marker, &deployment);
        only_served(
            &emulator,
            case,
            &control_marker,
            std::slice::from_ref(&refused_marker),
        )
        .await;
    }
}

/// A whole RUN of refusals leaves the route serving: every clause is refused back to back,
/// and the one request after them that the authorizer admits is forwarded to the integration
/// — which has logged that request and none of the refused ones.
#[tokio::test]
async fn a_run_of_refusals_leaves_the_route_and_its_integration_intact() {
    let (emulator, engine) = emulator();
    let (deployment, foreign_audience) =
        given_deployment(&emulator, &engine, "a run of refusals").await;
    let foreign = given_foreign_pool(&emulator).await;
    let case = "a run of refusals";

    let mut refused_markers = Vec::new();
    for (index, row) in refusals(&deployment, &foreign, &foreign_audience)
        .into_iter()
        .enumerate()
    {
        let marker = format!("run-refused-{index}");
        let answer = request(
            &emulator,
            &deployment,
            row.path,
            &marker,
            row.presented.as_deref(),
        )
        .await;
        refused(row.case, &answer, &row);
        refused_markers.push(marker);
    }

    let marker = "run-control";
    let answer = request(
        &emulator,
        &deployment,
        OPEN_PATH,
        marker,
        Some(&format!("Bearer {}", deployment.admitted)),
    )
    .await;
    served(case, &answer, marker, &deployment);
    only_served(&emulator, case, marker, &refused_markers).await;
}

/// Another user pool with an app client of its own, and the token that client's grant mints:
/// the issuer and the audience the recorded refusal presents, neither of which the api's
/// authorizer records.
async fn given_foreign_pool(emulator: &MultiServiceRouter) -> String {
    let pool = cognito(
        emulator,
        "CreateUserPool",
        json!({ "PoolName": "another-pool" }),
    )
    .await;
    let pool_id = pool["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an Id, got {pool}"))
        .to_string();
    cognito(
        emulator,
        "CreateResourceServer",
        json!({
            "UserPoolId": pool_id,
            "Identifier": RESOURCE_SERVER,
            "Name": "another orders api",
            "Scopes": resource_server_scopes(),
        }),
    )
    .await;
    let client_id = m2m_client(emulator, &pool_id, "the-other-pools-client").await;
    granted_token(emulator, &client_id, &[scope(READ)]).await
}
