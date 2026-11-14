// SPDX-License-Identifier: CC0-1.0

//! What Amazon RDS PUBLISHES onto the account's default EventBridge bus: every lifecycle
//! change it records for an event source is also an `aws.rds` event, delivered to each
//! target of every rule whose pattern matches it.
//!
//! The rule under test: a lifecycle change this service records for an event source
//! ([`service_com_amazonaws_rds_amazonrdsv19::state::events`], the log `DescribeEvents`
//! reports) is published onto the DEFAULT event bus of the changed resource's Region, with
//! `source` `aws.rds`, the `detail-type` of that resource's SOURCE TYPE, the changed
//! resource's ARN as the one entry of `resources`, and a `detail` carrying that change's
//! published event identifier, message, category, source type, source identifier, source ARN
//! and date — the same record the control plane reports, down to its `Date`. A rule on that
//! bus then delivers the event to EACH of its targets independently: a raw target receives
//! the whole envelope and a target carrying an `InputTransformer` receives only what its
//! template renders.
//!
//! The table below spans what the rule generalises over: every lifecycle change this service
//! records for a DB instance (its creation, a restart, a stop, a start), a change to a
//! resource of ANOTHER source type that publishes its own detail-type and its own ARN
//! resource type (a manual DB snapshot, a DB cluster), and so three source types, four
//! categories and six event identifiers. The restart row is the one a reboot publishes; the
//! other five are changes this suite drives through their own operations, so a mapping that
//! only held for one transition could not pass.
//!
//! The differential leg is a SECOND rule whose pattern pins `detail.EventID` and
//! `detail.SourceArn` to the restart of this one instance: its queue must receive that event
//! and none of the other five, so the publication is observably matched on its detail rather
//! than delivered to everything.
//!
//! EventBridge and Amazon SQS are mounted beside this service on ONE engine, exactly as the
//! assembled router mounts them, and every assertion reads a message a queue really holds —
//! so a publication that was not made, or not matched, or not fanned out to both targets,
//! fails here.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use crate::db_instances::{call, expect_ok, subtrees, text, REGION};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// How long the suite waits for a delivery before calling it absent.
///
/// Generous next to the work a publication does (two in-process calls) and next to the span
/// a status transition this service owes becomes due after, so a loaded test machine cannot
/// fail the suite.
const DELIVERY_BUDGET: Duration = Duration::from_secs(60);

/// How long a status poll loop is given before the suite calls a transition unobservable.
const POLL_BUDGET: Duration = Duration::from_secs(30);

/// The gap between two reads of the same loop.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The DB instance every instance-level change below is made to.
const INSTANCE: &str = "emu-bus-instance";

/// The manual snapshot taken of it.
const SNAPSHOT: &str = "emu-bus-snapshot";

/// The DB cluster created below.
const CLUSTER: &str = "emu-bus-cluster";

/// The queue the rule's RAW target delivers to.
const RAW_QUEUE: &str = "emu-bus-raw";

/// The queue the rule's `InputTransformer` target delivers to.
const TRANSFORMED_QUEUE: &str = "emu-bus-transformed";

/// The queue the differential rule — the one pinning `detail.EventID` — delivers to.
const RESTART_ONLY_QUEUE: &str = "emu-bus-restart-only";

/// The `source` every event Amazon RDS publishes carries.
const EVENT_SOURCE: &str = "aws.rds";

/// The `detail-type` of a DB INSTANCE event, and of the two sibling source types this suite
/// also drives a change to.
///
/// Source: Amazon RDS publishes one detail-type per event source type, spelled
/// `RDS <the source type, in words> Event`.
const DB_INSTANCE_EVENT: &str = "RDS DB Instance Event";
const DB_SNAPSHOT_EVENT: &str = "RDS DB Snapshot Event";
const DB_CLUSTER_EVENT: &str = "RDS DB Cluster Event";

