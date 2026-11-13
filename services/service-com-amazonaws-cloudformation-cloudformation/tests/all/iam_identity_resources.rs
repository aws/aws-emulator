// SPDX-License-Identifier: CC0-1.0

//! The IAM IDENTITY resources a stack declares — an `AWS::IAM::User`, that user's
//! `AWS::IAM::AccessKey` and an `AWS::IAM::ManagedPolicy` — are real records of IAM, and the
//! identity the stack publishes for each is the handle IAM itself takes for that kind of
//! resource.
//!
//! The rule under test: **a stack that reports a resource as created has called the owning
//! service's own API for it, and what it publishes for that resource is the identifier a
//! client calls that service with** — a user's own name, a managed policy's ARN, which
//! is what every IAM operation on a managed policy is addressed by (`GetPolicy$PolicyArn`,
//! `AttachRolePolicy$PolicyArn`), and the access key id and secret IAM minted, which is what
//! a client signs requests with. The same rule run backwards is the other half: a
//! declaration the owning service REFUSES is no resource, so the stack fails instead of
//! completing, and a stack's deletion removes what its deployment created.
//!
//! The rows differ along what the rule generalises over: the resource type, a resource the
//! template NAMES and one whose physical id CloudFormation generated, the properties each
//! type reaches IAM with (a path, an inline policy, an attached managed policy, a console
//! password, a group membership, tags, a key's status and serial), the lifecycle stage
//! (create, in-place update, replacement, delete) and both verdicts IAM can give. Every
//! expected value is computed here from what the row declares, never read back out of the
//! answer being checked.
//!
//! Everything goes over the wire each service speaks — awsQuery for CloudFormation and for
//! IAM — through the router `stack_lifecycle.rs` builds, which mounts IAM beside
//! CloudFormation on one engine, exactly as the production router does.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, iam, member, resource_event_reason,
    router, wait_for_the_transition, xml_text, xml_texts, ACCOUNT_ID,
};

/// The policy document the `tag` member of an IAM answer carries, percent-decoded back to
/// plain JSON text.
///
/// IAM reports a policy document "URL-encoded compliant with RFC 3986" and the caller "can use
/// a URL decoding method to convert the policy back to plain JSON text" (`GetRole`,
/// `GetRolePolicy`, `GetPolicyVersion`, …), so a suite that compares a document IAM reports
/// with the one a template declared reads it through this one decode — which is also what
/// `tests/all/sam_policies_shorthand.rs` reads an embedded policy through.
pub(crate) fn xml_policy_document(xml: &str, tag: &str) -> Option<String> {
    xml_text(xml, tag).map(|encoded| ::smithy_server_runtime::percent_decode(&encoded))
}

/// Why the resource `logical_id` failed to create, as the stack's own event for it says —
/// which is where the owning service's refusal is reported, the stack-level reason carrying
/// only which resources failed.
async fn create_failure(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    resource_event_reason(
        router,
        stack,
        logical_id,
        service_com_amazonaws_cloudformation_cloudformation::state::resource_status::CREATE_FAILED,
    )
    .await
}

/// A trust policy a role declaration carries inline, as a template object.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "ec2.amazonaws.com"},
            "Action": "sts:AssumeRole"
        }]
    })
}

/// A policy document granting one action, as a template object.
fn policy_document(action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": action, "Resource": "*"}]
    })
}

/// Deploy `template` as stack `name`, wait past the instant its create settles, and answer
/// the `StackStatus` and `StackStatusReason` the stack then reports.
async fn deploy(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
) -> (Option<String>, Option<String>) {
    let (status, body) = create_stack(router, name, template, &acknowledging_iam(&[])).await;
    assert_eq!(status, 200, "CreateStack({name}) answered {status}: {body}");
    settled_status(router, name).await
}

/// Update stack `name` to `template` and answer the status it settles on.
async fn redeploy(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
) -> (Option<String>, Option<String>) {
    let mut members = vec![
        member("StackName", name),
        member("TemplateBody", template),
    ];
    members.extend(acknowledging_iam(&[]));
    let (status, body) = call(router, "UpdateStack", &members).await;
    assert_eq!(status, 200, "UpdateStack({name}) answered {status}: {body}");
    settled_status(router, name).await
}

/// The status a stack reports once the operation in flight on it has settled.
async fn settled_status(
    router: &MultiServiceRouter,
    name: &str,
) -> (Option<String>, Option<String>) {
    wait_for_the_transition().await;
    let (status, body) = describe_stack(router, name).await;
    assert_eq!(status, 200, "DescribeStacks({name}) answered {status}: {body}");
    (
        xml_text(&body, "StackStatus"),
        xml_text(&body, "StackStatusReason"),
    )
}

/// The physical resource id the stack reports for one logical id.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResources({stack}, {logical_id}) answered {status}: {body}"
    );
    xml_text(&body, "PhysicalResourceId")
        .unwrap_or_else(|| panic!("expected a physical resource id for {logical_id}, got {body}"))
}

/// The `(key, value)` outputs the stack reports, in the order it reports them.
async fn outputs(router: &MultiServiceRouter, stack: &str) -> Vec<(String, String)> {
    let (status, body) = describe_stack(router, stack).await;
    assert_eq!(status, 200, "DescribeStacks({stack}) answered {status}: {body}");
    xml_texts(&body, "OutputKey")
        .into_iter()
        .zip(xml_texts(&body, "OutputValue"))
        .collect()
}

