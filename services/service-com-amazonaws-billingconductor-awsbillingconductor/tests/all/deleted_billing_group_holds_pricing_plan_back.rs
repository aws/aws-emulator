// SPDX-License-Identifier: CC0-1.0

//! The reach of `DeletePricingPlan`'s "must not be associated with any billing groups"
//! precondition over the groups a delete has since REMOVED, end to end over the service's
//! real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same path the
//! production router serves.
//!
//! RULE: the delete of a pricing plan is refused while any billing-group generation a
//! period-scoped read of this store can still report computes with that plan, whether or
//! not the group is still stored — the groups such a read reports are the stored ones and,
//! for every period before the delete took effect, the ones a delete has removed, each as
//! the generation that was in force then. It is served once no period reports a generation
//! that names the plan.
//!
//! The invariant it serves is that the readers of one association cannot disagree: a plan
//! ARN `ListBillingGroups` reports for a billing period names a plan the store still holds,
//! so it is a plan `UpdatePricingPlan` answers for and a plan a computation preference may
//! name — never an ARN that resolves to nothing.
//!
//! `tests/all/association_delete_gate_spans_billing_periods.rs` owns this rule for a group
//! that is still STORED (and lends this suite its readers); this suite owns the generations
//! of a group a delete has removed, which `ListBillingGroups` goes on reporting for the
//! periods before that delete (`tests/all/deleted_billing_group_period_reads.rs`).
//!
//! Rows differ along what the rule generalises over: how many billing periods the deleted
//! group's reported life spanned (none at all — created and deleted in one period — one,
//! two, three and thirteen, which crosses a year), where that life falls relative to the
//! current period (ending in it, ending a year back), which generation of the group names
//! the plan (the creation one, a frozen one, and one that took effect only in the period of
//! the delete, which no read reports), whether a STORED group names it too, and which
//! spelling the modelled `PricingPlanArn` pattern admits the delete addresses the plan by.
//!
//! The wire can neither write nor delete in a PAST billing period — a call happens now — so
//! a row writes through the real wire and then backdates the stored instants, through the
//! harness `tests/all/billing_group_billing_periods.rs` owns.

use serde_json::json;

use crate::association_delete_gate_spans_billing_periods::{
    assert_every_reported_plan_resolves, error_code, plan_is_stored, reported_plan,
};
use crate::billing_group_billing_periods::{
    current_period, primary_account_id, Emulator as GroupEmulator,
};
use crate::pricing_rule_association_periods::{shift_period, DELETE_PLAN_PATH};

/// How a row spells the plan its delete addresses: both spellings the modelled
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

/// One row of the table: a world in which ONE group that computed with the plan under test
/// has since been deleted.
struct Case {
    label: &'static str,
    /// How many billing periods back from the current one the group was CREATED in.
    created_back: i64,
    /// How many periods back the write that repointed the group's computation preference
    /// landed, when the row makes one.
    repointed_back: Option<i64>,
    /// Does the group's CREATION generation compute with the plan under test (otherwise
    /// the repoint is what points it at that plan)?
    plan_at_creation: bool,
    /// How many periods back the DELETE of the group was made in.
    deleted_back: i64,
    /// Does a group the store still HOLDS compute with the plan as well?
    stored_group_too: bool,
    /// How the delete under test spells the plan's ARN.
    spelling: Spelling,
    /// Is the delete of the plan refused in this world? Stated per row rather than
    /// derived, so the table says what the rule requires.
    refused: bool,
}

/// The billing periods a row probes: from one period before the group was created through
/// the period after the current one, so the probe spans the whole life the reads report the
/// group for and one period on either side of it.
fn probe_periods(now: &str, created_back: i64) -> Vec<String> {
    (-1..=created_back + 1)
        .map(|back| shift_period(now, -back))
        .collect()
}

