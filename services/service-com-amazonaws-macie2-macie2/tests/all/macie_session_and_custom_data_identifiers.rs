// SPDX-License-Identifier: CC0-1.0

//! The Macie session lifecycle and the custom-data-identifier CRUD loop, end to
//! end over the wire.
//!
//! Every case builds the production `MultiServiceRouter`, registers this service
//! on one shared engine and dispatches real `restJson1` requests — the same path
//! a socket-served SDK request takes — so what is pinned is the HTTP behaviour
//! and the response BODIES, not a handler called directly.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! request:
//!
//! * `EnableMacie` PERSISTS a session, so `GetMacieSession` reports what it
//!   stored — the settings named, or the documented default for each one
//!   omitted — and a second enable conflicts while a retried `clientToken`
//!   replays the first;
//! * `UpdateMacieSession` edits only the members it names, and moves
//!   `updatedAt` only when something actually changed;
//! * `CreateCustomDataIdentifier` stores every member of the request under a
//!   server-assigned id, so `GetCustomDataIdentifier` echoes the stored record
//!   and `ListCustomDataIdentifiers` enumerates exactly what is stored, paging
//!   by `maxResults` and resuming from its own `nextToken`;
//! * every documented bound on a create's members is refused 400
//!   `ValidationException`;
//! * `DeleteCustomDataIdentifier` SOFT deletes, so the record is still readable
//!   and reports `deleted`, while the enumeration drops it;
//! * Macie state belongs to the region the request signed for, and every
//!   operation but `EnableMacie` is refused 403 until Macie is enabled there;
//! * `DisableMacie` deletes the session and the resources it owns.

use std::collections::BTreeSet;
use std::sync::Arc;

use service_com_amazonaws_macie2_macie2 as macie2;
use smithy_server_runtime::MultiServiceRouter;

/// The account segment every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The region most cases sign for; it must reach every minted ARN.
const REGION: &str = "us-west-2";

/// One emulator: the production router with this service mounted on a shared
/// engine.
///
/// `pub(crate)` so a sibling suite in this test target drives the SAME harness
/// rather than a second copy of it.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the shared engine kept: a suite that has to seed a
/// record no implemented operation reaches yet needs the engine the service is
/// mounted on. Every case builds its emulator here, so the two forms cannot
/// drift apart on how the service is mounted.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator core's business, not this service's; off here
    // so a case asserts the service's behaviour rather than its signing.
    router.set_request_authentication(false);
    router.register_service(macie2::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A buffered `restJson1` request whose SigV4 credential scope names `region`
/// and this service's signing name, which is how a request names both.
fn request(region: &str, method: &str, uri: &str, body: &str) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/macie2/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("the harness request is well formed")
}

/// Dispatch a call signed for `region` and read back its status and JSON body.
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    let response = router
        .dispatch(request(region, method, uri, body))
        .await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{method} {uri}: the body must be JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// The same call, signed for [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    call_in(router, REGION, method, uri, body).await
}

/// `__type` as the restJson1 error envelope carries it, with any `shapeId#`
/// prefix dropped.
pub(crate) fn error_code(body: &serde_json::Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `message` as the restJson1 error envelope carries it.
pub(crate) fn error_message(body: &serde_json::Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// Enable Macie in `region`, requiring the documented 200.
pub(crate) async fn enable_in(router: &MultiServiceRouter, region: &str, body: &str) {
    let (status, reply) = call_in(router, region, "POST", "/macie", body).await;
    assert_eq!(
        status, 200,
        "case {region:?}: EnableMacie must answer 200, got {status} with {reply:?}"
    );
}

/// Create one custom data identifier and hand back the id it minted.
async fn create(router: &MultiServiceRouter, body: &serde_json::Value, case: &str) -> String {
    let (status, reply) = call(
        router,
        "POST",
        "/custom-data-identifiers",
        &body.to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCustomDataIdentifier must answer 200, got {status} with {reply:?}"
    );
    reply["customDataIdentifierId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a create must report a customDataIdentifierId, got {reply:?}")
        })
        .to_string()
}

/// Assert `id` is a canonical UUID, the form the service issues a custom data
/// identifier's id in: 36 characters, lowercase hex grouped 8-4-4-4-12.
pub(crate) fn assert_id_grammar(id: &str, case: &str) {
    assert_eq!(
        id.chars().count(),
        36,
        "case {case:?}: a custom data identifier's id is a 36-character UUID, got {id:?}"
    );
    assert_eq!(
        id.match_indices('-').map(|(at, _)| at).collect::<Vec<_>>(),
        vec![8, 13, 18, 23],
        "case {case:?}: a canonical UUID groups 8-4-4-4-12, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "case {case:?}: a canonical UUID is lowercase hex, got {id:?}"
    );
}

/// The `severityLevels` a read reports for an identifier whose create named
/// none: the service's own default level.
///
/// Written out here rather than read from the crate, so this expectation can
/// fail. Source: the `severityLevels` member doc of
/// `GetCustomDataIdentifierResponse` in this service's Smithy model — "By
/// default, Amazon Macie creates findings for S3 objects that contain at least
/// one occurrence of text that matches the detection criteria, and Macie
/// assigns the MEDIUM severity to those findings."
pub(crate) fn default_severity_levels_json() -> Vec<serde_json::Value> {
    vec![serde_json::json!({ "severity": "MEDIUM", "occurrencesThreshold": 1 })]
}

