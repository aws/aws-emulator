// SPDX-License-Identifier: CC0-1.0

//! An `UpdateStack` that changes the POLICY of the bucket a stack already has, and one whose
//! RENAME the owning service refuses — both end to end over the awsQuery wire.
//!
//! **The first rule.** A property the owning service publishes a call to write on the resource
//! that EXISTS is applied by that call, and the resource keeps its identity. The policy of a
//! bucket is such a property: an `AWS::S3::BucketPolicy` is "the policy of the bucket it
//! names", written by `PutBucketPolicy` ("applies an Amazon S3 bucket policy to an Amazon S3
//! bucket"), and a bucket's versioning state is written by `PutBucketVersioning` ("sets the
//! versioning state of an existing bucket") — so a stack update that changes either, or both at
//! once, settles `UPDATE_COMPLETE` with the new configuration in force, the same physical
//! resource id published, and the objects the bucket already held untouched.
//! `bucket_property_updates.rs` pins the versioning and tag halves of the same rule on a bucket
//! declared ALONE; this suite pins it for a stack that also carries the bucket's policy, and for
//! a bucket that is not empty.
//!
//! **The second rule.** A replacement the owning service REFUSES is not performed, so the
//! update fails and the stack ROLLS BACK: nothing was replaced, the resource keeps the identity
//! and configuration in force, and the stack reaches `UPDATE_ROLLBACK_COMPLETE` rather than
//! resting in `UPDATE_FAILED`. The rename of a bucket is a replacement — the bucket name is
//! create-only, so a declaration naming another bucket asks for a different resource
//! (`bucket_replacement_cleanup.rs` pins what becomes of the one it replaces) — and the refusal
//! the rows use is the one Amazon S3 answers a create of a bucket the account already holds.
//!
//! The rows differ along what the first rule generalises over: which property the update
//! changes (the policy alone, the versioning state alone, both at once), and whether the stack
//! had put the bucket in a versioning state at all before. The second rule's rows differ along
//! WHO holds the colliding name: a bucket a client took outside any stack, and one another live
//! stack of the Region manages.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::harness::{call_in, create_stack, member, router, xml_blocks, REGION};
use crate::stack_lifecycle::{xml_text, xml_texts};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

/// A Region that is NOT North Virginia, which is where a re-create of a bucket the account
/// already owns is REFUSED: "Amazon S3 returns this error in all Amazon Web Services Regions
/// except in the North Virginia Region. For legacy compatibility, if you re-create an existing
/// bucket that you already own in the North Virginia Region, Amazon S3 returns 200 OK"
/// (`BucketAlreadyOwnedByYou`'s own documentation) — so the rows of the second rule deploy
/// their stacks here, the Region in which a rename onto a taken name is a refusal at all.
const NON_LEGACY_REGION: &str = "us-west-2";

/// How long a poll waits for a stack operation to settle before the suite calls it stuck, and
/// how often it looks. The deadline is far longer than the spans this emulator gives a stack's
/// transitions, so a loaded host cannot fail a row; what is asserted is the CONDITION (a status
/// that is not an operation in flight), never the time it took.
const DEADLINE: Duration = Duration::from_secs(60);
/// See [`DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The logical ids every template below declares its two resources under.
const BUCKET_LOGICAL_ID: &str = "Store";
/// See [`BUCKET_LOGICAL_ID`].
const POLICY_LOGICAL_ID: &str = "StorePolicy";

/// The key the setup writes into the bucket, so every update here is of a bucket that holds
/// something.
const HELD_KEY: &str = "held/object.bin";

/// The bytes that object holds.
const BODY: &[u8] = b"bytes that outlive the stack's update";

/// A bucket policy granting the account that owns the bucket one action on its objects.
///
/// The principal is the account's own root ARN, which is a principal this emulator's IAM
/// resolves, and the grant is not a public one — the two things `PutBucketPolicy` judges a
/// document by before it stores it.
fn policy_document(bucket: &str, action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AccountOwnObjects",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": action,
            "Resource": format!("arn:aws:s3:::{bucket}/*"),
        }],
    })
}

