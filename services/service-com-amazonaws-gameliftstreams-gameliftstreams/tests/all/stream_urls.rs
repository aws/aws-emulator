// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the stream URL slice: `CreateStreamUrl`, `GetStreamUrl`,
//! `RevokeStreamUrl` and `ListStreamUrls`, over the wire through the same router
//! the emulator serves.
//!
//! The rules under test, over the CLASS of requests each one covers rather than one
//! recorded request:
//!
//! * a created stream URL is PERSISTED under the stream group that runs its
//!   sessions, and every member it reports comes from that record — the request's
//!   own values where it sent them, the model's documented defaults where it did
//!   not (`UsageLimit` "the minimum is 1, and the default is 1",
//!   `SessionLengthSeconds` "the default is 43200 seconds"), the minted id and
//!   composite ARN, and `ExpiresAt` exactly `UrlExpiresAfterMinutes` after
//!   `CreatedAt`;
//! * the record reads back through `GetStreamUrl` and `ListStreamUrls`, by either
//!   form of either identifier, unchanged between reads;
//! * `RevokeStreamUrl` moves it to `REVOKED` permanently, with the
//!   `StreamUrlStatusReason` its `RevocationMode` calls for;
//! * a request that breaks a rule the model states in prose is refused with the
//!   modelled error, and persists nothing.
//!
//! Every assertion names its case and the value it actually got.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::stream_workflow::{
    active_stream_group, as_label, call, emulator, ready_application, text, ACCOUNT, REGION,
};

/// A second Region the fixture stream groups hold a location in, so a `Locations`
/// list can differ in length and in order between rows.
const SECOND_LOCATION: &str = "us-east-1";

/// A `ClientToken` is modelled `length 32..128, pattern ^[\x21-\x7E]+$`.
const TOKEN: &str = "stream-url-token-aaaaaaaaaaaaaaaaaaa";
const OTHER_TOKEN: &str = "stream-url-token-bbbbbbbbbbbbbbbbbbb";

/// An `IamRoleArn` is modelled `length 20..2048, pattern
/// ^arn:aws[a-zA-Z-]*:iam::\d{12}:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/stream-role";

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

/// A message with the per-request correlation id this service appends stripped off,
/// so an assertion compares the sentence rather than one call's id.
fn without_internal_id(message: &str) -> String {
    message
        .split_once(" (Internal ID: ")
        .map_or_else(|| message.to_string(), |(head, _)| head.to_string())
}

/// The smallest `CreateStreamUrl` body: every member the model requires, and
/// nothing else.
fn minimal_body(application: &str, locations: &[&str], minutes: i64) -> Value {
    json!({
        "ApplicationIdentifier": application,
        "Protocol": "WebRTC",
        "Locations": locations,
        "UrlExpiresAfterMinutes": minutes,
    })
}

/// Create one stream URL in `group`, asserting the 201, and return the parsed body.
async fn create_stream_url(
    router: &MultiServiceRouter,
    group: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, created) = call(
        router,
        "POST",
        &format!("/streamgroups/{group}/streamurls"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateStreamUrl 201, got {status} {created}"
    );
    created
}

/// `GET /streamgroups/<group>/streamurls/<url>`.
async fn get_stream_url(
    router: &MultiServiceRouter,
    group: &str,
    url: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/streamgroups/{group}/streamurls/{url}"),
        None,
    )
    .await
}

/// `POST /streamgroups/<group>/streamurls/<url>/revoke`.
async fn revoke_stream_url(
    router: &MultiServiceRouter,
    group: &str,
    url: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/streamgroups/{group}/streamurls/{url}/revoke"),
        body,
    )
    .await
}

/// The `Items` of one `ListStreamUrls` page, asserting the 200.
async fn list_stream_urls(
    router: &MultiServiceRouter,
    query: &str,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, listed) = call(router, "GET", &format!("/streamurls{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListStreamUrls 200, got {status} {listed}"
    );
    let items = listed["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Items list, got {listed}"))
        .clone();
    let next = listed["NextToken"].as_str().map(str::to_string);
    (items, next)
}

