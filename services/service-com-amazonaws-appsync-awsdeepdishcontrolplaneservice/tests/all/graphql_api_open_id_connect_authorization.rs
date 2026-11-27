// SPDX-License-Identifier: CC0-1.0

//! What a GraphQL API whose authorization mode is `OPENID_CONNECT` serves, driven over its
//! published socket with tokens an issuer of this emulator really minted.
//!
//! Two rules, each one level above the recorded case (one API created with
//! `openIDConnectConfig.iatTTL`, one `NONE` data source and one APPSYNC_JS resolver returning
//! a marker):
//!
//! > A UNIT resolver whose runtime is APPSYNC_JS over a data source of type `NONE` resolves its
//! > field to the value its `response` function RETURNS: there is no external call, so the
//! > `payload` the `request` function returned is what the response function is handed as
//! > `ctx.result`, and whatever it returns — a value it built itself, the result passed
//! > through, an object, a number, a boolean, nothing at all — is the field's value under its
//! > response key, with no `errors` member for a run that raised nothing.
//!
//! > A call on such an API is decided by the TOKEN it presents in its `authorization` field,
//! > against the `openIDConnectConfig` the API stores: the call is served only when that value
//! > is a readable compact JWS whose `iss` names the issuer the configuration names, whose
//! > signature checks out against the key set that issuer publishes, which is inside its own
//! > validity window, whose client satisfies `clientId` where the configuration states one,
//! > and which is inside the window each stated TTL bounds it to — `iatTTL`, "the number of
//! > milliseconds that a token is valid after it's issued to a user", measured from the token's
//! > `iat`, and `authTTL`, "the number of milliseconds that a token is valid after being
//! > authenticated", measured from its `auth_time`. A TTL of `0` (the member's default) bounds
//! > nothing. Anything else authorizes nobody and is refused `401` `UnauthorizedException`,
//! > reaching no resolver — a token the mode READ and refused with `Unauthorized`, a value it
//! > could not read as a token with `Unable to parse JWT token.`, and a call presenting no
//! > material of any mode the API set up with the sentence every unauthorized call on this
//! > plane earns. What an admitted token CLAIMS is who the call speaks for, which a resolver
//! > reads as `$context.identity`.
//!
//! The rows span what those rules generalise over: the SHAPE a response function returns and
//! where it got it; which TTL the configuration states and how far past the claim it measures
//! from the call is made; the token KIND (an identity token, an access token); how it is
//! presented (bare, with the RFC 6750 bearer scheme, absent); what makes a value no token this
//! API accepts (another issuer's token, a rewritten claim set, a value that is no token, an
//! elapsed window, a client the configuration filters out); and the FORM the configuration
//! names the issuer in (the value the token itself claims, the regional form of the same
//! provider, one carrying a trailing separator, another provider's).
//!
//! Nothing asserted here is a value this emulator filled in for the assertion's sake: every
//! token is one `AdminInitiateAuth` minted for a user of a pool this suite created through
//! that service's own operations, and the tokens no sign-in produces — one older than a TTL,
//! one whose window has elapsed — are signed with that same pool's own signing key. The pool,
//! the user and the sign-in are made through the sibling suite's one reading of them
//! ([`crate::graphql_api_user_pool_authorization`]).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_realtime_endpoint::{exchange, handshake};
use crate::graphql_api_user_pool_authorization::{
    claims_of, emulator, id_token, pool_signed_token, pool_with_client, seconds_ago, sign_in,
    user_in_groups, with_rewritten_claims,
};

/// The Region every call of this suite is signed for, and the Region the pools are created in.
const REGION: &str = "us-west-2";

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The refusal a call presenting no material of any mode the API set up earns. The service's
/// own copies are `service::event_api::UNAUTHORIZED_ERROR_TYPE` and `…_MESSAGE`.
const UNAUTHORIZED_STATUS: u16 = 401;
const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";
const UNAUTHORIZED_MESSAGE: &str = "You are not authorized to make this call.";

/// What a caller presenting a token this mode READ and refused is told instead; the service's
/// own copy is `service::graphql_api::open_id_connect::UNAUTHORIZED_TOKEN_MESSAGE`.
const UNAUTHORIZED_TOKEN_MESSAGE: &str = "Unauthorized";

