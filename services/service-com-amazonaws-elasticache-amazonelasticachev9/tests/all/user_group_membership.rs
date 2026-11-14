// SPDX-License-Identifier: CC0-1.0

//! `ModifyUserGroup`: the rule a membership change follows, over the wire.
//!
//! The rule under test: a `ModifyUserGroup` that names an existing, active
//! group and a legal change accepts it, answers the group `modifying` with the
//! change reported under `PendingChanges` and its CURRENT members still under
//! `UserIds`, applies the engine it was given at once, and leaves the clock to
//! carry the membership into `UserIds` together with `active`; a change that
//! names no group, no user, a membership that is not there, one that is already
//! there, the same user twice, the `default` user's removal or nothing at all is
//! refused with the modelled error and persists nothing.
//!
//! Every row below is one input along that rule — how many ids are added, how
//! many removed, both at once, the case they are written in, the engine alone —
//! and the recorded `UserIdsToAdd=[one id]` case is a single row.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// The engine every group in this suite is created with, so a row that changes
/// the engine changes it to the other member of the published set
/// (`state::USER_ENGINES`).
const START_ENGINE: &str = "redis";

/// One membership-change row: what the group starts with, what the call asks
/// for, and what the service must answer and then settle at.
struct MembershipCase {
    case: &'static str,
    /// The members the group is created with; `default` is the built-in user
    /// every group must contain.
    start: &'static [&'static str],
    add: &'static [&'static str],
    remove: &'static [&'static str],
    /// `Engine`, when the row names one.
    engine: Option<&'static str>,
    /// The engine the group is listed for after the call.
    expected_engine: &'static str,
    /// The ids `PendingChanges` must report while the change is outstanding.
    pending_add: &'static [&'static str],
    pending_remove: &'static [&'static str],
    /// The membership the clock must leave behind.
    settled: &'static [&'static str],
}

/// The axis: the number of ids added, the number removed, both in one call, the
/// case an id is written in, and a call that changes only the engine.
const MEMBERSHIP_CASES: &[MembershipCase] = &[
    MembershipCase {
        case: "one user added",
        start: &["default", "member-one"],
        add: &["member-two"],
        remove: &[],
        engine: None,
        expected_engine: START_ENGINE,
        pending_add: &["member-two"],
        pending_remove: &[],
        settled: &["default", "member-one", "member-two"],
    },
    MembershipCase {
        case: "two users added at once",
        start: &["default"],
        add: &["member-one", "member-two"],
        remove: &[],
        engine: None,
        expected_engine: START_ENGINE,
        pending_add: &["member-one", "member-two"],
        pending_remove: &[],
        settled: &["default", "member-one", "member-two"],
    },
    MembershipCase {
        case: "one user removed",
        start: &["default", "member-one", "member-two"],
        add: &[],
        remove: &["member-one"],
        engine: None,
        expected_engine: START_ENGINE,
        pending_add: &[],
        pending_remove: &["member-one"],
        settled: &["default", "member-two"],
    },
    MembershipCase {
        case: "every member but the default removed",
        start: &["default", "member-one", "member-two"],
        add: &[],
        remove: &["member-one", "member-two"],
        engine: None,
        expected_engine: START_ENGINE,
        pending_add: &[],
        pending_remove: &["member-one", "member-two"],
        settled: &["default"],
    },
    MembershipCase {
        case: "added and removed in one call",
        start: &["default", "member-one"],
        add: &["member-two", "member-three"],
        remove: &["member-one"],
        engine: None,
        expected_engine: START_ENGINE,
        pending_add: &["member-two", "member-three"],
        pending_remove: &["member-one"],
        settled: &["default", "member-two", "member-three"],
    },
    MembershipCase {
        case: "an id written in another case",
        // "UserId is stored as a lowercase string", so the id resolves and is
        // stored lowercased however the request writes it.
        start: &["default"],
        add: &["Member-One"],
        remove: &[],
        engine: None,
        expected_engine: START_ENGINE,
        pending_add: &["member-one"],
        pending_remove: &[],
        settled: &["default", "member-one"],
    },
    MembershipCase {
        case: "only the engine changed",
        start: &["default", "member-one"],
        add: &[],
        remove: &[],
        // The other member of `state::USER_ENGINES`, which is the published set
        // of engines RBAC exists for.
        engine: Some("valkey"),
        expected_engine: "valkey",
        pending_add: &[],
        pending_remove: &[],
        settled: &["default", "member-one"],
    },
];

