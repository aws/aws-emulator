// SPDX-License-Identifier: CC0-1.0

//! The asynchronous MSK lifecycle SEQUENCES, walked end to end over the same
//! `MultiServiceRouter` the emulator serves from.
//!
//! RULE: every asynchronous state one of these sequences walks reaches its
//! TERMINAL value within one clock-owed span of the call that started it, and the
//! cluster operation such a call hands back reaches its `*_COMPLETE` state at that
//! same instant — so the FIRST read a client makes after that span reports the
//! terminal value, whatever cadence the client polls at, instead of a resource that
//! never settles and a poll that never ends.
//!
//! Each suite here is one of those sequences, step by step: after every write the
//! transient value is read back, the span is waited out ONCE, and the terminal
//! value is asserted on the next read. Nothing here polls: the spans are the
//! service's own published constants, so a suite cannot drift from the service, and
//! a step that needed a second span would fail here.
//!
//! The suites differ along the resource family whose state machine they walk (VPC
//! connection, replicator, channel, cluster operation) rather than repeating one
//! recorded request, and each one asserts the refusals that sit on the same path,
//! because a client that cannot get past a refusal never reaches the next state
//! either.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::vpc_connection_admission::CLIENT_SUBNET_COUNT_MESSAGE;
use service_com_amazonaws_kafka_kafka::state::{
    unassumable_service_execution_role_message, CONFIGURATION_IN_USE_MESSAGE,
    KAFKA_SERVICE_PRINCIPAL, REPLICATOR_ACTIVATION_DELAY, REPLICATOR_VERSION_NOT_CURRENT_MESSAGE,
    RESOURCE_NOT_FOUND_MESSAGE, TOPIC_ACTIVATION_DELAY, TOPIC_REPLICATION_DELAY,
};

use crate::kafka_harness::{
    given_iam_role_trusting, pct, sleep_past_activation, sleep_past_channel_activation,
    sleep_past_channel_update, sleep_past_configuration_deletion, sleep_past_update,
    sleep_past_vpc_connection_activation, sleep_past_vpc_connection_rejection, trust_policy_for,
    ClusterRequest, ConnectionRequest, Emulator, Reply, ACCOUNT,
};

/// A role ARN passed to the service, which stores it and reports it back.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/msk-lifecycle";
/// The destination bucket the channel rows deliver to.
const RECORDS_BUCKET_ARN: &str = "arn:aws:s3:::msk-lifecycle-records";

/// Sleep past the span a replicator takes to leave `CREATING`/`UPDATING`, the
/// service's own published constant.
fn sleep_past_replicator_span() {
    std::thread::sleep(REPLICATOR_ACTIVATION_DELAY + std::time::Duration::from_millis(250));
}

/// Sleep past the span a topic takes to become `ACTIVE`.
fn sleep_past_topic_span() {
    std::thread::sleep(TOPIC_ACTIVATION_DELAY + std::time::Duration::from_millis(250));
}

/// Sleep past the span a replicated topic takes to appear on its target cluster.
fn sleep_past_replication_span() {
    std::thread::sleep(TOPIC_REPLICATION_DELAY + std::time::Duration::from_millis(250));
}

/// The cluster's `CurrentVersion`, which every update quotes.
async fn current_version(emulator: &Emulator, cluster: &str) -> String {
    let described = emulator.describe(cluster).await;
    described
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("DescribeCluster reported no currentVersion: {}", described.body))
        .to_string()
}

/// The cluster's `State`.
async fn cluster_state(emulator: &Emulator, cluster: &str) -> String {
    let described = emulator.describe(cluster).await;
    described
        .at("/clusterInfo/state")
        .as_str()
        .unwrap_or_else(|| panic!("DescribeCluster reported no state: {}", described.body))
        .to_string()
}

/// `DescribeClusterOperation`'s `OperationState` for `operation`, read through the
/// V1 reader.
async fn operation_state(emulator: &Emulator, operation: &str) -> String {
    let reply = emulator
        .call("GET", &format!("/v1/operations/{}", pct(operation)), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "expected DescribeClusterOperation({operation}) to resolve, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/clusterOperationInfo/operationState")
        .as_str()
        .unwrap_or_else(|| panic!("the operation carries no operationState: {}", reply.body))
        .to_string()
}

