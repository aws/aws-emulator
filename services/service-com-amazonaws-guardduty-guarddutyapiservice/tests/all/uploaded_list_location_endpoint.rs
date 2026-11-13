// SPDX-License-Identifier: CC0-1.0

//! Where an uploaded list's `Location` may point, and what a change to a list whose
//! delete is already running answers, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded call:
//!
//! 1. An uploaded list's `Location` is accepted when it addresses an OBJECT — a bucket
//!    and a non-empty key — at an Amazon S3 endpoint, whatever that endpoint's host,
//!    port and scheme are: which host serves S3 is environment-specific, so it is no
//!    part of the rule, and the endpoint the GuardDuty request itself arrived at does
//!    not decide it either. Every row is therefore sent to EACH of several endpoint
//!    authorities and must get the same answer. A URL naming no object at all is
//!    refused with HTTP 400 under the `InvalidInputException` class and stores
//!    nothing. The rule is the uploaded-list FAMILY's, so the rows span all four
//!    families — the IPSet, the threat list and the two entity sets — on the create
//!    that stores a location, and the stored value reads back verbatim.
//! 2. A change to a list whose own delete has already been accepted collides with
//!    work in flight rather than naming an absent resource, so `UpdateIPSet` and
//!    `UpdateThreatIntelSet` refuse it under the `InternalServerErrorException` class
//!    with the 400 of a request the service will not perform, carrying
//!    `state::OPERATION_IN_PROGRESS_MESSAGE` — distinct, in both the class and the
//!    sentence, from the absence an id the detector never held gets — and the refused
//!    change moves neither the list's values nor its status.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_at, enabled_detector, error_code, error_shape, new_router, Router,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    IP_SET_TRANSITION_MILLIS, OPERATION_IN_PROGRESS_MESSAGE, SET_ID_NOT_FOUND_MESSAGE,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The error class a change that collides with work already running on the list
/// carries, in the body's `__type` and in the `x-amzn-errortype` header that resolves
/// the error shape.
const IN_PROGRESS_CLASS: &str = "InternalServerErrorException";

/// The endpoint authority an emulated AWS endpoint is reached at when it is not a
/// public one: a loopback address and a port, which is the form a client configures as
/// its endpoint URL — and so the authority a request arrives at.
const LOOPBACK_ENDPOINT: &str = "127.0.0.1:9297";

/// A second spelling of a non-public endpoint, so the rule is not read off one host.
const NAMED_ENDPOINT: &str = "localhost:4566";

/// The public authority a GuardDuty client reaches when no endpoint of its own is
/// configured.
const PUBLIC_ENDPOINT: &str = "guardduty.us-east-1.amazonaws.com";

/// The endpoint authorities every `Location` row below is sent to in turn. The rule is
/// about the URL the caller submits, not about where the request landed, so each row
/// must get the same answer at every one of them.
fn endpoints() -> [&'static str; 3] {
    [LOOPBACK_ENDPOINT, NAMED_ENDPOINT, PUBLIC_ENDPOINT]
}

/// One uploaded-list family: the route segment its sub-resource hangs off, the
/// `Format` value a create needs and the `jsonName` of the id the create reports.
struct Family {
    case: &'static str,
    segment: &'static str,
    id_member: &'static str,
    format: &'static str,
}

/// The four uploaded-list families. The `Format` values are members of `IpSetFormat`,
/// `ThreatIntelSetFormat`, `ThreatEntitySetFormat` and `TrustedEntitySetFormat`
/// (`smithy_query(shape="IpSetFormat,ThreatIntelSetFormat,ThreatEntitySetFormat,TrustedEntitySetFormat")`).
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "ipset",
            segment: "ipset",
            id_member: "ipSetId",
            format: "TXT",
        },
        Family {
            case: "threatintelset",
            segment: "threatintelset",
            id_member: "threatIntelSetId",
            format: "STIX",
        },
        Family {
            case: "threatentityset",
            segment: "threatentityset",
            id_member: "threatEntitySetId",
            format: "TXT",
        },
        Family {
            case: "trustedentityset",
            segment: "trustedentityset",
            id_member: "trustedEntitySetId",
            format: "TXT",
        },
    ]
}

