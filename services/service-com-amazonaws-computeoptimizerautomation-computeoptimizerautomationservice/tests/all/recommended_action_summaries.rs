// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRecommendedActionSummaries`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0
//! `POST /` through the same [`MultiServiceRouter`] the emulator binary mounts —
//! so a response that is not computed from persisted state fails here.
//!
//! The rule under test: a recommended-action summary listing reports exactly ONE
//! row for the requested selection — the ACCOUNT-WIDE aggregate — keyed by the
//! fixed text every such row carries, whose `total.recommendedActionCount` is the
//! number of actions `ListRecommendedActions` reports for the same unpaginated
//! filter and whose `total.estimatedMonthlySavings` sums those actions'
//! estimates. The row is reported even for a selection that holds nothing, with a
//! count of zero, and its key is fixed text: it is read off neither the request
//! nor the actions summarised.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EstimatedMonthlySavings, RecommendedAction, RecommendedActionFilter,
    RECOMMENDED_ACTION_FILTER_NAME_VALUES, RECOMMENDED_ACTION_TYPE_VALUES,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    encode_page_token, page_fingerprint, PageCursor, DEFAULT_MAX_RESULTS,
};

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, member, unenrolled_emulator_with_engine,
};
use crate::recommended_actions::{record, stored, stored_count, MEMBER_ACCOUNT_ID};

/// The operation under test.
const OPERATION: &str = "ListRecommendedActionSummaries";

/// The `key` every recommended-action summary row carries: fixed text naming the
/// account-wide aggregate of the requested selection, rather than a value read
/// off the request or off the actions totalled. It is the same literal whatever
/// the filter and whatever the store holds, which the cases below drive it over.
const AGGREGATE_KEY: &str = "ALL_Recommended_Action_Summary";

/// The account every emulated caller presents.
const CALLER_ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// A third account, so a case can span more than two owners.
const THIRD_ACCOUNT_ID: &str = "310987654321";

/// The two modelled `RecommendedActionType` values, named so a case reads.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// One `EstimatedMonthlySavings`, as Compute Optimizer's analysis records it.
fn savings(
    currency: &str,
    before_discount: f64,
    after_discount: f64,
    mode: &str,
) -> EstimatedMonthlySavings {
    EstimatedMonthlySavings {
        currency: currency.to_string(),
        before_discount_savings: before_discount,
        after_discount_savings: after_discount,
        savings_estimation_mode: mode.to_string(),
    }
}

