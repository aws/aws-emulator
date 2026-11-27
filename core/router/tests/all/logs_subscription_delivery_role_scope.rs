// SPDX-License-Identifier: CC0-1.0

//! WHICH role a CloudWatch Logs subscription delivery is made under, and WHAT that role has to
//! allow for a record to reach the destination.
//!
//! RULE: CloudWatch Logs writes to a subscription destination UNDER the IAM role the filter's
//! CONFIGURATION names — the filter's own `roleArn` where the destination is named directly
//! (`PutSubscriptionFilter$roleArn`, "an IAM role that grants CloudWatch Logs permissions to
//! deliver ingested log events to the destination stream"), and the `roleArn` the logical
//! destination carries where the filter names one of those instead (`Destination$roleArn`, "a
//! role for impersonation, used when delivering log events to the target", which is why the
//! filter needs none of its own) — so a record reaches the destination only where the
//! permissions in force on THAT role allow the destination service's own write action
//! (`kinesis:PutRecord` for a data stream, as `PutDestination$roleArn` states it: "grants
//! CloudWatch Logs permissions to call the Amazon Kinesis PutRecord operation on the destination
//! stream") on the PHYSICAL resource the record is written to. A grant that names anything else
//! authorizes nothing — another streaming service's write action, or the logical destination
//! rather than the target it resolves to — and neither the health-check record the put makes nor
//! the envelope the next batch owes is written. The ingestion stands either way: `PutLogEvents`
//! already accepted the batch, so the line stays readable in the log group whatever the delivery
//! did.
//!
//! The two halves this suite pins are the ones its neighbours do not: WHICH of two configured
//! roles governs the write, and which action-and-resource pair the governing role has to carry.
//! What a role allowed nothing at all decides is `logs_subscription_delivery_role.rs`, and what
//! a role nothing can be learned about decides is the CloudWatch Logs crate's own
//! `tests/all/subscription_delivery_authorization.rs`.
//!
//! Three services on one engine — CloudWatch Logs for the group, the filter and the logical
//! destination, IAM for the roles and their policies, Kinesis for the destination the records
//! are read off — because CloudWatch Logs reaches the other two over the in-process callback, so
//! a single-service router has neither to ask.
//!
//! The rows differ along what the rule generalises over: where the governing role comes from
//! (the filter, the logical destination), which action its grant names (the destination
//! service's own write action, another streaming service's), which resource it names (the
//! destination stream, every resource, the logical destination instead of its target), and
//! whether a role the configuration does NOT deliver under would have allowed the write.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    shard_id, SHARD_ITERATOR_TYPE_TRIM_HORIZON, STREAM_ACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::form_encode;

/// The Region every request below is signed for, and the one every ARN here names.
const REGION: &str = "us-west-2";

/// The filter every row configures, and the log stream it writes to.
const FILTER: &str = "to-kinesis";
const LOG_STREAM: &str = "instance-one";

/// The line every row writes, which an empty filter pattern selects.
const LINE: &str = "ERROR disk full";

/// The write action the destination's own service authorizes one delivered record under, and
/// the write action of the OTHER streaming service a subscription filter delivers to, which does
/// not cover a write to a data stream. Sources: `PutDestination$roleArn` ("the Amazon Kinesis
/// PutRecord operation on the destination stream") and `PutSubscriptionFilter$destinationArn`
/// ("A Kinesis Data Firehose delivery stream").
const KINESIS_PUT_RECORD: &str = "kinesis:PutRecord";
const FIREHOSE_PUT_RECORD: &str = "firehose:PutRecord";

/// The trust policy every role here carries: CloudWatch Logs may assume it. Being assumable is
/// not what this suite grades — which role the write is made under, and what it allows, is.
const TRUST_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Principal\":{\"Service\":\"logs.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}";

/// How long a destination stream is waited for before a row fails: far longer than the Kinesis
/// emulator's own `CREATING` -> `ACTIVE` span, so a loaded machine does not fail a row the clock
/// would have served.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(30);

/// How often that stream's status is looked at again while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The three services this suite exercises, mounted on one engine exactly as the router mounts
/// them, with request authentication off: each call carries a credential scope (which is what
/// names the Region) without a computed signature.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The signature is
/// never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Send one request and answer its status with the body as text.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    content_type: &str,
    target: Option<&str>,
    body: String,
) -> (u16, String) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one `awsQuery` IAM action that must succeed: `POST /` with the parameters in the form
/// body, each value percent-encoded as an SDK encodes one.
async fn iam(router: &MultiServiceRouter, case: &str, action: &str, parameters: &[(&str, &str)]) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = send(
        router,
        "iam",
        "application/x-www-form-urlencoded",
        None,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: IAM {action} must succeed, got {status} {answered}"
    );
}

