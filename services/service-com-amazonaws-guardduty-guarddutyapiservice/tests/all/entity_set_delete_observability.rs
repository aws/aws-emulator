// SPDX-License-Identifier: CC0-1.0

//! What a client that DELETES an entity set and then reads it back observes, on both
//! entity-set families, end to end over the wire.
//!
//! `threat_entity_set_delete.rs` and `trusted_entity_set_delete.rs` own one family's
//! delete each; this suite owns the one rule both reads follow once a delete has been
//! served.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, asserted over a TABLE spanning both families and everything
//! the rule generalises over — the status the set was in when the delete arrived
//! (including the transitional one), the `*EntitySetFormat` the create stored, whether
//! the caller activated the set, whether it carried an `ExpectedBucketOwner`, the
//! number of tags, the size of the name and the number of sets deleted under one
//! detector:
//!
//! 1. The removal a delete starts is OBSERVABLE through the family's own read, which
//!    reports a `Status` the model requires of it: the retained record answers HTTP
//!    200 with `DELETE_PENDING` while the removal is in flight and `DELETED` once the
//!    clock owes it, so a client that polls the read reaches the terminal status
//!    instead of an exception. `DELETED` is terminal — no later read walks the record
//!    back to a live status — and the record keeps reporting everything the create
//!    stored.
//! 2. Being READABLE is not being HELD: the detector stops holding a deleted set the
//!    instant the delete answers, so the listing names only the live sets, a live
//!    sibling of the same family still reads back, and a SECOND delete of that id is
//!    refused with the entity-set families' absence refusal — the same one an id no
//!    create ever minted gets.
//!
//! `entity_set_delete_retention.rs` owns what repeated create-and-delete CHURN leaves
//! behind — including the id whose retained record the store has dropped, which is an
//! absence again — and `entity_set_delete_absence.rs` the other ways an entity-set id
//! fails to resolve.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
use crate::set_delete_terminality::{assert_entity_set_absent, ABSENT_SET_ID};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ENTITY_SET_ACTIVE, ENTITY_SET_DELETED, ENTITY_SET_DELETE_PENDING, ENTITY_SET_INACTIVE,
};

/// How long a poll may wait for a status the clock owes before the test gives up.
/// Generous on purpose: the wait ends on the CONDITION, and this is only the deadline
/// past which the emulator is considered broken.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The gap between two reads of a poll. Only the pause between attempts; no loop
/// below ever ends on it.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// How many times a read of a deleted id is repeated to show the terminal status is
/// not a phase: every read after it agrees.
const REPEATED_READS: usize = 3;

/// One entity-set family: the collection segment under `/detector/{DetectorId}`, the
/// member its create reports the minted id in and the member its listing names ids
/// in.
struct Family {
    case: &'static str,
    segment: &'static str,
    id_member: &'static str,
    ids_member: &'static str,
}

/// Both families, so neither can drift from the shared rule.
const FAMILIES: [Family; 2] = [
    Family {
        case: "threat entity set",
        segment: "threatentityset",
        id_member: "threatEntitySetId",
        ids_member: "threatEntitySetIds",
    },
    Family {
        case: "trusted entity set",
        segment: "trustedentityset",
        id_member: "trustedEntitySetId",
        ids_member: "trustedEntitySetIds",
    },
];

/// One case: the create body, and whether the set's own transition is waited out
/// before the delete arrives.
struct Row {
    case: &'static str,
    create: Value,
    /// Whether to wait for the create's transition to settle before deleting, so the
    /// delete arrives at a settled set in one row and mid-transition in another.
    settle_first: bool,
}

/// The rows differ along what the rule generalises over: the status the set held when
/// the delete arrived, the FORMAT (members of `ThreatEntitySetFormat`, which the
/// trusted family models identically — `smithy_query(shape="ThreatEntitySetFormat")`
/// lists TXT, STIX, OTX_CSV, ALIEN_VAULT, PROOF_POINT and FIRE_EYE), the S3 URL form
/// of the location, whether the set was activated, whether it recorded an expected
/// bucket owner, the number of tags and the size of the name (both ends of the
/// modelled `len 1..300`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an ACTIVE set, settled before the delete",
            create: json!({
                "name": "deletion_probe-1",
                "format": "TXT",
                "location": "s3://guardduty-lists/probe.txt",
                "activate": true,
                "tags": {"team": "secops"},
            }),
            settle_first: true,
        },
        Row {
            case: "an ACTIVATING set, deleted mid-transition",
            create: json!({
                "name": "a",
                "format": "OTX_CSV",
                "location": "https://guardduty-lists.s3.us-west-2.amazonaws.com/otx.csv",
                "activate": true,
            }),
            settle_first: false,
        },
        Row {
            case: "an INACTIVE set the caller never activated",
            create: json!({
                "name": "f".repeat(300),
                "format": "FIRE_EYE",
                "location": "https://s3.amazonaws.com/guardduty-lists/deep/nested/fire-eye.txt",
                "activate": false,
                "expectedBucketOwner": "000000000000",
                "tags": {"team": "secops", "env": "test"},
            }),
            settle_first: true,
        },
        Row {
            case: "a STIX set with an expected bucket owner",
            create: json!({
                "name": "STIX_set-2",
                "format": "STIX",
                "location": "s3://guardduty-lists/threat.stix",
                "activate": false,
                "expectedBucketOwner": "123456789012",
            }),
            settle_first: false,
        },
    ]
}

