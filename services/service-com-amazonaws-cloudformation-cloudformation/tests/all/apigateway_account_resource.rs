// SPDX-License-Identifier: CC0-1.0

//! `AWS::ApiGateway::Account` end to end: the account-level CloudWatch Logs role a stack
//! declares is really in force in Amazon API Gateway, under that service's own read.
//!
//! RULE: a resource type whose owning service holds the record as an ACCOUNT-LEVEL SINGLETON
//! is deployed by making the one call that service publishes for it, so the setting the
//! declaration states is what that service reports after the stack completes — the changed
//! declaration of a stack UPDATE is applied to the same singleton rather than replacing a
//! resource, a declaration that no longer states the setting takes it off again, deleting the
//! resource takes off what it put there, and a declaration this emulator cannot apply in full
//! fails the resource instead of reporting a setting nothing wrote. For `AWS::ApiGateway::Account`
//! that call is `UpdateAccount` (`PATCH /account`, `patchOperations` carrying a `replace` of
//! `/cloudwatchRoleArn`) and that read is `GetAccount` (`GET /account`), whose
//! `cloudwatchRoleArn` is "the ARN of an Amazon CloudWatch role for the current Account".
//!
//! The rows of the first table differ along what the rule generalises over — the FORM of the
//! role ARN the declaration points at (a name CloudFormation generated, a name the template
//! states, a role filed under an IAM path, the second of two roles one stack creates) — since
//! the value travels through the patch rather than being interpreted, and no single recorded
//! request fixes it.
//!
//! Everything goes over the wire each service speaks through the one router
//! `stack_lifecycle.rs` builds — the CloudFormation calls over its awsQuery wire, the account
//! read over API Gateway's own `restJson1` wire — so what is asserted is what a client that
//! deploys the template and then calls API Gateway observes.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::apigateway_vpc_link_resources::{apigateway, is_api_gateway_id, member_text};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, resource_event_reason, resource_event_statuses,
    router, wait_for_the_transition, xml_blocks, xml_text,
};

/// The capabilities a template declaring an `AWS::IAM::Role` is submitted under: a stack that
/// creates an IAM identity needs `CAPABILITY_IAM` acknowledged, and one that creates it under a
/// name the template states needs `CAPABILITY_NAMED_IAM` — "if you don't specify either of
/// these capabilities, CloudFormation returns an `InsufficientCapabilities` error". Every
/// template here creates the role the account is pointed at, and some of them name it.
const IAM_CAPABILITIES: &[&str] = &["CAPABILITY_IAM", "CAPABILITY_NAMED_IAM"];

/// The acknowledgement members [`IAM_CAPABILITIES`] travels as, as the request's own list.
fn acknowledged() -> Vec<(String, String)> {
    IAM_CAPABILITIES
        .iter()
        .enumerate()
        .map(|(index, capability)| {
            member(&format!("Capabilities.member.{}", index + 1), capability)
        })
        .collect()
}

/// How many characters a minted API Gateway identifier carries: that service assigns a record
/// of the ACCOUNT ten lowercase alphanumeric characters, which is the width the `RestApi`,
/// `ApiKey` and `UsagePlan` ids carry too — and the width "Ref returns the ID for the account"
/// publishes.
const ACCOUNT_ID_CHARACTERS: usize = 10;

/// How many reads a case gives a stack to reach its terminal status in: a stack is owed that
/// status once the services that own its resources report them ready, which only a read asks
/// them about, so a client polls — which is what this is.
const POLL_ATTEMPTS: usize = 40;

/// How long a removal is given to come through, polled rather than waited out.
const REMOVAL_POLLS: usize = 40;
const REMOVAL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The `ResourceStatus` a resource whose create did not go through carries.
const CREATE_FAILED: &str = "CREATE_FAILED";

/// The service principal an account's CloudWatch role has to trust: API Gateway writes the
/// account's logs under that role, so its trust policy grants the assume to that service.
const APIGATEWAY_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The AWS-managed policy that lets API Gateway push logs under the role, which is the policy
/// the "API Gateway may push logs to CloudWatch" bootstrap attaches (AWS Managed Policy
/// Reference, filed under the `/service-role/` path).
const PUSH_TO_CLOUDWATCH_LOGS: &str =
    "arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs";

