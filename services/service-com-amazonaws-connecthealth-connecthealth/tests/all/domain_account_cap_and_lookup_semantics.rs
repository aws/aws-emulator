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
//! 2. `DeleteDomain` refuses every `domainId` it resolves no domain for with the ONE error
//!    the operation models, `ResourceNotFoundException` at HTTP 404, whose detail member
//!    names the requested id: one this service minted and has since purged, one whose
//!    domain is live in another Region, and one it never minted are each that. A READ
//!    (`GetDomain`) resolves the domain itself, so it reports the same modelled 404. The
//!    answer turns on what the Region HOLDS and on nothing about the id itself, which is
//!    asserted by driving ONE AND THE SAME id with and without allocation history, and an
//!    id the account was allocated keeps the modelled 404 for every later delete, all the
//!    way through the purge the clock owes it.
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

/// How long the poll below keeps looking for the purge the clock owes: many times over
/// the span `state::DOMAIN_DELETION_SPAN` owes, so a loaded host has room while a fast
/// one never waits at all.
const PURGE_DEADLINE: Duration = Duration::from_secs(30);

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

/// The detail text of an error body, read through the one reader this test target shares
/// ([`crate::domain_subscription_lifecycle::detail_text`]): the rows below span this
/// service's planes, which key that text differently
/// (`src/ops/types/exception_wire.rs`, pinned per plane in
/// `tests/all/error_detail_member.rs`). What this suite is about is the cap and the
/// lookup.
fn detail_text(case: &str, body: &Value) -> String {
    crate::domain_subscription_lifecycle::detail_text(case, body)
}

/// A well-formed `DomainId` that names nothing: `prefix` padded with `filler` to `len`.
///
/// Both modelled prefixes and the whole modelled length range (20..25) are reachable
/// this way, so no row depends on the one id the issue recorded.
fn unresolvable_domain_id(prefix: &str, filler: char, len: usize) -> String {
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

    // The purge the clock owes is observed by LOOKING again, not by waiting a fixed
    // interval: it falls due at an instant, and a fixed wait would make the outcome
    // depend on how fast the host runs.
    let deadline = std::time::Instant::now() + PURGE_DEADLINE;
    loop {
        let (status, body) = emulator
            .send("GET", &format!("/domain/{web}"), None)
            .await;
        if status == 404 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the record under {web} was still held after {PURGE_DEADLINE:?}, last read \
             {status} with {body:?}"
        );
        ::tokio::task::yield_now().await;
    }
    let (status, created) = emulator
        .send("POST", "/domain", Some(&json!({ "name": "after-the-purge" })))
        .await;
    assert_eq!(
        status, 201,
        "once the purge the clock owed has fallen due the slot is free, got {status} \
         {created:?}"
    );
}

/// Why a well-formed `domainId` resolves no domain in the Region a request is signed
/// for — which is what the rows below span, and what the refusal is the SAME for: an id
/// this service never allocated, and one it allocated and no longer holds here, are both
/// a resource this endpoint does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unresolvable {
    /// No allocation history at all.
    NeverAllocated,
    /// Allocated by this service, and not held in the Region the request names.
    AllocatedAndLost,
}

/// Assert how `DeleteDomain` refuses an id that resolves nothing: with the ONE error
/// `smithy_query(query="operation", operation="DeleteDomain")` lists for it,
/// `ResourceNotFoundException` at 404, whose detail member names the requested id —
/// whatever that id's own allocation history ([`Unresolvable`] spans both, and both take
/// this answer, because what the Region HOLDS is the whole of what the refusal depends on).
///
/// The status is a CLIENT fault: the condition is permanent and lies in the request's own
/// id, and an SDK retries a server fault.
fn assert_delete_refusal(
    case: &str,
    kind: Unresolvable,
    domain_id: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?} ({kind:?}): expected the modelled not-found, got {status} \
         {body:?}"
    );
    assert_eq!(
        detail_text(case, body),
        format!("Domain not found: {domain_id}"),
        "case {case:?} ({kind:?}): the not-found must name the requested id, got \
         {body:?}"
    );
    assert!(
        status < 500,
        "case {case:?} ({kind:?}): the condition lies in the request's own id and is \
         permanent, so it is never the server fault an SDK retries, got {status} {body:?}"
    );
}

