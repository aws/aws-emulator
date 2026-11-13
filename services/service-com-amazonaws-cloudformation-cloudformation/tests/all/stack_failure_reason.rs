// SPDX-License-Identifier: CC0-1.0

//! What a stack reports as its `StackStatusReason` when its RESOURCES are what failed the
//! operation, end to end over the awsQuery wire.
//!
//! RULE under test: **the stack-level `StackStatusReason` of an operation its resources
//! failed is one fixed summary sentence — `The following resource(s) failed to <action>:
//! [<comma-separated LogicalResourceIds>]. `, closed by its separating space — and carries
//! nothing about WHY any of them failed.** That diagnosis is a per-resource field:
//! `StackEvent$ResourceStatusReason` is the "success/failure message associated with the
//! resource", so the owning service's own refusal travels on the event of the resource that
//! failed and is read through `DescribeStackEvents`.
//!
//! Why it matters: deployment tooling parses the stack-level field to recover the SET of
//! failed logical ids and to decide whether a failure is resource-level at all. A provider's
//! error text appended to it makes that field's content depend on the refusal's wording, and
//! hides the layering an operator is taught to use — the per-resource cause in the event
//! stream.
//!
//! The rows differ along what the rule generalises over: the ACTION the operation was
//! carrying out (create, update), the logical id of the resource that failed, the service
//! that owns it (IAM, Amazon SNS, Amazon S3), and where the refusal comes from — the owning
//! service, or CloudFormation itself when a declaration describes no request that service
//! could be asked. The failed DELETE of the same rule is pinned by the router suite that has
//! a service refusing a delete (`cloudformation_provisions_stack_resources.rs`), and the
//! failure a resource SIGNAL causes by `resource_signals.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, iam, member, resource_event_reason,
    router, wait_for_the_transition, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status, rollback_requested, status,
};

/// `OnFailure=DO_NOTHING` keeps the failed state and whatever the create had already made,
/// so the stack stays in `CREATE_FAILED` with the reason the failure gave it: the reason is
/// read where a caller reads it rather than after a rollback has moved the stack on.
fn do_nothing(given: &Given) -> Vec<(String, String)> {
    let mut members = vec![member("OnFailure", "DO_NOTHING")];
    members.extend(deferring_the_conflict(given));
    acknowledging_iam(&members)
}

/// What a row whose refusal is the OWNING SERVICE's answer to a name it already holds has to
/// ask for: the pre-deployment validations OFF. With them on, the resource existence check
/// fails such a create before that service is asked at all, so nothing of its own wording
/// could be read — `DisableValidation` is what defers the conflict to provisioning
/// (`state::early_validation`).
fn deferring_the_conflict(given: &Given) -> Vec<(String, String)> {
    match given {
        Given::ARole(_) => vec![member("DisableValidation", "true")],
        Given::Nothing => Vec::new(),
    }
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

/// What a row needs to be there BEFORE the stack asks for it.
enum Given {
    /// Nothing: the declaration is refused on its own terms.
    Nothing,
    /// A role of this name, created through IAM the ordinary way, so the name the stack
    /// declares is already taken in the service that owns it.
    ARole(&'static str),
}

/// One row of the create table: the case, the stack name, the template, the logical id the
/// summary must name, and a fragment of the diagnosis the FAILING RESOURCE's own event must
/// carry — which the stack-level reason must NOT.
struct Refused {
    case: &'static str,
    stack: &'static str,
    given: Given,
    template: String,
    logical_id: &'static str,
    diagnosis: &'static str,
}

/// The rows: one per owning service and per origin of the refusal.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "an IAM role whose name is already taken",
            stack: "reason-taken-role",
            given: Given::ARole("reason-taken-role-name"),
            template: json!({"Resources": {"Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "reason-taken-role-name",
                    "AssumeRolePolicyDocument": trust_policy()
                }
            }}})
            .to_string(),
            logical_id: "Role",
            // IAM's own wording for a name it already holds.
            diagnosis: "already exists",
        },
        Refused {
            case: "a FIFO topic whose name does not carry the documented suffix",
            stack: "reason-fifo-topic",
            given: Given::Nothing,
            template: json!({"Resources": {"Notifications": {
                "Type": "AWS::SNS::Topic",
                "Properties": {"TopicName": "reason-fifo-topic-name", "FifoTopic": true}
            }}})
            .to_string(),
            logical_id: "Notifications",
            // Amazon SNS's own wording for a topic name that breaks its rule.
            diagnosis: "Invalid parameter: Topic Name",
        },
        Refused {
            case: "a declaration missing a property its type requires, refused before any \
                   service is asked",
            stack: "reason-trustless-role",
            given: Given::Nothing,
            template: json!({"Resources": {"Trustless": {
                "Type": "AWS::IAM::Role",
                "Properties": {"RoleName": "reason-trustless-role-name"}
            }}})
            .to_string(),
            logical_id: "Trustless",
            diagnosis: "AssumeRolePolicyDocument",
        },
    ]
}

