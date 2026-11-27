// SPDX-License-Identifier: CC0-1.0

//! Where a ConnectHealth failure reports its detail text, and under which key.
//!
//! The RULE under test, asserted over a table rather than one recorded response: the body
//! member an error of this service carries the exception's detail text in is decided by
//! the PLANE that answered the call, not by the error class, the operation's name or the
//! exception shape — which declares one member, `message`, with no `@jsonName`. The
//! SUBSCRIPTION plane (`/domains/{domainId}/subscriptions…`: the reader, the listing, the
//! create and the two status transitions) keys it `Message`; the domain plane
//! (`/domain/…`), the Medical Scribe streaming endpoint, the ARN-addressed tagging calls
//! and the patient-insights runtime endpoint key it `message`, the name the model spells,
//! which is also what the framework's own envelope writes for a request it refuses before
//! any handler runs (`vendor/smithy-server-runtime/src/pre_dispatch.rs`). The key is set
//! in ONE place, the member schema each of this service's exception shapes is serialized
//! through (`src/ops/types/exception_wire.rs`), and a body states its text under exactly
//! one of the two.
//!
//! The text itself is built from the REQUEST:
//!
//! * a not-found names the identifiers the request asked for — the subscription and its
//!   parent domain when a subscription could not be found, the domain when the domain
//!   could not be found;
//! * a value outside a member's modelled `@range` gets AWS's standard constraint sentence,
//!   `N validation error(s) detected: Value '<value>' at '<member>' failed to satisfy
//!   constraint: <requirement>`, naming the offending value, the member path and the bound
//!   it violated;
//! * a `domainId` the service cannot read as a domain id gets the service's own sentence
//!   instead of that clause (`src/service/domain_id_refusal.rs`), naming the value the
//!   request supplied.
//!
//! The rows differ along what the rule generalises over: which PLANE answers, which
//! operation is called, which error CLASS answers (404 `ResourceNotFoundException`, 400
//! `ValidationException`, 401 `AccessDeniedException`), whether the call reports or
//! mutates, which of the two identifiers is the missing one, the form of the identifiers
//! (both modelled `DomainId` prefixes, the shortest and longest the length constraint
//! allows, mixed-case `SubscriptionId` suffixes), and — for the range — each published
//! bound and one past each. The ids the issue recorded are two rows of that table.
//!
//! Model citations, all from
//! `smithy_query(query="operation",operation="GetSubscription,ActivateSubscription,DeactivateSubscription,ListSubscriptions,CreateSubscription",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! and `smithy_query(query="shape",shape="ResourceNotFoundException,ValidationException")`:
//! `DomainId` is `@length 20..25` with pattern `^(hai-|dom-)[a-z0-9]+$`, `SubscriptionId` is
//! `@length 25..25` with pattern `^sub-[a-zA-Z0-9]{21}$`, `ListSubscriptionsInput.maxResults`
//! is an `Integer` with `@range{min:1,max:100}` bound as `@httpQuery("maxResults")`, and both
//! `ResourceNotFoundException` (404) and `ValidationException` (400) model exactly one
//! member, `message` -> `String`.

use serde_json::{json, Value};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The body member the SUBSCRIPTION plane reports an exception's detail text in: the key
/// that plane keys it by, set on the member schema every exception shape of this service
/// is serialized through (`src/ops/types/exception_wire.rs`) and carried by every recorded
/// refusal of a subscription call.
const SUBSCRIPTION_PLANE_DETAIL: &str = "Message";

/// The body member every other plane of this service reports it in — the member name the
/// model spells, recorded for the domain plane and the Medical Scribe streaming endpoint —
/// which is also what the framework's own envelope writes for a request it refuses before
/// any handler runs (`vendor/smithy-server-runtime/src/pre_dispatch.rs` names that member
/// itself, and no service seam reaches it).
const MODEL_MEMBER_DETAIL: &str = "message";

