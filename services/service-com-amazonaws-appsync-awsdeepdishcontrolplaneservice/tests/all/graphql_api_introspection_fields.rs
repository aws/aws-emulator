// SPDX-License-Identifier: CC0-1.0

//! The fields GraphQL's INTROSPECTION SYSTEM adds to every schema, served on a GraphQL API's
//! own `GRAPHQL` endpoint.
//!
//! The rule asserted here, one level above the single document that exposed it:
//!
//! > A GraphQL API answers the three fields the introspection system adds to a schema from
//! > the API's OWN type system: `__typename` names the type the selection is on, and the
//! > query root's `__schema` and `__type(name:)` report that type system — the same one
//! > `GetIntrospectionSchema` answers with, so the control plane and the data plane cannot
//! > be told two different things about one API. No resolver is attached to any of them. An
//! > API that holds no type system has no schema to introspect, so its data plane refuses the
//! > request rather than reporting an empty type system
//! > ([`crate::graphql_api_served_schema`]).
//!
//! The rows span what the rule generalises over: which of the three fields the document
//! selects, where in the selection it sits (the root, inside a resolved object, inside an
//! introspection object), the KIND of the type read back (object, interface, enum, input
//! object, a built-in scalar, a name nothing declares), whether the field carries an alias,
//! and whether the API holds a type system at all.
//!
//! Every case goes through the socket the API publishes its endpoint on, as a client does.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
// The router, the socket client and the GraphQL path are this crate's shared data-plane
// harness, reused rather than copied.
use crate::graphql_api_mapping_template_resolvers::{
    emulator, try_post_answer, GraphqlApi, GRAPHQL_PATH,
};

/// How long a case waits for the clock to perform a schema creation, and how often it looks
/// again: the service's own span is far shorter, and the deadline is generous so a loaded
/// machine still reaches it.
const CREATION_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The `runtime` the suite's one resolver names (`AppSyncRuntime`'s own values).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The data source that resolver resolves against: `NONE` is "There is no data source", so
/// the request function's own payload is the result.
const DATA_SOURCE: &str = "local";

/// The schema the API of this suite holds: one object type implementing an interface, an
/// enum, an input object and a deprecated field, so a `__type` read spans the kinds the
/// introspection system reports differently.
const SCHEMA: &str = "schema { query: Query }\n\
                      \n\
                      interface Entry {\n  id: ID!\n}\n\
                      \n\
                      type Note implements Entry {\n  id: ID!\n  body: String\n  \
                      tags: [String!]\n  kind: Kind\n}\n\
                      \n\
                      enum Kind {\n  SHORT\n  LONG\n}\n\
                      \n\
                      input NoteFilter {\n  kind: Kind\n  limit: Int\n}\n\
                      \n\
                      type Query {\n  note(id: ID!): Note\n  notes(filter: NoteFilter): [Note]\n}\n";

/// POST one GraphQL document to the API's published endpoint and answer the response body.
async fn query(case: &str, api: &GraphqlApi, document: &str) -> Value {
    let body = json!({"query": document});
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &body,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    });
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the endpoint, got {status} with body {answered}"
    );
    assert_eq!(
        answered["errors"],
        Value::Null,
        "case {case:?}: expected no errors, got {answered}"
    );
    answered["data"].clone()
}

/// `__typename` names the type the selection is on, wherever the selection sits.
#[tokio::test]
async fn the_typename_field_names_the_type_the_selection_is_on() {
    let router = emulator();
    let api = create_api(&router, "typename").await;

    // (case, the document, the data the API must answer)
    let rows: &[(&str, &str, Value)] = &[
        (
            "the query root",
            "query { __typename }",
            json!({"__typename": "Query"}),
        ),
        (
            "the root under an alias",
            "query { named: __typename }",
            json!({"named": "Query"}),
        ),
        (
            "inside a resolved object",
            "query { note(id: \"7\") { __typename id } }",
            json!({"note": {"__typename": "Note", "id": "7"}}),
        ),
        (
            "inside each element of a resolved list",
            "query { notes { __typename } }",
            json!({"notes": [{"__typename": "Note"}, {"__typename": "Note"}]}),
        ),
        (
            "beside the type system's own fields",
            "query { __typename __schema { __typename } }",
            json!({"__typename": "Query", "__schema": {"__typename": "__Schema"}}),
        ),
        (
            "inside a type the introspection system defines",
            "query { __type(name: \"Note\") { __typename fields { __typename } } }",
            json!({"__type": {
                "__typename": "__Type",
                "fields": [
                    {"__typename": "__Field"},
                    {"__typename": "__Field"},
                    {"__typename": "__Field"},
                    {"__typename": "__Field"},
                ],
            }}),
        ),
    ];

    for (case, document, expected) in rows {
        let data = query(case, &api, document).await;
        assert_eq!(
            &data, expected,
            "case {case:?}: expected the data {expected}, got {data}"
        );
    }
}

