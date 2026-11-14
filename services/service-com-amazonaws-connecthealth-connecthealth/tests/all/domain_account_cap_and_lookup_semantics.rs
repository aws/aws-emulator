// SPDX-License-Identifier: CC0-1.0

//! The account-wide Domain cap, what a Domain lookup answers when it finds nothing, and
//! the scoping of a Medical Scribe listening session.
//!
//! Four rules, each asserted over a TABLE of inputs rather than the one case the issue
//! recorded:
//!
//! 1. `CreateDomain` is capped per ACCOUNT per REGION: once the account holds
//!    `state::MAX_DOMAINS_PER_ACCOUNT` Domains in the Region the create is addressed to,
//!    the next create there is refused with HTTP 400
//!    `ValidationException`, whose text names the account and the cap. The cap is a LIVE
//!    count, so a Domain whose deletion has been applied frees its slot and the retry
//!    succeeds with status `ACTIVE`. (That the count is per Region rather than
//!    cross-Region is asserted in `tests/all/region_scoping.rs`; every row here is signed
//!    for one Region.)
//! 2. `DeleteDomain` separates an id this service ALLOCATED — whose Domain is simply
//!    gone, the modelled 404 `ResourceNotFoundException` — from a well-formed id with no
//!    allocation history at all, which the front door answers with
//!    `AuthorizerConfigurationException` whose detail member is present and EMPTY (an
//!    explicit null), carrying no text.
//! 3. A Domain-not-found answer reports `Domain not found: <domainId>`, the requested id
//!    verbatim, from whichever operation resolved the Domain.
//! 4. `GetMedicalScribeListeningSession` resolves a session only under the exact
//!    (`domainId`, `subscriptionId`) scope it belongs to, reporting the PERSISTED session
//!    and answering the modelled 404 for every other scope — never a 501.
//!
//! Model citations (`smithy_query(query="shape",
//! shape="DomainId,SubscriptionId,ScribeSessionId,ValidationException,ResourceNotFoundException",
//! model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`):
//! `DomainId` is `@length 20..25` with pattern `^(hai-|dom-)[a-z0-9]+$`,
//! `SubscriptionId` is `@length 25..25` with pattern `^sub-[a-zA-Z0-9]{21}$`,
//! `ScribeSessionId` is `@length 36..36` with pattern
//! `[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}`, and
//! `ValidationException` / `ResourceNotFoundException` carry HTTP 400 / 404.
//!
//! The harness (`Emulator`, `error_code`, `string_at`) is the sibling suite's: one way of
//! driving this service through signed restJson1 requests, not two.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::ops::types::MedicalScribeChannelDefinition;
use service_com_amazonaws_connecthealth_connecthealth::state::{
    self, ScribeSession, ScribeStreamStatus, MAX_DOMAINS_PER_ACCOUNT,
};

use super::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The body member ConnectHealth reports an exception's detail text in.
const DETAIL_MEMBER: &str = "Message";

/// The member the framework's renderers would otherwise write that text under. No error
/// body may carry the text here.
const RENDERED_DETAIL_MEMBER: &str = "message";

/// The status a front door answers an authorizer-configuration failure with — the
/// default status of API Gateway's `AUTHORIZER_CONFIGURATION_ERROR` gateway response,
/// which is what `DeleteDomain` of an id with no allocation history surfaces.
const AUTHORIZER_CONFIGURATION_STATUS: u16 = 500;

/// Long enough for the `DELETING` -> purged transition `state::DOMAIN_DELETION_SPAN`
/// owes to have fallen due.
const PAST_THE_DELETION_SPAN: Duration = Duration::from_millis(900);

/// A `CreateWebAppConfiguration`: what a create names to ask for a web application, the
/// one resource a Domain owns OUTSIDE this service — and so the one whose deletion
/// cannot finish inside the deleting call.
fn web_app_setup() -> Value {
    json!({
        "ehrRole": "arn:aws:iam::000000000000:role/ehr-operations",
        "idcInstanceId": "ssoins-72230ed02aa4b4ea",
        "idcRegion": "us-east-1",
    })
}

