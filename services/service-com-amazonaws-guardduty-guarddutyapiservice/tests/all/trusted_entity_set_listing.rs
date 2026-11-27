// SPDX-License-Identifier: CC0-1.0

//! `ListTrustedEntitySets`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `ListTrustedEntitySets` names exactly the trusted entity sets the request's detector
//!    holds and GuardDuty still keeps — every id `CreateTrustedEntitySet` minted and
//!    nothing else — in `TrustedEntitySetId` order, and each id it reports resolves
//!    through `GetTrustedEntitySet`. A set whose delete has been served is no longer
//!    one the detector holds, so the list stops naming it, while its own read keeps
//!    reporting it under the status its deletion moved it to.
//! 2. It is PAGINATED (`smithy_query(operation="ListTrustedEntitySets")` reports
//!    `inputToken`/`outputToken` `NextToken`, `items` `TrustedEntitySetIds`, `pageSize`
//!    `MaxResults`): walking the pages with any page size yields each id exactly once in
//!    the unpaged order, the last page carries no `NextToken`, and no page is longer than
//!    `TrustedEntitySetIds`'s own `len 0..50` whatever the caller asks for.
//! 3. It REPORTS: listing changes nothing a later call sees — no status settles early,
//!    the detector's single trusted-set slot is neither consumed nor freed, and no later
//!    list or read differs because something was listed.
//! 4. A trusted entity set belongs to one detector, which is regional, so a list names
//!    only that detector's sets through the Region's endpoint that created it; a
//!    `detectorId` the store does not resolve and a `nextToken` this service never issued
//!    are refused with HTTP 400 under the `InvalidInputException` class naming the member
//!    at fault — never a hollow empty page, and never echoing the caller's value.

use std::time::Duration;

use serde_json::{json, Value};

use crate::client_rejections::page_token_refusal;
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, ENTITY_SET_ACTIVATING, ENTITY_SET_DELETED, ENTITY_SET_DELETE_PENDING,
    ENTITY_SET_INACTIVE, ENTITY_SET_TRANSITION_MILLIS, NEXT_TOKEN_MEMBER,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// `TrustedEntitySetIds` is `len 0..50` and `MaxResults` is `range 1..50`
/// (`smithy_query(operation="ListTrustedEntitySets")`), so one page carries at most 50 ids
/// however many entity sets the detector holds.
const TRUSTED_ENTITY_SET_IDS_MAX: i64 = 50;

/// How many trusted entity sets one detector keeps at once: "at any given time, you can
/// have only one trusted entity set"
/// (`smithy_query(query="doc",shape="CreateTrustedEntitySet")`), so the LIVE collection a
/// list names is never longer than this.
const LIVE_TRUSTED_ENTITY_SETS: usize = 1;

/// A well-formed entity-set id (32 lowercase hex characters, the form this service mints)
/// that no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// A margin on top of the transition span, for the wall clock the handler reads.
const SETTLE_MARGIN_MILLIS: u64 = 250;

/// Long enough that an activated set's `ACTIVATING` -> `ACTIVE` transition is due.
fn past_the_transition() -> Duration {
    let span = u64::try_from(ENTITY_SET_TRANSITION_MILLIS).unwrap_or_default();
    Duration::from_millis(span + SETTLE_MARGIN_MILLIS)
}

async fn create_set(router: &Router, region: &str, detector_id: &str, body: Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/trustedentityset"),
        Some(body),
    )
    .await
}

/// Create a trusted entity set and return its minted id, failing the test if the create
/// did not succeed.
async fn created(router: &Router, region: &str, detector_id: &str, case: &str, body: Value) -> String {
    let (status, response) = create_set(router, region, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-trusted-entity-set expected 200, got {status} {response:?}"
    );
    response["trustedEntitySetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a trustedEntitySetId, got {response:?}"))
        .to_string()
}

async fn delete_set(router: &Router, region: &str, detector_id: &str, set_id: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "DELETE",
        &format!("/detector/{detector_id}/trustedentityset/{set_id}"),
        None,
    )
    .await
}

