// SPDX-License-Identifier: CC0-1.0

//! How MANY stack-level events report one failure of a stack operation, end to end over the
//! awsQuery wire.
//!
//! RULE under test: **one failure of a stack operation is explained ONCE in the stack's own
//! event stream, by the event of the transition that accounts for it — the failed status the
//! stack rests in, or the rollback that supersedes it, whose reason carries the failure and
//! the clause saying the rollback was asked for. The terminal status a rollback settles into
//! reports that the rollback finished and carries no `ResourceStatusReason` of its own, while
//! the stack's `StackStatusReason` goes on carrying the failure for a client that reads the
//! stack rather than its events.**
//!
//! Why it matters: `DescribeStackEvents` is the audit trail deployment tooling reads to
//! report "what went wrong" and to decide whether one root cause or several resources failed.
//! A reason repeated on every transition that observed the failure inflates that count, so
//! "exactly one failure" is not assertable and a single root cause reads as several.
//!
//! The rows differ along what the rule generalises over: the OPERATION that failed (a create,
//! an update, a cancelled delete), the terminal status it settles into (`ROLLBACK_COMPLETE`,
//! `UPDATE_ROLLBACK_COMPLETE`, and the healthy status a cancelled delete is put back into),
//! and WHERE the failure comes from — CloudFormation's own diagnostic (an unresolvable
//! `Fn::ImportValue`, an export name another stack already publishes, an export in use that a
//! deployment would withdraw, one a deletion would remove) or the summary of a resource an
//! owning service refused.

use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_resource_provisioning::poll_to_terminal;
use crate::stack_lifecycle::{
    call, create_stack, deploy, member, router, xml_blocks, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The exporter every cross-stack row deploys, and the export it publishes.
const OWNER: &str = "one-event-owner";
const HELD: &str = "one-event-held";
const HELD_VALUE: &str = "held-value";

/// The stack that imports [`HELD`] in the rows about an export in use.
const IMPORTER: &str = "one-event-importer";

/// A template publishing one export, over a resource no owning service has to provision.
fn exporter_template(value: &str) -> String {
    format!(
        r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
            "Outputs":{{"Held":{{"Value":"{value}","Export":{{"Name":"{HELD}"}}}}}}}}"#
    )
}

/// A template reading one export into an output of its own.
fn importer_template(export_name: &str) -> String {
    format!(
        r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
            "Outputs":{{"Echoed":{{"Value":{{"Fn::ImportValue":"{export_name}"}}}}}}}}"#
    )
}

/// A template declaring one bucket, with whatever else a stage states for it.
fn bucket_template(bucket: &str, extra: &str) -> String {
    format!(
        r#"{{"Resources":{{"Store":{{"Type":"AWS::S3::Bucket","Properties":{{
            "BucketName":"{bucket}"{extra}}}}}}}}}"#
    )
}

/// Which failure a row drives.
#[derive(Clone, Copy, Debug)]
enum Failure {
    /// A create whose `Fn::ImportValue` names an export nothing publishes.
    UnresolvableImport,
    /// A create whose output claims an export name another live stack already publishes.
    ClaimedExportName,
    /// A create whose resource the owning service refuses.
    RefusedResourceOnCreate,
    /// An update that would publish another value under an export a live stack imports.
    WithdrawnExportOnUpdate,
    /// An update whose resource the owning service cannot change.
    RefusedResourceOnUpdate,
    /// A delete of the stack publishing an export a live stack still imports, which is
    /// CANCELLED rather than rolled back: the stack goes back to the healthy status it held.
    CanceledDelete,
}

/// One row: the failure, the stack it is driven on, the terminal status that stack settles
/// in, and the sentence exactly one stack-level event must carry.
struct Row {
    case: &'static str,
    failure: Failure,
    stack: &'static str,
    settles_in: &'static str,
    reason: String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create importing an export nothing publishes",
            failure: Failure::UnresolvableImport,
            stack: "one-event-ghost-importer",
            settles_in: status::ROLLBACK_COMPLETE,
            reason: "No export named one-event-ghost found.".to_string(),
        },
        Row {
            case: "a create claiming an export name another stack already publishes",
            failure: Failure::ClaimedExportName,
            stack: "one-event-rival",
            settles_in: status::ROLLBACK_COMPLETE,
            reason: format!(
                "Export with name {HELD} is already exported by stack {OWNER}."
            ),
        },
        Row {
            case: "a create whose resource the owning service refuses",
            failure: Failure::RefusedResourceOnCreate,
            stack: "one-event-refused-create",
            settles_in: status::ROLLBACK_COMPLETE,
            reason: "The following resource(s) failed to create: [Notifications].".to_string(),
        },
        Row {
            case: "an update that would publish another value under an export in use",
            failure: Failure::WithdrawnExportOnUpdate,
            stack: OWNER,
            settles_in: status::UPDATE_ROLLBACK_COMPLETE,
            reason: format!(
                "Cannot update export {HELD} as it is in use by {IMPORTER}."
            ),
        },
        Row {
            case: "an update whose resource the owning service cannot change",
            failure: Failure::RefusedResourceOnUpdate,
            stack: "one-event-refused-update",
            settles_in: status::UPDATE_ROLLBACK_COMPLETE,
            reason: "The following resource(s) failed to update: [Store].".to_string(),
        },
        Row {
            case: "a delete of the stack publishing an export in use, which is cancelled",
            failure: Failure::CanceledDelete,
            stack: OWNER,
            settles_in: status::CREATE_COMPLETE,
            reason: format!(
                "Delete canceled. Cannot delete export {HELD} as it is in use by {IMPORTER}."
            ),
        },
    ]
}

