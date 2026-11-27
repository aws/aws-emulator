// SPDX-License-Identifier: CC0-1.0

//! The PRE-DEPLOYMENT validation a stack operation's resources are held to against what the
//! STRUCTURES INSIDE their properties require, end to end over the awsQuery wire.
//!
//! RULE: CloudFormation validates each declared resource against its resource type's schema
//! before provisioning anything, and a property type is a structure with required members of
//! its own — so a declaration that STATES such a structure without a member that structure's
//! contract marks required fails the operation before any resource is created: the stack
//! records a `PROPERTY_VALIDATION` validation result — `FAILED`, handled by failing, reading
//! `Required property [<member>] not found` — at the JSON pointer of the structure that lacks
//! the member, and nothing is asked of the service that owns the resource. A declaration whose
//! structures each carry what they require is deployed as written, and no result of this
//! validation is recorded for it.
//!
//! The rows differ along what the rule generalises over: how DEEP the structure sits (an
//! element of a list-valued property, a structure inside such an element, one three levels
//! down, and a property that is a single structure with no index at all), WHICH element of the
//! list it sits in, WHICH member of a structure requiring several is the missing one, and which
//! resource type the declaration is of (two services). The catalogue the stage judges against
//! is `state::property_validation::nested_required_members::NESTED_REQUIRED_MEMBERS`, whose
//! rows are each property type's "Required: Yes" members; the EventBridge Scheduler rows and
//! the EventBridge target members are ones the report that led here did not cite.

use crate::harness::{
    call, create_stack, member, router, stack_own_events, wait_for_the_transition, xml_blocks,
    xml_text,
};

/// The validation CloudFormation names for the check that holds a declaration's properties to
/// its type's contract. Source: `state::property_validation::PROPERTY_VALIDATION`.
const PROPERTY_VALIDATION: &str = "PROPERTY_VALIDATION";

