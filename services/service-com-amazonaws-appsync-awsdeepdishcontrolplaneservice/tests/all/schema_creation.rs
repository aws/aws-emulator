// SPDX-License-Identifier: CC0-1.0

//! The schema of a GraphQL API, end to end over HTTP: `StartSchemaCreation`, the
//! `GetSchemaCreationStatus` that reports how it went, and the types the created schema
//! leaves the API holding.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. creating a schema is ASYNCHRONOUS: the call schedules the creation and answers
//!    the non-terminal `PROCESSING` state, which the clock then carries to `SUCCESS`
//!    with no further call, and the document's own types are what the API holds
//!    afterwards — each addressable by the name its definition declares, carrying that
//!    definition's description, the kind its keyword declares and an ARN named under the
//!    API's own path. Asserted over a table of documents spanning every kind of type
//!    GraphQL's SDL grammar declares, a document that also carries definitions which
//!    declare no type (a `schema` definition, a directive definition) and one that
//!    EXTENDS a type it declares;
//! 2. a document that defines no type system the API can hold settles to `FAILED`,
//!    carrying the reason as the status detail and leaving the API without types — over
//!    a table spanning a document GraphQL cannot parse, one that declares no type, one
//!    that declares a name twice, one that extends a type it does not declare, one that
//!    references a type nothing defines, and a payload that is not text at all;
//! 3. a schema document is the WHOLE type system: the types a creation installs replace
//!    the ones the API held, so a type the new document does not declare is gone,
//!    whether an earlier document or a `CreateType` put it there;
//! 4. while the creation the clock owes is in progress, a second creation and a
//!    `CreateType` are both refused 409 `ConcurrentModificationException`, and the
//!    creation those refusals left alone still comes due;
//! 5. an API no creation has ever run for is in `NOT_APPLICABLE`, and the REPORTING call
//!    refuses an `apiId` that cannot name an API with a 400 `BadRequestException` and one
//!    that names no API this account holds with a 404 `NotFoundException` (what the
//!    SUBMITTING call owes such an id is pinned by
//!    `schema_creation_plane_refusals`).
//!
//! The harness (`router`, `call`, `create_api`) is the one the association suites
//! already carry, so every assertion goes through the router the production emulator
//! mounts: routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.

use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{call, create_api, router, Answer, ACCOUNT, REGION};

/// How long a case that must observe the far side of the scheduled creation waits: the
/// service's own span plus slack. Only the TEST waits — the handler stores the due
/// instant.
pub(crate) const PAST_THE_CREATION: Duration = Duration::from_millis(250);

/// How long a wait for a scheduled creation keeps looking before it gives up: the
/// service's own span (`state::SCHEMA_CREATION_SPAN`) with room for a machine far slower
/// than this one, so nothing that waits depends on how fast the host runs.
pub(crate) const CREATION_DEADLINE: Duration = Duration::from_secs(60);

/// How long such a wait leaves between two looks at the creation's status.
pub(crate) const POLL_SPACING: Duration = Duration::from_millis(25);

/// Every value of the `SchemaStatus` enum — the whole set the `status` member of either
/// schema operation can take.
///
/// Source: `smithy_query(query="shape", shape="SchemaStatus")`.
pub(crate) const SCHEMA_STATUSES: &[&str] = &[
    "ACTIVE",
    "DELETING",
    "FAILED",
    "NOT_APPLICABLE",
    "PROCESSING",
    "SUCCESS",
];

/// `StartSchemaCreation`'s and `GetSchemaCreationStatus`'s shared path.
pub(crate) fn schema_path(api_id: &str) -> String {
    format!("/v1/apis/{api_id}/schemacreation")
}

/// Start the creation of `definition` on `api_id`.
///
/// The definition is a blob, which restJson1 carries base64-encoded in the JSON body —
/// the bytes are handed over exactly as a client would send them, so a payload that is
/// not text can be exercised too.
pub(crate) async fn start_creation(
    router: &MultiServiceRouter,
    api_id: &str,
    definition: &[u8],
) -> Answer {
    let encoded = base64::engine::general_purpose::STANDARD.encode(definition);
    call(
        router,
        "POST",
        &schema_path(api_id),
        Some(json!({"definition": encoded})),
    )
    .await
}