impl Family {
    async fn create(&self, router: &Router, detector_id: &str, body: Value, case: &str) -> String {
        let (status, got) = call(
            router,
            "POST",
            &format!("/detector/{detector_id}/{}", self.segment),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {got:?}"
        );
        got[self.id_member]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {} from the create, got {got:?}",
                    self.id_member
                )
            })
            .to_string()
    }

    async fn get(&self, router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
        call(
            router,
            "GET",
            &format!("/detector/{detector_id}/{}/{set_id}", self.segment),
            None,
        )
        .await
    }

    /// Read a set back, failing the test if the read did not answer 200.
    async fn read(&self, router: &Router, detector_id: &str, set_id: &str, case: &str) -> Value {
        let (status, body) = self.get(router, detector_id, set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status} {body:?}"
        );
        body
    }

    async fn delete(&self, router: &Router, detector_id: &str, set_id: &str, case: &str) {
        let (status, got) = self.delete_raw(router, detector_id, set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {got:?}"
        );
    }

    async fn delete_raw(&self, router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
        call(
            router,
            "DELETE",
            &format!("/detector/{detector_id}/{}/{set_id}", self.segment),
            None,
        )
        .await
    }

    /// The ids this family's listing names. The collections here are bounded well
    /// under the modelled page size, so one page holds them all.
    async fn listed(&self, router: &Router, detector_id: &str, case: &str) -> Vec<String> {
        let (status, got) = call(
            router,
            "GET",
            &format!("/detector/{detector_id}/{}", self.segment),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the listing to answer 200, got {status} {got:?}"
        );
        got[self.ids_member]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .map(|id| id.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Read `set_id` until it reports `want`, or until [`SETTLE_DEADLINE`] has passed
    /// — the CONDITION ends the wait, never a fixed pause. The record being waited on
    /// here is one the service still holds, so a read that does not answer 200 is a
    /// failure of the wait.
    async fn read_until_status(
        &self,
        router: &Router,
        detector_id: &str,
        set_id: &str,
        want: &str,
        case: &str,
    ) -> Value {
        let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let (status, body) = self.get(router, detector_id, set_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a record the service still holds must stay readable while it \
                 settles, got {status} {body:?}"
            );
            if body["status"].as_str() == Some(want) {
                return body;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {case:?}: {set_id} never reached {want}, last read {body:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Assert every one of a handful of consecutive reads of `set_id` reports the
    /// terminal `DELETED`, so the settled removal is not a phase the caller can read
    /// its way out of.
    async fn assert_stays_deleted(
        &self,
        router: &Router,
        detector_id: &str,
        set_id: &str,
        case: &str,
    ) {
        for attempt in 0..REPEATED_READS {
            let body = self
                .read(router, detector_id, set_id, &format!("{case}: read {attempt}"))
                .await;
            let reported = body["status"].as_str().unwrap_or_default();
            assert_eq!(
                reported, ENTITY_SET_DELETED,
                "case {case:?}: read {attempt} must still report {ENTITY_SET_DELETED}, got \
                 {reported:?} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the delete is observable, from DELETE_PENDING to the terminal DELETED.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_entity_set_reports_delete_pending_and_then_deleted() {
    for family in &FAMILIES {
        for row in rows() {
            let case = format!("{}: {}", family.case, row.case);
            let case = case.as_str();
            // One store per row: the trusted family keeps one set at a time, so a row
            // cannot share a detector with the next.
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let set_id = family
                .create(&router, &detector_id, row.create.clone(), case)
                .await;
            if row.settle_first {
                // The status the create's own transition settles on: a set the caller
                // asked GuardDuty to use ends ACTIVE, one it did not is stored
                // INACTIVE, which is settled already.
                let want = if row.create["activate"] == json!(true) {
                    ENTITY_SET_ACTIVE
                } else {
                    ENTITY_SET_INACTIVE
                };
                family
                    .read_until_status(&router, &detector_id, &set_id, want, case)
                    .await;
            }
            let created = family.read(&router, &detector_id, &set_id, case).await;

            family.delete(&router, &detector_id, &set_id, case).await;

            // Rule 1: the very next read answers 200 with the transitional status —
            // the removal is in flight and the client can see that it is.
            let pending = family.read(&router, &detector_id, &set_id, case).await;
            assert_eq!(
                pending["status"].as_str().unwrap_or_default(),
                ENTITY_SET_DELETE_PENDING,
                "case {case:?}: expected {ENTITY_SET_DELETE_PENDING} straight after the delete, \
                 got {pending:?}"
            );
            // The delete moved the status and nothing else: everything the create
            // stored is still reported, so a client reading the record it just
            // deleted still learns which list it was.
            for member in ["name", "format", "location", "tags", "createdAt"] {
                assert_eq!(
                    pending[member], created[member],
                    "case {case:?}: a deleted record must still report the stored {member}, \
                     expected {:?}, got {:?}",
                    created[member], pending[member]
                );
            }
            assert_eq!(
                pending["expectedBucketOwner"], created["expectedBucketOwner"],
                "case {case:?}: a deleted record must still report the stored \
                 expectedBucketOwner, expected {:?}, got {:?}",
                created["expectedBucketOwner"], pending["expectedBucketOwner"]
            );

            // Rule 1, the progression a client polls: the clock settles the removal
            // on the terminal member of the status enum, and it stays there.
            let deleted = family
                .read_until_status(&router, &detector_id, &set_id, ENTITY_SET_DELETED, case)
                .await;
            assert_eq!(
                deleted["name"], created["name"],
                "case {case:?}: a settled deletion still reports the stored name, got {deleted:?}"
            );
            family
                .assert_stays_deleted(&router, &detector_id, &set_id, case)
                .await;

            // Rule 2: the detector no longer HOLDS it, so a second delete has
            // nothing left to accept and is refused as absent.
            let (status, body) = family.delete_raw(&router, &detector_id, &set_id).await;
            assert_entity_set_absent(case, status, &body, &set_id);
            // That refusal did not take the record away: the read still answers.
            family
                .assert_stays_deleted(&router, &detector_id, &set_id, case)
                .await;

            // Rule 2: the refusal is a lookup — an id of the same form no create
            // minted is refused under this very detector, while the deleted id
            // reads back.
            let (status, body) = family.get(&router, &detector_id, ABSENT_SET_ID).await;
            assert_entity_set_absent(case, status, &body, ABSENT_SET_ID);
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the listing names the live sets, and a live sibling still reads back.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_entity_set_leaves_the_collection_but_not_its_own_read() {
    // The rows differ along HOW MANY of a detector's sets are deleted: the trusted
    // family holds one at a time, so it is driven one set per cycle, while the threat
    // family holds several and has two deleted out from under a live sibling.
    for family in &FAMILIES {
        let case = format!("{}: deleted beside a live sibling", family.case);
        let case = case.as_str();
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let mut deleted: Vec<String> = Vec::new();
        for index in 0..2 {
            let id = family
                .create(
                    &router,
                    &detector_id,
                    json!({
                        "name": format!("collection_probe-{index}"),
                        "format": "TXT",
                        "location": format!("s3://guardduty-lists/probe-{index}.txt"),
                        "activate": index % 2 == 0,
                    }),
                    case,
                )
                .await;
            family.delete(&router, &detector_id, &id, case).await;
            let listing = family.listed(&router, &detector_id, case).await;
            assert!(
                !listing.contains(&id),
                "case {case:?}: the listing must stop naming the deleted {id}, got {listing:?}"
            );
            deleted.push(id);
        }

        // A live sibling, to show the listing names what the detector really holds
        // rather than nothing at all.
        let live = family
            .create(
                &router,
                &detector_id,
                json!({
                    "name": "collection_probe-live",
                    "format": "STIX",
                    "location": "s3://guardduty-lists/live.txt",
                    "activate": false,
                }),
                case,
            )
            .await;
        let listing = family.listed(&router, &detector_id, case).await;
        assert_eq!(
            listing,
            vec![live.clone()],
            "case {case:?}: the listing must name exactly the live set, got {listing:?}"
        );

        // Each deleted id still reads back on the status its removal settled on, and
        // a second delete of it is refused: the detector has nothing left to accept.
        // The live sibling is untouched.
        for id in &deleted {
            family
                .read_until_status(&router, &detector_id, id, ENTITY_SET_DELETED, case)
                .await;
            family
                .assert_stays_deleted(&router, &detector_id, id, case)
                .await;
            let (status, body) = family.delete_raw(&router, &detector_id, id).await;
            assert_entity_set_absent(case, status, &body, id);
        }
        let body = family.read(&router, &detector_id, &live, case).await;
        assert_eq!(
            body["status"].as_str().unwrap_or_default(),
            ENTITY_SET_INACTIVE,
            "case {case:?}: the live sibling must still read back unused, got {body:?}"
        );
    }
}
