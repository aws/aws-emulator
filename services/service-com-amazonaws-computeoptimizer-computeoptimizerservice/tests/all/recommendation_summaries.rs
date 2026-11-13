// SPDX-License-Identifier: CC0-1.0

//! `GetRecommendationSummaries`, end to end over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests
//! prove is what a client sees — never what the state struct holds.
//!
//! The rule under test: a summary is the account's analysed resources OF ONE RESOURCE TYPE
//! aggregated — a count at each `Finding` the service reports for that resource type (and
//! at each `FindingReasonCode` behind it), a count at each `CurrentPerformanceRisk`, a
//! count at each `IdleFinding` found, and what adopting the recommendations would save per
//! month with that saving as a percentage of what the resources cost. So every figure in
//! one summary is a different view of the SAME rows, and the assertions below are the
//! identities that holds: the risk ratings total the finding counts, a reason code is
//! counted no more often than the finding it explains, and the aggregated saving is the
//! rightsizing saving plus the idle one.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    reports_rightsizing_recommendations, ANALYSIS_DELAY,
};

use crate::harness::{call, emulator, error_code, opt_in};

/// The account every caller of this emulator is (`engine::emulator_account_id`), and so
/// the account whose findings a summary reports.
fn caller_account() -> String {
    engine::emulator_account_id().to_string()
}

/// The findings a summary of each resource type reports a count at, under the name the
/// summary reports them with.
///
/// WHICH findings: the `GetRecommendationSummaries` documentation in this service's Smithy
/// model — "It returns the number of: Amazon EC2 instances in an account that are
/// Underprovisioned, Overprovisioned, or Optimized. EC2 Auto Scaling groups in an account
/// that are NotOptimized, or Optimized. Amazon EBS volumes in an account that are
/// NotOptimized, or Optimized. Lambda functions in an account that are NotOptimized, or
/// Optimized. Amazon ECS services in an account that are Underprovisioned,
/// Overprovisioned, or Optimized. Commercial software licenses in an account that are
/// InsufficientMetrics, NotOptimized or Optimized. Amazon Aurora and Amazon RDS databases
/// in an account that are Underprovisioned, Overprovisioned, Optimized, or NotOptimized" —
/// each group counted at the vocabulary of its OWN recommendations, which is why a database
/// instance is counted at the three `RDSInstanceFinding` values while its storage is counted
/// at the `RDSStorageFinding` values THAT storage can reach — the three provisioning
/// classifications for the storage a DB instance provisions, and the `Optimized`/`NotOptimized`
/// pair for an Aurora cluster's storage, which the service does not size — and a license at
/// the three `LicenseFinding` classifications the documentation names for one,
/// `InsufficientMetrics` included, a classification the model's `Finding` enum does not list.
/// A tracked resource
/// type the documentation names no vocabulary for is counted at the generic classification
/// pair.
///
/// Under WHICH NAME: a resource type whose recommendations carry a finding vocabulary of
/// their own is counted in that vocabulary's spelling (`EBSFinding`,
/// `LambdaFunctionRecommendationFinding`, `ECSServiceRecommendationFinding`,
/// `RDSInstanceFinding`, `RDSStorageFinding`, `LicenseFinding` — all camel case), and every
/// other one at the `Finding` vocabulary as the instance surface spells it, in upper snake
/// case.
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

/// The `RecommendationSourceType` values the IDLE analysis covers, and so the groups whose
/// summary carries `idleSummaries` and `idleSavingsOpportunity` — whether or not the account
/// holds an idle resource of them.
///
/// Source: the model's `IdleRecommendationResourceType` enum
/// (`crate::ops::types::IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES`) under the summary's own
/// spelling of the same resources (`EC2Instance` against `Ec2Instance`, `EBSVolume` against
/// `EbsVolume`, `ECSService` against `EcsService`, `RDSDBInstance` against `RdsDBInstance`).
const IDLE_COVERED_RESOURCE_TYPES: &[&str] = &[
    "Ec2Instance",
    "AutoScalingGroup",
    "EbsVolume",
    "EcsService",
    "RdsDBInstance",
    "NatGateway",
    "DynamoDBTable",
    "ElastiCacheCluster",
    "MemoryDBCluster",
    "DocumentDBCluster",
    "WorkSpaces",
    "SageMakerEndpoint",
];