/// One expected publication: the change, and the envelope it must be published as.
struct Published {
    case: &'static str,
    /// `detail.SourceIdentifier` — the identifier of the resource that changed.
    source_identifier: &'static str,
    /// The ARN resource type of that resource: `db`, `snapshot` or `cluster`.
    resource_type: &'static str,
    /// The `detail-type` of the envelope.
    detail_type: &'static str,
    /// `detail.EventID`.
    event_id: &'static str,
    /// `detail.Message`.
    message: &'static str,
    /// The one entry of `detail.EventCategories`.
    category: &'static str,
    /// `detail.SourceType`.
    detail_source_type: &'static str,
}

impl Published {
    /// The ARN of the resource this change is about, as this service's own records compute
    /// it: `arn:aws:rds:<region>:<account>:<resource type>:<name>`.
    fn source_arn(&self) -> String {
        format!(
            "arn:aws:rds:{REGION}:{ACCOUNT}:{}:{}",
            self.resource_type, self.source_identifier
        )
    }

    /// What the `InputTransformer` target must receive for this change: the template
    /// `"<db> :: <msg>"` filled with `detail.SourceIdentifier` and `detail.Message`, whose
    /// own quotes are delivered as written.
    fn rendered(&self) -> String {
        format!("\"{} :: {}\"", self.source_identifier, self.message)
    }
}

/// Every change this suite drives, and the envelope each must publish.
///
/// Source: the per-source-type tables of "Amazon RDS event categories and event messages"
/// (Amazon RDS User Guide), one row of which states the source type, the event identifier,
/// the category and the message of each change. The restart row (`RDS-EVENT-0006`) is the
/// one a `RebootDBInstance` publishes; the other five are the rows for the creation, the
/// stop and the start of a DB instance, the start of a manual DB snapshot and the creation
/// of a DB cluster.
const EXPECTED: &[Published] = &[
    Published {
        case: "the DB instance was created",
        source_identifier: INSTANCE,
        resource_type: "db",
        detail_type: DB_INSTANCE_EVENT,
        event_id: "RDS-EVENT-0005",
        message: "DB instance created",
        category: "creation",
        detail_source_type: "DB_INSTANCE",
    },
    Published {
        case: "a manual snapshot was taken of it",
        source_identifier: SNAPSHOT,
        resource_type: "snapshot",
        detail_type: DB_SNAPSHOT_EVENT,
        event_id: "RDS-EVENT-0040",
        message: "Creating manual snapshot",
        category: "creation",
        detail_source_type: "DB_SNAPSHOT",
    },
    Published {
        case: "the DB instance was restarted",
        source_identifier: INSTANCE,
        resource_type: "db",
        detail_type: DB_INSTANCE_EVENT,
        event_id: "RDS-EVENT-0006",
        message: "DB instance restarted",
        category: "availability",
        detail_source_type: "DB_INSTANCE",
    },
    Published {
        case: "the DB instance was stopped",
        source_identifier: INSTANCE,
        resource_type: "db",
        detail_type: DB_INSTANCE_EVENT,
        event_id: "RDS-EVENT-0087",
        message: "DB instance stopped",
        category: "notification",
        detail_source_type: "DB_INSTANCE",
    },
    Published {
        case: "the DB instance was started again",
        source_identifier: INSTANCE,
        resource_type: "db",
        detail_type: DB_INSTANCE_EVENT,
        event_id: "RDS-EVENT-0088",
        message: "DB instance started",
        category: "notification",
        detail_source_type: "DB_INSTANCE",
    },
    Published {
        case: "the DB cluster was created",
        source_identifier: CLUSTER,
        resource_type: "cluster",
        detail_type: DB_CLUSTER_EVENT,
        event_id: "RDS-EVENT-0170",
        message: "DB cluster created",
        category: "creation",
        detail_source_type: "DB_CLUSTER",
    },
];

/// The restart row, which the differential rule's pattern pins and the control-plane
/// cross-check reads back.
fn the_restart() -> &'static Published {
    EXPECTED
        .iter()
        .find(|row| row.event_id == "RDS-EVENT-0006")
        .expect("the table carries the restart row")
}

/// A router carrying this service beside the two a published event travels through —
/// EventBridge (the default bus and its rules) and Amazon SQS (the targets) — all over ONE
/// shared engine, exactly as the assembled router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests sign
    // nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One RPC call (`POST /` carrying `X-Amz-Target`) against the service `signing_name`,
