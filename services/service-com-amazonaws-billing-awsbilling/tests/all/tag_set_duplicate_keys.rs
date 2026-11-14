// SPDX-License-Identifier: CC0-1.0

//! The one rule every writer of a billing view's tag set applies to the
//! `resourceTags` list it is handed, end to end over the awsJson1_0 wire.
//!
//! A tag set is keyed by tag key, so one request may not carry the same key
//! twice: the stored value would otherwise be decided by the order the entries
//! happened to arrive in. Both operations that take a `ResourceTagList` —
//! `CreateBillingView`, which mints a view holding the request's tags, and
//! `TagResource`, which merges them into a stored view — therefore refuse such a
//! list with `ValidationException` (400) over the `resourceTags` member and
//! write nothing, and accept every list whose keys are distinct, storing each
//! entry's value.
//!
//! The table spans what the rule generalises over: where in the list the repeat
//! sits, how many entries the list holds, how many times a key repeats, whether
//! the repeated entries' values differ, whether `value` is carried at all, and
//! the key's length at the modelled bounds (`ResourceTagKey` is `@length`
//! 1..128, `ResourceTagValue` `@length` 0..256, `ResourceTagList` `@length`
//! 0..200).

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{call, emulator_with_engine, error_code, PRIMARY_ARN};

/// One `resourceTags` entry.
fn tag(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": value })
}

/// One `resourceTags` entry that omits the optional `value` member.
fn key_only(key: &str) -> Value {
    json!({ "key": key })
}

/// One row of the table: a `resourceTags` list and what every writer of a tag
/// set must do with it.
struct TagListRow {
    case: &'static str,
    /// The list, as the wire carries it.
    tags: Vec<Value>,
    /// The tag set the resource must end up holding, or `None` when the list
    /// must be refused and nothing written.
    expected: Option<BTreeMap<String, String>>,
}

/// A row whose list must be refused.
fn refused(case: &'static str, tags: Vec<Value>) -> TagListRow {
    TagListRow {
        case,
        tags,
        expected: None,
    }
}

