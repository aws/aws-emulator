// SPDX-License-Identifier: CC0-1.0

//! WHICH TYPE the sentence names when a resolver REFUSES the field it is resolving, driven
//! over the API's own published GraphQL endpoint.
//!
//! The rule these rows assert, one level above the recorded case (one APPSYNC_JS resolver
//! whose request half calls `util.unauthorized()` on a `String` field of `Query`, beside a
//! sibling that resolves):
//!
//! > A refusal a resolver RAISES while resolving a field — `util.unauthorized()` in
//! > APPSYNC_JS, `$util.unauthorized()` in a mapping template, from either half of the
//! > definition — is reported inside a `200` answer as one error of the field's own path,
//! > classed `Unauthorized` and worded "Not Authorized to access <fieldName> on type
//! > <typeName>", where `<fieldName>` is the field being resolved and `<typeName>` is the type
//! > the SERVED SCHEMA DECLARES FOR THAT FIELD, with everything it is wrapped in (lists,
//! > non-null) taken off. The field itself resolves to nothing, nothing else of its definition
//! > runs, and every sibling of the document resolves beside it.
//!
//! The rows span what the rule generalises over: the KIND of type the refused field is
//! declared as (a built-in scalar, a numeric scalar, an AWS scalar, an object type, an enum
//! wrapped in a non-null list), the LANGUAGE the refusing definition is written in (APPSYNC_JS
//! `code` and mapping templates), the HALF that refuses (the request half and the response
//! half), and the DEPTH the refused field sits at (a field of the operation's root type, and a
//! field of a type reached through it, whose declaring type is neither the root nor the type
//! the sentence names).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{
    emulator, install_schema, try_post_answer, GRAPHQL_PATH,
};

/// The class AppSync reports a field the caller may not access under, and the fixed opening of
/// the sentence it states. The service's own copies are
/// `service::graphql_api::field_auth::UNAUTHORIZED_FIELD_ERROR_TYPE` and
/// `unauthorized_field_message`.
const UNAUTHORIZED_ERROR_TYPE: &str = "Unauthorized";
const UNAUTHORIZED_FIELD: &str = "Not Authorized to access";

/// The `runtime` a resolver defined by `code` names. Source: `AppSyncRuntime`'s own members
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The data source every resolver of this suite resolves against: `NONE` is "There is no data
/// source", so nothing but the definition itself decides the field.
const DATA_SOURCE: &str = "local";

/// The value the two fields that DO resolve carry, which is what proves a refusal withholds
/// its own field and nothing besides.
const OPEN_VALUE: &str = "open-value";
const SHOWN_VALUE: &str = "shown-value";

/// The schema this suite's API serves: one refused field per KIND of declared type, a type
/// reached through another field, and the fields that resolve beside the refusals.
const SCHEMA: &str = "schema { query: Query }\n\
     \n\
     type Secret {\n  token: String\n}\n\
     \n\
     enum Grade {\n  HIGH\n  LOW\n}\n\
     \n\
     type Holder {\n  deniedInner: Secret\n  shown: String\n}\n\
     \n\
     type Query {\n  \
     deniedString: String\n  \
     deniedInt: Int\n  \
     deniedJson: AWSJSON\n  \
     deniedObject: Secret\n  \
     deniedGrades: [Grade!]!\n  \
     nested: Holder\n  \
     open: String\n}\n";

/// One created GraphQL API: its id, the address its GraphQL endpoint is served on, and a live
/// API key.
struct RefusingApi {
    graphql: String,
    api_key: String,
}

#[tokio::test]
async fn a_resolver_refusing_a_field_names_the_type_the_schema_declares_for_that_field() {
    let router = emulator();
    let api = create_api(&router).await;

    /// One row: the document a client POSTs, what the answer must hold under `data`, the path
    /// the one error must name, and the type its sentence must name.
    struct Row {
        case: &'static str,
        query: &'static str,
        data: Value,
        path: Vec<Value>,
        field: &'static str,
        declared: &'static str,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a field of a built-in scalar type, refused by the request half in code",
            query: "query { deniedString open }",
            data: json!({"deniedString": Value::Null, "open": OPEN_VALUE}),
            path: vec![json!("deniedString")],
            field: "deniedString",
            declared: "String",
        },
        Row {
            case: "a field of a numeric scalar type, refused by the response half in code",
            query: "query { deniedInt open }",
            data: json!({"deniedInt": Value::Null, "open": OPEN_VALUE}),
            path: vec![json!("deniedInt")],
            field: "deniedInt",
            declared: "Int",
        },
        Row {
            case: "a field of an AWS scalar type, refused by a mapping template",
            query: "query { deniedJson open }",
            data: json!({"deniedJson": Value::Null, "open": OPEN_VALUE}),
            path: vec![json!("deniedJson")],
            field: "deniedJson",
            declared: "AWSJSON",
        },
        Row {
            case: "a field of an object type, selected with a sub-selection",
            query: "query { deniedObject { token } open }",
            data: json!({"deniedObject": Value::Null, "open": OPEN_VALUE}),
            path: vec![json!("deniedObject")],
            field: "deniedObject",
            declared: "Secret",
        },
        Row {
            case: "a field declared as a non-null list of non-null enum values",
            query: "query { deniedGrades open }",
            data: json!({"deniedGrades": Value::Null, "open": OPEN_VALUE}),
            path: vec![json!("deniedGrades")],
            field: "deniedGrades",
            declared: "Grade",
        },
        Row {
            case: "a field of a type reached through another field, whose declaring type is \
                   neither the root nor the type named",
            query: "query { nested { deniedInner { token } shown } open }",
            data: json!({
                "nested": {"deniedInner": Value::Null, "shown": SHOWN_VALUE},
                "open": OPEN_VALUE,
            }),
            path: vec![json!("nested"), json!("deniedInner")],
            field: "deniedInner",
            declared: "Secret",
        },
    ];
    for row in rows {
        let case = row.case;
        let (status, answered) = post(case, &api, &json!({"query": row.query})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused field is a GraphQL error, not a refused request, got \
             {status} and {answered}"
        );
        assert_eq!(
            answered["data"], row.data,
            "case {case:?}: expected {} under data, got {answered}",
            row.data
        );
        let errors = answered["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected exactly the refused field's own error, got {answered}"
        );
        let entry = errors.first().cloned().unwrap_or(Value::Null);
        let expected = format!(
            "{UNAUTHORIZED_FIELD} {} on type {}",
            row.field, row.declared
        );
        assert_eq!(
            (
                entry["errorType"].as_str(),
                entry["message"].as_str(),
                entry["path"].as_array().cloned().unwrap_or_default(),
            ),
            (
                Some(UNAUTHORIZED_ERROR_TYPE),
                Some(expected.as_str()),
                row.path.clone(),
            ),
            "case {case:?}: expected the {UNAUTHORIZED_ERROR_TYPE:?} error of path {:?} \
             worded {expected:?}, got {entry}",
            row.path
        );
    }
}

