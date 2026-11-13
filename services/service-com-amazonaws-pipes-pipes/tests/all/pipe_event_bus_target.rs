// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING pipe whose `Target` is an EVENTBRIDGE EVENT BUS does, end to end over the
//! wire with pipes, Amazon SQS, IAM and EventBridge mounted on ONE engine.
//!
//! The RULE asserted over the table below: a pipe whose target is an event bus PUBLISHES
//! each event its target stage was handed on that bus as one event of its own — its `source`
//! and `detail-type` the `Source` and `DetailType` the pipe's
//! `PipeTargetEventBridgeEventBusParameters` configure, and its `detail` the payload the
//! stage was handed (the record envelope, or what the target's `InputTemplate` rendered from
//! it) — so a rule on that bus matching the configured source forwards the record to its own
//! target, and the source record is consumed whether or not any rule matches. A parameter
//! whose value is a JSON path into the event (`$.…`) contributes the value the record itself
//! carries, exactly as every other target parameter does.
//!
//! Nothing here publishes anything on the bus: the only data-plane call a row makes is the
//! `SendMessage` that puts one record on the source queue. Every message read back off the
//! SINK queue got there because the pipe published an event the bus rule matched, so a pipe
//! that reports `RUNNING` and publishes nothing leaves that queue empty and the row fails.
//!
//! The rows differ along what the rule generalises over: whether the `DetailType` is a
//! literal value or a path into the record, whether the target renders an `InputTemplate`
//! instead of handing the record on, which `Source` the events are published under (one the
//! bus's rule matches and one it does not) and how many `Resources` the configured event
//! names.
//!
//! The member names are the model's own
//! (`smithy_query(query="shape", shape="PipeTargetParameters,PipeTargetEventBridgeEventBusParameters")`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::SERVICE_PRINCIPAL as EVENTS_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::pipe_state::RUNNING;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{
    collect_at_least, collect_during, counted, given_queue, sqs, wait_for_counted, QUIET,
    VISIBLE_ATTRIBUTE,
};
use crate::pipe_execution_role::{wait_for_state, IN_FLIGHT_ATTRIBUTE};
use crate::pipe_lifecycle::{call, REGION};
use crate::pipe_metrics::given_role_stating;

/// How long work the pipe owes — settling into `RUNNING`, polling the record, publishing it
/// and the bus forwarding it — is given before a row fails. Far above the data plane's poll
/// interval and the span a pipe takes to settle, so a row fails only when the work did not
/// happen at all and a loaded host does not make a slow pass look like a missing one.
const BUDGET: Duration = Duration::from_secs(60);

/// The `source` the bus's rule matches, and one it does not: the first is what a routed row
/// publishes under, the second what the unrouted row publishes under.
const MATCHED_SOURCE: &str = "emulator.pipes";
/// See [`MATCHED_SOURCE`].
const UNMATCHED_SOURCE: &str = "emulator.elsewhere";

/// The bus a pipe below targets, the rule on it and the id its sink target carries.
const BUS: &str = "pipes-event-bus";
/// See [`BUS`].
const RULE: &str = "pipes-event-bus-rule";
/// See [`BUS`].
const TARGET_ID: &str = "sink";

/// The attributes that together say how many records the source still HOLDS: the ones
/// waiting on it plus the ones a consumer has taken and not deleted.
const HELD_ATTRIBUTES: [&str; 2] = [VISIBLE_ATTRIBUTE, IN_FLIGHT_ATTRIBUTE];

/// Pipes, Amazon SQS, IAM and EventBridge on one engine, as `core/router/src/lib.rs` mounts
/// them: the record is sent where a client sends it, the bus and its rule are EventBridge's,
/// the role the pipe's stages act as is IAM's, and the sink this suite reads is the queue the
/// rule delivered to.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: EventBridge {operation} answered {status} {parsed}"
    );
    parsed
}

