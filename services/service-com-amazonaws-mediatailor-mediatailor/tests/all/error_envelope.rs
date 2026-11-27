// SPDX-License-Identifier: CC0-1.0

//! The envelope every refusal of this service travels in, end to end over HTTP.
//!
//! The rule under test: every error response this service returns carries, in
//! its restJson1 body, the refusal's text under the member the error shape that
//! answered declares for it, and a `requestId` naming that response —
//! whichever operation was called, whichever error shape answers it (the
//! modelled `BadRequestException`, the service's own not-found envelope, or the
//! scaffold's not-implemented signal) and whatever status it carries.
//!
//! The codec keys a member by its `@jsonName` or else by its member name, so
//! that member is a property of the SHAPE and not of the operation: the
//! modelled `BadRequestException` declares `Message` and no `@jsonName`, and
//! the shapes this service synthesises for what its model declares no error for
//! declare `message`.
//!
//! The table spans the class the rule generalises over: both HTTP methods that
//! refuse with a body, each of the three error shapes and each status they
//! carry (400, 404, 501), a refusal raised before the store is read and one
//! raised by the read itself, a message that is entirely fixed text and one
//! with a variable part taken from the request, and a resource addressed by
//! name as well as one addressed by ARN. The recorded cases — a duplicate
//! channel name, a read of an absent channel, a `PercentEnabled` above the
//! documented maximum and a configure-logs call for an absent configuration —
//! are four rows of it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::channel_lifecycle::{
    encode, Emulator, Reply, CHANNEL_ALREADY_EXISTS_MESSAGE, CHANNEL_NOT_FOUND_MESSAGE,
    MAX_TIME_SHIFT_SECONDS, MIN_TIME_SHIFT_SECONDS, MODELLED_ERROR_MESSAGE_MEMBER, NOT_FOUND_CODE,
    NOT_FOUND_STATUS, RESOURCE_NOT_FOUND_MESSAGE, SYNTHETIC_ERROR_MESSAGE_MEMBER,
};

/// The wire error code of the one client error this service's model declares
/// ("A request contains unexpected data", HTTP 400).
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// The wire error code, and the status, of an operation this emulator has not
/// implemented yet.
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The three members an error body of this service carries: the error code the
/// restJson1 envelope discriminates on, the refusal's text under the member the
/// error shape declares for it, and the id naming the response.
pub(crate) fn error_body_members(message_member: &str) -> BTreeSet<String> {
    ["__type", message_member, "requestId"]
        .iter()
        .map(|member| (*member).to_string())
        .collect()
}

/// `PercentEnabled`'s documented maximum ("Valid values: 0 - 100"), the limit
/// the configure-logs refusal names.
const MAX_PERCENT_ENABLED: i64 = 100;

/// One refused request and the answer the rule requires for it.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
    /// The sentence the body must carry, or `None` where the row only requires
    /// that one is there (the scaffold signal words itself).
    message: Option<String>,
    /// The member the error shape that answers this row carries that sentence
    /// in.
    message_member: &'static str,
}

/// A channel request that is accepted, so a row can collide with it or read it
/// back.
fn a_channel() -> Value {
    json!({
        "PlaybackMode": "LOOP",
        "Tier": "BASIC",
        "Outputs": [{
            "ManifestName": "default",
            "SourceGroup": "default",
            "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
        }],
    })
}

/// A configure-logs request for `name` carrying `percent_enabled`.
fn configure_logs_request(name: &str, percent_enabled: i64) -> Value {
    json!({
        "PlaybackConfigurationName": name,
        "PercentEnabled": percent_enabled,
    })
}

/// Whether `value` has the shape of an AWS request id. The form is not restated
/// here: `service_common::query_error_envelope` owns both the minting of the id
/// and this reading of its form.
fn is_request_id_shaped(value: &str) -> bool {
    service_common::query_error_envelope::is_request_id(value)
}

/// Assert `reply` is the envelope the rule requires for `row`, and return the
/// `requestId` it named.
fn assert_envelope(row: &Row, reply: &Reply) -> String {
    let case = row.case;
    assert_eq!(
        reply.status, row.status,
        "case {case:?}: expected HTTP {}, got {} {:?}",
        row.status, reply.status, reply.body
    );
    assert_eq!(
        reply.error_code, row.code,
        "case {case:?}: expected the error type {:?}, got {:?}",
        row.code, reply.error_code
    );

    // The body carries exactly the envelope's three members: the text travels
    // under the member the error shape declares, and under no second key.
    let members: BTreeSet<String> = reply
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    assert_eq!(
        members,
        error_body_members(row.message_member),
        "case {case:?}: expected the error body's members, got {:?}",
        reply.body
    );

    match &row.message {
        Some(expected) => assert_eq!(
            &reply.message(),
            expected,
            "case {case:?}: expected the refusal to read {expected:?}, got {:?}",
            reply.body
        ),
        None => assert!(
            !reply.message().is_empty(),
            "case {case:?}: expected the refusal to say what was wrong, got {:?}",
            reply.body
        ),
    }

    let request_id = reply.request_id();
    assert!(
        is_request_id_shaped(&request_id),
        "case {case:?}: expected a request id naming the response, got {request_id:?}"
    );
    request_id
}