/// The same operation through the V2 reader, so both readers of one record are
/// asserted to agree about whether the work has finished.
async fn operation_state_v2(emulator: &Emulator, operation: &str) -> String {
    let reply = emulator
        .call("GET", &format!("/api/v2/operations/{}", pct(operation)), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "expected DescribeClusterOperationV2({operation}) to resolve, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/clusterOperationInfo/operationState")
        .as_str()
        .unwrap_or_else(|| panic!("the V2 operation carries no operationState: {}", reply.body))
        .to_string()
}

/// Assert both readers report `expected` for `operation` — the FIRST read after
/// the span, so an operation that needed a second probe fails here.
async fn assert_operation_state(emulator: &Emulator, operation: &str, expected: &str, case: &str) {
    let v1 = operation_state(emulator, operation).await;
    assert_eq!(
        v1, expected,
        "case {case}: expected OperationState {expected}, got {v1}"
    );
    let v2 = operation_state_v2(emulator, operation).await;
    assert_eq!(
        v2, expected,
        "case {case}: expected the V2 reader to agree on {expected}, got {v2}"
    );
}

/// `UpdateConnectivity`, turning multi-VPC private connectivity on with SASL/IAM.
async fn update_connectivity(emulator: &Emulator, cluster: &str, version: &str) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/connectivity", pct(cluster)),
            Some(json!({
                "currentVersion": version,
                "connectivityInfo": {
                    "vpcConnectivity": {
                        "clientAuthentication": { "sasl": { "iam": { "enabled": true } } },
                    },
                },
            })),
        )
        .await
}

/// The `clusterOperationArn` a mutation hands its caller back.
fn operation_arn(reply: &Reply, case: &str) -> String {
    reply
        .at("/clusterOperationArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the mutation handed back no clusterOperationArn: {}", reply.body))
        .to_string()
}

