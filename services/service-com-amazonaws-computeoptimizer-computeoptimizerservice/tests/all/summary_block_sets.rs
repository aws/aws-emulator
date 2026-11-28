// SPDX-License-Identifier: CC0-1.0

//! WHAT ONE ANSWER of `GetRecommendationSummaries` holds, end to end over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests prove
//! is what a client sees — never what the state struct holds.
//!
//! Two rules are under test.
//!
//! * ONE ACCOUNT'S ROLL-UP IS ANSWERED WHOLE. The summary set is the resource-type vocabulary
//!   — one row per `RecommendationSourceType` the service reports on, whatever the account holds
//!   of it — and `maxResults` is "The maximum number of recommendation summaries to return with
//!   a single request", which bounds how many summarised ACCOUNTS one answer carries, not how
//!   many groups the one account a request summarises is reported at ("Only one account ID can
//!   be specified per request"). So every size asked for is answered with the same rows, in the
//!   same order, and with no `nextToken`: it is "null when there are no more pages of
//!   recommendation summaries to return", and after one account's groups there are none. The
//!   rule generalises over the size asked for, so the rows below are the forms `maxResults`
//!   can take: absent, the modelled range's bounds, a size smaller than the set, one that
//!   divides it unevenly, exactly its size, and one larger. The `nextToken` cursor is the one
//!   the model declares: a token naming a group resumes the answer at it, and a value that is
//!   not one of this operation's tokens is refused.
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

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    encode_summary_page_token, MAX_PAGE_SIZE, TRACKED_SOURCE_TYPES,
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

/// The resource types a list of summaries covers, in the order reported.
fn summary_types(case: &str, summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["recommendationResourceType"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a summary names its resource type, got {summary}")
                })
                .to_string()
        })
        .collect()
}

/// One reading of the summaries, `max_results` at a time, walking the `nextToken` to the last
/// page: every summary reported, in order, and the size of each page that reported them.
///
/// `max_results` is `None` for a request that names no size at all, so the row that asks for
/// none drives exactly the same walk as the rows that do.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
) -> (Vec<Value>, Vec<usize>) {
    let mut summaries: Vec<Value> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["maxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let body = summary_set(router, case, request).await;
        let page = body["recommendationSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: recommendationSummaries must be a list, got {body}")
            });
        page_sizes.push(page.len());
        summaries.extend(page);
        next_token = body["nextToken"].as_str().map(ToString::to_string);
        if next_token.is_none() {
            return (summaries, page_sizes);
        }
        assert!(
            summaries.len() <= TRACKED_SOURCE_TYPES.len(),
            "case {case:?}: a token is handed out only while summaries remain, so a walk \
             cannot report more than the {} the service reports on, got {} over pages {:?}",
            TRACKED_SOURCE_TYPES.len(),
            summaries.len(),
            page_sizes
        );
    }
}

/// `maxResults` does not shorten one account's roll-up: whatever size a request asks for, the
/// answer carries every group of the account the request summarises, in the reported order, and
/// hands out no token — the same rows a request that asks for no size reads.
#[tokio::test]
async fn max_results_does_not_shorten_one_accounts_roll_up() {
    let router = emulator();
    opt_in(&router).await;

    let (whole, unsized_pages) = walk(&router, "no maxResults", None).await;
    assert_eq!(
        summary_types("no maxResults", &whole),
        TRACKED_SOURCE_TYPES.to_vec(),
        "expected one summary per resource type the service reports on, got {whole:?}"
    );
    // One answer, carrying the whole set: a request that asks for no size reads it in one
    // answer, as every other size does.
    assert_eq!(
        unsized_pages,
        vec![whole.len()],
        "case \"no maxResults\": expected one answer carrying the whole set"
    );

    // Rows: the forms `maxResults` can take. `0` is the modelled range's lower bound, which the
    // service documents no meaning for, and `1000` is its upper bound; none of them changes how
    // many groups one account's roll-up carries.
    let set = TRACKED_SOURCE_TYPES.len() as i64;
    let rows: [Option<i64>; 8] = [
        None,
        Some(0),
        Some(1),
        Some(3),
        Some(set - 1),
        Some(set),
        Some(set + 1),
        Some(MAX_PAGE_SIZE as i64),
    ];
    for max_results in rows {
        let case = format!("maxResults={max_results:?}");
        let (walked, pages) = walk(&router, &case, max_results).await;
        assert_eq!(
            pages,
            vec![whole.len()],
            "case {case:?}: expected the set of {} in one answer, got pages {pages:?}",
            whole.len()
        );
        assert_eq!(
            walked, whole,
            "case {case:?}: the size asked for changes neither which summaries one answer \
             carries nor in what order"
        );
    }
}

/// A size smaller than the vocabulary is OBSERVABLE only in that it changes nothing: a request
/// for one summary is answered with the account's whole roll-up and no token, and asking again
/// reads the same answer — so reading it changes nothing a later caller sees.
#[tokio::test]
async fn a_size_smaller_than_the_vocabulary_changes_nothing() {
    let router = emulator();
    opt_in(&router).await;

    let first = summary_set(&router, "a page of one", json!({"maxResults": 1})).await;
    assert_eq!(
        summary_resource_types("a page of one", &first),
        TRACKED_SOURCE_TYPES.to_vec(),
        "case \"a page of one\": expected the whole roll-up of the one account summarised, got \
         {first}"
    );
    assert!(
        first["nextToken"].is_null(),
        "case \"a page of one\": one account's roll-up is answered whole, so there is no page \
         to advance to, got {first}"
    );

    let again = summary_set(&router, "the first page again", json!({"maxResults": 1})).await;
    assert_eq!(
        again, first,
        "case \"the first page again\": reading a page must not change what a later caller sees"
    );
}

/// The last page of a walk hands out no token — "This value is null when there are no more
/// pages of recommendation summaries to return" — and every page before it does, whichever
/// size the walk uses to reach it.
#[tokio::test]
async fn a_token_is_handed_out_exactly_while_summaries_remain() {
    let router = emulator();
    opt_in(&router).await;

    // Rows: a size that divides the set unevenly, one that divides it evenly, one short of it,
    // exactly it, and one past it.
    let set = TRACKED_SOURCE_TYPES.len() as i64;
    for max_results in [3, 4, set - 1, set, set + 1] {
        let case = format!("maxResults={max_results}");
        let mut next_token: Option<String> = None;
        let mut reported = 0usize;
        loop {
            let mut request = json!({"maxResults": max_results});
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let body = summary_set(&router, &case, request).await;
            reported += body["recommendationSummaries"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            next_token = body["nextToken"].as_str().map(ToString::to_string);
            let remaining = TRACKED_SOURCE_TYPES.len().saturating_sub(reported);
            assert_eq!(
                next_token.is_some(),
                remaining > 0,
                "case {case:?}: a token is handed out exactly while summaries remain \
                 ({remaining} left after {reported}), got {body}"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            reported,
            TRACKED_SOURCE_TYPES.len(),
            "case {case:?}: a walk reports every summary exactly once, got {reported}"
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
