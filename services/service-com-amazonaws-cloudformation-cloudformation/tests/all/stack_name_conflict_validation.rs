// SPDX-License-Identifier: CC0-1.0

//! The PRE-DEPLOYMENT EXISTENCE check a stack operation's own resources are held to, end to
//! end over the awsQuery wire.
//!
//! RULE: CloudFormation runs its pre-deployment validations before provisioning anything, so
//! an operation that would CREATE a resource under a physical name the service that owns the
//! type ALREADY holds — and that no live stack of the account and Region manages — fails at
//! that stage: nothing is asked of any owning service, no resource of the stack reports an
//! event of its own, the resource that was already there is untouched, and the stack's own
//! failure event carries `DetailedStatus` `VALIDATION_FAILED` with CloudFormation's sentence
//! for the collision, whose fixed text is `Resource of type '…' with identifier '…' already
//! exists.` The detail is the validation result `DescribeEvents` reports: one
//! `VALIDATION_ERROR` event naming `NAME_CONFLICT_VALIDATION`, `FAILED`, handled by failing,
//! at the element of the template that declared the resource. `DisableValidation` turns the
//! stage off, and the same declaration then reaches the owning service, whose own refusal is
//! what that resource's event reports.
//!
//! The rows differ along what the rule generalises over: WHICH resource type the colliding
//! declaration is of (four owning services), the FORM of the identifier the collision is
//! about (a name the declaration states, and an ARN built from one), who holds the name (the
//! owning service, with nothing holding it as the control), and WHICH operation deploys the
//! template (`CreateStack` and `UpdateStack`) — plus the two boundaries: a name the stack
//! ITSELF manages is no collision, and a request that disables the validations defers the
//! conflict to provisioning.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::dynamic_references::{put_parameter, ssm};
use crate::logs_group_children_resources::{group_exists, logs};
use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, iam, member, router_with_engine, sns,
    xml_blocks, xml_text, ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

/// The logical id every template below declares the COLLIDING resource under.
const COLLIDING: &str = "Colliding";

/// ...and the one it declares a second, uncontested resource under: nothing is asked of any
/// owning service once the stage fails, so this one is not created either.
const SIBLING: &str = "Sibling";

/// The validation CloudFormation names for the check that a resource an operation would
/// create does not already exist. Source: `state::early_validation::NAME_CONFLICT_VALIDATION`.
const NAME_CONFLICT_VALIDATION: &str = "NAME_CONFLICT_VALIDATION";

/// The `EventType` a validation result is reported under, from the model's `EventType` enum
/// (`smithy_query(query="shape", shape="EventType")`).
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// `StackEvent$DetailedStatus` for an operation a pre-deployment validation failed, from the
/// model's `DetailedStatus` enum (`[CONFIGURATION_COMPLETE, VALIDATION_FAILED]`).
const VALIDATION_FAILED: &str = "VALIDATION_FAILED";

/// CloudFormation's own sentence for a resource it cannot create because one already exists
/// under the identifier the declaration asked for: fixed text around the declared type and
/// that identifier. Source: `state::early_validation::already_exists`.
fn already_exists(resource_type: &str, identifier: &str) -> String {
    format!("Resource of type '{resource_type}' with identifier '{identifier}' already exists.")
}