/// RULE, for the multi-VPC private-connectivity family: the `UpdateConnectivity`
/// operation reaches `UPDATE_COMPLETE` and the cluster `ACTIVE` within one update
/// span, a created VPC connection reaches `AVAILABLE` within one connection span,
/// a rejected one reaches `REJECTED` and leaves the cluster's active connections,
/// and a deleted one is answered `NotFoundException` by every later caller — each
/// observed on the FIRST read after its own span.
#[tokio::test]
async fn the_vpc_connection_sequence_settles_at_every_step() {
    let emulator = Emulator::new();
    let cluster = emulator.vpc_connection_target("vpc-sequence").await;
    assert_eq!(
        cluster_state(&emulator, &cluster).await,
        "ACTIVE",
        "the first read after the activation span must report the cluster ACTIVE"
    );

    // Step 1: multi-VPC connectivity, whose operation the client polls.
    let version = current_version(&emulator, &cluster).await;
    let accepted = update_connectivity(&emulator, &cluster, &version).await;
    assert_eq!(
        accepted.status, 200,
        "expected UpdateConnectivity to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    let operation = operation_arn(&accepted, "UpdateConnectivity");
    assert_operation_state(
        &emulator,
        &operation,
        "UPDATE_IN_PROGRESS",
        "the connectivity update inside its span",
    )
    .await;
    sleep_past_update();
    assert_operation_state(
        &emulator,
        &operation,
        "UPDATE_COMPLETE",
        "the connectivity update once its span is over",
    )
    .await;
    assert_eq!(
        cluster_state(&emulator, &cluster).await,
        "ACTIVE",
        "the cluster must be ACTIVE again on the first read after the update span"
    );

    // Step 2: the connection itself, which the client polls to AVAILABLE.
    let request = ConnectionRequest::to(&cluster);
    let created = emulator.create_vpc_connection(&request).await;
    assert_eq!(
        (created.status, created.at("/state").as_str()),
        (200, Some("CREATING")),
        "expected CreateVpcConnection to report CREATING, got {} {}",
        created.status,
        created.body
    );
    let connection = created
        .at("/vpcConnectionArn")
        .as_str()
        .expect("CreateVpcConnection returned no vpcConnectionArn")
        .to_string();
    sleep_past_vpc_connection_activation();
    let described = emulator.describe_vpc_connection(&connection).await;
    assert_eq!(
        (described.at("/state").as_str(), described.at("/vpcId").as_str()),
        (Some("AVAILABLE"), Some(request.vpc_id)),
        "the first read after the connection span must report AVAILABLE for the submitted \
         VPC, got {}",
        described.body
    );

    // Step 3: the reject transition, and the connection leaving the cluster's
    // active connections.
    let rejected = emulator
        .reject_client_vpc_connection(&cluster, &connection)
        .await;
    assert_eq!(
        rejected.status, 200,
        "expected RejectClientVpcConnection to be accepted, got {} {}",
        rejected.status, rejected.body
    );
    sleep_past_vpc_connection_rejection();
    let described = emulator.describe_vpc_connection(&connection).await;
    assert_eq!(
        described.at("/state").as_str(),
        Some("REJECTED"),
        "the first read after the rejection span must report REJECTED, got {}",
        described.body
    );
    let listed = emulator.list_client_vpc_connections(&cluster, "").await;
    let active: Vec<String> = listed
        .at("/clientVpcConnections")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry["state"].as_str() != Some("REJECTED"))
        .map(|entry| entry["vpcConnectionArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !active.contains(&connection),
        "a rejected connection must not be reported as an active one, got {active:?}"
    );

    // Step 4: the delete, which every later caller is answered the same way about.
    let deleted = emulator.delete_vpc_connection(&connection).await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteVpcConnection to be accepted, got {} {}",
        deleted.status, deleted.body
    );
    for (case, reply) in [
        ("DescribeVpcConnection", emulator.describe_vpc_connection(&connection).await),
        ("a repeated DeleteVpcConnection", emulator.delete_vpc_connection(&connection).await),
    ] {
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
            "case {case}: expected the deleted connection to be answered as absent, got {} {}",
            reply.status,
            reply.body
        );
    }

    // And the refusal on the same path: a placement that does not span the
    // cluster's own client subnets, over every count on either side of it.
    let placed = ClusterRequest::named("x").subnets.len();
    for count in [1usize, 2, placed + 1, placed + 2] {
        let subnets: Vec<&'static str> = ["subnet-0a1b2c30", "subnet-0a1b2c31", "subnet-0a1b2c32",
            "subnet-0a1b2c33", "subnet-0a1b2c34"][..count]
            .to_vec();
        let refused = emulator
            .create_vpc_connection(&ConnectionRequest {
                client_subnets: subnets,
                ..ConnectionRequest::to(&cluster)
            })
            .await;
        assert_eq!(
            (refused.status, refused.error_code(), refused.error_message()),
            (400, "BadRequestException", CLIENT_SUBNET_COUNT_MESSAGE),
            "case {count} client subnet(s) against {placed} cluster subnets: expected the \
             placement to be refused, got {} {}",
            refused.status,
            refused.body
        );
    }
    let left = emulator.list_vpc_connections("").await;
    assert_eq!(
        left.at("/vpcConnections").as_array().map(Vec::len),
        Some(0),
        "a refused create must leave no connection behind, got {}",
        left.body
    );
}

/// A `CreateReplicator` body over one flow, with the topics `topics_to_replicate`
/// selects.
fn replicator_body(
    name: &str,
    source: &str,
    target: &str,
    topics_to_replicate: &[&str],
    role: &str,
) -> Value {
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": role,
        "kafkaClusters": [
            {
                "amazonMskCluster": { "mskClusterArn": source },
                "vpcConfig": { "subnetIds": ["subnet-a", "subnet-b", "subnet-c"] },
            },
            {
                "amazonMskCluster": { "mskClusterArn": target },
                "vpcConfig": { "subnetIds": ["subnet-a", "subnet-b", "subnet-c"] },
            },
        ],
        "replicationInfoList": [{
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "targetCompressionType": "NONE",
            "topicReplication": { "topicsToReplicate": topics_to_replicate },
            "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
        }],
    })
}

