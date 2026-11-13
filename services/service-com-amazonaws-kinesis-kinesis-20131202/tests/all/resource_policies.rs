// SPDX-License-Identifier: CC0-1.0

//! A Kinesis resource's RESOURCE-BASED POLICY, end to end over the wire:
//! `PutResourcePolicy` -> `GetResourcePolicy` -> `DeleteResourcePolicy`.
//!
//! RULE: a Kinesis resource carries AT MOST ONE resource-based policy, stored on the resource
//! its `ResourceARN` names inside the region the request was signed for. `PutResourcePolicy`
//! attaches the document it carries to that resource, replacing whatever it carried before;
//! `GetResourcePolicy` reports that stored document byte for byte, and reports it without
//! changing anything a later caller sees; `DeleteResourcePolicy` removes it, so the resource
//! carries none again. One lookup resolves the ARN for all three, across every resource kind
//! a `ResourceARN` may name (`^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`) — a data
//! stream, a registered consumer, a channel — so each resource's policy is its own. An ARN
//! this account and region hold no resource under is the modelled `ResourceNotFoundException`
//! for all three; a resource that exists and carries no policy has none for the delete to
//! remove (the same refusal) while the READ is served for it, reporting the empty document —
//! that half of the rule is [`crate::resource_policy_absence`]'s. A `Policy` that is no policy
//! document at all is the modelled `InvalidArgumentException`, and a resource held in a state
//! the operation is not available in is the modelled `ResourceInUseException` — that
//! precondition is [`crate::resource_policy_stream_state`]'s.
//!
//! The recorded flows these were derived from attach ONE document to ONE stream and read it
//! back, and read a stream that carries none — each one row of the tables below. The rows
//! differ along what the rule generalises over: the resource KIND the ARN names, the form of
//! the document (statement count, nesting, non-ASCII text, whitespace), its SIZE (up to the
//! ceiling this emulator retains and one byte past it), how many attaches a resource takes,
//! and the region the request is signed for.
//!
//! The wire harness, the ARN builders and the channel helpers are [`crate::stream_tags`]' and
//! [`crate::channel_lifecycle`]', reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{channel_arn, MAX_RESOURCE_POLICY_BYTES};

use crate::resource_policy_absence::EMPTY_POLICY_DOCUMENT;

use crate::channel_lifecycle::{
    on_demand_stream, past_channel_activation, role_arn, storage_configuration,
    stream_configuration,
};
use crate::stream_tags::{
    error_code, error_message, past_activation, stream_arn, Emulator, REGION,
};

/// The account every resource of this emulator belongs to, which the refusals name.
pub(crate) fn account() -> &'static str {
    engine::EMULATOR_ACCOUNT_ID
}

/// What a `DeleteResourcePolicy` refusal must say when the resource the request named carries
/// no policy to remove: this service's not-found sentence for a policy, spelled out here
/// rather than taken from the handler's own helper so a drift away from the wording fails.
pub(crate) fn policy_not_found_says(arn: &str) -> String {
    format!("Policy for resource {arn} under account {} not found.", account())
}

/// A resource-based policy document: `Version`, one statement granting one action to one
/// principal on `arn`. The shape AWS documents for the member ("It must include the identity
/// of the principal and the actions allowed on this resource. This is formatted as a JSON
/// string", `PutResourcePolicyInput$Policy`).
pub(crate) fn policy_for(arn: &str, sid: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
            "Action": action,
            "Resource": arn,
        }],
    })
    .to_string()
}

/// A policy document whose serialized length is EXACTLY `total` bytes, padded out inside its
/// `Sid` — what the size ceiling is exercised at its bound and one byte past it with.
fn policy_of_bytes(total: usize) -> String {
    let head = r#"{"Version":"2012-10-17","Statement":[{"Sid":""#;
    let tail = r#"","Effect":"Allow","Principal":{"AWS":"*"},"Action":"kinesis:GetRecords"}]}"#;
    let fixed = head.len() + tail.len();
    assert!(
        total >= fixed,
        "a document of {total} bytes cannot carry the {fixed} bytes of fixed text"
    );
    // ASCII padding, so one character is one byte and the document is exactly `total` long.
    format!("{head}{}{tail}", "p".repeat(total - fixed))
}