/// Read the schema creation status of `api_id`.
pub(crate) async fn creation_status(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(router, "GET", &schema_path(api_id), None).await
}

/// Wait until the schema creation `api_id` has pending has been PERFORMED — until the
/// status it reports has left the non-terminal one — and report that read.
///
/// A wait on the CONDITION rather than on a fixed time: the transition is one the clock
/// owes, so the status is read again until it has left [`state::SCHEMA_PROCESSING`] or the
/// generous [`CREATION_DEADLINE`] passes, and a loaded machine and a fast one reach the
/// same assertion. The ONE poll of this plane: [`await_creation`] is this wait plus the
/// assertion that the creation succeeded, for the suites whose document defines a type
/// system, and a suite whose rows settle on EITHER terminal state waits with this one.
pub(crate) async fn await_settled_creation(
    router: &MultiServiceRouter,
    api_id: &str,
    case: &str,
) -> Answer {
    let deadline = tokio::time::Instant::now() + CREATION_DEADLINE;
    loop {
        let settled = creation_status(router, api_id).await;
        if settled.body["status"].as_str() != Some(state::SCHEMA_PROCESSING) {
            return settled;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case}: expected the schema creation to have been performed, got {settled:?}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Wait until the schema creation `api_id` has pending has been performed, and hold it to
/// having SUCCEEDED — what every suite that goes on to read what the creation installed
/// needs of it.
pub(crate) async fn await_creation(router: &MultiServiceRouter, api_id: &str, case: &str) {
    let settled = await_settled_creation(router, api_id, case).await;
    assert_eq!(
        settled.body["status"].as_str(),
        Some(state::SCHEMA_SUCCESS),
        "case {case}: expected the schema creation to have been performed, got {settled:?}"
    );
}

/// Read the type `type_name` of `api_id` back, in `format`.
pub(crate) async fn get_type(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    format: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types/{type_name}?format={format}"),
        None,
    )
    .await
}

/// Create one type under `api_id` from `definition` — the other way a type gets into an
/// API's type system.
async fn create_type(router: &MultiServiceRouter, api_id: &str, definition: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types"),
        Some(json!({"definition": definition, "format": state::SDL_FORMAT})),
    )
    .await
}

/// Start a creation and let the clock perform it, asserting it reached `SUCCESS`.
async fn create_schema(router: &MultiServiceRouter, label: &str, api_id: &str, definition: &str) {
    let started = start_creation(router, api_id, definition.as_bytes()).await;
    assert_eq!(
        started.status, 200,
        "case {label:?}: expected StartSchemaCreation 200, got {} {:?}",
        started.status, started.body
    );
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;
    let settled = creation_status(router, api_id).await;
    assert_eq!(
        settled.body["status"].as_str(),
        Some(state::SCHEMA_SUCCESS),
        "case {label:?}: expected the creation to have been performed, got {:?}",
        settled.body
    );
}

/// One type a document declares, as the API must report it afterwards.
#[derive(Debug, Clone)]
struct Declared {
    /// The name the definition declares, which the type is addressed by.
    name: &'static str,
    /// The introspection kind the definition's keyword declares.
    kind: &'static str,
    /// The definition's own description, which is the type's description.
    description: Option<&'static str>,
    /// The fields the type reports, for a kind that carries fields — the whole list, in
    /// document order, so an extension's contribution is pinned rather than sampled.
    fields: &'static [&'static str],
}

/// One row of the creation table: a schema document, the types it declares and how many
/// definitions it states.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    /// The schema document the creation carries, in GraphQL schema language.
    document: &'static str,
    declared: Vec<Declared>,
    /// The definitions the document states, which the success detail counts: the types
    /// above and every definition that declares none.
    definitions: usize,
}

