// SPDX-License-Identifier: CC0-1.0

//! `RollbackStack` — the rollback a CLIENT asks for — end to end over the awsQuery wire.
//!
//! RULE: a stack whose last operation failed and PRESERVED what it had provisioned is
//! rolled back, on request, to its last known stable state: with no such state the
//! resources that operation had provisioned are deleted again in the services that own
//! them and the stack settles in the terminal create-rollback status, and with one the
//! configuration in force before that operation is put back there and the stack settles in
//! the terminal update-rollback status, leaving the stack updatable. A stack in any other
//! status is refused and left exactly where it is.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the wire harness, and reads the resources back through the OWNING service's own wire
//! (Systems Manager for an `AWS::SSM::Parameter`, IAM for an `AWS::IAM::Role`) — so what it
//! asserts is what a client sees rather than a status word the stack published.
//!
//! The rows differ along what the rule generalises over: whether the stack holds a stable
//! state (a failed create versus a failed update), how the failed operation came to preserve
//! its state (`DisableRollback`, `OnFailure=DO_NOTHING`), what failed it (a wait condition
//! nothing signalled, which no call of the client's ends, versus an owning service's
//! refusal), and which direction the revert runs in (a changed resource put back, a resource
//! the failed update had removed created again, resources the failed operation had added
//! taken away). The refusal table spans the statuses a stack can be in when the call
//! arrives.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    changed_stack_does_not_exist, resource_status, stack_cannot_be_rolled_back,
    stack_does_not_exist, status,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, iam, member, router,
    wait_for_the_transition, xml_text, xml_texts,
};
use crate::update_rollback_recovery::{
    parameter, parameter_value, ssm, stack_events, status_of, template,
    template_with_an_unsignalled_gate, WAITED_OUT,
};

/// What has to hold the resource name a row's failing create asks for, so that create fails
/// on the owning service's own refusal rather than on the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Nothing: the row's operation is failed by a wait condition nothing signals.
    AnUnsignalledWaitCondition,
    /// An IAM role of this name already exists, and the row's template declares it — IAM
    /// refuses the second one, which fails the create after it has made the parameter.
    ARoleNameAlreadyTaken(&'static str),
}

/// One row of the rollback table.
struct Row {
    case: &'static str,
    stack: &'static str,
    setup: Setup,
    /// The template the stack is created from.
    created: String,
    /// The template an update then submits, for a row whose UPDATE is the operation that
    /// fails; `None` for a row whose CREATE fails.
    updated: Option<String>,
    /// The member that makes the failed operation preserve what it had provisioned, exactly
    /// as the request carries it.
    preserve: (&'static str, &'static str),
    /// The status the failed operation leaves the stack in.
    failed_status: &'static str,
    /// The status the requested rollback settles the stack in.
    settles_to: &'static str,
    /// `(parameter name, the value Parameter Store holds)` once the failed operation has
    /// been preserved and before the rollback is asked for; `None` is a parameter Parameter
    /// Store must not hold at all.
    before_rollback: Vec<(String, Option<&'static str>)>,
    /// The same, once the rollback has settled.
    after_rollback: Vec<(String, Option<&'static str>)>,
    /// Logical ids the rolled-back stack must still report, and the ones it must not.
    reported_after: Vec<&'static str>,
    absent_after: Vec<&'static str>,
}

/// A trust policy IAM accepts: an IAM JSON policy document carrying a `Statement`.
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

/// A parameter CloudFormation can create, declared first, and a role whose name is already
/// taken, declared second — so the create gets as far as the parameter and then fails, which
/// is what gives the rollback something to remove. The logical ids order the two whether the
/// template reader yields declaration order or sorted order.
fn parameter_and_a_taken_role(name: &str, value: &str, role: &str) -> String {
    template(json!({
        "AKeep": parameter(name, value),
        "ZRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": role,
                "AssumeRolePolicyDocument": trust_policy()
            }
        }
    }))
}

