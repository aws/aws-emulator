// SPDX-License-Identifier: CC0-1.0

//! The Amazon Kinesis Video Streams resources a stack declares, end to end: the video stream an
//! `AWS::KinesisVideo::Stream` declares and the WebRTC signaling channel an
//! `AWS::KinesisVideo::SignalingChannel` declares exist in that service itself, under the names
//! the stack published, with the values the type publishes read off that service's own report —
//! and deleting the stack takes them away.
//!
//! RULE: a resource type AWS publishes is one a template may declare, and a declaration of it is
//! DEPLOYED by calling the service that owns the type — so the handle the stack publishes
//! (`Ref`) resolves in that service, the values the type publishes are the ones that service
//! reported, and what that service refuses is a FAILED RESOURCE rather than a template-format
//! error. The `Template format error: Unrecognized resource types` refusal is reserved for a
//! type CloudFormation does NOT know, which is the last test here.
//!
//! The rows of the deploy table differ along what the rule generalises over: WHICH of the two
//! types is declared, whether the declaration names the resource at all (a generated name is
//! held to the owning service's own rules for one), where the declared retention sits inside the
//! bound each property's own model publishes (a stream's `DataRetentionInHours`, `@range min 0`,
//! at its minimum and above it; a channel's `MessageTtlSeconds`, `@range 5..120`, at each
//! published bound and at the value that service defaults to), which properties the declaration
//! carries beside the name, whether it carries the type's own `Tags`, which published return
//! value the output reads (`Ref`, `Fn::GetAtt Arn`), the template spelling (JSON with long-form
//! intrinsics, YAML with the short-form tags), and how many resources one stack declares.
//!
//! Everything goes over the wire each service speaks through the one router `harness.rs` builds —
//! the CloudFormation calls over its awsQuery wire, the Kinesis Video Streams calls over that
//! service's own `restJson1` wire — so what is asserted is what a client that deploys the
//! template and then calls Kinesis Video Streams observes.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    published_attribute_names, IMPLEMENTED_RESOURCE_TYPES,
};

use crate::harness::{
    call, create_stack, member, router, settled_stack_status, stack_event_reasons, REGION,
    SETTLE_DEADLINE, SETTLE_POLL_INTERVAL,
};
use crate::stack_lifecycle::{describe_stack, output_value};

/// The two resource types under test.
const STREAM: &str = "AWS::KinesisVideo::Stream";
const CHANNEL: &str = "AWS::KinesisVideo::SignalingChannel";

/// The fixed text CloudFormation refuses a template declaring a type it does NOT know with,
/// which no row of the deploy table may ever see.
const UNRECOGNIZED_PREFIX: &str = "Template format error: Unrecognized resource types: [";

/// The status a stream and a channel are usable in — the terminal state of either create.
const ACTIVE: &str = "ACTIVE";

/// The one channel type Kinesis Video Streams makes a channel in when a declaration names none:
/// "Currently, `SINGLE_MASTER` is the only supported channel type" (`ChannelType`).
const SINGLE_MASTER: &str = "SINGLE_MASTER";

/// The retention window that service gives a channel whose declaration states none: "the default
/// configuration for the channel message's time to live is 60 seconds (1 minute)".
const DEFAULT_MESSAGE_TTL: i64 = 60;

/// The published bounds of a channel's message retention window
/// (`SingleMasterConfiguration$MessageTtlSeconds`, `@range min 5 max 120`).
const MIN_MESSAGE_TTL: i64 = 5;
const MAX_MESSAGE_TTL: i64 = 120;

/// The error code that service answers for a stream or a channel it does not hold, the status it
/// carries, and the sentence each read words it with.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";
const NOT_FOUND_STATUS: u16 = 404;
const STREAM_NOT_FOUND: &str = "The requested stream is not found or not active.";
const CHANNEL_NOT_FOUND: &str = "The requested channel is not found or not active.";

/// The key a stack of this suite carries its own tag under, which CloudFormation propagates to
/// every resource it creates.
const STACK_TAG_KEY: &str = "sim2real";