#[tokio::test]
async fn a_plan_a_deleted_billing_group_still_computes_with_cannot_be_deleted() {
    let em = GroupEmulator::new();
    let now = current_period();

    let cases = [
        Case {
            label: "created one period back, deleted in the current one",
            created_back: 1,
            repointed_back: None,
            plan_at_creation: true,
            deleted_back: 0,
            stored_group_too: false,
            spelling: as_full_arn,
            refused: true,
        },
        Case {
            label: "created one period back, deleted in the current one, by bare id",
            created_back: 1,
            repointed_back: None,
            plan_at_creation: true,
            deleted_back: 0,
            stored_group_too: false,
            spelling: bare_id,
            refused: true,
        },
        Case {
            label: "a life of thirteen periods, crossing a year",
            created_back: 13,
            repointed_back: None,
            plan_at_creation: true,
            deleted_back: 0,
            stored_group_too: false,
            spelling: as_full_arn,
            refused: true,
        },
        Case {
            label: "a life of three periods that ended a year back",
            created_back: 15,
            repointed_back: None,
            plan_at_creation: true,
            deleted_back: 12,
            stored_group_too: false,
            spelling: bare_id,
            refused: true,
        },
        Case {
            label: "created and deleted in the current period, so no period reports it",
            created_back: 0,
            repointed_back: None,
            plan_at_creation: true,
            deleted_back: 0,
            stored_group_too: false,
            spelling: as_full_arn,
            refused: false,
        },
        Case {
            label: "created and deleted in one period a year back",
            created_back: 12,
            repointed_back: None,
            plan_at_creation: true,
            deleted_back: 12,
            stored_group_too: false,
            spelling: bare_id,
            refused: false,
        },
        Case {
            label: "the frozen creation generation computes with it, the live one did not",
            created_back: 2,
            repointed_back: Some(1),
            plan_at_creation: true,
            deleted_back: 0,
            stored_group_too: false,
            spelling: as_full_arn,
            refused: true,
        },
        Case {
            label: "a repoint one period before the delete pointed it at the plan",
            created_back: 2,
            repointed_back: Some(1),
            plan_at_creation: false,
            deleted_back: 0,
            stored_group_too: false,
            spelling: bare_id,
            refused: true,
        },
        Case {
            label: "the repoint that named it landed in the delete's own period",
            created_back: 1,
            repointed_back: Some(0),
            plan_at_creation: false,
            deleted_back: 0,
            stored_group_too: false,
            spelling: as_full_arn,
            refused: false,
        },
        Case {
            label: "a deleted group and a stored one both compute with it",
            created_back: 1,
            repointed_back: None,
            plan_at_creation: true,
            deleted_back: 0,
            stored_group_too: true,
            spelling: as_full_arn,
            refused: true,
        },
    ];

    // A group's primary account is one of its own linked accounts and an account belongs to
    // at most one billing group, so every group of this table gets its own.
    let mut account = 0usize;
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let plan = em.create_plan(&format!("DeletedGroupGatePlan{index}")).await;
        // The plan a repoint points the group at, or away from: a write that restates the
        // computation preference the group already holds changes nothing and so opens no
        // generation (`tests/all/update_billing_group_no_op_generations.rs`).
        let filler = em
            .create_plan(&format!("DeletedGroupGateFiller{index}"))
            .await;

        let created = shift_period(&now, -case.created_back);
        let deleted = shift_period(&now, -case.deleted_back);
        let initial = if case.plan_at_creation { &plan } else { &filler };
        let group = em
            .create_group(
                &format!("DeletedGroupGateGroup{index}"),
                initial,
                &primary_account_id(account),
                &json!({}),
            )
            .await;
        account += 1;
        em.created_in(&group, &created);
        if let Some(repointed_back) = case.repointed_back {
            let target = if case.plan_at_creation { &filler } else { &plan };
            em.update_group(
                label,
                &group,
                &json!({"ComputationPreference": {"PricingPlanArn": target}}),
            )
            .await;
            em.generation_started_in(&group, &shift_period(&now, -repointed_back));
        }
        em.delete_group(label, &group).await;
        em.deleted_in(&group, &deleted);

        // The group the store still holds, when the row has one: a holder the gate found
        // before this fix too, here to show the two kinds of holder are counted together.
        let mut holders = vec![group.clone()];
        if case.stored_group_too {
            let stored = em
                .create_group(
                    &format!("DeletedGroupGateStored{index}"),
                    &plan,
                    &primary_account_id(account),
                    &json!({}),
                )
                .await;
            account += 1;
            holders.push(stored);
        }

        let probes = probe_periods(&now, case.created_back);
        // What the period-scoped reader says about the deleted group BEFORE the delete of
        // the plan is what the gate has to decide from.
        let mut reporting: Vec<String> = Vec::new();
        for period in &probes {
            if reported_plan(&em, label, &group, period).await.as_deref() == Some(plan.as_str()) {
                reporting.push(period.clone());
            }
        }
        assert_eq!(
            !reporting.is_empty(),
            case.refused,
            "case {label:?}: expected the periods reporting the deleted group with plan {plan} \
             to be {}, got {reporting:?}",
            if case.refused { "some" } else { "none" }
        );

        let addressed = (case.spelling)(&plan);
        let (status, body) = em
            .post(DELETE_PLAN_PATH, &json!({"Arn": addressed}))
            .await;

        if !case.refused {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the delete of a plan no period reports to be served, \
                 got {status} with {body:?}"
            );
            assert!(
                !plan_is_stored(&em, label, &plan).await,
                "case {label:?}: the deleted plan {plan} is still stored"
            );
            assert_every_reported_plan_resolves(&em, label, &group, &probes).await;
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
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            holders.iter().any(|holder| message.contains(holder)),
            "case {label:?}: expected the conflict to name a billing group that computes with \
             the plan ({holders:?}), got {message:?}"
        );

        // The refusal wrote nothing, and it is what keeps the two readers of the
        // association agreeing: every plan ARN the group listing reports resolves.
        assert!(
            plan_is_stored(&em, label, &plan).await,
            "case {label:?}: a refused delete must leave the plan {plan} stored"
        );
        for period in &reporting {
            assert_eq!(
                reported_plan(&em, label, &group, period).await.as_deref(),
                Some(plan.as_str()),
                "case {label:?}: a refused delete must leave the period {period} reporting the \
                 plan it reported before"
            );
        }
        assert_every_reported_plan_resolves(&em, label, &group, &probes).await;
        for holder in &holders {
            assert_every_reported_plan_resolves(&em, label, holder, &probes).await;
        }
    }
}