/// Drive the row's failure, leaving its stack settled in a terminal status.
async fn drive(router: &MultiServiceRouter, row: &Row) {
    match row.failure {
        Failure::UnresolvableImport => {
            accepted(
                row.case,
                create_stack(
                    router,
                    row.stack,
                    &importer_template("one-event-ghost"),
                    &[],
                )
                .await,
            );
        }
        Failure::ClaimedExportName => {
            deploy(router, OWNER, &exporter_template(HELD_VALUE)).await;
            accepted(
                row.case,
                create_stack(router, row.stack, &exporter_template("rival-value"), &[]).await,
            );
        }
        Failure::RefusedResourceOnCreate => {
            // A FIFO topic whose name does not carry the documented suffix: Amazon SNS
            // refuses it, so the resource fails and the create fails with it.
            let template = r#"{"Resources":{"Notifications":{"Type":"AWS::SNS::Topic",
                "Properties":{"TopicName":"one-event-topic","FifoTopic":true}}}}"#;
            accepted(
                row.case,
                create_stack(router, row.stack, template, &[]).await,
            );
        }
        Failure::WithdrawnExportOnUpdate => {
            deploy(router, OWNER, &exporter_template(HELD_VALUE)).await;
            deploy(router, IMPORTER, &importer_template(HELD)).await;
            accepted(
                row.case,
                call(
                    router,
                    "UpdateStack",
                    &[
                        member("StackName", OWNER),
                        member("TemplateBody", &exporter_template("held-value-revised")),
                    ],
                )
                .await,
            );
        }
        Failure::RefusedResourceOnUpdate => {
            deploy(
                router,
                row.stack,
                &bucket_template("one-event-refused-update-store", ""),
            )
            .await;
            // A property no Amazon S3 call this emulator makes writes on a bucket that
            // exists, so the change cannot be applied and the update fails.
            accepted(
                row.case,
                call(
                    router,
                    "UpdateStack",
                    &[
                        member("StackName", row.stack),
                        member(
                            "TemplateBody",
                            &bucket_template(
                                "one-event-refused-update-store",
                                r#","WebsiteConfiguration":{"IndexDocument":"index.html"}"#,
                            ),
                        ),
                    ],
                )
                .await,
            );
        }
        Failure::CanceledDelete => {
            deploy(router, OWNER, &exporter_template(HELD_VALUE)).await;
            deploy(router, IMPORTER, &importer_template(HELD)).await;
            accepted(
                row.case,
                call(router, "DeleteStack", &[member("StackName", OWNER)]).await,
            );
        }
    }
}

/// Every mutating call of these rows is ACCEPTED — the stack is what reports the outcome.
fn accepted(case: &str, answered: (u16, String)) {
    let (code, body) = answered;
    assert_eq!(
        code, 200,
        "case {case:?}: the call must be accepted — the stack reports the outcome — got \
         {code} {body}"
    );
}

/// The stack's OWN events as `(ResourceStatus, ResourceStatusReason)`, newest first: those
/// whose `LogicalResourceId` is the stack itself, which is the stack-level audit trail.
async fn stack_level_events(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String)> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(stack))
        .map(|entry| {
            (
                xml_text(entry, "ResourceStatus").unwrap_or_default(),
                xml_text(entry, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn one_failure_is_reported_by_exactly_one_stack_level_event() {
    for row in rows() {
        let router = router();
        drive(&router, &row).await;
        let described = poll_to_terminal(&router, row.case, row.stack).await;
        let settled = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            settled, row.settles_in,
            "case {:?}: expected the stack to settle in {:?}, got {settled:?} with \
             {described}",
            row.case, row.settles_in
        );

        let events = stack_level_events(&router, row.case, row.stack).await;
        let carrying: Vec<&(String, String)> = events
            .iter()
            .filter(|(_, reason)| reason.contains(&row.reason))
            .collect();
        assert_eq!(
            carrying.len(),
            1,
            "case {:?}: expected exactly ONE stack-level event to carry {:?}, got \
             {carrying:?} among {events:?}",
            row.case,
            row.reason
        );

        // The terminal status of a ROLLBACK reports that the rollback finished and nothing
        // about why it ran, so the one event carrying the failure is never that one; a
        // cancelled delete is not a rollback — its reason belongs to the healthy status the
        // stack is put back into, which is the row that keeps this from over-reaching.
        let terminal = events
            .iter()
            .find(|(reported, _)| *reported == row.settles_in)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected an event for the terminal status {:?}, got \
                     {events:?}",
                    row.case, row.settles_in
                )
            });
        let terminal_reports_the_failure = terminal.1.contains(&row.reason);
        let expected_on_the_terminal_event = !row.settles_in.ends_with("ROLLBACK_COMPLETE");
        assert_eq!(
            terminal_reports_the_failure, expected_on_the_terminal_event,
            "case {:?}: expected the {:?} event to {} the failure, got {:?}",
            row.case,
            row.settles_in,
            if expected_on_the_terminal_event {
                "carry"
            } else {
                "carry nothing about"
            },
            terminal.1
        );

        // ...and a client that reads the STACK rather than its events still reads why it is
        // in the status it rests in.
        let stack_reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            stack_reason.contains(&row.reason),
            "case {:?}: expected the stack's own reason to carry {:?}, got {stack_reason:?}",
            row.case,
            row.reason
        );
    }
}
