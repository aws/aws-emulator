// SPDX-License-Identifier: CC0-1.0

//! What a ConnectHealth response actually puts on the wire: the member a refusal reports
//! its detail text in, the class a `DeleteDomain` miss is refused as, and the instants a
//! create reports.
//!
//! Three rules, each asserted over a TABLE of inputs rather than the one request the
//! issue recorded:
//!
//! 1. Every refusal of this service reports the exception's detail text in ONE body
//!    member, and never under a second key. WHICH key is decided by the PLANE that
//!    answered: `Message` for the subscription plane
//!    (`/domains/{domainId}/subscriptions…`), and `message` — the member name the model
//!    spells — for the domain plane, the Medical Scribe streaming endpoint, the
//!    ARN-addressed tagging calls and the patient-insights runtime endpoint, which is also
//!    what the framework's own envelope writes for a request it refuses before any handler
//!    runs (`src/ops/types/exception_wire.rs`, and
//!    `vendor/smithy-server-runtime/src/pre_dispatch.rs`). The rows span the error CLASSES
//!    this service answers (404, 400 from a handler's bound, 400 from the boundary walk,
//!    401) across the operations that answer them and across both planes, so the rule is
//!    not keyed to the two operations the corpus exercises.
//! 2. `DeleteDomain` refuses every `domainId` it resolves no domain for with its ONE
//!    modelled error, `ResourceNotFoundException` at HTTP 404
//!    (`smithy_query(query="operation",operation="DeleteDomain")` lists exactly that one),
//!    naming the requested id: an id this service minted and has since purged, one naming
//!    a domain live in another Region, and one it never allocated anywhere are each a
//!    resource this endpoint does not have. Never a 5xx, which an SDK would retry on the
//!    normal idempotent-cleanup path. Every way an id can fail to resolve is a row.
//! 3. `CreateSubscription`'s 201 reports the instants creation ESTABLISHED and no others:
//!    `createdAt` always, and `lastUpdatedAt` / `activatedAt` / `deactivatedAt` only from
//!    the transition each one records, so a subscription no call has updated comes back
//!    without `lastUpdatedAt` — while a READ of that record reports the instant it was
//!    last written. The rows span the transition sequences a record can have taken.
//!
//! The harness (`Emulator`, `error_code`, `string_at`), the runtime endpoint's route and
//! its minimal body, and the web-app setup that keeps a domain in `DELETING` are the
//! sibling suites': one way of driving this service, not two.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::error_detail::{
    domain_not_found_message, DOMAIN_NOT_AUTHORIZED_MESSAGE,
};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{
    error_code, string_at, without_update_instant, Emulator, UPDATE_INSTANT_MEMBER,
};
use crate::patient_insights_job_lifecycle::{minimal_body, start_path};

/// The body member a refusal of a SUBSCRIPTION call reports its detail text in: the key
/// that plane keys it by (`src/ops/types/exception_wire.rs`).
const SUBSCRIPTION_PLANE_DETAIL: &str = "Message";

/// The body member every other plane of this service reports it in — the member name the
/// model spells — which is also what the framework's own refusal envelope writes for a
/// request refused before any handler ran
/// (`vendor/smithy-server-runtime/src/pre_dispatch.rs`).
const MODEL_MEMBER_DETAIL: &str = "message";

/// The other of the two keys: whichever one a row's answer must NOT carry, so one body
/// never states its text twice.
fn other_member(member: &str) -> &'static str {
    if member == SUBSCRIPTION_PLANE_DETAIL {
        MODEL_MEMBER_DETAIL
    } else {
        SUBSCRIPTION_PLANE_DETAIL
    }
}

/// The modelled upper bound of `maxResults` on both list operations
/// (`smithy_query(query="operation",operation="ListDomains,ListSubscriptions")` reports
/// `range {min: 1, max: 100}`), so one past it is refused.
const MAX_RESULTS_MAX: i64 = 100;

/// How long [`poll_until_absent`] keeps looking: many times over the span
/// `state::DOMAIN_DELETION_SPAN` owes, so a loaded host has room while a fast one never
/// waits at all.
const PURGE_DEADLINE: Duration = Duration::from_secs(30);