/// A bus called [`BUS`] carrying one ENABLED rule that matches [`MATCHED_SOURCE`] and
/// forwards to the queue `sink_arn`; answers the bus's ARN.
async fn given_bus_forwarding_to(
    router: &MultiServiceRouter,
    case: &str,
    sink_arn: &str,
) -> String {
    let created = events(router, case, "CreateEventBus", &json!({ "Name": BUS })).await;
    let arn = created["EventBusArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no EventBusArn in {created}"))
        .to_string();
    events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": RULE,
            "EventBusName": BUS,
            "State": "ENABLED",
            "EventPattern": json!({ "source": [MATCHED_SOURCE] }).to_string(),
        }),
    )
    .await;
    let targeted = events(
        router,
        case,
        "PutTargets",
        &json!({
            "Rule": RULE,
            "EventBusName": BUS,
            "Targets": [{ "Id": TARGET_ID, "Arn": sink_arn }],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "case {case:?}: the sink target must be accepted, got {targeted}"
    );
    arn
}

/// Grant the EventBridge service principal `sqs:SendMessage` on the sink, which is the
/// document a rule's delivery into a queue is decided by ("for Lambda functions, Amazon SNS
/// topics and Amazon SQS queues, EventBridge relies on resource-based policies",
/// `PutTargets`).
async fn allow_the_rule_to_send(router: &MultiServiceRouter, case: &str, url: &str, arn: &str) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    })
    .to_string();
    let (status, answer) = sqs(
        router,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {answer}"
    );
}

/// What a row expects the published event's `detail` to be.
enum Detail {
    /// The record envelope itself: the body as sent, the message id Amazon SQS minted and
    /// the source queue the record came off.
    Record,
    /// What the target's `InputTemplate` rendered from the record.
    Rendered,
}

/// One row of the table: the target parameters the pipe publishes under and the event they
/// must come to.
struct Row {
    case: &'static str,
    /// `TargetParameters` as the pipe carries them, with `MARKER` standing for the row's own
    /// marker where a template names it.
    parameters: fn(&str) -> Value,
    /// Whether the bus's rule matches the event, so the sink receives it.
    routed: bool,
    /// The `detail-type` the published event must carry.
    detail_type: &'static str,
    /// The `resources` it must carry, in order.
    resources: &'static [&'static str],
    detail: Detail,
}

/// The `kind` every record below carries in its JSON body, which the path row reads its
/// `DetailType` out of.
const KIND: &str = "alpha";

