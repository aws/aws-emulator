// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the nine ElastiCache operation families that used to
//! answer the scaffold's `NotImplemented`: the legacy cache-security-group
//! surface, the Global datastore reads, mutations and topology changes, reserved
//! cache node offerings and purchases, replica scaling, users, user groups,
//! serverless caches and the reboot state gate.
//!
//! Every assertion goes over the wire through [`crate::harness`], so only the
//! HTTP status and the XML body are inspected and a response that is not
//! computed from persisted state cannot pass. A replication group these tests
//! need is CREATED over the wire through `CreateReplicationGroup`; a Global
//! datastore is seeded into the one store the handlers read through
//! [`crate::harness::Emulator::with_state`], because these rows name it by its
//! stored id and give it secondary members only another region's create could
//! add — and everything then asserted about it comes back over the wire. The
//! wire path that MINTS one is `CreateGlobalReplicationGroup`, which the
//! `global_replication_group_create` suite drives end to end.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (the operation, the shape of an unknown id, a password
//! length, a usage limit at and past each published bound, a node count, a
//! cluster status), and the case each cause was recorded against is only one
//! row.

use std::time::SystemTime;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::poll_cluster_gone;
use crate::harness::{
    hold_pending_changes, is_own_host, params, release_pending_changes, Emulator, RefusalCase,
    Reply, PAST_TRANSITION, REGION,
};

/// One table row that drives an operation and asserts a count: the case name,
/// the form pairs the request carries, and what the answer must hold that many
/// of (slice groups, node-group members).
type CountCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    usize,
);

/// One `CreateUser` row: the case name, the form pairs that say how the user
/// authenticates, and the `Authentication$Type` / `$PasswordCount` it must then
/// report.
type AuthenticationCase = (
    &'static str,
    Vec<(String, String)>,
    &'static str,
    Option<&'static str>,
);

/// One reserved-node filter row: the case name, the form pairs that make the
/// filter, and how many of the purchases it must select.
type FilterCase<'a> = (&'static str, &'a Vec<(String, String)>, usize);

/// One reboot row: the case name, the form pairs the cluster is created with,
/// and the cache node ids the reboot names.
type RebootCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

/// The id the recorded `PurchaseReservedCacheNodesOffering` check probes with,
/// and one no catalogue row can mint.
const ABSENT_OFFERING_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Assert one refusal row: the status and the `awsQuery` error code.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (status, code),
        "case {case:?}: expected HTTP {status} {code}, got HTTP {} {:?} — {}",
        reply.status,
        reply.error_code(),
        reply.body
    );
}

/// Seed one Global datastore with `node_groups` slice groups and one secondary
/// member per entry in `secondaries`, settled at `available`.
///
/// The datastore is built through the state module `CreateGlobalReplicationGroup`
/// writes, rather than over the wire, because these suites name the datastore by
/// its STORED id (prefix and all) and seed secondary members that only another
/// region's `CreateReplicationGroup` could add; the status is settled here rather
/// than slept for, because the transition this test is about is the one each
/// MUTATION starts.
///
/// `pub(crate)` so the sibling suites of this target seed the one store the same
/// way instead of carrying a second copy of this.
pub(crate) fn seed_global_datastore(
    emulator: &Emulator,
    id: &str,
    node_groups: i32,
    secondaries: &[(&str, &str)],
) {
    emulator.with_state(|store| {
        store.insert_global_replication_group(state::NewGlobalReplicationGroup {
            global_replication_group_id: id.to_string(),
            description: Some(format!("{id} datastore")),
            region: REGION.to_string(),
            engine: state::DEFAULT_ENGINE.to_string(),
            engine_version: state::default_engine_version(state::DEFAULT_ENGINE).to_string(),
            cache_node_type: Some("cache.m7g.large".to_string()),
            primary_replication_group_id: format!("{id}-primary"),
            primary_automatic_failover: state::FAILOVER_DISABLED.to_string(),
            node_group_count: node_groups,
            cluster_enabled: node_groups > 1,
            auth_token_enabled: false,
            transit_encryption_enabled: Some(false),
            at_rest_encryption_enabled: Some(false),
            tags: Vec::new(),
            now: SystemTime::now(),
        });
        let group = store
            .global_replication_groups
            .get_mut(&id.to_ascii_lowercase())
            .expect("the datastore just seeded");
        for (replication_group_id, region) in secondaries {
            group.members.push(state::GlobalMemberRecord {
                replication_group_id: (*replication_group_id).to_string(),
                region: (*region).to_string(),
                role: state::GLOBAL_ROLE_SECONDARY.to_string(),
                automatic_failover: state::FAILOVER_DISABLED.to_string(),
                status: state::STATUS_ASSOCIATED.to_string(),
            });
        }
        group.status = state::STATUS_AVAILABLE.to_string();
        group.pending_status = None;
    });
}

/// Create one replication group with `node_groups` shards of
/// `replicas_per_node_group` replicas each, over the wire, and wait for the
/// clock to settle it at `available`.
///
/// `CreateReplicationGroup` is implemented, so a group this test needs is
/// created the way a client creates one — the replica-count assertions below
/// are then about the stored group, not about a record a test wrote.
async fn create_replication_group(
    emulator: &Emulator,
    id: &str,
    node_groups: i32,
    replicas_per_node_group: i32,
) {
    let reply = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", &format!("{id} group")),
                ("Engine", state::DEFAULT_ENGINE),
                ("CacheNodeType", "cache.m7g.large"),
                ("NumNodeGroups", &node_groups.to_string()),
                ("ReplicasPerNodeGroup", &replicas_per_node_group.to_string()),
            ]),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "the replication group {id} must be creatable, got {}",
        reply.body
    );
    // A created group is `creating` until the clock owes it `available`, and
    // every operation below needs a settled group.
    std::thread::sleep(PAST_TRANSITION);
}

// ───────────────────────── cause 1: cache security groups ────────────────────

#[tokio::test]
async fn every_cache_security_group_operation_is_refused_account_wide() {
    let emulator = Emulator::new();
    // The axis: the operation, and the name / EC2 security group it names. The
    // rule is account-wide, so no row can succeed and no row answers a
    // per-name not-found.
    let cases: [(&str, &[(&str, &str)]); 8] = [
        (
            "CreateCacheSecurityGroup",
            &[
                ("CacheSecurityGroupName", "mysecuritygroup"),
                ("Description", "my security group"),
            ],
        ),
        (
            "CreateCacheSecurityGroup",
            &[
                ("CacheSecurityGroupName", "Another-Group"),
                ("Description", "tagged"),
                ("Tags.Tag.1.Key", "team"),
                ("Tags.Tag.1.Value", "cache"),
            ],
        ),
        ("DescribeCacheSecurityGroups", &[]),
        (
            "DescribeCacheSecurityGroups",
            &[("CacheSecurityGroupName", "mysecuritygroup")],
        ),
        (
            "AuthorizeCacheSecurityGroupIngress",
            &[
                ("CacheSecurityGroupName", "mysecuritygroup"),
                ("EC2SecurityGroupName", "my-ec2-group"),
                ("EC2SecurityGroupOwnerId", "123456789012"),
            ],
        ),
        (
            "RevokeCacheSecurityGroupIngress",
            &[
                ("CacheSecurityGroupName", "mysecuritygroup"),
                ("EC2SecurityGroupName", "my-ec2-group"),
                ("EC2SecurityGroupOwnerId", "123456789012"),
            ],
        ),
        (
            "DeleteCacheSecurityGroup",
            &[("CacheSecurityGroupName", "mysecuritygroup")],
        ),
        (
            "DeleteCacheSecurityGroup",
            &[("CacheSecurityGroupName", "never-created")],
        ),
    ];

    for (action, pairs) in cases {
        let reply = emulator.call(action, &params(pairs)).await;
        assert_refused(action, &reply, 400, "InvalidParameterValue");
        assert_eq!(
            reply.error_message(),
            // The recorded wording, as a literal: the status and the code are
            // the requirement, and this pins that the message a client reads is
            // the account-wide closure rather than a per-name not-found.
            "Use of cache security groups is not permitted in this API version for your account.",
            "case {action:?}: the refusal must be the account-wide closure, got {:?}",
            reply.error_message()
        );
    }
}

#[tokio::test]
async fn a_create_naming_a_cache_security_group_is_refused_by_the_same_closure() {
    let emulator = Emulator::new();
    // The same rule, reached through the other operation that takes a cache
    // security group name.
    let reply = emulator
        .call(
            "CreateCacheCluster",
            &crate::harness::cluster_create_params(
                "closure-probe",
                &[("CacheSecurityGroupNames.CacheSecurityGroupName.1", "sg-a")],
            ),
        )
        .await;
    assert_refused("CreateCacheCluster", &reply, 400, "InvalidParameterValue");
    assert_eq!(
        reply.error_message(),
        state::CACHE_SECURITY_GROUPS_UNSUPPORTED,
        "the cluster create must answer the same closure, got {:?}",
        reply.error_message()
    );
}

