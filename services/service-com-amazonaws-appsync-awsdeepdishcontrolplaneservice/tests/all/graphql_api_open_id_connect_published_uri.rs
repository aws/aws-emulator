// SPDX-License-Identifier: CC0-1.0

//! What a client reaches when it follows the discovery path on an `OPENID_CONNECT` GraphQL API:
//! create the API, read `GraphqlApi$uris`'s `GRAPHQL` entry ("The URIs.",
//! `smithy_query(query="shape",shape="GraphqlApi")`) AS PUBLISHED, and POST the GraphQL document
//! to exactly that URI with an OIDC identity token in the request's `authorization` field.
//!
//! The rule these cases assert, one level above the recorded case (one API created with
//! `openIDConnectConfig.iatTTL`, whose `uris.GRAPHQL` a client reads and POSTs `query P { ping }`
//! to):
//!
//! > Every URI a GraphQL API publishes is an address of the server that published it — a client
//! > that parses the published URI's scheme, host, port and path and dials exactly that reaches
//! > the API's own endpoint, and every reader of the API publishes the same one — where the
//! > API's authorization mode then decides the call: under `OPENID_CONNECT`, a token inside
//! > every window the STORED `openIDConnectConfig` bounds it to is served by the API's own
//! > resolver, and one past a window it states is refused `401` with exactly one
//! > `UnauthorizedException` error and no data, while an API stating no such window admits that
//! > very same token. `iatTTL` is "the number of milliseconds that a token is valid after it's
//! > issued to a user", measured from the token's `iat` (RFC 7519 §4.1.6), and its default of
//! > `0` bounds nothing.
//!
//! The rows span what the rule generalises over: WHICH reader published the address that was
//! dialed (the create, the single read, the listing), how FAR past its `iat` the presented token
//! sits (minted seconds ago, ten seconds, an hour, a day), the MAGNITUDE of the window the API
//! states (none at all, sub-second, seconds, minutes, an hour), whether the API states one at
//! all, and the form the token is presented in (bare, and under the RFC 6750 bearer scheme).
//!
//! Nothing asserted here is a value this emulator filled in for the assertion's sake: every
//! token is one `AdminInitiateAuth` minted for a user of a pool created through that service's
//! own operations, and the aged tokens no sign-in produces are signed with that same pool's own
//! identity-token key. The pool, the user and the sign-in are made through the sibling suite's
//! one reading of them ([`crate::graphql_api_user_pool_authorization`]), and the parsing of a
//! published URI through the publication suite's ([`crate::graphql_api_published_uri`]).

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::{appsync, loopback_port};
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_published_uri::split_uri;
use crate::graphql_api_user_pool_authorization::{
    claims_of, emulator, id_token, pool_signed_token, pool_with_client, seconds_ago, sign_in,
    user_in_groups,
};

/// The key `uris` and `dns` give the API's GraphQL endpoint, and the scheme that entry is an
/// address over: a client POSTs a document to it over HTTP.
const GRAPHQL_ENDPOINT: &str = "GRAPHQL";
const GRAPHQL_SCHEME: &str = "https";

/// The path the GraphQL endpoint serves, which the published URI carries; the service's own copy
/// is `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The operations whose reading of the API a row dials the published address of.
const CREATED: &str = "CreateGraphqlApi";
const FETCHED: &str = "GetGraphqlApi";
const LISTED: &str = "ListGraphqlApis";

/// The schema every API of this suite is created with: one field, so a call that reaches the
/// published address is answered with a value the API's own resolver produced.
const SCHEMA: &str = "schema { query: Query }\n\ntype Query {\n  ping: String\n}\n";

/// The document every row POSTs.
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

/// The window, in milliseconds, the APIs of the first table state — far wider than any step this
/// suite takes, so a row asserts the TTL rule and never how fast the host runs.
const GENEROUS_TTL_MS: i64 = 600_000;

