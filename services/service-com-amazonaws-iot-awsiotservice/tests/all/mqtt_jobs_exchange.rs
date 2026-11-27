// SPDX-License-Identifier: CC0-1.0

//! The JOBS exchange a device runs on the broker's reserved MQTT topics, driven the whole way
//! a device drives it: the job is created through the control plane, the address is discovered,
//! and every request and answer is spoken on a real connection to it.
//!
//! RULE: a device's work reaches it on the reserved topics of its own thing,
//! `$aws/things/{thingName}/jobs/…`, and over the SAME execution store the HTTP jobs plane
//! serves — `get` answers the pending executions, `{jobId}/get` (with `$next` naming the
//! device's next pending execution) answers one execution and the job DOCUMENT it has to run,
//! `start-next` hands out the next pending execution and leaves it `IN_PROGRESS`, and
//! `{jobId}/update` records the status the device reports, which is what the control plane's
//! `DescribeJobExecution` then reports. A request that cannot be carried out is answered on
//! that topic's own `rejected` topic with a `code` and a sentence, never left silent; and the
//! service announces a device's next execution on `notify-next` when one is enqueued for it.
//!
//! The rows differ along what the rule generalises over: each exchange of the reserved
//! namespace, the `$next` label beside a job named by id, a device with work and one with
//! none, and every class of request the exchange refuses.
//!
//! The device harness, the published address and the emulator assembly are the sibling
//! suite's (`tests/all/data_endpoint_mqtt_transport.rs`), shared rather than copied.
//!
//! Nothing here waits on a span: every request is published at QoS 1 and answered by the
//! broker's own PUBACK, and each answer is read off the subscriber's connection under the
//! sibling suite's generous packet deadline.

use serde_json::{json, Value};

use crate::data_endpoint_mqtt_transport::{emulator, Device, Emulator, Transport};

/// The thing the device below is registered as.
const THING_NAME: &str = "mqtt-jobs-device";

/// The job the device runs, and the document it carries.
const JOB_ID: &str = "mqtt-jobs-rollout";
/// See [`JOB_ID`].
const JOB_DOCUMENT: &str = r#"{"operation":"install","version":"2.0.1"}"#;

/// A second job, so a device holds more than one execution and the `$next` label has to choose.
const SECOND_JOB_ID: &str = "mqtt-jobs-followup";
/// See [`SECOND_JOB_ID`].
const SECOND_JOB_DOCUMENT: &str = r#"{"operation":"reboot"}"#;

/// One control-plane call that must be served, answered with its document.
async fn control_plane(
    emulator: &Emulator,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let (status, rendered) = emulator
        .send(
            method,
            path,
            "iot",
            "application/json",
            &[],
            body.map(|body| body.to_string().into_bytes())
                .unwrap_or_default(),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: {method} {path} answered {status} with {rendered}"
    );
    serde_json::from_str(&rendered).unwrap_or(Value::Null)
}

/// Register the thing the device connects as, answering its ARN — which is what a job targets.
async fn given_thing(emulator: &Emulator) -> String {
    let created = control_plane(
        emulator,
        "POST",
        &format!("/things/{THING_NAME}"),
        Some(json!({})),
    )
    .await;
    created
        .get("thingArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: CreateThing reported no thingArn: {created}"))
        .to_string()
}

/// Create the job `job_id` targeting `thing_arn` with `document`: the call that enqueues the
/// device's execution.
async fn given_job(emulator: &Emulator, job_id: &str, thing_arn: &str, document: &str) {
    control_plane(
        emulator,
        "PUT",
        &format!("/jobs/{job_id}"),
        Some(json!({ "targets": [thing_arn], "document": document })),
    )
    .await;
}

/// The reserved topic of one exchange of this device.
fn jobs_topic(levels: &str) -> String {
    format!("$aws/things/{THING_NAME}/jobs/{levels}")
}

