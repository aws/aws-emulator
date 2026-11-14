// SPDX-License-Identifier: CC0-1.0

//! The SDL a READ of a GraphQL type answers with, end to end over HTTP.
//!
//! The rule asserted here is stated over a CLASS of definitions rather than over the one
//! request that exposed the defect: a read of a type (`GetType`, `ListTypes`) reports
//! `Type$definition` as the service's own rendering of the type it holds — the declaring
//! keyword and the type's name on a line of their own, a blank line, the body's opening
//! brace on its own line, each member unindented and followed by a blank line,
//! consecutive members separated by one further blank line, the closing brace, and a
//! terminating newline — whatever text the definition arrived as and whichever way it got
//! into the API, while the call that STATED a definition (`CreateType`, `UpdateType`)
//! reports that definition back as the request wrote it.
//!
//! The table spans the shapes the rule generalises over: every kind of type a GraphQL SDL
//! definition declares, bodies of none, one, two and three members, definitions arriving
//! on one line and over several, with and without a description, arguments, default
//! values, list and non-null types and a `@deprecated` directive. The layout itself is
//! asserted byte for byte, since the bytes are what a client diffs or hashes.
//!
//! Every assertion goes through the router the production emulator mounts.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

// The router, the API-creating helper, the type calls and the schema-creation helpers are
// the crate's shared test harness, reused rather than copied.
use super::graphql_api_lifecycle::router;
use super::graphql_types::{call, create_api, create_type, get_type, Answer};
use super::schema_creation::{await_creation, start_creation};

/// One row of the rendering table: a definition as a request writes it, and the document
/// a read of the type it declares answers with.
///
/// Shared with the suites that assert the same rendering through another operation, so one
/// table states the layout for all of them.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    /// What the row covers, named in every assertion of it.
    pub(crate) label: &'static str,
    /// The name the definition declares, which the type is addressed by.
    pub(crate) name: &'static str,
    /// The definition as the request writes it.
    pub(crate) stated: &'static str,
    /// The document a read of the stored type answers with.
    pub(crate) rendered: &'static str,
}

/// Every kind of type a GraphQL SDL definition declares, and the member shapes one can
/// carry.
///
/// Source: GraphQL's SDL type definitions (object, interface, input object, enum, union,
/// scalar), which `CreateTypeRequest$definition` references the SDL documentation for,
/// and the `__TypeKind` values this service reports them under (`state::OBJECT_KIND` and
/// the rest). The rows differ in kind, in how many members the body holds, in whether the
/// request wrote the definition on one line or over several, and in whether the
/// definition carries a description, arguments, default values, wrapped types or a
/// `@deprecated` directive.
pub(crate) fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "an object type written on one line",
            name: "Blog",
            stated: "type Blog { id: ID! title: String }",
            rendered: "type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n",
        },
        Row {
            label: "an indented object type holding one field",
            name: "Query",
            stated: "type Query {\n  getItem: String\n}\n",
            rendered: "type Query\n\n{\ngetItem: String\n\n}\n",
        },
        Row {
            label: "an object type holding three fields",
            name: "Widget",
            stated: "type Widget { id: ID! name: String! tags: [String] }",
            rendered: "type Widget\n\n{\nid: ID!\n\n\nname: String!\n\n\ntags: [String]\n\n}\n",
        },
        Row {
            label: "a described object type implementing interfaces, with arguments, a \
                    default and a deprecation",
            name: "Post",
            stated: "\"\"\"A blog post.\"\"\"\n\
                     type Post implements Node & Timestamped {\n\
                     \x20 id: ID!\n\
                     \x20 comments(limit: Int = 10, after: String): [Comment!]!\n\
                     \x20 legacyTitle: String @deprecated(reason: \"use title\")\n\
                     }\n",
            rendered: "\"\"\"A blog post.\"\"\"\n\
                       type Post implements Node & Timestamped\n\n{\n\
                       id: ID!\n\n\n\
                       comments(limit: Int = 10, after: String): [Comment!]!\n\n\n\
                       legacyTitle: String @deprecated(reason: \"use title\")\n\n\
                       }\n",
        },
        Row {
            label: "an enum type written with tab-indented values",
            name: "Shelf",
            stated: "enum Shelf {\n\tNEW\n\tOLD\n}\n",
            rendered: "enum Shelf\n\n{\nNEW\n\n\nOLD\n\n}\n",
        },
        Row {
            label: "an interface type",
            name: "Node",
            stated: "interface Node {\n  id: ID!\n}\n",
            rendered: "interface Node\n\n{\nid: ID!\n\n}\n",
        },
        Row {
            label: "an object type holding a described field",
            name: "Gadget",
            stated: "type Gadget {\n  \"\"\"What it is called.\"\"\"\n  name: String\n}\n",
            rendered: "type Gadget\n\n{\n\"\"\"What it is called.\"\"\"\nname: String\n\n}\n",
        },
        Row {
            label: "an input object type with a default value",
            name: "PostInput",
            stated: "input PostInput { title: String! tags: [String!] = [] }",
            rendered: "input PostInput\n\n{\ntitle: String!\n\n\ntags: [String!] = []\n\n}\n",
        },
        Row {
            label: "a described enum type with a deprecated value",
            name: "PostOrder",
            stated: "\"\"\"How posts are ordered.\"\"\"\n\
                     enum PostOrder {\n  NEWEST\n  OLDEST @deprecated\n}\n",
            rendered: "\"\"\"How posts are ordered.\"\"\"\n\
                       enum PostOrder\n\n{\nNEWEST\n\n\nOLDEST @deprecated\n\n}\n",
        },
        Row {
            label: "a union type, which has no body at all",
            name: "SearchResult",
            stated: "union SearchResult = Post | Blog\n",
            rendered: "union SearchResult = Post | Blog\n",
        },
        Row {
            label: "a described scalar type, which has no body at all",
            name: "Money",
            stated: "\"\"\"An amount of money.\"\"\"\nscalar Money\n",
            rendered: "\"\"\"An amount of money.\"\"\"\nscalar Money\n",
        },
    ]
}

