// SPDX-License-Identifier: CC0-1.0

//! `ListInvestigations`' `FilterCriteria`, end to end over the wire (the harness lives
//! in the `detector_lifecycle` suite).
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded call:
//!
//! 1. Every criterion must hold, and each one is judged on the value the READS report
//!    for that key: `Equals` admits an investigation whose value is one of the values
//!    listed, so one that holds no such value cannot satisfy it; `NotEquals` excludes an
//!    investigation whose value is listed, so one holding no such value is not excluded;
//!    and a condition listing no value restricts nothing. The expectation of every row
//!    below is COMPUTED from the unfiltered listing's own summaries, so the filter is
//!    asserted to agree with what the service reports about each investigation.
//! 2. A `criterionKey` the model does not carry is REFUSED — ignoring it would report
//!    investigations the caller asked to exclude.
//! 3. The criteria are part of the listing a page token is a position in: a token minted
//!    for one selection does not resolve against another.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    all_finding_ids, call, investigation_detector, new_router, Router,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::INVESTIGATION_RUNNING;

/// How long a poll may wait for the clock to end an analysis before the test gives up.
/// Generous on purpose: the loop ends on the CONDITION it looks for, never on a duration.
const ENDED_DEADLINE: Duration = Duration::from_secs(30);

/// How long the poll pauses between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Each `InvestigationFilterCriterionKey` member
/// (`smithy_query(shape="InvestigationFilterCriterionKey")`) paired with the
/// `InvestigationSummary` member a listing reports the same value under
/// (`smithy_query(shape="InvestigationSummary")`).
const KEYS: [(&str, &str); 5] = [
    ("STATUS", "status"),
    ("RISK_LEVEL", "riskLevel"),
    ("CONFIDENCE", "confidence"),
    ("INVESTIGATION_TYPE", "investigationType"),
    ("FINDING_ID", "findingId"),
];

/// A value no investigation of the fixture holds, so a condition naming it admits
/// none of them.
const UNHELD_VALUE: &str = "A_VALUE_NO_INVESTIGATION_HOLDS";

/// The sample finding types the fixture's detector holds: one at the top of
/// GuardDuty's severity scale and one at the bottom, so the two investigations below
/// reach DIFFERENT assessments and a criterion can tell them apart.
const FINDING_TYPES: [&str; 3] = [
    "AttackSequence:IAM/CompromisedCredentials",
    "Policy:S3/BucketAnonymousAccessGranted",
    "Recon:EC2/PortProbeUnprotectedPort",
];

/// The finding type the finding-scoped investigation names in its prompt: the lowest
/// severity of the three, so its assessment differs from the account-wide one.
const NAMED_FINDING_TYPE: &str = "Recon:EC2/PortProbeUnprotectedPort";

/// Generate the fixture's findings.
async fn create_samples(router: &Router, detector_id: &str, case: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": FINDING_TYPES})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-sample-findings expected 200, got {status} {body:?}"
    );
}

/// The id of the stored finding whose type is `finding_type`.
async fn finding_of_type(
    router: &Router,
    detector_id: &str,
    finding_type: &str,
    case: &str,
) -> String {
    let ids = all_finding_ids(router, detector_id, 50).await;
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-findings expected 200, got {status} {body:?}"
    );
    body["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|finding| finding["type"].as_str() == Some(finding_type))
        .and_then(|finding| finding["id"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("case {case:?}: no stored {finding_type} finding, got {body:?}"))
}

/// Initiate one investigation with `prompt` and return its id.
async fn initiate(router: &Router, detector_id: &str, prompt: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation"),
        Some(json!({"triggerPrompt": prompt})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-investigation expected 200, got {status} {body:?}"
    );
    body["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an investigationId, got {body:?}"))
        .to_string()
}

/// One `ListInvestigations` call with `body` passed verbatim.
async fn list_raw(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation/list"),
        Some(body),
    )
    .await
}

/// The summaries `ListInvestigations` reports for `body`.
async fn listed(router: &Router, detector_id: &str, body: Value, case: &str) -> Vec<Value> {
    let (status, got) = list_raw(router, detector_id, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-investigations {body} expected 200, got {status} {got:?}"
    );
    got["investigations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an investigations list, got {got:?}"))
        .clone()
}

