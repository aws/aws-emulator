// SPDX-License-Identifier: CC0-1.0

//! An event bus whose `LogConfig` asks for the ERROR records reports every handover of a
//! matched event that FAILED, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: <prefix>.<Operation>`) through the real `MultiServiceRouter`, with
//! CloudWatch Logs and Amazon SQS mounted on the same engine so the records are asserted
//! where a client reads them: the log events `FilterLogEvents` reports on the group a
//! vended-log delivery points the bus at.
//!
//! The RULE asserted here: a bus whose `LogConfig$Level` covers the error records writes ONE
//! `INVOCATION_FAILURE` record, at the `ERROR` level, per target invocation of one of its
//! rules that did not succeed — naming the bus (its ARN and its name), the `EventId` of the
//! event that was not delivered, and, in its `details`, the ARN of the rule whose pattern
//! matched, the `Arn` of the target the handover was made on, the condition EventBridge
//! reports for the failure and what that condition reported — into the log group of every
//! delivery whose source represents that bus for a log type carrying those records. The
//! event itself travels in that record only where `IncludeDetail` is `FULL`, a bus whose
//! level does not cover the error records writes none, and a handover that SUCCEEDED is no
//! failure, so nothing is written about it.
//!
//! The tables span what the rule generalises over rather than one recorded request: every
//! modelled `Level` paired with both `IncludeDetail` values, the target kinds whose own
//! documents withhold an invocation (a CloudWatch Logs log group and an Amazon SQS queue),
//! those same kinds with the grant in place, several events in one publication, and the
//! pipeline differing in whether it exists at all and in which log type its source
//! represents the bus under. Every row is a FRESH emulator, so one row's pipeline never
//! carries another row's records.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::bus_logs::{
    ERROR_LOGS_LOG_TYPE, EVENT_INGEST_SUCCESS, INCLUDE_DETAIL_FULL, INFO_LOGS_LOG_TYPE,
    INVOCATION_FAILURE, LOG_LEVEL_ERROR, LOG_LEVEL_INFO, LOG_LEVEL_OFF, LOG_LEVEL_TRACE,
};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, NO_PERMISSIONS, NO_RESOURCE, SERVICE_PRINCIPAL,
};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the Region every ARN here carries.
const REGION: &str = "eu-west-2";

/// The `IncludeDetail` value that carries no event information (`INCLUDE_DETAIL_VALUES`).
const INCLUDE_DETAIL_NONE: &str = "NONE";

/// The bus every case publishes onto, and the rule on it whose target the handover is made
/// on.
const BUS: &str = "orders-bus";
const RULE: &str = "to-audit";

/// The event every case publishes, which the rule's pattern matches.
const SOURCE: &str = "custom.orders";
const DETAIL_TYPE: &str = "OrderPlaced";

/// The group a vended-log delivery carries the bus's records into.
const VENDED_GROUP: &str = "/aws/vendedlogs/events/bus-errors";

/// The log group a rule target names, which is NOT the group the bus's own records travel
/// to.
const TARGET_GROUP: &str = "/aws/events/audit";

/// The action a delivery to a log-group target is withheld from when the group grants it
/// nothing, which is what the record's reported message is about.
const PUT_LOG_EVENTS: &str = "logs:PutLogEvents";

/// The action a delivery to a queue target performs.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// EventBridge and the services a target of these cases lives in, on ONE engine — the shape
/// `core/router/src/lib.rs` mounts them in.
struct Aws {
    router: MultiServiceRouter,
}

fn emulator() -> Aws {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Aws { router }
}

/// One `awsJson` RPC call against the service whose signing name is `signing_name`.
async fn rpc(
    aws: &Aws,
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
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = aws.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// One call of `service`, asserting a 200 and answering the body.
async fn call(
    aws: &Aws,
    case: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, answer) = rpc(aws, signing_name, target_prefix, json_version, operation, body)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} answered {status} {answer}"
    );
    answer
}

/// One EventBridge call.
async fn events(aws: &Aws, case: &str, operation: &str, body: &Value) -> Value {
    call(aws, case, "events", "AWSEvents", "1.1", operation, body).await
}

/// One CloudWatch Logs call.
async fn logs(aws: &Aws, case: &str, operation: &str, body: &Value) -> Value {
    call(aws, case, "logs", "Logs_20140328", "1.1", operation, body).await
}