/// The model's `IdleFinding` enum (`crate::ops::types::IDLE_FINDING_VALUES`), the only
/// values `IdleSummary$name` can carry.
const IDLE_FINDINGS: &[&str] = &["Idle", "Unattached", "Unused"];

/// The model's `FindingReasonCode` enum (`crate::ops::types::FINDING_REASON_CODE_VALUES`),
/// the only values `ReasonCodeSummary$name` can carry.
const FINDING_REASON_CODES: &[&str] = &["MemoryOverprovisioned", "MemoryUnderprovisioned"];

/// The model's `InferredWorkloadType` enum
/// (`crate::ops::types::INFERRED_WORKLOAD_TYPE_VALUES`).
const INFERRED_WORKLOAD_TYPES: &[&str] = &[
    "AmazonEmr",
    "ApacheCassandra",
    "ApacheHadoop",
    "Kafka",
    "Memcached",
    "Nginx",
    "PostgreSql",
    "Redis",
    "SQLServer",
];

/// An emulator whose account is opted in and whose analysis is due: a summary covers the
/// account's analysed resources, and the service holds that analysis only once the analysis
/// the enrollment started is due ([`ANALYSIS_DELAY`]); the extra margin covers scheduler
/// jitter.
async fn analysed_emulator() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    router
}

/// Read one page of summaries, asserting the call succeeded.
async fn get_summaries(router: &smithy_server_runtime::MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "GetRecommendationSummaries", request.clone()).await;
    assert_eq!(
        status, 200,
        "summaries for {request}: expected 200, got {status}: {body}"
    );
    body
}

/// The summaries of one response, as a list.
fn summaries_of(body: &Value) -> Vec<Value> {
    body["recommendationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("recommendationSummaries must be a list, got {body}"))
        .clone()
}

/// A `number` member of a response object, which the wire carries as a JSON number.
fn number(value: &Value, case: &str) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a number, got {value}"))
}