/// Make a row's `Given` so.
async fn given(router: &MultiServiceRouter, case: &str, given: &Given) {
    match given {
        Given::Nothing => {}
        Given::ARole(name) => {
            let (code, body) = iam(
                router,
                "CreateRole",
                &[
                    ("RoleName", name),
                    ("AssumeRolePolicyDocument", &trust_policy().to_string()),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: the setup CreateRole must succeed, got {code} {body}"
            );
        }
    }
}

/// A resource an operation could not create leaves the stack reporting the summary sentence
/// alone, with the refusal on that resource's own event.
#[tokio::test]
async fn a_failed_create_reports_the_summary_sentence_alone() {
    for row in refused_rows() {
        let router = router();
        given(&router, row.case, &row.given).await;

        let (code, body) =
            create_stack(&router, row.stack, &row.template, &do_nothing(&row.given)).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must be accepted — the stack reports the outcome — got \
             {code} {body}",
            row.case
        );
        wait_for_the_transition().await;

        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_FAILED),
            "case {:?}: expected {} with OnFailure=DO_NOTHING, got {described}",
            row.case,
            status::CREATE_FAILED
        );

        let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
        let expected = resource_failure_summary(failed_action::CREATE, [row.logical_id]);
        assert_eq!(
            reason, expected,
            "case {:?}: expected the stack reason to be {expected:?}, got {reason:?}",
            row.case
        );
        // The sentence's own separating space is part of it, and survives the wire.
        assert!(
            reason.ends_with(". "),
            "case {:?}: expected the summary to end with its separating space, got \
             {reason:?}",
            row.case
        );
        assert!(
            !reason.contains(row.diagnosis),
            "case {:?}: the stack reason must not carry the resource's diagnosis {:?}, got \
             {reason:?}",
            row.case,
            row.diagnosis
        );

        // ...which is on the failing RESOURCE's event, where a client reads why one
        // resource failed.
        let on_the_resource = resource_event_reason(
            &router,
            row.stack,
            row.logical_id,
            resource_status::CREATE_FAILED,
        )
        .await;
        assert!(
            on_the_resource.contains(row.diagnosis),
            "case {:?}: expected the failed resource's event to carry {:?}, got \
             {on_the_resource:?}",
            row.case,
            row.diagnosis
        );
    }
}

/// The template of the update case: one bucket, with whatever else the stage declares for
/// it.
fn one_bucket(extra: Value) -> String {
    let mut properties = json!({"BucketName": "reason-updated-bucket-store"});
    if let (Some(target), Some(source)) = (properties.as_object_mut(), extra.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    json!({"Resources": {"Store": {
        "Type": "AWS::S3::Bucket",
        "Properties": properties
    }}})
    .to_string()
}

/// A resource an UPDATE could not change reports the same sentence with the update's own
/// verb: the rule is the operation's, not the create's.
#[tokio::test]
async fn a_failed_update_reports_the_summary_sentence_alone() {
    let case = "a bucket property this emulator cannot change in place";
    let router = router();
    let stack = "reason-updated-bucket";

    let (code, body) = create_stack(&router, stack, &one_bucket(json!({})), &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    let (_, created) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&created, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must complete before the update, got {created}"
    );

    let (code, updated) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member(
                "TemplateBody",
                // A property no Amazon S3 call this emulator makes writes on a bucket that
                // exists: the ones it DOES write (the tag set, the versioning state) are
                // applied to the live bucket instead (`bucket_property_updates.rs`).
                &one_bucket(json!({"WebsiteConfiguration": {"IndexDocument": "index.html"}})),
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: UpdateStack must be accepted, got {code} {updated}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, stack).await;
    // The refused change failed the update, and a failed update is rolled back: the stack's
    // terminal status is the rolled-back one, still carrying the failure's own reason.
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_ROLLBACK_COMPLETE),
        "case {case:?}: expected the refused change to fail the update and roll it back, got \
         {described}"
    );
    let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
    let expected = resource_failure_summary(failed_action::UPDATE, ["Store"]);
    assert_eq!(
        reason, expected,
        "case {case:?}: expected the stack reason to be {expected:?}, got {reason:?}"
    );
    let on_the_resource =
        resource_event_reason(&router, stack, "Store", resource_status::UPDATE_FAILED).await;
    assert!(
        on_the_resource.contains("NotImplemented"),
        "case {case:?}: expected the resource's own event to say what could not be changed, \
         got {on_the_resource:?}"
    );
}