/// One instant a response reports, required to be in the form the model pins.
///
/// Every timestamp member of this service targets `__timestampIso8601`, which
/// carries `@timestampFormat("date-time")`, and each is documented as "in UTC
/// and extended ISO 8601 format" — so the wire form is an RFC 3339 date-time
/// STRING, never the restJson1 default of an epoch-seconds number. The parsed
/// instant is handed back so a caller can compare two reports of it.
pub(crate) fn reported_instant(case: &str, member: &str, body: &serde_json::Value) -> f64 {
    let value = &body[member];
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} must be an RFC 3339 date-time string, never the \
             restJson1 default of an epoch-seconds number, got {value:?}"
        )
    });
    let parsed = aws_smithy_types::DateTime::from_str(
        text,
        aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|e| {
        panic!("case {case:?}: {member} must parse as a date-time, got {text:?} ({e})")
    });
    assert!(
        text.ends_with('Z'),
        "case {case:?}: {member} must be in UTC, got {text:?}"
    );
    parsed.as_secs_f64()
}

/// Now, as the epoch seconds a reported instant is compared against.
pub(crate) fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// The session lifecycle
// ---------------------------------------------------------------------------

/// `EnableMacie` persists the settings a request names, and the documented
/// default for each one it omits, and `GetMacieSession` reports them back. The
/// table spans every combination of present and absent across both settings,
/// and every value each enum carries.
#[tokio::test]
async fn the_session_reports_the_settings_enable_stored() {
    // Every value the model declares for each member, so the rule is asserted
    // over the whole enum rather than one value: `FindingPublishingFrequency`
    // and `MacieStatus` in `smithy-model`.
    let frequencies = macie2::ops::types::FINDING_PUBLISHING_FREQUENCY_VALUES;
    let statuses = macie2::ops::types::MACIE_STATUS_VALUES;
    assert!(
        frequencies.len() >= 3 && statuses.len() >= 2,
        "the modelled enums must carry the values this table spans, got {frequencies:?} / {statuses:?}"
    );

    let mut cases: Vec<(String, Option<&str>, Option<&str>)> = vec![(
        "neither setting named".to_string(),
        None,
        None,
    )];
    for frequency in frequencies {
        cases.push((
            format!("frequency {frequency} only"),
            Some(frequency),
            None,
        ));
        for status in statuses {
            cases.push((
                format!("frequency {frequency} with status {status}"),
                Some(frequency),
                Some(*status),
            ));
        }
    }
    for status in statuses {
        cases.push((format!("status {status} only"), None, Some(*status)));
    }

    for (case, frequency, status) in cases {
        let router = emulator();
        let mut body = serde_json::Map::new();
        if let Some(frequency) = frequency {
            body.insert("findingPublishingFrequency".to_string(), frequency.into());
        }
        if let Some(status) = status {
            body.insert("status".to_string(), status.into());
        }
        enable_in(
            &router,
            REGION,
            &serde_json::Value::Object(body).to_string(),
        )
        .await;

        let (get_status, session) = call(&router, "GET", "/macie", "").await;
        assert_eq!(
            get_status, 200,
            "case {case:?}: GetMacieSession must answer 200, got {get_status} with {session:?}"
        );
        assert_eq!(
            session["findingPublishingFrequency"].as_str(),
            Some(frequency.unwrap_or(macie2::state::DEFAULT_FINDING_PUBLISHING_FREQUENCY)),
            "case {case:?}: the stored frequency must be reported, got {session:?}"
        );
        assert_eq!(
            session["status"].as_str(),
            Some(status.unwrap_or(macie2::state::DEFAULT_MACIE_STATUS)),
            "case {case:?}: the stored status must be reported, got {session:?}"
        );
        assert_eq!(
            session["serviceRole"].as_str(),
            Some(
                format!(
                    "arn:aws:iam::{ACCOUNT}:{}",
                    macie2::state::SERVICE_LINKED_ROLE_RESOURCE
                )
                .as_str()
            ),
            "case {case:?}: the service role must be Macie's service-linked role, got {session:?}"
        );
        let created = session["createdAt"].clone();
        assert!(
            !created.is_null(),
            "case {case:?}: createdAt must be reported, got {session:?}"
        );
        reported_instant(&case, "createdAt", &session);
        reported_instant(&case, "updatedAt", &session);
        assert_eq!(
            session["updatedAt"], created,
            "case {case:?}: a session nothing has edited was updated when it was created, got {session:?}"
        );

        // Reporting only: reading the session twice must not change it.
        let (_, again) = call(&router, "GET", "/macie", "").await;
        assert_eq!(
            again, session,
            "case {case:?}: a read must not change what the next reader sees, got {again:?}"
        );
    }
}

