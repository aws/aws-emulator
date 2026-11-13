// SPDX-License-Identifier: CC0-1.0

//! WHAT a recommendation summary of one resource type reports, end to end over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests prove
//! is what a client sees — never what the state struct holds.
//!
//! The rule under test: a recommendation summary of a resource type reports only what the
//! service's analysis OF THAT RESOURCE TYPE produces — a count at each finding the type's own
//! recommendations are classified by, and, only where the analysis reaches a RECOMMENDED
//! CONFIGURATION for the type, the reason codes behind each finding, the savings adopting the
//! recommendations would make and the performance risk the current configurations run. A
//! resource type the service only CLASSIFIES — a database's storage, which carries a
//! `storageFinding` and no recommendation options, costs or risk of its own — is therefore
//! summarised by its finding counts alone, rather than by those blocks reported at zero.
//!
//! The rule generalises over the resource type, so the table below is every resource type a
//! summary can name, each with the findings it is counted at; it is crossed with the states
//! the account's analysis can be in (not due yet, due) and with the page sizes the summaries
//! are read in, because neither changes what a row carries.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::ANALYSIS_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, opt_in};

/// One row of the rule: a `RecommendationSourceType`, the findings a summary of it counts at,
/// and whether the service's analysis of that type reaches a recommended CONFIGURATION (and so
/// whether the summary explains, prices and rates one).
///
/// WHICH findings: the finding vocabulary of that resource type's OWN recommendations, from
/// this service's Smithy model — `EBSFinding` for a volume,
/// `LambdaFunctionRecommendationFinding` for a function, `ECSServiceRecommendationFinding` for
/// an Amazon ECS service, `LicenseFinding` for a license, `RDSInstanceFinding` for a database,
/// `RDSStorageFinding` for its storage, and the `Finding` enum itself (in the EC2 surface's
/// upper-snake spelling) for the rest — each narrowed to the classifications THAT type's
/// analysis can reach: a DB instance's storage is provisioned, so it is `Optimized`,
/// `Overprovisioned` or `Underprovisioned`, while an Aurora cluster's storage grows with the
/// data it holds rather than being sized for it, so its configuration reads `Optimized` or
/// `NotOptimized`
/// (`smithy_query(query="shape",shape="RDSStorageFinding,RDSInstanceFinding,Finding")`).
const SUMMARISED_RESOURCE_TYPES: &[(&str, &[&str], bool)] = &[
    (
        "Ec2Instance",
        &["UNDER_PROVISIONED", "OVER_PROVISIONED", "OPTIMIZED"],
        true,
    ),
    ("AutoScalingGroup", &["NOT_OPTIMIZED", "OPTIMIZED"], true),
    ("EbsVolume", &["NotOptimized", "Optimized"], true),
    ("LambdaFunction", &["NotOptimized", "Optimized"], true),
    (
        "EcsService",
        &["Underprovisioned", "Overprovisioned", "Optimized"],
        true,
    ),
    // A license's analysis reads which license EDITION its workload asks for and reaches a
    // `LicenseFinding` only: `LicenseRecommendation` declares no `currentPerformanceRisk` and
    // no `savingsOpportunity` of its own, so a license is CLASSIFIED and not rightsized. Its
    // vocabulary is the whole of `LicenseFinding`, `InsufficientMetrics` included, which the
    // operation documents for a license ("Commercial software licenses in an account that are
    // InsufficientMetrics, NotOptimized or Optimized") — a classification outside the model's
    // `Finding` enum, so a summary's vocabulary is the resource type's own and not that enum.
    (
        "License",
        &["Optimized", "NotOptimized", "InsufficientMetrics"],
        false,
    ),
    (
        "RdsDBInstance",
        &["Underprovisioned", "Overprovisioned", "Optimized"],
        true,
    ),
    (
        "RdsDBInstanceStorage",
        &["Optimized", "Overprovisioned", "Underprovisioned"],
        false,
    ),
    (
        "AuroraDBClusterStorage",
        &["Optimized", "NotOptimized"],
        false,
    ),
    ("NatGateway", &["NOT_OPTIMIZED", "OPTIMIZED"], true),
    ("DynamoDBTable", &["NOT_OPTIMIZED", "OPTIMIZED"], true),
    ("ElastiCacheCluster", &["NOT_OPTIMIZED", "OPTIMIZED"], true),
    ("MemoryDBCluster", &["NOT_OPTIMIZED", "OPTIMIZED"], true),
    ("DocumentDBCluster", &["NOT_OPTIMIZED", "OPTIMIZED"], true),
    ("WorkSpaces", &["NOT_OPTIMIZED", "OPTIMIZED"], true),
    ("SageMakerEndpoint", &["NOT_OPTIMIZED", "OPTIMIZED"], true),
];