/// The `StreamUrlId`s one page of `Items` names.
fn ids_of(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| item["StreamUrlId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the create table: the case, the request body, and what the stored
/// record must report for the members the row varies.
struct CreateRow {
    case: &'static str,
    body: Value,
    minutes: i64,
    usage_limit: i64,
    session_length_seconds: i64,
    locations: Vec<String>,
}

/// One refused `CreateStreamUrl`: the case, the stream group path label, the
/// request body, and the status, error code and message (its per-request internal
/// id stripped) the refusal must carry. `None` for the message asserts the code
/// alone, for a refusal the framework's own constraint walk words.
type Refusal = (
    &'static str,
    String,
    Value,
    u16,
    &'static str,
    Option<String>,
);

/// One read of a stream URL that is not there: the case, the method, the two path
/// labels, the status, and whether the answer is the service's missing-stream-URL
/// refusal (`false` for a label that names no stream URL at all, which is refused
/// as a bad value before the store is read).
type MissingRead = (
    &'static str,
    &'static str,
    String,
    String,
    u16,
    bool,
);

#[tokio::test]
async fn a_created_stream_url_reports_the_record_it_persisted() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "stream-url-app").await;
    let (group_id, group_arn) = active_stream_group(
        &router,
        "stream-url-group",
        &app_arn,
        &[REGION, SECOND_LOCATION],
    )
    .await;

    // The rows differ along what the rule generalises over: which optional members
    // the request sends, the number and ORDER of the locations, the expiry window,
    // the usage limit and the session length.
    let rows = vec![
        CreateRow {
            case: "only the required members",
            body: minimal_body(&app_arn, &[REGION], 1),
            minutes: 1,
            // "The minimum is 1, and the default is 1" (UsageLimit).
            usage_limit: 1,
            // "The default is 43200 seconds (12 hours)" (SessionLengthSeconds).
            session_length_seconds: 43_200,
            locations: vec![REGION.to_string()],
        },
        CreateRow {
            case: "every optional member, two locations in priority order",
            body: json!({
                "ApplicationIdentifier": app_arn,
                "Protocol": "WebRTC",
                "Locations": [SECOND_LOCATION, REGION],
                "UrlExpiresAfterMinutes": 720,
                "UsageLimit": 25,
                "SessionLengthSeconds": 86_400,
                "Description": "friends-and-family",
                "AdditionalLaunchArgs": ["--windowed", "--lang=en"],
                "AdditionalEnvironmentVariables": {"TIER": "gold"},
                "RoleArn": ROLE_ARN,
                "DisplayConfiguration": {"Resolution": {"Width": 1280, "Height": 720}},
            }),
            minutes: 720,
            usage_limit: 25,
            session_length_seconds: 86_400,
            locations: vec![SECOND_LOCATION.to_string(), REGION.to_string()],
        },
        CreateRow {
            case: "the application named by bare id, the other location, the maximum \
                   session length",
            body: json!({
                "ApplicationIdentifier": app_arn
                    .rsplit('/')
                    .next()
                    .expect("an application ARN ends in its id"),
                "Protocol": "WebRTC",
                "Locations": [SECOND_LOCATION],
                "UrlExpiresAfterMinutes": 43_200,
                "UsageLimit": 1,
                "SessionLengthSeconds": 1,
            }),
            minutes: 43_200,
            usage_limit: 1,
            session_length_seconds: 1,
            locations: vec![SECOND_LOCATION.to_string()],
        },
    ];

    let mut seen_urls: Vec<String> = Vec::new();
    for row in &rows {
        let case = row.case;
        let created = create_stream_url(&router, &group_id, row.body.clone(), case).await;

        let url_id = text(&created, "/StreamUrlId", case);
        assert!(
            url_id.starts_with("su-"),
            "case {case:?}: a stream URL id is minted with the service's own prefix, \
             got {url_id:?}"
        );
        // "Format is arn:aws:gameliftstreams:[AWS Region]:[AWS account]:streamurl/
        // [stream group resource ID]/[stream URL resource ID]."
        assert_eq!(
            text(&created, "/Arn", case),
            format!(
                "arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamurl/{group_id}/{url_id}"
            ),
            "case {case:?}: the ARN carries the stream group and the minted id, got {created}"
        );
        assert_eq!(
            created["Status"], "ACTIVE",
            "case {case:?}: a new stream URL can start a stream session, got {created}"
        );
        assert_eq!(
            created["StatusReason"],
            Value::Null,
            "case {case:?}: a StatusReason is populated only for a REVOKED stream URL, \
             got {created}"
        );
        assert_eq!(
            created["UsageLimit"],
            json!(row.usage_limit),
            "case {case:?}: expected UsageLimit {}, got {created}",
            row.usage_limit
        );
        assert_eq!(
            created["RemainingUses"],
            json!(row.usage_limit),
            "case {case:?}: a stream URL nothing has used still holds every use, got {created}"
        );
        assert_eq!(
            created["SessionLengthSeconds"],
            json!(row.session_length_seconds),
            "case {case:?}: expected SessionLengthSeconds {}, got {created}",
            row.session_length_seconds
        );
        assert_eq!(
            created["Locations"],
            json!(row.locations),
            "case {case:?}: the locations are the request's, in its order, got {created}"
        );
        assert_eq!(
            text(&created, "/StreamGroupArn", case),
            group_arn,
            "case {case:?}: the stream group that runs the sessions, got {created}"
        );
        assert_eq!(
            text(&created, "/ApplicationArn", case),
            app_arn,
            "case {case:?}: the application resolves to its stored ARN whichever form \
             the request named it by, got {created}"
        );
        // "The number of minutes after creation that the stream URL remains valid."
        let created_at = created["CreatedAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: CreatedAt is not a timestamp in {created}"));
        let expires_at = created["ExpiresAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: ExpiresAt is not a timestamp in {created}"));
        let expected_window = row.minutes as f64 * 60.0;
        assert!(
            (expires_at - created_at - expected_window).abs() < 1.0,
            "case {case:?}: expected ExpiresAt {expected_window} s after CreatedAt, got \
             {created_at} and {expires_at}"
        );
        // "Treat the stream URL as a secret": no two stream URLs share a locator.
        let stream_url = text(&created, "/StreamUrl", case);
        assert!(
            stream_url.contains(&url_id) && stream_url.starts_with("https://"),
            "case {case:?}: the shareable locator is built from the record, got {stream_url:?}"
        );
        assert!(
            !seen_urls.contains(&stream_url),
            "case {case:?}: two stream URLs cannot share the locator {stream_url:?}"
        );
        seen_urls.push(stream_url.clone());

        // ---- the record reads back, by either form of either identifier ----
        for (read_case, group_label, url_label) in [
            ("bare ids", group_id.clone(), url_id.clone()),
            ("the stream group by ARN", as_label(&group_arn), url_id.clone()),
            (
                "the stream URL by ARN",
                group_id.clone(),
                as_label(&text(&created, "/Arn", case)),
            ),
        ] {
            let (status, got) = get_stream_url(&router, &group_label, &url_label).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{read_case}: expected GetStreamUrl 200, got {status} {got}"
            );
            for member in [
                "Arn",
                "StreamUrlId",
                "StreamUrl",
                "Status",
                "UsageLimit",
                "RemainingUses",
                "CreatedAt",
                "ExpiresAt",
                "SessionLengthSeconds",
                "Locations",
                "Protocol",
                "StreamGroupArn",
                "ApplicationArn",
                "Description",
                "AdditionalLaunchArgs",
                "AdditionalEnvironmentVariables",
                "RoleArn",
                "DisplayConfiguration",
            ] {
                assert_eq!(
                    got[member], created[member],
                    "case {case:?}/{read_case}: {member} must read back exactly as the \
                     create reported it, got {got}"
                );
            }
            // "A list of the stream sessions that have been started through this
            // stream URL": nothing has used this one.
            assert_eq!(
                got["StreamSessions"],
                json!([]),
                "case {case:?}/{read_case}: no stream session has been started through \
                 this stream URL, got {got}"
            );
        }
    }

    // Every created stream URL is in the account-wide list, exactly once.
    let (items, next) = list_stream_urls(&router, "", "every stream URL of the account").await;
    assert_eq!(
        next, None,
        "the whole list fits one page, so no NextToken is owed, got {next:?}"
    );
    assert_eq!(
        items.len(),
        rows.len(),
        "expected one entry per created stream URL, got {items:?}"
    );
    for item in &items {
        assert_eq!(
            item["StreamGroupArn"],
            Value::String(group_arn.clone()),
            "every stream URL belongs to the fixture stream group, got {item}"
        );
        assert_eq!(
            item["Status"], "ACTIVE",
            "none of them has been revoked, got {item}"
        );
    }
}

