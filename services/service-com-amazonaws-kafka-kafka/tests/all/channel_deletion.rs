// SPDX-License-Identifier: CC0-1.0

//! `DeleteChannel`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from: what the delete does to the stored channel, what the
//! channel's reader reports inside and after the `DELETING` window, and how a
//! delete that resolves no live channel is refused.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request:
//! the rows differ along whatever the rule generalises over (destination kind,
//! tag count, the status the channel is in when the delete arrives, which cluster
//! holds it, how the addressed resource fails to resolve, which operation
//! answers). The dispatch harness — `Emulator`, `Reply`, `pct`, `ClusterRequest`
//! — is the shared one in `all/kafka_harness.rs`; only the `CreateChannel` body,
//! which the channel suites each build for their own rows, is local.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{CHANNEL_DELETION_DELAY, RESOURCE_NOT_FOUND_MESSAGE};

use crate::kafka_harness::{
    assert_channel_name_already_exists, pct, sleep_past_channel_activation, vpc_connection_uri,
    ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// A well-formed channel ARN under the cluster `cluster`/`cluster_id` that no
/// create ever minted. `with_id` appends a channel id after the channel name, the
/// way the service's own records carry one; both forms are channel ARNs, and
/// neither names a channel this cluster ever held.
fn absent_channel_arn(cluster: &str, cluster_id: &str, with_id: bool) -> String {
    let base = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/{cluster}/{cluster_id}/absent");
    if with_id {
        format!("{base}/11111111-2222-4333-8444-555555555555")
    } else {
        base
    }
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

/// One `CreateChannel` body: the required members plus the destination the row
/// delivers to and the tags it carries. The destination and converter values are
/// the ones `tests/all/channel_lifecycle.rs` drives `CreateChannel` with.
fn channel_body(name: &str, iceberg: bool, tags: &[(&str, &str)]) -> Value {
    let mut body = json!({
        "channelName": name,
        "topicConfigurationList": [{
            "recordConverter": { "valueConverter": "JSON" },
            "topicArn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/channels/orders"),
        }],
    });
    if iceberg {
        body["icebergDestinationConfiguration"] = json!({
            "appendOnly": true,
            "deadLetterQueueS3": { "bucketArn": "arn:aws:s3:::msk-channel-dlq" },
            "serviceExecutionRoleArn": "arn:aws:iam::000000000000:role/service-role/msk-channel",
            "destinationTableList": [{
                "destinationDatabaseName": "records",
                "destinationTableName": "orders",
                // `PartitionStrategy` names exactly one value, TIME_HOUR.
                "partitionSpec": {
                    "partitionStrategy": "TIME_HOUR",
                    "sourceList": [{ "sourceName": "event_time" }],
                },
            }],
            "schemaEvolution": { "enableSchemaEvolution": false },
            "tableCreation": { "enableTableCreation": false },
        });
    } else {
        body["s3DestinationConfiguration"] = json!({
            "deadLetterQueueS3": { "bucketArn": "arn:aws:s3:::msk-channel-dlq" },
            "serviceExecutionRoleArn": "arn:aws:iam::000000000000:role/service-role/msk-channel",
            "storage": {
                "bucketArn": "arn:aws:s3:::msk-channel-records",
                "compressionType": "GZIP",
                "storageClass": "STANDARD",
            },
        });
    }
    if !tags.is_empty() {
        body["tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    body
}

/// `CreateChannel` under `cluster_arn`.
async fn create_channel(emulator: &Emulator, cluster_arn: &str, body: Value) -> Reply {
    emulator.create_channel(cluster_arn, body).await
}

/// `DescribeChannel` in the suite's region.
async fn describe_channel(emulator: &Emulator, cluster_arn: &str, channel_arn: &str) -> Reply {
    emulator.describe_channel(cluster_arn, channel_arn).await
}

/// `DeleteChannel`, signed for `region` — the model binds it to
/// `DELETE /v1/clusters/{ClusterArn}/channels/{ChannelArn}`.
async fn delete_channel_in(
    emulator: &Emulator,
    region: &str,
    cluster_arn: &str,
    channel_arn: &str,
) -> Reply {
    emulator
        .call_in(
            region,
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

/// `DeleteChannel` in the suite's region.
async fn delete_channel(emulator: &Emulator, cluster_arn: &str, channel_arn: &str) -> Reply {
    delete_channel_in(emulator, REGION, cluster_arn, channel_arn).await
}

/// `ListTagsForResource`, which resolves any MSK resource ARN.
async fn list_tags(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/tags/{}", pct(arn)), None)
        .await
}

/// Create the channel `body` names under `cluster_arn`, assert it succeeded, and
/// hand back its ARN and the cluster operation the create reported.
async fn create_channel_ok(
    emulator: &Emulator,
    cluster_arn: &str,
    case: &str,
    body: Value,
) -> (String, String) {
    let reply = create_channel(emulator, cluster_arn, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected CreateChannel to return 200, got {} {}",
        reply.status, reply.body
    );
    let arn = reply
        .at("/channelArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateChannel returned no channelArn"))
        .to_string();
    let operation = reply
        .at("/clusterOperationArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateChannel returned no clusterOperationArn"))
        .to_string();
    (arn, operation)
}

/// Sleep past the span the emulator gives a channel's asynchronous delete, so a
/// later read sees the removal the clock now owes it.
fn sleep_past_channel_deletion() {
    std::thread::sleep(CHANNEL_DELETION_DELAY + std::time::Duration::from_millis(250));
}

/// One row of the deletion table: a channel to create and delete, and the status
/// it is in when the delete arrives.
#[derive(Debug, Clone)]
struct DeleteRow {
    name: &'static str,
    iceberg: bool,
    tags: Vec<(&'static str, &'static str)>,
    /// Whether to wait for the channel to reach `ACTIVE` before deleting it, so
    /// the rule is exercised from both statuses a created channel passes through.
    settle_first: bool,
    /// Which of the two clusters the row's channel belongs to.
    second_cluster: bool,
}

impl DeleteRow {
    fn named(name: &'static str) -> Self {
        Self {
            name,
            iceberg: false,
            tags: Vec::new(),
            settle_first: false,
            second_cluster: false,
        }
    }
}

/// RULE: an accepted channel delete moves the channel to the non-terminal
/// `DELETING` status and reports the cluster operation carrying it out, the
/// channel stays readable under that status until the clock owes its removal, and
/// from then on every reader answers as if it never existed. The delete is
/// idempotent: repeating it inside that window reports the same channel and the
/// same operation and cannot postpone the removal.
#[tokio::test]
async fn a_delete_moves_a_channel_through_deleting_and_then_removes_it() {
    let emulator = Emulator::new();
    let first = emulator.create_ok(&ClusterRequest::named("delete-host")).await;
    let second = emulator.create_ok(&ClusterRequest::named("delete-other")).await;

    // The rows differ along everything the rule generalises over: destination
    // kind, tag count, the status the delete arrives in, and which cluster holds
    // the channel.
    let rows = vec![
        DeleteRow::named("s3-creating"),
        DeleteRow {
            iceberg: true,
            ..DeleteRow::named("iceberg-creating")
        },
        DeleteRow {
            settle_first: true,
            tags: vec![("env", "prod"), ("team", "streams")],
            ..DeleteRow::named("s3-active-tagged")
        },
        DeleteRow {
            iceberg: true,
            settle_first: true,
            second_cluster: true,
            ..DeleteRow::named("iceberg-active-elsewhere")
        },
    ];

    let mut deleted: Vec<(DeleteRow, String, String)> = Vec::new();
    for row in &rows {
        let cluster = if row.second_cluster { &second } else { &first };
        let (arn, created_by) = create_channel_ok(
            &emulator,
            cluster,
            row.name,
            channel_body(row.name, row.iceberg, &row.tags),
        )
        .await;
        if row.settle_first {
            sleep_past_channel_activation();
        }

        let reply = delete_channel(&emulator, cluster, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: expected DeleteChannel to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/channelArn").as_str(),
            Some(arn.as_str()),
            "case {row:?}: DeleteChannel reported the wrong channelArn, got {}",
            reply.body
        );
        let deleted_by = reply
            .at("/clusterOperationArn")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {row:?}: DeleteChannel returned no clusterOperationArn, got {}", reply.body)
            })
            .to_string();
        assert!(
            deleted_by.starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/")),
            "case {row:?}: expected a cluster-operation ARN in this region, got {deleted_by:?}"
        );
        assert_ne!(
            deleted_by, created_by,
            "case {row:?}: the delete must report its own operation, not the create's"
        );

        // Inside the window: the channel is still readable, in `DELETING`, and
        // reports the delete as its in-flight operation.
        let described = describe_channel(&emulator, cluster, &arn).await;
        assert_eq!(
            described.status, 200,
            "case {row:?}: expected the channel to stay readable while DELETING, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.at("/status").as_str(),
            Some("DELETING"),
            "case {row:?}: expected status DELETING straight after the delete, got {}",
            described.body
        );
        assert_eq!(
            described.at("/clusterOperationArn").as_str(),
            Some(deleted_by.as_str()),
            "case {row:?}: the reader must report the delete's operation, got {}",
            described.at("/clusterOperationArn")
        );
        // A channel still in the registry still carries its tags, so the tagging
        // readers and the channel's own reader agree about it.
        if !row.tags.is_empty() {
            let tags = list_tags(&emulator, &arn).await;
            assert_eq!(
                tags.status, 200,
                "case {row:?}: a DELETING channel must still report its tags, got {} {}",
                tags.status, tags.body
            );
            for (key, value) in &row.tags {
                assert_eq!(
                    tags.at(&format!("/tags/{key}")).as_str(),
                    Some(*value),
                    "case {row:?}: expected tag {key}={value}, got {}",
                    tags.body
                );
            }
        }

        // Repeating the idempotent delete answers the same thing and changes
        // nothing about when the removal falls due.
        let repeated = delete_channel(&emulator, cluster, &arn).await;
        assert_eq!(
            (repeated.status, repeated.at("/channelArn").as_str()),
            (200, Some(arn.as_str())),
            "case {row:?}: a repeated delete must answer the same channel, got {} {}",
            repeated.status, repeated.body
        );
        assert_eq!(
            repeated.at("/clusterOperationArn").as_str(),
            Some(deleted_by.as_str()),
            "case {row:?}: a repeated delete must report the same operation, got {}",
            repeated.body
        );

        deleted.push((row.clone(), cluster.clone(), arn));
    }

    // One wait for the whole table: every removal above was already due before
    // this returns.
    sleep_past_channel_deletion();

    for (row, cluster, arn) in &deleted {
        let described = describe_channel(&emulator, cluster, arn).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "NotFoundException"),
            "case {row:?}: expected a removed channel to be gone, got {} {}",
            described.status, described.body
        );
        let again = delete_channel(&emulator, cluster, arn).await;
        assert_eq!(
            (again.status, again.error_code()),
            (404, "NotFoundException"),
            "case {row:?}: expected a removed channel's delete to answer 404, got {} {}",
            again.status, again.body
        );
        let tags = list_tags(&emulator, arn).await;
        assert_eq!(
            (tags.status, tags.error_code()),
            (404, "NotFoundException"),
            "case {row:?}: expected a removed channel to carry no tags, got {} {}",
            tags.status, tags.body
        );
    }
}

