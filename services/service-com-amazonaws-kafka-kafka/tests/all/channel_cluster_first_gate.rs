// SPDX-License-Identifier: CC0-1.0

//! The ORDER in which a channel-scoped call resolves what its URI names, driven end
//! to end over the same `MultiServiceRouter` the emulator serves from.
//!
//! The rules under test, each asserted over a TABLE rather than one request:
//!
//! 1. the cluster in the request path is the FIRST gate of EVERY channel operation:
//!    when this endpoint's region holds no cluster for a well-formed `ClusterArn`,
//!    every one of `CreateChannel`, `DescribeChannel`, `UpdateChannel`,
//!    `DeleteChannel` and `ListChannels` answers HTTP 404 `NotFoundException` with
//!    the service's one fixed not-found sentence, whatever the `ChannelArn` looks
//!    like — the channel reference is not consulted at all;
//! 2. under a cluster that DOES resolve, a `DeleteChannel` naming a channel that
//!    cluster never held is the caller's own identifier at fault: HTTP 400
//!    `BadRequestException` with MSK's fixed invalid-link-ARN sentence, while a
//!    channel the cluster HELD and the clock has since removed is the fixed
//!    not-found — and a refused delete changes nothing the cluster's channel
//!    listing reports.
//!
//! The rows differ along what the rules generalise over: which operation answers,
//! the shape of the channel reference (well formed with and without a channel id,
//! of another cluster, of another service, not an ARN at all), the ghost cluster's
//! name and resource id, and whether the cluster ever held the channel.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::RESOURCE_NOT_FOUND_MESSAGE;

use crate::kafka_harness::{
    pct, sleep_past_channel_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// MSK's fixed refusal for a channel reference the cluster in the request path
/// holds no channel for (it names a channel a "link" internally). Spelled out here
/// rather than read from the service, so a drift in the service's own wording fails
/// this suite; the apostrophe is U+2019, as the service answers it.
const INVALID_LINK_ARN: &str =
    "The specified link ARN isn\u{2019}t valid. Specify a valid link ARN, and try again.";

/// A `CreateChannel` body with every required member, as the channel suites of this
/// crate build one — a payload the service would otherwise accept.
fn create_body(name: &str) -> Value {
    json!({
        "channelName": name,
        "topicConfigurationList": [{
            "recordConverter": { "valueConverter": "JSON" },
            "topicArn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/channels/orders"),
        }],
        "s3DestinationConfiguration": {
            "deadLetterQueueS3": { "bucketArn": "arn:aws:s3:::msk-channel-dlq" },
            "serviceExecutionRoleArn": "arn:aws:iam::000000000000:role/service-role/msk-channel",
            "dataFreshnessInSeconds": 300,
            "storage": {
                "bucketArn": "arn:aws:s3:::msk-channel-records",
                "compressionType": "GZIP",
                "storageClass": "STANDARD",
            },
        },
    })
}

/// A `CreateChannel` body the service refuses ON ITS OWN MERITS once it gets as far
/// as reading it: it names no destination at all and asks for a
/// `dataFreshnessInSeconds` outside the documented 300..900 range. Used to show the
/// cluster gate runs AHEAD of what the request carries.
fn refusable_create_body(name: &str) -> Value {
    json!({
        "channelName": name,
        "topicConfigurationList": [],
    })
}

/// An `UpdateChannel` body naming the destination kind [`create_body`] creates
/// with, carrying a `DataFreshnessInSeconds` inside the documented 300..900 range —
/// so an update refused here is refused on the URI, not on its payload.
fn update_body() -> Value {
    json!({ "s3DestinationUpdate": { "dataFreshnessInSeconds": 600 } })
}

/// An `UpdateChannel` body the service refuses on its own merits: it names BOTH
/// destination updates, which "Exactly one of icebergDestinationUpdate or
/// s3DestinationUpdate must be supplied" forbids, and an out-of-range freshness with
/// it.
fn refusable_update_body() -> Value {
    json!({
        "s3DestinationUpdate": { "dataFreshnessInSeconds": 10 },
        "icebergDestinationUpdate": { "dataFreshnessInSeconds": 10 },
    })
}

/// The cluster name and resource id carried in `cluster_arn`
/// (`cluster/<name>/<id>`), which a channel ARN of that cluster repeats.
fn cluster_identity(cluster_arn: &str) -> (String, String) {
    let resource = cluster_arn.splitn(6, ':').nth(5).unwrap_or_default();
    let mut parts = resource.split('/').skip(1);
    (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
    )
}

/// A well-formed channel ARN under the cluster `cluster_arn` names that no create
/// ever minted. `with_id` appends a channel id after the channel name, the way this
/// service's own records carry one.
fn never_created_channel_arn(cluster_arn: &str, name: &str, with_id: bool) -> String {
    let (cluster, cluster_id) = cluster_identity(cluster_arn);
    let base =
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/{cluster}/{cluster_id}/{name}");
    if with_id {
        format!("{base}/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee")
    } else {
        base
    }
}

