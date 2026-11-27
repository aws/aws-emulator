// SPDX-License-Identifier: CC0-1.0

//! The VALUES a resolver may end its own step with, over the address the API publishes for its
//! GraphQL endpoint.
//!
//! The rule that a request handler calling `runtime.earlyReturn(value)` ends its step — the
//! data source is not called, the response handler does not run — is pinned by
//! [`crate::graphql_api_early_return`], across the steps it can happen in and the data sources
//! a step may name. This suite carries the rows that rule generalises over which that one does
//! not: the JSON FORMS `value` may take and how many fields of one document may each end their
//! own step.
//!
//! > The value a handler ends its step with is the step's result whatever JSON form it takes —
//! > a number, `null`, an object the request selects only part of — and each field of one
//! > document that ends its own step takes its own value, with no `errors` member for any of
//! > them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::try_post_answer;

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The `runtime` a resolver defined by `code` names. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// How long a case waits for the clock to perform the schema creation, and how often it looks
/// again.
const CREATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
const POLL_SPACING: std::time::Duration = std::time::Duration::from_millis(100);

/// The data source every resolver here resolves against: `NONE` is "there is no data source",
/// so nothing but the resolver's own definition can decide the field.
const DATA_SOURCE: &str = "local";

/// The schema every API of this suite is created with: one field per row below.
const SCHEMA: &str = "type Note {\n  id: ID\n  text: String\n}\n\
                      \n\
                      type Query {\n  counted: Int\n  nothing: String\n  note: Note\n  \
                      marker: String\n}\n";

/// What every response handler of this suite answers: a value no row expects, so a row that
/// read it back is one whose response handler RAN when the early return had ended its step.
const RESPONSE_MARKER: &str = "the response handler ran";

/// One created GraphQL API: the address its GraphQL endpoint is served on and a live API key.
struct GraphqlApi {
    graphql: String,
    api_key: String,
}

/// A router with just this service mounted: every row resolves against a `NONE` data source,
/// so no other service is reached.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService::new(engine),
    );
    router
}

#[tokio::test]
async fn the_value_a_step_ends_with_is_its_result_in_every_form_it_takes() {
    let router = emulator();
    let api = create_api(&router, "forms").await;

    /// One row: the document a client POSTs, and the data the API must answer it with.
    struct Row {
        case: &'static str,
        query: Value,
        data: Value,
    }
    let rows: &[Row] = &[
        Row {
            case: "a number, which is not a string the value could be read as",
            query: json!({"query": "query { counted }"}),
            data: json!({"counted": 7}),
        },
        Row {
            case: "null, which is a value a handler may end with",
            query: json!({"query": "query { nothing }"}),
            data: json!({"nothing": Value::Null}),
        },
        Row {
            case: "an object the request selects only part of",
            query: json!({"query": "query { note { text } }"}),
            data: json!({"note": {"text": "the note"}}),
        },
        Row {
            case: "three fields of one document, each ending its own step",
            query: json!({"query": "query { counted nothing marker }"}),
            data: json!({"counted": 7, "nothing": Value::Null, "marker": MARKER}),
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
        // The step ended with a value, so nothing of it failed: a field that resolved reports
        // no error of its own.
        assert!(
            answered.get("errors").is_none(),
            "case {case:?}: expected no errors key, got {answered}"
        );
    }
}

/// The marker the `marker` resolver ends with, which is what proves the ANSWER came from the
/// stored resolver's own code.
const MARKER: &str = "ended-early";

/// Create one GraphQL API with an API key, [`SCHEMA`], the `NONE` data source and one resolver
/// per field the rows select, and read the address its GraphQL endpoint is served on.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("early-return-{name}"),
            "authenticationType": "API_KEY",
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
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {created}");
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

/// The code of one resolver whose request handler ends its step with `ended`, and whose
/// response handler answers [`RESPONSE_MARKER`].
fn ending_code(ended: &str) -> String {
    format!(
        "export function request(ctx) {{ runtime.earlyReturn({ended}); }}\n\
         export function response(ctx) {{ return \"{RESPONSE_MARKER}\"; }}\n"
    )
}

/// Attach one resolver per field the rows select.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    let unit: &[(&str, String)] = &[
        ("counted", ending_code("3 + 4")),
        ("nothing", ending_code("null")),
        ("note", ending_code("{id: \"n-1\", text: \"the note\"}")),
        ("marker", ending_code(&format!("\"{MARKER}\""))),
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
}

/// POST one GraphQL request to the API's published endpoint, as a client does.
async fn post_graphql(
    case: &str,
    api: &GraphqlApi,
    body: &Value,
    fields: &[(&str, &str)],
) -> (u16, Value) {
    let (status, _, document) =
        try_post_answer(case, &api.graphql, GRAPHQL_PATH, "POST", body, fields)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
                    api.graphql
                )
            });
    (status, document)
}