/// The modelled `@range` of `maxResults` on both list operations, asserted at each bound
/// and one past each (`smithy_query(query="operation",operation="ListSubscriptions")`
/// reports `range {min: 1, max: 100}`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The detail text an error body carries under `member` — the key the plane that answered
/// keys it by — asserting it carries it under that member and under no other, so a client
/// never has to read two keys for one answer.
fn detail_text_in(case: &str, body: &Value, member: &str) -> String {
    let other = if member == SUBSCRIPTION_PLANE_DETAIL {
        MODEL_MEMBER_DETAIL
    } else {
        SUBSCRIPTION_PLANE_DETAIL
    };
    assert!(
        body.get(other).is_none(),
        "case {case:?}: the text is stated once, under {member}, never also under \
         {other}, got {body}"
    );
    let text = body
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the error body must carry a {member} member, got {body}")
        });
    assert!(
        !text.trim().is_empty(),
        "case {case:?}: {member} must carry the detail text, got {text:?}"
    );
    text.to_string()
}

/// AWS's standard constraint-violation sentence for one violation, written here
/// independently of the emulator's own renderer.
///
/// Source: the envelope AWS documents for a modelled constraint violation, and the form
/// the corpus recorded for `maxResults=101`
/// ("1 validation error detected: Value '101' at 'maxResults' failed to satisfy
/// constraint: Member must have value less than or equal to 100").
fn one_validation_error(value: i64, member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// A well-formed `DomainId` that names nothing: `prefix` + `filler` repeated to `len`.
///
/// Both modelled prefixes and the whole modelled length range are reachable this way, so
/// the table does not depend on one recorded id.
fn absent_domain_id(prefix: &str, len: usize) -> String {
    let mut id = prefix.to_string();
    while id.len() < len {
        id.push('z');
    }
    id
}

/// One not-found row: the request to make, every identifier its answer must name, and the
/// member the plane that answers reports the text in.
struct NotFound {
    case: String,
    method: &'static str,
    path: String,
    named: Vec<String>,
    member: &'static str,
}

#[tokio::test]
async fn every_not_found_names_the_requested_identifiers_in_the_detail_member() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );
    let subscription_id = string_at(
        &emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await
            .1,
        "subscriptionId",
    );

    // Round trip first: while both exist, the readers find them — so every 404 below is a
    // genuine absence rather than a blanket refusal.
    let (status, found) = emulator
        .send(
            "GET",
            &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "GetSubscription of a live subscription: {found}");
    for member in [SUBSCRIPTION_PLANE_DETAIL, MODEL_MEMBER_DETAIL] {
        assert!(
            found.get(member).is_none(),
            "a success carries no detail member ({member}), got {found}"
        );
    }

    // Absent domains across both modelled prefixes and the ends of the modelled length
    // range (20..25), and absent subscriptions across the character classes the
    // `SubscriptionId` pattern allows (21 of `[a-zA-Z0-9]`).
    let absent_domains = [
        absent_domain_id("dom-doesnotexist", 21),
        absent_domain_id("dom-", 20),
        absent_domain_id("dom-", 25),
        absent_domain_id("hai-", 20),
        absent_domain_id("hai-legacy", 25),
    ];
    let absent_subscriptions = [
        "sub-000000000000000000000",
        "sub-d4d4d4d4d4d4d4d4d4d4d",
        "sub-AbCdEfGhIjKlMnOpQrStU",
        "sub-zzzzzzzzzzzzzzzzzzzzz",
        "sub-000000000000000000001",
    ];

    // A domain this service allocated and has since deleted: its id names a domain that
    // is GONE, so every reader of it — including the repeated delete — names it in the
    // modelled not-found. (`DeleteDomain` answers that same 404 for an id this service
    // never minted; see `domain_account_cap_and_lookup_semantics`.)
    let deleted_domain = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-deleted" }))
            .await,
        "domainId",
    );
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{deleted_domain}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain of a live domain: {deleted}");

    let mut rows: Vec<NotFound> = Vec::new();
    for (operation, method) in [("GetDomain", "GET"), ("DeleteDomain", "DELETE")] {
        rows.push(NotFound {
            case: format!("{operation} of the deleted domain {deleted_domain}"),
            method,
            path: format!("/domain/{deleted_domain}"),
            named: vec![deleted_domain.clone()],
            // The domain plane answers these.
            member: MODEL_MEMBER_DETAIL,
        });
    }
    for absent_subscription in absent_subscriptions {
        // A subscription that is not in a domain that EXISTS: the answer must tell the
        // two identifiers apart, so it names both.
        for (operation, method, suffix) in [
            ("GetSubscription", "GET", ""),
            ("ActivateSubscription", "POST", "/activate"),
            ("DeactivateSubscription", "POST", "/deactivate"),
        ] {
            rows.push(NotFound {
                case: format!("{operation} of {absent_subscription} in a live domain"),
                method,
                path: format!(
                    "/domains/{domain_id}/subscriptions/{absent_subscription}{suffix}"
                ),
                named: vec![domain_id.clone(), absent_subscription.to_string()],
                member: SUBSCRIPTION_PLANE_DETAIL,
            });
        }
    }
    for absent_domain in &absent_domains {
        // The domain itself is the missing one: every operation addressed to it names it,
        // each in the member its own plane keys the text by.
        for (operation, method, path, member) in [
            (
                "GetDomain",
                "GET",
                format!("/domain/{absent_domain}"),
                MODEL_MEMBER_DETAIL,
            ),
            (
                "ListSubscriptions",
                "GET",
                format!("/domains/{absent_domain}/subscriptions"),
                SUBSCRIPTION_PLANE_DETAIL,
            ),
            (
                "CreateSubscription",
                "POST",
                format!("/domains/{absent_domain}/subscriptions"),
                SUBSCRIPTION_PLANE_DETAIL,
            ),
        ] {
            rows.push(NotFound {
                case: format!("{operation} under the absent domain {absent_domain}"),
                method,
                path,
                named: vec![absent_domain.clone()],
                member,
            });
        }
        // A live subscription id under an absent domain: both identifiers again.
        rows.push(NotFound {
            case: format!("GetSubscription of a live subscription under {absent_domain}"),
            method: "GET",
            path: format!("/domains/{absent_domain}/subscriptions/{subscription_id}"),
            named: vec![absent_domain.clone(), subscription_id.clone()],
            member: SUBSCRIPTION_PLANE_DETAIL,
        });
    }

    for row in &rows {
        let (status, body) = emulator
            .send(row.method, &row.path, Some(&json!({})))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {:?}: expected 404 ResourceNotFoundException, got {status} with {body}",
            row.case
        );
        let text = detail_text_in(&row.case, &body, row.member);
        for identifier in &row.named {
            assert!(
                text.contains(identifier),
                "case {:?}: the message must name {identifier:?}, got {text:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_with_the_standard_constraint_message() {
    let emulator = Emulator::new("eu-central-1");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );

    let (min, max) = MAX_RESULTS_RANGE;
    // Each published bound, and one past each, plus two further values outside it so the
    // sentence is not keyed to the recorded 101.
    let rows: [(i64, u16); 7] = [
        (min - 1, 400),
        (-7, 400),
        (min, 200),
        (max, 200),
        (max + 1, 400),
        (1_000, 400),
        (i64::from(i32::MAX), 400),
    ];
    // Both list operations bind `maxResults` the same way, so the rule covers both — each
    // answered by its own plane, which is what decides the member the text is in.
    for (operation, path, member) in [
        (
            "ListSubscriptions",
            format!("/domains/{domain_id}/subscriptions"),
            SUBSCRIPTION_PLANE_DETAIL,
        ),
        ("ListDomains", "/domain".to_string(), MODEL_MEMBER_DETAIL),
    ] {
        for (max_results, expected_status) in rows {
            let case = format!("{operation} with maxResults={max_results}");
            let (status, body) = emulator
                .send("GET", &format!("{path}?maxResults={max_results}"), None)
                .await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
            );
            if expected_status == 200 {
                continue;
            }
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: got {body}"
            );
            let requirement = if max_results < min {
                format!("Member must have value greater than or equal to {min}")
            } else {
                format!("Member must have value less than or equal to {max}")
            };
            let text = detail_text_in(&case, &body, member);
            assert_eq!(
                text,
                one_validation_error(max_results, "maxResults", &requirement),
                "case {case:?}: expected AWS's standard constraint sentence, got {text:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_refusal_made_before_a_handler_ran_still_states_why() {
    let emulator = Emulator::new("us-west-2");

    // A `domainId` outside the model's own length/pattern never reaches a handler: the
    // boundary validator refuses it, and this service words that refusal itself (its own
    // domain-id sentence, pinned over the whole class by
    // `tests/all/malformed_domain_id_refusal.rs`). The envelope is the FRAMEWORK's, so the
    // text arrives in the member that envelope names; the sentence still names the value
    // the request supplied.
    let malformed: [(&str, &str); 5] = [
        ("too short (19)", "dom-aaaaaaaaaaaaaaa"),
        ("too long (26)", "dom-aaaaaaaaaaaaaaaaaaaaaa"),
        ("uppercase, outside the character class", "dom-AAAAAAAAAAAAAAAA"),
        ("underscore, outside the character class", "dom-aaaaaaaaaaaa_aaa"),
        ("an unmodelled prefix", "zzz-aaaaaaaaaaaaaaaa"),
    ];
    for (case, bad_id) in malformed {
        let (status, body) = emulator
            .send("GET", &format!("/domain/{bad_id}"), None)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({bad_id}): expected 400 ValidationException, got {status} with {body}"
        );
        let text = detail_text_in(case, &body, MODEL_MEMBER_DETAIL);
        let expected_opening = format!("{bad_id} is not a valid domain id. (Service: ");
        assert!(
            text.starts_with(&expected_opening),
            "case {case:?}: expected a message starting {expected_opening:?}, got {text:?}"
        );
    }

    // Every operation at the ARN-addressed tagging URI resolves the ARN in its HANDLER,
    // whichever method the request used — and that plane keys the text as the model names
    // the member too, so the same key answers both halves of this test.
    let arn = "arn:aws:health-agent:us-west-2:000000000000:domain/dom-zzzzzzzzzzzzzzzz";
    let encoded = arn.replace(':', "%3A").replace('/', "%2F");
    // `tagKeys` is a required `@httpQuery` list of 1..50 keys
    // (`smithy_query(query="operation",operation="UntagResource")`), so a removal names at
    // least one.
    for (case, method, path) in [
        ("ListTagsForResource", "GET", format!("/tags/{encoded}")),
        ("TagResource", "POST", format!("/tags/{encoded}")),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{encoded}?tagKeys=env"),
        ),
    ] {
        let body = (method == "POST").then(|| json!({ "tags": { "env": "prod" } }));
        let (status, answered) = emulator.send(method, &path, body.as_ref()).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an unresolvable ARN must answer the modelled 404, got {status} \
             with {answered}"
        );
        let text = detail_text_in(case, &answered, MODEL_MEMBER_DETAIL);
        assert!(
            text.contains(arn),
            "case {case:?}: the 404 must name the ARN the request asked for, got {text:?}"
        );
    }
}