/// A row whose list must be stored, every entry's value under its own key (an
/// entry that omits `value` stores the empty value the model's 0-length
/// `ResourceTagValue` allows).
fn accepted(case: &'static str, tags: Vec<Value>) -> TagListRow {
    let expected = tags
        .iter()
        .map(|entry| {
            (
                entry["key"].as_str().unwrap_or_default().to_string(),
                entry
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    TagListRow {
        case,
        tags,
        expected: Some(expected),
    }
}

fn tag_list_rows() -> Vec<TagListRow> {
    // The bounds the model itself states for a tag key and for one request's
    // list: `ResourceTagKey` @length 1..128 and `ResourceTagList` @length 0..200
    // (smithy_query query="shape" shape="ResourceTagKey,ResourceTagList").
    let longest_key = "k".repeat(128);
    let other_longest_key = format!("j{}", "k".repeat(127));
    let shortest_key = "a";

    // A list that fills the modelled maximum with distinct keys, and the same
    // list with its first key repeated at the very end.
    let full_list: Vec<Value> = (0..200)
        .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index}")))
        .collect();
    let mut full_list_with_a_repeat: Vec<Value> = full_list[..199].to_vec();
    full_list_with_a_repeat.push(tag("key-000", "repeated at the end"));

    vec![
        refused(
            "two adjacent entries of one key with different values",
            vec![tag("env", "dev"), tag("env", "prod")],
        ),
        refused(
            "two entries of one key carrying the SAME value",
            vec![tag("env", "dev"), tag("env", "dev")],
        ),
        refused(
            "a repeat separated by other keys",
            vec![
                tag("owner", "team-a"),
                tag("env", "dev"),
                tag("cost-center", "42"),
                tag("env", "prod"),
            ],
        ),
        refused(
            "the first key repeated at the end of a full list",
            full_list_with_a_repeat,
        ),
        refused(
            "one key given three times",
            vec![
                tag("env", "dev"),
                tag("env", "test"),
                tag("env", "prod"),
                tag("other", "kept"),
            ],
        ),
        refused(
            "a repeat where one entry omits value",
            vec![tag("env", "dev"), key_only("env")],
        ),
        refused(
            "a repeat where both entries omit value",
            vec![key_only("env"), key_only("env")],
        ),
        refused(
            "a repeat of the shortest allowed key",
            vec![tag(shortest_key, "one"), tag(shortest_key, "two")],
        ),
        refused(
            "a repeat of the longest allowed key",
            vec![tag(&longest_key, "one"), tag(&longest_key, "two")],
        ),
        refused(
            "a repeat whose values are empty",
            vec![tag("env", ""), tag("env", "")],
        ),
        accepted("no tags at all", Vec::new()),
        accepted("a single tag", vec![tag("env", "dev")]),
        accepted(
            "two distinct keys, one of them with an empty value",
            vec![tag("env", "dev"), tag("owner", "")],
        ),
        accepted(
            "distinct keys where one omits value entirely",
            vec![tag("env", "dev"), key_only("owner")],
        ),
        accepted(
            "keys that differ only in case, so they are distinct keys",
            vec![tag("Env", "dev"), tag("env", "prod")],
        ),
        accepted(
            "keys that differ only in their last character, at the length maximum",
            vec![tag(&longest_key, "one"), tag(&other_longest_key, "two")],
        ),
        accepted("the modelled maximum of distinct keys", full_list),
        accepted(
            "a value at the modelled length maximum",
            vec![tag("env", &"v".repeat(256))],
        ),
    ]
}

/// Create one CUSTOM billing view with no tags and return its ARN.
async fn create_bare_view(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateBillingView",
        &json!({ "name": name, "sourceViews": [PRIMARY_ARN] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the harness create must answer 200, got {status} {body:?}"
    );
    body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: the harness create answered no arn: {body:?}"))
        .to_string()
}

/// The tags `ListTagsForResource` reports for `arn`, as a key -> value map.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = call(router, "ListTagsForResource", &json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource({arn}) must answer 200, got {status} {body:?}"
    );
    body["resourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTagsForResource({arn}) answered no resourceTags: {body:?}"))
        .iter()
        .map(|entry| {
            (
                entry["key"].as_str().unwrap_or_default().to_string(),
                entry["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every billing-view ARN `ListBillingViews` reports, so a refused create can be
/// shown to have stored nothing.
async fn listed_arns(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListBillingViews", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListBillingViews must answer 200, got {status} {body:?}"
    );
    body["billingViews"]
        .as_array()
        .unwrap_or_else(|| panic!("ListBillingViews answered no billingViews: {body:?}"))
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A refusal of the `resourceTags` member: HTTP 400, the modeled
/// `ValidationException`, the field-level reason and the member named in
/// `fieldList`, whichever writer answered.
fn assert_refuses_resource_tags(case: &str, writer: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?} ({writer}): expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?} ({writer}): expected ValidationException, got {body:?}"
    );
    assert_eq!(
        body["reason"].as_str(),
        Some("fieldValidationFailed"),
        "case {case:?} ({writer}): expected the field-level reason, got {body:?}"
    );
    let named: Vec<&str> = body["fieldList"]
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .map(|field| field["name"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        named.contains(&"resourceTags"),
        "case {case:?} ({writer}): the refusal must name resourceTags, got {body:?}"
    );
}

#[tokio::test]
async fn create_billing_view_applies_the_duplicate_tag_key_rule() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in tag_list_rows().iter().enumerate() {
        let before = listed_arns(&router).await;
        let name = format!("create-row-{index}");
        let (status, body) = call(
            &router,
            "CreateBillingView",
            &json!({
                "name": name,
                "sourceViews": [PRIMARY_ARN],
                "resourceTags": row.tags,
            }),
        )
        .await;

        let Some(expected) = row.expected.as_ref() else {
            assert_refuses_resource_tags(row.case, "CreateBillingView", status, &body);
            assert_eq!(
                listed_arns(&router).await,
                before,
                "case {:?} (CreateBillingView): a refused create must store no view",
                row.case
            );
            continue;
        };

        assert_eq!(
            status, 200,
            "case {:?} (CreateBillingView): expected HTTP 200, got {status} {body:?}",
            row.case
        );
        let arn = body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: create answered no arn: {body:?}", row.case))
            .to_string();
        let stored = list_tags(&router, &arn).await;
        assert_eq!(
            &stored, expected,
            "case {:?} (CreateBillingView): expected {expected:?}, got {stored:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn tag_resource_applies_the_duplicate_tag_key_rule() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in tag_list_rows().iter().enumerate() {
        let arn = create_bare_view(&router, &format!("tag-row-{index}")).await;
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "resourceArn": arn, "resourceTags": row.tags }),
        )
        .await;

        let Some(expected) = row.expected.as_ref() else {
            assert_refuses_resource_tags(row.case, "TagResource", status, &body);
            let stored = list_tags(&router, &arn).await;
            assert!(
                stored.is_empty(),
                "case {:?} (TagResource): a refused request must attach nothing, got {stored:?}",
                row.case
            );
            continue;
        };

        assert_eq!(
            status, 200,
            "case {:?} (TagResource): expected HTTP 200, got {status} {body:?}",
            row.case
        );
        let stored = list_tags(&router, &arn).await;
        assert_eq!(
            &stored, expected,
            "case {:?} (TagResource): expected {expected:?}, got {stored:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_create_refused_for_a_duplicated_tag_key_consumes_no_client_token() {
    let (router, _engine) = emulator_with_engine();
    let token = "dup-tags-token";

    let (status, body) = call(
        &router,
        "CreateBillingView",
        &json!({
            "name": "token-not-consumed",
            "sourceViews": [PRIMARY_ARN],
            "clientToken": token,
            "resourceTags": [tag("env", "dev"), tag("env", "prod")],
        }),
    )
    .await;
    assert_refuses_resource_tags(
        "a refused create under a client token",
        "CreateBillingView",
        status,
        &body,
    );

    // The refusal stored nothing at all, the token included: the same token with
    // a well-formed list is a FIRST create, not a replay of a view that was
    // never minted.
    let (status, body) = call(
        &router,
        "CreateBillingView",
        &json!({
            "name": "token-not-consumed",
            "sourceViews": [PRIMARY_ARN],
            "clientToken": token,
            "resourceTags": [tag("env", "prod")],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the token after a refusal\": expected HTTP 200, got {status} {body:?}"
    );
    let arn = body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the token after a refusal\": no arn: {body:?}"))
        .to_string();
    let stored = list_tags(&router, &arn).await;
    assert_eq!(
        stored,
        BTreeMap::from([("env".to_string(), "prod".to_string())]),
        "case \"the token after a refusal\": expected the second request's tags, got {stored:?}"
    );
}
