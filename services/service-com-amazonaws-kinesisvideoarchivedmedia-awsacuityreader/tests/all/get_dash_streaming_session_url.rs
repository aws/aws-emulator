// SPDX-License-Identifier: CC0-1.0

//! `GetDASHStreamingSessionURL`, end to end over HTTP.
//!
//! The harness and the seeded archive are `archived_media_read_path`'s — the
//! real `MultiServiceRouter` with this one service registered — so every
//! assertion below is about what the handler computes FROM that archive and
//! records in the service's own state.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;

use archived_media::state::{
    from_epoch_millis, service_state, ArchivedStream, SessionProtocol, StoredFragment,
    StreamingSession,
};

use crate::archived_media_read_path::{
    body_json, fragment_number, media, register_stream, seeded, Harness, EMULATOR_ENDPOINT,
    LIVE_FRAGMENTS, LIVE_STREAM, NO_RETENTION_STREAM, STREAM, T0_SECS,
};

/// The route this suite drives.
const PATH: &str = "/getDASHStreamingSessionURL";

/// The sentence a session with nothing to serve is refused with, as
/// `ResourceNotFoundException`'s own documentation describes the two cases that
/// raise it.
const NO_FRAGMENTS: &str = "No fragments found in the stream for the streaming request.";

/// The `Expires` bounds the model publishes for this operation
/// (`smithy_query(query="operation",operation="GetDASHStreamingSessionURL")`:
/// `Expires` range 300..43200).
const EXPIRES_MIN: i64 = 300;
/// The upper `Expires` bound of that same range.
const EXPIRES_MAX: i64 = 43200;

/// The session the URL in `response_body` names, read back out of the service's
/// own state, with the token the URL carries.
fn recorded_session(harness: &Harness, url: &str) -> StreamingSession {
    let token = url
        .rsplit("SessionToken=")
        .next()
        .expect("the URL carries a session token")
        .to_string();
    let state = service_state(&*harness.engine).expect("the service store");
    let state = state.lock();
    state
        .sessions
        .get(&token)
        .cloned()
        .unwrap_or_else(|| panic!("no session recorded for token {token:?}"))
}

/// The `DASHStreamingSessionURL` of a successful response.
fn session_url(response: &http::Response<bytes::Bytes>) -> String {
    body_json(response)["DASHStreamingSessionURL"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The error code a refusal carries.
fn error_code(response: &http::Response<bytes::Bytes>) -> String {
    body_json(response)["__type"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The error message a refusal carries.
fn error_message(response: &http::Response<bytes::Bytes>) -> String {
    let reported = body_json(response);
    reported["message"]
        .as_str()
        .or_else(|| reported["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// RULE: a session is minted over the fragments the playback mode and selector
/// select — those of the requested range for ON_DEMAND and LIVE_REPLAY, the
/// most recently ingested for LIVE — capped by `MaxManifestFragmentResults`
/// (default 5 for LIVE / LIVE_REPLAY, 1,000 for ON_DEMAND), which at the MINT
/// is filled from the start of the range for either range mode and from the
/// stream head for LIVE. The URL returned resolves to that recorded session.
///
/// The rows differ along what the rule generalises over: the playback mode, the
/// timestamp the selector reads, whether the range is closed or open-ended, and
/// the cap on the snapshot.
#[tokio::test]
async fn a_minted_session_serves_the_fragments_its_mode_and_selector_select() {
    // (case, request body, the seeded fragment indices the session must serve)
    let rows: Vec<(&str, serde_json::Value, Vec<u64>)> = vec![
        (
            "ON_DEMAND over producer timestamps, uncapped",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
                },
            }),
            vec![0, 1, 2, 3, 4],
        ),
        (
            "ON_DEMAND over server timestamps, capped at two",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "MaxManifestFragmentResults": 2,
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "SERVER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
                },
            }),
            vec![0, 1],
        ),
        (
            "ON_DEMAND over a range that starts inside the archive",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS + 20,
                        "EndTimestamp": T0_SECS + 30,
                    },
                },
            }),
            vec![2, 3],
        ),
        (
            "LIVE_REPLAY with an open-ended range, at the LIVE default of five",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            vec![0, 1, 2, 3, 4],
        ),
        (
            // A LIVE_REPLAY playlist "starts by including fragments from a
            // given start time", so its maximum is filled from the front of
            // the range at the mint.
            "LIVE_REPLAY capped at three",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "MaxManifestFragmentResults": 3,
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "SERVER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
                },
            }),
            vec![0, 1, 2],
        ),
        (
            "LIVE over a freshly ingested archive, at the LIVE default of five",
            serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
            ((LIVE_FRAGMENTS - 5)..LIVE_FRAGMENTS).collect(),
        ),
        (
            "LIVE capped at two",
            serde_json::json!({
                "StreamName": LIVE_STREAM,
                "PlaybackMode": "LIVE",
                "MaxManifestFragmentResults": 2,
            }),
            ((LIVE_FRAGMENTS - 2)..LIVE_FRAGMENTS).collect(),
        ),
    ];

    for (case, body, snapshot) in rows {
        let harness = seeded();
        let response = harness.post(PATH, &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );

        // The URL comes back to THIS emulator — the endpoint the request was
        // addressed to — and names the MPEG-DASH manifest resource.
        let url = session_url(&response);
        let prefix = format!("https://{EMULATOR_ENDPOINT}/dash/v1/getMPDManifest.mpd?SessionToken=");
        assert!(
            url.starts_with(&prefix),
            "case {case}: expected a URL starting {prefix:?}, got {url:?}"
        );
        assert!(
            url.len() > prefix.len(),
            "case {case}: the URL must carry a session token, got {url:?}"
        );

        let session = recorded_session(&harness, &url);
        assert_eq!(
            session.protocol,
            SessionProtocol::Dash,
            "case {case}: the session must be an MPEG-DASH one, got {session:?}"
        );
        assert_eq!(
            Some(session.stream_name.as_str()),
            body["StreamName"].as_str(),
            "case {case}: the session must name the requested stream, got {session:?}"
        );
        assert_eq!(
            session.fragment_numbers,
            snapshot
                .iter()
                .copied()
                .map(fragment_number)
                .collect::<Vec<_>>(),
            "case {case}: wrong manifest snapshot, got {session:?}"
        );
    }
}