/// asserted to answer 200 and parsed as the JSON it carries.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {answered}"
    );
    serde_json::from_str(&answered).unwrap_or(Value::Null)
}

/// An EventBridge call (`awsJson1_1`).
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    rpc(router, "events", "AWSEvents", "1.1", operation, body).await
}

/// An Amazon SQS call (`awsJson1_0`).
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// The ARN of the queue named `name` in this account.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
}

/// Create the queue named `name` and grant EventBridge the send it delivers with.
///
/// EventBridge reaches a queue as its own SERVICE PRINCIPAL and relies on the queue's
/// resource-based policy, so a queue that granted it nothing receives nothing.
async fn create_queue(router: &MultiServiceRouter, name: &str) -> String {
    let created = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue must answer a QueueUrl, got {created}"))
        .to_string();
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn(name),
        }],
    });
    sqs(
        router,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": document.to_string() } }),
    )
    .await;
    url
}

/// Every message on the queue right now, as the raw body text each carries.
async fn drain(router: &MultiServiceRouter, queue_url: &str) -> Vec<String> {
    let received = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Read the queue until it has handed over `wanted` messages, or [`DELIVERY_BUDGET`] passes.
///
/// A poll loop rather than a wait: the condition is how many messages have arrived, and the
/// deadline is only there so a missing delivery fails instead of hanging.
async fn collect(router: &MultiServiceRouter, queue_url: &str, wanted: usize) -> Vec<String> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut bodies: Vec<String> = Vec::new();
    loop {
        bodies.extend(drain(router, queue_url).await);
        if bodies.len() >= wanted || Instant::now() >= deadline {
            return bodies;
        }
        sleep(POLL_INTERVAL).await;
    }
}

/// Poll `DescribeDBInstances` until the instance reports `status`.
async fn wait_for_status(router: &MultiServiceRouter, identifier: &str, status: &str) {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let body = expect_ok(
            identifier,
            call(
                router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await,
        );
        let reported = subtrees(&body, "DBInstance")
            .into_iter()
            .next()
            .unwrap_or_default();
        let reported_status = text(&reported, "DBInstanceStatus");
        if reported_status == status {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {identifier:?}: expected the status {status:?} within {POLL_BUDGET:?}, \
             got {reported_status:?}"
        );
        sleep(POLL_INTERVAL).await;
    }
}

