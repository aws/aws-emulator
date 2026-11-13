// SPDX-License-Identifier: CC0-1.0

//! What becomes of the OLD physical resource of a replacement whose delete the owning service
//! REFUSES, end to end over the awsQuery wire.
//!
//! RULE under test: **a resource an update REPLACED is taken down in the update's CLEANUP
//! phase, which runs after the replacement is already in force — "the stack update was
//! successful, but CloudFormation is cleaning up the old resources"
//! (`UPDATE_COMPLETE_CLEANUP_IN_PROGRESS`) — so a delete the owning service refuses there is
//! reported on that resource and leaves the update COMPLETE: the stack records the resource
//! the replacement minted and stops recording the one it replaced, which is left exactly as
//! it is in the service that owns it.**
//!
//! The replacement this suite drives is the one every `AWS::S3::Bucket` rename is — the bucket
//! name is create-only, so a declaration naming another bucket asks for a different resource —
//! and the refusal is the one Amazon S3 publishes for a bucket that still holds objects ("the
//! bucket you tried to delete is not empty"). The rows differ along what the rule generalises
//! over: whether the replaced bucket can be deleted at all (an empty one is, so its record goes
//! and the bucket is gone) and whether it cannot (one holding an object, whose delete is
//! refused), and whether the stack carries a second resource the cleanup must leave alone.
//!
//! Every assertion is a client's: the stack is read through `DescribeStacks`,
//! `DescribeStackResources` and `DescribeStackEvents`, and the buckets through Amazon S3's own
//! wire.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_stack, member, router, xml_blocks};
use crate::stack_lifecycle::{xml_text, xml_texts};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

/// How long a poll waits for a stack operation to settle before the suite calls it stuck, and
/// how often it looks. The deadline is far longer than the spans this emulator gives a stack's
/// transitions, so a loaded host cannot fail a row; what is asserted is the CONDITION (a status
/// that is not an operation in flight), never the time it took.
const DEADLINE: Duration = Duration::from_secs(60);
/// See [`DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The logical id the bucket is declared under in every revision below.
const LOGICAL_ID: &str = "Store";

/// The template one revision deploys: the bucket under `name`, beside the second resource a
/// revision keeps unchanged (another bucket, under its own fixed name).
fn template(name: &str, keeps_a_sibling: bool) -> String {
    let mut resources = json!({
        LOGICAL_ID: {"Type": "AWS::S3::Bucket", "Properties": {"BucketName": name}}
    });
    if keeps_a_sibling {
        if let Some(members) = resources.as_object_mut() {
            members.insert(
                SIBLING_LOGICAL_ID.to_string(),
                json!({
                    "Type": "AWS::S3::Bucket",
                    "Properties": {"BucketName": "bucket-replacement-cleanup-sibling-kept"}
                }),
            );
        }
    }
    json!({"Resources": resources}).to_string()
}

/// The logical id of the resource a row's update leaves exactly as it is.
const SIBLING_LOGICAL_ID: &str = "Archive";