/// The template one revision deploys: the bucket under `bucket`, in the versioning state the
/// revision asks for when it asks for one, with the bucket's own policy beside it as the
/// `AWS::S3::BucketPolicy` resource that owns it.
fn template(bucket: &str, versioning: Option<&str>, action: &str) -> String {
    let mut properties = json!({ "BucketName": bucket });
    if let (Some(members), Some(status)) = (properties.as_object_mut(), versioning) {
        members.insert(
            "VersioningConfiguration".to_string(),
            json!({ "Status": status }),
        );
    }
    json!({
        "Resources": {
            BUCKET_LOGICAL_ID: {
                "Type": "AWS::S3::Bucket",
                "Properties": properties,
            },
            POLICY_LOGICAL_ID: {
                "Type": "AWS::S3::BucketPolicy",
                "Properties": {
                    "Bucket": { "Ref": BUCKET_LOGICAL_ID },
                    "PolicyDocument": policy_document(bucket, action),
                },
            },
        }
    })
    .to_string()
}

/// Poll `DescribeStacks` the way a client's waiter does and answer the `StackStatus` the stack
/// SETTLED on — the first status that is not an operation in flight.
async fn settled_status(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let (code, body) =
            call_in(router, region, "DescribeStacks", &[member("StackName", stack)]).await;
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

/// The `ResourceStatusReason` of every event the stack recorded, newest first — what a failed
/// assertion names the deployment's own account of itself by.
async fn reported_reasons(router: &MultiServiceRouter, region: &str, stack: &str) -> Vec<String> {
    let (_, events) =
        call_in(router, region, "DescribeStackEvents", &[member("StackName", stack)]).await;
    xml_texts(&events, "ResourceStatusReason")
}

/// The physical resource id the stack publishes under `logical_id` — the name a client calls
/// Amazon S3 with.
async fn published_id(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (code, body) = call_in(
        router,
        region,
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
        .find(|record| xml_text(record, "LogicalResourceId").as_deref() == Some(logical_id))
        .and_then(|record| xml_text(record, "PhysicalResourceId"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no physical id under {logical_id:?} in {body}")
        })
}

/// The versioning state Amazon S3 reports for `bucket` — the `<Status>` of
/// `GetBucketVersioning`, absent while the bucket has never been put in one.
async fn bucket_versioning(case: &str, router: &MultiServiceRouter, bucket: &str) -> Option<String> {
    let answer = s3(router, "GET", &format!("/{bucket}?versioning"), b"").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetBucketVersioning on {bucket:?} must be served, got {} {}",
        answer.status, answer.body
    );
    xml_text(&answer.body, "Status")
}

/// Deploy `body` as an update of `stack` and answer the status it settles in.
async fn updated(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    body: &str,
) -> String {
    let (code, answered) = call_in(
        router,
        region,
        "UpdateStack",
        &[member("StackName", stack), member("TemplateBody", body)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the update must be accepted, got {code} {answered}"
    );
    settled_status(case, router, region, stack).await
}

/// One row of the in-place table: the case, the stack and bucket it deploys, the versioning
/// state and policy action the create declares, and the pair the update declares.
struct InPlace {
    case: &'static str,
    stack: &'static str,
    bucket: &'static str,
    created: (Option<&'static str>, &'static str),
    updated: (Option<&'static str>, &'static str),
}

fn in_place_rows() -> Vec<InPlace> {
    vec![
        InPlace {
            case: "the policy alone",
            stack: "bucket-policy-update-policy",
            bucket: "bucket-policy-update-policy-store",
            created: (Some("Enabled"), "s3:GetObject"),
            updated: (Some("Enabled"), "s3:PutObject"),
        },
        InPlace {
            case: "the versioning state alone, on a bucket that had none",
            stack: "bucket-policy-update-versioning",
            bucket: "bucket-policy-update-versioning-store",
            created: (None, "s3:GetObject"),
            updated: (Some("Enabled"), "s3:GetObject"),
        },
        InPlace {
            case: "the versioning state and the policy at once",
            stack: "bucket-policy-update-both",
            bucket: "bucket-policy-update-both-store",
            created: (Some("Enabled"), "s3:GetObject"),
            updated: (Some("Suspended"), "s3:DeleteObject"),
        },
    ]
}

/// RULE 1: see the module header — a change Amazon S3 publishes a call for is applied to the
/// bucket that exists, and the stack completes.
#[tokio::test]
async fn an_update_of_a_bucket_and_its_policy_is_applied_in_place() {
    for row in in_place_rows() {
        let case = row.case;
        let router = router();
        let (code, body) = create_stack(
            &router,
            row.stack,
            &template(row.bucket, row.created.0, row.created.1),
            &[],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the create must be accepted, got {code} {body}"
        );
        let created = settled_status(case, &router, REGION, row.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {created:?}"
        );
        let stored = s3(
            &router,
            "PUT",
            &format!("/{}/{HELD_KEY}", row.bucket),
            BODY,
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the object must be stored first, got {} {}",
            stored.status, stored.body
        );

        let settled = updated(
            case,
            &router,
            REGION,
            row.stack,
            &template(row.bucket, row.updated.0, row.updated.1),
        )
        .await;
        assert_eq!(
            settled,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the update to complete, got {settled:?}; the stack reports \
             {:?}",
            reported_reasons(&router, REGION, row.stack).await
        );

        // The resource kept its identity...
        let published = published_id(case, &router, REGION, row.stack, BUCKET_LOGICAL_ID).await;
        assert_eq!(
            published, row.bucket,
            "case {case:?}: an in-place update keeps the bucket's identity, expected {:?}, got \
             {published:?}",
            row.bucket
        );
        // ...with what the update declared in force, read through Amazon S3's own wire...
        let reported = bucket_versioning(case, &router, row.bucket).await;
        assert_eq!(
            reported.as_deref(),
            row.updated.0,
            "case {case:?}: expected the declared versioning state {:?} in force, got \
             {reported:?}",
            row.updated.0
        );
        let policy = s3(&router, "GET", &format!("/{}?policy", row.bucket), b"").await;
        assert_eq!(
            policy.status, 200,
            "case {case:?}: the bucket must carry the declared policy, got {} {}",
            policy.status, policy.body
        );
        assert!(
            policy.body.contains(row.updated.1),
            "case {case:?}: expected the stored policy to carry the declared action {:?}, got {}",
            row.updated.1,
            policy.body
        );
        // ...and the object the bucket held is still there, byte for byte: a change in place
        // neither empties nor recreates the bucket.
        let read = s3(
            &router,
            "GET",
            &format!("/{}/{HELD_KEY}", row.bucket),
            b"",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the object the bucket held must survive the update, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.body.as_bytes(),
            BODY,
            "case {case:?}: the stored bytes must come back unchanged, got {:?}",
            read.body
        );
    }
}

/// Who holds the name a row's rename collides with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// A bucket a client created outside any stack.
    AClient,
    /// A bucket another live stack of this Region manages.
    AnotherStack,
}

/// One row of the refused-rename table: the case, the stack and bucket it deploys, the name the
/// rename asks for, and who already holds that name.
struct Refused {
    case: &'static str,
    stack: &'static str,
    bucket: &'static str,
    taken: &'static str,
    holder: Holder,
}

fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "a rename onto a bucket a client took outside any stack",
            stack: "bucket-rename-refused-client",
            bucket: "bucket-rename-refused-client-store",
            taken: "bucket-rename-refused-client-taken",
            holder: Holder::AClient,
        },
        Refused {
            case: "a rename onto a bucket another live stack manages",
            stack: "bucket-rename-refused-stack",
            bucket: "bucket-rename-refused-stack-store",
            taken: "bucket-rename-refused-stack-taken",
            holder: Holder::AnotherStack,
        },
    ]
}

