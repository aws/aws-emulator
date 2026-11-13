// SPDX-License-Identifier: CC0-1.0

//! A CHANNEL as a first-class resource of the account it belongs to, end to end over the
//! wire: `TagResource` / `UntagResource` / `ListTagsForResource` on a channel ARN, and
//! `DeleteStream` on a stream a channel SOURCES.
//!
//! Two rules, both about a channel being a resource of its own:
//!
//! 1. **a channel ARN is a tagging target.** `TagResourceInput$ResourceARN` is modelled
//!    `^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`
//!    (`smithy_query(query="operation",operation="TagResource")`), so the operation dispatches
//!    on the resource segment the ARN carries: a `channel/<name>` ARN is resolved against the
//!    channels this account and region hold and tagged there, and `ListTagsForResource` reports
//!    that stored set — the same stored set `UntagResource` unassigns from. An ARN naming no
//!    resource is refused `ResourceNotFoundException` worded for the KIND it names, so the
//!    stream sentence is reserved for `stream/` ARNs.
//! 2. **a stream a channel sources cannot be deleted.** A stream named in a channel's
//!    `StreamConfigurationList` is a dependency of that channel, so `DeleteStream` on it is
//!    refused `ResourceInUseException` ("The resource is not available for this operation") in
//!    the sentence the service publishes for it, the stream survives the refusal, and the
//!    identical call is served once the channels are deleted.
//!
//! The recorded flows this was derived from tag ONE channel just after it settles and delete
//! one stream a live channel sources, which are rows of the tables below. The rows differ
//! along what each rule generalises over: how many pairs one assignment carries and whether
//! its keys are new, the resource KIND the ARN names (a stream, a channel), the lifecycle
//! status the channel is in when the call arrives, and how many channels source the stream a
//! delete names.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! The wire harness is [`crate::stream_tags`]', and the channel builders
//! [`crate::channel_lifecycle`]' — reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    channel_arn, resource_not_found_message, stream_has_channels_message, CHANNEL_ACTIVE,
    CHANNEL_CREATING, STREAM_ACTIVE,
};

use crate::channel_lifecycle::{
    on_demand_stream, poll_until_channel_active, role_arn, storage_configuration,
    stream_configuration,
};
use crate::stream_tags::{
    error_code, error_message, poll_until_gone, stream_arn, Emulator, REGION,
};

/// The `CreateChannel` body for a channel named `name` sourcing `source_arn`.
fn create_body(name: &str, source_arn: &str) -> Value {
    json!({
        "ChannelName": name,
        "ServiceExecutionRoleARN": role_arn("channel-delivery"),
        "StreamConfigurationList": [stream_configuration(source_arn, "JSON", None)],
        "S3DestinationConfiguration": {
            "StorageConfiguration": storage_configuration("dependents-bucket"),
        },
    })
}

/// The `ChannelStatus` `DescribeChannel` reports for `arn`.
async fn channel_status(emulator: &Emulator, case: &str, arn: &str) -> String {
    let described = emulator
        .ok(case, "DescribeChannel", &json!({ "ChannelARN": arn }))
        .await;
    described["ChannelDescription"]["ChannelStatus"]
        .as_str()
        .unwrap_or("<no ChannelStatus>")
        .to_string()
}

/// The `StreamStatus` `DescribeStreamSummary` reports for `name`, and `Err(code)` when the
/// stream is no resource of this account any more.
async fn stream_status(emulator: &Emulator, name: &str) -> Result<String, String> {
    let (status, response) = emulator
        .call("DescribeStreamSummary", &json!({ "StreamName": name }))
        .await;
    if status != 200 {
        return Err(error_code(&response));
    }
    Ok(response["StreamDescriptionSummary"]["StreamStatus"]
        .as_str()
        .unwrap_or("<no StreamStatus>")
        .to_string())
}

