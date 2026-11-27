// SPDX-License-Identifier: CC0-1.0

//! What the CALLING PRINCIPAL lets a deployment do, end to end over the wire.
//!
//! RULE under test: **a stack that names no `RoleARN` is carried out with the CALLER's
//! credentials — "CloudFormation uses a temporary session that's generated from your user
//! credentials" — so every call such a deployment makes in the service that OWNS a declared
//! resource is decided against the calling principal's own identity documents, exactly as that
//! service would have decided the call had the client made it itself: under IAM's default deny a
//! principal whose readable documents do not allow the ACTION the resource's create invokes, on
//! the resource it is about, cannot create it, and the resource fails with that service's 403
//! authorization answer — the sentence naming the principal, the action and the resource,
//! classified `AccessDenied` — while the owning service holds nothing. A principal the documents
//! DO allow that action deploys the same template unchanged.**
//!
//! `service_role_grants_decide_the_deployment.rs` is the other half of the same reading, for a
//! stack that hands CloudFormation a role to assume instead; the last row here is the seam
//! between them: the same denied caller deploys the same template when a SERVICE ROLE is in
//! force, because the deployment's calls are then that role's and not the caller's.
//!
//! The rows differ along what the rule generalises over: which service owns the declared
//! resource and which wire it speaks (AWS Systems Manager on awsJson, Amazon SNS on awsQuery),
//! what the caller's one inline policy says about that service (the called action named in full,
//! a wildcard over the service, another action of the same service, the called action scoped to
//! another resource of it, nothing of it at all, and an allow of everything beside an explicit
//! deny of the call), and whether a stack service role is in force at all.
//!
//! Nothing here sleeps for a span the emulator owes: every wait polls for the CONDITION it is
//! about under a generous deadline.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

use crate::harness::{
    call, call_as, envelope_parts, member, router_with_engine, xml_blocks, xml_text, REGION,
};
use crate::service_role_permissions::trusts_cloudformation;
use crate::stack_lifecycle::{iam, sns, user_with_inline_policy, ACCOUNT_ID};
use crate::update_rollback_recovery::{parameter, parameter_value, template};

/// The `HandlerErrorCode` class a call that was not permitted is reported under, which is what
/// tells a client whether the failed resource is worth retrying.
const ACCESS_DENIED_CLASS: &str = "AccessDenied";

/// How long a case looks again for a condition the emulator owes, and how often.
///
/// Generous: every wait here is for a CONDITION (a stack reporting a status, an event being
/// recorded), never for a span of the clock, so a loaded host cannot decide a case.
const DEADLINE: Duration = Duration::from_secs(60);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The role one row hands the STACK, whose credentials that deployment's calls are then made
/// with instead of the caller's. It carries no permissions document at all, which is a role
/// whose permissions this emulator cannot establish — so what that row shows is that the
/// CALLER's documents stopped deciding, not that the role's allowed the call.
const SERVICE_ROLE: &str = "caller-grant-service-role";

/// The service that OWNS the resource a row declares, which is the service the deployment's
/// calls about it are made to — so it is also the action namespace the caller's documents are
/// asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Parameter,
    Topic,
}

impl Owner {
    /// The action namespace of that service, which is its signing name.
    fn namespace(self) -> &'static str {
        match self {
            Self::Parameter => "ssm",
            Self::Topic => "sns",
        }
    }

    /// The operation a CREATE of that resource calls on its owning service, as that service's
    /// own model names it: a Parameter Store parameter is written by `PutParameter`, a topic is
    /// created by `CreateTopic`.
    fn created_by(self) -> &'static str {
        match self {
            Self::Parameter => "PutParameter",
            Self::Topic => "CreateTopic",
        }
    }

    /// The logical id the template declares that resource under.
    fn logical_id(self) -> &'static str {
        match self {
            Self::Parameter => "Param",
            Self::Topic => "Topic",
        }
    }
}

