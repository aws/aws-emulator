// SPDX-License-Identifier: CC0-1.0

//! WHAT ONE ANSWER of `GetRecommendationSummaries` holds, end to end over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests prove
//! is what a client sees — never what the state struct holds.
//!
//! Two rules are under test.
//!
//! * The summary set is BOUNDED by the resource-type vocabulary — one row per
//!   `RecommendationSourceType` the service reports on, whatever the account holds of it — so
//!   the whole set is answered at once: a request is answered with every row and no
//!   `nextToken`, whatever `maxResults` it asks for. The rule generalises over the size asked
//!   for, so the rows below are the forms `maxResults` can take: absent, the modelled range's
//!   bounds, a size smaller than the set, exactly its size, and one larger. The `nextToken`
//!   cursor stays the one the model declares: a token naming a group resumes the answer at
//!   it, and a value that is not one of this operation's tokens is refused.
//! * A summary row counts the account's resources at the finding vocabulary of THAT resource
//!   type's own recommendations, which is not narrowed to the values the model's `Finding`
//!   enum lists: a commercial software license is counted at the three `LicenseFinding`
//!   classifications the operation documents for one ("Commercial software licenses in an
//!   account that are InsufficientMetrics, NotOptimized or Optimized"), and each count is the
//!   number of licenses `GetLicenseRecommendations` reports at that classification — so a
//!   count the summary states and the licenses behind it can never disagree. The rule
//!   generalises over the classification, so the rows are every `LicenseFinding` value,
//!   crossed with the states the account's analysis can be in.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    encode_summary_page_token, TRACKED_SOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, emulator, emulator_with_engine, error_code, make_analysis_due, opt_in,
    summary_of, summary_resource_types, summary_set,
};

/// The `LicenseFinding` classifications a license summary counts at, in the order the summary
/// reports them: the model's `LicenseFinding` value set
/// (`crate::ops::types::LICENSE_FINDING_VALUES` = InsufficientMetrics | NotOptimized |
/// Optimized), which is the vocabulary `GetLicenseRecommendations` reports a license's own
/// `finding` from.
const LICENSE_FINDINGS: &[&str] = &["Optimized", "NotOptimized", "InsufficientMetrics"];

/// Whatever page size a request asks for, the answer is the WHOLE summary set — one row per
/// resource type the service reports on, in one answer, with no `nextToken` to walk.
#[tokio::test]
async fn the_whole_summary_set_is_answered_whatever_max_results_asks_for() {
    let router = emulator();
    opt_in(&router).await;

    let unsized_answer = summary_set(&router, "no maxResults", json!({})).await;
    assert_eq!(
        summary_resource_types("no maxResults", &unsized_answer),
        TRACKED_SOURCE_TYPES.to_vec(),
        "expected one summary per resource type the service reports on, got {unsized_answer}"
    );

    // Rows: the forms `maxResults` can take — the modelled range's lower bound, the smallest
    // size, a size that divides the set unevenly, one short of the set, exactly its size, one
    // past it, and the modelled upper bound.
    let set = TRACKED_SOURCE_TYPES.len() as i64;
    for max_results in [0, 1, 3, set - 1, set, set + 1, 1000] {
        let case = format!("maxResults={max_results}");
        let answer = summary_set(&router, &case, json!({"maxResults": max_results})).await;
        assert!(
            answer["nextToken"].is_null(),
            "case {case:?}: the summary set is answered whole, so there is no page to \
             advance to, got {answer}"
        );
        assert_eq!(
            summary_resource_types(&case, &answer),
            TRACKED_SOURCE_TYPES.to_vec(),
            "case {case:?}: expected every resource type the service reports on, got {answer}"
        );
        assert_eq!(
            answer, unsized_answer,
            "case {case:?}: the size asked for must not change what is answered"
        );
    }
}

/// The `nextToken` cursor names the group an answer RESUMES at, so a token of this
/// operation's own cursor (`crate::state::encode_summary_page_token`, the one place its
/// format lives) is answered with the rest of the set and a value that is not one of its
/// tokens names no page at all — the modelled `InvalidParameterValueException` (HTTP 400).
#[tokio::test]
async fn a_next_token_resumes_at_the_group_it_names_and_a_value_that_is_not_one_is_refused() {
    let router = emulator();
    opt_in(&router).await;
    let whole = summary_set(&router, "the whole set", json!({})).await;

    // Rows: the cursor of each group of the vocabulary — the answer resumes at that group and
    // reports the rest of the set from there, which for the first group is the whole of it.
    for (at, resource_type) in TRACKED_SOURCE_TYPES.iter().enumerate() {
        let case = format!("resuming at {resource_type}");
        let token = encode_summary_page_token(resource_type);
        let answer = summary_set(&router, &case, json!({"nextToken": token})).await;
        assert_eq!(
            summary_resource_types(&case, &answer),
            TRACKED_SOURCE_TYPES[at..].to_vec(),
            "case {case:?}: expected the groups from {resource_type} on, got {answer}"
        );
        assert!(
            answer["nextToken"].is_null(),
            "case {case:?}: the rest of the set is answered whole, so there is no page to \
             advance to, got {answer}"
        );
        assert_eq!(
            summary_of(&case, &answer, resource_type),
            summary_of(&case, &whole, resource_type),
            "case {case:?}: resuming must not change what a summary says"
        );
    }

    // Rows: a value that is not hex, one that is hex but carries no group, one that is hex and
    // decodes to JSON naming no group, one a client built from a resource type it can see in
    // the answer, one naming a resource type outside the vocabulary, and the empty string.
    for token in [
        "not-a-token".to_string(),
        "6e6f742d612d746f6b656e".to_string(),
        "7b7d".to_string(),
        "Ec2Instance".to_string(),
        encode_summary_page_token("NotAResourceType"),
        String::new(),
    ] {
        let (status, body) = call(
            &router,
            "GetRecommendationSummaries",
            json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a value that is not one of this operation's tokens names no \
             page, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {token:?}: expected InvalidParameterValueException, got {body}"
        );
        assert!(
            body["recommendationSummaries"].is_null(),
            "case {token:?}: a refused request summarises nothing, got {body}"
        );
    }
}

