// SPDX-License-Identifier: CC0-1.0

//! A GraphQL API's `GRAPHQL` endpoint, driven over a real socket: the address the API
//! publishes for it, the GraphQL documents it executes against the API's own resolvers, and
//! the web ACL WAF puts in front of it.
//!
//! The rule these cases assert, one level above the recorded case (one API created with
//! `API_KEY`, one `NONE` data source, one APPSYNC_JS resolver returning a marker, and one
//! REGIONAL web ACL blocking a header):
//!
//! > A GraphQL API publishes, as its `GRAPHQL` endpoint, the address of the socket that
//! > endpoint is SERVED on — `dns.GRAPHQL` the `host:port` and `uris.GRAPHQL` the `https://`
//! > URL over it — so a client that POSTs to what the API published reaches this emulator.
//! > There `POST /graphql` carrying a GraphQL document is EXECUTED against the API's own
//! > resolvers: each selected field is answered by what the resolver attached to it returns,
//! > under the field's response key, with the field's arguments and the request's variables
//! > as the code reads them, and a field whose code raises is reported as an error of that
//! > field's path with a `null` value. A web ACL associated with the API's ARN decides every
//! > such request at that edge first: a request a rule BLOCKS is answered `403` with exactly
//! > one error, `WAFForbiddenException` / "403 Forbidden", and no `data`, while the same
//! > request the web ACL allows reaches the resolver and is answered `200` — and the
//! > association itself is unchanged by either.
//!
//! The rows span what the rule generalises over: the shape a resolver returns (a scalar, an
//! object with a sub-selection, a list, a value built from an argument, from a variable and
//! from an alias), the resolver kind (`UNIT` and a `PIPELINE` over a function), a resolver
//! that raises, the material a request presents (a live key, none, an unknown key), what it
//! addresses (the endpoint's path and method, and others), and what the web ACL in front of
//! the API decides (a rule that matches a header exactly, the same request without that
//! header, a rule that allows, a web ACL whose default action blocks, and no web ACL at
//! all).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::{appsync, loopback_port};

/// The region every control-plane call in this suite is signed for.
const REGION: &str = "us-west-2";

/// How long a case waits for one answer before giving up: generous for a loaded test
/// machine, and bounded so a case that will never be answered fails instead of hanging.
const ANSWER_DEADLINE: Duration = Duration::from_secs(30);

/// How long a case waits for the clock to perform a schema creation, and how often it looks
/// again — the span is the service's own (`state::SCHEMA_CREATION_SPAN`), and the deadline is
/// far past it so a loaded machine still reaches it.
const CREATION_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The envelope AppSync's edge answers a WAF-blocked request with; the service's own copies
/// are `service::graphql_api::query::WAF_FORBIDDEN_ERROR_TYPE` and `…_MESSAGE`.
const WAF_FORBIDDEN_ERROR_TYPE: &str = "WAFForbiddenException";
const WAF_FORBIDDEN_MESSAGE: &str = "403 Forbidden";

/// The header the blocking rules of this suite inspect, and the value they match.
const BLOCK_HEADER: &str = "x-fidelity-block";
const BLOCK_VALUE: &str = "yes";

/// The `runtime` a resolver defined by `code` names. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The schema every API of this suite is created with: the fields the rows below attach
/// resolvers to.
const SCHEMA: &str = "type Post {\n  id: ID!\n  title: String\n}\n\
                      \n\
                      type Query {\n  ping: String\n  echo(text: String): String\n  \
                      post(id: ID!): Post\n  posts: [Post]\n  refused: String\n  \
                      piped: String\n  unresolved: String\n}\n\
                      \n\
                      type Mutation {\n  addPost(title: String!): Post\n}\n\
                      \n\
                      type Subscription {\n  onAddPost: Post\n}\n";

/// One created GraphQL API: its id, its ARN, the address its GraphQL endpoint is served on
/// and a live API key.
struct GraphqlApi {
    api_id: String,
    arn: String,
    graphql: String,
    api_key: String,
}