#[tokio::test]
async fn revoking_a_stream_url_is_permanent_and_states_what_it_did_to_the_sessions() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "revoke-app").await;
    let (group_id, _) = active_stream_group(&router, "revoke-group", &app_arn, &[REGION]).await;

    // The rows differ along the `RevocationMode` the request sends: "If you do not
    // specify a value, the default is REVOKE_URL ... REVOKE_AND_TERMINATE_SESSIONS:
    // Stops new stream sessions and ends any running stream sessions."
    let rows: Vec<(&str, Option<Value>, &str)> = vec![
        ("no RevocationMode", None, "userRevoked"),
        (
            "RevocationMode REVOKE_URL",
            Some(json!({"RevocationMode": "REVOKE_URL"})),
            "userRevoked",
        ),
        (
            "RevocationMode REVOKE_AND_TERMINATE_SESSIONS",
            Some(json!({"RevocationMode": "REVOKE_AND_TERMINATE_SESSIONS"})),
            "revokedAndSessionsTerminated",
        ),
    ];

    for (case, body, expected_reason) in rows {
        let created =
            create_stream_url(&router, &group_id, minimal_body(&app_arn, &[REGION], 60), case)
                .await;
        let url_id = text(&created, "/StreamUrlId", case);

        let (status, answer) = revoke_stream_url(&router, &group_id, &url_id, body.clone()).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected RevokeStreamUrl 204, got {status} {answer}"
        );

        let (status, got) = get_stream_url(&router, &group_id, &url_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a revoked stream URL still reads back, got {status} {got}"
        );
        assert_eq!(
            got["Status"], "REVOKED",
            "case {case:?}: expected REVOKED, got {got}"
        );
        assert_eq!(
            got["StatusReason"], expected_reason,
            "case {case:?}: expected StatusReason {expected_reason:?}, got {got}"
        );
        // "Revoking a stream URL is permanent": a second revoke — here with the
        // OTHER mode — is idempotent and cannot reword the first one.
        let (status, answer) = revoke_stream_url(
            &router,
            &group_id,
            &url_id,
            Some(json!({"RevocationMode": "REVOKE_AND_TERMINATE_SESSIONS"})),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: a repeat revoke is idempotent, got {status} {answer}"
        );
        let (_, again) = get_stream_url(&router, &group_id, &url_id).await;
        assert_eq!(
            again["StatusReason"], expected_reason,
            "case {case:?}: the first revocation's reason stands, got {again}"
        );
        assert_eq!(
            again["RemainingUses"], created["RemainingUses"],
            "case {case:?}: a revocation does not spend a use, got {again}"
        );
    }

    // Revoking one stream URL leaves the others alone: the status filter sees
    // exactly the revoked ones.
    let (revoked, _) = list_stream_urls(&router, "?Status=REVOKED", "the revoked ones").await;
    assert_eq!(
        revoked.len(),
        3,
        "all three stream URLs were revoked, got {revoked:?}"
    );
    let (active, _) = list_stream_urls(&router, "?Status=ACTIVE", "the active ones").await;
    assert!(
        active.is_empty(),
        "none is active any more, got {active:?}"
    );
}