#[tokio::test]
async fn delete_domain_refuses_every_unresolvable_id_with_the_modelled_not_found() {
    let emulator = Emulator::new("us-east-1");

    // Every way a well-formed `domainId` can resolve no domain in the Region a request is
    // signed for: ids this service never minted (both modelled prefixes, both ends of the
    // modelled length range, digits only, and a pronounceable id), an id it minted and has
    // since purged, and the id of a domain that is LIVE in another Region.
    let mut rows: Vec<(String, Unresolvable, String)> = vec![
        (
            "never minted, digits only (24)".to_string(),
            Unresolvable::NeverAllocated,
            unresolvable_domain_id("dom-", '2', 24),
        ),
        (
            "never minted, the shortest dom- id (20)".to_string(),
            Unresolvable::NeverAllocated,
            unresolvable_domain_id("dom-", 'a', 20),
        ),
        (
            "never minted, the longest dom- id (25)".to_string(),
            Unresolvable::NeverAllocated,
            unresolvable_domain_id("dom-", 'z', 25),
        ),
        (
            "never minted, a legacy hai- id".to_string(),
            Unresolvable::NeverAllocated,
            unresolvable_domain_id("hai-", '0', 21),
        ),
        (
            "never minted, a named id".to_string(),
            Unresolvable::NeverAllocated,
            "dom-doesnotexist00000".to_string(),
        ),
    ];
    // An id this service DID mint, whose domain is gone: the delete that follows a delete
    // is the idempotent-cleanup path, and it has to terminate in "already gone". This
    // domain owns no web application, so its deletion completes inside the deleting call
    // and the record is purged by it.
    let purged = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-doomed" }))
            .await,
        "domainId",
    );
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{purged}"), None)
        .await;
    assert_eq!(status, 200, "the first DeleteDomain got {status} {deleted:?}");
    rows.push((
        "minted and since purged".to_string(),
        Unresolvable::AllocatedAndLost,
        purged,
    ));
    // A domain that is LIVE in another Region: an id this service minted, naming a domain
    // this Region's endpoint holds nothing under.
    let elsewhere = string_at(
        &emulator
            .create_domain_in("eu-west-1", &json!({ "name": "clinic-elsewhere" }))
            .await,
        "domainId",
    );
    rows.push((
        "live in another Region".to_string(),
        Unresolvable::AllocatedAndLost,
        elsewhere.clone(),
    ));

    for (case, kind, domain_id) in &rows {
        // Twice: the model declares the operation `@idempotent` and a refused delete
        // writes nothing, so the repeat is answered identically.
        for attempt in 1..=2 {
            let case = format!("{case} ({domain_id}), attempt {attempt}");
            let (status, body) = emulator
                .send("DELETE", &format!("/domain/{domain_id}"), None)
                .await;
            assert_delete_refusal(&case, *kind, domain_id, status, &body);
        }

        // The READ resolves the same domain, so it answers the same way: an id that
        // resolves nothing is the modelled not-found naming the requested id, whichever
        // operation resolved it.
        let (status, read) = emulator
            .send("GET", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?} ({domain_id}): GetDomain of an id that resolves nothing got \
             {status} {read:?}"
        );
        assert_eq!(
            detail_text(case, &read),
            format!("Domain not found: {domain_id}"),
            "case {case:?}: the not-found must name the requested id, got {read:?}"
        );
    }

    // The refusals deleted nothing: the domain this Region could not resolve is still
    // held by the Region that does.
    let (status, still_there) = emulator
        .send_in("eu-west-1", "GET", &format!("/domain/{elsewhere}"), None)
        .await;
    assert_eq!(
        status, 200,
        "the other Region's domain must be untouched by the refusals, got {status} \
         {still_there:?}"
    );
}