/// One Kinesis Video Streams call over the `restJson1` wire its model declares, on the URI that
/// operation's own `@http` binding names (every one of them a `POST`), signed for
/// `kinesisvideo` — that model's own `sigv4Name` — in the Region the stack's resources live in.
async fn kinesisvideo(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://kinesisvideo.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("kinesisvideo.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesisvideo/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed Kinesis Video Streams request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// That service's own read of one stream, by the name the stack published.
async fn describe_stream(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    kinesisvideo(router, "/describeStream", &json!({"StreamName": name})).await
}

/// That service's own read of one signaling channel, by the name the stack published.
async fn describe_channel(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    kinesisvideo(
        router,
        "/describeSignalingChannel",
        &json!({"ChannelName": name}),
    )
    .await
}

/// Every stream that service lists for the account and Region.
async fn list_streams(router: &MultiServiceRouter) -> Vec<Value> {
    let (_, answered) = kinesisvideo(router, "/listStreams", &json!({})).await;
    answered
        .get("StreamInfoList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every signaling channel it lists for them.
async fn list_channels(router: &MultiServiceRouter) -> Vec<Value> {
    let (_, answered) = kinesisvideo(router, "/listSignalingChannels", &json!({})).await;
    answered
        .get("ChannelInfoList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The tags that service holds on one resource, read through the surface that resource's tags
/// live on: a stream's own `ListTagsForStream`, by its name, and a channel's through the generic
/// `ListTagsForResource`, by the ARN that service minted. Both answer the `ResourceTags` map.
async fn held_tags(
    router: &MultiServiceRouter,
    case: &str,
    kind: Kind,
    name: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (uri, request) = match kind {
        Kind::Stream => ("/listTagsForStream", json!({"StreamName": name})),
        Kind::Channel => ("/ListTagsForResource", json!({"ResourceARN": arn})),
    };
    let (status, answered) = kinesisvideo(router, uri, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the tag read of {name}, got {status} with \
         {answered}"
    );
    answered
        .get("Tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Which of the two types a declared resource is, which decides every member name the reads
/// above and the assertions below spell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Stream,
    Channel,
}

impl Kind {
    /// The type a template declares the resource as.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Stream => STREAM,
            Self::Channel => CHANNEL,
        }
    }

    /// The member that service reports the resource's own name in.
    fn name_member(self) -> &'static str {
        match self {
            Self::Stream => "StreamName",
            Self::Channel => "ChannelName",
        }
    }

    /// The member it reports the ARN it minted in.
    fn arn_member(self) -> &'static str {
        match self {
            Self::Stream => "StreamARN",
            Self::Channel => "ChannelARN",
        }
    }

    /// The member it reports the resource's status in.
    fn status_member(self) -> &'static str {
        match self {
            Self::Stream => "Status",
            Self::Channel => "ChannelStatus",
        }
    }

    /// The resource path the ARN it minted carries before the resource's name.
    fn arn_path(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Channel => "channel",
        }
    }
}

/// Read one resource until that service reports it [`ACTIVE`], or answer the last read once the
/// deadline passes: the status is one the CLOCK makes due, so what is waited for is the
/// CONDITION rather than a span of wall clock.
async fn active_resource(
    router: &MultiServiceRouter,
    case: &str,
    kind: Kind,
    name: &str,
) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let record = reported(router, case, kind, name).await;
        let status = record
            .get(kind.status_member())
            .and_then(Value::as_str)
            .unwrap_or_default();
        if status == ACTIVE || std::time::Instant::now() >= deadline {
            return record;
        }
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    }
}

/// That service's own record of one resource, which it must hold for a resource a completed
/// stack created.
async fn reported(router: &MultiServiceRouter, case: &str, kind: Kind, name: &str) -> Value {
    let (status, answered) = match kind {
        Kind::Stream => describe_stream(router, name).await,
        Kind::Channel => describe_channel(router, name).await,
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected Kinesis Video Streams to report the {} the stack created under \
         {name:?}, got {status} with {answered}",
        kind.resource_type()
    );
    let member = match kind {
        Kind::Stream => "StreamInfo",
        Kind::Channel => "ChannelInfo",
    };
    answered
        .get(member)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {answered}"))
}

/// Deploy `template` as `stack_name` under the stack's own tag, and answer the `DescribeStacks`
/// body of the COMPLETED stack, quoting the stack's own failure reasons when the create did not
/// complete.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    submitted(router, case, stack_name, template).await;
    let reached = settled_stack_status(case, router, REGION, stack_name).await;
    assert_eq!(
        reached, "CREATE_COMPLETE",
        "case {case:?}: expected the create to complete, got {reached} with failure reasons {:?}",
        stack_event_reasons(router, REGION, stack_name).await
    );
    let (status, described) = describe_stack(router, stack_name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {described}"
    );
    described
}