/// One `Location` case: the location a create carries and whether the service may
/// store it. Every row is sent to each of [`endpoints`].
struct LocationCase {
    why: &'static str,
    location: String,
    accepted: bool,
}

/// The locations, spanning what the rule generalises over: the SPELLING of the S3
/// address (the `s3://` scheme, the path-style endpoint and the virtual-hosted one),
/// the HOST and PORT of the endpoint the object URL names (a public S3 endpoint, a
/// loopback address with a port, a name with a port, a name with none), its SCHEME
/// (`https` and `http`), the DEPTH of the object key, and the ways a value addresses
/// no object at all (no key, an empty key, no scheme).
///
/// The catalogue of values that are no S3 URL at all is the `ip_set_location_syntax`
/// suite's; the rows here carry one as a control and own the half of the rule that
/// says the endpoint does not matter — neither the one the object URL names nor the
/// one the GuardDuty request arrived at.
fn location_cases() -> Vec<LocationCase> {
    vec![
        LocationCase {
            why: "an s3:// URL",
            location: "s3://my-bucket/lists/trusted.txt".to_string(),
            accepted: true,
        },
        LocationCase {
            why: "a path-style public S3 endpoint",
            location: "https://s3.eu-west-1.amazonaws.com/my-bucket/lists/trusted.txt".to_string(),
            accepted: true,
        },
        LocationCase {
            why: "a virtual-hosted-style public S3 endpoint",
            location: "https://my-bucket.s3.us-west-2.amazonaws.com/lists/trusted.txt".to_string(),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on a loopback endpoint with a port",
            location: format!("https://{LOOPBACK_ENDPOINT}/my-bucket/lists/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on a named endpoint with a port",
            location: format!("https://{NAMED_ENDPOINT}/my-bucket/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on that endpoint with a deep key",
            location: format!("https://{NAMED_ENDPOINT}/my-bucket/lists/2026/03/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on an endpoint naming no port",
            location: "https://gd-endpoint.internal/my-bucket/trusted.txt".to_string(),
            accepted: true,
        },
        LocationCase {
            why: "an object URL over http rather than https",
            location: format!("http://{LOOPBACK_ENDPOINT}/my-bucket/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on an unrelated host",
            location: "https://files.example-corp.com/my-bucket/trusted.txt".to_string(),
            accepted: true,
        },
        LocationCase {
            why: "an object URL naming only a bucket",
            location: format!("https://{LOOPBACK_ENDPOINT}/my-bucket"),
            accepted: false,
        },
        LocationCase {
            why: "an object URL with an empty key",
            location: format!("https://{LOOPBACK_ENDPOINT}/my-bucket/"),
            accepted: false,
        },
        LocationCase {
            why: "a value that is no URL at all",
            location: "this is not a valid s3 uri or arn".to_string(),
            accepted: false,
        },
    ]
}

/// A router holding one enabled detector.
async fn detector() -> (Router, String) {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    (router, detector_id)
}

async fn create_at(
    router: &Router,
    endpoint: &str,
    detector_id: &str,
    family: &Family,
    body: Value,
) -> (u16, Value) {
    call_at(
        router,
        endpoint,
        "POST",
        &format!("/detector/{detector_id}/{}", family.segment),
        Some(body),
    )
    .await
}

async fn read(router: &Router, detector_id: &str, family: &Family, id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{}/{id}", family.segment),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the stored list must read back, got {status} {body:?}",
        family.case
    );
    body
}

// ---------------------------------------------------------------------------
// Rule 1: a Location addresses an object at an S3 endpoint, whichever one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_location_is_accepted_exactly_when_it_addresses_an_object_at_any_s3_endpoint() {
    for family in families() {
        for (index, row) in location_cases().into_iter().enumerate() {
            // The same row at each endpoint authority: the answer is a function of
            // the URL alone, not of where the request landed.
            for endpoint in endpoints() {
                // A router per case: a detector holds at most one trusted entity set
                // and the account may have only one ACTIVATED IPSet, and neither
                // allowance is this rule's business.
                let (router, detector_id) = detector().await;
                let case = format!("{} create at {endpoint}: {}", family.case, row.why);

                let (status, body) = create_at(
                    &router,
                    endpoint,
                    &detector_id,
                    &family,
                    json!({
                        "name": format!("located-{index}"),
                        "format": family.format,
                        "location": row.location,
                        "activate": true
                    }),
                )
                .await;

                if row.accepted {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected the create to be served, got {status} {body:?}"
                    );
                    let id = body[family.id_member]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {body:?}"))
                        .to_string();
                    // Stored, and stored VERBATIM: the location a caller gave is the
                    // one a reader reports.
                    let stored = read(&router, &detector_id, &family, &id).await;
                    let reported = stored["location"].as_str().unwrap_or_default();
                    assert_eq!(
                        reported, row.location,
                        "case {case:?}: expected the location {:?} back, got {reported:?}",
                        row.location
                    );
                } else {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400, got {status} {body:?}"
                    );
                    let class = error_code(&body);
                    assert_eq!(
                        class, INVALID_INPUT,
                        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
                    );
                    let message = body["message"].as_str().unwrap_or_default();
                    assert!(
                        !message.contains(&row.location),
                        "case {case:?}: a refusal must not echo {:?}, got {message:?}",
                        row.location
                    );
                    // A refused create stores nothing: the family's collection is
                    // empty.
                    let (status, listed) = call(
                        &router,
                        "GET",
                        &format!("/detector/{detector_id}/{}", family.segment),
                        None,
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: the listing must be served, got {status} {listed:?}"
                    );
                    let ids = listed[format!("{}s", family.id_member)]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    assert!(
                        ids.is_empty(),
                        "case {case:?}: a refused create must store no list, got {ids:?}"
                    );
                }
            }
        }
    }
}

