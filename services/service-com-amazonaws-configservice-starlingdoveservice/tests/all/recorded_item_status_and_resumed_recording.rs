// SPDX-License-Identifier: CC0-1.0

//! WHICH configuration item a running configuration recorder writes, and WHEN.
//!
//! Two rules are pinned here, both about the ingest path a running recorder drives:
//!
//! 1. A recorder records a configuration item for a resource only when it observes a
//!    state the resource's held item does not already record, so the resource's FIRST
//!    item — the only one an unchanged resource ever has — is stamped
//!    `ResourceDiscovered`, and a later state records `OK`.
//! 2. Recording is suspended between `StopConfigurationRecorder` and the next
//!    `StartConfigurationRecorder` and RESUMES after it: a change made while the
//!    recorder was stopped is recorded once it runs again.
//!
//! The resource every row below is about lives in its OWN service — Config is told
//! nothing about it — so every configuration item asserted on is one the recorder
//! observed, read back through `GetResourceConfigHistory` and
//! `BatchGetResourceConfig`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::Emulator;

/// The resource type Config records an Amazon SQS queue under, an entry of
/// `com.amazonaws.configservice#ResourceType`.
const QUEUE_RESOURCE_TYPE: &str = "AWS::SQS::Queue";

/// `ConfigurationItemStatus` values
/// (`smithy_query(query="shape", shape="ConfigurationItemStatus")`, whose values are
/// `OK`, `ResourceDiscovered`, `ResourceNotRecorded`, `ResourceDeleted` and
/// `ResourceDeletedNotRecorded`): the item that first discovered a resource, and the one
/// recording a configuration that has since been updated.
const RESOURCE_DISCOVERED: &str = "ResourceDiscovered";
/// See [`RESOURCE_DISCOVERED`].
const OK: &str = "OK";

/// The recorder every row drives, the one [`Emulator::recording`] writes.
const RECORDER_NAME: &str = "the-recorder";

/// The error a configuration-item read answers for a resource Config holds no item for
/// (`ResourceNotDiscoveredException`), which is what the read answers until the
/// recorder's pass has seen the resource.
const NOT_DISCOVERED: &str = "ResourceNotDiscoveredException";

/// The `lastStatus` a processed recording event reaches
/// (`com.amazonaws.configservice#RecorderStatus`).
const SUCCESS: &str = "Success";

/// How long a poll waits for the recording pass the clock owes. A pass falls due a
/// fraction of a second after the call that makes it due, so this is hundreds of passes'
/// worth of slack and not a span the behaviour depends on: the wait ends on the CONDITION,
/// and the budget is only there so a wedged emulator fails instead of hanging.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(180);

