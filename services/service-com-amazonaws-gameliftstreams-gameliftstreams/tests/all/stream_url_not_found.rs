// SPDX-License-Identifier: CC0-1.0

//! The message a 404 `ResourceNotFoundException` carries for the operations whose
//! addressed resource is a STREAM URL.
//!
//! The rule under test, over the whole class rather than one recorded request: the
//! service words a not-found refusal for the class of resource the operation
//! ADDRESSES, and for a stream URL that wording is the fixed sentence `Stream URL
//! not found` — it quotes no identifier and no ARN back, whatever the request's two
//! path labels carried — followed by the per-request correlation id this service
//! appends to every handler message as ` (Internal ID: <uuid>)`. The operations that
//! address another class keep their own wording: `Resource '<arn>' does not exist`
//! for a resource reported by ARN, `Stream session '<arn>' does not exist` for a
//! stream session.
//!
//! The model (`smithy_query(query="shape",
//! shape="ResourceNotFoundException")`) types the error with a single required
//! `Message` and `@httpError 404` and says nothing about the text, so what is
//! asserted here is the SENTENCE and the absence of any quoted identity — never a
//! recorded correlation id, whose value is per run.
//!
//! The rows differ along what the rule generalises over: the operation and its HTTP
//! method, which of the two resources is absent, the form of each identifier
//! (`Identifier` is modelled as "an Amazon Resource Name (ARN) or ID", `length
//! 1..128, pattern ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$)$`,
//! so a bare id resolves against the request's region and this account while an
//! identifier already in ARN form is its own resolved ARN), the Region of the
//! request's credential scope, and — for `RevokeStreamUrl` — the `RevocationMode`
//! its body carries.

use serde_json::{json, Value};

use super::stream_workflow::{
    active_stream_group, as_label, call, call_in_scope_region, emulator, ready_application, text,
    ACCOUNT, REGION,
};

/// The sentence the service answers a missing stream URL with, before the
/// per-request correlation id.
const STREAM_URL_NOT_FOUND: &str = "Stream URL not found";

/// A Region other than the one the suite's fixtures live in, used to show the
/// sentence does not depend on the request's credential scope.
const OTHER_REGION: &str = "eu-west-1";

/// A stream URL ARN in another Region and account: still a legal
/// `StreamUrlIdentifier`, and already resolved.
const ELSEWHERE_URL: &str =
    "arn:aws:gameliftstreams:eu-west-1:111122223333:streamurl/sg-elsewhere/su-elsewhere";

/// The error code a restJson1 error body carries.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .or_else(|| body["Code"].as_str())
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `<sentence> (Internal ID: <uuid>)`, split into its two parts, or `None` when the
/// message does not carry the suffix at all.
fn split_internal_id(message: &str) -> Option<(String, String)> {
    let (sentence, internal_id) = message
        .strip_suffix(')')?
        .rsplit_once(" (Internal ID: ")?;
    Some((sentence.to_string(), internal_id.to_string()))
}

/// A canonical 8-4-4-4-12 lowercase-hex UUID, which is all the per-request
/// correlation id has to be.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<usize> = value.split('-').map(str::len).collect();
    groups == [8, 4, 4, 4, 12]
        && value
            .chars()
            .all(|ch| ch == '-' || (ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()))
}

/// One request that addresses a stream URL which is not there.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The Region the request's credential scope names.
    scope_region: &'static str,
    /// Every string the request carried that the message must NOT quote back.
    forbidden: Vec<String>,
}

/// The `GetStreamUrl` path for two labels.
fn get_path(group: &str, url: &str) -> String {
    format!("/streamgroups/{group}/streamurls/{url}")
}

/// The `RevokeStreamUrl` path for two labels.
fn revoke_path(group: &str, url: &str) -> String {
    format!("/streamgroups/{group}/streamurls/{url}/revoke")
}