/// A router with AppSync, CloudWatch and WAFv2 mounted over ONE engine: the web ACL in front
/// of an API belongs to WAF, so the association a case makes is the one the API's endpoint
/// enforces.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(engine),
    );
    router
}

/// One WAFv2 call, over the wire that service speaks (`awsJson1_1`, `POST /` with
/// `X-Amz-Target`).
async fn wafv2(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://wafv2.example.com/")
        .header("host", "wafv2.example.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSWAF_20190729.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/wafv2/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same, asserting the call succeeded.
async fn waf_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, answered) = wafv2(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} and {answered}"
    );
    answered
}

#[tokio::test]
async fn the_published_graphql_endpoint_is_the_socket_this_emulator_serves_it_on() {
    let router = emulator();
    let api = create_api(&router, "published").await;
    let case = "the published graphql endpoint";

    // The published address is one this emulator answers on, and the URL is built over it —
    // on the create and on every later read of the same API.
    let (status, fetched) = appsync(&router, "GET", &format!("/v1/apis/{}", api.api_id), None).await;
    assert_eq!(status, 200, "GetGraphqlApi must answer 200, got {fetched}");
    let published = fetched["graphqlApi"]["dns"]["GRAPHQL"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        published, api.graphql,
        "case {case:?}: every read must report the address the create published, got \
         {published:?} and {:?}",
        api.graphql
    );
    assert_eq!(
        fetched["graphqlApi"]["uris"]["GRAPHQL"].as_str(),
        Some(format!("https://{published}{GRAPHQL_PATH}").as_str()),
        "case {case:?}: uris.GRAPHQL must be the HTTPS URL over the published address, got \
         {fetched}"
    );

    // And a client that POSTs to it is served the result of the API's own resolvers.
    let (status, answered) = post_graphql(
        case,
        &api,
        &json!({"query": "query { ping }"}),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        (status, answered["data"]["ping"].as_str()),
        (200, Some(PING_MARKER)),
        "case {case:?}: the published endpoint must execute the query against the API's \
         resolver, got {status} and {answered}"
    );
}

/// The marker the `ping` resolver returns, which is what proves the ANSWER came from the
/// stored resolver's own code rather than from anything the request carried.
const PING_MARKER: &str = "resolver-marker-ping";