/// What the device subscribes to: every topic the service ANSWERS on, and none of the request
/// topics — a session subscribed to its own request topics would be handed its own
/// publications, which says nothing about the exchange.
fn answer_filters() -> Vec<(String, u8)> {
    [
        jobs_topic("notify-next"),
        jobs_topic("get/accepted"),
        jobs_topic("get/rejected"),
        jobs_topic("start-next/accepted"),
        jobs_topic("start-next/rejected"),
        jobs_topic("+/get/accepted"),
        jobs_topic("+/get/rejected"),
        jobs_topic("+/update/accepted"),
        jobs_topic("+/update/rejected"),
    ]
    .into_iter()
    .map(|filter| (filter, 1))
    .collect()
}

/// Subscribe the device to every answer topic, asserting each filter was granted QoS 1.
async fn subscribe_to_answers(device: &mut Device) {
    let filters = answer_filters();
    let borrowed: Vec<(&str, u8)> = filters
        .iter()
        .map(|(filter, qos)| (filter.as_str(), *qos))
        .collect();
    let granted = device.subscribe(&borrowed).await;
    assert_eq!(
        granted,
        vec![1; borrowed.len()],
        "the device's answer subscriptions must be granted QoS 1, got {granted:?}"
    );
}

/// Publish one request on a reserved jobs topic at QoS 1 — so the broker's PUBACK says it has
/// served the exchange — and answer the message the service answered with: its topic and its
/// body.
async fn request(device: &mut Device, levels: &str, body: &Value) -> (String, Value) {
    let topic = jobs_topic(levels);
    device
        .publish_at(&topic, body.to_string().as_bytes(), 1, false)
        .await;
    answered(device).await
}

/// The next message the service wrote to this device, as its topic and parsed body.
async fn answered(device: &mut Device) -> (String, Value) {
    let delivered = device.next_delivery().await;
    let body: Value = serde_json::from_slice(&delivered.payload).unwrap_or_else(|error| {
        panic!(
            "the answer on {} is not JSON ({error}): {:?}",
            delivered.topic,
            String::from_utf8_lossy(&delivered.payload)
        )
    });
    (delivered.topic, body)
}

/// The `execution` member of one answer.
fn execution_of(case: &str, body: &Value) -> Value {
    body.get("execution")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no execution in {body}"))
}

/// One string member of a document.
fn text(case: &str, document: &Value, member: &str) -> String {
    document
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {document}"))
        .to_string()
}

/// The `jobDocument` one execution carries, which these topics carry as the DOCUMENT itself:
/// a device reads its members straight out of the message.
fn document_of(case: &str, execution: &Value) -> Value {
    execution
        .get("jobDocument")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no jobDocument in {execution}"))
}

/// The document a job was created with, as the value a device reads out of its message.
fn document(text: &str) -> Value {
    serde_json::from_str(text).expect("the suite's job documents are JSON")
}