/// The tags `ListTagsForResource` reports for `arn`, as sorted `(key, value)` pairs.
async fn listed_tags(emulator: &Emulator, case: &str, arn: &str) -> Vec<(String, String)> {
    let read = emulator
        .ok(case, "ListTagsForResource", &json!({ "ResourceARN": arn }))
        .await;
    let mut tags: Vec<(String, String)> = read["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    tags.sort();
    tags
}

/// A tag map as a request carries it.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// A channel ARN is tagged, read and untagged through the same operations a stream ARN is —
/// the ONE stored tag set per resource, whichever kind the ARN names.
#[tokio::test]
async fn a_channel_arn_is_a_tagging_target_like_a_stream_arn() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "dependents-tag-source").await;
    let channel = "dependents-tagged-channel";
    emulator
        .ok(channel, "CreateChannel", &create_body(channel, &source))
        .await;
    poll_until_channel_active(&emulator, channel, &channel_arn(REGION, channel)).await;
    assert_eq!(
        channel_status(&emulator, channel, &channel_arn(REGION, channel)).await,
        CHANNEL_ACTIVE,
        "case {channel}: expected the channel to have settled before it is tagged"
    );

    // (case, the ARN the calls address, the kind it names)
    let targets: [(&str, String); 2] = [
        ("a channel ARN", channel_arn(REGION, channel)),
        ("a stream ARN", stream_arn(REGION, "dependents-tag-source")),
    ];
    // One assignment of the table: what it carries, and the whole stored set afterwards.
    type Assignment = (&'static str, Vec<(&'static str, &'static str)>, Vec<(&'static str, &'static str)>);
    let assignments: [Assignment; 3] = [
        ("one pair", vec![("owner", "data")], vec![("owner", "data")]),
        (
            "two more pairs",
            vec![("env", "prod"), ("team", "ingest")],
            vec![("env", "prod"), ("owner", "data"), ("team", "ingest")],
        ),
        (
            "a key the resource already carries",
            vec![("owner", "platform")],
            vec![("env", "prod"), ("owner", "platform"), ("team", "ingest")],
        ),
    ];

    for (case, arn) in &targets {
        for (sub_case, assigned, expected) in &assignments {
            let (status, answered) = emulator
                .call(
                    "TagResource",
                    &json!({ "ResourceARN": arn, "Tags": tag_map(assigned) }),
                )
                .await;
            assert_eq!(
                (status, &answered),
                (200, &json!({})),
                "case {case}/{sub_case}: expected the modelled empty body, got {status} with \
                 {answered}"
            );
            let expected: Vec<(String, String)> = expected
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                listed_tags(&emulator, case, arn).await,
                expected,
                "case {case}/{sub_case}: expected the stored set a read reports"
            );
        }

        // The same stored set an unassignment reaches.
        emulator
            .ok(
                case,
                "UntagResource",
                &json!({ "ResourceARN": arn, "TagKeys": ["owner", "team"] }),
            )
            .await;
        assert_eq!(
            listed_tags(&emulator, case, arn).await,
            vec![("env".to_string(), "prod".to_string())],
            "case {case}: expected the unassignment to reach the very set the assignment \
             wrote"
        );
    }
}

/// A channel still inside the transition its create armed carries the same stored tag set: a
/// resource's tags exist for as long as the resource does.
#[tokio::test]
async fn a_channel_is_tagged_in_every_lifecycle_status() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "dependents-creating-source").await;
    let channel = "dependents-creating-channel";
    emulator
        .ok(channel, "CreateChannel", &create_body(channel, &source))
        .await;
    let arn = channel_arn(REGION, channel);
    assert_eq!(
        channel_status(&emulator, channel, &arn).await,
        CHANNEL_CREATING,
        "case {channel}: expected the channel still inside its create's transition"
    );
    emulator
        .ok(
            channel,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&[("stage", "creating")]) }),
        )
        .await;
    assert_eq!(
        listed_tags(&emulator, channel, &arn).await,
        vec![("stage".to_string(), "creating".to_string())],
        "case {channel}: expected the tag assigned while the channel was {CHANNEL_CREATING}"
    );
}

