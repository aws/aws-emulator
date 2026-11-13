// SPDX-License-Identifier: CC0-1.0

//! The BILLING-PERIOD reach of a delete's "must not be associated" precondition, end to
//! end over the service's real `restJson1` wire through [`MultiServiceRouter::dispatch`]
//! — the same path the production router serves.
//!
//! RULE: a delete that is refused while the resource is still associated decides that
//! precondition over every association a period-scoped read of this store can still
//! report, not only the one in force in the current billing period. So
//!
//! * `DeletePricingPlan` is refused while ANY generation a period-scoped read of the
//!   billing groups can still report computes with the plan — a group's
//!   `ComputationPreference.PricingPlanArn` is a member a write can change, so each
//!   generation carries the plan it computed with and `ListBillingGroups` reports the
//!   generation in force in the period it is asked about. The rows here keep the group
//!   STORED; `tests/all/deleted_billing_group_holds_pricing_plan_back.rs` owns the
//!   generations of a group a delete has since removed, which those reads go on reporting
//!   for the periods before the delete;
//! * `DeletePricingRule` is refused while a stored pricing plan associates the rule in
//!   ANY billing period, including an association a later call has since ended, which
//!   the readers of a closed period still report.
//!
//! The invariant both serve is that the readers of one association cannot disagree: a
//! plan ARN `ListBillingGroups` reports for a period, and a rule ARN the association
//! readers report for a period, name a resource the store still holds. A delete is served
//! again once NO period reports the association — a repoint inside the generation's own
//! period replaces it rather than freezing it, an association that began and ended in one
//! period was in force in no period at all, and a generation the bounded version history
//! has dropped is reported nowhere.
//!
//! Rows differ along what the rule generalises over: where the naming generation sits in
//! the history (the live one, the newest superseded one, two, three and five repoints
//! back, and one the version bound dropped), how many stored groups name the plan
//! (0, 1, 2), how far back an ended rule association started (the current period, the
//! previous one, two back, and a year and a month back, which crosses a year boundary),
//! and which spelling the modelled pattern admits the delete addresses the resource by.
//!
//! The wire cannot write in a PAST billing period — a write happens now — so a row writes
//! through the real wire and then backdates the stored instants, through the harnesses
//! `tests/all/billing_group_billing_periods.rs` and
//! `tests/all/pricing_rule_association_periods.rs` already own.

use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::state::MAX_BILLING_GROUP_VERSIONS;

use crate::billing_group_billing_periods::{
    current_period, primary_account_id, Emulator as GroupEmulator,
};
use crate::pricing_rule_association_periods::{
    probe_periods, shift_period, Emulator as PlanEmulator, DELETE_PLAN_PATH, DELETE_RULE_PATH,
    DISASSOCIATE_PATH,
};

/// The plan-side reader of a rule association, which is also the existence probe for a
/// pricing plan: 200 for a stored plan, 404 for one that is not stored.
const LIST_RULES_OF_PLAN_PATH: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// The wire error code of a failure response (`__type`), without any namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// How a row spells the resource its delete addresses: both spellings the modelled
/// `PricingPlanArn` pattern admits name the same record.
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

/// A count as the period arithmetic takes it.
fn months(count: usize) -> i64 {
    i64::try_from(count).expect("a small generation count fits in an i64")
}

/// Does the store still hold the pricing plan `plan`? Decided by the read-only
/// `ListPricingRulesAssociatedToPricingPlan`, which answers 200 for a stored plan and the
/// modelled 404 for one that is not.
pub(crate) async fn plan_is_stored(em: &GroupEmulator, case: &str, plan: &str) -> bool {
    let (status, body) = em
        .post(LIST_RULES_OF_PLAN_PATH, &json!({"PricingPlanArn": plan}))
        .await;
    match status {
        200 => true,
        404 => false,
        other => panic!(
            "case {case:?}: ListPricingRulesAssociatedToPricingPlan {plan}: expected 200 or 404, \
             got {other} with {body:?}"
        ),
    }
}

