// SPDX-License-Identifier: CC0-1.0

//! What an `OPENID_CONNECT` GraphQL API serves a token the CLOCK carries past the window its
//! `openIDConnectConfig` states, over the API's own published GraphQL endpoint.
//!
//! The rule these cases assert, one level above the recorded case (one API created with
//! `openIDConnectConfig.iatTTL`, presented a token of a Cognito user pool first while it was
//! fresh and again once it was older than that window):
//!
//! > A call on a GraphQL API that set up the `OPENID_CONNECT` mode is served only while the
//! > instant the stored configuration's window is measured FROM is less than that many
//! > milliseconds in the past, judged at the moment the call is made: `iatTTL` is "the number of
//! > milliseconds that a token is valid after it's issued to a user", measured from the token's
//! > `iat` (RFC 7519 §4.1.6), and `authTTL` "the number of milliseconds that a token is valid
//! > after being authenticated", measured from its `auth_time`. So one and the same token,
//! > admitted earlier by one of those windows, authorizes nothing once the clock has carried it
//! > past that window, even though its own validity period (`exp`) has not elapsed: the call is
//! > refused `401` with exactly one `UnauthorizedException` error carrying `Unauthorized` and no
//! > data, reaching no resolver. A configuration stating no such window (the member's default of
//! > `0`) bounds nothing, so the very same token keeps being served there, and nothing about the
//! > API changes by a call being refused: the window is a property of the TOKEN's age, read
//! > from the stored configuration on every call.
//!
//! What no sibling suite covers, and what these cases are for: every other aged token of this
//! crate is one whose `iat` was WRITTEN into the past and signed again, which pins the
//! comparison but not the clock it is taken against. Here the token is one `AdminInitiateAuth`
//! really minted, presented unchanged, and the only thing that moves is time — so a window that
//! is configuration-only would serve every one of these rows.
//!
//! The rows span what the rule generalises over: which claim the window is measured from
//! (`iat`, `auth_time`), the MAGNITUDE of the window the API states (one the clock crosses
//! while the suite watches, one minutes wide, none at all), the FORM the configuration names the
//! issuer in (the regional form of the pool, the value the token itself claims), the token KIND
//! (an identity token, an access token of the same sign-in) and how it is presented (bare, under
//! the RFC 6750 bearer scheme).
//!
//! Every wait here is a wait on the CONDITION — looking again until the answer changes or a
//! deadline far past any window this suite states passes — so no row's outcome turns on how fast
//! the host runs: a row asserting a call is SERVED presents a token under a window minutes wide,
//! and a row asserting one is REFUSED is only ever reached after the clock has already been
//! observed to cross the window it states.
//!
//! The pool, the user and the sign-in are made through the sibling suite's one reading of them
//! ([`crate::graphql_api_user_pool_authorization`]), and the schema installation and the socket
//! client through the resolver suite's ([`crate::graphql_api_mapping_template_resolvers`]).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_user_pool_authorization::{
    claims_of, emulator, id_token, pool_with_client, sign_in, user_in_groups,
};

/// The Region the pools of this suite are created in and every call of it is signed for.
const REGION: &str = "us-west-2";

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The schema every API of this suite is created with: one field, so a call that reaches the
/// endpoint is answered with a value the API's own resolver produced.
const SCHEMA: &str = "schema { query: Query }\n\ntype Query {\n  ping: String\n}\n";

/// The document every call of this suite POSTs.
const DOCUMENT: &str = "query P { ping }";

/// The value the field's response function returns, written in the resolver's own code and
/// nowhere in the request: an answer carrying it came from the stored resolver.
const PING_MARKER: &str = "pong";

/// The name of the local data source the field's resolver resolves against: `NONE` is "There is
/// no data source", so the response function's own value is the field's.
const DATA_SOURCE: &str = "local";