#[tokio::test]
async fn the_list_filters_and_pages_over_the_stored_stream_urls() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "list-app").await;
    let (first_group, first_group_arn) =
        active_stream_group(&router, "list-group-one", &app_arn, &[REGION]).await;
    let (second_group, second_group_arn) =
        active_stream_group(&router, "list-group-two", &app_arn, &[REGION]).await;

    let mut first_ids: Vec<String> = Vec::new();
    for index in 0..3 {
        let case = "a stream URL of the first group";
        let created = create_stream_url(
            &router,
            &first_group,
            minimal_body(&app_arn, &[REGION], 60 + index),
            case,
        )
        .await;
        first_ids.push(text(&created, "/StreamUrlId", case));
    }
    let second_case = "a stream URL of the second group";
    let created = create_stream_url(
        &router,
        &second_group,
        minimal_body(&app_arn, &[REGION], 5),
        second_case,
    )
    .await;
    let second_id = text(&created, "/StreamUrlId", second_case);

    // The rows differ along the filter: none, each group by bare id and by ARN, a
    // group the store does not hold, and a status.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no filter",
            String::new(),
            first_ids
                .iter()
                .cloned()
                .chain([second_id.clone()])
                .collect(),
        ),
        (
            "the first group by bare id",
            format!("?StreamGroupIdentifier={first_group}"),
            first_ids.clone(),
        ),
        (
            "the first group by ARN",
            format!("?StreamGroupIdentifier={}", as_label(&first_group_arn)),
            first_ids.clone(),
        ),
        (
            "the second group by bare id",
            format!("?StreamGroupIdentifier={second_group}"),
            vec![second_id.clone()],
        ),
        (
            "the second group by ARN",
            format!("?StreamGroupIdentifier={}", as_label(&second_group_arn)),
            vec![second_id.clone()],
        ),
        (
            "a stream group the store does not hold",
            "?StreamGroupIdentifier=sg-gone".to_string(),
            Vec::new(),
        ),
        (
            "a status no stream URL is in",
            "?Status=EXPIRED".to_string(),
            Vec::new(),
        ),
        (
            "the status every stream URL is in, in the first group",
            format!("?Status=ACTIVE&StreamGroupIdentifier={first_group}"),
            first_ids.clone(),
        ),
    ];

    for (case, query, expected) in rows {
        let (items, _) = list_stream_urls(&router, &query, case).await;
        let mut got = ids_of(&items);
        got.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: expected exactly {want:?}, got {got:?} from {items:?}"
        );
    }

    // One id per page: `NextToken` walks every stored stream URL exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut token = String::new();
    loop {
        let query = if token.is_empty() {
            "?MaxResults=1".to_string()
        } else {
            format!("?MaxResults=1&NextToken={token}")
        };
        let (items, next) = list_stream_urls(&router, &query, "one page at a time").await;
        assert!(
            items.len() <= 1,
            "one page holds at most MaxResults entries, got {items:?}"
        );
        walked.extend(ids_of(&items));
        match next {
            Some(next) => token = next,
            None => break,
        }
    }
    walked.sort();
    let mut expected: Vec<String> = first_ids.iter().cloned().chain([second_id]).collect();
    expected.sort();
    assert_eq!(
        walked, expected,
        "paging must visit every stored stream URL exactly once, got {walked:?}"
    );

    // A `StreamGroupIdentifier` that names another KIND of resource is a bad value
    // for the member, not an empty page.
    let (status, refused) = call(
        &router,
        "GET",
        &format!("/streamurls?StreamGroupIdentifier={}", as_label(&app_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "an application ARN names no stream group, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected the modelled ValidationException, got {refused}"
    );
}

#[tokio::test]
async fn a_create_that_breaks_a_rule_is_refused_and_persists_nothing() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "refusal-app").await;
    let (unassociated_id, unassociated_arn) =
        ready_application(&router, "unassociated-app").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "refusal-group", &app_arn, &[REGION]).await;

    let missing_group_arn =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamgroup/sg-gone");
    let missing_app_arn = format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application/a-gone");

    // (case, the stream group label, the body, the status, the error code, the
    // message the service words it with — the per-request internal id stripped)
    let rows: Vec<Refusal> = vec![
        (
            "an application the stream group is not associated with",
            group_id.clone(),
            minimal_body(&unassociated_arn, &[REGION], 60),
            400,
            "ValidationException",
            Some(format!(
                "Application '{unassociated_arn}' is not associated with stream group \
                 '{group_arn}'"
            )),
        ),
        (
            "that same application named by bare id",
            group_id.clone(),
            minimal_body(&unassociated_id, &[REGION], 60),
            400,
            "ValidationException",
            Some(format!(
                "Application '{unassociated_arn}' is not associated with stream group \
                 '{group_arn}'"
            )),
        ),
        (
            "a stream group the store does not hold",
            "sg-gone".to_string(),
            minimal_body(&app_arn, &[REGION], 60),
            404,
            "ResourceNotFoundException",
            Some(format!("Resource '{missing_group_arn}' does not exist")),
        ),
        (
            "an application the store does not hold",
            group_id.clone(),
            minimal_body("a-gone", &[REGION], 60),
            404,
            "ResourceNotFoundException",
            Some(format!("Resource '{missing_app_arn}' does not exist")),
        ),
        (
            "a location the stream group does not hold",
            group_id.clone(),
            minimal_body(&app_arn, &[SECOND_LOCATION], 60),
            400,
            "ValidationException",
            None,
        ),
        (
            "one held location and one the stream group does not hold",
            group_id.clone(),
            minimal_body(&app_arn, &[REGION, SECOND_LOCATION], 60),
            400,
            "ValidationException",
            None,
        ),
        (
            "an Identifier that names an application rather than a stream group",
            as_label(&app_arn),
            minimal_body(&app_arn, &[REGION], 60),
            400,
            "ValidationException",
            None,
        ),
        (
            "a Resolution with an odd Height",
            group_id.clone(),
            json!({
                "ApplicationIdentifier": app_arn,
                "Protocol": "WebRTC",
                "Locations": [REGION],
                "UrlExpiresAfterMinutes": 60,
                "DisplayConfiguration": {"Resolution": {"Width": 1280, "Height": 721}},
            }),
            400,
            "ValidationException",
            None,
        ),
        (
            "a Resolution past the pixel budget",
            group_id.clone(),
            json!({
                "ApplicationIdentifier": app_arn,
                "Protocol": "WebRTC",
                "Locations": [REGION],
                "UrlExpiresAfterMinutes": 60,
                "DisplayConfiguration": {"Resolution": {"Width": 3840, "Height": 2160}},
            }),
            400,
            "ValidationException",
            None,
        ),
        (
            "UrlExpiresAfterMinutes below the modelled minimum",
            group_id.clone(),
            minimal_body(&app_arn, &[REGION], 0),
            400,
            "ValidationException",
            None,
        ),
        (
            "no UrlExpiresAfterMinutes at all",
            group_id.clone(),
            json!({
                "ApplicationIdentifier": app_arn,
                "Protocol": "WebRTC",
                "Locations": [REGION],
            }),
            400,
            "ValidationException",
            None,
        ),
    ];

    for (case, group_label, body, expected_status, expected_code, expected_message) in rows {
        let (status, refused) = call(
            &router,
            "POST",
            &format!("/streamgroups/{group_label}/streamurls"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected a {expected_code}, got {refused}"
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                without_internal_id(&text(&refused, "/Message", case)),
                expected_message,
                "case {case:?}: expected the service's own wording, got {refused}"
            );
        }
    }

    // Not one of those refusals persisted a stream URL.
    let (items, _) = list_stream_urls(&router, "", "after the refusals").await;
    assert!(
        items.is_empty(),
        "a refused CreateStreamUrl persists nothing, got {items:?}"
    );
}

