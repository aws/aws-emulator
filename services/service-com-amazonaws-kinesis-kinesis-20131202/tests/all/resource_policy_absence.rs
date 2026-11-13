// SPDX-License-Identifier: CC0-1.0

//! What `GetResourcePolicy` reports for a Kinesis resource that carries NO resource-based
//! policy, end to end over the wire.
//!
//! RULE: `GetResourcePolicy` resolves its `ResourceARN` against the EXISTENCE of the resource
//! only. A resource this account and region hold, in a state the read is available in, is
//! served — HTTP 200 whose required `Policy` member is the policy governing it, which for a
//! resource no statement governs is the empty JSON object document `{}` — and the modelled
//! `ResourceNotFoundException` is reserved for an ARN that resolves to no resource at all. So a
//! never-configured resource and a resource whose policy was detached both read as `{}`, and a
//! read reports that without storing anything: the resource still carries no policy afterwards.
//! (A stream the read is NOT available for is refused before the policy slot is reached at all,
//! which is [`crate::resource_policy_stream_state`]'s.)
//!
//! The recorded flows these were derived from read the policy of ONE freshly created stream
//! that carries none, and re-read it after a detach — each one row of the table below. The rows
//! differ along what the rule generalises over: the resource KIND the ARN names (a data stream,
//! a registered consumer, a channel — every kind
//! `^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$` may name), the lifecycle status the
//! resource is in (still provisioning, ready), how the slot came to be empty (never attached,
//! attached and detached, attached and detached twice over), and the region the request is
//! signed for. The not-found half of the rule is checked over its own table: an ARN naming a
//! resource of a kind that was never created, and one naming a resource of another region.
//!
//! The wire harness, the ARN builders, the channel helpers and the policy helpers are
//! [`crate::stream_tags`]', [`crate::channel_lifecycle`]' and [`crate::resource_policies`]',
//! reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::channel_arn;

use crate::channel_lifecycle::{
    on_demand_stream, past_channel_activation, role_arn, storage_configuration,
    stream_configuration,
};
use crate::resource_policies::{
    account, active_stream, assert_refused, delete, get, policy_for, policy_not_found_says, put,
    reported_policy,
};
use crate::stream_tags::{past_activation, stream_arn, Emulator, REGION};

/// The `Policy` a resource carrying no resource-based policy reads as: the empty JSON object
/// document, spelled out here rather than taken from the handler's own constant — exactly as
/// [`crate::resource_policies::policy_not_found_says`] spells out a refusal's sentence — so a
/// drift of that constant away from the wire value fails these cases.
pub(crate) const EMPTY_POLICY_DOCUMENT: &str = "{}";

/// Assert that reading `arn`'s policy is SERVED and reports the empty document.
async fn assert_reads_as_empty(emulator: &Emulator, case: &str, region: &str, arn: &str) {
    let (status, response) = get(emulator, region, arn).await;
    assert_eq!(
        reported_policy(case, status, &response),
        EMPTY_POLICY_DOCUMENT,
        "case {case}: expected the empty policy document for a resource carrying no policy, \
         got {response}"
    );
}

