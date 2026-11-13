// SPDX-License-Identifier: CC0-1.0

//! The gates Amazon MSK puts in front of a create and in front of a replication
//! update: WHICH requests it refuses, in WHICH words, and in WHICH ORDER.
//!
//! Four rules, each asserted over a TABLE rather than one recorded request:
//!
//! 1. A provisioned create runs its brokers on a version of the service's OWN
//!    catalog — the one `ListKafkaVersions` serves — so a `KafkaVersion` that
//!    catalog does not publish is refused `BadRequestException` (400) naming
//!    `kafkaVersion`, and no cluster is registered. The sentence names the broker
//!    category the request asked for and the versions MSK offers for a new
//!    cluster.
//! 2. `CreateClusterV2` names exactly one cluster type, and the two ways to get
//!    that wrong are two DIFFERENT refusals: naming neither is a refusal of the
//!    `clusterType` parameter, naming both is MSK's body-level rejection.
//! 3. `UpdateReplicationInfo` admits the REQUEST before it resolves the replicator
//!    its path names: a request that leaves an end of its replication unnamed, or
//!    quotes a `CurrentVersion` that is not a version token at all, is answered
//!    about that member whether or not the ARN names a replicator.
//! 4. A replication configuration is changed only from the state that accepts a
//!    change, and the refusal names the states that do.
//!
//! The rows differ along what each rule generalises over: the operation (the v1
//! create and the v2 one), the broker category, the FORM of the rejected version,
//! the support status of an accepted one, which cluster-type bodies the request
//! carries and what is in them, which end of a replication is unnamed and how,
//! the FORM of an unusable version token, whether the replicator the path names
//! exists, and which non-accepting state the replicator is in.
//!
//! The REFUSING half of the client-subnet gate needs Amazon EC2 beside this
//! service and lives in `core/router/tests/all/kafka_placement_subnets_resolve_in_ec2.rs`;
//! the harness here mounts this service alone, so nothing resolves its subnets.
//!
//! The HTTP harness comes from [`crate::kafka_harness`] — the same router, the same
//! SigV4-scoped request builder — rather than a second one built here.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    KAFKA_VERSION_ACTIVE, KAFKA_VERSION_CATALOG, REPLICATOR_ACTIVATION_DELAY,
};

use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// The IAM role every replicator below is created with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// The uuid segment of the resource ARNs below that name nothing.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// What MSK answers a create that names NEITHER cluster type: one fixed sentence,
/// written out here rather than read from the service's own constant, so the
/// wording is pinned by this suite instead of by the code under test.
const NO_CLUSTER_TYPE_MESSAGE: &str =
    "You must specify a cluster type, either provisioned or serverless.";

/// What MSK answers a create that names BOTH: its body-level rejection, which says
/// nothing about which member to drop.
const BOTH_CLUSTER_TYPES_MESSAGE: &str = "Invalid request body";

/// What MSK answers a request whose `CurrentVersion` is not a version token at all.
const INVALID_CURRENT_VERSION_MESSAGE: &str = "Invalid parameter currentVersion.";

/// What MSK answers a value the member carrying it cannot carry — here, a value
/// that is not a replicator ARN. Written out rather than imported, so the wording
/// is pinned by this suite.
const VALUE_NOT_VALID_MESSAGE: &str = "The provided value is not valid.";

/// What MSK answers a replication update against a replicator in a state that does
/// not accept one. The bracketed list is the set of states that DO.
const NOT_RUNNING_MESSAGE: &str = "Invalid parameter replicatorArn: This operation is only valid \
     for resources that are in one of the following states: [RUNNING]";