#[tokio::test]
async fn every_missing_stream_url_answers_the_stream_url_sentence_without_quoting_the_request() {
    let router = emulator();
    // A stream group that IS there, so a row can miss only the stream URL.
    let (_, app_arn) = ready_application(&router, "missing-url-app").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "missing-url-group", &app_arn, &[REGION]).await;
    let url_arn_in_group =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamurl/{group_id}/su-gone");

    let rows: Vec<Row> = vec![
        Row {
            case: "GetStreamUrl, bare ids, the stream group present",
            method: "GET",
            path: get_path(&group_id, "su-gone"),
            body: None,
            scope_region: REGION,
            forbidden: vec![group_id.clone(), "su-gone".to_string()],
        },
        Row {
            case: "GetStreamUrl in a stream group that is not there",
            method: "GET",
            path: get_path("sg-gone", "su-gone"),
            body: None,
            scope_region: REGION,
            forbidden: vec!["sg-gone".to_string(), "su-gone".to_string()],
        },
        Row {
            case: "GetStreamUrl with the stream group named by ARN",
            method: "GET",
            path: get_path(&as_label(&group_arn), "su-gone"),
            body: None,
            scope_region: REGION,
            forbidden: vec![group_arn.clone(), "su-gone".to_string()],
        },
        Row {
            case: "GetStreamUrl with the stream URL named by ARN",
            method: "GET",
            path: get_path(&group_id, &as_label(&url_arn_in_group)),
            body: None,
            scope_region: REGION,
            forbidden: vec![url_arn_in_group.clone()],
        },
        Row {
            case: "GetStreamUrl with a stream URL ARN in another Region and account",
            method: "GET",
            path: get_path(&group_id, &as_label(ELSEWHERE_URL)),
            body: None,
            scope_region: REGION,
            forbidden: vec![ELSEWHERE_URL.to_string()],
        },
        Row {
            case: "GetStreamUrl signed for another Region",
            method: "GET",
            path: get_path("sg-gone", "su-gone"),
            body: None,
            scope_region: OTHER_REGION,
            forbidden: vec![
                OTHER_REGION.to_string(),
                "sg-gone".to_string(),
                "su-gone".to_string(),
            ],
        },
        Row {
            case: "RevokeStreamUrl, bare ids, the stream group present",
            method: "POST",
            path: revoke_path(&group_id, "su-gone"),
            body: None,
            scope_region: REGION,
            forbidden: vec![group_id.clone(), "su-gone".to_string()],
        },
        Row {
            case: "RevokeStreamUrl in a stream group that is not there",
            method: "POST",
            path: revoke_path("sg-gone", "su-gone"),
            body: None,
            scope_region: REGION,
            forbidden: vec!["sg-gone".to_string(), "su-gone".to_string()],
        },
        Row {
            case: "RevokeStreamUrl with RevocationMode REVOKE_URL",
            method: "POST",
            path: revoke_path(&group_id, "su-gone"),
            body: Some(json!({"RevocationMode": "REVOKE_URL"})),
            scope_region: REGION,
            forbidden: vec![group_id.clone(), "su-gone".to_string()],
        },
        Row {
            case: "RevokeStreamUrl with RevocationMode REVOKE_AND_TERMINATE_SESSIONS",
            method: "POST",
            path: revoke_path(&as_label(&group_arn), "su-gone"),
            body: Some(json!({"RevocationMode": "REVOKE_AND_TERMINATE_SESSIONS"})),
            scope_region: REGION,
            forbidden: vec![group_arn.clone(), "su-gone".to_string()],
        },
        Row {
            case: "RevokeStreamUrl with the stream URL named by ARN",
            method: "POST",
            path: revoke_path(&group_id, &as_label(&url_arn_in_group)),
            body: None,
            scope_region: REGION,
            forbidden: vec![url_arn_in_group.clone()],
        },
    ];

    let mut seen: Vec<String> = Vec::new();
    for row in &rows {
        let Row {
            case,
            method,
            path,
            body,
            scope_region,
            forbidden,
        } = row;
        let (status, refused) =
            call_in_scope_region(&router, method, path, body.clone(), scope_region).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {refused}"
        );
        let message = text(&refused, "/Message", case);
        let Some((sentence, internal_id)) = split_internal_id(&message) else {
            panic!(
                "case {case:?}: expected \"{STREAM_URL_NOT_FOUND} (Internal ID: <uuid>)\", \
                 got {message:?}"
            );
        };
        assert_eq!(
            sentence, STREAM_URL_NOT_FOUND,
            "case {case:?}: expected {STREAM_URL_NOT_FOUND:?}, got {sentence:?}"
        );
        assert!(
            is_uuid(&internal_id),
            "case {case:?}: expected a UUID internal id, got {internal_id:?} in {message:?}"
        );
        // The sentence quotes NOTHING the request carried, and no ARN at all.
        assert!(
            !message.contains("arn:"),
            "case {case:?}: the message must quote no ARN, got {message:?}"
        );
        for quoted in forbidden {
            assert!(
                !message.contains(quoted.as_str()),
                "case {case:?}: the message must not quote {quoted:?}, got {message:?}"
            );
        }
        // The correlation id identifies ONE request.
        assert!(
            !seen.contains(&internal_id),
            "case {case:?}: the internal id identifies one request, but {internal_id:?} \
             was already used"
        );
        seen.push(internal_id);
    }

    // None of those refusals touched the store: the stream group still reads back.
    let (status, got) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetStreamGroup after the refusals: expected 200, got {status} {got}"
    );
    assert_eq!(
        text(&got, "/Arn", "the fixture stream group"),
        group_arn,
        "the refusals must leave the stream group exactly as it was, got {got}"
    );
}

