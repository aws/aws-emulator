// SPDX-License-Identifier: CC0-1.0

//! RESTARTING a broker, PROMOTING one, and the user changes a restart applies — driven
//! end to end through the router, so routing, restJson1 (de)serialization and the
//! handlers are exercised together.
//!
//! The rules these tables pin, each Amazon MQ's and not one broker's:
//!
//! 1. `RebootBroker` is accepted only while the broker is RUNNING: the broker then
//!    reports `REBOOT_IN_PROGRESS` until the clock makes its return to RUNNING due. A
//!    broker in any other state is the modelled `BadRequestException` (400) naming the
//!    broker and the state it is in, and a `BrokerId` that resolves to no broker is the
//!    modelled `NotFoundException` (404) worded as this service words a missing broker.
//! 2. A user change does not take effect on the broker until the broker is rebooted:
//!    `CreateUser` stages `CREATE` and `DeleteUser` stages `DELETE`
//!    (`smithy_query(query="shape", shape="ChangeType")`), both reported as the user's
//!    `pending.pendingChange` and in the broker's own `users` list, and COMPLETING a
//!    reboot is what applies them — the added user settles with nothing pending and the
//!    deleted one is gone.
//! 3. `Promote` takes the primary role of a data replication pair. Only a broker whose
//!    `DataReplicationMode` is CRDR is in a pair, so promoting any other broker is the
//!    modelled `BadRequestException` carrying Amazon MQ's wholly fixed sentence for it
//!    and leaves the broker RUNNING and unchanged; a promoted replica reports the
//!    primary role on the broker read.
//! 4. A password refused for one of `User$Password`'s documented rules is refused with
//!    Amazon MQ's sentence for that RULE, which quotes nothing about the password sent
//!    (not its length, not how many unique characters it had, not the character at
//!    fault).
//!
//! The tables span what those rules generalise over: the broker states a restart can be
//! asked for in, the `BrokerId` forms that resolve to nothing, the number of users a
//! broker holds and which change is staged on each, the engine types
//! (`ENGINE_TYPE_VALUES`) and promote modes (`PROMOTE_MODE_VALUES`) the model
//! enumerates, and the password forms each documented rule rejects and accepts.
//!
//! Every wait here is a poll for the CONDITION with a generous deadline, never a sleep
//! for a span: the transition a restart owes is due at a stored instant, so a loaded
//! host just looks again.
//!
//! The wire harness is shared with [`crate::broker_lifecycle`], so every call goes
//! through the same `MultiServiceRouter::dispatch` the front door uses.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::{ENGINE_TYPE_VALUES, PROMOTE_MODE_VALUES};
use service_com_amazonaws_mq_mq::state::{
    broker_not_found_message, broker_not_restartable_message, BROKER_STATE_CREATION_IN_PROGRESS,
    BROKER_STATE_REBOOT_IN_PROGRESS, BROKER_STATE_RUNNING, CHANGE_TYPE_CREATE, CHANGE_TYPE_DELETE,
    DATA_REPLICATION_MODE_CRDR, DATA_REPLICATION_ROLE_PRIMARY, DATA_REPLICATION_ROLE_REPLICA,
    NOT_CROSS_REGION_REPLICATED_MESSAGE, PASSWORD_FORBIDDEN_CHARACTERS_MESSAGE,
    PASSWORD_LENGTH_MESSAGE, PASSWORD_UNIQUE_CHARACTERS_MESSAGE,
};

use crate::broker_lifecycle::{create_body, string_at, Mq};
// The user helpers this suite reads a staged change and its application through are the
// ones [`crate::user_deletion`] already drives the same operations with.
use crate::user_deletion::{broker_users, create_user, describe_user};

/// How long a poll for a broker state waits before giving up. Generous on purpose: the
/// transition is due at an instant the broker recorded, so this bounds only how long a
/// badly loaded host is allowed to take to serve the read that observes it.
const PATIENCE: Duration = Duration::from_secs(60);

/// How long between two looks at a broker that has not reached the wanted state yet.
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(20);

/// A password satisfying every documented rule, for the cases that are not about
/// passwords.
const GOOD_PASSWORD: &str = "GoodPassword12";

/// `POST /v1/brokers/{BrokerId}/reboot` — the operation under test, with the error code
/// a restJson1 client branches on.
async fn reboot(mq: &Mq, broker_id: &str) -> (u16, String, Value) {
    mq.call_with_code("POST", &format!("/v1/brokers/{broker_id}/reboot"), None)
        .await
}