/// Delete stack `stack`, waiting past the instant its deletion settles.
///
/// What the deletion DID is asserted on the owning service's side — IAM no longer holds
/// what the stack created — rather than on the stack record, which a completed deletion
/// takes away.
async fn delete_stack(router: &MultiServiceRouter, stack: &str) {
    let (status, body) = call(router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(status, 200, "DeleteStack({stack}) answered {status}: {body}");
    wait_for_the_transition().await;
}

/// One `AWS::IAM::ManagedPolicy` row: how the policy is declared, and what the stack must
/// then publish for it.
struct PolicyRow {
    case: &'static str,
    stack: &'static str,
    /// The `ManagedPolicyName` the template states, or `None` for a generated name.
    declared_name: Option<&'static str>,
    /// The `Path` the template states, or `None` for IAM's default.
    path: Option<&'static str>,
    /// The action the declared document grants, which the read must report.
    action: &'static str,
    /// Whether the declaration attaches the policy to the role the template also declares.
    attached_to_a_role: bool,
}

/// The template one [`PolicyRow`] deploys: the policy, the role it may attach itself to,
/// and the `Ref` the stack publishes for the policy.
fn policy_template(row: &PolicyRow) -> String {
    let mut properties = json!({"PolicyDocument": policy_document(row.action)});
    if let Some(name) = row.declared_name {
        properties["ManagedPolicyName"] = json!(name);
    }
    if let Some(path) = row.path {
        properties["Path"] = json!(path);
    }
    let mut resources = json!({"Policy": {
        "Type": "AWS::IAM::ManagedPolicy",
        "Properties": properties,
    }});
    if row.attached_to_a_role {
        resources["Policy"]["Properties"]["Roles"] = json!([{"Ref": "Holder"}]);
        resources["Holder"] = json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": format!("{}-holder", row.stack),
                "AssumeRolePolicyDocument": trust_policy(),
            },
        });
    }
    json!({
        "Resources": resources,
        "Outputs": {"PolicyRef": {"Value": {"Ref": "Policy"}}},
    })
    .to_string()
}

