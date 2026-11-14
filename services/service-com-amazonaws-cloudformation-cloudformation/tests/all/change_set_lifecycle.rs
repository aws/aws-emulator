// SPDX-License-Identifier: CC0-1.0

//! The change-set lifecycle, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **a change set is a persisted plan for
//! one stack, and a `CREATE` change set brings that stack into existence in
//! `REVIEW_IN_PROGRESS` keyed by the `StackId` the call answers; `ExecuteChangeSet` applies
//! the plan the change set stored — its template and configuration — to that same stack
//! record and drives it through `*_IN_PROGRESS` to its completed status, so every later
//! read of the stack resolves it.**
//!
//! The recorded trace this suite was written for is ONE row of each table below: the cases
//! span the template shapes, resource counts, change kinds and change-set types the rule
//! generalises over, and every read goes through the real request path (form-encoded body
//! in, XML envelope out) so what is asserted is what a client sees.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_action, change_set_status, execution_status, status, MAX_CHANGE_SETS_PER_STACK,
    NO_CHANGES_REASON, USER_INITIATED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_stack, get_template, member, router_with_engine, store, xml_text, xml_texts,
    REGION,
};
use crate::stack_lifecycle::{assert_stack_id_shape, describe_stack, ACCOUNT_ID};

/// `CreateChangeSet`, with the members a case varies spelled as an SDK sends them.
pub async fn create_change_set(
    router: &MultiServiceRouter,
    stack_name: &str,
    change_set_name: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("StackName", stack_name),
        member("ChangeSetName", change_set_name),
    ];
    members.extend_from_slice(extra);
    call(router, "CreateChangeSet", &members).await
}

/// A `CREATE` change set for a stack that does not exist yet.
pub async fn create_creating_change_set(
    router: &MultiServiceRouter,
    stack_name: &str,
    change_set_name: &str,
    template: &str,
) -> (u16, String) {
    create_change_set(
        router,
        stack_name,
        change_set_name,
        &[
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", template),
        ],
    )
    .await
}

pub async fn describe_change_set(
    router: &MultiServiceRouter,
    change_set: &str,
    stack_name: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("ChangeSetName", change_set)];
    if let Some(stack_name) = stack_name {
        members.push(member("StackName", stack_name));
    }
    call(router, "DescribeChangeSet", &members).await
}

pub async fn execute_change_set(
    router: &MultiServiceRouter,
    change_set: &str,
    stack_name: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("ChangeSetName", change_set)];
    if let Some(stack_name) = stack_name {
        members.push(member("StackName", stack_name));
    }
    call(router, "ExecuteChangeSet", &members).await
}

/// A change set's id is the ARN form CloudFormation mints:
/// `arn:aws:cloudformation:<region>:<account>:changeSet/<name>/<uuid>`, which is also what
/// the modelled `ChangeSetId` pattern `^arn:[-a-zA-Z0-9:/]*$` admits.
fn assert_change_set_id_shape(case: &str, change_set_id: &str, change_set_name: &str) {
    let prefix =
        format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:changeSet/{change_set_name}/");
    assert!(
        change_set_id.starts_with(&prefix),
        "case {case:?}: expected a change set id starting {prefix:?}, got {change_set_id:?}"
    );
    let uuid = &change_set_id[prefix.len()..];
    assert_eq!(
        uuid.len(),
        36,
        "case {case:?}: expected a 36-character uuid after the change set name, got {uuid:?}"
    );
    assert!(
        uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case:?}: expected a hex-and-hyphen uuid, got {uuid:?}"
    );
}

/// Make every transition this store owes due NOW, then let a read materialise them.
///
/// A stack's and a change set's terminal statuses become due a moment after the call that
/// starts them, and the suite below asserts the SETTLED state over and over. Sleeping past
/// each due instant would make every case a race against the machine's load, so the due
/// instants are pulled into the past instead and an ordinary read settles the store exactly
/// as the clock would have: what the cases assert is the emulator's own settling, not the
/// wall clock. The in-flight states are asserted where they belong — against a store held
/// BEFORE its due instant, in
/// [`a_change_set_cannot_be_executed_before_its_creation_completes`].
pub(crate) async fn settle_the_store(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) {
    {
        let state = store(engine);
        let mut held = state.lock();
        let due = SystemTime::now() - Duration::from_secs(1);
        for stack in &mut held.stacks {
            if let Some(pending) = stack.pending.as_mut() {
                pending.due_at = due;
            }
        }
        for change_set in &mut held.change_sets {
            if let Some(pending) = change_set.pending.as_mut() {
                pending.due_at = due;
            }
        }
    }
    // Every operation settles the store first, so any read materialises what is now due.
    let (code, body) = call(router, "ListStacks", &[]).await;
    assert_eq!(code, 200, "the settling read must succeed, got {code} {body}");
}

/// The change set as it reads once its creation has settled.
pub async fn settled_change_set(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    change_set: &str,
    stack_name: Option<&str>,
) -> String {
    settle_the_store(router, engine).await;
    let (code, body) = describe_change_set(router, change_set, stack_name).await;
    assert_eq!(
        code, 200,
        "a created change set must be describable, got {code} {body}"
    );
    body
}

