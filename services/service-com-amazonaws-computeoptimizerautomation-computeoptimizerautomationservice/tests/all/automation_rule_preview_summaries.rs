// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAutomationRulePreviewSummaries`.
//!
//! The rule under test: the summaries report the SAME selection
//! `ListAutomationRulePreview` reports one by one — the recommended actions this
//! service holds for the accounts the rule type and `organizationScope` cover,
//! restricted to the requested `recommendedActionTypes` and to the `criteria`
//! conditions — reduced to exactly ONE row, the account-wide aggregate of that
//! selection: the fixed aggregate key every recommended-action summary row of
//! this service carries, a `total.recommendedActionCount` equal to the number of
//! actions the unsummarised preview lists for the identical filter, and the sums
//! of their estimated monthly savings. A selection that holds nothing still
//! reports the row, with a count of zero.
//!
//! Every case drives the service the way a client does: an awsJson1_0 `POST /`
//! through the same router the emulator binary mounts, over an inventory seeded
//! where Compute Optimizer's own analysis would have written it (no operation of
//! this API mints a recommended action). So every expectation below is about what
//! the handler computes from persisted state.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    Criteria, EstimatedMonthlySavings, RecommendedAction,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    encode_page_token, preview_account_scope, preview_selection_fingerprint, service_state,
    PageCursor,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, unenrolled_emulator_with_engine,
};

/// The operation under test, and the sibling that lists the same selection one
/// action at a time.
const OPERATION: &str = "ListAutomationRulePreviewSummaries";
const PREVIEW: &str = "ListAutomationRulePreview";

/// The `key` every recommended-action summary row carries: fixed text naming the
/// account-wide aggregate of the requested selection, read off neither the
/// request nor the actions totalled.
const AGGREGATE_KEY: &str = "ALL_Recommended_Action_Summary";

/// The two values of the model's `RecommendedActionType`
/// (`smithy_query(query="shape",shape="RecommendedActionType")`), which is what
/// a preview request selects on.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// The one account every emulated caller presents.
fn caller_account() -> String {
    engine::emulator_account_id().to_string()
}

/// A second, member account id for the organization-scope rows: analysis output
/// about another account of the organization is seeded, never minted, so a
/// literal 12-digit id (the model's `AccountId` pattern) stands in for it.
const MEMBER_ACCOUNT: &str = "210987654321";

/// One row of seeded analysis output, with only the members a summary reads off
/// it: which account and type it belongs to, the region a `criteria` condition
/// selects on, and its estimate — or `None` for an action Compute Optimizer has
/// estimated no savings for.
///
/// The preview suite's own seed builder fixes the currency at USD and carries a
/// volume configuration no summary reads, so these rows are built here: the
/// aggregation cases below vary exactly the two members
/// (`currency`, `savingsEstimationMode`) that one holds constant.
struct Action<'a> {
    id: &'a str,
    account: String,
    action_type: &'a str,
    region: &'a str,
    /// `(beforeDiscountSavings, afterDiscountSavings, currency, savingsEstimationMode)`.
    savings: Estimate<'a>,
}

/// What one seeded action's `estimatedMonthlySavings` carries, or `None` for an
/// action Compute Optimizer has estimated no savings for.
type Estimate<'a> = Option<(f64, f64, &'a str, &'a str)>;

/// One row of the aggregation table: the case, the estimates of the selected
/// actions, and the total the rule says the aggregate row reports.
type AggregationCase<'a> = (&'a str, Vec<Estimate<'a>>, Summary);

impl Action<'_> {
    fn stored(&self) -> RecommendedAction {
        let resource_id = format!("vol-{}", self.id.to_ascii_lowercase());
        RecommendedAction {
            recommended_action_id: Some(self.id.to_string()),
            resource_arn: Some(format!(
                "arn:aws:ec2:{}:{}:volume/{resource_id}",
                self.region, self.account
            )),
            resource_id: Some(resource_id),
            account_id: Some(self.account.clone()),
            region: Some(self.region.to_string()),
            resource_type: Some("EbsVolume".to_string()),
            look_back_period_in_days: Some(14),
            recommended_action_type: Some(self.action_type.to_string()),
            current_resource_summary: None,
            current_resource_details: None,
            recommended_resource_summary: None,
            recommended_resource_details: None,
            restart_needed: Some(false),
            estimated_monthly_savings: self.savings.map(|(before, after, currency, mode)| {
                EstimatedMonthlySavings {
                    currency: currency.to_string(),
                    before_discount_savings: before,
                    after_discount_savings: after,
                    savings_estimation_mode: mode.to_string(),
                }
            }),
            resource_tags: Some(Vec::new()),
        }
    }
}

