// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RBAC user groups a replication group is created attached to.
//!
//! The rule under test: `UserGroupIds` names user groups, so a create resolves each id
//! against the user groups this account HOLDS — the ones `CreateUserGroup` stored — and
//! refuses `UserGroupNotFoundFault` (HTTP 404) only for an id no such group answers for. An
//! id that does resolve clears that validation, after which the request is answered for
//! whatever else it gets wrong, and a create that is accepted reports the attachment from
//! both ends.
//!
//! The tables span what the rule generalises over: how many groups are named, which kind of
//! RBAC identity the id belongs to (a user group, a bare user, nothing at all), and where in
//! the validation order the request is answered.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, and each attachment is read back through `DescribeReplicationGroups` and
//! `DescribeUserGroups`.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// The node type every group here runs; this suite is not about node types.
const NODE_TYPE: &str = "cache.t3.micro";

/// A parameter group name no account holds, for the row that shows the user-group
/// validation has been cleared.
const ABSENT_PARAMETER_GROUP: &str = "default.redis7.never-created";

/// Create one RBAC user of the account's own.
async fn create_user(emulator: &Emulator, id: &str) {
    let created = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@read"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: the user {id} must be created, got {} {}",
        created.status, created.body
    );
}

/// Create one user group holding the built-in user and `member`.
async fn create_user_group(emulator: &Emulator, id: &str, member: &str) {
    let created = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", id),
                ("Engine", "redis"),
                ("UserIds.member.1", state::DEFAULT_USER_ID),
                ("UserIds.member.2", member),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: the user group {id} must be created, got {} {}",
        created.status, created.body
    );
}

/// `CreateReplicationGroup` for `id`, with `extra` beside the shared members.
async fn create_group(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "an RBAC-attached group"),
        ("Engine", "redis"),
        ("CacheNodeType", NODE_TYPE),
        // RBAC replaces the password a client would otherwise authenticate with, so these
        // groups carry encryption in transit, as such a group does.
        ("TransitEncryptionEnabled", "true"),
    ]);
    pairs.extend(params(extra));
    emulator.call("CreateReplicationGroup", &pairs).await
}

/// One attachment row: the case name, the group it creates, the members the create carries
/// beside the shared ones, and the user groups the created group must report.
type AttachmentRow = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

#[tokio::test]
async fn a_group_is_created_attached_to_the_user_groups_the_account_holds() {
    let emulator = Emulator::new();
    create_user(&emulator, "rbac-member").await;
    create_user_group(&emulator, "rbac-one", "rbac-member").await;
    create_user_group(&emulator, "rbac-two", "rbac-member").await;

    // The axis: how many of the account's user groups the create names.
    let rows: &[AttachmentRow] = &[
        (
            "one user group",
            "rbac-attached-one",
            &[("UserGroupIds.member.1", "rbac-one")],
            &["rbac-one"],
        ),
        (
            "two user groups",
            "rbac-attached-two",
            &[
                ("UserGroupIds.member.1", "rbac-one"),
                ("UserGroupIds.member.2", "rbac-two"),
            ],
            &["rbac-one", "rbac-two"],
        ),
        (
            "the same user group named twice, which is one attachment",
            "rbac-attached-twice",
            &[
                ("UserGroupIds.member.1", "rbac-one"),
                ("UserGroupIds.member.2", "rbac-one"),
            ],
            &["rbac-one"],
        ),
        ("no user group at all", "rbac-attached-none", &[], &[]),
    ];

    for (case, id, extra, attached) in rows {
        let created = create_group(&emulator, id, extra).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.all("UserGroupIds/member"),
            attached.to_vec(),
            "case {case:?}: the answer must report the groups the request attached, got {}",
            created.body
        );
        // Read back: the STORED group reports the same attachment.
        let described = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", id)]),
            )
            .await;
        assert_eq!(
            described.all("UserGroupIds/member"),
            attached.to_vec(),
            "case {case:?}: the stored group must report the attachment, got {}",
            described.body
        );
        // And so does the other end of it: a user group reports the replication groups it
        // is associated to.
        for group_id in *attached {
            let user_group = emulator
                .call("DescribeUserGroups", &params(&[("UserGroupId", group_id)]))
                .await;
            assert!(
                user_group.all("ReplicationGroups/member").contains(id),
                "case {case:?}: the user group {group_id} must report {id}, got {}",
                user_group.body
            );
        }
    }
}

