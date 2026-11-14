// SPDX-License-Identifier: CC0-1.0

//! What a SCHEDULED configuration-snapshot delivery actually DELIVERS: the gzip-framed
//! snapshot object Config writes into the delivery channel's Amazon S3 bucket, and the
//! outcome of that write the channel then reports.
//!
//! The rule pinned here: a snapshot delivery accepted for a delivery channel is
//! PERFORMED — Config writes one object into that channel's bucket, under the
//! `AWSLogs/<account-id>/Config/<region>/` key prefix (behind the channel's own
//! `s3KeyPrefix` where it configures one) with the returned `configSnapshotId` in its
//! key, gzip-framed and holding the configuration items the recorder has captured — and
//! the channel's `configSnapshotDeliveryInfo` reports the outcome of THAT write: a
//! successful terminal status with `lastSuccessfulTime` and no error when the object was
//! written, and nothing claimed for a write that did not happen — an export no delivery
//! was performed for reports its schedule and no outcome.
//!
//! Config and S3 are mounted on ONE engine, reusing the harness
//! `config_recorded_resource_compliance` builds: the delivery writes into S3 through the
//! same in-process callback the recorder reads it through, so without S3 mounted there is
//! no bucket to deliver to (which is the FAILED outcome `config_deliver_config_snapshot`
//! pins). Config is driven over the wire, the way a client drives it; the buckets and the
//! delivered object are read through the AWS SDK for S3.

use std::io::Read;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::{Emulator, ACCOUNT_ID, REGION};

/// The resource type Config records an Amazon S3 general purpose bucket under — an entry
/// of `com.amazonaws.configservice#ResourceType`.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// The recorder and the delivery channel every case below sets up.
const RECORDER_NAME: &str = "the-recorder";
/// See [`RECORDER_NAME`].
const CHANNEL_NAME: &str = "the-channel";

/// The bucket every case delivers its snapshots INTO.
const DELIVERY_BUCKET: &str = "config-delivery-bucket";

/// The bucket every case has the recorder observe, and then looks for in the delivered
/// snapshot.
const OBSERVED_BUCKET: &str = "audited-bucket";

/// The `lastStatus` a delivery that was performed reports.
///
/// RECORDED behaviour, pinned as a LITERAL here rather than read from the service, so
/// this assertion fails if the spelling regresses to the model enum's `Success`
/// (`com.amazonaws.configservice#DeliveryStatus` spells its members `Success`, `Failure`,
/// `Not_Applicable`; the service answers them upper-cased).
const SUCCESS: &str = "SUCCESS";

/// How long a poll waits for the work the clock owes — the recording pass and the
/// delivery, each due a fraction of a second after the call that made it due. Many
/// passes' worth of slack, not a span the behaviour depends on.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(15);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Put the recorder and the delivery channel, delivering through `bucket` under `prefix`,
/// and start recording.
async fn recording_through(emulator: &Emulator, bucket: &str, prefix: Option<&str>) {
    emulator
        .config(
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": RECORDER_NAME,
                    "roleARN": format!("arn:aws:iam::{ACCOUNT_ID}:role/config-role"),
                    "recordingGroup": { "allSupported": true, "includeGlobalResourceTypes": false },
                }
            }),
        )
        .await;
    let mut channel = json!({ "name": CHANNEL_NAME, "s3BucketName": bucket });
    if let Some(prefix) = prefix {
        channel["s3KeyPrefix"] = json!(prefix);
    }
    emulator
        .config("PutDeliveryChannel", &json!({ "DeliveryChannel": channel }))
        .await;
    emulator
        .config(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER_NAME }),
        )
        .await;
}