/// Create one RBAC user, asserting the create itself succeeded.
pub async fn create_user(emulator: &Emulator, case: &str, id: &str) {
    let created = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", START_ENGINE),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateUser {id} must answer 200, got {} {}",
        created.status, created.body
    );
}

/// Create one user group out of `members`, asserting the create succeeded.
async fn create_user_group(emulator: &Emulator, case: &str, id: &str, members: &[&str]) {
    let mut request = params(&[("UserGroupId", id), ("Engine", START_ENGINE)]);
    for (index, member) in members.iter().enumerate() {
        request.push((format!("UserIds.member.{}", index + 1), (*member).to_string()));
    }
    let created = emulator.call("CreateUserGroup", &request).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateUserGroup {id} must answer 200, got {} {}",
        created.status, created.body
    );
}

/// `ModifyUserGroup` for `group`, asking for `add`, `remove` and `engine`.
pub async fn modify_user_group(
    emulator: &Emulator,
    group: &str,
    add: &[&str],
    remove: &[&str],
    engine: Option<&str>,
) -> Reply {
    let mut request = params(&[("UserGroupId", group)]);
    for (index, id) in add.iter().enumerate() {
        request.push((
            format!("UserIdsToAdd.member.{}", index + 1),
            (*id).to_string(),
        ));
    }
    for (index, id) in remove.iter().enumerate() {
        request.push((
            format!("UserIdsToRemove.member.{}", index + 1),
            (*id).to_string(),
        ));
    }
    if let Some(engine) = engine {
        request.push(("Engine".to_string(), engine.to_string()));
    }
    emulator.call("ModifyUserGroup", &request).await
}

/// Read one group back through the implemented reader, asserting the read
/// succeeded.
pub async fn describe_user_group(emulator: &Emulator, case: &str, group: &str) -> Reply {
    let described = emulator
        .call("DescribeUserGroups", &params(&[("UserGroupId", group)]))
        .await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeUserGroups must answer 200, got {} {}",
        described.status, described.body
    );
    described
}

/// Wait out the span this service publishes for a user group's transition, so
/// the clock owes it its next status. Only the test waits; nothing sleeps in a
/// handler.
pub fn wait_out_the_transition() {
    assert!(
        PAST_TRANSITION > state::CLUSTER_TRANSITION,
        "this suite waits {PAST_TRANSITION:?}, which must out-last the transition span ({:?}) \
         this service publishes",
        state::CLUSTER_TRANSITION
    );
    std::thread::sleep(PAST_TRANSITION);
}

