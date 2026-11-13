// SPDX-License-Identifier: CC0-1.0

//! The deletion time `ListApiKeys` projects is the key's expiration shifted by ONE
//! fixed span, end to end over HTTP: the service keeps every expired `da2` key for the
//! same post-expiry retention, so the derivation is a TRANSLATION of the expiration
//! rather than a per-key, per-API or per-call offset.
//!
//! The arithmetic itself — a key's `deletes` is its `expires` plus that retention,
//! floored to the hour — and the fact that the create, the update and the listing all
//! report one value per key are asserted by [`crate::api_key_deletion_window`], whose
//! constants, helpers and rule assertion this suite reuses rather than restating. What
//! this suite adds is the RELATION between the keys an API holds: a listing of many
//! keys must carry ONE offset for all of them, and so must preserve the order of their
//! expirations and the gaps between them. A derivation that used a shorter window for
//! some keys, or one anchored on anything but the key's own expiration, would answer a
//! listing whose deletion times no longer line up with its expirations even while each
//! value looked plausible on its own.
//!
//! The rows differ along the SET of expirations an API holds — how many keys there are,
//! how far apart their expirations sit (seconds inside one hour, whole hours, days, the
//! width of the accepted window), whether the creation order matches the expiration
//! order, and whether two keys share an expiration — and a second table varies the
//! signed amount a key's expiration is MOVED by, which must move its deletion time by
//! exactly as much.

use serde_json::{json, Value};

use crate::api_key_deletion_window::{
    assert_retention_span, create_key, floor_to_hour, unix_now, update_key, BOUND_SLACK, DAY, HOUR,
    MAX_VALIDITY, MIN_VALIDITY, RETENTION,
};
use crate::error_contract::{call, create_api, router};

/// One row of the uniform-offset table: its label, and the validities (seconds past the
/// clock read) of the keys the API holds, in CREATION order.
struct Row {
    case: &'static str,
    validities: &'static [i64],
}

/// The sets of expirations a listing is asserted over.
const ROWS: &[Row] = &[
    Row {
        case: "a single key",
        validities: &[7 * DAY],
    },
    Row {
        case: "two keys an hour apart",
        validities: &[MIN_VALIDITY + BOUND_SLACK, MIN_VALIDITY + BOUND_SLACK + HOUR],
    },
    Row {
        case: "three keys seconds apart inside one hour",
        validities: &[30 * DAY, 30 * DAY + 1, 30 * DAY + 2],
    },
    Row {
        case: "two keys sharing an expiration",
        validities: &[45 * DAY + 1_234, 45 * DAY + 1_234],
    },
    Row {
        case: "keys created in decreasing expiration order",
        validities: &[300 * DAY, 200 * DAY, 100 * DAY, 10 * DAY],
    },
    Row {
        case: "both published bounds and the middle of the window",
        validities: &[
            MIN_VALIDITY + BOUND_SLACK,
            183 * DAY,
            MAX_VALIDITY - BOUND_SLACK,
        ],
    },
    Row {
        case: "seven keys spread across the window, off hour boundaries",
        validities: &[
            2 * DAY + 17,
            19 * DAY + 5 * HOUR + 59,
            61 * DAY + 1,
            120 * DAY + 3_599,
            201 * DAY + 11 * HOUR + 7,
            280 * DAY + 43,
            364 * DAY + 13 * HOUR + 21,
        ],
    },
];

/// One listed key reduced to the two members under test.
#[derive(Debug, Clone, Copy)]
struct Window {
    expires: i64,
    deletes: i64,
}

