// SPDX-License-Identifier: CC0-1.0

//! The TEARDOWN a `DeleteReplicationGroup` begins ends by itself, on the service's own clock.
//!
//! The rule: a delete this service only BEGINS leaves the resource in its transient status for
//! a bounded span and then the resource CEASES TO EXIST, together with the resources it owns —
//! so a client that polls after an accepted delete converges: the by-id read of the group stops
//! being served and is refused the modelled not-found fault with that reader's own message, the
//! list form stops reporting it, every member cache cluster the delete took stops resolving in
//! `DescribeCacheClusters`, and the identifier is free for a new group again. `deleting` is a
//! status the group LEAVES by ceasing to exist, never a resting state. And because the removal
//! is owed by the CLOCK rather than by a reader, it holds for EVERY operation that resolves
//! such a resource by identity and not only for the describe: whichever call arrives first once
//! the teardown has fallen due is the one that must resolve it to nothing.
//!
//! What makes the first test different from [`crate::replication_group_delete`], which is about
//! what the delete ANSWERS and what the window shows: nothing in it touches the store. The due
//! instant of the teardown is the handler's own ([`state::REPLICATION_GROUP_TEARDOWN`]), and the
//! condition is looked for again through the shared poll helper until it holds or the suite's
//! generous deadline passes — so what passes there is a poll loop an SDK waiter or a teardown
//! script would run, not one a test nudged along.
//!
//! The rows span what the rule generalises over: how many shards the group is partitioned over
//! (one, two, three), how many members each shard holds, whether the keyspace is partitioned at
//! all, whether the group was placed in the account's VPC, whether the delete KEEPS one of the
//! member clusters (`RetainPrimaryCluster`) — which is what shows the teardown removes exactly
//! what the delete took and nothing else — and WHICH operation is the first to arrive once the
//! removal is due (a report, a catalogue read, a settings change, a second delete, a snapshot,
//! the three tagging calls, and the creates that reuse the freed identifier).
//!
//! Every assertion goes over the wire: `awsQuery` form-urlencoded requests through the
//! production router, read back as HTTP status plus XML body.

use service_com_amazonaws_elasticache_amazonelasticachev9::{service, state};

use crate::cache_cluster_lifecycle::{account_subnet_ids, subnet_group_params};
use crate::harness::{
    assert_refusal, cluster_create_params, params, poll_until, release_pending_changes,
    settle_pending_changes, Emulator, Reply, REGION, TEST_NODE_TYPE,
};

/// The cache subnet group the placed rows sit in — the account's own VPC, which is where the
/// cluster-mode group a client bootstraps from a configuration endpoint runs.
const SUBNET_GROUP: &str = "teardown-clock-subnets";

/// One row: the group to stand up, how the delete is sent, and the shape the teardown has to
/// leave behind.
struct Row {
    case: &'static str,
    id: &'static str,
    /// What the create sends beside the identity and the node type.
    extra: &'static [(&'static str, &'static str)],
    /// Whether the group is placed in [`SUBNET_GROUP`].
    placed: bool,
    /// How many member cache clusters the group is made of.
    members: usize,
    /// What the delete sends beside `ReplicationGroupId`.
    delete_extra: &'static [(&'static str, &'static str)],
    /// How many of those member clusters OUTLIVE the group: the primary a delete was asked to
    /// retain, and nothing else.
    survivors: usize,
}