/// POST one GraphQL request to the API's published endpoint, as a client does, answering the
/// status and the document.
async fn post(case: &str, api: &RefusingApi, body: &Value) -> (u16, Value) {
    let (status, _, document) = try_post_answer(
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

/// Create one `API_KEY` GraphQL API serving [`SCHEMA`], with the `NONE` data source and one
/// resolver per field the rows select.
async fn create_api(router: &MultiServiceRouter) -> RefusingApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": "resolver-refusal-type",
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
    create_resolvers(router, &api_id).await;
    RefusingApi {
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Attach the resolvers the rows select: the refusals, in both languages and from both halves,
/// and the two fields that resolve beside them.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    // The refusal raised by the REQUEST half, in APPSYNC_JS code: nothing of the definition
    // runs after it, so the response half's own value is never reached.
    let refusing_request = "import { util } from '@aws-appsync/utils';\n\
         export function request(ctx) { util.unauthorized(); }\n\
         export function response(ctx) { return 'this half never runs'; }";
    // The same refusal raised by the RESPONSE half.
    let refusing_response = "import { util } from '@aws-appsync/utils';\n\
         export function request(ctx) { return {payload: null}; }\n\
         export function response(ctx) { util.unauthorized(); }";
    for (type_name, field, definition) in [
        (
            "Query",
            "deniedString",
            json!({"code": refusing_request, "runtime": runtime()}),
        ),
        (
            "Query",
            "deniedInt",
            json!({"code": refusing_response, "runtime": runtime()}),
        ),
        (
            // The refusal written in mapping templates, which is the other definition form.
            "Query",
            "deniedJson",
            json!({
                "requestMappingTemplate": "{\"version\": \"2017-02-28\", \"payload\": {}}",
                "responseMappingTemplate": "$util.unauthorized()",
            }),
        ),
        (
            "Query",
            "deniedObject",
            json!({"code": refusing_request, "runtime": runtime()}),
        ),
        (
            "Query",
            "deniedGrades",
            json!({"code": refusing_request, "runtime": runtime()}),
        ),
        (
            "Holder",
            "deniedInner",
            json!({"code": refusing_request, "runtime": runtime()}),
        ),
        (
            "Query",
            "open",
            json!({"code": returning(OPEN_VALUE), "runtime": runtime()}),
        ),
        (
            // The field reached through `nested` resolves from the value its own parent's
            // resolver produced, which is what the refused sibling beside it is withheld from.
            "Query",
            "nested",
            json!({
                "code": returning_json(&json!({"shown": SHOWN_VALUE})),
                "runtime": runtime(),
            }),
        ),
    ] {
        create_resolver(router, api_id, type_name, field, definition).await;
    }
}

/// The `runtime` member a definition written in `code` carries.
fn runtime() -> Value {
    json!({"name": RUNTIME.0, "runtimeVersion": RUNTIME.1})
}

/// The code of a resolver whose field takes the string `value`.
fn returning(value: &str) -> String {
    format!(
        "export function request(ctx) {{ return {{payload: {}}}; }}\n\
         export function response(ctx) {{ return ctx.result; }}",
        json!(value)
    )
}

/// The same, for a resolver whose field takes the object `value`.
fn returning_json(value: &Value) -> String {
    format!(
        "export function request(ctx) {{ return {{payload: {value}}}; }}\n\
         export function response(ctx) {{ return ctx.result; }}"
    )
}

/// Attach one UNIT resolver to `<type_name>.<field>`, defined by `definition`.
async fn create_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
    definition: Value,
) {
    let mut request = json!({
        "fieldName": field,
        "kind": "UNIT",
        "dataSourceName": DATA_SOURCE,
    });
    if let Some(members) = definition.as_object() {
        for (name, value) in members {
            request[name] = value.clone();
        }
    }
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for {type_name}.{field} must answer 200, got {created}"
    );
}