/// The investigation ids in `summaries`.
fn ids_of(summaries: &[Value]) -> BTreeSet<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["investigationId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Read one investigation until its pass has ended, or until [`ENDED_DEADLINE`] passes.
async fn settle(router: &Router, detector_id: &str, id: &str, case: &str) {
    let deadline = std::time::Instant::now() + ENDED_DEADLINE;
    loop {
        let (status, body) = call(
            router,
            "GET",
            &format!("/detector/{detector_id}/investigation/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: get-investigation expected 200, got {status} {body:?}"
        );
        if body["investigation"]["status"] != json!(INVESTIGATION_RUNNING) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the analysis never ended within {ENDED_DEADLINE:?}, last read {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The fixture: a detector holding three findings and two ENDED investigations — one
/// scoped to a single finding, one account-wide — so every criterion key has two
/// investigations to tell apart.
async fn fixture(case: &str) -> (Router, String, Vec<Value>) {
    let router = new_router();
    let detector_id = investigation_detector(&router).await;
    create_samples(&router, &detector_id, case).await;
    let named = finding_of_type(&router, &detector_id, NAMED_FINDING_TYPE, case).await;

    let finding_scoped = initiate(
        &router,
        &detector_id,
        &format!("Investigate the finding {named}"),
        case,
    )
    .await;
    let account_wide = initiate(&router, &detector_id, "Investigate this account", case).await;
    for id in [&finding_scoped, &account_wide] {
        settle(&router, &detector_id, id, case).await;
    }

    let summaries = listed(&router, &detector_id, json!({}), case).await;
    assert_eq!(
        summaries.len(),
        2,
        "case {case:?}: expected the two investigations of the fixture, got {summaries:?}"
    );
    (router, detector_id, summaries)
}

/// The ids whose `member` holds `value`.
fn ids_where(summaries: &[Value], member: &str, value: &str) -> BTreeSet<String> {
    summaries
        .iter()
        .filter(|summary| summary[member].as_str() == Some(value))
        .map(|summary| {
            summary["investigationId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The ids whose `member` does NOT hold `value` — including an investigation holding no
/// value for it at all.
fn ids_where_not(summaries: &[Value], member: &str, value: &str) -> BTreeSet<String> {
    summaries
        .iter()
        .filter(|summary| summary[member].as_str() != Some(value))
        .map(|summary| {
            summary["investigationId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The values the fixture's summaries hold for `member`, plus one no investigation
/// holds.
fn values_of(summaries: &[Value], member: &str) -> Vec<String> {
    let mut values: BTreeSet<String> = summaries
        .iter()
        .filter_map(|summary| summary[member].as_str().map(str::to_string))
        .collect();
    values.insert(UNHELD_VALUE.to_string());
    values.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Rule 1.
// ---------------------------------------------------------------------------

/// Each criterion key, over each value the fixture's investigations hold for it and one
/// they do not, under `Equals`, under `NotEquals` and under a condition that lists
/// nothing.
#[tokio::test]
async fn a_criterion_selects_the_investigations_whose_value_it_names() {
    let case = "the fixture's two investigations";
    let (router, detector_id, summaries) = fixture(case).await;
    let all = ids_of(&summaries);

    for (key, member) in KEYS {
        // A condition that lists no value restricts nothing.
        let row = format!("{case}, {key} with an empty condition");
        let got = listed(
            &router,
            &detector_id,
            json!({"filterCriteria": {"criterion": {key: {}}}}),
            &row,
        )
        .await;
        assert_eq!(
            ids_of(&got),
            all,
            "case {row:?}: a condition listing no value restricts nothing"
        );

        for value in values_of(&summaries, member) {
            let row = format!("{case}, {key} equals {value:?}");
            let got = listed(
                &router,
                &detector_id,
                json!({"filterCriteria": {"criterion": {key: {"equals": [value]}}}}),
                &row,
            )
            .await;
            assert_eq!(
                ids_of(&got),
                ids_where(&summaries, member, &value),
                "case {row:?}: expected the investigations the listing reports {member}={value:?} \
                 for, got {got:?}"
            );

            let row = format!("{case}, {key} notEquals {value:?}");
            let got = listed(
                &router,
                &detector_id,
                json!({"filterCriteria": {"criterion": {key: {"notEquals": [value]}}}}),
                &row,
            )
            .await;
            assert_eq!(
                ids_of(&got),
                ids_where_not(&summaries, member, &value),
                "case {row:?}: expected every investigation the listing does not report \
                 {member}={value:?} for, got {got:?}"
            );
        }
    }
}

/// Every criterion must hold: two of them select the investigations that satisfy both.
#[tokio::test]
async fn two_criteria_select_the_investigations_that_satisfy_both() {
    let case = "two criteria together";
    let (router, detector_id, summaries) = fixture(case).await;

    for (key, member) in KEYS {
        for value in values_of(&summaries, member) {
            let row = format!("{case}: STATUS equals COMPLETED and {key} equals {value:?}");
            let got = listed(
                &router,
                &detector_id,
                json!({"filterCriteria": {"criterion": {
                    "STATUS": {"equals": ["COMPLETED"]},
                    key: {"equals": [value]},
                }}}),
                &row,
            )
            .await;
            let mut expected = ids_where(&summaries, member, &value);
            expected.retain(|id| ids_where(&summaries, "status", "COMPLETED").contains(id));
            assert_eq!(
                ids_of(&got),
                expected,
                "case {row:?}: expected the investigations both criteria admit, got {got:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2.
// ---------------------------------------------------------------------------

/// A criterion key the model does not carry is refused, naming the key and the keys the
/// enum does carry.
#[tokio::test]
async fn a_criterion_key_the_model_does_not_carry_is_refused() {
    let case = "keys outside InvestigationFilterCriterionKey";
    let (router, detector_id, _) = fixture(case).await;

    // The rows differ along the shape of the unknown key: an attribute of another
    // listing, the lower-camel spelling of a real member, a sort field that is not a
    // criterion key, and the empty string.
    for key in ["ACCOUNT_ID", "status", "START_TIME", ""] {
        let row = format!("{case}: {key:?}");
        let (status, refused) = list_raw(
            &router,
            &detector_id,
            json!({"filterCriteria": {"criterion": {key: {"equals": ["COMPLETED"]}}}}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: an unknown criterion key is refused, got {status} {refused:?}"
        );
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(key) && message.contains("STATUS") && message.contains("FINDING_ID"),
            "case {row:?}: expected a refusal naming the key and the supported keys, got \
             {refused:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3.
// ---------------------------------------------------------------------------

/// A page token names a position in ONE selection: the listing that minted it resumes
/// it, and a listing narrowed differently refuses it.
#[tokio::test]
async fn a_page_token_belongs_to_the_selection_that_minted_it() {
    let case = "a token minted over the unfiltered listing";
    let (router, detector_id, summaries) = fixture(case).await;

    let (status, first) = list_raw(&router, &detector_id, json!({"maxResults": 1})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first page, got {status} {first:?}"
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken, got {first:?}"))
        .to_string();

    // Its own listing resumes it, and the two pages together are the whole list.
    let second = listed(
        &router,
        &detector_id,
        json!({"maxResults": 1, "nextToken": token}),
        case,
    )
    .await;
    let mut walked = ids_of(&first["investigations"].as_array().cloned().unwrap_or_default());
    walked.extend(ids_of(&second));
    assert_eq!(
        walked,
        ids_of(&summaries),
        "case {case:?}: the two pages are the whole list, got {first:?} then {second:?}"
    );

    // A listing narrowed by criteria is a different list, so the token is not one it
    // issued.
    let (status, refused) = list_raw(
        &router,
        &detector_id,
        json!({
            "maxResults": 1,
            "nextToken": token,
            "filterCriteria": {"criterion": {"STATUS": {"equals": ["COMPLETED"]}}},
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a token of another selection does not resolve, got {status} {refused:?}"
    );
}
