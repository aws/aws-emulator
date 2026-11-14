// SPDX-License-Identifier: CC0-1.0

//! `ModifyReplicationGroup`: the rule a settings change follows, over the wire.
//!
//! The rule under test: `ModifyReplicationGroup` applies every member of the
//! request to the ONE stored replication group — the members the modelled
//! `ReplicationGroupPendingModifiedValues` carries are governed by
//! `ApplyImmediately` and reported under `PendingModifiedValues` until applied,
//! every other member has nowhere to be reported as pending and is applied at
//! once — carries the change into the group's member clusters, answers the group
//! read back out of the store, and leaves it in the transient status the
//! asynchronous work runs in until the clock settles it. A request this service
//! cannot satisfy is refused with the modelled error for the thing that is wrong
//! and persists nothing.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, using the harness in [`crate::harness`]; nothing reaches
//! into the service's state struct, so a response that is not computed from
//! persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule
//! generalises over (which member is changed, its value, whether the change is
//! applied immediately or deferred, the shape of the group it is applied to),
//! and a single `SnapshotRetentionLimit` change is only one row.

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// An `AuthToken` that satisfies every documented constraint (16 to 128
/// printable characters, only the permitted specials).
const GOOD_AUTH_TOKEN: &str = "Abcdefgh12345678";

/// Create one replication group, asserting the create itself was accepted.
async fn create_group(emulator: &Emulator, case: &str, id: &str, extra: &[(&str, &str)]) {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        (
            "ReplicationGroupDescription",
            "the ModifyReplicationGroup suite",
        ),
    ]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateReplicationGroup", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateReplicationGroup {id} must answer 200, got {} {}",
        created.status, created.body
    );
}

/// Create one user group out of the built-in `default` user, asserting the
/// create itself was accepted.
async fn create_user_group(emulator: &Emulator, case: &str, id: &str) {
    let created = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", id),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateUserGroup {id} must answer 200, got {} {}",
        created.status, created.body
    );
}

/// One `ModifyReplicationGroup` call on `id`.
async fn modify(emulator: &Emulator, id: &str, pairs: &[(&str, &str)]) -> Reply {
    let mut request = params(&[("ReplicationGroupId", id)]);
    request.extend(params(pairs));
    emulator.call("ModifyReplicationGroup", &request).await
}

/// One `ModifyReplicationGroup` call that must be accepted.
async fn modify_accepted(emulator: &Emulator, case: &str, id: &str, pairs: &[(&str, &str)]) -> Reply {
    let answered = modify(emulator, id, pairs).await;
    assert_eq!(
        answered.status, 200,
        "case {case}: ModifyReplicationGroup must answer 200, got {} {}",
        answered.status, answered.body
    );
    answered
}

/// Read one replication group back, asserting the describe itself succeeded.
async fn describe_group(emulator: &Emulator, case: &str, id: &str) -> Reply {
    let described = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeReplicationGroups must answer 200, got {} {}",
        described.status, described.body
    );
    described
}

/// Read one cache cluster back, asserting the describe itself succeeded.
async fn describe_cluster(emulator: &Emulator, case: &str, id: &str) -> Reply {
    let described = emulator
        .call("DescribeCacheClusters", &params(&[("CacheClusterId", id)]))
        .await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeCacheClusters {id} must answer 200, got {} {}",
        described.status, described.body
    );
    described
}

/// Assert `reply` reports `value` at every element path in `expected`.
fn assert_reports(case: &str, source: &str, reply: &Reply, expected: &[(&str, &str)]) {
    for (path, value) in expected {
        assert_eq!(
            reply.text(path),
            Some(*value),
            "case {case}: {source} must report {path} as {value:?}, got {:?} in {}",
            reply.text(path),
            reply.body
        );
    }
}