/// One Amazon SQS call.
async fn sqs(aws: &Aws, case: &str, operation: &str, body: &Value) -> Value {
    call(aws, case, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// A `LogConfig` as a client writes one: both members, which is the form EventBridge accepts.
fn log_config(level: &str, include_detail: &str) -> Value {
    json!({ "Level": level, "IncludeDetail": include_detail })
}

/// The bus every case publishes onto, configured with `config`; answers its ARN.
async fn given_bus(aws: &Aws, case: &str, config: Option<Value>) -> String {
    let mut create = json!({ "Name": BUS });
    if let Some(config) = config {
        create["LogConfig"] = config;
    }
    events(aws, case, "CreateEventBus", &create).await;
    let described = events(aws, case, "DescribeEventBus", &json!({ "Name": BUS })).await;
    described["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeEventBus reported no ARN: {described}"))
        .to_string()
}

/// A log group called `name`, answered with the ARN `DescribeLogGroups` reports for it.
async fn given_log_group(aws: &Aws, case: &str, name: &str) -> String {
    logs(aws, case, "CreateLogGroup", &json!({ "logGroupName": name })).await;
    let listed = logs(
        aws,
        case,
        "DescribeLogGroups",
        &json!({ "logGroupNamePrefix": name }),
    )
    .await;
    listed["logGroups"][0]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeLogGroups reported no ARN: {listed}"))
        .to_string()
}

/// `group_arn` naming the group alone, which is the ARN a rule's target carries.
fn bare(group_arn: &str) -> String {
    group_arn.strip_suffix(":*").unwrap_or(group_arn).to_string()
}

/// `group_arn` naming the group together with everything under it, the resource a grant for
/// the write is written over.
fn everything_under(group_arn: &str) -> String {
    format!("{}:*", bare(group_arn))
}

/// Grant the EventBridge service principal the two calls a log-group delivery is made of on
/// everything under `group_arn`.
async fn given_log_group_grant(aws: &Aws, case: &str, group_arn: &str) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": SERVICE_PRINCIPAL },
            "Action": ["logs:CreateLogStream", PUT_LOG_EVENTS],
            "Resource": everything_under(group_arn),
        }],
    })
    .to_string();
    logs(
        aws,
        case,
        "PutResourcePolicy",
        &json!({ "policyName": "group-delivery", "resourceArn": group_arn,
                 "policyDocument": document }),
    )
    .await;
}

/// A queue called `name`, granting the EventBridge service principal the send when `granted`;
/// answers its ARN.
async fn given_queue(aws: &Aws, case: &str, name: &str, granted: bool) -> String {
    let created = sqs(aws, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateQueue reported no url: {created}"))
        .to_string();
    let attributes = sqs(
        aws,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    if granted {
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": SERVICE_PRINCIPAL },
                "Action": SEND_MESSAGE,
                "Resource": arn,
            }],
        })
        .to_string();
        sqs(
            aws,
            case,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": document } }),
        )
        .await;
    }
    arn
}

/// The rule on the bus whose pattern matches the published events, pointed at `target_arn`.
async fn given_rule(aws: &Aws, case: &str, target_arn: &str) {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    events(
        aws,
        case,
        "PutRule",
        &json!({ "Name": RULE, "EventBusName": BUS, "EventPattern": pattern,
                 "State": "ENABLED" }),
    )
    .await;
    let installed = events(
        aws,
        case,
        "PutTargets",
        &json!({ "Rule": RULE, "EventBusName": BUS,
                 "Targets": [{ "Id": "t", "Arn": target_arn }] }),
    )
    .await;
    assert_eq!(
        installed["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the target installed, got {installed}"
    );
}

/// The three calls a client builds a vended-log pipeline with: a delivery source over the
/// bus's ARN for `log_type`, a destination over the group, and the delivery pairing them.
async fn given_delivery(
    aws: &Aws,
    case: &str,
    log_type: &str,
    bus_arn: &str,
    group_arn: &str,
) {
    logs(
        aws,
        case,
        "PutDeliverySource",
        &json!({ "name": "bus-logs", "resourceArn": bus_arn, "logType": log_type }),
    )
    .await;
    let destination = logs(
        aws,
        case,
        "PutDeliveryDestination",
        &json!({
            "name": "bus-logs-to-logs",
            "deliveryDestinationConfiguration": { "destinationResourceArn": group_arn },
        }),
    )
    .await;
    let destination_arn = destination["deliveryDestination"]["arn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PutDeliveryDestination reported no ARN: {destination}")
        })
        .to_string();
    logs(
        aws,
        case,
        "CreateDelivery",
        &json!({ "deliverySourceName": "bus-logs", "deliveryDestinationArn": destination_arn }),
    )
    .await;
}