/// A failed create the stack ROLLED BACK reports the same summary sentence, and the ROLLBACK
/// EVENT is where the clause saying the rollback was asked for goes — the place that
/// sentence's own separating space leaves. The stack-level field still carries the failed
/// logical ids and nothing else, and the resource's own diagnosis still stays on its event.
///
/// `OnFailure` decides which of the two a caller sees — `DO_NOTHING` keeps the failed state
/// (the rows above), the documented default rolls the create back — so the rows here are the
/// same failures as above, read after the rollback.
#[tokio::test]
async fn a_rolled_back_create_reports_the_rollback_clause_on_its_event() {
    for row in refused_rows() {
        let router = router();
        given(&router, row.case, &row.given).await;

        // No `OnFailure` at all: "ROLLBACK — Roll back the stack creation. This is the
        // default."
        let (code, body) = create_stack(
            &router,
            row.stack,
            &row.template,
            &acknowledging_iam(&deferring_the_conflict(&row.given)),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must be accepted — the stack reports the outcome — got \
             {code} {body}",
            row.case
        );
        // The failure, and then the rollback it owes, each one transition away.
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (_, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::ROLLBACK_COMPLETE),
            "case {:?}: expected the create to be rolled back, got {described}",
            row.case
        );
        let summary = resource_failure_summary(failed_action::CREATE, [row.logical_id]);
        let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
        assert_eq!(
            reason, summary,
            "case {:?}: expected the rolled-back stack's reason to be the summary {summary:?}, \
             got {reason:?}",
            row.case
        );
        assert!(
            !reason.contains(row.diagnosis),
            "case {:?}: the stack reason must not carry the resource's diagnosis {:?}, got \
             {reason:?}",
            row.case,
            row.diagnosis
        );
        // The rollback's own event is where the clause goes.
        let events = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await
        .1;
        let expected_event_reason = rollback_requested(&summary);
        assert!(
            events.contains(&expected_event_reason),
            "case {:?}: expected an event carrying {expected_event_reason:?}, got {events}",
            row.case
        );
    }
}

/// The sentence itself: the fixed text around the comma-separated logical ids, for every
/// action a stack operation can fail at and for one, two and three failed resources — and
/// the rolled-back form of each, which is that sentence plus the one clause.
#[test]
fn the_summary_is_the_fixed_text_around_the_failed_logical_ids() {
    // (the action, the ids that failed, the whole sentence)
    let rows: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            failed_action::CREATE,
            vec!["One"],
            "The following resource(s) failed to create: [One]. ",
        ),
        (
            failed_action::CREATE,
            vec!["One", "Two"],
            "The following resource(s) failed to create: [One, Two]. ",
        ),
        (
            failed_action::UPDATE,
            vec!["One"],
            "The following resource(s) failed to update: [One]. ",
        ),
        (
            failed_action::UPDATE,
            vec!["Alpha", "Beta", "Gamma"],
            "The following resource(s) failed to update: [Alpha, Beta, Gamma]. ",
        ),
        (
            failed_action::DELETE,
            vec!["Store"],
            "The following resource(s) failed to delete: [Store]. ",
        ),
        (
            failed_action::DELETE,
            vec!["Store", "Queue"],
            "The following resource(s) failed to delete: [Store, Queue]. ",
        ),
    ];
    for (action, ids, expected) in rows {
        let rendered = resource_failure_summary(action, ids.iter().copied());
        assert_eq!(
            rendered, expected,
            "case ({action:?}, {ids:?}): expected {expected:?}, got {rendered:?}"
        );
        // The rolled-back wording is that same sentence with the clause in the place its
        // separating space leaves — never a second period and never a lost space. It is
        // what the ROLLBACK's own event carries.
        let unwound = rollback_requested(&rendered);
        assert_eq!(
            unwound,
            format!("{expected}Rollback requested by user."),
            "case ({action:?}, {ids:?}): expected the rolled-back wording to add the one \
             clause, got {unwound:?}"
        );
    }
}