#[tokio::test]
async fn a_membership_change_is_reported_pending_and_then_settled() {
    for row in MEMBERSHIP_CASES {
        // One emulator per row, so each row's users and group are its own.
        let emulator = Emulator::new();
        for id in row.start.iter().chain(row.add) {
            let key = id.to_ascii_lowercase();
            if key == state::DEFAULT_USER_ID {
                // The built-in user every account already has; nothing creates it.
                continue;
            }
            create_user(&emulator, row.case, &key).await;
        }
        create_user_group(&emulator, row.case, "mod-group", row.start).await;
        // A created group is not ready yet, and an inactive one takes no
        // change: this read is answered once the create has settled it active.
        let created = describe_user_group(&emulator, row.case, "mod-group").await;
        assert_eq!(
            created.text("Status"),
            Some(state::USER_STATUS_ACTIVE),
            "case {}: the read after the create must report the group active, got {}",
            row.case,
            created.body
        );

        let modified =
            modify_user_group(&emulator, "mod-group", row.add, row.remove, row.engine).await;
        assert_eq!(
            modified.status, 200,
            "case {}: ModifyUserGroup must answer 200, got {} {}",
            row.case, modified.status, modified.body
        );
        assert_eq!(
            modified.text("Status"),
            Some(state::STATUS_MODIFYING),
            "case {}: the group must answer {}, got {}",
            row.case,
            state::STATUS_MODIFYING,
            modified.body
        );
        assert_eq!(
            modified.all("UserIds/member"),
            row.start.to_vec(),
            "case {}: the answer must still report the members the group has now, got {}",
            row.case,
            modified.body
        );
        assert_eq!(
            modified.all("UserIdsToAdd/member"),
            row.pending_add.to_vec(),
            "case {}: PendingChanges must report the additions, got {}",
            row.case,
            modified.body
        );
        assert_eq!(
            modified.all("UserIdsToRemove/member"),
            row.pending_remove.to_vec(),
            "case {}: PendingChanges must report the removals, got {}",
            row.case,
            modified.body
        );
        // The engine has no pending-value member to be reported through, so it
        // is the one thing the call applies at once.
        assert_eq!(
            modified.text("Engine"),
            Some(row.expected_engine),
            "case {}: the engine must be {}, got {}",
            row.case,
            row.expected_engine,
            modified.body
        );
        assert_eq!(
            modified.text("UserGroupId"),
            Some("mod-group"),
            "case {}: the answer must name the group, got {}",
            row.case,
            modified.body
        );

        // The change is persisted on the ONE copy of the group rather than only
        // reported: the reader is answered once the clock has applied it,
        // together with `active`.
        let settled = describe_user_group(&emulator, row.case, "mod-group").await;
        assert_eq!(
            settled.text("Status"),
            Some(state::USER_STATUS_ACTIVE),
            "case {}: the clock owes the group {}, got {}",
            row.case,
            state::USER_STATUS_ACTIVE,
            settled.body
        );
        assert_eq!(
            settled.all("UserIds/member"),
            row.settled.to_vec(),
            "case {}: the applied membership must be the stored one, got {}",
            row.case,
            settled.body
        );
        assert!(
            !settled.body.contains("PendingChanges"),
            "case {}: a group with nothing outstanding reports no PendingChanges, got {}",
            row.case,
            settled.body
        );
        assert_eq!(
            settled.text("Engine"),
            Some(row.expected_engine),
            "case {}: the stored engine must be {}, got {}",
            row.case,
            row.expected_engine,
            settled.body
        );

        // The membership has one copy: the users the group gained report it and
        // the ones it lost do not.
        for id in row.start.iter().chain(row.add) {
            let key = id.to_ascii_lowercase();
            if key == state::DEFAULT_USER_ID {
                continue;
            }
            let user = emulator
                .call("DescribeUsers", &params(&[("UserId", key.as_str())]))
                .await;
            let expected: Vec<&str> = if row.settled.contains(&key.as_str()) {
                vec!["mod-group"]
            } else {
                Vec::new()
            };
            assert_eq!(
                user.all("UserGroupIds/member"),
                expected,
                "case {}: user {key} must report {expected:?} as its groups, got {}",
                row.case,
                user.body
            );
        }
    }
}

/// One refusal row: the case, the pairs beside `UserGroupId`, and the status
/// plus `awsQuery` error code the request must be answered with.
type RefusalRow = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