/// One row of the applied-at-once table.
#[derive(Debug, Clone)]
struct AppliedCase {
    case: &'static str,
    /// The form pairs the group is created with.
    extra: &'static [(&'static str, &'static str)],
    /// The form pairs sent to `ModifyReplicationGroup` beside the id.
    modify: &'static [(&'static str, &'static str)],
    /// What the group must report afterwards, both in the modify's own answer
    /// and in a later describe.
    expected: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_setting_the_pending_shape_cannot_hold_is_applied_at_once_and_read_back() {
    // The axis: WHICH member is changed and to what. Every row below changes a
    // member the modelled `ReplicationGroupPendingModifiedValues` does not
    // carry, so there is nowhere to report it as pending and it is applied
    // whatever `ApplyImmediately` says — which the rows vary too (omitted,
    // false, true).
    let cases = [
        AppliedCase {
            case: "the description",
            extra: &[],
            modify: &[("ReplicationGroupDescription", "a re-described group")],
            expected: &[("ReplicationGroup/Description", "a re-described group")],
        },
        AppliedCase {
            case: "backups turned on, deferred",
            extra: &[],
            modify: &[
                ("SnapshotRetentionLimit", "1"),
                ("ApplyImmediately", "false"),
            ],
            expected: &[("ReplicationGroup/SnapshotRetentionLimit", "1")],
        },
        AppliedCase {
            case: "backups turned on, immediately",
            extra: &[],
            modify: &[
                ("SnapshotRetentionLimit", "2"),
                ("ApplyImmediately", "true"),
            ],
            expected: &[("ReplicationGroup/SnapshotRetentionLimit", "2")],
        },
        AppliedCase {
            case: "the published retention bound",
            extra: &[],
            modify: &[("SnapshotRetentionLimit", "35")],
            expected: &[("ReplicationGroup/SnapshotRetentionLimit", "35")],
        },
        AppliedCase {
            // "If the value of SnapshotRetentionLimit is set to zero (0),
            // backups are turned off."
            case: "backups turned off",
            extra: &[("SnapshotRetentionLimit", "5")],
            modify: &[("SnapshotRetentionLimit", "0")],
            expected: &[("ReplicationGroup/SnapshotRetentionLimit", "0")],
        },
        AppliedCase {
            case: "the snapshot window",
            extra: &[],
            modify: &[("SnapshotWindow", "07:00-08:00")],
            expected: &[("ReplicationGroup/SnapshotWindow", "07:00-08:00")],
        },
        AppliedCase {
            case: "the minor-version opt-in",
            extra: &[],
            modify: &[("AutoMinorVersionUpgrade", "true")],
            expected: &[("ReplicationGroup/AutoMinorVersionUpgrade", "true")],
        },
        AppliedCase {
            case: "the network discovery",
            extra: &[],
            modify: &[("IpDiscovery", "ipv6")],
            expected: &[("ReplicationGroup/IpDiscovery", "ipv6")],
        },
        AppliedCase {
            // "Use this parameter to change the durability mode of an existing
            // replication group, for example from sync to async or vice versa."
            case: "durability sync",
            extra: &[],
            modify: &[("Durability", "sync")],
            expected: &[
                ("ReplicationGroup/Durability", "sync"),
                ("ReplicationGroup/EffectiveDurability", "sync"),
            ],
        },
        AppliedCase {
            // "When Durability is set to default, the service resolves the
            // actual durability": the resolved value is what
            // `EffectiveDurability` reports.
            case: "durability back to default",
            extra: &[("Durability", "sync")],
            modify: &[("Durability", "default")],
            expected: &[
                ("ReplicationGroup/Durability", "default"),
                ("ReplicationGroup/EffectiveDurability", "async"),
            ],
        },
        AppliedCase {
            case: "scaled to another node type",
            extra: &[("CacheNodeType", "cache.t3.micro")],
            modify: &[("CacheNodeType", "cache.m5.large"), ("ApplyImmediately", "true")],
            expected: &[("ReplicationGroup/CacheNodeType", "cache.m5.large")],
        },
        AppliedCase {
            // "Modifies the engine listed in a replication group message", and
            // an engine name "is stored as a lowercase string".
            case: "moved onto the other engine",
            extra: &[],
            modify: &[("Engine", "VALKEY")],
            expected: &[("ReplicationGroup/Engine", "valkey")],
        },
        AppliedCase {
            // "You can upgrade to a newer engine version": a group created on an
            // older published version takes a newer one.
            case: "upgraded to a newer engine version",
            extra: &[("EngineVersion", "6.2")],
            modify: &[("EngineVersion", "7.0")],
            expected: &[("ReplicationGroup/Engine", "redis")],
        },
        AppliedCase {
            // Multi-AZ "is the promotion of a replica in another Availability
            // Zone", so it goes on together with the failover it needs.
            case: "multi-az and automatic failover together",
            extra: &[("NumCacheClusters", "2")],
            modify: &[
                ("AutomaticFailoverEnabled", "true"),
                ("MultiAZEnabled", "true"),
                ("ApplyImmediately", "true"),
            ],
            expected: &[
                ("ReplicationGroup/MultiAZ", "enabled"),
                ("ReplicationGroup/AutomaticFailover", "enabled"),
            ],
        },
        AppliedCase {
            // "The cluster ID that is used as the daily snapshot source for the
            // replication group": a member cluster of the group.
            case: "the daily snapshot source",
            extra: &[("NumCacheClusters", "2")],
            modify: &[("SnapshottingClusterId", "snapshot-source-002")],
            expected: &[(
                "ReplicationGroup/SnapshottingClusterId",
                "snapshot-source-002",
            )],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        // The daily-snapshot-source row names a member cluster, which is
        // derived from the group id, so that row is created under the id it
        // names.
        let id = if case.case == "the daily snapshot source" {
            "snapshot-source".to_string()
        } else {
            format!("applied-{index}")
        };
        create_group(&emulator, case.case, &id, case.extra).await;
        std::thread::sleep(PAST_TRANSITION);

        let answered = modify_accepted(&emulator, case.case, &id, case.modify).await;
        assert_reports(case.case, "the answer", &answered, case.expected);

        // The answer and a later describe are both built from the one stored
        // group, so they cannot disagree — and the change outlives the
        // transition the asynchronous rows start.
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_group(&emulator, case.case, &id).await;
        assert_reports(case.case, "a later describe", &described, case.expected);
        assert_eq!(
            described.text("ReplicationGroup/Status"),
            Some("available"),
            "case {}: the group must settle back to available, got {:?}",
            case.case,
            described.text("ReplicationGroup/Status")
        );
    }
}

/// One row of the deferred-versus-immediate table.
#[derive(Debug, Clone)]
struct DeferredCase {
    case: &'static str,
    /// The form pairs the group is created with.
    extra: &'static [(&'static str, &'static str)],
    /// A user group this row needs to exist before the call.
    user_group: Option<&'static str>,
    /// The form pairs sent beside the id, WITHOUT `ApplyImmediately`.
    modify: &'static [(&'static str, &'static str)],
    /// What `PendingModifiedValues` must report while the change is deferred.
    pending: &'static [(&'static str, &'static str)],
    /// What the group must report once the change is applied.
    applied: &'static [(&'static str, &'static str)],
    /// What the group still reports while the change is only pending — `None`
    /// for an element it must not report at all yet.
    still: &'static [(&'static str, Option<&'static str>)],
    /// Whether [`Self::pending`] is STILL reported after an immediate apply:
    /// true only for a change `ApplyImmediately` does not finish, which is the
    /// auth-token ROTATE — "ROTATE" adds the new password beside the old one
    /// and "SET - allowed only after ROTATE" is what ends that window, so the
    /// rotation is outstanding until the caller finalizes it however this call
    /// was applied.
    pending_survives_apply: bool,
}

#[tokio::test]
async fn a_change_the_pending_shape_carries_is_deferred_or_applied_by_apply_immediately() {
    // The axis: which member the modelled
    // `ReplicationGroupPendingModifiedValues` carries is changed — the cluster
    // mode, the automatic failover, the auth token, the in-transit encryption
    // mode, the user groups — and, for each, whether `ApplyImmediately` is false
    // (the documented default) or true.
    let cases = [
        DeferredCase {
            // "To modify cluster mode from Disabled to Enabled, you must first
            // set the cluster mode to Compatible."
            case: "the cluster mode",
            extra: &[],
            user_group: None,
            modify: &[("ClusterMode", "compatible")],
            pending: &[("PendingModifiedValues/ClusterMode", "compatible")],
            applied: &[("ReplicationGroup/ClusterMode", "compatible")],
            still: &[("ReplicationGroup/ClusterMode", Some("disabled"))],
            pending_survives_apply: false,
        },
        DeferredCase {
            case: "the automatic failover",
            extra: &[("NumCacheClusters", "2")],
            user_group: None,
            modify: &[("AutomaticFailoverEnabled", "true")],
            pending: &[(
                "PendingModifiedValues/AutomaticFailoverStatus",
                "enabled",
            )],
            applied: &[("ReplicationGroup/AutomaticFailover", "enabled")],
            still: &[("ReplicationGroup/AutomaticFailover", Some("disabled"))],
            pending_survives_apply: false,
        },
        DeferredCase {
            // "ROTATE - default, if no update strategy is provided", reported as
            // `ROTATING` while the rotation is outstanding.
            case: "the auth token",
            extra: &[("TransitEncryptionEnabled", "true")],
            user_group: None,
            modify: &[("AuthToken", GOOD_AUTH_TOKEN)],
            pending: &[("PendingModifiedValues/AuthTokenStatus", "ROTATING")],
            applied: &[("ReplicationGroup/AuthTokenEnabled", "true")],
            still: &[("ReplicationGroup/AuthTokenEnabled", Some("false"))],
            pending_survives_apply: true,
        },
        DeferredCase {
            case: "the in-transit encryption mode",
            extra: &[("TransitEncryptionEnabled", "true")],
            user_group: None,
            modify: &[("TransitEncryptionMode", "preferred")],
            pending: &[(
                "PendingModifiedValues/TransitEncryptionMode",
                "preferred",
            )],
            applied: &[("ReplicationGroup/TransitEncryptionMode", "preferred")],
            still: &[("ReplicationGroup/TransitEncryptionMode", None)],
            pending_survives_apply: false,
        },
        DeferredCase {
            case: "the user group",
            extra: &[],
            user_group: Some("deferred-rbac"),
            modify: &[("UserGroupIdsToAdd.member.1", "deferred-rbac")],
            pending: &[(
                "PendingModifiedValues/UserGroups/UserGroupIdsToAdd/member",
                "deferred-rbac",
            )],
            applied: &[("ReplicationGroup/UserGroupIds/member", "deferred-rbac")],
            still: &[("ReplicationGroup/UserGroupIds/member", None)],
            pending_survives_apply: false,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        for (apply, label) in [
            (None, "ApplyImmediately omitted"),
            (Some("false"), "ApplyImmediately false"),
            (Some("true"), "ApplyImmediately true"),
        ] {
            let name = format!("{} with {label}", case.case);
            let emulator = Emulator::new();
            let id = format!("deferred-{index}-{}", apply.unwrap_or("default"));
            create_group(&emulator, &name, &id, case.extra).await;
            if let Some(user_group) = case.user_group {
                create_user_group(&emulator, &name, user_group).await;
            }
            std::thread::sleep(PAST_TRANSITION);

            let mut pairs: Vec<(&str, &str)> = case.modify.to_vec();
            if let Some(apply) = apply {
                pairs.push(("ApplyImmediately", apply));
            }
            let answered = modify_accepted(&emulator, &name, &id, &pairs).await;
            // Whatever the answer said, the stored group is what a later read
            // reports; both are asserted, because both are built from it.
            std::thread::sleep(PAST_TRANSITION);
            let described = describe_group(&emulator, &name, &id).await;

            if apply == Some("true") {
                for (source, reply) in [("the answer", &answered), ("a later describe", &described)]
                {
                    assert_reports(&name, source, reply, case.applied);
                    if case.pending_survives_apply {
                        // The one change `ApplyImmediately` does not finish; see
                        // `DeferredCase::pending_survives_apply`.
                        assert_reports(&name, source, reply, case.pending);
                        continue;
                    }
                    for (path, _) in case.pending {
                        assert_eq!(
                            reply.text(path),
                            None,
                            "case {name}: an applied change reports nothing pending at {path}, \
                             got {:?}",
                            reply.text(path)
                        );
                    }
                }
            } else {
                for (source, reply) in [("the answer", &answered), ("a later describe", &described)]
                {
                    assert_reports(&name, source, reply, case.pending);
                    for (path, value) in case.still {
                        assert_eq!(
                            reply.text(path),
                            *value,
                            "case {name}: a deferred change leaves {path} at {value:?}, got {:?}",
                            reply.text(path)
                        );
                    }
                }
                // Deferred, so no asynchronous work started: the group is still
                // available rather than transiently modifying.
                assert_eq!(
                    described.text("ReplicationGroup/Status"),
                    Some("available"),
                    "case {name}: a deferred change leaves the group available, got {:?}",
                    described.text("ReplicationGroup/Status")
                );
            }
        }
    }
}

/// One row of the promotion table: the group id, the pairs it is created with,
/// its member clusters in stored order, and the member promoted to primary.
type PromotionCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
    &'static str,
);

#[tokio::test]
async fn primary_cluster_id_promotes_that_member_and_demotes_the_primary_it_had() {
    // The axis: how many clusters the group holds, and WHICH of its replicas is
    // named — so the promotion is not keyed to one shape or to the first
    // replica.
    let cases: [PromotionCase; 3] = [
        (
            "promote-two",
            &[("NumCacheClusters", "2")],
            &["promote-two-001", "promote-two-002"],
            "promote-two-002",
        ),
        (
            "promote-three-second",
            &[("NumCacheClusters", "3")],
            &[
                "promote-three-second-001",
                "promote-three-second-002",
                "promote-three-second-003",
            ],
            "promote-three-second-002",
        ),
        (
            "promote-three-third",
            &[("NumCacheClusters", "3")],
            &[
                "promote-three-third-001",
                "promote-three-third-002",
                "promote-three-third-003",
            ],
            "promote-three-third-003",
        ),
    ];

    for (id, extra, members, promoted) in cases {
        let emulator = Emulator::new();
        create_group(&emulator, id, id, extra).await;
        std::thread::sleep(PAST_TRANSITION);

        let described = describe_group(&emulator, id, id).await;
        assert_eq!(
            described.all("NodeGroupMember/CacheClusterId"),
            members.to_vec(),
            "case {id}: the group's members must be the stored ones before the promotion"
        );

        let answered = modify_accepted(
            &emulator,
            id,
            id,
            &[("PrimaryClusterId", promoted), ("ApplyImmediately", "true")],
        )
        .await;

        // The promoted member holds the primary role, the primary it had is a
        // replica, and this service stores a shard's members primary-first.
        let demoted = members[0];
        let mut expected: Vec<&str> = vec![promoted, demoted];
        expected.extend(members.iter().skip(1).filter(|member| **member != promoted));
        std::thread::sleep(PAST_TRANSITION);
        let settled = describe_group(&emulator, id, id).await;
        for (source, reply) in [("the answer", &answered), ("a later describe", &settled)] {
            assert_eq!(
                reply.all("NodeGroupMember/CacheClusterId"),
                expected,
                "case {id}: {source} must report {promoted} as the primary, with {demoted} demoted"
            );
            let mut roles = vec!["primary"];
            roles.extend(members.iter().skip(1).map(|_| "replica"));
            assert_eq!(
                reply.all("NodeGroupMember/CurrentRole"),
                roles,
                "case {id}: {source} must report exactly one primary"
            );
        }

        // A promotion is asynchronous work, so the group was transiently
        // modifying and the clock has settled it back.
        assert_eq!(
            answered.text("ReplicationGroup/Status"),
            Some("modifying"),
            "case {id}: the answer must report the status the promotion runs in, got {:?}",
            answered.text("ReplicationGroup/Status")
        );
        assert_eq!(
            settled.text("ReplicationGroup/Status"),
            Some("available"),
            "case {id}: the group must settle back to available, got {:?}",
            settled.text("ReplicationGroup/Status")
        );
    }
}

/// One row of the user-group association table: the case name, the modify
/// pairs, and the membership the group must report afterwards.
type AssociationStep = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

#[tokio::test]
async fn a_user_group_association_is_held_once_and_reported_from_both_ends() {
    // The axis: one group added, two added at once, one removed by id, and every
    // group removed with `RemoveUserGroups`.
    let emulator = Emulator::new();
    create_group(&emulator, "rbac", "rbac-rg", &[]).await;
    for user_group in ["rbac-one", "rbac-two"] {
        create_user_group(&emulator, "rbac", user_group).await;
    }
    std::thread::sleep(PAST_TRANSITION);

    let steps: [AssociationStep; 4] = [
        (
            "one group added",
            &[
                ("UserGroupIdsToAdd.member.1", "rbac-one"),
                ("ApplyImmediately", "true"),
            ],
            &["rbac-one"],
        ),
        (
            "a second group added",
            &[
                ("UserGroupIdsToAdd.member.1", "rbac-two"),
                ("ApplyImmediately", "true"),
            ],
            &["rbac-one", "rbac-two"],
        ),
        (
            "one group removed by id",
            &[
                ("UserGroupIdsToRemove.member.1", "rbac-one"),
                ("ApplyImmediately", "true"),
            ],
            &["rbac-two"],
        ),
        (
            "every group removed",
            &[("RemoveUserGroups", "true"), ("ApplyImmediately", "true")],
            &[],
        ),
    ];

    for (case, pairs, expected) in steps {
        let answered = modify_accepted(&emulator, case, "rbac-rg", pairs).await;
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_group(&emulator, case, "rbac-rg").await;
        for (source, reply) in [("the answer", &answered), ("a later describe", &described)] {
            assert_eq!(
                reply.all("UserGroupIds/member"),
                expected.to_vec(),
                "case {case}: {source} must report the stored membership"
            );
        }

        // The other end of the same membership: "the replication groups that the
        // user group is associated to", held once, on the user group.
        for user_group in ["rbac-one", "rbac-two"] {
            let read = emulator
                .call(
                    "DescribeUserGroups",
                    &params(&[("UserGroupId", user_group)]),
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {case}: DescribeUserGroups {user_group} must answer 200, got {} {}",
                read.status, read.body
            );
            let associated = expected.contains(&user_group);
            assert_eq!(
                read.all("ReplicationGroups/member"),
                if associated { vec!["rbac-rg"] } else { Vec::new() },
                "case {case}: user group {user_group} must report the replication groups it is \
                 associated to"
            );
        }
    }
}

#[tokio::test]
async fn a_group_wide_setting_is_carried_into_the_member_clusters() {
    // The axis: the members a replication group's clusters carry — "the cache
    // parameter group to apply to all of the clusters in this replication
    // group", the maintenance window, the VPC security groups, the SNS topic and
    // the snapshot settings — read back through `DescribeCacheClusters`, which
    // reports the same stored clusters.
    let emulator = Emulator::new();
    // The VPC security group the clusters move onto is an Amazon EC2 resource, so
    // its id is minted by the owner rather than written down here.
    let security_group = emulator.security_groups("group-wide", 1).await.remove(0);
    create_group(&emulator, "members", "member-wide", &[("NumCacheClusters", "2")]).await;
    std::thread::sleep(PAST_TRANSITION);

    modify_accepted(
        &emulator,
        "members",
        "member-wide",
        &[
            ("CacheParameterGroupName", "default.redis6.x"),
            ("PreferredMaintenanceWindow", "tue:05:00-tue:06:30"),
            ("SecurityGroupIds.SecurityGroupId.1", security_group.as_str()),
            ("NotificationTopicArn", "arn:aws:sns:us-east-1:000000000000:cache"),
            ("SnapshotRetentionLimit", "4"),
            ("SnapshotWindow", "03:00-04:00"),
            ("AutoMinorVersionUpgrade", "true"),
            ("ApplyImmediately", "true"),
        ],
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);

    for member in ["member-wide-001", "member-wide-002"] {
        let described = describe_cluster(&emulator, "members", member).await;
        assert_reports(
            member,
            "the member cluster",
            &described,
            &[
                (
                    "CacheParameterGroup/CacheParameterGroupName",
                    "default.redis6.x",
                ),
                (
                    "CacheCluster/PreferredMaintenanceWindow",
                    "tue:05:00-tue:06:30",
                ),
                (
                    "SecurityGroups/member/SecurityGroupId",
                    security_group.as_str(),
                ),
                (
                    "NotificationConfiguration/TopicArn",
                    "arn:aws:sns:us-east-1:000000000000:cache",
                ),
                ("CacheCluster/SnapshotRetentionLimit", "4"),
                ("CacheCluster/SnapshotWindow", "03:00-04:00"),
                ("CacheCluster/AutoMinorVersionUpgrade", "true"),
            ],
        );
    }

    // "The status of the Amazon SNS notification topic … Notifications are sent
    // only if the status is active": `inactive` PAUSES the notifications on each
    // member cluster and leaves the topic bound, so a later `active` resumes the
    // same configuration.
    for (status, expected) in [("inactive", "inactive"), ("active", "active")] {
        modify_accepted(
            &emulator,
            "members",
            "member-wide",
            &[("NotificationTopicStatus", status)],
        )
        .await;
        std::thread::sleep(PAST_TRANSITION);
        for member in ["member-wide-001", "member-wide-002"] {
            let described = describe_cluster(&emulator, "members", member).await;
            assert_reports(
                member,
                "the member cluster",
                &described,
                &[
                    (
                        "NotificationConfiguration/TopicArn",
                        "arn:aws:sns:us-east-1:000000000000:cache",
                    ),
                    ("NotificationConfiguration/TopicStatus", expected),
                ],
            );
        }
    }
}

/// One row of the transient-status table: the case name, the modify pairs, and
/// whether the change is work ElastiCache performs asynchronously.
type StatusCase = (&'static str, &'static [(&'static str, &'static str)], bool);

#[tokio::test]
async fn an_asynchronous_change_leaves_the_group_transient_and_a_settings_change_does_not() {
    // The axis: whether the request asks for work ElastiCache performs
    // asynchronously (a scaling, an upgrade, an engine change, a promotion) or
    // only changes settings — the second kind applies synchronously, so a caller
    // that confirms the change by reading it back is not then refused by the
    // next state-gated call.
    let cases: [StatusCase; 6] = [
        ("the description", &[("ReplicationGroupDescription", "x")], false),
        ("the snapshot retention", &[("SnapshotRetentionLimit", "3")], false),
        ("the durability", &[("Durability", "async")], false),
        (
            "a node-type scaling",
            &[("CacheNodeType", "cache.m5.large"), ("ApplyImmediately", "true")],
            true,
        ),
        ("an engine change", &[("Engine", "valkey")], true),
        (
            "a promotion",
            &[("PrimaryClusterId", "transient-002"), ("ApplyImmediately", "true")],
            true,
        ),
    ];

    for (case, pairs, asynchronous) in cases {
        let emulator = Emulator::new();
        create_group(&emulator, case, "transient", &[("NumCacheClusters", "2")]).await;
        // Read the group back: this is answered once the create has settled it,
        // which is what lets the modify below be accepted.
        describe_group(&emulator, case, "transient").await;

        let answered = modify_accepted(&emulator, case, "transient", pairs).await;
        let expected = if asynchronous { "modifying" } else { "available" };
        // The call that starts the work answers with the transient status; a
        // report of the group is answered once that work is done.
        assert_eq!(
            answered.text("ReplicationGroup/Status"),
            Some(expected),
            "case {case}: the answer must report {expected}, got {:?}",
            answered.text("ReplicationGroup/Status")
        );

        // A second modify is accepted while the first is still being worked
        // through — "this parameter causes the modifications in this request and
        // any pending modifications to be applied" — and the last value wins.
        let second = modify_accepted(
            &emulator,
            case,
            "transient",
            &[("ReplicationGroupDescription", "the second change")],
        )
        .await;
        assert_eq!(
            second.text("ReplicationGroup/Description"),
            Some("the second change"),
            "case {case}: the second modify must be applied, got {:?}",
            second.text("ReplicationGroup/Description")
        );

        let settled = describe_group(&emulator, case, "transient").await;
        assert_eq!(
            settled.text("ReplicationGroup/Status"),
            Some("available"),
            "case {case}: the group must settle at available, got {:?}",
            settled.text("ReplicationGroup/Status")
        );
        assert_eq!(
            settled.text("ReplicationGroup/Description"),
            Some("the second change"),
            "case {case}: the stored description is the one the last modify set, got {:?}",
            settled.text("ReplicationGroup/Description")
        );
    }
}

/// One refusal row: the case name, the group named, the modify pairs, and the
/// HTTP status plus `awsQuery` error code it must be answered with.
type ModifyRefusal = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

#[tokio::test]
async fn a_request_this_service_cannot_satisfy_is_refused_with_its_modelled_error() {
    let emulator = Emulator::new();
    // The groups the rows are taken against: a plain available one, a sharded
    // one, one with in-transit encryption on, one with a single cluster, and a
    // standalone cluster that belongs to no group.
    create_group(&emulator, "setup", "plain-rg", &[("NumCacheClusters", "2")]).await;
    create_group(
        &emulator,
        "setup",
        "sharded-rg",
        &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
    )
    .await;
    create_group(
        &emulator,
        "setup",
        "encrypted-rg",
        &[("TransitEncryptionEnabled", "true")],
    )
    .await;
    create_group(&emulator, "setup", "solo-rg", &[]).await;
    create_user_group(&emulator, "setup", "spare-rbac").await;
    let standalone = emulator
        .call(
            "CreateCacheCluster",
            &params(&[("CacheClusterId", "standalone"), ("Engine", "redis")]),
        )
        .await;
    assert_eq!(
        standalone.status, 200,
        "CreateCacheCluster must answer 200, got {} {}",
        standalone.status, standalone.body
    );
    std::thread::sleep(PAST_TRANSITION);

    // The axis: which part of the request is wrong — the group named, a value,
    // two values that cannot hold together, a resource that does not exist, or a
    // documented order that has not been followed.
    let cases: [ModifyRefusal; 25] = [
        (
            "no such replication group",
            "absent-rg",
            &[("ReplicationGroupDescription", "x")],
            404,
            "ReplicationGroupNotFoundFault",
        ),
        (
            "no such cache parameter group",
            "plain-rg",
            &[("CacheParameterGroupName", "default.nonsense")],
            404,
            "CacheParameterGroupNotFound",
        ),
        (
            "a cache security group, which this account has none of",
            "plain-rg",
            &[("CacheSecurityGroupNames.CacheSecurityGroupName.1", "classic")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a maintenance window that is not a weekly range",
            "plain-rg",
            &[("PreferredMaintenanceWindow", "notaday:05:00-notaday:06:00")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative number of retention days",
            "plain-rg",
            &[("SnapshotRetentionLimit", "-1")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a notification topic status that is neither active nor inactive",
            "plain-rg",
            &[("NotificationTopicStatus", "paused")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine no replication group runs",
            "plain-rg",
            &[("Engine", "memcached")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine version downgrade",
            "plain-rg",
            &[("EngineVersion", "6.2")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an engine version no published family covers",
            "plain-rg",
            &[("EngineVersion", "99.9")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a node type the service does not publish",
            "plain-rg",
            &[("CacheNodeType", "cache.nonsense.xlarge")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a data-tiering node type on a group without data tiering",
            "plain-rg",
            &[("CacheNodeType", "cache.r6gd.xlarge")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "cluster mode straight from disabled to enabled",
            "plain-rg",
            &[("ClusterMode", "enabled")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "in-transit encryption required before preferred",
            "encrypted-rg",
            &[("TransitEncryptionMode", "required")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "in-transit encryption enabled without the preferred mode",
            "plain-rg",
            &[("TransitEncryptionEnabled", "true")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an auth token without in-transit encryption",
            "plain-rg",
            &[("AuthToken", GOOD_AUTH_TOKEN)],
            400,
            "InvalidParameterValue",
        ),
        (
            "an auth token shorter than the documented minimum",
            "encrypted-rg",
            &[("AuthToken", "short")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an update strategy with no auth token to apply",
            "encrypted-rg",
            &[("AuthTokenUpdateStrategy", "ROTATE")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "SET before any ROTATE",
            "encrypted-rg",
            &[
                ("AuthToken", GOOD_AUTH_TOKEN),
                ("AuthTokenUpdateStrategy", "SET"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "DELETE without transitioning to RBAC",
            "encrypted-rg",
            &[("AuthTokenUpdateStrategy", "DELETE")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "no such user group",
            "plain-rg",
            &[("UserGroupIdsToAdd.member.1", "absent-rbac")],
            404,
            "UserGroupNotFound",
        ),
        (
            "a user group that is not associated",
            "plain-rg",
            &[("UserGroupIdsToRemove.member.1", "spare-rbac")],
            400,
            "InvalidParameterValue",
        ),
        (
            "one user group added and removed at once",
            "plain-rg",
            &[
                ("UserGroupIdsToAdd.member.1", "spare-rbac"),
                ("UserGroupIdsToRemove.member.1", "spare-rbac"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "every user group removed and one added",
            "plain-rg",
            &[
                ("RemoveUserGroups", "true"),
                ("UserGroupIdsToAdd.member.1", "spare-rbac"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "automatic failover with no replica to promote",
            "solo-rg",
            &[("AutomaticFailoverEnabled", "true")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "multi-az without automatic failover",
            "plain-rg",
            &[("MultiAZEnabled", "true")],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, id, pairs, status, code) in cases {
        let refused = modify(&emulator, id, pairs).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (status, code),
            "case {case}: expected {status} {code}, got {} {} {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }

    // The rows that name a cluster: one that does not exist is not found, one
    // that exists but belongs to no group is a bad value, and a cluster mode
    // enabled group has no single primary and no one snapshot source at all.
    let cluster_cases: [ModifyRefusal; 5] = [
        (
            "a primary cluster that does not exist",
            "plain-rg",
            &[("PrimaryClusterId", "no-such-cluster")],
            404,
            "CacheClusterNotFound",
        ),
        (
            "a primary cluster that is not a member",
            "plain-rg",
            &[("PrimaryClusterId", "standalone")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a primary cluster on a cluster mode enabled group",
            "sharded-rg",
            &[("PrimaryClusterId", "sharded-rg-0001-001")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a snapshot source on a cluster mode enabled group",
            "sharded-rg",
            &[("SnapshottingClusterId", "sharded-rg-0001-001")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a snapshot source that is not a member",
            "plain-rg",
            &[("SnapshottingClusterId", "standalone")],
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, id, pairs, status, code) in cluster_cases {
        let refused = modify(&emulator, id, pairs).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (status, code),
            "case {case}: expected {status} {code}, got {} {} {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }

    // A description one character past the documented bound, which is the only
    // row whose value is longer than a table literal carries.
    let bound =
        service_com_amazonaws_elasticache_amazonelasticachev9::state::MAX_REPLICATION_GROUP_DESCRIPTION_LENGTH;
    assert_eq!(
        bound, 255,
        "ReplicationGroupDescription documents a maximum length of 255 characters, got {bound}"
    );
    for (case, length, status) in [
        ("at the documented bound", bound, 200),
        ("one past the documented bound", bound + 1, 400),
    ] {
        let description = "d".repeat(length);
        let answered = modify(
            &emulator,
            "plain-rg",
            &[("ReplicationGroupDescription", &description)],
        )
        .await;
        assert_eq!(
            answered.status, status,
            "case {case}: a {length}-character description must answer {status}, got {} {}",
            answered.status, answered.body
        );
        if status == 400 {
            assert_eq!(
                answered.error_code(),
                "InvalidParameterValue",
                "case {case}: expected InvalidParameterValue, got {}",
                answered.body
            );
        }
    }

    // Every refusal above persisted nothing: the groups still run what they were
    // created with, apart from the description the accepted row above set.
    for (id, description) in [
        ("plain-rg", "d".repeat(bound)),
        ("sharded-rg", "the ModifyReplicationGroup suite".to_string()),
        ("encrypted-rg", "the ModifyReplicationGroup suite".to_string()),
        ("solo-rg", "the ModifyReplicationGroup suite".to_string()),
    ] {
        let described = describe_group(&emulator, "after the refusals", id).await;
        assert_eq!(
            described.text("ReplicationGroup/Description"),
            Some(description.as_str()),
            "case {id}: a refused modify must leave the group as it was, got {:?}",
            described.text("ReplicationGroup/Description")
        );
        assert_eq!(
            described.text("ReplicationGroup/AuthTokenEnabled"),
            Some("false"),
            "case {id}: a refused auth-token update must set no token, got {:?}",
            described.text("ReplicationGroup/AuthTokenEnabled")
        );
        assert!(
            described.all("UserGroupIds/member").is_empty(),
            "case {id}: a refused user-group change must associate none, got {:?}",
            described.all("UserGroupIds/member")
        );
    }
}

#[tokio::test]
async fn a_resource_that_is_not_ready_takes_no_change() {
    // "The requested replication group is not in the available state": the work
    // the create started has to finish first, whatever the request asks for.
    let emulator = Emulator::new();
    create_group(&emulator, "still creating", "creating-rg", &[]).await;
    for (case, pairs) in [
        ("a description change", &[("ReplicationGroupDescription", "x")][..]),
        ("a retention change", &[("SnapshotRetentionLimit", "1")][..]),
        ("a scaling", &[("CacheNodeType", "cache.m5.large")][..]),
    ] {
        let refused = modify(&emulator, "creating-rg", pairs).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "InvalidReplicationGroupState"),
            "case {case}: expected 400 InvalidReplicationGroupState, got {} {} {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }

    // "The user group is not in an active state": a group still being created
    // cannot be associated either.
    create_group(&emulator, "user group creating", "waiting-rg", &[]).await;
    std::thread::sleep(PAST_TRANSITION);
    create_user_group(&emulator, "user group creating", "fresh-rbac").await;
    let refused = modify(
        &emulator,
        "waiting-rg",
        &[("UserGroupIdsToAdd.member.1", "fresh-rbac")],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidUserGroupState"),
        "expected 400 InvalidUserGroupState, got {} {} {}",
        refused.status,
        refused.error_code(),
        refused.body
    );

    // Once the clock has settled the user group, the same call is accepted — the
    // refusal was about the state, not about the request.
    std::thread::sleep(PAST_TRANSITION);
    let accepted = modify_accepted(
        &emulator,
        "user group settled",
        "waiting-rg",
        &[
            ("UserGroupIdsToAdd.member.1", "fresh-rbac"),
            ("ApplyImmediately", "true"),
        ],
    )
    .await;
    assert_eq!(
        accepted.all("UserGroupIds/member"),
        vec!["fresh-rbac"],
        "the settled user group must be associated, got {:?}",
        accepted.all("UserGroupIds/member")
    );
}

#[tokio::test]
async fn modifying_a_replication_group_records_an_event() {
    // Modifying a replication group is one of the changes this service records,
    // under the group it is about.
    let emulator = Emulator::new();
    create_group(&emulator, "events", "event-rg", &[]).await;
    std::thread::sleep(PAST_TRANSITION);
    modify_accepted(
        &emulator,
        "events",
        "event-rg",
        &[("ReplicationGroupDescription", "described again")],
    )
    .await;

    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[("SourceType", "replication-group")]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "DescribeEvents must answer 200, got {} {}",
        events.status, events.body
    );
    assert!(
        events
            .all("Event/Message")
            .contains(&"Replication group modified"),
        "the modify must record its own event, got {:?}",
        events.all("Event/Message")
    );
    assert!(
        events
            .all("Event/SourceIdentifier")
            .contains(&"event-rg"),
        "the event must be recorded under the group it is about, got {:?}",
        events.all("Event/SourceIdentifier")
    );
}
