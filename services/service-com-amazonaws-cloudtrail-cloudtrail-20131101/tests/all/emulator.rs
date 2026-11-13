// SPDX-License-Identifier: CC0-1.0

//! The emulator under test, shared by this crate's suites.
//!
//! Every case goes over the wire: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: CloudTrail_20131101.<Operation>` through the same
//! `MultiServiceRouter` the production router serves, so what a suite asserts is
//! what a client sees — including the HTTP status and the error code.
//!
//! A trail's log-delivery destination is a bucket in the one object store, so a request
//! that names one has that bucket recorded there before it is dispatched — the
//! `CreateBucket` a client makes first. [`Emulator::without_provisioned_destinations`]
//! is the emulator that does not, for the suite whose subject IS the destination.

use serde_json::Value;
use std::sync::Arc;

/// The `X-Amz-Target` prefix CloudTrail publishes for its `awsJson1_1` API, as
/// every AWS SDK sends it: the prefix is itself dotted, and the operation name
/// follows the last dot.
pub const TARGET_PREFIX: &str = "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.";

/// The Region most cases call in.
pub const HOME_REGION: &str = "us-east-1";

/// The access key id the harness signs with unless a suite names another: the long-term
/// key this emulator publishes, which no IAM user holds — the account's own credential.
pub const AMBIENT_ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// This service's state, as a suite reaches it for a resource no implemented
/// operation can create yet.
pub type ServiceState = engine::ServiceState<
    service_com_amazonaws_cloudtrail_cloudtrail_20131101::ComAmazonawsCloudtrailCloudtrail20131101State,
>;

/// The emulator under test: one shared engine, one registered service, requests
/// dispatched through the production router.
pub struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    /// Whether the bucket a request's `S3BucketName` names is recorded in the one
    /// object store before the request is dispatched. See [`Emulator::call_in`].
    provision_destinations: bool,
}

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    /// The wire error code (`__type`), or `""` for a success.
    pub fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            // A `__type` may be namespaced (`com.amazonaws.cloudtrail#Foo`); the
            // code clients branch on is the shape name.
            .map(|t| t.rsplit('#').next().unwrap_or(t))
            .unwrap_or_default()
    }

    /// The refusal text a reply carries: the modelled error shapes spell the member
    /// `Message`, and the framework's pre-dispatch refusals spell it `message`, so a
    /// suite that asserts on wording reads both through here.
    pub fn message(&self) -> &str {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Assert this reply is the modelled failure `code` with HTTP `status`.
    pub fn expect_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code()),
            (status, code),
            "case {case}: expected HTTP {status} {code}, got {self:?}"
        );
    }

    /// Assert this reply is a success, and hand back its body.
    pub fn expect_ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {self:?}"
        );
        &self.body
    }
}

impl Emulator {
    pub fn new() -> Self {
        Self::with_destination_provisioning(true)
    }

    /// The same emulator with the trail DESTINATION left alone: the bucket a request's
    /// `S3BucketName` names is not recorded for it, which is how the refusal of a
    /// destination no bucket backs is asserted (`tests/all/request_validation.rs`).
    pub fn without_provisioned_destinations() -> Self {
        Self::with_destination_provisioning(false)
    }

