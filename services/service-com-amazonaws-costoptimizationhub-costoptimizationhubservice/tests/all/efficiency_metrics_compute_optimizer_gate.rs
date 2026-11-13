// SPDX-License-Identifier: CC0-1.0

//! The AWS Compute Optimizer opt-in `ListEfficiencyMetrics` is gated on.
//!
//! The rule under test: the efficiency and savings series this read reports is
//! derived from AWS Compute Optimizer's analysis of the account's resources, and
//! that service analyses an account only once it has opted IN, so the read is
//! refused (HTTP 400 `ValidationException`) for an account that is not opted in
//! and served once it is. The opt-in belongs to Compute Optimizer: it is written
//! here only through that service's own `UpdateEnrollmentStatus`, over the wire,
//! never by recording a flag into a store — so these cases prove the gate is
//! backed by the state the owning service reports, and that it follows that
//! state in BOTH directions (opting out refuses the read again).
//!
//! Every case is a well-formed request: the precondition is about the ACCOUNT,
//! so it holds whatever granularity, period, grouping, ordering or page size the
//! request carries, which is what the table spans.

use serde_json::{json, Value};

use crate::cost_optimization_hub::{
    call, call_compute_optimizer, daily_period, error_code, monthly_period,
    opt_into_compute_optimizer, router,
};

/// The operation under test, named once: it is also the only part of the refusal
/// sentence that varies, so the case data and the expected message cannot drift
/// apart.
const OPERATION: &str = "ListEfficiencyMetrics";

/// The sentence AWS refuses this read with while the account is not opted into
/// Compute Optimizer: fixed text with the operation called in front of it.
fn opt_in_required(operation: &str) -> String {
    format!(
        "{operation} requires your account to be opted into AWS Compute Optimizer. Please \
         enable Compute Optimizer in the AWS Compute Optimizer console or via the Compute \
         Optimizer API."
    )
}

/// Well-formed requests spanning what this read takes: both granularities, a
/// period of one and of several intervals, no grouping and each kind of grouping
/// dimension, an ordering in both directions, and a bounded page.
///
/// Every period is built from the clock, because the window this read reports
/// over ends at the present interval (`efficiency_metrics` owns that rule).
fn well_formed_requests() -> Vec<(String, Value)> {
    let mut cases: Vec<(String, Value)> = Vec::new();

    let (daily_start, daily_end) = daily_period(1, 1);
    cases.push((
        "one day, no grouping".to_string(),
        json!({
            "granularity": "Daily",
            "timePeriod": {"start": daily_start, "end": daily_end},
        }),
    ));

    let (week_start, week_end) = daily_period(10, 7);
    cases.push((
        "a week grouped by region".to_string(),
        json!({
            "granularity": "Daily",
            "timePeriod": {"start": week_start, "end": week_end},
            "groupBy": "Region",
        }),
    ));

    let (month_start, month_end) = monthly_period(1, 1);
    cases.push((
        "one month, no grouping".to_string(),
        json!({
            "granularity": "Monthly",
            "timePeriod": {"start": month_start, "end": month_end},
        }),
    ));

    let (months_start, months_end) = monthly_period(2, 2);
    cases.push((
        "two months grouped by account id, ordered".to_string(),
        json!({
            "granularity": "Monthly",
            "timePeriod": {"start": months_start, "end": months_end},
            "groupBy": "AccountId",
            "orderBy": {"dimension": "spend", "order": "Desc"},
        }),
    ));

    let (paged_start, paged_end) = monthly_period(1, 1);
    cases.push((
        "one month grouped by resource type, one group per page".to_string(),
        json!({
            "granularity": "Monthly",
            "timePeriod": {"start": paged_start, "end": paged_end},
            "groupBy": "ResourceType",
            "maxResults": 1,
        }),
    ));

    cases
}

/// Assert `payload` is the refusal an account that is not opted into Compute
/// Optimizer earns: HTTP 400 `ValidationException` whose `message` is the fixed
/// sentence with the operation called, carrying no series at all.
fn assert_opt_in_refusal(case: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let code = error_code(payload);
    assert_eq!(
        code, "ValidationException",
        "case {case:?}: expected a ValidationException, got {code:?} in {payload}"
    );
    let message = payload["message"].as_str().unwrap_or_default();
    assert_eq!(
        message,
        opt_in_required(OPERATION),
        "case {case:?}: expected the opt-in sentence, got {message:?} in {payload}"
    );
    assert!(
        payload["efficiencyMetricsByGroup"].is_null(),
        "case {case:?}: a refused read reports no groups, got {payload}"
    );
}

/// The groups one served answer reports. A served answer carries the member even
/// where the account has no readable resources, so this also tells a served read
/// from a refused one.
fn served_groups(case: &str, status: u16, payload: &Value) -> Vec<Value> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {payload}"
    );
    payload["efficiencyMetricsByGroup"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list of groups in {payload}"))
        .clone()
}