/// RULE: a managed policy a stack declares EXISTS in IAM, and the identity the stack
/// publishes for it — its physical resource id, which is what `Ref` resolves to — is the
/// policy's ARN, so `GetPolicy` serves it and an attachment made with it holds.
#[tokio::test]
async fn a_declared_managed_policy_is_reported_by_the_arn_iam_minted() {
    let rows = &[
        PolicyRow {
            case: "a policy the template names, at the default path",
            stack: "cfnpolicy-named",
            declared_name: Some("cfn-named-policy"),
            path: None,
            action: "s3:GetObject",
            attached_to_a_role: false,
        },
        PolicyRow {
            case: "a policy the template names, at a path of its own",
            stack: "cfnpolicy-pathed",
            declared_name: Some("cfn-pathed-policy"),
            path: Some("/department/audit/"),
            action: "logs:PutLogEvents",
            attached_to_a_role: false,
        },
        PolicyRow {
            case: "a policy CloudFormation names, attached to the stack's own role",
            stack: "cfnpolicy-generated",
            declared_name: None,
            path: None,
            action: "sqs:SendMessage",
            attached_to_a_role: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let (status, reason) = deploy(&router, row.stack, &policy_template(row)).await;
        assert_eq!(
            status.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {status:?} ({reason:?}): {}",
            create_failure(&router, row.stack, "Policy").await
        );

        // The physical id is the ARN IAM built from the account, the path and the name —
        // computed here from what the row declared.
        let reported = physical_id(&router, row.stack, "Policy").await;
        let path = row.path.unwrap_or("/");
        let prefix = format!("arn:aws:iam::{ACCOUNT_ID}:policy{path}");
        assert!(
            reported.starts_with(&prefix),
            "case {case:?}: expected the policy ARN to open with {prefix:?}, got {reported:?}"
        );
        let name = reported.rsplit('/').next().unwrap_or_default().to_string();
        if let Some(declared) = row.declared_name {
            assert_eq!(
                name, declared,
                "case {case:?}: expected the declared name in the ARN, got {reported:?}"
            );
        }
        assert!(
            !name.is_empty(),
            "case {case:?}: expected a policy name in the ARN, got {reported:?}"
        );

        // `Ref` resolves to that very ARN, which is what a template passing the reference
        // to another resource's `ManagedPolicyArns` would hand IAM.
        assert_eq!(
            outputs(&router, row.stack).await,
            vec![("PolicyRef".to_string(), reported.clone())],
            "case {case:?}: expected Ref to resolve to the policy's ARN"
        );

        // IAM serves the policy under that ARN, with the name and path the row declared.
        let (code, body) = iam(&router, "GetPolicy", &[("PolicyArn", &reported)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected GetPolicy({reported:?}) to answer, got {code}: {body}"
        );
        assert_eq!(
            xml_text(&body, "PolicyName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected IAM to report the created policy, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Path").as_deref(),
            Some(path),
            "case {case:?}: expected the declared path, got {body}"
        );

        // The document the template declared as an inline object reached IAM as the
        // policy's default version.
        let version = xml_text(&body, "DefaultVersionId").unwrap_or_default();
        let (code, body) = iam(
            &router,
            "GetPolicyVersion",
            &[("PolicyArn", &reported), ("VersionId", &version)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected GetPolicyVersion to answer, got {code}: {body}"
        );
        let document = xml_policy_document(&body, "Document").unwrap_or_default();
        assert!(
            document.contains(row.action),
            "case {case:?}: expected the declared document on the policy, got {document:?}"
        );

        if row.attached_to_a_role {
            let role = physical_id(&router, row.stack, "Holder").await;
            let (code, body) =
                iam(&router, "ListAttachedRolePolicies", &[("RoleName", &role)]).await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected ListAttachedRolePolicies to answer, got {code}: {body}"
            );
            assert!(
                xml_texts(&body, "PolicyArn").contains(&reported),
                "case {case:?}: expected the stack's policy attached to {role:?}, got {body}"
            );
        }
    }
}

/// RULE (the delete half): deleting the stack deletes the managed policy it created, with
/// the attachments its declaration asked for — so IAM holds neither afterwards.
#[tokio::test]
async fn deleting_the_stack_cascades_the_managed_policy_and_its_attachment() {
    let row = PolicyRow {
        case: "a policy attached to the stack's own role",
        stack: "cfnpolicy-cascade",
        declared_name: Some("cfn-cascade-policy"),
        path: None,
        action: "s3:ListBucket",
        attached_to_a_role: true,
    };
    let router = router();
    let (status, reason) = deploy(&router, row.stack, &policy_template(&row)).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let policy_arn = physical_id(&router, row.stack, "Policy").await;
    let role = physical_id(&router, row.stack, "Holder").await;

    delete_stack(&router, row.stack).await;

    let (code, body) = iam(&router, "GetPolicy", &[("PolicyArn", &policy_arn)]).await;
    assert_eq!(
        code, 404,
        "expected the policy to be gone from IAM, got {code}: {body}"
    );
    let (code, body) = iam(&router, "GetRole", &[("RoleName", &role)]).await;
    assert_eq!(
        code, 404,
        "expected the role the stack created to be gone too, got {code}: {body}"
    );
}

/// RULE (the in-place update half): a stack update that changes a managed policy's
/// document reaches the policy the stack already has — same ARN, a new DEFAULT version
/// carrying the document the template now declares.
#[tokio::test]
async fn a_changed_managed_policy_document_becomes_the_policys_new_default_version() {
    let row = PolicyRow {
        case: "a named policy whose document the update changes",
        stack: "cfnpolicy-updated",
        declared_name: Some("cfn-updated-policy"),
        path: None,
        action: "s3:GetObject",
        attached_to_a_role: false,
    };
    let router = router();
    let (status, reason) = deploy(&router, row.stack, &policy_template(&row)).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let before = physical_id(&router, row.stack, "Policy").await;

    let changed = PolicyRow {
        action: "dynamodb:PutItem",
        ..row
    };
    let (status, reason) = redeploy(&router, row.stack, &policy_template(&changed)).await;
    assert_eq!(
        status.as_deref(),
        Some("UPDATE_COMPLETE"),
        "expected the update to complete, got {status:?} ({reason:?})"
    );
    let after = physical_id(&router, row.stack, "Policy").await;
    assert_eq!(
        after, before,
        "an in-place update must keep the policy's identity"
    );

    let (code, body) = iam(&router, "GetPolicy", &[("PolicyArn", &after)]).await;
    assert_eq!(code, 200, "expected GetPolicy to answer, got {code}: {body}");
    let version = xml_text(&body, "DefaultVersionId").unwrap_or_default();
    let (code, body) = iam(
        &router,
        "GetPolicyVersion",
        &[("PolicyArn", &after), ("VersionId", &version)],
    )
    .await;
    assert_eq!(
        code, 200,
        "expected GetPolicyVersion to answer, got {code}: {body}"
    );
    let document = xml_policy_document(&body, "Document").unwrap_or_default();
    assert!(
        document.contains(changed.action) && !document.contains(row.action),
        "expected the default version to carry the document the update declared, got \
         {document:?}"
    );
}

/// One `AWS::IAM::User` row: which of the type's properties the declaration carries.
struct UserRow {
    case: &'static str,
    stack: &'static str,
    declared_name: Option<&'static str>,
    path: Option<&'static str>,
    /// The `(PolicyName, action)` of an inline policy the declaration embeds.
    inline_policy: Option<(&'static str, &'static str)>,
    /// A console password the declaration asks for.
    password: Option<&'static str>,
    /// The name of a group the account already holds and the declaration adds the user to.
    group: Option<&'static str>,
    /// A tag the declaration carries.
    tag: Option<(&'static str, &'static str)>,
}

/// The template one [`UserRow`] deploys, with the two identifiers the type publishes as
/// outputs so the stack's own report can be compared with IAM's record.
fn user_template(row: &UserRow) -> String {
    let mut properties = json!({});
    if let Some(name) = row.declared_name {
        properties["UserName"] = json!(name);
    }
    if let Some(path) = row.path {
        properties["Path"] = json!(path);
    }
    if let Some((name, action)) = row.inline_policy {
        properties["Policies"] = json!([{
            "PolicyName": name,
            "PolicyDocument": policy_document(action),
        }]);
    }
    if let Some(password) = row.password {
        properties["LoginProfile"] = json!({"Password": password, "PasswordResetRequired": true});
    }
    if let Some((key, value)) = row.tag {
        properties["Tags"] = json!([{"Key": key, "Value": value}]);
    }
    let mut resources = json!({"User": {
        "Type": "AWS::IAM::User",
        "Properties": properties,
    }});
    if let Some(group) = row.group {
        resources["User"]["Properties"]["Groups"] = json!([group]);
    }
    json!({
        "Resources": resources,
        "Outputs": {
            "UserRef": {"Value": {"Ref": "User"}},
            "UserArn": {"Value": {"Fn::GetAtt": ["User", "Arn"]}},
            "UserId": {"Value": {"Fn::GetAtt": ["User", "UserId"]}},
        },
    })
    .to_string()
}

/// The value of one declared output.
fn output(reported: &[(String, String)], key: &str) -> String {
    reported
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| panic!("expected an output {key}, got {reported:?}"))
}

/// RULE: a user a stack declares EXISTS in IAM — under the name the stack publishes for it
/// — with the path, inline policy, console password, group membership and tags the
/// declaration carried, each written through IAM's own API.
#[tokio::test]
async fn a_declared_user_exists_in_iam_with_everything_its_declaration_carries() {
    let rows = &[
        UserRow {
            case: "a user the template names, with a path, an inline policy and a tag",
            stack: "cfnuser-named",
            declared_name: Some("cfn-named-user"),
            path: Some("/engineering/"),
            inline_policy: Some(("read-objects", "s3:GetObject")),
            password: None,
            group: None,
            tag: Some(("team", "platform")),
        },
        UserRow {
            case: "a user with a console password and a group membership",
            stack: "cfnuser-console",
            declared_name: Some("cfn-console-user"),
            path: None,
            inline_policy: None,
            password: Some("a-console-password-1!"),
            group: Some("cfn-console-team"),
            tag: None,
        },
        UserRow {
            case: "a user CloudFormation names, with nothing else declared",
            stack: "cfnuser-generated",
            declared_name: None,
            path: None,
            inline_policy: None,
            password: None,
            group: None,
            tag: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        // A group the declaration adds the user to is one the ACCOUNT already holds — an
        // `AWS::IAM::Group` is not a type this emulator provisions — so the row creates it
        // through IAM's own wire first, which is also the position a template that names
        // an existing group is in.
        if let Some(group) = row.group {
            let (code, body) = iam(&router, "CreateGroup", &[("GroupName", group)]).await;
            assert_eq!(
                code, 200,
                "case {case:?}: the fixture group must be created, got {code}: {body}"
            );
        }
        let (status, reason) = deploy(&router, row.stack, &user_template(row)).await;
        assert_eq!(
            status.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {status:?} ({reason:?}): {}",
            create_failure(&router, row.stack, "User").await
        );

        let user_name = physical_id(&router, row.stack, "User").await;
        if let Some(declared) = row.declared_name {
            assert_eq!(
                user_name, declared,
                "case {case:?}: expected the declared UserName as the physical id"
            );
        }
        let reported = outputs(&router, row.stack).await;
        assert_eq!(
            output(&reported, "UserRef"),
            user_name,
            "case {case:?}: Ref must resolve to the user's name"
        );

        // IAM holds the user, at the declared path and under the ARN built from it.
        let (code, body) = iam(&router, "GetUser", &[("UserName", &user_name)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected GetUser({user_name:?}) to answer, got {code}: {body}"
        );
        let path = row.path.unwrap_or("/");
        assert_eq!(
            xml_text(&body, "Path").as_deref(),
            Some(path),
            "case {case:?}: expected the declared path on the user, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Arn").unwrap_or_default(),
            format!("arn:aws:iam::{ACCOUNT_ID}:user{path}{user_name}"),
            "case {case:?}: expected the ARN IAM builds from the path and the name"
        );
        // The identifiers the stack published for the user are the ones IAM holds.
        assert_eq!(
            output(&reported, "UserArn"),
            xml_text(&body, "Arn").unwrap_or_default(),
            "case {case:?}: the stack's Arn attribute must be IAM's own"
        );
        assert_eq!(
            output(&reported, "UserId"),
            xml_text(&body, "UserId").unwrap_or_default(),
            "case {case:?}: the stack's UserId attribute must be the one IAM minted"
        );

        if let Some((policy_name, action)) = row.inline_policy {
            let (code, body) = iam(
                &router,
                "GetUserPolicy",
                &[("UserName", &user_name), ("PolicyName", policy_name)],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected GetUserPolicy to answer, got {code}: {body}"
            );
            let document = xml_policy_document(&body, "PolicyDocument").unwrap_or_default();
            assert!(
                document.contains(action),
                "case {case:?}: expected the declared inline policy, got {document:?}"
            );
        }

        match row.password {
            // A declared password is a login profile IAM holds for the user.
            Some(_) => {
                let (code, body) =
                    iam(&router, "GetLoginProfile", &[("UserName", &user_name)]).await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: expected GetLoginProfile to answer, got {code}: {body}"
                );
                assert_eq!(
                    xml_text(&body, "PasswordResetRequired").as_deref(),
                    Some("true"),
                    "case {case:?}: expected the declared reset flag, got {body}"
                );
            }
            // A user with no declared password is still a user a client can give one to,
            // which is the plainest proof that the deployed identity is real.
            None => {
                let (code, body) = iam(
                    &router,
                    "CreateLoginProfile",
                    &[
                        ("UserName", &user_name),
                        ("Password", "a-later-password-1!"),
                    ],
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: expected CreateLoginProfile for the deployed user to \
                     succeed, got {code}: {body}"
                );
            }
        }

        if let Some(group) = row.group {
            let (code, body) = iam(&router, "GetGroup", &[("GroupName", group)]).await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected GetGroup to answer, got {code}: {body}"
            );
            assert!(
                xml_texts(&body, "UserName").contains(&user_name),
                "case {case:?}: expected the deployed user in the group, got {body}"
            );
        }

        if let Some((key, value)) = row.tag {
            let (code, body) = iam(&router, "ListUserTags", &[("UserName", &user_name)]).await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected ListUserTags to answer, got {code}: {body}"
            );
            assert!(
                xml_texts(&body, "Key").contains(&key.to_string())
                    && xml_texts(&body, "Value").contains(&value.to_string()),
                "case {case:?}: expected the declared tag on the user, got {body}"
            );
        }
    }
}