/// A trust policy granting `sts:AssumeRole` to one service principal, the form `CreateRole`'s
/// own modelled example carries.
fn trust_policy(principal: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// One `AWS::IAM::Role` declaration trusting API Gateway, with whatever else the row states.
fn role_resource(extra: Value) -> Value {
    let mut properties = json!({
        "AssumeRolePolicyDocument": trust_policy(APIGATEWAY_PRINCIPAL),
        "ManagedPolicyArns": [PUSH_TO_CLOUDWATCH_LOGS],
    });
    if let (Some(properties), Some(extra)) = (properties.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            properties.insert(key.clone(), value.clone());
        }
    }
    json!({ "Type": "AWS::IAM::Role", "Properties": properties })
}

/// One `AWS::ApiGateway::Account` declaration pointing at the ARN of the role `logical_id`.
fn account_resource(logical_id: &str) -> Value {
    json!({
        "Type": "AWS::ApiGateway::Account",
        "Properties": { "CloudWatchRoleArn": { "Fn::GetAtt": [logical_id, "Arn"] } },
    })
}

/// The outputs every template here publishes: what the account resource's `Ref` and its one
/// `Fn::GetAtt` attribute resolve to, and the ARN of the role the account is pointed at.
fn published_outputs(role_logical_id: &str) -> Value {
    json!({
        "AccountRef": { "Value": { "Ref": "Account" } },
        "AccountId": { "Value": { "Fn::GetAtt": ["Account", "Id"] } },
        "RoleArn": { "Value": { "Fn::GetAtt": [role_logical_id, "Arn"] } },
    })
}

/// A template whose account resource points at a role the stack creates, with `extra` stating
/// whatever that row varies about the role.
fn template_with_role(extra: Value) -> String {
    json!({
        "Resources": {
            "LogsRole": role_resource(extra),
            "Account": account_resource("LogsRole"),
        },
        "Outputs": published_outputs("LogsRole"),
    })
    .to_string()
}

/// One row of the first table: a stack name, and the role declaration whose ARN the account
/// must end up carrying.
struct RoleCase {
    case: &'static str,
    stack: &'static str,
    template: String,
}

fn role_cases() -> Vec<RoleCase> {
    vec![
        RoleCase {
            case: "a role whose name CloudFormation generates",
            stack: "apigwaccount-generated",
            template: template_with_role(json!({})),
        },
        RoleCase {
            case: "a role whose name the template states",
            stack: "apigwaccount-named",
            template: template_with_role(json!({ "RoleName": "apigwaccount-named-logs" })),
        },
        RoleCase {
            case: "a role filed under an IAM path, whose ARN carries it",
            stack: "apigwaccount-path",
            template: template_with_role(json!({
                "RoleName": "apigwaccount-path-logs",
                "Path": "/service-role/",
            })),
        },
        RoleCase {
            case: "the second of two roles one stack creates",
            stack: "apigwaccount-second",
            template: json!({
                "Resources": {
                    "FirstRole": role_resource(json!({})),
                    "LogsRole": role_resource(json!({})),
                    "Account": account_resource("LogsRole"),
                },
                "Outputs": published_outputs("LogsRole"),
            })
            .to_string(),
        },
    ]
}

/// `CreateStack` under the IAM acknowledgement, then poll the stack to a terminal status the
/// way a client does, answering that read's `DescribeStacks` body.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> String {
    let (status, body) = create_stack(router, stack, template, &acknowledged()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack({stack}) must answer 200, got {status} {body}"
    );
    poll_to_terminal(router, case, stack).await
}

