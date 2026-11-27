// SPDX-License-Identifier: CC0-1.0

//! `GraphqlApi$introspectionConfig`: whether a GraphQL API serves GraphQL's INTROSPECTION
//! SYSTEM on its own data plane.
//!
//! The rule asserted here, one level above the single document that exposed it:
//!
//! > An API whose `introspectionConfig` enables introspection — the value an API that named
//! > none takes — answers the query root's `__schema` and `__type(name:)` from its own type
//! > system. One whose configuration DISABLES it serves a schema the introspection meta-types
//! > are no part of: the request is not refused, but every field selected THROUGH one of
//! > those two meta-fields is a field of a type that declares none, so the document fails
//! > ordinary document validation inside a 200 envelope — the `data` member null, nothing
//! > resolved, and one `FieldUndefined` validation error per such selection, naming the
//! > selected field and the meta type that declares none of it, and carrying neither a `path`
//! > nor an `errorType`. Nothing else moves with it: `__typename`, the fields the API's own
//! > schema declares and the validation of those fields are untouched, and the configuration
//! > is reversible — re-enabling it restores the identical answer.
//!
//! The rows span what the rule generalises over: which of the two meta-fields the document
//! selects, how many subfields of one it selects, which meta type those subfields belong to,
//! whether the selection carries an alias, whether the configuration was named at CREATION
//! or by an UPDATE, and which value it names.
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

/// The two values `introspectionConfig` takes. Source: the enum's own values
/// (`smithy_query(query="shape",shape="GraphQLApiIntrospectionConfig")`).
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// The schema every API of this suite holds: one query root carrying a resolved scalar field
/// and an object field, and one object type whose two fields differ in whether they are
/// declared non-null — so one `__type` read spans a wrapped and an unwrapped type.
const SCHEMA: &str = "schema { query: Query } type Query { ping: String widget: Widget } \
                      type Widget { id: ID! name: String }";

/// The data source the suite's one resolver resolves against: `NONE` is "There is no data
/// source", so the request half's own payload is the result.
const DATA_SOURCE: &str = "local";

/// What that resolver answers for `Query.ping`.
const PONG: &str = "pong";

/// The fixed opening every GraphQL validation failure of the undefined-field class states.
const FIELD_UNDEFINED: &str = "Validation error of type FieldUndefined";

/// One document that selects both of the introspection system's root meta-fields.
const BOTH_META_FIELDS: &str = "query { __schema { queryType { name } } \
                                __type(name: \"Widget\") { name kind fields { name \
                                type { kind name ofType { kind name } } } } }";

/// What that document is answered with while the API serves introspection: the type system
/// the API itself holds.
fn served_type_system() -> Value {
    json!({
        "__schema": {"queryType": {"name": "Query"}},
        "__type": {
            "name": "Widget",
            "kind": "OBJECT",
            "fields": [
                {"name": "id", "type": {
                    "kind": "NON_NULL", "name": Value::Null,
                    "ofType": {"kind": "SCALAR", "name": "ID"},
                }},
                {"name": "name", "type": {
                    "kind": "SCALAR", "name": "String", "ofType": Value::Null,
                }},
            ],
        },
    })
}

/// The sentence one gated selection is reported with: the selected field, the meta type that
/// declares none of it, and the query path the selection sits at.
fn undefined(field: &str, meta_type: &str, at: &str) -> String {
    format!("{FIELD_UNDEFINED}: Field '{field}' in type '{meta_type}' is undefined @ '{at}'")
}

/// An API that names no `introspectionConfig` serves the introspection system, which is the
/// value such an API takes — and so does one that names the enabling value itself.
#[tokio::test]
async fn an_api_that_enables_introspection_answers_the_meta_fields_from_its_type_system() {
    for named in [None, Some(ENABLED)] {
        let router = emulator();
        let api = create_api(&router, "enabled", named).await;
        let case = match named {
            None => "an API that named no configuration",
            Some(value) => value,
        };
        assert_eq!(
            reported_config(&router, &api).await,
            Some(ENABLED.to_string()),
            "case {case:?}: expected the API to report the enabling configuration"
        );

        // (case, the document, the data the API must answer)
        let rows: &[(&str, &str, Value)] = &[
            (
                "both meta-fields in one document",
                BOTH_META_FIELDS,
                served_type_system(),
            ),
            (
                "the type system alone",
                "query { __schema { queryType { name } } }",
                json!({"__schema": {"queryType": {"name": "Query"}}}),
            ),
            (
                "one type alone",
                "query { __type(name: \"Widget\") { kind } }",
                json!({"__type": {"kind": "OBJECT"}}),
            ),
            (
                "the query root read as a type",
                "query { __type(name: \"Query\") { name fields { name } } }",
                json!({"__type": {
                    "name": "Query",
                    "fields": [{"name": "ping"}, {"name": "widget"}],
                }}),
            ),
            (
                "a meta-field under an alias",
                "query { described: __schema { queryType { name } } }",
                json!({"described": {"queryType": {"name": "Query"}}}),
            ),
            (
                "a name the schema does not declare",
                "query { __type(name: \"Absent\") { name } }",
                json!({"__type": Value::Null}),
            ),
        ];
        for (row, document, expected) in rows {
            let case = format!("{case}: {row}");
            let (status, answered) = post(&case, &api, document).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answered}"
            );
            assert_eq!(
                answered.get("errors"),
                None,
                "case {case:?}: expected no errors, got {answered}"
            );
            assert_eq!(
                &answered["data"], expected,
                "case {case:?}: expected {expected} under data, got {answered}"
            );
        }
    }
}