/// Send one `ListTypes` for `api_id`, in `format`.
pub(crate) async fn list_types(router: &MultiServiceRouter, api_id: &str, format: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types?format={format}"),
        None,
    )
    .await
}

/// The type named `name` in a listing answer, or `None` when the listing does not report
/// it.
fn listed(answer: &Answer, name: &str) -> Option<Value> {
    answer.body["types"]
        .as_array()
        .and_then(|types| {
            types
                .iter()
                .find(|listed| listed["name"].as_str() == Some(name))
        })
        .cloned()
}

/// The definition a reported `Type` carries.
pub(crate) fn definition_of(reported: &Value) -> &str {
    reported["definition"].as_str().unwrap_or_default()
}

/// Create the schema `document` on `api_id` and wait until the creation has been
/// performed.
///
/// The wait is the crate's shared poll loop on the CONDITION — the creation's own reported
/// status — so nothing here depends on how fast the host runs.
pub(crate) async fn create_schema(router: &MultiServiceRouter, api_id: &str, document: &str) {
    let started = start_creation(router, api_id, document.as_bytes()).await;
    assert_eq!(
        started.status, 200,
        "case start the schema creation: expected 200, got {started:?}"
    );
    await_creation(router, api_id, "the schema under test").await;
}

#[tokio::test]
async fn a_read_answers_the_services_own_rendering_of_every_kind_of_type() {
    let router = router();
    let api_id = create_api(&router, "type-sdl-rendering").await;

    for row in rows() {
        // The create answers the definition the request stated, byte for byte.
        let created = create_type(&router, &api_id, row.stated, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected CreateType 200, got {created:?}",
            row.label
        );
        assert_eq!(
            definition_of(&created.body["type"]),
            row.stated,
            "case {}: expected the create to answer the stated definition {:?}, got {:?}",
            row.label,
            row.stated,
            definition_of(&created.body["type"])
        );

        // Every read answers the rendering of the stored type instead.
        let read = get_type(&router, &api_id, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected GetType 200, got {read:?}",
            row.label
        );
        assert_eq!(
            definition_of(&read.body["type"]),
            row.rendered,
            "case {}: expected GetType to answer {:?}, got {:?}",
            row.label,
            row.rendered,
            definition_of(&read.body["type"])
        );
    }

    // The listing reports the same document the single read does, for every type held.
    let listing = list_types(&router, &api_id, state::SDL_FORMAT).await;
    assert_eq!(
        listing.status, 200,
        "case the listing: expected ListTypes 200, got {listing:?}"
    );
    for row in rows() {
        let reported = listed(&listing, row.name).unwrap_or_else(|| {
            panic!(
                "case {}: expected {:?} in the listing, got {listing:?}",
                row.label, row.name
            )
        });
        assert_eq!(
            definition_of(&reported),
            row.rendered,
            "case {}: expected ListTypes to answer {:?}, got {:?}",
            row.label,
            row.rendered,
            definition_of(&reported)
        );
    }
}