/// A well-formed `DomainId` that names nothing: `prefix` padded with `filler` to `len`.
///
/// `DomainId` is `@length 20..25` with pattern `^(hai-|dom-)[a-z0-9]+$`
/// (`smithy_query(query="shape",shape="DomainId")`), so both modelled prefixes and both
/// ends of the modelled length are reachable this way.
fn unresolvable_domain_id(prefix: &str, filler: char, len: usize) -> String {
    let mut id = prefix.to_string();
    while id.len() < len {
        id.push(filler);
    }
    id
}

/// The detail text of an error body, asserting it is reported under `member` — the key the
/// layer that rendered this answer writes — and under no other.
fn detail_text(case: &str, body: &Value, member: &str) -> String {
    let forbidden = other_member(member);
    assert!(
        body.get(forbidden).is_none(),
        "case {case:?}: an error body states its text once, under {member}, never also \
         under {forbidden}, got {body}"
    );
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the error body must carry a {member} member, got {body}")
        })
        .to_string()
}

/// Look at `GET /domain/{domainId}` until this Region holds no record under it — the purge
/// the clock owes a deleted domain, materialised by whichever call reads the store next —
/// or fail once [`PURGE_DEADLINE`] has passed.
///
/// The CONDITION is what is waited for, not an interval: the transition is owed at an
/// instant, so looking again is how a caller observes it, and a fixed wait would make the
/// outcome depend on how fast the host runs.
async fn poll_until_absent(emulator: &Emulator, domain_id: &str) {
    let deadline = std::time::Instant::now() + PURGE_DEADLINE;
    loop {
        let (status, body) = emulator
            .send("GET", &format!("/domain/{domain_id}"), None)
            .await;
        if status == 404 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the record under {domain_id} was still held after {PURGE_DEADLINE:?}, last read \
             {status} with {body}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// One refusal row: how to make the call, and what it must be answered with.
struct Refusal {
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
    /// The whole sentence when this service fixes it, or its opening when the rest is a
    /// per-request diagnostic tail.
    opening: String,
    whole: bool,
    /// The body member this row's answer reports its text in, which is decided by the
    /// layer that renders it.
    member: &'static str,
}

#[tokio::test]
async fn every_refusal_reports_its_detail_text_in_the_modelled_member() {
    let emulator = Emulator::new("us-west-2");
    let domain_id = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-wire" }))
            .await,
        "domainId",
    );
    let subscription_id = emulator.create_subscription(&domain_id).await;

    // A well-formed `ScribeSessionId` (36 characters of the modelled UUID shape) that no
    // session of this subscription was ever started under.
    let absent_session = "00000000-0000-4000-8000-000000000000";
    let unresolvable_arn =
        "arn:aws:health-agent:us-west-2:000000000000:domain/dom-zzzzzzzzzzzzzzzz";
    let encoded_arn = unresolvable_arn.replace(':', "%3A").replace('/', "%2F");

    let mut rows: Vec<Refusal> = Vec::new();
    // 404 on a domain that resolves to nothing, from every operation that resolves one:
    // one sentence, naming the id the request addressed.
    for absent_domain in [
        unresolvable_domain_id("dom-", '1', 20),
        unresolvable_domain_id("dom-", 'q', 25),
        unresolvable_domain_id("hai-", '0', 22),
    ] {
        for (operation, method, path, body, member) in [
            (
                "GetDomain",
                "GET",
                format!("/domain/{absent_domain}"),
                None,
                MODEL_MEMBER_DETAIL,
            ),
            (
                "ListSubscriptions",
                "GET",
                format!("/domains/{absent_domain}/subscriptions"),
                None,
                SUBSCRIPTION_PLANE_DETAIL,
            ),
            (
                "CreateSubscription",
                "POST",
                format!("/domains/{absent_domain}/subscriptions"),
                Some(json!({})),
                SUBSCRIPTION_PLANE_DETAIL,
            ),
        ] {
            rows.push(Refusal {
                case: format!("{operation} of the absent domain {absent_domain}"),
                method,
                path,
                body,
                status: 404,
                code: "ResourceNotFoundException",
                opening: domain_not_found_message(&absent_domain),
                whole: true,
                member,
            });
        }
    }
    // 404 from `DeleteDomain`, which is what an id this service ALLOCATED and resolves no
    // domain for earns (pinned over the whole class in
    // `delete_domain_refuses_every_unresolvable_id_on_the_ids_allocation_history` below);
    // the id this row addresses is one the service minted and has since purged.
    let purged = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-purged" }))
            .await,
        "domainId",
    );
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{purged}"), None)
        .await;
    assert_eq!(
        status, 200,
        "DeleteDomain of a live domain got {status} with {deleted}"
    );
    poll_until_absent(&emulator, &purged).await;
    rows.push(Refusal {
        case: format!("DeleteDomain of the purged domain {purged}"),
        method: "DELETE",
        path: format!("/domain/{purged}"),
        body: None,
        status: 404,
        code: "ResourceNotFoundException",
        opening: domain_not_found_message(&purged),
        whole: true,
        member: MODEL_MEMBER_DETAIL,
    });
    // 404 on a subscription, and on a listening session, under a scope that DOES resolve:
    // each names the identifier that could not be resolved.
    for absent_subscription in ["sub-000000000000000000000", "sub-AbCdEfGhIjKlMnOpQrStU"] {
        rows.push(Refusal {
            case: format!("GetSubscription of the absent {absent_subscription}"),
            method: "GET",
            path: format!("/domains/{domain_id}/subscriptions/{absent_subscription}"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            opening: format!("Subscription not found: {domain_id}/{absent_subscription}"),
            whole: true,
            member: SUBSCRIPTION_PLANE_DETAIL,
        });
    }
    rows.push(Refusal {
        case: format!("GetMedicalScribeListeningSession of the absent {absent_session}"),
        method: "GET",
        path: format!(
            "/medical-scribe-stream/domain/{domain_id}/subscription/{subscription_id}/session/{absent_session}"
        ),
        body: None,
        status: 404,
        code: "ResourceNotFoundException",
        opening: format!(
            "Resource not found: sessionId with ID {absent_session} does not exist"
        ),
        whole: true,
        member: MODEL_MEMBER_DETAIL,
    });
    // 404 on an ARN the tagging operations resolve to no resource of this service.
    rows.push(Refusal {
        case: "ListTagsForResource of an ARN that names nothing".to_string(),
        method: "GET",
        path: format!("/tags/{encoded_arn}"),
        body: None,
        status: 404,
        code: "ResourceNotFoundException",
        opening: format!("Resource not found: {unresolvable_arn}"),
        whole: true,
        member: MODEL_MEMBER_DETAIL,
    });
    // 400 from the HANDLER's own bound: a page size one past the modelled range, on both
    // listings, gets AWS's standard constraint sentence — rendered by the plane each
    // listing belongs to (`src/ops/types/list_subscriptions_types.rs`), which is what
    // decides the key it carries.
    let over_the_bound = MAX_RESULTS_MAX + 1;
    for (operation, base, member) in [
        ("ListDomains", "/domain".to_string(), MODEL_MEMBER_DETAIL),
        (
            "ListSubscriptions",
            format!("/domains/{domain_id}/subscriptions"),
            SUBSCRIPTION_PLANE_DETAIL,
        ),
    ] {
        rows.push(Refusal {
            case: format!("{operation} with maxResults={over_the_bound}"),
            method: "GET",
            path: format!("{base}?maxResults={over_the_bound}"),
            body: None,
            status: 400,
            code: "ValidationException",
            opening: format!(
                "1 validation error detected: Value '{over_the_bound}' at 'maxResults' failed \
                 to satisfy constraint: Member must have value less than or equal to \
                 {MAX_RESULTS_MAX}"
            ),
            whole: true,
            member,
        });
    }
    // 400 from the boundary walk, reworded by this service: a `domainId` outside what
    // `DomainId` declares, whose sentence carries a per-request diagnostic tail. The walk
    // renders it, so the framework's envelope names the member it travels in.
    for (label, malformed) in [
        ("too short (19)", "dom-aaaaaaaaaaaaaaa"),
        ("uppercase, outside the character class", "dom-AAAAAAAAAAAAAAAA"),
    ] {
        rows.push(Refusal {
            case: format!("GetDomain with a domainId {label}"),
            method: "GET",
            path: format!("/domain/{malformed}"),
            body: None,
            status: 400,
            code: "ValidationException",
            opening: format!("{malformed} is not a valid domain id."),
            whole: false,
            member: MODEL_MEMBER_DETAIL,
        });
    }
    // 401 from the patient-insights runtime endpoint, whose refusal is one FIXED sentence
    // naming nothing read off the request.
    rows.push(Refusal {
        case: "StartPatientInsightsJob under a domain that does not resolve".to_string(),
        method: "POST",
        path: start_path(&unresolvable_domain_id("dom-", '7', 21)),
        body: Some(minimal_body()),
        status: 401,
        code: "AccessDeniedException",
        opening: DOMAIN_NOT_AUTHORIZED_MESSAGE.to_string(),
        whole: true,
        member: MODEL_MEMBER_DETAIL,
    });

    for row in &rows {
        let (status, body) = emulator
            .send(row.method, &row.path, row.body.as_ref())
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (row.status, row.code),
            "case {:?}: expected HTTP {} {}, got {status} with {body}",
            row.case,
            row.status,
            row.code
        );
        let text = detail_text(&row.case, &body, row.member);
        if row.whole {
            assert_eq!(
                text, row.opening,
                "case {:?}: expected the detail text {:?}, got {text:?}",
                row.case, row.opening
            );
        } else {
            assert!(
                text.starts_with(&row.opening),
                "case {:?}: expected a detail text opening {:?}, got {text:?}",
                row.case,
                row.opening
            );
        }
    }
}