/// Drive one `awsJson1_1` operation of the service `signing_name` serves, answering its status
/// and parsed body.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, text) = send(
        router,
        signing_name,
        "application/x-amz-json-1.1",
        Some(&format!("{prefix}.{operation}")),
        body.to_string(),
    )
    .await;
    let parsed = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One CloudWatch Logs operation that must succeed.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, "logs", "Logs_20140328", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: logs {operation} must succeed, got {status} {parsed}"
    );
    parsed
}

/// One Kinesis operation that must succeed.
async fn kinesis(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, "kinesis", "Kinesis_20131202", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: kinesis {operation} must succeed, got {status} {parsed}"
    );
    parsed
}

/// Look at `stream` until it reports [`STREAM_ACTIVE`], so the rows below put to a stream that
/// can take a record.
///
/// The CONDITION is what is waited for, not a span of time: a create arms a transition the
/// emulator's clock owes, and a row looks again until the stream reports it has been made or
/// [`ACTIVE_DEADLINE`] has passed.
async fn await_active(router: &MultiServiceRouter, case: &str, stream: &str) {
    let deadline = Instant::now() + ACTIVE_DEADLINE;
    loop {
        let summary = kinesis(
            router,
            case,
            "DescribeStreamSummary",
            &json!({"StreamName": stream}),
        )
        .await;
        let status = summary["StreamDescriptionSummary"]["StreamStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status == STREAM_ACTIVE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the destination stream {stream:?} to reach \
             {STREAM_ACTIVE}, got {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The PARTITION KEY of every record the one-shard stream `stream` holds, from its oldest
/// retained one.
///
/// The key is what tells the two writes of one row apart without decompressing anything: a
/// `PutSubscriptionFilter`'s health check is keyed on the log group it validates the filter for,
/// and an envelope of log data grouped by log stream on the log stream it came from ("By
/// default, log data is grouped by log stream").
async fn delivered_keys(router: &MultiServiceRouter, case: &str, stream: &str) -> Vec<String> {
    let iterator = kinesis(
        router,
        case,
        "GetShardIterator",
        &json!({
            "StreamName": stream,
            "ShardId": shard_id(0),
            "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
        }),
    )
    .await["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetShardIterator reports an iterator"))
        .to_string();
    let answer = kinesis(
        router,
        case,
        "GetRecords",
        &json!({"ShardIterator": iterator, "Limit": 100}),
    )
    .await;
    answer["Records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|record| {
            record["PartitionKey"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a record carries a PartitionKey"))
                .to_string()
        })
        .collect()
}

/// The messages the log group `group` holds on [`LOG_STREAM`].
async fn ingested(router: &MultiServiceRouter, case: &str, group: &str) -> Vec<String> {
    let page = logs(
        router,
        case,
        "GetLogEvents",
        &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
    )
    .await;
    page["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| {
            event["message"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a stored event carries a message"))
                .to_string()
        })
        .collect()
}

/// The ARN of the Kinesis data stream `stream`, in the Region every call here is signed for.
fn stream_arn(stream: &str) -> String {
    format!(
        "arn:aws:kinesis:{REGION}:{}:stream/{stream}",
        engine::emulator_account_id()
    )
}

/// The ARN of the CloudWatch Logs logical destination `name`, the form the service documents for
/// one ("arn:aws:logs:Region:account-id:destination:destination-name").
fn destination_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:destination:{name}",
        engine::emulator_account_id()
    )
}

/// The IAM role ARN of this account that `name` names. An IAM ARN carries no Region.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// One identity-based policy document granting `action` on `resource`.
fn allowing(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// Where the role that governs the write comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Governs {
    /// The filter names the destination stream directly, so the filter's own `roleArn` governs.
    TheFiltersOwnRole,
    /// The filter names a logical destination, so that destination's `roleArn` governs the write
    /// to the target it resolves to.
    TheLogicalDestinationsRole,
}

