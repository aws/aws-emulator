// SPDX-License-Identifier: CC0-1.0

//! A resolver defined by MAPPING TEMPLATES, driven over the API's own published endpoint.
//!
//! The rule these cases assert, one level above the recorded case (one `NONE` data source and
//! one unit resolver whose request template builds a payload and whose response template
//! answers a member of it):
//!
//! > A resolver is defined EITHER by `code` with a `runtime` or by its two mapping templates,
//! > and both are definitions AppSync RUNS: a request that selects a field whose resolver
//! > carries `requestMappingTemplate` / `responseMappingTemplate` is answered by rendering
//! > them — the request template against the field's own `$context` to the configuration the
//! > data source is called with, and the response template against the result that produced
//! > to the value the field takes — so the field is answered `200` with that value under
//! > `data` and no error. A template that calls `$util.error` is a field that did NOT resolve
//! > and is reported as an error of its path with a `null` value; one that calls
//! > `$util.appendError` keeps the value it rendered and reports the error beside it. A
//! > pipeline resolver runs the same way round over its functions, each of which may itself
//! > be written either way, with the stash carried from step to step.
//!
//! The rows span what the rule generalises over: the shape the template renders (a member of
//! the payload, a value built with a directive, a value built by a loop, a structure), the
//! template language features a template is written with (references, methods, `#if`,
//! `#foreach`, `#set`, the stash), the DEFINITION FORM (templates, `code`, and a pipeline
//! mixing both), and the two ways a template reports a failure.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::{appsync, loopback_port};

/// The Region every control-plane call in this suite is signed for.
pub(crate) const REGION: &str = "us-west-2";

/// How long a case waits for one answer on the API's own socket before giving up: generous
/// for a loaded test machine, and bounded so a case that will never be answered fails
/// instead of hanging.
const ANSWER_DEADLINE: Duration = Duration::from_secs(30);

/// How long a case waits for the clock to perform a schema creation, and how often it looks
/// again — the span is the service's own (`state::SCHEMA_CREATION_SPAN`), and the deadline is
/// far past it so a loaded machine still reaches it.
const CREATION_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
pub(crate) const GRAPHQL_PATH: &str = "/graphql";

/// The `runtime` a resolver defined by `code` names. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The `functionVersion` a function defined in templates carries: "when using VTL and mapping
/// templates, the `functionVersion` is required" and "the only supported value is 2018-05-29"
/// (`CreateFunction$functionVersion`).
const FUNCTION_VERSION: &str = "2018-05-29";

/// The schema every API of this suite is created with: the fields the rows attach resolvers
/// to, under an explicit `schema` definition binding the query root.
pub(crate) const SCHEMA: &str = "schema { query: Query }\n\
                                 \n\
                                 type Query {\n  probe(msg: String!): String\n  \
                                 upper(text: String): String\n  total(values: [Int]): Int\n  \
                                 piped: String\n  refused: String\n  noted: String\n  \
                                 coded: String\n  bare: String\n  guarded: String\n}\n";

/// The name of the local data source every resolver of this suite resolves against: `NONE` is
/// "There is no data source", so the request half's own payload is the result.
pub(crate) const DATA_SOURCE: &str = "local";

/// One created GraphQL API: its id, the address its GraphQL endpoint is served on, and a live
/// API key.
pub(crate) struct GraphqlApi {
    pub(crate) api_id: String,
    pub(crate) graphql: String,
    pub(crate) api_key: String,
}