/// The versions MSK offers for a NEW cluster: the versions the catalog documented
/// in `src/state.rs` (the Amazon MSK Developer Guide's "Supported Apache Kafka
/// versions") publishes an `ACTIVE` row for, each named ONCE at its first published
/// row — which is the order the refusal renders them in. Read off the catalog's own
/// ROWS rather than through the service's offered-set helper, so this suite can
/// still disagree with that helper, and never written out, so it cannot drift from
/// the versions the service publishes.
fn offered_versions() -> Vec<&'static str> {
    let mut offered: Vec<&'static str> = Vec::new();
    for entry in KAFKA_VERSION_CATALOG {
        let offered_somewhere = KAFKA_VERSION_CATALOG
            .iter()
            .any(|row| row.version == entry.version && row.status == KAFKA_VERSION_ACTIVE);
        if offered_somewhere && !offered.contains(&entry.version) {
            offered.push(entry.version);
        }
    }
    offered
}

/// The sentence MSK refuses `version` for brokers of `category` with: its fixed
/// text, the version the request named, and the offered list.
fn unsupported_version_message(category: &str, version: &str) -> String {
    format!(
        "{category} instance types are not supported for Kafka version {version}. \
         Valid values: [{}]",
        offered_versions().join(", ")
    )
}

/// A `CreateClusterV2` body whose `Provisioned` block asks for `version` on brokers
/// of `instance_type`.
fn v2_provisioned_body(name: &str, version: &str, instance_type: &str) -> Value {
    json!({
        "clusterName": name,
        "provisioned": {
            "kafkaVersion": version,
            "numberOfBrokerNodes": 2,
            "brokerNodeGroupInfo": {
                "instanceType": instance_type,
                "clientSubnets": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
            },
        },
    })
}

/// A `CreateClusterV2` body whose `Serverless` block names `configurations` VPC
/// configurations of one subnet each.
fn v2_serverless_body(name: &str, configurations: usize) -> Value {
    json!({
        "clusterName": name,
        "serverless": {
            "vpcConfigs": (0..configurations)
                .map(|index| json!({ "subnetIds": [format!("subnet-{index:017x}")] }))
                .collect::<Vec<Value>>(),
        },
    })
}