/// The `Scope` entries a response carries, across every `ResourceChange` in it.
///
/// `Scope` is a list whose entries are `<member>` elements, so they are read out of each
/// `<Scope>` block rather than off the whole document, where a change's own `<member>`
/// wrapper would swallow them.
fn scope_entries(xml: &str) -> Vec<String> {
    crate::stack_lifecycle::xml_blocks(xml, "Scope")
        .iter()
        .flat_map(|scope| xml_texts(scope, "member"))
        .collect()
}

/// One row of the create-and-execute table: a template the class covers.
struct ChangeSetCase {
    case: &'static str,
    stack_name: &'static str,
    change_set_name: &'static str,
    template: String,
    /// The `(LogicalResourceId, ResourceType)` pairs the change set must report as `Add`s,
    /// in the order the change computation yields them (by logical id).
    added: Vec<(&'static str, &'static str)>,
}

/// The class the rule covers: any template CloudFormation reads — JSON or YAML, of any
/// size, declaring any number of resources of any type. The recorded trace's own case (one
/// `AWS::S3::Bucket`) is the first row.
fn change_set_cases() -> Vec<ChangeSetCase> {
    vec![
        ChangeSetCase {
            case: "one resource, the recorded trace's own template",
            stack_name: "cs-one",
            change_set_name: "cs-one-set",
            template: r#"{"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Bucket": {"Type": "AWS::S3::Bucket"}}}"#
                .to_string(),
            added: vec![("Bucket", "AWS::S3::Bucket")],
        },
        ChangeSetCase {
            case: "three resources of three types, with properties and metadata",
            stack_name: "cs-three",
            change_set_name: "cs-three-set",
            template: r#"{"Description":"three","Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"t"}},"Group":{"Type":"AWS::Logs::LogGroup","Metadata":{"m":1}},"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#
                .to_string(),
            added: vec![
                ("Group", "AWS::Logs::LogGroup"),
                ("Handle", "AWS::CloudFormation::WaitConditionHandle"),
                ("Topic", "AWS::SNS::Topic"),
            ],
        },
        ChangeSetCase {
            case: "a large template body with a padded resource property",
            stack_name: "cs-large",
            change_set_name: "cs-large-set",
            template: format!(
                r#"{{"Resources":{{"Pad":{{"Type":"AWS::CloudFormation::WaitConditionHandle","Metadata":{{"Pad":"{}"}}}}}}}}"#,
                "x".repeat(8192)
            ),
            added: vec![("Pad", "AWS::CloudFormation::WaitConditionHandle")],
        },
        ChangeSetCase {
            // The XML escaping a property full of punctuation needs: quotes, an ampersand
            // and angle brackets must survive the wire. The text a body carries past ASCII is
            // held as ASCII by the store (`template_body_ascii_text.rs`), so this row submits
            // ASCII and reads back byte for byte.
            case: "quotes and angle brackets in a resource property",
            stack_name: "cs-escaped",
            change_set_name: "cs-escaped-set",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"cafe \"quoted\" & <angled>"}}}}"#
                .to_string(),
            added: vec![("Topic", "AWS::SNS::Topic")],
        },
        ChangeSetCase {
            case: "a YAML template body, the other spelling CloudFormation accepts",
            stack_name: "cs-yaml",
            change_set_name: "cs-yaml-set",
            template: "Resources:\n  Group:\n    Type: AWS::Logs::LogGroup\n  Topic:\n    Type: AWS::SNS::Topic\n    Properties:\n      DisplayName: yaml\n"
                .to_string(),
            added: vec![("Group", "AWS::Logs::LogGroup"), ("Topic", "AWS::SNS::Topic")],
        },
    ]
}

