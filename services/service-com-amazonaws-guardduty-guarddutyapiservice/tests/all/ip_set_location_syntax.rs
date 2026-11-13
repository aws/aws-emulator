// SPDX-License-Identifier: CC0-1.0

//! The syntax an uploaded list's `Location` has to have, end to end over the wire.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//!
//! > Every uploaded list's `Location` is the URI of a file GuardDuty fetches from
//! > Amazon S3, so a value that is not an Amazon S3 URL is refused — HTTP 400 under
//! > the `InvalidInputException` class, naming the list family's own location
//! > parameter and echoing nothing of the value — whatever `Activate` says and
//! > whatever else the request carries, while an S3 object URL — the `s3://` scheme,
//! > or an `http`/`https` URL naming a bucket and a non-empty object key at any S3
//! > endpoint, whatever its host and port — is accepted and read back verbatim.
//!
//! The rule is the FAMILY's: it holds for the IPSet and the threat list, on the
//! create that stores a location and on the update that replaces one. (The entity-set
//! pair, whose documentation states the rule in so many words, is covered by the
//! `trusted_entity_set_lifecycle` suite.)
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, so these
//! tests exercise routing, restJson1 (de)serialization, modelled-constraint
//! validation and the handlers together.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, error_code, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    invalid_parameter_message, IP_SET_LOCATION_MEMBER, THREAT_INTEL_SET_LOCATION_MEMBER,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// One uploaded-list family: the route segment its sub-resource hangs off, the
/// `Format` member value a create needs, and the parameter name a refusal of its
/// `Location` carries.
struct Family {
    case: &'static str,
    segment: &'static str,
    /// The `jsonName` of the id the create reports (`ipSetId` / `threatIntelSetId`).
    id_member: &'static str,
    format: &'static str,
    member: &'static str,
}

/// The two detector-rooted uploaded lists. `Format` values are members of
/// `IpSetFormat` / `ThreatIntelSetFormat`
/// (`smithy_query(shape="IpSetFormat")`, `smithy_query(shape="ThreatIntelSetFormat")`).
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "ipset",
            segment: "ipset",
            id_member: "ipSetId",
            format: "TXT",
            member: IP_SET_LOCATION_MEMBER,
        },
        Family {
            case: "threatintelset",
            segment: "threatintelset",
            id_member: "threatIntelSetId",
            format: "STIX",
            member: THREAT_INTEL_SET_LOCATION_MEMBER,
        },
    ]
}

/// Locations that ARE Amazon S3 object URLs, spanning what the rule generalises over:
/// the SPELLING of the address (the `s3://` scheme, the path-style endpoint and the
/// virtual-hosted-style one), the HOST and PORT of the endpoint the object URL names
/// (a Region-less and a regional public endpoint, a private endpoint reached by name
/// or by loopback address, with and without an explicit port), the SCHEME of an object
/// URL (`https` and `http`), the DEPTH and the CHARACTER SET of the object key, and
/// the longest value `Location`'s modelled `len 1..300` admits.
///
/// Source for the spellings: the S3 URL forms — `s3://bucket/key`, the path-style
/// `<s3 endpoint>/bucket/key` and the virtual-hosted-style `bucket.s3<...>/key`.
/// Which host serves S3 is environment-specific and is no part of the rule, so a
/// non-public endpoint is an S3 endpoint too.
fn accepted_locations() -> Vec<(String, String)> {
    let longest = format!("s3://my-bucket/{}", "k".repeat(300 - "s3://my-bucket/".len()));
    vec![
        ("the shortest s3:// URL".to_string(), "s3://b/k".to_string()),
        (
            "an s3:// URL with a nested key".to_string(),
            "s3://my-bucket/lists/2026/trusted.txt".to_string(),
        ),
        (
            "a path-style HTTPS endpoint, Region-less".to_string(),
            "https://s3.amazonaws.com/my-bucket/lists/trusted.txt".to_string(),
        ),
        (
            "a path-style HTTPS endpoint, dotted Region".to_string(),
            "https://s3.eu-west-1.amazonaws.com/my-bucket/lists/trusted.txt".to_string(),
        ),
        (
            "a path-style HTTPS endpoint, dashed Region".to_string(),
            "https://s3-ap-northeast-1.amazonaws.com/my-bucket/lists/trusted.txt".to_string(),
        ),
        (
            "a virtual-hosted-style endpoint".to_string(),
            "https://my-bucket.s3.us-west-2.amazonaws.com/lists/trusted.txt".to_string(),
        ),
        (
            "a path-style object URL on a private endpoint named by host".to_string(),
            "https://s3.internal.example/my-bucket/lists/trusted.txt".to_string(),
        ),
        (
            "a path-style object URL on an endpoint with an explicit port".to_string(),
            "https://localhost:4566/my-bucket/lists/trusted.txt".to_string(),
        ),
        (
            "a path-style object URL on a loopback endpoint over http".to_string(),
            "http://127.0.0.1:9000/my-bucket/lists/trusted.txt".to_string(),
        ),
        (
            "a virtual-hosted-style object URL outside amazonaws.com".to_string(),
            "https://my-bucket.s3.internal.example/lists/trusted.txt".to_string(),
        ),
        (
            "a non-ASCII object key".to_string(),
            "s3://my-bucket/\u{30c8}\u{30e9}\u{30b9}\u{30c8}.txt".to_string(),
        ),
        ("the longest location the model admits".to_string(), longest),
    ]
}

