// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of how this service WORDS `NotFoundException`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `chime` signing name — so what is asserted is the body an AWS
//! SDK client parses, not an internal return value.
//!
//! The rule under test: a `NotFoundException` reports WHICH situation the request met,
//! with `Code` `NotFound` and HTTP 404. An id that never named a resource here is reported
//! as that id being absent, worded from the REQUEST — the kind of resource asked for and
//! the id supplied for it (`Meeting <MeetingId> not found`) — so a caller that passed two
//! ids can tell which one was wrong. A meeting that
//! EXISTED and has since been deleted is reported as having ENDED instead, because the
//! caller's id was right and no retry of it can ever succeed; `DeleteMeeting` is
//! documented as disconnecting every client and preventing new ones from joining, which
//! is exactly that outcome.
//!
//! An `AttendeeId` absent from a meeting that EXISTS is NOT this refusal: the meeting is
//! live, so access to that attendee's record is revoked rather than unknown, and the
//! answer is the modelled `ForbiddenException` (HTTP 403) naming both ids the request
//! carried. That is the recorded answer of this operation family for an `AttendeeId` the
//! meeting holds no attendee under — whatever made it unresolvable — rather than a reading
//! of `NotFoundException`'s generic documentation, which this service applies to the
//! `MeetingId` half of the same request; see
//! `issues/things-that-now-work/20261029T175426Z-chime-sdk-meetings-6-net-new-root-causes.md`
//! (cause 5), which this suite keeps pinned.
//!
//! The table spans the class the rule generalises over: which operation refuses (reader,
//! lister, writer and deleter, for both resource kinds), which resource is the absent one
//! (the meeting, or an attendee inside a meeting that exists), whether the id was never
//! created, was created in ANOTHER meeting or was deleted, whether the id is
//! caller-supplied or minted by this service, and the form of the `GuidString` itself
//! (lower-case hex, upper-case hex, both numeric extremes). No row uses the id the issue
//! recorded.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `chime` signing name
/// this service publishes (`OPERATIONS.md`, `## Wire protocol`). The scope is how the
/// router picks the claiming service; the signature itself is never checked here, since
/// the harness turns request authentication off.
const SIGNED_FOR_CHIME: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Well-formed `GuidString`s (modelled pattern
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`, from
/// `smithy_query(query="operation",operation="GetMeeting,GetAttendee",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`)
/// that name nothing in any store: the pattern's whole character class, spanning
/// lower-case hex, upper-case hex and both of its numeric extremes.
const ABSENT_IDS: &[(&str, &str)] = &[
    ("lower-case hex", "0c1d2e3f-4a5b-4c6d-8e9f-0a1b2c3d4e5f"),
    ("upper-case hex", "0C1D2E3F-4A5B-4C6D-8E9F-0A1B2C3D4E5F"),
    ("all zeroes", "00000000-0000-0000-0000-000000000000"),
    ("all f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
];

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A named member of the body, as a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body.get(name).and_then(Value::as_str).map(str::to_string)
    }
}

/// The emulator under test: ONE shared engine with this service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://meetings-chime.us-east-1.amazonaws.com{path}"))
            .header("authorization", SIGNED_FOR_CHIME)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        // restJson1 carries the wire error code in `x-amzn-errortype`; fall back to the
        // body's `__type` so a missing header never reads as "no error code".
        let error_code = header_code.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        });
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// Create a meeting and return its minted `MeetingId`.
    async fn new_meeting_id(&self, token: &str, external_meeting_id: &str) -> String {
        let reply = self
            .call(
                "POST",
                "/meetings",
                Some(&json!({
                    "ClientRequestToken": token,
                    "MediaRegion": "us-east-1",
                    "ExternalMeetingId": external_meeting_id,
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "CreateMeeting {token:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        reply
            .body
            .get("Meeting")
            .and_then(|meeting| meeting.get("MeetingId"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateMeeting {token:?} returned no MeetingId"))
            .to_string()
    }

    async fn delete_meeting(&self, meeting_id: &str) -> Reply {
        self.call("DELETE", &format!("/meetings/{meeting_id}"), None)
            .await
    }

    /// Create an attendee of `meeting_id` and return its minted `AttendeeId`.
    async fn new_attendee_id(&self, meeting_id: &str, user: &str) -> String {
        let reply = self
            .call(
                "POST",
                &format!("/meetings/{meeting_id}/attendees"),
                Some(&json!({ "ExternalUserId": user })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "CreateAttendee {user:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        reply
            .body
            .get("Attendee")
            .and_then(|attendee| attendee.get("AttendeeId"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateAttendee {user:?} returned no AttendeeId"))
            .to_string()
    }

    /// Every operation that resolves an `AttendeeId` inside a meeting, each called with the
    /// pair of ids given: a reader, a writer and the deleter.
    async fn attendee_calls(&self, meeting_id: &str, attendee_id: &str) -> Vec<(&str, Reply)> {
        vec![
            (
                "GetAttendee",
                self.call(
                    "GET",
                    &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
                    None,
                )
                .await,
            ),
            (
                "UpdateAttendeeCapabilities",
                self.call(
                    "PUT",
                    &format!("/meetings/{meeting_id}/attendees/{attendee_id}/capabilities"),
                    Some(&json!({ "Capabilities": admissible_capabilities() })),
                )
                .await,
            ),
            (
                "DeleteAttendee",
                self.call(
                    "DELETE",
                    &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
                    None,
                )
                .await,
            ),
        ]
    }
}

/// A capability set every meeting created with the documented default `MeetingFeatures`
/// admits (`AttendeeCapabilities`' documented corner cases: content reception needs video
/// reception, and neither member's maximum resolution is `None` by default), so an
/// `UpdateAttendeeCapabilities` row below is refused for the id it names rather than for
/// the set it carries.
fn admissible_capabilities() -> Value {
    json!({ "Audio": "SendReceive", "Video": "Receive", "Content": "Receive" })
}

/// How an absent resource of `kind` with id `id` is reported.
fn absent_message(kind: &str, id: &str) -> String {
    format!("{kind} <{id}> not found")
}

/// How a meeting that existed and has since been deleted is reported.
const ENDED_MESSAGE: &str = "The meeting has ended";

/// How an `AttendeeId` that names no attendee of a LIVE meeting is reported: both ids come
/// from the request, so a caller that passed two can see which pair was refused.
fn not_in_meeting_message(attendee_id: &str, meeting_id: &str) -> String {
    format!("Attendee <{attendee_id}> is not in the meeting <{meeting_id}>")
}

/// Assert one refusal is the modelled `ForbiddenException` carrying `expected`.
fn assert_forbidden(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (403, Some("ForbiddenException")),
        "case {case:?}: expected the modelled 403 ForbiddenException, got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.text("Code").as_deref(),
        Some("Forbidden"),
        "case {case:?}: expected Code Forbidden, got {:?}",
        reply.text("Code")
    );
    assert_eq!(
        reply.text("Message").as_deref(),
        Some(expected),
        "case {case:?}: expected Message {expected:?}, got {:?}",
        reply.text("Message")
    );
}

/// Assert one refusal is the modelled `NotFoundException` carrying `expected`.
fn assert_not_found(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (404, Some("NotFoundException")),
        "case {case:?}: expected the modelled 404 NotFoundException, got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.text("Code").as_deref(),
        Some("NotFound"),
        "case {case:?}: expected Code NotFound, got {:?}",
        reply.text("Code")
    );
    assert_eq!(
        reply.text("Message").as_deref(),
        Some(expected),
        "case {case:?}: expected Message {expected:?}, got {:?}",
        reply.text("Message")
    );
}

/// Every operation that resolves a `MeetingId` reports an id that never named a meeting
/// by naming it, and a meeting that was deleted as one that has ended.
#[tokio::test]
async fn an_absent_meeting_is_reported_by_the_situation_the_request_met() {
    let emulator = Emulator::new();

    // A meeting that DOES exist, so no row's refusal can be a store-wide failure.
    let present = emulator.new_meeting_id("tok-present", "ext-present").await;

    // (case, the absent MeetingId, the message that situation is reported with) — ids
    // never created, plus two minted by this service and then deleted.
    let mut rows: Vec<(String, String, String)> = ABSENT_IDS
        .iter()
        .map(|(case, id)| {
            (
                format!("never created, {case}"),
                (*id).to_string(),
                absent_message("Meeting", id),
            )
        })
        .collect();
    for (case, token) in [("with no attendees", "tok-deleted"), ("with one attendee", "tok-deleted-2")] {
        let deleted = emulator.new_meeting_id(token, &format!("ext-{token}")).await;
        if case == "with one attendee" {
            let created = emulator
                .call(
                    "POST",
                    &format!("/meetings/{deleted}/attendees"),
                    Some(&json!({ "ExternalUserId": "user-before-delete" })),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "CreateAttendee before the delete: expected 200, got {} with {:?}",
                created.status, created.body
            );
        }
        let gone = emulator.delete_meeting(&deleted).await;
        assert_eq!(
            gone.status, 204,
            "DeleteMeeting on a meeting that exists: expected 204, got {} with {:?}",
            gone.status, gone.body
        );
        rows.push((
            format!("minted by the service {case}, then deleted"),
            deleted,
            ENDED_MESSAGE.to_string(),
        ));
    }

    for (case, meeting_id, expected) in &rows {
        // Every operation whose first lookup is the meeting: a reader, a lister, a
        // writer, and the two attendee operations that must resolve the meeting first.
        let attendee_id = ABSENT_IDS[0].1;
        let refusals: Vec<(&str, Reply)> = vec![
            (
                "GetMeeting",
                emulator
                    .call("GET", &format!("/meetings/{meeting_id}"), None)
                    .await,
            ),
            (
                "ListAttendees",
                emulator
                    .call("GET", &format!("/meetings/{meeting_id}/attendees"), None)
                    .await,
            ),
            (
                "CreateAttendee",
                emulator
                    .call(
                        "POST",
                        &format!("/meetings/{meeting_id}/attendees"),
                        Some(&json!({ "ExternalUserId": "user-absent-meeting" })),
                    )
                    .await,
            ),
            (
                "GetAttendee",
                emulator
                    .call(
                        "GET",
                        &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
                        None,
                    )
                    .await,
            ),
            (
                "DeleteAttendee",
                emulator
                    .call(
                        "DELETE",
                        &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
                        None,
                    )
                    .await,
            ),
        ];
        for (operation, reply) in refusals {
            assert_not_found(
                &format!("{operation} on a meeting {case}"),
                &reply,
                expected,
            );
        }
    }

    // The two situations are reported DIFFERENTLY — an id that never named a meeting is
    // not reported as a meeting that ended, and the reverse.
    assert!(
        rows.iter().any(|(_, _, message)| message == ENDED_MESSAGE)
            && rows
                .iter()
                .any(|(_, _, message)| message != ENDED_MESSAGE),
        "the table must cover both situations, got {:?}",
        rows.iter().map(|(case, _, _)| case).collect::<Vec<_>>()
    );

    // The meeting that exists is still readable, so the refusals above are about the ids
    // they named and nothing else.
    let intact = emulator
        .call("GET", &format!("/meetings/{present}"), None)
        .await;
    assert_eq!(
        intact.status, 200,
        "GetMeeting on the meeting that exists: expected 200, got {} with {:?}",
        intact.status, intact.body
    );
}

/// When the MEETING exists but the attendee does not, the refusal is the modelled
/// `ForbiddenException` naming the ATTENDEE and the MEETING the request carried — so a
/// caller that passed both ids can tell which of the two was the absent one, and knows the
/// attendee is gone for good rather than mistyped.
///
/// The rows span the class the rule generalises over: WHICH operation resolves the
/// `AttendeeId` (the reader, the writer and the deleter, all three of which declare both
/// errors), HOW the id came to name no attendee of this meeting (never created in any
/// meeting, created in ANOTHER live meeting, or held by this meeting until `DeleteAttendee`
/// removed it), and the form of the `GuidString` itself. The 403 is a verdict about the
/// attendee the request named and not about its caller, which the tail of the test pins:
/// the SAME `Authorization` header is served HTTP 200 for the attendee of that meeting that
/// does exist and for `ListAttendees` on it.
#[tokio::test]
async fn an_attendee_absent_from_a_live_meeting_is_permanently_refused() {
    let emulator = Emulator::new();
    let meeting_id = emulator.new_meeting_id("tok-attendees", "ext-attendees").await;

    // An attendee that DOES exist in the same meeting and survives every row, so the
    // refusals below are about the id each one names rather than about an empty meeting.
    let survivor = emulator.new_attendee_id(&meeting_id, "user-present").await;
    // An attendee of ANOTHER live meeting: an id that names a real attendee, but not one of
    // the meeting the refused calls name.
    let other_meeting_id = emulator.new_meeting_id("tok-other", "ext-other").await;
    let other_attendee = emulator
        .new_attendee_id(&other_meeting_id, "user-other-meeting")
        .await;

    // (case, the AttendeeId that names no attendee of `meeting_id`) — every form the
    // modelled pattern admits, another meeting's attendee, and one this service minted here
    // and then deleted.
    let mut rows: Vec<(String, String)> = ABSENT_IDS
        .iter()
        .map(|(case, id)| (format!("never created, {case}"), (*id).to_string()))
        .collect();
    rows.push((
        "created in another live meeting".to_string(),
        other_attendee.clone(),
    ));
    let doomed = emulator.new_attendee_id(&meeting_id, "user-doomed").await;
    let deleted = emulator
        .call(
            "DELETE",
            &format!("/meetings/{meeting_id}/attendees/{doomed}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "DeleteAttendee on an attendee that exists: expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    rows.push(("minted by the service, then deleted".to_string(), doomed));

    for (case, attendee_id) in &rows {
        for (operation, reply) in emulator.attendee_calls(&meeting_id, attendee_id).await {
            assert_forbidden(
                &format!("{operation} on an attendee {case}"),
                &reply,
                &not_in_meeting_message(attendee_id, &meeting_id),
            );
        }
    }

    // The meeting itself is untouched: it is still readable after every refusal.
    let intact = emulator
        .call("GET", &format!("/meetings/{meeting_id}"), None)
        .await;
    assert_eq!(
        intact.status, 200,
        "GetMeeting after the attendee refusals: expected 200, got {} with {:?}",
        intact.status, intact.body
    );

    // The same caller, with the same header every refused row carried, is served the
    // attendee of that meeting that DOES exist and the list of them — so none of the 403s
    // above says anything about the client's permission to make the request.
    let served = emulator
        .call(
            "GET",
            &format!("/meetings/{meeting_id}/attendees/{survivor}"),
            None,
        )
        .await;
    assert_eq!(
        (
            served.status,
            served
                .body
                .get("Attendee")
                .and_then(|attendee| attendee.get("AttendeeId"))
                .and_then(Value::as_str)
                .map(str::to_string)
        ),
        (200, Some(survivor.clone())),
        "GetAttendee on the attendee that exists: expected 200 for {survivor:?}, got {} with \
         {:?}",
        served.status,
        served.body
    );
    let listed = emulator
        .call("GET", &format!("/meetings/{meeting_id}/attendees"), None)
        .await;
    let remaining: Vec<String> = listed
        .body
        .get("Attendees")
        .and_then(Value::as_array)
        .map(|attendees| {
            attendees
                .iter()
                .filter_map(|attendee| attendee.get("AttendeeId").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        (listed.status, remaining.clone()),
        (200, vec![survivor]),
        "ListAttendees after the refusals: expected 200 listing only the surviving attendee, \
         got {} with {:?}",
        listed.status,
        listed.body
    );
    // The other meeting's attendee the wrong-meeting row named is untouched too.
    let untouched = emulator
        .call(
            "GET",
            &format!("/meetings/{other_meeting_id}/attendees/{other_attendee}"),
            None,
        )
        .await;
    assert_eq!(
        untouched.status, 200,
        "GetAttendee through its own meeting: expected 200 for {other_attendee:?}, got {} with \
         {:?}",
        untouched.status, untouched.body
    );
}