/// A second `EnableMacie` conflicts, and a retry carrying the `clientToken` of
/// the first replays it instead. Spanned over the tokens a client sends and the
/// absence of one.
#[tokio::test]
async fn a_second_enable_conflicts_unless_it_replays_a_client_token() {
    for case in ["plain", "a-token-with-dashes", "0123456789abcdef"] {
        let router = emulator();
        let body = serde_json::json!({ "clientToken": case }).to_string();
        enable_in(&router, REGION, &body).await;

        // The same token replays the enable it performed.
        let (replay_status, replay) = call(&router, "POST", "/macie", &body).await;
        assert_eq!(
            replay_status, 200,
            "case {case:?}: a retry carrying the same clientToken must answer 200, \
             got {replay_status} with {replay:?}"
        );

        // Any other request is a second enable, which conflicts.
        for (conflict_case, conflict_body) in [
            ("no token", "{}".to_string()),
            (
                "another token",
                serde_json::json!({ "clientToken": format!("{case}-other") }).to_string(),
            ),
        ] {
            let (status, reply) = call(&router, "POST", "/macie", &conflict_body).await;
            assert_eq!(
                status, 409,
                "case {case:?}/{conflict_case:?}: a second EnableMacie must answer 409, \
                 got {status} with {reply:?}"
            );
            assert_eq!(
                error_code(&reply),
                "ConflictException",
                "case {case:?}/{conflict_case:?}: expected ConflictException, got {reply:?}"
            );
        }
    }
}

