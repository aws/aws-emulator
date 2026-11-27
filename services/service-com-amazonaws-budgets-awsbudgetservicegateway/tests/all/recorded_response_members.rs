// SPDX-License-Identifier: CC0-1.0

//! The MEMBER SET a budgets read reports, driven over the wire through the same
//! `MultiServiceRouter` the production router serves from.
//!
//! Three of these members are ones this service's model does not declare while the
//! recorded AWS answers carry them, and two are members the model does declare that a
//! recorded answer leaves out. Each test states the rule as a rule and asserts it over a
//! table whose rows differ along what the rule generalises over:
//!
//! 1. Every notification a read reports carries `ConsecutiveBreachCount` — how many budget
//!    periods in a row the spend it watches has passed its threshold, counted off the
//!    periods the budget has run for — and carries `ThresholdType` only when the threshold
//!    is not read as the percentage thresholds always are, so the default is never written
//!    out.
//! 2. Every `Budget` a read reports carries `CreatedAt`, the instant the budget was
//!    created, which an update does not move; the account-wide LISTING reports the same
//!    budget as the single read except for the `CalculatedSpend` the single read computes.
//! 3. Every subscriber of a budget ACTION reports the verification state this service
//!    records for its address, which no request can set; the subscribers of a NOTIFICATION
//!    report no such member.
//! 4. A monetary `Spend$Amount` is a decimal STRING, so it reads back in the spelling the
//!    caller wrote it in, on the budget's own read and on every record of its performance
//!    history.
//! 5. An optional collection a read has nothing to report for is left out rather than
//!    reported empty: `ListTagsForResource` answers a resource holding no tags without the
//!    `ResourceTags` member.
//!
//! The three members the model does not declare are declared from this crate's own overlay
//! model (`smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`)
//! rather than by editing the shipped snapshot, so the last tests here pin that overlay
//! against the snapshot it restates — shape by shape — and pin all three as members whose
//! value the SERVICE owns: a request claiming one changes nothing a read reports.
//!
//! The harness (`router`, `call`, `caller_account`, `same`) is the one
//! `tests/all/budget_lifecycle.rs` builds, and the budget, action, subscriber and tag
//! builders are the sibling suites', so every suite in this target drives the service the
//! same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The model-derived catalogs and the service's own published constants, so the tables
/// below cannot drift from the model or from the code under test.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::THRESHOLD_TYPE_VALUES;
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ATTRIBUTED_SPEND_AMOUNT, MAX_TAGS_PER_RESOURCE, NOTIFICATION_STATE_ALARM,
    NOTIFICATION_STATE_OK, SUBSCRIBER_VERIFICATION_PENDING,
};

use super::budget_action_execution::{
    create_body, email_subscriber, iam_definition, seed_budget, sns_subscriber,
};
use super::budget_lifecycle::{call, caller_account, router, same};
use super::budget_read_projection::month_start_offset;
use super::resource_tags::{action_arn, budget_arn, list_tags, many_tags, reported, tag_list};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declarations are checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-budgets-awsbudgetservicegateway.json"
));

/// The overlay model the local `Budget` / `Notification` / `Subscriber` declarations are
/// generated from, embedded for the same reason: what it restates of the snapshot must
/// still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The three shapes the overlay restates with a member added, each paired with the member
/// AWS carries on it and the snapshot does not declare.
const RESTATED_WITH_LOCAL_MEMBER: &[(&str, &str)] = &[
    ("com.amazonaws.budgets#Budget", "CreatedAt"),
    (
        "com.amazonaws.budgets#Notification",
        "ConsecutiveBreachCount",
    ),
    ("com.amazonaws.budgets#Subscriber", "VerificationStatus"),
];

/// The shapes the overlay ADDS, which the snapshot declares nowhere — the targets of the
/// three local members.
const ADDED_SHAPES: &[&str] = &[
    "com.amazonaws.budgets#ConsecutiveBreachCount",
    "com.amazonaws.budgets#VerificationStatus",
];

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types, enum
/// values and constraint traits — so comparing declarations has to ignore the one trait
/// that carries no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits"
                        || !value.as_object().is_some_and(serde_json::Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Value {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    Value::Object(
        parsed["shapes"]
            .as_object()
            .expect("a model document must carry a shapes object")
            .clone(),
    )
}

/// `supplied` — a `Subscribers` list as a request sent it — as a read of a budget ACTION
/// reports it: every subscriber carrying the verification state this service records for
/// its address.
///
/// `pub(crate)` so every suite in this target states the action-subscriber expectation
/// once rather than spelling the member out per assertion.
pub(crate) fn as_action_subscribers(supplied: &Value) -> Value {
    Value::Array(
        supplied
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut subscriber| {
                subscriber["VerificationStatus"] = json!(SUBSCRIBER_VERIFICATION_PENDING);
                subscriber
            })
            .collect(),
    )
}

/// `value` without `members` — what a comparison between two answers that differ in
/// exactly those members is made over.
///
/// `pub(crate)` for the sibling suites that compare the single budget read against the
/// listing, which report the same budget apart from one member.
pub(crate) fn without(value: &Value, members: &[&str]) -> Value {
    let mut trimmed = value.clone();
    if let Some(object) = trimmed.as_object_mut() {
        for member in members {
            object.remove(*member);
        }
    }
    trimmed
}