#[tokio::test]
async fn delete_domain_refuses_every_unresolvable_id_with_the_modelled_not_found() {
    let emulator = Emulator::new("us-east-1");

    // Every way a well-formed `domainId` can name no domain this Region holds, spanning what
    // the rule generalises over: ids no account was ever allocated (both modelled prefixes,
    // both ends of the modelled length, digits, repeated letters and a pronounceable id),
    // an id this service minted whose domain it has since purged, and one that is live in
    // another Region. Every one of them names a resource this endpoint does not have, which
    // is the ONE error this operation models
    // (`smithy_query(query="operation",operation="DeleteDomain")` reports
    // `ResourceNotFoundException` alone), stated with the id the request addressed — the
    // id's own allocation history cannot change the answer.
    let mut rows: Vec<(String, String)> = Vec::new();
    for (label, id) in [
        (
            "never minted, shortest dom-",
            unresolvable_domain_id("dom-", 'b', 20),
        ),
        (
            "never minted, longest dom-",
            unresolvable_domain_id("dom-", 'y', 25),
        ),
        (
            "never minted, legacy hai-",
            unresolvable_domain_id("hai-", '3', 23),
        ),
        (
            "never minted, digits only",
            unresolvable_domain_id("dom-", '2', 24),
        ),
        (
            "never minted, a named id",
            "dom-doesnotexist00000".to_string(),
        ),
    ] {
        rows.push((label.to_string(), id));
    }
    // An id this service DID mint, whose domain it has since purged: the delete that
    // follows the delete is the idempotent-cleanup path, and it has to terminate.
    let purged = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-purged" }))
            .await,
        "domainId",
    );
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{purged}"), None)
        .await;
    assert_eq!(status, 200, "the first DeleteDomain got {status} {deleted:?}");
    poll_until_absent(&emulator, &purged).await;
    rows.push(("minted and since purged".to_string(), purged));
    // A LIVE domain of another Region: its id is one this service minted, and this
    // Region's endpoint holds no domain under it.
    let elsewhere = string_at(
        &emulator
            .create_domain_in("eu-west-1", &json!({ "name": "clinic-elsewhere" }))
            .await,
        "domainId",
    );
    rows.push((
        "live in another Region".to_string(),
        elsewhere.clone(),
    ));

    for (case, domain_id) in &rows {
        // Twice: a repeat of the refused delete answers the identical refusal, because
        // the model declares the operation `@idempotent` and a miss writes nothing.
        for attempt in 1..=2 {
            let (status, body) = emulator
                .send("DELETE", &format!("/domain/{domain_id}"), None)
                .await;
            let case = format!("{case} ({domain_id}), attempt {attempt}");
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected the modelled not-found, got {status} with {body}"
            );
            assert_eq!(
                detail_text(&case, &body, MODEL_MEMBER_DETAIL),
                domain_not_found_message(domain_id),
                "case {case:?}: the refusal must name the requested id, got {body}"
            );
            assert!(
                body.get(SUBSCRIPTION_PLANE_DETAIL).is_none(),
                "case {case:?}: no second detail key, got {body}"
            );
        }
    }

    // The domain the rows addressed from the wrong Region is untouched by all of it: a
    // refusal deleted nothing.
    let (status, still_there) = emulator
        .send_in("eu-west-1", "GET", &format!("/domain/{elsewhere}"), None)
        .await;
    assert_eq!(
        (status, string_at(&still_there, "status").as_str()),
        (200, "ACTIVE"),
        "a refused delete must leave the other Region's domain exactly as it was, got \
         {status} with {still_there}"
    );
}

