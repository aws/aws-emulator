// SPDX-License-Identifier: CC0-1.0

//! What the BUCKET POLICY of a delivery channel's bucket decides about the channel.
//!
//! The rule under test: **Config delivers as its own service principal, so a delivery
//! channel is accepted only while the policy in force on the Amazon S3 bucket it names
//! allows `config.amazonaws.com` each S3 call Config makes on that bucket before it will
//! take the channel — reading the bucket's ACL and writing a delivered object — and a
//! bucket whose policy denies one of them earns `InsufficientDeliveryPolicyException`,
//! worded after the call that was denied, with nothing of the channel persisted.**
//!
//! Config and S3 are mounted on ONE engine (the harness
//! `config_recorded_resource_compliance` builds): the policy is read from S3 itself through
//! the same in-process callback a delivery writes through, so a suite that writes a policy
//! with S3's own `PutBucketPolicy` is writing the document the decision is made from.
//!
//! The rows differ along what the rule generalises over: which of the documented policy's
//! three statements the policy carries, whether one grant covers several actions at once,
//! which principal the grant names, which resource the delivery statement covers (the whole
//! bucket, the account's own Config path, another account's), whether the channel configures
//! a key prefix, and whether the bucket carries a policy at all.

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::{Emulator, ACCOUNT_ID};

/// The bucket every case points its delivery channel at.
const DELIVERY_BUCKET: &str = "config-delivery-bucket";

/// The recorder and the channel every case puts.
const RECORDER_NAME: &str = "the-recorder";
/// See [`RECORDER_NAME`].
const CHANNEL_NAME: &str = "the-channel";

/// The Config service principal a delivery bucket's policy has to name.
pub(crate) const CONFIG_PRINCIPAL: &str = "config.amazonaws.com";

/// The error code Config answers for a bucket whose policy does not allow it to deliver
/// (`PutDeliveryChannel`'s `InsufficientDeliveryPolicyException`).
pub(crate) const INSUFFICIENT_POLICY: &str = "InsufficientDeliveryPolicyException";

/// The three actions the documented delivery policy grants: the bucket-level pair and the
/// object write. Config's own refusals name two of them — the ACL read and the write — so a
/// policy that withholds only `s3:ListBucket` is still taken.
pub(crate) const GET_BUCKET_ACL: &str = "s3:GetBucketAcl";
/// See [`GET_BUCKET_ACL`].
pub(crate) const LIST_BUCKET: &str = "s3:ListBucket";
/// See [`GET_BUCKET_ACL`].
pub(crate) const PUT_OBJECT: &str = "s3:PutObject";

/// The ARN of the delivery bucket, which a statement's `Resource` names.
fn bucket_arn() -> String {
    format!("arn:aws:s3:::{DELIVERY_BUCKET}")
}

/// One `Allow` statement for `principal`, granting `actions` on `resources`, conditioned on
/// the delivering account the way AWS's documented delivery policy conditions its
/// statements.
pub(crate) fn allow(principal: &str, actions: &[&str], resources: &[String]) -> Value {
    json!({
        "Effect": "Allow",
        "Principal": { "Service": principal },
        "Action": actions,
        "Resource": resources,
        "Condition": { "StringEquals": { "AWS:SourceAccount": ACCOUNT_ID } },
    })
}

/// A bucket policy carrying `statements`.
pub(crate) fn policy(statements: Vec<Value>) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// The documented policy, less the statements granting `without`: the rows that drop one
/// action build their policy with it.
fn documented_policy_without(without: &[&str]) -> String {
    let bucket = bucket_arn();
    let object = format!("{bucket}/*");
    let statements: Vec<Value> = [
        (GET_BUCKET_ACL, vec![bucket.clone()]),
        (LIST_BUCKET, vec![bucket.clone()]),
        (PUT_OBJECT, vec![object]),
    ]
    .into_iter()
    .filter(|(action, _)| !without.contains(action))
    .map(|(action, resources)| allow(CONFIG_PRINCIPAL, &[action], &resources))
    .collect();
    policy(statements)
}

/// Put the delivery bucket, with `document` as its policy where the row gives one, and the
/// customer managed configuration recorder a channel needs.
async fn setup(emulator: &Emulator, document: Option<String>) {
    setup_with_bucket(emulator, true, document).await;
}

/// The same, with `bucket` saying whether the delivery bucket is created at all: a bucket S3
/// does not hold answers no policy, which this decision reads as nothing to refuse on.
async fn setup_with_bucket(emulator: &Emulator, bucket: bool, document: Option<String>) {
    if bucket {
        emulator.create_bucket(DELIVERY_BUCKET, &[]).await;
    }
    if let Some(document) = document {
        emulator
            .s3()
            .put_bucket_policy()
            .bucket(DELIVERY_BUCKET)
            .policy(document)
            .send()
            .await
            .unwrap_or_else(|error| panic!("writing the bucket policy must succeed, got {error:?}"));
    }
    emulator
        .config(
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": RECORDER_NAME,
                    "roleARN": format!("arn:aws:iam::{ACCOUNT_ID}:role/config-role"),
                    "recordingGroup": { "allSupported": true },
                }
            }),
        )
        .await;
}