/// One row of the class table: the call to make, the answer it earns, and the member the
/// plane that answers reports the detail text in.
struct Rendered {
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
    member: &'static str,
}

/// RULE: every error class this service answers reports its detail text under the key the
/// PLANE that answered keys it by, and under that key alone — so a client reading an error
/// body never has to look in a second place, whichever class answered and whichever
/// operation it called.
///
/// The rows span the error CLASSES reachable over this service's HTTP surface — 404
/// `ResourceNotFoundException` from a reader, from a state transition and from a listing,
/// 400 `ValidationException` from each bound a handler applies itself, and 401
/// `AccessDeniedException` from the patient-insights runtime endpoint — across both planes,
/// readers and mutations, three HTTP methods and both URI families. The remaining classes
/// are driven where their condition lives: 500 `InternalServerException` in
/// `tests/all/store_unreachable_channel.rs` and the event-stream operation's own refusals
/// in `tests/all/medical_scribe_listening_session_stream.rs`.
#[tokio::test]
async fn every_class_is_keyed_by_the_plane_that_answered() {
    let emulator = Emulator::new("ap-southeast-2");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );
    let subscription_id = string_at(
        &emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await
            .1,
        "subscriptionId",
    );
    let absent_domain = absent_domain_id("hai-", 23);
    let absent_subscription = "sub-AbCdEfGhIjKlMnOpQrStU";
    let (min, max) = MAX_RESULTS_RANGE;
    let over_long_key = "k".repeat(129);

    // `DeleteDomain`'s modelled 404 is what every `domainId` it resolves no domain for
    // earns, whatever that id's own history (the class is pinned in
    // `tests/all/domain_account_cap_and_lookup_semantics.rs` and
    // `tests/all/recorded_refusal_and_create_members.rs`). The mutation row below
    // addresses a domain created and deleted here.
    let purged_domain = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-purged" }))
            .await,
        "domainId",
    );
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{purged_domain}"), None)
        .await;
    assert_eq!(
        status, 200,
        "DeleteDomain of a live domain got {status} with {deleted}"
    );

    let rows: Vec<Rendered> = vec![
        Rendered {
            case: "GetSubscription of an absent subscription (a reader)".to_string(),
            method: "GET",
            path: format!("/domains/{domain_id}/subscriptions/{absent_subscription}"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            member: SUBSCRIPTION_PLANE_DETAIL,
        },
        Rendered {
            case: "ActivateSubscription of an absent subscription (a transition)".to_string(),
            method: "POST",
            path: format!("/domains/{domain_id}/subscriptions/{absent_subscription}/activate"),
            body: Some(json!({})),
            status: 404,
            code: "ResourceNotFoundException",
            member: SUBSCRIPTION_PLANE_DETAIL,
        },
        Rendered {
            case: "DeactivateSubscription of an absent subscription (a transition)".to_string(),
            method: "POST",
            path: format!("/domains/{domain_id}/subscriptions/{absent_subscription}/deactivate"),
            body: Some(json!({})),
            status: 404,
            code: "ResourceNotFoundException",
            member: SUBSCRIPTION_PLANE_DETAIL,
        },
        Rendered {
            case: "ListSubscriptions under an absent domain (a listing)".to_string(),
            method: "GET",
            path: format!("/domains/{absent_domain}/subscriptions"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            member: SUBSCRIPTION_PLANE_DETAIL,
        },
        Rendered {
            case: "GetDomain of an absent domain".to_string(),
            method: "GET",
            path: format!("/domain/{absent_domain}"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            member: MODEL_MEMBER_DETAIL,
        },
        Rendered {
            case: "DeleteDomain of a domain that is gone (a mutation)".to_string(),
            method: "DELETE",
            path: format!("/domain/{purged_domain}"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            member: MODEL_MEMBER_DETAIL,
        },
        Rendered {
            case: format!(
                "ListSubscriptions with maxResults={} (above the bound)",
                max + 1
            ),
            method: "GET",
            path: format!("/domains/{domain_id}/subscriptions?maxResults={}", max + 1),
            body: None,
            status: 400,
            code: "ValidationException",
            member: SUBSCRIPTION_PLANE_DETAIL,
        },
        Rendered {
            case: format!("ListDomains with maxResults={} (below the bound)", min - 1),
            method: "GET",
            path: format!("/domain?maxResults={}", min - 1),
            body: None,
            status: 400,
            code: "ValidationException",
            member: MODEL_MEMBER_DETAIL,
        },
        Rendered {
            case: "ListSubscriptions with a nextToken this service did not issue".to_string(),
            method: "GET",
            path: format!("/domains/{domain_id}/subscriptions?nextToken=deadbeef"),
            body: None,
            status: 400,
            code: "ValidationException",
            member: SUBSCRIPTION_PLANE_DETAIL,
        },
        Rendered {
            // `TagKey` is `@length 1..128`, so a key past it is refused by the one entry
            // rule every operation that accepts a `TagMap` applies
            // (`src/service/tag_bounds.rs`).
            case: "CreateDomain with a tag key past its modelled length".to_string(),
            method: "POST",
            path: "/domain".to_string(),
            body: Some(json!({
                "name": "clinic-tagged",
                "tags": { over_long_key.clone(): "v" },
            })),
            status: 400,
            code: "ValidationException",
            member: MODEL_MEMBER_DETAIL,
        },
        Rendered {
            // The patient-insights runtime endpoint authorizes the domain before it
            // resolves anything, so an unresolvable one is an authorization failure.
            case: "StartPatientInsightsJob under an absent domain".to_string(),
            method: "POST",
            path: format!("/domain/{absent_domain}/patient-insights-job"),
            body: Some(crate::patient_insights_job_lifecycle::minimal_body()),
            status: 401,
            code: "AccessDeniedException",
            member: MODEL_MEMBER_DETAIL,
        },
    ];

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
        // Under the key its own plane writes, and nowhere else: a body that also carried
        // the other key would state its text twice.
        let text = detail_text_in(&row.case, &body, row.member);
        assert!(
            !text.trim().is_empty(),
            "case {:?}: the refusal must say why, got {text:?}",
            row.case
        );
    }

    // The service still SERVES what it serves: the live subscription reads back, so every
    // refusal above is a refusal of the request rather than of the operation.
    let (status, served) = emulator
        .send(
            "GET",
            &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "GetSubscription of a live subscription: {served}"
    );
    for member in [SUBSCRIPTION_PLANE_DETAIL, MODEL_MEMBER_DETAIL] {
        assert!(
            served.get(member).is_none(),
            "a success carries no detail member ({member}), got {served}"
        );
    }
}

