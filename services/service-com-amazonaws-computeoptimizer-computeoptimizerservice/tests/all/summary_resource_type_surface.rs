// SPDX-License-Identifier: CC0-1.0

//! WHICH resource types `GetRecommendationSummaries` answers an enrolled account with, end
//! to end over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests prove
//! is what a client sees — never what the state struct holds.
//!
//! The rule under test: a recommendation summary is a COUNT ROLL-UP per resource type —
//! `Summary$value` is a count whose default is `0` — so an enrolled account is answered one
//! summary per resource type the service reports recommendations for (the model's
//! `RecommendationSourceType` vocabulary), with counts of zero for a resource type nothing of
//! which has been analysed, rather than only the resource types that currently carry a
//! finding. The analysis an enrollment starts therefore decides what the counts ARE and never
//! which resource types are summarised.
//!
//! The rule generalises over what the store holds, so the rows below are the states the
//! account's analysis can be in — nothing analysed yet, the seeded analysis, the seeded
//! analysis plus a recorded idle resource of a resource type the rightsizing catalog holds
//! nothing of — crossed with reading the summaries in one page and one at a time.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, IdleResourceAnalysis, ANALYSIS_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator_with_engine, opt_in};

/// The account every caller of this emulator is (`engine::emulator_account_id`), and so the
/// account a summary covers.
fn caller_account() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The findings a summary of each resource type reports a count at, under the name the
/// summary reports them with.
///
/// WHICH findings: the `GetRecommendationSummaries` documentation in this service's Smithy
/// model — "It returns the number of: Amazon EC2 instances in an account that are
/// Underprovisioned, Overprovisioned, or Optimized. EC2 Auto Scaling groups in an account
/// that are NotOptimized, or Optimized. Amazon EBS volumes in an account that are
/// NotOptimized, or Optimized. Lambda functions in an account that are NotOptimized, or
/// Optimized. Amazon ECS services in an account that are Underprovisioned, Overprovisioned,
/// or Optimized. Commercial software licenses in an account that are InsufficientMetrics,
/// NotOptimized or Optimized. Amazon Aurora and Amazon RDS databases in an account that are
/// Underprovisioned, Overprovisioned, Optimized, or NotOptimized." Each group is counted
/// at the vocabulary of its OWN recommendations, so a database instance carries the three
/// `RDSInstanceFinding` values while its storage carries the four of `RDSStorageFinding`, and
/// a license carries the three `LicenseFinding` classifications the documentation names for
/// one — `InsufficientMetrics` included, which the model's `Finding` enum does not list. A
/// resource type the documentation names no vocabulary for is counted at the generic
/// classification pair.
///
/// Under WHICH NAME: a resource type whose recommendations carry a finding vocabulary of
/// their own is counted in that vocabulary's spelling (`EBSFinding`,
/// `LambdaFunctionRecommendationFinding`, `ECSServiceRecommendationFinding`,
/// `RDSInstanceFinding`, `RDSStorageFinding`, `LicenseFinding` — all camel case), and every
/// other one at the `Finding` vocabulary as the instance surface spells it, in upper snake
/// case. A vocabulary is narrowed to the classifications THAT resource type's analysis can
/// reach, which is why a DB instance's storage carries the three provisioning
/// classifications while an Aurora cluster's storage — which the service does not size —
/// carries only the `Optimized`/`NotOptimized` pair.
const FINDINGS_BY_RESOURCE_TYPE: &[(&str, &[&str])] = &[
    (
        "Ec2Instance",
        &["UNDER_PROVISIONED", "OVER_PROVISIONED", "OPTIMIZED"],
    ),
    ("AutoScalingGroup", &["NOT_OPTIMIZED", "OPTIMIZED"]),
    ("EbsVolume", &["NotOptimized", "Optimized"]),
    ("LambdaFunction", &["NotOptimized", "Optimized"]),
    (
        "EcsService",
        &["Underprovisioned", "Overprovisioned", "Optimized"],
    ),
    (
        "License",
        &["Optimized", "NotOptimized", "InsufficientMetrics"],
    ),
    (
        "RdsDBInstance",
        &["Underprovisioned", "Overprovisioned", "Optimized"],
    ),
    (
        "RdsDBInstanceStorage",
        &["Optimized", "Overprovisioned", "Underprovisioned"],
    ),
    ("AuroraDBClusterStorage", &["Optimized", "NotOptimized"]),
    ("NatGateway", &["NOT_OPTIMIZED", "OPTIMIZED"]),
    ("DynamoDBTable", &["NOT_OPTIMIZED", "OPTIMIZED"]),
    ("ElastiCacheCluster", &["NOT_OPTIMIZED", "OPTIMIZED"]),
    ("MemoryDBCluster", &["NOT_OPTIMIZED", "OPTIMIZED"]),
    ("DocumentDBCluster", &["NOT_OPTIMIZED", "OPTIMIZED"]),
    ("WorkSpaces", &["NOT_OPTIMIZED", "OPTIMIZED"]),
    ("SageMakerEndpoint", &["NOT_OPTIMIZED", "OPTIMIZED"]),
];

