// SPDX-License-Identifier: CC0-1.0

//! A configuration-snapshot delivery whose bucket, bucket policy and recorder role were
//! provisioned by a CloudFormation stack.
//!
//! The rule under test: **an on-demand configuration-snapshot delivery that Config ACCEPTS
//! is performed — Config writes exactly one gzip-framed JSON object into the delivery
//! channel's bucket, under `AWSLogs/<account-id>/Config/<region>/` behind the channel's own
//! `s3KeyPrefix` where it configures one, with the returned `configSnapshotId` in its key
//! and the configuration items the recorder has captured as its payload — and the channel's
//! `configSnapshotDeliveryInfo` then reports the outcome of THAT write: a successful
//! terminal `lastStatus` with `lastAttemptTime` and `lastSuccessfulTime` set and no error
//! members.** `DeliverConfigSnapshot` models no error for the delivery itself
//! (`NoSuchDeliveryChannelException` / `NoAvailableConfigurationRecorderException` /
//! `NoRunningConfigurationRecorderException` are its only errors), so once the call is
//! accepted the outcome is raised nowhere but that status and the object.
//!
//! It holds however the pipeline was wired, which is what this suite adds to the rest of
//! the crate: here nothing is created by hand. One CloudFormation stack declares the
//! delivery bucket, the bucket policy AWS documents for a delivery channel — including the
//! `AWSConfigBucketDelivery` statement conditioned on `s3:x-amz-acl` being
//! `bucket-owner-full-control`, which a hand-written policy elsewhere in this crate leaves
//! out — and the IAM role trusting `config.amazonaws.com` the recorder runs as, and the
//! channel is then pointed at the stack's own outputs.
//!
//! Four services are mounted on ONE engine, because the pipeline spans them:
//! CloudFormation makes the provisioning calls, S3 holds the bucket and answers about its
//! policy, IAM answers whether Config may assume the recorder's role, and Config records
//! and delivers. CloudFormation and Config are driven over the wire, the way a client
//! drives them; the buckets and the delivered object are read with the AWS SDK for S3 over
//! the emulator's own in-process callback.
//!
//! The rows differ along what the rule generalises over in WHERE the object lands and what
//! the policy has to cover for it: the key prefix the channel configures (none, one, one
//! already ending in the delimiter) and the resource the stack's delivery statement grants
//! the write on (every object of the bucket, or just this account's Config path under the
//! channel's prefix).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

// The ONE decoder of a delivered snapshot this crate's test binary holds, which the suite
// that delivers through a hand-written policy writes (`config_snapshot_delivery`): a second
// copy here would let the two drift about what a delivered object is.
use crate::config_snapshot_delivery::gunzipped_snapshot;

/// The Region every request below is signed for, and so the Region the stack, the buckets
/// and the recorder belong to.
const REGION: &str = "us-west-2";

/// The service principal Config delivers as, which the stack's bucket policy and the
/// recorder role's trust document both name.
const CONFIG_PRINCIPAL: &str = "config.amazonaws.com";

/// The canned ACL the documented `AWSConfigBucketDelivery` statement conditions its grant
/// on (AWS Config Developer Guide, *Permissions for the Amazon S3 bucket*).
const DELIVERY_OBJECT_ACL: &str = "bucket-owner-full-control";

/// The resource type Config records an Amazon S3 general purpose bucket under — an entry of
/// `com.amazonaws.configservice#ResourceType`.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// The `lastStatus` a delivery that was performed reports. The model spells
/// `com.amazonaws.configservice#DeliveryStatus` as `Success`; the service answers it
/// upper-cased, which is pinned here as a literal.
const SUCCESS: &str = "SUCCESS";

/// The logical ids the template declares its resources under.
const BUCKET_LOGICAL_ID: &str = "DeliveryBucket";
/// See [`BUCKET_LOGICAL_ID`].
const POLICY_LOGICAL_ID: &str = "DeliveryBucketPolicy";
/// See [`BUCKET_LOGICAL_ID`].
const ROLE_LOGICAL_ID: &str = "RecorderRole";

/// The stack outputs a row reads the bucket and the role out of.
const BUCKET_OUTPUT: &str = "BucketName";
/// See [`BUCKET_OUTPUT`].
const ROLE_OUTPUT: &str = "RoleArn";