/// Submit `template` as `stack_name`, carrying the stack's own tag, and answer the `CreateStack`
/// body: a type AWS publishes is ADMITTED, so this never sees the template-format refusal.
async fn submitted(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = create_stack(
        router,
        stack_name,
        template,
        &[
            member("Tags.member.1.Key", STACK_TAG_KEY),
            member("Tags.member.1.Value", stack_name),
        ],
    )
    .await;
    assert!(
        !body.contains(UNRECOGNIZED_PREFIX),
        "case {case:?}: expected a type Amazon Web Services publishes to be recognised, got \
         {body}"
    );
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack — both AWS::KinesisVideo::* types are \
         types a template may declare — got {status} with body {body}"
    );
    body
}

/// Whether `arn` has the form a `ResourceARN` publishes for one of these resources
/// (`^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`): the
/// namespace and Region of this account, the resource path of the type, the resource's own name
/// and the instant that service recorded it at.
fn is_a_kinesis_video_arn(arn: &str, kind: Kind, name: &str) -> bool {
    let Some((prefix, resource)) = arn.split_once(":kinesisvideo:") else {
        return false;
    };
    let mut segments = resource.splitn(4, ':');
    let (region, account, path) = (
        segments.next().unwrap_or_default(),
        segments.next().unwrap_or_default(),
        segments.next().unwrap_or_default(),
    );
    let Some(recorded_at) = path
        .strip_prefix(&format!("{}/{name}/", kind.arn_path()))
        .filter(|digits| !digits.is_empty())
    else {
        return false;
    };
    prefix == "arn:aws"
        && region == REGION
        && account.chars().all(|digit| digit.is_ascii_digit())
        && !account.is_empty()
        && recorded_at.chars().all(|digit| digit.is_ascii_digit())
}

/// What one row expects of one resource the stack declares.
struct Declared {
    /// Which type it is.
    kind: Kind,
    /// The name the declaration states, or `None` where it names the resource nowhere and
    /// CloudFormation generates one.
    name: Option<&'static str>,
    /// The output key whose value is what `Ref` returned — the resource's own name.
    ref_output: &'static str,
    /// The output key reading the one attribute the type publishes, where the row reads it.
    arn_output: Option<&'static str>,
    /// The scalars the declaration states, as the owning service must report them back:
    /// `(the member that service reports, the value it must carry)`. A value left `Null` is one
    /// the row's own template declares and [`expected_scalar`] reads back out of it.
    reported: Vec<(&'static str, Value)>,
    /// The retention window a channel's declaration asks for, which that service reports nested
    /// inside the channel's `SingleMasterConfiguration`; `None` for a stream.
    message_ttl: Option<i64>,
    /// The type's own `Tags` the declaration states, which the resource must carry beside the
    /// stack's own propagated tag.
    tags: &'static [(&'static str, &'static str)],
}

/// One row: a template and the resources it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    declared: Vec<Declared>,
}