const ROWS: &[Row] = &[
    // The issue's own shape: a partitioned keyspace, one node per shard, in the account's VPC.
    Row {
        case: "three shards in the account's VPC",
        id: "teardown-three-shards",
        extra: &[
            ("Engine", "redis"),
            ("ClusterMode", "enabled"),
            ("NumNodeGroups", "3"),
            ("ReplicasPerNodeGroup", "0"),
        ],
        placed: true,
        members: 3,
        delete_extra: &[("RetainPrimaryCluster", "false")],
        survivors: 0,
    },
    Row {
        case: "two shards with a replica each",
        id: "teardown-two-shards",
        extra: &[
            ("Engine", "redis"),
            ("ClusterMode", "enabled"),
            ("NumNodeGroups", "2"),
            ("ReplicasPerNodeGroup", "1"),
        ],
        placed: true,
        members: 4,
        delete_extra: &[],
        survivors: 0,
    },
    Row {
        case: "one unpartitioned group outside any VPC",
        id: "teardown-unpartitioned",
        extra: &[("Engine", "valkey"), ("NumCacheClusters", "2")],
        placed: false,
        members: 2,
        delete_extra: &[],
        survivors: 0,
    },
    Row {
        case: "a group whose primary the delete keeps",
        id: "teardown-retained-primary",
        extra: &[("Engine", "redis"), ("NumCacheClusters", "2")],
        placed: false,
        members: 2,
        delete_extra: &[("RetainPrimaryCluster", "true")],
        survivors: 1,
    },
];

/// Create `row`'s group and answer the member cache cluster ids the control plane reports for
/// it, once it is settled.
///
/// The create's own transient phase is not what this suite is about, so the change the clock
/// already owes the new group is carried through here exactly as the next call of the service
/// would ([`settle_pending_changes`]); the TEARDOWN below is left to the service's own clock.
async fn available_group(emulator: &Emulator, row: &Row) -> Vec<String> {
    let mut pairs = params(&[
        ("ReplicationGroupId", row.id),
        ("ReplicationGroupDescription", "a group this suite tears down"),
        ("CacheNodeType", TEST_NODE_TYPE),
    ]);
    pairs.extend(params(row.extra));
    if row.placed {
        pairs.extend(params(&[("CacheSubnetGroupName", SUBNET_GROUP)]));
    }
    let created = emulator.call("CreateReplicationGroup", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: the create must answer 200, got {} {}",
        row.case, created.status, created.body
    );
    settle_pending_changes(emulator);
    let described = poll_until(
        emulator,
        REGION,
        row.case,
        "the group reaching available",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", row.id)]),
        |reply| reply.text("ReplicationGroup/Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
    let members: Vec<String> = described
        .all("NodeGroupMember/CacheClusterId")
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        members.len(),
        row.members,
        "case {:?}: the group must be made of {} member clusters, got {members:?}",
        row.case,
        row.members
    );
    members
}

/// `DescribeCacheClusters` for one cluster id.
async fn describe_cluster(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", id)]),
        )
        .await
}