/// `__type(name:)` reports one type of the API's own type system, by kind.
#[tokio::test]
async fn the_type_field_reports_one_type_of_the_api_s_type_system() {
    let router = emulator();
    let api = create_api(&router, "type").await;

    // (case, the document, the data the API must answer) — one row per KIND the
    // introspection system reports differently, plus a name the schema does not declare.
    let rows: &[(&str, &str, Value)] = &[
        (
            "an object type, with its fields and their types",
            "query { __type(name: \"Note\") { kind name fields { name type { kind name \
             ofType { name } } } } }",
            json!({"__type": {
                "kind": "OBJECT",
                "name": "Note",
                "fields": [
                    {"name": "id", "type": {"kind": "NON_NULL", "name": null,
                                             "ofType": {"name": "ID"}}},
                    {"name": "body", "type": {"kind": "SCALAR", "name": "String",
                                              "ofType": null}},
                    {"name": "tags", "type": {"kind": "LIST", "name": null,
                                              "ofType": {"name": null}}},
                    {"name": "kind", "type": {"kind": "ENUM", "name": "Kind",
                                              "ofType": null}},
                ],
            }}),
        ),
        (
            "the interface it implements, with the types that implement it",
            "query { __type(name: \"Entry\") { kind name possibleTypes { name } } }",
            json!({"__type": {
                "kind": "INTERFACE",
                "name": "Entry",
                "possibleTypes": [{"name": "Note"}],
            }}),
        ),
        (
            "an enum, with its values",
            "query { __type(name: \"Kind\") { kind name enumValues { name isDeprecated } } }",
            json!({"__type": {
                "kind": "ENUM",
                "name": "Kind",
                "enumValues": [
                    {"name": "SHORT", "isDeprecated": false},
                    {"name": "LONG", "isDeprecated": false},
                ],
            }}),
        ),
        (
            "an input object, with its input fields",
            "query { __type(name: \"NoteFilter\") { kind name inputFields { name } } }",
            json!({"__type": {
                "kind": "INPUT_OBJECT",
                "name": "NoteFilter",
                "inputFields": [{"name": "kind"}, {"name": "limit"}],
            }}),
        ),
        (
            "a built-in scalar the schema's fields resolve to",
            "query { __type(name: \"String\") { kind name fields { name } } }",
            json!({"__type": {"kind": "SCALAR", "name": "String", "fields": null}}),
        ),
        (
            "the query root itself, with the fields it declares",
            "query { __type(name: \"Query\") { kind fields { name args { name } } } }",
            json!({"__type": {
                "kind": "OBJECT",
                "fields": [
                    {"name": "note", "args": [{"name": "id"}]},
                    {"name": "notes", "args": [{"name": "filter"}]},
                ],
            }}),
        ),
        (
            "a name the schema does not declare",
            "query { __type(name: \"Missing\") { name } }",
            json!({"__type": Value::Null}),
        ),
        (
            "the same read under an alias",
            "query { kinds: __type(name: \"Kind\") { name } }",
            json!({"kinds": {"name": "Kind"}}),
        ),
    ];

    for (case, document, expected) in rows {
        let data = query(case, &api, document).await;
        assert_eq!(
            &data, expected,
            "case {case:?}: expected the data {expected}, got {data}"
        );
    }
}

