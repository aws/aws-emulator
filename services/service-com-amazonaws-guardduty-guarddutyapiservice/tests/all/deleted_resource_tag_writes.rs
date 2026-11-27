// SPDX-License-Identifier: CC0-1.0

//! `TagResource` and `UntagResource` against a resource whose delete has already
//! been ACCEPTED, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together. The resources come from the `resource_tag_removal` fixture, so this
//! suite and the two that drive the same operations against LIVE resources cannot
//! disagree about which ARN names which resource.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A resource whose delete this service has served can no longer be ACTED on:
//!    every write against it is refused exactly like an id the detector never held
//!    — HTTP 400 under the `InvalidInputException` class, with the message a
//!    well-formed ARN this endpoint resolves nothing for gets, which names the
//!    member and echoes nothing of the caller's. That holds whether the record was
//!    dropped by the delete or RETAINED in a deletion status, and
//!    whether or not any span this service opens has elapsed.
//! 2. Reporting follows the record: a class whose delete RETAINS it still reports it
//!    under the status its deletion moved it to and still reports its tags, a
//!    class whose delete DROPS it reports neither, and a refused write moves no tag
//!    map either way.
//! 3. The refusal is scoped to the deleted record: every resource still live accepts
//!    both writes and reports the result.

use serde_json::Value;

use crate::detector_lifecycle::{call_in, error_code, new_router, Router};
use crate::resource_tag_addition::tag_in;
use crate::resource_tag_removal::{tag_map, tagged_resources, tags_of, untag_in, uri_label, Tagged};
use service_com_amazonaws_guardduty_guarddutyapiservice::service::refusals::INVALID_RESOURCE_ARN_MESSAGE;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ENTITY_SET_DELETED, ENTITY_SET_DELETE_PENDING, IP_SET_DELETED, TAGGED_SUB_RESOURCE_SEGMENTS,
    THREAT_INTEL_SET_DELETED,
};

/// The Region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The tags every resource in this suite is created with, so a refused write is
/// observable as "this map did not move".
const BASE_TAGS: [(&str, &str); 2] = [("Env", "prod"), ("Reviewed", "")];

/// `BASE_TAGS` as a request/response `TagMap`.
fn base_tags() -> Value {
    tag_map(&BASE_TAGS)
}

/// How long a poll may wait for the clock to owe a retained record its settled
/// `DELETED` before the test gives up. Generous on purpose: the wait below ends on
/// the CONDITION, and this is only the deadline past which the emulator is considered
/// broken.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The gap between two reads of the poll below. Only the pause between attempts; the
/// loop ends on the status it is waiting for, never on this.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// What one class's own read answers once its delete has been served.
enum OwnRead {
    /// The record is RETAINED and still reported: the `Status` its read answers
    /// straight after the delete, and the terminal one its removal settles on — the
    /// same member for a family whose removal is synchronous, and the transitional
    /// one first for a family whose removal the clock settles.
    Reported {
        accepted: &'static str,
        terminal: &'static str,
    },
    /// The record is no longer one the detector holds, so its own read refuses with
    /// this HTTP status: the 400 of this service's classified rejection for the
    /// detector, a filter and a publishing destination.
    Refused(u16),
}

/// One class of resource a tagging ARN can name, and what its delete leaves behind.
struct Class {
    case: &'static str,
    /// The URI segment naming the collection it lives in, empty for the detector
    /// itself. One per entry of `state::TAGGED_SUB_RESOURCE_SEGMENTS`, which
    /// `every_tagged_class_is_covered` pins.
    segment: &'static str,
    /// What its OWN read answers after the delete.
    own_read: OwnRead,
    /// Whether the tag REPORT still resolves it. The tagging seam reports the tags of
    /// a record the store retains, which is every class whose delete does not DROP
    /// the record — so it is reported for the uploaded lists and the entity sets,
    /// whose own reads report the deleted record too, and for nothing else.
    tags_still_reported: bool,
}