#[tokio::test]
async fn a_create_change_set_materialises_its_stack_and_execute_completes_it() {
    let (router, engine) = router_with_engine();

    for case in change_set_cases() {
        let (code, body) =
            create_creating_change_set(&router, case.stack_name, case.change_set_name, &case.template)
                .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected CreateChangeSet to succeed, got {code} {body}",
            case.case
        );
        let change_set_id = xml_text(&body, "Id").unwrap_or_else(|| {
            panic!("case {:?}: CreateChangeSet answered no Id: {body}", case.case)
        });
        let stack_id = xml_text(&body, "StackId").unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateChangeSet answered no StackId: {body}",
                case.case
            )
        });
        assert_change_set_id_shape(case.case, &change_set_id, case.change_set_name);
        assert_stack_id_shape(case.case, &stack_id, case.stack_name);

        // The stack the call answered a StackId for EXISTS, addressed by either name or
        // id, in REVIEW_IN_PROGRESS until the change set is executed.
        for identifier in [case.stack_name.to_string(), stack_id.clone()] {
            let (code, body) = describe_stack(&router, &identifier).await;
            assert_eq!(
                code, 200,
                "case {:?}: DescribeStacks({identifier}) must find the change set's stack, \
                 got {code} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackStatus").as_deref(),
                Some(status::REVIEW_IN_PROGRESS),
                "case {:?}: the stack is in review until the change set is executed, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackStatusReason").as_deref(),
                Some(USER_INITIATED),
                "case {:?}: the review state's reason, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackId").as_deref(),
                Some(stack_id.as_str()),
                "case {:?}: the stack carries the id the change set reported, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "ChangeSetId").as_deref(),
                Some(change_set_id.as_str()),
                "case {:?}: the stack carries the change set that created it, got {body}",
                case.case
            );
        }

        // "CloudFormation creates a stack with a unique stack ID, but no template or
        // resources": the template the stack would be deployed from is the CHANGE SET's
        // until it is executed — and a read of the stack's template serves that very
        // document, which is what makes a prepared change set reviewable and executable
        // through the stack it created.
        let (code, body) = get_template(&router, case.stack_name, None).await;
        assert_eq!(
            code, 200,
            "case {:?}: a stack in review serves its change set's template, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.template.as_str()),
            "case {:?}: GetTemplate(StackName) answers the body the change set submitted, \
             got {body}",
            case.case
        );
        let (code, body) = call(
            &router,
            "GetTemplate",
            &[
                member("ChangeSetName", case.change_set_name),
                member("StackName", case.stack_name),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: the change set's own template is readable, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.template.as_str()),
            "case {:?}: GetTemplate(ChangeSetName) answers the submitted body, got {body}",
            case.case
        );

        // The change set reports the plan it computed from that template: one Add per
        // resource the template declares, in declaration order.
        let body =
            settled_change_set(&router, &engine, case.change_set_name, Some(case.stack_name)).await;
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(change_set_status::CREATE_COMPLETE),
            "case {:?}: a change set with changes completes, got {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "ExecutionStatus").as_deref(),
            Some(execution_status::AVAILABLE),
            "case {:?}: a completed change set is executable, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "Action"),
            vec![change_action::ADD.to_string(); case.added.len()],
            "case {:?}: every resource of a new stack is an addition, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "LogicalResourceId"),
            case.added
                .iter()
                .map(|(id, _)| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {:?}: the changes name the template's resources, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "ResourceType"),
            case.added
                .iter()
                .map(|(_, resource_type)| (*resource_type).to_string())
                .collect::<Vec<_>>(),
            "case {:?}: each change carries the resource's declared Type, got {body}",
            case.case
        );

        // Executing the plan drives THAT stack record to CREATE_COMPLETE.
        let (code, body) = execute_change_set(&router, case.change_set_name, Some(case.stack_name))
            .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected ExecuteChangeSet to succeed, got {code} {body}",
            case.case
        );
        let (code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            code, 200,
            "case {:?}: the stack is readable while it is being created, got {code} {body}",
            case.case
        );
        // The transition the clock owes is due a moment after the call, so a read taken
        // straight after it sees either side of that instant — but never the review state
        // the execution left behind.
        let status_now = xml_text(&body, "StackStatus");
        assert!(
            status_now.as_deref() == Some(status::CREATE_IN_PROGRESS)
                || status_now.as_deref() == Some(status::CREATE_COMPLETE),
            "case {:?}: executing a CREATE change set creates the stack, got {status_now:?} \
             in {body}",
            case.case
        );

        settle_the_store(&router, &engine).await;
        let (code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            code, 200,
            "case {:?}: the created stack stays readable, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack reaches CREATE_COMPLETE, got {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackId").as_deref(),
            Some(stack_id.as_str()),
            "case {:?}: the completed stack is the SAME record the change set created, got \
             {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "ChangeSetId").as_deref(),
            Some(change_set_id.as_str()),
            "case {:?}: the stack carries the executed change set, got {body}",
            case.case
        );
        // The template reached the stack, byte for byte, and is readable back through the
        // stack now.
        let (code, body) = get_template(&router, case.stack_name, None).await;
        assert_eq!(
            code, 200,
            "case {:?}: the executed stack's template is readable, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.template.as_str()),
            "case {:?}: the change set's template became the stack's, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "member"),
            vec!["Original".to_string(), "Processed".to_string()],
            "case {:?}: both template stages are available for a stack, got {body}",
            case.case
        );
        // The change set itself finished executing.
        let body =
            settled_change_set(&router, &engine, case.change_set_name, Some(case.stack_name)).await;
        assert_eq!(
            xml_text(&body, "ExecutionStatus").as_deref(),
            Some(execution_status::EXECUTE_COMPLETE),
            "case {:?}: the executed change set reports its execution, got {body}",
            case.case
        );
    }
}

/// One row of the change-computation table: what applying `next` to a stack whose template
/// is `previous` must be reported as.
struct DiffCase {
    case: &'static str,
    previous: &'static str,
    next: &'static str,
    /// `(Action, LogicalResourceId)` in the order the change set must report them.
    changes: Vec<(&'static str, &'static str)>,
    /// The `Scope` entries the change set must report across all its changes.
    scope: Vec<&'static str>,
}

