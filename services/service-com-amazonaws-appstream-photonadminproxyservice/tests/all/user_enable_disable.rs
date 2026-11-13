// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for disabling and enabling an AppStream user pool user over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * `DisableUser` and `EnableUser` move the stored user's `Enabled` flag and nothing
//!   else, so a later read reports the same user with the same ARN, names, status and
//!   creation instant and only that flag changed — and because the record stays in the
//!   pool, disabling is not a delete and the identity is still taken;
//! * either call asks for a state rather than for a change, so repeating it (or a whole
//!   sequence of them) leaves the user in the state the last call named, and the first
//!   and the repeat answer alike;
//! * the flag belongs to the one identity the request names: the other users of the pool
//!   are untouched;
//! * an identity the pool holds no record for — a name never created, a name differing
//!   only in capitalisation, or a name under an authentication type whose users this pool
//!   does not manage — is refused with `ResourceNotFoundException` (HTTP 400) and changes
//!   nothing;
//! * a request outside a modelled constraint on the user name is refused and changes
//!   nothing.
//!
//! No case here depends on how fast the host runs it: where the pool's answer changes once
//! an allocation has caught up, the case waits for THAT condition in a poll loop instead
//! of banking on the window not having elapsed yet.

use std::time::Duration;

use serde_json::{json, Value};

use crate::refusal_messages::{error_message, refusal};
use crate::stack_fleet_lifecycle::{assert_error, error_code, Emulator};

/// The only authentication type the user pool operations manage, which
/// `DisableUser.AuthenticationType` and `EnableUser.AuthenticationType` both document as
/// "You must specify USERPOOL".
const USER_POOL: &str = "USERPOOL";

/// The other authentication types the model declares for these operations
/// (`AuthenticationType`'s values, kept as `AUTHENTICATION_TYPE_VALUES` in
/// `src/ops/types/shared_types.rs`). None of them names a user pool identity, so the pool
/// holds no user under one.
const OTHER_AUTHENTICATION_TYPES: &[&str] = &["API", "AWS_AD", "SAML"];

/// AWS's sentence for a user pool identity that is not in the pool. It has no variable
/// part, so the refusal echoes nothing from the request back.
const USER_NOT_FOUND_MESSAGE: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::USER_NOT_FOUND_MESSAGE;

/// AWS's sentence for a user pool username that is already taken. It has no variable part
/// either, so this refusal echoes nothing from the request back.
const USER_ALREADY_EXISTS_MESSAGE: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::USER_ALREADY_EXISTS_MESSAGE;

/// The length bounds the model puts on `Username` (len 1..128).
const USER_NAME_MAX: usize = 128;

/// The two operations that move the flag, each with the value it writes.
const TRANSITIONS: &[(&str, bool)] = &[("DisableUser", false), ("EnableUser", true)];

/// One row of the user table: the identity to create, and the optional attributes the
/// create carries, so the assertions hold for a user with names and for one without.
struct Row {
    case: &'static str,
    user_name: String,
    first_name: Option<&'static str>,
    last_name: Option<&'static str>,
}

/// The user table. The rows differ along what the rule generalises over: the form and the
/// size of the user name (ASCII email, tagged address, non-ASCII letters, the shortest and
/// the longest name the model allows) and which attributes the user carries beside its
/// identity.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "both names",
            user_name: "first.last@example.com".to_string(),
            first_name: Some("Ada"),
            last_name: Some("Lovelace"),
        },
        Row {
            case: "no names",
            user_name: "plain@example.com".to_string(),
            first_name: None,
            last_name: None,
        },
        Row {
            case: "tagged address, first name only",
            user_name: "tagged+label@example.com".to_string(),
            first_name: Some("Grace Brewster"),
            last_name: None,
        },
        Row {
            case: "non-ASCII user name, last name only",
            user_name: "ユーザー@example.com".to_string(),
            first_name: None,
            last_name: Some("Hopper-Murray"),
        },
        Row {
            case: "shortest user name the model allows",
            user_name: "a".to_string(),
            first_name: None,
            last_name: None,
        },
        Row {
            case: "longest user name the model allows",
            user_name: "b".repeat(USER_NAME_MAX - "@example.com".len()) + "@example.com",
            first_name: Some("Edge"),
            last_name: Some("Case"),
        },
    ]
}

