// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE a deployment group's over-long target group name is refused with,
//! driven over the wire through the same front door a client uses.
//!
//! RULE: a target group name a `loadBalancerInfo` carries that is longer than the
//! maximum a target group name can be is refused with
//! `InvalidLoadBalancerInfoException` (HTTP 400) and the sentence "The target group
//! name <name> specified in <member> exceeds the maximum allowed length of 32
//! characters." — fixed text that spells the maximum inline and names the
//! `loadBalancerInfo` member the value was carried in, with the rejected value
//! copied verbatim from the request and nothing else variable. One at or under the
//! maximum is stored and reads back, whether or not a target group of that name
//! exists.
//!
//! The maximum is the one the service that owns target groups publishes: "A maximum
//! of 32 alphanumeric characters including hyphens are allowed"
//! (`CreateTargetGroupInput$Name`, Elastic Load Balancing v2's model).

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// The role every group below runs as. These cases mount no IAM beside CodeDeploy,
/// so the account has no records to resolve it against and only the load balancer
/// configuration is judged.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The published maximum a target group name may be, which the sentence spells
/// inline (`CreateTargetGroupInput$Name`).
const MAXIMUM: usize = 32;

/// The sentence a name carried by `member` earns when it is longer than [`MAXIMUM`].
fn too_long(member: &str, name: &str) -> String {
    format!(
        "The target group name {name} specified in {member} exceeds the maximum allowed \
         length of {MAXIMUM} characters."
    )
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

/// The `loadBalancerInfo` naming `name` through `targetGroupInfoList`.
fn named_directly(name: &str) -> Value {
    json!({"targetGroupInfoList": [{"name": name}]})
}

/// The `loadBalancerInfo` naming `name` as the second target group of the pair
/// `targetGroupPairInfoList` carries.
fn named_in_a_pair(name: &str) -> Value {
    json!({"targetGroupPairInfoList": [{
        "targetGroups": [{"name": "tg-blue"}, {"name": name}],
        "prodTrafficRoute": {"listenerArns": [
            "arn:aws:elasticloadbalancing:us-east-1:123456789012:listener/app/lb/0/1"
        ]},
    }]})
}

/// RULE, at the operation that CREATES a group: the refused sentence is AWS's
/// template with the request's own value and the member that carried it, and a name
/// within the maximum round-trips.
///
/// Rows differ along the length (the maximum, one past it, far past it), along the
/// character the name is drawn from (letters, digits, hyphens, and a value that also
/// breaks the character set — the length clause is the one reported, the order AWS
/// reports them in), and along the member that carries it.
#[tokio::test]
async fn an_over_long_target_group_name_is_refused_with_the_length_sentence() {
    let emulator = Emulator::new();
    let application = "tg-length-sentence";
    given_application(&emulator, application, "Server").await;

    // (case, the name, the loadBalancerInfo member it is carried in — `None` where
    // the group is created)
    let cases: Vec<(&str, String, Option<&str>)> = vec![
        ("the published maximum, 32 letters", "a".repeat(32), None),
        (
            "the published maximum spelled with hyphens",
            format!("{}-{}", "b".repeat(16), "c".repeat(15)),
            None,
        ),
        (
            "one character past the maximum",
            "a".repeat(33),
            Some("targetGroupInfoList"),
        ),
        (
            "far past the maximum",
            "z".repeat(200),
            Some("targetGroupInfoList"),
        ),
        (
            "past the maximum, spelled with digits and hyphens",
            format!("tg-{}", "9".repeat(40)),
            Some("targetGroupInfoList"),
        ),
        (
            // Over-long AND outside the character set: AWS reports the length
            // clause first, so that is the sentence the caller gets.
            "past the maximum and outside the character set",
            format!("tg_{}", "d".repeat(40)),
            Some("targetGroupInfoList"),
        ),
    ];

    for (index, (case, name, member)) in cases.iter().enumerate() {
        let group = format!("tg-length-{index}");
        let load_balancer_info = named_directly(name);
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "serviceRoleArn": SERVICE_ROLE,
                    "loadBalancerInfo": load_balancer_info,
                }),
            )
            .await;
        match member {
            Some(member) => {
                let expected = too_long(member, name);
                assert_eq!(
                    (status, error_code(&body).as_str(), body["message"].as_str()),
                    (400, "InvalidLoadBalancerInfoException", Some(expected.as_str())),
                    "case {case:?}: expected 400 InvalidLoadBalancerInfoException with \
                     {expected:?}, got {status} with {body:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the group to be created, got {status} with {body:?}"
                );
                let read = emulator
                    .ok(
                        "GetDeploymentGroup",
                        json!({"applicationName": application, "deploymentGroupName": group}),
                    )
                    .await;
                assert_eq!(
                    &read["deploymentGroupInfo"]["loadBalancerInfo"], &load_balancer_info,
                    "case {case:?}: expected the stored loadBalancerInfo to read back, \
                     got {read:?}"
                );
            }
        }
    }
}

