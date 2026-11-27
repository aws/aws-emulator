// SPDX-License-Identifier: CC0-1.0

//! WHEN `PutSubscriptionFilter` configures a filter against a STREAM destination, the service
//! that owns that stream decides whether the configuration is accepted at all.
//!
//! RULE: **a put VALIDATES the destination it configures — the service that owns the stream is
//! asked about it, and a stream that service reports it does not hold is a `destinationArn`
//! that could never deliver anything, so the put is refused `InvalidParameterException` with the
//! sentence this service states for a destination its test message cannot reach, and NO filter
//! is stored.** A stream that service does hold is configured: 200, and the group reports the
//! filter. The verdict is read from CURRENT state at every put, so a destination whose stream is
//! deleted after the filter was configured refuses the next identical put, and one created again
//! under the same name is configured again.
//!
//! This suite needs CloudWatch Logs, Kinesis and Firehose on one engine — CloudWatch Logs reaches
//! a destination through the in-process callback, so a single-service router has nothing to ask —
//! which is why it lives here rather than in the CloudWatch Logs crate.
//!
//! The rows differ along what the rule generalises over: the VENDOR that owns the destination
//! (a Kinesis data stream and a Firehose delivery stream), how the destination is named (the
//! stream's own ARN, and a `PutDestination` logical destination that resolves to it), and how the
//! stream came to be absent (never created, and created and then deleted).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{STREAM_ACTIVE, STREAM_MODE_ON_DEMAND};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for, and the one every ARN below names.
const REGION: &str = "us-west-2";

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The class a put refuses a destination it cannot deliver to with.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The whole sentence that refusal states. Fixed text for the class: it names neither the
/// destination, nor its vendor, nor anything else from the request. Stated here independently of
/// the crate under test.
const UNDELIVERABLE: &str =
    "Could not deliver test message to specified destination. Check if the destination is valid.";

/// The code Kinesis reports a stream it no longer holds with.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The role a put names for a streaming destination. IAM holds no such role here, which
/// establishes nothing about it — so no row is refused for the role.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CWLtoStream";

/// How long a wait for a stream to reach a status of its own is given. Generous on purpose: what
/// is under test is the status the stream settles in, never how fast the host gets there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How often such a wait looks again.
const LOOK_AGAIN: Duration = Duration::from_millis(20);

/// This emulator: the three services the rule spans, on ONE shared engine, with request
/// authentication off — each call carries a credential scope (which is what names the Region)
/// without a computed signature.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One `awsJson1_1` operation of the service `signing_name` serves.
    async fn json(
        &self,
        signing_name: &str,
        prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        let parsed = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One CloudWatch Logs operation.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.json("logs", "Logs_20140328", operation, body).await
    }

    /// One CloudWatch Logs operation that must succeed.
    async fn logs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: logs {operation} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// One Kinesis operation, answering its status and parsed body.
    async fn kinesis(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.json("kinesis", "Kinesis_20131202", operation, body)
            .await
    }

    /// One Kinesis operation that must succeed.
    async fn kinesis_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.kinesis(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: kinesis {operation} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// Delete the Kinesis data stream `name` and wait until Kinesis reports it holds no such
    /// stream — a delete leaves the stream `DELETING` until the clock has finished it, so what
    /// a case about an ABSENT stream waits for is the absence itself. The wait looks again until
    /// Kinesis says so or a generous deadline passes; it bounds nothing by how long the deletion
    /// took.
    async fn deleted_stream(&self, case: &str, name: &str) {
        self.kinesis_ok(
            case,
            "DeleteStream",
            &json!({"StreamName": name, "EnforceConsumerDeletion": true}),
        )
        .await;
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let (status, answered) = self
                .kinesis("DescribeStreamSummary", &json!({"StreamName": name}))
                .await;
            if status == 400 && error_code(&answered) == RESOURCE_NOT_FOUND {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: Kinesis never reported the stream {name} gone, last {status} \
                 {answered}"
            );
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }

    /// Create the Kinesis data stream `name` and wait for it to report `ACTIVE`, answering the
    /// `StreamARN` Kinesis itself reports for it. The wait looks again until the status holds or
    /// a generous deadline passes; it bounds nothing by how long the transition took.
    async fn active_stream(&self, case: &str, name: &str) -> String {
        self.kinesis_ok(
            case,
            "CreateStream",
            &json!({
                "StreamName": name,
                "StreamModeDetails": {"StreamMode": STREAM_MODE_ON_DEMAND},
            }),
        )
        .await;
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let summary = self
                .kinesis_ok(case, "DescribeStreamSummary", &json!({"StreamName": name}))
                .await;
            let reported = &summary["StreamDescriptionSummary"];
            if reported["StreamStatus"].as_str() == Some(STREAM_ACTIVE) {
                return reported["StreamARN"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an ACTIVE stream reports its ARN, got {summary}")
                    })
                    .to_string();
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the stream {name} never reported {STREAM_ACTIVE}, last \
                 {summary}"
            );
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }

    /// The filter names the group `group` holds, as `DescribeSubscriptionFilters` reports them.
    async fn filters(&self, case: &str, group: &str) -> Vec<String> {
        let page = self
            .logs_ok(
                case,
                "DescribeSubscriptionFilters",
                &json!({"logGroupName": group}),
            )
            .await;
        page.get("subscriptionFilters")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: a read reports subscriptionFilters, got {page}"))
            .iter()
            .filter_map(|filter| filter.get("filterName").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    }
}

