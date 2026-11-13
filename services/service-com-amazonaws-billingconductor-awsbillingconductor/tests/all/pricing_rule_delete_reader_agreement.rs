// SPDX-License-Identifier: CC0-1.0

//! What the READERS of a pricing rule / pricing plan association report around a
//! `DeletePricingRule`, end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE: every pricing rule ARN a period-scoped reader of a plan's associations reports
//! names a rule the store still holds, so the readers of one association cannot
//! disagree. Concretely, for every billing period a read can ask about:
//!
//! * the rules `ListPricingRulesAssociatedToPricingPlan` reports for a plan are exactly
//!   the ones `PricingPlanListElement.Size` counts for it;
//! * each of those rules is reported by `ListPricingRules` for that period, so the ARN
//!   handed to the caller resolves;
//! * `ListPricingPlansAssociatedWithPricingRule` for each of them answers 200 and names
//!   the plan back.
//!
//! The write that could break it is the rule's removal, so `DeletePricingRule` is served
//! only while NO billing period reports an association to the rule, and is refused with
//! the modelled `ConflictException` (reason
//! `PRICING_RULE_ATTACHED_TO_PRICING_PLAN_DELETE_CONFLICT`) while one does — including an
//! association a later call has ENDED, which a read of a period it was in force in still
//! reports. The hold is the PLAN's: a plan's associations go with the plan, so deleting
//! the plan makes the rule deletable again (the second table).
//!
//! Rows differ along what the rule generalises over: how far back the association started
//! (the current period, one, two and thirteen periods back, which crosses a year
//! boundary), whether it still stands or a later call ended it, whether the rule was ever
//! associated at all, how many other rules the plan keeps associated (0, 1, 2), which
//! write made the association (the create that names the rule, or `AssociatePricingRules`)
//! and which of the two spellings the modelled `PricingRuleArn` pattern admits the delete
//! addresses the rule by.
//!
//! The wire cannot make an association in a PAST billing period — an associating call
//! happens now — so a row writes through the real wire and then backdates the stored start
//! period through the harness `tests/all/pricing_rule_association_periods.rs` owns. That
//! harness owns the readers too (`Emulator::rules_for_plan`, `Emulator::plans_for_rule_answer`
//! and `Emulator::rule_is_reported`) and the invariant over them
//! (`Emulator::assert_readers_agree`), so every suite reads one association the same way and
//! is held to the same agreement — `tests/all/association_delete_gate_spans_billing_periods.rs`,
//! which owns the delete gate's billing-period reach, asserts it on its own rows.

use serde_json::json;

use crate::association_delete_gate_spans_billing_periods::error_code;
use crate::pricing_rule_association_periods::{
    probe_periods, shift_period, Emulator, DELETE_PLAN_PATH, DELETE_RULE_PATH, DISASSOCIATE_PATH,
};

/// How a row spells the rule its delete addresses: both spellings the modelled
/// `PricingRuleArn` pattern admits name the same record.
type Spelling = fn(&str) -> String;

/// The full ARN, unchanged.
fn as_full_arn(arn: &str) -> String {
    arn.to_string()
}

/// The bare resource id of a full ARN — the other spelling the modelled pattern admits.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .expect("a full ARN ends in its resource id")
        .to_string()
}

/// The billing period the service calls current, read from the same clock it answers from
/// so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// Does `ListPricingRules` report `rule` for `period`? The period-scoped existence probe
/// for a rule, through the harness ([`Emulator::rule_is_reported`]): the ARN a plan's
/// association reports resolves exactly while this holds.
async fn rule_is_reported(em: &Emulator, case: &str, rule: &str, period: &str) -> bool {
    em.rule_is_reported(case, rule, Some(period)).await
}

/// One row of the delete table.
struct DeleteCase {
    label: &'static str,
    /// How many billing periods back the plan's association with the rule started, or
    /// `None` for a rule no plan ever associated.
    months_back: Option<i64>,
    /// Did a later `DisassociatePricingRules` end that association?
    disassociated: bool,
    /// How many OTHER rules the plan keeps associated throughout.
    other_rules: usize,
    /// Was the association made by `AssociatePricingRules` rather than by the create that
    /// names the rule?
    associated_separately: bool,
    /// How the delete under test spells the rule's ARN.
    spelling: Spelling,
}