/// Locations that are NOT Amazon S3 object URLs, spanning the ways a value fails to be
/// one: free text, no scheme at all, a scheme no S3 object is addressed by, a URL with
/// no authority, a URL naming no object (a bucket with no key, or an empty key), and a
/// value at the modelled minimum length.
fn refused_locations() -> Vec<(&'static str, &'static str)> {
    vec![
        ("free text with spaces", "this is not a valid s3 uri or arn"),
        ("a bare path", "/lists/trusted.txt"),
        ("a bare bucket and key", "my-bucket/lists/trusted.txt"),
        ("the shortest value the model admits", "s"),
        ("another scheme", "ftp://my-bucket/lists/trusted.txt"),
        ("a file URL naming no authority", "file:///my-bucket/lists/trusted.txt"),
        ("an s3:// URL naming only a bucket", "s3://my-bucket"),
        ("an s3:// URL whose key is empty", "s3://my-bucket/"),
        (
            "a path-style endpoint naming only a bucket",
            "https://s3.amazonaws.com/my-bucket",
        ),
        (
            "a path-style endpoint whose key is empty",
            "https://s3.amazonaws.com/my-bucket/",
        ),
        (
            "an object URL naming a host and no object",
            "https://localhost:4566/my-bucket",
        ),
        (
            "a virtual-hosted-style endpoint whose key is empty",
            "https://my-bucket.s3.amazonaws.com/",
        ),
    ]
}

async fn create(router: &Router, detector: &str, family: &Family, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector}/{}", family.segment),
        Some(body),
    )
    .await
}

async fn update(
    router: &Router,
    detector: &str,
    family: &Family,
    id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector}/{}/{id}", family.segment),
        Some(body),
    )
    .await
}

async fn read(router: &Router, detector: &str, family: &Family, id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector}/{}/{id}", family.segment),
        None,
    )
    .await
}