/// Every class a tagging ARN names. A filter, a publishing destination and the
/// detector itself leave nothing behind: their delete drops the record.
const CLASSES: [Class; 7] = [
    Class {
        case: "the detector itself",
        segment: "",
        own_read: OwnRead::Refused(400),
        tags_still_reported: false,
    },
    Class {
        case: "a finding filter",
        segment: "filter",
        own_read: OwnRead::Refused(400),
        tags_still_reported: false,
    },
    Class {
        case: "an IPSet",
        segment: "ipset",
        own_read: OwnRead::Reported {
            accepted: IP_SET_DELETED,
            terminal: IP_SET_DELETED,
        },
        tags_still_reported: true,
    },
    Class {
        case: "a ThreatIntelSet",
        segment: "threatintelset",
        own_read: OwnRead::Reported {
            accepted: THREAT_INTEL_SET_DELETED,
            terminal: THREAT_INTEL_SET_DELETED,
        },
        tags_still_reported: true,
    },
    Class {
        case: "a trusted entity set",
        segment: "trustedentityset",
        // An entity set's removal is asynchronous, so its retained record reports
        // the transitional status until the clock owes it the terminal one.
        own_read: OwnRead::Reported {
            accepted: ENTITY_SET_DELETE_PENDING,
            terminal: ENTITY_SET_DELETED,
        },
        tags_still_reported: true,
    },
    Class {
        case: "a threat entity set",
        segment: "threatentityset",
        own_read: OwnRead::Reported {
            accepted: ENTITY_SET_DELETE_PENDING,
            terminal: ENTITY_SET_DELETED,
        },
        tags_still_reported: true,
    },
    Class {
        case: "a publishing destination",
        segment: "publishingDestination",
        own_read: OwnRead::Refused(400),
        tags_still_reported: false,
    },
];

/// The fixture resource `class` names.
fn resource_of<'a>(resources: &'a [Tagged], class: &Class) -> &'a Tagged {
    let found = resources.iter().find(|resource| {
        if class.segment.is_empty() {
            // The detector's own ARN path is `detector/{DetectorId}` and nothing else.
            resource.path.split('/').count() == 2
        } else {
            resource.path.contains(&format!("/{}/", class.segment))
        }
    });
    found.unwrap_or_else(|| {
        panic!(
            "case {:?}: the fixture must create one resource of every class",
            class.case
        )
    })
}

/// The resource's own read: `DELETE`/`GET` of a tagged resource is bound to the very
/// path its ARN carries, so the ARN resource path doubles as both URIs.
async fn read(router: &Router, resource: &Tagged) -> (u16, Value) {
    call_in(router, REGION, "GET", &resource.read_path(), None).await
}

/// Accept the deletion of the resource `resource` names, asserting the service took it.
async fn delete(router: &Router, resource: &Tagged, case: &str) {
    let (status, body) = call_in(router, REGION, "DELETE", &resource.read_path(), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to be accepted with 200, got {status} {body:?}"
    );
}

/// `ListTagsForResource`, bound to `GET /tags/{ResourceArn}` with a NON-GREEDY URI
/// label.
async fn list_tags(router: &Router, arn: &str) -> (u16, Value) {
    call_in(router, REGION, "GET", &format!("/tags/{}", uri_label(arn)), None).await
}

