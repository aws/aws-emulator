// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how AppStream allocates a user pool identity, and for what a
//! create of an identity that is already there answers.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * a `(UserName, AuthenticationType)` identity is allocated AT MOST ONCE, and once it is
//!   ESTABLISHED a further create of it is refused `ResourceAlreadyExistsException` (400)
//!   with AWS's sentence — which states that the username is taken and echoes nothing —
//!   whatever that create carries, and leaves the stored user exactly as it was;
//! * creates ISSUED TOGETHER on one identity race its allocation: every one of them
//!   succeeds and the pool holds one enabled user for the name afterwards, however many
//!   there are;
//! * deleting the identity frees the username: a create afterwards allocates it again,
//!   while a delete of an identity the pool does not hold is refused
//!   `ResourceNotFoundException` (400) — gone means unusable, not merely unlisted.

use std::time::Duration;

use serde_json::{json, Value};

use crate::refusal_messages::refusal;
use crate::stack_fleet_lifecycle::Emulator;

/// The only authentication type the user pool operations accept, which both
/// `CreateUser.AuthenticationType` and `DescribeUsers.AuthenticationType` document as
/// "You must specify USERPOOL".
const USER_POOL: &str = "USERPOOL";

/// AWS's sentences for a username that is taken and for an identity the pool does not
/// hold, read from the crate so the test tracks what the service states.
const ALREADY_EXISTS: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::USER_ALREADY_EXISTS_MESSAGE;
const NOT_FOUND: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::USER_NOT_FOUND_MESSAGE;

/// How long the allocation of an identity stays open to a create racing it, read from the
/// crate so the test tracks the constant rather than a copy of its value.
const ALLOCATION_GRACE: Duration =
    service_com_amazonaws_appstream_photonadminproxyservice::state::USER_ALLOCATION_GRACE;

/// Wait until the identity a create allocated is established, which is what makes a
/// further create of it a claim on a username that is taken. Nothing in the service sleeps
/// for the span; only this test waits for it.
async fn settle() {
    tokio::time::sleep(ALLOCATION_GRACE + Duration::from_millis(50)).await;
}

/// The users a read of the pool reports.
async fn user_pool(emulator: &Emulator, case: &str) -> Vec<Value> {
    let payload = emulator
        .ok(
            case,
            "DescribeUsers",
            json!({ "AuthenticationType": USER_POOL }),
        )
        .await;
    payload
        .get("Users")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One row of the duplicate table: the user the first create allocates, and what the
/// second create carries beside the identity.
struct Row {
    case: &'static str,
    user_name: &'static str,
    /// The members the FIRST create carries beside the identity.
    created: Value,
    /// The members the SECOND create carries beside the identity.
    repeated: Value,
}

/// The duplicate table. The rows differ along what the rule generalises over: the form of
/// the user name (ASCII email, tagged address, non-ASCII letters, a bare name), what the
/// first create stored, and what the second create asks for — identical, bare, a welcome
/// email resent, or a different attribute. Once the identity is established every one of
/// them is a claim on a username that is taken.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the identical request again",
            user_name: "settled@example.com",
            created: json!({ "MessageAction": "SUPPRESS" }),
            repeated: json!({ "MessageAction": "SUPPRESS" }),
        },
        Row {
            case: "only the identity",
            user_name: "tagged+label@example.com",
            created: json!({ "FirstName": "Ada", "LastName": "Lovelace" }),
            repeated: json!({}),
        },
        Row {
            case: "the welcome email resent",
            user_name: "ユーザー@example.com",
            created: json!({ "MessageAction": "SUPPRESS" }),
            repeated: json!({ "MessageAction": "RESEND" }),
        },
        Row {
            case: "a different first name",
            user_name: "collision",
            created: json!({ "FirstName": "Ada" }),
            repeated: json!({ "FirstName": "Augusta" }),
        },
        Row {
            case: "every attribute named again",
            user_name: "grace.hopper@example.com",
            created: json!({ "FirstName": "Grace", "LastName": "Hopper" }),
            repeated: json!({ "FirstName": "Grace", "LastName": "Hopper" }),
        },
    ]
}

/// The `CreateUser` request for `user_name` carrying `extra`.
fn create_request(user_name: &str, extra: &Value) -> Value {
    let mut request = json!({ "UserName": user_name, "AuthenticationType": USER_POOL });
    for (member, value) in extra.as_object().expect("the row carries members") {
        request[member] = value.clone();
    }
    request
}

#[tokio::test]
async fn a_create_of_an_established_identity_is_refused_as_a_username_that_is_taken() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        emulator
            .ok(
                case,
                "CreateUser",
                create_request(row.user_name, &row.created),
            )
            .await;

        // The identity is observable, and the clock has settled its allocation.
        let stored = user_pool(&emulator, case).await;
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: the create must have allocated exactly the one identity, got \
             {stored:?}"
        );
        settle().await;

        let message = refusal(
            &emulator,
            case,
            "CreateUser",
            create_request(row.user_name, &row.repeated),
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        assert_eq!(
            message, ALREADY_EXISTS,
            "case {case:?}: expected AWS's sentence {ALREADY_EXISTS:?}, got {message:?}"
        );
        assert!(
            !message.contains(row.user_name),
            "case {case:?}: the sentence states that the username is taken without echoing \
             it, got {message:?}"
        );

        // The refused create changed nothing about the user that is there.
        let after = user_pool(&emulator, case).await;
        assert_eq!(
            after, stored,
            "case {case:?}: a refused create must leave the stored user exactly as it was, \
             got {after:?}"
        );
    }
}

