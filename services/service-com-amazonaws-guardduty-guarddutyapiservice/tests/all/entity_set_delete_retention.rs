// SPDX-License-Identifier: CC0-1.0

//! What repeated create-and-delete CHURN leaves behind on both entity-set families,
//! end to end over the wire: `DeleteThreatEntitySet` and `DeleteTrustedEntitySet`
//! share one deletion helper, so the rule is asserted on each of them.
//!
//! `threat_entity_set_delete.rs` and `trusted_entity_set_delete.rs` own one delete
//! each and `entity_set_delete_observability.rs` the progression a single delete
//! leaves readable on its id; this suite owns what MANY of them do. A delete takes
//! the set off the detector and RETAINS its record so the removal can be read to its
//! terminal status, so the rule a caller can observe under churn is that the store
//! keeps the most recent deletions readable, bounded by
//! `state::DELETED_ENTITY_SETS_RETAINED`, drops the oldest ones past that bound, and
//! never brings a deleted set back into use or into the collection.
//! Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization and the
//! handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. Every id a delete took off the detector reports the terminal `DELETED` while
//!    its record is retained, and never a live status again — for the newest cycle
//!    of the churn as much as for any other one still retained.
//! 2. The retention is BOUNDED: once the churn has deleted more sets than the store
//!    retains, the oldest deletions are dropped, and a dropped id is refused with
//!    the entity-set families' absence refusal — the same one an id no create ever
//!    minted gets. The number of ids that still read back is exactly the bound.
//! 3. The listing names exactly the sets that are live: the churn leaves no ghost in
//!    the collection, and the set the last cycle created is readable and unused.
//!
//! The rows differ along the family, the number of cycles — below the bound, at it
//! and past it — and whether a read lands between a delete and the next create.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
use crate::set_delete_terminality::assert_entity_set_absent;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DELETED_ENTITY_SETS_RETAINED, ENTITY_SET_DELETED, ENTITY_SET_DELETE_PENDING,
    ENTITY_SET_INACTIVE, THREAT_ENTITY_SET_ID_MEMBER, TRUSTED_ENTITY_SET_ID_MEMBER,
};

/// How long a poll may wait for a status the clock owes before the test gives up.
/// Generous on purpose: the wait ends on the CONDITION, and this is only the deadline
/// past which the emulator is considered broken.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The gap between two reads of a poll. Only the pause between attempts; no loop
/// below ever ends on it.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// Assert one read of an id whose deleted record the store still retains: it answers
/// 200 with a status on the way out, and never a live one.
fn assert_never_live_again(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: a retained deleted record must still read back, got {status} {body:?}"
    );
    let reported = body["status"].as_str().unwrap_or_default();
    assert!(
        reported == ENTITY_SET_DELETE_PENDING || reported == ENTITY_SET_DELETED,
        "case {case:?}: expected {ENTITY_SET_DELETE_PENDING} or {ENTITY_SET_DELETED}, got \
         {reported:?} {body:?}"
    );
}

/// One entity-set family: the collection segment of its URI under
/// `/detector/{DetectorId}/`, the member its create reports the minted id in, and the
/// member its listing reports ids in.
struct Family {
    collection: &'static str,
    id_member: &'static str,
    ids_member: &'static str,
}

/// Both families, so neither can drift from the shared deletion rule.
const FAMILIES: [Family; 2] = [
    Family {
        collection: "threatentityset",
        id_member: THREAT_ENTITY_SET_ID_MEMBER,
        ids_member: "threatEntitySetIds",
    },
    Family {
        collection: "trustedentityset",
        id_member: TRUSTED_ENTITY_SET_ID_MEMBER,
        ids_member: "trustedEntitySetIds",
    },
];

/// Create one entity set in `family`, answering the id the create minted.
async fn create_set(
    router: &Router,
    detector_id: &str,
    family: &Family,
    index: usize,
    case: &str,
) -> String {
    let collection = family.collection;
    let (status, created) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/{collection}"),
        Some(json!({
            "name": format!("churn-{index}"),
            "format": "TXT",
            "location": format!("s3://guardduty-lists/churn-{index}.txt"),
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create of set {index} to answer 200, got {status} {created:?}"
    );
    created[family.id_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {} from the create of set {index}, got {created:?}",
                family.id_member
            )
        })
        .to_string()
}

/// Read one set of `family` back.
async fn get_set(router: &Router, detector_id: &str, family: &Family, id: &str) -> (u16, Value) {
    let collection = family.collection;
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{collection}/{id}"),
        None,
    )
    .await
}

