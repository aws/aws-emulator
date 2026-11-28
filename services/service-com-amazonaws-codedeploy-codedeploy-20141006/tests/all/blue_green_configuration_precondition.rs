// SPDX-License-Identifier: CC0-1.0

//! What a deployment group's `blueGreenDeploymentConfiguration` is judged against —
//! which is NOTHING — driven over the wire through the same front door a client
//! uses.
//!
//! RULE: the blue/green configuration and the deployment style of a deployment
//! group carry no cross-precondition. Every option the configuration holds reads as
//! an option of a blue/green deployment (when the replacement environment starts
//! taking traffic, how it is provisioned, what becomes of the original one), and the
//! deployment type is what says whether the group runs such a deployment — but a
//! call that states a type other than BLUE_GREEN while carrying a blue/green
//! configuration is SERVED (recorded behaviour): the configuration is stored as
//! given and read back as given, and the modelled
//! `InvalidBlueGreenDeploymentConfigurationException` is not what such a group
//! earns. The preconditions a group IS judged against — the style its compute
//! platform can run, and the load balancer its style requires — are asserted in
//! `deployment_style_precondition.rs` and
//! `deployment_group_role_and_target_group_names.rs`.
//!
//! These cases exist so a later change cannot reintroduce a refusal CodeDeploy does
//! not give: the configuration is inert on a group that does not run blue/green
//! deployments, not invalid.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::Emulator;

/// The role every group below runs as; this suite mounts no IAM, so the account has
/// no records to resolve it against and only the group's own configuration is
/// judged.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// A blue/green configuration naming what becomes of the ORIGINAL environment.
fn terminating_config() -> Value {
    json!({
        "terminateBlueInstancesOnDeploymentSuccess": {
            "action": "TERMINATE",
            "terminationWaitTimeInMinutes": 60,
        }
    })
}

/// A blue/green configuration naming the other two options instead: when the
/// replacement environment starts taking traffic, and how it is provisioned.
fn provisioning_config() -> Value {
    json!({
        "deploymentReadyOption": {
            "actionOnTimeout": "CONTINUE_DEPLOYMENT",
            "waitTimeInMinutes": 0,
        },
        "greenFleetProvisioningOption": {"action": "DISCOVER_EXISTING"},
    })
}

/// Create an application on `platform`.
async fn given_application(emulator: &Emulator, name: &str, platform: &str) {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": name, "computePlatform": platform}),
        )
        .await;
}

/// Read `group` back, answering its `deploymentGroupInfo`.
async fn stored_group(emulator: &Emulator, application: &str, group: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await["deploymentGroupInfo"]
        .clone()
}