/// An API whose configuration DISABLES introspection leaves every field selected through
/// either meta-field undefined: one validation error per such selection, in a 200 envelope
/// whose `data` is null.
#[tokio::test]
async fn an_api_that_disables_introspection_leaves_every_meta_field_selection_undefined() {
    let router = emulator();
    let api = create_api(&router, "disabled", Some(DISABLED)).await;
    assert_eq!(
        reported_config(&router, &api).await,
        Some(DISABLED.to_string()),
        "the API created with the disabling configuration must report it"
    );

    // (case, the document, the sentences the API must report, in the order it reports them)
    let rows: &[(&str, &str, Vec<String>)] = &[
        (
            "both meta-fields in one document",
            BOTH_META_FIELDS,
            vec![
                undefined("queryType", "__Schema", "__schema/queryType"),
                undefined("name", "__Type", "__type/name"),
                undefined("kind", "__Type", "__type/kind"),
                undefined("fields", "__Type", "__type/fields"),
            ],
        ),
        (
            "one subfield of the type system",
            "query { __schema { queryType { name } } }",
            vec![undefined("queryType", "__Schema", "__schema/queryType")],
        ),
        (
            "several subfields of the type system",
            "query { __schema { types { name } directives { name } mutationType { name } } }",
            vec![
                undefined("types", "__Schema", "__schema/types"),
                undefined("directives", "__Schema", "__schema/directives"),
                undefined("mutationType", "__Schema", "__schema/mutationType"),
            ],
        ),
        (
            "one subfield of one type",
            "query { __type(name: \"Widget\") { kind } }",
            vec![undefined("kind", "__Type", "__type/kind")],
        ),
        (
            "a meta-field selected under an alias",
            "query { described: __schema { queryType { name } } }",
            vec![undefined("queryType", "__Schema", "__schema/queryType")],
        ),
        (
            "a meta-field selected twice over",
            "query { __schema { queryType { name } } other: __schema { types { name } } }",
            vec![
                undefined("queryType", "__Schema", "__schema/queryType"),
                undefined("types", "__Schema", "__schema/types"),
            ],
        ),
    ];
    for (case, document, expected) in rows {
        let (status, answered) = post(case, &api, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the request is not refused, got {status} with {answered}"
        );
        // Validation ran before execution, so nothing resolved: the `data` member is there
        // and null rather than absent.
        assert!(
            answered.get("data").is_some_and(Value::is_null),
            "case {case:?}: expected the data member present and null, got {answered}"
        );
        let errors = answered["errors"].as_array().cloned().unwrap_or_default();
        let reported: Vec<String> = errors
            .iter()
            .map(|error| error["message"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected the sentences {expected:?}, got {answered}"
        );
        for error in &errors {
            // A validation error is a failure of the REQUEST rather than one of AppSync's
            // typed data-plane errors, so it names neither a field it happened at nor a
            // class.
            assert_eq!(
                error.get("path"),
                None,
                "case {case:?}: a validation error carries no path, got {error}"
            );
            assert_eq!(
                error.get("errorType"),
                None,
                "case {case:?}: a validation error carries no errorType, got {error}"
            );
        }
        // The word the configuration is named by is never part of the answer: the document
        // failed validation, which says nothing about why the fields are undefined.
        assert!(
            !answered.to_string().to_lowercase().contains("introspection"),
            "case {case:?}: expected the refusal to name no feature, got {answered}"
        );
    }
}

/// The configuration takes the INTROSPECTION SYSTEM away and nothing else: `__typename`, the
/// fields the API's own schema declares, and the validation of those fields are all answered
/// exactly as they are on an API that serves introspection.
#[tokio::test]
async fn disabling_introspection_leaves_the_api_s_own_fields_untouched() {
    let router = emulator();
    let api = create_api(&router, "untouched", Some(DISABLED)).await;

    // (case, the document, the data the API must answer, the sentences it must report)
    let rows: &[(&str, &str, Value, Vec<String>)] = &[
        (
            "a field the schema declares, resolved by its resolver",
            "query { ping }",
            json!({"ping": PONG}),
            Vec::new(),
        ),
        (
            "the type system's own __typename beside it",
            "query { __typename ping }",
            json!({"__typename": "Query", "ping": PONG}),
            Vec::new(),
        ),
        (
            "that same meta-field under an alias",
            "query { named: __typename }",
            json!({"named": "Query"}),
            Vec::new(),
        ),
        (
            "a field the schema does not declare, still reported as undefined",
            "query { absent }",
            Value::Null,
            vec![format!(
                "{FIELD_UNDEFINED}: Field 'absent' in type 'Query' is undefined @ 'absent'"
            )],
        ),
    ];
    for (case, document, data, expected) in rows {
        let (status, answered) = post(case, &api, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answered}"
        );
        assert_eq!(
            &answered["data"], data,
            "case {case:?}: expected {data} under data, got {answered}"
        );
        let reported: Vec<String> = answered["errors"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|error| error["message"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected the sentences {expected:?}, got {answered}"
        );
    }
}

/// The configuration is REVERSIBLE, and it is the API's own record that decides: the same
/// document is answered from the type system, then left undefined, then answered identically
/// again, as an update names each value.
#[tokio::test]
async fn the_configuration_decides_each_request_and_is_reversible() {
    let router = emulator();
    let api = create_api(&router, "reversible", None).await;

    let case = "the type system while the API serves introspection";
    let (status, served) = post(case, &api, BOTH_META_FIELDS).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {served}"
    );
    assert_eq!(
        served["data"],
        served_type_system(),
        "case {case:?}: expected the API's own type system, got {served}"
    );

    let case = "the same document once an update disables introspection";
    set_config(&router, &api, "reversible", DISABLED).await;
    assert_eq!(
        reported_config(&router, &api).await,
        Some(DISABLED.to_string()),
        "case {case:?}: expected the update to be reported by the reader"
    );
    let (status, gated) = post(case, &api, BOTH_META_FIELDS).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {gated}"
    );
    assert!(
        gated.get("data").is_some_and(Value::is_null),
        "case {case:?}: expected the data member present and null, got {gated}"
    );
    assert_eq!(
        gated["errors"].as_array().map(Vec::len),
        Some(4),
        "case {case:?}: expected one sentence per selected subfield, got {gated}"
    );

    let case = "the same document once an update enables it again";
    set_config(&router, &api, "reversible", ENABLED).await;
    assert_eq!(
        reported_config(&router, &api).await,
        Some(ENABLED.to_string()),
        "case {case:?}: expected the update to be reported by the reader"
    );
    let (status, restored) = post(case, &api, BOTH_META_FIELDS).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {restored}"
    );
    assert_eq!(
        restored, served,
        "case {case:?}: expected the identical answer back, got {restored}"
    );
}