/// Assert a response is the classified HTTP 400 naming `member` and echoing nothing
/// of `sent`.
fn assert_location_refusal(case: &str, status: u16, body: &Value, member: &str, sent: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    let expected = invalid_parameter_message(member);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
    assert!(
        // A value of a character or two is a substring of any English sentence, so
        // only a value long enough to witness an echo is checked for one.
        sent.len() < 4 || !message.contains(sent),
        "case {case:?}: a refusal must not echo the caller's value {sent:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// An S3 URL is accepted and read back verbatim.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_s3_url_spelling_is_accepted_and_read_back_verbatim() {
    for family in families() {
        for (index, (case, location)) in accepted_locations().into_iter().enumerate() {
            // A router per case: the account's activated-list allowance is a
            // different rule (the `ip_set_activation_allowance` suite's), and it
            // must not decide what this one observes.
            let router = new_router();
            let detector = enabled_detector(&router).await;
            let case = format!("{}: {case}", family.case);

            let (status, body) = create(
                &router,
                &detector,
                &family,
                json!({
                    "name": format!("located-{index}"),
                    "format": family.format,
                    "location": location,
                    "activate": true
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: an S3 URL must be accepted, got {status} {body:?}"
            );
            let id = body[family.id_member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {body:?}"))
                .to_string();

            let (status, stored) = read(&router, &detector, &family, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the stored list must read back, got {status} {stored:?}"
            );
            let reported = stored["location"].as_str().unwrap_or_default();
            assert_eq!(
                reported, location,
                "case {case:?}: expected the location {location:?} back, got {reported:?}"
            );

            // The same value is accepted by the update that replaces a location.
            let (status, updated) = update(
                &router,
                &detector,
                &family,
                &id,
                json!({"location": location}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: an update to the same S3 URL must be accepted, got {status} {updated:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Anything that is not an S3 URL is refused, and stores nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_whose_location_is_no_s3_url_is_refused_and_stores_nothing() {
    for family in families() {
        let router = new_router();
        let detector = enabled_detector(&router).await;

        for (index, (why, location)) in refused_locations().into_iter().enumerate() {
            // "Invalid URL formats will result in an error, regardless of whether
            // you activate the … set or not", so both values of `Activate` are
            // refused, and so is the create that carries the optional members.
            for activate in [true, false] {
                let case = format!("{} create: {why}, activate={activate}", family.case);
                let (status, body) = create(
                    &router,
                    &detector,
                    &family,
                    json!({
                        "name": format!("refused-{index}-{activate}"),
                        "format": family.format,
                        "location": location,
                        "activate": activate,
                        "expectedBucketOwner": "123456789012",
                        "tags": {"Env": "test"}
                    }),
                )
                .await;
                assert_location_refusal(&case, status, &body, family.member, location);
            }
        }

        // Nothing was stored: the detector's collection is still empty.
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/detector/{detector}/{}", family.segment),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the listing must be served, got {status} {listed:?}",
            family.case
        );
        let ids = listed[format!("{}s", family.id_member)]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            ids.is_empty(),
            "case {:?}: a refused create must store no list, got {ids:?}",
            family.case
        );
    }
}

#[tokio::test]
async fn an_update_whose_location_is_no_s3_url_is_refused_and_keeps_the_stored_one() {
    for family in families() {
        let router = new_router();
        let detector = enabled_detector(&router).await;
        let stored_location = "s3://my-bucket/lists/stored.txt";

        let (status, body) = create(
            &router,
            &detector,
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
            "case {:?}: the baseline create expected 200, got {status} {body:?}",
            family.case
        );
        let id = body[family.id_member]
            .as_str()
            .unwrap_or_else(|| panic!("expected an id, got {body:?}"))
            .to_string();

        for (why, location) in refused_locations() {
            // Beside the bad location: a member the update would otherwise have
            // written, so the refusal is not a function of what else it carries.
            for (shape, extra) in [
                ("alone", json!({})),
                ("beside a name", json!({"name": "renamed"})),
                ("beside activate", json!({"activate": true})),
            ] {
                let case = format!("{} update: {why}, {shape}", family.case);
                let mut request = json!({"location": location});
                if let Some(members) = extra.as_object() {
                    for (key, value) in members {
                        request[key] = value.clone();
                    }
                }
                let (status, response) =
                    update(&router, &detector, &family, &id, request).await;
                assert_location_refusal(&case, status, &response, family.member, location);

                // The refusal wrote nothing: the stored location is the one the
                // create left behind, and so is the name.
                let (status, stored) = read(&router, &detector, &family, &id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the list must still read back, got {status} {stored:?}"
                );
                let reported = stored["location"].as_str().unwrap_or_default();
                assert_eq!(
                    reported, stored_location,
                    "case {case:?}: a refused update must keep the stored location \
                     {stored_location:?}, got {reported:?}"
                );
                let name = stored["name"].as_str().unwrap_or_default();
                assert_eq!(
                    name, "stored-list",
                    "case {case:?}: a refused update must keep the stored name, got {name:?}"
                );
            }
        }
    }
}