/// The refusals, each one input along the rule: the group, the users, the
/// membership, the lists' contents and the engine.
const REFUSAL_ROWS: &[RefusalRow] = &[
    (
        "a group the account does not hold",
        &[
            ("UserGroupId", "never-created"),
            ("UserIdsToAdd.member.1", "outsider"),
        ],
        404,
        "UserGroupNotFound",
    ),
    (
        "adding a user that does not exist",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToAdd.member.1", "ghost-user"),
        ],
        404,
        "UserNotFound",
    ),
    (
        "adding a known user after an unknown one",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToAdd.member.1", "ghost-user"),
            ("UserIdsToAdd.member.2", "outsider"),
        ],
        404,
        "UserNotFound",
    ),
    (
        "removing a user that does not exist",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToRemove.member.1", "ghost-user"),
        ],
        404,
        "UserNotFound",
    ),
    (
        "removing a user that is not a member",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToRemove.member.1", "outsider"),
        ],
        400,
        "InvalidParameterValue",
    ),
    (
        "removing the default user",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToRemove.member.1", "default"),
        ],
        400,
        "DefaultUserRequired",
    ),
    (
        "removing the default user among others",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToRemove.member.1", "member-one"),
            ("UserIdsToRemove.member.2", "Default"),
        ],
        400,
        "DefaultUserRequired",
    ),
    (
        "adding a user that is already a member",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToAdd.member.1", "member-one"),
        ],
        400,
        "DuplicateUserName",
    ),
    (
        "one user named twice in UserIdsToAdd",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToAdd.member.1", "outsider"),
            ("UserIdsToAdd.member.2", "Outsider"),
        ],
        400,
        "DuplicateUserName",
    ),
    (
        "one user named twice in UserIdsToRemove",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToRemove.member.1", "member-one"),
            ("UserIdsToRemove.member.2", "MEMBER-ONE"),
        ],
        400,
        "DuplicateUserName",
    ),
    (
        "one user added and removed at once",
        &[
            ("UserGroupId", "mod-group"),
            ("UserIdsToAdd.member.1", "outsider"),
            ("UserIdsToRemove.member.1", "outsider"),
        ],
        400,
        "InvalidParameterCombination",
    ),
    (
        "no modification requested",
        &[("UserGroupId", "mod-group")],
        400,
        "InvalidParameterCombination",
    ),
    (
        // `memcached` is a legal CLUSTER engine and no RBAC engine, so it is not
        // in the published set `state::USER_ENGINES`.
        "an engine RBAC does not exist for",
        &[("UserGroupId", "mod-group"), ("Engine", "memcached")],
        400,
        "InvalidParameterValue",
    ),
    (
        "an engine that names nothing at all",
        &[("UserGroupId", "mod-group"), ("Engine", "notanengine")],
        400,
        "InvalidParameterValue",
    ),
];

/// The emulator the refusal rows run against: three users, and an active group
/// holding two of them.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    for id in ["member-one", "member-two", "outsider"] {
        create_user(&emulator, "seed", id).await;
    }
    create_user_group(
        &emulator,
        "seed",
        "mod-group",
        &["default", "member-one", "member-two"],
    )
    .await;
    wait_out_the_transition();
    emulator
}

