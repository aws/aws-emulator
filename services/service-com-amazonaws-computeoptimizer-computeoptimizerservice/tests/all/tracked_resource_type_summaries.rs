// SPDX-License-Identifier: CC0-1.0

//! The ORDER `GetRecommendationSummaries` rolls an account's findings up in, and the
//! vocabulary that order walks.
//!
//! The rule under test: the groups a summary page reports are the whole
//! `RecommendationSourceType` vocabulary `RecommendationSummary$recommendationResourceType`
//! names, in the order the model declares it, and that vocabulary BOUNDS the answer — so a
//! client that indexes the response, or derives the tracked resource-type set from it, reads
//! the same groups in the same places whatever size it asks for.
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

/// One answer reports the tracked vocabulary in the order the model declares it, and the size
/// a request asks for changes neither the order nor the set: the summary set is bounded by
/// that vocabulary, so it is answered whole with no `nextToken` to walk.
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
    // the lot — none of which moves a group out of the reported order.
    for max_results in [1, 3, 5, TRACKED_SOURCE_TYPES.len()] {
        let case = format!("maxResults={max_results}");
        let answer = summary_set(&router, &case, json!({"maxResults": max_results})).await;
        assert!(
            answer["nextToken"].is_null(),
            "case {case:?}: the whole vocabulary is answered, so there is no page to advance \
             to, got {answer}"
        );
        assert_eq!(
            summary_resource_types(&case, &answer),
            TRACKED_SOURCE_TYPES.to_vec(),
            "case {case:?}: the answer must be exactly the tracked vocabulary, in the tracked \
             order"
        );
        assert_eq!(
            answer, whole,
            "case {case:?}: the size asked for must not change what each summary says"
        );
    }
}
