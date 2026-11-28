// SPDX-License-Identifier: CC0-1.0

//! What AWS Config answers about an Amazon SQS queue on the FIRST read that asks about it,
//! while a customer managed configuration recorder is RECORDING.
//!
//! The rule under test: a recorder that is recording owes a configuration item for the state
//! every resource of a type its recording group covers is in, so the configuration item a
//! read asks about is the one the resource's configuration owed at the instant it was asked
//! for — the FIRST `GetResourceConfigHistory`, `BatchGetResourceConfig` and
//! `SelectResourceConfig` call about that resource answers it, with no second attempt, and
//! the recorder's own status leaves `Pending` for `Success` once it has processed a recording
//! event. A resource key the account has recorded nothing for is the one thing the history
//! read refuses, which only means something in an account whose other keys ARE answered.
//!
//! Why the first read is the subject: a client of Config polls this read, and the answer a
//! single attempt gets is what decides whether such a client makes progress at all. The
//! suite beside this one
//! (`config_recorded_queue_is_the_control_for_an_unrecorded_refusal`) asserts the same
//! contract for a client that keeps looking; this one asserts that looking once is enough,
//! and reads every surface exactly once per resource.
//!
//! Four crates have to agree for that to hold, which is why the suite lives here: IAM holds
//! the role the recorder runs as, Amazon S3 holds the delivery bucket and the policy in force
//! on it, Amazon SQS owns the queues and is the API the recorder reads them through, and
//! Config records, reads and refuses. Every service is mounted on ONE engine, exactly as
//! `core/router/src/lib.rs` mounts them.
//!
//! The rows differ along what the rule generalises over: WHEN the resource came to be
//! relative to the start of recording (a queue the account already held, one created while
//! recording, one whose configuration CHANGED while recording) and how it is CONFIGURED (a
//! visibility timeout, a FIFO queue, a queue carrying no attribute at all).
//!
//! Nothing here asserts on elapsed time or sleeps for a span. The one wait is for the
//! recorder's own status, which is polled on its CONDITION under a generous deadline, and
//! nothing here calls `PutResourceConfig`: every configuration item asserted on is one the
//! recorder observed by itself.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The credential-scope header builder and the awsQuery helpers this crate already owns: a
// suite that writes its own drifts from the others in what it signs and how it reads XML.
use super::cloudformation_provisions_s3vectors_resources::authorization;
use super::query_wire::{element, form_encode};

/// The Region every request below is signed for, and so the Region every resource belongs to.
const REGION: &str = "us-east-1";

/// Config's `X-Amz-Target` prefix (its model's service shape, `StarlingDoveService`) and the
/// signing name its credential scope carries (`sigv4Name: config`).
const CONFIG_TARGET_PREFIX: &str = "StarlingDoveService";
/// See [`CONFIG_TARGET_PREFIX`].
const CONFIG_SIGNING_NAME: &str = "config";

/// Amazon SQS' `X-Amz-Target` prefix and signing name.
const SQS_TARGET_PREFIX: &str = "AmazonSQS";
/// See [`SQS_TARGET_PREFIX`].
const SQS_SIGNING_NAME: &str = "sqs";

/// IAM's awsQuery API version, from that crate's `OPERATIONS.md` wire-protocol section.
const IAM_VERSION: &str = "2010-05-08";

/// The service principal Config assumes the recorder's role as, which the role's trust policy
/// therefore has to admit and the delivery bucket's policy has to allow.
const CONFIG_PRINCIPAL: &str = "config.amazonaws.com";

/// The resource type Config records an Amazon SQS queue under, a
/// `com.amazonaws.configservice#ResourceType` value
/// (`smithy_query(query="shape", shape="ResourceType")`).
const QUEUE_TYPE: &str = "AWS::SQS::Queue";

/// The attribute Amazon SQS reports a queue's own ARN under
/// (`com.amazonaws.sqs#QueueAttributeName`), which is what the queue's configuration item
/// carries as its `arn`.
const QUEUE_ARN_ATTRIBUTE: &str = "QueueArn";

/// The attribute whose value a row CHANGES, and the one a row is created with
/// (`com.amazonaws.sqs#QueueAttributeName`).
const VISIBILITY_TIMEOUT_ATTRIBUTE: &str = "VisibilityTimeout";