/// The whole life of one job as a device runs it over MQTT: it is announced, asked for, taken
/// up and reported finished — and the control plane reports the device's own report.
#[tokio::test]
async fn a_device_is_handed_its_job_document_and_its_report_is_recorded() {
    let emulator = emulator();
    let thing_arn = given_thing(&emulator).await;
    let address = emulator.discover().await;

    let mut device = Device::connect(Transport::Clear, &address, "jobs-lifecycle").await;
    subscribe_to_answers(&mut device).await;

    // Creating the job enqueues this device's execution, which is the moment its next work
    // changes: the service announces it on the device's own `notify-next` topic.
    given_job(&emulator, JOB_ID, &thing_arn, JOB_DOCUMENT).await;
    let (topic, announced) = answered(&mut device).await;
    assert_eq!(
        topic,
        jobs_topic("notify-next"),
        "case the announcement: expected notify-next, got {topic} with {announced}"
    );
    let execution = execution_of("the announcement", &announced);
    assert_eq!(
        text("the announcement", &execution, "jobId"),
        JOB_ID,
        "case the announcement: expected the created job, got {execution}"
    );
    assert_eq!(
        document_of("the announcement", &execution),
        document(JOB_DOCUMENT),
        "case the announcement: expected the job's own document, got {execution}"
    );
    assert_eq!(
        text("the announcement", &execution, "status"),
        "QUEUED",
        "case the announcement: a job just enqueued is QUEUED, got {execution}"
    );

    // `$next/get` — the device asks for the work it has to run, by the label that names
    // whichever execution is next.
    let (topic, answer) = request(
        &mut device,
        "$next/get",
        &json!({ "clientToken": "token-1" }),
    )
    .await;
    assert_eq!(
        topic,
        jobs_topic("$next/get/accepted"),
        "case $next/get: expected the accepted answer, got {topic} with {answer}"
    );
    assert_eq!(
        text("$next/get", &answer, "clientToken"),
        "token-1",
        "case $next/get: the client token must be echoed, got {answer}"
    );
    let execution = execution_of("$next/get", &answer);
    assert_eq!(
        text("$next/get", &execution, "jobId"),
        JOB_ID,
        "case $next/get: expected the device's next job, got {execution}"
    );
    assert_eq!(
        document_of("$next/get", &execution),
        document(JOB_DOCUMENT),
        "case $next/get: expected the job document, got {execution}"
    );
    assert_eq!(
        text("$next/get", &execution, "thingName"),
        THING_NAME,
        "case $next/get: expected the device the execution belongs to, got {execution}"
    );

    // The device takes the work up, asking for the state its report left behind.
    let (topic, answer) = request(
        &mut device,
        &format!("{JOB_ID}/update"),
        &json!({ "status": "IN_PROGRESS", "includeJobExecutionState": true }),
    )
    .await;
    assert_eq!(
        topic,
        jobs_topic(&format!("{JOB_ID}/update/accepted")),
        "case the IN_PROGRESS report: expected the accepted answer, got {topic} with {answer}"
    );
    let state = answer
        .get("executionState")
        .cloned()
        .unwrap_or_else(|| panic!("case the IN_PROGRESS report: no executionState in {answer}"));
    assert_eq!(
        text("the IN_PROGRESS report", &state, "status"),
        "IN_PROGRESS",
        "case the IN_PROGRESS report: expected the reported status, got {state}"
    );

    // ... and reports it finished, with details of its own.
    let (topic, answer) = request(
        &mut device,
        &format!("{JOB_ID}/update"),
        &json!({
            "status": "SUCCEEDED",
            "statusDetails": { "progress": "100" },
            "includeJobExecutionState": true,
        }),
    )
    .await;
    assert_eq!(
        topic,
        jobs_topic(&format!("{JOB_ID}/update/accepted")),
        "case the SUCCEEDED report: expected the accepted answer, got {topic} with {answer}"
    );

    // What the device reported is what a client reads through the control plane: ONE execution
    // store, two ways in.
    let read = control_plane(
        &emulator,
        "GET",
        &format!("/things/{THING_NAME}/jobs/{JOB_ID}"),
        None,
    )
    .await;
    let reported = read
        .get("execution")
        .cloned()
        .unwrap_or_else(|| panic!("case the control-plane read: no execution in {read}"));
    assert_eq!(
        text("the control-plane read", &reported, "status"),
        "SUCCEEDED",
        "case the control-plane read: expected the device's own report, got {reported}"
    );
    // The control plane's own `JobExecutionStatusDetails` carries the pairs under its
    // `detailsMap` member, which is where the device's own details are read back from.
    assert_eq!(
        reported
            .get("statusDetails")
            .and_then(|details| details.get("detailsMap"))
            .and_then(Value::as_object)
            .and_then(|pairs| pairs.get("progress"))
            .and_then(Value::as_str),
        Some("100"),
        "case the control-plane read: expected the reported details, got {reported}"
    );
}