#[tokio::test]
async fn a_read_of_a_stream_url_that_is_not_there_reports_the_stream_url_sentence() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "missing-url-app").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "missing-url-group", &app_arn, &[REGION]).await;

    let url_arn_in_group =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamurl/{group_id}/su-gone");

    // The rows differ along the operation, the form of each identifier, and whether
    // it is the stream group or the stream URL that is absent: the stream URL is the
    // resource the call addresses either way, so one sentence answers them all.
    let rows: Vec<MissingRead> = vec![
        (
            "GetStreamUrl, bare ids",
            "GET",
            group_id.clone(),
            "su-gone".to_string(),
            404,
            true,
        ),
        (
            "GetStreamUrl with the stream group by ARN",
            "GET",
            as_label(&group_arn),
            "su-gone".to_string(),
            404,
            true,
        ),
        (
            "GetStreamUrl with the stream URL by ARN",
            "GET",
            group_id.clone(),
            as_label(&url_arn_in_group),
            404,
            true,
        ),
        (
            "GetStreamUrl in a stream group that is not there",
            "GET",
            "sg-gone".to_string(),
            "su-gone".to_string(),
            404,
            true,
        ),
        (
            "RevokeStreamUrl, bare ids",
            "POST",
            group_id.clone(),
            "su-gone".to_string(),
            404,
            true,
        ),
        (
            "RevokeStreamUrl in a stream group that is not there",
            "POST",
            "sg-gone".to_string(),
            "su-gone".to_string(),
            404,
            true,
        ),
        (
            // An identifier that names another KIND of resource is a bad value for
            // the label, so it is refused before the store is read.
            "GetStreamUrl with a StreamUrlIdentifier naming an application",
            "GET",
            group_id.clone(),
            as_label(&app_arn),
            400,
            false,
        ),
    ];

    for (case, method, group_label, url_label, expected_status, expect_not_found) in rows {
        let path = if method == "POST" {
            format!("/streamgroups/{group_label}/streamurls/{url_label}/revoke")
        } else {
            format!("/streamgroups/{group_label}/streamurls/{url_label}")
        };
        let (status, refused) = call(&router, method, &path, None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {refused}"
        );
        if expect_not_found {
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {refused}"
            );
            assert_eq!(
                without_internal_id(&text(&refused, "/Message", case)),
                "Stream URL not found",
                "case {case:?}: expected the service's missing-stream-URL sentence, \
                 got {refused}"
            );
        } else {
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {refused}"
            );
        }
    }
}