/// The documents a creation succeeds on.
///
/// Source for the kinds and the `__TypeKind` each keyword declares: GraphQL's SDL type
/// definitions (object, interface, union, enum, input object, scalar) and its type
/// extensions, which `StartSchemaCreationRequest$definition` ("in GraphQL schema
/// language format") is written in. The rows differ in how many types the document
/// declares, in which kinds, in whether it also carries definitions that declare no type
/// at all, and in whether a type is extended — the shapes the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one object type",
            document: "type Query {\n  hello: String\n}\n",
            declared: vec![Declared {
                name: "Query",
                kind: state::OBJECT_KIND,
                description: None,
                fields: &["hello"],
            }],
            definitions: 1,
        },
        Row {
            label: "every kind the SDL grammar declares",
            document: "\"\"\"Anything addressable by id.\"\"\"\n\
                       interface Node {\n  id: ID!\n}\n\
                       \n\
                       \"\"\"A blog post.\"\"\"\n\
                       type Post implements Node {\n\
                       \x20 id: ID!\n\
                       \x20 title: String!\n\
                       \x20 state: PostState\n\
                       \x20 comments(limit: Int = 10): [Comment!]!\n\
                       }\n\
                       \n\
                       type Comment implements Node {\n  id: ID!\n  body: String\n}\n\
                       \n\
                       enum PostState {\n  DRAFT\n  PUBLISHED\n}\n\
                       \n\
                       input PostFilter {\n  state: PostState\n  titleContains: String\n}\n\
                       \n\
                       union Content = Post | Comment\n\
                       \n\
                       scalar Markdown\n\
                       \n\
                       type Query {\n\
                       \x20 post(id: ID!): Post\n\
                       \x20 posts(filter: PostFilter): [Post]\n\
                       \x20 rendered: Markdown\n\
                       \x20 content: Content\n\
                       }\n",
            declared: vec![
                Declared {
                    name: "Node",
                    kind: state::INTERFACE_KIND,
                    description: Some("Anything addressable by id."),
                    fields: &["id"],
                },
                Declared {
                    name: "Post",
                    kind: state::OBJECT_KIND,
                    description: Some("A blog post."),
                    fields: &["id", "title", "state", "comments"],
                },
                Declared {
                    name: "Comment",
                    kind: state::OBJECT_KIND,
                    description: None,
                    fields: &["id", "body"],
                },
                Declared {
                    name: "PostState",
                    kind: state::ENUM_KIND,
                    description: None,
                    fields: &[],
                },
                Declared {
                    name: "PostFilter",
                    kind: state::INPUT_OBJECT_KIND,
                    description: None,
                    fields: &[],
                },
                Declared {
                    name: "Content",
                    kind: state::UNION_KIND,
                    description: None,
                    fields: &[],
                },
                Declared {
                    name: "Markdown",
                    kind: state::SCALAR_KIND,
                    description: None,
                    fields: &[],
                },
                Declared {
                    name: "Query",
                    kind: state::OBJECT_KIND,
                    description: None,
                    fields: &["post", "posts", "rendered", "content"],
                },
            ],
            definitions: 8,
        },
        Row {
            label: "definitions that declare no type",
            document: "schema {\n  query: Query\n}\n\
                       \n\
                       directive @auth(level: Int) on FIELD_DEFINITION\n\
                       \n\
                       type Query {\n  hello: String @auth(level: 1)\n}\n",
            declared: vec![Declared {
                name: "Query",
                kind: state::OBJECT_KIND,
                description: None,
                fields: &["hello"],
            }],
            definitions: 3,
        },
        Row {
            label: "a type the document extends",
            document: "\"\"\"The root.\"\"\"\n\
                       type Query {\n  hello: String\n}\n\
                       \n\
                       extend type Query {\n  goodbye: String\n}\n\
                       \n\
                       enum Mood {\n  GOOD\n}\n\
                       \n\
                       extend enum Mood {\n  BAD\n}\n",
            declared: vec![
                Declared {
                    name: "Query",
                    kind: state::OBJECT_KIND,
                    description: Some("The root."),
                    fields: &["hello", "goodbye"],
                },
                Declared {
                    name: "Mood",
                    kind: state::ENUM_KIND,
                    description: None,
                    fields: &[],
                },
            ],
            definitions: 4,
        },
    ]
}