/// RULE (the in-place update half): a stack update that moves a user's PATH reaches the
/// user the stack already has, and "the user's unique identifier remains the same" — so
/// the `UserId` the stack published still names that user, while its ARN follows the path.
#[tokio::test]
async fn a_user_path_update_keeps_the_user_id_the_stack_published() {
    let row = UserRow {
        case: "a named user whose path the update moves",
        stack: "cfnuser-repath",
        declared_name: Some("cfn-repath-user"),
        path: Some("/before/"),
        inline_policy: None,
        password: None,
        group: None,
        tag: None,
    };
    let router = router();
    let (status, reason) = deploy(&router, row.stack, &user_template(&row)).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let user_name = physical_id(&router, row.stack, "User").await;
    let created_id = output(&outputs(&router, row.stack).await, "UserId");

    let moved = UserRow {
        path: Some("/after/"),
        ..row
    };
    let (status, reason) = redeploy(&router, row.stack, &user_template(&moved)).await;
    assert_eq!(
        status.as_deref(),
        Some("UPDATE_COMPLETE"),
        "expected the path update to complete, got {status:?} ({reason:?})"
    );

    let (code, body) = iam(&router, "GetUser", &[("UserName", &user_name)]).await;
    assert_eq!(code, 200, "expected GetUser to answer, got {code}: {body}");
    assert_eq!(
        xml_text(&body, "Path").as_deref(),
        Some("/after/"),
        "expected the path the update declared, got {body}"
    );
    assert_eq!(
        xml_text(&body, "UserId").unwrap_or_default(),
        created_id,
        "expected the user to keep the identifier it was created with"
    );
    assert_eq!(
        xml_text(&body, "Arn").unwrap_or_default(),
        format!("arn:aws:iam::{ACCOUNT_ID}:user/after/{user_name}"),
        "expected the ARN to follow the new path"
    );
}