/// The class the rule covers: any stack whose last operation failed with what it had
/// provisioned preserved, whichever operation that was and however it failed.
fn rollback_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create with no stable state, held at a wait condition and preserved by \
                   DisableRollback",
            stack: "rb-create-disabled",
            setup: Setup::AnUnsignalledWaitCondition,
            created: template_with_an_unsignalled_gate(
                json!({"AKeep": parameter("/rb-create-disabled/p", "kept")}),
            ),
            updated: None,
            preserve: ("DisableRollback", "true"),
            failed_status: status::CREATE_FAILED,
            settles_to: status::ROLLBACK_COMPLETE,
            before_rollback: vec![("/rb-create-disabled/p".to_string(), Some("kept"))],
            after_rollback: vec![("/rb-create-disabled/p".to_string(), None)],
            reported_after: vec!["AKeep"],
            absent_after: vec![],
        },
        Row {
            case: "the same create, preserved by OnFailure=DO_NOTHING instead",
            stack: "rb-create-do-nothing",
            setup: Setup::AnUnsignalledWaitCondition,
            created: template_with_an_unsignalled_gate(
                json!({"AKeep": parameter("/rb-create-do-nothing/p", "kept")}),
            ),
            updated: None,
            preserve: ("OnFailure", "DO_NOTHING"),
            failed_status: status::CREATE_FAILED,
            settles_to: status::ROLLBACK_COMPLETE,
            before_rollback: vec![("/rb-create-do-nothing/p".to_string(), Some("kept"))],
            after_rollback: vec![("/rb-create-do-nothing/p".to_string(), None)],
            reported_after: vec!["AKeep"],
            absent_after: vec![],
        },
        Row {
            case: "a create whose second resource the owning service refused",
            stack: "rb-create-refused",
            setup: Setup::ARoleNameAlreadyTaken("rb-create-refused-role"),
            created: parameter_and_a_taken_role(
                "/rb-create-refused/p",
                "kept",
                "rb-create-refused-role",
            ),
            updated: None,
            preserve: ("DisableRollback", "true"),
            failed_status: status::CREATE_FAILED,
            settles_to: status::ROLLBACK_COMPLETE,
            before_rollback: vec![("/rb-create-refused/p".to_string(), Some("kept"))],
            after_rollback: vec![("/rb-create-refused/p".to_string(), None)],
            reported_after: vec!["AKeep"],
            absent_after: vec![],
        },
        Row {
            case: "a failed update is put back to the configuration before it",
            stack: "rb-update-changed",
            setup: Setup::AnUnsignalledWaitCondition,
            created: template(json!({"Param": parameter("/rb-update-changed/p", "v1")})),
            updated: Some(template_with_an_unsignalled_gate(
                json!({"Param": parameter("/rb-update-changed/p", "v2")}),
            )),
            preserve: ("DisableRollback", "true"),
            failed_status: status::UPDATE_FAILED,
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
            before_rollback: vec![("/rb-update-changed/p".to_string(), Some("v2"))],
            after_rollback: vec![("/rb-update-changed/p".to_string(), Some("v1"))],
            reported_after: vec!["Param"],
            // The resources the failed update added are not the stack's: it installed
            // nothing.
            absent_after: vec!["Gate", "Wait"],
        },
        Row {
            case: "a failed update that had removed a resource has it created again",
            stack: "rb-update-dropped",
            setup: Setup::AnUnsignalledWaitCondition,
            created: template(json!({
                "Param": parameter("/rb-update-dropped/p", "keep"),
                "Dropped": parameter("/rb-update-dropped/d", "dropped"),
            })),
            updated: Some(template_with_an_unsignalled_gate(
                json!({"Param": parameter("/rb-update-dropped/p", "keep")}),
            )),
            preserve: ("DisableRollback", "true"),
            failed_status: status::UPDATE_FAILED,
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
            before_rollback: vec![("/rb-update-dropped/d".to_string(), None)],
            after_rollback: vec![
                ("/rb-update-dropped/p".to_string(), Some("keep")),
                ("/rb-update-dropped/d".to_string(), Some("dropped")),
            ],
            reported_after: vec!["Param", "Dropped"],
            absent_after: vec!["Gate", "Wait"],
        },
    ]
}

/// Every `LogicalResourceId` `ListStackResources` reports for a stack.
async fn listed_resources(case: &str, router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (http_status, body) =
        call(router, "ListStackResources", &[member("StackName", stack)]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected ListStackResources to succeed, got {http_status} {body}"
    );
    xml_texts(&body, "LogicalResourceId")
}

