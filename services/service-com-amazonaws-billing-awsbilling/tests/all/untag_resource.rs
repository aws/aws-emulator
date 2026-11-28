// SPDX-License-Identifier: CC0-1.0

//! `UntagResource`, end to end over the awsJson1_0 wire.
//!
//! Each test pins the RULE the operation follows over a table of inputs spanning
//! the class the rule generalises over (how many keys one request names, which of
//! them the resource actually carries, key length, repetition, which billing-view
//! type owns the ARN) rather than one recorded request:
//!
//! * a request removes exactly the keys it names from the resource's stored tag
//!   set and leaves every other key — and its value — alone, which
//!   `ListTagsForResource` then reports;
//! * naming a key the resource does not carry removes nothing and is not a
//!   failure, so the operation is idempotent;
//! * taggability is a property of the resource, so a managed billing view is
//!   refused with 400 `ValidationException` and an ARN naming no stored resource
//!   with 404 `ResourceNotFoundException`, and neither refusal removes anything.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{
    call, emulator, emulator_with_engine, error_code, ABSENT_ARN, ACCOUNT_ID, PRIMARY_ARN,
};

/// `ResourceTagKey` is `@length` 1..128 and `ResourceTagValue` 0..256 (the
/// `ResourceTag` members, per `smithy_query(query="shape",shape="ResourceTag")`).
const MAX_TAG_KEY_LEN: usize = 128;

