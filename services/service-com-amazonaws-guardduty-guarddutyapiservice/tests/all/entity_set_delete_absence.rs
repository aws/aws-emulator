// SPDX-License-Identifier: CC0-1.0

//! Why an entity-set id can fail to resolve, end to end over the wire, on both
//! entity-set families: `GetThreatEntitySet` / `DeleteThreatEntitySet` and their
//! trusted siblings.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test: the service holds an entity set from the create that minted it
//! until the delete that takes it away, and an id it holds no entity set for resolves
//! to NOTHING — however the id came to name no set. So every one of those ways gets the
//! one absence refusal both families share, HTTP 400 under the
//! `InvalidInputException` class with the fixed sentence
//! `scaffolded::ENTITY_SET_NOT_FOUND_MESSAGE`, which names neither the member nor the
//! value:
//!
//! * an id no create ever minted;
//! * an id a create minted under ANOTHER detector, in this Region or in another;
//! * an id a delete has taken away, for the DELETE that has nothing left to accept
//!   and again once a replacement has been created;
//! * an id whose set was deleted and then REPLACED by a new create, where the
//!   replacement answers on its own id and the deleted one stays out of the
//!   collection.
//!
//! What this rule does not reach: an id outside the alphabet the member is constrained
//! to is a CONSTRAINT violation decided from the request before anything is resolved
//! (`entity_set_id_refusal_classes.rs` owns that class), so it is asserted here only to
//! show the absence refusal did not swallow it — and the READ of an id whose delete
//! was served is not an absence at all while the record is retained
//! (`entity_set_delete_observability.rs` owns that progression and
//! `entity_set_delete_retention.rs` the bound past which it becomes one).
//!
//! `entity_set_delete_observability.rs` owns what a delete leaves on the id it named
//! and `entity_set_delete_retention.rs` what churn leaves behind; this suite owns the
//! one absence the ways above share.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, call_in, enabled_detector, new_router, Router};
use crate::entity_set_id_refusal_classes::assert_entity_set_id_constraint_violation;
use crate::set_delete_terminality::assert_entity_set_absent;

/// The Region the detector every case works against is created in.
const REGION: &str = "us-east-1";

/// Another Region, whose detector is a store of its own: an entity set hangs off a
/// REGIONAL detector, so an id minted there names no set here.
const OTHER_REGION: &str = "eu-west-1";

/// One entity-set family: the collection segment under `/detector/{DetectorId}`, the
/// member its create reports the minted id in and the member a rejection names.
struct Family {
    case: &'static str,
    segment: &'static str,
    id_member: &'static str,
}

/// Both families, so neither can drift from the shared absence.
const FAMILIES: [Family; 2] = [
    Family {
        case: "threat entity set",
        segment: "threatentityset",
        id_member: "threatEntitySetId",
    },
    Family {
        case: "trusted entity set",
        segment: "trustedentityset",
        id_member: "trustedEntitySetId",
    },
];

/// The create bodies the cases below use, differing along the `*EntitySetFormat` the
/// create stores (members of `ThreatEntitySetFormat`, which the trusted family models
/// identically — `smithy_query(shape="ThreatEntitySetFormat")`), the S3 URL form of the
/// `Location`, whether the caller activated the set, whether it carried an
/// `ExpectedBucketOwner` and how many tags it carried.
fn create_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a TXT set the caller activated",
            json!({
                "name": "absence_probe-1",
                "format": "TXT",
                "location": "s3://guardduty-lists/absence.txt",
                "activate": true,
                "tags": {"team": "secops"},
            }),
        ),
        (
            "an OTX_CSV set on a virtual-hosted object URL",
            json!({
                "name": "a",
                "format": "OTX_CSV",
                "location": "https://guardduty-lists.s3.us-west-2.amazonaws.com/absence.csv",
                "activate": false,
            }),
        ),
        (
            "a PROOF_POINT set with a bucket owner and the longest name the model accepts",
            json!({
                "name": "p".repeat(300),
                "format": "PROOF_POINT",
                "location": "https://s3.amazonaws.com/guardduty-lists/deep/absence.pp",
                "activate": false,
                "expectedBucketOwner": "123456789012",
                "tags": {"team": "secops", "env": "test"},
            }),
        ),
    ]
}

