// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DRS Recovery Plan slice: what a create persists, and
//! what a read of the plan it minted reports.
//!
//! Every test drives the service the way the router does — one buffered
//! `http::Request` per call through `MultiServiceRouter::dispatch` — so what is
//! asserted is the wire behaviour (status, error code, JSON body), not a handler's
//! Rust return value. The harness (the router, `call_in`, `Answer`) is the one
//! `replication_configuration_template_lifecycle` builds; the engine it hands back is
//! read directly in a few places, so that "a refused create stored nothing" is
//! asserted against the store itself rather than against another operation's answer.
//! What the enumeration of the account's plans reports is asserted in
//! `recovery_plan_pagination`.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. A create mints a `recoveryPlanID` and the plan's ARN — carrying the Region the
//!    request was signed for and the emulated account — persists the name, description
//!    and tags the request supplied, stamps `createdAt` and `updatedAt` with the
//!    instant of the call, and reports the STORED plan; `GetRecoveryPlan` on the
//!    reported ARN reads back the same plan, repeatedly and unchanged.
//! 2. A plan reports a status that is one of the model's own `RecoveryPlanStatus`
//!    values, and a plan holding no steps — which is every plan a create mints — is
//!    reported as executable.
//! 3. `clientToken` is modelled `@idempotencyToken`: a create repeating a token reports
//!    the plan that token already minted instead of minting a second one, while creates
//!    carrying different tokens, or none, each mint their own.
//! 4. An ARN this account holds no plan for — one never minted, or a stored plan's id
//!    under another Region, another account or another resource type — is 404
//!    `ResourceNotFoundException` naming the resource type, the ARN the request asked
//!    for and the plan id that ARN addresses, and changes nothing.
//! 5. A create outside a modelled bound on its members is refused and stores nothing;
//!    the same create at the bound is served.
//! 6. The store is bounded: it accepts the service's quota of plans and refuses the
//!    next create with the modelled `ServiceQuotaExceededException` (402), storing
//!    nothing and leaving every stored plan readable.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::ops::types::RECOVERY_PLAN_STATUS_VALUES;
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::state::ComAmazonawsDrsElasticdisasterrecoveryserviceState as DrsState;
use smithy_server_runtime::MultiServiceRouter;

