// SPDX-License-Identifier: CC0-1.0

//! What a `Stack` carries on the wire beside its outputs: the members CloudFormation
//! reports on every stack, the parameter values in force, the stack's recorded last
//! operation and the termination-protection flag.
//!
//! Every case drives the real awsQuery request path (form body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client sees.
//! The rules under test:
//!
//! * `NotificationARNs`, `Tags`, `RollbackConfiguration`, `DeploymentConfig` and
//!   `LastOperations` are reported on EVERY stack, as an empty element (or the documented
//!   default) where the stack carries nothing, while the members that report a stack's own
//!   declarations appear only when the stack has some.
//! * `Parameters` reports the values in force: what the request supplied, plus the
//!   template's own `Default` for a declared parameter the request passed no value for.
//! * `LastOperations` reports the stack operation that minted the id the call handed back,
//!   on `Stack` and on `StackSummary` alike.
//! * `EnableTerminationProtection` reports the flag `UpdateTerminationProtection` set.
//!
//! The evaluation of a template's `Outputs` has its own suite in `stack_outputs.rs`.

use super::harness::{
    call, create_stack, member, router, wait_for_the_transition, xml_text, xml_texts,
};
use super::stack_lifecycle::describe_stack;

/// Whether the response carries the named element at all, empty (`<Tags/>`) or not.
fn has_element(xml: &str, tag: &str) -> bool {
    xml.contains(&format!("<{tag}>")) || xml.contains(&format!("<{tag}/>"))
}

/// The members CloudFormation puts on every stack, whatever the request supplied.
///
/// One row per shape of create request: nothing optional at all, the collections
/// supplied, a deployment configuration stated explicitly, and rollback disabled through
/// the top-level flag — so the rule is asserted across the class rather than on one
/// recorded shape.
struct AlwaysPresentCase {
    case: &'static str,
    stack_name: &'static str,
    extra: Vec<(String, String)>,
    /// The `DeploymentConfig$Mode` the stack must report.
    mode: &'static str,
    /// The `DeploymentConfig$DisableRollback` the stack must report.
    disable_rollback: &'static str,
    notification_arns: Vec<String>,
    tag_keys: Vec<String>,
}