/// RULE: a channel name identifies at most one channel within its cluster for as
/// long as that channel is in the registry, so the name is taken while the
/// channel is `DELETING` and free once the clock has removed it — and the channel
/// created under it then is a new one, with its own ARN.
#[tokio::test]
async fn a_removed_channels_name_is_free_again() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("name-reuse")).await;

    // Rows differ in destination kind, because the name rule belongs to the
    // channel rather than to where it delivers.
    let rows = [("reuse-s3", false), ("reuse-iceberg", true)];
    for (name, iceberg) in rows {
        let (first, _) =
            create_channel_ok(&emulator, &cluster, name, channel_body(name, iceberg, &[])).await;
        let deleted = delete_channel(&emulator, &cluster, &first).await;
        assert_eq!(
            deleted.status, 200,
            "case {name}: expected DeleteChannel to return 200, got {} {}",
            deleted.status, deleted.body
        );

        // Still in the registry, so the name is still taken.
        let taken = create_channel(&emulator, &cluster, channel_body(name, iceberg, &[])).await;
        assert_channel_name_already_exists(
            &taken,
            &format!("{name} while DELETING"),
            name,
            &first,
        );

        sleep_past_channel_deletion();
        let (second, _) = create_channel_ok(
            &emulator,
            &cluster,
            name,
            channel_body(name, iceberg, &[]),
        )
        .await;
        assert_ne!(
            second, first,
            "case {name}: the channel created after the removal must be a new one"
        );
        let described = describe_channel(&emulator, &cluster, &second).await;
        assert_eq!(
            (described.status, described.at("/status").as_str()),
            (200, Some("CREATING")),
            "case {name}: expected a freshly created channel, got {} {}",
            described.status, described.body
        );
    }
}

