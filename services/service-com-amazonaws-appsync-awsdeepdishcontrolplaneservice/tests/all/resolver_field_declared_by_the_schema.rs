// SPDX-License-Identifier: CC0-1.0

//! Which FIELD a resolver may be attached to, over `CreateResolver` and `UpdateResolver`.
//!
//! The rule these cases assert, one level above the request that exposed the defect (a
//! CloudFormation stack adding a resolver on a field its schema never declared):
//!
//! > A resolver is attached to a FIELD of a type of its API, so where the API's active schema
//! > DECLARES the type the request addresses, that declaration decides which fields of it may
//! > carry a resolver: a name the type declares no field under is refused with the modelled
//! > `NotFoundException` (HTTP 404) naming the field and the type, and the refusal writes
//! > nothing — the field carries no resolver afterwards and a resolver already attached is left
//! > exactly as it was. A type the schema declares nothing about is no judgement either way:
//! > this service attaches a resolver to the type the request names without waiting for a
//! > schema to declare it.
//!
//! The rows span what the rule generalises over: WHICH type the request addresses (a root
//! operation type, an object type reached from one, a type of a kind that declares no field at
//! all, and a name the schema never declares), whether the named field is declared by it,
//! and — because GraphQL names are case-sensitive — a name that differs from a declared one
//! only in case. Both writes of a resolver are driven over every row.
//!
//! The control-plane harness is this crate's own HTTP client for the service
//! (`crate::event_api_data_plane::appsync`) and the schema installation the data-plane suites
//! share (`install_schema`), reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{emulator, install_schema};

/// The schema the API of this suite serves: a query root and a mutation root, an object type
/// reached from them, and an enum — a kind of type that declares no field at all.
const SCHEMA: &str = "schema { query: Query, mutation: Mutation }\n\
                      \n\
                      type Query {\n  echo(msg: String): Echo\n}\n\
                      \n\
                      type Mutation {\n  note(msg: String): Echo\n}\n\
                      \n\
                      type Echo {\n  msg: String\n  nested: Echo\n}\n\
                      \n\
                      enum Tone {\n  PLAIN\n  LOUD\n}\n";

/// The `NONE` data source the resolvers of this suite resolve against.
const DATA_SOURCE: &str = "local";

/// AppSync's fixed sentence for a field the named type does not declare, whose two variable
/// parts are the field and the type the request named. Source: the service's own reading of it
/// (`crate::state::no_field_named_message`), which every write of a resolver answers with.
fn no_field_named(field: &str, type_name: &str) -> String {
    format!("No field named {field} found on type {type_name}")
}

/// One GraphQL API serving [`SCHEMA`], with the data source its resolvers name.
async fn api_serving_the_schema(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("resolver-field-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    api_id
}

/// The body both writes of a UNIT resolver carry, for the field `field` where the write names
/// it in the body (`CreateResolver`) and without where the URI carries it (`UpdateResolver`).
fn unit_resolver(field: Option<&str>) -> Value {
    let mut body = json!({
        "kind": "UNIT",
        "dataSourceName": DATA_SOURCE,
        "requestMappingTemplate":
            "{\"version\": \"2017-02-28\", \"payload\": {\"msg\": $util.toJson($ctx.args.msg)}}",
        "responseMappingTemplate": "$util.toJson($ctx.result)",
    });
    if let Some(field) = field {
        body["fieldName"] = json!(field);
    }
    body
}

/// Send one `CreateResolver` on `type_name`/`field` under `api_id`.
async fn create_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
) -> (u16, Value) {
    appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
        Some(unit_resolver(Some(field))),
    )
    .await
}

/// Send one `UpdateResolver` on `type_name`/`field` under `api_id`.
async fn update_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
) -> (u16, Value) {
    appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers/{field}"),
        Some(unit_resolver(None)),
    )
    .await
}

/// Read the resolver attached to `type_name`/`field` back through `GetResolver`.
async fn get_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
) -> (u16, Value) {
    appsync(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers/{field}"),
        None,
    )
    .await
}

/// The error class and sentence one answer reports: restJson1 names the error shape in the
/// body's `__type`, as the sibling suites read it.
fn refusal(answered: &Value) -> (Option<&str>, Option<&str>) {
    (
        answered["__type"].as_str(),
        answered["message"].as_str(),
    )
}

/// One row: the type and field a write addresses, and the refusal it earns (none for a write
/// the schema admits).
struct Row<'a> {
    case: &'a str,
    type_name: &'a str,
    field: &'a str,
    refused: Option<String>,
}

