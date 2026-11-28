// SPDX-License-Identifier: CC0-1.0

//! The configuration an output REPORTS when the request left part of it unsaid.
//!
//! The rule under test, over a table rather than one case: an optional output-config
//! member whose model documentation names the value the service applies when the
//! request omits it is stored with that value, so every later read reports the
//! configuration the service actually applied; a member the request DID carry is
//! reported exactly as it was given, and a member the model documents no resolved
//! value for stays absent.
//!
//! Because the stored config is the configuration in force, a read-modify-write round
//! trip is a FIXED POINT: `GetFeed` -> `UpdateFeed` (a full-replacement PUT) of the
//! config just read back reports that same config, and so does an `AssociateFeed`
//! append of it.
//!
//! The rule for a member the model documents no resolved value for — a cropping output's
//! template groups, a clipping output's callback metadata — is that it stays absent.

use crate::harness::{call, output_config, past_the_transition_span, router};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The documented resolved values this service's output configs carry, as the model
/// states them (`smithy_query(query="shape",shape="ContextualMetadataConfig")` for the
/// two modes — each "ENABLED (default)" — and
/// `smithy_query(query="shape",shape="SubtitlingConfig")` for the filter, "DISABLED (no
/// filtering, default)").
const DEFAULT_SUMMARY_GENERATION: &str = "ENABLED";
const DEFAULT_EXTENDED_ANALYSIS: &str = "ENABLED";
const DEFAULT_PROFANITY_FILTER: &str = "DISABLED";

/// Every value `ProfanityFilterMode` admits
/// (`smithy_query(query="shape",shape="ProfanityFilterMode")`), so the "supplied" rows
/// below cover the whole enum and not only the default.
const PROFANITY_FILTER_VALUES: [&str; 3] = ["DISABLED", "CENSOR", "DROP"];

/// Every value the two contextual-metadata modes admit
/// (`smithy_query(query="shape",shape="SummaryGenerationMode,ExtendedAnalysisMode")`).
const ANALYSIS_MODE_VALUES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The aspect ratio a subtitling output created without one reports: 16:9, the
/// service-chosen geometry (`SubtitlingConfig$aspectRatio` is optional and the model
/// declares no default).
fn default_aspect_ratio() -> Value {
    json!({"width": 16, "height": 9})
}

/// One row: the `outputConfig` a request carries and the config every read of the
/// stored output is expected to report for it.
struct Row {
    label: String,
    submitted: Value,
    expected: Value,
}