/// An ARN naming no resource is refused in the sentence for the KIND it names, so a
/// `channel/` ARN is never reported as a missing stream.
#[tokio::test]
async fn an_arn_naming_no_resource_is_refused_for_the_kind_it_names() {
    let emulator = Emulator::new();
    // (case, the ARN the call addresses)
    let rows: [(&str, String); 3] = [
        (
            "a channel nothing created",
            channel_arn(REGION, "dependents-no-such-channel"),
        ),
        (
            "a channel of another region",
            channel_arn("eu-west-1", "dependents-elsewhere"),
        ),
        (
            "a stream nothing created",
            stream_arn(REGION, "dependents-no-such-stream"),
        ),
    ];
    for (case, arn) in &rows {
        for operation in ["TagResource", "UntagResource", "ListTagsForResource"] {
            let mut body = json!({ "ResourceARN": arn });
            match operation {
                "TagResource" => body["Tags"] = tag_map(&[("k", "v")]),
                "UntagResource" => body["TagKeys"] = json!(["k"]),
                _ => {}
            }
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}/{operation}: expected the modelled refusal, got {status} with \
                 body {response}"
            );
            assert_eq!(
                error_message(&response),
                resource_not_found_message(arn),
                "case {case}/{operation}: expected the sentence for the kind the ARN names, \
                 got {response}"
            );
        }
    }
}

/// A stream a channel sources is not available for removal: the delete is refused, the
/// stream survives it, and the identical call is served once the channels are gone.
#[tokio::test]
async fn a_stream_a_channel_sources_cannot_be_deleted() {
    let emulator = Emulator::new();
    // (case, how many channels source the stream)
    let rows: [(&str, usize); 3] = [
        ("dependents-none", 0),
        ("dependents-one", 1),
        ("dependents-two", 2),
    ];
    for (name, channels) in rows {
        let source = on_demand_stream(&emulator, name).await;
        for index in 0..channels {
            let channel = format!("{name}-channel-{index}");
            emulator
                .ok(name, "CreateChannel", &create_body(&channel, &source))
                .await;
        }

        if channels == 0 {
            // The positive control: a stream nothing sources is deleted.
            emulator
                .ok(name, "DeleteStream", &json!({ "StreamName": name }))
                .await;
            continue;
        }

        let (status, response) = emulator
            .call("DeleteStream", &json!({ "StreamName": name }))
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceInUseException"),
            "case {name}: expected the delete of a stream {channels} channel(s) source to be \
             refused, got {status} with body {response}"
        );
        assert_eq!(
            error_message(&response),
            stream_has_channels_message(name),
            "case {name}: expected the sentence naming the stream and the account, got \
             {response}"
        );
        assert_eq!(
            stream_status(&emulator, name).await,
            Ok(STREAM_ACTIVE.to_string()),
            "case {name}: the stream survives the refusal"
        );

        // Deleting the channels one by one: the stream is still held back while any of them
        // is left.
        for index in 0..channels {
            let channel = format!("{name}-channel-{index}");
            emulator
                .ok(
                    name,
                    "DeleteChannel",
                    &json!({ "ChannelARN": channel_arn(REGION, &channel) }),
                )
                .await;
            let left = channels - index - 1;
            let (status, response) = emulator
                .call("DeleteStream", &json!({ "StreamName": name }))
                .await;
            if left > 0 {
                assert_eq!(
                    (status, error_code(&response).as_str()),
                    (400, "ResourceInUseException"),
                    "case {name}: expected the delete still refused while {left} channel(s) \
                     source the stream, got {status} with body {response}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {name}: expected the identical delete served once no channel \
                     sources the stream, got {status} with body {response}"
                );
            }
        }
        // The removal the accepted delete owes: looked for, not waited out.
        poll_until_gone(&emulator, name, name).await;
        assert_eq!(
            stream_status(&emulator, name).await,
            Err("ResourceNotFoundException".to_string()),
            "case {name}: expected the stream gone once its removal fell due"
        );
    }
}