/// How far past its `iat` the aged token of the first table sits: deep outside that window, and
/// still inside the token's own validity period.
const WELL_PAST_THE_TTL: Duration = Duration::from_secs(3_600);

/// The refusal a token this mode read and did not admit earns. The service's own copies are
/// `service::event_api::UNAUTHORIZED_ERROR_TYPE` and
/// `service::graphql_api::open_id_connect::UNAUTHORIZED_TOKEN_MESSAGE`.
const UNAUTHORIZED_STATUS: u16 = 401;
const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";
const UNAUTHORIZED_TOKEN_MESSAGE: &str = "Unauthorized";

/// What one row owes the call it makes.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The call is served and `ping` resolves to the resolver's own marker.
    Served,
    /// The call is refused, reaching no resolver.
    Refused,
}

/// One created `OPENID_CONNECT` API: its id, and the `uris.GRAPHQL` each reader of it published.
struct PublishedApi {
    api_id: String,
    published: BTreeMap<&'static str, String>,
}

impl PublishedApi {
    /// The URI the reader `source` published, exactly as published.
    fn uri(&self, source: &str) -> &str {
        self.published
            .get(source)
            .map(String::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "{source} must have published a GraphQL uri for {}",
                    self.api_id
                )
            })
    }
}

/// The identity of one provider of this emulator: the pool, its app client, and the tokens one
/// sign-in of its user minted.
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

/// Create one `OPENID_CONNECT` GraphQL API over `configured`, install [`SCHEMA`] and the one
/// resolver its field is answered by, and collect the `uris.GRAPHQL` every reader of the API
/// publishes — asserting that all three agree, so a client that re-reads the API dials the same
/// server the create announced.
async fn create_api(router: &MultiServiceRouter, name: &str, configured: Value) -> PublishedApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("oidc-published-{name}"),
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

    // The single read and the listing of the same API, so the address every reader publishes is
    // compared with the one the create announced.
    let (status, fetched) = appsync(router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {name:?}: GetGraphqlApi must answer 200, got {fetched}"
    );
    // The configuration the mode judges a call against is the STORED one.
    assert_eq!(
        fetched["graphqlApi"]["openIDConnectConfig"]["iatTTL"],
        configured
            .get("iatTTL")
            .cloned()
            .unwrap_or_else(|| json!(0)),
        "case {name:?}: GetGraphqlApi must report the iatTTL the create stored (0 where the \
         create asked for none), got {fetched}"
    );
    let (status, listed) = appsync(router, "GET", "/v1/apis", None).await;
    assert_eq!(
        status, 200,
        "case {name:?}: ListGraphqlApis must answer 200, got {listed}"
    );
    let among = listed["graphqlApis"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .find(|api| api["apiId"].as_str() == Some(api_id.as_str()))
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: the listing must hold the API, got {listed}"));

    let mut published = BTreeMap::new();
    for (source, api) in [
        (CREATED, &created["graphqlApi"]),
        (FETCHED, &fetched["graphqlApi"]),
        (LISTED, &among),
    ] {
        let label = format!("{name} ({source})");
        let uri = api["uris"][GRAPHQL_ENDPOINT]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label:?}: the API must publish its GraphQL uri, got {api}")
            })
            .to_string();
        // What a published address is: this emulator's own, over the scheme and path the
        // endpoint is served under, and built over the endpoint's own `dns` record. A host
        // belonging to another server — an AWS-owned name among them — fails here.
        let (scheme, authority, path) = split_uri(&label, &uri);
        assert_eq!(
            (scheme.as_str(), path.as_str()),
            (GRAPHQL_SCHEME, GRAPHQL_PATH),
            "case {label:?}: expected the scheme and path the GraphQL endpoint is served over, \
             got {uri:?}"
        );
        assert_eq!(
            Some(::service_common::data_plane_addressing::host_only(&authority)),
            api["dns"][GRAPHQL_ENDPOINT].as_str(),
            "case {label:?}: the endpoint's own dns record must be the bare host of its uri, \
             got {api}"
        );
        assert!(
            !uri.contains("amazonaws.com"),
            "case {label:?}: a published address is this emulator's own, never a host of another \
             server, got {uri:?}"
        );
        assert!(
            loopback_port(&label, &authority) > 0,
            "case {label:?}: expected the port the endpoint is served on, got {uri:?}"
        );
        published.insert(source, uri);
    }
    assert_eq!(
        published.get(CREATED),
        published.get(FETCHED),
        "case {name:?}: every reader of the API must publish the same GraphQL uri, got \
         {published:?}"
    );
    assert_eq!(
        published.get(CREATED),
        published.get(LISTED),
        "case {name:?}: every reader of the API must publish the same GraphQL uri, got \
         {published:?}"
    );

    PublishedApi { api_id, published }
}

