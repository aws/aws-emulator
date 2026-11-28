// SPDX-License-Identifier: CC0-1.0

//! How MANY load balancers of each form a deployment group's `loadBalancerInfo` may
//! name, driven over the wire through the same front door a client uses.
//!
//! RULE: each member of `loadBalancerInfo` has a documented size — "You can add up
//! to 10 load balancers to the array" (`elbInfoList`), "You can add up to 10 target
//! groups to the array" (`targetGroupInfoList`) and "an array of
//! `TargeGroupPairInfo` objects with a maximum size of one"
//! (`targetGroupPairInfoList`), all from
//! `smithy_query(query="shape",shape="LoadBalancerInfo")` — so a configuration that
//! names more of one form than that is refused with the modelled
//! `InvalidLoadBalancerInfoException` (HTTP 400) and nothing is stored, while a
//! configuration AT the bound is stored and reads back. The bound is read off the
//! list's length, not off its entries: a list that is one too long is refused even
//! though every name in it is a name.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// The role every group below runs as; this suite mounts no IAM, so the account has
/// no records to resolve it against and only the load balancer configuration is
/// judged.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The documented maximum of `elbInfoList` and of `targetGroupInfoList`.
const MAX_LIST: usize = 10;

/// The documented maximum of `targetGroupPairInfoList`.
const MAX_PAIR_LIST: usize = 1;

/// `count` Classic Load Balancer names, each one a name.
fn elbs(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"name": format!("classic-elb-{index}")}))
            .collect(),
    )
}

/// `count` target group names, each one a name.
fn target_groups(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"name": format!("tg-{index}")}))
            .collect(),
    )
}

/// `count` target group PAIRS, each one well formed.
fn pairs(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "targetGroups": [
                        {"name": format!("tg-blue-{index}")},
                        {"name": format!("tg-green-{index}")},
                    ],
                    "prodTrafficRoute": {"listenerArns": [
                        "arn:aws:elasticloadbalancing:us-east-1:123456789012:listener/app/lb/0/1"
                    ]},
                })
            })
            .collect(),
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

/// Whether `emulator` holds a group under `group`, read through the group's own
/// `GetDeploymentGroup`.
async fn holds_group(emulator: &Emulator, application: &str, group: &str) -> bool {
    let (status, body) = emulator
        .call(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await;
    if status == 200 {
        return true;
    }
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DeploymentGroupDoesNotExistException"),
        "group {group:?}: a group this account does not hold must read as the modelled miss, \
         got {status} with {body:?}"
    );
    false
}