/// RULE: the documented parameter defaults are what an unspecified member means
/// — `Expires` 300 seconds, `DisplayFragmentTimestamp` NEVER and
/// `DisplayFragmentNumber` NEVER — and a member the request does name is the
/// one recorded on the session.
///
/// The rows differ along which of the three members the request carries and
/// which value it asks for.
#[tokio::test]
async fn the_documented_defaults_apply_and_a_stated_parameter_is_recorded() {
    // (case, the members added to the request, expected Expires, expected
    // DisplayFragmentTimestamp, expected DisplayFragmentNumber)
    let rows: Vec<(&str, serde_json::Value, u32, &str, &str)> = vec![
        (
            "every optional member unspecified",
            serde_json::json!({}),
            300,
            "NEVER",
            "NEVER",
        ),
        (
            "fragment timestamps displayed",
            serde_json::json!({ "DisplayFragmentTimestamp": "ALWAYS" }),
            300,
            "ALWAYS",
            "NEVER",
        ),
        (
            "fragment numbers displayed",
            serde_json::json!({ "DisplayFragmentNumber": "ALWAYS" }),
            300,
            "NEVER",
            "ALWAYS",
        ),
        (
            "both displayed, at the Expires minimum",
            serde_json::json!({
                "DisplayFragmentTimestamp": "ALWAYS",
                "DisplayFragmentNumber": "ALWAYS",
                "Expires": EXPIRES_MIN,
            }),
            300,
            "ALWAYS",
            "ALWAYS",
        ),
        (
            "at the Expires maximum",
            serde_json::json!({ "Expires": EXPIRES_MAX }),
            43200,
            "NEVER",
            "NEVER",
        ),
        (
            "an Expires between the bounds",
            serde_json::json!({ "Expires": 3600 }),
            3600,
            "NEVER",
            "NEVER",
        ),
    ];

    for (case, extra, expires, timestamp, number) in rows {
        let harness = seeded();
        let mut body = serde_json::json!({
            "StreamName": STREAM,
            "PlaybackMode": "ON_DEMAND",
            "DASHFragmentSelector": {
                "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
            },
        });
        for (key, value) in extra.as_object().expect("an object of members") {
            body[key.clone()] = value.clone();
        }

        let response = harness.post(PATH, &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let session = recorded_session(&harness, &session_url(&response));
        assert_eq!(
            session.expires_in_secs, expires,
            "case {case}: expected Expires {expires}, got {session:?}"
        );
        assert_eq!(
            session.display_fragment_timestamp, timestamp,
            "case {case}: expected DisplayFragmentTimestamp {timestamp}, got {session:?}"
        );
        assert_eq!(
            session.display_fragment_number.as_deref(),
            Some(number),
            "case {case}: expected DisplayFragmentNumber {number}, got {session:?}"
        );
        // MPEG-DASH models no `ContainerFormat` and no `DiscontinuityMode`: the
        // manifest serves MP4 fragments, and the HLS-only member is unset.
        assert_eq!(
            session.container_format, "FRAGMENTED_MP4",
            "case {case}: an MPEG-DASH session serves fragmented MP4, got {session:?}"
        );
        assert_eq!(
            session.discontinuity_mode, None,
            "case {case}: MPEG-DASH carries no DiscontinuityMode, got {session:?}"
        );
        assert!(
            session.expires_at() > SystemTime::now(),
            "case {case}: a freshly minted session must still be inside its window, got {session:?}"
        );
    }
}

/// RULE: `Expires` is bounded at 300 and 43,200 seconds by the model, so a
/// value outside that range is refused 400 `ValidationException` before the
/// handler runs — and nothing is recorded for a refused request.
///
/// The rows are each published bound and one past it, on both ends.
#[tokio::test]
async fn an_expires_outside_the_published_bounds_is_refused() {
    // (case, the Expires the request carries, the status expected)
    let rows: [(&str, i64, u16); 6] = [
        ("one second under the minimum", EXPIRES_MIN - 1, 400),
        ("exactly at the minimum", EXPIRES_MIN, 200),
        ("one second over the minimum", EXPIRES_MIN + 1, 200),
        ("one second under the maximum", EXPIRES_MAX - 1, 200),
        ("exactly at the maximum", EXPIRES_MAX, 200),
        ("one second over the maximum", EXPIRES_MAX + 1, 400),
    ];

    for (case, expires, status) in rows {
        let harness = seeded();
        let body = serde_json::json!({
            "StreamName": STREAM,
            "PlaybackMode": "ON_DEMAND",
            "Expires": expires,
            "DASHFragmentSelector": {
                "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
            },
        });
        let response = harness.post(PATH, &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 400 {
            let code = error_code(&response);
            assert_eq!(
                code, "ValidationException",
                "case {case}: expected ValidationException, got {code:?}"
            );
            let state = service_state(&*harness.engine).expect("the service store");
            let state = state.lock();
            assert!(
                state.sessions.is_empty(),
                "case {case}: a refused request must record no session, held {:?}",
                state.sessions.keys().collect::<Vec<_>>()
            );
        }
    }
}

/// RULE: the playback mode decides which selector a session may carry —
/// `DASHFragmentSelector` with a `TimestampRange` is required for ON_DEMAND and
/// LIVE_REPLAY, `EndTimestamp` is required for ON_DEMAND and optional for
/// LIVE_REPLAY, the end must be later than the start, and a LIVE session must
/// carry no `TimestampRange`.
#[tokio::test]
async fn the_selector_rules_of_each_mode_are_enforced() {
    let harness = seeded();
    // (case, request body, the status expected)
    let rows: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "ON_DEMAND with no selector",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "ON_DEMAND" }),
            400,
        ),
        (
            "LIVE_REPLAY with no selector",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "LIVE_REPLAY" }),
            400,
        ),
        (
            "ON_DEMAND with a selector carrying no range",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": { "FragmentSelectorType": "SERVER_TIMESTAMP" },
            }),
            400,
        ),
        (
            "ON_DEMAND with no EndTimestamp",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            400,
        ),
        (
            "ON_DEMAND with an end that precedes the start",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS + 10,
                        "EndTimestamp": T0_SECS,
                    },
                },
            }),
            400,
        ),
        (
            "LIVE_REPLAY with an end equal to the start",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS,
                    },
                },
            }),
            400,
        ),
        (
            "LIVE with a TimestampRange",
            serde_json::json!({
                "StreamName": LIVE_STREAM,
                "PlaybackMode": "LIVE",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "SERVER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
            400,
        ),
        (
            "LIVE with a selector carrying only a selector type",
            serde_json::json!({
                "StreamName": LIVE_STREAM,
                "PlaybackMode": "LIVE",
                "DASHFragmentSelector": { "FragmentSelectorType": "PRODUCER_TIMESTAMP" },
            }),
            200,
        ),
        (
            "LIVE_REPLAY with an open-ended range",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            200,
        ),
    ];

    for (case, body, status) in rows {
        let response = harness.post(PATH, &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 400 {
            let code = error_code(&response);
            assert_eq!(
                code, "InvalidArgumentException",
                "case {case}: expected InvalidArgumentException, got {code:?}"
            );
        }
    }
}

/// RULE: a session that would serve nothing does not exist —
/// `ResourceNotFoundException` (404) with one fixed sentence for ON_DEMAND /
/// LIVE_REPLAY with no fragment in the requested range, and for LIVE with no
/// fragment in the last 30 seconds — while the same stream in a mode that does
/// select fragments is served.
#[tokio::test]
async fn a_session_with_no_fragments_to_serve_is_not_found() {
    // (case, request body, the status expected)
    let rows: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "LIVE over an archive older than the live window",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "LIVE" }),
            404,
        ),
        (
            "LIVE over a freshly ingested archive",
            serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
            200,
        ),
        (
            "ON_DEMAND over a range before the archive",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS - 1000,
                        "EndTimestamp": T0_SECS - 1,
                    },
                },
            }),
            404,
        ),
        (
            "ON_DEMAND over a range after the archive",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        // Inside the 24 hours the range's own `EndTimestamp`
                        // admits, so this row is answered by the archive rather
                        // than refused for its span.
                        "StartTimestamp": T0_SECS + 100_000,
                        "EndTimestamp": T0_SECS + 150_000,
                    },
                },
            }),
            404,
        ),
        (
            "LIVE_REPLAY over a range the server timestamps miss",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "SERVER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS - 1000,
                        "EndTimestamp": T0_SECS - 1,
                    },
                },
            }),
            404,
        ),
        (
            "LIVE_REPLAY over the same range the producer timestamps hit",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
            200,
        ),
    ];

    for (case, body, status) in rows {
        let harness = seeded();
        let response = harness.post(PATH, &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 404 {
            let code = error_code(&response);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {code:?}"
            );
            let message = error_message(&response);
            assert_eq!(
                message, NO_FRAGMENTS,
                "case {case}: expected {NO_FRAGMENTS:?}, got {message:?}"
            );
            let state = service_state(&*harness.engine).expect("the service store");
            let state = state.lock();
            assert!(
                state.sessions.is_empty(),
                "case {case}: a session that serves nothing must not be recorded, held {:?}",
                state.sessions.keys().collect::<Vec<_>>()
            );
        }
    }
}