/// The `CreateUser` request that allocates a row's identity.
fn create_request(row: &Row) -> Value {
    let mut request = json!({
        "UserName": row.user_name,
        "AuthenticationType": USER_POOL,
    });
    if let Some(first_name) = row.first_name {
        request["FirstName"] = json!(first_name);
    }
    if let Some(last_name) = row.last_name {
        request["LastName"] = json!(last_name);
    }
    request
}

/// The identity members a `DisableUser`/`EnableUser` request carries.
fn transition_request(user_name: &str, authentication_type: &str) -> Value {
    json!({
        "UserName": user_name,
        "AuthenticationType": authentication_type,
    })
}

/// Every user of the pool, in the order the read reports them.
async fn user_pool(emulator: &Emulator, case: &str) -> Vec<Value> {
    let payload = emulator
        .ok(case, "DescribeUsers", json!({ "AuthenticationType": USER_POOL }))
        .await;
    assert!(
        payload.get("NextToken").is_none(),
        "case {case:?}: the whole pool fits in one page here, got {payload}"
    );
    payload
        .get("Users")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Users list, got {payload}"))
        .clone()
}

/// The one user the pool holds for `user_name`.
async fn stored_user(emulator: &Emulator, case: &str, user_name: &str) -> Value {
    user_pool(emulator, case)
        .await
        .into_iter()
        .find(|user| user.get("UserName").and_then(Value::as_str) == Some(user_name))
        .unwrap_or_else(|| panic!("case {case:?}: the pool must still hold {user_name:?}"))
}

/// The reported user with its `Enabled` flag dropped: what must be identical before and
/// after a transition, since the flag is the only thing either call moves.
fn without_enabled(case: &str, user: &Value) -> Value {
    let mut rest = user.clone();
    let members = rest
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {case:?}: a reported user is a structure, got {user}"));
    assert!(
        members.remove("Enabled").is_some(),
        "case {case:?}: a reported user carries Enabled, got {user}"
    );
    rest
}

/// The `Enabled` flag of a reported user.
fn enabled(case: &str, user: &Value) -> bool {
    user.get("Enabled")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: expected an Enabled flag, got {user}"))
}

/// Create the identity `row` already holds until the pool reports its username as taken,
/// and return the sentence that refusal carries.
///
/// The pool's username index catches up with an allocation on its own
/// (`state::USER_ALLOCATION_GRACE`), and once it has caught up it stays caught up — so
/// this waits for that CONDITION rather than for a span, and asserts what every attempt
/// must leave behind whichever side of it the attempt lands on: an attempt that still
/// races the allocation collapses into it and is answered the same empty success, an
/// attempt afterwards is refused as a username that is taken, and neither allocates the
/// identity a second time nor hands the sign-in flag back. Nothing in the service sleeps
/// for the span; only this loop looks again.
async fn create_until_the_username_is_reported_taken(
    emulator: &Emulator,
    case: &str,
    row: &Row,
) -> String {
    // Generous: the window an allocation stays open for is milliseconds, so this deadline
    // is reached only if the username is never reported taken at all.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let (status, payload) = emulator.call("CreateUser", create_request(row)).await;

        // Whatever this attempt answered, the pool still holds the single disabled record
        // the disable left behind.
        let pool = user_pool(emulator, case).await;
        assert_eq!(
            pool.len(),
            1,
            "case {case:?}: a create of a held identity allocates nothing, got {pool:?}"
        );
        let after = stored_user(emulator, case, &row.user_name).await;
        assert!(
            !enabled(case, &after),
            "case {case:?}: re-creating a held identity must not re-enable the user, got \
             {after}"
        );

        if status != 200 {
            assert_eq!(
                status, 400,
                "case {case:?}: a create of a taken username answers HTTP 400, got {status} \
                 {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceAlreadyExistsException",
                "case {case:?}: a taken username is refused as already existing, got {payload}"
            );
            return error_message(&payload);
        }

        assert_eq!(
            payload,
            json!({}),
            "case {case:?}: a create that still races the allocation collapses into it and \
             carries no members, got {payload}"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the pool must report {:?} as taken once its allocation has \
             caught up, still answering an empty success",
            row.user_name
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// The flag, and only the flag, moves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disabling_then_enabling_moves_only_the_stored_sign_in_flag() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.ok(case, "CreateUser", create_request(&row)).await;

        let created = stored_user(&emulator, case, &row.user_name).await;
        assert!(
            enabled(case, &created),
            "case {case:?}: a created user may sign in, got {created}"
        );

        let answer = emulator
            .ok(
                case,
                "DisableUser",
                transition_request(&row.user_name, USER_POOL),
            )
            .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case:?}: the modelled result carries no members, got {answer}"
        );

        let disabled = stored_user(&emulator, case, &row.user_name).await;
        assert!(
            !enabled(case, &disabled),
            "case {case:?}: the disabled user must not be able to sign in, got {disabled}"
        );
        assert_eq!(
            without_enabled(case, &disabled),
            without_enabled(case, &created),
            "case {case:?}: disabling moves the flag and nothing else, got {disabled}"
        );

        let answer = emulator
            .ok(
                case,
                "EnableUser",
                transition_request(&row.user_name, USER_POOL),
            )
            .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case:?}: the modelled result carries no members, got {answer}"
        );

        let reenabled = stored_user(&emulator, case, &row.user_name).await;
        assert_eq!(
            reenabled, created,
            "case {case:?}: enabling the user again restores exactly the created record, \
             got {reenabled}"
        );
    }
}