    fn with_destination_provisioning(provision_destinations: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's;
        // pinning it off keeps these cases independent of how the process was
        // started.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self {
            router,
            engine,
            provision_destinations,
        }
    }

    /// Record a bucket named `bucket` in `region` in the one object store, carrying the
    /// bucket policy that lets CloudTrail deliver a trail's log files into it — what a
    /// client does with Amazon S3's `CreateBucket` and `PutBucketPolicy` before it points
    /// a trail at it.
    ///
    /// A trail's log-delivery destination is resolved THERE by `CreateTrail` and
    /// `UpdateTrail` (`state::referenced_resources`), so a suite whose subject is the
    /// trail registry rather than the destination needs the bucket its requests name to
    /// exist AND to permit the delivery. Recording the same name twice leaves the first
    /// bucket alone and re-attaches the policy.
    pub fn provision_bucket(&self, bucket: &str, region: &str) {
        self.provision_bucket_with_policy(bucket, region, Some(&cloudtrail_bucket_policy(bucket)));
    }

    /// [`Emulator::provision_bucket`], with the bucket policy a suite chooses — `None`
    /// for a bucket created and never policied, which is the state `CreateBucket` leaves
    /// a bucket in.
    pub fn provision_bucket_with_policy(&self, bucket: &str, region: &str, policy: Option<&str>) {
        let state = service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the object store");
        let mut store = state.lock();
        let _ = store.create_bucket(bucket, region);
        if let Some(held) = store.bucket_mut(bucket) {
            held.policy = policy.map(str::to_string);
        }
    }

    /// Delete the bucket named `bucket` from the one object store — the `DeleteBucket` a
    /// client makes out of band, after a trail was already pointed at it. `false` when
    /// the object store held no such bucket.
    pub fn delete_bucket(&self, bucket: &str) -> bool {
        let state = service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the object store");
        let mut store = state.lock();
        store.remove_bucket(bucket, std::time::SystemTime::now())
    }

    /// Record an Amazon SNS topic at `topic_arn` carrying the access policy that lets
    /// CloudTrail publish a log-file-delivered notification to it — the `CreateTopic` plus
    /// `SetTopicAttributes` a client makes before naming the topic on a trail.
    pub fn provision_topic(&self, topic_arn: &str) {
        self.provision_topic_with_policy(topic_arn, Some(&cloudtrail_topic_policy(topic_arn)));
    }

    /// [`Emulator::provision_topic`], with the access policy a suite chooses — `None` for
    /// a topic carrying no policy of its own, which Amazon SNS reports under the default
    /// policy it generates for one.
    pub fn provision_topic_with_policy(&self, topic_arn: &str, policy: Option<&str>) {
        let state = service_com_amazonaws_sns_amazonsimplenotificationservice::state::service_state(
            &*self.engine,
        )
        .expect("the in-memory engine carries the Amazon SNS store");
        let name = topic_arn.rsplit(':').next().unwrap_or(topic_arn).to_string();
        let owner = engine::emulator_account_id().to_string();
        let mut attributes = std::collections::HashMap::new();
        if let Some(policy) = policy {
            attributes.insert(
                service_com_amazonaws_sns_amazonsimplenotificationservice::state::POLICY_ATTRIBUTE
                    .to_string(),
                policy.to_string(),
            );
        }
        let record =
            service_com_amazonaws_sns_amazonsimplenotificationservice::state::TopicRecord {
                arn: topic_arn.to_string(),
                name,
                owner,
                attributes,
                tags: std::collections::HashMap::new(),
                data_protection_policy: None,
                is_fifo: false,
                created_at: std::time::SystemTime::now(),
                subscriptions_deleted: 0,
                subscriptions_deleting: Vec::new(),
                archive_enabled_at: None,
            };
        state.lock().topics.insert(topic_arn.to_string(), record);
    }

    /// Record a KMS key the identifier `key_id` resolves to in `region`, in every form
    /// the `KmsKeyId` member accepts — the `CreateKey` (and `CreateAlias`) a client makes
    /// before encrypting a trail's log files under it.
    pub fn provision_kms_key(&self, key_id: &str, region: &str) {
        use service_com_amazonaws_kms_trentservice::state as kms;

        // The Region and the identifier an ARN form pins the key to; a bare id or alias
        // names one in the Region the request was signed for.
        let (scope, identifier) = match service_common::arn::parse(key_id) {
            Some(arn) if arn.vendor == "kms" => (arn.region.to_string(), arn.resource.to_string()),
            Some(_) => return,
            None => (region.to_string(), key_id.to_string()),
        };
        let (alias, id) = match identifier.strip_prefix("alias/") {
            Some(name) => (Some(format!("alias/{name}")), fixture_key_id(name)),
            None => (
                None,
                identifier
                    .strip_prefix("key/")
                    .unwrap_or(&identifier)
                    .to_string(),
            ),
        };
        let state = kms::service_state(&*self.engine)
            .expect("the in-memory engine carries the KMS store");
        let mut store = state.lock();
        store.insert_key(kms::Cmk::new(
            id.clone(),
            scope.clone(),
            String::new(),
            kms::KEY_USAGE_ENCRYPT_DECRYPT.to_string(),
            kms::KEY_SPEC_SYMMETRIC_DEFAULT.to_string(),
            kms::ORIGIN_AWS_KMS.to_string(),
            false,
            kms::default_key_policy(),
            std::collections::BTreeMap::new(),
            kms::KeyMaterial::Aes256([0x5a; kms::MATERIAL_LEN]),
        ));
        if let Some(alias) = alias {
            store.insert_alias(kms::Alias::new(scope, alias, id));
        }
    }

    /// Record the CloudWatch Logs log group a trail's `CloudWatchLogsLogGroupArn` names —
    /// the `CreateLogGroup` a client makes before pointing a trail at it. A value that is
    /// not a log-group ARN of this account names no group and records nothing.
    pub fn provision_log_group(&self, log_group_arn: &str) {
        use service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

        let Some(parsed) = service_common::arn::parse(log_group_arn) else {
            return;
        };
        if parsed.vendor != "logs" || parsed.resource_type() != "log-group" {
            return;
        }
        let name = logs::log_group_name_from_identifier(log_group_arn).to_string();
        if !service_common::cloudwatch_logs::is_log_group_name(&name) {
            return;
        }
        let state =
            logs::service_state(&*self.engine).expect("the in-memory engine carries the logs store");
        let record = logs::LogGroupRecord::new(parsed.region, &name, 0);
        state.lock().log_groups.insert(name, record);
    }

    /// This service's store in the shared engine — the SAME store every handler
    /// reads, which is how a suite seeds a resource whose creating operation is
    /// still a scaffold stub.
    pub fn state(&self) -> ServiceState {
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::service_state(&*self.engine)
            .expect("the in-memory engine carries this service's state")
    }

    /// The shared engine itself, for a suite that seeds the store of ANOTHER service of
    /// the account — the IAM records an authorization decision is read from
    /// (`tests/all/identity_authorization.rs`), which `iam:CreateUser` and
    /// `iam:PutUserPolicy` write.
    pub fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// The account's management-event log in the shared engine — the SAME log the
    /// handlers record into and `LookupEvents` reports from, which is how a suite
    /// seeds an event served at a known instant.
    pub fn management_event_log(
        &self,
    ) -> engine::ServiceState<service_common::management_events::ManagementEventLog> {
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::management_event_log(
            &*self.engine,
        )
        .expect("the in-memory engine carries the management-event log")
    }

    /// Call `operation` from `region` with `body`, under the target prefix every
    /// SDK sends ([`TARGET_PREFIX`]).
    pub async fn call_in(&self, operation: &str, region: &str, body: &Value) -> Reply {
        self.call_with_target(&format!("{TARGET_PREFIX}{operation}"), region, body)
            .await
    }

    /// Call with a verbatim `X-Amz-Target`, for pinning how a target header is
    /// read.
    ///
    /// A trail's settings name resources of OTHER services — the destination bucket, the
    /// notification topic, the encryption key and the CloudWatch Logs destination — and
    /// CloudTrail resolves every one of them before it accepts the trail. Every suite but
    /// the ones about those references is about the trail registry instead, so the harness
    /// records each resource the body names first, the way a client creates them before
    /// pointing a trail anywhere — unless it was built with
    /// [`Emulator::without_provisioned_destinations`].
    pub async fn call_with_target(&self, target: &str, region: &str, body: &Value) -> Reply {
        self.call_with_target_as(target, AMBIENT_ACCESS_KEY_ID, region, body)
            .await
    }

    /// [`Emulator::call_with_target`], signed with the access key id `access_key_id` —
    /// which is what a suite about WHO made the call varies, since the principal a request
    /// speaks for is resolved from the key its credential scope carries.
    pub async fn call_with_target_as(
        &self,
        target: &str,
        access_key_id: &str,
        region: &str,
        body: &Value,
    ) -> Reply {
        if self.provision_destinations {
            let member = |name: &str| {
                body.get(name)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            };
            if let Some(bucket) = member("S3BucketName") {
                self.provision_bucket(&bucket, region);
            }
            if let Some(topic) = member("SnsTopicName") {
                // `SnsTopicName` is "the name or ARN of the Amazon SNS topic", and the
                // trail keeps the ARN either form resolves to.
                let topic_arn = service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::
                    sns_topic_arn(region, &topic);
                self.provision_topic(&topic_arn);
            }
            if let Some(key_id) = member("KmsKeyId") {
                self.provision_kms_key(&key_id, region);
            }
            if let Some(log_group_arn) = member("CloudWatchLogsLogGroupArn") {
                self.provision_log_group(&log_group_arn);
            }
        }
        // The Region a request is served in comes from its SigV4 credential scope,
        // which is also how the router resolves the service.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/cloudtrail/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", target)
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Reply { status, body }
    }

    /// Call `operation` from [`HOME_REGION`].
    pub async fn call(&self, operation: &str, body: &Value) -> Reply {
        self.call_in(operation, HOME_REGION, body).await
    }

    /// Call `operation` from `region` signed with `access_key_id`, under the target prefix
    /// every SDK sends ([`TARGET_PREFIX`]).
    pub async fn call_in_as(
        &self,
        operation: &str,
        access_key_id: &str,
        region: &str,
        body: &Value,
    ) -> Reply {
        self.call_with_target_as(
            &format!("{TARGET_PREFIX}{operation}"),
            access_key_id,
            region,
            body,
        )
        .await
    }
}