fn rows() -> Vec<Row> {
    vec![
        // The issue's own stream stack: a named stream with a retention window above the
        // published minimum and a declared tag, both return values read back, in JSON with
        // long-form intrinsics.
        Row {
            case: "a named stream with a retention window and a tag, both return values read",
            stack_name: "kvs-stream-round-trip",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Stream": {
                        "Type": STREAM,
                        "Properties": {
                            "Name": "kvs-round-trip",
                            "DataRetentionInHours": 24,
                            "Tags": [{"Key": "sim2real-kvs", "Value": "round-trip"}],
                        }
                    }
                },
                "Outputs": {
                    "StreamName": {"Value": {"Ref": "Stream"}},
                    "StreamArn": {"Value": {"Fn::GetAtt": ["Stream", "Arn"]}}
                }
            })
            .to_string(),
            declared: vec![Declared {
                kind: Kind::Stream,
                name: Some("kvs-round-trip"),
                ref_output: "StreamName",
                arn_output: Some("StreamArn"),
                reported: vec![("DataRetentionInHours", Value::Null)],
                message_ttl: None,
                tags: &[("sim2real-kvs", "round-trip")],
            }],
        },
        // A stream at the published MINIMUM retention ("the default value is 0, indicating that
        // the stream does not persist data"), carrying the two descriptive properties the type
        // publishes and no tags of its own, in YAML with short-form intrinsics.
        Row {
            case: "a stream at the minimum retention with a device and a media type, YAML",
            stack_name: "kvs-stream-media",
            template: "\
Resources:
  Stream:
    Type: AWS::KinesisVideo::Stream
    Properties:
      Name: kvs-media
      DataRetentionInHours: 0
      DeviceName: kvs-device
      MediaType: video/h264
Outputs:
  StreamName:
    Value: !Ref Stream
  StreamArn:
    Value: !GetAtt Stream.Arn
"
            .to_string(),
            declared: vec![Declared {
                kind: Kind::Stream,
                name: Some("kvs-media"),
                ref_output: "StreamName",
                arn_output: Some("StreamArn"),
                reported: vec![
                    ("DeviceName", Value::Null),
                    ("MediaType", Value::Null),
                    ("DataRetentionInHours", Value::Null),
                ],
                message_ttl: None,
                tags: &[],
            }],
        },
        // A signaling channel of the one published type, with its message retention window at
        // the published MINIMUM and a tag of its own.
        Row {
            case: "a single-master channel at the minimum message retention, with a tag",
            stack_name: "kvs-channel-round-trip",
            template: json!({
                "Resources": {
                    "Channel": {
                        "Type": CHANNEL,
                        "Properties": {
                            "Name": "kvs-channel-round-trip",
                            "Type": SINGLE_MASTER,
                            "MessageTtlSeconds": MIN_MESSAGE_TTL,
                            "Tags": [{"Key": "sim2real-kvs", "Value": "channel"}],
                        }
                    }
                },
                "Outputs": {
                    "ChannelName": {"Value": {"Ref": "Channel"}},
                    "ChannelArn": {"Value": {"Fn::GetAtt": ["Channel", "Arn"]}}
                }
            })
            .to_string(),
            declared: vec![Declared {
                kind: Kind::Channel,
                name: Some("kvs-channel-round-trip"),
                ref_output: "ChannelName",
                arn_output: Some("ChannelArn"),
                reported: vec![("ChannelType", Value::String(SINGLE_MASTER.to_string()))],
                message_ttl: Some(MIN_MESSAGE_TTL),
                tags: &[("sim2real-kvs", "channel")],
            }],
        },
        // A channel at the published MAXIMUM window, declaring NO type at all — that service
        // makes it in the one type it supports — in YAML.
        Row {
            case: "a channel at the maximum message retention, declaring no type, YAML",
            stack_name: "kvs-channel-defaulted-type",
            template: format!(
                "\
Resources:
  Channel:
    Type: {CHANNEL}
    Properties:
      Name: kvs-channel-max-ttl
      MessageTtlSeconds: {MAX_MESSAGE_TTL}
Outputs:
  ChannelName:
    Value: !Ref Channel
  ChannelArn:
    Value: !GetAtt Channel.Arn
"
            ),
            declared: vec![Declared {
                kind: Kind::Channel,
                name: Some("kvs-channel-max-ttl"),
                ref_output: "ChannelName",
                arn_output: Some("ChannelArn"),
                reported: vec![("ChannelType", Value::String(SINGLE_MASTER.to_string()))],
                message_ttl: Some(MAX_MESSAGE_TTL),
                tags: &[],
            }],
        },
        // ONE stack declaring BOTH types, each naming nothing at all: CloudFormation generates
        // the name, and the resource is created under it in the owning service — the channel
        // with the retention window that service defaults to, and only the `Ref` read back.
        Row {
            case: "one stack declaring a generated-name stream and a generated-name channel",
            stack_name: "kvs-generated-names",
            template: json!({
                "Resources": {
                    "Stream": {"Type": STREAM, "Properties": {"DataRetentionInHours": 1}},
                    "Channel": {"Type": CHANNEL, "Properties": {}}
                },
                "Outputs": {
                    "StreamName": {"Value": {"Ref": "Stream"}},
                    "ChannelName": {"Value": {"Ref": "Channel"}}
                }
            })
            .to_string(),
            declared: vec![
                Declared {
                    kind: Kind::Stream,
                    name: None,
                    ref_output: "StreamName",
                    arn_output: None,
                    reported: vec![("DataRetentionInHours", Value::Null)],
                    message_ttl: None,
                    tags: &[],
                },
                Declared {
                    kind: Kind::Channel,
                    name: None,
                    ref_output: "ChannelName",
                    arn_output: None,
                    reported: vec![("ChannelType", Value::String(SINGLE_MASTER.to_string()))],
                    message_ttl: Some(DEFAULT_MESSAGE_TTL),
                    tags: &[],
                },
            ],
        },
    ]
}