#[tokio::test]
async fn an_identity_differing_only_in_capitalisation_is_still_free() {
    // Email addresses are case-sensitive, so the refusal above is about the name the
    // request spells and not about any other spelling of it.
    let case = "another capitalisation of a settled identity";
    let emulator = Emulator::new();
    emulator
        .ok(case, "CreateUser", create_request("settled@example.com", &json!({})))
        .await;
    settle().await;

    emulator
        .ok(case, "CreateUser", create_request("Settled@example.com", &json!({})))
        .await;
    let stored = user_pool(&emulator, case).await;
    assert_eq!(
        stored.len(),
        2,
        "case {case:?}: each spelling is its own identity, got {stored:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn creates_issued_together_on_one_identity_all_succeed_and_allocate_it_once() {
    // Rows: how many callers issue the create together, and what each of them carries.
    // However many of them arrive at once, each is answered 200 and the pool holds one
    // enabled user for the name afterwards.
    for (racers, extra) in [
        (2_usize, json!({})),
        (6, json!({ "MessageAction": "SUPPRESS" })),
        (16, json!({ "FirstName": "Ada" })),
    ] {
        let case = format!("{racers} creates issued together");
        let emulator = std::sync::Arc::new(Emulator::new());
        let request = create_request("together@example.com", &extra);

        let mut issued = Vec::new();
        for _ in 0..racers {
            let emulator = std::sync::Arc::clone(&emulator);
            let request = request.clone();
            issued.push(tokio::spawn(async move {
                emulator.call("CreateUser", request).await
            }));
        }
        for call in issued {
            let (status, payload) = call.await.expect("the racing call completes");
            assert_eq!(
                status, 200,
                "case {case:?}: every create racing one allocation must succeed, got \
                 {status} {payload}"
            );
            assert_eq!(
                payload,
                json!({}),
                "case {case:?}: every one of them gets the same empty result, got {payload}"
            );
        }

        let stored = user_pool(&emulator, &case).await;
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: the identity must be allocated exactly once, got {stored:?}"
        );
        assert_eq!(
            stored[0].get("Enabled"),
            Some(&json!(true)),
            "case {case:?}: the user they created must be enabled, got {:?}",
            stored[0]
        );

        // Once that allocation has settled, a further create is a claim on a username
        // that is taken.
        settle().await;
        refusal(
            &emulator,
            &case,
            "CreateUser",
            request,
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
    }
}

#[tokio::test]
async fn deleting_an_identity_frees_the_username_and_a_repeat_delete_is_not_found() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let identity = json!({ "UserName": row.user_name, "AuthenticationType": USER_POOL });
        emulator
            .ok(
                case,
                "CreateUser",
                create_request(row.user_name, &row.created),
            )
            .await;
        settle().await;

        let deleted = emulator.ok(case, "DeleteUser", identity.clone()).await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case:?}: the modelled result carries no members, got {deleted}"
        );
        let emptied = user_pool(&emulator, case).await;
        assert!(
            emptied.is_empty(),
            "case {case:?}: the deleted identity must be gone from the pool, got {emptied:?}"
        );

        // Gone means unusable: the identity may not be deleted again.
        let message = refusal(
            &emulator,
            case,
            "DeleteUser",
            identity,
            400,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, NOT_FOUND,
            "case {case:?}: expected AWS's sentence {NOT_FOUND:?}, got {message:?}"
        );

        // And the username is free again: a create after the delete allocates a fresh
        // identity rather than being refused as taken.
        emulator
            .ok(
                case,
                "CreateUser",
                create_request(row.user_name, &row.repeated),
            )
            .await;
        let recreated = user_pool(&emulator, case).await;
        assert_eq!(
            recreated.len(),
            1,
            "case {case:?}: the freed username must be allocatable again, got {recreated:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_user_takes_the_stack_entitlements_of_that_identity_with_it() {
    // A user entitled to stacks: deleting the identity leaves nothing naming a user the
    // pool no longer holds.
    let case = "an entitled user";
    let emulator = Emulator::new();
    let user = "entitled@example.com";
    for stack in ["entitled-stack-a", "entitled-stack-b"] {
        emulator
            .ok(case, "CreateStack", json!({ "Name": stack }))
            .await;
    }
    emulator
        .ok(case, "CreateUser", create_request(user, &json!({})))
        .await;
    emulator
        .ok(
            case,
            "BatchAssociateUserStack",
            json!({
                "UserStackAssociations": [
                    {
                        "UserName": user,
                        "AuthenticationType": USER_POOL,
                        "StackName": "entitled-stack-a",
                        "SendEmailNotification": false,
                    },
                    {
                        "UserName": user,
                        "AuthenticationType": USER_POOL,
                        "StackName": "entitled-stack-b",
                        "SendEmailNotification": false,
                    },
                ]
            }),
        )
        .await;

    let described = emulator
        .ok(
            case,
            "DescribeUserStackAssociations",
            json!({ "UserName": user, "AuthenticationType": USER_POOL }),
        )
        .await;
    assert_eq!(
        described
            .get("UserStackAssociations")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default(),
        2,
        "case {case:?}: the batch must have entitled the user to both stacks, got \
         {described}"
    );

    emulator
        .ok(
            case,
            "DeleteUser",
            json!({ "UserName": user, "AuthenticationType": USER_POOL }),
        )
        .await;
    let after = emulator
        .ok(
            case,
            "DescribeUserStackAssociations",
            json!({ "UserName": user, "AuthenticationType": USER_POOL }),
        )
        .await;
    let remaining = after
        .get("UserStackAssociations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        remaining.is_empty(),
        "case {case:?}: nothing may still entitle a user the pool no longer holds, got \
         {remaining:?}"
    );
}