/// Read one set of `family` until it reports `want`, or until [`SETTLE_DEADLINE`] has
/// passed — the CONDITION ends the wait, never a fixed pause. `false` once the store
/// has stopped retaining the record, so a wait cannot outlive what it waits on.
async fn read_until_status(
    router: &Router,
    detector_id: &str,
    family: &Family,
    id: &str,
    want: &str,
    case: &str,
) -> bool {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = get_set(router, detector_id, family, id).await;
        if status != 200 {
            return false;
        }
        if body["status"].as_str() == Some(want) {
            return true;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {id} never reached {want}, last read {body:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Delete one set of `family`, failing the test if the call did not succeed.
async fn delete_set(router: &Router, detector_id: &str, family: &Family, id: &str, case: &str) {
    let collection = family.collection;
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/{collection}/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete of {id} to answer 200, got {status} {body:?}"
    );
}

/// The ids `family`'s listing names for a detector. The whole collection is bounded
/// well under the modelled page size, so it arrives in one page.
async fn listed(router: &Router, detector_id: &str, family: &Family, case: &str) -> Vec<String> {
    let collection = family.collection;
    let (status, got) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{collection}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the listing to answer 200, got {status} {got:?}"
    );
    got[family.ids_member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected {} in {got:?}",
                family.ids_member
            )
        })
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn churn_retains_the_newest_deletions_and_drops_the_oldest() {
    // The rows differ along the number of create-and-delete cycles — one, a few
    // below the retention bound, exactly the bound and past it — so each row spans a
    // deletion that has just happened, deletions many cycles old, and (on the last
    // row) deletions the store has had to drop.
    for cycles in [
        1_usize,
        DELETED_ENTITY_SETS_RETAINED - 2,
        DELETED_ENTITY_SETS_RETAINED,
        DELETED_ENTITY_SETS_RETAINED + 3,
    ] {
        for family in &FAMILIES {
            let case = format!("{} churned {cycles} times", family.collection);
            let case = case.as_str();
            let router = new_router();
            let detector_id = enabled_detector(&router).await;

            let mut deleted_ids: Vec<String> = Vec::new();
            for index in 0..cycles {
                let id = create_set(&router, &detector_id, family, index, case).await;
                // Every id this cycle minted is live until its delete: one set at a
                // time is enough for both families, the one-at-a-time trusted set
                // included.
                let listing = listed(&router, &detector_id, family, case).await;
                assert_eq!(
                    listing,
                    vec![id.clone()],
                    "case {case:?}: cycle {index} must leave exactly its own set listed"
                );
                delete_set(&router, &detector_id, family, &id, case).await;
                // A read between the delete and the next create: the record is
                // retained, so the read reports the removal and resurrects nothing.
                if index % 2 == 0 {
                    let (status, body) = get_set(&router, &detector_id, family, &id).await;
                    assert_never_live_again(case, status, &body);
                }
                deleted_ids.push(id);
            }

            // Rules 1 and 2: the store retains the NEWEST deletions up to its bound
            // and has dropped the rest, so the ids that still read back are exactly
            // the last `DELETED_ENTITY_SETS_RETAINED` of the churn — each reporting
            // the terminal status — and every older one is the absence refusal.
            let retained_from = deleted_ids.len().saturating_sub(DELETED_ENTITY_SETS_RETAINED);
            // How many of the churned ids the SERVICE still answers for, counted from
            // its own answers rather than computed from the loop above.
            let mut still_readable = 0_usize;
            for (index, id) in deleted_ids.iter().enumerate() {
                let case = format!("{case}: deletion {index}");
                if index >= retained_from {
                    assert!(
                        read_until_status(
                            &router,
                            &detector_id,
                            family,
                            id,
                            ENTITY_SET_DELETED,
                            &case,
                        )
                        .await,
                        "case {case:?}: a deletion within the retention bound must settle on \
                         {ENTITY_SET_DELETED}"
                    );
                } else {
                    let (status, body) = get_set(&router, &detector_id, family, id).await;
                    assert_entity_set_absent(&case, status, &body, id);
                }
                let (status, _) = get_set(&router, &detector_id, family, id).await;
                if status == 200 {
                    still_readable += 1;
                }
            }
            assert_eq!(
                still_readable,
                deleted_ids.len().min(DELETED_ENTITY_SETS_RETAINED),
                "case {case:?}: the store retains at most {DELETED_ENTITY_SETS_RETAINED} deleted \
                 records, got {still_readable} of {} still readable",
                deleted_ids.len()
            );

            // Rule 3: the collection is empty, and a fresh create is served and
            // readable — the churn left the family usable.
            let empty = listed(&router, &detector_id, family, case).await;
            assert!(
                empty.is_empty(),
                "case {case:?}: every set was deleted, so the listing must name none, got \
                 {empty:?}"
            );
            let fresh = create_set(&router, &detector_id, family, cycles, case).await;
            assert!(
                !deleted_ids.contains(&fresh),
                "case {case:?}: a create after the churn must mint an unused id, got {fresh}"
            );
            let (status, body) = get_set(&router, &detector_id, family, &fresh).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fresh set must be readable, got {status} {body:?}"
            );
            assert_eq!(
                body["status"].as_str().unwrap_or_default(),
                ENTITY_SET_INACTIVE,
                "case {case:?}: the fresh set was created unused, got {body:?}"
            );
        }
    }
}