/// `RollbackStack`, with whatever else the case needs.
async fn rollback_stack(
    router: &MultiServiceRouter,
    stack: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("StackName", stack)];
    members.extend_from_slice(extra);
    call(router, "RollbackStack", &members).await
}

#[tokio::test]
async fn a_requested_rollback_puts_a_failed_stack_back_to_its_last_known_stable_state() {
    for row in rollback_rows() {
        let router = router();
        let mut extra = vec![member(row.preserve.0, row.preserve.1)];
        if let Setup::ARoleNameAlreadyTaken(role) = row.setup {
            let (http_status, body) = iam(
                &router,
                "CreateRole",
                &[
                    ("RoleName", role),
                    ("AssumeRolePolicyDocument", &trust_policy().to_string()),
                ],
            )
            .await;
            assert_eq!(
                http_status, 200,
                "case {:?}: the role name must be taken first, got {http_status} {body}",
                row.case
            );
            // The template declares an IAM resource with a name of its own, which the
            // request has to acknowledge — and the refusal this row is about is IAM's own
            // answer to a name it already holds, which the create reaches only with the
            // pre-deployment validations off: with them on, the existence check fails the
            // create before any resource is asked for (`state::early_validation`).
            extra.push(member("DisableValidation", "true"));
            extra = acknowledging_iam(&extra);
        }

        // The stack, created from the row's template — with the members that preserve a
        // failed state on the CREATE for a row whose create is what fails.
        let creating = row.updated.is_none();
        let (http_status, body) = create_stack(
            &router,
            row.stack,
            &row.created,
            if creating { &extra } else { &[] },
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
            row.case
        );
        if let Some(updated) = &row.updated {
            wait_for_the_transition().await;
            let created = status_of(&router, row.stack).await;
            assert_eq!(
                created,
                status::CREATE_COMPLETE,
                "case {:?}: expected the create to complete before the update, got \
                 {created:?}",
                row.case
            );
            let mut members = vec![
                member("StackName", row.stack),
                member("TemplateBody", updated),
            ];
            members.extend_from_slice(&extra);
            let (http_status, body) = call(&router, "UpdateStack", &members).await;
            assert_eq!(
                http_status, 200,
                "case {:?}: expected UpdateStack to succeed, got {http_status} {body}",
                row.case
            );
        }

        // The failed operation PRESERVED what it had provisioned, which is the state this
        // rollback exists to clear up: a wait condition nothing signals runs out with no
        // call of the client's, and an owning service's refusal lands at once.
        match row.setup {
            Setup::AnUnsignalledWaitCondition => tokio::time::sleep(WAITED_OUT).await,
            Setup::ARoleNameAlreadyTaken(_) => wait_for_the_transition().await,
        }
        let failed = status_of(&router, row.stack).await;
        assert_eq!(
            failed, row.failed_status,
            "case {:?}: expected the failed operation to leave the stack in {}, got \
             {failed:?}",
            row.case, row.failed_status
        );
        for (name, expected) in &row.before_rollback {
            let held = parameter_value(&router, name).await;
            assert_eq!(
                held.as_deref(),
                *expected,
                "case {:?}: expected Parameter Store to hold {expected:?} for {name:?} while \
                 the failed state was preserved, got {held:?}",
                row.case
            );
        }

        // The rollback the client asks for.
        let stack_id = {
            let (_, described) = describe_stack(&router, row.stack).await;
            xml_text(&described, "StackId").unwrap_or_default()
        };
        let (http_status, body) = rollback_stack(&router, row.stack, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected RollbackStack to succeed, got {http_status} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "StackId").as_deref(),
            Some(stack_id.as_str()),
            "case {:?}: expected the rollback to answer the stack's own id, got {body}",
            row.case
        );
        let operation_id = xml_text(&body, "OperationId").unwrap_or_default();
        assert!(
            !operation_id.is_empty(),
            "case {:?}: expected the rollback to answer an operation id, got {body}",
            row.case
        );

        wait_for_the_transition().await;
        let settled = status_of(&router, row.stack).await;
        assert_eq!(
            settled, row.settles_to,
            "case {:?}: expected the rollback to settle the stack at {}, got {settled:?}",
            row.case, row.settles_to
        );
        for (name, expected) in &row.after_rollback {
            let held = parameter_value(&router, name).await;
            assert_eq!(
                held.as_deref(),
                *expected,
                "case {:?}: expected Parameter Store to hold {expected:?} for {name:?} once \
                 the rollback had settled, got {held:?}",
                row.case
            );
        }
        // A parameter the rollback DELETED is refused by Parameter Store itself, rather
        // than answered with a value nothing holds.
        for (name, _) in row
            .after_rollback
            .iter()
            .filter(|(_, expected)| expected.is_none())
        {
            let (http_status, body) = ssm(&router, "GetParameter", &json!({"Name": name})).await;
            assert_eq!(
                http_status, 400,
                "case {:?}: expected Parameter Store to refuse {name:?} once the rollback had \
                 deleted it, got {http_status} {body}",
                row.case
            );
            assert!(
                body.to_string().contains("ParameterNotFound"),
                "case {:?}: expected the refusal of {name:?} to be ParameterNotFound, got \
                 {body}",
                row.case
            );
        }

        // What the rolled-back stack reports: every resource it still names, and none of the
        // ones the failed operation had added.
        let listed = listed_resources(row.case, &router, row.stack).await;
        for logical_id in &row.reported_after {
            assert!(
                listed.iter().any(|reported| reported == logical_id),
                "case {:?}: expected the rolled-back stack to report {logical_id:?}, got \
                 {listed:?}",
                row.case
            );
        }
        for logical_id in &row.absent_after {
            assert!(
                !listed.iter().any(|reported| reported == logical_id),
                "case {:?}: expected the rolled-back stack not to report {logical_id:?}, got \
                 {listed:?}",
                row.case
            );
        }

        // The stack's own events say what the rollback did: it opened, under its own
        // operation id, and then concluded.
        let events = stack_events(row.case, &router, row.stack).await;
        let opened = format!(
            "{}{}",
            row.settles_to
                .strip_suffix("_COMPLETE")
                .expect("a terminal rollback status ends in _COMPLETE"),
            "_IN_PROGRESS"
        );
        assert!(
            events.contains(&(row.stack.to_string(), opened.clone())),
            "case {:?}: expected the stack's own {opened} event, got {events:?}",
            row.case
        );
        assert!(
            events.contains(&(row.stack.to_string(), row.settles_to.to_string())),
            "case {:?}: expected the stack's own {} event, got {events:?}",
            row.case,
            row.settles_to
        );

        // A rollback with no stable state to go back to disposed of the stack's resources:
        // the records stay, at the terminal delete status, which is what says which resource
        // it disposed of.
        if row.settles_to == status::ROLLBACK_COMPLETE {
            for logical_id in &row.reported_after {
                let (http_status, body) = call(
                    &router,
                    "DescribeStackResource",
                    &[
                        member("StackName", row.stack),
                        member("LogicalResourceId", logical_id),
                    ],
                )
                .await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: expected {logical_id:?} to still be reported, got \
                     {http_status} {body}",
                    row.case
                );
                assert_eq!(
                    xml_text(&body, "ResourceStatus").as_deref(),
                    Some(resource_status::DELETE_COMPLETE),
                    "case {:?}: expected the rollback's terminal status for {logical_id:?}, \
                     got {body}",
                    row.case
                );
            }
        }

        // Whatever the rollback did, the role it never created is untouched: it was never
        // this stack's to remove.
        if let Setup::ARoleNameAlreadyTaken(role) = row.setup {
            let (http_status, body) = iam(&router, "GetRole", &[("RoleName", role)]).await;
            assert_eq!(
                http_status, 200,
                "case {:?}: the role that was already there must survive the rollback, got \
                 {http_status} {body}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_rolled_back_update_leaves_the_stack_updatable() {
    let router = router();
    let (http_status, body) = create_stack(
        &router,
        "rb-updatable",
        &template(json!({"Param": parameter("/rb-updatable/p", "v1")})),
        &[],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;

    let (http_status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "rb-updatable"),
            member(
                "TemplateBody",
                &template_with_an_unsignalled_gate(
                    json!({"Param": parameter("/rb-updatable/p", "v2")}),
                ),
            ),
            member("DisableRollback", "true"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected the failing UpdateStack to be accepted, got {http_status} {body}"
    );
    tokio::time::sleep(WAITED_OUT).await;
    assert_eq!(
        status_of(&router, "rb-updatable").await,
        status::UPDATE_FAILED,
        "expected the unsignalled wait condition to fail the update"
    );

    let (http_status, body) = call(
        &router,
        "RollbackStack",
        &[member("StackName", "rb-updatable")],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected RollbackStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, "rb-updatable").await,
        status::UPDATE_ROLLBACK_COMPLETE,
        "expected the requested rollback to settle the stack"
    );

    // "A last known stable state includes any status in a *_COMPLETE": the rolled-back stack
    // is in one, so the next update is accepted and carries through.
    let (http_status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "rb-updatable"),
            member(
                "TemplateBody",
                &template(json!({"Param": parameter("/rb-updatable/p", "v3")})),
            ),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected the update after the rollback to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, "rb-updatable").await,
        status::UPDATE_COMPLETE,
        "expected the update after the rollback to complete"
    );
    let held = parameter_value(&router, "/rb-updatable/p").await;
    assert_eq!(
        held.as_deref(),
        Some("v3"),
        "expected the update after the rollback to reach Parameter Store, got {held:?}"
    );
}