/// RULE: a delete acts only on a channel this endpoint's region holds under the
/// cluster named in the request. The cluster is the first gate: a `ClusterArn` that
/// is not a cluster ARN is refused with `BadRequestException` (HTTP 400) and a
/// well-formed one this region holds no cluster for with `NotFoundException`
/// (HTTP 404). Under a cluster that resolves, a `ChannelArn` that cluster never
/// held — malformed, of another cluster, or simply never created — is the caller's
/// own identifier at fault and is refused with `BadRequestException` (HTTP 400).
/// Neither refusal touches the channel that does exist.
#[tokio::test]
async fn a_delete_that_resolves_no_live_channel_is_refused() {
    let emulator = Emulator::new();
    let owner = emulator.create_ok(&ClusterRequest::named("owner")).await;
    let other = emulator.create_ok(&ClusterRequest::named("other")).await;
    let (arn, _) =
        create_channel_ok(&emulator, &owner, "owned", channel_body("owned", false, &[])).await;
    let (owner_name, owner_id) = cluster_identity(&owner);

    // Rows: the right channel under the wrong cluster, a well-formed channel ARN
    // nothing holds, a cluster ARN this region holds nothing for, and each way
    // the two ARN grammars can fail.
    let rows: Vec<(&str, String, String, u16, &str)> = vec![
        (
            // The channel exists, but not under THIS cluster, so this cluster
            // holds no such link.
            "the channel under another cluster",
            other.clone(),
            arn.clone(),
            400,
            "BadRequestException",
        ),
        (
            "a channel ARN nothing holds",
            owner.clone(),
            absent_channel_arn(&owner_name, &owner_id, true),
            400,
            "BadRequestException",
        ),
        (
            // The same, in the form that stops at the channel name: either way
            // this cluster never held a channel under that ARN.
            "a channel ARN nothing holds, without a channel id",
            owner.clone(),
            absent_channel_arn(&owner_name, &owner_id, false),
            400,
            "BadRequestException",
        ),
        (
            "a cluster ARN nothing holds",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
            ),
            absent_channel_arn("absent", "11111111-2222-4333-8444-555555555555-1", true),
            404,
            "NotFoundException",
        ),
        (
            "a channel ARN that is not an ARN",
            owner.clone(),
            "not-an-arn".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "a channel ARN of another service",
            owner.clone(),
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:channel/owner/abc-1/c/9"),
            400,
            "BadRequestException",
        ),
        (
            "a channel ARN missing the channel name",
            owner.clone(),
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/owner/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            "a channel ARN with an empty segment",
            owner.clone(),
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/owner//orders"),
            400,
            "BadRequestException",
        ),
        (
            "a channel ARN with more segments than a channel has",
            owner.clone(),
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/owner/abc-1/orders/id/extra"),
            400,
            "BadRequestException",
        ),
        (
            "a cluster ARN that is not an ARN",
            "not-an-arn".to_string(),
            arn.clone(),
            400,
            "BadRequestException",
        ),
    ];

    for (case, cluster_arn, channel_arn, status, code) in &rows {
        let reply = delete_channel(&emulator, cluster_arn, channel_arn).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case {case}: expected {status} {code} from DeleteChannel, got {} {}",
            reply.status,
            reply.body
        );
    }

    // The same channel, addressed from another region's endpoint: a channel lives
    // in its cluster's region only.
    for region in ["us-east-1", "ap-southeast-2"] {
        let reply = delete_channel_in(&emulator, region, &owner, &arn).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected DeleteChannel to answer 404, got {} {}",
            reply.status,
            reply.body
        );
    }

    // Every refusal above left the channel exactly as it was: still there, and
    // not moving towards removal.
    let described = describe_channel(&emulator, &owner, &arn).await;
    assert_eq!(
        described.status, 200,
        "a refused delete must leave the channel readable, got {} {}",
        described.status, described.body
    );
    assert_ne!(
        described.at("/status").as_str(),
        Some("DELETING"),
        "a refused delete must not have accepted a deletion, got {}",
        described.body
    );
    sleep_past_channel_deletion();
    let still_there = describe_channel(&emulator, &owner, &arn).await;
    assert_eq!(
        (still_there.status, still_there.at("/status").as_str()),
        (200, Some("ACTIVE")),
        "a refused delete must not remove the channel, got {} {}",
        still_there.status, still_there.body
    );
}