/// How long a poll waits for work the clock owes — the stack's resources, the recorder
/// status transition, a recording pass and the delivery, each due a fraction of a second
/// after the call that made it due. Many passes' worth of slack, not a span the behaviour
/// depends on.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(30);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The account id every ARN this emulator mints carries, which the stack's policy
/// conditions its grants on.
fn account_id() -> &'static str {
    ::emulator_server::emulator_account_id()
}

/// CloudFormation, S3, IAM and Config over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not any of these services'.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// A SigV4 `Authorization` header whose credential scope names `signing_name` — the key
    /// the router claims a request by and each handler reads its Region from. Nothing is
    /// verified: this router runs with request authentication off.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
        )
    }

    /// One CloudFormation action over its awsQuery wire.
    async fn cloudformation(&self, action: &str, members: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-15");
        for (name, value) in members {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("cloudformation"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One Config call (`awsJson1_1`), answering the status and the parsed body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("config.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("config"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("StarlingDoveService.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A Config call that must answer 200.
    async fn config(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
        );
        parsed
    }

    /// An S3 client over the in-process callback, addressing path style exactly as the
    /// delivery's own client does.
    fn s3(&self) -> aws_sdk_s3::Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
            .force_path_style(true)
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_s3::Client::from_conf(config)
    }
}

/// `value` as one form-encoded awsQuery parameter: every byte that is not unreserved is
/// percent-encoded, which is what lets a JSON template body travel as a parameter.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            byte => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The text of every `<tag>` element of an awsQuery answer, in document order.
fn elements(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        found.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    found
}

/// A stack's `Outputs`, as the key/value map a client reads them as.
fn outputs_of(xml: &str) -> std::collections::BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The role trust document the stack declares for the configuration recorder: Config
/// assumes that role to deliver, so the role has to admit the Config service principal.
fn recorder_trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": CONFIG_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The bucket policy AWS documents for a Config delivery channel's bucket, as a stack
/// declares it: the bucket-level grants the service's own checks make, and the
/// `AWSConfigBucketDelivery` grant of `s3:PutObject` on `objects`, conditioned on the
/// canned ACL Config writes a delivered object with and on the delivering account.
fn delivery_bucket_policy(objects: Value) -> Value {
    let bucket = json!({ "Fn::Sub": format!("arn:aws:s3:::${{{BUCKET_LOGICAL_ID}}}") });
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AWSConfigBucketPermissionsCheck",
                "Effect": "Allow",
                "Principal": { "Service": CONFIG_PRINCIPAL },
                "Action": "s3:GetBucketAcl",
                "Resource": bucket.clone(),
                "Condition": { "StringEquals": { "AWS:SourceAccount": account_id() } },
            },
            {
                "Sid": "AWSConfigBucketExistenceCheck",
                "Effect": "Allow",
                "Principal": { "Service": CONFIG_PRINCIPAL },
                "Action": "s3:ListBucket",
                "Resource": bucket,
                "Condition": { "StringEquals": { "AWS:SourceAccount": account_id() } },
            },
            {
                "Sid": "AWSConfigBucketDelivery",
                "Effect": "Allow",
                "Principal": { "Service": CONFIG_PRINCIPAL },
                "Action": "s3:PutObject",
                "Resource": objects,
                "Condition": {
                    "StringEquals": {
                        "s3:x-amz-acl": DELIVERY_OBJECT_ACL,
                        "AWS:SourceAccount": account_id(),
                    },
                },
            },
        ],
    })
}

/// The template a row deploys: the delivery bucket, the documented bucket policy pointed at
/// the row's own object resource, and the role the recorder runs as — with the bucket name
/// and the role ARN as the stack's outputs, which is what a client wires the channel from.
fn template(bucket: &str, objects: Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "An AWS Config delivery bucket, its policy and the recorder's role",
        "Resources": {
            BUCKET_LOGICAL_ID: {
                "Type": "AWS::S3::Bucket",
                "Properties": { "BucketName": bucket },
            },
            POLICY_LOGICAL_ID: {
                "Type": "AWS::S3::BucketPolicy",
                "Properties": {
                    "Bucket": { "Ref": BUCKET_LOGICAL_ID },
                    "PolicyDocument": delivery_bucket_policy(objects),
                },
            },
            ROLE_LOGICAL_ID: {
                "Type": "AWS::IAM::Role",
                "Properties": { "AssumeRolePolicyDocument": recorder_trust_policy() },
            },
        },
        "Outputs": {
            BUCKET_OUTPUT: { "Value": { "Ref": BUCKET_LOGICAL_ID } },
            ROLE_OUTPUT: { "Value": { "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] } },
        },
    })
    .to_string()
}