#[tokio::test]
async fn the_rollback_leg_does_not_depend_on_the_bounded_event_log() {
    // A stack's event log is bounded and evicts oldest-first, so a busy stack loses the
    // event that announced its create. The stable state a rollback goes back to must not be
    // read from there: this case empties the log, exactly as the bound does, and the
    // rollback must still REVERT rather than delete everything the stack holds.
    let (router, engine) = crate::stack_lifecycle::router_with_engine();
    let (http_status, body) = create_stack(
        &router,
        "rb-evicted",
        &template(json!({"Param": parameter("/rb-evicted/p", "v1")})),
        &[],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, "rb-evicted").await,
        status::CREATE_COMPLETE,
        "expected the create to complete"
    );

    {
        let state = crate::stack_lifecycle::store(&engine);
        let mut store = state.lock();
        let stack = store
            .stacks
            .iter_mut()
            .find(|stack| stack.stack_name == "rb-evicted")
            .expect("the stack the create made");
        stack.events.clear();
    }

    let (http_status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "rb-evicted"),
            member(
                "TemplateBody",
                &template_with_an_unsignalled_gate(
                    json!({"Param": parameter("/rb-evicted/p", "v2")}),
                ),
            ),
            member("DisableRollback", "true"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected the failing UpdateStack to be accepted, got {http_status} {body}"
    );
    tokio::time::sleep(WAITED_OUT).await;
    assert_eq!(
        status_of(&router, "rb-evicted").await,
        status::UPDATE_FAILED,
        "expected the unsignalled wait condition to fail the update"
    );

    let (http_status, body) = rollback_stack(&router, "rb-evicted", &[]).await;
    assert_eq!(
        http_status, 200,
        "expected RollbackStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, "rb-evicted").await,
        status::UPDATE_ROLLBACK_COMPLETE,
        "expected the rollback to revert the update rather than discard the stack"
    );
    let held = parameter_value(&router, "/rb-evicted/p").await;
    assert_eq!(
        held.as_deref(),
        Some("v1"),
        "expected the resource to be put back rather than deleted, got {held:?}"
    );
}