/// A `COST` budget of `time_unit`, limited at `amount` US dollars and starting at `start`
/// (epoch seconds) when a row names one.
fn cost_budget(name: &str, time_unit: &str, amount: &str, start: Option<i64>) -> Value {
    let mut budget = json!({
        "BudgetName": name,
        "TimeUnit": time_unit,
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": amount, "Unit": "USD"},
    });
    if let Some(start) = start {
        budget["TimePeriod"] = json!({"Start": start});
    }
    budget
}

/// Create `budget`, which every row of this suite must be allowed to create.
async fn create(router: &MultiServiceRouter, case: &str, account_id: &str, budget: &Value) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({"AccountId": account_id, "Budget": budget.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
    );
}

/// One row of the notification table: the budget to create it on, the notification as the
/// request sends it, and what a read must report for it.
struct NotificationRow {
    case: &'static str,
    time_unit: &'static str,
    /// The budget's `TimePeriod$Start`, in whole months relative to the month in progress,
    /// or `None` for a budget that names none and takes the documented default — the start
    /// of the period in progress, so exactly one period has run.
    start_months: Option<i32>,
    notification: Value,
    /// The `NotificationState` the service must report the notification in.
    state: &'static str,
    /// The `ConsecutiveBreachCount` the service must report for it.
    breaches: i64,
    /// The `ThresholdType` a read must report, or `None` where it must report none.
    threshold_type: Option<&'static str>,
}

/// The notification table. Nothing in this emulator spends behind a budget, so the spend
/// every notification below watches is the attributed zero (`ATTRIBUTED_SPEND_AMOUNT`):
/// a `GREATER_THAN` notification is therefore never in breach, a `LESS_THAN` one with a
/// positive threshold is in breach of every period that has run, and an `EQUAL_TO` one at
/// zero is too. The rows differ along the comparison (so along whether the notification is
/// in alarm at all), the threshold type and whether it was sent, the granularity and so
/// the number of periods that have run, the spend the notification watches, and the number
/// of periods the budget itself covers.
fn notification_rows() -> Vec<NotificationRow> {
    vec![
        NotificationRow {
            case: "a percentage threshold nothing has passed",
            time_unit: "MONTHLY",
            start_months: None,
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 70.0,
                "ThresholdType": "PERCENTAGE"
            }),
            state: NOTIFICATION_STATE_OK,
            breaches: 0,
            threshold_type: None,
        },
        NotificationRow {
            case: "the same notification with no ThresholdType sent",
            time_unit: "MONTHLY",
            start_months: None,
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 70.0
            }),
            state: NOTIFICATION_STATE_OK,
            breaches: 0,
            threshold_type: None,
        },
        NotificationRow {
            case: "a percentage threshold the period in progress is under",
            time_unit: "MONTHLY",
            start_months: None,
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 70.0,
                "ThresholdType": "PERCENTAGE"
            }),
            state: NOTIFICATION_STATE_ALARM,
            breaches: 1,
            threshold_type: None,
        },
        NotificationRow {
            case: "a budget that has run for four months",
            time_unit: "MONTHLY",
            start_months: Some(-3),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 50.0
            }),
            state: NOTIFICATION_STATE_ALARM,
            breaches: 4,
            threshold_type: None,
        },
        NotificationRow {
            case: "an absolute threshold the attributed spend is under",
            time_unit: "MONTHLY",
            start_months: None,
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 10.0,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            state: NOTIFICATION_STATE_ALARM,
            breaches: 1,
            threshold_type: Some("ABSOLUTE_VALUE"),
        },
        NotificationRow {
            case: "an absolute threshold the attributed spend is exactly at, daily",
            time_unit: "DAILY",
            start_months: None,
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "EQUAL_TO",
                "Threshold": 0.0,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            state: NOTIFICATION_STATE_ALARM,
            breaches: 1,
            threshold_type: Some("ABSOLUTE_VALUE"),
        },
        NotificationRow {
            case: "a forecasted notification, which watches a figure no budget here reports",
            time_unit: "QUARTERLY",
            start_months: None,
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 50.0,
                "ThresholdType": "PERCENTAGE"
            }),
            state: NOTIFICATION_STATE_OK,
            breaches: 0,
            threshold_type: None,
        },
        NotificationRow {
            case: "a granularity with no repeating period, whose history is its own period",
            time_unit: "ANNUALLY",
            start_months: None,
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 25.0,
                "ThresholdType": "PERCENTAGE"
            }),
            state: NOTIFICATION_STATE_ALARM,
            breaches: 1,
            threshold_type: None,
        },
    ]
}

/// The notification a read must report for `row`: the members the request named, with the
/// default threshold type dropped, plus the two members the service counts off the
/// budget's own figures.
fn expected_notification(row: &NotificationRow) -> Value {
    let mut expected = without(&row.notification, &["ThresholdType"]);
    if let Some(threshold_type) = row.threshold_type {
        expected["ThresholdType"] = json!(threshold_type);
    }
    expected["NotificationState"] = json!(row.state);
    expected["ConsecutiveBreachCount"] = json!(row.breaches);
    expected
}