/// RULE (the in-place update half, again): a stack update that REPLACES a user's inline
/// policy puts the policy it now declares and deletes the one it no longer does.
#[tokio::test]
async fn a_stack_update_replaces_the_users_inline_policy() {
    let row = UserRow {
        case: "a named user whose inline policy the update replaces",
        stack: "cfnuser-repolicy",
        declared_name: Some("cfn-repolicy-user"),
        path: None,
        inline_policy: Some(("first", "s3:GetObject")),
        password: None,
        group: None,
        tag: None,
    };
    let router = router();
    let (status, reason) = deploy(&router, row.stack, &user_template(&row)).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let user_name = physical_id(&router, row.stack, "User").await;

    let replaced = UserRow {
        inline_policy: Some(("second", "sqs:ReceiveMessage")),
        ..row
    };
    let (status, reason) = redeploy(&router, row.stack, &user_template(&replaced)).await;
    assert_eq!(
        status.as_deref(),
        Some("UPDATE_COMPLETE"),
        "expected the update to complete, got {status:?} ({reason:?})"
    );

    let (code, body) = iam(
        &router,
        "GetUserPolicy",
        &[("UserName", &user_name), ("PolicyName", "second")],
    )
    .await;
    assert_eq!(
        code, 200,
        "expected the policy the update declared, got {code}: {body}"
    );
    let document = xml_policy_document(&body, "PolicyDocument").unwrap_or_default();
    assert!(
        document.contains("sqs:ReceiveMessage"),
        "expected the new document on the user, got {document:?}"
    );
    let (code, body) = iam(
        &router,
        "GetUserPolicy",
        &[("UserName", &user_name), ("PolicyName", "first")],
    )
    .await;
    assert_eq!(
        code, 404,
        "expected the policy the update dropped to be gone, got {code}: {body}"
    );
}

/// RULE (the delete half): deleting the stack deletes the user it created, including the
/// console password and the policies the declaration gave it — none of which `DeleteUser`
/// would tolerate being left behind.
#[tokio::test]
async fn deleting_the_stack_removes_the_user_with_its_login_profile() {
    let row = UserRow {
        case: "a user with a password, an inline policy and a membership",
        stack: "cfnuser-cascade",
        declared_name: Some("cfn-cascade-user"),
        path: None,
        inline_policy: Some(("granted", "s3:GetObject")),
        password: Some("a-console-password-1!"),
        group: Some("cfn-cascade-team"),
        tag: None,
    };
    let router = router();
    let (code, body) = iam(
        &router,
        "CreateGroup",
        &[("GroupName", row.group.unwrap_or_default())],
    )
    .await;
    assert_eq!(
        code, 200,
        "the fixture group must be created, got {code}: {body}"
    );
    let (status, reason) = deploy(&router, row.stack, &user_template(&row)).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let user_name = physical_id(&router, row.stack, "User").await;

    delete_stack(&router, row.stack).await;
    let (code, body) = iam(&router, "GetUser", &[("UserName", &user_name)]).await;
    assert_eq!(
        code, 404,
        "expected the user to be gone from IAM, got {code}: {body}"
    );
    let (code, body) = iam(&router, "GetLoginProfile", &[("UserName", &user_name)]).await;
    assert_eq!(
        code, 404,
        "expected the login profile to be gone too, got {code}: {body}"
    );
}

