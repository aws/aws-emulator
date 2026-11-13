// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the capabilities of an OpenSearch UI application:
//! `RegisterCapability`, `GetCapability` and `DeregisterCapability`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the application a capability is registered for from
//! [`super::data_source_attachments`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a register persists the capability on the application it names and answers with the
//!   application, the name, the status and the configuration it carried, and the SAME
//!   record reads back through `GetCapability`;
//! * `capabilityName` names WHICH capability is registered and the service registers one,
//!   so a name outside the published length bounds or expression is refused at the
//!   boundary and a name inside them that is not that identifier is refused by the
//!   service — over a table of names differing in length, case, digits and hyphen
//!   placement — and neither persists anything, after which a read of the rejected name
//!   reports that the application holds no such capability;
//! * a capability is registered in a transient status and the clock moves it to active
//!   without any call asking for it;
//! * the name is unique per application, so registering it again registers no second
//!   capability and answers with the one record the application holds;
//! * a capability is scoped to the application it was registered for, so one
//!   application's capability is neither reported nor removed through another's;
//! * a deregistration reports the capability being removed, is idempotent, and the clock
//!   takes the record away — after which a read is the modelled not-found refusal and a
//!   register mints the capability afresh;
//! * every one of the three operations refuses an id naming no application, and the two
//!   that address a registration refuse a name the application holds none under;
//! * the set one application holds is bounded: one register per name and one name, so
//!   however many registers arrive the application holds a single record.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    CAPABILITY_STATUS_ACTIVE, CAPABILITY_STATUS_CREATING, CAPABILITY_STATUS_DELETING,
};

use super::data_source_attachments::application;
use super::domain_lifecycle::{
    error_code, poll_until, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// The ONE capability name this service registers: "for registering AI Assistant
/// capability, use ai-capability" (`RegisterCapabilityRequest$capabilityName`'s own
/// documentation). Every other value inside `capabilityName`'s published constraints is
/// refused by [`a_name_that_is_not_the_registrable_capability_is_refused`].
const CAPABILITY: &str = "ai-capability";

/// The only configuration branch `CapabilityBaseRequestConfig` declares — the AI settings
/// of an application's capability (`smithy_query(query="shape",shape="CapabilityBaseRequestConfig")`),
/// whose target `AIConfig` declares no member of its own.
fn ai_config() -> Value {
    json!({ "aiConfig": {} })
}

/// `RegisterCapability`:
/// `POST /2021-01-01/opensearch/application/{applicationId}/capability/register`.
async fn register(
    emulator: &Emulator,
    application_id: &str,
    capability_name: &str,
    config: Value,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application/{application_id}/capability/register"),
            Some(json!({
                "capabilityName": capability_name,
                "capabilityConfig": config,
            })),
        )
        .await
}

/// `GetCapability`:
/// `GET /2021-01-01/opensearch/application/{applicationId}/capability/{capabilityName}`.
async fn get_capability(emulator: &Emulator, application_id: &str, capability_name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "{PREFIX}/opensearch/application/{application_id}/capability/{capability_name}"
            ),
            None,
        )
        .await
}

/// `DeregisterCapability`:
/// `DELETE /2021-01-01/opensearch/application/{applicationId}/capability/deregister/{capabilityName}`.
async fn deregister(emulator: &Emulator, application_id: &str, capability_name: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!(
                "{PREFIX}/opensearch/application/{application_id}/capability/deregister/\
                 {capability_name}"
            ),
            None,
        )
        .await
}

/// The capability names this application holds a record for, read out of the store the
/// handlers write — the evidence a register persisted a capability rather than only
/// answering about one, and that a repeat added no second record.
fn held_capabilities(emulator: &Emulator, application_id: &str) -> Vec<String> {
    let store = emulator.store();
    let store = store.lock();
    store
        .applications
        .get(application_id)
        .map(|application| application.capabilities.keys().cloned().collect())
        .unwrap_or_default()
}

/// Assert a success and answer the body, naming the case on failure.
fn served(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body.clone()
}

