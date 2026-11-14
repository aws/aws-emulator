// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the sentence a change that needs an ACTIVE user group is refused
//! with — the text a client racing two changes, or waiting to retry, reads.
//!
//! The rule under test, over a SET of inputs rather than the one recorded race:
//! `InvalidUserGroupStateFault` (400, `InvalidUserGroupState`) is worded as two
//! sentences — the `UserGroupId` as the request named it, the status the group carries NOW
//! as `DescribeUserGroups` reports it, and then the status such a call admits — and it is
//! worded the one way at every entry point that needs the group to be active, because the
//! decision is the same one wherever it is taken.
//!
//! The rows differ along WHICH non-active status the group carries (still being created,
//! already carrying a change the clock has not settled, on its way out) and along the
//! entry point that is refused (`ModifyUserGroup`, `CreateServerlessCache`,
//! `ModifyReplicationGroup`). The status every expectation is built from is READ back out
//! of the describe, so a message carrying a literal could not pass, and the status a
//! refused call leaves behind is read again afterwards.
//!
//! Every assertion goes over the wire through [`crate::harness`]. The sentence itself is
//! written out here on purpose: a test that built it from the helper the handlers call
//! would pass whatever that helper said.

use crate::harness::{params, Emulator, Reply};
use crate::user_group_membership::{create_user, describe_user_group, modify_user_group};

/// Assert `reply` is the refusal `status` / `code` carrying exactly `message`.
fn assert_refusal(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (status, code),
        "case {case}: expected {status} {code}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected message {message:?}, got {:?}",
        reply.error_message()
    );
}

/// The sentence a group carrying `status` must refuse a change with.
fn expected_sentence(group: &str, status: &str) -> String {
    format!("User group {group} has status {status}. Only active user groups can be modified.")
}

/// `CreateUserGroup` for `id` holding the service's own `default` user, asserting the
/// create itself was accepted, and answering with that call's own reply.
async fn create_user_group(emulator: &Emulator, case: &str, id: &str) -> Reply {
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
    created
}

/// `DeleteUserGroup` for `id`, asserting the delete itself was accepted — the call that
/// puts a group on its way out — and answering with its reply.
async fn delete_user_group(emulator: &Emulator, case: &str, id: &str) -> Reply {
    let removed = emulator
        .call("DeleteUserGroup", &params(&[("UserGroupId", id)]))
        .await;
    assert_eq!(
        removed.status, 200,
        "case {case}: DeleteUserGroup {id} must answer 200, got {} {}",
        removed.status, removed.body
    );
    removed
}

/// The `Status` `reply` reports — the variable part the refusal takes from the store
/// rather than from the request, read off the answer of the call that put the group in
/// that status rather than written out here.
fn reported_status(case: &str, reply: &Reply) -> String {
    reply
        .text("Status")
        .unwrap_or_else(|| panic!("case {case}: the answer must report a Status"))
        .to_string()
}