/// Attach `policy` to `arn`, answering the status and body so a case can assert either.
pub(crate) async fn put(emulator: &Emulator, region: &str, arn: &str, policy: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "PutResourcePolicy",
            &json!({ "ResourceARN": arn, "Policy": policy }),
        )
        .await
}

/// Read the policy attached to `arn`.
pub(crate) async fn get(emulator: &Emulator, region: &str, arn: &str) -> (u16, Value) {
    emulator
        .call_in(region, "GetResourcePolicy", &json!({ "ResourceARN": arn }))
        .await
}

/// Detach the policy attached to `arn`.
pub(crate) async fn delete(emulator: &Emulator, region: &str, arn: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "DeleteResourcePolicy",
            &json!({ "ResourceARN": arn }),
        )
        .await
}

/// The `Policy` a served read reports, asserting the read was served at all.
pub(crate) fn reported_policy(case: &str, status: u16, response: &Value) -> String {
    assert_eq!(
        status, 200,
        "case {case}: expected GetResourcePolicy to answer 200, got {status} with body \
         {response}"
    );
    response
        .get("Policy")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: a served read must carry Policy, got {response}")
        })
        .to_string()
}

/// A refusal of `operation`, asserting its status, its modelled error code and its message.
pub(crate) fn assert_refused(case: &str, status: u16, response: &Value, code: &str, says: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected the modelled refusal, got {status} with body {response}"
    );
    assert_eq!(
        error_code(response),
        code,
        "case {case}: expected {code}, got {response}"
    );
    assert_eq!(
        error_message(response),
        says,
        "case {case}: expected the sentence this service states for that refusal, got \
         {response}"
    );
}

/// An `ACTIVE` stream named `name`, answering its ARN.
pub(crate) async fn active_stream(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    stream_arn(REGION, name)
}

/// One row of the document table: the document attached, and why it is a row.
struct Document {
    label: &'static str,
    policy: String,
}

/// The documents the attach-and-report rule is checked over. The rows differ in statement
/// count, nesting, the script their text is written in, their whitespace, and their size.
fn documents(arn: &str) -> Vec<Document> {
    vec![
        // The recorded shape: one statement granting one action to the account root.
        Document {
            label: "one-statement",
            policy: policy_for(arn, "ReadOnly", "kinesis:GetRecords"),
        },
        // Two statements, each with its own action list.
        Document {
            label: "two-statements",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "Readers",
                        "Effect": "Allow",
                        "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
                        "Action": ["kinesis:GetRecords", "kinesis:GetShardIterator"],
                        "Resource": arn,
                    },
                    {
                        "Sid": "DenyEverythingElse",
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "kinesis:DeleteStream",
                        "Resource": arn,
                    },
                ],
            })
            .to_string(),
        },
        // A nested `Condition` block: the document is stored and reported whole, however
        // deeply it nests.
        Document {
            label: "a-nested-condition",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "FromOneVpcOnly",
                    "Effect": "Allow",
                    "Principal": { "AWS": "*" },
                    "Action": "kinesis:PutRecord",
                    "Resource": arn,
                    "Condition": {
                        "StringEquals": { "aws:SourceVpce": ["vpce-0a1b2c3d", "vpce-4e5f6a7b"] },
                    },
                }],
            })
            .to_string(),
        },
        // Non-ASCII text inside the document round-trips unchanged.
        Document {
            label: "non-ascii-text",
            policy: policy_for(arn, "読み取り専用-Политика", "kinesis:GetRecords"),
        },
        // The document is reported BYTE FOR BYTE, so a pretty-printed one keeps its
        // newlines and indentation rather than being re-rendered.
        Document {
            label: "pretty-printed-whitespace",
            policy: format!(
                "{{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {{\n      \
                 \"Effect\": \"Allow\",\n      \"Principal\": {{ \"AWS\": \"*\" }},\n      \
                 \"Action\": \"kinesis:ListShards\",\n      \"Resource\": \"{arn}\"\n    }}\n  \
                 ]\n}}\n"
            ),
        },
        // The whole ceiling this emulator retains, exactly at its bound.
        Document {
            label: "the-whole-size-ceiling",
            policy: policy_of_bytes(MAX_RESOURCE_POLICY_BYTES),
        },
    ]
}