/// `POST /v1/brokers/{BrokerId}/promote` with `mode`.
async fn promote(mq: &Mq, broker_id: &str, mode: &str) -> (u16, String, Value) {
    mq.call_with_code(
        "POST",
        &format!("/v1/brokers/{broker_id}/promote"),
        Some(json!({"mode": mode})),
    )
    .await
}

/// `POST /v1/brokers/{BrokerId}/users/{Username}` with the error code a restJson1 client
/// branches on, which is what a refused password is read through. The accepted cases go
/// through the shared [`crate::user_deletion::create_user`].
async fn create_user_with_code(
    mq: &Mq,
    broker_id: &str,
    username: &str,
    password: &str,
) -> (u16, String, Value) {
    mq.call_with_code(
        "POST",
        &format!("/v1/brokers/{broker_id}/users/{username}"),
        Some(json!({"password": password})),
    )
    .await
}

/// The broker read taken once the broker reports `wanted`, looking again until it does
/// or [`PATIENCE`] runs out. The condition, never the clock, is what is waited on.
async fn poll_until_state(mq: &Mq, broker_id: &str, wanted: &str) -> Value {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (status, described) = mq.describe_broker(broker_id).await;
        assert_eq!(
            status, 200,
            "describe-broker {broker_id:?}: expected 200, got {status} {described:?}"
        );
        if string_at(&described, "/brokerState") == wanted {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "broker {broker_id:?}: expected state {wanted:?} before the deadline, last read \
             {described:?}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// A broker created from `body` and polled until it is RUNNING, which is the state a
/// restart and a promotion are asked for from.
async fn running_broker(mq: &Mq, body: Value) -> String {
    let (status, created) = mq.create_broker(body).await;
    assert_eq!(
        status, 200,
        "create-broker: expected 200, got {status} {created:?}"
    );
    let broker_id = string_at(&created, "/brokerId");
    poll_until_state(mq, &broker_id, BROKER_STATE_RUNNING).await;
    broker_id
}

/// RULE 1: a RUNNING broker accepts a restart, answers it with an empty 200, reports
/// `REBOOT_IN_PROGRESS` while the restart is in flight and comes back to RUNNING on its
/// own — whatever engine it runs, and however many times it is restarted.
#[tokio::test]
async fn a_running_broker_accepts_a_restart_and_comes_back_running() {
    let mq = Mq::new("us-east-1");

    // Rows: each engine the model enumerates, restarted twice in a row — the second
    // restart proves the broker is restartable again once it is back.
    for (index, engine_type) in ENGINE_TYPE_VALUES.iter().enumerate() {
        let mut body = create_body(&format!("restarted-{index}"));
        body["engineType"] = json!(engine_type);
        if *engine_type == "RABBITMQ" {
            body["deploymentMode"] = json!("CLUSTER_MULTI_AZ");
        }
        let broker_id = running_broker(&mq, body).await;

        for round in 1..=2 {
            let case = format!("{engine_type} broker, restart {round}");
            let (status, code, answer) = reboot(&mq, &broker_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {code:?} {answer:?}"
            );
            assert!(
                answer.as_object().is_some_and(serde_json::Map::is_empty)
                    || answer == Value::Null,
                "case {case:?}: the answer carries no member, got {answer:?}"
            );

            // The broker returns to RUNNING on its own, with no further call needed.
            let back = poll_until_state(&mq, &broker_id, BROKER_STATE_RUNNING).await;
            assert_eq!(
                string_at(&back, "/brokerId"),
                broker_id,
                "case {case:?}: the read must answer that very broker, got {back:?}"
            );
        }
    }
}

/// RULE 1: a restart is refused while the broker is in a state it cannot be restarted
/// from, naming the broker and that state — and the refusal changes nothing.
///
/// The rows are the two states a broker of this emulator can be in other than RUNNING:
/// still being provisioned, and already restarting. Each is transient, so the case is
/// retried on a fresh attempt when a loaded host let the span come due in between
/// rather than asserting on how fast the host ran.
#[tokio::test]
async fn a_restart_is_refused_while_the_broker_is_not_running() {
    let mq = Mq::new("eu-west-1");

    // Row 1: the create's own span. A broker that has just been created is being
    // provisioned, and a restart asked for then is refused with that state.
    let mut refused = None;
    for attempt in 0..8 {
        let (status, created) = mq
            .create_broker(create_body(&format!("provisioning-{attempt}")))
            .await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: expected 200 from create, got {status} {created:?}"
        );
        let broker_id = string_at(&created, "/brokerId");
        let answer = reboot(&mq, &broker_id).await;
        if answer.0 == 400 {
            refused = Some((
                string_at(&created, "/brokerId"),
                format!("provisioning-{attempt}"),
                answer,
            ));
            break;
        }
        // The span came due before the restart landed, so this attempt observed the
        // accepted case instead; let the broker settle and try again.
        poll_until_state(&mq, &broker_id, BROKER_STATE_RUNNING).await;
    }
    let (broker_id, broker_name, (status, code, answer)) =
        refused.expect("a restart asked for during the create's own span must be refused");
    assert_eq!(
        status, 400,
        "case \"while being provisioned\": expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        code, "BadRequestException",
        "case \"while being provisioned\": expected BadRequestException, got {code:?} {answer:?}"
    );
    assert_eq!(
        string_at(&answer, "/message"),
        broker_not_restartable_message(&broker_name, BROKER_STATE_CREATION_IN_PROGRESS),
        "case \"while being provisioned\": the sentence must name the broker and the state \
         it is in, got {answer:?}"
    );
    // Nothing changed: the broker still finishes provisioning and is then restartable.
    poll_until_state(&mq, &broker_id, BROKER_STATE_RUNNING).await;
    let (status, code, answer) = reboot(&mq, &broker_id).await;
    assert_eq!(
        status, 200,
        "case \"restartable once running\": expected 200, got {status} {code:?} {answer:?}"
    );

    // Row 2: a broker already restarting. A second restart asked for while the first is
    // in flight is refused with REBOOT_IN_PROGRESS.
    let mut refused = None;
    for attempt in 0..8 {
        let answer = reboot(&mq, &broker_id).await;
        if answer.0 == 400 {
            refused = Some(answer);
            break;
        }
        assert_eq!(
            answer.0, 200,
            "attempt {attempt}: a restart is either accepted or refused 400, got {answer:?}"
        );
        poll_until_state(&mq, &broker_id, BROKER_STATE_RUNNING).await;
    }
    let (status, code, answer) =
        refused.expect("a restart asked for while one is in flight must be refused");
    assert_eq!(
        status, 400,
        "case \"while restarting\": expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        code, "BadRequestException",
        "case \"while restarting\": expected BadRequestException, got {code:?} {answer:?}"
    );
    assert_eq!(
        string_at(&answer, "/message"),
        broker_not_restartable_message(&broker_name, BROKER_STATE_REBOOT_IN_PROGRESS),
        "case \"while restarting\": the sentence must name the state it is in, got {answer:?}"
    );
    poll_until_state(&mq, &broker_id, BROKER_STATE_RUNNING).await;
}

/// RULE 1: a `BrokerId` that resolves to no broker is the modelled `NotFoundException`,
/// worded the way this service words a missing broker everywhere — including the id of a
/// broker that HAS existed and was deleted, which is the recorded case.
#[tokio::test]
async fn restarting_a_broker_that_does_not_exist_reports_it_as_missing() {
    let mq = Mq::new("us-west-2");

    let broker_id = running_broker(&mq, create_body("to-be-deleted")).await;
    let (status, deleted) = mq
        .call("DELETE", &format!("/v1/brokers/{broker_id}"), None)
        .await;
    assert_eq!(
        status, 200,
        "setup: expected 200 from delete, got {status} {deleted:?}"
    );

    // Rows: the id of a broker that was deleted, and the id forms that never named one.
    for absent in [
        broker_id.as_str(),
        "b-00000000-0000-0000-0000-000000000000",
        "b-does-not-exist",
        "b-",
        "not-even-a-broker-id",
    ] {
        let case = format!("broker {absent:?}");
        let (status, code, answer) = reboot(&mq, absent).await;
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
            "case {case:?}: the refusal must name the member that resolved to nothing, got \
             {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/message"),
            broker_not_found_message(absent),
            "case {case:?}: expected this service's missing-broker sentence, got {answer:?}"
        );
    }
}

