// SPDX-License-Identifier: CC0-1.0

//! A Kinesis resource's tags addressed BY ARN, end to end over the wire: `TagResource` ->
//! `ListTagsForStream`, and the resource-tag family over a stream CONSUMER's ARN.
//!
//! RULE: `TagResource` ASSIGNS every key-value pair its request carries to the stored
//! resource its `ResourceARN` names — a key the resource already carries takes the new
//! value, every tag the request does not name stays as it was — and a later read reports
//! that STORED set. It is the SAME stored tag set `AddTagsToStream` writes and
//! `ListTagsForStream` reports, reached through the resource's ARN rather than through a
//! stream name, so the two operations are two ways into one set: whatever either assigns is
//! what a read reports, and re-assigning what is already there changes nothing.
//!
//! A `ResourceARN` names a resource of EVERY kind this service holds — a data stream, a
//! channel, a queue created on a stream, and a consumer registered with a stream, whose ARN
//! continues its stream's resource segment with `/consumer/<name>:<registration instant>` —
//! and each such resource
//! carries ONE stored tag set OF ITS OWN, so the whole family (`TagResource`,
//! `ListTagsForResource`, `UntagResource`) addresses a consumer exactly the way it addresses
//! a stream: the tags a registration carried and the tags a later assignment carried are one
//! set, that set is the consumer's and not its stream's, and it lives for as long as the
//! consumer does. A QUEUE is reached the same way, by the ARN its own create minted: "You can
//! also add, change, and remove tags after the queue is created with TagResource and
//! UntagResource" (`CreateQueueInput$Tags`).
//!
//! The refusals are the ones that rule implies and the operation models: an ARN this
//! account and region hold no resource under is `ResourceNotFoundException` (including an
//! ARN of a resource KIND this emulator holds none of, the ARN of a stream that lives in
//! another region, and a consumer ARN no registration minted), and an assignment that would
//! leave the resource carrying more tags than one resource may carry is
//! `LimitExceededException` — refused before the stored map grows, so it changes nothing. A
//! resource's tag set exists for as long as the resource does, so an assignment is served in
//! every lifecycle status it has.
//!
//! The rows differ along what the rule generalises over: the KIND of resource the ARN names,
//! how many pairs an assignment carries (one, several, the whole ceiling), whether its keys
//! are new or already present, whether the tags were put there by this operation, by
//! `AddTagsToStream` or by `RegisterStreamConsumer`, the form of a key and a value, and the
//! order the keys arrive in.
//!
//! The wire harness, the ARN builder and the tag helpers are [`crate::stream_tags`]',
//! reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    consumer_not_found_message, resource_not_found_message, tag_ceiling_exceeded_message,
    MAX_CONSUMER_TAGS, MAX_STREAM_TAGS, STREAM_ACTIVE, STREAM_CREATING, STREAM_DELETING,
    STREAM_MODE_ON_DEMAND,
};

use crate::stream_tags::{
    error_code, error_message, in_key_order, pair, pairs, past_activation, poll_until,
    poll_until_active, reported_tags, stream_arn, tag_map, Emulator, REGION,
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

// ---------------------------------------------------------------------------------------
// A stream CONSUMER is a resource of its own, addressed by the ARN its registration minted.
// ---------------------------------------------------------------------------------------

/// Register `consumer` with the stream `stream_arn` names, optionally carrying `tags`, and
/// answer the `ConsumerARN` the registration minted — the identifier every call below
/// addresses the consumer by, taken from the response rather than rebuilt here.
async fn register_consumer(
    emulator: &Emulator,
    case: &str,
    stream_arn: &str,
    consumer: &str,
    tags: &[(String, String)],
) -> String {
    let mut body = json!({ "StreamARN": stream_arn, "ConsumerName": consumer });
    if !tags.is_empty() {
        body["Tags"] = tag_map(tags);
    }
    let answered = emulator.ok(case, "RegisterStreamConsumer", &body).await;
    answered["Consumer"]["ConsumerARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case}: a registration must answer Consumer.ConsumerARN, got {answered}")
        })
        .to_string()
}

/// The tags the resource `arn` names reports, through the read that addresses a resource of
/// any kind by its ARN.
async fn tags_by_arn(emulator: &Emulator, case: &str, arn: &str) -> Vec<(String, String)> {
    let read = emulator
        .ok(case, "ListTagsForResource", &json!({ "ResourceARN": arn }))
        .await;
    reported_tags(case, &read)
}

