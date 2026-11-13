// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ENVELOPE of this service's tag family — the status a tag
//! request is answered with, and the text its missing-resource refusal carries.
//!
//! The two rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. A tag operation that returns no modelled member answers `204 No Content` with
//!    nothing for a client to parse, while the one that returns a member answers `200`
//!    with it: `TagResourceOutput` and `UntagResourceOutput` carry no member and
//!    `ListTagsForResourceOutput` carries `Tags`
//!    (`smithy_query(query="operation", operation="TagResource,UntagResource,ListTagsForResource")`).
//!    The rows differ along what the rule generalises over: WHICH of the two writes is
//!    made, the KIND of resource its ARN names (every taggable type of this service), and
//!    the shape of the request — how many pairs or keys it carries, whether they are
//!    already recorded on the resource, and whether it changes the stored set at all.
//! 2. The missing-resource refusal of EVERY operation of the family is the shared tagging
//!    frontend's `404 NotFoundException` — not the `ResourceNotFoundException` this
//!    service's model declares for these three operations and the rest of the service
//!    answers — whose `Message` is the fixed text of that failure class, with no variable
//!    part: no ARN, no account id and no bracketed class tag, and no member of the body
//!    echoing the request at all. The rows differ along the FORM of the unresolvable ARN:
//!    one per taggable kind, a stored resource's ARN under another account and in another
//!    Region, an Amazon Web Services official lens ARN, an ARN of another service, and a
//!    value that is not an ARN at all.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. Resources are created and read back through `super::resource_tags`' shared
//! table and wire helpers, and the two writes through `super::resource_tagging::tag` and
//! `super::untag_resource::untag`, so no request in this crate's tests is spelled twice.

use std::collections::BTreeMap;

use serde_json::Value;

use super::resource_tagging::tag;
use super::resource_tags::{
    create_tagged, list_tags, reported_tags, tags_of, Kind, TAGGING_NOT_FOUND_CODE,
    TAGGING_NOT_FOUND_MESSAGE, KINDS,
};
use super::untag_resource::untag;
use super::workload_harness::Client;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The whole `Message` the tag family's missing-resource refusal carries, under the alias
/// the shared tag helpers hold it under ([`TAGGING_NOT_FOUND_MESSAGE`]).
const NOT_FOUND_MESSAGE: &str = TAGGING_NOT_FOUND_MESSAGE;

/// The tags every resource in the tables below is created with: enough distinct keys for a
/// row to name one, several or all of them.
fn stored_tags() -> BTreeMap<String, String> {
    tags_of(&[
        ("Environment", "Production"),
        ("Owner", "platform"),
        ("Tier", "gold"),
    ])
}

/// Assert `answer` is a `204 No Content`: the status the family's writes report, and a body
/// with nothing in it for a client to parse.
fn assert_no_content(answer: &super::workload_harness::Answer, case: &str) {
    assert_eq!(
        answer.status, 204,
        "case {case:?}: expected 204 No Content, got {answer:?}"
    );
    assert!(
        answer
            .body
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "case {case:?}: a no-content answer carries no member, got {:?}",
        answer.body
    );
}