/// RULE (both verdicts IAM can give, on a role's trust policy): a document IAM refuses is
/// no role, so the stack rolls back instead of completing; a CHANGED document is an
/// in-place update of the role the stack already has, so the stack completes and the role
/// carries the new trust policy.
#[tokio::test]
async fn a_roles_trust_policy_is_iams_verdict_on_create_and_on_update() {
    let router = router();

    // Every way a template can state an `AssumeRolePolicyDocument` that is no IAM policy
    // document: text that is not JSON at all, JSON that is not an object, and an object
    // carrying no `Statement`.
    let malformed = [
        ("not JSON at all", json!("trust me")),
        ("a JSON list", json!([{"Effect": "Allow"}])),
        ("an object with no Statement", json!({"Version": "2012-10-17"})),
    ];
    for (index, (what, document)) in malformed.iter().enumerate() {
        let stack = format!("cfnrole-malformed-{index}");
        let template = json!({"Resources": {"Role": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": format!("cfn-malformed-{index}"),
                "AssumeRolePolicyDocument": document,
            },
        }}})
        .to_string();
        let (status, reason) = deploy(&router, &stack, &template).await;
        assert_eq!(
            status.as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {what:?}: expected the stack to roll back, got {status:?} ({reason:?})"
        );
        let (code, body) = iam(
            &router,
            "GetRole",
            &[("RoleName", &format!("cfn-malformed-{index}"))],
        )
        .await;
        assert_eq!(
            code, 404,
            "case {what:?}: expected no role to have been created, got {code}: {body}"
        );
    }

    // The update half: only the trust policy changes, which `UpdateAssumeRolePolicy`
    // applies to the role the stack already has.
    let role_name = "cfn-retrust-role";
    let template = |principal: &str| {
        json!({"Resources": {"Role": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": role_name,
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": principal},
                        "Action": "sts:AssumeRole"
                    }]
                },
            },
        }}})
        .to_string()
    };
    let (status, reason) = deploy(&router, "cfnrole-retrust", &template("ec2.amazonaws.com")).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let (status, reason) = redeploy(
        &router,
        "cfnrole-retrust",
        &template("lambda.amazonaws.com"),
    )
    .await;
    assert_eq!(
        status.as_deref(),
        Some("UPDATE_COMPLETE"),
        "expected the trust-policy update to complete, got {status:?} ({reason:?})"
    );
    let (code, body) = iam(&router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(code, 200, "expected GetRole to answer, got {code}: {body}");
    let document = xml_policy_document(&body, "AssumeRolePolicyDocument").unwrap_or_default();
    assert!(
        document.contains("lambda.amazonaws.com") && !document.contains("ec2.amazonaws.com"),
        "expected the trust policy the update declared, got {document:?}"
    );
}

/// One `AWS::IAM::AccessKey` row: how the key, and the user that holds it, are declared.
struct AccessKeyRow {
    case: &'static str,
    stack: &'static str,
    /// The `UserName` the template gives the user the key belongs to, or `None` for a user
    /// whose name CloudFormation generates.
    user_name: Option<&'static str>,
    /// The `Status` the declaration asks the key to be in, or `None` where it declares none
    /// — in which case the key carries the status every minted key does.
    status: Option<&'static str>,
    /// The `Serial` the declaration carries, or `None` where it declares none.
    serial: Option<u64>,
}

/// The status a key carries where its declaration states none: "the default status for new
/// keys is Active" (`CreateAccessKey`).
const DEFAULT_KEY_STATUS: &str = "Active";

/// The template one [`AccessKeyRow`] deploys at the `status`/`serial` given: a user, that
/// user's access key, and every value either type publishes as an output — which is how a
/// client reads back the credential a stack deployed.
fn access_key_template(row: &AccessKeyRow, status: Option<&str>, serial: Option<u64>) -> String {
    let mut user = json!({});
    if let Some(name) = row.user_name {
        user["UserName"] = json!(name);
    }
    let mut key = json!({"UserName": {"Ref": "User"}});
    if let Some(status) = status {
        key["Status"] = json!(status);
    }
    if let Some(serial) = serial {
        key["Serial"] = json!(serial);
    }
    json!({
        "Resources": {
            "User": {"Type": "AWS::IAM::User", "Properties": user},
            "Key": {"Type": "AWS::IAM::AccessKey", "Properties": key},
        },
        "Outputs": {
            "UserName": {"Value": {"Ref": "User"}},
            "AccessKeyId": {"Value": {"Ref": "Key"}},
            "SecretAccessKey": {"Value": {"Fn::GetAtt": ["Key", "SecretAccessKey"]}},
        },
    })
    .to_string()
}

/// The `(AccessKeyId, Status)` pairs IAM reports for one user, which is the only read that
/// service publishes of a user's keys.
async fn held_keys(router: &MultiServiceRouter, user_name: &str) -> Vec<(String, String)> {
    let (code, body) = iam(router, "ListAccessKeys", &[("UserName", user_name)]).await;
    assert_eq!(
        code, 200,
        "expected ListAccessKeys({user_name:?}) to answer, got {code}: {body}"
    );
    xml_texts(&body, "AccessKeyId")
        .into_iter()
        .zip(xml_texts(&body, "Status"))
        .collect()
}

/// The status IAM reports for one of a user's keys, or `None` where that user holds no such
/// key any more.
async fn held_status(
    router: &MultiServiceRouter,
    user_name: &str,
    access_key_id: &str,
) -> Option<String> {
    held_keys(router, user_name)
        .await
        .into_iter()
        .find(|(id, _)| id == access_key_id)
        .map(|(_, status)| status)
}