/// Every cluster name `ListClustersV2` reports.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    emulator
        .list_v2("")
        .await
        .at("/clusterInfoList")
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry["clusterName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a provisioned create's `KafkaVersion` is one the service's catalog
/// publishes; any other value is refused HTTP 400 `BadRequestException` naming
/// `kafkaVersion`, with MSK's sentence for the broker category the request asked
/// for, and registers no cluster.
#[tokio::test]
async fn a_provisioned_create_refuses_a_version_the_catalog_does_not_publish() {
    let emulator = Emulator::new();

    // The rows differ along the FORM of the rejected value — a release newer than
    // every catalogued one, an older-looking one, a two-component version, a
    // variant spelling of a catalogued release, a `.x` minor the catalog does not
    // publish and a value that is no version at all — and along the broker
    // category, which the sentence names: `kafka.*` instance types are the
    // standard ones, `express.*` the Express ones.
    let versions = [
        "9.9.9",
        "0.0.1",
        "3.7",
        "3.6.0.kraft",
        "3.5.x",
        "not-a-version",
    ];
    let categories = [
        ("Standard", "kafka.m5.large"),
        ("Express", "express.m7g.large"),
    ];

    for (index, version) in versions.iter().enumerate() {
        for (category, instance_type) in &categories {
            let expected = unsupported_version_message(category, version);

            // The v1 create and the v2 provisioned one submit the same broker
            // configuration, so the same rule holds for both.
            let name = format!("bad-version-v1-{index}-{category}");
            let case = format!("CreateCluster {version:?} on {instance_type}");
            let reply = emulator
                .create(&ClusterRequest {
                    kafka_version: version,
                    instance_type,
                    ..ClusterRequest::named(&name)
                })
                .await;
            assert_refusal(&reply, &case, "kafkaVersion", &expected);

            let name = format!("bad-version-v2-{index}-{category}");
            let case = format!("CreateClusterV2 {version:?} on {instance_type}");
            let reply = emulator
                .create_cluster_v2(v2_provisioned_body(&name, version, instance_type))
                .await;
            assert_refusal(&reply, &case, "kafkaVersion", &expected);
        }
    }

    // Not one of them registered anything.
    let listed = listed_names(&emulator).await;
    assert!(
        listed.is_empty(),
        "a refused create must register no cluster, got {listed:?}"
    );

    // The other side of the rule: every version the catalog PUBLISHES is admitted
    // — an offered one, a deprecated one (a cluster running it is what an upgrade
    // moves away from), the `.tiered` spelling, a `.x` minor and a `.kraft` line —
    // and the cluster reports the version it was created with.
    let accepted: Vec<&str> = vec!["3.6.0", "2.8.1", "2.8.2.tiered", "3.9.x", "3.9.x.kraft"];
    for (index, version) in accepted.iter().enumerate() {
        let case = format!("a create on the catalogued version {version:?}");
        let arn = emulator
            .create_ok(&ClusterRequest {
                kafka_version: version,
                ..ClusterRequest::named(&format!("good-version-{index}"))
            })
            .await;
        let described = emulator.describe(&arn).await;
        assert_eq!(
            described
                .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                .as_str(),
            Some(*version),
            "case {case}: the cluster must report the version it was created with, got {}",
            described.body
        );
    }
}

/// RULE: `CreateClusterV2` names exactly ONE cluster type, and the two ways of
/// getting that wrong are two DIFFERENT refusals — a missing cluster type names the
/// `clusterType` parameter, while naming both kinds is a rejection of the body as a
/// whole — so a client can tell the two input errors apart. Neither registers
/// anything.
#[tokio::test]
async fn the_two_cluster_type_refusals_are_distinct() {
    let emulator = Emulator::new();

    // The two messages are not variants of one sentence.
    assert_ne!(
        NO_CLUSTER_TYPE_MESSAGE, BOTH_CLUSTER_TYPES_MESSAGE,
        "the two cluster-type refusals must carry different sentences"
    );

    // Rows naming NEITHER kind differ in what else the body carries — nothing but
    // the name, tags, an empty tag set — because none of that names a cluster type.
    let neither: Vec<(&str, Value)> = vec![
        ("the name alone", json!({ "clusterName": "no-kind-plain" })),
        (
            "a tagged create",
            json!({ "clusterName": "no-kind-tagged", "tags": { "team": "data" } }),
        ),
        (
            "an empty tag set",
            json!({ "clusterName": "no-kind-untagged", "tags": {} }),
        ),
    ];
    for (case, body) in &neither {
        let reply = emulator.create_cluster_v2(body.clone()).await;
        assert_refusal(&reply, case, "clusterType", NO_CLUSTER_TYPE_MESSAGE);
    }

    // Rows naming BOTH kinds differ in what each body says — the broker count of
    // the provisioned one, how many VPC configurations the serverless one names —
    // because MSK rejects the body whatever is in the two blocks.
    for (index, (case, brokers, configurations)) in [
        ("one broker, one VPC configuration", 1, 1),
        ("three brokers, two VPC configurations", 3, 2),
    ]
    .iter()
    .enumerate()
    {
        let name = format!("both-kinds-{index}");
        let mut body = v2_provisioned_body(&name, "3.6.0", "kafka.m5.large");
        body["provisioned"]["numberOfBrokerNodes"] = json!(brokers);
        body["serverless"] = v2_serverless_body(&name, *configurations)["serverless"].clone();
        let reply = emulator.create_cluster_v2(body).await;
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (400, "BadRequestException", BOTH_CLUSTER_TYPES_MESSAGE),
            "case {case}: expected the body-level refusal, got {} {}",
            reply.status,
            reply.body
        );
    }

    let listed = listed_names(&emulator).await;
    assert!(
        listed.is_empty(),
        "a refused create must register no cluster, got {listed:?}"
    );

    // The control: exactly one cluster type IS accepted, whichever one it is.
    for (case, body) in [
        (
            "a provisioned create",
            v2_provisioned_body("one-kind-provisioned", "3.6.0", "kafka.m5.large"),
        ),
        (
            "a serverless create",
            v2_serverless_body("one-kind-serverless", 1),
        ),
    ] {
        let reply = emulator.create_cluster_v2(body).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: a create naming one cluster type must be accepted, got {} {}",
            reply.status, reply.body
        );
    }
}

