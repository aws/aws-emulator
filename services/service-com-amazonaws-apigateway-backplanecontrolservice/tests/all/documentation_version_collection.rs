// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetDocumentationVersions`: the collection of an API's
//! documentation-version snapshots.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], [`Answer`], `create_api`,
//! `string_member`, `page_items` and `assert_failure` are this crate's one wire-test
//! harness, reused from `rest_api_lifecycle`, and the create/read helpers come from the
//! `documentation_versions` suite.
//!
//! The rule under test: a collection reader reports every stored record of the named
//! parent in creation order, oldest first, one `position`/`limit` page at a time, each
//! entry being the record the single-record read reports; it changes nothing. So the
//! cases assert over SETS of inputs — a table of version identifiers whose creation order
//! is NOT their identifier order (the records are keyed by identifier, so key order would
//! answer a different sequence), collections from none at all up to past the default page
//! size, the whole documented page-size range with one value past each bound, cursors that
//! name a record and cursors that name nothing, and APIs that own the collection and APIs
//! that do not.

use serde_json::Value;

use crate::documentation_versions::{create_version, get_version, snapshot_request};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// The modelled error codes these cases assert.
const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// The rows of the collection table: a case name, the version identifier the request
/// carries and its description (`None` for a request that sets none).
///
/// The identifiers are deliberately created in an order that is NOT their sorted order —
/// `1.0.0-beta.1` sorts first and `v3` last — because the rule is about creation order,
/// and a reader that answered the store's key order would pass on any table whose rows
/// happen to be created in sorted order.
const SNAPSHOTS: &[(&str, &str, Option<&str>)] = &[
    ("a bare identifier, no description", "v3", None),
    ("a two-digit identifier", "v10", Some("the tenth cut")),
    ("a lower-numbered identifier created later", "v2", None),
    ("a dated identifier", "2026-01-01", Some("the January cut")),
    ("a single lowercase character", "a", None),
    ("a single uppercase character", "B", Some("the B cut")),
    ("a dotted prerelease identifier", "1.0.0-beta.1", None),
];

/// `GetDocumentationVersions` for `api`, with `query` appended verbatim (`""` for no
/// paging).
async fn get_versions(gateway: &Gateway, api: &str, query: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/versions{query}"),
            None,
        )
        .await
}

/// The version identifiers of a `GetDocumentationVersions` answer, in the order the
/// answer lists them — the order is itself part of the rule, so it is NOT sorted away
/// here.
fn reported_versions(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "version", case))
        .collect()
}

/// Create one snapshot of `api` and return the body the create answered.
async fn created_snapshot(
    gateway: &Gateway,
    api: &str,
    version: &str,
    description: Option<&str>,
    case: &str,
) -> Value {
    let created = create_version(gateway, api, snapshot_request(version, description)).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected 201 from CreateDocumentationVersion, got {} with {:?}",
        created.status, created.body
    );
    created.body
}

#[tokio::test]
async fn get_documentation_versions_reports_every_snapshot_as_the_single_read_reports_it() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-version-collection").await;

    // An API with no snapshots has the collection, and it is empty — not a refusal and
    // not an absent member.
    let empty = get_versions(&gateway, &api, "").await;
    assert_eq!(
        empty.status, 200,
        "case no snapshots: expected 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        reported_versions(&empty.body, "no snapshots"),
        Vec::<String>::new(),
        "case no snapshots: expected an empty collection, got {:?}",
        empty.body
    );
    assert!(
        empty.body["position"].is_null(),
        "case no snapshots: the last page omits position, got {:?}",
        empty.body["position"]
    );

    let mut expected: Vec<String> = Vec::new();
    for (case, version, description) in SNAPSHOTS {
        let created = created_snapshot(&gateway, &api, version, *description, case).await;
        expected.push((*version).to_string());

        let listed = get_versions(&gateway, &api, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from GetDocumentationVersions, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_versions(&listed.body, case),
            expected,
            "case {case}: expected every snapshot in creation order {expected:?}, got {:?}",
            listed.body
        );

        // Each entry is the stored record: the very body the single-snapshot read answers
        // for it, member for member.
        let entry = page_items(&listed.body, case)
            .into_iter()
            .find(|item| item["version"].as_str() == Some(*version))
            .unwrap_or_else(|| panic!("case {case}: the created snapshot must be listed"));
        let read = get_version(&gateway, &api, version).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetDocumentationVersion, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            entry, read.body,
            "case {case}: a listed snapshot must be the record GetDocumentationVersion \
             reports, got {entry:?} against {:?}",
            read.body
        );
        assert_eq!(
            entry, created,
            "case {case}: a listed snapshot must be the record the create stored, got \
             {entry:?} against {created:?}"
        );
        // The description the request asked for, reported or absent exactly as asked.
        match description {
            Some(text) => assert_eq!(
                entry["description"].as_str(),
                Some(*text),
                "case {case}: expected the created description, got {:?}",
                entry["description"]
            ),
            None => assert!(
                entry["description"].is_null(),
                "case {case}: a snapshot created with no description reports none, got {:?}",
                entry["description"]
            ),
        }
        assert!(
            entry["createdDate"].is_number(),
            "case {case}: expected a createdDate epoch timestamp, got {:?}",
            entry["createdDate"]
        );
    }
}