/// Remember that `domain_id` was allocated, the way a `CreateDomain` does
/// (`state::ComAmazonawsConnecthealthConnecthealthState::record_domain_allocation`, which
/// is the ledger that create writes and `DeleteDomain` reads).
///
/// Writing it directly is how one and the same id is put on BOTH sides of the split
/// below: a create mints the id it allocates, so no request can give allocation history
/// to an id chosen by the test.
fn remember_allocation(emulator: &Emulator, domain_id: &str) {
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    store.lock().record_domain_allocation(domain_id);
}

/// Assert that a refusal states its detail text under exactly ONE of the keys this
/// service's planes write it under: the control plane's own `message`, and no second
/// sentence beside it.
fn assert_one_detail_stated(case: &str, body: &Value) {
    let stated: Vec<&str> = crate::domain_subscription_lifecycle::DETAIL_MEMBERS
        .iter()
        .filter(|member| body.get(**member).and_then(Value::as_str).is_some())
        .copied()
        .collect();
    assert_eq!(
        stated.len(),
        1,
        "case {case:?}: this refusal states its sentence once, got it under {stated:?} in \
         {body:?}"
    );
}

#[tokio::test]
async fn delete_domain_answers_one_and_the_same_id_alike_whatever_its_allocation_history() {
    let emulator = Emulator::new("us-east-1");
    // The rows differ along everything the modelled `DomainId` generalises over — both
    // prefixes, both ends of the 20..25 length range, and digits-only, letters-only and
    // mixed bodies. Each one is driven TWICE: once with no allocation history anywhere,
    // then with that same id remembered as allocated and still resolving no domain. The
    // request is identical in both passes, and so is the answer: what it turns on is what
    // the Region HOLDS, never the id's history.
    let rows = [
        (
            "the shortest dom- id (20), letters only",
            unresolvable_domain_id("dom-", 'a', 20),
        ),
        (
            "a digits-only dom- id (24)",
            unresolvable_domain_id("dom-", '7', 24),
        ),
        (
            "the longest dom- id (25)",
            unresolvable_domain_id("dom-", 'z', 25),
        ),
        (
            "a legacy hai- id (21)",
            unresolvable_domain_id("hai-", '0', 21),
        ),
        (
            "a mixed dom- id (24)",
            "dom-1a2b3c4d5e6f7a8b9c0d".to_string(),
        ),
    ];

    for (row, domain_id) in &rows {
        let path = format!("/domain/{domain_id}");
        // An id with no allocation history anywhere: a resource this endpoint does not
        // have, which is the modelled not-found naming the requested id, stated once. The
        // repeat is answered identically: the model declares the
        // operation `@idempotent` and a refused delete writes nothing.
        for attempt in 1..=2 {
            let case = format!("{row} ({domain_id}), never allocated, attempt {attempt}");
            let (status, body) = emulator.send("DELETE", &path, None).await;
            assert_delete_refusal(&case, Unresolvable::NeverAllocated, domain_id, status, &body);
            assert_one_detail_stated(&case, &body);
        }

        // The SAME id, now remembered as allocated and still resolving no domain here: a
        // resource this account had and has lost is the modelled missing resource, worded
        // by this service and stated once.
        remember_allocation(&emulator, domain_id);
        for attempt in 1..=2 {
            let case = format!("{row} ({domain_id}), once allocated, attempt {attempt}");
            let (status, body) = emulator.send("DELETE", &path, None).await;
            assert_delete_refusal(&case, Unresolvable::AllocatedAndLost, domain_id, status, &body);
            assert_one_detail_stated(&case, &body);
        }

        // The READ resolves the same domain, so it answers the same way too: the modelled
        // 404 either way.
        let case = format!("{row} ({domain_id}), read back");
        let (status, read) = emulator.send("GET", &path, None).await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: GetDomain reports the modelled not-found whatever the id's \
             history, got {status} {read:?}"
        );
        assert_eq!(
            detail_text(&case, &read),
            format!("Domain not found: {domain_id}"),
            "case {case:?}: the not-found must name the requested id, got {read:?}"
        );
    }
}