/// The detail text of an error body, failing with the whole body when it carries none.
fn detail_text(case: &str, body: &Value) -> String {
    assert!(
        body.get(RENDERED_DETAIL_MEMBER).is_none(),
        "case {case:?}: the detail text must be reported under {DETAIL_MEMBER} only, got {body}"
    );
    body.get(DETAIL_MEMBER)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the error body must carry a {DETAIL_MEMBER} member, got {body}")
        })
        .to_string()
}

/// A well-formed `DomainId` that names nothing: `prefix` padded with `filler` to `len`.
///
/// Both modelled prefixes and the whole modelled length range (20..25) are reachable
/// this way, so no row depends on the one id the issue recorded.
fn unallocated_domain_id(prefix: &str, filler: char, len: usize) -> String {
    let mut id = prefix.to_string();
    while id.len() < len {
        id.push(filler);
    }
    id
}

/// The creates that fill one account's cap: `MAX_DOMAINS_PER_ACCOUNT` requests differing
/// along what `CreateDomain` generalises over — which optional members are present, the
/// size of `name`, and how many tags are carried. None asks for a web application, so
/// each one's deletion completes inside its deleting call.
fn cap_filling_creates() -> Vec<Value> {
    let kms_key = "arn:aws:kms:us-east-1:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789";
    let mut creates = vec![
        json!({ "name": "clinic-minimal" }),
        json!({ "name": "c" }),
        json!({ "name": "c".repeat(64) }),
        json!({ "name": "clinic-kms", "kmsKeyArn": kms_key }),
        json!({ "name": "clinic-tagged", "tags": { "env": "test" } }),
        json!({ "name": "clinic-tagged-3", "tags": { "a": "1", "b": "2", "c": "3" } }),
    ];
    // The rest are plain creates, so the table fills exactly the cap whatever the cap is.
    while creates.len() < MAX_DOMAINS_PER_ACCOUNT {
        creates.push(json!({ "name": format!("clinic-{}", creates.len()) }));
    }
    creates.truncate(MAX_DOMAINS_PER_ACCOUNT);
    creates
}

/// The creates attempted PAST the cap: the refusal is a property of the account's live
/// Domain count, not of the request, so every shape of request earns the same answer.
fn past_the_cap_creates() -> Vec<(&'static str, Value)> {
    let kms_key = "arn:aws:kms:us-east-1:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789";
    vec![
        ("a minimal create", json!({ "name": "one-too-many" })),
        (
            "a create naming a customer-managed key",
            json!({ "name": "one-too-many-kms", "kmsKeyArn": kms_key }),
        ),
        (
            "a create carrying tags",
            json!({ "name": "one-too-many-tagged", "tags": { "env": "test" } }),
        ),
        (
            "a create asking for a web application",
            json!({
                "name": "one-too-many-web",
                "webAppSetupConfiguration": web_app_setup(),
            }),
        ),
    ]
}

/// Fill `emulator`'s account to the cap, answering the ids created and the account every
/// response reported.
async fn fill_the_cap(emulator: &Emulator) -> (Vec<String>, String) {
    let mut ids = Vec::new();
    let mut account = String::new();
    for (index, request) in cap_filling_creates().into_iter().enumerate() {
        let (status, body) = emulator.send("POST", "/domain", Some(&request)).await;
        assert_eq!(
            status, 201,
            "domain {index} of the cap ({request}): got {status} {body:?}"
        );
        assert_eq!(
            body.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "domain {index} of the cap: a created domain is ACTIVE, got {body:?}"
        );
        account = string_at(&body, "accountId");
        ids.push(string_at(&body, "domainId"));
    }
    assert_eq!(
        ids.len(),
        MAX_DOMAINS_PER_ACCOUNT,
        "the cap must be reachable: created {ids:?}"
    );
    (ids, account)
}

/// The refusal the account cap answers with, for an account holding the cap.
fn cap_message(account: &str) -> String {
    format!("Account {account} has reached the maximum limit of {MAX_DOMAINS_PER_ACCOUNT} domains.")
}

/// Assert that every create past the cap is refused the same way.
async fn every_create_past_the_cap_is_refused(emulator: &Emulator, account: &str, label: &str) {
    for (case, request) in past_the_cap_creates() {
        let (status, body) = emulator.send("POST", "/domain", Some(&request)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({label}): expected 400 ValidationException past the cap, got \
             {status} {body:?}"
        );
        let text = detail_text(case, &body);
        assert_eq!(
            text,
            cap_message(account),
            "case {case:?} ({label}): the refusal must name the account and the cap, got \
             {text:?}"
        );
    }
}