#[tokio::test]
async fn an_id_that_names_no_user_group_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    create_user(&emulator, "bare-user").await;
    create_user_group(&emulator, "real-group", "bare-user").await;

    // The axis: what the id the request names actually is. A bare USER id and an id nothing
    // holds are both ids no user group answers for; the built-in user's id is one too, even
    // though that user exists and every group must contain it.
    for (case, named) in [
        ("a bare user id", "bare-user"),
        ("the built-in user's id", state::DEFAULT_USER_ID),
        ("an id nothing holds", "never-created-group"),
    ] {
        let refused = create_group(
            &emulator,
            &format!("rbac-miss-{}", named.to_ascii_lowercase()),
            &[("UserGroupIds.member.1", named)],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, "UserGroupNotFound"),
            "case {case:?}: expected 404 UserGroupNotFound, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            format!("User group {named} not found."),
            "case {case:?}: expected the message naming {named}, got {:?}",
            refused.error_message()
        );
    }

    // A real group id in the SAME request clears that validation, so the request is then
    // answered for the next thing it gets wrong — here the parameter group it names, which
    // is the modelled not-found of THAT resource.
    let later = create_group(
        &emulator,
        "rbac-cleared",
        &[
            ("UserGroupIds.member.1", "real-group"),
            ("CacheParameterGroupName", ABSENT_PARAMETER_GROUP),
        ],
    )
    .await;
    assert_eq!(
        (later.status, later.error_code()),
        (404, "CacheParameterGroupNotFound"),
        "a resolvable user group must clear the user-group validation, got {} {}",
        later.status,
        later.body
    );
    // And the order holds the other way round: a request wrong about both is answered for
    // its user groups first.
    let first = create_group(
        &emulator,
        "rbac-order",
        &[
            ("UserGroupIds.member.1", "never-created-group"),
            ("CacheParameterGroupName", ABSENT_PARAMETER_GROUP),
        ],
    )
    .await;
    assert_eq!(
        (first.status, first.error_code()),
        (404, "UserGroupNotFound"),
        "the user groups are resolved before the parameter group, got {} {}",
        first.status,
        first.body
    );
    // Neither refused create left a group behind.
    for id in ["rbac-cleared", "rbac-order"] {
        let described = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", id)]),
            )
            .await;
        assert_eq!(
            described.status, 404,
            "a refused create must store nothing, got {} {}",
            described.status, described.body
        );
    }
}

#[tokio::test]
async fn a_list_longer_than_the_account_can_hold_is_refused_before_it_is_walked() {
    // The member is caller-chosen and the model bounds its length only from below, so the
    // service bounds the count: no legal request can name more distinct user groups than an
    // account holds (`state::USER_GROUP_QUOTA`). The axis is the length — the quota itself,
    // satisfied by repeating a group the account HAS so the row is refused for its length
    // and not for an id, and one past it.
    let emulator = Emulator::new();
    create_user(&emulator, "bounded-user").await;
    create_user_group(&emulator, "bounded-group", "bounded-user").await;
    for (case, count, status) in [
        ("exactly the quota", state::USER_GROUP_QUOTA, 200),
        ("one past the quota", state::USER_GROUP_QUOTA + 1, 400),
    ] {
        let mut extra: Vec<(String, String)> = (1..=count)
            .map(|position| {
                (
                    format!("UserGroupIds.member.{position}"),
                    "bounded-group".to_string(),
                )
            })
            .collect();
        let id = format!("rbac-bounded-{count}");
        extra.insert(0, ("ReplicationGroupId".to_string(), id.clone()));
        extra.extend(params(&[
            ("ReplicationGroupDescription", "a bounded list"),
            ("Engine", "redis"),
            ("CacheNodeType", NODE_TYPE),
            ("TransitEncryptionEnabled", "true"),
        ]));
        let reply = emulator.call("CreateReplicationGroup", &extra).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        if status == 200 {
            // However many times the one group was named, the attachment is one.
            assert_eq!(
                reply.all("UserGroupIds/member"),
                vec!["bounded-group"],
                "case {case:?}: the group must report the one group it attached, got {}",
                reply.body
            );
            continue;
        }
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {}",
            reply.body
        );
        let described = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", &id)]),
            )
            .await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused create must store nothing, got {} {}",
            described.status, described.body
        );
    }
}

#[tokio::test]
async fn a_user_group_on_its_way_out_cannot_be_attached() {
    // `InvalidUserGroupStateFault` is "the user group is not in an active state": a group
    // whose removal has been asked for is about to stop holding anybody, so a create cannot
    // attach it. A group still coming up IS attachable — the create provisions its own
    // clusters over a span of its own — so that is the control.
    let emulator = Emulator::new();
    create_user(&emulator, "state-user").await;
    create_user_group(&emulator, "fresh-group", "state-user").await;
    create_user_group(&emulator, "leaving-group", "state-user").await;

    let attached_while_creating = create_group(
        &emulator,
        "rbac-while-creating",
        &[("UserGroupIds.member.1", "fresh-group")],
    )
    .await;
    assert_eq!(
        attached_while_creating.status, 200,
        "a group that is still coming up must be attachable, got {} {}",
        attached_while_creating.status, attached_while_creating.body
    );

    crate::harness::settle_pending_changes(&emulator);
    let deleted = emulator
        .call(
            "DeleteUserGroup",
            &params(&[("UserGroupId", "leaving-group")]),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "the user group must be deletable, got {} {}",
        deleted.status, deleted.body
    );
    // The removal is what the clock owes from the call that asked for it, so it is held out
    // of reach rather than raced for.
    crate::harness::hold_pending_changes(&emulator);
    let refused = create_group(
        &emulator,
        "rbac-while-deleting",
        &[("UserGroupIds.member.1", "leaving-group")],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidUserGroupState"),
        "a user group on its way out cannot be attached, got {} {}",
        refused.status,
        refused.body
    );
}
