// SPDX-License-Identifier: CC0-1.0

//! An optional tag value is stored and reported as the request gave it, end to
//! end over the awsJson1_0 wire.
//!
//! `ResourceTag$value` is optional and `ResourceTagValue` is `@length` 0..256, so
//! an entry that omits the member and an entry that sends `""` are two different
//! inputs, and the member `ListTagsForResource` fills is optional too. The rule
//! these tests pin, over a table of entry forms rather than one request: a tag
//! reads back carrying exactly the value its writer submitted, and carrying NO
//! value when its writer submitted none — for both writers of a tag set
//! (`CreateBillingView` and `TagResource`), and across a re-tag of the same key.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{call, emulator, PRIMARY_ARN};

/// The modelled maximum length of a tag value — `ResourceTagValue` is `@length`
/// 0..256 and `ResourceTag$value` carries no `@required`
/// (`smithy_query(query="shape", shape="ResourceTag")`), so the minimum is the
/// empty value and "no value at all" is a third, distinct input.
const MAX_TAG_VALUE_LEN: usize = 256;

/// The modelled maximum length of a tag key — `ResourceTagKey` is `@length`
/// 1..128 (same shape query as above).
const MAX_TAG_KEY_LEN: usize = 128;

/// One row of the entry table: the case, the tag key, and the value the request
/// carries (`None` for an entry that omits the `value` member entirely).
struct TagEntryRow {
    case: &'static str,
    key: String,
    value: Option<String>,
}

impl TagEntryRow {
    /// This row as one `resourceTags` entry on the wire. An entry with no value
    /// carries no `value` member at all, which is what distinguishes it from one
    /// carrying the empty value.
    fn entry(&self) -> Value {
        match &self.value {
            Some(value) => json!({ "key": self.key, "value": value }),
            None => json!({ "key": self.key }),
        }
    }
}

/// The rows span what the rule generalises over: whether the entry carries the
/// optional member at all, and — when it does — the value's length (the empty
/// value, one character, the modelled maximum) and its characters. The key forms
/// vary with them (the shortest and the longest key the model allows, mixed
/// case, punctuation), so no row is the recorded `{"key": "team"}` renamed.
fn tag_entry_rows() -> Vec<TagEntryRow> {
    vec![
        TagEntryRow {
            case: "an entry that omits the value member entirely",
            key: "team".to_string(),
            value: None,
        },
        TagEntryRow {
            case: "the shortest allowed key, omitting the value member",
            key: "a".to_string(),
            value: None,
        },
        TagEntryRow {
            case: "the longest allowed key, omitting the value member",
            key: "k".repeat(MAX_TAG_KEY_LEN),
            value: None,
        },
        TagEntryRow {
            case: "an entry carrying the empty value",
            key: "Env".to_string(),
            value: Some(String::new()),
        },
        TagEntryRow {
            case: "an entry carrying a one-character value",
            key: "owner".to_string(),
            value: Some("x".to_string()),
        },
        TagEntryRow {
            case: "an entry carrying a value at the modelled length maximum",
            key: "cost-center".to_string(),
            value: Some("v".repeat(MAX_TAG_VALUE_LEN)),
        },
        TagEntryRow {
            case: "an entry carrying a value of spaces and punctuation",
            key: "Cost Center/42".to_string(),
            value: Some("finance: q4 + q1 (eu-west)".to_string()),
        },
    ]
}

/// The tags `ListTagsForResource` reports for `arn`: every reported key with the
/// value member the response carries for it, and `None` where the response
/// carries none — the distinction the stored tag set has to preserve.
async fn reported_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, Option<String>> {
    let (status, body) = call(router, "ListTagsForResource", &json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {status} {body:?}"
    );
    body["resourceTags"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListTagsForResource answered no resourceTags: {body:?}")
        })
        .iter()
        .map(|tag| {
            let key = tag["key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a reported tag has no key: {tag:?}"))
                .to_string();
            let value = match tag.get("value") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: tag {key:?} reports a non-string value: {tag:?}")
                        })
                        .to_string(),
                ),
            };
            (key, value)
        })
        .collect()
}