#[tokio::test]
async fn get_documentation_versions_pages_through_creation_order_on_position_and_limit() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-version-paging").await;
    let mut created: Vec<String> = Vec::new();
    for (case, version, description) in SNAPSHOTS {
        created_snapshot(&gateway, &api, version, *description, case).await;
        created.push((*version).to_string());
    }

    // With no `limit` the default page size covers all seven, so the one page is the whole
    // collection and carries no next cursor.
    let all = get_versions(&gateway, &api, "").await;
    assert_eq!(
        reported_versions(&all.body, "no limit"),
        created,
        "case no limit: expected creation order {created:?}, got {:?}",
        all.body
    );
    assert_eq!(
        all.body["position"].as_str(),
        None,
        "case no limit: a page that holds the whole collection reports no cursor, got {:?}",
        all.body["position"]
    );

    // Rows span the documented page-size range — `GetDocumentationVersionsRequest$limit`:
    // "The maximum number of returned results per page. The default value is 25 and the
    // maximum value is 500" — its bounds, and one value past each bound, which is clamped
    // into the range rather than refused.
    for limit in [0, 1, 2, 3, 7, 8, 25, 500, 501, 10_000] {
        let case = format!("limit={limit}");
        let effective = limit.clamp(1, 500) as usize;
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &position {
                None => format!("?limit={limit}"),
                Some(cursor) => format!("?limit={limit}&position={cursor}"),
            };
            let answer = get_versions(&gateway, &api, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let items = page_items(&answer.body, &case);
            assert!(
                items.len() <= effective,
                "case {case}: a page holds at most {effective} items, got {}",
                items.len()
            );
            seen.extend(items.iter().map(|item| string_member(item, "version", &case)));
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            match answer.body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            seen, created,
            "case {case}: paging must reassemble creation order {created:?}, got {seen:?}"
        );
    }

    // A cursor is the identifier of the first snapshot the previous page did NOT hand
    // out, so a page requested at one lists creation order FROM that snapshot on.
    for (index, cursor) in created.iter().enumerate() {
        let case = format!("position={cursor}");
        let answer = get_versions(&gateway, &api, &format!("?position={cursor}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_versions(&answer.body, &case),
            created[index..].to_vec(),
            "case {case}: expected creation order from the cursor on, got {:?}",
            answer.body
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page.
    for cursor in ["nosuchversion", "v4", "ZZZZZZ"] {
        assert_failure(
            &get_versions(&gateway, &api, &format!("?position={cursor}")).await,
            400,
            BAD_REQUEST,
            &format!("position={cursor:?}"),
        );
    }
}

#[tokio::test]
async fn a_collection_past_the_default_page_size_is_cut_at_that_default() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-version-default-page").await;
    // Past the documented default of 25, so the default itself is observable: identifiers
    // are zero-padded so their sorted order IS their creation order here, which is what
    // makes the page boundary below a statement about the count rather than the ordering.
    let created: Vec<String> = (0..27).map(|n| format!("v{n:03}")).collect();
    for version in &created {
        created_snapshot(&gateway, &api, version, None, version).await;
    }

    let first = get_versions(&gateway, &api, "").await;
    assert_eq!(
        first.status, 200,
        "case default page: expected 200, got {} with {:?}",
        first.status, first.body
    );
    assert_eq!(
        reported_versions(&first.body, "default page"),
        created[..25].to_vec(),
        "case default page: expected the first 25 snapshots, got {:?}",
        first.body
    );
    assert_eq!(
        first.body["position"].as_str(),
        Some(created[25].as_str()),
        "case default page: expected the 26th identifier — the one the page did not report \
         — as the cursor of the page, got {:?}",
        first.body["position"]
    );

    let second = get_versions(
        &gateway,
        &api,
        &format!("?position={}", first.body["position"].as_str().unwrap()),
    )
    .await;
    assert_eq!(
        reported_versions(&second.body, "default page, second"),
        created[25..].to_vec(),
        "case default page, second: expected the remaining snapshots, got {:?}",
        second.body
    );
    assert_eq!(
        second.body["position"].as_str(),
        None,
        "case default page, second: the page that ends the collection reports no cursor, \
         got {:?}",
        second.body["position"]
    );
}

#[tokio::test]
async fn the_collection_belongs_to_the_api_that_owns_it() {
    let gateway = Gateway::new();
    let (first_api, _first_root) = create_api(&gateway, "documentation-version-owner-one").await;
    let (second_api, _second_root) = create_api(&gateway, "documentation-version-owner-two").await;
    created_snapshot(&gateway, &first_api, "v1", None, "the owning API").await;

    // Rows: the API that has the snapshot, a sibling API that has none, and an identifier
    // no API answers to.
    assert_eq!(
        reported_versions(
            &get_versions(&gateway, &first_api, "").await.body,
            "the owning API"
        ),
        vec!["v1".to_string()],
        "case the owning API: expected its one snapshot"
    );
    assert_eq!(
        reported_versions(
            &get_versions(&gateway, &second_api, "").await.body,
            "a sibling API"
        ),
        Vec::<String>::new(),
        "case a sibling API: another API's snapshot must not be listed on it"
    );
    assert_failure(
        &get_versions(&gateway, "zzzzzzzzzz", "").await,
        404,
        NOT_FOUND,
        "an unknown restApiId",
    );

    // A snapshot created on the sibling under the SAME identifier is its own record, and
    // joins only the sibling's collection.
    created_snapshot(&gateway, &second_api, "v1", Some("the sibling"), "the sibling").await;
    assert_eq!(
        reported_versions(
            &get_versions(&gateway, &second_api, "").await.body,
            "the sibling after its own snapshot"
        ),
        vec!["v1".to_string()],
        "case the sibling after its own snapshot: expected only its own record"
    );
    let mine = get_versions(&gateway, &first_api, "").await;
    assert_eq!(
        page_items(&mine.body, "the first API after the sibling created")
            .first()
            .map(|item| item["description"].clone()),
        Some(Value::Null),
        "case the first API after the sibling created: its snapshot is untouched, got {:?}",
        mine.body
    );

    // Deleting the API takes the whole collection with it.
    let dropped = gateway
        .call("DELETE", &format!("/restapis/{first_api}"), None)
        .await;
    assert_eq!(
        dropped.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        dropped.status, dropped.body
    );
    assert_failure(
        &get_versions(&gateway, &first_api, "").await,
        404,
        NOT_FOUND,
        "after DeleteRestApi",
    );
}

#[tokio::test]
async fn listing_the_snapshots_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-version-collection-stable").await;
    created_snapshot(&gateway, &api, "v1", None, "stable 1").await;
    created_snapshot(&gateway, &api, "v2", Some("the second"), "stable 2").await;

    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        // Rows: the whole collection, one page of it, and the page after that cursor — no
        // shape of read may move the collection on.
        for query in ["", "?limit=1", "?position=v2"] {
            let answer = get_versions(&gateway, &api, query).await;
            assert_eq!(
                answer.status, 200,
                "case read {query:?} in round {round}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            if query.is_empty() {
                seen.push(answer.body);
            }
        }
    }
    assert_eq!(
        seen[0], seen[2],
        "case repeated reads: a read must not change the collection, got {:?} then {:?}",
        seen[0], seen[2]
    );
    assert_eq!(
        reported_versions(&seen[2], "after the listings"),
        vec!["v1".to_string(), "v2".to_string()],
        "case after the listings: expected both snapshots in creation order"
    );
    // And it changed nothing a single-snapshot read sees either.
    let read = get_version(&gateway, &api, "v1").await;
    assert_eq!(
        read.status, 200,
        "case single read after the listings: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["version"].as_str(),
        Some("v1"),
        "case single read after the listings: expected the stored record, got {:?}",
        read.body
    );
    // A create after the listings appends to the same sequence rather than reordering it.
    created_snapshot(&gateway, &api, "v0", None, "stable 3").await;
    assert_eq!(
        reported_versions(&get_versions(&gateway, &api, "").await.body, "after a create"),
        vec!["v1".to_string(), "v2".to_string(), "v0".to_string()],
        "case after a create: the newest snapshot is listed last, whatever its identifier"
    );
}