/// RULE: whichever operation answers it, a request naming an MSK resource this
/// endpoint's region does not hold is refused with `NotFoundException` and the
/// service's one fixed not-found sentence; the request member at fault is named
/// by `NotFoundException$InvalidParameter`, not by the message.
#[tokio::test]
async fn every_operation_states_the_same_not_found_sentence() {
    let emulator = Emulator::new();
    let cluster = emulator.create_ok(&ClusterRequest::named("wording")).await;
    let (cluster_name, cluster_id) = cluster_identity(&cluster);
    let absent_channel = absent_channel_arn(&cluster_name, &cluster_id, false);
    let absent_cluster = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
    );
    let absent_connection = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/absent/\
         11111111-2222-4333-8444-555555555555"
    );

    // Rows span the operations that resolve a resource by ARN and the kinds of
    // resource they resolve: a channel, a cluster and a VPC connection, read and
    // deleted and created under. The channel rows address a channel under a
    // cluster this region holds nothing for, which is the gate every channel
    // operation answers this sentence from; what `DeleteChannel` answers for a
    // channel reference under a cluster that DOES resolve is its own refusal
    // (`a_delete_that_resolves_no_live_channel_is_refused`).
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "DeleteChannel",
            "DELETE",
            format!(
                "/v1/clusters/{}/channels/{}",
                pct(&absent_cluster),
                pct(&absent_channel)
            ),
            None,
        ),
        (
            "DescribeChannel",
            "GET",
            format!(
                "/v1/clusters/{}/channels/{}",
                pct(&cluster),
                pct(&absent_channel)
            ),
            None,
        ),
        (
            "CreateChannel under a cluster this region does not hold",
            "POST",
            format!("/v1/clusters/{}/channels", pct(&absent_cluster)),
            Some(channel_body("orphan", false, &[])),
        ),
        (
            "DescribeCluster",
            "GET",
            format!("/v1/clusters/{}", pct(&absent_cluster)),
            None,
        ),
        (
            "DeleteCluster",
            "DELETE",
            format!("/v1/clusters/{}", pct(&absent_cluster)),
            None,
        ),
        (
            "DescribeVpcConnection",
            "GET",
            vpc_connection_uri(&absent_connection),
            None,
        ),
        (
            "DeleteVpcConnection",
            "DELETE",
            vpc_connection_uri(&absent_connection),
            None,
        ),
    ];

    for (case, method, uri, body) in &rows {
        let reply = emulator.call(method, uri, body.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (404, "NotFoundException"),
            "case {case}: expected a 404 NotFoundException, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            RESOURCE_NOT_FOUND_MESSAGE,
            "case {case}: expected the service's not-found sentence, got {:?}",
            reply.error_message()
        );
    }
}
