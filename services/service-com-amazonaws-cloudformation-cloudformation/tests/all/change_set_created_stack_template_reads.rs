// SPDX-License-Identifier: CC0-1.0

//! What a read of a stack's TEMPLATE answers while that stack is the record a `CREATE` change
//! set brought into existence, end to end over the awsQuery wire.
//!
//! RULE: a stack a `CREATE` change set created holds no template of its own —
//! `CreateChangeSetInput$ChangeSetType`: "CloudFormation creates a stack with a unique stack
//! ID, but no template or resources. The stack will be in the REVIEW_IN_PROGRESS state until
//! you execute the change set" — but it carries that change set (`Stack$ChangeSetId`), whose
//! template IS the document the stack would be deployed from: a read of the stack's template
//! serves THAT document, at either `TemplateStage`, however the stack is addressed, and the
//! execution then puts the same document on the stack. So a client whose first deploy goes
//! through the change-set path never finds the stack it has just registered without a
//! template.
//!
//! The recorded trace this suite was written for — one deploy of one template through
//! `CreateChangeSet(ChangeSetType=CREATE)` — is ONE row of the table below. The rows differ
//! along what the rule generalises over: the language of the body, how many resources it
//! declares, whether it declares parameters a supplied value feeds, and the characters the
//! body carries. Every row is then read through every spelling of the request the rule covers:
//! the stack by name and by stack id, the change set by name beside its stack and by its ARN
//! alone, at `Original`, at `Processed` and at the stage the service defaults to.
//!
//! `change_set_lifecycle.rs` holds the plan a change set reports and the statuses its stack
//! moves through; what is new here is WHICH document each of those template reads answers
//! while the stack is in review, that a re-submitted plan moves the answer to the newest
//! change set, and that the answer survives the execution.

use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_set_status, execution_status, status,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{
    create_change_set, describe_change_set, execute_change_set, settle_the_store,
};
use crate::harness::{
    call, get_template, member, router_with_engine, xml_blocks, xml_text, xml_texts,
};
use crate::stack_lifecycle::describe_stack;

/// The `ChangeSetType` that brings the stack into existence
/// (`CreateChangeSetInput$ChangeSetType`, one of CREATE | IMPORT | UPDATE).
const CREATE: &str = "CREATE";

/// The two `TemplateStage` values the model declares (`GetTemplateInput$TemplateStage`, one of
/// Original | Processed), beside the request that names NONE of them — "by default,
/// CloudFormation specifies Processed" — which is the spelling a client that asks for no stage
/// sends.
const STAGES: [Option<&str>; 3] = [None, Some("Original"), Some("Processed")];

/// The stages a response advertises, read out of the `StagesAvailable` block so a `<member>`
/// of another list the envelope carries cannot be swept up with it.
fn stages_available(xml: &str) -> Vec<String> {
    match xml_blocks(xml, "StagesAvailable").into_iter().next() {
        Some(block) => xml_texts(&block, "member"),
        None => Vec::new(),
    }
}

/// Both stages, which is what a template declaring no transform makes available: "if the
/// template doesn't include transforms, Original and Processed return the same template".
fn both_stages() -> Vec<String> {
    vec!["Original".to_string(), "Processed".to_string()]
}

/// One row of the table: a template submitted through a `CREATE` change set.
struct Row {
    case: &'static str,
    stack: &'static str,
    change_set: &'static str,
    template: String,
    /// The `(ParameterKey, ParameterValue)` pairs the request supplies for the parameters the
    /// body declares.
    parameters: Vec<(&'static str, &'static str)>,
    /// The logical ids the executed stack must report, which this suite compares as a SET —
    /// `list_stack_resources.rs` is the suite that pins the order.
    resources: Vec<&'static str>,
}