/// The inventory the selection cases report over: three `UpgradeEbsVolumeType`
/// actions in the calling account (one of them with no estimate at all), two
/// `SnapshotAndDeleteUnattachedEbsVolume` ones, and one upgrade in a member
/// account — spanning three regions, so a `criteria` condition cuts across the
/// action types. Every figure is a multiple of 0.25, so each expected sum below
/// is exact in binary floating point.
///
/// `recommendedActionId` is the model's `^[0-9A-Za-z]{16}$`.
fn inventory() -> Vec<RecommendedAction> {
    [
        Action {
            id: "SA00000000000001",
            account: caller_account(),
            action_type: UPGRADE,
            region: "us-east-1",
            savings: Some((10.0, 6.0, "USD", "BeforeDiscount")),
        },
        Action {
            id: "SA00000000000002",
            account: caller_account(),
            action_type: SNAPSHOT_AND_DELETE,
            region: "us-east-1",
            savings: Some((100.0, 90.0, "USD", "AfterDiscount")),
        },
        Action {
            id: "SA00000000000003",
            account: caller_account(),
            action_type: UPGRADE,
            region: "eu-west-1",
            savings: Some((2.5, 1.25, "USD", "BeforeDiscount")),
        },
        Action {
            id: "SA00000000000004",
            account: caller_account(),
            action_type: UPGRADE,
            region: "ap-southeast-2",
            savings: None,
        },
        Action {
            id: "SA00000000000005",
            account: caller_account(),
            action_type: SNAPSHOT_AND_DELETE,
            region: "us-east-1",
            savings: Some((5.0, 4.0, "USD", "AfterDiscount")),
        },
        Action {
            id: "SA00000000000006",
            account: MEMBER_ACCOUNT.to_string(),
            action_type: UPGRADE,
            region: "us-east-1",
            savings: Some((7.0, 3.0, "USD", "BeforeDiscount")),
        },
    ]
    .iter()
    .map(Action::stored)
    .collect()
}

/// The crate's awsJson1_0 harness with the caller enrolled and the analysis
/// inventory holding `actions`. The engine comes back too, so a case can check
/// the store after a call.
fn emulator_with(actions: &[RecommendedAction]) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    seed_inventory(&engine, actions);
    (router, engine)
}

/// Record `actions` where Compute Optimizer's analysis writes them, through the
/// one fixture that records an inventory for this crate
/// (`crate::recommended_actions::record`), so a member account's analysis output
/// is seeded with the association that put it there.
fn seed_inventory(engine: &Arc<engine::InMemoryEngine>, actions: &[RecommendedAction]) {
    crate::recommended_actions::record(engine, actions.to_vec());
}

/// Every stored recommended action, rendered: a read that changed any field of
/// any action in place would change this string.
fn inventory_snapshot(engine: &engine::InMemoryEngine) -> String {
    let state = service_state(engine).expect("the engine carries this service's state");
    let snapshot = format!(
        "{:?}",
        state
            .lock()
            .recommended_actions
            .iter_in_order()
            .collect::<Vec<_>>()
    );
    snapshot
}

/// One summary as a response reports it: the row's key, its count and its
/// aggregated estimate.
#[derive(Debug, Clone, PartialEq)]
struct Summary {
    key: String,
    count: i64,
    before: f64,
    after: f64,
    currency: String,
    mode: String,
}

/// The summaries a response lists, in response order.
fn summaries(body: &Value) -> Vec<Summary> {
    body.get("previewResultSummaries")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| {
            let total = summary.get("total").unwrap_or(&Value::Null);
            let savings = total.get("estimatedMonthlySavings").unwrap_or(&Value::Null);
            Summary {
                key: summary
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                count: total
                    .get("recommendedActionCount")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1),
                before: savings
                    .get("beforeDiscountSavings")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                after: savings
                    .get("afterDiscountSavings")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                currency: savings
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                mode: savings
                    .get("savingsEstimationMode")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        })
        .collect()
}

