// SPDX-License-Identifier: CC0-1.0

//! The WORDS this endpoint refuses a request with, end to end over HTTP.
//!
//! Three renderings are pinned here, each over a SET of inputs rather than over
//! one request, because each is a rule about a class of refusal:
//!
//! 1. a constraint the model declares on the ELEMENTS of a collection member is
//!    stated once at that MEMBER — no element position, no `member` step and no
//!    value — with every requirement the element shape declares nested inside
//!    `Member must satisfy constraint: [ … ]`. A constraint broken by the
//!    collection ITSELF keeps its own clause, and a scalar member keeps the
//!    value it echoes;
//! 2. a request that does not name exactly one stream — neither member or both —
//!    is refused with ONE fixed sentence, whichever of the two cases it is;
//! 3. a `StreamName` or `StreamARN` that resolves to no readable stream is
//!    refused with one fixed sentence that names neither the stream nor the
//!    member that addressed it;
//! 4. a timestamp range whose end the model documents as having to be "in the
//!    past" when the request selects fragments by SERVER timestamp is refused
//!    with one fixed sentence, in every operation whose range carries that
//!    clause, while the same end selected by producer timestamp is not.
//!
//! The harness is `archived_media_read_path`'s: the real `MultiServiceRouter`
//! with this one service registered, so every request walks routing, restJson1
//! deserialization, constraint validation and the handler.

use crate::archived_media_read_path::{
    body_json, fragment_number, read_loop_operations, seeded, with_members, STREAM, T0_SECS,
};

/// The element constraint set `GetMediaForFragmentList$Fragments` states.
///
/// Read off the model, not off the request: `Fragments` targets
/// `FragmentNumberList`, whose member targets `FragmentNumberString`
/// (`smithy_query(query="shape",shape="FragmentNumberList,FragmentNumberString")`
/// — `@length(min: 1, max: 128)` and `@pattern("^[0-9]+$")`). Every element of
/// that member has to satisfy all three, which is what the clause states
/// whichever of them a request broke.
const FRAGMENT_ELEMENT_SET: &str = "Member must satisfy constraint: [Member must have length less \
                                    than or equal to 128, Member must have length greater than or \
                                    equal to 1, Member must satisfy regular expression pattern: \
                                    ^[0-9]+$]";

/// The code this endpoint answers a modelled-constraint refusal with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The sentence a request that does not name exactly one stream is refused with.
const STREAM_ADDRESSING: &str = "One of StreamName or StreamARN must be provided, but not both.";

/// The sentence a stream that cannot be read is refused with.
const STREAM_NOT_FOUND: &str = "The requested stream is not found or not active.";

/// The `__type` and `message`/`Message` a refusal reports.
fn reported(response: &http::Response<bytes::Bytes>) -> (String, String) {
    let body = body_json(response);
    let code = body["__type"].as_str().unwrap_or_default().to_string();
    // restJson1 carries the framework's constraint refusal under `message` and
    // this service's modelled errors under their `Message` member; a refusal
    // states exactly one of the two.
    let message = body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string();
    (code, message)
}

/// A numeric fragment number of `digits` digits — the only shape the element
/// shape's `@pattern` admits, so a row is outside the `@length` maximum for its
/// length alone.
fn digits(count: usize) -> String {
    "9".repeat(count)
}