#[tokio::test]
async fn a_client_token_serves_the_stream_url_it_created_and_refuses_a_second_request() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "token-app").await;
    let (group_id, _) = active_stream_group(&router, "token-group", &app_arn, &[REGION]).await;

    let mut body = minimal_body(&app_arn, &[REGION], 60);
    body["ClientToken"] = json!(TOKEN);
    let case = "the first request carrying the token";
    let first = create_stream_url(&router, &group_id, body.clone(), case).await;
    let first_id = text(&first, "/StreamUrlId", case);

    // "If you retry a request with the same ClientToken, Amazon GameLift Streams
    // returns the original response without performing the operation again."
    let replay_case = "the same token with the same parameters";
    let replayed = create_stream_url(&router, &group_id, body.clone(), replay_case).await;
    assert_eq!(
        text(&replayed, "/StreamUrlId", replay_case),
        first_id,
        "case {replay_case:?}: expected the stream URL the token already created, got {replayed}"
    );
    assert_eq!(
        replayed["CreatedAt"], first["CreatedAt"],
        "case {replay_case:?}: a replay performs the operation again not at all, got {replayed}"
    );

    // The same token with DIFFERENT parameters is the one case it refuses.
    let mut different = body.clone();
    different["UsageLimit"] = json!(7);
    let (status, refused) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamurls"),
        Some(different),
    )
    .await;
    assert_eq!(
        status, 409,
        "the same ClientToken cannot stand for two different requests, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "expected the modelled ConflictException, got {refused}"
    );

    // A different token creates a second stream URL.
    let mut other = body.clone();
    other["ClientToken"] = json!(OTHER_TOKEN);
    let other_case = "a different token";
    let second = create_stream_url(&router, &group_id, other, other_case).await;
    assert_ne!(
        text(&second, "/StreamUrlId", other_case),
        first_id,
        "case {other_case:?}: expected a second stream URL, got {second}"
    );

    let (items, _) = list_stream_urls(&router, "", "after the two tokens").await;
    assert_eq!(
        items.len(),
        2,
        "two tokens created two stream URLs and the replay created none, got {items:?}"
    );
}

#[tokio::test]
async fn deleting_the_stream_group_revokes_the_stream_urls_it_backs() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "deleted-group-app").await;
    let (group_id, _) =
        active_stream_group(&router, "deleted-group", &app_arn, &[REGION]).await;
    let case = "a stream URL of a stream group about to be deleted";
    let created =
        create_stream_url(&router, &group_id, minimal_body(&app_arn, &[REGION], 60), case).await;
    let url_id = text(&created, "/StreamUrlId", case);

    let (status, answer) = call(&router, "DELETE", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected DeleteStreamGroup 204, got {status} {answer}"
    );

    // "streamGroupDeleted: The stream group was deleted, which revoked the stream
    // URL." The deletion is under way, so the revocation it caused is already owed.
    let (status, got) = get_stream_url(&router, &group_id, &url_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the stream URL is still readable while the group is DELETING, \
         got {status} {got}"
    );
    assert_eq!(
        got["Status"], "REVOKED",
        "case {case:?}: expected REVOKED, got {got}"
    );
    assert_eq!(
        got["StatusReason"], "streamGroupDeleted",
        "case {case:?}: expected the deletion's own reason, got {got}"
    );
}