#[tokio::test]
async fn every_refusal_carries_its_text_and_names_its_request() {
    let emulator = Emulator::new();

    // The resources the rows address: one channel that is held, one whose ARN
    // addresses nothing once it is deleted, and one playback configuration.
    assert_eq!(
        emulator.create("taken", &a_channel()).await.status,
        200,
        "the channel a collision needs must be created"
    );
    let released = emulator.create("released", &a_channel()).await;
    assert_eq!(
        released.status, 200,
        "the channel whose ARN is freed must be created"
    );
    let released_arn = released.string("/Arn");
    assert_eq!(
        emulator.delete("released").await.status,
        200,
        "the channel whose ARN is freed must be deleted"
    );
    assert_eq!(
        emulator
            .request("PUT", "/playbackConfiguration", Some(json!({ "Name": "pc" })))
            .await
            .status,
        200,
        "the playback configuration the configure-logs rows address must be created"
    );

    let rows = vec![
        Row {
            case: "a create under a name the account already holds",
            method: "POST",
            uri: "/channel/taken".to_string(),
            body: Some(a_channel()),
            status: 400,
            code: BAD_REQUEST_CODE,
            message: Some(CHANNEL_ALREADY_EXISTS_MESSAGE.to_string()),
            message_member: MODELLED_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a create whose LINEAR channel names no filler slate",
            method: "POST",
            uri: "/channel/linear-without-slate".to_string(),
            body: Some(json!({
                "PlaybackMode": "LINEAR",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
            })),
            status: 400,
            code: BAD_REQUEST_CODE,
            message: Some(
                "FillerSlate is required for a channel using the LINEAR PlaybackMode".to_string(),
            ),
            message_member: MODELLED_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a create whose time delay is one second past the published maximum",
            method: "POST",
            uri: "/channel/past-the-time-shift-bound".to_string(),
            body: Some(json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "TimeShiftConfiguration": { "MaxTimeDelaySeconds": MAX_TIME_SHIFT_SECONDS + 1 },
            })),
            status: 400,
            code: BAD_REQUEST_CODE,
            // One row whose sentence has a variable part: the bounds and the
            // value the request sent.
            message: Some(format!(
                "MaxTimeDelaySeconds is between {MIN_TIME_SHIFT_SECONDS} and \
                 {MAX_TIME_SHIFT_SECONDS} seconds: {}",
                MAX_TIME_SHIFT_SECONDS + 1
            )),
            message_member: MODELLED_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a read of a channel the account does not hold",
            method: "GET",
            uri: "/channel/never-created".to_string(),
            body: None,
            status: NOT_FOUND_STATUS,
            code: NOT_FOUND_CODE,
            message: Some(CHANNEL_NOT_FOUND_MESSAGE.to_string()),
            message_member: SYNTHETIC_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a start of a channel the account does not hold",
            method: "PUT",
            uri: "/channel/never-created/start".to_string(),
            body: None,
            status: NOT_FOUND_STATUS,
            code: NOT_FOUND_CODE,
            message: Some(CHANNEL_NOT_FOUND_MESSAGE.to_string()),
            message_member: SYNTHETIC_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a configure-logs call above the documented maximum",
            method: "PUT",
            uri: "/configureLogs/playbackConfiguration".to_string(),
            body: Some(configure_logs_request("pc", MAX_PERCENT_ENABLED + 1)),
            status: 400,
            code: BAD_REQUEST_CODE,
            message: Some(format!(
                "PercentEnabled must not exceed {MAX_PERCENT_ENABLED}"
            )),
            message_member: MODELLED_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a configure-logs call for a configuration the account does not hold",
            method: "PUT",
            uri: "/configureLogs/playbackConfiguration".to_string(),
            body: Some(configure_logs_request("never-created", 10)),
            status: NOT_FOUND_STATUS,
            code: NOT_FOUND_CODE,
            // The playback-configuration sentence carries the name the request
            // addressed.
            message: Some("PlaybackConfiguration never-created not found".to_string()),
            message_member: SYNTHETIC_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a tag of an ARN whose channel was deleted",
            method: "POST",
            uri: format!("/tags/{}", encode(&released_arn)),
            body: Some(json!({ "tags": { "Env": "prod" } })),
            status: NOT_FOUND_STATUS,
            code: NOT_FOUND_CODE,
            message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
            message_member: SYNTHETIC_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a read of the tags of an ARN whose channel was deleted",
            method: "GET",
            uri: format!("/tags/{}", encode(&released_arn)),
            body: None,
            status: NOT_FOUND_STATUS,
            code: NOT_FOUND_CODE,
            message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
            message_member: SYNTHETIC_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "an operation this emulator has not implemented",
            method: "GET",
            uri: "/channels".to_string(),
            body: None,
            status: NOT_IMPLEMENTED_STATUS,
            code: NOT_IMPLEMENTED_CODE,
            // This row is the scaffold's own envelope, reached through whichever
            // operation is still a stub; implementing `ListChannels` means
            // pointing it at another one rather than dropping it.
            message: None,
            message_member: SYNTHETIC_ERROR_MESSAGE_MEMBER,
        },
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for row in &rows {
        let reply = emulator
            .request(row.method, &row.uri, row.body.clone())
            .await;
        request_ids.push(assert_envelope(row, &reply));
    }

    // One id per response: no two of these refusals named the same request.
    let distinct: BTreeSet<&String> = request_ids.iter().collect();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "expected every refusal to name its own request, got {request_ids:?}"
    );
}