#[tokio::test]
async fn a_change_that_cannot_hold_is_refused_and_persists_nothing() {
    let emulator = seeded().await;
    for (case, pairs, status, code) in REFUSAL_ROWS {
        let refused = emulator.call("ModifyUserGroup", &params(pairs)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (*status, *code),
            "case {case}: ModifyUserGroup must be refused {status} {code}, got {} {}",
            refused.status,
            refused.body
        );
        // Nothing was persisted: the group is untouched, still active, with no
        // change outstanding.
        let described = describe_user_group(&emulator, case, "mod-group").await;
        assert_eq!(
            described.text("Status"),
            Some(state::USER_STATUS_ACTIVE),
            "case {case}: a refused change leaves the group active, got {}",
            described.body
        );
        assert_eq!(
            described.all("UserIds/member"),
            vec!["default", "member-one", "member-two"],
            "case {case}: a refused change leaves the membership alone, got {}",
            described.body
        );
        assert_eq!(
            described.text("Engine"),
            Some(START_ENGINE),
            "case {case}: a refused change leaves the engine alone, got {}",
            described.body
        );
        assert!(
            !described.body.contains("PendingChanges"),
            "case {case}: a refused change leaves nothing outstanding, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_list_longer_than_the_account_can_hold_is_refused() {
    let emulator = seeded().await;
    // The ceiling is the published one: an account holds at most
    // `state::USER_QUOTA` users, so no legal change can name more distinct ids
    // than that. At and below it the length is NOT what refuses the request —
    // the ids simply name no user — which is how each bound is told apart.
    for (case, count, status, code) in [
        (
            "one id under the ceiling",
            state::USER_QUOTA - 1,
            404,
            "UserNotFound",
        ),
        (
            "exactly the ceiling",
            state::USER_QUOTA,
            404,
            "UserNotFound",
        ),
        (
            "one id past the ceiling",
            state::USER_QUOTA + 1,
            400,
            "InvalidParameterValue",
        ),
    ] {
        for member in ["UserIdsToAdd", "UserIdsToRemove"] {
            let mut request = params(&[("UserGroupId", "mod-group")]);
            for index in 0..count {
                request.push((
                    format!("{member}.member.{}", index + 1),
                    format!("bulk-user-{index}"),
                ));
            }
            let reply = emulator.call("ModifyUserGroup", &request).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (status, code),
                "case {case} ({member}, {count} ids): ModifyUserGroup must be refused {status} \
                 {code}, got {} {}",
                reply.status,
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_group_that_is_not_active_takes_no_change() {
    let emulator = Emulator::new();
    create_user(&emulator, "state gate", "member-one").await;
    create_user_group(&emulator, "state gate", "mod-group", &["default"]).await;

    // A group that is still being created is not active yet.
    let while_creating = modify_user_group(&emulator, "mod-group", &["member-one"], &[], None).await;
    assert_eq!(
        (while_creating.status, while_creating.error_code()),
        (400, "InvalidUserGroupState"),
        "case creating: a group that is not active takes no change, got {} {}",
        while_creating.status,
        while_creating.body
    );

    wait_out_the_transition();
    let accepted = modify_user_group(&emulator, "mod-group", &["member-one"], &[], None).await;
    assert_eq!(
        accepted.status, 200,
        "case active: the same change is accepted once the group is active, got {} {}",
        accepted.status, accepted.body
    );

    // And a group that is already carrying a change the clock has not applied is
    // not active either.
    let while_modifying = modify_user_group(&emulator, "mod-group", &[], &["member-one"], None).await;
    assert_eq!(
        (while_modifying.status, while_modifying.error_code()),
        (400, "InvalidUserGroupState"),
        "case modifying: a group with a change outstanding takes no second one, got {} {}",
        while_modifying.status,
        while_modifying.body
    );

    // The first change is still the one the clock applies.
    wait_out_the_transition();
    let settled = describe_user_group(&emulator, "state gate", "mod-group").await;
    assert_eq!(
        settled.all("UserIds/member"),
        vec!["default", "member-one"],
        "case modifying: the refused second change must not have been applied, got {}",
        settled.body
    );
}

#[tokio::test]
async fn a_user_deleted_before_the_change_settles_is_not_carried_into_the_group() {
    let emulator = Emulator::new();
    create_user(&emulator, "deleted member", "member-one").await;
    create_user_group(&emulator, "deleted member", "mod-group", &["default"]).await;
    wait_out_the_transition();

    let modified = modify_user_group(&emulator, "mod-group", &["member-one"], &[], None).await;
    assert_eq!(
        modified.status, 200,
        "the addition must be accepted, got {} {}",
        modified.status, modified.body
    );
    // "The user will be removed from all user groups": a user that stops
    // existing before the clock applies the change cannot be carried into the
    // group by it.
    let deleted = emulator
        .call("DeleteUser", &params(&[("UserId", "member-one")]))
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteUser must answer 200, got {} {}",
        deleted.status, deleted.body
    );

    wait_out_the_transition();
    let settled = describe_user_group(&emulator, "deleted member", "mod-group").await;
    assert_eq!(
        settled.all("UserIds/member"),
        vec![state::DEFAULT_USER_ID],
        "a deleted user must not be carried into the group, got {}",
        settled.body
    );
    assert!(
        !settled.body.contains("PendingChanges"),
        "the change is no longer outstanding, got {}",
        settled.body
    );
}