/// Make every change [`EXPECTED`] names, in that order, over the wire.
async fn drive_the_lifecycle(router: &MultiServiceRouter) {
    expect_ok(
        "create the instance",
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", INSTANCE),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.t4g.micro"),
            ],
        )
        .await,
    );
    wait_for_status(router, INSTANCE, "available").await;
    expect_ok(
        "snapshot the instance",
        call(
            router,
            "CreateDBSnapshot",
            &[
                ("DBSnapshotIdentifier", SNAPSHOT),
                ("DBInstanceIdentifier", INSTANCE),
            ],
        )
        .await,
    );
    expect_ok(
        "reboot the instance",
        call(
            router,
            "RebootDBInstance",
            &[("DBInstanceIdentifier", INSTANCE)],
        )
        .await,
    );
    wait_for_status(router, INSTANCE, "available").await;
    expect_ok(
        "stop the instance",
        call(
            router,
            "StopDBInstance",
            &[("DBInstanceIdentifier", INSTANCE)],
        )
        .await,
    );
    wait_for_status(router, INSTANCE, "stopped").await;
    expect_ok(
        "start the instance",
        call(
            router,
            "StartDBInstance",
            &[("DBInstanceIdentifier", INSTANCE)],
        )
        .await,
    );
    wait_for_status(router, INSTANCE, "available").await;
    expect_ok(
        "create the cluster",
        call(
            router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", CLUSTER),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
}

/// The rule every `aws.rds` event of this account matches, with a RAW target and an
/// `InputTransformer` sibling on the same rule — the two legs a delivery must fan out to
/// independently.
async fn rule_on_every_rds_event(router: &MultiServiceRouter, raw: &str, transformed: &str) {
    events(
        router,
        "PutRule",
        &json!({
            "Name": "emu-bus-every-rds-event",
            "EventPattern": json!({ "source": [EVENT_SOURCE] }).to_string(),
        }),
    )
    .await;
    let targeted = events(
        router,
        "PutTargets",
        &json!({
            "Rule": "emu-bus-every-rds-event",
            "Targets": [
                { "Id": "raw", "Arn": queue_arn(raw) },
                {
                    "Id": "transformed",
                    "Arn": queue_arn(transformed),
                    "InputTransformer": {
                        "InputPathsMap": {
                            "db": "$.detail.SourceIdentifier",
                            "msg": "$.detail.Message",
                        },
                        "InputTemplate": "\"<db> :: <msg>\"",
                    },
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "both targets must be accepted, got {targeted}"
    );
}

/// The differential rule: it matches the restart of THIS instance alone, by the published
/// event identifier and the instance's own ARN.
async fn rule_on_the_restart_alone(router: &MultiServiceRouter, queue: &str) {
    let restart = the_restart();
    events(
        router,
        "PutRule",
        &json!({
            "Name": "emu-bus-restart-only",
            "EventPattern": json!({
                "source": [EVENT_SOURCE],
                "detail-type": [restart.detail_type],
                "detail": {
                    "EventID": [restart.event_id],
                    "SourceArn": [restart.source_arn()],
                },
            })
            .to_string(),
        }),
    )
    .await;
    let targeted = events(
        router,
        "PutTargets",
        &json!({
            "Rule": "emu-bus-restart-only",
            "Targets": [{ "Id": "the-queue", "Arn": queue_arn(queue) }],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "the differential target must be accepted, got {targeted}"
    );
}

/// The envelope whose `detail.EventID` is `event_id` and whose `detail.SourceIdentifier` is
/// `source_identifier`, out of the bodies a queue handed over.
fn envelope_of<'a>(
    bodies: &'a [String],
    event_id: &str,
    source_identifier: &str,
) -> Option<&'a str> {
    bodies.iter().map(String::as_str).find(|body| {
        serde_json::from_str::<Value>(body).is_ok_and(|event| {
            event["detail"]["EventID"] == json!(event_id)
                && event["detail"]["SourceIdentifier"] == json!(source_identifier)
        })
    })
}

/// The instant `text` names, read with the same reader an SDK reads a date-time member with.
fn instant_of(case: &str, text: &str) -> f64 {
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {text:?} must be an RFC 3339 instant, got {error}")
        })
        .as_secs_f64()
}

#[tokio::test]
async fn every_recorded_lifecycle_change_is_published_onto_the_default_bus() {
    let router = emulator();
    let raw_url = create_queue(&router, RAW_QUEUE).await;
    let transformed_url = create_queue(&router, TRANSFORMED_QUEUE).await;
    rule_on_every_rds_event(&router, RAW_QUEUE, TRANSFORMED_QUEUE).await;

    drive_the_lifecycle(&router).await;

    let delivered = collect(&router, &raw_url, EXPECTED.len()).await;
    assert_eq!(
        delivered.len(),
        EXPECTED.len(),
        "the raw target receives one envelope per recorded change, got {delivered:?}"
    );
    for row in EXPECTED {
        let body = envelope_of(&delivered, row.event_id, row.source_identifier)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: no envelope carrying {} for {} reached the raw target, got \
                     {delivered:?}",
                    row.case, row.event_id, row.source_identifier
                )
            });
        let event: Value = serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("case {:?}: {body} must be JSON: {error}", row.case));
        assert_eq!(
            event["source"],
            json!(EVENT_SOURCE),
            "case {:?}: expected the source {EVENT_SOURCE}, got {event}",
            row.case
        );
        assert_eq!(
            event["detail-type"],
            json!(row.detail_type),
            "case {:?}: expected the detail-type {}, got {event}",
            row.case,
            row.detail_type
        );
        assert_eq!(
            event["account"],
            json!(ACCOUNT),
            "case {:?}: expected the caller's account {ACCOUNT}, got {event}",
            row.case
        );
        assert_eq!(
            event["region"],
            json!(REGION),
            "case {:?}: expected the request's Region {REGION}, got {event}",
            row.case
        );
        assert_eq!(
            event["resources"],
            json!([row.source_arn()]),
            "case {:?}: expected the changed resource's ARN {:?} as the one entry of \
             resources, got {event}",
            row.case,
            row.source_arn()
        );
        assert_eq!(
            event["detail"]["Message"],
            json!(row.message),
            "case {:?}: expected the message {:?}, got {event}",
            row.case,
            row.message
        );
        assert_eq!(
            event["detail"]["EventCategories"],
            json!([row.category]),
            "case {:?}: expected the category {:?}, got {event}",
            row.case,
            row.category
        );
        assert_eq!(
            event["detail"]["SourceType"],
            json!(row.detail_source_type),
            "case {:?}: expected the source type {:?}, got {event}",
            row.case,
            row.detail_source_type
        );
        assert_eq!(
            event["detail"]["SourceArn"],
            json!(row.source_arn()),
            "case {:?}: expected the source ARN {:?}, got {event}",
            row.case,
            row.source_arn()
        );
        let date = event["detail"]["Date"].as_str().unwrap_or_default();
        assert!(
            instant_of(row.case, date) > 0.0,
            "case {:?}: expected a dated event, got {event}",
            row.case
        );
    }

    // The sibling target on the SAME rule is handed only what its template rendered —
    // nothing of the envelope the raw target received.
    let rendered = collect(&router, &transformed_url, EXPECTED.len()).await;
    let got: BTreeSet<String> = rendered.iter().cloned().collect();
    let want: BTreeSet<String> = EXPECTED.iter().map(Published::rendered).collect();
    assert_eq!(
        got, want,
        "the InputTransformer target receives exactly its rendered template per event, \
         expected {want:?}, got {got:?}"
    );
}