#[tokio::test]
async fn an_id_this_account_was_allocated_keeps_the_modelled_not_found_past_its_purge() {
    let emulator = Emulator::new("eu-central-1");
    // Both ways a created domain reaches the end of its life: a deletion that finishes
    // inside the deleting call, and one that cannot — a domain owning a web application
    // passes through the non-terminal `DELETING` and is purged when the clock makes the
    // purge due. Either way the id WAS allocated to this account, so every later delete
    // of it is the modelled 404 and the idempotent-cleanup path can keep keying on it.
    for (row, request) in [
        (
            "a domain with no web application",
            json!({ "name": "clinic-plain" }),
        ),
        (
            "a domain that owns a web application",
            json!({
                "name": "clinic-web",
                "webAppSetupConfiguration": web_app_setup(),
            }),
        ),
    ] {
        let domain_id = string_at(&emulator.create_domain(&request).await, "domainId");
        let path = format!("/domain/{domain_id}");

        let (status, deleted) = emulator.send("DELETE", &path, None).await;
        assert_eq!(status, 200, "case {row:?}: DeleteDomain got {status} {deleted:?}");
        assert_eq!(
            string_at(&deleted, "domainId"),
            domain_id,
            "case {row:?}: the delete echoes the id it was addressed at, got {deleted:?}"
        );
        let reported = string_at(&deleted, "status");
        assert!(
            matches!(reported.as_str(), "DELETING" | "DELETED"),
            "case {row:?}: a deletion reports a deleting or deleted domain, got \
             {reported:?} in {deleted:?}"
        );

        // The purge is observed by LOOKING again until it has fallen due, never by
        // waiting a fixed interval.
        let deadline = std::time::Instant::now() + PURGE_DEADLINE;
        let read = loop {
            let (status, read) = emulator.send("GET", &path, None).await;
            if status == 404 {
                break read;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {row:?}: {domain_id} was still held after {PURGE_DEADLINE:?}, last \
                 read {status} with {read:?}"
            );
            ::tokio::task::yield_now().await;
        };
        assert_eq!(
            error_code(&read).as_str(),
            "ResourceNotFoundException",
            "case {row:?}: the purged domain reads back as the modelled not-found, got \
             {read:?}"
        );
        assert_eq!(
            detail_text(row, &read),
            format!("Domain not found: {domain_id}"),
            "case {row:?}: the not-found must name the requested id, got {read:?}"
        );

        for attempt in 1..=3 {
            let case = format!("{row} ({domain_id}), delete after the purge, attempt {attempt}");
            let (status, body) = emulator.send("DELETE", &path, None).await;
            assert_delete_refusal(&case, Unresolvable::AllocatedAndLost, &domain_id, status, &body);
            assert_ne!(
                error_code(&body).as_str(),
                "AuthorizerConfigurationException",
                "case {case:?}: an id this account WAS allocated is never refused as one \
                 with no allocation history, got {status} {body:?}"
            );
        }
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
    let never = unresolvable_domain_id("dom-", 'z', 25);

    let session = "7f8b1c2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";
    for domain_id in [&gone, &never] {
        for (operation, method, path) in [
            ("GetDomain", "GET", format!("/domain/{domain_id}")),
            ("DeleteDomain", "DELETE", format!("/domain/{domain_id}")),
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
            // A DELETE binds no body member, so only the operations that take one are
            // sent with it.
            let request = (method != "DELETE").then(|| json!({}));
            let (status, body) = emulator.send(method, &path, request.as_ref()).await;
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
                "GetDomain" | "DeleteDomain" | "ListSubscriptions" | "CreateSubscription"
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
        // The model's own record of whether the configuration carried the structured
        // medical-coding context, which a seeded session was given none of.
        medical_coding_context_provided: false,
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
