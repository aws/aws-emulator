// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `PortalSummary` that every element of a `ListPortals`
//! page carries.
//!
//! The rule these cases assert, one level above the recorded cases (an account holding one
//! portal, and an account holding two, each listed once):
//!
//! > **A collection reader projects EVERY element through the same record projection its
//! > single-resource reader serves**, so a member the resource always reports is on every
//! > element of every page, wherever the page boundary falls and whatever else the
//! > collection holds. For a portal summary that member is `PortalSummary$Preview`
//! > (`smithy_query(query="shape", shape="PortalSummary")`: member `Preview`, jsonName
//! > `preview`, target `Preview`): the EMPTY structure until something has built a preview,
//! > and the preview's status and URL once one is built — so a client that iterates the
//! > collection and reads the member to decide whether a preview exists is handed a
//! > structure rather than nothing at all.
//!
//! The rows span what the rule generalises over: how many portals the account holds, which
//! of them have had a preview built (none, some, every one), the page size the collection
//! is walked at (one item per page up to one past the total, plus unpaged), and therefore
//! each element's position within its page.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::PREVIEW_STATUS_PREVIEW_READY;

use crate::api_lifecycle::{as_str, call, emulator, read_until};
use crate::portal_lifecycle::portal_content;

/// The portals collection (`ListPortals`'s modelled `GET /v2/portals`).
const COLLECTION: &str = "/v2/portals";

/// The member under test, as `PortalSummary` names it on the wire.
const PREVIEW: &str = "preview";

/// The members of the reported `Preview` a built preview carries, as that shape names them
/// on the wire (`smithy_query(query="shape", shape="Preview")`).
const PREVIEW_STATUS: &str = "previewStatus";
const PREVIEW_URL: &str = "previewUrl";

/// Every member `PortalSummary` declares, as it names them on the wire
/// (`smithy_query(query="shape", shape="PortalSummary")`).
///
/// A summary and the single-portal body are compared over THESE rather than as whole
/// documents, so the comparison stays about the projection the two readers share and does
/// not turn into an assertion about the member set a sibling response shape happens to
/// declare.
const SUMMARY_MEMBERS: &[&str] = &[
    "authorization",
    "endpointConfiguration",
    "includedPortalProductArns",
    "lastModified",
    "lastPublished",
    "lastPublishedDescription",
    "portalArn",
    "portalContent",
    "portalId",
    PREVIEW,
    "publishStatus",
    "rumAppMonitorName",
    "statusException",
    "tags",
];

/// One row of the table: which of the portals the account creates have a preview built on
/// them, one flag per portal in creation order.
struct Row {
    case: &'static str,
    previewed: &'static [bool],
}

/// The accounts the rule is asserted over: the collection sizes and the mixes of built and
/// unbuilt previews across them. The recorded cases — one portal and two portals, none of
/// them previewed — are two of these rows.
const ROWS: &[Row] = &[
    Row {
        case: "one portal, nothing previewed",
        previewed: &[false],
    },
    Row {
        case: "one portal with a preview built",
        previewed: &[true],
    },
    Row {
        case: "two portals, neither previewed",
        previewed: &[false, false],
    },
    Row {
        case: "two portals, only the second previewed",
        previewed: &[false, true],
    },
    Row {
        case: "three portals, the outer two previewed",
        previewed: &[true, false, true],
    },
    Row {
        case: "five portals, every one previewed",
        previewed: &[true, true, true, true, true],
    },
];

/// Create one portal and answer its `PortalId`.
async fn create_portal(emulator: &MultiServiceRouter, case: &str, display_name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        COLLECTION,
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content(display_name)
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
    );
    as_str(&created, "portalId")
}

/// Ask the service to build a preview of `portal_id`: `PreviewPortal` answers its modelled
/// 202 and the build is the service's to finish.
async fn start_preview(emulator: &MultiServiceRouter, case: &str, portal_id: &str) {
    let uri = format!("{COLLECTION}/{portal_id}/preview");
    let (status, answered) = call(emulator, "POST", &uri, None).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from PreviewPortal on {portal_id}, got {status} {answered}"
    );
}

/// Read `portal_id` until the preview the service owes it reports READY.
///
/// The wait is for that CONDITION — the portal is re-read until it reports it — so a loaded
/// host reaches the same outcome as an idle one.
async fn await_preview_ready(emulator: &MultiServiceRouter, case: &str, portal_id: &str) {
    let uri = format!("{COLLECTION}/{portal_id}");
    read_until(emulator, case, &uri, |body| {
        body.get(PREVIEW)
            .and_then(|preview| preview.get(PREVIEW_STATUS))
            .and_then(Value::as_str)
            == Some(PREVIEW_STATUS_PREVIEW_READY)
    })
    .await;
}

/// One page of `ListPortals`, which must answer 200.
async fn list_portals(emulator: &MultiServiceRouter, case: &str, query: &str) -> Value {
    let (status, body) = call(emulator, "GET", &format!("{COLLECTION}{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListPortals, got {status} {body}"
    );
    body
}

/// Walk the collection from its first page to its last at `page_size` (`None` for an
/// unpaged listing) and answer every summary reported, in page order.
async fn walk_items(
    emulator: &MultiServiceRouter,
    case: &str,
    page_size: Option<usize>,
    total: usize,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more turn than pages are possible, so a token that never clears is caught here
    // instead of looping forever.
    for _ in 0..total + 2 {
        let query = match (page_size, &token) {
            (Some(size), Some(next)) => format!("?maxResults={size}&nextToken={next}"),
            (Some(size), None) => format!("?maxResults={size}"),
            (None, Some(next)) => format!("?nextToken={next}"),
            (None, None) => String::new(),
        };
        let body = list_portals(emulator, case, &query).await;
        let page = body["items"].as_array().cloned().unwrap_or_default();
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {case:?}: a page holds at most {size} summaries, got {body}"
            );
        }
        seen.extend(page);
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return seen;
        }
    }
    panic!(
        "case {case:?}: expected the page chain to end, got a NextToken after {} summaries",
        seen.len()
    )
}