/// The `EventType` a validation result is reported under ("Validation errors - Failures from
/// CloudFormation Early Validations").
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// One row: a template, and the `(member, pointer)` pairs the validation must report in order
/// — empty for a template whose nested structures each carry every member they require.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The logical id of the resource the validation is about.
    logical_id: &'static str,
    template: &'static str,
    missing: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "ECS parameters inside a target, without the task definition to run",
        stack: "nested-ecs-task-definition",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":[{"Id":"ecs","Arn":"arn:aws:ecs:us-east-1:000000000000:cluster/c",
                        "EcsParameters":{"TaskCount":1}}]}}}}"#,
        missing: &[(
            "TaskDefinitionArn",
            "/Resources/Rule/Properties/Targets/0/EcsParameters",
        )],
    },
    Row {
        case: "a target whose batch parameters state neither member they require",
        stack: "nested-batch-parameters",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":[{"Id":"batch","Arn":"arn:aws:batch:us-east-1:000000000000:job-queue/q",
                        "BatchParameters":{}}]}}}}"#,
        missing: &[
            (
                "JobDefinition",
                "/Resources/Rule/Properties/Targets/0/BatchParameters",
            ),
            (
                "JobName",
                "/Resources/Rule/Properties/Targets/0/BatchParameters",
            ),
        ],
    },
    Row {
        case: "the SECOND target of three is the one missing the id it is addressed by",
        stack: "nested-target-id",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":[{"Id":"first","Arn":"arn:aws:sqs:us-east-1:000000000000:one"},
                       {"Arn":"arn:aws:sqs:us-east-1:000000000000:two"},
                       {"Id":"third","Arn":"arn:aws:sqs:us-east-1:000000000000:three"}]}}}}"#,
        missing: &[("Id", "/Resources/Rule/Properties/Targets/1")],
    },
    Row {
        case: "a target stating neither the ARN it invokes nor an id",
        stack: "nested-target-arn",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":[{"Input":"{}"}]}}}}"#,
        missing: &[
            ("Arn", "/Resources/Rule/Properties/Targets/0"),
            ("Id", "/Resources/Rule/Properties/Targets/0"),
        ],
    },
    Row {
        case: "an awsvpc configuration three levels down, without its subnets",
        stack: "nested-awsvpc-subnets",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":[{"Id":"ecs","Arn":"arn:aws:ecs:us-east-1:000000000000:cluster/c",
                        "EcsParameters":{
                          "TaskDefinitionArn":"arn:aws:ecs:us-east-1:000000000000:task-definition/t:1",
                          "LaunchType":"FARGATE",
                          "NetworkConfiguration":{"AwsVpcConfiguration":{
                            "SecurityGroups":["sg-0123456789abcdef0"]}}}}]}}}}"#,
        missing: &[(
            "Subnets",
            "/Resources/Rule/Properties/Targets/0/EcsParameters/NetworkConfiguration/AwsVpcConfiguration",
        )],
    },
    Row {
        case: "an input transformer without the template it produces",
        stack: "nested-input-template",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":[{"Id":"q","Arn":"arn:aws:sqs:us-east-1:000000000000:q",
                        "InputTransformer":{"InputPathsMap":{"state":"$.detail.state"}}}]}}}}"#,
        missing: &[(
            "InputTemplate",
            "/Resources/Rule/Properties/Targets/0/InputTransformer",
        )],
    },
    Row {
        case: "the second run-command criterion of a target, without the values it selects by",
        stack: "nested-run-command-values",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":[{"Id":"ssm","Arn":"arn:aws:ssm:us-east-1:000000000000:document/AWS-RunShellScript",
                        "RunCommandParameters":{"RunCommandTargets":[
                          {"Key":"InstanceIds","Values":["i-0123456789abcdef0"]},
                          {"Key":"tag:Stage"}]}}]}}}}"#,
        missing: &[(
            "Values",
            "/Resources/Rule/Properties/Targets/0/RunCommandParameters/RunCommandTargets/1",
        )],
    },
    Row {
        case: "a schedule whose target states no role to invoke it as",
        stack: "nested-schedule-role",
        logical_id: "Schedule",
        template: r#"{"Resources":{"Schedule":{"Type":"AWS::Scheduler::Schedule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "FlexibleTimeWindow":{"Mode":"OFF"},
            "Target":{"Arn":"arn:aws:sqs:us-east-1:000000000000:q"}}}}}"#,
        missing: &[("RoleArn", "/Resources/Schedule/Properties/Target")],
    },
    Row {
        case: "a schedule whose flexible time window states no mode",
        stack: "nested-schedule-window-mode",
        logical_id: "Schedule",
        template: r#"{"Resources":{"Schedule":{"Type":"AWS::Scheduler::Schedule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "FlexibleTimeWindow":{"MaximumWindowInMinutes":15},
            "Target":{"Arn":"arn:aws:sqs:us-east-1:000000000000:q",
                      "RoleArn":"arn:aws:iam::000000000000:role/schedule"}}}}}"#,
        missing: &[("Mode", "/Resources/Schedule/Properties/FlexibleTimeWindow")],
    },
    Row {
        case: "a rule whose targets and nested structures each carry what they require",
        stack: "nested-rule-complete",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":[{"Id":"ecs","Arn":"arn:aws:ecs:us-east-1:000000000000:cluster/c",
                        "EcsParameters":{
                          "TaskDefinitionArn":"arn:aws:ecs:us-east-1:000000000000:task-definition/t:1",
                          "NetworkConfiguration":{"AwsVpcConfiguration":{
                            "Subnets":["subnet-0123456789abcdef0"]}}},
                        "InputTransformer":{"InputTemplate":"\"<state>\"",
                                            "InputPathsMap":{"state":"$.detail.state"}}}]}}}}"#,
        missing: &[],
    },
    Row {
        case: "a schedule whose target and time window each carry what they require",
        stack: "nested-schedule-complete",
        logical_id: "Schedule",
        template: r#"{"Resources":{"Schedule":{"Type":"AWS::Scheduler::Schedule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "FlexibleTimeWindow":{"Mode":"FLEXIBLE","MaximumWindowInMinutes":15},
            "Target":{"Arn":"arn:aws:sqs:us-east-1:000000000000:q",
                      "RoleArn":"arn:aws:iam::000000000000:role/schedule",
                      "EcsParameters":{
                        "TaskDefinitionArn":"arn:aws:ecs:us-east-1:000000000000:task-definition/t:1"}}}}}}"#,
        missing: &[],
    },
    Row {
        case: "a target list the template states as something that is no list of targets",
        stack: "nested-targets-not-a-list",
        logical_id: "Rule",
        template: r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
            "ScheduleExpression":"rate(1 hour)",
            "Targets":{"Id":"only"}}}}}"#,
        missing: &[],
    },
];