#[tokio::test]
async fn create_domain_is_capped_per_account_and_a_delete_frees_a_slot() {
    let emulator = Emulator::new("us-east-1");
    let (ids, account) = fill_the_cap(&emulator).await;
    assert!(
        account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit()),
        "the cap is measured against a 12-digit account, got {account:?}"
    );

    // At the cap, every create is refused — and nothing was created by the refusal.
    every_create_past_the_cap_is_refused(&emulator, &account, "at the cap").await;
    let (status, listed) = emulator.send("GET", "/domain?maxResults=100", None).await;
    assert_eq!(status, 200, "ListDomains got {status} {listed:?}");
    assert_eq!(
        listed
            .get("domains")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(MAX_DOMAINS_PER_ACCOUNT),
        "a refused create must store nothing: the account still holds exactly the cap, \
         got {listed:?}"
    );

    // A delete frees a slot: the count is LIVE, not a monotonic counter.
    let freed = &ids[0];
    let path = format!("/domain/{freed}");
    let (status, deleted) = emulator.send("DELETE", &path, None).await;
    assert_eq!(status, 200, "DeleteDomain got {status} {deleted:?}");
    let (status, read) = emulator.send("GET", &path, None).await;
    assert_eq!(
        (status, error_code(&read).as_str()),
        (404, "ResourceNotFoundException"),
        "the deleted domain must be gone before its slot is reused, got {status} {read:?}"
    );

    let (status, retried) = emulator
        .send("POST", "/domain", Some(&json!({ "name": "clinic-retry" })))
        .await;
    assert_eq!(
        status, 201,
        "the create retried into the freed slot got {status} {retried:?}"
    );
    assert_eq!(
        retried.get("status").and_then(Value::as_str),
        Some("ACTIVE"),
        "the retry must create an ACTIVE domain, got {retried:?}"
    );

    // And the cap binds again at once: exactly one slot was freed.
    every_create_past_the_cap_is_refused(&emulator, &account, "after the slot was reused").await;
}

#[tokio::test]
async fn a_domain_whose_deletion_is_still_in_flight_keeps_its_slot_until_the_purge() {
    let emulator = Emulator::new("us-west-2");
    // One of the cap's domains owns a web application, so its deletion cannot finish
    // inside the deleting call and it passes through the non-terminal `DELETING`.
    let web = string_at(
        &emulator
            .create_domain(&json!({
                "name": "clinic-web",
                "webAppSetupConfiguration": web_app_setup(),
            }))
            .await,
        "domainId",
    );
    for index in 1..MAX_DOMAINS_PER_ACCOUNT {
        let (status, body) = emulator
            .send(
                "POST",
                "/domain",
                Some(&json!({ "name": format!("clinic-{index}") })),
            )
            .await;
        assert_eq!(status, 201, "domain {index} of the cap: got {status} {body:?}");
    }

    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{web}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain got {status} {deleted:?}");
    assert_eq!(
        deleted.get("status").and_then(Value::as_str),
        Some("DELETING"),
        "a domain with a web application reports the non-terminal status, got {deleted:?}"
    );

    // The record is still held, so the slot is still taken.
    let (status, body) = emulator
        .send("POST", "/domain", Some(&json!({ "name": "too-early" })))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a deletion still in flight keeps its slot, got {status} {body:?}"
    );

    // Real elapsed time is required (the handlers read `SystemTime::now()`) and the
    // workspace `tokio` dependency does not enable the `time` feature, so this is a
    // plain blocking sleep.
    std::thread::sleep(PAST_THE_DELETION_SPAN);
    let (status, created) = emulator
        .send("POST", "/domain", Some(&json!({ "name": "after-the-purge" })))
        .await;
    assert_eq!(
        status, 201,
        "once the purge the clock owed has fallen due the slot is free, got {status} \
         {created:?}"
    );
}

