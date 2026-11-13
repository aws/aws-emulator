// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UntagResource`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. `UntagResource` deletes from the resource its ARN names exactly the tags whose keys
//!    the request lists, and leaves every other tag as it stands — "Existing tags of the
//!    resource whose keys are members of this list are removed from the resource"
//!    (`UntagResourceInput$TagKeys`). The rows differ along what the rule generalises
//!    over: the resource TYPE the ARN names (every taggable type of this service, from
//!    `super::resource_tags`), and the key list itself — one key, several, a key carrying
//!    the punctuation and the non-ASCII letters `TagKey`'s pattern allows, a key the
//!    resource does not carry, a key named twice, and every key the resource has.
//! 2. The delete is a WRITE and nothing else: what it leaves is what the next
//!    `ListTagsForResource` and the resource's own read report, and repeating the same
//!    request leaves the same tags (what it asks for is the absence of those keys).
//! 3. An ARN that names no resource this store holds is the shared tagging frontend's
//!    `404 NotFoundException`, stating that frontend's fixed sentence and nothing the
//!    request sent, and writes nothing — one row per taggable type, plus a string that is
//!    not an ARN at all.
//! 4. `TagKeyList`'s `@length(1, 50)` holds on both sides: the 50 keys it tops out at are
//!    all deleted, and a request naming 51 is refused 400 and deletes nothing.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The resources under test are created, and their tags read back, through
//! `super::resource_tags`' shared table and wire helpers, so the two tag suites cannot
//! disagree about how a taggable resource is made or read.

use std::collections::BTreeMap;

use serde_json::Value;

use super::resource_tags::{
    create_tagged, list_tags, reported_tags, tags_of, Kind, TAGGING_NOT_FOUND_CODE,
    TAGGING_NOT_FOUND_MESSAGE, KINDS,
};
use super::workload_harness::{encode_path_segment, minimal_create, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// `DELETE /tags/{WorkloadArn}?tagKeys=…`, the wire form of `UntagResource`: the ARN is a
/// non-greedy `@httpLabel` and the key list is bound to `@httpQuery("tagKeys")`, so "to
/// specify multiple tags, use separate tagKeys parameters" (the operation's doc) — one
/// parameter per key, each percent-encoded, exactly as an SDK sends it.
///
/// `pub(crate)` because `super::tag_family_envelopes` is about the envelopes of the same
/// requests, and the two suites must not disagree about how one is spelled.
pub(crate) async fn untag(client: &Client, resource_arn: &str, tag_keys: &[&str]) -> Answer {
    let path = format!("/tags/{}", encode_path_segment(resource_arn));
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_path_segment(key)))
        .collect::<Vec<_>>()
        .join("&");
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    client.send("DELETE", &uri, None).await
}

/// The tags every resource in the table below is created with. They span the forms
/// `TagKey`'s `^[\p{L}\p{N}\p{Z}_.:/=+@-]+$` allows — ASCII letters, the punctuation, a
/// separator, and the non-ASCII letters `\p{L}` covers — so the key list under test can
/// name any of them. Every value is non-empty, because an agent profile's tags travel as a
/// `Tag` list whose `value` is `@length(1, 256)`
/// (`smithy_query(query="shape", shape="Tag")`).
fn stored_tags() -> BTreeMap<String, String> {
    tags_of(&[
        ("Environment", "Production"),
        ("Owner", "platform"),
        ("cost.center:team/1=2+3@4-5_6", "owned by platform"),
        ("Zone One", "us west"),
        ("Ωμέγα", "Δοκιμή"),
    ])
}

/// One row of the key-list table: what it is called and the keys the request names.
struct KeyCase {
    case: &'static str,
    keys: Vec<&'static str>,
}