// ──────────────── causes 2 and 3: Global datastore not-found ─────────────────

/// Every operation that resolves a `GlobalReplicationGroupId`, with the form
/// pairs that make it a well-formed request apart from the id.
const GLOBAL_OPERATIONS: [(&str, &[(&str, &str)]); 8] = [
    ("DescribeGlobalReplicationGroups", &[]),
    (
        "DeleteGlobalReplicationGroup",
        &[("RetainPrimaryReplicationGroup", "true")],
    ),
    (
        "ModifyGlobalReplicationGroup",
        &[
            ("ApplyImmediately", "true"),
            ("CacheNodeType", "cache.m7g.large"),
        ],
    ),
    (
        "DisassociateGlobalReplicationGroup",
        &[
            ("ReplicationGroupId", "secondary-group"),
            ("ReplicationGroupRegion", "us-west-2"),
        ],
    ),
    (
        "FailoverGlobalReplicationGroup",
        &[
            ("PrimaryRegion", "us-west-2"),
            ("PrimaryReplicationGroupId", "secondary-group"),
        ],
    ),
    (
        "IncreaseNodeGroupsInGlobalReplicationGroup",
        &[("NodeGroupCount", "4"), ("ApplyImmediately", "true")],
    ),
    (
        "DecreaseNodeGroupsInGlobalReplicationGroup",
        &[("NodeGroupCount", "1"), ("ApplyImmediately", "true")],
    ),
    (
        "RebalanceSlotsInGlobalReplicationGroup",
        &[("ApplyImmediately", "true")],
    ),
];

#[tokio::test]
async fn every_global_replication_group_operation_refuses_an_unknown_id() {
    let emulator = Emulator::new();
    // One datastore exists, so a refusal below is about the id the request
    // names and not about an empty store.
    seed_global_datastore(&emulator, "ldgnf-present", 2, &[]);

    // The axis: the operation, and the shape of the id. A `Marker`-shaped id, a
    // mixed-case one and a uuid-shaped one are all simply ids this account does
    // not hold.
    let unknown_ids = ["ldgnf-absent", "Mixed-Case-Absent", ABSENT_OFFERING_ID];
    for (action, pairs) in GLOBAL_OPERATIONS {
        for id in unknown_ids {
            let mut request = params(&[("GlobalReplicationGroupId", id)]);
            request.extend(params(pairs));
            let reply = emulator.call(action, &request).await;
            assert_refused(
                &format!("{action} {id}"),
                &reply,
                404,
                "GlobalReplicationGroupNotFoundFault",
            );
        }
    }
}

#[tokio::test]
async fn a_global_datastore_read_reports_members_only_when_asked() {
    let emulator = Emulator::new();
    seed_global_datastore(
        &emulator,
        "ldgnf-reads",
        2,
        &[("secondary-group", "us-west-2")],
    );

    // The axis: `ShowMemberInfo`, which is what decides whether the member list
    // is reported at all.
    for (case, show, expected_members) in [("absent", None, 0), ("false", Some("false"), 0), ("true", Some("true"), 2)]
    {
        let mut request = params(&[("GlobalReplicationGroupId", "ldgnf-reads")]);
        if let Some(show) = show {
            request.extend(params(&[("ShowMemberInfo", show)]));
        }
        let reply = emulator
            .call("DescribeGlobalReplicationGroups", &request)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the describe must succeed, got {}",
            reply.body
        );
        let roles = reply.all("Role");
        assert_eq!(
            roles.len(),
            expected_members,
            "case {case:?}: expected {expected_members} members, got {roles:?}"
        );
    }

    // The listing form answers the same datastore, and the id lookup is what
    // distinguishes a missing one.
    let listed = emulator
        .call("DescribeGlobalReplicationGroups", &params(&[]))
        .await;
    assert_eq!(
        listed.all("GlobalReplicationGroupId"),
        vec!["ldgnf-reads"],
        "the listing must report the stored datastore, got {}",
        listed.body
    );
}

#[tokio::test]
async fn global_datastore_topology_changes_are_computed_from_stored_state() {
    let emulator = Emulator::new();
    seed_global_datastore(
        &emulator,
        "ldgnf-topology",
        2,
        &[("secondary-group", "us-west-2")],
    );

    /// The keyspace a datastore's slice groups must cover, from
    /// `state::KEYSPACE_SLOTS`.
    fn assert_covers_keyspace(case: &str, slots: &[&str]) {
        let mut next = 0;
        for range in slots {
            let (start, end) = range
                .split_once('-')
                .unwrap_or_else(|| panic!("case {case:?}: {range:?} is not a slot range"));
            let start: i32 = start.parse().expect("slot range start");
            let end: i32 = end.parse().expect("slot range end");
            assert_eq!(
                start, next,
                "case {case:?}: slot ranges {slots:?} are not contiguous"
            );
            assert!(
                end >= start,
                "case {case:?}: slot range {range:?} is empty or inverted"
            );
            next = end + 1;
        }
        assert_eq!(
            next,
            state::KEYSPACE_SLOTS,
            "case {case:?}: slot ranges {slots:?} do not cover the keyspace"
        );
    }

    // The axis: the topology change, and the slice-group count it leaves. Each
    // row is applied to the datastore the row before it left behind, and each
    // waits out the transition the mutation started.
    let cases: [CountCase; 4] = [
        (
            "IncreaseNodeGroupsInGlobalReplicationGroup",
            &[("NodeGroupCount", "5"), ("ApplyImmediately", "true")],
            5,
        ),
        (
            "RebalanceSlotsInGlobalReplicationGroup",
            &[("ApplyImmediately", "true")],
            5,
        ),
        (
            "DecreaseNodeGroupsInGlobalReplicationGroup",
            &[("NodeGroupCount", "3"), ("ApplyImmediately", "true")],
            3,
        ),
        (
            "IncreaseNodeGroupsInGlobalReplicationGroup",
            &[("NodeGroupCount", "4"), ("ApplyImmediately", "true")],
            4,
        ),
    ];
    for (action, pairs, expected) in cases {
        let mut request = params(&[("GlobalReplicationGroupId", "ldgnf-topology")]);
        request.extend(params(pairs));
        let reply = emulator.call(action, &request).await;
        assert_eq!(
            reply.status, 200,
            "case {action:?}: the change must succeed, got {}",
            reply.body
        );
        assert_eq!(
            reply.text("Status"),
            Some(state::STATUS_MODIFYING),
            "case {action:?}: a change puts the datastore in {} first, got {}",
            state::STATUS_MODIFYING,
            reply.body
        );
        let slots = reply.all("Slots");
        assert_eq!(
            slots.len(),
            expected,
            "case {action:?}: expected {expected} slice groups, got {slots:?}"
        );
        assert_covers_keyspace(action, &slots);

        // The clock owes the datastore its settled status; the handler never
        // sleeps, so only the test waits.
        std::thread::sleep(PAST_TRANSITION);
        let settled = emulator
            .call(
                "DescribeGlobalReplicationGroups",
                &params(&[("GlobalReplicationGroupId", "ldgnf-topology")]),
            )
            .await;
        assert_eq!(
            settled.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {action:?}: the datastore must settle at {}, got {}",
            state::STATUS_AVAILABLE,
            settled.body
        );
        let settled_slots = settled.all("Slots");
        assert_eq!(
            settled_slots, slots,
            "case {action:?}: the read-back slice groups must be the stored ones"
        );
    }
}

#[tokio::test]
async fn a_failover_swaps_the_roles_a_global_datastore_reports() {
    let emulator = Emulator::new();
    seed_global_datastore(
        &emulator,
        "ldgnf-failover",
        1,
        &[("secondary-group", "us-west-2")],
    );

    // Promoting the member that is already primary, or one that is not a member
    // at all, changes nothing and is refused.
    let refusals: [(&str, &[(&str, &str)]); 3] = [
        (
            "already primary",
            &[
                ("PrimaryRegion", REGION),
                ("PrimaryReplicationGroupId", "ldgnf-failover-primary"),
            ],
        ),
        (
            "not a member",
            &[
                ("PrimaryRegion", "eu-west-1"),
                ("PrimaryReplicationGroupId", "nobody"),
            ],
        ),
        (
            "member in the wrong region",
            &[
                ("PrimaryRegion", "eu-west-1"),
                ("PrimaryReplicationGroupId", "secondary-group"),
            ],
        ),
    ];
    for (case, pairs) in refusals {
        let mut request = params(&[("GlobalReplicationGroupId", "ldgnf-failover")]);
        request.extend(params(pairs));
        let reply = emulator
            .call("FailoverGlobalReplicationGroup", &request)
            .await;
        assert_refused(case, &reply, 400, "InvalidParameterValue");
    }

    let promoted = emulator
        .call(
            "FailoverGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-failover"),
                ("PrimaryRegion", "us-west-2"),
                ("PrimaryReplicationGroupId", "secondary-group"),
            ]),
        )
        .await;
    assert_eq!(
        promoted.status, 200,
        "the failover must succeed, got {}",
        promoted.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let read_back = emulator
        .call(
            "DescribeGlobalReplicationGroups",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-failover"),
                ("ShowMemberInfo", "true"),
            ]),
        )
        .await;
    let members: Vec<(&str, &str)> = read_back
        .all("ReplicationGroupId")
        .into_iter()
        .zip(read_back.all("Role"))
        .collect();
    assert!(
        members.contains(&("secondary-group", state::GLOBAL_ROLE_PRIMARY))
            && members.contains(&("ldgnf-failover-primary", state::GLOBAL_ROLE_SECONDARY)),
        "the promoted member must be the primary and the old primary a secondary, got {members:?}"
    );
}