/// A create body differing from its neighbours in name, format, location form, whether it
/// asks for activation and whether it names an expected bucket owner, so the rows the
/// listing tests build span those classes rather than repeating one recorded request.
fn create_body(index: usize) -> Value {
    // Every member of `TrustedEntitySetFormat`
    // (`smithy_query(shape="TrustedEntitySetFormat")`).
    const FORMATS: [&str; 6] = [
        "TXT",
        "STIX",
        "OTX_CSV",
        "ALIEN_VAULT",
        "PROOF_POINT",
        "FIRE_EYE",
    ];
    // The S3 URL forms a `Location` may take
    // (`smithy_query(query="doc",shape="CreateTrustedEntitySetRequest",member="Location")`):
    // the `s3://` scheme, the path-style HTTPS endpoint and the virtual-hosted-style one.
    let location = match index % 3 {
        0 => format!("s3://my-bucket/lists/listed-{index:03}.txt"),
        1 => format!("https://s3.us-east-1.amazonaws.com/my-bucket/lists/listed-{index:03}.txt"),
        _ => format!("https://my-bucket.s3.us-east-1.amazonaws.com/lists/listed-{index:03}.txt"),
    };
    let mut body = json!({
        "name": format!("listed-{index:03}"),
        "format": FORMATS[index % FORMATS.len()],
        "location": location,
        "activate": index.is_multiple_of(2)
    });
    if !index.is_multiple_of(2) {
        // `ExpectedBucketOwner` is `len 12..12`, `pattern ^[0-9]+$`
        // (`smithy_query(operation="CreateTrustedEntitySet")`).
        body["expectedBucketOwner"] = json!("123456789012");
    }
    body
}

/// One `ListTrustedEntitySets` call. `query` is the raw query string (`""` for none).
async fn list_sets(router: &Router, region: &str, detector_id: &str, query: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/trustedentityset{query}"),
        None,
    )
    .await
}

fn ids_in(body: &Value, case: &str) -> Vec<String> {
    body["trustedEntitySetIds"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a trustedEntitySetIds array, got {body:?}")
        })
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a trustedEntitySetId, got {value:?}")
                })
                .to_string()
        })
        .collect()
}

/// The `TrustedEntitySetIds` of one page, failing the test if the list did not succeed.
async fn page_ids(router: &Router, detector_id: &str, query: &str, case: &str) -> Vec<String> {
    let (status, body) = list_sets(router, "us-east-1", detector_id, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-trusted-entity-sets{query} expected 200, got {status} {body:?}"
    );
    ids_in(&body, case)
}

/// Walk every page of `ListTrustedEntitySets` with `page_size` (`None` asks for the
/// service's own page size) and return the ids in the order the pages reported them, plus
/// the length of each page.
async fn walk_pages(
    router: &Router,
    detector_id: &str,
    page_size: Option<i64>,
    case: &str,
) -> (Vec<String>, Vec<usize>) {
    let mut ids = Vec::new();
    let mut page_lengths = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token the service never advances cannot loop forever.
    for _ in 0..512 {
        let mut query = String::new();
        if let Some(size) = page_size {
            query.push_str(&format!("?maxResults={size}"));
        }
        if let Some(token) = token.as_deref() {
            query.push_str(if query.is_empty() { "?" } else { "&" });
            query.push_str(&format!("nextToken={token}"));
        }
        let (status, body) = list_sets(router, "us-east-1", detector_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: list-trusted-entity-sets{query} expected 200, got {status} {body:?}"
        );
        let page = ids_in(&body, case);
        assert!(
            page.len() as i64 <= TRUSTED_ENTITY_SET_IDS_MAX,
            "case {case:?}: a page must carry at most {TRUSTED_ENTITY_SET_IDS_MAX} ids, got {}",
            page.len()
        );
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: a page must not exceed maxResults={size}, got {}",
                page.len()
            );
        }
        page_lengths.push(page.len());
        ids.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return (ids, page_lengths),
        }
    }
    panic!("case {case:?}: list-trusted-entity-sets never ran out of pages");
}