/// The key lists every taggable resource type is tested with. They differ along the COUNT
/// of keys, the FORM of each key, whether the resource carries it, and whether one is named
/// twice — not along one recorded example.
fn key_cases() -> Vec<KeyCase> {
    vec![
        KeyCase {
            case: "one key",
            keys: vec!["Owner"],
        },
        KeyCase {
            case: "several keys",
            keys: vec!["Environment", "Zone One"],
        },
        KeyCase {
            case: "a key carrying punctuation",
            keys: vec!["cost.center:team/1=2+3@4-5_6"],
        },
        KeyCase {
            case: "a non-ASCII key",
            keys: vec!["Ωμέγα"],
        },
        KeyCase {
            // Nothing to remove: the request asks for the ABSENCE of this key, which
            // already holds.
            case: "a key the resource does not carry",
            keys: vec!["NeverSet"],
        },
        KeyCase {
            // The same key twice is the same demand twice.
            case: "one key named twice",
            keys: vec!["Owner", "Owner"],
        },
        KeyCase {
            case: "a present key beside an absent one",
            keys: vec!["Environment", "NeverSet"],
        },
        KeyCase {
            case: "every key the resource carries",
            keys: vec![
                "Environment",
                "Owner",
                "cost.center:team/1=2+3@4-5_6",
                "Zone One",
                "Ωμέγα",
            ],
        },
    ]
}

/// The tags left on a resource that carried `tags` after the keys in `keys` were deleted.
fn tags_without(tags: &BTreeMap<String, String>, keys: &[&str]) -> BTreeMap<String, String> {
    tags.iter()
        .filter(|(key, _)| !keys.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn assert_untagged(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 204,
        "case {case:?}: expected UntagResource 204, got {answer:?}"
    );
}

/// Rule 1: every taggable resource type loses exactly the keys the request names, for
/// every shape of key list.
#[tokio::test]
async fn untagging_deletes_exactly_the_keys_named() {
    let client = Client::new();
    let tags = stored_tags();

    for (row, key_case) in key_cases().into_iter().enumerate() {
        for (column, kind) in KINDS.into_iter().enumerate() {
            let case = format!("{} / {kind:?}", key_case.case);
            let slug = format!("u{row}{column}");
            let arn = create_tagged(&client, kind, &case, &slug, &tags).await;

            let answer = untag(&client, &arn, &key_case.keys).await;
            assert_untagged(&answer, &case);

            let expected = tags_without(&tags, &key_case.keys);
            let listed = list_tags(&client, &arn).await;
            assert_eq!(
                reported_tags(&listed, &case),
                expected,
                "case {case:?}: the resource must keep exactly the tags whose keys were \
                 not named, got {listed:?}"
            );
        }
    }
}

/// Rule 2: the delete is a write and nothing else. The tags it leaves are what the next
/// list reports, what the resource's OWN read reports, and what a repeat of the same
/// request leaves — and a second list in a row agrees with the first.
#[tokio::test]
async fn what_the_delete_leaves_is_what_every_later_read_reports() {
    let client = Client::new();
    let tags = stored_tags();

    let mut body = minimal_create("untag-roundtrip", "tok-untag-roundtrip");
    body["Tags"] = Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    );
    let created = client.create(body).await;
    assert_eq!(
        created.status, 200,
        "case \"round trip\": expected CreateWorkload 200, got {created:?}"
    );
    let workload_arn = created
        .body
        .get("WorkloadArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let workload_id = created
        .body
        .get("WorkloadId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let keys = ["Owner", "Zone One"];
    let expected = tags_without(&tags, &keys);

    let first = untag(&client, &workload_arn, &keys).await;
    assert_untagged(&first, "first delete");
    assert_eq!(
        reported_tags(&list_tags(&client, &workload_arn).await, "first delete"),
        expected,
        "case \"first delete\": the list must report the tags the workload now carries"
    );

    // Repeating the request asks for the same absence, so it leaves the same tags.
    let repeat = untag(&client, &workload_arn, &keys).await;
    assert_untagged(&repeat, "repeated delete");
    assert_eq!(
        reported_tags(&list_tags(&client, &workload_arn).await, "repeated delete"),
        expected,
        "case \"repeated delete\": repeating the delete must leave the same tags"
    );

    // The resource's own read agrees: the tags live on the workload, not beside it.
    let fetched = client.get(&workload_id).await;
    let reported: BTreeMap<String, String> = fetched
        .body
        .get("Workload")
        .and_then(|workload| workload.get("Tags"))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported, expected,
        "case \"read back\": GetWorkload must report the tags the delete left, got {fetched:?}"
    );
}

/// Rule 3: an ARN naming no stored resource is a 404 carrying the ARN as sent, and the
/// resources that DO exist keep every tag — the refusal wrote nothing.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_a_not_found() {
    let client = Client::new();
    let account = engine::emulator_account_id();
    let tags = stored_tags();

    let mut present: Vec<(Kind, String)> = Vec::new();
    for (column, kind) in KINDS.into_iter().enumerate() {
        let case = format!("present / {kind:?}");
        let arn = create_tagged(&client, kind, &case, &format!("m{column}"), &tags).await;
        present.push((kind, arn));
    }

    let absent_id = "0123456789abcdef0123456789abcdef";
    let rows: Vec<(&str, String)> = vec![
        (
            "absent workload",
            format!("arn:aws:wellarchitected:us-west-2:{account}:workload/{absent_id}"),
        ),
        (
            "absent custom lens",
            format!("arn:aws:wellarchitected:us-west-2:{account}:lens/{absent_id}"),
        ),
        (
            "absent profile",
            format!("arn:aws:wellarchitected:us-west-2:{account}:profile/{absent_id}"),
        ),
        (
            "absent review template",
            format!("arn:aws:wellarchitected:us-west-2:{account}:review-template/{absent_id}"),
        ),
        (
            "absent agent profile",
            format!("arn:aws:wellarchitected:us-west-2:{account}:agent-profile/{absent_id}"),
        ),
        (
            // However much of the ARN agrees, another account's resource is a different
            // resource.
            "another account's workload",
            format!("arn:aws:wellarchitected:us-west-2:210987654321:workload/{absent_id}"),
        ),
        ("not an ARN at all", "not-an-arn".to_string()),
    ];

    for (case, resource_arn) in rows {
        let answer = untag(&client, &resource_arn, &["Owner"]).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, TAGGING_NOT_FOUND_CODE,
            "case {case:?}: expected {TAGGING_NOT_FOUND_CODE}, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Message").and_then(Value::as_str),
            Some(TAGGING_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the frontend's fixed sentence, got {answer:?}"
        );
        assert!(
            !answer.body.to_string().contains(resource_arn.as_str()),
            "case {case:?}: the refusal must disclose nothing the request sent, got {answer:?}"
        );
    }

    for (kind, arn) in present {
        let case = format!("present / {kind:?}");
        let listed = list_tags(&client, &arn).await;
        assert_eq!(
            reported_tags(&listed, &case),
            tags,
            "case {case:?}: a refused delete must leave every tag in place, got {listed:?}"
        );
    }
}