#[tokio::test]
async fn a_global_datastore_is_disassociated_down_to_its_primary_then_deleted() {
    let emulator = Emulator::new();
    seed_global_datastore(
        &emulator,
        "ldgnf-teardown",
        1,
        &[("secondary-group", "us-west-2")],
    );

    // "Deleting a Global datastore is a two-step process": while a secondary is
    // still attached, the delete is refused.
    let too_early = emulator
        .call(
            "DeleteGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-teardown"),
                ("RetainPrimaryReplicationGroup", "true"),
            ]),
        )
        .await;
    assert_refused(
        "delete with a secondary attached",
        &too_early,
        400,
        "InvalidGlobalReplicationGroupState",
    );

    // The primary cannot be disassociated, and a pair that is not a member is
    // refused; the secondary can.
    let refusals: [(&str, &[(&str, &str)]); 2] = [
        (
            "the primary",
            &[
                ("ReplicationGroupId", "ldgnf-teardown-primary"),
                ("ReplicationGroupRegion", REGION),
            ],
        ),
        (
            "not a member",
            &[
                ("ReplicationGroupId", "nobody"),
                ("ReplicationGroupRegion", "eu-west-1"),
            ],
        ),
    ];
    for (case, pairs) in refusals {
        let mut request = params(&[("GlobalReplicationGroupId", "ldgnf-teardown")]);
        request.extend(params(pairs));
        let reply = emulator
            .call("DisassociateGlobalReplicationGroup", &request)
            .await;
        assert_refused(case, &reply, 400, "InvalidParameterValue");
    }

    let removed = emulator
        .call(
            "DisassociateGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-teardown"),
                ("ReplicationGroupId", "secondary-group"),
                ("ReplicationGroupRegion", "us-west-2"),
            ]),
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "the disassociate must succeed, got {}",
        removed.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let settled = emulator
        .call(
            "DescribeGlobalReplicationGroups",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-teardown"),
                ("ShowMemberInfo", "true"),
            ]),
        )
        .await;
    assert_eq!(
        settled.text("Status"),
        Some(state::STATUS_PRIMARY_ONLY),
        "a datastore with no secondary settles at {}, got {}",
        state::STATUS_PRIMARY_ONLY,
        settled.body
    );
    assert_eq!(
        settled.all("Role"),
        vec![state::GLOBAL_ROLE_PRIMARY],
        "only the primary must be left, got {}",
        settled.body
    );

    let deleted = emulator
        .call(
            "DeleteGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-teardown"),
                ("RetainPrimaryReplicationGroup", "true"),
            ]),
        )
        .await;
    assert_eq!(
        deleted.text("Status"),
        Some(state::STATUS_DELETING),
        "the delete must report the datastore leaving, got {}",
        deleted.body
    );
    let gone = emulator
        .call(
            "DescribeGlobalReplicationGroups",
            &params(&[("GlobalReplicationGroupId", "ldgnf-teardown")]),
        )
        .await;
    assert_refused(
        "describe after delete",
        &gone,
        404,
        "GlobalReplicationGroupNotFoundFault",
    );
}

#[tokio::test]
async fn a_deferred_global_datastore_change_is_refused() {
    let emulator = Emulator::new();
    seed_global_datastore(&emulator, "ldgnf-deferred", 2, &[]);

    // The axis: every operation that takes `ApplyImmediately`. None of them can
    // report a pending change, so none of them accepts a deferred one.
    let cases: [(&str, &[(&str, &str)]); 4] = [
        (
            "ModifyGlobalReplicationGroup",
            &[("CacheNodeType", "cache.m7g.xlarge")],
        ),
        (
            "IncreaseNodeGroupsInGlobalReplicationGroup",
            &[("NodeGroupCount", "4")],
        ),
        (
            "DecreaseNodeGroupsInGlobalReplicationGroup",
            &[("NodeGroupCount", "1")],
        ),
        ("RebalanceSlotsInGlobalReplicationGroup", &[]),
    ];
    for (action, pairs) in cases {
        let mut request = params(&[
            ("GlobalReplicationGroupId", "ldgnf-deferred"),
            ("ApplyImmediately", "false"),
        ]);
        request.extend(params(pairs));
        let reply = emulator.call(action, &request).await;
        assert_refused(action, &reply, 400, "InvalidParameterValue");
    }

    // A change the datastore has nowhere to report is refused for the same
    // reason: a `ReshardingConfiguration` places a member's new shards in
    // chosen Availability Zones, and the `GlobalReplicationGroup` shape reports
    // only each slice group's id and slots.
    let resharding = emulator
        .call(
            "IncreaseNodeGroupsInGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-deferred"),
                ("ApplyImmediately", "true"),
                ("NodeGroupCount", "4"),
                (
                    "RegionalConfigurations.RegionalConfiguration.1.ReplicationGroupId",
                    "ldgnf-deferred-primary",
                ),
                (
                    "RegionalConfigurations.RegionalConfiguration.1.ReplicationGroupRegion",
                    REGION,
                ),
                (
                    "RegionalConfigurations.RegionalConfiguration.1.ReshardingConfiguration.ReshardingConfiguration.1.NodeGroupId",
                    "0003",
                ),
            ]),
        )
        .await;
    assert_refused(
        "a resharding configuration",
        &resharding,
        400,
        "InvalidParameterValue",
    );

    // A regional configuration that names a replication group the datastore
    // does not hold is refused before any of that.
    let not_a_member = emulator
        .call(
            "IncreaseNodeGroupsInGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-deferred"),
                ("ApplyImmediately", "true"),
                ("NodeGroupCount", "4"),
                (
                    "RegionalConfigurations.RegionalConfiguration.1.ReplicationGroupId",
                    "nobody",
                ),
                (
                    "RegionalConfigurations.RegionalConfiguration.1.ReplicationGroupRegion",
                    "eu-west-1",
                ),
                (
                    "RegionalConfigurations.RegionalConfiguration.1.ReshardingConfiguration.ReshardingConfiguration.1.NodeGroupId",
                    "0003",
                ),
            ]),
        )
        .await;
    assert_refused(
        "a regional configuration naming a non-member",
        &not_a_member,
        400,
        "InvalidParameterValue",
    );
}

#[tokio::test]
async fn a_modify_persists_what_it_was_given() {
    let emulator = Emulator::new();
    seed_global_datastore(
        &emulator,
        "ldgnf-modify",
        1,
        &[("secondary-group", "us-west-2")],
    );

    let modified = emulator
        .call(
            "ModifyGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-modify"),
                ("ApplyImmediately", "true"),
                ("CacheNodeType", "cache.r7g.large"),
                ("EngineVersion", "7.0"),
                ("GlobalReplicationGroupDescription", "moved"),
                ("AutomaticFailoverEnabled", "true"),
            ]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "the modify must succeed, got {}",
        modified.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let read_back = emulator
        .call(
            "DescribeGlobalReplicationGroups",
            &params(&[
                ("GlobalReplicationGroupId", "ldgnf-modify"),
                ("ShowMemberInfo", "true"),
            ]),
        )
        .await;
    for (element, expected) in [
        ("CacheNodeType", "cache.r7g.large"),
        ("EngineVersion", "7.0"),
        ("GlobalReplicationGroupDescription", "moved"),
    ] {
        assert_eq!(
            read_back.text(element),
            Some(expected),
            "case {element:?}: the change must be read back, got {}",
            read_back.body
        );
    }
    assert_eq!(
        read_back.all("AutomaticFailover"),
        vec![state::FAILOVER_ENABLED, state::FAILOVER_ENABLED],
        "every member must report the new failover setting, got {}",
        read_back.body
    );

    // An engine the Global datastore surface does not serve, and a parameter
    // group it has nowhere to report, are refused rather than dropped.
    for (case, pairs) in [
        ("engine", &[("Engine", "memcached")][..]),
        (
            "parameter group",
            &[("CacheParameterGroupName", "default.redis7")][..],
        ),
    ] {
        let mut request = params(&[
            ("GlobalReplicationGroupId", "ldgnf-modify"),
            ("ApplyImmediately", "true"),
        ]);
        request.extend(params(pairs));
        let reply = emulator
            .call("ModifyGlobalReplicationGroup", &request)
            .await;
        assert_refused(case, &reply, 400, "InvalidParameterValue");
    }
}

// ─────────────── cause 4: reserved cache node offerings ─────────────────────

