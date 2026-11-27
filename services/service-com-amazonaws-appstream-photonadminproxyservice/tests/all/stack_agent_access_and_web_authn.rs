// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the stack configuration this service's model gained: the WebAuthn
//! redirection a stack carries, and the Replay Session half of its agent access
//! configuration.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * a configuration member the model declares on `Stack` is PERSISTED by the create and
//!   reported from the stored record, so the create's own response and every later
//!   `DescribeStacks` project one stack — the WebAuthn redirection and the agent access
//!   configuration's Replay Session members included;
//! * an `UpdateStack` member replaces the stored configuration and an omitted one keeps
//!   it, for those members as for every other;
//! * an `AttributesToDelete` entry clears the configuration it names, including the one
//!   that names a single FIELD of the agent access configuration — the key its Replay
//!   Session artifacts are encrypted with — which leaves the rest of that configuration in
//!   place;
//! * turning Replay Session off removes that key as well, which is what the member
//!   documents;
//! * setting a configuration and naming it in `AttributesToDelete` in one request is
//!   refused with AWS's sentence for that attribute, and writes nothing.

use serde_json::{json, Value};

use crate::refusal_messages::refusal;
use crate::stack_fleet_lifecycle::Emulator;

/// A WebAuthn redirection configuration, as `WebAuthnRedirection` declares one: `Enabled`
/// is its required member and `ProcessCompatibilityList` is "the list of process executable
/// names for which you enable WebAuthn redirection. For example, chrome.exe"
/// (`smithy_query(query="shape", shape="WebAuthnRedirection")`).
fn web_authn_redirection(enabled: bool, processes: &[&str]) -> Value {
    json!({ "Enabled": enabled, "ProcessCompatibilityList": processes })
}

/// A complete agent access configuration with Replay Session on, as `AgentAccessConfig`
/// declares one: the settings, the screen resolution and the image format are its required
/// members, and the KMS key is what the Replay Session artifacts are encrypted with
/// (`smithy_query(query="shape", shape="AgentAccessConfig")`).
fn agent_access_config() -> Value {
    json!({
        "Settings": [{ "AgentAction": "COMPUTER_VISION", "Permission": "ENABLED" }],
        "ScreenResolution": "W_1280xH_720",
        "ScreenImageFormat": "PNG",
        "ScreenshotsUploadEnabled": true,
        "S3BucketArn": "arn:aws:s3:::agent-artifacts",
        "ReplaySessionEnabled": true,
        "KmsKeyArn": "arn:aws:kms:us-east-1:123456789012:key/0e8cf9ba-4b11-4a7f-9f5a-1e2d3c4b5a60",
        "UserControlMode": "VIEW_STOP",
    })
}

/// The stack one `DescribeStacks` reports for `name`.
async fn described(emulator: &Emulator, case: &str, name: &str) -> Value {
    let payload = emulator
        .ok(case, "DescribeStacks", json!({ "Names": [name] }))
        .await;
    payload
        .pointer("/Stacks/0")
        .unwrap_or_else(|| panic!("case {case:?}: expected the stack, got {payload}"))
        .clone()
}

/// Create a stack carrying `members` beside its name, and answer the `Stack` the call
/// reported.
async fn create(emulator: &Emulator, case: &str, name: &str, members: Value) -> Value {
    let mut request = json!({ "Name": name });
    for (member, value) in members.as_object().expect("the row is an object") {
        request[member] = value.clone();
    }
    let payload = emulator.ok(case, "CreateStack", request).await;
    payload
        .get("Stack")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Stack, got {payload}"))
        .clone()
}

/// Update `name` with `members` and answer the `Stack` the call reported.
async fn update(emulator: &Emulator, case: &str, name: &str, members: Value) -> Value {
    let mut request = json!({ "Name": name });
    for (member, value) in members.as_object().expect("the row is an object") {
        request[member] = value.clone();
    }
    let payload = emulator.ok(case, "UpdateStack", request).await;
    payload
        .get("Stack")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Stack, got {payload}"))
        .clone()
}