/// Every key an API holds, as `ListApiKeys` reports them, reduced to their two time
/// members — in the order the listing reports them.
async fn listed_windows(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    api_id: &str,
) -> Vec<Window> {
    let answer = call(router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListApiKeys 200, got {answer:?}"
    );
    let keys: Vec<Value> = answer.body["apiKeys"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    keys.iter()
        .map(|key| {
            assert_retention_span(case, "ListApiKeys", key);
            Window {
                expires: key["expires"].as_i64().unwrap_or_default(),
                deletes: key["deletes"].as_i64().unwrap_or_default(),
            }
        })
        .collect()
}

/// Rule: one offset for every key of a listing, so the deletion times preserve the
/// order of the expirations and the gaps between them.
#[tokio::test]
async fn a_listing_shifts_every_expiration_by_the_same_span() {
    let router = router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let api_id = create_api(&router, &format!("Api_key_deletes_uniform_{index}")).await;

        for validity in row.validities {
            let answer = create_key(
                &router,
                &api_id,
                json!({
                    "description": format!("{case}: {validity} seconds"),
                    "expires": unix_now() + validity,
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected CreateApiKey 200 for the {validity}-second key, \
                 got {answer:?}"
            );
            assert_retention_span(case, "CreateApiKey", &answer.body["apiKey"]);
        }

        let mut listed = listed_windows(&router, case, &api_id).await;
        assert_eq!(
            listed.len(),
            row.validities.len(),
            "case {case:?}: expected the {} keys the row created, got {listed:?}",
            row.validities.len()
        );

        // One offset for all of them: the set of (deletes - expires) distances holds a
        // single value, the retention span.
        let mut offsets: Vec<i64> = listed
            .iter()
            .map(|window| window.deletes - window.expires)
            .collect();
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(
            offsets,
            vec![RETENTION],
            "case {case:?}: expected one offset {RETENTION} for every key of the listing, \
             got the distinct offsets {offsets:?} over {listed:?}"
        );

        // A translation preserves order and gaps: read in expiration order, the
        // deletion times rise in step with the expirations, and two keys sharing an
        // expiration share a deletion time.
        listed.sort_by_key(|window| window.expires);
        for pair in listed.windows(2) {
            let (earlier, later) = (pair[0], pair[1]);
            assert_eq!(
                later.deletes - earlier.deletes,
                later.expires - earlier.expires,
                "case {case:?}: expected the deletion times {} and {} to sit the same \
                 {} seconds apart as the expirations they follow, got {} apart",
                earlier.deletes,
                later.deletes,
                later.expires - earlier.expires,
                later.deletes - earlier.deletes
            );
            assert!(
                later.deletes >= earlier.deletes,
                "case {case:?}: expected the later expiration {} to carry the later \
                 deletion time, got {:?} after {:?}",
                later.expires,
                later,
                earlier
            );
        }
    }
}

/// One row of the moved-expiration table: its label, and the validity the update
/// submits, in seconds past the clock read.
const MOVES: &[(&str, i64)] = &[
    ("extended by a day", 31 * DAY),
    ("extended to just inside the maximum validity", MAX_VALIDITY - BOUND_SLACK),
    ("shortened to just inside the minimum validity", MIN_VALIDITY + BOUND_SLACK),
    ("moved onto an instant off an hour boundary", 77 * DAY + 2_521),
    ("moved by exactly the retention span", 60 * DAY),
    ("moved back inside its original hour", 30 * DAY + 11),
];

/// The same rule in time: moving ONE key's expiration moves its deletion time by
/// exactly the amount the expiration moved, and leaves its neighbour's alone.
#[tokio::test]
async fn moving_one_expiration_moves_its_deletion_time_by_the_same_amount() {
    let router = router();
    let api_id = create_api(&router, "Api_key_deletes_uniform_moves").await;

    // The subject the rows move, and a neighbour no row writes at all.
    let answer = create_key(
        &router,
        &api_id,
        json!({ "description": "moved", "expires": unix_now() + 30 * DAY }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case the subject key: expected CreateApiKey 200, got {answer:?}"
    );
    let created = answer.body["apiKey"].clone();
    let key_id = created["id"].as_str().unwrap_or_default().to_string();
    assert_retention_span("the subject key", "CreateApiKey", &created);

    let answer = create_key(
        &router,
        &api_id,
        json!({ "description": "left alone", "expires": unix_now() + 90 * DAY }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case the neighbour key: expected CreateApiKey 200, got {answer:?}"
    );
    let neighbour = answer.body["apiKey"].clone();
    let neighbour_id = neighbour["id"].as_str().unwrap_or_default().to_string();
    assert_retention_span("the neighbour key", "CreateApiKey", &neighbour);

    let mut before = Window {
        expires: created["expires"].as_i64().unwrap_or_default(),
        deletes: created["deletes"].as_i64().unwrap_or_default(),
    };

    for (case, validity) in MOVES {
        let case = *case;
        let submitted = unix_now() + validity;
        let answer = update_key(&router, &api_id, &key_id, json!({ "expires": submitted })).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateApiKey 200, got {answer:?}"
        );
        let updated = answer.body["apiKey"].clone();
        assert_retention_span(case, "UpdateApiKey", &updated);
        let after = Window {
            expires: updated["expires"].as_i64().unwrap_or_default(),
            deletes: updated["deletes"].as_i64().unwrap_or_default(),
        };
        assert_eq!(
            after.expires,
            floor_to_hour(submitted),
            "case {case:?}: expected the submitted expiration floored to the hour, \
             got {updated:?}"
        );
        assert_eq!(
            after.deletes - before.deletes,
            after.expires - before.expires,
            "case {case:?}: expected the deletion time to move the {} seconds the \
             expiration moved, got {} (from {before:?} to {after:?})",
            after.expires - before.expires,
            after.deletes - before.deletes
        );

        // The listing reports the moved key with that value, and the untouched
        // neighbour with the one its own expiration still derives.
        let listed = call(&router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListApiKeys 200, got {listed:?}"
        );
        let keys = listed.body["apiKeys"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for key in &keys {
            assert_retention_span(case, "ListApiKeys", key);
        }
        let found = keys
            .iter()
            .find(|key| key["id"].as_str() == Some(key_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: expected the listing to hold {key_id}"));
        assert_eq!(
            found["deletes"].as_i64(),
            Some(after.deletes),
            "case {case:?}: expected the listing to report the moved deletion time {}, \
             got {found:?}",
            after.deletes
        );
        let other = keys
            .iter()
            .find(|key| key["id"].as_str() == Some(neighbour_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: expected the listing to hold {neighbour_id}"));
        assert_eq!(
            other, &neighbour,
            "case {case:?}: expected the untouched neighbour to be unmoved, got {other:?}"
        );

        before = after;
    }
}