/// The aggregate row a case expects, written as a call so a table row stays
/// readable. Every row carries the one fixed key, so only the totals are spelled
/// out.
fn aggregate(count: i64, before: f64, after: f64, currency: &str, mode: &str) -> Summary {
    Summary {
        key: AGGREGATE_KEY.to_string(),
        count,
        before,
        after,
        currency: currency.to_string(),
        mode: mode.to_string(),
    }
}

/// Assert that a response carries exactly the ONE aggregate row `expected`,
/// naming the case and the actual values. The figures are summed floating point,
/// so they are compared within a tolerance far below the smallest amount any row
/// carries.
fn assert_aggregate_row(actual: &[Summary], expected: &Summary, case: &str) {
    assert_eq!(
        actual.len(),
        1,
        "case {case:?}: expected exactly one aggregate row {expected:?}, got {actual:?}"
    );
    let actual = &actual[0];
    assert_eq!(
        actual.key, AGGREGATE_KEY,
        "case {case:?}: expected the fixed aggregate key {AGGREGATE_KEY:?}, got {actual:?}"
    );
    assert_eq!(
        actual.count, expected.count,
        "case {case:?}: expected {} actions totalled, got {actual:?}",
        expected.count
    );
    assert!(
        (actual.before - expected.before).abs() < 1e-9,
        "case {case:?}: expected beforeDiscountSavings {}, got {actual:?}",
        expected.before
    );
    assert!(
        (actual.after - expected.after).abs() < 1e-9,
        "case {case:?}: expected afterDiscountSavings {}, got {actual:?}",
        expected.after
    );
    assert_eq!(
        actual.currency, expected.currency,
        "case {case:?}: expected currency {:?}, got {actual:?}",
        expected.currency
    );
    assert_eq!(
        actual.mode, expected.mode,
        "case {case:?}: expected savingsEstimationMode {:?}, got {actual:?}",
        expected.mode
    );
}

/// The actions the unsummarised preview lists for `request`, which is the oracle
/// the aggregate's count has to reconcile with.
async fn previewed_actions(router: &MultiServiceRouter, request: &Value, case: &str) -> Vec<Value> {
    let body = call_ok(router, PREVIEW, request, case).await;
    body.get("previewResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A figure one previewed action carries under `estimatedMonthlySavings`, or
/// zero for an action Compute Optimizer estimated no savings for.
fn figure(action: &Value, name: &str) -> f64 {
    action
        .get("estimatedMonthlySavings")
        .and_then(|estimate| estimate.get(name))
        .and_then(Value::as_f64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The aggregate row
// ---------------------------------------------------------------------------

/// The selection a request describes, totalled into ONE row: the fixed aggregate
/// key, the number of actions the selection holds and the sums of their
/// estimates. The rows differ along what the rule generalises over: how many
/// action types the selection admits, the account scope it covers, the criteria
/// it constrains with, and whether it holds any action at all.
#[tokio::test]
async fn the_preview_summary_is_one_aggregate_row_per_selection() {
    let (router, _engine) = emulator_with(&inventory());

    // (case, request, the aggregate row the rule says comes back)
    let cases: Vec<(&str, Value, Summary)> = vec![
        (
            "both action types, no criteria: every action of the calling account",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            // The upgrade with no estimate is one of the five actions and adds
            // nothing to either sum; the estimates disagree on the mode, so the
            // total reports the conservative one.
            aggregate(5, 117.5, 101.25, "USD", "BeforeDiscount"),
        ),
        (
            "one action type: the other type is not selected, so it is not counted",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [UPGRADE] }),
            aggregate(3, 12.5, 7.25, "USD", "BeforeDiscount"),
        ),
        (
            "the other action type alone",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [SNAPSHOT_AND_DELETE] }),
            aggregate(2, 105.0, 94.0, "USD", "AfterDiscount"),
        ),
        (
            "a criteria condition cuts across both action types",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }]
                },
            }),
            aggregate(3, 115.0, 100.0, "USD", "BeforeDiscount"),
        ),
        (
            "an organization preview with no scope spans every account the store holds \
             actions for",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            aggregate(6, 124.5, 104.25, "USD", "BeforeDiscount"),
        ),
        (
            "an organization scope naming only the member account",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
            aggregate(1, 7.0, 3.0, "USD", "BeforeDiscount"),
        ),
        (
            // The row a caller sizing a rule's impact reads before activating
            // it: a selection that holds nothing still has a total, which is
            // zero.
            "criteria nothing satisfies: the row stands with a count of zero",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["sa-east-1"] }]
                },
            }),
            aggregate(0, 0.0, 0.0, "", "BeforeDiscount"),
        ),
        (
            "a criteria condition on the estimate itself",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "estimatedMonthlySavings": [
                        { "comparison": "NumericGreaterThanEquals", "values": [10.0] }
                    ]
                },
            }),
            aggregate(2, 110.0, 96.0, "USD", "BeforeDiscount"),
        ),
    ];

    for (case, request, expected) in cases {
        let body = call_ok(&router, OPERATION, &request, case).await;
        assert_aggregate_row(&summaries(&body), &expected, case);
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: one aggregate row fits any page, so no token is issued, got {body}"
        );

        // The cross-check the preview flow exists for: the row's count is the
        // number of actions the unsummarised preview lists for the identical
        // filter.
        let previewed = previewed_actions(&router, &request, case).await;
        assert_eq!(
            expected.count,
            i64::try_from(previewed.len()).expect("a small fixture set"),
            "case {case:?}: the aggregate count must equal the {} actions the preview lists",
            previewed.len()
        );
    }
}