/// The `Status` `GetTrustedEntitySet` reports, failing the test if the read did not
/// succeed.
async fn status_of(router: &Router, detector_id: &str, set_id: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/trustedentityset/{set_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-trusted-entity-set expected 200, got {status} {body:?}"
    );
    body["status"].as_str().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: the list names exactly the live stored entity sets, in TrustedEntitySetId order.
// ---------------------------------------------------------------------------

/// One row: the case it stands for and how many trusted entity sets the detector holds.
///
/// The rows span what the rule generalises over — COUNT — up to the whole collection a
/// detector keeps at once ([`LIVE_TRUSTED_ENTITY_SETS`]), with each create differing in
/// name, format, location form, activation and expected bucket owner ([`create_body`]).
fn count_rows() -> Vec<(&'static str, usize)> {
    vec![
        ("a detector holding no trusted entity sets", 0),
        (
            "the detector's whole collection at its ceiling",
            LIVE_TRUSTED_ENTITY_SETS,
        ),
    ]
}

#[tokio::test]
async fn a_list_names_every_live_trusted_entity_set_in_order() {
    for (case, count) in count_rows() {
        let router = new_router();
        let detector = enabled_detector(&router).await;

        // Before any create the list is empty — not absent, not canned.
        let empty = page_ids(&router, &detector, "", case).await;
        assert!(
            empty.is_empty(),
            "case {case:?}: a detector with no trusted entity sets must list nothing, got {empty:?}"
        );

        let mut minted = Vec::new();
        for index in 0..count {
            minted.push(created(&router, "us-east-1", &detector, case, create_body(index)).await);
        }

        // `TrustedEntitySetId` order over every id the creates minted: the ids a caller
        // gets back are the stored keys, sorted, whatever order the creates arrived in.
        let mut expected = minted.clone();
        expected.sort();
        let (listed, _) = walk_pages(&router, &detector, None, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the minted ids in TrustedEntitySetId order {expected:?}, got {listed:?}"
        );

        // Every id the list reports resolves through the implemented reader, so the list
        // is the stored entity sets and not a synthesised answer.
        for (index, id) in listed.iter().enumerate() {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/detector/{detector}/trustedentityset/{id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: get-trusted-entity-set {id:?} expected 200, got {status} {body:?}"
            );
            let reported = body["name"].as_str().unwrap_or_default();
            assert!(
                reported.starts_with("listed-"),
                "case {case:?}: listed id {index} must read back a stored entity set, got {body:?}"
            );
        }
    }
}

/// A set GuardDuty is no longer keeping drops out of the LIST, while its retained
/// record keeps answering on its own id with the removal's own status — asserted over
/// several create-delete cycles, each create differing from the last.
#[tokio::test]
async fn a_deleted_trusted_entity_set_leaves_the_list_but_not_the_reader() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let mut gone: Vec<String> = Vec::new();

    for index in 0..3 {
        let case = format!("cycle {index}");
        let id = created(&router, "us-east-1", &detector, &case, create_body(index)).await;
        let listed = page_ids(&router, &detector, "", &case).await;
        assert_eq!(
            listed,
            vec![id.clone()],
            "case {case:?}: expected the live set {id:?} to be the whole list, got {listed:?}"
        );

        let (status, body) = delete_set(&router, "us-east-1", &detector, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete-trusted-entity-set expected 200, got {status} {body:?}"
        );

        let listed = page_ids(&router, &detector, "", &case).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a deleted set must leave the list, got {listed:?}"
        );
        gone.push(id);

        // Leaving the LIST is not leaving the reader: every id a cycle deleted, this
        // cycle's included, still reads back on its own id with the status its
        // removal is in, and never a live one.
        for id in &gone {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/detector/{detector}/trustedentityset/{id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a retained deleted record must still read back, got {status} \
                 {body:?}"
            );
            let reported = body["status"].as_str().unwrap_or_default();
            assert!(
                reported == ENTITY_SET_DELETE_PENDING || reported == ENTITY_SET_DELETED,
                "case {case:?}: expected {ENTITY_SET_DELETE_PENDING} or {ENTITY_SET_DELETED} on \
                 the deleted {id}, got {reported:?} {body:?}"
            );
        }
    }
}