/// `Parameters.member.<n>` as an SDK spells a parameter list on the wire.
fn parameter_members(parameters: &[(&str, &str)]) -> Vec<(String, String)> {
    parameters
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                member(&format!("Parameters.member.{position}.ParameterKey"), key),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

/// The class the rule covers: any template CloudFormation reads, submitted through a change
/// set that creates its stack.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one resource in a JSON body, the recorded trace's own shape",
            stack: "rev-one",
            change_set: "rev-one-set",
            template: r#"{"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#
                .to_string(),
            parameters: Vec::new(),
            resources: vec!["Topic"],
        },
        Row {
            case: "two resources in a YAML body, the other language CloudFormation reads",
            stack: "rev-yaml",
            change_set: "rev-yaml-set",
            template: "Resources:\n  Group:\n    Type: AWS::Logs::LogGroup\n  Topic:\n    Type: AWS::SNS::Topic\n    Properties:\n      DisplayName: yaml\n"
                .to_string(),
            parameters: Vec::new(),
            resources: vec!["Group", "Topic"],
        },
        Row {
            case: "a body whose declared parameter a supplied value feeds",
            stack: "rev-parameterised",
            change_set: "rev-parameterised-set",
            template: r#"{"Parameters":{"Label":{"Type":"String","Default":"unset"}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":{"Ref":"Label"}}}}}"#
                .to_string(),
            parameters: vec![("Label", "supplied")],
            resources: vec!["Topic"],
        },
        Row {
            // The XML escaping a property full of punctuation needs. The text a body carries
            // past ASCII is held as ASCII by the store (`template_body_ascii_text.rs`), so
            // this row submits ASCII and reads back byte for byte.
            case: "quotes and angle brackets in a resource property",
            stack: "rev-escaped",
            change_set: "rev-escaped-set",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"cafe \"quoted\" & <angled>"}}}}"#
                .to_string(),
            parameters: Vec::new(),
            resources: vec!["Topic"],
        },
        Row {
            case: "three resources of three types, with a description and metadata",
            stack: "rev-three",
            change_set: "rev-three-set",
            template: r#"{"Description":"three","Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"t"}},"Group":{"Type":"AWS::Logs::LogGroup","Metadata":{"m":1}},"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#
                .to_string(),
            parameters: Vec::new(),
            resources: vec!["Group", "Handle", "Topic"],
        },
    ]
}

/// A `CREATE` change set for `stack`, carrying `template` and whatever parameters it declares.
async fn create_the_stacks_change_set(
    router: &MultiServiceRouter,
    stack: &str,
    change_set: &str,
    template: &str,
    parameters: &[(&str, &str)],
) -> (u16, String) {
    let mut extra = vec![
        member("ChangeSetType", CREATE),
        member("TemplateBody", template),
    ];
    extra.extend(parameter_members(parameters));
    create_change_set(router, stack, change_set, &extra).await
}

/// Assert that EVERY template read of the stack `identifiers` address answers `expected`, at
/// every stage a request can name, advertising both stages as available.
async fn assert_the_stack_serves(
    case: &str,
    router: &MultiServiceRouter,
    identifiers: &[String],
    expected: &str,
) {
    for identifier in identifiers {
        for stage in STAGES {
            let named = stage.unwrap_or("the service's default");
            let (code, body) = get_template(router, identifier, stage).await;
            assert_eq!(
                code, 200,
                "case {case:?}: GetTemplate({identifier}, {named}) must answer the stack's \
                 template, got {code} {body}"
            );
            assert_eq!(
                xml_text(&body, "TemplateBody").as_deref(),
                Some(expected),
                "case {case:?}: GetTemplate({identifier}, {named}) must answer the submitted \
                 body, got {body}"
            );
            assert_eq!(
                stages_available(&body),
                both_stages(),
                "case {case:?}: GetTemplate({identifier}, {named}) must advertise both stages, \
                 got {body}"
            );
        }
    }
}