#[tokio::test]
async fn each_selected_field_is_answered_by_the_resolver_attached_to_it() {
    let router = emulator();
    let api = create_api(&router, "execution").await;

    /// One row: the document a client POSTs, and what the API must answer for it.
    struct Row {
        case: &'static str,
        query: Value,
        data: Value,
        /// The paths the answer must report an error for, in order.
        errors: &'static [&'static str],
    }
    let rows: &[Row] = &[
        Row {
            case: "a scalar field",
            query: json!({"query": "query { ping }"}),
            data: json!({"ping": PING_MARKER}),
            errors: &[],
        },
        Row {
            case: "a field whose code reads a literal argument",
            query: json!({"query": "query { echo(text: \"spoken\") }"}),
            data: json!({"echo": "echo:spoken"}),
            errors: &[],
        },
        Row {
            case: "a field whose code reads a variable",
            query: json!({
                "query": "query Spoken($text: String) { echo(text: $text) }",
                "variables": {"text": "varied"},
            }),
            data: json!({"echo": "echo:varied"}),
            errors: &[],
        },
        Row {
            case: "an alias over the same field, selected twice",
            query: json!({
                "query": "query { first: echo(text: \"one\") second: echo(text: \"two\") }",
            }),
            data: json!({"first": "echo:one", "second": "echo:two"}),
            errors: &[],
        },
        Row {
            case: "an object field with a sub-selection",
            query: json!({"query": "query { post(id: \"p-7\") { id title } }"}),
            data: json!({"post": {"id": "p-7", "title": "post p-7"}}),
            errors: &[],
        },
        Row {
            case: "a sub-selection of an object field, through a fragment",
            query: json!({
                "query": "query { post(id: \"p-8\") { ...Shown } } fragment Shown on Post { id }",
            }),
            data: json!({"post": {"id": "p-8"}}),
            errors: &[],
        },
        Row {
            case: "a list field with a sub-selection",
            query: json!({"query": "query { posts { title } }"}),
            data: json!({"posts": [{"title": "first"}, {"title": "second"}]}),
            errors: &[],
        },
        Row {
            case: "a pipeline resolver over one function",
            query: json!({"query": "query { piped }"}),
            data: json!({"piped": "piped:stashed"}),
            errors: &[],
        },
        Row {
            case: "a resolver that raises",
            query: json!({"query": "query { refused }"}),
            data: json!({"refused": Value::Null}),
            errors: &["refused"],
        },
        Row {
            case: "a field that carries no resolver",
            query: json!({"query": "query { unresolved }"}),
            data: json!({"unresolved": Value::Null}),
            errors: &[],
        },
        Row {
            case: "two fields of one document, one of which raises",
            query: json!({"query": "query { ping refused }"}),
            data: json!({"ping": PING_MARKER, "refused": Value::Null}),
            errors: &["refused"],
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answered) = post_graphql(
            case,
            &api,
            &row.query,
            &[("x-api-key", api.api_key.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an executed document is answered 200, got {status} and {answered}"
        );
        assert_eq!(
            answered["data"], row.data,
            "case {case:?}: expected the data {}, got {answered}",
            row.data
        );
        let reported: Vec<String> = answered["errors"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|error| error["path"][0].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported,
            row.errors
                .iter()
                .map(|path| (*path).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected errors for exactly {:?}, got {answered}",
            row.errors
        );
        for error in answered["errors"].as_array().cloned().unwrap_or_default() {
            assert!(
                error["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: every error must state what went wrong, got {error}"
            );
        }
    }
}

#[tokio::test]
async fn the_web_acl_in_front_of_the_api_decides_every_request_at_that_edge() {
    let router = emulator();
    let api = create_api(&router, "webacl").await;

    // A REGIONAL web ACL that blocks exactly the requests carrying the header, and allows
    // everything else; a second that blocks everything through its DEFAULT action; and a
    // third whose matching rule ALLOWS, so the rule's own action is what decides.
    let blocking = create_web_acl(
        &router,
        "blocks-the-header",
        json!({"Allow": {}}),
        json!([header_rule("block-marked", json!({"Block": {}}))]),
    )
    .await;
    let blocking_by_default = create_web_acl(
        &router,
        "blocks-by-default",
        json!({"Block": {}}),
        json!([]),
    )
    .await;
    let allowing = create_web_acl(
        &router,
        "allows-the-header",
        json!({"Allow": {}}),
        json!([header_rule("allow-marked", json!({"Allow": {}}))]),
    )
    .await;

    /// One row: which web ACL is in front of the API (`None` for no association at all),
    /// whether the request carries the blocked header value, and the status it must meet.
    struct Row {
        case: &'static str,
        web_acl: Option<usize>,
        header: Option<&'static str>,
        status: u16,
    }
    let acls = [&blocking, &blocking_by_default, &allowing];
    let rows: &[Row] = &[
        Row {
            case: "no web acl in front of the api, with the header",
            web_acl: None,
            header: Some(BLOCK_VALUE),
            status: 200,
        },
        Row {
            case: "a blocking rule, and the header it matches",
            web_acl: Some(0),
            header: Some(BLOCK_VALUE),
            status: 403,
        },
        Row {
            case: "a blocking rule, and no such header",
            web_acl: Some(0),
            header: None,
            status: 200,
        },
        Row {
            case: "a blocking rule, and another value of the header it matches exactly",
            web_acl: Some(0),
            header: Some("no"),
            status: 200,
        },
        Row {
            case: "a web acl whose default action blocks",
            web_acl: Some(1),
            header: None,
            status: 403,
        },
        Row {
            case: "a matching rule whose action allows",
            web_acl: Some(2),
            header: Some(BLOCK_VALUE),
            status: 200,
        },
    ];

    for row in rows {
        let case = row.case;
        match row.web_acl {
            Some(index) => {
                let (arn, _) = acls[index];
                waf_ok(
                    &router,
                    "AssociateWebACL",
                    json!({"ResourceArn": api.arn, "WebACLArn": arn}),
                )
                .await;
            }
            None => {
                let (status, _) = wafv2(
                    &router,
                    "DisassociateWebACL",
                    json!({"ResourceArn": api.arn}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DisassociateWebACL must answer 200, got {status}"
                );
            }
        }
        let mut fields: Vec<(&str, &str)> = vec![("x-api-key", api.api_key.as_str())];
        if let Some(value) = row.header {
            fields.push((BLOCK_HEADER, value));
        }
        let (status, answered) =
            post_graphql(case, &api, &json!({"query": "query { ping }"}), &fields).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected the status {}, got {status} and {answered}",
            row.status
        );
        if row.status == 403 {
            assert_eq!(
                answered,
                json!({"errors": [
                    {"errorType": WAF_FORBIDDEN_ERROR_TYPE, "message": WAF_FORBIDDEN_MESSAGE},
                ]}),
                "case {case:?}: a blocked request carries exactly one WAF-forbidden error \
                 and no data, got {answered}"
            );
        } else {
            assert_eq!(
                answered["data"]["ping"].as_str(),
                Some(PING_MARKER),
                "case {case:?}: an allowed request reaches the resolver, got {answered}"
            );
        }
        // Whatever the web ACL decided, the association itself is unchanged: WAF's own read
        // still reports the web ACL in front of the API.
        let reported = waf_ok(
            &router,
            "GetWebACLForResource",
            json!({"ResourceArn": api.arn}),
        )
        .await;
        let expected = row.web_acl.map(|index| acls[index].1.clone());
        assert_eq!(
            reported["WebACL"]["Name"].as_str().map(str::to_string),
            expected,
            "case {case:?}: GetWebACLForResource must still report {expected:?}, got {reported}"
        );
    }
}

#[tokio::test]
async fn the_endpoint_refuses_what_it_does_not_serve_or_authorize() {
    let router = emulator();
    let api = create_api(&router, "refusals").await;

    /// One row: what the request addresses and presents, and the status and `errorType` the
    /// refusal must carry.
    struct Row {
        case: &'static str,
        path: &'static str,
        method: &'static str,
        body: Value,
        api_key: Option<&'static str>,
        status: u16,
        error_type: &'static str,
    }
    let rows: &[Row] = &[
        Row {
            case: "a path this endpoint does not serve",
            path: "/event",
            method: "POST",
            body: json!({"query": "query { ping }"}),
            api_key: None,
            status: 404,
            error_type: "NotFoundException",
        },
        Row {
            case: "a method a GraphQL request is not made with",
            path: GRAPHQL_PATH,
            method: "GET",
            body: json!({"query": "query { ping }"}),
            api_key: None,
            status: 405,
            error_type: "BadRequestException",
        },
        Row {
            case: "no material presented at all",
            path: GRAPHQL_PATH,
            method: "POST",
            body: json!({"query": "query { ping }"}),
            api_key: None,
            status: 401,
            error_type: "UnauthorizedException",
        },
        Row {
            case: "a key this API never minted",
            path: GRAPHQL_PATH,
            method: "POST",
            body: json!({"query": "query { ping }"}),
            api_key: Some("da2-neverminted000000000000"),
            status: 401,
            error_type: "UnauthorizedException",
        },
        Row {
            case: "a body carrying no document",
            path: GRAPHQL_PATH,
            method: "POST",
            body: json!({"variables": {}}),
            api_key: Some(LIVE_KEY),
            status: 400,
            error_type: "BadRequestException",
        },
    ];

    for row in rows {
        let case = row.case;
        let key = match row.api_key {
            Some(LIVE_KEY) => api.api_key.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        let mut fields: Vec<(&str, &str)> = Vec::new();
        if row.api_key.is_some() {
            fields.push(("x-api-key", key.as_str()));
        }
        let (status, answered) =
            post_raw(case, &api, row.path, row.method, &row.body, &fields).await;
        assert_eq!(
            (status, answered.pointer("/errors/0/errorType").and_then(Value::as_str)),
            (row.status, Some(row.error_type)),
            "case {case:?}: expected {} {}, got {status} and {answered}",
            row.status,
            row.error_type
        );
        assert!(
            answered.get("data").is_none(),
            "case {case:?}: a refused request reaches no resolver, so it carries no data, \
             got {answered}"
        );
    }

    // A document this endpoint cannot execute is a GraphQL result with errors and no data,
    // which is what a GraphQL client reads such a fault from. The last three rows are the
    // bounds a document cannot talk this endpoint past: a nesting deeper than it resolves, a
    // fragment spread into itself, and an expansion wider than one request may select —
    // each refused before any resolver runs.
    for (case, query) in [
        ("a document that does not parse", "query { ping".to_string()),
        (
            "an operation the document does not declare",
            "query Named { ping }".to_string(),
        ),
        (
            "a subscription over the HTTP endpoint",
            "subscription { onAddPost { id } }".to_string(),
        ),
        ("a selection nested past the depth bound", deeply_nested()),
        (
            "a fragment spread into itself",
            "query { post(id: \"p\") { ...Loop } } fragment Loop on Post { id ...Loop }"
                .to_string(),
        ),
        ("an expansion wider than one request selects", widely_spread()),
    ] {
        let mut body = json!({"query": query});
        if case.starts_with("an operation") {
            body["operationName"] = json!("Other");
        }
        let (status, answered) =
            post_graphql(case, &api, &body, &[("x-api-key", api.api_key.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a GraphQL fault is answered 200 with errors, got {status} and \
             {answered}"
        );
        assert!(
            answered.get("data").is_none()
                && answered["errors"]
                    .as_array()
                    .is_some_and(|errors| errors.len() == 1),
            "case {case:?}: expected exactly one error and no data, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_deleted_api_stops_answering_where_it_published_its_graphql_endpoint() {
    let router = emulator();
    let api = create_api(&router, "deleted").await;
    let case = "a deleted API's graphql endpoint";

    let (status, deleted) =
        appsync(&router, "DELETE", &format!("/v1/apis/{}", api.api_id), None).await;
    assert_eq!(status, 200, "DeleteGraphqlApi must answer 200, got {deleted}");

    // Nothing serves THIS API's queries at the address it published any more: the port is
    // refused, the connection is severed, or — if the operating system has since handed the
    // port to another resource of this machine — whatever answers is not this API's
    // resolver.
    let answered = try_post_raw(
        case,
        &api,
        GRAPHQL_PATH,
        "POST",
        &json!({"query": "query { ping }"}),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert!(
        answered.as_ref().is_none_or(|(status, body)| *status != 200
            || body["data"]["ping"].as_str() != Some(PING_MARKER)),
        "case {case:?}: a deleted API resolves nothing at {:?}, got {answered:?}",
        api.graphql
    );
}

/// The stand-in a row uses for "the API's own live key", which is minted per run.
const LIVE_KEY: &str = "<the api's own key>";

/// A document whose selection nests deeper than this endpoint resolves: the field is one the
/// schema declares, so what the endpoint refuses is the DEPTH and nothing else.
///
/// The bound is the service's own (`service::graphql_api::execution::MAX_SELECTION_DEPTH`),
/// and this document sits a long way past it rather than one level over, so the case does
/// not have to track its value.
fn deeply_nested() -> String {
    let levels = 32;
    let mut query = "query { post(id: \"deep\")".to_string();
    for _ in 0..levels {
        query.push_str(" { post");
    }
    query.push_str(" { id }");
    for _ in 0..levels {
        query.push_str(" }");
    }
    query.push('}');
    query
}

/// A document whose FRAGMENTS expand to far more fields than one request may select, while
/// no single selection set is wide enough to be refused on its own: 64 fields each spreading
/// a second fragment of 64 fields is 64 × 64 fields in all.
///
/// This is the shape the per-level bounds do not catch — the expansion is the product of the
/// levels — so it is what the whole-document budget
/// (`service::graphql_api::execution::MAX_SELECTED_NODES`) is there for.
fn widely_spread() -> String {
    let width = 64;
    let outer: String = (0..width)
        .map(|index| format!("post(id: \"a{index}\") {{ ...Inner }} "))
        .collect();
    let inner: String = (0..width)
        .map(|index| format!("post(id: \"b{index}\") {{ id }} "))
        .collect();
    format!("query {{ ...Outer }} fragment Outer on Query {{ {outer} }} fragment Inner on Post {{ {inner} }}")
}

/// Create one GraphQL API with an API key, [`SCHEMA`], a `NONE` data source and one resolver
/// per field the rows select, and read the address its GraphQL endpoint is served on.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": format!("query-{name}"), "authenticationType": "API_KEY"})),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let arn = api["arn"].as_str().unwrap_or_default().to_string();
    let graphql = api["dns"]["GRAPHQL"].as_str().unwrap_or_default().to_string();
    assert!(
        graphql.starts_with(&format!("{LOOPBACK_HOST}:")),
        "dns.GRAPHQL must be an address this emulator serves, got {graphql:?}"
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    create_schema(router, &api_id).await;
    create_data_source(router, &api_id).await;
    create_resolvers(router, &api_id).await;
    GraphqlApi {
        api_id,
        arn,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Install [`SCHEMA`] on `api_id`, waiting for the clock to perform the creation.
async fn create_schema(router: &MultiServiceRouter, api_id: &str) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(SCHEMA);
    let (status, started) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": definition})),
    )
    .await;
    assert_eq!(status, 200, "StartSchemaCreation must answer 200, got {started}");
    let deadline = tokio::time::Instant::now() + CREATION_DEADLINE;
    loop {
        let (status, settled) = appsync(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "GetSchemaCreationStatus must answer 200, got {settled}"
        );
        if settled["status"].as_str() == Some("SUCCESS") {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the schema creation must be performed, got {settled}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// The name of the local data source every resolver of this suite resolves against: `NONE`
/// is "There is no data source", so the request function's own payload is the result.
const DATA_SOURCE: &str = "local";

/// Create that data source under `api_id`.
async fn create_data_source(router: &MultiServiceRouter, api_id: &str) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {created}");
}

/// Attach one resolver per field the rows select: each is APPSYNC_JS `code` whose `request`
/// function returns the payload the local data source passes through and whose `response`
/// function returns the field's value.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    let unit: &[(&str, String)] = &[
        (
            "ping",
            format!(
                "export function request(ctx) {{ return {{payload: \"{PING_MARKER}\"}}; }}\n\
                 export function response(ctx) {{ return ctx.result; }}"
            ),
        ),
        (
            "echo",
            "export function request(ctx) { return {payload: \"echo:\" + ctx.arguments.text}; }\n\
             export function response(ctx) { return ctx.result; }"
                .to_string(),
        ),
        (
            "post",
            "export function request(ctx) {\n\
               return {payload: {id: ctx.args.id, title: \"post \" + ctx.args.id, \
                                 secret: \"unselected\"}};\n\
             }\n\
             export function response(ctx) { return ctx.result; }"
                .to_string(),
        ),
        (
            "posts",
            "export function request(ctx) {\n\
               return {payload: [{id: \"a\", title: \"first\"}, {id: \"b\", title: \"second\"}]};\n\
             }\n\
             export function response(ctx) { return ctx.result; }"
                .to_string(),
        ),
        (
            "refused",
            "import { util } from '@aws-appsync/utils';\n\
             export function request(ctx) { util.error(\"this field refuses to resolve\"); }\n\
             export function response(ctx) { return ctx.result; }"
                .to_string(),
        ),
    ];
    for (field, code) in unit {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": code,
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {field} must answer 200, got {created}"
        );
    }

    // One PIPELINE resolver, over a function of its own: the resolver's before step stashes
    // a value, the function reads the stash, and the resolver's after step answers what the
    // function returned.
    let (status, function) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": "stashReader",
            "dataSourceName": DATA_SOURCE,
            "code": "export function request(ctx) { return {payload: \"piped:\" + ctx.stash.seen}; }\n\
                     export function response(ctx) { return ctx.result; }",
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateFunction must answer 200, got {function}");
    let function_id = function["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": "piped",
            "kind": "PIPELINE",
            "pipelineConfig": {"functions": [function_id]},
            "code": "export function request(ctx) { ctx.stash.seen = \"stashed\"; return {}; }\n\
                     export function response(ctx) { return ctx.prev.result; }",
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for the pipeline must answer 200, got {created}"
    );
}

/// Create a REGIONAL web ACL with `default_action` and `rules`, and answer its ARN and name.
async fn create_web_acl(
    router: &MultiServiceRouter,
    name: &str,
    default_action: Value,
    rules: Value,
) -> (String, String) {
    let created = waf_ok(
        router,
        "CreateWebACL",
        json!({
            "Name": name,
            "Scope": "REGIONAL",
            "DefaultAction": default_action,
            "Rules": rules,
            "VisibilityConfig": {
                "SampledRequestsEnabled": true,
                "CloudWatchMetricsEnabled": true,
                "MetricName": name,
            },
        }),
    )
    .await;
    (
        created["Summary"]["ARN"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        name.to_string(),
    )
}

/// A rule that matches the request field [`BLOCK_HEADER`] holding exactly [`BLOCK_VALUE`],
/// with `action` as the action it applies.
///
/// Source for the members: `ByteMatchStatement` (`SearchString`, `FieldToMatch`,
/// `TextTransformations`, `PositionalConstraint`) and `SingleHeader$Name`
/// (`smithy_query(query="shape",shape="ByteMatchStatement,SingleHeader,\
/// PositionalConstraint")`). A blob member travels base64-encoded in an awsJson body.
fn header_rule(name: &str, action: Value) -> Value {
    use base64::Engine as _;
    json!({
        "Name": name,
        "Priority": 0,
        "Statement": {
            "ByteMatchStatement": {
                "SearchString": base64::engine::general_purpose::STANDARD.encode(BLOCK_VALUE),
                "FieldToMatch": {"SingleHeader": {"Name": BLOCK_HEADER}},
                "TextTransformations": [{"Priority": 0, "Type": "NONE"}],
                "PositionalConstraint": "EXACTLY",
            }
        },
        "Action": action,
        "VisibilityConfig": {
            "SampledRequestsEnabled": true,
            "CloudWatchMetricsEnabled": true,
            "MetricName": name,
        },
    })
}

/// POST one GraphQL request to the API's published endpoint, as a client does.
async fn post_graphql(
    case: &str,
    api: &GraphqlApi,
    body: &Value,
    fields: &[(&str, &str)],
) -> (u16, Value) {
    post_raw(case, api, GRAPHQL_PATH, "POST", body, fields).await
}

/// The same, carrying whatever path, method and request FIELDS the case presents.
async fn post_raw(
    case: &str,
    api: &GraphqlApi,
    path: &str,
    method: &str,
    body: &Value,
    fields: &[(&str, &str)],
) -> (u16, Value) {
    try_post_raw(case, api, path, method, body, fields)
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
                api.graphql
            )
        })
}

/// The same, answering `None` for a request nothing served at all — a connection refused or
/// severed, or an answer that is no HTTP response. The seam for a case that asserts an
/// address no longer serves this API.
async fn try_post_raw(
    case: &str,
    api: &GraphqlApi,
    path: &str,
    method: &str,
    body: &Value,
    fields: &[(&str, &str)],
) -> Option<(u16, Value)> {
    let mut stream = tokio::time::timeout(
        ANSWER_DEADLINE,
        TcpStream::connect(("127.0.0.1", loopback_port(case, &api.graphql))),
    )
    .await
    .ok()?
    .ok()?;
    let payload = body.to_string();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n",
        api.graphql,
        payload.len()
    );
    for (field, value) in fields {
        request.push_str(&format!("{field}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut answered = Vec::new();
    tokio::time::timeout(ANSWER_DEADLINE, stream.read_to_end(&mut answered))
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&answered).to_string();
    let (head, payload) = text.split_once("\r\n\r\n")?;
    let status: u16 = head.split_whitespace().nth(1)?.parse().ok()?;
    let document = serde_json::from_str(payload).unwrap_or(Value::Null);
    Some((status, document))
}
