// SPDX-License-Identifier: CC0-1.0

//! What DataSync proves about the BUCKET before it stores an Amazon S3 transfer location.
//!
//! RULE: a location is a transfer endpoint on a bucket, so `CreateLocationS3` resolves the
//! bucket its `S3BucketArn` names in the object store and refuses the request with
//! `InvalidRequestException` (400) — storing nothing — when the object store answers that
//! it holds no such bucket. An answer the object store did not give establishes nothing: an
//! `S3BucketArn` that addresses an access point rather than a bucket is not refused on
//! these grounds.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body — against a router that
//! mounts DataSync beside this emulator's own Amazon S3 and IAM on ONE engine, so the
//! buckets the pre-flight resolves are real buckets of the object store, created and
//! removed through its own API.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request in this suite is signed for, and the one its SDK clients
/// address.
const REGION: &str = "us-west-2";

/// The one bucket access role these cases name.
const ROLE_NAME: &str = "datasync-bucket-access";

/// A bucket the object store holds throughout.
const HELD_BUCKET: &str = "preflight-held";

/// A second one, so the rule is not keyed to a single name.
const SECOND_HELD_BUCKET: &str = "preflight-second";

/// A bucket that is created and then removed again.
const DELETED_BUCKET: &str = "preflight-deleted";

/// A bucket name nothing is ever created under.
const ABSENT_BUCKET: &str = "preflight-absent";

/// The access point an `S3BucketArn` may address instead of a bucket.
const ACCESS_POINT: &str = "preflight-access-point";

/// DataSync, Amazon S3 and IAM on one shared engine.
struct Aws {
    router: MultiServiceRouter,
}

impl Aws {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(Arc::clone(&engine)),
        );
        // The bucket a location names is a bucket of THIS object store.
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // Whether the bucket access role can be assumed, and what it allows, is IAM's.
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    fn callback(&self) -> EmbeddedEmulator {
        EmbeddedEmulator::from_router(self.router.clone())
    }

    fn s3(&self) -> ::aws_sdk_s3::Client {
        ::service_common::s3_client::s3_client(&self.callback(), REGION)
    }

    fn iam(&self) -> ::aws_sdk_iam::Client {
        ::service_common::iam_client::iam_client_through(&self.callback(), REGION)
    }

    /// One `awsJson1_1` DataSync call, returning the HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("FmrsService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{REGION}\
                     /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&payload)
                .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
        };
        (status, parsed)
    }

    /// A DataSync call that must succeed.
    async fn ok(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create `bucket` in the object store.
    async fn create_bucket(&self, bucket: &str, case: &str) {
        let created = self
            .s3()
            .create_bucket()
            .bucket(bucket)
            .create_bucket_configuration(
                ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                    .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(
                        REGION,
                    ))
                    .build(),
            )
            .send()
            .await;
        assert!(
            created.is_ok(),
            "case {case:?}: Amazon S3 must create the bucket {bucket}, got {created:?}"
        );
    }

    /// Remove `bucket` from the object store again.
    async fn delete_bucket(&self, bucket: &str, case: &str) {
        let deleted = self.s3().delete_bucket().bucket(bucket).send().await;
        assert!(
            deleted.is_ok(),
            "case {case:?}: Amazon S3 must remove the bucket {bucket}, got {deleted:?}"
        );
    }

    /// Create the bucket access role this suite transfers under: a trust policy admitting
    /// DataSync, and an inline policy that allows the bucket listing over EVERY value the
    /// rows name — so the only thing any row can be refused for is the bucket itself.
    async fn create_role(&self, resources: &[String]) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "datasync.amazonaws.com" },
                "Action": "sts:AssumeRole",
            }],
        });
        let created = self
            .iam()
            .create_role()
            .role_name(ROLE_NAME)
            .assume_role_policy_document(trust.to_string())
            .send()
            .await
            .expect("IAM creates the bucket access role");
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ["s3:ListBucket", "s3:GetObject", "s3:PutObject"],
                "Resource": resources,
            }],
        });
        self.iam()
            .put_role_policy()
            .role_name(ROLE_NAME)
            .policy_name("bucket-access")
            .policy_document(document.to_string())
            .send()
            .await
            .expect("IAM stores the role's inline policy");
        created
            .role()
            .expect("the created role is reported")
            .arn()
            .to_string()
    }
}