#[tokio::test]
async fn delete_domain_separates_an_unallocated_id_from_a_domain_that_is_gone() {
    let emulator = Emulator::new("us-east-1");

    // Well-formed ids this service never allocated: both modelled prefixes, both ends of
    // the modelled length range, and several character classes the pattern allows. Row 1
    // is the id the issue recorded.
    let unallocated = [
        ("the recorded id", unallocated_domain_id("dom-", '2', 24)),
        ("the shortest dom- id (20)", unallocated_domain_id("dom-", 'a', 20)),
        ("the longest dom- id (25)", unallocated_domain_id("dom-", 'z', 25)),
        ("a legacy hai- id", unallocated_domain_id("hai-", '0', 21)),
        (
            "a named id that was never minted",
            "dom-doesnotexist00000".to_string(),
        ),
    ];

    for (case, domain_id) in &unallocated {
        // DeleteDomain: the front door cannot resolve an ARN for an id with no
        // allocation history, so its own authorizer failure is the answer — with no
        // detail text at all.
        let (status, body) = emulator
            .send("DELETE", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (
                AUTHORIZER_CONFIGURATION_STATUS,
                "AuthorizerConfigurationException"
            ),
            "case {case:?} ({domain_id}): expected the front door's refusal, got {status} \
             {body:?}"
        );
        assert!(
            body.get(DETAIL_MEMBER) == Some(&Value::Null)
                && body.get(RENDERED_DETAIL_MEMBER).is_none(),
            "case {case:?}: the refusal carries an EMPTY detail member, not text and not \
             an absent one, got {body:?}"
        );

        // A READ of the same id is the ordinary modelled not-found: the front-door
        // distinction is the delete's alone.
        let (status, read) = emulator
            .send("GET", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?} ({domain_id}): GetDomain of an unallocated id got {status} {read:?}"
        );
        assert_eq!(
            detail_text(case, &read),
            format!("Domain not found: {domain_id}"),
            "case {case:?}: the not-found must name the requested id, got {read:?}"
        );
    }

    // An id this service DID allocate names a domain that is gone: the modelled 404,
    // however many times the delete is repeated.
    let allocated = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-doomed" }))
            .await,
        "domainId",
    );
    let path = format!("/domain/{allocated}");
    let (status, deleted) = emulator.send("DELETE", &path, None).await;
    assert_eq!(status, 200, "the first DeleteDomain got {status} {deleted:?}");
    for attempt in 1..=2 {
        let (status, body) = emulator.send("DELETE", &path, None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "repeat {attempt} of DeleteDomain on an allocated id that is gone got {status} \
             {body:?}"
        );
        assert_eq!(
            detail_text("a domain that is gone", &body),
            format!("Domain not found: {allocated}"),
            "repeat {attempt}: the not-found must name the requested id, got {body:?}"
        );
    }
}

#[tokio::test]
async fn every_domain_lookup_words_its_not_found_the_same_way() {
    let emulator = Emulator::new("eu-west-1");
    // A live domain and subscription, so the operations below are shown to resolve what
    // exists before their 404s are read as genuine absences.
    let live_domain = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{live_domain}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {status} {subscription:?}");
    let live_subscription = string_at(&subscription, "subscriptionId");

    // A domain that was allocated and deleted, and one that never existed: both are
    // "not found" to every operation that resolves a domain.
    let gone = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-gone" }))
            .await,
        "domainId",
    );
    let (status, body) = emulator
        .send("DELETE", &format!("/domain/{gone}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain got {status} {body:?}");
    let never = unallocated_domain_id("dom-", 'z', 25);

    let session = "7f8b1c2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";
    for domain_id in [&gone, &never] {
        for (operation, method, path) in [
            ("GetDomain", "GET", format!("/domain/{domain_id}")),
            (
                "ListSubscriptions",
                "GET",
                format!("/domains/{domain_id}/subscriptions"),
            ),
            (
                "CreateSubscription",
                "POST",
                format!("/domains/{domain_id}/subscriptions"),
            ),
            (
                "GetSubscription",
                "GET",
                format!("/domains/{domain_id}/subscriptions/{live_subscription}"),
            ),
            (
                "GetMedicalScribeListeningSession",
                "GET",
                format!(
                    "/medical-scribe-stream/domain/{domain_id}/subscription/\
                     {live_subscription}/session/{session}"
                ),
            ),
        ] {
            let case = format!("{operation} under {domain_id}");
            let (status, body) = emulator.send(method, &path, Some(&json!({}))).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body:?}"
            );
            let text = detail_text(&case, &body);
            assert!(
                text.contains(domain_id.as_str()),
                "case {case:?}: the message must name the requested domain, got {text:?}"
            );
            // The operations that resolve the DOMAIN itself word it one way, stated once
            // in `service::error_detail::domain_not_found_message`.
            if matches!(
                operation,
                "GetDomain" | "ListSubscriptions" | "CreateSubscription"
            ) {
                assert_eq!(
                    text,
                    format!("Domain not found: {domain_id}"),
                    "case {case:?}: expected the service's domain-not-found wording, got \
                     {text:?}"
                );
            }
        }
    }
}

