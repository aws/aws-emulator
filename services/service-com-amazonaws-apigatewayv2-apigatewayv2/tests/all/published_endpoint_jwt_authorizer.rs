// SPDX-License-Identifier: CC0-1.0

//! What a client that holds nothing but an HTTP api's published `ApiEndpoint` observes of the
//! JWT authorizer in front of its route — the address RESOLVED and DIALLED exactly as the
//! control plane published it.
//!
//! The rule, one level above the recorded case (an api with a Cognito-backed JWT authorizer,
//! whose published endpoint a client could not dial at all):
//!
//! > An address this service publishes for a caller to reach a resource at is one THIS
//! > EMULATOR ANSWERS ON: its authority is a host the process is reached at — the `host:port`
//! > the core reports for the listener serving it — and the resource is named in the part of
//! > the URL that listener routes on, never in a DNS name this process made up, which nothing
//! > on the machine the emulator runs on resolves. So a client that reads `ApiEndpoint`,
//! > resolves its authority and appends `/{stageName}/{routePath}` ("the stage name is
//! > typically appended to this URI to form a complete path to a deployed API stage",
//! > `Api$ApiEndpoint`) reaches the api's route, and every answer the gateway owes that
//! > request is observable there: a JWT-authorized route serves a token its authorizer admits
//! > and whose scopes meet the route's `AuthorizationScopes` with the integration's answer,
//! > carrying that token's `claims` and `scopes` in `requestContext.authorizer.jwt` of the
//! > event; refuses a token it admits whose scopes meet none of them `403`
//! > `{"message":"Forbidden"}`; and refuses an unusable token — expired, from another issuer,
//! > for another audience, or absent — `401` `{"message":"Unauthorized"}`, with the
//! > integration never reached.
//!
//! The rows span what the rule generalises over: the token presented (the route's scope held,
//! the scope held among others, another scope, no scope claim, an expired token, a foreign
//! issuer, a foreign audience, nothing at all), the stage the invoke URL names (the reserved
//! stage that serves the api's root, reached with no stage segment, and a named one) and the
//! api addressed (the api and a twin published beside it on the same socket). The recorded
//! case — one accepted token, one scope mismatch, one foreign issuer/audience, one expired
//! token — is four of these rows.
//!
//! The ADDRESS is never spelled out here: it is read off `CreateApi`'s answer, held to the
//! reading every suite that drives a published address shares
//! (`published_invoke_socket_front_door::announced_parts`) and then RESOLVED with the machine's
//! own resolver, so a published value no client could resolve fails this suite before any
//! authorization decision is read. The connection itself is the one raw HTTP/1.1 client those
//! suites share, which dials the loopback interface the resolution just proved the published
//! authority names.

use std::net::ToSocketAddrs;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::http_api_backend_dispatch::{
    control, gateway_may_invoke, given_ungranted_function, lambda_integration, ECHO_EVENT,
};
use crate::http_api_jwt_data_plane::{now, signed_with};
use crate::published_invoke_socket_front_door::{
    announced_parts, member, request, served_emulator, Answered,
};

/// The issuer the api's authorizer records, in the form `JWTConfiguration$Issuer` documents
/// for a user pool ("https://cognito-idp.{region}.amazonaws.com/{userPoolId}"), and another
/// pool's, which is a FOREIGN issuer to this api.
const ISSUER: &str = "https://cognito-idp.us-east-1.amazonaws.com/us-east-1_pool123";
const FOREIGN_ISSUER: &str = "https://cognito-idp.us-east-1.amazonaws.com/us-east-1_other99";

/// The user-pool client id the authorizer records as its audience, and another client's id,
/// which is a FOREIGN audience to this api.
const AUDIENCE: &str = "1example23456789abcdef";
const FOREIGN_AUDIENCE: &str = "9nobodyconfigured8765";

/// The scope the route requires, the one it does not, and a scope set that holds the required
/// one among others.
const REQUIRED_SCOPE: &str = "files/read";
const OTHER_SCOPE: &str = "files/write";
const SCOPES_HELD: &str = "openid files/read profile";

/// The route every api below carries and the path within a stage that matches it.
const ROUTE_KEY: &str = "GET /claims";
const ROUTE_PATH: &str = "/claims";

/// The stages every api is deployed to: the reserved one "that serves an api's root", reached
/// with no stage segment at all, and a named one, reached with its name appended.
const DEFAULT_STAGE: &str = "$default";
const NAMED_STAGE: &str = "prod";