/// Every row: the declaration whose nested structure lacks a required member records the
/// validation result naming that member at that structure's pointer and nothing is provisioned
/// for the resource, and the declaration whose structures are complete records no result of
/// this validation at all.
#[tokio::test]
async fn a_nested_structure_missing_a_required_member_fails_the_operation() {
    let router = router();

    for row in ROWS {
        let case = row.case;
        let (code, created) = create_stack(&router, row.stack, row.template, &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: CreateStack must answer 200 — the request is valid and the stack \
             exists — got {code} {created}"
        );
        wait_for_the_transition().await;

        // The detail of the failure, where there is one: an event about the resource NOTHING
        // was done to, naming the validation, its status, how its failure was handled, the
        // member that is missing and the pointer of the structure it is missing from.
        let (code, events) =
            call(&router, "DescribeEvents", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the operation's events must be readable, got {code} {events}"
        );
        let reported: Vec<(String, String, String, String, String, String, String)> =
            xml_blocks(&events, "member")
                .iter()
                .filter(|event| xml_text(event, "ValidationName").is_some())
                .map(|event| {
                    (
                        xml_text(event, "LogicalResourceId").unwrap_or_default(),
                        xml_text(event, "EventType").unwrap_or_default(),
                        xml_text(event, "ValidationName").unwrap_or_default(),
                        xml_text(event, "ValidationStatus").unwrap_or_default(),
                        xml_text(event, "ValidationFailureMode").unwrap_or_default(),
                        xml_text(event, "ValidationStatusReason").unwrap_or_default(),
                        xml_text(event, "ValidationPath").unwrap_or_default(),
                    )
                })
                .collect();
        let expected: Vec<(String, String, String, String, String, String, String)> = row
            .missing
            .iter()
            .map(|(missing, path)| {
                (
                    row.logical_id.to_string(),
                    VALIDATION_ERROR.to_string(),
                    PROPERTY_VALIDATION.to_string(),
                    "FAILED".to_string(),
                    "FAIL".to_string(),
                    format!("Required property [{missing}] not found"),
                    (*path).to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the validation results {expected:?}, got {reported:?}"
        );

        // ...and nothing was asked of the service that owns the resource: a declaration this
        // stage refuses records no resource-level event at all, because the operation failed
        // before provisioning.
        let (code, resource_events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack's events must be readable, got {code} {resource_events}"
        );
        let about_the_resource = xml_blocks(&resource_events, "member")
            .iter()
            .filter(|event| {
                xml_text(event, "LogicalResourceId").as_deref() == Some(row.logical_id)
            })
            .count();
        assert_eq!(
            about_the_resource == 0,
            !row.missing.is_empty(),
            "case {case:?}: expected resource events only where the declaration passed the \
             validation, got {resource_events}"
        );

        // ...and the STACK itself counts the errors the stage found, which is one per missing
        // member: the summary a caller reads off the stack says how many there are and where
        // to read each one.
        if !row.missing.is_empty() {
            let reasons: Vec<String> = stack_own_events(&router, case, row.stack)
                .await
                .into_iter()
                .map(|event| event.reason)
                .collect();
            let summary = format!(
                "Validation failed with {} error(s). Call DescribeEvents to retrieve the full \
                 list of issues with resource and property details, resolve each error, then \
                 retry the operation.",
                row.missing.len()
            );
            assert!(
                reasons.contains(&summary),
                "case {case:?}: expected the stack to report {summary:?}, got {reasons:?}"
            );
        }
    }
}

/// A stack operation that turns the pre-deployment validations OFF deploys the declaration as
/// written: the same template the table above refuses records no validation result, because
/// the omission inside the nested structure is then the owning service's to report.
#[tokio::test]
async fn disabling_pre_deployment_validation_defers_the_missing_member() {
    let router = router();
    let case = "a target's ECS parameters missing the task definition, validations disabled";
    let template = r#"{"Resources":{"Rule":{"Type":"AWS::Events::Rule","Properties":{
        "ScheduleExpression":"rate(1 hour)",
        "Targets":[{"Id":"ecs","Arn":"arn:aws:ecs:us-east-1:000000000000:cluster/c",
                    "EcsParameters":{"TaskCount":1}}]}}}}"#;
    let (code, created) = create_stack(
        &router,
        "nested-ecs-deferred",
        template,
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {created}"
    );
    wait_for_the_transition().await;

    let (code, events) = call(
        &router,
        "DescribeEvents",
        &[member("StackName", "nested-ecs-deferred")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the operation's events must be readable, got {code} {events}"
    );
    assert!(
        !events.contains(PROPERTY_VALIDATION),
        "case {case:?}: expected no pre-deployment validation result, got {events}"
    );
}