#[tokio::test]
async fn an_accepted_delete_removes_the_group_and_its_members_on_the_services_own_clock() {
    let emulator = Emulator::new();
    let subnets = account_subnet_ids(&emulator).await;
    let created = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params(
                SUBNET_GROUP,
                "the subnets the groups of this suite sit in",
                &subnets[..2],
            ),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: the cache subnet group must be created, got {} {}",
        created.status, created.body
    );

    for row in ROWS {
        let members = available_group(&emulator, row).await;

        // "When you receive a successful response from this operation, Amazon ElastiCache
        // immediately begins deleting the selected resources": the call answers the group in
        // the transient status, and nothing else is asked of the service from here on.
        let mut pairs = params(&[("ReplicationGroupId", row.id)]);
        pairs.extend(params(row.delete_extra));
        let deleted = emulator.call("DeleteReplicationGroup", &pairs).await;
        assert_eq!(
            (deleted.status, deleted.text("ReplicationGroup/Status")),
            (200, Some(state::STATUS_DELETING)),
            "case {:?}: the delete must answer the group going away, got {} {}",
            row.case,
            deleted.status,
            deleted.body
        );

        // The poll an SDK waiter and every teardown script runs: look again until the id stops
        // resolving. It has to TERMINATE — a group that rested in `deleting` would leave this
        // loop running until the deadline.
        let gone = poll_until(
            &emulator,
            REGION,
            row.case,
            "the deleted group ceasing to exist",
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", row.id)]),
            |reply| reply.status != 200,
        )
        .await;
        assert_refusal(
            row.case,
            &gone,
            404,
            "ReplicationGroupNotFoundFault",
            &state::replication_group_not_found_message(row.id),
        );

        // The list form answers from the same records, so it stops reporting the group too.
        let listed = emulator
            .call("DescribeReplicationGroups", &params(&[]))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: the list form must answer 200, got {} {}",
            row.case, listed.status, listed.body
        );
        let reported = listed.all("ReplicationGroup/ReplicationGroupId");
        assert!(
            !reported.contains(&row.id),
            "case {:?}: the list form must stop reporting {}, got {reported:?}",
            row.case,
            row.id
        );

        // Every member cluster the delete TOOK went with the group; a primary the request asked
        // to keep stayed behind, as a cluster of its own that belongs to no group any more.
        let mut survived: Vec<&String> = Vec::new();
        for member in &members {
            let cluster = describe_cluster(&emulator, member).await;
            if cluster.status == 200 {
                survived.push(member);
                assert_eq!(
                    cluster.text("CacheCluster/ReplicationGroupId"),
                    None,
                    "case {:?}: the kept cluster {member} must belong to no group, got {}",
                    row.case,
                    cluster.body
                );
                continue;
            }
            assert_refusal(
                row.case,
                &cluster,
                404,
                "CacheClusterNotFound",
                &state::cache_cluster_not_found_message(member),
            );
        }
        assert_eq!(
            survived.len(),
            row.survivors,
            "case {:?}: {} member cluster(s) must outlive the group, got {survived:?}",
            row.case,
            row.survivors
        );

        // And the consequence the client depends on: the identifier is free again, so a create
        // reusing it is not refused as a duplicate for ever. A row whose delete kept a member
        // cluster still holds that cluster's own name, which is a different id's business.
        if row.survivors == 0 {
            let mut remade = params(&[
                ("ReplicationGroupId", row.id),
                ("ReplicationGroupDescription", "the id is free again"),
                ("CacheNodeType", TEST_NODE_TYPE),
            ]);
            remade.extend(params(row.extra));
            if row.placed {
                remade.extend(params(&[("CacheSubnetGroupName", SUBNET_GROUP)]));
            }
            let again = emulator.call("CreateReplicationGroup", &remade).await;
            assert_eq!(
                again.status, 200,
                "case {:?}: the id must be free for a new group, got {} {}",
                row.case, again.status, again.body
            );
        }
    }
}

/// Stand up one single-cluster replication group under `id`, settled, and answer the member
/// cache cluster the control plane reports for it.
///
/// For the tables below, each of whose rows needs a group of its own: the operation under test
/// has to be the FIRST call the service takes once the teardown has fallen due, or another
/// call would have materialised the removal for it.
async fn settled_group(emulator: &Emulator, case: &str, id: &str) -> String {
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", "a group this suite tears down"),
                ("Engine", "redis"),
                ("CacheNodeType", TEST_NODE_TYPE),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    settle_pending_changes(emulator);
    let described = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await;
    assert_eq!(
        (described.status, described.text("ReplicationGroup/Status")),
        (200, Some(state::STATUS_AVAILABLE)),
        "case {case:?}: the group must be settled before it is deleted, got {} {}",
        described.status,
        described.body
    );
    described
        .text("NodeGroupMember/CacheClusterId")
        .unwrap_or_default()
        .to_string()
}