/// How long a row watches while asserting that NO further item is recorded — several
/// passes' worth of time, so an item that was going to be recorded would have been. A
/// slower host only watches for longer than it needs to.
const QUIET_BUDGET: Duration = Duration::from_millis(2_500);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The configuration items `GetResourceConfigHistory` reports for `queue_url`, newest
/// first — empty while Config has discovered no such resource, which is the refusal that
/// read answers before the recorder's first pass has seen the queue.
async fn history(case: &str, emulator: &Emulator, queue_url: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call(
            "config",
            "StarlingDoveService",
            "1.1",
            "GetResourceConfigHistory",
            &json!({ "resourceType": QUEUE_RESOURCE_TYPE, "resourceId": queue_url }),
        )
        .await;
    if status == 400 && body["__type"] == json!(NOT_DISCOVERED) {
        return Vec::new();
    }
    assert_eq!(
        status, 200,
        "case {case:?}: GetResourceConfigHistory must answer 200, got {status} with {body}"
    );
    body["configurationItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `configurationItemStatus` of each item, newest first.
fn statuses(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item["configurationItemStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The recorded `configuration` document of `item`, decoded from the JSON string the
/// member carries.
fn configuration(case: &str, item: &Value) -> Value {
    let encoded = item["configuration"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a recorded configuration, got {item}"));
    serde_json::from_str(encoded).unwrap_or_else(|error| {
        panic!("case {case:?}: a recorded configuration must be JSON, got {error} for {encoded}")
    })
}

/// Read `queue_url`'s history until it holds `wanted` items, and answer them.
///
/// What is waited for is the pass the recorder owes, never a span: the assertion that
/// fails at the deadline names the items the read did report.
async fn await_items(case: &str, emulator: &Emulator, queue_url: &str, wanted: usize) -> Vec<Value> {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let items = history(case, emulator, queue_url).await;
        if items.len() == wanted {
            return items;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {wanted} configuration item(s) for {queue_url}, got {:?} \
             with statuses {:?}",
            items.len(),
            statuses(&items)
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert that `queue_url`'s history stays exactly `wanted` items for as long as
/// [`QUIET_BUDGET`]: observing a resource that is still in the state its held item
/// records writes no item at all.
async fn stays_at(case: &str, emulator: &Emulator, queue_url: &str, wanted: usize) -> Vec<Value> {
    let deadline = Instant::now() + QUIET_BUDGET;
    loop {
        let items = history(case, emulator, queue_url).await;
        assert_eq!(
            items.len(),
            wanted,
            "case {case:?}: an unchanged resource records no further configuration item, so \
             {queue_url} must keep {wanted} item(s), got statuses {:?} and configurations {:?}",
            statuses(&items),
            items
                .iter()
                .map(|item| item["configuration"].clone())
                .collect::<Vec<Value>>()
        );
        if Instant::now() >= deadline {
            return items;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a queue through Amazon SQS' own API and answer its url.
async fn create_queue(emulator: &Emulator, name: &str, attributes: &Value) -> String {
    let mut request = json!({ "QueueName": name });
    if !attributes.is_null() {
        request["Attributes"] = attributes.clone();
    }
    let created = emulator.sqs("CreateQueue", &request).await;
    created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue: expected a QueueUrl, got {created}"))
        .to_string()
}

#[tokio::test]
async fn every_resource_the_owning_service_lists_is_recorded_however_it_pages() {
    let emulator = Emulator::new();
    emulator.recording().await;

    // Amazon SQS answers a queue listing in PAGES, so an account holding more queues than
    // one page fits is observed by following the listing's token. The count here is one
    // past the page a listing that names no size answers with, and the names are padded so
    // the listing's own ordering is the one asserted on.
    const QUEUES: usize = 101;
    let mut urls = Vec::with_capacity(QUEUES);
    for index in 0..QUEUES {
        urls.push(create_queue(&emulator, &format!("paged-queue-{index:04}"), &Value::Null).await);
    }

    // The LAST queue of the listing is the one only a second page reports: it is recorded
    // like every other, and the first page's queues are not reported gone because a page
    // boundary hid them from one read.
    let case = "a queue a second page of the listing reports";
    let last = urls
        .last()
        .cloned()
        .expect("the listing has a last queue");
    await_items(case, &emulator, &last, 1).await;
    for (index, url) in urls.iter().enumerate() {
        let items = history(case, &emulator, url).await;
        assert_eq!(
            statuses(&items),
            vec![RESOURCE_DISCOVERED.to_string()],
            "case {case:?}: queue {index} must carry its discovery item and nothing else, \
             got {items:?}"
        );
    }
}

#[tokio::test]
async fn the_only_item_of_a_resource_the_recorder_discovers_is_its_discovery() {
    let emulator = Emulator::new();
    emulator.recording().await;

    // The rows differ along the SHAPE of the resource the recorder discovers: how many
    // attributes it was created with, whether it is a FIFO queue (which carries the
    // attributes only a FIFO queue has), and whether it carries tags — the whole of
    // which the recorded configuration item is built from.
    let rows: [(&str, &str, Value); 4] = [
        ("a queue created with no attributes", "plain-queue", Value::Null),
        (
            "a queue created with one attribute",
            "delayed-queue",
            json!({ "DelaySeconds": "9" }),
        ),
        (
            "a queue created with several attributes",
            "configured-queue",
            json!({
                "VisibilityTimeout": "45",
                "MessageRetentionPeriod": "600",
                "MaximumMessageSize": "2048",
                "ReceiveMessageWaitTimeSeconds": "7",
            }),
        ),
        (
            "a FIFO queue",
            "ordered-queue.fifo",
            json!({ "FifoQueue": "true", "ContentBasedDeduplication": "true" }),
        ),
    ];

    for (case, name, attributes) in rows {
        let queue_url = create_queue(&emulator, name, &attributes).await;

        // The resource's first configuration item is the one that DISCOVERED it, and it
        // stays the resource's only item for as long as nothing about the resource
        // changes.
        await_items(case, &emulator, &queue_url, 1).await;
        let items = stays_at(case, &emulator, &queue_url, 1).await;
        assert_eq!(
            statuses(&items),
            vec![RESOURCE_DISCOVERED.to_string()],
            "case {case:?}: the item of a newly discovered resource is its discovery, got {items:?}"
        );

        // The same item, read through the batch path: one projection of one recorded
        // state, so the status cannot differ between the two reads.
        let batch = emulator
            .config(
                "BatchGetResourceConfig",
                &json!({
                    "resourceKeys": [
                        { "resourceType": QUEUE_RESOURCE_TYPE, "resourceId": queue_url },
                    ],
                }),
            )
            .await;
        assert_eq!(
            batch["baseConfigurationItems"][0]["configurationItemStatus"],
            json!(RESOURCE_DISCOVERED),
            "case {case:?}: the batch read reports the status the item was recorded with, \
             got {batch}"
        );
    }
}

#[tokio::test]
async fn a_changed_resource_records_one_further_item_per_state_it_leaves() {
    let emulator = Emulator::new();
    emulator.recording().await;
    let queue_url = create_queue(&emulator, "changing-queue", &json!({ "DelaySeconds": "1" })).await;
    await_items("the discovery", &emulator, &queue_url, 1).await;

    // The rows differ along WHICH attribute changes and the value it moves to; each one
    // adds exactly one configuration item, whatever the recorder observes in between.
    let changes: [(&str, &str, &str); 3] = [
        ("the visibility timeout", "VisibilityTimeout", "41"),
        ("the delay", "DelaySeconds", "9"),
        ("the message retention period", "MessageRetentionPeriod", "600"),
    ];

    let mut expected = 1;
    for (case, attribute, updated) in changes {
        emulator
            .sqs(
                "SetQueueAttributes",
                &json!({ "QueueUrl": queue_url, "Attributes": { attribute: updated } }),
            )
            .await;
        expected += 1;
        await_items(case, &emulator, &queue_url, expected).await;
        let items = stays_at(case, &emulator, &queue_url, expected).await;

        // The newest item records the state the change left the resource in, and it is
        // an UPDATE of an already discovered resource; the oldest stays the discovery.
        assert_eq!(
            configuration(case, &items[0])[attribute],
            json!(updated),
            "case {case:?}: the newest item records the state the change left, got {:?}",
            items[0]
        );
        let reported = statuses(&items);
        assert_eq!(
            reported.first().map(String::as_str),
            Some(OK),
            "case {case:?}: an item recording a changed configuration is an update, got \
             {reported:?}"
        );
        assert_eq!(
            reported.last().map(String::as_str),
            Some(RESOURCE_DISCOVERED),
            "case {case:?}: the oldest item of the resource stays its discovery, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_change_made_while_the_recorder_was_stopped_is_recorded_once_it_runs_again() {
    let emulator = Emulator::new();
    emulator.recording().await;
    let queue_url =
        create_queue(&emulator, "paused-queue", &json!({ "VisibilityTimeout": "31" })).await;
    await_items("the discovery", &emulator, &queue_url, 1).await;

    // Two stop-and-restart cycles, differing along the attribute changed while the
    // recorder was stopped: recording resumes every time it is started, not only the
    // first time.
    let cycles: [(&str, &str, &str); 2] = [
        ("the visibility timeout", "VisibilityTimeout", "41"),
        ("the delay", "DelaySeconds", "9"),
    ];

    let mut expected = 1;
    for (case, attribute, updated) in cycles {
        emulator
            .config(
                "StopConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": RECORDER_NAME }),
            )
            .await;
        emulator
            .sqs(
                "SetQueueAttributes",
                &json!({ "QueueUrl": queue_url, "Attributes": { attribute: updated } }),
            )
            .await;

        // Nothing is recorded while the recorder is stopped: the change is invisible to
        // Config until it runs again.
        let items = stays_at(case, &emulator, &queue_url, expected).await;
        assert_ne!(
            configuration(case, &items[0])[attribute],
            json!(updated),
            "case {case:?}: a stopped recorder records nothing, got {:?}",
            items[0]
        );

        emulator
            .config(
                "StartConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": RECORDER_NAME }),
            )
            .await;
        expected += 1;
        let items = await_items(case, &emulator, &queue_url, expected).await;
        assert_eq!(
            configuration(case, &items[0])[attribute],
            json!(updated),
            "case {case:?}: the restarted recorder records the change it was stopped for, \
             got {:?}",
            items[0]
        );

        // The restarted recorder's own recording event is processed: it reports that it
        // is recording, and its last status leaves the in-progress value.
        let deadline = Instant::now() + CONVERGENCE_BUDGET;
        loop {
            let described = emulator
                .config(
                    "DescribeConfigurationRecorderStatus",
                    &json!({ "ConfigurationRecorderNames": [RECORDER_NAME] }),
                )
                .await;
            let status = described["ConfigurationRecordersStatus"][0].clone();
            if status["recording"] == json!(true) && status["lastStatus"] == json!(SUCCESS) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: a restarted recorder reaches lastStatus {SUCCESS:?} while \
                 recording, got {status}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    // A resource CREATED while the recorder was stopped is discovered once it runs
    // again: resumption is about what the recorder observes from then on, not only about
    // the resources it had already seen.
    let case = "a queue created while the recorder was stopped";
    emulator
        .config(
            "StopConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER_NAME }),
        )
        .await;
    let created_while_stopped =
        create_queue(&emulator, "created-while-stopped", &json!({ "DelaySeconds": "3" })).await;
    stays_at(case, &emulator, &created_while_stopped, 0).await;
    emulator
        .config(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER_NAME }),
        )
        .await;
    let items = await_items(case, &emulator, &created_while_stopped, 1).await;
    assert_eq!(
        statuses(&items),
        vec![RESOURCE_DISCOVERED.to_string()],
        "case {case:?}: the item is the resource's discovery, got {items:?}"
    );
    assert_eq!(
        configuration(case, &items[0])["DelaySeconds"],
        json!("3"),
        "case {case:?}: the discovery item records the state the resource is in, got {:?}",
        items[0]
    );
}