/// The table both writes are driven over.
fn rows() -> Vec<Row<'static>> {
    vec![
        Row {
            case: "a field the query root declares",
            type_name: "Query",
            field: "echo",
            refused: None,
        },
        Row {
            case: "a field the mutation root declares",
            type_name: "Mutation",
            field: "note",
            refused: None,
        },
        Row {
            case: "a field of an object type reached from a root",
            type_name: "Echo",
            field: "nested",
            refused: None,
        },
        Row {
            case: "a field the query root does not declare",
            type_name: "Query",
            field: "ghost",
            refused: Some(no_field_named("ghost", "Query")),
        },
        Row {
            case: "a field the mutation root does not declare",
            type_name: "Mutation",
            field: "vanish",
            refused: Some(no_field_named("vanish", "Mutation")),
        },
        Row {
            case: "a field an object type reached from a root does not declare",
            type_name: "Echo",
            field: "version",
            refused: Some(no_field_named("version", "Echo")),
        },
        Row {
            case: "a name that differs from a declared field only in case",
            type_name: "Query",
            field: "Echo",
            refused: Some(no_field_named("Echo", "Query")),
        },
        Row {
            case: "any field of a type whose kind declares none",
            type_name: "Tone",
            field: "PLAIN",
            refused: Some(no_field_named("PLAIN", "Tone")),
        },
        Row {
            case: "a field of a type the schema declares nothing about",
            type_name: "Unschemed",
            field: "anything",
            refused: None,
        },
    ]
}

#[tokio::test]
async fn a_create_attaches_a_resolver_only_to_a_field_the_schema_declares() {
    let router = emulator();
    let api_id = api_serving_the_schema(&router, "created").await;

    for row in rows() {
        let case = row.case;
        let (status, answered) = create_resolver(&router, &api_id, row.type_name, row.field).await;
        let read = get_resolver(&router, &api_id, row.type_name, row.field).await;
        match &row.refused {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected CreateResolver 200, got {status} and {answered}"
                );
                // The write is backed by state: the resolver reads back on the field it was
                // attached to.
                assert_eq!(
                    (read.0, read.1["resolver"]["fieldName"].as_str()),
                    (200, Some(row.field)),
                    "case {case:?}: expected the created resolver read back, got {read:?}"
                );
            }
            Some(message) => {
                assert_eq!(
                    (status, refusal(&answered)),
                    (404, (Some("NotFoundException"), Some(message.as_str()))),
                    "case {case:?}: expected AppSync's missing-field refusal, got {status} and \
                     {answered}"
                );
                // A refused write wrote nothing: the field carries no resolver afterwards.
                assert_eq!(
                    read.0, 404,
                    "case {case:?}: expected the field to carry no resolver, got {read:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_update_states_a_resolver_only_on_a_field_the_schema_declares() {
    let router = emulator();
    let api_id = api_serving_the_schema(&router, "updated").await;

    // Every row the schema admits is seeded first, so each update addresses a resolver that
    // exists: what the rule decides is the FIELD, not whether a resolver is attached to it.
    for row in rows().iter().filter(|row| row.refused.is_none()) {
        let (status, created) = create_resolver(&router, &api_id, row.type_name, row.field).await;
        assert_eq!(
            status, 200,
            "case seed {:?}: expected CreateResolver 200, got {created}",
            row.case
        );
    }

    for row in rows() {
        let case = row.case;
        let (status, answered) = update_resolver(&router, &api_id, row.type_name, row.field).await;
        match &row.refused {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected UpdateResolver 200, got {status} and {answered}"
                );
                let read = get_resolver(&router, &api_id, row.type_name, row.field).await;
                assert_eq!(
                    (read.0, read.1["resolver"]["fieldName"].as_str()),
                    (200, Some(row.field)),
                    "case {case:?}: expected the stated resolver read back, got {read:?}"
                );
            }
            Some(message) => {
                assert_eq!(
                    (status, refusal(&answered)),
                    (404, (Some("NotFoundException"), Some(message.as_str()))),
                    "case {case:?}: expected AppSync's missing-field refusal, got {status} and \
                     {answered}"
                );
            }
        }
    }

    // And the resolvers the schema does admit are exactly the ones the API holds: a refused
    // write left no field of its own behind.
    let (status, listed) = appsync(
        &router,
        "GET",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        None,
    )
    .await;
    let fields: Vec<&str> = listed["resolvers"]
        .as_array()
        .map(|resolvers| {
            resolvers
                .iter()
                .filter_map(|resolver| resolver["fieldName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        (status, fields),
        (200, vec!["echo"]),
        "case the query root's resolvers: expected only the declared field, got {listed}"
    );
}