/// Every well-formed request is refused while the account is not opted into
/// Compute Optimizer, and served once it is — the SAME request, on the same
/// store, with nothing between the two calls but that service's own opt-in.
#[tokio::test]
async fn the_read_is_refused_until_the_account_opts_into_compute_optimizer() {
    let router = router();

    for (case, request) in well_formed_requests() {
        let (status, payload) = call(&router, OPERATION, &request).await;
        assert_opt_in_refusal(&format!("not opted in, {case}"), status, &payload);
    }

    opt_into_compute_optimizer(&router).await;

    for (case, request) in well_formed_requests() {
        let (status, payload) = call(&router, OPERATION, &request).await;
        served_groups(&format!("opted in, {case}"), status, &payload);
    }
}

/// The gate follows the opt-in record rather than remembering that it was once
/// set: an account that opts out through Compute Optimizer is refused again, and
/// opting back in has it served again.
#[tokio::test]
async fn the_gate_follows_the_compute_optimizer_record_in_both_directions() {
    let router = router();
    let (case, request) = well_formed_requests()
        .into_iter()
        .next()
        .expect("the case table is not empty");

    // (the status to write through Compute Optimizer, whether the read is then
    // served): the record moves, and the read moves with it.
    let moves: &[(&str, bool)] = &[
        ("Active", true),
        ("Inactive", false),
        ("Active", true),
        ("Inactive", false),
    ];
    for (status_written, served) in moves {
        let (status, payload) = call_compute_optimizer(
            &router,
            "UpdateEnrollmentStatus",
            &json!({"status": status_written, "includeMemberAccounts": false}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case write {status_written:?}: expected HTTP 200 from Compute Optimizer, got \
             {status} with {payload}"
        );

        // What the OWNING service reports for the account, so the gate below is
        // checked against Compute Optimizer's own read of its own record.
        let (status, reported) =
            call_compute_optimizer(&router, "GetEnrollmentStatus", &json!({})).await;
        assert_eq!(
            status, 200,
            "case write {status_written:?}: expected the enrollment to be readable, got \
             {status} with {reported}"
        );
        assert_eq!(
            reported["status"].as_str(),
            Some(*status_written),
            "case write {status_written:?}: expected Compute Optimizer to report it, got \
             {reported}"
        );

        let (status, payload) = call(&router, OPERATION, &request).await;
        let moved = format!("{case} after writing {status_written:?}");
        if *served {
            served_groups(&moved, status, &payload);
        } else {
            assert_opt_in_refusal(&moved, status, &payload);
        }
    }
}

/// The refusal states the fixed sentence and the operation called, and nothing
/// of the one request that earned it: no account id, no period bound, no
/// granularity, and no field list, because a well-formed request earns it and
/// there is no member for a client to correct.
#[tokio::test]
async fn the_refusal_names_the_operation_and_nothing_per_run() {
    let router = router();

    for (case, request) in well_formed_requests() {
        let (status, payload) = call(&router, OPERATION, &request).await;
        assert_opt_in_refusal(&case, status, &payload);
        let message = payload["message"].as_str().unwrap_or_default();
        let period = &request["timePeriod"];
        for per_run in [
            engine::emulator_account_id(),
            period["start"].as_str().unwrap_or_default(),
            period["end"].as_str().unwrap_or_default(),
            request["granularity"].as_str().unwrap_or_default(),
        ] {
            assert!(
                !message.contains(per_run),
                "case {case:?}: the sentence must not carry the request's {per_run:?}, got \
                 {message:?}"
            );
        }
        assert!(
            payload["fields"].is_null() && payload["reason"].is_null(),
            "case {case:?}: a precondition no member accounts for carries no field \
             diagnosis, got {payload}"
        );
    }
}

/// The precondition belongs to the Compute-Optimizer-derived read alone: every
/// other read and write of this service answers an account that has not opted
/// into Compute Optimizer exactly as before, because none of them reports that
/// service's analysis.
#[tokio::test]
async fn the_other_operations_of_the_service_are_not_gated_on_the_opt_in() {
    let router = router();

    let others: &[(&str, Value)] = &[
        ("ListEnrollmentStatuses", json!({})),
        ("UpdateEnrollmentStatus", json!({"status": "Active"})),
        ("GetPreferences", json!({})),
        ("UpdatePreferences", json!({"savingsEstimationMode": "BeforeDiscounts"})),
        ("ListRecommendations", json!({"includeAllRecommendations": true})),
        (
            "ListRecommendationSummaries",
            json!({"groupBy": "ResourceType"}),
        ),
    ];
    for (operation, request) in others {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected HTTP 200 without the Compute Optimizer opt-in, got \
             {status} with {payload}"
        );
        let message = payload["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains("Compute Optimizer"),
            "case {operation}: expected no opt-in refusal, got {message:?}"
        );
    }
}
