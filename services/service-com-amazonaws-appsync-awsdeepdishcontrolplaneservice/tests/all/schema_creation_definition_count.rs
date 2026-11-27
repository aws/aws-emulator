// SPDX-License-Identifier: CC0-1.0

//! What the number in a successful schema creation's `details` is counted over, asserted by
//! PAIRS of documents that declare the same types, end to end over HTTP.
//!
//! RULE: the `details` of a creation that reached `SUCCESS` is AppSync's sentence
//! "Successfully created schema with `<N>` types.", and `N` is how many top-level
//! DEFINITIONS the submitted document states — a definition that declares no type (the
//! `schema` definition that binds the root operation types, a directive definition, an
//! extension of a type the document declares) is counted with the type definitions, while the
//! types the API goes on to hold are the type definitions alone. The count therefore comes
//! from the document the caller submitted and from nothing the service contributes to a
//! schema.
//!
//! Each row is a PAIR: two documents declaring exactly the same types, one of them stating
//! `extra` further definitions that declare none. The detail of the second must state exactly
//! `extra` more than the detail of the first, and both APIs must hold the same types — so the
//! rule is asserted as a difference the test does not have to know the absolute count for,
//! and a service that counted a type of its own, or counted the whole type system, would
//! fail the pair rather than one row of it.
//!
//! The rows differ along what the rule generalises over: which kind of type-less definition
//! is added (a `schema` definition, a directive definition, type extensions, several at
//! once), how many are added, and how many types the pair declares.
//!
//! The harness (`router`, `call`, `create_api`, `start_creation`, `creation_status` and the
//! poll that waits for the creation the clock owes) is the one the schema suites already
//! carry, so every assertion goes through the router the production emulator mounts.

use serde_json::Value;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, router};
use crate::schema_creation::{await_creation, creation_status, start_creation};

/// The page size a type listing is read in: `ListTypes$maxResults`' published maximum
/// (`smithy_query(query="operation", operation="ListTypes")`: range 0-25).
const TYPE_PAGE: usize = 25;

/// One row: two documents that declare the same types, the second stating `extra` further
/// definitions that declare none.
#[derive(Debug)]
struct Pair {
    label: &'static str,
    /// The document stating type definitions only.
    types_only: &'static str,
    /// The same type definitions, plus definitions that declare no type.
    with_type_less: &'static str,
    /// How many definitions that declare no type the second document adds.
    extra: usize,
}

/// The pairs the rule is asserted over.
///
/// Which definitions declare a type comes from GraphQL's SDL grammar, the language
/// `StartSchemaCreationRequest$definition` ("The schema definition, in GraphQL schema
/// language format") is written in: the type definitions (object, interface, union, enum,
/// input object, scalar) declare one, while a `schema` definition, a directive definition and
/// a type extension declare none.
fn pairs() -> Vec<Pair> {
    vec![
        Pair {
            label: "a schema definition binding the root",
            types_only: "type Query {\n  hello: String\n}\n",
            with_type_less: "schema { query: Query }\ntype Query {\n  hello: String\n}\n",
            extra: 1,
        },
        Pair {
            label: "a directive definition beside two types",
            types_only: "type Query {\n  post: Post\n}\ntype Post {\n  id: ID!\n}\n",
            with_type_less: "directive @auth(level: Int) on FIELD_DEFINITION\n\
                             type Query {\n  post: Post\n}\ntype Post {\n  id: ID!\n}\n",
            extra: 1,
        },
        Pair {
            label: "two extensions of the types the document declares",
            types_only: "type Query {\n  hello: String\n}\nenum Mood {\n  GOOD\n}\n",
            with_type_less: "type Query {\n  hello: String\n}\nenum Mood {\n  GOOD\n}\n\
                             extend type Query {\n  goodbye: String\n}\n\
                             extend enum Mood {\n  BAD\n}\n",
            extra: 2,
        },
        Pair {
            label: "a schema definition and a directive definition at once",
            types_only: "type Query {\n  mood: Mood\n}\nenum Mood {\n  GOOD\n  BAD\n}\n\
                         input Filter {\n  mood: Mood\n}\n",
            with_type_less: "schema { query: Query }\n\
                             directive @auth(level: Int) on FIELD_DEFINITION\n\
                             type Query {\n  mood: Mood\n}\nenum Mood {\n  GOOD\n  BAD\n}\n\
                             input Filter {\n  mood: Mood\n}\n",
            extra: 2,
        },
    ]
}