/// One row of the consumer-assignment table: what the registration carried, what successive
/// `TagResource` calls assign, and the set the consumer must then report.
struct ConsumerRow {
    label: &'static str,
    /// The `Tags` the registration carries, empty on a row whose registration names none.
    registered_with: Vec<(String, String)>,
    /// One `TagResource` request each, served in order.
    assignments: Vec<Vec<(String, String)>>,
    /// The tags the consumer must report, given UNSORTED so the key order a read promises is
    /// what the assertion checks.
    expected: Vec<(String, String)>,
}

/// The table the assignment rule is checked over for a consumer.
fn consumer_rows() -> Vec<ConsumerRow> {
    vec![
        // The recorded shape: one pair onto a consumer the registration tagged with none,
        // assigned to the ARN the registration had just answered.
        ConsumerRow {
            label: "one-pair-onto-an-untagged-consumer",
            registered_with: Vec::new(),
            assignments: vec![vec![pair("sim2real", "stream-consumer-fanout")]],
            expected: vec![pair("sim2real", "stream-consumer-fanout")],
        },
        // Several pairs at once, arriving in an order that is not key order.
        ConsumerRow {
            label: "three-pairs-at-once-out-of-key-order",
            registered_with: Vec::new(),
            assignments: vec![vec![
                pair("zeta", "last-by-key"),
                pair("alpha", "first-by-key"),
                pair("mu", "middle-by-key"),
            ]],
            expected: vec![
                pair("alpha", "first-by-key"),
                pair("mu", "middle-by-key"),
                pair("zeta", "last-by-key"),
            ],
        },
        // ONE stored set, two ways into it: the registration seeded it, the assignment
        // overwrites one of its keys and adds another, and every other tag stays.
        ConsumerRow {
            label: "overwrites-a-key-the-registration-carried",
            registered_with: vec![pair("Environment", "staging"), pair("Team", "platform")],
            assignments: vec![vec![pair("Environment", "production"), pair("Owner", "efo")]],
            expected: vec![
                pair("Environment", "production"),
                pair("Owner", "efo"),
                pair("Team", "platform"),
            ],
        },
        // Re-assigning exactly what is already there changes nothing.
        ConsumerRow {
            label: "re-assigning-the-same-pairs-changes-nothing",
            registered_with: vec![pair("Environment", "production")],
            assignments: vec![
                vec![pair("Environment", "production")],
                vec![pair("Environment", "production")],
            ],
            expected: vec![pair("Environment", "production")],
        },
        // The documented ceiling, filled across two assignments.
        ConsumerRow {
            label: "the-whole-tag-ceiling-across-two-assignments",
            registered_with: Vec::new(),
            assignments: vec![
                pairs(MAX_CONSUMER_TAGS / 2),
                pairs(MAX_CONSUMER_TAGS).split_off(MAX_CONSUMER_TAGS / 2),
            ],
            expected: pairs(MAX_CONSUMER_TAGS),
        },
    ]
}

/// Every row's assignments are served against the ARN its registration minted, and the
/// consumer then reports the union of what the registration carried and what was assigned,
/// in key order.
#[tokio::test]
async fn tags_assigned_to_a_consumer_arn_are_reported_back_in_key_order() {
    let emulator = Emulator::new();
    let stream = "consumer-tag-assignment";
    emulator
        .ok(
            stream,
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, stream, stream).await;
    let arn = stream_arn(REGION, stream);

    for row in consumer_rows() {
        let consumer_arn =
            register_consumer(&emulator, row.label, &arn, row.label, &row.registered_with).await;

        for assignment in &row.assignments {
            let (status, response) = emulator
                .call(
                    "TagResource",
                    &json!({ "ResourceARN": consumer_arn, "Tags": tag_map(assignment) }),
                )
                .await;
            assert_eq!(
                (status, &response),
                (200, &json!({})),
                "case {}: tagging a registered consumer is served with the modelled empty \
                 body, got {status} with body {response}",
                row.label
            );
        }

        let reported = tags_by_arn(&emulator, row.label, &consumer_arn).await;
        assert_eq!(
            reported,
            in_key_order(&row.expected),
            "case {}: expected the consumer's stored tag set in key order, got {reported:?}",
            row.label
        );
    }
}

