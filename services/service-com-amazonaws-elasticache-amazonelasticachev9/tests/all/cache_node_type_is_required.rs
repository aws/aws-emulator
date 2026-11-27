// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule that a create which provisions its own cache nodes has to
//! be told what shape they are.
//!
//! The rule under test: `CreateCacheCluster` resolves no node type of its own, so a request
//! that provisions nodes of its own — one that names no `ReplicationGroupId`, whose nodes
//! would otherwise take the group's shape — must name `CacheNodeType`, and one that does not
//! is refused `InvalidParameterValue` (HTTP 400) with the sentence this service words a
//! parameter it requires with, before anything is stored.
//!
//! The tables span what the rule generalises over: the engine the cluster would run, the
//! other members the request carries, the node count, and whether the request joins a
//! replication group (where the member is not required of the request at all). Every
//! refusal is also read back through `DescribeCacheClusters`, so a refusal that still left a
//! cluster behind cannot pass.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts.

use service_com_amazonaws_elasticache_amazonelasticachev9::service::absent_parameter;

use crate::harness::{params, Emulator, Reply, TEST_NODE_TYPE};

/// The sentence the refusal must state, read off the one helper that words a parameter this
/// service requires of a handler — so the suite pins the WORDING for the member this rule is
/// about rather than restating it.
fn required_sentence(member: &str) -> String {
    absent_parameter::conditionally_required_message(member)
}

/// `CreateCacheCluster` with `pairs` and nothing added.
async fn create(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator.call("CreateCacheCluster", &params(pairs)).await
}

/// `DescribeCacheClusters` for one id.
async fn describe(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call("DescribeCacheClusters", &params(&[("CacheClusterId", id)]))
        .await
}

/// One omitted-node-type row: the case name and the members the request carries BESIDE its
/// identifier — never a node type.
const WITHOUT_NODE_TYPE: &[(&str, &[(&str, &str)])] = &[
    ("nothing else at all", &[]),
    ("the default engine, named", &[("Engine", "redis")]),
    (
        "a memcached cluster of three nodes",
        &[("Engine", "memcached"), ("NumCacheNodes", "3")],
    ),
    ("a valkey cluster", &[("Engine", "valkey")]),
    (
        "an engine version and a port",
        &[("Engine", "redis"), ("EngineVersion", "7.1"), ("Port", "6380")],
    ),
    (
        "a parameter group the account holds",
        &[("CacheParameterGroupName", "default.redis7")],
    ),
    (
        "tags and a maintenance window",
        &[
            ("Tags.Tag.1.Key", "team"),
            ("Tags.Tag.1.Value", "cache"),
            ("PreferredMaintenanceWindow", "sun:23:00-mon:01:30"),
        ],
    ),
];

#[tokio::test]
async fn a_standalone_create_without_a_node_type_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let expected = required_sentence("CacheNodeType");
    for (index, (case, extra)) in WITHOUT_NODE_TYPE.iter().enumerate() {
        let id = format!("no-node-type-{index}");
        let mut pairs = vec![("CacheClusterId", id.as_str())];
        pairs.extend_from_slice(extra);
        let refused = create(&emulator, &pairs).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "InvalidParameterValue"),
            "case {case:?}: expected 400 InvalidParameterValue, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            refused.error_message()
        );
        // Nothing was created: the identifier resolves to no cluster at all.
        let described = describe(&emulator, &id).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "CacheClusterNotFound"),
            "case {case:?}: a refused create must store nothing, got {} {}",
            described.status,
            described.body
        );
    }
}

#[tokio::test]
async fn the_same_requests_with_a_node_type_are_served() {
    // The positive control for every row above: the SAME request, with the one member the
    // rule is about, is accepted and the cluster reports the node type it was asked for.
    let emulator = Emulator::new();
    for (index, (case, extra)) in WITHOUT_NODE_TYPE.iter().enumerate() {
        let id = format!("with-node-type-{index}");
        let mut pairs = vec![
            ("CacheClusterId", id.as_str()),
            ("CacheNodeType", TEST_NODE_TYPE),
        ];
        pairs.extend_from_slice(extra);
        let created = create(&emulator, &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the same request naming a node type must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("CacheCluster/CacheNodeType"),
            Some(TEST_NODE_TYPE),
            "case {case:?}: the cluster must run the node type the request named, got {:?}",
            created.text("CacheCluster/CacheNodeType")
        );
        // And the stored cluster reports it too, so the answer is the persisted record.
        let described = describe(&emulator, &id).await;
        assert_eq!(
            described.text("CacheCluster/CacheNodeType"),
            Some(TEST_NODE_TYPE),
            "case {case:?}: the stored cluster must run that node type, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_create_that_joins_a_replication_group_is_not_refused_for_the_node_type() {
    // The requirement is CONDITIONAL: a cluster joining a replication group takes that
    // group's shape, so the member is not required of such a request — whatever else that
    // request is then answered with, it is never the missing-node-type refusal. The axis is
    // whether the named group exists.
    let emulator = Emulator::new();
    let group = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "joined-group"),
                ("ReplicationGroupDescription", "a group a cluster would join"),
                ("Engine", "redis"),
                ("CacheNodeType", TEST_NODE_TYPE),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "setup: the replication group must be created, got {} {}",
        group.status, group.body
    );
    let missing_sentence = required_sentence("CacheNodeType");
    for (case, group_id) in [
        ("a group the account holds", "joined-group"),
        ("a group that does not exist", "no-such-group"),
    ] {
        let reply = create(
            &emulator,
            &[
                ("CacheClusterId", "joining-cluster"),
                ("ReplicationGroupId", group_id),
            ],
        )
        .await;
        assert_ne!(
            reply.error_message(),
            missing_sentence,
            "case {case:?}: a create that joins a group must not be refused for the node type, \
             got {}",
            reply.body
        );
    }
}