/// The five channel operations, each as (name, method, URI under the cluster and
/// channel the row names, body). `refusable_body` swaps in a payload the service
/// refuses on its own merits, which a cluster that does not resolve must mask.
type ChannelCall = (&'static str, &'static str, String, Option<Value>);

fn channel_calls(cluster_arn: &str, channel_arn: &str, refusable_body: bool) -> Vec<ChannelCall> {
    let channel_uri = format!(
        "/v1/clusters/{}/channels/{}",
        pct(cluster_arn),
        pct(channel_arn)
    );
    vec![
        (
            "CreateChannel",
            "POST",
            format!("/v1/clusters/{}/channels", pct(cluster_arn)),
            Some(if refusable_body {
                refusable_create_body("ghost")
            } else {
                create_body("ghost")
            }),
        ),
        ("DescribeChannel", "GET", channel_uri.clone(), None),
        (
            "UpdateChannel",
            "PUT",
            channel_uri.clone(),
            Some(if refusable_body {
                refusable_update_body()
            } else {
                update_body()
            }),
        ),
        ("DeleteChannel", "DELETE", channel_uri, None),
        (
            "ListChannels",
            "GET",
            format!("/v1/clusters/{}/channels", pct(cluster_arn)),
            None,
        ),
    ]
}

/// `DeleteChannel`: `DELETE /v1/clusters/{ClusterArn}/channels/{ChannelArn}`.
async fn delete_channel(emulator: &Emulator, cluster_arn: &str, channel_arn: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}/channels/{}",
                pct(cluster_arn),
                pct(channel_arn)
            ),
            None,
        )
        .await
}

