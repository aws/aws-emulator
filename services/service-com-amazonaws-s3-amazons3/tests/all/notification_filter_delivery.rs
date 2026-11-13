// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a notification configuration's KEY-NAME FILTER does to the
//! delivery of an object event: the one rule that makes the negative leg of a filter
//! observable.
//!
//! RULE: a destination configuration carrying a key-name `Filter` is notified of an object
//! change when the object's key passes EVERY rule of that filter — "the Amazon S3 API supports
//! filtering based on the prefix and suffix of the object key name"
//! (`NotificationConfigurationFilter`) — so a key the filter admits produces exactly ONE
//! notification at the destination and a key it does not produces NONE. The negative leg is
//! observable only because the positive one delivers, which is why both are asserted of the
//! SAME configuration and the SAME queue, with the key the filter rejects written FIRST: the
//! admitted key's record arriving is the barrier that proves the delivery path ran at all, and
//! the rejected key's absence is then an absence rather than a silence.
//!
//! The table spans what the rule generalises over: which rules the filter carries (a prefix, a
//! suffix, both, none at all), and — for each — a key that passes every rule beside keys that
//! fail one of them. Every call goes over the wire through a router carrying S3 beside the
//! Amazon SQS the destination lives in, and the deliveries are read with `ReceiveMessage`, so
//! what is pinned is the message a consumer receives.
//!
//! The harness is [`super::object_event_notifications`]'s, which also asserts the `s3:TestEvent`
//! a validated queue destination receives when the configuration is INSTALLED — so each row
//! below reads only what its own object changes delivered.

use serde_json::Value;

use super::object_event_notifications::{queue_arn, Destination, Emulator};

/// The event class every row subscribes to: the wildcard form, which "stands for every event
/// of that type", so the row's filter is the only thing that can exclude a write.
const CREATED: &str = "s3:ObjectCreated:*";

/// The `eventName` a `PutObject` of an admitted key must deliver.
const CREATED_BY_PUT: &str = "ObjectCreated:Put";

/// One row of the filter table: the rules the configuration's `Filter` carries, the key it
/// admits, and the keys it does not.
struct Row {
    case: &'static str,
    /// `(Name, Value)` per `FilterRule`, as the request spells them (the `FilterRuleName`
    /// vocabulary is lower case).
    filter_rules: &'static [(&'static str, &'static str)],
    /// A key every rule of the filter admits.
    admitted: &'static str,
    /// Keys at least one rule rejects, each of which must deliver nothing.
    rejected: &'static [&'static str],
}

/// The table. The two rule kinds are the two values `FilterRuleName` publishes
/// (`smithy_query(query="shape",shape="FilterRuleName")` reports `[prefix, suffix]`), and the
/// rows carry each alone, both together, and neither.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a prefix rule",
            filter_rules: &[("prefix", "images/")],
            admitted: "images/photo.jpg",
            rejected: &["logs/photo.jpg", "photo.jpg"],
        },
        Row {
            case: "a suffix rule",
            filter_rules: &[("suffix", ".jpg")],
            admitted: "images/photo.jpg",
            rejected: &["images/photo.txt", "notes.md"],
        },
        Row {
            case: "a prefix and a suffix, both of which must hold",
            filter_rules: &[("prefix", "images/"), ("suffix", ".jpg")],
            admitted: "images/holiday/photo.jpg",
            rejected: &["images/photo.txt", "logs/photo.jpg"],
        },
        Row {
            case: "no filter at all, which restricts nothing",
            filter_rules: &[],
            admitted: "anything/at/all.bin",
            rejected: &[],
        },
    ]
}

/// The `Records` one delivered message carries, with the key and the event name of each.
fn records_of(message: &Value) -> Vec<(String, String)> {
    message["Records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|record| {
            (
                record["s3"]["object"]["key"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                record["eventName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_configurations_filter_decides_which_writes_reach_its_queue() {
    for (index, row) in rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let bucket = format!("notification-filter-bucket-{index}");
        let queue = format!("notification-filter-queue-{index}");
        emulator.create_bucket(&bucket).await;
        let queue_url = emulator.create_queue(&queue).await;
        let arn = queue_arn(&queue);
        // The put is validated against the queue's own policy and takes the `s3:TestEvent`
        // that validation publishes, so what is read below is this row's own deliveries.
        emulator
            .put_notification(
                &bucket,
                &Destination::queue(&arn),
                &[CREATED],
                row.filter_rules,
                false,
            )
            .await;

        // The keys the filter REJECTS are written first, so the admitted key's record is the
        // barrier that proves the delivery path ran: nothing of these may be on the queue
        // once it arrives.
        for key in row.rejected {
            let written = emulator.put_object(&bucket, key, b"rejected").await;
            assert_eq!(
                written.status,
                200,
                "case {:?}: writing {key:?} must succeed, got {} with {}",
                row.case,
                written.status,
                written.text()
            );
        }
        let written = emulator.put_object(&bucket, row.admitted, b"admitted").await;
        assert_eq!(
            written.status,
            200,
            "case {:?}: writing {:?} must succeed, got {} with {}",
            row.case,
            row.admitted,
            written.status,
            written.text()
        );

        let delivered = emulator
            .delivered(&queue_url)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the admitted key must be delivered", row.case));
        let records = records_of(&delivered);
        assert_eq!(
            records,
            vec![(row.admitted.to_string(), CREATED_BY_PUT.to_string())],
            "case {:?}: expected one record naming {:?}, got {records:?} in {delivered}",
            row.case,
            row.admitted
        );

        // And nothing else: a key the filter rejects delivers no record of its own, early or
        // late, so the queue is empty once the admitted key's record has been taken.
        let late = emulator.nothing_delivered(&queue_url).await;
        assert!(
            late.is_empty(),
            "case {:?}: expected the rejected keys {:?} to deliver nothing, got {late:?}",
            row.case,
            row.rejected
        );
    }
}