#[tokio::test]
async fn a_rule_pinning_the_event_identifier_receives_that_change_alone() {
    let router = emulator();
    let only_url = create_queue(&router, RESTART_ONLY_QUEUE).await;
    rule_on_the_restart_alone(&router, RESTART_ONLY_QUEUE).await;

    drive_the_lifecycle(&router).await;

    let restart = the_restart();
    let delivered = collect(&router, &only_url, 1).await;
    assert_eq!(
        delivered.len(),
        1,
        "the rule pinning {} receives exactly the restart, got {delivered:?}",
        restart.event_id
    );
    let event: Value = serde_json::from_str(&delivered[0])
        .unwrap_or_else(|error| panic!("the delivered body must be JSON: {error}"));
    assert_eq!(
        event["detail"]["EventID"],
        json!(restart.event_id),
        "expected {}, got {event}",
        restart.event_id
    );

    // And the SAME change is readable from the control plane, with the same Date: the
    // envelope is rendered from the record `DescribeEvents` reports.
    let body = expect_ok(
        "the restart in the event log",
        call(
            &router,
            "DescribeEvents",
            &[
                ("SourceType", "db-instance"),
                ("SourceIdentifier", INSTANCE),
                ("EventCategories.EventCategory.1", restart.category),
            ],
        )
        .await,
    );
    let reported = subtrees(&body, "Event");
    assert_eq!(
        reported.len(),
        1,
        "the log reports the one restart of {INSTANCE}, got {body}"
    );
    let reported = &reported[0];
    assert_eq!(
        text(reported, "Message"),
        restart.message,
        "expected the message {:?}, got {body}",
        restart.message
    );
    assert_eq!(
        text(reported, "SourceArn"),
        restart.source_arn(),
        "expected the source ARN {:?}, got {body}",
        restart.source_arn()
    );
    let published = event["detail"]["Date"].as_str().unwrap_or_default();
    assert_eq!(
        instant_of("the published date", published),
        instant_of("the reported date", &text(reported, "Date")),
        "the published event and the reported one carry the same Date, got {published:?} \
         against {body}"
    );
}