/// One recorded action, owned by `account_id`, of `recommended_action_type`,
/// carrying `estimate`.
///
/// Built on the one recommended-action fixture builder this crate has, with the
/// three members these cases vary overridden: the owning account, the action
/// type, and the estimate the aggregate sums. An action the store records NO
/// type for is written by passing `None`.
fn action(
    index: usize,
    account_id: Option<&str>,
    recommended_action_type: Option<&str>,
    estimate: Option<EstimatedMonthlySavings>,
) -> RecommendedAction {
    RecommendedAction {
        account_id: account_id.map(ToString::to_string),
        recommended_action_type: recommended_action_type.map(ToString::to_string),
        estimated_monthly_savings: estimate,
        ..stored(
            index,
            &format!("vol-{index:04}"),
            account_id.unwrap_or(CALLER_ACCOUNT_ID),
            "us-east-1",
            14,
            recommended_action_type.unwrap_or(UPGRADE),
            Some("gp2"),
            false,
            &[("env", "prod")],
        )
    }
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

/// The aggregate row a case expects, written as a call so a table row stays
/// readable. Every row of every table carries the one fixed key, so only the
/// totals are spelled out.
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

/// The summaries one response reports, in response order. A response that
/// carries no summaries at all reads as the empty list, whether it renders the
/// member as `[]` or leaves it out.
fn reported(body: &Value) -> Vec<Summary> {
    let listed = member(body, "recommendedActionSummaries");
    if listed.is_null() {
        return Vec::new();
    }
    listed
        .as_array()
        .unwrap_or_else(|| panic!("recommendedActionSummaries is a list, got {body}"))
        .iter()
        .map(|summary| {
            let total = summary.get("total").unwrap_or(&Value::Null);
            let estimate = total.get("estimatedMonthlySavings").unwrap_or(&Value::Null);
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
                before: estimate
                    .get("beforeDiscountSavings")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                after: estimate
                    .get("afterDiscountSavings")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                currency: estimate
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                mode: estimate
                    .get("savingsEstimationMode")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        })
        .collect()
}

/// Assert that a response carries exactly the ONE aggregate row `expected`,
/// naming the case and the actual values. The figures are summed floating point,
/// so they are compared within a tolerance far below the smallest amount any row
/// carries.
fn assert_aggregate_row(case: &str, actual: &[Summary], expected: &Summary) {
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

/// The actions `ListRecommendedActions` reports for `request`, which is the
/// oracle the aggregate's count has to reconcile with.
async fn listed_actions(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: &Value,
    case: &str,
) -> Vec<Value> {
    let body = call_ok(router, "ListRecommendedActions", request, case).await;
    let listed = member(&body, "recommendedActions");
    if listed.is_null() {
        return Vec::new();
    }
    listed
        .as_array()
        .unwrap_or_else(|| panic!("recommendedActions is a list, got {body}"))
        .clone()
}

/// A figure one listed action carries under `estimatedMonthlySavings`, or zero
/// for an action Compute Optimizer estimated no savings for.
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

/// A table spanning what the rule generalises over: how many actions the store
/// holds, how many distinct types they carry and in which order, which accounts
/// own them, whether they carry an estimate at all, and whether the estimates
/// agree on a currency and a savings mode. Whatever the row, the answer is ONE
/// aggregate row keyed by the fixed text, counting every action of the
/// selection.
#[tokio::test]
async fn the_summary_is_one_account_wide_aggregate_row() {
    let scenarios: Vec<(&str, Vec<RecommendedAction>, Summary)> = vec![
        // Nothing has been analysed yet. The account still has a total, which is
        // zero: a caller reading the account-wide count finds the row rather
        // than no row at all.
        (
            "an-empty-store",
            vec![],
            aggregate(0, 0.0, 0.0, "", "BeforeDiscount"),
        ),
        (
            "one-action",
            vec![action(
                1,
                Some(CALLER_ACCOUNT_ID),
                Some(UPGRADE),
                Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
            )],
            aggregate(1, 12.5, 9.75, "USD", "AfterDiscount"),
        ),
        (
            "actions-of-one-type-are-summed",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 20.0, 18.0, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 0.25, 0.125, "AfterDiscount")),
                ),
            ],
            aggregate(3, 32.75, 27.875, "USD", "AfterDiscount"),
        ),
        (
            // Both modelled action types, which the aggregate does NOT break
            // apart: the row totals the account, not the type.
            "both-modelled-types-total-into-one-row",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("USD", 7.0, 6.5, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 2.5, 2.25, "AfterDiscount")),
                ),
            ],
            aggregate(3, 22.0, 18.5, "USD", "AfterDiscount"),
        ),
        (
            // Same actions, recorded the other way round: the row is an
            // aggregate, so the record order cannot change it.
            "the-row-does-not-depend-on-the-record-order",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("USD", 7.0, 6.5, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 2.5, 2.25, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
            ],
            aggregate(3, 22.0, 18.5, "USD", "AfterDiscount"),
        ),
        (
            // The row spans every account the caller retrieves from: it is the
            // account-wide total, including each associated member account's
            // analysis output.
            "every-owning-account-in-one-row",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(MEMBER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 7.0, 6.5, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(THIRD_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("USD", 2.5, 2.25, "AfterDiscount")),
                ),
            ],
            aggregate(3, 22.0, 18.5, "USD", "AfterDiscount"),
        ),
        (
            "an-action-the-store-holds-no-account-for-is-still-counted",
            vec![
                action(
                    1,
                    None,
                    Some(UPGRADE),
                    Some(savings("USD", 1.0, 0.5, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
            ],
            aggregate(2, 13.5, 10.25, "USD", "AfterDiscount"),
        ),
        (
            // The row is not keyed by the action type, so an action carrying no
            // type is one action of the account's total like any other.
            "an-action-the-store-holds-no-type-for-is-still-counted",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    None,
                    Some(savings("USD", 99.0, 98.0, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
            ],
            aggregate(2, 111.5, 107.75, "USD", "AfterDiscount"),
        ),
        (
            "an-action-without-an-estimate-is-counted-with-no-savings",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(2, Some(CALLER_ACCOUNT_ID), Some(UPGRADE), None),
            ],
            aggregate(2, 12.5, 9.75, "USD", "AfterDiscount"),
        ),
        (
            // No recorded estimate names a currency, so the row names none and
            // every sum is zero; the mode is the conservative reading.
            "no-stored-action-carries-an-estimate",
            vec![
                action(1, Some(CALLER_ACCOUNT_ID), Some(UPGRADE), None),
                action(2, Some(CALLER_ACCOUNT_ID), Some(SNAPSHOT_AND_DELETE), None),
            ],
            aggregate(2, 0.0, 0.0, "", "BeforeDiscount"),
        ),
        (
            // Agreement is judged over the whole aggregate now that it is one
            // row: estimates that disagree on the mode report the conservative
            // one.
            "estimates-disagreeing-on-the-mode",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 5.0, 4.0, "BeforeDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("USD", 1.0, 0.75, "AfterDiscount")),
                ),
            ],
            aggregate(3, 18.5, 14.5, "USD", "BeforeDiscount"),
        ),
        (
            "estimates-agreeing-on-before-discount",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "BeforeDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 5.0, 4.0, "BeforeDiscount")),
                ),
            ],
            aggregate(2, 17.5, 13.75, "USD", "BeforeDiscount"),
        ),
        (
            // The emulator converts nothing, so a total whose estimates are not
            // in one currency names no currency while still summing both
            // figures.
            "estimates-disagreeing-on-the-currency",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("EUR", 10.0, 8.0, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 1.0, 0.5, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("EUR", 4.0, 3.0, "AfterDiscount")),
                ),
            ],
            aggregate(3, 15.0, 11.5, "", "AfterDiscount"),
        ),
        (
            "a-currency-that-is-not-the-dollar-agreed-on",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("JPY", 10.0, 6.0, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("JPY", 2.0, 1.0, "AfterDiscount")),
                ),
            ],
            aggregate(2, 12.0, 7.0, "JPY", "AfterDiscount"),
        ),
    ];

    // The seeded inventories span the whole modelled `RecommendedActionType`
    // value set, so a type added to the model cannot be left untested: whatever
    // type an action carries, it is one action of the account's total.
    let seeded: BTreeSet<String> = scenarios
        .iter()
        .flat_map(|(_, actions, _)| {
            actions
                .iter()
                .filter_map(|action| action.recommended_action_type.clone())
        })
        .collect();
    let modelled: BTreeSet<String> = RECOMMENDED_ACTION_TYPE_VALUES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        seeded, modelled,
        "every modelled RecommendedActionType must be recorded somewhere in the table"
    );

    for (case, actions, expected) in &scenarios {
        let (router, engine) = emulator_with_engine();
        record(&engine, actions.clone());
        let recorded = stored_count(&engine);

        let listed = call_ok(&router, OPERATION, &json!({}), case).await;
        assert_aggregate_row(case, &reported(&listed), expected);
        assert!(
            listed.get("nextToken").is_none(),
            "case {case:?}: one aggregate row fits any page, so no token is issued: {listed}"
        );

        // The reconciliation the aggregate exists for: its count is the number
        // of actions the unpaginated listing reports for the same (absent)
        // filter.
        let actions_listed = listed_actions(&router, &json!({}), case).await;
        assert_eq!(
            expected.count,
            i64::try_from(actions_listed.len()).expect("a small fixture set"),
            "case {case:?}: the aggregate count must equal the {} actions the listing reports",
            actions_listed.len()
        );

        // Summarising is a read: it records nothing, and a second call answers
        // exactly the same.
        assert_eq!(
            stored_count(&engine),
            recorded,
            "case {case:?}: summarising must not change the store"
        );
        let again = call_ok(&router, OPERATION, &json!({}), case).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a repeated read must answer the same, got {again} then {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// The actions the filter cases are taken over, in record order: three
/// accounts, both action types, one action with no estimate.
fn fixtures() -> Vec<RecommendedAction> {
    vec![
        RecommendedAction {
            account_id: Some(CALLER_ACCOUNT_ID.to_string()),
            estimated_monthly_savings: Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
            ..stored(
                1,
                "vol-aaa1",
                CALLER_ACCOUNT_ID,
                "us-east-1",
                14,
                UPGRADE,
                Some("gp2"),
                false,
                &[("env", "prod")],
            )
        },
        RecommendedAction {
            account_id: Some(MEMBER_ACCOUNT_ID.to_string()),
            estimated_monthly_savings: Some(savings("USD", 20.0, 18.0, "AfterDiscount")),
            ..stored(
                2,
                "vol-bbb2",
                MEMBER_ACCOUNT_ID,
                "us-west-2",
                32,
                SNAPSHOT_AND_DELETE,
                Some("gp2"),
                true,
                &[("env", "test"), ("team", "infra")],
            )
        },
        RecommendedAction {
            account_id: Some(CALLER_ACCOUNT_ID.to_string()),
            estimated_monthly_savings: Some(savings("USD", 5.0, 4.0, "AfterDiscount")),
            ..stored(
                3,
                "vol-ccc3",
                CALLER_ACCOUNT_ID,
                "eu-west-1",
                93,
                UPGRADE,
                Some("io1"),
                true,
                &[],
            )
        },
        RecommendedAction {
            account_id: Some(THIRD_ACCOUNT_ID.to_string()),
            estimated_monthly_savings: None,
            ..stored(
                4,
                "vol-ddd4",
                THIRD_ACCOUNT_ID,
                "us-east-1",
                14,
                UPGRADE,
                None,
                false,
                &[("env", "prod")],
            )
        },
    ]
}

/// The filter lists the cross-check is taken over: every modelled
/// `RecommendedActionFilterName`, each selecting a different subset.
fn filter_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("no-filters", json!([])),
        (
            "ResourceType",
            json!([{"name": "ResourceType", "values": ["EbsVolume"]}]),
        ),
        (
            "RecommendedActionType-one",
            json!([{"name": "RecommendedActionType", "values": [UPGRADE]}]),
        ),
        (
            "RecommendedActionType-the-other",
            json!([{"name": "RecommendedActionType", "values": [SNAPSHOT_AND_DELETE]}]),
        ),
        (
            "ResourceId",
            json!([{"name": "ResourceId", "values": ["vol-aaa1", "vol-bbb2"]}]),
        ),
        (
            "AccountId-the-callers-own",
            json!([{"name": "AccountId", "values": [CALLER_ACCOUNT_ID]}]),
        ),
        (
            "AccountId-a-member-account-only",
            json!([{"name": "AccountId", "values": [MEMBER_ACCOUNT_ID]}]),
        ),
        (
            "AccountId-two-accounts",
            json!([{"name": "AccountId", "values": [CALLER_ACCOUNT_ID, THIRD_ACCOUNT_ID]}]),
        ),
        (
            "LookBackPeriodInDays",
            json!([{"name": "LookBackPeriodInDays", "values": ["14"]}]),
        ),
        (
            "CurrentResourceDetailsEbsVolumeType",
            json!([{"name": "CurrentResourceDetailsEbsVolumeType", "values": ["gp2"]}]),
        ),
        (
            "ResourceTagsKey",
            json!([{"name": "ResourceTagsKey", "values": ["team"]}]),
        ),
        (
            "ResourceTagsValue",
            json!([{"name": "ResourceTagsValue", "values": ["prod"]}]),
        ),
        (
            "RestartNeeded",
            json!([{"name": "RestartNeeded", "values": ["true"]}]),
        ),
        (
            "two-filters-are-anded",
            json!([
                {"name": "RecommendedActionType", "values": [UPGRADE]},
                {"name": "RestartNeeded", "values": ["false"]}
            ]),
        ),
        (
            "a-filter-nothing-matches",
            json!([{"name": "ResourceId", "values": ["vol-nothing"]}]),
        ),
    ]
}

