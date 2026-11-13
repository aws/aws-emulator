// SPDX-License-Identifier: CC0-1.0

//! What a FAILED stack creation leaves behind, over the awsQuery wire.
//!
//! RULE: a resource the service that owns it REFUSES to create — including a refusal that
//! the name is already taken in that service's own namespace — fails the create, and
//! CloudFormation then unwinds it as `CreateStackInput$OnFailure` says: `ROLLBACK` (the
//! default) removes the resources the create had already made and the stack reports
//! `ROLLBACK_COMPLETE`, `DO_NOTHING` (which `DisableRollback=true` also spells) leaves the
//! stack `CREATE_FAILED` with those resources in place, and `DELETE` takes the stack away
//! with them. Whichever it is, a resource the stack did NOT create still exists in the
//! service that owns it: the unwind removes only what the create itself made.
//!
//! The collision used here is an IAM role NAME, because IAM names are unique across the
//! whole account while a stack name is unique only within a Region: a template whose role
//! name is not Region-qualified therefore cannot be deployed twice, and the second deploy
//! has to fail loudly rather than quietly share the first deploy's role. The clash is one no
//! stack of the deploying Region manages, so the create reaches IAM at all only with the
//! pre-deployment validations OFF — with them on, the existence check fails the create
//! before any resource is asked for and there is no refused resource to unwind
//! (`state::early_validation`) — and every request below states `DisableValidation`.
//!
//! The rows differ along what the rule generalises over: every documented `OnFailure`
//! value plus `DisableRollback`, and where the clash comes from — another Region's stack
//! (the recorded case) or a role a client created outside any stack.

use serde_json::json;

use crate::stack_lifecycle::{
    acknowledging_iam, call, call_in, create_stack, describe_stack, iam, member,
    resource_event_reason, wait_for_the_transition, xml_text, REGION,
};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status, status,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the misdeployed copy of the recorded case sits in: the clash is only
/// interesting because the ROLE name is Region-free while the stack name is not.
const OTHER_REGION: &str = "us-west-2";

/// Where the role name the failing stack declares is already taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clash {
    /// The recorded case: another Region's stack created the role, under the same
    /// template.
    AnotherRegionsStack,
    /// A role a client created through IAM itself, with no stack involved.
    AClientsOwnRole,
}

/// What the stack must report once the create has failed and the unwind has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unwound {
    /// `ROLLBACK_COMPLETE`, and what the create had made is gone.
    RolledBack,
    /// `CREATE_FAILED`, and what the create had made is still there.
    LeftAsItFailed,
    /// The stack itself is gone.
    Deleted,
}

/// One row: the request members that decide the unwind, and what must come of it.
struct Row<'a> {
    case: &'a str,
    /// The stack name, which also seeds the bucket and role names so rows cannot collide
    /// with each other.
    stack: &'a str,
    /// `OnFailure` / `DisableRollback`, exactly as the request carries them.
    extra: &'a [(&'a str, &'a str)],
    clash: Clash,
    expected: Unwound,
}

/// The template: a bucket CloudFormation CAN create, declared first, and a role whose name
/// is already taken, declared second — so the create gets as far as the bucket and then
/// fails, which is what gives the unwind something to remove.
fn template(bucket: &str, role: &str) -> String {
    json!({
        "Resources": {
            // Logical ids chosen so the bucket is deployed first whether the template
            // reader yields declaration order or sorted order.
            "ABucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": bucket}
            },
            "ZRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role,
                    "AssumeRolePolicyDocument": trust_policy()
                }
            }
        },
        "Outputs": {"BucketName": {"Value": {"Ref": "ABucket"}}}
    })
    .to_string()
}

/// A trust policy IAM accepts: an IAM JSON policy document carrying a `Statement`.
fn trust_policy() -> serde_json::Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "glue.amazonaws.com"},
            "Action": "sts:AssumeRole"
        }]
    })
}

/// Does Amazon S3 hold this bucket?
async fn bucket_exists(router: &MultiServiceRouter, bucket: &str) -> bool {
    s3(router, "HEAD", &format!("/{bucket}"), b"").await.status == 200
}

/// Does IAM hold this role?
async fn role_exists(router: &MultiServiceRouter, role: &str) -> bool {
    iam(router, "GetRole", &[("RoleName", role)]).await.0 == 200
}

