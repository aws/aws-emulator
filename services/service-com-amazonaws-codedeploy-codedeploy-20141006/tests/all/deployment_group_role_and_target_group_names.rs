// SPDX-License-Identifier: CC0-1.0

//! What a deployment group's create and update RESOLVE before they store the
//! group: the service role it would run as, and the target group names its
//! `loadBalancerInfo` carries.
//!
//! Two rules, driven over the wire through the same front door a client uses.
//!
//! RULE 1 — the service role. A deployment group acts as the role its
//! `serviceRoleArn` names, so the role is resolved against the account's own IAM
//! records at the call that names it: a value naming a role of THIS account that
//! IAM holds nothing under, and one naming a role it holds whose trust policy
//! does not let CodeDeploy assume it, can neither be assumed, so both are refused
//! with the modelled `InvalidRoleException` (HTTP 400) and nothing is stored. A
//! value naming ANOTHER account's role is no evidence either way — that account's
//! IAM is not this emulator's to ask — so it is served.
//!
//! RULE 2 — the target group names. A target group's existence is NOT resolved (a
//! group may name target groups the account creates afterwards), but a value
//! outside Elastic Load Balancing's name grammar can never name one: "A maximum of
//! 32 alphanumeric characters including hyphens are allowed, but the name must not
//! begin or end with a hyphen" (`CreateTargetGroupInput$Name`, Elastic Load
//! Balancing v2's model). Such a value is refused with the modelled
//! `InvalidLoadBalancerInfoException` (HTTP 400), on every compute platform and in
//! both members that carry such a name, and nothing is stored.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, given_role, trust_policy, Emulator};

/// The service principal a role has to trust for CodeDeploy to act as it.
const SERVICE_PRINCIPAL: &str = "codedeploy.amazonaws.com";

/// A role ARN of THIS account that IAM holds nothing under: well-formed, so the
/// value itself is no refusal, and resolvable, so the account's records answer for
/// it.
fn absent_role_of_this_account(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// A role ARN this emulator has no records to answer for: another account's, whose
/// IAM this emulator is not the one to ask.
const ROLE_OF_ANOTHER_ACCOUNT: &str = "arn:aws:iam::210987654321:role/CodeDeployRole";

/// Create an application on `platform`, answering its name.
async fn given_application(emulator: &Emulator, name: &str, platform: &str) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": name, "computePlatform": platform}),
        )
        .await;
    name.to_string()
}

/// A `CreateDeploymentGroup` request for `application` naming `role`, plus `extra`.
fn create_request(application: &str, group: &str, role: &str, extra: &Value) -> Value {
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": role,
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        request[member] = value.clone();
    }
    request
}

/// Whether `emulator` holds a deployment group under `group`, read through the
/// group's own `GetDeploymentGroup`.
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

