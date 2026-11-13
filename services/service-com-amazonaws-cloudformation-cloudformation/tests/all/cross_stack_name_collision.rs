// SPDX-License-Identifier: CC0-1.0

//! A physical resource name ANOTHER live stack of the Region already manages, end to end over
//! the awsQuery wire.
//!
//! RULE: a create the service that owns the type refuses because the name is already in use is
//! reported in CLOUDFORMATION's own words when a live stack of the same account and Region is
//! what manages the resource holding that name: the resource's `CREATE_FAILED` event carries
//! `<physical-name> already exists in stack <owning-stack-ARN>`, with no resource-handler
//! wrapper around it, because WHICH STACK holds the name is the actionable half of the failure
//! and the only half the owning service cannot report. A name no stack of that Region manages
//! — one a client took outside any stack, one only another Region's stacks hold — is nothing
//! CloudFormation can add to, so that service's own report stands: such a create reaches that
//! service only with the pre-deployment validations OFF, since with them on the existence
//! check stops it before anything is asked (`state::early_validation`), so the rows about it
//! state `DisableValidation`.
//!
//! The rows differ along what the rule generalises over: the resource type whose namespace the
//! name is in; who holds the name (another stack of the Region, a client outside any stack,
//! another Region's stack, nobody); and which call deploys the template — `CreateStack`,
//! `UpdateStack` or `ExecuteChangeSet`, since the record CloudFormation keeps is the same one
//! whichever call asks it.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{create_change_set, execute_change_set, settle_the_store};
use crate::dynamic_references::put_parameter;
use crate::harness::{call, call_in, create_stack, member, router_with_engine, xml_text};
use crate::rds_stack_resources::physical_id;
use crate::stack_lifecycle::{
    acknowledging_iam, describe_stack, resource_event_reason, resource_event_statuses,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status, status,
};

/// Another Region, for the row whose name is held only there: a physical name is managed per
/// Region, so a stack of another Region's holding one says nothing about this Region.
const OTHER_REGION: &str = "us-west-2";

/// The logical id every deployed template declares its one resource under.
const LOGICAL_ID: &str = "Thing";

/// The wrapper CloudFormation puts around a refusal the service that OWNS the type answered,
/// which is what a reason reported by that service rather than by CloudFormation opens with.
const HANDLER_WRAPPER: &str = "Resource handler returned message:";

/// A template declaring one `AWS::SSM::Parameter` under the name it is given. The type's
/// required properties are its `Type` and its `Value` (source: the CloudFormation resource
/// reference for `AWS::SSM::Parameter`).
fn parameter_template(name: &str, value: &str) -> String {
    json!({"Resources": {"Thing": {
        "Type": "AWS::SSM::Parameter",
        "Properties": {"Name": name, "Type": "String", "Value": value}
    }}})
    .to_string()
}

/// A template declaring one `AWS::IAM::Role` under the name it is given — a type whose
/// namespace is the ACCOUNT's rather than a Region's.
fn role_template(name: &str) -> String {
    json!({"Resources": {"Thing": {
        "Type": "AWS::IAM::Role",
        "Properties": {
            "RoleName": name,
            "AssumeRolePolicyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "glue.amazonaws.com"},
                    "Action": "sts:AssumeRole"
                }]
            }
        }
    }}})
    .to_string()
}

/// What one row expects of the deployment that would take the name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expected {
    /// CLOUDFORMATION fails the resource itself, with this exact reason.
    CloudFormations(String),
    /// Nothing of CloudFormation's stops the create: the owning service is asked, and its own
    /// refusal — wrapped as a handler's — is what the resource reports.
    TheOwningServices,
    /// Nothing in this Region holds the name: the stack completes.
    Deployed,
}

/// Assert what the deployment of `stack` came to, and how its one resource's failure is worded.
async fn assert_outcome(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    rolled_back_to: &str,
    expected: &Expected,
) {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
    );
    let reached = xml_text(&body, "StackStatus").unwrap_or_default();
    let wanted = match expected {
        Expected::Deployed => status::CREATE_COMPLETE,
        _ => rolled_back_to,
    };
    assert_eq!(
        reached, wanted,
        "case {case:?}: expected the stack to settle {wanted}, got {reached} ({body})"
    );
    if *expected == Expected::Deployed {
        return;
    }
    let on_the_resource =
        resource_event_reason(router, stack, LOGICAL_ID, resource_status::CREATE_FAILED).await;
    match expected {
        Expected::CloudFormations(reason) => assert_eq!(
            &on_the_resource, reason,
            "case {case:?}: expected CloudFormation's own collision reason {reason:?}, got \
             {on_the_resource:?}"
        ),
        Expected::TheOwningServices => assert!(
            on_the_resource.starts_with(HANDLER_WRAPPER)
                && !on_the_resource.contains("already exists in stack"),
            "case {case:?}: expected the owning service's own refusal, got {on_the_resource:?}"
        ),
        Expected::Deployed => unreachable!("a deployed row returns above"),
    }
}