use crate::replication_configuration_template_lifecycle::{call_in, drs_with_engine, Answer};

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`),
/// which every ARN it reports carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region most rows sign for. Rows that pin the ARN's Region segment sign for
/// others.
const REGION: &str = "eu-west-1";

/// How many Recovery Plans the store holds, read straight off the engine (see the
/// module docs): what a refused create must leave untouched.
pub(crate) fn stored_plans(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine.state::<DrsState>().lock().recovery_plans.len()
}

/// The `recoveryPlan` structure of a create or a read.
pub(crate) fn plan_of(answer: &Answer) -> &Value {
    answer
        .member("recoveryPlan")
        .unwrap_or_else(|| panic!("expected a recoveryPlan member, got {answer:?}"))
}

/// The ARN a reported plan carries.
pub(crate) fn arn_of(answer: &Answer) -> String {
    plan_of(answer)
        .get("recoveryPlanArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a recoveryPlanArn, got {answer:?}"))
        .to_string()
}

/// A string member of an answer's body, for an assertion that names the case.
pub(crate) fn text(answer: &Answer, name: &str) -> String {
    answer
        .member(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `length` characters of what the model's `RecoveryPlanName` pattern
/// (`^[a-zA-Z0-9][a-zA-Z0-9 _-]*$`) allows in every position.
pub(crate) fn name_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// The `recoveryPlanID` an ARN addresses: what follows the resource type in a Recovery
/// Plan's ARN (`recovery-plan/<recoveryPlanID>`, the `RecoveryPlanResource` ARN
/// template).
fn id_of(plan_arn: &str) -> &str {
    plan_arn
        .split_once(":recovery-plan/")
        .unwrap_or_else(|| panic!("expected a recovery-plan ARN, got {plan_arn:?}"))
        .1
}

/// What the service's not-found message names a plan an ARN addresses by: the plan id
/// the ARN carries, or the value itself when the ARN addresses no plan at all.
pub(crate) fn plan_subject_of(plan_arn: &str) -> &str {
    plan_arn
        .split_once(":recovery-plan/")
        .map_or(plan_arn, |(_, id)| id)
}

/// The whole message a plan operation answers an ARN it holds no plan for with.
pub(crate) fn not_found_message(plan_arn: &str) -> String {
    format!("Recovery plan '{}' not found.", plan_subject_of(plan_arn))
}

/// Whether `value` is a `RecoveryPlanID`: the model gives it length 22..64 and the
/// pattern `^plan-[0-9a-zA-Z]{17,59}$`, checked without pulling in a regex engine.
fn is_plan_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("plan-") else {
        return false;
    };
    (17..=59).contains(&suffix.len()) && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Whether `value` is an `ISO8601DatetimeString` as the model constrains `createdAt`
/// and `updatedAt`: an RFC-3339 date-time in UTC.
pub(crate) fn is_iso8601(value: &str) -> bool {
    value.ends_with('Z')
        && ::aws_smithy_types::DateTime::from_str(
            value,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .is_ok()
}

/// Create a plan, failing the test on anything but the modelled 201.
pub(crate) async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> Answer {
    let answer = call_in(router, region, "CreateRecoveryPlan", body).await;
    assert_eq!(
        answer.status, 201,
        "create {body}: expected 201, got {answer:?}"
    );
    answer
}

/// Read a plan back by its ARN.
pub(crate) async fn get(router: &MultiServiceRouter, region: &str, plan_arn: &str) -> Answer {
    call_in(
        router,
        region,
        "GetRecoveryPlan",
        &json!({"recoveryPlanArn": plan_arn}),
    )
    .await
}

// ---------------------------------------------------------------------------
// Rules 1 and 2 — a create persists the plan, and a read reports it unchanged
// ---------------------------------------------------------------------------

/// Every create reports a plan built from the request, the store and the clock, the
/// store keeps it, and `GetRecoveryPlan` reads the SAME plan back twice. The rows
/// differ along what the rule generalises over: the Region signed for, the form and
/// length of the name, whether a description is absent, empty or at the modelled
/// maximum, and whether tags are absent, empty or several.
#[tokio::test]
async fn a_created_plan_is_stored_and_read_back_unchanged() {
    // The bounds the longest rows sit on: `RecoveryPlanName` is length 1..256 and
    // `RecoveryPlanDescription` length 0..1024.
    let longest_name = name_of_length(256);
    let longest_description = "d".repeat(1024);
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a one-character name, nothing else",
            REGION,
            json!({"name": "A"}),
        ),
        (
            "every separator the name pattern allows, in another Region",
            "us-east-1",
            json!({"name": "dr plan_1-a"}),
        ),
        (
            "a name starting with a digit, in a third Region",
            "ap-southeast-2",
            json!({"name": "0rder of recovery"}),
        ),
        (
            "a description and several tags",
            REGION,
            json!({
                "name": "Nightly DR",
                "description": "Recovers the order-processing tier",
                "tags": {"env": "prod", "team": "dr", "tier": "1"},
            }),
        ),
        (
            "an empty description and an empty tag set",
            REGION,
            json!({"name": "EmptyBits", "description": "", "tags": {}}),
        ),
        (
            "the longest name and description the model allows",
            REGION,
            json!({"name": longest_name, "description": longest_description}),
        ),
    ];

    let (router, engine) = drs_with_engine();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    for (case, region, body) in &cases {
        let answer = create(&router, region, body).await;
        let plan = plan_of(&answer).clone();

        // The id the service minted, and the ARN built from it: the Region signed
        // for, the emulated account, and the resource template of a Recovery Plan.
        let arn = arn_of(&answer);
        let id = id_of(&arn);
        assert!(
            is_plan_id(id),
            "case {case}: expected a RecoveryPlanID, got {id:?} in {arn:?}"
        );
        assert_eq!(
            arn,
            format!("arn:aws:drs:{region}:{ACCOUNT}:recovery-plan/{id}"),
            "case {case}: the ARN must carry the signed Region and this account"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: every create mints its own plan, got {arn:?} twice"
        );

        // Every other member of the reported plan comes from the request, from the
        // plan's own state, or from the clock.
        assert_eq!(
            plan.get("name"),
            body.get("name"),
            "case {case}: the name must be the one created, got {plan}"
        );
        assert_eq!(
            plan.get("description"),
            body.get("description"),
            "case {case}: the description must be the one created, got {plan}"
        );
        assert_eq!(
            plan.get("tags"),
            Some(&body.get("tags").cloned().unwrap_or_else(|| json!({}))),
            "case {case}: a plan carries the tag set it was created with, got {plan}"
        );

        // Rule 2: the model's `RecoveryPlanStatus` is exactly [ACTIVE, INVALID], and
        // `INVALID` is documented for a plan whose steps cannot be executed — a plan
        // holding no step at all is not composed yet, so a created plan is ACTIVE.
        let status = plan
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            status, "ACTIVE",
            "case {case}: a created plan is reported as executable, got {plan}"
        );
        assert!(
            RECOVERY_PLAN_STATUS_VALUES.contains(&status),
            "case {case}: {status:?} is not one of the modelled statuses \
             {RECOVERY_PLAN_STATUS_VALUES:?}"
        );

        let created_at = plan
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let updated_at = plan
            .get("updatedAt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            is_iso8601(created_at) && is_iso8601(updated_at),
            "case {case}: both timestamps must be ISO-8601 in UTC, got {plan}"
        );
        assert_eq!(
            created_at, updated_at,
            "case {case}: a plan nothing has changed was last updated when it was \
             created, got {plan}"
        );

        // The read path reports the STORED plan, and reading it twice changes nothing
        // a later caller sees.
        let read = get(&router, region, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the plan to read back, got {read:?}"
        );
        assert_eq!(
            plan_of(&read),
            &plan,
            "case {case}: the read must report the created plan, got {read:?}"
        );
        let again = get(&router, region, &arn).await;
        assert_eq!(
            plan_of(&again),
            &plan,
            "case {case}: a second read must report the same plan, got {again:?}"
        );
    }

    assert_eq!(
        stored_plans(&engine),
        cases.len(),
        "the store must hold exactly the plans these creates minted"
    );
}

// ---------------------------------------------------------------------------
// Rule 3 — the idempotency token identifies the create
// ---------------------------------------------------------------------------

/// A create carrying a `clientToken` an earlier create carried reports that plan again
/// and mints nothing; one carrying a different token, or none, mints its own plan. The
/// rows cross repeated tokens with the token forms the model allows
/// (`ClientIdempotencyToken`, length 0..64), including the empty token, which names no
/// request.
#[tokio::test]
async fn a_repeated_client_token_reports_the_plan_it_already_minted() {
    let longest_token = "t".repeat(64);
    let tokens: Vec<(&str, Value)> = vec![
        ("a plain token", json!("dr-token-1")),
        (
            "a token of the greatest modelled length",
            json!(longest_token),
        ),
    ];

    let (router, engine) = drs_with_engine();
    let mut distinct: BTreeSet<String> = BTreeSet::new();
    for (case, token) in &tokens {
        let body = json!({"name": "Idempotent", "clientToken": token});
        let first = create(&router, REGION, &body).await;
        let plan = plan_of(&first).clone();
        assert!(
            distinct.insert(arn_of(&first)),
            "case {case}: expected a plan of its own, got {plan} again"
        );

        // Twice more with the same token: the same plan, member for member — so the
        // repeat reports what the token minted rather than a second plan.
        for attempt in 1..=2 {
            let repeat = create(&router, REGION, &body).await;
            assert_eq!(
                plan_of(&repeat),
                &plan,
                "case {case}, repeat {attempt}: a repeated token must report the plan \
                 it minted, got {repeat:?}"
            );
        }
    }

    // Creates that name NO token are separate plans, and so is a create under another
    // token: the token is what makes a repeat a repeat. An empty token is within
    // `ClientIdempotencyToken`'s length and names no request, so two creates carrying
    // it are two plans as well.
    let separate: Vec<(&str, Value)> = vec![
        ("no token, first", json!({"name": "Untokened"})),
        ("no token, second", json!({"name": "Untokened"})),
        (
            "an empty token, first",
            json!({"name": "Empty", "clientToken": ""}),
        ),
        (
            "an empty token, second",
            json!({"name": "Empty", "clientToken": ""}),
        ),
        (
            "another token",
            json!({"name": "Other", "clientToken": "dr-token-2"}),
        ),
    ];
    for (case, body) in &separate {
        let answer = create(&router, REGION, body).await;
        let arn = arn_of(&answer);
        assert!(
            distinct.insert(arn.clone()),
            "case {case}: expected a plan of its own, got {arn:?} again"
        );
    }

    assert_eq!(
        stored_plans(&engine),
        distinct.len(),
        "a repeated token must mint nothing: the store holds one plan per distinct \
         create, {distinct:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4 — an ARN no plan of this account answers to
// ---------------------------------------------------------------------------

/// `GetRecoveryPlan` on an ARN this store holds no plan for answers 404
/// `ResourceNotFoundException`, reporting the service's name for the resource type, the
/// ARN the request named verbatim and the plan id that ARN addresses. The rows are ARNs
/// absent for different reasons: ids nothing minted, and a STORED plan's id addressed
/// under another Region, another account or another resource type.
#[tokio::test]
async fn an_arn_no_plan_answers_to_is_not_found() {
    let (router, engine) = drs_with_engine();
    let created = create(&router, REGION, &json!({"name": "Present"})).await;
    let present = arn_of(&created);
    let id = id_of(&present).to_string();

    // Every ARN below satisfies the modelled `StrictDRSARN`, so each reaches the
    // handler rather than the boundary validator.
    let absent: Vec<(&str, String)> = vec![
        (
            "an id nothing minted",
            format!("arn:aws:drs:{REGION}:{ACCOUNT}:recovery-plan/plan-00000000000000000"),
        ),
        (
            "an id nothing minted, in mixed case",
            format!("arn:aws:drs:{REGION}:{ACCOUNT}:recovery-plan/plan-aBcDeFgHiJkLmNoP0"),
        ),
        (
            "an id nothing minted, at the longest modelled length",
            format!(
                "arn:aws:drs:{REGION}:{ACCOUNT}:recovery-plan/plan-{}",
                "z".repeat(59)
            ),
        ),
        (
            "a stored plan's id under another Region",
            format!("arn:aws:drs:eu-central-1:{ACCOUNT}:recovery-plan/{id}"),
        ),
        (
            "a stored plan's id under another account",
            format!("arn:aws:drs:{REGION}:123456789012:recovery-plan/{id}"),
        ),
        (
            "another resource type of this service",
            format!(
                "arn:aws:drs:{REGION}:{ACCOUNT}:replication-configuration-template/\
                 rct-00000000000000000"
            ),
        ),
    ];

    for (case, arn) in &absent {
        let answer = get(&router, REGION, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404 for {arn:?}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            text(&answer, "resourceType"),
            "Recovery Plan",
            "case {case}: wrong resourceType in {answer:?}"
        );
        assert_eq!(
            text(&answer, "resourceId"),
            *arn,
            "case {case}: resourceId must be the ARN the request named, got {answer:?}"
        );
        assert_eq!(
            text(&answer, "message"),
            not_found_message(arn),
            "case {case}: wrong message in {answer:?}"
        );
    }

    // None of those refusals touched anything: the plan that WAS minted still reads
    // back, and it is still the only one.
    let read = get(&router, REGION, &present).await;
    assert_eq!(
        read.status, 200,
        "the stored plan must survive every refusal, got {read:?}"
    );
    assert_eq!(
        stored_plans(&engine),
        1,
        "a read must store nothing, whether or not it found a plan"
    );
}

// ---------------------------------------------------------------------------
// Rule 5 — a create outside a modelled bound stores nothing
// ---------------------------------------------------------------------------

/// A create whose members fall outside the bounds the model gives them is refused with
/// HTTP 400 and mints nothing; the same create at each bound is served. The rows walk
/// every published bound of `CreateRecoveryPlan`'s members and one past it: `name`
/// length 1..256 and pattern `^[a-zA-Z0-9][a-zA-Z0-9 _-]*$`, `description` length
/// 0..1024, `clientToken` length 0..64.
#[tokio::test]
async fn a_create_outside_a_modelled_bound_is_refused_and_stores_nothing() {
    let served: Vec<(&str, Value)> = vec![
        ("the shortest name", json!({"name": name_of_length(1)})),
        ("the longest name", json!({"name": name_of_length(256)})),
        (
            "the longest description",
            json!({"name": "Bounded", "description": "d".repeat(1024)}),
        ),
        (
            "the longest client token",
            json!({"name": "Bounded", "clientToken": "t".repeat(64)}),
        ),
    ];
    let refused: Vec<(&str, Value)> = vec![
        ("no name at all", json!({})),
        ("an empty name", json!({"name": ""})),
        (
            "a name one character too long",
            json!({"name": name_of_length(257)}),
        ),
        (
            "a name whose first character the pattern does not allow",
            json!({"name": "-plan"}),
        ),
        (
            "a name carrying a character the pattern does not allow",
            json!({"name": "plan!"}),
        ),
        (
            "a description one character too long",
            json!({"name": "TooMuchProse", "description": "d".repeat(1025)}),
        ),
        (
            "a client token one character too long",
            json!({"name": "TooLongToken", "clientToken": "t".repeat(65)}),
        ),
    ];

    let (router, engine) = drs_with_engine();
    for (case, body) in &served {
        let answer = call_in(&router, REGION, "CreateRecoveryPlan", body).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: a create at the bound is served, got {answer:?}"
        );
    }
    let stored = stored_plans(&engine);
    assert_eq!(
        stored,
        served.len(),
        "each create at a bound mints one plan, got {stored}"
    );

    for (case, body) in &refused {
        let answer = call_in(&router, REGION, "CreateRecoveryPlan", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 for {body}, got {answer:?}"
        );
        assert_eq!(
            stored_plans(&engine),
            stored,
            "case {case}: a refused create must store nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6 — the store is bounded
// ---------------------------------------------------------------------------

/// The store accepts the service's quota of plans and refuses the next create with the
/// modelled `ServiceQuotaExceededException` (402) — the model gives
/// `CreateRecoveryPlan` that error, and no bound of the emulator's is reported as any
/// other status. The refusal stores nothing and leaves the plans already minted
/// readable.
#[tokio::test]
async fn the_plan_store_is_bounded_by_the_services_quota() {
    use service_com_amazonaws_drs_elasticdisasterrecoveryservice::state::recovery_plans::{
        QUOTA, RESOURCE_TYPE,
    };

    let (router, engine) = drs_with_engine();
    let mut first_plan = String::new();
    for index in 0..QUOTA {
        // Each name differs, so no row is served by the idempotency path: the names
        // carry a digit and a space, both of which `RecoveryPlanName` allows.
        let answer = create(&router, REGION, &json!({"name": format!("Plan {index}")})).await;
        if index == 0 {
            first_plan = arn_of(&answer);
        }
    }
    assert_eq!(
        stored_plans(&engine),
        QUOTA,
        "the store must hold the quota of plans before the bound is tested"
    );

    // One past the bound: the modelled quota error, not a 500 and not an endlessly
    // growing store.
    let refused = call_in(
        &router,
        REGION,
        "CreateRecoveryPlan",
        &json!({"name": "One too many"}),
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "case create #{QUOTA} (at the quota): expected 402, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case create #{QUOTA} (at the quota): {refused:?}"
    );
    assert_eq!(
        text(&refused, "resourceType"),
        RESOURCE_TYPE,
        "case create #{QUOTA} (at the quota): wrong resourceType in {refused:?}"
    );
    assert_eq!(
        text(&refused, "serviceCode"),
        "drs",
        "case create #{QUOTA} (at the quota): wrong serviceCode in {refused:?}"
    );
    assert_eq!(
        stored_plans(&engine),
        QUOTA,
        "a create refused at the quota must store nothing"
    );

    // And the refusal left the store readable: the first plan minted still reads back.
    let read = get(&router, REGION, &first_plan).await;
    assert_eq!(
        read.status, 200,
        "the plans already minted must survive the refusal, got {read:?}"
    );
}