/// Assert that reading the CHANGE SET's own template — by name beside its stack, and by its
/// ARN alone, which is the other spelling the modelled `ChangeSetNameOrId` admits — answers
/// `expected`.
async fn assert_the_change_set_serves(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    change_set: &str,
    change_set_id: &str,
    expected: &str,
) {
    let addressings: [(&str, Vec<(String, String)>); 2] = [
        (
            "by name beside its stack",
            vec![
                member("ChangeSetName", change_set),
                member("StackName", stack),
            ],
        ),
        (
            "by its ARN alone",
            vec![member("ChangeSetName", change_set_id)],
        ),
    ];
    for (addressed, head) in addressings {
        for stage in STAGES {
            let named = stage.unwrap_or("the service's default");
            let mut members = head.clone();
            if let Some(stage) = stage {
                members.push(member("TemplateStage", stage));
            }
            let (code, body) = call(router, "GetTemplate", &members).await;
            assert_eq!(
                code, 200,
                "case {case:?}: GetTemplate({addressed}, {named}) must answer the change set's \
                 template, got {code} {body}"
            );
            assert_eq!(
                xml_text(&body, "TemplateBody").as_deref(),
                Some(expected),
                "case {case:?}: GetTemplate({addressed}, {named}) must answer the submitted \
                 body, got {body}"
            );
        }
    }
}

/// The logical ids the stack reports through `DescribeStackResources`, sorted, so the set is
/// compared without pinning an order this suite is not about.
async fn deployed_logical_ids(case: &str, router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the executed stack's resources must be readable, got {code} {body}"
    );
    let mut ids = xml_texts(&body, "LogicalResourceId");
    ids.sort();
    ids
}

#[tokio::test]
async fn a_stack_in_review_serves_the_template_of_the_change_set_that_created_it() {
    for row in rows() {
        // One store per row, so no row can read another's stack.
        let (router, engine) = router_with_engine();

        let (code, created) = create_the_stacks_change_set(
            &router,
            row.stack,
            row.change_set,
            &row.template,
            &row.parameters,
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected CreateChangeSet to succeed, got {code} {created}",
            row.case
        );
        let change_set_id = xml_text(&created, "Id").unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateChangeSet answered no Id: {created}",
                row.case
            )
        });
        let stack_id = xml_text(&created, "StackId").unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateChangeSet answered no StackId: {created}",
                row.case
            )
        });

        // The stack the call registered is in review, carrying the change set that created it.
        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: the registered stack must be readable, got {code} {described}",
            row.case
        );
        assert_eq!(
            (
                xml_text(&described, "StackStatus").as_deref(),
                xml_text(&described, "ChangeSetId").as_deref(),
            ),
            (
                Some(status::REVIEW_IN_PROGRESS),
                Some(change_set_id.as_str()),
            ),
            "case {:?}: the stack is in review carrying its change set, got {described}",
            row.case
        );

        // The reads the rule is about: while the stack is in review it has no template of its
        // own, and every template read of it — by name or by stack id, at either stage or at
        // none — answers the document the change set submitted.
        let identifiers = [row.stack.to_string(), stack_id.clone()];
        assert_the_stack_serves(row.case, &router, &identifiers, &row.template).await;
        assert_the_change_set_serves(
            row.case,
            &router,
            row.stack,
            row.change_set,
            &change_set_id,
            &row.template,
        )
        .await;

        // The plan is executable once its creation has settled, which is the state a client
        // waits for before it executes.
        settle_the_store(&router, &engine).await;
        let (code, planned) = describe_change_set(&router, row.change_set, Some(row.stack)).await;
        assert_eq!(
            code, 200,
            "case {:?}: the created change set must be describable, got {code} {planned}",
            row.case
        );
        assert_eq!(
            (
                xml_text(&planned, "Status").as_deref(),
                xml_text(&planned, "ExecutionStatus").as_deref(),
            ),
            (
                Some(change_set_status::CREATE_COMPLETE),
                Some(execution_status::AVAILABLE),
            ),
            "case {:?}: the plan must be complete and executable, got {planned}",
            row.case
        );

        let (code, executed) = execute_change_set(&router, row.change_set, Some(row.stack)).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected ExecuteChangeSet to succeed, got {code} {executed}",
            row.case
        );
        settle_the_store(&router, &engine).await;
        let (code, settled) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: the executed stack must be readable, got {code} {settled}",
            row.case
        );
        assert_eq!(
            (
                xml_text(&settled, "StackStatus").as_deref(),
                xml_text(&settled, "StackId").as_deref(),
            ),
            (Some(status::CREATE_COMPLETE), Some(stack_id.as_str())),
            "case {:?}: the execution completes the very record the change set created, got \
             {settled}",
            row.case
        );

        // The execution put that same document on the stack, so the reads that served the
        // change set's template now serve the stack's own — the same bytes either way.
        assert_the_stack_serves(row.case, &router, &identifiers, &row.template).await;
        assert_eq!(
            deployed_logical_ids(row.case, &router, row.stack).await,
            row.resources
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {:?}: the deployed stack must hold the resources the served template \
             declares",
            row.case
        );
    }
}

