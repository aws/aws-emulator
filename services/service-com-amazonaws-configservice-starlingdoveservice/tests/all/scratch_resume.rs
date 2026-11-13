// SPDX-License-Identifier: CC0-1.0

//! What a recorded configuration item CARRIES for a resource the recorder observed: the
//! resource's configuration as the owning service reports it, and the digest a caller
//! compares two items' states by.
//!
//! The rule pinned here: the `configuration` document of the configuration item Config
//! records for a resource carries the configuration the owning service reports for it,
//! with the value that service reports and nothing that is not configuration at all, and
//! the item's `configurationItemMD5Hash` is the digest of the state that item records, so
//! two items of one resource compare equal exactly while the resource has not changed
//! (`ConfigurationItem$configurationItemMD5Hash`: "unique MD5 hash that represents the
//! configuration item's state ... You can use MD5 hash to compare the states of two or
//! more configuration items that are associated with the same resource").

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::Emulator;

/// The resource type Config records an Amazon SQS queue under.
const QUEUE_RESOURCE_TYPE: &str = "AWS::SQS::Queue";

/// The error a configuration-item read answers for a resource Config holds no item for.
const NOT_DISCOVERED: &str = "ResourceNotDiscoveredException";

/// The attributes a queue's configuration item does NOT carry, because none of them is
/// how the queue is CONFIGURED: the three counts of the messages the queue holds at the
/// instant it is read, and the two instants saying when it was created and last changed
/// (`com.amazonaws.sqs#QueueAttributeName`).
const NOT_CONFIGURATION: [&str; 5] = [
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesDelayed",
    "ApproximateNumberOfMessagesNotVisible",
    "CreatedTimestamp",
    "LastModifiedTimestamp",
];

/// How long a poll waits for the recording pass the clock owes — hundreds of passes'
/// worth of slack, not a span the behaviour depends on: the wait ends on the CONDITION,
/// and the budget is only there so a wedged emulator fails instead of hanging.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(180);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The configuration items reported for `queue_url`, newest first; empty while Config has
/// discovered no such resource.
async fn history(emulator: &Emulator, queue_url: &str) -> Vec<Value> {
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
        "GetResourceConfigHistory must answer 200, got {status} with {body}"
    );
    body["configurationItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Read `queue_url`'s history until it holds `wanted` items, and answer them.
async fn await_items(case: &str, emulator: &Emulator, queue_url: &str, wanted: usize) -> Vec<Value> {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let items = history(emulator, queue_url).await;
        if items.len() == wanted {
            return items;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {wanted} configuration item(s) for {queue_url}, got {}",
            items.len()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The recorded `configuration` document of `item`, decoded.
fn configuration(case: &str, item: &Value) -> Value {
    let encoded = item["configuration"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a recorded configuration, got {item}"));
    serde_json::from_str(encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: a configuration must be JSON, got {error}"))
}

#[tokio::test]
async fn a_recorded_item_carries_the_configuration_the_owning_service_reports() {
    let emulator = Emulator::new();
    emulator.recording().await;

    // The rows differ along the attributes the queue is created with, which is what the
    // recorded document is built from.
    let rows: [(&str, &str, Value); 3] = [
        ("a queue created with no attributes", "echoed-plain", Value::Null),
        (
            "a queue created with several attributes",
            "echoed-configured",
            json!({ "VisibilityTimeout": "45", "DelaySeconds": "9" }),
        ),
        (
            "a FIFO queue",
            "echoed-ordered.fifo",
            json!({ "FifoQueue": "true", "ContentBasedDeduplication": "true" }),
        ),
    ];

    for (case, name, attributes) in rows {
        let mut request = json!({ "QueueName": name });
        if !attributes.is_null() {
            request["Attributes"] = attributes.clone();
        }
        let created = emulator.sqs("CreateQueue", &request).await;
        let queue_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
            .to_string();

        let items = await_items(case, &emulator, &queue_url, 1).await;
        let recorded = configuration(case, &items[0]);

        // Every attribute Amazon SQS reports as part of how the queue is CONFIGURED is in
        // the recorded document with the value Amazon SQS reports; the message counts and
        // the two instants are not configuration, so the document carries neither.
        let live = emulator
            .sqs(
                "GetQueueAttributes",
                &json!({ "QueueUrl": queue_url, "AttributeNames": ["All"] }),
            )
            .await;
        let live = live["Attributes"]
            .as_object()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected live attributes, got {live}"));
        for (attribute, value) in &live {
            if NOT_CONFIGURATION.contains(&attribute.as_str()) {
                assert!(
                    recorded[attribute].is_null(),
                    "case {case:?}: {attribute} is not part of how the queue is \
                     configured, so the item must not carry it, got {recorded}"
                );
                continue;
            }
            assert_eq!(
                &recorded[attribute], value,
                "case {case:?}: expected the recorded {attribute} to be the value Amazon \
                 SQS reports, got {recorded}"
            );
        }

        // The item's own digest: the state it records, so reading it again reports the
        // same hash, and a state the resource changed into carries a different one.
        let hash = items[0]["configurationItemMD5Hash"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a hash, got {:?}", items[0]))
            .to_string();
        assert_eq!(
            hash.len(),
            32,
            "case {case:?}: an MD5 hash is 32 hexadecimal characters, got {hash:?}"
        );
        let again = history(&emulator, &queue_url).await;
        assert_eq!(
            again[0]["configurationItemMD5Hash"], items[0]["configurationItemMD5Hash"],
            "case {case:?}: two reads of ONE item report the same hash, got {:?}",
            again[0]
        );

        emulator
            .sqs(
                "SetQueueAttributes",
                &json!({ "QueueUrl": queue_url, "Attributes": { "MessageRetentionPeriod": "600" } }),
            )
            .await;
        let changed = await_items(case, &emulator, &queue_url, 2).await;
        assert_ne!(
            changed[0]["configurationItemMD5Hash"], json!(hash),
            "case {case:?}: the item recording a changed state carries a different hash, \
             got {:?}",
            changed[0]
        );
        assert_eq!(
            changed[1]["configurationItemMD5Hash"], json!(hash),
            "case {case:?}: the item recording the state before it keeps its hash, got {:?}",
            changed[1]
        );
    }
}