/// The aggregated estimate of the row: the sums of the before- and
/// after-discount figures the selected actions' estimates carry, with the
/// currency they agree on and the mode they agree on — and, where they agree on
/// neither, the free-form `currency` naming none while the enum-typed
/// `savingsEstimationMode` falls back to the conservative before-discount
/// reading. A stored mode the model does not declare reads the same way: a total
/// can only name a value of `SavingsEstimationMode`.
#[tokio::test]
async fn the_row_total_aggregates_the_estimates_of_the_selected_actions() {
    // (case, the selection's estimates, the total the rule says it reports)
    let cases: Vec<AggregationCase> = vec![
        (
            "one estimate: the total is that estimate",
            vec![Some((10.0, 6.0, "USD", "BeforeDiscount"))],
            aggregate(1, 10.0, 6.0, "USD", "BeforeDiscount"),
        ),
        (
            "estimates agreeing on currency and mode",
            vec![
                Some((10.0, 6.0, "USD", "AfterDiscount")),
                Some((0.25, 0.125, "USD", "AfterDiscount")),
                Some((4.0, 2.0, "USD", "AfterDiscount")),
            ],
            aggregate(3, 14.25, 8.125, "USD", "AfterDiscount"),
        ),
        (
            "an action with no estimate: counted, adding nothing",
            vec![Some((10.0, 6.0, "USD", "BeforeDiscount")), None],
            aggregate(2, 10.0, 6.0, "USD", "BeforeDiscount"),
        ),
        (
            "no selected action carries an estimate: the count stands, the sums are \
             zero and no currency is named",
            vec![None, None],
            aggregate(2, 0.0, 0.0, "", "BeforeDiscount"),
        ),
        (
            "estimates disagreeing on mode: the before-discount reading",
            vec![
                Some((10.0, 6.0, "USD", "BeforeDiscount")),
                Some((2.0, 1.0, "USD", "AfterDiscount")),
            ],
            aggregate(2, 12.0, 7.0, "USD", "BeforeDiscount"),
        ),
        (
            "estimates disagreeing on currency: the sums are not one currency's total, so \
             the total names no currency",
            vec![
                Some((10.0, 6.0, "USD", "BeforeDiscount")),
                Some((2.0, 1.0, "EUR", "BeforeDiscount")),
            ],
            aggregate(2, 12.0, 7.0, "", "BeforeDiscount"),
        ),
        (
            "a currency that is not the dollar, agreed on",
            vec![
                Some((10.0, 6.0, "JPY", "AfterDiscount")),
                Some((2.0, 1.0, "JPY", "AfterDiscount")),
            ],
            aggregate(2, 12.0, 7.0, "JPY", "AfterDiscount"),
        ),
        (
            "a stored mode the model's SavingsEstimationMode does not declare: the total \
             must name one of the two it does, so the conservative reading stands",
            vec![Some((10.0, 6.0, "USD", "NotAModeTheModelDeclares"))],
            aggregate(1, 10.0, 6.0, "USD", "BeforeDiscount"),
        ),
    ];

    for (case, estimates, expected) in cases {
        // One fresh store per row: the row IS the whole inventory.
        let actions: Vec<RecommendedAction> = estimates
            .iter()
            .enumerate()
            .map(|(index, savings)| {
                Action {
                    id: &format!("SB0000000000000{index}"),
                    account: caller_account(),
                    action_type: UPGRADE,
                    region: "us-east-1",
                    savings: *savings,
                }
                .stored()
            })
            .collect();
        let (router, _engine) = emulator_with(&actions);

        let body = call_ok(
            &router,
            OPERATION,
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            case,
        )
        .await;
        assert_aggregate_row(&summaries(&body), &expected, case);
    }
}