/// The bucket policy a client attaches so CloudTrail may deliver a trail's log files
/// into `bucket`: the ACL check on the bucket and the object write under it, granted to
/// the CloudTrail service principal and scoped by the canned ACL the delivery carries —
/// the policy CloudTrail's own documentation states, with the write granted over the
/// WHOLE bucket so one fixture serves every `S3KeyPrefix` a suite points a trail at.
pub fn cloudtrail_bucket_policy(bucket: &str) -> String {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AWSCloudTrailAclCheck",
                "Effect": "Allow",
                "Principal": {"Service": "cloudtrail.amazonaws.com"},
                "Action": "s3:GetBucketAcl",
                "Resource": format!("arn:aws:s3:::{bucket}"),
            },
            {
                "Sid": "AWSCloudTrailWrite",
                "Effect": "Allow",
                "Principal": {"Service": "cloudtrail.amazonaws.com"},
                "Action": "s3:PutObject",
                "Resource": format!("arn:aws:s3:::{bucket}/*"),
                "Condition": {
                    "StringEquals": {"s3:x-amz-acl": "bucket-owner-full-control"},
                },
            },
        ],
    })
    .to_string()
}

/// The access policy a client attaches so CloudTrail may publish a trail's log-file
/// notifications to the topic `topic_arn`.
pub fn cloudtrail_topic_policy(topic_arn: &str) -> String {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AWSCloudTrailSNSPolicy",
            "Effect": "Allow",
            "Principal": {"Service": "cloudtrail.amazonaws.com"},
            "Action": "SNS:Publish",
            "Resource": topic_arn,
        }],
    })
    .to_string()
}

/// A 36-character KMS key id in the form the service mints (`^[0-9a-f-]+$`), derived
/// from `seed` so the same alias always names the same key.
fn fixture_key_id(seed: &str) -> String {
    let digest = seed
        .bytes()
        .fold(0u64, |acc, byte| acc.wrapping_mul(0x0100_0193) ^ u64::from(byte));
    let id = format!("{:08x}-0000-4000-8000-{:012x}", digest as u32, digest & 0xffff_ffff_ffff);
    assert_eq!(id.len(), 36, "a KMS key id is 36 characters: {id:?}");
    id
}