/// The class the change computation covers: a resource only the new template declares is
/// an addition, one only the stack's template declares is a removal, one both declare
/// differently is a modification scoped to the attributes that differ, and one whose `Type`
/// changed cannot be modified in place.
fn diff_cases() -> Vec<DiffCase> {
    vec![
        DiffCase {
            case: "a resource added beside an unchanged one",
            previous: r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#,
            next: r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"},"B":{"Type":"AWS::Logs::LogGroup"}}}"#,
            changes: vec![(change_action::ADD, "B")],
            scope: vec![],
        },
        DiffCase {
            case: "a resource removed",
            previous: r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"},"B":{"Type":"AWS::Logs::LogGroup"}}}"#,
            next: r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#,
            changes: vec![(change_action::REMOVE, "B")],
            scope: vec![],
        },
        DiffCase {
            // The execute half of this row APPLIES the change, and this emulator does not
            // change the properties of a resource it really created in place (see
            // `state::provisioning`, pinned by
            // `core/router/tests/all/cloudformation_provisions_sns_topics.rs`), so the
            // resource whose properties differ here is one of a type it leaves
            // declared-only. What the row is about — a property difference is one MODIFY
            // change scoped to `Properties` — is the same either way.
            case: "a resource's properties changed",
            previous: r#"{"Resources":{"A":{"Type":"AWS::Logs::LogGroup","Properties":{"RetentionInDays":7}}}}"#,
            next: r#"{"Resources":{"A":{"Type":"AWS::Logs::LogGroup","Properties":{"RetentionInDays":14}}}}"#,
            changes: vec![(change_action::MODIFY, "A")],
            scope: vec!["Properties"],
        },
        DiffCase {
            case: "a resource's metadata and deletion policy changed",
            previous: r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic","Metadata":{"m":1}}}}"#,
            next: r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic","Metadata":{"m":2},"DeletionPolicy":"Retain"}}}"#,
            changes: vec![(change_action::MODIFY, "A")],
            // Reported in the order the model's `ResourceAttribute` enum lists them.
            scope: vec!["DeletionPolicy", "Metadata"],
        },
        DiffCase {
            case: "a resource's Type changed, which cannot be modified in place",
            previous: r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#,
            next: r#"{"Resources":{"A":{"Type":"AWS::Logs::LogGroup"}}}"#,
            changes: vec![(change_action::REMOVE, "A"), (change_action::ADD, "A")],
            scope: vec![],
        },
        DiffCase {
            case: "three resources added, removed and modified at once",
            previous: r#"{"Resources":{"Keep":{"Type":"AWS::SNS::Topic"},"Change":{"Type":"AWS::Logs::LogGroup","Properties":{"RetentionInDays":1}},"Drop":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            next: r#"{"Resources":{"Keep":{"Type":"AWS::SNS::Topic"},"Change":{"Type":"AWS::Logs::LogGroup","Properties":{"RetentionInDays":3}},"New":{"Type":"AWS::SNS::Topic"}}}"#,
            changes: vec![
                (change_action::MODIFY, "Change"),
                (change_action::ADD, "New"),
                (change_action::REMOVE, "Drop"),
            ],
            scope: vec!["Properties"],
        },
    ]
}