#[tokio::test]
async fn a_create_reports_no_update_instant_until_a_transition_stamps_one() {
    // The rows differ along the transition sequence the record takes AFTER the create,
    // and along whether its domain owns a web application — neither of which may change
    // what the create itself reports.
    let rows: [(&str, &[&str], bool); 5] = [
        ("no transition", &[], false),
        ("deactivated", &["deactivate"], false),
        ("re-activated", &["deactivate", "activate"], true),
        ("activated from the start", &["activate"], false),
        (
            "alternated four times",
            &["activate", "deactivate", "activate", "deactivate"],
            true,
        ),
    ];

    for (case, transitions, web_app) in rows {
        let emulator = Emulator::new("eu-central-1");
        let mut request = json!({ "name": "clinic-instants" });
        if web_app {
            request["webAppSetupConfiguration"] = web_app_setup();
        }
        let domain_id = string_at(&emulator.create_domain(&request).await, "domainId");

        let (status, created) = emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateSubscription got {status} with {created}"
        );
        // Creation establishes the record and its status; it updates nothing and takes
        // neither transition, so it reports the creation instant and none of the three
        // instants an event stamps.
        assert!(
            created.get("createdAt").is_some(),
            "case {case:?}: the create must report createdAt, got {created}"
        );
        for member in [UPDATE_INSTANT_MEMBER, "activatedAt", "deactivatedAt"] {
            assert!(
                created.get(member).is_none(),
                "case {case:?}: a create that has taken no {member} event must not report \
                 it, got {created}"
            );
        }

        let subscription_id = string_at(&created, "subscriptionId");
        let path = format!("/domains/{domain_id}/subscriptions/{subscription_id}");

        // A READ of the same record reports the instant it was last written — which for a
        // record no call has updated is its own creation instant — and agrees with the
        // create about every other member.
        let (status, read) = emulator.send("GET", &path, None).await;
        let reported = read
            .get("subscription")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: a subscription member in {read}"));
        assert_eq!(
            status, 200,
            "case {case:?}: GetSubscription got {status} with {read}"
        );
        assert_eq!(
            without_update_instant(&reported),
            without_update_instant(&created),
            "case {case:?}: the reader must report the record the create projected, \
             expected {created}, got {reported}"
        );
        assert_eq!(
            reported.get(UPDATE_INSTANT_MEMBER),
            created.get("createdAt"),
            "case {case:?}: a record no call has updated was last written when it was \
             created, got {reported}"
        );

        // Once a transition has stamped an update instant, every responder reports it —
        // including the transition's own answer — and it is the instant of that
        // transition, which is also what the record's `activatedAt`/`deactivatedAt`
        // carries.
        for transition in transitions {
            let (status, answered) = emulator
                .send("POST", &format!("{path}/{transition}"), Some(&json!({})))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {transition} got {status} with {answered}"
            );
            let stamped = answered
                .get("subscription")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: a subscription member in {answered}"));
            let stamp = match *transition {
                "activate" => "activatedAt",
                _ => "deactivatedAt",
            };
            assert_eq!(
                stamped.get(UPDATE_INSTANT_MEMBER),
                stamped.get(stamp),
                "case {case:?}: {transition} must report the update instant it stamped, \
                 got {stamped}"
            );
            let (status, read) = emulator.send("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetSubscription after {transition} got {status} with {read}"
            );
            assert_eq!(
                read.get("subscription"),
                Some(&stamped),
                "case {case:?}: the reader must report what {transition} wrote, got {read}"
            );
        }
    }
}