/// RULE: a read reports a notification with the `ConsecutiveBreachCount` the service
/// counts off the periods the budget has run for — non-zero exactly while the notification
/// is in alarm — and reports `ThresholdType` only where it says something the default does
/// not, since "thresholds are always a percentage".
///
/// Asserted on BOTH read paths that report a notification, over the same stored
/// notification, so the two listings cannot come to report different members.
#[tokio::test]
async fn every_reported_notification_carries_the_breach_count_and_hides_the_default_threshold_type()
{
    // The row that expects a reported `ABSOLUTE_VALUE` and the rows that expect no
    // threshold type between them span the whole modelled catalog, which is what makes
    // "the default is the one that is left out" a statement about the catalog.
    assert_eq!(
        THRESHOLD_TYPE_VALUES,
        &["ABSOLUTE_VALUE", "PERCENTAGE"],
        "the ThresholdType catalog this table spans changed: {THRESHOLD_TYPE_VALUES:?}"
    );

    for (index, row) in notification_rows().into_iter().enumerate() {
        // One store per row, so the account-wide listing reports exactly this row's
        // budget.
        let router = router();
        let account_id = caller_account();
        let name = format!("breach-count-{index}");
        let budget = cost_budget(
            &name,
            row.time_unit,
            "200",
            row.start_months.map(month_start_offset),
        );
        create(&router, row.case, &account_id, &budget).await;

        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &name,
                "Notification": row.notification.clone(),
                "Subscribers": [email_subscriber(0)]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateNotification to answer 200, got {status} {body}",
            row.case
        );

        let expected = expected_notification(&row);

        let (status, body) = call(
            &router,
            "DescribeNotificationsForBudget",
            &json!({"AccountId": &account_id, "BudgetName": &name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeNotificationsForBudget to answer 200, got \
             {status} {body}",
            row.case
        );
        let per_budget = body["Notifications"][0].clone();
        assert!(
            same(&per_budget, &expected),
            "case {:?}: expected the per-budget read to report {expected}, got {per_budget}",
            row.case
        );

        let (status, body) = call(
            &router,
            "DescribeBudgetNotificationsForAccount",
            &json!({"AccountId": &account_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeBudgetNotificationsForAccount to answer 200, got \
             {status} {body}",
            row.case
        );
        let account_wide = body["BudgetNotificationsForAccount"][0]["Notifications"][0].clone();
        assert!(
            same(&account_wide, &expected),
            "case {:?}: expected the account-wide read to report {expected}, got \
             {account_wide}",
            row.case
        );

        // The counter is non-zero exactly while the notification is in alarm: both are
        // read off the same figures.
        let in_alarm = per_budget["NotificationState"] == json!(NOTIFICATION_STATE_ALARM);
        let counted = per_budget["ConsecutiveBreachCount"].as_i64().unwrap_or(-1);
        assert_eq!(
            in_alarm,
            counted > 0,
            "case {:?}: expected the breach counter to be non-zero exactly while the \
             notification is in alarm, got {per_budget}",
            row.case
        );
    }
}

/// RULE: a read reports a notification, it does not establish one — a second read of the
/// same notification reports the same members and the same breach count.
#[tokio::test]
async fn reading_a_notification_twice_reports_the_same_breach_count() {
    let router = router();
    let account_id = caller_account();
    let name = "breach-count-purity";
    create(
        &router,
        "purity",
        &account_id,
        &cost_budget(name, "MONTHLY", "200", Some(month_start_offset(-2))),
    )
    .await;
    let (status, body) = call(
        &router,
        "CreateNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": name,
            "Notification": {
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 40.0
            },
            "Subscribers": [email_subscriber(1)]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateNotification to answer 200, got {status} {body}"
    );

    let ask = json!({"AccountId": &account_id, "BudgetName": name});
    let (_, first) = call(&router, "DescribeNotificationsForBudget", &ask).await;
    let (_, second) = call(&router, "DescribeNotificationsForBudget", &ask).await;
    assert!(
        same(&first["Notifications"], &second["Notifications"]),
        "expected a second read to report the same notifications as {}, got {}",
        first["Notifications"],
        second["Notifications"]
    );
}

/// One row of the budget-read table: a budget to create, and the unit its tracked spend is
/// reported in — `None` for a budget whose read reports no limit, and so no spend.
struct BudgetRow {
    case: &'static str,
    budget: Value,
    spend_unit: Option<&'static str>,
}

/// The budget-read table. The rows differ along the budget type, the granularity, where
/// the limit a read reports comes from (a fixed `BudgetLimit`, the planned entry in force,
/// the documented utilization default, nowhere at all) and the scoping syntax, which is
/// what the rule generalises over.
fn budget_rows() -> Vec<BudgetRow> {
    vec![
        BudgetRow {
            case: "a monthly cost budget with a fixed limit",
            budget: cost_budget("read-members", "MONTHLY", "250", None),
            spend_unit: Some("USD"),
        },
        BudgetRow {
            case: "a daily usage budget measured in GB",
            budget: json!({
                "BudgetName": "read-members",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "3", "Unit": "GB"},
                "CostFilters": {"Service": ["Amazon Elastic Compute Cloud - Compute"]}
            }),
            spend_unit: Some("GB"),
        },
        BudgetRow {
            case: "a monthly cost budget whose limit comes from a planned map",
            budget: json!({
                "BudgetName": "read-members",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {
                    month_start_offset(0).to_string(): {"Amount": "300.0", "Unit": "JPY"}
                }
            }),
            spend_unit: Some("JPY"),
        },
        BudgetRow {
            case: "a utilization budget that takes the documented default limit",
            budget: json!({
                "BudgetName": "read-members",
                "TimeUnit": "ANNUALLY",
                "BudgetType": "RI_UTILIZATION"
            }),
            spend_unit: Some("PERCENTAGE"),
        },
        BudgetRow {
            case: "an auto-adjusting budget, whose read reports no limit",
            budget: json!({
                "BudgetName": "read-members",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "AutoAdjustData": {"AutoAdjustType": "FORECAST"}
            }),
            spend_unit: None,
        },
    ]
}

/// The `Budget` `DescribeBudget` reports for `name`.
async fn describe(router: &MultiServiceRouter, case: &str, account_id: &str, name: &str) -> Value {
    let (status, body) = call(
        router,
        "DescribeBudget",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
    );
    body["Budget"].clone()
}

/// The `Budget` the account-wide listing reports for `name`.
async fn describe_in_list(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) -> Value {
    let (status, body) = call(router, "DescribeBudgets", &json!({"AccountId": account_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudgets to answer 200, got {status} {body}"
    );
    body["Budgets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|listed| listed["BudgetName"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: {name} is not listed: {body}"))
}

/// RULE: every `Budget` a read reports carries the instant it was created, on both read
/// paths, and the two read paths report the same budget apart from the `CalculatedSpend`
/// the single-budget read computes and the listing leaves out.
#[tokio::test]
async fn a_read_reports_the_creation_instant_and_the_listing_leaves_the_spend_out() {
    for row in budget_rows() {
        // One store per row: every request names the one account this service serves.
        let router = router();
        let account_id = caller_account();
        let name = row.budget["BudgetName"]
            .as_str()
            .expect("every row names its budget")
            .to_string();
        create(&router, row.case, &account_id, &row.budget).await;

        let read = describe(&router, row.case, &account_id, &name).await;
        let created_at = read["CreatedAt"].as_f64();
        assert!(
            created_at.is_some_and(|at| at > 0.0),
            "case {:?}: expected the read to carry the instant the budget was created, got \
             {read}",
            row.case
        );
        // A budget is created and stamped by one call, so the instant it was created is
        // the instant it was last updated — until something updates it.
        assert_eq!(
            read["CreatedAt"].as_f64(),
            read["LastUpdatedTime"].as_f64(),
            "case {:?}: expected a freshly created budget's CreatedAt to be its \
             LastUpdatedTime, got {read}",
            row.case
        );

        let listed = describe_in_list(&router, row.case, &account_id, &name).await;
        assert_eq!(
            listed["CreatedAt"].as_f64(),
            created_at,
            "case {:?}: expected the listing to report the same creation instant, got \
             {listed}",
            row.case
        );
        assert!(
            listed.get("CalculatedSpend").is_none(),
            "case {:?}: expected the listing to leave the tracked spend out, got {listed}",
            row.case
        );
        match row.spend_unit {
            Some(unit) => assert!(
                same(
                    &read["CalculatedSpend"],
                    &json!({"ActualSpend": {"Amount": ATTRIBUTED_SPEND_AMOUNT, "Unit": unit}})
                ),
                "case {:?}: expected the single read to report the spend it computes, got \
                 {read}",
                row.case
            ),
            None => assert!(
                read.get("CalculatedSpend").is_none(),
                "case {:?}: a budget that reports no limit names no unit, so it reports no \
                 spend, got {read}",
                row.case
            ),
        }
        assert!(
            same(&without(&read, &["CalculatedSpend"]), &listed),
            "case {:?}: expected the two read paths to differ in the tracked spend alone, \
             got {read} and {listed}",
            row.case
        );
    }
}

/// RULE: the creation instant is the instant the budget CAME INTO BEING, so an update
/// replaces every part of the budget the caller may change and leaves it where it was.
#[tokio::test]
async fn an_update_does_not_move_the_instant_the_budget_was_created() {
    let router = router();
    let account_id = caller_account();
    let name = "created-at-across-updates";
    create(
        &router,
        "created",
        &account_id,
        &cost_budget(name, "MONTHLY", "100", None),
    )
    .await;
    let created_at = describe(&router, "created", &account_id, name).await["CreatedAt"].as_f64();

    // Each step is a `NewBudget` an `UpdateBudget` sends: another amount, another unit,
    // another granularity, another budget type and a replaced limit source.
    let steps: Vec<(&str, Value)> = vec![
        ("the amount changed", cost_budget(name, "MONTHLY", "500", None)),
        (
            "the unit and granularity changed",
            json!({
                "BudgetName": name,
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "9", "Unit": "GB"}
            }),
        ),
        (
            "the limit replaced by a planned map",
            json!({
                "BudgetName": name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {
                    month_start_offset(0).to_string(): {"Amount": "20", "Unit": "USD"}
                }
            }),
        ),
    ];
    for (case, new_budget) in steps {
        let (status, body) = call(
            &router,
            "UpdateBudget",
            &json!({"AccountId": &account_id, "NewBudget": new_budget}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBudget to answer 200, got {status} {body}"
        );
        let read = describe(&router, case, &account_id, name).await;
        assert_eq!(
            read["CreatedAt"].as_f64(),
            created_at,
            "case {case:?}: expected the creation instant to be unchanged by an update, got \
             {read}"
        );
    }
}

/// One row of the subscriber table: the set a write supplies.
struct SubscriberRow {
    case: &'static str,
    supplied: Value,
}

/// The subscriber table. The rows differ along the subscription types in the set and the
/// size of the set, inside the modelled `@length(1, 11)` of `Subscribers` and the
/// documented per-type capacities (one SNS topic, up to ten email addresses).
fn subscriber_rows() -> Vec<SubscriberRow> {
    vec![
        SubscriberRow {
            case: "one email address",
            supplied: json!([email_subscriber(0)]),
        },
        SubscriberRow {
            case: "an email address and an SNS topic",
            supplied: json!([email_subscriber(1), sns_subscriber()]),
        },
        SubscriberRow {
            case: "three email addresses",
            supplied: json!([
                email_subscriber(2),
                email_subscriber(3),
                email_subscriber(4)
            ]),
        },
    ]
}

/// RULE: every subscriber of a budget ACTION is reported with the verification state this
/// service records for its address — an address nobody has confirmed — on every read that
/// reports an action, and no request can set it.
#[tokio::test]
async fn an_action_reports_the_verification_state_of_each_of_its_subscribers() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "action-subscriber-verification";
    seed_budget(&router, &account_id, budget_name).await;

    for (index, row) in subscriber_rows().into_iter().enumerate() {
        let case = row.case;
        // A create whose request CLAIMS the addresses are already confirmed: the member is
        // the service's own record, so the claim is not stored.
        let claimed = Value::Array(
            row.supplied
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|mut subscriber| {
                    subscriber["VerificationStatus"] = json!("VERIFIED");
                    subscriber
                })
                .collect(),
        );
        let (status, body) = call(
            &router,
            "CreateBudgetAction",
            &create_body(
                &account_id,
                budget_name,
                "APPLY_IAM_POLICY",
                &iam_definition(),
                "MANUAL",
                "ACTUAL",
                &json!({
                    "ActionThresholdValue": 10.0 * (index as f64 + 1.0),
                    "ActionThresholdType": "PERCENTAGE"
                }),
                &claimed,
                None,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudgetAction to answer 200, got {status} {body}"
        );
        let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
        let expected = as_action_subscribers(&row.supplied);

        let (status, body) = call(
            &router,
            "DescribeBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudgetAction to answer 200, got {status} {body}"
        );
        assert!(
            same(&body["Action"]["Subscribers"], &expected),
            "case {case:?}: expected the action to hold {expected}, got {}",
            body["Action"]["Subscribers"]
        );

        // The same action read out of the per-budget listing and out of its own history
        // reports the same subscribers: one stored record, one projection.
        let (status, body) = call(
            &router,
            "DescribeBudgetActionsForBudget",
            &json!({"AccountId": &account_id, "BudgetName": budget_name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudgetActionsForBudget to answer 200, got \
             {status} {body}"
        );
        let listed = body["Actions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|action| action["ActionId"] == json!(&action_id))
            .unwrap_or_else(|| panic!("case {case:?}: the action is not listed"));
        assert!(
            same(&listed["Subscribers"], &expected),
            "case {case:?}: expected the listed action to hold {expected}, got {}",
            listed["Subscribers"]
        );

        let (status, body) = call(
            &router,
            "DescribeBudgetActionHistories",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudgetActionHistories to answer 200, got \
             {status} {body}"
        );
        let recorded = body["ActionHistories"][0]["ActionHistoryDetails"]["Action"].clone();
        assert!(
            same(&recorded["Subscribers"], &expected),
            "case {case:?}: expected the recorded action to hold {expected}, got {}",
            recorded["Subscribers"]
        );
    }
}

/// RULE, on the update path: a replacement subscriber set is reported the same way — a
/// replaced address is an address nobody has confirmed either.
#[tokio::test]
async fn a_replacement_subscriber_set_reports_the_same_verification_state() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "action-subscriber-replacement";
    seed_budget(&router, &account_id, budget_name).await;
    let (status, body) = call(
        &router,
        "CreateBudgetAction",
        &create_body(
            &account_id,
            budget_name,
            "APPLY_IAM_POLICY",
            &iam_definition(),
            "MANUAL",
            "ACTUAL",
            &json!({"ActionThresholdValue": 60.0, "ActionThresholdType": "PERCENTAGE"}),
            &json!([email_subscriber(0)]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudgetAction to answer 200, got {status} {body}"
    );
    let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();

    for row in subscriber_rows() {
        let case = row.case;
        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id,
                "Subscribers": row.supplied.clone()
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBudgetAction to answer 200, got {status} {body}"
        );
        let expected = as_action_subscribers(&row.supplied);
        assert!(
            same(&body["NewAction"]["Subscribers"], &expected),
            "case {case:?}: expected the replacement set to be reported as {expected}, got {}",
            body["NewAction"]["Subscribers"]
        );
    }
}

/// RULE: the subscribers of a NOTIFICATION are reported as the two members the model
/// declares — the recorded reads of a notification's subscribers carry no verification
/// state — so a set stored on a notification reads back exactly as it was supplied.
#[tokio::test]
async fn a_notifications_subscribers_report_no_verification_state() {
    let router = router();
    let account_id = caller_account();
    let name = "notification-subscriber-members";
    create(
        &router,
        "notification subscribers",
        &account_id,
        &cost_budget(name, "MONTHLY", "100", None),
    )
    .await;

    for (index, row) in subscriber_rows().into_iter().enumerate() {
        let case = row.case;
        let notification = json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "GREATER_THAN",
            "Threshold": 10.0 * (index as f64 + 1.0)
        });
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": notification.clone(),
                "Subscribers": row.supplied.clone()
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateNotification to answer 200, got {status} {body}"
        );

        let (status, body) = call(
            &router,
            "DescribeSubscribersForNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": notification
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeSubscribersForNotification to answer 200, got \
             {status} {body}"
        );
        assert!(
            same(&body["Subscribers"], &row.supplied),
            "case {case:?}: expected the notification to report {}, got {}",
            row.supplied,
            body["Subscribers"]
        );
    }
}

/// RULE: a monetary amount is a decimal STRING, so every read reports it in the spelling
/// the caller wrote — `Spend$Amount` targets `NumericValue`, a string whose
/// `@pattern ^([0-9]*\.)?[0-9]+$` admits spellings a numeric round trip would not preserve
/// (a trailing `.0`, trailing zeros, a leading zero), and nothing in the service renders
/// an amount through a numeric type.
///
/// The rows are those spellings, asserted on the budget's own read AND on the
/// `BudgetedAmount` of every record of its performance history, which is where the amount
/// travels furthest from the write that stored it.
#[tokio::test]
async fn every_spelling_of_an_amount_reads_back_exactly_as_it_was_written() {
    // Each spelling is one `NumericValue` the pattern admits and a float round trip would
    // respell: `250` would come back `250.0`, `250.00` and `0300.50` would lose their
    // trailing and leading zeros, and `0.10` its trailing zero.
    for amount in ["250", "250.0", "250.00", "0300.50", "0.10", "15000000000000"] {
        let router = router();
        let account_id = caller_account();
        let name = format!("amount-{amount}");
        let budget = cost_budget(&name, "MONTHLY", amount, Some(month_start_offset(-2)));
        create(&router, amount, &account_id, &budget).await;

        let read = describe(&router, amount, &account_id, &name).await;
        assert_eq!(
            read["BudgetLimit"],
            json!({"Amount": amount, "Unit": "USD"}),
            "case {amount:?}: expected the limit to read back as written, got {read}"
        );

        let (status, body) = call(
            &router,
            "DescribeBudgetPerformanceHistory",
            &json!({"AccountId": &account_id, "BudgetName": &name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {amount:?}: expected DescribeBudgetPerformanceHistory to answer 200, got \
             {status} {body}"
        );
        let records = body["BudgetPerformanceHistory"]["BudgetedAndActualAmountsList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !records.is_empty(),
            "case {amount:?}: expected the periods the budget has run for, got {body}"
        );
        for record in &records {
            assert_eq!(
                record["BudgetedAmount"],
                json!({"Amount": amount, "Unit": "USD"}),
                "case {amount:?}: expected the budgeted amount as written, got {record}"
            );
            assert_eq!(
                record["ActualAmount"],
                json!({"Amount": ATTRIBUTED_SPEND_AMOUNT, "Unit": "USD"}),
                "case {amount:?}: expected the attributed spend in the limit's unit, got \
                 {record}"
            );
        }
    }
}

/// RULE: a read reports an optional collection it has nothing to report for by LEAVING IT
/// OUT — `ListTagsForResource` answers a resource holding no tags without the
/// `ResourceTags` member, and reports the member for one holding any.
///
/// Asserted over both kinds of taggable resource ("a budget or budget action resource")
/// and over the range of tag counts one may hold, including each published bound of
/// `ResourceTagList`.
#[tokio::test]
async fn an_untagged_resource_is_reported_without_the_tag_member() {
    let router = router();
    let account_id = caller_account();

    for count in [0, 1, 2, 7, MAX_TAGS_PER_RESOURCE] {
        let tags = many_tags(count);
        let budget_name = format!("tag-count-{count:03}");
        let mut request = json!({
            "AccountId": &account_id,
            "Budget": cost_budget(&budget_name, "MONTHLY", "100", None)
        });
        if count > 0 {
            request["ResourceTags"] = tag_list(&tags);
        }
        let (status, body) = call(&router, "CreateBudget", &request).await;
        assert_eq!(
            status, 200,
            "case {count} tags: expected CreateBudget to answer 200, got {status} {body}"
        );
        let action_request = create_body(
            &account_id,
            &budget_name,
            "APPLY_IAM_POLICY",
            &iam_definition(),
            "MANUAL",
            "ACTUAL",
            &json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
            &json!([email_subscriber(0)]),
            (count > 0).then(|| tag_list(&tags)),
        );
        let (status, body) = call(&router, "CreateBudgetAction", &action_request).await;
        assert_eq!(
            status, 200,
            "case {count} tags: expected CreateBudgetAction to answer 200, got {status} {body}"
        );
        let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();

        for (kind, arn) in [
            ("budget", budget_arn(&account_id, &budget_name)),
            (
                "budget action",
                action_arn(&account_id, &budget_name, &action_id),
            ),
        ] {
            let (status, body) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {count} tags on a {kind}: expected 200, got {status} {body}"
            );
            assert_eq!(
                body.get("ResourceTags").is_some(),
                count > 0,
                "case {count} tags on a {kind}: expected the ResourceTags member to be \
                 reported only for a resource holding tags, got {body}"
            );
            assert_eq!(
                reported(&body),
                tags,
                "case {count} tags on a {kind}: expected the tags it was created with, got \
                 {body}"
            );
        }
    }
}

/// RULE, the other way round: the member follows the tags a resource HOLDS, not the way it
/// was created — untagging a tagged resource leaves it reported without the member again.
#[tokio::test]
async fn untagging_a_resource_leaves_it_reported_without_the_tag_member() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "untagged-again";
    let tags = many_tags(3);
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": cost_budget(budget_name, "MONTHLY", "100", None),
            "ResourceTags": tag_list(&tags)
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget to answer 200, got {status} {body}"
    );
    let arn = budget_arn(&account_id, budget_name);

    let (_, body) = list_tags(&router, &arn).await;
    assert!(
        body.get("ResourceTags").is_some(),
        "expected a tagged resource to report its tags, got {body}"
    );

    let keys: Vec<String> = tags.iter().map(|(key, _)| key.clone()).collect();
    let (status, body) = call(
        &router,
        "UntagResource",
        &json!({"ResourceARN": &arn, "ResourceTagKeys": keys}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UntagResource to answer 200, got {status} {body}"
    );

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource to answer 200, got {status} {body}"
    );
    assert!(
        body.get("ResourceTags").is_none(),
        "expected an untagged resource to be reported without the tag member, got {body}"
    );
}

/// RULE: a member AWS answers with that the published snapshot does not declare is
/// declared from this crate's OWN overlay model, never by editing the snapshot — so
/// anything the overlay restates of the snapshot (a member's target, a shape's type, an
/// enum's value set, a constraint trait) must still BE what the snapshot says, and the only
/// difference is the member AWS carries on each of the three shapes.
///
/// A refreshed snapshot that declares one of the three, adds a member the overlay does not
/// restate, or changes a target, value set or constraint fails here rather than silently
/// narrowing or widening what the service accepts and reports.
#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_members_it_adds() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        if ADDED_SHAPES.contains(&shape_id.as_str()) {
            assert!(
                snapshot[shape_id].is_null(),
                "the snapshot now declares {shape_id}, so the local declaration should give \
                 way to the modelled shape"
            );
            continue;
        }
        let modelled = &snapshot[shape_id];
        assert!(
            !modelled.is_null(),
            "the overlay restates {shape_id}, which the snapshot no longer declares"
        );

        if let Some((_, local_member)) = RESTATED_WITH_LOCAL_MEMBER
            .iter()
            .find(|(restated_id, _)| *restated_id == shape_id.as_str())
        {
            // A shape that differs on purpose: the snapshot's members, restated as the
            // snapshot declares them, plus the one member AWS carries and it does not.
            let declared_members = declared["members"]
                .as_object()
                .unwrap_or_else(|| panic!("the overlay declares {shape_id}'s members"));
            let modelled_members = modelled["members"]
                .as_object()
                .unwrap_or_else(|| panic!("the snapshot declares {shape_id}'s members"));
            for (member, modelled_member) in modelled_members {
                assert_eq!(
                    without_docs(&declared_members[member]),
                    without_docs(modelled_member),
                    "member {member} of {shape_id} must be restated as the snapshot declares \
                     it, got {:?}",
                    declared_members.get(member)
                );
            }
            let added: Vec<&String> = declared_members
                .keys()
                .filter(|member| !modelled_members.contains_key(*member))
                .collect();
            assert_eq!(
                added,
                vec![local_member],
                "{shape_id} may add {local_member} and nothing else, added {added:?}"
            );
            continue;
        }

        if declared["members"]
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            // A STRUCTURE restated for its type alone: codegen resolves a structure member
            // through the Rust field's type (the modelled type in `shared_types`), reading
            // only the shape's type from the model.
            assert_eq!(
                declared["type"],
                json!("structure"),
                "{shape_id} is restated without members, which only a structure may be"
            );
            assert_eq!(
                declared["type"], modelled["type"],
                "{shape_id} must be restated with the type the snapshot gives it"
            );
            continue;
        }

        assert_eq!(
            without_docs(declared),
            without_docs(modelled),
            "{shape_id} must be restated exactly as the snapshot declares it"
        );
    }

    // Every shape the overlay restates with a member added is covered above, so none of
    // the three can fall out of the overlay unnoticed.
    for (shape_id, local_member) in RESTATED_WITH_LOCAL_MEMBER {
        assert!(
            restated.contains_key(*shape_id),
            "the overlay must declare {shape_id}, which carries {local_member}"
        );
    }
}

