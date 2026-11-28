// SPDX-License-Identifier: CC0-1.0

//! The ORDER `GetRecommendationSummaries` rolls an account's findings up in, and the
//! vocabulary that order walks.
//!
//! The rule under test: the groups a summary answer reports are the whole
//! `RecommendationSourceType` vocabulary `RecommendationSummary$recommendationResourceType`
//! names, in the order the service reports them, and ONE ACCOUNT'S ROLL-UP IS ANSWERED WHOLE —
//! `maxResults` is "the maximum number of recommendation summaries to return with a single
//! request", which bounds how many summarised ACCOUNTS one answer carries and never shortens the
//! group list of the one account a request summarises ("Only one account ID can be specified per
//! request"), so a client reads the same groups in the same order whatever size it asks for.
//!
//! That the entries exist at all for an account whose analysis has produced nothing, and what
//! each of them counts, is asserted in `tests/all/summary_resource_type_surface.rs` and
//! `tests/all/recommendation_summaries.rs`; this suite pins the LIST the handler walks
//! against the model's own enum, so a value the model adds or renames cannot be missed, and
//! pins the reported order against it.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::TRACKED_SOURCE_TYPES;

use crate::harness::{
    emulator_with_engine, make_analysis_due, opt_in, summary_resource_types, summary_set,
};

/// The sizes a request can ask for, as `maxResults` admits them (`@range min 0 max 1000`), with
/// `None` for the member left out.
fn sizes(vocabulary: usize) -> Vec<Option<i64>> {
    let set = vocabulary as i64;
    vec![
        None,
        Some(0),
        Some(1),
        Some(3),
        Some(set - 1),
        Some(set),
        Some(set + 1),
        Some(1000),
    ]
}

/// RULE: whatever size a request asks for, the answer carries the whole roll-up of the one
/// account it summarises — every modelled `RecommendationSourceType` exactly once, in one order —
/// and hands out no `nextToken`.
///
/// `maxResults` is "the maximum number of recommendation summaries to return with a single
/// request" while `accountIds` names the one account a request summarises ("only one account ID
/// can be specified per request"), so the size bounds the summarised ACCOUNTS an answer carries
/// rather than the resource-type groups the one account is reported at — and an absent group is a
/// different statement from a group whose counts are zero: it says the service does not report on
/// that resource type at all. The groups are checked against the MODEL's own value set rather
/// than against the order the handler walks, so a group dropped from the answer fails this test
/// whatever list the crate keeps.
#[tokio::test]
async fn one_accounts_roll_up_is_answered_whole_whatever_size_is_asked_for() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    // Read with the analysis the enrollment started DUE, so the groups carry the counts a real
    // read reports rather than the counts of an account whose analysis has not run.
    make_analysis_due(&engine);

    let modelled: BTreeSet<&str> = RECOMMENDATION_SOURCE_TYPE_VALUES.iter().copied().collect();
    let reported = summary_set(&router, "no maxResults", json!({})).await;
    let order = summary_resource_types("no maxResults", &reported);
    assert_eq!(
        order.iter().map(String::as_str).collect::<BTreeSet<&str>>(),
        modelled,
        "expected one summary per modelled RecommendationSourceType, got {order:?}"
    );
    assert_eq!(
        order.len(),
        modelled.len(),
        "expected no resource type reported twice, got {order:?}"
    );

    for size in sizes(modelled.len()) {
        let case = format!("maxResults={size:?}");
        let mut request = json!({});
        if let Some(size) = size {
            request["maxResults"] = json!(size);
        }
        let answer = summary_set(&router, &case, request).await;
        assert_eq!(
            summary_resource_types(&case, &answer),
            order,
            "case {case:?}: expected the whole roll-up of the one account summarised, in the \
             order every other size reports it in"
        );
        assert!(
            answer["nextToken"].is_null(),
            "case {case:?}: one account's groups are the whole of this walk, so no token may be \
             handed out, got {answer}"
        );
    }
}

/// The resource types the service rolls findings up for are EXACTLY the model's
/// `RecommendationSourceType` vocabulary — no value missing and none invented, each once — so
/// the list the handler walks cannot drift from the model's enum as that enum grows.
#[test]
fn the_tracked_resource_types_are_the_modelled_vocabulary() {
    let tracked: BTreeSet<&str> = TRACKED_SOURCE_TYPES.iter().copied().collect();
    let modelled: BTreeSet<&str> = RECOMMENDATION_SOURCE_TYPE_VALUES.iter().copied().collect();
    assert_eq!(
        tracked, modelled,
        "the tracked resource types must be the modelled RecommendationSourceType values"
    );
    assert_eq!(
        tracked.len(),
        TRACKED_SOURCE_TYPES.len(),
        "no resource type may be tracked twice, got {TRACKED_SOURCE_TYPES:?}"
    );
}

/// One answer reports the tracked vocabulary in the order the service reports it, and the size a
/// request asks for changes neither the order nor the set: one account's roll-up is answered
/// whole, so every size reports every group once and no token is handed out.
#[tokio::test]
async fn the_summaries_are_reported_in_the_tracked_order() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    // Read with the analysis the enrollment started DUE, so the counts differ between groups
    // and this order is not the order of an all-zero response alone. The due instant is
    // derived from the stored enrollment record, so it is moved rather than waited for.
    make_analysis_due(&engine);

    let whole = summary_set(&router, "the whole set", json!({})).await;
    assert_eq!(
        summary_resource_types("the whole set", &whole),
        TRACKED_SOURCE_TYPES.to_vec(),
        "expected one summary per tracked resource type, in the tracked order, got {whole}"
    );

    // Rows: sizes smaller than the vocabulary, one that divides it unevenly, and one holding
    // the lot — none of which moves a group out of the reported order or out of the answer: the
    // size bounds the accounts summarised, and this request summarises one account.
    for max_results in [1, 3, 5, TRACKED_SOURCE_TYPES.len()] {
        let case = format!("maxResults={max_results}");
        let answer = summary_set(&router, &case, json!({"maxResults": max_results})).await;
        assert_eq!(
            summary_resource_types(&case, &answer),
            TRACKED_SOURCE_TYPES.to_vec(),
            "case {case:?}: the answer must report exactly the tracked vocabulary, in the \
             tracked order"
        );
        assert!(
            answer["nextToken"].is_null(),
            "case {case:?}: one account's roll-up is answered whole, so there is no page to \
             advance to, got {answer}"
        );
    }
}
