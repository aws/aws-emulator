// SPDX-License-Identifier: CC0-1.0

//! How a GraphQL request REFUSED BEFORE EXECUTION is answered, driven over the API's own
//! published endpoint.
//!
//! The rule these cases assert, one level above the request that exposed the defect (a
//! selection of a field the schema does not declare):
//!
//! > AppSync's GraphQL endpoint tells two refusal classes apart by whether the answer carries
//! > the `data` member at all. A body it cannot turn into a GraphQL OPERATION — a `query` that
//! > is not a GraphQL document, and one that carries no document at all — is refused by the
//! > HTTP front door: `400`, one error naming that class in `errorType`, no `locations`, and no
//! > `data` member. A document that PARSES and then fails GraphQL validation — a selection the
//! > type does not declare, a variable with no value for a non-null type, a variable value that
//! > does not coerce to the type it was declared with — is answered `200` with BOTH members
//! > present: `data` present and null, because validation runs before anything is executed, and
//! > a non-empty `errors`, each entry carrying the position in the document it is about
//! > (integer `line` and `column`) and NO `path` and NO `errorType`, a validation failure being
//! > a request error rather than one of AppSync's typed data-plane errors.
//!
//! The rows span what the rule generalises over: for the validation class, WHICH rule refused
//! the document (an undeclared selection, a non-null variable with no value, an explicit null
//! for one, and values of several forms that do not coerce to the scalar they were declared
//! with) and how many errors one document earns; for the front-door class, the two bodies that
//! reach no operation; and, as the control, the same document with variables that do satisfy
//! their declarations.
//!
//! The data-plane harness is the one the mapping-template suite carries (the emulator the
//! router mounts, `install_schema`, `post_graphql`), reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{
    emulator, install_schema, post_graphql, GraphqlApi,
};

/// The schema the API of this suite serves: one root field per scalar form a variable of these
/// cases is declared with, so a value can be supplied for each of them. Each field answers the
/// value its own argument carries, so it is declared to RETURN the type that argument is of.
const SCHEMA: &str = "schema { query: Query }\n\
                      \n\
                      type Query {\n  \
                      echo(id: ID!): ID\n  \
                      titled(title: String): String\n  \
                      counted(count: Int): Int\n  \
                      stamped(at: AWSTimestamp): AWSTimestamp\n  \
                      listed(ids: [ID!]): [ID!]\n\
                      }\n";

/// The `NONE` data source every resolver of this suite resolves against: the request half's
/// own payload is the result.
const DATA_SOURCE: &str = "local";

/// The `errorType` AppSync's HTTP front door refuses a body that is no GraphQL operation with.
/// Source: the recorded behaviour of that endpoint, which this suite pins on both bodies of
/// that class.
const MALFORMED: &str = "MalformedHttpRequestException";

/// Create one GraphQL API serving [`SCHEMA`], with a live API key and a resolver on every root
/// field, and read the address its GraphQL endpoint is served on.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("refused-{name}"),
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
    for (field, argument) in [
        ("echo", "id"),
        ("titled", "title"),
        ("counted", "count"),
        ("stamped", "at"),
        ("listed", "ids"),
    ] {
        let (status, attached) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate": format!(
                    "{{\"version\": \"2017-02-28\", \"payload\": \
                     {{\"value\": $util.toJson($ctx.args.{argument})}}}}"
                ),
                "responseMappingTemplate":
                    "$util.toJson($ctx.result.value)".to_string(),
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for Query.{field} must answer 200, got {attached}"
        );
    }
    GraphqlApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// The errors one answer reports, in the order it reports them.