/// The error code an answered refusal carries, whichever spelling the envelope uses.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .map(|raw| raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string())
        .unwrap_or_default()
}

/// The sentence an answered refusal states.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A Kinesis data stream ARN of this account and Region.
fn kinesis_arn(stream: &str) -> String {
    format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/{stream}")
}

/// A Firehose delivery stream ARN of this account and Region.
fn firehose_arn(stream: &str) -> String {
    format!("arn:aws:firehose:{REGION}:{ACCOUNT}:deliverystream/{stream}")
}

/// One row of the refusal table: how the destination is named, and the ARN the put carries.
struct Row {
    case: &'static str,
    group: &'static str,
    destination_arn: String,
}

/// A put against a destination whose stream its own service does not hold is refused, and the
/// group holds no filter afterwards.
#[tokio::test]
async fn a_put_against_an_absent_destination_stream_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    // One stream that IS there and one that was there and is gone, so the rows differ in how
    // the absence came about rather than only in the name.
    emulator
        .active_stream("the stream that is deleted", "gone-stream")
        .await;
    emulator
        .deleted_stream("the stream that is deleted", "gone-stream")
        .await;

    let rows = vec![
        Row {
            case: "a Kinesis stream never created",
            group: "/subscription/absent-kinesis",
            destination_arn: kinesis_arn("never-created"),
        },
        Row {
            case: "a Kinesis stream created and then deleted",
            group: "/subscription/deleted-kinesis",
            destination_arn: kinesis_arn("gone-stream"),
        },
        Row {
            case: "a Firehose delivery stream never created",
            group: "/subscription/absent-firehose",
            destination_arn: firehose_arn("never-created"),
        },
    ];

    for row in &rows {
        emulator
            .logs_ok(
                row.case,
                "CreateLogGroup",
                &json!({"logGroupName": row.group}),
            )
            .await;
        let (status, answered) = emulator
            .logs(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": row.group,
                    "filterName": "to-the-stream",
                    "filterPattern": "",
                    "destinationArn": row.destination_arn,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message_of(&answered).as_str()
            ),
            (400, INVALID_PARAMETER, UNDELIVERABLE),
            "case {:?} ({}): expected 400 {INVALID_PARAMETER} {UNDELIVERABLE:?}, got {status} \
             {answered}",
            row.case,
            row.destination_arn
        );
        let held = emulator.filters(row.case, row.group).await;
        assert!(
            held.is_empty(),
            "case {:?}: a refused put stores no filter, got {held:?}",
            row.case
        );
    }
}

