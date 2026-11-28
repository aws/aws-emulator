// SPDX-License-Identifier: CC0-1.0

//! The RETRIEVAL SCOPE of every selection over the recommended-action store.
//!
//! The rule under test: a caller selects the analysis output of its OWN account
//! and of each member account whose association is in place at the time of the
//! call, and of no other account — the association is read per call, never
//! inferred from the action being stored. It holds for every operation that
//! SELECTS over the store, so the table below takes each row through all four of
//! them: `ListRecommendedActions`, `ListRecommendedActionSummaries` (whose
//! account-wide total covers exactly the actions the listing reports) and the two
//! rule previews (whose selection is what a rule would act on, so a scope naming
//! an account the caller may not be scoped to previews nothing of it).
//!
//! The rows differ along the dimension the rule generalises over — which
//! accounts are reachable at the moment of the call — and walk an organization
//! through association, removal of one member, removal of the other, and a
//! re-association, so no row is a property of one account or of one order of
//! calls.
//!
//! Every call goes over the wire: an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`] the emulator binary mounts, through the harness
//! `automation_rule_lifecycle` owns and the opted-in router
//! `recommended_action_inventory` owns (opting in over the wire is what records
//! the caller's own analysed inventory). Nothing reaches into the store, so each
//! row is judged on association state its own calls put there.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::RECOMMENDED_ACTION_TYPE_VALUES;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{call, call_ok, member, string_member};
use super::member_account_association_gate::{
    action_ids, assert_access_denied, associate, disassociate, MEMBER_A,
};
use super::organization_rule_scope_gate::organization_rule;
use super::recommended_action_inventory::{listed_actions, opted_in_emulator};

/// A second member account id in the model's `AccountId` form (`^[0-9]{12}$`),
/// distinct from `MEMBER_A` and from the calling account, so a removal can be
/// judged per account.
///
/// `pub(crate)` with the three helpers below, because the sibling suite that
/// takes the same rule over the EVENT store
/// ([`crate::disassociated_member_event_selection`]) drives the same
/// organization rather than describing a second one.
pub(crate) const MEMBER_B: &str = "777788889999";

/// How savings are compared: the figures are `Double`s the service computed from
/// its price list, so two sums of the same set agree to within rounding.
pub(crate) const SAVINGS_TOLERANCE: f64 = 1e-6;

/// The fixed `key` the account-wide recommended-action summary row carries.
const AGGREGATE_KEY: &str = "ALL_Recommended_Action_Summary";

/// Every item a paginated listing reports for `request`, walked page by page so
/// the whole result set is seen however it is paged.
pub(crate) async fn walk(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    items: &str,
    case: &str,
) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(token) = &token {
            page_request["nextToken"] = json!(token);
        }
        let page = call_ok(router, operation, &page_request, case).await;
        walked.extend(
            member(&page, items)
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: {items} is a list, got {page}"))
                .clone(),
        );
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return walked;
        }
        assert!(
            walked.len() <= 1_000,
            "case {case:?}: the {operation} walk must terminate, walked {} items",
            walked.len()
        );
    }
}

/// The `recommendedActionId`s of reported actions or preview results, as a set:
/// which accounts' actions a selection covers is what every row asserts, and the
/// record order the store walks is pinned by the paging suites.
fn ids(items: &[Value]) -> BTreeSet<String> {
    items
        .iter()
        .map(|item| string_member(item, "recommendedActionId"))
        .collect()
}

/// A member of an `EstimatedMonthlySavings` an item carries, read at `pointer`;
/// an item Compute Optimizer estimated nothing for adds nothing.
pub(crate) fn savings_at(item: &Value, pointer: &str) -> f64 {
    item.pointer(pointer).and_then(Value::as_f64).unwrap_or(0.0)
}

/// A preview request for every recommended action type the model declares, so
/// the selection is narrowed by the SCOPE alone, optionally naming
/// `organizationScope.accountIds`.
fn preview_request(scope: Option<&str>) -> Value {
    let mut request = json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": RECOMMENDED_ACTION_TYPE_VALUES,
    });
    if let Some(account_id) = scope {
        request["organizationScope"] = json!({"accountIds": [account_id]});
    }
    request
}