/// Rule 1, the WRITE half: every shape of tag write, on every taggable resource kind, is
/// answered 204 with no content — and the read of the resource afterwards is a 200 that
/// still carries its `Tags` map, so the two statuses follow the modelled output and not the
/// path they share.
#[tokio::test]
async fn every_tag_write_is_answered_no_content_and_the_read_is_not() {
    /// One write under test: what it is called, and the request it makes.
    enum Write {
        /// `TagResource` with these pairs.
        Tag(Vec<(&'static str, &'static str)>),
        /// `UntagResource` with these keys.
        Untag(Vec<&'static str>),
    }

    let most_tags: Vec<(String, String)> = (0..state::MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();

    let rows: Vec<(&str, Write)> = vec![
        ("tag one new pair", Write::Tag(vec![("Fresh", "yes")])),
        (
            "tag several new pairs",
            Write::Tag(vec![("Fresh", "yes"), ("Second", "also")]),
        ),
        (
            // A key the resource already carries: the write changes a value rather than
            // adding one, and is reported the same way.
            "tag a key the resource already carries",
            Write::Tag(vec![("Owner", "another-team")]),
        ),
        (
            // Nothing changes: the pair is already recorded exactly as sent.
            "tag a pair already recorded",
            Write::Tag(vec![("Tier", "gold")]),
        ),
        ("untag one key", Write::Untag(vec!["Owner"])),
        (
            "untag several keys",
            Write::Untag(vec!["Owner", "Environment"]),
        ),
        (
            // Nothing to remove: the request asks for an absence that already holds.
            "untag a key the resource does not carry",
            Write::Untag(vec!["NeverSet"]),
        ),
        (
            "untag every key the resource carries",
            Write::Untag(vec!["Environment", "Owner", "Tier"]),
        ),
    ];

    let client = Client::new();
    let stored = stored_tags();

    for (row, (shape, write)) in rows.iter().enumerate() {
        for (column, kind) in KINDS.into_iter().enumerate() {
            let case = format!("{shape} / {kind:?}");
            let arn = create_tagged(&client, kind, &case, &format!("e{row}{column}"), &stored).await;

            match write {
                Write::Tag(pairs) => {
                    let requested = tags_of(pairs);
                    assert_no_content(&tag(&client, &arn, &requested).await, &case);
                }
                Write::Untag(keys) => {
                    assert_no_content(&untag(&client, &arn, keys).await, &case);
                }
            }

            // The READ of the same path answers 200 with the member its output models, so
            // the 204 above is the empty output shape's status and not the path's.
            let listed = list_tags(&client, &arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected ListTagsForResource 200, got {listed:?}"
            );
            assert!(
                listed.body.get("Tags").is_some(),
                "case {case:?}: the read must carry its Tags member, got {listed:?}"
            );
        }
    }

    // The most pairs a `TagMap` carries, written in one request: the biggest body the
    // write admits is still answered with no content.
    let arn = create_tagged(
        &client,
        Kind::Workload,
        "the most pairs a TagMap carries",
        "e-max",
        &BTreeMap::new(),
    )
    .await;
    let requested: BTreeMap<String, String> = most_tags.iter().cloned().collect();
    assert_no_content(
        &tag(&client, &arn, &requested).await,
        "the most pairs a TagMap carries",
    );
    assert_eq!(
        reported_tags(
            &list_tags(&client, &arn).await,
            "the most pairs a TagMap carries"
        ),
        requested,
        "case \"the most pairs a TagMap carries\": the write must be recorded whole"
    );

    // The same keys deleted in one request: the biggest key list the delete admits is
    // answered with no content too.
    let keys: Vec<String> = most_tags.iter().map(|(key, _)| key.clone()).collect();
    let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    assert_no_content(
        &untag(&client, &arn, &key_refs).await,
        "the most keys a TagKeyList carries",
    );
    assert!(
        reported_tags(
            &list_tags(&client, &arn).await,
            "the most keys a TagKeyList carries"
        )
        .is_empty(),
        "case \"the most keys a TagKeyList carries\": every named key must be gone"
    );
}

/// Rule 2: the family's missing-resource refusal is the tagging frontend's own error shape
/// and carries the FIXED text of its failure class — the same code and the same `Message`
/// from every operation of the family and for every form of unresolvable ARN, with nothing
/// of the request anywhere in the body.
#[tokio::test]
async fn the_tag_family_words_a_missing_resource_with_its_fixed_text() {
    let client = Client::new();
    let account = engine::emulator_account_id();
    let stored = stored_tags();

    // One resource of every kind exists, so every row below is a real miss rather than an
    // empty store.
    let mut present: Vec<(Kind, String)> = Vec::new();
    for (column, kind) in KINDS.into_iter().enumerate() {
        let case = format!("present / {kind:?}");
        let arn = create_tagged(&client, kind, &case, &format!("f{column}"), &stored).await;
        present.push((kind, arn));
    }
    let workload_arn = present
        .iter()
        .find(|(kind, _)| matches!(kind, Kind::Workload))
        .map(|(_, arn)| arn.clone())
        .expect("the suite created a workload");

    let absent_id = "fedcba9876543210fedcba9876543210";
    let rows: Vec<(&str, String)> = vec![
        (
            "a workload ARN no workload carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:workload/{absent_id}"),
        ),
        (
            "a custom lens ARN no lens carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:lens/{absent_id}"),
        ),
        (
            "a profile ARN no profile carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:profile/{absent_id}"),
        ),
        (
            "a review template ARN no template carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:review-template/{absent_id}"),
        ),
        (
            "an agent profile ARN no profile carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:agent-profile/{absent_id}"),
        ),
        (
            // However much of the ARN agrees, another account's resource is a different
            // resource — and the answer still says nothing about which account was read.
            "a stored workload's ARN under another account",
            workload_arn.replace(account, "210987654321"),
        ),
        (
            "a stored workload's ARN in another Region",
            workload_arn.replace("us-west-2", "eu-west-1"),
        ),
        (
            "an Amazon Web Services official lens ARN, which names no account's resource",
            "arn:aws:wellarchitected:us-west-2::lens/serverless".to_string(),
        ),
        (
            "an ARN of another service entirely",
            format!("arn:aws:s3:::a-bucket/{absent_id}"),
        ),
        ("a value that is not an ARN at all", "not-an-arn".to_string()),
    ];

    for (shape, arn) in rows {
        assert_ne!(
            arn, workload_arn,
            "case {shape:?}: the row must not name the resource that exists"
        );
        for (operation, answer) in [
            (
                "TagResource",
                tag(&client, &arn, &tags_of(&[("Fresh", "yes")])).await,
            ),
            ("UntagResource", untag(&client, &arn, &["Owner"]).await),
            ("ListTagsForResource", list_tags(&client, &arn).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {shape:?}/{operation}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, TAGGING_NOT_FOUND_CODE,
                "case {shape:?}/{operation}: expected {TAGGING_NOT_FOUND_CODE}, got {answer:?}"
            );
            let message = answer
                .body
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                message, NOT_FOUND_MESSAGE,
                "case {shape:?}/{operation}: the message is the family's fixed text, got \
                 {answer:?}"
            );
            // The sentence has no variable part at all: neither the ARN the request named,
            // nor the account the lookup ran in, nor a class tag ahead of the text.
            for (what, fragment) in [("the ARN", arn.as_str()), ("the account", account)] {
                assert!(
                    !message.contains(fragment),
                    "case {shape:?}/{operation}: the message must not disclose {what}, got \
                     {message:?}"
                );
            }
            assert!(
                !message.contains('['),
                "case {shape:?}/{operation}: the message carries no bracketed class tag, got \
                 {message:?}"
            );
            // Nor does any other member of the body: the frontend's refusal names the
            // failure class and nothing the caller sent.
            let body = answer.body.to_string();
            for (what, fragment) in [("the ARN", arn.as_str()), ("the account", account)] {
                assert!(
                    !body.contains(fragment),
                    "case {shape:?}/{operation}: the body must not disclose {what}, got \
                     {answer:?}"
                );
            }
        }
    }

    // Every refusal above wrote nothing: each resource that DOES exist still carries the
    // tags it was created with.
    for (kind, arn) in present {
        let case = format!("present / {kind:?}");
        let listed = list_tags(&client, &arn).await;
        assert_eq!(
            reported_tags(&listed, &case),
            stored,
            "case {case:?}: a refused tag request must change nothing, got {listed:?}"
        );
    }
}