/// The plan ARN `ListBillingGroups` reports for `group` in `period` — the plan the
/// generation in force then computed with — or `None` when the listing does not report
/// the group for that period at all.
pub(crate) async fn reported_plan(
    em: &GroupEmulator,
    case: &str,
    group: &str,
    period: &str,
) -> Option<String> {
    let element = em.element_in(case, group, Some(period)).await?;
    Some(
        element["ComputationPreference"]["PricingPlanArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: no ComputationPreference.PricingPlanArn in {element:?}")
            })
            .to_string(),
    )
}

/// The INVARIANT the gate exists for: every plan ARN the period-scoped group listing
/// reports, for any period, names a plan the store still holds — no reader hands a caller
/// an ARN that resolves to nothing.
pub(crate) async fn assert_every_reported_plan_resolves(
    em: &GroupEmulator,
    case: &str,
    group: &str,
    periods: &[String],
) {
    for period in periods {
        let Some(plan) = reported_plan(em, case, group, period).await else {
            continue;
        };
        assert!(
            plan_is_stored(em, case, &plan).await,
            "case {case:?}: ListBillingGroups reports pricing plan {plan} for billing period \
             {period}, but no such plan is stored"
        );
    }
}

/// Every billing period from one before `repoints` months ago through the current one —
/// the whole span a group of these tables is reported for, plus one period before it.
fn periods_around(now: &str, repoints: usize) -> Vec<String> {
    (0..=repoints + 1)
        .map(|back| shift_period(now, -months(back)))
        .collect()
}

/// One row of the generation table.
struct GenerationCase {
    label: &'static str,
    /// How many period-crossing repoints each group made AFTER the generation that names
    /// the plan — 0 leaves the LIVE generation naming it.
    repoints: usize,
    /// How many stored billing groups name the plan in one of their generations.
    groups: usize,
    /// How the delete under test spells the plan's ARN.
    spelling: Spelling,
}

/// The world a group-side table runs in: the emulator, the billing period it calls
/// current, and the two plans a repoint can point a group at.
struct Groups {
    em: GroupEmulator,
    /// The plans a repoint alternates between, because a write that restates the
    /// computation preference the group already holds changes nothing and so opens no
    /// generation (`tests/all/update_billing_group_no_op_generations.rs`).
    fillers: [String; 2],
    now: String,
}

impl Groups {
    /// An emulator with the two filler plans minted, named from `prefix` so two tables
    /// of this suite never contend for one plan name.
    async fn new(prefix: &str) -> Self {
        let em = GroupEmulator::new();
        let fillers = [
            em.create_plan(&format!("{prefix}FillerA")).await,
            em.create_plan(&format!("{prefix}FillerB")).await,
        ];
        let now = current_period();
        Self { em, fillers, now }
    }

    /// Create a group computing with `plan`, backdate it so its creating call lands
    /// `repoints` billing periods back, and then repoint it that many times, each write
    /// landing in a LATER period than the generation it displaces — so the generation
    /// that computes with `plan` is frozen and `repoints` generations have superseded it
    /// (at most the [`MAX_BILLING_GROUP_VERSIONS`] the store keeps).
    async fn group_with_a_frozen_generation(
        &self,
        label: &str,
        name: &str,
        account: usize,
        plan: &str,
        repoints: usize,
    ) -> String {
        let em = &self.em;
        let group = em
            .create_group(name, plan, &primary_account_id(account), &json!({}))
            .await;
        em.created_in(&group, &shift_period(&self.now, -months(repoints)));
        for step in 1..=repoints {
            em.update_group(
                label,
                &group,
                &json!({"ComputationPreference":
                        {"PricingPlanArn": self.fillers[(step - 1) % 2]}}),
            )
            .await;
            em.generation_started_in(&group, &shift_period(&self.now, -months(repoints - step)));
        }
        assert_eq!(
            em.superseded_generations(&group),
            repoints.min(MAX_BILLING_GROUP_VERSIONS),
            "case {label:?}: expected {} superseded generation(s) of {group}, got {}",
            repoints.min(MAX_BILLING_GROUP_VERSIONS),
            em.superseded_generations(&group)
        );
        group
    }
}

