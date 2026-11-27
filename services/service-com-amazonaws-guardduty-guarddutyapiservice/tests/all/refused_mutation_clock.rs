// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED delete, update or acceptance leaves behind, end to end over the
//! wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test is the one the `refused_create_clock` suite asserts for the
//! creates, here over the SET of mutating operations that act on a resource the
//! request NAMES and over the SET of ways each of them is refused: a mutating
//! operation reads the service's monotonic "this resource changed now" clock only on
//! the path that actually writes, so a refused call changes nothing at all — it
//! neither moves a resource nor consumes an instant — and the `CreatedAt`/`UpdatedAt`
//! a LATER accepted call stamps therefore still reports the wall-clock instant of
//! that change rather than one pushed into the future by the refusals before it.
//!
//! Observable per request: the clock a mutation reads is what the detector's own
//! `UpdatedAt` records when a sub-resource is added to it
//! (`smithy_query(operation="GetDetector")` models it as a String), so a burst of
//! refused calls between two accepted mutations is enough to see whether the
//! refusals consumed instants: the clock may advance across the burst only by the
//! wall-clock time that really passed over the same span, whereas a handler that
//! reads the clock before it refuses spends an instant per refused call on top of
//! that.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, enabled_detector, router_over, ABSENT_DETECTOR_ID,
};
use crate::negative_class_rejections::seed_invitation;
use crate::refused_create_clock::{
    as_millis, detector_updated_at, READING_SLACK_MILLIS, REFUSALS_PER_BURST,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::now_millis;

/// The Region every request of this suite is signed for, which is the Region the
/// detector a refused call names lives in (a detector is a REGIONAL resource).
const REGION: &str = "us-east-1";

/// A well-formed id of the shape this service mints for a detector sub-resource (32
/// lowercase hex characters) that no create ever minted, so every id-scoped mutation
/// below is refused on a value it cannot resolve.
const ABSENT_SUB_RESOURCE_ID: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f0";

/// An account id of an administrator that invited nobody here
/// (`smithy_query(shape="AccountId")`: 12 digits), so an acceptance naming it has no
/// pending invitation to accept.
const UNINVITING_ADMINISTRATOR: &str = "111122223333";

/// The administrator whose invitation the seeded rows put in the detector's inbox.
const INVITING_ADMINISTRATOR: &str = "444455556666";

/// The invitation id the seeded rows put in the inbox, and one that was never issued
/// — the pair `(administrator, invitation)` is what an acceptance has to match.
const PENDING_INVITATION_ID: &str = "c4b3a2f1e0d9c8b7a6958473625140ff";
const UNISSUED_INVITATION_ID: &str = "ffffffffffffffffffffffffffffffff";

/// One row: a mutating operation, one way of refusing it, and an accepted create
/// afterwards whose timestamp shows what the refusals did to the clock.
struct Row {
    case: &'static str,
    /// An invitation to put in the detector's inbox before the burst, as
    /// `(administrator id, invitation id)` — the rows whose refusal is a request that
    /// matches a PENDING invitation only partly.
    seed: Option<(&'static str, &'static str)>,
    /// `true` when the refused calls name a `DetectorId` the store does not hold,
    /// `false` when they are addressed to the real detector and refused by the
    /// sub-resource or the relationship they name.
    absent_detector: bool,
    /// The refused request: its method and the path under `/detector/{detectorId}`,
    /// from each operation's `http` binding
    /// (`smithy_query(operation="DeleteIPSet,UpdateIPSet,DeleteTrustedEntitySet,\
    /// DeleteThreatEntitySet,DeleteThreatIntelSet,DeleteFilter,\
    /// DeletePublishingDestination,AcceptInvitation,AcceptAdministratorInvitation,\
    /// UpdateMalwareScanSettings,UpdateOrganizationConfiguration")`).
    method: &'static str,
    suffix: String,
    body: Option<Value>,
}

/// The HTTP status every refusal in this suite carries: HTTP 400, the one client
/// rejection every family of this service answers — for a sub-resource id the
/// detector does not hold as much as for a `DetectorId` the store does not hold. The
/// clock rule is the same whichever family is addressed, which is what this suite
/// asserts.
const REFUSED_STATUS: u16 = 400;

/// Every detector-scoped mutation that acts on something the request NAMES, refused
/// in each of the ways these handlers refuse one: a `DetectorId` the store does not
/// hold, a sub-resource id or filter name the detector does not hold, a value that
/// cannot be one of this service's ids at all, and an invitation that is not pending
/// under the pair the request carries. The rows differ along the operation, the
/// resource family, the HTTP method and what makes the call unacceptable.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "delete ip set, an ipSetId the detector does not hold",
            seed: None,
            absent_detector: false,
            method: "DELETE",
            suffix: format!("ipset/{ABSENT_SUB_RESOURCE_ID}"),
            body: None,
        },
        Row {
            case: "delete ip set, a DetectorId the store does not hold",
            seed: None,
            absent_detector: true,
            method: "DELETE",
            suffix: format!("ipset/{ABSENT_SUB_RESOURCE_ID}"),
            body: None,
        },
        Row {
            case: "delete ip set, a value that cannot be one of this service's ids",
            seed: None,
            absent_detector: false,
            method: "DELETE",
            suffix: "ipset/not-an-ip-set-id".to_string(),
            body: None,
        },
        Row {
            case: "update ip set, an ipSetId the detector does not hold",
            seed: None,
            absent_detector: false,
            method: "POST",
            suffix: format!("ipset/{ABSENT_SUB_RESOURCE_ID}"),
            body: Some(json!({"name": "renamed-ip-set", "activate": true})),
        },
        Row {
            case: "delete trusted entity set, an id the detector does not hold",
            seed: None,
            absent_detector: false,
            method: "DELETE",
            suffix: format!("trustedentityset/{ABSENT_SUB_RESOURCE_ID}"),
            body: None,
        },
        Row {
            case: "delete threat entity set, an id the detector does not hold",
            seed: None,
            absent_detector: false,
            method: "DELETE",
            suffix: format!("threatentityset/{ABSENT_SUB_RESOURCE_ID}"),
            body: None,
        },
        Row {
            // The sibling that already classified its refusals before reading the
            // clock: a row of the same table, so the rule is asserted for it too.
            case: "delete threat intel set, an id the detector does not hold",
            seed: None,
            absent_detector: false,
            method: "DELETE",
            suffix: format!("threatintelset/{ABSENT_SUB_RESOURCE_ID}"),
            body: None,
        },
        Row {
            case: "delete filter, a filterName the detector does not hold",
            seed: None,
            absent_detector: false,
            method: "DELETE",
            suffix: "filter/absent-filter".to_string(),
            body: None,
        },
        Row {
            case: "delete publishing destination, a destinationId the detector does not hold",
            seed: None,
            absent_detector: false,
            method: "DELETE",
            suffix: format!("publishingDestination/{ABSENT_SUB_RESOURCE_ID}"),
            body: None,
        },
        Row {
            case: "accept invitation, with nothing in the detector's inbox",
            seed: None,
            absent_detector: false,
            method: "POST",
            suffix: "master".to_string(),
            body: Some(json!({
                "masterId": UNINVITING_ADMINISTRATOR,
                "invitationId": UNISSUED_INVITATION_ID,
            })),
        },
        Row {
            case: "accept invitation, the pending administrator with an invitationId it never issued",
            seed: Some((INVITING_ADMINISTRATOR, PENDING_INVITATION_ID)),
            absent_detector: false,
            method: "POST",
            suffix: "master".to_string(),
            body: Some(json!({
                "masterId": INVITING_ADMINISTRATOR,
                "invitationId": UNISSUED_INVITATION_ID,
            })),
        },
        Row {
            case: "accept invitation, a DetectorId the store does not hold",
            seed: None,
            absent_detector: true,
            method: "POST",
            suffix: "master".to_string(),
            body: Some(json!({
                "masterId": UNINVITING_ADMINISTRATOR,
                "invitationId": UNISSUED_INVITATION_ID,
            })),
        },
        Row {
            case: "accept administrator invitation, with nothing in the detector's inbox",
            seed: None,
            absent_detector: false,
            method: "POST",
            suffix: "administrator".to_string(),
            body: Some(json!({
                "administratorId": UNINVITING_ADMINISTRATOR,
                "invitationId": UNISSUED_INVITATION_ID,
            })),
        },
        Row {
            case: "accept administrator invitation, an administrator the pending invitation is not from",
            seed: Some((INVITING_ADMINISTRATOR, PENDING_INVITATION_ID)),
            absent_detector: false,
            method: "POST",
            suffix: "administrator".to_string(),
            body: Some(json!({
                "administratorId": UNINVITING_ADMINISTRATOR,
                "invitationId": PENDING_INVITATION_ID,
            })),
        },
        Row {
            case: "update malware scan settings, a DetectorId the store does not hold",
            seed: None,
            absent_detector: true,
            method: "POST",
            suffix: "malware-scan-settings".to_string(),
            // `EbsSnapshotPreservation` declares NO_RETENTION and
            // RETENTION_WITH_FINDING (`smithy_query(shape="EbsSnapshotPreservation")`).
            body: Some(json!({"ebsSnapshotPreservation": "RETENTION_WITH_FINDING"})),
        },
        Row {
            case: "update organization configuration, a DetectorId the store does not hold",
            seed: None,
            absent_detector: true,
            method: "POST",
            suffix: "admin".to_string(),
            // `AutoEnableMembers` declares NEW, ALL and NONE
            // (`smithy_query(shape="AutoEnableMembers")`), and exactly one of the two
            // auto-enablement views may be given.
            body: Some(json!({"autoEnableOrganizationMembers": "ALL"})),
        },
    ]
}