/// What a caller presenting a value no token mode can READ as a token is told instead; the
/// service's own copy is `service::graphql_api::user_pool::UNREADABLE_TOKEN_MESSAGE`.
const UNREADABLE_TOKEN_MESSAGE: &str = "Unable to parse JWT token.";

/// The `runtime` the resolvers of this suite are defined under. Source: `AppSyncRuntime`'s own
/// values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The name of the local data source every resolver of this suite resolves against: `NONE` is
/// "There is no data source", so the request function's own payload is the result.
const DATA_SOURCE: &str = "local";

/// How long a TTL of this suite's APIs bounds a token for, in milliseconds, and how far past
/// the claim it measures from an aged token of a row sits.
///
/// Both are far wider than any step of this suite takes, so what a row asserts is the TTL rule
/// and never how fast the host runs: a token minted now is deep inside the window, and the
/// aged one is deep outside it.
const GENEROUS_TTL_MS: i64 = 600_000;
const WELL_PAST_THE_TTL: Duration = Duration::from_secs(3_600);

/// The schema every API of this suite is created with: one field per SHAPE a local resolver's
/// response function returns, and one that reports the identity the call resolved to.
const SCHEMA: &str = "type Note {\n  id: ID!\n  title: String\n}\n\
     \n\
     type Query {\n  \
     ping: String\n  \
     echoed(text: String): String\n  \
     counted: Int\n  \
     flagged: Boolean\n  \
     noted: Note\n  \
     missing: String\n  \
     whoami: String\n}\n";

/// The value the `ping` resolver's response function returns, written in the resolver's own
/// code and nowhere in the request: an answer carrying it came from the stored resolver.
const PING_MARKER: &str = "pong";

/// One created GraphQL API: the address its GraphQL endpoint is served on.
struct GraphqlApi {
    graphql: String,
}

/// Create one GraphQL API in the `OPENID_CONNECT` mode over `configured`, with [`SCHEMA`], a
/// `NONE` data source and one resolver per field.
///
/// `configured` is the whole `openIDConnectConfig` the API is created with, so a row varies any
/// member of it; the create is read back, because the configuration the mode judges a call
/// against is the STORED one.
async fn create_api(router: &MultiServiceRouter, name: &str, configured: Value) -> GraphqlApi {
    // What a later read has to report: every member the create asks for, and the two TTLs at
    // the `0` their model default states for a create that asks for neither.
    let mut expected = configured.clone();
    for (member, default) in [("iatTTL", json!(0)), ("authTTL", json!(0))] {
        if expected.get(member).is_none() {
            expected[member] = default;
        }
    }
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("oidc-{name}"),
            "authenticationType": "OPENID_CONNECT",
            "openIDConnectConfig": configured,
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "GRAPHQL",
    );
    assert!(
        graphql.starts_with(&format!("{LOOPBACK_HOST}:")),
        "an API's GraphQL endpoint must be published on an address this emulator serves, got \
         {graphql:?}"
    );
    install_schema(router, &api_id, SCHEMA).await;
    let (status, fetched) = appsync(router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(status, 200, "GetGraphqlApi must answer 200, got {fetched}");
    // The configuration the mode judges a call against is the STORED one.
    assert_eq!(
        fetched["graphqlApi"]["openIDConnectConfig"], expected,
        "GetGraphqlApi must report the openIDConnectConfig the create stored, got {fetched}"
    );
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    create_resolvers(router, &api_id).await;
    GraphqlApi { graphql }
}

/// Attach one resolver per field of [`SCHEMA`]: each is APPSYNC_JS `code` over the local data
/// source, and each returns its field's value from its RESPONSE function — the one AppSync runs
/// after the (absent) data-source call.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    let resolvers: &[(&str, &str)] = &[
        // The recorded shape: a request function returning no payload at all, and a response
        // function returning a value of its own.
        (
            "ping",
            "export function request(ctx) { return {}; }\n\
             export function response(ctx) { return \"pong\"; }",
        ),
        // The payload passed through the (absent) call, built from the field's own argument.
        (
            "echoed",
            "export function request(ctx) { return {payload: ctx.args.text}; }\n\
             export function response(ctx) { return ctx.result; }",
        ),
        (
            "counted",
            "export function request(ctx) { return {payload: {of: [1, 2, 3, 4]}}; }\n\
             export function response(ctx) { return ctx.result.of.length; }",
        ),
        (
            "flagged",
            "export function request(ctx) { return {payload: true}; }\n\
             export function response(ctx) { return ctx.result; }",
        ),
        (
            "noted",
            "export function request(ctx) { return {payload: {id: \"n-1\"}}; }\n\
             export function response(ctx) {\n\
               return {id: ctx.result.id, title: \"note \" + ctx.result.id};\n\
             }",
        ),
        (
            "missing",
            "export function request(ctx) { return {}; }\n\
             export function response(ctx) { return null; }",
        ),
        (
            "whoami",
            "export function request(ctx) { return {}; }\n\
             export function response(ctx) { return ctx.identity.sub; }",
        ),
    ];
    for (field, code) in resolvers {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
                "code": code,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {field} must answer 200, got {created}"
        );
    }
}