/// RULE 2: a user change is staged until a restart applies it. Whatever set of users a
/// broker holds, and whichever change is staged on each, the staged change is reported
/// until a restart completes, and completing one settles the additions and drops the
/// deletions.
#[tokio::test]
async fn a_restart_applies_the_user_changes_staged_on_the_broker() {
    let mq = Mq::new("us-east-1");

    // Rows: how many users are added and how many of the broker's users are deleted
    // before the restart — none of each, one of each, and several, including the user
    // the broker was created with.
    let rows: &[(&str, usize, usize)] = &[
        ("one added, none deleted", 1, 0),
        ("none added, one deleted", 0, 1),
        ("one added, one deleted", 1, 1),
        ("three added, two deleted", 3, 2),
    ];

    for (index, (case, added, deleted)) in rows.iter().enumerate() {
        let mut body = create_body(&format!("staged-{index}"));
        // The users the broker is CREATED with exist from the start, so they are the
        // ones a deletion can be staged on.
        let created_with: Vec<String> = (0..=*deleted).map(|n| format!("founder{n}")).collect();
        body["users"] = json!(created_with
            .iter()
            .map(|username| json!({"username": username, "password": GOOD_PASSWORD}))
            .collect::<Vec<Value>>());
        let broker_id = running_broker(&mq, body).await;

        // A user the broker was created with has nothing pending.
        for username in &created_with {
            let (status, user) = describe_user(&mq, &broker_id, username).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for {username:?}, got {status} {user:?}"
            );
            assert_eq!(
                user.pointer("/pending"),
                None,
                "case {case:?}: a user the broker was created with has nothing pending, got \
                 {user:?}"
            );
        }

        let additions: Vec<String> = (0..*added).map(|n| format!("added{n}")).collect();
        for username in &additions {
            let (status, answer) = create_user(
                &mq,
                &broker_id,
                username,
                json!({"password": GOOD_PASSWORD}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from create-user {username:?}, got {status} \
                 {answer:?}"
            );
            let (status, user) = describe_user(&mq, &broker_id, username).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for {username:?}, got {status} {user:?}"
            );
            assert_eq!(
                user.pointer("/pending/pendingChange").and_then(Value::as_str),
                Some(CHANGE_TYPE_CREATE),
                "case {case:?}: a user just added is pending {CHANGE_TYPE_CREATE}, got {user:?}"
            );
        }

        let deletions: Vec<String> = created_with[..*deleted].to_vec();
        for username in &deletions {
            let (status, answer) = mq
                .call(
                    "DELETE",
                    &format!("/v1/brokers/{broker_id}/users/{username}"),
                    None,
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from delete-user {username:?}, got {status} \
                 {answer:?}"
            );
            let (status, user) = describe_user(&mq, &broker_id, username).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a user pending deletion is still served, got {status} {user:?}"
            );
            assert_eq!(
                user.pointer("/pending/pendingChange").and_then(Value::as_str),
                Some(CHANGE_TYPE_DELETE),
                "case {case:?}: a user just deleted is pending {CHANGE_TYPE_DELETE}, got \
                 {user:?}"
            );
        }

        // The broker reports the same staged changes on its own users list.
        let staged = broker_users(&mq, &broker_id).await;
        for (username, pending) in &staged {
            let expected = if additions.contains(username) {
                Some(CHANGE_TYPE_CREATE)
            } else if deletions.contains(username) {
                Some(CHANGE_TYPE_DELETE)
            } else {
                None
            };
            assert_eq!(
                pending.as_deref(),
                expected,
                "case {case:?}: expected {username:?} pending {expected:?}, got {staged:?}"
            );
        }

        // The restart applies every staged change.
        let (status, code, answer) = reboot(&mq, &broker_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the restart, got {status} {code:?} {answer:?}"
        );
        poll_until_state(&mq, &broker_id, BROKER_STATE_RUNNING).await;

        for username in &additions {
            let (status, user) = describe_user(&mq, &broker_id, username).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {username:?} must be served after the restart, got {status} \
                 {user:?}"
            );
            assert_eq!(
                user.pointer("/pending"),
                None,
                "case {case:?}: the restart applied the change, so nothing is pending for \
                 {username:?}, got {user:?}"
            );
        }
        for username in &deletions {
            let (status, user) = describe_user(&mq, &broker_id, username).await;
            assert_eq!(
                status, 404,
                "case {case:?}: the restart dropped {username:?}, so it resolves to nothing, \
                 got {status} {user:?}"
            );
        }
        let settled = broker_users(&mq, &broker_id).await;
        let mut expected_names: Vec<String> = created_with[*deleted..].to_vec();
        expected_names.extend(additions.iter().cloned());
        expected_names.sort();
        let mut settled_names: Vec<String> =
            settled.iter().map(|(name, _)| name.clone()).collect();
        settled_names.sort();
        assert_eq!(
            settled_names, expected_names,
            "case {case:?}: expected the users the restart left, got {settled:?}"
        );
        assert!(
            settled.iter().all(|(_, pending)| pending.is_none()),
            "case {case:?}: a restart leaves nothing pending, got {settled:?}"
        );
    }
}