/// The `PutDeliveryChannel` request for a channel delivering into the bucket under
/// `prefix`.
fn channel_request(prefix: Option<&str>) -> Value {
    let mut channel = json!({ "name": CHANNEL_NAME, "s3BucketName": DELIVERY_BUCKET });
    if let Some(prefix) = prefix {
        channel["s3KeyPrefix"] = json!(prefix);
    }
    json!({ "DeliveryChannel": channel })
}

/// The delivery channels Config holds.
async fn held_channels(emulator: &Emulator) -> Vec<Value> {
    let answered = emulator
        .config("DescribeDeliveryChannels", &json!({}))
        .await;
    answered["DeliveryChannels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row of the table: the policy the bucket carries, the prefix the channel configures,
/// and whether the rule says Config must take the channel — and, where it must not, the
/// `unable to …` clause the refusal has to name.
struct Row {
    case: &'static str,
    /// The policy document the bucket carries, or `None` for a bucket carrying none.
    document: fn() -> Option<String>,
    /// The `s3KeyPrefix` the channel configures.
    prefix: Option<&'static str>,
    /// The call the policy denies, as the refusal words it, or `None` where the policy
    /// admits the channel.
    denied: Option<&'static str>,
}

/// The `unable to …` clause the refusal names when the bucket's policy denies the ACL read,
/// and the one it names when the policy denies the object write — the latter carrying the
/// key prefix the channel asked for.
const DENIED_BUCKET_ACL: &str = "get s3 bucket acl";
/// See [`DENIED_BUCKET_ACL`].
fn denied_object_write(prefix: Option<&str>) -> String {
    format!(
        "write to bucket, provided s3 key prefix is '{}'",
        prefix.unwrap_or("null")
    )
}

#[tokio::test]
async fn a_channel_is_taken_only_while_its_buckets_policy_admits_config() {
    let rows: Vec<Row> = vec![
        Row {
            case: "the documented policy, three statements",
            document: || Some(documented_policy_without(&[])),
            prefix: None,
            denied: None,
        },
        Row {
            case: "one statement granting all three actions on the bucket and its objects",
            document: || {
                Some(policy(vec![allow(
                    CONFIG_PRINCIPAL,
                    &[GET_BUCKET_ACL, LIST_BUCKET, PUT_OBJECT],
                    &[bucket_arn(), format!("{}/*", bucket_arn())],
                )]))
            },
            prefix: None,
            denied: None,
        },
        Row {
            case: "the documented policy, with the channel delivering under a prefix",
            document: || Some(documented_policy_without(&[])),
            prefix: Some("audit"),
            denied: None,
        },
        Row {
            case: "a delivery statement covering the account's own Config path",
            document: || {
                let bucket = bucket_arn();
                Some(policy(vec![
                    allow(
                        CONFIG_PRINCIPAL,
                        &[GET_BUCKET_ACL, LIST_BUCKET],
                        std::slice::from_ref(&bucket),
                    ),
                    allow(
                        CONFIG_PRINCIPAL,
                        &[PUT_OBJECT],
                        &[format!("{bucket}/AWSLogs/{ACCOUNT_ID}/Config/*")],
                    ),
                ]))
            },
            prefix: None,
            denied: None,
        },
        Row {
            case: "no existence check, which Config's own checks do not turn on",
            document: || Some(documented_policy_without(&[LIST_BUCKET])),
            prefix: None,
            denied: None,
        },
        Row {
            case: "no permissions check",
            document: || Some(documented_policy_without(&[GET_BUCKET_ACL])),
            prefix: None,
            denied: Some(DENIED_BUCKET_ACL),
        },
        Row {
            case: "no policy at all",
            document: || None,
            prefix: None,
            denied: Some(DENIED_BUCKET_ACL),
        },
        Row {
            case: "every statement granted to another service principal",
            document: || {
                let bucket = bucket_arn();
                Some(policy(vec![allow(
                    "cloudtrail.amazonaws.com",
                    &[GET_BUCKET_ACL, LIST_BUCKET, PUT_OBJECT],
                    &[bucket.clone(), format!("{bucket}/*")],
                )]))
            },
            prefix: None,
            denied: Some(DENIED_BUCKET_ACL),
        },
        Row {
            case: "no delivery statement",
            document: || Some(documented_policy_without(&[PUT_OBJECT])),
            prefix: None,
            denied: Some("write to bucket, provided s3 key prefix is 'null'"),
        },
        Row {
            case: "a delivery statement covering another account's Config path",
            document: || {
                let bucket = bucket_arn();
                Some(policy(vec![
                    allow(
                        CONFIG_PRINCIPAL,
                        &[GET_BUCKET_ACL, LIST_BUCKET],
                        std::slice::from_ref(&bucket),
                    ),
                    allow(
                        CONFIG_PRINCIPAL,
                        &[PUT_OBJECT],
                        &[format!("{bucket}/AWSLogs/999999999999/Config/*")],
                    ),
                ]))
            },
            prefix: None,
            denied: Some("write to bucket, provided s3 key prefix is 'null'"),
        },
        Row {
            case: "a delivery statement that does not reach under the channel's prefix",
            document: || {
                let bucket = bucket_arn();
                Some(policy(vec![
                    allow(
                        CONFIG_PRINCIPAL,
                        &[GET_BUCKET_ACL, LIST_BUCKET],
                        std::slice::from_ref(&bucket),
                    ),
                    allow(
                        CONFIG_PRINCIPAL,
                        &[PUT_OBJECT],
                        &[format!("{bucket}/AWSLogs/{ACCOUNT_ID}/Config/*")],
                    ),
                ]))
            },
            prefix: Some("audit"),
            denied: Some("write to bucket, provided s3 key prefix is 'audit'"),
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        setup(&emulator, (row.document)()).await;

        let Some(denied) = row.denied else {
            emulator
                .config("PutDeliveryChannel", &channel_request(row.prefix))
                .await;
            let held = held_channels(&emulator).await;
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: the channel the policy admits is held, got {held:?}"
            );
            assert_eq!(
                held[0]["s3BucketName"],
                json!(DELIVERY_BUCKET),
                "case {case:?}: the held channel names the bucket it delivers into, got {:?}",
                held[0]
            );
            continue;
        };

        // The refusal is the modelled one, worded after the call the policy denied.
        let (status, body) = emulator
            .call(
                "config",
                "StarlingDoveService",
                "1.1",
                "PutDeliveryChannel",
                &channel_request(row.prefix),
            )
            .await;
        let expected = format!(
            "Insufficient delivery policy to s3 bucket: {DELIVERY_BUCKET}, unable to \
             {denied}."
        );
        let code = body["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<no __type>");
        assert_eq!(
            (status, code, body["message"].as_str()),
            (400, INSUFFICIENT_POLICY, Some(expected.as_str())),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        let held = held_channels(&emulator).await;
        assert!(
            held.is_empty(),
            "case {case:?}: a refused put persists no channel, got {held:?}"
        );
    }
}

/// A channel the policy denies the WRITE for is refused after the ACL read it admits, so
/// the sentence names the write and the key prefix the channel asked for. The rows differ
/// along the prefix the channel configures.
#[tokio::test]
async fn the_refusal_names_the_prefix_the_channel_asked_for() {
    let cases: [(&str, Option<&str>); 2] = [("no prefix", None), ("a prefix", Some("audit"))];

    for (case, prefix) in cases {
        let emulator = Emulator::new();
        setup(&emulator, Some(documented_policy_without(&[PUT_OBJECT]))).await;
        let (status, body) = emulator
            .call(
                "config",
                "StarlingDoveService",
                "1.1",
                "PutDeliveryChannel",
                &channel_request(prefix),
            )
            .await;
        let expected = format!(
            "Insufficient delivery policy to s3 bucket: {DELIVERY_BUCKET}, unable to {}.",
            denied_object_write(prefix)
        );
        assert_eq!(
            (status, body["message"].as_str()),
            (400, Some(expected.as_str())),
            "case {case:?}: expected HTTP 400 and {expected:?}, got {status} with {body}"
        );
    }
}

/// What this decision reads from S3 is the POLICY in force, so a read that establishes no
/// policy grounds no refusal of this call: a channel naming a bucket S3 does not hold, and
/// one naming no bucket at all, are both taken — whether such a channel is refused for the
/// BUCKET's sake is `NoSuchBucketException`'s own requirement, which this decision does not
/// take on. The rows differ along what the channel names.
#[tokio::test]
async fn a_channel_no_policy_can_be_read_for_is_taken() {
    let cases: [(&str, bool, bool); 2] = [
        ("a bucket S3 does not hold", false, true),
        ("no bucket at all", true, false),
    ];

    for (case, bucket, names_bucket) in cases {
        let emulator = Emulator::new();
        setup_with_bucket(&emulator, bucket, None).await;
        let request = if names_bucket {
            channel_request(None)
        } else {
            json!({ "DeliveryChannel": { "name": CHANNEL_NAME } })
        };
        emulator.config("PutDeliveryChannel", &request).await;
        let held = held_channels(&emulator).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: the channel no policy was established for is held, got {held:?}"
        );
        assert_eq!(
            held[0]["name"],
            json!(CHANNEL_NAME),
            "case {case:?}: the held channel is the one the call named, got {:?}",
            held[0]
        );
    }
}
