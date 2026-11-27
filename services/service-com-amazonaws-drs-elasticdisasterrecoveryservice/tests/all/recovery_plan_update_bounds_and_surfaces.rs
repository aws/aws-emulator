// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests that pin `UpdateRecoveryPlan` where the other plan suites
//! leave off: what the operation refuses, and which readers report what it wrote.
//!
//! Every test drives the service the way the router does — one buffered
//! `http::Request` per call through `MultiServiceRouter::dispatch` — so what is
//! asserted is the wire behaviour (status, error code, JSON body), not a handler's
//! Rust return value. The harness (`call_in`, `Answer`, the plan helpers, the tagging
//! route's `list_tags`) is the one the sibling suites build; the engine is read
//! directly only to COUNT stored plans.
//!
//! What `recovery_plan_update_and_delete` asserts — the members an update writes, the
//! ones it leaves alone and the refusal for an ARN no plan answers to — is not re-asserted
//! here; rule 2 below says where the two suites' tables meet and what each of them is
//! about. The rules under test, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. An update whose members fall outside the bounds the model gives them is refused
//!    and changes nothing: the plan it addressed keeps every member it had, `updatedAt`
//!    included. The refusal asserted is the SERVICE's, not this operation's modelled
//!    `ValidationException`: this service answers every body that does not satisfy the
//!    schema its model publishes with one check made ahead of any handler — HTTP 400,
//!    `BadRequestException`, the fixed message `Invalid request body` — whichever
//!    member failed and whichever rule it failed, which `request_body_schema_refusal`
//!    pins over the whole operation surface.
//! 2. The same member sitting ON each published bound is served and written, so a bound is
//!    admitted rather than refused along with what lies past it. These rows share values
//!    with `recovery_plan_update_and_delete`'s table, which asserts a different rule over
//!    them (WHICH members an update writes); what is asserted here is the one rule 1
//!    refuses past — each bound, from the side that is inside it.
//! 3. An update writes to the ONE stored plan every reader reports, so the single-plan
//!    read, the summary the enumeration reports and the tag set the tagging route
//!    reports all agree afterwards: the first two carry what the update wrote, and the
//!    tag set — which an update names no member for — is the one the plan already
//!    carried.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_plan_lifecycle::{arn_of, create, get, name_of_length, plan_of, stored_plans};
use crate::recovery_plan_pagination::{plans_of, SUMMARY_MEMBERS};
use crate::recovery_plan_update_and_delete::update;
// The error code and the whole message this service answers a request body that does not
// satisfy the schema its model publishes with: `request_body_schema_refusal` owns that
// rule service-wide and declares both literals, so these rows read them from there rather
// than carrying a second copy.
use crate::request_body_schema_refusal::{BAD_REQUEST_CODE, INVALID_REQUEST_BODY};
use crate::replication_configuration_template_lifecycle::{call_in, drs_with_engine};
use crate::resource_tags::list_tags;

/// The Region every request in this suite signs for; every ARN this service mints
/// carries it.
const REGION: &str = "eu-west-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`),
/// which every ARN it reports carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The summary the enumeration reports for one plan's ARN. The members a summary
/// carries and the way its list is read off an answer are `recovery_plan_pagination`'s
/// `SUMMARY_MEMBERS` and `plans_of`, so there is one copy of each.
async fn summary_of(router: &MultiServiceRouter, plan_arn: &str) -> Value {
    let answer = call_in(router, REGION, "ListRecoveryPlans", &json!({})).await;
    assert_eq!(
        answer.status, 200,
        "expected the enumeration to be served, got {answer:?}"
    );
    plans_of(&answer)
        .iter()
        .find(|summary| summary.get("recoveryPlanArn") == Some(&json!(plan_arn)))
        .unwrap_or_else(|| panic!("expected {plan_arn} to be listed, got {answer:?}"))
        .clone()
}

/// The `tags` member the tagging route reports for one plan's ARN.
async fn tags_of(router: &MultiServiceRouter, plan_arn: &str) -> Value {
    let answer = list_tags(router, REGION, plan_arn).await;
    assert_eq!(
        answer.status, 200,
        "expected the plan's tags to be reported, got {answer:?}"
    );
    answer
        .member("tags")
        .cloned()
        .unwrap_or_else(|| panic!("expected a tags member, got {answer:?}"))
}