/// The table: every member of the `OutputConfig` union
/// (`smithy_query(query="shape",shape="OutputConfig")`: cropping, clipping, subtitling,
/// contextualMetadata), and for each config carrying a service-resolved member every
/// combination of stating it and leaving it out — including values that are NOT the
/// documented default, which the service must never rewrite.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        // The configs the model documents no resolved member for: stored verbatim.
        Row {
            label: "cropping, whose only member the request states".to_string(),
            submitted: output_config("cropping"),
            expected: output_config("cropping"),
        },
        Row {
            label: "cropping with two template groups".to_string(),
            submitted: json!({"cropping": {"templateGroups": [
                {"name": "group1", "templateUris": ["s3://templates/wide.psd"]},
                {"name": "group2", "templateUris": [
                    "s3://templates/tall.psd", "s3://templates/square.psd"
                ]}
            ]}}),
            expected: json!({"cropping": {"templateGroups": [
                {"name": "group1", "templateUris": ["s3://templates/wide.psd"]},
                {"name": "group2", "templateUris": [
                    "s3://templates/tall.psd", "s3://templates/square.psd"
                ]}
            ]}}),
        },
        Row {
            label: "clipping with both of its optional members".to_string(),
            submitted: output_config("clipping"),
            expected: output_config("clipping"),
        },
        Row {
            label: "clipping with neither of its optional members".to_string(),
            submitted: json!({"clipping": {}}),
            expected: json!({"clipping": {}}),
        },
        // contextualMetadata: two independent modes, each documented ENABLED.
        Row {
            label: "contextualMetadata stating neither mode".to_string(),
            submitted: json!({"contextualMetadata": {}}),
            expected: json!({"contextualMetadata": {
                "summaryGeneration": DEFAULT_SUMMARY_GENERATION,
                "extendedAnalysis": DEFAULT_EXTENDED_ANALYSIS
            }}),
        },
        // Subtitling: the shortest config the shape admits, and one that states the
        // filter but not the ratio.
        Row {
            label: "subtitling with only its required language".to_string(),
            submitted: json!({"subtitling": {"language": "por"}}),
            expected: json!({"subtitling": {
                "language": "por",
                "aspectRatio": default_aspect_ratio(),
                "profanityFilter": DEFAULT_PROFANITY_FILTER
            }}),
        },
        Row {
            label: "subtitling with a 21:9 ratio and no filter".to_string(),
            submitted: json!({"subtitling": {
                "language": "eng-au",
                "aspectRatio": {"width": 21, "height": 9}
            }}),
            expected: json!({"subtitling": {
                "language": "eng-au",
                "aspectRatio": {"width": 21, "height": 9},
                "profanityFilter": DEFAULT_PROFANITY_FILTER
            }}),
        },
    ];

    // Each mode stated on its own: the other one still resolves, because the model
    // calls the two settings independent.
    for mode in ANALYSIS_MODE_VALUES {
        rows.push(Row {
            label: format!("contextualMetadata stating summaryGeneration={mode} alone"),
            submitted: json!({"contextualMetadata": {"summaryGeneration": mode}}),
            expected: json!({"contextualMetadata": {
                "summaryGeneration": mode,
                "extendedAnalysis": DEFAULT_EXTENDED_ANALYSIS
            }}),
        });
        rows.push(Row {
            label: format!("contextualMetadata stating extendedAnalysis={mode} alone"),
            submitted: json!({"contextualMetadata": {"extendedAnalysis": mode}}),
            expected: json!({"contextualMetadata": {
                "summaryGeneration": DEFAULT_SUMMARY_GENERATION,
                "extendedAnalysis": mode
            }}),
        });
    }
    // Both modes stated: every pair of the enum's values is echoed, the documented
    // default included (an explicitly stated default is indistinguishable from a
    // resolved one).
    for summary in ANALYSIS_MODE_VALUES {
        for extended in ANALYSIS_MODE_VALUES {
            rows.push(Row {
                label: format!(
                    "contextualMetadata stating summaryGeneration={summary}, \
                     extendedAnalysis={extended}"
                ),
                submitted: json!({"contextualMetadata": {
                    "summaryGeneration": summary,
                    "extendedAnalysis": extended
                }}),
                expected: json!({"contextualMetadata": {
                    "summaryGeneration": summary,
                    "extendedAnalysis": extended
                }}),
            });
        }
    }
    // Every profanity-filter value the enum admits, stated: kept as given.
    for filter in PROFANITY_FILTER_VALUES {
        rows.push(Row {
            label: format!("subtitling stating profanityFilter={filter}"),
            submitted: json!({"subtitling": {
                "language": "deu",
                "aspectRatio": {"width": 4, "height": 3},
                "profanityFilter": filter
            }}),
            expected: json!({"subtitling": {
                "language": "deu",
                "aspectRatio": {"width": 4, "height": 3},
                "profanityFilter": filter
            }}),
        });
    }

    rows
}

/// `POST /v1/feed` with one output carrying `config`; returns (id, create response).
async fn create_feed_with(
    router: &MultiServiceRouter,
    config: &Value,
    label: &str,
) -> (String, Value) {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "config-defaults-feed",
            "outputs": [{"name": "out1", "outputConfig": config, "status": "ENABLED"}]
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {label:?}: CreateFeed answers 202, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the created feed carries an id: {created}"))
        .to_string();
    (id, created)
}