/// A router with AppSync, CloudWatch and CloudWatch Logs mounted over ONE engine: what an API
/// logs about a request goes into the log store CloudWatch Logs' own readers answer from
/// ([`crate::graphql_api_field_logging`]), and the API's metrics into CloudWatch's.
pub(crate) fn emulator() -> MultiServiceRouter {
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
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

#[tokio::test]
async fn a_field_whose_resolver_is_written_in_templates_is_answered_by_rendering_them() {
    let router = emulator();
    let api = create_api(&router, "templates", None).await;

    /// One row: the document a client POSTs, what the API must answer under `data`, and the
    /// paths it must report an error for.
    struct Row<'a> {
        case: &'a str,
        query: Value,
        data: Value,
        errors: &'a [&'a str],
    }
    let rows: &[Row] = &[
        Row {
            case: "a template that answers a member of the payload it built",
            query: json!({"query": "query { probe(msg: \"hello\") }"}),
            data: json!({"probe": "hello"}),
            errors: &[],
        },
        Row {
            case: "the same resolver over another argument",
            query: json!({"query": "query { probe(msg: \"other\") }"}),
            data: json!({"probe": "other"}),
            errors: &[],
        },
        Row {
            case: "an argument carried in a variable",
            query: json!({
                "query": "query Probed($msg: String!) { probe(msg: $msg) }",
                "variables": {"msg": "varied"},
            }),
            data: json!({"probe": "varied"}),
            errors: &[],
        },
        Row {
            case: "an alias over the same field, selected twice",
            query: json!({
                "query": "query { first: probe(msg: \"one\") second: probe(msg: \"two\") }",
            }),
            data: json!({"first": "one", "second": "two"}),
            errors: &[],
        },
        Row {
            case: "a response template written with #if and a method",
            query: json!({"query": "query { upper(text: \"quiet\") }"}),
            data: json!({"upper": "QUIET"}),
            errors: &[],
        },
        Row {
            case: "the other branch of that #if",
            query: json!({"query": "query { upper }"}),
            data: json!({"upper": Value::Null}),
            errors: &[],
        },
        Row {
            case: "a response template written with #set and #foreach",
            query: json!({"query": "query { total(values: [1, 2, 3, 4]) }"}),
            data: json!({"total": 10}),
            errors: &[],
        },
        Row {
            case: "the same loop over no elements",
            query: json!({"query": "query { total(values: []) }"}),
            data: json!({"total": 0}),
            errors: &[],
        },
        Row {
            case: "a pipeline resolver over a function, through the stash",
            query: json!({"query": "query { piped }"}),
            data: json!({"piped": "piped:stashed"}),
            errors: &[],
        },
        Row {
            case: "a template that calls util.error",
            query: json!({"query": "query { refused }"}),
            data: json!({"refused": Value::Null}),
            errors: &["refused"],
        },
        Row {
            case: "a template that calls util.appendError, which keeps its value",
            query: json!({"query": "query { noted }"}),
            data: json!({"noted": "noted-value"}),
            errors: &["noted"],
        },
        Row {
            case: "a resolver defined by code rather than by templates",
            query: json!({"query": "query { coded }"}),
            data: json!({"coded": "coded-value"}),
            errors: &[],
        },
        Row {
            case: "both definition forms in one document",
            query: json!({"query": "query { probe(msg: \"both\") coded }"}),
            data: json!({"probe": "both", "coded": "coded-value"}),
            errors: &[],
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
            "case {case:?}: a GraphQL request is answered 200, got {status} and {answered}"
        );
        assert_eq!(
            answered["data"], row.data,
            "case {case:?}: expected {} under data, got {answered}",
            row.data
        );
        let reported: Vec<String> = answered["errors"]
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .map(|error| error["path"][0].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<String> = row.errors.iter().map(|path| (*path).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected errors for {expected:?}, got {answered}"
        );
        for error in answered["errors"].as_array().unwrap_or(&Vec::new()) {
            let message = error["message"].as_str().unwrap_or_default();
            assert!(
                !message.is_empty(),
                "case {case:?}: every reported error states what went wrong, got {error}"
            );
        }
    }
}

#[tokio::test]
async fn a_templates_own_errors_are_the_ones_the_field_reports() {
    let router = emulator();
    let api = create_api(&router, "refusals", None).await;
    for (case, field, expected) in [
        (
            "the sentence util.error was given",
            "refused",
            REFUSAL_MESSAGE.to_string(),
        ),
        (
            "the sentence util.appendError was given",
            "noted",
            NOTED_MESSAGE.to_string(),
        ),
        (
            // The refusal AppSync words for a resolver that refuses the caller the field
            // names the field being resolved and the type the schema declares for it, so it
            // is the field's own sentence rather than a fixed one.
            "the field util.unauthorized refused",
            "guarded",
            "Not Authorized to access guarded on type String".to_string(),
        ),
    ] {
        let (status, answered) = post_graphql(
            case,
            &api,
            &json!({"query": format!("query {{ {field} }}")}),
            &[("x-api-key", api.api_key.as_str())],
        )
        .await;
        let message = answered["errors"][0]["message"].as_str().unwrap_or_default();
        assert_eq!(
            (status, message),
            (200, expected.as_str()),
            "case {case:?}: expected the template's own sentence, got {status} and {answered}"
        );
    }
}

/// The sentences the two failing templates of this suite state, which are the templates'
/// own text rather than anything this emulator words.
const REFUSAL_MESSAGE: &str = "refused by the template";
const NOTED_MESSAGE: &str = "noted beside the value";