/// One row: where the governing role comes from, what it and a role the configuration does not
/// deliver under allow, and whether the writes must reach the destination.
struct Row {
    case: &'static str,
    /// The row's own suffix: its roles, log group, destination and stream are all named from it,
    /// so every row is isolated from every other.
    name: &'static str,
    governs: Governs,
    /// The inline policy document embedded in the role the configuration names, or `None` for a
    /// role carrying none.
    governing: Option<String>,
    /// The inline policy of a SECOND role the filter carries beside a logical destination — a
    /// role the configuration does not deliver under.
    ungoverning: Option<String>,
    /// Whether the put's health check and the batch's envelope must reach the destination.
    delivers: bool,
}

impl Row {
    /// The role the write is authorized against — the one whose permissions the configuration
    /// puts in force — which IAM holds under the row's own name.
    fn governing_role(&self) -> &'static str {
        self.name
    }

    /// The role the filter carries where it is not the governing one.
    fn ungoverning_role(&self) -> String {
        format!("{}-filter", self.name)
    }
}

fn rows() -> Vec<Row> {
    let own_stream = |name: &str| stream_arn(&format!("stream-{name}"));
    vec![
        Row {
            case: "the filter's own role allowing the destination service's write on the stream",
            name: "filter-allows",
            governs: Governs::TheFiltersOwnRole,
            governing: Some(allowing(KINESIS_PUT_RECORD, &own_stream("filter-allows"))),
            ungoverning: None,
            delivers: true,
        },
        Row {
            case: "the filter's own role allowing that write on every resource",
            name: "filter-every-resource",
            governs: Governs::TheFiltersOwnRole,
            governing: Some(allowing(KINESIS_PUT_RECORD, "*")),
            ungoverning: None,
            delivers: true,
        },
        Row {
            case: "the filter's own role allowing ANOTHER streaming service's write on every \
                   resource",
            name: "filter-other-service",
            governs: Governs::TheFiltersOwnRole,
            governing: Some(allowing(FIREHOSE_PUT_RECORD, "*")),
            ungoverning: None,
            delivers: false,
        },
        Row {
            case: "a logical destination's own role allowing the write on the stream it targets",
            name: "logical-allows",
            governs: Governs::TheLogicalDestinationsRole,
            governing: Some(allowing(KINESIS_PUT_RECORD, &own_stream("logical-allows"))),
            ungoverning: None,
            delivers: true,
        },
        Row {
            case: "a logical destination's own role allowing nothing",
            name: "logical-allows-nothing",
            governs: Governs::TheLogicalDestinationsRole,
            governing: None,
            ungoverning: None,
            delivers: false,
        },
        Row {
            case: "a logical destination's own role allowing the write on the destination rather \
                   than on the stream it targets",
            name: "logical-other-resource",
            governs: Governs::TheLogicalDestinationsRole,
            governing: Some(allowing(
                KINESIS_PUT_RECORD,
                &destination_arn("logical-other-resource"),
            )),
            ungoverning: None,
            delivers: false,
        },
        Row {
            case: "a logical destination's own role allowing nothing while the role the filter \
                   itself carries allows the write",
            name: "logical-over-filter",
            governs: Governs::TheLogicalDestinationsRole,
            governing: None,
            ungoverning: Some(allowing(
                KINESIS_PUT_RECORD,
                &own_stream("logical-over-filter"),
            )),
            delivers: false,
        },
    ]
}