/// Read `GetResourceConfigHistory` for `bucket` until the recorder has captured a
/// configuration item for it, and answer that item.
async fn await_recorded_item(case: &str, emulator: &Emulator, bucket: &str) -> Value {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let (status, body) = emulator
            .call(
                "config",
                "StarlingDoveService",
                "1.1",
                "GetResourceConfigHistory",
                &json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceId": bucket }),
            )
            .await;
        if status == 200 {
            if let Some(item) = body["configurationItems"]
                .as_array()
                .and_then(|items| items.last())
            {
                return item.clone();
            }
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a configuration item for {bucket}, got {status} with \
             {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `configSnapshotDeliveryInfo` the channel reports.
async fn snapshot_info(case: &str, emulator: &Emulator) -> Value {
    channel_status(case, emulator).await["configSnapshotDeliveryInfo"].clone()
}

/// The whole `DeliveryChannelStatus` the channel reports.
async fn channel_status(case: &str, emulator: &Emulator) -> Value {
    let response = emulator
        .config(
            "DescribeDeliveryChannelStatus",
            &json!({ "DeliveryChannelNames": [CHANNEL_NAME] }),
        )
        .await;
    let reported = response["DeliveryChannelsStatus"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: DeliveryChannelsStatus must be a list, got {response}")
        });
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the named channel is reported, got {response}"
    );
    reported[0].clone()
}

/// Read the channel's snapshot delivery info until it reports a terminal status, and
/// answer it.
async fn await_settled_delivery(case: &str, emulator: &Emulator) -> Value {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let info = snapshot_info(case, emulator).await;
        if info["lastStatus"].is_string() {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the delivery to reach a terminal lastStatus, got \
             {info}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every key the delivery bucket holds.
async fn delivered_keys(case: &str, emulator: &Emulator, bucket: &str) -> Vec<String> {
    let listed = emulator
        .s3()
        .list_objects_v2()
        .bucket(bucket)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: listing {bucket} must succeed, got {error:?}"));
    listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// Read the delivery bucket until it holds a key carrying `snapshot_id`, and answer every
/// key it holds.
async fn await_delivered_key(
    case: &str,
    emulator: &Emulator,
    bucket: &str,
    snapshot_id: &str,
) -> Vec<String> {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let keys = delivered_keys(case, emulator, bucket).await;
        if keys.iter().any(|key| key.contains(snapshot_id)) {
            return keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivered object naming {snapshot_id}, got {keys:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The bytes of one delivered object.
async fn delivered_body(case: &str, emulator: &Emulator, bucket: &str, key: &str) -> Vec<u8> {
    let object = emulator
        .s3()
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {key} must succeed, got {error:?}"));
    object
        .body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: {key} must be readable, got {error:?}"))
        .to_vec()
}

/// The gzip-framed body of a delivered snapshot, gunzipped and parsed.
///
/// `pub(crate)`: the stack-provisioned suite
/// (`cloudformation_provisioned_snapshot_delivery`) reads a delivered snapshot the same
/// way, and both modules compile into this crate's one test binary, so there is ONE
/// decoder for both.
pub(crate) fn gunzipped_snapshot(case: &str, body: &[u8]) -> Value {
    assert!(
        body.starts_with(&[0x1f, 0x8b]),
        "case {case:?}: a delivered snapshot is gzip-framed, got {:?}",
        &body[..2.min(body.len())]
    );
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(body)
        .read_to_end(&mut decoded)
        .unwrap_or_else(|error| panic!("case {case:?}: the body must gunzip, got {error:?}"));
    serde_json::from_slice(&decoded)
        .unwrap_or_else(|error| panic!("case {case:?}: the snapshot must be JSON, got {error:?}"))
}

/// Schedule a delivery and answer the `configSnapshotId` it was given.
async fn deliver(case: &str, emulator: &Emulator) -> String {
    let response = emulator
        .config(
            "DeliverConfigSnapshot",
            &json!({ "deliveryChannelName": CHANNEL_NAME }),
        )
        .await;
    response["configSnapshotId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a configSnapshotId, got {response}"))
        .to_string()
}

#[tokio::test]
async fn a_scheduled_snapshot_is_written_into_the_channels_bucket_and_reported_as_delivered() {
    // The rows differ along the key prefix the channel configures — the one thing the
    // rule generalises over in WHERE the object lands: none at all, one, and one already
    // ending in the delimiter.
    let cases: [(&str, Option<&str>, &str); 3] = [
        ("no key prefix", None, ""),
        ("a key prefix", Some("audit"), "audit/"),
        (
            "a key prefix ending in the delimiter",
            Some("audit/"),
            "audit/",
        ),
    ];

    for (case, prefix, expected_prefix) in cases {
        let emulator = Emulator::new();
        emulator.create_delivery_bucket(DELIVERY_BUCKET).await;
        recording_through(&emulator, DELIVERY_BUCKET, prefix).await;

        // The resource the snapshot has to carry, created through its OWN service after
        // the recorder was started, and read back through Config's own history read.
        emulator.create_bucket(OBSERVED_BUCKET, &[]).await;
        let recorded = await_recorded_item(case, &emulator, OBSERVED_BUCKET).await;
        assert!(
            recorded["arn"]
                .as_str()
                .is_some_and(|arn| arn.ends_with(OBSERVED_BUCKET)),
            "case {case:?}: the recorded item names the bucket's own ARN, got {recorded}"
        );

        let snapshot_id = deliver(case, &emulator).await;
        let info = await_settled_delivery(case, &emulator).await;
        assert_eq!(
            info["lastStatus"],
            json!(SUCCESS),
            "case {case:?}: a delivery that wrote its object reports {SUCCESS}, got {info}"
        );
        for member in ["lastErrorCode", "lastErrorMessage"] {
            assert!(
                info.get(member).is_none(),
                "case {case:?}: a delivery that succeeded reports no {member}, got {info}"
            );
        }
        let attempt = info["lastAttemptTime"].as_f64().unwrap_or_default();
        let successful = info["lastSuccessfulTime"].as_f64().unwrap_or_default();
        assert!(
            attempt > 0.0 && successful >= attempt,
            "case {case:?}: the delivery succeeded at or after it was attempted, got \
             {info}"
        );

        // ONE object, under the prefix the channel configures, naming the snapshot the
        // call answered with.
        let keys = await_delivered_key(case, &emulator, DELIVERY_BUCKET, &snapshot_id).await;
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: one delivery writes one object, got {keys:?}"
        );
        let key = keys[0].clone();
        let expected_root = format!("{expected_prefix}AWSLogs/{ACCOUNT_ID}/Config/{REGION}/");
        assert!(
            key.starts_with(&expected_root),
            "case {case:?}: expected a key under {expected_root:?}, got {key:?}"
        );

        // The object IS the snapshot: gzip-framed JSON carrying the configuration items
        // the recorder captured, the observed bucket's among them.
        let body = delivered_body(case, &emulator, DELIVERY_BUCKET, &key).await;
        let snapshot = gunzipped_snapshot(case, &body);
        assert_eq!(
            snapshot["configSnapshotId"],
            json!(snapshot_id),
            "case {case:?}: the snapshot names the delivery it was written for, got \
             {snapshot}"
        );
        let items = snapshot["configurationItems"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the snapshot carries configurationItems, got {snapshot}")
            });
        let observed = items
            .iter()
            .find(|item| item["resourceId"] == json!(OBSERVED_BUCKET))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the snapshot carries the recorded bucket, got {snapshot}")
            });
        assert!(
            observed["ARN"]
                .as_str()
                .is_some_and(|arn| arn.ends_with(OBSERVED_BUCKET)),
            "case {case:?}: the delivered item carries the bucket's ARN, got {observed}"
        );
        assert_eq!(
            observed["awsRegion"],
            json!(REGION),
            "case {case:?}: the delivered item names the Region it was recorded in, got \
             {observed}"
        );
        assert_eq!(
            observed["resourceType"],
            json!(BUCKET_RESOURCE_TYPE),
            "case {case:?}: the delivered item names the type it was recorded under, got \
             {observed}"
        );

        // Reading the status again reports the same settled delivery: a read delivers
        // nothing of its own.
        let again = snapshot_info(case, &emulator).await;
        assert_eq!(
            info, again,
            "case {case:?}: reading the status changes nothing a later caller sees, got \
             {info} then {again}"
        );
        let unchanged = delivered_keys(case, &emulator, DELIVERY_BUCKET).await;
        assert_eq!(
            unchanged, keys,
            "case {case:?}: reading the status writes no further object, got {unchanged:?}"
        );

        // And nothing is claimed for the delivery nobody made: no operation of this API
        // delivers a configuration history, so that export reports when the next
        // delivery is due and no outcome at all.
        let history = channel_status(case, &emulator).await["configHistoryDeliveryInfo"].clone();
        assert!(
            history["nextDeliveryTime"].is_number(),
            "case {case:?}: the configuration history reports its schedule, got {history}"
        );
        for member in ["lastStatus", "lastAttemptTime", "lastSuccessfulTime"] {
            assert!(
                history.get(member).is_none(),
                "case {case:?}: no configuration history was delivered, so it reports no \
                 {member}, got {history}"
            );
        }
    }
}

#[tokio::test]
async fn every_scheduled_delivery_writes_its_own_object_and_keeps_the_ones_before_it() {
    // Rows differ along how many deliveries one channel is asked for: each is written as
    // its own object, named for its own snapshot id, and none replaces another.
    for scheduled in [1_usize, 2, 3] {
        let case = format!("{scheduled} deliveries");
        let emulator = Emulator::new();
        emulator.create_delivery_bucket(DELIVERY_BUCKET).await;
        recording_through(&emulator, DELIVERY_BUCKET, None).await;
        emulator.create_bucket(OBSERVED_BUCKET, &[]).await;
        await_recorded_item(&case, &emulator, OBSERVED_BUCKET).await;

        let mut ids: Vec<String> = Vec::new();
        for _ in 0..scheduled {
            let id = deliver(&case, &emulator).await;
            // Each delivery is awaited before the next is scheduled: the channel reports
            // the LAST delivery, so an id is only observable while it is that one.
            await_delivered_key(&case, &emulator, DELIVERY_BUCKET, &id).await;
            ids.push(id);
        }

        let keys = delivered_keys(&case, &emulator, DELIVERY_BUCKET).await;
        assert_eq!(
            keys.len(),
            scheduled,
            "case {case:?}: every delivery writes its own object, got {keys:?}"
        );
        for id in &ids {
            assert!(
                keys.iter().any(|key| key.contains(id)),
                "case {case:?}: the object delivered for {id} is still there, got {keys:?}"
            );
        }

        // The channel reports the LAST delivery, and reports it as delivered.
        let info = await_settled_delivery(&case, &emulator).await;
        assert_eq!(
            info["lastStatus"],
            json!(SUCCESS),
            "case {case:?}: the last delivery wrote its object, got {info}"
        );
        let last = ids.last().map(String::as_str).unwrap_or_default();
        let body = delivered_body(
            &case,
            &emulator,
            DELIVERY_BUCKET,
            &keys
                .iter()
                .find(|key| key.contains(last))
                .cloned()
                .unwrap_or_default(),
        )
        .await;
        assert_eq!(
            gunzipped_snapshot(&case, &body)["configSnapshotId"],
            json!(last),
            "case {case:?}: the last object delivered names the last snapshot id"
        );
    }
}