/// The names this suite provisions the account under: one recorder, one delivery bucket and
/// one role.
const RECORDER: &str = "sim2real-first-read-recorder";
/// See [`RECORDER`].
const DELIVERY_BUCKET: &str = "sim2real-first-read-delivery";
/// See [`RECORDER`].
const RECORDER_ROLE: &str = "sim2real-first-read-role";

/// The error a configuration-item read answers for a well-formed resource key the account has
/// recorded no configuration item for, and the status it carries
/// (`smithy_query(query="operation", operation="GetResourceConfigHistory")`).
const NOT_DISCOVERED: &str = "ResourceNotDiscoveredException";
/// See [`NOT_DISCOVERED`].
const CLIENT_ERROR: u16 = 400;

/// The `RecorderStatus` value a recording event that has been PROCESSED reports, and the one
/// an event still being processed reports — the two members of that enum a started recorder
/// moves between (`smithy_query(query="shape", shape="RecorderStatus")`: `Pending`, `Success`,
/// `Failure`, `NotApplicable`).
const STATUS_SUCCESS: &str = "Success";
/// See [`STATUS_SUCCESS`].
const STATUS_PENDING: &str = "Pending";

/// How long the recorder-status poll waits for the recording event the clock owes to be
/// processed. The event is processed a fraction of a second after the start, so this is many
/// passes' worth of slack and not a span any assertion depends on: the wait ends on its
/// CONDITION, and the budget is there only so a wedged emulator fails instead of hanging.
const PROCESSED_DEADLINE: Duration = Duration::from_secs(180);

