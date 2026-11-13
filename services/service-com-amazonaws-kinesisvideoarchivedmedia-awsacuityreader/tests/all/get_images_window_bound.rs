// SPDX-License-Identifier: CC0-1.0

//! The ARGUMENT rules `GetImages` decides a request by BEFORE any archive is read,
//! pinned as rules over a set of requests rather than over one request:
//!
//! 1. a window wider than the documented maximum is refused with the modelled client
//!    error and ONE fixed sentence whose only parameterised part is that documented
//!    limit — no stream name, no timestamp, no computed width is interpolated into it;
//! 2. a window ON the documented maximum, or narrower, is never refused for its width,
//!    whatever the archive then answers;
//! 3. an argument refusal is decided from the request alone, so it answers a request
//!    that also names an archive holding nothing — and a request breaking BOTH argument
//!    rules is still refused at the argument level, never as a missing resource.
//!
//! The harness, the request helpers and the seeded streams are `get_images_generation`'s,
//! so these suites read the same archive the rest of the `GetImages` tests do.

use crate::get_images_generation::{
    harness, images, producer_secs, reported, with, EMPTY_STREAM, IMAGES_STREAM,
    INCOMPLETE_RESIZE, INVALID_ARGUMENT, NO_FRAGMENTS, WINDOW_TOO_WIDE,
};

/// The widest window the model admits, in seconds.
///
/// Read off the model, not off a request: `smithy_query(query="doc",
/// shape="GetImagesInput", member="EndTimestamp")` states "If the time range between
/// StartTimestamp and EndTimestamp is more than 300 seconds above StartTimestamp, you
/// will receive an IllegalArgumentException."
const MAX_WINDOW_SECS: i64 = 300;

/// The code a window no fragment falls in is refused with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// How far before the seeded archive a window that holds nothing starts, in seconds.
const BEFORE_THE_ARCHIVE_SECS: i64 = 3_600;

/// A `GetImages` body addressing `stream` (the stream-naming members) over a window of
/// `width_secs` starting at `start_secs`, carrying `extra` beside them.
fn window_request(
    stream: serde_json::Value,
    selector: &str,
    start_secs: i64,
    width_secs: i64,
    format: &str,
    extra: serde_json::Value,
) -> serde_json::Value {
    let base = serde_json::json!({
        "ImageSelectorType": selector,
        "StartTimestamp": start_secs,
        "EndTimestamp": start_secs + width_secs,
        "Format": format,
    });
    with(&with(&base, stream), extra)
}

/// RULE: a requested window wider than the documented maximum is refused HTTP 400
/// `InvalidArgumentException` with one fixed sentence, parameterised only by that
/// documented limit — nothing of the request (the stream it addresses, its timestamps,
/// how far past the limit it reaches) appears in the message.
///
/// The rows differ along what the rule generalises over: how far past the limit the
/// window reaches, where it sits on the time line, which member addresses the stream,
/// which timestamp field is selected on, the output format, and the extra members the
/// request carries.
#[tokio::test]
async fn a_window_past_the_documented_maximum_is_refused_with_one_fixed_sentence() {
    let harness = harness();
    let by_name = |stream: &str| serde_json::json!({ "StreamName": stream });
    // (case, the stream-naming members, selector, start, width in seconds, format, extra)
    let rows: Vec<(&str, serde_json::Value, &str, i64, i64, &str, serde_json::Value)> = vec![
        (
            "one second past the limit",
            by_name(IMAGES_STREAM),
            "PRODUCER_TIMESTAMP",
            producer_secs(0),
            MAX_WINDOW_SECS + 1,
            "JPEG",
            serde_json::json!({}),
        ),
        (
            "two minutes past the limit",
            by_name(IMAGES_STREAM),
            "PRODUCER_TIMESTAMP",
            producer_secs(1),
            420,
            "JPEG",
            serde_json::json!({}),
        ),
        (
            "twice the limit, read on server timestamps and asked for PNG",
            by_name(IMAGES_STREAM),
            "SERVER_TIMESTAMP",
            producer_secs(0),
            600,
            "PNG",
            serde_json::json!({}),
        ),
        (
            "an hour, on a stream whose archive holds nothing",
            by_name(EMPTY_STREAM),
            "PRODUCER_TIMESTAMP",
            producer_secs(0),
            BEFORE_THE_ARCHIVE_SECS,
            "JPEG",
            serde_json::json!({}),
        ),
        (
            "a day, decades before the seeded archive",
            by_name(IMAGES_STREAM),
            "PRODUCER_TIMESTAMP",
            946_684_800,
            86_400,
            "JPEG",
            serde_json::json!({}),
        ),
        (
            "an hour, addressed by StreamARN",
            serde_json::json!({ "StreamARN": harness.arn.clone() }),
            "PRODUCER_TIMESTAMP",
            producer_secs(0),
            BEFORE_THE_ARCHIVE_SECS,
            "JPEG",
            serde_json::json!({}),
        ),
        (
            "past the limit, beside a complete output size and a sampling interval",
            by_name(IMAGES_STREAM),
            "PRODUCER_TIMESTAMP",
            producer_secs(2),
            1_000,
            "JPEG",
            serde_json::json!({
                "WidthPixels": 320,
                "HeightPixels": 240,
                "SamplingInterval": 1_000,
                "MaxResults": 2,
            }),
        ),
        (
            "past the limit, beside a named JPEG quality",
            by_name(IMAGES_STREAM),
            "PRODUCER_TIMESTAMP",
            producer_secs(0),
            3_600,
            "JPEG",
            serde_json::json!({ "FormatConfig": { "JPEGQuality": "50" } }),
        ),
    ];

    for (case, stream, selector, start, width, format, extra) in rows {
        let body = window_request(stream.clone(), selector, start, width, format, extra);
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (code, message) = reported(&response);
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_ARGUMENT, WINDOW_TOO_WIDE),
            "case {case:?}: expected ({INVALID_ARGUMENT:?}, {WINDOW_TOO_WIDE:?}), got \
             ({code:?}, {message:?})"
        );
        // The sentence is fixed: the only number in it is the documented limit, so no
        // value this request carried can be read back out of the refusal.
        let addressed = stream
            .as_object()
            .and_then(|members| members.values().next())
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        for carried in [
            addressed,
            start.to_string(),
            (start + width).to_string(),
            width.to_string(),
        ] {
            assert!(
                !carried.is_empty() && !message.contains(&carried),
                "case {case:?}: the refusal must carry nothing of the request, but \
                 {message:?} contains {carried:?}"
            );
        }
    }
}

