// SPDX-License-Identifier: CC0-1.0

//! WHICH documentation part a `merge` import of a documentation set lands on, asserted on
//! the wire.
//!
//! The RULE under test: a documentation part is identified by the REST API it belongs to plus
//! the COMPLETED location of the entity it documents, so an import entry for an entity the API
//! ALREADY documents lands on the part that documents it — reporting THAT part's identifier
//! and replacing its content map — rather than adding a second part for one entity. The
//! sibling `documentation_parts` suite asserts that rule for a part a `CreateDocumentationPart`
//! made; this suite asserts it for a part an EARLIER IMPORT made, which is the shape of the
//! normal "re-import the documentation whenever it is regenerated" flow, and `merge` is the
//! documented default mode.
//!
//! What the rows span: how the second body documents the entity (the same entry again, the
//! same entity with a new content map), how many entities the bodies document, the order the
//! second body documents them in, the entity type and so how many members its location sets,
//! whether the second body spells the documented defaults out or leaves them to be defaulted,
//! and the mode the re-import runs in (absent, so merge by default, and `merge` spelled out).
//!
//! Every case drives the real wire path — a `restJson1` request through the multi-service
//! router into the mounted service — through the crate's one wire-test harness ([`Gateway`]),
//! so what is asserted is what a client sees.

use serde_json::{json, Value};

use crate::documentation_parts::{import_body, import_parts, listed_entities};
use crate::rest_api_lifecycle::{create_api, Gateway};

/// The `ids` an accepted import reports, asserting the modelled 200.
async fn imported_ids(
    gateway: &Gateway,
    api: &str,
    mode: Option<&str>,
    body: &str,
    case: &str,
) -> Vec<String> {
    let answer = import_parts(gateway, api, mode, false, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ids"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: the import reports ids, got {:?}", answer.body))
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case}: an id is a string, got {id:?}"))
                .to_string()
        })
        .collect()
}

/// One row: the documentation set a first import writes, and the one a `merge` re-import
/// writes over it.
struct Row {
    /// What makes this row its own case.
    case: &'static str,
    /// The entries of the body the first import carries, as `(location, content map)` pairs.
    first: Vec<(Value, Value)>,
    /// The entries of the body the re-import carries.
    again: Vec<(Value, Value)>,
    /// Which of the first import's entities the re-import documents, by index into `first`:
    /// the identifiers the re-import must report, in the order its own body documents them.
    lands_on: Vec<usize>,
}

#[tokio::test]
async fn a_merge_import_lands_on_the_part_an_earlier_import_made_for_that_entity() {
    let rows: Vec<Row> = vec![
        Row {
            case: "the same single entry imported twice",
            first: vec![(
                json!({ "type": "RESOURCE", "path": "/pets" }),
                json!({ "description": "the pets" }),
            )],
            again: vec![(
                json!({ "type": "RESOURCE", "path": "/pets" }),
                json!({ "description": "the pets" }),
            )],
            lands_on: vec![0],
        },
        Row {
            case: "the same entity re-documented with a new content map",
            first: vec![(
                json!({ "type": "API" }),
                json!({ "info": { "description": "the store" } }),
            )],
            again: vec![(
                json!({ "type": "API" }),
                json!({ "info": { "description": "the store, documented again" } }),
            )],
            lands_on: vec![0],
        },
        Row {
            case: "one entity of several re-documented",
            first: vec![
                (
                    json!({ "type": "API" }),
                    json!({ "info": { "description": "the store" } }),
                ),
                (
                    json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
                    json!({ "description": "list pets" }),
                ),
                (
                    json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "200" }),
                    json!({ "description": "the pets" }),
                ),
            ],
            again: vec![(
                json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
                json!({ "description": "list every pet" }),
            )],
            lands_on: vec![1],
        },
        Row {
            case: "every entity re-documented in another order",
            first: vec![
                (
                    json!({ "type": "MODEL", "name": "Pet" }),
                    json!({ "description": "a pet" }),
                ),
                (
                    json!({ "type": "QUERY_PARAMETER", "path": "/pets", "method": "GET", "name": "page" }),
                    json!({ "description": "the page" }),
                ),
            ],
            again: vec![
                (
                    json!({ "type": "QUERY_PARAMETER", "path": "/pets", "method": "GET", "name": "page" }),
                    json!({ "description": "the page to read" }),
                ),
                (
                    json!({ "type": "MODEL", "name": "Pet" }),
                    json!({ "description": "a pet, documented again" }),
                ),
            ],
            lands_on: vec![1, 0],
        },
        Row {
            case: "the entity re-documented with its defaulted members spelled out",
            first: vec![(
                json!({ "type": "METHOD", "path": "/pets" }),
                json!({ "description": "every verb of the resource" }),
            )],
            again: vec![(
                json!({ "type": "METHOD", "path": "/pets", "method": "*" }),
                json!({ "description": "every verb, documented again" }),
            )],
            lands_on: vec![0],
        },
    ];

    for row in &rows {
        for mode in [None, Some("merge")] {
            let case = format!("{} with mode {mode:?}", row.case);
            let gateway = Gateway::new();
            let (api, _root) = create_api(&gateway, "documentation-import-identity").await;

            let created = imported_ids(&gateway, &api, None, &import_body(&row.first), &case).await;
            assert_eq!(
                created.len(),
                row.first.len(),
                "case {case}: expected one id per entity the first body documents, got \
                 {created:?}"
            );

            let landed = imported_ids(&gateway, &api, mode, &import_body(&row.again), &case).await;
            let expected: Vec<String> = row
                .lands_on
                .iter()
                .map(|at| created[*at].clone())
                .collect();
            assert_eq!(
                landed, expected,
                "case {case}: expected the re-import to report the parts that already \
                 document the entities ({expected:?}), got {landed:?}"
            );

            // One part per entity: `merge` keeps what the second body does not document and
            // re-documents what it does, so the API documents exactly the entities the two
            // bodies named between them — each once, carrying the later content map.
            let stored = listed_entities(&gateway, &api, &case).await;
            assert_eq!(
                stored.len(),
                row.first.len(),
                "case {case}: expected the API to document {} entities, got {stored:?}",
                row.first.len()
            );
            for (location, documented) in &row.again {
                assert!(
                    stored
                        .iter()
                        .any(|(stored_location, stored_properties)| stored_location == location
                            && stored_properties == documented),
                    "case {case}: expected the part at {location:?} to carry the re-imported \
                     content map {documented:?}, got {stored:?}"
                );
            }
        }
    }
}
