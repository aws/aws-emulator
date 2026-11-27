// SPDX-License-Identifier: CC0-1.0

//! Provisioning an AWS Config configuration recorder the way AWS documents it CONVERGES,
//! and the resource-config plane behind it then answers.
//!
//! The rule under test: **a control-plane precondition gate refuses only while its
//! precondition is really unmet, so once the account holds what the gate reads — an IAM role
//! `config.amazonaws.com` can assume, a delivery bucket whose grants admit the delivery, and
//! a delivery channel — the very NEXT call is served: `PutConfigurationRecorder`,
//! `PutDeliveryChannel` and `StartConfigurationRecorder` each answer success on their FIRST
//! attempt, with no refusal for a client to retry past.** A gate that keeps refusing a
//! satisfied precondition is indistinguishable from a hung service: a client following the
//! documented order (put the recorder, put the channel, start the recorder) retries for ever.
//!
//! Three crates have to agree for that to hold — IAM holds the role and the identity policies
//! in force on it, Amazon S3 holds the bucket and the policy in force on it, and Config reads
//! both before it takes the channel — so the suite lives here: a crate-local test can only
//! ever see one side of those calls.
//!
//! The rows differ along what the rule generalises over: whether the recorder's own role
//! states the delivery grants BESIDE the bucket's policy (the calls Config makes on the
//! bucket are made as its own service principal, so the bucket's policy is the one source of
//! that permission and the role's grants add nothing), whether the request NAMES the
//! recorder and the channel or leaves each to Config's own default, which recording strategy
//! the recording group asks for, and whether the channel configures an `s3KeyPrefix` and a
//! snapshot delivery frequency.
//!
//! Once a row's recorder is recording, the plane behind it is exercised the way a client
//! reaches it: `PutResourceConfig` records a configuration document for a resource type the
//! registry holds and is refused `ValidationException` for one it holds none for,
//! `GetResourceConfigHistory` reports the recorded document back verbatim and refuses a
//! resource key nothing ever ingested with `ResourceNotDiscoveredException`, and
//! `BatchGetResourceConfig` answers the keys it holds and an empty list for the ones it does
//! not.
//!
//! Nothing here asserts on elapsed time: "converges" is asserted as the FIRST answer to each
//! call being a success, and the recorder's own clock-owed status move is waited for by
//! polling the condition under a generous deadline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element reader this crate writes ONCE
// (`tests/all/query_wire.rs`): a suite that writes its own drifts from the others.
use super::query_wire::{element, form_encode};

/// The Region every request here is signed for, and so the Region each resource belongs to.
const REGION: &str = "us-east-1";

/// Config's `X-Amz-Target` prefix, from its model's service shape (`StarlingDoveService`),
/// and the signing name its credential scope carries (`sigv4Name: config`).
const CONFIG_TARGET_PREFIX: &str = "StarlingDoveService";
/// See [`CONFIG_TARGET_PREFIX`].
const CONFIG_SIGNING_NAME: &str = "config";

/// IAM's awsQuery API version, from that crate's `OPERATIONS.md` wire-protocol section.
const IAM_VERSION: &str = "2010-05-08";

/// The service principal Config assumes the configuration recorder's role as, which the
/// role's trust policy therefore has to admit.
const CONFIG_PRINCIPAL: &str = "config.amazonaws.com";

/// The name Config assigns a configuration recorder and a delivery channel created without
/// one ("By default, Config assigns the name \"default\" when creating the delivery
/// channel", `DeliveryChannel$name`).
const DEFAULT_NAME: &str = "default";

/// The `RecorderStatus` a recording event that has been processed settles at
/// (`smithy_query(query="shape", shape="RecorderStatus")`).
const RECORDER_STATUS_SUCCESS: &str = "Success";

/// A resource type the CloudFormation registry publishes, so `PutResourceConfig` records a
/// configuration for it (`smithy_query(query="shape", shape="ResourceType")`).
const S3_BUCKET_TYPE: &str = "AWS::S3::Bucket";

/// Generously long for the recorder's clock-owed status move to come due; the suite polls
/// for the condition and stops as soon as it holds, so this bounds only a failure.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll waits between reads: short enough that the wait is the condition's
/// own, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What a row's account states about the delivery beside the bucket's own policy. Config
/// makes those S3 calls as its own service principal, which holds no identity-based
/// policies, so the policy in force on the bucket states the permission in every row and the
/// recorder's role is the dimension that varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grants {
    /// The bucket's own policy alone — the policy AWS documents for a delivery channel's
    /// bucket — with the role stating nothing of S3.
    BucketPolicy,
    /// The same policy, with the recorder's role stating the delivery grants as well.
    Both,
}

