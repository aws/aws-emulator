// SPDX-License-Identifier: CC0-1.0

//! Which parts of a billing view's data filter are its DEFINITION, over the
//! awsJson1_0 wire.
//!
//! One rule: `viewDefinitionLastUpdatedAt` records when the view's definition was
//! last changed, and the definition is the WHOLE stored data filter expression —
//! so an update moves that instant exactly when any part of the expression it
//! sends differs from the part stored, and leaves it where it stood when every
//! part is the same.
//!
//! `billing_view_definition_instant.rs` spans the three filter kinds a billing
//! view supports (`LINKED_ACCOUNT` dimensions, `Tags`, `CostCategories`); the
//! rows here span the parts of an expression that table holds fixed — the two
//! bounds of the filter's `timeRange`, each moved, dropped and restored, and the
//! ORDER of a filter's values, which every read hands back as it was stored and
//! so is part of what the stored definition says. Each row states whether it
//! changes the definition, and the walk sends every row twice: once as the row
//! states it, then again unchanged, which cannot change a definition whatever
//! the row did.
//!
//! The harness (`call`, `emulator`, …) is the one `billing_views.rs` owns.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{call, emulator, ACCOUNT_ID, PRIMARY_ARN};
use crate::credit_allocation_history::at;

/// One step of the walk: the expression it sends, and whether that expression
/// differs from the one the view holds when the step runs.
struct Step {
    case: &'static str,
    expression: Value,
    changes: bool,
}

/// A `timeRange` carrying the bounds it is given, each in epoch seconds.
fn period(begin: Option<f64>, end: Option<f64>) -> Value {
    let mut range: Map<String, Value> = Map::new();
    if let Some(begin) = begin {
        range.insert("beginDateInclusive".to_string(), json!(begin));
    }
    if let Some(end) = end {
        range.insert("endDateInclusive".to_string(), json!(end));
    }
    Value::Object(range)
}

/// A tag filter on `key` over `values`, in the order they are given, with the
/// period `range` beside it.
fn tags_over(key: &str, values: &[&str], range: Value) -> Value {
    json!({ "tags": { "key": key, "values": values }, "timeRange": range })
}

/// The steps, in the order the walk sends them. Each one's `changes` is stated
/// here rather than derived, so the table says what the rule requires instead of
/// recomputing it.
fn steps() -> Vec<Step> {
    let march = at(2024, 3, 1, 0) as f64;
    let april = at(2024, 4, 1, 0) as f64;
    let june = at(2024, 6, 1, 0) as f64;
    vec![
        Step {
            case: "the first filter, a period closed at both ends",
            expression: tags_over("team", &["finance", "ops"], period(Some(march), Some(june))),
            changes: true,
        },
        Step {
            case: "the begin bound moved forward",
            expression: tags_over("team", &["finance", "ops"], period(Some(april), Some(june))),
            changes: true,
        },
        Step {
            case: "the end bound moved back, still later than the begin",
            expression: tags_over(
                "team",
                &["finance", "ops"],
                period(Some(april), Some(june - 1.0)),
            ),
            changes: true,
        },
        Step {
            case: "the end bound moved forward",
            expression: tags_over(
                "team",
                &["finance", "ops"],
                period(Some(april), Some(june + 1.0)),
            ),
            changes: true,
        },
        Step {
            case: "the end bound dropped — a period open at its end",
            expression: tags_over("team", &["finance", "ops"], period(Some(april), None)),
            changes: true,
        },
        Step {
            case: "the begin bound dropped too — a period open at both ends",
            expression: tags_over("team", &["finance", "ops"], period(None, None)),
            changes: true,
        },
        Step {
            case: "the end bound restored, the begin still open",
            expression: tags_over("team", &["finance", "ops"], period(None, Some(june))),
            changes: true,
        },
        Step {
            case: "the same values in the other order",
            expression: tags_over("team", &["ops", "finance"], period(None, Some(june))),
            changes: true,
        },
        Step {
            case: "the values back in the first order",
            expression: tags_over("team", &["finance", "ops"], period(None, Some(june))),
            changes: true,
        },
        Step {
            case: "the same filter with a sub-second begin bound",
            expression: tags_over(
                "team",
                &["finance", "ops"],
                period(Some(march + 0.25), Some(june)),
            ),
            changes: true,
        },
        Step {
            case: "that begin bound a quarter of a second later",
            expression: tags_over(
                "team",
                &["finance", "ops"],
                period(Some(march + 0.5), Some(june)),
            ),
            changes: true,
        },
        Step {
            case: "a dimension filter beside the period",
            expression: json!({
                "dimensions": { "key": "LINKED_ACCOUNT", "values": [ACCOUNT_ID] },
                "timeRange": period(Some(march + 0.5), Some(june)),
            }),
            changes: true,
        },
    ]
}