/// POST one GraphQL document to the API's published endpoint, as a client does.
async fn post(case: &str, api: &GraphqlApi, document: &str) -> (u16, Value) {
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

/// What `GetGraphqlApi` reports as this API's `introspectionConfig`.
async fn reported_config(router: &MultiServiceRouter, api: &GraphqlApi) -> Option<String> {
    let (status, read) = appsync(router, "GET", &format!("/v1/apis/{}", api.api_id), None).await;
    assert_eq!(status, 200, "GetGraphqlApi must answer 200, got {read}");
    read["graphqlApi"]["introspectionConfig"]
        .as_str()
        .map(ToString::to_string)
}

/// Update the API's `introspectionConfig` to `config`, leaving the rest of it as created.
async fn set_config(router: &MultiServiceRouter, api: &GraphqlApi, name: &str, config: &str) {
    let (status, updated) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{}", api.api_id),
        Some(json!({
            "name": api_name(name),
            "authenticationType": "API_KEY",
            "introspectionConfig": config,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateGraphqlApi must answer 200, got {updated}"
    );
}

/// Create one API with [`SCHEMA`] installed, an API key, a local data source and a resolver on
/// `Query.ping` — and the `introspectionConfig` the case names, or none at all.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    config: Option<&str>,
) -> GraphqlApi {
    let mut request = json!({"name": api_name(name), "authenticationType": "API_KEY"});
    if let Some(config) = config {
        request["introspectionConfig"] = json!(config);
    }
    let (status, created) = appsync(router, "POST", "/v1/apis", Some(request)).await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let record = &created["graphqlApi"];
    let api_id = record["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(record, "GRAPHQL");
    install_schema(router, &api_id).await;
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    let (status, attached) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": "ping",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "requestMappingTemplate":
                format!("{{\"version\":\"2018-05-29\",\"payload\":\"{PONG}\"}}"),
            "responseMappingTemplate": "$util.toJson($ctx.result)",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateResolver must answer 200, got {attached}");
    GraphqlApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// The `name` the API of one case is created under, which an update of it has to carry again
/// (`UpdateGraphqlApi$name` is required).
fn api_name(case: &str) -> String {
    format!("introspection-config-{case}")
}

/// Install [`SCHEMA`] on `api_id`, looking again until AppSync reports the creation done.
async fn install_schema(router: &MultiServiceRouter, api_id: &str) {
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
