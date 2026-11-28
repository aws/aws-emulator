// SPDX-License-Identifier: CC0-1.0

//! The tagging plane's ONE decision about the resource an ARN names, driven end
//! to end over the awsJson1_0 wire through each of the three operations that
//! take it — `TagResource`, `UntagResource` and `ListTagsForResource`.
//!
//! Taggability is a property of the RESOURCE, not of the direction of the call,
//! so all three resolve their target through the same decision in
//! `crate::state` and each only lifts its answer into the modeled error its own
//! operation declares. What this suite pins is that the three cannot drift
//! apart: for one ARN they answer the SAME status and the SAME payload —
//! `message`, `reason` and `fieldList` for a view AWS manages, and `message`,
//! `resourceId` and `resourceType` for an ARN nothing is stored under — and a
//! stored CUSTOM view is served by all three.
//!
//! The rows span what the decision generalises over: every non-`CUSTOM`
//! `BillingViewType` the model declares (so no managed type is taggable through
//! any of the three), whether the managed view carries tags at all, and the
//! shapes an absent ARN can take (the minted form, a one-character id, a
//! resource type this service stores nothing of).
//!
//! The harness (`call`, `error_code`, `plant_view`, …) is the one
//! `billing_views.rs` owns.
//!
//! What this suite owns, against its neighbour: `refusal_and_projection_fidelity.rs`
//! owns the SENTENCE each refusal carries (that the managed-view refusal is not a
//! not-found one, and that the tagging not-found sentence does not echo the ARN
//! while the billing-view reads' does). What is asserted here is that the three
//! operations take ONE decision: the whole modeled payload — `reason`,
//! `fieldList`, `resourceId`, `resourceType` — over every managed
//! `BillingViewType` and every shape of absent ARN, and the three answers being
//! identical to each other.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::ops::types::BILLING_VIEW_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{
    call, emulator_with_engine, error_code, plant_view, ABSENT_ARN, ACCOUNT_ID, PRIMARY_ARN,
};
use crate::rejection_fidelity::message;

/// The sentence a tagging request against a view AWS manages is refused with
/// (`state::MANAGED_VIEW_NOT_TAGGABLE`). Spelled out here so the expectation is
/// written independently of the service code.
const MANAGED_NOT_TAGGABLE: &str = "Managed billing views cannot be tagged";

/// The fixed sentence the tagging APIs answer for a resource that does not
/// exist (`state::RESOURCE_NOT_FOUND_MESSAGE`).
const RESOURCE_NOT_FOUND: &str = "Resource not found";

/// The `ValidationException$reason` a member-level refusal carries
/// (`state::VALIDATION_REASON_FIELD`).
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The `ResourceNotFoundException$resourceType` reported for a billing view
/// (`state::BILLING_VIEW_RESOURCE_TYPE`).
const BILLING_VIEW_RESOURCE_TYPE: &str = "billingview";

/// The `BillingViewType` a caller's own view carries, the one type the tagging
/// plane admits (`BILLING_VIEW_TYPE_VALUES` in `ops::types`).
const CUSTOM: &str = "CUSTOM";

/// A pattern-valid billing-view ARN of the caller's account with resource id
/// `id`.
fn view_arn(id: &str) -> String {
    format!("arn:aws:billing::{ACCOUNT_ID}:billingview/{id}")
}

/// One request body per tagging operation, each naming `arn` as its resource, so
/// one ARN is put to all three operations of the plane.
fn tagging_calls(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({ "resourceArn": arn, "resourceTags": [{ "key": "env", "value": "prod" }] }),
        ),
        (
            "UntagResource",
            json!({ "resourceArn": arn, "resourceTagKeys": ["env"] }),
        ),
        ("ListTagsForResource", json!({ "resourceArn": arn })),
    ]
}

/// The status and body each of the three tagging operations answers for `arn`.
async fn tagging_answers(
    router: &MultiServiceRouter,
    arn: &str,
) -> Vec<(&'static str, u16, Value)> {
    let mut answers = Vec::new();
    for (operation, body) in tagging_calls(arn) {
        let (status, answered) = call(router, operation, &body).await;
        answers.push((operation, status, answered));
    }
    answers
}

