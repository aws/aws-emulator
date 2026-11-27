// SPDX-License-Identifier: CC0-1.0

//! What a delete LEAVES BEHIND on the four detector-scoped set families — an
//! `IPSet`, a `ThreatIntelSet`, a `ThreatEntitySet` and a `TrustedEntitySet` — end to
//! end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a TABLE spanning the four families and
//! the status the set was in when the delete arrived, rather than over one call:
//!
//! 1. A delete takes the set off the detector the instant the call is served, and
//!    leaves its record READABLE on its own id reporting the delete member of the
//!    family's status enum (`smithy_query(shape="IpSetStatus")` and its three
//!    siblings all carry `DELETE_PENDING` and `DELETED`), still reporting what the
//!    create stored. Which member comes first is the family's: an uploaded list
//!    settles on the terminal `DELETED` at once, while an entity set's removal is
//!    asynchronous, so it is `DELETE_PENDING` until the clock owes it `DELETED`.
//! 2. An id the detector holds no record for is refused with its family's own
//!    absence refusal, and the SAME one whatever put the id out of reach — a value
//!    no create ever minted, or one whose deleted record the store no longer
//!    retains: HTTP 400 under the `InvalidInputException` class with the family's
//!    one fixed sentence ([`assert_absence_refusal`], and
//!    [`assert_entity_set_absent`] for the entity sets' own sentence) — except that
//!    an id outside the alphabet an
//!    entity-set id is constrained to is refused as a CONSTRAINT violation there,
//!    before anything is resolved.
//! 3. A SECOND delete is that same refusal, and writes nothing: the detector's own
//!    `UpdatedAt` moved on the first delete and does not move again.
//! 4. The collection stops naming a deleted set, and the name it held is free: a
//!    create that reuses it is served and mints a different id, so a client that
//!    deletes and re-creates never collides with a ghost.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, error_code, error_shape, new_router, Router};
use crate::entity_set_id_refusal_classes::assert_entity_set_id_constraint_violation;
use service_com_amazonaws_guardduty_guarddutyapiservice::scaffolded::{
    BAD_REQUEST_EXCEPTION, ENTITY_SET_NOT_FOUND_MESSAGE, INVALID_INPUT_EXCEPTION,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ENTITY_SET_ACTIVE, ENTITY_SET_DELETED, ENTITY_SET_DELETE_PENDING, ENTITY_SET_INACTIVE,
    IP_SET_ACTIVE, IP_SET_DELETED, IP_SET_INACTIVE, NO_SUCH_RESOURCE_MESSAGE,
    THREAT_INTEL_SET_ACTIVE, THREAT_INTEL_SET_DELETED, THREAT_INTEL_SET_INACTIVE,
};

/// A well-formed set id (32 lowercase hex characters, the form this service mints)
/// that no create ever minted.
pub(crate) const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// How long a poll may wait for a status the clock owes before the test gives up.
/// Generous on purpose: every wait below ends on the CONDITION it is waiting for, and
/// this is only the deadline past which the emulator is considered broken.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The gap between two reads of a poll. Only the pause between attempts; no loop
/// below ever ends on it.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// Assert `status`/`body` is the absence refusal BOTH ENTITY-SET families answer an
/// id the detector holds no record for with: the shared client rejection carrying
/// the entity sets' own fixed sentence.
///
/// It is the same CLASS every other family answers
/// ([`assert_absence_refusal`]) and differs only in that sentence:
/// `BadRequestException` is the only client error either family's operations
/// declare (`smithy_query(operation="GetThreatEntitySet")`), so a 404 class is one
/// neither may answer.
pub(crate) fn assert_entity_set_absent(case: &str, status: u16, body: &Value, id: &str) {
    assert_absence_refusal(case, status, body, id, ENTITY_SET_NOT_FOUND_MESSAGE);
}

/// Assert `status`/`body` is the one client rejection this service answers an
/// identifier it holds no resource for with, carrying `expected_message`: HTTP 400,
/// the `InvalidInputException` class in the body's `__type` and its `type` member,
/// the modelled `BadRequestException` envelope in `x-amzn-errortype`, and a fixed
/// sentence that names neither the member nor the caller's value.
pub(crate) fn assert_absence_refusal(
    case: &str,
    status: u16,
    body: &Value,
    id: &str,
    expected_message: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT_EXCEPTION,
        "case {case:?}: expected the {INVALID_INPUT_EXCEPTION} class, got {class:?} {body:?}"
    );
    assert_eq!(
        body["type"].as_str().unwrap_or_default(),
        INVALID_INPUT_EXCEPTION,
        "case {case:?}: expected the class in the body's type member, got {body:?}"
    );
    let shape = error_shape(body);
    assert_eq!(
        shape, BAD_REQUEST_EXCEPTION,
        "case {case:?}: expected the {BAD_REQUEST_EXCEPTION} shape in x-amzn-errortype, got \
         {shape:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {expected_message:?}, got {message:?}"
    );
    assert!(
        !message.contains(id),
        "case {case:?}: the refusal must not echo the id {id:?}, got {message:?}"
    );
}