/// Create one CUSTOM billing view named `name` carrying `entries`, and return its
/// ARN.
async fn create_view(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    entries: &[Value],
) -> String {
    let (status, body) = call(
        router,
        "CreateBillingView",
        &json!({ "name": name, "sourceViews": [PRIMARY_ARN], "resourceTags": entries }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBillingView must answer 200, got {status} {body:?}"
    );
    body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateBillingView answered no arn: {body:?}"))
        .to_string()
}

/// Attach `entries` to `arn` and require the call to be answered.
async fn tag_resource(router: &MultiServiceRouter, case: &str, arn: &str, entries: &[Value]) {
    let (status, body) = call(
        router,
        "TagResource",
        &json!({ "resourceArn": arn, "resourceTags": entries }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource must answer 200, got {status} {body:?}"
    );
}

#[tokio::test]
async fn create_billing_view_stores_the_tag_value_member_it_was_given() {
    let router = emulator();

    for (index, row) in tag_entry_rows().iter().enumerate() {
        let arn = create_view(
            &router,
            row.case,
            &format!("create-tag-value-{index}"),
            &[row.entry()],
        )
        .await;
        let reported = reported_tags(&router, &arn, row.case).await;
        assert_eq!(
            reported,
            BTreeMap::from([(row.key.clone(), row.value.clone())]),
            "case {case:?}: the created view must report the tag its request carried, got {reported:?}",
            case = row.case,
        );
    }
}

#[tokio::test]
async fn tag_resource_stores_the_tag_value_member_it_was_given() {
    let router = emulator();

    for (index, row) in tag_entry_rows().iter().enumerate() {
        let arn = create_view(&router, row.case, &format!("tag-value-{index}"), &[]).await;
        tag_resource(&router, row.case, &arn, &[row.entry()]).await;
        let reported = reported_tags(&router, &arn, row.case).await;
        assert_eq!(
            reported,
            BTreeMap::from([(row.key.clone(), row.value.clone())]),
            "case {case:?}: TagResource must store the value member its request carried, got {reported:?}",
            case = row.case,
        );
    }
}

#[tokio::test]
async fn one_request_reports_each_of_its_entries_as_it_was_given() {
    let router = emulator();
    let rows = tag_entry_rows();
    let case = "every entry form in one request";

    // One list carrying every form at once: a writer that collapses "no value"
    // into the empty value cannot tell these entries apart afterwards.
    let entries: Vec<Value> = rows.iter().map(TagEntryRow::entry).collect();
    let expected: BTreeMap<String, Option<String>> = rows
        .iter()
        .map(|row| (row.key.clone(), row.value.clone()))
        .collect();

    let created = create_view(&router, case, "every-entry-form-at-create", &entries).await;
    let reported = reported_tags(&router, &created, case).await;
    assert_eq!(
        reported, expected,
        "case {case:?}: the create must report every entry as it was given, got {reported:?}"
    );

    let attached = create_view(&router, case, "every-entry-form-attached", &[]).await;
    tag_resource(&router, case, &attached, &entries).await;
    let reported = reported_tags(&router, &attached, case).await;
    assert_eq!(
        reported, expected,
        "case {case:?}: TagResource must report every entry as it was given, got {reported:?}"
    );
}

#[tokio::test]
async fn a_tag_stored_without_a_value_reports_no_value_member_on_the_wire() {
    let router = emulator();
    let case = "the reported entry on the wire";
    // The two forms side by side in one response, read from the raw body rather
    // than through the helper above: the omitted value must be an ABSENT member,
    // and the empty value a member carrying "".
    let arn = create_view(
        &router,
        case,
        "value-member-on-the-wire",
        &[json!({ "key": "bare" }), json!({ "key": "empty", "value": "" })],
    )
    .await;

    let (status, body) = call(&router, "ListTagsForResource", &json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {status} {body:?}"
    );
    let entries = body["resourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no resourceTags in {body:?}"));
    let entry_for = |key: &str| -> Value {
        entries
            .iter()
            .find(|entry| entry["key"] == json!(key))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no entry for {key:?} in {entries:?}"))
    };
    assert_eq!(
        entry_for("bare"),
        json!({ "key": "bare" }),
        "case {case:?}: the tag written with no value must report none, got {:?}",
        entry_for("bare")
    );
    assert_eq!(
        entry_for("empty"),
        json!({ "key": "empty", "value": "" }),
        "case {case:?}: the tag written with the empty value must report it, got {:?}",
        entry_for("empty")
    );
}

#[tokio::test]
async fn re_tagging_a_key_replaces_whether_it_carries_a_value() {
    let router = emulator();
    let case = "re-tagging one key";
    // The key a step re-tags, and a key no step names, which every step must
    // leave exactly as the create stored it.
    let key = "team";
    let untouched = "untouched";

    let arn = create_view(
        &router,
        case,
        "re-tag-value-presence",
        &[json!({ "key": untouched, "value": "kept" })],
    )
    .await;

    // Each step attaches ONE entry for the same key and states what the resource
    // must report for it afterwards: the value a step omits replaces a stored
    // one with no value, and the empty value replaces it with a value that is
    // empty — two outcomes a store holding only strings cannot tell apart.
    let steps: Vec<(&str, Option<String>)> = vec![
        ("a value where none was stored", Some("finance".to_string())),
        ("no value where one was stored", None),
        ("the empty value where none was stored", Some(String::new())),
        ("no value where the empty value was stored", None),
        ("a value where none was stored again", Some("ops".to_string())),
        (
            "a value at the modelled maximum over a shorter one",
            Some("v".repeat(MAX_TAG_VALUE_LEN)),
        ),
    ];

    for (step, value) in &steps {
        let entry = match value {
            Some(value) => json!({ "key": key, "value": value }),
            None => json!({ "key": key }),
        };
        tag_resource(&router, step, &arn, &[entry]).await;
        let reported = reported_tags(&router, &arn, step).await;
        assert_eq!(
            reported,
            BTreeMap::from([
                (key.to_string(), value.clone()),
                (untouched.to_string(), Some("kept".to_string())),
            ]),
            "case {step:?}: the re-tagged key must report the value member this request carried \
             and leave the other key alone, got {reported:?}"
        );
    }
}