/// RULE 3: a broker that is not configured for cross-Region data replication is refused
/// a promotion in Amazon MQ's own wholly fixed words, and the call has no effect.
#[tokio::test]
async fn promoting_a_broker_that_is_not_a_replica_is_refused() {
    let mq = Mq::new("us-east-1");

    // Rows: each engine the model enumerates, each promote mode it enumerates, and both
    // ways a broker ends up in no replication pair — the mode left out of the create,
    // and the mode sent as NONE.
    let mut rows = 0usize;
    for engine_type in ENGINE_TYPE_VALUES {
        for mode in PROMOTE_MODE_VALUES {
            for replication in [None, Some("NONE")] {
                let case = format!("{engine_type}/{mode}/dataReplicationMode {replication:?}");
                let mut body = create_body(&format!("not-replica-{rows}"));
                body["engineType"] = json!(engine_type);
                if *engine_type == "RABBITMQ" {
                    body["deploymentMode"] = json!("CLUSTER_MULTI_AZ");
                }
                if let Some(replication) = replication {
                    body["dataReplicationMode"] = json!(replication);
                }
                let broker_id = running_broker(&mq, body).await;

                let (status, code, answer) = promote(&mq, &broker_id, mode).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} {answer:?}"
                );
                assert_eq!(
                    code, "BadRequestException",
                    "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
                );
                assert_eq!(
                    string_at(&answer, "/message"),
                    NOT_CROSS_REGION_REPLICATED_MESSAGE,
                    "case {case:?}: expected Amazon MQ's fixed sentence, got {answer:?}"
                );

                // The broker is left as it was: still RUNNING, still in no pair.
                let (status, described) = mq.describe_broker(&broker_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from the read, got {status} {described:?}"
                );
                assert_eq!(
                    string_at(&described, "/brokerState"),
                    BROKER_STATE_RUNNING,
                    "case {case:?}: a refused promotion leaves the broker running, got \
                     {described:?}"
                );
                assert_eq!(
                    described.pointer("/dataReplicationMetadata"),
                    None,
                    "case {case:?}: a broker in no pair reports no replication metadata, got \
                     {described:?}"
                );
                rows += 1;
            }
        }
    }
    assert_eq!(
        rows,
        ENGINE_TYPE_VALUES.len() * PROMOTE_MODE_VALUES.len() * 2,
        "every row of the table must be covered, covered {rows}"
    );
}