/// Read `resource` until its own read reports `want`, or until [`SETTLE_DEADLINE`]
/// passes — the CONDITION ends the wait, never a fixed pause. Answers `false` once the
/// service has stopped holding the record, which is how a deletion with a span ends.
async fn read_until_status(router: &Router, resource: &Tagged, want: &str, case: &str) -> bool {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = read(router, resource).await;
        if status != 200 {
            return false;
        }
        if body["status"].as_str() == Some(want) {
            return true;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the deleted record never reached {want}, last read {body:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert one write answered the refusal a well-formed ARN this endpoint resolves
/// nothing for gets.
fn assert_refused(case: &str, arn: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let expected = INVALID_RESOURCE_ARN_MESSAGE;
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(arn),
        "case {case:?}: the refusal must not echo {arn:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Rules 1 + 2: both writes are refused on a sub-resource whose delete was
// accepted, in either deletion status, and the report of its tags does not move.
// ---------------------------------------------------------------------------

/// The sub-resource classes, deleted together in one store: they are independent of
/// each other, so one fixture covers every class and one wait covers every span.
///
/// `settled` chooses WHEN the writes arrive: `false` straight after the delete,
/// `true` once every span this service opens would have come due — the answer is the
/// same either way, which is the point.
async fn refuses_every_write_on_a_deleted_sub_resource(settled: bool) {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
    let phase = if settled {
        "once every span would have come due"
    } else {
        "straight after the delete"
    };

    let sub_resources: Vec<&Class> = CLASSES
        .iter()
        .filter(|class| !class.segment.is_empty())
        .collect();
    for class in &sub_resources {
        let resource = resource_of(&resources, class);
        delete(&router, resource, class.case).await;
    }
    if settled {
        // Wait for the CONDITION each retained record's deletion settles on, rather
        // than for a span: every class whose record is reported publishes the
        // terminal status it ends on.
        for class in &sub_resources {
            if let OwnRead::Reported { terminal, .. } = &class.own_read {
                read_until_status(&router, resource_of(&resources, class), terminal, class.case)
                    .await;
            }
        }
    }

    let untouched = base_tags();
    for class in &sub_resources {
        let resource = resource_of(&resources, class);
        let arn = resource.arn(REGION);
        let case = format!("{} {phase}", class.case);

        // TagResource.
        let (status, body) = tag_in(&router, REGION, &arn, &tag_map(&[("Owner", "me")])).await;
        assert_refused(&format!("tagging {case}"), &arn, status, &body);

        // UntagResource, against a key the resource really carries — so an accepted
        // call would be observable rather than a no-op.
        let (status, body) = untag_in(&router, REGION, &arn, &["Env"]).await;
        assert_refused(&format!("untagging {case}"), &arn, status, &body);

        // Its own read: the families whose delete RETAINS a readable record report
        // the status their removal is in, and the classes whose delete DROPS the
        // record refuse the id with this service's classified rejection — the
        // detector, a filter and a publishing destination.
        let (status, body) = read(&router, resource).await;
        match &class.own_read {
            OwnRead::Reported { accepted, terminal } => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a record this family retains must stay reported, got {status} \
                     {body:?}"
                );
                let actual_status = body["status"].as_str().unwrap_or_default();
                // Once every span has come due only the terminal status may be
                // reported; straight after the delete either member of the
                // removal is correct, because the clock settles it on its own.
                let allowed: &[&str] = if settled {
                    &[terminal]
                } else {
                    &[accepted, terminal]
                };
                assert!(
                    allowed.contains(&actual_status),
                    "case {case:?}: expected one of {allowed:?}, got {actual_status:?} {body:?}"
                );
            }
            OwnRead::Refused(expected_status) => assert_eq!(
                status, *expected_status,
                "case {case:?}: a record its delete took off the detector must refuse its own \
                 read with {expected_status}, got {status} {body:?}"
            ),
        }

        // The refused writes left its tag map exactly as the create stored it,
        // wherever that map is still reported. A record the service has stopped
        // holding reports no tags either, for the same reason its own read does not
        // report it.
        let (status, body) = list_tags(&router, &arn).await;
        if class.tags_still_reported {
            assert_eq!(
                status, 200,
                "case {case:?}: a retained record's tags are still reported, got {status} {body:?}"
            );
            let listed = body.get("tags").cloned().unwrap_or(Value::Null);
            assert_eq!(
                listed, untouched,
                "case {case:?}: expected the reported tags {untouched:?}, got {listed:?}"
            );
            let actual = tags_of(&router, REGION, resource, &case).await;
            assert_eq!(
                actual, untouched,
                "case {case:?}: a refused write must leave the tags as they were, expected \
                 {untouched:?}, got {actual:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a record the service no longer holds has no tags to report, got \
                 {status} {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_write_to_a_sub_resource_with_a_pending_deletion_is_refused() {
    refuses_every_write_on_a_deleted_sub_resource(false).await;
}

#[tokio::test]
async fn a_write_to_a_sub_resource_whose_deletion_settled_is_refused() {
    refuses_every_write_on_a_deleted_sub_resource(true).await;
}

// ---------------------------------------------------------------------------
// Rule 1, for the detector: its delete takes everything hanging off it, so every
// ARN under it resolves nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_to_any_resource_of_a_deleted_detector_is_refused() {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    let detector_class = CLASSES
        .iter()
        .find(|class| class.segment.is_empty())
        .expect("the detector is one of the classes");
    delete(
        &router,
        resource_of(&resources, detector_class),
        detector_class.case,
    )
    .await;

    for class in &CLASSES {
        let resource = resource_of(&resources, class);
        let arn = resource.arn(REGION);
        let case = format!("{} of a deleted detector", class.case);

        let (status, body) = tag_in(&router, REGION, &arn, &tag_map(&[("Owner", "me")])).await;
        assert_refused(&format!("tagging {case}"), &arn, status, &body);

        let (status, body) = untag_in(&router, REGION, &arn, &["Env"]).await;
        assert_refused(&format!("untagging {case}"), &arn, status, &body);
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the refusal reaches the deleted record only.
// ---------------------------------------------------------------------------

/// One sub-resource is deleted; every OTHER resource of the same detector still
/// takes both writes and reports the result. The rows differ along WHICH class was
/// deleted, so no class's refusal can leak onto its siblings.
#[tokio::test]
async fn every_live_resource_still_takes_both_writes() {
    for deleted in CLASSES.iter().filter(|class| !class.segment.is_empty()) {
        // One store per row, so what a read reports is this row's work.
        let router = new_router();
        let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
        let target = resource_of(&resources, deleted);
        delete(&router, target, deleted.case).await;

        for resource in &resources {
            if resource.path == target.path {
                continue;
            }
            let case = format!("{} while {} is deleted", resource.case, deleted.case);
            let arn = resource.arn(REGION);

            let (status, body) = tag_in(&router, REGION, &arn, &tag_map(&[("Owner", "me")])).await;
            assert_eq!(
                status, 204,
                "case {case:?}: a live resource still takes a tag, got {status} {body:?}"
            );
            let (status, body) = untag_in(&router, REGION, &arn, &["Env"]).await;
            assert_eq!(
                status, 204,
                "case {case:?}: a live resource still takes an untag, got {status} {body:?}"
            );

            let expected = tag_map(&[("Owner", "me"), ("Reviewed", "")]);
            let actual = tags_of(&router, REGION, resource, &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// Every class a tagging ARN names is covered above: one `CLASSES` row per
/// `state::TAGGED_SUB_RESOURCE_SEGMENTS` entry plus the detector itself, so a
/// segment this suite does not classify fails here rather than passing silently.
#[test]
fn every_tagged_class_is_covered() {
    assert_eq!(
        CLASSES.len(),
        TAGGED_SUB_RESOURCE_SEGMENTS.len() + 1,
        "every tagged class must be classified here, got {} for {} segments",
        CLASSES.len(),
        TAGGED_SUB_RESOURCE_SEGMENTS.len()
    );
    for (segment, _) in TAGGED_SUB_RESOURCE_SEGMENTS {
        let matching = CLASSES
            .iter()
            .filter(|class| class.segment == segment)
            .count();
        assert_eq!(
            matching, 1,
            "case {segment:?}: exactly one class must name this segment, got {matching}"
        );
    }
    let detectors = CLASSES
        .iter()
        .filter(|class| class.segment.is_empty())
        .count();
    assert_eq!(
        detectors, 1,
        "exactly one class is the detector itself, got {detectors}"
    );
}
