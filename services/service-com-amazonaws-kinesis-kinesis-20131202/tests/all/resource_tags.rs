// SPDX-License-Identifier: CC0-1.0

//! A Kinesis resource's tags addressed BY ARN, end to end over the wire: `TagResource` ->
//! `ListTagsForStream`.
//!
//! RULE: `TagResource` ASSIGNS every key-value pair its request carries to the stored
//! resource its `ResourceARN` names — a key the resource already carries takes the new
//! value, every tag the request does not name stays as it was — and a later read reports
//! that STORED set. It is the SAME stored tag set `AddTagsToStream` writes and
//! `ListTagsForStream` reports, reached through the resource's ARN rather than through a
//! stream name, so the two operations are two ways into one set: whatever either assigns is
//! what a read reports, and re-assigning what is already there changes nothing.
//!
//! The refusals are the ones that rule implies and the operation models: an ARN this
//! account and region hold no resource under is `ResourceNotFoundException` (including an
//! ARN of a resource KIND this emulator holds none of, and the ARN of a stream that lives
//! in another region), and an assignment that would leave the resource carrying more tags
//! than one resource may carry is `LimitExceededException` — refused before the stored map
//! grows, so it changes nothing. A resource's tag set exists for as long as the resource
//! does, so an assignment is served in every lifecycle status it has.
//!
//! The rows differ along what the rule generalises over: how many pairs an assignment
//! carries (one, several, the whole ceiling), whether its keys are new or already present,
//! whether the tags were put there by this operation or by `AddTagsToStream`, the form of a
//! key and a value, and the order the keys arrive in.
//!
//! The wire harness, the ARN builder and the tag helpers are [`crate::stream_tags`]',
//! reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    resource_not_found_message, tag_ceiling_exceeded_message, MAX_STREAM_TAGS, STREAM_ACTIVE,
    STREAM_CREATING, STREAM_DELETING,
};

use crate::stream_tags::{
    error_code, error_message, in_key_order, pair, pairs, past_activation, poll_until_active,
    reported_tags, stream_arn, tag_map, Emulator, REGION,
};

/// One row of the assignment table: what the stream was created with, what successive
/// calls assign to it (each through the operation named), and the tag set it must then
/// report.
struct Row {
    label: &'static str,
    /// The `Tags` the create carries, empty on a row whose create names none.
    created_with: Vec<(String, String)>,
    /// One request each, served in order, as `(operation, the pairs it assigns)`.
    assignments: Vec<(&'static str, Vec<(String, String)>)>,
    /// The tags the stream must report, given UNSORTED so the key order a read promises is
    /// what the assertion checks.
    expected: Vec<(String, String)>,
}

/// The table the assignment rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape: one pair onto an untagged stream, by ARN.
        Row {
            label: "one-pair-onto-an-untagged-stream",
            created_with: Vec::new(),
            assignments: vec![("TagResource", vec![pair("Environment", "production")])],
            expected: vec![pair("Environment", "production")],
        },
        // Several pairs at once, arriving in an order that is not key order.
        Row {
            label: "three-pairs-at-once-out-of-key-order",
            created_with: Vec::new(),
            assignments: vec![(
                "TagResource",
                vec![
                    pair("zeta", "last-by-key"),
                    pair("alpha", "first-by-key"),
                    pair("mu", "middle-by-key"),
                ],
            )],
            expected: vec![
                pair("alpha", "first-by-key"),
                pair("mu", "middle-by-key"),
                pair("zeta", "last-by-key"),
            ],
        },
        // A key the create already put there takes the new value; the other stays.
        Row {
            label: "overwrites-a-key-the-create-carried",
            created_with: vec![pair("Environment", "staging"), pair("Team", "platform")],
            assignments: vec![("TagResource", vec![pair("Environment", "production")])],
            expected: vec![pair("Environment", "production"), pair("Team", "platform")],
        },
        // ONE stored set, two operations into it: what `AddTagsToStream` wrote is still
        // there, and what this operation overwrites takes effect.
        Row {
            label: "shares-one-stored-set-with-add-tags-to-stream",
            created_with: Vec::new(),
            assignments: vec![
                (
                    "AddTagsToStream",
                    vec![pair("by-add-tags", "kept"), pair("shared", "written-first")],
                ),
                (
                    "TagResource",
                    vec![pair("by-tag-resource", "added"), pair("shared", "overwritten")],
                ),
            ],
            expected: vec![
                pair("by-add-tags", "kept"),
                pair("by-tag-resource", "added"),
                pair("shared", "overwritten"),
            ],
        },
        // Re-assigning exactly what is already there changes nothing.
        Row {
            label: "re-assigning-the-same-pairs-changes-nothing",
            created_with: vec![pair("Environment", "production")],
            assignments: vec![
                ("TagResource", vec![pair("Environment", "production")]),
                ("TagResource", vec![pair("Environment", "production")]),
            ],
            expected: vec![pair("Environment", "production")],
        },
        // The whole documented ceiling in one assignment.
        Row {
            label: "the-whole-tag-ceiling-at-once",
            created_with: Vec::new(),
            assignments: vec![("TagResource", pairs(MAX_STREAM_TAGS))],
            expected: pairs(MAX_STREAM_TAGS),
        },
    ]
}

