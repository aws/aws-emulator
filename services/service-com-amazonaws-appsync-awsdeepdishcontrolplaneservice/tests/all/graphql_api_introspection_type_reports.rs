// SPDX-License-Identifier: CC0-1.0

//! What a `__Type` an introspection answer carries reports about the type it names, over the
//! GraphQL endpoint a GraphQL API publishes.
//!
//! The rule asserted here, one level above the single document that exposed it:
//!
//! > Every `__Type` an introspection answer carries IS the type the schema holds under that
//! > name, so any field of `__Type` selected on one is answered from that type: the three root
//! > operation types `__Schema` reports, the interfaces a type implements, the possible types
//! > of an interface or a union, and the type a field resolves to all report that type's kind,
//! > its fields and the rest of what `__Type` declares — never only the name by which the
//! > answer named it. A `__Type` that names NO type — the `LIST` and `NON_NULL` wrappers,
//! > whose own name is null — carries only what it is, which is what GraphQL reports for a
//! > wrapper. On a MERGED API the type system reported is the COMPOSED one, so the fields its
//! > source APIs contributed are discoverable through it.
//!
//! The rows span what the rule generalises over: WHERE the `__Type` sits in the answer (a root
//! operation type, an interface, a union member, a field's own type, a wrapper around one),
//! the KIND of the type named (object, interface, union, scalar), which of `__Type`'s fields
//! the document selects, whether the field carries an alias, and whether the API holds its
//! schema in its own right or by merge.
//!
//! The harness is this crate's shared data-plane one, reused rather than copied, and every
//! wait is a wait on the CONDITION the clock owes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::graphql_api_mapping_template_resolvers::install_schema;
use crate::graphql_api_served_schema::bare_api;
use crate::merged_api_endpoint_execution::{
    associate, await_terminal_merge, create_merged_api, emulator, given_role, source_api,
    statements, try_query, Api, MERGE_SUCCESS, SOURCE_GRAPHQL_ACTION,
};

/// The schema the standard API of this suite holds: a query root and a mutation root bound by
/// a `schema` definition, an interface, the object type that implements it, a union over that
/// object, a list of a non-null scalar, and no subscription root at all — so one read spans the
/// kinds a `__Type` is reported differently for and a wrapper that names no type.
const SCHEMA: &str = "schema {\n  query: Query\n  mutation: Mutation\n}\n\
                      \n\
                      type Query {\n  note(id: ID!): Note\n  tags: [String!]\n  \
                      entry: Entry\n  anything: Thing\n}\n\
                      \n\
                      type Mutation {\n  putNote(body: String): Note\n}\n\
                      \n\
                      interface Entry {\n  id: ID!\n}\n\
                      \n\
                      type Note implements Entry {\n  id: ID!\n  body: String\n}\n\
                      \n\
                      union Thing = Note\n";

/// The three root operation types `__Schema` reports are the types the schema binds, reported
/// as any other type of it: a selection on one is answered from that type.
#[tokio::test]
async fn the_root_operation_types_report_the_types_the_schema_binds() {
    let router = emulator();
    let api = api_with_schema(&router, "roots").await;

    // (case, the document, the data the API must answer)
    let rows: &[(&str, &str, Value)] = &[
        (
            "the query root, with the fields it declares",
            "query { __schema { queryType { kind name fields { name } } } }",
            json!({"__schema": {"queryType": {
                "kind": "OBJECT",
                "name": "Query",
                "fields": [
                    {"name": "note"},
                    {"name": "tags"},
                    {"name": "entry"},
                    {"name": "anything"},
                ],
            }}}),
        ),
        (
            "the mutation root, with its field's arguments",
            "query { __schema { mutationType { kind name fields { name args { name } } } } }",
            json!({"__schema": {"mutationType": {
                "kind": "OBJECT",
                "name": "Mutation",
                "fields": [{"name": "putNote", "args": [{"name": "body"}]}],
            }}}),
        ),
        (
            "a root operation type the schema binds none of",
            "query { __schema { subscriptionType { kind name fields { name } } } }",
            json!({"__schema": {"subscriptionType": Value::Null}}),
        ),
        (
            "the query root under an alias, with the interfaces it implements",
            "query { __schema { root: queryType { name interfaces { name } } } }",
            json!({"__schema": {"root": {"name": "Query", "interfaces": []}}}),
        ),
        (
            "the type each field of the query root resolves to",
            "query { __schema { queryType { fields { name type { name fields { name } } } } } }",
            json!({"__schema": {"queryType": {"fields": [
                {"name": "note", "type": {
                    "name": "Note",
                    "fields": [{"name": "id"}, {"name": "body"}],
                }},
                // A list wrapper names no type, so it carries none of a type's own members.
                {"name": "tags", "type": {"name": Value::Null, "fields": Value::Null}},
                {"name": "entry", "type": {"name": "Entry", "fields": [{"name": "id"}]}},
                // A union holds members rather than fields.
                {"name": "anything", "type": {"name": "Thing", "fields": Value::Null}},
            ]}}}),
        ),
    ];

    for (case, document, expected) in rows {
        let data = introspected(case, &api, document).await;
        assert_eq!(
            &data, expected,
            "case {case:?}: expected the data {expected}, got {data}"
        );
    }
}

