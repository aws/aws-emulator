// SPDX-License-Identifier: CC0-1.0

//! An AppSync GraphQL API gated by a Lambda AUTHORIZER whose decisions are cached for the window
//! the API CONFIGURED, reached the way a client reaches it: by dialing the address the API
//! published for its GraphQL endpoint.
//!
//! RULE — every address a GraphQL API publishes under `GraphqlApi$uris` is an address of the
//! server that published it, so a client that parses `uris.GRAPHQL` and dials exactly that
//! reaches the API's own endpoint; and a request so delivered is decided by the API's Lambda
//! authorizer, whose decision is reused only while a positive window of it is live. The window
//! in force for a function that states none of its own is the API's configured one: "the number
//! of seconds a response should be cached for. The default is 0 seconds, which disables caching.
//! If you don't specify a value for `authorizerResultTtlInSeconds`, the default value is used.
//! The maximum value is one hour (3600 seconds)"
//! (`smithy_query(query="shape", shape="LambdaAuthorizerConfig")`). So a window of zero — stated
//! or left to that default — is one authorizer invocation per request, a positive one is a
//! single invocation shared by every request inside it, and a decision is never shared by two
//! different tokens.
//!
//! Two services have to agree for the rule to hold — Lambda runs the authorizer and AppSync
//! publishes the endpoint, invokes the authorizer for what a request presented and binds what it
//! published onto the resolver's context — so this is the suite that can pin it.
//!
//! The rows span what the rule generalises over: the configured window, at the minimum of its
//! published range (zero), left out of the configuration altogether so the documented default
//! stands, well inside the range, and at the published maximum of one hour. Each row reads the
//! decision its requests were served under through a VTL mapping template, which is a reader of
//! `$ctx.identity.resolverContext` independent of the one the sibling APPSYNC_JS suite drives.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appsync_lambda_data_source_and_authorizer::{
    appsync_ok, build_router, create_function, post_published_uri, published_parts,
};

/// The path a GraphQL API's `GRAPHQL` endpoint serves, and the scheme a client addresses it over.
const GRAPHQL_PATH: &str = "/graphql";
const GRAPHQL_SCHEME: &str = "https";

/// The host every socket this emulator opens for a resource is bound on, which a published
/// address therefore names.
const OWN_HOST: &str = "127.0.0.1";

/// The published bounds of `authorizerResultTtlInSeconds` (`@range` 0..3600, `@default` 0;
/// `smithy_query(query="shape", shape="LambdaAuthorizerConfig")`).
const TTL_MIN: u64 = 0;
const TTL_MAX: u64 = 3600;

/// A configured window well inside that range, so a positive window is exercised away from its
/// bound as well as at it.
const TTL_INSIDE: u64 = 300;

/// The data source every resolver here resolves against: `NONE` is "there is no data source", so
/// nothing but the resolver's own definition decides the field.
const LOCAL_DATA_SOURCE: &str = "local";

/// How long a poll loop waits for a state another service owes, and how often it looks again.
/// Generous: what is under test is what the endpoint answers, never how fast the host reaches a
/// transition.
const BUDGET: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The status a schema creation settles at once the document has been installed.
const SCHEMA_CREATED: &str = "SUCCESS";

/// The schema every API of this suite is created with: one field, which carries the id of the
/// authorizer invocation the request was decided by.
const SCHEMA: &str = "schema { query: Query }\n\ntype Query {\n  decision: String\n}\n";

/// The authorizer every API of this suite is gated by.
///
/// It authorizes every call and publishes the id of its OWN invocation — so a row can tell one
/// invocation from several — and states NO `ttlOverride` at all, which leaves the window the API
/// configured in force.
const AUTHORIZER_HANDLER: &str = r#"
def handler(event, context):
    return {
        "isAuthorized": True,
        "resolverContext": {"decision": context.aws_request_id},
    }
"#;

/// The template pair that reads the decision a request was served under: the request half writes
/// what the authorizer published as the payload a `NONE` data source hands back, and the response
/// half answers it.
const DECISION_REQUEST_TEMPLATE: &str = "{\"version\": \"2018-05-29\", \"payload\": \
                                         $util.toJson($ctx.identity.resolverContext.decision)}";
const DECISION_RESPONSE_TEMPLATE: &str = "$util.toJson($ctx.result)";

/// One row: the window the API configures for its authorizer's decisions, and how many
/// invocations of the authorizer the row's requests must come from.
struct Row {
    case: &'static str,
    /// The configured window, or `None` for a configuration that leaves the member out.
    configured: Option<u64>,
    invocations: usize,
}

/// How many requests each row makes presenting one token.
const REQUESTS: usize = 3;

/// A second token each row also presents, whose decision is its own.
const OTHER_TOKEN: &str = "another-caller-token";