/// The same rule through a LOGICAL destination: the put resolves it to the physical stream it
/// names, so a destination whose target is gone refuses the put, and the destination whose
/// target is there configures the filter — read back through the group's own enumeration.
#[tokio::test]
async fn a_logical_destination_is_validated_through_the_stream_it_names() {
    let emulator = Emulator::new();
    let case = "a logical destination over a Kinesis stream";
    let group = "/subscription/logical-destination";

    emulator
        .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let stream_arn = emulator.active_stream(case, "logical-target").await;
    let destination = emulator
        .logs_ok(
            case,
            "PutDestination",
            &json!({
                "destinationName": "logical-dest",
                "targetArn": stream_arn,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let destination_arn = destination["destination"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a put answers the destination's ARN, got {destination}"))
        .to_string();
    emulator
        .logs_ok(
            case,
            "PutDestinationPolicy",
            &json!({
                "destinationName": "logical-dest",
                "accessPolicy": json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"AWS": ACCOUNT},
                        "Action": "logs:PutSubscriptionFilter",
                        "Resource": destination_arn,
                    }],
                })
                .to_string(),
            }),
        )
        .await;

    // The target is there, so the put is configured and the group reports the filter.
    emulator
        .logs_ok(
            case,
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": group,
                "filterName": "through-the-destination",
                "filterPattern": "",
                "destinationArn": destination_arn,
            }),
        )
        .await;
    let held = emulator.filters(case, group).await;
    assert_eq!(
        held,
        vec!["through-the-destination".to_string()],
        "case {case:?}: the accepted put stores its filter, got {held:?}"
    );

    // The target goes away. The verdict is read from current state, so the identical put is
    // refused now — and the filter the group already holds is left exactly as it was.
    emulator.deleted_stream(case, "logical-target").await;
    let (status, answered) = emulator
        .logs(
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": group,
                "filterName": "through-the-destination",
                "filterPattern": "",
                "destinationArn": destination_arn,
            }),
        )
        .await;
    assert_eq!(
        (
            status,
            error_code(&answered).as_str(),
            message_of(&answered).as_str()
        ),
        (400, INVALID_PARAMETER, UNDELIVERABLE),
        "case {case:?} after the target was deleted: expected 400 {INVALID_PARAMETER} \
         {UNDELIVERABLE:?}, got {status} {answered}"
    );
    let after = emulator.filters(case, group).await;
    assert_eq!(
        after, held,
        "case {case:?}: a refused put leaves the group's filters alone, got {after:?}"
    );

    // The stream comes back under the same name, which presents the identical ARN: the put is
    // configured again, because nothing about the refusal is remembered.
    let restored = emulator.active_stream(case, "logical-target").await;
    assert_eq!(
        restored, stream_arn,
        "case {case:?}: a stream created again under the same name presents the same ARN"
    );
    emulator
        .logs_ok(
            case,
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": group,
                "filterName": "through-the-destination",
                "filterPattern": "",
                "destinationArn": destination_arn,
            }),
        )
        .await;
}

/// A put against a stream its own service DOES hold is configured, whichever vendor owns it —
/// the boundary of the rule, so a refusal that fires on everything cannot pass.
#[tokio::test]
async fn a_put_against_a_stream_its_service_holds_is_configured() {
    let emulator = Emulator::new();
    let case = "a Kinesis stream this account holds";
    let group = "/subscription/present-kinesis";

    emulator
        .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let stream_arn = emulator.active_stream(case, "present-stream").await;

    for filter_name in ["first", "second"] {
        emulator
            .logs_ok(
                case,
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": filter_name,
                    "filterPattern": "",
                    "destinationArn": stream_arn,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
    }
    let mut held = emulator.filters(case, group).await;
    held.sort();
    assert_eq!(
        held,
        vec!["first".to_string(), "second".to_string()],
        "case {case:?}: both accepted puts store their filters, got {held:?}"
    );
}