/// The `runtime` the resolver is defined under. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The refusal a token this mode read and did not admit earns. The service's own copies are
/// `service::event_api::UNAUTHORIZED_ERROR_TYPE` and
/// `service::graphql_api::open_id_connect::UNAUTHORIZED_TOKEN_MESSAGE`.
const UNAUTHORIZED_STATUS: u16 = 401;
const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";
const UNAUTHORIZED_TOKEN_MESSAGE: &str = "Unauthorized";

/// The window, in milliseconds, whose crossing the suite WATCHES: short enough that the clock
/// carries a token minted now past it while the suite looks, and the only window any row of this
/// suite waits on.
const CROSSED_WINDOW_MS: i64 = 2_000;

/// The window, in milliseconds, of the APIs whose rows assert a call is SERVED: minutes wide, so
/// far past anything this suite's steps take that such a row asserts the rule and never how fast
/// the host runs.
const GENEROUS_WINDOW_MS: i64 = 600_000;

/// How long the suite is willing to look for the clock to carry a token past
/// [`CROSSED_WINDOW_MS`], and how often it looks again. The deadline is far past that window, so
/// a machine slower than this one only goes round the loop more often.
const CROSSING_DEADLINE: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What one row owes the call it makes.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The call is served and `ping` resolves to the resolver's own marker.
    Served,
    /// The call is refused, reaching no resolver.
    Refused,
}

/// One provider of this emulator: the pool, its app client, and the tokens one sign-in of its
/// user minted.
struct Provider {
    user_pool_id: String,
    client_id: String,
    tokens: Value,
}

impl Provider {
    /// The `iss` the tokens this provider minted claim, which is the issuer a configuration
    /// discovered from it holds.
    fn claimed(&self) -> String {
        claims_of(&id_token(&self.tokens))["iss"]
            .as_str()
            .unwrap_or_else(|| panic!("a minted token must claim an issuer"))
            .to_string()
    }

    /// The regional form of the same provider — the one the Amazon Cognito developer guide
    /// documents ("`https://cognito-idp.[region].amazonaws.com/[userPoolId]`"), which a
    /// configuration written against AWS holds for the pool.
    fn regional(&self) -> String {
        format!(
            "https://cognito-idp.{REGION}.amazonaws.com/{}",
            self.user_pool_id
        )
    }
}

/// Create one pool with one app client and one confirmed user, and sign that user in.
async fn provider(router: &MultiServiceRouter, name: &str) -> Provider {
    let (user_pool_id, client_id) = pool_with_client(router, name).await;
    user_in_groups(router, &user_pool_id, "alice", &[]).await;
    let tokens = sign_in(router, &user_pool_id, &client_id, "alice").await;
    Provider {
        user_pool_id,
        client_id,
        tokens,
    }
}

/// The access token of a sign-in, which carries its own `iat` and `auth_time`.
fn access_token(result: &Value) -> String {
    result["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in must mint an AccessToken, got {result}"))
        .to_string()
}

/// One created `OPENID_CONNECT` API: its id, and the address its GraphQL endpoint is served on.
struct Api {
    api_id: String,
    graphql: String,
}

/// Create one `OPENID_CONNECT` GraphQL API over `configured`, install [`SCHEMA`] and the one
/// resolver its field is answered by, and read the API back — because the configuration a call
/// is judged against is the STORED one.
async fn create_api(router: &MultiServiceRouter, name: &str, configured: Value) -> Api {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("oidc-age-{name}"),
            "authenticationType": "OPENID_CONNECT",
            "openIDConnectConfig": configured,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateGraphqlApi must answer 200, got {status} and {created}"
    );
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: the create must mint an apiId, got {created}"))
        .to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "GRAPHQL",
    );
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateDataSource must answer 200, got {source}"
    );
    let (status, resolver) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": "ping",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            "code": format!(
                "export function request(ctx) {{ return {{}}; }}\n\
                 export function response(ctx) {{ return \"{PING_MARKER}\"; }}"
            ),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateResolver must answer 200, got {resolver}"
    );
    // Every member the create asked for, and the two windows at the `0` their model default
    // states for a create that asked for neither.
    let mut expected = configured.clone();
    for (member, default) in [("iatTTL", json!(0)), ("authTTL", json!(0))] {
        if expected.get(member).is_none() {
            expected[member] = default;
        }
    }
    assert_eq!(
        stored_configuration(router, &api_id).await,
        expected,
        "case {name:?}: GetGraphqlApi must report the openIDConnectConfig the create stored"
    );
    Api { api_id, graphql }
}