/// The IAM action reaching that service's create asks for, spelled as the service that refuses
/// it names an action in its OWN refusal.
///
/// The case is the refusing service's wording rather than part of the decision — IAM compares
/// action names case-insensitively — and the workspace states it once
/// (`::service_common::iam_authorization::reported_action`), so this suite asks the same
/// question the gate answers rather than spelling a third copy of it.
fn reported_action(owner: Owner) -> String {
    ::service_common::iam_authorization::reported_action(
        &::service_common::iam_authorization::iam_action(owner.namespace(), owner.created_by()),
    )
}

/// The name a row's resources are created under, and the identifier a policy statement — and an
/// authorization refusal — names that resource by.
struct Named {
    stack: String,
    resource: String,
    /// What the deployment's calls about the resource are authorized against: the ARN the
    /// resource carries, which is what a statement names it by.
    target: String,
}

impl Named {
    fn of(owner: Owner, stack: &str) -> Self {
        let resource = format!("{stack}-resource");
        let target = match owner {
            Owner::Topic => format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{resource}"),
            // A Parameter Store parameter's own identifier is its NAME, so what a statement
            // scoped to it names is the ARN of a parameter of that name
            // (`arn:<partition>:ssm:<region>:<account>:parameter/<name>`, the form AWS
            // documents for a Systems Manager parameter).
            Owner::Parameter => format!("arn:aws:ssm:{REGION}:{ACCOUNT_ID}:parameter/{resource}"),
        };
        Self {
            stack: stack.to_string(),
            resource,
            target,
        }
    }
}

/// The `Resources` section a row deploys: the one resource its owner names.
fn declared(owner: Owner, named: &Named) -> Value {
    match owner {
        Owner::Parameter => json!({"Param": parameter(&named.resource, "made-by-the-caller")}),
        Owner::Topic => json!({
            "Topic": {
                "Type": "AWS::SNS::Topic",
                "Properties": {"TopicName": named.resource},
            },
        }),
    }
}

/// One statement of an identity policy, as IAM's own grammar writes it.
fn statement(effect: &str, action: &str, resource: &str) -> Value {
    json!({"Effect": effect, "Action": action, "Resource": resource})
}

/// What one row expects of the deployment, and — where the caller may not make the call — which
/// of IAM's rules withheld it, as the refusal's own reason clause.
enum Expected {
    Created,
    RefusedForWantOfAGrant,
    RefusedByAnExplicitDeny,
}

/// One row: what the caller's one inline policy says, which resource the stack declares, whether
/// the request hands the stack a service role, and whether the deployment is carried out.
struct Row {
    case: &'static str,
    user: &'static str,
    stack: &'static str,
    /// The statements of the caller's inline policy beside the CloudFormation grant every row
    /// needs to reach the operation at all.
    statements: fn(&Named) -> Value,
    /// Whether the request hands the STACK a service role (`RoleARN`), which is what decides
    /// WHOSE credentials the deployment's calls are made with.
    service_role: bool,
    owner: Owner,
    expected: Expected,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the called service's own create action, named in full",
            user: "caller-grant-named",
            stack: "caller-grant-named",
            statements: |_| json!([statement("Allow", "ssm:PutParameter", "*")]),
            service_role: false,
            owner: Owner::Parameter,
            expected: Expected::Created,
        },
        Row {
            case: "a wildcard over every action of the called service",
            user: "caller-grant-wildcard",
            stack: "caller-grant-wildcard",
            statements: |_| json!([statement("Allow", "sns:*", "*")]),
            service_role: false,
            owner: Owner::Topic,
            expected: Expected::Created,
        },
        // The least-privilege shape the rule exists for: a caller holding every action of the
        // service but the one the create invokes cannot make that call.
        Row {
            case: "another action of the called service",
            user: "caller-grant-other-action",
            stack: "caller-grant-other-action",
            statements: |_| json!([statement("Allow", "ssm:DeleteParameter", "*")]),
            service_role: false,
            owner: Owner::Parameter,
            expected: Expected::RefusedForWantOfAGrant,
        },
        Row {
            case: "the create action scoped to another resource of the called service",
            user: "caller-grant-other-resource",
            stack: "caller-grant-other-resource",
            statements: |_| {
                json!([statement(
                    "Allow",
                    "sns:CreateTopic",
                    &format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:someone-else"),
                )])
            },
            service_role: false,
            owner: Owner::Topic,
            expected: Expected::RefusedForWantOfAGrant,
        },
        Row {
            case: "every action of another service and none of the called one",
            user: "caller-grant-elsewhere",
            stack: "caller-grant-elsewhere",
            statements: |_| json!([statement("Allow", "ssm:*", "*")]),
            service_role: false,
            owner: Owner::Topic,
            expected: Expected::RefusedForWantOfAGrant,
        },
        // "An explicit deny in any policy overrides any allows", whatever else the caller may do
        // in that service.
        Row {
            case: "everything allowed beside an explicit deny of the create",
            user: "caller-grant-denied",
            stack: "caller-grant-denied",
            statements: |named| {
                json!([
                    statement("Allow", "*", "*"),
                    statement("Deny", "ssm:PutParameter", &named.target),
                ])
            },
            service_role: false,
            owner: Owner::Parameter,
            expected: Expected::RefusedByAnExplicitDeny,
        },
        // The seam: the same caller holding nothing of the called service, with a SERVICE ROLE
        // handed to the stack. The deployment's calls are then that role's rather than the
        // caller's, so the caller's own documents decide none of them.
        Row {
            case: "a caller holding nothing of the called service, under a stack service role",
            user: "caller-grant-servicerole",
            stack: "caller-grant-servicerole",
            statements: |_| json!([statement("Allow", "ssm:*", "*")]),
            service_role: true,
            owner: Owner::Topic,
            expected: Expected::Created,
        },
    ]
}