/// The instants a request may CLAIM a budget was created at. They differ along where the
/// claim sits relative to the write that stores it — the epoch, a long-past instant and an
/// instant in the future — none of which is the instant of the write, which is the only
/// value the service may report.
fn claimed_instants() -> Vec<(&'static str, Value)> {
    vec![
        ("the epoch", json!(0)),
        ("a long-past instant", json!(1_000_000_000_i64)),
        ("an instant in the future", json!(4_000_000_000_i64)),
    ]
}

/// RULE: a member whose value the SERVICE owns is not settable by a request — a write
/// stores what the service stamps and every later read reports that, whatever the request
/// carried for it.
///
/// Asserted for `Budget$CreatedAt` on both write paths: the create stamps the instant of
/// the write (which, for a budget nothing has updated yet, is also its `LastUpdatedTime`),
/// and the update leaves it where it was however the request spells it.
#[tokio::test]
async fn a_request_cannot_set_the_instant_a_budget_was_created() {
    for (index, (case, claimed)) in claimed_instants().into_iter().enumerate() {
        let router = router();
        let account_id = caller_account();
        let name = format!("claimed-created-at-{index}");

        let mut budget = cost_budget(&name, "MONTHLY", "100", None);
        budget["CreatedAt"] = claimed.clone();
        create(&router, case, &account_id, &budget).await;

        let read = describe(&router, case, &account_id, &name).await;
        assert_ne!(
            read["CreatedAt"], claimed,
            "case {case:?}: expected the claimed creation instant to be discarded, got {read}"
        );
        assert_eq!(
            read["CreatedAt"], read["LastUpdatedTime"],
            "case {case:?}: expected the create to stamp the instant of the write, got {read}"
        );
        let stamped = read["CreatedAt"].clone();

        // The same claim on the update path, which does not create a budget at all.
        for (update_case, update_claim) in claimed_instants() {
            let mut new_budget = cost_budget(&name, "MONTHLY", "500", None);
            new_budget["CreatedAt"] = update_claim.clone();
            let (status, body) = call(
                &router,
                "UpdateBudget",
                &json!({"AccountId": &account_id, "NewBudget": new_budget}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} updated with {update_case:?}: expected UpdateBudget to answer \
                 200, got {status} {body}"
            );
            let read = describe(&router, case, &account_id, &name).await;
            assert_eq!(
                read["CreatedAt"], stamped,
                "case {case:?} updated with {update_case:?}: expected the creation instant to \
                 be unchanged by an update, got {read}"
            );
        }
    }
}