/// Publish one entry per `detail` onto the bus, and answer the `EventId` of each in the
/// request's order.
async fn publish(aws: &Aws, case: &str, details: &[&str]) -> Vec<String> {
    let entries: Vec<Value> = details
        .iter()
        .map(|detail| {
            json!({
                "EventBusName": BUS,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": detail,
            })
        })
        .collect();
    let answer = events(aws, case, "PutEvents", &json!({ "Entries": entries })).await;
    assert_eq!(
        answer["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected every entry accepted, got {answer}"
    );
    answer["Entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["EventId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an accepted entry carries an EventId: {entry}")
                })
                .to_string()
        })
        .collect()
}

/// Every record the group `group` holds, parsed out of the log events `FilterLogEvents`
/// reports.
async fn records(aws: &Aws, case: &str, group: &str) -> Vec<Value> {
    let answer = logs(
        aws,
        case,
        "FilterLogEvents",
        &json!({ "logGroupName": group }),
    )
    .await;
    answer["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| {
            let message = event["message"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(message).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivered record is JSON ({error}): {message}")
            })
        })
        .collect()
}

/// The records of `group` that report a failed handover.
async fn failures(aws: &Aws, case: &str, group: &str) -> Vec<Value> {
    records(aws, case, group)
        .await
        .into_iter()
        .filter(|record| record["message_type"] == json!(INVOCATION_FAILURE))
        .collect()
}

/// The details every case publishes: they differ in the shape and the size of the document,
/// so one row covers a set of events rather than one.
fn details() -> Vec<&'static str> {
    vec![
        r#"{"orderId":"A-1","amount":7}"#,
        r#"{"order":{"id":"A-2","lines":[{"sku":"x","qty":2}]},"tags":["eu","web"]}"#,
    ]
}