/// The blocks of a summary that describe a recommended configuration: what adopting the
/// recommendations would save, what the aggregate of every kind of recommendation for the
/// type would save, and the risk the current configurations run.
const RECOMMENDATION_BLOCKS: &[&str] = &[
    "savingsOpportunity",
    "aggregatedSavingsOpportunity",
    "currentPerformanceRiskRatings",
];

/// Every summary the service reports, `max_results` at a time when one is asked for, walking
/// the pages to the last.
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
            "case {case:?}: the summaries for {request} must be served, got {status}: {body}"
        );
        summaries.extend(body["recommendationSummaries"].as_array().cloned().unwrap_or_else(
            || panic!("case {case:?}: recommendationSummaries must be a list, got {body}"),
        ));
        next_token = body["nextToken"].as_str().map(ToString::to_string);
        if next_token.is_none() {
            return summaries;
        }
        assert!(
            summaries.len() <= SUMMARISED_RESOURCE_TYPES.len() + 1,
            "case {case:?}: paging must terminate, got {} summaries",
            summaries.len()
        );
    }
}

/// The summary of `resource_type` among `summaries`.
fn summary_of<'a>(case: &str, summaries: &'a [Value], resource_type: &str) -> &'a Value {
    summaries
        .iter()
        .find(|summary| summary["recommendationResourceType"] == json!(resource_type))
        .unwrap_or_else(|| {
            panic!("case {case:?}: {resource_type} must be summarised, got {summaries:?}")
        })
}

/// Assert every row of the rule against one reading of the summaries.
fn assert_rows(case: &str, summaries: &[Value]) {
    for (resource_type, findings, rightsizes) in SUMMARISED_RESOURCE_TYPES {
        let row = format!("{case}/{resource_type}");
        let summary = summary_of(&row, summaries, resource_type);

        let counts = summary["summaries"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {row:?}: expected a count at each of {findings:?}, got {summary}"));
        let reported: Vec<&str> = counts
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, *findings,
            "case {row:?}: expected a count at each finding its own recommendations carry \
             {findings:?}, got {reported:?}"
        );
        for entry in &counts {
            let count = entry["value"].as_f64().unwrap_or_else(|| {
                panic!("case {row:?}: a finding count is a number, got {entry}")
            });
            assert!(
                count >= 0.0 && count.fract() == 0.0,
                "case {row:?}: a finding count is a whole number of resources, got {entry}"
            );
            // A reason code states what a recommendation would CHANGE, so it is reported
            // exactly where the service recommends a configuration for the resource type.
            assert_eq!(
                entry["reasonCodeSummaries"].is_array(),
                *rightsizes,
                "case {row:?}: the reasons behind a finding are reported exactly where the \
                 service recommends a configuration, got {entry}"
            );
        }

        for block in RECOMMENDATION_BLOCKS {
            assert_eq!(
                summary[*block].is_object(),
                *rightsizes,
                "case {row:?}: {block} describes a recommended configuration, so it is reported \
                 exactly where the service recommends one, got {summary}"
            );
        }
        if !*rightsizes {
            // Nothing else of the recommendation surface either: a group the service only
            // classifies has no idle analysis and no idle saving to report.
            for block in ["idleSummaries", "idleSavingsOpportunity", "inferredWorkloadSavings"] {
                assert!(
                    summary[block].is_null(),
                    "case {row:?}: a group the service only classifies reports no {block}, got \
                     {summary}"
                );
            }
        }
        assert_eq!(
            summary["accountId"].as_str(),
            Some(crate::harness::ACCOUNT),
            "case {row:?}: a summary reports the account whose resources it covers, got {summary}"
        );
    }
}