/// One idle Amazon DynamoDB table to record into the analysis: a resource of a type the
/// rightsizing catalog holds nothing of, so the row where it is recorded differs from the
/// others in which resource types carry a record at all.
fn record_idle_table(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    state.lock().idle_recommendations.insert(IdleResourceAnalysis {
        account_id: caller_account(),
        arn_service: "dynamodb".to_string(),
        arn_resource: "table/retired-sessions".to_string(),
        finding: "Unused".to_string(),
        utilization_metrics: Vec::new(),
        days_observed: 30.0,
        monthly_cost: 18.75,
        tags: Vec::new(),
    });
}

/// The summaries of one page, as a list.
fn summaries_of(case: &str, body: &Value) -> Vec<Value> {
    body["recommendationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: recommendationSummaries must be a list, got {body}"))
        .clone()
}

/// Read every summary the service reports, `max_results` at a time when one is asked for,
/// walking the pages to the last.
async fn all_summaries(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["maxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(router, "GetRecommendationSummaries", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: summaries for {request} must be served, got {status}: {body}"
        );
        summaries.extend(summaries_of(case, &body));
        next_token = body["nextToken"].as_str().map(ToString::to_string);
        if next_token.is_none() {
            return summaries;
        }
        assert!(
            summaries.len() <= RECOMMENDATION_SOURCE_TYPE_VALUES.len() + 1,
            "case {case:?}: paging must terminate, got {} summaries",
            summaries.len()
        );
    }
}

/// A number member of a response, which the wire carries as a JSON number.
fn number(value: &Value, case: &str) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a number, got {value}"))
}