/// The listing exchange and the start exchange: what a device with two executions is told it
/// has to do, what starting hands it, and what a device with nothing pending is told.
#[tokio::test]
async fn the_pending_listing_and_the_start_exchange_serve_the_devices_own_queue() {
    let emulator = emulator();
    let thing_arn = given_thing(&emulator).await;
    let address = emulator.discover().await;

    let mut device = Device::connect(Transport::Clear, &address, "jobs-queue").await;
    subscribe_to_answers(&mut device).await;

    // A device with NOTHING pending: its queue is empty and starting hands it no execution at
    // all, which is not a refusal.
    let (topic, answer) = request(&mut device, "get", &json!({})).await;
    assert_eq!(
        topic,
        jobs_topic("get/accepted"),
        "case an empty queue: expected the accepted answer, got {topic} with {answer}"
    );
    for member in ["inProgressJobs", "queuedJobs"] {
        assert_eq!(
            answer.get(member).and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case an empty queue: expected no {member}, got {answer}"
        );
    }
    let (topic, answer) = request(&mut device, "start-next", &json!({})).await;
    assert_eq!(
        topic,
        jobs_topic("start-next/accepted"),
        "case starting with nothing pending: expected the accepted answer, got {topic} with \
         {answer}"
    );
    assert!(
        answer.get("execution").is_none(),
        "case starting with nothing pending: expected no execution, got {answer}"
    );

    // Two jobs, enqueued in order: each announces itself, and the announcements are read off
    // the connection before the exchanges below.
    given_job(&emulator, JOB_ID, &thing_arn, JOB_DOCUMENT).await;
    let (topic, _) = answered(&mut device).await;
    assert_eq!(
        topic,
        jobs_topic("notify-next"),
        "case the first job: expected its announcement, got {topic}"
    );
    given_job(&emulator, SECOND_JOB_ID, &thing_arn, SECOND_JOB_DOCUMENT).await;
    let (topic, _) = answered(&mut device).await;
    assert_eq!(
        topic,
        jobs_topic("notify-next"),
        "case the second job: expected its announcement, got {topic}"
    );

    // The listing reports both as QUEUED, oldest first, with none in progress.
    let (_topic, answer) = request(&mut device, "get", &json!({})).await;
    let queued: Vec<String> = answer
        .get("queuedJobs")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| text("the two-job listing", summary, "jobId"))
        .collect();
    assert_eq!(
        queued,
        vec![JOB_ID.to_string(), SECOND_JOB_ID.to_string()],
        "case the two-job listing: expected both jobs oldest first, got {answer}"
    );
    assert_eq!(
        answer
            .get("inProgressJobs")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "case the two-job listing: nothing has been started, got {answer}"
    );

    // Starting hands out the OLDEST queued execution with its document, and leaves it
    // IN_PROGRESS — which the listing then reports.
    let (topic, answer) = request(&mut device, "start-next", &json!({})).await;
    assert_eq!(
        topic,
        jobs_topic("start-next/accepted"),
        "case the start: expected the accepted answer, got {topic} with {answer}"
    );
    let execution = execution_of("the start", &answer);
    assert_eq!(
        text("the start", &execution, "jobId"),
        JOB_ID,
        "case the start: expected the oldest queued job, got {execution}"
    );
    assert_eq!(
        document_of("the start", &execution),
        document(JOB_DOCUMENT),
        "case the start: expected the job document, got {execution}"
    );
    assert_eq!(
        text("the start", &execution, "status"),
        "IN_PROGRESS",
        "case the start: the started execution must be IN_PROGRESS, got {execution}"
    );

    let (_topic, answer) = request(&mut device, "get", &json!({})).await;
    let in_progress: Vec<String> = answer
        .get("inProgressJobs")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| text("the listing after the start", summary, "jobId"))
        .collect();
    assert_eq!(
        in_progress,
        vec![JOB_ID.to_string()],
        "case the listing after the start: expected the started job in progress, got {answer}"
    );

    // An execution named by its own id is answered whatever is next, and a request that asks
    // for the document to be left out is answered without it.
    let (topic, answer) = request(
        &mut device,
        &format!("{SECOND_JOB_ID}/get"),
        &json!({ "includeJobDocument": false }),
    )
    .await;
    assert_eq!(
        topic,
        jobs_topic(&format!("{SECOND_JOB_ID}/get/accepted")),
        "case a job named by id: expected the accepted answer, got {topic} with {answer}"
    );
    let execution = execution_of("a job named by id", &answer);
    assert_eq!(
        text("a job named by id", &execution, "jobId"),
        SECOND_JOB_ID,
        "case a job named by id: expected that job's execution, got {execution}"
    );
    assert!(
        execution.get("jobDocument").is_none(),
        "case a job named by id: the document was asked to be left out, got {execution}"
    );
}