/// `CreateReplicator` for a replicator spanning the two Amazon MSK clusters
/// `source` and `target`, with one replication between them.
fn replicator_body(name: &str, source: &str, target: &str) -> Value {
    let cluster = |arn: &str| {
        json!({
            "amazonMskCluster": { "mskClusterArn": arn },
            "vpcConfig": {
                "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
            },
        })
    };
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": [cluster(source), cluster(target)],
        "replicationInfoList": [{
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "targetCompressionType": "NONE",
            "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
            "topicReplication": { "topicsToReplicate": [".*"] },
        }],
    })
}

/// A replication update addressing the replication between `source` and `target`
/// and quoting `version`.
fn update_body(source: &str, target: &str, version: &str) -> Value {
    json!({
        "currentVersion": version,
        "sourceKafkaClusterArn": source,
        "targetKafkaClusterArn": target,
        "topicReplication": {
            "topicsToReplicate": ["accepted-.*"],
            "topicsToExclude": ["__.*"],
            "detectAndCopyNewTopics": true,
            "copyTopicConfigurations": true,
            "copyAccessControlListsForTopics": false,
        },
    })
}

/// `UpdateReplicationInfo` for `arn`.
async fn update(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/replication/v1/replicators/{}/replication-info", pct(arn)),
            Some(body),
        )
        .await
}

/// `DescribeReplicator` for `arn`.
async fn describe_replicator(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(arn)),
            None,
        )
        .await
}

/// One replicator spanning two clusters this suite creates, so it PROVISIONS and
/// reaches `RUNNING` — the state a configuration change is accepted from. Hands
/// back its ARN and the ARNs of the two clusters it spans.
async fn replicator(emulator: &Emulator, name: &str) -> (String, String, String) {
    let source = emulator
        .create_ok(&ClusterRequest::named(&format!("{name}-source")))
        .await;
    let target = emulator
        .create_ok(&ClusterRequest::named(&format!("{name}-target")))
        .await;
    let created = emulator
        .call(
            "POST",
            "/replication/v1/replicators",
            Some(replicator_body(name, &source, &target)),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name}: expected CreateReplicator to return 200, got {} {}",
        created.status, created.body
    );
    let arn = created.body["replicatorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: CreateReplicator returned no replicatorArn"))
        .to_string();
    (arn, source, target)
}