/// The row totals exactly the actions the unsummarised preview lists for the
/// same request: the same count and the same savings sums, whichever selection
/// is asked for. The two operations read one selection.
#[tokio::test]
async fn the_row_totals_the_actions_the_unsummarised_preview_lists() {
    let (router, _engine) = emulator_with(&inventory());

    let requests: Vec<(&str, Value)> = vec![
        (
            "both action types over the calling account",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
        ),
        (
            "one action type",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [SNAPSHOT_AND_DELETE] }),
        ),
        (
            "the whole organization",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
        ),
        (
            "a scoped organization preview",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT, &caller_account()] },
            }),
        ),
        (
            "a criteria-restricted preview",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringLike", "values": ["us-*"] }]
                },
            }),
        ),
        (
            "a preview nothing satisfies",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["sa-east-1"] }]
                },
            }),
        ),
    ];

    for (case, request) in requests {
        let results = previewed_actions(&router, &request, case).await;
        let count = i64::try_from(results.len()).expect("a small fixture set");
        let before: f64 = results
            .iter()
            .map(|result| figure(result, "beforeDiscountSavings"))
            .sum();
        let after: f64 = results
            .iter()
            .map(|result| figure(result, "afterDiscountSavings"))
            .sum();

        let summarised = call_ok(&router, OPERATION, &request, case).await;
        let actual = summaries(&summarised);
        // How the row names a currency and a savings mode is the aggregation
        // table's business, so this cross-check carries the reported ones over
        // and asserts the count and the sums.
        let currency = actual.first().map(|row| row.currency.clone()).unwrap_or_default();
        let mode = actual.first().map(|row| row.mode.clone()).unwrap_or_default();
        assert_aggregate_row(
            &actual,
            &aggregate(count, before, after, &currency, &mode),
            case,
        );
    }
}

