// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for changing and removing a DRS Recovery Plan: what
//! `UpdateRecoveryPlan` writes, what `DeleteRecoveryPlan` takes away, and what a read
//! of the plan reports afterwards.
//!
//! Every test drives the service the way the router does — one buffered
//! `http::Request` per call through `MultiServiceRouter::dispatch` — so what is
//! asserted is the wire behaviour (status, error code, JSON body), not a handler's
//! Rust return value. The harness (`call_in`, `Answer`, the plan helpers) is the one
//! `replication_configuration_template_lifecycle` and `recovery_plan_lifecycle` build;
//! the engine is read directly only to COUNT stored plans, which no operation of this
//! service reports.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. An update writes the members the request NAMED to the stored plan, leaves the
//!    ones it did not name holding the values they had, restamps `updatedAt`, never
//!    moves `createdAt`, `recoveryPlanArn` or the plan's tags, and reports the stored
//!    plan — so the following read reports what the update answered, member for member.
//! 2. A delete takes the plan out of the store and echoes its ARN; from then on the
//!    plan is not found, by the same refusal as an ARN nothing ever minted, while every
//!    other plan is still readable.
//! 3. An update or a delete addressing an ARN this account holds no plan for is 404
//!    `ResourceNotFoundException`, naming the resource type, the ARN the request named
//!    and the plan id that ARN addresses, and changes nothing.
//! 4. A plan's status is one of the model's own `RecoveryPlanStatus` values and stays
//!    the executable one across create, update and read: it is not recomputed from
//!    steps the plan does not hold.

use serde_json::{json, Value};
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::ops::types::RECOVERY_PLAN_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_plan_lifecycle::{
    arn_of, create, get, is_iso8601, not_found_message, plan_of, stored_plans, text,
};
use crate::replication_configuration_template_lifecycle::{call_in, drs_with_engine, Answer};

/// The Region most rows sign for. A plan's ARN carries the Region the caller signed
/// for, so rows that pin that segment sign for others.
const REGION: &str = "eu-west-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`),
/// which every ARN it reports carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// `POST /UpdateRecoveryPlan` with `changes` merged onto the plan's ARN.
pub(crate) async fn update(
    router: &MultiServiceRouter,
    region: &str,
    plan_arn: &str,
    changes: &Value,
) -> Answer {
    let mut body = json!({"recoveryPlanArn": plan_arn});
    if let (Some(target), Some(changes)) = (body.as_object_mut(), changes.as_object()) {
        for (name, value) in changes {
            target.insert(name.clone(), value.clone());
        }
    }
    call_in(router, region, "UpdateRecoveryPlan", &body).await
}

/// `POST /DeleteRecoveryPlan` for one plan's ARN.
async fn delete(router: &MultiServiceRouter, region: &str, plan_arn: &str) -> Answer {
    call_in(
        router,
        region,
        "DeleteRecoveryPlan",
        &json!({"recoveryPlanArn": plan_arn}),
    )
    .await
}

/// `value` as the INSTANT it denotes, which is what an order comparison between two
/// timestamps reads.
///
/// The modelled `ISO8601DatetimeString` renders fractional seconds with trailing zeros
/// trimmed, so two instants of the same second carry fractions of different widths and
/// their TEXT does not order them: `…:47.513Z` is the same instant as `…:47.513000Z` and
/// precedes `…:47.513364Z`, while comparing the two strings puts it after. Parsed with
/// the same reader [`is_iso8601`] admits the value with.
fn instant(value: &str) -> (i64, u32) {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        value,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("expected {value:?} to be an ISO-8601 instant: {error}"));
    (parsed.secs(), parsed.subsec_nanos())
}