/// Work added to a device's queue by the OTHER operation that enqueues an execution over the
/// same store — `AssociateTargetsWithJob`, which points a CONTINUOUS job at a further device —
/// is announced on that device's own `notify-next` topic too: the announcement follows the
/// ENQUEUE and not the call that made it.
#[tokio::test]
async fn work_added_by_a_later_target_association_is_announced_too() {
    let emulator = emulator();
    let thing_arn = given_thing(&emulator).await;
    let address = emulator.discover().await;

    let mut device = Device::connect(Transport::Clear, &address, "jobs-association").await;
    subscribe_to_answers(&mut device).await;

    // A CONTINUOUS job created against ANOTHER device, so nothing is enqueued for this one
    // yet: "you can associate a target with a job... the job must have been created with the
    // targetSelection field set to CONTINUOUS".
    let other = control_plane(
        &emulator,
        "POST",
        "/things/mqtt-jobs-other-device",
        Some(json!({})),
    )
    .await;
    let other_arn = other
        .get("thingArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: CreateThing reported no thingArn: {other}"))
        .to_string();
    control_plane(
        &emulator,
        "PUT",
        &format!("/jobs/{JOB_ID}"),
        Some(json!({
            "targets": [other_arn],
            "document": JOB_DOCUMENT,
            "targetSelection": "CONTINUOUS",
        })),
    )
    .await;

    // Pointing the same job at this device enqueues its execution, and the announcement
    // arrives on the device's own topic with the document it has to run.
    control_plane(
        &emulator,
        "POST",
        &format!("/jobs/{JOB_ID}/targets"),
        Some(json!({ "targets": [thing_arn] })),
    )
    .await;
    let (topic, announced) = answered(&mut device).await;
    assert_eq!(
        topic,
        jobs_topic("notify-next"),
        "case the association: expected notify-next, got {topic} with {announced}"
    );
    let execution = execution_of("the association", &announced);
    assert_eq!(
        text("the association", &execution, "jobId"),
        JOB_ID,
        "case the association: expected the associated job, got {execution}"
    );
    assert_eq!(
        document_of("the association", &execution),
        document(JOB_DOCUMENT),
        "case the association: expected the job's own document, got {execution}"
    );
}

/// One row of the refusal table: the request, and the `code` the service refuses it with.
struct RefusalCase {
    /// What the case is called in an assertion.
    label: &'static str,
    /// The levels of the request topic under the device's own jobs namespace.
    levels: String,
    /// The body the device publishes.
    body: Value,
    /// The `code` the refusal carries.
    code: &'static str,
}