/// A create the service refused leaves the list exactly as it was: the ids named are the
/// ones that were really persisted.
#[tokio::test]
async fn a_refused_create_adds_nothing_to_the_list() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let kept = created(
        &router,
        "us-east-1",
        &detector,
        "the stored trusted entity set",
        create_body(0),
    )
    .await;

    // Each row: the case, the detector the create names, and the create body — a name
    // outside the documented character class, one outside the modelled `len 1..300`, a
    // format the enum does not carry, a location that is not an S3 URL, a create under a
    // detector the store does not resolve, and a second set past the one-at-a-time rule.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "a name longer than the modelled maximum",
            detector.clone(),
            json!({"name": "n".repeat(301), "format": "TXT", "location": "s3://b/l.txt", "activate": true}),
        ),
        (
            "an empty name, below the modelled minimum",
            detector.clone(),
            json!({"name": "", "format": "TXT", "location": "s3://b/l.txt", "activate": true}),
        ),
        (
            "a name holding a character the documented class excludes",
            detector.clone(),
            json!({"name": "bad name", "format": "TXT", "location": "s3://b/l.txt", "activate": true}),
        ),
        (
            "a format the enum does not carry",
            detector.clone(),
            json!({"name": "bad-format", "format": "NOT_A_FORMAT", "location": "s3://b/l.txt", "activate": true}),
        ),
        (
            "a location that is not an S3 URL",
            detector.clone(),
            json!({"name": "bad-location", "format": "TXT", "location": "ftp://b/l.txt", "activate": true}),
        ),
        (
            "a create under a detector the store does not resolve",
            ABSENT_DETECTOR_ID.to_string(),
            create_body(1),
        ),
        (
            "a second set, past the one GuardDuty keeps at a time",
            detector.clone(),
            create_body(2),
        ),
    ];

    for (case, detector_id, body) in rows {
        let (status, response) = create_set(&router, "us-east-1", &detector_id, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused with 400, got {status} {response:?}"
        );
        let listed = page_ids(&router, &detector, "", case).await;
        assert_eq!(
            listed,
            vec![kept.clone()],
            "case {case:?}: a refused create must not change the list, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: paging with any page size yields each id exactly once, in order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_yields_each_trusted_entity_set_exactly_once_in_order() {
    for (case, count) in count_rows() {
        let router = new_router();
        let detector = enabled_detector(&router).await;
        let mut minted = Vec::new();
        for index in 0..count {
            minted.push(created(&router, "us-east-1", &detector, case, create_body(index)).await);
        }
        let mut expected = minted.clone();
        expected.sort();

        // The page sizes span `MaxResults`'s modelled range (`range 1..50`): its published
        // minimum, a size that exactly covers the list, one past it, a size in between,
        // its published maximum, and the service's own page size when the caller names
        // none.
        let mut page_sizes: Vec<Option<i64>> = [
            1,
            count.max(1) as i64,
            count as i64 + 1,
            25,
            TRUSTED_ENTITY_SET_IDS_MAX,
        ]
        .into_iter()
        .map(Some)
        .collect();
        page_sizes.push(None);

        for size in page_sizes {
            let page_case = match size {
                Some(size) => format!("{case}, maxResults={size}"),
                None => format!("{case}, no maxResults"),
            };
            let (listed, page_lengths) = walk_pages(&router, &detector, size, &page_case).await;
            assert_eq!(
                listed, expected,
                "case {page_case:?}: paging must yield every id exactly once in order, got {listed:?}"
            );
            // Every page but the last is full: that is what makes the token walk
            // terminate in the number of calls a client budgets for.
            let per_page = size
                .unwrap_or(TRUSTED_ENTITY_SET_IDS_MAX)
                .min(TRUSTED_ENTITY_SET_IDS_MAX) as usize;
            for (index, length) in page_lengths.iter().enumerate() {
                if index + 1 < page_lengths.len() {
                    assert_eq!(
                        *length, per_page,
                        "case {page_case:?}: page {index} must be full at {per_page}, got {length}"
                    );
                } else {
                    assert!(
                        *length <= per_page,
                        "case {page_case:?}: the last page must carry at most {per_page} ids, got {length}"
                    );
                }
            }
        }

        // A page that covers the whole list carries no token at all, at every size that
        // covers it — a token this service hands out always names an entry that exists.
        for size in [count.max(1) as i64, count as i64 + 1, TRUSTED_ENTITY_SET_IDS_MAX] {
            let page_case = format!("{case}, a complete page at maxResults={size}");
            let (status, body) =
                list_sets(&router, "us-east-1", &detector, &format!("?maxResults={size}")).await;
            assert_eq!(
                status, 200,
                "case {page_case:?}: expected 200, got {status} {body:?}"
            );
            assert!(
                body.get("nextToken").is_none_or(Value::is_null),
                "case {page_case:?}: a complete page must carry no nextToken, got {:?}",
                body["nextToken"]
            );
        }

        // One past each published bound of `MaxResults` is refused by the modelled
        // constraint, before the handler runs: a list is not asked for a page size the
        // model does not admit.
        for size in [0, TRUSTED_ENTITY_SET_IDS_MAX + 1] {
            let page_case = format!("{case}, maxResults={size}, outside the modelled range");
            let (status, body) =
                list_sets(&router, "us-east-1", &detector, &format!("?maxResults={size}")).await;
            assert_eq!(
                status, 400,
                "case {page_case:?}: expected 400, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: listing reports; it changes nothing a later call sees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_call_sees() {
    // The detector keeps one trusted entity set at a time, so each row is its own
    // router: one set the create asked GuardDuty to start using (transitional) and one it
    // did not (stored unused). Neither status may move because something listed.
    for (case, body, expected_status) in [
        ("activate=true", create_body(0), ENTITY_SET_ACTIVATING),
        ("activate=false", create_body(1), ENTITY_SET_INACTIVE),
    ] {
        let router = new_router();
        let detector = enabled_detector(&router).await;
        let set = created(&router, "us-east-1", &detector, case, body).await;

        let first = page_ids(&router, &detector, "", case).await;
        // Several lists, some of them paged, in between.
        for size in [1, 2, TRUSTED_ENTITY_SET_IDS_MAX] {
            let paged = format!("{case}, an interleaved page at maxResults={size}");
            walk_pages(&router, &detector, Some(size), &paged).await;
        }
        let again = page_ids(&router, &detector, "", case).await;
        assert_eq!(
            again, first,
            "case {case:?}: listing must not change what a later list sees, got {again:?} after {first:?}"
        );

        // No status settled early: the transition is owed to the CLOCK, not to a list.
        let status = status_of(&router, &detector, &set, case).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: a list must not settle the status, expected {expected_status}, got {status:?}"
        );

        // Nor did listing free or consume the detector's single slot: a second create is
        // still refused, and the set that holds the slot is still the one that held it.
        let (refused, body) = create_set(&router, "us-east-1", &detector, create_body(3)).await;
        assert_eq!(
            refused, 400,
            "case {case:?}: a list must not free the detector's slot, got {refused} {body:?}"
        );
        let listed = page_ids(&router, &detector, "", case).await;
        assert_eq!(
            listed,
            vec![set.clone()],
            "case {case:?}: the slot must still be held by {set:?}, got {listed:?}"
        );

        // And once the clock has settled the status, further lists still change nothing.
        tokio::time::sleep(past_the_transition()).await;
        let settled = status_of(&router, &detector, &set, case).await;
        for size in [1, 2] {
            let paged = format!("{case}, a list after the transition");
            walk_pages(&router, &detector, Some(size), &paged).await;
        }
        let now = status_of(&router, &detector, &set, case).await;
        assert_eq!(
            now, settled,
            "case {case:?}: a list must not move a settled status, got {now:?} after {settled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: scoped to one detector and Region; bad input is a classified 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_is_scoped_to_its_detector_and_region() {
    let router = new_router();
    let home = enabled_detector(&router).await;
    let home_ids = vec![
        created(
            &router,
            "us-east-1",
            &home,
            "the us-east-1 detector",
            create_body(0),
        )
        .await,
    ];

    // A detector of its own in another Region, with an entity set of its own: one
    // Region's list must name neither the other Region's entity sets nor its detector.
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create-detector in eu-west-1\": expected 200, got {status} {body:?}"
    );
    let away = body["detectorId"]
        .as_str()
        .expect("a detectorId in eu-west-1")
        .to_string();
    let away_ids = vec![
        created(
            &router,
            "eu-west-1",
            &away,
            "the eu-west-1 detector",
            create_body(1),
        )
        .await,
    ];

    let listed = page_ids(&router, &home, "", "the us-east-1 list").await;
    assert_eq!(
        listed, home_ids,
        "case \"the us-east-1 list\": expected only this detector's entity sets {home_ids:?}, got {listed:?}"
    );

    let (status, body) = list_sets(&router, "eu-west-1", &away, "").await;
    assert_eq!(
        status, 200,
        "case \"the eu-west-1 list\": expected 200, got {status} {body:?}"
    );
    let listed = ids_in(&body, "the eu-west-1 list");
    assert_eq!(
        listed, away_ids,
        "case \"the eu-west-1 list\": expected only that detector's entity sets {away_ids:?}, got {listed:?}"
    );

    // Each detector is served only through the Region that created it.
    for (region, detector_id) in [("eu-west-1", home.as_str()), ("us-west-2", away.as_str())] {
        let case = format!("{detector_id} through {region}");
        let (status, body) = list_sets(&router, region, detector_id, "").await;
        assert_eq!(
            status, 400,
            "case {case:?}: another Region's endpoint must not list the entity sets, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn an_unresolvable_detector_or_token_is_refused_without_echoing_it() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let stored = vec![
        created(
            &router,
            "us-east-1",
            &detector,
            "the stored entity set",
            create_body(0),
        )
        .await,
    ];

    // Each row: the case, the detectorId the list uses, the query it carries, and the
    // member the refusal must name.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a well-formed detectorId no create minted",
            ABSENT_DETECTOR_ID.to_string(),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a value that cannot be a detectorId at all",
            "not-a-detector".to_string(),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a detectorId at the modelled minimum length",
            "x".to_string(),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a detectorId at the modelled maximum length",
            "d".repeat(300),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "an entity-set id used where a detectorId belongs",
            ABSENT_SET_ID.to_string(),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a nextToken that is not a token this service issues",
            detector.clone(),
            "?nextToken=not-a-token".to_string(),
            NEXT_TOKEN_MEMBER,
        ),
        (
            "a nextToken pointing past the end of the list",
            detector.clone(),
            "?nextToken=9999".to_string(),
            NEXT_TOKEN_MEMBER,
        ),
        (
            "a nextToken pointing at the entry just past the last one",
            detector.clone(),
            format!("?nextToken={}", stored.len()),
            NEXT_TOKEN_MEMBER,
        ),
    ];

    for (case, detector_id, query, member) in rows {
        let (status, body) = list_sets(&router, "us-east-1", &detector_id, &query).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body:?}");
        let class = error_code(&body);
        assert_eq!(
            class, INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        if member == NEXT_TOKEN_MEMBER {
            // A page token this service did not issue is refused with the ONE
            // sentence the whole pagination surface answers, which names no member
            // at all.
            assert_eq!(
                message,
                page_token_refusal(),
                "case {case:?}: expected the page-token refusal, got {message:?}"
            );
        } else {
            assert!(
                message.contains(member),
                "case {case:?}: the refusal must name {member}, got {message:?}"
            );
        }
        for echoed in [
            "not-a-detector",
            "not-a-token",
            "9999",
            ABSENT_DETECTOR_ID,
            ABSENT_SET_ID,
        ] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
            );
        }
        // The refusal leaves the store alone: the stored entity set still lists.
        let listed = page_ids(&router, &detector, "", case).await;
        assert_eq!(
            listed, stored,
            "case {case:?}: a refusal must not change the list, got {listed:?}"
        );
    }

    // The token walk the service DOES serve is accepted on the same list: the refusals
    // above are about tokens it never handed out, not about paging itself. The first page
    // covers the whole live collection, so it carries no token at all.
    let (status, body) = list_sets(&router, "us-east-1", &detector, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case \"the first page\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        ids_in(&body, "the first page"),
        stored,
        "case \"the first page\": expected the live collection, got {body:?}"
    );
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "case \"the first page\": a page covering the list must carry no nextToken, got {:?}",
        body["nextToken"]
    );
}