/// One row of the table: the arrangement step it runs first, and the accounts
/// the caller retrieves from once that step has been served.
struct Row {
    case: &'static str,
    /// The association step to run: `Some((true, id))` associates `id`,
    /// `Some((false, id))` removes its association, `None` runs no step.
    step: Option<(bool, &'static str)>,
    /// The accounts whose analysis output the caller then reaches.
    reachable: Vec<String>,
}

/// The union of the analysed inventories of `accounts`. The inventory is a pure
/// function of the account id (`state::recommended_action_inventory`), so a row
/// names the actions it expects without listing them.
fn inventory_of(accounts: &[String]) -> BTreeSet<String> {
    accounts
        .iter()
        .flat_map(|account_id| action_ids(account_id))
        .collect()
}

/// Assert that `ListRecommendedActionSummaries` totals exactly the actions
/// `ListRecommendedActions` reports: ONE aggregate row, counting those actions
/// and summing their estimates.
async fn assert_summaries_cover(router: &MultiServiceRouter, listed: &[Value], case: &str) {
    let summarised = walk(
        router,
        "ListRecommendedActionSummaries",
        &json!({}),
        "recommendedActionSummaries",
        case,
    )
    .await;

    let count = i64::try_from(listed.len()).expect("a small fixture set");
    let before: f64 = listed
        .iter()
        .map(|action| savings_at(action, "/estimatedMonthlySavings/beforeDiscountSavings"))
        .sum();
    let after: f64 = listed
        .iter()
        .map(|action| savings_at(action, "/estimatedMonthlySavings/afterDiscountSavings"))
        .sum();

    assert_eq!(
        summarised.len(),
        1,
        "case {case:?}: the summaries must be the one account-wide aggregate row, got \
         {summarised:?}"
    );
    let row = &summarised[0];
    assert_eq!(
        string_member(row, "key"),
        AGGREGATE_KEY,
        "case {case:?}: the aggregate row carries the fixed key, got {row}"
    );
    let reported_count = row
        .pointer("/total/recommendedActionCount")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    assert_eq!(
        reported_count, count,
        "case {case:?}: the row must count the {count} actions the listing reports, got {row}"
    );
    let reported_before = savings_at(row, "/total/estimatedMonthlySavings/beforeDiscountSavings");
    assert!(
        (reported_before - before).abs() < SAVINGS_TOLERANCE,
        "case {case:?}: the row must sum the reported actions' savings ({before}), got \
         {reported_before}"
    );
    let reported_after = savings_at(row, "/total/estimatedMonthlySavings/afterDiscountSavings");
    assert!(
        (reported_after - after).abs() < SAVINGS_TOLERANCE,
        "case {case:?}: the row must sum the reported actions' discounted savings ({after}), got \
         {reported_after}"
    );
}

/// Every selection over the store covers exactly the accounts the caller reaches
/// AT THE TIME OF THE CALL: its own, plus each member account whose association
/// is in place then — through association, a removal per member and a
/// re-association.
#[tokio::test]
async fn every_selection_covers_the_accounts_reachable_at_the_time_of_the_call() {
    let caller = state::caller_account_id();
    let router = opted_in_emulator("scope").await;
    associate(&router, MEMBER_A, "arrange").await;
    associate(&router, MEMBER_B, "arrange").await;

    let rows: Vec<Row> = vec![
        Row {
            case: "both-members-associated",
            step: None,
            reachable: vec![caller.clone(), MEMBER_A.to_string(), MEMBER_B.to_string()],
        },
        Row {
            case: "one-member-removed",
            step: Some((false, MEMBER_A)),
            reachable: vec![caller.clone(), MEMBER_B.to_string()],
        },
        Row {
            case: "both-members-removed",
            step: Some((false, MEMBER_B)),
            reachable: vec![caller.clone()],
        },
        Row {
            case: "one-member-associated-again",
            step: Some((true, MEMBER_A)),
            reachable: vec![caller.clone(), MEMBER_A.to_string()],
        },
    ];

    for row in &rows {
        let case = row.case;
        let reachable = &row.reachable;
        if let Some((associate_it, account_id)) = &row.step {
            if *associate_it {
                associate(&router, account_id, case).await;
            } else {
                disassociate(&router, account_id, case).await;
            }
        }
        let expected = inventory_of(reachable);

        // The retrieval listing.
        let listed = listed_actions(&router, &json!({}), case).await;
        assert_eq!(
            ids(&listed),
            expected,
            "case {case:?}: the listing must report the actions of {reachable:?} and no others"
        );

        // The summaries, over the very same actions.
        assert_summaries_cover(&router, &listed, case).await;

        // A preview naming no scope spans the organization, which is every
        // account the caller reaches.
        let previewed = walk(
            &router,
            "ListAutomationRulePreview",
            &preview_request(None),
            "previewResults",
            case,
        )
        .await;
        assert_eq!(
            ids(&previewed),
            expected,
            "case {case:?}: an unscoped preview must cover the actions of {reachable:?} and no \
             others"
        );
        let preview_summaries = walk(
            &router,
            "ListAutomationRulePreviewSummaries",
            &preview_request(None),
            "previewResultSummaries",
            case,
        )
        .await;
        let summarised_count: i64 = preview_summaries
            .iter()
            .map(|summary| {
                summary
                    .pointer("/total/recommendedActionCount")
                    .and_then(Value::as_i64)
                    .unwrap_or_default()
            })
            .sum();
        assert_eq!(
            summarised_count,
            previewed.len() as i64,
            "case {case:?}: the preview summaries must count the {} previewed actions, got \
             {preview_summaries:?}",
            previewed.len()
        );

        // And a preview scoped to ONE account reports that account's inventory
        // while it is reachable, and nothing of it once it is not.
        for account_id in [caller.as_str(), MEMBER_A, MEMBER_B] {
            let row = format!("{case}/scope-{account_id}");
            let scoped = walk(
                &router,
                "ListAutomationRulePreview",
                &preview_request(Some(account_id)),
                "previewResults",
                &row,
            )
            .await;
            let expected_scoped: BTreeSet<String> = if reachable.iter().any(|a| a == account_id) {
                action_ids(account_id).into_iter().collect()
            } else {
                BTreeSet::new()
            };
            assert_eq!(
                ids(&scoped),
                expected_scoped,
                "case {row:?}: a scope previews an account's actions only while the caller \
                 reaches that account"
            );
        }
    }
}

/// A preview is what the rule it describes would act on, so the accounts a
/// scope previews are the accounts a rule may be scoped to: for every account,
/// an organization-scoped preview reports its inventory exactly when a
/// `CreateAutomationRule` carrying that same scope is served, and reports
/// nothing of it exactly when that write is refused.
#[tokio::test]
async fn a_scope_previews_only_the_accounts_a_rule_may_be_scoped_to() {
    let caller = state::caller_account_id();
    let router = opted_in_emulator("rule-scope").await;
    associate(&router, MEMBER_A, "arrange").await;
    associate(&router, MEMBER_B, "arrange").await;
    disassociate(&router, MEMBER_A, "arrange").await;

    // (case, the account the scope names, whether a rule may be scoped to it)
    let rows: [(&str, &str, bool); 3] = [
        ("removed-member", MEMBER_A, false),
        ("kept-member", MEMBER_B, true),
        ("caller-own-account", caller.as_str(), true),
    ];

    for (case, account_id, scopable) in rows {
        let previewed = walk(
            &router,
            "ListAutomationRulePreview",
            &preview_request(Some(account_id)),
            "previewResults",
            case,
        )
        .await;
        let created = call(
            &router,
            "CreateAutomationRule",
            &organization_rule(case, Some(&[account_id.to_string()])),
        )
        .await;

        if scopable {
            assert_eq!(
                ids(&previewed),
                action_ids(account_id).into_iter().collect::<BTreeSet<_>>(),
                "case {case:?}: the scope must preview that account's analysed actions"
            );
            let (status, body) = created;
            assert_eq!(
                status, 200,
                "case {case:?}: a rule may be scoped to an account whose actions are previewed, \
                 got {status} and {body}"
            );
        } else {
            assert!(
                previewed.is_empty(),
                "case {case:?}: a scope the caller may not create a rule for must preview \
                 nothing, got {:?}",
                ids(&previewed)
            );
            assert_access_denied(created, case);
        }
    }
}