/// RULE: an event-bus target publishes one event per event handed to the target stage, under
/// the `Source`, `DetailType` and `Resources` the pipe configured — each resolved against the
/// record when it is a path into it — with the payload the stage was handed as the event's
/// `detail`; a rule on the bus matching that source then forwards it, and the record leaves
/// the source either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_bus_target_publishes_each_record_under_its_configured_members() {
    let rows: &[Row] = &[
        Row {
            case: "the source and detail type as configured",
            parameters: |_| {
                json!({
                    "EventBridgeEventBusParameters": {
                        "Source": MATCHED_SOURCE,
                        "DetailType": "OrderPlaced",
                    }
                })
            },
            routed: true,
            detail_type: "OrderPlaced",
            resources: &[],
            detail: Detail::Record,
        },
        Row {
            case: "the detail type taken from the record",
            parameters: |_| {
                json!({
                    "EventBridgeEventBusParameters": {
                        "Source": MATCHED_SOURCE,
                        "DetailType": "$.body.kind",
                    }
                })
            },
            routed: true,
            detail_type: KIND,
            resources: &[],
            detail: Detail::Record,
        },
        Row {
            case: "the detail rendered by the target's input template",
            parameters: |marker| {
                json!({
                    "InputTemplate": format!("{{\"ordered\": \"<$.body.marker>\", \"marker\": \"{marker}\"}}"),
                    "EventBridgeEventBusParameters": {
                        "Source": MATCHED_SOURCE,
                        "DetailType": "OrderRendered",
                    }
                })
            },
            routed: true,
            detail_type: "OrderRendered",
            resources: &[],
            detail: Detail::Rendered,
        },
        Row {
            case: "the resources the event concerns",
            parameters: |_| {
                json!({
                    "EventBridgeEventBusParameters": {
                        "Source": MATCHED_SOURCE,
                        "DetailType": "OrderPlaced",
                        "Resources": ["arn:aws:s3:::orders", "$.eventSourceARN"],
                    }
                })
            },
            routed: true,
            detail_type: "OrderPlaced",
            resources: &["arn:aws:s3:::orders"],
            detail: Detail::Record,
        },
        Row {
            case: "a source no rule on the bus matches",
            parameters: |_| {
                json!({
                    "EventBridgeEventBusParameters": {
                        "Source": UNMATCHED_SOURCE,
                        "DetailType": "OrderPlaced",
                    }
                })
            },
            routed: false,
            detail_type: "OrderPlaced",
            resources: &[],
            detail: Detail::Record,
        },
    ];

    let name = "event-bus-target";
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{name}-src")).await;
    let (sink_url, sink_arn) = given_queue(&router, &format!("{name}-sink")).await;
    let bus_arn = given_bus_forwarding_to(&router, "the bus of the table", &sink_arn).await;
    allow_the_rule_to_send(&router, "the bus of the table", &sink_url, &sink_arn).await;
    let role = given_role_stating(
        &router,
        &format!("{name}-role"),
        SERVICE_PRINCIPAL,
        json!([
            {
                "Effect": "Allow",
                "Action": [
                    "sqs:ReceiveMessage",
                    "sqs:DeleteMessage",
                    "sqs:GetQueueAttributes",
                ],
                "Resource": source_arn,
            },
            { "Effect": "Allow", "Action": "events:PutEvents", "Resource": bus_arn },
        ]),
    )
    .await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let marker = format!("marker-{index}");
        // The first row CREATES the pipe and each later one re-installs its target
        // parameters on the same pipe, so exactly one poller ever reads the source queue.
        let installed = if index == 0 {
            call(
                &router,
                "POST",
                &format!("/v1/pipes/{name}"),
                Some(json!({
                    "Source": source_arn,
                    "Target": bus_arn,
                    "RoleArn": role,
                    "DesiredState": "RUNNING",
                    "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
                    "TargetParameters": (row.parameters)(&marker),
                })),
                REGION,
            )
            .await
        } else {
            call(
                &router,
                "PUT",
                &format!("/v1/pipes/{name}"),
                Some(json!({
                    "RoleArn": role,
                    "TargetParameters": (row.parameters)(&marker),
                })),
                REGION,
            )
            .await
        };
        assert_eq!(
            installed.status, 200,
            "case {case:?}: installing the target parameters answered {} {}",
            installed.status, installed.body
        );
        let state = wait_for_state(&router, name, RUNNING, BUDGET).await;
        assert_eq!(
            state, RUNNING,
            "case {case:?}: expected the pipe running under the parameters just installed, \
             got {state:?}"
        );

        let body = json!({ "kind": KIND, "marker": marker }).to_string();
        let (status, answer) = sqs(
            &router,
            "SendMessage",
            &json!({ "QueueUrl": source_url, "MessageBody": body }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SendMessage of {body:?} answered {status} {answer}"
        );

        if row.routed {
            let event = waited_for_event(&router, case, &sink_url, &marker).await;
            assert_eq!(
                event["source"].as_str(),
                Some(MATCHED_SOURCE),
                "case {case:?}: expected the configured source, got {event}"
            );
            assert_eq!(
                event["detail-type"].as_str(),
                Some(row.detail_type),
                "case {case:?}: expected the detail type {:?}, got {event}",
                row.detail_type
            );
            let carried: Vec<&str> = event["resources"]
                .as_array()
                .map(|list| list.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            // A configured resource that is a path into the record contributes the record's
            // own value, so the row states the literal ones and the path is asserted next.
            for resource in row.resources {
                assert!(
                    carried.contains(resource),
                    "case {case:?}: expected the resource {resource:?} among {carried:?}"
                );
            }
            if !row.resources.is_empty() {
                assert!(
                    carried.contains(&source_arn.as_str()),
                    "case {case:?}: expected the resource path to contribute the record's \
                     own source queue {source_arn:?}, got {carried:?}"
                );
            }
            match row.detail {
                Detail::Record => {
                    let detail = &event["detail"];
                    assert_eq!(
                        detail["body"].as_str(),
                        Some(body.as_str()),
                        "case {case:?}: expected the record's body as the event detail, got \
                         {event}"
                    );
                    assert_eq!(
                        detail["eventSourceARN"].as_str(),
                        Some(source_arn.as_str()),
                        "case {case:?}: expected the source the record came off, got {event}"
                    );
                    assert!(
                        detail["messageId"]
                            .as_str()
                            .is_some_and(|id| !id.is_empty()),
                        "case {case:?}: expected the record's message id, got {event}"
                    );
                }
                Detail::Rendered => {
                    assert_eq!(
                        event["detail"]["ordered"].as_str(),
                        Some(marker.as_str()),
                        "case {case:?}: expected the template's render as the event detail, \
                         got {event}"
                    );
                    assert!(
                        event["detail"].get("body").is_none(),
                        "case {case:?}: a rendered detail carries nothing of the envelope, \
                         got {event}"
                    );
                }
            }
        } else {
            let seen = carrying(&collect_during(&router, &sink_url, QUIET).await, &marker);
            assert!(
                seen.is_empty(),
                "case {case:?}: expected no event at the sink for a source the rule does \
                 not match, got {seen:?}"
            );
        }

        // Published or merely unrouted, the record was taken by the pipe: it is the bus that
        // decides which rules fire, so the pipe is finished with the record either way.
        let left =
            wait_for_counted(&router, &source_url, &HELD_ATTRIBUTES, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: expected the record consumed by the pipe, the source holds {left}"
        );
    }
}