/// Assert that the stack's TEARDOWN of the failed deployment disposed of the resource whose
/// create failed, exactly as it disposes of every other record it held.
async fn assert_disposed(router: &MultiServiceRouter, case: &str, stack: &str) {
    let statuses = resource_event_statuses(router, case, stack, LOGICAL_ID).await;
    assert_eq!(
        statuses.first().map(String::as_str),
        Some(resource_status::DELETE_COMPLETE),
        "case {case:?}: expected the rollback's own disposal to be the resource's last \
         event, got {statuses:?}"
    );
    assert!(
        statuses.contains(&resource_status::DELETE_IN_PROGRESS.to_string())
            && statuses.contains(&resource_status::CREATE_FAILED.to_string()),
        "case {case:?}: expected the create's failure and the delete that followed it, got \
         {statuses:?}"
    );
}

/// RULE: a `CreateStack` whose resource would take a physical name another live stack of the
/// Region manages fails that resource in CloudFormation's own words, naming the stack that
/// holds the name; a name held anywhere else is left to the service that owns the type.
#[tokio::test]
async fn a_name_another_stack_of_the_region_manages_fails_the_create_in_cloudformations_words() {
    let (router, engine) = router_with_engine();

    // The stack that owns the names: one name in a Region's namespace (a parameter), one in
    // the ACCOUNT's (a role).
    let owner = json!({"Resources": {
        "Param": {
            "Type": "AWS::SSM::Parameter",
            "Properties": {"Name": "/owned/param", "Type": "String", "Value": "owned"}
        },
        "Role": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": "owned-role",
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "glue.amazonaws.com"},
                        "Action": "sts:AssumeRole"
                    }]
                }
            }
        }
    }})
    .to_string();
    let (code, body) = create_stack(&router, "name-owner", &owner, &acknowledging_iam(&[])).await;
    assert_eq!(
        code, 200,
        "the owning stack must be created first, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_stack(&router, "name-owner").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the owning stack must complete, got {body}"
    );
    let owner_id = xml_text(&body, "StackId").expect("the owning stack's id");

    // A name a CLIENT holds, written through Systems Manager's own wire, so no stack manages
    // it...
    put_parameter(
        &router,
        "a client's own parameter",
        "/unmanaged/param",
        "String",
        "live",
    )
    .await;
    // ...and a ROLE another REGION's stack manages: the role name is the ACCOUNT's namespace
    // while the record of who manages it is the Region's.
    let (code, body) = call_in(
        &router,
        OTHER_REGION,
        "CreateStack",
        &acknowledging_iam(&[
            member("StackName", "role-owner"),
            member("TemplateBody", &role_template("elsewhere-role")),
        ]),
    )
    .await;
    assert_eq!(
        code, 200,
        "the other Region's stack must be created, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;

    /// `(case, stack, template, the extra request members, expectation)`.
    type Row = (
        &'static str,
        &'static str,
        String,
        Vec<(String, String)>,
        Expected,
    );
    let rows: Vec<Row> = vec![
        (
            "a parameter name another stack of the Region manages",
            "collide-param",
            parameter_template("/owned/param", "mine"),
            Vec::new(),
            Expected::CloudFormations(format!("/owned/param already exists in stack {owner_id}")),
        ),
        (
            "a role name another stack of the Region manages: the ACCOUNT's own namespace",
            "collide-role",
            role_template("owned-role"),
            acknowledging_iam(&[]),
            Expected::CloudFormations(format!("owned-role already exists in stack {owner_id}")),
        ),
        (
            "a parameter name a client holds outside any stack",
            "collide-unmanaged",
            parameter_template("/unmanaged/param", "mine"),
            vec![member("DisableValidation", "true")],
            Expected::TheOwningServices,
        ),
        (
            "a role name only another Region's stack manages",
            "collide-elsewhere",
            role_template("elsewhere-role"),
            acknowledging_iam(&[member("DisableValidation", "true")]),
            Expected::TheOwningServices,
        ),
        (
            "a parameter name nobody holds",
            "collide-free",
            parameter_template("/free/param", "mine"),
            Vec::new(),
            Expected::Deployed,
        ),
    ];

    for (case, stack, template, extra, expected) in &rows {
        let (code, body) = create_stack(&router, stack, template, extra).await;
        assert_eq!(
            code, 200,
            "case {case:?}: CreateStack must answer 200 — the creation starts, and the stack \
             reports the outcome — got {code} {body}"
        );
        // Once for the create's own conclusion, once for the unwind it owes.
        settle_the_store(&router, &engine).await;
        settle_the_store(&router, &engine).await;
        assert_outcome(&router, case, stack, status::ROLLBACK_COMPLETE, expected).await;
        if *expected != Expected::Deployed {
            // The rollback disposes of the resource whose create failed as it disposes of
            // every record it held: nothing was created for it, so nothing is asked of the
            // owning service and the delete completes at once.
            assert_disposed(&router, case, stack).await;
            // The stack-level reason stays the summary of which logical resource failed; the
            // collision itself is reported on that resource's event.
            let (_, body) = describe_stack(&router, stack).await;
            let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
            assert_eq!(
                reason,
                resource_failure_summary(failed_action::CREATE, [LOGICAL_ID]),
                "case {case:?}: expected the stack's reason to be the summary of what failed, \
                 got {reason:?}"
            );
        }
    }

    // Nothing of the owning stack was touched: it still holds its names, and the value its
    // parameter carries is the one IT deployed — the refused create never reached Systems
    // Manager.
    let (_, body) = describe_stack(&router, "name-owner").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the owning stack must be untouched by the refused deployments, got {body}"
    );
    let (code, parameter) = crate::dynamic_references::ssm(
        &router,
        "GetParameter",
        &json!({"Name": "/owned/param"}),
    )
    .await;
    assert_eq!(
        (code, parameter.pointer("/Parameter/Value").cloned()),
        (200, Some(json!("owned"))),
        "the owning stack's parameter must still carry its own value, got {parameter}"
    );
}