#[tokio::test]
async fn a_created_schema_is_the_types_its_document_declares() {
    let router = router();
    let rows = rows();
    let mut apis: Vec<String> = Vec::new();

    // Every row's creation is scheduled first, each on its own API, so the ONE wait
    // further down crosses the due instant of all of them: the transition is owed by the
    // clock rather than by a call, so no row needs a wait of its own.
    for row in &rows {
        let api = create_api(&router, row.label, state::GRAPHQL_API_TYPE).await;

        // Before the call there is no creation to report: the API is in none of the
        // states a schema creation moves through.
        let fresh = creation_status(&router, &api.id).await;
        assert_eq!(
            (fresh.status, fresh.body["status"].as_str()),
            (200, Some(state::SCHEMA_NOT_APPLICABLE)),
            "case {:?}: expected a fresh API to report {:?}, got {} {:?}",
            row.label,
            state::SCHEMA_NOT_APPLICABLE,
            fresh.status,
            fresh.body
        );

        // The call SCHEDULES the creation: it answers the non-terminal state the API is
        // left in, and the status the reporting call gives agrees with it.
        let started = start_creation(&router, &api.id, row.document.as_bytes()).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected StartSchemaCreation 200, got {} {:?}",
            row.label, started.status, started.body
        );
        let status = started.body["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            SCHEMA_STATUSES.contains(&status.as_str()),
            "case {:?}: expected a modelled SchemaStatus out of {SCHEMA_STATUSES:?}, got \
             {status:?}",
            row.label
        );
        assert_eq!(
            status,
            state::SCHEMA_PROCESSING,
            "case {:?}: expected the scheduled creation to report {:?}, got {status:?}",
            row.label,
            state::SCHEMA_PROCESSING
        );
        let scheduled = creation_status(&router, &api.id).await;
        assert_eq!(
            (
                scheduled.body["status"].as_str(),
                scheduled.body["details"].as_str()
            ),
            (
                Some(state::SCHEMA_PROCESSING),
                Some(state::SCHEMA_PROCESSING_DETAIL)
            ),
            "case {:?}: expected the scheduled creation to be reported as processing, got {:?}",
            row.label,
            scheduled.body
        );
        apis.push(api.id);
    }

    // No creation is started or changed in between: the transitions below are the ones
    // the clock owes, and the wait looks again for them rather than for a fixed time.
    for (row, api_id) in rows.iter().zip(&apis) {
        await_creation(&router, api_id, row.label).await;
    }

    for (row, api_id) in rows.iter().zip(&apis) {
        let settled = creation_status(&router, api_id).await;
        // The detail counts the definitions the document states: the types it declares
        // and the definitions that declare none alike. Written out here rather than read
        // off the service's own template.
        let detail = format!(
            "Successfully created schema with {} types.",
            row.definitions
        );
        assert_eq!(
            (
                settled.body["status"].as_str(),
                settled.body["details"].as_str()
            ),
            (Some(state::SCHEMA_SUCCESS), Some(detail.as_str())),
            "case {:?}: expected the creation to have been performed past the due instant, got \
             {:?}",
            row.label,
            settled.body
        );

        // The API now holds exactly the types the document declares, each reported as
        // the definition that declared it.
        for declared in &row.declared {
            let reported = get_type(&router, api_id, declared.name, state::SDL_FORMAT).await;
            assert_eq!(
                reported.status, 200,
                "case {:?} type {:?}: expected GetType 200 once the schema was created, got {} \
                 {:?}",
                row.label, declared.name, reported.status, reported.body
            );
            let r#type = &reported.body["type"];
            assert_eq!(
                r#type["name"].as_str(),
                Some(declared.name),
                "case {:?} type {:?}: expected the declared name, got {:?}",
                row.label,
                declared.name,
                r#type["name"]
            );
            assert_eq!(
                r#type["description"].as_str(),
                declared.description,
                "case {:?} type {:?}: expected the definition's description {:?}, got {:?}",
                row.label,
                declared.name,
                declared.description,
                r#type["description"]
            );
            assert_eq!(
                r#type["arn"].as_str(),
                Some(
                    format!(
                        "arn:aws:appsync:{REGION}:{ACCOUNT}:{}",
                        state::type_resource(api_id, declared.name)
                    )
                    .as_str()
                ),
                "case {:?} type {:?}: expected the type's ARN under its API's path, got {:?}",
                row.label,
                declared.name,
                r#type["arn"]
            );
            // The SDL reported is the service's rendering of that one type, so it
            // declares the name the type is addressed by.
            let sdl = r#type["definition"].as_str().unwrap_or_default();
            assert!(
                sdl.contains(declared.name),
                "case {:?} type {:?}: expected the type's own definition text, got {sdl:?}",
                row.label,
                declared.name
            );

            // The JSON representation is computed from the stored definitions, so it
            // carries the kind the keyword declared and the fields the document wrote —
            // including the ones an extension of that type contributed.
            let as_json = get_type(&router, api_id, declared.name, state::JSON_FORMAT).await;
            let introspection: Value = serde_json::from_str(
                as_json.body["type"]["definition"]
                    .as_str()
                    .unwrap_or_default(),
            )
            .unwrap_or(Value::Null);
            assert_eq!(
                introspection["kind"].as_str(),
                Some(declared.kind),
                "case {:?} type {:?}: expected kind {:?}, got {:?}",
                row.label,
                declared.name,
                declared.kind,
                introspection["kind"]
            );
            if !declared.fields.is_empty() {
                let fields: Vec<&str> = introspection["fields"]
                    .as_array()
                    .map(|fields| {
                        fields
                            .iter()
                            .map(|field| field["name"].as_str().unwrap_or_default())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    fields, declared.fields,
                    "case {:?} type {:?}: expected the document's fields {:?}, got {fields:?}",
                    row.label, declared.name, declared.fields
                );
            }
        }
    }
}

