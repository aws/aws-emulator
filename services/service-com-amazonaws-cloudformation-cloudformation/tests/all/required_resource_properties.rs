// SPDX-License-Identifier: CC0-1.0

//! The PRE-DEPLOYMENT validation a stack operation's resources are held to against what
//! their type REQUIRES, end to end over the awsQuery wire.
//!
//! RULE: CloudFormation validates each declared resource against its resource type's schema
//! before provisioning anything, so a declaration that does not carry a property that schema
//! marks required fails the operation before any resource is created: the stack records a
//! `PROPERTY_VALIDATION` validation result — `FAILED`, handled by failing, reading
//! `Required property [<member>] not found` at the resource's own
//! `/Resources/<logical id>/Properties` — and rolls back to `ROLLBACK_COMPLETE` with nothing
//! provisioned. A declaration that carries every required property is deployed as written.
//!
//! The rows differ along what the rule generalises over: WHICH resource type the declaration
//! is of (five services), WHICH of that type's required properties is the missing one (the
//! first, a later one, every one of them at once — which is one error per missing property),
//! and the form the declaration takes (a
//! property document missing one member, and no `Properties` member at all). The catalogue
//! the stage judges against is `state::property_validation::REQUIRED_PROPERTIES`, whose rows
//! are each type's "Required: Yes" properties, and the row for `AWS::EC2::Subnet` is one the
//! report that led here did not cite.

use crate::harness::{call, create_stack, member, router, wait_for_the_transition};
use crate::stack_lifecycle::{describe_stack, xml_blocks, xml_text};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The validation CloudFormation names for the check that holds a declaration's properties
/// to its type's contract. Source: `state::property_validation::PROPERTY_VALIDATION`.
const PROPERTY_VALIDATION: &str = "PROPERTY_VALIDATION";

/// The `EventType` a validation result is reported under ("Validation errors - Failures from
/// CloudFormation Early Validations").
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// One row: a template, and the properties the validation must name as missing, in order —
/// empty for a template whose resources carry every property their types require.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The logical id of the resource the validation is about.
    logical_id: &'static str,
    template: &'static str,
    missing: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a pipe whose declaration omits the target it delivers to",
        stack: "required-pipe-target",
        logical_id: "ThePipe",
        template: r#"{"Resources":{"ThePipe":{"Type":"AWS::Pipes::Pipe","Properties":{
            "Name":"required-pipe-target-pipe",
            "Source":"arn:aws:sqs:us-east-1:000000000000:in",
            "RoleArn":"arn:aws:iam::000000000000:role/pipe-role"}}}}"#,
        missing: &["Target"],
    },
    Row {
        case: "a pipe whose declaration omits the role it acts as",
        stack: "required-pipe-role",
        logical_id: "ThePipe",
        template: r#"{"Resources":{"ThePipe":{"Type":"AWS::Pipes::Pipe","Properties":{
            "Source":"arn:aws:sqs:us-east-1:000000000000:in",
            "Target":"arn:aws:sqs:us-east-1:000000000000:out"}}}}"#,
        missing: &["RoleArn"],
    },
    Row {
        case: "a pipe with no Properties member at all is one error per required property",
        stack: "required-pipe-nothing",
        logical_id: "ThePipe",
        template: r#"{"Resources":{"ThePipe":{"Type":"AWS::Pipes::Pipe"}}}"#,
        missing: &["RoleArn", "Source", "Target"],
    },
    Row {
        case: "a parameter whose declaration omits its value",
        stack: "required-parameter-value",
        logical_id: "Setting",
        template: r#"{"Resources":{"Setting":{"Type":"AWS::SSM::Parameter","Properties":{
            "Type":"String"}}}}"#,
        missing: &["Value"],
    },
    Row {
        case: "a subscription whose declaration omits its protocol",
        stack: "required-subscription-protocol",
        logical_id: "Feed",
        template: r#"{"Resources":{"Feed":{"Type":"AWS::SNS::Subscription","Properties":{
            "TopicArn":"arn:aws:sns:us-east-1:000000000000:events",
            "Endpoint":"ops@example.com"}}}}"#,
        missing: &["Protocol"],
    },
    Row {
        case: "a subnet whose declaration omits the VPC it lives in",
        stack: "required-subnet-vpc",
        logical_id: "Private",
        template: r#"{"Resources":{"Private":{"Type":"AWS::EC2::Subnet","Properties":{
            "CidrBlock":"10.0.1.0/24"}}}}"#,
        missing: &["VpcId"],
    },
    Row {
        case: "a queue policy whose declaration omits the queues it is on",
        stack: "required-queue-policy",
        logical_id: "Access",
        template: r#"{"Resources":{"Access":{"Type":"AWS::SQS::QueuePolicy","Properties":{
            "PolicyDocument":{"Version":"2012-10-17","Statement":[]}}}}}"#,
        missing: &["Queues"],
    },
    Row {
        case: "a parameter carrying every property its type requires",
        stack: "required-parameter-complete",
        logical_id: "Setting",
        template: r#"{"Resources":{"Setting":{"Type":"AWS::SSM::Parameter","Properties":{
            "Name":"required-parameter-complete-setting","Type":"String","Value":"on"}}}}"#,
        missing: &[],
    },
];