/// What the function the route fronts answers: the event it was given, so the members the
/// gateway put in it are read off the body.
const SERVED: u16 = 200;

/// The statuses and sentences the gateway owes a request its authorizer refuses.
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// A compact JWS of the api's own pool and client, valid for the next hour, carrying `scope`
/// when the case states one — the token a client of a JWT-authorized route presents.
fn token(issuer: &str, audience: &str, scope: Option<&str>, expires_in: i64) -> String {
    let mut claims = json!({
        "iss": issuer,
        "aud": audience,
        "token_use": "access",
        "sub": "11111111-2222-3333-4444-555555555555",
        "iat": now(),
        "exp": now() + expires_in
    });
    if let Some(scope) = scope {
        claims["scope"] = json!(scope);
    }
    signed_with(
        json!({ "alg": "ES256", "kid": "e3b0c44298fc1c14", "typ": "JWT" }),
        claims,
    )
}

/// One api as a client of its published endpoint holds it: the id the service minted, the
/// authority that client resolves and the base path the published value carries.
struct GatedApi {
    api_id: String,
    authority: String,
    base: String,
}

impl GatedApi {
    /// The invoke path for `stage`: the published base path with the stage name appended, which
    /// for the stage that serves the api's root is no segment at all.
    fn at(&self, stage: &str, route_path: &str) -> String {
        format!("{}{}", self.base, Self::within(stage, route_path))
    }

    /// The path WITHIN the api that an invoke of `stage` addresses — the path the api's own
    /// records are read against, which is what the event reports as `rawPath`: the route path
    /// alone for the stage that serves the api's root, and the stage name ahead of it for a
    /// named stage.
    fn within(stage: &str, route_path: &str) -> String {
        if stage == DEFAULT_STAGE {
            route_path.to_string()
        } else {
            format!("/{stage}{route_path}")
        }
    }
}

/// Create an api whose `GET /claims` route is gated by a JWT authorizer of [`ISSUER`] and
/// [`AUDIENCE`] and requires [`REQUIRED_SCOPE`], front it with the echoing function, release
/// both stages over it, and answer the address the control plane PUBLISHED for it.
async fn published_api(
    emulator: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
) -> GatedApi {
    let function_arn = given_ungranted_function(emulator, engine, case, name, ECHO_EVENT).await;
    gateway_may_invoke(emulator, case, name, None).await;

    let (status, api) = control(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi answered {status} {api}"
    );
    let api_id = member(case, &api, "apiId");
    let announced = member(case, &api, "apiEndpoint");

    let (status, authorizer) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "pool-authorizer",
            "authorizerType": "JWT",
            "identitySource": ["$request.header.Authorization"],
            "jwtConfiguration": { "issuer": ISSUER, "audience": [AUDIENCE] }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateAuthorizer answered {status} {authorizer}"
    );
    let authorizer_id = member(case, &authorizer, "authorizerId");

    let (status, integration) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(lambda_integration(&function_arn, "2.0")),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration answered {status} {integration}"
    );
    let integration_id = member(case, &integration, "integrationId");

    let (status, route) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": ROUTE_KEY,
            "target": format!("integrations/{integration_id}"),
            "authorizationType": "JWT",
            "authorizerId": authorizer_id,
            "authorizationScopes": [REQUIRED_SCOPE]
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute answered {status} {route}"
    );

    for stage_name in [DEFAULT_STAGE, NAMED_STAGE] {
        let (status, stage) = control(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": stage_name, "autoDeploy": true })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateStage {stage_name:?} answered {status} {stage}"
        );
    }

    // The split and the shape of a published address are the reading the sibling suite owns
    // (an https endpoint on an address of this emulator's own, with the api named behind the
    // reserved data-plane marker), so this suite takes it rather than writing a second one.
    let (authority, base) = announced_parts(case, &api_id, &announced);
    GatedApi {
        api_id,
        authority,
        base,
    }
}

/// Assert that resolving `authority` — the step a client takes before it dials — answers at
/// least one address, and that every address it answers is on THIS machine: the value an api
/// publishes is dialable as given, and leads nowhere else.
fn assert_resolves_here(case: &str, authority: &str) {
    let addresses: Vec<std::net::SocketAddr> = authority
        .to_socket_addrs()
        .map(|resolved| resolved.collect())
        .unwrap_or_default();
    assert!(
        !addresses.is_empty(),
        "case {case:?}: the published authority {authority:?} must be one a client can \
         resolve, and nothing resolved it"
    );
    assert!(
        addresses.iter().all(|address| address.ip().is_loopback()),
        "case {case:?}: the published authority {authority:?} must lead back to this machine, \
         got {addresses:?}"
    );
}