/// RULE 2: see the module header — a replacement the owning service refuses rolls the stack
/// back to the resource it had.
#[tokio::test]
async fn a_rename_onto_a_name_already_held_rolls_the_stack_back() {
    for row in refused_rows() {
        let case = row.case;
        let router = router();

        // The name the rename collides with, held as the row says.
        match row.holder {
            Holder::AClient => {
                // Taken outside any stack, through Amazon S3's own wire. The create is signed
                // for North Virginia, which is where this crate's one S3 caller addresses the
                // service; the bucket NAME it takes is the account's in every Region, which is
                // what makes the rename below a collision.
                let created = s3(&router, "PUT", &format!("/{}", row.taken), b"").await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: the bucket the rename collides with must be created first, \
                     got {} {}",
                    created.status, created.body
                );
            }
            Holder::AnotherStack => {
                let other = format!("{}-owner", row.stack);
                let (code, body) = call_in(
                    &router,
                    NON_LEGACY_REGION,
                    "CreateStack",
                    &[
                        member("StackName", &other),
                        member(
                            "TemplateBody",
                            &template(row.taken, Some("Enabled"), "s3:GetObject"),
                        ),
                    ],
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: the owning stack's create must be accepted, got {code} {body}"
                );
                let created = settled_status(case, &router, NON_LEGACY_REGION, &other).await;
                assert_eq!(
                    created,
                    status::CREATE_COMPLETE,
                    "case {case:?}: the owning stack must complete first, got {created:?}"
                );
            }
        }

        let (code, body) = call_in(
            &router,
            NON_LEGACY_REGION,
            "CreateStack",
            &[
                member("StackName", row.stack),
                member(
                    "TemplateBody",
                    &template(row.bucket, Some("Enabled"), "s3:GetObject"),
                ),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the create must be accepted, got {code} {body}"
        );
        let created = settled_status(case, &router, NON_LEGACY_REGION, row.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {created:?}"
        );
        let stored = s3(
            &router,
            "PUT",
            &format!("/{}/{HELD_KEY}", row.bucket),
            BODY,
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the object must be stored first, got {} {}",
            stored.status, stored.body
        );

        // The RENAME the owning service refuses: the name is already taken, and this stack is
        // in a Region where re-creating a bucket the account owns is a refusal.
        let settled = updated(
            case,
            &router,
            NON_LEGACY_REGION,
            row.stack,
            &template(row.taken, Some("Enabled"), "s3:GetObject"),
        )
        .await;
        assert_eq!(
            settled,
            status::UPDATE_ROLLBACK_COMPLETE,
            "case {case:?}: expected the refused replacement to roll the stack back, got \
             {settled:?}; the stack reports {:?}",
            reported_reasons(&router, NON_LEGACY_REGION, row.stack).await
        );

        // The resource in force is the bucket the stack had, with its object still in it.
        let published =
            published_id(case, &router, NON_LEGACY_REGION, row.stack, BUCKET_LOGICAL_ID).await;
        assert_eq!(
            published, row.bucket,
            "case {case:?}: a rolled-back rename leaves the bucket the stack had, expected {:?}, \
             got {published:?}",
            row.bucket
        );
        let read = s3(
            &router,
            "GET",
            &format!("/{}/{HELD_KEY}", row.bucket),
            b"",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the bucket in force must still hold its object, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.body.as_bytes(),
            BODY,
            "case {case:?}: the stored bytes must come back unchanged, got {:?}",
            read.body
        );
        // And the bucket the rename collided with is left exactly as it is: the stack never
        // managed it.
        let head = s3(&router, "HEAD", &format!("/{}", row.taken), b"").await;
        assert_eq!(
            head.status, 200,
            "case {case:?}: the bucket the rename collided with must be left as it is, got {}",
            head.status
        );
    }
}