#[tokio::test]
async fn a_user_group_that_is_not_active_refuses_a_change_with_its_status() {
    let emulator = Emulator::new();
    create_user(&emulator, "the member every row adds", "joiner").await;

    // The axis: WHICH non-active status the group carries when the change arrives — still
    // being created, already carrying a change the clock has not settled, and already on
    // its way out.
    //
    // The status each row drives is read off the answer of the call that PUT the group in
    // it, and the gated call follows that answer straight away: a report of the group is
    // answered only once the span has passed, so a read between the two would wait the
    // group out of the status the row is about.
    for (case, group) in [
        ("still creating", "creating-group"),
        ("already modifying", "modifying-group"),
        ("already deleting", "deleting-group"),
    ] {
        let created = create_user_group(&emulator, case, group).await;
        let status = match case {
            "still creating" => reported_status(case, &created),
            "already modifying" => {
                // Reading the group back is answered once the create has settled it
                // `active`, which is what lets the first change be accepted; that
                // accepted call is what puts the group in the status this row is about.
                describe_user_group(&emulator, case, group).await;
                let accepted = modify_user_group(&emulator, group, &["joiner"], &[], None).await;
                assert_eq!(
                    accepted.status, 200,
                    "case {case}: the first ModifyUserGroup must answer 200, got {} {}",
                    accepted.status, accepted.body
                );
                reported_status(case, &accepted)
            }
            _ => {
                describe_user_group(&emulator, case, group).await;
                let removed = delete_user_group(&emulator, case, group).await;
                reported_status(case, &removed)
            }
        };
        assert_ne!(
            status, "active",
            "case {case}: the row must drive a group that is not active, got {status:?}"
        );
        let expected = expected_sentence(group, &status);

        let refused = modify_user_group(&emulator, group, &["joiner"], &[], None).await;
        assert_refusal(
            &format!("{case} through ModifyUserGroup"),
            &refused,
            400,
            "InvalidUserGroupState",
            &expected,
        );

        // The same gate, the same sentence, at a second entry point: a change that
        // ATTACHES the group to another resource needs it active too — and that it is
        // still refused over the SAME status is what shows the first refusal changed
        // nothing.
        let attached = emulator
            .call(
                "CreateServerlessCache",
                &params(&[
                    ("ServerlessCacheName", &format!("cache-for-{group}")),
                    ("Engine", "redis"),
                    ("UserGroupId", group),
                ]),
            )
            .await;
        assert_refusal(
            &format!("{case} through CreateServerlessCache"),
            &attached,
            400,
            "InvalidUserGroupState",
            &expected,
        );
    }

    // The status the gate DOES admit: once the clock has settled the group `active` the
    // same change is accepted and the membership converges, which is what makes the
    // refusals above the state rule speaking rather than a blanket refusal.
    create_user_group(&emulator, "an active group", "active-group").await;
    let active = describe_user_group(&emulator, "an active group", "active-group").await;
    assert_eq!(
        active.text("Status"),
        Some("active"),
        "the read after the create must report the group active, got {}",
        active.body
    );
    let accepted = modify_user_group(&emulator, "active-group", &["joiner"], &[], None).await;
    assert_eq!(
        accepted.status, 200,
        "an active group must take the change, got {} {}",
        accepted.status, accepted.body
    );
    let described = describe_user_group(&emulator, "an active group", "active-group").await;
    assert_eq!(
        described.all("UserIds/member"),
        vec!["default", "joiner"],
        "the accepted change must be carried into the membership, got {}",
        described.body
    );
    assert_eq!(
        described.text("Status"),
        Some("active"),
        "the settled group must be active again, got {}",
        described.body
    );
}

#[tokio::test]
async fn attaching_a_user_group_that_is_not_active_states_the_same_sentence() {
    let emulator = Emulator::new();
    // The replication group the user groups below are attached to, settled `available` so
    // the modify is refused over the USER group's status and nothing else.
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "rbac-rg"),
                ("ReplicationGroupDescription", "the user-group gate suite"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateReplicationGroup must answer 200, got {} {}",
        created.status, created.body
    );
    let available = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", "rbac-rg")]),
        )
        .await;
    assert_eq!(
        available.text("ReplicationGroup/Status"),
        Some("available"),
        "the read after the create must report the replication group available, got {}",
        available.body
    );

    // The axis: the entry point, and the non-active status the attached group carries.
    // `ModifyReplicationGroup` reaches the same gate through `UserGroupIdsToAdd`.
    for (case, group) in [
        ("still creating", "fresh-rbac"),
        ("already deleting", "doomed-rbac"),
    ] {
        let created = create_user_group(&emulator, case, group).await;
        let status = if case == "already deleting" {
            describe_user_group(&emulator, case, group).await;
            let removed = delete_user_group(&emulator, case, group).await;
            reported_status(case, &removed)
        } else {
            reported_status(case, &created)
        };
        let refused = emulator
            .call(
                "ModifyReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", "rbac-rg"),
                    ("UserGroupIdsToAdd.member.1", group),
                ]),
            )
            .await;
        assert_refusal(
            &format!("{case} through ModifyReplicationGroup"),
            &refused,
            400,
            "InvalidUserGroupState",
            &expected_sentence(group, &status),
        );
    }
}