/// The status of a reported plan.
fn status_of(answer: &Answer) -> String {
    plan_of(answer)
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert that `answer` reports a plan in a modelled status, and that it is the
/// executable one: a plan holding no steps is not judged unexecutable.
fn assert_executable(case: &str, answer: &Answer) {
    let status = status_of(answer);
    assert!(
        RECOVERY_PLAN_STATUS_VALUES.contains(&status.as_str()),
        "case {case}: {status:?} is not one of the modelled statuses \
         {RECOVERY_PLAN_STATUS_VALUES:?}"
    );
    assert_eq!(
        status, "ACTIVE",
        "case {case}: a plan holding no steps is executable, got {answer:?}"
    );
}

/// ARNs that address no plan of this account, each absent for a different reason: an
/// id nothing minted, a stored plan's id under another Region or another account, and
/// an ARN of another resource type of this service. Every one satisfies the modelled
/// `StrictDRSARN`, so each reaches the handler rather than the boundary validator.
fn absent_arns(stored_id: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "an id nothing minted",
            format!("arn:aws:drs:{REGION}:{ACCOUNT}:recovery-plan/plan-00000000000000000"),
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
            format!("arn:aws:drs:eu-central-1:{ACCOUNT}:recovery-plan/{stored_id}"),
        ),
        (
            "a stored plan's id under another account",
            format!("arn:aws:drs:{REGION}:123456789012:recovery-plan/{stored_id}"),
        ),
        (
            "another resource type of this service",
            format!(
                "arn:aws:drs:{REGION}:{ACCOUNT}:replication-configuration-template/\
                 rct-00000000000000000"
            ),
        ),
    ]
}

/// The `recoveryPlanID` of a plan ARN.
fn id_of(plan_arn: &str) -> String {
    plan_arn
        .split_once(":recovery-plan/")
        .unwrap_or_else(|| panic!("expected a recovery-plan ARN, got {plan_arn:?}"))
        .1
        .to_string()
}

// ---------------------------------------------------------------------------
// Rules 1 and 4 — an update writes what it names, and the read reports it
// ---------------------------------------------------------------------------

/// Every update writes the members it named onto the stored plan and the read that
/// follows reports exactly what the update answered. The rows differ along what the
/// rule generalises over: which of the two changeable members the request names,
/// whether the new value is short, empty or at the modelled maximum, and the Region
/// the plan was created in.
#[tokio::test]
async fn an_update_writes_what_it_names_and_the_read_reports_it() {
    // The bounds the longest rows sit on: `RecoveryPlanName` is length 1..256 and
    // `RecoveryPlanDescription` length 0..1024.
    let longest_name = "n".repeat(256);
    let longest_description = "d".repeat(1024);
    let cases: Vec<(&str, &str, Value)> = vec![
        ("the name alone", REGION, json!({"name": "Renamed plan"})),
        (
            "the description alone",
            REGION,
            json!({"description": "Recovers the order-processing tier"}),
        ),
        (
            "both members at once",
            "us-east-1",
            json!({"name": "Both-at-once", "description": "and a new description"}),
        ),
        (
            "an empty description, which the model's length 0..1024 allows",
            REGION,
            json!({"description": ""}),
        ),
        (
            "a one-character name",
            "ap-southeast-2",
            json!({"name": "A"}),
        ),
        (
            "both members at the longest modelled length",
            REGION,
            json!({"name": longest_name, "description": longest_description}),
        ),
        (
            "neither member: an update that names nothing to change",
            REGION,
            json!({}),
        ),
    ];

    let (router, engine) = drs_with_engine();
    for (case, region, changes) in &cases {
        // Each row starts from a plan carrying BOTH changeable members and a tag set,
        // so a member the update leaves out is proved to keep the value it had.
        let created = create(
            &router,
            region,
            &json!({
                "name": "Nightly DR",
                "description": "the description the create stored",
                "tags": {"env": "prod", "team": "dr"},
            }),
        )
        .await;
        let before = plan_of(&created).clone();
        let arn = arn_of(&created);

        let answer = update(&router, region, &arn, changes).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 for {changes}, got {answer:?}"
        );
        let after = plan_of(&answer).clone();

        // What the update NAMED is what changed; everything else the plan held is
        // still what it was.
        for member in ["name", "description"] {
            let expected = changes.get(member).or_else(|| before.get(member));
            assert_eq!(
                after.get(member),
                expected,
                "case {case}: {member} must be {expected:?} after {changes}, got {after}"
            );
        }
        for member in ["recoveryPlanArn", "createdAt", "tags"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: an update must not move {member}, got {after}"
            );
        }
        let updated_at = after
            .get("updatedAt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            is_iso8601(updated_at),
            "case {case}: updatedAt must stay an ISO-8601 instant in UTC, got {after}"
        );
        let before_updated_at = before
            .get("updatedAt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            instant(updated_at) >= instant(before_updated_at),
            "case {case}: updatedAt must not move backwards from {before_updated_at}, got \
             {after}"
        );
        assert_executable(case, &answer);

        // Rule 1's point: the write reached the STORE, so the read reports it — twice,
        // and a read changes nothing a later caller sees.
        for attempt in 1..=2 {
            let read = get(&router, region, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case}, read {attempt}: expected the plan to read back, got {read:?}"
            );
            assert_eq!(
                plan_of(&read),
                &after,
                "case {case}, read {attempt}: the read must report what the update \
                 wrote, got {read:?}"
            );
        }
    }

    assert_eq!(
        stored_plans(&engine),
        cases.len(),
        "an update must change a plan rather than mint one"
    );
}