/// A secret access key IAM mints on its own wire, for a user created here: the CONTROL a
/// stack's published secret is compared against, so what a row asserts about that secret is
/// the shape IAM gives a credential rather than any particular value.
async fn minted_secret(router: &MultiServiceRouter, user_name: &str) -> String {
    for (action, params) in [
        ("CreateUser", vec![("UserName", user_name)]),
        ("CreateAccessKey", vec![("UserName", user_name)]),
    ] {
        let (code, body) = iam(router, action, &params).await;
        assert_eq!(
            code, 200,
            "the control credential needs {action} to answer, got {code}: {body}"
        );
        if action == "CreateAccessKey" {
            return xml_text(&body, "SecretAccessKey")
                .unwrap_or_else(|| panic!("expected a minted secret, got {body}"));
        }
    }
    unreachable!("the loop returns on CreateAccessKey")
}

/// RULE: every output of a settled stack carries a VALUE — the one the type's published
/// return values define — so a `Ref` to a deployed access key resolves to the access key id
/// IAM minted and an `Fn::GetAtt` of its `SecretAccessKey` to the secret IAM answered with,
/// never to a name CloudFormation generated or to no value at all.
///
/// The rows differ along what the rule generalises over: whether the template names the user
/// its key belongs to or leaves the name to CloudFormation, and which `Status`/`Serial` the
/// declaration carries.
#[tokio::test]
async fn a_deployed_access_key_is_published_with_the_credential_iam_minted() {
    let rows = &[
        AccessKeyRow {
            case: "a key of a user the template names, declaring neither status nor serial",
            stack: "cfnkey-named",
            user_name: Some("cfn-named-key-user"),
            status: None,
            serial: None,
        },
        AccessKeyRow {
            case: "a key the declaration asks for Active, of a user CloudFormation names",
            stack: "cfnkey-active",
            user_name: None,
            status: Some("Active"),
            serial: Some(1),
        },
        AccessKeyRow {
            case: "a key the declaration asks for Inactive",
            stack: "cfnkey-inactive",
            user_name: Some("cfn-inactive-key-user"),
            status: Some("Inactive"),
            serial: Some(7),
        },
    ];
    for row in rows {
        let case = row.case;
        let router = router();
        let control = minted_secret(&router, "cfnkey-control-user").await;
        let (status, reason) = deploy(
            &router,
            row.stack,
            &access_key_template(row, row.status, row.serial),
        )
        .await;
        assert_eq!(
            status.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {status:?} ({reason:?})"
        );

        // Every declared output is reported, and every one of them has a value: a stack that
        // settled with an output carrying no `OutputValue` publishes nothing a client can
        // read the deployed credential out of. WHICH outputs, not in which order — the
        // protocol fixes no order for `Stack$Outputs`.
        let reported = outputs(&router, row.stack).await;
        let mut keys: Vec<&str> = reported.iter().map(|(key, _)| key.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["AccessKeyId", "SecretAccessKey", "UserName"],
            "case {case:?}: expected one entry per declared output, got {reported:?}"
        );
        for (key, value) in &reported {
            assert!(
                !value.is_empty(),
                "case {case:?}: expected output {key} to carry a value, got {reported:?}"
            );
        }

        let user_name = output(&reported, "UserName");
        if let Some(declared) = row.user_name {
            assert_eq!(
                user_name, declared,
                "case {case:?}: expected the user the template named"
            );
        }

        // The id the stack publishes is the resource's identity AND the one IAM holds the
        // key under — not the `<stack>-<logical id>-<unique id>` name CloudFormation
        // generates for a resource whose own service names nothing.
        let access_key_id = output(&reported, "AccessKeyId");
        assert_eq!(
            access_key_id,
            physical_id(&router, row.stack, "Key").await,
            "case {case:?}: expected Ref to resolve to the key's physical id"
        );
        assert!(
            !access_key_id.contains(row.stack),
            "case {case:?}: expected the id IAM minted, got the generated name \
             {access_key_id:?}"
        );
        let held = held_status(&router, &user_name, &access_key_id).await;
        assert_eq!(
            held.as_deref(),
            Some(row.status.unwrap_or(DEFAULT_KEY_STATUS)),
            "case {case:?}: expected IAM to hold the key in the declared status, got {:?}",
            held_keys(&router, &user_name).await
        );

        // The secret is the one IAM answered the create with: a credential of the shape that
        // service mints, and this key's own rather than any other's.
        let secret = output(&reported, "SecretAccessKey");
        assert_eq!(
            secret.len(),
            control.len(),
            "case {case:?}: expected a secret of the length IAM mints, got {secret:?}"
        );
        assert!(
            !secret.contains(char::is_whitespace),
            "case {case:?}: expected a credential, got {secret:?}"
        );
        assert_ne!(
            secret, control,
            "case {case:?}: each access key carries its own secret"
        );
    }
}

/// RULE (the in-place update half): `Status` is the one property IAM can change on a key
/// that exists — "changes the status of the specified access key from Active to Inactive, or
/// vice versa" — so a stack update that deactivates a deployed key keeps the key's identity:
/// the same id and the same secret, in the status the template now declares.
#[tokio::test]
async fn deactivating_a_deployed_access_key_keeps_the_credential_it_published() {
    let row = AccessKeyRow {
        case: "a key whose status the update moves",
        stack: "cfnkey-deactivated",
        user_name: Some("cfn-deactivated-key-user"),
        status: None,
        serial: Some(1),
    };
    let router = router();
    let (status, reason) = deploy(
        &router,
        row.stack,
        &access_key_template(&row, None, row.serial),
    )
    .await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let before = outputs(&router, row.stack).await;
    let user_name = output(&before, "UserName");
    let access_key_id = output(&before, "AccessKeyId");

    // Both directions the one call serves, each from the status the previous deployment left.
    for asked in ["Inactive", "Active"] {
        let (status, reason) = redeploy(
            &router,
            row.stack,
            &access_key_template(&row, Some(asked), row.serial),
        )
        .await;
        assert_eq!(
            status.as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {asked:?}: expected the status update to complete, got {status:?} \
             ({reason:?})"
        );
        let held = held_status(&router, &user_name, &access_key_id).await;
        assert_eq!(
            held.as_deref(),
            Some(asked),
            "case {asked:?}: expected IAM to report the status the template declares, \
             got {:?}",
            held_keys(&router, &user_name).await
        );
        let after = outputs(&router, row.stack).await;
        assert_eq!(
            after, before,
            "case {asked:?}: an in-place update must leave the published credential as it is"
        );
    }
}