/// RULE, for the OTHER member a name is carried in: the sentence names
/// `targetGroupPairInfoList` when that is where the value came from, so a caller of
/// a multi-member configuration can tell which list to fix.
#[tokio::test]
async fn the_sentence_names_the_member_the_value_came_from() {
    let emulator = Emulator::new();
    let application = "tg-length-member";
    given_application(&emulator, application, "Server").await;

    for (case, name, load_balancer_info, member) in [
        (
            "an over-long name in targetGroupInfoList",
            "e".repeat(33),
            named_directly as fn(&str) -> Value,
            "targetGroupInfoList",
        ),
        (
            "an over-long name in the target group pair",
            "f".repeat(64),
            named_in_a_pair as fn(&str) -> Value,
            "targetGroupPairInfoList",
        ),
    ] {
        let group = format!("tg-member-{member}");
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "serviceRoleArn": SERVICE_ROLE,
                    "loadBalancerInfo": load_balancer_info(&name),
                }),
            )
            .await;
        let expected = too_long(member, &name);
        assert_eq!(
            (status, error_code(&body).as_str(), body["message"].as_str()),
            (
                400,
                "InvalidLoadBalancerInfoException",
                Some(expected.as_str())
            ),
            "case {case:?}: expected 400 InvalidLoadBalancerInfoException with {expected:?}, \
             got {status} with {body:?}"
        );
    }
}

/// RULE, at the other operation that WRITES a group's load balancer configuration:
/// the same sentence, and the group keeps the configuration it had.
#[tokio::test]
async fn an_update_states_the_same_length_sentence() {
    let emulator = Emulator::new();
    let application = "tg-length-update";
    given_application(&emulator, application, "Server").await;
    let group = "tg-length-update-group";
    let stored = named_directly("tg-blue");
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "serviceRoleArn": SERVICE_ROLE,
                "loadBalancerInfo": stored,
            }),
        )
        .await;

    for (case, name) in [
        ("a replacement one past the maximum", "g".repeat(33)),
        ("a replacement far past the maximum", "h".repeat(100)),
    ] {
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "loadBalancerInfo": named_directly(&name),
                }),
            )
            .await;
        let expected = too_long("targetGroupInfoList", &name);
        assert_eq!(
            (status, error_code(&body).as_str(), body["message"].as_str()),
            (
                400,
                "InvalidLoadBalancerInfoException",
                Some(expected.as_str())
            ),
            "case {case:?}: expected 400 InvalidLoadBalancerInfoException with {expected:?}, \
             got {status} with {body:?}"
        );
        let read = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_eq!(
            &read["deploymentGroupInfo"]["loadBalancerInfo"], &stored,
            "case {case:?}: expected the group to keep the configuration it had, got {read:?}"
        );
    }
}