/// RULE: a bus delivery is made AS THE PIPE'S EXECUTION ROLE, so a role that may not call
/// `events:PutEvents` on the configured bus is an event that is never published — and the
/// record stays on its source, to be handed over again once the grant is in force.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_role_without_put_events_publishes_nothing_and_leaves_the_record() {
    let case = "a role granted the source reads and nothing on the bus";
    let name = "event-bus-denied";
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{name}-src")).await;
    let (sink_url, sink_arn) = given_queue(&router, &format!("{name}-sink")).await;
    let bus_arn = given_bus_forwarding_to(&router, case, &sink_arn).await;
    allow_the_rule_to_send(&router, case, &sink_url, &sink_arn).await;
    let role = given_role_stating(
        &router,
        &format!("{name}-role"),
        SERVICE_PRINCIPAL,
        json!([{
            "Effect": "Allow",
            "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
            "Resource": source_arn,
        }]),
    )
    .await;

    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{name}"),
        Some(json!({
            "Source": source_arn,
            "Target": bus_arn,
            "RoleArn": role,
            "DesiredState": "RUNNING",
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
            "TargetParameters": {
                "EventBridgeEventBusParameters": {
                    "Source": MATCHED_SOURCE,
                    "DetailType": "OrderPlaced",
                }
            },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );
    let state = wait_for_state(&router, name, RUNNING, BUDGET).await;
    assert_eq!(
        state, RUNNING,
        "case {case:?}: expected the pipe to come up, got {state:?}"
    );

    let marker = "withheld";
    let body = json!({ "kind": KIND, "marker": marker }).to_string();
    let (status, answer) = sqs(
        &router,
        "SendMessage",
        &json!({ "QueueUrl": source_url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage answered {status} {answer}"
    );

    let seen = carrying(&collect_during(&router, &sink_url, QUIET).await, marker);
    assert!(
        seen.is_empty(),
        "case {case:?}: expected nothing published while the role may not put events, got \
         {seen:?}"
    );
    let held = counted(&router, &source_url, &HELD_ATTRIBUTES).await;
    assert_eq!(
        held, 1,
        "case {case:?}: expected the withheld record left on the source, it holds {held}"
    );
}

/// The event carrying `marker` that reached the queue at `url`, waited for until it is there
/// or [`BUDGET`] has elapsed.
///
/// Every body the sink hands over is looked at, because the sink holds what the earlier rows
/// published too: the one this row is about is the one carrying its own marker.
async fn waited_for_event(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    marker: &str,
) -> Value {
    let deadline = std::time::Instant::now() + BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        seen.extend(collect_at_least(router, url, 1, crate::pipe_data_plane::LOOK_AGAIN).await);
        if let Some(first) = carrying(&seen, marker).first() {
            return serde_json::from_str(first).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivered body is not JSON ({error}): {first:?}")
            });
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: no event carrying {marker:?} reached the sink, saw {seen:?}"
        );
        tokio::time::sleep(crate::pipe_data_plane::LOOK_AGAIN).await;
    }
}

/// The bodies of `bodies` that carry `marker`: one row's own deliveries, told apart from
/// another's by the marker the record it was derived from carried.
fn carrying(bodies: &[String], marker: &str) -> Vec<String> {
    bodies
        .iter()
        .filter(|body| body.contains(marker))
        .cloned()
        .collect()
}