/// Create an IAM user carrying the row's statements beside the CloudFormation grant every row
/// needs to reach the operation at all, and answer the access key id its requests sign with.
async fn caller_for(router: &MultiServiceRouter, row: &Row, named: &Named) -> String {
    let statements: Vec<Value> =
        std::iter::once(statement("Allow", "cloudformation:*", "*"))
            .chain((row.statements)(named).as_array().cloned().unwrap_or_default())
            .collect();
    let document = json!({"Version": "2012-10-17", "Statement": statements}).to_string();
    user_with_inline_policy(router, row.case, row.user, "what-the-caller-may-do", &document).await
}

/// The ARN IAM holds a user of this account under, which is what a refusal of its call names it
/// by.
fn user_arn(user_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:user/{user_name}")
}

/// Create the role one row hands the stack, trusting CloudFormation to assume it.
async fn create_the_service_role(router: &MultiServiceRouter, case: &str) -> String {
    let (http_status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", SERVICE_ROLE),
            ("AssumeRolePolicyDocument", &trusts_cloudformation()),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected CreateRole({SERVICE_ROLE}) to succeed, got {http_status} \
         {created}"
    );
    xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole reported no Arn: {created}"))
}

/// Poll `stack` until it reports `wanted`, or [`DEADLINE`] passes.
async fn until_status(router: &MultiServiceRouter, case: &str, stack: &str, wanted: &str) {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let (http_status, body) =
            call(router, "DescribeStacks", &[member("StackName", stack)]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: DescribeStacks({stack:?}) must answer 200, got {http_status} {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if reported == wanted {
            return;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: expected {stack:?} to reach {wanted}, last reported {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every `ResourceStatusReason` the stack recorded for `logical_id` at `CREATE_FAILED`, looked
/// for again until at least one is there or [`DEADLINE`] passes.
async fn until_failure_reasons(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> Vec<String> {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let (http_status, body) =
            call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: DescribeStackEvents({stack:?}) must answer 200, got {http_status} \
             {body}"
        );
        let recorded: Vec<String> = xml_blocks(&body, "member")
            .iter()
            .filter(|entry| {
                xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id)
                    && xml_text(entry, "ResourceStatus").as_deref()
                        == Some(resource_status::CREATE_FAILED)
            })
            .map(|entry| xml_text(entry, "ResourceStatusReason").unwrap_or_default())
            .collect();
        if !recorded.is_empty() {
            return recorded;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: expected a {} event for {logical_id:?} of {stack:?}",
            resource_status::CREATE_FAILED
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Whether the owning service holds the resource `named` addresses right now, read through that
/// service's OWN wire — which is what says whether a call was made at all.
async fn owning_service_holds(
    router: &MultiServiceRouter,
    case: &str,
    owner: Owner,
    named: &Named,
) -> bool {
    match owner {
        Owner::Parameter => parameter_value(router, &named.resource).await.is_some(),
        Owner::Topic => {
            let (http_status, body) =
                sns(router, "GetTopicAttributes", &[("TopicArn", &named.target)]).await;
            assert!(
                matches!(http_status, 200 | 400 | 404),
                "case {case:?}: GetTopicAttributes answered {http_status} {body}"
            );
            http_status == 200
        }
    }
}

/// RULE: a deployment carried out with the caller's own credentials makes only the calls that
/// caller is permitted; one it is not permitted fails that resource with the owning service's
/// own authorization answer, and the service holds nothing.
#[tokio::test]
async fn a_deployment_call_the_caller_may_not_make_fails_that_resource() {
    for row in rows() {
        let case = row.case;
        let (router, _engine) = router_with_engine();
        let named = Named::of(row.owner, row.stack);
        let access_key_id = caller_for(&router, &row, &named).await;
        // `DisableRollback` keeps the failed resource's stack at `CREATE_FAILED`, so what the
        // deployment did is read off the stack it left rather than off a rollback.
        let mut members = vec![
            member("StackName", &named.stack),
            member("TemplateBody", &template(declared(row.owner, &named))),
            member("DisableRollback", "true"),
        ];
        if row.service_role {
            let role_arn = create_the_service_role(&router, case).await;
            members.push(member("RoleARN", &role_arn));
        }
        let (http_status, created) =
            call_as(&router, &access_key_id, "CreateStack", &members).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: CreateStack must be accepted — the decision is the RESOURCE's, not \
             the request's — got {http_status} {created}"
        );

        let (wanted, refused) = match row.expected {
            Expected::Created => (status::CREATE_COMPLETE, None),
            Expected::RefusedForWantOfAGrant => (
                status::CREATE_FAILED,
                Some(
                    ::service_common::iam_authorization::no_identity_policy_allows(
                        &reported_action(row.owner),
                    ),
                ),
            ),
            Expected::RefusedByAnExplicitDeny => (
                status::CREATE_FAILED,
                Some(
                    ::service_common::iam_authorization::IDENTITY_POLICY_EXPLICIT_DENY.to_string(),
                ),
            ),
        };
        until_status(&router, case, &named.stack, wanted).await;

        // What the owning service really holds, which is the point of the rule: a call that was
        // not made created nothing, and one that was made created the resource.
        let held = owning_service_holds(&router, case, row.owner, &named).await;
        assert_eq!(
            held,
            refused.is_none(),
            "case {case:?}: expected the owning service to hold {:?} = {}, got {held}",
            named.resource,
            refused.is_none()
        );

        let Some(clause) = refused else {
            continue;
        };
        let logical_id = row.owner.logical_id();
        let recorded = until_failure_reasons(&router, case, &named.stack, logical_id).await;
        assert_eq!(
            recorded.len(),
            1,
            "case {case:?}: expected exactly one {} event for {logical_id:?}, got {recorded:?}",
            resource_status::CREATE_FAILED
        );
        let (quoted, _, class) = envelope_parts(case, &recorded[0]);
        assert_eq!(
            class, ACCESS_DENIED_CLASS,
            "case {case:?}: expected the failure classified as a call that was not permitted, \
             got {class:?} out of {:?}",
            recorded[0]
        );
        // The refusal is QUOTED as the resource's provider reported it, and a provider that
        // frames an answer in wording of its own carries that sentence inside its frame — so
        // what every row is held to is that the sentence is there, exactly as AWS words it.
        let sentence = ::service_common::iam_authorization::not_authorized_by_identity_policy(
            &user_arn(row.user),
            &reported_action(row.owner),
            &named.target,
            &clause,
        );
        assert!(
            quoted.contains(&sentence),
            "case {case:?}: expected the refusal {sentence:?} in the reason, got {quoted:?}"
        );
    }
}