/// A consumer's tag set is its OWN: tagging the consumer leaves its stream's set alone and
/// tagging the stream leaves the consumer's alone, even where both carry the same key.
#[tokio::test]
async fn a_consumer_and_its_stream_carry_separate_tag_sets() {
    let emulator = Emulator::new();
    let stream = "consumer-tag-isolation";
    emulator
        .ok(
            stream,
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, stream, stream).await;
    let arn = stream_arn(REGION, stream);
    let consumer_arn = register_consumer(&emulator, stream, &arn, "isolated", &[]).await;

    emulator
        .ok(
            stream,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&[pair("scope", "the-stream")]) }),
        )
        .await;
    emulator
        .ok(
            stream,
            "TagResource",
            &json!({
                "ResourceARN": consumer_arn,
                "Tags": tag_map(&[pair("scope", "the-consumer")]),
            }),
        )
        .await;

    assert_eq!(
        tags_by_arn(&emulator, "the consumer's own set", &consumer_arn).await,
        vec![pair("scope", "the-consumer")],
        "case the consumer's own set: a consumer carries the tags assigned to ITS ARN"
    );
    assert_eq!(
        tags_by_arn(&emulator, "the stream's own set", &arn).await,
        vec![pair("scope", "the-stream")],
        "case the stream's own set: the stream keeps the tags assigned to its own ARN"
    );
}

/// `UntagResource` deletes from the very set the assignment wrote and the read reports: a key
/// the consumer carries goes, a key it does not carry is ignored, and every tag the request
/// does not name stays.
#[tokio::test]
async fn keys_unassigned_from_a_consumer_arn_are_gone_and_the_rest_stay() {
    let emulator = Emulator::new();
    let stream = "consumer-tag-removal";
    emulator
        .ok(
            stream,
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, stream, stream).await;
    let arn = stream_arn(REGION, stream);
    let registered = [
        pair("Environment", "production"),
        pair("Owner", "efo"),
        pair("Team", "platform"),
    ];

    // Each row names the keys the request unassigns and the pairs the consumer must then
    // report.
    struct Removal {
        case: &'static str,
        keys: Vec<&'static str>,
        expected: Vec<(String, String)>,
    }
    let cases = vec![
        Removal {
            case: "one-key-the-consumer-carries",
            keys: vec!["Owner"],
            expected: vec![pair("Environment", "production"), pair("Team", "platform")],
        },
        Removal {
            case: "every-key-the-consumer-carries",
            keys: vec!["Environment", "Owner", "Team"],
            expected: Vec::new(),
        },
        Removal {
            case: "a-key-the-consumer-does-not-carry",
            keys: vec!["never-assigned"],
            expected: registered.to_vec(),
        },
        Removal {
            case: "a-carried-key-beside-one-that-is-not",
            keys: vec!["Team", "never-assigned"],
            expected: vec![pair("Environment", "production"), pair("Owner", "efo")],
        },
    ];

    for Removal {
        case,
        keys,
        expected,
    } in &cases
    {
        let consumer_arn = register_consumer(&emulator, case, &arn, case, &registered).await;

        let (status, response) = emulator
            .call(
                "UntagResource",
                &json!({ "ResourceARN": consumer_arn, "TagKeys": keys }),
            )
            .await;
        assert_eq!(
            (status, &response),
            (200, &json!({})),
            "case {case}: unassigning from a registered consumer is served with the modelled \
             empty body, got {status} with body {response}"
        );

        let reported = tags_by_arn(&emulator, case, &consumer_arn).await;
        assert_eq!(
            reported,
            in_key_order(expected),
            "case {case}: expected what the removal left, got {reported:?}"
        );
    }
}

