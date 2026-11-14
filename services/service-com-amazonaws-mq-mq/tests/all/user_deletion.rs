// SPDX-License-Identifier: CC0-1.0

//! Deleting a broker user, driven over the wire.
//!
//! RULE: `DeleteUser` records the deletion of exactly the user the request names on the
//! broker the request names, as that user's pending change. A user change does not take
//! effect on the broker until the broker is rebooted — which is what
//! `UserPendingChanges` reports — so the user keeps being served, now carrying
//! `pendingChange` `DELETE` (`smithy_query(query="shape", shape="ChangeType")`), and
//! every other user of that broker is left exactly as it was. How many users the broker
//! is left with is not a precondition of the call: the "at least one user" rule is
//! `CreateBroker`'s (`state::MIN_USERS_PER_BROKER`), so deleting a broker's last user,
//! administrative or not, is accepted.
//!
//! RULE: a `BrokerId` that resolves to no broker, or a `Username` that resolves to no
//! user of that broker, is the modelled `NotFoundException` (404) naming the member that
//! resolved to nothing, with the same sentence every handler that resolves the same
//! thing states.
//!
//! The tables span what those rules generalise over: the number of users the broker
//! holds, which of them is deleted (first, middle, last, only), the username forms the
//! model's documented charset and length bounds allow
//! (`smithy_query(query="doc", shape="CreateUserRequest", member="Username")` — "only
//! alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~) … must be
//! 2-100 characters long"), and the engine type the broker runs (`ENGINE_TYPE_VALUES`,
//! the model's own enumeration). The recorded case — one broker, one administrative
//! user, deleted and then read back — is one row of the first table.
//!
//! The wire harness is shared with [`crate::broker_lifecycle`], so every call goes
//! through the same `MultiServiceRouter::dispatch` the front door uses.

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::ENGINE_TYPE_VALUES;
use service_com_amazonaws_mq_mq::state::{
    user_not_found_message, CHANGE_TYPE_DELETE, MIN_USERS_PER_BROKER,
};

use crate::broker_lifecycle::{create_body, string_at, Mq};

/// `POST /v1/brokers/{BrokerId}/users/{Username}` — `CreateUser`.
pub(crate) async fn create_user(mq: &Mq, broker_id: &str, username: &str, body: Value) -> (u16, Value) {
    mq.call(
        "POST",
        &format!("/v1/brokers/{broker_id}/users/{username}"),
        Some(body),
    )
    .await
}

/// `GET /v1/brokers/{BrokerId}/users/{Username}` — `DescribeUser`, the reader this
/// suite proves a delete through.
pub(crate) async fn describe_user(mq: &Mq, broker_id: &str, username: &str) -> (u16, Value) {
    mq.call(
        "GET",
        &format!("/v1/brokers/{broker_id}/users/{username}"),
        None,
    )
    .await
}

/// `DELETE /v1/brokers/{BrokerId}/users/{Username}` — the operation under test, also
/// reporting the error code a restJson1 client branches on.
async fn delete_user(mq: &Mq, broker_id: &str, username: &str) -> (u16, String, Value) {
    mq.call_with_code(
        "DELETE",
        &format!("/v1/brokers/{broker_id}/users/{username}"),
        None,
    )
    .await
}