#[tokio::test]
async fn an_operation_addressing_another_resource_class_keeps_its_own_wording() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "other-class-app").await;
    let (group_id, _) =
        active_stream_group(&router, "other-class-group", &app_arn, &[REGION]).await;
    let missing_group_arn =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamgroup/sg-gone");
    let missing_session_arn =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/{group_id}/ss-gone");

    // (case, method, path, body, the sentence the addressed class is worded by)
    let rows: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "GetStreamGroup addresses the stream group",
            "GET",
            "/streamgroups/sg-gone".to_string(),
            None,
            format!("Resource '{missing_group_arn}' does not exist"),
        ),
        (
            "CreateStreamUrl addresses the stream group that owns the new URL",
            "POST",
            "/streamgroups/sg-gone/streamurls".to_string(),
            Some(json!({
                "ApplicationIdentifier": app_arn,
                "Protocol": "WebRTC",
                "Locations": [REGION],
                "UrlExpiresAfterMinutes": 60,
            })),
            format!("Resource '{missing_group_arn}' does not exist"),
        ),
        (
            "GetStreamSession addresses the stream session",
            "GET",
            format!("/streamgroups/{group_id}/streamsessions/ss-gone"),
            None,
            format!("Stream session '{missing_session_arn}' does not exist"),
        ),
    ];

    for (case, method, path, body, expected) in rows {
        let (status, refused) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {refused}"
        );
        let message = text(&refused, "/Message", case);
        let Some((sentence, internal_id)) = split_internal_id(&message) else {
            panic!("case {case:?}: expected a trailing internal id, got {message:?}");
        };
        assert_eq!(
            sentence, expected,
            "case {case:?}: expected {expected:?}, got {sentence:?}"
        );
        assert!(
            is_uuid(&internal_id),
            "case {case:?}: expected a UUID internal id, got {internal_id:?} in {message:?}"
        );
    }
}

#[tokio::test]
async fn a_stream_url_that_exists_is_never_reported_missing() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "live-url-app").await;
    let (group_id, _) = active_stream_group(&router, "live-url-group", &app_arn, &[REGION]).await;

    let (status, created) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamurls"),
        Some(json!({
            "ApplicationIdentifier": app_arn,
            "Protocol": "WebRTC",
            "Locations": [REGION],
            "UrlExpiresAfterMinutes": 60,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateStreamUrl: expected 201, got {status} {created}"
    );
    let url_id = text(&created, "/StreamUrlId", "a created stream URL");
    let url_arn = text(&created, "/Arn", "a created stream URL");

    // Both identifier forms reach the record the create persisted, so the shared
    // not-found wording did not swallow the success path.
    for (case, label) in [
        ("bare id", url_id.clone()),
        ("ARN", as_label(&url_arn)),
    ] {
        let (status, got) = call(&router, "GET", &get_path(&group_id, &label), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamUrl 200, got {status} {got}"
        );
        assert_eq!(
            text(&got, "/Arn", case),
            url_arn,
            "case {case:?}: both identifier forms name the same stream URL, got {got}"
        );
    }

    // A revoked stream URL is still a stream URL that EXISTS: the revoke succeeds and
    // the read that follows it reports the record rather than the missing sentence.
    let (status, revoked) = call(&router, "POST", &revoke_path(&group_id, &url_id), None).await;
    assert_eq!(
        status, 204,
        "RevokeStreamUrl: expected 204, got {status} {revoked}"
    );
    let (status, got) = call(&router, "GET", &get_path(&group_id, &url_id), None).await;
    assert_eq!(
        status, 200,
        "GetStreamUrl after the revoke: expected 200, got {status} {got}"
    );
    assert_eq!(
        text(&got, "/Status", "a revoked stream URL"),
        "REVOKED",
        "a revoked stream URL reports REVOKED, got {got}"
    );
}