fn errors(answered: &Value) -> Vec<Value> {
    answered["errors"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn a_document_refused_before_execution_is_answered_with_data_and_its_errors() {
    let router = emulator();
    let api = create_api(&router, "validation").await;

    /// One row: the body a client POSTs, and the fragment of AWS's own sentence each error it
    /// earns must state.
    struct Row<'a> {
        case: &'a str,
        body: Value,
        states: Vec<&'a str>,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a selection the query root does not declare",
            body: json!({"query": "query { nope }"}),
            states: vec!["Validation error of type FieldUndefined: Field 'nope'"],
        },
        Row {
            case: "two undeclared selections, one error each",
            body: json!({"query": "query { nope gone }"}),
            states: vec![
                "Validation error of type FieldUndefined: Field 'nope'",
                "Validation error of type FieldUndefined: Field 'gone'",
            ],
        },
        Row {
            case: "a non-null variable the request supplies no value for",
            body: json!({
                "query": "query Q($id: ID!) { echo(id: $id) }",
                "variables": {},
            }),
            states: vec!["Variable 'id' has coerced Null value for NonNull type 'ID!'"],
        },
        Row {
            case: "an explicit null for a non-null variable",
            body: json!({
                "query": "query Q($id: ID!) { echo(id: $id) }",
                "variables": {"id": Value::Null},
            }),
            states: vec!["Variable 'id' has coerced Null value for NonNull type 'ID!'"],
        },
        Row {
            case: "an object where an ID was declared",
            body: json!({
                "query": "query Q($id: ID!) { echo(id: $id) }",
                "variables": {"id": {"not": "an ID"}},
            }),
            states: vec!["Variable 'id' has an invalid value"],
        },
        Row {
            case: "a boolean where a String was declared",
            body: json!({
                "query": "query Q($title: String) { titled(title: $title) }",
                "variables": {"title": true},
            }),
            states: vec!["Variable 'title' has an invalid value"],
        },
        Row {
            case: "a fractional number where an Int was declared",
            body: json!({
                "query": "query Q($count: Int) { counted(count: $count) }",
                "variables": {"count": 1.5},
            }),
            states: vec!["Variable 'count' has an invalid value"],
        },
        Row {
            case: "a string where an AWSTimestamp was declared",
            body: json!({
                "query": "query Q($at: AWSTimestamp) { stamped(at: $at) }",
                "variables": {"at": "1970-01-01"},
            }),
            states: vec!["Variable 'at' has an invalid value"],
        },
        Row {
            case: "a list holding a null where non-null elements were declared",
            body: json!({
                "query": "query Q($ids: [ID!]) { listed(ids: $ids) }",
                "variables": {"ids": ["a", Value::Null]},
            }),
            states: vec!["Variable 'ids' has an invalid value"],
        },
        Row {
            case: "two variables neither of which satisfies its declaration",
            body: json!({
                "query": "query Q($id: ID!, $count: Int) { echo(id: $id) }",
                "variables": {"count": "many"},
            }),
            states: vec![
                "Variable 'id' has coerced Null value for NonNull type 'ID!'",
                "Variable 'count' has an invalid value",
            ],
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, answered) =
            post_graphql(case, &api, &row.body, &[("x-api-key", api.api_key.as_str())]).await;
        // A document that became an operation is answered as a GraphQL result, whatever
        // refused it.
        assert_eq!(
            status, 200,
            "case {case:?}: a refusal before execution is still answered 200, got {status} and \
             {answered}"
        );
        // Validation ran before execution, so the request resolved nothing: `data` is present
        // and null, which is what tells this class from the front-door class below.
        assert!(
            answered.get("data").is_some_and(Value::is_null),
            "case {case:?}: expected the data member present and null, got {answered}"
        );
        let reported = errors(&answered);
        assert_eq!(
            reported.len(),
            row.states.len(),
            "case {case:?}: expected {} error(s), got {answered}",
            row.states.len()
        );
        for (error, states) in reported.iter().zip(&row.states) {
            let message = error["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(states),
                "case {case:?}: expected a message stating {states:?}, got {error}"
            );
            // A validation error points at a position in the DOCUMENT, counted from one along
            // both axes…
            let locations = error["locations"].as_array().cloned().unwrap_or_default();
            assert!(
                !locations.is_empty()
                    && locations.iter().all(|at| {
                        at["line"].as_u64().is_some_and(|line| line >= 1)
                            && at["column"].as_u64().is_some_and(|column| column >= 1)
                    }),
                "case {case:?}: expected integer line and column locations, got {error}"
            );
            // … and, being a REQUEST error, at no path in a result and under no typed class.
            assert!(
                error.get("path").is_none() && error.get("errorType").is_none(),
                "case {case:?}: a request error carries no path and no errorType, got {error}"
            );
        }
    }
}