/// The `openIDConnectConfig` `api_id` holds now, as its single read reports it.
async fn stored_configuration(router: &MultiServiceRouter, api_id: &str) -> Value {
    let (status, fetched) = appsync(router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetGraphqlApi must answer 200 for {api_id}, got {fetched}"
    );
    fetched["graphqlApi"]["openIDConnectConfig"].clone()
}

/// POST [`DOCUMENT`] to `api`'s published GraphQL endpoint, presenting `presented` in the
/// request's `authorization` field.
async fn post_graphql(case: &str, api: &Api, presented: &str) -> (u16, Value) {
    let (status, _, document) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &json!({"query": DOCUMENT}),
        &[("authorization", presented)],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    });
    (status, document)
}

/// Hold one answer to what the row owes it.
fn assert_answered(case: &str, expected: Expected, status: u16, answered: &Value) {
    match expected {
        Expected::Served => assert_eq!(
            (status, answered),
            (200, &json!({"data": {"ping": PING_MARKER}})),
            "case {case:?}: the call must be served by the API's own resolver, got {status} and \
             {answered}"
        ),
        Expected::Refused => {
            assert_eq!(
                (status, &answered["errors"]),
                (
                    UNAUTHORIZED_STATUS,
                    &json!([{
                        "errorType": UNAUTHORIZED_ERROR_TYPE,
                        "message": UNAUTHORIZED_TOKEN_MESSAGE,
                    }]),
                ),
                "case {case:?}: expected {UNAUTHORIZED_STATUS} and exactly one \
                 {UNAUTHORIZED_ERROR_TYPE:?} error carrying {UNAUTHORIZED_TOKEN_MESSAGE:?}, got \
                 {status} and {answered}"
            );
            assert!(
                answered.get("data").is_none_or(Value::is_null),
                "case {case:?}: a refused call must reach no resolver, got {answered}"
            );
        }
    }
}