/// RULE: a bus whose level covers the error records writes one `INVOCATION_FAILURE` record
/// per handover that failed, carrying the rule, the target and the condition — and the event
/// itself only under `IncludeDetail` `FULL`; a bus whose level does not cover them writes
/// nothing.
#[tokio::test]
async fn the_configured_level_and_detail_decide_the_failure_records_a_bus_writes() {
    // One row per (`Level`, `IncludeDetail`) pair over the values the two modelled enums
    // admit (`LEVEL_VALUES`, `INCLUDE_DETAIL_VALUES`), plus a bus that configures no logging
    // at all.
    struct Row {
        case: &'static str,
        config: Option<Value>,
        writes_records: bool,
        carries_event: bool,
    }
    let rows = vec![
        Row {
            case: "no logging configured at all",
            config: None,
            writes_records: false,
            carries_event: false,
        },
        Row {
            case: "OFF with full detail",
            config: Some(log_config(LOG_LEVEL_OFF, INCLUDE_DETAIL_FULL)),
            writes_records: false,
            carries_event: false,
        },
        Row {
            case: "ERROR with full detail",
            config: Some(log_config(LOG_LEVEL_ERROR, INCLUDE_DETAIL_FULL)),
            writes_records: true,
            carries_event: true,
        },
        Row {
            case: "ERROR with no detail",
            config: Some(log_config(LOG_LEVEL_ERROR, INCLUDE_DETAIL_NONE)),
            writes_records: true,
            carries_event: false,
        },
        Row {
            case: "INFO with full detail",
            config: Some(log_config(LOG_LEVEL_INFO, INCLUDE_DETAIL_FULL)),
            writes_records: true,
            carries_event: true,
        },
        Row {
            case: "TRACE with no detail",
            config: Some(log_config(LOG_LEVEL_TRACE, INCLUDE_DETAIL_NONE)),
            writes_records: true,
            carries_event: false,
        },
    ];

    for Row {
        case,
        config,
        writes_records,
        carries_event,
    } in rows
    {
        let aws = emulator();
        let bus_arn = given_bus(&aws, case, config).await;
        // The target group grants this service nothing, so every handover to it is
        // withheld: that is the failure the records below report.
        let target_arn = bare(&given_log_group(&aws, case, TARGET_GROUP).await);
        given_rule(&aws, case, &target_arn).await;
        let vended_arn = given_log_group(&aws, case, VENDED_GROUP).await;
        given_delivery(&aws, case, ERROR_LOGS_LOG_TYPE, &bus_arn, &vended_arn).await;

        let published = details();
        let event_ids = publish(&aws, case, &published).await;
        // Nothing reached the target group: every handover was withheld.
        let delivered = records(&aws, case, TARGET_GROUP).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: expected the ungranted target group to stay empty, got {delivered:?}"
        );

        let written = failures(&aws, case, VENDED_GROUP).await;
        if !writes_records {
            assert!(
                written.is_empty(),
                "case {case:?}: expected no record at all, got {written:?}"
            );
            continue;
        }
        assert_eq!(
            written.len(),
            event_ids.len(),
            "case {case:?}: expected one record per withheld handover ({}), got {written:?}",
            event_ids.len()
        );
        for event_id in &event_ids {
            let record = written
                .iter()
                .find(|record| record["event_id"] == json!(event_id))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no record for EventId {event_id}, got {written:?}")
                });
            for (member, expected) in [
                ("message_type", json!(INVOCATION_FAILURE)),
                ("resource_arn", json!(bus_arn)),
                ("event_bus_name", json!(BUS)),
                // The record is an ERROR one however verbose the bus's own level is.
                ("log_level", json!(LOG_LEVEL_ERROR)),
            ] {
                assert_eq!(
                    record[member], expected,
                    "case {case:?}: expected the {member} member to be {expected}, got {record}"
                );
            }
            assert!(
                record["message_timestamp_ms"].as_i64().unwrap_or_default() > 0,
                "case {case:?}: expected an epoch-millisecond instant, got {record}"
            );
            for (member, expected) in [
                ("rule_arn", json!(rule_arn(REGION, BUS, RULE))),
                ("target_arn", json!(target_arn)),
                ("final_invocation_status", json!(NO_PERMISSIONS)),
            ] {
                assert_eq!(
                    record["details"][member], expected,
                    "case {case:?}: expected the {member} detail to be {expected}, got {record}"
                );
            }
            let reported = record["details"]["error_message"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                reported.contains(PUT_LOG_EVENTS) && reported.contains(SERVICE_PRINCIPAL),
                "case {case:?}: expected the refusal of {PUT_LOG_EVENTS} as \
                 {SERVICE_PRINCIPAL}, got {reported:?}"
            );
            let carried = record["details"].get("target_input").cloned();
            if carries_event {
                let carried = carried.clone().unwrap_or(Value::Null);
                let envelope: Value = serde_json::from_str(carried.as_str().unwrap_or_default())
                    .unwrap_or_else(|error| {
                        panic!(
                            "case {case:?}: the input the target was to be handed is the JSON \
                             text a target receives ({error}): {carried}"
                        )
                    });
                assert_eq!(
                    envelope["id"],
                    json!(event_id),
                    "case {case:?}: expected the envelope of the event that was not \
                     delivered, got {envelope}"
                );
                assert!(
                    published.iter().any(|detail| {
                        serde_json::from_str::<Value>(detail).unwrap_or(Value::Null)
                            == envelope["detail"]
                    }),
                    "case {case:?}: expected the envelope to carry one of the published \
                     details, got {envelope}"
                );
            } else {
                assert!(
                    carried.is_none(),
                    "case {case:?}: expected no target_input, got {carried:?}"
                );
            }
        }
    }
}

/// Which target the handover was made on, and whether that target's own documents admit it.
enum Target {
    /// A CloudWatch Logs log group, whose resource policy decides the write.
    LogGroup { granted: bool },
    /// An Amazon SQS queue, whose `Policy` decides the send.
    Queue { granted: bool },
}

