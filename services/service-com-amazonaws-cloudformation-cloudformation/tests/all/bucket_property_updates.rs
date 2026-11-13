// SPDX-License-Identifier: CC0-1.0

//! A CHANGED declaration of an `AWS::S3::Bucket` the stack already has, and what a bucket
//! name already in use reports — both end to end over the awsQuery wire.
//!
//! **The first rule.** A bucket is not re-created to change it: a property Amazon S3
//! publishes a call to write on a bucket that EXISTS is applied by that call to the bucket the
//! stack created, so the resource keeps its identity and the stack settles `UPDATE_COMPLETE`.
//! The properties that call reaches are the bucket's `Tags` (`PutBucketTagging`, and
//! `DeleteBucketTagging` for a declaration that no longer carries any) and its
//! `VersioningConfiguration` (`PutBucketVersioning`) — each read back here through Amazon S3's
//! own wire, so what is asserted is what a client of that bucket sees.
//!
//! The rows differ along what the rule generalises over: whether the declaration ADDS a tag
//! set to a bucket that had none, changes a tag's VALUE, adds a second key, DROPS the set
//! altogether, turns versioning on, suspends it, or changes both properties at once. Beside
//! whatever the declaration asks for, the bucket carries the three tags CloudFormation records
//! on every resource it provisions, which are CloudFormation's to set and which no revision of
//! the declaration changes.
//!
//! **The second rule.** What CloudFormation quotes in a failed resource's
//! `ResourceStatusReason` is the message the resource's own PROVIDER reported, and the bucket
//! provider reports a name already in use as the bucket NAME it asked for — so the event says
//! which bucket collided. That is the reason for a name no stack of the Region manages;
//! a name one of its stacks DOES manage is reported as CloudFormation's own collision
//! (`cross_stack_name_collision.rs`).

use serde_json::{json, Value};

use crate::harness::{call, call_in, create_stack, member, router, wait_for_the_transition};
use crate::stack_lifecycle::{describe_stack, xml_text, xml_texts};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

/// The bucket every row of the first rule deploys.
const BUCKET: &str = "bucket-property-updates-store";