#[tokio::test]
async fn a_plan_any_generation_of_a_stored_group_computes_with_cannot_be_deleted() {
    let world = Groups::new("GenerationGate").await;
    let em = &world.em;
    let now = world.now.clone();

    // Rows differ along where the generation that computes with the plan sits in the
    // group's history, how many stored groups name it that way, and the spelling the
    // delete addresses the plan by.
    let cases = [
        GenerationCase {
            label: "no stored group names the plan",
            repoints: 0,
            groups: 0,
            spelling: as_full_arn,
        },
        GenerationCase {
            label: "the live generation names it, by full ARN",
            repoints: 0,
            groups: 1,
            spelling: as_full_arn,
        },
        GenerationCase {
            label: "the newest superseded generation names it, by full ARN",
            repoints: 1,
            groups: 1,
            spelling: as_full_arn,
        },
        GenerationCase {
            label: "the newest superseded generation names it, by bare id",
            repoints: 1,
            groups: 1,
            spelling: bare_id,
        },
        GenerationCase {
            label: "a generation two repoints back names it",
            repoints: 2,
            groups: 1,
            spelling: as_full_arn,
        },
        GenerationCase {
            label: "a generation five repoints back names it, by bare id",
            repoints: 5,
            groups: 1,
            spelling: bare_id,
        },
        GenerationCase {
            label: "two groups' superseded generations name it",
            repoints: 1,
            groups: 2,
            spelling: as_full_arn,
        },
        GenerationCase {
            label: "two groups, three repoints each, by bare id",
            repoints: 3,
            groups: 2,
            spelling: bare_id,
        },
    ];

    // A group's primary account is one of its own linked accounts and an account belongs
    // to at most one group, so every group of this table gets its own.
    let mut account = 0usize;
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let plan = em.create_plan(&format!("GenerationGatePlan{index}")).await;
        let creation = shift_period(&now, -months(case.repoints));
        let mut groups = Vec::new();
        for ordinal in 0..case.groups {
            let group = world
                .group_with_a_frozen_generation(
                    label,
                    &format!("GenerationGateGroup{index}x{ordinal}"),
                    account,
                    &plan,
                    case.repoints,
                )
                .await;
            account += 1;
            // The generation in force in the creation period is the one that computes
            // with the plan, whatever the group computes with now.
            assert_eq!(
                reported_plan(em, label, &group, &creation).await.as_deref(),
                Some(plan.as_str()),
                "case {label:?}: expected the period {creation} to report the plan the \
                 generation in force then computed with"
            );
            groups.push(group);
        }

        let addressed = (case.spelling)(&plan);
        let (status, body) = em
            .post(DELETE_PLAN_PATH, &json!({"Arn": addressed}))
            .await;

        if case.groups == 0 {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the delete of an unheld plan to be served, got \
                 {status} with {body:?}"
            );
            assert!(
                !plan_is_stored(em, label, &plan).await,
                "case {label:?}: the deleted plan {plan} is still stored"
            );
            continue;
        }

        assert_eq!(
            status, 409,
            "case {label:?}: expected HTTP 409 deleting {addressed}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("PRICING_PLAN_ATTACHED_TO_BILLING_GROUP_DELETE_CONFLICT"),
            "case {label:?}: expected the reason the model enumerates for this conflict, got \
             {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(plan.as_str()),
            "case {label:?}: expected the conflict to name the plan in use, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("pricingplan"),
            "case {label:?}: expected the conflict to name the resource type, got {body:?}"
        );

        // The refusal wrote nothing — and it is what keeps the two readers of the
        // association agreeing.
        assert!(
            plan_is_stored(em, label, &plan).await,
            "case {label:?}: a refused delete must leave the plan {plan} stored"
        );
        let periods = periods_around(&now, case.repoints);
        for group in &groups {
            assert_eq!(
                em.superseded_generations(group),
                case.repoints,
                "case {label:?}: a refused delete must leave {group}'s history untouched"
            );
            assert_eq!(
                reported_plan(em, label, group, &creation).await.as_deref(),
                Some(plan.as_str()),
                "case {label:?}: a refused delete must leave the period {creation} reporting \
                 the plan it reported before"
            );
            assert_every_reported_plan_resolves(em, label, group, &periods).await;
        }
    }
}