/// Every row's assignments are served through the operation it names, and the stream then
/// reports the union of what was there and what was assigned, in key order.
#[tokio::test]
async fn tags_assigned_by_arn_are_reported_back_in_key_order() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every stream is created first, so the rows share ONE wait for the transition the
    // creates armed rather than one each.
    for row in &rows {
        let mut body = json!({ "StreamName": row.label, "ShardCount": 1 });
        if !row.created_with.is_empty() {
            body["Tags"] = tag_map(&row.created_with);
        }
        emulator.ok(row.label, "CreateStream", &body).await;
    }
    tokio::time::sleep(past_activation()).await;

    for row in &rows {
        let arn = stream_arn(REGION, row.label);
        for (operation, assignment) in &row.assignments {
            let body = match *operation {
                "TagResource" => json!({ "ResourceARN": arn, "Tags": tag_map(assignment) }),
                _ => json!({ "StreamARN": arn, "Tags": tag_map(assignment) }),
            };
            let answered = emulator.ok(row.label, operation, &body).await;
            assert!(
                answered.as_object().is_some_and(|body| body.is_empty())
                    || answered.is_null(),
                "case {}: expected {operation} to answer the modelled empty body, got \
                 {answered}",
                row.label
            );
        }

        let read = emulator
            .ok(
                row.label,
                "ListTagsForStream",
                &json!({ "StreamARN": arn }),
            )
            .await;
        assert_eq!(
            reported_tags(row.label, &read),
            in_key_order(&row.expected),
            "case {}: expected the stored tag set in key order, got {read}",
            row.label
        );
    }
}