#[tokio::test]
async fn disabling_a_user_does_not_delete_it() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.ok(case, "CreateUser", create_request(&row)).await;
        emulator
            .ok(
                case,
                "DisableUser",
                transition_request(&row.user_name, USER_POOL),
            )
            .await;

        let pool = user_pool(&emulator, case).await;
        assert_eq!(
            pool.len(),
            1,
            "case {case:?}: the disabled user is still in the pool, got {pool:?}"
        );

        // The identity is still taken, so a create of the same user allocates nothing: it
        // describes the user that is already there while the allocation is still open, and
        // is refused as a username that is taken once the pool's index has caught up.
        // Neither answer hands the flag back either: only `EnableUser` does that, which is
        // asserted on every attempt the loop makes.
        // The sentence has no variable part, so comparing it whole is also what says the
        // refusal echoes nothing from the request back.
        let message = create_until_the_username_is_reported_taken(&emulator, case, &row).await;
        assert_eq!(
            message, USER_ALREADY_EXISTS_MESSAGE,
            "case {case:?}: expected message {USER_ALREADY_EXISTS_MESSAGE:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Either call asks for a state, so repeating it changes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn repeating_a_transition_leaves_the_user_in_the_state_it_names() {
    // Rows: sequences of calls on one user, differing in length, in which call comes
    // first and in how often each is repeated. Whatever the sequence, every call is
    // answered the same empty success and the user ends in the state its LAST call names.
    for operations in [
        vec!["DisableUser"],
        vec!["DisableUser", "DisableUser"],
        vec!["EnableUser"],
        vec!["EnableUser", "EnableUser", "EnableUser"],
        vec!["DisableUser", "EnableUser"],
        vec!["EnableUser", "DisableUser"],
        vec!["DisableUser", "DisableUser", "EnableUser", "EnableUser"],
        vec!["DisableUser", "EnableUser", "DisableUser", "EnableUser", "DisableUser"],
    ] {
        let case = operations.join(" then ");
        let emulator = Emulator::new();
        let row = Row {
            case: "repeated",
            user_name: "repeat@example.com".to_string(),
            first_name: Some("Ada"),
            last_name: Some("Lovelace"),
        };
        emulator.ok(&case, "CreateUser", create_request(&row)).await;
        let created = stored_user(&emulator, &case, &row.user_name).await;

        for operation in &operations {
            let answer = emulator
                .ok(
                    &case,
                    operation,
                    transition_request(&row.user_name, USER_POOL),
                )
                .await;
            assert_eq!(
                answer,
                json!({}),
                "case {case:?}: every {operation} answers the same empty result, got {answer}"
            );
        }

        let expected = TRANSITIONS
            .iter()
            .find(|(operation, _)| Some(operation) == operations.last())
            .map(|(_, enabled)| *enabled)
            .unwrap_or_else(|| panic!("case {case:?}: the sequence names a known transition"));
        let user = stored_user(&emulator, &case, &row.user_name).await;
        assert_eq!(
            enabled(&case, &user),
            expected,
            "case {case:?}: the user must end Enabled={expected}, got {user}"
        );
        assert_eq!(
            without_enabled(&case, &user),
            without_enabled(&case, &created),
            "case {case:?}: no repeat may change anything but the flag, got {user}"
        );
    }
}