/// Who already holds the name a row's template declares.
#[derive(Debug, Clone, Copy)]
enum Holder {
    /// A log group CloudWatch Logs holds, created through its own wire.
    ALogGroup(&'static str),
    /// A role IAM holds.
    ARole(&'static str),
    /// A parameter Parameter Store holds.
    AParameter(&'static str),
    /// A topic Amazon SNS holds, whose identifier is the ARN built from its name.
    ATopic(&'static str),
    /// Nobody: the control row, whose declaration deploys.
    Nobody,
}

/// One row: the declaration that collides, and what the collision is about.
struct Row {
    case: &'static str,
    stack: &'static str,
    holder: Holder,
    resource_type: &'static str,
    /// The identifier the owning service knows the resource by, which is what the collision
    /// names: the stated name for a type identified by it, the ARN for one identified by that.
    identifier: String,
    /// The colliding resource's declaration, as the template carries it.
    declaration: Value,
    /// The log group name the uncontested sibling declares, which no row's holder holds.
    sibling: &'static str,
    /// What the request has to acknowledge or ask for beyond the template.
    extra: Vec<(String, String)>,
}

/// A trust policy IAM accepts: a policy document carrying a `Statement`.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "glue.amazonaws.com"},
            "Action": "sts:AssumeRole"
        }]
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a log group whose name CloudWatch Logs already holds",
            stack: "conflict-log-group",
            holder: Holder::ALogGroup("/aws/lambda/conflict-held-group"),
            resource_type: "AWS::Logs::LogGroup",
            identifier: "/aws/lambda/conflict-held-group".to_string(),
            declaration: json!({
                "Type": "AWS::Logs::LogGroup",
                "Properties": {
                    "LogGroupName": "/aws/lambda/conflict-held-group",
                    "RetentionInDays": 3
                }
            }),
            sibling: "/conflict/log-group-sibling",
            extra: Vec::new(),
        },
        Row {
            case: "a role whose name IAM already holds",
            stack: "conflict-role",
            holder: Holder::ARole("conflict-held-role"),
            resource_type: "AWS::IAM::Role",
            identifier: "conflict-held-role".to_string(),
            declaration: json!({
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "conflict-held-role",
                    "AssumeRolePolicyDocument": trust_policy()
                }
            }),
            sibling: "/conflict/role-sibling",
            extra: acknowledging_iam(&[]),
        },
        Row {
            case: "a parameter whose name Parameter Store already holds",
            stack: "conflict-parameter",
            holder: Holder::AParameter("/conflict/held-param"),
            resource_type: "AWS::SSM::Parameter",
            identifier: "/conflict/held-param".to_string(),
            declaration: json!({
                "Type": "AWS::SSM::Parameter",
                "Properties": {
                    "Name": "/conflict/held-param",
                    "Type": "String",
                    "Value": "the stack's"
                }
            }),
            sibling: "/conflict/parameter-sibling",
            extra: Vec::new(),
        },
        Row {
            // The identifier is the ARN the create produces from the stated name, which is
            // what Amazon SNS knows the topic by.
            case: "a topic whose name Amazon SNS already holds, identified by its ARN",
            stack: "conflict-topic",
            holder: Holder::ATopic("conflict-held-topic"),
            resource_type: "AWS::SNS::Topic",
            identifier: format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:conflict-held-topic"),
            declaration: json!({
                "Type": "AWS::SNS::Topic",
                "Properties": {"TopicName": "conflict-held-topic"}
            }),
            sibling: "/conflict/topic-sibling",
            extra: Vec::new(),
        },
        Row {
            case: "the control: a log group name nothing holds",
            stack: "conflict-free",
            holder: Holder::Nobody,
            resource_type: "AWS::Logs::LogGroup",
            identifier: "/conflict/free-group".to_string(),
            declaration: json!({
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": "/conflict/free-group"}
            }),
            sibling: "/conflict/free-sibling",
            extra: Vec::new(),
        },
    ]
}

/// The template a row deploys: the colliding declaration, and one uncontested log group
/// beside it.
fn template(declaration: &Value, sibling: &str) -> String {
    declaring(&[
        (COLLIDING, declaration.clone()),
        (
            SIBLING,
            json!({
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": sibling}
            }),
        ),
    ])
}

/// One template body declaring `resources`, each under the logical id it is paired with.
fn declaring(resources: &[(&str, Value)]) -> String {
    let mut declared = serde_json::Map::new();
    for (logical_id, declaration) in resources {
        declared.insert((*logical_id).to_string(), declaration.clone());
    }
    json!({"Resources": declared}).to_string()
}

/// Make the row's holder hold the name, through the owning service's OWN wire — so the name
/// is taken outside any stack, which is the case this stage is about.
async fn seed(router: &MultiServiceRouter, row: &Row) {
    match row.holder {
        Holder::ALogGroup(name) => {
            let (status_code, body) =
                logs(router, "CreateLogGroup", &json!({"logGroupName": name})).await;
            assert_eq!(
                status_code, 200,
                "case {:?}: the setup CreateLogGroup({name}) must answer 200, got \
                 {status_code} {body}",
                row.case
            );
        }
        Holder::ARole(name) => {
            let (status_code, body) = iam(
                router,
                "CreateRole",
                &[
                    ("RoleName", name),
                    ("AssumeRolePolicyDocument", &trust_policy().to_string()),
                ],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {:?}: the setup CreateRole({name}) must answer 200, got {status_code} \
                 {body}",
                row.case
            );
        }
        Holder::AParameter(name) => {
            put_parameter(router, row.case, name, "String", "a client's").await;
        }
        Holder::ATopic(name) => {
            let (status_code, body) = sns(router, "CreateTopic", &[("Name", name)]).await;
            assert_eq!(
                status_code, 200,
                "case {:?}: the setup CreateTopic({name}) must answer 200, got {status_code} \
                 {body}",
                row.case
            );
        }
        Holder::Nobody => {}
    }
}