impl DeleteCase {
    /// Is the delete of this row's rule served? Exactly when no billing period reports an
    /// association to it: it was never associated, or its association began and ended in
    /// one period and so was in force in none.
    fn is_served(&self) -> bool {
        match self.months_back {
            None => true,
            Some(back) => self.disassociated && back == 0,
        }
    }
}

#[tokio::test]
async fn deleting_a_pricing_rule_leaves_every_association_reader_agreeing() {
    let em = Emulator::new();
    let now = current_period();
    let cases = [
        DeleteCase {
            label: "an association still standing, made in the current period",
            months_back: Some(0),
            disassociated: false,
            other_rules: 0,
            associated_separately: false,
            spelling: as_full_arn,
        },
        DeleteCase {
            label: "an association made and ended in the current period",
            months_back: Some(0),
            disassociated: true,
            other_rules: 1,
            associated_separately: false,
            spelling: as_full_arn,
        },
        DeleteCase {
            label: "an association started one period back and ended now, by bare id",
            months_back: Some(1),
            disassociated: true,
            other_rules: 0,
            associated_separately: false,
            spelling: bare_id,
        },
        DeleteCase {
            label: "an association started two periods back and ended now",
            months_back: Some(2),
            disassociated: true,
            other_rules: 2,
            associated_separately: false,
            spelling: as_full_arn,
        },
        DeleteCase {
            label: "an association started thirteen periods back and ended now",
            months_back: Some(13),
            disassociated: true,
            other_rules: 1,
            associated_separately: false,
            spelling: bare_id,
        },
        DeleteCase {
            label: "an association still standing, started one period back",
            months_back: Some(1),
            disassociated: false,
            other_rules: 1,
            associated_separately: false,
            spelling: as_full_arn,
        },
        DeleteCase {
            label: "an association AssociatePricingRules made, started two back and ended now",
            months_back: Some(2),
            disassociated: true,
            other_rules: 1,
            associated_separately: true,
            spelling: bare_id,
        },
        DeleteCase {
            label: "a rule no plan ever associated",
            months_back: None,
            disassociated: false,
            other_rules: 2,
            associated_separately: false,
            spelling: bare_id,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let target = em.create_rule(&format!("AgreementTarget{index}")).await;
        let mut others = Vec::new();
        for ordinal in 0..case.other_rules {
            others.push(
                em.create_rule(&format!("AgreementOther{index}x{ordinal}"))
                    .await,
            );
        }

        // The plan associates the target either at create time or through the associating
        // call, and keeps the other rules associated throughout. A plan is created with
        // at least one rule, so the rows that associate separately keep another rule.
        let mut created_with = others.clone();
        if case.months_back.is_some() && !case.associated_separately {
            created_with.push(target.clone());
        }
        let plan = em
            .create_plan(&format!("agreement-plan-{index}"), &created_with)
            .await;
        if case.months_back.is_some() && case.associated_separately {
            em.associate(&plan, std::slice::from_ref(&target)).await;
        }

        let start = shift_period(&now, -case.months_back.unwrap_or(0));
        if case.months_back.is_some_and(|back| back > 0) {
            em.start_association_in(&plan, &target, &start);
        }
        if case.months_back.is_some() {
            let reported = em.rules_for_plan(label, &plan, Some(&start)).await;
            assert!(
                reported.iter().any(|arn| arn == &target),
                "case {label:?}: setup expected the period {start} to report the association \
                 with {target}, got {reported:?}"
            );
        }
        if case.disassociated {
            let (status, body) = em
                .put(
                    DISASSOCIATE_PATH,
                    &json!({"Arn": plan, "PricingRuleArns": [target]}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: setup DisassociatePricingRules expected 200, got {status} with \
                 {body:?}"
            );
        }

        let addressed = (case.spelling)(&target);
        let (status, body) = em
            .post(DELETE_RULE_PATH, &json!({"Arn": addressed}))
            .await;
        let periods = probe_periods(&start, &now);

        if case.is_served() {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the delete of {addressed} to be served, got {status} \
                 with {body:?}"
            );
            assert_eq!(
                body["Arn"].as_str(),
                Some(target.as_str()),
                "case {label:?}: expected the deleted rule's full ARN in the response, got \
                 {body:?}"
            );
            let (status, plans) = em.plans_for_rule_answer(&target, Some(&now)).await;
            assert_eq!(
                status, 404,
                "case {label:?}: expected the deleted rule {target} to resolve nowhere, got \
                 {status} with {plans:?}"
            );
            for period in &periods {
                assert!(
                    !rule_is_reported(&em, label, &target, period).await,
                    "case {label:?}: ListPricingRules still reports the deleted rule {target} \
                     for {period}"
                );
                let reported = em.rules_for_plan(label, &plan, Some(period)).await;
                assert!(
                    !reported.iter().any(|arn| arn == &target),
                    "case {label:?}: the plan {plan} still reports the deleted rule {target} \
                     for {period}, got {reported:?}"
                );
            }
        } else {
            assert_eq!(
                status, 409,
                "case {label:?}: expected HTTP 409 deleting {addressed}, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {label:?}: expected ConflictException, got {body:?}"
            );
            assert_eq!(
                body["Reason"].as_str(),
                Some("PRICING_RULE_ATTACHED_TO_PRICING_PLAN_DELETE_CONFLICT"),
                "case {label:?}: expected the reason the model enumerates for this conflict, \
                 got {body:?}"
            );
            assert_eq!(
                body["ResourceId"].as_str(),
                Some(target.as_str()),
                "case {label:?}: expected the conflict to name the rule in use, got {body:?}"
            );
            assert_eq!(
                body["ResourceType"].as_str(),
                Some("pricingrule"),
                "case {label:?}: expected the conflict to name the resource type, got {body:?}"
            );
            // The refusal wrote nothing: the rule is still there and the period that
            // reported the association still reports it.
            assert!(
                rule_is_reported(&em, label, &target, &start).await,
                "case {label:?}: a refused delete must leave the rule {target} stored"
            );
            let reported = em.rules_for_plan(label, &plan, Some(&start)).await;
            assert!(
                reported.iter().any(|arn| arn == &target),
                "case {label:?}: a refused delete must leave the period {start} reporting the \
                 association with {target}, got {reported:?}"
            );
        }

        // Whatever the outcome, the other rules the plan associates are untouched and no
        // reader of any association of this plan disagrees with another.
        let still_there = em.rules_for_plan(label, &plan, Some(&now)).await;
        for other in &others {
            assert!(
                still_there.iter().any(|arn| arn == other),
                "case {label:?}: the plan {plan} must still associate {other} in {now}, got \
                 {still_there:?}"
            );
        }
        em.assert_readers_agree(label, &plan, &periods).await;
    }
}

#[tokio::test]
async fn a_rule_is_deletable_once_the_plan_holding_its_association_is_gone() {
    let em = Emulator::new();
    let now = current_period();
    // Rows differ along how far back the ended association started, so the hold the plan
    // has on the rule is released by the plan's own delete whatever period it covers.
    for (index, months_back) in [1i64, 2, 13].into_iter().enumerate() {
        let start = shift_period(&now, -months_back);
        let case = format!("an association in force from {start:?} until {now:?}");
        let rule = em.create_rule(&format!("ReleasedRule{index}")).await;
        let plan = em
            .create_plan(
                &format!("released-plan-{index}"),
                std::slice::from_ref(&rule),
            )
            .await;
        em.start_association_in(&plan, &rule, &start);
        let (status, body) = em
            .put(
                DISASSOCIATE_PATH,
                &json!({"Arn": plan, "PricingRuleArns": [rule]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup DisassociatePricingRules expected 200, got {status} with \
             {body:?}"
        );

        // While the plan holds the ended association, the rule's delete is refused.
        let (status, body) = em.post(DELETE_RULE_PATH, &json!({"Arn": rule})).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the delete to be refused while {plan} reports the \
             association, got {status} with {body:?}"
        );

        // A plan's associations are the plan's own records, so they go with it.
        let (status, body) = em.post(DELETE_PLAN_PATH, &json!({"Arn": plan})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete of {plan} to be served, got {status} with \
             {body:?}"
        );

        let (status, body) = em.post(DELETE_RULE_PATH, &json!({"Arn": rule})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rule's delete to be served once no plan reports the \
             association, got {status} with {body:?}"
        );
        for period in probe_periods(&start, &now) {
            assert!(
                !rule_is_reported(&em, &case, &rule, &period).await,
                "case {case:?}: ListPricingRules still reports the deleted rule {rule} for \
                 {period}"
            );
            let (status, plans) = em.plans_for_rule_answer(&rule, Some(&period)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the deleted rule to resolve nowhere in {period}, got \
                 {status} with {plans:?}"
            );
        }
    }
}
