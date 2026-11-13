// SPDX-License-Identifier: CC0-1.0

//! What a resource whose owning service REFUSED a call reports, for a service whose SDK
//! words a diagnostic of the call — end to end over the awsQuery wire.
//!
//! RULE under test: **a resource provider does not word its own diagnosis of a refused call:
//! what CloudFormation quotes in the failed resource's `ResourceStatusReason` is the
//! exception that provider's SDK raised — the owning service's own message followed by that
//! SDK's diagnostic of the call, naming the service as its SDK names it, the HTTP status the
//! service answered, the id (and, for Amazon S3, the EXTENDED id) of the call and how many
//! attempts the SDK made — closed by the `RequestToken` of the resource request and the
//! `HandlerErrorCode` class the refusal falls in.**
//!
//! Why it matters: a client classifies a failed resource by its `HandlerErrorCode` to decide
//! whether the failure is worth retrying, and diagnoses it from the quoted message. A refusal
//! the owning service answered because of the STATE of the resource it manages — a bucket
//! that still holds objects — is not a fault in the declaration: reported as a client-side
//! `InvalidRequest`, automation stops retrying something that would have cleared.
//!
//! The rows differ along what the rule generalises over: WHICH operation of the stack ran
//! into the refusal (a `DeleteStack` of a stack whose bucket is not empty, and the ROLLBACK
//! of a failed create over one), since the quoted exception is the same whichever call made
//! it. The control row is the same template over an EMPTY bucket: nothing refuses it, so the
//! resource reports no failure at all.

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, resource_event_reason, router,
    wait_for_the_transition, xml_text,
};
use crate::stack_resource_provisioning::s3;

/// The message Amazon S3 answers for a bucket that still holds objects, which is the text
/// the quoted exception OPENS with (`BucketNotEmpty`, HTTP 409 Conflict).
const BUCKET_NOT_EMPTY: &str = "The bucket you tried to delete is not empty";

/// The class a refusal of this kind falls in: the owning service refused a well-formed call
/// because of the state of the resource it manages, which the provider reports as the called
/// service's own exception rather than as a fault in the declaration. One of the model's
/// `HandlerErrorCode` values.
const EXPECTED_CLASS: &str = "GeneralServiceException";

/// Pull the quoted exception and the reported class out of one `ResourceStatusReason`.
pub(crate) fn parts(case: &str, reason: &str) -> (String, String) {
    let opens = "Resource handler returned message: \"";
    let rest = reason.strip_prefix(opens).unwrap_or_else(|| {
        panic!("case {case:?}: expected the reason to open with {opens:?}, got {reason:?}")
    });
    let (quoted, tail) = rest.split_once("\" (RequestToken: ").unwrap_or_else(|| {
        panic!("case {case:?}: expected a quoted message and a RequestToken, got {reason:?}")
    });
    let class = tail
        .split_once("HandlerErrorCode: ")
        .and_then(|(_, class)| class.strip_suffix(')'))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a HandlerErrorCode, got {reason:?}")
        });
    (quoted.to_string(), class.to_string())
}

/// Hold one quoted exception to the SDK diagnostic Amazon S3's SDK appends: the service, the
/// status, both ids and the attempt count — each present, and each of the FORM that service
/// answers rather than one particular recorded value.
fn assert_s3_diagnostic(case: &str, quoted: &str) {
    assert!(
        quoted.starts_with(BUCKET_NOT_EMPTY),
        "case {case:?}: expected the quoted text to open with the service's own message, got \
         {quoted:?}"
    );
    let diagnostic = quoted
        .strip_prefix(BUCKET_NOT_EMPTY)
        .map(str::trim_start)
        .unwrap_or_default();
    let (envelope, attempts) = diagnostic.split_once(") (SDK Attempt Count: ").unwrap_or_else(
        || panic!("case {case:?}: expected the SDK diagnostic, got {quoted:?}"),
    );
    let envelope = envelope.strip_prefix('(').unwrap_or(envelope);
    let fields: Vec<&str> = envelope.split(", ").collect();
    assert_eq!(
        fields.len(),
        4,
        "case {case:?}: expected the service, the status and both ids, got {envelope:?}"
    );
    assert_eq!(
        fields[0], "Service: S3",
        "case {case:?}: expected the SDK's own name for the service, got {:?}",
        fields[0]
    );
    assert_eq!(
        fields[1], "Status Code: 409",
        "case {case:?}: expected the status Amazon S3 answered, got {:?}",
        fields[1]
    );
    let request_id = fields[2]
        .strip_prefix("Request ID: ")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Request ID, got {:?}", fields[2]));
    assert!(
        !request_id.is_empty()
            && request_id
                .chars()
                .all(|held| held.is_ascii_uppercase() || held.is_ascii_digit()),
        "case {case:?}: expected an opaque upper-case request id, got {request_id:?}"
    );
    let extended = fields[3].strip_prefix("Extended Request ID: ").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the extended id Amazon S3 answers, got {:?}",
            fields[3]
        )
    });
    assert!(
        !extended.is_empty()
            && extended
                .chars()
                .all(|held| held.is_ascii_alphanumeric() || "+/=".contains(held)),
        "case {case:?}: expected an opaque extended request id, got {extended:?}"
    );
    let attempts = attempts.strip_suffix(')').unwrap_or(attempts);
    assert!(
        attempts.parse::<u32>().is_ok_and(|made| made >= 1),
        "case {case:?}: expected the attempts the SDK made, got {attempts:?}"
    );
}