/// RULE, at the operation that CREATES a group.
///
/// Rows differ along the member that is over-named, and each member is driven at
/// its published bound, at one past it and far past it; one row names several
/// members at once, which the member's own documentation allows ("You can use load
/// balancers and target groups in combination"), and one names the first list
/// within its bound beside a second list past its own, so the refusal is not keyed
/// to one list being present.
#[tokio::test]
async fn each_load_balancer_list_is_bounded_by_its_documented_size() {
    let emulator = Emulator::new();
    let application = "lb-list-sizes";
    given_application(&emulator, application, "Server").await;

    // (case, the loadBalancerInfo the request names, the sentence the refusal must
    // state — `None` where the group is created)
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "target groups at the published bound",
            json!({"targetGroupInfoList": target_groups(MAX_LIST)}),
            None,
        ),
        (
            "one target group past the bound",
            json!({"targetGroupInfoList": target_groups(MAX_LIST + 1)}),
            Some(format!(
                "The maximum number of target groups was exceeded: {MAX_LIST} are allowed in \
                 targetGroupInfoList."
            )),
        ),
        (
            "far past the target group bound",
            json!({"targetGroupInfoList": target_groups(40)}),
            Some(format!(
                "The maximum number of target groups was exceeded: {MAX_LIST} are allowed in \
                 targetGroupInfoList."
            )),
        ),
        (
            "Classic Load Balancers at the published bound",
            json!({"elbInfoList": elbs(MAX_LIST)}),
            None,
        ),
        (
            "one Classic Load Balancer past the bound",
            json!({"elbInfoList": elbs(MAX_LIST + 1)}),
            Some(format!(
                "The maximum number of load balancers was exceeded: {MAX_LIST} are allowed in \
                 elbInfoList."
            )),
        ),
        (
            "the target group pair list at its published bound",
            json!({"targetGroupPairInfoList": pairs(MAX_PAIR_LIST)}),
            None,
        ),
        (
            "two target group pairs",
            json!({"targetGroupPairInfoList": pairs(MAX_PAIR_LIST + 1)}),
            Some(format!(
                "The maximum number of target group pairs was exceeded: {MAX_PAIR_LIST} is \
                 allowed in targetGroupPairInfoList."
            )),
        ),
        (
            // The members are usable in combination, so each within its own bound
            // is stored.
            "both lists, each within its own bound",
            json!({
                "elbInfoList": elbs(2),
                "targetGroupInfoList": target_groups(5),
            }),
            None,
        ),
        (
            // The first list is within its bound: the refusal names the list that
            // is not.
            "Classic Load Balancers within the bound beside too many target groups",
            json!({
                "elbInfoList": elbs(2),
                "targetGroupInfoList": target_groups(MAX_LIST + 1),
            }),
            Some(format!(
                "The maximum number of target groups was exceeded: {MAX_LIST} are allowed in \
                 targetGroupInfoList."
            )),
        ),
        (
            // Nothing named at all is within every bound.
            "an all-empty configuration",
            json!({}),
            None,
        ),
    ];

    for (index, (case, load_balancer_info, sentence)) in cases.iter().enumerate() {
        let group = format!("lb-size-{index}");
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
        if let Some(sentence) = sentence {
            assert_eq!(
                (status, error_code(&body).as_str(), body["message"].as_str()),
                (
                    400,
                    "InvalidLoadBalancerInfoException",
                    Some(sentence.as_str())
                ),
                "case {case:?}: expected 400 InvalidLoadBalancerInfoException with \
                 {sentence:?}, got {status} with {body:?}"
            );
            assert!(
                !holds_group(&emulator, application, &group).await,
                "case {case:?}: no group may exist after the refusal"
            );
            continue;
        }
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
            &read["deploymentGroupInfo"]["loadBalancerInfo"], load_balancer_info,
            "case {case:?}: expected the stored loadBalancerInfo to read back, got {read:?}"
        );
    }
}

/// RULE, at the other operation that WRITES a group's load balancer configuration:
/// the same bounds, and a group whose update is refused keeps the configuration it
/// had.
#[tokio::test]
async fn an_update_is_bounded_by_the_same_documented_sizes() {
    let emulator = Emulator::new();
    let application = "lb-list-sizes-update";
    given_application(&emulator, application, "Server").await;
    let group = "lb-size-update-group";
    let stored = json!({"targetGroupInfoList": target_groups(1)});
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

    // Rows differ along the list the replacement over-names, and the last row is
    // the replacement the service accepts — so the member is not simply frozen.
    for (case, replacement, refused) in [
        (
            "a replacement past the target group bound",
            json!({"targetGroupInfoList": target_groups(MAX_LIST + 1)}),
            true,
        ),
        (
            "a replacement past the Classic Load Balancer bound",
            json!({"elbInfoList": elbs(MAX_LIST + 1)}),
            true,
        ),
        (
            "a replacement with two target group pairs",
            json!({"targetGroupPairInfoList": pairs(MAX_PAIR_LIST + 1)}),
            true,
        ),
        (
            "a replacement at the target group bound",
            json!({"targetGroupInfoList": target_groups(MAX_LIST)}),
            false,
        ),
    ] {
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "loadBalancerInfo": replacement,
                }),
            )
            .await;
        let expected = if refused {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidLoadBalancerInfoException"),
                "case {case:?}: expected 400 InvalidLoadBalancerInfoException, got {status} \
                 with {body:?}"
            );
            stored.clone()
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status} with {body:?}"
            );
            replacement.clone()
        };
        let read = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_eq!(
            &read["deploymentGroupInfo"]["loadBalancerInfo"], &expected,
            "case {case:?}: expected the group to carry {expected}, got {read:?}"
        );
    }
}