/// The same holds wherever one type NAMES another: the reference reports the type it names.
#[tokio::test]
async fn a_type_reference_reports_the_type_it_names() {
    let router = emulator();
    let api = api_with_schema(&router, "references").await;

    // (case, the document, the data the API must answer)
    let rows: &[(&str, &str, Value)] = &[
        (
            "an interface a type implements",
            "query { __type(name: \"Note\") { interfaces { kind name fields { name } } } }",
            json!({"__type": {"interfaces": [
                {"kind": "INTERFACE", "name": "Entry", "fields": [{"name": "id"}]},
            ]}}),
        ),
        (
            "the types that implement an interface",
            "query { __type(name: \"Entry\") { possibleTypes { name fields { name } } } }",
            json!({"__type": {"possibleTypes": [
                {"name": "Note", "fields": [{"name": "id"}, {"name": "body"}]},
            ]}}),
        ),
        (
            "the members of a union",
            "query { __type(name: \"Thing\") { kind possibleTypes { name fields { name } } } }",
            json!({"__type": {
                "kind": "UNION",
                "possibleTypes": [
                    {"name": "Note", "fields": [{"name": "id"}, {"name": "body"}]},
                ],
            }}),
        ),
        (
            "the type a field resolves to, and what it is wrapped in",
            "query { __type(name: \"Query\") { fields { type { kind name ofType \
             { kind name } } } } }",
            json!({"__type": {"fields": [
                {"type": {"kind": "OBJECT", "name": "Note", "ofType": Value::Null}},
                {"type": {
                    "kind": "LIST",
                    "name": Value::Null,
                    "ofType": {"kind": "NON_NULL", "name": Value::Null},
                }},
                {"type": {"kind": "INTERFACE", "name": "Entry", "ofType": Value::Null}},
                {"type": {"kind": "UNION", "name": "Thing", "ofType": Value::Null}},
            ]}}),
        ),
        (
            "a built-in scalar a field resolves to",
            "query { __type(name: \"Note\") { fields { name type { kind name fields \
             { name } } } } }",
            json!({"__type": {"fields": [
                {"name": "id", "type": {
                    "kind": "NON_NULL",
                    "name": Value::Null,
                    "fields": Value::Null,
                }},
                // A scalar holds no field, which introspection reports as null.
                {"name": "body", "type": {
                    "kind": "SCALAR",
                    "name": "String",
                    "fields": Value::Null,
                }},
            ]}}),
        ),
    ];

    for (case, document, expected) in rows {
        let data = introspected(case, &api, document).await;
        assert_eq!(
            &data, expected,
            "case {case:?}: expected the data {expected}, got {data}"
        );
    }
}

/// A MERGED API introspects to the COMPOSED schema: its query root names exactly the fields
/// the source APIs associated with it contributed, and those fields still resolve.
#[tokio::test]
async fn a_merged_apis_introspection_reports_the_composed_schema() {
    let router = emulator();
    let role = given_role(
        &router,
        "appsync-merged-introspection",
        Some(&statements(&[("Allow", SOURCE_GRAPHQL_ACTION, "*")])),
    )
    .await;
    let one = source_api(&router, "alpha", &["sourceOnly"]).await;
    let two = source_api(&router, "beta", &["otherOnly"]).await;
    let merged = create_merged_api(&router, "introspected", &role).await;
    for source in [&one, &two] {
        let association = associate(&router, source, &merged, "AUTO_MERGE").await;
        let settled = await_terminal_merge(&router, &merged.api_id, &association).await;
        assert_eq!(
            settled["sourceApiAssociationStatus"].as_str(),
            Some(MERGE_SUCCESS),
            "setup: the merge of {association} must have been performed, got {settled}"
        );
    }

    let case = "the composed query root of a merged api";
    let data = introspected(
        case,
        &merged,
        "query { __schema { queryType { kind name fields { name } } } }",
    )
    .await;
    let root = &data["__schema"]["queryType"];
    assert_eq!(
        (root["kind"].as_str(), root["name"].as_str()),
        (Some("OBJECT"), Some("Query")),
        "case {case:?}: expected the composed query root to be reported, got {data}"
    );
    let mut named: Vec<String> = root["fields"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a list of the composed root's fields, got {data}")
        })
        .iter()
        .filter_map(|field| field["name"].as_str().map(ToString::to_string))
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec!["otherOnly".to_string(), "sourceOnly".to_string()],
        "case {case:?}: expected the fields every associated source API contributed, got {data}"
    );

    // ...and what introspection names is what the endpoint resolves.
    for (field, value) in [
        ("sourceOnly", "sourceOnly-value"),
        ("otherOnly", "otherOnly-value"),
    ] {
        let case = "a merged field named by the composed schema";
        let (status, answered) =
            try_query(case, &merged, &json!({"query": format!("query {{ {field} }}")})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {field}, got {status} with {answered}"
        );
        assert_eq!(
            answered["data"][field].as_str(),
            Some(value),
            "case {case:?}: expected {field} to resolve to {value:?}, got {answered}"
        );
    }
}

/// POST one introspection document to the address `api` publishes and answer the `data` it was
/// served, having insisted the request was served without error.
async fn introspected(case: &str, api: &Api, document: &str) -> Value {
    let (status, answered) = try_query(case, api, &json!({"query": document})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the endpoint, got {status} with {answered}"
    );
    assert_eq!(
        answered["errors"],
        Value::Null,
        "case {case:?}: expected no errors, got {answered}"
    );
    answered["data"].clone()
}

/// Create one API holding [`SCHEMA`], with an API key and the address its endpoint is served
/// on: no resolver is attached, because no resolver can be attached to an introspection field.
async fn api_with_schema(router: &MultiServiceRouter, name: &str) -> Api {
    let api = bare_api(router, name).await;
    install_schema(router, &api.api_id, SCHEMA).await;
    api
}