/// `UpdateMacieSession` edits only the members it names, and `updatedAt` moves
/// only when the stored session actually changed.
#[tokio::test]
async fn update_edits_only_the_settings_it_names() {
    let router = emulator();
    enable_in(
        &router,
        REGION,
        &serde_json::json!({
            "findingPublishingFrequency": "FIFTEEN_MINUTES",
            "status": "ENABLED",
        })
        .to_string(),
    )
    .await;
    let (_, before) = call(&router, "GET", "/macie", "").await;

    // Naming the settings already stored changes nothing, so `updatedAt` stays
    // where it was.
    let (status, reply) = call(
        &router,
        "PATCH",
        "/macie",
        &serde_json::json!({ "status": "ENABLED" }).to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"no-op update\": UpdateMacieSession must answer 200, got {status} with {reply:?}"
    );
    let (_, unchanged) = call(&router, "GET", "/macie", "").await;
    assert_eq!(
        unchanged, before,
        "case \"no-op update\": a session nothing changed keeps its updatedAt, got {unchanged:?}"
    );

    // Each member, edited on its own: the other keeps its stored value.
    for (case, patch, expected_frequency, expected_status) in [
        (
            "frequency only",
            serde_json::json!({ "findingPublishingFrequency": "SIX_HOURS" }),
            "SIX_HOURS",
            "ENABLED",
        ),
        (
            "status only",
            serde_json::json!({ "status": "PAUSED" }),
            "SIX_HOURS",
            "PAUSED",
        ),
        (
            "both",
            serde_json::json!({ "findingPublishingFrequency": "ONE_HOUR", "status": "ENABLED" }),
            "ONE_HOUR",
            "ENABLED",
        ),
    ] {
        let (status, reply) = call(&router, "PATCH", "/macie", &patch.to_string()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateMacieSession must answer 200, got {status} with {reply:?}"
        );
        let (_, session) = call(&router, "GET", "/macie", "").await;
        assert_eq!(
            session["findingPublishingFrequency"].as_str(),
            Some(expected_frequency),
            "case {case:?}: expected frequency {expected_frequency:?}, got {session:?}"
        );
        assert_eq!(
            session["status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected status {expected_status:?}, got {session:?}"
        );
        assert_eq!(
            session["createdAt"], before["createdAt"],
            "case {case:?}: an edit must not move createdAt, got {session:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The custom-data-identifier loop
// ---------------------------------------------------------------------------

/// Every member of a create is STORED, so `GetCustomDataIdentifier` echoes the
/// record back. The table spans the members a request may carry: none of the
/// optional ones, each kind on its own, and all of them together.
#[tokio::test]
async fn a_created_identifier_reads_back_exactly_as_it_was_stored() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let cases = vec![
        (
            "required members only",
            serde_json::json!({ "name": "minimal", "regex": "[0-9]{3}" }),
        ),
        (
            "with a description",
            serde_json::json!({
                "name": "described",
                "regex": "x+",
                "description": "a description",
            }),
        ),
        (
            "with keywords and ignore words",
            serde_json::json!({
                "name": "worded",
                "regex": "[a-z]+",
                "keywords": ["ssn", "social security"],
                "ignoreWords": ["sample", "example"],
            }),
        ),
        (
            "with an explicit match distance",
            serde_json::json!({
                "name": "distanced",
                "regex": "\\d+",
                "keywords": ["acct"],
                "maximumMatchDistance": 7,
            }),
        ),
        (
            "with severity levels and tags",
            serde_json::json!({
                "name": "severe",
                "regex": "[A-Z]{2}\\d{6}",
                "severityLevels": [
                    { "severity": "LOW", "occurrencesThreshold": 1 },
                    { "severity": "MEDIUM", "occurrencesThreshold": 10 },
                    { "severity": "HIGH", "occurrencesThreshold": 100 },
                ],
                "tags": { "team": "security", "env": "test" },
            }),
        ),
        (
            "every member at once",
            serde_json::json!({
                "name": "everything",
                "regex": "^[0-9]{3}-[0-9]{2}-[0-9]{4}$",
                "description": "all members",
                "keywords": ["ssn"],
                "ignoreWords": ["0000"],
                "maximumMatchDistance": 300,
                "severityLevels": [{ "severity": "HIGH", "occurrencesThreshold": 2 }],
                "tags": { "owner": "macie" },
            }),
        ),
    ];

    for (case, body) in cases {
        let id = create(&router, &body, case).await;
        assert_id_grammar(&id, case);

        let (status, record) = call(
            &router,
            "GET",
            &format!("/custom-data-identifiers/{id}"),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCustomDataIdentifier must answer 200, got {status} with {record:?}"
        );
        assert_eq!(
            record["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the record must carry the id the create minted, got {record:?}"
        );
        assert_eq!(
            record["arn"].as_str(),
            Some(format!("arn:aws:macie2:{REGION}:{ACCOUNT}:custom-data-identifier/{id}").as_str()),
            "case {case:?}: the ARN must name this service, region, account and id, got {record:?}"
        );
        for member in ["name", "regex", "description", "tags"] {
            let sent = body.get(member).cloned().unwrap_or(serde_json::Value::Null);
            if sent.is_null() {
                assert!(
                    record.get(member).is_none_or(serde_json::Value::is_null),
                    "case {case:?}: {member} was not sent, so nothing must be reported for it, \
                     got {:?}",
                    record.get(member)
                );
            } else {
                assert_eq!(
                    record[member], sent,
                    "case {case:?}: {member} must read back as it was stored, got {record:?}"
                );
            }
        }
        // The list members a read always serializes: what was stored, and an
        // empty array when the create named none.
        for member in ["keywords", "ignoreWords"] {
            let sent = body
                .get(member)
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            assert_eq!(
                record[member], sent,
                "case {case:?}: {member} must be reported as the stored array, empty when the \
                 create named none, got {:?}",
                record.get(member)
            );
        }
        assert_eq!(
            record["maximumMatchDistance"].as_i64(),
            Some(
                body.get("maximumMatchDistance")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(i64::from(macie2::state::DEFAULT_MAXIMUM_MATCH_DISTANCE))
            ),
            "case {case:?}: the stored match distance, or the documented default, \
             must be reported, got {record:?}"
        );
        let mut reported = record["severityLevels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut expected = match body.get("severityLevels") {
            Some(sent) => sent.as_array().cloned().unwrap_or_default(),
            // Named none, so the service's own default level is in effect and
            // is what a read reports.
            None => default_severity_levels_json(),
        };
        reported.sort_by_key(|level| level["severity"].as_str().unwrap_or("").to_string());
        expected.sort_by_key(|level| level["severity"].as_str().unwrap_or("").to_string());
        assert_eq!(
            reported, expected,
            "case {case:?}: the severity levels in effect must be reported, got {record:?}"
        );
        assert_eq!(
            record["deleted"].as_bool(),
            Some(false),
            "case {case:?}: an identifier nothing deleted is not deleted, got {record:?}"
        );
        assert!(
            record["createdAt"].as_str().is_some(),
            "case {case:?}: createdAt must be reported as an ISO 8601 string, got {record:?}"
        );
    }
}

/// Every modelled instant this slice reports is an RFC 3339 / ISO 8601
/// date-time STRING naming the instant of the call that stamped it, in every
/// response shape that carries one — the session read, the identifier read and
/// the summary a listing reports. The rows differ along the operation reporting
/// it and the member it carries.
#[tokio::test]
async fn every_reported_instant_is_an_iso_8601_date_time_string() {
    let router = emulator();
    let opened = now_seconds() - 1.0;
    enable_in(&router, REGION, "{}").await;
    let id = create(
        &router,
        &serde_json::json!({ "name": "stamped", "regex": "a", "description": "stamped" }),
        "stamped",
    )
    .await;
    let closed = now_seconds() + 1.0;

    let (_, session) = call(&router, "GET", "/macie", "").await;
    let (_, record) = call(
        &router,
        "GET",
        &format!("/custom-data-identifiers/{id}"),
        "",
    )
    .await;
    let (_, listed) = call(&router, "POST", "/custom-data-identifiers/list", "{}").await;
    let summary = listed["items"]
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(|| panic!("the listing must carry the identifier, got {listed:?}"));

    for (case, member, body) in [
        ("GetMacieSession createdAt", "createdAt", &session),
        ("GetMacieSession updatedAt", "updatedAt", &session),
        ("GetCustomDataIdentifier createdAt", "createdAt", &record),
        ("CustomDataIdentifierSummary createdAt", "createdAt", &summary),
    ] {
        let instant = reported_instant(case, member, body);
        assert!(
            (opened..=closed).contains(&instant),
            "case {case:?}: the instant must be the one the call ran at, \
             inside [{opened}, {closed}], got {instant}"
        );
    }

    // The read and the summary are two projections of ONE stored record, so the
    // instant they report is the same value in the same form.
    assert_eq!(
        summary["createdAt"], record["createdAt"],
        "a summary and a read must report one stored createdAt, got {summary:?} and {record:?}"
    );
}

/// Every documented bound on a create's members is refused 400
/// `ValidationException`, and the value at each bound is accepted. The limits
/// come from the members' own documentation in `smithy-model`, which
/// `crate::state`'s constants cite one by one.
#[tokio::test]
async fn a_create_outside_a_documented_bound_is_refused() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let max_name = "n".repeat(macie2::state::MAX_NAME_CHARS);
    let over_name = "n".repeat(macie2::state::MAX_NAME_CHARS + 1);
    let max_regex = "x".repeat(macie2::state::MAX_REGEX_CHARS);
    let over_regex = "x".repeat(macie2::state::MAX_REGEX_CHARS + 1);
    let over_description = "d".repeat(macie2::state::MAX_DESCRIPTION_CHARS + 1);
    let short_keyword = "k".repeat(macie2::state::MIN_KEYWORD_CHARS - 1);
    let long_keyword = "k".repeat(macie2::state::MAX_KEYWORD_CHARS + 1);
    let short_word = "w".repeat(macie2::state::MIN_IGNORE_WORD_CHARS - 1);
    let long_word = "w".repeat(macie2::state::MAX_IGNORE_WORD_CHARS + 1);
    let too_many_keywords: Vec<String> =
        vec!["key".to_string(); macie2::state::MAX_KEYWORDS + 1];
    let too_many_words: Vec<String> =
        vec!["word".to_string(); macie2::state::MAX_IGNORE_WORDS + 1];

    // The value AT each published bound is inside it.
    for (case, body) in [
        (
            "name at its maximum",
            serde_json::json!({ "name": max_name, "regex": "a" }),
        ),
        (
            "regex at its maximum",
            serde_json::json!({ "name": "at-bound", "regex": max_regex }),
        ),
        (
            "match distance at its minimum",
            serde_json::json!({
                "name": "at-min-distance",
                "regex": "a",
                "maximumMatchDistance": macie2::state::MIN_MAXIMUM_MATCH_DISTANCE,
            }),
        ),
        (
            "match distance at its maximum",
            serde_json::json!({
                "name": "at-max-distance",
                "regex": "a",
                "maximumMatchDistance": macie2::state::MAX_MAXIMUM_MATCH_DISTANCE,
            }),
        ),
        (
            "keywords at both character bounds",
            serde_json::json!({
                "name": "at-keyword-bounds",
                "regex": "a",
                "keywords": [
                    "k".repeat(macie2::state::MIN_KEYWORD_CHARS),
                    "k".repeat(macie2::state::MAX_KEYWORD_CHARS),
                ],
            }),
        ),
        (
            "ignore words at both character bounds",
            serde_json::json!({
                "name": "at-word-bounds",
                "regex": "a",
                "ignoreWords": [
                    "w".repeat(macie2::state::MIN_IGNORE_WORD_CHARS),
                    "w".repeat(macie2::state::MAX_IGNORE_WORD_CHARS),
                ],
            }),
        ),
    ] {
        create(&router, &body, case).await;
    }

    // One past each bound, or outside each documented rule, is refused.
    for (case, body) in [
        ("name omitted", serde_json::json!({ "regex": "a" })),
        ("regex omitted", serde_json::json!({ "name": "no-regex" })),
        (
            "empty name",
            serde_json::json!({ "name": "", "regex": "a" }),
        ),
        (
            "empty regex",
            serde_json::json!({ "name": "no-pattern", "regex": "" }),
        ),
        (
            "name one past its maximum",
            serde_json::json!({ "name": over_name, "regex": "a" }),
        ),
        (
            "regex one past its maximum",
            serde_json::json!({ "name": "long-regex", "regex": over_regex }),
        ),
        (
            "description one past its maximum",
            serde_json::json!({
                "name": "long-description",
                "regex": "a",
                "description": over_description,
            }),
        ),
        (
            "one keyword too many",
            serde_json::json!({
                "name": "many-keywords",
                "regex": "a",
                "keywords": too_many_keywords,
            }),
        ),
        (
            "a keyword one character short",
            serde_json::json!({ "name": "short-keyword", "regex": "a", "keywords": [short_keyword] }),
        ),
        (
            "a keyword one character too long",
            serde_json::json!({ "name": "long-keyword", "regex": "a", "keywords": [long_keyword] }),
        ),
        (
            "one ignore word too many",
            serde_json::json!({
                "name": "many-words",
                "regex": "a",
                "ignoreWords": too_many_words,
            }),
        ),
        (
            "an ignore word one character short",
            serde_json::json!({ "name": "short-word", "regex": "a", "ignoreWords": [short_word] }),
        ),
        (
            "an ignore word one character too long",
            serde_json::json!({ "name": "long-word", "regex": "a", "ignoreWords": [long_word] }),
        ),
        (
            "match distance one below its minimum",
            serde_json::json!({
                "name": "below-distance",
                "regex": "a",
                "maximumMatchDistance": macie2::state::MIN_MAXIMUM_MATCH_DISTANCE - 1,
            }),
        ),
        (
            "match distance one past its maximum",
            serde_json::json!({
                "name": "above-distance",
                "regex": "a",
                "maximumMatchDistance": macie2::state::MAX_MAXIMUM_MATCH_DISTANCE + 1,
            }),
        ),
        (
            "one severity level too many",
            serde_json::json!({
                "name": "many-levels",
                "regex": "a",
                "severityLevels": [
                    { "severity": "LOW", "occurrencesThreshold": 1 },
                    { "severity": "MEDIUM", "occurrencesThreshold": 2 },
                    { "severity": "HIGH", "occurrencesThreshold": 3 },
                    { "severity": "LOW", "occurrencesThreshold": 4 },
                ],
            }),
        ),
        (
            "the same severity twice",
            serde_json::json!({
                "name": "repeated-severity",
                "regex": "a",
                "severityLevels": [
                    { "severity": "LOW", "occurrencesThreshold": 1 },
                    { "severity": "LOW", "occurrencesThreshold": 2 },
                ],
            }),
        ),
        (
            "thresholds not ascending by severity",
            serde_json::json!({
                "name": "descending",
                "regex": "a",
                "severityLevels": [
                    { "severity": "LOW", "occurrencesThreshold": 100 },
                    { "severity": "HIGH", "occurrencesThreshold": 1 },
                ],
            }),
        ),
    ] {
        let (status, reply) = call(
            &router,
            "POST",
            "/custom-data-identifiers",
            &body.to_string(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a create outside a documented bound must answer 400, \
             got {status} with {reply:?}"
        );
        assert_eq!(
            error_code(&reply),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
        assert!(
            reply["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: a refusal must say what was wrong, got {reply:?}"
        );
    }
}

/// `ListCustomDataIdentifiers` enumerates exactly what is stored, honours
/// `maxResults` as a page bound and resumes from its own `nextToken`. Spanned
/// over the number stored and the page size asked for.
#[tokio::test]
async fn the_enumeration_pages_through_exactly_what_is_stored() {
    for stored in [0_usize, 1, 2, 5] {
        let router = emulator();
        enable_in(&router, REGION, "{}").await;

        let mut expected = BTreeSet::new();
        for index in 0..stored {
            let case = format!("{stored} stored, item {index}");
            let id = create(
                &router,
                &serde_json::json!({
                    "name": format!("identifier-{index}"),
                    "regex": format!("[0-9]{{{}}}", index + 1),
                }),
                &case,
            )
            .await;
            expected.insert(id);
        }

        for page_size in [None, Some(1), Some(2), Some(100)] {
            let case = format!("{stored} stored, maxResults {page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            // One more round than there can be pages, so a token that never
            // clears is a failure rather than a hang.
            for _ in 0..=stored.saturating_add(1) {
                let mut body = serde_json::Map::new();
                if let Some(size) = page_size {
                    body.insert("maxResults".to_string(), size.into());
                }
                if let Some(token) = token.as_deref() {
                    body.insert("nextToken".to_string(), token.into());
                }
                let (status, reply) = call(
                    &router,
                    "POST",
                    "/custom-data-identifiers/list",
                    &serde_json::Value::Object(body).to_string(),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: ListCustomDataIdentifiers must answer 200, \
                     got {status} with {reply:?}"
                );
                let items = reply["items"].as_array().cloned().unwrap_or_default();
                let bound = page_size.unwrap_or(macie2::state::DEFAULT_PAGE_SIZE);
                assert!(
                    items.len() <= bound,
                    "case {case:?}: a page carries at most {bound} items, got {}",
                    items.len()
                );
                for item in &items {
                    let id = item["id"].as_str().unwrap_or_else(|| {
                        panic!("case {case:?}: every summary must carry an id, got {item:?}")
                    });
                    assert_eq!(
                        item["arn"].as_str(),
                        Some(
                            format!(
                                "arn:aws:macie2:{REGION}:{ACCOUNT}:custom-data-identifier/{id}"
                            )
                            .as_str()
                        ),
                        "case {case:?}: a summary's ARN must name the identifier, got {item:?}"
                    );
                    assert!(
                        item["name"].as_str().is_some_and(|n| n.starts_with("identifier-")),
                        "case {case:?}: a summary must carry the stored name, got {item:?}"
                    );
                    assert!(
                        item["createdAt"].as_str().is_some(),
                        "case {case:?}: a summary must carry the stored createdAt, got {item:?}"
                    );                    seen.push(id.to_string());
                }
                token = reply["nextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert!(
                token.is_none(),
                "case {case:?}: paging must end with no nextToken, got {token:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {case:?}: the enumeration must report exactly what is stored, got {seen:?}"
            );
            assert_eq!(
                seen.len(),
                expected.len(),
                "case {case:?}: no identifier may be reported twice, got {seen:?}"
            );
        }
    }
}

/// A `nextToken` this operation did not issue names no page, so it is refused
/// rather than silently read as "start again".
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    for case in ["not-a-token", "abc", "ff", "%%%"] {
        let (status, reply) = call(
            &router,
            "POST",
            "/custom-data-identifiers/list",
            &serde_json::json!({ "nextToken": case }).to_string(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign nextToken must answer 400, got {status} with {reply:?}"
        );
        assert_eq!(
            error_code(&reply),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
    }
}

/// `DeleteCustomDataIdentifier` SOFT deletes: the record stays readable and
/// says it was deleted, and only the enumeration drops it.
#[tokio::test]
async fn deleting_an_identifier_soft_deletes_it() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let kept = create(
        &router,
        &serde_json::json!({ "name": "kept", "regex": "a" }),
        "kept",
    )
    .await;
    let retired = create(
        &router,
        &serde_json::json!({ "name": "retired", "regex": "b", "keywords": ["abc"] }),
        "retired",
    )
    .await;

    let (status, reply) = call(
        &router,
        "DELETE",
        &format!("/custom-data-identifiers/{retired}"),
        "",
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete\": DeleteCustomDataIdentifier must answer 200, got {status} with {reply:?}"
    );

    // Still readable, and it says it was deleted — the operation soft deletes.
    let (get_status, record) = call(
        &router,
        "GET",
        &format!("/custom-data-identifiers/{retired}"),
        "",
    )
    .await;
    assert_eq!(
        get_status, 200,
        "case \"read a soft-deleted identifier\": expected 200, got {get_status} with {record:?}"
    );
    assert_eq!(
        record["deleted"].as_bool(),
        Some(true),
        "case \"read a soft-deleted identifier\": deleted must be true, got {record:?}"
    );
    assert_eq!(
        record["name"].as_str(),
        Some("retired"),
        "case \"read a soft-deleted identifier\": the stored record is unchanged, got {record:?}"
    );

    // The enumeration drops it: a summary carries no `deleted` member, so a
    // retired identifier could not be told from a live one in a page.
    let (_, listed) = call(&router, "POST", "/custom-data-identifiers/list", "{}").await;
    let ids: Vec<&str> = listed["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![kept.as_str()],
        "case \"enumerate after a delete\": only the live identifier is enumerated, got {listed:?}"
    );
}

/// An id no identifier of this region carries is 404 `ResourceNotFoundException`
/// on both the read and the delete — never a hollow 200.
#[tokio::test]
async fn an_unknown_identifier_is_not_found() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;
    let known = create(
        &router,
        &serde_json::json!({ "name": "known", "regex": "a" }),
        "known",
    )
    .await;

    for case in [
        "doesnotexist",
        "0000000000000000000000",
        "zzzzzzzzzzzzzzzzzzzzzz",
    ] {
        assert_ne!(
            case, known,
            "the table must name ids the store does not hold"
        );
        for (method, uri) in [
            ("GET", format!("/custom-data-identifiers/{case}")),
            ("DELETE", format!("/custom-data-identifiers/{case}")),
        ] {
            let (status, reply) = call(&router, method, &uri, "").await;
            assert_eq!(
                status, 404,
                "case {case:?}: {method} {uri} must answer 404, got {status} with {reply:?}"
            );
            assert_eq!(
                error_code(&reply),
                "ResourceNotFoundException",
                "case {case:?}: {method} {uri} expected ResourceNotFoundException, got {reply:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Session scope
// ---------------------------------------------------------------------------

/// Macie has to be enabled before anything else can be read or written, so
/// every other operation in this slice is refused 403 until it is.
#[tokio::test]
async fn every_operation_is_refused_until_macie_is_enabled() {
    let calls: Vec<(&str, &str, String, String)> = vec![
        ("GetMacieSession", "GET", "/macie".to_string(), String::new()),
        (
            "UpdateMacieSession",
            "PATCH",
            "/macie".to_string(),
            serde_json::json!({ "status": "PAUSED" }).to_string(),
        ),
        ("DisableMacie", "DELETE", "/macie".to_string(), String::new()),
        (
            "CreateCustomDataIdentifier",
            "POST",
            "/custom-data-identifiers".to_string(),
            serde_json::json!({ "name": "demo", "regex": "a" }).to_string(),
        ),
        (
            "GetCustomDataIdentifier",
            "GET",
            "/custom-data-identifiers/0000000000000000000000".to_string(),
            String::new(),
        ),
        (
            "ListCustomDataIdentifiers",
            "POST",
            "/custom-data-identifiers/list".to_string(),
            "{}".to_string(),
        ),
        (
            "DeleteCustomDataIdentifier",
            "DELETE",
            "/custom-data-identifiers/0000000000000000000000".to_string(),
            String::new(),
        ),
    ];

    for (case, method, uri, body) in &calls {
        let router = emulator();
        let (status, reply) = call(&router, method, uri, body).await;
        assert_eq!(
            status, 403,
            "case {case:?}: must answer 403 while Macie is not enabled, got {status} with {reply:?}"
        );
        assert_eq!(
            error_code(&reply),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {reply:?}"
        );
    }

    // And each of them gets past that refusal once Macie is enabled.
    let router = emulator();
    enable_in(&router, REGION, "{}").await;
    for (case, method, uri, body) in &calls {
        let (status, reply) = call(&router, method, uri, body).await;
        assert_ne!(
            status, 403,
            "case {case:?}: must not be refused once Macie is enabled, got {status} with {reply:?}"
        );
        // Re-enable, because DisableMacie retires the session this loop needs.
        if *case == "DisableMacie" {
            enable_in(&router, REGION, "{}").await;
        }
    }
}

/// `DisableMacie` "deletes all settings and resources for a Macie account", so
/// the session and the identifiers it owned are both gone afterwards — and
/// enabling again starts from an empty registry.
#[tokio::test]
async fn disabling_macie_deletes_the_session_and_its_resources() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;
    let id = create(
        &router,
        &serde_json::json!({ "name": "doomed", "regex": "a" }),
        "doomed",
    )
    .await;

    let (status, reply) = call(&router, "DELETE", "/macie", "").await;
    assert_eq!(
        status, 200,
        "case \"disable\": DisableMacie must answer 200, got {status} with {reply:?}"
    );

    let (session_status, session) = call(&router, "GET", "/macie", "").await;
    assert_eq!(
        session_status, 403,
        "case \"read after disable\": there is no session to report, got {session_status} \
         with {session:?}"
    );

    enable_in(&router, REGION, "{}").await;
    let (get_status, record) = call(
        &router,
        "GET",
        &format!("/custom-data-identifiers/{id}"),
        "",
    )
    .await;
    assert_eq!(
        get_status, 404,
        "case \"read a resource of a disabled session\": expected 404, got {get_status} \
         with {record:?}"
    );
    let (_, listed) = call(&router, "POST", "/custom-data-identifiers/list", "{}").await;
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(0),
        "case \"enumerate after re-enabling\": the registry starts empty, got {listed:?}"
    );
}

/// Macie is enabled per account and Region, so a session and the identifiers it
/// owns belong to the region the request signed for and are invisible to any
/// other.
#[tokio::test]
async fn macie_state_belongs_to_the_region_the_request_signs_for() {
    let router = emulator();
    let regions = ["us-east-1", "eu-west-3", "ap-southeast-2"];

    // Enabling in one region leaves the others without a session.
    enable_in(
        &router,
        regions[0],
        &serde_json::json!({ "findingPublishingFrequency": "SIX_HOURS" }).to_string(),
    )
    .await;
    for region in &regions[1..] {
        let (status, reply) = call_in(&router, region, "GET", "/macie", "").await;
        assert_eq!(
            status, 403,
            "case {region:?}: enabling Macie elsewhere must not enable it here, \
             got {status} with {reply:?}"
        );
    }

    // Each region's own identifiers, with the region in the ARN, invisible to
    // the others.
    let mut minted = Vec::new();
    for region in &regions {
        if *region != regions[0] {
            enable_in(&router, region, "{}").await;
        }
        let (status, reply) = call_in(
            &router,
            region,
            "POST",
            "/custom-data-identifiers",
            &serde_json::json!({ "name": format!("in-{region}"), "regex": "a" }).to_string(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: a create must answer 200, got {status} with {reply:?}"
        );
        let id = reply["customDataIdentifierId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: expected an id, got {reply:?}"))
            .to_string();
        minted.push((*region, id));
    }

    for (region, id) in &minted {
        let (status, record) = call_in(
            &router,
            region,
            "GET",
            &format!("/custom-data-identifiers/{id}"),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: its own identifier must be readable, got {status} with {record:?}"
        );
        assert_eq!(
            record["arn"].as_str(),
            Some(format!("arn:aws:macie2:{region}:{ACCOUNT}:custom-data-identifier/{id}").as_str()),
            "case {region:?}: the ARN must carry the region the create addressed, got {record:?}"
        );

        for (other, _) in minted.iter().filter(|(other, _)| other != region) {
            let (status, reply) = call_in(
                &router,
                other,
                "GET",
                &format!("/custom-data-identifiers/{id}"),
                "",
            )
            .await;
            assert_eq!(
                status, 404,
                "case {region:?} read from {other:?}: an identifier of another region is \
                 not found, got {status} with {reply:?}"
            );
        }

        let (_, listed) = call_in(
            &router,
            region,
            "POST",
            "/custom-data-identifiers/list",
            "{}",
        )
        .await;
        let ids: Vec<&str> = listed["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["id"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![id.as_str()],
            "case {region:?}: the enumeration reports only this region's identifiers, \
             got {listed:?}"
        );
    }
}

/// A retried `CreateCustomDataIdentifier` carrying the same `clientToken`
/// replays the identifier that token minted rather than minting a second one.
#[tokio::test]
async fn a_retried_create_replays_the_identifier_its_client_token_minted() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    for case in ["token-a", "0123456789", "a.token.with.dots"] {
        let body = serde_json::json!({
            "name": format!("for-{case}"),
            "regex": "a",
            "clientToken": case,
        });
        let first = create(&router, &body, case).await;
        let second = create(&router, &body, case).await;
        assert_eq!(
            first, second,
            "case {case:?}: a retry must replay the identifier the token minted, \
             got {first:?} then {second:?}"
        );

        let body_without_token = serde_json::json!({
            "name": format!("for-{case}"),
            "regex": "a",
        });
        let fresh = create(&router, &body_without_token, case).await;
        assert_ne!(
            fresh, first,
            "case {case:?}: a create carrying no token mints a new identifier, got {fresh:?}"
        );
    }
}