/// Seed one bucket holding one object, through Amazon S3's own wire.
async fn seed_a_non_empty_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let created = s3(router, "PUT", &format!("/{bucket}"), b"").await;
    assert!(
        (200..300).contains(&created.status),
        "case {case:?}: the bucket must exist, got {} {}",
        created.status,
        created.body
    );
    let written = s3(
        router,
        "PUT",
        &format!("/{bucket}/held/object.txt"),
        b"an object the deletion cannot remove",
    )
    .await;
    assert!(
        (200..300).contains(&written.status),
        "case {case:?}: the object must be written, got {} {}",
        written.status,
        written.body
    );
}

/// A template declaring one bucket by name.
fn bucket_template(bucket: &str) -> String {
    format!(
        r#"{{"Resources":{{"Bucket":{{"Type":"AWS::S3::Bucket",
            "Properties":{{"BucketName":"{bucket}"}}}}}}}}"#
    )
}

#[tokio::test]
async fn a_delete_the_owning_service_refuses_reports_its_sdk_exception() {
    let case = "a DeleteStack over a bucket that still holds an object";
    let router = router();
    seed_a_non_empty_bucket(&router, case, "refused-delete-bucket").await;

    // The create takes the bucket over: "if you re-create an existing bucket that you
    // already own in the us-east-1 Region, Amazon S3 returns 200 OK", so the stack records
    // it as a resource it manages. The pre-deployment validations are OFF because this case
    // is about what the OWNING SERVICE answers: with them on, the existence check fails a
    // create whose declared name is already held before S3 is asked at all
    // (`state::early_validation`).
    let (code, body) = create_stack(
        &router,
        "refused-delete",
        &bucket_template("refused-delete-bucket"),
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;

    let (code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "refused-delete")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DeleteStack must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let reason = resource_event_reason(
        &router,
        "refused-delete",
        "Bucket",
        "DELETE_FAILED",
    )
    .await;
    let (quoted, class) = parts(case, &reason);
    assert_s3_diagnostic(case, &quoted);
    assert_eq!(
        class, EXPECTED_CLASS,
        "case {case:?}: expected the {EXPECTED_CLASS} class, got {class:?} in {reason:?}"
    );

    // The object is still there, so the refusal was real.
    let read = s3(&router, "GET", "/refused-delete-bucket/held/object.txt", b"").await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the object must survive, got {} {}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_rollback_the_owning_service_refuses_reports_the_same_exception() {
    let case = "the ROLLBACK of a failed create over a bucket that still holds an object";
    let router = router();
    seed_a_non_empty_bucket(&router, case, "refused-rollback-bucket").await;

    // A second resource the owning service refuses fails the create, so the rollback has to
    // remove the bucket the create had taken over — which is the deletion S3 refuses.
    let template = r#"{"Resources":{
        "Bucket":{"Type":"AWS::S3::Bucket",
          "Properties":{"BucketName":"refused-rollback-bucket"}},
        "SubnetGroup":{"Type":"AWS::RDS::DBSubnetGroup","DependsOn":"Bucket",
          "Properties":{"DBSubnetGroupDescription":"a group over values that name no subnet",
            "SubnetIds":["not-a-subnet-id","also-not-a-subnet-id"]}}}}"#;
    let (code, body) = create_stack(
        &router,
        "refused-rollback",
        template,
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let reason = resource_event_reason(
        &router,
        "refused-rollback",
        "Bucket",
        "DELETE_FAILED",
    )
    .await;
    let (quoted, class) = parts(case, &reason);
    assert_s3_diagnostic(case, &quoted);
    assert_eq!(
        class, EXPECTED_CLASS,
        "case {case:?}: expected the {EXPECTED_CLASS} class, got {class:?} in {reason:?}"
    );
}

/// The control row: the same template over an EMPTY bucket. Nothing refuses the deletion, so
/// the resource reports no failure and the stack's own deletion completes.
#[tokio::test]
async fn a_delete_the_owning_service_accepts_reports_no_refusal() {
    let case = "a DeleteStack over an empty bucket";
    let router = router();
    let (code, body) = create_stack(
        &router,
        "accepted-delete",
        &bucket_template("accepted-delete-bucket"),
        &[],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;

    // Addressed by STACK ID from here on, since a completed deletion leaves the live set.
    let (code, body) = describe_stack(&router, "accepted-delete").await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
    );
    let stack_id = xml_text(&body, "StackId").unwrap_or_default();

    let (code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "accepted-delete")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DeleteStack must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let failed = resource_event_reason(&router, &stack_id, "Bucket", "DELETE_FAILED").await;
    assert!(
        failed.is_empty(),
        "case {case:?}: expected no refused deletion, got {failed:?}"
    );
    let (code, body) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "case {case:?}: expected the deletion to complete, got {body}"
    );
}