/// RULE, for the replicator family: a created replicator reaches `RUNNING` within
/// one replicator span, the topics its flow selects appear on the target cluster
/// within one replication span, an accepted `UpdateReplicationInfo` is back to
/// `RUNNING` within one more span with the new configuration readable, and a
/// deleted replicator is answered as absent — each on the FIRST read after its own
/// span.
#[tokio::test]
async fn the_replicator_sequence_settles_at_every_step() {
    let emulator = Emulator::new();
    let source = emulator.create_ok(&ClusterRequest::named("repl-source")).await;
    let target = emulator.create_ok(&ClusterRequest::named("repl-target")).await;
    sleep_past_activation();

    let created = emulator
        .call(
            "POST",
            "/replication/v1/replicators",
            Some(replicator_body(
                "sequence",
                &source,
                &target,
                &["orders.*"],
                ROLE_ARN,
            )),
        )
        .await;
    assert_eq!(
        (created.status, created.at("/replicatorState").as_str()),
        (200, Some("CREATING")),
        "expected CreateReplicator to report CREATING, got {} {}",
        created.status,
        created.body
    );
    let replicator = created
        .at("/replicatorArn")
        .as_str()
        .expect("CreateReplicator returned no replicatorArn")
        .to_string();

    sleep_past_replicator_span();
    let described = emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(&replicator)),
            None,
        )
        .await;
    assert_eq!(
        described.at("/replicatorState").as_str(),
        Some("RUNNING"),
        "the first read after the replicator span must report RUNNING, got {}",
        described.body
    );

    // A topic created on the source after the flow is running appears on the
    // target under the source cluster's alias.
    let topic = emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(&source)),
            Some(json!({ "topicName": "orders", "partitionCount": 1, "replicationFactor": 1 })),
        )
        .await;
    assert_eq!(
        topic.status, 200,
        "expected CreateTopic to be accepted, got {} {}",
        topic.status, topic.body
    );
    sleep_past_topic_span();
    sleep_past_replication_span();
    let listed = emulator
        .call("GET", &format!("/v1/clusters/{}/topics", pct(&target)), None)
        .await;
    let names: Vec<String> = listed
        .at("/topics")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["topicName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        names.iter().any(|name| name.ends_with(".orders")),
        "the first read after the replication span must report the replicated topic, got \
         {names:?}"
    );

    // The optimistic lock on the same path: a stale version is refused in the
    // service's fixed sentence, with no version value in it.
    let version = described
        .at("/currentVersion")
        .as_str()
        .expect("DescribeReplicator reported no currentVersion")
        .to_string();
    let update_body = |pattern: &str, quoted: &str| {
        json!({
            "currentVersion": quoted,
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "topicReplication": {
                "copyAccessControlListsForTopics": false,
                "copyTopicConfigurations": false,
                "detectAndCopyNewTopics": false,
                "topicsToExclude": [],
                "topicsToReplicate": [pattern],
            },
        })
    };
    let stale = emulator
        .call(
            "PUT",
            &format!("/replication/v1/replicators/{}/replication-info", pct(&replicator)),
            Some(update_body("payments.*", "K0000000000000")),
        )
        .await;
    assert_eq!(
        (stale.status, stale.error_code(), stale.error_message()),
        (400, "BadRequestException", REPLICATOR_VERSION_NOT_CURRENT_MESSAGE),
        "a stale currentVersion must be refused in the service's fixed sentence, got {} {}",
        stale.status,
        stale.body
    );

    let accepted = emulator
        .call(
            "PUT",
            &format!("/replication/v1/replicators/{}/replication-info", pct(&replicator)),
            Some(update_body("payments.*", &version)),
        )
        .await;
    assert_eq!(
        (accepted.status, accepted.at("/replicatorState").as_str()),
        (200, Some("UPDATING")),
        "expected the accepted update to report UPDATING, got {} {}",
        accepted.status,
        accepted.body
    );
    sleep_past_replicator_span();
    let described = emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(&replicator)),
            None,
        )
        .await;
    assert_eq!(
        described.at("/replicatorState").as_str(),
        Some("RUNNING"),
        "the first read after the update span must report RUNNING again, got {}",
        described.body
    );
    assert_eq!(
        described.at("/replicationInfoList/0/topicReplication/topicsToReplicate"),
        &json!(["payments.*"]),
        "the update must be readable from the replicator, got {}",
        described.body
    );

    // And the teardown: the replicator is absent for every later caller.
    let deleted = emulator
        .call(
            "DELETE",
            &format!("/replication/v1/replicators/{}", pct(&replicator)),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteReplicator to be accepted, got {} {}",
        deleted.status, deleted.body
    );
    let gone = emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(&replicator)),
            None,
        )
        .await;
    assert_eq!(
        (gone.status, gone.error_code()),
        (404, "NotFoundException"),
        "a deleted replicator must be answered as absent, got {} {}",
        gone.status,
        gone.body
    );
}