/// RULE: a constraint the model declares on the ELEMENTS of a collection member
/// is reported against that member, with the element shape's whole constraint
/// set and no value; a constraint the COLLECTION itself broke keeps its own
/// clause; a scalar member keeps the framework's clause and its echoed value.
///
/// The rows differ along what the rule generalises over: which of the element
/// shape's constraints was broken, how long the offending value is, how many
/// elements offended and at which positions, and whether the violation is about
/// the elements, the collection or another member entirely.
#[tokio::test]
async fn an_element_constraint_is_reported_against_the_collection_member() {
    let harness = seeded();
    let valid = fragment_number(0);
    // (case, the `Fragments` the request carries, the message expected)
    let rows: Vec<(&str, Vec<String>, String)> = vec![
        (
            "one element outside the element pattern",
            vec!["not-a-fragment-number".to_string()],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "an empty element, under the element minimum",
            vec![String::new()],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "an element one digit over the element maximum",
            vec![digits(129)],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "an element far over the element maximum",
            vec![digits(4096)],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "two offending elements, breaking two different constraints",
            vec![valid.clone(), "abc".to_string(), digits(200)],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "an offending element after several valid ones",
            vec![valid.clone(), valid.clone(), "0x2a".to_string()],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            // The collection's OWN `@length` is not an element constraint: it
            // keeps its own requirement, and still states no value, because a
            // list has no scalar rendering to quote.
            "the empty list, under the collection's own minimum",
            Vec::new(),
            "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
        ),
    ];

    for (case, fragments, expected) in rows {
        let body = serde_json::json!({ "StreamName": STREAM, "Fragments": fragments });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (code, message) = reported(&response);
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {case}: expected {VALIDATION_EXCEPTION}, got {code:?}"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: a constraint broken by a SCALAR member is still the framework's own
/// clause — its path, its one requirement and the value the caller submitted —
/// and the envelope counts one clause per refused member, in the order the
/// model declares them.
#[tokio::test]
async fn a_scalar_member_keeps_its_own_clause_and_its_echoed_value() {
    let harness = seeded();
    let valid = fragment_number(0);
    // (case, the request body, the message expected)
    let rows: Vec<(&str, serde_json::Value, String)> = vec![
        (
            "a StreamName outside its pattern",
            serde_json::json!({ "StreamName": "bad name!", "Fragments": [valid.clone()] }),
            "1 validation error detected: Value 'bad name!' at 'streamName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"
                .to_string(),
        ),
        (
            "a StreamARN outside its pattern",
            serde_json::json!({ "StreamARN": "not-an-arn", "Fragments": [valid.clone()] }),
            "1 validation error detected: Value 'not-an-arn' at 'streamARN' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             ^arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$"
                .to_string(),
        ),
        (
            "a scalar member and an element, both refused",
            serde_json::json!({ "StreamName": "bad name!", "Fragments": ["nope"] }),
            format!(
                "2 validation errors detected: Value 'bad name!' at 'streamName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 ^[a-zA-Z0-9_.-]+$; Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
    ];

    for (case, body, expected) in rows {
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (code, message) = reported(&response);
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {case}: expected {VALIDATION_EXCEPTION}, got {code:?}"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: every operation that reads a stream refuses a request that does not
/// name exactly one with ONE fixed sentence (`InvalidArgumentException`, 400),
/// and one whose name or ARN resolves to no readable stream with one fixed
/// sentence that carries nothing of the request (`ResourceNotFoundException`,
/// 404).
///
/// The rows differ along what the two sentences generalise over: which of the
/// two addressing members was supplied, the spelling and length of the name, and
/// the ARN's region, account and creation suffix — none of which may appear in
/// either message.
#[tokio::test]
async fn the_stream_addressing_refusals_are_one_fixed_sentence_each() {
    let harness = seeded();
    // (case, the stream-naming members, the status, the error, the message)
    let cases: Vec<(&str, serde_json::Value, u16, &str, &str)> = vec![
        (
            "both StreamName and StreamARN",
            serde_json::json!({ "StreamName": STREAM, "StreamARN": harness.arn.clone() }),
            400,
            "InvalidArgumentException",
            STREAM_ADDRESSING,
        ),
        (
            "both, where neither names a held stream",
            serde_json::json!({
                "StreamName": "other-stream",
                "StreamARN": "arn:aws:kinesisvideo:eu-west-1:000000000001:stream/other/1700000000",
            }),
            400,
            "InvalidArgumentException",
            STREAM_ADDRESSING,
        ),
        (
            "neither StreamName nor StreamARN",
            serde_json::json!({}),
            400,
            "InvalidArgumentException",
            STREAM_ADDRESSING,
        ),
        (
            "a StreamName no stream answers to",
            serde_json::json!({ "StreamName": "no-such-stream" }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
        (
            "a differently spelled absent StreamName",
            serde_json::json!({ "StreamName": "probe.stream_7-ABSENT" }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
        (
            "a one-character absent StreamName",
            serde_json::json!({ "StreamName": "x" }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
        (
            "a StreamARN no stream answers to",
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent/1761000000"
            }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
        (
            "an absent StreamARN in another region and account",
            serde_json::json!({
                "StreamARN": "arn:aws-cn:kinesisvideo:cn-north-1:123456789012:stream/gone/1",
            }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
    ];

    for (path, extra) in read_loop_operations() {
        for (case, stream, status, error, expected) in &cases {
            let body = with_members(stream, &extra);
            let response = harness.post(path, &body).await;
            assert_eq!(
                response.status().as_u16(),
                *status,
                "case {path} / {case}: expected HTTP {status}, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let (code, message) = reported(&response);
            assert_eq!(
                code, *error,
                "case {path} / {case}: expected {error}, got {code:?}"
            );
            assert_eq!(
                message, *expected,
                "case {path} / {case}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// RULE: the not-found sentence is value-free — it names neither the stream the
/// request asked for nor the member that addressed it — so no part of the
/// request can be read out of it.
#[tokio::test]
async fn the_not_found_sentence_carries_nothing_of_the_request() {
    let harness = seeded();
    // Names and ARNs whose substrings would be visible in any message built
    // from the request.
    let probes = [
        ("a StreamName", "StreamName", "sim2real-probe-kvam-lf-missing"),
        (
            "a StreamARN",
            "StreamARN",
            "arn:aws:kinesisvideo:us-west-2:000000000000:stream/probe-missing/1699999999",
        ),
    ];
    for (case, member, value) in probes {
        let body = serde_json::json!({
            member: value,
            "Fragments": [fragment_number(0)],
        });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            404,
            "case {case}: expected HTTP 404, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (_, message) = reported(&response);
        assert_eq!(
            message, STREAM_NOT_FOUND,
            "case {case}: expected {STREAM_NOT_FOUND:?}, got {message:?}"
        );
        for leaked in [value, member] {
            assert!(
                !message.contains(leaked),
                "case {case}: the message must carry nothing of the request, but {message:?} \
                 contains {leaked:?}"
            );
        }
    }
}

/// The sentence a range read by SERVER timestamps whose end lies ahead of the
/// clock is refused with.
///
/// Worded with the clause the three ranges that carry it state —
/// `smithy_query(query="doc",shape="HLSTimestampRange",member="EndTimestamp")`,
/// and the same sentence on `DASHTimestampRange$EndTimestamp` and
/// `ClipTimestampRange$EndTimestamp`: "If FragmentSelectorType for the request
/// is SERVER_TIMESTAMP, this value must be in the past."
const SERVER_END_AHEAD_OF_THE_CLOCK: &str =
    "EndTimestamp must be in the past when FragmentSelectorType is SERVER_TIMESTAMP.";

/// How wide, in seconds, a range whose end carries that clause may be: the
/// neighbouring clause in the same paragraph bounds it to 24 hours, so a row
/// that places its end a day ahead of the clock stays inside that bound and is
/// decided by the clause under test alone.
const MAX_RANGE_SPAN_SECS: i64 = 24 * 60 * 60;

/// One operation whose timestamp range's end carries the clause, with the
/// playback mode its request states (none for the operation that has no mode).
struct ClockBoundedRange {
    /// The operation's route.
    path: &'static str,
    /// The input member it carries its fragment selector in.
    selector_member: &'static str,
    /// The `PlaybackMode` the request states, for an operation that has one.
    playback_mode: Option<&'static str>,
}

/// Every operation of this service whose range end carries the clause, in each
/// playback mode that admits a bounded range.
const CLOCK_BOUNDED_RANGES: &[ClockBoundedRange] = &[
    ClockBoundedRange {
        path: "/getHLSStreamingSessionURL",
        selector_member: "HLSFragmentSelector",
        playback_mode: Some("ON_DEMAND"),
    },
    ClockBoundedRange {
        path: "/getHLSStreamingSessionURL",
        selector_member: "HLSFragmentSelector",
        playback_mode: Some("LIVE_REPLAY"),
    },
    ClockBoundedRange {
        path: "/getDASHStreamingSessionURL",
        selector_member: "DASHFragmentSelector",
        playback_mode: Some("ON_DEMAND"),
    },
    ClockBoundedRange {
        path: "/getDASHStreamingSessionURL",
        selector_member: "DASHFragmentSelector",
        playback_mode: Some("LIVE_REPLAY"),
    },
    ClockBoundedRange {
        path: "/getClip",
        selector_member: "ClipFragmentSelector",
        playback_mode: None,
    },
];

/// What a row expects of the operation it is driven through.
#[derive(Debug, Clone, Copy)]
enum EndExpectation {
    /// The clause refuses the range: HTTP 400 `InvalidArgumentException` with
    /// [`SERVER_END_AHEAD_OF_THE_CLOCK`].
    Refused,
    /// The clause admits the range, and the addressed archive holds no fragment
    /// in it: HTTP 404 `ResourceNotFoundException`.
    NoFragments,
    /// The clause admits the range and the archive answers over it.
    Served,
}

/// What an operation of [`CLOCK_BOUNDED_RANGES`] is asked over the range
/// `[start_secs, end_secs]`, selecting on `selector_type`.
fn clock_bounded_request(
    operation: &ClockBoundedRange,
    selector_type: &str,
    start_secs: i64,
    end_secs: i64,
) -> serde_json::Value {
    let mut body = serde_json::json!({ "StreamName": STREAM });
    if let Some(mode) = operation.playback_mode {
        body["PlaybackMode"] = serde_json::json!(mode);
    }
    body[operation.selector_member] = serde_json::json!({
        "FragmentSelectorType": selector_type,
        "TimestampRange": {
            "StartTimestamp": start_secs,
            "EndTimestamp": end_secs,
        },
    });
    body
}

/// Epoch seconds now, the instant every row places its end relative to.
fn now_secs() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the Unix epoch");
    i64::try_from(since_epoch.as_secs()).expect("an epoch second this archive can hold")
}

/// RULE: a timestamp range whose `EndTimestamp` is documented as having to be
/// "in the past" when `FragmentSelectorType` is `SERVER_TIMESTAMP` admits an end
/// at or before the instant the request is decided at and refuses a later one —
/// `InvalidArgumentException` (400) with one fixed sentence carrying nothing of
/// the request — in every operation whose range carries that clause; a range
/// read by producer timestamp, which the service does not stamp, is not bounded
/// by the clock at all.
///
/// The rows differ along what the rule generalises over: which timestamp the
/// selector reads, how far ahead of or behind the clock the end lies (a day, an
/// hour, seconds, exactly at it, and well in the past), and the operation and
/// playback mode the range is asked for in.
#[tokio::test]
async fn a_server_timestamp_range_ending_ahead_of_the_clock_is_refused_wherever_it_is_bounded() {
    let harness = seeded();
    let now = now_secs();
    // (case, FragmentSelectorType, StartTimestamp, EndTimestamp, expectation)
    let rows: &[(&str, &str, i64, i64, EndExpectation)] = &[
        (
            "a day ahead of the clock, on server timestamps",
            "SERVER_TIMESTAMP",
            now,
            now + MAX_RANGE_SPAN_SECS,
            EndExpectation::Refused,
        ),
        (
            "an hour ahead of the clock, on server timestamps",
            "SERVER_TIMESTAMP",
            now - 3600,
            now + 3600,
            EndExpectation::Refused,
        ),
        (
            "ten seconds ahead of the clock, on server timestamps",
            "SERVER_TIMESTAMP",
            now - 3600,
            now + 10,
            EndExpectation::Refused,
        ),
        (
            // The clause bounds an end AHEAD of the clock, so an end at it is
            // admitted and the archive decides the answer.
            "at the clock, on server timestamps",
            "SERVER_TIMESTAMP",
            now - 3600,
            now,
            EndExpectation::NoFragments,
        ),
        (
            "well in the past, on server timestamps, over the seeded archive",
            "SERVER_TIMESTAMP",
            T0_SECS,
            T0_SECS + 1000,
            EndExpectation::Served,
        ),
        (
            // The clause names the server-stamped selector only: a producer
            // stamps its own timestamps, so the service bounds neither.
            "a day ahead of the clock, on producer timestamps",
            "PRODUCER_TIMESTAMP",
            now,
            now + MAX_RANGE_SPAN_SECS,
            EndExpectation::NoFragments,
        ),
        (
            "an hour ahead of the clock, on producer timestamps",
            "PRODUCER_TIMESTAMP",
            now - 3600,
            now + 3600,
            EndExpectation::NoFragments,
        ),
        (
            "well in the past, on producer timestamps, over the seeded archive",
            "PRODUCER_TIMESTAMP",
            T0_SECS,
            T0_SECS + 1000,
            EndExpectation::Served,
        ),
    ];

    for operation in CLOCK_BOUNDED_RANGES {
        let mode = operation.playback_mode.unwrap_or("no playback mode");
        for (case, selector_type, start, end, expect) in rows {
            let case = format!("{case} over {} in {mode}", operation.path);
            let body = clock_bounded_request(operation, selector_type, *start, *end);
            let response = harness.post(operation.path, &body).await;
            let status = response.status().as_u16();
            match expect {
                EndExpectation::Refused => {
                    assert_eq!(
                        status,
                        400,
                        "case {case}: expected HTTP 400, got {status} with body {:?}",
                        String::from_utf8_lossy(response.body())
                    );
                    let (code, message) = reported(&response);
                    assert_eq!(
                        code, "InvalidArgumentException",
                        "case {case}: expected InvalidArgumentException, got {code:?}"
                    );
                    assert_eq!(
                        message, SERVER_END_AHEAD_OF_THE_CLOCK,
                        "case {case}: expected {SERVER_END_AHEAD_OF_THE_CLOCK:?}, got {message:?}"
                    );
                    // The sentence is fixed: nothing the request carried can be
                    // read back out of it.
                    for carried in [STREAM.to_string(), start.to_string(), end.to_string()] {
                        assert!(
                            !message.contains(&carried),
                            "case {case}: the refusal must carry nothing of the request, but \
                             {message:?} contains {carried:?}"
                        );
                    }
                }
                EndExpectation::NoFragments => {
                    assert_eq!(
                        status,
                        404,
                        "case {case}: expected HTTP 404, got {status} with body {:?}",
                        String::from_utf8_lossy(response.body())
                    );
                    let (code, message) = reported(&response);
                    assert_eq!(
                        code, "ResourceNotFoundException",
                        "case {case}: expected ResourceNotFoundException, got {code:?}"
                    );
                    assert_ne!(
                        message, SERVER_END_AHEAD_OF_THE_CLOCK,
                        "case {case}: the clause admits this range, so its refusal must not be \
                         reported, got {message:?}"
                    );
                }
                EndExpectation::Served => {
                    assert_eq!(
                        status,
                        200,
                        "case {case}: expected HTTP 200, got {status} with body {:?}",
                        String::from_utf8_lossy(response.body())
                    );
                }
            }
        }
    }
}