/// The table above is every resource type a summary can name: the whole
/// `RecommendationSourceType` vocabulary the model declares, so a value the model adds cannot
/// be missed by the rows above or left out of the response.
#[tokio::test]
async fn the_rule_covers_every_resource_type_a_summary_can_name() {
    let mut tabled: Vec<&str> = SUMMARISED_RESOURCE_TYPES
        .iter()
        .map(|(resource_type, _, _)| *resource_type)
        .collect();
    tabled.sort_unstable();
    let mut modelled: Vec<&str> = RECOMMENDATION_SOURCE_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        tabled, modelled,
        "case \"the vocabulary\": expected a row per RecommendationSourceType {modelled:?}, got \
         {tabled:?}"
    );

    let router = emulator();
    opt_in(&router).await;
    let summaries = all_summaries(&router, "the vocabulary", None).await;
    let mut summarised: Vec<&str> = summaries
        .iter()
        .map(|summary| {
            summary["recommendationResourceType"]
                .as_str()
                .unwrap_or_default()
        })
        .collect();
    summarised.sort_unstable();
    assert_eq!(
        summarised, modelled,
        "case \"the vocabulary\": expected one summary per RecommendationSourceType \
         {modelled:?}, got {summarised:?}"
    );
}

/// Each summary carries the findings and the blocks its own resource type's analysis produces
/// — whatever the account holds, and however the summaries are paged.
#[tokio::test]
async fn each_summary_carries_what_its_resource_type_s_analysis_produces() {
    let router = emulator();
    opt_in(&router).await;

    // Row: the analysis the enrollment started is not due yet, so every count is zero.
    let fresh = all_summaries(&router, "nothing analysed yet", None).await;
    assert_rows("nothing analysed yet", &fresh);

    // Row: the analysis is due, so the counts are the seeded analysis — and what each row
    // carries is unchanged, because the analysis decides the COUNTS and not the shape.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let analysed = all_summaries(&router, "the analysis is due", None).await;
    assert_rows("the analysis is due", &analysed);

    // Rows: the page sizes the same surface is walked in.
    for size in [1, 2, SUMMARISED_RESOURCE_TYPES.len() as i64] {
        let case = format!("maxResults={size}");
        let walked = all_summaries(&router, &case, Some(size)).await;
        assert_rows(&case, &walked);
        assert_eq!(
            walked, analysed,
            "case {case:?}: paging must walk exactly the unpaged summaries, in order"
        );
    }
}

/// The two storage groups are classified by DIFFERENT vocabularies, so neither is summarised
/// from one shared storage template: the storage a DB instance provisions can be sized for
/// more or less than its workload, while an Aurora cluster's storage is only in the right
/// configuration or not.
#[tokio::test]
async fn each_storage_group_is_counted_at_its_own_classifications() {
    let router = emulator();
    opt_in(&router).await;
    let summaries = all_summaries(&router, "storage vocabularies", None).await;

    let names = |resource_type: &str| -> Vec<String> {
        summary_of("storage vocabularies", &summaries, resource_type)["summaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let instance_storage = names("RdsDBInstanceStorage");
    let cluster_storage = names("AuroraDBClusterStorage");
    assert_ne!(
        instance_storage, cluster_storage,
        "case \"storage vocabularies\": each storage group is counted at its own \
         classifications, got {instance_storage:?} for both"
    );
    assert!(
        instance_storage.iter().any(|name| name == "Overprovisioned")
            && !instance_storage.iter().any(|name| name == "NotOptimized"),
        "case \"RdsDBInstanceStorage\": provisioned storage is classified by how it is sized, \
         got {instance_storage:?}"
    );
    assert!(
        cluster_storage.iter().any(|name| name == "NotOptimized")
            && !cluster_storage
                .iter()
                .any(|name| name == "Overprovisioned" || name == "Underprovisioned"),
        "case \"AuroraDBClusterStorage\": cluster storage is classified by its configuration, \
         not by its size, got {cluster_storage:?}"
    );
}