#[tokio::test]
async fn a_refusal_raised_before_the_handler_also_names_its_request() {
    let emulator = Emulator::new();
    let created = emulator.create("tagged", &a_channel()).await;
    assert_eq!(created.status, 200, "the channel to tag must be created");
    let arn = created.string("/Arn");

    // The framework refuses these before any handler runs — the first for a
    // `@required` member the body omits, the second for a value outside a
    // modelled bound — so they are answered by a layer no error shape of this
    // service reaches. The envelope is a property of the RESPONSE, so they name
    // their request too.
    for (case, method, uri, body) in [
        (
            "a tag call whose body omits the required tag map",
            "POST",
            format!("/tags/{}", encode(&arn)),
            Some(json!({})),
        ),
        (
            "a create whose body omits the required playback mode and outputs",
            "POST",
            "/channel/missing-members".to_string(),
            Some(json!({})),
        ),
    ] {
        let refused = emulator.request(method, &uri, body).await;
        assert!(
            refused.status >= 400,
            "case {case:?}: expected the request to be refused, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: expected the refusal to say what was wrong, got {:?}",
            refused.body
        );
        let request_id = refused.request_id();
        assert!(
            is_request_id_shaped(&request_id),
            "case {case:?}: expected a request id naming the response, got {request_id:?} in {:?}",
            refused.body
        );
    }
}