/// Every row: the stack a declaration missing a required property starts ends
/// `ROLLBACK_COMPLETE` carrying the validation result, and the one whose declaration is
/// complete reaches `CREATE_COMPLETE`.
#[tokio::test]
async fn a_declaration_missing_a_required_property_rolls_the_stack_back() {
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

        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack must be readable, got {code} {described}"
        );
        let expected_status = match row.missing.is_empty() {
            false => status::ROLLBACK_COMPLETE,
            true => status::CREATE_COMPLETE,
        };
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(expected_status),
            "case {case:?}: expected the stack to reach {expected_status}, got {described}"
        );

        // The detail of the failure, where there is one: an event about the resource NOTHING
        // was done to, naming the validation, its status, how its failure was handled, the
        // property that is missing and the path the validation was applied at.
        let (code, events) = call(
            &router,
            "DescribeEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the operation's events must be readable, got {code} {events}"
        );
        let validations: Vec<(String, String, String, String, String, String)> =
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
                    )
                })
                .collect();
        let expected_validations: Vec<(String, String, String, String, String, String)> = row
            .missing
            .iter()
            .map(|property| {
                (
                    row.logical_id.to_string(),
                    VALIDATION_ERROR.to_string(),
                    PROPERTY_VALIDATION.to_string(),
                    "FAILED".to_string(),
                    "FAIL".to_string(),
                    format!("Required property [{property}] not found"),
                )
            })
            .collect();
        assert_eq!(
            validations, expected_validations,
            "case {case:?}: expected the validation results {expected_validations:?}, got \
             {validations:?}"
        );
        let paths: Vec<String> = xml_blocks(&events, "member")
            .iter()
            .filter(|event| xml_text(event, "ValidationName").is_some())
            .map(|event| xml_text(event, "ValidationPath").unwrap_or_default())
            .collect();
        let expected_paths: Vec<String> = row
            .missing
            .iter()
            .map(|_| format!("/Resources/{}/Properties", row.logical_id))
            .collect();
        assert_eq!(
            paths, expected_paths,
            "case {case:?}: expected the validation to be applied at {expected_paths:?}, got \
             {paths:?}"
        );

        // ...and no resource-level event was recorded for the resource: the operation failed
        // before provisioning, so nothing was asked of the service that owns it. The
        // declaration that passes the stage IS deployed, so its resource reports its own
        // transitions.
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
            about_the_resource > 0,
            row.missing.is_empty(),
            "case {case:?}: expected resource events for the resource only where the \
             declaration was deployed, got {resource_events}"
        );
    }
}

/// A stack operation that turns the pre-deployment validations OFF deploys the declaration
/// as written: the same template the first test rolls back reaches a terminal state with the
/// resource recorded, because the problem is then the owning service's to report.
#[tokio::test]
async fn disabling_pre_deployment_validation_defers_the_missing_property() {
    let router = router();
    let case = "a pipe missing its target, with the validations disabled";
    let template = r#"{"Resources":{"ThePipe":{"Type":"AWS::Pipes::Pipe","Properties":{
        "Name":"deferred-pipe","Source":"arn:aws:sqs:us-east-1:000000000000:in",
        "RoleArn":"arn:aws:iam::000000000000:role/pipe-role"}}}}"#;
    let (code, created) = create_stack(
        &router,
        "required-pipe-deferred",
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
        &[member("StackName", "required-pipe-deferred")],
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