/// How long the poll waits between two reads: short enough that the wait is the condition's
/// own, long enough not to spin.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// This emulator's account, which is the account every resource below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// IAM, Amazon S3, Amazon SQS and Config over ONE shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so signatures are not verified; the credential
        // scope is still carried, because it is what names the Region and routes the call.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_configservice_starlingdoveservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One awsQuery call to IAM, every component percent-encoded so a policy document travels
    /// verbatim.
    async fn iam_ok(&self, case: &str, action: &str, members: &[(&str, &str)]) -> String {
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
            .header("authorization", authorization("iam", REGION))
            .body(Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from IAM {action}, got {status} with {body}"
        );
        body
    }

    /// One S3 call on the path-style address, which must be served.
    async fn s3_ok(&self, case: &str, path: &str, body: &str) {
        let request = http::Request::builder()
            .method("PUT")
            .uri(path)
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("content-type", "application/xml")
            .header("authorization", authorization("s3", REGION))
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert!(
            (200..300).contains(&status),
            "case {case:?}: expected S3 PUT {path} to be served, got {status} with {answered}"
        );
    }

    /// One `awsJson` RPC call, answering the HTTP status and the parsed body.
    async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", authorization(signing_name, REGION))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One Config call (`awsJson1_1`).
    async fn config(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.rpc(
            CONFIG_SIGNING_NAME,
            CONFIG_TARGET_PREFIX,
            "1.1",
            operation,
            body,
        )
        .await
    }

    /// A Config call that must answer 200.
    async fn config_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.config(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
        );
        parsed
    }

    /// An Amazon SQS call (`awsJson1_0`) that must answer 200.
    async fn sqs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .rpc(SQS_SIGNING_NAME, SQS_TARGET_PREFIX, "1.0", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
        );
        parsed
    }

    /// Provision the account the way AWS documents it and START the recorder: the role Config
    /// assumes, the delivery bucket and the policy that admits the delivery, the recorder
    /// recording every supported resource type, and its delivery channel.
    async fn given_recording(&self, case: &str) {
        let created = self
            .iam_ok(
                case,
                "CreateRole",
                &[
                    ("RoleName", RECORDER_ROLE),
                    ("AssumeRolePolicyDocument", &trust_policy()),
                ],
            )
            .await;
        let role_arn = element(&created, "Arn");
        self.s3_ok(case, &format!("/{DELIVERY_BUCKET}"), "").await;
        self.s3_ok(
            case,
            &format!("/{DELIVERY_BUCKET}?policy"),
            &delivery_bucket_policy(),
        )
        .await;
        self.config_ok(
            case,
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": RECORDER,
                    "roleARN": role_arn,
                    "recordingGroup": {
                        "allSupported": true,
                        "includeGlobalResourceTypes": false,
                    },
                }
            }),
        )
        .await;
        self.config_ok(
            case,
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": DELIVERY_BUCKET } }),
        )
        .await;
        self.config_ok(
            case,
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
        let status = self.recorder_status(case).await;
        assert_eq!(
            status["recording"].as_bool(),
            Some(true),
            "case {case:?}: expected the started recorder to report that it is recording, got \
             {status}"
        );
    }

    /// Create the queue called `name` with `attributes` through Amazon SQS itself, and answer
    /// its url and the ARN Amazon SQS minted for it.
    async fn queue(&self, case: &str, name: &str, attributes: &[(&str, &str)]) -> (String, String) {
        let mut request = json!({ "QueueName": name });
        if !attributes.is_empty() {
            request["Attributes"] = attributes
                .iter()
                .map(|(attribute, value)| ((*attribute).to_string(), json!(value)))
                .collect();
        }
        let created = self.sqs_ok(case, "CreateQueue", &request).await;
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
            .to_string();
        let read = self
            .sqs_ok(
                case,
                "GetQueueAttributes",
                &json!({ "QueueUrl": &url, "AttributeNames": ["All"] }),
            )
            .await;
        let arn = read["Attributes"][QUEUE_ARN_ATTRIBUTE]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a {QUEUE_ARN_ATTRIBUTE}, got {read}"))
            .to_string();
        (url, arn)
    }

    /// Change one attribute of the queue `url` through Amazon SQS itself.
    async fn set_attribute(&self, case: &str, url: &str, attribute: &str, value: &str) {
        self.sqs_ok(
            case,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { attribute: value } }),
        )
        .await;
    }

    /// The configuration items `GetResourceConfigHistory` reports for one resource key, or
    /// `None` for a key Config refuses as undiscovered — which is how the read answers a key
    /// the account recorded nothing for.
    async fn history(
        &self,
        case: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<Vec<Value>> {
        let (status, body) = self
            .config(
                "GetResourceConfigHistory",
                &json!({ "resourceType": resource_type, "resourceId": resource_id }),
            )
            .await;
        if status == CLIENT_ERROR && error_code(&body) == NOT_DISCOVERED {
            return None;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: the history of {resource_id:?} must be served or refused as \
             undiscovered, got {status} with {body}"
        );
        Some(
            body["configurationItems"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        )
    }

    /// The configuration items the FIRST history read about `resource_id` answers, which the
    /// rule says is already the resource's own history: one attempt, never a second.
    async fn first_read_items(
        &self,
        case: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Vec<Value> {
        let answered = self.history(case, resource_type, resource_id).await;
        let items = answered.clone().unwrap_or_default();
        assert!(
            answered.is_some() && !items.is_empty(),
            "case {case:?}: expected the FIRST {resource_type} history read for \
             {resource_id:?} to answer the item the recording recorder owes it, got \
             {answered:?}"
        );
        items
    }

    /// What `BatchGetResourceConfig` answers for ONE resource key: its base configuration
    /// items and the keys it could not process.
    async fn batch(
        &self,
        case: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> (Vec<Value>, Vec<Value>) {
        let answered = self
            .config_ok(
                case,
                "BatchGetResourceConfig",
                &json!({
                    "resourceKeys": [
                        { "resourceType": resource_type, "resourceId": resource_id },
                    ],
                }),
            )
            .await;
        (
            answered["baseConfigurationItems"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
            answered["unprocessedResourceKeys"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        )
    }

    /// The rows `SelectResourceConfig` answers for one resource key, read through the query a
    /// client asks that question with.
    async fn selected(&self, case: &str, resource_type: &str, resource_id: &str) -> Vec<Value> {
        let answered = self
            .config_ok(
                case,
                "SelectResourceConfig",
                &json!({
                    "Expression": format!(
                        "SELECT resourceId WHERE resourceId = '{resource_id}' AND resourceType \
                         = '{resource_type}'"
                    ),
                }),
            )
            .await;
        answered["Results"].as_array().cloned().unwrap_or_default()
    }

    /// The status Config reports for this account's recorder.
    async fn recorder_status(&self, case: &str) -> Value {
        let answered = self
            .config_ok(
                case,
                "DescribeConfigurationRecorderStatus",
                &json!({ "ConfigurationRecorderNames": [RECORDER] }),
            )
            .await;
        answered["ConfigurationRecordersStatus"][0].clone()
    }

    /// The recorder status once the recording event it is processing HAS been processed.
    ///
    /// Polls the CONDITION rather than sleeping a span, and fails only when the status still
    /// reports no processed event at [`PROCESSED_DEADLINE`], naming what it last reported.
    async fn processed_recorder_status(&self, case: &str) -> Value {
        let deadline = Instant::now() + PROCESSED_DEADLINE;
        loop {
            let reported = self.recorder_status(case).await;
            if reported["lastStatus"].as_str() == Some(STATUS_SUCCESS) {
                return reported;
            }
            assert!(
                reported["lastStatus"].as_str() == Some(STATUS_PENDING),
                "case {case:?}: a started recorder's event is either still being processed \
                 or processed, got {reported}"
            );
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the recording recorder to report a processed \
                 recording event, last read {reported}"
            );
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
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

/// The trust policy that lets Config assume the recorder's role: one `Allow` of
/// `sts:AssumeRole` to the Config service principal.
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

/// The bucket policy AWS documents for a Config delivery channel's bucket: one `Allow` for
/// the Config service principal per S3 call a delivery performs, each conditioned on the
/// account the delivery is made for.
fn delivery_bucket_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AWSConfigBucketPermissionsCheck",
                "Effect": "Allow",
                "Principal": { "Service": CONFIG_PRINCIPAL },
                "Action": ["s3:GetBucketAcl", "s3:ListBucket"],
                "Resource": format!("arn:aws:s3:::{DELIVERY_BUCKET}"),
                "Condition": { "StringEquals": { "AWS:SourceAccount": account() } },
            },
            {
                "Sid": "AWSConfigBucketDelivery",
                "Effect": "Allow",
                "Principal": { "Service": CONFIG_PRINCIPAL },
                "Action": "s3:PutObject",
                "Resource": format!("arn:aws:s3:::{DELIVERY_BUCKET}/*"),
                "Condition": {
                    "StringEquals": {
                        "s3:x-amz-acl": "bucket-owner-full-control",
                        "AWS:SourceAccount": account(),
                    },
                },
            },
        ],
    })
    .to_string()
}

/// How Config words the refusal of a resource key it has recorded nothing for, with the id and
/// the type taken from the request.
fn not_discovered_message(resource_id: &str, resource_type: &str) -> String {
    format!(
        "Resource {resource_id} of resourceType:{resource_type} is unknown or has not been \
         discovered"
    )
}

/// The `configuration` document of one configuration item, parsed out of the JSON string the
/// item carries it as.
fn configuration(case: &str, item: &Value) -> Value {
    let document = item["configuration"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a configuration document, got {item}"));
    serde_json::from_str(document).unwrap_or_else(|error| {
        panic!("case {case:?}: the configuration document must be JSON, got {document:?}: {error}")
    })
}

/// Assert that every read surface answers the resource `url` on its FIRST call: the history
/// read with at least one item whose `arn` is the one Amazon SQS minted, the batch read with
/// exactly one base configuration item carrying that same ARN in its own configuration
/// document, and the query surface with exactly one row.
///
/// Answers the history items, newest first, for a row that goes on to assert what the latest
/// one recorded.
async fn served_on_the_first_read(
    emulator: &Emulator,
    case: &str,
    url: &str,
    arn: &str,
) -> Vec<Value> {
    let items = emulator.first_read_items(case, QUEUE_TYPE, url).await;
    let latest = &items[0];
    assert_eq!(
        (
            latest["resourceType"].as_str(),
            latest["resourceId"].as_str(),
            latest["arn"].as_str(),
        ),
        (Some(QUEUE_TYPE), Some(url), Some(arn)),
        "case {case:?}: expected the latest item to record the queue Amazon SQS minted, got \
         {latest}"
    );

    let (held, unprocessed) = emulator.batch(case, QUEUE_TYPE, url).await;
    assert_eq!(
        (held.len(), unprocessed.len()),
        (1, 0),
        "case {case:?}: expected the FIRST batch read to answer exactly one base \
         configuration item and leave nothing unprocessed, got {held:?} and {unprocessed:?}"
    );
    let document = configuration(case, &held[0]);
    assert_eq!(
        document[QUEUE_ARN_ATTRIBUTE].as_str(),
        Some(arn),
        "case {case:?}: expected the base configuration item's document to carry the queue's \
         own ARN, got {document}"
    );

    let rows = emulator.selected(case, QUEUE_TYPE, url).await;
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: expected the FIRST query read to answer one row for the recorded \
         queue, got {rows:?}"
    );

    items
}

/// One queue of the recorded leg: how it is named and how it is CONFIGURED.
struct Recorded {
    case: &'static str,
    name: &'static str,
    attributes: &'static [(&'static str, &'static str)],
}

/// The queues created while the recorder is recording. The rows differ along the queue's own
/// configuration — which attributes it carries, a FIFO queue among them, and a queue
/// configured with nothing at all — because what a configuration item records is the
/// resource's configuration, whatever that configuration is.
const CREATED_WHILE_RECORDING: [Recorded; 3] = [
    Recorded {
        case: "a queue whose visibility timeout is configured",
        name: "sim2real-first-read-visibility",
        attributes: &[(VISIBILITY_TIMEOUT_ATTRIBUTE, "137")],
    },
    Recorded {
        case: "a FIFO queue deduplicating on content",
        name: "sim2real-first-read-ordered.fifo",
        attributes: &[("FifoQueue", "true"), ("ContentBasedDeduplication", "true")],
    },
    Recorded {
        case: "a queue created with no attributes at all",
        name: "sim2real-first-read-plain",
        attributes: &[],
    },
];

/// The queues the account ALREADY HELD when recording started, which the start's own
/// observation of what the account holds is what records. The rows differ along the same
/// configuration axis as the ones created later.
const HELD_BEFORE_RECORDING: [Recorded; 2] = [
    Recorded {
        case: "a queue the account held before recording started",
        name: "sim2real-first-read-pre-existing",
        attributes: &[(VISIBILITY_TIMEOUT_ATTRIBUTE, "97")],
    },
    Recorded {
        case: "a FIFO queue the account held before recording started",
        name: "sim2real-first-read-pre-existing.fifo",
        attributes: &[("FifoQueue", "true")],
    },
];

#[tokio::test]
async fn a_queue_created_while_recording_is_answered_by_the_first_read_of_every_surface() {
    let emulator = Emulator::new();
    emulator.given_recording("the recorded leg").await;

    for row in &CREATED_WHILE_RECORDING {
        let case = row.case;
        let (url, arn) = emulator.queue(case, row.name, row.attributes).await;
        let items = served_on_the_first_read(&emulator, case, &url, &arn).await;

        // The item records the queue's own configuration, which is what the attributes the
        // create asked for left it at.
        let document = configuration(case, &items[0]);
        for (attribute, value) in row.attributes {
            assert_eq!(
                document[*attribute].as_str(),
                Some(*value),
                "case {case:?}: expected the item to record {attribute}={value:?} as Amazon \
                 SQS reports it, got {document}"
            );
        }
    }
}

#[tokio::test]
async fn a_queue_the_account_already_held_is_answered_by_the_first_read() {
    let emulator = Emulator::new();

    // Created BEFORE anything of Config exists, so nothing but the recorder's own
    // observation of what the account holds can have recorded them.
    let mut held = Vec::new();
    for row in &HELD_BEFORE_RECORDING {
        let (url, arn) = emulator.queue(row.case, row.name, row.attributes).await;
        held.push((row, url, arn));
    }

    emulator.given_recording("the pre-existing leg").await;

    for (row, url, arn) in &held {
        served_on_the_first_read(&emulator, row.case, url, arn).await;
    }
}

#[tokio::test]
async fn a_configuration_change_is_answered_by_the_first_read_after_it() {
    let case = "a queue whose visibility timeout changed while recording";
    let emulator = Emulator::new();
    emulator.given_recording(case).await;

    let (url, arn) = emulator
        .queue(case, "sim2real-first-read-changed", &[(VISIBILITY_TIMEOUT_ATTRIBUTE, "31")])
        .await;
    let created = served_on_the_first_read(&emulator, case, &url, &arn).await;
    assert_eq!(
        configuration(case, &created[0])[VISIBILITY_TIMEOUT_ATTRIBUTE].as_str(),
        Some("31"),
        "case {case:?}: expected the first item to record the configuration the queue was \
         created with, got {:?}",
        created[0]
    );

    // The CHANGED state is a state of the resource the recorder owes an item for, so the
    // first read after the change answers it.
    emulator
        .set_attribute(case, &url, VISIBILITY_TIMEOUT_ATTRIBUTE, "287")
        .await;
    let changed = served_on_the_first_read(&emulator, case, &url, &arn).await;
    assert_eq!(
        configuration(case, &changed[0])[VISIBILITY_TIMEOUT_ATTRIBUTE].as_str(),
        Some("287"),
        "case {case:?}: expected the first read after the change to answer the changed \
         configuration, got {:?}",
        changed[0]
    );
    assert!(
        changed.len() > created.len(),
        "case {case:?}: expected the change to add a state to the queue's history, got \
         {} items after {} before",
        changed.len(),
        created.len()
    );
}

#[tokio::test]
async fn the_recorder_that_recorded_an_item_reports_its_processed_event() {
    let case = "a recorder that has recorded an item";
    let emulator = Emulator::new();
    emulator.given_recording(case).await;

    let (url, arn) = emulator
        .queue(case, "sim2real-first-read-status", &[(VISIBILITY_TIMEOUT_ATTRIBUTE, "77")])
        .await;
    served_on_the_first_read(&emulator, case, &url, &arn).await;

    // The recording event the start made pending is processed by the recorder's own pass, so
    // the status it reports leaves `Pending` for `Success` with the instant of that event.
    let reported = emulator.processed_recorder_status(case).await;
    assert_eq!(
        (
            reported["recording"].as_bool(),
            reported["lastStatus"].as_str(),
            reported["lastErrorCode"].as_str(),
            reported["lastErrorMessage"].as_str(),
        ),
        (Some(true), Some(STATUS_SUCCESS), None, None),
        "case {case:?}: expected a recording recorder that has recorded to report a \
         processed event and no error, got {reported}"
    );
    assert!(
        reported["lastStatusChangeTime"].is_number()
            && reported["lastStartTime"].is_number()
            && reported["lastStatusChangeTime"].as_f64() >= reported["lastStartTime"].as_f64(),
        "case {case:?}: expected the processed event to be stamped no earlier than the start \
         that made it pending, got {reported}"
    );
}

#[tokio::test]
async fn a_key_the_account_recorded_nothing_for_is_refused_beside_a_key_it_answers() {
    let case = "a queue url of the same shape as a recorded one";
    let emulator = Emulator::new();
    emulator.given_recording(case).await;

    let (url, arn) = emulator
        .queue(case, "sim2real-first-read-control", &[(VISIBILITY_TIMEOUT_ATTRIBUTE, "137")])
        .await;
    served_on_the_first_read(&emulator, case, &url, &arn).await;

    // The same account, asked about a key nothing was ever created under: the history read
    // REFUSES it, naming the id and the type the request sent, while the other two surfaces
    // answer the same absence by OMISSION.
    let absent = match url.rsplit_once('/') {
        Some((endpoint, _)) => format!("{endpoint}/sim2real-first-read-never-created"),
        None => "sim2real-first-read-never-created".to_string(),
    };
    let (status, refused) = emulator
        .config(
            "GetResourceConfigHistory",
            &json!({ "resourceType": QUEUE_TYPE, "resourceId": &absent }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused), error_message(&refused)),
        (
            CLIENT_ERROR,
            NOT_DISCOVERED.to_string(),
            not_discovered_message(&absent, QUEUE_TYPE),
        ),
        "case {case:?}: expected the history of an unrecorded key to be refused, got {status} \
         with {refused}"
    );
    let (held, unprocessed) = emulator.batch(case, QUEUE_TYPE, &absent).await;
    assert_eq!(
        (held.len(), unprocessed.len()),
        (0, 0),
        "case {case:?}: expected the batch read to omit an unrecorded key from both of its \
         lists, got {held:?} and {unprocessed:?}"
    );
    let rows = emulator.selected(case, QUEUE_TYPE, &absent).await;
    assert!(
        rows.is_empty(),
        "case {case:?}: expected the query to answer no row for an unrecorded key, got {rows:?}"
    );

    // And the control is still answered afterwards, so the refusal left it where it was.
    served_on_the_first_read(&emulator, case, &url, &arn).await;
}