/// Every document of the table is attached to one stream in turn, and each read reports the
/// LAST document attached, byte for byte: one policy per resource, replaced by the next
/// attach. The read is pure, and re-attaching the same document changes nothing.
#[tokio::test]
async fn an_attached_policy_is_reported_back_byte_for_byte() {
    let emulator = Emulator::new();
    let arn = active_stream(&emulator, "policy-document-forms").await;

    for document in documents(&arn) {
        let case = document.label;
        let (status, response) = put(&emulator, REGION, &arn, &document.policy).await;
        assert_eq!(
            status, 200,
            "case {case}: expected PutResourcePolicy to answer 200, got {status} with body \
             {response}"
        );

        let (status, read) = get(&emulator, REGION, &arn).await;
        assert_eq!(
            reported_policy(case, status, &read),
            document.policy,
            "case {case}: expected the document the attach carried, got {read}"
        );

        // The read changes nothing: a second one answers the same document.
        let (status, again) = get(&emulator, REGION, &arn).await;
        assert_eq!(
            reported_policy(case, status, &again),
            document.policy,
            "case {case}: expected a second read to report the same document, got {again}"
        );

        // Attaching the very same document again is served and leaves the same document.
        let (status, _) = put(&emulator, REGION, &arn, &document.policy).await;
        assert_eq!(
            status, 200,
            "case {case}: expected re-attaching the same document to be served, got {status}"
        );
        let (status, unchanged) = get(&emulator, REGION, &arn).await;
        assert_eq!(
            reported_policy(case, status, &unchanged),
            document.policy,
            "case {case}: expected the document to be unchanged, got {unchanged}"
        );
    }
}

/// Each resource holds its OWN policy, whichever kind its `ResourceARN` names: a stream, a
/// consumer registered with it, and a channel each take a different document, and each read
/// reports that resource's own. A policy is metadata of the resource, so it is attached and
/// reported while the resource is still provisioning and once it is ready.
#[tokio::test]
async fn every_resource_kind_holds_its_own_policy() {
    let emulator = Emulator::new();
    let stream = on_demand_stream(&emulator, "policy-resource-kinds").await;

    // A consumer registered with that stream: its ARN is the stream's, extended.
    let registered = emulator
        .ok(
            "consumer",
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream, "ConsumerName": "policy-consumer" }),
        )
        .await;
    let consumer = registered
        .get("Consumer")
        .and_then(|consumer| consumer.get("ConsumerARN"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the registration must carry a ConsumerARN, got {registered}"))
        .to_string();

    // A channel reading from that stream.
    let channel_name = "policy-channel";
    emulator
        .ok(
            "channel",
            "CreateChannel",
            &json!({
                "ChannelName": channel_name,
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(&stream, "JSON", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("policy-channel-bucket"),
                },
            }),
        )
        .await;
    let channel = channel_arn(REGION, channel_name);

    // Each resource takes its own document, while the consumer and the channel are both
    // still in CREATING.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "stream",
            stream.clone(),
            policy_for(&stream, "StreamPolicy", "kinesis:PutRecord"),
        ),
        (
            "consumer",
            consumer.clone(),
            policy_for(&consumer, "ConsumerPolicy", "kinesis:SubscribeToShard"),
        ),
        (
            "channel",
            channel.clone(),
            policy_for(&channel, "ChannelPolicy", "kinesis:DescribeChannel"),
        ),
    ];

    for (case, arn, policy) in &rows {
        let (status, response) = put(&emulator, REGION, arn, policy).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the attach to be served while the resource provisions, \
             got {status} with body {response}"
        );
    }

    // Every read reports the document of the resource it named, before and after the
    // transitions the creates armed fall due.
    for (case, arn, policy) in &rows {
        let (status, read) = get(&emulator, REGION, arn).await;
        assert_eq!(
            &reported_policy(case, status, &read),
            policy,
            "case {case}: expected this resource's own document, got {read}"
        );
    }
    tokio::time::sleep(past_channel_activation()).await;
    for (case, arn, policy) in &rows {
        let (status, read) = get(&emulator, REGION, arn).await;
        assert_eq!(
            &reported_policy(case, status, &read),
            policy,
            "case {case}: expected the same document once the resource is ready, got {read}"
        );
    }

    // Detaching one resource's policy leaves every other resource's policy alone.
    let (status, response) = delete(&emulator, REGION, &consumer).await;
    assert_eq!(
        status, 200,
        "case consumer: expected DeleteResourcePolicy to answer 200, got {status} with body \
         {response}"
    );
    let (status, gone) = get(&emulator, REGION, &consumer).await;
    assert_eq!(
        reported_policy("consumer after the detach", status, &gone),
        EMPTY_POLICY_DOCUMENT,
        "case consumer after the detach: expected the empty document, got {gone}"
    );
    for (case, arn, policy) in rows.iter().filter(|(case, _, _)| *case != "consumer") {
        let (status, read) = get(&emulator, REGION, arn).await;
        assert_eq!(
            &reported_policy(case, status, &read),
            policy,
            "case {case}: expected another resource's detach to leave this one alone, got \
             {read}"
        );
    }
}