/// `UpdateStack` with a changed template, then poll the stack to a terminal status.
async fn update(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> String {
    let mut members = vec![
        member("StackName", stack),
        member("TemplateBody", template),
    ];
    members.extend(acknowledged());
    let (status, body) = call(router, "UpdateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateStack({stack}) must answer 200, got {status} {body}"
    );
    poll_to_terminal(router, case, stack).await
}

/// Read a stack until it reports a status no longer in progress, and answer that read's body.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        wait_for_the_transition().await;
        let (status, body) = describe_stack(router, stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks({stack}) must answer 200, got {status} {body}"
        );
        described = body;
        if !xml_text(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// The `StackStatus` one read reports.
fn stack_status(described: &str) -> String {
    xml_text(described, "StackStatus").unwrap_or_default()
}

/// The value the stack reports for one declared output.
fn output_value(described: &str, key: &str) -> Option<String> {
    let outputs = xml_blocks(described, "Outputs").into_iter().next()?;
    xml_blocks(&outputs, "member").into_iter().find_map(|entry| {
        (xml_text(&entry, "OutputKey").as_deref() == Some(key))
            .then(|| xml_text(&entry, "OutputValue"))
            .flatten()
    })
}

/// The value the stack reports for one output, or a panic naming the case: every template here
/// publishes all three.
fn required_output(case: &str, described: &str, key: &str) -> String {
    output_value(described, key).unwrap_or_else(|| {
        panic!("case {case:?}: expected the stack to publish the output {key:?}, got {described}")
    })
}

/// `GetAccount` (`GET /account`): the account settings API Gateway holds for this account and
/// Region, and the role they report — `None` where the account carries none.
async fn account_role(router: &MultiServiceRouter, case: &str) -> Option<String> {
    let (status, document) = apigateway(router, "GET", "/account").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetAccount to answer 200, got {status} with {document}"
    );
    member_text(&document, "cloudwatchRoleArn")
}

/// Poll `GetAccount` until the role it reports is the expected one, driving the stack's own
/// progress between reads: a stack's terminal status is owed once the owning services report
/// its resources settled, which only a read asks them about.
async fn poll_account_role(
    router: &MultiServiceRouter,
    case: &str,
    expected: Option<&str>,
) -> Option<String> {
    let mut reported = account_role(router, case).await;
    for _ in 0..REMOVAL_POLLS {
        if reported.as_deref() == expected {
            break;
        }
        tokio::time::sleep(REMOVAL_POLL_INTERVAL).await;
        let _ = call(router, "ListStacks", &[]).await;
        reported = account_role(router, case).await;
    }
    reported
}

/// Every row: the role ARN a declared `AWS::ApiGateway::Account` points at is the one API
/// Gateway reports for the account once the stack completes, and the identifier the stack
/// publishes for the resource is of the form that service writes one in.
#[tokio::test]
async fn a_declared_account_resource_sets_the_role_api_gateway_reports() {
    for case in role_cases() {
        let name = case.case;
        // A router per row: the account settings are a SINGLETON of the account and Region,
        // so each row deploys against an account nothing has set a role on.
        let router = router();
        let described = deploy(&router, name, case.stack, &case.template).await;
        assert_eq!(
            stack_status(&described),
            "CREATE_COMPLETE",
            "case {name:?}: expected the create to complete, got {described}"
        );

        let role_arn = required_output(name, &described, "RoleArn");
        let reported = account_role(&router, name).await;
        assert_eq!(
            reported.as_deref(),
            Some(role_arn.as_str()),
            "case {name:?}: expected GetAccount to report the declared role {role_arn:?}, got \
             {reported:?}"
        );

        // The identifier the resource publishes: "Ref returns the ID for the account", which
        // is also this type's one `Fn::GetAtt` attribute, so both readers report it.
        let published = required_output(name, &described, "AccountRef");
        let attribute = required_output(name, &described, "AccountId");
        assert_eq!(
            published, attribute,
            "case {name:?}: expected Ref and the Id attribute to report one identifier, got \
             {published:?} and {attribute:?}"
        );
        assert!(
            is_api_gateway_id(&published, ACCOUNT_ID_CHARACTERS),
            "case {name:?}: expected an identifier of the form API Gateway writes one in, got \
             {published:?}"
        );
    }
}

/// A stack UPDATE that re-points the account at another role applies to the same singleton:
/// the setting in force afterwards is the one the CHANGED declaration states.
#[tokio::test]
async fn an_updated_declaration_repoints_the_account_at_the_role_it_now_names() {
    let case = "an account re-pointed at a second role";
    let router = router();
    let stack = "apigwaccount-repointed";
    let created = deploy(&router, case, stack, &template_with_role(json!({}))).await;
    assert_eq!(
        stack_status(&created),
        "CREATE_COMPLETE",
        "case {case:?}: expected the create to complete, got {created}"
    );
    let first = required_output(case, &created, "RoleArn");

    // The same stack, with a SECOND role beside the first and the account pointed at it.
    let changed = json!({
        "Resources": {
            "LogsRole": role_resource(json!({})),
            "OtherRole": role_resource(json!({})),
            "Account": account_resource("OtherRole"),
        },
        "Outputs": published_outputs("OtherRole"),
    })
    .to_string();
    let updated = update(&router, case, stack, &changed).await;
    assert_eq!(
        stack_status(&updated),
        "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {updated}"
    );
    let second = required_output(case, &updated, "RoleArn");
    assert_ne!(
        second, first,
        "case {case:?}: expected the update to name a different role, got {second:?} twice"
    );
    let reported = poll_account_role(&router, case, Some(second.as_str())).await;
    assert_eq!(
        reported.as_deref(),
        Some(second.as_str()),
        "case {case:?}: expected GetAccount to report the role the update names {second:?}, got \
         {reported:?}"
    );
}

/// A stack UPDATE whose account resource no longer states a role takes the setting off again,
/// rather than leaving in force a role the template stopped asking for.
#[tokio::test]
async fn a_declaration_that_drops_the_role_clears_the_account_setting() {
    let case = "an account declaration that drops the role";
    let router = router();
    let stack = "apigwaccount-dropped";
    let created = deploy(&router, case, stack, &template_with_role(json!({}))).await;
    assert_eq!(
        stack_status(&created),
        "CREATE_COMPLETE",
        "case {case:?}: expected the create to complete, got {created}"
    );
    let role_arn = required_output(case, &created, "RoleArn");
    let reported = account_role(&router, case).await;
    assert_eq!(
        reported.as_deref(),
        Some(role_arn.as_str()),
        "case {case:?}: expected the created stack to set {role_arn:?}, got {reported:?}"
    );

    let changed = json!({
        "Resources": {
            "LogsRole": role_resource(json!({})),
            "Account": json!({ "Type": "AWS::ApiGateway::Account", "Properties": {} }),
        },
        "Outputs": published_outputs("LogsRole"),
    })
    .to_string();
    let updated = update(&router, case, stack, &changed).await;
    assert_eq!(
        stack_status(&updated),
        "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {updated}"
    );
    let reported = poll_account_role(&router, case, None).await;
    assert_eq!(
        reported, None,
        "case {case:?}: expected GetAccount to report no role once the declaration states none, \
         got {reported:?}"
    );
}

/// Deleting the stack takes the account's role off again: the setting was the whole of the
/// resource, so a client that removed the stack finds the account as the stack found it.
#[tokio::test]
async fn deleting_the_stack_takes_the_account_role_off_again() {
    let case = "a deleted account stack";
    let router = router();
    let stack = "apigwaccount-deleted";
    let described = deploy(&router, case, stack, &template_with_role(json!({}))).await;
    assert_eq!(
        stack_status(&described),
        "CREATE_COMPLETE",
        "case {case:?}: expected the create to complete, got {described}"
    );
    let role_arn = required_output(case, &described, "RoleArn");
    let reported = account_role(&router, case).await;
    assert_eq!(
        reported.as_deref(),
        Some(role_arn.as_str()),
        "case {case:?}: expected the created stack to set {role_arn:?}, got {reported:?}"
    );

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteStack({stack}) answered {status}: {body}"
    );
    let reported = poll_account_role(&router, case, None).await;
    assert_eq!(
        reported, None,
        "case {case:?}: expected GetAccount to report no role once the stack that set it is \
         gone, got {reported:?}"
    );
}