/// RULE: the same record answers the same way whichever call deploys the template — an
/// `UpdateStack` that adds a resource under a name another stack manages, and a change set
/// executed against a stack, both fail that resource with CloudFormation's own reason.
#[tokio::test]
async fn the_collision_is_cloudformations_whichever_call_deploys_the_template() {
    let (router, engine) = router_with_engine();
    let (code, body) = create_stack(
        &router,
        "ops-owner",
        &parameter_template("/ops/owned", "owned"),
        &[],
    )
    .await;
    assert_eq!(
        code, 200,
        "the owning stack must be created first, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_stack(&router, "ops-owner").await;
    let owner_id = xml_text(&body, "StackId").expect("the owning stack's id");
    let expected =
        Expected::CloudFormations(format!("/ops/owned already exists in stack {owner_id}"));

    // The UPDATE path: a live stack whose new template declares the owned name as well.
    let keeps_its_own = json!({"Resources": {
        "Kept": {
            "Type": "AWS::SSM::Parameter",
            "Properties": {"Name": "/ops/own", "Type": "String", "Value": "mine"}
        }
    }})
    .to_string();
    let (code, body) = create_stack(&router, "ops-updated", &keeps_its_own, &[]).await;
    assert_eq!(code, 200, "the stack to update must be created, got {code} {body}");
    settle_the_store(&router, &engine).await;
    let adds_the_owned_name = json!({"Resources": {
        "Kept": {
            "Type": "AWS::SSM::Parameter",
            "Properties": {"Name": "/ops/own", "Type": "String", "Value": "mine"}
        },
        "Thing": {
            "Type": "AWS::SSM::Parameter",
            "Properties": {"Name": "/ops/owned", "Type": "String", "Value": "mine"}
        }
    }})
    .to_string();
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "ops-updated"),
            member("TemplateBody", &adds_the_owned_name),
        ],
    )
    .await;
    assert_eq!(code, 200, "the update must be accepted, got {code} {body}");
    settle_the_store(&router, &engine).await;
    settle_the_store(&router, &engine).await;
    assert_outcome(
        &router,
        "an UpdateStack that adds a resource under a name another stack manages",
        "ops-updated",
        status::UPDATE_ROLLBACK_COMPLETE,
        &expected,
    )
    .await;

    // The CHANGE SET path: the plan is created with the pre-deployment existence check off,
    // so what answers the collision is the deployment the execution drives.
    let (code, body) = create_change_set(
        &router,
        "ops-planned",
        "cs",
        &[
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &parameter_template("/ops/owned", "mine")),
            member("DisableValidation", "true"),
        ],
    )
    .await;
    assert_eq!(code, 200, "the change set must be created, got {code} {body}");
    settle_the_store(&router, &engine).await;
    let (code, body) = execute_change_set(&router, "cs", Some("ops-planned")).await;
    assert_eq!(code, 200, "the plan must be executable, got {code} {body}");
    settle_the_store(&router, &engine).await;
    settle_the_store(&router, &engine).await;
    assert_outcome(
        &router,
        "a change set executed against a stack",
        "ops-planned",
        status::ROLLBACK_COMPLETE,
        &expected,
    )
    .await;
    assert_disposed(&router, "a change set executed against a stack", "ops-planned").await;

    // Whichever call asked, the owning stack still holds the name in the Region.
    let (_, body) = describe_stack(&router, "ops-owner").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the owning stack must be untouched, got {body}"
    );
    assert_eq!(
        physical_id(&router, "ops-owner", LOGICAL_ID).await,
        "/ops/owned",
        "the owning stack must still report the name it manages"
    );
}