// ---------------------------------------------------------------------------
// Rule 1 — an update outside a modelled bound changes nothing
// ---------------------------------------------------------------------------

/// Every update whose members fall outside their modelled bounds is refused with 400
/// `BadRequestException`, and the plan it addressed is untouched afterwards. The rows
/// walk each published bound of `UpdateRecoveryPlan`'s members and one past it —
/// `recoveryPlanArn` length 20..2048 and pattern
/// `^arn:aws(-[a-z0-9]+)*:drs:[a-z0-9-]+:[0-9]{12}:[a-zA-Z0-9_/.-]+$`, `name` length
/// 1..256 and pattern `^[a-zA-Z0-9][a-zA-Z0-9 _-]*$`, `description` length 0..1024 —
/// plus the request that omits the one required member.
#[tokio::test]
async fn an_update_outside_a_modelled_bound_is_refused_and_changes_nothing() {
    let (router, engine) = drs_with_engine();
    let created = create(
        &router,
        REGION,
        &json!({
            "name": "Bounded plan",
            "description": "the description the create stored",
            "tags": {"env": "prod"},
        }),
    )
    .await;
    let arn = arn_of(&created);
    let before = plan_of(&created).clone();

    let refused: Vec<(&str, Value)> = vec![
        (
            "no recoveryPlanArn at all, which the model makes required",
            json!({"name": "Nowhere"}),
        ),
        (
            "an empty name, one character short of the shortest modelled",
            json!({"recoveryPlanArn": arn.clone(), "name": ""}),
        ),
        (
            "a name one character past the longest modelled",
            json!({"recoveryPlanArn": arn.clone(), "name": name_of_length(257)}),
        ),
        (
            "a name whose first character the pattern does not allow",
            json!({"recoveryPlanArn": arn.clone(), "name": "-plan"}),
        ),
        (
            "a name carrying a character the pattern does not allow",
            json!({"recoveryPlanArn": arn.clone(), "name": "plan!"}),
        ),
        (
            "a description one character past the longest modelled",
            json!({"recoveryPlanArn": arn.clone(), "description": "d".repeat(1025)}),
        ),
        (
            "an ARN shorter than the shortest modelled",
            json!({"recoveryPlanArn": "arn:aws:drs:::x", "name": "Nowhere"}),
        ),
        (
            "an ARN of another service, which this ARN's pattern does not allow",
            json!({
                "recoveryPlanArn": "arn:aws:s3:::a-bucket/an-object",
                "name": "Nowhere",
            }),
        ),
        (
            "an ARN whose account is not the twelve digits the pattern requires",
            json!({
                "recoveryPlanArn":
                    format!("arn:aws:drs:{REGION}:12345:recovery-plan/plan-00000000000000000"),
                "name": "Nowhere",
            }),
        ),
    ];

    for (case, body) in &refused {
        let answer = call_in(&router, REGION, "UpdateRecoveryPlan", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 for {body}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some(BAD_REQUEST_CODE),
            "case {case}: expected {BAD_REQUEST_CODE}, got {answer:?}"
        );
        assert_eq!(
            answer.member("message").and_then(Value::as_str),
            Some(INVALID_REQUEST_BODY),
            "case {case}: expected the fixed body-schema message, got {answer:?}"
        );

        // Nothing the refusal addressed moved: the plan still reads back exactly as
        // the create left it, `updatedAt` included.
        let read = get(&router, REGION, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the plan must survive the refusal, got {read:?}"
        );
        assert_eq!(
            plan_of(&read),
            &before,
            "case {case}: a refused update must change nothing, got {read:?}"
        );
        assert_eq!(
            stored_plans(&engine),
            1,
            "case {case}: a refused update must neither mint nor remove a plan"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — the bound itself is served
// ---------------------------------------------------------------------------

/// An update sitting ON each published bound is served and written: the answer reports
/// the value and the read that follows reports it too. The rows are the shortest and
/// longest `name` the model allows, every separator its pattern admits, and the
/// shortest and longest `description`.
#[tokio::test]
async fn an_update_at_each_modelled_bound_is_served_and_written() {
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "the shortest name",
            "name",
            json!({"name": name_of_length(1)}),
        ),
        (
            "the longest name",
            "name",
            json!({"name": name_of_length(256)}),
        ),
        (
            "every separator the name pattern allows",
            "name",
            json!({"name": "dr plan_1-a"}),
        ),
        (
            "the shortest description, which the model's length 0..1024 allows",
            "description",
            json!({"description": ""}),
        ),
        (
            "the longest description",
            "description",
            json!({"description": "d".repeat(1024)}),
        ),
    ];

    let (router, engine) = drs_with_engine();
    for (case, member, changes) in &cases {
        let created = create(&router, REGION, &json!({"name": "At the bound"})).await;
        let arn = arn_of(&created);

        let answer = update(&router, REGION, &arn, changes).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: an update at the bound is served, got {answer:?}"
        );
        assert_eq!(
            plan_of(&answer).get(*member),
            changes.get(*member),
            "case {case}: the answer must report the {member} the update wrote, got \
             {answer:?}"
        );

        let read = get(&router, REGION, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the plan to read back, got {read:?}"
        );
        assert_eq!(
            plan_of(&read).get(*member),
            changes.get(*member),
            "case {case}: the read must report the {member} the update wrote, got \
             {read:?}"
        );
    }

    assert_eq!(
        stored_plans(&engine),
        cases.len(),
        "each row changed the plan it created rather than minting another"
    );
}