/// RULE: Amazon MSK provisions a replicator by ASSUMING the service-execution role
/// the create passed it, so a replicator whose role MSK cannot assume never starts:
/// the create is still admitted (`CREATING`), and the clock converges the replicator
/// to `FAILED` with the `StateInfo` that says why — while a replicator whose role
/// MSK can assume reaches `RUNNING`. The failure is grounded in what IAM answered:
/// with no IAM to ask, nothing is reported unassumable.
#[tokio::test]
async fn a_replicator_whose_role_msk_cannot_assume_converges_to_failed() {
    // The rows differ along what the account holds for the role the create passes —
    // a role MSK is trusted to assume, a role that trusts another service only, a
    // role the account never created — and along whether the service that owns the
    // roles is there to be asked at all.
    // (case, IAM mounted, the trust document the role carries, the state the clock owes)
    let rows: &[(&str, bool, Option<Option<&str>>, &str)] = &[
        (
            "a role whose trust policy admits MSK",
            true,
            Some(Some(KAFKA_SERVICE_PRINCIPAL)),
            "RUNNING",
        ),
        (
            "a role whose trust policy admits another service",
            true,
            Some(Some("ec2.amazonaws.com")),
            "FAILED",
        ),
        ("a role carrying no trust policy", true, Some(None), "FAILED"),
        ("a role the account never created", true, None, "FAILED"),
        ("no IAM to ask about the role", false, None, "RUNNING"),
    ];
    for (case, iam_mounted, trust, expected) in rows {
        let emulator = match iam_mounted {
            true => Emulator::with_iam(),
            false => Emulator::new(),
        };
        let role = match trust {
            Some(principal) => given_iam_role_trusting(
                &emulator.engine,
                "msk-replicator",
                principal.map(trust_policy_for).as_deref(),
            ),
            None => format!("arn:aws:iam::{ACCOUNT}:role/msk-replicator-absent"),
        };
        let source = emulator.create_ok(&ClusterRequest::named("role-source")).await;
        let target = emulator.create_ok(&ClusterRequest::named("role-target")).await;
        sleep_past_activation();

        let created = emulator
            .call(
                "POST",
                "/replication/v1/replicators",
                Some(replicator_body("roles", &source, &target, &["orders.*"], &role)),
            )
            .await;
        assert_eq!(
            (created.status, created.at("/replicatorState").as_str()),
            (200, Some("CREATING")),
            "case {case}: the create is admitted whatever the role, got {} {}",
            created.status,
            created.body
        );
        let replicator = created
            .at("/replicatorArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: CreateReplicator returned no replicatorArn"))
            .to_string();

        sleep_past_replicator_span();
        for (reader, reply) in [
            (
                "DescribeReplicator",
                emulator
                    .call(
                        "GET",
                        &format!("/replication/v1/replicators/{}", pct(&replicator)),
                        None,
                    )
                    .await,
            ),
            (
                "ListReplicators",
                emulator
                    .call("GET", "/replication/v1/replicators", None)
                    .await,
            ),
        ] {
            let state = match reader {
                "DescribeReplicator" => reply.at("/replicatorState").as_str(),
                _ => reply.at("/replicators/0/replicatorState").as_str(),
            };
            assert_eq!(
                state,
                Some(*expected),
                "case {case}: expected {reader} to report {expected}, got {}",
                reply.body
            );
        }
        let described = emulator
            .call(
                "GET",
                &format!("/replication/v1/replicators/{}", pct(&replicator)),
                None,
            )
            .await;
        let state_info = described.at("/stateInfo/message").as_str();
        match *expected {
            "FAILED" => {
                let message = state_info.unwrap_or_else(|| {
                    panic!("case {case}: a FAILED replicator must explain itself, got {}", described.body)
                });
                assert_eq!(
                    message,
                    unassumable_service_execution_role_message(&role),
                    "case {case}: expected the role failure's own sentence, got {message:?}"
                );
            }
            _ => assert_eq!(
                state_info, None,
                "case {case}: a replicator that is not FAILED explains nothing, got {}",
                described.body
            ),
        }
    }
}