#[tokio::test]
async fn every_stack_reports_the_members_cloudformation_always_reports() {
    let cases = vec![
        AlwaysPresentCase {
            case: "nothing optional supplied",
            stack_name: "bare",
            extra: vec![],
            mode: "STANDARD",
            disable_rollback: "false",
            notification_arns: vec![],
            tag_keys: vec![],
        },
        AlwaysPresentCase {
            case: "tags, notification topics and a rollback configuration supplied",
            stack_name: "furnished",
            extra: vec![
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "dev"),
                member(
                    "NotificationARNs.member.1",
                    "arn:aws:sns:us-east-1:000000000000:one",
                ),
                member(
                    "NotificationARNs.member.2",
                    "arn:aws:sns:us-east-1:000000000000:two",
                ),
                member("RollbackConfiguration.MonitoringTimeInMinutes", "15"),
            ],
            mode: "STANDARD",
            disable_rollback: "false",
            notification_arns: vec![
                "arn:aws:sns:us-east-1:000000000000:one".to_string(),
                "arn:aws:sns:us-east-1:000000000000:two".to_string(),
            ],
            tag_keys: vec!["env".to_string()],
        },
        AlwaysPresentCase {
            case: "a deployment configuration stated explicitly",
            stack_name: "express",
            extra: vec![
                member("DeploymentConfig.Mode", "EXPRESS"),
                member("DeploymentConfig.DisableRollback", "true"),
            ],
            mode: "EXPRESS",
            disable_rollback: "true",
            notification_arns: vec![],
            tag_keys: vec![],
        },
        AlwaysPresentCase {
            case: "rollback disabled through the top-level flag",
            stack_name: "no-rollback",
            extra: vec![member("DisableRollback", "true")],
            mode: "STANDARD",
            disable_rollback: "true",
            notification_arns: vec![],
            tag_keys: vec![],
        },
    ];

    let router = router();
    for case in &cases {
        let (status, created) =
            create_stack(&router, case.stack_name, r#"{"Description":"d"}"#, &case.extra).await;
        assert_eq!(
            status, 200,
            "case {:?}: the stack under test must be created, got {status} {created}",
            case.case
        );
        let operation_id = xml_text(&created, "OperationId")
            .unwrap_or_else(|| panic!("case {:?}: CreateStack answered no OperationId", case.case));

        let (status, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStacks to succeed, got {status} {body}",
            case.case
        );
        for always in [
            "NotificationARNs",
            "Tags",
            "RollbackConfiguration",
            "DeploymentConfig",
            "LastOperations",
        ] {
            assert!(
                has_element(&body, always),
                "case {:?}: {always} is reported on every stack, got {body}",
                case.case
            );
        }
        assert_eq!(
            xml_texts(&body, "member")
                .into_iter()
                .filter(|text| text.starts_with("arn:aws:sns"))
                .collect::<Vec<_>>(),
            case.notification_arns,
            "case {:?}: the notification topics are read back from the store, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "Key"),
            case.tag_keys,
            "case {:?}: the tag keys are read back from the store, got {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Mode").as_deref(),
            Some(case.mode),
            "case {:?}: expected DeploymentConfig Mode {}, got {body}",
            case.case,
            case.mode
        );
        assert_eq!(
            xml_texts(&body, "DisableRollback").last().map(String::as_str),
            Some(case.disable_rollback),
            "case {:?}: expected DeploymentConfig DisableRollback {}, got {body}",
            case.case,
            case.disable_rollback
        );
        assert_eq!(
            xml_text(&body, "OperationType").as_deref(),
            Some("CREATE_STACK"),
            "case {:?}: the last operation on a created stack is its create, got {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "OperationId").as_deref(),
            Some(operation_id.as_str()),
            "case {:?}: LastOperations carries the id CreateStack handed back, got {body}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_last_operation_follows_the_stack_through_update_and_delete() {
    let router = router();
    create_stack(&router, "tracked", r#"{"Description":"v1"}"#, &[]).await;
    wait_for_the_transition().await;

    // One row per stack operation that carries an `OperationId`, in the order a stack
    // lives through them; `OperationType` comes from the model's enum.
    struct OperationCase {
        action: &'static str,
        operation_type: &'static str,
        members: Vec<(String, String)>,
    }

    let operations = vec![
        OperationCase {
            action: "UpdateStack",
            operation_type: "UPDATE_STACK",
            members: vec![
                member("StackName", "tracked"),
                member("TemplateBody", r#"{"Description":"v2"}"#),
            ],
        },
        OperationCase {
            action: "DeleteStack",
            operation_type: "DELETE_STACK",
            members: vec![member("StackName", "tracked")],
        },
    ];
    let mut seen_ids: Vec<String> = Vec::new();
    for OperationCase {
        action,
        operation_type,
        members,
    } in operations
    {
        let (status, body) = call(&router, action, &members).await;
        assert_eq!(
            status, 200,
            "case {action:?}: the operation must be accepted, got {status} {body}"
        );
        let (status, described) = describe_stack(&router, "tracked").await;
        assert_eq!(
            status, 200,
            "case {action:?}: expected DescribeStacks to succeed, got {status} {described}"
        );
        assert_eq!(
            xml_text(&described, "OperationType").as_deref(),
            Some(operation_type),
            "case {action:?}: expected LastOperations OperationType {operation_type}, got \
             {described}"
        );
        let id = xml_text(&described, "OperationId")
            .unwrap_or_else(|| panic!("case {action:?}: no OperationId reported"));
        assert!(
            !seen_ids.contains(&id),
            "case {action:?}: each operation has its own id, got {id} after {seen_ids:?}"
        );
        seen_ids.push(id);
        wait_for_the_transition().await;
    }
}

#[tokio::test]
async fn list_stacks_reports_the_same_last_operation_as_describe_stacks() {
    let router = router();
    let (_, created) = create_stack(&router, "listed", r#"{"Description":"d"}"#, &[]).await;
    let operation_id = xml_text(&created, "OperationId").expect("an operation id");

    // `StackSummary` carries `LastOperations` too, so the summary a list answers reports
    // the same operation the stack itself does.
    let (status, body) = call(&router, "ListStacks", &[]).await;
    assert_eq!(
        status, 200,
        "expected ListStacks to succeed, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "OperationType").as_deref(),
        Some("CREATE_STACK"),
        "the summary reports the operation that created the stack, got {body}"
    );
    assert_eq!(
        xml_text(&body, "OperationId").as_deref(),
        Some(operation_id.as_str()),
        "the summary carries the id CreateStack handed back, got {body}"
    );
}

/// One create request and the parameters the stack must report back.
struct ParameterCase {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    supplied: Vec<(&'static str, &'static str)>,
    expected: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn parameters_report_the_values_in_force_including_template_defaults() {
    let cases = vec![
        ParameterCase {
            case: "a declared parameter whose value the request supplied",
            stack_name: "par-supplied",
            template: r#"{"Parameters":{"TagValue":{"Type":"String"}}}"#,
            supplied: vec![("TagValue", "initial")],
            expected: vec![("TagValue", "initial")],
        },
        ParameterCase {
            case: "a declared parameter the request left to its Default",
            stack_name: "par-default",
            template: r#"{"Parameters":{"TagValue":{"Type":"String","Default":"initial"}}}"#,
            supplied: vec![],
            expected: vec![("TagValue", "initial")],
        },
        ParameterCase {
            case: "a supplied value overrides the declared Default",
            stack_name: "par-override",
            template: r#"{"Parameters":{"TagValue":{"Type":"String","Default":"initial"}}}"#,
            supplied: vec![("TagValue", "supplied")],
            expected: vec![("TagValue", "supplied")],
        },
        ParameterCase {
            // A parameter with no `Default` has to be given a value, so the stack in force
            // here mixes one of each: the supplied value and the untouched default.
            case: "a parameter with no Default supplied beside one left to its Default",
            stack_name: "par-absent",
            template: r#"{"Parameters":{"TagValue":{"Type":"String"},"Other":{"Type":"String","Default":"kept"}}}"#,
            supplied: vec![("TagValue", "given")],
            expected: vec![("Other", "kept"), ("TagValue", "given")],
        },
        ParameterCase {
            case: "two declared defaults, one of them overridden",
            stack_name: "par-several",
            template: r#"{"Parameters":{"Alpha":{"Type":"String","Default":"a"},"Zulu":{"Type":"String","Default":"z"}}}"#,
            supplied: vec![("Zulu", "Z")],
            expected: vec![("Alpha", "a"), ("Zulu", "Z")],
        },
        ParameterCase {
            case: "a template declaring no parameters reports none",
            stack_name: "par-none",
            template: r#"{"Description":"plain"}"#,
            supplied: vec![],
            expected: vec![],
        },
        ParameterCase {
            case: "a body that is not JSON, whose declaration takes the supplied value",
            stack_name: "par-yaml",
            template: "Description: plain\nParameters:\n  Only:\n    Type: String\n",
            supplied: vec![("Only", "supplied")],
            expected: vec![("Only", "supplied")],
        },
    ];

    let router = router();
    for case in &cases {
        let mut extra = Vec::new();
        for (index, (key, value)) in case.supplied.iter().enumerate() {
            let n = index + 1;
            extra.push(member(&format!("Parameters.member.{n}.ParameterKey"), key));
            extra.push(member(
                &format!("Parameters.member.{n}.ParameterValue"),
                value,
            ));
        }
        let (status, created) =
            create_stack(&router, case.stack_name, case.template, &extra).await;
        assert_eq!(
            status, 200,
            "case {:?}: the stack under test must be created, got {status} {created}",
            case.case
        );

        let (status, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStacks to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "ParameterKey"),
            case.expected
                .iter()
                .map(|(key, _)| key.to_string())
                .collect::<Vec<_>>(),
            "case {:?}: expected the parameter keys in force, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "ParameterValue"),
            case.expected
                .iter()
                .map(|(_, value)| value.to_string())
                .collect::<Vec<_>>(),
            "case {:?}: expected the parameter values in force, got {body}",
            case.case
        );
        assert_eq!(
            case.expected.is_empty(),
            !has_element(&body, "Parameters"),
            "case {:?}: a stack with no parameters in force reports no Parameters member, \
             got {body}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_reports_the_parameters_in_force_after_it() {
    let router = router();
    let original = r#"{"Parameters":{"Env":{"Type":"String","Default":"first"}}}"#;
    let replacement =
        r#"{"Parameters":{"Env":{"Type":"String","Default":"first"},"Added":{"Type":"String","Default":"new"}}}"#;
    create_stack(&router, "par-updated", original, &[]).await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "par-updated").await;
    assert_eq!(
        xml_texts(&body, "ParameterValue"),
        vec!["first".to_string()],
        "case \"after the create\": the declared default is in force, got {body}"
    );

    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "par-updated"),
            member("TemplateBody", replacement),
            member("Parameters.member.1.ParameterKey", "Env"),
            member("Parameters.member.1.ParameterValue", "second"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the update must be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "par-updated").await;
    assert_eq!(
        xml_texts(&body, "ParameterKey"),
        vec!["Added".to_string(), "Env".to_string()],
        "case \"after the update\": the new template's parameters are in force, got {body}"
    );
    assert_eq!(
        xml_texts(&body, "ParameterValue"),
        vec!["new".to_string(), "second".to_string()],
        "case \"after the update\": the supplied value wins over the declared default, got \
         {body}"
    );
}

#[tokio::test]
async fn update_termination_protection_is_read_back_from_describe_stacks() {
    let router = router();
    let (_, created) = create_stack(&router, "guarded", "{}", &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");
    wait_for_the_transition().await;

    // The flag is a boolean the caller sets either way, addressed by name or by stack id;
    // each row is read back through DescribeStacks.
    let cases: Vec<(&str, String, &str)> = vec![
        ("enabled by name", "guarded".to_string(), "true"),
        ("disabled again by name", "guarded".to_string(), "false"),
        ("enabled by stack id", stack_id.clone(), "true"),
    ];
    for (case, name_or_id, flag) in cases {
        let (status, body) = call(
            &router,
            "UpdateTerminationProtection",
            &[
                member("StackName", &name_or_id),
                member("EnableTerminationProtection", flag),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call must be accepted, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "StackId").as_deref(),
            Some(stack_id.as_str()),
            "case {case:?}: the call answers the stack it changed, got {body}"
        );

        let (status, body) = describe_stack(&router, "guarded").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStacks to succeed, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "EnableTerminationProtection").as_deref(),
            Some(flag),
            "case {case:?}: the stack must report the flag that was set, got {body}"
        );
    }
}

#[tokio::test]
async fn update_termination_protection_refuses_an_identifier_that_names_no_live_stack() {
    let router = router();
    let (_, created) = create_stack(&router, "doomed", "{}", &[]).await;
    let deleted_id = xml_text(&created, "StackId").expect("a stack id");
    wait_for_the_transition().await;
    call(&router, "DeleteStack", &[member("StackName", "doomed")]).await;
    wait_for_the_transition().await;

    let cases: Vec<(&str, String)> = vec![
        ("a name no stack ever had", "never-existed".to_string()),
        ("the id of a deleted stack", deleted_id),
    ];
    for (case, name_or_id) in cases {
        let (status, body) = call(
            &router,
            "UpdateTerminationProtection",
            &[
                member("StackName", &name_or_id),
                member("EnableTerminationProtection", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
    }
}