// ---------------------------------------------------------------------------
// The rule: a refused mutation consumes no instant, so the next accepted call's
// timestamp is still a wall-clock instant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refused_mutations_leave_the_clock_where_the_wall_clock_is() {
    for row in rows() {
        let case = row.case;
        // A router, an engine and a detector per row, so a row cannot pass or fail
        // on another row's writes.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector(&router).await;
        if let Some((administrator_id, invitation_id)) = row.seed {
            // No request this single-account emulator can answer delivers an
            // invitation to its own inbox, so the pending one is written through this
            // service's own state module — the store the handlers read.
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                administrator_id,
                invitation_id,
                now_millis(),
            );
        }

        let addressed = if row.absent_detector {
            ABSENT_DETECTOR_ID.to_string()
        } else {
            detector_id.clone()
        };
        let refused_path = format!("/detector/{addressed}/{}", row.suffix);

        // The instant the last ACCEPTED mutation stamped, and the wall clock just
        // after it: between these two readings and the accepted create below, the
        // only calls the service sees are refused ones, so the clock may advance
        // only by the time that really passes.
        let before_burst = detector_updated_at(&router, &detector_id, case).await;
        let wall_before = now_millis();

        for attempt in 0..REFUSALS_PER_BURST {
            let (status, refusal) =
                call(&router, row.method, &refused_path, row.body.clone()).await;
            assert_eq!(
                status, REFUSED_STATUS,
                "case {case:?}: refused call {attempt} must answer {REFUSED_STATUS}, got {status} \
                 {refusal:?}"
            );
        }

        // A refused mutation changes nothing, which the detector that was supposed to
        // be left alone reports first of all.
        let after_burst = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_burst, before_burst,
            "case {case:?}: {REFUSALS_PER_BURST} refused calls must not move the detector's \
             updatedAt, expected {before_burst:?}, got {after_burst:?}"
        );

        // A refused acceptance consumed no invitation either: the one in the inbox is
        // still pending, so the refusal left the relationship state where it was.
        if row.seed.is_some() {
            let (status, counted) = call(&router, "GET", "/invitation/count", None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the invitations count to answer 200, got {status} \
                 {counted:?}"
            );
            assert_eq!(
                counted["invitationsCount"].as_i64(),
                Some(1),
                "case {case:?}: a refused acceptance must leave the invitation pending, got \
                 {counted:?}"
            );
        }

        // The accepted create right after the burst: it writes, so it stamps the
        // instant it happened — and that instant is a WALL-CLOCK instant, not one the
        // refusals pushed into the future.
        let probe_collection = format!("/detector/{detector_id}/ipset");
        let (status, created) = call(
            &router,
            "POST",
            &probe_collection,
            Some(json!({
                "name": "probe-ip-set",
                "format": "TXT",
                "location": "s3://guardduty-lists/probe-ip-set.txt",
                "activate": false,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the probing create to answer 200, got {status} {created:?}"
        );
        let minted = created["ipSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ipSetId, got {created:?}"))
            .to_string();

        let wall_after = now_millis();
        let stamped = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            stamped > after_burst,
            "case {case:?}: the accepted create changed the detector, so its updatedAt must move \
             past {after_burst:?}, got {stamped:?}"
        );
        // The measurement: how far the clock moved across the burst, against how far
        // the WALL clock moved across the same span. A refused mutation consumes no
        // instant, so the two agree to within the slack above however long the burst
        // took — where a handler that read the clock before refusing would have spent
        // an instant per refused call on top of it.
        let elapsed = wall_after - wall_before;
        let advance = as_millis(&stamped, case) - as_millis(&before_burst, case);
        assert!(
            advance <= elapsed + READING_SLACK_MILLIS,
            "case {case:?}: {REFUSALS_PER_BURST} refused calls consume no instant, so the clock \
             may advance only by the {elapsed} ms that really passed (plus \
             {READING_SLACK_MILLIS} ms of slack), got {advance} ms (from {before_burst:?} to \
             {stamped:?})"
        );

        // Round trip: the IPSet the probing create minted is one a read resolves, so
        // the timestamp asserted above records a change that really landed.
        let (status, read_back) =
            call(&router, "GET", &format!("{probe_collection}/{minted}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of the minted ipSetId to answer 200, got {status} \
             {read_back:?}"
        );
    }
}