/// Sleep past the emulator's own span into the settled state, so a later call sees
/// a settled replicator. `std::thread::sleep` because the workspace's tokio carries
/// no timer feature; nothing else has to make progress here.
fn sleep_past_running() {
    std::thread::sleep(REPLICATOR_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// The `CurrentVersion` the reader reports for `arn`.
async fn current_version(emulator: &Emulator, arn: &str) -> String {
    describe_replicator(emulator, arn)
        .await
        .at("/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("DescribeReplicator reported no currentVersion for {arn}"))
        .to_string()
}

/// RULE: `UpdateReplicationInfo` admits the REQUEST before it resolves the
/// replicator its path names, so a `CurrentVersion` that is not a version token at
/// all is refused HTTP 400 `BadRequestException` with MSK's fixed sentence for that
/// member — whether or not the ARN names a replicator — and changes nothing.
#[tokio::test]
async fn an_unusable_current_version_is_refused_before_the_replicator_is_resolved() {
    let emulator = Emulator::new();
    let (present, source, target) = replicator(&emulator, "versions").await;
    sleep_past_running();
    let version = current_version(&emulator, &present).await;
    let intact = describe_replicator(&emulator, &present).await.body;
    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/absent/{GHOST_ID}");

    // Rows differ along the FORM of the unusable value: empty, a plain number, a
    // word, the right length with the wrong prefix, a token one character short,
    // one too long, and a well-shaped token in the wrong case.
    let unusable = [
        ("empty", ""),
        ("a plain number", "1"),
        ("a word", "latest"),
        ("the wrong prefix", "X3AEGXETSR30VB"),
        ("one character short", "K3AEGXETSR30V"),
        ("one character too long", "K3AEGXETSR30VBB"),
        ("lower case", "k3aegxetsr30vb"),
    ];
    // The path dimension is what makes the ORDER of the two gates observable: the
    // same body is answered the same way against a replicator that exists and one
    // that was never issued, which only holds if the body is admitted first.
    for (form, quoted) in &unusable {
        for (path_case, arn) in [("present", &present), ("never-issued", &absent)] {
            let case = format!("{form}/{path_case}");
            let reply = update(&emulator, arn, update_body(&source, &target, quoted)).await;
            assert_refusal(
                &reply,
                &case,
                "replicatorArn",
                INVALID_CURRENT_VERSION_MESSAGE,
            );
        }
    }
    assert_eq!(
        describe_replicator(&emulator, &present).await.body,
        intact,
        "a refused update must leave the replicator exactly as it was"
    );

    // A well-shaped token the replicator is not at is a DIFFERENT refusal: it is
    // decided against the stored record, so it says something else.
    let stale = update(
        &emulator,
        &present,
        update_body(&source, &target, "K0000000000000"),
    )
    .await;
    assert_eq!(
        (stale.status, stale.error_code()),
        (400, "BadRequestException"),
        "a well-formed but stale token must be refused, got {} {}",
        stale.status,
        stale.body
    );
    assert_ne!(
        stale.error_message(),
        INVALID_CURRENT_VERSION_MESSAGE,
        "a stale token must not be answered as an unusable parameter, got {}",
        stale.body
    );

    // The control: the token the reader reports is accepted.
    let accepted = update(
        &emulator,
        &present,
        update_body(&source, &target, &version),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the replicator's own version must be accepted, got {} {}",
        accepted.status, accepted.body
    );
}

/// RULE: a request that does not name one end of its replication exactly once is
/// refused HTTP 400 `BadRequestException` naming THAT end's Amazon MSK cluster ARN
/// member as the parameter at fault — the member a caller would send to fix the
/// request — before the replicator the path names is resolved.
#[tokio::test]
async fn an_unnamed_replication_end_names_that_ends_member() {
    let emulator = Emulator::new();
    let (present, source, target) = replicator(&emulator, "ends").await;
    sleep_past_running();
    let version = current_version(&emulator, &present).await;
    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/absent/{GHOST_ID}");

    // Rows differ along which end is unnamed, how it fails to be named exactly
    // once (omitted, empty, given twice) and how the OTHER end is named.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "no source",
            json!({ "targetKafkaClusterArn": target }),
            "sourceKafkaClusterArn",
        ),
        (
            "empty source",
            json!({ "sourceKafkaClusterArn": "", "targetKafkaClusterArn": target }),
            "sourceKafkaClusterArn",
        ),
        (
            "source named twice",
            json!({
                "sourceKafkaClusterArn": source,
                "sourceKafkaClusterId": "source-by-id",
                "targetKafkaClusterArn": target,
            }),
            "sourceKafkaClusterArn",
        ),
        (
            "no target",
            json!({ "sourceKafkaClusterArn": source }),
            "targetKafkaClusterArn",
        ),
        (
            "no target, source by id",
            json!({ "sourceKafkaClusterId": "source-by-id" }),
            "targetKafkaClusterArn",
        ),
        (
            "target named twice",
            json!({
                "sourceKafkaClusterArn": source,
                "targetKafkaClusterArn": target,
                "targetKafkaClusterId": "target-by-id",
            }),
            "targetKafkaClusterArn",
        ),
    ];

    for (case, ends, parameter) in &rows {
        for (path_case, arn) in [("present", &present), ("never-issued", &absent)] {
            let mut body = update_body(&source, &target, &version);
            for member in [
                "sourceKafkaClusterArn",
                "sourceKafkaClusterId",
                "targetKafkaClusterArn",
                "targetKafkaClusterId",
            ] {
                body.as_object_mut()
                    .expect("the update body is a JSON object")
                    .remove(member);
            }
            for (member, value) in ends.as_object().expect("the ends are a JSON object") {
                body[member] = value.clone();
            }
            let reply = update(&emulator, arn, body).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case {case}/{path_case}: expected a 400 BadRequestException, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some(*parameter),
                "case {case}/{path_case}: expected the refusal to name {parameter}, got {}",
                reply.body
            );
        }
    }
}