/// One offering from the published catalog, as the members a purchase copies
/// from it.
struct Offering {
    id: String,
    cache_node_type: String,
    duration: String,
    product_description: String,
    offering_type: String,
    fixed_price: String,
}

/// Read the published catalog through the wire describe.
async fn published_offerings(emulator: &Emulator, filters: &[(&str, &str)]) -> Vec<Offering> {
    let reply = emulator
        .call("DescribeReservedCacheNodesOfferings", &params(filters))
        .await;
    assert_eq!(
        reply.status, 200,
        "the offerings describe must succeed, got {}",
        reply.body
    );
    let ids = reply.all("ReservedCacheNodesOfferingId");
    let node_types = reply.all("CacheNodeType");
    let durations = reply.all("Duration");
    let descriptions = reply.all("ProductDescription");
    let offering_types = reply.all("OfferingType");
    let fixed = reply.all("FixedPrice");
    assert!(
        !ids.is_empty(),
        "the catalog must publish offerings, got {}",
        reply.body
    );
    (0..ids.len())
        .map(|index| Offering {
            id: ids[index].to_string(),
            cache_node_type: node_types[index].to_string(),
            duration: durations[index].to_string(),
            product_description: descriptions[index].to_string(),
            offering_type: offering_types[index].to_string(),
            fixed_price: fixed[index].to_string(),
        })
        .collect()
}

#[tokio::test]
async fn purchasing_an_offering_is_refused_for_an_unknown_id_and_stored_for_a_published_one() {
    let emulator = Emulator::new();

    // The axis: the shape of the id. None of them is an offering this Region
    // publishes, so every one is the modelled not-found.
    for id in [
        ABSENT_OFFERING_ID,
        "ri-not-an-offering",
        "438012d3-4052-4cc7-b2e3-8d3372e0e706",
    ] {
        let reply = emulator
            .call(
                "PurchaseReservedCacheNodesOffering",
                &params(&[("ReservedCacheNodesOfferingId", id)]),
            )
            .await;
        assert_refused(id, &reply, 404, "ReservedCacheNodesOfferingNotFound");
    }

    // The axis: the offering bought, and whether the request names its own
    // reservation id and node count. The offerings are taken from across the
    // catalog rather than from its head.
    let catalogue = published_offerings(&emulator, &[]).await;
    let rows = [
        ("my-reservation", Some("3")),
        ("second-reservation", None),
        // "If this parameter is not specified, ElastiCache automatically
        // generates an identifier for the reservation."
        ("", None),
    ];
    // What was bought, so the filter rows below assert against the purchases
    // rather than against a count that depends on the catalog's order.
    let mut bought: Vec<&Offering> = Vec::new();
    for (index, (reservation_id, count)) in rows.into_iter().enumerate() {
        let offering = &catalogue[index * (catalogue.len() / rows.len())];
        bought.push(offering);
        let mut request = params(&[(
            "ReservedCacheNodesOfferingId",
            offering.id.as_str(),
        )]);
        if !reservation_id.is_empty() {
            request.extend(params(&[("ReservedCacheNodeId", reservation_id)]));
        }
        if let Some(count) = count {
            request.extend(params(&[("CacheNodeCount", count)]));
        }
        let purchased = emulator
            .call("PurchaseReservedCacheNodesOffering", &request)
            .await;
        assert_eq!(
            purchased.status, 200,
            "case {reservation_id:?}: the purchase must succeed, got {}",
            purchased.body
        );
        let stored_id = purchased
            .text("ReservedCacheNodeId")
            .expect("the reservation id")
            .to_string();
        if !reservation_id.is_empty() {
            assert_eq!(
                stored_id, reservation_id,
                "case {reservation_id:?}: the customer-specified id must be kept"
            );
        } else {
            assert!(
                stored_id.starts_with("ri-"),
                "case {reservation_id:?}: a generated id carries the ri- prefix, got {stored_id:?}"
            );
        }
        assert_eq!(
            purchased.text("State"),
            Some(state::RESERVATION_STATE_PAYMENT_PENDING),
            "case {reservation_id:?}: a purchase starts {}, got {}",
            state::RESERVATION_STATE_PAYMENT_PENDING,
            purchased.body
        );
        assert_eq!(
            purchased.text("CacheNodeCount"),
            Some(count.unwrap_or("1")),
            "case {reservation_id:?}: the node count must be the one asked for (default 1), got {}",
            purchased.body
        );

        // The round trip: the reservation reports the term and the prices of
        // the offering it was bought from, and the clock settles the charge.
        std::thread::sleep(PAST_TRANSITION);
        let read_back = emulator
            .call(
                "DescribeReservedCacheNodes",
                &params(&[("ReservedCacheNodeId", stored_id.as_str())]),
            )
            .await;
        assert_eq!(
            read_back.status, 200,
            "case {reservation_id:?}: the reservation must be read back, got {}",
            read_back.body
        );
        for (element, expected) in [
            ("ReservedCacheNodesOfferingId", offering.id.as_str()),
            ("CacheNodeType", offering.cache_node_type.as_str()),
            ("Duration", offering.duration.as_str()),
            ("ProductDescription", offering.product_description.as_str()),
            ("OfferingType", offering.offering_type.as_str()),
            ("FixedPrice", offering.fixed_price.as_str()),
            ("State", state::RESERVATION_STATE_ACTIVE),
        ] {
            assert_eq!(
                read_back.text(element),
                Some(expected),
                "case {reservation_id:?}: {element} must come from the offering, got {}",
                read_back.body
            );
        }
    }

    // A reservation id tracks one reservation, so it cannot be bought twice.
    let again = emulator
        .call(
            "PurchaseReservedCacheNodesOffering",
            &params(&[
                ("ReservedCacheNodesOfferingId", catalogue[0].id.as_str()),
                ("ReservedCacheNodeId", "my-reservation"),
            ]),
        )
        .await;
    assert_refused(
        "duplicate reservation id",
        &again,
        404,
        "ReservedCacheNodeAlreadyExists",
    );

    // The listing reports every purchase, and a reservation id nothing bought
    // is the modelled not-found.
    let listed = emulator
        .call("DescribeReservedCacheNodes", &params(&[]))
        .await;
    assert_eq!(
        listed.all("ReservedCacheNodeId").len(),
        3,
        "every purchase must be listed, got {}",
        listed.body
    );

    // The axis: the filter and the case it is written in. A reservation is
    // selected by the same five members the offerings describe filters on, and
    // the documented display values match whatever case the caller used. Each
    // row's expectation is counted from the purchases above.
    let one_year = crate::harness::params(&[("Duration", "1")]);
    let one_year_seconds = crate::harness::params(&[("Duration", "31536000")]);
    let three_years = crate::harness::params(&[("Duration", "3")]);
    let lowercased_type = crate::harness::params(&[(
        "OfferingType",
        bought[0].offering_type.to_ascii_lowercase().as_str(),
    )]);
    let first_node_type =
        crate::harness::params(&[("CacheNodeType", bought[0].cache_node_type.as_str())]);
    let cases: [FilterCase; 5] = [
        (
            "the term in years",
            &one_year,
            bought.iter().filter(|o| o.duration == "31536000").count(),
        ),
        (
            "the term in seconds",
            &one_year_seconds,
            bought.iter().filter(|o| o.duration == "31536000").count(),
        ),
        (
            "the other term",
            &three_years,
            bought.iter().filter(|o| o.duration == "94608000").count(),
        ),
        (
            "the offering type, lowercased",
            &lowercased_type,
            bought
                .iter()
                .filter(|o| o.offering_type == bought[0].offering_type)
                .count(),
        ),
        (
            "the node type the first purchase was for",
            &first_node_type,
            bought
                .iter()
                .filter(|o| o.cache_node_type == bought[0].cache_node_type)
                .count(),
        ),
    ];
    for (case, filters, expected) in cases {
        let filtered = emulator.call("DescribeReservedCacheNodes", filters).await;
        assert_eq!(
            filtered.status, 200,
            "case {case:?}: the filtered describe must succeed, got {}",
            filtered.body
        );
        assert_eq!(
            filtered.all("ReservedCacheNodeId").len(),
            expected,
            "case {case:?}: expected {expected} reservations, got {}",
            filtered.body
        );
    }
    // A value outside a filter's documented set is refused rather than
    // answered with an empty page.
    for (case, filters) in [
        ("duration", &[("Duration", "2")][..]),
        ("offering type", &[("OfferingType", "Free Lunch")][..]),
    ] {
        let reply = emulator
            .call("DescribeReservedCacheNodes", &params(filters))
            .await;
        assert_refused(case, &reply, 400, "InvalidParameterValue");
    }
    let missing = emulator
        .call(
            "DescribeReservedCacheNodes",
            &params(&[("ReservedCacheNodeId", "never-bought")]),
        )
        .await;
    assert_refused(
        "describe an unknown reservation",
        &missing,
        404,
        "ReservedCacheNodeNotFound",
    );
}