/// An assignment that would take a consumer past the documented tag ceiling is refused
/// before the stored map grows, so the tags it already carries are untouched.
#[tokio::test]
async fn an_assignment_past_the_consumer_tag_ceiling_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let stream = "consumer-tag-ceiling";
    emulator
        .ok(
            stream,
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, stream, stream).await;
    let arn = stream_arn(REGION, stream);
    let stored = pairs(MAX_CONSUMER_TAGS);
    let consumer_arn = register_consumer(&emulator, stream, &arn, "at-the-ceiling", &stored).await;

    let (status, response) = emulator
        .call(
            "TagResource",
            &json!({
                "ResourceARN": consumer_arn,
                "Tags": tag_map(&[pair("one-too-many", "refused")]),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case past the consumer ceiling: expected the modelled refusal, got {status} with \
         body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case past the consumer ceiling: expected LimitExceededException, got {response}"
    );
    assert_eq!(
        error_message(&response),
        tag_ceiling_exceeded_message(MAX_CONSUMER_TAGS + 1),
        "case past the consumer ceiling: expected the one sentence this service states the \
         ceiling in, got {response}"
    );
    assert_eq!(
        tags_by_arn(&emulator, "consumer-ceiling-read-back", &consumer_arn).await,
        in_key_order(&stored),
        "case consumer-ceiling-read-back: the refused assignment left the stored tags alone"
    );
}

/// Only an ARN this account and region hold NO resource under reaches the not-found refusal:
/// a consumer ARN under a stream of another region, a consumer name never registered, a
/// registration instant no consumer of that name was minted at, and the ARN of a consumer a
/// deregistration has released — the tag set is gone with the resource it belonged to.
#[tokio::test]
async fn a_consumer_arn_naming_no_registered_consumer_is_refused() {
    let emulator = Emulator::new();
    let stream = "consumer-tag-refusals";
    emulator
        .ok(
            stream,
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, stream, stream).await;
    let arn = stream_arn(REGION, stream);
    let live_arn = register_consumer(&emulator, stream, &arn, "still-registered", &[]).await;

    // A consumer that WAS registered and then deregistered: its record, and with it its tag
    // set, is released, so its ARN names no resource any more.
    let released_arn = register_consumer(&emulator, stream, &arn, "deregistered", &[]).await;
    emulator
        .ok(
            stream,
            "DeregisterStreamConsumer",
            &json!({ "ConsumerARN": released_arn }),
        )
        .await;
    poll_until(
        "a consumer a deregistration released",
        "the consumer is released",
        || async {
            let (status, response) = emulator
                .call(
                    "DescribeStreamConsumer",
                    &json!({ "ConsumerARN": released_arn }),
                )
                .await;
            status == 400 && error_code(&response) == "ResourceNotFoundException"
        },
    )
    .await;

    // Each candidate is the LIVE ARN with exactly one part changed, so the row differs only
    // in what makes it name no consumer this account and region hold.
    let rows: Vec<(&str, String)> = vec![
        (
            "a consumer of another region's stream",
            live_arn.replacen(&format!(":{REGION}:"), ":eu-west-1:", 1),
        ),
        (
            "a consumer name never registered",
            live_arn.replacen("/consumer/still-registered:", "/consumer/absent:", 1),
        ),
        (
            "another registration instant of a live name",
            format!("{live_arn}9"),
        ),
        ("a consumer a deregistration released", released_arn.clone()),
    ];

    for (case, candidate) in &rows {
        for operation in ["TagResource", "ListTagsForResource", "UntagResource"] {
            let body = match operation {
                "TagResource" => json!({
                    "ResourceARN": candidate,
                    "Tags": tag_map(&[pair("k", "v")]),
                }),
                "UntagResource" => json!({ "ResourceARN": candidate, "TagKeys": ["k"] }),
                _ => json!({ "ResourceARN": candidate }),
            };
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case} through {operation}: expected the modelled refusal, got {status} \
                 with body {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case} through {operation}: expected ResourceNotFoundException, got \
                 {response}"
            );
            assert_eq!(
                error_message(&response),
                // The sentence this service states for the KIND the ARN names: a consumer ARN
                // is refused as a missing CONSUMER under the stream its ARN extends, not as a
                // missing stream.
                consumer_not_found_message(candidate, candidate),
                "case {case} through {operation}: expected the consumer not-found sentence, \
                 got {response}"
            );
            assert_eq!(
                error_message(&response),
                resource_not_found_message(candidate),
                "case {case} through {operation}: one ARN is refused in one wording whatever \
                 operation of the family it reached, got {response}"
            );
        }
    }

    // The consumer this account does hold is untouched by the refusals, and still taggable.
    emulator
        .ok(
            "after the consumer refusals",
            "TagResource",
            &json!({ "ResourceARN": live_arn, "Tags": tag_map(&[pair("k", "v")]) }),
        )
        .await;
    assert_eq!(
        tags_by_arn(&emulator, "after the consumer refusals", &live_arn).await,
        vec![pair("k", "v")],
        "case after the consumer refusals: the live consumer still takes an assignment"
    );
}