/// Create the row's stack and read it until it reaches a terminal status; answer the
/// `DescribeStacks` body of that observation.
async fn create_and_settle(case: &str, emulator: &Emulator, stack: &str, template: &str) -> String {
    let (status, body) = emulator
        .cloudformation(
            "CreateStack",
            &[
                ("StackName", stack),
                ("TemplateBody", template),
                // The template declares an IAM role, so the request acknowledges it.
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let (status, described) = emulator
            .cloudformation("DescribeStacks", &[("StackName", stack)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with \
             {described}"
        );
        let reported = elements(&described, "StackStatus")
            .into_iter()
            .next()
            .unwrap_or_default();
        if reported.ends_with("_COMPLETE") || reported.ends_with("_FAILED") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stack to settle, it is still {reported:?} in \
             {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read the recorder's status until Config reports it recording.
async fn await_recording(case: &str, emulator: &Emulator, recorder: &str) {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let reported = emulator
            .config(
                case,
                "DescribeConfigurationRecorderStatus",
                &json!({ "ConfigurationRecorderNames": [recorder] }),
            )
            .await;
        let status = reported["ConfigurationRecordersStatus"][0].clone();
        if status["recording"] == json!(true) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the recorder to be recording, got {status}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read `GetResourceConfigHistory` for `bucket` until the recorder has captured a
/// configuration item for it, and answer that item.
async fn await_recorded_item(case: &str, emulator: &Emulator, bucket: &str) -> Value {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let (status, body) = emulator
            .call(
                "GetResourceConfigHistory",
                &json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceId": bucket }),
            )
            .await;
        if status == 200 {
            if let Some(item) = body["configurationItems"]
                .as_array()
                .and_then(|items| items.last())
            {
                return item.clone();
            }
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a configuration item for {bucket}, got {status} with \
             {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read the channel's `configSnapshotDeliveryInfo` until the delivery reports a terminal
/// status, and answer it.
async fn await_settled_delivery(case: &str, emulator: &Emulator, channel: &str) -> Value {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let reported = emulator
            .config(
                case,
                "DescribeDeliveryChannelStatus",
                &json!({ "DeliveryChannelNames": [channel] }),
            )
            .await;
        let info = reported["DeliveryChannelsStatus"][0]["configSnapshotDeliveryInfo"].clone();
        if info["lastStatus"].is_string() {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the delivery to reach a terminal lastStatus, got {info}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every key the delivery bucket holds.
async fn delivered_keys(case: &str, emulator: &Emulator, bucket: &str) -> Vec<String> {
    let listed = emulator
        .s3()
        .list_objects_v2()
        .bucket(bucket)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: listing {bucket} must succeed, got {error:?}")
        });
    listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// Read the delivery bucket until it holds a key carrying `snapshot_id`, and answer every
/// key it holds.
async fn await_delivered_keys(
    case: &str,
    emulator: &Emulator,
    bucket: &str,
    snapshot_id: &str,
) -> Vec<String> {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let keys = delivered_keys(case, emulator, bucket).await;
        if keys.iter().any(|key| key.contains(snapshot_id)) {
            return keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivered object naming {snapshot_id}, got {keys:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The bytes of one delivered object.
async fn delivered_body(case: &str, emulator: &Emulator, bucket: &str, key: &str) -> Vec<u8> {
    let object = emulator
        .s3()
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {key} must succeed, got {error:?}"));
    object
        .body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: {key} must be readable, got {error:?}"))
        .to_vec()
}

/// One row: the stack it deploys, the prefix the channel delivers under, and the resource
/// the stack's delivery statement grants the write on.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The bucket the stack creates and the channel delivers into.
    bucket: &'static str,
    /// The bucket created after recording starts, whose item the snapshot has to carry.
    observed: &'static str,
    /// The `s3KeyPrefix` the channel configures, if any.
    prefix: Option<&'static str>,
    /// The `Resource` of the stack's `AWSConfigBucketDelivery` statement, as a template
    /// expression over the bucket's own logical id.
    objects: fn() -> Value,
    /// The key root the delivered object has to sit under, which the channel's prefix and
    /// nothing else decides.
    expected_prefix: &'static str,
}

/// Every object resource below is written against the stack's own bucket, so a row states
/// what the policy covers without naming the bucket twice.
fn every_object() -> Value {
    json!({ "Fn::Sub": format!("arn:aws:s3:::${{{BUCKET_LOGICAL_ID}}}/*") })
}

/// See [`every_object`]: the account's own Config path under the channel's prefix, which is
/// the narrowest resource a delivery into it can be granted on.
fn config_path_under_audit() -> Value {
    json!({
        "Fn::Sub": format!(
            "arn:aws:s3:::${{{BUCKET_LOGICAL_ID}}}/audit/AWSLogs/{}/Config/*",
            account_id()
        ),
    })
}

#[tokio::test]
async fn a_stack_provisioned_pipeline_delivers_a_snapshot_carrying_the_recorded_item() {
    let rows: [Row; 3] = [
        Row {
            case: "no key prefix, every object granted",
            stack: "config-delivery-plain",
            bucket: "config-delivery-plain",
            observed: "audited-plain",
            prefix: None,
            objects: every_object,
            expected_prefix: "",
        },
        Row {
            case: "a key prefix, only the account's Config path under it granted",
            stack: "config-delivery-prefixed",
            bucket: "config-delivery-prefixed",
            observed: "audited-prefixed",
            prefix: Some("audit"),
            objects: config_path_under_audit,
            expected_prefix: "audit/",
        },
        Row {
            case: "a key prefix ending in the delimiter, every object granted",
            stack: "config-delivery-trailing",
            bucket: "config-delivery-trailing",
            observed: "audited-trailing",
            prefix: Some("audit/"),
            objects: every_object,
            expected_prefix: "audit/",
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();

        // The pipeline is PROVISIONED: the bucket, its policy and the recorder's role all
        // come from the stack, and the channel is wired from the stack's own outputs.
        let described = create_and_settle(
            case,
            &emulator,
            row.stack,
            &template(row.bucket, (row.objects)()),
        )
        .await;
        let reported_status = elements(&described, "StackStatus")
            .into_iter()
            .next()
            .unwrap_or_default();
        assert_eq!(
            reported_status, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to create completely, got {described}"
        );
        let outputs = outputs_of(&described);
        let bucket = outputs.get(BUCKET_OUTPUT).cloned().unwrap_or_default();
        let role_arn = outputs.get(ROLE_OUTPUT).cloned().unwrap_or_default();
        assert_eq!(
            bucket, row.bucket,
            "case {case:?}: the stack's bucket output names the bucket it declared, got \
             {outputs:?}"
        );
        assert!(
            role_arn.starts_with(&format!("arn:aws:iam::{}:role/", account_id())),
            "case {case:?}: the stack's role output is the role's own ARN, got {outputs:?}"
        );

        let recorder = format!("{}-recorder", row.stack);
        let channel = format!("{}-channel", row.stack);
        emulator
            .config(
                case,
                "PutConfigurationRecorder",
                &json!({
                    "ConfigurationRecorder": {
                        "name": recorder,
                        "roleARN": role_arn,
                        "recordingGroup": {
                            "allSupported": true,
                            "includeGlobalResourceTypes": false,
                        },
                    }
                }),
            )
            .await;
        let mut declared = json!({ "name": channel, "s3BucketName": bucket });
        if let Some(prefix) = row.prefix {
            declared["s3KeyPrefix"] = json!(prefix);
        }
        emulator
            .config(
                case,
                "PutDeliveryChannel",
                &json!({ "DeliveryChannel": declared }),
            )
            .await;
        emulator
            .config(
                case,
                "StartConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": recorder }),
            )
            .await;
        await_recording(case, &emulator, &recorder).await;

        // The resource the snapshot has to carry, created through its OWN service once the
        // recorder is running, and read back through Config's own history read.
        emulator
            .s3()
            .create_bucket()
            .bucket(row.observed)
            .create_bucket_configuration(
                aws_sdk_s3::types::CreateBucketConfiguration::builder()
                    .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                    .build(),
            )
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: creating {} must succeed, got {error:?}",
                    row.observed
                )
            });
        let recorded = await_recorded_item(case, &emulator, row.observed).await;
        assert!(
            recorded["arn"]
                .as_str()
                .is_some_and(|arn| arn.ends_with(row.observed)),
            "case {case:?}: the recorded item names the bucket's own ARN, got {recorded}"
        );

        // The delivery: accepted, then performed.
        let answered = emulator
            .config(
                case,
                "DeliverConfigSnapshot",
                &json!({ "deliveryChannelName": channel }),
            )
            .await;
        let snapshot_id = answered["configSnapshotId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a configSnapshotId, got {answered}")
            })
            .to_string();

        let info = await_settled_delivery(case, &emulator, &channel).await;
        assert_eq!(
            info["lastStatus"],
            json!(SUCCESS),
            "case {case:?}: a delivery that wrote its object reports {SUCCESS}, got {info}"
        );
        for member in ["lastErrorCode", "lastErrorMessage"] {
            assert!(
                info.get(member).is_none(),
                "case {case:?}: a delivery that succeeded reports no {member}, got {info}"
            );
        }
        let attempt = info["lastAttemptTime"].as_f64().unwrap_or_default();
        let successful = info["lastSuccessfulTime"].as_f64().unwrap_or_default();
        assert!(
            attempt > 0.0 && successful >= attempt,
            "case {case:?}: the delivery succeeded at or after it was attempted, got {info}"
        );

        // ONE object, under the prefix the channel configures, naming the snapshot the call
        // answered with.
        let keys = await_delivered_keys(case, &emulator, &bucket, &snapshot_id).await;
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: one delivery writes one object, got {keys:?}"
        );
        let key = keys[0].clone();
        let expected_root = format!(
            "{}AWSLogs/{}/Config/{REGION}/",
            row.expected_prefix,
            account_id()
        );
        assert!(
            key.starts_with(&expected_root) && key.ends_with(".json.gz"),
            "case {case:?}: expected a gzip-named key under {expected_root:?}, got {key:?}"
        );

        // The object IS the snapshot: gzip-framed JSON naming the delivery and carrying the
        // configuration items the recorder captured, the observed bucket's among them.
        let body = delivered_body(case, &emulator, &bucket, &key).await;
        let snapshot = gunzipped_snapshot(case, &body);
        assert_eq!(
            snapshot["configSnapshotId"],
            json!(snapshot_id),
            "case {case:?}: the snapshot names the delivery it was written for, got {snapshot}"
        );
        let items = snapshot["configurationItems"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the snapshot carries configurationItems, got {snapshot}")
            });
        let observed = items
            .iter()
            .find(|item| item["resourceId"] == json!(row.observed))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the snapshot carries the recorded bucket, got {snapshot}")
            });
        assert_eq!(
            (
                observed["awsRegion"].as_str(),
                observed["resourceType"].as_str(),
                observed["awsAccountId"].as_str(),
            ),
            (
                Some(REGION),
                Some(BUCKET_RESOURCE_TYPE),
                Some(account_id()),
            ),
            "case {case:?}: the delivered item names the Region, the type and the account it \
             was recorded in, got {observed}"
        );
        assert!(
            observed["ARN"]
                .as_str()
                .is_some_and(|arn| arn.ends_with(row.observed)),
            "case {case:?}: the delivered item carries the bucket's own ARN, got {observed}"
        );

        // And the delivery bucket, which the same recorder also records, is in the snapshot
        // too: a snapshot is a complete picture of what is being recorded, not of what one
        // call touched.
        assert!(
            items
                .iter()
                .any(|item| item["resourceId"] == json!(row.bucket)),
            "case {case:?}: the snapshot carries the stack's delivery bucket, got {snapshot}"
        );

        // Reading the status again reports the same settled delivery, and writes no further
        // object: a read delivers nothing of its own.
        let again = await_settled_delivery(case, &emulator, &channel).await;
        assert_eq!(
            info, again,
            "case {case:?}: reading the status changes nothing a later caller sees, got \
             {info} then {again}"
        );
        let unchanged = delivered_keys(case, &emulator, &bucket).await;
        assert_eq!(
            unchanged, keys,
            "case {case:?}: reading the status writes no further object, got {unchanged:?}"
        );
    }
}