/// One set family: the route it hangs off a detector, the members its create and its
/// listing report, and what its delete leaves readable.
struct Family {
    case: &'static str,
    /// The URI segment naming the family's collection under
    /// `/detector/{DetectorId}`.
    segment: &'static str,
    /// The response member the create reports the minted id in.
    id_member: &'static str,
    /// The response member the listing reports the family's ids in.
    ids_member: &'static str,
    /// A member of this family's `*Format` enum — each family models the same six
    /// (`smithy_query(shape="IpSetFormat")`), so the row varies it to show the rule
    /// does not depend on the format a set was created with.
    format: &'static str,
    /// The member of this family's own status enum a set GuardDuty is using settles
    /// on.
    active_status: &'static str,
    /// The member of this family's own status enum a set GuardDuty is not using
    /// settles on.
    inactive_status: &'static str,
    /// The `Status` the family's own read reports STRAIGHT AFTER a delete is
    /// served: the terminal member for a family whose removal is synchronous, the
    /// transitional one for a family whose removal the clock settles.
    accepted_delete_status: &'static str,
    /// The `Status` the family's own read settles on once the removal is done.
    settled_delete_status: &'static str,
    /// Whether this family enforces the entity-set id alphabet on the identifier
    /// before resolving it, so a value outside that alphabet is a constraint
    /// violation rather than an absence.
    constrains_id_form: bool,
    /// The fixed sentence this family refuses an identifier it holds no record for
    /// with.
    absence_message: &'static str,
}

/// Every detector-scoped set family. All four RETAIN a readable record when a
/// delete is served; they differ in the sentence their absence refusal carries, in
/// whether the id's form is constrained, and in whether the removal settles at once
/// or through the transitional status.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "ipset",
            segment: "ipset",
            id_member: "ipSetId",
            ids_member: "ipSetIds",
            format: "TXT",
            active_status: IP_SET_ACTIVE,
            inactive_status: IP_SET_INACTIVE,
            accepted_delete_status: IP_SET_DELETED,
            settled_delete_status: IP_SET_DELETED,
            constrains_id_form: false,
            absence_message: NO_SUCH_RESOURCE_MESSAGE,
        },
        Family {
            case: "threatintelset",
            segment: "threatintelset",
            id_member: "threatIntelSetId",
            ids_member: "threatIntelSetIds",
            format: "STIX",
            active_status: THREAT_INTEL_SET_ACTIVE,
            inactive_status: THREAT_INTEL_SET_INACTIVE,
            accepted_delete_status: THREAT_INTEL_SET_DELETED,
            settled_delete_status: THREAT_INTEL_SET_DELETED,
            constrains_id_form: false,
            absence_message: NO_SUCH_RESOURCE_MESSAGE,
        },
        Family {
            case: "threatentityset",
            segment: "threatentityset",
            id_member: "threatEntitySetId",
            ids_member: "threatEntitySetIds",
            // A format no cited case used, to show the rule is indifferent to it.
            format: "ALIEN_VAULT",
            active_status: ENTITY_SET_ACTIVE,
            inactive_status: ENTITY_SET_INACTIVE,
            accepted_delete_status: ENTITY_SET_DELETE_PENDING,
            settled_delete_status: ENTITY_SET_DELETED,
            constrains_id_form: true,
            absence_message: ENTITY_SET_NOT_FOUND_MESSAGE,
        },
        Family {
            case: "trustedentityset",
            segment: "trustedentityset",
            id_member: "trustedEntitySetId",
            ids_member: "trustedEntitySetIds",
            format: "PROOF_POINT",
            active_status: ENTITY_SET_ACTIVE,
            inactive_status: ENTITY_SET_INACTIVE,
            accepted_delete_status: ENTITY_SET_DELETE_PENDING,
            settled_delete_status: ENTITY_SET_DELETED,
            constrains_id_form: true,
            absence_message: ENTITY_SET_NOT_FOUND_MESSAGE,
        },
    ]
}