/// A consumer's tag set exists for as long as the consumer does, so an assignment is served
/// whatever status the consumer is in — immediately after the registration answered, which is
/// the recorded flow, and once the consumer reports `ACTIVE`. The status is read only to name
/// it in the failure, never waited on by the clock.
#[tokio::test]
async fn an_assignment_to_a_consumer_is_served_in_every_status_it_reaches() {
    let emulator = Emulator::new();
    let stream = "consumer-tag-statuses";
    emulator
        .ok(
            stream,
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, stream, stream).await;
    let arn = stream_arn(REGION, stream);

    for (case, wait_for_active) in [("right-after-registration", false), ("once-active", true)] {
        let consumer_arn = register_consumer(&emulator, case, &arn, case, &[]).await;
        if wait_for_active {
            poll_until(case, "the consumer reports ACTIVE", || async {
                let (status, response) = emulator
                    .call(
                        "DescribeStreamConsumer",
                        &json!({ "ConsumerARN": consumer_arn }),
                    )
                    .await;
                status == 200
                    && response["ConsumerDescription"]["ConsumerStatus"].as_str() == Some("ACTIVE")
            })
            .await;
        }

        let described = emulator
            .ok(
                case,
                "DescribeStreamConsumer",
                &json!({ "ConsumerARN": consumer_arn }),
            )
            .await;
        let at_assignment = described["ConsumerDescription"]["ConsumerStatus"]
            .as_str()
            .unwrap_or("<no ConsumerStatus>")
            .to_string();

        let (status, response) = emulator
            .call(
                "TagResource",
                &json!({
                    "ResourceARN": consumer_arn,
                    "Tags": tag_map(&[pair("stamped-in", &at_assignment)]),
                }),
            )
            .await;
        assert_eq!(
            (status, &response),
            (200, &json!({})),
            "case {case}: an assignment to a consumer in {at_assignment} is served, got \
             {status} with body {response}"
        );
        assert_eq!(
            tags_by_arn(&emulator, case, &consumer_arn).await,
            vec![pair("stamped-in", &at_assignment)],
            "case {case}: the assignment served in {at_assignment} is stored"
        );
    }
}

/// `ListTagsForResource` on a consumer the registration tagged with nothing reports the EMPTY
/// list rather than refusing: the member is modelled `len 0..200`, so a resource carrying no
/// tags is a served read.
#[tokio::test]
async fn a_consumer_carrying_no_tags_reports_the_empty_list() {
    let emulator = Emulator::new();
    let stream = "consumer-tag-empty";
    emulator
        .ok(
            stream,
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, stream, stream).await;
    let arn = stream_arn(REGION, stream);
    let consumer_arn = register_consumer(&emulator, stream, &arn, "untagged", &[]).await;

    let read = emulator
        .ok(
            stream,
            "ListTagsForResource",
            &json!({ "ResourceARN": consumer_arn }),
        )
        .await;
    assert_eq!(
        read.get("Tags").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "case an untagged consumer: expected the empty Tags list, got {read}"
    );
}

// ---------------------------------------------------------------------------------------
// A QUEUE created on a stream is a resource of its own, addressed by the ARN its create
// minted: "You can also add, change, and remove tags after the queue is created with
// TagResource and UntagResource" (`CreateQueueInput$Tags`).
// ---------------------------------------------------------------------------------------

/// Create one queue on the stream `stream_arn` names, optionally carrying `tags`, and answer
/// the `QueueARN` the create minted — the identifier every call below addresses the queue by,
/// taken from the response rather than rebuilt here, because "the resource identifier in the
/// ARN is generated when the queue is created and is not derived from QueueName"
/// (`QueueDescription$QueueARN`).
async fn create_queue(
    emulator: &Emulator,
    case: &str,
    stream_arn: &str,
    queue: &str,
    tags: &[(String, String)],
) -> String {
    let mut body = json!({
        "QueueName": queue,
        "StreamConfigurationList": [{ "StreamARN": stream_arn }],
    });
    if !tags.is_empty() {
        body["Tags"] = tag_map(tags);
    }
    let answered = emulator.ok(case, "CreateQueue", &body).await;
    answered["QueueDescription"]["QueueARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case}: a create must answer QueueDescription.QueueARN, got {answered}")
        })
        .to_string()
}