/// The resource types a list of summaries covers, in the order reported.
fn resource_types(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["recommendationResourceType"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// How many idle resources `summary` counts across the `IdleFinding` values it reports; a
/// resource type the idle analysis does not cover reports none at all.
fn idle_counts(summary: &Value, case: &str) -> f64 {
    summary["idleSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| number(&entry["value"], case))
        .sum()
}

/// How many resources `summary` counts across the `Finding` values it reports, plus the
/// `CurrentPerformanceRisk` ratings it rates and the idle resources it counts. A summary that
/// states no ratings at all — a group the service only classifies — rates nothing.
fn counted(summary: &Value, case: &str) -> f64 {
    let findings: f64 = summary["summaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| number(&entry["value"], case))
        .sum();
    let rated: f64 = ["high", "medium", "low", "veryLow"]
        .iter()
        .map(|rating| {
            let value = &summary["currentPerformanceRiskRatings"][*rating];
            if value.is_null() {
                0.0
            } else {
                number(value, case)
            }
        })
        .sum();
    let idle: f64 = summary["idleSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| number(&entry["value"], case))
        .sum();
    findings + rated + idle
}

/// Assert `summaries` is one entry per resource type the service reports recommendations for,
/// each the caller's own account's, each carrying a count at every `Finding` the service
/// documents for its resource type — whatever the account's analysis holds.
fn assert_rolls_up_every_resource_type(case: &str, summaries: &[Value]) {
    let reported = resource_types(summaries);
    let expected: Vec<String> = RECOMMENDATION_SOURCE_TYPE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    let reported_set: BTreeSet<&String> = reported.iter().collect();
    assert_eq!(
        reported_set.len(),
        reported.len(),
        "case {case:?}: a resource type is summarised once, got {reported:?}"
    );
    let mut sorted = reported.clone();
    sorted.sort();
    let mut wanted = expected.clone();
    wanted.sort();
    assert_eq!(
        sorted, wanted,
        "case {case:?}: expected a summary per RecommendationSourceType {wanted:?}, got \
         {sorted:?}"
    );

    for summary in summaries {
        let resource_type = summary["recommendationResourceType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let row = format!("{case}/{resource_type}");
        assert_eq!(
            summary["accountId"].as_str(),
            Some(caller_account().as_str()),
            "case {row:?}: a summary reports the account whose resources it covers, got {summary}"
        );

        let findings = FINDINGS_BY_RESOURCE_TYPE
            .iter()
            .find(|(name, _)| *name == resource_type)
            .map(|(_, findings)| *findings)
            .unwrap_or_else(|| {
                panic!("case {row:?}: unknown resource type in {summary}");
            });
        let counts = summary["summaries"].as_array().cloned().unwrap_or_else(|| {
            panic!("case {row:?}: expected a count at each of {findings:?}, got {summary}")
        });
        let names: Vec<&str> = counts
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names, *findings,
            "case {row:?}: expected a count at each documented finding {findings:?}, got \
             {names:?}"
        );
        for entry in &counts {
            let count = number(&entry["value"], &row);
            assert!(
                count >= 0.0 && count.fract() == 0.0,
                "case {row:?}: a finding count is a whole number of resources, got {entry}"
            );
        }
    }
}

/// The summary surface of an enrolled account is the resource types the service reports
/// recommendations for, in every state its analysis can be in and however the pages are
/// walked — including the state the issue's own client reads: opted in, with the analysis the
/// enrollment started not due yet, where every count is zero and no entry is missing.
#[tokio::test]
async fn an_enrolled_account_is_summarised_per_resource_type_the_service_reports() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;

    // Row: nothing analysed yet — the analysis the enrollment started is not due, so the
    // roll-up counts nothing at all while still covering every resource type.
    let fresh = all_summaries(&router, "nothing analysed yet", None).await;
    assert_rolls_up_every_resource_type("nothing analysed yet", &fresh);
    for summary in &fresh {
        let row = format!(
            "nothing analysed yet/{}",
            summary["recommendationResourceType"]
        );
        assert_eq!(
            counted(summary, &row),
            0.0,
            "case {row:?}: no resource has been analysed, so every count is zero, got {summary}"
        );
        // A group the service recommends no configuration for states no rightsizing saving at
        // all; where one is stated, with nothing analysed it is nothing.
        let stated_saving = &summary["savingsOpportunity"]["estimatedMonthlySavings"]["value"];
        if !stated_saving.is_null() {
            assert_eq!(
                number(stated_saving, &row),
                0.0,
                "case {row:?}: with nothing analysed there is nothing to save, got {summary}"
            );
        }
        assert_eq!(
            idle_counts(summary, &row),
            0.0,
            "case {row:?}: nothing was found idle, so every idle count is zero, got {summary}"
        );
    }

    // Row: the analysis the enrollment started is due, so the counts are the seeded analysis
    // — and the surface is unchanged, including the resource types it holds nothing of.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let analysed = all_summaries(&router, "the analysis is due", None).await;
    assert_rolls_up_every_resource_type("the analysis is due", &analysed);
    assert_eq!(
        resource_types(&analysed),
        resource_types(&fresh),
        "the analysis decides what the counts are, not which resource types are summarised"
    );
    let with_rows = analysed
        .iter()
        .filter(|summary| counted(summary, "the analysis is due") > 0.0)
        .count();
    assert!(
        with_rows > 0 && with_rows < analysed.len(),
        "this fixture must analyse resources of some but not all resource types for this to \
         prove anything, got {with_rows} of {} summarised",
        analysed.len()
    );

    // Row: an idle resource recorded of a resource type the rightsizing catalog holds nothing
    // of — one more resource type carries a record, and the surface is still the same.
    record_idle_table(&engine);
    let recorded = all_summaries(&router, "an idle resource recorded", None).await;
    assert_rolls_up_every_resource_type("an idle resource recorded", &recorded);
    let table = recorded
        .iter()
        .find(|summary| summary["recommendationResourceType"] == json!("DynamoDBTable"))
        .unwrap_or_else(|| panic!("the vocabulary covers DynamoDBTable, got {recorded:?}"));
    assert!(
        counted(table, "an idle resource recorded/DynamoDBTable") > 0.0,
        "case \"an idle resource recorded/DynamoDBTable\": the recorded idle resource must be \
         counted, got {table}"
    );

    // Rows: the page sizes the same surface is walked in — one at a time, two at a time, and
    // a page that holds the whole vocabulary.
    for size in [1, 2, RECOMMENDATION_SOURCE_TYPE_VALUES.len() as i64] {
        let case = format!("maxResults={size}");
        let walked = all_summaries(&router, &case, Some(size)).await;
        assert_rolls_up_every_resource_type(&case, &walked);
        assert_eq!(
            walked, recorded,
            "case {case:?}: paging must walk exactly the unpaged summaries, in order"
        );
    }
}