/// Present `presented` to `api` again and again until the call is REFUSED — the clock carrying
/// the token past the window that API stores — or [`CROSSING_DEADLINE`] passes, and answer that
/// refusal.
///
/// The wait is on the CONDITION, never on a span: an answer that is still being served is the
/// token still being inside the window, so the loop looks again.
async fn poll_until_refused(case: &str, api: &Api, presented: &str) -> (u16, Value) {
    let deadline = tokio::time::Instant::now() + CROSSING_DEADLINE;
    loop {
        let (status, answered) = post_graphql(case, api, presented).await;
        if status != 200 {
            return (status, answered);
        }
        assert_eq!(
            (status, &answered),
            (200, &json!({"data": {"ping": PING_MARKER}})),
            "case {case:?}: while the token is inside the window the call must be served by the \
             API's own resolver, got {status} and {answered}"
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the clock must carry the token past the {CROSSED_WINDOW_MS} ms \
             window its API states, last answer {status} and {answered}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: a token an `OPENID_CONNECT` API admitted authorizes nothing once the clock has carried
/// it past the window the API's stored configuration measures from the claim that window names —
/// `iat` for `iatTTL`, `auth_time` for `authTTL` — while its own validity period is still open;
/// the call is then refused `401` with one `UnauthorizedException`/`Unauthorized` error and no
/// data, and an API stating a window minutes wide, or none at all, serves that very same token.
#[tokio::test]
async fn the_clock_carrying_a_token_past_a_stated_window_refuses_the_call() {
    let (router, _engine) = emulator();
    let issuer = provider(&router, "clock-crossed-window").await;

    // One API per configuration a row judges the same token against: the recorded shape (the
    // regional form of the issuer, the token's own client, a window the clock crosses), the same
    // window stated over the issuer the token itself claims, one that bounds AUTHENTICATION
    // instead of issuance, one whose window is minutes wide, and a twin stating none at all.
    let bounded = create_api(
        &router,
        "crossed",
        json!({
            "issuer": issuer.regional(),
            "clientId": issuer.client_id,
            "iatTTL": CROSSED_WINDOW_MS,
        }),
    )
    .await;
    let bounded_as_claimed = create_api(
        &router,
        "crossed-claimed-issuer",
        json!({
            "issuer": issuer.claimed(),
            "clientId": issuer.client_id,
            "iatTTL": CROSSED_WINDOW_MS,
        }),
    )
    .await;
    let authenticated = create_api(
        &router,
        "crossed-auth",
        json!({
            "issuer": issuer.regional(),
            "clientId": issuer.client_id,
            "authTTL": CROSSED_WINDOW_MS,
        }),
    )
    .await;
    let generous = create_api(
        &router,
        "generous",
        json!({
            "issuer": issuer.regional(),
            "clientId": issuer.client_id,
            "iatTTL": GENEROUS_WINDOW_MS,
        }),
    )
    .await;
    let unbounded = create_api(
        &router,
        "twin",
        json!({"issuer": issuer.regional(), "clientId": issuer.client_id}),
    )
    .await;

    // The tokens the rows judge, minted once every API above is installed: nothing of the setup
    // sits between minting them and the calls below, so the first call of the loop that follows
    // is made while the token is still inside the window it is about to cross.
    let tokens = sign_in(&router, &issuer.user_pool_id, &issuer.client_id, "alice").await;
    let minted = id_token(&tokens);
    let access = access_token(&tokens);
    // The token's own validity period, which stays open for the whole test: what a row past a
    // window asserts is the window and never an expiry.
    let expires_at = claims_of(&minted)["exp"]
        .as_i64()
        .unwrap_or_else(|| panic!("a minted token must claim an exp"));

    // The token is live and admitted where the window is minutes wide: the rows below are about
    // the one that changes afterwards, which is the token's AGE.
    let case = "the minted token under a window minutes wide";
    let (status, answered) = post_graphql(case, &generous, &minted).await;
    assert_answered(case, Expected::Served, status, &answered);

    // The crossing itself: the same token, unchanged, presented to the API whose window the
    // clock crosses, until the answer changes.
    let case = "the minted token as the clock crosses the window its API states";
    let (status, answered) = poll_until_refused(case, &bounded, &minted).await;
    assert_answered(case, Expected::Refused, status, &answered);

    /// One row: which API the call is made on, what it presents, and what it owes.
    struct Row<'api> {
        case: &'static str,
        api: &'api Api,
        presented: String,
        expected: Expected,
    }
    // Every row is reached only after the crossing above, so the identity token's `iat` and
    // `auth_time` are both already past the crossed window and still minutes inside the
    // generous one.
    let rows: &[Row<'_>] = &[
        Row {
            case: "the same token again on the API whose window it is past",
            api: &bounded,
            presented: minted.clone(),
            expected: Expected::Refused,
        },
        Row {
            case: "that token under the bearer scheme on the same API",
            api: &bounded,
            presented: format!("Bearer {minted}"),
            expected: Expected::Refused,
        },
        Row {
            case: "the access token of the same sign-in on that API",
            api: &bounded,
            presented: access.clone(),
            expected: Expected::Refused,
        },
        Row {
            case: "the same token on an API naming the issuer as the token claims it",
            api: &bounded_as_claimed,
            presented: minted.clone(),
            expected: Expected::Refused,
        },
        Row {
            case: "the same token on an API bounding authentication instead of issuance",
            api: &authenticated,
            presented: minted.clone(),
            expected: Expected::Refused,
        },
        Row {
            case: "the access token on that same API",
            api: &authenticated,
            presented: access.clone(),
            expected: Expected::Refused,
        },
        Row {
            case: "the same token on the API whose window is minutes wide",
            api: &generous,
            presented: minted.clone(),
            expected: Expected::Served,
        },
        Row {
            case: "the same token on the twin API stating no window at all",
            api: &unbounded,
            presented: minted.clone(),
            expected: Expected::Served,
        },
        Row {
            case: "the access token on that twin API",
            api: &unbounded,
            presented: access.clone(),
            expected: Expected::Served,
        },
    ];

    for row in rows {
        let (status, answered) = post_graphql(row.case, row.api, &row.presented).await;
        assert_answered(row.case, row.expected, status, &answered);
    }

    // The token the rows above were refused for is still inside its own validity period, so the
    // only bound that refused it is the window its API states.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    assert!(
        i64::try_from(now).unwrap_or(i64::MAX) < expires_at,
        "the refused token must still be inside its own validity period, now {now} against an \
         exp of {expires_at}"
    );
}

/// RULE: the window is read from the stored configuration on every call and bounds the TOKEN's
/// age, so a call refused for being past it changes nothing a later caller sees — the API reports
/// the same configuration, every later call presenting that token is refused the same way, and a
/// token the provider mints afterwards is admitted again.
#[tokio::test]
async fn a_call_refused_for_its_age_changes_nothing_a_later_caller_sees() {
    let (router, _engine) = emulator();
    let issuer = provider(&router, "age-refusal-changes-nothing").await;

    let bounded = create_api(
        &router,
        "repeated",
        json!({
            "issuer": issuer.regional(),
            "clientId": issuer.client_id,
            "iatTTL": CROSSED_WINDOW_MS,
        }),
    )
    .await;
    let generous = create_api(
        &router,
        "repeated-generous",
        json!({
            "issuer": issuer.regional(),
            "clientId": issuer.client_id,
            "iatTTL": GENEROUS_WINDOW_MS,
        }),
    )
    .await;
    let before = stored_configuration(&router, &bounded.api_id).await;

    // Minted once both APIs are installed, so the first call below is made while the token is
    // still inside the window it is about to cross.
    let crossing = sign_in(&router, &issuer.user_pool_id, &issuer.client_id, "alice").await;
    let minted = id_token(&crossing);
    let case = "the token as the clock crosses the window";
    let (status, answered) = poll_until_refused(case, &bounded, &minted).await;
    assert_answered(case, Expected::Refused, status, &answered);

    // The rows differ along HOW MANY times the same past-the-window token has been presented:
    // the refusal is the token's age being read again, not a decision the API recorded.
    for again in 1..=3 {
        let case = format!("presenting the same past-the-window token again (round {again})");
        let (status, answered) = post_graphql(&case, &bounded, &minted).await;
        assert_answered(&case, Expected::Refused, status, &answered);
    }
    assert_eq!(
        stored_configuration(&router, &bounded.api_id).await,
        before,
        "the refusals must leave the API reporting the configuration it stored"
    );

    // A token the same provider mints afterwards is a token whose own age is inside that window
    // again. It is admitted where the window is minutes wide, which asks nothing of how fast
    // this host runs; the API that refused the older token is read back unchanged above.
    let later = sign_in(&router, &issuer.user_pool_id, &issuer.client_id, "alice").await;
    let fresh = id_token(&later);
    assert_ne!(
        fresh, minted,
        "a later sign-in must mint a token of its own, got the earlier one back"
    );
    let case = "a token minted after the refusals, under a window minutes wide";
    let (status, answered) = post_graphql(case, &generous, &fresh).await;
    assert_answered(case, Expected::Served, status, &answered);
}
