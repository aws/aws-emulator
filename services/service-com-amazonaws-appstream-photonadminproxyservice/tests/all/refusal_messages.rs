// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MESSAGE every AppStream control-plane refusal carries.
//!
//! Each case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is asserted is
//! the body a client actually parses.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * a refusal for a resource the account does not hold carries AWS's sentence for that
//!   error class — the resource kind, the name the REQUEST asked for and the account
//!   that was searched — whichever operation reports it;
//! * a refusal for a name already taken carries AWS's duplicate-name sentence, with the
//!   same two variable parts;
//! * a refusal because a fleet/stack association still exists states what blocks the
//!   delete and how to clear it, and names NONE of the resources on the other side of
//!   the association, however many there are.

use serde_json::{json, Value};

use crate::stack_fleet_lifecycle::{error_code, Emulator};

/// The account every refusal names: the single account this emulator presents, so the
/// sentence reports the account that was searched rather than a value from one run.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The resources these tests leave stored, so a refusal row can make exactly one side of
/// a two-sided request the missing one.
const STORED_FLEET: &str = "stored-fleet";
const STORED_STACK: &str = "stored-stack";

/// AWS's `ResourceNotFoundException` sentence for a named resource the account does not
/// hold.
///
/// Shared with the other suites in this target (`tests/all/stack_registry.rs`), so every
/// suite states the sentence once.
pub(crate) fn not_found_sentence(kind: &str, name: &str) -> String {
    format!("The {kind} {name} for account {ACCOUNT} does not exist.")
}

/// AWS's `ResourceAlreadyExistsException` sentence for a name already taken. The two
/// spaces after the account id are AWS's own.
///
/// Shared with the other suites in this target (`tests/all/stack_theme.rs`), so every
/// suite states the sentence once.
pub(crate) fn already_exists_sentence(kind: &str, name: &str) -> String {
    format!(
        "The {kind} {name} for account {ACCOUNT}  already exists. \
         Use a unique name within an AWS region in your AWS account."
    )
}

/// The `Message` of a failure body (the modelled member is `Message`; the synthetic
/// channel spells it `message`).
///
/// Shared with the other suites in this target (`tests/all/user_pool.rs`), so every
/// message assertion reads the body the same way.
pub(crate) fn error_message(payload: &Value) -> String {
    payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Send a request that must be refused with `status` and `code`, and return its message.
pub(crate) async fn refusal(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: Value,
    status: u16,
    code: &str,
) -> String {
    let (actual_status, payload) = emulator.call(operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: {operation} must answer HTTP {status}, got {actual_status} {payload}"
    );
    assert_eq!(
        error_code(&payload),
        code,
        "case {case:?}: {operation} must answer {code}, got {payload}"
    );
    error_message(&payload)
}

/// Names spanning the class: the rows differ along length, character classes and case.
/// The last is the longest name a create admits — the `Name` member's pattern is
/// `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`
/// (`smithy_query(query="operation", operation="CreateStack")`), so 101 characters.
fn names() -> Vec<String> {
    vec![
        "absent".to_string(),
        "Mixed.Case_name-9".to_string(),
        "0123456789".to_string(),
        format!("z{}", "9".repeat(100)),
    ]
}

/// One not-found row: the operation that reports it, the resource kind its refusal
/// names, and how the request asks for `name`.
struct NotFoundRow {
    operation: &'static str,
    kind: &'static str,
    request: fn(&str) -> Value,
}

/// Every implemented operation that refuses a name the account does not hold, for both
/// resource kinds: a delete, a reader, and both sides of the two-sided association
/// operations.
fn not_found_rows() -> Vec<NotFoundRow> {
    vec![
        NotFoundRow {
            operation: "DeleteFleet",
            kind: "fleet",
            request: |name| json!({ "Name": name }),
        },
        NotFoundRow {
            operation: "DescribeFleets",
            kind: "fleet",
            request: |name| json!({ "Names": [name] }),
        },
        NotFoundRow {
            operation: "AssociateFleet",
            kind: "fleet",
            request: |name| json!({ "FleetName": name, "StackName": STORED_STACK }),
        },
        NotFoundRow {
            operation: "DisassociateFleet",
            kind: "fleet",
            request: |name| json!({ "FleetName": name, "StackName": STORED_STACK }),
        },
        NotFoundRow {
            operation: "DeleteStack",
            kind: "stack",
            request: |name| json!({ "Name": name }),
        },
        NotFoundRow {
            operation: "DescribeStacks",
            kind: "stack",
            request: |name| json!({ "Names": [name] }),
        },
        NotFoundRow {
            operation: "UpdateStack",
            kind: "stack",
            request: |name| json!({ "Name": name, "Description": "an update nobody applies" }),
        },
        NotFoundRow {
            operation: "AssociateFleet",
            kind: "stack",
            request: |name| json!({ "FleetName": STORED_FLEET, "StackName": name }),
        },
        NotFoundRow {
            operation: "DisassociateFleet",
            kind: "stack",
            request: |name| json!({ "FleetName": STORED_FLEET, "StackName": name }),
        },
    ]
}

/// Store one fleet and one stack, so a two-sided request can have exactly one missing
/// side.
async fn emulator_with_stored_pair() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok("setup", "CreateStack", json!({ "Name": STORED_STACK }))
        .await;
    emulator
        .ok(
            "setup",
            "CreateFleet",
            json!({ "Name": STORED_FLEET, "InstanceType": "stream.standard.small" }),
        )
        .await;
    emulator
}