/// How many licenses `GetLicenseRecommendations` reports at each `LicenseFinding`.
async fn licenses_by_finding(router: &MultiServiceRouter, case: &str) -> BTreeMap<String, f64> {
    let (status, body) = call(router, "GetLicenseRecommendations", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the licenses must be served, got {status}: {body}"
    );
    let mut counted: BTreeMap<String, f64> = LICENSE_FINDINGS
        .iter()
        .map(|finding| ((*finding).to_string(), 0.0))
        .collect();
    for reported in body["licenseRecommendations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let finding = reported["finding"].as_str().unwrap_or_default().to_string();
        assert!(
            counted.contains_key(&finding),
            "case {case:?}: {finding:?} is not a LicenseFinding value, got {reported}"
        );
        *counted.entry(finding).or_insert(0.0) += 1.0;
    }
    counted
}

/// The license summary counts the account's licenses at every `LicenseFinding` — including
/// `InsufficientMetrics`, the classification of a license whose metrics the analysis could not
/// read — and each count is exactly what the per-license reader reports at it.
#[tokio::test]
async fn the_license_summary_counts_the_licenses_at_each_license_finding() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;

    // Rows: the states the account's analysis can be in — the analysis the enrollment started
    // is not due yet, so every count is zero; and it is due, so the counts are the licenses
    // the per-license reader reports. The classification set is the same in both.
    for case in ["nothing analysed yet", "the analysis is due"] {
        if case == "the analysis is due" {
            make_analysis_due(&engine);
        }
        let answer = summary_set(&router, case, json!({})).await;
        let row = summary_of(case, &answer, "License");
        let counts = row["summaries"].as_array().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: a license is counted at each finding, got {row}")
        });
        let reported: Vec<&str> = counts
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, *LICENSE_FINDINGS,
            "case {case:?}: expected a count at each LicenseFinding {LICENSE_FINDINGS:?}, got \
             {reported:?}"
        );

        let expected = licenses_by_finding(&router, case).await;
        let total: f64 = expected.values().sum();
        assert_eq!(
            total > 0.0,
            case == "the analysis is due",
            "case {case:?}: this fixture must analyse licenses exactly once the analysis is \
             due for these rows to prove anything, got {expected:?}"
        );
        for entry in &counts {
            let finding = entry["name"].as_str().unwrap_or_default().to_string();
            let counted = entry["value"].as_f64().unwrap_or_else(|| {
                panic!("case {case:?}: a finding count is a number, got {entry}")
            });
            let want = expected.get(&finding).copied().unwrap_or_default();
            assert_eq!(
                counted, want,
                "case {case:?}/{finding:?}: expected the {want} licenses the per-license \
                 reader reports at that finding, got {counted}"
            );
            // A reason code, a saving and a performance risk describe a RECOMMENDED
            // CONFIGURATION, and a license's analysis reaches a classification only
            // (`LicenseRecommendation` declares no performance risk and no saving of its
            // own), so a license summary is its counts alone.
            assert!(
                entry["reasonCodeSummaries"].is_null(),
                "case {case:?}/{finding:?}: a group the service only classifies states no \
                 reason for a finding, got {entry}"
            );
        }
        for block in [
            "savingsOpportunity",
            "idleSavingsOpportunity",
            "aggregatedSavingsOpportunity",
            "currentPerformanceRiskRatings",
            "idleSummaries",
        ] {
            assert!(
                row[block].is_null(),
                "case {case:?}: a group the service only classifies reports no {block}, got \
                 {row}"
            );
        }
    }

    // At least one of the account's licenses is classified `InsufficientMetrics`, so the
    // count at that classification is backed by stored licenses rather than fixed at zero.
    let insufficient = licenses_by_finding(&router, "InsufficientMetrics")
        .await
        .get("InsufficientMetrics")
        .copied()
        .unwrap_or_default();
    assert!(
        insufficient > 0.0,
        "this fixture must hold a license the analysis could not read for the count at \
         InsufficientMetrics to be observable, got {insufficient}"
    );
}