/// Rule 4: `TagKeyList`'s `@length(1, 50)` on both sides — the 50 keys it tops out at are
/// all deleted, and a request naming 51 is refused 400 and deletes nothing.
#[tokio::test]
async fn a_key_list_outside_its_published_bound_is_refused() {
    let client = Client::new();

    // A resource carrying the most tags one can hold, so the whole key list is spendable.
    let at_bound: BTreeMap<String, String> = (0..state::MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let keys: Vec<String> = at_bound.keys().cloned().collect();
    let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();

    let refused_arn = create_tagged(
        &client,
        Kind::Workload,
        "past the bound",
        "bound-past",
        &at_bound,
    )
    .await;
    let mut past_bound: Vec<&str> = key_refs.clone();
    past_bound.push("one-too-many");
    let refused = untag(&client, &refused_arn, &past_bound).await;
    assert_eq!(
        refused.status,
        400,
        "case \"past the bound\": a list of {} keys must be refused 400, got {refused:?}",
        past_bound.len()
    );
    assert_eq!(
        reported_tags(&list_tags(&client, &refused_arn).await, "past the bound").len(),
        state::MAX_TAGS_PER_RESOURCE,
        "case \"past the bound\": the refused delete must remove nothing"
    );

    // No key at all is not a request this operation can even be routed from: `TagKeys` is
    // `@required` and bound to `@httpQuery`, so a request carrying no `tagKeys` parameter
    // matches no route — the framework's required-binding routing, not this handler.

    let arn = create_tagged(
        &client,
        Kind::Workload,
        "at the bound",
        "bound-at",
        &at_bound,
    )
    .await;
    let answer = untag(&client, &arn, &key_refs).await;
    assert_untagged(&answer, "at the bound");
    assert!(
        reported_tags(&list_tags(&client, &arn).await, "at the bound").is_empty(),
        "case \"at the bound\": deleting all {} keys must leave no tag",
        state::MAX_TAGS_PER_RESOURCE
    );
}