/// Poll `DescribeStacks` the way a client's waiter does and answer the `StackStatus` the stack
/// SETTLED on — the first status that is not an operation in flight.
async fn settled_status(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let (code, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if !reported.is_empty() && !reported.ends_with("_IN_PROGRESS") {
            return reported;
        }
        if seen.last().map(String::as_str) != Some(reported.as_str()) {
            seen.push(reported);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {stack} to settle, saw {seen:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every `(LogicalResourceId, PhysicalResourceId)` the stack records.
async fn recorded_resources(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
) -> Vec<(String, String)> {
    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackResources must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter_map(|record| {
            Some((
                xml_text(record, "LogicalResourceId")?,
                xml_text(record, "PhysicalResourceId").unwrap_or_default(),
            ))
        })
        .collect()
}

/// The `(ResourceStatus, ResourceStatusReason)` of every event the stack recorded for
/// `logical_id`, newest first — what says how the cleanup of the replaced resource ended.
async fn events_for(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> Vec<(String, String)> {
    let (code, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|event| xml_text(event, "LogicalResourceId").as_deref() == Some(logical_id))
        .map(|event| {
            (
                xml_text(event, "ResourceStatus").unwrap_or_default(),
                xml_text(event, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// `HEAD /<bucket>` — does Amazon S3 still hold this bucket?
async fn holds_bucket(router: &MultiServiceRouter, bucket: &str) -> bool {
    s3(router, "HEAD", &format!("/{bucket}"), b"").await.status == 200
}

/// One row: whether the replaced bucket holds an object (so Amazon S3 refuses its delete), and
/// whether the stack declares a second resource beside it.
struct Row {
    case: &'static str,
    stack: &'static str,
    first: &'static str,
    second: &'static str,
    /// The object to store in the first bucket, which is what makes its delete refused.
    holds_an_object: bool,
    keeps_a_sibling: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the replaced bucket is empty, so its delete goes through",
            stack: "bucket-replacement-cleanup-empty",
            first: "bucket-replacement-cleanup-empty-one",
            second: "bucket-replacement-cleanup-empty-two",
            holds_an_object: false,
            keeps_a_sibling: false,
        },
        Row {
            case: "the replaced bucket still holds an object, so its delete is refused",
            stack: "bucket-replacement-cleanup-held",
            first: "bucket-replacement-cleanup-held-one",
            second: "bucket-replacement-cleanup-held-two",
            holds_an_object: true,
            keeps_a_sibling: false,
        },
        Row {
            case: "a refused cleanup beside a resource the update keeps",
            stack: "bucket-replacement-cleanup-sibling",
            first: "bucket-replacement-cleanup-sibling-one",
            second: "bucket-replacement-cleanup-sibling-two",
            holds_an_object: true,
            keeps_a_sibling: true,
        },
    ]
}

/// The key the row's object is stored at.
const HELD_KEY: &str = "held/object.bin";

/// RULE: see the module header — the cleanup of a replaced resource cannot fail the update
/// that already landed.
#[tokio::test]
async fn a_refused_cleanup_of_a_replaced_bucket_leaves_the_update_complete() {
    for row in rows() {
        let router = router();
        let (code, body) = create_stack(
            &router,
            row.stack,
            &template(row.first, row.keeps_a_sibling),
            &[],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: the create must be accepted, got {code} {body}",
            row.case
        );
        let created = settled_status(row.case, &router, row.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {:?}: expected the create to complete, got {created:?}",
            row.case
        );

        if row.holds_an_object {
            let stored = s3(
                &router,
                "PUT",
                &format!("/{}/{HELD_KEY}", row.first),
                b"bytes that outlive the stack's record",
            )
            .await;
            assert_eq!(
                stored.status, 200,
                "case {:?}: the object must be stored first, got {} {}",
                row.case, stored.status, stored.body
            );
        }

        // The RENAME: the bucket name is create-only, so this declaration asks for a different
        // bucket and the update replaces the resource.
        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member(
                    "TemplateBody",
                    &template(row.second, row.keeps_a_sibling),
                ),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: the update must be accepted, got {code} {body}",
            row.case
        );
        let updated = settled_status(row.case, &router, row.stack).await;
        assert_eq!(
            updated,
            status::UPDATE_COMPLETE,
            "case {:?}: expected the update to complete whatever the cleanup did, got \
             {updated:?}",
            row.case
        );

        // The stack records the bucket the replacement minted, once.
        let recorded = recorded_resources(row.case, &router, row.stack).await;
        let under_the_logical_id: Vec<&(String, String)> = recorded
            .iter()
            .filter(|(logical, _)| logical == LOGICAL_ID)
            .collect();
        assert_eq!(
            under_the_logical_id.len(),
            1,
            "case {:?}: expected exactly one record under {LOGICAL_ID:?}, got {recorded:?}",
            row.case
        );
        assert_eq!(
            under_the_logical_id[0].1, row.second,
            "case {:?}: expected the record to name the replacement, got {:?}",
            row.case, under_the_logical_id[0].1
        );
        if row.keeps_a_sibling {
            assert!(
                recorded
                    .iter()
                    .any(|(logical, _)| logical == SIBLING_LOGICAL_ID),
                "case {:?}: expected the untouched resource to be recorded still, got \
                 {recorded:?}",
                row.case
            );
        }

        // The replacement exists, and the replaced bucket is gone exactly where its delete
        // went through.
        assert!(
            holds_bucket(&router, row.second).await,
            "case {:?}: expected Amazon S3 to hold the replacement {:?}",
            row.case,
            row.second
        );
        assert_eq!(
            holds_bucket(&router, row.first).await,
            row.holds_an_object,
            "case {:?}: expected the replaced bucket {:?} to be {}",
            row.case,
            row.first,
            if row.holds_an_object {
                "left where it is"
            } else {
                "deleted"
            }
        );

        let events = events_for(row.case, &router, row.stack, LOGICAL_ID).await;
        if row.holds_an_object {
            // The refusal is REPORTED on the resource, carrying the message Amazon S3
            // answered, and the bytes the bucket holds are still readable through its own
            // wire: nothing of the orphaned bucket was taken away.
            let failed = events
                .iter()
                .find(|(state, _)| state == resource_status::DELETE_FAILED);
            let Some((_, reason)) = failed else {
                panic!(
                    "case {:?}: expected a {} event for the replaced bucket, got {events:?}",
                    row.case,
                    resource_status::DELETE_FAILED
                )
            };
            assert!(
                reason.contains("not empty"),
                "case {:?}: expected the reason to carry the refusal Amazon S3 answered, got \
                 {reason:?}",
                row.case
            );
            let read = s3(
                &router,
                "GET",
                &format!("/{}/{HELD_KEY}", row.first),
                b"",
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected the orphaned bucket's object to be readable, got {} {}",
                row.case, read.status, read.body
            );
        } else {
            assert!(
                events
                    .iter()
                    .any(|(state, _)| state == resource_status::DELETE_COMPLETE),
                "case {:?}: expected the replaced bucket's delete to be reported complete, got \
                 {events:?}",
                row.case
            );
            assert!(
                !events
                    .iter()
                    .any(|(state, _)| state == resource_status::DELETE_FAILED),
                "case {:?}: expected no delete failure, got {events:?}",
                row.case
            );
        }

        // And the stack itself reports no failure reason for an update that completed.
        let (code, body) = call(
            &router,
            "DescribeStacks",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {body}",
            row.case
        );
        assert!(
            xml_texts(&body, "StackStatusReason").is_empty(),
            "case {:?}: expected no stack status reason, got {:?}",
            row.case,
            xml_texts(&body, "StackStatusReason")
        );
    }
}