/// Every kind of resource a `ResourceARN` may name reads as the empty document while it carries
/// no policy: a data stream, a consumer registered with it, and a channel, each read while the
/// resource itself is still provisioning on a stream the read is available for, and again once
/// every transition has fallen due. A stream of its own that is still provisioning is not read
/// at all — the read is not available for it yet, which is its own suite's — and reads as the
/// empty document as soon as it is `ACTIVE`.
#[tokio::test]
async fn a_resource_carrying_no_policy_reads_as_the_empty_document() {
    let emulator = Emulator::new();
    let stream = on_demand_stream(&emulator, "policy-absence-kinds").await;

    let registered = emulator
        .ok(
            "consumer",
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream, "ConsumerName": "absence-consumer" }),
        )
        .await;
    let consumer = registered
        .get("Consumer")
        .and_then(|consumer| consumer.get("ConsumerARN"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the registration must carry a ConsumerARN, got {registered}"))
        .to_string();

    let channel_name = "policy-absence-channel";
    emulator
        .ok(
            "channel",
            "CreateChannel",
            &json!({
                "ChannelName": channel_name,
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(&stream, "JSON", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("policy-absence-bucket"),
                },
            }),
        )
        .await;
    let channel = channel_arn(REGION, channel_name);

    // A stream whose activation has not fallen due yet: still provisioning, so the read is
    // not available for it until it is, and its policy slot is just as empty when it is.
    let provisioning = "policy-absence-provisioning";
    emulator
        .ok(
            provisioning,
            "CreateStream",
            &json!({ "StreamName": provisioning, "ShardCount": 1 }),
        )
        .await;
    let creating_stream = stream_arn(REGION, provisioning);

    let rows: Vec<(&str, &String)> = vec![
        ("an active stream", &stream),
        ("a consumer still provisioning", &consumer),
        ("a channel still provisioning", &channel),
    ];
    for (case, arn) in &rows {
        assert_reads_as_empty(&emulator, case, REGION, arn).await;
    }

    // Once every transition the creates armed has fallen due, the same reads answer the same
    // empty document: being ready attaches no policy — and the stream that was still
    // provisioning reads as the empty document too, now that the read is available for it.
    tokio::time::sleep(past_channel_activation()).await;
    tokio::time::sleep(past_activation()).await;
    for (case, arn) in &rows {
        assert_reads_as_empty(&emulator, &format!("{case}, once ready"), REGION, arn).await;
    }
    assert_reads_as_empty(
        &emulator,
        "the stream that was still provisioning, once ready",
        REGION,
        &creating_stream,
    )
    .await;
}

/// The read is a pure report: answering the empty document for a resource carrying no policy
/// stores nothing, so the stored slot stays empty — the delete still has nothing to remove, a
/// later attach is the resource's FIRST policy, and a detach puts the resource back to reading
/// as the empty document, however many times the cycle runs.
#[tokio::test]
async fn reporting_the_empty_document_stores_no_policy() {
    let emulator = Emulator::new();
    let arn = active_stream(&emulator, "policy-absence-purity").await;

    // Repeated reads of a never-configured resource: each answers the empty document, and
    // nothing is stored for it.
    for round in 1..=3 {
        let case = format!("read {round} of a never-configured resource");
        assert_reads_as_empty(&emulator, &case, REGION, &arn).await;
        let stored = emulator
            .store()
            .lock()
            .resource_policy_by_arn(std::time::SystemTime::now(), REGION, &arn)
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the stream must still be held"));
        assert!(
            stored.is_none(),
            "case {case}: a read must store no policy, got {stored:?}"
        );
    }

    // ... so the delete still finds nothing to remove.
    let (status, response) = delete(&emulator, REGION, &arn).await;
    assert_refused(
        "the delete after reads of an empty slot",
        status,
        &response,
        "ResourceNotFoundException",
        &policy_not_found_says(&arn),
    );

    // Two full attach / detach cycles: the document is reported while it is attached, and the
    // empty document again once it is gone.
    for round in 1..=2 {
        let case = format!("cycle {round}");
        let policy = policy_for(&arn, &format!("Readers{round}"), "kinesis:GetRecords");
        let (status, response) = put(&emulator, REGION, &arn, &policy).await;
        assert_eq!(
            status, 200,
            "case {case}: the attach must be served, got {status} with body {response}"
        );
        let (status, read) = get(&emulator, REGION, &arn).await;
        assert_eq!(
            reported_policy(&case, status, &read),
            policy,
            "case {case}: expected the attached document, got {read}"
        );

        let (status, response) = delete(&emulator, REGION, &arn).await;
        assert_eq!(
            status, 200,
            "case {case}: the detach must be served, got {status} with body {response}"
        );
        assert_reads_as_empty(&emulator, &format!("{case}, after the detach"), REGION, &arn).await;
    }
}

/// A Kinesis resource "is also scoped by Amazon Web Services Region": the empty document is
/// reported for the resource of the region the request was signed for, so one region's stream
/// reads as empty while the other region's stream of the SAME name reports its own attached
/// document, and an ARN of a region whose endpoint was not called names no resource at all.
#[tokio::test]
async fn the_empty_document_belongs_to_the_region_its_resource_lives_in() {
    let emulator = Emulator::new();
    let name = "policy-absence-regional";
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

    // Neither stream carries a policy yet: each region's endpoint reports the empty document
    // for its own stream.
    assert_reads_as_empty(&emulator, "this region's own stream", REGION, &here).await;
    assert_reads_as_empty(&emulator, "the other region's own stream", elsewhere, &there).await;

    // One region's stream takes a document; the other's still reads as empty.
    let attached = policy_for(&there, "There", "kinesis:PutRecord");
    let (status, response) = put(&emulator, elsewhere, &there, &attached).await;
    assert_eq!(
        status, 200,
        "the attach in {elsewhere} must be served, got {status} with body {response}"
    );
    let (status, read) = get(&emulator, elsewhere, &there).await;
    assert_eq!(
        reported_policy("the stream that took a document", status, &read),
        attached,
        "case the stream that took a document in {elsewhere}: expected the document that \
         region's stream carries, got {read}"
    );
    assert_reads_as_empty(
        &emulator,
        "the stream of the other region, still carrying none",
        REGION,
        &here,
    )
    .await;
}

/// The not-found half of the rule: an ARN that resolves to NO resource of this account and
/// region is still the modelled `ResourceNotFoundException`, in the wording for the resource
/// kind that ARN names — the absence of the RESOURCE, never the absence of a policy.
#[tokio::test]
async fn an_arn_naming_no_resource_is_still_refused() {
    let emulator = Emulator::new();
    let held = active_stream(&emulator, "policy-absence-contrast").await;
    let elsewhere = "eu-west-1";

    // (case, the ARN read, the sentence the refusal must carry)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a stream nothing created",
            stream_arn(REGION, "never-created"),
            format!("Stream never-created under account {} not found.", account()),
        ),
        (
            "a stream of another region",
            stream_arn(elsewhere, "policy-absence-contrast"),
            format!(
                "Stream policy-absence-contrast under account {} not found.",
                account()
            ),
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
                 policy-absence-contrast, account {} not found.",
                account()
            ),
        ),
    ];

    for (case, arn, says) in &rows {
        let (status, response) = get(&emulator, REGION, arn).await;
        assert_refused(case, status, &response, "ResourceNotFoundException", says);
    }

    // The stream that DOES exist, read through the very same handler, is served.
    assert_reads_as_empty(&emulator, "the stream this account holds", REGION, &held).await;
}