/// RULE, the same rule on the other two locally declared members: the count a read reports
/// for a notification is the one the service counts off the budget's figures, so a notified
/// request's claim changes nothing a read reports.
///
/// Each row writes the same notification twice — once with a claimed count and once without
/// — on two budgets that are identical but for their name, so the figures behind them are
/// the same. The reads must agree, on the create path and after an update.
#[tokio::test]
async fn a_request_cannot_set_the_breach_count_of_a_notification() {
    // The claims differ along their size and whether they could be a real count at all:
    // zero, a count inside the periods a budget retains, and one far beyond it.
    for (index, (case, claimed)) in [
        ("no breaches claimed", json!(0)),
        ("a plausible count claimed", json!(2)),
        ("a count beyond any budget's periods claimed", json!(9_999)),
    ]
    .into_iter()
    .enumerate()
    {
        let router = router();
        let account_id = caller_account();
        let claimed_on = format!("claimed-breaches-{index}");
        let counted_on = format!("counted-breaches-{index}");
        let start = Some(month_start_offset(-2));
        for name in [&claimed_on, &counted_on] {
            create(
                &router,
                case,
                &account_id,
                &cost_budget(name, "MONTHLY", "200", start),
            )
            .await;
        }

        let notification = json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "LESS_THAN",
            "Threshold": 60.0
        });
        let mut with_claim = notification.clone();
        with_claim["ConsecutiveBreachCount"] = claimed.clone();

        for (name, sent) in [(&claimed_on, &with_claim), (&counted_on, &notification)] {
            let (status, body) = call(
                &router,
                "CreateNotification",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": name,
                    "Notification": sent.clone(),
                    "Subscribers": [email_subscriber(0)]
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} on {name}: expected CreateNotification to answer 200, got \
                 {status} {body}"
            );
        }

        let read_one = read_notification(&router, case, &account_id, &claimed_on).await;
        let counted = read_notification(&router, case, &account_id, &counted_on).await;
        assert!(
            same(&read_one, &counted),
            "case {case:?}: expected the claim to change nothing a read reports, got \
             {read_one} beside {counted}"
        );
        assert!(
            read_one["ConsecutiveBreachCount"].as_i64().is_some(),
            "case {case:?}: expected every reported notification to carry the count, got \
             {read_one}"
        );

        // The update path writes the member the same way: a replacement notification
        // claiming a count is stored without it and read back counted.
        let mut replacement = notification.clone();
        replacement["Threshold"] = json!(80.0);
        let mut claimed_replacement = replacement.clone();
        claimed_replacement["ConsecutiveBreachCount"] = claimed.clone();
        for (name, sent) in [
            (&claimed_on, &claimed_replacement),
            (&counted_on, &replacement),
        ] {
            let (status, body) = call(
                &router,
                "UpdateNotification",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": name,
                    "OldNotification": notification.clone(),
                    "NewNotification": sent.clone()
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} on {name}: expected UpdateNotification to answer 200, got \
                 {status} {body}"
            );
        }
        let updated_claim = read_notification(&router, case, &account_id, &claimed_on).await;
        let updated_counted = read_notification(&router, case, &account_id, &counted_on).await;
        assert!(
            same(&updated_claim, &updated_counted),
            "case {case:?}: expected the replacement's claim to change nothing a read \
             reports, got {updated_claim} beside {updated_counted}"
        );
    }
}

/// The one notification `DescribeNotificationsForBudget` reports for `name`.
async fn read_notification(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) -> Value {
    let (status, body) = call(
        router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeNotificationsForBudget to answer 200, got \
         {status} {body}"
    );
    body["Notifications"][0].clone()
}