/// Every type name `api_id` holds, read through `ListTypes` page by page.
async fn listed_types(router: &MultiServiceRouter, api_id: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = format!("format={}&maxResults={TYPE_PAGE}", state::SDL_FORMAT);
        if let Some(token) = token.as_deref() {
            query.push_str(&format!("&nextToken={token}"));
        }
        let answer = call(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/types?{query}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case list the types of {api_id}: expected ListTypes 200, got {answer:?}"
        );
        for listed in answer.body["types"].as_array().into_iter().flatten() {
            names.push(listed["name"].as_str().unwrap_or_default().to_string());
        }
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
}

/// Create an API, schedule the creation of `document` on it and answer its id.
async fn scheduled(router: &MultiServiceRouter, name: &str, case: &str, document: &str) -> String {
    let api_id = create_api(router, name).await;
    let started = start_creation(router, &api_id, document.as_bytes()).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected StartSchemaCreation 200, got {started:?}"
    );
    api_id
}

/// The number the `details` of a SETTLED creation of `api_id` states, with the status and
/// the whole sentence asserted on the way: the fixed text is AppSync's, written out here
/// rather than read off the service's own template.
async fn detailed_count(router: &MultiServiceRouter, api_id: &str, case: &str) -> usize {
    await_creation(router, api_id, case).await;
    let settled = creation_status(router, api_id).await;
    assert_eq!(
        settled.body["status"].as_str(),
        Some(state::SCHEMA_SUCCESS),
        "case {case:?}: expected the creation to have succeeded, got {:?}",
        settled.body
    );
    let counted = counted_types(&settled.body["details"], case);
    let expected = format!("Successfully created schema with {counted} types.");
    assert_eq!(
        settled.body["details"].as_str(),
        Some(expected.as_str()),
        "case {case:?}: expected the detail {expected:?}, got {:?}",
        settled.body
    );
    counted
}

/// The number the detail states, read back out of the sentence.
fn counted_types(details: &Value, case: &str) -> usize {
    let text = details.as_str().unwrap_or_default();
    text.split_whitespace()
        .find_map(|word| word.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a type count in the detail, got {text:?}"))
}

/// A definition that declares no type is counted by the detail and is not one of the API's
/// types: the pair's two documents differ in the count by exactly the definitions added, and
/// not at all in the types the API holds.
#[tokio::test]
async fn a_type_less_definition_is_counted_and_is_not_a_type_of_the_api() {
    let router = router();
    let pairs = pairs();

    // Both creations of every pair are scheduled first, so the ONE wait inside the reads
    // below is crossed by all of them: the transition is one the clock owes.
    let mut apis: Vec<(String, String)> = Vec::new();
    for (index, pair) in pairs.iter().enumerate() {
        let plain = scheduled(
            &router,
            &format!("Api_defs_plain_{index}"),
            pair.label,
            pair.types_only,
        )
        .await;
        let extended = scheduled(
            &router,
            &format!("Api_defs_extra_{index}"),
            pair.label,
            pair.with_type_less,
        )
        .await;
        apis.push((plain, extended));
    }

    for (pair, (plain, extended)) in pairs.iter().zip(&apis) {
        let plain_count = detailed_count(&router, plain, pair.label).await;
        let extended_count = detailed_count(&router, extended, pair.label).await;
        assert_eq!(
            extended_count,
            plain_count + pair.extra,
            "case {:?}: expected the document stating {} further definitions that declare no \
             type to be detailed as {} rather than {extended_count}",
            pair.label,
            pair.extra,
            plain_count + pair.extra
        );

        // The types the API holds are the type definitions alone, so the two documents leave
        // the same type system behind however many type-less definitions the second states.
        let plain_types = listed_types(&router, plain).await;
        let extended_types = listed_types(&router, extended).await;
        assert_eq!(
            plain_types, extended_types,
            "case {:?}: expected both documents to leave the same types, got {plain_types:?} \
             and {extended_types:?}",
            pair.label
        );
        assert_eq!(
            plain_count,
            plain_types.len(),
            "case {:?}: expected the document of type definitions only to be detailed as the \
             {} types it declares, got {plain_count}",
            pair.label,
            plain_types.len()
        );
    }
}