#[tokio::test]
async fn a_create_whose_resource_is_refused_is_unwound_as_on_failure_says() {
    let rows: &[Row<'_>] = &[
        Row {
            case: "no OnFailure at all: ROLLBACK is the default",
            stack: "unwind-default",
            extra: &[],
            clash: Clash::AnotherRegionsStack,
            expected: Unwound::RolledBack,
        },
        Row {
            case: "OnFailure=ROLLBACK, stated",
            stack: "unwind-rollback",
            extra: &[("OnFailure", "ROLLBACK")],
            clash: Clash::AClientsOwnRole,
            expected: Unwound::RolledBack,
        },
        Row {
            case: "OnFailure=DO_NOTHING keeps the failed state",
            stack: "unwind-do-nothing",
            extra: &[("OnFailure", "DO_NOTHING")],
            clash: Clash::AnotherRegionsStack,
            expected: Unwound::LeftAsItFailed,
        },
        Row {
            case: "DisableRollback=true spells the same thing",
            stack: "unwind-disabled",
            extra: &[("DisableRollback", "true")],
            clash: Clash::AClientsOwnRole,
            expected: Unwound::LeftAsItFailed,
        },
        Row {
            case: "OnFailure=DELETE takes the stack away",
            stack: "unwind-delete",
            extra: &[("OnFailure", "DELETE")],
            clash: Clash::AClientsOwnRole,
            expected: Unwound::Deleted,
        },
    ];

    for row in rows {
        let router = crate::stack_lifecycle::router();
        // The role name carries no Region, which is the whole point: one account-global
        // name, declared by a template two deployments share.
        let role = format!("{}-role", row.stack);
        let bucket = format!("{}-bucket", row.stack);

        // Whoever holds that role name first.
        match row.clash {
            Clash::AnotherRegionsStack => {
                let first = format!("{}-first-bucket", row.stack);
                let (status, body) = call_in(
                    &router,
                    OTHER_REGION,
                    "CreateStack",
                    &acknowledging_iam(&[
                        member("StackName", row.stack),
                        member("TemplateBody", &template(&first, &role)),
                    ]),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the first deployment must be accepted, got {status} {body}",
                    row.case
                );
                wait_for_the_transition().await;
                let (_, described) = call_in(
                    &router,
                    OTHER_REGION,
                    "DescribeStacks",
                    &[member("StackName", row.stack)],
                )
                .await;
                assert_eq!(
                    xml_text(&described, "StackStatus").as_deref(),
                    Some(status::CREATE_COMPLETE),
                    "case {:?}: the first deployment must complete, got {described}",
                    row.case
                );
            }
            Clash::AClientsOwnRole => {
                let (status, body) = iam(
                    &router,
                    "CreateRole",
                    &[
                        ("RoleName", role.as_str()),
                        ("AssumeRolePolicyDocument", &trust_policy().to_string()),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the client's own CreateRole must succeed, got {status} \
                     {body}",
                    row.case
                );
            }
        }

        // The deployment that cannot have the role: the stack name is free in THIS Region,
        // so the create starts, and the role is what fails it.
        let members = acknowledging_iam(
            &row.extra
                .iter()
                .map(|(key, value)| member(key, value))
                .chain([member("DisableValidation", "true")])
                .collect::<Vec<_>>(),
        );
        let (status, created) =
            create_stack(&router, row.stack, &template(&bucket, &role), &members).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateStack must answer 200 — the creation starts, and the stack \
             reports the outcome — got {status} {created}",
            row.case
        );
        wait_for_the_transition().await;

        let (status, described) = describe_stack(&router, row.stack).await;
        match row.expected {
            Unwound::Deleted => {
                assert_eq!(
                    status, 400,
                    "case {:?}: the stack must be gone, got {status} {described}",
                    row.case
                );
                assert!(
                    !bucket_exists(&router, &bucket).await,
                    "case {:?}: the bucket the failed create made must go with the stack",
                    row.case
                );
            }
            Unwound::RolledBack | Unwound::LeftAsItFailed => {
                assert_eq!(
                    status, 200,
                    "case {:?}: DescribeStacks must answer 200, got {status} {described}",
                    row.case
                );
                let expected_status = if row.expected == Unwound::RolledBack {
                    status::ROLLBACK_COMPLETE
                } else {
                    status::CREATE_FAILED
                };
                assert_eq!(
                    xml_text(&described, "StackStatus").as_deref(),
                    Some(expected_status),
                    "case {:?}: expected {expected_status}, got {described}",
                    row.case
                );
                // The STACK's reason is the summary sentence naming the resource that
                // failed, and nothing else; IAM's own refusal is reported on that
                // resource's event instead, where a client reads the per-resource cause.
                let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
                assert_eq!(
                    reason,
                    resource_failure_summary(failed_action::CREATE, ["ZRole"]),
                    "case {:?}: expected the stack reason to be the summary of what failed, \
                     got {reason:?}",
                    row.case
                );
                let on_the_resource = resource_event_reason(
                    &router,
                    row.stack,
                    "ZRole",
                    resource_status::CREATE_FAILED,
                )
                .await;
                assert!(
                    on_the_resource.contains(&format!("Role with name {role} already exists."))
                        && on_the_resource.contains("HandlerErrorCode: AlreadyExists"),
                    "case {:?}: expected IAM's own refusal on the failed resource's event, \
                     got {on_the_resource:?}",
                    row.case
                );
                // No output resolves over resources the stack does not have.
                assert!(
                    xml_text(&described, "OutputValue").is_none(),
                    "case {:?}: a create that did not complete publishes no outputs, got \
                     {described}",
                    row.case
                );
                let bucket_kept = bucket_exists(&router, &bucket).await;
                assert_eq!(
                    bucket_kept,
                    row.expected == Unwound::LeftAsItFailed,
                    "case {:?}: expected the bucket the create had made to be {}, got \
                     kept={bucket_kept}",
                    row.case,
                    if row.expected == Unwound::LeftAsItFailed {
                        "left in place"
                    } else {
                        "removed by the rollback"
                    }
                );
            }
        }

        // Whatever the unwind did, the role it could not create is untouched: it was never
        // this stack's to remove.
        assert!(
            role_exists(&router, &role).await,
            "case {:?}: the role that was already there must survive the failed create",
            row.case
        );
        if row.clash == Clash::AnotherRegionsStack {
            let (_, described) = call_in(
                &router,
                OTHER_REGION,
                "DescribeStacks",
                &[member("StackName", row.stack)],
            )
            .await;
            assert_eq!(
                xml_text(&described, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {:?}: the stack that owns the role must be untouched, got {described}",
                row.case
            );
        }
    }
}

/// The resources a rolled-back create had made are gone from the services that own them,
/// while the stack KEEPS a record of each resource the create declared, at the terminal
/// `DELETE_COMPLETE` the rollback disposed of it at — which is what says which declared
/// resource was disposed of.
///
/// The row here is the recorded shape end to end: one template, deployed to the Region it
/// belongs in while the misdeployed copy still holds the Region-free role name.
#[tokio::test]
async fn a_rolled_back_create_keeps_its_resource_records_and_holds_no_resources() {
    let router = crate::stack_lifecycle::router();
    let role = "redeploy-global-role";
    let (status, body) = iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", role),
            ("AssumeRolePolicyDocument", &trust_policy().to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the role must be taken first, got {status} {body}"
    );

    let (status, created) = create_stack(
        &router,
        "rolled-back-stack",
        &template("rolled-back-bucket", role),
        &acknowledging_iam(&[member("DisableValidation", "true")]),
    )
    .await;
    assert_eq!(status, 200, "CreateStack must answer 200, got {status} {created}");
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "rolled-back-stack").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "the failed create must be rolled back, got {described}"
    );
    assert!(
        !bucket_exists(&router, "rolled-back-bucket").await,
        "the rollback must have removed the bucket the create made"
    );

    // Each resource the create declared is still readable, at the status the rollback left
    // it in: the row is the only thing that says what the rollback disposed of.
    for (case, logical_id, resource_type) in [
        (
            "the resource the create had made",
            "ABucket",
            "AWS::S3::Bucket",
        ),
        ("the resource that failed", "ZRole", "AWS::IAM::Role"),
    ] {
        let (status, body) = call(
            &router,
            "DescribeStackResource",
            &[
                member("StackName", "rolled-back-stack"),
                member("LogicalResourceId", logical_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a rolled-back stack still reports {logical_id}, got {status} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourceStatus").as_deref(),
            Some("DELETE_COMPLETE"),
            "case {case:?}: expected the rollback's terminal status for {logical_id}, got \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourceType").as_deref(),
            Some(resource_type),
            "case {case:?}: expected the declared type for {logical_id}, got {body}"
        );
    }

    // The stack is still there to be deleted, in the Region it was created in, and its
    // name is taken until it is: "a stack name must be unique in the Region".
    let (status, clash) = create_stack(
        &router,
        "rolled-back-stack",
        &template("another-bucket", "another-role"),
        &acknowledging_iam(&[]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a rolled-back stack still holds its name in {REGION}, got {status} {clash}"
    );
    let (status, deleted) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "rolled-back-stack")],
    )
    .await;
    assert_eq!(
        status, 200,
        "a rolled-back stack must be deletable, got {status} {deleted}"
    );
    wait_for_the_transition().await;
    assert!(
        role_exists(&router, role).await,
        "deleting the rolled-back stack must not delete the role it never created"
    );
}