/// Before the account opts in there are no findings to summarise at all, and the service
/// says so with the modelled `OptInRequiredException` (HTTP 403) rather than an empty
/// answer; an opted-in account is summarised per resource type the service reports
/// recommendations for, whether or not the analysis its enrollment started is due yet, and
/// every summary reported is the caller's own account's.
#[tokio::test]
async fn summaries_need_the_account_to_be_opted_in_and_report_the_caller_account() {
    let router = emulator();

    let (status, body) = call(&router, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(
        status, 403,
        "before opting in: expected 403, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "before opting in: expected OptInRequiredException, got {body}"
    );

    opt_in(&router).await;
    // Rows: the two phases of one enrollment — the analysis not due yet, and due. The set of
    // summaries is the same in both, because a summary is a count roll-up per resource type
    // and the analysis decides what those counts ARE, not whether the resource type is
    // summarised.
    for phase in ["just opted in", "the analysis is due"] {
        if phase == "the analysis is due" {
            tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
        }
        let body = get_summaries(&router, json!({})).await;
        let summaries = summaries_of(&body);
        assert!(
            !summaries.is_empty(),
            "case {phase:?}: an opted-in account is summarised per resource type the service \
             reports recommendations for, got {body}"
        );
        for summary in &summaries {
            let resource_type = summary["recommendationResourceType"]
                .as_str()
                .unwrap_or_default();
            let case = format!("{phase}/{resource_type}");
            assert_eq!(
                summary["accountId"].as_str(),
                Some(caller_account().as_str()),
                "case {case:?}: a summary reports the account whose resources it covers, got \
                 {summary}"
            );
            assert!(
                RECOMMENDATION_SOURCE_TYPE_VALUES.contains(&resource_type),
                "case {case:?}: recommendationResourceType must be a RecommendationSourceType, \
                 got {summary}"
            );
        }
    }

    // Opting back out takes the findings away again: the gate is the stored enrollment,
    // not whether the account ever enrolled.
    let (status, body) = call(
        &router,
        "UpdateEnrollmentStatus",
        json!({"status": "Inactive"}),
    )
    .await;
    assert_eq!(status, 200, "opting out: expected 200, got {status}: {body}");
    let (status, body) = call(&router, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(
        status, 403,
        "after opting out: expected 403, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "after opting out: expected OptInRequiredException, got {body}"
    );
}

/// Each summary is one resource type's analysed resources aggregated, so its parts are the
/// identities of that one set of rows: the findings it reports are exactly the ones the
/// service documents for that resource type, the performance-risk ratings total the
/// finding counts, a reason code is counted no more often than the finding it explains,
/// the idle counts are a subset of the resources, and the aggregated saving is the
/// rightsizing saving plus the idle one — every saving in USD, every percentage within
/// 0..=100 and zero exactly when the saving is zero.
#[tokio::test]
async fn every_summary_is_its_resource_type_s_resources_counted_and_costed() {
    let router = analysed_emulator().await;

    let body = get_summaries(&router, json!({})).await;
    let summaries = summaries_of(&body);
    assert!(
        summaries.len() > 1,
        "the catalog must span more than one resource type for this to prove anything, got \
         {body}"
    );

    for summary in &summaries {
        let resource_type = summary["recommendationResourceType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let case = resource_type.clone();
        let expected_findings: &[&str] = FINDINGS_BY_RESOURCE_TYPE
            .iter()
            .find(|(name, _)| *name == resource_type)
            .map(|(_, findings)| *findings)
            .unwrap_or_else(|| {
                panic!("case {case:?}: unknown resource type in {summary}");
            });

        let finding_summaries = summary["summaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: summaries must be a list, got {summary}"))
            .clone();
        let reported: Vec<&str> = finding_summaries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, *expected_findings,
            "case {case:?}: expected a count at each documented finding {expected_findings:?}, \
             got {reported:?}"
        );

        let mut resources = 0.0;
        for entry in &finding_summaries {
            let finding = entry["name"].as_str().unwrap_or_default().to_string();
            let count = number(&entry["value"], &format!("{case}/{finding}"));
            assert!(
                count >= 0.0 && count.fract() == 0.0,
                "case {case:?}/{finding:?}: a finding count is a whole number of resources, got \
                 {count}"
            );
            resources += count;

            if let Some(reason_codes) = entry["reasonCodeSummaries"].as_array() {
                let reported: Vec<&str> = reason_codes
                    .iter()
                    .map(|reason| reason["name"].as_str().unwrap_or_default())
                    .collect();
                assert_eq!(
                    reported, *FINDING_REASON_CODES,
                    "case {case:?}/{finding:?}: expected a count at each FindingReasonCode \
                     {FINDING_REASON_CODES:?}, got {reported:?}"
                );
                for reason in reason_codes {
                    let reason_count = number(&reason["value"], &format!("{case}/{finding}"));
                    assert!(
                        reason_count >= 0.0 && reason_count <= count,
                        "case {case:?}/{finding:?}: a reason code explains at most the {count} \
                         resources at that finding, got {reason_count}"
                    );
                }
            }
        }
        let idle_counts = summary["idleSummaries"].as_array().cloned();

        // The reason codes behind a finding, the performance-risk ratings and every savings
        // opportunity describe a RECOMMENDED CONFIGURATION, so a group the service only
        // CLASSIFIES carries none of them — its summary is the finding counts alone.
        if !reports_rightsizing_recommendations(&resource_type) {
            for entry in &finding_summaries {
                assert!(
                    entry["reasonCodeSummaries"].is_null(),
                    "case {case:?}: a group the service only classifies states no reason for a \
                     finding, got {entry}"
                );
            }
            for member in [
                "savingsOpportunity",
                "idleSavingsOpportunity",
                "aggregatedSavingsOpportunity",
                "currentPerformanceRiskRatings",
                "idleSummaries",
            ] {
                assert!(
                    summary[member].is_null(),
                    "case {case:?}: a group the service only classifies reports no {member}, got \
                     {summary}"
                );
            }
            continue;
        }

        let ratings = &summary["currentPerformanceRiskRatings"];
        let rated = ["high", "medium", "low", "veryLow"]
            .iter()
            .map(|rating| number(&ratings[*rating], &format!("{case}/{rating}")))
            .sum::<f64>();
        assert_eq!(
            rated, resources,
            "case {case:?}: every analysed resource runs exactly one performance risk, so the \
             ratings must total the {resources} resources counted by finding, got {ratings}"
        );

        // An idle resource is a record of its own — the analysis `GetIdleRecommendations`
        // reports — and a summary describes it beside the rightsizing findings: a count at
        // each `IdleFinding` for every resource type the idle analysis covers, zero where
        // none of the account's resources of it was found idle, and no idle member at all
        // for a resource type that analysis does not cover.
        let covers_idle = IDLE_COVERED_RESOURCE_TYPES.contains(&resource_type.as_str());
        match idle_counts {
            Some(idle_counts) => {
                assert!(
                    covers_idle,
                    "case {case:?}: the idle analysis does not cover this resource type, so it \
                     has no idle counts to report, got {summary}"
                );
                let names: Vec<&str> = idle_counts
                    .iter()
                    .map(|entry| entry["name"].as_str().unwrap_or_default())
                    .collect();
                assert_eq!(
                    names, *IDLE_FINDINGS,
                    "case {case:?}: expected a count at each IdleFinding {IDLE_FINDINGS:?}, got \
                     {names:?}"
                );
                for entry in &idle_counts {
                    let finding = entry["name"].as_str().unwrap_or_default().to_string();
                    let count = number(&entry["value"], &format!("{case}/{finding}"));
                    assert!(
                        count >= 0.0 && count.fract() == 0.0,
                        "case {case:?}/{finding:?}: an idle count is a whole number of \
                         resources, got {count}"
                    );
                }
            }
            None => {
                assert!(
                    !covers_idle,
                    "case {case:?}: the idle analysis covers this resource type, so its summary \
                     reports a count at each IdleFinding, got {summary}"
                );
                assert!(
                    summary["idleSavingsOpportunity"].is_null(),
                    "case {case:?}: a resource type the idle analysis does not cover has no \
                     idle saving to report either, got {summary}"
                );
            }
        }

        let rightsizing = savings_value(summary, "savingsOpportunity", &case);
        let idle = if covers_idle {
            savings_value(summary, "idleSavingsOpportunity", &case)
        } else {
            0.0
        };
        let aggregated = savings_value(summary, "aggregatedSavingsOpportunity", &case);
        assert!(
            (aggregated - (rightsizing + idle)).abs() < 0.01,
            "case {case:?}: the aggregated saving is the rightsizing saving {rightsizing} plus \
             the idle one {idle}, got {aggregated}"
        );

        if let Some(inferred) = summary["inferredWorkloadSavings"].as_array() {
            assert!(
                !inferred.is_empty() && inferred.len() <= 5,
                "case {case:?}: inferredWorkloadSavings reports the top five opportunities at \
                 most, got {} of them",
                inferred.len()
            );
            let mut previous = f64::INFINITY;
            for entry in inferred {
                let types = entry["inferredWorkloadTypes"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a type list in {entry}"))
                    .clone();
                assert!(
                    !types.is_empty(),
                    "case {case:?}: an inferred-workload saving names the workload types it is \
                     for, got {entry}"
                );
                for workload in &types {
                    let name = workload.as_str().unwrap_or_default();
                    assert!(
                        INFERRED_WORKLOAD_TYPES.contains(&name),
                        "case {case:?}: {name:?} is not an InferredWorkloadType, got {entry}"
                    );
                }
                let value = number(&entry["estimatedMonthlySavings"]["value"], &case);
                assert!(
                    value > 0.0 && value <= aggregated,
                    "case {case:?}: an inferred-workload saving is part of the {aggregated} this \
                     resource type can save, got {value}"
                );
                assert!(
                    value <= previous,
                    "case {case:?}: inferred-workload savings are reported largest first, got \
                     {value} after {previous}"
                );
                previous = value;
            }
        }
    }
}

/// One savings opportunity of a summary, asserting the invariants every one of them holds:
/// a non-negative amount in a currency the model names, and a percentage of monthly cost
/// within 0..=100 that is zero exactly when the amount is.
fn savings_value(summary: &Value, member: &str, case: &str) -> f64 {
    let opportunity = &summary[member];
    let value = number(&opportunity["estimatedMonthlySavings"]["value"], case);
    assert!(
        value >= 0.0,
        "case {case:?}/{member}: a saving is never negative, got {value}"
    );
    // The model's `Currency` enum (`crate::ops::types::CURRENCY_VALUES`).
    let currency = opportunity["estimatedMonthlySavings"]["currency"]
        .as_str()
        .unwrap_or_default();
    assert!(
        ["USD", "CNY"].contains(&currency),
        "case {case:?}/{member}: {currency:?} is not a Currency, got {opportunity}"
    );
    let percentage = number(&opportunity["savingsOpportunityPercentage"], case);
    assert!(
        (0.0..=100.0).contains(&percentage),
        "case {case:?}/{member}: a saving is a percentage of monthly cost, got {percentage}"
    );
    assert_eq!(
        percentage == 0.0,
        value == 0.0,
        "case {case:?}/{member}: a percentage is zero exactly when the saving is, got \
         {percentage} for {value}"
    );
    value
}

/// `accountIds` selects WHOSE findings are summarised: absent or the caller's own account
/// reads the caller's, any other account is refused (this emulator presents one standalone
/// account and records no organization, so no other account is a member account it can be
/// asked about), and more than one account id is refused — "Only one account ID can be
/// specified per request."
#[tokio::test]
async fn account_ids_selects_one_account_and_refuses_more_than_one() {
    let router = analysed_emulator().await;

    let own = caller_account();
    let unpaged = summaries_of(&get_summaries(&router, json!({})).await);

    // Rows: the member is absent, empty, and names the caller — each reads the caller's own
    // summaries.
    let cases: [(&str, Value); 3] = [
        ("absent", json!({})),
        ("empty", json!({"accountIds": []})),
        ("own account", json!({"accountIds": [own.clone()]})),
    ];
    for (case, request) in cases {
        let body = get_summaries(&router, request.clone()).await;
        assert_eq!(
            summaries_of(&body),
            unpaged,
            "case {case:?}: expected the caller's own summaries, got {body}"
        );
    }

    // Rows: another account, which this service cannot be asked about at all — refused
    // rather than summarised as nothing. The whole class of `accountIds` values is covered
    // in `tests/all/cross_account_requests.rs`.
    for account_id in ["111122223333", "210987654321"] {
        let (status, body) = call(
            &router,
            "GetRecommendationSummaries",
            json!({"accountIds": [account_id]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {account_id:?}: another account cannot be asked about, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {account_id:?}: expected InvalidParameterValueException, got {body}"
        );
    }

    // Rows: two distinct ids, and the same id twice — both ask for more than one account.
    for accounts in [vec![own.clone(), "111122223333".to_string()], vec![
        own.clone(),
        own.clone(),
    ]] {
        let case = format!("{accounts:?}");
        let (status, body) = call(
            &router,
            "GetRecommendationSummaries",
            json!({"accountIds": accounts}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: only one account ID can be specified per request, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
    }
}

/// `maxResults` never changes WHAT is summarised: the summary set is bounded by the
/// resource-type vocabulary, so every request is answered with the whole set in one answer,
/// in the same order and with no `nextToken` — whatever size it asks for. A token, which this
/// operation therefore never hands out, is refused.
#[tokio::test]
async fn the_summary_set_is_answered_whole_whatever_size_is_asked_for() {
    let router = analysed_emulator().await;

    let unpaged = summaries_of(&get_summaries(&router, json!({})).await);
    assert!(
        unpaged.len() > 2,
        "this test needs several summaries for a size to be observable, got {}",
        unpaged.len()
    );

    // Rows: the smallest size, a size that divides the set unevenly, one that holds it
    // all, the modelled upper bound of `maxResults`, its lower bound `0` (no size asked
    // for) and the member left out entirely.
    let page_sizes: [Option<i64>; 6] = [
        Some(1),
        Some(2),
        Some(unpaged.len() as i64),
        Some(1000),
        Some(0),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("maxResults={page_size:?}");
        let mut request = json!({});
        if let Some(size) = page_size {
            request["maxResults"] = json!(size);
        }
        let body = get_summaries(&router, request).await;
        assert_eq!(
            summaries_of(&body),
            unpaged,
            "case {case:?}: the size asked for must not change what is summarised"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: the whole set is answered, so there is no page to advance to, \
             got {body}"
        );
    }

    // Rows: a token that is not hex, one that is hex but not a token this service issued,
    // and one a client built from a resource type it can see.
    for token in ["not-a-token", "6e6f742d612d746f6b656e", "Ec2Instance"] {
        let (status, body) = call(
            &router,
            "GetRecommendationSummaries",
            json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a token this service did not issue names no page, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {token:?}: expected InvalidParameterValueException, got {body}"
        );
    }
}

/// An `accountIds` value that is not an account id names no account to summarise, so it is
/// a bad request rather than "an account with no findings": every value past the "12-digit
/// Amazon Web Services account ID" shape (`Scope$value`) is refused with the modelled
/// `InvalidParameterValueException` (HTTP 400), whatever direction it misses the shape in,
/// and the refusal is answered before the enrollment gate because it is about the request
/// and not about what the store holds.
#[tokio::test]
async fn account_ids_refuses_a_value_that_is_not_a_12_digit_account_id() {
    let router = analysed_emulator().await;

    // Rows: the class of values that are not a 12-digit account id, each missing the shape
    // in a different direction — nothing at all, too short, too long, letters, one non-digit
    // among twelve characters, digits with separators, a signed number, surrounding
    // whitespace, and twelve non-ASCII digits.
    let malformed = [
        "",
        "123",
        "not-an-account",
        "12345678901",
        "1234567890123",
        "00000000000a",
        "123-456-7890",
        "+12345678901",
        " 123456789012",
        "123456789012 ",
        "１２３４５６７８９０１２",
    ];

    for account_id in malformed {
        let (status, body) = call(
            &router,
            "GetRecommendationSummaries",
            json!({"accountIds": [account_id]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {account_id:?}: a value that is not an account id must be refused, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {account_id:?}: expected InvalidParameterValueException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("accountIds") && message.contains("12-digit"),
            "case {account_id:?}: the refusal must name the member and its constraint, got \
             {message:?}"
        );
        assert!(
            body["recommendationSummaries"].is_null(),
            "case {account_id:?}: a refused request summarises nothing, got {body}"
        );
    }

    // Contrast row: the one well-formed value this service can be asked about still reads
    // the caller's own summaries, so the rule above refuses malformed values rather than
    // every value.
    let own = caller_account();
    assert_eq!(
        summaries_of(&get_summaries(&router, json!({"accountIds": [own.clone()]})).await),
        summaries_of(&get_summaries(&router, json!({})).await),
        "case {own:?}: a well-formed own account id must read the caller's own summaries"
    );

    // The refusal is about the request, so it does not wait on the enrollment gate: an
    // account that never opted in reads nothing (403) but still gets the 400 for a value
    // that is not an account id.
    let not_opted_in = emulator();
    let (status, body) = call(&not_opted_in, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(
        status, 403,
        "case \"not opted in\": expected 403, got {status}: {body}"
    );
    for account_id in malformed {
        let (status, body) = call(
            &not_opted_in,
            "GetRecommendationSummaries",
            json!({"accountIds": [account_id]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case \"not opted in\"/{account_id:?}: the parameter refusal precedes the opt-in \
             gate, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case \"not opted in\"/{account_id:?}: expected InvalidParameterValueException, got \
             {body}"
        );
    }
}

/// Summarising is a READ: two identical calls answer identically, and reading the
/// summaries moves neither the enrollment record nor the stored preferences.
#[tokio::test]
async fn reading_summaries_changes_nothing_a_later_caller_sees() {
    let router = analysed_emulator().await;

    let before = call(&router, "GetEnrollmentStatus", json!({})).await.1;
    let first = get_summaries(&router, json!({})).await;
    let second = get_summaries(&router, json!({})).await;
    assert_eq!(
        first, second,
        "two reads of the same findings must answer identically"
    );

    let after = call(&router, "GetEnrollmentStatus", json!({})).await.1;
    assert_eq!(
        before, after,
        "summarising findings must not move the enrollment record"
    );
    let preferences = call(
        &router,
        "GetRecommendationPreferences",
        json!({"resourceType": "Ec2Instance"}),
    )
    .await
    .1;
    assert_eq!(
        preferences["recommendationPreferencesDetails"],
        json!([]),
        "summarising findings must not write a preference, got {preferences}"
    );
}