/// RULE: a replicator's replication configuration is changed only from a state
/// that accepts a change, so a call against one in any other state is refused HTTP
/// 400 `BadRequestException` with the sentence naming the states that DO accept it,
/// and nothing is applied.
#[tokio::test]
async fn a_replication_update_is_accepted_only_from_an_accepting_state() {
    let emulator = Emulator::new();

    // A replicator over clusters that do not exist is never provisioned, so the
    // clock owes it the terminal FAILED.
    let ghost = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/ghost/{GHOST_ID}");
    let other_ghost = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/ghost-two/{GHOST_ID}");
    let failing = emulator
        .call(
            "POST",
            "/replication/v1/replicators",
            Some(replicator_body("failing", &ghost, &other_ghost)),
        )
        .await;
    assert_eq!(
        failing.status, 200,
        "a create whose clusters do not resolve is admitted, got {} {}",
        failing.status, failing.body
    );
    let failing = failing.body["replicatorArn"]
        .as_str()
        .expect("CreateReplicator returned no replicatorArn")
        .to_string();

    let (settling, source, target) = replicator(&emulator, "states").await;

    // CREATING: the replicator has just been created, and the clock has not yet
    // owed it its settled state. The call quotes the version the replicator is
    // actually at, so what refuses it is the STATE and not the version.
    let creating_version = current_version(&emulator, &settling).await;
    let creating = update(
        &emulator,
        &settling,
        update_body(&source, &target, &creating_version),
    )
    .await;
    assert_state_refusal(&creating, "CREATING");

    sleep_past_running();
    let version = current_version(&emulator, &settling).await;
    let before = describe_replicator(&emulator, &settling).await.body;

    // FAILED: a terminal state, so no configuration change is ever accepted.
    let failed = update(
        &emulator,
        &failing,
        update_body(&ghost, &other_ghost, &current_version(&emulator, &failing).await),
    )
    .await;
    assert_state_refusal(&failed, "FAILED");

    // RUNNING: accepted, which moves the replicator to UPDATING.
    let accepted = update(
        &emulator,
        &settling,
        update_body(&source, &target, &version),
    )
    .await;
    assert_eq!(
        (accepted.status, accepted.at("/replicatorState").as_str()),
        (200, Some("UPDATING")),
        "an update from RUNNING must be accepted, got {} {}",
        accepted.status,
        accepted.body
    );

    // UPDATING: a change is already rolling out, so a second one is refused — even
    // though it quotes the version the first one minted.
    let rolling = current_version(&emulator, &settling).await;
    let updating = update(
        &emulator,
        &settling,
        update_body(&source, &target, &rolling),
    )
    .await;
    assert_state_refusal(&updating, "UPDATING");

    // Once the clock owes the replicator RUNNING again, the same call is accepted:
    // the gate is the STATE, not the call.
    sleep_past_running();
    let again = update(
        &emulator,
        &settling,
        update_body(&source, &target, &rolling),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "an update from the settled state must be accepted, got {} {}",
        again.status, again.body
    );

    // The refused calls applied nothing: the configuration the reader reports is
    // the one the accepted update wrote, and the FAILED replicator is as it was.
    let described = describe_replicator(&emulator, &failing).await;
    assert_eq!(
        described.at("/replicationInfoList/0/topicReplication/topicsToReplicate"),
        &json!([".*"]),
        "a refused update must leave the replicator's configuration alone, got {}",
        described.body
    );
    assert_ne!(
        before.pointer("/currentVersion"),
        describe_replicator(&emulator, &settling)
            .await
            .body
            .pointer("/currentVersion"),
        "an accepted update must have moved the version on"
    );
}