/// One row of the refusal table: a document that defines no type system, and the detail
/// the failed creation reports.
#[derive(Debug, Clone)]
struct Failure {
    label: &'static str,
    /// The payload the creation carries, as bytes — a document is text, but a client can
    /// send anything.
    document: &'static [u8],
    /// The detail the status must carry, written out here rather than read off the
    /// service's own code, so a change of wording is caught rather than followed. A
    /// document the parser rejects carries the position reading it stopped at, so only
    /// the text in front of that position is pinned.
    detail: &'static str,
    /// Whether `detail` is the whole text or only its beginning.
    whole_detail: bool,
}

/// Every ground a schema document is rejected on: one GraphQL cannot parse, one that
/// declares no type, one that declares a name twice, one that extends a type it does not
/// declare, one that references a type neither it nor GraphQL defines, and a payload
/// that is not text at all.
///
/// Each is a rule of GraphQL's own type system — a type system defines each name once,
/// an extension extends a type the document defines, and every type reference resolves —
/// so the rows span the ways a document can fail to define one rather than the one
/// document that exposed the defect.
fn failures() -> Vec<Failure> {
    vec![
        Failure {
            label: "a document GraphQL cannot parse",
            document: b"type Query {\n  hello: String\n",
            detail: "errors=[InvalidSyntaxError{ message=Invalid Syntax ,locations=\
                     [SourceLocation{line=",
            whole_detail: false,
        },
        Failure {
            label: "a document declaring no type",
            document: b"schema {\n  query: Query\n}\n",
            detail: "Found 1 problem(s) with the schema:\n\
                     The schema document declares no GraphQL type.",
            whole_detail: true,
        },
        Failure {
            label: "a name declared twice",
            document: b"type Query {\n  hello: String\n}\ntype Query {\n  goodbye: String\n}\n",
            detail: "Found 1 problem(s) with the schema:\n\
                     The schema document declares the type Query more than once.",
            whole_detail: true,
        },
        Failure {
            label: "an extension of a type the document does not declare",
            document: b"type Query {\n  hello: String\n}\nextend type Post {\n  id: ID!\n}\n",
            detail: "Found 1 problem(s) with the schema:\n\
                     The schema document extends the type Post, which it does not declare.",
            whole_detail: true,
        },
        Failure {
            label: "a reference to a type nothing defines",
            document: b"type Query {\n  post: Post\n}\n",
            detail: "Found 1 problem(s) with the schema:\n\
                     The field type 'Post' is not present when resolving type 'Query'.",
            whole_detail: true,
        },
        Failure {
            label: "two references to types nothing defines",
            document: b"type Query {\n  post: Post\n  author: Person\n}\n",
            detail: "Found 2 problem(s) with the schema:\n\
                     The field type 'Post' is not present when resolving type 'Query'.\n\
                     The field type 'Person' is not present when resolving type 'Query'.",
            whole_detail: true,
        },
        Failure {
            label: "a payload that is not text",
            document: &[0x74, 0x79, 0x70, 0x65, 0xff, 0xfe],
            detail: "errors=[InvalidSyntaxError{ message=Invalid Syntax ,locations=\
                     [SourceLocation{line=",
            whole_detail: false,
        },
    ]
}