#[tokio::test]
async fn a_missing_resource_is_refused_with_its_kind_name_and_account() {
    let emulator = emulator_with_stored_pair().await;

    for row in not_found_rows() {
        for name in names() {
            let case = format!("{} {} {name:?}", row.operation, row.kind);
            let message = refusal(
                &emulator,
                &case,
                row.operation,
                (row.request)(&name),
                400,
                "ResourceNotFoundException",
            )
            .await;

            let expected = not_found_sentence(row.kind, &name);
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_already_taken_is_refused_with_awss_duplicate_name_sentence() {
    // Rows: the two resources a name can collide on, each created with the members that
    // create requires.
    struct CreateRow {
        operation: &'static str,
        kind: &'static str,
        request: fn(&str) -> Value,
    }

    let creates = vec![
        CreateRow {
            operation: "CreateStack",
            kind: "stack",
            request: |name| json!({ "Name": name }),
        },
        CreateRow {
            operation: "CreateFleet",
            kind: "fleet",
            request: |name| json!({ "Name": name, "InstanceType": "stream.standard.small" }),
        },
    ];

    for row in creates {
        for name in names() {
            let emulator = Emulator::new();
            let case = format!("{} {name:?}", row.operation);
            emulator
                .ok(&case, row.operation, (row.request)(&name))
                .await;

            let message = refusal(
                &emulator,
                &case,
                row.operation,
                (row.request)(&name),
                400,
                "ResourceAlreadyExistsException",
            )
            .await;

            let expected = already_exists_sentence(row.kind, &name);
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_an_associated_fleet_is_refused_without_naming_the_stacks() {
    // Rows: how many stacks the fleet is associated with. The sentence is the same for
    // one as for many, and names none of them.
    for stack_count in 1..=3_usize {
        for fleet_name in names() {
            let case = format!("{fleet_name:?} associated with {stack_count} stack(s)");
            let emulator = Emulator::new();
            emulator
                .ok(
                    &case,
                    "CreateFleet",
                    json!({ "Name": fleet_name, "InstanceType": "stream.standard.small" }),
                )
                .await;

            let stacks: Vec<String> = (0..stack_count)
                .map(|index| format!("partner-stack-{index}"))
                .collect();
            for stack in &stacks {
                emulator
                    .ok(&case, "CreateStack", json!({ "Name": stack }))
                    .await;
                emulator
                    .ok(
                        &case,
                        "AssociateFleet",
                        json!({ "FleetName": fleet_name, "StackName": stack }),
                    )
                    .await;
            }

            let message = refusal(
                &emulator,
                &case,
                "DeleteFleet",
                json!({ "Name": fleet_name }),
                400,
                "ResourceInUseException",
            )
            .await;

            let expected = format!(
                "The fleet {fleet_name} is associated with a stack. To delete the fleet, \
                 you must dissociate it from the stack."
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
            for stack in &stacks {
                assert!(
                    !message.contains(stack.as_str()),
                    "case {case:?}: the refusal must not name the associated stack \
                     {stack:?}, got {message:?}"
                );
            }

            // Clearing every edge makes the fleet deletable, so the refusal reported a
            // live association rather than a permanent state.
            for stack in &stacks {
                emulator
                    .ok(
                        &case,
                        "DisassociateFleet",
                        json!({ "FleetName": fleet_name, "StackName": stack }),
                    )
                    .await;
            }
            emulator
                .ok(&case, "DeleteFleet", json!({ "Name": fleet_name }))
                .await;
        }
    }
}

#[tokio::test]
async fn deleting_a_stack_with_fleets_is_refused_without_naming_the_fleets() {
    // Rows: how many fleets the stack has associated. The sentence is the same for one
    // as for many, and names none of them.
    for fleet_count in 1..=3_usize {
        for stack_name in names() {
            let case = format!("{stack_name:?} with {fleet_count} associated fleet(s)");
            let emulator = Emulator::new();
            emulator
                .ok(&case, "CreateStack", json!({ "Name": stack_name }))
                .await;

            let fleets: Vec<String> = (0..fleet_count)
                .map(|index| format!("partner-fleet-{index}"))
                .collect();
            for fleet in &fleets {
                emulator
                    .ok(
                        &case,
                        "CreateFleet",
                        json!({ "Name": fleet, "InstanceType": "stream.standard.small" }),
                    )
                    .await;
                emulator
                    .ok(
                        &case,
                        "AssociateFleet",
                        json!({ "FleetName": fleet, "StackName": stack_name }),
                    )
                    .await;
            }

            let message = refusal(
                &emulator,
                &case,
                "DeleteStack",
                json!({ "Name": stack_name }),
                400,
                "ResourceInUseException",
            )
            .await;

            let expected = format!(
                "Cannot delete stack {stack_name}. This stack has fleets associated with \
                 it. Only stacks with no fleets associated with it can be deleted."
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
            for fleet in &fleets {
                assert!(
                    !message.contains(fleet.as_str()),
                    "case {case:?}: the refusal must not name the associated fleet \
                     {fleet:?}, got {message:?}"
                );
            }

            for fleet in &fleets {
                emulator
                    .ok(
                        &case,
                        "DisassociateFleet",
                        json!({ "FleetName": fleet, "StackName": stack_name }),
                    )
                    .await;
            }
            emulator
                .ok(&case, "DeleteStack", json!({ "Name": stack_name }))
                .await;
        }
    }
}