/// A resource that carries no policy has none for the DELETE to remove — before any attach,
/// and again after a detach — and an ARN this account and region hold no resource under names
/// nothing at all, for the read, the delete and the attach alike. Every one of them is the
/// modelled `ResourceNotFoundException`, in the wording for what was not found. (What the
/// READ answers for a resource carrying no policy is [`crate::resource_policy_absence`]'s.)
#[tokio::test]
async fn an_absent_policy_is_refused_by_the_delete() {
    let emulator = Emulator::new();
    let held = active_stream(&emulator, "policy-absence").await;

    // Before any attach: nothing to delete.
    let (status, response) = delete(&emulator, REGION, &held).await;
    assert_refused(
        "delete before any attach",
        status,
        &response,
        "ResourceNotFoundException",
        &policy_not_found_says(&held),
    );

    // Attached, detached, and absent again: the detach is served once, and a second detach
    // of the same resource has nothing left to remove.
    let policy = policy_for(&held, "Readers", "kinesis:GetRecords");
    let (status, _) = put(&emulator, REGION, &held, &policy).await;
    assert_eq!(status, 200, "the attach must be served, got {status}");
    let (status, response) = delete(&emulator, REGION, &held).await;
    assert_eq!(
        status, 200,
        "the detach must be served, got {status} with body {response}"
    );
    let (status, response) = delete(&emulator, REGION, &held).await;
    assert_refused(
        "delete after the detach",
        status,
        &response,
        "ResourceNotFoundException",
        &policy_not_found_says(&held),
    );

    // An ARN naming no resource of this account and region: refused with the sentence this
    // service states for the resource KIND that ARN names, not with the policy one.
    let elsewhere = "eu-west-1";
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a stream nothing created",
            stream_arn(REGION, "never-created"),
            format!("Stream never-created under account {} not found.", account()),
        ),
        (
            "a stream of another region",
            stream_arn(elsewhere, "policy-absence"),
            format!("Stream policy-absence under account {} not found.", account()),
        ),
        (
            "a channel nothing created",
            channel_arn(REGION, "never-created"),
            format!("Channel never-created under account {} not found.", account()),
        ),
        (
            "a consumer nothing registered",
            format!("{held}/consumer/never-registered:1700000000"),
            format!(
                "Consumer never-registered with creation timestamp 1700000000 under stream: \
                 policy-absence, account {} not found.",
                account()
            ),
        ),
    ];
    for (case, arn, says) in &rows {
        let (status, read) = get(&emulator, REGION, arn).await;
        assert_refused(case, status, &read, "ResourceNotFoundException", says);
        let (status, removed) = delete(&emulator, REGION, arn).await;
        assert_refused(
            &format!("{case}: the delete"),
            status,
            &removed,
            "ResourceNotFoundException",
            says,
        );
        let (status, attached) = put(
            &emulator,
            REGION,
            arn,
            &policy_for(arn, "Readers", "kinesis:GetRecords"),
        )
        .await;
        assert_refused(
            &format!("{case}: the attach"),
            status,
            &attached,
            "ResourceNotFoundException",
            says,
        );
    }
}