#[tokio::test]
async fn a_document_defining_no_type_system_fails_and_leaves_no_types() {
    let router = router();
    let failures = failures();
    let mut apis: Vec<String> = Vec::new();

    // Every row's creation is scheduled first, each on its own API, so the ONE wait
    // further down crosses the due instant of all of them.
    for failure in &failures {
        let api = create_api(&router, failure.label, state::GRAPHQL_API_TYPE).await;

        // The creation is scheduled whatever the document says — the operation is
        // asynchronous, so what is wrong with the document is reported as the OUTCOME of
        // the creation rather than as the answer to this call.
        let started = start_creation(&router, &api.id, failure.document).await;
        assert_eq!(
            (started.status, started.body["status"].as_str()),
            (200, Some(state::SCHEMA_PROCESSING)),
            "case {:?}: expected StartSchemaCreation 200 {:?}, got {} {:?}",
            failure.label,
            state::SCHEMA_PROCESSING,
            started.status,
            started.body
        );
        apis.push(api.id);
    }

    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    for (failure, api_id) in failures.iter().zip(&apis) {
        let settled = creation_status(&router, api_id).await;
        assert_eq!(
            settled.body["status"].as_str(),
            Some(state::SCHEMA_FAILED),
            "case {:?}: expected the creation to have failed, got {:?}",
            failure.label,
            settled.body
        );
        let detail = settled.body["details"].as_str().unwrap_or_default();
        if failure.whole_detail {
            assert_eq!(
                detail, failure.detail,
                "case {:?}: expected the detail {:?}, got {detail:?}",
                failure.label, failure.detail
            );
        } else {
            assert!(
                detail.starts_with(failure.detail) && detail.len() > failure.detail.len(),
                "case {:?}: expected a detail opening with {:?} and stating what the parser \
                 found, got {detail:?}",
                failure.label,
                failure.detail
            );
        }

        // A creation that failed installed nothing: the API holds no type at all.
        let absent = get_type(&router, api_id, "Query", state::SDL_FORMAT).await;
        assert_eq!(
            (absent.status, absent.error_type.as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected no type to have been installed, got {} {} {:?}",
            failure.label,
            absent.status,
            absent.error_type,
            absent.body
        );
    }

    // A failed creation leaves the API usable: a document that does define a type
    // system is created on the very API the last failure was reported for.
    let failed_api = apis.last().cloned().unwrap_or_default();
    create_schema(
        &router,
        "after a failure",
        &failed_api,
        "type Query {\n  hello: String\n}\n",
    )
    .await;
    let created = get_type(&router, &failed_api, "Query", state::SDL_FORMAT).await;
    assert_eq!(
        created.status, 200,
        "case after a failure: expected a later creation to install its types, got {} {:?}",
        created.status, created.body
    );
}

#[tokio::test]
async fn a_created_schema_replaces_the_types_the_api_held() {
    let router = router();
    let api = create_api(&router, "replace", state::GRAPHQL_API_TYPE).await;

    // A type created on its own, before any schema document.
    let created = create_type(&router, &api.id, "type Legacy {\n  id: ID\n}\n").await;
    assert_eq!(
        created.status, 200,
        "case created on its own: expected CreateType 200, got {} {:?}",
        created.status, created.body
    );

    // The first document declares a type system of its own, which REPLACES what the API
    // held: the separately created type is not part of it.
    create_schema(
        &router,
        "first document",
        &api.id,
        "type Query {\n  post: Post\n}\ntype Post {\n  id: ID!\n}\n",
    )
    .await;
    for (name, expected) in [("Query", 200), ("Post", 200), ("Legacy", 404)] {
        let answer = get_type(&router, &api.id, name, state::SDL_FORMAT).await;
        assert_eq!(
            answer.status, expected,
            "case first document, type {name:?}: expected {expected}, got {} {:?}",
            answer.status, answer.body
        );
    }

    // A second document is the whole type system too, so the type the first one
    // declared and this one does not is gone.
    create_schema(
        &router,
        "second document",
        &api.id,
        "type Query {\n  hello: String\n}\n",
    )
    .await;
    for (name, expected) in [("Query", 200), ("Post", 404), ("Legacy", 404)] {
        let answer = get_type(&router, &api.id, name, state::SDL_FORMAT).await;
        assert_eq!(
            answer.status, expected,
            "case second document, type {name:?}: expected {expected}, got {} {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_creation_in_progress_refuses_another_modification() {
    let router = router();
    let api = create_api(&router, "in-progress", state::GRAPHQL_API_TYPE).await;

    let started = start_creation(
        &router,
        &api.id,
        b"type Query {\n  hello: String\n}\ntype Post {\n  id: ID!\n}\n",
    )
    .await;
    assert_eq!(
        (started.status, started.body["status"].as_str()),
        (200, Some(state::SCHEMA_PROCESSING)),
        "case in progress: expected StartSchemaCreation 200 {:?}, got {} {:?}",
        state::SCHEMA_PROCESSING,
        started.status,
        started.body
    );

    // While the creation the clock owes is in progress, the API's schema is being
    // altered: another alteration of it is refused with the status the model gives that
    // error and the sentence the service words a schema alteration already under way
    // with.
    let second = start_creation(&router, &api.id, b"type Query {\n  other: String\n}\n").await;
    let added = create_type(&router, &api.id, "type Added {\n  id: ID\n}\n").await;
    for (label, refused) in [("a second creation", second), ("a created type", added)] {
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (409, "ConcurrentModificationException"),
            "case {label:?}: expected a 409 ConcurrentModificationException while the creation is \
             owed, got {} {} {:?}",
            refused.status,
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some("Schema is currently being altered, please wait until that is complete."),
            "case {label:?}: expected the schema-alteration text, got {:?}",
            refused.body["message"]
        );
    }

    // The refusals left the creation where it was, so the clock still performs it — and
    // the document the first call carried is the one that was created. A wait on the
    // CONDITION, not on a fixed time.
    await_creation(&router, &api.id, "in progress").await;
    let settled = creation_status(&router, &api.id).await;
    assert_eq!(
        settled.body["status"].as_str(),
        Some(state::SCHEMA_SUCCESS),
        "case in progress: expected the creation to have been performed, got {:?}",
        settled.body
    );
    for (name, expected) in [("Query", 200), ("Post", 200), ("Added", 404)] {
        let answer = get_type(&router, &api.id, name, state::SDL_FORMAT).await;
        assert_eq!(
            answer.status, expected,
            "case in progress, type {name:?}: expected {expected}, got {} {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_document_at_the_type_quota_is_created_and_one_past_it_fails() {
    let router = router();

    // The published per-API type quota, and one type past it: the bound holds for a
    // whole document exactly as it does for types created one at a time. Both rows are
    // scheduled before the ONE wait, which crosses the due instant of each.
    let counts = [state::TYPES_PER_API, state::TYPES_PER_API + 1];
    let mut apis: Vec<String> = Vec::new();
    for declared in counts {
        let api = create_api(&router, &format!("quota-{declared}"), state::GRAPHQL_API_TYPE).await;
        let document: String = (0..declared)
            .map(|index| format!("type T{index} {{\n  id: ID\n}}\n"))
            .collect();

        let started = start_creation(&router, &api.id, document.as_bytes()).await;
        assert_eq!(
            started.status, 200,
            "case {declared} types: expected StartSchemaCreation 200, got {} {:?}",
            started.status, started.body
        );
        apis.push(api.id);
    }

    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    for (declared, api_id) in counts.into_iter().zip(&apis) {
        let settled = creation_status(&router, api_id).await;
        if declared <= state::TYPES_PER_API {
            assert_eq!(
                settled.body["status"].as_str(),
                Some(state::SCHEMA_SUCCESS),
                "case {declared} types: expected a document at the quota to be created, got {:?}",
                settled.body
            );
            let reported = get_type(&router, api_id, "T0", state::SDL_FORMAT).await;
            assert_eq!(
                reported.status, 200,
                "case {declared} types: expected the document's types to be installed, got {} {:?}",
                reported.status, reported.body
            );
        } else {
            assert_eq!(
                (
                    settled.body["status"].as_str(),
                    settled.body["details"].as_str()
                ),
                (
                    Some(state::SCHEMA_FAILED),
                    Some(
                        format!(
                            "Found 1 problem(s) with the schema:\n\
                             The schema document declares {declared} types, more than the {} an \
                             API can hold.",
                            state::TYPES_PER_API
                        )
                        .as_str()
                    )
                ),
                "case {declared} types: expected a document past the quota to fail, got {:?}",
                settled.body
            );
            let absent = get_type(&router, api_id, "T0", state::SDL_FORMAT).await;
            assert_eq!(
                absent.status, 404,
                "case {declared} types: expected no type to have been installed, got {} {:?}",
                absent.status, absent.body
            );
        }
    }
}

/// One row of the reference table: an `apiId` that names no API of this account, and the
/// answer the READING call of the schema plane owes it.
///
/// What the SUBMITTING call owes the same id follows a rule of its own, pinned by its own
/// suites (`schema_creation_plane_refusals`, `schema_creation_api_resolution`), so it is
/// not carried per row here.
#[derive(Debug)]
struct Reference {
    label: &'static str,
    api_id: &'static str,
    read_status: u16,
    read_error_type: &'static str,
    /// The text the reading call's refusal must carry, written out here rather than read
    /// off the service's own constant. `None` where the row only pins the status and the
    /// error shape.
    read_message: Option<&'static str>,
}

/// The ways an `apiId` can name no API: a value no AppSync API id can be — one character
/// past the 26 one has (`state::API_ID_LEN`) — and a well-formed id this account does
/// not hold, in the alphabet the service mints ids over and outside it (the alphabet is
/// no part of what makes an id nameable, so either names no API).
fn references() -> Vec<Reference> {
    vec![
        Reference {
            label: "an id too long to be one",
            api_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaa",
            read_status: 400,
            read_error_type: "BadRequestException",
            read_message: None,
        },
        Reference {
            label: "an id outside the id alphabet",
            api_id: "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            read_status: 404,
            read_error_type: "NotFoundException",
            read_message: Some("API not found."),
        },
        Reference {
            label: "an api this account does not hold",
            api_id: "abcdefghijklmnopqrstuvwxyz",
            read_status: 404,
            read_error_type: "NotFoundException",
            read_message: Some("API not found."),
        },
    ]
}

#[tokio::test]
async fn an_api_id_naming_no_api_is_refused_by_the_reporting_call() {
    let router = router();

    for reference in references() {
        let answer = creation_status(&router, reference.api_id).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (reference.read_status, reference.read_error_type),
            "case {:?} GetSchemaCreationStatus: expected {} {}, got {} {} {:?}",
            reference.label,
            reference.read_status,
            reference.read_error_type,
            answer.status,
            answer.error_type,
            answer.body
        );
        match reference.read_message {
            Some(message) => assert_eq!(
                answer.body["message"].as_str(),
                Some(message),
                "case {:?} GetSchemaCreationStatus: expected the message {message:?}, got {:?}",
                reference.label,
                answer.body["message"]
            ),
            None => assert!(
                answer.body["message"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "case {:?} GetSchemaCreationStatus: expected a message stating why, got {:?}",
                reference.label,
                answer.body["message"]
            ),
        }
    }
}