/// Deleting a stack whose account resource was UPDATED takes off the role the LATEST
/// declaration put in force, not the superseded one: the record the delete decides from is the
/// one the update left behind.
#[tokio::test]
async fn deleting_an_updated_stack_takes_off_the_role_the_update_put_in_force() {
    let case = "a deleted account stack that had been re-pointed";
    let router = router();
    let stack = "apigwaccount-updated-deleted";
    let created = deploy(&router, case, stack, &template_with_role(json!({}))).await;
    assert_eq!(
        stack_status(&created),
        "CREATE_COMPLETE",
        "case {case:?}: expected the create to complete, got {created}"
    );
    let first = required_output(case, &created, "RoleArn");

    let changed = json!({
        "Resources": {
            "LogsRole": role_resource(json!({})),
            "OtherRole": role_resource(json!({})),
            "Account": account_resource("OtherRole"),
        },
        "Outputs": published_outputs("OtherRole"),
    })
    .to_string();
    let updated = update(&router, case, stack, &changed).await;
    assert_eq!(
        stack_status(&updated),
        "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {updated}"
    );
    let second = required_output(case, &updated, "RoleArn");
    assert_ne!(
        second, first,
        "case {case:?}: expected the update to name a different role, got {second:?} twice"
    );
    let reported = poll_account_role(&router, case, Some(second.as_str())).await;
    assert_eq!(
        reported.as_deref(),
        Some(second.as_str()),
        "case {case:?}: expected the update to put {second:?} in force, got {reported:?}"
    );

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteStack({stack}) answered {status}: {body}"
    );
    let reported = poll_account_role(&router, case, None).await;
    assert_eq!(
        reported, None,
        "case {case:?}: expected GetAccount to report no role once the re-pointed stack is \
         gone, got {reported:?}"
    );
}