/// RULE 3: a broker that IS one of a replication pair reports the replica role, and a
/// promotion takes the primary role — read back through the broker read, whichever mode
/// was requested.
#[tokio::test]
async fn promoting_a_replica_takes_the_primary_role() {
    let mq = Mq::new("us-east-1");

    for (index, mode) in PROMOTE_MODE_VALUES.iter().enumerate() {
        let case = format!("mode {mode}");
        // The counterpart is a broker of the pair's other Region, which this account's
        // store does not hold: what the replica records is the ARN its create named.
        let primary_arn = format!(
            "arn:aws:mq:eu-west-1:123456789012:broker:primary-{index}:b-1111111{index}"
        );
        let mut body = create_body(&format!("replica-{index}"));
        body["dataReplicationMode"] = json!(DATA_REPLICATION_MODE_CRDR);
        body["dataReplicationPrimaryBrokerArn"] = json!(primary_arn);
        let broker_id = running_broker(&mq, body).await;

        let (status, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read, got {status} {described:?}"
        );
        assert_eq!(
            string_at(&described, "/dataReplicationMetadata/dataReplicationRole"),
            DATA_REPLICATION_ROLE_REPLICA,
            "case {case:?}: a broker created against a primary is its replica, got \
             {described:?}"
        );
        assert_eq!(
            string_at(
                &described,
                "/dataReplicationMetadata/dataReplicationCounterpart/brokerId"
            ),
            format!("b-1111111{index}"),
            "case {case:?}: the counterpart is the broker the ARN names, got {described:?}"
        );
        assert_eq!(
            string_at(
                &described,
                "/dataReplicationMetadata/dataReplicationCounterpart/region"
            ),
            "eu-west-1",
            "case {case:?}: the counterpart is in the ARN's Region, got {described:?}"
        );

        let (status, code, answer) = promote(&mq, &broker_id, mode).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {code:?} {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/brokerId"),
            broker_id,
            "case {case:?}: the answer names the promoted broker, got {answer:?}"
        );

        let (status, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read, got {status} {described:?}"
        );
        assert_eq!(
            string_at(&described, "/dataReplicationMetadata/dataReplicationRole"),
            DATA_REPLICATION_ROLE_PRIMARY,
            "case {case:?}: the promoted broker holds the primary role, got {described:?}"
        );
    }
}