#[tokio::test]
async fn a_body_that_is_no_graphql_operation_is_refused_by_the_front_door() {
    let router = emulator();
    let api = create_api(&router, "malformed").await;

    // The rows differ along WHY the body reaches no operation: a document that does not parse
    // (two forms of syntax fault), and a `query` carrying no document at all.
    for (case, body, message) in [
        (
            "a document whose selection set never closes",
            json!({"query": "query { echo(id: }"}),
            "Unable to parse GraphQL query.",
        ),
        (
            "a document that is not GraphQL at all",
            json!({"query": "{{{"}),
            "Unable to parse GraphQL query.",
        ),
        (
            "a query carrying no document at all",
            json!({"query": ""}),
            "Invalid request, `query` can't be empty.",
        ),
        (
            "a query of nothing but whitespace",
            json!({"query": "   \n"}),
            "Invalid request, `query` can't be empty.",
        ),
    ] {
        let (status, answered) =
            post_graphql(case, &api, &body, &[("x-api-key", api.api_key.as_str())]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the front door's 400, got {status} and {answered}"
        );
        // No `data` member at all: nothing of the API was reached, so there is no result to
        // report a null for.
        assert!(
            answered.get("data").is_none(),
            "case {case:?}: expected no data member, got {answered}"
        );
        let reported = errors(&answered);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one error, got {answered}"
        );
        assert_eq!(
            (
                reported[0]["errorType"].as_str(),
                reported[0]["message"].as_str(),
            ),
            (Some(MALFORMED), Some(message)),
            "case {case:?}: expected {MALFORMED} stating {message:?}, got {answered}"
        );
        // The front door reports no position in a document it could not read as one.
        assert!(
            reported[0].get("locations").is_none(),
            "case {case:?}: expected no locations, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_document_whose_variables_satisfy_their_declarations_is_executed() {
    let router = emulator();
    let api = create_api(&router, "executed").await;

    // The control for every refused row above: the same declarations, with values that DO
    // coerce — a string and an integer for an ID, an integer for an Int, one value where a
    // list was declared, and nothing at all for a nullable variable.
    //
    // `data` is the answer the row must carry, and `None` where the row asserts only that the
    // operation RESOLVED: an integer supplied for an `ID` is a value the variable takes
    // ("either a string or an integer"), while what an `ID` field is then SERIALIZED as is
    // another rule, so that row pins the coercion it is about and not the serialization.
    for (case, body, data) in [
        (
            "a string for a non-null ID",
            json!({
                "query": "query Q($id: ID!) { echo(id: $id) }",
                "variables": {"id": "abc"},
            }),
            Some(json!({"echo": "abc"})),
        ),
        (
            "an integer for a non-null ID, which an ID takes as well",
            json!({
                "query": "query Q($id: ID!) { echo(id: $id) }",
                "variables": {"id": 7},
            }),
            None,
        ),
        (
            "an integer for an Int",
            json!({
                "query": "query Q($count: Int) { counted(count: $count) }",
                "variables": {"count": 3},
            }),
            Some(json!({"counted": 3})),
        ),
        (
            "nothing at all for a nullable variable",
            json!({
                "query": "query Q($title: String) { titled(title: $title) }",
                "variables": {},
            }),
            Some(json!({"titled": Value::Null})),
        ),
    ] {
        let (status, answered) =
            post_graphql(case, &api, &body, &[("x-api-key", api.api_key.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} and {answered}"
        );
        assert_eq!(
            errors(&answered),
            Vec::<Value>::new(),
            "case {case:?}: expected no error at all, got {answered}"
        );
        match data {
            Some(data) => assert_eq!(
                answered["data"], data,
                "case {case:?}: expected {data} under data, got {answered}"
            ),
            // The operation was executed: the selected field answered a value of its own
            // rather than the null a refused document carries.
            None => assert!(
                answered["data"]["echo"].is_string() || answered["data"]["echo"].is_number(),
                "case {case:?}: expected the field to resolve to the value it was given, got \
                 {answered}"
            ),
        }
    }
}