/// RULE 1: the role a deployment group would run as is resolved against the
/// account's IAM records, and a role that cannot be assumed is refused with
/// `InvalidRoleException` before anything is stored.
///
/// Rows differ along what the account's records answer for the value: a role of
/// this account IAM holds nothing under, the same under a path, the same spelled
/// in another partition, a role IAM holds named under a path it holds nothing
/// under, a role it holds that trusts another service, a role it holds that trusts
/// CodeDeploy, and a role of another account (which this emulator's IAM cannot
/// answer for at all).
#[tokio::test]
async fn a_service_role_is_resolved_against_the_accounts_iam_records() {
    let emulator = Emulator::with_iam();
    let application = given_application(&emulator, "role-resolution", "Server").await;
    let trusted = given_role(&emulator, "resolved-trusted", &trust_policy(SERVICE_PRINCIPAL)).await;
    let untrusted = given_role(
        &emulator,
        "resolved-untrusted",
        &trust_policy("ec2.amazonaws.com"),
    )
    .await;

    // (case, the role the request names, the modelled error code it earns — or
    // `None` where the group is created)
    let cases: Vec<(&str, String, Option<&str>)> = vec![
        (
            "a role of this account IAM holds nothing under",
            absent_role_of_this_account("NeverCreated"),
            Some("InvalidRoleException"),
        ),
        (
            "a role of this account under a path IAM holds nothing under",
            absent_role_of_this_account("service-role/NeverCreated"),
            Some("InvalidRoleException"),
        ),
        (
            // An IAM ARN is partition-global and names the same account's role in
            // every partition, so the records answer for this one too.
            "the same absent role spelled in another partition",
            format!(
                "arn:aws-cn:iam::{}:role/NeverCreated",
                engine::emulator_account_id()
            ),
            Some("InvalidRoleException"),
        ),
        (
            // IAM holds a role under a NAME and a PATH, and both are what the ARN
            // names: the role this account holds at the root is not the role an
            // ARN names under a path it holds nothing under.
            "a role IAM holds, named under a path it holds nothing under",
            trusted.replace(":role/", ":role/service-role/"),
            Some("InvalidRoleException"),
        ),
        (
            "a role IAM holds that trusts another service",
            untrusted.clone(),
            Some("InvalidRoleException"),
        ),
        (
            "a role IAM holds that trusts CodeDeploy",
            trusted.clone(),
            None,
        ),
        (
            "another account's role, which this emulator cannot answer for",
            ROLE_OF_ANOTHER_ACCOUNT.to_string(),
            None,
        ),
    ];

    for (index, (case, role, expected_code)) in cases.iter().enumerate() {
        let group = format!("role-group-{index}");
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                create_request(&application, &group, role, &json!({})),
            )
            .await;
        match expected_code {
            Some(code) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, *code),
                    "case {case:?}: expected 400 {code}, got {status} with {body:?}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(
                        format!(
                            "AWS CodeDeploy does not have the permissions required to assume \
                             the role {role}."
                        )
                        .as_str()
                    ),
                    "case {case:?}: expected the sentence naming the role, got {body:?}"
                );
                assert!(
                    !holds_group(&emulator, &application, &group).await,
                    "case {case:?}: no group may exist after the refusal"
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
                    read["deploymentGroupInfo"]["serviceRoleArn"].as_str(),
                    Some(role.as_str()),
                    "case {case:?}: expected the stored role to read back, got {read:?}"
                );
            }
        }
    }
}

/// RULE 1, for the other operation that SETS a group's role: a replacement is
/// resolved the same way, and a group whose update is refused keeps the role it
/// had.
#[tokio::test]
async fn a_replacement_service_role_is_resolved_the_same_way() {
    let emulator = Emulator::with_iam();
    let application = given_application(&emulator, "role-replacement", "Server").await;
    let original = given_role(
        &emulator,
        "replacement-original",
        &trust_policy(SERVICE_PRINCIPAL),
    )
    .await;
    let group = "replacement-group";
    emulator
        .ok(
            "CreateDeploymentGroup",
            create_request(&application, group, &original, &json!({})),
        )
        .await;

    // Rows differ along what the records answer for the replacement: an absent
    // role of this account, and one of another account (no answer at all, so the
    // update goes ahead).
    for (case, replacement, refused) in [
        (
            "a replacement IAM holds nothing under",
            absent_role_of_this_account("ReplacementNeverCreated"),
            true,
        ),
        (
            "a replacement of another account",
            ROLE_OF_ANOTHER_ACCOUNT.to_string(),
            false,
        ),
    ] {
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "serviceRoleArn": replacement,
                }),
            )
            .await;
        let expected_role = if refused {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidRoleException"),
                "case {case:?}: expected 400 InvalidRoleException, got {status} with {body:?}"
            );
            original.clone()
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
            read["deploymentGroupInfo"]["serviceRoleArn"].as_str(),
            Some(expected_role.as_str()),
            "case {case:?}: expected the group to run as {expected_role}, got {read:?}"
        );
    }
}

