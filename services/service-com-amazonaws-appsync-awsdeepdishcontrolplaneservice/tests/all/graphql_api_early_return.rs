// SPDX-License-Identifier: CC0-1.0

//! A resolver that ends its own step through `runtime.earlyReturn`, driven over the address
//! the API PUBLISHES for its GraphQL endpoint.
//!
//! The rule these cases assert, one level above the recorded case (one `NONE` data source and
//! one APPSYNC_JS unit resolver whose request handler calls
//! `runtime.earlyReturn(ctx.args.msg)`):
//!
//! > `runtime.earlyReturn(value)` returns prematurely from a request handler: the step that
//! > handler belongs to is OVER, with `value` as its result — the data source the step names
//! > is not called and the step's own response handler does not run. The step is the UNIT
//! > resolver, or one function of a pipeline (after which the pipeline carries on with the
//! > next step, handed that value as `ctx.prev.result`), or — for the handler of a `PIPELINE`
//! > resolver itself — the whole resolver, whose functions and response handler then do not
//! > run. A handler that returns the ordinary way is unaffected: its value is the request the
//! > data source is called with, and the response handler runs over that result.
//!
//! Every case is driven as a client drives it: a `POST` to the socket `uris.GRAPHQL` names,
//! carrying the API's own key, so what is asserted is what the published endpoint ANSWERS.
//!
//! The rows span what the rule generalises over: the JSON shape the value takes (a string
//! from an argument, a string from a variable, an object with a sub-selection, a list, and no
//! value at all), the step the early return happens in (a unit resolver, one function of a
//! pipeline, and a pipeline resolver's own handler), the data source the step names (the local
//! one and one of a type this endpoint never calls), and — as the control — a resolver that
//! does not return early at all.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{emulator, try_post_answer, GRAPHQL_PATH};

/// How long a case waits for the clock to perform a schema creation, and how often it looks
/// again — the span is the service's own (`state::SCHEMA_CREATION_SPAN`), and the deadline is
/// far past it so a loaded machine still reaches it.
const CREATION_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The `runtime` a resolver defined by `code` names. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// What every response handler of this suite answers: a value no case expects, so a case that
/// reads it back is one whose response handler RAN when it should not have.
const RESPONSE_MARKER: &str = "the response handler ran";

/// The local data source the steps of this suite resolve against: `NONE` is "There is no data
/// source", so an ordinary request handler's own `payload` is the result.
const LOCAL_DATA_SOURCE: &str = "local";

/// A data source of a type this endpoint never calls, so a step that reached it would be
/// reported as a field that did not resolve: naming it is how a case proves the early return
/// skipped the call. Source: `DataSourceType`'s own values
/// (`smithy_query(query="shape",shape="DataSourceType")`).
const REMOTE_DATA_SOURCE: &str = "remote";

/// The schema every API of this suite is created with: one field per row below.
const SCHEMA: &str = "schema { query: Query }\n\
                      \n\
                      type Marked {\n  id: ID\n  label: String\n}\n\
                      \n\
                      type Query {\n  scalar(msg: String!): String\n  shaped: Marked\n  \
                      listed: [Int]\n  nothing: String\n  remote: String\n  piped: String\n  \
                      short: String\n  plain(msg: String!): String\n}\n";

/// One created GraphQL API: its id, the address its GraphQL endpoint is served on, and a live
/// API key.
struct GraphqlApi {
    graphql: String,
    api_key: String,
}