/// An ARN this account and region hold no resource under names nothing to tag, whatever
/// form it takes: another region's stream, a stream nothing created, and a resource KIND
/// this emulator holds none of.
#[tokio::test]
async fn an_arn_naming_no_resource_of_this_account_and_region_is_refused() {
    let emulator = Emulator::new();
    let name = "tag-resource-refusals";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, name, name).await;

    let rows: Vec<(&str, String)> = vec![
        ("a stream nothing created", stream_arn(REGION, "never-created")),
        (
            "a stream of another region",
            stream_arn("eu-west-1", name),
        ),
        (
            "a resource kind this emulator holds none of",
            format!(
                "arn:aws:kinesis:{REGION}:{}:channel/a-channel",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];
    for (case, arn) in &rows {
        let (status, response) = emulator
            .call(
                "TagResource",
                &json!({ "ResourceARN": arn, "Tags": tag_map(&[pair("k", "v")]) }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            // The sentence is the one this service states for the KIND the ARN names: a
            // `channel/` ARN names a channel, which is a tagging target of its own, so it is
            // not reported as a missing stream.
            resource_not_found_message(arn),
            "case {case}: expected the not-found sentence this service states, got {response}"
        );
    }

    // The stream this account does hold is untouched by the refusals.
    let read = emulator
        .ok(
            "after the refusals",
            "ListTagsForStream",
            &json!({ "StreamName": name }),
        )
        .await;
    assert!(
        reported_tags("after the refusals", &read).is_empty(),
        "case after the refusals: a refused assignment stores nothing, got {read}"
    );
}

/// A resource's tag set exists for as long as the resource does, so an assignment by ARN
/// is served whatever lifecycle status the resource is in — the same rule
/// `AddTagsToStream` applies to the one stored tag set both write, and the rule the
/// `UntagResource` that unassigns from it already follows.
///
/// The rows span the statuses a stream reaches here: still inside the transition its
/// create armed (`CREATING`), past it (`ACTIVE`), and with a delete accepted for it
/// (`DELETING`).
#[tokio::test]
async fn an_assignment_is_served_in_every_lifecycle_status() {
    // (label, wait past the activation, delete first, the status the assignment lands in)
    let cases = [
        ("tag-resource-creating", false, false, STREAM_CREATING),
        ("tag-resource-active", true, false, STREAM_ACTIVE),
        ("tag-resource-deleting", true, true, STREAM_DELETING),
    ];

    for (name, wait_for_activation, delete_first, expected_status) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
        if wait_for_activation {
            tokio::time::sleep(past_activation()).await;
        }
        if delete_first {
            emulator
                .ok(name, "DeleteStream", &json!({ "StreamName": name }))
                .await;
        }
        let arn = stream_arn(REGION, name);

        let summary = emulator
            .ok(
                name,
                "DescribeStreamSummary",
                &json!({ "StreamName": name }),
            )
            .await;
        let reported_status = summary
            .get("StreamDescriptionSummary")
            .and_then(|summary| summary.get("StreamStatus"))
            .and_then(Value::as_str)
            .unwrap_or("<no StreamStatus>");
        assert_eq!(
            reported_status, expected_status,
            "case {name}: the resource is in {expected_status} when the assignment lands, \
             got {summary}"
        );

        let (status, response) = emulator
            .call(
                "TagResource",
                &json!({ "ResourceARN": arn, "Tags": tag_map(&[pair("k", "v")]) }),
            )
            .await;
        assert_eq!(
            (status, &response),
            (200, &json!({})),
            "case {name}: an assignment to a resource in {expected_status} is served, got \
             {status} with body {response}"
        );

        let read = emulator
            .ok(name, "ListTagsForStream", &json!({ "StreamARN": arn }))
            .await;
        assert_eq!(
            reported_tags(name, &read),
            vec![pair("k", "v")],
            "case {name}: the assignment served in {expected_status} is stored, got {read}"
        );
    }
}

/// An assignment that would take the resource past the documented tag ceiling is refused
/// before the stored map grows, so the tags it already carries are untouched — while an
/// assignment that only overwrites keys it already carries stays inside the ceiling and is
/// served.
#[tokio::test]
async fn an_assignment_past_the_tag_ceiling_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let name = "tag-resource-ceiling";
    let stored = pairs(MAX_STREAM_TAGS);
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let arn = stream_arn(REGION, name);
    emulator
        .ok(
            name,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&stored) }),
        )
        .await;

    let (status, response) = emulator
        .call(
            "TagResource",
            &json!({
                "ResourceARN": arn,
                "Tags": tag_map(&[pair("one-too-many", "refused")]),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case past the ceiling: expected the modelled refusal, got {status} with body \
         {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case past the ceiling: expected LimitExceededException, got {response}"
    );
    assert_eq!(
        error_message(&response),
        tag_ceiling_exceeded_message(MAX_STREAM_TAGS + 1),
        "case past the ceiling: expected the one sentence this service states the ceiling \
         in, got {response}"
    );

    // Overwriting a key the resource already carries stays inside the ceiling.
    emulator
        .ok(
            "an overwrite at the ceiling",
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&[pair("key-00", "replaced")]) }),
        )
        .await;

    let read = emulator
        .ok(
            "ceiling-read-back",
            "ListTagsForStream",
            &json!({ "StreamARN": arn, "Limit": MAX_STREAM_TAGS }),
        )
        .await;
    let mut expected = in_key_order(&stored);
    expected[0] = pair("key-00", "replaced");
    assert_eq!(
        reported_tags("ceiling-read-back", &read),
        expected,
        "case ceiling-read-back: the refused assignment left the stored tags alone and the \
         overwrite took effect, got {read}"
    );
}