/// RULE: a value that is not shaped like a replicator ARN names no replicator, so
/// every operation that takes one refuses it HTTP 400 `BadRequestException` with
/// the service's ONE fixed sentence for a value it cannot read — naming
/// `replicatorArn` in `invalidParameter` and echoing no part of the value — which
/// is a different refusal from a well-formed ARN no replicator holds (404).
#[tokio::test]
async fn a_value_that_is_no_replicator_arn_is_refused_in_the_services_fixed_words() {
    let emulator = Emulator::new();
    let (present, source, target) = replicator(&emulator, "shapes").await;
    sleep_past_running();
    let version = current_version(&emulator, &present).await;

    // Rows differ along HOW the value fails to be a replicator ARN: not an ARN at
    // all, the wrong service, the wrong resource type, a missing resource segment,
    // an empty resource name, and a cluster ARN of this very account.
    let unusable = [
        ("not an ARN at all", "not-a-valid-replicator-arn".to_string()),
        (
            "the wrong service",
            format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:replicator/wrong/{GHOST_ID}"),
        ),
        (
            "the wrong resource type",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/wrong/{GHOST_ID}"),
        ),
        (
            "no resource segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator"),
        ),
        (
            "an empty resource name",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator//{GHOST_ID}"),
        ),
    ];

    for (form, value) in &unusable {
        // The same rule holds wherever the member is taken, so the rows run over
        // the three operations that take one.
        let requests: Vec<(&str, Reply)> = vec![
            (
                "UpdateReplicationInfo",
                update(&emulator, value, update_body(&source, &target, &version)).await,
            ),
            (
                "DescribeReplicator",
                describe_replicator(&emulator, value).await,
            ),
            (
                "DeleteReplicator",
                emulator
                    .call(
                        "DELETE",
                        &format!("/replication/v1/replicators/{}", pct(value)),
                        None,
                    )
                    .await,
            ),
        ];
        for (operation, reply) in requests {
            let case = format!("{operation} with {form}");
            assert_refusal(&reply, &case, "replicatorArn", VALUE_NOT_VALID_MESSAGE);
            assert!(
                !reply.error_message().contains(value.as_str()),
                "case {case}: the refusal must echo no part of the value, got {}",
                reply.body
            );
        }
    }

    // The other side of the distinction: a well-formed replicator ARN this region
    // holds nothing for is a 404 instead, and it is a different sentence.
    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/absent/{GHOST_ID}");
    let reply = describe_replicator(&emulator, &absent).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (404, "NotFoundException"),
        "a well-formed ARN nothing holds must be a 404, got {} {}",
        reply.status,
        reply.body
    );
    assert_ne!(
        reply.error_message(),
        VALUE_NOT_VALID_MESSAGE,
        "a resource that was not found must not be answered as an unusable value, got {}",
        reply.body
    );
}

/// One refusal: HTTP 400 `BadRequestException`, naming `parameter` at fault and
/// carrying `message` exactly.
fn assert_refusal(reply: &Reply, case: &str, parameter: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code(), reply.error_message()),
        (400, "BadRequestException", message),
        "case {case}: expected a 400 BadRequestException {message:?}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(parameter),
        "case {case}: expected the refusal to name {parameter} at fault, got {}",
        reply.body
    );
}

/// The state refusal: HTTP 400 `BadRequestException` with the sentence naming the
/// accepting states, and NO `invalidParameter` of its own — the request is well
/// formed, so the refusal is about the resource.
fn assert_state_refusal(reply: &Reply, state: &str) {
    assert_eq!(
        (reply.status, reply.error_code(), reply.error_message()),
        (400, "BadRequestException", NOT_RUNNING_MESSAGE),
        "case a replicator in {state}: expected a 400 BadRequestException \
         {NOT_RUNNING_MESSAGE:?}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter"),
        &Value::Null,
        "case a replicator in {state}: the refusal must name no parameter, got {}",
        reply.body
    );
}