/// RULE: the documented maximum is INCLUSIVE — a window exactly that wide, or narrower,
/// is never refused for its width, and the answer is then whatever the addressed archive
/// holds.
///
/// The rows differ along the width (the published bound, one second inside it, the
/// narrowest window there is) and along whether the addressed archive answers the
/// window at all.
#[tokio::test]
async fn a_window_on_or_inside_the_documented_maximum_is_not_refused_for_its_width() {
    let harness = harness();
    // (case, stream, start, width in seconds, the status expected)
    let rows: Vec<(&str, &str, i64, i64, u16)> = vec![
        (
            "exactly the published bound, over an archive that answers",
            IMAGES_STREAM,
            producer_secs(0),
            MAX_WINDOW_SECS,
            200,
        ),
        (
            "one second inside the bound, over an archive that answers",
            IMAGES_STREAM,
            producer_secs(0),
            MAX_WINDOW_SECS - 1,
            200,
        ),
        (
            "exactly the published bound, over an archive holding nothing",
            EMPTY_STREAM,
            producer_secs(0),
            MAX_WINDOW_SECS,
            404,
        ),
        (
            "the narrowest window, over an archive holding nothing",
            EMPTY_STREAM,
            producer_secs(0),
            1,
            404,
        ),
        (
            "exactly the published bound, before the archive it addresses",
            IMAGES_STREAM,
            producer_secs(0) - BEFORE_THE_ARCHIVE_SECS,
            MAX_WINDOW_SECS,
            404,
        ),
    ];

    for (case, stream, start, width, status) in rows {
        let body = window_request(
            serde_json::json!({ "StreamName": stream }),
            "PRODUCER_TIMESTAMP",
            start,
            width,
            "JPEG",
            serde_json::json!({}),
        );
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 404 {
            let (code, message) = reported(&response);
            assert_eq!(
                (code.as_str(), message.as_str()),
                (NOT_FOUND, NO_FRAGMENTS),
                "case {case:?}: expected ({NOT_FOUND:?}, {NO_FRAGMENTS:?}), got \
                 ({code:?}, {message:?})"
            );
            continue;
        }
        assert!(
            !images(&response).is_empty(),
            "case {case:?}: expected a generated image, got {:?}",
            String::from_utf8_lossy(response.body())
        );
    }
}