/// The value one declared scalar must be reported with: the one the row states, or the one the
/// row's own template declared where the row leaves it `Null` (the row states a value only where
/// the OWNING SERVICE decides it).
fn expected_scalar(template: &str, logical_id: &str, property: &str, stated: &Value) -> Value {
    if !stated.is_null() {
        return stated.clone();
    }
    // The declaration is read out of the row's own template, so no row restates a value twice.
    // A YAML row carries it as `  <property>: <value>` and a JSON row as `"<property>":<value>`.
    if let Ok(parsed) = serde_json::from_str::<Value>(template) {
        if let Some(declared) = parsed
            .get("Resources")
            .and_then(|resources| resources.get(logical_id))
            .and_then(|resource| resource.get("Properties"))
            .and_then(|properties| properties.get(property))
        {
            return declared.clone();
        }
    }
    // The YAML spelling carries it as `  <property>: <value>`, inside the block of the resource
    // that declares it: the scan starts at that resource's own `  <logical id>:` line and stops
    // at the next line indented as far, so a sibling resource's property of the same name is
    // never picked up.
    let marker = format!("{property}: ");
    let block = format!("{logical_id}:");
    let mut inside = false;
    for line in template.lines() {
        let trimmed = line.trim();
        if trimmed == block {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.starts_with("Resources:") || !line.starts_with(' ') {
            break;
        }
        if let Some(value) = trimmed.strip_prefix(&marker) {
            return match value.parse::<i64>() {
                Ok(number) => Value::Number(number.into()),
                Err(_) => Value::String(value.to_string()),
            };
        }
    }
    Value::Null
}

#[tokio::test]
async fn a_declared_kinesis_video_resource_is_created_in_the_owning_service() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let described = deployed(&router, case, row.stack_name, &row.template).await;

        for declared in &row.declared {
            let kind = declared.kind;
            // `Ref` resolves to the resource's own NAME, which is the handle every call of that
            // service takes — the declared one where the template states it.
            let published = output_value(&described, declared.ref_output).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no {} output in {described}",
                    declared.ref_output
                )
            });
            if let Some(name) = declared.name {
                assert_eq!(
                    published, name,
                    "case {case:?}: expected Ref to resolve to the declared name, got \
                     {published:?}"
                );
            } else {
                assert!(
                    !published.is_empty()
                        && published
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
                        && published.chars().count() <= 256,
                    "case {case:?}: expected a generated name inside the owning service's own \
                     rules for one, got {published:?}"
                );
            }

            // The resource really is in that service, and it comes up: a created stream and a
            // created channel are both CREATING, and the stack completed only once that service
            // reported the resource ACTIVE.
            let record = active_resource(&router, case, kind, &published).await;
            assert_eq!(
                (
                    record.get(kind.name_member()).and_then(Value::as_str),
                    record.get(kind.status_member()).and_then(Value::as_str),
                ),
                (Some(published.as_str()), Some(ACTIVE)),
                "case {case:?}: expected an ACTIVE {} named {published:?}, got {record}",
                kind.resource_type()
            );

            // The ARN is that service's own, in the form its model publishes, and it is what the
            // type's `Fn::GetAtt Arn` resolves to where the row reads it.
            let arn = record
                .get(kind.arn_member())
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                is_a_kinesis_video_arn(&arn, kind, &published),
                "case {case:?}: expected the ARN that service minted for {published:?}, got \
                 {arn:?}"
            );
            if let Some(key) = declared.arn_output {
                let read = output_value(&described, key)
                    .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {described}"));
                assert_eq!(
                    read, arn,
                    "case {case:?}: expected the published {key} to be the ARN that service \
                     reports, got {read:?}"
                );
            }

            // ...and the listing that service serves resolves the same identity.
            let listed = match kind {
                Kind::Stream => list_streams(&router).await,
                Kind::Channel => list_channels(&router).await,
            };
            assert!(
                listed.iter().any(|summary| {
                    summary.get(kind.name_member()).and_then(Value::as_str)
                        == Some(published.as_str())
                        && summary.get(kind.arn_member()).and_then(Value::as_str)
                            == Some(arn.as_str())
                }),
                "case {case:?}: expected the listing to carry {published:?} under {arn:?}, got \
                 {listed:?}"
            );

            // Every declared scalar reads back off that service's own record of the resource.
            let logical_id = match kind {
                Kind::Stream => "Stream",
                Kind::Channel => "Channel",
            };
            for (reported_member, stated) in &declared.reported {
                let expected =
                    expected_scalar(&row.template, logical_id, reported_member, stated);
                assert_eq!(
                    record.get(*reported_member),
                    Some(&expected),
                    "case {case:?}: expected {reported_member} to read back as {expected}, got \
                     {record}"
                );
            }
            if let Some(ttl) = declared.message_ttl {
                assert_eq!(
                    record
                        .pointer("/SingleMasterConfiguration/MessageTtlSeconds")
                        .and_then(Value::as_i64),
                    Some(ttl),
                    "case {case:?}: expected the channel's message retention window to be \
                     {ttl}, got {record}"
                );
            }

            // The tags the resource carries are the declaration's own beside the stack's.
            let held = held_tags(&router, case, kind, &published, &arn).await;
            let mut expected: BTreeMap<String, String> = declared
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            expected.insert(STACK_TAG_KEY.to_string(), row.stack_name.to_string());
            assert!(
                expected
                    .iter()
                    .all(|(key, value)| held.get(key) == Some(value)),
                "case {case:?}: expected the declared tags beside the stack's own {expected:?}, \
                 got {held:?}"
            );
            // ...and CloudFormation's own three records of where the resource came from.
            for key in [
                "aws:cloudformation:stack-name",
                "aws:cloudformation:stack-id",
                "aws:cloudformation:logical-id",
            ] {
                assert!(
                    held.contains_key(key),
                    "case {case:?}: expected the resource to carry {key}, got {held:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_changed_declaration_reaches_the_resource_the_stack_has() {
    // A changed declaration of a property the owning service CAN change on a resource that
    // exists is applied to that resource — a stream's retention window through
    // `UpdateDataRetention` (in either direction) and a channel's message retention window
    // through `UpdateSignalingChannel` — rather than replacing it or being dropped; the tag set
    // each resource ends up carrying is reconciled beside it.
    let router = router();
    let case = "a stack whose stream and channel are re-declared twice";
    let stack_name = "kvs-updated";
    let template_of = |retention: i64, ttl: i64, tag: (&str, &str)| {
        json!({
            "Resources": {
                "Stream": {
                    "Type": STREAM,
                    "Properties": {
                        "Name": "kvs-updated-stream",
                        "DataRetentionInHours": retention,
                        "Tags": [{"Key": tag.0, "Value": tag.1}],
                    }
                },
                "Channel": {
                    "Type": CHANNEL,
                    "Properties": {
                        "Name": "kvs-updated-channel",
                        "MessageTtlSeconds": ttl,
                        "Tags": [{"Key": tag.0, "Value": tag.1}],
                    }
                }
            },
            "Outputs": {
                "StreamName": {"Value": {"Ref": "Stream"}},
                "StreamArn": {"Value": {"Fn::GetAtt": ["Stream", "Arn"]}},
                "ChannelName": {"Value": {"Ref": "Channel"}},
                "ChannelArn": {"Value": {"Fn::GetAtt": ["Channel", "Arn"]}}
            }
        })
        .to_string()
    };

    let described = deployed(
        &router,
        case,
        stack_name,
        &template_of(10, 20, ("before", "one")),
    )
    .await;
    let stream = output_value(&described, "StreamName")
        .unwrap_or_else(|| panic!("case {case:?}: no StreamName output in {described}"));
    let channel = output_value(&described, "ChannelName")
        .unwrap_or_else(|| panic!("case {case:?}: no ChannelName output in {described}"));
    let stream_arn = active_resource(&router, case, Kind::Stream, &stream)
        .await
        .get("StreamARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let channel_arn = active_resource(&router, case, Kind::Channel, &channel)
        .await
        .get("ChannelARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // (the case, the retention and window the deployment declares, the tag it gives both)
    let deployments: [(&str, i64, i64, (&str, &str)); 2] = [
        ("narrowed retention and a widened window", 4, 100, ("after", "two")),
        ("widened retention and a narrowed window", 48, 7, ("later", "three")),
    ];
    for (phase, retention, ttl, tag) in deployments {
        let case = format!("{case}: {phase}");
        let case = case.as_str();
        let (status, updated) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack_name),
                member("TemplateBody", &template_of(retention, ttl, tag)),
                member("Tags.member.1.Key", STACK_TAG_KEY),
                member("Tags.member.1.Value", stack_name),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {updated}"
        );
        let reached = settled_stack_status(case, &router, REGION, stack_name).await;
        assert_eq!(
            reached, "UPDATE_COMPLETE",
            "case {case:?}: expected the update to complete, got {reached} with failure reasons \
             {:?}",
            stack_event_reasons(&router, REGION, stack_name).await
        );

        // The change reached the resources THAT service holds, and each kept the identity it was
        // created with — the ARN carries the instant that service recorded it at, so a
        // replacement would publish another one.
        let reported_stream = reported(&router, case, Kind::Stream, &stream).await;
        assert_eq!(
            (
                reported_stream
                    .get("DataRetentionInHours")
                    .and_then(Value::as_i64),
                reported_stream.get("StreamARN").and_then(Value::as_str),
            ),
            (Some(retention), Some(stream_arn.as_str())),
            "case {case:?}: expected the stream to carry {retention} hours under the ARN it was \
             created with, got {reported_stream}"
        );
        let reported_channel = reported(&router, case, Kind::Channel, &channel).await;
        assert_eq!(
            (
                reported_channel
                    .pointer("/SingleMasterConfiguration/MessageTtlSeconds")
                    .and_then(Value::as_i64),
                reported_channel.get("ChannelARN").and_then(Value::as_str),
            ),
            (Some(ttl), Some(channel_arn.as_str())),
            "case {case:?}: expected the channel to carry a {ttl} second window under the ARN it \
             was created with, got {reported_channel}"
        );

        // And the tag set each resource ends up carrying is this deployment's: the key it no
        // longer gives the resource is gone, and the stack's own is still there.
        for (kind, name, arn) in [
            (Kind::Stream, &stream, &stream_arn),
            (Kind::Channel, &channel, &channel_arn),
        ] {
            let held = held_tags(&router, case, kind, name, arn).await;
            assert_eq!(
                (
                    held.get(tag.0).map(String::as_str),
                    held.get("before").map(String::as_str),
                    held.get(STACK_TAG_KEY).map(String::as_str),
                ),
                (Some(tag.1), None, Some(stack_name)),
                "case {case:?}: expected the {} to carry this deployment's tags alone, got \
                 {held:?}",
                kind.resource_type()
            );
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_removes_the_stream_and_the_channel() {
    // A stack's resources are the stack's own: deleting it takes them away in the owning
    // service, which then answers its own not-found sentence for each.
    let router = router();
    let case = "one stack holding a stream and a channel, deleted";
    let stack_name = "kvs-deleted";
    let template = json!({
        "Resources": {
            "Stream": {
                "Type": STREAM,
                "Properties": {"Name": "kvs-deleted-stream", "DataRetentionInHours": 2}
            },
            "Channel": {
                "Type": CHANNEL,
                "Properties": {"Name": "kvs-deleted-channel", "MessageTtlSeconds": 30}
            }
        },
        "Outputs": {
            "StreamName": {"Value": {"Ref": "Stream"}},
            "ChannelName": {"Value": {"Ref": "Channel"}}
        }
    })
    .to_string();

    let described = deployed(&router, case, stack_name, &template).await;
    let stream = output_value(&described, "StreamName")
        .unwrap_or_else(|| panic!("case {case:?}: no StreamName output in {described}"));
    let channel = output_value(&described, "ChannelName")
        .unwrap_or_else(|| panic!("case {case:?}: no ChannelName output in {described}"));
    active_resource(&router, case, Kind::Stream, &stream).await;
    active_resource(&router, case, Kind::Channel, &channel).await;

    let (status, deleted) = call(&router, "DeleteStack", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    settle_the_deletion(&router, case, &stream, &channel).await;

    // Each read says so in that service's own words, under the code and status its model
    // declares for a resource it does not hold.
    for (kind, name, sentence) in [
        (Kind::Stream, &stream, STREAM_NOT_FOUND),
        (Kind::Channel, &channel, CHANNEL_NOT_FOUND),
    ] {
        let (status, answered) = match kind {
            Kind::Stream => describe_stream(&router, name).await,
            Kind::Channel => describe_channel(&router, name).await,
        };
        assert_eq!(
            (
                status,
                answered.get("__type").and_then(Value::as_str),
                answered.get("Message").and_then(Value::as_str),
            ),
            (
                NOT_FOUND_STATUS,
                Some(RESOURCE_NOT_FOUND),
                Some(sentence),
            ),
            "case {case:?}: expected the {} {name:?} to be gone with the stack, got {status} \
             with {answered}",
            kind.resource_type()
        );
    }
    let left_streams = list_streams(&router).await;
    assert!(
        !left_streams
            .iter()
            .any(|summary| summary.get("StreamName").and_then(Value::as_str)
                == Some(stream.as_str())),
        "case {case:?}: expected no stream left for {stream:?}, got {left_streams:?}"
    );
    let left_channels = list_channels(&router).await;
    assert!(
        !left_channels
            .iter()
            .any(|summary| summary.get("ChannelName").and_then(Value::as_str)
                == Some(channel.as_str())),
        "case {case:?}: expected no channel left for {channel:?}, got {left_channels:?}"
    );
}

/// Wait until that service holds neither resource.
///
/// Waits for the CONDITION rather than for a span, under the harness's own generous deadline, so
/// a loaded host cannot decide the outcome: a deleted stream is MARKED for deletion and removed
/// when the clock makes its removal due.
async fn settle_the_deletion(
    router: &MultiServiceRouter,
    case: &str,
    stream: &str,
    channel: &str,
) {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (stream_status, _) = describe_stream(router, stream).await;
        let (channel_status, _) = describe_channel(router, channel).await;
        if stream_status == NOT_FOUND_STATUS && channel_status == NOT_FOUND_STATUS {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected both resources the stack created to be released, the stream \
             read answers {stream_status} and the channel read {channel_status}"
        );
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_declaration_the_owning_service_refuses_rolls_the_stack_back() {
    // A declared value outside the bound the owning service's own model publishes is refused by
    // THAT service, so the resource fails and the stack rolls back — never the template-format
    // refusal a type CloudFormation does not know is answered with.
    let router = router();
    let case = "a channel one second past the published maximum message retention";
    let stack_name = "kvs-channel-over-max-ttl";
    let name = "kvs-over-max-ttl";
    let template = json!({
        "Resources": {
            "Channel": {
                "Type": CHANNEL,
                "Properties": {"Name": name, "MessageTtlSeconds": MAX_MESSAGE_TTL + 1}
            }
        },
        "Outputs": {"ChannelName": {"Value": {"Ref": "Channel"}}}
    })
    .to_string();

    // What that service answers for exactly that create is read from the service itself rather
    // than written out here.
    let (refused_status, refusal) = kinesisvideo(
        &router,
        "/createSignalingChannel",
        &json!({
            "ChannelName": "kvs-over-max-ttl-direct",
            "SingleMasterConfiguration": {"MessageTtlSeconds": MAX_MESSAGE_TTL + 1},
        }),
    )
    .await;
    assert_eq!(
        refused_status, 400,
        "case {case:?}: expected Kinesis Video Streams to refuse the window itself, got \
         {refused_status} with {refusal}"
    );

    submitted(&router, case, stack_name, &template).await;
    let reached = settled_stack_status(case, &router, REGION, stack_name).await;
    assert_eq!(
        reached, "ROLLBACK_COMPLETE",
        "case {case:?}: expected the failed create to be rolled back rather than publish a \
         channel nothing created, got {reached} with failure reasons {:?}",
        stack_event_reasons(&router, REGION, stack_name).await
    );
    let (_, described) = describe_stack(&router, stack_name).await;
    assert_eq!(
        output_value(&described, "ChannelName"),
        None,
        "case {case:?}: expected no output for a resource nothing created, got {described}"
    );

    // The failure is the owning service's own, on the resource that failed — and never a
    // refusal of the template.
    let reasons = stack_event_reasons(&router, REGION, stack_name).await;
    assert!(
        !reasons.is_empty()
            && reasons
                .iter()
                .all(|reason| !reason.contains(UNRECOGNIZED_PREFIX)),
        "case {case:?}: expected the owning service's own refusal rather than a template-format \
         error, got {reasons:?}"
    );

    // And nothing was left behind.
    let (status, answered) = describe_channel(&router, name).await;
    assert_eq!(
        (status, answered.get("__type").and_then(Value::as_str)),
        (NOT_FOUND_STATUS, Some(RESOURCE_NOT_FOUND)),
        "case {case:?}: expected Kinesis Video Streams to hold no channel named {name:?}, got \
         {status} with {answered}"
    );
}

#[tokio::test]
async fn the_template_format_refusal_is_reserved_for_a_type_cloudformation_does_not_know() {
    // The restrictive half of the same rule: both published types are catalogued and carry the
    // return values their own documentation publishes, while a type under the SAME namespace
    // that CloudFormation publishes nothing for is refused outright.
    for resource_type in [STREAM, CHANNEL] {
        assert!(
            IMPLEMENTED_RESOURCE_TYPES.contains(&resource_type),
            "case {resource_type:?}: expected the type to be one this emulator provisions, the \
             catalogue carries {}",
            IMPLEMENTED_RESOURCE_TYPES.len()
        );
        let published = published_attribute_names(resource_type);
        assert_eq!(
            published,
            vec!["Arn"],
            "case {resource_type:?}: expected the one return value the type publishes, got \
             {published:?}"
        );
    }

    let router = router();
    let unknown = "AWS::KinesisVideo::NotAThing";
    let template = json!({
        "Resources": {"Nope": {"Type": unknown, "Properties": {"Name": "nope"}}}
    })
    .to_string();
    let (status, body) = create_stack(&router, "kvs-unknown-type", &template, &[]).await;
    assert_eq!(
        status, 400,
        "case {unknown:?}: expected HTTP 400 for a type CloudFormation does not know, got \
         {status} with {body}"
    );
    assert!(
        body.contains(&format!("{UNRECOGNIZED_PREFIX}{unknown}]")),
        "case {unknown:?}: expected the recorded template-format refusal, got {body}"
    );
}
