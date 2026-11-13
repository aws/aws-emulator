// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ExportServerlessCacheSnapshot`: the export of a stored
//! serverless cache snapshot's data set into an Amazon S3 bucket.
//!
//! Every ElastiCache assertion goes over the wire, through the same router the
//! production emulator mounts, using the shared `harness` request builder and XML
//! reader — and the object the export writes is read back through Amazon S3's OWN
//! `GetObject`, the service that owns buckets and objects, so an export this
//! service reports is only accepted here when the owner of objects really holds
//! what it reported.
//!
//! The rule under test: an export writes the named snapshot's data set into the
//! bucket the request names, as the object keyed by the snapshot's stored
//! (lowercased) name, and answers with the persisted snapshot, which the export
//! leaves exactly as it stands. It is refused, and writes nothing, when the
//! account holds no snapshot under the name (the modelled not-found fault, HTTP
//! 404), when the snapshot has not settled yet and so has no finished data set
//! (the modelled snapshot-state fault, HTTP 400), and when the object store will
//! not take the object into the bucket — one it holds nothing under, or one it
//! holds in another Region, since "the Amazon S3 bucket must also be in same
//! region as the snapshot" (HTTP 400, `InvalidParameterValue`).
//!
//! The tests are tables: each row is one input along an axis the rule
//! generalises over (the engine the source cache runs, the case and the length of
//! the snapshot's name, the form of the bucket name, the Region the whole flow
//! runs in, and — for the refusals — every way an export cannot be performed), so
//! no single recorded request is what makes them pass.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// A second Region, for the row that runs the whole flow outside the Region every
/// other row signs for, and for the bucket that lives outside the snapshot's.
const OTHER_REGION: &str = "eu-west-1";

/// One row of the export table: the cache to snapshot, the snapshot taken of it,
/// and the bucket its data set is exported into.
#[derive(Debug, Clone, Copy)]
struct ExportCase {
    /// What the row is called in a failure message.
    case: &'static str,
    /// The Region the cache, the snapshot and the bucket all live in.
    region: &'static str,
    /// The `ServerlessCacheName` the create sends.
    cache_request_name: &'static str,
    /// The engine the source cache runs.
    engine: &'static str,
    /// The `ServerlessCacheSnapshotName` the create and the export send, and the
    /// identity it is stored under — the name "is stored as a lowercase string".
    snapshot_request_name: &'static str,
    snapshot_stored_name: &'static str,
    /// The `S3BucketName` the export names.
    bucket: &'static str,
}

/// A 63-character snapshot name: `ServerlessCacheSnapshotName` "must be between 1
/// and 255 characters", so a long name is as good a name as a short one — and the
/// object key the export writes under follows the name whatever its length.
const LONG_SNAPSHOT_NAME: &str = "a-serverless-cache-snapshot-name-of-sixty-three-characters-long";

/// The axis: the engine the source cache runs, the case and the length of the
/// snapshot's name, the form of the destination bucket's name, and the Region the
/// flow runs in. The export rule holds over all of them.
const CASES: &[ExportCase] = &[
    ExportCase {
        case: "redis, plain names",
        region: REGION,
        cache_request_name: "redis-export-cache",
        engine: "redis",
        snapshot_request_name: "redis-export-snapshot",
        snapshot_stored_name: "redis-export-snapshot",
        bucket: "elasticache-exports-redis",
    },
    ExportCase {
        case: "valkey, the snapshot named in mixed case and a dotted bucket",
        region: REGION,
        cache_request_name: "Valkey-Export-Cache",
        engine: "valkey",
        snapshot_request_name: "Valkey-Export-Snapshot",
        snapshot_stored_name: "valkey-export-snapshot",
        bucket: "elasticache.exports.valkey",
    },
    ExportCase {
        case: "memcached, a single-character snapshot name and the shortest bucket name",
        region: REGION,
        cache_request_name: "memcached-export-cache",
        engine: "memcached",
        snapshot_request_name: "X",
        snapshot_stored_name: "x",
        bucket: "ecx",
    },
    ExportCase {
        case: "redis in another Region, with a 63-character snapshot name",
        region: OTHER_REGION,
        cache_request_name: "other-region-export-cache",
        engine: "redis",
        snapshot_request_name: LONG_SNAPSHOT_NAME,
        snapshot_stored_name: LONG_SNAPSHOT_NAME,
        bucket: "elasticache-exports-other-region",
    },
];

