// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a detector's own `UpdatedAt` records when a sub-resource
//! is taken OFF it, asserted uniformly over every family a detector holds.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test: a call that changes WHICH sub-resources a detector holds
//! changes the detector record itself, so `GetDetector`'s `UpdatedAt` moves to the
//! instant of that call — and a call that leaves that set alone (a refused delete, a
//! repeated delete of something already gone, a read) moves it nowhere. The answer
//! does not depend on WHICH collection the sub-resource lives in, nor on whether its
//! create minted an id or echoed the caller's name, nor on whether the family's delete
//! drops the record at once or retains it in a terminal `DELETED` status: one rule
//! covers them all.
//!
//! `detector_sub_resource_creates` asserts the other half of the same rule over the
//! families whose create mints an id, and `filter_detector_timestamp` covers the
//! filter family's create. This suite is the DELETE half over the whole class, so no
//! family can answer this question differently from its siblings.
//!
//! The rows differ along what the rule generalises over: the family and its collection
//! segment, how the delete addresses the sub-resource (a minted id or the caller's
//! name), the `Format` the create carried, whether the caller asked to activate,
//! whether tags were carried, and which status the family's own absence refusal
//! answers. Segments, id members and formats come from
//! `smithy_query(operation="CreateFilter,CreateIPSet,CreateThreatIntelSet,\
//! CreateThreatEntitySet,CreateTrustedEntitySet,CreatePublishingDestination")` and
//! `smithy_query(shape="IpSetFormat,ThreatIntelSetFormat,DestinationType")`.
//!
//! `UpdatedAt` is an ISO-8601 string with a fixed-width millisecond field
//! (`smithy_query(operation="GetDetector")` models it as a String), so these
//! assertions order the instants as text, which is all a client has.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};

/// A KMS key ARN, the form `DestinationProperties.KmsKeyArn` carries
/// (`smithy_query(shape="DestinationProperties")`).
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// How a family's delete addresses the sub-resource its create saved: the create
/// either mints an id of its own or echoes the name the caller chose, and the delete
/// takes whichever one back as its URI label.
enum Address {
    /// The create minted the label and returned it under this output member.
    MintedId(&'static str),
    /// The create echoed the caller's label under this output member.
    EchoedName(&'static str),
}

/// One family of sub-resource that hangs off a detector.
struct Family {
    case: &'static str,
    /// The collection segment under `/detector/{detectorId}`, from each operation's
    /// `http` binding.
    segment: &'static str,
    /// A create this service accepts.
    create: Value,
    /// Where the delete's URI label comes from.
    address: Address,
    /// The HTTP status a delete answers for a label this detector does not hold —
    /// one no create saved, and one a delete has already taken off the detector,
    /// which are the same absence: HTTP 400 under `InvalidInputException`, the one
    /// client rejection every family of this service answers. Either way the second
    /// call changes which sub-resources the detector holds not at all.
    absence_status: u16,
    /// A well-formed label of this family that no create here ever saved, so a delete
    /// naming it is refused and removes nothing.
    absent_label: &'static str,
}

/// Every sub-resource family whose create AND delete are both implemented, so the rule
/// is asserted on the whole class rather than on the one family a report cited.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "filter, addressed by the caller's name, a repeated delete refused",
            segment: "filter",
            // `Name` is modelled length 3..64 and `FindingCriteria` is required
            // (`smithy_query(operation="CreateFilter")`).
            create: json!({
                "name": "deletes.move-the_detector",
                "action": "ARCHIVE",
                "rank": 20,
                "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 5}}},
            }),
            address: Address::EchoedName("name"),
            absence_status: 400,
            absent_label: "never.saved-filter_name",
        },
        Family {
            case: "ip set, TXT, activated, a repeated delete refused",
            segment: "ipset",
            create: json!({
                "name": "deletes-ip-set",
                "format": "TXT",
                "location": "s3://guardduty-lists/deletes-ip-set.txt",
                "activate": true,
            }),
            address: Address::MintedId("ipSetId"),
            absence_status: 400,
            absent_label: "deadbeefdeadbeefdeadbeefdeadbeef",
        },
        Family {
            case: "threat intel set, STIX, not activated, tagged, a repeated delete refused",
            segment: "threatintelset",
            create: json!({
                "name": "deletes-threat-list",
                "format": "STIX",
                "location": "s3://guardduty-lists/deletes-threat-list.xml",
                "activate": false,
                "tags": {"team": "secops"},
            }),
            address: Address::MintedId("threatIntelSetId"),
            absence_status: 400,
            absent_label: "feedfacefeedfacefeedfacefeedface",
        },
        Family {
            case: "threat entity set, OTX_CSV, activated, a repeated delete refused",
            segment: "threatentityset",
            create: json!({
                "name": "deletes-threat-entities",
                "format": "OTX_CSV",
                "location": "s3://guardduty-lists/deletes-threat-entities.csv",
                "activate": true,
            }),
            address: Address::MintedId("threatEntitySetId"),
            absence_status: 400,
            absent_label: "0123456789abcdef0123456789abcdef",
        },
        Family {
            case: "trusted entity set, FIRE_EYE, not activated, tagged, a repeated delete \
                   refused",
            segment: "trustedentityset",
            create: json!({
                "name": "deletes-trusted-entities",
                "format": "FIRE_EYE",
                "location": "s3://guardduty-lists/deletes-trusted-entities.txt",
                "activate": false,
                "tags": {"env": "test", "owner": "team-a"},
            }),
            address: Address::MintedId("trustedEntitySetId"),
            absence_status: 400,
            absent_label: "fedcba9876543210fedcba9876543210",
        },
        Family {
            case: "publishing destination, S3, a repeated delete refused",
            segment: "publishingDestination",
            create: json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings/exports/",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
            address: Address::MintedId("destinationId"),
            absence_status: 400,
            absent_label: "abcdef0123456789abcdef0123456789",
        },
    ]
}