/// RULE, at the operation that CREATES a group: whatever deployment style the call
/// states, a blue/green configuration is stored and reads back.
///
/// Rows differ along the style the call states (both values of `DeploymentType`
/// against both values of `DeploymentOption`, the half-stated forms and no style at
/// all, where the server-side default decides) and along the configuration carried
/// (each group of options, and an all-empty one).
#[tokio::test]
async fn a_blue_green_configuration_is_stored_whatever_style_the_call_states() {
    let emulator = Emulator::new();
    let application = "blue-green-inert";
    given_application(&emulator, application, "Server").await;

    // (case, the deploymentStyle the call states — null for none, the
    // blueGreenDeploymentConfiguration it carries, the deploymentType the stored
    // group runs by)
    let cases: Vec<(&str, Value, Value, &str)> = vec![
        (
            "blue/green with traffic control",
            json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"}),
            terminating_config(),
            "BLUE_GREEN",
        ),
        (
            // The one pair CodeDeploy does not support is blue/green WITHOUT
            // traffic control (`deployment_style_precondition`), so the blue/green
            // rows here state the supported pair.
            "blue/green with traffic control, carrying the provisioning options",
            json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"}),
            provisioning_config(),
            "BLUE_GREEN",
        ),
        (
            "in-place without traffic control",
            json!({"deploymentType": "IN_PLACE", "deploymentOption": "WITHOUT_TRAFFIC_CONTROL"}),
            terminating_config(),
            "IN_PLACE",
        ),
        (
            "in-place with traffic control",
            json!({"deploymentType": "IN_PLACE", "deploymentOption": "WITH_TRAFFIC_CONTROL"}),
            provisioning_config(),
            "IN_PLACE",
        ),
        (
            "a style stating only the in-place type",
            json!({"deploymentType": "IN_PLACE"}),
            terminating_config(),
            "IN_PLACE",
        ),
        (
            "a style stating only traffic control",
            json!({"deploymentOption": "WITH_TRAFFIC_CONTROL"}),
            terminating_config(),
            // The type nothing stated is the server-side default's.
            "IN_PLACE",
        ),
        (
            "no style at all",
            Value::Null,
            terminating_config(),
            "IN_PLACE",
        ),
        (
            // The member is stored however much of it is filled in.
            "an all-empty configuration beside an in-place style",
            json!({"deploymentType": "IN_PLACE", "deploymentOption": "WITHOUT_TRAFFIC_CONTROL"}),
            json!({}),
            "IN_PLACE",
        ),
    ];

    for (index, (case, style, config, expected_type)) in cases.iter().enumerate() {
        let group = format!("bg-inert-{index}");
        let mut request = json!({
            "applicationName": application,
            "deploymentGroupName": group,
            "serviceRoleArn": SERVICE_ROLE,
            "blueGreenDeploymentConfiguration": config,
        });
        if !style.is_null() {
            request["deploymentStyle"] = style.clone();
        }
        let (status, body) = emulator.call("CreateDeploymentGroup", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the group to be created, got {status} with {body:?}"
        );
        let stored = stored_group(&emulator, application, &group).await;
        assert_eq!(
            stored["deploymentStyle"]["deploymentType"].as_str(),
            Some(*expected_type),
            "case {case:?}: expected the group to run by {expected_type}, got {stored:?}"
        );
        for (member, value) in config.as_object().into_iter().flatten() {
            assert_eq!(
                &stored["blueGreenDeploymentConfiguration"][member], value,
                "case {case:?}: expected the stored configuration to carry {member} as {value}, \
                 got {stored:?}"
            );
        }
    }
}

/// RULE, at the other operation that WRITES a group: an update that moves a
/// blue/green group to an in-place style keeps the configuration the group carries
/// rather than being refused for it, and one that replaces the configuration of an
/// in-place group is served too.
#[tokio::test]
async fn an_update_may_move_the_style_while_the_configuration_stays() {
    let emulator = Emulator::new();
    let application = "blue-green-inert-update";
    given_application(&emulator, application, "Server").await;
    let group = "blue-green-inert-update-group";
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "serviceRoleArn": SERVICE_ROLE,
                "deploymentStyle": {
                    "deploymentType": "BLUE_GREEN",
                    "deploymentOption": "WITH_TRAFFIC_CONTROL",
                },
                "blueGreenDeploymentConfiguration": terminating_config(),
            }),
        )
        .await;

    // (case, the members the update names, the deploymentType the group runs by
    // afterwards)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "the style alone moved to in-place, the configuration left where it was",
            json!({
                "deploymentStyle": {
                    "deploymentType": "IN_PLACE",
                    "deploymentOption": "WITHOUT_TRAFFIC_CONTROL",
                }
            }),
            "IN_PLACE",
        ),
        (
            "a replacement configuration on the now in-place group",
            json!({"blueGreenDeploymentConfiguration": provisioning_config()}),
            "IN_PLACE",
        ),
        (
            "both members moved back together",
            json!({
                "deploymentStyle": {
                    "deploymentType": "BLUE_GREEN",
                    "deploymentOption": "WITH_TRAFFIC_CONTROL",
                },
                "blueGreenDeploymentConfiguration": terminating_config(),
            }),
            "BLUE_GREEN",
        ),
    ];

    for (case, members, expected_type) in cases {
        let mut request = json!({
            "applicationName": application,
            "currentDeploymentGroupName": group,
        });
        for (member, value) in members.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        let (status, body) = emulator.call("UpdateDeploymentGroup", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be served, got {status} with {body:?}"
        );
        let stored = stored_group(&emulator, application, group).await;
        assert_eq!(
            stored["deploymentStyle"]["deploymentType"].as_str(),
            Some(expected_type),
            "case {case:?}: expected the group to run by {expected_type}, got {stored:?}"
        );
        assert!(
            stored["blueGreenDeploymentConfiguration"].is_object(),
            "case {case:?}: expected the group to keep a blue/green configuration, got {stored:?}"
        );
    }
}