/// The template one revision deploys: the bucket under its fixed name, with the properties
/// the revision declares.
fn bucket_template(properties: Value) -> String {
    let mut declared = json!({"BucketName": BUCKET});
    if let (Some(target), Some(source)) = (declared.as_object_mut(), properties.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    json!({"Resources": {"Store": {"Type": "AWS::S3::Bucket", "Properties": declared}}})
        .to_string()
}

/// The tag keys CloudFormation records on every resource it provisions. Source: the
/// `aws:cloudformation:*` tags CloudFormation's own documentation names
/// (`crate::state::provisioning`'s `STACK_NAME_TAG` / `STACK_ID_TAG` / `LOGICAL_ID_TAG`).
const STACK_NAME_TAG: &str = "aws:cloudformation:stack-name";
const STACK_ID_TAG: &str = "aws:cloudformation:stack-id";
const LOGICAL_ID_TAG: &str = "aws:cloudformation:logical-id";

/// The logical id the bucket is declared under in every revision below.
const LOGICAL_ID: &str = "Store";

/// One declared tag, as a template writes it.
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// One row: the properties the revision declares, and the tag set and versioning state Amazon
/// S3 must report once it has been applied.
struct Revision {
    case: &'static str,
    properties: Value,
    /// The tags the bucket must carry, as `(Key, Value)` pairs in any order.
    tags: Vec<(&'static str, &'static str)>,
    /// The versioning state Amazon S3 must report, or `None` where the bucket has none.
    versioning: Option<&'static str>,
}

fn revisions() -> Vec<Revision> {
    vec![
        Revision {
            case: "a tag set added to a bucket that carried none",
            properties: json!({"Tags": [tag("owner", "platform")]}),
            tags: vec![("owner", "platform")],
            versioning: None,
        },
        Revision {
            case: "a tag VALUE changed",
            properties: json!({"Tags": [tag("owner", "data")]}),
            tags: vec![("owner", "data")],
            versioning: None,
        },
        Revision {
            case: "a second key added beside it",
            properties: json!({"Tags": [tag("owner", "data"), tag("tier", "gold")]}),
            tags: vec![("owner", "data"), ("tier", "gold")],
            versioning: None,
        },
        Revision {
            case: "versioning enabled beside the tags",
            properties: json!({
                "Tags": [tag("owner", "data"), tag("tier", "gold")],
                "VersioningConfiguration": {"Status": "Enabled"}
            }),
            tags: vec![("owner", "data"), ("tier", "gold")],
            versioning: Some("Enabled"),
        },
        Revision {
            case: "the tag set dropped, versioning left as it is",
            properties: json!({"VersioningConfiguration": {"Status": "Enabled"}}),
            tags: Vec::new(),
            versioning: Some("Enabled"),
        },
        Revision {
            case: "versioning suspended",
            properties: json!({"VersioningConfiguration": {"Status": "Suspended"}}),
            tags: Vec::new(),
            versioning: Some("Suspended"),
        },
    ]
}

/// The tag set Amazon S3 reports for the bucket, as `(Key, Value)` pairs.
async fn bucket_tags(case: &str, router: &MultiServiceRouter) -> Vec<(String, String)> {
    let answer = s3(router, "GET", &format!("/{BUCKET}?tagging"), b"").await;
    if answer.status != 200 {
        // "NoSuchTagSet" is what Amazon S3 answers for a bucket carrying no tag set, which
        // is a bucket whose tags are the empty set.
        return Vec::new();
    }
    let keys = xml_texts(&answer.body, "Key");
    let values = xml_texts(&answer.body, "Value");
    assert_eq!(
        keys.len(),
        values.len(),
        "case {case:?}: expected one value per key, got {}",
        answer.body
    );
    keys.into_iter().zip(values).collect()
}

/// The versioning state Amazon S3 reports for the bucket, or `None` where it reports none.
async fn bucket_versioning(router: &MultiServiceRouter) -> Option<String> {
    let answer = s3(router, "GET", &format!("/{BUCKET}?versioning"), b"").await;
    (answer.status == 200)
        .then(|| xml_text(&answer.body, "Status"))
        .flatten()
}

/// The `PhysicalResourceId` the stack reports for the bucket.
async fn physical_id(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[member("StackName", stack), member("LogicalResourceId", LOGICAL_ID)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the bucket's record must be readable, got {code} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

#[tokio::test]
async fn a_changed_bucket_declaration_is_applied_to_the_bucket_that_exists() {
    let router = router();
    let stack = "bucket-property-updates";
    let (code, body) = create_stack(&router, stack, &bucket_template(json!({})), &[]).await;
    assert_eq!(
        code, 200,
        "the stack must be created first, got {code} {body}"
    );
    wait_for_the_transition().await;
    let created = physical_id("the create", &router, stack).await;
    assert_eq!(
        created, BUCKET,
        "the bucket must be created under the name the template states, got {created:?}"
    );
    // The stack's own id, which is the value of the `stack-id` tag every one of its
    // resources carries.
    let (code, described) = describe_stack(&router, stack).await;
    let stack_id = xml_text(&described, "StackId").unwrap_or_else(|| {
        panic!("the stack must report a StackId, got {code} {described}")
    });

    for row in revisions() {
        let case = row.case;
        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member("TemplateBody", &bucket_template(row.properties.clone())),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the update must be accepted, got {code} {body}"
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (code, body) = describe_stack(&router, stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {case:?}: expected the update to complete, got {code} {body}"
        );
        // The resource kept its identity: nothing was replaced to change it.
        let reported = physical_id(case, &router, stack).await;
        assert_eq!(
            reported, BUCKET,
            "case {case:?}: expected the bucket to keep its identity"
        );
        // ...and Amazon S3 itself reports what the declaration now asks for, beside the three
        // tags CloudFormation records on the resource.
        let mut reported_tags = bucket_tags(case, &router).await;
        reported_tags.sort();
        let mut expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected_tags.push((STACK_NAME_TAG.to_string(), stack.to_string()));
        expected_tags.push((STACK_ID_TAG.to_string(), stack_id.clone()));
        expected_tags.push((LOGICAL_ID_TAG.to_string(), LOGICAL_ID.to_string()));
        expected_tags.sort();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected Amazon S3 to report the declared tags"
        );
        assert_eq!(
            bucket_versioning(&router).await.as_deref(),
            row.versioning,
            "case {case:?}: expected Amazon S3 to report the declared versioning state"
        );
    }
}

/// RULE: the bucket provider reports a name already in use as the bucket name it asked for,
/// inside the wrapper CloudFormation quotes a provider's message in — so a client reads WHICH
/// bucket collided and that the failure is an `AlreadyExists` one.
///
/// The name is taken through Amazon S3's OWN wire rather than by another stack, because a
/// bucket name is taken from whoever holds it anywhere: a name no stack of the Region manages
/// is the case the provider's own message is reported for, while a name one of this Region's
/// stacks manages is reported as CloudFormation's own collision instead
/// (`cross_stack_name_collision.rs`). Such a create reaches the provider only with the
/// pre-deployment validations OFF — with them on, the existence check fails it before the
/// bucket is asked for (`state::early_validation`) — so the request below states
/// `DisableValidation`.
///
/// The collision is deployed in a Region other than the one Amazon S3 answers a re-create of
/// your own bucket with 200 in ("for legacy compatibility, if you re-create an existing bucket
/// that you already own in the North Virginia Region, Amazon S3 returns 200 OK"), since there
/// is no refusal to report where the create succeeds.
#[tokio::test]
async fn a_bucket_name_already_in_use_names_the_bucket_it_asked_for() {
    const REGION: &str = "us-west-2";
    const TAKEN: &str = "bucket-collision-taken";

    let router = router();
    let template = json!({
        "Resources": {"Store": {
            "Type": "AWS::S3::Bucket",
            "Properties": {"BucketName": TAKEN}
        }}
    })
    .to_string();
    let taken = s3(&router, "PUT", &format!("/{TAKEN}"), b"").await;
    assert_eq!(
        taken.status, 200,
        "the name must be taken first, got {} {}",
        taken.status, taken.body
    );

    // A STACK asking for that name: the create is refused, and the bucket's own event
    // carries the provider's report of it.
    let (code, body) = call_in(
        &router,
        REGION,
        "CreateStack",
        &[
            member("StackName", "bucket-collision-second"),
            member("TemplateBody", &template),
            // Keep the failed state, so the event the failure wrote is read rather than the
            // rollback's.
            member("OnFailure", "DO_NOTHING"),
            member("DisableValidation", "true"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "the colliding create is accepted and then fails, got {code} {body}"
    );
    wait_for_the_transition().await;

    let (code, body) = call_in(
        &router,
        REGION,
        "DescribeStackEvents",
        &[member("StackName", "bucket-collision-second")],
    )
    .await;
    assert_eq!(
        code, 200,
        "the failed stack's events must be readable, got {code} {body}"
    );
    let failed: Vec<(String, String)> = crate::harness::xml_blocks(&body, "member")
        .iter()
        .filter(|event| {
            xml_text(event, "ResourceStatus").as_deref() == Some(resource_status::CREATE_FAILED)
                && xml_text(event, "LogicalResourceId").as_deref() == Some("Store")
        })
        .map(|event| {
            (
                xml_text(event, "LogicalResourceId").unwrap_or_default(),
                xml_text(event, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        failed.len(),
        1,
        "expected exactly one failed resource, got {failed:?}"
    );
    let (logical_id, reason) = &failed[0];
    assert_eq!(logical_id, "Store", "expected the bucket to be the failure");
    // Fixed text apart from the bucket name, which is the template's, and the request token,
    // which is one per resource request.
    let opening = format!(
        "Resource handler returned message: \"{TAKEN} already exists (Service: S3, Status \
         Code: 0, Request ID: null)\" (RequestToken: "
    );
    assert!(
        reason.starts_with(&opening) && reason.ends_with(", HandlerErrorCode: AlreadyExists)"),
        "expected the provider's own report of the collision, got {reason:?}"
    );
}