/// `__schema` reports the API's whole type system, and reports the same one
/// `GetIntrospectionSchema` answers with — it is computed from the API's records, not from
/// the request.
#[tokio::test]
async fn the_schema_field_reports_the_type_system_the_control_plane_reports() {
    let router = emulator();
    let api = create_api(&router, "schema").await;
    let case = "the type system the API holds";

    let data = query(
        case,
        &api,
        "query { __schema { queryType { name } mutationType { name } types { name kind } \
         directives { name } } }",
    )
    .await;
    let schema = &data["__schema"];
    assert_eq!(
        schema["queryType"]["name"].as_str(),
        Some("Query"),
        "case {case:?}: expected the bound query root, got {schema}"
    );
    // The schema binds no mutation root, so introspection reports none rather than a type
    // the API does not hold.
    assert_eq!(
        schema["mutationType"],
        Value::Null,
        "case {case:?}: expected no mutation root, got {schema}"
    );
    assert!(
        schema["directives"].is_array(),
        "case {case:?}: expected the directives the schema defines, got {schema}"
    );

    // Every type the schema declares is reported, the built-in scalars its fields resolve
    // to among them.
    let mut reported: Vec<String> = schema["types"]
        .as_array()
        .map(|types| {
            types
                .iter()
                .filter_map(|entry| entry["name"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    for declared in ["Entry", "ID", "Int", "Kind", "Note", "NoteFilter", "Query", "String"] {
        assert!(
            reported.iter().any(|name| name == declared),
            "case {case:?}: expected the type {declared:?} to be reported, got {reported:?}"
        );
    }

    // ...and it is the same type system the control plane's own read answers with.
    let control_plane = crate::introspection_schema::get_schema(
        &router,
        &api.api_id,
        "format=JSON&includeDirectives=true",
    )
    .await;
    assert_eq!(
        control_plane.status, 200,
        "case {case:?}: expected HTTP 200 from GetIntrospectionSchema, got {} with {}",
        control_plane.status,
        control_plane.text()
    );
    let read: Value = serde_json::from_str(&control_plane.text()).unwrap_or(Value::Null);
    let mut answered: Vec<String> = read["data"]["__schema"]["types"]
        .as_array()
        .map(|types| {
            types
                .iter()
                .filter_map(|entry| entry["name"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    answered.sort();
    assert_eq!(
        answered, reported,
        "case {case:?}: expected the data plane to report the types the control plane does \
         ({answered:?}), got {reported:?}"
    );
}

/// An API that holds NO type system has no schema to introspect: there is nothing for the
/// introspection system to report, so the request is refused rather than answered with an
/// empty type system — the refusal the whole rule lives in
/// [`crate::graphql_api_served_schema`].
#[tokio::test]
async fn an_api_with_no_type_system_introspects_to_nothing() {
    let router = emulator();
    let api = create_bare_api(&router, "bare").await;

    // (case, the document) — each of the three introspection fields, on an API holding none
    // of the type system they report.
    let rows: &[(&str, &str)] = &[
        (
            "the type system of an API that holds none",
            "query { __schema { queryType { name } } }",
        ),
        (
            "a type of an API that declares none",
            "query { __type(name: \"Query\") { name } }",
        ),
        ("the name of a type no schema declares", "query { __typename }"),
    ];

    for (case, document) in rows {
        let (status, answered) = refused(case, &api, document).await;
        assert_eq!(
            status, 502,
            "case {case:?}: expected the schema refusal's status, got {status} with {answered}"
        );
        assert!(
            answered.get("data").is_none(),
            "case {case:?}: expected no data member, got {answered}"
        );
        assert_eq!(
            answered["errors"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected exactly one error, got {answered}"
        );
        assert_eq!(
            answered["errors"][0]["errorType"].as_str(),
            Some("GraphQLSchemaException"),
            "case {case:?}: expected the schema exception, got {answered}"
        );
    }
}

/// POST one document to `api`'s published endpoint and answer the status and the body,
/// whatever the status — the one this suite's refused rows read, where [`query`] insists on a
/// served answer.
async fn refused(case: &str, api: &GraphqlApi, document: &str) -> (u16, Value) {
    let body = json!({"query": document});
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &body,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    });
    (status, answered)
}

/// One request's introspection answers are BOUNDED: `__schema` answers the whole type
/// system, and how many times a document asks for it is the request's to choose, so a
/// document that asks for more than one request may be given is answered for as many fields
/// as fit and reports the rest as the fields that met the bound — never by materialising a
/// copy per selected field.
#[tokio::test]
async fn the_introspection_answers_of_one_request_are_bounded() {
    let router = emulator();
    let api = create_bare_api(&router, "bounded").await;
    // A type system big enough that a handful of copies of it is past any per-request
    // ceiling: one wide type beside the query root, written by the test rather than by hand.
    let fields: String = (0..WIDE_FIELDS)
        .map(|index| format!("  wideFieldNumber{index}: String\n"))
        .collect();
    let schema = format!(
        "schema {{ query: Query }}\n\ntype Query {{\n  note: String\n}}\n\ntype Wide {{\n{fields}}}\n"
    );
    install_schema_document(&router, &api.api_id, &schema).await;

    // One alias per copy asked for, each selecting one small member of it, so what the
    // answer CARRIES is small while what each field is given is the whole document.
    let selections: String = (0..ASKED_COPIES)
        .map(|index| format!("copy{index}: __schema {{ queryType {{ name }} }} "))
        .collect();
    let case = "a document asking for many copies of the type system";
    let body = json!({"query": format!("query {{ {selections} }}")});
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &body,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the endpoint must answer this request"));
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {answered}"
    );

    // The first fields are answered...
    assert_eq!(
        answered["data"]["copy0"]["queryType"]["name"].as_str(),
        Some("Query"),
        "case {case:?}: expected the first copy to be answered, got {answered}"
    );
    // ...and the ones past the bound resolve to nothing, each reported as a field that met
    // it rather than being given another copy.
    let answered_copies = (0..ASKED_COPIES)
        .filter(|index| !answered["data"][format!("copy{index}")].is_null())
        .count();
    assert!(
        answered_copies < ASKED_COPIES,
        "case {case:?}: expected the copies past the bound to resolve to nothing, got all \
         {answered_copies} of them answered"
    );
    let errors = answered["errors"].as_array().cloned().unwrap_or_default();
    assert!(
        errors
            .iter()
            .any(|error| error["message"]
                .as_str()
                .unwrap_or_default()
                .contains("introspection answers hold at most")),
        "case {case:?}: expected the bound to be reported for the fields that met it, got \
         {answered}"
    );
}

/// How many fields the wide type of the bound's own case declares, and how many copies of the
/// type system that case asks for: the product is far past any per-request ceiling, while the
/// schema itself stays well inside what one definition may hold and the asked-for fields
/// inside what one selection set may carry.
const WIDE_FIELDS: usize = 2_000;
const ASKED_COPIES: usize = 60;

/// Create one API with an API key and no schema at all.
async fn create_bare_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("introspection-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    GraphqlApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// The same, with [`SCHEMA`] installed, a local data source and a resolver on each field of
/// the query root — so a selection inside a resolved object has an object to be inside.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let api = create_bare_api(router, name).await;
    install_schema(router, &api.api_id).await;
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{}/datasources", api.api_id),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {created}");
    let resolvers: &[(&str, &str)] = &[
        (
            "note",
            "export function request(ctx) { return {payload: {id: ctx.args.id, \
             body: \"one\"}}; }\n\
             export function response(ctx) { return ctx.result; }",
        ),
        (
            "notes",
            "export function request(ctx) { return {payload: [{id: \"a\"}, {id: \"b\"}]}; }\n\
             export function response(ctx) { return ctx.result; }",
        ),
    ];
    for (field, code) in resolvers {
        let (status, attached) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{}/types/Query/resolvers", api.api_id),
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
            "CreateResolver for {field} must answer 200, got {attached}"
        );
    }
    api
}

/// Install [`SCHEMA`] on `api_id`, looking again until AppSync reports the creation done.
async fn install_schema(router: &MultiServiceRouter, api_id: &str) {
    install_schema_document(router, api_id, SCHEMA).await;
}

/// The same, for whatever document a case states.
async fn install_schema_document(
    router: &MultiServiceRouter,
    api_id: &str,
    definition: &str,
) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(definition);
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