/// `GetDetector`, so an assertion can see the detector's own timestamps.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

/// The detector's `UpdatedAt` as the string a client sees.
async fn detector_updated_at(router: &Router, detector_id: &str, case: &str) -> String {
    let got = read_detector(router, detector_id, case).await;
    got["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the detector's updatedAt, got {got:?}"))
        .to_string()
}

/// Save one sub-resource of `family` under `detector_id` and return the label its
/// delete addresses it by.
async fn save(router: &Router, detector_id: &str, family: &Family) -> String {
    let case = family.case;
    let collection = format!("/detector/{detector_id}/{}", family.segment);
    let (status, created) = call(router, "POST", &collection, Some(family.create.clone())).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to answer 200, got {status} {created:?}"
    );
    let member = match family.address {
        Address::MintedId(member) | Address::EchoedName(member) => member,
    };
    let label = created[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {created:?}"))
        .to_string();
    match family.address {
        Address::MintedId(_) => assert!(
            label.len() == 32 && label.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected a 32-character hexadecimal {member}, got {label:?}"
        ),
        Address::EchoedName(_) => {
            let asked = family.create[member].as_str().unwrap_or_default();
            assert_eq!(
                label, asked,
                "case {case:?}: expected the create to answer with the {member} it was asked \
                 for, {asked:?}, got {label:?}"
            );
        }
    }

    // Round trip: the label the create answered with is a sub-resource a read resolves,
    // so what the detector's moved UpdatedAt records really landed in the store.
    let (status, read_back) = call(router, "GET", &format!("{collection}/{label}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a read of the saved sub-resource to answer 200, got {status} \
         {read_back:?}"
    );
    label
}

// ---------------------------------------------------------------------------
// The rule, over every sub-resource family: an accepted delete moves the detector's
// UpdatedAt, and a call that takes nothing off it moves nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn taking_any_sub_resource_off_a_detector_moves_its_updated_at() {
    for family in families() {
        let case = family.case;
        // A router and a detector per row, so a row cannot pass on another row's
        // write.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let collection = format!("/detector/{detector_id}/{}", family.segment);

        let before = read_detector(&router, &detector_id, case).await;
        let created_at = before["createdAt"].clone();
        let updated_before = before["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {before:?}"))
            .to_string();

        // A delete naming something the detector never held removes nothing, so it
        // moves nothing.
        let (status, refusal) = call(
            &router,
            "DELETE",
            &format!("{collection}/{}", family.absent_label),
            None,
        )
        .await;
        assert_eq!(
            status, family.absence_status,
            "case {case:?}: a delete of a sub-resource the detector does not hold must answer \
             {}, got {status} {refusal:?}",
            family.absence_status
        );
        let after_refusal = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_refusal, updated_before,
            "case {case:?}: a refused delete must not move the detector's updatedAt, expected \
             {updated_before:?}, got {after_refusal:?}"
        );

        let label = save(&router, &detector_id, &family).await;
        let after_create = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            after_create > updated_before,
            "case {case:?}: the create changed the detector, so its updatedAt must move past \
             {updated_before:?}, got {after_create:?}"
        );

        // The accepted delete: the detector holds one fewer sub-resource, so its
        // UpdatedAt moves.
        let (status, deleted) = call(&router, "DELETE", &format!("{collection}/{label}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
        );
        let settled = read_detector(&router, &detector_id, case).await;
        let after_delete = settled["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {settled:?}"))
            .to_string();
        assert!(
            after_delete > after_create,
            "case {case:?}: the delete changed the detector, so its updatedAt must move past \
             {after_create:?}, got {after_delete:?}"
        );
        assert_eq!(
            settled["createdAt"], created_at,
            "case {case:?}: taking a sub-resource off a detector must not move its createdAt, \
             expected {created_at:?}, got {:?}",
            settled["createdAt"]
        );

        // A second delete of the same label takes nothing off the detector — the
        // label names nothing the detector holds any more, so it is refused exactly
        // like one no create ever saved — so it moves nothing.
        let (status, again) = call(&router, "DELETE", &format!("{collection}/{label}"), None).await;
        assert_eq!(
            status, family.absence_status,
            "case {case:?}: expected a repeated delete to answer {}, got {status} {again:?}",
            family.absence_status
        );
        let after_repeat = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_repeat, after_delete,
            "case {case:?}: a repeated delete takes nothing off the detector, so its updatedAt \
             must stay at {after_delete:?}, got {after_repeat:?}"
        );

        // A read is not a change either.
        let after_read = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_read, after_delete,
            "case {case:?}: reading the detector must not move its updatedAt, expected \
             {after_delete:?}, got {after_read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// One detector holding one of every family: each delete moves the instant on again,
// so the answer does not depend on which collection the sub-resource lived in.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_family_taken_off_one_detector_moves_the_instant_again() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let families = families();
    let mut labels: Vec<(&'static str, &'static str, String)> = Vec::new();
    for family in &families {
        let label = save(&router, &detector_id, family).await;
        labels.push((family.case, family.segment, label));
    }

    let mut previous =
        detector_updated_at(&router, &detector_id, "the detector holding one of each").await;
    for (case, segment, label) in labels {
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/detector/{detector_id}/{segment}/{label}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
        );
        let now = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            now > previous,
            "case {case:?}: each sub-resource taken off the detector must move its updatedAt \
             past {previous:?}, got {now:?}"
        );
        previous = now;
    }
}