/// One row of the not-found table: the call, and the members it needs besides `StackName`.
struct AbsentStackRow {
    case: &'static str,
    action: &'static str,
    extra: Vec<(String, String)>,
    /// Whether the call is one that asks the stack to CHANGE, which is what decides how the
    /// refusal names it.
    changes_the_stack: bool,
}

#[tokio::test]
async fn a_call_that_asks_an_absent_stack_to_change_names_it_in_square_brackets() {
    let router = router();
    // A stack that was created and deleted, beside a name nothing ever created: both are
    // unaddressable, and the refusal is worded the same way for either.
    let (http_status, body) = create_stack(
        &router,
        "rb-gone",
        &template(json!({"Param": parameter("/rb-gone/p", "v1")})),
        &[],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    call(&router, "DeleteStack", &[member("StackName", "rb-gone")]).await;
    wait_for_the_transition().await;

    let rows = vec![
        AbsentStackRow {
            case: "an update of a stack that is not there",
            action: "UpdateStack",
            extra: vec![member("TemplateBody", &template(json!({"Param": parameter("/x/p", "v1")})))],
            changes_the_stack: true,
        },
        AbsentStackRow {
            case: "setting termination protection on a stack that is not there",
            action: "UpdateTerminationProtection",
            extra: vec![member("EnableTerminationProtection", "true")],
            changes_the_stack: true,
        },
        AbsentStackRow {
            case: "rolling back a stack that is not there",
            action: "RollbackStack",
            extra: vec![],
            changes_the_stack: true,
        },
        // The control: a READ of the same stack keeps its own wording, which is the one
        // difference between the two templates.
        AbsentStackRow {
            case: "a read of a stack that is not there",
            action: "DescribeStacks",
            extra: vec![],
            changes_the_stack: false,
        },
    ];

    for identifier in ["rb-gone", "rb-never-created"] {
        for row in &rows {
            let mut members = vec![member("StackName", identifier)];
            members.extend_from_slice(&row.extra);
            let (http_status, body) = call(&router, row.action, &members).await;
            assert_eq!(
                http_status, 400,
                "case {:?}: {} must refuse {identifier:?}, got {http_status} {body}",
                row.case, row.action
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {:?}: expected a ValidationError, got {body}",
                row.case
            );
            let expected = if row.changes_the_stack {
                changed_stack_does_not_exist(identifier)
            } else {
                stack_does_not_exist(identifier)
            };
            assert_eq!(
                xml_text(&body, "Message").unwrap_or_default(),
                expected,
                "case {:?}: expected {expected:?} from {}, got {body}",
                row.case,
                row.action
            );
        }
    }
}

/// One row of the refusal table: how the stack gets into the status it is in when the
/// rollback arrives.
struct RefusedRow {
    case: &'static str,
    stack: &'static str,
    /// The status the stack is in when `RollbackStack` arrives, or `None` for a stack that
    /// is not there at all.
    status: Option<&'static str>,
}

#[tokio::test]
async fn only_a_stack_whose_last_operation_failed_takes_a_rollback() {
    let rows = [
        RefusedRow {
            case: "a stack whose create completed",
            stack: "rb-complete",
            status: Some(status::CREATE_COMPLETE),
        },
        RefusedRow {
            case: "a stack whose update completed",
            stack: "rb-updated",
            status: Some(status::UPDATE_COMPLETE),
        },
        RefusedRow {
            case: "a stack whose create was already rolled back",
            stack: "rb-rolled-back",
            status: Some(status::ROLLBACK_COMPLETE),
        },
        RefusedRow {
            case: "a name no stack of this Region holds",
            stack: "rb-absent",
            status: None,
        },
    ];

    for row in rows {
        let router = router();
        match row.status {
            None => {}
            Some(status::ROLLBACK_COMPLETE) => {
                // A create whose resource an owning service refuses, with nothing asking for
                // the failed state to be kept: CloudFormation rolls it back itself.
                let (http_status, body) = iam(
                    &router,
                    "CreateRole",
                    &[
                        ("RoleName", "rb-rolled-back-role"),
                        ("AssumeRolePolicyDocument", &trust_policy().to_string()),
                    ],
                )
                .await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: the role name must be taken first, got {http_status} {body}",
                    row.case
                );
                let (http_status, body) = create_stack(
                    &router,
                    row.stack,
                    &parameter_and_a_taken_role(
                        "/rb-rolled-back/p",
                        "kept",
                        "rb-rolled-back-role",
                    ),
                    &acknowledging_iam(&[]),
                )
                .await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
                    row.case
                );
                wait_for_the_transition().await;
            }
            Some(expected) => {
                let (http_status, body) = create_stack(
                    &router,
                    row.stack,
                    &template(json!({"Param": parameter(&format!("/{}/p", row.stack), "v1")})),
                    &[],
                )
                .await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
                    row.case
                );
                wait_for_the_transition().await;
                if expected == status::UPDATE_COMPLETE {
                    let (http_status, body) = call(
                        &router,
                        "UpdateStack",
                        &[
                            member("StackName", row.stack),
                            member(
                                "TemplateBody",
                                &template(json!({
                                    "Param": parameter(&format!("/{}/p", row.stack), "v2")
                                })),
                            ),
                        ],
                    )
                    .await;
                    assert_eq!(
                        http_status, 200,
                        "case {:?}: expected UpdateStack to succeed, got {http_status} {body}",
                        row.case
                    );
                    wait_for_the_transition().await;
                }
            }
        }
        if let Some(expected) = row.status {
            let reported = status_of(&router, row.stack).await;
            assert_eq!(
                reported, expected,
                "case {:?}: expected the stack to be in {expected} before the rollback, got \
                 {reported:?}",
                row.case
            );
        }

        let (http_status, body) = rollback_stack(&router, row.stack, &[]).await;
        assert_eq!(
            http_status, 400,
            "case {:?}: expected the rollback to be refused, got {http_status} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected a ValidationError, got {body}",
            row.case
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        match row.status {
            Some(expected) => {
                assert_eq!(
                    message,
                    stack_cannot_be_rolled_back(),
                    "case {:?}: expected the refusal to state the statuses a rollback can be \
                     called from, got {message:?}",
                    row.case
                );
                // The stack's own id and the status it is in are this run's state, and the
                // fixed sentence carries neither.
                let (_, described) = describe_stack(&router, row.stack).await;
                let stack_id = xml_text(&described, "StackId").unwrap_or_default();
                for leaked in [stack_id.as_str(), expected] {
                    assert!(
                        !message.contains(leaked),
                        "case {:?}: expected the refusal to carry no per-run value, got \
                         {leaked:?} in {message:?}",
                        row.case
                    );
                }
                // The stack is left exactly where it was.
                let reported = status_of(&router, row.stack).await;
                assert_eq!(
                    reported, expected,
                    "case {:?}: expected the refused rollback to change nothing, got \
                     {reported:?}",
                    row.case
                );
            }
            // A call that asks a stack to CHANGE names the stack it could not find in
            // square brackets, which is not how a read of the same stack words it.
            None => assert_eq!(
                message,
                changed_stack_does_not_exist(row.stack),
                "case {:?}: expected the refusal to name the stack, got {message:?}",
                row.case
            ),
        }
    }
}