/// Create one serverless cache in `region` and insist the create itself
/// succeeded.
async fn create_cache(emulator: &Emulator, case: &ExportCase) {
    let created = emulator
        .call_in(
            case.region,
            "CreateServerlessCache",
            &params(&[
                ("ServerlessCacheName", case.cache_request_name),
                ("Engine", case.engine),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {}: CreateServerlessCache must answer 200, got {} {}",
        case.case, created.status, created.body
    );
}

/// Take the row's snapshot of the row's cache and insist the create succeeded.
async fn create_snapshot(emulator: &Emulator, case: &ExportCase) {
    let created = emulator
        .call_in(
            case.region,
            "CreateServerlessCacheSnapshot",
            &params(&[
                ("ServerlessCacheSnapshotName", case.snapshot_request_name),
                ("ServerlessCacheName", case.cache_request_name),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {}: CreateServerlessCacheSnapshot must answer 200, got {} {}",
        case.case, created.status, created.body
    );
}

/// Create `bucket` in `region` through Amazon S3's OWN `CreateBucket`, so the
/// destination an export resolves is a bucket that service really holds.
///
/// A regional endpoint admits the create only when it states its own location
/// constraint, and the one Region S3 reports no constraint for takes none.
async fn create_bucket(emulator: &Emulator, case: &str, bucket: &str, region: &str) {
    let mut request = emulator.s3(region).create_bucket().bucket(bucket);
    if region != REGION {
        request = request.create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        );
    }
    request
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case}: Amazon S3 creates the bucket {bucket}: {error}"));
}

/// The bytes Amazon S3 holds under `key` in `bucket`, or `None` when it holds no
/// such object.
async fn stored_object(emulator: &Emulator, region: &str, bucket: &str, key: &str) -> Option<Vec<u8>> {
    let answer = emulator
        .s3(region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .ok()?;
    let body = answer
        .body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("the object {bucket}/{key} can be read: {error}"));
    Some(body.into_bytes().to_vec())
}

/// `ExportServerlessCacheSnapshot` with the two members it takes.
async fn export(emulator: &Emulator, region: &str, snapshot_name: &str, bucket: &str) -> Reply {
    emulator
        .call_in(
            region,
            "ExportServerlessCacheSnapshot",
            &params(&[
                ("ServerlessCacheSnapshotName", snapshot_name),
                ("S3BucketName", bucket),
            ]),
        )
        .await
}

/// Wait for the row's cache to reach the status a snapshot of it needs.
///
/// A create leaves a serverless cache `creating`, and the handlers never sleep, so the
/// suite looks again until the CONDITION holds rather than waiting a fixed time: what it
/// asserts then does not depend on how fast or how loaded the host is.
async fn await_settled_cache(emulator: &Emulator, case: &ExportCase) {
    poll_until(
        emulator,
        case.region,
        case.case,
        &format!(
            "the serverless cache {} settling at available",
            case.cache_request_name
        ),
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", case.cache_request_name)]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
}

/// Wait for the row's snapshot to reach the status an export of it needs; see
/// [`await_settled_cache`] for why this is a poll and not a sleep.
async fn await_settled_snapshot(emulator: &Emulator, case: &ExportCase) {
    poll_until(
        emulator,
        case.region,
        case.case,
        &format!(
            "the serverless cache snapshot {} settling at available",
            case.snapshot_request_name
        ),
        "DescribeServerlessCacheSnapshots",
        &params(&[(
            "ServerlessCacheSnapshotName",
            case.snapshot_request_name,
        )]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
}

/// Read one snapshot back by name, asserting the describe itself succeeded.
async fn describe_snapshot(emulator: &Emulator, case: &ExportCase) -> Reply {
    let described = emulator
        .call_in(
            case.region,
            "DescribeServerlessCacheSnapshots",
            &params(&[("ServerlessCacheSnapshotName", case.snapshot_request_name)]),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {}: DescribeServerlessCacheSnapshots must answer 200, got {} {}",
        case.case, described.status, described.body
    );
    described
}

/// The messages this service recorded under the snapshot of `case`.
async fn snapshot_event_messages(emulator: &Emulator, case: &ExportCase) -> Vec<String> {
    let events = emulator
        .call_in(
            case.region,
            "DescribeEvents",
            &params(&[
                ("SourceIdentifier", case.snapshot_stored_name),
                ("SourceType", state::SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT),
            ]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "case {}: DescribeEvents must answer 200, got {} {}",
        case.case, events.status, events.body
    );
    events
        .all("Message")
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn every_stored_snapshot_is_exported_into_the_bucket_the_request_names() {
    let emulator = Emulator::with_s3();

    for case in CASES {
        create_cache(&emulator, case).await;
        create_bucket(&emulator, case.case, case.bucket, case.region).await;
    }
    // A snapshot is taken of a settled cache, and exported from a settled
    // snapshot, so each row waits for those two conditions in turn.
    for case in CASES {
        await_settled_cache(&emulator, case).await;
        create_snapshot(&emulator, case).await;
    }
    for case in CASES {
        await_settled_snapshot(&emulator, case).await;
    }

    for case in CASES {
        let before = describe_snapshot(&emulator, case).await;
        let exported = export(&emulator, case.region, case.snapshot_request_name, case.bucket).await;
        assert_eq!(
            exported.status, 200,
            "case {}: ExportServerlessCacheSnapshot must answer 200, got {} {}",
            case.case, exported.status, exported.body
        );
        // The export answers with the PERSISTED snapshot: its stored name, the
        // status it is in and the configuration of the cache it was taken of.
        for (member, expected) in [
            ("ServerlessCacheSnapshotName", case.snapshot_stored_name),
            ("Status", state::STATUS_AVAILABLE),
            ("SnapshotType", state::SERVERLESS_SNAPSHOT_TYPE_MANUAL),
            ("Engine", case.engine),
        ] {
            assert_eq!(
                exported.text(member),
                Some(expected),
                "case {}: expected {member} {expected:?}, got {:?} ({})",
                case.case,
                exported.text(member),
                exported.body
            );
        }
        assert_eq!(
            exported.text("CreateTime"),
            before.text("CreateTime"),
            "case {}: the export must report the snapshot as stored, got {}",
            case.case,
            exported.body
        );

        // The object the export wrote is read back from the service that OWNS
        // objects, under the key the snapshot's STORED name gives it.
        let key = format!("{}.rdb", case.snapshot_stored_name);
        let body = stored_object(&emulator, case.region, case.bucket, &key)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {}: Amazon S3 must hold the exported snapshot at {}/{key}",
                    case.case, case.bucket
                )
            });
        // What was written is a Valkey/Redis OSS RDB snapshot file, which is the
        // form an exported snapshot takes in a bucket.
        assert!(
            body.starts_with(b"REDIS"),
            "case {}: the object {}/{key} must be an RDB snapshot file, got {} byte(s) {:?}",
            case.case,
            case.bucket,
            body.len(),
            String::from_utf8_lossy(&body[..body.len().min(16)])
        );

        // The export is one of the events this service records, under the
        // snapshot itself, naming where the data set went.
        let messages = snapshot_event_messages(&emulator, case).await;
        assert!(
            messages
                .iter()
                .any(|message| message.contains("exported") && message.contains(case.bucket)),
            "case {}: the export must be recorded in the event log, got {messages:?}",
            case.case
        );

        // Nothing about the snapshot changed: an export copies the data set out
        // and leaves the resource as it stands.
        let after = describe_snapshot(&emulator, case).await;
        for member in [
            "ServerlessCacheSnapshotName",
            "Status",
            "SnapshotType",
            "ARN",
            "CreateTime",
        ] {
            assert_eq!(
                after.text(member),
                before.text(member),
                "case {}: the export must leave {member} alone, got {:?} where it was {:?}",
                case.case,
                after.text(member),
                before.text(member)
            );
        }
    }

    // The bucket is a parameter, not a property of the snapshot: the same
    // snapshot exported again lands in whichever bucket the second call names,
    // and the first object stays where it was.
    let case = &CASES[0];
    let second_bucket = "elasticache-exports-second-destination";
    create_bucket(&emulator, case.case, second_bucket, case.region).await;
    let again = export(
        &emulator,
        case.region,
        case.snapshot_request_name,
        second_bucket,
    )
    .await;
    assert_eq!(
        again.status, 200,
        "case {}: a second export must answer 200, got {} {}",
        case.case, again.status, again.body
    );
    let key = format!("{}.rdb", case.snapshot_stored_name);
    for bucket in [case.bucket, second_bucket] {
        assert!(
            stored_object(&emulator, case.region, bucket, &key)
                .await
                .is_some(),
            "case {}: Amazon S3 must hold the exported snapshot at {bucket}/{key}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_export_that_cannot_be_performed_is_refused_and_writes_nothing() {
    let emulator = Emulator::with_s3();
    let case = &CASES[0];
    let bucket = "elasticache-exports-refusals";
    let other_region_bucket = "elasticache-exports-refusals-abroad";
    create_cache(&emulator, case).await;
    create_bucket(&emulator, case.case, bucket, case.region).await;
    create_bucket(&emulator, case.case, other_region_bucket, OTHER_REGION).await;
    await_settled_cache(&emulator, case).await;

    // A name the account holds no snapshot under: the modelled not-found fault,
    // whichever bucket the request names, and whatever case the name is in.
    for missing in ["no-such-snapshot", "No-Such-Snapshot", "x"] {
        let refused = export(&emulator, case.region, missing, bucket).await;
        assert_eq!(
            refused.status, 404,
            "case {missing}: expected HTTP 404, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ServerlessCacheSnapshotNotFoundFault",
            "case {missing}: expected the not-found code, got {:?} ({})",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            // The class's own fixed sentence: it states only that no snapshot is held
            // under the specified name, and names nothing the request sent.
            "Serverless cache snapshot with specified name does not exists.",
            "case {missing}: the fault must state the class's own sentence, got {:?}",
            refused.error_message()
        );
    }

    // A snapshot that has not settled yet has no finished data set to export:
    // the modelled snapshot-state fault, naming the state it is in.
    create_snapshot(&emulator, case).await;
    let unsettled = export(&emulator, case.region, case.snapshot_request_name, bucket).await;
    assert_eq!(
        unsettled.status, 400,
        "case {}: expected HTTP 400 while the snapshot is {}, got {} {}",
        case.case,
        state::STATUS_CREATING,
        unsettled.status,
        unsettled.body
    );
    assert_eq!(
        unsettled.error_code(),
        "InvalidServerlessCacheSnapshotStateFault",
        "case {}: expected the snapshot-state fault, got {:?} ({})",
        case.case,
        unsettled.error_code(),
        unsettled.body
    );
    for fragment in [case.snapshot_stored_name, state::STATUS_CREATING] {
        assert!(
            unsettled.error_message().contains(fragment),
            "case {}: the state fault must mention {fragment:?}, got {:?}",
            case.case,
            unsettled.error_message()
        );
    }

    // Once the snapshot has settled it is exportable, so the two bucket refusals
    // below are about the BUCKET and not about the snapshot.
    await_settled_snapshot(&emulator, case).await;
    let key = format!("{}.rdb", case.snapshot_stored_name);

    // A bucket the object store holds nothing under takes no export.
    for absent in ["no-such-export-bucket", "another.absent.bucket"] {
        let refused = export(&emulator, case.region, case.snapshot_request_name, absent).await;
        assert_eq!(
            refused.status, 400,
            "case {absent}: expected HTTP 400 for a bucket Amazon S3 does not hold, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {absent}: expected the bad-parameter code, got {:?} ({})",
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.error_message().contains(absent),
            "case {absent}: the refusal must name the bucket, got {:?}",
            refused.error_message()
        );
    }

    // A bucket the object store holds in ANOTHER Region is not a destination
    // this snapshot can be exported to: "the Amazon S3 bucket must also be in
    // same region as the snapshot".
    let abroad = export(
        &emulator,
        case.region,
        case.snapshot_request_name,
        other_region_bucket,
    )
    .await;
    assert_eq!(
        abroad.status, 400,
        "case {}: expected HTTP 400 for a bucket in {OTHER_REGION}, got {} {}",
        case.case, abroad.status, abroad.body
    );
    assert_eq!(
        abroad.error_code(),
        "InvalidParameterValue",
        "case {}: expected the bad-parameter code, got {:?} ({})",
        case.case,
        abroad.error_code(),
        abroad.body
    );
    for fragment in [other_region_bucket, OTHER_REGION, case.region] {
        assert!(
            abroad.error_message().contains(fragment),
            "case {}: the refusal must mention {fragment:?}, got {:?}",
            case.case,
            abroad.error_message()
        );
    }
    // And it wrote nothing: a refused export leaves no object behind in the
    // bucket it named.
    assert_eq!(
        stored_object(&emulator, OTHER_REGION, other_region_bucket, &key).await,
        None,
        "case {}: a refused export must leave {other_region_bucket}/{key} empty",
        case.case
    );
    // Nor did any refusal record an export of the snapshot.
    let messages = snapshot_event_messages(&emulator, case).await;
    assert!(
        !messages.iter().any(|message| message.contains("exported")),
        "case {}: no refused export may be recorded as one, got {messages:?}",
        case.case
    );

    // The same snapshot, into a bucket in its own Region, IS exported — so the
    // refusals above are the rule's, not a blanket refusal.
    let served = export(&emulator, case.region, case.snapshot_request_name, bucket).await;
    assert_eq!(
        served.status, 200,
        "case {}: the export into {bucket} must answer 200, got {} {}",
        case.case, served.status, served.body
    );
    assert!(
        stored_object(&emulator, case.region, bucket, &key)
            .await
            .is_some(),
        "case {}: Amazon S3 must hold the exported snapshot at {bucket}/{key}",
        case.case
    );
}