#[tokio::test]
async fn a_subscription_delivery_is_authorized_against_the_role_its_configuration_names() {
    let router = emulator();
    let rows = rows();

    // Every row's roles, destination stream, log group and log stream, before anything is put:
    // the streams are created together so one wait covers the spans their creates armed.
    for row in &rows {
        let case = row.case;
        iam(
            &router,
            case,
            "CreateRole",
            &[
                ("RoleName", row.governing_role()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        if let Some(governing) = &row.governing {
            iam(
                &router,
                case,
                "PutRolePolicy",
                &[
                    ("RoleName", row.governing_role()),
                    ("PolicyName", "delivery"),
                    ("PolicyDocument", governing.as_str()),
                ],
            )
            .await;
        }
        if let Some(ungoverning) = &row.ungoverning {
            iam(
                &router,
                case,
                "CreateRole",
                &[
                    ("RoleName", row.ungoverning_role().as_str()),
                    ("AssumeRolePolicyDocument", TRUST_POLICY),
                ],
            )
            .await;
            iam(
                &router,
                case,
                "PutRolePolicy",
                &[
                    ("RoleName", row.ungoverning_role().as_str()),
                    ("PolicyName", "delivery"),
                    ("PolicyDocument", ungoverning.as_str()),
                ],
            )
            .await;
        }
        kinesis(
            &router,
            case,
            "CreateStream",
            &json!({"StreamName": format!("stream-{}", row.name), "ShardCount": 1}),
        )
        .await;
        let group = format!("/aws/emulator/{}", row.name);
        logs(&router, case, "CreateLogGroup", &json!({"logGroupName": group})).await;
        logs(
            &router,
            case,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
        )
        .await;
    }
    for row in &rows {
        await_active(&router, row.case, &format!("stream-{}", row.name)).await;
    }

    for row in &rows {
        let case = row.case;
        let stream = format!("stream-{}", row.name);
        let group = format!("/aws/emulator/{}", row.name);

        // The destination the filter names, and the role the filter itself carries: a stream
        // named directly is delivered to under the filter's own role, while a logical
        // destination carries the role its target is written to under — and is authorized by
        // the access policy that lets this account subscribe to it ("the destination owner must
        // call PutDestinationPolicy after PutDestination").
        let (destination, filter_role) = match row.governs {
            Governs::TheFiltersOwnRole => {
                (stream_arn(&stream), Some(role_arn(row.governing_role())))
            }
            Governs::TheLogicalDestinationsRole => {
                let created = logs(
                    &router,
                    case,
                    "PutDestination",
                    &json!({
                        "destinationName": row.name,
                        "targetArn": stream_arn(&stream),
                        "roleArn": role_arn(row.governing_role()),
                    }),
                )
                .await;
                let reported = created["destination"]["arn"].as_str().unwrap_or_default();
                assert_eq!(
                    reported,
                    destination_arn(row.name),
                    "case {case:?}: the destination reports its own ARN, got {created}"
                );
                let arn = reported.to_string();
                logs(
                    &router,
                    case,
                    "PutDestinationPolicy",
                    &json!({
                        "destinationName": row.name,
                        "accessPolicy": json!({
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Principal": {
                                    "AWS": format!(
                                        "arn:aws:iam::{}:root",
                                        engine::emulator_account_id()
                                    ),
                                },
                                "Action": "logs:PutSubscriptionFilter",
                                "Resource": arn,
                            }],
                        })
                        .to_string(),
                    }),
                )
                .await;
                (
                    arn,
                    row.ungoverning
                        .as_ref()
                        .map(|_| role_arn(&row.ungoverning_role())),
                )
            }
        };

        // The put is accepted whatever the role is allowed to write: what the role decides is
        // what reaches the destination, not whether the filter is configured.
        let mut put = json!({
            "logGroupName": group,
            "filterName": FILTER,
            "filterPattern": "",
            "destinationArn": destination,
        });
        if let Some(filter_role) = &filter_role {
            put["roleArn"] = json!(filter_role);
        }
        logs(&router, case, "PutSubscriptionFilter", &put).await;

        // One in-window event: the ingestion window reaches 14 days back, so an hour ago is
        // comfortably inside it.
        let timestamp = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the Unix epoch")
                .as_millis(),
        )
        .expect("the epoch milliseconds fit in an i64")
            - 60 * 60 * 1_000;
        logs(
            &router,
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": LOG_STREAM,
                "logEvents": [{"timestamp": timestamp, "message": LINE}],
            }),
        )
        .await;

        let keys = delivered_keys(&router, case, &stream).await;
        // What the stream holds, in order: the test message `PutDestination` delivered while it
        // VALIDATED the destination — keyed on the destination it was configuring, and written
        // for every row that creates one, because validating a target is not a delivery of log
        // data and so is not the delivery role's to allow — then, for a row whose role governs
        // the write, the health check the put wrote (keyed on the log group it validated the
        // filter for) and the envelope of the batch (keyed on the log stream it came from).
        let mut expected: Vec<String> = Vec::new();
        if matches!(row.governs, Governs::TheLogicalDestinationsRole) {
            expected.push(row.name.to_string());
        }
        if row.delivers {
            expected.push(group.clone());
            expected.push(LOG_STREAM.to_string());
        }
        assert_eq!(
            keys, expected,
            "case {case:?}: expected the destination to hold {expected:?}, got {keys:?}"
        );

        // The batch is ingested whatever the delivery did: `PutLogEvents` answered it, and
        // whether a destination took the envelope is not part of that answer.
        let stored = ingested(&router, case, &group).await;
        assert_eq!(
            stored,
            vec![LINE.to_string()],
            "case {case:?}: the ingested line stays readable, got {stored:?}"
        );
    }
}