/// The tags `ListTagsForResource` reports for `arn`, as a key -> value map.
async fn stored_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = call(router, "ListTagsForResource", &json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource({arn}) must answer 200, got {status} {body:?}"
    );
    body["resourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTagsForResource({arn}) answered no resourceTags: {body:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Create one CUSTOM billing view carrying `tags` and return its ARN.
async fn view_with_tags(
    router: &MultiServiceRouter,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let (status, body) = call(
        router,
        "CreateBillingView",
        &json!({
            "name": name,
            "sourceViews": [PRIMARY_ARN],
            "resourceTags": tags
                .iter()
                .map(|(key, value)| json!({ "key": key, "value": value }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateBillingView must answer 200, got {status} {body:?}"
    );
    body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateBillingView answered no arn: {body:?}"))
        .to_string()
}

/// A pattern-valid billing-view ARN with resource id `id`.
fn view_arn(id: &str) -> String {
    format!("arn:aws:billing::{ACCOUNT_ID}:billingview/{id}")
}

/// One row of the removal table: the case name, the tags the view starts with and
/// the keys one `UntagResource` request names.
type UntagRow = (&'static str, Vec<(String, String)>, Vec<String>);

/// The rows of the removal table, spanning how many keys a request names, whether
/// they are carried, repeated or at the modeled key-length maximum.
fn untag_rows() -> Vec<UntagRow> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let stored = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    };
    let keys = |keys: &[&str]| -> Vec<String> { keys.iter().map(|key| key.to_string()).collect() };

    vec![
        (
            "one of three keys",
            stored(&[("env", "prod"), ("team", "finance"), ("owner", "ops")]),
            keys(&["team"]),
        ),
        (
            "two of three keys",
            stored(&[("env", "prod"), ("team", "finance"), ("owner", "ops")]),
            keys(&["env", "owner"]),
        ),
        (
            "every key the view carries",
            stored(&[("env", "prod"), ("team", "finance")]),
            keys(&["env", "team"]),
        ),
        (
            "no keys at all",
            stored(&[("env", "prod"), ("team", "finance")]),
            keys(&[]),
        ),
        (
            "a key the view never carried",
            stored(&[("env", "prod")]),
            keys(&["absent"]),
        ),
        (
            "a carried and an uncarried key together",
            stored(&[("env", "prod"), ("team", "finance")]),
            keys(&["env", "absent"]),
        ),
        (
            "the same key twice in one request",
            stored(&[("env", "prod"), ("team", "finance")]),
            keys(&["env", "env"]),
        ),
        (
            "a key whose value is empty",
            stored(&[("env", ""), ("team", "finance")]),
            keys(&["env"]),
        ),
        (
            "a key at the modeled maximum length",
            vec![
                (long_key.clone(), "v".to_string()),
                ("env".to_string(), "prod".to_string()),
            ],
            vec![long_key.clone()],
        ),
        (
            "a view holding exactly one key, removed",
            stored(&[("only", "one")]),
            keys(&["only"]),
        ),
        (
            "a view holding no tags",
            stored(&[]),
            keys(&["env", "team"]),
        ),
        (
            "many keys at once",
            (0..32)
                .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
                .collect(),
            (0..32)
                .filter(|index| index % 2 == 0)
                .map(|index| format!("key-{index:02}"))
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn untagging_removes_exactly_the_named_keys_and_keeps_the_rest() {
    let router = emulator();

    for (index, (case, start, removed)) in untag_rows().into_iter().enumerate() {
        let start: BTreeMap<String, String> = start.into_iter().collect();
        let arn = view_with_tags(&router, &format!("untag-{index}"), &start).await;

        let before = stored_tags(&router, &arn).await;
        assert_eq!(
            before, start,
            "case {case:?}: the create must store the tags it was given, got {before:?}"
        );

        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({ "resourceArn": arn, "resourceTagKeys": removed }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource to answer 200, got {status} {body:?}"
        );

        // The rule: exactly the named keys are gone; every other key keeps the
        // value it had.
        let mut expected = start.clone();
        for key in &removed {
            expected.remove(key);
        }
        let after = stored_tags(&router, &arn).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected the keys the request did not name, got {after:?}"
        );

        // Removal is idempotent: a replay of the same request answers 200 and
        // leaves the same set, because a key that is not carried is nothing to
        // remove.
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({ "resourceArn": arn, "resourceTagKeys": removed }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed UntagResource must answer 200, got {status} {body:?}"
        );
        let after = stored_tags(&router, &arn).await;
        assert_eq!(
            after, expected,
            "case {case:?}: a replayed UntagResource must change nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_removed_key_can_be_attached_again_with_a_new_value() {
    let router = emulator();
    let start: BTreeMap<String, String> =
        [("env".to_string(), "prod".to_string())].into_iter().collect();
    let arn = view_with_tags(&router, "round-trip", &start).await;

    // Untag then tag the same key: the stored value is the one the later request
    // carried, so removal really drops the entry rather than hiding it.
    for (case, value) in [
        ("reattached with a new value", "dev"),
        ("reattached with an empty value", ""),
        ("reattached with the original value", "prod"),
    ] {
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({ "resourceArn": arn, "resourceTagKeys": ["env"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource to answer 200, got {status} {body:?}"
        );
        let after = stored_tags(&router, &arn).await;
        assert!(
            !after.contains_key("env"),
            "case {case:?}: the removed key must be gone, got {after:?}"
        );

        let (status, body) = call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": arn,
                "resourceTags": [{ "key": "env", "value": value }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource to answer 200, got {status} {body:?}"
        );
        let after = stored_tags(&router, &arn).await;
        assert_eq!(
            after.get("env").map(String::as_str),
            Some(value),
            "case {case:?}: expected the reattached value, got {:?}",
            after.get("env")
        );
    }
}

#[tokio::test]
async fn untagging_refuses_a_managed_view_and_an_unknown_resource() {
    let (router, engine) = emulator_with_engine();
    let start: BTreeMap<String, String> = [
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "finance".to_string()),
    ]
    .into_iter()
    .collect();
    let custom = view_with_tags(&router, "refusals", &start).await;

    // A managed view's tags are not the customer's to remove: the modeled 400,
    // not a 404 and not a silent success.
    let (status, body) = call(
        &router,
        "UntagResource",
        &json!({ "resourceArn": PRIMARY_ARN, "resourceTagKeys": ["env"] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"managed view\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"managed view\": expected ValidationException, got {body:?}"
    );

    // An ARN naming no stored resource is the modeled 404, for every shape of
    // absent id — including a request that names no keys at all, which would
    // otherwise be a no-op.
    for (case, arn, keys) in [
        ("the absent arn", ABSENT_ARN.to_string(), vec!["env"]),
        (
            "an absent id in the minted form",
            view_arn("custom-a1b2c3d4-e5f6-47a8-9b0c-d1e2f3a4b5c6"),
            vec!["env"],
        ),
        ("a one-character absent id", view_arn("q"), vec!["env"]),
        (
            "an absent non-billingview resource",
            format!("arn:aws:billing::{ACCOUNT_ID}:budget/none"),
            vec!["env"],
        ),
        ("an absent arn and no keys", view_arn("gone"), vec![]),
    ] {
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({ "resourceArn": arn, "resourceTagKeys": keys }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // Every refusal above removed nothing — from the custom view a later caller
    // reads, and from the managed view whose tag set no read can report.
    let after = stored_tags(&router, &custom).await;
    assert_eq!(
        after, start,
        "a refused UntagResource must remove no tag, got {after:?}"
    );

    {
        let state = service_com_amazonaws_billing_awsbilling::state::service_state(&*engine)
            .expect("the billing state is reachable from the shared engine");
        let mut state = state.lock();
        let primary = state
            .billing_views
            .get_mut(PRIMARY_ARN)
            .expect("the account's PRIMARY view exists before any call");
        primary
            .tags
            .insert("seeded".to_string(), Some("v".to_string()));
    }

    let (status, body) = call(
        &router,
        "UntagResource",
        &json!({ "resourceArn": PRIMARY_ARN, "resourceTagKeys": ["seeded"] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"managed view carrying a tag\": expected HTTP 400, got {status} {body:?}"
    );
    let state = service_com_amazonaws_billing_awsbilling::state::service_state(&*engine)
        .expect("the billing state is reachable from the shared engine");
    let state = state.lock();
    let primary = state
        .billing_views
        .get(PRIMARY_ARN)
        .expect("the account's PRIMARY view is still stored");
    assert_eq!(
        primary.tags.get("seeded").map(Option::as_deref),
        Some(Some("v")),
        "a refused UntagResource must leave the managed view's tags alone, got {:?}",
        primary.tags
    );
}