/// The channel ARNs `ListChannels` reports for `cluster_arn`, which every refusal
/// below must leave exactly as it was.
async fn listed_channels(emulator: &Emulator, cluster_arn: &str, case: &str) -> Vec<String> {
    let reply = emulator.list_channels(cluster_arn, "").await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListChannels on a real cluster to answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/channels")
        .as_array()
        .map(|channels| {
            channels
                .iter()
                .filter_map(|channel| channel["channelArn"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Create the channel `name` under `cluster_arn`, assert it succeeded and hand back
/// its ARN.
async fn create_channel_ok(emulator: &Emulator, cluster_arn: &str, name: &str) -> String {
    let reply = emulator.create_channel(cluster_arn, create_body(name)).await;
    assert_eq!(
        reply.status, 200,
        "case {name}: expected CreateChannel to answer 200, got {} {}",
        reply.status, reply.body
    );
    reply.body["channelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: CreateChannel returned no channelArn"))
        .to_string()
}

/// RULE 1: the cluster named in a channel operation's path is resolved FIRST, so a
/// well-formed `ClusterArn` this endpoint's region holds no cluster for is refused
/// identically by every channel operation — HTTP 404 `NotFoundException` with the
/// service's one fixed not-found sentence — whatever the request's channel
/// reference looks like and whatever its payload carries, including a payload the
/// service would refuse on its own merits once it read it.
#[tokio::test]
async fn every_channel_operation_resolves_its_cluster_before_the_channel_reference() {
    let emulator = Emulator::new();

    // The positive control: a cluster this endpoint really holds answers its
    // channel listing, so a 404 below is about the ghost cluster and not about the
    // route.
    let real = emulator.create_ok(&ClusterRequest::named("gate-control")).await;
    assert!(
        listed_channels(&emulator, &real, "the control cluster")
            .await
            .is_empty(),
        "a fresh cluster holds no channels"
    );

    // Ghost clusters: well formed, of this account and region, never created. The
    // rows differ in the cluster name and in the resource id's shape, including the
    // `-<n>` ordinal suffix this service's own ids carry.
    let ghost_clusters = [
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-2"),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/another-ghost/99999999-8888-4777-8666-555555555555"),
    ];
    // Channel references: each shape a caller can present, including ones that are
    // not channel ARNs at all — none of them may change the answer.
    let channel_refs = [
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/absent/11111111-2222-4333-8444-555555555555-2/ghost/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/absent/11111111-2222-4333-8444-555555555555-2/ghost"),
        "not-an-arn".to_string(),
    ];

    for cluster_arn in &ghost_clusters {
        for channel_arn in &channel_refs {
            // Crossed with the payload: one the service would accept, and one it
            // would refuse on its own merits — a cluster that does not resolve
            // masks both.
            for refusable_body in [false, true] {
                for (operation, method, uri, body) in
                    channel_calls(cluster_arn, channel_arn, refusable_body)
                {
                    let case = format!(
                        "{operation} cluster={cluster_arn} channel={channel_arn} \
                         refusable_body={refusable_body}"
                    );
                    let reply = emulator.call(method, &uri, body).await;
                    assert_eq!(
                        (reply.status, reply.error_code()),
                        (404, "NotFoundException"),
                        "case {case:?}: expected 404 NotFoundException for a cluster this \
                         region does not hold, got {} {}",
                        reply.status,
                        reply.body
                    );
                    assert_eq!(
                        reply.error_message(),
                        RESOURCE_NOT_FOUND_MESSAGE,
                        "case {case:?}: expected the service's one fixed not-found sentence, \
                         got {:?}",
                        reply.error_message()
                    );
                }
            }
        }
    }

    // And the control cluster is untouched by any of it.
    assert!(
        listed_channels(&emulator, &real, "the control cluster after the refusals")
            .await
            .is_empty(),
        "no refusal may register a channel"
    );
}

/// RULE 2: once the cluster resolves, a `DeleteChannel` is judged on the channel
/// reference itself — a reference this cluster never held is HTTP 400
/// `BadRequestException` with MSK's fixed invalid-link-ARN sentence, while a
/// channel this cluster held and the clock has removed is HTTP 404
/// `NotFoundException` with the fixed not-found sentence. Neither changes what the
/// cluster's channel listing reports.
#[tokio::test]
async fn a_delete_under_a_real_cluster_separates_a_bad_reference_from_a_removed_channel() {
    let emulator = Emulator::new();
    let owner = emulator.create_ok(&ClusterRequest::named("link-owner")).await;
    let other = emulator.create_ok(&ClusterRequest::named("link-other")).await;
    // A channel of ANOTHER cluster: it exists, but not under the cluster the rows
    // below address it under.
    let elsewhere = create_channel_ok(&emulator, &other, "owned-elsewhere").await;

    let before = listed_channels(&emulator, &owner, "before the refusals").await;

    // Rows: every way a channel reference can fail to be one of THIS cluster's
    // channels — never created (with and without a channel id), another cluster's,
    // another service's, and not an ARN at all.
    let never_held: Vec<(&str, String)> = vec![
        (
            "a well-formed channel ARN of this cluster that was never created",
            never_created_channel_arn(&owner, "never", true),
        ),
        (
            "the same, without a channel id",
            never_created_channel_arn(&owner, "never", false),
        ),
        ("a channel of another cluster", elsewhere.clone()),
        (
            "a channel ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:channel/link-owner/abc-1/never/9"),
        ),
        ("a value that is not an ARN", "not-an-arn".to_string()),
    ];

    for (case, channel_arn) in &never_held {
        let reply = delete_channel(&emulator, &owner, channel_arn).await;
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (400, "BadRequestException", INVALID_LINK_ARN),
            "case {case:?}: expected the invalid-link-ARN bad request, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("ChannelArn"),
            "case {case:?}: the refusal must name ChannelArn as the parameter at fault, got {}",
            reply.body
        );
    }

    // A channel this cluster DID hold, deleted and then removed by the clock: the
    // same call is now the missing-resource refusal, not a bad reference.
    let held = create_channel_ok(&emulator, &owner, "held-then-removed").await;
    sleep_past_channel_activation();
    let accepted = delete_channel(&emulator, &owner, &held).await;
    assert_eq!(
        accepted.status, 200,
        "the delete of a channel this cluster holds must be accepted, got {} {}",
        accepted.status, accepted.body
    );
    // Poll for the removal the clock owes rather than assuming when it lands: the
    // channel is gone once no reader reports it.
    let mut gone = None;
    for _ in 0..600 {
        let reply = delete_channel(&emulator, &owner, &held).await;
        if reply.status != 200 {
            gone = Some(reply);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let removed = gone.expect("the channel's removal must fall due");
    assert_eq!(
        (removed.status, removed.error_code(), removed.error_message()),
        (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
        "case \"a channel this cluster held and the clock removed\": expected the fixed \
         not-found refusal, got {} {}",
        removed.status,
        removed.body
    );
    // The reader of the same removed channel says the same thing.
    let read = emulator.describe_channel(&owner, &held).await;
    assert_eq!(
        (read.status, read.error_code(), read.error_message()),
        (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
        "case \"a removed channel's reader\": expected the fixed not-found refusal, got {} {}",
        read.status,
        read.body
    );

    // Every refusal above left the listing exactly as it was, and the channel of
    // the other cluster is still there.
    let after = listed_channels(&emulator, &owner, "after the refusals").await;
    assert_eq!(
        after, before,
        "a refused delete must change nothing the listing reports, got {after:?}"
    );
    let elsewhere_listed = listed_channels(&emulator, &other, "the other cluster").await;
    assert_eq!(
        elsewhere_listed,
        vec![elsewhere.clone()],
        "a delete addressed at the wrong cluster must not remove the channel, got \
         {elsewhere_listed:?}"
    );
}
