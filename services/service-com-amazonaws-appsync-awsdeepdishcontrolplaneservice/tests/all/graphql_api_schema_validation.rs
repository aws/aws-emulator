// SPDX-License-Identifier: CC0-1.0

//! How a GraphQL document is validated against the API's ACTIVE schema before any of it is
//! executed, driven over the API's own published endpoint.
//!
//! The rule these cases assert, one level above the request that exposed the defect (a
//! selection carrying a field only a later, rolled-back schema declared):
//!
//! > AppSync validates the incoming document against the schema the API serves BEFORE
//! > dispatching any resolver, so a selected field the type it is selected from does not
//! > declare is a failure of the REQUEST, not a field that resolves to nothing: the answer
//! > carries one `errors` entry per such selection — GraphQL's own validation error, naming
//! > the class of the failure (`FieldUndefined`), the field, the type that does not declare
//! > it and the query path it sits at — and its `data` member null, so none of the
//! > document's other fields is resolved either. A document every selection of which the
//! > schema declares is executed as before, and the fields GraphQL's introspection system
//! > adds to every schema are not selections of the API's own type system.
//!
//! The rows span what the rule generalises over: WHERE the undeclared selection sits (the
//! query root, an object type one level down, two levels down, and the mutation root), how
//! many of them a document carries, whether declared selections stand beside it, and — for
//! the accepted class — a document of declared selections only, the introspection fields,
//! and a selection on a type the schema declares nothing about.
//!
//! The data-plane harness is the one the mapping-template suite carries (the emulator the
//! router mounts, `install_schema`, `post_graphql`), reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{emulator, install_schema, post_graphql, GraphqlApi};

/// The schema the APIs of this suite serve: two root types, an object type reached from each
/// of them, and that object type nesting itself, so a selection can be written at every level
/// the walk descends through.
const SCHEMA: &str = "schema { query: Query, mutation: Mutation }\n\
                      \n\
                      type Query {\n  echo(msg: String): Echo\n  tally(values: [Int]): Int\n}\n\
                      \n\
                      type Mutation {\n  note(msg: String): Echo\n}\n\
                      \n\
                      type Echo {\n  msg: String\n  nested: Echo\n}\n";

/// The `NONE` data source every resolver of this suite resolves against: the request half's
/// own payload is the result.
const DATA_SOURCE: &str = "local";

/// The fixed opening of a validation failure's sentence and the name of the class a selection
/// the type does not declare is of. Source: the service's own reading of it
/// (`service::graphql_api::execution`), which states the same sentence it asserts here.
const VALIDATION_ERROR: &str = "Validation error of type FieldUndefined";

/// The sentence the answer must carry for one undeclared selection.
fn undefined(field: &str, type_name: &str, at: &str) -> String {
    format!("{VALIDATION_ERROR}: Field '{field}' in type '{type_name}' is undefined @ '{at}'")
}

/// Create one GraphQL API serving [`SCHEMA`], with a live API key and a resolver on each root
/// field, and read the address its GraphQL endpoint is served on.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("validated-{name}"),
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
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    // One resolver per root field, each written in mapping templates: the request half builds
    // the payload the `NONE` data source passes through, and the response half answers it.
    for (type_name, field, members, response) in [
        (
            "Query",
            "echo",
            "\"msg\": $util.toJson($ctx.args.msg)",
            "$util.toJson($ctx.result)",
        ),
        (
            "Query",
            "tally",
            "\"values\": $util.toJson($ctx.args.values)",
            "#set($sum = 0)#foreach($value in $ctx.result.values)#set($sum = $sum + $value)#end\
             $util.toJson($sum)",
        ),
        (
            "Mutation",
            "note",
            "\"msg\": $util.toJson($ctx.args.msg)",
            "$util.toJson($ctx.result)",
        ),
    ] {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate":
                    format!("{{\"version\": \"2017-02-28\", \"payload\": {{{members}}}}}"),
                "responseMappingTemplate": response,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {type_name}.{field} must answer 200, got {created}"
        );
    }
    GraphqlApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// The messages one answer reports, in the order it reports them.