/// POST one GraphQL document to `api`'s published endpoint, presenting `presented` in the
/// request's `authorization` field — or nothing at all.
async fn post_graphql(
    case: &str,
    api: &GraphqlApi,
    query: &str,
    presented: Option<&str>,
) -> (u16, Value) {
    let body = json!({"query": query});
    let fields: Vec<(&str, &str)> = match presented {
        Some(value) => vec![("authorization", value)],
        None => Vec::new(),
    };
    let (status, _, document) =
        try_post_answer(case, &api.graphql, GRAPHQL_PATH, "POST", &body, &fields)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
                    api.graphql
                )
            });
    (status, document)
}

/// The issuer the regional form names for `user_pool_id` — the form the Amazon Cognito
/// developer guide documents ("`https://cognito-idp.[region].amazonaws.com/[userPoolId]`"),
/// which a configuration written against AWS holds for a pool.
fn regional_issuer(user_pool_id: &str) -> String {
    format!("https://cognito-idp.{REGION}.amazonaws.com/{user_pool_id}")
}

/// One pool with one app client, one confirmed user and that user's tokens.
struct Issuer {
    user_pool_id: String,
    client_id: String,
    tokens: Value,
}

impl Issuer {
    /// The `iss` the tokens this issuer minted claim, which is the issuer a configuration
    /// discovered from this provider holds.
    fn claimed(&self) -> String {
        claims_of(&id_token(&self.tokens))["iss"]
            .as_str()
            .unwrap_or_else(|| panic!("a minted token must claim an issuer"))
            .to_string()
    }
}

/// Create one pool with one app client and one confirmed user, and sign that user in.
async fn issuer(router: &MultiServiceRouter, name: &str) -> Issuer {
    let (user_pool_id, client_id) = pool_with_client(router, name).await;
    user_in_groups(router, &user_pool_id, "alice", &[]).await;
    let tokens = sign_in(router, &user_pool_id, &client_id, "alice").await;
    assert_eq!(
        claims_of(&id_token(&tokens))["aud"].as_str(),
        Some(client_id.as_str()),
        "a sign-in's identity token names the app client it was minted for"
    );
    Issuer {
        user_pool_id,
        client_id,
        tokens,
    }
}

/// RULE: a UNIT resolver whose runtime is APPSYNC_JS over a `NONE` data source resolves its
/// field to the value its RESPONSE function returns, whatever shape that value takes and
/// wherever the function got it — and an answer that raised nothing carries no `errors` member.
#[tokio::test]
async fn a_local_appsync_js_resolver_answers_with_what_its_response_function_returns() {
    let (router, _engine) = emulator();
    let provider = issuer(&router, "local-resolvers").await;
    let api = create_api(
        &router,
        "local-resolvers",
        json!({"issuer": provider.claimed(), "clientId": provider.client_id}),
    )
    .await;
    let presented = id_token(&provider.tokens);

    /// One row: the document a client POSTs and the `data` the API must answer for it.
    struct Row {
        case: &'static str,
        query: &'static str,
        data: Value,
    }
    let rows: &[Row] = &[
        Row {
            case: "a value the response function built itself, over no payload at all",
            query: "query P { ping }",
            data: json!({"ping": PING_MARKER}),
        },
        Row {
            case: "the payload the request function passed through",
            query: "query P { echoed(text: \"spoken\") }",
            data: json!({"echoed": "spoken"}),
        },
        Row {
            case: "a number computed from the payload",
            query: "query P { counted }",
            data: json!({"counted": 4}),
        },
        Row {
            case: "a boolean passed through",
            query: "query P { flagged }",
            data: json!({"flagged": true}),
        },
        Row {
            case: "an object with the selection projected onto it",
            query: "query P { noted { id title } }",
            data: json!({"noted": {"id": "n-1", "title": "note n-1"}}),
        },
        Row {
            case: "nothing at all",
            query: "query P { missing }",
            data: json!({"missing": Value::Null}),
        },
        Row {
            case: "two fields of one document",
            query: "query P { ping counted }",
            data: json!({"ping": PING_MARKER, "counted": 4}),
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answered) = post_graphql(case, &api, row.query, Some(&presented)).await;
        assert_eq!(
            (status, &answered),
            (200, &json!({"data": row.data})),
            "case {case:?}: the field must be the value its response function returned, got \
             {status} and {answered}"
        );
    }
}