/// What a request is expected to be answered with.
enum Expect {
    /// HTTP 400 `InvalidArgumentException` with exactly this sentence.
    Argument(&'static str),
    /// HTTP 400 `InvalidArgumentException` with one of the two argument sentences: the
    /// request breaks both rules, and which of the two answers is not a requirement —
    /// that it is refused at the argument level is.
    EitherArgument,
    /// The archive's own answer for a window it holds no fragment in.
    Archive,
}

/// RULE: both argument rules are decided from the request alone, so they answer before
/// the addressed archive is consulted — a request breaking one, or both, is refused
/// `InvalidArgumentException` (400) and never with the archive's missing-resource
/// refusal, while a request breaking neither falls through to the archive.
///
/// The rows cross the output-size pair (both sides, one side, neither) with the window
/// width (inside the documented maximum, past it) over archives that answer nothing, so
/// the ordering of the two checks against the lookup is observable rather than assumed.
#[tokio::test]
async fn an_argument_refusal_answers_before_the_archive_however_many_rules_are_broken() {
    let harness = harness();
    // (case, stream, start, width in seconds, the size members, what is expected)
    let rows: Vec<(&str, &str, i64, i64, serde_json::Value, Expect)> = vec![
        (
            "a width alone, inside the window bound",
            EMPTY_STREAM,
            producer_secs(0),
            1,
            serde_json::json!({ "WidthPixels": 320 }),
            Expect::Argument(INCOMPLETE_RESIZE),
        ),
        (
            "a height alone, inside the window bound",
            EMPTY_STREAM,
            producer_secs(0),
            MAX_WINDOW_SECS,
            serde_json::json!({ "HeightPixels": 240 }),
            Expect::Argument(INCOMPLETE_RESIZE),
        ),
        (
            "a complete pair, past the window bound",
            EMPTY_STREAM,
            producer_secs(0),
            MAX_WINDOW_SECS + 1,
            serde_json::json!({ "WidthPixels": 320, "HeightPixels": 240 }),
            Expect::Argument(WINDOW_TOO_WIDE),
        ),
        (
            "no size at all, past the window bound",
            EMPTY_STREAM,
            producer_secs(0),
            BEFORE_THE_ARCHIVE_SECS,
            serde_json::json!({}),
            Expect::Argument(WINDOW_TOO_WIDE),
        ),
        (
            "a width alone AND a window past the bound",
            EMPTY_STREAM,
            producer_secs(0),
            600,
            serde_json::json!({ "WidthPixels": 320 }),
            Expect::EitherArgument,
        ),
        (
            "a height alone AND a window past the bound, before a real archive",
            IMAGES_STREAM,
            producer_secs(0) - BEFORE_THE_ARCHIVE_SECS,
            600,
            serde_json::json!({ "HeightPixels": 240 }),
            Expect::EitherArgument,
        ),
        (
            "a complete pair inside every bound",
            EMPTY_STREAM,
            producer_secs(0),
            MAX_WINDOW_SECS,
            serde_json::json!({ "WidthPixels": 320, "HeightPixels": 240 }),
            Expect::Archive,
        ),
        (
            "no size at all, inside every bound, before a real archive",
            IMAGES_STREAM,
            producer_secs(0) - BEFORE_THE_ARCHIVE_SECS,
            1,
            serde_json::json!({}),
            Expect::Archive,
        ),
    ];

    for (case, stream, start, width, size, expect) in rows {
        let body = window_request(
            serde_json::json!({ "StreamName": stream }),
            "PRODUCER_TIMESTAMP",
            start,
            width,
            "JPEG",
            size,
        );
        let response = harness.post("/getImages", &body).await;
        let status = response.status().as_u16();
        let (code, message) = reported(&response);
        let expected_status = match expect {
            Expect::Archive => 404,
            _ => 400,
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {:?}",
            String::from_utf8_lossy(response.body())
        );
        match expect {
            Expect::Argument(expected) => assert_eq!(
                (code.as_str(), message.as_str()),
                (INVALID_ARGUMENT, expected),
                "case {case:?}: expected ({INVALID_ARGUMENT:?}, {expected:?}), got \
                 ({code:?}, {message:?})"
            ),
            Expect::EitherArgument => {
                assert_eq!(
                    code, INVALID_ARGUMENT,
                    "case {case:?}: expected {INVALID_ARGUMENT:?}, got {code:?}"
                );
                assert!(
                    message == INCOMPLETE_RESIZE || message == WINDOW_TOO_WIDE,
                    "case {case:?}: expected one of ({INCOMPLETE_RESIZE:?}, \
                     {WINDOW_TOO_WIDE:?}), got {message:?}"
                );
            }
            Expect::Archive => assert_eq!(
                (code.as_str(), message.as_str()),
                (NOT_FOUND, NO_FRAGMENTS),
                "case {case:?}: expected ({NOT_FOUND:?}, {NO_FRAGMENTS:?}), got \
                 ({code:?}, {message:?})"
            ),
        }
    }
}