#[tokio::test]
async fn an_update_states_a_definition_the_later_reads_render() {
    let router = router();
    let api_id = create_api(&router, "type-sdl-rendering-update").await;

    for row in rows() {
        let case = format!("{} replaced by an update", row.label);
        // A type of another kind is created first, so what the reads answer afterwards
        // can only come from the update's definition.
        let seeded = format!("scalar {}\n", row.name);
        let created = create_type(&router, &api_id, &seeded, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateType 200, got {created:?}"
        );

        let updated = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{}", row.name),
            Some(json!({"definition": row.stated, "format": state::SDL_FORMAT})),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateType 200, got {updated:?}"
        );
        assert_eq!(
            definition_of(&updated.body["type"]),
            row.stated,
            "case {case}: expected the update to answer the stated definition {:?}, got {:?}",
            row.stated,
            definition_of(&updated.body["type"])
        );

        let read = get_type(&router, &api_id, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            definition_of(&read.body["type"]),
            row.rendered,
            "case {case}: expected the read to answer {:?}, got {:?}",
            row.rendered,
            definition_of(&read.body["type"])
        );
    }
}

#[tokio::test]
async fn a_type_declared_by_a_schema_document_is_rendered_the_same_way() {
    let router = router();
    let api_id = create_api(&router, "type-sdl-rendering-schema").await;

    // One document declaring the rows whose definitions resolve against each other, in a
    // layout of the document's own: a type that arrived inside a schema is reported with
    // the same document as one created on its own.
    let declared: Vec<Row> = rows()
        .into_iter()
        .filter(|row| matches!(row.name, "Query" | "Node" | "PostOrder" | "Money"))
        .collect();
    let document: String = declared
        .iter()
        .map(|row| row.stated.to_string())
        .collect::<Vec<String>>()
        .join("\n");
    create_schema(&router, &api_id, &document).await;

    let listing = list_types(&router, &api_id, state::SDL_FORMAT).await;
    assert_eq!(
        listing.status, 200,
        "case the listing: expected ListTypes 200, got {listing:?}"
    );
    for row in &declared {
        let case = format!("{} declared by a schema document", row.label);
        let reported = listed(&listing, row.name)
            .unwrap_or_else(|| panic!("case {case}: expected {:?} listed, got {listing:?}", row.name));
        assert_eq!(
            definition_of(&reported),
            row.rendered,
            "case {case}: expected ListTypes to answer {:?}, got {:?}",
            row.rendered,
            definition_of(&reported)
        );

        let read = get_type(&router, &api_id, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            definition_of(&read.body["type"]),
            row.rendered,
            "case {case}: expected GetType to answer {:?}, got {:?}",
            row.rendered,
            definition_of(&read.body["type"])
        );
    }
}

#[tokio::test]
async fn the_rendering_a_read_answers_declares_the_same_type_again() {
    let router = router();
    let first = create_api(&router, "type-sdl-rendering-first").await;
    let second = create_api(&router, "type-sdl-rendering-round-trip").await;

    for row in rows() {
        let case = format!("{} round-tripped", row.label);
        let created = create_type(&router, &first, row.stated, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateType 200, got {created:?}"
        );
        let rendered = definition_of(
            &get_type(&router, &first, row.name, state::SDL_FORMAT)
                .await
                .body["type"],
        )
        .to_string();

        // The document a read answers is itself a definition of the same type: creating
        // from it under another API declares the same name and renders the same document,
        // so the rendering is stable however often a client round-trips it.
        let again = create_type(&router, &second, &rendered, state::SDL_FORMAT).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected the rendered document to be a definition, got {again:?}"
        );
        assert_eq!(
            again.body["type"]["name"].as_str(),
            Some(row.name),
            "case {case}: expected it to declare {:?}, got {again:?}",
            row.name
        );
        let read = get_type(&router, &second, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            definition_of(&read.body["type"]),
            rendered,
            "case {case}: expected the rendering to be stable, got {:?}",
            definition_of(&read.body["type"])
        );
    }
}