/// The ARN of a plain Amazon S3 bucket.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// The ARN of an Amazon S3 access point of this account, the other form `S3BucketArn`
/// admits (`arn:<partition>:s3:<region>:<account>:accesspoint/<name>`).
fn access_point_arn(name: &str) -> String {
    format!(
        "arn:aws:s3:{REGION}:{account}:accesspoint/{name}",
        account = engine::emulator_account_id()
    )
}

/// RULE: `CreateLocationS3` stores a location only for a bucket the object store reports
/// holding; a bucket it answers is not there is refused with
/// `InvalidRequestException` (400) naming that bucket, and nothing is stored. An
/// `S3BucketArn` that addresses no bucket at all is not refused on these grounds.
///
/// The rows differ along what the rule generalises over: a bucket that is held, a second
/// one so the rule is not keyed to one name, a name nothing was ever created under, a
/// bucket that was created and then removed, and an access point, which is the other form
/// the member admits.
#[tokio::test]
async fn create_location_s3_resolves_the_bucket_in_the_object_store() {
    let aws = Aws::new();
    for bucket in [HELD_BUCKET, SECOND_HELD_BUCKET, DELETED_BUCKET] {
        aws.create_bucket(bucket, "setup").await;
    }
    let role_arn = aws
        .create_role(&[
            bucket_arn(HELD_BUCKET),
            bucket_arn(SECOND_HELD_BUCKET),
            bucket_arn(DELETED_BUCKET),
            bucket_arn(ABSENT_BUCKET),
            access_point_arn(ACCESS_POINT),
        ])
        .await;
    aws.delete_bucket(DELETED_BUCKET, "setup").await;

    // (case, the S3BucketArn the request carries, the bucket a refusal must name)
    let rows = [
        (
            "a bucket the object store holds",
            bucket_arn(HELD_BUCKET),
            None,
        ),
        (
            "a second bucket the object store holds",
            bucket_arn(SECOND_HELD_BUCKET),
            None,
        ),
        (
            "a bucket name nothing was created under",
            bucket_arn(ABSENT_BUCKET),
            Some(ABSENT_BUCKET),
        ),
        (
            "a bucket that was created and then removed",
            bucket_arn(DELETED_BUCKET),
            Some(DELETED_BUCKET),
        ),
        (
            // An access point is a resource of its own: the object store is not asked
            // about a bucket here, so nothing is established about one.
            "an access point, which addresses no bucket",
            access_point_arn(ACCESS_POINT),
            None,
        ),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (case, s3_bucket_arn, absent_bucket) in rows {
        let request = json!({
            "S3BucketArn": s3_bucket_arn,
            "S3Config": { "BucketAccessRoleArn": role_arn },
        });
        match absent_bucket {
            None => {
                let created = aws.ok("CreateLocationS3", request, case).await;
                let location_arn = created["LocationArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a LocationArn, got {created}")
                    })
                    .to_string();
                accepted.push(location_arn);
            }
            Some(bucket) => {
                let (status, parsed) = aws.call("CreateLocationS3", request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected CreateLocationS3 to answer 400, got {status} \
                     with {parsed}"
                );
                let wire_type = parsed["__type"].as_str().unwrap_or_default();
                assert!(
                    wire_type.contains("InvalidRequestException"),
                    "case {case:?}: expected an InvalidRequestException, got {parsed}"
                );
                assert_eq!(
                    parsed["message"].as_str(),
                    Some(
                        format!(
                            "Datasync bucket access test failed: no such bucket {bucket} exists."
                        )
                        .as_str()
                    ),
                    "case {case:?}: the refusal must name the bucket it could not find, \
                     got {parsed}"
                );
            }
        }
    }

    // A refused create stored nothing: the listing holds exactly the accepted locations.
    let listed = aws.ok("ListLocations", json!({}), "listing").await;
    let held: Vec<String> = listed["Locations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry["LocationArn"].as_str().map(ToString::to_string))
        .collect();
    assert_eq!(
        held, accepted,
        "case \"listing\": expected only the accepted locations, got {listed}"
    );
}