/// The parts of a reported expression, on the scales a request names them on:
/// each filter member as its key and its values IN ORDER, and the two bounds of
/// the period as epoch seconds. Both sides of an assertion are read through
/// here, so a comparison is about the parts and not about how a number was
/// rendered.
fn parts(expression: &Value) -> Value {
    json!({
        "dimensions": filter(&expression["dimensions"]),
        "tags": filter(&expression["tags"]),
        "costCategories": filter(&expression["costCategories"]),
        "beginDateInclusive": expression["timeRange"]["beginDateInclusive"].as_f64(),
        "endDateInclusive": expression["timeRange"]["endDateInclusive"].as_f64(),
    })
}

/// One filter member as its key and its values, in order.
fn filter(member: &Value) -> Value {
    json!({ "key": member["key"].as_str(), "values": member["values"].as_array() })
}

/// The projection `GetBillingView` reports for `arn`.
async fn view_of(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, got) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetBillingView to answer 200 for {arn}, got {status} {got:?}"
    );
    got["billingView"].clone()
}

#[tokio::test]
async fn the_definition_instant_follows_every_part_of_the_stored_filter() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "CreateBillingView",
        &json!({
            "name": "definition-parts",
            "sourceViews": [PRIMARY_ARN],
            "clientToken": "definition-parts",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the harness create must answer 200, got {status} {created:?}"
    );
    let arn = created["arn"].as_str().unwrap_or_default().to_string();

    // A create records no definition instant, so the walk starts from an absent
    // one.
    let view = view_of(&router, "the created view", &arn).await;
    assert!(
        view["viewDefinitionLastUpdatedAt"].is_null(),
        "case \"the created view\": a created view carries no viewDefinitionLastUpdatedAt, \
         got {:?}",
        view["viewDefinitionLastUpdatedAt"]
    );

    let mut instant = Value::Null;
    for step in steps() {
        // Each expression as the row states it, then again unchanged: a re-sent
        // expression changes no definition whatever the row itself did.
        for (attempt, changes) in [("as stated", step.changes), ("re-sent unchanged", false)] {
            let case = format!("{} / {attempt}", step.case);
            let (status, updated) = call(
                &router,
                "UpdateBillingView",
                &json!({ "arn": arn, "dataFilterExpression": step.expression.clone() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateBillingView to answer 200, got {status} {updated:?}"
            );
            let stored = view_of(&router, &case, &arn).await;
            assert_eq!(
                parts(&stored["dataFilterExpression"]),
                parts(&step.expression),
                "case {case:?}: expected the stored filter to be {:?}, got {:?}",
                step.expression,
                stored["dataFilterExpression"]
            );
            let got = stored["viewDefinitionLastUpdatedAt"].clone();
            if changes {
                assert!(
                    got.is_number(),
                    "case {case:?}: a changed definition stamps the instant, got {got:?}"
                );
                assert_ne!(
                    got, instant,
                    "case {case:?}: a changed definition moves the instant off {instant:?}, \
                     got {got:?}"
                );
                instant = got;
            } else {
                assert_eq!(
                    got, instant,
                    "case {case:?}: an unchanged definition leaves the instant at {instant:?}, \
                     got {got:?}"
                );
            }
        }
    }
}