/// RULE: every handover that failed is reported, whichever target kind withheld it and
/// whichever log type the delivery carries the bus's records under — and a handover that
/// SUCCEEDED is reported by no failure record at all.
#[tokio::test]
async fn every_withheld_handover_is_reported_through_a_delivery_that_carries_this_bus() {
    struct Row {
        case: &'static str,
        target: Target,
        /// The source's `logType`, and `None` for an account that built no pipeline.
        log_type: Option<&'static str>,
        /// The condition the record reports, and `None` where no record is written.
        reports: Option<&'static str>,
    }
    let rows = vec![
        Row {
            case: "a log group granting nothing, over a delivery of the error logs",
            target: Target::LogGroup { granted: false },
            log_type: Some(ERROR_LOGS_LOG_TYPE),
            reports: Some(NO_PERMISSIONS),
        },
        Row {
            case: "a log group granting nothing, over a delivery of the informational logs",
            target: Target::LogGroup { granted: false },
            log_type: Some(INFO_LOGS_LOG_TYPE),
            reports: Some(NO_PERMISSIONS),
        },
        Row {
            case: "a log group granting nothing, with no delivery at all",
            target: Target::LogGroup { granted: false },
            log_type: None,
            reports: None,
        },
        Row {
            case: "a log group granting the write",
            target: Target::LogGroup { granted: true },
            log_type: Some(ERROR_LOGS_LOG_TYPE),
            reports: None,
        },
        Row {
            case: "a queue granting nothing",
            target: Target::Queue { granted: false },
            log_type: Some(ERROR_LOGS_LOG_TYPE),
            // Amazon SQS does not disclose a queue it will not let the caller write to
            // and answers under its own not-found code rather than refusing the identity
            // the way a log group does, so the class read off that answer is the
            // resource one.
            reports: Some(NO_RESOURCE),
        },
        Row {
            case: "a queue granting the send",
            target: Target::Queue { granted: true },
            log_type: Some(ERROR_LOGS_LOG_TYPE),
            reports: None,
        },
    ];

    for Row {
        case,
        target,
        log_type,
        reports,
    } in rows
    {
        let aws = emulator();
        let bus_arn = given_bus(
            &aws,
            case,
            Some(log_config(LOG_LEVEL_ERROR, INCLUDE_DETAIL_FULL)),
        )
        .await;
        let target_arn = match target {
            Target::LogGroup { granted } => {
                let group_arn = given_log_group(&aws, case, TARGET_GROUP).await;
                if granted {
                    given_log_group_grant(&aws, case, &group_arn).await;
                }
                bare(&group_arn)
            }
            Target::Queue { granted } => given_queue(&aws, case, "audit-queue", granted).await,
        };
        given_rule(&aws, case, &target_arn).await;
        let vended_arn = given_log_group(&aws, case, VENDED_GROUP).await;
        if let Some(log_type) = log_type {
            given_delivery(&aws, case, log_type, &bus_arn, &vended_arn).await;
        }

        let event_ids = publish(&aws, case, &details()).await;
        let written = failures(&aws, case, VENDED_GROUP).await;
        let Some(condition) = reports else {
            assert!(
                written.is_empty(),
                "case {case:?}: expected no failure record, got {written:?}"
            );
            // A bus at the error level writes no informational record either, whatever its
            // pipeline carries.
            let ingested: Vec<Value> = records(&aws, case, VENDED_GROUP)
                .await
                .into_iter()
                .filter(|record| record["message_type"] == json!(EVENT_INGEST_SUCCESS))
                .collect();
            assert!(
                ingested.is_empty(),
                "case {case:?}: expected no ingest record at the error level, got {ingested:?}"
            );
            continue;
        };
        assert_eq!(
            written.len(),
            event_ids.len(),
            "case {case:?}: expected one record per withheld handover ({}), got {written:?}",
            event_ids.len()
        );
        for record in &written {
            for (member, expected) in [
                ("rule_arn", json!(rule_arn(REGION, BUS, RULE))),
                ("target_arn", json!(target_arn)),
                ("final_invocation_status", json!(condition)),
            ] {
                assert_eq!(
                    record["details"][member], expected,
                    "case {case:?}: expected the {member} detail to be {expected}, got {record}"
                );
            }
            assert!(
                record["details"]["error_message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: expected the refusal the target's service gave, got {record}"
            );
        }
    }
}
