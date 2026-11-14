// SPDX-License-Identifier: CC0-1.0

//! The SDL document `ListTypes` reports for a type, over every page cut a request can make.
//!
//! The rule asserted here is the one `type_sdl_rendering.rs` states and whose table this
//! suite reuses: a read reports `Type$definition` as the service's own rendering of the
//! type system the API holds — the declaring keyword and the type's name on a line of
//! their own, a blank line, the body's opening brace on its own line, each member written
//! unindented and followed by a blank line, consecutive members separated by one further
//! blank line, the closing brace on its own line, and the whole document terminated by a
//! newline — whatever text the definition arrived as and whichever way the type got into
//! the API.
//!
//! What this suite adds to that one is the dimension a LISTING has and a single read does
//! not: the page. One API is filled both ways a type arrives — declared by a
//! `StartSchemaCreation` document, and created on its own by `CreateType` — and the whole
//! listing is walked at every page size `maxResults` can cut it into, from one type per
//! page up to past the number of types held and the unspecified one, because the document
//! a type is reported with cannot depend on which page reports it.
//!
//! The table, the router and the type calls are the crate's shared harness, reused rather
//! than copied.

use serde_json::Value;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::graphql_api_lifecycle::router;
use super::graphql_types::{call, create_api, create_type, get_type, Answer};
use super::type_sdl_rendering::{create_schema, definition_of, rows, Row};

/// The types this suite declares through a schema document rather than through
/// `CreateType`: the rows of the shared table whose definitions resolve against each other,
/// so one document declares them all. Every other row is created on its own.
const SCHEMA_DECLARED: &[&str] = &["Query", "Node", "PostOrder", "Money"];

/// Whether `row` is one of the types a schema document declares.
fn declared_by_schema(row: &Row) -> bool {
    SCHEMA_DECLARED.contains(&row.name)
}

/// Send one `ListTypes` for `api_id` in SDL, with the paging the case selects.
async fn list_page(
    router: &MultiServiceRouter,
    api_id: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
) -> Answer {
    let mut query = format!("format={}", state::SDL_FORMAT);
    if let Some(max_results) = max_results {
        query.push_str(&format!("&maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push_str(&format!("&nextToken={next_token}"));
    }
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types?{query}"),
        None,
    )
    .await
}

/// Walk the whole listing from its first page with `max_results` per page and return every
/// type reported.
async fn walk(
    router: &MultiServiceRouter,
    api_id: &str,
    max_results: Option<usize>,
    held: usize,
    case: &str,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    // One page per type held, plus the page that closes the walk: a cursor needing more
    // than that is not resuming.
    for page in 0..=held + 1 {
        let answer = list_page(router, api_id, max_results, next_token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case} page {page}: expected ListTypes 200, got {answer:?}"
        );
        let items = answer.body["types"].as_array().cloned().unwrap_or_default();
        if let Some(size) = max_results {
            assert!(
                items.len() <= size,
                "case {case} page {page}: expected at most {size} types, got {}",
                items.len()
            );
        }
        collected.extend(items);
        match answer.body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return collected,
        }
    }
    panic!("case {case}: the cursor never closed the walk after {collected:?}");
}

/// The type named `name` in a collected listing.
fn reported<'a>(collected: &'a [Value], name: &str, case: &str) -> &'a Value {
    collected
        .iter()
        .find(|listed| listed["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case {case}: expected {name:?} in the listing, got {collected:?}"))
}

/// Fill one API with the shared table's types, both ways a type arrives, and return its id.
///
/// The schema creation runs FIRST, because a creation installs the type system its document
/// declares; the `CreateType` rows are added to it afterwards, so one listing reports types
/// of both provenances.
async fn fill(router: &MultiServiceRouter, name: &str, held: &[Row]) -> String {
    let api_id = create_api(router, name).await;

    let document: String = held
        .iter()
        .filter(|row| declared_by_schema(row))
        .map(|row| format!("{}\n", row.stated))
        .collect();
    create_schema(router, &api_id, &document).await;

    for row in held.iter().filter(|row| !declared_by_schema(row)) {
        let created = create_type(router, &api_id, row.stated, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected CreateType 200, got {created:?}",
            row.label
        );
    }
    api_id
}

/// A listing reports every type with the service's own rendering, whatever page cut the
/// request makes and however the type arrived.
#[tokio::test]
async fn a_listing_reports_the_services_own_rendering_on_every_page_cut() {
    let router = router();
    let held = rows();
    let api_id = fill(&router, "list-types-sdl-layout", &held).await;

    // Every page size from one type per page up to past the number of types held, and the
    // unspecified one (`maxResults` defaults to 0, i.e. "unspecified").
    let sizes: Vec<Option<usize>> = (1..=held.len() + 1)
        .map(Some)
        .chain(std::iter::once(None))
        .collect();
    for size in sizes {
        let case = format!("page size {size:?}");
        let collected = walk(&router, &api_id, size, held.len(), &case).await;
        assert_eq!(
            collected.len(),
            held.len(),
            "case {case}: expected {} types, got {collected:?}",
            held.len()
        );
        for row in &held {
            let listed = reported(&collected, row.name, &case);
            let definition = definition_of(listed);
            assert_eq!(
                definition, row.rendered,
                "case {} ({case}): expected the listing to report {:?}, got {definition:?}",
                row.label, row.rendered
            );
            assert_eq!(
                listed["format"].as_str(),
                Some(state::SDL_FORMAT),
                "case {} ({case}): expected the SDL format reported back, got {listed:?}",
                row.label
            );
        }
    }
}

/// The reported document is the service's rendering and not the text the request wrote:
/// every document is newline-terminated, a body is written over its own lines, and a
/// definition whose text differs from the rendering is not echoed.
#[tokio::test]
async fn the_reported_document_is_rendered_rather_than_echoed() {
    let router = router();
    let held = rows();
    let api_id = fill(&router, "list-types-sdl-not-echoed", &held).await;
    let collected = walk(&router, &api_id, None, held.len(), "the whole listing").await;

    for row in &held {
        let listed = reported(&collected, row.name, row.label);
        let definition = definition_of(listed);
        assert!(
            definition.ends_with('\n'),
            "case {}: expected the document to be newline-terminated, got {definition:?}",
            row.label
        );
        if row.rendered.contains('{') {
            // A body opens on its own line after a blank one, so the document a client
            // line-parses is multi-line whatever the request wrote.
            assert!(
                definition.contains("\n\n{\n") && definition.ends_with("\n\n}\n"),
                "case {}: expected the body written over its own lines, got {definition:?}",
                row.label
            );
        }
        if row.stated != row.rendered {
            assert_ne!(
                definition, row.stated,
                "case {}: expected the stated text NOT to be echoed, got {definition:?}",
                row.label
            );
        }
    }
}

/// A page of a listing and a single read report one type with one document, so a client
/// cannot see two renderings of the same type.
#[tokio::test]
async fn a_listing_page_and_a_single_read_report_one_document() {
    let router = router();
    let held = rows();
    let api_id = fill(&router, "list-types-sdl-agreement", &held).await;
    let collected = walk(&router, &api_id, Some(2), held.len(), "the two-per-page walk").await;

    for row in &held {
        let read = get_type(&router, &api_id, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected GetType 200, got {read:?}",
            row.label
        );
        assert_eq!(
            &read.body["type"],
            reported(&collected, row.name, row.label),
            "case {}: expected the listing to report what GetType reports, got {:?}",
            row.label,
            read.body["type"]
        );
    }
}