/// RULE (the replacement half): the `Serial` is create-only — "incrementing this value
/// notifies CloudFormation that you want to rotate your access key… CloudFormation will
/// replace the AccessKey resource with a new access key" — so a stack update that increments
/// it publishes a NEW credential and the key it replaced is gone from IAM.
#[tokio::test]
async fn rotating_the_serial_replaces_the_key_and_takes_the_old_one_away() {
    let row = AccessKeyRow {
        case: "a key the update rotates",
        stack: "cfnkey-rotated",
        user_name: Some("cfn-rotated-key-user"),
        status: None,
        serial: Some(1),
    };
    let router = router();
    let (status, reason) = deploy(
        &router,
        row.stack,
        &access_key_template(&row, None, Some(1)),
    )
    .await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let before = outputs(&router, row.stack).await;
    let user_name = output(&before, "UserName");
    let rotated_out = output(&before, "AccessKeyId");

    let (status, reason) = redeploy(
        &router,
        row.stack,
        &access_key_template(&row, None, Some(2)),
    )
    .await;
    assert_eq!(
        status.as_deref(),
        Some("UPDATE_COMPLETE"),
        "expected the rotation to complete, got {status:?} ({reason:?})"
    );
    let after = outputs(&router, row.stack).await;
    let rotated_in = output(&after, "AccessKeyId");
    assert_ne!(
        rotated_in, rotated_out,
        "expected a rotation to publish a new access key id"
    );
    assert_ne!(
        output(&after, "SecretAccessKey"),
        output(&before, "SecretAccessKey"),
        "expected the new key's own secret"
    );
    let held = held_keys(&router, &user_name).await;
    assert!(
        held.iter().any(|(id, _)| id == &rotated_in),
        "expected IAM to hold the key the rotation created, got {held:?}"
    );
    assert!(
        !held.iter().any(|(id, _)| id == &rotated_out),
        "expected the rotated-out key to be gone from IAM, got {held:?}"
    );
}

/// RULE (the delete half, and both verdicts IAM can give): deleting the stack deletes the
/// access key it created before the user that holds it — which is the only order IAM admits,
/// "you must delete the items attached to the user manually, or the deletion fails" — while
/// a key declared for a user nothing holds is no credential at all, so the stack fails with
/// IAM's own refusal instead of completing.
#[tokio::test]
async fn an_access_keys_lifecycle_is_iams_verdict_on_create_and_on_delete() {
    let row = AccessKeyRow {
        case: "a key the stack's deletion takes away",
        stack: "cfnkey-cascade",
        user_name: Some("cfn-cascade-key-user"),
        status: None,
        serial: None,
    };
    let router = router();
    let (status, reason) =
        deploy(&router, row.stack, &access_key_template(&row, None, None)).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let user_name = output(&outputs(&router, row.stack).await, "UserName");

    delete_stack(&router, row.stack).await;
    let (code, body) = iam(&router, "GetUser", &[("UserName", &user_name)]).await;
    assert_eq!(
        code, 404,
        "expected the user and its key to be gone from IAM, got {code}: {body}"
    );

    // The create half's other verdict: the key names a user the account does not hold, so
    // IAM refuses and the stack rolls back carrying that refusal.
    let orphan = "cfn-orphan-key-user";
    let template = json!({"Resources": {"Key": {
        "Type": "AWS::IAM::AccessKey",
        "Properties": {"UserName": orphan},
    }}})
    .to_string();
    let (status, reason) = deploy(&router, "cfnkey-orphan", &template).await;
    assert_eq!(
        status.as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "expected the stack to roll back, got {status:?} ({reason:?})"
    );
    let failure = create_failure(&router, "cfnkey-orphan", "Key").await;
    assert!(
        failure.contains(orphan),
        "expected IAM's refusal to name the user it could not find, got {failure:?}"
    );

    // And the same verdict on the status the declaration asks for: IAM will not put a key
    // in a status outside the ones it models, so the resource is not created — and because
    // the key minted on the way there is withdrawn, the rollback can delete the user the
    // same deployment created, which IAM refuses while a user still holds a key.
    let refused = AccessKeyRow {
        case: "a key declaring a status IAM will not set",
        stack: "cfnkey-badstatus",
        user_name: Some("cfn-badstatus-key-user"),
        status: Some("Disabled"),
        serial: None,
    };
    let (status, reason) = deploy(
        &router,
        refused.stack,
        &access_key_template(&refused, refused.status, None),
    )
    .await;
    assert_eq!(
        status.as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "expected the stack to roll back, got {status:?} ({reason:?})"
    );
    let (code, body) = iam(
        &router,
        "GetUser",
        &[("UserName", refused.user_name.unwrap_or_default())],
    )
    .await;
    assert_eq!(
        code, 404,
        "expected the rollback to have deleted the user, which it can only do once the \
         minted key is withdrawn, got {code}: {body}"
    );
}