/// The role every row of the target group rules runs under. These suites mount no
/// IAM beside CodeDeploy, so the account has no records to resolve it against and
/// the value itself is the only thing judged.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The clause an over-long name carried by `targetGroupInfoList` is refused with:
/// CodeDeploy's own length sentence, which names the member the value came from and
/// spells the maximum inline (recorded behaviour).
const TOO_LONG_IN_LIST: &str =
    "specified in targetGroupInfoList exceeds the maximum allowed length of 32 characters.";

/// The same clause for a name carried by `targetGroupPairInfoList`.
const TOO_LONG_IN_PAIR_LIST: &str =
    "specified in targetGroupPairInfoList exceeds the maximum allowed length of 32 characters.";

/// A name of `length` characters drawn from the character set a target group name
/// admits, for a row that is about the LENGTH alone.
fn name_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// The `loadBalancerInfo` naming `name` through `targetGroupInfoList`, the form an
/// Application or Network Load Balancer's target groups are named in.
fn named_directly(name: &str) -> Value {
    json!({"targetGroupInfoList": [{"name": name}]})
}

/// The `loadBalancerInfo` naming `name` as the second target group of the pair an
/// Amazon ECS deployment moves production traffic between, the form
/// `targetGroupPairInfoList` carries.
fn named_in_a_pair(name: &str) -> Value {
    json!({"targetGroupPairInfoList": [{
        "targetGroups": [{"name": "tg-blue"}, {"name": name}],
        "prodTrafficRoute": {"listenerArns": [
            "arn:aws:elasticloadbalancing:us-east-1:123456789012:listener/app/lb/0/1"
        ]},
    }]})
}