/// One row: how the account is provisioned and what the two `Put…` calls ask for.
struct Row {
    case: &'static str,
    grants: Grants,
    /// The recorder's name, or `None` to leave it to Config's own default.
    recorder_name: Option<&'static str>,
    /// The `recordingGroup` the recorder is put with.
    recording_group: Value,
    /// The channel's name, or `None` to leave it to Config's own default.
    channel_name: Option<&'static str>,
    /// The `s3KeyPrefix` the channel configures, or `None` for a channel with none.
    key_prefix: Option<&'static str>,
    /// The `deliveryFrequency` the channel's snapshot properties configure, or `None`.
    delivery_frequency: Option<&'static str>,
}

/// Every row of the provisioning table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the documented bucket policy, a role stating nothing of S3",
            grants: Grants::BucketPolicy,
            recorder_name: None,
            recording_group: json!({ "allSupported": true }),
            channel_name: None,
            key_prefix: None,
            delivery_frequency: None,
        },
        Row {
            case: "the documented bucket policy, the grants stated on the role as well",
            grants: Grants::Both,
            recorder_name: Some("sim2real-recorder"),
            recording_group: json!({
                "allSupported": false,
                "resourceTypes": [S3_BUCKET_TYPE, "AWS::EC2::Volume"],
                "recordingStrategy": { "useOnly": "INCLUSION_BY_RESOURCE_TYPES" },
            }),
            channel_name: Some("sim2real-channel"),
            key_prefix: Some("config/"),
            delivery_frequency: Some("TwentyFour_Hours"),
        },
        Row {
            case: "both sources stating the grants, recording with an exclusion",
            grants: Grants::Both,
            recorder_name: Some("sim2real-both"),
            recording_group: json!({
                "allSupported": false,
                "exclusionByResourceTypes": { "resourceTypes": ["AWS::EC2::Volume"] },
                "recordingStrategy": { "useOnly": "EXCLUSION_BY_RESOURCE_TYPES" },
            }),
            channel_name: None,
            key_prefix: Some("team/config"),
            delivery_frequency: Some("Six_Hours"),
        },
    ]
}

/// IAM, Amazon S3 and Config over ONE shared engine, exactly as `core/router/src/lib.rs`
/// mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is not the subject here, so signatures are not verified; the credential scope
    // is still carried, because it is what names the Region and routes the request.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_configservice_starlingdoveservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`],