/// The tags `ListTagsForResource` reports for `arn`, as a key -> value map.
async fn reported_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call(router, "ListTagsForResource", &json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read-back must answer 200, got {status} {body:?}"
    );
    body["resourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the read answered no resourceTags: {body:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_tagging_operation_refuses_a_managed_view_identically() {
    let (router, engine) = emulator_with_engine();

    // The rows: every `BillingViewType` the model declares except the one a
    // caller owns, each both without tags and carrying one, so the decision is
    // about the view's TYPE and not about what its stored tag set holds.
    let mut rows: Vec<(String, String)> = Vec::new();
    for view_type in BILLING_VIEW_TYPE_VALUES
        .iter()
        .filter(|view_type| **view_type != CUSTOM)
    {
        for (suffix, tagged) in [("bare", false), ("tagged", true)] {
            let case = format!("{view_type} ({suffix})");
            let arn = view_arn(&format!("{}-{suffix}", view_type.to_lowercase()));
            plant_view(&engine, &arn, |view| {
                view.billing_view_type = (*view_type).to_string();
                view.name = format!("managed-{suffix}");
                view.tags.clear();
                if tagged {
                    view.tags
                        .insert("env".to_string(), Some("staging".to_string()));
                }
            });
            rows.push((case, arn));
        }
    }
    // The account's own seeded default view is the row no suite has to plant.
    rows.push(("the seeded PRIMARY view".to_string(), PRIMARY_ARN.to_string()));

    for (case, arn) in &rows {
        let answers = tagging_answers(&router, arn).await;
        for (operation, status, body) in &answers {
            assert_eq!(
                *status, 400,
                "case {case:?}/{operation}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(body),
                "ValidationException",
                "case {case:?}/{operation}: expected ValidationException, got {body:?}"
            );
            assert_eq!(
                message(body),
                MANAGED_NOT_TAGGABLE,
                "case {case:?}/{operation}: expected the managed-view sentence, got {body:?}"
            );
            assert_eq!(
                body["reason"].as_str(),
                Some(REASON_FIELD_VALIDATION_FAILED),
                "case {case:?}/{operation}: expected the field-validation reason, got {body:?}"
            );
            assert_eq!(
                (
                    body["fieldList"][0]["name"].as_str(),
                    body["fieldList"][0]["message"].as_str()
                ),
                (Some("resourceArn"), Some(MANAGED_NOT_TAGGABLE)),
                "case {case:?}/{operation}: expected the refusal over resourceArn, got {body:?}"
            );
        }
        // One decision, so one payload: the three operations answer the same
        // ARN with the same bytes, whichever direction the call was in.
        let (first_operation, _, first) = &answers[0];
        for (operation, _, body) in &answers[1..] {
            assert_eq!(
                body, first,
                "case {case:?}: {operation} and {first_operation} must answer the same refusal, \
                 got {body:?} and {first:?}"
            );
        }
    }

    // And the refusals changed nothing: the managed view that carried a tag
    // still carries it, and the one that carried none still carries none.
    let state = service_com_amazonaws_billing_awsbilling::state::service_state(&*engine)
        .expect("the billing state is reachable from the shared engine");
    let state = state.lock();
    for (case, arn) in &rows {
        let view = state
            .billing_views
            .get(arn.as_str())
            .unwrap_or_else(|| panic!("case {case:?}: the managed view is still stored"));
        let expected = usize::from(case.ends_with("(tagged)"));
        assert_eq!(
            view.tags.len(),
            expected,
            "case {case:?}: a refused tagging call must leave the stored tags alone, got {:?}",
            view.tags
        );
    }
}

#[tokio::test]
async fn every_tagging_operation_refuses_an_unknown_resource_identically() {
    let (router, _engine) = emulator_with_engine();

    // The rows: the shapes an ARN nothing is stored under can take, all of them
    // inside the caller's own account so the account boundary is not what
    // answers.
    for (case, arn) in [
        ("the absent arn", ABSENT_ARN.to_string()),
        (
            "an absent id in the minted form",
            view_arn("custom-a1b2c3d4-e5f6-47a8-9b0c-d1e2f3a4b5c6"),
        ),
        ("a one-character absent id", view_arn("q")),
        (
            "an absent non-billingview resource",
            format!("arn:aws:billing::{ACCOUNT_ID}:budget/none"),
        ),
        (
            // The modeled `ResourceArn` admits 71 characters after the account
            // id, which the `billingview/` prefix takes 12 of (the pattern
            // `^arn:aws[a-z-]*:(billing)::[0-9]{12}:[a-zA-Z0-9/:_\+=\.\@-]{0,70}[a-zA-Z0-9]$`,
            // per `smithy_query(query="shape",shape="ResourceArn")`).
            "an id filling the modeled arn",
            view_arn(&"z".repeat(59)),
        ),
    ] {
        let answers = tagging_answers(&router, &arn).await;
        for (operation, status, body) in &answers {
            assert_eq!(
                *status, 404,
                "case {case:?}/{operation}: expected HTTP 404, got {status} {body:?}"
            );
            assert_eq!(
                error_code(body),
                "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {body:?}"
            );
            assert_eq!(
                message(body),
                RESOURCE_NOT_FOUND,
                "case {case:?}/{operation}: expected the fixed not-found sentence, got {body:?}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(arn.as_str()),
                "case {case:?}/{operation}: expected the requested ARN in resourceId, got {body:?}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(BILLING_VIEW_RESOURCE_TYPE),
                "case {case:?}/{operation}: expected the billing-view resourceType, got {body:?}"
            );
        }
        let (first_operation, _, first) = &answers[0];
        for (operation, _, body) in &answers[1..] {
            assert_eq!(
                body, first,
                "case {case:?}: {operation} and {first_operation} must answer the same refusal, \
                 got {body:?} and {first:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_stored_custom_view_is_served_by_every_tagging_operation() {
    let (router, engine) = emulator_with_engine();

    // The control: the one type the decision admits is admitted through all
    // three operations, so the shared refusal is a refusal and not a gate. The
    // calls run in order with a READ between them, so each write is observed
    // through the reader rather than inferred from its own status: the attach
    // first, then the removal.
    let case = "a stored CUSTOM view";
    let arn = view_arn("served");
    plant_view(&engine, &arn, |view| {
        view.billing_view_type = CUSTOM.to_string();
        view.name = "served".to_string();
        view.tags.clear();
    });

    let attached = [("env", "prod"), ("team", "finance")];
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": arn,
            "resourceTags": attached
                .iter()
                .map(|(key, value)| json!({ "key": key, "value": value }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}/TagResource: expected HTTP 200, got {status} {body:?}"
    );

    assert_eq!(
        reported_tags(&router, &arn, case).await,
        attached
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<String, String>>(),
        "case {case:?}: expected the read to report what the attach wrote"
    );

    let (status, body) = call(
        &router,
        "UntagResource",
        &json!({ "resourceArn": arn, "resourceTagKeys": ["env", "team"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}/UntagResource: expected HTTP 200, got {status} {body:?}"
    );

    let after = reported_tags(&router, &arn, case).await;
    assert!(
        after.is_empty(),
        "case {case:?}: expected the removal to leave no tag, got {after:?}"
    );
}