/// `GET /v1/feed/{id}`, asserting it is served.
async fn get_feed(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// A create reports the configuration in force — the request's members plus the
/// documented value for each one it omitted — and so does every later read.
#[tokio::test]
async fn omitted_config_members_report_their_documented_resolved_value() {
    for row in rows() {
        let label = &row.label;
        let router = router();
        let (id, created) = create_feed_with(&router, &row.submitted, label).await;
        assert_eq!(
            created["outputs"][0]["outputConfig"], row.expected,
            "case {label:?}: CreateFeed reports the configuration in force, got {:?}",
            created["outputs"][0]["outputConfig"]
        );

        let fetched = get_feed(&router, &id, label).await;
        assert_eq!(
            fetched["outputs"][0]["outputConfig"], row.expected,
            "case {label:?}: GetFeed reports the stored config, got {:?}",
            fetched["outputs"][0]["outputConfig"]
        );
    }
}

/// `UpdateFeed` (a full replacement) and `AssociateFeed` (an append) resolve the same
/// members, so no write path reports less than the create did.
#[tokio::test]
async fn update_and_associate_resolve_the_same_members() {
    for row in rows() {
        let label = &row.label;
        let router = router();
        let (id, _) = create_feed_with(&router, &output_config("cropping"), label).await;

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}"),
            Some(json!({
                "name": "config-defaults-feed",
                "outputs": [
                    {"name": "out1", "outputConfig": row.submitted, "status": "ENABLED"}
                ]
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            updated["outputs"][0]["outputConfig"], row.expected,
            "case {label:?}: UpdateFeed reports the configuration in force, got {:?}",
            updated["outputs"][0]["outputConfig"]
        );

        // The append path goes through the same store, so wait for the update the
        // clock owes to have fallen due before associating.
        past_the_transition_span();
        let (status, associated) = call(
            &router,
            "POST",
            &format!("/v1/feed/{id}/associate"),
            Some(json!({
                "associatedResourceName": "source-media",
                "outputs": [
                    {"name": "out2", "outputConfig": row.submitted, "status": "DISABLED"}
                ]
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: AssociateFeed answers 200, got {status} with {associated}"
        );
        let fetched = get_feed(&router, &id, label).await;
        assert_eq!(
            fetched["outputs"][1]["outputConfig"], row.expected,
            "case {label:?}: the appended output is stored with the members resolved, \
             got {:?}",
            fetched["outputs"][1]["outputConfig"]
        );
    }
}

/// A read-modify-write round trip is a FIXED POINT: PUTting back exactly the config
/// `GetFeed` reported leaves the stored config unchanged, so a client that replaces a
/// feed with what it read never silently drops the settings the service resolved.
#[tokio::test]
async fn a_read_modify_write_round_trip_does_not_drift() {
    for row in rows() {
        let label = &row.label;
        let router = router();
        let (id, _) = create_feed_with(&router, &row.submitted, label).await;

        let mut previous = get_feed(&router, &id, label).await["outputs"][0]["outputConfig"]
            .clone();
        // Twice, so a difference that only shows on the second rewrite is caught too.
        for pass in 1..=2 {
            let (status, updated) = call(
                &router,
                "PUT",
                &format!("/v1/feed/{id}"),
                Some(json!({
                    "name": "config-defaults-feed",
                    "outputs": [
                        {"name": "out1", "outputConfig": previous, "status": "ENABLED"}
                    ]
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?} pass {pass}: UpdateFeed answers 200, got {status} with \
                 {updated}"
            );
            let rewritten = updated["outputs"][0]["outputConfig"].clone();
            assert_eq!(
                rewritten, previous,
                "case {label:?} pass {pass}: writing back the config just read leaves it \
                 unchanged, got {rewritten:?}"
            );
            assert_eq!(
                rewritten, row.expected,
                "case {label:?} pass {pass}: the round trip keeps the configuration in \
                 force, got {rewritten:?}"
            );
            previous = rewritten;
        }
    }
}