/// The summaries are a REPORT: the call leaves the inventory exactly as it was,
/// creates no rule, and answers the same thing when asked twice.
#[tokio::test]
async fn summarising_writes_nothing() {
    let (router, engine) = emulator_with(&inventory());
    let case = "a summary over a seeded inventory";
    let before = inventory_snapshot(&engine);

    let request = json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        "maxResults": 1,
    });
    let first = call_ok(&router, OPERATION, &request, case).await;
    let again = call_ok(&router, OPERATION, &request, case).await;

    assert_eq!(
        summaries(&first),
        summaries(&again),
        "case {case:?}: two identical reads must report the same row, got {first} then {again}"
    );
    assert_eq!(
        inventory_snapshot(&engine),
        before,
        "case {case:?}: a read must leave every stored action untouched"
    );
    let listed = call_ok(&router, "ListAutomationRules", &json!({}), case).await;
    assert_eq!(
        listed
            .get("automationRules")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case {case:?}: a preview summary creates no rule, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// The listing reports ONE row, so every page size serves that row and no walk
/// is ever cut: whatever `maxResults` asks for, the answer is the aggregate and
/// no token.
#[tokio::test]
async fn the_aggregate_row_fits_every_page_size_and_mints_no_token() {
    let (router, _engine) = emulator_with(&inventory());
    let request = json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
    });
    let unpaged = call_ok(&router, OPERATION, &request, "one unpaged page").await;
    let all = summaries(&unpaged);
    assert_aggregate_row(
        &all,
        &aggregate(6, 124.5, 104.25, "USD", "BeforeDiscount"),
        "one unpaged page",
    );

    // Every page size from the smallest the model admits to its maximum.
    for page_size in [1_u64, 2, 3, 100] {
        let case = format!("maxResults={page_size}");
        let mut body = request.clone();
        body["maxResults"] = json!(page_size);
        let response = call_ok(&router, OPERATION, &body, &case).await;
        assert_eq!(
            summaries(&response),
            all,
            "case {case:?}: every page size serves the one aggregate row, got {response}"
        );
        assert!(
            response.get("nextToken").is_none(),
            "case {case:?}: nothing remains after the only row, so no token is carried, got \
             {response}"
        );
    }
}