/// RULE 2: a target group name outside Elastic Load Balancing's grammar is refused
/// `InvalidLoadBalancerInfoException` and nothing is stored; one inside it is
/// stored and reads back, whether or not a target group of that name exists.
///
/// Rows differ along WHICH clause of the grammar the value breaks and along which
/// member carries it: the published maximum length and one character past it (and
/// far past it), each way the character set can be broken (an underscore, a dot, a
/// space), a leading and a trailing hyphen, an empty name, a name no target group
/// holds (accepted — the existence is not resolved), and the same length rule on the
/// pair an Amazon ECS deployment moves traffic between. Each refused row names the
/// clause the refusal has to state.
#[tokio::test]
async fn a_target_group_name_is_read_before_the_group_is_stored() {
    let emulator = Emulator::new();
    let application = given_application(&emulator, "tg-names", "Server").await;

    // (case, the loadBalancerInfo the request names, the CLAUSE the refusal must
    // name — `None` where the group is created). A clause is the rule the value
    // broke, as Elastic Load Balancing documents and reports it for a target group
    // name, so each refused row says which rule it is about.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a name of the published maximum, 32 characters",
            named_directly(&name_of_length(32)),
            None,
        ),
        (
            "a name one character past the published maximum",
            named_directly(&name_of_length(33)),
            Some(TOO_LONG_IN_LIST),
        ),
        (
            "a name far past the published maximum",
            named_directly(&name_of_length(200)),
            Some(TOO_LONG_IN_LIST),
        ),
        (
            "a name holding an underscore",
            named_directly("tg_blue"),
            Some("can only contain characters that are alphanumeric characters or hyphens"),
        ),
        (
            "a name holding a dot",
            named_directly("tg.blue"),
            Some("can only contain characters that are alphanumeric characters or hyphens"),
        ),
        (
            "a name holding a space",
            named_directly("tg blue"),
            Some("can only contain characters that are alphanumeric characters or hyphens"),
        ),
        (
            "a name beginning with a hyphen",
            named_directly("-tg-blue"),
            Some("cannot begin or end with a hyphen"),
        ),
        (
            "a name ending with a hyphen",
            named_directly("tg-blue-"),
            Some("cannot begin or end with a hyphen"),
        ),
        ("an empty name", named_directly(""), Some("cannot be empty")),
        (
            // The existence of the target group is NOT resolved: a well-formed
            // name no target group holds is stored as it stands.
            "a well-formed name no target group holds",
            named_directly("tg-nothing-holds"),
            None,
        ),
        (
            "an over-long name in the target group pair",
            named_in_a_pair(&name_of_length(33)),
            Some(TOO_LONG_IN_PAIR_LIST),
        ),
        (
            "a well-formed pair of names",
            named_in_a_pair("tg-green"),
            None,
        ),
    ];

    for (index, (case, load_balancer_info, clause)) in cases.iter().enumerate() {
        let group = format!("tg-group-{index}");
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                create_request(
                    &application,
                    &group,
                    SERVICE_ROLE,
                    &json!({"loadBalancerInfo": load_balancer_info}),
                ),
            )
            .await;
        if let Some(clause) = clause {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidLoadBalancerInfoException"),
                "case {case:?}: expected 400 InvalidLoadBalancerInfoException, got {status} \
                 with {body:?}"
            );
            // The sentence names the member it is about and the ONE rule the value
            // broke, not every rule a target group name has.
            assert!(
                body["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("The target group name")
                        && message.ends_with(clause)),
                "case {case:?}: expected a sentence about the target group name ending in \
                 {clause:?}, got {body:?}"
            );
            assert!(
                !holds_group(&emulator, &application, &group).await,
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

/// RULE 2 holds for an Amazon ECS group as well, where the group's style routes
/// traffic behind the load balancer it names: the name is read, and a value that
/// is none is refused for itself rather than stored.
#[tokio::test]
async fn an_ecs_groups_target_group_names_are_read_too() {
    let emulator = Emulator::new();
    let application = given_application(&emulator, "tg-names-ecs", "ECS").await;
    let style = json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"});

    for (case, load_balancer_info, refused) in [
        (
            "an ECS group naming an over-long target group",
            named_in_a_pair(&name_of_length(33)),
            true,
        ),
        (
            "an ECS group naming a well-formed pair",
            named_in_a_pair("tg-green"),
            false,
        ),
    ] {
        let group = format!("ecs-tg-{}", case.len());
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                create_request(
                    &application,
                    &group,
                    SERVICE_ROLE,
                    &json!({"deploymentStyle": style, "loadBalancerInfo": load_balancer_info}),
                ),
            )
            .await;
        if refused {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidLoadBalancerInfoException"),
                "case {case:?}: expected 400 InvalidLoadBalancerInfoException, got {status} \
                 with {body:?}"
            );
            assert!(
                !holds_group(&emulator, &application, &group).await,
                "case {case:?}: no group may exist after the refusal"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected the group to be created, got {status} with {body:?}"
        );
    }
}

/// RULE 2, for the operation that REPLACES a group's load balancer configuration:
/// an update naming a value that is no target group name is refused the same way,
/// and the group keeps the configuration it had.
#[tokio::test]
async fn an_update_reads_the_target_group_names_it_would_store() {
    let emulator = Emulator::new();
    let application = given_application(&emulator, "tg-names-update", "Server").await;
    let group = "tg-update-group";
    let stored = named_directly("tg-blue");
    emulator
        .ok(
            "CreateDeploymentGroup",
            create_request(
                &application,
                group,
                SERVICE_ROLE,
                &json!({"loadBalancerInfo": stored}),
            ),
        )
        .await;

    // Rows differ along the clause the replacement breaks, and the last one is the
    // replacement the service accepts — so the member is not simply frozen.
    for (case, replacement, refused) in [
        (
            "a replacement past the published maximum length",
            named_directly(&name_of_length(33)),
            true,
        ),
        (
            "a replacement outside the character set",
            named_directly("tg_green"),
            true,
        ),
        (
            "a replacement inside the grammar",
            named_directly("tg-green"),
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