/// Send one invoke to the api AT ITS PUBLISHED ADDRESS: the authority RESOLVED first, then
/// dialled on the loopback interface that resolution answered with, naming the published
/// authority in the `Host` — which is every step a client that holds nothing but `ApiEndpoint`
/// takes.
async fn invoked(case: &str, api: &GatedApi, stage: &str, presented: Option<&str>) -> Answered {
    assert_resolves_here(case, &api.authority);
    let headers: Vec<(&str, &str)> = presented
        .map(|token| vec![("Authorization", token)])
        .unwrap_or_default();
    request(
        case,
        &api.authority,
        "GET",
        &api.at(stage, ROUTE_PATH),
        &api.authority,
        &headers,
    )
    .await
}

/// One token presented to the gated route, and what the gateway owes it.
struct Row {
    case: &'static str,
    /// The token the `Authorization` header carries, or `None` to present nothing at all.
    presented: Option<String>,
    status: u16,
    /// The sentence the gateway's own refusal words, or `None` for a request it serves.
    message: Option<&'static str>,
    /// The scopes the event must report for a request the route serves, in the order a set of
    /// them reads ([`crate::http_api_jwt_data_plane`]'s own reading: sorted, so a scope claimed
    /// under either spelling is reported once).
    scopes: &'static [&'static str],
}

/// The token table: the four legs the recorded checks exercise (the route's scope held, a
/// scope mismatch, a foreign issuer, a foreign audience and an expired token) plus the forms a
/// client can otherwise present.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: an access token holding the route's scope",
            presented: Some(token(ISSUER, AUDIENCE, Some(REQUIRED_SCOPE), 3600)),
            status: SERVED,
            message: None,
            scopes: &[REQUIRED_SCOPE],
        },
        Row {
            case: "the route's scope held among others, presented with the Bearer prefix",
            presented: Some(format!(
                "Bearer {}",
                token(ISSUER, AUDIENCE, Some(SCOPES_HELD), 3600)
            )),
            status: SERVED,
            message: None,
            scopes: &[REQUIRED_SCOPE, "openid", "profile"],
        },
        Row {
            case: "a token whose only scope is another one",
            presented: Some(token(ISSUER, AUDIENCE, Some(OTHER_SCOPE), 3600)),
            status: FORBIDDEN,
            message: Some(FORBIDDEN_MESSAGE),
            scopes: &[],
        },
        Row {
            case: "a token claiming no scope at all",
            presented: Some(token(ISSUER, AUDIENCE, None, 3600)),
            status: FORBIDDEN,
            message: Some(FORBIDDEN_MESSAGE),
            scopes: &[],
        },
        Row {
            case: "an expired token carrying the route's scope",
            presented: Some(token(ISSUER, AUDIENCE, Some(REQUIRED_SCOPE), -3600)),
            status: UNAUTHORIZED,
            message: Some(UNAUTHORIZED_MESSAGE),
            scopes: &[],
        },
        Row {
            case: "a token from another issuer",
            presented: Some(token(FOREIGN_ISSUER, AUDIENCE, Some(REQUIRED_SCOPE), 3600)),
            status: UNAUTHORIZED,
            message: Some(UNAUTHORIZED_MESSAGE),
            scopes: &[],
        },
        Row {
            case: "a token for another audience",
            presented: Some(token(ISSUER, FOREIGN_AUDIENCE, Some(REQUIRED_SCOPE), 3600)),
            status: UNAUTHORIZED,
            message: Some(UNAUTHORIZED_MESSAGE),
            scopes: &[],
        },
        Row {
            case: "no token at all",
            presented: None,
            status: UNAUTHORIZED,
            message: Some(UNAUTHORIZED_MESSAGE),
            scopes: &[],
        },
    ]
}