/// Whether the owning service still holds what the row seeded — read back through that
/// service's own wire, which is the only place the answer lives.
async fn still_held(router: &MultiServiceRouter, row: &Row) -> bool {
    match row.holder {
        Holder::ALogGroup(name) => group_exists(router, name).await,
        Holder::ARole(name) => iam(router, "GetRole", &[("RoleName", name)]).await.0 == 200,
        Holder::AParameter(name) => {
            ssm(router, "GetParameter", &json!({"Name": name})).await.0 == 200
        }
        Holder::ATopic(name) => {
            let arn = format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}");
            sns(router, "GetTopicAttributes", &[("TopicArn", arn.as_str())])
                .await
                .0
                == 200
        }
        Holder::Nobody => true,
    }
}

/// `(LogicalResourceId, ResourceStatus, DetailedStatus, ResourceStatusReason)` for every
/// event `DescribeStackEvents` reports for `stack`, newest first.
async fn stack_events(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String, String, String)> {
    let (status_code, body) =
        call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the stack's events must be readable, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|event| {
            (
                xml_text(event, "LogicalResourceId").unwrap_or_default(),
                xml_text(event, "ResourceStatus").unwrap_or_default(),
                xml_text(event, "DetailedStatus").unwrap_or_default(),
                xml_text(event, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// The validation RESULTS `DescribeEvents` reports for `stack`: one tuple per event that
/// carries a `ValidationName`.
async fn validation_results(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String, String, String, String, String, String)> {
    let (status_code, body) = call(router, "DescribeEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the operation's events must be readable, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|event| xml_text(event, "ValidationName").is_some())
        .map(|event| {
            (
                xml_text(event, "LogicalResourceId").unwrap_or_default(),
                xml_text(event, "ResourceType").unwrap_or_default(),
                xml_text(event, "EventType").unwrap_or_default(),
                xml_text(event, "ValidationName").unwrap_or_default(),
                xml_text(event, "ValidationStatus").unwrap_or_default(),
                xml_text(event, "ValidationFailureMode").unwrap_or_default(),
                xml_text(event, "ValidationStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// Every row: a create whose declared name the owning service already holds fails at the
/// validation stage, and the control row deploys.
#[tokio::test]
async fn a_name_the_owning_service_holds_fails_the_create_before_provisioning() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = router_with_engine();
        seed(&router, &row).await;

        let (status_code, created) = create_stack(
            &router,
            row.stack,
            &template(&row.declaration, row.sibling),
            &row.extra,
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStack must answer 200 — the request is valid and the stack \
             exists — got {status_code} {created}"
        );
        // Once for the create's own conclusion, once for the unwind its failure owes.
        settle_the_store(&router, &engine).await;
        settle_the_store(&router, &engine).await;

        let collides = !matches!(row.holder, Holder::Nobody);
        let reason = already_exists(row.resource_type, &row.identifier);

        let (status_code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the stack must be readable, got {status_code} {described}"
        );
        let expected_status = if collides {
            status::ROLLBACK_COMPLETE
        } else {
            status::CREATE_COMPLETE
        };
        let settled = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            settled, expected_status,
            "case {case:?}: expected the stack to settle {expected_status}, got {settled:?} \
             ({described})"
        );
        if collides {
            let settled_reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
            assert_eq!(
                settled_reason, reason,
                "case {case:?}: expected the stack to report the collision {reason:?}, got \
                 {settled_reason:?}"
            );
        }

        // The stack's own failure event is the one report of the failure, and it carries the
        // detailed status the model gives a validation failure.
        let events = stack_events(&router, case, row.stack).await;
        let failed: Vec<&(String, String, String, String)> = events
            .iter()
            .filter(|(logical_id, event_status, _, _)| {
                logical_id == row.stack && event_status == resource_status::CREATE_FAILED
            })
            .collect();
        assert_eq!(
            failed.len(),
            usize::from(collides),
            "case {case:?}: expected one stack-level CREATE_FAILED for a failed validation \
             and none otherwise, got {events:?}"
        );
        if let Some((_, _, detailed_status, event_reason)) = failed.first() {
            assert_eq!(
                detailed_status, VALIDATION_FAILED,
                "case {case:?}: expected the failure event to report {VALIDATION_FAILED}, got \
                 {detailed_status:?} in {events:?}"
            );
            assert_eq!(
                event_reason, &reason,
                "case {case:?}: expected the failure event to carry {reason:?}, got \
                 {event_reason:?}"
            );
        }

        // Nothing was asked of any owning service: neither the colliding resource nor the
        // uncontested one beside it reports an event of its own.
        let about_the_resources: Vec<&(String, String, String, String)> = events
            .iter()
            .filter(|(logical_id, _, _, _)| logical_id == COLLIDING || logical_id == SIBLING)
            .collect();
        assert_eq!(
            about_the_resources.is_empty(),
            collides,
            "case {case:?}: expected resource events only where the template was deployed, \
             got {about_the_resources:?}"
        );

        // ...and the detail of the failure is the validation result `DescribeEvents` carries.
        let results = validation_results(&router, case, row.stack).await;
        let expected_results = if collides {
            vec![(
                COLLIDING.to_string(),
                row.resource_type.to_string(),
                VALIDATION_ERROR.to_string(),
                NAME_CONFLICT_VALIDATION.to_string(),
                "FAILED".to_string(),
                "FAIL".to_string(),
                reason.clone(),
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            results, expected_results,
            "case {case:?}: expected the validation results {expected_results:?}, got \
             {results:?}"
        );

        // The resource that was already there is untouched, and the uncontested sibling was
        // never created — except on the control row, where the whole template deployed.
        assert!(
            still_held(&router, &row).await,
            "case {case:?}: the resource that already existed must survive the failed create"
        );
        let sibling_there = group_exists(&router, row.sibling).await;
        assert_eq!(
            sibling_there, !collides,
            "case {case:?}: expected the uncontested sibling to exist only where the \
             deployment ran, got {sibling_there}"
        );
    }
}

/// The path into the template the validation was applied at, read on one row: the element of
/// the submitted document the resource was declared at.
#[tokio::test]
async fn the_validation_result_names_the_element_the_resource_was_declared_at() {
    let case = "a log group whose name CloudWatch Logs already holds";
    let (router, engine) = router_with_engine();
    let held = "/conflict/path-held-group";
    let (status_code, body) = logs(&router, "CreateLogGroup", &json!({"logGroupName": held})).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the setup CreateLogGroup must answer 200, got {status_code} {body}"
    );

    let declaration = json!({
        "Type": "AWS::Logs::LogGroup",
        "Properties": {"LogGroupName": held}
    });
    let (status_code, created) = create_stack(
        &router,
        "conflict-path",
        &template(&declaration, "/conflict/path-sibling"),
        &[],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    settle_the_store(&router, &engine).await;
    settle_the_store(&router, &engine).await;

    let (status_code, body) = call(
        &router,
        "DescribeEvents",
        &[member("StackName", "conflict-path")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the operation's events must be readable, got {status_code} {body}"
    );
    let paths: Vec<String> = xml_blocks(&body, "member")
        .iter()
        .filter(|event| xml_text(event, "ValidationName").is_some())
        .map(|event| xml_text(event, "ValidationPath").unwrap_or_default())
        .collect();
    let expected = vec![format!("/Resources/{COLLIDING}")];
    assert_eq!(
        paths, expected,
        "case {case:?}: expected the validation to be applied at {expected:?}, got {paths:?}"
    );
}

/// A request that turns the pre-deployment validations OFF deploys the declaration as
/// written: the owning service is asked, and ITS refusal is what the resource's own event
/// reports — the conflict deferred to provisioning rather than caught before it.
#[tokio::test]
async fn disabling_the_validations_defers_the_conflict_to_the_owning_service() {
    let case = "a held log group name, with the validations disabled";
    let (router, engine) = router_with_engine();
    let held = "/conflict/deferred-group";
    let (status_code, body) = logs(&router, "CreateLogGroup", &json!({"logGroupName": held})).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the setup CreateLogGroup must answer 200, got {status_code} {body}"
    );

    let declaration = json!({
        "Type": "AWS::Logs::LogGroup",
        "Properties": {"LogGroupName": held}
    });
    let (status_code, created) = create_stack(
        &router,
        "conflict-deferred",
        &template(&declaration, "/conflict/deferred-sibling"),
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    settle_the_store(&router, &engine).await;
    settle_the_store(&router, &engine).await;

    // The declaration reached CloudWatch Logs, which refused it: the failure is on the
    // RESOURCE's own event.
    let events = stack_events(&router, case, "conflict-deferred").await;
    let refused: Vec<&(String, String, String, String)> = events
        .iter()
        .filter(|(logical_id, event_status, _, _)| {
            logical_id == COLLIDING && event_status == resource_status::CREATE_FAILED
        })
        .collect();
    assert_eq!(
        refused.len(),
        1,
        "case {case:?}: expected the colliding resource to fail at provisioning, got {events:?}"
    );
    assert!(
        !refused[0].3.is_empty(),
        "case {case:?}: expected the owning service's refusal on that event, got {refused:?}"
    );
    // ...and no validation ran, so no validation result is reported.
    let results = validation_results(&router, case, "conflict-deferred").await;
    assert!(
        results.is_empty(),
        "case {case:?}: expected no validation result with the stage off, got {results:?}"
    );
    // The group that was already there is untouched.
    assert!(
        group_exists(&router, held).await,
        "case {case:?}: the group that already existed must survive"
    );
}

/// The rule is the OPERATION's, not the create's: an update that ADDS a resource under a held
/// name fails the same way, while a resource the stack ITSELF already manages is no collision
/// — an update that keeps it and changes it is carried out.
#[tokio::test]
async fn an_update_is_held_to_the_same_check_and_its_own_resources_are_no_collision() {
    let case = "an update that adds a resource under a held name";
    let (router, engine) = router_with_engine();
    let held = "/conflict/update-held-group";
    let managed = "/conflict/update-managed-group";
    let (status_code, body) = logs(&router, "CreateLogGroup", &json!({"logGroupName": held})).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the setup CreateLogGroup must answer 200, got {status_code} {body}"
    );

    let one_group = declaring(&[(
        SIBLING,
        json!({
            "Type": "AWS::Logs::LogGroup",
            "Properties": {"LogGroupName": managed}
        }),
    )]);
    let (status_code, created) = create_stack(&router, "conflict-update", &one_group, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    settle_the_store(&router, &engine).await;
    let (_, described) = describe_stack(&router, "conflict-update").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must complete before the update, got {described}"
    );

    // The update KEEPS the group the stack manages — its name is no collision, because that
    // resource exists BECAUSE the stack has it — and ADDS one under the held name.
    let adds_a_collision = declaring(&[
        (
            SIBLING,
            json!({
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": managed, "RetentionInDays": 5}
            }),
        ),
        (
            COLLIDING,
            json!({
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": held}
            }),
        ),
    ]);
    let (status_code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "conflict-update"),
            member("TemplateBody", &adds_a_collision),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: UpdateStack must answer 200, got {status_code} {body}"
    );
    settle_the_store(&router, &engine).await;
    settle_the_store(&router, &engine).await;

    let reason = already_exists("AWS::Logs::LogGroup", held);
    let events = stack_events(&router, case, "conflict-update").await;
    let failed: Vec<&(String, String, String, String)> = events
        .iter()
        .filter(|(logical_id, event_status, _, _)| {
            logical_id == "conflict-update" && event_status == resource_status::UPDATE_FAILED
        })
        .collect();
    assert_eq!(
        failed.len(),
        1,
        "case {case:?}: expected one stack-level UPDATE_FAILED, got {events:?}"
    );
    assert_eq!(
        (failed[0].2.as_str(), failed[0].3.as_str()),
        (VALIDATION_FAILED, reason.as_str()),
        "case {case:?}: expected the failure event to report {VALIDATION_FAILED} and \
         {reason:?}, got {failed:?}"
    );
    // Nothing was asked of CloudWatch Logs: the resource the update would have added reports
    // no event of its own, and the group that was already there is untouched.
    assert!(
        !events
            .iter()
            .any(|(logical_id, _, _, _)| logical_id == COLLIDING),
        "case {case:?}: expected no event about the resource nothing was asked for, got \
         {events:?}"
    );
    assert!(
        group_exists(&router, held).await,
        "case {case:?}: the group that already existed must survive the failed update"
    );
    // The validation result is the update's own detail, naming the same validation.
    let results = validation_results(&router, case, "conflict-update").await;
    let expected_results = vec![(
        COLLIDING.to_string(),
        "AWS::Logs::LogGroup".to_string(),
        VALIDATION_ERROR.to_string(),
        NAME_CONFLICT_VALIDATION.to_string(),
        "FAILED".to_string(),
        "FAIL".to_string(),
        reason.clone(),
    )];
    assert_eq!(
        results, expected_results,
        "case {case:?}: expected the validation results {expected_results:?}, got {results:?}"
    );

    // ...and the resource the stack manages is still there: the update that would have kept
    // it changed nothing in CloudWatch Logs either way.
    assert!(
        group_exists(&router, managed).await,
        "case {case:?}: the group the stack manages must still be there"
    );
}