#[tokio::test]
async fn an_update_change_set_reports_the_changes_its_template_would_make() {
    let (router, engine) = router_with_engine();

    for (index, case) in diff_cases().into_iter().enumerate() {
        let stack_name = format!("diff-{index}");
        let change_set_name = format!("diff-set-{index}");
        let (code, body) = create_stack(&router, &stack_name, case.previous, &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: the stack the change set compares against, got {code} {body}",
            case.case
        );
        settle_the_store(&router, &engine).await;

        // No `ChangeSetType`: "By default, CloudFormation specifies UPDATE."
        let (code, body) = create_change_set(
            &router,
            &stack_name,
            &change_set_name,
            &[member("TemplateBody", case.next)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected CreateChangeSet to succeed, got {code} {body}",
            case.case
        );
        let body =
            settled_change_set(&router, &engine, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(change_set_status::CREATE_COMPLETE),
            "case {:?}: a change set with changes completes, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "Action"),
            case.changes
                .iter()
                .map(|(action, _)| (*action).to_string())
                .collect::<Vec<_>>(),
            "case {:?}: the computed actions, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "LogicalResourceId"),
            case.changes
                .iter()
                .map(|(_, logical_id)| (*logical_id).to_string())
                .collect::<Vec<_>>(),
            "case {:?}: the resources the changes name, got {body}",
            case.case
        );
        assert_eq!(
            scope_entries(&body),
            case.scope
                .iter()
                .map(|scope| (*scope).to_string())
                .collect::<Vec<_>>(),
            "case {:?}: a modification is scoped to the attributes that differ, got {body}",
            case.case
        );
        // A change to a resource the stack has already deployed carries that resource's
        // physical id; an addition has none to carry, since nothing is deployed under that
        // logical id yet.
        let acts_on_deployed = case
            .changes
            .iter()
            .filter(|(action, _)| *action != change_action::ADD)
            .count();
        let physical_ids = xml_texts(&body, "PhysicalResourceId");
        assert_eq!(
            physical_ids.len(),
            acts_on_deployed,
            "case {:?}: every change to a deployed resource names it physically, and no \
             addition does, got {body}",
            case.case
        );
        assert!(
            physical_ids.iter().all(|id| !id.is_empty()),
            "case {:?}: a reported physical id is the deployed resource's, got \
             {physical_ids:?}",
            case.case
        );

        // Executing it applies the new template to the same stack and updates it.
        let (code, body) = execute_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected ExecuteChangeSet to succeed, got {code} {body}",
            case.case
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            code, 200,
            "case {:?}: the updated stack is readable, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {:?}: executing an UPDATE change set updates the stack, got {body}",
            case.case
        );
        let (code, body) = get_template(&router, &stack_name, None).await;
        assert_eq!(
            code, 200,
            "case {:?}: the updated template is readable, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.next),
            "case {:?}: the change set's template became the stack's, got {body}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_change_set_that_would_change_nothing_fails_and_cannot_be_executed() {
    let (router, engine) = router_with_engine();
    let template = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#;
    create_stack(&router, "no-changes", template, &[]).await;
    settle_the_store(&router, &engine).await;

    let (code, body) = create_change_set(
        &router,
        "no-changes",
        "no-changes-set",
        &[member("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a change set that changes nothing\": the change set is still created, got \
         {code} {body}"
    );
    let body = settled_change_set(&router, &engine, "no-changes-set", Some("no-changes")).await;
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(change_set_status::FAILED),
        "case \"a change set that changes nothing\": it fails, got {body}"
    );
    assert_eq!(
        xml_text(&body, "StatusReason").as_deref(),
        Some(NO_CHANGES_REASON),
        "case \"a change set that changes nothing\": with the reason it failed, got {body}"
    );
    assert_eq!(
        xml_text(&body, "ExecutionStatus").as_deref(),
        Some(execution_status::UNAVAILABLE),
        "case \"a change set that changes nothing\": it is not executable, got {body}"
    );

    let (code, body) = execute_change_set(&router, "no-changes-set", Some("no-changes")).await;
    assert_eq!(
        code, 400,
        "case \"executing a failed change set\": it is refused, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("InvalidChangeSetStatus"),
        "case \"executing a failed change set\": with the modelled error, got {body}"
    );
    // The stack it would have updated is untouched.
    let (_, body) = describe_stack(&router, "no-changes").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case \"executing a failed change set\": the stack did not move, got {body}"
    );
}

#[tokio::test]
async fn a_change_set_cannot_be_executed_before_its_creation_completes() {
    let (router, engine) = router_with_engine();
    let (code, body) = create_creating_change_set(
        &router,
        "pending-exec",
        "pending-exec-set",
        r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#,
    )
    .await;
    assert_eq!(code, 200, "the change set is created, got {code} {body}");

    // The creation is in flight until the clock makes it due. Rather than race that
    // instant, the store is held at it: the transition is pushed out so the state a caller
    // acting straight after the call sees is the state this case asserts on.
    {
        let state = store(&engine);
        let mut held = state.lock();
        let change_set = held
            .resolve_change_set_mut(REGION, "pending-exec-set", Some("pending-exec"))
            .expect("the change set just created");
        let pending = change_set
            .pending
            .as_mut()
            .expect("a change set whose creation is still due");
        pending.due_at = SystemTime::now() + Duration::from_secs(3_600);
    }

    let (code, body) = describe_change_set(&router, "pending-exec-set", Some("pending-exec")).await;
    assert_eq!(code, 200, "the change set is describable, got {code} {body}");
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(change_set_status::CREATE_PENDING),
        "case \"straight after the create\": the creation is still pending, got {body}"
    );
    assert_eq!(
        xml_text(&body, "ExecutionStatus").as_deref(),
        Some(execution_status::UNAVAILABLE),
        "case \"straight after the create\": it is not executable yet, got {body}"
    );
    let (code, body) = execute_change_set(&router, "pending-exec-set", Some("pending-exec")).await;
    assert_eq!(
        code, 400,
        "case \"executing a pending change set\": it is refused, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("InvalidChangeSetStatus"),
        "case \"executing a pending change set\": with the modelled error, got {body}"
    );
    let (_, body) = describe_stack(&router, "pending-exec").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::REVIEW_IN_PROGRESS),
        "case \"executing a pending change set\": the stack stays in review, got {body}"
    );

    // Once the creation is due the same call is accepted.
    {
        let state = store(&engine);
        let mut held = state.lock();
        let change_set = held
            .resolve_change_set_mut(REGION, "pending-exec-set", Some("pending-exec"))
            .expect("the change set created above");
        let pending = change_set
            .pending
            .as_mut()
            .expect("a change set whose creation is still due");
        pending.due_at = SystemTime::now() - Duration::from_millis(1);
    }
    let (code, body) = execute_change_set(&router, "pending-exec-set", Some("pending-exec")).await;
    assert_eq!(
        code, 200,
        "case \"executing a settled change set\": it is accepted, got {code} {body}"
    );
    // And it cannot be executed twice: the second call finds it already executing.
    let (code, body) = execute_change_set(&router, "pending-exec-set", Some("pending-exec")).await;
    assert_eq!(
        code, 400,
        "case \"executing a change set twice\": the second call is refused, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("InvalidChangeSetStatus"),
        "case \"executing a change set twice\": with the modelled error, got {body}"
    );
}