#[tokio::test]
async fn a_transition_moves_the_flag_of_the_named_user_only() {
    // Rows: which of the pool's users the call names, so the assertion holds wherever the
    // named user sits in the stored order rather than for one position.
    let names = [
        "a-first@example.com",
        "m-middle@example.com",
        "z-last@example.com",
    ];
    for (operation, writes) in TRANSITIONS {
        for named in names {
            let case = format!("{operation} on {named}");
            let emulator = Emulator::new();
            for name in names {
                emulator
                    .ok(
                        &case,
                        "CreateUser",
                        json!({ "UserName": name, "AuthenticationType": USER_POOL }),
                    )
                    .await;
            }
            // Start from the opposite state, so the call under test has something to move.
            let setup = if *writes { "DisableUser" } else { "EnableUser" };
            for name in names {
                emulator
                    .ok(&case, setup, transition_request(name, USER_POOL))
                    .await;
            }

            emulator
                .ok(&case, operation, transition_request(named, USER_POOL))
                .await;

            for name in names {
                let user = stored_user(&emulator, &case, name).await;
                let expected = if name == named { *writes } else { !*writes };
                assert_eq!(
                    enabled(&case, &user),
                    expected,
                    "case {case:?}: {name} must be Enabled={expected}, got {user}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_transition_for_a_user_the_pool_does_not_hold_is_refused() {
    // Rows: identities no record is stored under — a name never created, the stored name
    // with its capitalisation changed (email addresses are case-sensitive, so that is a
    // different identity), and the stored name under each of the other authentication
    // types, whose users this pool does not manage.
    const STORED: &str = "stored@example.com";
    let mut identities: Vec<(String, String, String)> = vec![
        (
            "a user name never created".to_string(),
            "absent@example.com".to_string(),
            USER_POOL.to_string(),
        ),
        (
            "the stored name in a different capitalisation".to_string(),
            "Stored@example.com".to_string(),
            USER_POOL.to_string(),
        ),
    ];
    for authentication_type in OTHER_AUTHENTICATION_TYPES {
        identities.push((
            format!("the stored name under {authentication_type}"),
            STORED.to_string(),
            (*authentication_type).to_string(),
        ));
    }

    for (operation, _) in TRANSITIONS {
        for (row, user_name, authentication_type) in &identities {
            let case = format!("{operation}: {row}");
            let emulator = Emulator::new();
            emulator
                .ok(
                    &case,
                    "CreateUser",
                    json!({ "UserName": STORED, "AuthenticationType": USER_POOL }),
                )
                .await;
            let before = stored_user(&emulator, &case, STORED).await;

            let message = refusal(
                &emulator,
                &case,
                operation,
                transition_request(user_name, authentication_type),
                400,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                message, USER_NOT_FOUND_MESSAGE,
                "case {case:?}: expected message {USER_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(user_name.as_str()),
                "case {case:?}: the sentence states that no user has that username without \
                 echoing it, got {message:?}"
            );

            let after = stored_user(&emulator, &case, STORED).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused transition must leave the pool as it was, got \
                 {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_user_name_outside_the_models_constraints_is_refused() {
    // Rows: user names outside the published bounds — one shorter than the minimum of 1,
    // one past the maximum of 128 — and one outside `Username`'s character pattern.
    for (operation, _) in TRANSITIONS {
        let emulator = Emulator::new();
        emulator
            .ok(
                "setup",
                "CreateUser",
                json!({ "UserName": "constrained@example.com", "AuthenticationType": USER_POOL }),
            )
            .await;

        for (row, user_name) in [
            ("shorter than the minimum", String::new()),
            (
                "one past the maximum",
                "a".repeat(USER_NAME_MAX + 1 - "@example.com".len()) + "@example.com",
            ),
            (
                "a space, outside the pattern",
                "first last@example.com".to_string(),
            ),
        ] {
            let case = format!("{operation}: {row}");
            assert_error(
                &emulator,
                &case,
                operation,
                transition_request(&user_name, USER_POOL),
                400,
                "ValidationException",
            )
            .await;
        }

        let user = stored_user(&emulator, operation, "constrained@example.com").await;
        assert!(
            enabled(operation, &user),
            "case {operation:?}: a refused request must leave the stored user as it was, \
             got {user}"
        );
    }
}