/// Every class of request this exchange refuses is answered on the request's own `rejected`
/// topic, with the code that says which part of it was refused.
#[tokio::test]
async fn a_request_the_exchange_refuses_is_answered_on_its_rejected_topic() {
    let emulator = emulator();
    let thing_arn = given_thing(&emulator).await;
    let address = emulator.discover().await;

    let mut device = Device::connect(Transport::Clear, &address, "jobs-refusals").await;
    subscribe_to_answers(&mut device).await;

    // A device with nothing pending at all: the `$next` label names no execution.
    let (topic, answer) = request(&mut device, "$next/get", &json!({})).await;
    assert_eq!(
        topic,
        jobs_topic("$next/get/rejected"),
        "case nothing pending: expected the rejected answer, got {topic} with {answer}"
    );
    assert_eq!(
        text("nothing pending", &answer, "code"),
        "ResourceNotFound",
        "case nothing pending: unexpected code, got {answer}"
    );

    given_job(&emulator, JOB_ID, &thing_arn, JOB_DOCUMENT).await;
    let (topic, _) = answered(&mut device).await;
    assert_eq!(
        topic,
        jobs_topic("notify-next"),
        "case the job: expected its announcement, got {topic}"
    );

    // The rows span what the exchange refuses: a job the device holds no execution of, a body
    // that is no JSON document, an update naming no status, an update naming a version the
    // service does not hold, and an update to a status the execution's own state refuses
    // (REJECTED is only reachable while the execution is still queued, so it is reported after
    // the execution has been carried to SUCCEEDED below).
    let cases = vec![
        RefusalCase {
            label: "a job the device holds no execution of",
            levels: "mqtt-jobs-absent/get".to_string(),
            body: json!({}),
            code: "ResourceNotFound",
        },
        RefusalCase {
            label: "an update naming no status",
            levels: format!("{JOB_ID}/update"),
            body: json!({ "statusDetails": { "progress": "10" } }),
            code: "InvalidRequest",
        },
        RefusalCase {
            label: "an update whose status detail is no string",
            levels: format!("{JOB_ID}/update"),
            body: json!({ "status": "IN_PROGRESS", "statusDetails": { "progress": 100 } }),
            code: "InvalidRequest",
        },
        RefusalCase {
            label: "an update whose statusDetails is no map at all",
            levels: format!("{JOB_ID}/update"),
            body: json!({ "status": "IN_PROGRESS", "statusDetails": "progress" }),
            code: "InvalidRequest",
        },
        RefusalCase {
            label: "an update naming a version the service does not hold",
            levels: format!("{JOB_ID}/update"),
            body: json!({ "status": "IN_PROGRESS", "expectedVersion": 99 }),
            code: "VersionMismatch",
        },
    ];

    for case in &cases {
        let (topic, answer) = request(&mut device, &case.levels, &case.body).await;
        assert_eq!(
            topic,
            jobs_topic(&format!("{}/rejected", case.levels)),
            "case {}: expected the rejected answer, got {topic} with {answer}",
            case.label
        );
        assert_eq!(
            text(case.label, &answer, "code"),
            case.code,
            "case {}: unexpected code, got {answer}",
            case.label
        );
        assert!(
            answer.get("message").and_then(Value::as_str).is_some(),
            "case {}: a refusal must carry a sentence, got {answer}",
            case.label
        );
    }

    // A body that is no JSON document at all is refused as such, with nothing read out of it.
    let topic = jobs_topic(&format!("{JOB_ID}/update"));
    device.publish_at(&topic, b"not a document", 1, false).await;
    let (answered_on, answer) = answered(&mut device).await;
    assert_eq!(
        answered_on,
        format!("{topic}/rejected"),
        "case a body that is no document: expected the rejected answer, got {answered_on} with \
         {answer}"
    );
    assert_eq!(
        text("a body that is no document", &answer, "code"),
        "InvalidJson",
        "case a body that is no document: unexpected code, got {answer}"
    );
    assert_eq!(
        text("a body that is no document", &answer, "message"),
        "Invalid JSON",
        "case a body that is no document: the sentence must say nothing of the reading, got \
         {answer}"
    );

    // An execution carried to a terminal status takes no further report: the refusal names the
    // transition and carries the state it was refused against.
    let (_topic, _answer) = request(
        &mut device,
        &format!("{JOB_ID}/update"),
        &json!({ "status": "SUCCEEDED" }),
    )
    .await;
    let (topic, answer) = request(
        &mut device,
        &format!("{JOB_ID}/update"),
        &json!({ "status": "IN_PROGRESS" }),
    )
    .await;
    assert_eq!(
        topic,
        jobs_topic(&format!("{JOB_ID}/update/rejected")),
        "case a report after a terminal status: expected the rejected answer, got {topic} with \
         {answer}"
    );
    assert_eq!(
        text("a report after a terminal status", &answer, "code"),
        "InvalidStateTransition",
        "case a report after a terminal status: unexpected code, got {answer}"
    );
    let state = answer
        .get("executionState")
        .cloned()
        .unwrap_or_else(|| panic!("case a report after a terminal status: no state in {answer}"));
    assert_eq!(
        text("a report after a terminal status", &state, "status"),
        "SUCCEEDED",
        "case a report after a terminal status: expected the state it was refused against, got \
         {state}"
    );
}