#[tokio::test]
async fn a_change_set_is_addressable_by_its_arn_alone() {
    let (router, _engine) = router_with_engine();
    let (_, created) = create_creating_change_set(
        &router,
        "by-arn",
        "by-arn-set",
        r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#,
    )
    .await;
    let change_set_id = xml_text(&created, "Id").expect("a change set id");

    // "The name or Amazon Resource Name (ARN) of the change set": an ARN names it on its
    // own, a bare name needs the stack it belongs to — and a name with no stack at all is
    // a bad request, not a change set that does not exist.
    for (case, change_set, stack_name, expected, expected_code) in [
        ("the ARN alone", change_set_id.as_str(), None, 200, ""),
        (
            "the ARN with its stack",
            change_set_id.as_str(),
            Some("by-arn"),
            200,
            "",
        ),
        ("the name with its stack", "by-arn-set", Some("by-arn"), 200, ""),
        ("the name alone", "by-arn-set", None, 400, "ValidationError"),
        (
            // The stack is resolved BEFORE the change set, so a stack this Region holds none
            // of is refused about the stack rather than about the change set.
            "the name with the wrong stack",
            "by-arn-set",
            Some("by-arn-other"),
            400,
            "ValidationError",
        ),
        (
            "a name no change set carries",
            "nope",
            Some("by-arn"),
            404,
            "ChangeSetNotFound",
        ),
    ] {
        let (code, body) = describe_change_set(&router, change_set, stack_name).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected HTTP {expected}, got {code} {body}"
        );
        if expected == 200 {
            assert_eq!(
                xml_text(&body, "ChangeSetId").as_deref(),
                Some(change_set_id.as_str()),
                "case {case:?}: the change set reports its own id, got {body}"
            );
            assert_eq!(
                xml_text(&body, "StackName").as_deref(),
                Some("by-arn"),
                "case {case:?}: and the stack it belongs to, got {body}"
            );
        } else {
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some(expected_code),
                "case {case:?}: expected the {expected_code} error, got {body}"
            );
        }
    }
}

/// One row of a request-refusal table: the request to make, and the status and wire error
/// code it must be answered with.
struct RefusalCase {
    case: &'static str,
    members: Vec<(String, String)>,
    status: u16,
    error_code: &'static str,
}