/// For every filter list, the one aggregate row totals exactly the actions
/// `ListRecommendedActions` reports for the SAME filters — so a filter nothing
/// matches reports the row with a count of zero rather than no row at all, and
/// the key is the same fixed text whatever the filter selected.
#[tokio::test]
async fn filters_narrow_the_actions_the_aggregate_row_totals() {
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());

    // The table spans the whole modelled value set, so a name added to the model
    // cannot be left untested.
    let covered: BTreeSet<String> = filter_cases()
        .iter()
        .flat_map(|(_, filters)| {
            filters
                .as_array()
                .expect("a filter list")
                .iter()
                .map(|filter| filter["name"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .collect();
    let modelled: BTreeSet<String> = RECOMMENDED_ACTION_FILTER_NAME_VALUES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        covered, modelled,
        "every modelled RecommendedActionFilterName must appear in the table"
    );

    // At least one row must select nothing and one must select something, so the
    // cases cover both ends of the rule.
    let mut selected_nothing = 0usize;
    let mut selected_something = 0usize;

    for (case, filters) in filter_cases() {
        let request = json!({"filters": filters, "maxResults": DEFAULT_MAX_RESULTS});
        let actions = listed_actions(&router, &request, case).await;

        // The oracle: the actions that operation reported for these filters,
        // counted and summed. How the aggregate names a currency and a savings
        // mode is the table above's business, so this cross-check carries the
        // reported ones over.
        let count = i64::try_from(actions.len()).expect("a small fixture set");
        let before: f64 = actions
            .iter()
            .map(|action| figure(action, "beforeDiscountSavings"))
            .sum();
        let after: f64 = actions
            .iter()
            .map(|action| figure(action, "afterDiscountSavings"))
            .sum();
        if count == 0 {
            selected_nothing += 1;
        } else {
            selected_something += 1;
        }

        let summarised = call_ok(&router, OPERATION, &request, case).await;
        let got = reported(&summarised);
        let reported_currency = got.first().map(|row| row.currency.clone()).unwrap_or_default();
        let reported_mode = got.first().map(|row| row.mode.clone()).unwrap_or_default();
        assert_aggregate_row(
            case,
            &got,
            &aggregate(count, before, after, &reported_currency, &reported_mode),
        );
        assert!(
            summarised.get("nextToken").is_none(),
            "case {case:?}: one aggregate row fits any page, so no token is issued: {summarised}"
        );
    }

    assert!(
        selected_nothing > 0 && selected_something > 0,
        "the filter table must span both a selection that holds nothing and one that holds \
         actions, got {selected_nothing} and {selected_something}"
    );

    // A filter name outside the modelled value set is refused, not ignored, and
    // the refusal reports the member and the accepted set without echoing the
    // rejected value back to the caller.
    let (status, body) = call(
        &router,
        OPERATION,
        &json!({"filters": [{"name": "ResourceArn", "values": ["vol-aaa1"]}]}),
    )
    .await;
    assert_refused(
        (status, body.clone()),
        400,
        "InvalidParameterValueException",
        "unknown-filter-name",
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        message.starts_with("1 validation error detected: Value at 'filters")
            && message.contains("member.name")
            && !message.contains("ResourceArn")
            && RECOMMENDED_ACTION_FILTER_NAME_VALUES
                .iter()
                .all(|name| message.contains(name)),
        "case \"unknown-filter-name\": the refusal must name the member and the accepted set \
         without echoing the rejected value, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The listing reports ONE row, so every page size serves that row and no walk
/// is ever cut: whatever `maxResults` asks for, the answer is the aggregate and
/// no token.
#[tokio::test]
async fn the_aggregate_row_fits_every_page_size_and_mints_no_token() {
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());

    // The reference: the row the listing reports when nothing is asked of the
    // page size.
    let whole = reported(&call_ok(&router, OPERATION, &json!({}), "one-page").await);
    assert_eq!(
        whole.len(),
        1,
        "the listing reports one aggregate row, got {whole:?}"
    );

    for page_size in [1_i32, 2, 3, DEFAULT_MAX_RESULTS] {
        let case = format!("maxResults-{page_size}");
        let body = call_ok(&router, OPERATION, &json!({"maxResults": page_size}), &case).await;
        let page = reported(&body);
        assert_eq!(
            page, whole,
            "case {case:?}: every page size serves the one aggregate row, got {page:?}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: nothing remains after the only row, so no token is carried: {body}"
        );
    }

    // `maxResults` carries a member `@range` of min 1 / max 100, which the
    // generated request walk enforces before the handler runs: each bound of
    // that range is served, and a value past either end is refused — including
    // 1000, the ceiling the member's PROSE names, which the range trait
    // overrides.
    for (case, max_results, expected_status) in [
        ("maxResults-at-the-range-minimum", 1, 200),
        ("maxResults-at-the-range-maximum", DEFAULT_MAX_RESULTS, 200),
        ("maxResults-below-the-range-minimum", 0, 400),
        (
            "maxResults-above-the-range-maximum",
            DEFAULT_MAX_RESULTS + 1,
            400,
        ),
        ("maxResults-at-the-documented-ceiling", 1000, 400),
        ("maxResults-negative", -1, 400),
    ] {
        let answered = call(&router, OPERATION, &json!({"maxResults": max_results})).await;
        if expected_status == 400 {
            assert_refused(answered, 400, "InvalidParameterValueException", case);
        } else {
            let (status, body) = answered;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
            );
        }
    }
}

/// A `nextToken` is accepted only where this listing could have minted it: for
/// the same filters, naming a position the row sequence reaches. Anything else —
/// a token that decodes to nothing, one carrying another query's fingerprint,
/// one another operation issued, or one naming a position past the only row — is
/// refused rather than answered with a page of something else.
#[tokio::test]
async fn a_token_resumes_only_the_filters_it_was_taken_with() {
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());

    assert_refused(
        call(&router, OPERATION, &json!({"nextToken": "notatokenweissued"})).await,
        400,
        "InvalidParameterValueException",
        "foreign-token",
    );
    assert_refused(
        call(&router, OPERATION, &json!({"nextToken": hex::encode("0:")})).await,
        400,
        "InvalidParameterValueException",
        "token-with-a-foreign-fingerprint",
    );

    // The fingerprint this listing digests an unfiltered walk under, and the one
    // it digests a walk under one filter: a cursor carrying either resumes the
    // walk it names at the end of its one row — the position a walk whose tail
    // was removed resumes at — while the same cursor under the OTHER filter list
    // is a token that walk never issued.
    let account_filters = json!([{"name": "AccountId", "values": [CALLER_ACCOUNT_ID]}]);
    // Each walk digests its filter list beside the `organizationalUnitIds`
    // restriction, which neither request names.
    let no_units = None::<Vec<String>>;
    let unfiltered = page_fingerprint(
        OPERATION,
        &(Vec::<RecommendedActionFilter>::new(), no_units.clone()),
    );
    let filtered = page_fingerprint(
        OPERATION,
        &(
            vec![RecommendedActionFilter {
                name: "AccountId".to_string(),
                values: vec![CALLER_ACCOUNT_ID.to_string()],
            }],
            no_units,
        ),
    );
    let cursor = |offset: usize, fingerprint: &str| {
        encode_page_token(&PageCursor {
            offset,
            filter_fingerprint: fingerprint.to_string(),
        })
    };

    // (case, the request the cursor is presented with, the cursor)
    let resumes: [(&str, Value, String); 2] = [
        (
            "the-unfiltered-walk-resumed-at-the-end-of-its-row-sequence",
            json!({}),
            cursor(1, &unfiltered),
        ),
        (
            "the-filtered-walk-resumed-at-the-end-of-its-row-sequence",
            json!({"filters": account_filters.clone()}),
            cursor(1, &filtered),
        ),
    ];
    for (case, request, token) in resumes {
        let mut resumed_request = request;
        resumed_request["nextToken"] = json!(token);
        let resumed = call_ok(&router, OPERATION, &resumed_request, case).await;
        assert!(
            reported(&resumed).is_empty() && resumed.get("nextToken").is_none(),
            "case {case:?}: the remainder after the only row is empty and ends the walk, got \
             {resumed}"
        );
    }

    // (case, the request the cursor is presented with, the cursor)
    let refusals: [(&str, Value, String); 3] = [
        (
            "a-position-the-row-sequence-never-reached",
            json!({}),
            cursor(2, &unfiltered),
        ),
        (
            "a-token-taken-with-other-filters",
            json!({}),
            cursor(1, &filtered),
        ),
        (
            "a-token-taken-without-the-filters-it-is-presented-with",
            json!({"filters": account_filters}),
            cursor(1, &unfiltered),
        ),
    ];
    for (case, request, token) in refusals {
        let mut refused_request = request;
        refused_request["nextToken"] = json!(token);
        assert_refused(
            call(&router, OPERATION, &refused_request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }

    // The token `ListRecommendedActions` issues for its own walk fingerprints
    // that operation's name into the cursor, so it is refused here.
    let first = call_ok(
        &router,
        "ListRecommendedActions",
        &json!({"maxResults": 1}),
        "another-operations-token",
    )
    .await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of one of four actions leaves a further page")
        .to_string();
    assert_refused(
        call(&router, OPERATION, &json!({"nextToken": token})).await,
        400,
        "InvalidParameterValueException",
        "token-of-another-list-operation",
    );
}

// ---------------------------------------------------------------------------
// The enrollment gate
// ---------------------------------------------------------------------------

/// An account that is not opted in is refused the whole call, whatever the store
/// holds, rather than told there is nothing to summarise.
#[tokio::test]
async fn an_account_that_is_not_opted_in_is_refused() {
    let (router, engine) = unenrolled_emulator_with_engine();
    record(&engine, fixtures());

    for (case, request) in [
        ("unenrolled-no-filters", json!({})),
        (
            "unenrolled-with-filters",
            json!({"filters": [{"name": "RecommendedActionType", "values": [UPGRADE]}]}),
        ),
        ("unenrolled-with-max-results", json!({"maxResults": 1})),
    ] {
        assert_refused(
            call(&router, OPERATION, &request).await,
            403,
            "OptInRequiredException",
            case,
        );
    }
    assert_eq!(
        stored_count(&engine),
        fixtures().len(),
        "a refusal must not change the store"
    );

    // The gate is not the absence of data: once enrolled, the same store answers
    // with the account-wide aggregate of everything it holds.
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());
    let listed = call_ok(&router, OPERATION, &json!({}), "enrolled").await;
    assert_aggregate_row(
        "enrolled",
        &reported(&listed),
        &aggregate(4, 37.5, 31.75, "USD", "AfterDiscount"),
    );

    // The row totals the actions `ListRecommendedActions` reports for the same
    // (absent) filters, so no action is lost or double counted — the two
    // independent reads reconcile.
    let actions = listed_actions(&router, &json!({}), "enrolled-oracle").await;
    let mut per_type: BTreeMap<String, i64> = BTreeMap::new();
    for action in &actions {
        let key = action
            .get("recommendedActionType")
            .and_then(Value::as_str)
            .expect("every fixture action carries its type")
            .to_string();
        *per_type.entry(key).or_default() += 1;
    }
    assert!(
        per_type.len() > 1,
        "the fixtures must hold more than one action type, so the aggregate really is an \
         aggregate, got {per_type:?}"
    );
    assert_eq!(
        reported(&listed)[0].count,
        per_type.values().sum::<i64>(),
        "the aggregate count must equal the actions the listing reports, got {per_type:?}"
    );
}
