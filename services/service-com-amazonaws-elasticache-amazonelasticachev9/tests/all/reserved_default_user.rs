// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule that the built-in `default` RBAC user is the service's own.
//!
//! The rule under test: the user ElastiCache gives every account governs the access every
//! RBAC user group grants, so the service keeps it as it ships — a call that would change it
//! or take it away is refused `InvalidParameterValue` (HTTP 400) and leaves it exactly as it
//! was, while the identical call against a user the account created is served.
//!
//! The tables span what the rule generalises over: which part of the user a request would
//! change (the whole rule list, rules appended to it, the engine, each of the ways a user
//! authenticates, and several at once), how the reserved id is spelled, and which of the two
//! operations that could write to the user is called.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, and each refusal is read back through `DescribeUsers`, so a refusal that changed
//! the user anyway cannot pass.

use service_com_amazonaws_elasticache_amazonelasticachev9::service::refusal_messages;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// A password that satisfies the published length rule.
const PASSWORD: &str = "aVeryLongPassword1";

/// The change a row asks for, named by what part of the user it would write.
const CHANGES: &[(&str, &[(&str, &str)])] = &[
    ("the whole rule list", &[("AccessString", "on ~* &* +@all")]),
    ("rules appended to the list", &[("AppendAccessString", "+@write")]),
    ("the engine the user is listed for", &[("Engine", "valkey")]),
    ("a password", &[("Passwords.member.1", PASSWORD)]),
    ("no password at all", &[("NoPasswordRequired", "true")]),
    (
        "an authentication mode",
        &[
            ("AuthenticationMode.Type", "password"),
            ("AuthenticationMode.Passwords.member.1", PASSWORD),
        ],
    ),
    (
        "the rule list and the engine at once",
        &[("AccessString", "on ~app:* +@read"), ("Engine", "valkey")],
    ),
];

/// `DescribeUsers` for one user id.
async fn describe_user(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call("DescribeUsers", &params(&[("UserId", id)]))
        .await
}

/// The built-in user as the store holds it: its status and its rule list.
async fn built_in_user(emulator: &Emulator) -> (Option<String>, Option<String>) {
    let described = describe_user(emulator, state::DEFAULT_USER_ID).await;
    assert_eq!(
        described.status, 200,
        "the built-in user must be readable, got {} {}",
        described.status, described.body
    );
    (
        described.text("Status").map(str::to_string),
        described.text("AccessString").map(str::to_string),
    )
}

#[tokio::test]
async fn the_built_in_user_takes_no_change_and_the_accounts_own_user_does() {
    let emulator = Emulator::new();
    // The positive control: a user of the account's own, created with the same rule list
    // the built-in user ships with, so each row's change is one the service CAN make.
    let created = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "own-user"),
                ("UserName", "own-user"),
                ("Engine", "redis"),
                ("AccessString", state::DEFAULT_USER_ACCESS_STRING),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: the account's own user must be created, got {} {}",
        created.status, created.body
    );

    let expected = refusal_messages::built_in_user_refusal("modified");
    let before = built_in_user(&emulator).await;
    for (case, change) in CHANGES {
        let mut request = params(&[("UserId", state::DEFAULT_USER_ID)]);
        request.extend(params(change));
        let refused = emulator.call("ModifyUser", &request).await;
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
        // The built-in user is untouched: same status, same rule list.
        let after = built_in_user(&emulator).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused call must leave the built-in user as it was, got \
             {after:?}"
        );

        // The identical change against the account's own user is served, so the row is
        // refused for WHICH user it names and not for what it asks.
        let mut allowed = params(&[("UserId", "own-user")]);
        allowed.extend(params(change));
        let modified = emulator.call("ModifyUser", &allowed).await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: the same change on the account's own user must answer 200, got {} {}",
            modified.status, modified.body
        );
        // The user settles before the next row asks for another change of it.
        crate::harness::settle_pending_changes(&emulator);
    }
}

#[tokio::test]
async fn every_spelling_of_the_reserved_id_is_refused_the_same_way() {
    // The refusal is about the user the id RESOLVES to: an id is stored lowercased, so
    // every casing of the reserved one names that same user.
    let emulator = Emulator::new();
    let expected = refusal_messages::built_in_user_refusal("modified");
    let before = built_in_user(&emulator).await;
    for spelling in ["default", "DEFAULT", "Default", "dEfAuLt"] {
        let refused = emulator
            .call(
                "ModifyUser",
                &params(&[("UserId", spelling), ("AccessString", "on ~* +@all")]),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code(), refused.error_message()),
            (400, "InvalidParameterValue", expected.as_str()),
            "case {spelling:?}: expected 400 InvalidParameterValue {expected:?}, got {} {}",
            refused.status,
            refused.body
        );
    }
    let after = built_in_user(&emulator).await;
    assert_eq!(
        after, before,
        "the built-in user must be unchanged by every refused spelling, got {after:?}"
    );
}

#[tokio::test]
async fn the_delete_states_the_same_rule_about_the_same_user() {
    // The other operation that could write to the built-in user words the rule the same
    // way, for what IT would have done: both sentences come from one template, so the two
    // cannot drift apart.
    let emulator = Emulator::new();
    let deleted = emulator
        .call(
            "DeleteUser",
            &params(&[("UserId", state::DEFAULT_USER_ID)]),
        )
        .await;
    assert_eq!(
        deleted.status, 400,
        "deleting the built-in user must be refused 400, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.error_message(),
        refusal_messages::built_in_user_refusal("deleted"),
        "the delete must state the rule for what it would have done, got {:?}",
        deleted.error_message()
    );
    let (status, access_string) = built_in_user(&emulator).await;
    assert_eq!(
        (status.as_deref(), access_string.as_deref()),
        (
            Some(state::USER_STATUS_ACTIVE),
            Some(state::DEFAULT_USER_ACCESS_STRING)
        ),
        "the built-in user must still be the one this service ships"
    );
}