impl Family {
    /// The settled member of this family's status enum a create that did or did not
    /// ask GuardDuty to use the set ends up on.
    fn settled_status(&self, activate: bool) -> &'static str {
        if activate {
            self.active_status
        } else {
            self.inactive_status
        }
    }

    /// Read `set_id` until its reported `Status` is `want`, or until
    /// [`SETTLE_DEADLINE`] has passed — the CONDITION ends the wait, never a fixed
    /// pause. Answers the body of the read that saw it, or `None` once the service
    /// has stopped holding the record: that read is the family's absence refusal, so
    /// a wait cannot outlive the record it is waiting on.
    async fn read_until_status(
        &self,
        router: &Router,
        detector_id: &str,
        set_id: &str,
        want: &str,
        case: &str,
    ) -> Option<Value> {
        let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let (status, body) = self.get(router, detector_id, set_id).await;
            if status != 200 {
                self.assert_absent(case, status, &body, set_id);
                return None;
            }
            if body["status"].as_str() == Some(want) {
                return Some(body);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {case:?}: {set_id} never reached {want}, last read {body:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    fn assert_absent(&self, case: &str, status: u16, body: &Value, id: &str) {
        assert_absence_refusal(case, status, body, id, self.absence_message);
    }

    /// Assert how this family refuses `id`. An id outside the alphabet an entity-set
    /// id is constrained to is a CONSTRAINT violation on the two entity-set
    /// families, decided from the request before anything is resolved; everything
    /// else, and every form on the uploaded lists, is the family's own absence
    /// refusal.
    fn assert_refused(&self, case: &str, status: u16, body: &Value, id: &str, well_formed: bool) {
        if self.constrains_id_form && !well_formed {
            assert_entity_set_id_constraint_violation(case, status, body, self.id_member, id);
        } else {
            self.assert_absent(case, status, body, id);
        }
    }

    async fn create(&self, router: &Router, detector_id: &str, name: &str, activate: bool) -> (u16, Value) {
        call(
            router,
            "POST",
            &format!("/detector/{detector_id}/{}", self.segment),
            Some(json!({
                "name": name,
                "format": self.format,
                "location": "s3://guardduty-lists/set.txt",
                "activate": activate,
            })),
        )
        .await
    }

    async fn created(&self, router: &Router, detector_id: &str, name: &str, activate: bool) -> String {
        let (status, body) = self.create(router, detector_id, name, activate).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the create to answer 200, got {status} {body:?}",
            self.case
        );
        body[self.id_member]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a {} in {body:?}",
                    self.case, self.id_member
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

    async fn delete(&self, router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
        call(
            router,
            "DELETE",
            &format!("/detector/{detector_id}/{}/{set_id}", self.segment),
            None,
        )
        .await
    }

    async fn list(&self, router: &Router, detector_id: &str) -> Vec<String> {
        let (status, body) = call(
            router,
            "GET",
            &format!("/detector/{detector_id}/{}", self.segment),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the listing to answer 200, got {status} {body:?}",
            self.case
        );
        body[self.ids_member]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .map(|id| id.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The detector's own `UpdatedAt`, as `GetDetector` reports it.
async fn detector_updated_at(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {body:?}"
    );
    body["updatedAt"].clone()
}

/// The statuses a set can be in when the delete arrives. The delete's answer, and
/// what it leaves behind, must not depend on which.
fn arrival_rows() -> Vec<(&'static str, bool, bool)> {
    vec![
        ("a set in use", true, true),
        ("a set stored unused", false, true),
        ("a set still in its create's transition", true, false),
    ]
}

// ---------------------------------------------------------------------------
// Rules 1-3: a delete completes, and the id it deleted is afterwards refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_completes_and_the_deleted_id_is_refused_afterwards() {
    for family in families() {
        for (arrival, activate, settle_first) in arrival_rows() {
            let case = format!("{} {arrival}", family.case);
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let set_id = family
                .created(&router, &detector_id, "deleted_set-1", activate)
                .await;
            if settle_first {
                let settled = family
                    .read_until_status(
                        &router,
                        &detector_id,
                        &set_id,
                        family.settled_status(activate),
                        &case,
                    )
                    .await;
                assert!(
                    settled.is_some(),
                    "case {case:?}: a set nothing deleted must stay readable until it settles"
                );
            }

            let (status, body) = family.delete(&router, &detector_id, &set_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the delete to answer the modelled 200, got {status} \
                 {body:?}"
            );
            let after_first_delete = detector_updated_at(&router, &detector_id, &case).await;

            // Rule 1: the record is still READABLE on its own id, reporting the
            // status the delete put it in and everything the create stored. Which
            // delete member comes first is the family's: a synchronous removal is
            // already terminal, an asynchronous one is transitional until the clock
            // owes it the terminal member.
            let (status, body) = family.get(&router, &detector_id, &set_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a deleted set stays readable while its record is held, got \
                 {status} {body:?}"
            );
            assert_eq!(
                body["status"].as_str().unwrap_or_default(),
                family.accepted_delete_status,
                "case {case:?}: expected {} straight after the delete, got {body:?}",
                family.accepted_delete_status
            );
            // The read reports what the create stored: the delete moved the status
            // and nothing else.
            assert_eq!(
                body["name"].as_str().unwrap_or_default(),
                "deleted_set-1",
                "case {case:?}: a deleted record must still report the stored name, got {body:?}"
            );

            // Rule 3: a second delete is the family's absence refusal, and moves
            // neither the record nor the detector.
            let (status, body) = family.delete(&router, &detector_id, &set_id).await;
            family.assert_absent(&case, status, &body, &set_id);
            let after_second_delete = detector_updated_at(&router, &detector_id, &case).await;
            assert_eq!(
                after_second_delete, after_first_delete,
                "case {case:?}: a refused second delete must not move the detector's updatedAt, \
                 expected {after_first_delete:?}, got {after_second_delete:?}"
            );

            // Rule 1 again: the record settles on the terminal member of its status
            // enum and stays there, still reporting what the create stored — so a
            // later read agrees with the first and nothing walks a deleted set back.
            let settled = family
                .read_until_status(
                    &router,
                    &detector_id,
                    &set_id,
                    family.settled_delete_status,
                    &case,
                )
                .await;
            let settled = settled.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a deleted set must settle on {} while its record is held",
                    family.settled_delete_status
                )
            });
            assert_eq!(
                settled["name"].as_str().unwrap_or_default(),
                "deleted_set-1",
                "case {case:?}: a settled deletion still reports the stored name, got {settled:?}"
            );
            let (status, body) = family.get(&router, &detector_id, &set_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the retained record stays readable, got {status} {body:?}"
            );
            assert_eq!(
                body["status"].as_str().unwrap_or_default(),
                family.settled_delete_status,
                "case {case:?}: {} is terminal, got {body:?}",
                family.settled_delete_status
            );

            // Rule 4: the collection stops naming it.
            let listed = family.list(&router, &detector_id).await;
            assert!(
                !listed.contains(&set_id),
                "case {case:?}: the listing must stop naming the deleted set, got {listed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an id no create ever minted gets the same refusal as a deleted one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_the_detector_never_held_is_refused_the_same_way() {
    for family in families() {
        let case = family.case.to_string();
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        // A live set of the same family, so the refusals below are about the id in
        // the request and not about an empty collection.
        let live = family
            .created(&router, &detector_id, "live_set-1", false)
            .await;

        // The rows differ along the FORM of the id: one this service could have
        // minted, one that is far too short to be one, and one with characters a
        // minted id never carries. The last is outside the alphabet an ENTITY-SET id
        // is constrained to, so those two families answer it with the constraint
        // violation their own suite pins rather than with an absence; the uploaded
        // lists answer their one absence sentence whatever the form.
        for (form, absent, well_formed) in [
            ("well-formed but never minted", ABSENT_SET_ID, true),
            ("too short to be an id", "abc", true),
            ("characters an id never carries", "not-a-set-id!", false),
        ] {
            let row = format!("{case}: {form}");
            let (status, body) = family.get(&router, &detector_id, absent).await;
            family.assert_refused(&row, status, &body, absent, well_formed);

            let (status, body) = family.delete(&router, &detector_id, absent).await;
            family.assert_refused(&row, status, &body, absent, well_formed);

            // The refusals touched nothing: the live set is still there.
            let (status, body) = family.get(&router, &detector_id, &live).await;
            assert_eq!(
                status, 200,
                "case {row:?}: the live set must be untouched, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the name a deleted set held is free again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_sets_name_can_be_created_again() {
    for family in families() {
        let case = family.case.to_string();
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let name = "recreated_set-1";

        let first = family.created(&router, &detector_id, name, true).await;
        let (status, body) = family.delete(&router, &detector_id, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {body:?}"
        );

        // The slot the deleted set held is free, so the same name is accepted again —
        // and it is a DIFFERENT resource, with an id of its own.
        let second = family.created(&router, &detector_id, name, true).await;
        assert_ne!(
            second, first,
            "case {case:?}: a re-create must mint a new id, got {second:?} twice"
        );
        let listed = family.list(&router, &detector_id).await;
        assert!(
            listed.contains(&second) && !listed.contains(&first),
            "case {case:?}: the listing must name only the live set, got {listed:?}"
        );

        let (status, body) = family.get(&router, &detector_id, &second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-created set must be readable, got {status} {body:?}"
        );
        assert_eq!(
            body["name"].as_str().unwrap_or_default(),
            name,
            "case {case:?}: the re-created set must report the name it was created with, got \
             {body:?}"
        );
    }
}