/// RULE: "Data retention must be greater than 0" — a stream whose
/// `DataRetentionInHours` is 0 cannot have a streaming session, in any playback
/// mode, and the refusal is `NoDataRetentionException` (400).
#[tokio::test]
async fn a_stream_that_retains_no_data_has_no_session() {
    let harness = seeded();
    // (case, the mode-specific members the request carries)
    let rows: Vec<(&str, serde_json::Value)> = vec![
        ("LIVE", serde_json::json!({ "PlaybackMode": "LIVE" })),
        (
            "ON_DEMAND",
            serde_json::json!({
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
                },
            }),
        ),
        (
            "LIVE_REPLAY",
            serde_json::json!({
                "PlaybackMode": "LIVE_REPLAY",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "SERVER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
        ),
    ];

    for (case, extra) in rows {
        let mut body = serde_json::json!({ "StreamName": NO_RETENTION_STREAM });
        for (key, value) in extra.as_object().expect("an object of members") {
            body[key.clone()] = value.clone();
        }
        let response = harness.post(PATH, &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let code = error_code(&response);
        assert_eq!(
            code, "NoDataRetentionException",
            "case {case}: expected NoDataRetentionException, got {code:?}"
        );
    }
}

/// RULE: every call MINTS a session — the operation's own documentation says
/// "Kinesis Video Streams creates an MPEG-DASH streaming session" — so two
/// calls with the same parameters answer two different URLs and both sessions
/// are recorded, each with its own token and its own snapshot.
#[tokio::test]
async fn each_call_mints_a_distinct_session() {
    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": STREAM,
        "PlaybackMode": "ON_DEMAND",
        "DASHFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });

    let mut urls = Vec::new();
    for call in 0..3 {
        let response = harness.post(PATH, &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case call {call}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        urls.push(session_url(&response));
    }

    for (call, url) in urls.iter().enumerate() {
        let session = recorded_session(&harness, url);
        assert_eq!(
            session.protocol,
            SessionProtocol::Dash,
            "case call {call}: expected an MPEG-DASH session, got {session:?}"
        );
    }
    let distinct: std::collections::BTreeSet<&String> = urls.iter().collect();
    assert_eq!(
        distinct.len(),
        urls.len(),
        "case three calls: each call must mint its own session, got {urls:?}"
    );

    let state = service_state(&*harness.engine).expect("the service store");
    let state = state.lock();
    assert_eq!(
        state.sessions.len(),
        urls.len(),
        "case three calls: every minted session must be recorded, held {:?}",
        state.sessions.keys().collect::<Vec<_>>()
    );
}

/// RULE: a stream the control plane does not hold has no session —
/// `ResourceNotFoundException` (404) — and a request that names neither or both
/// identifiers is `InvalidArgumentException` (400). The LIVE mode reaches the
/// same stream-resolution rule as the modes that carry a selector.
#[tokio::test]
async fn the_stream_is_resolved_by_the_same_rule_in_every_mode() {
    let harness = seeded();
    // (case, the stream-addressing members, the status expected, the error code)
    let rows: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "neither identifier",
            serde_json::json!({}),
            400,
            "InvalidArgumentException",
        ),
        (
            "both identifiers",
            serde_json::json!({ "StreamName": STREAM, "StreamARN": harness.arn.clone() }),
            400,
            "InvalidArgumentException",
        ),
        (
            "a name no stream answers to",
            serde_json::json!({ "StreamName": "absent-stream" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN no stream answers to",
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent/1761000000",
            }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, stream, status, code) in rows {
        for mode in ["LIVE", "ON_DEMAND"] {
            let mut body = serde_json::json!({ "PlaybackMode": mode });
            if mode == "ON_DEMAND" {
                body["DASHFragmentSelector"] = serde_json::json!({
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
                });
            }
            for (key, value) in stream.as_object().expect("an object of members") {
                body[key.clone()] = value.clone();
            }
            let response = harness.post(PATH, &body).await;
            assert_eq!(
                response.status().as_u16(),
                status,
                "case {case} in {mode}: expected HTTP {status}, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let reported = error_code(&response);
            assert_eq!(
                reported, code,
                "case {case} in {mode}: expected {code}, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Fragments that share one producer start timestamp.
// ---------------------------------------------------------------------------
//
// The selection rule below is stated for the manifest this operation mints a
// session over AND, in the same words, for the HLS media playlist of
// `GetHLSStreamingSessionURL`, which reaches the same selection. The rows
// therefore drive BOTH operations, so neither can drift from the other.

/// The region the streams this suite seeds itself are created in: the one the
/// harness signs its requests for.
const REGION: &str = "us-east-1";

/// The stream the archived collapse rows select from.
const SHARED_TIMESTAMP_STREAM: &str = "shared-timestamp-stream";

/// The stream the LIVE collapse rows select from: fragments ingested just now,
/// so a LIVE session over it exists.
const LIVE_SHARED_TIMESTAMP_STREAM: &str = "live-shared-timestamp-stream";

/// The two streaming-session operations that share this selection rule: the
/// path, the input's fragment-selector member, its maximum-fragment member, the
/// output member carrying the session URL, and the protocol the recorded
/// session names.
const SESSION_OPERATIONS: &[(&str, &str, &str, &str, SessionProtocol)] = &[
    (
        "/getDASHStreamingSessionURL",
        "DASHFragmentSelector",
        "MaxManifestFragmentResults",
        "DASHStreamingSessionURL",
        SessionProtocol::Dash,
    ),
    (
        "/getHLSStreamingSessionURL",
        "HLSFragmentSelector",
        "MaxMediaPlaylistFragmentResults",
        "HLSStreamingSessionURL",
        SessionProtocol::Hls,
    ),
];

/// The archive the collapse rows select from: the fragment's index (its number
/// is `fragment_number(index)`, which rises with it), its producer start
/// timestamp and its server timestamp as offsets in SECONDS from [`T0_SECS`],
/// and its duration in milliseconds.
///
/// Two groups share a producer start timestamp (a pair and a trio), one
/// fragment stands alone, one pair shares a SERVER timestamp while carrying
/// distinct producer ones, and one pair carries producer timestamps a second
/// apart with five-second durations — fragments "that have different timestamps
/// but have overlapping durations".
const SHARED_TIMESTAMP_ARCHIVE: &[(u64, i64, i64, u64)] = &[
    (10, 500, 900, 2000),
    (11, 500, 901, 2000),
    (20, 510, 910, 2000),
    (21, 510, 911, 2000),
    (22, 510, 912, 2000),
    (30, 520, 920, 2000),
    (40, 530, 930, 2000),
    (41, 540, 930, 2000),
    (50, 550, 950, 5000),
    (51, 551, 951, 5000),
];

/// One row of the collapse table.
struct CollapseRow {
    /// What this row is about, named in every assertion.
    case: &'static str,
    /// The `PlaybackMode` the session is asked for.
    mode: &'static str,
    /// The `FragmentSelectorType` the selector reads.
    selector_type: &'static str,
    /// The range the selector asks for, as offsets in seconds from [`T0_SECS`],
    /// with the end left open when the row gives none.
    range: (i64, Option<i64>),
    /// The maximum-fragment member's value, when the row names one.
    max: Option<i64>,
    /// The indices of the fragments the session must serve, in order.
    expected: &'static [u64],
}

/// Register `stream` in both planes and append one fragment per row of
/// `fragments`: (the index its fragment number comes from, its producer start
/// timestamp, its server timestamp, its duration in milliseconds).
fn seed_fragments(
    harness: &Harness,
    stream: &str,
    fragments: &[(u64, SystemTime, SystemTime, u64)],
) {
    let state = service_state(&*harness.engine).expect("the service store");
    let mut state = state.lock();
    let archived = ArchivedStream::new(stream, REGION, from_epoch_millis(T0_SECS * 1000), 24);
    register_stream(&harness.engine, &archived);
    state.put_stream(archived);
    for (index, producer, server, duration_millis) in fragments.iter().copied() {
        assert!(
            state.append_fragment(
                stream,
                StoredFragment::new(
                    fragment_number(index),
                    producer,
                    server,
                    duration_millis,
                    media(index),
                ),
            ),
            "case seed {stream} fragment {index}: the seeded stream must exist"
        );
    }
}

/// [`SHARED_TIMESTAMP_ARCHIVE`] seeded into [`SHARED_TIMESTAMP_STREAM`].
fn seed_shared_timestamp_archive(harness: &Harness) {
    let fragments: Vec<(u64, SystemTime, SystemTime, u64)> = SHARED_TIMESTAMP_ARCHIVE
        .iter()
        .copied()
        .map(|(index, producer_secs, server_secs, duration_millis)| {
            (
                index,
                from_epoch_millis((T0_SECS + producer_secs) * 1000),
                from_epoch_millis((T0_SECS + server_secs) * 1000),
                duration_millis,
            )
        })
        .collect();
    seed_fragments(harness, SHARED_TIMESTAMP_STREAM, &fragments);
}

/// One streaming-session request over `stream`, as this row asks for it.
fn collapse_request(
    stream: &str,
    row: &CollapseRow,
    selector_member: &str,
    max_member: &str,
) -> serde_json::Value {
    let (start, end) = row.range;
    let mut range = serde_json::json!({ "StartTimestamp": T0_SECS + start });
    if let Some(end) = end {
        range["EndTimestamp"] = serde_json::json!(T0_SECS + end);
    }
    let mut body = serde_json::json!({
        "StreamName": stream,
        "PlaybackMode": row.mode,
    });
    body[selector_member] = serde_json::json!({
        "FragmentSelectorType": row.selector_type,
        "TimestampRange": range,
    });
    if let Some(max) = row.max {
        body[max_member] = serde_json::json!(max);
    }
    body
}

/// The session URL `member` of a successful response, failing `case` when the
/// call did not succeed.
fn minted_url(case: &str, response: &http::Response<bytes::Bytes>, member: &str) -> String {
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case}: expected HTTP 200, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    body_json(response)[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the response carries no {member}"))
        .to_string()
}

/// RULE: when a streaming session's fragment selector reads PRODUCER
/// timestamps, fragments of the stream that carry the SAME start timestamp are
/// collapsed to the one with the largest fragment number before the snapshot is
/// capped — "the other fragments are not included" — while fragments with
/// different start timestamps are all kept, overlapping durations and all, and
/// a selector reading server timestamps collapses nothing.
///
/// The rows differ along what the rule generalises over: how many fragments
/// share a timestamp (two, three, none), which timestamp the selector reads,
/// the playback mode, whether the range is closed or open-ended, and whether
/// the snapshot is capped below the size of the group.
#[tokio::test]
async fn fragments_sharing_one_producer_start_timestamp_collapse_to_the_newest() {
    let rows: &[CollapseRow] = &[
        CollapseRow {
            case: "a pair sharing one producer start timestamp, uncapped",
            mode: "ON_DEMAND",
            selector_type: "PRODUCER_TIMESTAMP",
            range: (499, Some(501)),
            max: None,
            expected: &[11],
        },
        CollapseRow {
            case: "that pair with the snapshot capped at one fragment",
            mode: "ON_DEMAND",
            selector_type: "PRODUCER_TIMESTAMP",
            range: (499, Some(501)),
            max: Some(1),
            expected: &[11],
        },
        CollapseRow {
            case: "a trio sharing one producer start timestamp",
            mode: "ON_DEMAND",
            selector_type: "PRODUCER_TIMESTAMP",
            range: (509, Some(511)),
            max: None,
            expected: &[22],
        },
        CollapseRow {
            case: "every group of the archive by producer timestamp",
            mode: "ON_DEMAND",
            selector_type: "PRODUCER_TIMESTAMP",
            range: (499, Some(560)),
            max: None,
            expected: &[11, 22, 30, 40, 41, 50, 51],
        },
        CollapseRow {
            case: "fragments whose durations overlap but whose timestamps differ",
            mode: "ON_DEMAND",
            selector_type: "PRODUCER_TIMESTAMP",
            range: (549, Some(553)),
            max: None,
            expected: &[50, 51],
        },
        CollapseRow {
            case: "a pair sharing one server timestamp, selected by server timestamp",
            mode: "ON_DEMAND",
            selector_type: "SERVER_TIMESTAMP",
            range: (929, Some(931)),
            max: None,
            expected: &[40, 41],
        },
        CollapseRow {
            case: "the whole archive by server timestamp",
            mode: "ON_DEMAND",
            selector_type: "SERVER_TIMESTAMP",
            range: (899, Some(960)),
            max: None,
            expected: &[10, 11, 20, 21, 22, 30, 40, 41, 50, 51],
        },
        CollapseRow {
            case: "LIVE_REPLAY from an open-ended start, at the default of five",
            mode: "LIVE_REPLAY",
            selector_type: "PRODUCER_TIMESTAMP",
            range: (499, None),
            max: None,
            expected: &[11, 22, 30, 40, 41],
        },
        CollapseRow {
            case: "LIVE_REPLAY capped at two fragments",
            mode: "LIVE_REPLAY",
            selector_type: "PRODUCER_TIMESTAMP",
            range: (499, Some(560)),
            max: Some(2),
            expected: &[11, 22],
        },
    ];

    for (path, selector_member, max_member, url_member, protocol) in SESSION_OPERATIONS {
        for row in rows {
            let case = format!("{} over {path}", row.case);
            let harness = seeded();
            seed_shared_timestamp_archive(&harness);
            let body = collapse_request(
                SHARED_TIMESTAMP_STREAM,
                row,
                selector_member,
                max_member,
            );
            let response = harness.post(path, &body).await;
            let url = minted_url(&case, &response, url_member);
            let session = recorded_session(&harness, &url);
            assert_eq!(
                session.protocol, *protocol,
                "case {case}: wrong session protocol, got {session:?}"
            );
            let expected: Vec<String> = row
                .expected
                .iter()
                .copied()
                .map(fragment_number)
                .collect();
            assert_eq!(
                session.fragment_numbers, expected,
                "case {case}: expected the snapshot {expected:?}, got {:?}",
                session.fragment_numbers
            );
        }
    }
}

/// RULE (the same one, in the mode that selects by ingestion): a LIVE session
/// whose selector reads producer timestamps also collapses the fragments that
/// share one start timestamp to the largest fragment number, and it does so
/// before the snapshot is capped; a LIVE session selecting by server timestamp
/// collapses nothing.
///
/// The two rows differ along which timestamp the selector reads, over one
/// freshly ingested archive in which one PAIR shares a producer start timestamp
/// while every server timestamp is distinct, and a third row puts the newer of
/// a sharing pair at the HEAD of the stream and its superseded twin at the
/// oldest ingestion, where the fragment that is kept must still be ordered and
/// capped by its own ingestion.
#[tokio::test]
async fn a_live_session_collapses_a_shared_producer_start_timestamp_too() {
    // The archive one row ingests: (the index its fragment number comes from,
    // how long before the row's one instant its producer start timestamp is, in
    // milliseconds, and how long before it was ingested).
    type LiveArchive = &'static [(u64, u64, u64)];
    // A pair sharing one producer start timestamp, ingested next to each other.
    let adjacent: LiveArchive = &[
        (60, 5_000, 5_000),
        (61, 3_000, 3_000),
        (62, 3_000, 2_000),
    ];
    // A pair sharing one producer start timestamp at the two ENDS of the
    // ingestion order: the superseded fragment is the oldest ingested and the
    // one that is kept is the stream head.
    let far_apart: LiveArchive = &[
        (70, 4_000, 9_000),
        (71, 7_000, 7_000),
        (72, 6_000, 5_000),
        (73, 4_000, 1_000),
    ];
    // (case, FragmentSelectorType, the archive, the fragment indices a playlist
    // of two fragments must carry)
    let rows: &[(&str, &str, LiveArchive, &[u64])] = &[
        (
            "a producer-timestamp selector collapses the pair",
            "PRODUCER_TIMESTAMP",
            adjacent,
            &[60, 62],
        ),
        (
            "a server-timestamp selector collapses nothing",
            "SERVER_TIMESTAMP",
            adjacent,
            &[61, 62],
        ),
        (
            "the fragment that is kept is the stream head",
            "PRODUCER_TIMESTAMP",
            far_apart,
            &[72, 73],
        ),
    ];

    for (path, selector_member, max_member, url_member, _protocol) in SESSION_OPERATIONS {
        for (case, selector_type, archive, expected) in rows {
            let case = format!("{case} over {path}");
            let harness = seeded();
            // Ingested just now, so the stream head is inside the LIVE window,
            // and spaced from ONE instant rather than from the wall clock
            // between statements.
            let now = SystemTime::now();
            let at = |millis_ago: u64| now - Duration::from_millis(millis_ago);
            let fragments: Vec<(u64, SystemTime, SystemTime, u64)> = archive
                .iter()
                .copied()
                .map(|(index, producer_ago, server_ago)| {
                    (index, at(producer_ago), at(server_ago), 1000)
                })
                .collect();
            seed_fragments(&harness, LIVE_SHARED_TIMESTAMP_STREAM, &fragments);

            let mut body = serde_json::json!({
                "StreamName": LIVE_SHARED_TIMESTAMP_STREAM,
                "PlaybackMode": "LIVE",
            });
            body[*selector_member] =
                serde_json::json!({ "FragmentSelectorType": selector_type });
            body[*max_member] = serde_json::json!(2);

            let response = harness.post(path, &body).await;
            let url = minted_url(&case, &response, url_member);
            let session = recorded_session(&harness, &url);
            let expected: Vec<String> = expected.iter().copied().map(fragment_number).collect();
            assert_eq!(
                session.fragment_numbers, expected,
                "case {case}: expected the snapshot {expected:?}, got {:?}",
                session.fragment_numbers
            );
        }
    }
}

/// RULE (the same one, as a PLAYER sees it): the manifest the session serves
/// names the fragment with the larger fragment number and does not name the
/// superseded one, since "the other fragments are not included".
#[tokio::test]
async fn the_served_manifest_names_only_the_newer_of_two_shared_timestamp_fragments() {
    let harness = seeded();
    seed_shared_timestamp_archive(&harness);
    let case = "a pair sharing one producer start timestamp";
    let body = serde_json::json!({
        "StreamName": SHARED_TIMESTAMP_STREAM,
        "PlaybackMode": "ON_DEMAND",
        "DisplayFragmentNumber": "ALWAYS",
        "DASHFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": {
                "StartTimestamp": T0_SECS + 499,
                "EndTimestamp": T0_SECS + 501,
            },
        },
    });
    let response = harness.post(PATH, &body).await;
    let url = minted_url(case, &response, "DASHStreamingSessionURL");

    let manifest = harness.fetch(&url).await;
    assert_eq!(
        manifest.status().as_u16(),
        200,
        "case {case}: the manifest must be served, got {} with body {:?}",
        manifest.status(),
        String::from_utf8_lossy(manifest.body())
    );
    let manifest = String::from_utf8_lossy(manifest.body()).to_string();
    let newer = fragment_number(11);
    let superseded = fragment_number(10);
    assert!(
        manifest.contains(&newer),
        "case {case}: the manifest must name the newer fragment {newer}, got {manifest:?}"
    );
    assert!(
        !manifest.contains(&superseded),
        "case {case}: the manifest must not name the superseded fragment {superseded}, \
         got {manifest:?}"
    );
}