#[tokio::test]
async fn a_retried_refusal_repeats_its_text_under_a_fresh_request_id() {
    let emulator = Emulator::new();

    // Two refusals of different kinds, so the retry property is checked on the
    // modelled error and on the service's not-found envelope alike.
    assert_eq!(
        emulator.create("retried", &a_channel()).await.status,
        200,
        "the channel a collision needs must be created"
    );
    let rows = [
        (
            "a retried collision",
            "POST",
            "/channel/retried".to_string(),
            Some(a_channel()),
            400,
            CHANNEL_ALREADY_EXISTS_MESSAGE.to_string(),
        ),
        (
            "a retried read of an absent channel",
            "GET",
            "/channel/never-created".to_string(),
            None,
            NOT_FOUND_STATUS,
            CHANNEL_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (case, method, uri, body, status, message) in rows {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for attempt in 1..=3 {
            let reply = emulator.request(method, &uri, body.clone()).await;
            assert_eq!(
                reply.status, status,
                "case {case:?} attempt {attempt}: expected HTTP {status}, got {} {:?}",
                reply.status, reply.body
            );
            // The text is a property of the refusal, so it repeats byte for
            // byte...
            assert_eq!(
                reply.message(),
                message,
                "case {case:?} attempt {attempt}: expected the same sentence, got {:?}",
                reply.body
            );
            // ...while the id names one response, so every attempt gets its own.
            let request_id = reply.request_id();
            assert!(
                is_request_id_shaped(&request_id),
                "case {case:?} attempt {attempt}: expected a request id, got {request_id:?}"
            );
            assert!(
                seen.insert(request_id.clone()),
                "case {case:?} attempt {attempt}: expected a fresh request id, saw {request_id:?} \
                 before"
            );
        }
    }
}

/// A tag key one character past AWS's documented tagging limit of 128
/// characters, so the tagging write below is refused with this service's one
/// modelled client error.
fn an_oversized_tag_key() -> String {
    "k".repeat(129)
}

/// A tagging write carrying that one oversized key.
fn an_oversized_tag_write() -> Value {
    let mut tags = serde_json::Map::new();
    tags.insert(an_oversized_tag_key(), json!("v"));
    json!({ "tags": tags })
}

#[tokio::test]
async fn the_member_a_refusal_states_its_text_in_is_the_one_its_error_shape_declares() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create("held", &a_channel()).await.status,
        200,
        "the channel a collision needs must be created"
    );
    let tagged = emulator.create("tagged", &a_channel()).await;
    assert_eq!(tagged.status, 200, "the channel to tag must be created");
    let arn = tagged.string("/Arn");
    assert_eq!(
        emulator
            .request("PUT", "/playbackConfiguration", Some(json!({ "Name": "pc" })))
            .await
            .status,
        200,
        "the configuration the configure-logs row addresses must be created"
    );

    // The rows span the operations whose refusal is each error shape: four
    // routes that raise the modelled `BadRequestException` — a collision, a
    // member the service's own rule rejects, a stored-size bound and a numeric
    // bound, on three different resources — and two that answer a shape this
    // service synthesises. The member the text travels in follows the SHAPE,
    // not the operation.
    let rows: Vec<Row> = vec![
        Row {
            case: "a channel create under a name the account already holds",
            method: "POST",
            uri: "/channel/held".to_string(),
            body: Some(a_channel()),
            status: 400,
            code: BAD_REQUEST_CODE,
            message: Some(CHANNEL_ALREADY_EXISTS_MESSAGE.to_string()),
            message_member: MODELLED_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a playback configuration whose name carries the path separator",
            method: "PUT",
            uri: "/playbackConfiguration".to_string(),
            body: Some(json!({ "Name": "with/separator" })),
            status: 400,
            code: BAD_REQUEST_CODE,
            message: Some(
                "Name must be a non-empty identifier that contains no '/': with/separator"
                    .to_string(),
            ),
            message_member: MODELLED_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a tag write whose key is past the documented length",
            method: "POST",
            uri: format!("/tags/{}", encode(&arn)),
            body: Some(an_oversized_tag_write()),
            status: 400,
            code: BAD_REQUEST_CODE,
            message: Some(format!(
                "a tag key is at most 128 characters: {}",
                an_oversized_tag_key()
            )),
            message_member: MODELLED_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a configure-logs call below the documented minimum",
            method: "PUT",
            uri: "/configureLogs/playbackConfiguration".to_string(),
            body: Some(configure_logs_request("pc", -1)),
            status: 400,
            code: BAD_REQUEST_CODE,
            message: Some("PercentEnabled must not be less than 0".to_string()),
            message_member: MODELLED_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "a read of a channel the account does not hold",
            method: "GET",
            uri: "/channel/never-created".to_string(),
            body: None,
            status: NOT_FOUND_STATUS,
            code: NOT_FOUND_CODE,
            message: Some(CHANNEL_NOT_FOUND_MESSAGE.to_string()),
            message_member: SYNTHETIC_ERROR_MESSAGE_MEMBER,
        },
        Row {
            case: "an operation this emulator has not implemented",
            method: "GET",
            uri: "/channels".to_string(),
            body: None,
            status: NOT_IMPLEMENTED_STATUS,
            code: NOT_IMPLEMENTED_CODE,
            message: None,
            message_member: SYNTHETIC_ERROR_MESSAGE_MEMBER,
        },
    ];

    for row in &rows {
        let reply = emulator
            .request(row.method, &row.uri, row.body.clone())
            .await;
        let case = row.case;
        assert_envelope(row, &reply);
        // The sentence is under the member its shape declares, and under no
        // other spelling of it.
        let other = if row.message_member == MODELLED_ERROR_MESSAGE_MEMBER {
            SYNTHETIC_ERROR_MESSAGE_MEMBER
        } else {
            MODELLED_ERROR_MESSAGE_MEMBER
        };
        assert!(
            reply
                .get(&format!("/{}", row.message_member))
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected the refusal's text under {:?}, got {:?}",
            row.message_member,
            reply.body
        );
        assert_eq!(
            reply.get(&format!("/{other}")),
            &Value::Null,
            "case {case:?}: expected no second spelling of the text under {other:?}, got {:?}",
            reply.body
        );
    }
}