#[tokio::test]
async fn a_plan_no_surviving_generation_names_any_more_is_deletable() {
    let world = Groups::new("SamePeriod").await;
    let em = &world.em;
    let now = world.now.clone();
    let fillers = &world.fillers;
    // One billing period has ONE generation of a group, so a repoint made in the
    // generation's own period REPLACES it: afterwards no generation of the group — and
    // so no period a read can ask about — computes with the plan it was pointed away
    // from. Rows differ along how many such repoints were made and the spelling the
    // delete addresses the plan by.
    let rows: [(&str, usize, Spelling); 3] = [
        ("one same-period repoint, by full ARN", 1, as_full_arn),
        ("two same-period repoints, by bare id", 2, bare_id),
        ("five same-period repoints, by full ARN", 5, as_full_arn),
    ];

    for (index, (case, repoints, spelling)) in rows.into_iter().enumerate() {
        let plan = em.create_plan(&format!("SamePeriodPlan{index}")).await;
        let group = em
            .create_group(
                &format!("SamePeriodGroup{index}"),
                &plan,
                &primary_account_id(index),
                &json!({}),
            )
            .await;
        for step in 1..=repoints {
            em.update_group(
                case,
                &group,
                &json!({"ComputationPreference": {"PricingPlanArn": fillers[(step - 1) % 2]}}),
            )
            .await;
        }
        assert_eq!(
            em.superseded_generations(&group),
            0,
            "case {case:?}: a write in the generation's own period replaces it, so no \
             generation is superseded"
        );
        let holds = &fillers[(repoints - 1) % 2];
        assert_eq!(
            reported_plan(em, case, &group, &now).await.as_deref(),
            Some(holds.as_str()),
            "case {case:?}: expected the group to compute with {holds} after its repoints"
        );

        let (status, body) = em
            .post(DELETE_PLAN_PATH, &json!({"Arn": spelling(&plan)}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete of a plan no generation names to be served, \
             got {status} with {body:?}"
        );
        assert!(
            !plan_is_stored(em, case, &plan).await,
            "case {case:?}: the deleted plan {plan} is still stored"
        );
        assert_every_reported_plan_resolves(em, case, &group, &periods_around(&now, 1)).await;
    }
}

#[tokio::test]
async fn a_naming_generation_the_version_bound_dropped_holds_the_plan_back_no_longer() {
    let world = Groups::new("BoundedGate").await;
    let em = &world.em;
    let now = world.now.clone();
    let case = "the naming generation dropped at the kept-history bound";
    let plan = em.create_plan("BoundedGatePlan").await;

    // ONE more period-crossing repoint than the store keeps superseded generations, so
    // the generation that computed with the plan is exactly the one the bound drops —
    // after which the oldest generation still kept answers for the periods before it, and
    // no period reports the plan at all.
    let repoints = MAX_BILLING_GROUP_VERSIONS + 1;
    let group = world
        .group_with_a_frozen_generation(case, "BoundedGateGroup", 0, &plan, repoints)
        .await;
    assert_eq!(
        em.superseded_generations(&group),
        MAX_BILLING_GROUP_VERSIONS,
        "case {case:?}: the store keeps at most {MAX_BILLING_GROUP_VERSIONS} superseded \
         generations"
    );

    // The periods the dropped generation covered fall back to the oldest one still kept,
    // so none of them names the plan any more.
    let probed: Vec<String> = [0, 1, 2, repoints / 2, repoints - 1, repoints, repoints + 1]
        .into_iter()
        .map(|back| shift_period(&now, -months(back)))
        .collect();
    for period in &probed {
        assert_ne!(
            reported_plan(em, case, &group, period).await.as_deref(),
            Some(plan.as_str()),
            "case {case:?}: no period may report a generation the version bound dropped, but \
             {period} reports {plan}"
        );
    }

    let (status, body) = em.post(DELETE_PLAN_PATH, &json!({"Arn": plan})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to be served once no period reports the plan, got \
         {status} with {body:?}"
    );
    assert!(
        !plan_is_stored(em, case, &plan).await,
        "case {case:?}: the deleted plan {plan} is still stored"
    );
    assert_every_reported_plan_resolves(em, case, &group, &probed).await;
}

/// The rules the plan associates in `period`, as the plan-side reader reports them —
/// through the shared harness, so one association has one reader.
async fn rules_of_plan(em: &PlanEmulator, case: &str, plan: &str, period: &str) -> Vec<String> {
    em.rules_for_plan(case, plan, Some(period)).await
}

/// Does the store still hold the pricing rule `rule`? `ListPricingRules` filtered to its
/// ARN reports it exactly while it is stored ([`PlanEmulator::rule_is_reported`]).
async fn rule_is_stored(em: &PlanEmulator, case: &str, rule: &str) -> bool {
    em.rule_is_reported(case, rule, None).await
}

#[tokio::test]
async fn a_rule_a_plan_associated_in_an_earlier_period_cannot_be_deleted() {
    let em = PlanEmulator::new();
    let now = current_period();
    // Rows differ along how far back the ended association started: in the current period
    // (so, the span being half-open, it was in force in no period at all), the previous
    // one, two back, and a year and a month back, which crosses a year boundary.
    for (index, months_back) in [0i64, 1, 2, 13].into_iter().enumerate() {
        let start = shift_period(&now, -months_back);
        let case = format!("an association in force from {start:?} until {now:?}");
        let rule = em.create_rule(&format!("PeriodGateRule{index}")).await;
        let plan = em
            .create_plan(
                &format!("period-gate-plan-{index}"),
                std::slice::from_ref(&rule),
            )
            .await;
        // The wire cannot make an association in a PAST period — an associating call
        // happens now — so the stored start period is backdated.
        if months_back > 0 {
            em.start_association_in(&plan, &rule, &start);
        }
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

        let (status, body) = em.post(DELETE_RULE_PATH, &json!({"Arn": rule})).await;
        if months_back == 0 {
            assert_eq!(
                status, 200,
                "case {case:?}: an association in force in no period holds nothing back, so the \
                 delete is served, got {status} with {body:?}"
            );
            assert!(
                !rule_is_stored(&em, &case, &rule).await,
                "case {case:?}: the deleted rule {rule} is still stored"
            );
            assert!(
                rules_of_plan(&em, &case, &plan, &start).await.is_empty(),
                "case {case:?}: no period reported the association, so none may report the \
                 deleted rule"
            );
            // No reader of this plan's associations may disagree with another either.
            em.assert_readers_agree(&case, &plan, &probe_periods(&start, &now))
                .await;
            continue;
        }

        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 deleting {rule}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("PRICING_RULE_ATTACHED_TO_PRICING_PLAN_DELETE_CONFLICT"),
            "case {case:?}: expected the reason the model enumerates for this conflict, got \
             {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(rule.as_str()),
            "case {case:?}: expected the conflict to name the rule in use, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("pricingrule"),
            "case {case:?}: expected the conflict to name the resource type, got {body:?}"
        );

        // The refusal wrote nothing, and the period that still reports the ended
        // association reports a rule ARN that resolves.
        assert!(
            rule_is_stored(&em, &case, &rule).await,
            "case {case:?}: a refused delete must leave the rule {rule} stored"
        );
        let reported = rules_of_plan(&em, &case, &plan, &start).await;
        assert_eq!(
            reported,
            vec![rule.clone()],
            "case {case:?}: expected the period {start} to still report the association, got \
             {reported:?}"
        );
        em.assert_readers_agree(&case, &plan, &probe_periods(&start, &now))
            .await;
    }
}