/// Updates applied one after another accumulate on the one plan: each names one member
/// and the other keeps what the previous update left, so the plan's state is the whole
/// history of its writes rather than the last request echoed back.
#[tokio::test]
async fn updates_accumulate_on_the_stored_plan() {
    let (router, engine) = drs_with_engine();
    let created = create(&router, REGION, &json!({"name": "First name"})).await;
    let arn = arn_of(&created);
    assert_eq!(
        plan_of(&created).get("description"),
        None,
        "a create that named no description stores none, got {created:?}"
    );

    let steps: Vec<(&str, Value, &str, Option<&str>)> = vec![
        (
            "a description onto a plan that had none",
            json!({"description": "first description"}),
            "First name",
            Some("first description"),
        ),
        (
            "a rename, leaving the description alone",
            json!({"name": "Second name"}),
            "Second name",
            Some("first description"),
        ),
        (
            "a re-description, leaving the name alone",
            json!({"description": "second description"}),
            "Second name",
            Some("second description"),
        ),
        (
            "both at once",
            json!({"name": "Third name", "description": ""}),
            "Third name",
            Some(""),
        ),
    ];

    for (case, changes, expected_name, expected_description) in &steps {
        let answer = update(&router, REGION, &arn, changes).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {answer:?}"
        );
        let read = get(&router, REGION, &arn).await;
        let plan = plan_of(&read).clone();
        assert_eq!(
            plan.get("name").and_then(Value::as_str),
            Some(*expected_name),
            "case {case}: the read must report the accumulated name, got {plan}"
        );
        assert_eq!(
            plan.get("description").and_then(Value::as_str),
            *expected_description,
            "case {case}: the read must report the accumulated description, got {plan}"
        );
        assert_executable(case, &read);
    }

    assert_eq!(
        stored_plans(&engine),
        1,
        "every update changed the one plan the create minted"
    );
}

// ---------------------------------------------------------------------------
// Rule 2 — a delete takes the plan away
// ---------------------------------------------------------------------------