/// An update is held to the same rule, and to no endpoint of its own: the object URL
/// a list was uploaded through is accepted on an update sent to ANY endpoint, while a
/// value addressing no object is refused on each of them and keeps the stored one.
#[tokio::test]
async fn an_update_reads_a_location_by_its_own_syntax_whatever_endpoint_it_arrives_at() {
    for family in families() {
        for endpoint in endpoints() {
            let (router, detector_id) = detector().await;
            let case = format!("{} update at {endpoint}", family.case);
            let stored_location = "s3://my-bucket/lists/stored.txt";

            let (status, body) = create_at(
                &router,
                endpoint,
                &detector_id,
                &family,
                json!({
                    "name": "stored-list",
                    "format": family.format,
                    "location": stored_location,
                    "activate": false
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the baseline create expected 200, got {status} {body:?}"
            );
            let id = body[family.id_member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {body:?}"))
                .to_string();
            let path = format!("/detector/{detector_id}/{}/{id}", family.segment);

            // A URL that names a bucket but no object is refused, and the stored
            // location stands.
            let (status, refused) = call_at(
                &router,
                endpoint,
                "POST",
                &path,
                Some(json!({"location": format!("https://{NAMED_ENDPOINT}/other-bucket")})),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a URL naming no object must be refused, got {status} {refused:?}"
            );
            let kept = read(&router, &detector_id, &family, &id).await;
            assert_eq!(
                kept["location"].as_str(),
                Some(stored_location),
                "case {case:?}: a refused update must keep the stored location, got {kept:?}"
            );

            // An object URL on an endpoint unrelated to the one this request arrived
            // at is an S3 object URL all the same.
            let object_url = format!("https://{NAMED_ENDPOINT}/other-bucket/lists/new.txt");
            let (status, updated) = call_at(
                &router,
                endpoint,
                "POST",
                &path,
                Some(json!({"location": object_url})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: an object URL must be accepted, got {status} {updated:?}"
            );
            let after = read(&router, &detector_id, &family, &id).await;
            assert_eq!(
                after["location"].as_str(),
                Some(object_url.as_str()),
                "case {case:?}: expected the updated location back, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a change to a list whose delete is running collides with that delete.
// ---------------------------------------------------------------------------

/// Long enough for the clock to owe a deleted list its terminal status.
fn past_the_deletion() -> std::time::Duration {
    std::time::Duration::from_millis(IP_SET_TRANSITION_MILLIS as u64 + 250)
}

/// The two detector-rooted uploaded lists whose delete RETAINS the record, so the
/// collision is observable at all: the route segment and the `jsonName` of the id the
/// create reports.
const RETAINED: [(&str, &str); 2] = [("ipset", "ipSetId"), ("threatintelset", "threatIntelSetId")];

#[tokio::test]
async fn an_update_of_a_list_whose_delete_is_running_collides_with_it() {
    for (segment, id_member) in RETAINED {
        let case_family = segment;
        let format = if segment == "ipset" { "TXT" } else { "STIX" };
        // The rows: both ends of the deletion's own transition, and three shapes of
        // change — a rename, an activation and a relocation — so the refusal is not a
        // function of what the update asks for.
        for (when, wait) in [
            ("while the delete is still pending", false),
            ("once the deletion has settled", true),
        ] {
            for (shape, update) in [
                ("a rename", json!({"name": "resurrected"})),
                ("an activation", json!({"activate": true})),
                (
                    "a relocation",
                    json!({"location": "s3://my-bucket/lists/moved.txt"}),
                ),
            ] {
                let case = format!("{case_family} {when}: {shape}");
                let (router, detector_id) = detector().await;
                let (status, created) = call(
                    &router,
                    "POST",
                    &format!("/detector/{detector_id}/{segment}"),
                    Some(json!({
                        "name": "doomed-list",
                        "format": format,
                        "location": "s3://my-bucket/lists/doomed.txt",
                        "activate": true
                    })),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the baseline create expected 200, got {status} {created:?}"
                );
                let id = created[id_member]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {created:?}"))
                    .to_string();
                let path = format!("/detector/{detector_id}/{segment}/{id}");

                let (status, deleted) = call(&router, "DELETE", &path, None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the delete expected 200, got {status} {deleted:?}"
                );
                if wait {
                    tokio::time::sleep(past_the_deletion()).await;
                }
                let before = call(&router, "GET", &path, None).await.1;

                let (status, body) = call(&router, "POST", &path, Some(update.clone())).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body:?}"
                );
                let class = error_code(&body);
                assert_eq!(
                    class, IN_PROGRESS_CLASS,
                    "case {case:?}: expected the {IN_PROGRESS_CLASS} class, got {class:?} \
                     {body:?}"
                );
                let shape_header = error_shape(&body);
                assert_eq!(
                    shape_header, IN_PROGRESS_CLASS,
                    "case {case:?}: expected the {IN_PROGRESS_CLASS} error shape, got \
                     {shape_header:?} {body:?}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, OPERATION_IN_PROGRESS_MESSAGE,
                    "case {case:?}: expected the in-progress sentence, got {message:?}"
                );
                assert!(
                    !message.contains(id.as_str()),
                    "case {case:?}: the refusal must not echo {id:?}, got {message:?}"
                );

                // The refusal wrote nothing: every member the read reports is the one
                // the delete left behind.
                let after = call(&router, "GET", &path, None).await.1;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused update must change nothing, expected {before:?}, \
                     got {after:?}"
                );

                // And the collision is NOT the absence refusal: an id the detector
                // never held is still the family's fixed not-found sentence under the
                // client-rejection class.
                let absent = format!(
                    "/detector/{detector_id}/{segment}/0123456789abcdef0123456789abcdef"
                );
                let (status, body) = call(&router, "POST", &absent, Some(update)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: an absent id expected HTTP 400, got {status} {body:?}"
                );
                let class = error_code(&body);
                assert_eq!(
                    class, INVALID_INPUT,
                    "case {case:?}: an absent id expected the {INVALID_INPUT} class, got \
                     {class:?} {body:?}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, SET_ID_NOT_FOUND_MESSAGE,
                    "case {case:?}: an absent id expected the not-found sentence, got {message:?}"
                );
            }
        }
    }
}