/// Well-formed entity-set ids (32 lowercase hex characters, the form this service
/// mints) that no create ever minted — one all-zero, one all-f and one mixed, so the
/// rule is not about a single value.
const UNMINTED_IDS: [&str; 3] = [
    "00000000000000000000000000000000",
    "ffffffffffffffffffffffffffffffff",
    "0123456789abcdef0123456789abcdef",
];

/// Values outside the alphabet an entity-set id is constrained to (ASCII letters and
/// digits only, `scaffolded::ENTITY_SET_ID_PATTERN`), which keep their own
/// constraint-violation refusal rather than this suite's absence.
const MALFORMED_IDS: [&str; 2] = ["not-a-set-id!", "set_id_with_underscores"];

impl Family {
    fn collection(&self, detector_id: &str) -> String {
        format!("/detector/{detector_id}/{}", self.segment)
    }

    fn resource(&self, detector_id: &str, set_id: &str) -> String {
        format!("{}/{set_id}", self.collection(detector_id))
    }

    /// Create one set in `region`'s detector, answering the id the create minted.
    async fn create_in(
        &self,
        router: &Router,
        region: &str,
        detector_id: &str,
        body: Value,
        case: &str,
    ) -> String {
        let (status, got) = call_in(
            router,
            region,
            "POST",
            &self.collection(detector_id),
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

    async fn get_in(
        &self,
        router: &Router,
        region: &str,
        detector_id: &str,
        set_id: &str,
    ) -> (u16, Value) {
        call_in(
            router,
            region,
            "GET",
            &self.resource(detector_id, set_id),
            None,
        )
        .await
    }

    async fn delete_in(
        &self,
        router: &Router,
        region: &str,
        detector_id: &str,
        set_id: &str,
    ) -> (u16, Value) {
        call_in(
            router,
            region,
            "DELETE",
            &self.resource(detector_id, set_id),
            None,
        )
        .await
    }

    /// Assert both the read and the delete of `set_id` answer the shared absence
    /// refusal: the two operations resolve the same id against the same store, so
    /// neither may admit what the other turns away.
    async fn assert_absent_to_both(
        &self,
        router: &Router,
        detector_id: &str,
        set_id: &str,
        case: &str,
    ) {
        let (status, body) = self.get_in(router, REGION, detector_id, set_id).await;
        assert_entity_set_absent(&format!("{case}: read"), status, &body, set_id);
        let (status, body) = self.delete_in(router, REGION, detector_id, set_id).await;
        assert_entity_set_absent(&format!("{case}: delete"), status, &body, set_id);
    }

    /// Assert the DELETE of `set_id` answers the shared absence refusal, which is
    /// what a second delete of a set the detector no longer holds gets. The read is
    /// not asserted here: while the deleted record is retained it reports the
    /// removal instead (`entity_set_delete_observability.rs`).
    async fn assert_delete_absent(
        &self,
        router: &Router,
        detector_id: &str,
        set_id: &str,
        case: &str,
    ) {
        let (status, body) = self.delete_in(router, REGION, detector_id, set_id).await;
        assert_entity_set_absent(&format!("{case}: delete"), status, &body, set_id);
    }
}

/// A router holding one enabled detector in [`REGION`].
async fn detector() -> (Router, String) {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    (router, detector_id)
}

/// A second detector, in `OTHER_REGION`, so an id minted under it belongs to another
/// store.
async fn other_detector(router: &Router, case: &str) -> String {
    let (status, body) = call_in(
        router,
        OTHER_REGION,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other Region's detector to be created, got {status} {body:?}"
    );
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {body:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// An id no create minted, and an id minted in another Region's store.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_the_detector_holds_no_set_for_is_refused_as_absent() {
    for family in &FAMILIES {
        let case = format!("{}: never minted here", family.case);
        let case = case.as_str();
        let (router, detector_id) = detector().await;
        // A live set of the same family, so every refusal below is about the id in
        // the request rather than about an empty collection.
        let live = family
            .create_in(
                &router,
                REGION,
                &detector_id,
                create_bodies()[0].1.clone(),
                case,
            )
            .await;

        for unminted in UNMINTED_IDS {
            family
                .assert_absent_to_both(&router, &detector_id, unminted, case)
                .await;
        }

        // An id another Region's detector really holds: it is a well-formed id this
        // service minted, and it names no set in this store.
        let elsewhere = other_detector(&router, case).await;
        let theirs = family
            .create_in(
                &router,
                OTHER_REGION,
                &elsewhere,
                create_bodies()[1].1.clone(),
                case,
            )
            .await;
        family
            .assert_absent_to_both(&router, &detector_id, &theirs, case)
            .await;
        // And it still answers where it was minted, so the refusal above is about
        // this store and not about the id.
        let (status, body) = family
            .get_in(&router, OTHER_REGION, &elsewhere, &theirs)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the other Region's set must still read back there, got {status} \
             {body:?}"
        );

        // A value outside the alphabet the member is constrained to keeps its own
        // refusal: the absence has not swallowed the constraint violation.
        for malformed in MALFORMED_IDS {
            let (status, body) = family.get_in(&router, REGION, &detector_id, malformed).await;
            assert_entity_set_id_constraint_violation(
                case,
                status,
                &body,
                family.id_member,
                malformed,
            );
        }

        // None of the refusals touched the set the detector does hold.
        let (status, body) = family.get_in(&router, REGION, &detector_id, &live).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the live set must be untouched, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An id a delete has taken away, and the replacement a later create mints.
// ---------------------------------------------------------------------------

/// The delete-then-replace loop a client runs: delete the set, confirm the detector
/// no longer holds it by asking the DELETE again, then create the replacement and read
/// THAT back. The rows differ along the create body, so the rule holds for every form
/// of set, and the confirmation must reach its absence branch at once rather than
/// after any wait.
#[tokio::test]
async fn a_deleted_set_is_no_longer_held_and_its_replacement_is_a_resource_of_its_own() {
    for family in &FAMILIES {
        for (row, body) in create_bodies() {
            let case = format!("{}: {row}", family.case);
            let case = case.as_str();
            // One store per row: the trusted family keeps one set at a time, so a row
            // cannot share a detector with the next.
            let (router, detector_id) = detector().await;
            let set_id = family
                .create_in(&router, REGION, &detector_id, body.clone(), case)
                .await;
            let (status, created) = family.get_in(&router, REGION, &detector_id, &set_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the created set must be readable, got {status} {created:?}"
            );

            let (status, deleted) = family.delete_in(&router, REGION, &detector_id, &set_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
            );

            // The second delete a cleanup loop would make: the detector no longer
            // holds the set, so there is nothing left to accept.
            family
                .assert_delete_absent(&router, &detector_id, &set_id, case)
                .await;

            // The replacement: a create under the same name is served, answers on an
            // id of its own and reports the location it was created with, while the
            // deleted id stays refused.
            let replacement = family
                .create_in(&router, REGION, &detector_id, body.clone(), case)
                .await;
            assert_ne!(
                replacement, set_id,
                "case {case:?}: the replacement must be a resource of its own, got {set_id} again"
            );
            let (status, read_back) = family
                .get_in(&router, REGION, &detector_id, &replacement)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the replacement must read back, got {status} {read_back:?}"
            );
            assert_eq!(
                read_back["location"], body["location"],
                "case {case:?}: expected the replacement to report the location it was created \
                 with, got {read_back:?}"
            );
            family
                .assert_delete_absent(&router, &detector_id, &set_id, case)
                .await;

            // The detector's collection names the replacement and nothing else of
            // this family, so the deleted id is gone from the listing too.
            let (status, listing) = call(&router, "GET", &family.collection(&detector_id), None)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the listing to answer 200, got {status} {listing:?}"
            );
            let named: Vec<&str> = listing[format!("{}s", family.id_member)]
                .as_array()
                .map(|ids| ids.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            assert_eq!(
                named,
                vec![replacement.as_str()],
                "case {case:?}: the listing must name exactly the replacement, got {named:?}"
            );
        }
    }
}