/// One seeded listening session: the status it is in, and whether it has ended.
struct SessionCase {
    case: &'static str,
    session_id: &'static str,
    status: ScribeStreamStatus,
    ended: bool,
}

/// Write `session` into the store of the subscription it belongs to, the way the
/// operation that STARTS a session would — that operation is not generated into this
/// crate (its input is an event stream), so this is how a stored session is reached.
fn seed_session(
    emulator: &Emulator,
    domain_id: &str,
    subscription_id: &str,
    case: &SessionCase,
) -> ScribeSession {
    let now = SystemTime::now();
    let session = ScribeSession {
        session_id: case.session_id.to_string(),
        domain_id: domain_id.to_string(),
        subscription_id: subscription_id.to_string(),
        // `MEDICAL_SCRIBE_LANGUAGE_CODE_VALUES` / `MEDICAL_SCRIBE_MEDIA_ENCODING_VALUES`
        // / `MEDICAL_SCRIBE_PARTICIPANT_ROLE_VALUES`: the modelled enum values.
        language_code: "en-US".to_string(),
        media_sample_rate_hertz: 16_000,
        media_encoding: "pcm".to_string(),
        channel_definitions: vec![
            MedicalScribeChannelDefinition {
                channel_id: 0,
                participant_role: "PATIENT".to_string(),
            },
            MedicalScribeChannelDefinition {
                channel_id: 1,
                participant_role: "CLINICIAN".to_string(),
            },
        ],
        post_stream_action_settings: None,
        post_stream_action_result: None,
        encounter_context_provided: false,
        stream_status: case.status,
        stream_creation_time: now,
        stream_end_time: case.ended.then_some(now),
    };
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let mut store = store.lock();
    let subscription = store
        .live_subscription_mut(emulator.region(), domain_id, subscription_id)
        .unwrap_or_else(|| panic!("case {:?}: a live subscription to seed", case.case));
    subscription
        .scribe_sessions
        .insert(session.session_id.clone(), session.clone());
    session
}

/// The path `GetMedicalScribeListeningSession` is addressed at
/// (`smithy_query(query="operation", operation="GetMedicalScribeListeningSession")`:
/// `GET /medical-scribe-stream/domain/{domainId}/subscription/{subscriptionId}/session/{sessionId}`).
fn session_path(domain_id: &str, subscription_id: &str, session_id: &str) -> String {
    format!(
        "/medical-scribe-stream/domain/{domain_id}/subscription/{subscription_id}/session/\
         {session_id}"
    )
}