#[tokio::test]
async fn create_change_set_refuses_what_it_cannot_plan() {
    let (router, engine) = router_with_engine();
    create_stack(
        &router,
        "taken",
        r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#,
        &[],
    )
    .await;
    settle_the_store(&router, &engine).await;
    let template = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#;
    create_creating_change_set(&router, "dup", "dup-set", template).await;

    let cases = vec![
        RefusalCase {
            case: "a CREATE change set for a stack that already exists",
            members: vec![
                member("StackName", "taken"),
                member("ChangeSetName", "cs"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", template),
            ],
            status: 400,
            // The REQUEST cannot be satisfied — a stack that exists cannot be created
            // again — rather than a change-set name colliding, which is what the modelled
            // AlreadyExistsException names.
            error_code: "ValidationError",
        },
        RefusalCase {
            case: "a change set name the stack already carries, with different attributes",
            members: vec![
                member("StackName", "dup"),
                member("ChangeSetName", "dup-set"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", template),
                member("Description", "not the stored one"),
            ],
            status: 400,
            error_code: "AlreadyExistsException",
        },
        RefusalCase {
            case: "an UPDATE change set for a stack that does not exist",
            members: vec![
                member("StackName", "absent"),
                member("ChangeSetName", "cs"),
                member("TemplateBody", template),
            ],
            status: 400,
            error_code: "ValidationError",
        },
        RefusalCase {
            case: "no template source at all",
            members: vec![member("StackName", "taken"), member("ChangeSetName", "cs")],
            status: 400,
            error_code: "ValidationError",
        },
        RefusalCase {
            case: "two template sources",
            members: vec![
                member("StackName", "taken"),
                member("ChangeSetName", "cs"),
                member("TemplateBody", template),
                member("UsePreviousTemplate", "true"),
            ],
            status: 400,
            error_code: "ValidationError",
        },
        RefusalCase {
            case: "a template that declares no resources",
            members: vec![
                member("StackName", "new-empty"),
                member("ChangeSetName", "cs"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", r#"{"Description":"nothing"}"#),
            ],
            status: 400,
            error_code: "ValidationError",
        },
        RefusalCase {
            case: "a resource with no Type",
            members: vec![
                member("StackName", "new-typeless"),
                member("ChangeSetName", "cs"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", r#"{"Resources":{"A":{"Properties":{}}}}"#),
            ],
            status: 400,
            error_code: "ValidationError",
        },
        RefusalCase {
            case: "resources to import without ChangeSetType IMPORT",
            members: vec![
                member("StackName", "taken"),
                member("ChangeSetName", "cs"),
                member("TemplateBody", template),
                member("ResourcesToImport.member.1.ResourceType", "AWS::S3::Bucket"),
                member("ResourcesToImport.member.1.LogicalResourceId", "A"),
                member(
                    "ResourcesToImport.member.1.ResourceIdentifier.entry.1.key",
                    "BucketName",
                ),
                member(
                    "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                    "b",
                ),
            ],
            status: 400,
            error_code: "ValidationError",
        },
        // A `TemplateURL` is RESOLVED: the object it addresses is read out of this
        // emulator's own Amazon S3, so a URL naming a bucket nothing created is the
        // request's own bad reference.
        RefusalCase {
            case: "a template URL naming no object this account can read",
            members: vec![
                member("StackName", "s3-template"),
                member("ChangeSetName", "cs"),
                member("ChangeSetType", "CREATE"),
                member("TemplateURL", "https://s3.amazonaws.com/b/t.json"),
            ],
            status: 400,
            error_code: "ValidationError",
        },
        RefusalCase {
            case: "an IMPORT that names no resource to import",
            members: vec![
                member("StackName", "import"),
                member("ChangeSetName", "cs"),
                member("ChangeSetType", "IMPORT"),
                member(
                    "TemplateBody",
                    r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#,
                ),
            ],
            status: 400,
            error_code: "ValidationError",
        },
        RefusalCase {
            case: "a template body that is neither JSON nor YAML",
            members: vec![
                member("StackName", "unreadable"),
                member("ChangeSetName", "cs"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", "{not a template at all"),
            ],
            status: 400,
            error_code: "ValidationError",
        },
    ];

    for case in cases {
        let (code, body) = call(&router, "CreateChangeSet", &case.members).await;
        assert_eq!(
            code, case.status,
            "case {:?}: expected HTTP {}, got {code} {body}",
            case.case, case.status
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some(case.error_code),
            "case {:?}: expected the {} error, got {body}",
            case.case,
            case.error_code
        );
    }

    // Nothing above created a stack or a change set beyond the two this case seeded.
    let (_, body) = call(&router, "ListStacks", &[]).await;
    assert_eq!(
        xml_texts(&body, "StackName"),
        vec!["taken".to_string(), "dup".to_string()],
        "a refused change set must not have created a stack, got {body}"
    );
}

#[tokio::test]
async fn a_change_set_is_deleted_by_request_by_execution_and_with_its_stack() {
    let (router, engine) = router_with_engine();
    let template = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#;

    // Deleting one the store never had succeeds and changes nothing: the model declares no
    // not-found error for DeleteChangeSet.
    let (code, body) = call(
        &router,
        "DeleteChangeSet",
        &[member("ChangeSetName", "never"), member("StackName", "gone")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"deleting a change set that is not there\": it succeeds, got {code} {body}"
    );

    create_creating_change_set(&router, "dropped", "dropped-set", template).await;
    // A plan CloudFormation is still computing is not one it discards, so the change set's
    // own creation is settled first — `change_set_status_refusals.rs` is where that refusal
    // is held to its wording.
    settle_the_store(&router, &engine).await;
    let (code, body) = call(
        &router,
        "DeleteChangeSet",
        &[
            member("ChangeSetName", "dropped-set"),
            member("StackName", "dropped"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"deleting a change set\": it succeeds, got {code} {body}"
    );
    let (code, body) = describe_change_set(&router, "dropped-set", Some("dropped")).await;
    assert_eq!(
        code, 404,
        "case \"deleting a change set\": it is gone afterwards, got {code} {body}"
    );
    // The stack it planned for is NOT deleted with it: deleting a change set discards a
    // plan.
    let (code, _) = describe_stack(&router, "dropped").await;
    assert_eq!(
        code, 200,
        "case \"deleting a change set\": the stack it planned for stays"
    );

    // "When you execute a change set, CloudFormation deletes all other change sets
    // associated with the stack because they aren't valid for the updated stack."
    create_creating_change_set(&router, "siblings", "first-set", template).await;
    create_change_set(
        &router,
        "siblings",
        "second-set",
        &[
            member("ChangeSetType", "CREATE"),
            member(
                "TemplateBody",
                r#"{"Resources":{"B":{"Type":"AWS::Logs::LogGroup"}}}"#,
            ),
        ],
    )
    .await;
    settle_the_store(&router, &engine).await;
    let (code, body) = execute_change_set(&router, "first-set", Some("siblings")).await;
    assert_eq!(code, 200, "the first change set executes, got {code} {body}");
    let (code, body) = describe_change_set(&router, "second-set", Some("siblings")).await;
    assert_eq!(
        code, 404,
        "case \"executing a change set\": the stack's other change sets are gone, got {code} \
         {body}"
    );
    let (code, _) = describe_change_set(&router, "first-set", Some("siblings")).await;
    assert_eq!(
        code, 200,
        "case \"executing a change set\": the executed one stays addressable"
    );

    // A stack's change sets die with the stack.
    settle_the_store(&router, &engine).await;
    let (code, body) = call(&router, "DeleteStack", &[member("StackName", "siblings")]).await;
    assert_eq!(code, 200, "the stack is deleted, got {code} {body}");
    settle_the_store(&router, &engine).await;
    // The stack is resolved BEFORE the change set, so a read addressing one of its change
    // sets by name is now refused about the STACK rather than about the change set.
    let (code, body) = describe_change_set(&router, "first-set", Some("siblings")).await;
    assert_eq!(
        code, 400,
        "case \"deleting a stack\": the stack it addressed is gone, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some("Stack [siblings] does not exist"),
        "case \"deleting a stack\": expected the refusal to name the stack, got {body}"
    );
    assert!(
        store(&engine).lock().change_sets.is_empty(),
        "case \"deleting a stack\": no change set record outlives its stack"
    );
}

/// Rule: the operation a stack reports in `LastOperations` is the one that last ACTED on
/// it. Creating a change set acts on the stack only when it is the change set that brings
/// that stack into existence; on a stack that already exists a change set changes nothing
/// until it is executed, and the execution is a stack create or update.
#[tokio::test]
async fn the_stacks_last_operation_is_the_one_that_last_acted_on_it() {
    let (router, engine) = router_with_engine();
    let template = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#;
    let next = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"x"}}}}"#;

    // (case, the OperationType the stack must report after the step)
    create_creating_change_set(&router, "ops", "ops-create", template).await;
    let (_, body) = describe_stack(&router, "ops").await;
    assert_eq!(
        xml_text(&body, "OperationType").as_deref(),
        Some("CREATE_CHANGESET"),
        "case \"after the change set that creates the stack\": the change set is what acted \
         on it, got {body}"
    );

    settle_the_store(&router, &engine).await;
    execute_change_set(&router, "ops-create", Some("ops")).await;
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_stack(&router, "ops").await;
    assert_eq!(
        xml_text(&body, "OperationType").as_deref(),
        Some("CREATE_STACK"),
        "case \"after executing the change set that creates the stack\": the stack was \
         created, got {body}"
    );

    // A change set for a stack that already exists leaves its last operation alone.
    create_change_set(
        &router,
        "ops",
        "ops-update",
        &[member("TemplateBody", next)],
    )
    .await;
    let (_, body) = describe_stack(&router, "ops").await;
    assert_eq!(
        xml_text(&body, "OperationType").as_deref(),
        Some("CREATE_STACK"),
        "case \"after a change set for a stack that already exists\": nothing acted on the \
         stack, got {body}"
    );
    settle_the_store(&router, &engine).await;
    execute_change_set(&router, "ops-update", Some("ops")).await;
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_stack(&router, "ops").await;
    assert_eq!(
        xml_text(&body, "OperationType").as_deref(),
        Some("UPDATE_STACK"),
        "case \"after executing an UPDATE change set\": the stack was updated, got {body}"
    );
    // Every stack reports the envelope members CloudFormation always carries, empty for a
    // stack that has none of them.
    assert!(
        body.contains("<NotificationARNs>") && body.contains("<Tags>"),
        "case \"a stack with no topics and no tags\": the members are still reported, got \
         {body}"
    );
    assert_eq!(
        xml_text(&body, "Mode").as_deref(),
        Some("STANDARD"),
        "case \"a stack deployed with no explicit mode\": the default mode is reported, got \
         {body}"
    );
}

#[tokio::test]
async fn one_stack_holds_at_most_the_change_set_quota() {
    let (router, _engine) = router_with_engine();
    let template = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#;
    create_creating_change_set(&router, "quota", "set-0", template).await;

    for index in 1..MAX_CHANGE_SETS_PER_STACK {
        let (code, body) = create_change_set(
            &router,
            "quota",
            &format!("set-{index}"),
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", template),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case \"change set {index} of the quota\": it is accepted, got {code} {body}"
        );
    }
    let (code, body) = create_change_set(
        &router,
        "quota",
        "one-too-many",
        &[
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", template),
        ],
    )
    .await;
    assert_eq!(
        code, 400,
        "case \"past the per-stack change set quota\": it is refused, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("LimitExceededException"),
        "case \"past the per-stack change set quota\": with the modelled error, got {body}"
    );
}

#[tokio::test]
async fn a_retried_create_change_set_with_the_same_client_token_creates_one_change_set() {
    let (router, engine) = router_with_engine();
    let template = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#;
    let members = [
        member("ChangeSetType", "CREATE"),
        member("TemplateBody", template),
        member("ClientToken", "token-1"),
    ];
    let (code, first) = create_change_set(&router, "idem", "idem-set", &members).await;
    assert_eq!(code, 200, "the first create succeeds, got {code} {first}");
    let (code, second) = create_change_set(&router, "idem", "idem-set", &members).await;
    assert_eq!(code, 200, "the retry succeeds too, got {code} {second}");
    assert_eq!(
        xml_text(&second, "Id"),
        xml_text(&first, "Id"),
        "case \"a retried create\": the same change set is answered, got {second}"
    );
    assert_eq!(
        store(&engine).lock().change_sets.len(),
        1,
        "case \"a retried create\": exactly one change set was created"
    );
    assert_eq!(
        store(&engine).lock().stacks.len(),
        1,
        "case \"a retried create\": exactly one stack was created"
    );
}