/// POST [`DOCUMENT`] to `published` exactly as published — parsing the URI and dialing what it
/// names — presenting `presented` in the request's `authorization` field.
async fn post_as_published(case: &str, published: &str, presented: &str) -> (u16, Value) {
    let (_, authority, path) = split_uri(case, published);
    let (status, _, document) = try_post_answer(
        case,
        &authority,
        &path,
        "POST",
        &json!({"query": DOCUMENT}),
        &[("authorization", presented)],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the uri this emulator published, {published:?}, must be an address it \
             answers on"
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

/// `token`'s claim set with its `iat` moved `ago` into the past, signed by the pool's own
/// identity-token key: the one token no sign-in mints, since a pool issues none that was issued
/// before now.
fn aged_by(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    token: &str,
    ago: Duration,
) -> String {
    let mut claims = claims_of(token);
    claims["iat"] = json!(seconds_ago(ago));
    pool_signed_token(engine, user_pool_id, &claims)
}

/// RULE: a client that parses the URI the API published and dials exactly that reaches the API's
/// own GraphQL endpoint, where the stored `openIDConnectConfig` decides the call — a token inside
/// the window `iatTTL` bounds it to is served, the same token past that window is refused `401`
/// `UnauthorizedException` reaching no resolver, and a twin API stating no window admits it.
#[tokio::test]
async fn the_published_uri_is_where_the_apis_iat_ttl_decides_a_call() {
    let (router, engine) = emulator();
    let issuer = provider(&router, "published-iat-ttl").await;
    let minted = id_token(&issuer.tokens);
    let aged = aged_by(&engine, &issuer.user_pool_id, &minted, WELL_PAST_THE_TTL);

    let bounded = create_api(
        &router,
        "bounded",
        json!({
            "issuer": issuer.claimed(),
            "clientId": issuer.client_id,
            "iatTTL": GENEROUS_TTL_MS,
        }),
    )
    .await;
    let twin = create_api(
        &router,
        "twin",
        json!({"issuer": issuer.claimed(), "clientId": issuer.client_id}),
    )
    .await;
    // A token minted AFTER the aged one, which the window admits again.
    let minted_again = id_token(
        &sign_in(&router, &issuer.user_pool_id, &issuer.client_id, "alice").await,
    );

    /// One row: which API's published uri is dialed, which reader published it, and what the
    /// call presents.
    struct Row<'api> {
        case: &'static str,
        api: &'api PublishedApi,
        source: &'static str,
        presented: String,
        expected: Expected,
    }
    let rows: &[Row<'_>] = &[
        Row {
            case: "a token minted seconds ago, at the uri the create published",
            api: &bounded,
            source: CREATED,
            presented: minted.clone(),
            expected: Expected::Served,
        },
        Row {
            case: "that same token issued long before the stated window",
            api: &bounded,
            source: CREATED,
            presented: aged.clone(),
            expected: Expected::Refused,
        },
        Row {
            case: "the aged token on the twin api, which states no window at all",
            api: &twin,
            source: CREATED,
            presented: aged.clone(),
            expected: Expected::Served,
        },
        Row {
            case: "a newly minted token, at the uri the single read published",
            api: &bounded,
            source: FETCHED,
            presented: minted_again.clone(),
            expected: Expected::Served,
        },
        Row {
            case: "the fresh token under the bearer scheme, at the uri the listing published",
            api: &bounded,
            source: LISTED,
            presented: format!("Bearer {minted}"),
            expected: Expected::Served,
        },
        Row {
            case: "the aged token, at the uri the listing published",
            api: &bounded,
            source: LISTED,
            presented: aged.clone(),
            expected: Expected::Refused,
        },
        Row {
            case: "the aged token on the twin api, at the uri its single read published",
            api: &twin,
            source: FETCHED,
            presented: aged.clone(),
            expected: Expected::Served,
        },
    ];

    for row in rows {
        let (status, answered) =
            post_as_published(row.case, row.api.uri(row.source), &row.presented).await;
        assert_answered(row.case, row.expected, status, &answered);
    }
}

/// RULE: the window `iatTTL` states is a span in MILLISECONDS measured from the token's `iat`,
/// whatever its magnitude — a call made inside it is served and one made past it refused — and
/// the member's default of `0` states no window at all.
#[tokio::test]
async fn a_stated_window_bounds_the_token_whatever_its_magnitude() {
    let (router, engine) = emulator();
    let issuer = provider(&router, "published-window-magnitudes").await;
    let minted = id_token(&issuer.tokens);

    // The windows differ by three orders of magnitude, and include the member's default. Each is
    // one API, because the window a call is judged against is the one its own API stored.
    let windows: &[(&'static str, i64)] = &[
        ("no window at all", 0),
        ("a sub-second window", 500),
        ("a window of an hour", 3_600_000),
        ("a generous window", GENEROUS_TTL_MS),
    ];
    let mut apis: BTreeMap<i64, PublishedApi> = BTreeMap::new();
    for (name, window) in windows {
        let mut configured = json!({
            "issuer": issuer.claimed(),
            "clientId": issuer.client_id,
        });
        if *window != 0 {
            configured["iatTTL"] = json!(window);
        }
        apis.insert(*window, create_api(&router, name, configured).await);
    }

    // Each row states the window, how far past its `iat` the presented token sits, and what the
    // call owes. Every "inside" row presents a token under a window far wider than anything this
    // suite's steps take, so no row's outcome turns on how fast the host runs.
    let rows: &[(&str, i64, Duration, Expected)] = &[
        (
            "a day-old token where no window is stated",
            0,
            Duration::from_secs(86_400),
            Expected::Served,
        ),
        (
            "a ten-second-old token against a sub-second window",
            500,
            Duration::from_secs(10),
            Expected::Refused,
        ),
        (
            "a day-old token against a window of an hour",
            3_600_000,
            Duration::from_secs(86_400),
            Expected::Refused,
        ),
        (
            "a freshly minted token against a window of an hour",
            3_600_000,
            Duration::ZERO,
            Expected::Served,
        ),
        (
            "an hour-old token against a generous window of minutes",
            GENEROUS_TTL_MS,
            WELL_PAST_THE_TTL,
            Expected::Refused,
        ),
        (
            "a freshly minted token against that generous window",
            GENEROUS_TTL_MS,
            Duration::ZERO,
            Expected::Served,
        ),
    ];

    for (case, window, age, expected) in rows {
        let api = apis
            .get(window)
            .unwrap_or_else(|| panic!("case {case:?}: no API states a window of {window}"));
        let presented = if *age == Duration::ZERO {
            minted.clone()
        } else {
            aged_by(&engine, &issuer.user_pool_id, &minted, *age)
        };
        let (status, answered) = post_as_published(case, api.uri(CREATED), &presented).await;
        assert_answered(case, *expected, status, &answered);
    }
}