/// Assert a refusal: the recorded missing-resource status, the error code and the message.
fn refused(answer: &Answer, case: &str, status: u16, code: &str, message: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        code,
        "case {case:?}: expected {code}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str().unwrap_or_default(),
        message,
        "case {case:?}: expected the refusal to state {message:?}, got {:?}",
        answer.body
    );
}

/// The names a register is held to, from `capabilityName`'s published constraints:
/// `@length(min: 3, max: 30)` and `@pattern("^[a-zA-Z0-9-]+$")`
/// (`smithy_query(query="operation",operation="RegisterCapability")`). Each row is inside
/// both and is still not the capability this service registers ([`CAPABILITY`]), and they
/// differ in length, case, digits and hyphen placement — so what refuses them is the value
/// rule rather than a property of one name.
fn unregistrable_names() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a name of its own", "not-ai-capability"),
        ("the published minimum length", "abc"),
        (
            "the published maximum length",
            "ai-capability-at-the-upper-max",
        ),
        ("the registrable name in another case", "AI-Capability"),
        ("the registrable name without its hyphen", "aicapability"),
        ("digits and hyphens throughout", "a-b-c-1-2-3"),
    ]
}

#[tokio::test]
async fn a_registered_capability_reads_back_through_its_own_get() {
    let emulator = Emulator::new();
    let id = application(&emulator, "capability-round-trip").await;
    let case = "the registrable capability";

    let answer = register(&emulator, &id, CAPABILITY, ai_config()).await;
    let body = served(&answer, case);
    assert_eq!(
        string_member(&body, "applicationId", case),
        id,
        "case {case:?}: the answer names the application the capability was registered \
         for, got {body:?}"
    );
    assert_eq!(
        string_member(&body, "capabilityName", case),
        CAPABILITY,
        "case {case:?}: the answer names the capability that was registered, got {body:?}"
    );
    assert_eq!(
        string_member(&body, "status", case),
        CAPABILITY_STATUS_CREATING,
        "case {case:?}: a capability is being enabled the instant the register returns, \
         got {body:?}"
    );
    assert_eq!(
        body["capabilityConfig"],
        ai_config(),
        "case {case:?}: the answer carries the configuration the register named, got \
         {body:?}"
    );

    // The SAME record, read back through the operation that reports one.
    let read = get_capability(&emulator, &id, CAPABILITY).await;
    let reported = served(&read, case);
    assert_eq!(
        string_member(&reported, "applicationId", case),
        id,
        "case {case:?}: the read names the application, got {reported:?}"
    );
    assert_eq!(
        string_member(&reported, "capabilityName", case),
        CAPABILITY,
        "case {case:?}: the read names the capability, got {reported:?}"
    );
    assert_eq!(
        reported["capabilityConfig"],
        ai_config(),
        "case {case:?}: the read reports the stored configuration, got {reported:?}"
    );
    assert!(
        reported["failures"].is_null(),
        "case {case:?}: a capability nothing failed for carries no failures, got \
         {reported:?}"
    );

    // And it was persisted on the application, under the name it was registered with.
    assert_eq!(
        held_capabilities(&emulator, &id),
        vec![CAPABILITY.to_string()],
        "case {case:?}: the application holds the record the register made"
    );
}