/// A declaration stating a property no call here carries FAILS the resource naming that
/// property, and the account is left carrying no role: a resource reported created with less
/// than its declaration asked for would be a success that dropped the caller's configuration.
#[tokio::test]
async fn a_property_this_emulator_cannot_apply_fails_the_resource() {
    let case = "an account declaration carrying an unpublished property";
    let router = router();
    let stack = "apigwaccount-unpublished";
    let template = json!({
        "Resources": {
            "LogsRole": role_resource(json!({})),
            "Account": {
                "Type": "AWS::ApiGateway::Account",
                "Properties": {
                    "CloudWatchRoleArn": { "Fn::GetAtt": ["LogsRole", "Arn"] },
                    "NotAProperty": "x",
                },
            },
        },
    })
    .to_string();
    let described = deploy(&router, case, stack, &template).await;
    assert_eq!(
        stack_status(&described),
        "ROLLBACK_COMPLETE",
        "case {case:?}: expected the stack to roll back, got {described}"
    );

    let statuses = resource_event_statuses(&router, case, stack, "Account").await;
    assert!(
        statuses.iter().any(|status| status == CREATE_FAILED),
        "case {case:?}: expected a {CREATE_FAILED} event for the account, got {statuses:?}"
    );
    let reason = resource_event_reason(&router, stack, "Account", CREATE_FAILED).await;
    assert!(
        reason.contains("NotAProperty"),
        "case {case:?}: expected the failure to name the property nothing applies, got {reason:?}"
    );
    let reported = account_role(&router, case).await;
    assert_eq!(
        reported, None,
        "case {case:?}: expected the rolled-back stack to leave no account role, got {reported:?}"
    );
}

/// A create that was refused before it wrote anything takes nothing off on the way back out:
/// the role another stack put in force is still in force once the failed stack has rolled
/// back, because the clear undoes this resource's own write and no other.
#[tokio::test]
async fn a_rolled_back_create_leaves_a_role_another_stack_set_alone() {
    let case = "a rolled-back account stack beside one that set the role";
    let router = router();
    let holder = "apigwaccount-holder";
    let described = deploy(&router, case, holder, &template_with_role(json!({}))).await;
    assert_eq!(
        stack_status(&described),
        "CREATE_COMPLETE",
        "case {case:?}: expected the first create to complete, got {described}"
    );
    let in_force = required_output(case, &described, "RoleArn");

    // A second stack whose account resource cannot be created at all: its role is never
    // patched, so its rollback has nothing of its own to take off.
    let failing = json!({
        "Resources": {
            "LogsRole": role_resource(json!({})),
            "Account": {
                "Type": "AWS::ApiGateway::Account",
                "Properties": {
                    "CloudWatchRoleArn": { "Fn::GetAtt": ["LogsRole", "Arn"] },
                    "NotAProperty": "x",
                },
            },
        },
    })
    .to_string();
    let described = deploy(&router, case, "apigwaccount-rolledback", &failing).await;
    assert_eq!(
        stack_status(&described),
        "ROLLBACK_COMPLETE",
        "case {case:?}: expected the second stack to roll back, got {described}"
    );

    let reported = account_role(&router, case).await;
    assert_eq!(
        reported.as_deref(),
        Some(in_force.as_str()),
        "case {case:?}: expected the role the first stack set {in_force:?} to survive the \
         second stack's rollback, got {reported:?}"
    );
}