/// which is what routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call to IAM, every component percent-encoded so a policy document travels
/// verbatim.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version={IAM_VERSION}");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One S3 call on the path-style address.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization("s3"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Config operation over its awsJson1_1 wire.
async fn config(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{CONFIG_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{CONFIG_TARGET_PREFIX}.{operation}"),
        )
        .header("authorization", authorization(CONFIG_SIGNING_NAME))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code of a Config refusal: awsJson1_1 carries it as the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` of a Config refusal.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The trust policy that lets Config assume this role: one `Allow` of `sts:AssumeRole` to
/// the Config service principal.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": CONFIG_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The identity policy a client puts on the recorder's role so the delivery calls Config
/// makes under it are allowed by the role itself: the bucket ACL read and the object write
/// AWS documents for a Config delivery role.
fn role_delivery_policy(bucket: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["s3:PutObject", "s3:PutObjectAcl"],
                "Resource": format!("arn:aws:s3:::{bucket}/*"),
            },
            {
                "Effect": "Allow",
                "Action": ["s3:GetBucketAcl", "s3:ListBucket"],
                "Resource": format!("arn:aws:s3:::{bucket}"),
            },
        ],
    })
    .to_string()
}

/// The bucket policy AWS documents for a Config delivery channel's bucket: one `Allow` for
/// the Config service principal per S3 call a delivery performs, each conditioned on the
/// account the delivery is made for.
fn delivery_bucket_policy(bucket: &str) -> String {
    let account = engine::emulator_account_id();
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AWSConfigBucketPermissionsCheck",
                "Effect": "Allow",
                "Principal": { "Service": CONFIG_PRINCIPAL },
                "Action": ["s3:GetBucketAcl", "s3:ListBucket"],
                "Resource": format!("arn:aws:s3:::{bucket}"),
                "Condition": { "StringEquals": { "AWS:SourceAccount": account } },
            },
            {
                "Sid": "AWSConfigBucketDelivery",
                "Effect": "Allow",
                "Principal": { "Service": CONFIG_PRINCIPAL },
                "Action": "s3:PutObject",
                "Resource": format!("arn:aws:s3:::{bucket}/*"),
                "Condition": {
                    "StringEquals": {
                        "s3:x-amz-acl": "bucket-owner-full-control",
                        "AWS:SourceAccount": account,
                    },
                },
            },
        ],
    })
    .to_string()
}

/// Hold the role the recorder runs as, through IAM's own `CreateRole` — and, where the row
/// asks for it, the identity policy in force on it through `PutRolePolicy`. Answers the ARN
/// IAM minted for it.
async fn recorder_role(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    bucket: &str,
    grants: Grants,
) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreateRole({role_name:?}) must succeed, got {status} with \
         {created}"
    );
    if grants != Grants::BucketPolicy {
        let (status, answered) = iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", role_name),
                ("PolicyName", "config-delivery"),
                ("PolicyDocument", &role_delivery_policy(bucket)),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup PutRolePolicy({role_name:?}) must succeed, got {status} with \
             {answered}"
        );
    }
    element(&created, "Arn")
}

/// Hold the delivery bucket, through S3's own `CreateBucket`, and the policy AWS documents
/// for a delivery channel's bucket in force on it through `PutBucketPolicy` — the one source
/// of the permission the delivery needs, so every row states it.
async fn delivery_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let (status, created) = s3(router, "PUT", &format!("/{bucket}"), "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreateBucket({bucket:?}) must succeed, got {status} with {created}"
    );
    let (status, answered) = s3(
        router,
        "PUT",
        &format!("/{bucket}?policy"),
        &delivery_bucket_policy(bucket),
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: setup PutBucketPolicy({bucket:?}) must succeed, got {status} with \
         {answered}"
    );
}

/// The recorder status Config reports under `name`, waited for until its clock-owed status
/// move has come due. Polls the CONDITION rather than sleeping a fixed time, and fails only
/// when the condition still does not hold at [`SETTLE_DEADLINE`].
async fn settled_recorder_status(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = config(
            router,
            "DescribeConfigurationRecorderStatus",
            &json!({ "ConfigurationRecorderNames": [name] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the recorder status must be served, got {status} with {body}"
        );
        let reported = body["ConfigurationRecordersStatus"][0].clone();
        if reported["lastStatus"].as_str() == Some(RECORDER_STATUS_SUCCESS) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the started recorder's status must settle at \
             {RECORDER_STATUS_SUCCESS:?}, last read {reported}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `PutResourceConfig` body recording `configuration` for one resource.
fn resource_config(resource_type: &str, resource_id: &str, configuration: &str) -> Value {
    json!({
        "ResourceType": resource_type,
        "SchemaVersionId": "1-0-0",
        "ResourceId": resource_id,
        "Configuration": configuration,
    })
}

#[tokio::test]
async fn the_documented_recorder_provisioning_sequence_is_served_on_its_first_attempt() {
    for row in rows() {
        let Row {
            case,
            grants,
            recorder_name,
            recording_group,
            channel_name,
            key_prefix,
            delivery_frequency,
        } = &row;
        // One account per row: an account holds ONE customer managed configuration recorder
        // and ONE delivery channel per Region, so each row is provisioned from nothing.
        let router = build_router();
        let bucket = "sim2real-config-delivery";
        let role_name = "sim2real-config-recorder-role";
        let role_arn = recorder_role(&router, case, role_name, bucket, *grants).await;
        delivery_bucket(&router, case, bucket).await;

        // 1. The recorder, with the role IAM holds: served on the FIRST attempt — nothing
        //    about the role is still to be provisioned, so there is no refusal to retry past.
        let mut recorder = json!({ "roleARN": &role_arn, "recordingGroup": recording_group });
        if let Some(named) = recorder_name {
            recorder["name"] = json!(named);
        }
        let (status, body) = config(
            &router,
            "PutConfigurationRecorder",
            &json!({ "ConfigurationRecorder": recorder }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutConfigurationRecorder must be served on the first attempt, got \
             {status} with {body}"
        );
        let recorder_name = recorder_name.unwrap_or(DEFAULT_NAME);

        // 2. The delivery channel into that bucket: served on the FIRST attempt, the
        //    bucket's own policy stating what the delivery needs.
        let mut channel = json!({ "s3BucketName": bucket });
        if let Some(named) = channel_name {
            channel["name"] = json!(named);
        }
        if let Some(prefix) = key_prefix {
            channel["s3KeyPrefix"] = json!(prefix);
        }
        if let Some(frequency) = delivery_frequency {
            channel["configSnapshotDeliveryProperties"] =
                json!({ "deliveryFrequency": frequency });
        }
        let (status, body) = config(
            &router,
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": channel }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutDeliveryChannel must be served on the first attempt, got {status} \
             with {body}"
        );
        let channel_name = channel_name.unwrap_or(DEFAULT_NAME);

        // 3. The start: served on the FIRST attempt, there being a channel to deliver into.
        let (status, body) = config(
            &router,
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": recorder_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartConfigurationRecorder must be served on the first attempt, got \
             {status} with {body}"
        );

        // What the account holds afterwards, read back through the operations that report it:
        // the recorder under its role, the channel under its bucket and prefix, and a status
        // that says the recorder is recording.
        let (status, described) = config(
            &router,
            "DescribeConfigurationRecorders",
            &json!({ "ConfigurationRecorderNames": [recorder_name] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the recorder must be served, got {status} with {described}"
        );
        let held = &described["ConfigurationRecorders"][0];
        assert_eq!(
            (held["name"].as_str(), held["roleARN"].as_str()),
            (Some(recorder_name), Some(role_arn.as_str())),
            "case {case:?}: expected the recorder stored under {recorder_name:?} with the role \
             {role_arn:?}, got {described}"
        );

        let (status, channels) = config(
            &router,
            "DescribeDeliveryChannels",
            &json!({ "DeliveryChannelNames": [channel_name] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the channel must be served, got {status} with {channels}"
        );
        let stored = &channels["DeliveryChannels"][0];
        assert_eq!(
            (
                stored["name"].as_str(),
                stored["s3BucketName"].as_str(),
                stored["s3KeyPrefix"].as_str(),
                stored["configSnapshotDeliveryProperties"]["deliveryFrequency"].as_str(),
            ),
            (
                Some(channel_name),
                Some(bucket),
                *key_prefix,
                *delivery_frequency,
            ),
            "case {case:?}: expected the channel stored as it was put, got {channels}"
        );

        let settled = settled_recorder_status(&router, case, recorder_name).await;
        assert_eq!(
            settled["recording"], json!(true),
            "case {case:?}: a started recorder is recording, got {settled}"
        );
        assert!(
            settled["lastStartTime"].is_number(),
            "case {case:?}: a started recorder carries the instant it was started, got {settled}"
        );
    }
}

#[tokio::test]
async fn the_resource_config_plane_behind_a_provisioned_recorder_answers() {
    let router = build_router();
    let case = "a recorder provisioned the documented way";
    let bucket = "sim2real-plane-delivery";
    let role_arn = recorder_role(
        &router,
        case,
        "sim2real-plane-role",
        bucket,
        Grants::BucketPolicy,
    )
    .await;
    delivery_bucket(&router, case, bucket).await;
    for (operation, body) in [
        (
            "PutConfigurationRecorder",
            json!({
                "ConfigurationRecorder": {
                    "roleARN": role_arn,
                    // The recording group is narrowed to a type nothing here holds, because
                    // the subject of this test is the plane a CALLER writes through: a
                    // recorder whose group covered the type below would reconcile that type
                    // against what the owning service holds on its own passes, and record
                    // the deletion of a resource only `PutResourceConfig` ever held — a
                    // second configuration item, owed by the clock rather than by anything
                    // this test asked for.
                    "recordingGroup": {
                        "allSupported": false,
                        "resourceTypes": ["AWS::EC2::Volume"],
                        "recordingStrategy": { "useOnly": "INCLUSION_BY_RESOURCE_TYPES" },
                    },
                },
            }),
        ),
        (
            "PutDeliveryChannel",
            json!({ "DeliveryChannel": { "s3BucketName": bucket } }),
        ),
        (
            "StartConfigurationRecorder",
            json!({ "ConfigurationRecorderName": DEFAULT_NAME }),
        ),
    ] {
        let (status, answered) = config(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must be served on the first attempt, got {status} with \
             {answered}"
        );
    }

    // The configuration documents a row records, differing along what the rule generalises
    // over: an ASCII document, one carrying characters outside the Basic Latin block, and a
    // nested one.
    let documents: [(&str, &str); 3] = [
        ("an ASCII document", r#"{"greeting":"hello"}"#),
        (
            "a document carrying non-Latin characters",
            r#"{"greeting":"héllo 日本語"}"#,
        ),
        (
            "a nested document",
            r#"{"tags":{"team":"platform"},"replicas":[1,2,3]}"#,
        ),
    ];
    for (index, (document_case, configuration)) in documents.into_iter().enumerate() {
        let resource_id = format!("sim2real-recorded-{index}");
        let (status, answered) = config(
            &router,
            "PutResourceConfig",
            &resource_config(S3_BUCKET_TYPE, &resource_id, configuration),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {document_case:?}: recording a registered resource type must be served, got \
             {status} with {answered}"
        );

        // Read back through the operation that reports it: the document is held verbatim.
        let (status, history) = config(
            &router,
            "GetResourceConfigHistory",
            &json!({ "resourceType": S3_BUCKET_TYPE, "resourceId": &resource_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {document_case:?}: the history of a recorded resource must be served, got \
             {status} with {history}"
        );
        let items = history["configurationItems"]
            .as_array()
            .unwrap_or_else(|| panic!("case {document_case:?}: expected a list, got {history}"));
        assert_eq!(
            items.len(),
            1,
            "case {document_case:?}: one recorded state, one configuration item, got {history}"
        );
        assert_eq!(
            items[0]["configuration"].as_str(),
            Some(configuration),
            "case {document_case:?}: expected the recorded document back verbatim, got {history}"
        );

        // And through the batch read, which answers the key it holds.
        let (status, batch) = config(
            &router,
            "BatchGetResourceConfig",
            &json!({
                "resourceKeys": [{ "resourceType": S3_BUCKET_TYPE, "resourceId": &resource_id }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {document_case:?}: the batch read must be served, got {status} with {batch}"
        );
        assert_eq!(
            batch["baseConfigurationItems"][0]["resourceId"].as_str(),
            Some(resource_id.as_str()),
            "case {document_case:?}: the batch read answers the key it holds, got {batch}"
        );
    }

    // A resource type the registry holds no registration for names nothing Config can
    // record: refused `ValidationException` naming the type the request sent. The rows
    // differ along the organization and the depth of the name, none of them a published
    // type.
    for unregistered in [
        "Sim2RealTest::Widget::Resource",
        "Acme::Storage::Vault",
        "AWS::NotAService::NotAType",
    ] {
        let (status, refused) = config(
            &router,
            "PutResourceConfig",
            &resource_config(unregistered, "sim2real-unregistered", r#"{"a":1}"#),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused), error_message(&refused)),
            (
                400,
                "ValidationException".to_string(),
                format!(
                    "Cannot find registered Resource Type with name {unregistered}. Please \
                     register the Resource Type with Cloudformation and try again."
                ),
            ),
            "case {unregistered:?}: an unregistered resource type is refused, got {status} with \
             {refused}"
        );
    }

    // A resource key nothing ever ingested: the history is refused rather than answered
    // empty, naming the id and the type the request sent, and the batch read reports no
    // base configuration item for it. The rows differ along the resource type and the form
    // of the id.
    for (resource_type, resource_id) in [
        (S3_BUCKET_TYPE, "sim2real-never-ingested"),
        ("AWS::EC2::Volume", "vol-00000000000000000"),
        ("AWS::IAM::Role", "AROA0000000000000000"),
    ] {
        let (status, refused) = config(
            &router,
            "GetResourceConfigHistory",
            &json!({ "resourceType": resource_type, "resourceId": resource_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused), error_message(&refused)),
            (
                400,
                "ResourceNotDiscoveredException".to_string(),
                format!(
                    "Resource {resource_id} of resourceType:{resource_type} is unknown or has \
                     not been discovered"
                ),
            ),
            "case {resource_id:?}: a key nothing ingested is refused, got {status} with {refused}"
        );

        let (status, batch) = config(
            &router,
            "BatchGetResourceConfig",
            &json!({
                "resourceKeys": [{ "resourceType": resource_type, "resourceId": resource_id }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {resource_id:?}: the batch read must be served, got {status} with {batch}"
        );
        assert_eq!(
            batch["baseConfigurationItems"], json!([]),
            "case {resource_id:?}: a key nothing ingested has no base configuration item, got \
             {batch}"
        );
    }
}