/// RULE 3: a promotion of a `BrokerId` that resolves to no broker is the modelled
/// `NotFoundException`, worded as this service words a missing broker.
#[tokio::test]
async fn promoting_a_broker_that_does_not_exist_reports_it_as_missing() {
    let mq = Mq::new("us-east-1");

    for absent in ["b-00000000-0000-0000-0000-000000000000", "b-", "nobody"] {
        for mode in PROMOTE_MODE_VALUES {
            let case = format!("{absent:?}/{mode}");
            let (status, code, answer) = promote(&mq, absent, mode).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {answer:?}"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
            );
            assert_eq!(
                string_at(&answer, "/message"),
                broker_not_found_message(absent),
                "case {case:?}: expected this service's missing-broker sentence, got {answer:?}"
            );
        }
    }
}

/// RULE 4: a password is refused with Amazon MQ's sentence for the RULE it broke, and
/// that sentence quotes nothing about the password sent. A password that satisfies every
/// rule is accepted.
#[tokio::test]
async fn a_refused_password_is_refused_in_amazon_mqs_words_for_the_rule_it_broke() {
    let mq = Mq::new("us-east-1");
    let broker_id = running_broker(&mq, create_body("passwords")).await;

    // Rows span each documented rule and the forms that break it: the published bounds
    // ("at least 12 characters long", "at least 4 unique characters"), one value past
    // each, and each forbidden character (,:=). Source: `User$Password`'s own
    // documentation (`smithy_query(query="doc", shape="User", member="Password")`).
    let refusals: &[(&str, &str, &str)] = &[
        ("one unique character", "aaaaaaaaaaaa", PASSWORD_UNIQUE_CHARACTERS_MESSAGE),
        ("two unique characters", "ababababababab", PASSWORD_UNIQUE_CHARACTERS_MESSAGE),
        ("one short of the unique bound", "abcabcabcabc", PASSWORD_UNIQUE_CHARACTERS_MESSAGE),
        ("empty", "", PASSWORD_LENGTH_MESSAGE),
        ("one short of the length bound", "Abcdefghijk", PASSWORD_LENGTH_MESSAGE),
        ("a comma", "Abcdefghijk,l", PASSWORD_FORBIDDEN_CHARACTERS_MESSAGE),
        ("a colon", "Abcdefghijk:l", PASSWORD_FORBIDDEN_CHARACTERS_MESSAGE),
        ("an equal sign", "Abcdefghijk=l", PASSWORD_FORBIDDEN_CHARACTERS_MESSAGE),
    ];

    for (index, (case, password, expected)) in refusals.iter().enumerate() {
        let username = format!("refused{index}");
        let (status, code, answer) =
            create_user_with_code(&mq, &broker_id, &username, password).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
        );
        let message = string_at(&answer, "/message");
        assert_eq!(
            &message, expected,
            "case {case:?}: expected Amazon MQ's sentence for the rule, got {answer:?}"
        );
        // The sentence states the rule, so nothing about the rejected secret is in it.
        if !password.is_empty() {
            assert!(
                !message.contains(password),
                "case {case:?}: the refusal must not quote the password, got {message:?}"
            );
        }
        // And nothing was created.
        let (status, user) = describe_user(&mq, &broker_id, &username).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create stores no user, got {status} {user:?}"
        );
    }

    // The accepted rows: each published bound exactly met, and a longer password.
    for (index, (case, password)) in [
        ("exactly the bounds", "abcdabcdabcd"),
        ("four unique characters, longer", "abcdabcdabcdabcd"),
        ("a password with punctuation that is allowed", "Abcd-efgh.ijkl~1"),
    ]
    .iter()
    .enumerate()
    {
        let username = format!("accepted{index}");
        let (status, answer) =
            create_user(&mq, &broker_id, &username, json!({"password": password})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answer:?}"
        );
        let (status, user) = describe_user(&mq, &broker_id, &username).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the user must read back, got {status} {user:?}"
        );
    }
}