#[tokio::test]
async fn a_published_endpoint_reuses_its_authorizer_decision_only_inside_the_configured_window() {
    let router = build_router();
    let authorizer = create_function(&router, "configured-ttl-authorizer", AUTHORIZER_HANDLER).await;

    // The rows differ along the configured window: each published bound of its range, the
    // documented default where the configuration leaves the member out, and a value inside the
    // range.
    let rows: Vec<Row> = vec![
        Row {
            case: "a configured window of zero, which disables caching",
            configured: Some(TTL_MIN),
            invocations: REQUESTS,
        },
        Row {
            case: "a configuration leaving the window out, which is the documented default",
            configured: None,
            invocations: REQUESTS,
        },
        Row {
            case: "a configured window inside the published range",
            configured: Some(TTL_INSIDE),
            invocations: 1,
        },
        Row {
            case: "a configured window of the published maximum",
            configured: Some(TTL_MAX),
            invocations: 1,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let graphql_uri = create_api(
            &router,
            &format!("configured-ttl-{index}"),
            &authorizer,
            row.configured,
        )
        .await;
        let token = format!("caller-token-{index}");

        // Every request is delivered to the address the API PUBLISHED, and each one reports the
        // authorizer invocation it was decided by.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for attempt in 0..REQUESTS {
            let decision = decision_read(case, attempt, &graphql_uri, &token).await;
            seen.insert(decision);
        }
        assert_eq!(
            seen.len(),
            row.invocations,
            "case {case:?}: expected {REQUESTS} requests to be decided by {} authorizer \
             invocation(s), got the decisions {seen:?}",
            row.invocations
        );

        // A decision is the decision of the token it was taken for and of no other, whatever its
        // window: another caller presenting another token is decided on its own.
        let other = decision_read(case, REQUESTS, &graphql_uri, OTHER_TOKEN).await;
        assert!(
            !seen.contains(&other),
            "case {case:?}: expected another token to be decided by an invocation of its own, \
             got {other:?} among {seen:?}"
        );
    }
}

/// POST the one document of [`SCHEMA`] to the published URL, presenting `token` the way AppSync
/// reads an authorizer's input, and answer the decision the request was served under.
async fn decision_read(case: &str, attempt: usize, graphql_uri: &str, token: &str) -> String {
    let (status, answered) = post_published_uri(
        case,
        graphql_uri,
        &json!({"query": "query { decision }"}),
        &[("authorization", token.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} request {attempt}: expected the published endpoint to answer HTTP 200, \
         got {status} and {answered}"
    );
    assert!(
        answered.get("errors").is_none(),
        "case {case:?} request {attempt}: expected no errors member, got {answered}"
    );
    answered["data"]["decision"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?} request {attempt}: expected the field to carry the authorizer \
                 invocation the request was decided by, got {answered}"
            )
        })
        .to_string()
}

/// Create one `AWS_LAMBDA` API gated by `function_arn` whose authorizer's decisions are cached
/// for `configured` seconds (for the documented default where that is `None`), install
/// [`SCHEMA`], attach the one resolver that reads the decision, and answer the URL the API
/// published for its GraphQL endpoint.
///
/// The published address is GRADED here rather than assumed: it is the address a client dials,
/// so this is what makes every case below a case about what a customer of this emulator reaches.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    function_arn: &str,
    configured: Option<u64>,
) -> String {
    let mut lambda_authorizer_config = json!({"authorizerUri": function_arn});
    if let Some(seconds) = configured {
        lambda_authorizer_config["authorizerResultTtlInSeconds"] = json!(seconds);
    }
    let created = appsync_ok(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": name,
            "authenticationType": "AWS_LAMBDA",
            "lambdaAuthorizerConfig": lambda_authorizer_config,
        })),
    )
    .await;
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql_uri = published_graphql_uri(name, api);

    // Every later read publishes the same address, so a client that reads the API back dials
    // what the create announced.
    let fetched = appsync_ok(router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        fetched["graphqlApi"]["uris"], api["uris"],
        "case {name:?}: expected GetGraphqlApi to publish the same uris, got {fetched}"
    );

    appsync_ok(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": ::aws_smithy_types::base64::encode(SCHEMA)})),
    )
    .await;
    let deadline = Instant::now() + BUDGET;
    loop {
        let settled = appsync_ok(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        if settled["status"].as_str() == Some(SCHEMA_CREATED) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {name:?}: expected the schema creation to be performed, last read {settled}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    appsync_ok(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": LOCAL_DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    appsync_ok(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": "decision",
            "kind": "UNIT",
            "dataSourceName": LOCAL_DATA_SOURCE,
            "requestMappingTemplate": DECISION_REQUEST_TEMPLATE,
            "responseMappingTemplate": DECISION_RESPONSE_TEMPLATE,
        })),
    )
    .await;
    graphql_uri
}

/// The URL `api` published for its GraphQL endpoint, once it is an address of THIS emulator: the
/// scheme and the path the endpoint is served over, the host a resource listener of this
/// emulator is bound on, a port that listener answers on, and the API's own `dns` record for
/// that endpoint — never a host belonging to another server.
fn published_graphql_uri(case: &str, api: &Value) -> String {
    let published = api["uris"]["GRAPHQL"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a published GRAPHQL endpoint, got {api}"))
        .to_string();
    assert!(
        published.starts_with(&format!("{GRAPHQL_SCHEME}://")),
        "case {case:?}: expected a {GRAPHQL_SCHEME} URL for the endpoint, got {published:?}"
    );
    // The decomposition is this target's own, so one reading of a published URL serves every
    // suite that grades one.
    let (authority, path) = published_parts(case, &published);
    assert_eq!(
        path, GRAPHQL_PATH,
        "case {case:?}: expected the path this endpoint is served on, got {published:?}"
    );
    // The record is the bare HOST of that URI: the authority without the port of the socket
    // the endpoint is served on, which is all a DNS record can hold.
    assert_eq!(
        authority
            .rsplit_once(':')
            .map(|(host, _port)| host),
        api["dns"]["GRAPHQL"].as_str(),
        "case {case:?}: expected the endpoint's own dns record to be the bare host of its uri, \
         got {api}"
    );
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {case:?}: expected a host and a port in {published:?}"));
    let port: u16 = port
        .parse()
        .unwrap_or_else(|error| panic!("case {case:?}: expected a port in {published:?}: {error}"));
    assert_eq!(
        host, OWN_HOST,
        "case {case:?}: expected the host this emulator binds a resource listener on, got \
         {published:?}"
    );
    assert!(
        port > 0,
        "case {case:?}: expected the port the endpoint is served on, got {published:?}"
    );
    published
}