/// A `nextToken` is accepted only where this listing could have minted it: for
/// the same operation and the same derived selection, naming a position the row
/// sequence reaches. Anything else — a cursor taken for another selection, the
/// sibling preview's token, one this service never issued, or one naming a
/// position past the only row — is refused rather than answered with a page of
/// something else.
#[tokio::test]
async fn a_token_resumes_only_the_selection_it_was_taken_with() {
    let (router, _engine) = emulator_with(&inventory());
    let selection = json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
    });

    // The cursor this listing mints for that selection, built from the service's
    // own fingerprint of it: one aggregate row leaves no page to resume, so the
    // only position a cursor can name is the END of the row sequence — where a
    // walk whose tail was removed resumes.
    let account_scope = preview_account_scope("AccountRule", None);
    let both_types = vec![UPGRADE.to_string(), SNAPSHOT_AND_DELETE.to_string()];
    let cursor = |offset: usize, operation: &str| {
        encode_page_token(&PageCursor {
            offset,
            filter_fingerprint: preview_selection_fingerprint(
                operation,
                &account_scope,
                &both_types,
                &Criteria::default(),
            ),
        })
    };

    let mut resumed_request = selection.clone();
    resumed_request["nextToken"] = json!(cursor(1, OPERATION));
    let resumed = call_ok(
        &router,
        OPERATION,
        &resumed_request,
        "the cursor resumed with its own selection",
    )
    .await;
    assert!(
        summaries(&resumed).is_empty() && resumed.get("nextToken").is_none(),
        "case \"the cursor resumed with its own selection\": the remainder after the only row is \
         empty and ends the walk, got {resumed}"
    );

    // The token the UNSUMMARISED preview mints for the same request, whose pages
    // are single actions rather than one aggregate.
    let other_operation_token = call_ok(
        &router,
        PREVIEW,
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            "maxResults": 1,
        }),
        "the unsummarised preview's first page",
    )
    .await
    .get("nextToken")
    .and_then(Value::as_str)
    .expect("a five-action selection paged at one action has a next page")
    .to_string();

    let cases: Vec<(&str, Value)> = vec![
        (
            "a cursor presented with different action types",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "nextToken": cursor(1, OPERATION),
            }),
        ),
        (
            "a cursor presented with different criteria",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }]
                },
                "nextToken": cursor(1, OPERATION),
            }),
        ),
        (
            "a cursor presented with a different rule type",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": cursor(1, OPERATION),
            }),
        ),
        (
            "a position the row sequence never reached",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": cursor(2, OPERATION),
            }),
        ),
        (
            "the unsummarised preview's token",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": other_operation_token.clone(),
            }),
        ),
        (
            "a token this service never issued",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": "notAtokenThisServiceIssued",
            }),
        ),
    ];
    for (case, request) in cases {
        assert_refused(
            call(&router, OPERATION, &request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }

    // And the other way round: a cursor fingerprinted for the summaries is not a
    // token of the unsummarised preview, which pages a different sequence.
    assert_refused(
        call(
            &router,
            PREVIEW,
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": cursor(1, OPERATION),
            }),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "the summaries cursor on the unsummarised preview",
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A request the model admits but this service cannot evaluate is refused with
/// the operation's only client-input error — the same reading its unsummarised
/// sibling takes, so one request cannot be valid for a preview and invalid for
/// that preview's totals.
#[tokio::test]
async fn a_request_the_service_cannot_evaluate_is_refused() {
    let (router, engine) = emulator_with(&inventory());

    let cases: Vec<(&str, Value)> = vec![
        (
            "no action type to include",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [] }),
        ),
        (
            "an organization scope on an account-scoped preview",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
        ),
        (
            "a numeric comparison on a string criterion",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": {
                    "region": [{ "comparison": "NumericGreaterThan", "values": ["us-east-1"] }]
                },
            }),
        ),
        (
            "a string comparison on a double criterion",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": {
                    "estimatedMonthlySavings": [{ "comparison": "StringLike", "values": [1.0] }]
                },
            }),
        ),
        (
            "a condition with no comparison operator",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": { "region": [{ "values": ["us-east-1"] }] },
            }),
        ),
        (
            "a condition with no values",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": { "region": [{ "comparison": "StringEquals", "values": [] }] },
            }),
        ),
        (
            "a resourceTag condition that names no tag key",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": {
                    "resourceTag": [{ "comparison": "StringEquals", "values": ["Production"] }]
                },
            }),
        ),
    ];
    for (case, request) in &cases {
        assert_refused(
            call(&router, OPERATION, request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
        // The refused request is refused by the unsummarised preview too.
        assert_refused(
            call(&router, PREVIEW, request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }

    // Nothing was written while those requests were refused.
    let snapshot = inventory_snapshot(&engine);
    assert!(
        snapshot.contains("SA00000000000001"),
        "a refused request must leave the inventory in place, got {snapshot}"
    );
}

/// `maxResults` is modelled `range(min: 1, max: 100)`, so each published bound
/// is accepted and a value one past either is refused at the protocol boundary.
#[tokio::test]
async fn max_results_honours_its_modelled_bounds() {
    let (router, _engine) = emulator_with(&inventory());
    let request = |max_results: i64| {
        json!({
            "ruleType": "OrganizationRule",
            "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            "maxResults": max_results,
        })
    };

    for max_results in [1, 100] {
        let case = format!("maxResults={max_results} is a published bound");
        let body = call_ok(&router, OPERATION, &request(max_results), &case).await;
        let page = summaries(&body);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: the one aggregate row fits a page of {max_results}, got {page:?}"
        );
    }

    for max_results in [0, 101] {
        let case = format!("maxResults={max_results} is one past a published bound");
        let (status, body) = call(&router, OPERATION, &request(max_results)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a maxResults outside range(min: 1, max: 100) is refused, got \
             {status} and {body}"
        );
    }
}

/// The service-wide enrollment gate stands in front of this operation too: an
/// account that is not opted in is refused the call rather than handed an empty
/// summary.
#[tokio::test]
async fn an_unenrolled_account_is_refused() {
    let (router, engine) = unenrolled_emulator_with_engine();
    seed_inventory(&engine, &inventory());

    assert_refused(
        call(
            &router,
            OPERATION,
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
        )
        .await,
        403,
        "OptInRequiredException",
        "an account that never opted in",
    );

    // Opting in over the wire opens the gate, and the summary then reports the
    // inventory that was there all along. Opting in is also what records the
    // account's OWN analysed inventory (see
    // `crate::recommended_action_inventory`), so the request names the volumes
    // this suite seeded — every one of them, and nothing else — to total exactly
    // the rows the expectation below is written over.
    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active"}),
        "opt in",
    )
    .await;
    let body = call_ok(
        &router,
        OPERATION,
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            "criteria": {"resourceArn": [{
                "comparison": "StringLike",
                "values": ["arn:aws:ec2:*:*:volume/vol-sa*"],
            }]},
        }),
        "opted in",
    )
    .await;
    assert_aggregate_row(
        &summaries(&body),
        &aggregate(5, 117.5, 101.25, "USD", "BeforeDiscount"),
        "opted in",
    );
}