/// One row of the re-submitted-plan table: the body the stack is registered with, and the body
/// a SECOND `CREATE` change set submits for that same stack while it is still in review.
struct Resubmission {
    case: &'static str,
    stack: &'static str,
    first_template: &'static str,
    second_template: &'static str,
}

/// The class: any second `CREATE` change set for a stack still in review — the retry a client
/// makes when its first attempt was never executed — whatever the two bodies are.
fn resubmissions() -> Vec<Resubmission> {
    vec![
        Resubmission {
            case: "a resource added to the plan",
            stack: "rev-again",
            first_template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            second_template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"},"Group":{"Type":"AWS::Logs::LogGroup"}}}"#,
        },
        Resubmission {
            case: "the plan re-submitted in the other language",
            stack: "rev-again-yaml",
            first_template: r#"{"Resources":{"Group":{"Type":"AWS::Logs::LogGroup"}}}"#,
            second_template: "Resources:\n  Group:\n    Type: AWS::Logs::LogGroup\n    Properties:\n      RetentionInDays: 7\n",
        },
    ]
}

#[tokio::test]
async fn the_newest_create_change_set_is_the_template_a_stack_in_review_serves() {
    for row in resubmissions() {
        let (router, engine) = router_with_engine();

        let (code, first) =
            create_the_stacks_change_set(&router, row.stack, "first-plan", row.first_template, &[])
                .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected the first CreateChangeSet to succeed, got {code} {first}",
            row.case
        );
        let first_id = xml_text(&first, "Id").expect("the first change set's Id");

        let (code, second) = create_the_stacks_change_set(
            &router,
            row.stack,
            "second-plan",
            row.second_template,
            &[],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: a second CREATE change set for a stack still in review must be \
             accepted, got {code} {second}",
            row.case
        );
        let second_id = xml_text(&second, "Id").expect("the second change set's Id");
        let stack_id = xml_text(&second, "StackId").expect("the second change set's StackId");

        // `Stack$ChangeSetId` is the change set the stack carries, so the template its reads
        // serve moves with it: the newest plan is the document the stack would be deployed
        // from.
        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: the stack must still be readable, got {code} {described}",
            row.case
        );
        assert_eq!(
            (
                xml_text(&described, "StackStatus").as_deref(),
                xml_text(&described, "ChangeSetId").as_deref(),
            ),
            (Some(status::REVIEW_IN_PROGRESS), Some(second_id.as_str())),
            "case {:?}: the stack carries the newest change set, got {described}",
            row.case
        );
        let identifiers = [row.stack.to_string(), stack_id.clone()];
        assert_the_stack_serves(row.case, &router, &identifiers, row.second_template).await;

        // Each change set still serves ITS OWN template: the first plan is unchanged by the
        // second, and is readable by its ARN alone.
        assert_the_change_set_serves(
            row.case,
            &router,
            row.stack,
            "first-plan",
            &first_id,
            row.first_template,
        )
        .await;
        assert_the_change_set_serves(
            row.case,
            &router,
            row.stack,
            "second-plan",
            &second_id,
            row.second_template,
        )
        .await;

        // Executing the newest plan puts the document the stack was serving onto the stack.
        settle_the_store(&router, &engine).await;
        let (code, executed) = execute_change_set(&router, "second-plan", Some(row.stack)).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected ExecuteChangeSet to succeed, got {code} {executed}",
            row.case
        );
        settle_the_store(&router, &engine).await;
        let (code, settled) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: the executed stack must be readable, got {code} {settled}",
            row.case
        );
        assert_eq!(
            xml_text(&settled, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: executing the newest plan creates the stack, got {settled}",
            row.case
        );
        assert_the_stack_serves(row.case, &router, &identifiers, row.second_template).await;
    }
}