#[tokio::test]
async fn a_listening_session_is_readable_only_under_its_own_subscription() {
    let emulator = Emulator::new("us-east-1");
    let home_domain = string_at(
        &emulator.create_domain(&json!({ "name": "clinic-home" })).await,
        "domainId",
    );
    let other_domain = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-other" }))
            .await,
        "domainId",
    );
    let mut subscriptions = Vec::new();
    for (domain, label) in [
        (&home_domain, "home"),
        (&home_domain, "sibling"),
        (&other_domain, "other domain"),
    ] {
        let (status, body) = emulator
            .send(
                "POST",
                &format!("/domains/{domain}/subscriptions"),
                Some(&json!({})),
            )
            .await;
        assert_eq!(
            status, 201,
            "the {label} subscription got {status} {body:?}"
        );
        subscriptions.push(string_at(&body, "subscriptionId"));
    }
    let (home, sibling, elsewhere) = (
        subscriptions[0].clone(),
        subscriptions[1].clone(),
        subscriptions[2].clone(),
    );

    // The rows differ along the session's own stored state — its status and whether it
    // has ended — and along the case of its hex digits, which the modelled pattern
    // admits in either case.
    let cases = [
        SessionCase {
            case: "a session still streaming",
            session_id: "11111111-2222-4333-8444-555555555555",
            status: ScribeStreamStatus::InProgress,
            ended: false,
        },
        SessionCase {
            case: "a paused session",
            session_id: "AABBCCDD-EEFF-4011-8233-445566778899",
            status: ScribeStreamStatus::Paused,
            ended: false,
        },
        SessionCase {
            case: "a completed session",
            session_id: "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
            status: ScribeStreamStatus::Completed,
            ended: true,
        },
        SessionCase {
            case: "a failed session",
            session_id: "deadbeef-0000-4000-8000-feedfacecafe",
            status: ScribeStreamStatus::Failed,
            ended: true,
        },
    ];

    for case in &cases {
        let stored = seed_session(&emulator, &home_domain, &home, case);

        // The home scope reports the PERSISTED session, member for member.
        let (status, body) = emulator
            .send(
                "GET",
                &session_path(&home_domain, &home, case.session_id),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: the session's own scope got {status} {body:?}",
            case.case
        );
        let details = body
            .get("medicalScribeListeningSessionDetails")
            .unwrap_or_else(|| panic!("case {:?}: the details member, got {body:?}", case.case));
        assert_eq!(
            (
                details.get("sessionId").and_then(Value::as_str),
                details.get("domainId").and_then(Value::as_str),
                details.get("subscriptionId").and_then(Value::as_str),
                details.get("streamStatus").and_then(Value::as_str),
                details.get("languageCode").and_then(Value::as_str),
                details.get("mediaEncoding").and_then(Value::as_str),
                details.get("mediaSampleRateHertz").and_then(Value::as_i64),
            ),
            (
                Some(stored.session_id.as_str()),
                Some(stored.domain_id.as_str()),
                Some(stored.subscription_id.as_str()),
                Some(stored.stream_status.as_wire()),
                Some(stored.language_code.as_str()),
                Some(stored.media_encoding.as_str()),
                Some(i64::from(stored.media_sample_rate_hertz)),
            ),
            "case {:?}: every member must be read back out of the stored session, got \
             {details:?}",
            case.case
        );
        assert_eq!(
            details
                .get("channelDefinitions")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(stored.channel_definitions.len()),
            "case {:?}: the stored channel definitions must be reported, got {details:?}",
            case.case
        );
        assert_eq!(
            details.get("streamEndTime").is_some(),
            case.ended,
            "case {:?}: a session reports an end instant exactly when it has one, got \
             {details:?}",
            case.case
        );

        // Every OTHER scope is the modelled not-found: a sibling subscription of the same
        // domain, a subscription of another domain, and the session id under a
        // subscription that does not exist.
        let absent_subscription = "sub-000000000000000000000";
        for (scope, domain_id, subscription_id) in [
            ("a sibling subscription", &home_domain, sibling.clone()),
            ("another domain's subscription", &other_domain, elsewhere.clone()),
            (
                "a subscription that does not exist",
                &home_domain,
                absent_subscription.to_string(),
            ),
        ] {
            let label = format!("{} through {scope}", case.case);
            let (status, body) = emulator
                .send(
                    "GET",
                    &session_path(domain_id, &subscription_id, case.session_id),
                    None,
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {label:?}: a session must never be reachable outside its own scope, \
                 got {status} {body:?}"
            );
            // Never the scaffold's signal: the operation has a lookup.
            assert_ne!(
                error_code(&body),
                "NotImplemented",
                "case {label:?}: got {body:?}"
            );
        }
    }

    // A well-formed session id that was never started, under a real domain and
    // subscription: the modelled 404, naming the id the request asked for.
    for never_started in [
        "00000000-0000-4000-8000-000000000000",
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
        "12345678-90ab-4cde-8f01-234567890abc",
    ] {
        let (status, body) = emulator
            .send(
                "GET",
                &session_path(&home_domain, &home, never_started),
                None,
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "a never-started session ({never_started}) got {status} {body:?}"
        );
        assert_eq!(
            detail_text("a never-started session", &body),
            format!("Resource not found: sessionId with ID {never_started} does not exist"),
            "the not-found must name the session the request asked for, got {body:?}"
        );
    }
}