#[tokio::test]
async fn a_create_persists_the_configuration_members_the_stack_shape_declares() {
    // Rows: the Region the request's credential scope names, and — within each Region —
    // whether the redirection is on or off and how many processes it lists.
    for region in ["us-east-1", "eu-west-2"] {
        let emulator = Emulator::in_region(region);
        let rows = [
            ("enabled for one process", web_authn_redirection(true, &["chrome.exe"])),
            (
                "enabled for several processes",
                web_authn_redirection(true, &["chrome.exe", "msedge.exe", "firefox.exe"]),
            ),
            ("disabled", json!({ "Enabled": false })),
        ];

        for (index, (row, redirection)) in rows.into_iter().enumerate() {
            let name = format!("web-authn-{index}");
            let case = format!("{region} {row}");
            let created = create(
                &emulator,
                &case,
                &name,
                json!({
                    "WebAuthnRedirection": redirection.clone(),
                    "AgentAccessConfig": agent_access_config(),
                }),
            )
            .await;

            assert_eq!(
                created["WebAuthnRedirection"], redirection,
                "case {case:?}: the stack must report the redirection it was created \
                 with, got {created}"
            );
            assert_eq!(
                created["AgentAccessConfig"],
                agent_access_config(),
                "case {case:?}: the stack must report the agent access configuration it \
                 was created with, got {created}"
            );

            // The create answers the stored record, so a read reports the same stack.
            let read = described(&emulator, &case, &name).await;
            assert_eq!(
                read, created,
                "case {case:?}: the read must report the record the create wrote \
                 ({created}), got {read}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_replaces_the_configuration_it_carries_and_keeps_the_rest() {
    let emulator = Emulator::new();

    // Rows: which members the update carries, and what the stack reports afterwards.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a redirection replaced",
            json!({ "WebAuthnRedirection": web_authn_redirection(false, &["chrome.exe"]) }),
            json!({ "WebAuthnRedirection": web_authn_redirection(false, &["chrome.exe"]) }),
        ),
        (
            "a redirection's process list replaced",
            json!({ "WebAuthnRedirection": web_authn_redirection(true, &["msedge.exe"]) }),
            json!({ "WebAuthnRedirection": web_authn_redirection(true, &["msedge.exe"]) }),
        ),
        (
            "the redirection cleared",
            json!({ "AttributesToDelete": ["WEB_AUTHN_REDIRECTION"] }),
            Value::Null,
        ),
        (
            "Replay Session turned off, which removes the key with it",
            json!({ "AgentAccessConfig": { "ReplaySessionEnabled": false } }),
            json!({
                "AgentAccessConfig": {
                    "Settings": [{ "AgentAction": "COMPUTER_VISION", "Permission": "ENABLED" }],
                    "ScreenResolution": "W_1280xH_720",
                    "ScreenImageFormat": "PNG",
                    "ScreenshotsUploadEnabled": true,
                    "S3BucketArn": "arn:aws:s3:::agent-artifacts",
                    "ReplaySessionEnabled": false,
                    "UserControlMode": "VIEW_STOP",
                },
            }),
        ),
        (
            "the key cleared on its own",
            json!({ "AttributesToDelete": ["AGENT_ACCESS_CONFIG_KMS_KEY_ARN"] }),
            json!({
                "AgentAccessConfig": {
                    "Settings": [{ "AgentAction": "COMPUTER_VISION", "Permission": "ENABLED" }],
                    "ScreenResolution": "W_1280xH_720",
                    "ScreenImageFormat": "PNG",
                    "ScreenshotsUploadEnabled": true,
                    "S3BucketArn": "arn:aws:s3:::agent-artifacts",
                    "ReplaySessionEnabled": true,
                    "UserControlMode": "VIEW_STOP",
                },
            }),
        ),
        (
            "the key replaced beside the rest of the configuration",
            json!({
                "AgentAccessConfig": {
                    "KmsKeyArn": "arn:aws:kms:us-east-1:123456789012:key/11112222-3333-4444-5555-666677778888",
                },
            }),
            json!({
                "AgentAccessConfig": {
                    "Settings": [{ "AgentAction": "COMPUTER_VISION", "Permission": "ENABLED" }],
                    "ScreenResolution": "W_1280xH_720",
                    "ScreenImageFormat": "PNG",
                    "ScreenshotsUploadEnabled": true,
                    "S3BucketArn": "arn:aws:s3:::agent-artifacts",
                    "ReplaySessionEnabled": true,
                    "KmsKeyArn": "arn:aws:kms:us-east-1:123456789012:key/11112222-3333-4444-5555-666677778888",
                    "UserControlMode": "VIEW_STOP",
                },
            }),
        ),
    ];

    for (index, (case, members, expected)) in rows.into_iter().enumerate() {
        let name = format!("updated-{index}");
        let created = create(
            &emulator,
            case,
            &name,
            json!({
                "WebAuthnRedirection": web_authn_redirection(true, &["chrome.exe"]),
                "AgentAccessConfig": agent_access_config(),
            }),
        )
        .await;

        let updated = update(&emulator, case, &name, members).await;
        let read = described(&emulator, case, &name).await;
        assert_eq!(
            read, updated,
            "case {case:?}: the read must report the record the update wrote ({updated}), \
             got {read}"
        );

        match expected.as_object() {
            Some(members) => {
                for (member, value) in members {
                    assert_eq!(
                        updated.get(member),
                        Some(value),
                        "case {case:?}: expected {member} {value}, got {updated}"
                    );
                }
                // Every other member of the stack is left as the create stored it.
                for (member, value) in created.as_object().expect("a stack object") {
                    if members.contains_key(member) {
                        continue;
                    }
                    assert_eq!(
                        updated.get(member),
                        Some(value),
                        "case {case:?}: {member} must be left as it was ({value}), got \
                         {updated}"
                    );
                }
            }
            None => {
                assert!(
                    updated.get("WebAuthnRedirection").is_none(),
                    "case {case:?}: the cleared configuration must be absent, got {updated}"
                );
                assert_eq!(
                    updated["AgentAccessConfig"],
                    agent_access_config(),
                    "case {case:?}: clearing one attribute must leave the others, got \
                     {updated}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_update_that_both_sets_and_deletes_one_attribute_is_refused() {
    let emulator = Emulator::new();

    // Rows: the attribute named twice, and the member that sets it. The last row names the
    // attribute that clears ONE field of the agent access configuration beside an update
    // that does NOT name that field, which asks nothing of it and so is admitted.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "the WebAuthn redirection",
            json!({
                "WebAuthnRedirection": web_authn_redirection(true, &["chrome.exe"]),
                "AttributesToDelete": ["WEB_AUTHN_REDIRECTION"],
            }),
            Some("You can’t update and delete web authn redirection at the same time."),
        ),
        (
            "the whole agent access configuration",
            json!({
                "AgentAccessConfig": agent_access_config(),
                "AttributesToDelete": ["AGENT_ACCESS_CONFIG"],
            }),
            Some("You can’t update and delete agent access config at the same time."),
        ),
        (
            "the key the Replay Session artifacts are encrypted with",
            json!({
                "AgentAccessConfig": {
                    "KmsKeyArn": "arn:aws:kms:us-east-1:123456789012:key/aaaabbbb-cccc-dddd-eeee-ffff00001111",
                },
                "AttributesToDelete": ["AGENT_ACCESS_CONFIG_KMS_KEY_ARN"],
            }),
            Some(
                "You can’t update and delete agent access config kms key arn at the same \
                 time.",
            ),
        ),
        (
            "the key cleared beside an update that asks nothing of it",
            json!({
                "AgentAccessConfig": { "ScreenImageFormat": "JPEG" },
                "AttributesToDelete": ["AGENT_ACCESS_CONFIG_KMS_KEY_ARN"],
            }),
            None,
        ),
    ];

    for (index, (case, members, expected)) in rows.into_iter().enumerate() {
        let name = format!("conflict-{index}");
        let created = create(
            &emulator,
            case,
            &name,
            json!({
                "WebAuthnRedirection": web_authn_redirection(true, &["chrome.exe"]),
                "AgentAccessConfig": agent_access_config(),
            }),
        )
        .await;
        let mut request = json!({ "Name": name.clone() });
        for (member, value) in members.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }

        match expected {
            Some(sentence) => {
                let message = refusal(
                    &emulator,
                    case,
                    "UpdateStack",
                    request,
                    400,
                    "InvalidParameterCombinationException",
                )
                .await;
                assert_eq!(
                    message, sentence,
                    "case {case:?}: expected message {sentence:?}, got {message:?}"
                );

                // The refused update wrote nothing.
                let read = described(&emulator, case, &name).await;
                assert_eq!(
                    read, created,
                    "case {case:?}: the refused update must leave the stack untouched \
                     ({created}), got {read}"
                );
            }
            None => {
                let updated = emulator.ok(case, "UpdateStack", request).await;
                let config = &updated["Stack"]["AgentAccessConfig"];
                assert_eq!(
                    config["ScreenImageFormat"],
                    json!("JPEG"),
                    "case {case:?}: the supplied member must be stored, got {config}"
                );
                assert!(
                    config.get("KmsKeyArn").is_none(),
                    "case {case:?}: the attribute named for deletion must be cleared, got \
                     {config}"
                );
            }
        }
    }
}