/// RULE applied to the resource kind the model added: a queue carries ONE stored tag set of
/// its own — the tags its create assigned — and the resource-tag family reaches that set
/// through the queue's ARN exactly as it reaches a stream's and a consumer's: an assignment
/// overwrites the keys it names and leaves the rest, an unassignment deletes the keys it names
/// and ignores a key the queue does not carry, and the set is the QUEUE's rather than its
/// stream's.
///
/// The rows differ along what the create carried and what the later calls do to it: a queue
/// created with no tags at all and one created with tags, then an assignment that both
/// overwrites and adds, then an unassignment naming a key the queue carries beside one it does
/// not.
#[tokio::test]
async fn the_resource_tag_family_addresses_a_queue_by_the_arn_its_create_minted() {
    let emulator = Emulator::new();
    let stream = "queue-tag-resource";
    emulator
        .ok(
            stream,
            "CreateStream",
            &json!({
                "StreamName": stream,
                // Queues "are supported only on data streams in on-demand capacity mode"
                // (`CreateQueue`).
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
    poll_until_active(&emulator, stream, stream).await;
    let arn = stream_arn(REGION, stream);

    // (case, the tags the create carries, the set the queue must then report).
    let created_with: [(&str, Vec<(String, String)>); 2] = [
        ("a queue created with no tags", Vec::new()),
        (
            "a queue created with tags",
            vec![pair("Owner", "payments"), pair("Environment", "test")],
        ),
    ];
    for (index, (case, tags)) in created_with.into_iter().enumerate() {
        let queue_arn = create_queue(
            &emulator,
            case,
            &arn,
            &format!("tagged-queue-{index}"),
            &tags,
        )
        .await;
        assert_eq!(
            tags_by_arn(&emulator, case, &queue_arn).await,
            in_key_order(&tags),
            "case {case}: expected the queue to report the tags its create assigned"
        );

        // An assignment overwrites the keys it names and leaves every other tag alone — the
        // same rule the stream and the consumer rows above pin.
        emulator
            .ok(
                case,
                "TagResource",
                &json!({
                    "ResourceARN": &queue_arn,
                    "Tags": tag_map(&[pair("Owner", "billing"), pair("Team", "stream")]),
                }),
            )
            .await;
        let mut expected: Vec<(String, String)> = tags
            .iter()
            .filter(|(key, _)| key != "Owner")
            .cloned()
            .collect();
        expected.push(pair("Owner", "billing"));
        expected.push(pair("Team", "stream"));
        assert_eq!(
            tags_by_arn(&emulator, case, &queue_arn).await,
            in_key_order(&expected),
            "case {case}: expected the assignment to overwrite the key it named and leave the \
             rest"
        );

        // An unassignment deletes the key it names and ignores one the queue does not carry.
        emulator
            .ok(
                case,
                "UntagResource",
                &json!({
                    "ResourceARN": &queue_arn,
                    "TagKeys": ["Owner", "a-key-this-queue-does-not-carry"],
                }),
            )
            .await;
        let remaining: Vec<(String, String)> = expected
            .into_iter()
            .filter(|(key, _)| key != "Owner")
            .collect();
        assert_eq!(
            tags_by_arn(&emulator, case, &queue_arn).await,
            in_key_order(&remaining),
            "case {case}: expected the named key deleted and the unknown key ignored"
        );

        // The set is the QUEUE's: its stream carries none of it.
        assert_eq!(
            tags_by_arn(&emulator, case, &arn).await,
            Vec::new(),
            "case {case}: expected the queue's tags to be the queue's rather than its stream's"
        );
    }

    // An ARN of this resource KIND that no create minted is the not-found refusal, worded for
    // the kind the ARN names.
    let absent = format!(
        "arn:aws:kinesis:{REGION}:{}:queue/0123456789abcdef0123456789abcdef",
        engine::EMULATOR_ACCOUNT_ID
    );
    for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
        let mut body = json!({ "ResourceARN": &absent });
        match operation {
            "TagResource" => body["Tags"] = tag_map(&[pair("k", "v")]),
            "UntagResource" => body["TagKeys"] = json!(["k"]),
            _ => {}
        }
        let (status, response) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&response)),
            (400, "ResourceNotFoundException".to_string()),
            "case a queue ARN no create minted: expected {operation} refused, got {status} \
             with {response}"
        );
        assert_eq!(
            error_message(&response),
            resource_not_found_message(&absent),
            "case a queue ARN no create minted: expected the sentence this service states for \
             the kind the ARN names, got {response}"
        );
    }
}