// ---------------------------------------------------------------------------
// Rule 3 — every reader of the plan reports what the update wrote
// ---------------------------------------------------------------------------

/// What an update writes is what every reader of that plan reports: the single-plan
/// read, the summary the enumeration reports, and the tag set the tagging route
/// reports — the plan's own, which an update names no member for and so does not
/// touch. The rows differ along which changeable member the update names, and each row
/// then names the other member, so the accumulated plan is read through every surface
/// twice.
#[tokio::test]
async fn an_update_is_reported_by_every_reader_of_the_plan() {
    let tags = json!({"env": "prod", "team": "dr"});
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the name first, then the description",
            json!({"name": "Renamed once"}),
            json!({"description": "and then described"}),
        ),
        (
            "the description first, then the name",
            json!({"description": "described first"}),
            json!({"name": "Renamed second"}),
        ),
        (
            "both members at once, then the name alone",
            json!({"name": "Both-at-once", "description": "both at once"}),
            json!({"name": "Renamed after both"}),
        ),
    ];

    let (router, engine) = drs_with_engine();
    for (case, first, second) in &cases {
        let created = create(
            &router,
            REGION,
            &json!({
                "name": "Nightly DR",
                "description": "the description the create stored",
                "tags": tags.clone(),
            }),
        )
        .await;
        let arn = arn_of(&created);
        assert!(
            arn.starts_with(&format!("arn:aws:drs:{REGION}:{ACCOUNT}:recovery-plan/")),
            "case {case}: expected a plan ARN of this Region and account, got {arn:?}"
        );

        for (step, changes) in [("first", first), ("second", second)] {
            let answer = update(&router, REGION, &arn, changes).await;
            assert_eq!(
                answer.status, 200,
                "case {case}, {step} update: expected 200 for {changes}, got {answer:?}"
            );
            let written = plan_of(&answer).clone();

            // The single-plan read reports what the update answered, member for
            // member.
            let read = get(&router, REGION, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case}, {step} update: expected the plan to read back, got {read:?}"
            );
            assert_eq!(
                plan_of(&read),
                &written,
                "case {case}, {step} update: the read must report what the update \
                 wrote, got {read:?}"
            );

            // So does the summary the enumeration reports, for every member it
            // carries.
            let summary = summary_of(&router, &arn).await;
            for member in SUMMARY_MEMBERS {
                assert_eq!(
                    summary.get(member),
                    written.get(member),
                    "case {case}, {step} update: the listed {member} must be the one \
                     the update wrote, got {summary} and {written}"
                );
            }

            // And the tagging route reports the plan's own tag set, which an update
            // carries no member for: the write reached the plan's name and
            // description, not its tags.
            assert_eq!(
                tags_of(&router, &arn).await,
                tags,
                "case {case}, {step} update: an update must leave the plan's tag set \
                 where it was"
            );
            assert_eq!(
                written.get("tags"),
                Some(&tags),
                "case {case}, {step} update: the updated plan reports its own tag set, \
                 got {written}"
            );
        }
    }

    assert_eq!(
        stored_plans(&engine),
        cases.len(),
        "every update changed the plan its row created"
    );
}