/// What `DescribeBroker` reports for each of a broker's users: the username and the
/// change the broker still owes it, in the order the broker reports them. A user whose
/// pending change is not a deletion is reported with whatever change it does carry —
/// which is why the assertions below test for a pending DELETE rather than for the
/// absence of any pending change at all.
pub(crate) async fn broker_users(mq: &Mq, broker_id: &str) -> Vec<(String, Option<String>)> {
    let (status, described) = mq.describe_broker(broker_id).await;
    assert_eq!(
        status, 200,
        "describe-broker {broker_id:?}: expected 200, got {status} {described:?}"
    );
    described
        .pointer("/users")
        .and_then(Value::as_array)
        .map(|users| {
            users
                .iter()
                .map(|user| {
                    (
                        string_at(user, "/username"),
                        user.pointer("/pendingChange")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A broker holding `usernames`: the first is the administrative user `CreateBroker`
/// mints the broker with, the rest are added through `CreateUser`.
async fn broker_with_users(mq: &Mq, name: &str, usernames: &[String]) -> String {
    let mut body = create_body(name);
    body["users"] = json!([{
        "username": usernames[0],
        "password": "GoodPassword12",
    }]);
    let (status, created) = mq.create_broker(body).await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected 200 from create-broker, got {status} {created:?}"
    );
    let broker_id = string_at(&created, "/brokerId");
    for username in &usernames[1..] {
        let (status, body) = create_user(
            mq,
            &broker_id,
            username,
            json!({"password": "GoodPassword12", "consoleAccess": true, "groups": ["admins"]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup {name:?}: expected 200 from create-user {username:?}, got {status} {body:?}"
        );
    }
    broker_id
}

/// Usernames spanning the documented charset and both published length bounds.
fn user_names() -> Vec<String> {
    vec![
        "ab".to_string(),
        "a-b.c_d~e".to_string(),
        "User99".to_string(),
        "u".repeat(100),
    ]
}

/// RULE 1: whatever the set of users on the broker, the delete marks exactly the one
/// named, which keeps reading back with its change pending, and leaves every other one
/// as it was.
#[tokio::test]
async fn delete_user_records_the_change_on_exactly_the_named_user() {
    let mq = Mq::new("us-east-1");
    let names = user_names();

    // Rows: the position of the deleted user in a set of a given size — the only user,
    // the first of several, one in the middle, the last — crossed with the username
    // forms the documented charset allows.
    let sizes = [1usize, 2, 3, 4];
    let mut rows = 0usize;
    for size in sizes {
        for target in 0..size {
            let usernames: Vec<String> = names[..size].to_vec();
            let case = format!(
                "{size} users, deleting index {target} ({:?})",
                usernames[target]
            );
            let broker_id =
                broker_with_users(&mq, &format!("del-{size}-{target}"), &usernames).await;

            let (status, code, answer) = delete_user(&mq, &broker_id, &usernames[target]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {code:?} {answer:?}"
            );

            // The deleted user is still served, and now reports the change the
            // broker owes it — with no other pending member, because nothing about
            // the user is being changed.
            let (status, read_back) = describe_user(&mq, &broker_id, &usernames[target]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the user must keep being served until the broker applies \
                 the change, got {status} {read_back:?}"
            );
            assert_eq!(
                string_at(&read_back, "/username"),
                usernames[target],
                "case {case:?}: expected the stored username back, got {read_back:?}"
            );
            assert_eq!(
                read_back
                    .pointer("/pending/pendingChange")
                    .and_then(Value::as_str),
                Some(CHANGE_TYPE_DELETE),
                "case {case:?}: expected a pending {CHANGE_TYPE_DELETE}, got {read_back:?}"
            );

            // No other user of the broker was touched: each is still served and none
            // of them is the one the broker is to drop.
            for (index, username) in usernames.iter().enumerate() {
                if index == target {
                    continue;
                }
                let (status, user) = describe_user(&mq, &broker_id, username).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {username:?} must still read back, got {status} {user:?}"
                );
                assert_ne!(
                    user.pointer("/pending/pendingChange")
                        .and_then(Value::as_str),
                    Some(CHANGE_TYPE_DELETE),
                    "case {case:?}: {username:?} must not be pending deletion, got {user:?}"
                );
            }

            // The broker's own listing reports the same: it owes a deletion for that
            // user and for no other, and still reports every user it holds.
            let reported = broker_users(&mq, &broker_id).await;
            let mut reported_names: Vec<String> =
                reported.iter().map(|(name, _)| name.clone()).collect();
            reported_names.sort();
            let mut all_names = usernames.clone();
            all_names.sort();
            assert_eq!(
                reported_names, all_names,
                "case {case:?}: expected every user still reported, got {reported:?}"
            );
            let pending_deletion: Vec<&String> = reported
                .iter()
                .filter(|(_, pending)| pending.as_deref() == Some(CHANGE_TYPE_DELETE))
                .map(|(name, _)| name)
                .collect();
            assert_eq!(
                pending_deletion,
                vec![&usernames[target]],
                "case {case:?}: expected only the deleted user pending \
                 {CHANGE_TYPE_DELETE}, got {reported:?}"
            );
            rows += 1;
        }
    }
    assert_eq!(
        rows,
        sizes.iter().sum::<usize>(),
        "every position in every set size must be covered, covered {rows}"
    );
}

/// RULE 1's other half: the user count is not a precondition, so every user of a broker
/// can be deleted — the last one, the administrative one the broker was created with,
/// included — and the broker itself is untouched. This is the recorded case, generalised
/// over the engine type and over the set the broker starts with.
#[tokio::test]
async fn deleting_a_brokers_last_user_is_accepted() {
    let mq = Mq::new("eu-west-1");
    let names = user_names();

    for (index, engine_type) in ENGINE_TYPE_VALUES.iter().enumerate() {
        for size in [MIN_USERS_PER_BROKER, MIN_USERS_PER_BROKER + 2] {
            let case = format!("{engine_type} broker starting with {size} users");
            let usernames: Vec<String> = names[..size].to_vec();
            let mut body = create_body(&format!("last-user-{index}-{size}"));
            body["engineType"] = json!(engine_type);
            if *engine_type == "RABBITMQ" {
                // The deployment mode a RabbitMQ broker runs as a cluster with.
                body["deploymentMode"] = json!("CLUSTER_MULTI_AZ");
            }
            body["users"] = json!([{"username": usernames[0], "password": "GoodPassword12"}]);
            let (status, created) = mq.create_broker(body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from create-broker, got {status} {created:?}"
            );
            let broker_id = string_at(&created, "/brokerId");
            for username in &usernames[1..] {
                let (status, body) = create_user(
                    &mq,
                    &broker_id,
                    username,
                    json!({"password": "GoodPassword12"}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from create-user, got {status} {body:?}"
                );
            }

            // Delete every user, so the final call deletes the only one whose change
            // is not already pending — the administrative user the broker was
            // created with.
            let mut left = usernames.len();
            for username in usernames.iter().rev() {
                let (status, code, answer) = delete_user(&mq, &broker_id, username).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: deleting {username:?} with {left} user(s) not yet \
                     deleted must be accepted, got {status} {code:?} {answer:?}"
                );
                left -= 1;
            }

            // Every one of them now carries the pending deletion, and the broker is
            // still there.
            let reported = broker_users(&mq, &broker_id).await;
            assert_eq!(
                reported.len(),
                usernames.len(),
                "case {case:?}: expected every user still reported, got {reported:?}"
            );
            for (username, pending) in &reported {
                assert_eq!(
                    pending.as_deref(),
                    Some(CHANGE_TYPE_DELETE),
                    "case {case:?}: expected {username:?} pending {CHANGE_TYPE_DELETE}, \
                     got {reported:?}"
                );
            }
            let (status, described) = mq.describe_broker(&broker_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the broker itself must be untouched, got {status} {described:?}"
            );
        }
    }
}

/// Asking again for a deletion that is already pending is accepted and leaves the one
/// pending change as it is: the user is still there, so the request resolves.
#[tokio::test]
async fn repeating_a_pending_deletion_changes_nothing() {
    let mq = Mq::new("us-east-1");
    let usernames = vec!["admin".to_string(), "alice".to_string()];
    let broker_id = broker_with_users(&mq, "repeated-delete", &usernames).await;

    for attempt in 1..=3 {
        let (status, code, answer) = delete_user(&mq, &broker_id, "alice").await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: expected 200, got {status} {code:?} {answer:?}"
        );
        let (status, user) = describe_user(&mq, &broker_id, "alice").await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: expected the user still served, got {status} {user:?}"
        );
        assert_eq!(
            user.pointer("/pending/pendingChange").and_then(Value::as_str),
            Some(CHANGE_TYPE_DELETE),
            "attempt {attempt}: expected a pending {CHANGE_TYPE_DELETE}, got {user:?}"
        );
    }

    let reported = broker_users(&mq, &broker_id).await;
    let pending_deletion: Vec<&String> = reported
        .iter()
        .filter(|(_, pending)| pending.as_deref() == Some(CHANGE_TYPE_DELETE))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        pending_deletion,
        vec![&"alice".to_string()],
        "repeated deletes must leave the one deletion pending on the one user, got \
         {reported:?}"
    );
}

/// RULE 2: an id that resolves to nothing is `NotFoundException` (404) naming the
/// member that resolved to nothing, with the sentence this service states that failure
/// with everywhere.
#[tokio::test]
async fn delete_user_reports_what_resolved_to_nothing() {
    let mq = Mq::new("us-east-1");
    let usernames = vec!["admin".to_string(), "alice".to_string()];
    let broker_id = broker_with_users(&mq, "missing-cases", &usernames).await;

    // Rows: the forms a `BrokerId` can take that resolve to no broker.
    for absent in [
        "b-does-not-exist",
        "b-00000000-0000-0000-0000-000000000000",
        "b-",
        "not-even-a-broker-id",
    ] {
        let case = format!("broker {absent:?}");
        let (status, code, answer) = delete_user(&mq, absent, "admin").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answer:?}"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
        );
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            Some("broker-id"),
            "case {case:?}: expected errorAttribute \"broker-id\", got {answer:?}"
        );
    }

    // Rows: usernames the broker does not hold — never created, outside the documented
    // charset, and outside each published length bound. Every one is the same refusal,
    // and none of them disturbs the users the broker does hold.
    let over_long = "u".repeat(101);
    for username in ["nobody", "also-nobody", "a", &over_long] {
        let case = format!("user {username:?}");
        let (status, code, answer) = delete_user(&mq, &broker_id, username).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answer:?}"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
        );
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            Some("username"),
            "case {case:?}: expected errorAttribute \"username\", got {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/message"),
            user_not_found_message(&broker_id, username),
            "case {case:?}: the sentence must name the user and the broker asked for, \
             got {answer:?}"
        );
        // The handler that reads a user up states the identical sentence for the
        // identical failure.
        let (status, read) = describe_user(&mq, &broker_id, username).await;
        assert_eq!(
            status, 404,
            "case {case:?}: describe-user must agree, got {status} {read:?}"
        );
        assert_eq!(
            string_at(&read, "/message"),
            string_at(&answer, "/message"),
            "case {case:?}: both handlers must state the one failure identically, got \
             {read:?} and {answer:?}"
        );
    }

    let reported = broker_users(&mq, &broker_id).await;
    let names: Vec<&String> = reported.iter().map(|(name, _)| name).collect();
    assert_eq!(
        names,
        vec![&"admin".to_string(), &"alice".to_string()],
        "a refused delete must change nothing, got {reported:?}"
    );
    assert!(
        reported
            .iter()
            .all(|(_, pending)| pending.as_deref() != Some(CHANGE_TYPE_DELETE)),
        "a refused delete must mark no user for deletion, got {reported:?}"
    );
}