#[tokio::test]
async fn a_rollback_under_a_token_another_operation_claimed_is_refused() {
    let router = router();
    let (http_status, body) = create_stack(
        &router,
        "rb-token",
        &template_with_an_unsignalled_gate(json!({"Param": parameter("/rb-token/p", "kept")})),
        &[
            member("DisableRollback", "true"),
            member("ClientRequestToken", "the-create-token"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    tokio::time::sleep(WAITED_OUT).await;
    assert_eq!(
        status_of(&router, "rb-token").await,
        status::CREATE_FAILED,
        "expected the unsignalled wait condition to fail the create"
    );

    // "A client request token already exists": the token names the create, so a rollback
    // presenting it is the duplicate submission the modelled error refuses, and nothing is
    // rolled back.
    let (http_status, body) = rollback_stack(
        &router,
        "rb-token",
        &[member("ClientRequestToken", "the-create-token")],
    )
    .await;
    assert_eq!(
        http_status, 400,
        "expected the reused token to be refused, got {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("TokenAlreadyExistsException"),
        "expected the modelled token error, got {body}"
    );
    assert_eq!(
        status_of(&router, "rb-token").await,
        status::CREATE_FAILED,
        "expected the refused rollback to change nothing"
    );
    let held = parameter_value(&router, "/rb-token/p").await;
    assert_eq!(
        held.as_deref(),
        Some("kept"),
        "expected the refused rollback to delete nothing, got {held:?}"
    );

    // A token of its own is accepted, and the rollback goes through.
    let (http_status, body) = rollback_stack(
        &router,
        "rb-token",
        &[member("ClientRequestToken", "the-rollback-token")],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected a rollback under its own token to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, "rb-token").await,
        status::ROLLBACK_COMPLETE,
        "expected the rollback to settle the stack"
    );
    let held = parameter_value(&router, "/rb-token/p").await;
    assert_eq!(
        held, None,
        "expected the rollback to delete what the failed create had made, got {held:?}"
    );
}