#[tokio::test]
async fn a_request_handler_that_returns_early_ends_its_step_with_that_value() {
    let router = emulator();
    let api = create_api(&router, "early-return").await;

    /// One row: the document a client POSTs, and what the published endpoint must answer
    /// under `data` for it.
    struct Row {
        case: &'static str,
        query: Value,
        data: Value,
    }
    let rows: &[Row] = &[
        Row {
            case: "a unit resolver early-returning a string built from an argument",
            query: json!({"query": "query { scalar(msg: \"handed back\") }"}),
            data: json!({"scalar": "handed back"}),
        },
        Row {
            case: "the same resolver over another argument",
            query: json!({"query": "query { scalar(msg: \"and another\") }"}),
            data: json!({"scalar": "and another"}),
        },
        Row {
            case: "the same resolver over an argument carried in a variable",
            query: json!({
                "query": "query Handed($msg: String!) { scalar(msg: $msg) }",
                "variables": {"msg": "carried in a variable"},
            }),
            data: json!({"scalar": "carried in a variable"}),
        },
        Row {
            case: "an early return of an object, with a sub-selection",
            query: json!({"query": "query { shaped { id label } }"}),
            data: json!({"shaped": {"id": "m-1", "label": "shaped early"}}),
        },
        Row {
            case: "an early return of a list",
            query: json!({"query": "query { listed }"}),
            data: json!({"listed": [3, 1, 2]}),
        },
        Row {
            case: "an early return handed no value at all, through the runtime's bare name",
            query: json!({"query": "query { nothing }"}),
            data: json!({"nothing": Value::Null}),
        },
        Row {
            case: "an early return over a data source this endpoint never calls",
            query: json!({"query": "query { remote }"}),
            data: json!({"remote": "without calling the data source"}),
        },
        Row {
            case: "a pipeline function that returns early, the pipeline carrying on",
            query: json!({"query": "query { piped }"}),
            data: json!({"piped": "from the function"}),
        },
        Row {
            case: "a pipeline resolver's own handler returning early, skipping its function",
            query: json!({"query": "query { short }"}),
            data: json!({"short": "before the pipeline"}),
        },
        Row {
            case: "a resolver that does not return early, which runs both halves",
            query: json!({"query": "query { plain(msg: \"through\") }"}),
            data: json!({"plain": "plain:through"}),
        },
        Row {
            case: "an early-returning and an ordinary field in one document",
            query: json!({"query": "query { scalar(msg: \"both\") plain(msg: \"both\") }"}),
            data: json!({"scalar": "both", "plain": "plain:both"}),
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answered) = post_graphql(case, &api, &row.query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an executed document is answered 200, got {status} and {answered}"
        );
        assert_eq!(
            answered["data"], row.data,
            "case {case:?}: expected {} under data, got {answered}",
            row.data
        );
        assert!(
            answered.get("errors").is_none(),
            "case {case:?}: expected no error to be reported, got {answered}"
        );
        assert!(
            !answered.to_string().contains(RESPONSE_MARKER),
            "case {case:?}: an early return's own step must not run its response handler, \
             got {answered}"
        );
    }
}