/// A `Policy` that is no policy document is refused with the modelled
/// `InvalidArgumentException`, and the resource keeps whatever it carried: a value that is not
/// JSON at all, a JSON value that is not an object, and a document one byte past the size
/// ceiling this emulator retains.
///
/// The document check states ONE fixed sentence, which quotes neither the document, nor an
/// offset, nor a member name — so every form of document that is not a policy is refused with
/// the very same text.
#[tokio::test]
async fn a_policy_that_is_no_policy_document_is_refused() {
    let emulator = Emulator::new();
    let arn = active_stream(&emulator, "policy-document-refusals").await;
    let kept = policy_for(&arn, "Readers", "kinesis:GetRecords");
    let (status, _) = put(&emulator, REGION, &arn, &kept).await;
    assert_eq!(status, 200, "the attach of the kept document must be served");

    let oversized = policy_of_bytes(MAX_RESOURCE_POLICY_BYTES + 1);
    // The sentence the service states for a document that is no policy document, written out
    // here rather than borrowed from the crate so the wording itself is what the cases pin.
    let invalid_json = "Policy validation error: This policy contains invalid Json".to_string();
    // (case, the value sent, what the refusal must say)
    let rows: Vec<(&str, String, String)> = vec![
        ("not json at all", "Allow everyone to read".to_string(), invalid_json.clone()),
        (
            "truncated json",
            r#"{"Version":"2012-10-17","Statement":["#.to_string(),
            invalid_json.clone(),
        ),
        (
            "a json array",
            r#"[{"Effect":"Allow"}]"#.to_string(),
            invalid_json.clone(),
        ),
        ("a json string", r#""Allow""#.to_string(), invalid_json.clone()),
        ("a json number", "2012".to_string(), invalid_json.clone()),
        (
            "one byte past the size ceiling",
            oversized.clone(),
            format!(
                "Policy must be at most {MAX_RESOURCE_POLICY_BYTES} bytes; the document sent \
                 is {} bytes.",
                oversized.len()
            ),
        ),
    ];

    for (case, policy, says) in &rows {
        let (status, response) = put(&emulator, REGION, &arn, policy).await;
        assert_refused(
            case,
            status,
            &response,
            "InvalidArgumentException",
            says,
        );

        // A refused attach stores nothing: the document the resource carried is still there.
        let (status, read) = get(&emulator, REGION, &arn).await;
        assert_eq!(
            reported_policy(case, status, &read),
            kept,
            "case {case}: a refused attach must leave the stored document alone, got {read}"
        );
    }
}

/// A Kinesis resource "is also scoped by Amazon Web Services Region": a policy is attached to
/// the resource of the region the request was signed for, so the same ARN names no resource
/// from another region's endpoint, and two streams of one name in two regions hold two
/// policies.
#[tokio::test]
async fn a_policy_belongs_to_the_region_its_resource_lives_in() {
    let emulator = Emulator::new();
    let name = "regional-policy";
    let elsewhere = "eu-west-1";

    for region in [REGION, elsewhere] {
        emulator
            .ok_in(
                name,
                region,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    let here = stream_arn(REGION, name);
    let there = stream_arn(elsewhere, name);
    let here_policy = policy_for(&here, "Here", "kinesis:GetRecords");
    let there_policy = policy_for(&there, "There", "kinesis:PutRecord");

    let (status, _) = put(&emulator, REGION, &here, &here_policy).await;
    assert_eq!(status, 200, "the attach in {REGION} must be served");
    let (status, _) = put(&emulator, elsewhere, &there, &there_policy).await;
    assert_eq!(status, 200, "the attach in {elsewhere} must be served");

    let (status, read) = get(&emulator, REGION, &here).await;
    assert_eq!(
        reported_policy(REGION, status, &read),
        here_policy,
        "case {REGION}: expected the document of the stream this region holds, got {read}"
    );
    let (status, read) = get(&emulator, elsewhere, &there).await;
    assert_eq!(
        reported_policy(elsewhere, status, &read),
        there_policy,
        "case {elsewhere}: expected the document of the stream that region holds, got {read}"
    );

    // One region's endpoint holds no resource under the other region's ARN at all.
    let (status, response) = get(&emulator, REGION, &there).await;
    assert_refused(
        "another region's ARN",
        status,
        &response,
        "ResourceNotFoundException",
        &format!("Stream {name} under account {} not found.", account()),
    );
}