#[tokio::test]
async fn a_document_whose_fragments_expand_without_bound_is_refused_before_anything_runs() {
    let router = emulator();
    let api = create_api(&router, "expansion", None).await;

    // A chain of fragments each spreading the next several times, selecting NO field until
    // the last level: the expansion multiplies level by level, so what bounds it is the
    // count of nodes the whole document expands to rather than the fields it produces.
    let levels = 16;
    let fanout = 8;
    let mut document = String::from("query { ...Level0 }");
    for level in 0..levels {
        let spreads: String = (0..fanout)
            .map(|_| format!("...Level{} ", level + 1))
            .collect();
        document.push_str(&format!(" fragment Level{level} on Query {{ {spreads} }}"));
    }
    document.push_str(&format!(
        " fragment Level{levels} on Query {{ probe(msg: \"deep\") }}"
    ));

    let case = "a document whose fragments expand without bound";
    let (status, answered) = post_graphql(
        case,
        &api,
        &json!({"query": document}),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a document this endpoint cannot execute is a GraphQL result, got \
         {status} and {answered}"
    );
    assert!(
        answered["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty()),
        "case {case:?}: expected the expansion to be refused, got {answered}"
    );
    assert!(
        answered.get("data").is_none(),
        "case {case:?}: a document refused before execution carries no data, got {answered}"
    );

    // And the endpoint still answers the next request, so the refusal cost the request that
    // made it and nothing else.
    let (status, answered) = post_graphql(
        case,
        &api,
        &json!({"query": "query { probe(msg: \"after\") }"}),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        (status, answered["data"]["probe"].as_str()),
        (200, Some("after")),
        "case {case:?}: the endpoint serves the next request, got {status} and {answered}"
    );
}

#[tokio::test]
async fn a_resolver_reaches_the_runtimes_own_names_without_importing_them() {
    let router = emulator();
    let api = create_api(&router, "bare-names", None).await;
    let case = "a handler that calls util with no import of its own";
    let (status, answered) = post_graphql(
        case,
        &api,
        &json!({"query": "query { bare }"}),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    let value = answered["data"]["bare"].as_str().unwrap_or_default();
    assert_eq!(
        (status, answered["errors"].as_array()),
        (200, None),
        "case {case:?}: the APPSYNC_JS runtime binds its own names for the code it runs, got \
         {status} and {answered}"
    );
    // The value is the identifier the handler minted through `util.autoId`, so the answer
    // proves the helper RAN rather than that the name merely existed.
    let minted = value.strip_prefix("bare:").unwrap_or_default();
    let groups: Vec<usize> = minted.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected the identifier util.autoId minted, got {value:?}"
    );
}

/// Create one GraphQL API with an API key, [`SCHEMA`], a `NONE` data source and one resolver
/// per field the rows select, and read the address its GraphQL endpoint is served on.
///
/// `log_config` is the API's own `logConfig`, or `None` for an API that configures no logging
/// — the seam the logging suite ([`crate::graphql_api_field_logging`]) varies.
pub(crate) async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    log_config: Option<Value>,
) -> GraphqlApi {
    let mut request = json!({
        "name": format!("templates-{name}"),
        "authenticationType": "API_KEY",
    });
    if let Some(config) = log_config {
        request["logConfig"] = config;
    }
    let (status, created) = appsync(router, "POST", "/v1/apis", Some(request)).await;
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
    install_schema(router, &api_id, SCHEMA).await;
    create_data_source(router, &api_id).await;
    create_resolvers(router, &api_id).await;
    GraphqlApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Install the schema `document` on `api_id`, waiting for the clock to perform the creation.
///
/// The wait is on the CONDITION — the creation's own reported status — so nothing here depends
/// on how fast the host runs. Shared with the suites that drive a data plane of their own over
/// a schema of their own ([`crate::graphql_api_dynamodb_marshalling`],
/// [`crate::graphql_api_published_uri`]), so one reading of how a schema is installed serves
/// every one of them.
pub(crate) async fn install_schema(router: &MultiServiceRouter, api_id: &str, document: &str) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(document);
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

/// Create the local data source every resolver of this suite resolves against.
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

/// The request template of a local resolver: the `payload` the `NONE` data source passes
/// through, built from `members` — the form AppSync documents for a local resolver.
fn request_template(members: &str) -> String {
    format!("{{\"version\": \"2017-02-28\", \"payload\": {{{members}}}}}")
}

/// Attach one resolver per field the rows select: each written in mapping templates, except
/// the one field that is written in `code`, so both definition forms are exercised through
/// the same endpoint.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    let templated: &[(&str, String, String)] = &[
        (
            "probe",
            request_template("\"msg\": $util.toJson($ctx.args.msg)"),
            "$util.toJson($ctx.result.msg)".to_string(),
        ),
        (
            "upper",
            request_template("\"text\": $util.toJson($ctx.args.text)"),
            "#if($ctx.result.text)$util.toJson($ctx.result.text.toUpperCase())\
             #{else}null#end"
                .to_string(),
        ),
        (
            "total",
            request_template("\"values\": $util.toJson($ctx.args.values)"),
            "#set($sum = 0)\
             #foreach($value in $ctx.result.values)#set($sum = $sum + $value)#end\
             $util.toJson($sum)"
                .to_string(),
        ),
        (
            "refused",
            request_template("\"msg\": \"unused\""),
            format!("$util.error('{REFUSAL_MESSAGE}')"),
        ),
        (
            "noted",
            request_template("\"msg\": \"noted-value\""),
            format!("$util.appendError('{NOTED_MESSAGE}')$util.toJson($ctx.result.msg)"),
        ),
        (
            "guarded",
            request_template("\"msg\": \"unused\""),
            "$util.unauthorized()".to_string(),
        ),
    ];
    for (field, request, response) in templated {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate": request,
                "responseMappingTemplate": response,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {field} must answer 200, got {created}"
        );
    }

    // One resolver defined by `code`, so one document can select a field of each definition
    // form and be answered for both.
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": "coded",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": "export function request(ctx) { return {payload: \"coded-value\"}; }\n\
                     export function response(ctx) { return ctx.result; }",
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for the coded field must answer 200, got {created}"
    );

    // One resolver whose code calls the runtime's own helpers WITHOUT importing them, which
    // is how much of the APPSYNC_JS code in the wild is written.
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": "bare",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": "export function request(ctx) { return {payload: {id: util.autoId()}}; }\n\
                     export function response(ctx) {\n\
                       if (!ctx.result.id) { util.error('the handler minted no id'); }\n\
                       return 'bare:' + ctx.result.id;\n\
                     }",
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for the bare field must answer 200, got {created}"
    );

    // One PIPELINE resolver over a function, both written in templates: the resolver's
    // before template stashes a value, the function reads the stash, and the resolver's
    // after template answers what the function returned.
    let (status, function) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": "stashReader",
            "dataSourceName": DATA_SOURCE,
            "functionVersion": FUNCTION_VERSION,
            "requestMappingTemplate": request_template(
                "\"text\": $util.toJson(\"piped:\" + $ctx.stash.seen)",
            ),
            "responseMappingTemplate": "$util.toJson($ctx.result.text)",
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
            "requestMappingTemplate": "$util.quiet($ctx.stash.put('seen', 'stashed')){}",
            "responseMappingTemplate": "$util.toJson($ctx.prev.result)",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for the piped field must answer 200, got {created}"
    );
}