/// POST one GraphQL document to the address the API published, carrying its own key.
async fn post_graphql(case: &str, api: &GraphqlApi, body: &Value) -> (u16, Value) {
    let (status, _fields, document) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        body,
        &[("x-api-key", api.api_key.as_str())],
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

/// Create one API with field-level logging configured — the configuration the recorded case
/// carries, so every step below is also one the API collects a trace of — install [`SCHEMA`],
/// the two data sources and one resolver per field, and read the address the API publishes.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("early-return-{name}"),
            "authenticationType": "API_KEY",
            "logConfig": {
                "fieldLogLevel": "ALL",
                "cloudWatchLogsRoleArn": "arn:aws:iam::000000000000:role/appsync-logs",
                "excludeVerboseContent": false,
            },
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    assert!(
        graphql.starts_with(&format!("{LOOPBACK_HOST}:")),
        "uris.GRAPHQL must be published on an address this emulator serves, got {graphql:?}"
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
    create_data_sources(router, &api_id).await;
    create_resolvers(router, &api_id).await;
    GraphqlApi {
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
    assert_eq!(
        status, 200,
        "StartSchemaCreation must answer 200, got {started}"
    );
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

/// Create the local data source the steps resolve against and the remote one a step names to
/// show the call was never made.
async fn create_data_sources(router: &MultiServiceRouter, api_id: &str) {
    for body in [
        json!({"name": LOCAL_DATA_SOURCE, "type": "NONE"}),
        json!({
            "name": REMOTE_DATA_SOURCE,
            "type": "HTTP",
            "httpConfig": {"endpoint": "https://orders.example.com"},
        }),
    ] {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateDataSource {body} must answer 200, got {created}"
        );
    }
}

/// Attach one resolver per field of [`SCHEMA`]: each handler of this suite answers
/// [`RESPONSE_MARKER`] from its response half, so a value that reaches a client is either the
/// early-returned one or proof the response half ran.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    let unit: &[(&str, &str, String)] = &[
        (
            "scalar",
            LOCAL_DATA_SOURCE,
            format!(
                "import {{ runtime }} from '@aws-appsync/utils';\n\
                 export function request(ctx) {{ runtime.earlyReturn(ctx.args.msg); }}\n\
                 export function response(ctx) {{ return '{RESPONSE_MARKER}'; }}"
            ),
        ),
        (
            "shaped",
            LOCAL_DATA_SOURCE,
            format!(
                "import {{ runtime }} from '@aws-appsync/utils';\n\
                 export function request(ctx) {{\n  \
                   runtime.earlyReturn({{id: 'm-1', label: 'shaped early'}});\n\
                 }}\n\
                 export function response(ctx) {{\n  \
                   return {{id: '{RESPONSE_MARKER}', label: '{RESPONSE_MARKER}'}};\n\
                 }}"
            ),
        ),
        (
            "listed",
            LOCAL_DATA_SOURCE,
            "import { runtime } from '@aws-appsync/utils';\n\
             export function request(ctx) { runtime.earlyReturn([3, 1, 2]); }\n\
             export function response(ctx) { return [9, 9]; }"
                .to_string(),
        ),
        (
            // No import of its own: the runtime binds its own names for the code it runs.
            "nothing",
            LOCAL_DATA_SOURCE,
            format!(
                "export function request(ctx) {{ runtime.earlyReturn(); }}\n\
                 export function response(ctx) {{ return '{RESPONSE_MARKER}'; }}"
            ),
        ),
        (
            "remote",
            REMOTE_DATA_SOURCE,
            format!(
                "import {{ runtime }} from '@aws-appsync/utils';\n\
                 export function request(ctx) {{\n  \
                   runtime.earlyReturn('without calling the data source');\n\
                 }}\n\
                 export function response(ctx) {{ return '{RESPONSE_MARKER}'; }}"
            ),
        ),
        (
            "plain",
            LOCAL_DATA_SOURCE,
            "export function request(ctx) { return {payload: ctx.args.msg}; }\n\
             export function response(ctx) { return 'plain:' + ctx.result; }"
                .to_string(),
        ),
    ];
    for (field, data_source, code) in unit {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": data_source,
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

    // A pipeline whose FUNCTION returns early: the pipeline carries on, so the resolver's own
    // response handler answers what that function produced as `ctx.prev.result`.
    let early_function = create_function(
        router,
        api_id,
        "earlyFunction",
        &format!(
            "import {{ runtime }} from '@aws-appsync/utils';\n\
             export function request(ctx) {{ runtime.earlyReturn('from the function'); }}\n\
             export function response(ctx) {{ return '{RESPONSE_MARKER}'; }}"
        ),
    )
    .await;
    create_pipeline(
        router,
        api_id,
        "piped",
        &[early_function],
        "export function request(ctx) { return {}; }\n\
         export function response(ctx) { return ctx.prev.result; }",
    )
    .await;

    // And a pipeline whose OWN handler returns early: its function never runs, so the value a
    // client reads is neither the function's nor the resolver's response handler's.
    let ordinary_function = create_function(
        router,
        api_id,
        "ordinaryFunction",
        "export function request(ctx) { return {payload: 'the function ran'}; }\n\
         export function response(ctx) { return ctx.result; }",
    )
    .await;
    create_pipeline(
        router,
        api_id,
        "short",
        &[ordinary_function],
        &format!(
            "import {{ runtime }} from '@aws-appsync/utils';\n\
             export function request(ctx) {{ runtime.earlyReturn('before the pipeline'); }}\n\
             export function response(ctx) {{ return '{RESPONSE_MARKER}'; }}"
        ),
    )
    .await;
}

/// Create one pipeline function defined by `code`, answering the id the pipeline names it by.
async fn create_function(
    router: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    code: &str,
) -> String {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": name,
            "dataSourceName": LOCAL_DATA_SOURCE,
            "code": code,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateFunction {name} must answer 200, got {created}"
    );
    created["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Attach a `PIPELINE` resolver over `functions` to `field`, defined by `code`.
async fn create_pipeline(
    router: &MultiServiceRouter,
    api_id: &str,
    field: &str,
    functions: &[String],
    code: &str,
) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": field,
            "kind": "PIPELINE",
            "pipelineConfig": {"functions": functions},
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