fn messages(answered: &Value) -> Vec<String> {
    answered["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .map(|error| error["message"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_selection_the_active_schema_does_not_declare_is_refused_before_execution() {
    let router = emulator();
    let api = create_api(&router, "undeclared").await;

    /// One row: the document a client POSTs and the validation errors the answer must carry.
    struct Row<'a> {
        case: &'a str,
        query: Value,
        errors: Vec<String>,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a field the query root does not declare",
            query: json!({"query": "query { nope }"}),
            errors: vec![undefined("nope", "Query", "nope")],
        },
        Row {
            case: "a field the object type one level down does not declare",
            query: json!({"query": "query { echo(msg: \"x\") { msg version } }"}),
            errors: vec![undefined("version", "Echo", "echo/version")],
        },
        Row {
            case: "a field two levels down",
            query: json!({"query": "query { echo(msg: \"x\") { nested { gone } } }"}),
            errors: vec![undefined("gone", "Echo", "echo/nested/gone")],
        },
        Row {
            case: "a field the mutation root does not declare",
            query: json!({"query": "mutation { vanish(msg: \"x\") { msg } }"}),
            errors: vec![undefined("vanish", "Mutation", "vanish")],
        },
        Row {
            case: "two undeclared selections, reported in the document's own order",
            query: json!({"query": "query { nope tally(values: [1]) later }"}),
            errors: vec![
                undefined("nope", "Query", "nope"),
                undefined("later", "Query", "later"),
            ],
        },
        Row {
            case: "an undeclared selection beside declared ones",
            query: json!({
                "query": "query { tally(values: [1, 2]) echo(msg: \"x\") { msg absent } }",
            }),
            errors: vec![undefined("absent", "Echo", "echo/absent")],
        },
        Row {
            case: "an undeclared selection spliced in by a fragment",
            query: json!({
                "query": "query { echo(msg: \"x\") { ...Parts } } fragment Parts on Echo \
                          { msg missing }",
            }),
            errors: vec![undefined("missing", "Echo", "echo/missing")],
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, answered) =
            post_graphql(case, &api, &row.query, &[("x-api-key", api.api_key.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a validated request is still answered 200, got {status} and \
             {answered}"
        );
        assert_eq!(
            messages(&answered),
            row.errors,
            "case {case:?}: expected {:?}, got {answered}",
            row.errors
        );
        // Validation ran before execution, so nothing of the document was executed: the
        // answer carries its `data` member as the null it resolved to — not the values of the
        // selections the schema does declare.
        assert!(
            answered.get("data").is_some_and(Value::is_null),
            "case {case:?}: expected the data member present and null, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_document_the_active_schema_declares_is_executed() {
    let router = emulator();
    let api = create_api(&router, "declared").await;

    for (case, query, data) in [
        (
            "a declared selection of the query root",
            json!({"query": "query { tally(values: [2, 3]) }"}),
            json!({"tally": 5}),
        ),
        (
            "a declared selection of an object type one level down",
            json!({"query": "query { echo(msg: \"kept\") { msg } }"}),
            json!({"echo": {"msg": "kept"}}),
        ),
        (
            "a declared selection of the mutation root",
            json!({"query": "mutation { note(msg: \"written\") { msg } }"}),
            json!({"note": {"msg": "written"}}),
        ),
        (
            "the type-name field the introspection system adds to every object",
            json!({"query": "query { echo(msg: \"x\") { __typename msg } }"}),
            json!({"echo": {"__typename": "Echo", "msg": "x"}}),
        ),
        (
            "the introspection root field, which no type of the schema declares",
            json!({"query": "query { __schema { queryType { name } } }"}),
            json!({"__schema": {"queryType": {"name": "Query"}}}),
        ),
    ] {
        let (status, answered) =
            post_graphql(case, &api, &query, &[("x-api-key", api.api_key.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} and {answered}"
        );
        assert_eq!(
            messages(&answered),
            Vec::<String>::new(),
            "case {case:?}: expected no validation error, got {answered}"
        );
        assert_eq!(
            answered["data"], data,
            "case {case:?}: expected {data} under data, got {answered}"
        );
    }
}

#[tokio::test]
async fn the_schema_the_api_serves_now_is_the_one_a_document_is_validated_against() {
    let router = emulator();
    let api = create_api(&router, "replaced").await;
    let selection = json!({"query": "query { echo(msg: \"x\") { msg version } }"});

    // Against the schema installed above, `version` is a selection no type declares.
    let (_, answered) = post_graphql(
        "before the schema declares the field",
        &api,
        &selection,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        messages(&answered),
        vec![undefined("version", "Echo", "echo/version")],
        "case before the schema declares the field: expected the validation error, got \
         {answered}"
    );

    // A schema that DOES declare it makes the same document valid: what a document is
    // validated against is the schema the API serves now.
    install_schema(
        &router,
        &api.api_id,
        "schema { query: Query, mutation: Mutation }\n\
         \n\
         type Query {\n  echo(msg: String): Echo\n  tally(values: [Int]): Int\n}\n\
         \n\
         type Mutation {\n  note(msg: String): Echo\n}\n\
         \n\
         type Echo {\n  msg: String\n  version: String\n  nested: Echo\n}\n",
    )
    .await;
    let (status, answered) = post_graphql(
        "after the schema declares the field",
        &api,
        &selection,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        (status, messages(&answered)),
        (200, Vec::<String>::new()),
        "case after the schema declares the field: expected the document to be executed, got \
         {answered}"
    );
    assert_eq!(
        answered["data"],
        json!({"echo": {"msg": "x", "version": Value::Null}}),
        "case after the schema declares the field: expected the declared field to resolve to \
         nothing, got {answered}"
    );

    // And installing a schema that drops it again refuses the document once more: the
    // validation follows the ACTIVE schema in both directions.
    install_schema(&router, &api.api_id, SCHEMA).await;
    let (_, answered) = post_graphql(
        "after the schema drops the field again",
        &api,
        &selection,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        messages(&answered),
        vec![undefined("version", "Echo", "echo/version")],
        "case after the schema drops the field again: expected the validation error, got \
         {answered}"
    );
}