#[tokio::test]
async fn a_name_that_is_not_the_registrable_capability_is_refused() {
    let emulator = Emulator::new();
    let id = application(&emulator, "capability-value-rule").await;

    for (case, name) in unregistrable_names() {
        let answer = register(&emulator, &id, name, ai_config()).await;
        let expected = format!(
            "Invalid capability name for AI Capability: {name}. Only {CAPABILITY} is allowed"
        );
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {name:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException for {name:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str().unwrap_or_default(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.body
        );

        // Nothing was registered for it, so a read of that name reports that the
        // application holds no such capability.
        assert!(
            held_capabilities(&emulator, &id).is_empty(),
            "case {case:?}: a refused register persists nothing, got {:?}",
            held_capabilities(&emulator, &id)
        );
        refused(
            &get_capability(&emulator, &id, name).await,
            &format!("a read of the rejected {name:?}"),
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            &format!("Capability {name} not found for application {id}"),
        );
    }

    // ... while the capability this service DOES register is served, so what refuses the
    // rows above is the name and nothing else.
    served(
        &register(&emulator, &id, CAPABILITY, ai_config()).await,
        "the registrable capability beside them",
    );
}

#[tokio::test]
async fn a_name_outside_its_published_constraints_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let id = application(&emulator, "capability-refusals").await;

    // Each row breaks one of `capabilityName`'s published constraints: one character
    // under the minimum, one past the maximum, and three values outside the expression.
    let rows: Vec<(&str, String)> = vec![
        ("one under the published minimum", "ab".to_string()),
        ("one past the published maximum", "a".repeat(31)),
        ("an underscore the expression refuses", "ai_capability".to_string()),
        ("a dot the expression refuses", "ai.capability".to_string()),
        ("a space the expression refuses", "ai capability".to_string()),
    ];
    for (case, name) in rows {
        let answer = register(&emulator, &id, &name, ai_config()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert!(
            held_capabilities(&emulator, &id).is_empty(),
            "case {case:?}: a refused register persists nothing, got {:?}",
            held_capabilities(&emulator, &id)
        );
    }
}

#[tokio::test]
async fn a_capability_settles_from_creating_to_active_on_the_clock() {
    let emulator = Emulator::new();
    let id = application(&emulator, "capability-settling").await;
    let name = "ai-capability";

    let answer = register(&emulator, &id, name, ai_config()).await;
    let body = served(&answer, "a fresh register");
    assert_eq!(
        string_member(&body, "status", "a fresh register"),
        CAPABILITY_STATUS_CREATING,
        "case \"a fresh register\": the span is never zero, so the caller observes the \
         transient status, got {body:?}"
    );
    let read = get_capability(&emulator, &id, name).await;
    assert_eq!(
        string_member(&served(&read, "a read while it is being enabled"), "status", "creating"),
        CAPABILITY_STATUS_CREATING,
        "case \"a read while it is being enabled\": the read reports the transient status, \
         got {:?}",
        read.body
    );

    // Nothing asks for the move: the span the clock owed it falls due on its own, and the
    // read that comes after it reports the status enabling left the capability in. The
    // case waits for THAT condition rather than for a stretch of time.
    poll_until("the settled capability", || async {
        let read = get_capability(&emulator, &id, name).await;
        read.body["status"].as_str() == Some(CAPABILITY_STATUS_ACTIVE)
    })
    .await;
    let settled = get_capability(&emulator, &id, name).await;
    assert_eq!(
        string_member(&served(&settled, "the settled capability"), "status", "active"),
        CAPABILITY_STATUS_ACTIVE,
        "case \"the settled capability\": the move the clock owed has landed, got {:?}",
        settled.body
    );

    // A capability registered after that wait is still owed its own span: the move is
    // owed per record rather than once per application, so a second application's
    // registration is being enabled while this one is already active.
    let other = application(&emulator, "capability-settling-other").await;
    let second = register(&emulator, &other, name, ai_config()).await;
    assert_eq!(
        string_member(&served(&second, "a later register"), "status", "a later register"),
        CAPABILITY_STATUS_CREATING,
        "case \"a later register\": a fresh capability is being enabled, got {:?}",
        second.body
    );
}

#[tokio::test]
async fn registering_a_held_capability_again_registers_no_second_capability() {
    let emulator = Emulator::new();
    let id = application(&emulator, "capability-idempotent").await;
    let case = "a repeat of the registration";

    let first = served(&register(&emulator, &id, CAPABILITY, ai_config()).await, case);
    let again = served(&register(&emulator, &id, CAPABILITY, ai_config()).await, case);
    for (member, expected) in [
        ("applicationId", id.clone()),
        ("capabilityName", CAPABILITY.to_string()),
    ] {
        assert_eq!(
            string_member(&again, member, case),
            expected,
            "case {case:?}: the repeat answers about the capability the first register \
             made, got {again:?}"
        );
    }
    assert_eq!(
        again["capabilityConfig"], first["capabilityConfig"],
        "case {case:?}: the repeat reports the stored configuration, got {again:?}"
    );
    assert_eq!(
        held_capabilities(&emulator, &id),
        vec![CAPABILITY.to_string()],
        "case {case:?}: the name is unique per application, got {:?}",
        held_capabilities(&emulator, &id)
    );

    // The same holds once the capability is active: a repeat answers with the record as
    // it stands rather than putting it back into its transient status.
    poll_until("the capability becoming active", || async {
        let read = get_capability(&emulator, &id, CAPABILITY).await;
        read.body["status"].as_str() == Some(CAPABILITY_STATUS_ACTIVE)
    })
    .await;
    let name = CAPABILITY;
    let repeat = served(
        &register(&emulator, &id, name, ai_config()).await,
        "a repeat of an active capability",
    );
    assert_eq!(
        string_member(&repeat, "status", "a repeat of an active capability"),
        CAPABILITY_STATUS_ACTIVE,
        "case \"a repeat of an active capability\": the answer reports where the capability \
         stands, got {repeat:?}"
    );
    assert_eq!(
        held_capabilities(&emulator, &id),
        vec![CAPABILITY.to_string()],
        "case \"a repeat of an active capability\": no record was added, got {:?}",
        held_capabilities(&emulator, &id)
    );
}

#[tokio::test]
async fn a_capability_is_scoped_to_the_application_it_was_registered_for() {
    let emulator = Emulator::new();
    let one = application(&emulator, "capability-scope-one").await;
    let other = application(&emulator, "capability-scope-two").await;
    let name = "ai-capability";

    served(&register(&emulator, &one, name, ai_config()).await, "the first application");
    for (case, application_id) in [("the first application", &one), ("the second", &other)] {
        let held = held_capabilities(&emulator, application_id);
        let expected: Vec<String> = if case == "the first application" {
            vec![name.to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            held, expected,
            "case {case:?}: a capability is held by the application it was registered for"
        );
    }

    // The other application holds no such capability, so a read and a deregistration
    // addressed to it are refused rather than reaching the first application's record.
    let expected = format!("Capability {name} not found for application {other}");
    for (case, answer) in [
        ("a read scoped to another application", get_capability(&emulator, &other, name).await),
        (
            "a deregistration scoped to another application",
            deregister(&emulator, &other, name).await,
        ),
    ] {
        refused(
            &answer,
            case,
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            &expected,
        );
    }
    assert_eq!(
        held_capabilities(&emulator, &one),
        vec![name.to_string()],
        "case \"the first application\": a call about another application left its record \
         where it was"
    );
}

#[tokio::test]
async fn a_deregistered_capability_is_removed_on_the_clock() {
    let emulator = Emulator::new();
    let id = application(&emulator, "capability-lifecycle").await;
    let name = "ai-capability";

    served(&register(&emulator, &id, name, ai_config()).await, "the registration");
    poll_until("the registration becoming active", || async {
        let read = get_capability(&emulator, &id, name).await;
        read.body["status"].as_str() == Some(CAPABILITY_STATUS_ACTIVE)
    })
    .await;

    // The call reports the capability being removed, and so does a read taken before the
    // removal falls due: the record rests in that status rather than vanishing at once.
    let gone = served(&deregister(&emulator, &id, name).await, "the deregistration");
    assert_eq!(
        string_member(&gone, "status", "the deregistration"),
        CAPABILITY_STATUS_DELETING,
        "case \"the deregistration\": the capability is being removed, got {gone:?}"
    );
    let read = get_capability(&emulator, &id, name).await;
    assert_eq!(
        string_member(&served(&read, "a read while it is being removed"), "status", "deleting"),
        CAPABILITY_STATUS_DELETING,
        "case \"a read while it is being removed\": the read reports the same status, got {:?}",
        read.body
    );
    // Idempotent: repeating the call reports that same status rather than refusing or
    // starting a second removal.
    let repeat = served(&deregister(&emulator, &id, name).await, "a repeated deregistration");
    assert_eq!(
        string_member(&repeat, "status", "a repeated deregistration"),
        CAPABILITY_STATUS_DELETING,
        "case \"a repeated deregistration\": the capability is still being removed, got \
         {repeat:?}"
    );

    // The clock takes the record away, after which the application holds no such
    // capability at all: the removal is materialised by whichever later call reads the
    // record, so the read that no longer resolves it is what takes it out of the store.
    poll_until("the removal landing", || async {
        get_capability(&emulator, &id, name).await.status != 200
    })
    .await;
    refused(
        &get_capability(&emulator, &id, name).await,
        "a read of a removed capability",
        RECORDED_NOT_FOUND_STATUS,
        "ResourceNotFoundException",
        &format!("Capability {name} not found for application {id}"),
    );
    assert!(
        held_capabilities(&emulator, &id).is_empty(),
        "case \"the removed capability\": the record is gone, got {:?}",
        held_capabilities(&emulator, &id)
    );

    // A register after the removal is a fresh registration rather than the old record
    // reported back.
    let again = served(&register(&emulator, &id, name, ai_config()).await, "a later register");
    assert_eq!(
        string_member(&again, "status", "a later register"),
        CAPABILITY_STATUS_CREATING,
        "case \"a later register\": the capability is being enabled again, got {again:?}"
    );
    assert_eq!(
        held_capabilities(&emulator, &id),
        vec![name.to_string()],
        "case \"a later register\": the application holds the capability again"
    );
}

#[tokio::test]
async fn every_capability_operation_refuses_an_id_that_names_no_application() {
    let emulator = Emulator::new();
    let held = application(&emulator, "capability-misses").await;
    let name = "ai-capability";
    served(&register(&emulator, &held, name, ai_config()).await, "the held capability");

    // Ids this account holds no application under, differing in length and in the
    // characters the service admits, so the refusal is not a property of one value.
    for absent in ["noapplication0", "abc", "a1b2c3d4e5f6g7h8i9j0"] {
        let expected = format!("Application {absent} not found");
        for (case, answer) in [
            ("register", register(&emulator, absent, name, ai_config()).await),
            ("get", get_capability(&emulator, absent, name).await),
            ("deregister", deregister(&emulator, absent, name).await),
        ] {
            refused(
                &answer,
                &format!("{case} on the absent application {absent}"),
                RECORDED_NOT_FOUND_STATUS,
                "ResourceNotFoundException",
                &expected,
            );
        }
    }

    // A name the application holds no registration under is refused by the two operations
    // that address one.
    for unregistered in ["other-capability", "xyz", "ai-capability-never-registered"] {
        let expected = format!("Capability {unregistered} not found for application {held}");
        for (case, answer) in [
            ("get", get_capability(&emulator, &held, unregistered).await),
            ("deregister", deregister(&emulator, &held, unregistered).await),
        ] {
            refused(
                &answer,
                &format!("{case} of the unregistered capability {unregistered}"),
                RECORDED_NOT_FOUND_STATUS,
                "ResourceNotFoundException",
                &expected,
            );
        }
    }
}

#[tokio::test]
async fn the_capability_set_of_one_application_is_bounded() {
    let emulator = Emulator::new();
    let id = application(&emulator, "capability-quota").await;

    // The name IS the key the application holds the record under, and one name is
    // registrable, so however many registers arrive — with that name, or with names the
    // value rule refuses — the application holds exactly one record and the set cannot be
    // grown by a caller.
    for (case, name) in unregistrable_names() {
        let answer = register(&emulator, &id, name, ai_config()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a register of {name:?} adds no record, got {} with {:?}",
            answer.status, answer.body
        );
    }
    for round in 0..3 {
        let answer = register(&emulator, &id, CAPABILITY, ai_config()).await;
        assert_eq!(
            answer.status, 200,
            "case \"register round {round}\": the registrable capability is served, got {} with \
             {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            held_capabilities(&emulator, &id),
            vec![CAPABILITY.to_string()],
            "case \"register round {round}\": the application holds one record, got {:?}",
            held_capabilities(&emulator, &id)
        );
    }
}