/// A delete answers 200 echoing the ARN it removed, the plan stops being readable —
/// with the same refusal an ARN nothing ever minted gets — and every other plan is
/// untouched. The rows differ along which plan of several is removed, the Region it was
/// created in, and whether it carried a description and tags.
#[tokio::test]
async fn a_deleted_plan_is_no_longer_readable() {
    let fleet: Vec<(&str, &str, Value)> = vec![
        (
            "the first plan minted, name only",
            REGION,
            json!({"name": "Plan one"}),
        ),
        (
            "a plan carrying a description and tags",
            REGION,
            json!({
                "name": "Plan two",
                "description": "the second plan",
                "tags": {"env": "prod"},
            }),
        ),
        (
            "a plan in another Region",
            "us-east-1",
            json!({"name": "Plan three"}),
        ),
        (
            "the last plan minted",
            "ap-southeast-2",
            json!({"name": "Plan four", "description": ""}),
        ),
    ];

    let (router, engine) = drs_with_engine();
    let mut plans: Vec<(&str, &str, String)> = Vec::new();
    for (case, region, body) in &fleet {
        let created = create(&router, region, body).await;
        assert_executable(case, &created);
        plans.push((case, region, arn_of(&created)));
    }
    assert_eq!(
        stored_plans(&engine),
        fleet.len(),
        "every create must have minted its plan before any is deleted"
    );

    for index in 0..plans.len() {
        let (case, region, arn) = plans[index].clone();
        let removed = delete(&router, region, &arn).await;
        assert_eq!(
            removed.status, 200,
            "case {case}: expected 200 from the delete, got {removed:?}"
        );
        assert_eq!(
            removed.member("recoveryPlanArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case}: the delete must echo the ARN it removed, got {removed:?}"
        );

        // The plan is gone: the read is the modelled not-found refusal, and so is a
        // repeat of the delete.
        let read = get(&router, region, &arn).await;
        assert_eq!(
            read.status, 404,
            "case {case}: a deleted plan must not read back, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {read:?}"
        );
        assert_eq!(
            text(&read, "message"),
            not_found_message(&arn),
            "case {case}: wrong message in {read:?}"
        );
        assert_eq!(
            text(&read, "resourceType"),
            "Recovery Plan",
            "case {case}: wrong resourceType in {read:?}"
        );
        let again = delete(&router, region, &arn).await;
        assert_eq!(
            again.status, 404,
            "case {case}: a repeated delete must be the not-found refusal, got {again:?}"
        );

        // An update of the deleted plan is refused as well, and the plans NOT deleted
        // are all still readable.
        let changed = update(&router, region, &arn, &json!({"name": "Revived"})).await;
        assert_eq!(
            changed.status, 404,
            "case {case}: a deleted plan cannot be updated, got {changed:?}"
        );
        assert_eq!(
            stored_plans(&engine),
            plans.len() - index - 1,
            "case {case}: the delete must remove exactly one plan"
        );
        for (other_case, other_region, other_arn) in plans.iter().skip(index + 1) {
            let survivor = get(&router, other_region, other_arn).await;
            assert_eq!(
                survivor.status, 200,
                "case {case}: deleting it must leave {other_case} readable, got {survivor:?}"
            );
        }
    }

    assert_eq!(
        stored_plans(&engine),
        0,
        "every plan the fleet minted was deleted"
    );
}

// ---------------------------------------------------------------------------
// Rule 3 — an ARN no plan of this account answers to
// ---------------------------------------------------------------------------

/// An update or a delete addressing an ARN this account holds no plan for is 404
/// `ResourceNotFoundException` naming the plan id that ARN addresses, and changes
/// nothing: the plan that WAS minted keeps every member it had.
#[tokio::test]
async fn an_absent_plan_cannot_be_updated_or_deleted() {
    let (router, engine) = drs_with_engine();
    let created = create(
        &router,
        REGION,
        &json!({"name": "Present", "description": "untouched", "tags": {"env": "prod"}}),
    )
    .await;
    let present = arn_of(&created);
    let before = plan_of(&created).clone();
    let id = id_of(&present);

    for (case, arn) in &absent_arns(&id) {
        for operation in ["UpdateRecoveryPlan", "DeleteRecoveryPlan"] {
            let answer = if operation == "UpdateRecoveryPlan" {
                update(&router, REGION, arn, &json!({"name": "Nowhere"})).await
            } else {
                delete(&router, REGION, arn).await
            };
            assert_eq!(
                answer.status, 404,
                "case {case} on {operation}: expected 404 for {arn:?}, got {answer:?}"
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case} on {operation}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                text(&answer, "message"),
                not_found_message(arn),
                "case {case} on {operation}: wrong message in {answer:?}"
            );
            assert_eq!(
                text(&answer, "resourceId"),
                *arn,
                "case {case} on {operation}: resourceId must be the ARN the request \
                 named, got {answer:?}"
            );
        }

        let read = get(&router, REGION, &present).await;
        assert_eq!(
            plan_of(&read),
            &before,
            "case {case}: a refused update or delete must change nothing, got {read:?}"
        );
    }

    assert_eq!(
        stored_plans(&engine),
        1,
        "a refused update or delete must neither mint nor remove a plan"
    );
}