/// Accept the delete of `id` and make the removal the clock owes it DUE, without letting any
/// other call of the service observe it.
///
/// The service's own span is what [`an_accepted_delete_removes_the_group_and_its_members_on_the_services_own_clock`]
/// waits out; here only the due INSTANT is moved ([`release_pending_changes`]), so each row
/// below asks one question: does the operation under test materialise a removal that is
/// already due?
async fn delete_and_let_the_teardown_fall_due(emulator: &Emulator, case: &str, id: &str) {
    let deleted = emulator
        .call(
            "DeleteReplicationGroup",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await;
    assert_eq!(
        (deleted.status, deleted.text("ReplicationGroup/Status")),
        (200, Some(state::STATUS_DELETING)),
        "case {case:?}: the delete must answer the group going away, got {} {}",
        deleted.status,
        deleted.body
    );
    release_pending_changes(emulator);
}

/// What the operation under test names the resource with.
enum Names {
    /// `ReplicationGroupId=<the group>`.
    GroupId,
    /// `ResourceName=<the group's ARN>` — the tagging calls, which take an ARN.
    GroupArn,
    /// `CacheClusterId=<the group's member cluster>`.
    MemberId,
    /// `ResourceName=<that member cluster's ARN>`.
    MemberArn,
}

/// One row: the operation that is the only call made once the teardown has fallen due, and the
/// modelled refusal it has to answer.
struct Probe {
    case: &'static str,
    /// The group this row stands up and deletes — one per row; see [`settled_group`].
    id: &'static str,
    action: &'static str,
    names: Names,
    /// What the request carries beside the resource's identity.
    extra: &'static [(&'static str, &'static str)],
    /// The `awsQuery` error code of the refusal.
    code: &'static str,
    /// The refusal's sentence, built from the resource's own name — each operation words this
    /// fault its own way, which is why the row carries the template rather than the text.
    message: fn(&str) -> String,
}

/// The operations that resolve a replication group, or one of its member cache clusters, by
/// identity — every one of them must stop resolving a resource whose teardown the clock has
/// carried through, not only the describe the issue was filed against.
const PROBES: &[Probe] = &[
    Probe {
        case: "the by-id describe of the group",
        id: "gone-describe-rg",
        action: "DescribeReplicationGroups",
        names: Names::GroupId,
        extra: &[],
        code: "ReplicationGroupNotFoundFault",
        message: state::replication_group_not_found_message,
    },
    Probe {
        case: "the node types the group could scale to",
        id: "gone-allowed-rg",
        action: "ListAllowedNodeTypeModifications",
        names: Names::GroupId,
        extra: &[],
        code: "ReplicationGroupNotFoundFault",
        message: state::replication_group_not_found_message,
    },
    Probe {
        case: "a settings change of the group",
        id: "gone-modify-rg",
        action: "ModifyReplicationGroup",
        names: Names::GroupId,
        extra: &[("ReplicationGroupDescription", "a group that is gone")],
        code: "ReplicationGroupNotFoundFault",
        message: service::replication_group_not_found::quoted_message,
    },
    Probe {
        case: "a second delete of the group",
        id: "gone-delete-rg",
        action: "DeleteReplicationGroup",
        names: Names::GroupId,
        extra: &[],
        code: "ReplicationGroupNotFoundFault",
        message: |_| service::replication_group_not_found::DELETE_MESSAGE.to_string(),
    },
    Probe {
        case: "a snapshot of the group",
        id: "gone-snapshot-rg",
        action: "CreateSnapshot",
        names: Names::GroupId,
        extra: &[("SnapshotName", "a-snapshot-of-a-group-that-is-gone")],
        code: "ReplicationGroupNotFoundFault",
        message: state::replication_group_not_found_message,
    },
    Probe {
        case: "the tags of the group",
        id: "gone-list-tags-rg",
        action: "ListTagsForResource",
        names: Names::GroupArn,
        extra: &[],
        code: "ReplicationGroupNotFoundFault",
        message: not_present,
    },
    Probe {
        case: "tagging the group",
        id: "gone-add-tags-rg",
        action: "AddTagsToResource",
        names: Names::GroupArn,
        extra: &[("Tags.Tag.1.Key", "owner"), ("Tags.Tag.1.Value", "nobody")],
        code: "ReplicationGroupNotFoundFault",
        message: not_present,
    },
    Probe {
        case: "untagging the group",
        id: "gone-remove-tags-rg",
        action: "RemoveTagsFromResource",
        names: Names::GroupArn,
        extra: &[("TagKeys.member.1", "owner")],
        code: "ReplicationGroupNotFoundFault",
        message: not_present,
    },
    Probe {
        case: "the by-id describe of a member cluster",
        id: "gone-describe-member",
        action: "DescribeCacheClusters",
        names: Names::MemberId,
        extra: &[],
        code: "CacheClusterNotFound",
        message: state::cache_cluster_not_found_message,
    },
    Probe {
        case: "the node types a member cluster could scale to",
        id: "gone-allowed-member",
        action: "ListAllowedNodeTypeModifications",
        names: Names::MemberId,
        extra: &[],
        code: "CacheClusterNotFound",
        message: state::cache_cluster_not_found_message,
    },
    Probe {
        case: "the tags of a member cluster",
        id: "gone-tags-member",
        action: "ListTagsForResource",
        names: Names::MemberArn,
        extra: &[],
        code: "CacheClusterNotFound",
        message: not_present,
    },
];

/// How a tagging call words an ARN that names no resource of this service, which is what the
/// ARN of a torn-down resource is: the resource's own name and the rule it broke.
fn not_present(name: &str) -> String {
    format!("{name} is not present")
}

#[tokio::test]
async fn no_operation_resolves_a_group_whose_teardown_the_clock_has_carried_through() {
    let emulator = Emulator::new();
    for probe in PROBES {
        let member = settled_group(&emulator, probe.case, probe.id).await;
        assert!(
            !member.is_empty(),
            "case {:?}: the group must report the member cluster it is made of",
            probe.case
        );
        delete_and_let_the_teardown_fall_due(&emulator, probe.case, probe.id).await;

        let (resource, mut request) = match probe.names {
            Names::GroupId => (
                probe.id.to_string(),
                params(&[("ReplicationGroupId", probe.id)]),
            ),
            Names::GroupArn => (
                probe.id.to_string(),
                params(&[(
                    "ResourceName",
                    state::replication_group_arn(REGION, probe.id).as_str(),
                )]),
            ),
            Names::MemberId => (
                member.clone(),
                params(&[("CacheClusterId", member.as_str())]),
            ),
            Names::MemberArn => (
                member.clone(),
                params(&[(
                    "ResourceName",
                    state::cluster_arn(REGION, &member).as_str(),
                )]),
            ),
        };
        request.extend(params(probe.extra));
        let refused = emulator.call(probe.action, &request).await;
        assert_refusal(
            probe.case,
            &refused,
            404,
            probe.code,
            &(probe.message)(&resource),
        );
    }
}

/// One freed-identifier row: the case, the group that is torn down, and the create that reuses
/// an identifier the teardown gave back.
struct Freed {
    case: &'static str,
    id: &'static str,
    /// Whether the create reuses the GROUP's identifier (`CreateReplicationGroup`) or the
    /// member cluster's (`CreateCacheCluster`).
    group: bool,
}

const FREED: &[Freed] = &[
    Freed {
        case: "the group's own identifier",
        id: "freed-group-id",
        group: true,
    },
    Freed {
        case: "a member cluster's identifier",
        id: "freed-member-id",
        group: false,
    },
];

#[tokio::test]
async fn an_identifier_a_completed_teardown_gave_back_is_accepted_again() {
    // The consequence a client depends on, and the other half of the rule: a resource that has
    // ceased to exist no longer HOLDS its identifier, so the create that reuses it is served
    // rather than refused as a collision with a resource that is gone — and the create is the
    // first call the service takes once the teardown fell due, so nothing else materialised
    // the removal for it.
    let emulator = Emulator::new();
    for row in FREED {
        let member = settled_group(&emulator, row.case, row.id).await;
        delete_and_let_the_teardown_fall_due(&emulator, row.case, row.id).await;

        let (action, request) = match row.group {
            true => (
                "CreateReplicationGroup",
                params(&[
                    ("ReplicationGroupId", row.id),
                    ("ReplicationGroupDescription", "the identifier is free again"),
                    ("Engine", "redis"),
                    ("CacheNodeType", TEST_NODE_TYPE),
                ]),
            ),
            false => (
                "CreateCacheCluster",
                cluster_create_params(&member, &[("Engine", "redis"), ("NumCacheNodes", "1")]),
            ),
        };
        let created = emulator.call(action, &request).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: {action} must be served for an identifier the teardown gave back, got \
             {} {}",
            row.case, created.status, created.body
        );
    }
}
