// SPDX-License-Identifier: CC0-1.0

//! How many types a successful schema creation DETAILS, end to end over HTTP.
//!
//! RULE: the `details` of a creation that reached `SUCCESS` states how many DEFINITIONS the
//! document it compiled states — every type definition, and the definitions that declare no
//! type at all beside them (a `schema` definition binding the root operation types) — in
//! AppSync's own sentence "Successfully created schema with `<N>` types.", whose only variable
//! part is that number. So a document of nothing but type definitions is detailed as the
//! number of types the API goes on to report, and one that also binds its root is detailed as
//! one more than that, which is what pins the count to the submitted document rather than to
//! the test's arithmetic.
//!
//! The rows differ along what the rule generalises over: how many types the document
//! declares (one, a handful, and a generated document far past what is written out by
//! hand), which KINDS it declares them with, and whether it also carries definitions that
//! declare no type at all (a `schema` definition). Every row cross-checks the number in
//! the sentence against the types `ListTypes` pages over for the same API.
//!
//! The harness (`router`, `call`, `create_api`, `start_creation`, `creation_status` and
//! the poll that waits for the creation the clock owes) is the one the schema suites
//! already carry, so every assertion goes through the router the production emulator
//! mounts.

use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, router};
use crate::schema_creation::{await_creation, creation_status, start_creation};

/// The page size a type listing is read in: `ListTypes$maxResults`' published maximum
/// (`smithy_query(query="operation", operation="ListTypes")`: range 0-25).
const TYPE_PAGE: usize = 25;

/// How many object types the generated row declares beside its `Query` type: enough to
/// page the listing several times over, and small enough to stay fast.
const GENERATED_TYPES: usize = 40;

/// One row: a document, how many types it declares, and how many definitions it states.
#[derive(Debug)]
struct Row {
    label: &'static str,
    document: String,
    /// The types the document declares, which the API holds afterwards.
    declared: usize,
    /// The definitions the document states, which the detail counts: the types above plus
    /// every definition that declares no type.
    definitions: usize,
}

/// The documents the rule is asserted over.
fn rows() -> Vec<Row> {
    let mut generated = String::from("schema { query: Query }\ntype Query {\n");
    for index in 0..GENERATED_TYPES {
        generated.push_str(&format!("  t{index}: T{index}\n"));
    }
    generated.push_str("}\n");
    for index in 0..GENERATED_TYPES {
        generated.push_str(&format!("type T{index} {{\n  id: ID\n}}\n"));
    }

    vec![
        Row {
            label: "a document declaring one type and nothing else",
            document: "type Query {\n  hello: String\n}\n".to_string(),
            declared: 1,
            definitions: 1,
        },
        Row {
            label: "a schema definition, which declares no type, and two that do",
            document: "schema { query: Query }\ntype Query {\n  post: Post\n}\ntype Post {\n  \
                       id: ID!\n}\n"
                .to_string(),
            declared: 2,
            definitions: 3,
        },
        Row {
            label: "four types declared with four different keywords",
            document: "type Query {\n  mood: Mood\n  named: Named\n}\nenum Mood {\n  GOOD\n  \
                       BAD\n}\ninterface Named {\n  name: String\n}\ninput Filter {\n  name: \
                       String\n}\n"
                .to_string(),
            declared: 4,
            definitions: 4,
        },
        Row {
            // The shape of the recorded document: N object types, a Query type selecting
            // all of them, and a `schema` definition binding the root.
            label: "a generated document of many object types and a Query over them",
            document: generated,
            declared: GENERATED_TYPES + 1,
            definitions: GENERATED_TYPES + 2,
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

#[tokio::test]
async fn the_success_detail_counts_the_types_the_created_schema_holds() {
    let router = router();
    let rows = rows();
    let mut apis: Vec<String> = Vec::new();

    // Every row's creation is scheduled first, each on its own API, so the ONE wait below
    // is crossed by all of them: the transition is one the clock owes.
    for row in &rows {
        let api_id = create_api(&router, &format!("Api_count_{}", row.definitions)).await;
        let started = start_creation(&router, &api_id, row.document.as_bytes()).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected StartSchemaCreation 200, got {started:?}",
            row.label
        );
        apis.push(api_id);
    }

    for (row, api_id) in rows.iter().zip(&apis) {
        await_creation(&router, api_id, row.label).await;
        let settled = creation_status(&router, api_id).await;

        // The sentence, written out here rather than read off the service's own template,
        // so a change of wording is caught rather than followed.
        let expected = format!(
            "Successfully created schema with {} types.",
            row.definitions
        );
        assert_eq!(
            (
                settled.body["status"].as_str(),
                settled.body["details"].as_str()
            ),
            (Some(state::SCHEMA_SUCCESS), Some(expected.as_str())),
            "case {:?}: expected {:?} detailed as {expected:?}, got {:?}",
            row.label,
            state::SCHEMA_SUCCESS,
            settled.body
        );

        // The number follows the document that was compiled, and the type system that
        // document installed is what the API reports for itself: the types it declares are
        // the API's types, and the definitions that declare none are counted by the detail
        // without ever becoming one.
        let listed = listed_types(&router, api_id).await;
        assert_eq!(
            listed.len(),
            row.declared,
            "case {:?}: expected the API to hold the {} types the document declared, got \
             {listed:?}",
            row.label,
            row.declared
        );
        let counted = counted_types(&settled.body["details"], row.label);
        assert_eq!(
            counted,
            row.definitions,
            "case {:?}: expected the detail to count the {} definitions the document stated \
             (the API holds {listed:?}), got {counted}",
            row.label,
            row.definitions
        );
    }
}

#[tokio::test]
async fn a_second_creation_recounts_the_new_document() {
    let router = router();
    let api_id = create_api(&router, "Api_count_recomputed").await;

    // A schema document is the whole type system, so the detail of the creation that ran
    // last counts the definitions THAT document states — a different number each time,
    // never an accumulation of the ones before it.
    for (label, document, definitions) in [
        (
            "the first document",
            "type Query {\n  book: Book\n}\ntype Book {\n  id: ID!\n}\n",
            2,
        ),
        ("the second document", "type Query {\n  hello: String\n}\n", 1),
        (
            "the third document",
            "schema { query: Query }\ntype Query {\n  a: A\n  b: B\n}\ntype A {\n  id: ID\n}\n\
             type B {\n  id: ID\n}\n",
            4,
        ),
    ] {
        let started = start_creation(&router, &api_id, document.as_bytes()).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: expected StartSchemaCreation 200, got {started:?}"
        );
        await_creation(&router, &api_id, label).await;

        let settled = creation_status(&router, &api_id).await;
        let expected = format!("Successfully created schema with {definitions} types.");
        assert_eq!(
            settled.body["details"].as_str(),
            Some(expected.as_str()),
            "case {label:?}: expected the detail {expected:?}, got {:?}",
            settled.body
        );
    }
}

/// The number the detail states, read back out of the sentence.
fn counted_types(details: &serde_json::Value, case: &str) -> usize {
    let text = details.as_str().unwrap_or_default();
    text.split_whitespace()
        .find_map(|word| word.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a type count in the detail, got {text:?}"))
}