/// The `Preview` a portal summary must report, given whether a preview was built on it.
fn assert_preview(case: &str, portal_id: &str, item: &Value, previewed: bool) {
    let preview = item.get(PREVIEW).unwrap_or_else(|| {
        panic!("case {case:?}: expected the summary of {portal_id} to carry {PREVIEW}, got {item}")
    });
    assert!(
        preview.is_object(),
        "case {case:?}: expected {PREVIEW} on {portal_id} to be a structure, got {preview}"
    );
    if previewed {
        assert_eq!(
            preview.get(PREVIEW_STATUS).and_then(Value::as_str),
            Some(PREVIEW_STATUS_PREVIEW_READY),
            "case {case:?}: expected the built preview's status on {portal_id}, got {preview}"
        );
        let url = preview
            .get(PREVIEW_URL)
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            url.contains(portal_id),
            "case {case:?}: expected a preview URL naming {portal_id}, got {preview}"
        );
    } else {
        assert_eq!(
            preview,
            &json!({}),
            "case {case:?}: expected the empty preview on {portal_id}, nothing having \
             previewed it, got {preview}"
        );
    }
}

#[tokio::test]
async fn every_listed_portal_summary_carries_the_preview_member_at_every_page_size() {
    for row in ROWS {
        // A fresh emulator per row, so the account holds exactly this row's portals.
        let emulator = emulator();
        let mut created: Vec<String> = Vec::new();
        for index in 0..row.previewed.len() {
            created.push(
                create_portal(
                    &emulator,
                    row.case,
                    &format!("Listed Portal {index}"),
                )
                .await,
            );
        }
        // Every preview this row asks for is STARTED before any of them is waited on, so
        // the builds the service owes run side by side rather than one after another.
        for (portal_id, previewed) in created.iter().zip(row.previewed) {
            if *previewed {
                start_preview(&emulator, row.case, portal_id).await;
            }
        }
        for (portal_id, previewed) in created.iter().zip(row.previewed) {
            if *previewed {
                await_preview_ready(&emulator, row.case, portal_id).await;
            }
        }
        let expected: BTreeMap<String, bool> = created
            .iter()
            .cloned()
            .zip(row.previewed.iter().copied())
            .collect();

        // Every page size from one summary per page up to one past the total, and the
        // unpaged listing: the member is on each element wherever the boundary falls.
        for page_size in (1..=created.len() + 1).map(Some).chain([None]) {
            let case = match page_size {
                Some(size) => format!("{}, maxResults={size}", row.case),
                None => format!("{}, unpaged", row.case),
            };
            let case = case.as_str();
            let items = walk_items(&emulator, case, page_size, created.len()).await;
            assert_eq!(
                items.len(),
                created.len(),
                "case {case:?}: expected every portal the account holds to be listed, got \
                 {items:?}"
            );
            for item in &items {
                let portal_id = as_str(item, "portalId");
                let previewed = expected.get(&portal_id).copied().unwrap_or_else(|| {
                    panic!("case {case:?}: an unknown portal {portal_id} was listed, got {item}")
                });
                assert_preview(case, &portal_id, item, previewed);
            }
        }
    }
}

#[tokio::test]
async fn a_listed_portal_summary_reports_the_preview_get_portal_reports() {
    let emulator = emulator();
    // One account in MIXED preview states, so no element's projection can stand in for
    // another's.
    let previewed = [true, false, true];
    let case = "three portals, the outer two previewed";
    let mut created: Vec<String> = Vec::new();
    for index in 0..previewed.len() {
        created.push(create_portal(&emulator, case, &format!("Read Back Portal {index}")).await);
    }
    for (portal_id, built) in created.iter().zip(previewed) {
        if built {
            start_preview(&emulator, case, portal_id).await;
        }
    }
    for (portal_id, built) in created.iter().zip(previewed) {
        if built {
            await_preview_ready(&emulator, case, portal_id).await;
        }
    }

    let listed = walk_items(&emulator, case, None, created.len()).await;
    for (portal_id, built) in created.iter().zip(previewed) {
        let item = listed
            .iter()
            .find(|item| as_str(item, "portalId") == *portal_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected {portal_id} to be listed"));
        let (status, read) = call(&emulator, "GET", &format!("{COLLECTION}/{portal_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal on {portal_id}, got {status} {read}"
        );
        // The summary is the record the single-portal reader reports, member for member —
        // the listing projects the store rather than a shape of its own.
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                item.get(member),
                read.get(member),
                "case {case:?}: expected the summary of {portal_id} to report {member} as \
                 GetPortal reports it, got {item} and {read}"
            );
        }
        // Asserted of BOTH readers, so the equality above cannot be satisfied by two
        // readers that each omit the member.
        assert_preview(case, portal_id, item, built);
        assert_preview(case, portal_id, &read, built);
    }
}