/// RULE: an admitted token's claims are who the call speaks for, which a resolver reads as
/// `$context.identity`.
#[tokio::test]
async fn an_admitted_tokens_claims_are_the_identity_the_resolver_reads() {
    let (router, _engine) = emulator();
    let provider = issuer(&router, "identity").await;
    let api = create_api(
        &router,
        "identity",
        json!({"issuer": provider.claimed(), "clientId": provider.client_id}),
    )
    .await;

    // The rows differ along the token KIND, because each names the caller the same way and
    // this API filters no client out.
    for (case, presented) in [
        ("an identity token", id_token(&provider.tokens)),
        (
            "an access token of the same sign-in",
            provider.tokens["AccessToken"]
                .as_str()
                .unwrap_or_else(|| panic!("a sign-in must mint an AccessToken"))
                .to_string(),
        ),
    ] {
        let subject = claims_of(&presented)["sub"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a minted token must claim a subject"))
            .to_string();
        let (status, answered) =
            post_graphql(case, &api, "query P { whoami }", Some(&presented)).await;
        assert_eq!(
            (status, answered["data"]["whoami"].as_str()),
            (200, Some(subject.as_str())),
            "case {case:?}: the resolver must read the subject the token claims, got {status} \
             and {answered}"
        );
    }
}

/// What one row of the token table owes the call.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The call is served and `ping` resolves to the resolver's own marker.
    Served,
    /// The call is refused, with this sentence.
    Refused(&'static str),
}

/// RULE: a call on an `OPENID_CONNECT` API is served only when the token it presents satisfies
/// every member of the API's stored `openIDConnectConfig` and its own validity window; anything
/// else is refused `401` `UnauthorizedException` and reaches no resolver.
#[tokio::test]
async fn only_a_token_the_configured_provider_vouches_for_reaches_a_field() {
    let (router, engine) = emulator();
    let provider = issuer(&router, "tokens").await;
    let other = issuer(&router, "other").await;
    let minted = id_token(&provider.tokens);
    let claims = claims_of(&minted);

    // The tokens no sign-in mints: one issued long before the TTL this API states, and one
    // whose own window has elapsed. Both are signed with the pool's OWN key, so what a row
    // asserts is the check the configuration states rather than a value no issuer would sign.
    let aged = pool_signed_token(&engine, &provider.user_pool_id, &{
        let mut aged = claims.clone();
        aged["iat"] = json!(seconds_ago(WELL_PAST_THE_TTL));
        aged
    });
    let elapsed = pool_signed_token(&engine, &provider.user_pool_id, &{
        let mut elapsed = claims.clone();
        elapsed["exp"] = json!(seconds_ago(Duration::from_secs(600)));
        elapsed
    });
    let authenticated_long_ago = pool_signed_token(&engine, &provider.user_pool_id, &{
        let mut stale = claims.clone();
        stale["auth_time"] = json!(seconds_ago(WELL_PAST_THE_TTL));
        stale
    });

    // One API per configuration the rows judge a token against: the recorded one bounds a
    // token by `iatTTL`, its twin states no TTL at all, one bounds it by `authTTL`, and two
    // state a `clientId` — the token's own, and one no client of this issuer matches.
    let bounded = create_api(
        &router,
        "iat-ttl",
        json!({"issuer": provider.claimed(), "iatTTL": GENEROUS_TTL_MS}),
    )
    .await;
    let unbounded = create_api(&router, "twin", json!({"issuer": provider.claimed()})).await;
    let authenticated = create_api(
        &router,
        "auth-ttl",
        json!({"issuer": provider.claimed(), "authTTL": GENEROUS_TTL_MS}),
    )
    .await;
    let own_client = create_api(
        &router,
        "client",
        json!({"issuer": provider.claimed(), "clientId": provider.client_id}),
    )
    .await;
    let other_client = create_api(
        &router,
        "other-client",
        json!({"issuer": provider.claimed(), "clientId": "no-such-client"}),
    )
    .await;

    /// One row: which API the call is made on, what it presents, and what the call owes.
    struct Row<'api> {
        case: &'static str,
        api: &'api GraphqlApi,
        presented: Option<String>,
        expected: Expected,
    }
    let refused = Expected::Refused(UNAUTHORIZED_TOKEN_MESSAGE);
    let rows: &[Row<'_>] = &[
        Row {
            case: "a freshly minted token on the API that bounds it by iatTTL",
            api: &bounded,
            presented: Some(minted.clone()),
            expected: Expected::Served,
        },
        Row {
            case: "the same token with the bearer scheme in front of it",
            api: &bounded,
            presented: Some(format!("Bearer {minted}")),
            expected: Expected::Served,
        },
        Row {
            case: "a token issued long before that iatTTL, its own window still open",
            api: &bounded,
            presented: Some(aged.clone()),
            expected: refused,
        },
        Row {
            case: "that same aged token on the twin API, which states no TTL",
            api: &unbounded,
            presented: Some(aged.clone()),
            expected: Expected::Served,
        },
        Row {
            case: "a freshly minted token on the API that bounds it by authTTL",
            api: &authenticated,
            presented: Some(minted.clone()),
            expected: Expected::Served,
        },
        Row {
            case: "a token authenticated long before that authTTL",
            api: &authenticated,
            presented: Some(authenticated_long_ago.clone()),
            expected: refused,
        },
        Row {
            case: "that same token on the twin API, which states no TTL",
            api: &unbounded,
            presented: Some(authenticated_long_ago.clone()),
            expected: Expected::Served,
        },
        Row {
            case: "a token whose own window has elapsed",
            api: &unbounded,
            presented: Some(elapsed.clone()),
            expected: refused,
        },
        Row {
            case: "a token minted by another provider entirely",
            api: &unbounded,
            presented: Some(id_token(&other.tokens)),
            expected: refused,
        },
        Row {
            case: "the provider's token with its claim set rewritten after signing",
            api: &unbounded,
            presented: Some(with_rewritten_claims(&minted, |claims| {
                claims["sub"] = json!("somebody-else");
            })),
            expected: refused,
        },
        Row {
            case: "a presented value that is no token at all",
            api: &unbounded,
            presented: Some("not-a-token-at-all".to_string()),
            expected: Expected::Refused(UNREADABLE_TOKEN_MESSAGE),
        },
        Row {
            case: "nothing presented at all",
            api: &unbounded,
            presented: None,
            // Not the sentence a token this mode JUDGED earns: a call presenting no material
            // of any mode the API set up is refused before any mode reads one.
            expected: Expected::Refused(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a token of the client the configuration names",
            api: &own_client,
            presented: Some(minted.clone()),
            expected: Expected::Served,
        },
        Row {
            case: "a token of a client the configuration filters out",
            api: &other_client,
            presented: Some(minted.clone()),
            expected: refused,
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answered) =
            post_graphql(case, row.api, "query P { ping }", row.presented.as_deref()).await;
        match row.expected {
            Expected::Served => assert_eq!(
                (status, &answered),
                (200, &json!({"data": {"ping": PING_MARKER}})),
                "case {case:?}: the call must be served by the API's own resolver, got {status} \
                 and {answered}"
            ),
            Expected::Refused(message) => {
                assert_eq!(
                    (
                        status,
                        answered["errors"][0]["errorType"].as_str(),
                        answered["errors"][0]["message"].as_str(),
                    ),
                    (
                        UNAUTHORIZED_STATUS,
                        Some(UNAUTHORIZED_ERROR_TYPE),
                        Some(message),
                    ),
                    "case {case:?}: expected ({UNAUTHORIZED_STATUS}, \
                     {UNAUTHORIZED_ERROR_TYPE:?}, {message:?}), got {status} and {answered}"
                );
                assert!(
                    answered.get("data").is_none() || answered["data"].is_null(),
                    "case {case:?}: a refused call must reach no resolver, got {answered}"
                );
            }
        }
    }
}

/// RULE: the issuer a configuration names and the one a token claims have to name the same
/// provider — every address this emulator publishes one of its own issuers at names it, and
/// nothing else does.
#[tokio::test]
async fn the_configured_issuer_has_to_name_the_provider_the_token_came_from() {
    let (router, _engine) = emulator();
    let provider = issuer(&router, "issuer-forms").await;
    let other = issuer(&router, "issuer-forms-other").await;
    let presented = id_token(&provider.tokens);

    // The rows differ along the FORM the configuration names the issuer in: the value the
    // token itself claims, the regional form the developer guide documents for the same pool,
    // the same value carrying a trailing separator, another provider's issuer, and a prefix of
    // the configured one.
    let rows: &[(&str, String, Expected)] = &[
        (
            "the issuer the token itself claims",
            provider.claimed(),
            Expected::Served,
        ),
        (
            "the regional form of the same provider",
            regional_issuer(&provider.user_pool_id),
            Expected::Served,
        ),
        (
            "the same value with a trailing separator",
            format!("{}/", provider.claimed()),
            Expected::Served,
        ),
        (
            "another provider's issuer",
            other.claimed(),
            Expected::Refused(UNAUTHORIZED_TOKEN_MESSAGE),
        ),
        (
            "an issuer naming no provider of this emulator",
            "https://issuer.example/realm".to_string(),
            Expected::Refused(UNAUTHORIZED_TOKEN_MESSAGE),
        ),
    ];

    for (case, configured, expected) in rows {
        let api = create_api(&router, case, json!({"issuer": configured})).await;
        let (status, answered) =
            post_graphql(case, &api, "query P { ping }", Some(&presented)).await;
        match expected {
            Expected::Served => assert_eq!(
                (status, &answered),
                (200, &json!({"data": {"ping": PING_MARKER}})),
                "case {case:?}: a configuration naming the token's own provider must serve the \
                 call, got {status} and {answered}"
            ),
            Expected::Refused(message) => assert_eq!(
                (
                    status,
                    answered["errors"][0]["errorType"].as_str(),
                    answered["errors"][0]["message"].as_str(),
                ),
                (
                    UNAUTHORIZED_STATUS,
                    Some(UNAUTHORIZED_ERROR_TYPE),
                    Some(*message),
                ),
                "case {case:?}: expected ({UNAUTHORIZED_STATUS}, {UNAUTHORIZED_ERROR_TYPE:?}, \
                 {message:?}), got {status} and {answered}"
            ),
        }
    }
}

/// RULE: the signature is checked against the key set the issuer PUBLISHES, which this process
/// holds only for an issuer it serves itself. For an issuer outside this process there is no key
/// set to read and no network to fetch one over, so a token claiming it is judged on its CLAIMS
/// alone — the issuer, the client and the windows the configuration states, which are the whole
/// check there — while a token claiming an issuer this process DOES publish has to be one that
/// issuer signed.
///
/// The rows differ along which kind of issuer the API configured and along what the token
/// carries: a claim set no issuer of this emulator signed, the same set aged past the
/// configuration's own window, and the same set claiming an in-process provider.
#[tokio::test]
async fn a_token_of_an_issuer_outside_this_process_is_judged_on_its_claims_alone() {
    let (router, _engine) = emulator();
    let provider = issuer(&router, "external").await;
    // An issuer no part of this emulator serves, so nothing here publishes a key set for it.
    let outside = "https://issuer.example/realm";
    let seconds = |ago: Duration| json!(seconds_ago(ago));
    let fabricated = |claimed_issuer: &str, issued_at: Value| {
        fabricated_token(&json!({
            "iss": claimed_issuer,
            "aud": provider.client_id,
            "sub": "outside-subject",
            "iat": issued_at,
            "exp": seconds_ago(Duration::ZERO) + 3_600,
        }))
    };

    let external = create_api(
        &router,
        "external-issuer",
        json!({
            "issuer": outside,
            "clientId": provider.client_id,
            "iatTTL": GENEROUS_TTL_MS,
        }),
    )
    .await;
    let in_process = create_api(
        &router,
        "in-process-issuer",
        json!({"issuer": provider.claimed(), "clientId": provider.client_id}),
    )
    .await;

    let rows: &[(&str, &GraphqlApi, String, Expected)] = &[
        (
            "a claim set nothing signed, for the external issuer the API configured",
            &external,
            fabricated(outside, seconds(Duration::from_secs(1))),
            Expected::Served,
        ),
        (
            "the same claim set issued long before that API's iatTTL",
            &external,
            fabricated(outside, seconds(WELL_PAST_THE_TTL)),
            Expected::Refused(UNAUTHORIZED_TOKEN_MESSAGE),
        ),
        (
            "the same claim set naming the in-process provider, which signed no such token",
            &in_process,
            fabricated(&provider.claimed(), seconds(Duration::from_secs(1))),
            Expected::Refused(UNAUTHORIZED_TOKEN_MESSAGE),
        ),
    ];

    for (case, api, presented, expected) in rows {
        let (status, answered) =
            post_graphql(case, api, "query P { ping }", Some(presented)).await;
        match expected {
            Expected::Served => assert_eq!(
                (status, &answered),
                (200, &json!({"data": {"ping": PING_MARKER}})),
                "case {case:?}: a token of an issuer this process publishes no key set for is \
                 judged on its claims, got {status} and {answered}"
            ),
            Expected::Refused(message) => assert_eq!(
                (
                    status,
                    answered["errors"][0]["errorType"].as_str(),
                    answered["errors"][0]["message"].as_str(),
                ),
                (
                    UNAUTHORIZED_STATUS,
                    Some(UNAUTHORIZED_ERROR_TYPE),
                    Some(*message),
                ),
                "case {case:?}: expected ({UNAUTHORIZED_STATUS}, {UNAUTHORIZED_ERROR_TYPE:?}, \
                 {message:?}), got {status} and {answered}"
            ),
        }
    }
}

/// A compact JWS carrying `claims` that no issuer signed: the form rules a presented token has
/// to satisfy (three segments, a named algorithm, a signature segment that is not empty) over a
/// signature nobody produced.
///
/// What it is for: a token of an issuer this process publishes no key set for, which is the
/// only kind this plane cannot check a signature on.
fn fabricated_token(claims: &Value) -> String {
    format!(
        "{}.{}.{}",
        service_common::jwt::encode_base64url(b"{\"alg\":\"RS256\",\"kid\":\"outside\"}"),
        service_common::jwt::encode_base64url(claims.to_string().as_bytes()),
        service_common::jwt::encode_base64url(b"no issuer signed this")
    )
}

/// RULE: the same decision guards a REAL-TIME connection, which is the other endpoint the API
/// publishes: a connection presenting a token past the window the configuration states is
/// refused the moment it initialises, and one presenting a live token is acked.
#[tokio::test]
async fn the_same_token_decision_guards_the_realtime_endpoint() {
    let (router, engine) = emulator();
    let provider = issuer(&router, "realtime").await;
    let minted = id_token(&provider.tokens);
    let aged = pool_signed_token(&engine, &provider.user_pool_id, &{
        let mut aged = claims_of(&minted);
        aged["iat"] = json!(seconds_ago(WELL_PAST_THE_TTL));
        aged
    });
    let (status, created) = appsync(
        &router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": "oidc-realtime-api",
            "authenticationType": "OPENID_CONNECT",
            "openIDConnectConfig": {
                "issuer": provider.claimed(),
                "iatTTL": GENEROUS_TTL_MS,
            },
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let realtime = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "REALTIME",
    );

    for (case, presented, acked) in [
        ("a freshly minted token", minted.clone(), true),
        ("a token issued long before the iatTTL", aged.clone(), false),
    ] {
        // The material travels in the handshake's base64url `header` object, which is where a
        // real-time client of a GraphQL API puts a request's own fields.
        let mut socket = handshake(
            case,
            &realtime,
            GRAPHQL_PATH,
            true,
            Some(&json!({"host": realtime, "Authorization": presented})),
            &[],
        )
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the handshake must complete, got {status}")
        });
        let answered = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
        let kind = answered.as_ref().and_then(|frame| frame["type"].as_str());
        match acked {
            true => assert_eq!(
                kind,
                Some("connection_ack"),
                "case {case:?}: a live token must open the connection, got {answered:?}"
            ),
            false => assert_eq!(
                (
                    kind,
                    answered
                        .as_ref()
                        .and_then(|frame| frame.pointer("/payload/errors/0/errorType"))
                        .and_then(Value::as_str),
                ),
                (Some("connection_error"), Some(UNAUTHORIZED_ERROR_TYPE)),
                "case {case:?}: a token past the configured window must be refused at its \
                 init, got {answered:?}"
            ),
        }
    }
}
