// SPDX-License-Identifier: CC0-1.0

//! The page-size bound each operation declares, driven end-to-end over the
//! wire (`awsJson1_0`) through the same multi-service router the emulator
//! boots.
//!
//! The rule under test: a member is bounded by the `@range` the MEMBER
//! declares, which overrides the range of the shape it targets — so every
//! paginated operation enforces its own `maxResults` bound, and two operations
//! sharing the `MaxResults` shape can have different ones
//! (`smithy_query(query="operation", operation="ListRecommendations,ListEnrollmentStatuses,ListRecommendationSummaries,ListEfficiencyMetrics")`
//! gives `maxResults` `range` 0..1000 for three of them and 0..100 for
//! `ListEnrollmentStatuses`; `smithy_query(query="shape", shape="MaxResults")`
//! gives the shape's own 0..100).
//!
//! Asserted over a table holding, for every paginated operation of this
//! service, each published bound (accepted) and one value past each (refused
//! with HTTP 400 `ValidationException`, whose `fields` entry states the bound
//! that operation declares).

use serde_json::{json, Value};

use crate::cost_optimization_hub::{
    call, compute_optimizer_opted_in_router, error_code, fields, monthly_period, router,
    FIELD_VALIDATION_FAILED, FIELD_VALIDATION_SUMMARY,
};

/// One paginated operation: its name, the rest of a well-formed request for it,
/// the bounds its own `maxResults` member declares, and the status a request
/// inside those bounds gets.
struct Paginated {
    operation: &'static str,
    request: Value,
    min: i64,
    max: i64,
    served: u16,
}

fn paginated_operations() -> Vec<Paginated> {
    // The period the time-series read is asked over comes from the clock: that
    // operation reports over a window ending at the present interval.
    let (start, end) = monthly_period(1, 1);
    vec![
        Paginated {
            operation: "ListRecommendations",
            request: json!({"includeAllRecommendations": true}),
            min: 0,
            max: 1000,
            served: 200,
        },
        Paginated {
            operation: "ListEnrollmentStatuses",
            request: json!({"includeOrganizationInfo": false}),
            min: 0,
            max: 100,
            served: 200,
        },
        Paginated {
            operation: "ListRecommendationSummaries",
            request: json!({"groupBy": "ActionType", "metrics": ["SavingsPercentage"]}),
            min: 0,
            max: 1000,
            served: 200,
        },
        Paginated {
            operation: "ListEfficiencyMetrics",
            request: json!({
                "granularity": "Monthly",
                "timePeriod": {"start": start, "end": end},
            }),
            min: 0,
            max: 1000,
            served: 200,
        },
    ]
}

/// A `maxResults` at either published bound of the operation's own `@range` is
/// served — including the documented maximum, which is the page size a client
/// paging at the limit sends on every call.
///
/// The account is opted into AWS Compute Optimizer, the precondition the
/// efficiency-metrics read of the table states before it is served at all
/// (`efficiency_metrics_compute_optimizer_gate` owns that rule): this suite is
/// about the page-size bound, so the request must reach it.
#[tokio::test]
async fn each_operation_accepts_its_own_published_page_size() {
    let router = compute_optimizer_opted_in_router().await;

    for op in paginated_operations() {
        // Both bounds, and a value strictly inside the range.
        for max_results in [op.min, op.max, (op.min + op.max) / 2] {
            let mut request = op.request.clone();
            request["maxResults"] = json!(max_results);
            let case = format!("{} maxResults {max_results}", op.operation);
            let (status, payload) = call(&router, op.operation, &request).await;
            assert_eq!(
                status, op.served,
                "case {case:?}: expected HTTP {}, got {status} with {payload}",
                op.served
            );
            assert!(
                payload["fields"].is_null(),
                "case {case:?}: a page size inside the bound carries no field list, got {payload}"
            );
        }
    }
}

/// A `maxResults` past either published bound is refused with HTTP 400
/// `ValidationException`, whose single `fields` entry names the member and
/// states the bound THAT operation declares.
#[tokio::test]
async fn each_operation_refuses_a_page_size_past_its_own_bound() {
    let router = router();

    for op in paginated_operations() {
        // (the value sent, the clause the field entry must state)
        let cases: [(i64, String); 2] = [
            (
                op.max + 1,
                format!("Member must have value less than or equal to {}", op.max),
            ),
            (
                op.min - 1,
                format!("Member must have value greater than or equal to {}", op.min),
            ),
        ];
        for (max_results, clause) in &cases {
            let mut request = op.request.clone();
            request["maxResults"] = json!(max_results);
            let case = format!("{} maxResults {max_results}", op.operation);
            let (status, payload) = call(&router, op.operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            let code = error_code(&payload);
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected a ValidationException, got {code:?} in {payload}"
            );
            assert_eq!(
                payload["message"].as_str(),
                Some(FIELD_VALIDATION_SUMMARY),
                "case {case:?}: expected the fixed summary, got {payload}"
            );
            assert_eq!(
                payload["reason"].as_str(),
                Some(FIELD_VALIDATION_FAILED),
                "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {payload}"
            );
            assert_eq!(
                fields(&payload),
                vec![("maxResults".to_string(), clause.clone())],
                "case {case:?}: expected one entry stating {clause:?}, got {payload}"
            );
        }
    }
}

/// The bound is the operation's own, so the two operations that share the
/// `MaxResults` shape disagree about one page size: the shape's maximum plus
/// one is served by the operation whose member allows it and refused by the
/// one whose member does not.
#[tokio::test]
async fn two_operations_sharing_one_shape_keep_their_own_bounds() {
    let router = router();
    // One past `MaxResults`' own maximum (100), and the largest page
    // `ListRecommendations` declares (1000).
    for max_results in [101, 1000] {
        let served = call(
            &router,
            "ListRecommendations",
            &json!({"includeAllRecommendations": true, "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            served.0, 200,
            "case ListRecommendations maxResults {max_results}: expected HTTP 200, got {} with {}",
            served.0, served.1
        );

        let refused = call(
            &router,
            "ListEnrollmentStatuses",
            &json!({"includeOrganizationInfo": false, "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            refused.0, 400,
            "case ListEnrollmentStatuses maxResults {max_results}: expected HTTP 400, got {} with {}",
            refused.0, refused.1
        );
    }
}