#[tokio::test]
async fn a_reservation_is_bounded_by_the_node_count_the_service_serves() {
    let emulator = Emulator::new();
    let catalogue = published_offerings(&emulator, &[]).await;

    // The axis: `CacheNodeCount`. It is bounded below by one node and above by
    // the account's reserved-node quota, counted over the nodes already
    // reserved, so neither one huge purchase nor many small ones can reserve
    // without a ceiling. Each refusal also asserts that nothing was reserved.
    let quota = i64::from(state::RESERVED_CACHE_NODE_QUOTA);
    for (case, count, expected_code) in [
        ("zero nodes", "0".to_string(), Some("InvalidParameterValue")),
        (
            "a negative count",
            "-1".to_string(),
            Some("InvalidParameterValue"),
        ),
        ("one node", "1".to_string(), None),
        (
            "every node left in the quota",
            (quota - 1).to_string(),
            None,
        ),
        (
            "one node past the quota",
            "1".to_string(),
            Some("ReservedCacheNodeQuotaExceeded"),
        ),
        (
            "a count nothing could reserve",
            "1000000".to_string(),
            Some("ReservedCacheNodeQuotaExceeded"),
        ),
    ] {
        let reply = emulator
            .call(
                "PurchaseReservedCacheNodesOffering",
                &params(&[
                    ("ReservedCacheNodesOfferingId", catalogue[0].id.as_str()),
                    ("ReservedCacheNodeId", case),
                    ("CacheNodeCount", count.as_str()),
                ]),
            )
            .await;
        match expected_code {
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: {count} nodes must be reservable, got {}",
                    reply.body
                );
                assert_eq!(
                    reply.text("CacheNodeCount"),
                    Some(count.as_str()),
                    "case {case:?}: the reservation must report the count it was bought with, \
                     got {}",
                    reply.body
                );
            }
            Some(code) => {
                assert_refused(case, &reply, 400, code);
                let read_back = emulator
                    .call(
                        "DescribeReservedCacheNodes",
                        &params(&[("ReservedCacheNodeId", case)]),
                    )
                    .await;
                assert_refused(case, &read_back, 404, "ReservedCacheNodeNotFound");
            }
        }
    }
}

/// The account id every ARN this emulator mints carries, from the framework's
/// own [`engine::emulator_account_id`] rather than a literal.
fn emulator_account() -> &'static str {
    engine::emulator_account_id()
}

// ───────────────────── cause 5: replica count validation ────────────────────