/// RULE: the published `ApiEndpoint` is an address a client resolves and dials, and the JWT
/// authorizer on the route decides every request made there — an admitted token with the
/// route's scope is served by the integration with the token's claims and scopes in the event,
/// a scope mismatch is `403` and an unusable token `401`, neither reaching the integration.
#[tokio::test(flavor = "multi_thread")]
async fn the_published_endpoint_is_dialable_and_its_jwt_authorizer_decides() {
    let (emulator, engine) = served_emulator();
    let case = "the api a client holds the published endpoint of";
    let api = published_api(&emulator, &engine, case, "jwt-claims-echo").await;

    for row in rows() {
        for stage in [DEFAULT_STAGE, NAMED_STAGE] {
            let case = row.case;
            let presented = row.presented.as_deref();
            let answered = invoked(case, &api, stage, presented).await;
            assert_eq!(
                answered.status, row.status,
                "case {case:?} at stage {stage:?}: expected {}, got {answered:?}",
                row.status
            );
            match row.message {
                // A refusal is the gateway's own sentence, and the event the function echoes is
                // nowhere in it: the integration was not reached.
                Some(message) => {
                    assert_eq!(
                        answered.message(),
                        message,
                        "case {case:?} at stage {stage:?}: expected the {message:?} body, got \
                         {answered:?}"
                    );
                    assert!(
                        answered.json().get("requestContext").is_none(),
                        "case {case:?} at stage {stage:?}: a refused request must never reach \
                         the integration, got {answered:?}"
                    );
                }
                // A served request is the function's answer, so the event it was handed is the
                // body: the accepted token's claims and scopes are in it, under the members the
                // 2.0 event carries for a JWT-authorized route.
                None => {
                    let event = answered.json();
                    let jwt = event
                        .pointer("/requestContext/authorizer/jwt")
                        .cloned()
                        .unwrap_or(Value::Null);
                    assert_eq!(
                        jwt.pointer("/claims/iss").and_then(Value::as_str),
                        Some(ISSUER),
                        "case {case:?} at stage {stage:?}: expected the accepted token's issuer \
                         among the claims, got {jwt}"
                    );
                    assert_eq!(
                        jwt.pointer("/claims/aud").and_then(Value::as_str),
                        Some(AUDIENCE),
                        "case {case:?} at stage {stage:?}: expected the accepted token's \
                         audience among the claims, got {jwt}"
                    );
                    assert_eq!(
                        jwt.get("scopes")
                            .and_then(Value::as_array)
                            .map(|scopes| scopes
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<&str>>())
                            .unwrap_or_default(),
                        row.scopes.to_vec(),
                        "case {case:?} at stage {stage:?}: expected the scopes the token \
                         claimed, got {jwt}"
                    );
                    assert_eq!(
                        event.get("rawPath").and_then(Value::as_str),
                        Some(GatedApi::within(stage, ROUTE_PATH).as_str()),
                        "case {case:?} at stage {stage:?}: expected the event to carry the path \
                         within the api the client addressed, got {event}"
                    );
                }
            }
        }
    }
}

/// RULE: one socket serves every api of the account, so each api is published at the SAME
/// dialable authority under its own path — and a request made at one api's published address
/// is decided by THAT api's authorizer and served by THAT api's integration.
#[tokio::test(flavor = "multi_thread")]
async fn each_api_is_published_at_the_same_address_under_its_own_path() {
    let (emulator, engine) = served_emulator();
    let case = "two apis of one account, each holding its own published endpoint";
    let api = published_api(&emulator, &engine, case, "jwt-claims-echo-first").await;
    let twin = published_api(&emulator, &engine, case, "jwt-claims-echo-twin").await;

    assert_eq!(
        api.authority, twin.authority,
        "case {case:?}: expected both apis published on one socket, got {:?} and {:?}",
        api.authority, twin.authority
    );
    assert_ne!(
        api.base, twin.base,
        "case {case:?}: expected each api published under its own path, got {:?} for both",
        api.base
    );

    let admitted = token(ISSUER, AUDIENCE, Some(REQUIRED_SCOPE), 3600);
    for (which, published) in [("the api", &api), ("its twin", &twin)] {
        let answered = invoked(case, published, DEFAULT_STAGE, Some(&admitted)).await;
        assert_eq!(
            answered.status, SERVED,
            "case {case:?}: expected {which} served at its own published address, got \
             {answered:?}"
        );
        assert_eq!(
            answered
                .json()
                .pointer("/requestContext/apiId")
                .and_then(Value::as_str),
            Some(published.api_id.as_str()),
            "case {case:?}: expected {which} to serve its own api, got {answered:?}"
        );
    }
}