/// A `CreateChannel` body streaming `topic_arn` to Amazon S3.
fn channel_body(name: &str, topic_arn: &str, freshness: i64) -> Value {
    json!({
        "channelName": name,
        "topicConfigurationList": [{
            "recordConverter": { "valueConverter": "JSON" },
            "topicArn": topic_arn,
        }],
        "s3DestinationConfiguration": {
            "dataFreshnessInSeconds": freshness,
            "deadLetterQueueS3": { "bucketArn": RECORDS_BUCKET_ARN },
            "serviceExecutionRoleArn": ROLE_ARN,
            "storage": {
                "bucketArn": RECORDS_BUCKET_ARN,
                "compressionType": "NONE",
                "storageClass": "STANDARD",
            },
        },
    })
}

/// RULE, for the channel family: a created channel reaches `ACTIVE` within one
/// channel span and the create's own cluster operation finishes at that same
/// instant, an accepted `UpdateChannel` is back to `ACTIVE` with the new
/// destination within one update span (its operation likewise), and a deleted
/// channel disappears — each on the FIRST read after its own span, with the
/// wrong-state refusal on the same path.
#[tokio::test]
async fn the_channel_sequence_settles_at_every_step() {
    let emulator = Emulator::new();
    // An Express cluster, which is the cluster kind a data-delivery channel
    // streams from.
    let cluster = emulator
        .create_ok(&ClusterRequest {
            kafka_version: "3.9.x",
            instance_type: "express.m7g.large",
            ..ClusterRequest::named("channel-sequence")
        })
        .await;
    sleep_past_activation();
    assert_eq!(
        cluster_state(&emulator, &cluster).await,
        "ACTIVE",
        "the first read after the activation span must report the cluster ACTIVE"
    );

    let topic = emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(&cluster)),
            Some(json!({ "topicName": "orders", "partitionCount": 1, "replicationFactor": 1 })),
        )
        .await;
    assert_eq!(
        topic.status, 200,
        "expected CreateTopic to be accepted, got {} {}",
        topic.status, topic.body
    );
    let topic_arn = topic
        .at("/topicArn")
        .as_str()
        .expect("CreateTopic returned no topicArn")
        .to_string();
    sleep_past_topic_span();
    let described = emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/topics/{}", pct(&cluster), pct("orders")),
            None,
        )
        .await;
    assert_eq!(
        described.at("/status").as_str(),
        Some("ACTIVE"),
        "the first read after the topic span must report the topic ACTIVE, got {}",
        described.body
    );

    // Step 1: the channel, whose create hands back both a channel ARN and the
    // cluster operation carrying it out.
    let created = emulator
        .create_channel(&cluster, channel_body("deliveries", &topic_arn, 300))
        .await;
    assert_eq!(
        created.status, 200,
        "expected CreateChannel to be accepted, got {} {}",
        created.status, created.body
    );
    let channel = created
        .at("/channelArn")
        .as_str()
        .expect("CreateChannel returned no channelArn")
        .to_string();
    let create_operation = operation_arn(&created, "CreateChannel");
    let reported = emulator.describe_channel(&cluster, &channel).await;
    assert_eq!(
        reported.at("/status").as_str(),
        Some("CREATING"),
        "a channel inside its span must report CREATING, got {}",
        reported.body
    );
    assert_operation_state(
        &emulator,
        &create_operation,
        "UPDATE_IN_PROGRESS",
        "the channel create inside its span",
    )
    .await;
    sleep_past_channel_activation();
    let reported = emulator.describe_channel(&cluster, &channel).await;
    assert_eq!(
        reported.at("/status").as_str(),
        Some("ACTIVE"),
        "the first read after the channel span must report ACTIVE, got {}",
        reported.body
    );
    assert_operation_state(
        &emulator,
        &create_operation,
        "UPDATE_COMPLETE",
        "the channel create once its span is over",
    )
    .await;
    let listed = emulator.list_channels(&cluster, "").await;
    let names: Vec<String> = listed
        .at("/channels")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["channelName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["deliveries".to_string()],
        "the listing must report the settled channel, got {}",
        listed.body
    );

    // Step 2: the destination change, which the client polls back to ACTIVE.
    let updated = emulator
        .call(
            "PUT",
            &format!(
                "/v1/clusters/{}/channels/{}",
                pct(&cluster),
                pct(&channel)
            ),
            Some(json!({ "s3DestinationUpdate": { "dataFreshnessInSeconds": 900 } })),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected UpdateChannel to be accepted, got {} {}",
        updated.status, updated.body
    );
    let update_operation = operation_arn(&updated, "UpdateChannel");
    assert_operation_state(
        &emulator,
        &update_operation,
        "UPDATE_IN_PROGRESS",
        "the channel update inside its span",
    )
    .await;
    sleep_past_channel_update();
    let reported = emulator.describe_channel(&cluster, &channel).await;
    assert_eq!(
        (
            reported.at("/status").as_str(),
            reported.at("/s3DestinationConfiguration/dataFreshnessInSeconds").as_i64(),
        ),
        (Some("ACTIVE"), Some(900)),
        "the first read after the update span must report ACTIVE with the new destination, \
         got {}",
        reported.body
    );
    assert_operation_state(
        &emulator,
        &update_operation,
        "UPDATE_COMPLETE",
        "the channel update once its span is over",
    )
    .await;

    // Step 3: the delete, the refusal that sits on it, and the disappearance.
    let deleted = emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}/channels/{}",
                pct(&cluster),
                pct(&channel)
            ),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteChannel to be accepted, got {} {}",
        deleted.status, deleted.body
    );
    let refused = emulator
        .call(
            "PUT",
            &format!(
                "/v1/clusters/{}/channels/{}",
                pct(&cluster),
                pct(&channel)
            ),
            Some(json!({ "s3DestinationUpdate": { "dataFreshnessInSeconds": 900 } })),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code(), refused.error_message()),
        (
            400,
            "BadRequestException",
            "Channel cannot be updated in state DELETING. Channel must be in ACTIVE state to \
             be updated."
        ),
        "an update of a DELETING channel must be refused, got {} {}",
        refused.status,
        refused.body
    );
    sleep_past_channel_activation();
    let gone = emulator.describe_channel(&cluster, &channel).await;
    assert_eq!(
        gone.status, 404,
        "the first read after the deletion span must report the channel absent, got {} {}",
        gone.status, gone.body
    );
}