#[tokio::test]
async fn the_replica_count_operations_validate_arguments_before_resolving_the_group() {
    let emulator = Emulator::new();
    // A replication group that DOES exist, so a refusal below is about the
    // request and not about an empty store.
    create_replication_group(&emulator, "rg-present", 1, 1).await;

    // The axis: the operation, the argument that is wrong, and the id. Every
    // row names an id the account does not hold, and every row is answered 400
    // — the ordering difference this cause is about: a lookup-first operation
    // would answer `ReplicationGroupNotFoundFault` (404) for all of them.
    let cases: [RefusalCase; 10] = [
        (
            "Increase: valid argument, unknown group",
            &[
                ("ReplicationGroupId", "rg-absent"),
                ("NewReplicaCount", "2"),
                ("ApplyImmediately", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "Decrease: valid argument, unknown group",
            &[
                ("ReplicationGroupId", "rg-absent"),
                ("NewReplicaCount", "0"),
                ("ApplyImmediately", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "Increase: no count at all",
            &[
                ("ReplicationGroupId", "rg-absent"),
                ("ApplyImmediately", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "Increase: count above the published maximum",
            &[
                ("ReplicationGroupId", "rg-absent"),
                ("NewReplicaCount", "6"),
                ("ApplyImmediately", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "Increase: negative count",
            &[
                ("ReplicationGroupId", "rg-absent"),
                ("NewReplicaCount", "-1"),
                ("ApplyImmediately", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "Increase: both count members",
            &[
                ("ReplicationGroupId", "rg-absent"),
                ("NewReplicaCount", "2"),
                ("ReplicaConfiguration.ConfigureShard.1.NodeGroupId", "0001"),
                (
                    "ReplicaConfiguration.ConfigureShard.1.NewReplicaCount",
                    "2",
                ),
                ("ApplyImmediately", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "Decrease: both count members",
            &[
                ("ReplicationGroupId", "rg-absent"),
                ("NewReplicaCount", "0"),
                ("ReplicaConfiguration.ConfigureShard.1.NodeGroupId", "0001"),
                (
                    "ReplicaConfiguration.ConfigureShard.1.NewReplicaCount",
                    "0",
                ),
                ("ApplyImmediately", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "Increase: a deferred change",
            &[
                ("ReplicationGroupId", "rg-present"),
                ("NewReplicaCount", "2"),
                ("ApplyImmediately", "false"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "Increase: a node group the group does not have",
            &[
                ("ReplicationGroupId", "rg-present"),
                ("ReplicaConfiguration.ConfigureShard.1.NodeGroupId", "0007"),
                (
                    "ReplicaConfiguration.ConfigureShard.1.NewReplicaCount",
                    "2",
                ),
                ("ApplyImmediately", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "Increase: the count it already has",
            &[
                ("ReplicationGroupId", "rg-present"),
                ("NewReplicaCount", "1"),
                ("ApplyImmediately", "true"),
            ],
            400,
            "NoOperationFault",
        ),
    ];
    for (case, pairs, status, code) in cases {
        let action = if case.starts_with("Decrease") {
            "DecreaseReplicaCount"
        } else {
            "IncreaseReplicaCount"
        };
        let reply = emulator.call(action, &params(pairs)).await;
        assert_refused(case, &reply, status, code);
    }

    // The ordering is observable in the message: with a valid argument the
    // refusal is about the group, and with a bad one it is about the argument.
    let about_the_group = emulator
        .call(
            "IncreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", "rg-absent"),
                ("NewReplicaCount", "2"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert!(
        about_the_group.error_message().contains("rg-absent"),
        "the refusal must name the replication group, got {:?}",
        about_the_group.error_message()
    );
    let about_the_argument = emulator
        .call(
            "IncreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", "rg-absent"),
                ("NewReplicaCount", "6"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert!(
        !about_the_argument.error_message().contains("rg-absent")
            && about_the_argument
                .error_message()
                .contains("replica count must be between"),
        "argument validation runs first, so the refusal must state the replica-count rule rather \
         than the group, got {:?}",
        about_the_argument.error_message()
    );
}

#[tokio::test]
async fn replica_scaling_changes_the_members_a_replication_group_reports() {
    let emulator = Emulator::new();
    create_replication_group(&emulator, "rg-scaling", 2, 1).await;

    // The axis: the operation and how it names the target — one count for every
    // shard, or a per-shard configuration. The group has two shards of one
    // replica each, so the member count below is the total across both: a
    // replica IS a member cluster, which `MemberClusters` reports and
    // `DescribeCacheClusters` has to agree with.
    let cases: [CountCase; 3] = [
        (
            "IncreaseReplicaCount",
            &[("NewReplicaCount", "3"), ("ApplyImmediately", "true")],
            8,
        ),
        (
            "DecreaseReplicaCount",
            &[("NewReplicaCount", "1"), ("ApplyImmediately", "true")],
            4,
        ),
        (
            "IncreaseReplicaCount",
            &[
                ("ReplicaConfiguration.ConfigureShard.1.NodeGroupId", "0001"),
                (
                    "ReplicaConfiguration.ConfigureShard.1.NewReplicaCount",
                    "5",
                ),
                ("ApplyImmediately", "true"),
            ],
            8,
        ),
    ];
    for (action, pairs, expected_members) in cases {
        let mut request = params(&[("ReplicationGroupId", "rg-scaling")]);
        request.extend(params(pairs));
        let reply = emulator.call(action, &request).await;
        assert_eq!(
            reply.status, 200,
            "case {action:?}: the scaling must succeed, got {}",
            reply.body
        );
        assert_eq!(
            reply.text("Status"),
            Some(state::STATUS_MODIFYING),
            "case {action:?}: scaling puts the group in {} first, got {}",
            state::STATUS_MODIFYING,
            reply.body
        );
        std::thread::sleep(PAST_TRANSITION);

        let read_back = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", "rg-scaling")]),
            )
            .await;
        assert_eq!(
            read_back.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {action:?}: the group must settle at {}, got {}",
            state::STATUS_AVAILABLE,
            read_back.body
        );
        let members = read_back.all("ClusterId");
        assert_eq!(
            members.len(),
            expected_members,
            "case {action:?}: expected {expected_members} member clusters, got {members:?}"
        );
        let mut unique = members.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            members.len(),
            unique.len(),
            "case {action:?}: every member cluster id must be distinct, got {members:?}"
        );
        // A replica is a cache cluster of its own, so the cluster describe has
        // to report exactly the clusters the group says it is made of.
        let clusters = emulator
            .call("DescribeCacheClusters", &params(&[]))
            .await;
        let reported = clusters.all("CacheClusterId");
        for member in &members {
            assert!(
                reported.contains(member),
                "case {action:?}: member cluster {member} must be reported by \
                 DescribeCacheClusters, got {reported:?}"
            );
        }
    }

    // Removing a named replica takes that member out, and a name that is not a
    // replica of the group is refused. The first shard is the one with the
    // replicas to spare, so the removal is configured per shard; its members
    // are the ones whose id the group lists for shard 0001.
    let replicas = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", "rg-scaling")]),
        )
        .await;
    // `NodeGroupMembers` are reported shard by shard, the primary first, so the
    // second member of the first shard is a replica of it.
    let a_replica = replicas
        .all("CacheClusterId")
        .get(1)
        .map(|id| (*id).to_string())
        .expect("a replica of the first shard");
    let refused = emulator
        .call(
            "DecreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", "rg-scaling"),
                ("ReplicaConfiguration.ConfigureShard.1.NodeGroupId", "0001"),
                ("ReplicaConfiguration.ConfigureShard.1.NewReplicaCount", "4"),
                ("ReplicasToRemove.member.1", "not-a-member"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_refused("remove a non-member", &refused, 400, "InvalidParameterValue");
    let removed = emulator
        .call(
            "DecreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", "rg-scaling"),
                ("ReplicaConfiguration.ConfigureShard.1.NodeGroupId", "0001"),
                ("ReplicaConfiguration.ConfigureShard.1.NewReplicaCount", "4"),
                ("ReplicasToRemove.member.1", a_replica.as_str()),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "the named replica must be removable, got {}",
        removed.body
    );
    assert!(
        !removed.all("ClusterId").contains(&a_replica.as_str()),
        "the removed replica must be gone from the group, got {}",
        removed.body
    );
    // And its member cluster is gone with it: a replica removed is a cluster
    // deleted, so the cluster describe no longer reports it.
    let clusters = emulator
        .call("DescribeCacheClusters", &params(&[]))
        .await;
    assert!(
        !clusters.all("CacheClusterId").contains(&a_replica.as_str()),
        "the removed replica's member cluster must be gone, got {}",
        clusters.body
    );
}

// ─────────────────── causes 6 and 7: users and user groups ──────────────────

/// Create one user with a password, the way the positive rows below need it.
async fn create_user(emulator: &Emulator, user_id: &str, password: &str) -> Reply {
    emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", user_id),
                ("UserName", user_id),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("Passwords.member.1", password),
            ]),
        )
        .await
}

/// A password of exactly `length` printable ASCII characters.
fn password_of(length: usize) -> String {
    "aA1!bB2@cC3#dD4$eE5%fF6^gG7&hH8*iI9(jJ0)kKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ"
        .chars()
        .cycle()
        .take(length)
        .collect()
}

#[tokio::test]
async fn create_user_enforces_the_published_password_rules() {
    let emulator = Emulator::new();

    // The axis: the password length, at and past each published bound, plus the
    // count rule. The 7-character password this cause was recorded against is
    // one row.
    let refused: [(&str, Vec<(String, String)>); 6] = [
        ("one character", vec![("Passwords.member.1".to_string(), password_of(1))]),
        ("the recorded seven", vec![("Passwords.member.1".to_string(), "short12".to_string())]),
        (
            "one below the minimum",
            vec![(
                "Passwords.member.1".to_string(),
                password_of(state::PASSWORD_MIN_LENGTH - 1),
            )],
        ),
        (
            "one above the maximum",
            vec![(
                "Passwords.member.1".to_string(),
                password_of(state::PASSWORD_MAX_LENGTH + 1),
            )],
        ),
        (
            "one good and one short",
            vec![
                (
                    "Passwords.member.1".to_string(),
                    password_of(state::PASSWORD_MIN_LENGTH),
                ),
                ("Passwords.member.2".to_string(), password_of(4)),
            ],
        ),
        (
            "more passwords than a user may have",
            vec![
                (
                    "Passwords.member.1".to_string(),
                    password_of(state::PASSWORD_MIN_LENGTH),
                ),
                (
                    "Passwords.member.2".to_string(),
                    password_of(state::PASSWORD_MIN_LENGTH + 1),
                ),
                (
                    "Passwords.member.3".to_string(),
                    password_of(state::PASSWORD_MIN_LENGTH + 2),
                ),
            ],
        ),
    ];
    for (case, passwords) in refused {
        let mut request = params(&[
            ("UserId", "weak-user"),
            ("UserName", "weak-user"),
            ("Engine", "redis"),
            ("AccessString", "on ~* +@all"),
        ]);
        request.extend(passwords);
        let reply = emulator.call("CreateUser", &request).await;
        assert_refused(case, &reply, 400, "InvalidParameterValue");
        // And no user was created: the describe answers the modelled not-found.
        let read_back = emulator
            .call("DescribeUsers", &params(&[("UserId", "weak-user")]))
            .await;
        assert_refused(case, &read_back, 404, "UserNotFound");
    }

    // The accepted rows: each published bound, and each way of saying how the
    // user authenticates.
    let accepted: [AuthenticationCase; 4] = [
        (
            "at the minimum",
            vec![(
                "Passwords.member.1".to_string(),
                password_of(state::PASSWORD_MIN_LENGTH),
            )],
            state::authentication_type::PASSWORD,
            Some("1"),
        ),
        (
            "at the maximum",
            vec![(
                "Passwords.member.1".to_string(),
                password_of(state::PASSWORD_MAX_LENGTH),
            )],
            state::authentication_type::PASSWORD,
            Some("1"),
        ),
        (
            "two passwords",
            vec![
                (
                    "Passwords.member.1".to_string(),
                    password_of(state::PASSWORD_MIN_LENGTH),
                ),
                (
                    "Passwords.member.2".to_string(),
                    password_of(state::PASSWORD_MAX_LENGTH),
                ),
            ],
            state::authentication_type::PASSWORD,
            Some("2"),
        ),
        (
            "no password required",
            vec![("NoPasswordRequired".to_string(), "true".to_string())],
            state::authentication_type::NO_PASSWORD,
            None,
        ),
    ];
    for (index, (case, extra, expected_type, expected_count)) in
        accepted.into_iter().enumerate()
    {
        let user_id = format!("user-{index}");
        let mut request = params(&[
            ("UserId", user_id.as_str()),
            ("UserName", user_id.as_str()),
            ("Engine", "valkey"),
            ("AccessString", "on ~app:* +@read"),
        ]);
        request.extend(extra);
        let created = emulator.call("CreateUser", &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the user must be created, got {}",
            created.body
        );
        let read_back = emulator
            .call("DescribeUsers", &params(&[("UserId", user_id.as_str())]))
            .await;
        assert_eq!(
            read_back.text("UserId"),
            Some(user_id.as_str()),
            "case {case:?}: the user must be read back, got {}",
            read_back.body
        );
        assert_eq!(
            read_back.text("Type"),
            Some(expected_type),
            "case {case:?}: expected authentication type {expected_type:?}, got {}",
            read_back.body
        );
        assert_eq!(
            read_back.text("PasswordCount"),
            expected_count,
            "case {case:?}: expected password count {expected_count:?}, got {}",
            read_back.body
        );
        assert_eq!(
            read_back.text("AccessString"),
            // The EXPANDED form of the rule list given: a list that states no command
            // baseline is stored with the deny-all term before its command rules (see
            // `state::canonical_access_string` and the normalization table in
            // `rbac_user_access_string_and_deletion.rs`).
            Some("on ~app:* -@all +@read"),
            "case {case:?}: the access string must be the expanded form of the one given, got {}",
            read_back.body
        );
        assert_eq!(
            read_back.text("ARN"),
            Some(
                format!(
                    "arn:aws:elasticache:{REGION}:{}:user:{user_id}",
                    emulator_account()
                )
                .as_str()
            ),
            "case {case:?}: the ARN must name the user, got {}",
            read_back.body
        );
    }

    // The account's built-in default user is reported without anyone creating
    // it, and the engines without access control are refused.
    let default_user = emulator
        .call(
            "DescribeUsers",
            &params(&[("UserId", state::DEFAULT_USER_ID)]),
        )
        .await;
    assert_eq!(
        default_user.text("UserId"),
        Some(state::DEFAULT_USER_ID),
        "every account already has the default user, got {}",
        default_user.body
    );
    for engine in ["memcached", "postgres", ""] {
        let reply = emulator
            .call(
                "CreateUser",
                &params(&[
                    ("UserId", "engineless"),
                    ("UserName", "engineless"),
                    ("Engine", engine),
                    ("AccessString", "on ~* +@all"),
                    ("NoPasswordRequired", "true"),
                ]),
            )
            .await;
        assert_refused(engine, &reply, 400, "InvalidParameterValue");
    }

    // A user ID identifies a user, so it cannot be reused. A user NAME does not:
    // uniqueness is on the id, and a group needs a member whose name is `default`
    // (see `tests/all/cache_membership_rules.rs`), so a second user may carry a
    // name another holds.
    let duplicate_id = create_user(&emulator, "user-0", &password_of(20)).await;
    assert_refused("duplicate user id", &duplicate_id, 400, "UserAlreadyExists");
    let duplicate_name = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "other-id"),
                ("UserName", "user-0"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        duplicate_name.status, 200,
        "case a name another user holds: expected HTTP 200, got {} with {}",
        duplicate_name.status, duplicate_name.body
    );
}

#[tokio::test]
async fn a_user_group_is_refused_when_a_member_user_does_not_exist() {
    let emulator = Emulator::new();
    assert_eq!(
        create_user(&emulator, "known-user", &password_of(20)).await.status,
        200,
        "the member user must be created first"
    );

    // The axis: which member is a ghost, and where in the list it sits. The
    // `['default', <ghost>]` case this cause was recorded against is one row.
    let cases: [(&str, &[(&str, &str)]); 4] = [
        (
            "the recorded default plus a ghost",
            &[
                ("UserIds.member.1", "default"),
                ("UserIds.member.2", "ghost-user"),
            ],
        ),
        (
            "the ghost first",
            &[
                ("UserIds.member.1", "ghost-user"),
                ("UserIds.member.2", "default"),
            ],
        ),
        (
            "a ghost among known members",
            &[
                ("UserIds.member.1", "default"),
                ("UserIds.member.2", "known-user"),
                ("UserIds.member.3", "another-ghost"),
            ],
        ),
        ("only a ghost", &[("UserIds.member.1", "ghost-user")]),
    ];
    for (index, (case, members)) in cases.into_iter().enumerate() {
        let group_id = format!("group-{index}");
        let mut request = params(&[("UserGroupId", group_id.as_str()), ("Engine", "redis")]);
        request.extend(params(members));
        let reply = emulator.call("CreateUserGroup", &request).await;
        assert_refused(case, &reply, 404, "UserNotFound");
        // And the group was not created.
        let read_back = emulator
            .call(
                "DescribeUserGroups",
                &params(&[("UserGroupId", group_id.as_str())]),
            )
            .await;
        assert_refused(case, &read_back, 404, "UserGroupNotFound");
    }

    // A group that leaves the default user out is refused too, and a member
    // named twice is a duplicate.
    let no_default = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "no-default"),
                ("Engine", "redis"),
                ("UserIds.member.1", "known-user"),
            ]),
        )
        .await;
    assert_refused("no default user", &no_default, 400, "DefaultUserRequired");
    let duplicated = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "duplicated"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
                ("UserIds.member.2", "Default"),
            ]),
        )
        .await;
    assert_refused("a member named twice", &duplicated, 400, "DuplicateUserName");

    // The restore leg: the same call succeeds once every member exists, and the
    // group is read back from the stored state.
    let created = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "good-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
                ("UserIds.member.2", "known-user"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the group must be created once its members exist, got {}",
        created.body
    );
    assert_eq!(
        created.text("Status"),
        Some(state::STATUS_CREATING),
        "a new group is not ready yet, got {}",
        created.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let read_back = emulator
        .call(
            "DescribeUserGroups",
            &params(&[("UserGroupId", "good-group")]),
        )
        .await;
    assert_eq!(
        read_back.text("Status"),
        Some(state::USER_STATUS_ACTIVE),
        "the clock owes the group {}, got {}",
        state::USER_STATUS_ACTIVE,
        read_back.body
    );
    assert_eq!(
        read_back.all("UserIds/member"),
        vec!["default", "known-user"],
        "the group must report its stored members, got {}",
        read_back.body
    );
    // The membership is on the user too.
    let member = emulator
        .call("DescribeUsers", &params(&[("UserId", "known-user")]))
        .await;
    assert_eq!(
        member.all("UserGroupIds/member"),
        vec!["good-group"],
        "the user must report the group it was put in, got {}",
        member.body
    );
    // And the group id identifies a group, so it cannot be reused.
    let again = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "good-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
            ]),
        )
        .await;
    assert_refused("duplicate group id", &again, 400, "UserGroupAlreadyExists");
}

// ───────────────── cause 8: serverless cache usage limits ───────────────────

/// `CreateServerlessCache` with the usage-limit pairs a row names.
async fn create_serverless_cache(
    emulator: &Emulator,
    name: &str,
    limits: &[(&str, &str)],
) -> Reply {
    let mut request = params(&[("ServerlessCacheName", name), ("Engine", "redis")]);
    request.extend(params(limits));
    emulator.call("CreateServerlessCache", &request).await
}

#[tokio::test]
async fn create_serverless_cache_enforces_the_usage_limit_rules() {
    let emulator = Emulator::new();

    // The axis: the member, the bound and which side of it the value is on. The
    // `DataStorage.Maximum=-5` and the inverted `50 > 10` cases this cause was
    // recorded against are two rows. The bounds are the ones
    // `state::validate_cache_usage_limits` publishes: `DataStorage` is bounded
    // at both ends, `ECPUPerSecond` only below, and either member takes zero to
    // remove the limit.
    let refused: [(&str, &[(&str, &str)]); 8] = [
        (
            "the recorded negative maximum",
            &[
                ("CacheUsageLimits.DataStorage.Maximum", "-5"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ],
        ),
        (
            "one below the storage minimum",
            &[
                ("CacheUsageLimits.DataStorage.Minimum", "-1"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ],
        ),
        (
            "one above the storage maximum",
            &[
                ("CacheUsageLimits.DataStorage.Maximum", "5001"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ],
        ),
        (
            "the recorded inverted storage limits",
            &[
                ("CacheUsageLimits.DataStorage.Minimum", "50"),
                ("CacheUsageLimits.DataStorage.Maximum", "10"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ],
        ),
        (
            "inverted storage limits one apart",
            &[
                ("CacheUsageLimits.DataStorage.Minimum", "2"),
                ("CacheUsageLimits.DataStorage.Maximum", "1"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ],
        ),
        (
            "inverted ECPU limits",
            &[
                ("CacheUsageLimits.ECPUPerSecond.Minimum", "200000"),
                ("CacheUsageLimits.ECPUPerSecond.Maximum", "1000"),
            ],
        ),
        (
            "a bad limit beside a good one",
            &[
                ("CacheUsageLimits.DataStorage.Maximum", "100"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.ECPUPerSecond.Maximum", "-5"),
            ],
        ),
        (
            "a negative ECPU minimum",
            &[("CacheUsageLimits.ECPUPerSecond.Minimum", "-1")],
        ),
    ];
    for (case, limits) in refused {
        let reply = create_serverless_cache(&emulator, "limited-cache", limits).await;
        assert_refused(case, &reply, 400, "InvalidParameterValue");
        // And nothing was created.
        let read_back = emulator
            .call(
                "DescribeServerlessCaches",
                &params(&[("ServerlessCacheName", "limited-cache")]),
            )
            .await;
        assert_refused(case, &read_back, 404, "ServerlessCacheNotFoundFault");
    }

    // The accepted rows: each published bound, and the zero that removes a
    // limit.
    let accepted: [(&str, &[(&str, &str)]); 7] = [
        (
            "at the storage bounds",
            &[
                ("CacheUsageLimits.DataStorage.Minimum", "1"),
                ("CacheUsageLimits.DataStorage.Maximum", "5000"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ],
        ),
        (
            "equal storage bounds",
            &[
                ("CacheUsageLimits.DataStorage.Minimum", "10"),
                ("CacheUsageLimits.DataStorage.Maximum", "10"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ],
        ),
        (
            "zero removes the limit",
            &[
                ("CacheUsageLimits.DataStorage.Maximum", "0"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ],
        ),
        (
            "at the ECPU bounds this service serves",
            &[
                ("CacheUsageLimits.ECPUPerSecond.Minimum", "1000"),
                ("CacheUsageLimits.ECPUPerSecond.Maximum", "15000000"),
            ],
        ),
        (
            "an ECPU floor of one, which the API bounds only below",
            &[("CacheUsageLimits.ECPUPerSecond.Minimum", "1")],
        ),
        (
            "zero removes the ECPU limit too",
            &[("CacheUsageLimits.ECPUPerSecond.Maximum", "0")],
        ),
        ("no limits at all", &[]),
    ];
    for (index, (case, limits)) in accepted.into_iter().enumerate() {
        let name = format!("cache-{index}");
        let created = create_serverless_cache(&emulator, &name, limits).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the cache must be created, got {}",
            created.body
        );
        assert_eq!(
            created.text("Status"),
            Some(state::SERVERLESS_STATUS_CREATING),
            "case {case:?}: a new cache is not ready yet, got {}",
            created.body
        );
        std::thread::sleep(PAST_TRANSITION);

        let read_back = emulator
            .call(
                "DescribeServerlessCaches",
                &params(&[("ServerlessCacheName", name.as_str())]),
            )
            .await;
        assert_eq!(
            read_back.text("Status"),
            Some(state::SERVERLESS_STATUS_AVAILABLE),
            "case {case:?}: the clock owes the cache {}, got {}",
            state::SERVERLESS_STATUS_AVAILABLE,
            read_back.body
        );
        // The limits the request asked for are the ones reported back.
        for (member, value) in limits {
            let element = member.rsplit('.').next().expect("a member name");
            if element == "Unit" {
                continue;
            }
            assert!(
                read_back.all(element).contains(value),
                "case {case:?}: {member} must be read back as {value:?}, got {}",
                read_back.body
            );
        }
        let addresses = read_back.all("Address");
        assert_eq!(
            addresses.len(),
            2,
            "case {case:?}: a ready cache has a write and a read endpoint, got {addresses:?}"
        );
        assert!(
            addresses.iter().all(|address| is_own_host(address)),
            "case {case:?}: the endpoints must be addresses the emulator answers on, got \
             {addresses:?}"
        );
        assert_eq!(
            read_back.text("StorageEncryptionType"),
            Some(state::STORAGE_ENCRYPTION_SSE_ELASTICACHE),
            "case {case:?}: a cache with no KMS key is encrypted with the service key, got {}",
            read_back.body
        );
    }

    // The name identifies the cache, so it cannot be created twice.
    let again = create_serverless_cache(&emulator, "cache-0", &[]).await;
    assert_refused(
        "duplicate serverless cache name",
        &again,
        400,
        "ServerlessCacheAlreadyExistsFault",
    );

    // A user group the account does not hold is the modelled not-found, and a
    // Memcached cache cannot carry one at all.
    let unknown_group = emulator
        .call(
            "CreateServerlessCache",
            &params(&[
                ("ServerlessCacheName", "with-group"),
                ("Engine", "valkey"),
                ("UserGroupId", "never-created"),
            ]),
        )
        .await;
    assert_refused("unknown user group", &unknown_group, 404, "UserGroupNotFound");
    let memcached_group = emulator
        .call(
            "CreateServerlessCache",
            &params(&[
                ("ServerlessCacheName", "with-group"),
                ("Engine", "memcached"),
                ("UserGroupId", "never-created"),
            ]),
        )
        .await;
    assert_refused(
        "a user group on memcached",
        &memcached_group,
        400,
        "InvalidParameterCombination",
    );
}

// ───────────────────────── cause 9: the reboot gate ─────────────────────────

#[tokio::test]
async fn a_cluster_is_rebooted_only_from_a_status_the_service_reboots_from() {
    let emulator = Emulator::new();

    // The axis: the engine and node count, which is what decides how many nodes
    // there are to name. Each row is created, refused while `creating`,
    // rebooted once `available`, and refused again once deleted.
    let cases: [RebootCase; 3] = [
        ("redis-one", &[], &["0001"]),
        (
            "memcached-three",
            &[("Engine", "memcached"), ("NumCacheNodes", "3")],
            &["0001", "0002", "0003"],
        ),
        (
            "memcached-subset",
            &[("Engine", "memcached"), ("NumCacheNodes", "4")],
            &["0002", "0004"],
        ),
    ];
    for (id, extra, nodes) in cases {
        let create = crate::harness::cluster_create_params(id, extra);
        let created = emulator.call("CreateCacheCluster", &create).await;
        assert_eq!(
            created.status, 200,
            "case {id:?}: the cluster must be created, got {}",
            created.body
        );
        assert_eq!(
            created.text("CacheClusterStatus"),
            Some(state::STATUS_CREATING),
            "case {id:?}: a new cluster is not ready yet, got {}",
            created.body
        );

        // The gate: the identical call is refused while the cluster is still
        // being created. The clock owes a just-created cluster its settled status
        // from the call that created it, so that phase is held out of reach here
        // rather than raced for.
        hold_pending_changes(&emulator);
        let mut reboot = params(&[("CacheClusterId", id)]);
        for (index, node) in nodes.iter().enumerate() {
            reboot.extend(params(&[(
                &format!("CacheNodeIdsToReboot.CacheNodeId.{}", index + 1),
                node,
            )]));
        }
        let too_early = emulator.call("RebootCacheCluster", &reboot).await;
        assert_refused(id, &too_early, 400, "InvalidCacheClusterState");
        assert!(
            too_early.error_message().contains(state::STATUS_CREATING),
            "case {id:?}: the refusal must name the status the cluster has, got {:?}",
            too_early.error_message()
        );

        // And the same call succeeds once the clock owes the cluster
        // `available`.
        release_pending_changes(&emulator);
        let rebooted = emulator.call("RebootCacheCluster", &reboot).await;
        assert_eq!(
            rebooted.status, 200,
            "case {id:?}: the reboot must succeed once available, got {}",
            rebooted.body
        );
        assert_eq!(
            rebooted.text("CacheClusterStatus"),
            Some(state::STATUS_REBOOTING_NODES),
            "case {id:?}: a reboot is visible as {}, got {}",
            state::STATUS_REBOOTING_NODES,
            rebooted.body
        );
        // Only the nodes the request named are restarting, in the node's own
        // status vocabulary.
        let restarting = rebooted
            .all("CacheNodeStatus")
            .into_iter()
            .filter(|status| *status == state::NODE_STATUS_REBOOTING)
            .count();
        assert_eq!(
            restarting,
            nodes.len(),
            "case {id:?}: expected {} nodes restarting, got {}",
            nodes.len(),
            rebooted.body
        );

        // A reboot while the cluster is rebooting is refused by the same gate,
        // over the same held phase.
        hold_pending_changes(&emulator);
        let during = emulator.call("RebootCacheCluster", &reboot).await;
        assert_refused(id, &during, 400, "InvalidCacheClusterState");

        release_pending_changes(&emulator);
        let settled = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[("CacheClusterId", id), ("ShowCacheNodeInfo", "true")]),
            )
            .await;
        assert_eq!(
            settled.text("CacheClusterStatus"),
            Some(state::STATUS_AVAILABLE),
            "case {id:?}: the cluster must settle back at {}, got {}",
            state::STATUS_AVAILABLE,
            settled.body
        );

        // A node id the cluster does not have is a bad parameter value, not a
        // reboot of whatever nodes it does have.
        let unknown_node = emulator
            .call(
                "RebootCacheCluster",
                &params(&[
                    ("CacheClusterId", id),
                    ("CacheNodeIdsToReboot.CacheNodeId.1", "0099"),
                ]),
            )
            .await;
        assert_refused(id, &unknown_node, 400, "ValidationException");

        // And a reboot of a cluster whose teardown has BEGUN is refused for the
        // state it is in, until the clock has removed it — from when the
        // cluster is gone rather than unavailable.
        let deleted = emulator
            .call("DeleteCacheCluster", &params(&[("CacheClusterId", id)]))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {id:?}: the cluster must be deletable, got {}",
            deleted.body
        );
        let tearing_down = emulator.call("RebootCacheCluster", &reboot).await;
        assert_refused(id, &tearing_down, 400, "InvalidCacheClusterState");
        poll_cluster_gone(&emulator, id, id).await;
        let gone = emulator.call("RebootCacheCluster", &reboot).await;
        assert_refused(id, &gone, 404, "CacheClusterNotFound");
    }
}

// ───────────────────────── paging, across the surfaces ──────────────────────

#[tokio::test]
async fn a_page_the_describes_serve_resumes_where_it_ended() {
    let emulator = Emulator::new();
    // Enough users for a page to be partial at the smallest size the shared
    // describes serve.
    for index in 0..state::MAX_RECORDS_MIN + 5 {
        let user_id = format!("paged-user-{index:03}");
        assert_eq!(
            create_user(&emulator, &user_id, &password_of(20)).await.status,
            200,
            "case {user_id:?}: the user must be created"
        );
    }

    let first = emulator
        .call(
            "DescribeUsers",
            &params(&[("MaxRecords", &state::MAX_RECORDS_MIN.to_string())]),
        )
        .await;
    let first_ids: Vec<String> = first
        .all("UserId")
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        first_ids.len(),
        usize::try_from(state::MAX_RECORDS_MIN).expect("a page size"),
        "the first page must hold MaxRecords users, got {}",
        first.body
    );
    let marker = first
        .marker()
        .expect("a partial page carries a marker")
        .to_string();
    let second = emulator
        .call("DescribeUsers", &params(&[("Marker", marker.as_str())]))
        .await;
    let second_ids: Vec<String> = second
        .all("UserId")
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        second_ids.iter().all(|id| !first_ids.contains(id)),
        "the second page must not repeat the first: {first_ids:?} then {second_ids:?}"
    );
    assert_eq!(
        first_ids.len() + second_ids.len(),
        // Every user created above, plus the account's built-in default user.
        usize::try_from(state::MAX_RECORDS_MIN + 6).expect("a user count"),
        "the two pages together must hold every user, got {}",
        second.body
    );

    // A marker this service did not mint is a bad parameter value, not an empty
    // page.
    let bogus = emulator
        .call("DescribeUsers", &params(&[("Marker", "not-a-marker")]))
        .await;
    assert_eq!(
        bogus.status, 400,
        "a marker this service did not mint must be refused, got {}",
        bogus.body
    );

    // The serverless describe pages on its own members.
    let serverless = emulator
        .call("DescribeServerlessCaches", &params(&[("MaxResults", "0")]))
        .await;
    assert_refused("MaxResults 0", &serverless, 400, "InvalidParameterValue");
}