/// POST one GraphQL request to the API's published endpoint, as a client does, answering the
/// status and the document.
pub(crate) async fn post_graphql(
    case: &str,
    api: &GraphqlApi,
    body: &Value,
    fields: &[(&str, &str)],
) -> (u16, Value) {
    let (status, _, document) = post_graphql_answer(case, api, body, fields).await;
    (status, document)
}

/// The same, answering the response FIELDS as well — the seam for a case about the request
/// id an answer carries.
pub(crate) async fn post_graphql_answer(
    case: &str,
    api: &GraphqlApi,
    body: &Value,
    fields: &[(&str, &str)],
) -> (u16, Vec<(String, String)>, Value) {
    try_post_answer(case, &api.graphql, GRAPHQL_PATH, "POST", body, fields)
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
                api.graphql
            )
        })
}

/// POST (or otherwise address) one request to the address a GraphQL API publishes, over a
/// socket, and answer the status, the response fields and the document — or `None` for a
/// request nothing served at all: a connection refused or severed, or an answer that is no
/// HTTP response.
///
/// The ONE socket client of this crate's GraphQL data-plane suites: the endpoint-publication
/// suite ([`crate::graphql_api_query_endpoint`]) drives its own cases through this too, so
/// there is one reading of how a client reaches a published address rather than one per
/// suite.
pub(crate) async fn try_post_answer(
    case: &str,
    authority: &str,
    path: &str,
    method: &str,
    body: &Value,
    fields: &[(&str, &str)],
) -> Option<(u16, Vec<(String, String)>, Value)> {
    let mut stream = tokio::time::timeout(
        ANSWER_DEADLINE,
        TcpStream::connect(("127.0.0.1", loopback_port(case, authority))),
    )
    .await
    .ok()?
    .ok()?;
    let payload = body.to_string();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {authority}\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n",
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
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())?;
    let fields: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
        .collect();
    let document = serde_json::from_str(payload).unwrap_or(Value::Null);
    Some((status, fields, document))
}