/// RULE, for the cluster-operation family: a cluster mutation's operation reaches
/// `UPDATE_COMPLETE` within one update span, the cluster is `ACTIVE` again and
/// reports the new value at that same instant, and a configuration a cluster has
/// been re-pointed away from is deletable right then — without any caller having to
/// read the cluster in between, since the transition the clock owed it is owed
/// whether or not anybody looked.
#[tokio::test]
async fn the_cluster_operation_sequence_settles_at_every_step() {
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest {
            brokers: 3,
            ..ClusterRequest::named("operation-sequence")
        })
        .await;
    sleep_past_activation();

    // A broker-count change: the new count is visible once the operation is over.
    let version = current_version(&emulator, &cluster).await;
    let scaled = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/count", pct(&cluster)),
            Some(json!({ "currentVersion": version, "targetNumberOfBrokerNodes": 6 })),
        )
        .await;
    assert_eq!(
        scaled.status, 200,
        "expected UpdateBrokerCount to be accepted, got {} {}",
        scaled.status, scaled.body
    );
    let operation = operation_arn(&scaled, "UpdateBrokerCount");
    assert_operation_state(
        &emulator,
        &operation,
        "UPDATE_IN_PROGRESS",
        "the broker-count change inside its span",
    )
    .await;
    sleep_past_update();
    assert_operation_state(
        &emulator,
        &operation,
        "UPDATE_COMPLETE",
        "the broker-count change once its span is over",
    )
    .await;
    let described = emulator.describe_v2(&cluster).await;
    assert_eq!(
        (
            described.at("/clusterInfo/state").as_str(),
            described
                .at("/clusterInfo/provisioned/numberOfBrokerNodes")
                .as_i64(),
        ),
        (Some("ACTIVE"), Some(6)),
        "the first read after the update span must report the cluster ACTIVE with the new \
         broker count, got {}",
        described.body
    );

    // A configuration a cluster uses cannot be deleted; one it has been
    // re-pointed away from can.
    let first = emulator
        .create_configuration_ok("seq-one", None, b"auto.create.topics.enable=true\n")
        .await;
    let second = emulator
        .create_configuration_ok("seq-two", None, b"auto.create.topics.enable=true\n")
        .await;
    for (case, configuration) in [("the first", &first), ("the second", &second)] {
        let version = current_version(&emulator, &cluster).await;
        let pointed = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/configuration", pct(&cluster)),
                Some(json!({
                    "currentVersion": version,
                    "configurationInfo": { "arn": configuration, "revision": 1 },
                })),
            )
            .await;
        assert_eq!(
            pointed.status, 200,
            "case {case}: expected UpdateClusterConfiguration to be accepted, got {} {}",
            pointed.status, pointed.body
        );
        let operation = operation_arn(&pointed, "UpdateClusterConfiguration");
        sleep_past_update();
        assert_operation_state(
            &emulator,
            &operation,
            "UPDATE_COMPLETE",
            &format!("the re-pointing to {case} configuration"),
        )
        .await;
    }

    // The cluster now uses the second configuration, and nothing has read it
    // since: the first configuration is free and the second is held.
    let refused = emulator
        .call("DELETE", &format!("/v1/configurations/{}", pct(&second)), None)
        .await;
    assert_eq!(
        (refused.status, refused.error_code(), refused.error_message()),
        (400, "BadRequestException", CONFIGURATION_IN_USE_MESSAGE),
        "a configuration the cluster uses must not be deletable, got {} {}",
        refused.status,
        refused.body
    );
    let still = emulator.describe_configuration(&second).await;
    assert_eq!(
        still.at("/state").as_str(),
        Some("ACTIVE"),
        "a refused delete must leave the configuration ACTIVE, got {}",
        still.body
    );
    let accepted = emulator
        .call("DELETE", &format!("/v1/configurations/{}", pct(&first)), None)
        .await;
    assert_eq!(
        (accepted.status, accepted.at("/state").as_str()),
        (200, Some("DELETING")),
        "a configuration the cluster was re-pointed away from must be deletable, got {} {}",
        accepted.status,
        accepted.body
    );
    sleep_past_configuration_deletion();
    let gone = emulator.describe_configuration(&first).await;
    assert_eq!(
        gone.status, 400,
        "the first read after the deletion span must report the configuration absent, got {} {}",
        gone.status, gone.body
    );
}
